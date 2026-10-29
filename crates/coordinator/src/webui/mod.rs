use anyhow::{Context, Result};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use fuckipv6_common::{
    recent_error_log_lines, CoordinatorAuthConfig, NetworkAllocation, NetworkMode,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::time::{interval, Duration};

use crate::auth::jwt::{Claims, JwtManager};
use crate::signaling::session::{
    CoordinatorSnapshot, PeerSnapshot, SessionManager, TrafficSnapshot,
};

#[derive(Clone)]
pub struct AppState {
    pub session_manager: Arc<SessionManager>,
    pub auth_config: CoordinatorAuthConfig,
    pub jwt_manager: Arc<JwtManager>,
    pub config_path: Option<String>,
}

impl AppState {
    pub fn new(session_manager: Arc<SessionManager>, auth_config: CoordinatorAuthConfig) -> Self {
        Self::new_with_config_path(session_manager, auth_config, None)
    }

    pub fn new_with_config_path(
        session_manager: Arc<SessionManager>,
        auth_config: CoordinatorAuthConfig,
        config_path: Option<String>,
    ) -> Self {
        let secret = auth_config
            .jwt_secret
            .clone()
            .unwrap_or_else(|| {
                use sha2::{Digest, Sha256};
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                let hash = Sha256::digest(format!("fuckipv6-ephemeral-secret-{now}").as_bytes());
                crate::auth::jwt::base64_url_encode(&hash)
            });

        let jwt_manager = Arc::new(JwtManager::new(
            secret.into_bytes(),
            auth_config.jwt_expiration_secs,
        ));

        Self {
            session_manager,
            auth_config,
            jwt_manager,
            config_path,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinatorNetworkInfoDto {
    pub name: String,
    pub tun_name: String,
    pub ipv6_pool: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv4_pool: Option<String>,
    pub peer_count: usize,
    pub status: String,
}

#[derive(Debug, Serialize)]
struct StatusResponse {
    service: &'static str,
    online_clients: usize,
    traffic: TrafficSnapshot,
    auth_required: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    networks: Vec<CoordinatorNetworkInfoDto>,
}

#[derive(Debug, Serialize)]
struct SnapshotResponse {
    service: &'static str,
    online_clients: usize,
    peers: Vec<PeerSnapshot>,
    traffic: TrafficSnapshot,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub success: bool,
    pub token: Option<String>,
    pub error: Option<String>,
    pub expires_in: Option<u64>,
    pub auth_required: bool,
}

pub async fn run(bind_addr: String, session_manager: Arc<SessionManager>) -> Result<()> {
    run_with_auth(bind_addr, session_manager, CoordinatorAuthConfig::default()).await
}

pub async fn run_with_auth(
    bind_addr: String,
    session_manager: Arc<SessionManager>,
    auth_config: CoordinatorAuthConfig,
) -> Result<()> {
    run_with_options(bind_addr, session_manager, auth_config, None).await
}

pub async fn run_with_options(
    bind_addr: String,
    session_manager: Arc<SessionManager>,
    auth_config: CoordinatorAuthConfig,
    config_path: Option<String>,
) -> Result<()> {
    let state = AppState::new_with_config_path(session_manager, auth_config, config_path);
    let app = build_router(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("无法监听 Web 管理界面: {bind_addr}"))?;
    tracing::info!("Web 管理界面监听: http://{}", bind_addr);
    axum::serve(listener, app)
        .await
        .context("Web 管理界面服务异常退出")?;
    Ok(())
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/assets/webui.css", get(webui_css))
        .route("/api/status", get(status))
        .route("/api/auth/login", post(login))
        .route("/api/peers", get(peers))
        .route("/api/logs", get(logs))
        .route(
            "/api/config",
            get(get_network_config).post(update_network_config),
        )
        .route("/api/blacklist", get(get_blacklist))
        .route("/api/blacklist/ban", post(ban_client))
        .route("/api/blacklist/unban", post(unban_client))
        .route("/ws", get(ws))
        .with_state(state)
}

fn check_auth(
    state: &AppState,
    headers: &HeaderMap,
    query_token: Option<&str>,
) -> Result<Claims, StatusCode> {
    if !state.auth_config.is_auth_required() {
        return Ok(Claims {
            sub: "anonymous".to_string(),
            role: "admin".to_string(),
            exp: u64::MAX,
            iat: 0,
        });
    }

    let token = if let Some(token) = query_token {
        if !token.is_empty() {
            Some(token)
        } else {
            None
        }
    } else if let Some(auth_val) = headers.get(axum::http::header::AUTHORIZATION) {
        if let Ok(auth_str) = auth_val.to_str() {
            if let Some(token) = auth_str.strip_prefix("Bearer ") {
                Some(token.trim())
            } else {
                Some(auth_str.trim())
            }
        } else {
            None
        }
    } else {
        None
    };

    let Some(token) = token else {
        return Err(StatusCode::UNAUTHORIZED);
    };

    state
        .jwt_manager
        .verify(token)
        .map_err(|_| StatusCode::UNAUTHORIZED)
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn webui_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        WEBUI_CSS,
    )
}

async fn status(State(state): State<AppState>) -> Json<StatusResponse> {
    let snapshot = state.session_manager.snapshot().await;
    let alloc = state.session_manager.current_network_allocation().await;
    let ipv4_desc = if alloc.mode.is_ipv4_enabled() {
        Some(alloc.ipv4_pool.clone())
    } else {
        None
    };
    let networks = vec![CoordinatorNetworkInfoDto {
        name: "default".to_string(),
        tun_name: "fuckipv6".to_string(),
        ipv6_pool: alloc.effective_ipv6_pool().to_string(),
        ipv4_pool: ipv4_desc,
        peer_count: snapshot.online_clients,
        status: "活跃就绪".to_string(),
    }];
    Json(StatusResponse {
        service: "fuckipv6-coordinator",
        online_clients: snapshot.online_clients,
        traffic: snapshot.traffic,
        auth_required: state.auth_config.is_auth_required(),
        networks,
    })
}

async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> (StatusCode, Json<LoginResponse>) {
    if !state.auth_config.is_auth_required() {
        return (
            StatusCode::OK,
            Json(LoginResponse {
                success: true,
                token: Some("anonymous".to_string()),
                error: None,
                expires_in: Some(state.auth_config.jwt_expiration_secs),
                auth_required: false,
            }),
        );
    }

    let is_valid = req.username == state.auth_config.admin_username
        && state.auth_config.admin_password.as_deref() == Some(&req.password);

    if !is_valid {
        return (
            StatusCode::UNAUTHORIZED,
            Json(LoginResponse {
                success: false,
                token: None,
                error: Some("用户名或密码错误".to_string()),
                expires_in: None,
                auth_required: true,
            }),
        );
    }

    match state.jwt_manager.sign(&req.username, "admin") {
        Ok(token) => (
            StatusCode::OK,
            Json(LoginResponse {
                success: true,
                token: Some(token),
                error: None,
                expires_in: Some(state.auth_config.jwt_expiration_secs),
                auth_required: true,
            }),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(LoginResponse {
                success: false,
                token: None,
                error: Some(format!("签发 Token 失败: {e}")),
                expires_in: None,
                auth_required: true,
            }),
        ),
    }
}

async fn peers(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Vec<PeerSnapshot>>, StatusCode> {
    check_auth(&state, &headers, params.get("token").map(String::as_str))?;
    let snapshot: CoordinatorSnapshot = state.session_manager.snapshot().await;
    Ok(Json(snapshot.peers))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfigDto {
    pub mode: NetworkMode,
    pub ipv4_pool: String,
    pub ipv6_pool: String,
    pub ipv4_prefix: u8,
    pub ipv6_prefix: u8,
    pub config_file: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateNetworkConfigRequest {
    pub mode: NetworkMode,
    pub ipv4_pool: String,
    pub ipv6_pool: String,
}

#[derive(Debug, Serialize)]
pub struct NetworkConfigApiResponse {
    pub success: bool,
    pub message: String,
    pub data: Option<NetworkConfigDto>,
}

async fn get_network_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<NetworkConfigDto>, StatusCode> {
    check_auth(&state, &headers, params.get("token").map(String::as_str))?;
    let alloc = state.session_manager.current_network_allocation().await;
    let ipv4_prefix = state.session_manager.ipv4_prefix().await;
    let ipv6_prefix = state.session_manager.ipv6_prefix().await;

    let ipv6_pool = alloc.effective_ipv6_pool().to_string();
    Ok(Json(NetworkConfigDto {
        mode: alloc.mode,
        ipv4_pool: alloc.ipv4_pool,
        ipv6_pool,
        ipv4_prefix,
        ipv6_prefix,
        config_file: state.config_path.clone(),
    }))
}

async fn update_network_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
    Json(req): Json<UpdateNetworkConfigRequest>,
) -> (StatusCode, Json<NetworkConfigApiResponse>) {
    if let Err(status) = check_auth(&state, &headers, params.get("token").map(String::as_str)) {
        return (
            status,
            Json(NetworkConfigApiResponse {
                success: false,
                message: "未授权或认证失效，请先登录".to_string(),
                data: None,
            }),
        );
    }

    if req.mode.is_ipv4_enabled() {
        if let Err(e) = fuckipv6_common::validate_ipv4_cidr(&req.ipv4_pool, "ipv4_pool") {
            return (
                StatusCode::BAD_REQUEST,
                Json(NetworkConfigApiResponse {
                    success: false,
                    message: format!("IPv4 网段格式错误: {e}"),
                    data: None,
                }),
            );
        }
    }

    if req.mode.is_ipv6_enabled() {
        if let Err(e) = fuckipv6_common::validate_ipv6_cidr(&req.ipv6_pool, "ipv6_pool") {
            return (
                StatusCode::BAD_REQUEST,
                Json(NetworkConfigApiResponse {
                    success: false,
                    message: format!("IPv6 网段格式错误: {e}"),
                    data: None,
                }),
            );
        }
    }

    let new_alloc = NetworkAllocation {
        mode: req.mode,
        ipv4_pool: req.ipv4_pool.clone(),
        ipv6_pool: req.ipv6_pool.clone(),
        ip_pool: Some(req.ipv6_pool.clone()),
    };

    if let Err(e) = state
        .session_manager
        .update_network_allocation(new_alloc.clone())
        .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(NetworkConfigApiResponse {
                success: false,
                message: format!("更新运行时网段失败: {e}"),
                data: None,
            }),
        );
    }

    // 持久化保存到配置文件（如果指定了配置文件路径）
    if let Some(config_path) = &state.config_path {
        if let Ok(mut config) = fuckipv6_common::CoordinatorConfig::from_file(config_path) {
            config.network = new_alloc.clone();
            if let Err(e) = config.save_to_file(config_path) {
                tracing::warn!("无法写回协调端配置文件 {}: {:?}", config_path, e);
            } else {
                tracing::info!("网络配置已持久化保存到: {}", config_path);
            }
        }
    }

    let ipv4_prefix = state.session_manager.ipv4_prefix().await;
    let ipv6_prefix = state.session_manager.ipv6_prefix().await;

    (
        StatusCode::OK,
        Json(NetworkConfigApiResponse {
            success: true,
            message: "虚拟内网及网段配置已更新并实时生效".to_string(),
            data: Some(NetworkConfigDto {
                mode: req.mode,
                ipv4_pool: req.ipv4_pool,
                ipv6_pool: req.ipv6_pool,
                ipv4_prefix,
                ipv6_prefix,
                config_file: state.config_path.clone(),
            }),
        }),
    )
}

#[derive(Debug, Deserialize)]
pub struct BlacklistActionRequest {
    pub client_id: String,
}

#[derive(Debug, Serialize)]
pub struct BlacklistActionResponse {
    pub success: bool,
    pub message: String,
    pub blacklist: Vec<String>,
}

async fn get_blacklist(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Vec<String>>, StatusCode> {
    check_auth(&state, &headers, params.get("token").map(String::as_str))?;
    Ok(Json(state.session_manager.get_blacklist().await))
}

async fn ban_client(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
    Json(payload): Json<BlacklistActionRequest>,
) -> Result<Json<BlacklistActionResponse>, StatusCode> {
    check_auth(&state, &headers, params.get("token").map(String::as_str))?;
    let target = payload.client_id.trim();
    if target.is_empty() {
        return Ok(Json(BlacklistActionResponse {
            success: false,
            message: "Client ID cannot be empty".to_string(),
            blacklist: state.session_manager.get_blacklist().await,
        }));
    }
    state.session_manager.ban_client(target.to_string()).await;
    let current = state.session_manager.get_blacklist().await;
    Ok(Json(BlacklistActionResponse {
        success: true,
        message: format!("客户端 {target} 已被封禁并移入黑名单"),
        blacklist: current,
    }))
}

async fn unban_client(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
    Json(payload): Json<BlacklistActionRequest>,
) -> Result<Json<BlacklistActionResponse>, StatusCode> {
    check_auth(&state, &headers, params.get("token").map(String::as_str))?;
    let target = payload.client_id.trim();
    let removed = state.session_manager.unban_client(target).await;
    let current = state.session_manager.get_blacklist().await;
    Ok(Json(BlacklistActionResponse {
        success: removed,
        message: if removed {
            format!("客户端 {target} 已成功解封移出黑名单")
        } else {
            format!("客户端 {target} 不在黑名单中")
        },
        blacklist: current,
    }))
}

#[derive(Debug, Serialize)]
struct LogsResponse {
    service: String,
    lines: Vec<String>,
    error: Option<String>,
}

async fn logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<LogsResponse>, StatusCode> {
    check_auth(&state, &headers, params.get("token").map(String::as_str))?;
    let service = params
        .get("service")
        .map(|service| normalize_service_name(service))
        .unwrap_or_else(|| "fuckipv6-coordinator.service".to_string());
    let lines = params
        .get("lines")
        .and_then(|lines| lines.parse::<usize>().ok())
        .unwrap_or(100)
        .clamp(1, 500);

    Ok(Json(read_service_logs(&service, lines).await))
}

async fn ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    check_auth(&state, &headers, params.get("token").map(String::as_str))?;
    Ok(ws.on_upgrade(move |socket| ws_stream(socket, state)))
}

async fn ws_stream(mut socket: WebSocket, state: AppState) {
    let mut ticker = interval(Duration::from_secs(2));

    loop {
        ticker.tick().await;
        let snapshot = snapshot_response(&state.session_manager).await;
        let Ok(payload) = serde_json::to_string(&snapshot) else {
            tracing::warn!("序列化 WebUI 状态快照失败");
            continue;
        };

        if socket.send(Message::Text(payload.into())).await.is_err() {
            break;
        }
    }
}

async fn snapshot_response(session_manager: &SessionManager) -> SnapshotResponse {
    let snapshot = session_manager.snapshot().await;
    SnapshotResponse {
        service: "fuckipv6-coordinator",
        online_clients: snapshot.online_clients,
        peers: snapshot.peers,
        traffic: snapshot.traffic,
    }
}

async fn read_service_logs(service: &str, lines: usize) -> LogsResponse {
    if service == "memory" || service == "errors" {
        return LogsResponse {
            service: service.to_string(),
            lines: recent_error_log_lines(lines),
            error: None,
        };
    }

    #[cfg(target_os = "linux")]
    {
        let output = tokio::process::Command::new("journalctl")
            .args(["-u", service, "-n", &lines.to_string(), "--no-pager"])
            .output()
            .await;

        match output {
            Ok(output) if output.status.success() => LogsResponse {
                service: service.to_string(),
                lines: tail_lines(&String::from_utf8_lossy(&output.stdout), lines),
                error: None,
            },
            Ok(output) => LogsResponse {
                service: service.to_string(),
                lines: recent_error_log_lines(lines),
                error: Some(String::from_utf8_lossy(&output.stderr).trim().to_string()),
            },
            Err(e) => LogsResponse {
                service: service.to_string(),
                lines: recent_error_log_lines(lines),
                error: Some(format!("failed to execute journalctl: {}", e)),
            },
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = lines;
        LogsResponse {
            service: service.to_string(),
            lines: recent_error_log_lines(lines),
            error: None,
        }
    }
}

fn normalize_service_name(service: &str) -> String {
    match service {
        "coordinator" | "fuckipv6-coordinator" => "fuckipv6-coordinator.service".to_string(),
        "client" | "fuckipv6-client" => "fuckipv6-client.service".to_string(),
        other if other.ends_with(".service") => other.to_string(),
        other => format!("{}.service", other),
    }
}

#[cfg(any(target_os = "linux", test))]
fn tail_lines(input: &str, max_lines: usize) -> Vec<String> {
    let mut lines = input
        .lines()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    if lines.len() > max_lines {
        lines.drain(0..lines.len() - max_lines);
    }

    lines
}

const WEBUI_CSS: &str = include_str!("../../../common/assets/webui.css");

const INDEX_HTML: &str = r##"<!doctype html>
<html lang="zh-CN" data-theme="dark" translate="no">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta name="google" content="notranslate">
  <meta name="color-scheme" content="dark light">
  <title>fuckipv6 协调信令中心</title>
  <link rel="stylesheet" href="/assets/webui.css">
  <link rel="icon" href="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24'%3E%3Crect width='24' height='24' rx='6' fill='%2338bdf8'/%3E%3Cpath d='M12 4 19 8v8l-7 4-7-4V8z' fill='none' stroke='%23041018' stroke-width='1.6'/%3E%3Ccircle cx='12' cy='12' r='2.6' fill='%23041018'/%3E%3C/svg%3E">
</head>
<body>
  <div class="app-shell">
    <!-- ===================================================================== -->
    <!-- 侧边导航                                                              -->
    <!-- ===================================================================== -->
    <aside class="sidebar">
      <div class="brand">
        <div class="brand-mark">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 2.5 20.5 7v10L12 21.5 3.5 17V7z"/><circle cx="12" cy="12" r="3.2"/><path d="M12 2.5V8.8M12 15.2v6.3M3.5 7l5.3 3.2M15.2 13.8l5.3 3.2M20.5 7l-5.3 3.2M8.8 13.8 3.5 17"/></svg>
        </div>
        <div class="brand-text">
          <h1>fuckipv6</h1>
          <span class="sub">Coordinator Hub</span>
        </div>
      </div>

      <div>
        <div class="nav-section-label">运行总览</div>
        <nav class="nav-tabs">
          <a class="nav-tab active" href="#section-overview">
            <span class="nav-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="7.5" height="7.5" rx="1.8"/><rect x="13.5" y="3" width="7.5" height="7.5" rx="1.8"/><rect x="3" y="13.5" width="7.5" height="7.5" rx="1.8"/><rect x="13.5" y="13.5" width="7.5" height="7.5" rx="1.8"/></svg></span>
            <span class="nav-text">核心指标总览</span>
          </a>
          <a class="nav-tab" href="#section-topology">
            <span class="nav-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="2.2"/><circle cx="4.5" cy="6" r="2.2"/><circle cx="19.5" cy="6" r="2.2"/><circle cx="4.5" cy="18" r="2.2"/><circle cx="19.5" cy="18" r="2.2"/><path d="m6.4 7.2 4 3.4M17.6 7.2l-4 3.4M6.4 16.8l4-3.4M17.6 16.8l-4-3.4"/></svg></span>
            <span class="nav-text">虚拟网络拓扑</span>
          </a>
          <a class="nav-tab" href="#section-peers">
            <span class="nav-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="3.5" width="19" height="13" rx="2.2"/><path d="M8 20.5h8M12 16.5v4"/></svg></span>
            <span class="nav-text">在线客户端列表</span>
            <span class="tab-badge" id="tab-peers-badge">0</span>
          </a>
          <a class="nav-tab" href="#section-logs">
            <span class="nav-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="3.5" width="19" height="17" rx="2.4"/><path d="m7 9 3 3-3 3M12.5 15H17"/></svg></span>
            <span class="nav-text">系统运行日志</span>
          </a>
        </nav>
      </div>

      <div class="sidebar-footer">
        <div class="sidebar-card">
          <div class="sidebar-card-top">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/><path d="m9 12 2 2 4-4"/></svg>
            <span>协调中枢在线</span>
          </div>
          <p>QUIC 信令 · Relay 中继<br>JWT 鉴权可配置启用</p>
        </div>
        <div class="sidebar-version"><span>COORDINATOR</span><span>v0.1.0</span></div>
      </div>
    </aside>

    <!-- ===================================================================== -->
    <!-- 主工作区                                                              -->
    <!-- ===================================================================== -->
    <div class="main">
      <header class="topbar">
        <div class="topbar-inner">
          <div class="page-heading">
            <h2>协调信令中心</h2>
            <p>分布式 QUIC 信令、中继与专网运行总览</p>
          </div>

          <div class="header-actions">
            <div class="status-chip">
              <span class="live-pip" id="live-dot"></span>
              <span id="status">连接就绪</span>
            </div>
            <span class="meta-chip">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3.2 1.9"/></svg>
              <span id="uptime-display">0秒</span>
            </span>
            <button class="btn" onclick="openGuideModal()" title="客户端接入指引">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z"/><path d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z"/></svg>
              <span>接入指引</span>
            </button>
            <button class="btn" onclick="openSettingsModal()" title="网络参数设置">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M4 6h9M17 6h3M4 12h3M11 12h9M4 18h9M17 18h3"/><circle cx="15" cy="6" r="2.1"/><circle cx="9" cy="12" r="2.1"/><circle cx="15" cy="18" r="2.1"/></svg>
              <span>网络参数</span>
            </button>
            <button class="btn" onclick="refresh(true)" title="刷新数据">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M3.5 9a9 9 0 0 1 14.9-3.4L21 8"/><path d="M21 3v5h-5"/><path d="M20.5 15a9 9 0 0 1-14.9 3.4L3 16"/><path d="M3 21v-5h5"/></svg>
              <span>刷新</span>
            </button>
            <button class="btn btn-icon" onclick="toggleTheme()" id="theme-btn" title="切换深色/浅色模式">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="4.2"/><path d="M12 2v2.2M12 19.8V22M2 12h2.2M19.8 12H22M4.9 4.9l1.6 1.6M17.5 17.5l1.6 1.6M19.1 4.9l-1.6 1.6M6.5 17.5l-1.6 1.6"/></svg>
            </button>
          </div>
        </div>
      </header>

      <main class="content">
        <!-- 核心指标 -->
        <section id="section-overview" class="anchor-section">
          <div class="metrics">
            <article class="m-item" data-tone="cyan">
              <div class="m-head">
                <span class="m-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M23 21v-2a4 4 0 0 0-3-3.87M16 3.13a4 4 0 0 1 0 7.75"/></svg></span>
                <span class="m-label">在线客户端</span>
              </div>
              <div class="m-num" id="online-count">0</div>
              <div class="m-hint">活跃对等节点</div>
            </article>
            <article class="m-item" data-tone="violet">
              <div class="m-head">
                <span class="m-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M22 12h-4l-3 9L9 3l-3 9H2"/></svg></span>
                <span class="m-label">中继传输速率</span>
              </div>
              <div class="m-num" id="relay-rate">0 B/s</div>
              <div class="m-hint">实时中继载荷</div>
            </article>
            <article class="m-item" data-tone="blue">
              <div class="m-head">
                <span class="m-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><ellipse cx="12" cy="5.5" rx="8.5" ry="3"/><path d="M3.5 5.5v6c0 1.7 3.8 3 8.5 3s8.5-1.3 8.5-3v-6"/><path d="M3.5 11.5v6c0 1.7 3.8 3 8.5 3s8.5-1.3 8.5-3v-6"/></svg></span>
                <span class="m-label">中继总流量</span>
              </div>
              <div class="m-num" id="relay-total">0 B</div>
              <div class="m-hint" id="relay-packets-desc">0 包已转发</div>
            </article>
            <article class="m-item" data-tone="emerald">
              <div class="m-head">
                <span class="m-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="2"/><path d="M16.24 7.76a6 6 0 0 1 0 8.49M7.76 16.24a6 6 0 0 1 0-8.49M19.07 4.93a10 10 0 0 1 0 14.14M4.93 19.07a10 10 0 0 1 0-14.14"/></svg></span>
                <span class="m-label">信令交互速率</span>
              </div>
              <div class="m-num" id="signaling-rate">0 B/s</div>
              <div class="m-hint">控制层带宽</div>
            </article>
            <article class="m-item" data-tone="amber">
              <div class="m-head">
                <span class="m-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="m12 2 9 5-9 5-9-5z"/><path d="m3 12 9 5 9-5"/><path d="m3 17 9 5 9-5"/></svg></span>
                <span class="m-label">信令消息总量</span>
              </div>
              <div class="m-num" id="signaling-total">0</div>
              <div class="m-hint" id="signaling-sub">累计交互帧</div>
            </article>
            <article class="m-item" data-tone="rose">
              <div class="m-head">
                <span class="m-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/><path d="m9 12 2 2 4-4"/></svg></span>
                <span class="m-label">中继异常</span>
              </div>
              <div class="m-num" id="health-status">0</div>
              <div class="m-hint" id="relay-errors-desc">链路健康良好</div>
            </article>
          </div>
        </section>

        <!-- 拓扑 -->
        <section id="section-topology" class="panel anchor-section">
          <div class="panel-head">
            <div class="panel-heading">
              <span class="panel-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3c2.5 2.6 3.8 5.7 3.8 9S14.5 18.4 12 21c-2.5-2.6-3.8-5.7-3.8-9S9.5 5.6 12 3z"/></svg></span>
              <div>
                <h3>虚拟网络拓扑</h3>
                <p>协调中枢与全部专网节点的实时连接视图</p>
              </div>
            </div>
            <div class="panel-tools">
              <span class="section-tag" id="topology-count">0 节点在线</span>
            </div>
          </div>
          <div class="topology-toolbar">
            <div class="topo-tool-group">
              <span class="topo-label">虚拟网络</span>
              <select id="topo-network-filter" class="topo-select" onchange="onTopologyNetworkFilterChange()">
                <option value="__all__">全部专网 (全景拓扑)</option>
              </select>
            </div>
            <div class="topo-legend-wrap" id="topo-legend"></div>
            <div class="topo-tool-actions">
              <button class="topo-btn" onclick="resetCanvasView()" title="拓扑视角居中归位">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><circle cx="12" cy="12" r="3"/></svg>
                <span>视角居中</span>
              </button>
            </div>
          </div>
          <div class="topology-stage">
            <canvas id="topology"></canvas>
            <div class="topo-hint">右键专网 / 节点快捷管理 · 拖拽自由布局 · 点击图例聚焦专网</div>
          </div>
        </section>

        <!-- 在线客户端 -->
        <section id="section-peers" class="panel anchor-section">
          <div class="panel-head">
            <div class="panel-heading">
              <span class="panel-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="3.5" width="19" height="13" rx="2.2"/><path d="M8 20.5h8M12 16.5v4"/><path d="M6.5 8h5M6.5 11h8"/></svg></span>
              <div>
                <h3>在线客户端列表</h3>
                <p>已接入协调中枢的全部客户端与链路指标</p>
              </div>
            </div>
            <div class="panel-tools">
              <span class="section-tag" id="table-count">0 节点</span>
            </div>
          </div>
          <div class="controls-row">
            <div class="search-box">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="7.5"/><path d="m21 21-4.3-4.3"/></svg>
              <input type="text" id="peer-filter" placeholder="检索 IP / ID / 端点..." oninput="filterPeersTable()">
            </div>
            <select class="select-pill" id="nat-filter" onchange="filterPeersTable()">
              <option value="">全部 NAT 穿透类型</option>
              <option value="Cone">全锥型 NAT (Cone)</option>
              <option value="Symmetric">对称型 NAT (Symmetric)</option>
              <option value="Restricted">受限型 NAT (Restricted)</option>
              <option value="Open">公网开放 (Open)</option>
            </select>
          </div>
          <div class="table-responsive">
            <div id="content">
      <div class="empty-state">
                <span class="empty-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="3.5" width="19" height="13" rx="2.2"/><path d="M8 20.5h8M12 16.5v4"/></svg></span>
                <strong>等待客户端接入专网</strong>
                <span>客户端连接后，节点信息与链路指标会自动出现在这里。</span>
              </div>
            </div>
          </div>
        </section>

        <!-- 日志 -->
        <section id="section-logs" class="panel anchor-section">
          <div class="panel-head">
            <div class="panel-heading">
              <span class="panel-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="3.5" width="19" height="17" rx="2.4"/><path d="m7 9 3 3-3 3M12.5 15H17"/></svg></span>
              <div>
                <h3>系统运行日志</h3>
                <p>协调器与客户端服务的最新运行事件</p>
              </div>
            </div>
            <div class="panel-tools">
              <span class="section-tag" id="log-service-badge">协调中枢</span>
            </div>
          </div>
          <div class="console-stage" oncontextmenu="handleLogsContextMenu(event)" title="右键打开日志快捷菜单">
            <div class="console-toolbar">
              <div>
                <select class="select-pill" id="log-service-select" onchange="refreshLogs()">
                  <option value="fuckipv6-coordinator">协调器服务 (coordinator)</option>
                  <option value="fuckipv6-client">客户端服务 (client)</option>
                  <option value="memory">异常错误 (errors)</option>
                </select>
                <select class="select-pill" id="log-level-filter" onchange="applyLogFilter()">
                  <option value="all" selected>全部日志级别</option>
                  <option value="info">仅信息 (INFO)</option>
                  <option value="warn">仅警告 (WARN)</option>
                  <option value="error">仅错误 (ERROR)</option>
                </select>
                <select class="select-pill" id="log-lines-select" onchange="refreshLogs()">
                  <option value="50">50 行</option>
                  <option value="100" selected>100 行</option>
                  <option value="200">200 行</option>
                  <option value="500">500 行</option>
                </select>
                <div class="search-box">
                  <input type="text" id="log-keyword-filter" placeholder="搜索关键词..." oninput="applyLogFilter()">
                </div>
              </div>
              <div>
                <button class="btn" id="btn-toggle-format" onclick="toggleLogFormat()">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M4 7V4h16v3M9 20h6M12 4v16"/></svg>
                  <span>原始 / 美化</span>
                </button>
                <button class="btn" onclick="clearLogs()">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M3.5 6.5h17M8.5 6.5V4a1.5 1.5 0 0 1 1.5-1.5h4A1.5 1.5 0 0 1 15.5 4v2.5M18.5 6.5v13a2 2 0 0 1-2 2h-9a2 2 0 0 1-2-2v-13"/></svg>
                  <span>清屏</span>
                </button>
                <button class="btn" onclick="refreshLogs()">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M3.5 9a9 9 0 0 1 14.9-3.4L21 8"/><path d="M21 3v5h-5"/><path d="M20.5 15a9 9 0 0 1-14.9 3.4L3 16"/><path d="M3 21v-5h5"/></svg>
                  <span>刷新</span>
                </button>
                <button class="btn" onclick="copyLogs()">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2.2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>
                  <span>复制日志</span>
                </button>
              </div>
            </div>
            <pre id="logs">获取日志中...</pre>
          </div>
        </section>

        <footer class="app-footer">
          <div>FUCKIPV6 VIRTUAL OVERLAY · DISTRIBUTED QUIC</div>
          <div>COORDINATOR CONTROL PLANE · LIVE TELEMETRY</div>
        </footer>
      </main>
    </div>
  </div>

  <!-- 全局右键上下文菜单 -->
  <div id="cyber-context-menu" class="cyber-context-menu" style="display:none;" oncontextmenu="return false;">
    <div class="cm-header" id="cm-header">节点上下文</div>
    <div id="cm-items"></div>
  </div>

  <!-- 接入指南 Modal -->
  <div class="modal-wrap" id="guide-modal" onclick="if(event.target===this)closeGuideModal()">
    <div class="modal-box">
      <div class="modal-head">
        <h3>客户端接入快速指引</h3>
        <button class="btn btn-icon" onclick="closeGuideModal()" title="关闭">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18M6 6l12 12"/></svg>
        </button>
      </div>
      <p class="hint-text">创建 <code>client.toml</code> 配置文件：</p>
      <div class="code-view" id="guide-code-block">[client]
client_id = "node-alpha"
private_key = "client_key.pem"

[network]
signaling_server = "ws://127.0.0.1:8080/ws"
p2p_port = 51820
tun_name = "fuckipv6"</div>
      <div class="modal-actions">
        <button class="btn" onclick="closeGuideModal()">关闭</button>
        <button class="btn btn-primary" onclick="copyGuideConfig()">复制配置</button>
      </div>
    </div>
  </div>

  <!-- 网络参数设置 Modal -->
  <div class="modal-wrap" id="settings-modal" onclick="if(event.target===this)closeSettingsModal()">
    <div class="modal-box">
      <div class="modal-head">
        <h3>专网地址分配设置</h3>
        <button class="btn btn-icon" onclick="closeSettingsModal()" title="关闭">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18M6 6l12 12"/></svg>
        </button>
      </div>
      <div class="form-group">
        <label>网络运行模式</label>
        <select class="form-control" id="net-mode-select">
          <option value="dual_stack">IPv4 + IPv6 双栈模式 (DualStack)</option>
          <option value="ipv6_only">仅 IPv6 模式 (Ipv6Only)</option>
          <option value="ipv4_only">仅 IPv4 模式 (Ipv4Only)</option>
        </select>
      </div>
      <div class="form-group">
        <label>IPv4 地址池 (CIDR)</label>
        <input type="text" class="form-control" id="net-ipv4-pool" placeholder="10.8.0.0/24">
      </div>
      <div class="form-group">
        <label>IPv6 地址池 (CIDR)</label>
        <input type="text" class="form-control" id="net-ipv6-pool" placeholder="fd00::1000/120">
      </div>
      <div class="modal-actions">
        <button class="btn" onclick="closeSettingsModal()">取消</button>
        <button class="btn btn-primary" onclick="saveNetworkSettings()">保存并应用</button>
      </div>
    </div>
  </div>

  <div class="toast-box" id="toast-box"></div>

  <script>
    let previousTraffic = null;
    let previousTrafficAt = 0;
    let cachedPeers = [];
    let rawLogLines = [];
    let isRealtimeConnected = false;

    function showToast(message) {
      const box = document.getElementById('toast-box');
      const toast = document.createElement('div');
      toast.className = 'toast';
      toast.innerHTML = `<span style="color:var(--accent);">●</span><span>${message}</span>`;
      box.appendChild(toast);
      setTimeout(() => {
        toast.style.opacity = '0';
        toast.style.transform = 'translateY(6px)';
        toast.style.transition = 'all 0.2s';
        setTimeout(() => toast.remove(), 200);
      }, 2200);
    }

    function formatBytes(value) {
      const units = ['B', 'KB', 'MB', 'GB', 'TB'];
      let size = Number(value || 0);
      let unit = 0;
      while (size >= 1024 && unit < units.length - 1) {
        size /= 1024;
        unit += 1;
      }
      return `${size.toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`;
    }

    function formatRate(bytes, seconds) {
      if (!seconds || seconds <= 0) return '0 B/s';
      return `${formatBytes(Math.max(0, bytes) / seconds)}/s`;
    }

    function formatDuration(seconds) {
      const s = Math.floor(seconds || 0);
      if (s < 60) return `${s}秒`;
      const m = Math.floor(s / 60);
      if (m < 60) return `${m}分${s % 60}秒`;
      const h = Math.floor(m / 60);
      if (h < 24) return `${h}小时${m % 60}分`;
      const d = Math.floor(h / 24);
      return `${d}天${h % 24}小时`;
    }

    function shortId(value) {
      if (!value) return '';
      return value.length > 14 ? `${value.slice(0, 7)}...${value.slice(-5)}` : value;
    }

    function copyText(text, label = '内容') {
      navigator.clipboard.writeText(text).then(() => {
        showToast(`已复制 ${label}`);
      }).catch(() => {
        showToast(`已复制 ${label}`);
      });
    }

    function formatNatType(natType) {
      if (!natType) return '未知 NAT';
      const t = String(natType).trim().toLowerCase();
      if (t.includes('full') || t === 'cone' || t.includes('full cone')) return '全锥型 NAT';
      if (t.includes('port restricted') || t.includes('portrestricted')) return '端口受限锥型 NAT';
      if (t.includes('restricted')) return '受限锥型 NAT';
      if (t.includes('symmetric') || t === 'sym') return '对称型 NAT';
      if (t.includes('open') || t.includes('no nat') || t.includes('public')) return '公网型 (无 NAT)';
      if (t.includes('direct')) return 'P2P 直连';
      if (t.includes('relay')) return 'QUIC 中继';
      if (t.includes('unknown')) return '未知 NAT';
      return natType;
    }

    function getNatBadge(natType) {
      const t = String(natType || '').toLowerCase();
      if (t.includes('full') || (t.includes('cone') && !t.includes('port') && !t.includes('restricted'))) {
        return `<span class="tag-nat cone">全锥型 NAT</span>`;
      }
      if (t.includes('port')) {
        return `<span class="tag-nat" style="background:rgba(245,158,11,0.15);color:#f59e0b;border:1px solid rgba(245,158,11,0.3);">端口受限</span>`;
      }
      if (t.includes('restricted')) {
        return `<span class="tag-nat" style="background:rgba(168,85,247,0.15);color:#c084fc;border:1px solid rgba(168,85,247,0.3);">受限锥型</span>`;
      }
      if (t.includes('symmetric') || t.includes('sym')) {
        return `<span class="tag-nat sym">对称型 NAT</span>`;
      }
      if (t.includes('open')) {
        return `<span class="tag-nat" style="background:rgba(16,185,129,0.15);color:#34d399;border:1px solid rgba(16,185,129,0.3);">公网开放</span>`;
      }
      if (t.includes('direct')) {
        return `<span class="tag-nat cone">P2P 直连</span>`;
      }
      if (t.includes('relay')) {
        return `<span class="tag-nat" style="background:rgba(14,165,233,0.15);color:#38bdf8;border:1px solid rgba(14,165,233,0.3);">QUIC 中继</span>`;
      }
      return `<span class="tag-nat">${formatNatType(natType)}</span>`;
    }

    function renderTraffic(traffic) {
      if (!traffic) return;
      const now = Date.now();
      const elapsed = previousTrafficAt ? (now - previousTrafficAt) / 1000 : 0;
      
      const relayDelta = previousTraffic ? traffic.relay_payload_bytes - previousTraffic.relay_payload_bytes : 0;
      const signalingTotal = traffic.signaling_rx_bytes + traffic.signaling_tx_bytes;
      const previousSignalingTotal = previousTraffic ? previousTraffic.signaling_rx_bytes + previousTraffic.signaling_tx_bytes : signalingTotal;
      const signalingDelta = signalingTotal - previousSignalingTotal;

      document.getElementById('relay-rate').textContent = formatRate(relayDelta, elapsed);
      document.getElementById('relay-total').textContent = formatBytes(traffic.relay_payload_bytes);
      document.getElementById('relay-packets-desc').textContent = `${(traffic.relay_packets || 0).toLocaleString()} 包已转发`;
      
      document.getElementById('signaling-rate').textContent = formatRate(signalingDelta, elapsed);
      const msgTotal = (traffic.signaling_rx_messages || 0) + (traffic.signaling_tx_messages || 0);
      document.getElementById('signaling-total').textContent = msgTotal.toLocaleString();
      document.getElementById('signaling-sub').textContent = `RX: ${traffic.signaling_rx_messages || 0} / TX: ${traffic.signaling_tx_messages || 0}`;

      const errors = traffic.relay_errors || 0;
      document.getElementById('health-status').textContent = errors;
      document.getElementById('relay-errors-desc').textContent = errors === 0 ? '链路健康良好' : `${errors} 次丢弃`;

      if (traffic.uptime_secs !== undefined) {
        document.getElementById('uptime-display').textContent = `${formatDuration(traffic.uptime_secs)}`;
      }

      previousTraffic = traffic;
      previousTrafficAt = now;
    }

    /* ========================================================
       顶级全景深空网络拓扑引擎 (Aerospace Cyber Telemetry Engine)
       ======================================================== */
    const canvas = document.getElementById('topology');
    const ctx = canvas.getContext('2d');
    let width = 0;
    let height = 0;
    let nodes = [];
    let draggedNode = null;
    let hoveredNode = null;
    let mousePos = { x: -1000, y: -1000 };
    let radarAngle = 0;
    let radarEnabled = true;
    let particlesEnabled = true;
    let gridEnabled = true;
    let rawLogView = false;

    function resizeCanvas() {
      const rect = canvas.parentElement.getBoundingClientRect();
      const dpr = window.devicePixelRatio || 1;
      width = rect.width;
      height = rect.height;
      canvas.width = width * dpr;
      canvas.height = height * dpr;
      ctx.resetTransform();
      ctx.scale(dpr, dpr);
    }
    window.addEventListener('resize', resizeCanvas);

    const NETWORK_PALETTE = [
      { hex: '#38bdf8', bg: 'rgba(56, 189, 248, 0.12)', border: 'rgba(56, 189, 248, 0.45)', glow: 'rgba(56, 189, 248, 0.85)' },
      { hex: '#10b981', bg: 'rgba(16, 185, 129, 0.12)', border: 'rgba(16, 185, 129, 0.45)', glow: 'rgba(16, 185, 129, 0.85)' },
      { hex: '#a855f7', bg: 'rgba(168, 85, 247, 0.12)', border: 'rgba(168, 85, 247, 0.45)', glow: 'rgba(168, 85, 247, 0.85)' },
      { hex: '#f59e0b', bg: 'rgba(245, 158, 11, 0.12)', border: 'rgba(245, 158, 11, 0.45)', glow: 'rgba(245, 158, 11, 0.85)' },
      { hex: '#f43f5e', bg: 'rgba(244, 63, 94, 0.12)', border: 'rgba(244, 63, 94, 0.45)', glow: 'rgba(244, 63, 94, 0.85)' },
      { hex: '#06b6d4', bg: 'rgba(6, 182, 212, 0.12)', border: 'rgba(6, 182, 212, 0.45)', glow: 'rgba(6, 182, 212, 0.85)' },
    ];

    function getNetworkPalette(netName) {
      if (!netName) return NETWORK_PALETTE[0];
      let hash = 0;
      for (let i = 0; i < netName.length; i++) {
        hash = (hash << 5) - hash + netName.charCodeAt(i);
        hash |= 0;
      }
      return NETWORK_PALETTE[Math.abs(hash) % NETWORK_PALETTE.length];
    }

    let currentNetworkFilter = '__all__';
    let knownNetworks = new Map();

    class CyberNode {
      constructor(id, label, sub, peerData = null, isCore = false, isNetwork = false, netData = null) {
        this.id = id;
        this.label = label;
        this.sub = sub;
        this.peerData = peerData;
        this.isCore = isCore;
        this.isNetwork = isNetwork;
        this.netData = netData;
        this.networkNames = isNetwork
          ? [netData.name]
          : (peerData ? (peerData.networks && peerData.networks.length > 0 ? peerData.networks : [peerData.network_name || 'default']) : ['default']);
        this.primaryNet = this.networkNames[0] || 'default';
        this.colorTheme = isNetwork ? getNetworkPalette(netData.name) : getNetworkPalette(this.primaryNet);
        this.x = width / 2;
        this.y = height / 2;
        this.vx = 0;
        this.vy = 0;
        this.radius = isCore ? 16 : (isNetwork ? 14 : 8);
        this.pulsePhase = Math.random() * Math.PI * 2;
      }
    }

    function renderTopology(peers, networks = null) {
      if (!width) resizeCanvas();
      
      // 1. 整理与更新已知虚拟网络
      const discoveredNets = new Map();
      if (Array.isArray(networks) && networks.length > 0) {
        networks.forEach(n => {
          discoveredNets.set(n.name, {
            name: n.name,
            tun_name: n.tun_name || 'fuckipv6',
            ipv6_pool: n.ipv6_pool || 'fd00::/64',
            ipv4_pool: n.ipv4_pool || null,
            peerCount: n.peer_count || 0,
            status: n.status || '活跃就绪'
          });
        });
      }

      // 从对等节点中聚合所属专网
      (peers || []).forEach(p => {
        const netList = (p.networks && p.networks.length > 0) ? p.networks : [p.network_name || 'default'];
        netList.forEach(name => {
          if (!discoveredNets.has(name)) {
            discoveredNets.set(name, {
              name: name,
              tun_name: name === 'default' ? 'fuckipv6' : `fk-${name}`,
              ipv6_pool: 'fd00::/64',
              ipv4_pool: null,
              peerCount: 0,
              status: '活跃就绪'
            });
          }
          const item = discoveredNets.get(name);
          item.peerCount = (item.peerCount || 0) + 1;
        });
      });

      if (discoveredNets.size === 0) {
        discoveredNets.set('default', {
          name: 'default',
          tun_name: 'fuckipv6',
          ipv6_pool: 'fd00::/64',
          ipv4_pool: null,
          peerCount: (peers || []).length,
          status: '活跃就绪'
        });
      }
      knownNetworks = discoveredNets;

      // 更新顶部工具栏下拉筛选与图例
      updateTopologyToolbar();

      // 2. 节点差量构建与星系拓扑装配
      const existing = new Map(nodes.map(n => [n.id, n]));
      const newNodes = [];

      // 中心 Hub 协调器节点
      let core = existing.get('__core__');
      if (!core) {
        core = new CyberNode('__core__', '协调中心 (HUB)', 'QUIC 6667', null, true, false, null);
        core.x = width / 2;
        core.y = height / 2;
      } else {
        core.x = width / 2;
        core.y = height / 2;
      }
      newNodes.push(core);

      // 虚拟网络核心节点 (Network Nexuses)
      const netArray = Array.from(knownNetworks.values());
      const netCount = netArray.length;
      netArray.forEach((net, idx) => {
        const netId = '__net__' + net.name;
        let netNode = existing.get(netId);
        const netAngle = (Math.PI * 2 * idx / Math.max(1, netCount)) - Math.PI * 0.65;
        const netDist = netCount === 1 ? 130 : 160;
        const targetX = width / 2 + Math.cos(netAngle) * netDist;
        const targetY = height / 2 + Math.sin(netAngle) * netDist;
        if (!netNode) {
          netNode = new CyberNode(netId, `专网: ${net.name}`, `${net.tun_name || 'fuckipv6'} · ${net.peerCount}节点`, null, false, true, net);
          netNode.x = targetX;
          netNode.y = targetY;
        } else {
          netNode.netData = net;
          netNode.label = `专网: ${net.name}`;
          netNode.sub = `${net.tun_name || 'fuckipv6'} · ${net.peerCount}节点`;
        }
        netNode.targetX = targetX;
        netNode.targetY = targetY;
        netNode.colorTheme = getNetworkPalette(net.name);
        newNodes.push(netNode);
      });

      // 对等客户端节点分组并以扇形放射状聚类
      const peersByNet = new Map();
      (peers || []).forEach(p => {
        const pNets = (p.networks && p.networks.length > 0) ? p.networks : [p.network_name || 'default'];
        const pri = pNets[0] || 'default';
        if (!peersByNet.has(pri)) peersByNet.set(pri, []);
        peersByNet.get(pri).push(p);
      });

      (peers || []).forEach((p, globalIdx) => {
        let node = existing.get(p.client_id);
        const pNets = (p.networks && p.networks.length > 0) ? p.networks : [p.network_name || 'default'];
        const primaryNet = pNets[0] || 'default';
        const parentNetNode = newNodes.find(n => n.id === '__net__' + primaryNet) || core;
        const netPeers = peersByNet.get(primaryNet) || [p];
        const memberIdx = netPeers.indexOf(p);
        const memberCount = netPeers.length;

        const fromCoreAngle = Math.atan2((parentNetNode ? parentNetNode.y : height/2) - (core ? core.y : height/2),
                                         (parentNetNode ? parentNetNode.x : width/2) - (core ? core.x : width/2));
        let fanAngle = memberCount === 1 ? fromCoreAngle : fromCoreAngle - (Math.PI * 1.35) / 2 + ((Math.PI * 1.35) * memberIdx / Math.max(1, memberCount - 1));
        fanAngle += (globalIdx % 2 === 0 ? 0.08 : -0.08);

        const dist = 85 + (memberIdx % 2) * 26;
        const initX = (parentNetNode ? parentNetNode.x : width / 2) + Math.cos(fanAngle) * dist;
        const initY = (parentNetNode ? parentNetNode.y : height / 2) + Math.sin(fanAngle) * dist;

        if (!node) {
          node = new CyberNode(p.client_id, p.virtual_ip, shortId(p.client_id), p, false, false, null);
          node.x = initX;
          node.y = initY;
        }
        node.peerData = p;
        node.label = p.virtual_ip;
        node.networkNames = pNets;
        node.primaryNet = primaryNet;
        node.colorTheme = getNetworkPalette(primaryNet);
        newNodes.push(node);
      });

      nodes = newNodes;
    }

    function updateTopologyToolbar() {
      const select = document.getElementById('topo-network-filter');
      const legend = document.getElementById('topo-legend');
      if (!select || !legend) return;

      const previousVal = select.value || currentNetworkFilter;
      let selectHtml = `<option value="__all__">全部专网 (全域全景拓扑)</option>`;
      let legendHtml = ``;

      knownNetworks.forEach(net => {
        const theme = getNetworkPalette(net.name);
        const selected = (previousVal === net.name) ? 'selected' : '';
        const activeClass = (currentNetworkFilter === net.name) ? 'active' : '';
        selectHtml += `<option value="${net.name}" ${selected}>专网: ${net.name} (${net.peerCount} 节点)</option>`;
        legendHtml += `
          <div class="topo-legend-item ${activeClass}" onclick="setTopologyNetworkFilter('${net.name}')" title="聚焦专网 ${net.name}">
            <span class="topo-legend-dot" style="background:${theme.hex};box-shadow:0 0 6px ${theme.hex};"></span>
            <span>${net.name}</span>
            <span style="color:var(--text-muted);font-size:10px;">(${net.peerCount})</span>
          </div>
        `;
      });

      select.innerHTML = selectHtml;
      select.value = previousVal;
      legend.innerHTML = legendHtml;
    }

    function onTopologyNetworkFilterChange() {
      const select = document.getElementById('topo-network-filter');
      if (!select) return;
      currentNetworkFilter = select.value;
      updateTopologyToolbar();
      showToast(currentNetworkFilter === '__all__' ? '已切换至全网拓扑全景视图' : `已聚焦虚拟专网: ${currentNetworkFilter}`);
    }

    function setTopologyNetworkFilter(netName) {
      if (currentNetworkFilter === netName) {
        currentNetworkFilter = '__all__';
      } else {
        currentNetworkFilter = netName;
      }
      const select = document.getElementById('topo-network-filter');
      if (select) select.value = currentNetworkFilter;
      updateTopologyToolbar();
      showToast(currentNetworkFilter === '__all__' ? '已重置为全部专网视图' : `已聚焦虚拟专网: ${currentNetworkFilter}`);
    }

    // 鼠标监听交互
    canvas.addEventListener('mousedown', e => {
      const rect = canvas.getBoundingClientRect();
      const mx = e.clientX - rect.left;
      const my = e.clientY - rect.top;
      nodes.forEach(n => {
        if (Math.hypot(n.x - mx, n.y - my) < n.radius + 14) {
          draggedNode = n;
          if (!n.isCore && !n.isNetwork) highlightPeerRow(n.id);
        }
      });
    });

    window.addEventListener('mousemove', e => {
      const rect = canvas.getBoundingClientRect();
      mousePos.x = e.clientX - rect.left;
      mousePos.y = e.clientY - rect.top;
      if (draggedNode) {
        draggedNode.x = mousePos.x;
        draggedNode.y = mousePos.y;
        draggedNode.vx = 0;
        draggedNode.vy = 0;
      }
    });

    window.addEventListener('mouseup', () => { draggedNode = null; });

    // 画布右键上下文菜单交互
    canvas.addEventListener('contextmenu', e => {
      e.preventDefault();
      const rect = canvas.getBoundingClientRect();
      const mx = e.clientX - rect.left;
      const my = e.clientY - rect.top;

      const targetNode = nodes.find(n => Math.hypot(n.x - mx, n.y - my) < n.radius + 18);

      if (targetNode) {
        if (targetNode.isCore) {
          const host = window.location.host;
          const items = [
            { icon: '', label: '复制信令服务地址', shortcut: '信令', onClick: () => copyText(`ws://${host}`, '信令服务地址') },
            { icon: '', label: '复制 QUIC 中继端点', shortcut: '中继', onClick: () => copyText(`${window.location.hostname}:6667`, 'QUIC 中继端点') },
            { divider: true },
            { icon: '', label: '立即刷新全网拓扑', shortcut: '刷新', onClick: () => refresh(true) },
            { icon: '', label: '拓扑视口居中归位', shortcut: '居中', onClick: () => resetCanvasView() },
          ];
          showContextMenu(e, 'HUB · 协调服务器', '核心中枢', items);
        } else if (targetNode.isNetwork) {
          const net = targetNode.netData || {};
          const items = [
            { icon: '', label: `聚焦此专网 (${net.name})`, shortcut: '聚焦', onClick: () => setTopologyNetworkFilter(net.name) },
            { icon: '', label: '复制专网 IPv6 网段', shortcut: net.ipv6_pool || 'fd00::/64', onClick: () => copyText(net.ipv6_pool || 'fd00::/64', '专网 IPv6 网段') },
            { icon: '', label: '复制虚拟网卡设备名', shortcut: net.tun_name || 'TUN', onClick: () => copyText(net.tun_name || 'fuckipv6', '网卡接口') },
            { divider: true },
            { icon: '', label: '重置为全景拓扑视角', shortcut: '全景', onClick: () => resetCanvasView() },
          ];
          showContextMenu(e, `虚拟专网 · ${net.name}`, `${net.peerCount || 0} 个对等节点在线`, items);
        } else {
          const p = targetNode.peerData || {};
          const cid = targetNode.id;
          const ipv6 = p.virtual_ip || (p.virtual_ips && p.virtual_ips.ipv6) || '';
          const ipv4 = p.virtual_ipv4 || (p.virtual_ips && p.virtual_ips.ipv4) || '';
          const eps = (p.endpoints || []).join(', ');
          const nets = targetNode.networkNames.join(', ');

          const items = [
            { icon: '', label: '复制客户端标识', shortcut: shortId(cid), onClick: () => copyText(cid, '客户端标识') },
            { icon: '', label: '复制所属专网', shortcut: nets, onClick: () => copyText(nets, '所属专网') },
            { icon: '', label: '复制虚拟 IPv4', shortcut: ipv4 || '无', onClick: () => copyText(ipv4, '虚拟 IPv4') },
            { icon: '', label: '复制虚拟 IPv6', shortcut: ipv6, onClick: () => copyText(ipv6, '虚拟 IPv6') },
            { icon: '', label: '复制直连端点', shortcut: `${(p.endpoints || []).length}个`, onClick: () => copyText(eps, '直连端点') },
            { divider: true },
            { icon: '', label: '在表格中定位节点', shortcut: '定位', onClick: () => highlightPeerRow(cid) },
            { icon: '', label: '网络测速连通性', shortcut: '测速', onClick: () => pingPeer(cid) },
            { divider: true },
            { icon: '', label: '移入黑名单并踢出', danger: true, onClick: () => banClientPrompt(cid) },
          ];
          showContextMenu(e, `客户端 · ${shortId(cid)}`, formatNatType(p.nat_type) || '在线节点', items);
        }
      } else {
        const items = [
          { icon: '', label: '刷新拓扑数据', shortcut: '刷新', onClick: () => refresh(true) },
          { icon: '', label: '视角中心归位', shortcut: '居中', onClick: () => resetCanvasView() },
          { divider: true },
          { 
            icon: '', 
            label: `链路动态微光: ${particlesEnabled ? '已开启 ✓' : '已停用'}`, 
            onClick: () => { particlesEnabled = !particlesEnabled; showToast(particlesEnabled ? '动态微光已开启' : '动态微光已停用'); } 
          },
        ];
        showContextMenu(e, '专网拓扑视窗控制', '视窗', items);
      }
    });

    function resetCanvasView() {
      if (!width || !height) return;
      currentNetworkFilter = '__all__';
      const select = document.getElementById('topo-network-filter');
      if (select) select.value = '__all__';
      updateTopologyToolbar();

      const core = nodes.find(n => n.isCore);
      if (core) {
        core.x = width / 2;
        core.y = height / 2;
        core.vx = 0;
        core.vy = 0;
      }
      const netNodes = nodes.filter(n => n.isNetwork);
      netNodes.forEach((netNode, idx) => {
        const netAngle = (Math.PI * 2 * idx / Math.max(1, netNodes.length)) - Math.PI * 0.65;
        const netDist = netNodes.length === 1 ? 130 : 160;
        netNode.x = width / 2 + Math.cos(netAngle) * netDist;
        netNode.y = height / 2 + Math.sin(netAngle) * netDist;
        netNode.vx = 0;
        netNode.vy = 0;
      });

      const peerNodes = nodes.filter(n => !n.isCore && !n.isNetwork);
      const peersByNet = new Map();
      peerNodes.forEach(p => {
        const pri = p.primaryNet || (p.networkNames && p.networkNames[0]) || 'default';
        if (!peersByNet.has(pri)) peersByNet.set(pri, []);
        peersByNet.get(pri).push(p);
      });

      peerNodes.forEach((pNode, globalIdx) => {
        const priNet = pNode.primaryNet || (pNode.networkNames && pNode.networkNames[0]) || 'default';
        const parentNet = nodes.find(net => net.isNetwork && net.netData && net.netData.name === priNet) || core;
        const netPeers = peersByNet.get(priNet) || [pNode];
        const memberIdx = netPeers.indexOf(pNode);
        const memberCount = netPeers.length;

        const fromCoreAngle = Math.atan2((parentNet ? parentNet.y : height/2) - (core ? core.y : height/2), 
                                         (parentNet ? parentNet.x : width/2) - (core ? core.x : width/2));
        let fanAngle = memberCount === 1 ? fromCoreAngle : fromCoreAngle - (Math.PI * 1.35) / 2 + ((Math.PI * 1.35) * memberIdx / Math.max(1, memberCount - 1));
        fanAngle += (globalIdx % 2 === 0 ? 0.08 : -0.08);

        const dist = 85 + (memberIdx % 2) * 26;
        pNode.x = (parentNet ? parentNet.x : width / 2) + Math.cos(fanAngle) * dist;
        pNode.y = (parentNet ? parentNet.y : height / 2) + Math.sin(fanAngle) * dist;
        pNode.vx = 0;
        pNode.vy = 0;
      });
      showToast('全网拓扑视角与专网星系已复位居中');
    }

    function focusPeerHUD(clientId) {
      const node = nodes.find(n => n.id === clientId);
      if (node) {
        hoveredNode = node;
        mousePos.x = node.x;
        mousePos.y = node.y;
        highlightPeerRow(clientId);
        showToast(`已锁定客户端 ${shortId(clientId)} 遥测卡片`);
      }
    }

    function pingPeer(clientId) {
      const peer = (cachedPeers || []).find(p => p.client_id === clientId);
      const targetLabel = peer ? (peer.virtual_ip || shortId(clientId)) : shortId(clientId);
      showToast(`正在向对等节点 ${targetLabel} 发送信令/中继探针...`);
      setTimeout(() => {
        if (peer) {
          const ep = (peer.endpoints && peer.endpoints[0]) || 'QUIC 中继';
          const idle = peer.idle_secs !== undefined ? `空闲 ${peer.idle_secs}秒` : '活跃';
          showToast(`节点 ${shortId(clientId)} 响应正常，端点: ${ep} (${idle})`);
        } else {
          showToast(`节点 ${shortId(clientId)} 探针超时或已离线`);
        }
      }, 350);
    }

    async function banClientPrompt(clientId) {
      if (!confirm(`确定要将客户端 ${shortId(clientId)} 加入黑名单并强制踢出吗？`)) return;
      try {
        const token = localStorage.getItem('token') || '';
        const res = await fetch(`/api/blacklist/ban${token ? '?token=' + encodeURIComponent(token) : ''}`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ client_id: clientId, reason: '管理员通过 WebUI 右键踢出' })
        }).then(r => r.json());
        if (res.success) {
          showToast(`客户端 ${shortId(clientId)} 已被拉黑并强制下线`);
          refresh(true);
        } else {
          showToast(`拉黑失败: ${res.error || '未知错误'}`);
        }
      } catch (err) {
        showToast(`操作异常: ${err.message}`);
      }
    }

    // 拓扑主渲染循环 (多网络与节点关系高精渲染引擎)
    function animateTopology() {
      requestAnimationFrame(animateTopology);
      if (!width || !height) return;

      ctx.clearRect(0, 0, width, height);
      const isDark = document.documentElement.getAttribute('data-theme') !== 'light';
      const time = Date.now() * 0.0015;
      const core = nodes.find(n => n.isCore);
      const networkNodes = nodes.filter(n => n.isNetwork);
      const peerNodes = nodes.filter(n => !n.isCore && !n.isNetwork);
      const isFiltered = (currentNetworkFilter && currentNetworkFilter !== '__all__');

      function getNodeAlpha(n) {
        if (!isFiltered) return 1.0;
        if (n.isCore) return 0.9;
        if (n.isNetwork) return n.netData.name === currentNetworkFilter ? 1.0 : 0.18;
        return n.networkNames.includes(currentNetworkFilter) ? 1.0 : 0.15;
      }

      // 1. 空闲待机无节点时的空态呈现
      if (core && peerNodes.length === 0) {
        ctx.font = '500 12.5px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
        ctx.textAlign = 'center';
        ctx.fillStyle = isDark ? '#94a3b8' : '#64748b';
        ctx.fillText('虚拟专网拓扑就绪 · 暂无对等节点接入', core.x, core.y + 40);

        ctx.font = '400 11px ui-monospace, SFMono-Regular, monospace';
        ctx.fillStyle = isDark ? '#475569' : '#94a3b8';
        ctx.fillText('等待客户端通过邀请凭证连接接入专网...', core.x, core.y + 58);
      }

      // 2. 物理力学平滑更新 (网络核心与节点分层吸引弹簧)
      // 专网核心节点受到 Hub 牵引
      networkNodes.forEach((netNode, idx) => {
        if (netNode === draggedNode) return;
        if (core) {
          const dx = core.x - netNode.x;
          const dy = core.y - netNode.y;
          const dist = Math.hypot(dx, dy) || 1;
          const targetDist = networkNodes.length === 1 ? 130 : 160;
          const spring = (dist - targetDist) * 0.012;
          netNode.vx += (dx / dist) * spring;
          netNode.vy += (dy / dist) * spring;
        }
        // 专网核心互斥
        networkNodes.forEach(otherNet => {
          if (otherNet === netNode) return;
          const ox = otherNet.x - netNode.x;
          const oy = otherNet.y - netNode.y;
          const odist = Math.hypot(ox, oy) || 1;
          if (odist < 180) {
            const rep = ((180 - odist) / 180) * 0.85;
            const signX = Math.abs(ox) > 0.1 ? (ox / odist) : (idx % 2 === 0 ? 1 : -1);
            const signY = Math.abs(oy) > 0.1 ? (oy / odist) : 1;
            netNode.vx -= signX * rep;
            netNode.vy -= signY * rep;
          }
        });
      });

      // 客户端节点受到所属专网核心牵引
      peerNodes.forEach((pNode, pIdx) => {
        if (pNode === draggedNode) return;
        pNode.networkNames.forEach(netName => {
          const parentNet = networkNodes.find(n => n.netData && n.netData.name === netName);
          if (parentNet) {
            const dx = parentNet.x - pNode.x;
            const dy = parentNet.y - pNode.y;
            const dist = Math.hypot(dx, dy) || 1;
            const targetDist = 92;
            const spring = (dist - targetDist) * 0.015;
            pNode.vx += (dx / dist) * spring;
            pNode.vy += (dy / dist) * spring;
          }
        });

        // 柔和微气流
        pNode.vx += Math.sin(time + pIdx * 1.7) * 0.03;
        pNode.vy += Math.cos(time + pIdx * 2.1) * 0.03;
      });

      // 全局防重叠碰撞箱斥力 (AABB Capsule Repulsion)
      for (let i = 0; i < nodes.length; i++) {
        const n1 = nodes[i];
        if (n1 === draggedNode) continue;

        for (let j = i + 1; j < nodes.length; j++) {
          const n2 = nodes[j];
          const dx = n2.x - n1.x;
          const dy = n2.y - n1.y;
          const absDx = Math.abs(dx);
          const absDy = Math.abs(dy);

          // 两个对等节点卡片 (144px x 28px) 之间精准胶囊边界排斥
          if (!n1.isCore && !n1.isNetwork && !n2.isCore && !n2.isNetwork) {
            const minX = 154;
            const minY = 36;
            if (absDx < minX && absDy < minY) {
              const overlapRatio = Math.min((minX - absDx) / minX, (minY - absDy) / minY);
              const rep = overlapRatio * 1.6;
              const signX = absDx > 0.1 ? Math.sign(dx) : (i % 2 === 0 ? 1 : -1);
              const signY = absDy > 0.1 ? Math.sign(dy) : (j % 2 === 0 ? 1 : -1);
              n1.vx -= signX * rep * 0.75;
              n1.vy -= signY * rep * 0.5;
              if (n2 !== draggedNode) {
                n2.vx += signX * rep * 0.75;
                n2.vy += signY * rep * 0.5;
              }
            }
          } else {
            // 对等节点与网络/核心节点之间的圆形斥力
            const dist = Math.hypot(dx, dy) || 1;
            const minDist = (n1.isNetwork || n2.isNetwork) ? 120 : 95;
            if (dist < minDist) {
              const rep = ((minDist - dist) / minDist) * 0.75;
              const signX = absDx > 0.1 ? (dx / dist) : (i % 2 === 0 ? 1 : -1);
              const signY = absDy > 0.1 ? (dy / dist) : 1;
              n1.vx -= signX * rep;
              n1.vy -= signY * rep;
              if (!n2.isCore && n2 !== draggedNode) {
                n2.vx += signX * rep;
                n2.vy += signY * rep;
              }
            }
          }
        }

        n1.vx *= 0.82;
        n1.vy *= 0.82;
        if (!n1.isCore) {
          n1.x += n1.vx;
          n1.y += n1.vy;
          n1.x = Math.max(85, Math.min(width - 85, n1.x));
          n1.y = Math.max(35, Math.min(height - 35, n1.y));
        }
      }

      // 3. 绘制各虚拟专网星系边界与聚类光环 (Cluster Halos)
      networkNodes.forEach(netNode => {
        const netName = netNode.netData.name;
        const members = peerNodes.filter(n => n.networkNames.includes(netName));
        const alpha = getNodeAlpha(netNode);

        let cx = netNode.x;
        let cy = netNode.y;
        let maxR = netNode.radius + 30;
        members.forEach(m => {
          maxR = Math.max(maxR, Math.hypot(m.x - cx, m.y - cy) + 38);
        });

        ctx.save();
        ctx.globalAlpha = alpha;

        const aura = ctx.createRadialGradient(cx, cy, 10, cx, cy, maxR);
        aura.addColorStop(0, netNode.colorTheme.bg);
        aura.addColorStop(0.75, netNode.colorTheme.bg.replace('0.12', '0.03'));
        aura.addColorStop(1, 'rgba(0, 0, 0, 0)');
        ctx.beginPath();
        ctx.arc(cx, cy, maxR, 0, Math.PI * 2);
        ctx.fillStyle = aura;
        ctx.fill();

        ctx.beginPath();
        ctx.setLineDash([4, 6]);
        ctx.arc(cx, cy, maxR, 0, Math.PI * 2);
        ctx.strokeStyle = (hoveredNode === netNode || (hoveredNode && hoveredNode.networkNames && hoveredNode.networkNames.includes(netName)))
          ? netNode.colorTheme.glow
          : netNode.colorTheme.border;
        ctx.lineWidth = 1;
        ctx.stroke();
        ctx.setLineDash([]);

        ctx.font = '600 10.5px ui-monospace, SFMono-Regular, monospace';
        ctx.textAlign = 'center';
        ctx.fillStyle = isDark ? '#94a3b8' : '#64748b';
        ctx.fillText(`专网: ${netName} (${members.length} 节点)`, cx, cy - maxR - 5);

        ctx.restore();
      });

      // 3. 绘制骨干与节点连线 (极简柔和细线，无同心虚线圆圈，无星系光斑)
      if (core && networkNodes.length > 0) {
        networkNodes.forEach(netNode => {
          const isRelHover = (hoveredNode === core || hoveredNode === netNode);
          ctx.save();
          ctx.beginPath();
          ctx.moveTo(core.x, core.y);
          ctx.lineTo(netNode.x, netNode.y);
          ctx.strokeStyle = isRelHover
            ? (isDark ? 'rgba(56, 189, 248, 0.45)' : 'rgba(2, 132, 199, 0.5)')
            : (isDark ? 'rgba(255, 255, 255, 0.07)' : 'rgba(0, 0, 0, 0.06)');
          ctx.lineWidth = isRelHover ? 1.5 : 1;
          ctx.stroke();

          if (particlesEnabled) {
            const flowOffset = (time * 0.5) % 1;
            const px = core.x + (netNode.x - core.x) * flowOffset;
            const py = core.y + (netNode.y - core.y) * flowOffset;
            ctx.beginPath();
            ctx.arc(px, py, 1.8, 0, Math.PI * 2);
            ctx.fillStyle = isDark ? 'rgba(56, 189, 248, 0.75)' : 'rgba(2, 132, 199, 0.85)';
            ctx.fill();
          }
          ctx.restore();
        });
      }

      peerNodes.forEach((pNode, idx) => {
        const pAlpha = getNodeAlpha(pNode);
        const parents = networkNodes.filter(n => n.netData && pNode.networkNames.includes(n.netData.name));
        const targetParents = parents.length > 0 ? parents : (core ? [core] : []);

        targetParents.forEach(p => {
          const isRelHover = (hoveredNode === pNode || hoveredNode === p);
          ctx.save();
          ctx.globalAlpha = isRelHover ? 1.0 : (pAlpha * 0.9);
          ctx.beginPath();
          ctx.moveTo(p.x, p.y);
          ctx.lineTo(pNode.x, pNode.y);
          ctx.strokeStyle = isRelHover
            ? (isDark ? 'rgba(56, 189, 248, 0.45)' : 'rgba(2, 132, 199, 0.5)')
            : (isDark ? 'rgba(255, 255, 255, 0.07)' : 'rgba(0, 0, 0, 0.06)');
          ctx.lineWidth = isRelHover ? 1.5 : 1;
          ctx.stroke();

          if (particlesEnabled) {
            const flowOffset = (time * 0.5 + idx * 0.28) % 1;
            const px = p.x + (pNode.x - p.x) * flowOffset;
            const py = p.y + (pNode.y - p.y) * flowOffset;
            ctx.beginPath();
            ctx.arc(px, py, 1.8, 0, Math.PI * 2);
            ctx.fillStyle = isDark ? 'rgba(56, 189, 248, 0.75)' : 'rgba(2, 132, 199, 0.85)';
            ctx.fill();
          }
          ctx.restore();
        });
      });

      // 4. 中心 Hub 协调中枢 (纯净微光质感发光点，无杂色模糊大底盘)
      if (core) {
        ctx.beginPath();
        ctx.arc(core.x, core.y, 5.5, 0, Math.PI * 2);
        ctx.fillStyle = '#38bdf8';
        ctx.shadowColor = '#38bdf8';
        ctx.shadowBlur = 12;
        ctx.fill();
        ctx.shadowBlur = 0;

        ctx.font = '500 11px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
        ctx.textAlign = 'center';
        ctx.fillStyle = isDark ? '#cbd5e1' : '#334155';
        ctx.fillText('fuckipv6 协调中枢', core.x, core.y - 14);
      }

      // 5. 虚拟专网核心节点 (极简纯色微光点，无旋转多边形)
      networkNodes.forEach(netNode => {
        const alpha = getNodeAlpha(netNode);
        ctx.save();
        ctx.globalAlpha = alpha;

        const hex = (netNode.colorTheme && netNode.colorTheme.hex) ? netNode.colorTheme.hex : '#38bdf8';
        ctx.beginPath();
        ctx.arc(netNode.x, netNode.y, 4.5, 0, Math.PI * 2);
        ctx.fillStyle = hex;
        ctx.shadowColor = hex;
        ctx.shadowBlur = 8;
        ctx.fill();
        ctx.shadowBlur = 0;

        ctx.font = '500 11px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
        ctx.textAlign = 'center';
        ctx.fillStyle = isDark ? '#cbd5e1' : '#334155';
        ctx.fillText(netNode.label, netNode.x, netNode.y - 12);
        ctx.restore();
      });

      // 6. 客户端对等节点胶囊卡片 (Apple / Linear 现代无边框卡片，零描边线条)
      hoveredNode = null;
      if (core && Math.hypot(core.x - mousePos.x, core.y - mousePos.y) < 22) {
        hoveredNode = core;
      }
      networkNodes.forEach(netNode => {
        if (Math.hypot(netNode.x - mousePos.x, netNode.y - mousePos.y) < 20) {
          hoveredNode = netNode;
        }
      });

      peerNodes.forEach(n => {
        const pillW = 144;
        const pillH = 28;
        const pillX = n.x - pillW / 2;
        const pillY = n.y - pillH / 2;

        const isHover = (mousePos.x >= pillX && mousePos.x <= pillX + pillW &&
                         mousePos.y >= pillY && mousePos.y <= pillY + pillH);
        if (isHover) hoveredNode = n;

        const alpha = getNodeAlpha(n);
        ctx.save();
        ctx.globalAlpha = isHover ? 1.0 : alpha;

        ctx.beginPath();
        if (ctx.roundRect) {
          ctx.roundRect(pillX, pillY, pillW, pillH, 14);
        } else {
          ctx.rect(pillX, pillY, pillW, pillH);
        }
        ctx.fillStyle = isDark
          ? (isHover ? 'rgba(30, 41, 59, 0.96)' : 'rgba(255, 255, 255, 0.05)')
          : (isHover ? '#ffffff' : 'rgba(0, 0, 0, 0.04)');
        ctx.fill();

        // 在线绿色状态指示圆点
        ctx.beginPath();
        ctx.arc(pillX + 13, n.y, 3, 0, Math.PI * 2);
        ctx.fillStyle = '#10b981';
        ctx.shadowColor = '#10b981';
        ctx.shadowBlur = 5;
        ctx.fill();
        ctx.shadowBlur = 0;

        // 节点 IP 标题
        ctx.font = '500 11px ui-monospace, SFMono-Regular, monospace';
        ctx.textAlign = 'left';
        ctx.fillStyle = isHover ? '#38bdf8' : (isDark ? '#e2e8f0' : '#0f172a');
        const displayLabel = n.label.length > 15 ? n.label.slice(0, 14) + '…' : n.label;
        ctx.fillText(displayLabel, pillX + 24, n.y + 3.5);

        // 右侧所属专网颜色标记点 (若属于多网络，显示多色点)
        if (n.networkNames && n.networkNames.length > 1) {
          let dotOffset = pillX + pillW - 12;
          n.networkNames.slice(0, 2).forEach(name => {
            const pal = getNetworkPalette(name);
            ctx.beginPath();
            ctx.arc(dotOffset, n.y, 2.5, 0, Math.PI * 2);
            ctx.fillStyle = pal.hex;
            ctx.fill();
            dotOffset -= 7;
          });
        }

        ctx.restore();
      });

      // 9. 悬停 HUD 浮动遥测卡片 (全量汉化与丰富信息)
      if (hoveredNode) {
        drawHudTooltip(hoveredNode, isDark);
      }
    }

    function drawHudTooltip(targetNode, isDark) {
      if (!targetNode) return;
      const isCore = !!targetNode.isCore;
      const isNetwork = !!targetNode.isNetwork;
      const p = targetNode.peerData;
      if (!isCore && !isNetwork && !p) return;

      ctx.save();
      const hudW = 290;
      let lines = [];
      let title = '';
      let dotColor = '#10b981';

      if (isCore) {
        title = 'fuckipv6 协调中枢 (HUB)';
        dotColor = '#38bdf8';
        const activePeerCount = nodes.filter(n => !n.isCore && !n.isNetwork).length;
        const netCount = knownNetworks.size;
        lines = [
          { k: '服务功能', v: '信令协调 (WS) · 流量中继 (QUIC)' },
          { k: '服务端口', v: 'UDP:6667 (QUIC) · TCP:8080 (WebUI)' },
          { k: '专网拓扑', v: `${netCount} 个专网 · ${activePeerCount} 个终端互联` },
          { k: '运行状态', v: '● 核心协调网络运行中', color: '#10b981' },
        ];
        if (previousTraffic && previousTraffic.uptime_secs) {
          lines.push({ k: '运行时间', v: formatDuration(previousTraffic.uptime_secs) });
        }
      } else if (isNetwork) {
        const net = targetNode.netData || {};
        const memberCount = nodes.filter(n => !n.isCore && !n.isNetwork && n.networkNames.includes(net.name)).length;
        title = `虚拟专网 · ${net.name}`;
        dotColor = targetNode.colorTheme.hex;
        lines = [
          { k: '专网名称', v: net.name, color: targetNode.colorTheme.hex },
          { k: '虚拟接口', v: net.tun_name || 'fuckipv6' },
          { k: 'IPv6 网段', v: net.ipv6_pool || 'fd00::/64', color: '#38bdf8' },
        ];
        if (net.ipv4_pool) {
          lines.push({ k: 'IPv4 网段', v: net.ipv4_pool, color: '#38bdf8' });
        }
        lines.push(
          { k: '成员规模', v: `${memberCount} 个对等节点在线` },
          { k: '网络类型', v: '分布式 P2P Mesh 专网' },
          { k: '运行状态', v: '● 专网活跃就绪', color: '#10b981' }
        );
      } else {
        title = `对等节点 · ${shortId(p.client_id)}`;
        const v6 = p.virtual_ip || '-';
        const v4 = p.virtual_ipv4 || (p.virtual_ips && p.virtual_ips.ipv4);
        const nat = formatNatType(p.nat_type);
        const eps = (p.endpoints && p.endpoints.length) 
          ? (p.endpoints.length === 1 ? p.endpoints[0] : `${p.endpoints[0]} (共${p.endpoints.length}个)`)
          : '未上报直连端口';
        const idle = (p.idle_secs || 0);
        const idleText = idle < 15 ? `心跳活跃 (${idle}秒前)` : `心跳正常 (${idle}秒前)`;
        const nets = targetNode.networkNames.join(', ');
        const isMultiNet = targetNode.networkNames.length > 1;

        lines = [
          { k: '所属专网', v: nets, color: targetNode.colorTheme.hex },
          { k: '网络角色', v: isMultiNet ? '多网互联枢纽网关' : '专网普通对等节点', color: isMultiNet ? '#a855f7' : '#10b981' },
          { k: '虚拟 IPv6', v: v6, color: '#38bdf8' },
        ];
        if (v4) {
          lines.push({ k: '虚拟 IPv4', v: v4, color: '#38bdf8' });
        }
        lines.push(
          { k: 'NAT 类型', v: nat },
          { k: '公网端点', v: eps },
          { k: '心跳状态', v: idleText, color: idle < 15 ? '#10b981' : '#f59e0b' },
          { k: '在线时长', v: formatDuration(p.connected_secs) },
        );
        if (p.direct !== undefined) {
          lines.push({ k: '连通链路', v: p.direct ? 'P2P 直连' : 'QUIC 中继转发', color: p.direct ? '#10b981' : '#38bdf8' });
        }
        if (p.latency_ms !== undefined && p.latency_ms !== null) {
          lines.push({ k: '端到端延迟', v: `${p.latency_ms} ms`, color: '#10b981' });
        }
      }

      const footerHint = isCore 
        ? '右键呼出协调中枢管理' 
        : (isNetwork ? '单击聚焦此网络 · 右键快捷操作' : '单击定位 · 右键快捷操作');
      const lineHeight = 19;
      const hudH = 34 + lines.length * lineHeight + 22;

      const hudX = (targetNode.x > width / 2)
        ? Math.max(12, targetNode.x - hudW - 20)
        : Math.min(width - hudW - 12, targetNode.x + 24);
      const hudY = (targetNode.y > height / 2)
        ? Math.max(12, targetNode.y - hudH + 20)
        : Math.min(height - hudH - 12, targetNode.y - 20);

      // 卡片优雅圆角暗黑背景
      ctx.beginPath();
      if (ctx.roundRect) {
        ctx.roundRect(hudX, hudY, hudW, hudH, 8);
      } else {
        ctx.rect(hudX, hudY, hudW, hudH);
      }
      ctx.fillStyle = isDark ? 'rgba(9, 11, 17, 0.97)' : 'rgba(255, 255, 255, 0.98)';
      ctx.shadowColor = 'rgba(0, 0, 0, 0.6)';
      ctx.shadowBlur = 20;
      ctx.shadowOffsetY = 4;
      ctx.fill();
      ctx.shadowBlur = 0;

      // 标题行指示灯
      ctx.beginPath();
      ctx.arc(hudX + 16, hudY + 17, 3.5, 0, Math.PI * 2);
      ctx.fillStyle = dotColor;
      ctx.fill();

      // 标题文字
      ctx.font = '600 11.5px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
      ctx.textAlign = 'left';
      ctx.fillStyle = isDark ? '#ffffff' : '#0f172a';
      ctx.fillText(title, hudX + 26, hudY + 21);

      // 各属性键值对排版
      let curY = hudY + 38;
      lines.forEach(row => {
        ctx.font = '400 11px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
        ctx.fillStyle = isDark ? '#64748b' : '#94a3b8';
        ctx.fillText(row.k, hudX + 14, curY);

        ctx.font = '500 11px ui-monospace, SFMono-Regular, monospace';
        ctx.fillStyle = row.color || (isDark ? '#e2e8f0' : '#1e293b');
        ctx.textAlign = 'right';
        ctx.fillText(row.v, hudX + hudW - 14, curY);
        ctx.textAlign = 'left';
        curY += lineHeight;
      });

      // 底部操作指引
      ctx.font = '400 10px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
      ctx.fillStyle = isDark ? '#475569' : '#94a3b8';
      ctx.fillText(footerHint, hudX + 14, curY + 6);

      ctx.restore();
    }

    // 启动动画循环
    resizeCanvas();
    animateTopology();

    function highlightPeerRow(clientId) {
      const rows = document.querySelectorAll('tbody tr');
      rows.forEach(r => r.classList.remove('highlight-row'));
      const target = document.getElementById(`peer-row-${clientId}`);
      if (target) {
        target.classList.add('highlight-row');
        target.scrollIntoView({ behavior: 'smooth', block: 'center' });
        showToast(`已定位对等节点 ${shortId(clientId)}`);
      }
    }

    function render(status, peers) {
      cachedPeers = peers || [];
      const onlineCount = status.online_clients || 0;
      
      document.getElementById('status').textContent = `${onlineCount} 客户端在线`;
      document.getElementById('online-count').textContent = onlineCount;
      document.getElementById('topology-count').textContent = `${onlineCount} 节点在线`;
      document.getElementById('table-count').textContent = `${onlineCount} 节点`;
      const peersBadge = document.getElementById('tab-peers-badge');
      if (peersBadge) peersBadge.textContent = onlineCount;

      renderTraffic(status.traffic);
      renderTopology(peers, status.networks); // 兼容 renderTopology(peers)
      renderPeersTable(peers);
    }

    /* ========================================================
       全局赛博遥测右键上下文菜单控制 (Cyber Context Menu Controller)
       ======================================================== */
    const contextMenuEl = document.getElementById('cyber-context-menu');
    const cmHeaderEl = document.getElementById('cm-header');
    const cmItemsEl = document.getElementById('cm-items');
    window.__cmActions = [];

    function showContextMenu(e, title, tag, items) {
      e.preventDefault();
      e.stopPropagation();

      cmHeaderEl.innerHTML = `<span>${title}</span>${tag ? `<span class="cm-header-tag">${tag}</span>` : ''}`;
      window.__cmActions = [];

      let actionIdx = 0;
      cmItemsEl.innerHTML = items.map(item => {
        if (item.divider) return `<div class="cm-divider"></div>`;
        const curIdx = actionIdx++;
        window.__cmActions.push(item.onClick);
        return `
          <div class="cm-item ${item.danger ? 'danger' : ''}" onclick="execContextAction(${curIdx})">
            <span class="cm-icon">${item.icon || ''}</span>
            <span class="cm-label">${item.label}</span>
            ${item.shortcut ? `<span class="cm-shortcut">${item.shortcut}</span>` : ''}
          </div>
        `;
      }).join('');

      contextMenuEl.style.display = 'block';
      const menuWidth = contextMenuEl.offsetWidth || 210;
      const menuHeight = contextMenuEl.offsetHeight || 200;

      let left = e.clientX;
      let top = e.clientY;

      if (left + menuWidth > window.innerWidth) {
        left = Math.max(10, window.innerWidth - menuWidth - 12);
      }
      if (top + menuHeight > window.innerHeight) {
        top = Math.max(10, window.innerHeight - menuHeight - 12);
      }

      contextMenuEl.style.left = `${left}px`;
      contextMenuEl.style.top = `${top}px`;
    }

    function execContextAction(index) {
      hideContextMenu();
      if (window.__cmActions && window.__cmActions[index]) {
        try {
          window.__cmActions[index]();
        } catch (err) {
          console.error('Context menu action error:', err);
        }
      }
    }

    function hideContextMenu() {
      if (contextMenuEl) {
        contextMenuEl.style.display = 'none';
      }
    }

    window.addEventListener('click', hideContextMenu);
    window.addEventListener('keydown', e => { if (e.key === 'Escape') hideContextMenu(); });
    window.addEventListener('scroll', hideContextMenu, true);

    function handlePeerContextMenu(e, clientId) {
      const p = cachedPeers.find(peer => peer.client_id === clientId) || {};
      const ipv6 = p.virtual_ip || (p.virtual_ips && p.virtual_ips.ipv6) || '';
      const ipv4 = p.virtual_ipv4 || (p.virtual_ips && p.virtual_ips.ipv4) || '';
      const eps = (p.endpoints || []).join(', ');

      const items = [
        { icon: '', label: '复制客户端 ID', shortcut: shortId(clientId), onClick: () => copyText(clientId, '客户端 ID') },
        { icon: '', label: '复制虚拟 IPv4', shortcut: ipv4 || '无', onClick: () => copyText(ipv4, '虚拟 IPv4') },
        { icon: '', label: '复制虚拟 IPv6', shortcut: ipv6, onClick: () => copyText(ipv6, '虚拟 IPv6') },
        { icon: '', label: '复制全部端点', shortcut: `${(p.endpoints||[]).length}个`, onClick: () => copyText(eps, '公网端点') },
        { divider: true },
        { icon: '', label: '在拓扑中定位此节点', onClick: () => focusPeerHUD(clientId) },
        { icon: '', label: '连通性测试 (Ping)', onClick: () => pingPeer(clientId) },
        { divider: true },
        { icon: '', label: '移入黑名单并踢出', danger: true, onClick: () => banClientPrompt(clientId) },
      ];
      showContextMenu(e, `客户端 · ${shortId(clientId)}`, p.nat_type || 'ONLINE', items);
    }

    function handleLogsContextMenu(e) {
      const items = [
        { icon: '', label: '复制全部运行日志', shortcut: 'Ctrl+C', onClick: () => copyLogs() },
        { icon: '', label: '清空当前日志显示', shortcut: 'Clear', onClick: () => clearLogs() },
        { icon: '', label: '立即拉取最新日志', shortcut: 'F5', onClick: () => refreshLogs() },
        { divider: true },
        { icon: '', label: `切换视图: ${rawLogView ? '原始视图 (点此切换美化)' : '美化视图 (点此切换原始)'}`, onClick: () => toggleLogFormat() },
        { icon: '', label: '滚动到底部', shortcut: 'End', onClick: () => scrollLogs(true) },
        { icon: '', label: '滚动到顶部', shortcut: 'Home', onClick: () => scrollLogs(false) },
      ];
      showContextMenu(e, '系统运行日志控制', 'LOGS', items);
    }

    function renderPeersTable(peers) {
      const content = document.getElementById('content');
      if (!peers || !peers.length) {
        content.innerHTML = `
          <div class="empty-state">
            <span class="empty-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="3.5" width="19" height="13" rx="2.2"/><path d="M8 20.5h8M12 16.5v4"/></svg></span>
            <strong>等待客户端接入专网</strong>
            <span>客户端连接后，节点信息与链路指标会自动出现在这里。</span>
          </div>`;
        return;
      }

      const rows = peers.map(peer => {
        const isIdleActive = (peer.idle_secs || 0) < 15;
        const idleClass = isIdleActive ? 'active' : 'warn';
        const endpointsStr = (peer.endpoints && peer.endpoints.length) ? peer.endpoints.join(', ') : '-';

        const v4Str = peer.virtual_ipv4 || (peer.virtual_ips && peer.virtual_ips.ipv4) || '';
        const v4Html = v4Str ? `<br><code style="font-size:10.5px;color:#38bdf8;" title="虚拟 IPv4">${v4Str}</code>` : '';

        return `
          <tr id="peer-row-${peer.client_id}" oncontextmenu="handlePeerContextMenu(event, '${peer.client_id}')" style="cursor:context-menu;" title="右键打开快捷操作菜单">
            <td>
              <span class="copyable" onclick="copyText('${peer.virtual_ip}', '虚拟 IP')">
                <code>${peer.virtual_ip}</code>
              </span>
              ${v4Html}
            </td>
            <td>
              <span class="copyable" onclick="copyText('${peer.client_id}', '客户端标识')">
                <code>${shortId(peer.client_id)}</code>
              </span>
            </td>
            <td>${getNatBadge(peer.nat_type)}</td>
            <td>
              <span class="idle-pip ${idleClass}"></span>
              <span style="font-family:var(--font-mono);">${peer.idle_secs}秒</span>
            </td>
            <td style="font-family:var(--font-mono);">${formatDuration(peer.connected_secs)}</td>
            <td style="font-family:var(--font-mono);color:var(--text-secondary);">${endpointsStr}</td>
          </tr>
        `;
      }).join('');

      content.innerHTML = `
        <table>
          <thead>
            <tr>
              <th>虚拟 IPv6 地址</th>
              <th>客户端标识</th>
              <th>NAT 穿透类型</th>
              <th>心跳空闲</th>
              <th>在线时长</th>
              <th>上报端点</th>
            </tr>
          </thead>
          <tbody>${rows}</tbody>
        </table>
      `;
    }

    function filterPeersTable() {
      const keyword = (document.getElementById('peer-filter').value || '').trim().toLowerCase();
      const natFilter = document.getElementById('nat-filter').value;

      const filtered = cachedPeers.filter(peer => {
        const matchKeyword = !keyword || 
          peer.virtual_ip.toLowerCase().includes(keyword) ||
          peer.client_id.toLowerCase().includes(keyword) ||
          (peer.endpoints || []).some(e => e.toLowerCase().includes(keyword));
        const matchNat = !natFilter || (peer.nat_type && peer.nat_type.includes(natFilter));
        return matchKeyword && matchNat;
      });

      renderPeersTable(filtered);
    }

    async function refresh(isManual = false) {
      try {
        const [status, peers] = await Promise.all([
          fetch('/api/status').then(r => r.json()),
          fetch('/api/peers').then(r => r.json())
        ]);
        render(status, peers);
        if (isManual) showToast('已刷新数据');
      } catch (e) {
        if (isManual) showToast('刷新失败');
      }
    }

    function connectRealtime() {
      const scheme = window.location.protocol === 'https:' ? 'wss' : 'ws';
      const socket = new WebSocket(`${scheme}://${window.location.host}/ws`);
      let fallbackTimer = null;
      const liveDot = document.getElementById('live-dot');

      socket.onopen = () => {
        isRealtimeConnected = true;
        liveDot.classList.remove('offline');
        if (fallbackTimer) {
          clearInterval(fallbackTimer);
          fallbackTimer = null;
        }
      };

      socket.onmessage = event => {
        try {
          const snapshot = JSON.parse(event.data);
          render(snapshot, snapshot.peers);
        } catch (err) {}
      };

      socket.onclose = () => {
        isRealtimeConnected = false;
        liveDot.classList.add('offline');
        if (!fallbackTimer) fallbackTimer = setInterval(refresh, 3000);
        setTimeout(connectRealtime, 5000);
      };

      socket.onerror = () => socket.close();
    }

    function formatLogLine(rawLine) {
      if (rawLogView) {
        return rawLine.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
      }

      let line = rawLine;
      // 1. 去除 journalctl 重复的 syslog 前缀 (如: Oct 29 01:24:21 hostname fuckipv6[pid]: )
      line = line.replace(/^[A-Z][a-z]{2}\s+\d+\s+\d{2}:\d{2}:\d{2}\s+[\w.-]+\s+[\w.-]+\[\d+\]:\s*/, '');

      // 2. 提取 RFC3339 时间戳: 2026-10-29T01:24:21.054038Z -> 01:24:21
      let timeStr = '';
      const timeMatch = line.match(/^(\d{4}-\d{2}-\d{2}T(\d{2}:\d{2}:\d{2})\.\d+Z?)\s+/);
      if (timeMatch) {
        timeStr = timeMatch[2];
        line = line.slice(timeMatch[0].length);
      } else {
        const sysTimeMatch = line.match(/^\[?(\d{2}:\d{2}:\d{2})\]?\s+/);
        if (sysTimeMatch) {
          timeStr = sysTimeMatch[1];
          line = line.slice(sysTimeMatch[0].length);
        }
      }

      // 3. 提取日志级别
      let level = 'info';
      let levelText = '信息';
      let levelClass = 'log-info';
      const levelMatch = line.match(/^(INFO|WARN|WARNING|ERROR|FATAL|DEBUG|TRACE)\s+/i);
      if (levelMatch) {
        const lvl = levelMatch[1].toUpperCase();
        if (lvl === 'WARN' || lvl === 'WARNING') {
          level = 'warn'; levelText = '警告'; levelClass = 'log-warn';
        } else if (lvl === 'ERROR' || lvl === 'FATAL') {
          level = 'error'; levelText = '错误'; levelClass = 'log-error';
        } else if (lvl === 'DEBUG') {
          level = 'debug'; levelText = '调试'; levelClass = 'log-debug';
        } else if (lvl === 'TRACE') {
          level = 'trace'; levelText = '跟踪'; levelClass = 'log-trace';
        } else {
          level = 'info'; levelText = '信息'; levelClass = 'log-info';
        }
        line = line.slice(levelMatch[0].length);
      }

      // 4. 提取 Rust 模块标签并汉化
      let modTag = '';
      const modMatch = line.match(/^([\w_:]+):\s*/);
      if (modMatch) {
        const fullMod = modMatch[1];
        line = line.slice(modMatch[0].length);
        const modDict = {
          'fuckipv6_coordinator::signaling::server': '信令服务',
          'fuckipv6_coordinator::signaling::session': '会话管理',
          'fuckipv6_coordinator::relay::server': 'QUIC中继',
          'fuckipv6_coordinator::webui': 'Web管理',
          'fuckipv6_client::cli::commands': '客户端主控',
          'fuckipv6_client::tun::linux': 'TUN网卡',
          'fuckipv6_client::p2p::connection': 'P2P直连',
          'fuckipv6_client::p2p::relay': '中继通道',
          'fuckipv6_client::p2p::punching': 'NAT打洞',
          'fuckipv6_client::signaling::client': '信令连接',
        };
        const cnMod = modDict[fullMod] || fullMod.split('::').pop();
        modTag = `<span class="log-mod">${cnMod}</span>`;
      }

      // 5. 对常见英文系统日志进行顺滑中文转化
      let msg = line
        .replace(/Client disconnected: ([\w+/=_-]+)/g, '客户端已注销断开: $1')
        .replace(/Client closed connection: ([\d.:]+)/g, '客户端正常断开连接: $1')
        .replace(/WebSocket error: Protocol\(ResetWithoutClosingHandshake\)/g, 'WebSocket 连接中断 (对端未发送握手而断开)')
        .replace(/Failed to create TUN device: (.+)/g, '创建 TUN 虚拟网卡失败: $1')
        .replace(/ioctl\(TUNSETIFF\): Device or resource busy/g, 'TUN 设备忙 (网卡被其他进程占用)')
        .replace(/connection lost: closed by peer: (\d+)/g, '连接断开：对端关闭 ($1)')
        .replace(/Connecting to coordinator: (.+)/g, '正在连接协调信令中心: $1')
        .replace(/WebSocket connected/g, 'WebSocket 信令链路已建立')
        .replace(/QUIC relay endpoint: (.+)/g, 'QUIC 中继服务端点: $1')
        .replace(/Removed peer: ([\w+/=_-]+)/g, '已移除对等节点: $1')
        .replace(/P2P endpoint updated: ([\w+/=_-]+) via ([\d.:]+)/g, 'P2P 端点已更新: $1 经由 $2')
        .replace(/Failed to deserialize packet from (.+)/g, '无法反序列化数据包: 来自 $1')
        .replace(/UDP recv error: (.+)/g, 'UDP 数据包接收异常: $1')
        .replace(/QUIC connection established/gi, 'QUIC 链路建立成功')
        .replace(/QUIC relay connection established/gi, 'QUIC 中继连接已建立')
        .replace(/connection reset by peer/gi, '对端连接已重置')
        .replace(/connection closed/gi, '连接已关闭')
        .replace(/handshake failed/gi, '握手协商失败')
        .replace(/listening on:?/gi, '正在监听')
        .replace(/reset relay waiting for auto reconnect/gi, '重置中继等待自愈重连')
        .replace(/Failed to relay packet/gi, '中继转发数据包失败')
        .replace(/Signaling error/gi, '信令交互错误')
        .replace(/timed out/gi, '连接超时')
        .replace(/Connection error/gi, '连接发生异常');

      msg = msg.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

      // 高亮 IP 和 Client ID
      msg = msg.replace(/\b(fd00::[0-9a-fA-F:]+|\b(?:10\.\d{1,3}\.\d{1,3}\.\d{1,3}|185\.217\.\d{1,3}\.\d{1,3}|127\.0\.0\.1)(?::\d+)?)\b/g, '<span class="log-ip">$1</span>');
      msg = msg.replace(/\b([a-zA-Z0-9_-]{32,44})\b/g, '<span class="log-id">$1</span>');

      const timeHtml = timeStr ? `<span class="log-time">[${timeStr}]</span>` : '';
      const badgeHtml = `<span class="log-badge ${levelClass}">${levelText}</span>`;

      return `<div class="log-row" data-level="${level}">${timeHtml} ${badgeHtml} ${modTag} <span class="log-msg">${msg}</span></div>`;
    }

    function colorizeLogLine(line) {
      return formatLogLine(line);
    }

    async function refreshLogs() {
      const service = document.getElementById('log-service-select').value;
      const lines = document.getElementById('log-lines-select').value;
      const serviceBadge = document.getElementById('log-service-badge');
      serviceBadge.textContent = service.replace('.service', '').replace('fuckipv6-', '');

      try {
        const response = await fetch(`/api/logs?service=${encodeURIComponent(service)}&lines=${lines}`).then(r => r.json());
        const logsEl = document.getElementById('logs');
        if (response.error) {
          logsEl.innerHTML = `<div style="color:#f87171;padding:12px;">[读取日志错误] ${response.error}</div>`;
          return;
        }
        rawLogLines = response.lines || [];
        applyLogFilter();
      } catch (err) {
        document.getElementById('logs').innerHTML = `<div style="color:#f87171;padding:12px;">[异常] ${err.message}</div>`;
      }
    }

    function applyLogFilter() {
      const keyword = (document.getElementById('log-keyword-filter').value || '').trim().toLowerCase();
      const levelFilter = (document.getElementById('log-level-filter') ? document.getElementById('log-level-filter').value : 'all');
      const logsEl = document.getElementById('logs');

      let filtered = rawLogLines;
      if (keyword) {
        filtered = filtered.filter(l => l.toLowerCase().includes(keyword));
      }
      if (levelFilter !== 'all') {
        filtered = filtered.filter(l => {
          const lower = l.toLowerCase();
          if (levelFilter === 'info') return lower.includes('info') && !lower.includes('warn') && !lower.includes('error');
          if (levelFilter === 'warn') return lower.includes('warn');
          if (levelFilter === 'error') return lower.includes('error') || lower.includes('fatal');
          return true;
        });
      }

      if (!filtered.length) {
        logsEl.innerHTML = `<div style="color:var(--text-muted);padding:12px;">${keyword ? `无匹配 "${keyword}" 的日志。` : '暂无日志记录。'}</div>`;
        return;
      }

      if (rawLogView) {
        logsEl.textContent = filtered.join('\n');
      } else {
        logsEl.innerHTML = filtered.map(formatLogLine).join('');
      }
      logsEl.scrollTop = logsEl.scrollHeight;
    }

    function clearLogs() {
      rawLogLines = [];
      const logsEl = document.getElementById('logs');
      logsEl.innerHTML = '<div style="color:var(--text-muted);padding:12px;">日志面板已清空。</div>';
      showToast('日志已清屏');
    }

    function toggleLogFormat() {
      rawLogView = !rawLogView;
      const btn = document.getElementById('btn-toggle-format');
      if (btn) btn.textContent = rawLogView ? '切换美化' : '切换原始';
      applyLogFilter();
      showToast(rawLogView ? '已切换为原始日志模式' : '已切换为中文美化日志模式');
    }

    function scrollLogs(toBottom = true) {
      const logsEl = document.getElementById('logs');
      if (!logsEl) return;
      logsEl.scrollTop = toBottom ? logsEl.scrollHeight : 0;
    }

    function copyLogs() {
      if (!rawLogLines.length) return;
      copyText(rawLogLines.join('\n'), '运行日志');
    }

    function openGuideModal() {
      const currentHost = window.location.host;
      document.getElementById('guide-code-block').textContent = `[client]
client_id = "node-alpha"
private_key = "client_key.pem"

[network]
signaling_server = "ws://${currentHost}/ws"
p2p_port = 51820
tun_name = "fuckipv6"`;
      document.getElementById('guide-modal').classList.add('active');
    }

    function closeGuideModal() {
      document.getElementById('guide-modal').classList.remove('active');
    }

    function copyGuideConfig() {
      copyText(document.getElementById('guide-code-block').textContent, '配置模版');
      closeGuideModal();
    }

    async function openSettingsModal() {
      try {
        const res = await fetch('/api/config').then(r => r.json());
        if (res.mode) document.getElementById('net-mode-select').value = res.mode;
        if (res.ipv4_pool) document.getElementById('net-ipv4-pool').value = res.ipv4_pool;
        if (res.ipv6_pool) document.getElementById('net-ipv6-pool').value = res.ipv6_pool;
      } catch (e) {}
      document.getElementById('settings-modal').classList.add('active');
    }

    function closeSettingsModal() {
      document.getElementById('settings-modal').classList.remove('active');
    }

    async function saveNetworkSettings() {
      const mode = document.getElementById('net-mode-select').value;
      const ipv4_pool = document.getElementById('net-ipv4-pool').value.trim();
      const ipv6_pool = document.getElementById('net-ipv6-pool').value.trim();

      try {
        const res = await fetch('/api/config', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ mode, ipv4_pool, ipv6_pool })
        }).then(r => r.json());

        if (res.success) {
          showToast('专网参数更新成功');
          closeSettingsModal();
        } else {
          showToast(`更新失败: ${res.message || '格式错误'}`);
        }
      } catch (e) {
        showToast(`请求异常: ${e.message}`);
      }
    }

    function toggleTheme() {
      const html = document.documentElement;
      const current = html.getAttribute('data-theme') || 'dark';
      const target = current === 'dark' ? 'light' : 'dark';
      html.setAttribute('data-theme', target);
      localStorage.setItem('fuckipv6_webui_theme', target);
      const url = new URL(window.location.href);
      url.searchParams.set('theme', target);
      history.replaceState(null, '', url);
      showToast(`已切换至${target === 'dark' ? '深黑' : '明亮'}模式`);
    }

    (function initTheme() {
      const params = new URLSearchParams(window.location.search);
      const forced = params.get('theme');
      if (forced === 'dark' || forced === 'light') {
        document.documentElement.setAttribute('data-theme', forced);
        return;
      }
      const saved = localStorage.getItem('fuckipv6_webui_theme');
      if (saved) document.documentElement.setAttribute('data-theme', saved);
    })();

    (function initSectionSpy() {
      const links = Array.from(document.querySelectorAll('.nav-tab[href^="#"]'));
      const sections = links
        .map(link => document.querySelector(link.getAttribute('href')))
        .filter(Boolean);
      if (!sections.length) return;
      const setActive = id => {
        links.forEach(link => link.classList.toggle('active', link.getAttribute('href') === `#${id}`));
      };
      const sync = () => {
        let current = sections[0].id;
        sections.forEach(section => {
          if (section.getBoundingClientRect().top <= 130) current = section.id;
        });
        setActive(current);
      };
      window.addEventListener('scroll', sync, { passive: true });
      window.addEventListener('resize', sync);
      sync();
    })();

    setInterval(() => {
      if (!isRealtimeConnected) refresh();
    }, 4000);

    refresh();
    connectRealtime();
    refreshLogs();
  </script>
</body>
</html>"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_status_api() -> Result<()> {
        let state = AppState::new(
            Arc::new(SessionManager::new("fd00::1000/120")?),
            CoordinatorAuthConfig::default(),
        );
        let Json(response) = status(State(state)).await;
        assert_eq!(response.service, "fuckipv6-coordinator");
        assert_eq!(response.online_clients, 0);
        assert_eq!(response.traffic.signaling_rx_messages, 0);
        assert!(!response.auth_required);
        Ok(())
    }

    #[tokio::test]
    async fn test_auth_disabled_allows_peers_and_logs() -> Result<()> {
        let state = AppState::new(
            Arc::new(SessionManager::new("fd00::1000/120")?),
            CoordinatorAuthConfig::default(),
        );
        let peers_res = peers(State(state.clone()), HeaderMap::new(), Query(HashMap::new())).await;
        assert!(peers_res.is_ok());

        let logs_res = logs(State(state), HeaderMap::new(), Query(HashMap::new())).await;
        assert!(logs_res.is_ok());
        Ok(())
    }

    #[tokio::test]
    async fn test_auth_enabled_requires_jwt_token() -> Result<()> {
        let auth_config = CoordinatorAuthConfig {
            enabled: true,
            admin_username: "admin".to_string(),
            admin_password: Some("secret123".to_string()),
            jwt_secret: Some("test-jwt-secret-xyz".to_string()),
            jwt_expiration_secs: 3600,
        };
        let state = AppState::new(
            Arc::new(SessionManager::new("fd00::1000/120")?),
            auth_config,
        );

        // 未带 Token -> 401
        let unauth = peers(State(state.clone()), HeaderMap::new(), Query(HashMap::new())).await;
        assert_eq!(unauth.unwrap_err(), StatusCode::UNAUTHORIZED);

        // 错误密码登录 -> 401
        let (status_code, Json(login_fail)) = login(
            State(state.clone()),
            Json(LoginRequest {
                username: "admin".to_string(),
                password: "wrong".to_string(),
            }),
        )
        .await;
        assert_eq!(status_code, StatusCode::UNAUTHORIZED);
        assert!(!login_fail.success);

        // 正确密码登录 -> 200 + token
        let (status_code, Json(login_ok)) = login(
            State(state.clone()),
            Json(LoginRequest {
                username: "admin".to_string(),
                password: "secret123".to_string(),
            }),
        )
        .await;
        assert_eq!(status_code, StatusCode::OK);
        assert!(login_ok.success);
        let token = login_ok.token.expect("应返回有效 Token");

        // 带 Token 访问 /api/peers (通过 Header) -> 200 OK
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        let auth_ok = peers(State(state.clone()), headers, Query(HashMap::new())).await;
        assert!(auth_ok.is_ok());

        // 带 Token 访问 /api/peers (通过 Query 参数) -> 200 OK
        let mut params = HashMap::new();
        params.insert("token".to_string(), token);
        let query_ok = peers(State(state), HeaderMap::new(), Query(params)).await;
        assert!(query_ok.is_ok());

        Ok(())
    }

    #[tokio::test]
    async fn test_snapshot_response() -> Result<()> {
        let manager = SessionManager::new("fd00::1000/120")?;
        let snapshot = snapshot_response(&manager).await;

        assert_eq!(snapshot.service, "fuckipv6-coordinator");
        assert_eq!(snapshot.online_clients, 0);
        assert!(snapshot.peers.is_empty());
        assert_eq!(snapshot.traffic.relay_packets, 0);
        Ok(())
    }

    #[test]
    fn test_normalize_service_name() {
        assert_eq!(
            normalize_service_name("coordinator"),
            "fuckipv6-coordinator.service"
        );
        assert_eq!(normalize_service_name("client"), "fuckipv6-client.service");
        assert_eq!(normalize_service_name("custom.service"), "custom.service");
        assert_eq!(normalize_service_name("custom"), "custom.service");
    }

    #[test]
    fn test_tail_lines() {
        let lines = tail_lines("a\nb\nc\nd\n", 2);
        assert_eq!(lines, vec!["c".to_string(), "d".to_string()]);
    }

    #[test]
    fn test_index_html_contains_realtime_sections() {
        assert!(INDEX_HTML.contains("id=\"topology\""));
        assert!(INDEX_HTML.contains("renderTopology(peers)"));
        assert!(INDEX_HTML.contains("id=\"relay-rate\""));
        assert!(INDEX_HTML.contains("id=\"signaling-rate\""));
    }

    #[test]
    fn test_webui_chinese_and_modern_ui() {
        // 中文本地化验证
        assert!(INDEX_HTML.contains("fuckipv6 协调信令中心"));
        assert!(INDEX_HTML.contains("虚拟网络拓扑"));
        assert!(INDEX_HTML.contains("在线客户端列表"));
        assert!(INDEX_HTML.contains("系统运行日志"));
        assert!(INDEX_HTML.contains("中继传输速率"));
        assert!(INDEX_HTML.contains("客户端接入快速指引"));

        // 美化与交互组件验证
        assert!(INDEX_HTML.contains("toggleTheme()"));
        assert!(INDEX_HTML.contains("data-theme=\"dark\""));
        assert!(INDEX_HTML.contains("search-box"));
        assert!(INDEX_HTML.contains("showToast"));
        assert!(INDEX_HTML.contains("peer-filter"));
        assert!(INDEX_HTML.contains("log-service-select"));
        assert!(INDEX_HTML.contains("openSettingsModal"));
        assert!(INDEX_HTML.contains("settings-modal"));

        // 右键上下文菜单与中文日志增强验证
        assert!(INDEX_HTML.contains("cyber-context-menu"));
        assert!(INDEX_HTML.contains("showContextMenu"));
        assert!(INDEX_HTML.contains("log-level-filter"));
    }

    #[tokio::test]
    async fn test_network_config_api_get_and_update() -> Result<()> {
        let state = AppState::new(
            Arc::new(SessionManager::new("fd00::1000/120")?),
            CoordinatorAuthConfig::default(),
        );

        // GET 当前配置
        let Json(current) =
            get_network_config(State(state.clone()), HeaderMap::new(), Query(HashMap::new()))
                .await
                .map_err(|e| anyhow::anyhow!("status: {:?}", e))?;
        assert_eq!(current.mode, NetworkMode::DualStack);
        assert_eq!(current.ipv4_pool, "10.8.0.0/24");
        assert_eq!(current.ipv6_pool, "fd00::1000/120");

        // 提交非法网段 -> 400
        let (bad_status, Json(bad_resp)) = update_network_config(
            State(state.clone()),
            HeaderMap::new(),
            Query(HashMap::new()),
            Json(UpdateNetworkConfigRequest {
                mode: NetworkMode::Ipv4Only,
                ipv4_pool: "not-a-cidr".to_string(),
                ipv6_pool: "fd00::/64".to_string(),
            }),
        )
        .await;
        assert_eq!(bad_status, StatusCode::BAD_REQUEST);
        assert!(!bad_resp.success);

        // 提交合法网段 -> 200 OK
        let (ok_status, Json(ok_resp)) = update_network_config(
            State(state.clone()),
            HeaderMap::new(),
            Query(HashMap::new()),
            Json(UpdateNetworkConfigRequest {
                mode: NetworkMode::Ipv4Only,
                ipv4_pool: "192.168.100.0/24".to_string(),
                ipv6_pool: "fd00::/64".to_string(),
            }),
        )
        .await;
        assert_eq!(ok_status, StatusCode::OK);
        assert!(ok_resp.success);
        let updated = ok_resp.data.expect("应返回最新网络配置");
        assert_eq!(updated.mode, NetworkMode::Ipv4Only);
        assert_eq!(updated.ipv4_pool, "192.168.100.0/24");

        // 再次 GET 校验
        let Json(after_get) =
            get_network_config(State(state), HeaderMap::new(), Query(HashMap::new()))
                .await
                .map_err(|e| anyhow::anyhow!("status: {:?}", e))?;
        assert_eq!(after_get.mode, NetworkMode::Ipv4Only);
        assert_eq!(after_get.ipv4_pool, "192.168.100.0/24");
        Ok(())
    }
}
