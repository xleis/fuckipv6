//! Reusable HTTP and WebSocket adapter for the transport-neutral node API.
//!
//! The adapter only projects the shared command bus, state store and event bus
//! onto HTTP/WebSocket. It never owns a node or network state machine.

use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{
        rejection::{JsonRejection, QueryRejection},
        ws::{rejection::WebSocketUpgradeRejection, Message, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    http::StatusCode,
    response::{
        sse::{Event as SseEvent, KeepAlive, Sse},
        IntoResponse, Json, Response,
    },
    routing::{get, post},
    Router,
};
use chrono::{SecondsFormat, Utc};
use futures::SinkExt as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use super::{
    NodeCommand, NodeCommandBus, NodeCommandBusError, NodeCommandOutcome, NodeCommandRequest,
    NodeCommandResult, NodeControlHealth, NodeEvent, NodeEventBus, NodeEventKind, NodeProblem,
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
    pub result: NodeCommandResult,
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
    pub code: NodeHttpErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_schema_version: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supported_schema_version: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<NodeProblem>,
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
        requested_after: Option<u64>,
        latest_sequence: u64,
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
    ResyncRequired,
    BackendRejected,
    BackendUnavailable,
    ContractViolation,
}

/// Versioned HTTP error DTO. It is also used for WebSocket version mismatches
/// so clients receive a machine-readable failure before an upgrade.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeHttpErrorResponse {
    pub schema_version: u16,
    pub code: NodeHttpErrorCode,
    pub message: String,
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
        code: NodeHttpErrorCode,
        message: String,
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

/// HTTP/WebSocket projection over the shared node bus and state store.
///
/// The only mutable data held here is the request-id allocator for bus
/// correlation. All operational and durable state remains in the shared node
/// services passed to [`NodeHttpAdapter::new`].
#[derive(Clone)]
pub struct NodeHttpAdapter {
    commands: NodeCommandBus,
    state: NodeStateStore,
    events: NodeEventBus,
    next_request_id: Arc<AtomicU64>,
}

impl NodeHttpAdapter {
    pub fn new(commands: NodeCommandBus, state: NodeStateStore, events: NodeEventBus) -> Self {
        Self {
            commands,
            state,
            events,
            next_request_id: Arc::new(AtomicU64::new(1)),
        }
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
        NodeProblemCode::Conflict | NodeProblemCode::Cancelled => StatusCode::CONFLICT,
        NodeProblemCode::Unsupported => StatusCode::UNPROCESSABLE_ENTITY,
        NodeProblemCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        NodeProblemCode::PermissionDenied => StatusCode::FORBIDDEN,
        NodeProblemCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[derive(Debug, Default, Deserialize)]
struct NodeApiQuery {
    schema_version: Option<u16>,
    after: Option<u64>,
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

    match adapter.execute(payload.request).await {
        Ok(result) => {
            let result = result_for_control_client(result);
            let status = status_for_command_result(&result);
            (
                status,
                Json(NodeApiCommandResponse {
                    meta: NodeApiResponseMeta::new(request_id),
                    result,
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

    // Operation lifecycle and persistence are implemented by W4-02. Until an
    // operation registry exists, returning a stable 404 is the only truthful
    // response; a placeholder "unavailable" operation would hide a contract
    // that the backend cannot yet satisfy.
    HttpApiError::not_found(request_id, format!("operation not found: {id}")).into_response()
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
    request_id: u64,
    events: NodeEventBus,
}

async fn handle_v1_events(
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

    // Subscribe before reading the snapshot so events published during the
    // read are queued for this stream. Full replay/reconnect semantics remain
    // W4-03; a cursor that does not match the retained boundary is rejected
    // explicitly instead of silently skipping an event gap.
    let (receiver, boundary) = adapter.events.subscribe_with_sequence();
    if query.after.is_some_and(|after| after != boundary) {
        return HttpApiError::resync_required(request_id, query.after, boundary).into_response();
    }

    let snapshot = match adapter.snapshot().await {
        Ok(snapshot) => snapshot_for_control_client(snapshot),
        Err(error) => return HttpApiError::from_adapter(request_id, error).into_response(),
    };
    let initial = NodeApiEvent::Snapshot {
        schema_version: NODE_HTTP_SCHEMA_VERSION,
        request_id,
        server_time: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        snapshot: Box::new(snapshot),
    };
    let state = NodeApiEventStreamState {
        receiver,
        initial: Some(initial),
        request_id,
        events: adapter.events,
    };
    let stream = futures::stream::unfold(state, move |mut state| async move {
        if let Some(initial) = state.initial.take() {
            return Some((
                Ok::<_, Infallible>(sse_event(initial, Some(boundary))),
                state,
            ));
        }

        match state.receiver.recv().await {
            Ok(event) => {
                let sequence = event.sequence;
                let message = NodeApiEvent::Event {
                    schema_version: NODE_HTTP_SCHEMA_VERSION,
                    request_id: state.request_id,
                    server_time: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                    event,
                };
                Some((Ok(sse_event(message, Some(sequence))), state))
            }
            Err(broadcast::error::RecvError::Lagged(_)) => {
                let message = NodeApiEvent::ResyncRequired {
                    schema_version: NODE_HTTP_SCHEMA_VERSION,
                    request_id: state.request_id,
                    server_time: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                    requested_after: None,
                    latest_sequence: state.events.latest_sequence(),
                };
                Some((Ok(sse_event(message, None)), state))
            }
            Err(broadcast::error::RecvError::Closed) => None,
        }
    });
    Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response()
}

fn sse_event(message: NodeApiEvent, id: Option<u64>) -> SseEvent {
    let event_name = match &message {
        NodeApiEvent::Snapshot { .. } => "snapshot",
        NodeApiEvent::Event { .. } => "node",
        NodeApiEvent::ResyncRequired { .. } => "resync_required",
    };
    let payload = serde_json::to_string(&message).expect("node API event is serializable");
    let event = SseEvent::default().event(event_name).data(payload);
    match id {
        Some(id) => event.id(id.to_string()),
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
                code: NodeHttpErrorCode::BackendUnavailable,
                message: error.to_string(),
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
                            code: NodeHttpErrorCode::ContractViolation,
                            message: error.to_string(),
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
    body: NodeHttpErrorResponse,
}

impl HttpAdapterError {
    fn schema_version_mismatch(requested: u16, supported: u16) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            body: NodeHttpErrorResponse {
                schema_version: NODE_HTTP_SCHEMA_VERSION,
                code: NodeHttpErrorCode::SchemaVersionMismatch,
                message: format!(
                    "节点 HTTP schema 版本不兼容: requested={requested}, supported={supported}"
                ),
                requested_schema_version: Some(requested),
                supported_schema_version: Some(supported),
                problem: None,
            },
        }
    }

    fn upgrade_required() -> Self {
        Self {
            status: StatusCode::UPGRADE_REQUIRED,
            body: NodeHttpErrorResponse {
                schema_version: NODE_HTTP_SCHEMA_VERSION,
                code: NodeHttpErrorCode::ContractViolation,
                message: "事件流端点需要 WebSocket Upgrade".into(),
                requested_schema_version: None,
                supported_schema_version: Some(NODE_HTTP_SCHEMA_VERSION),
                problem: None,
            },
        }
    }

    fn from_adapter(error: NodeHttpAdapterError) -> Self {
        let (status, code, problem) = match &error {
            NodeHttpAdapterError::SchemaVersionMismatch { .. } => (
                StatusCode::BAD_REQUEST,
                NodeHttpErrorCode::SchemaVersionMismatch,
                None,
            ),
            NodeHttpAdapterError::Command(NodeCommandBusError::Invalid(problem)) => (
                StatusCode::BAD_REQUEST,
                NodeHttpErrorCode::InvalidRequest,
                Some(problem.clone()),
            ),
            NodeHttpAdapterError::Command(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                NodeHttpErrorCode::BackendUnavailable,
                None,
            ),
            NodeHttpAdapterError::Rejected(problem) => (
                StatusCode::SERVICE_UNAVAILABLE,
                NodeHttpErrorCode::BackendRejected,
                Some(problem.clone()),
            ),
            NodeHttpAdapterError::UnexpectedOutcome(_)
            | NodeHttpAdapterError::ManifestSchemaMismatch { .. }
            | NodeHttpAdapterError::InvalidManifest { .. } => (
                StatusCode::BAD_GATEWAY,
                NodeHttpErrorCode::ContractViolation,
                None,
            ),
        };
        Self {
            status,
            body: NodeHttpErrorResponse {
                schema_version: NODE_HTTP_SCHEMA_VERSION,
                code,
                message: error.to_string(),
                requested_schema_version: None,
                supported_schema_version: Some(NODE_HTTP_SCHEMA_VERSION),
                problem,
            },
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
    fn invalid_request(request_id: u64, message: impl Into<String>) -> Self {
        Self::new(
            request_id,
            StatusCode::BAD_REQUEST,
            NodeHttpErrorCode::InvalidRequest,
            message,
        )
    }

    fn not_found(request_id: u64, message: impl Into<String>) -> Self {
        Self::new(
            request_id,
            StatusCode::NOT_FOUND,
            NodeHttpErrorCode::NotFound,
            message,
        )
    }

    fn resync_required(request_id: u64, requested_after: Option<u64>, latest: u64) -> Self {
        let requested = requested_after
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".into());
        let mut error = Self::new(
            request_id,
            StatusCode::CONFLICT,
            NodeHttpErrorCode::ResyncRequired,
            format!(
                "事件 cursor 已失效: requested_after={requested}, latest_sequence={latest}; 请重新读取快照"
            ),
        );
        error.body.requested_schema_version = None;
        error.body.supported_schema_version = Some(NODE_HTTP_SCHEMA_VERSION);
        error
    }

    fn schema_version_mismatch(request_id: u64, requested: u16, supported: u16) -> Self {
        let mut error = Self::new(
            request_id,
            StatusCode::BAD_REQUEST,
            NodeHttpErrorCode::SchemaVersionMismatch,
            format!("节点 HTTP schema 版本不兼容: requested={requested}, supported={supported}"),
        );
        error.body.requested_schema_version = Some(requested);
        error.body.supported_schema_version = Some(supported);
        error
    }

    fn from_adapter(request_id: u64, error: NodeHttpAdapterError) -> Self {
        let (status, code, problem) = match &error {
            NodeHttpAdapterError::SchemaVersionMismatch { .. } => (
                StatusCode::BAD_REQUEST,
                NodeHttpErrorCode::SchemaVersionMismatch,
                None,
            ),
            NodeHttpAdapterError::Command(NodeCommandBusError::Invalid(problem)) => (
                StatusCode::BAD_REQUEST,
                NodeHttpErrorCode::InvalidRequest,
                Some(problem.clone()),
            ),
            NodeHttpAdapterError::Command(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                NodeHttpErrorCode::BackendUnavailable,
                None,
            ),
            NodeHttpAdapterError::Rejected(problem) => (
                StatusCode::SERVICE_UNAVAILABLE,
                NodeHttpErrorCode::BackendRejected,
                Some(problem.clone()),
            ),
            NodeHttpAdapterError::UnexpectedOutcome(_)
            | NodeHttpAdapterError::ManifestSchemaMismatch { .. }
            | NodeHttpAdapterError::InvalidManifest { .. } => (
                StatusCode::BAD_GATEWAY,
                NodeHttpErrorCode::ContractViolation,
                None,
            ),
        };
        let mut error = Self::new(request_id, status, code, error.to_string());
        error.body.supported_schema_version = Some(NODE_HTTP_SCHEMA_VERSION);
        error.body.problem = problem;
        error
    }

    fn new(
        request_id: u64,
        status: StatusCode,
        code: NodeHttpErrorCode,
        message: impl Into<String>,
    ) -> Self {
        Self {
            status,
            body: NodeApiErrorResponse {
                meta: NodeApiResponseMeta::new(request_id),
                code,
                message: message.into(),
                requested_schema_version: None,
                supported_schema_version: None,
                problem: None,
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
    use std::time::Duration;

    use axum::{
        body::{to_bytes, Body},
        http::{header, Method, Request, StatusCode},
    };
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
    struct StubHandler {
        state: NodeStateStore,
        manifest: NodeUiManifest,
    }

    impl NodeCommandHandler for StubHandler {
        fn handle(&self, request: NodeCommandRequest) -> NodeCommandFuture {
            let state = self.state.clone();
            let manifest = self.manifest.clone();
            Box::pin(async move {
                let request_id = request.request_id;
                match request.command {
                    NodeCommand::GetSnapshot => {
                        NodeCommandResult::snapshot(request_id, state.snapshot().await)
                    }
                    NodeCommand::GetUiManifest => {
                        NodeCommandResult::ui_manifest(request_id, manifest)
                    }
                    _ => NodeCommandResult::rejected(
                        request_id,
                        NodeProblem::new(
                            NodeProblemCode::Unavailable,
                            "stub only implements read commands",
                        ),
                    ),
                }
            })
        }
    }

    fn adapter_fixture(
        manifest: NodeUiManifest,
    ) -> (NodeHttpAdapter, NodeStateStore, NodeEventBus) {
        let events = NodeEventBus::new(32);
        let state = NodeStateStore::with_events(events.clone());
        let commands = NodeCommandBus::spawn(
            Arc::new(StubHandler {
                state: state.clone(),
                manifest,
            }),
            events.clone(),
            8,
        );
        (
            NodeHttpAdapter::new(commands, state.clone(), events.clone()),
            state,
            events,
        )
    }

    async fn json_body<T: serde::de::DeserializeOwned>(response: Response) -> T {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read response body");
        serde_json::from_slice(&bytes).expect("decode response body")
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
        };

        let response = post_v1_command(adapter, &payload).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body: NodeApiCommandResponse = json_body(response).await;
        assert_eq!(body.meta.schema_version, NODE_HTTP_SCHEMA_VERSION);
        assert_eq!(body.meta.request_id, 51);
        chrono::DateTime::parse_from_rfc3339(&body.meta.server_time)
            .expect("command server time is RFC3339");
        assert_eq!(body.result.request_id, 51);
        assert!(matches!(
            body.result.outcome,
            NodeCommandOutcome::Snapshot(_)
        ));
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
        assert_eq!(body.code, NodeHttpErrorCode::NotFound);
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
        assert_eq!(missing.code, NodeHttpErrorCode::NotFound);
    }

    #[tokio::test]
    async fn v1_events_stream_starts_with_snapshot_and_rejects_stale_cursor() {
        let (adapter, _state, events) = adapter_fixture(node_ui_manifest());
        let response = adapter
            .clone()
            .router()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/events?schema_version=1")
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
        let chunk = timeout(Duration::from_secs(2), body.next())
            .await
            .expect("initial SSE event timeout")
            .expect("initial SSE event")
            .expect("valid SSE chunk");
        let text = String::from_utf8(chunk.to_vec()).expect("SSE chunk is UTF-8");
        let data = text
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .expect("SSE data line");
        let initial: NodeApiEvent = serde_json::from_str(data).expect("decode initial SSE event");
        assert!(
            matches!(initial, NodeApiEvent::Snapshot { snapshot, .. } if snapshot.revision == 0)
        );

        events.publish(NodeEventKind::StateChanged { revision: 1 });
        let stale = adapter
            .router()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/events?schema_version=1&after=0")
                    .body(Body::empty())
                    .expect("stale cursor request"),
            )
            .await
            .expect("stale cursor response");
        assert_eq!(stale.status(), StatusCode::CONFLICT);
        let stale: NodeApiErrorResponse = json_body(stale).await;
        assert_eq!(stale.code, NodeHttpErrorCode::ResyncRequired);
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
            assert_eq!(body.code, NodeHttpErrorCode::SchemaVersionMismatch);
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
        };

        let response = post_command(adapter, &payload).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: NodeHttpErrorResponse = json_body(response).await;
        assert_eq!(body.code, NodeHttpErrorCode::InvalidRequest);
        let problem = body.problem.expect("validation problem in response");
        assert_eq!(problem.code, NodeProblemCode::InvalidRequest);
    }

    #[tokio::test]
    async fn command_endpoint_preserves_rejected_unavailable_result() {
        let (adapter, _state, _events) = adapter_fixture(node_ui_manifest());
        let payload = NodeCommandHttpRequest {
            schema_version: NODE_HTTP_SCHEMA_VERSION,
            request: NodeCommandRequest::new(43, NodeCommand::Shutdown),
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
        };

        let response = post_command(adapter, &payload).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: NodeHttpErrorResponse = json_body(response).await;
        assert_eq!(body.code, NodeHttpErrorCode::SchemaVersionMismatch);
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
            assert_eq!(body.code, NodeHttpErrorCode::SchemaVersionMismatch);
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
