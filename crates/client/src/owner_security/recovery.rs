//! Local-only owner recovery for the all-Passkeys-lost case.
//!
//! This module intentionally lives outside the WebUI router. A local terminal
//! command verifies the recovery code from the protected owner store, opens an
//! ephemeral loopback-only WebAuthn page, and atomically replaces every old
//! Passkey and session only after the browser confirms the new recovery code.

use super::audit::OwnerSecurityAudit;
use super::store::{OwnerLoad, OwnerPersistence, OwnerStore};
use super::webauthn::OwnerWebauthn;
use crate::config_manager::ConfigManager;
use anyhow::{Context, Result};
use axum::{
    extract::{ConnectInfo, OriginalUri, State},
    http::{header, HeaderMap, HeaderValue, StatusCode, Uri},
    response::{Html, IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use fuckipv6_common::domain::NodeId;
use fuckipv6_common::i18n_browser;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use tokio::{net::TcpListener, sync::watch};
use webauthn_rs::prelude::{
    CreationChallengeResponse, Passkey, PasskeyRegistration, RegisterPublicKeyCredential,
};
use zeroize::Zeroizing;

const RECOVERY_TTL: Duration = Duration::from_secs(10 * 60);
const RECOVERY_HTML: &str = include_str!("owner_recovery.html");
const RECOVERY_CSS: &str = include_str!("owner_recovery.css");
const RECOVERY_JS: &str = include_str!("owner_recovery.js");

#[derive(Debug, Clone, Copy)]
struct RecoveryFailure {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
}

impl RecoveryFailure {
    const fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
        }
    }

    fn forbidden() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "恢复入口仅允许本机回环访问",
        )
    }

    fn invalid_ticket() -> Self {
        Self::new(StatusCode::FORBIDDEN, "invalid_ticket", "恢复会话无效")
    }

    fn conflict(message: &'static str) -> Self {
        Self::new(StatusCode::CONFLICT, "recovery_conflict", message)
    }

    fn registration_failed() -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "registration_failed",
            "Passkey 注册未完成或验证失败，请重新开始",
        )
    }

    fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "本地恢复失败，所有者凭据未更改",
        )
    }

    fn committed_audit_failed() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "recovery_completed_audit_failed",
            "所有者恢复已完成，旧凭据已失效，但完成审计写入失败",
        )
    }
}

enum RecoveryPhase {
    Idle,
    AwaitingRegistration {
        registration: PasskeyRegistration,
        started_at: Instant,
    },
    AwaitingConfirmation {
        candidate: OwnerStore,
        expected_fingerprint: [u8; 32],
        new_recovery_verifier: [u8; 32],
        started_at: Instant,
    },
    Complete,
    Cancelled,
}

struct LocalRecovery {
    persistence: OwnerPersistence,
    audit: OwnerSecurityAudit,
    webauthn: OwnerWebauthn,
    node_id: NodeId,
    display_name: String,
    ticket: Zeroizing<String>,
    phase: Mutex<RecoveryPhase>,
    shutdown: watch::Sender<bool>,
    _recovery_lock: File,
}

impl LocalRecovery {
    fn new(config: &ConfigManager, port: u16, recovery_code: &str) -> Result<Self> {
        if recovery_code.is_empty() {
            anyhow::bail!("恢复码不能为空");
        }
        let persistence = OwnerPersistence::new(config.config_dir().to_path_buf())?;
        let recovery_lock = persistence.acquire_recovery_lock()?;
        let store = match persistence.load()? {
            OwnerLoad::Complete(store) => store,
            OwnerLoad::Uninitialized => anyhow::bail!("节点所有者尚未初始化"),
            OwnerLoad::Corrupt(reason) => anyhow::bail!("所有者安全状态损坏: {reason}"),
        };
        if !persistence.verify_recovery_code(recovery_code)? {
            anyhow::bail!("恢复码无效");
        }
        let identity = config.load_node_identity()?;
        let node_id = NodeId::from_public_key(&identity.public_key_bytes());
        if store.owner.node_id != node_id {
            anyhow::bail!("所有者记录属于其他节点");
        }
        let display_name = config
            .node_display_name()?
            .unwrap_or_else(|| store.owner.display_name.clone());
        let audit = OwnerSecurityAudit::new(config.config_dir())?;
        let webauthn = OwnerWebauthn::new(port)?;
        let (shutdown, _) = watch::channel(false);
        let recovery = Self {
            persistence,
            audit,
            webauthn,
            node_id,
            display_name,
            ticket: generate_secret()?,
            phase: Mutex::new(RecoveryPhase::Idle),
            shutdown,
            _recovery_lock: recovery_lock,
        };
        recovery.audit.record(
            "local",
            Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            "CLI",
            "owner/recover",
            Some(200),
            "local_owner_recovery_authorized",
        )?;
        Ok(recovery)
    }

    fn origin(&self) -> &str {
        self.webauthn.origin()
    }

    fn authority(&self) -> &str {
        self.origin()
            .strip_prefix("http://")
            .unwrap_or_else(|| self.origin())
    }

    fn html(&self) -> String {
        RECOVERY_HTML
            .replace("__RECOVERY_TICKET__", self.ticket.as_str())
            .replace("__RECOVERY_ORIGIN__", self.origin())
    }

    fn validate_http(
        &self,
        remote: SocketAddr,
        headers: &HeaderMap,
        uri: &Uri,
        require_origin: bool,
    ) -> std::result::Result<(), RecoveryFailure> {
        if !remote.ip().is_loopback() {
            return Err(RecoveryFailure::forbidden());
        }
        if uri.query().is_some() {
            return Err(RecoveryFailure::new(
                StatusCode::BAD_REQUEST,
                "query_not_allowed",
                "恢复凭据不得通过 URL 传递",
            ));
        }
        let host = headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok());
        if host != Some(self.authority()) {
            return Err(RecoveryFailure::forbidden());
        }
        if require_origin {
            let origin = headers
                .get(header::ORIGIN)
                .and_then(|value| value.to_str().ok());
            if origin != Some(self.origin()) {
                return Err(RecoveryFailure::forbidden());
            }
        }
        if headers
            .get("sec-fetch-site")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == "cross-site")
        {
            return Err(RecoveryFailure::forbidden());
        }
        Ok(())
    }

    fn check_ticket(&self, ticket: &str) -> std::result::Result<(), RecoveryFailure> {
        if self.ticket.as_bytes().ct_eq(ticket.as_bytes()).into() {
            Ok(())
        } else {
            Err(RecoveryFailure::invalid_ticket())
        }
    }

    fn expire_phase(&self, phase: &mut RecoveryPhase) {
        let started_at = match &*phase {
            RecoveryPhase::AwaitingRegistration { started_at, .. }
            | RecoveryPhase::AwaitingConfirmation { started_at, .. } => *started_at,
            _ => return,
        };
        if started_at.elapsed() >= RECOVERY_TTL {
            *phase = RecoveryPhase::Idle;
        }
    }

    fn start_registration(
        &self,
        ticket: &str,
    ) -> std::result::Result<CreationChallengeResponse, RecoveryFailure> {
        self.check_ticket(ticket)?;
        let mut phase = self.phase.lock().map_err(|_| RecoveryFailure::internal())?;
        self.expire_phase(&mut phase);
        match &*phase {
            RecoveryPhase::Idle => {}
            RecoveryPhase::AwaitingRegistration { .. }
            | RecoveryPhase::AwaitingConfirmation { .. } => {
                return Err(RecoveryFailure::conflict("已有恢复注册流程正在进行"))
            }
            RecoveryPhase::Complete => return Err(RecoveryFailure::conflict("所有者恢复已经完成")),
            RecoveryPhase::Cancelled => {
                return Err(RecoveryFailure::conflict("所有者恢复已经取消"))
            }
        }
        let (challenge, registration) = self
            .webauthn
            .start_registration(&self.node_id, &self.display_name, &[])
            .map_err(|error| {
                tracing::warn!(%error, "无法开始本机所有者恢复 Passkey 注册");
                RecoveryFailure::internal()
            })?;
        *phase = RecoveryPhase::AwaitingRegistration {
            registration,
            started_at: Instant::now(),
        };
        Ok(challenge)
    }

    fn finish_registration(
        &self,
        ticket: &str,
        credential: &RegisterPublicKeyCredential,
    ) -> std::result::Result<String, RecoveryFailure> {
        self.check_ticket(ticket)?;
        let registration = {
            let mut phase = self.phase.lock().map_err(|_| RecoveryFailure::internal())?;
            self.expire_phase(&mut phase);
            let RecoveryPhase::AwaitingRegistration { .. } = &*phase else {
                return Err(RecoveryFailure::conflict("恢复注册流程不存在或已过期"));
            };
            let RecoveryPhase::AwaitingRegistration { registration, .. } =
                std::mem::replace(&mut *phase, RecoveryPhase::Idle)
            else {
                unreachable!("recovery phase was checked while holding the lock");
            };
            registration
        };
        let passkey = match self.webauthn.finish_registration(credential, &registration) {
            Ok(passkey) => passkey,
            Err(error) => {
                tracing::warn!(%error, "本机所有者恢复 Passkey 注册验证失败");
                return Err(RecoveryFailure::registration_failed());
            }
        };
        self.stage_passkey(passkey)
    }

    fn stage_passkey(&self, passkey: Passkey) -> std::result::Result<String, RecoveryFailure> {
        let now = unix_time_ms().map_err(|error| {
            tracing::error!(%error, "无法读取本机所有者恢复时间");
            RecoveryFailure::internal()
        })?;
        let new_recovery_code = generate_secret().map_err(|error| {
            tracing::error!(%error, "无法生成新的所有者恢复码");
            RecoveryFailure::internal()
        })?;
        let new_recovery_verifier = secret_verifier(&new_recovery_code);
        let flags = passkey_flags(&passkey).map_err(|error| {
            tracing::error!(%error, "无法读取新 Passkey 安全属性");
            RecoveryFailure::internal()
        })?;
        let candidate = OwnerStore::new(
            self.node_id.clone(),
            self.display_name.clone(),
            passkey,
            flags.backup_eligible,
            flags.backup_state,
            now,
        )
        .map_err(|error| {
            tracing::error!(%error, "无法构造所有者恢复候选记录");
            RecoveryFailure::internal()
        })?;
        let expected_fingerprint = match self.persistence.load() {
            Ok(OwnerLoad::Complete(store)) => store.fingerprint().map_err(|error| {
                tracing::error!(%error, "无法计算恢复前所有者状态指纹");
                RecoveryFailure::internal()
            })?,
            Ok(OwnerLoad::Uninitialized) => {
                return Err(RecoveryFailure::internal());
            }
            Ok(OwnerLoad::Corrupt(_)) => {
                return Err(RecoveryFailure::internal());
            }
            Err(error) => {
                tracing::error!(%error, "无法重新读取恢复前所有者状态");
                return Err(RecoveryFailure::internal());
            }
        };
        let mut phase = self.phase.lock().map_err(|_| RecoveryFailure::internal())?;
        self.expire_phase(&mut phase);
        if !matches!(*phase, RecoveryPhase::Idle) {
            return Err(RecoveryFailure::conflict("恢复流程状态已变化"));
        }
        let code = new_recovery_code.to_string();
        *phase = RecoveryPhase::AwaitingConfirmation {
            candidate,
            expected_fingerprint,
            new_recovery_verifier,
            started_at: Instant::now(),
        };
        Ok(code)
    }

    fn confirm(
        &self,
        ticket: &str,
        confirmed: bool,
        remote: IpAddr,
    ) -> std::result::Result<(), RecoveryFailure> {
        self.check_ticket(ticket)?;
        if !confirmed {
            return Err(RecoveryFailure::conflict("必须先确认已离线保存新的恢复码"));
        }
        let mut phase = self.phase.lock().map_err(|_| RecoveryFailure::internal())?;
        self.expire_phase(&mut phase);
        let RecoveryPhase::AwaitingConfirmation { .. } = &*phase else {
            return Err(RecoveryFailure::conflict("没有等待确认的恢复结果"));
        };
        self.audit
            .record(
                "local",
                Some(remote),
                "POST",
                "/api/recovery/confirm",
                Some(200),
                "local_owner_recovery_commit_started",
            )
            .map_err(|error| {
                tracing::error!(%error, "无法记录所有者恢复提交审计，操作已失败关闭");
                RecoveryFailure::internal()
            })?;
        let RecoveryPhase::AwaitingConfirmation {
            candidate,
            expected_fingerprint,
            new_recovery_verifier,
            ..
        } = std::mem::replace(&mut *phase, RecoveryPhase::Idle)
        else {
            unreachable!("recovery phase was checked while holding the lock");
        };
        if let Err(error) = self.persistence.replace_after_recovery(
            &expected_fingerprint,
            &candidate,
            &new_recovery_verifier,
        ) {
            tracing::error!(%error, "本机所有者恢复提交失败");
            return Err(RecoveryFailure::internal());
        }
        *phase = RecoveryPhase::Complete;
        drop(phase);
        if let Err(error) = self.audit.record(
            "local",
            Some(remote),
            "POST",
            "/api/recovery/confirm",
            Some(200),
            "local_owner_recovery_completed",
        ) {
            tracing::error!(%error, "所有者恢复已完成，但完成审计写入失败");
            let _ = self.shutdown.send(true);
            return Err(RecoveryFailure::committed_audit_failed());
        }
        let _ = self.shutdown.send(true);
        Ok(())
    }

    fn cancel(&self, ticket: &str, remote: IpAddr) -> std::result::Result<(), RecoveryFailure> {
        self.check_ticket(ticket)?;
        let mut phase = self.phase.lock().map_err(|_| RecoveryFailure::internal())?;
        if matches!(*phase, RecoveryPhase::Complete) {
            return Err(RecoveryFailure::conflict("所有者恢复已经完成"));
        }
        *phase = RecoveryPhase::Cancelled;
        drop(phase);
        let _ = self.audit.record(
            "local",
            Some(remote),
            "POST",
            "/api/recovery/cancel",
            Some(200),
            "local_owner_recovery_cancelled",
        );
        let _ = self.shutdown.send(true);
        Ok(())
    }

    fn completed(&self) -> bool {
        self.phase
            .lock()
            .map(|phase| matches!(*phase, RecoveryPhase::Complete))
            .unwrap_or(false)
    }

    #[cfg(test)]
    fn stage_passkey_for_test(
        &self,
        passkey: Passkey,
    ) -> std::result::Result<String, RecoveryFailure> {
        self.stage_passkey(passkey)
    }
}

#[derive(Serialize)]
struct ApiError<'a> {
    error: &'a str,
    code: &'a str,
}

#[derive(Serialize)]
struct RecoveryCompleteResponse {
    status: &'static str,
}

#[derive(Serialize)]
struct FinishResponse {
    recovery_code: String,
}

#[derive(Deserialize)]
struct TicketRequest {
    ticket: String,
}

#[derive(Deserialize)]
struct FinishRequest {
    ticket: String,
    credential: RegisterPublicKeyCredential,
}

#[derive(Deserialize)]
struct ConfirmRequest {
    ticket: String,
    confirmed: bool,
}

fn recovery_router(recovery: Arc<LocalRecovery>) -> Router {
    Router::new()
        .route("/", get(handle_recovery_page))
        .route("/assets/recovery.css", get(handle_recovery_css))
        .route("/assets/recovery.js", get(handle_recovery_js))
        .route("/assets/i18n.js", get(handle_i18n_js))
        .route("/api/recovery/start", post(handle_recovery_start))
        .route("/api/recovery/finish", post(handle_recovery_finish))
        .route("/api/recovery/confirm", post(handle_recovery_confirm))
        .route("/api/recovery/cancel", post(handle_recovery_cancel))
        .with_state(recovery)
}

async fn handle_recovery_page(
    State(recovery): State<Arc<LocalRecovery>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Response {
    match recovery.validate_http(remote, &headers, &uri, false) {
        Ok(()) => with_security_headers(Html(recovery.html()).into_response()),
        Err(error) => recovery_error_response(&recovery, remote.ip(), "GET", "/", error),
    }
}

async fn handle_recovery_css(
    State(recovery): State<Arc<LocalRecovery>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Response {
    match recovery.validate_http(remote, &headers, &uri, false) {
        Ok(()) => with_security_headers(
            (
                [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                RECOVERY_CSS,
            )
                .into_response(),
        ),
        Err(error) => {
            recovery_error_response(&recovery, remote.ip(), "GET", "/assets/recovery.css", error)
        }
    }
}

async fn handle_recovery_js(
    State(recovery): State<Arc<LocalRecovery>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Response {
    match recovery.validate_http(remote, &headers, &uri, false) {
        Ok(()) => with_security_headers(
            (
                [(
                    header::CONTENT_TYPE,
                    "application/javascript; charset=utf-8",
                )],
                RECOVERY_JS,
            )
                .into_response(),
        ),
        Err(error) => {
            recovery_error_response(&recovery, remote.ip(), "GET", "/assets/recovery.js", error)
        }
    }
}

async fn handle_i18n_js(
    State(recovery): State<Arc<LocalRecovery>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Response {
    match recovery.validate_http(remote, &headers, &uri, false) {
        Ok(()) => with_security_headers(
            (
                [
                    (
                        header::CONTENT_TYPE,
                        "application/javascript; charset=utf-8",
                    ),
                    (header::CACHE_CONTROL, "no-store"),
                ],
                i18n_browser::browser_runtime_script(),
            )
                .into_response(),
        ),
        Err(error) => {
            recovery_error_response(&recovery, remote.ip(), "GET", "/assets/i18n.js", error)
        }
    }
}

async fn handle_recovery_start(
    State(recovery): State<Arc<LocalRecovery>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Json(request): Json<TicketRequest>,
) -> Response {
    let operation = recovery
        .validate_http(remote, &headers, &uri, true)
        .and_then(|()| recovery.start_registration(&request.ticket));
    match operation {
        Ok(challenge) => {
            if let Err(error) = recovery.audit.record(
                "local",
                Some(remote.ip()),
                "POST",
                "/api/recovery/start",
                Some(200),
                "local_owner_recovery_started",
            ) {
                tracing::error!(%error, "无法记录所有者恢复开始审计");
                return recovery_error_response(
                    &recovery,
                    remote.ip(),
                    "POST",
                    "/api/recovery/start",
                    RecoveryFailure::internal(),
                );
            }
            with_security_headers(Json(challenge).into_response())
        }
        Err(error) => {
            recovery_error_response(&recovery, remote.ip(), "POST", "/api/recovery/start", error)
        }
    }
}

async fn handle_recovery_finish(
    State(recovery): State<Arc<LocalRecovery>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Json(request): Json<FinishRequest>,
) -> Response {
    let operation = recovery
        .validate_http(remote, &headers, &uri, true)
        .and_then(|()| recovery.finish_registration(&request.ticket, &request.credential));
    match operation {
        Ok(recovery_code) => {
            if let Err(error) = recovery.audit.record(
                "local",
                Some(remote.ip()),
                "POST",
                "/api/recovery/finish",
                Some(200),
                "local_owner_recovery_passkey_registered",
            ) {
                tracing::error!(%error, "无法记录所有者恢复 Passkey 注册审计");
                return recovery_error_response(
                    &recovery,
                    remote.ip(),
                    "POST",
                    "/api/recovery/finish",
                    RecoveryFailure::internal(),
                );
            }
            with_security_headers(Json(FinishResponse { recovery_code }).into_response())
        }
        Err(error) => recovery_error_response(
            &recovery,
            remote.ip(),
            "POST",
            "/api/recovery/finish",
            error,
        ),
    }
}

async fn handle_recovery_confirm(
    State(recovery): State<Arc<LocalRecovery>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Json(request): Json<ConfirmRequest>,
) -> Response {
    let operation = recovery
        .validate_http(remote, &headers, &uri, true)
        .and_then(|()| recovery.confirm(&request.ticket, request.confirmed, remote.ip()));
    match operation {
        Ok(()) => with_security_headers(
            Json(RecoveryCompleteResponse { status: "complete" }).into_response(),
        ),
        Err(error) => recovery_error_response(
            &recovery,
            remote.ip(),
            "POST",
            "/api/recovery/confirm",
            error,
        ),
    }
}

async fn handle_recovery_cancel(
    State(recovery): State<Arc<LocalRecovery>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Json(request): Json<TicketRequest>,
) -> Response {
    let operation = recovery
        .validate_http(remote, &headers, &uri, true)
        .and_then(|()| recovery.cancel(&request.ticket, remote.ip()));
    match operation {
        Ok(()) => with_security_headers(
            Json(RecoveryCompleteResponse {
                status: "cancelled",
            })
            .into_response(),
        ),
        Err(error) => recovery_error_response(
            &recovery,
            remote.ip(),
            "POST",
            "/api/recovery/cancel",
            error,
        ),
    }
}

fn recovery_error_response(
    recovery: &LocalRecovery,
    remote: IpAddr,
    method: &str,
    path: &str,
    failure: RecoveryFailure,
) -> Response {
    if let Err(error) = recovery.audit.record(
        "local",
        Some(remote),
        method,
        path,
        Some(failure.status.as_u16()),
        "local_owner_recovery_rejected",
    ) {
        tracing::error!(%error, "无法记录所有者恢复拒绝审计");
    }
    with_security_headers(
        (
            failure.status,
            Json(ApiError {
                error: failure.message,
                code: failure.code,
            }),
        )
            .into_response(),
    )
}

fn with_security_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, max-age=0"),
    );
    headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
        ),
    );
    response
}

/// Run the local terminal recovery flow. The recovery code is read from a TTY
/// with echo disabled and is never accepted as a command-line argument.
pub(crate) async fn run_local_recovery() -> Result<()> {
    let config = ConfigManager::new().context("无法读取节点所有者配置目录")?;
    let recovery_code = tokio::task::spawn_blocking(|| rpassword::prompt_password("本机恢复码: "))
        .await
        .context("读取恢复码任务失败")?
        .context("无法从终端读取恢复码")?;
    let recovery_code = Zeroizing::new(recovery_code.trim().to_string());

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .context("无法启动本机恢复监听器")?;
    let port = listener.local_addr()?.port();
    let recovery = Arc::new(LocalRecovery::new(&config, port, &recovery_code)?);
    let url = format!("http://localhost:{port}/");
    let shutdown = recovery.shutdown.subscribe();
    let app = recovery_router(recovery.clone());

    println!("本机所有者恢复仅监听 {url}");
    println!("请在浏览器中完成新 Passkey 注册并保存新的恢复码。关闭终端不会更改现有凭据。");
    if crate::ui::DisplayEnvironment::detect().has_desktop() {
        crate::ui::launch_webui_window(&url);
    }

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(wait_for_recovery_shutdown(shutdown))
    .await
    .context("本机所有者恢复服务失败")?;

    if recovery.completed() {
        println!("所有者恢复已完成：旧 Passkey 和旧会话已失效。");
    } else {
        println!("所有者恢复已取消，现有 Passkey 和会话未更改。");
    }
    Ok(())
}

async fn wait_for_recovery_shutdown(mut shutdown: watch::Receiver<bool>) {
    tokio::select! {
        result = shutdown.changed() => {
            let _ = result;
        }
        result = tokio::signal::ctrl_c() => {
            let _ = result;
        }
    }
}

#[derive(Clone, Copy)]
struct PasskeyFlags {
    backup_eligible: bool,
    backup_state: bool,
}

fn passkey_flags(passkey: &Passkey) -> Result<PasskeyFlags> {
    let value = serde_json::to_value(passkey).context("无法读取 Passkey 安全属性")?;
    let credential = value.get("cred").unwrap_or(&value);
    Ok(PasskeyFlags {
        backup_eligible: credential
            .get("backup_eligible")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        backup_state: credential
            .get("backup_state")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    })
}

fn generate_secret() -> Result<Zeroizing<String>> {
    let mut bytes = Zeroizing::new([0u8; 32]);
    getrandom::fill(bytes.as_mut()).context("无法生成所有者恢复秘密")?;
    Ok(Zeroizing::new(URL_SAFE_NO_PAD.encode(bytes.as_ref())))
}

fn secret_verifier(secret: &str) -> [u8; 32] {
    Sha256::digest(secret.as_bytes()).into()
}

fn unix_time_ms() -> Result<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("系统时间早于 Unix epoch")?
        .as_millis();
    u64::try_from(millis).context("系统时间超出范围")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::owner_security::store::{fixture_passkey, fixture_passkey_with_id, StoredSession};
    use fuckipv6_common::domain::{PasskeyCredentialId, SessionRef};

    fn fixture_config(suffix: &str) -> Result<(ConfigManager, std::path::PathBuf)> {
        let dir = std::env::temp_dir().join(format!(
            "fuckipv6-owner-recovery-{suffix}-{}-{}",
            std::process::id(),
            unix_time_ms()?
        ));
        let config = ConfigManager::new_for_dir(dir.clone())?;
        Ok((config, dir))
    }

    fn provision_owner(config: &ConfigManager, recovery_code: &str) -> Result<NodeId> {
        let identity = config.load_node_identity()?;
        let node_id = NodeId::from_public_key(&identity.public_key_bytes());
        let mut store = OwnerStore::new(
            node_id.clone(),
            "Owner".to_string(),
            fixture_passkey(),
            false,
            false,
            1_700_000_000_000,
        )?;
        store.add_passkey(
            fixture_passkey_with_id(b"owner-recovery-old-secondary"),
            "Old secondary".to_string(),
            false,
            false,
            1_700_000_000_001,
        )?;
        store.insert_session(StoredSession {
            reference: SessionRef::new([9u8; 32]),
            credential_id: store.owner.passkeys[0].credential_id.clone(),
            channel: super::super::store::SessionChannel::Local,
            rp_id: "localhost".to_string(),
            origin: "http://localhost:8080".to_string(),
            created_at_unix_ms: 1_700_000_000_002,
            expires_at_unix_ms: 1_700_000_010_002,
            last_authenticated_at_unix_ms: 1_700_000_000_002,
        })?;
        OwnerPersistence::new(config.config_dir().to_path_buf())?
            .commit(&store, &secret_verifier(recovery_code))?;
        Ok(node_id)
    }

    #[test]
    fn recovery_rotates_passkeys_sessions_and_recovery_code() -> Result<()> {
        let (config, dir) = fixture_config("rotate")?;
        let old_code = "old-recovery-code-0000000000000001";
        provision_owner(&config, old_code)?;
        let recovery = LocalRecovery::new(&config, 19001, old_code)?;
        let new_code = recovery
            .stage_passkey_for_test(fixture_passkey_with_id(b"owner-recovery-new-passkey"))
            .map_err(|failure| anyhow::anyhow!(failure.message))?;
        recovery
            .confirm(
                recovery.ticket.as_str(),
                true,
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            )
            .map_err(|failure| anyhow::anyhow!(failure.message))?;

        let persistence = OwnerPersistence::new(config.config_dir().to_path_buf())?;
        let OwnerLoad::Complete(store) = persistence.load()? else {
            panic!("recovered owner must load completely");
        };
        assert_eq!(store.passkeys.len(), 1);
        assert!(store.sessions.is_empty());
        assert!(store.owner.sessions.is_empty());
        assert_eq!(
            store.passkeys[0].reference.credential_id,
            PasskeyCredentialId::from_public_key(
                fixture_passkey_with_id(b"owner-recovery-new-passkey").cred_id()
            )
        );
        assert!(!persistence.verify_recovery_code(old_code)?);
        assert!(persistence.verify_recovery_code(&new_code)?);
        assert!(recovery
            .confirm(
                recovery.ticket.as_str(),
                true,
                IpAddr::V4(Ipv4Addr::LOCALHOST)
            )
            .is_err());

        let audit = std::fs::read_to_string(recovery.audit.path())?;
        assert!(audit.contains("local_owner_recovery_completed"));
        assert!(!audit.contains(old_code));
        assert!(!audit.contains(&new_code));
        assert!(!audit.contains("owner-recovery-new-passkey"));

        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn wrong_code_cancel_and_stale_state_leave_owner_unchanged() -> Result<()> {
        let (config, dir) = fixture_config("cancel")?;
        let old_code = "old-recovery-code-0000000000000002";
        provision_owner(&config, old_code)?;
        let persistence = OwnerPersistence::new(config.config_dir().to_path_buf())?;
        let OwnerLoad::Complete(before) = persistence.load()? else {
            panic!("owner must load");
        };
        let before_fingerprint = before.fingerprint()?;
        assert!(LocalRecovery::new(&config, 19002, "wrong-code").is_err());

        let recovery = LocalRecovery::new(&config, 19002, old_code)?;
        let _new_code = recovery
            .stage_passkey_for_test(fixture_passkey_with_id(b"owner-recovery-cancelled-passkey"))
            .map_err(|failure| anyhow::anyhow!(failure.message))?;
        recovery
            .cancel(recovery.ticket.as_str(), IpAddr::V4(Ipv4Addr::LOCALHOST))
            .map_err(|failure| anyhow::anyhow!(failure.message))?;
        let OwnerLoad::Complete(after) = persistence.load()? else {
            panic!("owner must remain loadable");
        };
        assert_eq!(before_fingerprint, after.fingerprint()?);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn stale_recovery_candidate_is_rejected() -> Result<()> {
        let (config, dir) = fixture_config("stale")?;
        let old_code = "old-recovery-code-0000000000000003";
        provision_owner(&config, old_code)?;
        let recovery = LocalRecovery::new(&config, 19003, old_code)?;
        let _new_code = recovery
            .stage_passkey_for_test(fixture_passkey_with_id(b"owner-recovery-stale-passkey"))
            .map_err(|failure| anyhow::anyhow!(failure.message))?;
        let persistence = OwnerPersistence::new(config.config_dir().to_path_buf())?;
        persistence.update(|store| {
            store.add_passkey(
                fixture_passkey_with_id(b"owner-recovery-concurrent-passkey"),
                "Concurrent".to_string(),
                false,
                false,
                1_700_000_000_010,
            )?;
            Ok(())
        })?;
        assert!(recovery
            .confirm(
                recovery.ticket.as_str(),
                true,
                IpAddr::V4(Ipv4Addr::LOCALHOST)
            )
            .is_err());
        let OwnerLoad::Complete(store) = persistence.load()? else {
            panic!("owner must remain loadable");
        };
        assert_eq!(store.passkeys.len(), 3);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn expired_confirmation_leaves_owner_and_recovery_code_unchanged() -> Result<()> {
        let (config, dir) = fixture_config("expiry")?;
        let old_code = "old-recovery-code-0000000000000005";
        provision_owner(&config, old_code)?;
        let persistence = OwnerPersistence::new(config.config_dir().to_path_buf())?;
        let OwnerLoad::Complete(before) = persistence.load()? else {
            panic!("owner must load");
        };
        let before_fingerprint = before.fingerprint()?;
        let recovery = LocalRecovery::new(&config, 19005, old_code)?;
        let new_code = recovery
            .stage_passkey_for_test(fixture_passkey_with_id(b"owner-recovery-expired-passkey"))
            .map_err(|failure| anyhow::anyhow!(failure.message))?;

        let mut phase = recovery
            .phase
            .lock()
            .map_err(|_| anyhow::anyhow!("phase lock"))?;
        let RecoveryPhase::AwaitingConfirmation { started_at, .. } = &mut *phase else {
            panic!("recovery must await confirmation");
        };
        *started_at = Instant::now()
            .checked_sub(RECOVERY_TTL)
            .expect("test instant supports recovery ttl");
        drop(phase);

        assert!(recovery
            .confirm(
                recovery.ticket.as_str(),
                true,
                IpAddr::V4(Ipv4Addr::LOCALHOST)
            )
            .is_err());
        let OwnerLoad::Complete(after) = persistence.load()? else {
            panic!("owner must remain loadable");
        };
        assert_eq!(before_fingerprint, after.fingerprint()?);
        assert!(persistence.verify_recovery_code(old_code)?);
        assert!(!persistence.verify_recovery_code(&new_code)?);

        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn recovery_assets_do_not_use_browser_storage() {
        for asset in [RECOVERY_HTML, RECOVERY_CSS, RECOVERY_JS] {
            assert!(!asset.contains("localStorage"));
            assert!(!asset.contains("sessionStorage"));
        }
        assert!(RECOVERY_HTML.contains("__RECOVERY_TICKET__"));
        assert!(RECOVERY_HTML.contains("__RECOVERY_ORIGIN__"));
        assert!(RECOVERY_HTML.contains("/assets/recovery.css"));
        assert!(RECOVERY_HTML.contains("/assets/recovery.js"));
        assert!(RECOVERY_HTML.contains("/assets/i18n.js"));
    }

    #[test]
    fn request_guard_rejects_remote_foreign_host_and_cross_site_origin() -> Result<()> {
        let (config, dir) = fixture_config("guard")?;
        let old_code = "old-recovery-code-0000000000000004";
        provision_owner(&config, old_code)?;
        let recovery = LocalRecovery::new(&config, 19004, old_code)?;
        let uri: Uri = "http://localhost:19004/api/recovery/start".parse()?;
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("localhost:19004"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://localhost:19004"),
        );
        assert!(recovery
            .validate_http("203.0.113.9:50000".parse()?, &headers, &uri, true)
            .is_err());
        assert!(recovery
            .validate_http("127.0.0.1:50000".parse()?, &headers, &uri, true)
            .is_ok());
        headers.insert(header::HOST, HeaderValue::from_static("evil.example:19004"));
        assert!(recovery
            .validate_http("127.0.0.1:50000".parse()?, &headers, &uri, true)
            .is_err());
        headers.insert(header::HOST, HeaderValue::from_static("localhost:19004"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example"),
        );
        assert!(recovery
            .validate_http("127.0.0.1:50000".parse()?, &headers, &uri, true)
            .is_err());
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
}
