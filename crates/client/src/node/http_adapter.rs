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
        Path, Query, State,
    },
    http::{HeaderMap, StatusCode},
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
use tokio::sync::broadcast;

use super::{
    NodeCommand, NodeCommandBus, NodeCommandBusError, NodeCommandOutcome, NodeCommandRequest,
    NodeCommandResult, NodeControlHealth, NodeErrorCategory, NodeEvent, NodeEventBus,
    NodeEventCursor, NodeEventKind, NodeEventReplay, NodeEventReplayReason, NodeProblem,
    NodeProblemCode, NodeSnapshot, NodeStateStore, NodeUiManifest, NODE_UI_MANIFEST_SCHEMA_VERSION,
};

/// Version of the HTTP/WebSocket envelope contract used by desktop and
/// browser node UIs. It is independent of the command and UI-manifest schemas.
pub const NODE_HTTP_SCHEMA_VERSION: u16 = 1;

/// Stable path prefix for the shared node control adapter.
pub const NODE_HTTP_API_PREFIX: &str = "/api/node";

/// Stable prefix for the versioned control API. The legacy `/api/node` routes
/// remain available while UI adapters migrate, but new clients must use v1.
pub const NODE_HTTP_V1_API_PREFIX: &str = "/api/v1";

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
    pub snapshot: NodeSnapshot,
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
    pub result: Option<NodeCommandResult>,
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
    pub result: Option<NodeCommandResult>,
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
        identity: super::NodeIdentitySnapshot,
    },
    Network {
        id: String,
        network: super::NodeNetworkSnapshot,
    },
    Peer {
        id: String,
        peer: super::NodePeerSnapshot,
    },
    EmbeddedService {
        id: String,
        service: super::NodeEmbeddedServiceSnapshot,
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
        snapshot: Box<NodeSnapshot>,
    },
    Event {
        schema_version: u16,
        request_id: u64,
        server_time: String,
        event: NodeEvent,
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
    pub snapshot: NodeSnapshot,
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
    pub result: NodeCommandResult,
}

/// Stable error codes for HTTP version negotiation and backend failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeHttpErrorCode {
    SchemaVersionMismatch,
    InvalidRequest,
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
            Self::SchemaVersionMismatch | Self::InvalidRequest => NodeErrorCategory::Validation,
            Self::NotFound
            | Self::OperationConflict
            | Self::OperationCancelled
            | Self::ResyncRequired
            | Self::BackendRejected => NodeErrorCategory::State,
            Self::AuthenticationRequired => NodeErrorCategory::Authentication,
            Self::PermissionDenied => NodeErrorCategory::Authorization,
            Self::CapabilityUnavailable => NodeErrorCategory::Capability,
            Self::OperationTimedOut | Self::DependencyUnavailable | Self::BackendUnavailable => {
                NodeErrorCategory::Dependency
            }
            Self::ContractViolation | Self::Internal => NodeErrorCategory::Internal,
        }
    }

    fn message_key(self) -> &'static str {
        match self {
            Self::SchemaVersionMismatch => "error.schema_version_mismatch",
            Self::InvalidRequest => "error.invalid_request",
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
            Self::SchemaVersionMismatch | Self::InvalidRequest => {
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
            | Self::BackendUnavailable => Some(("retry", "recovery.retry")),
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
            params: (*problem.params).clone(),
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
        snapshot: Box<NodeSnapshot>,
    },
    Event {
        schema_version: u16,
        event: NodeEvent,
    },
    Resync {
        schema_version: u16,
        reason: NodeWsResyncReason,
        snapshot: Box<NodeSnapshot>,
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

/// The event stream had a consistency violation that cannot be recovered by a
/// snapshot, such as the canonical store moving backwards in revision.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "节点状态 revision 回退: last_revision={last_revision}, observed_revision={observed_revision}"
)]
pub struct NodeWsSessionError {
    pub last_revision: u64,
    pub observed_revision: u64,
}

/// Per-connection cursors over the canonical event stream.
///
/// This is transport state only: the latest durable state always comes from
/// `NodeStateStore`, and no network/runtime state is copied or derived here.
pub struct NodeWsSession {
    receiver: broadcast::Receiver<NodeEvent>,
    state: NodeStateStore,
    last_sequence: Option<u64>,
    last_revision: u64,
}

impl NodeWsSession {
    pub fn new(
        receiver: broadcast::Receiver<NodeEvent>,
        state: NodeStateStore,
        initial_revision: u64,
    ) -> Self {
        Self {
            receiver,
            state,
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
        let snapshot = snapshot_for_control_client(self.state.snapshot().await);
        let required_revision = minimum_revision.unwrap_or(self.last_revision);
        if snapshot.revision < required_revision {
            return Err(NodeWsSessionError {
                last_revision: required_revision,
                observed_revision: snapshot.revision,
            });
        }
        self.last_revision = snapshot.revision;
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
}

#[derive(Clone)]
struct OperationRecord {
    operation: Operation,
    result: Option<NodeCommandResult>,
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
}

impl NodeHttpAdapter {
    pub fn new(commands: NodeCommandBus, state: NodeStateStore, events: NodeEventBus) -> Self {
        Self {
            commands,
            state,
            events,
            next_request_id: Arc::new(AtomicU64::new(1)),
            operations: Arc::new(Mutex::new(OperationRegistry::default())),
        }
    }

    fn operation(&self, id: &str) -> Option<NodeOperationSnapshot> {
        let registry = self
            .operations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        registry
            .records
            .get(id)
            .map(|record| NodeOperationSnapshot {
                operation: record.operation.clone(),
                result: record.result.clone(),
            })
    }

    /// Register and launch one mutating command. Registration and idempotency
    /// resolution happen under the same lock, so concurrent retries cannot
    /// dispatch duplicate side effects.
    async fn submit_operation(
        &self,
        request: NodeCommandRequest,
        idempotency_key: IdempotencyKey,
    ) -> Result<(NodeOperationSnapshot, bool), OperationSubmitError> {
        request.validate().map_err(OperationSubmitError::Invalid)?;
        if !request.command.kind().is_write() {
            return Err(OperationSubmitError::Internal(
                "read-only command cannot be registered as an operation".into(),
            ));
        }

        let request_digest = ContentHash::from_bytes(
            &serde_json::to_vec(&request.command)
                .map_err(|error| OperationSubmitError::Internal(error.to_string()))?,
        );
        let actor = self.local_owner_actor().await;
        let operation_id = new_operation_id()?;
        let now = unix_time_ms();
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
        };
        operation
            .validate()
            .map_err(|error| OperationSubmitError::Internal(error.to_string()))?;

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
                        },
                    );
                    let snapshot = registry
                        .records
                        .get(operation_id.as_str())
                        .map(|record| NodeOperationSnapshot {
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
                        .map(|record| NodeOperationSnapshot {
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
            tokio::spawn(async move {
                adapter.run_operation(operation_id, request).await;
            });
        }
        Ok((snapshot, created))
    }

    async fn local_owner_actor(&self) -> DomainActorRef {
        let node_id = self
            .state
            .snapshot()
            .await
            .identity
            .as_ref()
            .and_then(|identity| NodeId::parse(identity.node_id.as_str()).ok())
            .unwrap_or_else(|| NodeId::from_public_key(b"fuckipv6-uninitialized-local-owner"));
        DomainActorRef {
            node_id,
            display: "local_owner".into(),
        }
    }

    async fn run_operation(&self, operation_id: OperationId, request: NodeCommandRequest) {
        let request_id = request.request_id;
        if !self.transition_operation(&operation_id, OperationState::Running, "dispatching") {
            return;
        }
        self.progress_operation(&operation_id, "executing");
        let result = self.commands.execute(request).await;
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
            NodeOperationSnapshot {
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
            NodeOperationSnapshot {
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
            NodeOperationSnapshot {
                operation: record.operation.clone(),
                result: record.result.clone(),
            }
        };
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
            NodeOperationSnapshot {
                operation: record.operation.clone(),
                result: record.result.clone(),
            }
        };
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
                .cancel(Some(error), unix_time_ms())
                .is_err()
            {
                return;
            }
            record.result = Some(result);
            NodeOperationSnapshot {
                operation: record.operation.clone(),
                result: record.result.clone(),
            }
        };
        self.publish_operation(&snapshot.operation);
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
                post(handle_command),
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
                post(handle_v1_command),
            )
            .route(
                &format!("{NODE_HTTP_V1_API_PREFIX}/operations/{{id}}"),
                get(handle_v1_operation),
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
        request: NodeCommandRequest,
    ) -> Result<NodeCommandResult, NodeHttpAdapterError> {
        self.commands
            .execute(request)
            .await
            .map_err(NodeHttpAdapterError::from)
    }

    async fn execute_generated(
        &self,
        command: NodeCommand,
    ) -> Result<NodeCommandResult, NodeHttpAdapterError> {
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        self.execute(NodeCommandRequest::new(request_id, command))
            .await
    }

    fn allocate_request_id(&self) -> u64 {
        self.next_request_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Fetch the latest snapshot through `NodeCommand::GetSnapshot`; the
    /// adapter never synthesizes a snapshot from private state.
    pub async fn snapshot(&self) -> Result<NodeSnapshot, NodeHttpAdapterError> {
        let result = self.execute_generated(NodeCommand::GetSnapshot).await?;
        match result.outcome {
            NodeCommandOutcome::Snapshot(snapshot) => Ok(*snapshot),
            NodeCommandOutcome::Rejected(problem) => Err(NodeHttpAdapterError::Rejected(problem)),
            outcome => Err(NodeHttpAdapterError::UnexpectedOutcome(format!(
                "{outcome:?}"
            ))),
        }
    }

    /// Fetch the declarative UI contract through `NodeCommand::GetUiManifest`.
    pub async fn manifest(&self) -> Result<NodeUiManifest, NodeHttpAdapterError> {
        let result = self.execute_generated(NodeCommand::GetUiManifest).await?;
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
        NodeProblemCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
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
        NodeProblemCode::Unavailable => NodeHttpErrorCode::DependencyUnavailable,
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
        NodeCommand::CleanupResiduals { .. } => OperationKind::NodeCleanup,
        NodeCommand::CreateNetwork(_) => OperationKind::NetworkCreate,
        NodeCommand::JoinNetwork(_) => OperationKind::NetworkApply,
        NodeCommand::UpdateRouting(_) => OperationKind::TrafficRulesApply,
        NodeCommand::SetNetworkRole(_) => OperationKind::NetworkRoleUpdate,
        NodeCommand::BanClient(_) | NodeCommand::UnbanClient(_) => {
            OperationKind::ClientBlacklistUpdate
        }
        NodeCommand::UpdateNetworkAllocation(_) => OperationKind::NetworkAllocationUpdate,
        // Read-only commands never enter the operation registry. Keeping a
        // total mapping avoids a panic if a future caller violates that
        // invariant before the command is rejected by `submit_operation`.
        NodeCommand::GetSnapshot
        | NodeCommand::GetUiManifest
        | NodeCommand::GetDiagnostics
        | NodeCommand::GetBlacklist { .. }
        | NodeCommand::GetNetworkAllocation { .. }
        | NodeCommand::ProbePeer(_) => OperationKind::PolicyTest,
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
        NodeCommand::JoinNetwork(command) => command
            .network_name
            .as_deref()
            .map(network_target)
            .unwrap_or_else(node_target),
        NodeCommand::UpdateRouting(command) => network_target(&command.network_name),
        NodeCommand::SetNetworkRole(command) => network_target(&command.network_name),
        NodeCommand::BanClient(command) | NodeCommand::UnbanClient(command) => {
            network_target(&command.network_name)
        }
        NodeCommand::UpdateNetworkAllocation(command) => network_target(&command.network_name),
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
    payload: Result<Json<NodeCommandHttpRequest>, JsonRejection>,
) -> Response {
    let payload = match payload {
        Ok(Json(payload)) => payload,
        Err(error) => {
            let request_id = adapter.allocate_request_id();
            return HttpApiError::invalid_request(request_id, error.body_text()).into_response();
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

    if payload.request.command.kind().is_write() {
        let Some(idempotency_key) = payload.idempotency_key else {
            return HttpApiError::invalid_request(
                request_id,
                "写命令必须提供 idempotency_key，防止重试产生重复副作用",
            )
            .into_response();
        };
        return match adapter
            .submit_operation(payload.request, idempotency_key)
            .await
        {
            Ok((operation, _created)) => (
                StatusCode::ACCEPTED,
                Json(NodeApiCommandResponse {
                    meta: NodeApiResponseMeta::new(request_id),
                    result: None,
                    operation: Some(operation),
                }),
            )
                .into_response(),
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

    match adapter.execute(payload.request).await {
        Ok(result) => {
            let result = result_for_control_client(result);
            let status = status_for_command_result(&result);
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

    match adapter.snapshot().await {
        Ok(snapshot) => Json(NodeApiSnapshotResponse {
            meta: NodeApiResponseMeta::new(request_id),
            snapshot: snapshot_for_control_client(snapshot),
        })
        .into_response(),
        Err(error) => HttpApiError::from_adapter(request_id, error).into_response(),
    }
}

async fn handle_v1_manifest(
    State(adapter): State<NodeHttpAdapter>,
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

    match adapter.manifest().await {
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
        Some(operation) => Json(NodeApiOperationResponse {
            meta: NodeApiResponseMeta::new(request_id),
            operation,
        })
        .into_response(),
        None => HttpApiError::not_found(request_id, format!("operation not found: {id}"))
            .into_response(),
    }
}

async fn handle_v1_object(
    State(adapter): State<NodeHttpAdapter>,
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

    let snapshot = match adapter.snapshot().await {
        Ok(snapshot) => snapshot_for_control_client(snapshot),
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
                service,
            }),
        "network_configuration" => snapshot
            .configuration
            .networks
            .into_iter()
            .find(|configuration| configuration.network_name == id)
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

    let snapshot = match adapter.snapshot().await {
        Ok(snapshot) => snapshot_for_control_client(snapshot),
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
    Json(payload): Json<NodeCommandHttpRequest>,
) -> Response {
    if let Err(error) = require_schema_version_value(payload.schema_version) {
        return error.into_response();
    }

    match adapter.execute(payload.request).await {
        Ok(result) => {
            let result = result_for_control_client(result);
            let status = status_for_command_result(&result);
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
    Query(query): Query<NodeHttpQuery>,
) -> Result<Json<NodeSnapshotResponse>, HttpAdapterError> {
    require_schema_version(&query)?;
    let snapshot =
        snapshot_for_control_client(adapter.snapshot().await.map_err(HttpAdapterError::from)?);
    Ok(Json(NodeSnapshotResponse {
        schema_version: NODE_HTTP_SCHEMA_VERSION,
        snapshot,
    }))
}

async fn handle_manifest(
    State(adapter): State<NodeHttpAdapter>,
    Query(query): Query<NodeHttpQuery>,
) -> Result<Json<NodeManifestResponse>, HttpAdapterError> {
    require_schema_version(&query)?;
    let manifest = adapter.manifest().await.map_err(HttpAdapterError::from)?;
    Ok(Json(NodeManifestResponse {
        schema_version: NODE_HTTP_SCHEMA_VERSION,
        manifest,
    }))
}

async fn handle_events(
    State(adapter): State<NodeHttpAdapter>,
    Query(query): Query<NodeHttpQuery>,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    if let Err(error) = require_schema_version(&query) {
        return error.into_response();
    }
    let Ok(ws) = ws else {
        return HttpAdapterError::upgrade_required().into_response();
    };
    ws.on_upgrade(move |socket| run_event_socket(socket, adapter))
}

async fn run_event_socket(mut socket: WebSocket, adapter: NodeHttpAdapter) {
    // Subscribe before fetching the snapshot so no event is lost in the gap.
    let receiver = adapter.events.subscribe();
    let snapshot = match adapter.snapshot().await {
        Ok(snapshot) => snapshot_for_control_client(snapshot),
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

    let mut session = NodeWsSession::new(receiver, adapter.state.clone(), snapshot.revision);
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

/// A successful HTTP/WebSocket response is proof that this adapter can reach
/// the authoritative node state. The observation is applied only to the
/// outbound projection and never mutates the canonical store or revision.
fn snapshot_for_control_client(mut snapshot: NodeSnapshot) -> NodeSnapshot {
    snapshot.control_health = NodeControlHealth::Available;
    snapshot
}

fn result_for_control_client(mut result: NodeCommandResult) -> NodeCommandResult {
    if let NodeCommandOutcome::Snapshot(snapshot) = &mut result.outcome {
        snapshot.control_health = NodeControlHealth::Available;
    }
    result
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
    use fuckipv6_common::domain::{IdempotencyKey, ObjectKind, OperationState};
    use futures::{SinkExt, StreamExt};
    use tokio::{net::TcpListener, sync::broadcast, time::timeout};
    use tokio_tungstenite::{connect_async, tungstenite::Message as ClientMessage};
    use tower::ServiceExt;

    use super::*;
    use crate::node::{
        node_ui_manifest, NodeCommandFuture, NodeCommandHandler, NodeCommandResult,
        NodeNetworkSnapshot, NodeNetworkStatus, NodeProblemCode, NodeTrafficSnapshot,
    };

    #[derive(Clone)]
    enum StubWriteBehavior {
        Completed,
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
                        if !write_delay.is_zero() {
                            tokio::time::sleep(write_delay).await;
                        }
                        match write_behavior {
                            StubWriteBehavior::Completed => NodeCommandResult::completed(
                                request_id,
                                NodeCommandOutcome::Completed {
                                    message: "stub write completed".into(),
                                },
                            ),
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
            .router()
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
            .router()
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

    async fn fetch_v1_operation(adapter: &NodeHttpAdapter, operation_id: &str) -> Response {
        adapter
            .clone()
            .router()
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

    #[tokio::test]
    async fn snapshot_and_manifest_success_are_versioned_and_forwarded() {
        let (adapter, state, _events) = adapter_fixture(node_ui_manifest());
        state.update(|snapshot| snapshot.revision = 6).await;

        let snapshot_response = adapter
            .clone()
            .router()
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
            .router()
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
        assert_eq!(manifest.manifest.pages.len(), 11);
    }

    #[tokio::test]
    async fn v1_snapshot_and_manifest_carry_meta_and_authoritative_payload() {
        let (adapter, state, _events) = adapter_fixture(node_ui_manifest());
        state.update(|snapshot| snapshot.revision = 8).await;

        let snapshot_response = adapter
            .clone()
            .router()
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
            .router()
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
        assert_eq!(manifest.manifest.pages.len(), 11);
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
        assert!(matches!(result.outcome, NodeCommandOutcome::Snapshot(_)));
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
            NodeCommandOutcome::Completed { .. }
        ));
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
            .router()
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
            .router()
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
                if id == "home" && network.name == "home" && network.uptime_secs == 12
        ));

        let missing = adapter
            .router()
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
            .router()
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
                event: NodeEvent {
                    sequence: 1,
                    snapshot_revision: 1,
                    kind: NodeEventKind::StateChanged { revision: 1 },
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
            .router()
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
                event: NodeEvent {
                    sequence: 2,
                    snapshot_revision: 1,
                    kind: NodeEventKind::CommandCompleted { success: true, .. },
                },
                ..
            }
        ));
        let (id, resumed) = next_sse_event(&mut body).await;
        assert_eq!(id.as_deref(), Some("3:2"));
        assert!(matches!(
            resumed,
            NodeApiEvent::Event {
                event: NodeEvent {
                    sequence: 3,
                    snapshot_revision: 2,
                    kind: NodeEventKind::StateChanged { revision: 2 },
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
            .router()
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
            .router()
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
                .router()
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
                    event: NodeEvent {
                        sequence: 1,
                        snapshot_revision: 0,
                        kind: NodeEventKind::CommandCompleted { success: true, .. },
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
                .router()
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
            NodeCommandOutcome::Snapshot(snapshot) if snapshot.revision == 0
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
            NodeCommandOutcome::Rejected(problem)
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
                .router()
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
            .manifest()
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
            .manifest()
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
            axum::serve(listener, adapter.router())
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
                        event: NodeEvent {
                            kind: NodeEventKind::StateChanged { .. },
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
        assert_eq!(event.kind, NodeEventKind::StateChanged { revision: 1 });

        socket
            .send(ClientMessage::Close(None))
            .await
            .expect("close websocket");
        server.abort();
    }

    #[tokio::test]
    async fn duplicate_and_stale_revisions_are_suppressed() {
        let events = NodeEventBus::new(16);
        let state = NodeStateStore::with_events(events.clone());
        let initial = state.snapshot().await;
        let mut session = NodeWsSession::new(events.subscribe(), state.clone(), initial.revision);

        state.update(|_| {}).await;
        let first = session
            .next_message()
            .await
            .expect("read first revision")
            .expect("first revision message");
        assert!(matches!(
            first,
            NodeWsMessage::Event {
                event: NodeEvent {
                    kind: NodeEventKind::StateChanged { revision: 1 },
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
                event: NodeEvent {
                    kind: NodeEventKind::StateChanged { revision: 2 },
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
        let mut session = NodeWsSession::new(events.subscribe(), state.clone(), initial.revision);

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
        let mut session = NodeWsSession::new(events.subscribe(), state.clone(), initial.revision);

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
        let mut session = NodeWsSession::new(receiver, state, 0);

        assert_eq!(session.next_message().await.expect("closed stream"), None);
    }
}
