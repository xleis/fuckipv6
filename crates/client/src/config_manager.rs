use anyhow::{Context, Result};
use fuckipv6_common::{ClientConfig, Identity};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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

    #[cfg(test)]
    pub(crate) fn new_for_dir(config_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&config_dir).context("无法创建配置目录")?;
        Ok(Self { config_dir })
    }

    /// 获取配置目录
    fn get_config_dir() -> Result<PathBuf> {
        // 优先使用 $XDG_CONFIG_HOME/fuckipv6
        if let Ok(xdg_config) = std::env::var("XDG_CONFIG_HOME") {
            return Ok(PathBuf::from(xdg_config).join("fuckipv6"));
        }

        // 其次使用 $HOME/.config/fuckipv6
        if let Ok(home) = std::env::var("HOME") {
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

    /// Load or create the node-wide signing identity.
    pub(crate) fn load_node_identity(&self) -> Result<Identity> {
        Identity::load_or_generate(self.node_identity_path()).context("无法加载节点身份")
    }

    /// Load the optional display name from node-wide metadata.
    pub(crate) fn node_display_name(&self) -> Result<Option<String>> {
        let metadata_path = self.config_dir.join(NODE_METADATA_FILE);
        if !metadata_path.exists() {
            return Ok(None);
        }

        let content = std::fs::read_to_string(&metadata_path)
            .with_context(|| format!("无法读取节点元数据: {}", metadata_path.display()))?;
        let metadata: NodeMetadata = serde_json::from_str(&content)
            .with_context(|| format!("无法解析节点元数据: {}", metadata_path.display()))?;
        normalize_display_name(metadata.display_name)
    }

    /// Persist a node-wide display name. Empty or whitespace-only values clear it.
    #[cfg(test)]
    pub(crate) fn set_node_display_name(&self, display_name: Option<&str>) -> Result<()> {
        let metadata = NodeMetadata {
            display_name: normalize_display_name(display_name.map(str::to_string))?,
        };
        let metadata_path = self.config_dir.join(NODE_METADATA_FILE);
        let content = serde_json::to_string_pretty(&metadata).context("无法序列化节点元数据")?;
        std::fs::write(&metadata_path, content)
            .with_context(|| format!("无法写入节点元数据: {}", metadata_path.display()))?;
        Ok(())
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
        std::fs::write(&networks_file, content)
            .with_context(|| format!("无法写入网络列表: {}", networks_file.display()))?;
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
        let mut list = self.load_network_list()?;

        if list.networks.contains_key(network_name) || network_dir.exists() {
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

        let config_file = network_dir.join("config.toml");
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
            },
        );

        if set_default || list.default_network.is_none() {
            list.default_network = Some(network_name.to_string());
        }

        self.save_network_list(&list)?;

        Ok(config_file)
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
}
