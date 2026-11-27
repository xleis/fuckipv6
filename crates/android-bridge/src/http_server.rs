//! Loopback HTTP control and File REST server for the Android embedded node.
//!
//! Binds strictly to `127.0.0.1:0`, authenticates every request against a
//! per-launch random 256-bit bearer token using constant-time equality, and
//! mounts the exact shared [`NodeHttpAdapter`] routes alongside `/api/v1/file/*`
//! and Android telemetry endpoints.

use std::collections::{BTreeMap, HashMap};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{Path as AxumPath, Query, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use fuckipv6_client::embedded::EmbeddedNodeRuntime;
use fuckipv6_client::file_share::{
    FileTransferDirection, ListFileDirectoryRequest, StartFileTransferRequest,
    DEFAULT_FILE_DIRECTORY_LIST_LIMIT, DEFAULT_FILE_TRANSFER_CHUNK_SIZE, MAX_DIRECTORY_LIST_LIMIT,
};
use fuckipv6_client::node::{NodeCommandContext, NodeHttpAdapter, NodeStateStore};
use fuckipv6_common::domain::{FileShareId, OperationId, SharedPath, TransferId, TransferState};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::{watch, OwnedSemaphorePermit, RwLock, Semaphore};

use crate::platform::AndroidEmbeddedPlatform;
use crate::tun_pump::TunnelCountersSnapshot;

const MAX_ANDROID_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ANDROID_TRANSFER_SLOTS: usize = 4;
const MAX_ANDROID_DOWNLOAD_TOKENS: usize = 16;
const DOWNLOAD_TOKEN_TTL_MS: u64 = 15 * 60 * 1_000;

#[derive(Clone)]
struct AuthState {
    expected_token: Arc<String>,
    node_state: NodeStateStore,
}

#[derive(Clone)]
struct AndroidFileHttpState {
    runtime: Arc<RwLock<Option<EmbeddedNodeRuntime>>>,
    platform: Arc<AndroidEmbeddedPlatform>,
    started_at: Instant,
    transfer_slots: Arc<Semaphore>,
    downloads: Arc<Mutex<HashMap<String, PendingDownload>>>,
}

struct PendingDownload {
    token: String,
    transfer_id: String,
    operation_id: String,
    file_path: PathBuf,
    expires_at_unix_ms: u64,
    _temp_dir: TempDir,
    _permit: OwnedSemaphorePermit,
}

#[derive(Debug, Clone, Serialize)]
pub struct AndroidTelemetry {
    pub uptime_secs: u64,
    pub active_tunnel_sessions: usize,
    pub counters: TunnelCountersSnapshot,
}

#[derive(Debug, Serialize)]
struct FilePeerTargetDto {
    peer: String,
    network_id: String,
    network_name: String,
    share_id: String,
}

#[derive(Debug, Deserialize)]
struct DirectoryQuery {
    peer: String,
    #[serde(default)]
    network_id: Option<String>,
    share_id: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UploadQuery {
    peer: String,
    #[serde(default)]
    network_id: Option<String>,
    share_id: String,
    path: String,
    #[serde(default)]
    chunk_size: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct DownloadRequestBody {
    peer: String,
    #[serde(default)]
    network_id: Option<String>,
    share_id: String,
    path: String,
    #[serde(default)]
    chunk_size: Option<u64>,
}

#[derive(Debug, Serialize)]
struct StartedUploadDto {
    transfer_id: String,
    operation_id: String,
    size_bytes: u64,
}

#[derive(Debug, Serialize)]
struct PendingDownloadDto {
    token: String,
    transfer_id: String,
    operation_id: String,
    expires_at_unix_ms: u64,
}

#[derive(Debug, Serialize)]
struct ApiErrorDto {
    error: String,
}

pub struct LoopbackHttpServer {
    pub addr: SocketAddr,
    pub token: String,
    shutdown: watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl LoopbackHttpServer {
    pub async fn start(
        runtime: Arc<RwLock<Option<EmbeddedNodeRuntime>>>,
        platform: Arc<AndroidEmbeddedPlatform>,
        started_at: Instant,
    ) -> std::io::Result<Self> {
        let token = generate_token()?;
        let (commands, state, events) = {
            let guard = runtime.read().await;
            let node = guard
                .as_ref()
                .ok_or_else(|| std::io::Error::other("embedded node not running"))?;
            (node.commands(), node.state(), node.event_bus())
        };

        let adapter = NodeHttpAdapter::new(commands, state.clone(), events);
        let auth_state = AuthState {
            expected_token: Arc::new(token.clone()),
            node_state: state,
        };
        let android_state = AndroidFileHttpState {
            runtime,
            platform,
            started_at,
            transfer_slots: Arc::new(Semaphore::new(MAX_ANDROID_TRANSFER_SLOTS)),
            downloads: Arc::new(Mutex::new(HashMap::new())),
        };

        let file_and_android_routes = Router::new()
            .route("/api/v1/file/targets", get(handle_targets))
            .route("/api/v1/file/directory", get(handle_directory))
            .route("/api/v1/file/transfers", get(handle_transfers))
            .route("/api/v1/file/uploads", post(handle_upload))
            .route("/api/v1/file/downloads", post(handle_start_download))
            .route(
                "/api/v1/file/downloads/{token}",
                get(handle_download_stream),
            )
            .route(
                "/api/v1/file/transfers/{transfer_id}/{operation_id}/pause",
                post(handle_pause),
            )
            .route(
                "/api/v1/file/transfers/{transfer_id}/{operation_id}/resume",
                post(handle_resume),
            )
            .route(
                "/api/v1/file/transfers/{transfer_id}/{operation_id}",
                delete(handle_cancel),
            )
            .route("/api/v1/android/telemetry", get(handle_telemetry))
            .route(
                "/api/v1/android/network-changed",
                post(handle_network_changed),
            )
            .with_state(android_state);

        let router = adapter
            .router()
            .merge(file_and_android_routes)
            .route_layer(middleware::from_fn_with_state(auth_state, require_bearer));

        let listener =
            TcpListener::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))).await?;
        let addr = listener.local_addr()?;
        let (shutdown, mut shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(async move {
            let server = axum::serve(listener, router).with_graceful_shutdown(async move {
                let _ = shutdown_rx.wait_for(|done| *done).await;
            });
            if let Err(error) = server.await {
                tracing::warn!(%error, "Android loopback HTTP server exited with error");
            }
        });

        Ok(Self {
            addr,
            token,
            shutdown,
            task: Some(task),
        })
    }

    pub fn api_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub async fn stop(mut self) {
        self.shutdown.send_replace(true);
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(3), task).await;
        }
    }
}

async fn require_bearer(
    State(auth): State<AuthState>,
    mut request: Request,
    next: Next,
) -> Response {
    let header_value = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    let Some(provided) = header_value else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(ApiErrorDto {
                error: "missing bearer token".to_owned(),
            }),
        )
            .into_response();
    };
    if !constant_time_token_eq(provided, &auth.expected_token) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(ApiErrorDto {
                error: "invalid bearer token".to_owned(),
            }),
        )
            .into_response();
    }

    let snapshot = auth.node_state.snapshot().await;
    let context = NodeCommandContext::local_owner_for_snapshot(&snapshot, "android-loopback");
    request.extensions_mut().insert(context);
    next.run(request).await
}

pub fn constant_time_token_eq(provided: &str, expected: &str) -> bool {
    let a = provided.as_bytes();
    let b = expected.as_bytes();
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

async fn handle_telemetry(State(state): State<AndroidFileHttpState>) -> Json<AndroidTelemetry> {
    Json(AndroidTelemetry {
        uptime_secs: state.started_at.elapsed().as_secs(),
        active_tunnel_sessions: state.platform.active_session_count(),
        counters: state.platform.counters_snapshot(),
    })
}

async fn handle_network_changed(
    State(state): State<AndroidFileHttpState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiErrorDto>)> {
    let guard = state.runtime.read().await;
    let node = guard.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiErrorDto {
                error: "embedded node stopped".to_owned(),
            }),
        )
    })?;
    node.network_path_changed().await.map_err(|problem| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiErrorDto {
                error: problem.message,
            }),
        )
    })?;
    Ok(Json(serde_json::json!({ "status": "ok" })))
}

async fn handle_targets(
    State(state): State<AndroidFileHttpState>,
) -> Result<Json<Vec<FilePeerTargetDto>>, (StatusCode, Json<ApiErrorDto>)> {
    let guard = state.runtime.read().await;
    let node = guard.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiErrorDto {
                error: "embedded node stopped".to_owned(),
            }),
        )
    })?;
    let snapshot = node.state().snapshot().await;
    let mut network_ids = BTreeMap::new();
    for membership in &snapshot.memberships {
        if !membership.network_id.trim().is_empty() && !membership.network_name.trim().is_empty() {
            network_ids.insert(
                membership.network_name.as_str(),
                membership.network_id.as_str(),
            );
        }
    }
    let mut targets = BTreeMap::new();
    for peer in &snapshot.peers {
        if !peer.online || peer.peer_id.trim().is_empty() {
            continue;
        }
        let Some(network_id) = network_ids.get(peer.network_name.as_str()).copied() else {
            continue;
        };
        let share_id = FileShareId::from_public_key(
            format!("file-share:{}:{}", peer.peer_id, network_id).as_bytes(),
        );
        targets.insert(
            (
                peer.network_name.clone(),
                peer.peer_id.clone(),
                network_id.to_owned(),
            ),
            FilePeerTargetDto {
                peer: peer.peer_id.clone(),
                network_id: network_id.to_owned(),
                network_name: peer.network_name.clone(),
                share_id: share_id.as_str().to_owned(),
            },
        );
    }
    Ok(Json(targets.into_values().collect()))
}

async fn handle_directory(
    State(state): State<AndroidFileHttpState>,
    Query(query): Query<DirectoryQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiErrorDto>)> {
    let limit = query
        .limit
        .unwrap_or(DEFAULT_FILE_DIRECTORY_LIST_LIMIT)
        .clamp(1, MAX_DIRECTORY_LIST_LIMIT);
    let request = ListFileDirectoryRequest {
        peer: query.peer,
        network_id: query.network_id,
        share_id: query.share_id,
        path: query.path,
        limit,
        cursor: query.cursor,
    };
    let guard = state.runtime.read().await;
    let node = guard.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiErrorDto {
                error: "embedded node stopped".to_owned(),
            }),
        )
    })?;
    let listing = node.list_file_directory(request).await.map_err(|problem| {
        (
            StatusCode::BAD_GATEWAY,
            Json(ApiErrorDto {
                error: problem.message,
            }),
        )
    })?;
    let json = serde_json::to_value(listing).map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiErrorDto {
                error: error.to_string(),
            }),
        )
    })?;
    Ok(Json(json))
}

async fn handle_transfers(
    State(state): State<AndroidFileHttpState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiErrorDto>)> {
    let guard = state.runtime.read().await;
    let node = guard.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiErrorDto {
                error: "embedded node stopped".to_owned(),
            }),
        )
    })?;
    let list = node.list_file_transfers().await.unwrap_or_default();
    let json = serde_json::to_value(list).map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiErrorDto {
                error: error.to_string(),
            }),
        )
    })?;
    Ok(Json(json))
}

async fn handle_upload(
    State(state): State<AndroidFileHttpState>,
    Query(query): Query<UploadQuery>,
    body: Body,
) -> Result<(StatusCode, Json<StartedUploadDto>), (StatusCode, Json<ApiErrorDto>)> {
    if SharedPath::parse(&query.path).is_err() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(ApiErrorDto {
                error: "invalid portable upload path".to_owned(),
            }),
        ));
    }
    let _permit = state
        .transfer_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            (
                StatusCode::TOO_MANY_REQUESTS,
                Json(ApiErrorDto {
                    error: "android file transfer slots exhausted".to_owned(),
                }),
            )
        })?;

    let temp_dir = TempDir::new().map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiErrorDto {
                error: format!("failed to allocate upload staging dir: {error}"),
            }),
        )
    })?;
    let staged_path = temp_dir.path().join("upload.bin");
    let mut file = tokio::fs::File::create(&staged_path)
        .await
        .map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiErrorDto {
                    error: format!("failed to create upload staging file: {error}"),
                }),
            )
        })?;

    let mut total_bytes: u64 = 0;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                Json(ApiErrorDto {
                    error: format!("failed reading upload stream: {error}"),
                }),
            )
        })?;
        total_bytes = total_bytes.saturating_add(bytes.len() as u64);
        if total_bytes > MAX_ANDROID_FILE_BYTES {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(ApiErrorDto {
                    error: "upload exceeds Android staging limit".to_owned(),
                }),
            ));
        }
        file.write_all(&bytes).await.map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiErrorDto {
                    error: format!("failed writing upload staging file: {error}"),
                }),
            )
        })?;
    }
    file.flush().await.map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiErrorDto {
                error: format!("failed flushing upload staging file: {error}"),
            }),
        )
    })?;
    drop(file);

    let (transfer_id_gen, operation_id_gen) = random_transfer_ids().map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiErrorDto {
                error: format!("failed to generate transfer ids: {error}"),
            }),
        )
    })?;
    let transfer_req = StartFileTransferRequest {
        peer: query.peer,
        network_id: query.network_id,
        share_id: query.share_id,
        transfer_id: transfer_id_gen.as_str().to_owned(),
        operation_id: operation_id_gen.as_str().to_owned(),
        path: query.path,
        local_path: staged_path.to_string_lossy().into_owned(),
        direction: FileTransferDirection::Upload,
        chunk_size: query.chunk_size.unwrap_or(DEFAULT_FILE_TRANSFER_CHUNK_SIZE),
    };
    let handle = {
        let guard = state.runtime.read().await;
        let node = guard.as_ref().ok_or_else(|| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ApiErrorDto {
                    error: "embedded node stopped".to_owned(),
                }),
            )
        })?;
        node.start_file_transfer(transfer_req)
            .await
            .map_err(|problem| {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(ApiErrorDto {
                        error: problem.message,
                    }),
                )
            })?
    };

    // Keep the temporary staging directory alive until the transfer finishes.
    let transfer_id = handle.transfer_id.clone();
    let operation_id = handle.operation_id.clone();
    let runtime = state.runtime.clone();
    tokio::spawn(async move {
        let _keep_dir = temp_dir;
        let _keep_permit = _permit;
        let deadline = Instant::now() + Duration::from_secs(300);
        while Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let guard = runtime.read().await;
            let Some(node) = guard.as_ref() else { break };
            if let Ok(statuses) = node.list_file_transfers().await {
                if let Some(status) = statuses
                    .iter()
                    .find(|s| s.transfer_id == transfer_id && s.operation_id == operation_id)
                {
                    if matches!(
                        status.state,
                        TransferState::Completed | TransferState::Failed | TransferState::Cancelled
                    ) {
                        break;
                    }
                }
            }
        }
    });

    Ok((
        StatusCode::CREATED,
        Json(StartedUploadDto {
            transfer_id: handle.transfer_id,
            operation_id: handle.operation_id,
            size_bytes: total_bytes,
        }),
    ))
}

async fn handle_start_download(
    State(state): State<AndroidFileHttpState>,
    Json(body): Json<DownloadRequestBody>,
) -> Result<(StatusCode, Json<PendingDownloadDto>), (StatusCode, Json<ApiErrorDto>)> {
    if SharedPath::parse(&body.path).is_err() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(ApiErrorDto {
                error: "invalid portable download path".to_owned(),
            }),
        ));
    }
    let permit = state
        .transfer_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            (
                StatusCode::TOO_MANY_REQUESTS,
                Json(ApiErrorDto {
                    error: "android file transfer slots exhausted".to_owned(),
                }),
            )
        })?;
    let temp_dir = TempDir::new().map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiErrorDto {
                error: format!("failed to allocate download staging dir: {error}"),
            }),
        )
    })?;
    let local_path = temp_dir.path().join("download.bin");
    let (transfer_id_gen, operation_id_gen) = random_transfer_ids().map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiErrorDto {
                error: format!("failed to generate transfer ids: {error}"),
            }),
        )
    })?;
    let transfer_req = StartFileTransferRequest {
        peer: body.peer,
        network_id: body.network_id,
        share_id: body.share_id,
        transfer_id: transfer_id_gen.as_str().to_owned(),
        operation_id: operation_id_gen.as_str().to_owned(),
        path: body.path,
        local_path: local_path.to_string_lossy().into_owned(),
        direction: FileTransferDirection::Download,
        chunk_size: body.chunk_size.unwrap_or(DEFAULT_FILE_TRANSFER_CHUNK_SIZE),
    };
    let handle = {
        let guard = state.runtime.read().await;
        let node = guard.as_ref().ok_or_else(|| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ApiErrorDto {
                    error: "embedded node stopped".to_owned(),
                }),
            )
        })?;
        node.start_file_transfer(transfer_req)
            .await
            .map_err(|problem| {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(ApiErrorDto {
                        error: problem.message,
                    }),
                )
            })?
    };

    // Wait briefly (up to 30s) for download completion so the short-lived token
    // serves the finished verified file.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut completed = false;
    while Instant::now() < deadline {
        let guard = state.runtime.read().await;
        if let Some(node) = guard.as_ref() {
            if let Ok(statuses) = node.list_file_transfers().await {
                if let Some(status) = statuses.iter().find(|s| {
                    s.transfer_id == handle.transfer_id && s.operation_id == handle.operation_id
                }) {
                    match status.state {
                        TransferState::Completed => {
                            completed = true;
                            break;
                        }
                        TransferState::Failed | TransferState::Cancelled => {
                            return Err((
                                StatusCode::BAD_GATEWAY,
                                Json(ApiErrorDto {
                                    error: status
                                        .error_code
                                        .clone()
                                        .unwrap_or_else(|| "file_download_failed".to_owned()),
                                }),
                            ));
                        }
                        _ => {}
                    }
                }
            }
        } else {
            break;
        }
        drop(guard);
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    if !completed {
        return Err((
            StatusCode::GATEWAY_TIMEOUT,
            Json(ApiErrorDto {
                error: "file_download_timeout".to_owned(),
            }),
        ));
    }

    let token = generate_token().map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiErrorDto {
                error: error.to_string(),
            }),
        )
    })?;
    let now_ms = unix_time_ms();
    let expires_at_unix_ms = now_ms.saturating_add(DOWNLOAD_TOKEN_TTL_MS);
    {
        let mut guard = state
            .downloads
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.retain(|_, item| item.expires_at_unix_ms > now_ms);
        if guard.len() >= MAX_ANDROID_DOWNLOAD_TOKENS {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                Json(ApiErrorDto {
                    error: "too many pending downloads".to_owned(),
                }),
            ));
        }
        guard.insert(
            token.clone(),
            PendingDownload {
                token: token.clone(),
                transfer_id: handle.transfer_id.clone(),
                operation_id: handle.operation_id.clone(),
                file_path: local_path,
                expires_at_unix_ms,
                _temp_dir: temp_dir,
                _permit: permit,
            },
        );
    }

    Ok((
        StatusCode::CREATED,
        Json(PendingDownloadDto {
            token,
            transfer_id: handle.transfer_id,
            operation_id: handle.operation_id,
            expires_at_unix_ms,
        }),
    ))
}

async fn handle_download_stream(
    State(state): State<AndroidFileHttpState>,
    AxumPath(token): AxumPath<String>,
) -> Response {
    let now_ms = unix_time_ms();
    let pending = {
        let mut guard = state
            .downloads
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.retain(|_, item| item.expires_at_unix_ms > now_ms);
        guard.remove(&token)
    };
    let Some(pending) = pending else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiErrorDto {
                error: "download token not found or expired".to_owned(),
            }),
        )
            .into_response();
    };
    let _ = (&pending.token, &pending.transfer_id, &pending.operation_id);
    match tokio::fs::read(&pending.file_path).await {
        Ok(bytes) => {
            let len = bytes.len();
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .header(header::CONTENT_LENGTH, len.to_string())
                .body(Body::from(bytes))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiErrorDto {
                error: format!("failed reading staged download: {error}"),
            }),
        )
            .into_response(),
    }
}

async fn handle_pause(
    State(state): State<AndroidFileHttpState>,
    AxumPath((transfer_id, operation_id)): AxumPath<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiErrorDto>)> {
    validate_ids(&transfer_id, &operation_id)?;
    let guard = state.runtime.read().await;
    let node = guard.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiErrorDto {
                error: "embedded node stopped".to_owned(),
            }),
        )
    })?;
    node.pause_file_transfer(transfer_id, operation_id)
        .await
        .map_err(|problem| {
            (
                StatusCode::BAD_GATEWAY,
                Json(ApiErrorDto {
                    error: problem.message,
                }),
            )
        })?;
    Ok(Json(serde_json::json!({ "status": "paused" })))
}

async fn handle_resume(
    State(state): State<AndroidFileHttpState>,
    AxumPath((transfer_id, operation_id)): AxumPath<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiErrorDto>)> {
    validate_ids(&transfer_id, &operation_id)?;
    let guard = state.runtime.read().await;
    let node = guard.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiErrorDto {
                error: "embedded node stopped".to_owned(),
            }),
        )
    })?;
    node.resume_file_transfer(transfer_id, operation_id)
        .await
        .map_err(|problem| {
            (
                StatusCode::BAD_GATEWAY,
                Json(ApiErrorDto {
                    error: problem.message,
                }),
            )
        })?;
    Ok(Json(serde_json::json!({ "status": "resumed" })))
}

async fn handle_cancel(
    State(state): State<AndroidFileHttpState>,
    AxumPath((transfer_id, operation_id)): AxumPath<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiErrorDto>)> {
    validate_ids(&transfer_id, &operation_id)?;
    let guard = state.runtime.read().await;
    let node = guard.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiErrorDto {
                error: "embedded node stopped".to_owned(),
            }),
        )
    })?;
    node.cancel_file_transfer(transfer_id, operation_id)
        .await
        .map_err(|problem| {
            (
                StatusCode::BAD_GATEWAY,
                Json(ApiErrorDto {
                    error: problem.message,
                }),
            )
        })?;
    Ok(Json(serde_json::json!({ "status": "cancelled" })))
}

fn validate_ids(
    transfer_id: &str,
    operation_id: &str,
) -> Result<(), (StatusCode, Json<ApiErrorDto>)> {
    if TransferId::parse(transfer_id.to_owned()).is_err()
        || OperationId::parse(operation_id.to_owned()).is_err()
    {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(ApiErrorDto {
                error: "invalid transfer_id or operation_id".to_owned(),
            }),
        ));
    }
    Ok(())
}

fn random_transfer_ids() -> std::io::Result<(TransferId, OperationId)> {
    let mut transfer_seed = [0u8; 32];
    let mut operation_seed = [0u8; 32];
    getrandom::fill(&mut transfer_seed)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    getrandom::fill(&mut operation_seed)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok((
        TransferId::from_public_key(&transfer_seed),
        OperationId::from_public_key(&operation_seed),
    ))
}

fn generate_token() -> std::io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| std::io::Error::other(error.to_string()))?;
    let mut hex = String::with_capacity(64);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{byte:02x}");
    }
    Ok(hex)
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().try_into().unwrap_or(u64::MAX))
        .unwrap_or_default()
}
