//! Authenticated WebUI File content adapters.
//!
//! The browser only supplies a peer/network/share selector and a logical path.
//! Requester-local paths, actor/ACL context, transfer IDs and operation IDs are
//! always derived by the node-side WebUI process. Upload and download payloads
//! are staged in private, process-owned temporary directories with bounded
//! lifetimes; those paths never cross the HTTP boundary.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{Extension, Path as AxumPath, Query, State};
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::Json;
use bytes::Bytes;
use fuckipv6_common::domain::{
    OperationId, SharedPath, TransferId, TransferState, MAX_TRANSFER_CHUNK_BYTES,
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::{sleep, Instant};

use crate::file_share::{
    FileDirectoryListing, FileTransferDirection, FileTransferStatus, ListFileDirectoryRequest,
    StartFileTransferRequest, DEFAULT_FILE_DIRECTORY_LIST_LIMIT, DEFAULT_FILE_TRANSFER_CHUNK_SIZE,
    MAX_DIRECTORY_LIST_LIMIT,
};
use crate::webui::controller::{AppController, FilePeerTarget};

use super::{api_error, api_error_from_app_controller, ApiError};

pub(crate) const MAX_WEB_FILE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_WEB_FILE_TRANSFER_SLOTS: usize = 4;
const MAX_WEB_DOWNLOAD_TOKENS: usize = 32;
const WEB_FILE_TRANSFER_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);
const FILE_STATUS_POLL_INTERVAL: Duration = Duration::from_millis(250);
const FILE_STATUS_INITIAL_GRACE: Duration = Duration::from_secs(30);
const DOWNLOAD_BUFFER_BYTES: usize = 64 * 1024;

type ApiFailure = (StatusCode, Json<ApiError>);
type ApiResponse<T> = Result<T, ApiFailure>;

#[derive(Clone, Default)]
pub(crate) struct FileWebState {
    inner: Arc<FileWebStateInner>,
}

struct FileWebStateInner {
    transfer_slots: Arc<Semaphore>,
    downloads: Mutex<HashMap<String, PendingDownload>>,
}

impl Default for FileWebStateInner {
    fn default() -> Self {
        Self {
            transfer_slots: Arc::new(Semaphore::new(MAX_WEB_FILE_TRANSFER_SLOTS)),
            downloads: Mutex::new(HashMap::new()),
        }
    }
}

impl FileWebState {
    fn try_transfer_permit(&self) -> ApiResponse<OwnedSemaphorePermit> {
        self.inner
            .transfer_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                api_error(
                    StatusCode::TOO_MANY_REQUESTS,
                    "WebUI File 临时传输槽位已满，请稍后重试",
                )
            })
    }

    fn insert_download(&self, download: PendingDownload, now_unix_ms: u64) -> ApiResponse<()> {
        let mut downloads =
            self.inner.downloads.lock().map_err(|_| {
                api_error(StatusCode::INTERNAL_SERVER_ERROR, "File 临时区状态不可用")
            })?;
        downloads.retain(|_, pending| pending.expires_at_unix_ms > now_unix_ms);
        if downloads.len() >= MAX_WEB_DOWNLOAD_TOKENS {
            return Err(api_error(
                StatusCode::TOO_MANY_REQUESTS,
                "WebUI 下载令牌数量已达上限，请稍后重试",
            ));
        }
        downloads.insert(download.token.clone(), download);
        Ok(())
    }

    fn take_download(&self, token: &str, now_unix_ms: u64) -> Option<PendingDownload> {
        let mut downloads = self.inner.downloads.lock().ok()?;
        downloads.retain(|_, pending| pending.expires_at_unix_ms > now_unix_ms);
        downloads.remove(token)
    }
}

struct PendingDownload {
    token: String,
    transfer_id: String,
    operation_id: String,
    file_path: std::path::PathBuf,
    expires_at_unix_ms: u64,
    _temporary: TempDir,
    _permit: OwnedSemaphorePermit,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DirectoryQuery {
    peer: String,
    #[serde(default)]
    network_id: Option<String>,
    share_id: String,
    #[serde(default)]
    path: String,
    #[serde(default = "default_directory_limit")]
    limit: u32,
    #[serde(default)]
    cursor: Option<String>,
}

fn default_directory_limit() -> u32 {
    DEFAULT_FILE_DIRECTORY_LIST_LIMIT
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UploadQuery {
    peer: String,
    #[serde(default)]
    network_id: Option<String>,
    share_id: String,
    path: String,
    #[serde(default = "default_chunk_size")]
    chunk_size: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DownloadRequest {
    peer: String,
    #[serde(default)]
    network_id: Option<String>,
    share_id: String,
    path: String,
    #[serde(default = "default_chunk_size")]
    chunk_size: u64,
}

fn default_chunk_size() -> u64 {
    DEFAULT_FILE_TRANSFER_CHUNK_SIZE
}

#[derive(Debug, Serialize)]
pub(crate) struct StartedFileTransfer {
    pub transfer_id: String,
    pub operation_id: String,
    pub size_bytes: u64,
}

#[derive(Debug, Serialize)]
pub(crate) struct PendingDownloadResponse {
    pub token: String,
    pub transfer_id: String,
    pub operation_id: String,
    pub expires_at_unix_ms: u64,
}

#[derive(Debug, Serialize)]
pub(crate) struct FileControlResponse {
    pub status: &'static str,
}

pub(crate) async fn handle_targets(
    State(controller): State<Arc<AppController>>,
) -> ApiResponse<Json<Vec<FilePeerTarget>>> {
    controller
        .list_file_targets()
        .await
        .map(Json)
        .map_err(|error| api_error_from_app_controller("File 目标读取失败", &error))
}

pub(crate) async fn handle_directory(
    State(controller): State<Arc<AppController>>,
    Query(query): Query<DirectoryQuery>,
) -> ApiResponse<Json<FileDirectoryListing>> {
    let request = directory_request(query)?;
    controller
        .list_file_directory(request)
        .await
        .map(Json)
        .map_err(|error| api_error_from_app_controller("File 目录浏览失败", &error))
}

pub(crate) async fn handle_transfers(
    State(controller): State<Arc<AppController>>,
) -> ApiResponse<Json<Vec<FileTransferStatus>>> {
    controller
        .list_file_transfers()
        .await
        .map(Json)
        .map_err(|error| api_error_from_app_controller("File 传输状态读取失败", &error))
}

pub(crate) async fn handle_upload(
    State(controller): State<Arc<AppController>>,
    Extension(state): Extension<FileWebState>,
    Query(query): Query<UploadQuery>,
    body: Body,
) -> ApiResponse<(StatusCode, Json<StartedFileTransfer>)> {
    let (peer, network_id, share_id, path, chunk_size) = validate_transfer_query(
        query.peer,
        query.network_id,
        query.share_id,
        query.path,
        query.chunk_size,
    )?;
    let permit = state.try_transfer_permit()?;
    let temporary = private_tempdir("fuckipv6-webui-upload-").map_err(temp_error)?;
    let local_path = temporary.path().join("source");
    let mut file = create_private_file(&local_path).map_err(temp_error)?;
    let size_bytes = receive_upload_body(body, &mut file, MAX_WEB_FILE_BYTES).await?;
    file.flush().await.map_err(|error| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("WebUI 上传临时文件刷新失败: {error}"),
        )
    })?;
    file.sync_all().await.map_err(|error| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("WebUI 上传临时文件同步失败: {error}"),
        )
    })?;
    drop(file);

    let (transfer_id, operation_id) = random_id_pair()?;
    let request = StartFileTransferRequest {
        peer,
        network_id,
        direction: FileTransferDirection::Upload,
        share_id,
        transfer_id: transfer_id.as_str().to_string(),
        operation_id: operation_id.as_str().to_string(),
        path,
        local_path: local_path.to_string_lossy().into_owned(),
        chunk_size,
    };
    controller
        .start_file_transfer(request)
        .await
        .map_err(|error| api_error_from_app_controller("File 上传启动失败", &error))?;

    let cleanup_controller = Arc::clone(&controller);
    let cleanup_transfer_id = transfer_id.as_str().to_string();
    let cleanup_operation_id = operation_id.as_str().to_string();
    tokio::spawn(async move {
        let _permit = permit;
        let _temporary = temporary;
        cleanup_terminal_transfer(
            cleanup_controller,
            cleanup_transfer_id,
            cleanup_operation_id,
        )
        .await;
    });

    Ok((
        StatusCode::CREATED,
        Json(StartedFileTransfer {
            transfer_id: transfer_id.as_str().to_string(),
            operation_id: operation_id.as_str().to_string(),
            size_bytes,
        }),
    ))
}

pub(crate) async fn handle_start_download(
    State(controller): State<Arc<AppController>>,
    Extension(state): Extension<FileWebState>,
    Json(payload): Json<DownloadRequest>,
) -> ApiResponse<(StatusCode, Json<PendingDownloadResponse>)> {
    let (peer, network_id, share_id, path, chunk_size) = validate_transfer_query(
        payload.peer,
        payload.network_id,
        payload.share_id,
        payload.path,
        payload.chunk_size,
    )?;
    let permit = state.try_transfer_permit()?;
    let temporary = private_tempdir("fuckipv6-webui-download-").map_err(temp_error)?;
    let file_path = temporary.path().join("download");
    let (transfer_id, operation_id) = random_id_pair()?;
    let transfer_id_text = transfer_id.as_str().to_string();
    let operation_id_text = operation_id.as_str().to_string();

    controller
        .start_file_transfer(StartFileTransferRequest {
            peer,
            network_id,
            direction: FileTransferDirection::Download,
            share_id,
            transfer_id: transfer_id_text.clone(),
            operation_id: operation_id_text.clone(),
            path,
            local_path: file_path.to_string_lossy().into_owned(),
            chunk_size,
        })
        .await
        .map_err(|error| api_error_from_app_controller("File 下载启动失败", &error))?;

    let mut token_bytes = [0_u8; 32];
    getrandom::fill(&mut token_bytes).map_err(|error| {
        api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("WebUI 下载令牌生成失败: {error}"),
        )
    })?;
    let token = base64_url(&token_bytes);
    let now_unix_ms = unix_time_ms();
    let expires_at_unix_ms =
        now_unix_ms.saturating_add(WEB_FILE_TRANSFER_LIFETIME.as_millis() as u64);
    let download = PendingDownload {
        token: token.clone(),
        transfer_id: transfer_id_text.clone(),
        operation_id: operation_id_text.clone(),
        file_path,
        expires_at_unix_ms,
        _temporary: temporary,
        _permit: permit,
    };
    if let Err(error) = state.insert_download(download, now_unix_ms) {
        let _ = controller
            .cancel_file_transfer(transfer_id_text, operation_id_text)
            .await;
        return Err(error);
    }

    spawn_download_expiry(
        state,
        Arc::clone(&controller),
        token.clone(),
        expires_at_unix_ms,
    );

    Ok((
        StatusCode::CREATED,
        Json(PendingDownloadResponse {
            token,
            transfer_id: transfer_id_text,
            operation_id: operation_id_text,
            expires_at_unix_ms,
        }),
    ))
}

pub(crate) async fn handle_download(
    State(controller): State<Arc<AppController>>,
    Extension(state): Extension<FileWebState>,
    AxumPath(token): AxumPath<String>,
) -> ApiResponse<Response> {
    if token.len() != 43 || !token.bytes().all(is_base64_url_byte) {
        return Err(api_error(StatusCode::NOT_FOUND, "下载令牌不存在或已过期"));
    }
    let Some(download) = state.take_download(&token, unix_time_ms()) else {
        return Err(api_error(StatusCode::NOT_FOUND, "下载令牌不存在或已过期"));
    };

    wait_for_download(
        &controller,
        &download.transfer_id,
        &download.operation_id,
        download.expires_at_unix_ms,
    )
    .await?;

    let metadata = tokio::fs::metadata(&download.file_path)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("WebUI 下载临时文件不可用: {error}"),
            )
        })?;
    if !metadata.is_file() {
        return Err(api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "WebUI 下载临时结果不是普通文件",
        ));
    }
    let body = streaming_file_body(download).await?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, metadata.len())
        .header(header::CACHE_CONTROL, "no-store")
        .body(body)
        .map_err(|error| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("WebUI 下载响应构建失败: {error}"),
            )
        })
}

pub(crate) async fn handle_pause(
    State(controller): State<Arc<AppController>>,
    AxumPath((transfer_id, operation_id)): AxumPath<(String, String)>,
) -> ApiResponse<Json<FileControlResponse>> {
    let (transfer_id, operation_id) = validate_operation_ids(transfer_id, operation_id)?;
    controller
        .pause_file_transfer(transfer_id, operation_id)
        .await
        .map_err(|error| api_error_from_app_controller("File 传输暂停失败", &error))?;
    Ok(Json(FileControlResponse { status: "paused" }))
}

pub(crate) async fn handle_resume(
    State(controller): State<Arc<AppController>>,
    AxumPath((transfer_id, operation_id)): AxumPath<(String, String)>,
) -> ApiResponse<Json<FileControlResponse>> {
    let (transfer_id, operation_id) = validate_operation_ids(transfer_id, operation_id)?;
    controller
        .resume_file_transfer(transfer_id, operation_id)
        .await
        .map_err(|error| api_error_from_app_controller("File 传输恢复失败", &error))?;
    Ok(Json(FileControlResponse { status: "resumed" }))
}

pub(crate) async fn handle_cancel(
    State(controller): State<Arc<AppController>>,
    AxumPath((transfer_id, operation_id)): AxumPath<(String, String)>,
) -> ApiResponse<Json<FileControlResponse>> {
    let (transfer_id, operation_id) = validate_operation_ids(transfer_id, operation_id)?;
    controller
        .cancel_file_transfer(transfer_id, operation_id)
        .await
        .map_err(|error| api_error_from_app_controller("File 传输取消失败", &error))?;
    Ok(Json(FileControlResponse {
        status: "cancelled",
    }))
}

fn directory_request(query: DirectoryQuery) -> ApiResponse<ListFileDirectoryRequest> {
    if query.peer.trim().is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "File peer 不能为空"));
    }
    if query.share_id.trim().is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "File share_id 不能为空"));
    }
    if query.limit == 0 || query.limit > MAX_DIRECTORY_LIST_LIMIT {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "File 目录分页大小超出允许范围",
        ));
    }
    SharedPath::parse(query.path.clone()).map_err(|error| {
        api_error(
            StatusCode::BAD_REQUEST,
            format!("File 逻辑路径无效: {error}"),
        )
    })?;
    Ok(ListFileDirectoryRequest {
        peer: query.peer,
        network_id: query.network_id,
        share_id: query.share_id,
        path: query.path,
        limit: query.limit,
        cursor: query.cursor,
    })
}

fn validate_transfer_query(
    peer: String,
    network_id: Option<String>,
    share_id: String,
    path: String,
    chunk_size: u64,
) -> ApiResponse<(String, Option<String>, String, String, u64)> {
    if peer.trim().is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "File peer 不能为空"));
    }
    if share_id.trim().is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "File share_id 不能为空"));
    }
    let parsed = SharedPath::parse(path.clone()).map_err(|error| {
        api_error(
            StatusCode::BAD_REQUEST,
            format!("File 逻辑路径无效: {error}"),
        )
    })?;
    if parsed.is_root() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "File 上传/下载路径不能是共享根目录",
        ));
    }
    if chunk_size == 0 || chunk_size > MAX_TRANSFER_CHUNK_BYTES {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "File 分块大小超出允许范围",
        ));
    }
    Ok((peer, network_id, share_id, path, chunk_size))
}

fn validate_operation_ids(
    transfer_id: String,
    operation_id: String,
) -> ApiResponse<(String, String)> {
    TransferId::parse(transfer_id.clone())
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "File transfer_id 无效"))?;
    OperationId::parse(operation_id.clone())
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "File operation_id 无效"))?;
    Ok((transfer_id, operation_id))
}

async fn receive_upload_body(
    body: Body,
    file: &mut tokio::fs::File,
    max_bytes: u64,
) -> ApiResponse<u64> {
    let mut stream = body.into_data_stream();
    let mut total = 0_u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            api_error(
                StatusCode::BAD_REQUEST,
                format!("WebUI 上传请求体读取失败: {error}"),
            )
        })?;
        total = total
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| api_error(StatusCode::PAYLOAD_TOO_LARGE, "WebUI 上传大小溢出"))?;
        if total > max_bytes {
            return Err(api_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("WebUI 上传超过 {} 字节上限", MAX_WEB_FILE_BYTES),
            ));
        }
        file.write_all(&chunk).await.map_err(|error| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("WebUI 上传临时文件写入失败: {error}"),
            )
        })?;
    }
    Ok(total)
}

async fn wait_for_download(
    controller: &AppController,
    transfer_id: &str,
    operation_id: &str,
    expires_at_unix_ms: u64,
) -> ApiResponse<()> {
    let deadline =
        Instant::now() + Duration::from_millis(expires_at_unix_ms.saturating_sub(unix_time_ms()));
    let started = Instant::now();
    let mut observed = false;
    loop {
        if Instant::now() >= deadline {
            let _ = controller
                .cancel_file_transfer(transfer_id.to_string(), operation_id.to_string())
                .await;
            return Err(api_error(
                StatusCode::GATEWAY_TIMEOUT,
                "File 下载在 WebUI 临时区过期前未完成",
            ));
        }
        match controller.list_file_transfers().await {
            Ok(statuses) => {
                if let Some(status) = statuses.iter().find(|status| {
                    status.transfer_id == transfer_id && status.operation_id == operation_id
                }) {
                    observed = true;
                    match status.state {
                        TransferState::Completed => return Ok(()),
                        TransferState::Failed => {
                            return Err(api_error(
                                StatusCode::BAD_GATEWAY,
                                status
                                    .error_code
                                    .as_deref()
                                    .map(|code| format!("File 下载失败: {code}"))
                                    .unwrap_or_else(|| "File 下载失败".to_string()),
                            ));
                        }
                        TransferState::Cancelled | TransferState::Expired => {
                            return Err(api_error(StatusCode::CONFLICT, "File 下载已取消或过期"));
                        }
                        _ => {}
                    }
                }
            }
            Err(error) => {
                return Err(api_error_from_app_controller(
                    "File 下载状态读取失败",
                    &error,
                ));
            }
        }
        if !observed && started.elapsed() >= FILE_STATUS_INITIAL_GRACE {
            let _ = controller
                .cancel_file_transfer(transfer_id.to_string(), operation_id.to_string())
                .await;
            return Err(api_error(
                StatusCode::BAD_GATEWAY,
                "File 运行时未发布下载状态",
            ));
        }
        sleep(FILE_STATUS_POLL_INTERVAL).await;
    }
}

async fn cleanup_terminal_transfer(
    controller: Arc<AppController>,
    transfer_id: String,
    operation_id: String,
) {
    let started = Instant::now();
    let deadline = started + WEB_FILE_TRANSFER_LIFETIME;
    let mut observed = false;
    loop {
        if Instant::now() >= deadline {
            let _ = controller
                .cancel_file_transfer(transfer_id.clone(), operation_id.clone())
                .await;
            return;
        }
        if let Ok(statuses) = controller.list_file_transfers().await {
            if let Some(status) = statuses.iter().find(|status| {
                status.transfer_id == transfer_id && status.operation_id == operation_id
            }) {
                observed = true;
                if status.state.is_terminal() {
                    return;
                }
            }
        }
        if !observed && started.elapsed() >= FILE_STATUS_INITIAL_GRACE {
            let _ = controller
                .cancel_file_transfer(transfer_id.clone(), operation_id.clone())
                .await;
            return;
        }
        sleep(FILE_STATUS_POLL_INTERVAL).await;
    }
}

fn spawn_download_expiry(
    state: FileWebState,
    controller: Arc<AppController>,
    token: String,
    expires_at_unix_ms: u64,
) {
    tokio::spawn(async move {
        let delay = Duration::from_millis(expires_at_unix_ms.saturating_sub(unix_time_ms()));
        sleep(delay).await;
        if let Some(expired) = state.take_download(&token, unix_time_ms()) {
            let _ = controller
                .cancel_file_transfer(expired.transfer_id, expired.operation_id)
                .await;
        }
    });
}

async fn streaming_file_body(download: PendingDownload) -> ApiResponse<Body> {
    let file = tokio::fs::File::open(&download.file_path)
        .await
        .map_err(|error| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("WebUI 下载临时文件打开失败: {error}"),
            )
        })?;
    let stream = futures::stream::unfold(
        (file, download, false),
        |(mut file, download, mut finished)| async move {
            if finished {
                return None;
            }
            let mut buffer = vec![0_u8; DOWNLOAD_BUFFER_BYTES];
            match file.read(&mut buffer).await {
                Ok(0) => None,
                Ok(read) => {
                    buffer.truncate(read);
                    Some((
                        Ok::<Bytes, io::Error>(Bytes::from(buffer)),
                        (file, download, finished),
                    ))
                }
                Err(error) => {
                    finished = true;
                    Some((Err(error), (file, download, finished)))
                }
            }
        },
    );
    Ok(Body::from_stream(stream))
}

fn private_tempdir(prefix: &str) -> io::Result<TempDir> {
    tempfile::Builder::new().prefix(prefix).tempdir()
}

fn create_private_file(path: &std::path::Path) -> io::Result<tokio::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).read(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map(tokio::fs::File::from_std)
}

fn random_id_pair() -> ApiResponse<(TransferId, OperationId)> {
    let mut bytes = [0_u8; 64];
    getrandom::fill(&mut bytes).map_err(|error| {
        api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("File 操作 ID 生成失败: {error}"),
        )
    })?;
    Ok((
        TransferId::from_public_key(&bytes[..32]),
        OperationId::from_public_key(&bytes[32..]),
    ))
}

fn base64_url(bytes: &[u8]) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;
    URL_SAFE_NO_PAD.encode(bytes)
}

fn is_base64_url_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
}

fn temp_error(error: io::Error) -> ApiFailure {
    api_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("WebUI File 临时区不可用: {error}"),
    )
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().try_into().unwrap_or(u64::MAX))
        .unwrap_or(1)
        .max(1)
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use axum::http::{Method, Request};
    use tower::ServiceExt;

    use super::*;
    use crate::webui::{create_app_web_router_with_auth, security::WebUiAuth};

    #[test]
    fn request_dtos_reject_sensitive_or_local_fields() {
        for forbidden in [
            "local_path",
            "actor",
            "actor_id",
            "acl_revision",
            "access_epoch",
            "transfer_id",
            "operation_id",
        ] {
            let mut upload = serde_json::json!({
                "peer": "peer",
                "share_id": "share",
                "path": "docs/file.bin"
            });
            upload
                .as_object_mut()
                .expect("upload object")
                .insert(forbidden.to_owned(), serde_json::json!("forbidden"));
            assert!(
                serde_json::from_value::<UploadQuery>(upload).is_err(),
                "upload accepted {forbidden}"
            );

            let mut directory = serde_json::json!({
                "peer": "peer",
                "share_id": "share",
                "path": ""
            });
            directory
                .as_object_mut()
                .expect("directory object")
                .insert(forbidden.to_owned(), serde_json::json!("forbidden"));
            assert!(
                serde_json::from_value::<DirectoryQuery>(directory).is_err(),
                "directory accepted {forbidden}"
            );

            let mut download = serde_json::json!({
                "peer": "peer",
                "share_id": "share",
                "path": "docs/file.bin"
            });
            download
                .as_object_mut()
                .expect("download object")
                .insert(forbidden.to_owned(), serde_json::json!("forbidden"));
            assert!(
                serde_json::from_value::<DownloadRequest>(download).is_err(),
                "download accepted {forbidden}"
            );
        }
    }

    #[tokio::test]
    async fn upload_stream_enforces_limit_and_cleans_partial_file() {
        let temporary = private_tempdir("fuckipv6-webui-test-").expect("tempdir");
        let path = temporary.path().join("source");
        let mut file = create_private_file(&path).expect("private source");
        let result = receive_upload_body(Body::from("12345"), &mut file, 4).await;
        assert!(result.is_err());
        drop(file);
        drop(temporary);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn streaming_download_body_returns_bytes_and_cleans_temporary_directory() {
        let temporary = private_tempdir("fuckipv6-webui-test-").expect("tempdir");
        let root = temporary.path().to_path_buf();
        let path = temporary.path().join("download");
        tokio::fs::write(&path, b"download bytes")
            .await
            .expect("download fixture");
        let slots = Arc::new(Semaphore::new(1));
        let download = PendingDownload {
            token: "token".to_string(),
            transfer_id: "transfer".to_string(),
            operation_id: "operation".to_string(),
            file_path: path.clone(),
            expires_at_unix_ms: unix_time_ms() + 60_000,
            _temporary: temporary,
            _permit: slots.try_acquire_owned().expect("permit"),
        };
        let body = streaming_file_body(download).await.expect("body");
        let bytes = to_bytes(body, 1024).await.expect("body bytes");
        assert_eq!(&bytes[..], b"download bytes");
        assert!(!root.exists());
    }

    #[tokio::test]
    async fn content_routes_reject_sensitive_query_fields_before_controller_dispatch() {
        let app =
            create_app_web_router_with_auth(Arc::new(AppController::new()), WebUiAuth::disabled());
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/file/uploads?peer=peer&share_id=share&path=docs%2Ffile.bin&local_path=%2Fprivate%2Ffile")
                    .header(header::HOST, "localhost")
                    .header(header::ORIGIN, "http://localhost")
                    .header("sec-fetch-site", "same-origin")
                    .body(Body::from("payload"))
                    .expect("upload request"),
            )
            .await
            .expect("upload response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/file/downloads")
                    .header(header::HOST, "localhost")
                    .header(header::ORIGIN, "http://localhost")
                    .header("sec-fetch-site", "same-origin")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"peer":"peer","share_id":"share","path":"docs/file.bin","actor":"forged"}"#,
                    ))
                    .expect("download request"),
            )
            .await
            .expect("download response");
        assert!(response.status().is_client_error());
    }

    #[tokio::test]
    async fn missing_file_backend_is_reported_as_unsupported() {
        let app =
            create_app_web_router_with_auth(Arc::new(AppController::new()), WebUiAuth::disabled());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/file/transfers")
                    .body(Body::empty())
                    .expect("transfers request"),
            )
            .await
            .expect("transfers response");
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(content_type.starts_with("application/json"));
    }
}
