use anyhow::{Context, Result};
use fuckipv6_common::CoordinatorAuthConfig;
use fuckipv6_coordinator::auth::jwt::{Claims, JwtManager};
use std::collections::BTreeMap;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const DEFAULT_WEBUI_BIND: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
pub const DEFAULT_WEBUI_PORT: u16 = 8080;
pub const ENV_WEBUI_BIND: &str = "FUCKIPV6_WEBUI_BIND";
pub const ENV_WEBUI_ALLOW_REMOTE: &str = "FUCKIPV6_WEBUI_ALLOW_REMOTE";
pub const ENV_WEBUI_USERNAME: &str = "FUCKIPV6_WEBUI_USERNAME";
pub const ENV_WEBUI_PASSWORD: &str = "FUCKIPV6_WEBUI_PASSWORD";
pub const ENV_WEBUI_JWT_SECRET: &str = "FUCKIPV6_WEBUI_JWT_SECRET";

const MIN_JWT_SECRET_BYTES: usize = 32;
const MIN_JWT_SECRET_ENTROPY_BITS: f64 = 128.0;
const LOGIN_MAX_FAILURES: u32 = 5;
const LOGIN_BASE_BACKOFF: Duration = Duration::from_secs(1);
const LOGIN_MAX_BACKOFF: Duration = Duration::from_secs(5 * 60);
const LOGIN_ENTRY_TTL: Duration = Duration::from_secs(15 * 60);
const LOGIN_MAX_ENTRIES: usize = 4_096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LoginThrottlePolicy {
    max_failures: u32,
    base_backoff: Duration,
    max_backoff: Duration,
    entry_ttl: Duration,
    max_entries: usize,
}

impl Default for LoginThrottlePolicy {
    fn default() -> Self {
        Self {
            max_failures: LOGIN_MAX_FAILURES,
            base_backoff: LOGIN_BASE_BACKOFF,
            max_backoff: LOGIN_MAX_BACKOFF,
            entry_ttl: LOGIN_ENTRY_TTL,
            max_entries: LOGIN_MAX_ENTRIES,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebUiLoginError {
    InvalidCredentials,
    RateLimited { retry_after: Duration },
    Internal,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct LoginKey {
    source_ip: IpAddr,
    username_hash: [u8; 32],
}

#[derive(Clone, Copy, Debug)]
struct LoginAttemptState {
    failures: u32,
    last_seen: Instant,
    blocked_until: Option<Instant>,
}

#[derive(Debug, PartialEq, Eq)]
enum LoginThrottleError {
    InvalidCredentials,
    RateLimited(Duration),
    Unavailable,
}

#[derive(Debug)]
struct LoginThrottle {
    policy: LoginThrottlePolicy,
    entries: Mutex<BTreeMap<LoginKey, LoginAttemptState>>,
}

impl LoginThrottle {
    fn new(policy: LoginThrottlePolicy) -> Self {
        Self {
            policy,
            entries: Mutex::new(BTreeMap::new()),
        }
    }

    fn evaluate(
        &self,
        key: LoginKey,
        verify_credentials: impl FnOnce() -> bool,
    ) -> std::result::Result<(), LoginThrottleError> {
        self.evaluate_at(key, Instant::now(), verify_credentials)
    }

    fn evaluate_at(
        &self,
        key: LoginKey,
        now: Instant,
        verify_credentials: impl FnOnce() -> bool,
    ) -> std::result::Result<(), LoginThrottleError> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| LoginThrottleError::Unavailable)?;
        self.prune_expired_at(&mut entries, now);

        if let Some(state) = entries.get(&key) {
            if let Some(blocked_until) = state.blocked_until {
                if blocked_until > now {
                    return Err(LoginThrottleError::RateLimited(
                        blocked_until.duration_since(now),
                    ));
                }
            }
        }

        if verify_credentials() {
            entries.remove(&key);
            return Ok(());
        }

        if entries.len() >= self.policy.max_entries.max(1) && !entries.contains_key(&key) {
            if let Some(oldest_key) = entries
                .iter()
                .filter(|(_, state)| {
                    state
                        .blocked_until
                        .is_none_or(|blocked_until| blocked_until <= now)
                })
                .min_by_key(|(_, state)| state.last_seen)
                .map(|(key, _)| key.clone())
            {
                entries.remove(&oldest_key);
            } else {
                // Never evict an active backoff entry. Otherwise an attacker
                // could reset a target key by flooding the bounded table with
                // unique usernames.
                return Err(LoginThrottleError::RateLimited(Duration::from_secs(1)));
            }
        }

        let state = entries.entry(key).or_insert(LoginAttemptState {
            failures: 0,
            last_seen: now,
            blocked_until: None,
        });
        state.failures = state.failures.saturating_add(1);
        state.last_seen = now;
        state.blocked_until = None;
        if state.failures >= self.policy.max_failures.max(1) {
            let exponent = state
                .failures
                .saturating_sub(self.policy.max_failures.max(1))
                .min(31);
            let multiplier = 1_u32.checked_shl(exponent).unwrap_or(u32::MAX);
            let backoff = self
                .policy
                .base_backoff
                .saturating_mul(multiplier)
                .min(self.policy.max_backoff);
            state.blocked_until = now.checked_add(backoff);
        }

        Err(LoginThrottleError::InvalidCredentials)
    }

    fn prune_expired_at(&self, entries: &mut BTreeMap<LoginKey, LoginAttemptState>, now: Instant) {
        let ttl = self.policy.entry_ttl;
        entries.retain(|_, state| {
            now.checked_duration_since(state.last_seen)
                .is_none_or(|age| age < ttl)
        });
    }

    #[cfg(test)]
    fn entry_count_at(&self, now: Instant) -> usize {
        let mut entries = self.entries.lock().expect("login throttle state");
        self.prune_expired_at(&mut entries, now);
        entries.len()
    }
}

#[derive(Clone)]
pub struct WebUiAuth {
    config: CoordinatorAuthConfig,
    jwt_manager: Arc<JwtManager>,
    login_throttle: Arc<LoginThrottle>,
}

impl WebUiAuth {
    pub fn disabled() -> Self {
        Self::new(CoordinatorAuthConfig::default())
            .expect("disabled WebUI authentication configuration must be valid")
    }

    pub fn new(config: CoordinatorAuthConfig) -> Result<Self> {
        Self::new_inner(config, LoginThrottlePolicy::default(), true)
    }

    fn new_inner(
        config: CoordinatorAuthConfig,
        login_policy: LoginThrottlePolicy,
        enforce_secret_strength: bool,
    ) -> Result<Self> {
        if config.is_auth_required()
            && config
                .admin_password
                .as_deref()
                .is_none_or(|password| password.is_empty())
        {
            anyhow::bail!("WebUI 认证已启用但没有配置非空管理员密码");
        }

        let secret = match config.jwt_secret.clone() {
            Some(secret) if !secret.is_empty() => {
                if config.is_auth_required() && enforce_secret_strength {
                    validate_explicit_jwt_secret(&secret)?;
                }
                secret
            }
            Some(_) => anyhow::bail!("{ENV_WEBUI_JWT_SECRET} 不能为空"),
            None if config.is_auth_required() => ephemeral_secret()?,
            None => "fuckipv6-webui-authentication-disabled".to_string(),
        };
        let jwt_manager = Arc::new(JwtManager::new(
            secret.into_bytes(),
            config.jwt_expiration_secs.max(1),
        ));
        Ok(Self {
            config,
            jwt_manager,
            login_throttle: Arc::new(LoginThrottle::new(login_policy)),
        })
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(config: CoordinatorAuthConfig) -> Result<Self> {
        Self::new_inner(config, LoginThrottlePolicy::default(), false)
    }

    pub fn from_process_env() -> Result<Self> {
        let password = optional_env(ENV_WEBUI_PASSWORD)?;
        let username = optional_env(ENV_WEBUI_USERNAME)?
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "admin".to_string());
        let jwt_secret = optional_env(ENV_WEBUI_JWT_SECRET)?;
        Self::new(CoordinatorAuthConfig {
            enabled: password.is_some(),
            admin_username: username,
            admin_password: password,
            jwt_secret,
            ..CoordinatorAuthConfig::default()
        })
    }

    pub fn is_required(&self) -> bool {
        self.config.is_auth_required()
    }

    pub fn admin_username(&self) -> &str {
        &self.config.admin_username
    }

    pub fn expiration_secs(&self) -> u64 {
        self.config.jwt_expiration_secs.max(1)
    }

    pub fn login_from(
        &self,
        source_ip: IpAddr,
        username: &str,
        password: &str,
    ) -> std::result::Result<String, WebUiLoginError> {
        if !self.is_required() {
            return Ok("anonymous".to_string());
        }

        let key = LoginKey {
            source_ip,
            username_hash: *blake3::hash(username.as_bytes()).as_bytes(),
        };
        self.login_throttle
            .evaluate(key, || self.credentials_match(username, password))
            .map_err(|error| match error {
                LoginThrottleError::InvalidCredentials => WebUiLoginError::InvalidCredentials,
                LoginThrottleError::RateLimited(retry_after) => {
                    WebUiLoginError::RateLimited { retry_after }
                }
                LoginThrottleError::Unavailable => WebUiLoginError::Internal,
            })?;

        self.jwt_manager
            .sign(username, "admin")
            .map_err(|_| WebUiLoginError::Internal)
    }

    pub fn verify(&self, token: &str) -> Result<Claims> {
        if !self.is_required() {
            return Ok(Claims {
                sub: "anonymous".to_string(),
                role: "admin".to_string(),
                exp: u64::MAX,
                iat: 0,
            });
        }
        self.jwt_manager.verify(token)
    }

    fn credentials_match(&self, username: &str, password: &str) -> bool {
        let username_matches =
            constant_time_eq(self.admin_username().as_bytes(), username.as_bytes());
        let password_matches = self
            .config
            .admin_password
            .as_deref()
            .is_some_and(|expected| constant_time_eq(expected.as_bytes(), password.as_bytes()));
        username_matches && password_matches
    }
}

impl fmt::Debug for WebUiAuth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WebUiAuth")
            .field("required", &self.is_required())
            .field("admin_username", &self.admin_username())
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct WebUiConfig {
    bind: IpAddr,
    port: u16,
    allow_remote: bool,
    auth: WebUiAuth,
}

impl WebUiConfig {
    pub fn from_process_env(port: u16) -> Result<Self> {
        let bind = match std::env::var(ENV_WEBUI_BIND) {
            Ok(value) if value.is_empty() => anyhow::bail!("{ENV_WEBUI_BIND} 不能为空"),
            Ok(value) => value
                .parse::<IpAddr>()
                .with_context(|| format!("{ENV_WEBUI_BIND} 不是有效 IP 地址: {value}"))?,
            Err(std::env::VarError::NotPresent) => DEFAULT_WEBUI_BIND,
            Err(std::env::VarError::NotUnicode(_)) => {
                anyhow::bail!("{ENV_WEBUI_BIND} 必须是有效的 UTF-8")
            }
        };
        let allow_remote = parse_bool_env(ENV_WEBUI_ALLOW_REMOTE)?;
        Self::new(bind, port, allow_remote, WebUiAuth::from_process_env()?)
    }

    pub fn from_env(bind: IpAddr, port: u16, allow_remote: bool) -> Result<Self> {
        Self::new(bind, port, allow_remote, WebUiAuth::from_process_env()?)
    }

    pub fn new(bind: IpAddr, port: u16, allow_remote: bool, auth: WebUiAuth) -> Result<Self> {
        if !bind.is_loopback() && !allow_remote {
            anyhow::bail!(
                "拒绝监听非回环地址 {bind}:{port}；远程 WebUI/API 必须显式设置 {ENV_WEBUI_ALLOW_REMOTE}=1"
            );
        }
        if !bind.is_loopback() && !auth.is_required() {
            anyhow::bail!(
                "拒绝启动远程 WebUI/API：{bind}:{port} 不是回环地址，但未配置认证；请设置 {ENV_WEBUI_PASSWORD}（可选 {ENV_WEBUI_USERNAME}）"
            );
        }
        Ok(Self {
            bind,
            port,
            allow_remote,
            auth,
        })
    }

    pub fn loopback(port: u16) -> Self {
        Self {
            bind: DEFAULT_WEBUI_BIND,
            port,
            allow_remote: false,
            auth: WebUiAuth::disabled(),
        }
    }

    pub fn bind(&self) -> IpAddr {
        self.bind
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn socket_addr(&self) -> SocketAddr {
        SocketAddr::new(self.bind, self.port)
    }

    pub fn auth(&self) -> &WebUiAuth {
        &self.auth
    }

    pub fn is_remote(&self) -> bool {
        !self.bind.is_loopback()
    }

    pub fn url(&self) -> String {
        http_url(SocketAddr::new(self.bind, self.port))
    }

    pub fn validate_runtime_exposure(&self) -> Result<()> {
        let addr = self.socket_addr();
        if self.is_remote() {
            if !self.allow_remote {
                anyhow::bail!("远程 WebUI/API 缺少显式危险开关，拒绝启动");
            }
            if !self.auth.is_required() {
                anyhow::bail!("远程 WebUI/API 未配置认证，拒绝启动");
            }
            tracing::warn!(
                listen = %addr,
                authentication = "required",
                dangerous_remote_bind = true,
                "危险：WebUI/API 正在监听非回环地址；远程控制面仅允许持有有效 Bearer Token 的管理员访问"
            );
        } else {
            tracing::info!(
                listen = %addr,
                authentication = if self.auth.is_required() { "required" } else { "disabled" },
                "WebUI/API 仅监听回环地址"
            );
        }
        Ok(())
    }
}

impl Default for WebUiConfig {
    fn default() -> Self {
        Self::loopback(DEFAULT_WEBUI_PORT)
    }
}

impl fmt::Debug for WebUiConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WebUiConfig")
            .field("bind", &self.bind)
            .field("port", &self.port)
            .field("allow_remote", &self.allow_remote)
            .field("auth", &self.auth)
            .finish()
    }
}

pub(crate) fn http_url(addr: SocketAddr) -> String {
    match addr.ip() {
        IpAddr::V4(ip) => format!("http://{ip}:{}", addr.port()),
        IpAddr::V6(ip) => format!("http://[{ip}]:{}", addr.port()),
    }
}

fn optional_env(name: &str) -> Result<Option<String>> {
    match std::env::var(name) {
        Ok(value) if value.is_empty() => anyhow::bail!("{name} 不能为空"),
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            anyhow::bail!("{name} 必须是有效的 UTF-8")
        }
    }
}

fn parse_bool_env(name: &str) -> Result<bool> {
    match std::env::var(name) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => anyhow::bail!("{name} 必须是 1/true/yes/on 或 0/false/no/off"),
        },
        Err(std::env::VarError::NotPresent) => Ok(false),
        Err(std::env::VarError::NotUnicode(_)) => {
            anyhow::bail!("{name} 必须是有效的 UTF-8")
        }
    }
}

fn ephemeral_secret() -> Result<String> {
    let mut secret = [0_u8; 32];
    getrandom::fill(&mut secret).context("无法为 WebUI JWT 生成安全随机密钥")?;
    Ok(secret.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn validate_explicit_jwt_secret(secret: &str) -> Result<()> {
    let bytes = secret.as_bytes();
    if bytes.len() < MIN_JWT_SECRET_BYTES {
        anyhow::bail!(
            "认证开启时显式配置的 {ENV_WEBUI_JWT_SECRET} 至少需要 {MIN_JWT_SECRET_BYTES} 字节"
        );
    }

    let entropy_bits = estimated_entropy_bits(bytes);
    if entropy_bits < MIN_JWT_SECRET_ENTROPY_BITS {
        anyhow::bail!(
            "认证开启时显式配置的 {ENV_WEBUI_JWT_SECRET} 估计熵不足 {MIN_JWT_SECRET_ENTROPY_BITS:.0} bit；请使用密码学随机生成的长密钥"
        );
    }
    Ok(())
}

fn estimated_entropy_bits(bytes: &[u8]) -> f64 {
    if bytes.is_empty() {
        return 0.0;
    }
    let mut counts = [0_usize; 256];
    for &byte in bytes {
        counts[usize::from(byte)] += 1;
    }
    let length = bytes.len() as f64;
    counts
        .into_iter()
        .filter(|count| *count > 0)
        .map(|count| {
            let probability = count as f64 / length;
            -probability * probability.log2()
        })
        .sum::<f64>()
        * length
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        let lhs = left.get(index).copied().unwrap_or(0);
        let rhs = right.get(index).copied().unwrap_or(0);
        difference |= usize::from(lhs ^ rhs);
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv6Addr;

    fn auth(password: &str) -> WebUiAuth {
        WebUiAuth::new_for_test(CoordinatorAuthConfig {
            enabled: true,
            admin_username: "admin".to_string(),
            admin_password: Some(password.to_string()),
            jwt_secret: Some("test-webui-jwt-secret".to_string()),
            jwt_expiration_secs: 3600,
        })
        .expect("test auth configuration")
    }

    #[test]
    fn default_and_loopback_ipv6_bind_are_local() {
        let default = WebUiConfig::default();
        assert_eq!(default.bind(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert!(default.bind().is_loopback());
        assert!(!default.is_remote());
        assert_eq!(default.url(), "http://127.0.0.1:8080");

        let ipv6 = WebUiConfig::new(
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            8080,
            false,
            WebUiAuth::disabled(),
        )
        .expect("IPv6 loopback must be accepted");
        assert!(ipv6.bind().is_loopback());
        assert_eq!(ipv6.url(), "http://[::1]:8080");
    }

    #[test]
    fn remote_bind_requires_explicit_danger_flag() {
        let error = WebUiConfig::new(
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            8080,
            false,
            auth("secret"),
        )
        .expect_err("wildcard bind without danger flag must fail");
        assert!(error.to_string().contains(ENV_WEBUI_ALLOW_REMOTE));
    }

    #[test]
    fn remote_bind_requires_configured_authentication() {
        for bind in [
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        ] {
            let error = WebUiConfig::new(bind, 8080, true, WebUiAuth::disabled())
                .expect_err("anonymous remote bind must fail");
            assert!(error.to_string().contains("未配置认证"));
        }
    }

    #[test]
    fn remote_bind_with_authentication_is_explicitly_allowed() {
        let config = WebUiConfig::new(
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            8080,
            true,
            auth("secret"),
        )
        .expect("authenticated remote bind should be allowed");
        assert!(config.is_remote());
        assert!(config.auth().is_required());
        config
            .validate_runtime_exposure()
            .expect("validated remote exposure");
    }

    #[test]
    fn authentication_rejects_wrong_password_and_accepts_valid_jwt() {
        let auth = auth("secret");
        let source = IpAddr::V4(Ipv4Addr::LOCALHOST);
        assert_eq!(
            auth.login_from(source, "admin", "wrong"),
            Err(WebUiLoginError::InvalidCredentials)
        );
        assert_eq!(
            auth.login_from(source, "wrong", "secret"),
            Err(WebUiLoginError::InvalidCredentials)
        );
        let token = auth
            .login_from(source, "admin", "secret")
            .expect("valid credentials");
        assert_eq!(
            auth.verify(&token)
                .expect("issued token should verify")
                .role,
            "admin"
        );
        assert!(auth.verify("not-a-jwt").is_err());
    }

    fn throttle_policy(
        max_failures: u32,
        base_backoff: Duration,
        max_backoff: Duration,
        entry_ttl: Duration,
        max_entries: usize,
    ) -> LoginThrottlePolicy {
        LoginThrottlePolicy {
            max_failures,
            base_backoff,
            max_backoff,
            entry_ttl,
            max_entries,
        }
    }

    fn login_key(ip: Ipv4Addr, username: &str) -> LoginKey {
        LoginKey {
            source_ip: IpAddr::V4(ip),
            username_hash: *blake3::hash(username.as_bytes()).as_bytes(),
        }
    }

    #[test]
    fn login_throttle_backs_off_per_ip_and_username_and_unlocks_on_success() {
        let policy = throttle_policy(
            2,
            Duration::from_secs(1),
            Duration::from_secs(8),
            Duration::from_secs(60),
            16,
        );
        let throttle = LoginThrottle::new(policy);
        let now = Instant::now();
        let key = login_key(Ipv4Addr::new(192, 0, 2, 1), "admin");

        assert_eq!(
            throttle.evaluate_at(key.clone(), now, || false),
            Err(LoginThrottleError::InvalidCredentials)
        );
        assert_eq!(
            throttle.evaluate_at(key.clone(), now, || false),
            Err(LoginThrottleError::InvalidCredentials)
        );
        assert_eq!(
            throttle.evaluate_at(key.clone(), now, || true),
            Err(LoginThrottleError::RateLimited(Duration::from_secs(1)))
        );

        let other_ip = login_key(Ipv4Addr::new(192, 0, 2, 2), "admin");
        assert_eq!(throttle.evaluate_at(other_ip.clone(), now, || true), Ok(()));
        let other_user = login_key(Ipv4Addr::new(192, 0, 2, 1), "operator");
        assert_eq!(throttle.evaluate_at(other_user, now, || true), Ok(()));

        assert_eq!(
            throttle.evaluate_at(key, now + Duration::from_secs(1), || true),
            Ok(())
        );
    }

    #[test]
    fn successful_login_clears_accumulated_failures() {
        let throttle = LoginThrottle::new(throttle_policy(
            3,
            Duration::from_secs(1),
            Duration::from_secs(8),
            Duration::from_secs(60),
            16,
        ));
        let now = Instant::now();
        let key = login_key(Ipv4Addr::LOCALHOST, "admin");

        assert_eq!(
            throttle.evaluate_at(key.clone(), now, || false),
            Err(LoginThrottleError::InvalidCredentials)
        );
        assert_eq!(
            throttle.evaluate_at(key.clone(), now, || false),
            Err(LoginThrottleError::InvalidCredentials)
        );
        assert_eq!(throttle.evaluate_at(key.clone(), now, || true), Ok(()));
        assert_eq!(throttle.entry_count_at(now), 0);

        assert_eq!(
            throttle.evaluate_at(key.clone(), now, || false),
            Err(LoginThrottleError::InvalidCredentials)
        );
        assert_eq!(throttle.evaluate_at(key, now, || true), Ok(()));
    }

    #[test]
    fn login_throttle_expires_stale_entries_and_bounds_capacity() {
        let throttle = LoginThrottle::new(throttle_policy(
            5,
            Duration::from_secs(1),
            Duration::from_secs(8),
            Duration::from_secs(10),
            2,
        ));
        let now = Instant::now();

        for (index, ip) in [1_u8, 2, 3].into_iter().enumerate() {
            let key = login_key(Ipv4Addr::new(192, 0, 2, ip), "admin");
            let _ = throttle.evaluate_at(key, now + Duration::from_millis(index as u64), || false);
        }
        assert_eq!(throttle.entry_count_at(now + Duration::from_secs(1)), 2);
        assert!(!throttle
            .entries
            .lock()
            .expect("login throttle state")
            .contains_key(&login_key(Ipv4Addr::new(192, 0, 2, 1), "admin")));

        assert_eq!(throttle.entry_count_at(now + Duration::from_secs(11)), 0);
    }

    #[test]
    fn login_throttle_does_not_evict_an_active_backoff_entry() {
        let throttle = LoginThrottle::new(throttle_policy(
            1,
            Duration::from_secs(5),
            Duration::from_secs(8),
            Duration::from_secs(60),
            1,
        ));
        let now = Instant::now();
        let blocked = login_key(Ipv4Addr::new(192, 0, 2, 1), "admin");
        let flood = login_key(Ipv4Addr::new(192, 0, 2, 2), "different-user");

        assert_eq!(
            throttle.evaluate_at(blocked.clone(), now, || false),
            Err(LoginThrottleError::InvalidCredentials)
        );
        assert_eq!(
            throttle.evaluate_at(flood, now, || false),
            Err(LoginThrottleError::RateLimited(Duration::from_secs(1)))
        );
        assert_eq!(
            throttle.evaluate_at(blocked.clone(), now, || true),
            Err(LoginThrottleError::RateLimited(Duration::from_secs(5)))
        );
        assert_eq!(
            throttle.evaluate_at(blocked, now + Duration::from_secs(5), || true),
            Ok(())
        );
    }

    #[test]
    fn explicit_jwt_secret_requires_length_and_estimated_entropy_when_auth_is_enabled() {
        let base = CoordinatorAuthConfig {
            enabled: true,
            admin_username: "admin".to_string(),
            admin_password: Some("secret".to_string()),
            jwt_secret: Some("too-short".to_string()),
            jwt_expiration_secs: 3600,
        };
        let length_error = WebUiAuth::new(base.clone()).expect_err("short secret must fail");
        assert!(length_error.to_string().contains("至少需要 32 字节"));
        assert!(!length_error.to_string().contains("too-short"));

        let weak_secret = "a".repeat(64);
        let weak_error = WebUiAuth::new(CoordinatorAuthConfig {
            jwt_secret: Some(weak_secret.clone()),
            ..base.clone()
        })
        .expect_err("low entropy secret must fail");
        assert!(weak_error.to_string().contains("估计熵不足"));
        assert!(!weak_error.to_string().contains(&weak_secret));

        WebUiAuth::new(CoordinatorAuthConfig {
            jwt_secret: Some("0123456789ABCDEFGHIJKLMNOPQRSTUV".to_string()),
            ..base.clone()
        })
        .expect("32 distinct bytes provide at least 128 bits of estimated entropy");

        WebUiAuth::new(CoordinatorAuthConfig {
            enabled: false,
            admin_password: None,
            jwt_secret: Some("short-but-authentication-is-disabled".to_string()),
            ..base
        })
        .expect("disabled authentication does not validate an explicitly configured secret");
    }

    #[test]
    fn auth_debug_redacts_password_secret_and_token_material() {
        let auth = auth("password-must-not-appear");
        let debug = format!("{auth:?}");
        assert!(debug.contains("required"));
        assert!(!debug.contains("password-must-not-appear"));
        assert!(!debug.contains("test-webui-jwt-secret"));

        let token = auth
            .login_from(
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                "admin",
                "password-must-not-appear",
            )
            .expect("valid login");
        assert!(!format!("{auth:?}").contains(&token));
    }
}
