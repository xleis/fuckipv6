pub mod controller;
mod node_surface;
mod owner_bootstrap;
mod public_claim;
pub mod security;
mod tls;

use crate::config_manager::ConfigManager;
use crate::node::{
    NodeCommandContext, NodeHttpAdapter, NodeProblem, NodeProblemCode, NodeStateStore,
};
use crate::owner_security::service::{
    AuthenticatedOwnerSession, IssuedOwnerSession, OwnerBootstrapChannel, OwnerSecurityError,
    OwnerSecurityService,
};
use crate::ui::{resolve_ui_mode, SurfaceLease, UiCapabilities, UiModeRequest};
use anyhow::{Context, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, Extension, Path as AxumPath, Query, State,
    },
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Json, Redirect, Response},
    routing::{delete, get, post},
    Router,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
pub use controller::{
    AppController, AppControllerError, AppUiState, BanRequest, ConnectRequest, CoordinatorPeerDto,
    CoordinatorTrafficDto, HostRequest, LogsResponse, PeerUiInfo, RuntimeTrafficDto,
    UnifiedSnapshotResponse, UpdateRoutingRequest,
};
use fuckipv6_common::domain::PasskeyCredentialId;
use fuckipv6_common::i18n::{LanguagePreference, Locale};
use fuckipv6_coordinator::signaling::session::PeerSnapshot;
use fuckipv6_coordinator::webui::{
    BlacklistActionRequest, BlacklistActionResponse, NetworkConfigApiResponse, NetworkConfigDto,
    UpdateNetworkConfigRequest,
};
use public_claim::PublicClaimConfig;
use security::{http_url, MutationRateLimiter, WebUiAuth, WebUiConfig, WebUiLoginError};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tls::WebUiTlsConfig;
use webauthn_rs::prelude::{PublicKeyCredential, RegisterPublicKeyCredential, Url};

pub use security::{
    DEFAULT_WEBUI_BIND, DEFAULT_WEBUI_PORT, ENV_WEBUI_ALLOW_REMOTE, ENV_WEBUI_BIND,
    ENV_WEBUI_JWT_SECRET, ENV_WEBUI_PASSWORD, ENV_WEBUI_USERNAME,
};

pub fn create_app_web_router(controller: Arc<AppController>) -> Result<Router> {
    Ok(create_app_web_router_with_auth(
        controller,
        WebUiAuth::from_process_env()?,
    ))
}

/// Build a WebUI router backed by the shared Node HTTP adapter.
///
/// This is the migration boundary for EX-10/NX-10: the browser surface fetches
/// the canonical manifest/snapshot endpoints and submits typed commands through
/// the same adapter used by native and programmatic clients.
pub fn create_app_web_router_with_node(
    controller: Arc<AppController>,
    node_adapter: NodeHttpAdapter,
) -> Result<Router> {
    Ok(create_app_web_router_with_auth_and_node(
        controller,
        WebUiAuth::from_process_env()?,
        Some(node_adapter),
    ))
}

fn create_app_web_router_with_auth(controller: Arc<AppController>, auth: WebUiAuth) -> Router {
    create_app_web_router_with_auth_and_node(controller, auth, None)
}

fn create_app_web_router_with_auth_and_node(
    controller: Arc<AppController>,
    auth: WebUiAuth,
    node_adapter: Option<NodeHttpAdapter>,
) -> Router {
    create_app_web_router_with_auth_node_and_owner(controller, auth, node_adapter, None)
}

fn create_app_web_router_with_auth_node_and_owner(
    controller: Arc<AppController>,
    auth: WebUiAuth,
    node_adapter: Option<NodeHttpAdapter>,
    owner_security: Option<Arc<OwnerSecurityService>>,
) -> Router {
    create_app_web_router_with_auth_node_owner_and_public_claim(
        controller,
        auth,
        node_adapter,
        owner_security,
        None,
    )
}

fn create_app_web_router_with_auth_node_owner_and_public_claim(
    controller: Arc<AppController>,
    auth: WebUiAuth,
    node_adapter: Option<NodeHttpAdapter>,
    owner_security: Option<Arc<OwnerSecurityService>>,
    public_claim: Option<Arc<PublicClaimConfig>>,
) -> Router {
    create_app_web_router_with_mutation_limiter(
        controller,
        auth,
        node_adapter,
        owner_security,
        public_claim,
        Arc::new(MutationRateLimiter::default()),
        None,
    )
}

fn create_app_web_router_with_mutation_limiter(
    controller: Arc<AppController>,
    auth: WebUiAuth,
    node_adapter: Option<NodeHttpAdapter>,
    owner_security: Option<Arc<OwnerSecurityService>>,
    public_claim: Option<Arc<PublicClaimConfig>>,
    mutation_limiter: Arc<MutationRateLimiter>,
    language_config: Option<ConfigManager>,
) -> Router {
    let auth = Arc::new(auth);
    let direct_tls = public_claim
        .as_ref()
        .is_some_and(|claim| claim.direct_tls());
    let protected = Router::new()
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
        .with_state(controller);
    let node_state = node_adapter.as_ref().map(NodeHttpAdapter::state_store);
    let protected = match node_adapter {
        Some(adapter) => protected.merge(adapter.router()),
        None => protected,
    };
    let protected = match language_config {
        Some(config) => protected
            .route(
                "/api/v1/settings/language",
                get(handle_get_language_settings).put(handle_put_language_settings),
            )
            .layer(Extension(config)),
        None => protected,
    };
    let protected = protected.route_layer(middleware::from_fn_with_state(
        ApiAuthState {
            auth: auth.clone(),
            direct_tls,
            owner: owner_security.clone(),
            public_claim: public_claim.clone(),
            mutation_limiter,
            node_state,
        },
        require_api_auth,
    ));

    let owner_routes = owner_security
        .as_ref()
        .map(|owner| owner_security_router(owner.clone(), public_claim.clone()));

    let mut app = Router::new()
        .route("/", get(handle_index))
        .route("/assets/webui.css", get(handle_webui_css))
        .route("/assets/i18n.js", get(handle_i18n_js))
        .route("/assets/app.js", get(handle_node_app_js))
        .route("/assets/app.css", get(handle_node_app_css))
        .route("/assets/owner_bootstrap.js", get(handle_owner_bootstrap_js))
        .route("/api/auth/login", post(handle_login))
        .merge(protected);
    if let Some(owner_routes) = owner_routes {
        app = app.merge(owner_routes);
    }
    let security_headers = SecurityHeadersState {
        direct_tls,
        public_claim,
    };
    app.layer(Extension(auth))
        .layer(Extension(WebUiSurface {
            owner_security,
            direct_tls,
        }))
        .layer(middleware::from_fn_with_state(
            security_headers,
            apply_security_headers,
        ))
}

fn ensure_webui_start_allowed(capabilities: UiCapabilities) -> Result<()> {
    let resolution = resolve_ui_mode(UiModeRequest::WebUi, capabilities)
        .context("当前主机存在桌面会话，拒绝启动 WebUI")?;
    debug_assert!(resolution.effective.is_web_ui());
    Ok(())
}

/// Start the only production WebUI surface: a manifest-driven view backed by
/// the authenticated node command/state/event adapters.
pub(crate) async fn start_app_webui_with_node(
    port: u16,
    controller: Arc<AppController>,
    node_adapter: NodeHttpAdapter,
    surface_lease: SurfaceLease,
) -> Result<tokio::task::JoinHandle<()>> {
    // Keep this gate at the actual listener boundary, not only in CLI option
    // parsing, so every future caller is subject to the same desktop policy.
    ensure_webui_start_allowed(UiCapabilities::detect())?;
    let tls_config = WebUiTlsConfig::from_process_env()?;
    let direct_tls = tls_config.is_some();
    let config = WebUiConfig::from_process_env_with_direct_tls(port, direct_tls)?;
    let public_claim = PublicClaimConfig::from_process_env(direct_tls)?.map(Arc::new);
    if public_claim
        .as_ref()
        .is_some_and(|claim| !claim.direct_tls())
        && config.is_remote()
    {
        anyhow::bail!(
            "反向代理公网认领只允许转发到回环监听；禁止让 WebUI 直接以明文 HTTP 监听公网"
        );
    }
    config.validate_runtime_exposure()?;
    let rustls_config = match (&tls_config, &public_claim) {
        (Some(tls), Some(claim)) => Some(
            tls.rustls_config(claim.host())
                .await
                .context("WebUI 直接 HTTPS 配置失败")?,
        ),
        (Some(_), None) => unreachable!("direct TLS requires a validated public origin"),
        (None, _) => None,
    };
    let addr = config.socket_addr();
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("WebUI 无法监听 {addr}"))?;
    let local_addr = listener.local_addr().context("读取 WebUI 监听地址失败")?;
    let (http_listener, https_server) = match rustls_config {
        Some(rustls_config) => {
            let listener = listener.into_std().context("转换为 HTTPS 监听器失败")?;
            let server = axum_server::from_tcp_rustls(listener, rustls_config)
                .context("创建 WebUI HTTPS 服务失败")?;
            (None, Some(server))
        }
        None => (Some(listener), None),
    };
    let owner_config = ConfigManager::new().context("无法读取节点所有者配置目录")?;
    let (owner_security, bootstrap_code) = OwnerSecurityService::from_config_with_public_origin(
        &owner_config,
        local_addr.port(),
        public_claim.as_ref().map(|claim| claim.origin()),
    )
    .context("无法初始化节点所有者安全边界")?;
    if let Some(bootstrap_code) = bootstrap_code {
        write_bootstrap_code_to_terminal(
            bootstrap_code.as_str(),
            local_addr.port(),
            public_claim.as_deref(),
        )?;
    }
    let app = create_app_web_router_with_mutation_limiter(
        controller.clone(),
        config.auth().clone(),
        Some(node_adapter),
        Some(Arc::new(owner_security)),
        public_claim.clone(),
        Arc::new(MutationRateLimiter::default()),
        Some(owner_config),
    );

    let displayed_url = public_claim
        .as_ref()
        .map(|claim| claim.origin_string().to_string())
        .unwrap_or_else(|| http_url(local_addr));
    tracing::info!(
        listen = %local_addr,
        remote = config.is_remote(),
        authentication = if config.auth().is_required() { "required" } else { "disabled" },
        public_claim = public_claim
            .as_ref()
            .map(|claim| if claim.direct_tls() { "direct-https" } else { "verified-reverse-proxy" }),
        "fuckipv6 统一控制台已启动: {displayed_url}"
    );

    // 定期刷新在线 Peers 状态
    let ctrl = controller.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            ctrl.refresh_peers().await;
        }
    });

    let handle = tokio::spawn(async move {
        // The caller must acquire this lease before constructing a WebUI. Keep
        // it in the listener task so the runtime directory cannot admit GPUI or
        // a second WebUI until this server has actually stopped.
        let _surface_lease = surface_lease;
        let result = match (http_listener, https_server) {
            (Some(listener), None) => axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .map_err(|error| error.to_string()),
            (None, Some(server)) => server
                .serve(app.into_make_service_with_connect_info::<SocketAddr>())
                .await
                .map_err(|error| error.to_string()),
            _ => Err("WebUI 监听器配置无效".to_string()),
        };
        if let Err(err) = result {
            tracing::error!("WebUI 服务发生错误: {err}");
        }
    });

    Ok(handle)
}

#[cfg(unix)]
fn write_bootstrap_code_to_terminal(
    code: &str,
    port: u16,
    public_claim: Option<&PublicClaimConfig>,
) -> Result<()> {
    use std::io::Write;

    // `/dev/tty` bypasses redirected stdout/stderr and therefore service
    // journals, pipe collectors, and persistent process logs. If there is no
    // controlling terminal, keep the WebUI and node control plane running but
    // leave owner initialization locked. Never fall back to a log sink. The
    // owner can restart interactively to display a fresh one-time code.
    let mut terminal = match std::fs::OpenOptions::new().write(true).open("/dev/tty") {
        Ok(terminal) => terminal,
        Err(error) => {
            tracing::warn!(
                %error,
                "当前进程没有控制终端；所有者初始化保持锁定，请从本机终端重新启动以显示一次性初始化码"
            );
            return Ok(());
        }
    };
    writeln!(
        terminal,
        "------------------------------------------------------------------------"
    )
    .context("无法向本机控制终端显示初始化说明")?;
    writeln!(terminal, "🔐 本机所有者初始化码（仅显示一次）: {code}")
        .context("无法向本机控制终端显示初始化码")?;
    if public_claim.is_some_and(PublicClaimConfig::direct_tls) {
        writeln!(terminal, "所有者初始化: 使用公网 HTTPS 地址")
            .context("无法向本机控制终端显示初始化地址")?;
    } else {
        writeln!(terminal, "本机初始化: http://localhost:{port}/")
            .context("无法向本机控制终端显示初始化地址")?;
    }
    if let Some(public_claim) = public_claim {
        let transport = if public_claim.direct_tls() {
            "节点直接提供 HTTPS"
        } else {
            "仅通过受信 HTTPS 反向代理"
        };
        writeln!(
            terminal,
            "公网认领: {}/（{transport}）",
            public_claim.origin_string()
        )
        .context("无法向本机控制终端显示公网初始化地址")?;
    }
    writeln!(
        terminal,
        "------------------------------------------------------------------------"
    )
    .context("无法向本机控制终端显示初始化说明")?;
    terminal.flush().context("无法刷新本机控制终端输出")
}

#[cfg(not(unix))]
fn write_bootstrap_code_to_terminal(
    code: &str,
    port: u16,
    public_claim: Option<&PublicClaimConfig>,
) -> Result<()> {
    use std::io::{IsTerminal, Write};

    let mut terminal = std::io::stderr();
    if !terminal.is_terminal() {
        tracing::warn!(
            "当前进程没有控制终端；所有者初始化保持锁定，请从本机终端重新启动以显示一次性初始化码"
        );
        return Ok(());
    }
    writeln!(
        terminal,
        "------------------------------------------------------------------------"
    )?;
    writeln!(terminal, "🔐 本机所有者初始化码（仅显示一次）: {code}")?;
    if public_claim.is_some_and(PublicClaimConfig::direct_tls) {
        writeln!(terminal, "所有者初始化: 使用上方公网 HTTPS 地址")?;
    } else {
        writeln!(terminal, "本机初始化: http://localhost:{port}/")?;
    }
    if let Some(public_claim) = public_claim {
        let transport = if public_claim.direct_tls() {
            "节点直接提供 HTTPS"
        } else {
            "仅通过受信 HTTPS 反向代理"
        };
        writeln!(
            terminal,
            "公网认领: {}/（{transport}）",
            public_claim.origin_string()
        )?;
    }
    writeln!(
        terminal,
        "------------------------------------------------------------------------"
    )?;
    terminal.flush()?;
    Ok(())
}

#[derive(Clone)]
struct ApiAuthState {
    auth: Arc<WebUiAuth>,
    direct_tls: bool,
    owner: Option<Arc<OwnerSecurityService>>,
    public_claim: Option<Arc<PublicClaimConfig>>,
    mutation_limiter: Arc<MutationRateLimiter>,
    node_state: Option<NodeStateStore>,
}

async fn attach_owner_command_context(
    state: &ApiAuthState,
    request: &mut axum::extract::Request,
    source: &str,
    session: Option<&AuthenticatedOwnerSession>,
) {
    let Some(node_state) = &state.node_state else {
        return;
    };
    let snapshot = node_state.snapshot().await;
    let context = match session {
        Some(session) => NodeCommandContext::owner_session_for_snapshot(
            &snapshot,
            source,
            session.recently_authenticated(unix_time_ms()),
        ),
        // Test and explicitly disabled-auth routers do not carry a WebUI owner
        // service. Their loopback caller remains the local OS owner.
        None => NodeCommandContext::local_owner_for_snapshot(&snapshot, source),
    };
    request.extensions_mut().insert(context);
}

async fn require_api_auth(
    State(state): State<ApiAuthState>,
    mut request: axum::extract::Request,
    next: Next,
) -> Response {
    let connect_ip = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(remote)| remote.ip());
    let has_forwarding = request_has_forwarding_headers(&request);
    let is_public =
        has_forwarding || (state.direct_tls && connect_ip.is_none_or(|ip| !ip.is_loopback()));
    let is_mutation = request_is_mutation(&request);

    if is_public {
        let Some(public_claim) = &state.public_claim else {
            return unauthorized_response("公网管理 API 需要已验证的 HTTPS 边界和所有者认证");
        };
        let Some(peer) = connect_ip else {
            return unauthorized_response("公网管理 API 缺少可信来源");
        };
        let client_ip = match public_claim.authorize(peer, request.method(), request.headers()) {
            Ok(client_ip) => client_ip,
            Err(_) => return unauthorized_response("公网管理 API 来源校验失败"),
        };
        let Some(owner) = &state.owner else {
            return unauthorized_response("公网管理 API 需要节点所有者 Passkey 会话");
        };
        let Some(token) = owner_session_cookie(request.headers()) else {
            return unauthorized_response("公网管理 API 需要节点所有者 Passkey 会话");
        };
        let session = match owner.authenticate_session(OwnerBootstrapChannel::Public, token) {
            Ok(session) => session,
            Err(_) => return unauthorized_response("节点所有者 Passkey 会话无效或已过期"),
        };
        if is_mutation {
            if let Err(retry_after) = state.mutation_limiter.check(client_ip) {
                return mutation_rate_limited_response(retry_after);
            }
        }
        attach_owner_command_context(&state, &mut request, "webui_public_owner", Some(&session))
            .await;
        return next.run(request).await;
    }

    // Production WebUI routers always carry an owner security service. Once it
    // exists, every protected control-plane request must present a valid Local
    // owner session, including requests received over loopback. There is no
    // anonymous loopback, legacy bearer, or trusted-network shortcut.
    if let Some(owner) = &state.owner {
        let Some(token) = owner_session_cookie(request.headers()) else {
            return unauthorized_response("控制面认证失败：请使用节点所有者 Passkey 登录");
        };
        let session = match owner.authenticate_session(OwnerBootstrapChannel::Local, token) {
            Ok(session) => session,
            Err(_) => return unauthorized_response("节点所有者会话无效或已过期"),
        };
        if is_mutation && !local_mutation_has_same_origin(&state, &request) {
            return forbidden_response("拒绝跨站状态变更请求：Origin 或 Sec-Fetch-Site 校验失败");
        }
        if is_mutation {
            let source_ip = connect_ip.unwrap_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
            if let Err(retry_after) = state.mutation_limiter.check(source_ip) {
                return mutation_rate_limited_response(retry_after);
            }
        }
        attach_owner_command_context(&state, &mut request, "webui_local_owner", Some(&session))
            .await;
        return next.run(request).await;
    }

    // Browser WebSocket clients authenticate with the same-origin owner cookie
    // checked above. Never accept credentials in URL query strings because they
    // leak into access logs, browser history, and proxy traces.
    let bearer_token = authorization_bearer_token(&request)
        .or_else(|| websocket_subprotocol_bearer_token(request.headers()));
    let bearer_authenticated = state.auth.is_required()
        && bearer_token.is_some_and(|token| state.auth.verify(token).is_ok());

    if state.auth.is_required() && !bearer_authenticated {
        return unauthorized_response("控制面认证失败：请通过 /api/auth/login 获取 Bearer Token");
    }

    if is_mutation && !bearer_authenticated && !local_mutation_has_same_origin(&state, &request) {
        return forbidden_response("拒绝跨站状态变更请求：Origin 或 Sec-Fetch-Site 校验失败");
    }

    if is_mutation {
        let source_ip = connect_ip.unwrap_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
        if let Err(retry_after) = state.mutation_limiter.check(source_ip) {
            return mutation_rate_limited_response(retry_after);
        }
    }

    let source = if bearer_authenticated {
        "webui_bearer_owner"
    } else {
        "webui_loopback_owner"
    };
    attach_owner_command_context(&state, &mut request, source, None).await;
    next.run(request).await
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().try_into().unwrap_or(u64::MAX))
        .unwrap_or(1)
        .max(1)
}

fn request_is_mutation(request: &axum::extract::Request) -> bool {
    !matches!(
        request.method(),
        &axum::http::Method::GET | &axum::http::Method::HEAD | &axum::http::Method::OPTIONS
    )
}

fn local_mutation_has_same_origin(state: &ApiAuthState, request: &axum::extract::Request) -> bool {
    let headers = request.headers();
    let Some(origin) = single_header_value(headers, header::ORIGIN.as_str()) else {
        return false;
    };
    let Ok(origin) = Url::parse(origin) else {
        return false;
    };
    if !origin.username().is_empty()
        || origin.password().is_some()
        || origin.query().is_some()
        || origin.fragment().is_some()
        || origin.path() != "/"
    {
        return false;
    }

    let expected_origin = if state.direct_tls {
        let Some(public_claim) = &state.public_claim else {
            return false;
        };
        public_claim.origin().origin().ascii_serialization()
    } else if let Some(owner) = &state.owner {
        match owner.origin_for(OwnerBootstrapChannel::Local) {
            Ok(origin) => origin.to_string(),
            Err(_) => return false,
        }
    } else {
        let Some(host) = single_header_value(headers, header::HOST.as_str()) else {
            return false;
        };
        let Ok(expected) = Url::parse(&format!("http://{host}")) else {
            return false;
        };
        expected.origin().ascii_serialization()
    };
    if origin.origin().ascii_serialization() != expected_origin {
        return false;
    }

    match single_header_value(headers, "sec-fetch-site") {
        Some(value) => value.eq_ignore_ascii_case("same-origin"),
        None => true,
    }
}

fn single_header_value<'a>(headers: &'a HeaderMap, name: &'static str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?.trim();
    (!value.is_empty() && values.next().is_none()).then_some(value)
}

const OWNER_SESSION_COOKIE: &str = "fuckipv6_owner_session";

fn owner_session_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|part| {
            let (name, value) = part.trim().split_once('=')?;
            (name == OWNER_SESSION_COOKIE && !value.is_empty()).then_some(value)
        })
}

fn owner_session_cookie_header(token: &str, channel: OwnerBootstrapChannel) -> HeaderValue {
    let secure = matches!(channel, OwnerBootstrapChannel::Public)
        .then_some("; Secure")
        .unwrap_or_default();
    HeaderValue::from_str(&format!(
        "{OWNER_SESSION_COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age=43200{secure}"
    ))
    .expect("owner session token is cookie-safe")
}

fn authorization_bearer_token(request: &axum::extract::Request) -> Option<&str> {
    let value = request
        .headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .trim();
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then_some(token.trim())
        .filter(|token| !token.is_empty())
}

const WEBSOCKET_BEARER_PROTOCOL_PREFIX: &str = "fuckipv6.bearer.";

/// Browsers cannot attach an Authorization header to `WebSocket`. Keep legacy
/// bearer-token login usable without putting the credential in the URL by
/// carrying it in an offered WebSocket subprotocol during the HTTP upgrade.
fn websocket_subprotocol_bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all("sec-websocket-protocol")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .find_map(|protocol| {
            protocol
                .trim()
                .strip_prefix(WEBSOCKET_BEARER_PROTOCOL_PREFIX)
                .filter(|token| !token.is_empty())
        })
}

fn unauthorized_response(message: impl Into<String>) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [
            (header::WWW_AUTHENTICATE, "Bearer realm=\"fuckipv6-webui\""),
            (header::CACHE_CONTROL, "no-store"),
        ],
        Json(ApiError {
            error: message.into(),
            code: None,
            problem: None,
        }),
    )
        .into_response()
}

fn forbidden_response(message: impl Into<String>) -> Response {
    (
        StatusCode::FORBIDDEN,
        [(header::CACHE_CONTROL, "no-store")],
        Json(ApiError {
            error: message.into(),
            code: None,
            problem: None,
        }),
    )
        .into_response()
}

fn mutation_rate_limited_response(retry_after: Duration) -> Response {
    let retry_after_secs = retry_after
        .as_secs()
        .saturating_add(u64::from(retry_after.subsec_nanos() > 0))
        .max(1);
    (
        StatusCode::TOO_MANY_REQUESTS,
        [
            (
                header::RETRY_AFTER,
                HeaderValue::from_str(&retry_after_secs.to_string())
                    .expect("retry-after seconds are a valid header value"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        Json(ApiError {
            error: "控制命令过于频繁，请稍后重试".to_string(),
            code: None,
            problem: None,
        }),
    )
        .into_response()
}

#[derive(Clone)]
struct WebUiSurface {
    owner_security: Option<Arc<OwnerSecurityService>>,
    direct_tls: bool,
}

#[derive(Clone)]
struct SecurityHeadersState {
    direct_tls: bool,
    public_claim: Option<Arc<PublicClaimConfig>>,
}

const CSP_NONCE_PLACEHOLDER: &str = "__CSP_NONCE__";
const HSTS_VALUE: &str = "max-age=31536000; includeSubDomains";
const PRODUCT_CONTENT_SECURITY_POLICY: &str = "default-src 'none'; script-src 'self'; script-src-attr 'none'; style-src 'self'; style-src-attr 'unsafe-inline'; img-src 'self' data:; connect-src 'self' ws: wss:; object-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";

async fn apply_security_headers(
    State(state): State<SecurityHeadersState>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let secured = request_uses_https_transport(&state, &request);
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static("camera=(), microphone=(), geolocation=(), payment=(), usb=()"),
    );
    headers.insert(
        "cross-origin-opener-policy",
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("same-origin"),
    );
    if secured {
        headers.insert(
            "strict-transport-security",
            HeaderValue::from_static(HSTS_VALUE),
        );
    }
    response
}

fn request_uses_https_transport(
    state: &SecurityHeadersState,
    request: &axum::extract::Request,
) -> bool {
    if state.direct_tls {
        return true;
    }
    let Some(public_claim) = &state.public_claim else {
        return false;
    };
    let Some(ConnectInfo(remote)) = request.extensions().get::<ConnectInfo<SocketAddr>>() else {
        return false;
    };
    public_claim
        .authorize(remote.ip(), request.method(), request.headers())
        .is_ok()
}

fn render_product_html(template: &'static str) -> Response {
    let mut response = Html(template).into_response();
    response.headers_mut().insert(
        "content-security-policy",
        HeaderValue::from_static(PRODUCT_CONTENT_SECURITY_POLICY),
    );
    with_no_store(response)
}

fn render_html_surface(template: &'static str) -> Response {
    if !template.contains(CSP_NONCE_PLACEHOLDER) {
        let mut response = Html(template).into_response();
        response.headers_mut().insert(
            "content-security-policy",
            HeaderValue::from_static(PRODUCT_CONTENT_SECURITY_POLICY),
        );
        return with_no_store(response);
    }

    let Some(nonce) = content_security_nonce() else {
        tracing::error!("无法生成 WebUI CSP nonce，页面失败关闭");
        let mut response = (
            StatusCode::INTERNAL_SERVER_ERROR,
            Html("<!doctype html><title>安全错误</title><p>无法生成安全页面。</p>"),
        )
            .into_response();
        response.headers_mut().insert(
            "content-security-policy",
            HeaderValue::from_static(
                "default-src 'none'; style-src 'none'; script-src 'none'; frame-ancestors 'none'; base-uri 'none'",
            ),
        );
        return with_no_store(response);
    };
    let html = template.replace(CSP_NONCE_PLACEHOLDER, &nonce);
    let policy = format!(
        "default-src 'none'; script-src 'self' 'nonce-{nonce}'; script-src-attr 'none'; style-src 'self' 'nonce-{nonce}'; style-src-attr 'unsafe-inline'; img-src 'self' data:; connect-src 'self' ws: wss:; object-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"
    );
    let policy =
        HeaderValue::from_str(&policy).expect("generated WebUI CSP is an ASCII header value");
    let mut response = Html(html).into_response();
    response
        .headers_mut()
        .insert("content-security-policy", policy);
    with_no_store(response)
}

fn content_security_nonce() -> Option<String> {
    let mut bytes = [0_u8; 18];
    getrandom::fill(&mut bytes).ok()?;
    Some(BASE64_STANDARD.encode(bytes))
}

async fn handle_index(Extension(surface): Extension<WebUiSurface>, headers: HeaderMap) -> Response {
    if let Some(owner) = &surface.owner_security {
        let status = match owner.status() {
            Ok(status) => status,
            Err(error) => {
                tracing::error!(%error, "所有者安全状态读取失败，WebUI 已失败关闭");
                let mut response = render_html_surface(owner_bootstrap::OWNER_UNAVAILABLE_HTML);
                *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                return response;
            }
        };
        if !status.initialized {
            if !surface.direct_tls {
                if let Some(redirect) = localhost_redirect(&headers) {
                    return with_no_store(redirect.into_response());
                }
            }
            return render_html_surface(owner_bootstrap::OWNER_BOOTSTRAP_HTML);
        }
    }

    render_product_html(node_surface::NODE_APP_HTML)
}

fn localhost_redirect(headers: &HeaderMap) -> Option<Redirect> {
    let host = headers.get(header::HOST)?.to_str().ok()?;
    let port = host.strip_prefix("127.0.0.1:")?;
    if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(Redirect::temporary(&format!("http://localhost:{port}/")))
}

fn with_no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[derive(Debug, Serialize)]
struct LanguageSettingsResponse {
    preference: LanguagePreference,
    supported_preferences: [LanguagePreference; 3],
    supported_locales: [Locale; 2],
}

impl LanguageSettingsResponse {
    fn new(preference: LanguagePreference) -> Self {
        Self {
            preference,
            supported_preferences: LanguagePreference::ALL,
            supported_locales: Locale::ALL,
        }
    }
}

#[derive(Debug, Deserialize)]
struct LanguageSettingsRequest {
    preference: LanguagePreference,
}

async fn handle_get_language_settings(Extension(config): Extension<ConfigManager>) -> Response {
    match config.node_language_preference() {
        Ok(preference) => Json(LanguageSettingsResponse::new(preference)).into_response(),
        Err(error) => {
            tracing::error!(%error, "读取节点语言偏好失败");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "code": "language_persistence_failed",
                    "message_key": "error.language_persistence_failed",
                })),
            )
                .into_response()
        }
    }
}

async fn handle_put_language_settings(
    Extension(config): Extension<ConfigManager>,
    Json(request): Json<LanguageSettingsRequest>,
) -> Response {
    match config.set_node_language_preference(request.preference) {
        Ok(()) => Json(LanguageSettingsResponse::new(request.preference)).into_response(),
        Err(error) => {
            tracing::error!(%error, "保存节点语言偏好失败");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "code": "language_persistence_failed",
                    "message_key": "error.language_persistence_failed",
                })),
            )
                .into_response()
        }
    }
}

async fn handle_node_app_js() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        node_surface::NODE_APP_JS,
    )
}

async fn handle_node_app_css() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        node_surface::NODE_APP_CSS,
    )
}

async fn handle_owner_bootstrap_js() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        owner_bootstrap::OWNER_BOOTSTRAP_JS,
    )
}

async fn handle_webui_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        fuckipv6_common::ui_components::complete_webui_css(),
    )
}

async fn handle_i18n_js() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        fuckipv6_common::i18n_browser::browser_runtime_script(),
    )
}

#[derive(Clone)]
struct OwnerAccessState {
    owner: Arc<OwnerSecurityService>,
    public_claim: Option<Arc<PublicClaimConfig>>,
}

fn owner_security_router(
    owner: Arc<OwnerSecurityService>,
    public_claim: Option<Arc<PublicClaimConfig>>,
) -> Router {
    let access = OwnerAccessState {
        owner: owner.clone(),
        public_claim,
    };
    Router::new()
        .route("/api/v1/auth/bootstrap", get(handle_bootstrap_status))
        .route(
            "/api/v1/auth/bootstrap/verify",
            post(handle_bootstrap_verify),
        )
        .route(
            "/api/v1/auth/webauthn/registration/start",
            post(handle_webauthn_registration_start),
        )
        .route(
            "/api/v1/auth/webauthn/registration/finish",
            post(handle_webauthn_registration_finish),
        )
        .route(
            "/api/v1/auth/bootstrap/recovery-confirm",
            post(handle_bootstrap_recovery_confirm),
        )
        .route(
            "/api/v1/auth/passkey/authentication/start",
            post(handle_owner_authentication_start),
        )
        .route(
            "/api/v1/auth/passkey/authentication/finish",
            post(handle_owner_authentication_finish),
        )
        .route(
            "/api/v1/auth/passkey/reauth/start",
            post(handle_owner_reauthentication_start),
        )
        .route(
            "/api/v1/auth/passkey/reauth/finish",
            post(handle_owner_reauthentication_finish),
        )
        .route("/api/v1/auth/session", get(handle_owner_session_status))
        .route("/api/v1/auth/passkey/logout", post(handle_owner_logout))
        .route("/api/v1/auth/passkeys", get(handle_owner_passkeys))
        .route(
            "/api/v1/auth/passkeys/registration/start",
            post(handle_owner_passkey_registration_start),
        )
        .route(
            "/api/v1/auth/passkeys/registration/finish",
            post(handle_owner_passkey_registration_finish),
        )
        .route(
            "/api/v1/auth/passkeys/{credential_id}",
            delete(handle_owner_passkey_revoke),
        )
        .with_state(owner)
        .route_layer(middleware::from_fn_with_state(
            access,
            authorize_owner_access,
        ))
        .route_layer(middleware::from_fn(owner_no_store))
}

async fn authorize_owner_access(
    State(access): State<OwnerAccessState>,
    mut request: axum::extract::Request,
    next: Next,
) -> Response {
    let method = request.method().as_str().to_owned();
    let path = request.uri().path().to_owned();
    let connect_ip = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(remote_addr)| remote_addr.ip());
    let has_forwarding = request_has_forwarding_headers(&request);
    let channel_hint = connect_ip.map(|ip| {
        if ip.is_loopback() && !has_forwarding {
            OwnerBootstrapChannel::Local
        } else {
            OwnerBootstrapChannel::Public
        }
    });

    if request.uri().query().is_some() {
        return finish_owner_response(
            &access,
            connect_ip,
            channel_hint,
            &method,
            &path,
            owner_api_error(
                StatusCode::BAD_REQUEST,
                "所有者初始化凭据不得出现在 URL query 中",
            ),
        );
    }

    let Some(remote_ip) = connect_ip else {
        return finish_owner_response(
            &access,
            None,
            None,
            &method,
            &path,
            owner_api_error(StatusCode::FORBIDDEN, "所有者初始化请求缺少可信来源"),
        );
    };
    let (channel, source_ip) = if remote_ip.is_loopback() && !has_forwarding {
        (OwnerBootstrapChannel::Local, remote_ip)
    } else {
        let Some(public_claim) = &access.public_claim else {
            return finish_owner_response(
                &access,
                Some(remote_ip),
                Some(OwnerBootstrapChannel::Public),
                &method,
                &path,
                owner_api_error(StatusCode::FORBIDDEN, "本机初始化仅允许回环访问"),
            );
        };
        let client_ip = match public_claim.authorize(remote_ip, request.method(), request.headers())
        {
            Ok(client_ip) => client_ip,
            Err(rejection) => {
                return finish_owner_response(
                    &access,
                    Some(remote_ip),
                    Some(OwnerBootstrapChannel::Public),
                    &method,
                    &path,
                    owner_api_error(rejection.status(), rejection.message()),
                );
            }
        };
        match access.owner.status_for(OwnerBootstrapChannel::Public) {
            Ok(status) if status.initialized && path.starts_with("/api/v1/auth/bootstrap") => {
                return finish_owner_response(
                    &access,
                    Some(client_ip),
                    Some(OwnerBootstrapChannel::Public),
                    &method,
                    &path,
                    owner_api_error(StatusCode::GONE, "公网认领入口已永久关闭"),
                );
            }
            Ok(_) => {}
            Err(error) => {
                return finish_owner_response(
                    &access,
                    Some(client_ip),
                    Some(OwnerBootstrapChannel::Public),
                    &method,
                    &path,
                    owner_service_error(error),
                );
            }
        }
        (OwnerBootstrapChannel::Public, client_ip)
    };

    request.extensions_mut().insert(channel);
    let response = next.run(request).await;
    finish_owner_response(
        &access,
        Some(source_ip),
        Some(channel),
        &method,
        &path,
        response,
    )
}

fn finish_owner_response(
    access: &OwnerAccessState,
    source_ip: Option<IpAddr>,
    channel: Option<OwnerBootstrapChannel>,
    method: &str,
    path: &str,
    response: Response,
) -> Response {
    let status = response.status();
    match access.owner.record_bootstrap_audit(
        channel,
        source_ip,
        method,
        path,
        Some(status.as_u16()),
        owner_audit_outcome(status),
    ) {
        Ok(()) => response,
        Err(error) => {
            tracing::error!(
                error = %error,
                method,
                path,
                "所有者安全审计写入失败，控制面拒绝继续操作"
            );
            owner_api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "安全审计写入失败，控制面拒绝继续操作",
            )
        }
    }
}

fn owner_audit_outcome(status: StatusCode) -> &'static str {
    if status.is_success() {
        return "success";
    }
    match status {
        StatusCode::BAD_REQUEST => "bad_request",
        StatusCode::UNAUTHORIZED => "invalid_code",
        StatusCode::FORBIDDEN => "forbidden",
        StatusCode::NOT_FOUND => "ceremony_missing",
        StatusCode::CONFLICT => "conflict_or_closed",
        StatusCode::GONE => "permanently_closed",
        StatusCode::TOO_MANY_REQUESTS => "rate_limited",
        status if status.is_server_error() => "internal_error",
        _ => "rejected",
    }
}

fn request_has_forwarding_headers(request: &axum::extract::Request) -> bool {
    [
        "forwarded",
        "x-forwarded-for",
        "x-forwarded-host",
        "x-forwarded-proto",
        "x-real-ip",
    ]
    .iter()
    .any(|name| request.headers().contains_key(*name))
}

async fn owner_no_store(request: axum::extract::Request, next: Next) -> Response {
    with_no_store(next.run(request).await)
}

fn owner_api_error(status: StatusCode, message: impl Into<String>) -> Response {
    owner_api_error_with_code(status, None, message)
}

fn owner_api_error_with_code(
    status: StatusCode,
    code: Option<&'static str>,
    message: impl Into<String>,
) -> Response {
    (
        status,
        [(header::CACHE_CONTROL, "no-store")],
        Json(ApiError {
            error: message.into(),
            code,
            problem: None,
        }),
    )
        .into_response()
}

fn owner_service_error(error: OwnerSecurityError) -> Response {
    let status = match &error {
        OwnerSecurityError::AlreadyInitialized | OwnerSecurityError::CeremonyInProgress => {
            StatusCode::CONFLICT
        }
        OwnerSecurityError::InvalidBootstrapCode => StatusCode::UNAUTHORIZED,
        OwnerSecurityError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        OwnerSecurityError::CeremonyMissing
        | OwnerSecurityError::AuthenticationCeremonyMissing
        | OwnerSecurityError::PasskeyNotFound => StatusCode::NOT_FOUND,
        OwnerSecurityError::InvalidSession => StatusCode::UNAUTHORIZED,
        OwnerSecurityError::RecentAuthenticationRequired => StatusCode::FORBIDDEN,
        OwnerSecurityError::PasskeyAlreadyExists
        | OwnerSecurityError::PasskeyAlreadyRevoked
        | OwnerSecurityError::PasskeyLimitReached
        | OwnerSecurityError::LastPasskey => StatusCode::CONFLICT,
        OwnerSecurityError::InvalidPasskeyLabel => StatusCode::BAD_REQUEST,
        OwnerSecurityError::NotInitialized | OwnerSecurityError::RecoveryNotConfirmed => {
            StatusCode::BAD_REQUEST
        }
        OwnerSecurityError::CorruptState | OwnerSecurityError::Internal(_) => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    let code = match &error {
        OwnerSecurityError::AlreadyInitialized => "already_initialized",
        OwnerSecurityError::InvalidBootstrapCode => "invalid_bootstrap_code",
        OwnerSecurityError::RateLimited => "rate_limited",
        OwnerSecurityError::CeremonyInProgress => "ceremony_in_progress",
        OwnerSecurityError::CeremonyMissing => "registration_ceremony_missing",
        OwnerSecurityError::AuthenticationCeremonyMissing => "authentication_ceremony_missing",
        OwnerSecurityError::NotInitialized => "owner_not_initialized",
        OwnerSecurityError::InvalidSession => "invalid_session",
        OwnerSecurityError::RecentAuthenticationRequired => "recent_authentication_required",
        OwnerSecurityError::PasskeyNotFound => "passkey_not_found",
        OwnerSecurityError::PasskeyAlreadyExists => "passkey_already_exists",
        OwnerSecurityError::PasskeyAlreadyRevoked => "passkey_already_revoked",
        OwnerSecurityError::PasskeyLimitReached => "passkey_limit_reached",
        OwnerSecurityError::InvalidPasskeyLabel => "invalid_passkey_label",
        OwnerSecurityError::LastPasskey => "last_passkey",
        OwnerSecurityError::RecoveryNotConfirmed => "recovery_not_confirmed",
        OwnerSecurityError::CorruptState => "owner_state_corrupt",
        OwnerSecurityError::Internal(_) => "owner_internal",
    };
    owner_api_error_with_code(status, Some(code), error.to_string())
}

#[derive(Deserialize)]
struct BootstrapCodeRequest {
    code: String,
}

impl fmt::Debug for BootstrapCodeRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BootstrapCodeRequest")
            .field("code", &"[REDACTED]")
            .finish()
    }
}

#[derive(Serialize)]
struct BootstrapVerifyResponse {
    verified: bool,
}

#[derive(Deserialize)]
struct WebauthnRegistrationFinishRequest {
    credential: RegisterPublicKeyCredential,
}

#[derive(Serialize)]
struct WebauthnRegistrationFinishResponse {
    recovery_code: String,
}

impl fmt::Debug for WebauthnRegistrationFinishResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WebauthnRegistrationFinishResponse")
            .field("recovery_code", &"[REDACTED]")
            .finish()
    }
}

#[derive(Deserialize)]
struct RecoveryConfirmationRequest {
    confirmed: bool,
}

async fn handle_bootstrap_status(
    State(owner): State<Arc<OwnerSecurityService>>,
    Extension(channel): Extension<OwnerBootstrapChannel>,
) -> Response {
    match owner.status_for(channel) {
        Ok(status) => (StatusCode::OK, Json(status)).into_response(),
        Err(error) => owner_service_error(error),
    }
}

async fn handle_bootstrap_verify(
    State(owner): State<Arc<OwnerSecurityService>>,
    Extension(channel): Extension<OwnerBootstrapChannel>,
    Json(request): Json<BootstrapCodeRequest>,
) -> Response {
    match owner.verify_bootstrap_code_for(channel, &request.code) {
        Ok(()) => (
            StatusCode::OK,
            Json(BootstrapVerifyResponse { verified: true }),
        )
            .into_response(),
        Err(error) => owner_service_error(error),
    }
}

async fn handle_webauthn_registration_start(
    State(owner): State<Arc<OwnerSecurityService>>,
    Extension(channel): Extension<OwnerBootstrapChannel>,
    Json(request): Json<BootstrapCodeRequest>,
) -> Response {
    match owner.start_registration_for(channel, &request.code) {
        Ok(challenge) => (StatusCode::OK, Json(challenge)).into_response(),
        Err(error) => owner_service_error(error),
    }
}

async fn handle_webauthn_registration_finish(
    State(owner): State<Arc<OwnerSecurityService>>,
    Extension(channel): Extension<OwnerBootstrapChannel>,
    Json(request): Json<WebauthnRegistrationFinishRequest>,
) -> Response {
    match owner.finish_registration_for(channel, &request.credential) {
        Ok(recovery_code) => (
            StatusCode::OK,
            Json(WebauthnRegistrationFinishResponse { recovery_code }),
        )
            .into_response(),
        Err(error) => owner_service_error(error),
    }
}

async fn handle_bootstrap_recovery_confirm(
    State(owner): State<Arc<OwnerSecurityService>>,
    Extension(channel): Extension<OwnerBootstrapChannel>,
    Json(request): Json<RecoveryConfirmationRequest>,
) -> Response {
    match owner.confirm_recovery_for(channel, request.confirmed) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => owner_service_error(error),
    }
}

#[derive(Deserialize)]
struct WebauthnAuthenticationFinishRequest {
    credential: PublicKeyCredential,
}

#[derive(Deserialize)]
struct PasskeyRegistrationStartRequest {
    label: String,
}

#[derive(Serialize)]
struct OwnerSessionStatusResponse {
    authenticated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    credential_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    channel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at_unix_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_authenticated_at_unix_ms: Option<u64>,
}

#[derive(Serialize)]
struct PasskeyListResponse {
    passkeys: Vec<fuckipv6_common::domain::PasskeyCredentialRef>,
}

#[derive(Serialize)]
struct NewPasskeyResponse {
    passkey: fuckipv6_common::domain::PasskeyCredentialRef,
}

#[derive(Serialize)]
struct RevokedPasskeyResponse {
    passkey: fuckipv6_common::domain::PasskeyCredentialRef,
    current_session_revoked: bool,
}

fn owner_session_status(
    session: Option<&AuthenticatedOwnerSession>,
    expected_origin: Option<&str>,
) -> OwnerSessionStatusResponse {
    match session {
        Some(session) => OwnerSessionStatusResponse {
            authenticated: true,
            expected_origin: expected_origin.map(ToString::to_string),
            credential_id: Some(session.credential_id.as_str().to_string()),
            channel: Some(session.channel.as_str().to_string()),
            expires_at_unix_ms: Some(session.expires_at_unix_ms),
            last_authenticated_at_unix_ms: Some(session.last_authenticated_at_unix_ms),
        },
        None => OwnerSessionStatusResponse {
            authenticated: false,
            expected_origin: expected_origin.map(ToString::to_string),
            credential_id: None,
            channel: None,
            expires_at_unix_ms: None,
            last_authenticated_at_unix_ms: None,
        },
    }
}

fn issued_owner_session_response(
    channel: OwnerBootstrapChannel,
    issued: IssuedOwnerSession,
    expected_origin: &str,
) -> Response {
    let cookie = owner_session_cookie_header(issued.token.as_str(), channel);
    let body = owner_session_status(Some(&issued.session), Some(expected_origin));
    let mut response = (StatusCode::OK, Json(body)).into_response();
    response.headers_mut().insert(header::SET_COOKIE, cookie);
    response
}

fn clear_owner_session_cookie(response: &mut Response, channel: OwnerBootstrapChannel) {
    let secure = matches!(channel, OwnerBootstrapChannel::Public)
        .then_some("; Secure")
        .unwrap_or_default();
    let cookie = HeaderValue::from_str(&format!(
        "{OWNER_SESSION_COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0{secure}"
    ))
    .expect("owner session clear cookie is valid");
    response.headers_mut().insert(header::SET_COOKIE, cookie);
}

async fn handle_owner_authentication_start(
    State(owner): State<Arc<OwnerSecurityService>>,
    Extension(channel): Extension<OwnerBootstrapChannel>,
) -> Response {
    match owner.start_authentication(channel) {
        Ok(challenge) => (StatusCode::OK, Json(challenge)).into_response(),
        Err(error) => owner_service_error(error),
    }
}

async fn handle_owner_authentication_finish(
    State(owner): State<Arc<OwnerSecurityService>>,
    Extension(channel): Extension<OwnerBootstrapChannel>,
    Json(request): Json<WebauthnAuthenticationFinishRequest>,
) -> Response {
    let expected_origin = match owner.origin_for(channel) {
        Ok(origin) => origin.to_string(),
        Err(error) => return owner_service_error(error),
    };
    match owner.finish_authentication(channel, &request.credential) {
        Ok(issued) => issued_owner_session_response(channel, issued, &expected_origin),
        Err(error) => owner_service_error(error),
    }
}

async fn handle_owner_reauthentication_start(
    State(owner): State<Arc<OwnerSecurityService>>,
    Extension(channel): Extension<OwnerBootstrapChannel>,
    headers: HeaderMap,
) -> Response {
    let Some(token) = owner_session_cookie(&headers) else {
        return owner_api_error(StatusCode::UNAUTHORIZED, "所有者会话缺失或已过期");
    };
    match owner.start_reauthentication(channel, token) {
        Ok(challenge) => (StatusCode::OK, Json(challenge)).into_response(),
        Err(error) => owner_service_error(error),
    }
}

async fn handle_owner_reauthentication_finish(
    State(owner): State<Arc<OwnerSecurityService>>,
    Extension(channel): Extension<OwnerBootstrapChannel>,
    headers: HeaderMap,
    Json(request): Json<WebauthnAuthenticationFinishRequest>,
) -> Response {
    let Some(token) = owner_session_cookie(&headers) else {
        return owner_api_error(StatusCode::UNAUTHORIZED, "所有者会话缺失或已过期");
    };
    if let Err(error) = owner.authenticate_session(channel, token) {
        return owner_service_error(error);
    }
    let expected_origin = match owner.origin_for(channel) {
        Ok(origin) => origin.to_string(),
        Err(error) => return owner_service_error(error),
    };
    match owner.finish_authentication(channel, &request.credential) {
        Ok(issued) => issued_owner_session_response(channel, issued, &expected_origin),
        Err(error) => owner_service_error(error),
    }
}

async fn handle_owner_session_status(
    State(owner): State<Arc<OwnerSecurityService>>,
    Extension(channel): Extension<OwnerBootstrapChannel>,
    headers: HeaderMap,
) -> Response {
    let session = owner_session_cookie(&headers)
        .and_then(|token| owner.authenticate_session(channel, token).ok());
    let expected_origin = match owner.origin_for(channel) {
        Ok(origin) => Some(origin),
        Err(error) => return owner_service_error(error),
    };
    (
        StatusCode::OK,
        Json(owner_session_status(session.as_ref(), expected_origin)),
    )
        .into_response()
}

async fn handle_owner_logout(
    State(owner): State<Arc<OwnerSecurityService>>,
    Extension(channel): Extension<OwnerBootstrapChannel>,
    headers: HeaderMap,
) -> Response {
    let result = owner_session_cookie(&headers)
        .map(|token| owner.logout(channel, token))
        .transpose();
    match result {
        Ok(_) => {
            let mut response = StatusCode::NO_CONTENT.into_response();
            clear_owner_session_cookie(&mut response, channel);
            response
        }
        Err(error) => owner_service_error(error),
    }
}

async fn handle_owner_passkeys(
    State(owner): State<Arc<OwnerSecurityService>>,
    Extension(channel): Extension<OwnerBootstrapChannel>,
    headers: HeaderMap,
) -> Response {
    let Some(token) = owner_session_cookie(&headers) else {
        return owner_api_error(StatusCode::UNAUTHORIZED, "所有者会话缺失或已过期");
    };
    match owner.list_passkeys(channel, token) {
        Ok(passkeys) => (StatusCode::OK, Json(PasskeyListResponse { passkeys })).into_response(),
        Err(error) => owner_service_error(error),
    }
}

async fn handle_owner_passkey_registration_start(
    State(owner): State<Arc<OwnerSecurityService>>,
    Extension(channel): Extension<OwnerBootstrapChannel>,
    headers: HeaderMap,
    Json(request): Json<PasskeyRegistrationStartRequest>,
) -> Response {
    let Some(token) = owner_session_cookie(&headers) else {
        return owner_api_error(StatusCode::UNAUTHORIZED, "所有者会话缺失或已过期");
    };
    match owner.start_passkey_registration(channel, token, &request.label) {
        Ok(challenge) => (StatusCode::OK, Json(challenge)).into_response(),
        Err(error) => owner_service_error(error),
    }
}

async fn handle_owner_passkey_registration_finish(
    State(owner): State<Arc<OwnerSecurityService>>,
    Extension(channel): Extension<OwnerBootstrapChannel>,
    headers: HeaderMap,
    Json(request): Json<WebauthnRegistrationFinishRequest>,
) -> Response {
    let Some(token) = owner_session_cookie(&headers) else {
        return owner_api_error(StatusCode::UNAUTHORIZED, "所有者会话缺失或已过期");
    };
    match owner.finish_passkey_registration(channel, token, &request.credential) {
        Ok(issued) => (
            StatusCode::CREATED,
            Json(NewPasskeyResponse {
                passkey: issued.passkey,
            }),
        )
            .into_response(),
        Err(error) => owner_service_error(error),
    }
}

async fn handle_owner_passkey_revoke(
    State(owner): State<Arc<OwnerSecurityService>>,
    Extension(channel): Extension<OwnerBootstrapChannel>,
    AxumPath(credential_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    let credential_id = match PasskeyCredentialId::parse(credential_id) {
        Ok(credential_id) => credential_id,
        Err(_) => return owner_api_error(StatusCode::BAD_REQUEST, "Passkey 凭据 ID 无效"),
    };
    let Some(token) = owner_session_cookie(&headers) else {
        return owner_api_error(StatusCode::UNAUTHORIZED, "所有者会话缺失或已过期");
    };
    match owner.revoke_passkey(channel, token, &credential_id) {
        Ok((passkey, current_session_revoked)) => {
            let mut response = (
                StatusCode::OK,
                Json(RevokedPasskeyResponse {
                    passkey,
                    current_session_revoked,
                }),
            )
                .into_response();
            if current_session_revoked {
                clear_owner_session_cookie(&mut response, channel);
            }
            response
        }
        Err(error) => owner_service_error(error),
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    problem: Option<Box<NodeProblem>>,
}

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;

fn api_error(status: StatusCode, message: impl Into<String>) -> (StatusCode, Json<ApiError>) {
    api_error_with_problem(status, message, None)
}

fn api_error_with_problem(
    status: StatusCode,
    message: impl Into<String>,
    problem: Option<NodeProblem>,
) -> (StatusCode, Json<ApiError>) {
    (
        status,
        Json(ApiError {
            error: message.into(),
            code: None,
            problem: problem.map(Box::new),
        }),
    )
}

fn controller_problem(error: &anyhow::Error) -> Option<NodeProblem> {
    error
        .downcast_ref::<AppControllerError>()
        .map(|error| error.problem().clone())
}

fn api_error_from_controller(
    message: impl Into<String>,
    error: &anyhow::Error,
) -> (StatusCode, Json<ApiError>) {
    let problem = controller_problem(error);
    let status = problem
        .as_ref()
        .map(|problem| status_for_node_problem(problem.code))
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    api_error_with_problem(status, message, problem)
}

fn api_error_from_app_controller(
    message: impl Into<String>,
    error: &AppControllerError,
) -> (StatusCode, Json<ApiError>) {
    let problem = error.problem().clone();
    api_error_with_problem(
        status_for_node_problem(problem.code),
        message,
        Some(problem),
    )
}

fn status_for_node_problem(code: NodeProblemCode) -> StatusCode {
    match code {
        NodeProblemCode::AuthenticationRequired => StatusCode::UNAUTHORIZED,
        NodeProblemCode::InvalidRequest => StatusCode::BAD_REQUEST,
        NodeProblemCode::NotFound => StatusCode::NOT_FOUND,
        NodeProblemCode::Conflict | NodeProblemCode::Cancelled => StatusCode::CONFLICT,
        NodeProblemCode::Unsupported => StatusCode::UNPROCESSABLE_ENTITY,
        NodeProblemCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        NodeProblemCode::PermissionDenied => StatusCode::FORBIDDEN,
        NodeProblemCode::TimedOut => StatusCode::GATEWAY_TIMEOUT,
        NodeProblemCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

async fn handle_peer_probe(
    State(ctrl): State<Arc<AppController>>,
    Json(payload): Json<ProbePeerRequest>,
) -> Result<Json<crate::p2p::connection::PeerProbeOutcome>, (StatusCode, Json<ApiError>)> {
    ctrl.probe_peer(payload.target, payload.timeout_ms)
        .await
        .map(Json)
        .map_err(|error| {
            if controller_problem(&error).is_some() {
                api_error_from_controller(format!("Peer 探测失败: {error:#}"), &error)
            } else {
                api_error(StatusCode::BAD_GATEWAY, format!("Peer 探测失败: {error:#}"))
            }
        })
}

async fn handle_status(
    State(ctrl): State<Arc<AppController>>,
    Extension(auth): Extension<Arc<WebUiAuth>>,
) -> Json<serde_json::Value> {
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
        "auth_required": auth.is_required(),
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

async fn handle_get_config(State(ctrl): State<Arc<AppController>>) -> ApiResult<NetworkConfigDto> {
    ctrl.get_network_config()
        .await
        .map(Json)
        .map_err(|error| api_error_from_app_controller("读取网段配置失败", &error))
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
            status_for_node_problem(e.problem().code),
            Json(NetworkConfigApiResponse {
                success: false,
                message: format!("更新网段失败: {e:#}"),
                data: None,
            }),
        ),
    }
}

async fn handle_get_blacklist(State(ctrl): State<Arc<AppController>>) -> ApiResult<Vec<String>> {
    ctrl.get_blacklist()
        .await
        .map(Json)
        .map_err(|error| api_error_from_app_controller("读取黑名单失败", &error))
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

async fn handle_blacklist_ban(
    State(ctrl): State<Arc<AppController>>,
    Json(payload): Json<BlacklistActionRequest>,
) -> BlacklistActionHttpResult {
    let client_id = payload.client_id.trim().to_string();
    let previous = ctrl.get_blacklist().await.unwrap_or_default();
    if client_id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            blacklist_response(false, "客户端标识不能为空", previous),
        ));
    }

    match ctrl.ban_client(client_id.clone()).await {
        Ok(snapshot) => {
            let message = if snapshot.changed {
                format!("客户端 {client_id} 已封禁加入黑名单")
            } else {
                format!("客户端 {client_id} 已在黑名单中，无需重复封禁")
            };
            Ok(blacklist_response(true, message, snapshot.clients))
        }
        Err(error) => {
            let message = format!("封禁失败，运行期状态未修改: {error}");
            ctrl.log(message.clone()).await;
            Err((
                status_for_node_problem(error.problem().code),
                blacklist_response(
                    false,
                    message,
                    ctrl.get_blacklist().await.unwrap_or(previous),
                ),
            ))
        }
    }
}

async fn handle_blacklist_unban(
    State(ctrl): State<Arc<AppController>>,
    Json(payload): Json<BlacklistActionRequest>,
) -> BlacklistActionHttpResult {
    let client_id = payload.client_id.trim().to_string();
    let previous = ctrl.get_blacklist().await.unwrap_or_default();
    if client_id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            blacklist_response(false, "客户端标识不能为空", previous),
        ));
    }

    match ctrl.unban_client(&client_id).await {
        Ok(snapshot) => {
            let message = if snapshot.changed {
                format!("客户端 {client_id} 已解除封禁移出黑名单")
            } else {
                format!("客户端 {client_id} 不在黑名单中，无需解封")
            };
            Ok(blacklist_response(true, message, snapshot.clients))
        }
        Err(error) => {
            let message = format!("解封失败，运行期状态未修改: {error}");
            ctrl.log(message.clone()).await;
            Err((
                status_for_node_problem(error.problem().code),
                blacklist_response(
                    false,
                    message,
                    ctrl.get_blacklist().await.unwrap_or(previous),
                ),
            ))
        }
    }
}

#[derive(Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

impl fmt::Debug for LoginRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoginRequest")
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

#[derive(Serialize)]
struct LoginResponse {
    success: bool,
    token: Option<String>,
    error: Option<String>,
    expires_in: Option<u64>,
    auth_required: bool,
}

impl fmt::Debug for LoginResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoginResponse")
            .field("success", &self.success)
            .field("token", &self.token.as_ref().map(|_| "[REDACTED]"))
            .field("error", &self.error)
            .field("expires_in", &self.expires_in)
            .field("auth_required", &self.auth_required)
            .finish()
    }
}

async fn handle_login(
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    Extension(auth): Extension<Arc<WebUiAuth>>,
    Json(req): Json<LoginRequest>,
) -> Response {
    match auth.login_from(remote_addr.ip(), &req.username, &req.password) {
        Ok(token) => (
            StatusCode::OK,
            [(header::CACHE_CONTROL, "no-store")],
            Json(LoginResponse {
                success: true,
                token: Some(token),
                error: None,
                expires_in: Some(auth.expiration_secs()),
                auth_required: auth.is_required(),
            }),
        )
            .into_response(),
        Err(WebUiLoginError::InvalidCredentials) => (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer realm=\"fuckipv6-webui\"")],
            Json(LoginResponse {
                success: false,
                token: None,
                error: Some("用户名或密码错误".to_string()),
                expires_in: None,
                auth_required: auth.is_required(),
            }),
        )
            .into_response(),
        Err(WebUiLoginError::RateLimited { retry_after }) => {
            let retry_after_secs = retry_after
                .as_secs()
                .saturating_add(u64::from(retry_after.subsec_nanos() > 0))
                .max(1);
            (
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, retry_after_secs.to_string())],
                Json(LoginResponse {
                    success: false,
                    token: None,
                    error: Some("登录尝试过于频繁，请稍后重试".to_string()),
                    expires_in: None,
                    auth_required: auth.is_required(),
                }),
            )
                .into_response()
        }
        Err(WebUiLoginError::Internal) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(LoginResponse {
                success: false,
                token: None,
                error: Some("认证服务内部错误".to_string()),
                expires_in: None,
                auth_required: auth.is_required(),
            }),
        )
            .into_response(),
    }
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
    Extension(auth): Extension<Arc<WebUiAuth>>,
) -> impl IntoResponse {
    ws.protocols(["fuckipv6"])
        .on_upgrade(move |socket| ws_stream(socket, ctrl, auth))
}

async fn ws_stream(mut socket: WebSocket, ctrl: Arc<AppController>, auth: Arc<WebUiAuth>) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(2));
    loop {
        ticker.tick().await;
        let snapshot = ctrl.get_unified_snapshot().await;
        let Ok(mut payload) = serde_json::to_value(snapshot) else {
            continue;
        };
        if let Some(object) = payload.as_object_mut() {
            object.insert(
                "auth_required".to_string(),
                serde_json::Value::Bool(auth.is_required()),
            );
        }
        let Ok(payload) = serde_json::to_string(&payload) else {
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
            Err(api_error_from_controller(message, &e))
        }
    }
}

async fn handle_disconnect(State(ctrl): State<Arc<AppController>>) -> ApiResult<AppUiState> {
    match ctrl.disconnect().await {
        Ok(_) => Ok(Json(ctrl.get_state().await)),
        Err(e) => {
            let message = format!("断开网络失败: {e:#}");
            ctrl.log(message.clone()).await;
            Err(api_error_from_controller(message, &e))
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
            Err(api_error_from_controller(message, &e))
        }
    }
}

async fn handle_stop_host(State(ctrl): State<Arc<AppController>>) -> ApiResult<AppUiState> {
    match ctrl.stop_host().await {
        Ok(_) => Ok(Json(ctrl.get_state().await)),
        Err(e) => {
            let message = format!("停止自建网络失败: {e:#}");
            ctrl.log(message.clone()).await;
            Err(api_error_from_controller(message, &e))
        }
    }
}

async fn handle_coordinator_ban(
    State(ctrl): State<Arc<AppController>>,
    Json(payload): Json<BanRequest>,
) -> ApiResult<AppUiState> {
    let client_id = payload.client_id.trim().to_string();
    if let Err(error) = ctrl.ban_client(client_id.clone()).await {
        let message = format!("封禁客户端失败: {error}");
        ctrl.log(message.clone()).await;
        return Err(api_error_from_app_controller(message, &error));
    }

    Ok(Json(ctrl.get_state().await))
}

async fn handle_coordinator_unban(
    State(ctrl): State<Arc<AppController>>,
    Json(payload): Json<BanRequest>,
) -> ApiResult<AppUiState> {
    let client_id = payload.client_id.trim().to_string();
    if let Err(error) = ctrl.unban_client(&client_id).await {
        let message = format!("解封客户端失败: {error}");
        ctrl.log(message.clone()).await;
        return Err(api_error_from_app_controller(message, &error));
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
            Err(api_error_from_controller(message, &e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{
        node_ui_manifest, NodeCommand, NodeCommandBus, NodeCommandFuture, NodeCommandHandler,
        NodeCommandOutcome, NodeCommandRequest, NodeCommandResult, NodeEventBus, NodeHttpAdapter,
        NodeStateStore, NodeUiManifest,
    };
    use crate::owner_security::store::fixture_passkey;
    use axum::body::{to_bytes, Body};
    use axum::extract::ConnectInfo;
    use axum::http::{Request, StatusCode};
    use fuckipv6_common::CoordinatorAuthConfig;
    use std::net::{IpAddr, Ipv4Addr};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tower::ServiceExt;

    static TEST_DIR_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    struct OwnerAppFixture {
        app: Router,
        service: Arc<OwnerSecurityService>,
        code: zeroize::Zeroizing<String>,
        config_dir: PathBuf,
    }

    impl Drop for OwnerAppFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.config_dir);
        }
    }

    fn unique_test_dir(prefix: &str) -> PathBuf {
        let sequence = TEST_DIR_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "fuckipv6-webui-{prefix}-{}-{nanos}-{sequence}",
            std::process::id()
        ))
    }

    fn owner_app_fixture(port: u16) -> OwnerAppFixture {
        owner_node_app_fixture(port, None)
    }

    fn initialize_owner(fixture: &OwnerAppFixture) {
        fixture
            .service
            .start_registration(fixture.code.as_str())
            .expect("owner registration start");
        fixture
            .service
            .finish_registration_for_test(fixture_passkey())
            .expect("owner registration finish");
        fixture
            .service
            .confirm_recovery(true)
            .expect("owner recovery confirmation");
    }

    fn initialize_owner_and_issue_local_session(
        fixture: &OwnerAppFixture,
    ) -> (String, IssuedOwnerSession) {
        initialize_owner(fixture);
        let issued = fixture
            .service
            .issue_session_for_test(OwnerBootstrapChannel::Local)
            .expect("owner test session");
        let cookie =
            owner_session_cookie_header(issued.token.as_str(), OwnerBootstrapChannel::Local)
                .to_str()
                .expect("owner session cookie")
                .to_string();
        (cookie, issued)
    }

    fn initialize_owner_and_issue_local_cookie(fixture: &OwnerAppFixture) -> String {
        initialize_owner_and_issue_local_session(fixture).0
    }

    fn owner_node_app_fixture(port: u16, node_adapter: Option<NodeHttpAdapter>) -> OwnerAppFixture {
        let config_dir = unique_test_dir("owner");
        let config = ConfigManager::new_for_dir(config_dir.clone()).expect("owner test config");
        let (service, code) =
            OwnerSecurityService::from_config(&config, port).expect("owner service");
        let service = Arc::new(service);
        let app = create_app_web_router_with_auth_node_and_owner(
            Arc::new(AppController::new()),
            WebUiAuth::disabled(),
            node_adapter,
            Some(service.clone()),
        );
        OwnerAppFixture {
            app,
            service,
            code: code.expect("uninitialized owner bootstrap code"),
            config_dir,
        }
    }

    fn public_owner_app_fixture(port: u16) -> OwnerAppFixture {
        let config_dir = unique_test_dir("public-owner");
        let config = ConfigManager::new_for_dir(config_dir.clone()).expect("owner test config");
        let public_claim = Arc::new(
            PublicClaimConfig::new("https://relay.xlei.cc", "127.0.0.1/32,::1/128")
                .expect("public claim config"),
        );
        let (service, code) = OwnerSecurityService::from_config_with_public_origin(
            &config,
            port,
            Some(public_claim.origin()),
        )
        .expect("public owner service");
        let service = Arc::new(service);
        let app = create_app_web_router_with_auth_node_owner_and_public_claim(
            Arc::new(AppController::new()),
            WebUiAuth::disabled(),
            None,
            Some(service.clone()),
            Some(public_claim),
        );
        OwnerAppFixture {
            app,
            service,
            code: code.expect("uninitialized owner bootstrap code"),
            config_dir,
        }
    }

    fn direct_tls_public_owner_app_fixture(port: u16) -> OwnerAppFixture {
        let config_dir = unique_test_dir("direct-tls-owner");
        let config = ConfigManager::new_for_dir(config_dir.clone()).expect("owner test config");
        let public_claim = Arc::new(
            PublicClaimConfig::new_direct_tls("https://relay.xlei.cc")
                .expect("direct TLS claim config"),
        );
        let (service, code) = OwnerSecurityService::from_config_with_public_origin(
            &config,
            port,
            Some(public_claim.origin()),
        )
        .expect("direct TLS owner service");
        let service = Arc::new(service);
        let app = create_app_web_router_with_auth_node_owner_and_public_claim(
            Arc::new(AppController::new()),
            WebUiAuth::disabled(),
            None,
            Some(service.clone()),
            Some(public_claim),
        );
        OwnerAppFixture {
            app,
            service,
            code: code.expect("uninitialized owner bootstrap code"),
            config_dir,
        }
    }

    fn request_from(mut request: Request<Body>, source_ip: IpAddr) -> Request<Body> {
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::new(source_ip, 49_152)));
        request
    }

    fn verified_public_request(method: &str, uri: &str, body: &str) -> Request<Body> {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::HOST, "relay.xlei.cc")
            .header("x-forwarded-proto", "https")
            .header("x-forwarded-host", "relay.xlei.cc")
            .header("x-forwarded-for", "203.0.113.9");
        if method != "GET" && method != "HEAD" && method != "OPTIONS" {
            builder = builder
                .header(header::ORIGIN, "https://relay.xlei.cc")
                .header("sec-fetch-site", "same-origin")
                .header(header::CONTENT_TYPE, "application/json");
        }
        let request = builder
            .body(Body::from(body.to_string()))
            .expect("verified public request");
        request_from(request, IpAddr::V4(Ipv4Addr::LOCALHOST))
    }

    fn direct_tls_public_request(method: &str, uri: &str, body: &str) -> Request<Body> {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::HOST, "relay.xlei.cc");
        if method != "GET" && method != "HEAD" && method != "OPTIONS" {
            builder = builder
                .header(header::ORIGIN, "https://relay.xlei.cc")
                .header("sec-fetch-site", "same-origin")
                .header(header::CONTENT_TYPE, "application/json");
        }
        let request = builder
            .body(Body::from(body.to_string()))
            .expect("direct TLS public request");
        request_from(request, "203.0.113.9".parse().unwrap())
    }

    #[derive(Clone)]
    struct NodeRouteHandler {
        state: NodeStateStore,
        manifest: NodeUiManifest,
        command_count: Arc<AtomicUsize>,
    }

    impl NodeCommandHandler for NodeRouteHandler {
        fn handle(&self, request: NodeCommandRequest) -> NodeCommandFuture {
            let state = self.state.clone();
            let manifest = self.manifest.clone();
            let command_count = self.command_count.clone();
            Box::pin(async move {
                command_count.fetch_add(1, Ordering::SeqCst);
                let request_id = request.request_id;
                match request.command {
                    NodeCommand::GetSnapshot => {
                        NodeCommandResult::snapshot(request_id, state.snapshot().await)
                    }
                    NodeCommand::GetUiManifest => {
                        NodeCommandResult::ui_manifest(request_id, manifest)
                    }
                    _ => NodeCommandResult::completed(
                        request_id,
                        NodeCommandOutcome::Completed {
                            message: "handled by shared command bus".into(),
                        },
                    ),
                }
            })
        }
    }

    fn node_adapter_fixture(manifest: NodeUiManifest) -> (NodeHttpAdapter, Arc<AtomicUsize>) {
        let events = NodeEventBus::new(32);
        let state = NodeStateStore::with_events(events.clone());
        let command_count = Arc::new(AtomicUsize::new(0));
        let commands = NodeCommandBus::spawn(
            Arc::new(NodeRouteHandler {
                state: state.clone(),
                manifest,
                command_count: command_count.clone(),
            }),
            events.clone(),
            8,
        );
        (NodeHttpAdapter::new(commands, state, events), command_count)
    }

    fn test_auth() -> WebUiAuth {
        WebUiAuth::new_for_test(CoordinatorAuthConfig {
            enabled: true,
            admin_username: "admin".to_string(),
            admin_password: Some("secret".to_string()),
            jwt_secret: Some("test-webui-integration-secret".to_string()),
            jwt_expiration_secs: 3600,
        })
        .expect("test WebUI auth")
    }

    fn json_request(method: &str, uri: &str, body: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .expect("JSON request")
    }

    fn bearer_request(method: &str, uri: &str, token: &str, body: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(body.to_string()))
            .expect("authorized request")
    }

    fn local_browser_mutation_request(
        uri: &str,
        body: &str,
        cookie: Option<&str>,
        origin: Option<&str>,
        sec_fetch_site: Option<&str>,
    ) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::HOST, "localhost:8080")
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(cookie) = cookie {
            builder = builder.header(header::COOKIE, cookie);
        }
        if let Some(origin) = origin {
            builder = builder.header(header::ORIGIN, origin);
        }
        if let Some(sec_fetch_site) = sec_fetch_site {
            builder = builder.header("sec-fetch-site", sec_fetch_site);
        }
        builder
            .body(Body::from(body.to_string()))
            .expect("local browser mutation request")
    }

    async fn response_text(response: axum::response::Response) -> String {
        let bytes = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("response body");
        String::from_utf8(bytes.to_vec()).expect("UTF-8 response")
    }

    fn login_request(body: &str, source_ip: IpAddr) -> Request<Body> {
        let mut request = json_request("POST", "/api/auth/login", body);
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::new(source_ip, 49_152)));
        request
    }

    async fn response_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("response body");
        serde_json::from_slice(&bytes).expect("JSON response")
    }

    fn read_audit_records(service: &OwnerSecurityService) -> (String, Vec<serde_json::Value>) {
        let content = std::fs::read_to_string(service.audit_path()).expect("owner audit JSONL");
        let records = content
            .lines()
            .map(|line| serde_json::from_str(line).expect("owner audit record"))
            .collect();
        (content, records)
    }

    fn assert_no_store(response: &axum::response::Response) {
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
    }

    fn response_header<'a>(response: &'a axum::response::Response, name: &str) -> &'a str {
        response
            .headers()
            .get(name)
            .unwrap_or_else(|| panic!("missing response header {name}"))
            .to_str()
            .unwrap_or_else(|_| panic!("response header {name} is not visible ASCII"))
    }

    fn csp_nonce(response: &axum::response::Response) -> String {
        let policy = response_header(response, "content-security-policy");
        let marker = "'nonce-";
        let start = policy
            .find(marker)
            .unwrap_or_else(|| panic!("CSP does not contain a nonce: {policy}"))
            + marker.len();
        let end = policy[start..]
            .find('\'')
            .map(|offset| start + offset)
            .unwrap_or_else(|| panic!("CSP nonce is not terminated: {policy}"));
        let nonce = &policy[start..end];
        assert!(
            nonce.len() >= 24,
            "CSP nonce is unexpectedly short: {nonce}"
        );
        assert!(
            policy.contains(&format!("script-src 'self' {marker}{nonce}'")),
            "script-src and style-src must share one strict nonce: {policy}"
        );
        assert!(
            policy.contains(&format!("style-src 'self' {marker}{nonce}'")),
            "script-src and style-src must share one strict nonce: {policy}"
        );
        nonce.to_string()
    }

    async fn response_html_with_nonce(
        response: axum::response::Response,
        template: &str,
    ) -> String {
        let nonce = csp_nonce(&response);
        let body = response_text(response).await;
        assert!(!body.contains(CSP_NONCE_PLACEHOLDER));
        assert!(
            body.contains(&format!(r#"nonce="{nonce}""#)),
            "response HTML does not consume the CSP nonce"
        );
        assert_eq!(body, template.replace(CSP_NONCE_PLACEHOLDER, &nonce));
        body
    }

    fn assert_security_headers(response: &axum::response::Response, hsts: bool) {
        assert_eq!(
            response_header(response, "x-content-type-options"),
            "nosniff"
        );
        assert_eq!(response_header(response, "x-frame-options"), "DENY");
        assert_eq!(response_header(response, "referrer-policy"), "no-referrer");
        assert_eq!(
            response_header(response, "permissions-policy"),
            "camera=(), microphone=(), geolocation=(), payment=(), usb=()"
        );
        assert_eq!(
            response_header(response, "cross-origin-opener-policy"),
            "same-origin"
        );
        assert_eq!(
            response_header(response, "cross-origin-resource-policy"),
            "same-origin"
        );
        if hsts {
            assert_eq!(
                response_header(response, "strict-transport-security"),
                HSTS_VALUE
            );
        } else {
            assert!(
                response
                    .headers()
                    .get("strict-transport-security")
                    .is_none(),
                "HSTS must not be emitted outside a verified HTTPS boundary"
            );
        }
    }

    #[tokio::test]
    async fn owner_bootstrap_api_is_loopback_only_and_always_no_store() {
        let fixture = owner_app_fixture(8080);

        let missing_connect_info = fixture
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/auth/bootstrap")
                    .body(Body::empty())
                    .expect("bootstrap request without connect info"),
            )
            .await
            .expect("bootstrap response without connect info");
        assert_eq!(missing_connect_info.status(), StatusCode::FORBIDDEN);
        assert_no_store(&missing_connect_info);

        let remote = fixture
            .app
            .clone()
            .oneshot(request_from(
                Request::builder()
                    .uri("/api/v1/auth/bootstrap")
                    .body(Body::empty())
                    .expect("remote bootstrap request"),
                IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)),
            ))
            .await
            .expect("remote bootstrap response");
        assert_eq!(remote.status(), StatusCode::FORBIDDEN);
        assert_no_store(&remote);

        let forwarded = fixture
            .app
            .clone()
            .oneshot(request_from(
                Request::builder()
                    .uri("/api/v1/auth/bootstrap")
                    .header("x-forwarded-for", "203.0.113.5")
                    .body(Body::empty())
                    .expect("forwarded bootstrap request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("forwarded bootstrap response");
        assert_eq!(forwarded.status(), StatusCode::FORBIDDEN);
        assert_no_store(&forwarded);

        let loopback = fixture
            .app
            .clone()
            .oneshot(request_from(
                Request::builder()
                    .uri("/api/v1/auth/bootstrap")
                    .body(Body::empty())
                    .expect("loopback bootstrap request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("loopback bootstrap response");
        assert_eq!(loopback.status(), StatusCode::OK);
        assert_no_store(&loopback);
        let payload = response_json(loopback).await;
        assert_eq!(payload["initialized"], false);
        assert_eq!(payload["origin"], "http://localhost:8080");

        let legacy =
            create_app_web_router_with_auth(Arc::new(AppController::new()), WebUiAuth::disabled());
        let legacy_owner_route = legacy
            .oneshot(
                Request::builder()
                    .uri("/api/v1/auth/bootstrap")
                    .body(Body::empty())
                    .expect("legacy owner API request"),
            )
            .await
            .expect("legacy owner API response");
        assert_eq!(legacy_owner_route.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn direct_tls_public_owner_bootstrap_works_but_regular_api_fails_closed() {
        let fixture = direct_tls_public_owner_app_fixture(8080);

        let page = fixture
            .app
            .clone()
            .oneshot(direct_tls_public_request("GET", "/", ""))
            .await
            .expect("direct TLS bootstrap page");
        assert_eq!(page.status(), StatusCode::OK);
        assert_no_store(&page);
        response_html_with_nonce(page, owner_bootstrap::OWNER_BOOTSTRAP_HTML).await;

        let status = fixture
            .app
            .clone()
            .oneshot(direct_tls_public_request(
                "GET",
                "/api/v1/auth/bootstrap",
                "",
            ))
            .await
            .expect("direct TLS bootstrap status");
        assert_eq!(status.status(), StatusCode::OK);
        assert_no_store(&status);
        let payload = response_json(status).await;
        assert_eq!(payload["initialized"], false);
        assert_eq!(payload["origin"], "https://relay.xlei.cc");

        let verify = fixture
            .app
            .clone()
            .oneshot(direct_tls_public_request(
                "POST",
                "/api/v1/auth/bootstrap/verify",
                &serde_json::json!({ "code": fixture.code.as_str() }).to_string(),
            ))
            .await
            .expect("direct TLS bootstrap verification");
        assert_eq!(verify.status(), StatusCode::OK);
        assert_no_store(&verify);
        assert!(!response_text(verify).await.contains(fixture.code.as_str()));

        let protected = fixture
            .app
            .clone()
            .oneshot(direct_tls_public_request("GET", "/api/status", ""))
            .await
            .expect("direct TLS protected API");
        assert_eq!(protected.status(), StatusCode::UNAUTHORIZED);
        assert_no_store(&protected);

        let no_peer = fixture
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/status")
                    .body(Body::empty())
                    .expect("missing peer request"),
            )
            .await
            .expect("missing peer response");
        assert_eq!(no_peer.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn webui_security_headers_bind_csp_nonce_and_hsts_to_verified_https() {
        let direct = direct_tls_public_owner_app_fixture(8080);
        let direct_page = direct
            .app
            .clone()
            .oneshot(direct_tls_public_request("GET", "/", ""))
            .await
            .expect("direct TLS secured page");
        assert_eq!(direct_page.status(), StatusCode::OK);
        assert_no_store(&direct_page);
        assert_security_headers(&direct_page, true);
        let direct_nonce = csp_nonce(&direct_page);
        response_html_with_nonce(direct_page, owner_bootstrap::OWNER_BOOTSTRAP_HTML).await;

        let second_direct_page = direct
            .app
            .clone()
            .oneshot(direct_tls_public_request("GET", "/", ""))
            .await
            .expect("second direct TLS secured page");
        assert_security_headers(&second_direct_page, true);
        assert_ne!(
            csp_nonce(&second_direct_page),
            direct_nonce,
            "CSP nonce must be fresh for every HTML response"
        );

        let protected_api = direct
            .app
            .clone()
            .oneshot(direct_tls_public_request("GET", "/api/status", ""))
            .await
            .expect("direct TLS protected API");
        assert_eq!(protected_api.status(), StatusCode::UNAUTHORIZED);
        assert_no_store(&protected_api);
        assert_security_headers(&protected_api, true);

        let proxied = public_owner_app_fixture(8080);
        let proxied_page = proxied
            .app
            .clone()
            .oneshot(verified_public_request("GET", "/", ""))
            .await
            .expect("verified proxy secured page");
        assert_eq!(proxied_page.status(), StatusCode::OK);
        assert_no_store(&proxied_page);
        assert_security_headers(&proxied_page, true);
        response_html_with_nonce(proxied_page, owner_bootstrap::OWNER_BOOTSTRAP_HTML).await;

        let local = owner_app_fixture(8080);
        let local_page = local
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::HOST, "localhost:8080")
                    .body(Body::empty())
                    .expect("local plaintext page"),
            )
            .await
            .expect("local plaintext response");
        assert_eq!(local_page.status(), StatusCode::OK);
        assert_no_store(&local_page);
        assert_security_headers(&local_page, false);
        response_html_with_nonce(local_page, owner_bootstrap::OWNER_BOOTSTRAP_HTML).await;

        let rejected_plaintext = proxied
            .app
            .clone()
            .oneshot(request_from(
                Request::builder()
                    .uri("/api/v1/auth/bootstrap")
                    .header(header::HOST, "relay.xlei.cc")
                    .header("x-forwarded-proto", "http")
                    .header("x-forwarded-host", "relay.xlei.cc")
                    .header("x-forwarded-for", "203.0.113.9")
                    .body(Body::empty())
                    .expect("rejected plaintext proxy request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("rejected plaintext proxy response");
        assert_eq!(rejected_plaintext.status(), StatusCode::FORBIDDEN);
        assert_no_store(&rejected_plaintext);
        assert_security_headers(&rejected_plaintext, false);

        let legacy =
            create_app_web_router_with_auth(Arc::new(AppController::new()), WebUiAuth::disabled());
        let legacy_page = legacy
            .oneshot(
                Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .expect("legacy plaintext page"),
            )
            .await
            .expect("legacy plaintext response");
        assert_eq!(legacy_page.status(), StatusCode::OK);
        assert_no_store(&legacy_page);
        assert_security_headers(&legacy_page, false);
        let policy = response_header(&legacy_page, "content-security-policy");
        assert!(policy.contains("script-src 'self'"));
        assert!(policy.contains("script-src-attr 'none'"));
        assert!(policy.contains("style-src 'self'"));
        assert!(policy.contains("style-src-attr 'unsafe-inline'"));
        assert!(!policy.contains("script-src 'self' 'unsafe-inline'"));
        assert_eq!(
            response_text(legacy_page).await,
            node_surface::NODE_APP_HTML
        );
    }

    #[tokio::test]
    async fn public_owner_claim_requires_verified_https_proxy_and_matching_origin() {
        let fixture = public_owner_app_fixture(8080);

        let untrusted = fixture
            .app
            .clone()
            .oneshot(request_from(
                Request::builder()
                    .uri("/api/v1/auth/bootstrap")
                    .header(header::HOST, "relay.xlei.cc")
                    .header("x-forwarded-proto", "https")
                    .header("x-forwarded-host", "relay.xlei.cc")
                    .header("x-forwarded-for", "203.0.113.9")
                    .body(Body::empty())
                    .expect("untrusted proxy request"),
                IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)),
            ))
            .await
            .expect("untrusted proxy response");
        assert_eq!(untrusted.status(), StatusCode::FORBIDDEN);
        assert_no_store(&untrusted);

        let plaintext = fixture
            .app
            .clone()
            .oneshot(request_from(
                Request::builder()
                    .uri("/api/v1/auth/bootstrap")
                    .header(header::HOST, "relay.xlei.cc")
                    .header("x-forwarded-proto", "http")
                    .header("x-forwarded-host", "relay.xlei.cc")
                    .header("x-forwarded-for", "203.0.113.9")
                    .body(Body::empty())
                    .expect("plaintext proxy request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("plaintext proxy response");
        assert_eq!(plaintext.status(), StatusCode::FORBIDDEN);
        assert_no_store(&plaintext);

        let wrong_host = fixture
            .app
            .clone()
            .oneshot(request_from(
                Request::builder()
                    .uri("/api/v1/auth/bootstrap")
                    .header(header::HOST, "evil.example")
                    .header("x-forwarded-proto", "https")
                    .header("x-forwarded-host", "relay.xlei.cc")
                    .header("x-forwarded-for", "203.0.113.9")
                    .body(Body::empty())
                    .expect("host injection request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("host injection response");
        assert_eq!(wrong_host.status(), StatusCode::FORBIDDEN);
        assert_no_store(&wrong_host);

        let status = fixture
            .app
            .clone()
            .oneshot(verified_public_request("GET", "/api/v1/auth/bootstrap", ""))
            .await
            .expect("verified public bootstrap status");
        assert_eq!(status.status(), StatusCode::OK);
        assert_no_store(&status);
        let payload = response_json(status).await;
        assert_eq!(payload["initialized"], false);
        assert_eq!(payload["origin"], "https://relay.xlei.cc");

        let page = fixture
            .app
            .clone()
            .oneshot(verified_public_request("GET", "/", ""))
            .await
            .expect("public owner bootstrap page");
        assert_eq!(page.status(), StatusCode::OK);
        assert_no_store(&page);
        response_html_with_nonce(page, owner_bootstrap::OWNER_BOOTSTRAP_HTML).await;

        let protected = fixture
            .app
            .clone()
            .oneshot(verified_public_request("GET", "/api/status", ""))
            .await
            .expect("public protected API request");
        assert_eq!(protected.status(), StatusCode::UNAUTHORIZED);
        assert_no_store(&protected);
    }

    #[tokio::test]
    async fn public_owner_claim_binds_webauthn_origin_rejects_url_secrets_and_closes() {
        let fixture = public_owner_app_fixture(8080);

        let query_uri = format!("/api/v1/auth/bootstrap?code={}", fixture.code.as_str());
        let query = fixture
            .app
            .clone()
            .oneshot(verified_public_request("GET", &query_uri, ""))
            .await
            .expect("bootstrap query rejection");
        assert_eq!(query.status(), StatusCode::BAD_REQUEST);
        assert_no_store(&query);
        let query_body = response_text(query).await;
        assert!(!query_body.contains(fixture.code.as_str()));
        assert!(!query_body.contains("?code="));

        let verify = fixture
            .app
            .clone()
            .oneshot(verified_public_request(
                "POST",
                "/api/v1/auth/bootstrap/verify",
                &serde_json::json!({ "code": fixture.code.as_str() }).to_string(),
            ))
            .await
            .expect("public bootstrap verification");
        assert_eq!(verify.status(), StatusCode::OK);
        assert_no_store(&verify);
        let verify_body = response_text(verify).await;
        assert!(!verify_body.contains(fixture.code.as_str()));

        let start = fixture
            .app
            .clone()
            .oneshot(verified_public_request(
                "POST",
                "/api/v1/auth/webauthn/registration/start",
                &serde_json::json!({ "code": fixture.code.as_str() }).to_string(),
            ))
            .await
            .expect("public WebAuthn registration start");
        assert_eq!(start.status(), StatusCode::OK);
        assert_no_store(&start);
        let challenge = response_json(start).await;
        assert_eq!(challenge["publicKey"]["rp"]["id"], "relay.xlei.cc");

        let recovery_code = fixture
            .service
            .finish_registration_for_test(fixture_passkey())
            .expect("public WebAuthn finish fixture");
        assert!(!recovery_code.is_empty());

        let confirm = fixture
            .app
            .clone()
            .oneshot(verified_public_request(
                "POST",
                "/api/v1/auth/bootstrap/recovery-confirm",
                r#"{"confirmed":true}"#,
            ))
            .await
            .expect("public recovery confirmation");
        assert_eq!(confirm.status(), StatusCode::NO_CONTENT);
        assert_no_store(&confirm);

        let closed = fixture
            .app
            .clone()
            .oneshot(verified_public_request("GET", "/api/v1/auth/bootstrap", ""))
            .await
            .expect("closed public claim response");
        assert_eq!(closed.status(), StatusCode::GONE);
        assert_no_store(&closed);
        assert!(!response_text(closed).await.contains(fixture.code.as_str()));

        let (audit, records) = read_audit_records(&fixture.service);
        let verified = records
            .iter()
            .find(|record| {
                record["path"] == "/api/v1/auth/bootstrap/verify" && record["status"] == 200
            })
            .expect("verified public claim audit");
        assert_eq!(verified["channel"], "public");
        assert_eq!(verified["source_ip"], "203.0.113.9");
        assert_eq!(verified["outcome"], "success");

        let permanently_closed = records
            .iter()
            .find(|record| record["status"] == 410)
            .expect("closed public claim audit");
        assert_eq!(permanently_closed["channel"], "public");
        assert_eq!(permanently_closed["source_ip"], "203.0.113.9");
        assert_eq!(permanently_closed["outcome"], "permanently_closed");
        assert_eq!(
            records
                .iter()
                .filter(|record| record["status"] == 200)
                .count(),
            2,
            "GET/status and POST/verify each produce exactly one audit record"
        );
        assert!(records.iter().all(|record| {
            !record["path"]
                .as_str()
                .is_some_and(|path| path.contains('?') || path.contains("code="))
        }));
        assert!(records.iter().all(|record| record["outcome"] != "accepted"));
        assert!(!audit.contains(fixture.code.as_str()));
        assert!(!audit.contains(&recovery_code));
    }

    #[tokio::test]
    async fn owner_bootstrap_index_serves_secure_local_flow_only() {
        let fixture = owner_app_fixture(8080);

        let redirect = fixture
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::HOST, "127.0.0.1:8080")
                    .body(Body::empty())
                    .expect("literal loopback index request"),
            )
            .await
            .expect("literal loopback index response");
        assert_eq!(redirect.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(
            redirect
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok()),
            Some("http://localhost:8080/")
        );
        assert_no_store(&redirect);

        let page = fixture
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::HOST, "localhost:8080")
                    .body(Body::empty())
                    .expect("owner bootstrap index request"),
            )
            .await
            .expect("owner bootstrap index response");
        assert_eq!(page.status(), StatusCode::OK);
        assert_no_store(&page);
        response_html_with_nonce(page, owner_bootstrap::OWNER_BOOTSTRAP_HTML).await;

        let script = fixture
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/assets/owner_bootstrap.js")
                    .body(Body::empty())
                    .expect("owner bootstrap script request"),
            )
            .await
            .expect("owner bootstrap script response");
        assert_eq!(script.status(), StatusCode::OK);
        assert_no_store(&script);
        assert_eq!(
            response_text(script).await,
            owner_bootstrap::OWNER_BOOTSTRAP_JS
        );
    }

    #[tokio::test]
    async fn owner_api_errors_expose_stable_frontend_codes() {
        let recent = owner_service_error(OwnerSecurityError::RecentAuthenticationRequired);
        assert_eq!(recent.status(), StatusCode::FORBIDDEN);
        assert_no_store(&recent);
        assert_eq!(
            response_json(recent).await["code"],
            "recent_authentication_required"
        );

        let last = owner_service_error(OwnerSecurityError::LastPasskey);
        assert_eq!(last.status(), StatusCode::CONFLICT);
        assert_no_store(&last);
        assert_eq!(response_json(last).await["code"], "last_passkey");

        let invalid_session = owner_service_error(OwnerSecurityError::InvalidSession);
        assert_eq!(invalid_session.status(), StatusCode::UNAUTHORIZED);
        assert_no_store(&invalid_session);
        assert_eq!(
            response_json(invalid_session).await["code"],
            "invalid_session"
        );
    }

    #[tokio::test]
    async fn owner_bootstrap_rejects_bad_input_without_echoing_secrets() {
        let fixture = owner_app_fixture(8080);
        let supplied = "not-the-bootstrap-code-but-long-enough";
        let response = fixture
            .app
            .clone()
            .oneshot(request_from(
                json_request(
                    "POST",
                    "/api/v1/auth/bootstrap/verify",
                    &serde_json::json!({ "code": supplied }).to_string(),
                ),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("invalid bootstrap code response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_no_store(&response);
        let body = response_text(response).await;
        assert!(!body.contains(supplied));
        assert!(!body.contains(fixture.code.as_str()));

        let malformed = fixture
            .app
            .clone()
            .oneshot(request_from(
                json_request("POST", "/api/v1/auth/bootstrap/verify", "{\"code\":"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("malformed bootstrap request response");
        assert!(!malformed.status().is_success());
        assert_no_store(&malformed);
        assert!(!response_text(malformed)
            .await
            .contains(fixture.code.as_str()));

        let request_debug = format!(
            "{:?}",
            BootstrapCodeRequest {
                code: fixture.code.to_string(),
            }
        );
        assert!(request_debug.contains("[REDACTED]"));
        assert!(!request_debug.contains(fixture.code.as_str()));

        let response_debug = format!(
            "{:?}",
            WebauthnRegistrationFinishResponse {
                recovery_code: "do-not-log-recovery-code".to_string(),
            }
        );
        assert!(response_debug.contains("[REDACTED]"));
        assert!(!response_debug.contains("do-not-log-recovery-code"));
    }

    #[tokio::test]
    async fn owner_bootstrap_rate_limits_failures_and_audits_each_final_response_once() {
        let fixture = owner_app_fixture(8080);
        let supplied = "wrong-bootstrap-code-that-must-never-be-logged";

        for attempt in 1..=5 {
            let response = fixture
                .app
                .clone()
                .oneshot(request_from(
                    json_request(
                        "POST",
                        "/api/v1/auth/bootstrap/verify",
                        &serde_json::json!({ "code": supplied }).to_string(),
                    ),
                    IpAddr::V4(Ipv4Addr::LOCALHOST),
                ))
                .await
                .expect("invalid bootstrap attempt");
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "attempt {attempt}"
            );
            assert_no_store(&response);
            assert!(!response_text(response).await.contains(supplied));
        }

        let limited = fixture
            .app
            .clone()
            .oneshot(request_from(
                json_request(
                    "POST",
                    "/api/v1/auth/bootstrap/verify",
                    &serde_json::json!({ "code": supplied }).to_string(),
                ),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("rate-limited bootstrap attempt");
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_no_store(&limited);
        assert!(!response_text(limited).await.contains(supplied));

        let (content, records) = read_audit_records(&fixture.service);
        assert_eq!(records.len(), 6);
        assert!(records.iter().all(|record| record["channel"] == "local"));
        assert!(records
            .iter()
            .all(|record| record["source_ip"] == "127.0.0.1"));
        assert_eq!(
            records
                .iter()
                .filter(|record| record["outcome"] == "invalid_code")
                .count(),
            5
        );
        assert_eq!(
            records
                .iter()
                .filter(|record| record["outcome"] == "rate_limited")
                .count(),
            1
        );
        assert!(records.iter().all(|record| record["outcome"] != "accepted"));
        assert!(!content.contains(supplied));
        assert!(!content.contains(fixture.code.as_str()));
    }

    #[tokio::test]
    async fn owner_bootstrap_completion_closes_initialization_and_unlocks_product_ui() {
        let fixture = owner_app_fixture(8080);
        let verify = fixture
            .app
            .clone()
            .oneshot(request_from(
                json_request(
                    "POST",
                    "/api/v1/auth/bootstrap/verify",
                    &serde_json::json!({ "code": fixture.code.as_str() }).to_string(),
                ),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("bootstrap verify response");
        assert_eq!(verify.status(), StatusCode::OK);
        assert_no_store(&verify);

        let start = fixture
            .app
            .clone()
            .oneshot(request_from(
                json_request(
                    "POST",
                    "/api/v1/auth/webauthn/registration/start",
                    &serde_json::json!({ "code": fixture.code.as_str() }).to_string(),
                ),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("WebAuthn registration start response");
        assert_eq!(start.status(), StatusCode::OK);
        assert_no_store(&start);

        let recovery_code = fixture
            .service
            .finish_registration_for_test(fixture_passkey())
            .expect("test webauthn registration finish");
        assert!(!recovery_code.is_empty());

        let confirm = fixture
            .app
            .clone()
            .oneshot(request_from(
                json_request(
                    "POST",
                    "/api/v1/auth/bootstrap/recovery-confirm",
                    r#"{"confirmed":true}"#,
                ),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("recovery confirmation response");
        assert_eq!(confirm.status(), StatusCode::NO_CONTENT);
        assert_no_store(&confirm);

        let status = fixture
            .app
            .clone()
            .oneshot(request_from(
                Request::builder()
                    .uri("/api/v1/auth/bootstrap")
                    .body(Body::empty())
                    .expect("completed bootstrap status request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("completed bootstrap status response");
        assert_eq!(status.status(), StatusCode::OK);
        assert_no_store(&status);
        let payload = response_json(status).await;
        assert_eq!(payload["initialized"], true);

        let closed = fixture
            .app
            .clone()
            .oneshot(request_from(
                json_request(
                    "POST",
                    "/api/v1/auth/webauthn/registration/start",
                    &serde_json::json!({ "code": fixture.code.as_str() }).to_string(),
                ),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("closed registration response");
        assert_eq!(closed.status(), StatusCode::CONFLICT);
        assert_no_store(&closed);
        assert!(!response_text(closed).await.contains(fixture.code.as_str()));

        let page = fixture
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::HOST, "localhost:8080")
                    .body(Body::empty())
                    .expect("completed product index request"),
            )
            .await
            .expect("completed product index response");
        assert_eq!(page.status(), StatusCode::OK);
        assert_no_store(&page);
        assert_eq!(response_text(page).await, node_surface::NODE_APP_HTML);
    }

    #[tokio::test]
    async fn owner_security_failure_locks_the_webui_instead_of_falling_back() {
        let fixture = owner_app_fixture(8080);
        std::fs::create_dir(fixture.config_dir.join("owner"))
            .expect("create incomplete owner directory");

        let page = fixture
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::HOST, "localhost:8080")
                    .body(Body::empty())
                    .expect("locked index request"),
            )
            .await
            .expect("locked index response");
        assert_eq!(page.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_no_store(&page);
        let body = response_html_with_nonce(page, owner_bootstrap::OWNER_UNAVAILABLE_HTML).await;
        assert!(!body.contains("node-snapshot"));
        assert!(!body.contains(node_surface::NODE_APP_HTML));
    }

    #[tokio::test]
    async fn node_adapter_selects_manifest_surface_and_serves_embedded_assets() {
        let auth = WebUiAuth::disabled();
        let (adapter, _commands) = node_adapter_fixture(node_ui_manifest());
        let node_app = create_app_web_router_with_auth_and_node(
            Arc::new(AppController::new()),
            auth.clone(),
            Some(adapter),
        );
        let response = node_app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .expect("node index request"),
            )
            .await
            .expect("node index response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_text(response).await;
        assert_eq!(body, node_surface::NODE_APP_HTML);

        let response = node_app
            .oneshot(
                Request::builder()
                    .uri("/assets/app.js")
                    .body(Body::empty())
                    .expect("node application script request"),
            )
            .await
            .expect("node application script response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/javascript; charset=utf-8")
        );
        assert_eq!(response_text(response).await, node_surface::NODE_APP_JS);

        let legacy_app = create_app_web_router_with_auth(Arc::new(AppController::new()), auth);
        let response = legacy_app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .expect("legacy index request"),
            )
            .await
            .expect("legacy index response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_text(response).await, node_surface::NODE_APP_HTML);
    }

    #[tokio::test]
    async fn node_routes_accept_bearer_only_and_reject_query_tokens() {
        let auth = test_auth();
        let token = auth
            .login_from(IpAddr::V4(Ipv4Addr::LOCALHOST), "admin", "secret")
            .expect("login token");
        let (adapter, _commands) = node_adapter_fixture(node_ui_manifest());
        let app = create_app_web_router_with_auth_and_node(
            Arc::new(AppController::new()),
            auth,
            Some(adapter),
        );

        for uri in [
            "/api/node/manifest?schema_version=1",
            "/api/node/snapshot?schema_version=1",
            "/api/node/commands",
        ] {
            let request = if uri.ends_with("/commands") {
                json_request(
                    "POST",
                    uri,
                    r#"{"schema_version":1,"request":{"request_id":1,"command":"shutdown"}}"#,
                )
            } else {
                Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .expect("unauthorized node request")
            };
            let response = app
                .clone()
                .oneshot(request)
                .await
                .expect("unauthorized node response");
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{uri}");
        }

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/node/snapshot?schema_version=1&token={token}"))
                    .body(Body::empty())
                    .expect("query token snapshot request"),
            )
            .await
            .expect("query token snapshot response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        for uri in [
            "/api/node/manifest?schema_version=1",
            "/api/node/snapshot?schema_version=1",
        ] {
            let response = app
                .clone()
                .oneshot(bearer_request("GET", uri, &token, ""))
                .await
                .expect("authorized node read");
            let status = response.status();
            let body = response_text(response).await;
            assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        }

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/node/events?schema_version=1&token={token}"))
                    .body(Body::empty())
                    .expect("event query token request"),
            )
            .await
            .expect("event query token response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/node/events?schema_version=1")
                    .body(Body::empty())
                    .expect("anonymous event request"),
            )
            .await
            .expect("anonymous event response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn owner_cookie_authorizes_node_api_and_websocket_upgrade() {
        let (adapter, _commands) = node_adapter_fixture(node_ui_manifest());
        let fixture = owner_node_app_fixture(8080, Some(adapter));
        let cookie = initialize_owner_and_issue_local_cookie(&fixture);

        let session = fixture
            .app
            .clone()
            .oneshot(request_from(
                Request::builder()
                    .uri("/api/v1/auth/session")
                    .header(header::COOKIE, cookie.clone())
                    .body(Body::empty())
                    .expect("owner session status request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("owner session status response");
        assert_eq!(session.status(), StatusCode::OK);
        let payload = response_json(session).await;
        assert_eq!(payload["authenticated"], true);
        assert_eq!(payload["channel"], "local");
        assert_eq!(payload["expected_origin"], "http://localhost:8080");

        let snapshot = fixture
            .app
            .clone()
            .oneshot(request_from(
                Request::builder()
                    .uri("/api/node/snapshot?schema_version=1")
                    .header(header::COOKIE, cookie.clone())
                    .body(Body::empty())
                    .expect("owner cookie snapshot request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("owner cookie snapshot response");
        assert_eq!(snapshot.status(), StatusCode::OK);

        let events = fixture
            .app
            .clone()
            .oneshot(request_from(
                Request::builder()
                    .uri("/api/node/events?schema_version=1")
                    .header(header::COOKIE, cookie.clone())
                    .body(Body::empty())
                    .expect("owner cookie event request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("owner cookie event response");
        assert_eq!(events.status(), StatusCode::UPGRADE_REQUIRED);

        let logout = fixture
            .app
            .clone()
            .oneshot(request_from(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/passkey/logout")
                    .header(header::COOKIE, cookie.clone())
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::empty())
                    .expect("owner logout request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("owner logout response");
        assert_eq!(logout.status(), StatusCode::NO_CONTENT);
        let clear_cookie = logout
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .expect("owner logout clear cookie");
        assert!(clear_cookie.contains("HttpOnly"));
        assert!(clear_cookie.contains("SameSite=Strict"));
        assert!(clear_cookie.contains("Max-Age=0"));

        let revoked = fixture
            .app
            .clone()
            .oneshot(request_from(
                Request::builder()
                    .uri("/api/v1/auth/session")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .expect("revoked owner cookie session request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("revoked owner cookie session response");
        assert_eq!(revoked.status(), StatusCode::OK);
        assert_eq!(response_json(revoked).await["authenticated"], false);
    }

    #[tokio::test]
    async fn owner_session_status_exposes_channel_specific_webauthn_origin() {
        let local = owner_app_fixture(8080);
        initialize_owner(&local);
        let local_session = local
            .service
            .issue_session_for_test(OwnerBootstrapChannel::Local)
            .expect("local owner session");
        let local_cookie = format!("{OWNER_SESSION_COOKIE}={}", local_session.token.as_str());
        let local_response = local
            .app
            .clone()
            .oneshot(request_from(
                Request::builder()
                    .uri("/api/v1/auth/session")
                    .header(header::COOKIE, local_cookie)
                    .body(Body::empty())
                    .expect("local session request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("local session response");
        assert_eq!(local_response.status(), StatusCode::OK);
        let local_payload = response_json(local_response).await;
        assert_eq!(local_payload["channel"], "local");
        assert_eq!(local_payload["expected_origin"], "http://localhost:8080");

        let public = public_owner_app_fixture(8080);
        initialize_owner(&public);
        let public_session = public
            .service
            .issue_session_for_test(OwnerBootstrapChannel::Public)
            .expect("public owner session");
        let public_cookie = format!("{OWNER_SESSION_COOKIE}={}", public_session.token.as_str());
        let mut public_request = verified_public_request("GET", "/api/v1/auth/session", "");
        public_request.headers_mut().insert(
            header::COOKIE,
            HeaderValue::from_str(&public_cookie).expect("public session cookie"),
        );
        let public_response = public
            .app
            .clone()
            .oneshot(public_request)
            .await
            .expect("public session response");
        assert_eq!(public_response.status(), StatusCode::OK);
        let public_payload = response_json(public_response).await;
        assert_eq!(public_payload["channel"], "public");
        assert_eq!(public_payload["expected_origin"], "https://relay.xlei.cc");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires Chrome/Chromium and runs the virtual-authenticator browser lifecycle"]
    async fn passkey_browser_lifecycle_uses_real_webauthn_api() -> anyhow::Result<()> {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let port = listener.local_addr()?.port();
        let (adapter, _commands) = node_adapter_fixture(node_ui_manifest());
        let fixture = owner_node_app_fixture(port, Some(adapter));
        let app = fixture.app.clone();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
        });

        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../scripts/test_webui_webauthn_browser.mjs");
        let output = tokio::process::Command::new("node")
            .arg(script)
            .env("FUCKIPV6_E2E_URL", format!("http://localhost:{port}"))
            .env("FUCKIPV6_E2E_BOOTSTRAP_CODE", fixture.code.as_str())
            .output()
            .await?;
        server.abort();

        assert!(
            output.status.success(),
            "WebAuthn browser test failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[tokio::test]
    async fn node_commands_are_executed_through_the_shared_command_bus() {
        let auth = test_auth();
        let token = auth
            .login_from(IpAddr::V4(Ipv4Addr::LOCALHOST), "admin", "secret")
            .expect("login token");
        let (adapter, command_count) = node_adapter_fixture(node_ui_manifest());
        let app = create_app_web_router_with_auth_and_node(
            Arc::new(AppController::new()),
            auth,
            Some(adapter),
        );
        let response = app
            .oneshot(bearer_request(
                "POST",
                "/api/node/commands",
                &token,
                r#"{"schema_version":1,"request":{"request_id":9,"command":{"command":"shutdown"}}}"#,
            ))
            .await
            .expect("command response");
        if response.status() != StatusCode::OK {
            panic!(
                "unexpected command response: {}",
                response_text(response).await
            );
        }
        let payload = response_json(response).await;
        assert_eq!(payload["result"]["request_id"], 9);
        assert_eq!(payload["result"]["outcome"]["status"], "completed");
        assert_eq!(command_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn local_mutation_csrf_accepts_same_origin_owner_cookie_and_rejects_cross_site() {
        let (adapter, command_count) = node_adapter_fixture(node_ui_manifest());
        let fixture = owner_node_app_fixture(8080, Some(adapter));
        let cookie = initialize_owner_and_issue_local_cookie(&fixture);
        let body =
            r#"{"schema_version":1,"request":{"request_id":31,"command":{"command":"shutdown"}}}"#;

        let accepted = fixture
            .app
            .clone()
            .oneshot(request_from(
                local_browser_mutation_request(
                    "/api/node/commands",
                    body,
                    Some(cookie.as_str()),
                    Some("http://localhost:8080"),
                    Some("same-origin"),
                ),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("same-origin owner cookie command");
        assert_eq!(accepted.status(), StatusCode::OK);
        assert_eq!(command_count.load(Ordering::SeqCst), 1);

        let loopback_alias = fixture
            .app
            .clone()
            .oneshot(request_from(
                local_browser_mutation_request(
                    "/api/node/commands",
                    body,
                    Some(cookie.as_str()),
                    Some("http://127.0.0.1:8080"),
                    Some("same-origin"),
                ),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("literal loopback owner cookie command");
        assert_eq!(loopback_alias.status(), StatusCode::FORBIDDEN);
        assert_no_store(&loopback_alias);

        let cross_origin = fixture
            .app
            .clone()
            .oneshot(request_from(
                local_browser_mutation_request(
                    "/api/node/commands",
                    body,
                    Some(cookie.as_str()),
                    Some("https://evil.example"),
                    Some("cross-site"),
                ),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("cross-origin owner cookie command");
        assert_eq!(cross_origin.status(), StatusCode::FORBIDDEN);
        assert_no_store(&cross_origin);

        let cross_site_fetch = fixture
            .app
            .clone()
            .oneshot(request_from(
                local_browser_mutation_request(
                    "/api/node/commands",
                    body,
                    Some(cookie.as_str()),
                    Some("http://localhost:8080"),
                    Some("cross-site"),
                ),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("cross-site fetch metadata command");
        assert_eq!(cross_site_fetch.status(), StatusCode::FORBIDDEN);

        let missing_origin = fixture
            .app
            .clone()
            .oneshot(request_from(
                local_browser_mutation_request(
                    "/api/node/commands",
                    body,
                    Some(cookie.as_str()),
                    None,
                    None,
                ),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("origin-less cookie command");
        assert_eq!(missing_origin.status(), StatusCode::FORBIDDEN);
        assert_eq!(command_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn stale_local_owner_reauthenticates_before_v1_sensitive_commands() {
        let (adapter, command_count) = node_adapter_fixture(node_ui_manifest());
        let fixture = owner_node_app_fixture(8080, Some(adapter));
        let (cookie, issued) = initialize_owner_and_issue_local_session(&fixture);
        let fresh_body = r#"{"schema_version":1,"idempotency_key":"idem_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","request":{"request_id":31,"command":{"command":"shutdown"}}}"#;

        let fresh = fixture
            .app
            .clone()
            .oneshot(request_from(
                local_browser_mutation_request(
                    "/api/v1/commands",
                    fresh_body,
                    Some(cookie.as_str()),
                    Some("http://localhost:8080"),
                    Some("same-origin"),
                ),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("fresh owner sensitive command");
        assert_eq!(fresh.status(), StatusCode::ACCEPTED);
        tokio::time::timeout(Duration::from_secs(2), async {
            while command_count.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("fresh owner operation must reach the handler");
        assert_eq!(command_count.load(Ordering::SeqCst), 1);

        fixture
            .service
            .age_session_for_test(issued.token.as_str(), Duration::from_secs(301))
            .expect("age owner session");
        let stale_body = r#"{"schema_version":1,"idempotency_key":"idem_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAQ","request":{"request_id":32,"command":{"command":"shutdown"}}}"#;
        let stale = fixture
            .app
            .clone()
            .oneshot(request_from(
                local_browser_mutation_request(
                    "/api/v1/commands",
                    stale_body,
                    Some(cookie.as_str()),
                    Some("http://localhost:8080"),
                    Some("same-origin"),
                ),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("stale owner sensitive command");
        let stale_status = stale.status();
        let stale_payload = response_json(stale).await;
        assert_eq!(
            stale_status,
            StatusCode::UNAUTHORIZED,
            "unexpected stale response: {stale_payload}"
        );
        assert_eq!(
            stale_payload["result"]["outcome"]["data"]["code"],
            "authentication_required"
        );
        assert_eq!(
            stale_payload["result"]["outcome"]["data"]["params"]["authorization_code"],
            "permission.recent_authentication_required"
        );
        assert!(stale_payload["operation"].is_null());
        assert_eq!(command_count.load(Ordering::SeqCst), 1);

        let read_body = r#"{"schema_version":1,"request":{"request_id":33,"command":{"command":"get_snapshot"}}}"#;
        let read = fixture
            .app
            .clone()
            .oneshot(request_from(
                local_browser_mutation_request(
                    "/api/v1/commands",
                    read_body,
                    Some(cookie.as_str()),
                    Some("http://localhost:8080"),
                    Some("same-origin"),
                ),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("stale owner read command");
        assert_eq!(read.status(), StatusCode::OK);
        assert_eq!(command_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn anonymous_local_mutations_still_require_same_origin() {
        let (adapter, command_count) = node_adapter_fixture(node_ui_manifest());
        let app = create_app_web_router_with_auth_and_node(
            Arc::new(AppController::new()),
            WebUiAuth::disabled(),
            Some(adapter),
        );
        let body =
            r#"{"schema_version":1,"request":{"request_id":41,"command":{"command":"shutdown"}}}"#;

        let accepted = app
            .clone()
            .oneshot(request_from(
                local_browser_mutation_request(
                    "/api/node/commands",
                    body,
                    None,
                    Some("http://localhost:8080"),
                    Some("same-origin"),
                ),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("same-origin anonymous command");
        assert_eq!(accepted.status(), StatusCode::OK);
        assert_eq!(command_count.load(Ordering::SeqCst), 1);

        let rejected = app
            .oneshot(request_from(
                local_browser_mutation_request(
                    "/api/node/commands",
                    body,
                    None,
                    Some("https://evil.example"),
                    Some("cross-site"),
                ),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("cross-origin anonymous command");
        assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
        assert_eq!(command_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn language_settings_require_auth_and_persist_node_preference() -> anyhow::Result<()> {
        let config_dir = unique_test_dir("language-settings");
        let config = ConfigManager::new_for_dir(config_dir.clone())?;
        let auth = test_auth();
        let token = auth
            .login_from(IpAddr::V4(Ipv4Addr::LOCALHOST), "admin", "secret")
            .expect("language settings token");
        let app = create_app_web_router_with_mutation_limiter(
            Arc::new(AppController::new()),
            auth,
            None,
            None,
            None,
            Arc::new(MutationRateLimiter::default()),
            Some(config.clone()),
        );
        let source = IpAddr::V4(Ipv4Addr::LOCALHOST);

        let unauthorized = app
            .clone()
            .oneshot(request_from(
                json_request("GET", "/api/v1/settings/language", ""),
                source,
            ))
            .await
            .expect("unauthorized language settings response");
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let initial = app
            .clone()
            .oneshot(request_from(
                bearer_request("GET", "/api/v1/settings/language", &token, ""),
                source,
            ))
            .await
            .expect("initial language settings response");
        assert_eq!(initial.status(), StatusCode::OK);
        let initial_body = to_bytes(initial.into_body(), usize::MAX).await?;
        let initial_json: serde_json::Value = serde_json::from_slice(&initial_body)?;
        assert_eq!(initial_json["preference"], "system");
        assert_eq!(
            initial_json["supported_preferences"],
            serde_json::json!(["system", "zh-CN", "en"])
        );
        assert_eq!(
            initial_json["supported_locales"],
            serde_json::json!(["zh-CN", "en"])
        );

        let updated = app
            .oneshot(request_from(
                bearer_request(
                    "PUT",
                    "/api/v1/settings/language",
                    &token,
                    r#"{"preference":"zh-CN"}"#,
                ),
                source,
            ))
            .await
            .expect("updated language settings response");
        assert_eq!(updated.status(), StatusCode::OK);
        let updated_body = to_bytes(updated.into_body(), usize::MAX).await?;
        let updated_json: serde_json::Value = serde_json::from_slice(&updated_body)?;
        assert_eq!(updated_json["preference"], "zh-CN");
        assert_eq!(config.node_language_preference()?, LanguagePreference::ZhCn);
        let reopened = ConfigManager::new_for_dir(config_dir.clone())?;
        assert_eq!(
            reopened.node_language_preference()?,
            LanguagePreference::ZhCn
        );

        let _ = std::fs::remove_dir_all(config_dir);
        Ok(())
    }

    #[tokio::test]
    async fn authenticated_command_rate_limit_is_per_source_and_independent_from_login() {
        let auth = test_auth();
        let token = auth
            .login_from(IpAddr::V4(Ipv4Addr::LOCALHOST), "admin", "secret")
            .expect("login token");
        let (adapter, command_count) = node_adapter_fixture(node_ui_manifest());
        let limiter = Arc::new(MutationRateLimiter::with_policy_for_test(
            2,
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            16,
        ));
        let app = create_app_web_router_with_mutation_limiter(
            Arc::new(AppController::new()),
            auth,
            Some(adapter),
            None,
            None,
            limiter,
            None,
        );
        let source = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10));
        let command_body = |request_id: u64| {
            format!(
                r#"{{"schema_version":1,"request":{{"request_id":{request_id},"command":{{"command":"shutdown"}}}}}}"#
            )
        };

        for request_id in 1..=2 {
            let response = app
                .clone()
                .oneshot(request_from(
                    bearer_request(
                        "POST",
                        "/api/node/commands",
                        &token,
                        &command_body(request_id),
                    ),
                    source,
                ))
                .await
                .expect("rate-limited command response");
            assert_eq!(response.status(), StatusCode::OK);
        }
        let limited = app
            .clone()
            .oneshot(request_from(
                bearer_request("POST", "/api/node/commands", &token, &command_body(3)),
                source,
            ))
            .await
            .expect("command rate limit response");
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_no_store(&limited);
        let retry_after = response_header(&limited, header::RETRY_AFTER.as_str())
            .parse::<u64>()
            .expect("numeric retry-after");
        assert!(retry_after > 0);
        assert_eq!(command_count.load(Ordering::SeqCst), 2);

        let other_source = app
            .clone()
            .oneshot(request_from(
                bearer_request("POST", "/api/node/commands", &token, &command_body(4)),
                IpAddr::V4(Ipv4Addr::new(192, 0, 2, 11)),
            ))
            .await
            .expect("command from independent source");
        assert_eq!(other_source.status(), StatusCode::OK);

        let login = app
            .oneshot(login_request(
                r#"{"username":"admin","password":"secret"}"#,
                source,
            ))
            .await
            .expect("login after command rate limit");
        assert_eq!(login.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn node_manifest_contract_violation_fails_closed_with_502() {
        let auth = test_auth();
        let token = auth
            .login_from(IpAddr::V4(Ipv4Addr::LOCALHOST), "admin", "secret")
            .expect("login token");
        let mut manifest = node_ui_manifest();
        manifest.pages.swap(0, 1);
        let (adapter, _commands) = node_adapter_fixture(manifest);
        let app = create_app_web_router_with_auth_and_node(
            Arc::new(AppController::new()),
            auth,
            Some(adapter),
        );
        let response = app
            .oneshot(bearer_request(
                "GET",
                "/api/node/manifest?schema_version=1",
                &token,
                "",
            ))
            .await
            .expect("invalid manifest response");
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let payload = response_json(response).await;
        assert_eq!(payload["code"], "contract_violation");
        assert_eq!(payload["schema_version"], 1);
    }

    #[tokio::test]
    async fn anonymous_loopback_router_keeps_control_api_accessible() {
        let app =
            create_app_web_router_with_auth(Arc::new(AppController::new()), WebUiAuth::disabled());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/status")
                    .body(Body::empty())
                    .expect("status request"),
            )
            .await
            .expect("router response");
        assert_eq!(response.status(), StatusCode::OK);
        let payload = response_json(response).await;
        assert_eq!(payload["auth_required"], false);
    }

    #[test]
    fn production_webui_start_is_rejected_on_desktop_hosts() {
        let desktop = UiCapabilities {
            display: crate::ui::DisplayEnvironment::LinuxWayland,
            gpui_available: true,
        };
        let error = ensure_webui_start_allowed(desktop)
            .expect_err("a desktop host must never expose a second WebUI surface");
        assert!(error.to_string().contains("桌面会话"));

        let headless = UiCapabilities {
            display: crate::ui::DisplayEnvironment::Headless,
            gpui_available: false,
        };
        ensure_webui_start_allowed(headless).expect("headless WebUI should be allowed");
    }

    #[tokio::test]
    async fn owner_router_requires_a_local_session_even_over_loopback() {
        let (adapter, commands) = node_adapter_fixture(node_ui_manifest());
        let fixture = owner_node_app_fixture(8080, Some(adapter));
        let anonymous = fixture
            .app
            .clone()
            .oneshot(request_from(
                Request::builder()
                    .uri("/api/status")
                    .body(Body::empty())
                    .expect("anonymous status request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("anonymous status response");
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

        let anonymous_node_api = fixture
            .app
            .clone()
            .oneshot(request_from(
                Request::builder()
                    .uri("/api/node/snapshot?schema_version=1")
                    .body(Body::empty())
                    .expect("anonymous node snapshot request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("anonymous node snapshot response");
        assert_eq!(anonymous_node_api.status(), StatusCode::UNAUTHORIZED);

        let anonymous_mutation = fixture
            .app
            .clone()
            .oneshot(request_from(
                Request::builder()
                    .method("POST")
                    .uri("/api/node/commands")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"schema_version":1,"request":{"request_id":30,"command":{"command":"shutdown"}}}"#,
                    ))
                    .expect("anonymous command request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("anonymous command response");
        assert_eq!(anonymous_mutation.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(commands.load(Ordering::SeqCst), 0);

        initialize_owner(&fixture);
        let unauthenticated = fixture
            .app
            .clone()
            .oneshot(request_from(
                Request::builder()
                    .uri("/api/status")
                    .body(Body::empty())
                    .expect("unauthenticated status request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("unauthenticated status response");
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

        let invalid_cookie = fixture
            .app
            .clone()
            .oneshot(request_from(
                Request::builder()
                    .uri("/api/status")
                    .header(header::COOKIE, format!("{OWNER_SESSION_COOKIE}=invalid"))
                    .body(Body::empty())
                    .expect("invalid-cookie status request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("invalid-cookie status response");
        assert_eq!(invalid_cookie.status(), StatusCode::UNAUTHORIZED);

        let bearer_without_owner_cookie = fixture
            .app
            .clone()
            .oneshot(request_from(
                Request::builder()
                    .uri("/api/status")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .body(Body::empty())
                    .expect("bearer-without-owner-cookie status request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("bearer-without-owner-cookie status response");
        assert_eq!(
            bearer_without_owner_cookie.status(),
            StatusCode::UNAUTHORIZED
        );

        let issued = fixture
            .service
            .issue_session_for_test(OwnerBootstrapChannel::Local)
            .expect("owner local session");
        let cookie =
            owner_session_cookie_header(issued.token.as_str(), OwnerBootstrapChannel::Local);
        let authenticated = fixture
            .app
            .clone()
            .oneshot(request_from(
                Request::builder()
                    .uri("/api/status")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .expect("authenticated status request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("authenticated status response");
        assert_eq!(authenticated.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn protected_http_api_rejects_missing_and_invalid_tokens() {
        let auth = test_auth();
        let app = create_app_web_router_with_auth(Arc::new(AppController::new()), auth);

        let missing = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/status")
                    .body(Body::empty())
                    .expect("status request"),
            )
            .await
            .expect("router response");
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            missing
                .headers()
                .get(header::WWW_AUTHENTICATE)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer realm=\"fuckipv6-webui\"")
        );
        let payload = response_json(missing).await;
        assert!(payload["error"]
            .as_str()
            .is_some_and(|message| message.contains("认证失败")));

        let invalid = app
            .oneshot(
                Request::builder()
                    .uri("/api/status")
                    .header(header::AUTHORIZATION, "Bearer not-a-jwt")
                    .body(Body::empty())
                    .expect("status request"),
            )
            .await
            .expect("router response");
        assert_eq!(invalid.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn protected_http_api_does_not_accept_tokens_in_query_strings() {
        let auth = test_auth();
        let token = auth
            .login_from(IpAddr::V4(Ipv4Addr::LOCALHOST), "admin", "secret")
            .expect("login token");
        let app = create_app_web_router_with_auth(Arc::new(AppController::new()), auth);
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/status?token={token}"))
                    .body(Body::empty())
                    .expect("status request"),
            )
            .await
            .expect("router response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn login_rejects_wrong_credentials_and_bearer_token_allows_api_access() {
        let auth = test_auth();
        let app = create_app_web_router_with_auth(Arc::new(AppController::new()), auth);

        let rejected = app
            .clone()
            .oneshot(login_request(
                r#"{"username":"admin","password":"wrong"}"#,
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("login response");
        assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
        let rejected_payload = response_json(rejected).await;
        assert_eq!(rejected_payload["success"], false);
        assert_eq!(rejected_payload["error"], "用户名或密码错误");

        let accepted = app
            .clone()
            .oneshot(login_request(
                r#"{"username":"admin","password":"secret"}"#,
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ))
            .await
            .expect("login response");
        assert_eq!(accepted.status(), StatusCode::OK);
        assert_eq!(
            accepted
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        let accepted_payload = response_json(accepted).await;
        assert_eq!(accepted_payload["success"], true);
        assert_eq!(accepted_payload["auth_required"], true);
        let token = accepted_payload["token"]
            .as_str()
            .expect("login response token")
            .to_string();

        let status = app
            .oneshot(
                Request::builder()
                    .uri("/api/status")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .expect("status request"),
            )
            .await
            .expect("router response");
        assert_eq!(status.status(), StatusCode::OK);
        let status_payload = response_json(status).await;
        assert_eq!(status_payload["auth_required"], true);
    }

    #[test]
    fn login_dto_debug_output_redacts_password_and_token() {
        let request: LoginRequest =
            serde_json::from_str(r#"{"username":"admin","password":"do-not-log-password"}"#)
                .expect("login request");
        let request_debug = format!("{request:?}");
        assert!(request_debug.contains("admin"));
        assert!(!request_debug.contains("do-not-log-password"));
        assert!(request_debug.contains("[REDACTED]"));

        let response = LoginResponse {
            success: true,
            token: Some("do-not-log-token".to_string()),
            error: None,
            expires_in: Some(3600),
            auth_required: true,
        };
        let response_debug = format!("{response:?}");
        assert!(!response_debug.contains("do-not-log-token"));
        assert!(response_debug.contains("[REDACTED]"));
    }

    #[tokio::test]
    async fn login_endpoint_rate_limits_failed_attempts_per_source_ip() {
        let app = create_app_web_router_with_auth(Arc::new(AppController::new()), test_auth());
        let source = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10));

        for _ in 0..5 {
            let response = app
                .clone()
                .oneshot(login_request(
                    r#"{"username":"admin","password":"wrong"}"#,
                    source,
                ))
                .await
                .expect("rejected login response");
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        let throttled = app
            .clone()
            .oneshot(login_request(
                r#"{"username":"admin","password":"secret"}"#,
                source,
            ))
            .await
            .expect("throttled login response");
        assert_eq!(throttled.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            throttled
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok()),
            Some("1")
        );
        let payload = response_json(throttled).await;
        assert_eq!(payload["success"], false);
        assert_eq!(payload["error"], "登录尝试过于频繁，请稍后重试");

        let other_source = app
            .oneshot(login_request(
                r#"{"username":"admin","password":"secret"}"#,
                IpAddr::V4(Ipv4Addr::new(192, 0, 2, 11)),
            ))
            .await
            .expect("login from other source");
        assert_eq!(other_source.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn successful_login_clears_previous_failures() {
        let app = create_app_web_router_with_auth(Arc::new(AppController::new()), test_auth());
        let source = IpAddr::V4(Ipv4Addr::LOCALHOST);

        for _ in 0..4 {
            let response = app
                .clone()
                .oneshot(login_request(
                    r#"{"username":"admin","password":"wrong"}"#,
                    source,
                ))
                .await
                .expect("rejected login response");
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
        let success = app
            .clone()
            .oneshot(login_request(
                r#"{"username":"admin","password":"secret"}"#,
                source,
            ))
            .await
            .expect("successful login response");
        assert_eq!(success.status(), StatusCode::OK);

        for _ in 0..4 {
            let response = app
                .clone()
                .oneshot(login_request(
                    r#"{"username":"admin","password":"wrong"}"#,
                    source,
                ))
                .await
                .expect("rejected login after reset");
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
        let success = app
            .oneshot(login_request(
                r#"{"username":"admin","password":"secret"}"#,
                source,
            ))
            .await
            .expect("successful login after accumulated failures were reset");
        assert_eq!(success.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn websocket_rejects_anonymous_upgrade_when_auth_is_required() {
        let app = create_app_web_router_with_auth(Arc::new(AppController::new()), test_auth());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/ws")
                    .body(Body::empty())
                    .expect("websocket request"),
            )
            .await
            .expect("router response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn websocket_bearer_uses_subprotocol_header_not_query_string() {
        let auth = test_auth();
        let token = auth
            .login_from(IpAddr::V4(Ipv4Addr::LOCALHOST), "admin", "secret")
            .expect("login token");
        let app = create_app_web_router_with_auth(Arc::new(AppController::new()), auth);

        let rejected = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/ws?token={token}"))
                    .header("connection", "Upgrade")
                    .header("upgrade", "websocket")
                    .header("sec-websocket-version", "13")
                    .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
                    .header("sec-websocket-protocol", "fuckipv6")
                    .body(Body::empty())
                    .expect("query-token websocket request"),
            )
            .await
            .expect("query-token websocket response");
        assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);

        let accepted = app
            .oneshot(
                Request::builder()
                    .uri("/ws")
                    .header("connection", "Upgrade")
                    .header("upgrade", "websocket")
                    .header("sec-websocket-version", "13")
                    .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
                    .header(
                        "sec-websocket-protocol",
                        format!("fuckipv6, fuckipv6.bearer.{token}"),
                    )
                    .body(Body::empty())
                    .expect("subprotocol bearer websocket request"),
            )
            .await
            .expect("subprotocol bearer websocket response");
        // `tower::oneshot` does not install an upgrade transport, so axum
        // returns 426 after authentication. Reaching that response proves the
        // subprotocol bearer passed middleware; anonymous/query-token requests
        // are rejected earlier with 401.
        assert_eq!(accepted.status(), StatusCode::UPGRADE_REQUIRED);
    }

    #[tokio::test]
    async fn management_requests_fail_closed_without_running_coordinator() {
        let ctrl = Arc::new(AppController::new());

        let error = ctrl
            .get_blacklist()
            .await
            .expect_err("blacklist query without a coordinator must fail");
        assert_eq!(error.problem().code, NodeProblemCode::NotFound);
        assert!(error.problem().message.contains("当前没有运行中的协调网络"));

        let ban_error = handle_blacklist_ban(
            State(ctrl.clone()),
            Json(BlacklistActionRequest {
                client_id: "node-bad".to_string(),
            }),
        )
        .await
        .expect_err("ban without a running coordinator must not report success");
        let (ban_status, Json(ban_body)) = ban_error;
        assert_eq!(ban_status, StatusCode::NOT_FOUND);
        assert!(!ban_body.success);
        assert!(ban_body.message.contains("当前没有运行中的协调网络"));
        assert!(ban_body.blacklist.is_empty());

        let unban_error = handle_blacklist_unban(
            State(ctrl.clone()),
            Json(BlacklistActionRequest {
                client_id: "node-bad".to_string(),
            }),
        )
        .await
        .expect_err("unban without a running coordinator must not report success");
        let (unban_status, Json(unban_body)) = unban_error;
        assert_eq!(unban_status, StatusCode::NOT_FOUND);
        assert!(!unban_body.success);
        assert!(unban_body.message.contains("当前没有运行中的协调网络"));
        assert!(unban_body.blacklist.is_empty());

        let config_error = handle_get_config(State(ctrl.clone()))
            .await
            .expect_err("allocation query without a coordinator must fail");
        assert_eq!(config_error.0, StatusCode::NOT_FOUND);
        assert_eq!(
            config_error.1.problem.as_ref().map(|problem| problem.code),
            Some(NodeProblemCode::NotFound)
        );

        let (config_status, Json(config_body)) = handle_post_config(
            State(ctrl),
            Json(UpdateNetworkConfigRequest {
                mode: fuckipv6_common::NetworkMode::DualStack,
                ipv4_pool: "10.42.0.0/24".to_string(),
                ipv6_pool: "fd42::1000/120".to_string(),
            }),
        )
        .await;
        assert_eq!(config_status, StatusCode::NOT_FOUND);
        assert!(!config_body.success);
        assert!(config_body.message.contains("当前没有运行中的协调网络"));
    }

    #[tokio::test]
    async fn blacklist_mutations_apply_and_are_idempotent_with_running_coordinator() {
        let ctrl = Arc::new(AppController::new_with_coordinator().await);

        let created = ctrl
            .ban_client(" node-bad ".to_string())
            .await
            .expect("ban client");
        assert!(created.changed);
        assert_eq!(created.clients, vec!["node-bad".to_string()]);

        let repeated = ctrl
            .ban_client("node-bad".to_string())
            .await
            .expect("repeat ban");
        assert!(!repeated.changed);
        assert_eq!(repeated.clients, vec!["node-bad".to_string()]);
        assert_eq!(
            ctrl.get_blacklist().await.expect("read blacklist"),
            vec!["node-bad".to_string()]
        );

        let removed = ctrl.unban_client(" node-bad ").await.expect("unban client");
        assert!(removed.changed);
        assert!(removed.clients.is_empty());

        let repeated = ctrl.unban_client("node-bad").await.expect("repeat unban");
        assert!(!repeated.changed);
        assert!(repeated.clients.is_empty());
    }

    #[tokio::test]
    async fn allocation_updates_round_trip_through_running_coordinator() {
        let ctrl = Arc::new(AppController::new_with_coordinator().await);
        let updated = ctrl
            .update_network_config(UpdateNetworkConfigRequest {
                mode: fuckipv6_common::NetworkMode::Ipv4Only,
                ipv4_pool: "192.168.100.0/24".to_string(),
                ipv6_pool: "fd00::1000/120".to_string(),
            })
            .await
            .expect("update network allocation");
        assert_eq!(updated.mode, fuckipv6_common::NetworkMode::Ipv4Only);
        assert_eq!(updated.ipv4_pool, "192.168.100.0/24");
        assert_eq!(updated.ipv4_prefix, 24);

        let fetched = ctrl
            .get_network_config()
            .await
            .expect("read network allocation");
        assert_eq!(fetched.mode, fuckipv6_common::NetworkMode::Ipv4Only);
        assert_eq!(fetched.ipv4_pool, "192.168.100.0/24");
    }

    #[test]
    fn owner_session_cookie_is_http_only_strict_and_secure_on_public_channel() {
        let local = owner_session_cookie_header("fv1_local", OwnerBootstrapChannel::Local);
        let local = local.to_str().expect("local cookie header");
        assert!(local.contains("HttpOnly"));
        assert!(local.contains("SameSite=Strict"));
        assert!(local.contains("Path=/"));
        assert!(local.contains("Max-Age=43200"));
        assert!(!local.contains("Secure"));

        let public = owner_session_cookie_header("fv1_public", OwnerBootstrapChannel::Public);
        let public = public.to_str().expect("public cookie header");
        assert!(public.contains("HttpOnly"));
        assert!(public.contains("SameSite=Strict"));
        assert!(public.contains("Max-Age=43200"));
        assert!(public.contains("; Secure"));
    }
}
