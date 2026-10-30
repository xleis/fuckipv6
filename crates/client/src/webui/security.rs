use anyhow::{Context, Result};
use fuckipv6_common::CoordinatorAuthConfig;
use fuckipv6_coordinator::auth::jwt::{Claims, JwtManager};
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

pub const DEFAULT_WEBUI_BIND: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
pub const DEFAULT_WEBUI_PORT: u16 = 8080;
pub const ENV_WEBUI_BIND: &str = "FUCKIPV6_WEBUI_BIND";
pub const ENV_WEBUI_ALLOW_REMOTE: &str = "FUCKIPV6_WEBUI_ALLOW_REMOTE";
pub const ENV_WEBUI_USERNAME: &str = "FUCKIPV6_WEBUI_USERNAME";
pub const ENV_WEBUI_PASSWORD: &str = "FUCKIPV6_WEBUI_PASSWORD";
pub const ENV_WEBUI_JWT_SECRET: &str = "FUCKIPV6_WEBUI_JWT_SECRET";

#[derive(Clone)]
pub struct WebUiAuth {
    config: CoordinatorAuthConfig,
    jwt_manager: Arc<JwtManager>,
}

impl WebUiAuth {
    pub fn disabled() -> Self {
        Self::new(CoordinatorAuthConfig::default())
            .expect("disabled WebUI authentication configuration must be valid")
    }

    pub fn new(config: CoordinatorAuthConfig) -> Result<Self> {
        if config.is_auth_required()
            && config
                .admin_password
                .as_deref()
                .is_none_or(|password| password.is_empty())
        {
            anyhow::bail!("WebUI 认证已启用但没有配置非空管理员密码");
        }

        let secret = match config.jwt_secret.clone() {
            Some(secret) if !secret.is_empty() => secret,
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
        })
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

    pub fn login(&self, username: &str, password: &str) -> Result<String> {
        if !self.is_required() {
            return Ok("anonymous".to_string());
        }

        let username_matches =
            constant_time_eq(self.admin_username().as_bytes(), username.as_bytes());
        let password_matches = self
            .config
            .admin_password
            .as_deref()
            .is_some_and(|expected| constant_time_eq(expected.as_bytes(), password.as_bytes()));
        if !username_matches || !password_matches {
            anyhow::bail!("用户名或密码错误");
        }

        self.jwt_manager.sign(username, "admin")
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
        WebUiAuth::new(CoordinatorAuthConfig {
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
        assert!(auth.login("admin", "wrong").is_err());
        assert!(auth.login("wrong", "secret").is_err());
        let token = auth.login("admin", "secret").expect("valid credentials");
        assert_eq!(
            auth.verify(&token)
                .expect("issued token should verify")
                .role,
            "admin"
        );
        assert!(auth.verify("not-a-jwt").is_err());
    }
}
