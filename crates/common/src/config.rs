use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::types::NetworkMode;

/// 客户端配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientConfig {
    /// 客户端ID (如果为None，将自动生成)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,

    /// 协调服务器地址
    pub coordinator_url: String,

    /// 私钥文件路径
    #[serde(default = "default_private_key_path")]
    pub private_key_path: PathBuf,

    /// 虚拟网络配置
    pub network: NetworkConfig,

    /// TUN设备配置
    #[serde(default)]
    pub tun: TunConfig,

    /// P2P配置
    #[serde(default)]
    pub p2p: P2pConfig,

    /// 日志配置
    #[serde(default)]
    pub log: LogConfig,

    /// 访问控制策略 (ACL)
    #[serde(default)]
    pub acl: crate::acl::AclConfig,

    /// 智能分流与路由策略
    #[serde(default)]
    pub routing: crate::routing::RoutingConfig,

    /// 作为出口网关配置
    #[serde(default)]
    pub exit_node: crate::routing::ExitNodeConfig,
}

impl ClientConfig {
    /// 从TOML文件加载配置
    pub fn from_file(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("读取客户端配置失败: {}", path.display()))?;
        Self::from_toml_str(&content)
    }

    pub fn from_toml_str(content: &str) -> anyhow::Result<Self> {
        let config: Self = toml::from_str(content).context("解析客户端配置 TOML 失败")?;
        config.validate().context("客户端配置校验失败")?;
        Ok(config)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        validate_ws_url(&self.coordinator_url, "coordinator_url")?;
        validate_network_config(&self.network)?;
        validate_tun_config(&self.tun)?;
        validate_p2p_config(&self.p2p)?;
        validate_log_config(&self.log)?;

        if self.private_key_path.as_os_str().is_empty() {
            anyhow::bail!("private_key_path 不能为空");
        }

        Ok(())
    }

    /// 保存配置到TOML文件
    ///
    /// 写入同目录临时文件并 `fsync` 后原子替换目标，避免进程或主机崩溃留下截断配置。
    pub fn save_to_file(&self, path: impl AsRef<std::path::Path>) -> Result<()> {
        let path = path.as_ref();
        let content = toml::to_string_pretty(self).context("序列化客户端配置失败")?;
        atomic_write(path, content.as_bytes())
            .with_context(|| format!("写入客户端配置失败: {}", path.display()))
    }
}

/// 网络配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// 工作模式 (可选，默认使用双栈/由服务器决定)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<NetworkMode>,

    /// 期望的虚拟 IPv4 地址
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_ipv4: Option<Ipv4Addr>,

    /// 期望的虚拟 IPv6 地址
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_ipv6: Option<Ipv6Addr>,

    /// 虚拟网络前缀 (e.g., "fd00::/64")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub virtual_network: Option<String>,

    /// 期望的虚拟IP (可选，由服务器分配)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_ip: Option<Ipv6Addr>,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            mode: Some(NetworkMode::DualStack),
            preferred_ipv4: None,
            preferred_ipv6: None,
            virtual_network: Some("fd00::/64".to_string()),
            preferred_ip: None,
        }
    }
}

/// TUN设备配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunConfig {
    /// TUN设备名称
    #[serde(default = "default_tun_name")]
    pub name: String,

    /// MTU大小
    #[serde(default = "default_mtu")]
    pub mtu: u16,
}

impl Default for TunConfig {
    fn default() -> Self {
        Self {
            name: default_tun_name(),
            mtu: default_mtu(),
        }
    }
}

/// P2P配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct P2pConfig {
    /// QUIC监听端口 (0表示随机)
    #[serde(default)]
    pub listen_port: u16,

    /// 连接超时 (秒)
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout: u64,

    /// 心跳间隔 (秒)
    #[serde(default = "default_heartbeat_interval")]
    pub heartbeat_interval: u64,

    /// Fallback阈值
    #[serde(default)]
    pub fallback_threshold: FallbackThreshold,

    /// NAT 类型检测配置
    #[serde(default)]
    pub nat_detection: NatDetectionConfig,
}

impl Default for P2pConfig {
    fn default() -> Self {
        Self {
            listen_port: 0,
            connect_timeout: default_connect_timeout(),
            heartbeat_interval: default_heartbeat_interval(),
            fallback_threshold: FallbackThreshold::default(),
            nat_detection: NatDetectionConfig::default(),
        }
    }
}

/// NAT 类型检测配置
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NatDetectionConfig {
    /// 是否在客户端注册前执行 NAT 类型检测
    #[serde(default = "default_nat_detection_enabled")]
    pub enabled: bool,

    /// STUN/RFC 5780 服务器地址，至少一个地址用于基础映射检测。
    #[serde(default = "default_stun_servers")]
    pub stun_servers: Vec<String>,

    /// 单次 STUN 探测超时（毫秒）
    #[serde(default = "default_stun_timeout_ms")]
    pub timeout_ms: u64,
}

impl Default for NatDetectionConfig {
    fn default() -> Self {
        Self {
            enabled: default_nat_detection_enabled(),
            stun_servers: default_stun_servers(),
            timeout_ms: default_stun_timeout_ms(),
        }
    }
}

/// Fallback阈值配置
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FallbackThreshold {
    /// 最大可接受RTT (毫秒)
    #[serde(default = "default_max_rtt_ms")]
    pub max_rtt_ms: u64,

    /// 最大丢包率 (0.0-1.0)
    #[serde(default = "default_max_loss_rate")]
    pub max_loss_rate: f32,

    /// 连接超时 (秒)
    #[serde(default = "default_connection_timeout")]
    pub timeout_secs: u64,
}

impl Default for FallbackThreshold {
    fn default() -> Self {
        Self {
            max_rtt_ms: default_max_rtt_ms(),
            max_loss_rate: default_max_loss_rate(),
            timeout_secs: default_connection_timeout(),
        }
    }
}

/// 日志配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogConfig {
    /// 日志级别
    #[serde(default = "default_log_level")]
    pub level: String,

    /// 日志文件路径 (可选)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            file: None,
        }
    }
}

/// 服务器配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinatorConfig {
    /// 信令服务监听地址
    #[serde(default = "default_signaling_bind")]
    pub signaling_bind: String,

    /// 中继服务监听地址
    #[serde(default = "default_relay_bind")]
    pub relay_bind: String,

    /// Web UI监听地址
    #[serde(default = "default_webui_bind")]
    pub webui_bind: String,

    /// 客户端心跳超时时间（秒）
    #[serde(default = "default_heartbeat_timeout_secs")]
    pub heartbeat_timeout_secs: u64,

    /// 虚拟网络分配
    pub network: NetworkAllocation,

    /// TLS证书配置 (可选，用于HTTPS)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls: Option<TlsConfig>,

    /// 日志配置
    #[serde(default)]
    pub log: LogConfig,

    /// WebUI 与 API 访问鉴权配置
    #[serde(default)]
    pub auth: CoordinatorAuthConfig,

    /// 节点准入黑名单配置
    #[serde(default)]
    pub blacklist: crate::acl::BlacklistConfig,

    /// 分布式 QUIC 中继网格节点列表 (可选)
    #[serde(default)]
    pub relay_endpoints: Vec<crate::protocol::RelayEndpoint>,
}

impl CoordinatorConfig {
    /// 从TOML文件加载配置
    pub fn from_file(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("读取协调端配置失败: {}", path.display()))?;
        let config: Self = toml::from_str(&content).context("解析协调端配置 TOML 失败")?;
        config.validate().context("协调端配置校验失败")?;
        Ok(config)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        validate_socket_addr(&self.signaling_bind, "signaling_bind")?;
        validate_socket_addr(&self.relay_bind, "relay_bind")?;
        validate_socket_addr(&self.webui_bind, "webui_bind")?;

        if self.network.mode.is_ipv4_enabled() {
            validate_ipv4_cidr(&self.network.ipv4_pool, "network.ipv4_pool")?;
        }
        if self.network.mode.is_ipv6_enabled() {
            validate_ipv6_cidr(self.network.effective_ipv6_pool(), "network.ipv6_pool")?;
        }
        validate_log_config(&self.log)?;

        if self.heartbeat_timeout_secs == 0 {
            anyhow::bail!("heartbeat_timeout_secs 必须大于 0");
        }

        if let Some(tls) = &self.tls {
            if tls.cert_path.as_os_str().is_empty() {
                anyhow::bail!("tls.cert_path 不能为空");
            }
            if tls.key_path.as_os_str().is_empty() {
                anyhow::bail!("tls.key_path 不能为空");
            }
        }

        Ok(())
    }

    /// 保存配置到TOML文件
    ///
    /// 写入同目录临时文件并 `fsync` 后原子替换目标，避免进程或主机崩溃留下截断配置。
    pub fn save_to_file(&self, path: impl AsRef<std::path::Path>) -> Result<()> {
        let path = path.as_ref();
        let content = toml::to_string_pretty(self).context("序列化协调端配置失败")?;
        atomic_write(path, content.as_bytes())
            .with_context(|| format!("写入协调端配置失败: {}", path.display()))
    }
}

/// 网络地址分配
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkAllocation {
    /// 虚拟网络工作模式 (ipv4_only, ipv6_only, dual_stack)
    #[serde(default = "default_network_mode")]
    pub mode: NetworkMode,

    /// IPv4 地址池 (e.g., "10.8.0.0/24")
    #[serde(default = "default_ipv4_pool")]
    pub ipv4_pool: String,

    /// IPv6 地址池 (e.g., "fd00::1000/120")
    #[serde(default = "default_ipv6_pool")]
    pub ipv6_pool: String,

    /// 兼容旧版 IP地址池配置
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip_pool: Option<String>,
}

impl Default for NetworkAllocation {
    fn default() -> Self {
        Self {
            mode: default_network_mode(),
            ipv4_pool: default_ipv4_pool(),
            ipv6_pool: default_ipv6_pool(),
            ip_pool: None,
        }
    }
}

impl NetworkAllocation {
    pub fn effective_ipv6_pool(&self) -> &str {
        if let Some(pool) = &self.ip_pool {
            pool.as_str()
        } else {
            self.ipv6_pool.as_str()
        }
    }
}

/// TLS证书配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

/// 协调端 WebUI 与 API 访问鉴权配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CoordinatorAuthConfig {
    /// 是否强制开启鉴权（若为 false 且设置了 admin_password，仍自动启用）
    #[serde(default)]
    pub enabled: bool,
    /// 管理员用户名
    #[serde(default = "default_admin_username")]
    pub admin_username: String,
    /// 管理员密码
    #[serde(default)]
    pub admin_password: Option<String>,
    /// JWT 签名密钥（留空时服务启动自动随机生成）
    #[serde(default)]
    pub jwt_secret: Option<String>,
    /// Token 有效期（秒，默认 86400）
    #[serde(default = "default_jwt_expiration_secs")]
    pub jwt_expiration_secs: u64,
}

fn default_admin_username() -> String {
    "admin".to_string()
}

fn default_jwt_expiration_secs() -> u64 {
    86400
}

impl Default for CoordinatorAuthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            admin_username: default_admin_username(),
            admin_password: None,
            jwt_secret: None,
            jwt_expiration_secs: default_jwt_expiration_secs(),
        }
    }
}

impl CoordinatorAuthConfig {
    pub fn is_auth_required(&self) -> bool {
        self.enabled || self.admin_password.is_some()
    }
}

pub fn default_network_mode() -> NetworkMode {
    NetworkMode::DualStack
}

pub fn default_ipv4_pool() -> String {
    "10.8.0.0/24".to_string()
}

pub fn default_ipv6_pool() -> String {
    "fd00::1000/120".to_string()
}

// 默认值函数
fn default_private_key_path() -> PathBuf {
    PathBuf::from("client_key.pem")
}

fn default_tun_name() -> String {
    "fuckipv6".to_string()
}

fn default_mtu() -> u16 {
    1280 // IPv6 最小 MTU 要求
}

fn default_connect_timeout() -> u64 {
    10
}

fn default_heartbeat_interval() -> u64 {
    30
}

fn default_max_rtt_ms() -> u64 {
    200
}

fn default_max_loss_rate() -> f32 {
    0.05
}

fn default_connection_timeout() -> u64 {
    30
}

fn default_nat_detection_enabled() -> bool {
    true
}

fn default_stun_servers() -> Vec<String> {
    vec![
        "stun.l.google.com:19302".to_string(),
        "stun1.l.google.com:19302".to_string(),
    ]
}

fn default_stun_timeout_ms() -> u64 {
    800
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_signaling_bind() -> String {
    "[::]:6666".to_string()
}

fn default_relay_bind() -> String {
    "[::]:6667".to_string()
}

fn default_webui_bind() -> String {
    "[::]:8080".to_string()
}

fn default_heartbeat_timeout_secs() -> u64 {
    120
}

fn validate_ws_url(value: &str, field: &str) -> anyhow::Result<()> {
    if value.trim().is_empty() {
        anyhow::bail!("{} 不能为空", field);
    }

    let Some(rest) = value
        .strip_prefix("ws://")
        .or_else(|| value.strip_prefix("wss://"))
    else {
        anyhow::bail!("{} 必须以 ws:// 或 wss:// 开头", field);
    };

    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty() {
        anyhow::bail!("{} 缺少主机名", field);
    }

    if authority.contains(' ') {
        anyhow::bail!("{} 不能包含空格", field);
    }

    Ok(())
}

fn validate_network_config(config: &NetworkConfig) -> anyhow::Result<()> {
    if let Some(network) = &config.virtual_network {
        validate_ipv6_cidr(network, "network.virtual_network")?;
    }

    Ok(())
}

fn validate_tun_config(config: &TunConfig) -> anyhow::Result<()> {
    if config.name.trim().is_empty() {
        anyhow::bail!("tun.name 不能为空");
    }

    if config.mtu < 1280 {
        anyhow::bail!("tun.mtu 不能小于 IPv6 最小 MTU 1280");
    }

    Ok(())
}

fn validate_p2p_config(config: &P2pConfig) -> anyhow::Result<()> {
    if config.connect_timeout == 0 {
        anyhow::bail!("p2p.connect_timeout 必须大于 0");
    }

    if config.heartbeat_interval == 0 {
        anyhow::bail!("p2p.heartbeat_interval 必须大于 0");
    }

    validate_fallback_threshold(&config.fallback_threshold)?;
    validate_nat_detection_config(&config.nat_detection)
}

fn validate_fallback_threshold(config: &FallbackThreshold) -> anyhow::Result<()> {
    if config.max_rtt_ms == 0 {
        anyhow::bail!("p2p.fallback_threshold.max_rtt_ms 必须大于 0");
    }

    if !(0.0..=1.0).contains(&config.max_loss_rate) {
        anyhow::bail!("p2p.fallback_threshold.max_loss_rate 必须在 0.0 到 1.0 之间");
    }

    if config.timeout_secs == 0 {
        anyhow::bail!("p2p.fallback_threshold.timeout_secs 必须大于 0");
    }

    Ok(())
}

fn validate_nat_detection_config(config: &NatDetectionConfig) -> anyhow::Result<()> {
    if config.timeout_ms == 0 {
        anyhow::bail!("p2p.nat_detection.timeout_ms 必须大于 0");
    }

    if config.enabled && config.stun_servers.is_empty() {
        anyhow::bail!("p2p.nat_detection.stun_servers 启用时不能为空");
    }

    for server in &config.stun_servers {
        if server.trim().is_empty() {
            anyhow::bail!("p2p.nat_detection.stun_servers 不能包含空地址");
        }
    }

    Ok(())
}

fn validate_log_config(config: &LogConfig) -> anyhow::Result<()> {
    match config.level.trim().to_ascii_lowercase().as_str() {
        "trace" | "debug" | "info" | "warn" | "warning" | "error" => Ok(()),
        "" => anyhow::bail!("log.level 不能为空"),
        other => anyhow::bail!(
            "log.level `{}` 无效，可用值: trace, debug, info, warn, error",
            other
        ),
    }
}

fn validate_socket_addr(value: &str, field: &str) -> anyhow::Result<()> {
    value
        .parse::<SocketAddr>()
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("{} 不是有效监听地址 `{}`: {}", field, value, e))
}

pub fn validate_ipv4_cidr(value: &str, field: &str) -> anyhow::Result<()> {
    let Some((ip, prefix)) = value.split_once('/') else {
        anyhow::bail!("{} 必须是 IPv4 CIDR，例如 10.8.0.0/24", field);
    };

    let _ip: Ipv4Addr = ip
        .parse()
        .map_err(|e| anyhow::anyhow!("{} 的 IPv4 地址无效 `{}`: {}", field, ip, e))?;
    let prefix: u8 = prefix
        .parse()
        .map_err(|e| anyhow::anyhow!("{} 的前缀长度无效 `{}`: {}", field, prefix, e))?;

    if prefix > 32 {
        anyhow::bail!("{} 的前缀长度不能大于 32", field);
    }

    Ok(())
}

pub fn validate_ipv6_cidr(value: &str, field: &str) -> anyhow::Result<()> {
    let Some((ip, prefix)) = value.split_once('/') else {
        anyhow::bail!("{} 必须是 IPv6 CIDR，例如 fd00::/64", field);
    };

    let _ip: Ipv6Addr = ip
        .parse()
        .map_err(|e| anyhow::anyhow!("{} 的 IPv6 地址无效 `{}`: {}", field, ip, e))?;
    let prefix: u8 = prefix
        .parse()
        .map_err(|e| anyhow::anyhow!("{} 的前缀长度无效 `{}`: {}", field, prefix, e))?;

    if prefix > 128 {
        anyhow::bail!("{} 的前缀长度不能大于 128", field);
    }

    Ok(())
}

static CONFIG_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .with_context(|| format!("配置路径缺少文件名: {}", path.display()))?;
    let existing_permissions = std::fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions());
    let sequence = CONFIG_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut temporary_name = file_name.to_os_string();
    temporary_name.push(format!(".tmp-{}-{sequence}", std::process::id()));
    let temporary_path = parent.join(temporary_name);

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let write_result = (|| -> Result<()> {
        let mut file = options
            .open(&temporary_path)
            .with_context(|| format!("创建临时配置文件失败: {}", temporary_path.display()))?;
        if let Some(permissions) = existing_permissions {
            std::fs::set_permissions(&temporary_path, permissions)
                .with_context(|| format!("继承配置文件权限失败: {}", temporary_path.display()))?;
        }
        file.write_all(content)
            .with_context(|| format!("写入临时配置文件失败: {}", temporary_path.display()))?;
        file.sync_all()
            .with_context(|| format!("同步临时配置文件失败: {}", temporary_path.display()))?;
        std::fs::rename(&temporary_path, path)
            .with_context(|| format!("原子替换配置文件失败: {}", path.display()))?;
        #[cfg(unix)]
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .with_context(|| format!("同步配置目录失败: {}", parent.display()))?;
        Ok(())
    })();

    if write_result.is_err() {
        let _ = std::fs::remove_file(&temporary_path);
    }
    write_result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_complete_file_and_preserves_permissions() -> Result<()> {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "fuckipv6-atomic-config-{}-{unique}.toml",
            std::process::id()
        ));
        std::fs::write(&path, b"old")?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640))?;
        }

        atomic_write(&path, b"new complete config")?;
        assert_eq!(std::fs::read(&path)?, b"new complete config");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path)?.permissions().mode() & 0o777,
                0o640
            );
        }

        std::fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn test_client_config_serialization() -> anyhow::Result<()> {
        let config = ClientConfig {
            client_id: Some("test".to_string()),
            coordinator_url: "wss://example.com:6666".to_string(),
            private_key_path: PathBuf::from("key.pem"),
            network: NetworkConfig {
                mode: Some(NetworkMode::DualStack),
                preferred_ipv4: None,
                preferred_ipv6: None,
                virtual_network: Some("fd00::/64".to_string()),
                preferred_ip: None,
            },
            tun: TunConfig::default(),
            p2p: P2pConfig::default(),
            log: LogConfig::default(),
            acl: Default::default(),
            routing: Default::default(),
            exit_node: Default::default(),
        };

        let toml_str = toml::to_string_pretty(&config)?;
        let _deserialized: ClientConfig = toml::from_str(&toml_str)?;
        Ok(())
    }

    #[test]
    fn client_config_validation_accepts_defaults() -> anyhow::Result<()> {
        let config = ClientConfig {
            client_id: None,
            coordinator_url: "ws://localhost:6666".to_string(),
            private_key_path: PathBuf::from("key.pem"),
            network: NetworkConfig::default(),
            tun: TunConfig::default(),
            p2p: P2pConfig::default(),
            log: LogConfig::default(),
            acl: Default::default(),
            routing: Default::default(),
            exit_node: Default::default(),
        };

        config.validate()?;
        Ok(())
    }

    #[test]
    fn client_config_validation_rejects_bad_values() {
        let mut config = ClientConfig {
            client_id: None,
            coordinator_url: "http://localhost:6666".to_string(),
            private_key_path: PathBuf::from("key.pem"),
            network: NetworkConfig::default(),
            tun: TunConfig::default(),
            p2p: P2pConfig::default(),
            log: LogConfig::default(),
            acl: Default::default(),
            routing: Default::default(),
            exit_node: Default::default(),
        };

        assert!(config
            .validate()
            .expect_err("http coordinator URL should be rejected")
            .to_string()
            .contains("ws://"));

        config.coordinator_url = "ws://localhost:6666".to_string();
        config.tun.mtu = 1279;
        assert!(config
            .validate()
            .expect_err("MTU below IPv6 minimum should be rejected")
            .to_string()
            .contains("1280"));

        config.tun.mtu = 1280;
        config.p2p.fallback_threshold.max_loss_rate = 1.5;
        assert!(config
            .validate()
            .expect_err("loss rate above 1.0 should be rejected")
            .to_string()
            .contains("max_loss_rate"));

        config.p2p.fallback_threshold.max_loss_rate = 0.05;
        config.log.level = "verbose".to_string();
        assert!(config
            .validate()
            .expect_err("unsupported log level should be rejected")
            .to_string()
            .contains("log.level"));

        let mut config = ClientConfig {
            client_id: None,
            coordinator_url: "ws://localhost:6666".to_string(),
            private_key_path: PathBuf::from("key.pem"),
            network: NetworkConfig::default(),
            tun: TunConfig::default(),
            p2p: P2pConfig::default(),
            log: LogConfig::default(),
            acl: Default::default(),
            routing: Default::default(),
            exit_node: Default::default(),
        };
        config.p2p.nat_detection.timeout_ms = 0;
        assert!(config
            .validate()
            .expect_err("zero NAT detection timeout should be rejected")
            .to_string()
            .contains("timeout_ms"));

        config.p2p.nat_detection.timeout_ms = 800;
        config.p2p.nat_detection.stun_servers.clear();
        assert!(config
            .validate()
            .expect_err("empty STUN server list should be rejected")
            .to_string()
            .contains("stun_servers"));
    }

    #[test]
    fn coordinator_config_validation() -> anyhow::Result<()> {
        let config = CoordinatorConfig {
            signaling_bind: "[::]:6666".to_string(),
            relay_bind: "[::]:6667".to_string(),
            webui_bind: "0.0.0.0:8080".to_string(),
            heartbeat_timeout_secs: 120,
            network: NetworkAllocation {
                mode: NetworkMode::DualStack,
                ipv4_pool: "10.8.0.0/24".to_string(),
                ipv6_pool: "fd00::1000/120".to_string(),
                ip_pool: Some("fd00::1000/120".to_string()),
            },
            tls: None,
            log: LogConfig::default(),
            auth: CoordinatorAuthConfig::default(),
            blacklist: Default::default(),
            relay_endpoints: Default::default(),
        };

        config.validate()?;

        let mut invalid = config.clone();
        invalid.signaling_bind = "not an addr".to_string();
        assert!(invalid
            .validate()
            .expect_err("invalid signaling bind address should be rejected")
            .to_string()
            .contains("signaling_bind"));

        let mut invalid = config;
        invalid.network.ip_pool = Some("fd00::/129".to_string());
        assert!(invalid
            .validate()
            .expect_err("invalid IPv6 prefix length should be rejected")
            .to_string()
            .contains("128"));
        Ok(())
    }
}
