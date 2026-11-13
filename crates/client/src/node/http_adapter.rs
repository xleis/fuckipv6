//! Reusable HTTP and WebSocket adapter for the transport-neutral node API.
//!
//! The adapter only projects the shared command bus, state store and event bus
//! onto HTTP/WebSocket. It never owns a node or network state machine.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{
    extract::{
        rejection::{JsonRejection, QueryRejection},
        ws::{rejection::WebSocketUpgradeRejection, Message, WebSocket, WebSocketUpgrade},
        DefaultBodyLimit, Extension, Path, Query, State,
    },
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{
        sse::{Event as SseEvent, KeepAlive, Sse},
        IntoResponse, Json, Response,
    },
    routing::{get, post},
    Router,
};
use chrono::{SecondsFormat, Utc};
use fuckipv6_common::domain::{
    ContentHash, DomainActorRef, DomainObjectRef, IdempotencyKey, IdempotencyRegistry,
    IdempotentSubmission, NodeId, ObjectKind, Operation, OperationError, OperationId,
    OperationKind, OperationRecovery, OperationState,
};
use futures::SinkExt as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, watch, OwnedSemaphorePermit, Semaphore};

use super::{project_node_command_result, project_node_event, project_node_snapshot};
use super::{
    NodeCommand, NodeCommandBus, NodeCommandBusError, NodeCommandContext, NodeCommandOutcome,
    NodeCommandRequest, NodeCommandResult, NodeControlHealth, NodeErrorCategory, NodeEvent,
    NodeEventBus, NodeEventCursor, NodeEventKind, NodeEventReplay, NodeEventReplayReason,
    NodeProblem, NodeProblemCode, NodeSnapshot, NodeSnapshotProjectionError, NodeStateStore,
    NodeUiManifest, ProjectedNodeCommandResult, ProjectedNodeEmbeddedServiceSnapshot,
    ProjectedNodeEvent, ProjectedNodeIdentitySnapshot, ProjectedNodeNetworkSnapshot,
    ProjectedNodePeerSnapshot, ProjectedNodeSnapshot, NODE_UI_MANIFEST_SCHEMA_VERSION,
};

/// Version of the HTTP/WebSocket envelope contract used by desktop and
/// browser node UIs. It is independent of the command and UI-manifest schemas.
pub const NODE_HTTP_SCHEMA_VERSION: u16 = 1;

/// Stable path prefix for the shared node control adapter.
pub const NODE_HTTP_API_PREFIX: &str = "/api/node";

/// Stable prefix for the versioned control API. The legacy `/api/node` routes
/// remain available while UI adapters migrate, but new clients must use v1.
pub const NODE_HTTP_V1_API_PREFIX: &str = "/api/v1";

/// Maximum JSON body accepted by either command endpoint. The limit is applied
/// by the HTTP adapter before JSON deserialization so oversized chunked or
/// content-length requests cannot consume unbounded memory.
pub const NODE_HTTP_MAX_COMMAND_BODY_BYTES: usize = 256 * 1024;

/// Maximum number of command requests an adapter processes concurrently.
/// Requests above this bound fail fast instead of accumulating in an
/// unbounded wait queue.
pub const NODE_HTTP_MAX_CONCURRENT_COMMANDS: usize = 32;

/// Identity rotation invalidates the WebUI owner session before its operation
/// can be polled. Keep the authenticated POST open long enough to return the
/// terminal result, but never claim success when the operation is still active.
const NODE_HTTP_IDENTITY_ROTATION_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Metadata carried by every v1 success and error response.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeApiResponseMeta {
    pub schema_version: u16,
    pub request_id: u64,
    pub server_time: String,
}

impl NodeApiResponseMeta {
    fn new(request_id: u64) -> Self {
        Self {
            schema_version: NODE_HTTP_SCHEMA_VERSION,
            request_id,
            server_time: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        }
    }
}

/// Versioned snapshot response used by `GET /api/v1/snapshot`.
#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct NodeApiSnapshotResponse {
    #[serde(flatten)]
    pub meta: NodeApiResponseMeta,
    pub snapshot: ProjectedNodeSnapshot,
}

/// Versioned UI-manifest response used by `GET /api/v1/manifest`.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeApiManifestResponse {
    #[serde(flatten)]
    pub meta: NodeApiResponseMeta,
    pub manifest: NodeUiManifest,
}

/// Versioned command result returned by `POST /api/v1/commands`.
#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct NodeApiCommandResponse {
    #[serde(flatten)]
    pub meta: NodeApiResponseMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<ProjectedNodeCommandResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<NodeOperationSnapshot>,
}

/// Transport projection of the shared `Operation` state machine. `result` is
/// only populated after a mutation reaches a terminal state; consumers must
/// treat a queued/running operation as in progress, never as successful.
#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct NodeOperationSnapshot {
    #[serde(flatten)]
    pub operation: Operation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<ProjectedNodeCommandResult>,
}

/// Versioned response for operation submission and lookup.
#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct NodeApiOperationResponse {
    #[serde(flatten)]
    pub meta: NodeApiResponseMeta,
    pub operation: NodeOperationSnapshot,
}

/// A single canonical object fetched by kind and ID. Objects are projections of
/// the authoritative snapshot, never independently maintained UI state.
#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeApiObject {
    Node {
        id: String,
        identity: ProjectedNodeIdentitySnapshot,
    },
    Network {
        id: String,
        network: ProjectedNodeNetworkSnapshot,
    },
    Peer {
        id: String,
        peer: ProjectedNodePeerSnapshot,
    },
    EmbeddedService {
        id: String,
        service: Box<ProjectedNodeEmbeddedServiceSnapshot>,
    },
    NetworkConfiguration {
        id: String,
        configuration: super::NodeNetworkConfigSnapshot,
    },
}

/// Versioned object response used by `GET /api/v1/objects/<kind>/<id>`.
#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct NodeApiObjectResponse {
    #[serde(flatten)]
    pub meta: NodeApiResponseMeta,
    pub object: NodeApiObject,
}

/// Versioned error response. The `problem` payload is optional because some
/// transport-level failures do not originate from a node command.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeApiErrorResponse {
    #[serde(flatten)]
    pub meta: NodeApiResponseMeta,
    #[serde(flatten)]
    pub error: NodeErrorDetails,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_schema_version: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supported_schema_version: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<NodeProblem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_cursor: Option<NodeEventCursor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_cursor: Option<NodeEventCursor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resync_reason: Option<NodeEventReplayReason>,
}

/// JSON payload carried by `/api/v1/events` Server-Sent Events. The stream
/// begins with a canonical snapshot and then carries ordered node events.
#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeApiEvent {
    Snapshot {
        schema_version: u16,
        request_id: u64,
        server_time: String,
        /// Resume position for this connection. The snapshot itself is already
        /// authoritative; replayed events after this cursor are notifications
        /// that the client may deduplicate by sequence.
        cursor: NodeEventCursor,
        snapshot: Box<ProjectedNodeSnapshot>,
    },
    Event {
        schema_version: u16,
        request_id: u64,
        server_time: String,
        event: ProjectedNodeEvent,
    },
    ResyncRequired {
        schema_version: u16,
        request_id: u64,
        server_time: String,
        requested_cursor: NodeEventCursor,
        latest_cursor: NodeEventCursor,
        reason: NodeEventReplayReason,
    },
}

/// Versioned DTO returned by the snapshot endpoint.
#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct NodeSnapshotResponse {
    pub schema_version: u16,
    pub snapshot: ProjectedNodeSnapshot,
}

/// Versioned DTO returned by the UI-manifest endpoint.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeManifestResponse {
    pub schema_version: u16,
    pub manifest: NodeUiManifest,
}

/// Versioned request DTO for `POST /api/node/commands`.
///
/// The embedded `NodeCommandRequest` is forwarded unchanged to
/// `NodeCommandBus`; the adapter does not inspect or mutate node state.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeCommandHttpRequest {
    pub schema_version: u16,
    pub request: NodeCommandRequest,
    /// Required for mutating commands. Reusing the key with the same command
    /// returns the original operation; reusing it with another command is a
    /// conflict and never dispatches a second side effect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<IdempotencyKey>,
}

/// Versioned response DTO containing the exact result returned by the command
/// bus. Rejected results remain visible in the body alongside their HTTP status.
#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct NodeCommandHttpResponse {
    pub schema_version: u16,
    pub result: ProjectedNodeCommandResult,
}

/// Stable error codes for HTTP version negotiation and backend failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeHttpErrorCode {
    SchemaVersionMismatch,
    InvalidRequest,
    PayloadTooLarge,
    RateLimited,
    NotFound,
    AuthenticationRequired,
    PermissionDenied,
    OperationConflict,
    OperationCancelled,
    OperationTimedOut,
    ResyncRequired,
    CapabilityUnavailable,
    DependencyUnavailable,
    BackendRejected,
    BackendUnavailable,
    ContractViolation,
    Internal,
}

/// Transport-neutral error metadata carried by HTTP, WebSocket and operation
/// failures. `message` remains a human-readable fallback; clients branch on
/// `code`, `category` and `message_key`, interpolate `params`, and use
/// `recovery` to offer a safe next action.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeErrorDetails {
    pub code: NodeHttpErrorCode,
    pub category: NodeErrorCategory,
    pub message: String,
    pub message_key: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object: Option<DomainObjectRef>,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<OperationRecovery>,
}

impl NodeHttpErrorCode {
    fn category(self) -> NodeErrorCategory {
        match self {
            Self::SchemaVersionMismatch | Self::InvalidRequest | Self::PayloadTooLarge => {
                NodeErrorCategory::Validation
            }
            Self::NotFound
            | Self::OperationConflict
            | Self::OperationCancelled
            | Self::ResyncRequired
            | Self::BackendRejected => NodeErrorCategory::State,
            Self::AuthenticationRequired => NodeErrorCategory::Authentication,
            Self::PermissionDenied => NodeErrorCategory::Authorization,
            Self::CapabilityUnavailable => NodeErrorCategory::Capability,
            Self::OperationTimedOut
            | Self::DependencyUnavailable
            | Self::BackendUnavailable
            | Self::RateLimited => NodeErrorCategory::Dependency,
            Self::ContractViolation | Self::Internal => NodeErrorCategory::Internal,
        }
    }

    fn message_key(self) -> &'static str {
        match self {
            Self::SchemaVersionMismatch => "error.schema_version_mismatch",
            Self::InvalidRequest => "error.invalid_request",
            Self::PayloadTooLarge => "error.payload_too_large",
            Self::RateLimited => "error.rate_limited",
            Self::NotFound => "error.not_found",
            Self::AuthenticationRequired => "error.authentication_required",
            Self::PermissionDenied => "error.permission_denied",
            Self::OperationConflict => "error.operation_conflict",
            Self::OperationCancelled => "error.operation_cancelled",
            Self::OperationTimedOut => "error.operation_timed_out",
            Self::ResyncRequired => "error.resync_required",
            Self::CapabilityUnavailable => "error.capability_unavailable",
            Self::DependencyUnavailable | Self::BackendUnavailable => {
                "error.dependency_unavailable"
            }
            Self::BackendRejected => "error.backend_rejected",
            Self::ContractViolation => "error.contract_violation",
            Self::Internal => "error.internal",
        }
    }

    fn default_message(self) -> &'static str {
        match self {
            Self::SchemaVersionMismatch => "请求的节点 API 版本不受支持",
            Self::InvalidRequest => "请求无效",
            Self::PayloadTooLarge => "请求体超过允许的大小",
            Self::RateLimited => "请求过于频繁，请稍后重试",
            Self::NotFound => "请求的对象不存在",
            Self::AuthenticationRequired => "需要重新进行身份认证",
            Self::PermissionDenied => "无权执行此操作",
            Self::OperationConflict => "当前状态与请求冲突",
            Self::OperationCancelled => "操作已取消",
            Self::OperationTimedOut => "操作已超时",
            Self::ResyncRequired => "事件流已无法安全续接，请重新读取快照",
            Self::CapabilityUnavailable => "当前节点不支持所需能力",
            Self::DependencyUnavailable | Self::BackendUnavailable => "依赖服务暂不可用",
            Self::BackendRejected => "节点拒绝了请求",
            Self::ContractViolation => "节点返回了不兼容的响应",
            Self::Internal => "节点内部错误",
        }
    }

    fn recovery(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::SchemaVersionMismatch | Self::InvalidRequest | Self::PayloadTooLarge => {
                Some(("edit_request", "recovery.fix_request"))
            }
            Self::NotFound | Self::OperationConflict | Self::ResyncRequired => {
                Some(("refresh", "recovery.refresh_state"))
            }
            Self::AuthenticationRequired => Some(("reauth", "recovery.reauthenticate")),
            Self::PermissionDenied => Some(("request_access", "recovery.request_access")),
            Self::OperationCancelled
            | Self::OperationTimedOut
            | Self::DependencyUnavailable
            | Self::BackendRejected
            | Self::BackendUnavailable
            | Self::RateLimited => Some(("retry", "recovery.retry")),
            Self::CapabilityUnavailable => {
                Some(("change_request", "recovery.use_supported_capability"))
            }
            Self::ContractViolation | Self::Internal => {
                Some(("inspect_diagnostics", "recovery.inspect_diagnostics"))
            }
        }
    }

    fn retryable(self) -> bool {
        matches!(
            self,
            Self::OperationTimedOut
                | Self::DependencyUnavailable
                | Self::BackendUnavailable
                | Self::BackendRejected
                | Self::RateLimited
        )
    }
}

impl NodeErrorDetails {
    fn new(code: NodeHttpErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            category: code.category(),
            message: message.into(),
            message_key: code.message_key().into(),
            params: BTreeMap::new(),
            object: None,
            retryable: code.retryable(),
            recovery: code.recovery().map(|(kind, hint_key)| OperationRecovery {
                kind: kind.into(),
                hint_key: hint_key.into(),
            }),
        }
    }

    fn with_param(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.params.insert(key.into(), value.into());
        self
    }

    fn from_problem(problem: &NodeProblem, object: Option<&DomainObjectRef>) -> Self {
        let code = code_for_problem(problem.code);
        let hide_object = matches!(
            problem.category,
            NodeErrorCategory::Authentication | NodeErrorCategory::Authorization
        );
        Self {
            code,
            category: problem.category,
            message: if hide_object {
                code.default_message().into()
            } else {
                problem.message.clone()
            },
            message_key: problem.message_key.clone(),
            params: if hide_object {
                BTreeMap::new()
            } else {
                (*problem.params).clone()
            },
            object: if hide_object { None } else { object.cloned() },
            retryable: problem.retryable,
            recovery: problem.recovery.as_deref().cloned().or_else(|| {
                code.recovery().map(|(kind, hint_key)| OperationRecovery {
                    kind: kind.into(),
                    hint_key: hint_key.into(),
                })
            }),
        }
    }
}

/// Versioned HTTP error DTO. It is also used for WebSocket version mismatches
/// so clients receive a machine-readable failure before an upgrade.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeHttpErrorResponse {
    pub schema_version: u16,
    #[serde(flatten)]
    pub error: NodeErrorDetails,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_schema_version: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supported_schema_version: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<NodeProblem>,
}

/// Versioned WebSocket messages. A snapshot is sent immediately after upgrade;
/// every later state or operational event carries the same adapter version.
#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeWsMessage {
    Snapshot {
        schema_version: u16,
        snapshot: Box<ProjectedNodeSnapshot>,
    },
    Event {
        schema_version: u16,
        event: ProjectedNodeEvent,
    },
    Resync {
        schema_version: u16,
        reason: NodeWsResyncReason,
        snapshot: Box<ProjectedNodeSnapshot>,
    },
    Error {
        schema_version: u16,
        #[serde(flatten)]
        error: NodeErrorDetails,
    },
}

impl NodeWsMessage {
    pub fn schema_version(&self) -> u16 {
        match self {
            Self::Snapshot { schema_version, .. }
            | Self::Event { schema_version, .. }
            | Self::Resync { schema_version, .. }
            | Self::Error { schema_version, .. } => *schema_version,
        }
    }
}

/// Why the server replaced the event stream with a fresh canonical snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeWsResyncReason {
    /// The broadcast receiver fell behind and lost one or more events.
    Lagged,
    /// Event revisions skipped values, so incremental application is unsafe.
    RevisionGap,
}

/// The WebSocket projection could not produce a safe outbound message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NodeWsSessionError {
    /// The canonical store moved backwards in revision.
    #[error(
        "节点状态 revision 回退: last_revision={last_revision}, observed_revision={observed_revision}"
    )]
    RevisionRollback {
        last_revision: u64,
        observed_revision: u64,
    },
    /// Field projection failed closed instead of emitting canonical state.
    #[error("节点事件字段投影失败: {0}")]
    Projection(#[from] NodeSnapshotProjectionError),
}

/// Per-connection cursors over the canonical event stream.
///
/// This is transport state only: the latest durable state always comes from
/// `NodeStateStore`, and no network/runtime state is copied or derived here.
pub struct NodeWsSession {
    receiver: broadcast::Receiver<NodeEvent>,
    state: NodeStateStore,
    context: NodeCommandContext,
    last_sequence: Option<u64>,
    last_revision: u64,
}

impl NodeWsSession {
    pub fn new(
        receiver: broadcast::Receiver<NodeEvent>,
        state: NodeStateStore,
        context: NodeCommandContext,
        initial_revision: u64,
    ) -> Self {
        Self {
            receiver,
            state,
            context,
            last_sequence: None,
            last_revision: initial_revision,
        }
    }

    /// Produce the next non-duplicate message. Lagging and revision gaps are
    /// converted into a fresh snapshot so clients never apply a partial event
    /// stream as if it were contiguous.
    pub async fn next_message(&mut self) -> Result<Option<NodeWsMessage>, NodeWsSessionError> {
        loop {
            let event = match self.receiver.recv().await {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    self.last_sequence = None;
                    return self
                        .resync(NodeWsResyncReason::Lagged, None)
                        .await
                        .map(Some);
                }
                Err(broadcast::error::RecvError::Closed) => return Ok(None),
            };

            if self
                .last_sequence
                .is_some_and(|sequence| event.sequence <= sequence)
            {
                continue;
            }
            self.last_sequence = Some(event.sequence);

            if let NodeEventKind::StateChanged { revision } = event.kind {
                if revision <= self.last_revision {
                    continue;
                }
                if revision > self.last_revision.saturating_add(1) {
                    return self
                        .resync(NodeWsResyncReason::RevisionGap, Some(revision))
                        .await
                        .map(Some);
                }
                self.last_revision = revision;
            }

            let event = project_node_event(&event, &self.context, unix_time_ms())?;
            return Ok(Some(NodeWsMessage::Event {
                schema_version: NODE_HTTP_SCHEMA_VERSION,
                event,
            }));
        }
    }

    async fn resync(
        &mut self,
        reason: NodeWsResyncReason,
        minimum_revision: Option<u64>,
    ) -> Result<NodeWsMessage, NodeWsSessionError> {
        let canonical = self.state.snapshot().await;
        let required_revision = minimum_revision.unwrap_or(self.last_revision);
        if canonical.revision < required_revision {
            return Err(NodeWsSessionError::RevisionRollback {
                last_revision: required_revision,
                observed_revision: canonical.revision,
            });
        }
        let snapshot = project_snapshot_for_control_client(&canonical, &self.context)?;
        self.last_revision = canonical.revision;
        Ok(NodeWsMessage::Resync {
            schema_version: NODE_HTTP_SCHEMA_VERSION,
            reason,
            snapshot: Box::new(snapshot),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NodeHttpAdapterError {
    #[error("节点 HTTP schema 版本不兼容: requested={requested}, supported={supported}")]
    SchemaVersionMismatch { requested: u16, supported: u16 },
    #[error("节点命令执行失败: {0}")]
    Command(#[from] NodeCommandBusError),
    #[error("节点命令被拒绝: {0:?}")]
    Rejected(NodeProblem),
    #[error("节点命令返回了适配器不支持的 outcome: {0}")]
    UnexpectedOutcome(String),
    #[error("节点 UI manifest schema 不兼容: current={current}, supported={supported}")]
    ManifestSchemaMismatch { current: u16, supported: u16 },
    #[error("节点 UI manifest 违反共享契约: {violations:?}")]
    InvalidManifest { violations: Vec<String> },
    #[error("节点字段投影失败: {0}")]
    Projection(#[from] NodeSnapshotProjectionError),
}

#[derive(Clone)]
struct OperationRecord {
    operation: Operation,
    result: Option<NodeCommandResult>,
    cancel: Option<watch::Sender<bool>>,
}

/// Canonical operation record used inside the adapter. Only [`Self::project`]
/// may construct the outbound DTO.
#[derive(Clone)]
struct OperationView {
    operation: Operation,
    result: Option<NodeCommandResult>,
}

impl OperationView {
    fn project(
        &self,
        context: &NodeCommandContext,
    ) -> Result<NodeOperationSnapshot, NodeSnapshotProjectionError> {
        Ok(NodeOperationSnapshot {
            operation: self.operation.clone(),
            result: self
                .result
                .as_ref()
                .map(|result| project_result_for_control_client(result, context))
                .transpose()?,
        })
    }
}

#[derive(Default)]
struct OperationRegistry {
    idempotency: IdempotencyRegistry,
    records: BTreeMap<String, OperationRecord>,
}

enum OperationSubmitError {
    Invalid(NodeProblem),
    Conflict(OperationId),
    Internal(String),
}

enum OperationCancellationError {
    NotFound,
    Conflict(NodeProblem),
    Internal(String),
}

/// HTTP/WebSocket projection over the shared node bus and state store.
///
/// The adapter owns a request-id allocator and an in-process projection of
/// idempotency keys and operation states. Durable node and network state still
/// remains in the shared services passed to [`NodeHttpAdapter::new`]; the
/// operation registry contains transport lifecycle metadata rather than a
/// second product state machine.
#[derive(Clone)]
pub struct NodeHttpAdapter {
    commands: NodeCommandBus,
    state: NodeStateStore,
    events: NodeEventBus,
    next_request_id: Arc<AtomicU64>,
    operations: Arc<Mutex<OperationRegistry>>,
    command_slots: Arc<Semaphore>,
    command_concurrency_limit: usize,
    identity_rotation_wait_timeout: Duration,
}

impl NodeHttpAdapter {
    pub fn new(commands: NodeCommandBus, state: NodeStateStore, events: NodeEventBus) -> Self {
        Self {
            commands,
            state,
            events,
            next_request_id: Arc::new(AtomicU64::new(1)),
            operations: Arc::new(Mutex::new(OperationRegistry::default())),
            command_slots: Arc::new(Semaphore::new(NODE_HTTP_MAX_CONCURRENT_COMMANDS)),
            command_concurrency_limit: NODE_HTTP_MAX_CONCURRENT_COMMANDS,
            identity_rotation_wait_timeout: NODE_HTTP_IDENTITY_ROTATION_WAIT_TIMEOUT,
        }
    }

    pub(crate) fn durable_audit(
        &self,
    ) -> Option<Arc<crate::owner_security::audit::OwnerSecurityAudit>> {
        self.commands.durable_audit()
    }

    #[cfg(test)]
    fn with_command_concurrency_limit(mut self, limit: usize) -> Self {
        self.command_concurrency_limit = limit.max(1);
        self.command_slots = Arc::new(Semaphore::new(self.command_concurrency_limit));
        self
    }

    #[cfg(test)]
    fn with_identity_rotation_wait_timeout(mut self, timeout: Duration) -> Self {
        self.identity_rotation_wait_timeout = timeout;
        self
    }

    fn try_acquire_command_permit(&self) -> Option<OwnedSemaphorePermit> {
        self.command_slots.clone().try_acquire_owned().ok()
    }

    fn operation(&self, id: &str) -> Option<OperationView> {
        let registry = self
            .operations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        registry.records.get(id).map(|record| OperationView {
            operation: record.operation.clone(),
            result: record.result.clone(),
        })
    }

    fn request_operation_cancellation(
        &self,
        context: &NodeCommandContext,
        id: &str,
    ) -> Result<OperationView, OperationCancellationError> {
        let snapshot = {
            let mut registry = self
                .operations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(record) = registry.records.get_mut(id) else {
                return Err(OperationCancellationError::NotFound);
            };
            // Operations are private to their submitting principal. Returning
            // not-found for another actor avoids leaking operation existence.
            if !operation_visible_to(context, &record.operation) {
                return Err(OperationCancellationError::NotFound);
            }
            if record.operation.state.is_terminal() {
                return Err(OperationCancellationError::Conflict(
                    NodeProblem::new(NodeProblemCode::Conflict, "操作已结束，无法取消")
                        .with_message_key("operation.cancel.already_terminal"),
                ));
            }
            if !record.operation.cancellable {
                return Err(OperationCancellationError::Conflict(
                    NodeProblem::new(NodeProblemCode::Conflict, "此操作不支持取消")
                        .with_message_key(
                            record
                                .operation
                                .cancel_blocked_reason
                                .clone()
                                .unwrap_or_else(|| "operation.cancel.not_supported".into()),
                        ),
                ));
            }
            let sender = record.cancel.as_ref().ok_or_else(|| {
                OperationCancellationError::Internal(
                    "operation cancellation channel unavailable".into(),
                )
            })?;
            sender.send_replace(true);
            // Progress is best-effort here because cancellation may arrive
            // while queued. The terminal state still waits for the handler.
            let _ = record.operation.progress("cancelling", unix_time_ms());
            OperationView {
                operation: record.operation.clone(),
                result: record.result.clone(),
            }
        };
        self.publish_operation(&snapshot.operation);
        Ok(snapshot)
    }

    async fn wait_for_operation_terminal(
        &self,
        id: &OperationId,
        wait_timeout: Duration,
    ) -> Option<OperationView> {
        let deadline = tokio::time::Instant::now() + wait_timeout;
        loop {
            if let Some(operation) = self.operation(id.as_str()) {
                if operation.operation.state.is_terminal() {
                    return Some(operation);
                }
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return self.operation(id.as_str());
            }
            tokio::time::sleep((deadline - now).min(Duration::from_millis(20))).await;
        }
    }

    /// Register and launch one operation command. Registration and idempotency
    /// resolution happen under the same lock, so concurrent retries cannot
    /// dispatch duplicate work.
    async fn submit_operation(
        &self,
        context: NodeCommandContext,
        request: NodeCommandRequest,
        idempotency_key: IdempotencyKey,
    ) -> Result<(OperationView, bool), OperationSubmitError> {
        request.validate().map_err(OperationSubmitError::Invalid)?;
        if !request.command.kind().is_operation() {
            return Err(OperationSubmitError::Internal(
                "command cannot be registered as an operation".into(),
            ));
        }

        let request_digest = ContentHash::from_bytes(
            &serde_json::to_vec(&request.command)
                .map_err(|error| OperationSubmitError::Internal(error.to_string()))?,
        );
        let actor = DomainActorRef {
            node_id: context.actor().clone(),
            display: context.source().to_owned(),
        };
        let operation_id = new_operation_id()?;
        let now = unix_time_ms();
        let cancellable = matches!(&request.command, NodeCommand::GetDiagnostics);
        let operation = Operation {
            operation_id: operation_id.clone(),
            kind: operation_kind(&request.command),
            actor: actor.clone(),
            target: operation_target(&request.command, &actor.node_id),
            idempotency_key: idempotency_key.clone(),
            state: OperationState::Queued,
            stage: "queued".into(),
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
            attempt: 0,
            max_attempts: 3,
            error: None,
            cancellable,
            cancel_blocked_reason: (!cancellable)
                .then(|| "operation.cancel.not_supported".to_owned()),
        };
        operation
            .validate()
            .map_err(|error| OperationSubmitError::Internal(error.to_string()))?;

        let (cancel_sender, cancel) = if cancellable {
            let (sender, receiver) = watch::channel(false);
            (Some(sender), Some(receiver))
        } else {
            (None, None)
        };

        let (snapshot, created) = {
            let mut registry = self
                .operations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match registry.idempotency.resolve_or_insert(
                &actor.node_id,
                &idempotency_key,
                operation_id.clone(),
                request_digest,
            ) {
                IdempotentSubmission::Created { operation_id } => {
                    registry.records.insert(
                        operation_id.as_str().to_owned(),
                        OperationRecord {
                            operation,
                            result: None,
                            cancel: cancel_sender.clone(),
                        },
                    );
                    let snapshot = registry
                        .records
                        .get(operation_id.as_str())
                        .map(|record| OperationView {
                            operation: record.operation.clone(),
                            result: None,
                        })
                        .expect("newly inserted operation");
                    (snapshot, true)
                }
                IdempotentSubmission::Existing { operation_id } => {
                    let snapshot = registry
                        .records
                        .get(operation_id.as_str())
                        .map(|record| OperationView {
                            operation: record.operation.clone(),
                            result: record.result.clone(),
                        })
                        .ok_or_else(|| {
                            OperationSubmitError::Internal(
                                "idempotency registry referenced a missing operation".into(),
                            )
                        })?;
                    (snapshot, false)
                }
                IdempotentSubmission::Conflict {
                    existing_operation_id,
                } => return Err(OperationSubmitError::Conflict(existing_operation_id)),
            }
        };

        if created {
            self.publish_operation(&snapshot.operation);
            let adapter = self.clone();
            let operation_id = snapshot.operation.operation_id.clone();
            let context = context.with_audit_operation_id(operation_id.clone());
            tokio::spawn(async move {
                adapter
                    .run_operation(operation_id, context, request, cancel)
                    .await;
            });
        }
        Ok((snapshot, created))
    }

    async fn run_operation(
        &self,
        operation_id: OperationId,
        context: NodeCommandContext,
        request: NodeCommandRequest,
        cancel: Option<watch::Receiver<bool>>,
    ) {
        let request_id = request.request_id;
        if !self.transition_operation(&operation_id, OperationState::Running, "dispatching") {
            return;
        }
        self.progress_operation(&operation_id, "executing");
        let result = match cancel {
            Some(cancel) => {
                self.commands
                    .execute_cancellable(context, request, cancel)
                    .await
            }
            None => self.commands.execute(context, request).await,
        };
        match result {
            Ok(result) => match result.outcome.clone() {
                NodeCommandOutcome::Rejected(problem)
                    if problem.code == NodeProblemCode::Cancelled =>
                {
                    self.cancel_operation(&operation_id, problem, result);
                }
                NodeCommandOutcome::Rejected(problem)
                    if problem.code == NodeProblemCode::TimedOut =>
                {
                    self.fail_operation(&operation_id, problem, result, true);
                }
                NodeCommandOutcome::Rejected(problem) => {
                    self.fail_operation(&operation_id, problem, result, false);
                }
                _ => self.succeed_operation(&operation_id, result),
            },
            Err(error) => {
                let problem = NodeProblem::new(NodeProblemCode::Internal, error.to_string());
                self.fail_operation(
                    &operation_id,
                    problem,
                    NodeCommandResult::rejected(
                        request_id,
                        NodeProblem::new(NodeProblemCode::Internal, error.to_string()),
                    ),
                    false,
                );
            }
        }
    }

    fn transition_operation(&self, id: &OperationId, state: OperationState, stage: &str) -> bool {
        let snapshot = {
            let mut registry = self
                .operations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(record) = registry.records.get_mut(id.as_str()) else {
                return false;
            };
            if record
                .operation
                .transition(state, stage, unix_time_ms())
                .is_err()
            {
                return false;
            }
            OperationView {
                operation: record.operation.clone(),
                result: record.result.clone(),
            }
        };
        self.publish_operation(&snapshot.operation);
        true
    }

    fn progress_operation(&self, id: &OperationId, stage: &str) -> bool {
        let snapshot = {
            let mut registry = self
                .operations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(record) = registry.records.get_mut(id.as_str()) else {
                return false;
            };
            if record.operation.progress(stage, unix_time_ms()).is_err() {
                return false;
            }
            OperationView {
                operation: record.operation.clone(),
                result: record.result.clone(),
            }
        };
        self.publish_operation(&snapshot.operation);
        true
    }

    fn succeed_operation(&self, id: &OperationId, result: NodeCommandResult) {
        let snapshot = {
            let mut registry = self
                .operations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(record) = registry.records.get_mut(id.as_str()) else {
                return;
            };
            if record
                .operation
                .transition(OperationState::Succeeded, "succeeded", unix_time_ms())
                .is_err()
            {
                return;
            }
            record.result = Some(result);
            OperationView {
                operation: record.operation.clone(),
                result: record.result.clone(),
            }
        };
        self.clear_cancel_sender(id);
        self.publish_operation(&snapshot.operation);
    }

    fn fail_operation(
        &self,
        id: &OperationId,
        problem: NodeProblem,
        result: NodeCommandResult,
        timed_out: bool,
    ) {
        let snapshot = {
            let mut registry = self
                .operations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(record) = registry.records.get_mut(id.as_str()) else {
                return;
            };
            let error = operation_error_for_problem(&problem, &record.operation.target);
            if record
                .operation
                .fail(error, timed_out, unix_time_ms())
                .is_err()
            {
                return;
            }
            record.result = Some(result);
            OperationView {
                operation: record.operation.clone(),
                result: record.result.clone(),
            }
        };
        self.clear_cancel_sender(id);
        self.publish_operation(&snapshot.operation);
    }

    fn cancel_operation(&self, id: &OperationId, problem: NodeProblem, result: NodeCommandResult) {
        let snapshot = {
            let mut registry = self
                .operations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(record) = registry.records.get_mut(id.as_str()) else {
                return;
            };
            let error = operation_error_for_problem(&problem, &record.operation.target);
            if record
                .operation
                .finish_cancelled(Some(error), unix_time_ms())
                .is_err()
            {
                return;
            }
            record.result = Some(result);
            OperationView {
                operation: record.operation.clone(),
                result: record.result.clone(),
            }
        };
        self.clear_cancel_sender(id);
        self.publish_operation(&snapshot.operation);
    }

    fn clear_cancel_sender(&self, id: &OperationId) {
        if let Some(record) = self
            .operations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .records
            .get_mut(id.as_str())
        {
            record.cancel.take();
        }
    }

    fn publish_operation(&self, operation: &Operation) {
        self.events.publish(NodeEventKind::OperationChanged {
            operation_id: operation.operation_id.as_str().to_owned(),
            state: operation.state,
            stage: operation.stage.clone(),
        });
    }

    /// Build a router. It intentionally contains only shared node endpoints;
    /// callers may merge it with static WebUI assets or other transport routes.
    pub fn router(self) -> Router {
        Router::new()
            .route(
                &format!("{NODE_HTTP_API_PREFIX}/snapshot"),
                get(handle_snapshot),
            )
            .route(
                &format!("{NODE_HTTP_API_PREFIX}/manifest"),
                get(handle_manifest),
            )
            .route(
                &format!("{NODE_HTTP_API_PREFIX}/events"),
                get(handle_events),
            )
            .route(
                &format!("{NODE_HTTP_API_PREFIX}/commands"),
                post(handle_command).layer(DefaultBodyLimit::max(NODE_HTTP_MAX_COMMAND_BODY_BYTES)),
            )
            .route(
                &format!("{NODE_HTTP_V1_API_PREFIX}/snapshot"),
                get(handle_v1_snapshot),
            )
            .route(
                &format!("{NODE_HTTP_V1_API_PREFIX}/manifest"),
                get(handle_v1_manifest),
            )
            .route(
                &format!("{NODE_HTTP_V1_API_PREFIX}/commands"),
                post(handle_v1_command)
                    .layer(DefaultBodyLimit::max(NODE_HTTP_MAX_COMMAND_BODY_BYTES)),
            )
            .route(
                &format!("{NODE_HTTP_V1_API_PREFIX}/operations/{{id}}"),
                get(handle_v1_operation),
            )
            .route(
                &format!("{NODE_HTTP_V1_API_PREFIX}/operations/{{id}}/cancel"),
                post(handle_v1_cancel_operation),
            )
            .route(
                &format!("{NODE_HTTP_V1_API_PREFIX}/objects/{{kind}}/{{id}}"),
                get(handle_v1_object),
            )
            .route(
                &format!("{NODE_HTTP_V1_API_PREFIX}/events"),
                get(handle_v1_events),
            )
            .with_state(self)
    }

    /// Submit any node command unchanged to the shared command bus.
    ///
    /// HTTP action handlers use this method directly. No action path may call
    /// `NodeStateStore::update` or maintain a second operation state machine.
    pub async fn execute(
        &self,
        context: NodeCommandContext,
        request: NodeCommandRequest,
    ) -> Result<NodeCommandResult, NodeHttpAdapterError> {
        self.commands
            .execute(context, request)
            .await
            .map_err(NodeHttpAdapterError::from)
    }

    async fn execute_generated(
        &self,
        context: NodeCommandContext,
        command: NodeCommand,
    ) -> Result<NodeCommandResult, NodeHttpAdapterError> {
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        self.execute(context, NodeCommandRequest::new(request_id, command))
            .await
    }

    pub(crate) fn state_store(&self) -> NodeStateStore {
        self.state.clone()
    }

    #[cfg(test)]
    fn test_router(self) -> Router {
        self.router()
            .layer(Extension(crate::node::test_owner_context()))
    }

    fn allocate_request_id(&self) -> u64 {
        self.next_request_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Fetch the latest snapshot through `NodeCommand::GetSnapshot`; the
    /// adapter never synthesizes a snapshot from private state.
    pub async fn snapshot(
        &self,
        context: NodeCommandContext,
    ) -> Result<NodeSnapshot, NodeHttpAdapterError> {
        let result = self
            .execute_generated(context, NodeCommand::GetSnapshot)
            .await?;
        match result.outcome {
            NodeCommandOutcome::Snapshot(snapshot) => Ok(*snapshot),
            NodeCommandOutcome::Rejected(problem) => Err(NodeHttpAdapterError::Rejected(problem)),
            outcome => Err(NodeHttpAdapterError::UnexpectedOutcome(format!(
                "{outcome:?}"
            ))),
        }
    }

    /// Fetch the declarative UI contract through `NodeCommand::GetUiManifest`.
    pub async fn manifest(
        &self,
        context: NodeCommandContext,
    ) -> Result<NodeUiManifest, NodeHttpAdapterError> {
        let result = self
            .execute_generated(context, NodeCommand::GetUiManifest)
            .await?;
        match result.outcome {
            NodeCommandOutcome::UiManifest(manifest) => {
                if manifest.schema_version != NODE_UI_MANIFEST_SCHEMA_VERSION {
                    return Err(NodeHttpAdapterError::ManifestSchemaMismatch {
                        current: manifest.schema_version,
                        supported: NODE_UI_MANIFEST_SCHEMA_VERSION,
                    });
                }
                if let Err(violations) = manifest.validate() {
                    return Err(NodeHttpAdapterError::InvalidManifest { violations });
                }
                Ok(*manifest)
            }
            NodeCommandOutcome::Rejected(problem) => Err(NodeHttpAdapterError::Rejected(problem)),
            outcome => Err(NodeHttpAdapterError::UnexpectedOutcome(format!(
                "{outcome:?}"
            ))),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct NodeHttpQuery {
    schema_version: Option<u16>,
}

fn require_schema_version(query: &NodeHttpQuery) -> Result<u16, HttpAdapterError> {
    require_schema_version_value(query.schema_version.unwrap_or(NODE_HTTP_SCHEMA_VERSION))
}

fn require_schema_version_value(version: u16) -> Result<u16, HttpAdapterError> {
    if version != NODE_HTTP_SCHEMA_VERSION {
        return Err(HttpAdapterError::schema_version_mismatch(
            version,
            NODE_HTTP_SCHEMA_VERSION,
        ));
    }
    Ok(version)
}

/// Stable HTTP status mapping for command-bus results.
fn status_for_command_result(result: &NodeCommandResult) -> StatusCode {
    match &result.outcome {
        NodeCommandOutcome::Rejected(problem) => status_for_problem(problem.code),
        NodeCommandOutcome::Accepted { .. } => StatusCode::ACCEPTED,
        NodeCommandOutcome::Snapshot(_)
        | NodeCommandOutcome::UiManifest(_)
        | NodeCommandOutcome::Diagnostics(_)
        | NodeCommandOutcome::Cleanup(_)
        | NodeCommandOutcome::Blacklist(_)
        | NodeCommandOutcome::NetworkAllocation(_)
        | NodeCommandOutcome::Probe(_)
        | NodeCommandOutcome::NetworkCreated(_)
        | NodeCommandOutcome::NetworkProfilePreview(_)
        | NodeCommandOutcome::NetworkProfileUpdated(_)
        | NodeCommandOutcome::NetworkJoinPreview(_)
        | NodeCommandOutcome::NetworkJoined(_)
        | NodeCommandOutcome::JoinRequestSubmitted(_)
        | NodeCommandOutcome::JoinRequests(_)
        | NodeCommandOutcome::NetworkMembers(_)
        | NodeCommandOutcome::JoinRequestsReviewed(_)
        | NodeCommandOutcome::JoinRequestUpdated(_)
        | NodeCommandOutcome::InvitationCreated(_)
        | NodeCommandOutcome::Invitations(_)
        | NodeCommandOutcome::InvitationStatus(_)
        | NodeCommandOutcome::InvitationRevoked(_)
        | NodeCommandOutcome::InvitationPreview(_)
        | NodeCommandOutcome::InvitationRedeemed(_)
        | NodeCommandOutcome::NetworkLeft(_)
        | NodeCommandOutcome::MembershipPolicyAcknowledged(_)
        | NodeCommandOutcome::LocalExitCapability(_)
        | NodeCommandOutcome::IdentityRotated(_)
        | NodeCommandOutcome::Configuration(_)
        | NodeCommandOutcome::ConfigurationPreview(_)
        | NodeCommandOutcome::ConfigurationApplied(_)
        | NodeCommandOutcome::ConfigurationHistory(_)
        | NodeCommandOutcome::Policy(_)
        | NodeCommandOutcome::PolicyPreview(_)
        | NodeCommandOutcome::PolicyApplied(_)
        | NodeCommandOutcome::PolicyHistory(_)
        | NodeCommandOutcome::PolicyRolledBack(_)
        | NodeCommandOutcome::PolicyTest(_)
        | NodeCommandOutcome::TrafficPolicy(_)
        | NodeCommandOutcome::TrafficPolicyPreview(_)
        | NodeCommandOutcome::TrafficPolicyApplied(_)
        | NodeCommandOutcome::TrafficPolicyHistory(_)
        | NodeCommandOutcome::TrafficPolicyRolledBack(_)
        | NodeCommandOutcome::TrafficPolicyTest(_)
        | NodeCommandOutcome::MemberRemovalPreview(_)
        | NodeCommandOutcome::MemberLifecycleChanged(_)
        | NodeCommandOutcome::Completed { .. } => StatusCode::OK,
    }
}

fn status_for_problem(code: NodeProblemCode) -> StatusCode {
    match code {
        NodeProblemCode::InvalidRequest => StatusCode::BAD_REQUEST,
        NodeProblemCode::NotFound => StatusCode::NOT_FOUND,
        NodeProblemCode::AuthenticationRequired => StatusCode::UNAUTHORIZED,
        NodeProblemCode::Conflict | NodeProblemCode::Cancelled => StatusCode::CONFLICT,
        NodeProblemCode::Unsupported => StatusCode::UNPROCESSABLE_ENTITY,
        NodeProblemCode::Unavailable | NodeProblemCode::ResourceExhausted => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        NodeProblemCode::PermissionDenied => StatusCode::FORBIDDEN,
        NodeProblemCode::TimedOut => StatusCode::GATEWAY_TIMEOUT,
        NodeProblemCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn code_for_problem(code: NodeProblemCode) -> NodeHttpErrorCode {
    match code {
        NodeProblemCode::AuthenticationRequired => NodeHttpErrorCode::AuthenticationRequired,
        NodeProblemCode::PermissionDenied => NodeHttpErrorCode::PermissionDenied,
        NodeProblemCode::InvalidRequest => NodeHttpErrorCode::InvalidRequest,
        NodeProblemCode::NotFound => NodeHttpErrorCode::NotFound,
        NodeProblemCode::Conflict => NodeHttpErrorCode::OperationConflict,
        NodeProblemCode::Cancelled => NodeHttpErrorCode::OperationCancelled,
        NodeProblemCode::TimedOut => NodeHttpErrorCode::OperationTimedOut,
        NodeProblemCode::Unavailable | NodeProblemCode::ResourceExhausted => {
            NodeHttpErrorCode::DependencyUnavailable
        }
        NodeProblemCode::Unsupported => NodeHttpErrorCode::CapabilityUnavailable,
        NodeProblemCode::Internal => NodeHttpErrorCode::Internal,
    }
}

fn new_operation_id() -> Result<OperationId, OperationSubmitError> {
    let mut random = [0_u8; 32];
    getrandom::fill(&mut random).map_err(|error| {
        OperationSubmitError::Internal(format!("operation id randomness unavailable: {error}"))
    })?;
    Ok(OperationId::from_public_key(&random))
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().try_into().unwrap_or(u64::MAX))
        .unwrap_or_default()
}

fn operation_kind(command: &NodeCommand) -> OperationKind {
    match command {
        NodeCommand::StartNetworks { .. } | NodeCommand::StartNode => OperationKind::NodeStart,
        NodeCommand::StopNetworks { .. } | NodeCommand::StopNode | NodeCommand::Shutdown => {
            OperationKind::NodeStop
        }
        NodeCommand::RestartNetworks { .. } | NodeCommand::RestartNode => {
            OperationKind::NodeRestart
        }
        NodeCommand::RebuildTun { .. } => OperationKind::TunRebuild,
        NodeCommand::CleanupResiduals { .. } => OperationKind::NodeCleanup,
        NodeCommand::CreateNetwork(_) => OperationKind::NetworkCreate,
        NodeCommand::UpdateNetworkProfile(_) => OperationKind::NetworkProfileUpdate,
        NodeCommand::PublishNetworkDiscovery(_)
        | NodeCommand::UpdateNetworkDiscovery(_)
        | NodeCommand::UnpublishNetworkDiscovery(_)
        | NodeCommand::RefreshDiscoveryIndex { .. }
        | NodeCommand::RetryDiscoveryIndex => OperationKind::DiscoveryIndexUpdate,
        NodeCommand::JoinNetwork(_)
        | NodeCommand::SubmitJoinRequest(_)
        | NodeCommand::ReviewJoinRequest(_)
        | NodeCommand::BatchReviewJoinRequests(_)
        | NodeCommand::ChangeMemberLifecycle(_)
        | NodeCommand::WithdrawJoinRequest(_)
        | NodeCommand::ResubmitJoinRequest(_)
        | NodeCommand::CreateInvitation(_)
        | NodeCommand::RevokeInvitation(_)
        | NodeCommand::RedeemInvitation(_)
        | NodeCommand::AcknowledgeMembershipPolicy(_) => OperationKind::NetworkApply,
        NodeCommand::LeaveNetwork(_) => OperationKind::NetworkLeave,
        NodeCommand::UpdateRouting(_) => OperationKind::TrafficRulesApply,
        NodeCommand::SetNetworkRole(_) => OperationKind::NetworkRoleUpdate,
        NodeCommand::SetLocalExitCapability(command) if command.enabled => {
            OperationKind::CapabilityEnable
        }
        NodeCommand::SetLocalExitCapability(_) => OperationKind::CapabilityDisable,
        NodeCommand::SetNodeDisplayName(_) | NodeCommand::RotateNodeIdentity(_) => {
            OperationKind::IdentityUpdate
        }
        NodeCommand::BanClient(_) | NodeCommand::UnbanClient(_) => {
            OperationKind::ClientBlacklistUpdate
        }
        NodeCommand::UpdateNetworkAllocation(_) => OperationKind::NetworkAllocationUpdate,
        NodeCommand::ApplyConfiguration(_) | NodeCommand::ApplyPolicy(_) => {
            OperationKind::PolicyApply
        }
        NodeCommand::RollbackConfiguration(_) | NodeCommand::RollbackPolicy(_) => {
            OperationKind::PolicyRollback
        }
        NodeCommand::GetDiagnostics => OperationKind::Diagnostics,
        // Synchronous read-only commands never enter the operation registry.
        // Keeping a total mapping avoids a panic if a future caller violates
        // that invariant before the command is rejected by `submit_operation`.
        NodeCommand::GetSnapshot
        | NodeCommand::GetUiManifest
        | NodeCommand::GetBlacklist { .. }
        | NodeCommand::GetNetworkAllocation { .. }
        | NodeCommand::ProbePeer(_)
        | NodeCommand::GetConfiguration
        | NodeCommand::PreviewConfiguration(_)
        | NodeCommand::PreviewNetworkProfile(_)
        | NodeCommand::PreviewJoinNetwork(_)
        | NodeCommand::ListJoinRequests(_)
        | NodeCommand::ListNetworkMembers(_)
        | NodeCommand::PreviewMemberRemoval(_)
        | NodeCommand::ListInvitations(_)
        | NodeCommand::GetInvitationStatus(_)
        | NodeCommand::PreviewInvitation(_)
        | NodeCommand::GetConfigurationHistory => OperationKind::ConfigurationHistory,
        NodeCommand::GetPolicy
        | NodeCommand::PreviewPolicy(_)
        | NodeCommand::PreviewPolicyRollback(_)
        | NodeCommand::GetPolicyHistory
        | NodeCommand::TestPolicy(_)
        | NodeCommand::GetTrafficPolicy
        | NodeCommand::PreviewTrafficPolicy(_)
        | NodeCommand::PreviewTrafficPolicyRollback(_)
        | NodeCommand::GetTrafficPolicyHistory
        | NodeCommand::TestTrafficPolicy(_) => OperationKind::PolicyTest,
        NodeCommand::ApplyTrafficPolicy(_) => OperationKind::TrafficRulesApply,
        NodeCommand::RollbackTrafficPolicy(_) => OperationKind::PolicyRollback,
    }
}

fn operation_target(command: &NodeCommand, local_node_id: &NodeId) -> DomainObjectRef {
    let node_target = || DomainObjectRef {
        kind: ObjectKind::Node,
        id: local_node_id.as_str().to_owned(),
    };
    let network_target = |id: &str| DomainObjectRef {
        kind: ObjectKind::Network,
        id: id.to_owned(),
    };
    match command {
        NodeCommand::CreateNetwork(command) => network_target(&command.name),
        NodeCommand::PreviewNetworkProfile(command)
        | NodeCommand::UpdateNetworkProfile(command) => network_target(&command.network_name),
        NodeCommand::PublishNetworkDiscovery(command)
        | NodeCommand::UpdateNetworkDiscovery(command)
        | NodeCommand::UnpublishNetworkDiscovery(command) => network_target(&command.network_name),
        NodeCommand::PreviewJoinNetwork(command) => network_target(&command.network_id),
        NodeCommand::ListJoinRequests(command) => network_target(&command.network_id),
        NodeCommand::ListNetworkMembers(command) => network_target(&command.network_id),
        NodeCommand::PreviewMemberRemoval(command) => network_target(&command.network_id),
        NodeCommand::ChangeMemberLifecycle(command) => network_target(&command.network_id),
        NodeCommand::SubmitJoinRequest(command) => network_target(&command.network_id),
        NodeCommand::ReviewJoinRequest(command) => network_target(&command.network_id),
        NodeCommand::BatchReviewJoinRequests(command) => network_target(&command.network_id),
        NodeCommand::WithdrawJoinRequest(command) => network_target(&command.network_id),
        NodeCommand::ResubmitJoinRequest(command) => network_target(&command.network_id),
        NodeCommand::CreateInvitation(command) => network_target(command.network_id.as_str()),
        NodeCommand::ListInvitations(command) => network_target(command.network_id.as_str()),
        NodeCommand::GetInvitationStatus(command) => network_target(command.network_id.as_str()),
        NodeCommand::RevokeInvitation(command) => network_target(command.network_id.as_str()),
        NodeCommand::PreviewInvitation(command) => network_target(command.network_id.as_str()),
        NodeCommand::RedeemInvitation(command) => {
            network_target(command.request.network_id.as_str())
        }
        NodeCommand::JoinNetwork(command) => command
            .network_name
            .as_deref()
            .or(command.network_id.as_deref())
            .map(network_target)
            .unwrap_or_else(node_target),
        NodeCommand::LeaveNetwork(command) => network_target(&command.network_name),
        NodeCommand::AcknowledgeMembershipPolicy(command) => network_target(&command.network_id),
        NodeCommand::UpdateRouting(command) => network_target(&command.network_name),
        NodeCommand::SetNetworkRole(command) => network_target(&command.network_name),
        NodeCommand::SetLocalExitCapability(_) => node_target(),
        NodeCommand::RebuildTun { network_name } => network_target(network_name),
        NodeCommand::BanClient(command) | NodeCommand::UnbanClient(command) => {
            network_target(&command.network_name)
        }
        NodeCommand::UpdateNetworkAllocation(command) => network_target(&command.network_name),
        NodeCommand::TestPolicy(command) => network_target(command.network_id.as_str()),
        NodeCommand::GetBlacklist { network_name }
        | NodeCommand::GetNetworkAllocation { network_name } => network_target(network_name),
        NodeCommand::StartNetworks { .. }
        | NodeCommand::StopNetworks { .. }
        | NodeCommand::RestartNetworks { .. }
        | NodeCommand::GetSnapshot
        | NodeCommand::GetUiManifest
        | NodeCommand::GetDiagnostics
        | NodeCommand::ProbePeer(_)
        | NodeCommand::StartNode
        | NodeCommand::StopNode
        | NodeCommand::RestartNode
        | NodeCommand::CleanupResiduals { .. }
        | NodeCommand::GetConfiguration
        | NodeCommand::PreviewConfiguration(_)
        | NodeCommand::ApplyConfiguration(_)
        | NodeCommand::RollbackConfiguration(_)
        | NodeCommand::GetConfigurationHistory
        | NodeCommand::GetPolicy
        | NodeCommand::PreviewPolicy(_)
        | NodeCommand::PreviewPolicyRollback(_)
        | NodeCommand::ApplyPolicy(_)
        | NodeCommand::RollbackPolicy(_)
        | NodeCommand::GetPolicyHistory
        | NodeCommand::GetTrafficPolicy
        | NodeCommand::PreviewTrafficPolicy(_)
        | NodeCommand::PreviewTrafficPolicyRollback(_)
        | NodeCommand::ApplyTrafficPolicy(_)
        | NodeCommand::RollbackTrafficPolicy(_)
        | NodeCommand::GetTrafficPolicyHistory
        | NodeCommand::TestTrafficPolicy(_)
        | NodeCommand::RefreshDiscoveryIndex { .. }
        | NodeCommand::RetryDiscoveryIndex
        | NodeCommand::SetNodeDisplayName(_)
        | NodeCommand::RotateNodeIdentity(_)
        | NodeCommand::Shutdown => node_target(),
    }
}

fn operation_error_for_problem(problem: &NodeProblem, target: &DomainObjectRef) -> OperationError {
    let hide_object = matches!(
        problem.category,
        NodeErrorCategory::Authentication | NodeErrorCategory::Authorization
    );
    let code = match problem.code {
        NodeProblemCode::AuthenticationRequired => "operation.authentication_required",
        NodeProblemCode::InvalidRequest => "operation.invalid_request",
        NodeProblemCode::NotFound => "operation.not_found",
        NodeProblemCode::Conflict => "operation.conflict",
        NodeProblemCode::Unavailable => "operation.unavailable",
        NodeProblemCode::ResourceExhausted => "operation.unavailable",
        NodeProblemCode::PermissionDenied => "operation.permission_denied",
        NodeProblemCode::Cancelled => "operation.cancelled",
        NodeProblemCode::TimedOut => "operation.timed_out",
        NodeProblemCode::Unsupported => "operation.unsupported",
        NodeProblemCode::Internal => "operation.internal",
    };
    let recovery = problem.recovery.as_deref().cloned().or_else(|| {
        problem
            .code
            .recovery()
            .map(|(kind, hint_key)| OperationRecovery {
                kind: kind.into(),
                hint_key: hint_key.into(),
            })
    });
    OperationError {
        code: code.into(),
        message_key: problem.message_key.clone(),
        params: if hide_object {
            BTreeMap::new()
        } else {
            (*problem.params).clone()
        },
        object: if hide_object {
            None
        } else {
            Some(target.clone())
        },
        retryable: problem.retryable,
        recovery,
        partial_changes: Vec::new(),
    }
}

#[derive(Debug, Default, Deserialize)]
struct NodeApiQuery {
    schema_version: Option<u16>,
    /// SSE cursor in `sequence:snapshot_revision` form. A bare sequence is
    /// accepted for migration and bound to the subscription's current revision.
    after: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestedNodeCursor {
    Sequence(u64),
    Cursor(NodeEventCursor),
}

impl RequestedNodeCursor {
    fn bind(self, boundary: NodeEventCursor) -> NodeEventCursor {
        match self {
            Self::Sequence(sequence) => NodeEventCursor::new(sequence, boundary.snapshot_revision),
            Self::Cursor(cursor) => cursor,
        }
    }
}

fn parse_node_cursor(value: &str) -> Option<RequestedNodeCursor> {
    let (sequence, revision) = match value.split_once(':') {
        Some((sequence, revision)) => (sequence, Some(revision)),
        None => (value, None),
    };
    let sequence = sequence.parse::<u64>().ok()?;
    match revision {
        Some(revision) => Some(RequestedNodeCursor::Cursor(NodeEventCursor::new(
            sequence,
            revision.parse::<u64>().ok()?,
        ))),
        None => Some(RequestedNodeCursor::Sequence(sequence)),
    }
}

fn requested_node_cursor(
    headers: &HeaderMap,
    query: &NodeApiQuery,
    request_id: u64,
) -> Result<Option<RequestedNodeCursor>, Box<HttpApiError>> {
    let value = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or(query.after.as_deref());
    let Some(value) = value else {
        return Ok(None);
    };
    parse_node_cursor(value).map(Some).ok_or_else(|| {
        Box::new(HttpApiError::invalid_request(
            request_id,
            "事件 cursor 无效: 应为 sequence 或 sequence:snapshot_revision",
        ))
    })
}

fn node_cursor_id(cursor: NodeEventCursor) -> String {
    format!("{}:{}", cursor.sequence, cursor.snapshot_revision)
}

fn api_json_rejection_response(request_id: u64, error: JsonRejection) -> Response {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        HttpApiError::payload_too_large(request_id).into_response()
    } else {
        HttpApiError::invalid_request(request_id, error.body_text()).into_response()
    }
}

fn with_retry_after(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    response
}

fn validate_api_query(query: &NodeApiQuery, request_id: u64) -> Result<(), Box<HttpApiError>> {
    let version = query.schema_version.unwrap_or(NODE_HTTP_SCHEMA_VERSION);
    if version != NODE_HTTP_SCHEMA_VERSION {
        return Err(Box::new(HttpApiError::schema_version_mismatch(
            request_id,
            version,
            NODE_HTTP_SCHEMA_VERSION,
        )));
    }
    Ok(())
}

async fn handle_v1_command(
    State(adapter): State<NodeHttpAdapter>,
    Extension(context): Extension<NodeCommandContext>,
    payload: Result<Json<NodeCommandHttpRequest>, JsonRejection>,
) -> Response {
    let payload = match payload {
        Ok(Json(payload)) => payload,
        Err(error) => {
            let request_id = adapter.allocate_request_id();
            return api_json_rejection_response(request_id, error);
        }
    };
    let request_id = payload.request.request_id;
    if payload.schema_version != NODE_HTTP_SCHEMA_VERSION {
        return HttpApiError::schema_version_mismatch(
            request_id,
            payload.schema_version,
            NODE_HTTP_SCHEMA_VERSION,
        )
        .into_response();
    }
    if request_id == 0 {
        return HttpApiError::invalid_request(request_id, "request_id 必须大于 0").into_response();
    }

    // Operation commands are registered before they execute. Authorize them
    // here as well so a stale WebUI owner cannot create an operation record (or
    // reach a handler) before the command bus rejects the request.
    if let Err(denial) = super::authorization::authorize_command(&context, &payload.request.command)
    {
        let result = super::command::denied(request_id, &denial);
        let status = status_for_command_result(&result);
        let result = match project_result_for_control_client(&result, &context) {
            Ok(result) => result,
            Err(error) => {
                return HttpApiError::from_adapter(
                    request_id,
                    NodeHttpAdapterError::Projection(error),
                )
                .into_response();
            }
        };
        return (
            status,
            Json(NodeApiCommandResponse {
                meta: NodeApiResponseMeta::new(request_id),
                result: Some(result),
                operation: None,
            }),
        )
            .into_response();
    }

    let _command_permit = match adapter.try_acquire_command_permit() {
        Some(permit) => permit,
        None => {
            return with_retry_after(
                HttpApiError::rate_limited(request_id, adapter.command_concurrency_limit)
                    .into_response(),
            );
        }
    };

    if payload.request.command.kind().is_operation() {
        let Some(idempotency_key) = payload.idempotency_key else {
            return HttpApiError::invalid_request(
                request_id,
                "operation 命令必须提供 idempotency_key，防止重试产生重复操作",
            )
            .into_response();
        };
        let rotates_identity =
            matches!(&payload.request.command, NodeCommand::RotateNodeIdentity(_));
        let projection_context = context.clone();
        return match adapter
            .submit_operation(context, payload.request, idempotency_key)
            .await
        {
            Ok((operation, _created)) => {
                let operation = if rotates_identity {
                    adapter
                        .wait_for_operation_terminal(
                            &operation.operation.operation_id,
                            adapter.identity_rotation_wait_timeout,
                        )
                        .await
                        .unwrap_or(operation)
                } else {
                    operation
                };
                if rotates_identity && operation.operation.state.is_terminal() {
                    if let Some(result) = operation.result.as_ref() {
                        let status = status_for_command_result(result);
                        let result =
                            match project_result_for_control_client(result, &projection_context) {
                                Ok(result) => result,
                                Err(error) => {
                                    return HttpApiError::from_adapter(
                                        request_id,
                                        NodeHttpAdapterError::Projection(error),
                                    )
                                    .into_response();
                                }
                            };
                        let operation = match operation.project(&projection_context) {
                            Ok(operation) => operation,
                            Err(error) => {
                                return HttpApiError::from_adapter(
                                    request_id,
                                    NodeHttpAdapterError::Projection(error),
                                )
                                .into_response();
                            }
                        };
                        return (
                            status,
                            Json(NodeApiCommandResponse {
                                meta: NodeApiResponseMeta::new(request_id),
                                result: Some(result),
                                operation: Some(operation),
                            }),
                        )
                            .into_response();
                    }
                }
                let operation = match operation.project(&projection_context) {
                    Ok(operation) => operation,
                    Err(error) => {
                        return HttpApiError::from_adapter(
                            request_id,
                            NodeHttpAdapterError::Projection(error),
                        )
                        .into_response();
                    }
                };
                (
                    StatusCode::ACCEPTED,
                    Json(NodeApiCommandResponse {
                        meta: NodeApiResponseMeta::new(request_id),
                        result: None,
                        operation: Some(operation),
                    }),
                )
                    .into_response()
            }
            Err(OperationSubmitError::Invalid(problem)) => {
                let mut error = HttpApiError::invalid_request(request_id, problem.message.clone());
                error.body.problem = Some(problem);
                error.into_response()
            }
            Err(OperationSubmitError::Conflict(existing_operation_id)) => {
                let mut error = HttpApiError::operation_conflict(
                    request_id,
                    format!(
                        "idempotency_key 已用于另一个命令；原操作: {}",
                        existing_operation_id.as_str()
                    ),
                );
                error.body.supported_schema_version = Some(NODE_HTTP_SCHEMA_VERSION);
                error.into_response()
            }
            Err(OperationSubmitError::Internal(message)) => {
                HttpApiError::internal(request_id, message).into_response()
            }
        };
    }

    let projection_context = context.clone();
    match adapter.execute(context, payload.request).await {
        Ok(result) => {
            let status = status_for_command_result(&result);
            let result = match project_result_for_control_client(&result, &projection_context) {
                Ok(result) => result,
                Err(error) => {
                    return HttpApiError::from_adapter(
                        request_id,
                        NodeHttpAdapterError::Projection(error),
                    )
                    .into_response();
                }
            };
            (
                status,
                Json(NodeApiCommandResponse {
                    meta: NodeApiResponseMeta::new(request_id),
                    result: Some(result),
                    operation: None,
                }),
            )
                .into_response()
        }
        Err(error) => HttpApiError::from_adapter(request_id, error).into_response(),
    }
}

async fn handle_v1_snapshot(
    State(adapter): State<NodeHttpAdapter>,
    Extension(context): Extension<NodeCommandContext>,
    query: Result<Query<NodeApiQuery>, QueryRejection>,
) -> Response {
    let request_id = adapter.allocate_request_id();
    let query = match query {
        Ok(Query(query)) => query,
        Err(error) => {
            return HttpApiError::invalid_request(request_id, error.body_text()).into_response();
        }
    };
    if let Err(error) = validate_api_query(&query, request_id) {
        return (*error).into_response();
    }

    match adapter.snapshot(context.clone()).await {
        Ok(snapshot) => match project_snapshot_for_control_client(&snapshot, &context) {
            Ok(snapshot) => Json(NodeApiSnapshotResponse {
                meta: NodeApiResponseMeta::new(request_id),
                snapshot,
            })
            .into_response(),
            Err(error) => {
                HttpApiError::from_adapter(request_id, NodeHttpAdapterError::Projection(error))
                    .into_response()
            }
        },
        Err(error) => HttpApiError::from_adapter(request_id, error).into_response(),
    }
}

async fn handle_v1_manifest(
    State(adapter): State<NodeHttpAdapter>,
    Extension(context): Extension<NodeCommandContext>,
    query: Result<Query<NodeApiQuery>, QueryRejection>,
) -> Response {
    let request_id = adapter.allocate_request_id();
    let query = match query {
        Ok(Query(query)) => query,
        Err(error) => {
            return HttpApiError::invalid_request(request_id, error.body_text()).into_response();
        }
    };
    if let Err(error) = validate_api_query(&query, request_id) {
        return (*error).into_response();
    }

    match adapter.manifest(context).await {
        Ok(manifest) => Json(NodeApiManifestResponse {
            meta: NodeApiResponseMeta::new(request_id),
            manifest,
        })
        .into_response(),
        Err(error) => HttpApiError::from_adapter(request_id, error).into_response(),
    }
}

async fn handle_v1_operation(
    State(adapter): State<NodeHttpAdapter>,
    Extension(context): Extension<NodeCommandContext>,
    Path(id): Path<String>,
    query: Result<Query<NodeApiQuery>, QueryRejection>,
) -> Response {
    let request_id = adapter.allocate_request_id();
    let query = match query {
        Ok(Query(query)) => query,
        Err(error) => {
            return HttpApiError::invalid_request(request_id, error.body_text()).into_response();
        }
    };
    if let Err(error) = validate_api_query(&query, request_id) {
        return (*error).into_response();
    }

    match adapter.operation(&id) {
        Some(operation) if operation_visible_to(&context, &operation.operation) => {
            match operation.project(&context) {
                Ok(operation) => Json(NodeApiOperationResponse {
                    meta: NodeApiResponseMeta::new(request_id),
                    operation,
                })
                .into_response(),
                Err(error) => {
                    HttpApiError::from_adapter(request_id, NodeHttpAdapterError::Projection(error))
                        .into_response()
                }
            }
        }
        Some(_) | None => HttpApiError::not_found(request_id, format!("operation not found: {id}"))
            .into_response(),
    }
}

async fn handle_v1_cancel_operation(
    State(adapter): State<NodeHttpAdapter>,
    Extension(context): Extension<NodeCommandContext>,
    Path(id): Path<String>,
    query: Result<Query<NodeApiQuery>, QueryRejection>,
) -> Response {
    let request_id = adapter.allocate_request_id();
    let query = match query {
        Ok(Query(query)) => query,
        Err(error) => {
            return HttpApiError::invalid_request(request_id, error.body_text()).into_response();
        }
    };
    if let Err(error) = validate_api_query(&query, request_id) {
        return (*error).into_response();
    }

    match adapter.request_operation_cancellation(&context, &id) {
        Ok(operation) => match operation.project(&context) {
            Ok(operation) => (
                StatusCode::ACCEPTED,
                Json(NodeApiOperationResponse {
                    meta: NodeApiResponseMeta::new(request_id),
                    operation,
                }),
            )
                .into_response(),
            Err(error) => {
                HttpApiError::from_adapter(request_id, NodeHttpAdapterError::Projection(error))
                    .into_response()
            }
        },
        Err(OperationCancellationError::NotFound) => {
            HttpApiError::not_found(request_id, format!("operation not found: {id}"))
                .into_response()
        }
        Err(OperationCancellationError::Conflict(problem)) => {
            HttpApiError::from_problem(request_id, StatusCode::CONFLICT, problem).into_response()
        }
        Err(OperationCancellationError::Internal(message)) => {
            HttpApiError::internal(request_id, message).into_response()
        }
    }
}

async fn handle_v1_object(
    State(adapter): State<NodeHttpAdapter>,
    Extension(context): Extension<NodeCommandContext>,
    Path((kind, id)): Path<(String, String)>,
    query: Result<Query<NodeApiQuery>, QueryRejection>,
) -> Response {
    let request_id = adapter.allocate_request_id();
    let query = match query {
        Ok(Query(query)) => query,
        Err(error) => {
            return HttpApiError::invalid_request(request_id, error.body_text()).into_response();
        }
    };
    if let Err(error) = validate_api_query(&query, request_id) {
        return (*error).into_response();
    }

    let snapshot = match adapter.snapshot(context.clone()).await {
        Ok(snapshot) => match project_snapshot_for_control_client(&snapshot, &context) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return HttpApiError::from_adapter(
                    request_id,
                    NodeHttpAdapterError::Projection(error),
                )
                .into_response();
            }
        },
        Err(error) => return HttpApiError::from_adapter(request_id, error).into_response(),
    };
    let object = match kind.as_str() {
        "node" => snapshot
            .identity
            .clone()
            .filter(|identity| identity.node_id == id)
            .map(|identity| NodeApiObject::Node {
                id: id.clone(),
                identity,
            }),
        "network" => snapshot
            .networks
            .into_iter()
            .find(|network| network.name == id)
            .map(|network| NodeApiObject::Network {
                id: id.clone(),
                network,
            }),
        "peer" => snapshot
            .peers
            .into_iter()
            .find(|peer| peer.peer_id == id)
            .map(|peer| NodeApiObject::Peer {
                id: id.clone(),
                peer,
            }),
        "embedded_service" => snapshot
            .embedded_services
            .into_iter()
            .find(|service| service.network_name == id)
            .map(|service| NodeApiObject::EmbeddedService {
                id: id.clone(),
                service: Box::new(service),
            }),
        "network_configuration" => snapshot
            .configuration
            .and_then(|configuration| {
                configuration
                    .networks
                    .into_iter()
                    .find(|network| network.network_name == id)
            })
            .map(|configuration| NodeApiObject::NetworkConfiguration {
                id: id.clone(),
                configuration,
            }),
        _ => None,
    };

    match object {
        Some(object) => Json(NodeApiObjectResponse {
            meta: NodeApiResponseMeta::new(request_id),
            object,
        })
        .into_response(),
        None => HttpApiError::not_found(request_id, format!("object not found: {kind}/{id}"))
            .into_response(),
    }
}

struct NodeApiEventStreamState {
    receiver: broadcast::Receiver<NodeEvent>,
    context: NodeCommandContext,
    initial: Option<NodeApiEvent>,
    replay: std::collections::VecDeque<NodeEvent>,
    request_id: u64,
    events: NodeEventBus,
    cursor: NodeEventCursor,
    last_revision: u64,
    done: bool,
}

async fn handle_v1_events(
    State(adapter): State<NodeHttpAdapter>,
    Extension(context): Extension<NodeCommandContext>,
    headers: HeaderMap,
    query: Result<Query<NodeApiQuery>, QueryRejection>,
) -> Response {
    let request_id = adapter.allocate_request_id();
    let query = match query {
        Ok(Query(query)) => query,
        Err(error) => {
            return HttpApiError::invalid_request(request_id, error.body_text()).into_response();
        }
    };
    if let Err(error) = validate_api_query(&query, request_id) {
        return (*error).into_response();
    }
    let requested = match requested_node_cursor(&headers, &query, request_id) {
        Ok(cursor) => cursor,
        Err(error) => return error.into_response(),
    };

    // Subscribing and reading the boundary happen under the event-bus lock. Any
    // event published after the boundary is queued for this receiver, while
    // retained history before the boundary can be replayed exactly once.
    let (receiver, boundary) = adapter.events.subscribe_with_cursor();
    let requested_cursor = requested.map(|cursor| cursor.bind(boundary));
    let replay = match requested_cursor {
        Some(cursor) => match adapter.events.replay_after(cursor, boundary) {
            NodeEventReplay::Events(events) => events.into(),
            NodeEventReplay::ResyncRequired {
                requested,
                latest,
                reason,
            } => {
                return HttpApiError::resync_required(request_id, requested, latest, reason)
                    .into_response();
            }
        },
        None => std::collections::VecDeque::new(),
    };

    let snapshot = match adapter.snapshot(context.clone()).await {
        Ok(snapshot) => match project_snapshot_for_control_client(&snapshot, &context) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return HttpApiError::from_adapter(
                    request_id,
                    NodeHttpAdapterError::Projection(error),
                )
                .into_response();
            }
        },
        Err(error) => return HttpApiError::from_adapter(request_id, error).into_response(),
    };
    // The snapshot is authoritative through `snapshot.revision`. The cursor's
    // sequence is the last event the client acknowledged, so replay can start
    // strictly after it without regressing the client's dedupe position.
    let resume_sequence = requested_cursor
        .map(|cursor| cursor.sequence)
        .unwrap_or(boundary.sequence);
    let snapshot_cursor = NodeEventCursor::new(resume_sequence, snapshot.revision);
    let initial = NodeApiEvent::Snapshot {
        schema_version: NODE_HTTP_SCHEMA_VERSION,
        request_id,
        server_time: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        cursor: snapshot_cursor,
        snapshot: Box::new(snapshot.clone()),
    };
    let state = NodeApiEventStreamState {
        receiver,
        context,
        initial: Some(initial),
        replay,
        request_id,
        events: adapter.events,
        cursor: snapshot_cursor,
        last_revision: snapshot.revision,
        done: false,
    };
    let stream = futures::stream::unfold(state, move |mut state| async move {
        if state.done {
            return None;
        }
        if let Some(initial) = state.initial.take() {
            return Some((
                Ok::<_, Infallible>(sse_event(initial, Some(snapshot_cursor))),
                state,
            ));
        }

        // A reconnect may race with a snapshot that is newer than the queued
        // events. Discard acknowledged sequences inside the stream state so a
        // duplicate never turns into an empty SSE chunk carrying only a
        // comment.
        let event = loop {
            let event = if let Some(event) = state.replay.pop_front() {
                event
            } else {
                match state.receiver.recv().await {
                    Ok(event) => event,
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        state.done = true;
                        let message = NodeApiEvent::ResyncRequired {
                            schema_version: NODE_HTTP_SCHEMA_VERSION,
                            request_id: state.request_id,
                            server_time: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                            requested_cursor: state.cursor,
                            latest_cursor: state.events.latest_cursor(),
                            reason: NodeEventReplayReason::Lagged,
                        };
                        return Some((Ok(sse_event(message, None)), state));
                    }
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            };
            if event.sequence > state.cursor.sequence {
                break event;
            }
        };

        if let NodeEventKind::StateChanged { revision } = &event.kind {
            if *revision > state.last_revision.saturating_add(1) {
                state.done = true;
                let message = NodeApiEvent::ResyncRequired {
                    schema_version: NODE_HTTP_SCHEMA_VERSION,
                    request_id: state.request_id,
                    server_time: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                    requested_cursor: state.cursor,
                    latest_cursor: state.events.latest_cursor(),
                    reason: NodeEventReplayReason::RevisionGap,
                };
                return Some((Ok(sse_event(message, None)), state));
            }
            state.last_revision = state.last_revision.max(*revision);
        }

        let cursor = NodeEventCursor::new(event.sequence, event.snapshot_revision);
        state.cursor = cursor;
        let event = match project_node_event(&event, &state.context, unix_time_ms()) {
            Ok(event) => event,
            Err(error) => {
                tracing::warn!(diagnostic = %error, "node SSE event projection failed closed");
                return None;
            }
        };
        let message = NodeApiEvent::Event {
            schema_version: NODE_HTTP_SCHEMA_VERSION,
            request_id: state.request_id,
            server_time: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            event,
        };
        Some((Ok(sse_event(message, Some(cursor))), state))
    });
    Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response()
}

fn sse_event(message: NodeApiEvent, id: Option<NodeEventCursor>) -> SseEvent {
    let event_name = match &message {
        NodeApiEvent::Snapshot { .. } => "snapshot",
        NodeApiEvent::Event { .. } => "node",
        NodeApiEvent::ResyncRequired { .. } => "resync_required",
    };
    let payload = serde_json::to_string(&message).expect("node API event is serializable");
    let event = SseEvent::default().event(event_name).data(payload);
    match id {
        Some(id) => event.id(node_cursor_id(id)),
        None => event,
    }
}

async fn handle_command(
    State(adapter): State<NodeHttpAdapter>,
    Extension(context): Extension<NodeCommandContext>,
    payload: Result<Json<NodeCommandHttpRequest>, JsonRejection>,
) -> Response {
    let payload = match payload {
        Ok(Json(payload)) => payload,
        Err(error) => return HttpAdapterError::from_json_rejection(error).into_response(),
    };
    if let Err(error) = require_schema_version_value(payload.schema_version) {
        return error.into_response();
    }

    let _command_permit = match adapter.try_acquire_command_permit() {
        Some(permit) => permit,
        None => {
            return with_retry_after(
                HttpAdapterError::rate_limited(adapter.command_concurrency_limit).into_response(),
            );
        }
    };

    let projection_context = context.clone();
    match adapter.execute(context, payload.request).await {
        Ok(result) => {
            let status = status_for_command_result(&result);
            let result = match project_result_for_control_client(&result, &projection_context) {
                Ok(result) => result,
                Err(error) => {
                    return HttpAdapterError::from(NodeHttpAdapterError::Projection(error))
                        .into_response();
                }
            };
            (
                status,
                Json(NodeCommandHttpResponse {
                    schema_version: NODE_HTTP_SCHEMA_VERSION,
                    result,
                }),
            )
                .into_response()
        }
        Err(error) => HttpAdapterError::from(error).into_response(),
    }
}

async fn handle_snapshot(
    State(adapter): State<NodeHttpAdapter>,
    Extension(context): Extension<NodeCommandContext>,
    Query(query): Query<NodeHttpQuery>,
) -> Result<Json<NodeSnapshotResponse>, HttpAdapterError> {
    require_schema_version(&query)?;
    let snapshot = adapter
        .snapshot(context.clone())
        .await
        .map_err(HttpAdapterError::from)?;
    let snapshot = project_snapshot_for_control_client(&snapshot, &context)
        .map_err(NodeHttpAdapterError::Projection)
        .map_err(HttpAdapterError::from)?;
    Ok(Json(NodeSnapshotResponse {
        schema_version: NODE_HTTP_SCHEMA_VERSION,
        snapshot,
    }))
}

async fn handle_manifest(
    State(adapter): State<NodeHttpAdapter>,
    Extension(context): Extension<NodeCommandContext>,
    Query(query): Query<NodeHttpQuery>,
) -> Result<Json<NodeManifestResponse>, HttpAdapterError> {
    require_schema_version(&query)?;
    let manifest = adapter
        .manifest(context)
        .await
        .map_err(HttpAdapterError::from)?;
    Ok(Json(NodeManifestResponse {
        schema_version: NODE_HTTP_SCHEMA_VERSION,
        manifest,
    }))
}

async fn handle_events(
    State(adapter): State<NodeHttpAdapter>,
    Extension(context): Extension<NodeCommandContext>,
    Query(query): Query<NodeHttpQuery>,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    if let Err(error) = require_schema_version(&query) {
        return error.into_response();
    }
    let Ok(ws) = ws else {
        return HttpAdapterError::upgrade_required().into_response();
    };
    ws.on_upgrade(move |socket| run_event_socket(socket, adapter, context))
}

async fn run_event_socket(
    mut socket: WebSocket,
    adapter: NodeHttpAdapter,
    context: NodeCommandContext,
) {
    // Subscribe before fetching the snapshot so no event is lost in the gap.
    let receiver = adapter.events.subscribe();
    let snapshot = match adapter.snapshot(context.clone()).await {
        Ok(snapshot) => match project_snapshot_for_control_client(&snapshot, &context) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                let message = NodeWsMessage::Error {
                    schema_version: NODE_HTTP_SCHEMA_VERSION,
                    error: NodeErrorDetails::new(
                        NodeHttpErrorCode::ContractViolation,
                        error.to_string(),
                    ),
                };
                let _ = send_ws_message(&mut socket, &message).await;
                let _ = socket.close().await;
                return;
            }
        },
        Err(error) => {
            let message = NodeWsMessage::Error {
                schema_version: NODE_HTTP_SCHEMA_VERSION,
                error: NodeErrorDetails::new(
                    NodeHttpErrorCode::BackendUnavailable,
                    error.to_string(),
                ),
            };
            let _ = send_ws_message(&mut socket, &message).await;
            let _ = socket.close().await;
            return;
        }
    };

    let initial = NodeWsMessage::Snapshot {
        schema_version: NODE_HTTP_SCHEMA_VERSION,
        snapshot: Box::new(snapshot.clone()),
    };
    if !send_ws_message(&mut socket, &initial).await {
        let _ = socket.close().await;
        return;
    }

    let mut session =
        NodeWsSession::new(receiver, adapter.state.clone(), context, snapshot.revision);
    loop {
        tokio::select! {
            incoming = socket.recv() => {
                match incoming {
                    None | Some(Ok(Message::Close(_))) | Some(Err(_)) => break,
                    Some(Ok(_)) => {}
                }
            }
            message = session.next_message() => {
                match message {
                    Ok(Some(message)) => {
                        if !send_ws_message(&mut socket, &message).await {
                            break;
                        }
                    }
                    Ok(None) => {
                        let _ = socket.close().await;
                        break;
                    }
                    Err(error) => {
                        let message = NodeWsMessage::Error {
                            schema_version: NODE_HTTP_SCHEMA_VERSION,
                            error: NodeErrorDetails::new(
                                NodeHttpErrorCode::ContractViolation,
                                error.to_string(),
                            ),
                        };
                        let _ = send_ws_message(&mut socket, &message).await;
                        let _ = socket.close().await;
                        break;
                    }
                }
            }
        }
    }
}

/// Operation records are private to the submitting actor. The local node owner
/// may inspect all local operations; every other principal receives a 404 to
/// avoid leaking whether an operation exists.
fn operation_visible_to(context: &NodeCommandContext, operation: &Operation) -> bool {
    &operation.actor.node_id == context.actor() || context.is_local_owner()
}

/// A successful HTTP/WebSocket response is proof that this adapter can reach
/// the authoritative node state. This transport observation is applied to a
/// clone and then field-projected; the canonical store and its revision are
/// never mutated.
fn project_snapshot_for_control_client(
    snapshot: &NodeSnapshot,
    context: &NodeCommandContext,
) -> Result<ProjectedNodeSnapshot, NodeSnapshotProjectionError> {
    let mut outbound = snapshot.clone();
    outbound.control_health = NodeControlHealth::Available;
    project_node_snapshot(&outbound, context, unix_time_ms())
}

fn project_result_for_control_client(
    result: &NodeCommandResult,
    context: &NodeCommandContext,
) -> Result<ProjectedNodeCommandResult, NodeSnapshotProjectionError> {
    let mut outbound = result.clone();
    match &mut outbound.outcome {
        NodeCommandOutcome::Snapshot(snapshot) => {
            snapshot.control_health = NodeControlHealth::Available;
        }
        NodeCommandOutcome::Rejected(problem) => {
            *problem = sanitize_problem_for_client(problem.clone());
        }
        _ => {}
    }
    project_node_command_result(&outbound, context, unix_time_ms())
}

/// Authentication and authorization failures must be indistinguishable from
/// an inaccessible object, including parameters and network labels. The
/// stable code/key still lets clients choose a recovery action.
fn sanitize_problem_for_client(mut problem: NodeProblem) -> NodeProblem {
    if matches!(
        problem.category,
        NodeErrorCategory::Authentication | NodeErrorCategory::Authorization
    ) {
        problem.message = match problem.category {
            NodeErrorCategory::Authentication => "需要重新进行身份认证",
            NodeErrorCategory::Authorization => "无权执行此操作",
            _ => unreachable!("guarded by authentication/authorization check"),
        }
        .into();
        problem.params = Box::default();
        problem.network_name = None;
        problem.details = None;
    }
    problem
}

async fn send_ws_message(socket: &mut WebSocket, message: &NodeWsMessage) -> bool {
    let Ok(payload) = serde_json::to_string(message) else {
        return false;
    };
    socket.send(Message::Text(payload.into())).await.is_ok()
}

#[derive(Debug)]
struct HttpAdapterError {
    status: StatusCode,
    body: Box<NodeHttpErrorResponse>,
}

impl HttpAdapterError {
    fn schema_version_mismatch(requested: u16, supported: u16) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            body: Box::new(NodeHttpErrorResponse {
                schema_version: NODE_HTTP_SCHEMA_VERSION,
                error: NodeErrorDetails::new(
                    NodeHttpErrorCode::SchemaVersionMismatch,
                    NodeHttpErrorCode::SchemaVersionMismatch.default_message(),
                )
                .with_param("requested", requested.to_string())
                .with_param("supported", supported.to_string()),
                requested_schema_version: Some(requested),
                supported_schema_version: Some(supported),
                problem: None,
            }),
        }
    }

    fn upgrade_required() -> Self {
        Self {
            status: StatusCode::UPGRADE_REQUIRED,
            body: Box::new(NodeHttpErrorResponse {
                schema_version: NODE_HTTP_SCHEMA_VERSION,
                error: NodeErrorDetails::new(
                    NodeHttpErrorCode::InvalidRequest,
                    "事件流端点需要 WebSocket Upgrade",
                ),
                requested_schema_version: None,
                supported_schema_version: Some(NODE_HTTP_SCHEMA_VERSION),
                problem: None,
            }),
        }
    }

    fn from_problem(status: StatusCode, problem: NodeProblem) -> Self {
        let hide_problem = matches!(
            problem.category,
            NodeErrorCategory::Authentication | NodeErrorCategory::Authorization
        );
        Self {
            status,
            body: Box::new(NodeHttpErrorResponse {
                schema_version: NODE_HTTP_SCHEMA_VERSION,
                error: NodeErrorDetails::from_problem(&problem, None),
                requested_schema_version: None,
                supported_schema_version: Some(NODE_HTTP_SCHEMA_VERSION),
                problem: (!hide_problem).then_some(problem),
            }),
        }
    }

    fn from_adapter(error: NodeHttpAdapterError) -> Self {
        match &error {
            NodeHttpAdapterError::SchemaVersionMismatch {
                requested,
                supported,
            } => Self::schema_version_mismatch(*requested, *supported),
            NodeHttpAdapterError::Command(NodeCommandBusError::Invalid(problem)) => {
                Self::from_problem(StatusCode::BAD_REQUEST, problem.clone())
            }
            NodeHttpAdapterError::Rejected(problem) => {
                Self::from_problem(status_for_problem(problem.code), problem.clone())
            }
            NodeHttpAdapterError::Command(error) => Self::from_code(
                StatusCode::SERVICE_UNAVAILABLE,
                NodeHttpErrorCode::DependencyUnavailable,
                error.to_string(),
            ),
            NodeHttpAdapterError::UnexpectedOutcome(detail) => Self::from_code(
                StatusCode::BAD_GATEWAY,
                NodeHttpErrorCode::ContractViolation,
                detail.clone(),
            ),
            NodeHttpAdapterError::ManifestSchemaMismatch { current, supported } => Self::from_code(
                StatusCode::BAD_GATEWAY,
                NodeHttpErrorCode::ContractViolation,
                format!("manifest schema {current} is not supported ({supported})"),
            ),
            NodeHttpAdapterError::InvalidManifest { violations } => Self::from_code(
                StatusCode::BAD_GATEWAY,
                NodeHttpErrorCode::ContractViolation,
                format!("invalid manifest: {violations:?}"),
            ),
            NodeHttpAdapterError::Projection(error) => Self::from_code(
                StatusCode::BAD_GATEWAY,
                NodeHttpErrorCode::ContractViolation,
                error.to_string(),
            ),
        }
    }

    fn rate_limited(concurrency_limit: usize) -> Self {
        let mut response = Self::from_code(
            StatusCode::TOO_MANY_REQUESTS,
            NodeHttpErrorCode::RateLimited,
            "node command concurrency limit reached",
        );
        response.body.error = response
            .body
            .error
            .with_param("concurrency_limit", concurrency_limit.to_string());
        response
    }

    fn from_json_rejection(error: JsonRejection) -> Self {
        if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
            let mut response = Self::from_code(
                StatusCode::PAYLOAD_TOO_LARGE,
                NodeHttpErrorCode::PayloadTooLarge,
                "request body exceeds command limit",
            );
            response.body.error = response
                .body
                .error
                .with_param("limit_bytes", NODE_HTTP_MAX_COMMAND_BODY_BYTES.to_string());
            response
        } else {
            Self::from_code(
                error.status(),
                NodeHttpErrorCode::InvalidRequest,
                error.body_text(),
            )
        }
    }

    fn from_code(
        status: StatusCode,
        code: NodeHttpErrorCode,
        diagnostic: impl Into<String>,
    ) -> Self {
        tracing::warn!(diagnostic = %diagnostic.into(), ?code, "node adapter request failed");
        Self {
            status,
            body: Box::new(NodeHttpErrorResponse {
                schema_version: NODE_HTTP_SCHEMA_VERSION,
                error: NodeErrorDetails::new(code, code.default_message()),
                requested_schema_version: None,
                supported_schema_version: Some(NODE_HTTP_SCHEMA_VERSION),
                problem: None,
            }),
        }
    }
}

impl From<NodeHttpAdapterError> for HttpAdapterError {
    fn from(error: NodeHttpAdapterError) -> Self {
        Self::from_adapter(error)
    }
}

impl IntoResponse for HttpAdapterError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

#[derive(Debug)]
struct HttpApiError {
    status: StatusCode,
    body: NodeApiErrorResponse,
}

impl HttpApiError {
    fn invalid_request(request_id: u64, diagnostic: impl Into<String>) -> Self {
        tracing::debug!(diagnostic = %diagnostic.into(), "invalid node API request");
        Self::new(
            request_id,
            StatusCode::BAD_REQUEST,
            NodeHttpErrorCode::InvalidRequest,
        )
    }

    fn rate_limited(request_id: u64, concurrency_limit: usize) -> Self {
        let mut error = Self::new(
            request_id,
            StatusCode::TOO_MANY_REQUESTS,
            NodeHttpErrorCode::RateLimited,
        );
        error.body.error = error
            .body
            .error
            .with_param("concurrency_limit", concurrency_limit.to_string());
        error
    }

    fn payload_too_large(request_id: u64) -> Self {
        let mut error = Self::new(
            request_id,
            StatusCode::PAYLOAD_TOO_LARGE,
            NodeHttpErrorCode::PayloadTooLarge,
        );
        error.body.error = error
            .body
            .error
            .with_param("limit_bytes", NODE_HTTP_MAX_COMMAND_BODY_BYTES.to_string());
        error
    }

    fn not_found(request_id: u64, diagnostic: impl Into<String>) -> Self {
        tracing::debug!(diagnostic = %diagnostic.into(), "node API object not found");
        Self::new(
            request_id,
            StatusCode::NOT_FOUND,
            NodeHttpErrorCode::NotFound,
        )
    }

    fn operation_conflict(request_id: u64, diagnostic: impl Into<String>) -> Self {
        tracing::debug!(diagnostic = %diagnostic.into(), "node API operation conflict");
        Self::new(
            request_id,
            StatusCode::CONFLICT,
            NodeHttpErrorCode::OperationConflict,
        )
    }

    fn internal(request_id: u64, diagnostic: impl Into<String>) -> Self {
        tracing::error!(diagnostic = %diagnostic.into(), "node API internal error");
        Self::new(
            request_id,
            StatusCode::INTERNAL_SERVER_ERROR,
            NodeHttpErrorCode::Internal,
        )
    }

    fn resync_required(
        request_id: u64,
        requested: NodeEventCursor,
        latest: NodeEventCursor,
        reason: NodeEventReplayReason,
    ) -> Self {
        let mut error = Self::new(
            request_id,
            StatusCode::CONFLICT,
            NodeHttpErrorCode::ResyncRequired,
        );
        error.body.error = error
            .body
            .error
            .with_param("requested_sequence", requested.sequence.to_string())
            .with_param(
                "requested_snapshot_revision",
                requested.snapshot_revision.to_string(),
            )
            .with_param("latest_sequence", latest.sequence.to_string())
            .with_param(
                "latest_snapshot_revision",
                latest.snapshot_revision.to_string(),
            )
            .with_param("reason", format!("{reason:?}"));
        error.body.requested_schema_version = None;
        error.body.supported_schema_version = Some(NODE_HTTP_SCHEMA_VERSION);
        error.body.requested_cursor = Some(requested);
        error.body.latest_cursor = Some(latest);
        error.body.resync_reason = Some(reason);
        error
    }

    fn schema_version_mismatch(request_id: u64, requested: u16, supported: u16) -> Self {
        let mut error = Self::new(
            request_id,
            StatusCode::BAD_REQUEST,
            NodeHttpErrorCode::SchemaVersionMismatch,
        );
        error.body.error = error
            .body
            .error
            .with_param("requested", requested.to_string())
            .with_param("supported", supported.to_string());
        error.body.requested_schema_version = Some(requested);
        error.body.supported_schema_version = Some(supported);
        error
    }

    fn from_adapter(request_id: u64, error: NodeHttpAdapterError) -> Self {
        match &error {
            NodeHttpAdapterError::SchemaVersionMismatch {
                requested,
                supported,
            } => Self::schema_version_mismatch(request_id, *requested, *supported),
            NodeHttpAdapterError::Command(NodeCommandBusError::Invalid(problem)) => {
                Self::from_problem(request_id, StatusCode::BAD_REQUEST, problem.clone())
            }
            NodeHttpAdapterError::Rejected(problem) => Self::from_problem(
                request_id,
                status_for_problem(problem.code),
                problem.clone(),
            ),
            NodeHttpAdapterError::Command(error) => 'error: {
                tracing::warn!(diagnostic = %error, "node command backend unavailable");
                let mut response = Self::new(
                    request_id,
                    StatusCode::SERVICE_UNAVAILABLE,
                    NodeHttpErrorCode::DependencyUnavailable,
                );
                response.body.supported_schema_version = Some(NODE_HTTP_SCHEMA_VERSION);
                break 'error response;
            }
            NodeHttpAdapterError::UnexpectedOutcome(detail) => {
                tracing::warn!(diagnostic = %detail, "node adapter contract violation");
                let mut response = Self::new(
                    request_id,
                    StatusCode::BAD_GATEWAY,
                    NodeHttpErrorCode::ContractViolation,
                );
                response.body.supported_schema_version = Some(NODE_HTTP_SCHEMA_VERSION);
                response
            }
            NodeHttpAdapterError::ManifestSchemaMismatch { current, supported } => {
                tracing::warn!(current, supported, "node UI manifest schema mismatch");
                let mut response = Self::new(
                    request_id,
                    StatusCode::BAD_GATEWAY,
                    NodeHttpErrorCode::ContractViolation,
                );
                response.body.supported_schema_version = Some(NODE_HTTP_SCHEMA_VERSION);
                response
            }
            NodeHttpAdapterError::InvalidManifest { violations } => {
                tracing::warn!(?violations, "invalid node UI manifest");
                let mut response = Self::new(
                    request_id,
                    StatusCode::BAD_GATEWAY,
                    NodeHttpErrorCode::ContractViolation,
                );
                response.body.supported_schema_version = Some(NODE_HTTP_SCHEMA_VERSION);
                response
            }
            NodeHttpAdapterError::Projection(error) => {
                tracing::warn!(diagnostic = %error, "node field projection failed closed");
                let mut response = Self::new(
                    request_id,
                    StatusCode::BAD_GATEWAY,
                    NodeHttpErrorCode::ContractViolation,
                );
                response.body.supported_schema_version = Some(NODE_HTTP_SCHEMA_VERSION);
                response
            }
        }
    }

    fn from_problem(request_id: u64, status: StatusCode, problem: NodeProblem) -> Self {
        let hide_problem = matches!(
            problem.category,
            NodeErrorCategory::Authentication | NodeErrorCategory::Authorization
        );
        let mut response = Self::new(request_id, status, code_for_problem(problem.code));
        response.body.error = NodeErrorDetails::from_problem(&problem, None);
        response.body.supported_schema_version = Some(NODE_HTTP_SCHEMA_VERSION);
        response.body.problem = (!hide_problem).then_some(problem);
        response
    }

    fn new(request_id: u64, status: StatusCode, code: NodeHttpErrorCode) -> Self {
        Self {
            status,
            body: NodeApiErrorResponse {
                meta: NodeApiResponseMeta::new(request_id),
                error: NodeErrorDetails::new(code, code.default_message()),
                requested_schema_version: None,
                supported_schema_version: None,
                problem: None,
                requested_cursor: None,
                latest_cursor: None,
                resync_reason: None,
            },
        }
    }
}

impl IntoResponse for HttpApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use axum::{
        body::{to_bytes, Body},
        http::{header, Method, Request, StatusCode},
    };
    use fuckipv6_common::domain::{
        ActivityEntry, AuditId, AuditOutcome, FieldPrivacyPolicy, FieldVisibility,
        FieldVisibilityRule, IdempotencyKey, MembershipRole, MembershipState, NetworkId, NodeId,
        ObjectKind, OperationState, PrivacyField,
    };
    use futures::{SinkExt, StreamExt};
    use tokio::{net::TcpListener, sync::broadcast, time::timeout};
    use tokio_tungstenite::{connect_async, tungstenite::Message as ClientMessage};
    use tower::ServiceExt;

    use super::*;
    use crate::node::state::NodeAccessControlSnapshot;
    use crate::node::{
        node_ui_manifest, NetworkCommandScope, NodeCommandFuture, NodeCommandHandler,
        NodeCommandResult, NodeIdentityRotationSnapshot, NodeIdentitySnapshot,
        NodeJoinNetworkCommand, NodeLogEntry, NodeLogLevel, NodeNetworkMembershipSnapshot,
        NodeNetworkSnapshot, NodeNetworkStatus, NodePeerSnapshot, NodeProblemCode,
        NodeTrafficSnapshot, ProjectedNodeCommandOutcome, ProjectedNodeEventKind,
    };

    #[derive(Clone)]
    enum StubWriteBehavior {
        Completed,
        IdentityRotated(NodeIdentityRotationSnapshot),
        Rejected(NodeProblem),
    }

    #[derive(Clone)]
    struct StubHandler {
        state: NodeStateStore,
        manifest: NodeUiManifest,
        write_behavior: StubWriteBehavior,
        write_delay: Duration,
        write_count: Arc<AtomicUsize>,
    }

    impl NodeCommandHandler for StubHandler {
        fn handle(&self, request: NodeCommandRequest) -> NodeCommandFuture {
            self.run(request, None)
        }

        fn handle_authorized_cancellable(
            &self,
            _context: NodeCommandContext,
            request: NodeCommandRequest,
            cancel: watch::Receiver<bool>,
        ) -> NodeCommandFuture {
            self.run(request, Some(cancel))
        }
    }

    impl StubHandler {
        fn run(
            &self,
            request: NodeCommandRequest,
            mut cancel: Option<watch::Receiver<bool>>,
        ) -> NodeCommandFuture {
            let state = self.state.clone();
            let manifest = self.manifest.clone();
            let write_behavior = self.write_behavior.clone();
            let write_delay = self.write_delay;
            let write_count = self.write_count.clone();
            Box::pin(async move {
                let request_id = request.request_id;
                match request.command {
                    NodeCommand::GetSnapshot => {
                        NodeCommandResult::snapshot(request_id, state.snapshot().await)
                    }
                    NodeCommand::GetUiManifest => {
                        NodeCommandResult::ui_manifest(request_id, manifest)
                    }
                    _ => {
                        write_count.fetch_add(1, Ordering::SeqCst);
                        if cancel.as_ref().is_some_and(|cancel| *cancel.borrow()) {
                            tokio::time::sleep(write_delay).await;
                            return NodeCommandResult::rejected(
                                request_id,
                                NodeProblem::new(
                                    NodeProblemCode::Cancelled,
                                    "stub operation cancelled",
                                ),
                            );
                        }
                        if let Some(cancel) = cancel.as_mut() {
                            if !write_delay.is_zero() {
                                tokio::select! {
                                    _ = tokio::time::sleep(write_delay) => {}
                                    _ = cancel.changed() => {
                                        if *cancel.borrow() {
                                            // Model actor-owned cleanup before the
                                            // terminal cancellation is reported.
                                            tokio::time::sleep(write_delay).await;
                                            return NodeCommandResult::rejected(
                                                request_id,
                                                NodeProblem::new(
                                                    NodeProblemCode::Cancelled,
                                                    "stub operation cancelled",
                                                ),
                                            );
                                        }
                                    }
                                }
                            }
                        } else if !write_delay.is_zero() {
                            tokio::time::sleep(write_delay).await;
                        }
                        match write_behavior {
                            StubWriteBehavior::Completed => NodeCommandResult::completed(
                                request_id,
                                NodeCommandOutcome::Completed {
                                    message: "stub write completed".into(),
                                },
                            ),
                            StubWriteBehavior::IdentityRotated(rotation) => {
                                NodeCommandResult::completed(
                                    request_id,
                                    NodeCommandOutcome::IdentityRotated(rotation),
                                )
                            }
                            StubWriteBehavior::Rejected(problem) => {
                                NodeCommandResult::rejected(request_id, problem)
                            }
                        }
                    }
                }
            })
        }
    }

    fn adapter_fixture(
        manifest: NodeUiManifest,
    ) -> (NodeHttpAdapter, NodeStateStore, NodeEventBus) {
        let (adapter, state, events, _writes) = adapter_fixture_with_write(
            manifest,
            StubWriteBehavior::Rejected(NodeProblem::new(
                NodeProblemCode::Unavailable,
                "stub only implements read commands",
            )),
            Duration::ZERO,
        );
        (adapter, state, events)
    }

    fn adapter_fixture_with_write(
        manifest: NodeUiManifest,
        write_behavior: StubWriteBehavior,
        write_delay: Duration,
    ) -> (
        NodeHttpAdapter,
        NodeStateStore,
        NodeEventBus,
        Arc<AtomicUsize>,
    ) {
        let events = NodeEventBus::new(32);
        let state = NodeStateStore::with_events(events.clone());
        let write_count = Arc::new(AtomicUsize::new(0));
        let commands = NodeCommandBus::spawn(
            Arc::new(StubHandler {
                state: state.clone(),
                manifest,
                write_behavior,
                write_delay,
                write_count: write_count.clone(),
            }),
            events.clone(),
            8,
        );
        (
            NodeHttpAdapter::new(commands, state.clone(), events.clone()),
            state,
            events,
            write_count,
        )
    }

    const HIDDEN_DISPLAY_NAME: &str = "privacy-hidden-display-name";
    const HIDDEN_FINGERPRINT: &str = "privacy-hidden-fingerprint";
    const HIDDEN_TUN_NAME: &str = "privacy-hidden-tun0";
    const HIDDEN_IPV6: &str = "2001:db8::dead:beef";
    const HIDDEN_ENDPOINT: &str = "203.0.113.44:8443";
    const HIDDEN_LOG_TARGET: &str = "privacy-hidden-log-target";
    const HIDDEN_LOG_MESSAGE: &str = "privacy-hidden-log-message";
    const HIDDEN_ACTIVITY_SUMMARY: &str = "privacy-hidden-activity-summary";

    fn privacy_rule(field: PrivacyField) -> FieldVisibilityRule {
        FieldVisibilityRule {
            field,
            visibility: FieldVisibility::OwnerOnly,
        }
    }

    fn privacy_transport_context(snapshot: &NodeSnapshot) -> NodeCommandContext {
        let membership = snapshot
            .memberships
            .first()
            .expect("privacy fixture membership");
        let network_id =
            NetworkId::parse(membership.network_id.clone()).expect("privacy fixture network id");
        NodeCommandContext::network_session(
            NodeId::from_public_key(b"privacy-transport-viewer"),
            "privacy-transport-test",
            [crate::node::NodePermission::ViewState],
            [(
                membership.network_name.clone(),
                NetworkCommandScope::new(
                    network_id,
                    fuckipv6_common::domain::NetworkRole::Administrator,
                    membership.state,
                    Vec::new(),
                ),
            )],
            true,
        )
        .with_access_control_snapshot(&snapshot.access_control)
    }

    fn privacy_transport_fixture() -> (NodeSnapshot, NodeCommandContext) {
        let network_id = NetworkId::from_public_key(b"privacy-transport-home");
        let node_id = NodeId::from_public_key(b"privacy-transport-node");
        let peer_id = NodeId::from_public_key(b"privacy-transport-peer");
        let policy = FieldPrivacyPolicy {
            network_id: Some(network_id.clone()),
            revision: 9,
            issued_at_unix_ms: 0,
            effective_at_unix_ms: 0,
            expires_at_unix_ms: u64::MAX,
            rules: [
                PrivacyField::NodeDisplayName,
                PrivacyField::NodePublicKey,
                PrivacyField::NodeOnlineState,
                PrivacyField::NodeEndpoints,
                PrivacyField::NodeRealNetworkAddresses,
                PrivacyField::NodeTrafficStatistics,
                PrivacyField::NodeLocalPaths,
                PrivacyField::NodeDetailedLogs,
                PrivacyField::ActivityActor,
            ]
            .into_iter()
            .map(privacy_rule)
            .collect(),
        };
        let snapshot = NodeSnapshot {
            revision: 41,
            control_health: NodeControlHealth::Available,
            identity: Some(NodeIdentitySnapshot {
                node_id: node_id.to_string(),
                display_name: Some(HIDDEN_DISPLAY_NAME.into()),
                public_key_fingerprint: HIDDEN_FINGERPRINT.into(),
                created_at_unix_ms: Some(10),
            }),
            access_control: NodeAccessControlSnapshot {
                local_policy: None,
                network_acls: Vec::new(),
                field_privacy_policies: vec![policy],
            },
            memberships: vec![NodeNetworkMembershipSnapshot {
                network_name: "home".into(),
                network_id: network_id.to_string(),
                node_id: node_id.to_string(),
                state: MembershipState::Active,
                role: MembershipRole::Administrator,
                local_policy_revision: 0,
                network_policy_revision: None,
                privacy_policy_revision: Some(9),
                member_limit: None,
                exit_contribution: None,
                exit_contribution_scope: None,
                exit_contribution_mode: None,
                terms_hash: None,
                current_terms: None,
                current_terms_hash: None,
                policy_confirmation_required: false,
                acknowledged_at_unix_ms: None,
                created_at_unix_ms: 1,
                updated_at_unix_ms: 1,
                suspension_reason: None,
            }],
            networks: vec![NodeNetworkSnapshot {
                name: "home".into(),
                tun_name: HIDDEN_TUN_NAME.into(),
                status: NodeNetworkStatus::Online,
                assigned_ipv6: Some(HIDDEN_IPV6.parse().unwrap()),
                assigned_ipv4: Some("192.0.2.44".parse().unwrap()),
                p2p_listen_addr: Some(HIDDEN_ENDPOINT.parse().unwrap()),
                uptime_secs: 42,
                roles: Vec::new(),
                peer_count: 1,
                traffic: NodeTrafficSnapshot {
                    tun_rx_bytes: 44,
                    ..NodeTrafficSnapshot::default()
                },
            }],
            peers: vec![NodePeerSnapshot {
                peer_id: peer_id.to_string(),
                network_name: "home".into(),
                virtual_ipv6: HIDDEN_IPV6.parse().unwrap(),
                virtual_ipv4: Some("192.0.2.45".parse().unwrap()),
                endpoints: vec![HIDDEN_ENDPOINT.parse().unwrap()],
                online: true,
                p2p_active: true,
                p2p_healthy: true,
                rtt_us: Some(9_000),
                loss_rate: 0.5,
                traffic: NodeTrafficSnapshot {
                    p2p_rx_bytes: 45,
                    ..NodeTrafficSnapshot::default()
                },
            }],
            recent_activity: vec![ActivityEntry {
                audit_id: AuditId::from_public_key(b"privacy-transport-audit"),
                operation_id: None,
                actor: node_id,
                network_id: Some(network_id),
                action: "privacy.hidden.action".into(),
                resource: "member:privacy-hidden".into(),
                outcome: AuditOutcome::Succeeded,
                occurred_at_unix_ms: 1_500,
                summary: HIDDEN_ACTIVITY_SUMMARY.into(),
                category: fuckipv6_common::domain::ActivityCategory::UserActivity,
                severity: fuckipv6_common::domain::ActivitySeverity::Info,
                actor_role: Some("owner".into()),
                object_kind: "member".into(),
                object_id: Some("privacy-hidden".into()),
                reason: Some("succeeded".into()),
                details: BTreeMap::from([("secret".into(), "hidden".into())]),
            }],
            logs: vec![NodeLogEntry {
                unix_ms: 1_500,
                level: NodeLogLevel::Info,
                target: HIDDEN_LOG_TARGET.into(),
                message: HIDDEN_LOG_MESSAGE.into(),
            }],
            ..NodeSnapshot::default()
        };
        let context = privacy_transport_context(&snapshot);
        (snapshot, context)
    }

    fn assert_privacy_serialization_omits_hidden_values(label: &str, value: &str) {
        for secret in [
            HIDDEN_DISPLAY_NAME,
            HIDDEN_FINGERPRINT,
            HIDDEN_TUN_NAME,
            HIDDEN_IPV6,
            HIDDEN_ENDPOINT,
            HIDDEN_LOG_TARGET,
            HIDDEN_LOG_MESSAGE,
            HIDDEN_ACTIVITY_SUMMARY,
        ] {
            assert!(
                !value.contains(secret),
                "{label} leaked hidden value {secret}: {value}"
            );
        }
    }

    async fn json_body<T: serde::de::DeserializeOwned>(response: Response) -> T {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read response body");
        serde_json::from_slice(&bytes).expect("decode response body")
    }

    async fn next_sse_event<S>(body: &mut S) -> (Option<String>, NodeApiEvent)
    where
        S: futures::Stream<Item = Result<bytes::Bytes, axum::Error>> + Unpin,
    {
        let chunk = timeout(Duration::from_secs(2), body.next())
            .await
            .expect("SSE event timeout")
            .expect("SSE event stream ended")
            .expect("valid SSE chunk");
        let text = String::from_utf8(chunk.to_vec()).expect("SSE chunk is UTF-8");
        let id = text
            .lines()
            .find_map(|line| line.strip_prefix("id:"))
            .map(str::trim)
            .map(ToOwned::to_owned);
        let data = text
            .lines()
            .find_map(|line| line.strip_prefix("data:"))
            .map(str::trim)
            .expect("SSE data line");
        let event = serde_json::from_str(data).expect("decode SSE event");
        (id, event)
    }

    async fn post_command(adapter: NodeHttpAdapter, payload: &NodeCommandHttpRequest) -> Response {
        let body = serde_json::to_vec(payload).expect("serialize command request");
        adapter
            .test_router()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/node/commands")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .expect("command request"),
            )
            .await
            .expect("command response")
    }

    async fn post_v1_command(
        adapter: NodeHttpAdapter,
        payload: &NodeCommandHttpRequest,
    ) -> Response {
        let body = serde_json::to_vec(payload).expect("serialize command request");
        adapter
            .test_router()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/commands")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .expect("v1 command request"),
            )
            .await
            .expect("v1 command response")
    }

    async fn post_raw_command(adapter: NodeHttpAdapter, path: &str, body: Vec<u8>) -> Response {
        adapter
            .test_router()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(path)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .expect("raw command request"),
            )
            .await
            .expect("raw command response")
    }

    async fn fetch_v1_operation(adapter: &NodeHttpAdapter, operation_id: &str) -> Response {
        adapter
            .clone()
            .test_router()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/operations/{operation_id}?schema_version=1"
                    ))
                    .body(Body::empty())
                    .expect("operation lookup request"),
            )
            .await
            .expect("operation lookup response")
    }

    async fn fetch_v1_operation_as(
        adapter: &NodeHttpAdapter,
        operation_id: &str,
        context: NodeCommandContext,
    ) -> Response {
        adapter
            .clone()
            .router()
            .layer(Extension(context))
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/operations/{operation_id}?schema_version=1"
                    ))
                    .body(Body::empty())
                    .expect("operation lookup request"),
            )
            .await
            .expect("operation lookup response")
    }

    async fn post_v1_cancel(adapter: &NodeHttpAdapter, operation_id: &str) -> Response {
        adapter
            .clone()
            .test_router()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/v1/operations/{operation_id}/cancel?schema_version=1"
                    ))
                    .body(Body::empty())
                    .expect("operation cancellation request"),
            )
            .await
            .expect("operation cancellation response")
    }

    async fn post_v1_cancel_as(
        adapter: &NodeHttpAdapter,
        operation_id: &str,
        context: NodeCommandContext,
    ) -> Response {
        adapter
            .clone()
            .router()
            .layer(Extension(context))
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/v1/operations/{operation_id}/cancel?schema_version=1"
                    ))
                    .body(Body::empty())
                    .expect("operation cancellation request"),
            )
            .await
            .expect("operation cancellation response")
    }

    async fn wait_for_terminal_operation(
        adapter: &NodeHttpAdapter,
        operation_id: &str,
    ) -> NodeApiOperationResponse {
        for _ in 0..200 {
            let response = fetch_v1_operation(adapter, operation_id).await;
            assert_eq!(response.status(), StatusCode::OK);
            let body: NodeApiOperationResponse = json_body(response).await;
            if body.operation.operation.state.is_terminal() {
                return body;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("operation {operation_id} did not reach a terminal state");
    }

    #[test]
    fn transport_dtos_serialize_only_projected_snapshot_and_result_fields() {
        let (snapshot, context) = privacy_transport_fixture();
        let projected =
            project_snapshot_for_control_client(&snapshot, &context).expect("project snapshot");

        assert!(projected.identity.is_none());
        assert!(projected.logs.is_empty());
        assert!(projected.recent_activity.is_empty());
        let network = projected.networks.first().expect("visible network row");
        assert!(network.tun_name.is_none());
        assert!(network.assigned_ipv6.is_none());
        assert!(network.p2p_listen_addr.is_none());
        let peer = projected.peers.first().expect("visible peer row");
        assert!(peer.virtual_ipv6.is_none());
        assert!(peer.endpoints.is_none());
        assert!(!peer.online.unwrap_or_default());

        let snapshot_json = serde_json::to_string(&NodeApiSnapshotResponse {
            meta: NodeApiResponseMeta::new(1),
            snapshot: projected.clone(),
        })
        .expect("serialize snapshot response");
        assert_privacy_serialization_omits_hidden_values("snapshot response", &snapshot_json);

        let result = NodeCommandResult::snapshot(17, snapshot.clone());
        let projected_result =
            project_result_for_control_client(&result, &context).expect("project command result");
        let command_json = serde_json::to_string(&NodeCommandHttpResponse {
            schema_version: NODE_HTTP_SCHEMA_VERSION,
            result: projected_result,
        })
        .expect("serialize command response");
        assert_privacy_serialization_omits_hidden_values("command response", &command_json);

        let operation = Operation {
            operation_id: OperationId::from_public_key(b"privacy-transport-operation"),
            kind: OperationKind::Diagnostics,
            actor: DomainActorRef {
                node_id: context.actor().clone(),
                display: "privacy-transport-test".into(),
            },
            target: DomainObjectRef {
                kind: ObjectKind::Node,
                id: context.actor().to_string(),
            },
            idempotency_key: IdempotencyKey::from_public_key(b"privacy-transport-idempotency"),
            state: OperationState::Succeeded,
            stage: "succeeded".into(),
            created_at_unix_ms: 1_500,
            updated_at_unix_ms: 1_500,
            attempt: 1,
            max_attempts: 3,
            error: None,
            cancellable: false,
            cancel_blocked_reason: None,
        };
        let projected_operation = OperationView {
            operation,
            result: Some(result),
        }
        .project(&context)
        .expect("project operation response");
        assert!(projected_operation.result.is_some());
        let operation_json = serde_json::to_string(&NodeApiOperationResponse {
            meta: NodeApiResponseMeta::new(2),
            operation: projected_operation,
        })
        .expect("serialize operation response");
        assert_privacy_serialization_omits_hidden_values("operation response", &operation_json);

        let object_json = serde_json::to_string(&NodeApiObjectResponse {
            meta: NodeApiResponseMeta::new(3),
            object: NodeApiObject::Network {
                id: network.name.clone(),
                network: network.clone(),
            },
        })
        .expect("serialize object response");
        assert_privacy_serialization_omits_hidden_values("network object response", &object_json);

        let object_json = serde_json::to_string(&NodeApiObjectResponse {
            meta: NodeApiResponseMeta::new(4),
            object: NodeApiObject::Peer {
                id: peer.peer_id.clone(),
                peer: peer.clone(),
            },
        })
        .expect("serialize peer object response");
        assert_privacy_serialization_omits_hidden_values("peer object response", &object_json);
    }

    #[tokio::test]
    async fn snapshot_and_manifest_success_are_versioned_and_forwarded() {
        let (adapter, state, _events) = adapter_fixture(node_ui_manifest());
        state.update(|snapshot| snapshot.revision = 6).await;

        let snapshot_response = adapter
            .clone()
            .test_router()
            .oneshot(
                Request::builder()
                    .uri("/api/node/snapshot?schema_version=1")
                    .body(Body::empty())
                    .expect("snapshot request"),
            )
            .await
            .expect("snapshot response");
        assert_eq!(snapshot_response.status(), StatusCode::OK);
        let snapshot: NodeSnapshotResponse = json_body(snapshot_response).await;
        assert_eq!(snapshot.schema_version, NODE_HTTP_SCHEMA_VERSION);
        assert_eq!(snapshot.snapshot.revision, 7);
        assert_eq!(
            snapshot.snapshot.control_health,
            NodeControlHealth::Available
        );
        assert_eq!(
            state.snapshot().await.control_health,
            NodeControlHealth::Unknown
        );

        let manifest_response = adapter
            .test_router()
            .oneshot(
                Request::builder()
                    .uri("/api/node/manifest?schema_version=1")
                    .body(Body::empty())
                    .expect("manifest request"),
            )
            .await
            .expect("manifest response");
        assert_eq!(manifest_response.status(), StatusCode::OK);
        let manifest: NodeManifestResponse = json_body(manifest_response).await;
        assert_eq!(manifest.schema_version, NODE_HTTP_SCHEMA_VERSION);
        assert_eq!(
            manifest.manifest.schema_version,
            NODE_UI_MANIFEST_SCHEMA_VERSION
        );
        assert_eq!(manifest.manifest.pages.len(), 13);
    }

    #[tokio::test]
    async fn v1_snapshot_and_manifest_carry_meta_and_authoritative_payload() {
        let (adapter, state, _events) = adapter_fixture(node_ui_manifest());
        state.update(|snapshot| snapshot.revision = 8).await;

        let snapshot_response = adapter
            .clone()
            .test_router()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/snapshot?schema_version=1")
                    .body(Body::empty())
                    .expect("v1 snapshot request"),
            )
            .await
            .expect("v1 snapshot response");
        assert_eq!(snapshot_response.status(), StatusCode::OK);
        let snapshot: NodeApiSnapshotResponse = json_body(snapshot_response).await;
        assert_eq!(snapshot.meta.schema_version, NODE_HTTP_SCHEMA_VERSION);
        assert!(snapshot.meta.request_id > 0);
        chrono::DateTime::parse_from_rfc3339(&snapshot.meta.server_time)
            .expect("snapshot server time is RFC3339");
        assert_eq!(snapshot.snapshot.revision, 9);
        assert_eq!(
            snapshot.snapshot.control_health,
            NodeControlHealth::Available
        );

        let manifest_response = adapter
            .test_router()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/manifest?schema_version=1")
                    .body(Body::empty())
                    .expect("v1 manifest request"),
            )
            .await
            .expect("v1 manifest response");
        assert_eq!(manifest_response.status(), StatusCode::OK);
        let manifest: NodeApiManifestResponse = json_body(manifest_response).await;
        assert_eq!(manifest.meta.schema_version, NODE_HTTP_SCHEMA_VERSION);
        assert!(manifest.meta.request_id > snapshot.meta.request_id);
        assert_eq!(manifest.manifest.pages.len(), 13);
    }

    #[tokio::test]
    async fn command_endpoints_reject_oversized_json_before_deserialization() {
        let (adapter, _state, _events) = adapter_fixture(node_ui_manifest());
        let oversized = vec![b' '; NODE_HTTP_MAX_COMMAND_BODY_BYTES + 1];

        for path in ["/api/node/commands", "/api/v1/commands"] {
            let response = post_raw_command(adapter.clone(), path, oversized.clone()).await;
            assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE, "{path}");
            let body: serde_json::Value = json_body(response).await;
            assert_eq!(body["schema_version"], NODE_HTTP_SCHEMA_VERSION, "{path}");
            assert_eq!(body["code"], "payload_too_large", "{path}");
            assert_eq!(body["message_key"], "error.payload_too_large", "{path}");
            assert_eq!(
                body["params"]["limit_bytes"],
                NODE_HTTP_MAX_COMMAND_BODY_BYTES.to_string(),
                "{path}"
            );
            assert_eq!(body["retryable"], false, "{path}");
        }
    }

    #[tokio::test]
    async fn command_endpoints_fail_fast_when_concurrency_limit_is_exhausted() {
        let (adapter, _state, _events, writes) = adapter_fixture_with_write(
            node_ui_manifest(),
            StubWriteBehavior::Completed,
            Duration::ZERO,
        );
        let adapter = adapter.with_command_concurrency_limit(2);
        let permits = (0..2)
            .map(|_| {
                adapter
                    .command_slots
                    .clone()
                    .try_acquire_owned()
                    .expect("hold concurrency permit")
            })
            .collect::<Vec<_>>();
        let requests = [
            (
                "/api/node/commands",
                NodeCommandHttpRequest {
                    schema_version: NODE_HTTP_SCHEMA_VERSION,
                    request: NodeCommandRequest::new(71, NodeCommand::StartNode),
                    idempotency_key: None,
                },
            ),
            (
                "/api/v1/commands",
                NodeCommandHttpRequest {
                    schema_version: NODE_HTTP_SCHEMA_VERSION,
                    request: NodeCommandRequest::new(72, NodeCommand::StartNode),
                    idempotency_key: Some(IdempotencyKey::from_public_key(b"concurrency-limit-v1")),
                },
            ),
        ];

        for (path, payload) in requests {
            let body = serde_json::to_vec(&payload).expect("serialize command request");
            let response = post_raw_command(adapter.clone(), path, body).await;
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS, "{path}");
            assert_eq!(
                response
                    .headers()
                    .get(header::RETRY_AFTER)
                    .and_then(|value| value.to_str().ok()),
                Some("1"),
                "{path}"
            );
            let body: serde_json::Value = json_body(response).await;
            assert_eq!(body["code"], "rate_limited", "{path}");
            assert_eq!(body["message_key"], "error.rate_limited", "{path}");
            assert_eq!(body["category"], "dependency", "{path}");
            assert_eq!(body["retryable"], true, "{path}");
            assert_eq!(body["params"]["concurrency_limit"], "2", "{path}");
            assert_eq!(body["recovery"]["kind"], "retry", "{path}");
        }
        assert_eq!(
            writes.load(Ordering::SeqCst),
            0,
            "rate-limited commands must not reach the command bus"
        );

        drop(permits);
        let response = post_v1_command(
            adapter,
            &NodeCommandHttpRequest {
                schema_version: NODE_HTTP_SCHEMA_VERSION,
                request: NodeCommandRequest::new(73, NodeCommand::GetSnapshot),
                idempotency_key: None,
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn command_concurrency_permit_is_released_after_success_and_rejection() {
        let (adapter, _state, _events, _writes) = adapter_fixture_with_write(
            node_ui_manifest(),
            StubWriteBehavior::Completed,
            Duration::ZERO,
        );
        let adapter = adapter.with_command_concurrency_limit(1);

        let success = post_v1_command(
            adapter.clone(),
            &NodeCommandHttpRequest {
                schema_version: NODE_HTTP_SCHEMA_VERSION,
                request: NodeCommandRequest::new(74, NodeCommand::GetSnapshot),
                idempotency_key: None,
            },
        )
        .await;
        assert_eq!(success.status(), StatusCode::OK);
        assert_eq!(adapter.command_slots.available_permits(), 1);

        let rejection = post_v1_command(
            adapter.clone(),
            &NodeCommandHttpRequest {
                schema_version: NODE_HTTP_SCHEMA_VERSION,
                request: NodeCommandRequest::new(75, NodeCommand::StartNode),
                idempotency_key: None,
            },
        )
        .await;
        assert_eq!(rejection.status(), StatusCode::BAD_REQUEST);
        assert_eq!(adapter.command_slots.available_permits(), 1);

        let final_success = post_v1_command(
            adapter.clone(),
            &NodeCommandHttpRequest {
                schema_version: NODE_HTTP_SCHEMA_VERSION,
                request: NodeCommandRequest::new(76, NodeCommand::GetSnapshot),
                idempotency_key: None,
            },
        )
        .await;
        assert_eq!(final_success.status(), StatusCode::OK);
        assert_eq!(adapter.command_slots.available_permits(), 1);
    }

    #[tokio::test]
    async fn v1_command_echoes_request_id_and_preserves_command_result() {
        let (adapter, _state, _events) = adapter_fixture(node_ui_manifest());
        let payload = NodeCommandHttpRequest {
            schema_version: NODE_HTTP_SCHEMA_VERSION,
            request: NodeCommandRequest::new(51, NodeCommand::GetSnapshot),
            idempotency_key: None,
        };

        let response = post_v1_command(adapter, &payload).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body: NodeApiCommandResponse = json_body(response).await;
        assert_eq!(body.meta.schema_version, NODE_HTTP_SCHEMA_VERSION);
        assert_eq!(body.meta.request_id, 51);
        chrono::DateTime::parse_from_rfc3339(&body.meta.server_time)
            .expect("command server time is RFC3339");
        let result = body.result.expect("read command returns a result");
        assert_eq!(result.request_id, 51);
        assert!(matches!(
            result.outcome,
            ProjectedNodeCommandOutcome::Snapshot(_)
        ));
    }

    #[tokio::test]
    async fn v1_write_command_requires_an_idempotency_key() {
        let (adapter, _state, _events, writes) = adapter_fixture_with_write(
            node_ui_manifest(),
            StubWriteBehavior::Completed,
            Duration::ZERO,
        );
        let payload = NodeCommandHttpRequest {
            schema_version: NODE_HTTP_SCHEMA_VERSION,
            request: NodeCommandRequest::new(52, NodeCommand::StartNode),
            idempotency_key: None,
        };

        let response = post_v1_command(adapter, &payload).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: NodeApiErrorResponse = json_body(response).await;
        assert_eq!(body.error.code, NodeHttpErrorCode::InvalidRequest);
        assert_eq!(writes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn v1_write_command_returns_202_without_optimistic_result() {
        let (adapter, _state, _events, writes) = adapter_fixture_with_write(
            node_ui_manifest(),
            StubWriteBehavior::Completed,
            Duration::ZERO,
        );
        let payload = NodeCommandHttpRequest {
            schema_version: NODE_HTTP_SCHEMA_VERSION,
            request: NodeCommandRequest::new(53, NodeCommand::StartNode),
            idempotency_key: Some(IdempotencyKey::from_public_key(b"write-success")),
        };

        let response = post_v1_command(adapter.clone(), &payload).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body: NodeApiCommandResponse = json_body(response).await;
        assert!(body.result.is_none(), "acceptance must not claim success");
        let submitted = body.operation.expect("write returns an operation");
        assert_eq!(submitted.operation.state, OperationState::Queued);
        assert_eq!(submitted.operation.stage, "queued");
        assert!(submitted.result.is_none());

        let completed =
            wait_for_terminal_operation(&adapter, submitted.operation.operation_id.as_str()).await;
        assert_eq!(
            completed.operation.operation.state,
            OperationState::Succeeded
        );
        assert_eq!(completed.operation.operation.stage, "succeeded");
        let result = completed
            .operation
            .result
            .expect("succeeded operation returns its result");
        assert!(matches!(
            result.outcome,
            ProjectedNodeCommandOutcome::Completed { .. }
        ));
        assert_eq!(writes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn diagnostics_submit_as_cancellable_operations_and_cancel_is_idempotent() {
        let (adapter, _state, _events, writes) = adapter_fixture_with_write(
            node_ui_manifest(),
            StubWriteBehavior::Completed,
            Duration::from_millis(200),
        );
        let payload = NodeCommandHttpRequest {
            schema_version: NODE_HTTP_SCHEMA_VERSION,
            request: NodeCommandRequest::new(79, NodeCommand::GetDiagnostics),
            idempotency_key: Some(IdempotencyKey::from_public_key(b"diagnostics-http-cancel")),
        };

        let response = post_v1_command(adapter.clone(), &payload).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let submitted: NodeApiCommandResponse = json_body(response).await;
        let operation = submitted
            .operation
            .expect("diagnostics return an operation");
        assert_eq!(operation.operation.kind, OperationKind::Diagnostics);
        assert!(operation.operation.cancellable);
        assert!(operation.operation.cancel_blocked_reason.is_none());

        let first_cancel =
            post_v1_cancel(&adapter, operation.operation.operation_id.as_str()).await;
        assert_eq!(first_cancel.status(), StatusCode::ACCEPTED);
        let first_cancel: NodeApiOperationResponse = json_body(first_cancel).await;
        assert!(!first_cancel.operation.operation.state.is_terminal());

        let repeated = post_v1_cancel(&adapter, operation.operation.operation_id.as_str()).await;
        assert_eq!(repeated.status(), StatusCode::ACCEPTED);

        let terminal =
            wait_for_terminal_operation(&adapter, operation.operation.operation_id.as_str()).await;
        assert_eq!(
            terminal.operation.operation.state,
            OperationState::Cancelled
        );
        assert_eq!(writes.load(Ordering::SeqCst), 1);

        let after_terminal =
            post_v1_cancel(&adapter, operation.operation.operation_id.as_str()).await;
        assert_eq!(after_terminal.status(), StatusCode::CONFLICT);
        let error: NodeApiErrorResponse = json_body(after_terminal).await;
        assert_eq!(error.error.code, NodeHttpErrorCode::OperationConflict);
        assert_eq!(error.error.message_key, "operation.cancel.already_terminal");
    }

    #[tokio::test]
    async fn operation_cancel_rejects_non_cancellable_running_work() {
        let (adapter, _state, _events, _writes) = adapter_fixture_with_write(
            node_ui_manifest(),
            StubWriteBehavior::Completed,
            Duration::from_millis(200),
        );
        let payload = NodeCommandHttpRequest {
            schema_version: NODE_HTTP_SCHEMA_VERSION,
            request: NodeCommandRequest::new(81, NodeCommand::StartNode),
            idempotency_key: Some(IdempotencyKey::from_public_key(
                b"non-cancellable-http-start",
            )),
        };
        let submitted: NodeApiCommandResponse =
            json_body(post_v1_command(adapter.clone(), &payload).await).await;
        let operation = submitted
            .operation
            .expect("start node returns an operation");
        assert!(!operation.operation.cancellable);
        assert_eq!(
            operation.operation.cancel_blocked_reason.as_deref(),
            Some("operation.cancel.not_supported")
        );

        let rejected = post_v1_cancel(&adapter, operation.operation.operation_id.as_str()).await;
        assert_eq!(rejected.status(), StatusCode::CONFLICT);
        let error: NodeApiErrorResponse = json_body(rejected).await;
        assert_eq!(error.error.code, NodeHttpErrorCode::OperationConflict);
        assert_eq!(error.error.message_key, "operation.cancel.not_supported");

        wait_for_terminal_operation(&adapter, operation.operation.operation_id.as_str()).await;
    }

    #[tokio::test]
    async fn operation_cancel_hides_other_owners_and_rejects_unknown_ids() {
        let (adapter, _state, _events, _writes) = adapter_fixture_with_write(
            node_ui_manifest(),
            StubWriteBehavior::Completed,
            Duration::from_millis(100),
        );
        let payload = NodeCommandHttpRequest {
            schema_version: NODE_HTTP_SCHEMA_VERSION,
            request: NodeCommandRequest::new(80, NodeCommand::GetDiagnostics),
            idempotency_key: Some(IdempotencyKey::from_public_key(b"diagnostics-http-owner")),
        };
        let submitted: NodeApiCommandResponse =
            json_body(post_v1_command(adapter.clone(), &payload).await).await;
        let operation = submitted
            .operation
            .expect("diagnostics return an operation");

        let other_owner = NodeCommandContext::network_session(
            NodeId::from_public_key(b"another-webui-owner"),
            "other-network-member",
            [crate::node::NodePermission::ViewState],
            [],
            true,
        );
        let hidden = fetch_v1_operation_as(
            &adapter,
            operation.operation.operation_id.as_str(),
            other_owner.clone(),
        )
        .await;
        assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
        let hidden: NodeApiErrorResponse = json_body(hidden).await;
        assert_eq!(hidden.error.code, NodeHttpErrorCode::NotFound);

        let hidden = post_v1_cancel_as(
            &adapter,
            operation.operation.operation_id.as_str(),
            other_owner,
        )
        .await;
        assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
        let hidden: NodeApiErrorResponse = json_body(hidden).await;
        assert_eq!(hidden.error.code, NodeHttpErrorCode::NotFound);

        let missing = post_v1_cancel(&adapter, "missing-operation").await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);

        let owner_cancel =
            post_v1_cancel(&adapter, operation.operation.operation_id.as_str()).await;
        assert_eq!(owner_cancel.status(), StatusCode::ACCEPTED);
        wait_for_terminal_operation(&adapter, operation.operation.operation_id.as_str()).await;
    }

    #[tokio::test]
    async fn identity_rotation_returns_its_terminal_result_in_one_authenticated_post() {
        let rotation = NodeIdentityRotationSnapshot {
            old_node_id: "node-old".into(),
            old_fingerprint: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
            new_node_id: "node-new".into(),
            new_fingerprint: "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB".into(),
            created_at_unix_ms: 1_800_000_000_000,
            owner_initialized: true,
            owner_sessions_invalidated: true,
            affected_configured_network_count: 2,
        };
        let (adapter, _state, _events, writes) = adapter_fixture_with_write(
            node_ui_manifest(),
            StubWriteBehavior::IdentityRotated(rotation.clone()),
            Duration::ZERO,
        );
        let key = IdempotencyKey::from_public_key(b"identity-rotation-once");
        let payload = NodeCommandHttpRequest {
            schema_version: NODE_HTTP_SCHEMA_VERSION,
            request: NodeCommandRequest::new(
                78,
                NodeCommand::RotateNodeIdentity(crate::node::NodeRotateNodeIdentityCommand {
                    expected_fingerprint: rotation.old_fingerprint.clone(),
                }),
            ),
            idempotency_key: Some(key.clone()),
        };

        let response = post_v1_command(adapter.clone(), &payload).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body: NodeApiCommandResponse = json_body(response).await;
        let result = body.result.expect("rotation returns a synchronous result");
        assert!(matches!(
            result.outcome,
            ProjectedNodeCommandOutcome::IdentityRotated(ref completed)
                if completed.new_node_id == rotation.new_node_id
        ));
        let operation = body.operation.expect("rotation keeps the operation record");
        assert_eq!(operation.operation.state, OperationState::Succeeded);
        assert_eq!(operation.operation.stage, "succeeded");

        let retry = NodeCommandHttpRequest {
            schema_version: NODE_HTTP_SCHEMA_VERSION,
            request: NodeCommandRequest::new(
                79,
                NodeCommand::RotateNodeIdentity(crate::node::NodeRotateNodeIdentityCommand {
                    expected_fingerprint: rotation.old_fingerprint,
                }),
            ),
            idempotency_key: Some(key),
        };
        let retry_response = post_v1_command(adapter, &retry).await;
        assert_eq!(retry_response.status(), StatusCode::OK);
        let retry_body: NodeApiCommandResponse = json_body(retry_response).await;
        assert_eq!(
            retry_body
                .operation
                .expect("retry returns the existing operation")
                .operation
                .operation_id,
            operation.operation.operation_id
        );
        assert!(retry_body.result.is_some());
        assert_eq!(writes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn identity_rotation_timeout_returns_the_real_background_operation() {
        let rotation = NodeIdentityRotationSnapshot {
            old_node_id: "node-old".into(),
            old_fingerprint: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
            new_node_id: "node-new".into(),
            new_fingerprint: "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB".into(),
            created_at_unix_ms: 1_800_000_000_000,
            owner_initialized: false,
            owner_sessions_invalidated: false,
            affected_configured_network_count: 0,
        };
        let (adapter, _state, _events, writes) = adapter_fixture_with_write(
            node_ui_manifest(),
            StubWriteBehavior::IdentityRotated(rotation.clone()),
            Duration::from_millis(100),
        );
        let adapter = adapter.with_identity_rotation_wait_timeout(Duration::from_millis(10));
        let payload = NodeCommandHttpRequest {
            schema_version: NODE_HTTP_SCHEMA_VERSION,
            request: NodeCommandRequest::new(
                80,
                NodeCommand::RotateNodeIdentity(crate::node::NodeRotateNodeIdentityCommand {
                    expected_fingerprint: rotation.old_fingerprint,
                }),
            ),
            idempotency_key: Some(IdempotencyKey::from_public_key(
                b"identity-rotation-timeout",
            )),
        };

        let response = post_v1_command(adapter.clone(), &payload).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body: NodeApiCommandResponse = json_body(response).await;
        assert!(body.result.is_none());
        let submitted = body.operation.expect("timeout returns the real operation");
        assert!(!submitted.operation.state.is_terminal());

        let completed =
            wait_for_terminal_operation(&adapter, submitted.operation.operation_id.as_str()).await;
        assert_eq!(
            completed.operation.operation.state,
            OperationState::Succeeded
        );
        let result = completed
            .operation
            .result
            .expect("background rotation eventually exposes its result");
        assert!(matches!(
            result.outcome,
            ProjectedNodeCommandOutcome::IdentityRotated(ref completed)
                if completed.new_fingerprint == rotation.new_fingerprint
        ));
        assert_eq!(writes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn v1_operation_audit_links_request_operation_actor_and_outcome() {
        let (adapter, _state, _events, writes) = adapter_fixture_with_write(
            node_ui_manifest(),
            StubWriteBehavior::Completed,
            Duration::ZERO,
        );
        let invitation = "fk://owner-secret-invitation@example.test:6666";
        let payload = NodeCommandHttpRequest {
            schema_version: NODE_HTTP_SCHEMA_VERSION,
            request: NodeCommandRequest::new(
                77,
                NodeCommand::JoinNetwork(Box::new(NodeJoinNetworkCommand {
                    target: invitation.into(),
                    network_name: Some("home".into()),
                    start: false,
                    discovery_record: None,
                    network_id: None,
                    preview_digest: None,
                    network_policy_revision: None,
                    privacy_policy_revision: None,
                    terms_hash: None,
                    acknowledged_at_unix_ms: None,
                    exit_contribution_acknowledged: None,
                    signed_join_preview: None,
                })),
            ),
            idempotency_key: Some(IdempotencyKey::from_public_key(b"audit-operation-link")),
        };

        let response = post_v1_command(adapter.clone(), &payload).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let submitted: NodeApiCommandResponse = json_body(response).await;
        let operation_id = submitted
            .operation
            .expect("submitted operation")
            .operation
            .operation_id;
        let completed = wait_for_terminal_operation(&adapter, operation_id.as_str()).await;
        assert_eq!(
            completed.operation.operation.state,
            OperationState::Succeeded
        );

        let audit = adapter.commands.audit_entries();
        assert_eq!(audit.len(), 1);
        assert_eq!(
            audit[0].outcome,
            fuckipv6_common::domain::AuditOutcome::Succeeded
        );
        assert_eq!(
            audit[0].operation_id.as_ref().map(|id| id.as_str()),
            Some(operation_id.as_str())
        );
        assert_eq!(
            audit[0].actor,
            NodeId::from_public_key(b"fuckipv6-command-bus-test-owner")
        );
        assert_eq!(audit[0].details["request_id"], "77");
        assert_eq!(audit[0].details["actor_source"], "test");
        assert_eq!(audit[0].details["actor_role"], "owner");
        assert_eq!(audit[0].details["command"], "join_network");
        assert!(!format!("{:?}", audit[0]).contains("owner-secret-invitation"));
        assert_eq!(writes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn v1_write_reuses_the_operation_for_the_same_idempotency_key() {
        let (adapter, _state, _events, writes) = adapter_fixture_with_write(
            node_ui_manifest(),
            StubWriteBehavior::Completed,
            Duration::from_millis(20),
        );
        let key = IdempotencyKey::from_public_key(b"idempotent-start");
        let first = NodeCommandHttpRequest {
            schema_version: NODE_HTTP_SCHEMA_VERSION,
            request: NodeCommandRequest::new(54, NodeCommand::StartNode),
            idempotency_key: Some(key.clone()),
        };
        let retry = NodeCommandHttpRequest {
            schema_version: NODE_HTTP_SCHEMA_VERSION,
            request: NodeCommandRequest::new(55, NodeCommand::StartNode),
            idempotency_key: Some(key),
        };

        let first_response = post_v1_command(adapter.clone(), &first).await;
        assert_eq!(first_response.status(), StatusCode::ACCEPTED);
        let first_body: NodeApiCommandResponse = json_body(first_response).await;
        let first_operation = first_body.operation.expect("first operation");

        let retry_response = post_v1_command(adapter.clone(), &retry).await;
        assert_eq!(retry_response.status(), StatusCode::ACCEPTED);
        let retry_body: NodeApiCommandResponse = json_body(retry_response).await;
        let retry_operation = retry_body.operation.expect("replayed operation");

        assert_eq!(
            retry_operation.operation.operation_id,
            first_operation.operation.operation_id
        );
        let completed =
            wait_for_terminal_operation(&adapter, first_operation.operation.operation_id.as_str())
                .await;
        assert_eq!(
            completed.operation.operation.state,
            OperationState::Succeeded
        );
        assert_eq!(writes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn v1_write_rejects_idempotency_key_reuse_for_another_command() {
        let (adapter, _state, _events, writes) = adapter_fixture_with_write(
            node_ui_manifest(),
            StubWriteBehavior::Completed,
            Duration::ZERO,
        );
        let key = IdempotencyKey::from_public_key(b"conflicting-reuse");
        let first = NodeCommandHttpRequest {
            schema_version: NODE_HTTP_SCHEMA_VERSION,
            request: NodeCommandRequest::new(56, NodeCommand::StartNode),
            idempotency_key: Some(key.clone()),
        };
        let conflict = NodeCommandHttpRequest {
            schema_version: NODE_HTTP_SCHEMA_VERSION,
            request: NodeCommandRequest::new(57, NodeCommand::StopNode),
            idempotency_key: Some(key),
        };

        let first_response = post_v1_command(adapter.clone(), &first).await;
        assert_eq!(first_response.status(), StatusCode::ACCEPTED);
        let first_body: NodeApiCommandResponse = json_body(first_response).await;
        let first_operation_id = first_body
            .operation
            .expect("first operation")
            .operation
            .operation_id;
        wait_for_terminal_operation(&adapter, first_operation_id.as_str()).await;

        let conflict_response = post_v1_command(adapter, &conflict).await;
        assert_eq!(conflict_response.status(), StatusCode::CONFLICT);
        let body: NodeApiErrorResponse = json_body(conflict_response).await;
        assert_eq!(body.error.code, NodeHttpErrorCode::OperationConflict);
        assert_eq!(writes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn stale_v1_sensitive_command_is_rejected_before_handler_dispatch() {
        let (adapter, state, _events, writes) = adapter_fixture_with_write(
            node_ui_manifest(),
            StubWriteBehavior::Completed,
            Duration::ZERO,
        );
        let snapshot = state.snapshot().await;
        let context =
            NodeCommandContext::owner_session_for_snapshot(&snapshot, "webui-public-owner", false);
        let payload = NodeCommandHttpRequest {
            schema_version: NODE_HTTP_SCHEMA_VERSION,
            request: NodeCommandRequest::new(58, NodeCommand::Shutdown),
            idempotency_key: Some(IdempotencyKey::from_public_key(
                b"stale-reauthentication-v1",
            )),
        };
        let body = serde_json::to_vec(&payload).expect("serialize command request");
        let response = adapter
            .router()
            .layer(Extension(context))
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/commands")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .expect("v1 command request"),
            )
            .await
            .expect("v1 command response");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body: NodeApiCommandResponse = json_body(response).await;
        let result = body.result.expect("synchronous rejection result");
        let ProjectedNodeCommandOutcome::Rejected(problem) = result.outcome else {
            panic!("expected sensitive command rejection");
        };
        assert_eq!(problem.code, NodeProblemCode::AuthenticationRequired);
        assert!(problem.params.is_empty());
        assert!(problem.network_name.is_none());
        assert!(problem.details.is_none());
        assert!(body.operation.is_none());
        assert_eq!(writes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn authorization_errors_do_not_reveal_protected_objects_or_parameters() {
        let protected_network = "owners-only-network";
        let protected_object = "protected-node-id";
        let problem = NodeProblem::new(
            NodeProblemCode::PermissionDenied,
            "object protected-node-id exists in owners-only-network",
        )
        .for_network(protected_network)
        .with_param("object_id", protected_object);
        let (adapter, _state, _events, _writes) = adapter_fixture_with_write(
            node_ui_manifest(),
            StubWriteBehavior::Rejected(problem),
            Duration::ZERO,
        );
        let payload = NodeCommandHttpRequest {
            schema_version: NODE_HTTP_SCHEMA_VERSION,
            request: NodeCommandRequest::new(59, NodeCommand::StartNode),
            idempotency_key: None,
        };

        let response = post_command(adapter.clone(), &payload).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let raw: serde_json::Value = json_body(response).await;
        let serialized = raw.to_string();
        assert!(
            !serialized.contains(protected_network),
            "response leaked network: {serialized}"
        );
        assert!(!serialized.contains(protected_object));
        assert!(!serialized.contains("protected-node-id exists"));
        let body: NodeCommandHttpResponse = serde_json::from_value(raw).expect("decode response");
        let ProjectedNodeCommandOutcome::Rejected(problem) = body.result.outcome else {
            panic!("expected a rejected command");
        };
        assert_eq!(problem.code, NodeProblemCode::PermissionDenied);
        assert_eq!(problem.category, NodeErrorCategory::Authorization);
        assert_eq!(problem.message_key, "error.operation_permission_denied");
        assert_eq!(problem.message, "无权执行此操作");
        assert!(problem.params.is_empty());
        assert!(problem.network_name.is_none());

        let payload = NodeCommandHttpRequest {
            schema_version: NODE_HTTP_SCHEMA_VERSION,
            request: NodeCommandRequest::new(60, NodeCommand::StartNode),
            idempotency_key: Some(IdempotencyKey::from_public_key(b"authorization-redaction")),
        };
        let response = post_v1_command(adapter.clone(), &payload).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let submitted: NodeApiCommandResponse = json_body(response).await;
        let operation_id = submitted
            .operation
            .expect("submitted operation")
            .operation
            .operation_id;
        let failed = wait_for_terminal_operation(&adapter, operation_id.as_str()).await;
        let error = failed
            .operation
            .operation
            .error
            .expect("failed operation carries a stable error");
        assert!(error.object.is_none());
        assert!(error.params.is_empty());
        assert_eq!(error.code, "operation.permission_denied");
        assert_eq!(error.message_key, "error.operation_permission_denied");
        let result = failed.operation.result.expect("failed operation result");
        let ProjectedNodeCommandOutcome::Rejected(problem) = result.outcome else {
            panic!("expected a rejected operation result");
        };
        assert_eq!(problem.message, "无权执行此操作");
        assert!(problem.params.is_empty());
        assert!(problem.network_name.is_none());
    }

    #[tokio::test]
    async fn failed_operation_exposes_stable_error_object_and_recovery() {
        let problem = NodeProblem::new(NodeProblemCode::Unavailable, "runtime backend unavailable")
            .retryable(true);
        let (adapter, _state, _events, _writes) = adapter_fixture_with_write(
            node_ui_manifest(),
            StubWriteBehavior::Rejected(problem),
            Duration::ZERO,
        );
        let payload = NodeCommandHttpRequest {
            schema_version: NODE_HTTP_SCHEMA_VERSION,
            request: NodeCommandRequest::new(58, NodeCommand::StartNode),
            idempotency_key: Some(IdempotencyKey::from_public_key(b"failed-operation")),
        };
        let response = post_v1_command(adapter.clone(), &payload).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let submitted: NodeApiCommandResponse = json_body(response).await;
        let operation_id = submitted
            .operation
            .expect("submitted operation")
            .operation
            .operation_id;

        let failed = wait_for_terminal_operation(&adapter, operation_id.as_str()).await;
        assert_eq!(failed.operation.operation.state, OperationState::Failed);
        let error = failed
            .operation
            .operation
            .error
            .expect("failed operation carries a stable error");
        assert_eq!(error.code, "operation.unavailable");
        assert_eq!(error.message_key, "error.operation_unavailable");
        assert_eq!(
            error.object.as_ref().map(|object| &object.kind),
            Some(&ObjectKind::Node)
        );
        assert!(error.retryable);
        assert_eq!(
            error
                .recovery
                .as_ref()
                .map(|recovery| recovery.kind.as_str()),
            Some("retry")
        );
        assert!(failed.operation.result.is_some());
    }

    #[tokio::test]
    async fn cancelled_and_timed_out_operations_have_distinct_terminal_states() {
        for (key_seed, problem_code, expected, stage) in [
            (
                b"cancelled-operation".as_slice(),
                NodeProblemCode::Cancelled,
                OperationState::Cancelled,
                "cancelled",
            ),
            (
                b"timed-out-operation".as_slice(),
                NodeProblemCode::TimedOut,
                OperationState::TimedOut,
                "timed_out",
            ),
        ] {
            let (adapter, _state, _events, _writes) = adapter_fixture_with_write(
                node_ui_manifest(),
                StubWriteBehavior::Rejected(
                    NodeProblem::new(problem_code, "terminal state test").retryable(true),
                ),
                Duration::ZERO,
            );
            let payload = NodeCommandHttpRequest {
                schema_version: NODE_HTTP_SCHEMA_VERSION,
                request: NodeCommandRequest::new(59, NodeCommand::StartNode),
                idempotency_key: Some(IdempotencyKey::from_public_key(key_seed)),
            };
            let response = post_v1_command(adapter.clone(), &payload).await;
            assert_eq!(response.status(), StatusCode::ACCEPTED);
            let submitted: NodeApiCommandResponse = json_body(response).await;
            let operation_id = submitted
                .operation
                .expect("submitted operation")
                .operation
                .operation_id;
            let operation = wait_for_terminal_operation(&adapter, operation_id.as_str()).await;
            assert_eq!(operation.operation.operation.state, expected);
            assert_eq!(operation.operation.operation.stage, stage);
        }
    }

    #[tokio::test]
    async fn operation_events_report_each_progress_stage_in_order() {
        let (adapter, _state, events, _writes) = adapter_fixture_with_write(
            node_ui_manifest(),
            StubWriteBehavior::Completed,
            Duration::from_millis(20),
        );
        let mut receiver = events.subscribe();
        let payload = NodeCommandHttpRequest {
            schema_version: NODE_HTTP_SCHEMA_VERSION,
            request: NodeCommandRequest::new(60, NodeCommand::StartNode),
            idempotency_key: Some(IdempotencyKey::from_public_key(b"operation-events")),
        };
        let response = post_v1_command(adapter.clone(), &payload).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let submitted: NodeApiCommandResponse = json_body(response).await;
        let operation_id = submitted
            .operation
            .expect("submitted operation")
            .operation
            .operation_id;
        wait_for_terminal_operation(&adapter, operation_id.as_str()).await;

        let mut updates = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            if let NodeEventKind::OperationChanged {
                operation_id: observed,
                state,
                stage,
            } = event.kind
            {
                if observed == operation_id.as_str() {
                    updates.push((state, stage));
                }
            }
        }
        assert_eq!(
            updates,
            vec![
                (OperationState::Queued, "queued".into()),
                (OperationState::Running, "dispatching".into()),
                (OperationState::Running, "executing".into()),
                (OperationState::Succeeded, "succeeded".into()),
            ]
        );
    }

    #[tokio::test]
    async fn v1_operation_route_returns_stable_not_found_envelope() {
        let (adapter, _state, _events) = adapter_fixture(node_ui_manifest());
        let response = adapter
            .test_router()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/operations/op_missing?schema_version=1")
                    .body(Body::empty())
                    .expect("operation request"),
            )
            .await
            .expect("operation response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body: NodeApiErrorResponse = json_body(response).await;
        assert_eq!(body.error.code, NodeHttpErrorCode::NotFound);
        assert!(body.meta.request_id > 0);
        assert_eq!(body.meta.schema_version, NODE_HTTP_SCHEMA_VERSION);
        chrono::DateTime::parse_from_rfc3339(&body.meta.server_time)
            .expect("error server time is RFC3339");
    }

    #[tokio::test]
    async fn v1_object_reads_are_projected_from_the_canonical_snapshot() {
        let (adapter, state, _events) = adapter_fixture(node_ui_manifest());
        state
            .update(|snapshot| {
                snapshot.networks.push(NodeNetworkSnapshot {
                    name: "home".into(),
                    tun_name: "tun0".into(),
                    status: NodeNetworkStatus::Online,
                    assigned_ipv6: None,
                    assigned_ipv4: None,
                    p2p_listen_addr: None,
                    uptime_secs: 12,
                    roles: Vec::new(),
                    peer_count: 0,
                    traffic: NodeTrafficSnapshot::default(),
                });
            })
            .await;

        let response = adapter
            .clone()
            .test_router()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/objects/network/home?schema_version=1")
                    .body(Body::empty())
                    .expect("object request"),
            )
            .await
            .expect("object response");
        assert_eq!(response.status(), StatusCode::OK);
        let body: NodeApiObjectResponse = json_body(response).await;
        assert_eq!(body.meta.schema_version, NODE_HTTP_SCHEMA_VERSION);
        assert!(matches!(
            body.object,
            NodeApiObject::Network { id, network }
                if id == "home" && network.name == "home" && network.uptime_secs == Some(12)
        ));

        let missing = adapter
            .test_router()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/objects/network/missing?schema_version=1")
                    .body(Body::empty())
                    .expect("missing object request"),
            )
            .await
            .expect("missing object response");
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        let missing: NodeApiErrorResponse = json_body(missing).await;
        assert_eq!(missing.error.code, NodeHttpErrorCode::NotFound);
    }

    #[tokio::test]
    async fn v1_events_reconnect_from_cursor_without_losing_or_repeating_events() {
        let (adapter, state, _events) = adapter_fixture(node_ui_manifest());
        state.update(|_| {}).await;

        let response = adapter
            .clone()
            .test_router()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/events?schema_version=1&after=0:0")
                    .body(Body::empty())
                    .expect("event stream request"),
            )
            .await
            .expect("event stream response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream")
        );
        let mut body = response.into_body().into_data_stream();
        let (id, initial) = next_sse_event(&mut body).await;
        assert_eq!(id.as_deref(), Some("0:1"));
        assert!(matches!(
            initial,
            NodeApiEvent::Snapshot {
                cursor,
                snapshot,
                ..
            } if cursor == NodeEventCursor::new(0, 1) && snapshot.revision == 1
        ));
        let (id, replayed) = next_sse_event(&mut body).await;
        assert_eq!(id.as_deref(), Some("1:1"));
        assert!(matches!(
            replayed,
            NodeApiEvent::Event {
                event: ProjectedNodeEvent {
                    sequence: 1,
                    snapshot_revision: 1,
                    kind: ProjectedNodeEventKind::StateChanged { revision: 1 },
                },
                ..
            }
        ));
        drop(body);

        // A reconnect sends a fresh authoritative snapshot, then resumes
        // strictly after Last-Event-ID. The already acknowledged event is not
        // repeated.
        state.update(|_| {}).await;
        let response = adapter
            .test_router()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/events?schema_version=1")
                    .header("last-event-id", "1:1")
                    .body(Body::empty())
                    .expect("reconnect event stream request"),
            )
            .await
            .expect("reconnect event stream response");
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body().into_data_stream();
        let (id, snapshot) = next_sse_event(&mut body).await;
        assert_eq!(id.as_deref(), Some("1:2"));
        assert!(matches!(
            snapshot,
            NodeApiEvent::Snapshot {
                cursor,
                snapshot,
                ..
            } if cursor == NodeEventCursor::new(1, 2) && snapshot.revision == 2
        ));
        // Fetching the authoritative reconnect snapshot also completes a read
        // command, so sequence 2 is that command event. Sequence 3 is the state
        // change that happened after the original snapshot.
        let (id, completed) = next_sse_event(&mut body).await;
        assert_eq!(id.as_deref(), Some("2:1"));
        assert!(matches!(
            completed,
            NodeApiEvent::Event {
                event: ProjectedNodeEvent {
                    sequence: 2,
                    snapshot_revision: 1,
                    kind: ProjectedNodeEventKind::CommandCompleted { success: true, .. },
                },
                ..
            }
        ));
        let (id, resumed) = next_sse_event(&mut body).await;
        assert_eq!(id.as_deref(), Some("3:2"));
        assert!(matches!(
            resumed,
            NodeApiEvent::Event {
                event: ProjectedNodeEvent {
                    sequence: 3,
                    snapshot_revision: 2,
                    kind: ProjectedNodeEventKind::StateChanged { revision: 2 },
                },
                ..
            }
        ));
    }

    #[tokio::test]
    async fn v1_events_reject_expired_and_malformed_cursors() {
        let (adapter, _state, events) = adapter_fixture(node_ui_manifest());
        for revision in 1..=40 {
            events.publish(NodeEventKind::StateChanged { revision });
        }

        let expired = adapter
            .clone()
            .test_router()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/events?schema_version=1&after=0:0")
                    .body(Body::empty())
                    .expect("expired cursor request"),
            )
            .await
            .expect("expired cursor response");
        assert_eq!(expired.status(), StatusCode::CONFLICT);
        let expired: NodeApiErrorResponse = json_body(expired).await;
        assert_eq!(expired.error.code, NodeHttpErrorCode::ResyncRequired);
        assert_eq!(
            expired.resync_reason,
            Some(NodeEventReplayReason::HistoryGap)
        );
        assert_eq!(expired.requested_cursor, Some(NodeEventCursor::new(0, 0)));
        assert_eq!(
            expired.latest_cursor.map(|cursor| cursor.sequence),
            Some(40)
        );

        let malformed = adapter
            .test_router()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/events?schema_version=1&after=not-a-cursor")
                    .body(Body::empty())
                    .expect("malformed cursor request"),
            )
            .await
            .expect("malformed cursor response");
        assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
        let malformed: NodeApiErrorResponse = json_body(malformed).await;
        assert_eq!(malformed.error.code, NodeHttpErrorCode::InvalidRequest);
    }

    #[tokio::test]
    async fn v1_events_lag_and_revision_gap_end_in_resync_required() {
        for expected_reason in [
            NodeEventReplayReason::RevisionGap,
            NodeEventReplayReason::Lagged,
        ] {
            let (adapter, _state, events) = adapter_fixture(node_ui_manifest());
            let response = adapter
                .test_router()
                .oneshot(
                    Request::builder()
                        .uri("/api/v1/events?schema_version=1")
                        .body(Body::empty())
                        .expect("event stream request"),
                )
                .await
                .expect("event stream response");
            let mut body = response.into_body().into_data_stream();
            let (_, initial) = next_sse_event(&mut body).await;
            assert!(matches!(initial, NodeApiEvent::Snapshot { .. }));
            let (id, completed) = next_sse_event(&mut body).await;
            assert_eq!(id.as_deref(), Some("1:0"));
            assert!(matches!(
                completed,
                NodeApiEvent::Event {
                    event: ProjectedNodeEvent {
                        sequence: 1,
                        snapshot_revision: 0,
                        kind: ProjectedNodeEventKind::CommandCompleted { success: true, .. },
                    },
                    ..
                }
            ));

            let (latest_sequence, latest_revision) =
                if expected_reason == NodeEventReplayReason::Lagged {
                    for index in 1..=40 {
                        events.publish(NodeEventKind::LogAppended {
                            level: crate::node::NodeLogLevel::Info,
                            message: format!("lag-{index}"),
                        });
                    }
                    (41, 0)
                } else {
                    events.publish(NodeEventKind::StateChanged { revision: 2 });
                    (2, 2)
                };

            let (id, event) = next_sse_event(&mut body).await;
            assert_eq!(id, None);
            assert!(matches!(
                event,
                NodeApiEvent::ResyncRequired {
                    requested_cursor,
                    latest_cursor,
                    reason,
                    ..
                } if requested_cursor == NodeEventCursor::new(1, 0)
                    && latest_cursor == NodeEventCursor::new(latest_sequence, latest_revision)
                    && reason == expected_reason
            ));
        }
    }

    #[tokio::test]
    async fn v1_routes_reject_schema_mismatch_with_meta_envelope() {
        let (adapter, _state, _events) = adapter_fixture(node_ui_manifest());
        for path in [
            "/api/v1/snapshot?schema_version=2",
            "/api/v1/manifest?schema_version=2",
            "/api/v1/operations/op?schema_version=2",
            "/api/v1/objects/network/home?schema_version=2",
            "/api/v1/events?schema_version=2",
        ] {
            let response = adapter
                .clone()
                .test_router()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .body(Body::empty())
                        .expect("v1 version mismatch request"),
                )
                .await
                .expect("v1 version mismatch response");
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
            let body: NodeApiErrorResponse = json_body(response).await;
            assert_eq!(body.error.code, NodeHttpErrorCode::SchemaVersionMismatch);
            assert_eq!(body.meta.schema_version, NODE_HTTP_SCHEMA_VERSION);
            assert!(body.meta.request_id > 0);
            assert_eq!(body.requested_schema_version, Some(2));
            assert_eq!(
                body.supported_schema_version,
                Some(NODE_HTTP_SCHEMA_VERSION)
            );
        }
    }

    #[tokio::test]
    async fn command_endpoint_forwards_successful_read_command_to_bus() {
        let (adapter, _state, _events) = adapter_fixture(node_ui_manifest());
        let payload = NodeCommandHttpRequest {
            schema_version: NODE_HTTP_SCHEMA_VERSION,
            request: NodeCommandRequest::new(41, NodeCommand::GetSnapshot),
            idempotency_key: None,
        };

        let response = post_command(adapter, &payload).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body: NodeCommandHttpResponse = json_body(response).await;
        assert_eq!(body.schema_version, NODE_HTTP_SCHEMA_VERSION);
        assert_eq!(body.result.request_id, 41);
        assert!(matches!(
            body.result.outcome,
            ProjectedNodeCommandOutcome::Snapshot(snapshot) if snapshot.revision == 0
        ));
    }

    #[tokio::test]
    async fn command_endpoint_maps_bus_validation_errors_to_bad_request() {
        let (adapter, _state, _events) = adapter_fixture(node_ui_manifest());
        let payload = NodeCommandHttpRequest {
            schema_version: NODE_HTTP_SCHEMA_VERSION,
            request: NodeCommandRequest::new(
                42,
                NodeCommand::StopNetworks {
                    networks: Vec::new(),
                    all: false,
                },
            ),
            idempotency_key: None,
        };

        let response = post_command(adapter, &payload).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: NodeHttpErrorResponse = json_body(response).await;
        assert_eq!(body.error.code, NodeHttpErrorCode::InvalidRequest);
        let problem = body.problem.expect("validation problem in response");
        assert_eq!(problem.code, NodeProblemCode::InvalidRequest);
    }

    #[tokio::test]
    async fn command_endpoint_preserves_rejected_unavailable_result() {
        let (adapter, _state, _events) = adapter_fixture(node_ui_manifest());
        let payload = NodeCommandHttpRequest {
            schema_version: NODE_HTTP_SCHEMA_VERSION,
            request: NodeCommandRequest::new(43, NodeCommand::Shutdown),
            idempotency_key: None,
        };

        let response = post_command(adapter, &payload).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body: NodeCommandHttpResponse = json_body(response).await;
        assert_eq!(body.schema_version, NODE_HTTP_SCHEMA_VERSION);
        assert_eq!(body.result.request_id, 43);
        assert!(matches!(
            body.result.outcome,
            ProjectedNodeCommandOutcome::Rejected(problem)
                if problem.code == NodeProblemCode::Unavailable
        ));
    }

    #[test]
    fn unsupported_problem_uses_non_retryable_client_error_semantics() {
        assert_eq!(
            status_for_problem(NodeProblemCode::Unsupported),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    #[tokio::test]
    async fn command_endpoint_rejects_schema_mismatch_before_bus_submission() {
        let (adapter, _state, _events) = adapter_fixture(node_ui_manifest());
        let payload = NodeCommandHttpRequest {
            schema_version: NODE_HTTP_SCHEMA_VERSION + 1,
            request: NodeCommandRequest::new(44, NodeCommand::GetSnapshot),
            idempotency_key: None,
        };

        let response = post_command(adapter, &payload).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: NodeHttpErrorResponse = json_body(response).await;
        assert_eq!(body.error.code, NodeHttpErrorCode::SchemaVersionMismatch);
        assert_eq!(
            body.requested_schema_version,
            Some(NODE_HTTP_SCHEMA_VERSION + 1)
        );
        assert_eq!(
            body.supported_schema_version,
            Some(NODE_HTTP_SCHEMA_VERSION)
        );
    }

    #[tokio::test]
    async fn schema_version_mismatch_fails_before_reading_node_state() {
        let (adapter, _state, _events) = adapter_fixture(node_ui_manifest());

        for path in [
            "/api/node/snapshot?schema_version=2",
            "/api/node/manifest?schema_version=2",
            "/api/node/events?schema_version=2",
        ] {
            let response = adapter
                .clone()
                .test_router()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .body(Body::empty())
                        .expect("version mismatch request"),
                )
                .await
                .expect("version mismatch response");
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
            let body: NodeHttpErrorResponse = json_body(response).await;
            assert_eq!(body.error.code, NodeHttpErrorCode::SchemaVersionMismatch);
            assert_eq!(body.requested_schema_version, Some(2));
            assert_eq!(
                body.supported_schema_version,
                Some(NODE_HTTP_SCHEMA_VERSION)
            );
        }
    }

    #[tokio::test]
    async fn manifest_schema_mismatch_is_a_contract_error() {
        let mut manifest = node_ui_manifest();
        manifest.schema_version += 1;
        let (adapter, _state, _events) = adapter_fixture(manifest);

        let error = adapter
            .manifest(crate::node::test_owner_context())
            .await
            .expect_err("manifest must be rejected");
        assert!(matches!(
            error,
            NodeHttpAdapterError::ManifestSchemaMismatch { .. }
        ));
    }

    #[tokio::test]
    async fn malformed_manifest_is_rejected_before_rendering() {
        let mut manifest = node_ui_manifest();
        manifest.pages.swap(0, 1);
        let (adapter, _state, _events) = adapter_fixture(manifest);

        let error = adapter
            .manifest(crate::node::test_owner_context())
            .await
            .expect_err("malformed manifest must be rejected");
        assert!(matches!(
            error,
            NodeHttpAdapterError::InvalidManifest { violations }
                if violations.iter().any(|violation| violation.starts_with("page_set_or_order_mismatch:"))
        ));
    }

    #[tokio::test]
    async fn websocket_stream_sends_initial_snapshot_and_live_events() {
        let (adapter, state, _events) = adapter_fixture(node_ui_manifest());
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .expect("bind test server");
        let address = listener.local_addr().expect("test server address");
        let server = tokio::spawn(async move {
            axum::serve(listener, adapter.test_router())
                .await
                .expect("serve adapter");
        });

        let (mut socket, _) =
            connect_async(format!("ws://{address}/api/node/events?schema_version=1"))
                .await
                .expect("connect websocket");
        let initial = timeout(Duration::from_secs(2), socket.next())
            .await
            .expect("initial snapshot timeout")
            .expect("initial snapshot message")
            .expect("valid initial snapshot frame");
        let ClientMessage::Text(initial) = initial else {
            panic!("expected text websocket frame");
        };
        let initial: NodeWsMessage =
            serde_json::from_str(initial.as_str()).expect("decode initial snapshot");
        assert_eq!(initial.schema_version(), NODE_HTTP_SCHEMA_VERSION);
        match initial {
            NodeWsMessage::Snapshot { snapshot, .. } => {
                assert_eq!(snapshot.control_health, NodeControlHealth::Available);
            }
            other => panic!("expected initial snapshot, got {other:?}"),
        }

        state.update(|_| {}).await;

        let live_event = timeout(Duration::from_secs(2), async {
            loop {
                let frame = socket
                    .next()
                    .await
                    .expect("live event frame")
                    .expect("valid live event frame");
                let ClientMessage::Text(frame) = frame else {
                    continue;
                };
                let message: NodeWsMessage =
                    serde_json::from_str(frame.as_str()).expect("decode live event");
                if matches!(
                    message,
                    NodeWsMessage::Event {
                        event: ProjectedNodeEvent {
                            kind: ProjectedNodeEventKind::StateChanged { .. },
                            ..
                        },
                        ..
                    }
                ) {
                    break message;
                }
            }
        })
        .await
        .expect("live event timeout");
        let NodeWsMessage::Event { event, .. } = live_event else {
            unreachable!();
        };
        assert_eq!(
            event.kind,
            ProjectedNodeEventKind::StateChanged { revision: 1 }
        );

        socket
            .send(ClientMessage::Close(None))
            .await
            .expect("close websocket");
        server.abort();
    }

    #[tokio::test]
    async fn websocket_session_omits_hidden_live_log_content() {
        let (snapshot, context) = privacy_transport_fixture();
        let events = NodeEventBus::new(16);
        let state = NodeStateStore::with_events(events.clone());
        state.update(|current| *current = snapshot).await;
        let initial = state.snapshot().await;
        let mut session =
            NodeWsSession::new(events.subscribe(), state.clone(), context, initial.revision);

        events.publish(NodeEventKind::LogAppended {
            level: NodeLogLevel::Info,
            message: HIDDEN_LOG_MESSAGE.into(),
        });
        let message = session
            .next_message()
            .await
            .expect("project live event")
            .expect("live event message");
        let NodeWsMessage::Event { event, .. } = message else {
            panic!("expected live event, got {message:?}");
        };
        let ProjectedNodeEventKind::LogAppended { level, message } = event.kind else {
            panic!("expected projected log event");
        };
        assert_eq!(level, NodeLogLevel::Info);
        assert!(message.is_none());
    }

    #[tokio::test]
    async fn websocket_lag_resync_serializes_only_the_projected_snapshot() {
        let (snapshot, context) = privacy_transport_fixture();
        let events = NodeEventBus::new(1);
        let state = NodeStateStore::with_events(events.clone());
        state.update(|current| *current = snapshot).await;
        let initial = state.snapshot().await;
        let mut session =
            NodeWsSession::new(events.subscribe(), state.clone(), context, initial.revision);

        state.update(|_| {}).await;
        state.update(|_| {}).await;
        let message = session
            .next_message()
            .await
            .expect("project lag recovery")
            .expect("lag recovery message");
        let NodeWsMessage::Resync {
            reason, snapshot, ..
        } = message
        else {
            panic!("expected lag recovery, got {message:?}");
        };
        assert_eq!(reason, NodeWsResyncReason::Lagged);
        assert!(snapshot.identity.is_none());
        assert!(snapshot.logs.is_empty());
        assert!(snapshot.recent_activity.is_empty());
        let serialized = serde_json::to_string(&snapshot).expect("serialize resync snapshot");
        assert_privacy_serialization_omits_hidden_values("websocket resync", &serialized);
    }

    #[tokio::test]
    async fn websocket_session_fails_closed_for_ambiguous_field_policies() {
        let (mut snapshot, _) = privacy_transport_fixture();
        let duplicate = snapshot.access_control.field_privacy_policies[0].clone();
        snapshot
            .access_control
            .field_privacy_policies
            .push(duplicate);
        let context = privacy_transport_context(&snapshot);
        let events = NodeEventBus::new(16);
        let state = NodeStateStore::with_events(events.clone());
        state.update(|current| *current = snapshot).await;
        let initial = state.snapshot().await;
        let mut session =
            NodeWsSession::new(events.subscribe(), state.clone(), context, initial.revision);

        events.publish(NodeEventKind::LogAppended {
            level: NodeLogLevel::Info,
            message: HIDDEN_LOG_MESSAGE.into(),
        });
        assert!(matches!(
            session.next_message().await,
            Err(NodeWsSessionError::Projection(
                NodeSnapshotProjectionError::InvalidFieldPolicy
            ))
        ));
    }

    fn policy_test_command() -> NodeCommand {
        let network_id = NetworkId::from_public_key(b"http-policy-test-network");
        NodeCommand::TestPolicy(
            fuckipv6_common::domain::parse_policy_test_input(
                &serde_json::json!({
                    "network_id": network_id.to_string(),
                    "actor": NodeId::from_public_key(b"http-policy-test-actor").to_string(),
                    "role": "member",
                    "groups": [],
                    "membership_active": true,
                    "recent_reauthentication": true,
                    "emergency_coordinator": false,
                    "request": {
                        "action": "read",
                        "resource": "network",
                        "resource_network_id": network_id.to_string(),
                        "intent": "read_only"
                    },
                    "now_unix_ms": 1_500
                })
                .to_string(),
            )
            .expect("valid policy test command"),
        )
    }

    #[test]
    fn policy_test_results_are_synchronous_network_scoped_operations() {
        let command = policy_test_command();
        let NodeCommand::TestPolicy(input) = &command else {
            panic!("expected policy test command");
        };
        let outcome = NodeCommandOutcome::PolicyTest(Box::new(input.evaluate()));
        let result = NodeCommandResult::completed(46, outcome);
        assert_eq!(status_for_command_result(&result), StatusCode::OK);
        assert_eq!(operation_kind(&command), OperationKind::PolicyTest);
        let local_node = NodeId::from_public_key(b"http-local-node");
        assert_eq!(
            operation_target(&command, &local_node),
            DomainObjectRef {
                kind: ObjectKind::Network,
                id: input.network_id.to_string(),
            }
        );
    }

    #[test]
    fn configuration_history_has_a_distinct_operation_kind() {
        assert_eq!(
            operation_kind(&NodeCommand::GetConfigurationHistory),
            OperationKind::ConfigurationHistory
        );
    }

    #[tokio::test]
    async fn duplicate_and_stale_revisions_are_suppressed() {
        let events = NodeEventBus::new(16);
        let state = NodeStateStore::with_events(events.clone());
        let initial = state.snapshot().await;
        let mut session = NodeWsSession::new(
            events.subscribe(),
            state.clone(),
            crate::node::test_owner_context(),
            initial.revision,
        );

        state.update(|_| {}).await;
        let first = session
            .next_message()
            .await
            .expect("read first revision")
            .expect("first revision message");
        assert!(matches!(
            first,
            NodeWsMessage::Event {
                event: ProjectedNodeEvent {
                    kind: ProjectedNodeEventKind::StateChanged { revision: 1 },
                    ..
                },
                ..
            }
        ));

        events.publish(NodeEventKind::StateChanged { revision: 1 });
        state.update(|_| {}).await;
        let second = session
            .next_message()
            .await
            .expect("read second revision")
            .expect("second revision message");
        assert!(matches!(
            second,
            NodeWsMessage::Event {
                event: ProjectedNodeEvent {
                    kind: ProjectedNodeEventKind::StateChanged { revision: 2 },
                    ..
                },
                ..
            }
        ));
    }

    #[tokio::test]
    async fn revision_gap_returns_a_fresh_resync_snapshot() {
        let events = NodeEventBus::new(16);
        let state = NodeStateStore::with_events(events.clone());
        let initial = state.snapshot().await;
        let mut session = NodeWsSession::new(
            events.subscribe(),
            state.clone(),
            crate::node::test_owner_context(),
            initial.revision,
        );

        state.update(|snapshot| snapshot.revision = 9).await;
        let message = session
            .next_message()
            .await
            .expect("read resync")
            .expect("resync message");
        match message {
            NodeWsMessage::Resync {
                reason,
                snapshot,
                schema_version,
            } => {
                assert_eq!(schema_version, NODE_HTTP_SCHEMA_VERSION);
                assert_eq!(reason, NodeWsResyncReason::RevisionGap);
                assert_eq!(snapshot.revision, 10);
            }
            other => panic!("expected resync, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn lagging_receiver_recovers_from_canonical_state() {
        let events = NodeEventBus::new(1);
        let state = NodeStateStore::with_events(events.clone());
        let initial = state.snapshot().await;
        let mut session = NodeWsSession::new(
            events.subscribe(),
            state.clone(),
            crate::node::test_owner_context(),
            initial.revision,
        );

        state.update(|_| {}).await;
        state.update(|_| {}).await;

        let message = session
            .next_message()
            .await
            .expect("read lag recovery")
            .expect("lag recovery message");
        match message {
            NodeWsMessage::Resync {
                reason, snapshot, ..
            } => {
                assert_eq!(reason, NodeWsResyncReason::Lagged);
                assert_eq!(snapshot.revision, 2);
            }
            other => panic!("expected lag resync, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn event_source_disconnect_ends_the_stream() {
        let (sender, receiver) = broadcast::channel(1);
        drop(sender);
        let state = NodeStateStore::new();
        let mut session = NodeWsSession::new(receiver, state, crate::node::test_owner_context(), 0);

        assert_eq!(session.next_message().await.expect("closed stream"), None);
    }
}
