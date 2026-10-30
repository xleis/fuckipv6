pub mod controller;

use anyhow::{Context, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::{header, StatusCode},
    response::{Html, IntoResponse, Json},
    routing::{get, post},
    Router,
};
pub use controller::{
    AppController, AppUiState, BanRequest, ConnectRequest, CoordinatorPeerDto,
    CoordinatorTrafficDto, HostRequest, LogsResponse, PeerUiInfo, RuntimeTrafficDto,
    UnifiedSnapshotResponse, UpdateRoutingRequest,
};
use fuckipv6_coordinator::signaling::session::PeerSnapshot;
use fuckipv6_coordinator::webui::{
    BlacklistActionRequest, BlacklistActionResponse, LoginRequest, LoginResponse,
    NetworkConfigApiResponse, NetworkConfigDto, UpdateNetworkConfigRequest,
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;

pub fn create_app_web_router(controller: Arc<AppController>) -> Router {
    Router::new()
        .route("/", get(handle_index))
        .route("/assets/webui.css", get(handle_webui_css))
        .route("/api/status", get(handle_status))
        .route("/api/peers", get(handle_peers))
        .route("/api/peers/probe", post(handle_peer_probe))
        .route(
            "/api/config",
            get(handle_get_config).post(handle_post_config),
        )
        .route("/api/blacklist", get(handle_get_blacklist))
        .route("/api/blacklist/ban", post(handle_blacklist_ban))
        .route("/api/blacklist/unban", post(handle_blacklist_unban))
        .route("/api/auth/login", post(handle_login))
        .route("/api/logs", get(handle_logs))
        .route("/ws", get(handle_ws))
        // 客户端网络与分流控制
        .route("/api/connect", post(handle_connect))
        .route("/api/disconnect", post(handle_disconnect))
        .route("/api/host", post(handle_host))
        .route("/api/stop_host", post(handle_stop_host))
        .route("/api/routing", post(handle_routing))
        // 兼容接口
        .route("/api/coordinator/ban", post(handle_coordinator_ban))
        .route("/api/coordinator/unban", post(handle_coordinator_unban))
        .with_state(controller)
}

pub async fn start_app_webui(
    port: u16,
    controller: Arc<AppController>,
) -> Result<tokio::task::JoinHandle<()>> {
    let app = create_app_web_router(controller.clone());
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("WebUI 无法监听端口: {port}"))?;

    tracing::info!("fuckipv6 统一控制台已启动: http://0.0.0.0:{port}");

    // 定期刷新在线 Peers 状态
    let ctrl = controller.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            ctrl.refresh_peers().await;
        }
    });

    let handle = tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, app).await {
            tracing::error!("WebUI 服务发生错误: {err}");
        }
    });

    Ok(handle)
}

async fn handle_index() -> impl IntoResponse {
    Html(UNIFIED_MASTER_HTML)
}

async fn handle_webui_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        WEBUI_CSS,
    )
}

#[derive(Debug, Deserialize)]
struct ProbePeerRequest {
    target: String,
    #[serde(default = "default_probe_timeout_ms")]
    timeout_ms: u64,
}

fn default_probe_timeout_ms() -> u64 {
    1_500
}

#[derive(Debug, Serialize)]
struct ApiError {
    error: String,
}

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;

fn api_error(status: StatusCode, message: impl Into<String>) -> (StatusCode, Json<ApiError>) {
    (
        status,
        Json(ApiError {
            error: message.into(),
        }),
    )
}

async fn handle_peer_probe(
    State(ctrl): State<Arc<AppController>>,
    Json(payload): Json<ProbePeerRequest>,
) -> Result<Json<crate::p2p::connection::PeerProbeOutcome>, (StatusCode, Json<ApiError>)> {
    ctrl.probe_peer(payload.target, payload.timeout_ms)
        .await
        .map(Json)
        .map_err(|error| {
            (
                StatusCode::BAD_GATEWAY,
                Json(ApiError {
                    error: format!("{error:#}"),
                }),
            )
        })
}

async fn handle_status(State(ctrl): State<Arc<AppController>>) -> Json<serde_json::Value> {
    let state = ctrl.get_state().await;
    let online_count = if state.runtime_available {
        state.peers.len()
    } else if state.coordinator_active {
        state.coordinator_online_clients
    } else {
        0
    };

    Json(serde_json::json!({
        "service": "fuckipv6-unified",
        "online_clients": online_count,
        "traffic": state.coordinator_traffic,
        "runtime_available": state.runtime_available,
        "runtime_p2p_addr": state.runtime_p2p_addr,
        "runtime_traffic": state.runtime_traffic,
        "auth_required": false,
        "status": state.status,
        "virtual_ipv6": state.virtual_ipv6,
        "virtual_ipv4": state.virtual_ipv4,
        "routing_mode": state.routing_mode,
        "exit_node": state.exit_node,
        "is_exit_node": state.is_exit_node,
        "connected_target": state.connected_target,
        "hosting_ticket": state.hosting_ticket,
        "network_name": state.network_name,
        "networks": state.networks,
        "peers": state.peers,
        "coordinator_active": state.coordinator_active,
        "coordinator_signaling_addr": state.coordinator_signaling_addr,
        "coordinator_relay_addr": state.coordinator_relay_addr,
        "coordinator_online_clients": state.coordinator_online_clients,
        "coordinator_traffic": state.coordinator_traffic,
        "coordinator_peers": state.coordinator_peers,
        "coordinator_blacklist": state.coordinator_blacklist,
        "logs": state.logs,
    }))
}

async fn handle_peers(State(ctrl): State<Arc<AppController>>) -> Json<Vec<PeerSnapshot>> {
    Json(ctrl.get_peers_snapshots().await)
}

async fn handle_get_config(State(ctrl): State<Arc<AppController>>) -> Json<NetworkConfigDto> {
    Json(ctrl.get_network_config().await)
}

async fn handle_post_config(
    State(ctrl): State<Arc<AppController>>,
    Json(req): Json<UpdateNetworkConfigRequest>,
) -> (StatusCode, Json<NetworkConfigApiResponse>) {
    match ctrl.update_network_config(req).await {
        Ok(dto) => (
            StatusCode::OK,
            Json(NetworkConfigApiResponse {
                success: true,
                message: "虚拟专网地址池参数已更新并实时生效".to_string(),
                data: Some(dto),
            }),
        ),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(NetworkConfigApiResponse {
                success: false,
                message: format!("更新网段失败: {e:#}"),
                data: None,
            }),
        ),
    }
}

async fn handle_get_blacklist(State(ctrl): State<Arc<AppController>>) -> Json<Vec<String>> {
    Json(ctrl.get_blacklist().await)
}

type BlacklistActionHttpResult =
    Result<Json<BlacklistActionResponse>, (StatusCode, Json<BlacklistActionResponse>)>;

fn blacklist_response(
    success: bool,
    message: impl Into<String>,
    blacklist: Vec<String>,
) -> Json<BlacklistActionResponse> {
    Json(BlacklistActionResponse {
        success,
        message: message.into(),
        blacklist,
    })
}

fn blacklist_contains(blacklist: &[String], client_id: &str) -> bool {
    blacklist
        .iter()
        .any(|entry| entry.trim() == client_id.trim())
}

async fn handle_blacklist_ban(
    State(ctrl): State<Arc<AppController>>,
    Json(payload): Json<BlacklistActionRequest>,
) -> BlacklistActionHttpResult {
    let client_id = payload.client_id.trim().to_string();
    if client_id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            blacklist_response(false, "客户端标识不能为空", ctrl.get_blacklist().await),
        ));
    }

    if let Err(error) = ctrl.ban_client(client_id.clone()).await {
        let message = format!("封禁失败，运行期状态未确认修改: {error:#}");
        ctrl.log(message.clone()).await;
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            blacklist_response(false, message, ctrl.get_blacklist().await),
        ));
    }

    let blacklist = ctrl.get_blacklist().await;
    // AppController currently logs persistence errors internally. Confirm the
    // requested state changed before reporting success to the WebUI/API.
    if !blacklist_contains(&blacklist, &client_id) {
        let message = "封禁失败，协调服务未确认客户端进入黑名单，运行期状态未修改".to_string();
        ctrl.log(message.clone()).await;
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            blacklist_response(false, message, blacklist),
        ));
    }

    Ok(blacklist_response(
        true,
        format!("客户端 {client_id} 已封禁加入黑名单"),
        blacklist,
    ))
}

async fn handle_blacklist_unban(
    State(ctrl): State<Arc<AppController>>,
    Json(payload): Json<BlacklistActionRequest>,
) -> BlacklistActionHttpResult {
    let client_id = payload.client_id.trim().to_string();
    if client_id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            blacklist_response(false, "客户端标识不能为空", ctrl.get_blacklist().await),
        ));
    }

    let before = ctrl.get_blacklist().await;
    if let Err(error) = ctrl.unban_client(&client_id).await {
        let message = format!("解封失败，运行期状态未确认修改: {error:#}");
        ctrl.log(message.clone()).await;
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            blacklist_response(false, message, ctrl.get_blacklist().await),
        ));
    }

    let blacklist = ctrl.get_blacklist().await;
    if !blacklist_contains(&blacklist, &client_id) {
        return Ok(blacklist_response(
            true,
            format!("客户端 {client_id} 已解除封禁移出黑名单"),
            blacklist,
        ));
    }

    if blacklist_contains(&before, &client_id) {
        let message = "解封失败，协调服务未确认客户端移出黑名单，运行期状态未修改".to_string();
        ctrl.log(message.clone()).await;
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            blacklist_response(false, message, blacklist),
        ));
    }

    Ok(blacklist_response(
        false,
        format!("客户端 {client_id} 不在黑名单中"),
        blacklist,
    ))
}

async fn handle_login(Json(_req): Json<LoginRequest>) -> Json<LoginResponse> {
    Json(LoginResponse {
        success: true,
        token: Some("authorized".to_string()),
        error: None,
        expires_in: Some(86400),
        auth_required: false,
    })
}

#[derive(Debug, Deserialize)]
pub struct LogsQuery {
    pub service: Option<String>,
    pub lines: Option<usize>,
}

async fn handle_logs(
    State(ctrl): State<Arc<AppController>>,
    Query(params): Query<LogsQuery>,
) -> Json<LogsResponse> {
    let service = params.service.unwrap_or_else(|| "coordinator".to_string());
    let lines = params.lines.unwrap_or(100).clamp(10, 500);
    Json(ctrl.read_logs(&service, lines).await)
}

async fn handle_ws(
    ws: WebSocketUpgrade,
    State(ctrl): State<Arc<AppController>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| ws_stream(socket, ctrl))
}

async fn ws_stream(mut socket: WebSocket, ctrl: Arc<AppController>) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(2));
    loop {
        ticker.tick().await;
        let snap = ctrl.get_unified_snapshot().await;
        let Ok(payload) = serde_json::to_string(&snap) else {
            continue;
        };
        if socket.send(Message::Text(payload.into())).await.is_err() {
            break;
        }
    }
}

async fn handle_connect(
    State(ctrl): State<Arc<AppController>>,
    Json(payload): Json<ConnectRequest>,
) -> ApiResult<AppUiState> {
    match ctrl
        .connect(payload.target, payload.routing_mode, payload.exit_node)
        .await
    {
        Ok(_) => Ok(Json(ctrl.get_state().await)),
        Err(e) => {
            let message = format!("连接网络失败: {e:#}");
            ctrl.log(message.clone()).await;
            Err(api_error(StatusCode::INTERNAL_SERVER_ERROR, message))
        }
    }
}

async fn handle_disconnect(State(ctrl): State<Arc<AppController>>) -> ApiResult<AppUiState> {
    match ctrl.disconnect().await {
        Ok(_) => Ok(Json(ctrl.get_state().await)),
        Err(e) => {
            let message = format!("断开网络失败: {e:#}");
            ctrl.log(message.clone()).await;
            Err(api_error(StatusCode::INTERNAL_SERVER_ERROR, message))
        }
    }
}

async fn handle_host(
    State(ctrl): State<Arc<AppController>>,
    Json(payload): Json<HostRequest>,
) -> ApiResult<AppUiState> {
    match ctrl
        .host_network(
            payload.name,
            payload.port,
            payload.relay_port,
            payload.auto_connect,
            payload.external_host,
        )
        .await
    {
        Ok(_) => Ok(Json(ctrl.get_state().await)),
        Err(e) => {
            let message = format!("自建网络失败: {e:#}");
            ctrl.log(message.clone()).await;
            Err(api_error(StatusCode::INTERNAL_SERVER_ERROR, message))
        }
    }
}

async fn handle_stop_host(State(ctrl): State<Arc<AppController>>) -> ApiResult<AppUiState> {
    match ctrl.stop_host().await {
        Ok(_) => Ok(Json(ctrl.get_state().await)),
        Err(e) => {
            let message = format!("停止自建网络失败: {e:#}");
            ctrl.log(message.clone()).await;
            Err(api_error(StatusCode::INTERNAL_SERVER_ERROR, message))
        }
    }
}

async fn handle_coordinator_ban(
    State(ctrl): State<Arc<AppController>>,
    Json(payload): Json<BanRequest>,
) -> ApiResult<AppUiState> {
    let client_id = payload.client_id.trim().to_string();
    if let Err(error) = ctrl.ban_client(client_id.clone()).await {
        let message = format!("封禁客户端失败: {error:#}");
        ctrl.log(message.clone()).await;
        return Err(api_error(StatusCode::BAD_REQUEST, message));
    }

    if !blacklist_contains(&ctrl.get_blacklist().await, &client_id) {
        let message = "封禁客户端失败，协调服务未确认运行期状态修改".to_string();
        ctrl.log(message.clone()).await;
        return Err(api_error(StatusCode::INTERNAL_SERVER_ERROR, message));
    }

    Ok(Json(ctrl.get_state().await))
}

async fn handle_coordinator_unban(
    State(ctrl): State<Arc<AppController>>,
    Json(payload): Json<BanRequest>,
) -> ApiResult<AppUiState> {
    let client_id = payload.client_id.trim().to_string();
    let before = ctrl.get_blacklist().await;
    if let Err(error) = ctrl.unban_client(&client_id).await {
        let message = format!("解封客户端失败: {error:#}");
        ctrl.log(message.clone()).await;
        return Err(api_error(StatusCode::BAD_REQUEST, message));
    }

    let after = ctrl.get_blacklist().await;
    if blacklist_contains(&before, &client_id) && blacklist_contains(&after, &client_id) {
        let message = "解封客户端失败，协调服务未确认运行期状态修改".to_string();
        ctrl.log(message.clone()).await;
        return Err(api_error(StatusCode::INTERNAL_SERVER_ERROR, message));
    }

    Ok(Json(ctrl.get_state().await))
}

async fn handle_routing(
    State(ctrl): State<Arc<AppController>>,
    Json(payload): Json<UpdateRoutingRequest>,
) -> ApiResult<AppUiState> {
    match ctrl
        .update_routing(payload.mode, payload.exit_node, payload.is_exit_node)
        .await
    {
        Ok(_) => Ok(Json(ctrl.get_state().await)),
        Err(e) => {
            let message = format!("路由分流策略更新失败: {e:#}");
            ctrl.log(message.clone()).await;
            Err(api_error(StatusCode::INTERNAL_SERVER_ERROR, message))
        }
    }
}

const WEBUI_CSS: &str = include_str!("../../../common/assets/webui.css");

const UNIFIED_MASTER_HTML: &str = r##"<!doctype html>
<html lang="zh-CN" data-theme="dark" translate="no">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta name="google" content="notranslate">
  <meta name="color-scheme" content="dark light">
  <title>fuckipv6 控制中心</title>
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
          <span class="sub">分布式拓扑中枢</span>
        </div>
      </div>

      <div>
        <div class="nav-section-label">控制台</div>
        <nav class="nav-tabs">
          <button class="nav-tab active" data-tab="tab-overview" onclick="switchTab('tab-overview')">
            <span class="nav-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="2.2"/><circle cx="4.5" cy="6" r="2.2"/><circle cx="19.5" cy="6" r="2.2"/><circle cx="4.5" cy="18" r="2.2"/><circle cx="19.5" cy="18" r="2.2"/><path d="m6.4 7.2 4 3.4M17.6 7.2l-4 3.4M6.4 16.8l4-3.4M17.6 16.8l-4-3.4"/></svg></span>
            <span class="nav-text">全景拓扑大盘</span>
          </button>
          <button class="nav-tab" data-tab="tab-peers" onclick="switchTab('tab-peers')">
            <span class="nav-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="3.5" width="19" height="13" rx="2.2"/><path d="M8 20.5h8M12 16.5v4"/></svg></span>
            <span class="nav-text">对等节点与终端</span>
            <span class="tab-badge" id="tab-peers-badge">0</span>
          </button>
          <button class="nav-tab" data-tab="tab-routing" onclick="switchTab('tab-routing')">
            <span class="nav-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="6" cy="6" r="2.5"/><circle cx="6" cy="18" r="2.5"/><circle cx="18" cy="8" r="2.5"/><path d="M6 8.5v7M18 10.5c0 3.2-2.2 4.5-5.2 4.5H8.5"/></svg></span>
            <span class="nav-text">原生智能分流</span>
          </button>
          <button class="nav-tab" data-tab="tab-settings" onclick="switchTab('tab-settings')">
            <span class="nav-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M4 6h9M17 6h3M4 12h3M11 12h9M4 18h9M17 18h3"/><circle cx="15" cy="6" r="2.1"/><circle cx="9" cy="12" r="2.1"/><circle cx="15" cy="18" r="2.1"/></svg></span>
            <span class="nav-text">专网中枢设置</span>
          </button>
          <button class="nav-tab" data-tab="tab-logs" onclick="switchTab('tab-logs')">
            <span class="nav-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="3.5" width="19" height="17" rx="2.4"/><path d="m7 9 3 3-3 3M12.5 15H17"/></svg></span>
            <span class="nav-text">系统运行日志</span>
          </button>
        </nav>
      </div>

      <div class="sidebar-footer">
        <div class="sidebar-card">
          <div class="sidebar-card-top">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/><path d="m9 12 2 2 4-4"/></svg>
            <span>端到端加密</span>
          </div>
          <p>QUIC · TLS 1.3 隧道<br>信令与中继链路全程加密</p>
        </div>
        <div class="sidebar-version"><span>控制台</span><span>v0.3.0</span></div>
      </div>
    </aside>

    <!-- ===================================================================== -->
    <!-- 主工作区                                                              -->
    <!-- ===================================================================== -->
    <div class="main">
      <header class="topbar">
        <div class="topbar-inner">
          <div class="page-heading">
            <h2 id="page-title">全景拓扑大盘</h2>
            <p id="page-subtitle">实时掌握专网节点、链路质量与流量态势</p>
          </div>

          <div class="header-actions">
            <div class="status-chip">
              <span class="live-pip" id="live-dot"></span>
              <span id="status">连接就绪</span>
            </div>
            <span class="meta-chip">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3.2 1.9"/></svg>
              <span id="uptime-display">0s</span>
            </span>
            <button class="btn btn-primary" onclick="copyNetworkTicket()" id="btn-copy-ticket" title="复制 fk:// 网络邀请凭证">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M21 2l-2 2m-1.5 1.5L14 9l-1.5-1.5L11 9l-1.5-1.5L8 9 3 14v7h7l5-5 1.5 1.5L18 16l1.5-1.5L18 13l3.5-3.5a2.12 2.12 0 0 0 0-3L21 2z"/></svg>
              <span>复制邀请凭证</span>
            </button>
            <button class="btn" onclick="openGuideModal()" title="客户端接入指引">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z"/><path d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z"/></svg>
              <span>接入指引</span>
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
        <!-- ================================================================= -->
        <!-- TAB 1: 全景拓扑大盘                                                -->
        <!-- ================================================================= -->
        <div id="tab-overview" class="tab-content">
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
                <span class="m-label">链路传输速率</span>
              </div>
              <div class="m-num" id="relay-rate">0 B/s</div>
              <div class="m-hint">实时 P2P / 中继载荷</div>
            </article>
            <article class="m-item" data-tone="blue">
              <div class="m-head">
                <span class="m-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><ellipse cx="12" cy="5.5" rx="8.5" ry="3"/><path d="M3.5 5.5v6c0 1.7 3.8 3 8.5 3s8.5-1.3 8.5-3v-6"/><path d="M3.5 11.5v6c0 1.7 3.8 3 8.5 3s8.5-1.3 8.5-3v-6"/></svg></span>
                <span class="m-label">链路累计流量</span>
              </div>
              <div class="m-num" id="relay-total">0 B</div>
              <div class="m-hint" id="relay-packets-desc">0 包已转发</div>
            </article>
            <article class="m-item" data-tone="emerald">
              <div class="m-head">
                <span class="m-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="2"/><path d="M16.24 7.76a6 6 0 0 1 0 8.49M7.76 16.24a6 6 0 0 1 0-8.49M19.07 4.93a10 10 0 0 1 0 14.14M4.93 19.07a10 10 0 0 1 0-14.14"/></svg></span>
                <span class="m-label">控制信令速率</span>
              </div>
              <div class="m-num" id="signaling-rate">0 B/s</div>
              <div class="m-hint">实时消息吞吐</div>
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
                <span class="m-label">链路健康状态</span>
              </div>
              <div class="m-num" id="health-status">0</div>
              <div class="m-hint" id="relay-errors-desc">链路健康良好</div>
            </article>
          </div>

          <section class="panel">
            <div class="panel-head">
              <div class="panel-heading">
                <span class="panel-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3c2.5 2.6 3.8 5.7 3.8 9S14.5 18.4 12 21c-2.5-2.6-3.8-5.7-3.8-9S9.5 5.6 12 3z"/></svg></span>
                <div>
                  <h3>虚拟网络拓扑</h3>
                  <p>P2P 链路、NAT 穿透与专网节点的实时视图</p>
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
        </div>

        <!-- ================================================================= -->
        <!-- TAB 2: 对等节点与终端                                              -->
        <!-- ================================================================= -->
        <div id="tab-peers" class="tab-content" style="display:none;">
          <div class="machine-status-bar">
            <div class="machine-info">
              <div class="machine-prop">
                <span class="machine-prop-label">本机虚拟 IPv6</span>
                <span class="machine-prop-val copyable" id="bar-v6-val" onclick="copyText(this.textContent, '虚拟 IPv6')">未分配</span>
              </div>
              <div class="machine-prop">
                <span class="machine-prop-label">本机虚拟 IPv4</span>
                <span class="machine-prop-val copyable" id="bar-v4-val" onclick="copyText(this.textContent, '虚拟 IPv4')">未分配</span>
              </div>
              <div class="machine-prop">
                <span class="machine-prop-label">运行状态</span>
                <span class="machine-prop-val" id="bar-status-val">离线</span>
              </div>
              <div class="machine-prop">
                <span class="machine-prop-label">当前分流策略</span>
                <span class="machine-prop-val" id="bar-routing-val" style="color:var(--accent);">MeshOnly</span>
              </div>
            </div>
            <div class="inline-actions">
              <button class="btn btn-primary" id="btn-toggle-connection" onclick="toggleLocalConnection()">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M18.36 6.64a9 9 0 1 1-12.73 0"/><path d="M12 2v10"/></svg>
                <span id="txt-toggle-connection">连接网络</span>
              </button>
            </div>
          </div>

          <section class="panel">
            <div class="panel-head">
              <div class="panel-heading">
                <span class="panel-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="3.5" width="19" height="13" rx="2.2"/><path d="M8 20.5h8M12 16.5v4"/><path d="M6.5 8h5M6.5 11h8"/></svg></span>
                <div>
                  <h3>对等节点与终端</h3>
                  <p>已加入本专网的全部设备与链路状态</p>
                </div>
              </div>
              <div class="panel-tools">
                <span class="section-tag">P2P 直连 / 中继</span>
              </div>
            </div>
            <div class="controls-row">
              <div class="search-box">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="7.5"/><path d="m21 21-4.3-4.3"/></svg>
                <input type="text" id="peer-filter" placeholder="检索 IP / 标识 / 端点..." oninput="filterPeersTable()">
              </div>
              <select class="select-pill" id="nat-filter" onchange="filterPeersTable()">
                <option value="">全部 NAT 穿透类型</option>
                <option value="Cone">全锥型 NAT</option>
                <option value="Symmetric">对称型 NAT</option>
                <option value="Restricted">受限型 NAT</option>
              </select>
            </div>
            <div class="table-responsive">
              <div id="content">
                <p style="color:var(--text-muted);padding:24px 20px;">等待对等节点接入专网...</p>
              </div>
            </div>
          </section>
        </div>

        <!-- ================================================================= -->
        <!-- TAB 3: 原生智能分流                                                -->
        <!-- ================================================================= -->
        <div id="tab-routing" class="tab-content" style="display:none;">
          <div class="section-title-wrap">
            <h2>
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="6" cy="6" r="2.5"/><circle cx="6" cy="18" r="2.5"/><circle cx="18" cy="8" r="2.5"/><path d="M6 8.5v7M18 10.5c0 3.2-2.2 4.5-5.2 4.5H8.5"/></svg>
              <span>原生三档分流策略</span>
            </h2>
            <span class="section-tag" id="current-routing-mode-tag">仅对等专网</span>
          </div>

          <div class="routing-modes-grid">
            <div class="mode-card active" id="mode-card-mesh" onclick="selectRoutingMode('MeshOnly')">
              <div class="mode-badge green">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
                <span>仅对等专网 · 内网互通</span>
              </div>
              <div class="mode-title">纯内网加密组网</div>
              <div class="mode-desc">仅在虚拟专网内部各对等节点之间直接建立加密通道互联，常规外网访问使用本地物理网络直连，不走代理。</div>
            </div>
            <div class="mode-card" id="mode-card-bypass" onclick="selectRoutingMode('BypassMainland')">
              <div class="mode-badge purple">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
                <span>智能分流 · 绕过大陆</span>
              </div>
              <div class="mode-title">大陆直连 · 海外加速</div>
              <div class="mode-desc">自动检测中国大陆 IP 网段与大陆域名走本地直连，海外流量与受限站点自动路由至指定流量出口网关加速。</div>
            </div>
            <div class="mode-card" id="mode-card-global" onclick="selectRoutingMode('GlobalProxy')">
              <div class="mode-badge blue">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
                <span>全局代理 · 流量中转</span>
              </div>
              <div class="mode-title">全局流量代理出境</div>
              <div class="mode-desc">本机全部公网网络流量 100% 路由至虚拟对等网络中的出口节点代理中转，实现全局匿名与跨境访问。</div>
            </div>
          </div>

          <section class="card-block">
            <div class="card-header">
              <span class="card-title">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/><path d="m9 12 2 2 4-4"/></svg>
                <span>流量出口网关节点控制</span>
              </span>
            </div>
            <div class="card-body">
              <div class="form-row">
                <div class="form-group">
                  <label>指定流量出口网关节点</label>
                  <select class="form-control" id="exit-node-select">
                    <option value="">(自动寻找网络中可用出口网关)</option>
                  </select>
                </div>
                <label class="checkbox-row" for="is-exit-node-check">
                  <input type="checkbox" id="is-exit-node-check">
                  <span>允许本机作为出口节点网关，为网络内其他节点代理上网</span>
                </label>
                <button class="btn btn-primary" onclick="applyRoutingConfig()">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
                  <span>保存并应用</span>
                </button>
              </div>
            </div>
          </section>
        </div>

        <!-- ================================================================= -->
        <!-- TAB 4: 专网中枢设置                                                -->
        <!-- ================================================================= -->
        <div id="tab-settings" class="tab-content" style="display:none;">
          <div class="settings-grid">
            <section class="card-block span-2">
              <div class="card-header">
                <span class="card-title">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/></svg>
                  <span>对等网络接入目标</span>
                </span>
                <button class="btn" onclick="openHostModal()">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M12 2v4M12 18v4M4.9 4.9l2.8 2.8M16.3 16.3l2.8 2.8M2 12h4M18 12h4M4.9 19.1l2.8-2.8M16.3 7.7l2.8-2.8"/></svg>
                  <span>自建新网络</span>
                </button>
              </div>
              <div class="card-body">
                <div class="form-row">
                  <div class="form-group">
                    <label>协调器端点或 fk:// 邀请凭证</label>
                    <input type="text" class="form-control" id="connect-target-input" placeholder="输入 fk:// 邀请凭证 或 协调器端点 (例如 185.217.110.15:6666)">
                  </div>
                  <button class="btn btn-primary" onclick="clientConnect()">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
                    <span>加入网络</span>
                  </button>
                  <button class="btn btn-danger" onclick="clientDisconnect()">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="3.5" y="3.5" width="17" height="17" rx="2.2"/></svg>
                    <span>断开</span>
                  </button>
                </div>
              </div>
            </section>

            <section class="card-block span-2">
              <div class="card-header">
                <span class="card-title">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M21 2l-2 2m-1.5 1.5L14 9l-1.5-1.5L11 9l-1.5-1.5L8 9 3 14v7h7l5-5 1.5 1.5L18 16l1.5-1.5L18 13l3.5-3.5a2.12 2.12 0 0 0 0-3L21 2z"/></svg>
                  <span>网络邀请凭证与终端接入</span>
                </span>
                <button class="btn btn-primary" id="btn-copy-network-ticket" onclick="copyNetworkTicket()" style="display:none;">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2.2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>
                  <span>复制凭证</span>
                </button>
              </div>
              <div class="card-body">
                <p class="hint-text" style="margin-bottom:10px;">其他设备可在客户端输入以下 fk:// 凭证自动协商接入，或在终端执行加入命令。</p>
                <div class="code-view" id="display-network-ticket">fk://...</div>
                <div class="row" style="justify-content:space-between;margin-top:10px;">
                  <span class="hint-text">终端直连加入命令</span>
                  <button class="btn" onclick="copyCliCommand()">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2.2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>
                    <span>复制 CLI 加入指令</span>
                  </button>
                </div>
              </div>
            </section>

            <section class="card-block">
              <div class="card-header">
                <span class="card-title">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="2.5" width="19" height="7" rx="2"/><rect x="2.5" y="14.5" width="19" height="7" rx="2"/><path d="M6 6h.01M6 18h.01"/></svg>
                  <span>专网 IP 地址池</span>
                </span>
                <button class="btn btn-primary" onclick="saveNetworkSettings()">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
                  <span>保存并应用</span>
                </button>
              </div>
              <div class="card-body">
                <div class="form-grid cols-2">
                  <div class="form-group">
                    <label>运行模式</label>
                    <select class="form-control" id="net-mode-select">
                      <option value="dual_stack">IPv4 + IPv6 双栈模式</option>
                      <option value="ipv6_only">仅分配 IPv6 单栈</option>
                      <option value="ipv4_only">仅分配 IPv4 单栈</option>
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
                </div>
              </div>
            </section>

            <section class="card-block">
              <div class="card-header">
                <span class="card-title">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3c2.5 2.6 3.8 5.7 3.8 9S14.5 18.4 12 21c-2.5-2.6-3.8-5.7-3.8-9S9.5 5.6 12 3z"/></svg>
                  <span>协调中枢服务端口配置</span>
                </span>
                <button class="btn btn-danger" id="btn-stop-host" onclick="stopHostNetwork()" style="display:none;">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="5" y="5" width="14" height="14" rx="2"/></svg>
                  <span>停止自建网络</span>
                </button>
                <button class="btn" onclick="openHostModal()">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M12 2v4M12 18v4M4.9 4.9l2.8 2.8M16.3 16.3l2.8 2.8M2 12h4M18 12h4M4.9 19.1l2.8-2.8M16.3 7.7l2.8-2.8"/></svg>
                  <span>重置 / 新建</span>
                </button>
              </div>
              <div class="card-body">
                <div class="form-grid cols-2">
                  <div class="form-group">
                    <label>信令监听端口</label>
                    <input type="text" class="form-control" id="hub-signal-port" value="--" readonly>
                  </div>
                  <div class="form-group">
                    <label>QUIC 中继端口</label>
                    <input type="text" class="form-control" id="hub-relay-port" value="--" readonly>
                  </div>
                  <div class="form-group">
                    <label>外部公网端点</label>
                    <input type="text" class="form-control" id="hub-external-host" value="由邀请票据决定" readonly>
                  </div>
                  <div class="form-group">
                    <label>网络标识名</label>
                    <input type="text" class="form-control" id="hub-network-name" value="--" readonly>
                  </div>
                </div>
              </div>
            </section>

            <section class="card-block span-2">
              <div class="card-header">
                <span class="card-title">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="m5.5 5.5 13 13"/></svg>
                  <span>黑名单访问控制管理</span>
                </span>
                <span class="section-tag">访问控制名单</span>
              </div>
              <div class="card-body">
                <div class="form-row" style="margin-bottom:14px;">
                  <div class="form-group">
                    <label>手动添加封禁客户端标识 (Client ID)</label>
                    <input type="text" class="form-control" id="manual-ban-id" placeholder="输入要强制封禁踢出的客户端标识 (Client ID)...">
                  </div>
                  <button class="btn btn-danger" onclick="manualBanClient()">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="m5.5 5.5 13 13"/></svg>
                    <span>强制拉黑</span>
                  </button>
                </div>
                <div class="table-responsive">
                  <div id="blacklist-content">
                    <p style="color:var(--text-muted);padding:14px 0;">暂无被封禁的客户端记录。</p>
                  </div>
                </div>
              </div>
            </section>

            <section class="card-block span-2">
              <div class="card-header">
                <span class="card-title">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="4" y="4" width="16" height="16" rx="2.2"/><rect x="9" y="9" width="6" height="6"/><path d="M9 1.5v3M15 1.5v3M9 19.5v3M15 19.5v3M19.5 9h3M19.5 15h3M1.5 9h3M1.5 15h3"/></svg>
                  <span>本地虚拟网卡与引擎状态 · TUN 适配器</span>
                </span>
                <span class="section-tag" id="runtime-card-state" style="color:var(--text-muted);">未连接</span>
              </div>
              <div class="card-body">
                <div class="form-grid cols-4">
                  <div class="machine-prop">
                    <span class="machine-prop-label">TUN 设备名</span>
                      <span class="machine-prop-val" id="runtime-tun-name">--</span>
                  </div>
                  <div class="machine-prop">
                      <span class="machine-prop-label">P2P 监听地址</span>
                      <span class="machine-prop-val" id="runtime-p2p-addr">--</span>
                  </div>
                  <div class="machine-prop">
                      <span class="machine-prop-label">运行时 uptime</span>
                      <span class="machine-prop-val" id="runtime-uptime">--</span>
                  </div>
                  <div class="machine-prop">
                      <span class="machine-prop-label">数据来源</span>
                      <span class="machine-prop-val" id="runtime-source">无活动运行时</span>
                  </div>
                </div>
              </div>
            </section>
          </div>
        </div>

        <!-- ================================================================= -->
        <!-- TAB 5: 系统运行日志                                                -->
        <!-- ================================================================= -->
        <div id="tab-logs" class="tab-content" style="display:none;">
          <section class="panel">
            <div class="panel-head">
              <div class="panel-heading">
                <span class="panel-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="3.5" width="19" height="17" rx="2.4"/><path d="m7 9 3 3-3 3M12.5 15H17"/></svg></span>
                <div>
                  <h3>全局运行日志控制台</h3>
                  <p>实时追踪信令、中继与客户端引擎的运行事件</p>
                </div>
              </div>
              <div class="panel-tools">
                <span class="section-tag" id="log-service-badge">fuckipv6</span>
              </div>
            </div>
            <div class="console-stage" oncontextmenu="handleLogsContextMenu(event)" title="右键打开日志快捷菜单">
              <div class="console-toolbar">
                <div>
                  <select class="select-pill" id="log-service-select" onchange="refreshLogs()">
                    <option value="coordinator">协调中枢服务</option>
                    <option value="client">客户端服务</option>
                    <option value="memory">异常错误日志</option>
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
                    <span>美化 / 原始</span>
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
        </div>

        <footer class="app-footer">
          <div>FUCKIPV6 虚拟专网 · 分布式 QUIC 网络</div>
          <div>控制平面 · 实时遥测态势</div>
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
        <button class="btn btn-primary" onclick="copyGuideConfig()">复制配置模版</button>
      </div>
    </div>
  </div>

  <!-- 自建网络 Modal -->
  <div class="modal-wrap" id="host-modal" onclick="if(event.target===this)closeHostModal()">
    <div class="modal-box">
      <div class="modal-head">
        <h3>自建对等专网中枢</h3>
        <button class="btn btn-icon" onclick="closeHostModal()" title="关闭">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18M6 6l12 12"/></svg>
        </button>
      </div>
      <div class="form-group">
        <label>网络名称</label>
        <input type="text" class="form-control" id="host-net-name" value="default">
      </div>
      <div class="form-group">
        <label>信令服务端口</label>
        <input type="number" class="form-control" id="host-net-port" value="6666">
      </div>
      <div class="form-group">
        <label>QUIC 中继转发端口</label>
        <input type="number" class="form-control" id="host-net-relay-port" value="6667">
      </div>
      <div class="form-group">
        <label>公网外部主机名或 IP (用于写入邀请票据)</label>
        <input type="text" class="form-control" id="host-net-external" placeholder="例如: relay.xlei.cc 或 185.217.110.15">
      </div>
      <div class="modal-actions">
        <button class="btn" onclick="closeHostModal()">取消</button>
        <button class="btn btn-primary" onclick="submitHostNetwork()">立即创建并发布专网</button>
      </div>
    </div>
  </div>

  <div class="toast-box" id="toast-box"></div>

  <script>
    /* 100% SVG 矢量图标库，彻底杜绝任何 Emoji */
    const SVG_ICONS = {
      copy: '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>',
      ipv4: '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="2" y="2" width="20" height="8" rx="2" ry="2"/><rect x="2" y="14" width="20" height="8" rx="2" ry="2"/><line x1="6" y1="6" x2="6.01" y2="6"/><line x1="6" y1="18" x2="6.01" y2="18"/></svg>',
      ipv6: '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M4.5 16.5c-1.5 1.26-2 5-2 5s3.74-.5 5-2c.71-.84.7-2.13-.09-2.91a2.18 2.18 0 0 0-2.91-.09z"/><path d="m12 15-3-3a22 22 0 0 1 2-3.95A12.88 12.88 0 0 1 22 2c0 2.72-.78 7.5-6 11a22.35 22.35 0 0 1-4 2z"/></svg>',
      location: '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M21 10c0 7-9 13-9 13s-9-6-9-13a9 9 0 0 1 18 0z"/><circle cx="12" cy="10" r="3"/></svg>',
      target: '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"/><circle cx="12" cy="12" r="6"/><circle cx="12" cy="12" r="2"/></svg>',
      zap: '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2"/></svg>',
      shield: '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/></svg>',
      ban: '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"/><line x1="4.93" y1="4.93" x2="19.07" y2="19.07"/></svg>',
      refresh: '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="23 4 23 10 17 10"/><polyline points="1 20 1 14 7 14"/><path d="M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15"/></svg>',
      trash: '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>',
      down: '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><line x1="12" y1="5" x2="12" y2="19"/><polyline points="19 12 12 19 5 12"/></svg>',
      up: '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><line x1="12" y1="19" x2="12" y2="5"/><polyline points="5 12 12 5 19 12"/></svg>',
      format: '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="4 7 4 4 20 4 20 7"/><line x1="9" y1="20" x2="15" y2="20"/><line x1="12" y1="4" x2="12" y2="20"/></svg>',
      radar: '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M4.93 4.93a10 10 0 0 1 14.14 0"/><path d="M7.76 7.76a6 6 0 0 1 8.48 0"/><circle cx="12" cy="12" r="2"/></svg>',
      sparkles: '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 2v4M12 18v4M4.93 4.93l2.83 2.83M16.24 16.24l2.83 2.83M2 12h4M18 12h4M4.93 19.07l2.83-2.83M16.24 7.76l2.83-2.83"/></svg>',
      grid: '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"/><line x1="12" y1="2" x2="12" y2="22"/><line x1="2" y1="12" x2="22" y2="12"/></svg>',
    };

    let previousTraffic = null;
    let previousRuntimeTraffic = null;
    let previousTrafficAt = 0;
    let cachedPeers = [];
    let rawLogLines = [];
    let isRealtimeConnected = false;
    let currentSelectedRoutingMode = 'MeshOnly';
    let currentTicket = '';
    let currentClientStatus = 'offline';
    let lastStatusData = null;

    function showToast(message) {
      const box = document.getElementById('toast-box');
      const toast = document.createElement('div');
      toast.className = 'toast';
      const marker = document.createElement('span');
      marker.style.color = 'var(--accent)';
      marker.style.fontSize = '10px';
      marker.textContent = '•';
      const text = document.createElement('span');
      text.textContent = String(message);
      toast.append(marker, text);
      box.appendChild(toast);
      setTimeout(() => {
        toast.style.opacity = '0';
        toast.style.transform = 'translateY(6px)';
        toast.style.transition = 'all 0.2s';
        setTimeout(() => toast.remove(), 200);
      }, 2400);
    }

    async function requestJson(url, options = {}) {
      const response = await fetch(url, options);
      const contentType = response.headers.get('content-type') || '';
      let payload = null;
      if (contentType.includes('application/json')) {
        payload = await response.json().catch(() => null);
      } else {
        const text = await response.text().catch(() => '');
        payload = text ? { error: text } : null;
      }
      if (!response.ok) {
        const detail = payload && (payload.error || payload.message);
        throw new Error(detail || `HTTP ${response.status} ${response.statusText}`.trim());
      }
      return payload;
    }

    const TAB_META = {
      'tab-overview': ['全景拓扑大盘', '实时掌握专网节点、链路质量与流量态势'],
      'tab-peers': ['对等节点与终端', '管理已加入专网的设备、端点与 NAT 穿透状态'],
      'tab-routing': ['原生智能分流', '选择流量出口策略并配置出境网关'],
      'tab-settings': ['专网中枢设置', '网络地址池、协调端点与安全策略'],
      'tab-logs': ['系统运行日志', '实时追踪控制面与数据面运行事件'],
    };

    function switchTab(tabId) {
      document.querySelectorAll('.tab-content').forEach(el => el.style.display = 'none');
      document.querySelectorAll('.nav-tab').forEach(el => el.classList.remove('active'));
      const target = document.getElementById(tabId);
      if (target) target.style.display = 'block';
      const btn = document.querySelector(`.nav-tab[data-tab="${tabId}"]`);
      if (btn) btn.classList.add('active');

      const meta = TAB_META[tabId];
      if (meta) {
        const title = document.getElementById('page-title');
        const subtitle = document.getElementById('page-subtitle');
        if (title) title.textContent = meta[0];
        if (subtitle) subtitle.textContent = meta[1];
      }
      const url = new URL(window.location.href);
      if (url.searchParams.get('tab') !== tabId) {
        url.searchParams.set('tab', tabId);
        history.replaceState(null, '', url);
      }

      if (tabId === 'tab-overview') {
        setTimeout(resizeCanvas, 50);
      } else if (tabId === 'tab-settings') {
        loadConfigAndBlacklist();
      } else if (tabId === 'tab-logs') {
        refreshLogs();
      }
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
      if (m < 60) return `${m}分 ${s % 60}秒`;
      const h = Math.floor(m / 60);
      if (h < 24) return `${h}小时 ${m % 60}分`;
      const d = Math.floor(h / 24);
      return `${d}天 ${h % 24}小时`;
    }

    function formatNatType(natType) {
      if (!natType) return '未知 NAT';
      const t = String(natType).trim().toLowerCase();
      if (t.includes('full') || t === 'cone' || t.includes('full cone')) return '全锥型 NAT';
      if (t.includes('port restricted') || t.includes('portrestricted')) return '端口受限锥型 NAT';
      if (t.includes('restricted')) return '受限锥型 NAT';
      if (t.includes('symmetric') || t === 'sym') return '对称型 NAT';
      if (t.includes('open') || t.includes('public')) return '公网型 (无 NAT)';
      if (t.includes('direct')) return 'P2P 直连';
      if (t.includes('relay')) return 'QUIC 中继';
      return natType;
    }

    function shortId(value) {
      if (!value) return '';
      return value.length > 14 ? `${value.slice(0, 7)}...${value.slice(-5)}` : value;
    }

    function copyText(text, label = '内容') {
      if (!text || text === '未分配' || text === '-') {
        showToast('暂无可复制内容');
        return;
      }
      navigator.clipboard.writeText(text).then(() => {
        showToast(`已复制 ${label}`);
      }).catch(() => {
        showToast(`已复制 ${label}`);
      });
    }

    function copyNetworkTicket() {
      if (!currentTicket) {
        showToast('当前未运行网络或暂无票据');
        return;
      }
      copyText(currentTicket, '网络邀请凭证 (fk://)');
    }

    function copyCliCommand() {
      if (!currentTicket) {
        showToast('当前暂无可用的网络凭证');
        return;
      }
      const cmd = `fuckipv6 --connect "${currentTicket}"`;
      copyText(cmd, '终端加入命令');
    }

    function getNatBadge(natType) {
      const t = String(natType || '').toLowerCase();
      if (t.includes('full') || t === 'cone') return `<span class="tag-nat cone">全锥型 NAT</span>`;
      if (t.includes('symmetric') || t === 'sym') return `<span class="tag-nat sym">对称型 NAT</span>`;
      if (t.includes('restricted')) return `<span class="tag-nat">受限型 NAT</span>`;
      return `<span class="tag-nat">${formatNatType(natType)}</span>`;
    }

    function renderTraffic(traffic, runtimeTraffic) {
      const now = Date.now();
      const elapsed = previousTrafficAt ? (now - previousTrafficAt) / 1000 : 0;

      if (runtimeTraffic) {
        const p2pTotal = (runtimeTraffic.p2p_rx_bytes || 0) + (runtimeTraffic.p2p_tx_bytes || 0);
        const previousP2PTotal = previousRuntimeTraffic
          ? (previousRuntimeTraffic.p2p_rx_bytes || 0) + (previousRuntimeTraffic.p2p_tx_bytes || 0)
          : p2pTotal;
        const p2pDelta = Math.max(0, p2pTotal - previousP2PTotal);
        const p2pPackets = (runtimeTraffic.p2p_rx_packets || 0) + (runtimeTraffic.p2p_tx_packets || 0);
        const msgTotal = (runtimeTraffic.signaling_messages_received || 0) + (runtimeTraffic.signaling_messages_sent || 0);
        const previousMsgTotal = previousRuntimeTraffic
          ? (previousRuntimeTraffic.signaling_messages_received || 0) + (previousRuntimeTraffic.signaling_messages_sent || 0)
          : msgTotal;
        const msgDelta = Math.max(0, msgTotal - previousMsgTotal);
        const msgRate = elapsed > 0 ? msgDelta / elapsed : 0;
        const tunErrors = (runtimeTraffic.tun_read_errors || 0) + (runtimeTraffic.tun_write_errors || 0);
        const p2pErrors = (runtimeTraffic.p2p_send_errors || 0) + (runtimeTraffic.p2p_recv_errors || 0);

        document.getElementById('relay-rate').textContent = formatRate(p2pDelta, elapsed);
        document.getElementById('relay-total').textContent = formatBytes(p2pTotal);
        document.getElementById('relay-packets-desc').textContent = `${p2pPackets.toLocaleString()} 个 P2P 包`;
        document.getElementById('signaling-rate').textContent = `${msgRate < 10 ? msgRate.toFixed(1) : Math.round(msgRate)} msg/s`;
        document.getElementById('signaling-total').textContent = msgTotal.toLocaleString();
        document.getElementById('signaling-sub').textContent = `RX: ${runtimeTraffic.signaling_messages_received || 0} / TX: ${runtimeTraffic.signaling_messages_sent || 0} · 重连 ${runtimeTraffic.signaling_reconnects || 0}`;
        document.getElementById('health-status').textContent = tunErrors + p2pErrors;
        document.getElementById('relay-errors-desc').textContent = tunErrors + p2pErrors === 0
          ? '运行时链路健康良好'
          : `TUN ${tunErrors} / P2P ${p2pErrors} 次错误`;
        document.getElementById('uptime-display').textContent = formatDuration(runtimeTraffic.uptime_secs || 0);
        previousRuntimeTraffic = runtimeTraffic;
        previousTraffic = null;
        previousTrafficAt = now;
        return;
      }

      previousRuntimeTraffic = null;
      if (!traffic) {
        document.getElementById('relay-rate').textContent = '0 B/s';
        document.getElementById('relay-total').textContent = '0 B';
        document.getElementById('relay-packets-desc').textContent = '暂无协调中继数据';
        document.getElementById('signaling-rate').textContent = '0 msg/s';
        document.getElementById('signaling-total').textContent = '0';
        document.getElementById('signaling-sub').textContent = '运行时未连接';
        document.getElementById('health-status').textContent = '--';
        document.getElementById('relay-errors-desc').textContent = '等待真实运行时数据';
        document.getElementById('uptime-display').textContent = '--';
        previousTrafficAt = now;
        return;
      }

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
       赛博全景网络引力拓扑引擎 (Aerospace Cyber Telemetry Engine)
       包含弹簧力学模拟、光子能量粒子流、悬停 HUD 探针、右键上下文菜单
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
      const coordinatorActive = !!(lastStatusData && (lastStatusData.runtime_available || lastStatusData.coordinator_active));
      
      // 1. 整理与更新已知虚拟网络
      const discoveredNets = new Map();
      if (Array.isArray(networks) && networks.length > 0) {
        networks.forEach(n => {
          discoveredNets.set(n.name, {
            name: n.name,
            tun_name: n.tun_name || 'fuckipv6',
            assigned_ip: n.assigned_ip || 'fd00::/64',
            assigned_ipv4: n.assigned_ipv4 || null,
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
              assigned_ip: 'fd00::/64',
              assigned_ipv4: null,
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
          assigned_ip: 'fd00::/64',
          assigned_ipv4: null,
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
      core.label = '协调中心 (HUB)';
      core.sub = coordinatorActive ? '实时状态可用' : '未连接';
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

    canvas.addEventListener('contextmenu', e => {
      e.preventDefault();
      const rect = canvas.getBoundingClientRect();
      const mx = e.clientX - rect.left;
      const my = e.clientY - rect.top;
      const targetNode = nodes.find(n => Math.hypot(n.x - mx, n.y - my) < n.radius + 18);

      if (targetNode) {
        if (targetNode.isCore) {
          const status = lastStatusData || {};
          const items = [];
          if (currentTicket) {
            items.push({ svg: SVG_ICONS.copy, label: '复制当前网络邀请凭证 (fk://)', onClick: () => copyNetworkTicket() });
          }
          if (status.coordinator_active && status.coordinator_signaling_addr) {
            items.push({ svg: SVG_ICONS.copy, label: '复制信令服务地址', onClick: () => copyText(status.coordinator_signaling_addr, '信令服务地址') });
          }
          if (status.coordinator_active && status.coordinator_relay_addr) {
            items.push({ svg: SVG_ICONS.copy, label: '复制 QUIC 中继端点', onClick: () => copyText(status.coordinator_relay_addr, 'QUIC 中继端点') });
          }
          if (items.length) items.push({ divider: true });
          items.push(
            { svg: SVG_ICONS.refresh, label: '立即刷新全网拓扑', onClick: () => refresh(true) },
            { svg: SVG_ICONS.target, label: '视口节点居中归位', onClick: () => resetCanvasView() },
          );
          const connected = !!(status.runtime_available || status.coordinator_active);
          showContextMenu(e, `HUB · ${connected ? '协调中枢' : '未连接'}`, connected ? 'CORE' : 'OFFLINE', items);
        } else if (targetNode.isNetwork) {
          const net = targetNode.netData || {};
          const items = [
            { svg: SVG_ICONS.target, label: `聚焦此专网 (${net.name})`, onClick: () => setTopologyNetworkFilter(net.name) },
            { svg: SVG_ICONS.copy, label: '复制专网 IPv6 网段', onClick: () => copyText(net.assigned_ip || 'fd00::/64', '专网 IPv6 网段') },
            { svg: SVG_ICONS.copy, label: '复制虚拟网卡设备名', onClick: () => copyText(net.tun_name || 'fuckipv6', '网卡接口') },
            { divider: true },
            { svg: SVG_ICONS.target, label: '重置为全景拓扑视角', onClick: () => resetCanvasView() },
          ];
          showContextMenu(e, `虚拟专网 · ${net.name}`, `${net.peerCount || 0} 个对等节点在线`, items);
        } else {
          const p = targetNode.peerData || {};
          const cid = targetNode.id;
          const ipv6 = p.virtual_ip || '';
          const ipv4 = p.virtual_ipv4 || '';
          const eps = (p.endpoints || []).join(', ');
          const nets = targetNode.networkNames.join(', ');

          const items = [
            { svg: SVG_ICONS.copy, label: '复制客户端 ID', shortcut: shortId(cid), onClick: () => copyText(cid, '客户端 ID') },
            { svg: SVG_ICONS.copy, label: '复制所属专网', shortcut: nets, onClick: () => copyText(nets, '所属专网') },
            { svg: SVG_ICONS.ipv4, label: '复制虚拟 IPv4', shortcut: ipv4 || '无', onClick: () => copyText(ipv4, '虚拟 IPv4') },
            { svg: SVG_ICONS.ipv6, label: '复制虚拟 IPv6', shortcut: ipv6, onClick: () => copyText(ipv6, '虚拟 IPv6') },
            { svg: SVG_ICONS.location, label: '复制直连端点', onClick: () => copyText(eps, '直连端点') },
            { divider: true },
            { svg: SVG_ICONS.target, label: '锁定聚焦遥测卡片', shortcut: 'HUD', onClick: () => focusPeerHUD(cid) },
            { svg: SVG_ICONS.zap, label: '网络测速 (Ping)', onClick: () => pingPeer(cid) },
            { svg: SVG_ICONS.shield, label: '设为流量出口节点', onClick: () => setAsExitNode(cid) },
            { svg: SVG_ICONS.location, label: '在表格中定位', onClick: () => highlightPeerRow(cid) },
            { divider: true },
            { svg: SVG_ICONS.ban, label: '踢出并加入黑名单', danger: true, onClick: () => banClientPrompt(cid) },
          ];
          showContextMenu(e, `客户端 · ${shortId(cid)}`, formatNatType(p.nat_type) || '在线对等节点', items);
        }
      } else {
        const items = [
          { svg: SVG_ICONS.refresh, label: '立即刷新全网拓扑', onClick: () => refresh(true) },
          { svg: SVG_ICONS.target, label: '拓扑视口居中归位', onClick: () => resetCanvasView() },
          { divider: true },
          { svg: SVG_ICONS.sparkles, label: `链路动态微光: ${particlesEnabled ? '已开启' : '已停用'}`, onClick: () => { particlesEnabled = !particlesEnabled; showToast(particlesEnabled ? '动态微光已开启' : '动态微光已停用'); } },
        ];
        showContextMenu(e, '专网拓扑视窗控制', '拓扑视窗', items);
      }
    });

    function resetCanvasView() {
      if (!width || !height) return;
      currentNetworkFilter = '__all__';
      const select = document.getElementById('topo-network-filter');
      if (select) select.value = '__all__';
      updateTopologyToolbar();

      const core = nodes.find(n => n.isCore);
      if (core) { core.x = width / 2; core.y = height / 2; core.vx = 0; core.vy = 0; }
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
      showToast('拓扑全景视角与专网星系已复位居中');
    }

    function focusPeerHUD(clientId) {
      switchTab('tab-overview');
      const node = nodes.find(n => n.id === clientId);
      if (node) {
        hoveredNode = node;
        mousePos.x = node.x;
        mousePos.y = node.y;
        showToast(`已在拓扑中聚焦终端 ${shortId(clientId)}`);
      }
    }

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

      // 1. 空闲无节点空态呈现
      if (core && peerNodes.length === 0) {
        ctx.font = '500 12.5px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
        ctx.textAlign = 'center';
        ctx.fillStyle = isDark ? '#94a3b8' : '#64748b';
        ctx.fillText('虚拟专网拓扑就绪 · 暂无对等节点接入', core.x, core.y + 40);

        ctx.font = '400 11px ui-monospace, SFMono-Regular, monospace';
        ctx.fillStyle = isDark ? '#475569' : '#94a3b8';
        ctx.fillText('等待对端通过邀请凭证或 ticket 加入此网络...', core.x, core.y + 58);
      }

      // 2. 引力弹簧与斥力物理力学更新
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

          // 两个对等节点卡片 (148px x 28px) 之间精准胶囊边界排斥
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

      // 4. 绘制骨干与节点连线 (极简柔和细线，无同心虚线圆圈，无星系光斑)
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

        // 在线绿色状态指示圆点 (无描边)
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

        // 所属专网颜色标记点 (若属于多网络，显示多色点)
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

      // 9. 悬停 HUD 浮动遥测卡片
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
        title = 'fuckipv6 分布式协调中枢';
        const status = lastStatusData || {};
        const coordinatorActive = !!(status.runtime_available || status.coordinator_active);
        dotColor = coordinatorActive ? '#10b981' : '#64748b';
        const activePeerCount = nodes.filter(n => !n.isCore && !n.isNetwork).length;
        const netCount = knownNetworks.size;
        lines = [
          { k: '连接状态', v: coordinatorActive ? '● 实时运行时可用' : '○ 当前无活动连接', color: coordinatorActive ? '#10b981' : '#64748b' },
          { k: '专网拓扑', v: `${netCount} 个专网 · ${activePeerCount} 个终端互联` },
        ];
        if (status.coordinator_active) {
          lines.push(
            { k: '信令监听', v: status.coordinator_signaling_addr || '--' },
            { k: 'QUIC 中继', v: status.coordinator_relay_addr || '--' }
          );
        }
        const uptime = status.runtime_traffic && status.runtime_traffic.uptime_secs != null
          ? status.runtime_traffic.uptime_secs
          : (status.traffic && status.traffic.uptime_secs);
        if (coordinatorActive && uptime != null) {
          lines.push({ k: '运行时间', v: formatDuration(uptime) });
        }
      } else if (isNetwork) {
        const net = targetNode.netData || {};
        const memberCount = nodes.filter(n => !n.isCore && !n.isNetwork && n.networkNames.includes(net.name)).length;
        title = `虚拟专网 · ${net.name}`;
        dotColor = targetNode.colorTheme.hex;
        lines = [
          { k: '专网名称', v: net.name, color: targetNode.colorTheme.hex },
          { k: '虚拟接口', v: net.tun_name || 'fuckipv6' },
          { k: 'IPv6 网段', v: net.assigned_ip || 'fd00::/64', color: '#38bdf8' },
        ];
        if (net.assigned_ipv4) {
          lines.push({ k: 'IPv4 网段', v: net.assigned_ipv4, color: '#38bdf8' });
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
          { k: '在线时长', v: formatDuration(p.connected_secs || 0) },
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

      ctx.beginPath();
      ctx.arc(hudX + 16, hudY + 17, 3.5, 0, Math.PI * 2);
      ctx.fillStyle = dotColor;
      ctx.fill();

      ctx.font = '600 11.5px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
      ctx.textAlign = 'left';
      ctx.fillStyle = isDark ? '#ffffff' : '#0f172a';
      ctx.fillText(title, hudX + 26, hudY + 21);

      let curY = hudY + 38;
      lines.forEach(item => {
        ctx.font = '400 11px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
        ctx.fillStyle = isDark ? '#64748b' : '#94a3b8';
        ctx.fillText(item.k, hudX + 14, curY);

        ctx.font = '500 11px ui-monospace, SFMono-Regular, monospace';
        ctx.fillStyle = item.color || (isDark ? '#e2e8f0' : '#1e293b');
        ctx.textAlign = 'right';
        ctx.fillText(item.v, hudX + hudW - 14, curY);
        ctx.textAlign = 'left';
        curY += lineHeight;
      });

      ctx.font = '400 10px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
      ctx.fillStyle = isDark ? '#475569' : '#94a3b8';
      ctx.fillText(footerHint, hudX + 14, curY + 6);

      ctx.restore();
    }

    resizeCanvas();
    animateTopology();

    function highlightPeerRow(clientId) {
      switchTab('tab-peers');
      const rows = document.querySelectorAll('tbody tr');
      rows.forEach(r => r.classList.remove('highlight-row'));
      const target = document.getElementById(`peer-row-${clientId}`);
      if (target) {
        target.classList.add('highlight-row');
        target.scrollIntoView({ behavior: 'smooth', block: 'center' });
        showToast(`已定位对等节点 ${shortId(clientId)}`);
      }
    }

    /* ========================================================
       右键菜单控制 (纯 SVG 矢量图标)
       ======================================================== */
    const contextMenuEl = document.getElementById('cyber-context-menu');
    const cmHeaderEl = document.getElementById('cm-header');
    const cmItemsEl = document.getElementById('cm-items');
    window.__cmActions = [];

    function showContextMenu(e, title, tag, items) {
      e.preventDefault();
      e.stopPropagation();

      cmHeaderEl.innerHTML = `<span>${title}</span>${tag ? `<span class="cm-shortcut">${tag}</span>` : ''}`;
      window.__cmActions = [];

      let actionIdx = 0;
      cmItemsEl.innerHTML = items.map(item => {
        if (item.divider) return `<div class="cm-divider"></div>`;
        const curIdx = actionIdx++;
        window.__cmActions.push(item.onClick);
        return `
          <div class="cm-item ${item.danger ? 'danger' : ''}" onclick="execContextAction(${curIdx})">
            ${item.svg ? `<span class="cm-icon">${item.svg}</span>` : ''}
            <span class="cm-label">${item.label}</span>
            ${item.shortcut ? `<span class="cm-shortcut">${item.shortcut}</span>` : ''}
          </div>
        `;
      }).join('');

      contextMenuEl.style.display = 'block';
      const menuWidth = contextMenuEl.offsetWidth || 220;
      const menuHeight = contextMenuEl.offsetHeight || 200;

      let left = e.clientX;
      let top = e.clientY;

      if (left + menuWidth > window.innerWidth) left = Math.max(10, window.innerWidth - menuWidth - 12);
      if (top + menuHeight > window.innerHeight) top = Math.max(10, window.innerHeight - menuHeight - 12);

      contextMenuEl.style.left = `${left}px`;
      contextMenuEl.style.top = `${top}px`;
    }

    function execContextAction(index) {
      hideContextMenu();
      if (window.__cmActions && window.__cmActions[index]) {
        try { window.__cmActions[index](); } catch (err) {}
      }
    }

    function hideContextMenu() {
      if (contextMenuEl) contextMenuEl.style.display = 'none';
    }

    window.addEventListener('click', hideContextMenu);
    window.addEventListener('keydown', e => { if (e.key === 'Escape') hideContextMenu(); });

    function handlePeerContextMenu(e, clientId) {
      const p = cachedPeers.find(peer => peer.client_id === clientId) || {};
      const ipv6 = p.virtual_ip || '';
      const ipv4 = p.virtual_ipv4 || '';
      const eps = (p.endpoints || []).join(', ');

      const items = [
        { svg: SVG_ICONS.copy, label: '复制客户端 ID', shortcut: shortId(clientId), onClick: () => copyText(clientId, '客户端 ID') },
        { svg: SVG_ICONS.ipv4, label: '复制虚拟 IPv4', shortcut: ipv4 || '无', onClick: () => copyText(ipv4, '虚拟 IPv4') },
        { svg: SVG_ICONS.ipv6, label: '复制虚拟 IPv6', shortcut: ipv6, onClick: () => copyText(ipv6, '虚拟 IPv6') },
        { svg: SVG_ICONS.location, label: '复制全部端点', shortcut: `${(p.endpoints||[]).length}个`, onClick: () => copyText(eps, '公网端点') },
        { divider: true },
        { svg: SVG_ICONS.target, label: '在拓扑中聚焦此节点', onClick: () => focusPeerHUD(clientId) },
        { svg: SVG_ICONS.zap, label: '连通性测试 (Ping)', onClick: () => pingPeer(clientId) },
        { svg: SVG_ICONS.shield, label: '设为流量出口节点', onClick: () => setAsExitNode(clientId) },
        { divider: true },
        { svg: SVG_ICONS.ban, label: '移入黑名单并踢出', danger: true, onClick: () => banClientPrompt(clientId) },
      ];
      showContextMenu(e, `客户端 · ${shortId(clientId)}`, p.nat_type ? formatNatType(p.nat_type) : '在线', items);
    }

    function handleLogsContextMenu(e) {
      const items = [
        { svg: SVG_ICONS.copy, label: '复制全部运行日志', shortcut: 'Ctrl+C', onClick: () => copyLogs() },
        { svg: SVG_ICONS.trash, label: '清空当前日志控制台', shortcut: 'Clear', onClick: () => clearLogs() },
        { svg: SVG_ICONS.refresh, label: '立即拉取最新日志', shortcut: 'F5', onClick: () => refreshLogs() },
        { divider: true },
        { svg: SVG_ICONS.format, label: `视图切换: ${rawLogView ? '当前原始 (点此切换美化)' : '当前美化 (点此切换原始)'}`, onClick: () => toggleLogFormat() },
        { svg: SVG_ICONS.down, label: '滚动到底部', shortcut: 'End', onClick: () => scrollLogs(true) },
        { svg: SVG_ICONS.up, label: '滚动到顶部', shortcut: 'Home', onClick: () => scrollLogs(false) },
      ];
      showContextMenu(e, '系统运行日志控制', 'LOGS', items);
    }

    function scrollLogs(toBottom = true) {
      const logsEl = document.getElementById('logs');
      if (!logsEl) return;
      logsEl.scrollTop = toBottom ? logsEl.scrollHeight : 0;
    }

    async function pingPeer(clientId) {
      const peer = (cachedPeers || []).find(p => p.client_id === clientId);
      const targetLabel = peer ? (peer.virtual_ip || shortId(clientId)) : shortId(clientId);
      showToast(`正在向对等节点 ${targetLabel} 发送真实 P2P 探针...`);
      try {
        const result = await requestJson('/api/peers/probe', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ target: clientId, timeout_ms: 1500 })
        });
        if (result.timed_out) {
          showToast(`节点 ${shortId(clientId)} 探针超时，未收到 P2P Pong`);
          return;
        }
        const latency = result.rtt_us == null ? '未知' : `${(result.rtt_us / 1000).toFixed(1)} ms`;
        const channel = result.p2p_active ? '直连 (P2P Direct)' : '中继 (QUIC Relay)';
        showToast(`节点 ${shortId(clientId)} 连通正常，往返延迟 ${latency} [${channel}]`);
      } catch (e) {
        showToast(`节点探测失败: ${e.message}`);
      }
    }

    function setAsExitNode(clientId) {
      switchTab('tab-routing');
      const sel = document.getElementById('exit-node-select');
      if (sel) {
        sel.value = clientId;
        showToast(`已指定 ${shortId(clientId)} 为出口节点，请点击应用`);
      }
    }

    async function banClientPrompt(clientId) {
      if (!confirm(`确定要将客户端 ${shortId(clientId)} 加入黑名单并踢出吗？`)) return;
      try {
        const res = await requestJson('/api/blacklist/ban', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ client_id: clientId })
        });
        if (res.success) {
          showToast(`客户端 ${shortId(clientId)} 已被拉黑并强制下线`);
          refresh(true);
        } else {
          showToast(`拉黑失败: ${res.message || '未知错误'}`);
        }
      } catch (err) {
        showToast(`操作异常: ${err.message}`);
      }
    }

    /* ========================================================
       渲染总调度
       ======================================================== */
    function render(statusData, peers) {
      lastStatusData = statusData;
      cachedPeers = peers || [];
      const onlineCount = statusData.online_clients || 0;

      document.getElementById('status').textContent = `${onlineCount} 节点在线`;
      document.getElementById('online-count').textContent = onlineCount;
      document.getElementById('topology-count').textContent = `${onlineCount} 节点在线`;
      document.getElementById('tab-peers-badge').textContent = onlineCount;

      renderTraffic(statusData.traffic, statusData.runtime_traffic);
      renderTopology(peers, statusData.networks); // 兼容 renderTopology(peers)
      renderPeersTable(peers);
      renderMachineBar(statusData);
      renderRuntimeCard(statusData);
      renderCoordinatorSettings(statusData);

      const activeTicket = statusData.coordinator_active ? statusData.hosting_ticket : null;
      if (activeTicket) {
        currentTicket = activeTicket;
        const disp = document.getElementById('display-network-ticket');
        if (disp) disp.textContent = currentTicket;
        const btn = document.getElementById('btn-copy-network-ticket');
        if (btn) btn.style.display = 'inline-flex';
      } else {
        currentTicket = '';
        const disp = document.getElementById('display-network-ticket');
        if (disp) disp.textContent = '当前未托管自建网络';
        const btn = document.getElementById('btn-copy-network-ticket');
        if (btn) btn.style.display = 'none';
      }
    }

    function renderMachineBar(statusData) {
      currentClientStatus = statusData.status || statusData.client_status || 'offline';
      const isOnline = currentClientStatus === 'online';

      document.getElementById('bar-v6-val').textContent = statusData.virtual_ipv6 || '未分配';
      document.getElementById('bar-v4-val').textContent = statusData.virtual_ipv4 || '未分配';

      const barStatus = document.getElementById('bar-status-val');
      barStatus.textContent = isOnline ? '已在线' : (currentClientStatus === 'hosting' ? '中枢运行中' : '未连接 / 离线');
      barStatus.style.color = isOnline ? 'var(--green)' : 'var(--text-muted)';

      const mode = statusData.routing_mode || 'MeshOnly';
      const modeZh = mode === 'MeshOnly' ? '仅对等专网' : (mode === 'BypassMainland' ? '绕过大陆智能分流' : '全局对等代理');
      document.getElementById('bar-routing-val').textContent = modeZh;
      selectRoutingMode(mode, false);

      const btnTxt = document.getElementById('txt-toggle-connection');
      const btnEl = document.getElementById('btn-toggle-connection');
      if (isOnline) {
        btnTxt.textContent = '断开连接';
        btnEl.className = 'btn btn-danger';
      } else {
        btnTxt.textContent = '连接网络';
        btnEl.className = 'btn btn-primary';
      }

      // 出口节点下拉
      const exitSel = document.getElementById('exit-node-select');
      const currentExit = statusData.exit_node || '';
      const opts = ['<option value="">(自动寻找网络中可用出口网关)</option>'];
      cachedPeers.forEach(p => {
        const isSel = p.client_id === currentExit || p.virtual_ip === currentExit;
        opts.push(`<option value="${p.client_id}" ${isSel ? 'selected' : ''}>${p.virtual_ip} (${shortId(p.client_id)})</option>`);
      });
      exitSel.innerHTML = opts.join('');

      document.getElementById('is-exit-node-check').checked = !!statusData.is_exit_node;
    }

    function renderRuntimeCard(statusData) {
      const available = !!statusData.runtime_available;
      const network = (statusData.networks || [])[0] || {};
      const traffic = statusData.runtime_traffic || {};
      const stateTag = document.getElementById('runtime-card-state');
      stateTag.textContent = available ? '实时运行中' : '未连接';
      stateTag.style.color = available ? 'var(--green)' : 'var(--text-muted)';
      document.getElementById('runtime-tun-name').textContent = available ? (network.tun_name || '--') : '--';
      document.getElementById('runtime-p2p-addr').textContent = available ? (statusData.runtime_p2p_addr || '--') : '--';
      document.getElementById('runtime-uptime').textContent = available && traffic.uptime_secs != null ? formatDuration(traffic.uptime_secs) : '--';
      document.getElementById('runtime-source').textContent = available ? 'IPC 实时快照' : '无活动运行时';
    }

    function endpointPort(value) {
      if (!value) return '--';
      const index = value.lastIndexOf(':');
      return index >= 0 ? value.slice(index + 1) : value;
    }

    function renderCoordinatorSettings(statusData) {
      const active = !!statusData.coordinator_active;
      document.getElementById('btn-stop-host').style.display = active ? 'inline-flex' : 'none';
      document.getElementById('hub-signal-port').value = active ? endpointPort(statusData.coordinator_signaling_addr) : '--';
      document.getElementById('hub-relay-port').value = active ? endpointPort(statusData.coordinator_relay_addr) : '--';
      document.getElementById('hub-network-name').value = active ? (statusData.network_name || '--') : '--';
    }

    function selectRoutingMode(mode, doNotice = true) {
      currentSelectedRoutingMode = mode;
      document.getElementById('current-routing-mode-tag').textContent = mode;
      ['mesh', 'bypass', 'global'].forEach(m => {
        const el = document.getElementById(`mode-card-${m}`);
        if (el) el.classList.remove('active');
      });
      if (mode.toLowerCase().includes('bypass')) {
        document.getElementById('mode-card-bypass').classList.add('active');
      } else if (mode.toLowerCase().includes('global')) {
        document.getElementById('mode-card-global').classList.add('active');
      } else {
        document.getElementById('mode-card-mesh').classList.add('active');
      }
      if (doNotice) {
        showToast(`已选择 [${mode}] 策略，请点击保存应用`);
      }
    }

    async function applyRoutingConfig() {
      const mode = currentSelectedRoutingMode;
      const exit_node = document.getElementById('exit-node-select').value || null;
      const is_exit_node = document.getElementById('is-exit-node-check').checked;

      try {
        const updated = await requestJson('/api/routing', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ mode, exit_node, is_exit_node })
        });
        currentSelectedRoutingMode = updated.routing_mode || mode;
        showToast(`路由分流策略已生效: ${currentSelectedRoutingMode}`);
        refresh();
      } catch (e) {
        showToast(`应用分流失败: ${e.message}`);
      }
    }

    async function toggleLocalConnection() {
      if (currentClientStatus === 'online') {
        await clientDisconnect();
      } else {
        const target = document.getElementById('connect-target-input').value.trim() || '127.0.0.1:6666';
        document.getElementById('connect-target-input').value = target;
        await clientConnect();
      }
    }

    async function clientConnect() {
      const target = document.getElementById('connect-target-input').value.trim();
      if (!target) {
        showToast('请输入接入目标或 fk:// 凭证');
        return;
      }
      showToast('正在建立对等网络连接...');
      try {
        const state = await requestJson('/api/connect', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ target, routing_mode: currentSelectedRoutingMode })
        });
        if (!state || state.status !== 'connecting') {
          throw new Error('后端未确认连接已启动');
        }
        showToast('连接已启动，等待运行时分配地址');
        setTimeout(refresh, 800);
      } catch (e) {
        showToast(`连接失败: ${e.message}`);
      }
    }

    async function clientDisconnect() {
      showToast('正在断开网络连接...');
      try {
        const state = await requestJson('/api/disconnect', { method: 'POST' });
        if (!state || state.status !== 'offline') {
          throw new Error('后端未确认运行时已断开');
        }
        showToast('网络已断开');
        setTimeout(refresh, 500);
      } catch (e) {
        showToast(`断开失败: ${e.message}`);
      }
    }

    function renderPeersTable(peers) {
      const content = document.getElementById('content');
      if (!peers || !peers.length) {
        content.innerHTML = `
          <div class="empty-state">
            <span class="empty-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="3.5" width="19" height="13" rx="2.2"/><path d="M8 20.5h8M12 16.5v4"/></svg></span>
            <strong>等待对等节点接入专网</strong>
            <span>复制邀请凭证并分享给其他设备，即可自动协商接入。</span>
          </div>`;
        return;
      }

      const rows = peers.map(peer => {
        const isIdleActive = (peer.idle_secs || 0) < 15;
        const idleClass = isIdleActive ? 'active' : 'warn';
        const endpointsStr = (peer.endpoints && peer.endpoints.length) ? peer.endpoints.join(', ') : '-';
        const v4Str = peer.virtual_ipv4 || '';
        const v4Html = v4Str ? `<br><code style="font-size:10.5px;color:var(--accent);" title="虚拟 IPv4">${v4Str}</code>` : '';

        return `
          <tr id="peer-row-${peer.client_id}" oncontextmenu="handlePeerContextMenu(event, '${peer.client_id}')" style="cursor:context-menu;" title="右键快捷操作">
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
            <td style="font-family:var(--font-mono);color:var(--text-secondary);font-size:11px;">${endpointsStr}</td>
            <td>
              <div style="display:flex;gap:6px;">
                <button class="btn" style="padding:3px 8px;font-size:11px;" onclick="pingPeer('${peer.client_id}')">测速</button>
                <button class="btn" style="padding:3px 8px;font-size:11px;" onclick="setAsExitNode('${peer.client_id}')">出口</button>
                <button class="btn btn-danger" style="padding:3px 8px;font-size:11px;" onclick="banClientPrompt('${peer.client_id}')">踢出</button>
              </div>
            </td>
          </tr>
        `;
      }).join('');

      content.innerHTML = `
        <table>
          <thead>
            <tr>
              <th>虚拟 IP 地址</th>
              <th>客户端标识</th>
              <th>NAT 穿透类型</th>
              <th>心跳空闲</th>
              <th>在线时长</th>
              <th>公网物理端点</th>
              <th>快捷操作</th>
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
        const [statusData, peers] = await Promise.all([
          requestJson('/api/status'),
          requestJson('/api/peers')
        ]);
        render(statusData, peers);
        if (isManual) showToast('已拉取最新网络状态');
      } catch (e) {
        if (isManual) showToast('网络状态拉取异常');
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
        setTimeout(connectRealtime, 4000);
      };

      socket.onerror = () => socket.close();
    }

    /* ========================================================
       设置与黑名单
       ======================================================== */
    async function loadConfigAndBlacklist() {
      try {
        const [cfg, bl] = await Promise.all([
          requestJson('/api/config'),
          requestJson('/api/blacklist')
        ]);
        if (cfg.mode) document.getElementById('net-mode-select').value = cfg.mode;
        if (cfg.ipv4_pool) document.getElementById('net-ipv4-pool').value = cfg.ipv4_pool;
        if (cfg.ipv6_pool) document.getElementById('net-ipv6-pool').value = cfg.ipv6_pool;

        renderBlacklist(bl);
      } catch (e) {}
    }

    function renderBlacklist(bl) {
      const c = document.getElementById('blacklist-content');
      if (!bl || !bl.length) {
        c.innerHTML = `
          <div class="empty-state">
            <span class="empty-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/><path d="m9 12 2 2 4-4"/></svg></span>
            <strong>暂无被封禁的客户端</strong>
            <span>当前网络访问控制状态良好，未发现需要拦截的设备。</span>
          </div>`;
        return;
      }
      const rows = bl.map(id => `
        <tr>
          <td><code style="color:var(--red);">${id}</code></td>
          <td><button class="btn" style="padding:2px 8px;font-size:11px;color:var(--green);" onclick="unbanClient('${id}')">解除封禁</button></td>
        </tr>
      `).join('');
      c.innerHTML = `
        <table>
          <thead><tr><th>被封禁的客户端标识 (Client ID)</th><th>操作</th></tr></thead>
          <tbody>${rows}</tbody>
        </table>
      `;
    }

    async function unbanClient(id) {
      try {
        const res = await requestJson('/api/blacklist/unban', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ client_id: id })
        });
        if (!res || !res.success) {
          throw new Error((res && res.message) || '后端未确认解封成功');
        }
        showToast(res.message);
        await loadConfigAndBlacklist();
      } catch (e) {
        showToast(`解封失败: ${e.message}`);
      }
    }

    async function manualBanClient() {
      const id = document.getElementById('manual-ban-id').value.trim();
      if (!id) {
        showToast('请输入要封禁的客户端标识');
        return;
      }
      banClientPrompt(id);
      document.getElementById('manual-ban-id').value = '';
    }

    async function saveNetworkSettings() {
      const mode = document.getElementById('net-mode-select').value;
      const ipv4_pool = document.getElementById('net-ipv4-pool').value.trim();
      const ipv6_pool = document.getElementById('net-ipv6-pool').value.trim();

      try {
        const res = await requestJson('/api/config', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ mode, ipv4_pool, ipv6_pool })
        });

        if (!res || !res.success) {
          throw new Error((res && res.message) || '后端未确认更新成功');
        }
        showToast('专网参数更新成功并已生效');
      } catch (e) {
        showToast(`更新失败: ${e.message}`);
      }
    }

    /* ========================================================
       日志控制台
       ======================================================== */
    function formatLogLine(rawLine) {
      if (rawLogView) {
        return rawLine.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
      }

      let line = rawLine;
      line = line.replace(/^[A-Z][a-z]{2}\s+\d+\s+\d{2}:\d{2}:\d{2}\s+[\w.-]+\s+[\w.-]+\[\d+\]:\s*/, '');

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
        }
        line = line.slice(levelMatch[0].length);
      }

      let modTag = '';
      const modMatch = line.match(/^([\w_:]+):\s*/);
      if (modMatch) {
        const fullMod = modMatch[1];
        line = line.slice(modMatch[0].length);
        const cnMod = fullMod.split('::').pop();
        modTag = `<span class="log-mod">${cnMod}</span>`;
      }

      let msg = line
        .replace(/Client disconnected: ([\w+/=_-]+)/g, '客户端注销断开: $1')
        .replace(/Client closed connection: ([\d.:]+)/g, '客户端正常断开: $1')
        .replace(/WebSocket connected/g, 'WebSocket 信令链路已建立')
        .replace(/QUIC relay endpoint: (.+)/g, 'QUIC 中继服务端点: $1')
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
      msg = msg.replace(/\b(fd00::[0-9a-fA-F:]+|\b(?:10\.\d{1,3}\.\d{1,3}\.\d{1,3}|185\.217\.\d{1,3}\.\d{1,3}|127\.0\.0\.1)(?::\d+)?)\b/g, '<span class="log-ip">$1</span>');
      msg = msg.replace(/\b([a-zA-Z0-9_-]{32,44})\b/g, '<span class="log-id">$1</span>');

      const timeHtml = timeStr ? `<span class="log-time">[${timeStr}]</span>` : '';
      const badgeHtml = `<span class="log-badge ${levelClass}">${levelText}</span>`;

      return `<div class="log-row" data-level="${level}">${timeHtml} ${badgeHtml} ${modTag} <span class="log-msg">${msg}</span></div>`;
    }

    async function refreshLogs() {
      const service = document.getElementById('log-service-select').value;
      const lines = document.getElementById('log-lines-select').value;
      document.getElementById('log-service-badge').textContent = service;

      try {
        const response = await requestJson(`/api/logs?service=${encodeURIComponent(service)}&lines=${lines}`);
        const logsEl = document.getElementById('logs');
        if (response.error) {
          logsEl.innerHTML = `<div style="color:#f87171;padding:12px;">[读取日志错误] ${response.error}</div>`;
          return;
        }
        rawLogLines = response.lines || [];
        applyLogFilter();
      } catch (err) {
        document.getElementById('logs').innerHTML = `<div style="color:#f87171;padding:12px;">[日志异常] ${err.message}</div>`;
      }
    }

    function applyLogFilter() {
      const keyword = (document.getElementById('log-keyword-filter').value || '').trim().toLowerCase();
      const levelFilter = document.getElementById('log-level-filter').value;
      const logsEl = document.getElementById('logs');

      let filtered = rawLogLines;
      if (keyword) filtered = filtered.filter(l => l.toLowerCase().includes(keyword));
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
        logsEl.innerHTML = `<div style="color:var(--text-muted);padding:12px;">${keyword ? `无匹配 "${keyword}" 的日志` : '暂无日志记录'}</div>`;
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
      document.getElementById('logs').innerHTML = '<div style="color:var(--text-muted);padding:12px;">日志面板已清空。</div>';
      showToast('日志已清空');
    }

    function toggleLogFormat() {
      rawLogView = !rawLogView;
      const btn = document.getElementById('btn-toggle-format');
      if (btn) btn.innerHTML = rawLogView ? '<span>切换美化</span>' : '<span>切换原始</span>';
      applyLogFilter();
      showToast(rawLogView ? '已切换原始日志' : '已切换美化日志');
    }

    function copyLogs() {
      if (!rawLogLines.length) return;
      copyText(rawLogLines.join('\n'), '运行日志');
    }

    /* ========================================================
       Modals
       ======================================================== */
    function openGuideModal() {
      const host = window.location.host;
      document.getElementById('guide-code-block').textContent = `[client]
client_id = "node-alpha"
private_key = "client_key.pem"

[network]
signaling_server = "ws://${host}/ws"
p2p_port = 51820
tun_name = "fuckipv6"`;
      document.getElementById('guide-modal').classList.add('active');
    }
    function closeGuideModal() { document.getElementById('guide-modal').classList.remove('active'); }
    function copyGuideConfig() { copyText(document.getElementById('guide-code-block').textContent, '配置模版'); closeGuideModal(); }

    function openHostModal() { document.getElementById('host-modal').classList.add('active'); }
    function closeHostModal() { document.getElementById('host-modal').classList.remove('active'); }

    async function stopHostNetwork() {
      if (!confirm('确定停止本地自建网络并断开当前客户端运行时吗？')) return;
      showToast('正在停止自建网络...');
      try {
        const state = await requestJson('/api/stop_host', { method: 'POST' });
        if (!state || state.coordinator_active) {
          throw new Error('后端未确认自建网络已停止');
        }
        showToast('自建网络已停止');
        refresh();
      } catch (e) {
        showToast(`停止自建网络失败: ${e.message}`);
      }
    }

    async function submitHostNetwork() {
      const name = document.getElementById('host-net-name').value.trim() || 'default';
      const port = Number(document.getElementById('host-net-port').value) || 6666;
      const relay_port = Number(document.getElementById('host-net-relay-port').value) || 6667;
      const external_host = document.getElementById('host-net-external').value.trim() || null;

      showToast('正在创建并发布专网...');
      try {
        const state = await requestJson('/api/host', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ name, port, relay_port, auto_connect: true, external_host })
        });
        if (!state || !state.hosting_ticket) {
          throw new Error('后端未返回已发布的专网凭证');
        }
        closeHostModal();
        showToast('专网已创建并发布，正在等待运行时接入');
        refresh();
      } catch (e) {
        showToast(`建网失败: ${e.message}`);
      }
    }

    function toggleTheme() {
      const html = document.documentElement;
      const cur = html.getAttribute('data-theme') || 'dark';
      const target = cur === 'dark' ? 'light' : 'dark';
      html.setAttribute('data-theme', target);
      localStorage.setItem('fuckipv6_theme', target);
      const url = new URL(window.location.href);
      url.searchParams.set('theme', target);
      history.replaceState(null, '', url);
      showToast(`已切换至 ${target === 'dark' ? '深空暗黑' : '明亮'} 模式`);
    }

    (function initTheme() {
      const params = new URLSearchParams(window.location.search);
      const forced = params.get('theme');
      if (forced === 'dark' || forced === 'light') {
        document.documentElement.setAttribute('data-theme', forced);
        return;
      }
      const saved = localStorage.getItem('fuckipv6_theme');
      if (saved) document.documentElement.setAttribute('data-theme', saved);
    })();

    (function initTabFromQuery() {
      const params = new URLSearchParams(window.location.search);
      const tab = params.get('tab');
      const target = tab ? document.getElementById(tab) : null;
      if (target && target.classList.contains('tab-content')) switchTab(tab);
    })();

    refresh();
    connectRealtime();
    refreshLogs();
  </script>
</body>
</html>"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unified_master_html_zero_emoji_and_svg_icons() {
        // 彻底验证绝无任何 Emoji
        for ch in UNIFIED_MASTER_HTML.chars() {
            let u = ch as u32;
            assert!(
                !(0x1F000..=0x1FAFF).contains(&u) && !(0x2600..=0x27BF).contains(&u),
                "发现非法 Emoji 字符: U+{:X} ({})",
                u,
                ch
            );
        }

        // 验证 SVG 矢量图标库
        assert!(UNIFIED_MASTER_HTML.contains("<svg"));
        assert!(UNIFIED_MASTER_HTML.contains("SVG_ICONS"));
        assert!(UNIFIED_MASTER_HTML.contains("cyber-context-menu"));
    }

    #[test]
    fn test_unified_master_html_design_system() {
        // 新版控制台采用共享设计系统：侧边栏 + 分层面板 + 明暗双主题
        assert!(UNIFIED_MASTER_HTML.contains("app-shell"));
        assert!(UNIFIED_MASTER_HTML.contains("sidebar"));
        assert!(UNIFIED_MASTER_HTML.contains("panel-head"));
        assert!(UNIFIED_MASTER_HTML.contains(concat!("v", env!("CARGO_PKG_VERSION"))));
        assert!(UNIFIED_MASTER_HTML.contains("/assets/webui.css"));
        assert!(WEBUI_CSS.contains("--sidebar-w"));
        assert!(WEBUI_CSS.contains("--panel"));
        assert!(WEBUI_CSS.contains(".modal-wrap.active"));
        assert!(WEBUI_CSS.contains("[data-theme=\"light\"]"));
        assert!(WEBUI_CSS.contains("@media (max-width: 900px)"));
    }

    #[test]
    fn test_unified_master_html_five_core_tabs() {
        // 验证 5 大现代化控制标签页完整存在
        assert!(UNIFIED_MASTER_HTML.contains("全景拓扑大盘"));
        assert!(UNIFIED_MASTER_HTML.contains("对等节点与终端"));
        assert!(UNIFIED_MASTER_HTML.contains("原生智能分流"));
        assert!(UNIFIED_MASTER_HTML.contains("专网中枢设置"));
        assert!(UNIFIED_MASTER_HTML.contains("系统运行日志"));
    }

    #[test]
    fn test_unified_master_html_topology_and_physics_engine() {
        // 验证原 8080 赛博航天引力拓扑遥测引擎完全补全
        assert!(UNIFIED_MASTER_HTML.contains("canvas id=\"topology\""));
        assert!(UNIFIED_MASTER_HTML.contains("CyberNode"));
        assert!(UNIFIED_MASTER_HTML.contains("animateTopology"));
        assert!(UNIFIED_MASTER_HTML.contains("radarAngle"));
        assert!(UNIFIED_MASTER_HTML.contains("particlesEnabled"));
        assert!(UNIFIED_MASTER_HTML.contains("handlePeerContextMenu"));
        assert!(UNIFIED_MASTER_HTML.contains("handleLogsContextMenu"));
        assert!(UNIFIED_MASTER_HTML.contains("focusPeerHUD"));
    }

    #[test]
    fn test_unified_master_html_complete_settings_and_routing() {
        // 验证专网 IP 池设置、中枢端口、邀请凭证、黑名单与原生智能分流
        assert!(UNIFIED_MASTER_HTML.contains("net-mode-select"));
        assert!(UNIFIED_MASTER_HTML.contains("net-ipv4-pool"));
        assert!(UNIFIED_MASTER_HTML.contains("net-ipv6-pool"));
        assert!(UNIFIED_MASTER_HTML.contains("hub-signal-port"));
        assert!(UNIFIED_MASTER_HTML.contains("hub-relay-port"));
        assert!(UNIFIED_MASTER_HTML.contains("hub-external-host"));
        assert!(UNIFIED_MASTER_HTML.contains("display-network-ticket"));
        assert!(UNIFIED_MASTER_HTML.contains("copyCliCommand"));
        assert!(UNIFIED_MASTER_HTML.contains("manual-ban-id"));
        assert!(UNIFIED_MASTER_HTML.contains("exit-node-select"));
        assert!(UNIFIED_MASTER_HTML.contains("is-exit-node-check"));
    }

    #[test]
    fn test_control_actions_only_report_confirmed_success() {
        assert!(UNIFIED_MASTER_HTML.contains("if (!response.ok)"));
        assert!(UNIFIED_MASTER_HTML.contains("state.status !== 'connecting'"));
        assert!(UNIFIED_MASTER_HTML.contains("state.status !== 'offline'"));
        assert!(UNIFIED_MASTER_HTML.contains("if (!res || !res.success)"));
        assert!(UNIFIED_MASTER_HTML.contains("!state.hosting_ticket"));
        assert!(!UNIFIED_MASTER_HTML.contains("已下发连接指令"));
        assert!(!UNIFIED_MASTER_HTML.contains("await fetch('/api/host'"));
    }

    #[test]
    fn test_runtime_card_uses_ipc_snapshot_instead_of_static_platform_values() {
        assert!(UNIFIED_MASTER_HTML.contains("renderRuntimeCard"));
        assert!(UNIFIED_MASTER_HTML.contains("runtime-p2p-addr"));
        assert!(UNIFIED_MASTER_HTML.contains("runtime-card-state"));
        assert!(UNIFIED_MASTER_HTML.contains("IPC 实时快照"));
        assert!(!UNIFIED_MASTER_HTML.contains("1420 字节"));
        assert!(!UNIFIED_MASTER_HTML.contains("Linux x86_64 Musl"));
    }

    #[test]
    fn test_host_lifecycle_controls_require_runtime_confirmation() {
        assert!(UNIFIED_MASTER_HTML.contains("stopHostNetwork"));
        assert!(UNIFIED_MASTER_HTML.contains("/api/stop_host"));
        assert!(UNIFIED_MASTER_HTML.contains("state.coordinator_active"));
        assert!(UNIFIED_MASTER_HTML.contains("renderCoordinatorSettings"));
        assert!(!UNIFIED_MASTER_HTML.contains("value=\"6666\" readonly"));
        assert!(!UNIFIED_MASTER_HTML.contains("value=\"relay.xlei.cc\" readonly"));
    }

    #[test]
    fn test_offline_ui_does_not_retain_stale_tickets_or_coordinator_status() {
        assert!(UNIFIED_MASTER_HTML.contains(
            "const activeTicket = statusData.coordinator_active ? statusData.hosting_ticket : null"
        ));
        assert!(UNIFIED_MASTER_HTML.contains("当前未托管自建网络"));
        assert!(UNIFIED_MASTER_HTML.contains("status.coordinator_signaling_addr || '--'"));
        assert!(!UNIFIED_MASTER_HTML.contains("核心协调网络运行中"));
        assert!(!UNIFIED_MASTER_HTML.contains("UDP:6667 (QUIC)"));
    }

    #[test]
    fn blacklist_contains_matches_trimmed_client_ids() {
        let blacklist = vec![" node-bad ".to_string(), "node-other".to_string()];
        assert!(blacklist_contains(&blacklist, "node-bad"));
        assert!(blacklist_contains(&blacklist, " node-bad "));
        assert!(!blacklist_contains(&blacklist, "node-missing"));
    }

    #[tokio::test]
    async fn blacklist_mutations_fail_closed_without_coordinator() {
        let ctrl = Arc::new(AppController::new());

        let ban_error = handle_blacklist_ban(
            State(ctrl.clone()),
            Json(BlacklistActionRequest {
                client_id: "node-bad".to_string(),
            }),
        )
        .await
        .expect_err("ban without an active coordinator must not report success");
        let (ban_status, Json(ban_body)) = ban_error;
        assert_eq!(ban_status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!ban_body.success);
        assert!(ban_body.message.contains("本地协调服务未运行"));
        assert!(ban_body.blacklist.is_empty());

        let unban_error = handle_blacklist_unban(
            State(ctrl),
            Json(BlacklistActionRequest {
                client_id: "node-bad".to_string(),
            }),
        )
        .await
        .expect_err("unban without an active coordinator must not report success");
        let (unban_status, Json(unban_body)) = unban_error;
        assert_eq!(unban_status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!unban_body.success);
        assert!(unban_body.message.contains("本地协调服务未运行"));
        assert!(unban_body.blacklist.is_empty());
    }
}
