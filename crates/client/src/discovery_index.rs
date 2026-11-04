//! Signed public-network discovery without coupling the index control plane to
//! any joined network's data plane.
//!
//! The index wire protocol is intentionally small:
//! - `GET  {index_url}/v1/discovery/networks`
//! - `PUT  {index_url}/v1/discovery/networks/{network_id}`
//!
//! Both directions carry [`NetworkDiscoveryRecord`] values. A client persists
//! only public signed records, their digests, revision watermarks and stable
//! error categories. Search text, invitation credentials and tickets are never
//! accepted by this module and therefore cannot be persisted or logged here.

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fuckipv6_common::domain::{
    ContentHash, DomainError, ExitContributionObligation, MembershipTerms,
    NetworkDiscoveryDocument, NetworkDiscoveryRecord, NetworkJoinMode, NodeId, OperationId,
    NETWORK_DISCOVERY_VALIDITY_MS,
};
use fuckipv6_common::Identity;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::{watch, Mutex as AsyncMutex};

use crate::config_manager::{write_json_atomic, ConfigManager};
use crate::node::NodeStateStore;

const DISCOVERY_INDEX_FILE: &str = "discovery_index.json";
const DISCOVERY_INDEX_SCHEMA_VERSION: u16 = 1;
const DISCOVERY_INDEX_WIRE_SCHEMA_VERSION: u16 = 1;
const MAX_INDEX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const INDEX_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const INDEX_MAX_ATTEMPTS: usize = 3;
const INDEX_RETRY_BASE_DELAY: Duration = Duration::from_millis(250);
const INDEX_RETRY_MAX_DELAY: Duration = Duration::from_secs(2);

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Stable discovery-index states consumed by every UI adapter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeDiscoveryIndexState {
    #[default]
    Idle,
    Pending,
    Ready,
    Degraded,
    Failed,
    Unsupported,
}

impl NodeDiscoveryIndexState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Degraded => "degraded",
            Self::Failed => "failed",
            Self::Unsupported => "unsupported",
        }
    }
}

/// Stable, non-secret categories for persisted request failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeDiscoveryIndexErrorCategory {
    Offline,
    Timeout,
    Tls,
    Signature,
    PartialCorruption,
    IncompatibleVersion,
    Unsupported,
    Rejected,
    InvalidResponse,
    Storage,
    Server,
    Cancelled,
}

impl NodeDiscoveryIndexErrorCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Offline => "offline",
            Self::Timeout => "timeout",
            Self::Tls => "tls",
            Self::Signature => "signature",
            Self::PartialCorruption => "partial_corruption",
            Self::IncompatibleVersion => "incompatible_version",
            Self::Unsupported => "unsupported",
            Self::Rejected => "rejected",
            Self::InvalidResponse => "invalid_response",
            Self::Storage => "storage",
            Self::Server => "server",
            Self::Cancelled => "cancelled",
        }
    }

    pub const fn retryable(self) -> bool {
        matches!(
            self,
            Self::Offline
                | Self::Timeout
                | Self::Tls
                | Self::PartialCorruption
                | Self::InvalidResponse
                | Self::Server
        )
    }

    pub const fn message_key(self) -> &'static str {
        match self {
            Self::Offline => "discovery.error.offline",
            Self::Timeout => "discovery.error.timeout",
            Self::Tls => "discovery.error.tls",
            Self::Signature => "discovery.error.signature",
            Self::PartialCorruption => "discovery.error.partial_corruption",
            Self::IncompatibleVersion => "discovery.error.incompatible_version",
            Self::Unsupported => "discovery.error.unsupported",
            Self::Rejected => "discovery.error.rejected",
            Self::InvalidResponse => "discovery.error.invalid_response",
            Self::Storage => "discovery.error.storage",
            Self::Server => "discovery.error.server",
            Self::Cancelled => "discovery.error.cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeDiscoveryIndexError {
    pub category: NodeDiscoveryIndexErrorCategory,
    pub retryable: bool,
}

impl NodeDiscoveryIndexError {
    pub const fn new(category: NodeDiscoveryIndexErrorCategory) -> Self {
        Self {
            category,
            retryable: category.retryable(),
        }
    }

    pub const fn message_key(&self) -> &'static str {
        self.category.message_key()
    }
}

/// One signed record accepted from the index or supplied by a join flow.
///
/// `discoverable` is false for expired, malformed or revoked records. The raw
/// signed record is retained so revocations and revision watermarks survive a
/// restart and can reject later rollback attempts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeDiscoveryIndexSource {
    #[default]
    None,
    Cache,
    Network,
    Join,
}

impl NodeDiscoveryIndexSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Cache => "cache",
            Self::Network => "network",
            Self::Join => "join",
        }
    }

    pub const fn message_key(self) -> &'static str {
        match self {
            Self::None => "discovery.source.none",
            Self::Cache => "discovery.source.cache",
            Self::Network => "discovery.source.network",
            Self::Join => "discovery.source.join",
        }
    }
}

/// Aggregate-only discovery telemetry. It deliberately has no URL, response
/// body, search text, record name or free-form error text.
#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeDiscoveryIndexMetrics {
    pub request_attempts: u64,
    pub requests_succeeded: u64,
    pub requests_failed: u64,
    pub retries_scheduled: u64,
    pub cancellations: u64,
    pub records_accepted: u64,
    pub records_rejected: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_request_duration_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeDiscoveryIndexRecordSnapshot {
    pub record: NetworkDiscoveryRecord,
    pub digest: String,
    pub accepted_at_unix_ms: u64,
    pub source: NodeDiscoveryIndexSource,
    pub discoverable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invalid_reason: Option<String>,
}

/// Public discovery-index status included in the canonical node snapshot.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeDiscoveryIndexSnapshot {
    pub state: NodeDiscoveryIndexState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_attempt_unix_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_success_unix_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<NodeDiscoveryIndexError>,
    pub source: NodeDiscoveryIndexSource,
    pub rejected_record_count: usize,
    pub records: Vec<NodeDiscoveryIndexRecordSnapshot>,
    pub metrics: NodeDiscoveryIndexMetrics,
    /// The discovery index is a control-plane convenience. Its state never
    /// changes any joined network's lifecycle or forwarding data plane.
    pub data_plane_affected: bool,
    /// The discovery index client has no search-query field. This explicit flag
    /// documents the privacy contract for generated UI and Android clients.
    pub search_history_stored: bool,
}

impl Default for NodeDiscoveryIndexSnapshot {
    fn default() -> Self {
        Self {
            state: NodeDiscoveryIndexState::Idle,
            index_url: None,
            last_attempt_unix_ms: None,
            last_success_unix_ms: None,
            last_error: None,
            source: NodeDiscoveryIndexSource::None,
            rejected_record_count: 0,
            records: Vec::new(),
            metrics: NodeDiscoveryIndexMetrics::default(),
            data_plane_affected: false,
            search_history_stored: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryIndexError {
    pub category: NodeDiscoveryIndexErrorCategory,
    /// Stable product-facing reason for join-policy failures. Request and
    /// transport failures leave this unset.
    pub reason_key: Option<&'static str>,
}

impl DiscoveryIndexError {
    fn new(category: NodeDiscoveryIndexErrorCategory) -> Self {
        Self {
            category,
            reason_key: None,
        }
    }

    fn join(reason_key: &'static str) -> Self {
        Self {
            category: NodeDiscoveryIndexErrorCategory::Rejected,
            reason_key: Some(reason_key),
        }
    }

    pub const fn retryable(&self) -> bool {
        self.category.retryable()
    }

    pub const fn message_key(&self) -> &'static str {
        match self.reason_key {
            Some(reason_key) => reason_key,
            None => self.category.message_key(),
        }
    }

    fn from_domain(error: DomainError) -> Self {
        let category = match error.code() {
            "domain.discovery_unsupported_version" => {
                NodeDiscoveryIndexErrorCategory::IncompatibleVersion
            }
            "domain.invalid_signature" | "domain.discovery_publisher_mismatch" => {
                NodeDiscoveryIndexErrorCategory::Signature
            }
            "domain.discovery_profile_unsigned"
            | "domain.discovery_profile_expired"
            | "domain.discovery_expired"
            | "domain.discovery_rollback"
            | "domain.discovery_conflict"
            | "domain.discovery_already_revoked" => NodeDiscoveryIndexErrorCategory::Rejected,
            _ => NodeDiscoveryIndexErrorCategory::InvalidResponse,
        };
        Self::new(category)
    }
}

impl fmt::Display for NodeDiscoveryIndexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.category.as_str())
    }
}

/// Result of one untrusted index fetch. Malformed records are counted instead
/// of discarding the entire response, allowing valid records to remain cached
/// while the projection is explicitly marked degraded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiscoveryIndexFetchResult {
    pub records: Vec<NetworkDiscoveryRecord>,
    pub malformed_record_count: usize,
}

/// Injectable transport used by tests and embedders. Production uses
/// [`HttpDiscoveryIndexTransport`]. The command layer verifies every record
/// after transport, so a transport is never trusted as an authenticity source.
pub trait DiscoveryIndexTransport: Send + Sync + 'static {
    fn fetch<'a>(
        &'a self,
        index_url: &'a str,
    ) -> BoxFuture<'a, Result<DiscoveryIndexFetchResult, DiscoveryIndexError>>;

    fn publish<'a>(
        &'a self,
        index_url: &'a str,
        record: &'a NetworkDiscoveryRecord,
    ) -> BoxFuture<'a, Result<(), DiscoveryIndexError>>;
}

#[derive(Clone)]
pub struct HttpDiscoveryIndexTransport {
    client: reqwest::Client,
}

impl HttpDiscoveryIndexTransport {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(INDEX_REQUEST_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { client }
    }
}

impl Default for HttpDiscoveryIndexTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Serialize, Deserialize)]
struct DiscoveryIndexWireResponse {
    schema_version: u16,
    records: Vec<serde_json::Value>,
}

impl DiscoveryIndexTransport for HttpDiscoveryIndexTransport {
    fn fetch<'a>(
        &'a self,
        index_url: &'a str,
    ) -> BoxFuture<'a, Result<DiscoveryIndexFetchResult, DiscoveryIndexError>> {
        Box::pin(async move {
            let endpoint = format!("{}/v1/discovery/networks", index_url.trim_end_matches('/'));
            let response = self.client.get(endpoint).send().await.map_err(http_error)?;
            let response = require_success(response)?;
            if response
                .content_length()
                .is_some_and(|length| length > MAX_INDEX_RESPONSE_BYTES as u64)
            {
                return Err(DiscoveryIndexError::new(
                    NodeDiscoveryIndexErrorCategory::InvalidResponse,
                ));
            }
            let body = response.bytes().await.map_err(http_error)?;
            if body.len() > MAX_INDEX_RESPONSE_BYTES {
                return Err(DiscoveryIndexError::new(
                    NodeDiscoveryIndexErrorCategory::InvalidResponse,
                ));
            }
            let response: DiscoveryIndexWireResponse =
                serde_json::from_slice(&body).map_err(|_| {
                    DiscoveryIndexError::new(NodeDiscoveryIndexErrorCategory::InvalidResponse)
                })?;
            if response.schema_version != DISCOVERY_INDEX_WIRE_SCHEMA_VERSION {
                return Err(DiscoveryIndexError::new(
                    NodeDiscoveryIndexErrorCategory::IncompatibleVersion,
                ));
            }
            let mut records = Vec::with_capacity(response.records.len());
            let mut malformed_record_count = 0usize;
            for value in response.records {
                match serde_json::from_value::<NetworkDiscoveryRecord>(value) {
                    Ok(record) => records.push(record),
                    Err(_) => malformed_record_count += 1,
                }
            }
            Ok(DiscoveryIndexFetchResult {
                records,
                malformed_record_count,
            })
        })
    }

    fn publish<'a>(
        &'a self,
        index_url: &'a str,
        record: &'a NetworkDiscoveryRecord,
    ) -> BoxFuture<'a, Result<(), DiscoveryIndexError>> {
        Box::pin(async move {
            let endpoint = format!(
                "{}/v1/discovery/networks/{}",
                index_url.trim_end_matches('/'),
                record.document.network_id.as_str()
            );
            let body = serde_json::to_vec(record).map_err(|_| {
                DiscoveryIndexError::new(NodeDiscoveryIndexErrorCategory::InvalidResponse)
            })?;
            let response = self
                .client
                .put(endpoint)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body)
                .send()
                .await
                .map_err(http_error)?;
            require_success(response)?;
            Ok(())
        })
    }
}

fn http_error(error: reqwest::Error) -> DiscoveryIndexError {
    let category = if error.is_timeout() {
        NodeDiscoveryIndexErrorCategory::Timeout
    } else if is_tls_error(&error) {
        NodeDiscoveryIndexErrorCategory::Tls
    } else {
        NodeDiscoveryIndexErrorCategory::Offline
    };
    DiscoveryIndexError::new(category)
}

fn is_tls_error(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut source = error.source();
    while let Some(error) = source {
        if error.downcast_ref::<rustls::Error>().is_some() {
            return true;
        }
        source = error.source();
    }
    false
}

fn require_success(response: reqwest::Response) -> Result<reqwest::Response, DiscoveryIndexError> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status().as_u16();
    let category = match status {
        404 | 405 | 501 => NodeDiscoveryIndexErrorCategory::Unsupported,
        500..=599 => NodeDiscoveryIndexErrorCategory::Server,
        _ => NodeDiscoveryIndexErrorCategory::Rejected,
    };
    Err(DiscoveryIndexError::new(category))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedDiscoveryIndex {
    #[serde(default = "default_schema_version")]
    schema_version: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    index_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_attempt_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_success_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_error: Option<NodeDiscoveryIndexErrorCategory>,
    #[serde(default)]
    source: NodeDiscoveryIndexSource,
    #[serde(default)]
    rejected_record_count: usize,
    #[serde(default)]
    metrics: NodeDiscoveryIndexMetrics,
    #[serde(default)]
    records: BTreeMap<String, PersistedDiscoveryRecord>,
    #[serde(skip)]
    state: NodeDiscoveryIndexState,
}

fn default_schema_version() -> u16 {
    DISCOVERY_INDEX_SCHEMA_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedDiscoveryRecord {
    record: NetworkDiscoveryRecord,
    digest: String,
    accepted_at_unix_ms: u64,
    #[serde(default)]
    source: NodeDiscoveryIndexSource,
}

/// Canonical pre-join acknowledgement material. Every mutable governance or
/// network-routing disclosure shown to the owner is included so confirmation
/// cannot silently apply a different revision.
#[derive(Serialize)]
struct JoinPreviewDigest<'a> {
    schema_version: u16,
    discovery_digest: &'a str,
    network_id: &'a str,
    network_name: &'a str,
    public_key: &'a str,
    owner: &'a NodeId,
    administrators: &'a [NodeId],
    discovery_revision: u64,
    network_policy_revision: u64,
    privacy_policy_revision: u64,
    terms_hash: &'a ContentHash,
    member_count: u32,
    member_limit: Option<u32>,
    exit_contribution: fuckipv6_common::domain::ExitContributionObligation,
    exit_contribution_scope: fuckipv6_common::domain::ExitContributionScope,
    capability_summary: &'a [fuckipv6_common::domain::CapabilityKind],
    endpoints: &'a [fuckipv6_common::domain::NetworkEndpoint],
    privacy_changes: &'a [String],
    issued_at_unix_ms: u64,
    expires_at_unix_ms: u64,
}

#[derive(Clone)]
pub struct DiscoveryIndexManager {
    store_path: PathBuf,
    transport: Arc<dyn DiscoveryIndexTransport>,
    operation: Arc<AsyncMutex<()>>,
    persisted: Arc<Mutex<PersistedDiscoveryIndex>>,
}

impl DiscoveryIndexManager {
    pub fn production(configs: &ConfigManager) -> Self {
        Self::new(
            configs.config_dir().join(DISCOVERY_INDEX_FILE),
            Arc::new(HttpDiscoveryIndexTransport::new()),
        )
    }

    pub fn new(store_path: PathBuf, transport: Arc<dyn DiscoveryIndexTransport>) -> Self {
        let (mut persisted, load_error) = load_persisted(&store_path);
        if let Some(category) = load_error {
            persisted.last_error = Some(category);
            persisted.state = state_for(category, !persisted.records.is_empty());
        } else if persisted.index_url.is_some() {
            persisted.state = state_for_loaded(&persisted);
        }
        Self {
            store_path,
            transport,
            operation: Arc::new(AsyncMutex::new(())),
            persisted: Arc::new(Mutex::new(persisted)),
        }
    }

    pub fn snapshot(&self) -> NodeDiscoveryIndexSnapshot {
        let persisted = self
            .persisted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        snapshot_from_persisted(&persisted, now_unix_ms())
    }

    pub async fn publish(
        &self,
        state: &NodeStateStore,
        configs: &ConfigManager,
        network_name: &str,
        index_url: &str,
        operation_id: Option<OperationId>,
    ) -> Result<NodeDiscoveryIndexSnapshot, DiscoveryIndexError> {
        self.mutate(
            state,
            configs,
            network_name,
            index_url,
            DiscoveryMutation::Publish,
            operation_id,
        )
        .await
    }

    pub async fn update(
        &self,
        state: &NodeStateStore,
        configs: &ConfigManager,
        network_name: &str,
        index_url: &str,
        operation_id: Option<OperationId>,
    ) -> Result<NodeDiscoveryIndexSnapshot, DiscoveryIndexError> {
        self.mutate(
            state,
            configs,
            network_name,
            index_url,
            DiscoveryMutation::Update,
            operation_id,
        )
        .await
    }

    pub async fn unpublish(
        &self,
        state: &NodeStateStore,
        configs: &ConfigManager,
        network_name: &str,
        index_url: &str,
        operation_id: Option<OperationId>,
    ) -> Result<NodeDiscoveryIndexSnapshot, DiscoveryIndexError> {
        self.mutate(
            state,
            configs,
            network_name,
            index_url,
            DiscoveryMutation::Unpublish,
            operation_id,
        )
        .await
    }

    pub async fn refresh(
        &self,
        state: &NodeStateStore,
        index_url: Option<&str>,
    ) -> Result<NodeDiscoveryIndexSnapshot, DiscoveryIndexError> {
        self.refresh_cancellable(state, index_url, None).await
    }

    /// Refresh with cooperative cancellation. Cancellation never turns an
    /// offline or partially loaded index into an empty result: the last
    /// verified projection is restored and only aggregate counters advance.
    pub async fn refresh_cancellable(
        &self,
        state: &NodeStateStore,
        index_url: Option<&str>,
        mut cancel: Option<watch::Receiver<bool>>,
    ) -> Result<NodeDiscoveryIndexSnapshot, DiscoveryIndexError> {
        if cancellation_requested(cancel.as_ref()) {
            return Err(DiscoveryIndexError::new(
                NodeDiscoveryIndexErrorCategory::Cancelled,
            ));
        }
        let _operation = self.operation.lock().await;
        let url = match index_url {
            Some(url) => validate_index_url(url)?,
            None => self
                .persisted
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .index_url
                .clone()
                .ok_or_else(|| {
                    DiscoveryIndexError::new(NodeDiscoveryIndexErrorCategory::Rejected)
                })?,
        };
        self.refresh_locked(state, url, cancel.as_mut()).await
    }

    pub async fn retry(
        &self,
        state: &NodeStateStore,
    ) -> Result<NodeDiscoveryIndexSnapshot, DiscoveryIndexError> {
        self.refresh(state, None).await
    }

    pub async fn retry_cancellable(
        &self,
        state: &NodeStateStore,
        cancel: Option<watch::Receiver<bool>>,
    ) -> Result<NodeDiscoveryIndexSnapshot, DiscoveryIndexError> {
        self.refresh_cancellable(state, None, cancel).await
    }

    /// Verify signed material supplied by a join flow against the local
    /// revision/revocation watermark, then persist the accepted watermark.
    /// Callers must still invoke this before starting the join itself.
    pub async fn accept_join_record(
        &self,
        state: &NodeStateStore,
        record: NetworkDiscoveryRecord,
    ) -> Result<NodeDiscoveryIndexSnapshot, DiscoveryIndexError> {
        let _operation = self.operation.lock().await;
        let now = now_unix_ms();
        record
            .validate_at(now)
            .map_err(DiscoveryIndexError::from_domain)?;
        if record.document.revoked_at_unix_ms.is_some() {
            return Err(DiscoveryIndexError::new(
                NodeDiscoveryIndexErrorCategory::Rejected,
            ));
        }
        let snapshot = self.accept_record_locked(record, now, false)?;
        state.replace_discovery_index(snapshot.clone()).await;
        Ok(snapshot)
    }

    /// Build the complete direct-join disclosure from the persisted signed
    /// record. This method is strictly read-only: it never advances the
    /// discovery watermark, timestamps, error state or accepted record.
    pub async fn preview_join_record(
        &self,
        network_id: &str,
        already_member: bool,
    ) -> Result<crate::node::NodeNetworkJoinPreview, DiscoveryIndexError> {
        let persisted = self
            .persisted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let accepted = persisted
            .records
            .get(network_id)
            .ok_or_else(|| DiscoveryIndexError::join("join.error.not_found"))?;
        preview_from_record(
            &accepted.record,
            &accepted.digest,
            already_member,
            now_unix_ms(),
        )
    }

    /// Re-read and verify the signed record used to submit an approval-based
    /// join request. This path is read-only, accepts only networks that require
    /// approval, and preserves the same expiry, revocation, capacity and
    /// transport checks as direct joining.
    pub async fn approval_join_record(
        &self,
        network_id: &str,
    ) -> Result<NetworkDiscoveryRecord, DiscoveryIndexError> {
        let persisted = self
            .persisted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let accepted = persisted
            .records
            .get(network_id)
            .ok_or_else(|| DiscoveryIndexError::join("join.error.not_found"))?;
        validate_approval_join_record(&accepted.record, now_unix_ms())
    }

    /// Re-read the exact accepted record and verify every acknowledgement field
    /// before the caller mutates local configuration or starts a runtime.
    pub async fn confirm_join_record(
        &self,
        network_id: &str,
        preview_digest: &str,
        network_policy_revision: u64,
        privacy_policy_revision: u64,
        terms_hash: &ContentHash,
        already_member: bool,
    ) -> Result<NetworkDiscoveryRecord, DiscoveryIndexError> {
        let _operation = self.operation.lock().await;
        let persisted = self
            .persisted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let accepted = persisted
            .records
            .get(network_id)
            .ok_or_else(|| DiscoveryIndexError::join("join.error.not_found"))?;
        let now = now_unix_ms();
        let preview = preview_from_record(&accepted.record, &accepted.digest, already_member, now)?;
        if preview.preview_digest != preview_digest {
            return Err(DiscoveryIndexError::join("join.error.preview_changed"));
        }
        if preview.network_policy_revision != network_policy_revision
            || preview.privacy_policy_revision != privacy_policy_revision
        {
            return Err(DiscoveryIndexError::join("join.error.policy_changed"));
        }
        if &preview.terms_hash != terms_hash {
            return Err(DiscoveryIndexError::join("join.error.terms_changed"));
        }
        if !preview.can_join {
            let reason = match preview.blocked_reason_key.as_deref() {
                Some("join.error.approval_required") => "join.error.approval_required",
                Some("join.error.invite_only") => "join.error.invite_only",
                Some("join.error.member_limit") => "join.error.member_limit",
                Some("join.error.endpoint_unavailable") => "join.error.endpoint_unavailable",
                _ => "join.error.unavailable",
            };
            return Err(DiscoveryIndexError::join(reason));
        }
        Ok(accepted.record.clone())
    }

    /// Validate legacy discovery material supplied with an invite without
    /// persisting it before the runtime join succeeds.
    pub fn validate_join_record(
        &self,
        record: &NetworkDiscoveryRecord,
    ) -> Result<(), DiscoveryIndexError> {
        record
            .validate_at(now_unix_ms())
            .map_err(DiscoveryIndexError::from_domain)?;
        if record.document.revoked_at_unix_ms.is_some() {
            return Err(DiscoveryIndexError::join("join.error.revoked"));
        }
        Ok(())
    }

    async fn mutate(
        &self,
        state: &NodeStateStore,
        configs: &ConfigManager,
        network_name: &str,
        index_url: &str,
        mutation: DiscoveryMutation,
        operation_id: Option<OperationId>,
    ) -> Result<NodeDiscoveryIndexSnapshot, DiscoveryIndexError> {
        let _operation = self.operation.lock().await;
        let url = validate_index_url(index_url)?;
        let now = now_unix_ms();
        self.set_pending(state, &url).await;

        let record = match self
            .build_record(configs, network_name, mutation, now, operation_id)
            .await
        {
            Ok(record) => record,
            Err(error) => {
                self.fail_request(
                    state,
                    &url,
                    error.category,
                    now,
                    DiscoveryAttemptSummary::default(),
                )
                .await?;
                return Err(error);
            }
        };
        if let Err(error) = record.validate_at(now) {
            let error = DiscoveryIndexError::from_domain(error);
            self.fail_request(
                state,
                &url,
                error.category,
                now,
                DiscoveryAttemptSummary::default(),
            )
            .await?;
            return Err(error);
        }

        let mut summary = DiscoveryAttemptSummary::default();
        let mut last_error = None;
        for attempt in 0..INDEX_MAX_ATTEMPTS {
            summary.attempts = summary.attempts.saturating_add(1);
            let started = Instant::now();
            let result = self.transport.publish(&url, &record).await;
            summary.duration_ms = elapsed_ms(started);
            match result {
                Ok(()) => {
                    last_error = None;
                    break;
                }
                Err(error) if error.retryable() && attempt + 1 < INDEX_MAX_ATTEMPTS => {
                    last_error = Some(error);
                    summary.retries = summary.retries.saturating_add(1);
                    if let Err(cancel_error) = wait_for_retry(retry_delay(attempt), None).await {
                        self.fail_request(state, &url, cancel_error.category, now, summary)
                            .await?;
                        return Err(cancel_error);
                    }
                }
                Err(error) => {
                    last_error = Some(error);
                    break;
                }
            }
        }
        if let Some(error) = last_error {
            self.fail_request(state, &url, error.category, now, summary)
                .await?;
            return Err(error);
        }
        {
            let mut current = self
                .persisted
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            apply_attempt_metrics(&mut current.metrics, summary, true);
            self.persist_candidate(&current)?;
            *self
                .persisted
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = current;
        }

        let snapshot = match self.accept_record_locked(record, now, true) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.restore_state(state).await;
                return Err(error);
            }
        };
        state.replace_discovery_index(snapshot.clone()).await;
        Ok(snapshot)
    }

    async fn build_record(
        &self,
        configs: &ConfigManager,
        network_name: &str,
        mutation: DiscoveryMutation,
        now: u64,
        _operation_id: Option<OperationId>,
    ) -> Result<NetworkDiscoveryRecord, DiscoveryIndexError> {
        let configs = configs.clone();
        let network_name = network_name.to_owned();
        let accepted = self
            .persisted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .records
            .values()
            .map(|accepted| accepted.record.clone())
            .collect::<Vec<_>>();
        tokio::task::spawn_blocking(move || {
            let record = configs
                .load_network_record(&network_name)
                .map_err(|_| DiscoveryIndexError::new(NodeDiscoveryIndexErrorCategory::Storage))?;
            let local_identity = configs
                .load_node_identity()
                .map_err(|_| DiscoveryIndexError::new(NodeDiscoveryIndexErrorCategory::Storage))?;
            let local_owner = NodeId::from_public_key(&local_identity.public_key_bytes());
            if record.owner != local_owner {
                return Err(DiscoveryIndexError::new(
                    NodeDiscoveryIndexErrorCategory::Rejected,
                ));
            }
            let identity_path = configs
                .network_identity_path(&network_name)
                .map_err(|_| DiscoveryIndexError::new(NodeDiscoveryIndexErrorCategory::Storage))?;
            let identity = Identity::load_from_file(&identity_path)
                .map_err(|_| DiscoveryIndexError::new(NodeDiscoveryIndexErrorCategory::Storage))?;
            let current = accepted
                .into_iter()
                .find(|candidate| candidate.document.network_id == record.network_id);
            let revision = match (mutation, current.as_ref()) {
                (DiscoveryMutation::Publish, None) => 1,
                (DiscoveryMutation::Publish, Some(_)) => {
                    return Err(DiscoveryIndexError::new(
                        NodeDiscoveryIndexErrorCategory::Rejected,
                    ));
                }
                (DiscoveryMutation::Update, None) => {
                    return Err(DiscoveryIndexError::new(
                        NodeDiscoveryIndexErrorCategory::Rejected,
                    ));
                }
                (DiscoveryMutation::Unpublish, None) => {
                    return Err(DiscoveryIndexError::new(
                        NodeDiscoveryIndexErrorCategory::Rejected,
                    ));
                }
                (_, Some(current)) if current.document.revoked_at_unix_ms.is_some() => {
                    return Err(DiscoveryIndexError::new(
                        NodeDiscoveryIndexErrorCategory::Rejected,
                    ));
                }
                (_, Some(current)) => current.document.revision.saturating_add(1).max(1),
            };
            let revoked_at_unix_ms = (mutation == DiscoveryMutation::Unpublish).then_some(now);
            let document = NetworkDiscoveryDocument::from_record(
                &record,
                revision,
                now,
                now.saturating_add(NETWORK_DISCOVERY_VALIDITY_MS),
                revoked_at_unix_ms,
            )
            .map_err(DiscoveryIndexError::from_domain)?;
            NetworkDiscoveryRecord::sign(document, &identity)
                .map_err(DiscoveryIndexError::from_domain)
        })
        .await
        .map_err(|_| DiscoveryIndexError::new(NodeDiscoveryIndexErrorCategory::Storage))?
    }

    async fn refresh_locked(
        &self,
        state: &NodeStateStore,
        url: String,
        mut cancel: Option<&mut watch::Receiver<bool>>,
    ) -> Result<NodeDiscoveryIndexSnapshot, DiscoveryIndexError> {
        let before = self.snapshot();
        let now = now_unix_ms();
        self.set_pending(state, &url).await;
        let mut summary = DiscoveryAttemptSummary::default();
        let mut last_error = None;

        for attempt in 0..INDEX_MAX_ATTEMPTS {
            summary.attempts = summary.attempts.saturating_add(1);
            let started = Instant::now();
            let request = self.transport.fetch(&url);
            let result = match cancel.as_deref_mut() {
                Some(cancel) => tokio::select! {
                    result = request => result,
                    _ = wait_for_cancel(cancel) => {
                        summary.duration_ms = elapsed_ms(started);
                        self.finish_cancelled(state, &before, now, summary).await?;
                        return Err(DiscoveryIndexError::new(
                            NodeDiscoveryIndexErrorCategory::Cancelled,
                        ));
                    }
                },
                None => request.await,
            };
            summary.duration_ms = elapsed_ms(started);
            match result {
                Ok(fetch) => {
                    summary.fetched_records = summary
                        .fetched_records
                        .saturating_add(fetch.records.len() as u64);
                    summary.malformed_records = summary
                        .malformed_records
                        .saturating_add(fetch.malformed_record_count as u64);
                    return self.apply_fetch(state, url, now, fetch, summary).await;
                }
                Err(error) if error.retryable() && attempt + 1 < INDEX_MAX_ATTEMPTS => {
                    last_error = Some(error);
                    summary.retries = summary.retries.saturating_add(1);
                    match wait_for_retry(retry_delay(attempt), cancel.as_deref_mut()).await {
                        Ok(()) => {}
                        Err(cancel_error) => {
                            self.finish_cancelled(state, &before, now, summary).await?;
                            return Err(cancel_error);
                        }
                    }
                }
                Err(error) => {
                    last_error = Some(error);
                    break;
                }
            }
        }

        let error = last_error
            .unwrap_or_else(|| DiscoveryIndexError::new(NodeDiscoveryIndexErrorCategory::Offline));
        self.fail_request(state, &url, error.category, now, summary)
            .await?;
        Err(error)
    }

    async fn apply_fetch(
        &self,
        state: &NodeStateStore,
        url: String,
        now: u64,
        fetch: DiscoveryIndexFetchResult,
        mut summary: DiscoveryAttemptSummary,
    ) -> Result<NodeDiscoveryIndexSnapshot, DiscoveryIndexError> {
        let mut current = self
            .persisted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        current.schema_version = DISCOVERY_INDEX_SCHEMA_VERSION;
        current.index_url = Some(url);
        current.last_attempt_unix_ms = Some(now);
        current.source = NodeDiscoveryIndexSource::Network;

        let mut rejected = fetch.malformed_record_count;
        let mut rejected_category = (fetch.malformed_record_count > 0)
            .then_some(NodeDiscoveryIndexErrorCategory::InvalidResponse);
        let mut accepted_batch = 0usize;
        for record in fetch.records {
            let id = record.document.network_id.as_str().to_owned();
            let validation = match current.records.get(&id) {
                Some(existing) => record.validate_successor(
                    existing.record.document.revision,
                    Some(existing.digest.as_str()),
                    existing.record.document.revoked_at_unix_ms.is_some(),
                    now,
                ),
                None => record.validate_at(now),
            };
            if let Err(error) = validation {
                rejected = rejected.saturating_add(1);
                let category = DiscoveryIndexError::from_domain(error).category;
                merge_rejected_category(&mut rejected_category, category);
                continue;
            }
            let Ok(digest) = record.digest() else {
                rejected = rejected.saturating_add(1);
                merge_rejected_category(
                    &mut rejected_category,
                    NodeDiscoveryIndexErrorCategory::PartialCorruption,
                );
                continue;
            };
            current.records.insert(
                id,
                PersistedDiscoveryRecord {
                    record,
                    digest,
                    accepted_at_unix_ms: now,
                    source: NodeDiscoveryIndexSource::Network,
                },
            );
            accepted_batch = accepted_batch.saturating_add(1);
        }
        current.rejected_record_count = rejected;
        summary.accepted_records = accepted_batch as u64;
        summary.rejected_records = rejected as u64;
        apply_attempt_metrics(&mut current.metrics, summary, true);

        if rejected == 0 {
            current.last_success_unix_ms = Some(now);
            current.last_error = None;
            current.state = NodeDiscoveryIndexState::Ready;
        } else {
            let category = match rejected_category {
                Some(NodeDiscoveryIndexErrorCategory::InvalidResponse)
                    if fetch.malformed_record_count > 0 =>
                {
                    NodeDiscoveryIndexErrorCategory::PartialCorruption
                }
                Some(category) => category,
                None => NodeDiscoveryIndexErrorCategory::PartialCorruption,
            };
            current.last_success_unix_ms = Some(now);
            current.last_error = Some(category);
            current.source = NodeDiscoveryIndexSource::Cache;
            current.state = if current.records.is_empty() {
                NodeDiscoveryIndexState::Failed
            } else {
                NodeDiscoveryIndexState::Degraded
            };
            self.persist_candidate(&current)?;
            let snapshot = snapshot_from_persisted(&current, now);
            *self
                .persisted
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = current;
            state.replace_discovery_index(snapshot).await;
            return Err(DiscoveryIndexError::new(category));
        }
        self.persist_candidate(&current)?;
        let snapshot = snapshot_from_persisted(&current, now);
        *self
            .persisted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = current;
        state.replace_discovery_index(snapshot.clone()).await;
        Ok(snapshot)
    }

    async fn set_pending(&self, state: &NodeStateStore, url: &str) {
        let mut current = self
            .persisted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        current.index_url = Some(url.to_owned());
        current.state = NodeDiscoveryIndexState::Pending;
        state
            .replace_discovery_index(snapshot_from_persisted(&current, now_unix_ms()))
            .await;
    }

    async fn fail_request(
        &self,
        state: &NodeStateStore,
        url: &str,
        category: NodeDiscoveryIndexErrorCategory,
        now: u64,
        summary: DiscoveryAttemptSummary,
    ) -> Result<NodeDiscoveryIndexSnapshot, DiscoveryIndexError> {
        match self.finish_failure(url, category, now, summary).await {
            Ok(snapshot) => {
                state.replace_discovery_index(snapshot.clone()).await;
                Ok(snapshot)
            }
            Err(error) => {
                self.restore_state(state).await;
                Err(error)
            }
        }
    }

    async fn restore_state(&self, state: &NodeStateStore) {
        state.replace_discovery_index(self.snapshot()).await;
    }

    async fn finish_failure(
        &self,
        url: &str,
        category: NodeDiscoveryIndexErrorCategory,
        now: u64,
        summary: DiscoveryAttemptSummary,
    ) -> Result<NodeDiscoveryIndexSnapshot, DiscoveryIndexError> {
        let mut current = self
            .persisted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        current.index_url = Some(url.to_owned());
        current.last_attempt_unix_ms = Some(now);
        current.last_error = Some(category);
        current.source = if current.records.is_empty() {
            NodeDiscoveryIndexSource::None
        } else {
            NodeDiscoveryIndexSource::Cache
        };
        current.state = state_for(category, !current.records.is_empty());
        apply_attempt_metrics(&mut current.metrics, summary, false);
        self.persist_candidate(&current)?;
        let snapshot = snapshot_from_persisted(&current, now);
        *self
            .persisted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = current;
        Ok(snapshot)
    }

    async fn finish_cancelled(
        &self,
        state: &NodeStateStore,
        before: &NodeDiscoveryIndexSnapshot,
        now: u64,
        mut summary: DiscoveryAttemptSummary,
    ) -> Result<(), DiscoveryIndexError> {
        let mut current = self
            .persisted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        current.last_attempt_unix_ms = Some(now);
        current.source = before.source;
        current.last_error = before.last_error.as_ref().map(|error| error.category);
        current.state = before.state;
        summary.cancelled = true;
        apply_attempt_metrics(&mut current.metrics, summary, false);
        self.persist_candidate(&current)?;
        let snapshot = snapshot_from_persisted(&current, now);
        *self
            .persisted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = current;
        state.replace_discovery_index(snapshot).await;
        Ok(())
    }

    fn accept_record_locked(
        &self,
        record: NetworkDiscoveryRecord,
        now: u64,
        retry_failed_request: bool,
    ) -> Result<NodeDiscoveryIndexSnapshot, DiscoveryIndexError> {
        let id = record.document.network_id.as_str().to_owned();
        let mut current = self
            .persisted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        record
            .validate_at(now)
            .map_err(DiscoveryIndexError::from_domain)?;
        if let Some(existing) = current.records.get(&id) {
            record
                .validate_successor(
                    existing.record.document.revision,
                    Some(existing.digest.as_str()),
                    existing.record.document.revoked_at_unix_ms.is_some(),
                    now,
                )
                .map_err(DiscoveryIndexError::from_domain)?;
        }
        let digest = record.digest().map_err(DiscoveryIndexError::from_domain)?;
        current.records.insert(
            id,
            PersistedDiscoveryRecord {
                record,
                digest,
                accepted_at_unix_ms: now,
                source: if retry_failed_request {
                    NodeDiscoveryIndexSource::Network
                } else {
                    NodeDiscoveryIndexSource::Join
                },
            },
        );
        current.last_attempt_unix_ms = Some(now);
        current.metrics.records_accepted = current.metrics.records_accepted.saturating_add(1);
        if retry_failed_request {
            current.last_success_unix_ms = Some(now);
            current.last_error = None;
            current.rejected_record_count = 0;
            current.source = NodeDiscoveryIndexSource::Network;
            current.state = NodeDiscoveryIndexState::Ready;
        } else {
            current.source = NodeDiscoveryIndexSource::Join;
        }
        self.persist_candidate(&current)?;
        let snapshot = snapshot_from_persisted(&current, now);
        *self
            .persisted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = current;
        Ok(snapshot)
    }

    fn persist_candidate(
        &self,
        candidate: &PersistedDiscoveryIndex,
    ) -> Result<(), DiscoveryIndexError> {
        write_json_atomic(&self.store_path, candidate)
            .map_err(|_| DiscoveryIndexError::new(NodeDiscoveryIndexErrorCategory::Storage))
    }
}

fn validate_approval_join_record(
    record: &NetworkDiscoveryRecord,
    now: u64,
) -> Result<NetworkDiscoveryRecord, DiscoveryIndexError> {
    record
        .validate_at(now)
        .map_err(DiscoveryIndexError::from_domain)?;
    if record.document.revoked_at_unix_ms.is_some() {
        return Err(DiscoveryIndexError::join("join.error.revoked"));
    }
    if record.document.join_mode != NetworkJoinMode::ApprovalRequired {
        return Err(DiscoveryIndexError::join("join.error.unavailable"));
    }
    if record
        .document
        .member_limit
        .is_some_and(|limit| record.document.member_count >= limit)
    {
        return Err(DiscoveryIndexError::join("join.error.member_limit"));
    }
    if !record.document.endpoints.iter().any(|endpoint| {
        endpoint.transport == fuckipv6_common::domain::DiscoveryTransport::Websocket
    }) {
        return Err(DiscoveryIndexError::join("join.error.endpoint_unavailable"));
    }
    Ok(record.clone())
}

fn preview_from_record(
    record: &NetworkDiscoveryRecord,
    discovery_digest: &str,
    already_member: bool,
    now: u64,
) -> Result<crate::node::NodeNetworkJoinPreview, DiscoveryIndexError> {
    record
        .validate_at(now)
        .map_err(DiscoveryIndexError::from_domain)?;
    if record.document.revoked_at_unix_ms.is_some() {
        return Err(DiscoveryIndexError::join("join.error.revoked"));
    }
    let document = &record.document;
    let terms = MembershipTerms {
        network_policy_revision: document.policy_revision,
        privacy_policy_revision: document.privacy_policy_revision,
        exit_contribution: document.exit_contribution,
        exit_contribution_scope: document.exit_contribution_scope,
        member_limit: document.member_limit,
    };
    let terms_hash = terms
        .content_hash()
        .map_err(DiscoveryIndexError::from_domain)?;
    let digest_payload = JoinPreviewDigest {
        schema_version: 1,
        discovery_digest,
        network_id: document.network_id.as_str(),
        network_name: &document.name,
        public_key: document.public_key.as_str(),
        owner: &document.owner,
        administrators: &document.administrators,
        discovery_revision: document.revision,
        network_policy_revision: document.policy_revision,
        privacy_policy_revision: document.privacy_policy_revision,
        terms_hash: &terms_hash,
        member_count: document.member_count,
        member_limit: document.member_limit,
        exit_contribution: document.exit_contribution,
        exit_contribution_scope: document.exit_contribution_scope,
        capability_summary: &document.capability_summary,
        endpoints: &document.endpoints,
        privacy_changes: &document.privacy_changes,
        issued_at_unix_ms: document.issued_at_unix_ms,
        expires_at_unix_ms: document.expires_at_unix_ms,
    };
    let encoded = serde_json::to_vec(&digest_payload)
        .map_err(|_| DiscoveryIndexError::new(NodeDiscoveryIndexErrorCategory::InvalidResponse))?;
    let preview_digest = blake3::hash(&encoded).to_hex().to_string();
    let member_limit_reached = !already_member
        && document
            .member_limit
            .is_some_and(|limit| document.member_count >= limit);
    let has_websocket_endpoint = document.endpoints.iter().any(|endpoint| {
        endpoint.transport == fuckipv6_common::domain::DiscoveryTransport::Websocket
    });
    let blocked_reason_key = if document.join_mode != NetworkJoinMode::Open {
        Some(match document.join_mode {
            NetworkJoinMode::Open => unreachable!(),
            NetworkJoinMode::ApprovalRequired => "join.error.approval_required",
            NetworkJoinMode::InviteOnly => "join.error.invite_only",
        })
    } else if member_limit_reached {
        Some("join.error.member_limit")
    } else if !has_websocket_endpoint {
        Some("join.error.endpoint_unavailable")
    } else {
        None
    };
    Ok(crate::node::NodeNetworkJoinPreview {
        network_id: document.network_id.to_string(),
        network_name: document.name.clone(),
        public_key: document.public_key.to_string(),
        owner: document.owner.to_string(),
        administrators: document
            .administrators
            .iter()
            .map(ToString::to_string)
            .collect(),
        discovery_revision: document.revision,
        network_policy_revision: document.policy_revision,
        privacy_policy_revision: document.privacy_policy_revision,
        join_mode: document.join_mode,
        visibility: document.visibility,
        capability_summary: document.capability_summary.clone(),
        endpoints: document.endpoints.clone(),
        member_count: document.member_count,
        member_limit: document.member_limit,
        exit_contribution: document.exit_contribution,
        exit_contribution_scope: document.exit_contribution_scope,
        exit_contribution_purpose_key: "join.exit.disclosure.purpose".into(),
        exit_contribution_bandwidth_impact_key: "join.exit.disclosure.bandwidth".into(),
        exit_contribution_logging_boundary_key: "join.exit.disclosure.logging".into(),
        exit_contribution_disable_key: "join.exit.disclosure.disable".into(),
        local_exit_available: false,
        local_exit_enabled: false,
        can_join_without_local_exit: document.exit_contribution
            == ExitContributionObligation::NotRequired,
        join_requires_exit_consent: document.exit_contribution
            == ExitContributionObligation::Required,
        privacy_changes: document.privacy_changes.clone(),
        already_member,
        can_join: blocked_reason_key.is_none(),
        blocked_reason_key: blocked_reason_key.map(str::to_owned),
        terms_hash,
        preview_digest,
        issued_at_unix_ms: document.issued_at_unix_ms,
        expires_at_unix_ms: document.expires_at_unix_ms,
        locally_contributed_capabilities: Vec::new(),
        contribution_policy_key: "join.contribution.local_control".into(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiscoveryMutation {
    Publish,
    Update,
    Unpublish,
}

#[derive(Debug, Clone, Copy, Default)]
struct DiscoveryAttemptSummary {
    attempts: u64,
    retries: u64,
    duration_ms: u64,
    fetched_records: u64,
    malformed_records: u64,
    accepted_records: u64,
    rejected_records: u64,
    cancelled: bool,
}

fn apply_attempt_metrics(
    metrics: &mut NodeDiscoveryIndexMetrics,
    summary: DiscoveryAttemptSummary,
    succeeded: bool,
) {
    if summary.attempts > 0 {
        metrics.request_attempts = metrics.request_attempts.saturating_add(summary.attempts);
        metrics.requests_succeeded = metrics
            .requests_succeeded
            .saturating_add(u64::from(succeeded));
        metrics.requests_failed = metrics
            .requests_failed
            .saturating_add(u64::from(!succeeded && !summary.cancelled));
        if summary.duration_ms > 0 {
            metrics.last_request_duration_ms = Some(summary.duration_ms);
        }
    }
    metrics.retries_scheduled = metrics.retries_scheduled.saturating_add(summary.retries);
    metrics.cancellations = metrics
        .cancellations
        .saturating_add(u64::from(summary.cancelled));
    metrics.records_accepted = metrics
        .records_accepted
        .saturating_add(summary.accepted_records);
    metrics.records_rejected = metrics
        .records_rejected
        .saturating_add(summary.rejected_records);
}

fn merge_rejected_category(
    current: &mut Option<NodeDiscoveryIndexErrorCategory>,
    category: NodeDiscoveryIndexErrorCategory,
) {
    fn priority(category: NodeDiscoveryIndexErrorCategory) -> u8 {
        match category {
            NodeDiscoveryIndexErrorCategory::Signature => 4,
            NodeDiscoveryIndexErrorCategory::IncompatibleVersion => 3,
            NodeDiscoveryIndexErrorCategory::Rejected => 2,
            _ => 1,
        }
    }
    if current.is_none_or(|existing| priority(category) > priority(existing)) {
        *current = Some(category);
    }
}

fn retry_delay(attempt: usize) -> Duration {
    let multiplier = 1u32 << attempt.min(8);
    INDEX_RETRY_BASE_DELAY
        .saturating_mul(multiplier)
        .min(INDEX_RETRY_MAX_DELAY)
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn cancellation_requested(cancel: Option<&watch::Receiver<bool>>) -> bool {
    cancel.is_some_and(|cancel| *cancel.borrow())
}

async fn wait_for_cancel(cancel: &mut watch::Receiver<bool>) {
    if *cancel.borrow() {
        return;
    }
    while cancel.changed().await.is_ok() {
        if *cancel.borrow() {
            return;
        }
    }
}

async fn wait_for_retry(
    delay: Duration,
    mut cancel: Option<&mut watch::Receiver<bool>>,
) -> Result<(), DiscoveryIndexError> {
    let sleep = tokio::time::sleep(delay);
    tokio::pin!(sleep);
    match cancel.as_deref_mut() {
        Some(cancel) => tokio::select! {
            _ = &mut sleep => Ok(()),
            _ = wait_for_cancel(cancel) => Err(DiscoveryIndexError::new(
                NodeDiscoveryIndexErrorCategory::Cancelled,
            )),
        },
        None => {
            sleep.await;
            Ok(())
        }
    }
}

fn load_persisted(
    path: &Path,
) -> (
    PersistedDiscoveryIndex,
    Option<NodeDiscoveryIndexErrorCategory>,
) {
    if !path.exists() {
        return (PersistedDiscoveryIndex::default(), None);
    }
    let content = match std::fs::read(path) {
        Ok(content) => content,
        Err(_) => {
            return (
                PersistedDiscoveryIndex::default(),
                Some(NodeDiscoveryIndexErrorCategory::Storage),
            );
        }
    };
    let mut persisted: PersistedDiscoveryIndex = match serde_json::from_slice(&content) {
        Ok(persisted) => persisted,
        Err(_) => {
            return (
                PersistedDiscoveryIndex::default(),
                Some(NodeDiscoveryIndexErrorCategory::InvalidResponse),
            );
        }
    };
    if persisted.schema_version != DISCOVERY_INDEX_SCHEMA_VERSION {
        return (
            PersistedDiscoveryIndex::default(),
            Some(NodeDiscoveryIndexErrorCategory::IncompatibleVersion),
        );
    }
    persisted.source = if persisted.records.is_empty() {
        NodeDiscoveryIndexSource::None
    } else {
        NodeDiscoveryIndexSource::Cache
    };
    persisted.state = state_for_loaded(&persisted);
    (persisted, None)
}

fn state_for_loaded(persisted: &PersistedDiscoveryIndex) -> NodeDiscoveryIndexState {
    match persisted.last_error {
        Some(category) => state_for(category, !persisted.records.is_empty()),
        None if persisted.index_url.is_some() && persisted.last_success_unix_ms.is_some() => {
            NodeDiscoveryIndexState::Ready
        }
        None => NodeDiscoveryIndexState::Idle,
    }
}

fn state_for(
    category: NodeDiscoveryIndexErrorCategory,
    has_records: bool,
) -> NodeDiscoveryIndexState {
    if matches!(
        category,
        NodeDiscoveryIndexErrorCategory::Unsupported
            | NodeDiscoveryIndexErrorCategory::IncompatibleVersion
    ) {
        NodeDiscoveryIndexState::Unsupported
    } else if has_records {
        NodeDiscoveryIndexState::Degraded
    } else {
        NodeDiscoveryIndexState::Failed
    }
}

fn snapshot_from_persisted(
    persisted: &PersistedDiscoveryIndex,
    now: u64,
) -> NodeDiscoveryIndexSnapshot {
    let mut records = persisted
        .records
        .values()
        .map(|accepted| {
            let validation = accepted.record.validate_at(now);
            let discoverable =
                validation.is_ok() && accepted.record.document.revoked_at_unix_ms.is_none();
            NodeDiscoveryIndexRecordSnapshot {
                record: accepted.record.clone(),
                digest: accepted.digest.clone(),
                accepted_at_unix_ms: accepted.accepted_at_unix_ms,
                source: accepted.source,
                discoverable,
                invalid_reason: validation.err().map(|error| error.code().to_owned()),
            }
        })
        .collect::<Vec<_>>();
    records.sort_by(|left, right| {
        left.record
            .document
            .name
            .cmp(&right.record.document.name)
            .then_with(|| {
                left.record
                    .document
                    .network_id
                    .as_str()
                    .cmp(right.record.document.network_id.as_str())
            })
    });
    NodeDiscoveryIndexSnapshot {
        state: persisted.state,
        index_url: persisted.index_url.clone(),
        last_attempt_unix_ms: persisted.last_attempt_unix_ms,
        last_success_unix_ms: persisted.last_success_unix_ms,
        last_error: persisted.last_error.map(NodeDiscoveryIndexError::new),
        source: persisted.source,
        rejected_record_count: persisted.rejected_record_count,
        records,
        metrics: persisted.metrics.clone(),
        data_plane_affected: false,
        search_history_stored: false,
    }
}

pub(crate) fn validate_index_url(value: &str) -> Result<String, DiscoveryIndexError> {
    let value = value.trim().trim_end_matches('/');
    let url = reqwest::Url::parse(value)
        .map_err(|_| DiscoveryIndexError::new(NodeDiscoveryIndexErrorCategory::Rejected))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(DiscoveryIndexError::new(
            NodeDiscoveryIndexErrorCategory::Rejected,
        ));
    }
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if url.scheme() == "http" && !loopback {
        return Err(DiscoveryIndexError::new(
            NodeDiscoveryIndexErrorCategory::Unsupported,
        ));
    }
    Ok(value.to_owned())
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuckipv6_common::domain::{
        CapabilityKind, DiscoveryTransport, ExitContributionObligation, MembershipRole,
        MembershipState, NetworkCreateProfile, NetworkEndpoint, NetworkJoinMode, NetworkRecord,
        NetworkVisibility, PublicKey,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::node::{
        NodeNetworkMembershipSnapshot, NodeNetworkRole, NodeNetworkSnapshot, NodeNetworkStatus,
        NodeTrafficSnapshot,
    };

    #[derive(Default)]
    struct FakeTransport {
        records: Mutex<Vec<NetworkDiscoveryRecord>>,
        fail_fetch: AtomicUsize,
        fail_categories: Mutex<Vec<NodeDiscoveryIndexErrorCategory>>,
        fail_publish: Mutex<Vec<NodeDiscoveryIndexErrorCategory>>,
        malformed_record_count: AtomicUsize,
        fetch_attempts: AtomicUsize,
        published: Mutex<Vec<NetworkDiscoveryRecord>>,
    }

    impl DiscoveryIndexTransport for FakeTransport {
        fn fetch<'a>(
            &'a self,
            _index_url: &'a str,
        ) -> BoxFuture<'a, Result<DiscoveryIndexFetchResult, DiscoveryIndexError>> {
            Box::pin(async move {
                self.fetch_attempts.fetch_add(1, Ordering::SeqCst);
                let category = self
                    .fail_categories
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .pop()
                    .or_else(|| {
                        (self
                            .fail_fetch
                            .try_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                                remaining.checked_sub(1)
                            })
                            .is_ok())
                        .then_some(NodeDiscoveryIndexErrorCategory::Timeout)
                    });
                if let Some(category) = category {
                    return Err(DiscoveryIndexError::new(category));
                }
                Ok(DiscoveryIndexFetchResult {
                    records: self.records.lock().unwrap().clone(),
                    malformed_record_count: self.malformed_record_count.load(Ordering::SeqCst),
                })
            })
        }

        fn publish<'a>(
            &'a self,
            _index_url: &'a str,
            record: &'a NetworkDiscoveryRecord,
        ) -> BoxFuture<'a, Result<(), DiscoveryIndexError>> {
            Box::pin(async move {
                if let Some(category) = self
                    .fail_publish
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .pop()
                {
                    return Err(DiscoveryIndexError::new(category));
                }
                self.published.lock().unwrap().push(record.clone());
                Ok(())
            })
        }
    }

    fn temp_path(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("fuckipv6-discovery-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path.join("discovery_index.json")
    }

    fn seed_persisted_record(manager: &DiscoveryIndexManager, record: NetworkDiscoveryRecord) {
        let id = record.document.network_id.to_string();
        let digest = record.digest().unwrap();
        manager
            .persisted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .records
            .insert(
                id,
                PersistedDiscoveryRecord {
                    record,
                    digest,
                    accepted_at_unix_ms: now_unix_ms(),
                    source: NodeDiscoveryIndexSource::Network,
                },
            );
    }

    fn signed_network_profile(identity: &Identity) -> NetworkRecord {
        signed_network_profile_with(identity, NetworkJoinMode::Open, None, 0)
    }

    fn signed_network_profile_with(
        identity: &Identity,
        join_mode: NetworkJoinMode,
        member_limit: Option<u32>,
        additional_members: usize,
    ) -> NetworkRecord {
        let public_key = PublicKey::from_bytes(identity.public_key_bytes());
        let owner = NodeId::from_public_key(b"owner");
        let mut record = NetworkRecord::create_owned(
            NetworkCreateProfile {
                name: "home".into(),
                purpose: "Home network".into(),
                icon: "home".into(),
                language: "en".into(),
                region: "US".into(),
                tags: vec!["home".into()],
                capability_declarations: vec![CapabilityKind::Relay],
                visibility: NetworkVisibility::Public,
                join_mode,
                exit_contribution: Default::default(),
                exit_contribution_scope: Default::default(),
            },
            public_key,
            owner,
            100,
        )
        .unwrap();
        record.member_limit = member_limit;
        for index in 0..additional_members {
            record.members.push(NodeId::from_public_key(
                format!("member-{index}").as_bytes(),
            ));
        }
        record.policy_revision = 7;
        record.privacy_policy_revision = 3;
        record.exit_contribution = ExitContributionObligation::Required;
        record.exit_contribution_scope =
            fuckipv6_common::domain::ExitContributionScope::JoiningMembers;
        record.privacy_changes = vec!["Network operators can observe connection metadata".into()];
        let now = now_unix_ms();
        record
            .sign_public_profile(
                identity,
                now.saturating_sub(1),
                now.saturating_add(fuckipv6_common::domain::NETWORK_PUBLIC_PROFILE_VALIDITY_MS),
            )
            .unwrap();
        record
    }

    fn signed_network_revision_from_profile(
        identity: &Identity,
        profile: &NetworkRecord,
        revision: u64,
    ) -> NetworkDiscoveryRecord {
        let now = now_unix_ms();
        let document = NetworkDiscoveryDocument::from_record_with_endpoints(
            profile,
            revision,
            now.saturating_sub(1),
            now.saturating_add(NETWORK_DISCOVERY_VALIDITY_MS),
            None,
            vec![NetworkEndpoint {
                transport: DiscoveryTransport::Websocket,
                host: "coordinator.home.example".into(),
                port: 443,
            }],
        )
        .unwrap();
        NetworkDiscoveryRecord::sign(document, identity).unwrap()
    }

    fn signed_network_revision(identity: &Identity, revision: u64) -> NetworkDiscoveryRecord {
        signed_network_revision_from_profile(identity, &signed_network_profile(identity), revision)
    }

    fn signed_network() -> (NetworkDiscoveryRecord, Identity) {
        let identity = Identity::generate().unwrap();
        let record = signed_network_revision(&identity, 1);
        (record, identity)
    }

    #[tokio::test]
    async fn refresh_verifies_rejects_rollback_and_survives_outage() {
        let path = temp_path("refresh");
        let transport = Arc::new(FakeTransport::default());
        let manager = DiscoveryIndexManager::new(path, transport.clone());
        let state = NodeStateStore::new();
        let identity = Identity::generate().unwrap();
        let profile = signed_network_profile(&identity);
        let newer = signed_network_revision_from_profile(&identity, &profile, 2);
        *transport.records.lock().unwrap() = vec![newer.clone()];
        manager
            .refresh(&state, Some("https://index.example"))
            .await
            .unwrap();
        let snapshot = state.snapshot().await.discovery_index;
        assert_eq!(snapshot.state, NodeDiscoveryIndexState::Ready);
        assert_eq!(snapshot.records.len(), 1);
        assert!(snapshot.records[0].discoverable);

        let older = signed_network_revision_from_profile(&identity, &profile, 1);
        *transport.records.lock().unwrap() = vec![older];
        assert_eq!(
            manager.retry(&state).await.unwrap_err().category,
            NodeDiscoveryIndexErrorCategory::Rejected
        );
        let degraded = state.snapshot().await.discovery_index;
        assert_eq!(degraded.state, NodeDiscoveryIndexState::Degraded);
        assert_eq!(
            degraded.last_error.unwrap().category,
            NodeDiscoveryIndexErrorCategory::Rejected
        );
        assert_eq!(degraded.records[0].record.document.revision, 2);
        transport
            .fail_fetch
            .store(INDEX_MAX_ATTEMPTS, Ordering::SeqCst);
        assert_eq!(
            manager.retry(&state).await.unwrap_err().category,
            NodeDiscoveryIndexErrorCategory::Timeout
        );
        let degraded = state.snapshot().await.discovery_index;
        assert_eq!(degraded.state, NodeDiscoveryIndexState::Degraded);
        assert_eq!(degraded.records[0].record.document.revision, 2);
        assert!(!degraded.data_plane_affected);
        assert!(!degraded.search_history_stored);
        assert_eq!(
            state.snapshot().await.lifecycle.phase,
            crate::node::NodePhase::Stopped
        );
    }

    #[tokio::test]
    async fn failed_fetch_without_records_becomes_failed() {
        let path = temp_path("failed-empty");
        let transport = Arc::new(FakeTransport::default());
        transport
            .fail_fetch
            .store(INDEX_MAX_ATTEMPTS, Ordering::SeqCst);
        let manager = DiscoveryIndexManager::new(path, transport);
        let state = NodeStateStore::new();

        assert_eq!(
            manager
                .refresh(&state, Some("https://index.example"))
                .await
                .unwrap_err()
                .category,
            NodeDiscoveryIndexErrorCategory::Timeout
        );
        let snapshot = state.snapshot().await.discovery_index;
        assert_eq!(snapshot.state, NodeDiscoveryIndexState::Failed);
        assert_eq!(
            snapshot.last_error.unwrap().category,
            NodeDiscoveryIndexErrorCategory::Timeout
        );
        assert!(snapshot.records.is_empty());
    }

    #[tokio::test]
    async fn failed_fetch_with_records_becomes_degraded() {
        let path = temp_path("failed-existing");
        let transport = Arc::new(FakeTransport::default());
        let manager = DiscoveryIndexManager::new(path, transport.clone());
        let state = NodeStateStore::new();
        let (record, _) = signed_network();
        *transport.records.lock().unwrap() = vec![record];
        manager
            .refresh(&state, Some("https://index.example"))
            .await
            .unwrap();

        transport
            .fail_fetch
            .store(INDEX_MAX_ATTEMPTS, Ordering::SeqCst);
        assert_eq!(
            manager.retry(&state).await.unwrap_err().category,
            NodeDiscoveryIndexErrorCategory::Timeout
        );
        let snapshot = state.snapshot().await.discovery_index;
        assert_eq!(snapshot.state, NodeDiscoveryIndexState::Degraded);
        assert_eq!(
            snapshot.last_error.unwrap().category,
            NodeDiscoveryIndexErrorCategory::Timeout
        );
        assert_eq!(snapshot.records.len(), 1);
    }

    #[tokio::test]
    async fn storage_failure_restores_previous_projection_and_records() {
        let path = temp_path("failed-storage");
        let transport = Arc::new(FakeTransport::default());
        let manager = DiscoveryIndexManager::new(path.clone(), transport.clone());
        let state = NodeStateStore::new();
        let (record, _) = signed_network();
        *transport.records.lock().unwrap() = vec![record];
        manager
            .refresh(&state, Some("https://index.example"))
            .await
            .unwrap();
        let before = state.snapshot().await.discovery_index;

        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        transport
            .fail_fetch
            .store(INDEX_MAX_ATTEMPTS, Ordering::SeqCst);
        assert_eq!(
            manager.retry(&state).await.unwrap_err().category,
            NodeDiscoveryIndexErrorCategory::Storage
        );

        let after = state.snapshot().await.discovery_index;
        assert_eq!(after.state, before.state);
        assert_eq!(after.index_url, before.index_url);
        assert_eq!(after.last_error, before.last_error);
        assert_eq!(after.records, before.records);
        assert_eq!(manager.snapshot().records, before.records);
    }

    #[tokio::test]
    async fn tls_and_timeout_refreshes_are_bounded_and_leave_metrics_sanitized() {
        for category in [
            NodeDiscoveryIndexErrorCategory::Timeout,
            NodeDiscoveryIndexErrorCategory::Tls,
        ] {
            let path = temp_path(&format!("bounded-{}", category.as_str()));
            let transport = Arc::new(FakeTransport::default());
            *transport.fail_categories.lock().unwrap() = vec![category; INDEX_MAX_ATTEMPTS];
            let manager = DiscoveryIndexManager::new(path, transport.clone());
            let state = NodeStateStore::new();

            assert_eq!(
                manager
                    .refresh(&state, Some("https://index.example"))
                    .await
                    .unwrap_err()
                    .category,
                category
            );
            let snapshot = state.snapshot().await.discovery_index;
            assert_eq!(
                transport.fetch_attempts.load(Ordering::SeqCst),
                INDEX_MAX_ATTEMPTS
            );
            assert_eq!(snapshot.metrics.request_attempts, INDEX_MAX_ATTEMPTS as u64);
            assert_eq!(snapshot.metrics.requests_failed, 1);
            assert_eq!(snapshot.metrics.retries_scheduled, 2);
            assert_eq!(snapshot.state, NodeDiscoveryIndexState::Failed);
            assert_eq!(snapshot.source, NodeDiscoveryIndexSource::None);

            let metrics = serde_json::to_string(&snapshot.metrics).unwrap();
            for forbidden in ["https://", "index.example", "search", "query", "token"] {
                assert!(!metrics.contains(forbidden));
            }
        }
    }

    #[tokio::test]
    async fn signature_failure_is_not_retried_and_never_replaces_cache() {
        let path = temp_path("signature");
        let transport = Arc::new(FakeTransport::default());
        let manager = DiscoveryIndexManager::new(path, transport.clone());
        let state = NodeStateStore::new();
        let (mut record, _) = signed_network();
        record.document.name = "tampered".into();
        *transport.records.lock().unwrap() = vec![record];

        assert_eq!(
            manager
                .refresh(&state, Some("https://index.example"))
                .await
                .unwrap_err()
                .category,
            NodeDiscoveryIndexErrorCategory::Signature
        );
        let snapshot = state.snapshot().await.discovery_index;
        assert_eq!(transport.fetch_attempts.load(Ordering::SeqCst), 1);
        assert_eq!(snapshot.state, NodeDiscoveryIndexState::Failed);
        assert_eq!(
            snapshot.last_error.unwrap().category,
            NodeDiscoveryIndexErrorCategory::Signature
        );
        assert!(snapshot.records.is_empty());
        assert_eq!(snapshot.metrics.retries_scheduled, 0);
    }

    #[tokio::test]
    async fn partial_corruption_keeps_valid_records_and_reports_a_degraded_cache() {
        let path = temp_path("partial-corruption");
        let transport = Arc::new(FakeTransport::default());
        let manager = DiscoveryIndexManager::new(path, transport.clone());
        let state = NodeStateStore::new();
        let (record, _) = signed_network();
        *transport.records.lock().unwrap() = vec![record];
        transport.malformed_record_count.store(1, Ordering::SeqCst);

        assert_eq!(
            manager
                .refresh(&state, Some("https://index.example"))
                .await
                .unwrap_err()
                .category,
            NodeDiscoveryIndexErrorCategory::PartialCorruption
        );
        let snapshot = state.snapshot().await.discovery_index;
        assert_eq!(snapshot.state, NodeDiscoveryIndexState::Degraded);
        assert_eq!(snapshot.source, NodeDiscoveryIndexSource::Cache);
        assert_eq!(snapshot.records.len(), 1);
        assert_eq!(
            snapshot.records[0].source,
            NodeDiscoveryIndexSource::Network
        );
        assert_eq!(snapshot.rejected_record_count, 1);
        assert_eq!(snapshot.metrics.records_accepted, 1);
        assert_eq!(snapshot.metrics.records_rejected, 1);
        assert!(!snapshot.data_plane_affected);
    }

    #[test]
    fn persisted_incompatible_schema_is_distinct_from_a_missing_feature() {
        let path = temp_path("incompatible-persisted");
        std::fs::write(&path, r#"{"schema_version":2}"#).unwrap();
        let manager = DiscoveryIndexManager::new(path, Arc::new(FakeTransport::default()));
        let snapshot = manager.snapshot();
        assert_eq!(snapshot.state, NodeDiscoveryIndexState::Unsupported);
        assert_eq!(
            snapshot.last_error.unwrap().category,
            NodeDiscoveryIndexErrorCategory::IncompatibleVersion
        );
    }

    #[test]
    fn retry_backoff_is_exponential_and_capped() {
        assert_eq!(retry_delay(0), Duration::from_millis(250));
        assert_eq!(retry_delay(1), Duration::from_millis(500));
        assert_eq!(retry_delay(2), Duration::from_secs(1));
        assert_eq!(retry_delay(3), INDEX_RETRY_MAX_DELAY);
        assert_eq!(retry_delay(usize::MAX), INDEX_RETRY_MAX_DELAY);
    }

    #[tokio::test]
    async fn cancellation_during_retry_restores_projection_and_stops_attempts() {
        let path = temp_path("cancel-retry");
        let transport = Arc::new(FakeTransport::default());
        *transport.fail_categories.lock().unwrap() =
            vec![NodeDiscoveryIndexErrorCategory::Timeout; INDEX_MAX_ATTEMPTS];
        let manager = Arc::new(DiscoveryIndexManager::new(path, transport.clone()));
        let state = NodeStateStore::new();
        let (cancel, cancel_rx) = watch::channel(false);
        let task_manager = manager.clone();
        let task_state = state.clone();
        let task = tokio::spawn(async move {
            task_manager
                .refresh_cancellable(&task_state, Some("https://index.example"), Some(cancel_rx))
                .await
        });

        while transport.fetch_attempts.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        cancel.send(true).unwrap();
        let error = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("cancellation must interrupt retry backoff")
            .unwrap()
            .unwrap_err();
        assert_eq!(error.category, NodeDiscoveryIndexErrorCategory::Cancelled);

        let snapshot = state.snapshot().await.discovery_index;
        assert_eq!(snapshot.state, NodeDiscoveryIndexState::Idle);
        assert!(snapshot.last_attempt_unix_ms.is_some());
        assert_eq!(snapshot.metrics.cancellations, 1);
        assert_eq!(snapshot.metrics.requests_failed, 0);
        assert_eq!(transport.fetch_attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn offline_index_preserves_joined_network_and_membership_projections() {
        let path = temp_path("offline-joined-network");
        let transport = Arc::new(FakeTransport::default());
        *transport.fail_categories.lock().unwrap() =
            vec![NodeDiscoveryIndexErrorCategory::Offline; INDEX_MAX_ATTEMPTS];
        let manager = DiscoveryIndexManager::new(path, transport);
        let state = NodeStateStore::new();
        let network = NodeNetworkSnapshot {
            name: "home".into(),
            tun_name: "fk-home".into(),
            status: NodeNetworkStatus::Online,
            assigned_ipv6: None,
            assigned_ipv4: None,
            p2p_listen_addr: None,
            uptime_secs: 42,
            roles: vec![NodeNetworkRole::Member],
            peer_count: 2,
            traffic: NodeTrafficSnapshot::default(),
        };
        let membership = NodeNetworkMembershipSnapshot {
            network_name: "home".into(),
            network_id: "network-home".into(),
            node_id: "node-local".into(),
            state: MembershipState::Active,
            role: MembershipRole::Member,
            local_policy_revision: 3,
            network_policy_revision: Some(3),
            privacy_policy_revision: Some(2),
            member_limit: Some(20),
            exit_contribution: Some(ExitContributionObligation::Required),
            exit_contribution_scope: None,
            terms_hash: None,
            current_terms: None,
            current_terms_hash: None,
            policy_confirmation_required: false,
            acknowledged_at_unix_ms: Some(100),
            created_at_unix_ms: 50,
            updated_at_unix_ms: 100,
            suspension_reason: None,
        };
        let expected_network = network.clone();
        let expected_membership = membership.clone();
        state
            .update(|snapshot| {
                snapshot.networks.push(network);
                snapshot.memberships.push(membership);
            })
            .await;

        assert_eq!(
            manager
                .refresh(&state, Some("https://index.example"))
                .await
                .unwrap_err()
                .category,
            NodeDiscoveryIndexErrorCategory::Offline
        );
        let snapshot = state.snapshot().await;
        assert_eq!(snapshot.networks, vec![expected_network]);
        assert_eq!(snapshot.memberships, vec![expected_membership]);
        assert_eq!(
            snapshot.discovery_index.state,
            NodeDiscoveryIndexState::Failed
        );
        assert!(!snapshot.discovery_index.data_plane_affected);
    }

    #[tokio::test]
    async fn publication_failure_preserves_the_local_network_record() {
        let root = temp_path("publish-preserves-local").with_extension("config");
        let _ = std::fs::remove_dir_all(&root);
        let configs = ConfigManager::new_for_dir(root).unwrap();
        let profile = NetworkCreateProfile {
            name: "home".into(),
            purpose: "Home network".into(),
            icon: "home".into(),
            language: "en".into(),
            region: "US".into(),
            tags: vec!["home".into()],
            capability_declarations: vec![CapabilityKind::Relay],
            visibility: NetworkVisibility::Public,
            join_mode: NetworkJoinMode::Open,
            exit_contribution: Default::default(),
            exit_contribution_scope: Default::default(),
        };
        let created = configs
            .create_owned_network(profile, "ws://127.0.0.1:6666", true)
            .unwrap();
        let before = configs.load_network_record("home").unwrap();
        assert_eq!(before.network_id, created.record.network_id);

        let transport = Arc::new(FakeTransport::default());
        *transport.fail_publish.lock().unwrap() =
            vec![NodeDiscoveryIndexErrorCategory::Timeout; INDEX_MAX_ATTEMPTS];
        let manager =
            DiscoveryIndexManager::new(temp_path("publish-preserves-local-index"), transport);
        let state = NodeStateStore::new();
        assert_eq!(
            manager
                .publish(&state, &configs, "home", "https://index.example", None)
                .await
                .unwrap_err()
                .category,
            NodeDiscoveryIndexErrorCategory::Timeout
        );

        let after = configs.load_network_record("home").unwrap();
        assert_eq!(after.network_id, before.network_id);
        assert!(configs
            .list_networks()
            .unwrap()
            .iter()
            .any(|network| network.name == "home"));
    }

    #[tokio::test]
    async fn join_preview_is_complete_read_only_and_digest_is_stable() {
        let path = temp_path("join-preview-read-only");
        let manager = DiscoveryIndexManager::new(path.clone(), Arc::new(FakeTransport::default()));
        let (record, _) = signed_network();
        let network_id = record.document.network_id.to_string();
        manager
            .accept_record_locked(record, now_unix_ms(), true)
            .unwrap();
        let before = std::fs::read(&path).unwrap();

        let first = manager
            .preview_join_record(&network_id, false)
            .await
            .unwrap();
        let after_first = std::fs::read(&path).unwrap();
        let second = manager
            .preview_join_record(&network_id, false)
            .await
            .unwrap();

        assert_eq!(before, after_first);
        assert_eq!(after_first, std::fs::read(&path).unwrap());
        assert_eq!(first.preview_digest, second.preview_digest);
        assert_eq!(first.network_name, "home");
        assert_eq!(first.owner, NodeId::from_public_key(b"owner").to_string());
        assert_eq!(first.network_policy_revision, 7);
        assert_eq!(first.privacy_policy_revision, 3);
        assert_eq!(
            first.exit_contribution,
            ExitContributionObligation::Required
        );
        assert_eq!(first.administrators.len(), 0);
        assert_eq!(first.capability_summary, vec![CapabilityKind::Relay]);
        assert_eq!(first.endpoints.len(), 1);
        assert_eq!(first.member_count, 1);
        assert!(first.can_join);
        assert!(first.blocked_reason_key.is_none());
    }

    #[tokio::test]
    async fn join_confirmation_rejects_tampered_digest_and_terms() {
        let path = temp_path("join-confirm-tamper");
        let manager = DiscoveryIndexManager::new(path, Arc::new(FakeTransport::default()));
        let (record, _) = signed_network();
        let network_id = record.document.network_id.to_string();
        manager
            .accept_record_locked(record, now_unix_ms(), true)
            .unwrap();
        let preview = manager
            .preview_join_record(&network_id, false)
            .await
            .unwrap();

        let digest_error = manager
            .confirm_join_record(
                &network_id,
                "tampered",
                preview.network_policy_revision,
                preview.privacy_policy_revision,
                &preview.terms_hash,
                false,
            )
            .await
            .unwrap_err();
        assert_eq!(digest_error.reason_key, Some("join.error.preview_changed"));

        let terms_error = manager
            .confirm_join_record(
                &network_id,
                &preview.preview_digest,
                preview.network_policy_revision,
                preview.privacy_policy_revision,
                &ContentHash::from_bytes(b"tampered terms"),
                false,
            )
            .await
            .unwrap_err();
        assert_eq!(terms_error.reason_key, Some("join.error.terms_changed"));
    }

    #[tokio::test]
    async fn join_preview_blocks_closed_and_full_networks() {
        for (name, join_mode, member_limit, expected_reason) in [
            (
                "closed",
                NetworkJoinMode::ApprovalRequired,
                None,
                "join.error.approval_required",
            ),
            (
                "full",
                NetworkJoinMode::Open,
                Some(1),
                "join.error.member_limit",
            ),
        ] {
            let path = temp_path(&format!("join-blocked-{name}"));
            let manager = DiscoveryIndexManager::new(path, Arc::new(FakeTransport::default()));
            let identity = Identity::generate().unwrap();
            let profile = signed_network_profile_with(&identity, join_mode, member_limit, 0);
            let record = signed_network_revision_from_profile(&identity, &profile, 1);
            let network_id = record.document.network_id.to_string();
            manager
                .accept_record_locked(record, now_unix_ms(), true)
                .unwrap();
            let preview = manager
                .preview_join_record(&network_id, false)
                .await
                .unwrap();
            assert!(!preview.can_join);
            assert_eq!(preview.blocked_reason_key.as_deref(), Some(expected_reason));
            let error = manager
                .confirm_join_record(
                    &network_id,
                    &preview.preview_digest,
                    preview.network_policy_revision,
                    preview.privacy_policy_revision,
                    &preview.terms_hash,
                    false,
                )
                .await
                .unwrap_err();
            assert_eq!(error.reason_key, Some(expected_reason));
        }
    }

    #[tokio::test]
    async fn approval_join_record_is_read_only_and_requires_an_approval_network() {
        let path = temp_path("approval-join-read-only");
        let manager = DiscoveryIndexManager::new(path.clone(), Arc::new(FakeTransport::default()));
        let identity = Identity::generate().unwrap();
        let profile =
            signed_network_profile_with(&identity, NetworkJoinMode::ApprovalRequired, None, 0);
        let record = signed_network_revision_from_profile(&identity, &profile, 1);
        let network_id = record.document.network_id.to_string();
        manager
            .accept_record_locked(record.clone(), now_unix_ms(), true)
            .unwrap();
        let before = std::fs::read(&path).unwrap();

        assert_eq!(
            manager.approval_join_record(&network_id).await.unwrap(),
            record
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);

        let open_path = temp_path("approval-join-open-rejected");
        let open_manager =
            DiscoveryIndexManager::new(open_path, Arc::new(FakeTransport::default()));
        let open =
            signed_network_revision_from_profile(&identity, &signed_network_profile(&identity), 1);
        open_manager
            .accept_record_locked(open, now_unix_ms(), true)
            .unwrap();
        let error = open_manager
            .approval_join_record(&network_id)
            .await
            .unwrap_err();
        assert_eq!(error.reason_key, Some("join.error.unavailable"));

        let full_path = temp_path("approval-join-full-rejected");
        let full_manager =
            DiscoveryIndexManager::new(full_path, Arc::new(FakeTransport::default()));
        let full_profile =
            signed_network_profile_with(&identity, NetworkJoinMode::ApprovalRequired, Some(1), 0);
        let full = signed_network_revision_from_profile(&identity, &full_profile, 1);
        full_manager
            .accept_record_locked(full, now_unix_ms(), true)
            .unwrap();
        let error = full_manager
            .approval_join_record(&network_id)
            .await
            .unwrap_err();
        assert_eq!(error.reason_key, Some("join.error.member_limit"));

        let no_endpoint_path = temp_path("approval-join-no-endpoint");
        let no_endpoint_manager =
            DiscoveryIndexManager::new(no_endpoint_path, Arc::new(FakeTransport::default()));
        let now = now_unix_ms();
        let document = NetworkDiscoveryDocument::from_record_with_endpoints(
            &profile,
            1,
            now.saturating_sub(1),
            now.saturating_add(NETWORK_DISCOVERY_VALIDITY_MS),
            None,
            Vec::new(),
        )
        .unwrap();
        let no_endpoint = NetworkDiscoveryRecord::sign(document, &identity).unwrap();
        no_endpoint_manager
            .accept_record_locked(no_endpoint, now, true)
            .unwrap();
        let error = no_endpoint_manager
            .approval_join_record(&network_id)
            .await
            .unwrap_err();
        assert_eq!(error.reason_key, Some("join.error.endpoint_unavailable"));
    }

    #[tokio::test]
    async fn approval_join_record_rejects_tampering_expiry_and_revocation() {
        let manager = DiscoveryIndexManager::new(
            temp_path("approval-join-invalid"),
            Arc::new(FakeTransport::default()),
        );
        let identity = Identity::generate().unwrap();
        let profile =
            signed_network_profile_with(&identity, NetworkJoinMode::ApprovalRequired, None, 0);
        let network_id = profile.network_id.to_string();
        let endpoint = NetworkEndpoint {
            transport: DiscoveryTransport::Websocket,
            host: "coordinator.home.example".into(),
            port: 443,
        };
        let now = now_unix_ms();

        let expired_document = NetworkDiscoveryDocument::from_record_with_endpoints(
            &profile,
            1,
            now.saturating_sub(100),
            now.saturating_sub(1),
            None,
            vec![endpoint.clone()],
        )
        .unwrap();
        let expired = NetworkDiscoveryRecord::sign(expired_document, &identity).unwrap();
        seed_persisted_record(&manager, expired);
        assert_eq!(
            manager
                .approval_join_record(&network_id)
                .await
                .unwrap_err()
                .category,
            NodeDiscoveryIndexErrorCategory::Rejected
        );

        let revoked_document = NetworkDiscoveryDocument::from_record_with_endpoints(
            &profile,
            2,
            now.saturating_sub(100),
            now.saturating_add(NETWORK_DISCOVERY_VALIDITY_MS),
            Some(now.saturating_sub(100)),
            vec![endpoint.clone()],
        )
        .unwrap();
        let revoked = NetworkDiscoveryRecord::sign(revoked_document, &identity).unwrap();
        seed_persisted_record(&manager, revoked);
        assert_eq!(
            manager
                .approval_join_record(&network_id)
                .await
                .unwrap_err()
                .reason_key,
            Some("join.error.revoked")
        );

        let valid_document = NetworkDiscoveryDocument::from_record_with_endpoints(
            &profile,
            3,
            now.saturating_sub(1),
            now.saturating_add(NETWORK_DISCOVERY_VALIDITY_MS),
            None,
            vec![endpoint],
        )
        .unwrap();
        let mut tampered = NetworkDiscoveryRecord::sign(valid_document, &identity).unwrap();
        tampered.document.name = "tampered".into();
        seed_persisted_record(&manager, tampered);
        assert_eq!(
            manager
                .approval_join_record(&network_id)
                .await
                .unwrap_err()
                .category,
            NodeDiscoveryIndexErrorCategory::Signature
        );
    }

    #[tokio::test]
    async fn join_acceptance_rejects_forgery_expiry_revocation_replay_and_offline_index() {
        let path = temp_path("join-security-matrix");
        let transport = Arc::new(FakeTransport::default());
        let manager = DiscoveryIndexManager::new(path, transport.clone());
        let state = NodeStateStore::new();
        let identity = Identity::generate().unwrap();
        let profile = signed_network_profile(&identity);
        let valid = signed_network_revision_from_profile(&identity, &profile, 2);
        let network_id = valid.document.network_id.to_string();
        let endpoint = NetworkEndpoint {
            transport: DiscoveryTransport::Websocket,
            host: "coordinator.home.example".into(),
            port: 443,
        };
        let now = now_unix_ms();

        let mut forged = valid.clone();
        forged.document.purpose = "forged public purpose".into();
        assert_eq!(
            manager
                .accept_join_record(&state, forged)
                .await
                .unwrap_err()
                .category,
            NodeDiscoveryIndexErrorCategory::Signature
        );

        let expired_document = NetworkDiscoveryDocument::from_record_with_endpoints(
            &profile,
            3,
            now.saturating_sub(100),
            now.saturating_sub(1),
            None,
            vec![endpoint.clone()],
        )
        .unwrap();
        let expired = NetworkDiscoveryRecord::sign(expired_document, &identity).unwrap();
        assert_eq!(
            manager
                .accept_join_record(&state, expired)
                .await
                .unwrap_err()
                .category,
            NodeDiscoveryIndexErrorCategory::Rejected
        );

        let revoked_document = NetworkDiscoveryDocument::from_record_with_endpoints(
            &profile,
            4,
            now.saturating_sub(100),
            now.saturating_add(NETWORK_DISCOVERY_VALIDITY_MS),
            Some(now.saturating_sub(100)),
            vec![endpoint],
        )
        .unwrap();
        let revoked = NetworkDiscoveryRecord::sign(revoked_document, &identity).unwrap();
        assert_eq!(
            manager
                .accept_join_record(&state, revoked)
                .await
                .unwrap_err()
                .category,
            NodeDiscoveryIndexErrorCategory::Rejected
        );

        manager
            .accept_join_record(&state, valid)
            .await
            .expect("first valid signed record");
        let replay = signed_network_revision_from_profile(&identity, &profile, 1);
        assert_eq!(
            manager
                .accept_join_record(&state, replay)
                .await
                .unwrap_err()
                .category,
            NodeDiscoveryIndexErrorCategory::Rejected
        );

        *transport.fail_categories.lock().unwrap() =
            vec![NodeDiscoveryIndexErrorCategory::Offline; INDEX_MAX_ATTEMPTS];
        assert_eq!(
            manager
                .refresh(&state, Some("https://index.example"))
                .await
                .unwrap_err()
                .category,
            NodeDiscoveryIndexErrorCategory::Offline
        );
        let snapshot = state.snapshot().await;
        assert_eq!(
            snapshot.discovery_index.state,
            NodeDiscoveryIndexState::Degraded
        );
        assert!(snapshot.discovery_index.records.iter().any(|record| record
            .record
            .document
            .network_id
            .to_string()
            == network_id));
        assert!(!snapshot.discovery_index.data_plane_affected);
    }

    #[tokio::test]
    async fn join_confirmation_rejects_revision_race_after_preview() {
        let path = temp_path("join-confirm-revision-race");
        let manager = DiscoveryIndexManager::new(path, Arc::new(FakeTransport::default()));
        let identity = Identity::generate().unwrap();
        let first_profile = signed_network_profile(&identity);
        let first = signed_network_revision_from_profile(&identity, &first_profile, 1);
        let network_id = first.document.network_id.to_string();
        manager
            .accept_record_locked(first, now_unix_ms(), true)
            .unwrap();
        let preview = manager
            .preview_join_record(&network_id, false)
            .await
            .unwrap();

        let mut changed_profile = first_profile;
        changed_profile.policy_revision = 8;
        let changed = signed_network_revision_from_profile(&identity, &changed_profile, 2);
        manager
            .accept_record_locked(changed, now_unix_ms(), true)
            .unwrap();

        let error = manager
            .confirm_join_record(
                &network_id,
                &preview.preview_digest,
                preview.network_policy_revision,
                preview.privacy_policy_revision,
                &preview.terms_hash,
                false,
            )
            .await
            .unwrap_err();
        assert_eq!(error.reason_key, Some("join.error.preview_changed"));
    }

    #[test]
    fn persisted_state_never_contains_search_text_or_credentials() {
        let path = temp_path("privacy");
        let manager = DiscoveryIndexManager::new(path.clone(), Arc::new(FakeTransport::default()));
        let (record, _) = signed_network();
        manager
            .accept_record_locked(record, now_unix_ms(), true)
            .unwrap();
        let persisted = std::fs::read_to_string(path).unwrap();
        for forbidden in [
            "search",
            "query",
            "ticket",
            "invitation",
            "credential",
            "token",
        ] {
            assert!(!persisted.contains(forbidden));
        }
    }

    #[test]
    fn index_url_rejects_embedded_credentials_and_plaintext_remote_hosts() {
        assert!(validate_index_url("https://index.example/v1").is_ok());
        assert!(validate_index_url("http://127.0.0.1:8080").is_ok());
        assert!(validate_index_url("http://index.example").is_err());
        assert!(validate_index_url("https://user:secret@index.example").is_err());
        assert!(validate_index_url("https://index.example/?token=secret").is_err());
    }
}
