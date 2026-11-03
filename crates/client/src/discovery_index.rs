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
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fuckipv6_common::domain::{
    ContentHash, DomainError, MembershipTerms, NetworkDiscoveryDocument, NetworkDiscoveryRecord,
    NetworkJoinMode, NodeId, OperationId, NETWORK_DISCOVERY_VALIDITY_MS,
};
use fuckipv6_common::Identity;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;

use crate::config_manager::{write_json_atomic, ConfigManager};
use crate::node::NodeStateStore;

const DISCOVERY_INDEX_FILE: &str = "discovery_index.json";
const DISCOVERY_INDEX_SCHEMA_VERSION: u16 = 1;
const DISCOVERY_INDEX_WIRE_SCHEMA_VERSION: u16 = 1;
const MAX_INDEX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const INDEX_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

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
    Unsupported,
    Rejected,
    InvalidResponse,
    Storage,
    Server,
}

impl NodeDiscoveryIndexErrorCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Offline => "offline",
            Self::Timeout => "timeout",
            Self::Unsupported => "unsupported",
            Self::Rejected => "rejected",
            Self::InvalidResponse => "invalid_response",
            Self::Storage => "storage",
            Self::Server => "server",
        }
    }

    pub const fn retryable(self) -> bool {
        !matches!(self, Self::Unsupported)
    }

    pub const fn message_key(self) -> &'static str {
        match self {
            Self::Offline => "discovery.error.offline",
            Self::Timeout => "discovery.error.timeout",
            Self::Unsupported => "discovery.error.unsupported",
            Self::Rejected => "discovery.error.rejected",
            Self::InvalidResponse => "discovery.error.invalid_response",
            Self::Storage => "discovery.error.storage",
            Self::Server => "discovery.error.server",
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
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeDiscoveryIndexRecordSnapshot {
    pub record: NetworkDiscoveryRecord,
    pub digest: String,
    pub accepted_at_unix_ms: u64,
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
    pub rejected_record_count: usize,
    pub records: Vec<NodeDiscoveryIndexRecordSnapshot>,
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
            rejected_record_count: 0,
            records: Vec::new(),
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
            "domain.discovery_profile_unsigned"
            | "domain.discovery_profile_expired"
            | "domain.discovery_expired"
            | "domain.discovery_rollback"
            | "domain.discovery_conflict"
            | "domain.discovery_already_revoked"
            | "domain.discovery_unsupported_version"
            | "domain.discovery_publisher_mismatch"
            | "domain.invalid_signature" => NodeDiscoveryIndexErrorCategory::Rejected,
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

/// Injectable transport used by tests and embedders. Production uses
/// [`HttpDiscoveryIndexTransport`]. The command layer verifies every record
/// after transport, so a transport is never trusted as an authenticity source.
pub trait DiscoveryIndexTransport: Send + Sync + 'static {
    fn fetch<'a>(
        &'a self,
        index_url: &'a str,
    ) -> BoxFuture<'a, Result<Vec<NetworkDiscoveryRecord>, DiscoveryIndexError>>;

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
    records: Vec<NetworkDiscoveryRecord>,
}

impl DiscoveryIndexTransport for HttpDiscoveryIndexTransport {
    fn fetch<'a>(
        &'a self,
        index_url: &'a str,
    ) -> BoxFuture<'a, Result<Vec<NetworkDiscoveryRecord>, DiscoveryIndexError>> {
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
                    NodeDiscoveryIndexErrorCategory::Unsupported,
                ));
            }
            Ok(response.records)
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
    } else {
        NodeDiscoveryIndexErrorCategory::Offline
    };
    DiscoveryIndexError::new(category)
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
    rejected_record_count: usize,
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
        self.refresh_locked(state, url).await
    }

    pub async fn retry(
        &self,
        state: &NodeStateStore,
    ) -> Result<NodeDiscoveryIndexSnapshot, DiscoveryIndexError> {
        self.refresh(state, None).await
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
                self.fail_request(state, &url, error.category, now).await?;
                return Err(error);
            }
        };
        if let Err(error) = record.validate_at(now) {
            let error = DiscoveryIndexError::from_domain(error);
            self.fail_request(state, &url, error.category, now).await?;
            return Err(error);
        }

        let mut last_error = None;
        for attempt in 0..2 {
            match self.transport.publish(&url, &record).await {
                Ok(()) => {
                    last_error = None;
                    break;
                }
                Err(error) if error.retryable() && attempt == 0 => {
                    last_error = Some(error);
                }
                Err(error) => {
                    last_error = Some(error);
                    break;
                }
            }
        }
        if let Some(error) = last_error {
            self.fail_request(state, &url, error.category, now).await?;
            return Err(error);
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
    ) -> Result<NodeDiscoveryIndexSnapshot, DiscoveryIndexError> {
        let now = now_unix_ms();
        self.set_pending(state, &url).await;

        let mut response = None;
        let mut last_error = None;
        for attempt in 0..2 {
            match self.transport.fetch(&url).await {
                Ok(records) => {
                    response = Some(records);
                    last_error = None;
                    break;
                }
                Err(error) if error.retryable() && attempt == 0 => {
                    last_error = Some(error);
                }
                Err(error) => {
                    last_error = Some(error);
                    break;
                }
            }
        }
        let Some(records) = response else {
            let error = last_error.unwrap_or_else(|| {
                DiscoveryIndexError::new(NodeDiscoveryIndexErrorCategory::Offline)
            });
            self.fail_request(state, &url, error.category, now).await?;
            return Err(error);
        };

        let mut current = self
            .persisted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        current.schema_version = DISCOVERY_INDEX_SCHEMA_VERSION;
        current.index_url = Some(url);
        current.last_attempt_unix_ms = Some(now);
        let mut rejected = 0usize;
        for record in records {
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
            let Ok(()) = validation else {
                rejected += 1;
                continue;
            };
            let Ok(digest) = record.digest() else {
                rejected += 1;
                continue;
            };
            current.records.insert(
                id,
                PersistedDiscoveryRecord {
                    record,
                    digest,
                    accepted_at_unix_ms: now,
                },
            );
        }
        current.rejected_record_count = rejected;
        if rejected == 0 {
            current.last_success_unix_ms = Some(now);
            current.last_error = None;
            current.state = NodeDiscoveryIndexState::Ready;
        } else {
            current.last_error = Some(NodeDiscoveryIndexErrorCategory::Rejected);
            current.state = if current.records.is_empty() {
                NodeDiscoveryIndexState::Failed
            } else {
                NodeDiscoveryIndexState::Degraded
            };
        }
        if let Err(error) = self.persist_candidate(&current) {
            self.restore_state(state).await;
            return Err(error);
        }
        let snapshot = snapshot_from_persisted(&current, now);
        *self
            .persisted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = current;
        state.replace_discovery_index(snapshot.clone()).await;
        if rejected > 0 {
            return Err(DiscoveryIndexError::new(
                NodeDiscoveryIndexErrorCategory::Rejected,
            ));
        }
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
    ) -> Result<NodeDiscoveryIndexSnapshot, DiscoveryIndexError> {
        match self.finish_failure(url, category, now).await {
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
    ) -> Result<NodeDiscoveryIndexSnapshot, DiscoveryIndexError> {
        let mut current = self
            .persisted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        current.index_url = Some(url.to_owned());
        current.last_attempt_unix_ms = Some(now);
        current.last_error = Some(category);
        current.state = state_for(category, !current.records.is_empty());
        self.persist_candidate(&current)?;
        let snapshot = snapshot_from_persisted(&current, now);
        *self
            .persisted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = current;
        Ok(snapshot)
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
            },
        );
        current.last_attempt_unix_ms = Some(now);
        if retry_failed_request {
            current.last_success_unix_ms = Some(now);
            current.last_error = None;
            current.rejected_record_count = 0;
            current.state = NodeDiscoveryIndexState::Ready;
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
            Some(NodeDiscoveryIndexErrorCategory::Unsupported),
        );
    }
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
    if category == NodeDiscoveryIndexErrorCategory::Unsupported {
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
        rejected_record_count: persisted.rejected_record_count,
        records,
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
        CapabilityKind, DiscoveryTransport, ExitContributionObligation, NetworkCreateProfile,
        NetworkEndpoint, NetworkJoinMode, NetworkRecord, NetworkVisibility, PublicKey,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct FakeTransport {
        records: Mutex<Vec<NetworkDiscoveryRecord>>,
        fail_fetch: AtomicUsize,
        published: Mutex<Vec<NetworkDiscoveryRecord>>,
    }

    impl DiscoveryIndexTransport for FakeTransport {
        fn fetch<'a>(
            &'a self,
            _index_url: &'a str,
        ) -> BoxFuture<'a, Result<Vec<NetworkDiscoveryRecord>, DiscoveryIndexError>> {
            Box::pin(async move {
                if self.fail_fetch.load(Ordering::SeqCst) > 0 {
                    self.fail_fetch.fetch_sub(1, Ordering::SeqCst);
                    return Err(DiscoveryIndexError::new(
                        NodeDiscoveryIndexErrorCategory::Timeout,
                    ));
                }
                Ok(self.records.lock().unwrap().clone())
            })
        }

        fn publish<'a>(
            &'a self,
            _index_url: &'a str,
            record: &'a NetworkDiscoveryRecord,
        ) -> BoxFuture<'a, Result<(), DiscoveryIndexError>> {
            Box::pin(async move {
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
        transport.fail_fetch.store(2, Ordering::SeqCst);
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
        transport.fail_fetch.store(2, Ordering::SeqCst);
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

        transport.fail_fetch.store(2, Ordering::SeqCst);
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
        transport.fail_fetch.store(2, Ordering::SeqCst);
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
            NodeDiscoveryIndexErrorCategory::Rejected
        );
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
