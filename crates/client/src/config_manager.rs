use anyhow::{Context, Result};
use fuckipv6_common::i18n::LanguagePreference;
use fuckipv6_common::{ClientConfig, Identity};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;

const NODE_IDENTITY_FILE: &str = "node_identity.pem";
const NODE_METADATA_FILE: &str = "node_identity.json";

/// 网络配置信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub name: String,
    pub coordinator_url: String,
    pub created_at: String,
    pub last_used: String,
    /// Whether this node owns and must restore the embedded signaling/relay
    /// services for this network.
    #[serde(default)]
    pub owns_embedded_services: bool,
    /// The concrete listening ports are persisted after the services bind so a
    /// restart can restore the same invitation endpoint. `None` keeps old
    /// network-list files readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signaling_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip_pool: Option<String>,
}

/// 网络配置列表
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct NetworkList {
    pub networks: HashMap<String, NetworkConfig>,
    pub default_network: Option<String>,
}

/// Node-wide metadata that is independent from any one joined network.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NodeMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    #[serde(default)]
    language: LanguagePreference,
}

/// 配置管理器
#[derive(Clone)]
pub struct ConfigManager {
    config_dir: PathBuf,
}

impl ConfigManager {
    /// 创建配置管理器
    pub fn new() -> Result<Self> {
        let config_dir = Self::get_config_dir()?;

        // 确保配置目录存在
        std::fs::create_dir_all(&config_dir).context("无法创建配置目录")?;

        Ok(Self { config_dir })
    }

    /// Create a manager rooted at an explicit directory.
    ///
    /// This is used by embedders and tests that must not touch the user's
    /// default configuration directory.
    pub fn new_for_dir(config_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&config_dir).context("无法创建配置目录")?;
        Ok(Self { config_dir })
    }

    /// Read-only root used for node-local protected state.
    pub(crate) fn config_dir(&self) -> &std::path::Path {
        &self.config_dir
    }

    /// 获取配置目录
    fn get_config_dir() -> Result<PathBuf> {
        Self::resolve_config_dir(
            std::env::var_os("FUCKIPV6_CONFIG_DIR"),
            std::env::var_os("XDG_CONFIG_HOME"),
            std::env::var_os("HOME"),
        )
    }

    fn resolve_config_dir(
        configured: Option<OsString>,
        xdg_config: Option<OsString>,
        home: Option<OsString>,
    ) -> Result<PathBuf> {
        // Service managers need a fixed, writable configuration root that is
        // independent of the invoking user's desktop session.
        if let Some(configured) = configured {
            return Ok(PathBuf::from(configured));
        }

        // 优先使用 $XDG_CONFIG_HOME/fuckipv6
        if let Some(xdg_config) = xdg_config {
            return Ok(PathBuf::from(xdg_config).join("fuckipv6"));
        }

        // 其次使用 $HOME/.config/fuckipv6
        if let Some(home) = home {
            return Ok(PathBuf::from(home).join(".config").join("fuckipv6"));
        }

        anyhow::bail!("无法确定配置目录位置")
    }

    /// 获取网络配置目录
    fn get_network_dir(&self, network_name: &str) -> PathBuf {
        self.config_dir.join(network_name)
    }

    fn validate_network_name(network_name: &str) -> Result<()> {
        if network_name.is_empty() {
            anyhow::bail!("网络名称不能为空");
        }

        if network_name == "." || network_name == ".." {
            anyhow::bail!("网络名称不能为 '.' 或 '..'");
        }

        if network_name
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'))
        {
            anyhow::bail!("网络名称只能包含字母、数字、'.'、'-' 和 '_'");
        }

        Ok(())
    }

    /// 获取网络列表文件路径
    fn get_networks_file(&self) -> PathBuf {
        self.config_dir.join("networks.json")
    }

    /// Stable node identity shared by every network hosted by this node.
    pub(crate) fn node_identity_path(&self) -> PathBuf {
        self.config_dir.join(NODE_IDENTITY_FILE)
    }

    /// Per-network coordinator configuration, including the persistent blacklist.
    pub(crate) fn coordinator_config_path(&self, network_name: &str) -> Result<PathBuf> {
        Self::validate_network_name(network_name)?;
        Ok(self.get_network_dir(network_name).join("coordinator.toml"))
    }

    /// Load or create the node-wide signing identity.
    pub(crate) fn load_node_identity(&self) -> Result<Identity> {
        Identity::load_or_generate(self.node_identity_path()).context("无法加载节点身份")
    }

    fn load_node_metadata(&self) -> Result<NodeMetadata> {
        let metadata_path = self.config_dir.join(NODE_METADATA_FILE);
        if !metadata_path.exists() {
            return Ok(NodeMetadata::default());
        }
        let content = std::fs::read_to_string(&metadata_path)
            .with_context(|| format!("无法读取节点元数据: {}", metadata_path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("无法解析节点元数据: {}", metadata_path.display()))
    }

    fn save_node_metadata(&self, metadata: &NodeMetadata) -> Result<()> {
        let metadata_path = self.config_dir.join(NODE_METADATA_FILE);
        let content = serde_json::to_string_pretty(metadata).context("无法序列化节点元数据")?;
        std::fs::write(&metadata_path, content)
            .with_context(|| format!("无法写入节点元数据: {}", metadata_path.display()))
    }

    /// Load the optional display name from node-wide metadata.
    pub(crate) fn node_display_name(&self) -> Result<Option<String>> {
        normalize_display_name(self.load_node_metadata()?.display_name)
    }

    /// Persist a node-wide display name. Empty or whitespace-only values clear it.
    #[cfg(test)]
    pub(crate) fn set_node_display_name(&self, display_name: Option<&str>) -> Result<()> {
        let mut metadata = self.load_node_metadata()?;
        metadata.display_name = normalize_display_name(display_name.map(str::to_string))?;
        self.save_node_metadata(&metadata)
    }

    /// Node-local language choice. This is deliberately not synchronized as
    /// network state: each owner controls their own interface language.
    pub(crate) fn node_language_preference(&self) -> Result<LanguagePreference> {
        Ok(self.load_node_metadata()?.language)
    }

    pub(crate) fn set_node_language_preference(&self, language: LanguagePreference) -> Result<()> {
        let mut metadata = self.load_node_metadata()?;
        metadata.language = language;
        self.save_node_metadata(&metadata)
    }

    /// 加载网络列表
    fn load_network_list(&self) -> Result<NetworkList> {
        let networks_file = self.get_networks_file();

        if networks_file.exists() {
            let content = std::fs::read_to_string(&networks_file)
                .with_context(|| format!("无法读取网络列表: {}", networks_file.display()))?;
            Ok(serde_json::from_str(&content)
                .with_context(|| format!("无法解析网络列表: {}", networks_file.display()))?)
        } else {
            Ok(NetworkList::default())
        }
    }

    /// 保存网络列表
    fn save_network_list(&self, list: &NetworkList) -> Result<()> {
        let networks_file = self.get_networks_file();
        let content = serde_json::to_string_pretty(list).context("无法序列化网络列表")?;
        let temporary_file =
            networks_file.with_extension(format!("json.tmp-{}", std::process::id()));
        std::fs::write(&temporary_file, content)
            .with_context(|| format!("无法写入临时网络列表: {}", temporary_file.display()))?;
        if let Err(error) = std::fs::rename(&temporary_file, &networks_file) {
            let _ = std::fs::remove_file(&temporary_file);
            return Err(error)
                .with_context(|| format!("无法替换网络列表: {}", networks_file.display()));
        }
        Ok(())
    }

    /// 创建新网络配置
    pub fn create_network(
        &self,
        network_name: &str,
        coordinator_url: &str,
        set_default: bool,
    ) -> Result<PathBuf> {
        Self::validate_network_name(network_name)?;
        let network_dir = self.get_network_dir(network_name);
        let config_file = network_dir.join("config.toml");
        let mut list = self.load_network_list()?;

        // The coordinator persists coordinator.toml under the same per-network
        // directory before the client configuration is created. Only an
        // existing client config (or list entry) is a duplicate network.
        if list.networks.contains_key(network_name) || config_file.exists() {
            anyhow::bail!("网络配置已存在: {}", network_name);
        }

        // 创建网络目录
        std::fs::create_dir_all(&network_dir)
            .with_context(|| format!("无法创建网络配置目录: {}", network_dir.display()))?;

        // 生成配置文件
        let config = ClientConfig {
            client_id: None,
            coordinator_url: coordinator_url.to_string(),
            private_key_path: self.node_identity_path(),
            network: Default::default(),
            tun: Default::default(),
            p2p: Default::default(),
            log: Default::default(),
            acl: Default::default(),
            routing: Default::default(),
            exit_node: Default::default(),
        };

        config
            .save_to_file(&config_file)
            .with_context(|| format!("无法保存网络配置: {}", config_file.display()))?;

        // 更新网络列表
        let now = chrono::Utc::now().to_rfc3339();
        list.networks.insert(
            network_name.to_string(),
            NetworkConfig {
                name: network_name.to_string(),
                coordinator_url: coordinator_url.to_string(),
                created_at: now.clone(),
                last_used: now,
                owns_embedded_services: false,
                signaling_port: None,
                relay_port: None,
                external_host: None,
                ip_pool: None,
            },
        );

        if set_default || list.default_network.is_none() {
            list.default_network = Some(network_name.to_string());
        }

        self.save_network_list(&list)?;

        Ok(config_file)
    }

    /// Record the concrete endpoints of embedded services owned by this node.
    /// This must happen after the listeners bind because port `0` is resolved by
    /// the operating system and the invitation ticket contains the actual port.
    pub fn set_embedded_service_metadata(
        &self,
        network_name: &str,
        signaling_port: u16,
        relay_port: u16,
        external_host: Option<String>,
        ip_pool: Option<String>,
    ) -> Result<()> {
        Self::validate_network_name(network_name)?;
        let mut list = self.load_network_list()?;
        let network = list
            .networks
            .get_mut(network_name)
            .with_context(|| format!("无法保存内嵌服务元数据：网络 [{network_name}] 不存在"))?;
        network.owns_embedded_services = true;
        network.signaling_port = Some(signaling_port);
        network.relay_port = Some(relay_port);
        network.external_host = external_host;
        network.ip_pool = ip_pool;
        self.save_network_list(&list)?;
        Ok(())
    }

    /// 获取网络配置文件路径
    pub fn get_network_config(&self, network_name: &str) -> Result<PathBuf> {
        Self::validate_network_name(network_name)?;
        let config_file = self.get_network_dir(network_name).join("config.toml");

        if !config_file.exists() {
            anyhow::bail!("网络配置不存在: {}", network_name);
        }

        // 更新最后使用时间
        if let Ok(mut list) = self.load_network_list() {
            if let Some(network) = list.networks.get_mut(network_name) {
                network.last_used = chrono::Utc::now().to_rfc3339();
                let _ = self.save_network_list(&list);
            }
        }

        Ok(config_file)
    }

    /// 删除网络配置
    pub fn delete_network(&self, network_name: &str) -> Result<()> {
        Self::validate_network_name(network_name)?;
        let network_dir = self.get_network_dir(network_name);

        if network_dir.exists() {
            std::fs::remove_dir_all(&network_dir)
                .with_context(|| format!("无法删除网络配置目录: {}", network_dir.display()))?;
        }

        // 更新网络列表
        let mut list = self.load_network_list()?;
        list.networks.remove(network_name);

        // 如果删除的是默认网络，清除默认设置
        if list.default_network.as_deref() == Some(network_name) {
            list.default_network = None;
        }

        self.save_network_list(&list)?;

        Ok(())
    }

    /// 列出所有网络配置
    pub fn list_networks(&self) -> Result<Vec<NetworkConfig>> {
        let list = self.load_network_list()?;
        let mut networks: Vec<_> = list.networks.into_values().collect();

        // 按最后使用时间排序
        networks.sort_by(|a, b| b.last_used.cmp(&a.last_used));

        Ok(networks)
    }

    /// 获取默认网络名称
    pub fn get_default_network(&self) -> Result<Option<String>> {
        let list = self.load_network_list()?;
        Ok(list.default_network)
    }

    /// 设置默认网络
    pub fn set_default_network(&self, network_name: &str) -> Result<()> {
        Self::validate_network_name(network_name)?;
        let mut list = self.load_network_list()?;

        if !list.networks.contains_key(network_name) {
            anyhow::bail!("网络配置不存在: {}", network_name);
        }

        list.default_network = Some(network_name.to_string());
        self.save_network_list(&list)?;

        Ok(())
    }

    /// 检查网络是否存在
    pub fn network_exists(&self, network_name: &str) -> bool {
        if Self::validate_network_name(network_name).is_err() {
            return false;
        }

        if let Ok(list) = self.load_network_list() {
            list.networks.contains_key(network_name)
        } else {
            false
        }
    }

    /// 从URL提取网络名称
    pub fn extract_network_name(url: &str) -> String {
        // 从URL中提取域名作为网络名称
        // 例如: ws://example.com:6666 -> example
        //      wss://vpn.example.com -> vpn.example

        let url = url.trim_start_matches("ws://").trim_start_matches("wss://");

        let domain = if let Some(rest) = url.strip_prefix('[') {
            rest.split(']').next().unwrap_or("ipv6")
        } else {
            url.split(':').next().unwrap_or(url)
        };
        let domain = url_to_network_slug(domain.split('/').next().unwrap_or(domain));

        // 移除顶级域名，保留主域名和子域名
        let parts: Vec<&str> = domain.split('.').collect();
        if parts.len() > 2 {
            parts[..parts.len() - 1].join(".")
        } else if !parts.is_empty() {
            parts[0].to_string()
        } else {
            "default".to_string()
        }
    }
}

fn normalize_display_name(display_name: Option<String>) -> Result<Option<String>> {
    let Some(display_name) = display_name else {
        return Ok(None);
    };
    let display_name = display_name.trim();
    if display_name.is_empty() {
        return Ok(None);
    }
    if display_name.chars().any(char::is_control) {
        anyhow::bail!("节点显示名不能包含控制字符");
    }
    Ok(Some(display_name.to_string()))
}

fn url_to_network_slug(value: &str) -> String {
    let mut slug = String::new();

    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
            slug.push(ch);
        } else if matches!(ch, ':' | ' ' | '\t') && !slug.ends_with('-') {
            slug.push('-');
        }
    }

    let slug = slug.trim_matches(['.', '-', '_']).to_string();
    if slug.is_empty() {
        "default".to_string()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config_dir(name: &str) -> PathBuf {
        let unique = format!("fuckipv6-config-manager-{}-{}", name, std::process::id());
        std::env::temp_dir().join(unique)
    }

    #[test]
    fn manages_multiple_networks_and_switches_default() -> anyhow::Result<()> {
        let dir = temp_config_dir("switch");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;

        let alpha = manager.create_network("alpha", "ws://alpha.example:6666", true)?;
        let beta = manager.create_network("beta", "wss://beta.example:6666", false)?;

        assert!(alpha.ends_with("alpha/config.toml"));
        assert!(beta.ends_with("beta/config.toml"));
        assert_eq!(manager.get_default_network()?, Some("alpha".to_string()));

        manager.set_default_network("beta")?;
        assert_eq!(manager.get_default_network()?, Some("beta".to_string()));
        assert_eq!(manager.get_network_config("beta")?, beta);

        let networks = manager.list_networks()?;
        assert_eq!(networks.len(), 2);
        assert!(networks.iter().any(|network| network.name == "alpha"));
        assert!(networks.iter().any(|network| network.name == "beta"));

        manager.delete_network("beta")?;
        assert_eq!(manager.get_default_network()?, None);
        assert!(!manager.network_exists("beta"));

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn reads_legacy_network_list_without_new_optional_fields() -> anyhow::Result<()> {
        let dir = temp_config_dir("legacy-network-list");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;
        std::fs::write(
            dir.join("networks.json"),
            r#"{
                "networks": {
                    "legacy": {
                        "name": "legacy",
                        "coordinator_url": "ws://legacy.example:6666",
                        "created_at": "2025-01-01T00:00:00Z",
                        "last_used": "2025-01-01T00:00:00Z"
                    }
                },
                "default_network": "legacy"
            }"#,
        )?;

        let networks = manager.list_networks()?;
        let legacy = networks
            .iter()
            .find(|network| network.name == "legacy")
            .expect("legacy network");
        assert!(!legacy.owns_embedded_services);
        assert_eq!(legacy.signaling_port, None);
        assert_eq!(legacy.relay_port, None);
        assert_eq!(legacy.external_host, None);
        assert_eq!(legacy.ip_pool, None);
        assert_eq!(manager.get_default_network()?, Some("legacy".into()));

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn rejects_duplicate_and_unsafe_network_names() -> anyhow::Result<()> {
        let dir = temp_config_dir("reject");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;

        manager.create_network("prod", "ws://prod.example:6666", true)?;
        assert!(manager
            .create_network("prod", "ws://other.example:6666", false)
            .is_err());
        assert!(manager
            .create_network("../escape", "ws://escape.example:6666", false)
            .is_err());
        assert!(!manager.network_exists("../escape"));

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn creates_client_config_when_coordinator_state_directory_exists() -> anyhow::Result<()> {
        let dir = temp_config_dir("coordinator-dir");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;
        let coordinator_config = manager.coordinator_config_path("hosted")?;
        std::fs::create_dir_all(coordinator_config.parent().expect("coordinator parent"))?;
        std::fs::write(&coordinator_config, "persisted coordinator state")?;

        let config = manager.create_network("hosted", "ws://127.0.0.1:6666", true)?;

        assert!(config.exists());
        assert!(coordinator_config.exists());
        assert!(manager.network_exists("hosted"));

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn extracts_safe_network_names_from_urls() {
        assert_eq!(
            ConfigManager::extract_network_name("ws://vpn.example.com:6666"),
            "vpn.example"
        );
        assert_eq!(
            ConfigManager::extract_network_name("wss://example.com/connect"),
            "example"
        );
        assert_eq!(
            ConfigManager::extract_network_name("ws://[2001:db8::1]:6666"),
            "2001-db8-1"
        );
    }

    #[test]
    fn service_config_override_wins_over_desktop_environment() -> anyhow::Result<()> {
        let resolved = ConfigManager::resolve_config_dir(
            Some(OsString::from("/etc/fuckipv6")),
            Some(OsString::from("/home/operator/.config")),
            Some(OsString::from("/home/operator")),
        )?;
        assert_eq!(resolved, PathBuf::from("/etc/fuckipv6"));
        Ok(())
    }

    #[test]
    fn node_identity_and_display_name_are_stable_and_node_wide() -> anyhow::Result<()> {
        let dir = temp_config_dir("identity");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;

        let first = manager.load_node_identity()?;
        let first_id = first.client_id().to_string();
        let first_fingerprint = Identity::public_key_fingerprint(&first.public_key_bytes());
        assert!(manager.node_identity_path().exists());
        assert_eq!(manager.node_display_name()?, None);

        manager.set_node_display_name(Some("  edge-gateway  "))?;
        assert_eq!(manager.node_display_name()?, Some("edge-gateway".into()));

        let reopened = ConfigManager::new_for_dir(dir.clone())?;
        let second = reopened.load_node_identity()?;
        assert_eq!(second.client_id(), first_id);
        assert_eq!(
            Identity::public_key_fingerprint(&second.public_key_bytes()),
            first_fingerprint
        );
        assert_eq!(reopened.node_display_name()?, Some("edge-gateway".into()));

        let config_path = reopened.create_network("home", "ws://home.example:6666", true)?;
        let network_config = ClientConfig::from_file(config_path)?;
        assert_eq!(
            network_config.private_key_path,
            reopened.node_identity_path()
        );

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn node_display_name_rejects_control_characters_and_can_be_cleared() -> anyhow::Result<()> {
        let dir = temp_config_dir("display-name");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;

        assert!(manager.set_node_display_name(Some("bad\nname")).is_err());
        manager.set_node_display_name(Some("gateway"))?;
        manager.set_node_display_name(Some("   "))?;
        assert_eq!(manager.node_display_name()?, None);

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn language_preference_defaults_for_legacy_metadata_and_persists() -> anyhow::Result<()> {
        let dir = temp_config_dir("language-preference");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;
        std::fs::write(
            dir.join(NODE_METADATA_FILE),
            r#"{"display_name":"legacy-node"}"#,
        )?;

        assert_eq!(
            manager.node_language_preference()?,
            LanguagePreference::System
        );
        manager.set_node_display_name(Some("gateway"))?;
        manager.set_node_language_preference(LanguagePreference::ZhCn)?;

        let reopened = ConfigManager::new_for_dir(dir.clone())?;
        assert_eq!(reopened.node_display_name()?, Some("gateway".into()));
        assert_eq!(
            reopened.node_language_preference()?,
            LanguagePreference::ZhCn
        );

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }
}
