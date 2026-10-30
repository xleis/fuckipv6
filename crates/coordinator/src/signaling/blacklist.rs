use anyhow::{bail, Context, Result};
use fuckipv6_common::{BlacklistConfig, CoordinatorConfig, NetworkAuthorizationRevocation};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// 黑名单持久化边界。
///
/// 实现必须在 `save` 返回成功后保证新状态已经持久化；失败时不得修改目标存储。
pub trait BlacklistStore: Send + Sync {
    fn load(&self) -> Result<BlacklistConfig>;
    fn save(&self, blacklist: &BlacklistConfig) -> Result<()>;

    /// 加载持久化撤销目录。调用方必须把读取错误视为拒绝，而不能当作空目录。
    fn load_authorization_revocations(&self) -> Result<Vec<NetworkAuthorizationRevocation>> {
        bail!("当前持久化存储不支持授权撤销目录")
    }

    /// 原子地新增一条网络作用域撤销记录。
    fn revoke_authorization(&self, _record: &NetworkAuthorizationRevocation) -> Result<bool> {
        bail!("当前持久化存储不支持授权撤销")
    }

    /// 原子地移除指定网络中的撤销记录。
    fn lift_authorization(
        &self,
        _network_id: &str,
        _client_id: &str,
        _public_key_fingerprint: &str,
    ) -> Result<bool> {
        bail!("当前持久化存储不支持授权撤销")
    }

    /// 当前协调器配置所属的持久网络作用域。
    ///
    /// 具体存储应覆盖此方法并返回重启后稳定的网络标识。
    fn authorization_network_id(&self) -> String {
        "default".to_string()
    }
}

/// 将协调端黑名单持久化到 `CoordinatorConfig` 的 `blacklist` 字段。
#[derive(Debug, Clone)]
pub struct CoordinatorConfigBlacklistStore {
    path: PathBuf,
    default_config: CoordinatorConfig,
    update_lock: Arc<Mutex<()>>,
}

impl CoordinatorConfigBlacklistStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            default_config: CoordinatorConfig::default(),
            update_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn with_default_config(
        path: impl Into<PathBuf>,
        default_config: CoordinatorConfig,
    ) -> Self {
        Self {
            path: path.into(),
            default_config,
            update_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn load_config(&self) -> Result<CoordinatorConfig> {
        match CoordinatorConfig::from_file(&self.path) {
            Ok(config) => Ok(config),
            Err(error) if is_not_found(&error) => Ok(self.default_config.clone()),
            Err(error) => Err(error).with_context(|| {
                format!("读取协调端持久配置失败: {}", self.path.to_string_lossy())
            }),
        }
    }

    fn save_config(&self, config: &CoordinatorConfig) -> Result<()> {
        config.validate().context("持久协调端配置校验失败")?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建协调端配置目录失败: {}", parent.to_string_lossy()))?;
        }
        config
            .save_to_file(&self.path)
            .with_context(|| format!("写入协调端持久配置失败: {}", self.path.to_string_lossy()))
    }

    fn update_guard(&self) -> Result<std::sync::MutexGuard<'_, ()>> {
        self.update_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("协调端配置写入锁已损坏"))
    }
}

impl BlacklistStore for CoordinatorConfigBlacklistStore {
    fn load(&self) -> Result<BlacklistConfig> {
        self.load_config()
            .map(|config| config.blacklist)
            .with_context(|| format!("读取协调端持久黑名单失败: {}", self.path.to_string_lossy()))
    }

    fn save(&self, blacklist: &BlacklistConfig) -> Result<()> {
        let _guard = self.update_guard()?;
        let mut config = self.load_config().with_context(|| {
            format!(
                "更新持久黑名单前读取协调端配置失败: {}",
                self.path.to_string_lossy()
            )
        })?;
        if config.blacklist == *blacklist {
            return Ok(());
        }
        config.blacklist = blacklist.clone();
        self.save_config(&config).context("持久黑名单配置写入失败")
    }

    fn load_authorization_revocations(&self) -> Result<Vec<NetworkAuthorizationRevocation>> {
        self.load_config()
            .map(|config| config.authorization_revocations)
            .with_context(|| {
                format!(
                    "读取协调端持久授权撤销目录失败: {}",
                    self.path.to_string_lossy()
                )
            })
    }

    fn revoke_authorization(&self, record: &NetworkAuthorizationRevocation) -> Result<bool> {
        record.validate()?;
        let _guard = self.update_guard()?;
        let mut config = self.load_config().with_context(|| {
            format!(
                "更新授权撤销目录前读取协调端配置失败: {}",
                self.path.to_string_lossy()
            )
        })?;

        let already_revoked = config.authorization_revocations.iter().any(|existing| {
            existing.network_id == record.network_id
                && (existing.client_id == record.client_id
                    || existing.public_key_fingerprint == record.public_key_fingerprint)
        });
        if already_revoked {
            return Ok(false);
        }

        config.authorization_revocations.push(record.clone());
        config.authorization_revocations.sort_by(|left, right| {
            (
                &left.network_id,
                &left.client_id,
                &left.public_key_fingerprint,
            )
                .cmp(&(
                    &right.network_id,
                    &right.client_id,
                    &right.public_key_fingerprint,
                ))
        });
        config.authorization_revocations.dedup();
        self.save_config(&config)
            .context("持久授权撤销目录写入失败")?;
        Ok(true)
    }

    fn lift_authorization(
        &self,
        network_id: &str,
        client_id: &str,
        public_key_fingerprint: &str,
    ) -> Result<bool> {
        let _guard = self.update_guard()?;
        let mut config = self.load_config().with_context(|| {
            format!(
                "移除授权撤销记录前读取协调端配置失败: {}",
                self.path.to_string_lossy()
            )
        })?;
        let original_len = config.authorization_revocations.len();
        config.authorization_revocations.retain(|record| {
            !(record.network_id == network_id
                && (record.client_id == client_id
                    || record.public_key_fingerprint == public_key_fingerprint))
        });
        if config.authorization_revocations.len() == original_len {
            return Ok(false);
        }

        self.save_config(&config)
            .context("持久授权撤销目录写入失败")?;
        Ok(true)
    }

    fn authorization_network_id(&self) -> String {
        let digest = Sha256::digest(self.path.to_string_lossy().as_bytes());
        let encoded = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        format!("coordinator-config:{encoded}")
    }
}

fn is_not_found(error: &anyhow::Error) -> bool {
    error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
        .any(|error| error.kind() == std::io::ErrorKind::NotFound)
}

/// 为需要共享同一存储实例的调用方提供类型擦除。
pub type SharedBlacklistStore = Arc<dyn BlacklistStore>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signaling::session::SessionManager;
    use fuckipv6_common::{CoordinatorAuthConfig, LogConfig, NetworkAllocation};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEST_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_config_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let sequence = TEST_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "fuckipv6-{name}-{}-{unique}-{sequence}.toml",
            std::process::id()
        ))
    }

    fn coordinator_config() -> CoordinatorConfig {
        CoordinatorConfig {
            signaling_bind: "[::]:6666".to_string(),
            relay_bind: "[::]:6667".to_string(),
            webui_bind: "127.0.0.1:8080".to_string(),
            heartbeat_timeout_secs: 120,
            network: NetworkAllocation::default(),
            tls: None,
            log: LogConfig::default(),
            auth: CoordinatorAuthConfig::default(),
            blacklist: BlacklistConfig::default(),
            authorization_revocations: Vec::new(),
            relay_endpoints: Vec::new(),
        }
    }

    #[test]
    fn config_store_round_trips_revocations_and_preserves_other_config() -> Result<()> {
        let path = test_config_path("blacklist-roundtrip");
        coordinator_config().save_to_file(&path)?;
        let store = CoordinatorConfigBlacklistStore::new(&path);

        assert!(store.load()?.banned_clients.is_empty());
        let blacklist = BlacklistConfig {
            banned_clients: vec!["node-bad".to_string(), "ABCDEF012345".to_string()],
        };
        store.save(&blacklist)?;
        assert_eq!(store.load()?, blacklist);
        assert_eq!(
            CoordinatorConfig::from_file(&path)?.network,
            NetworkAllocation::default()
        );

        std::fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn missing_config_is_created_on_first_blacklist_save() -> Result<()> {
        let path = test_config_path("blacklist-missing").join("coordinator.toml");
        let mut default_config = coordinator_config();
        default_config.network.ipv6_pool = "fd42::1000/120".to_string();
        default_config.network.ip_pool = Some("fd42::1000/120".to_string());
        let store = CoordinatorConfigBlacklistStore::with_default_config(&path, default_config);

        assert!(store.load()?.banned_clients.is_empty());
        store.save(&BlacklistConfig {
            banned_clients: vec!["node-bad".to_string()],
        })?;

        let persisted = CoordinatorConfig::from_file(&path)?;
        assert_eq!(persisted.network.effective_ipv6_pool(), "fd42::1000/120");
        assert_eq!(persisted.blacklist.banned_clients, vec!["node-bad"]);

        std::fs::remove_dir_all(path.parent().expect("test config parent"))?;
        Ok(())
    }

    #[tokio::test]
    async fn session_manager_reloads_persisted_ban_after_restart() -> Result<()> {
        let path = test_config_path("blacklist-session-restart");
        coordinator_config().save_to_file(&path)?;

        let first = SessionManager::new("fd00::1000/120")?;
        first
            .attach_blacklist_store(Arc::new(CoordinatorConfigBlacklistStore::new(&path)))
            .await?;
        assert!(first.try_ban_client("node-bad".to_string()).await?);

        let second = SessionManager::new("fd00::1000/120")?;
        second
            .attach_blacklist_store(Arc::new(CoordinatorConfigBlacklistStore::new(&path)))
            .await?;
        assert_eq!(second.get_blacklist().await, vec!["node-bad".to_string()]);
        assert!(second.is_blacklisted("node-bad", &[7; 32]).await);
        assert!(second.try_unban_client("node-bad").await?);

        let third = SessionManager::new("fd00::1000/120")?;
        third
            .attach_blacklist_store(Arc::new(CoordinatorConfigBlacklistStore::new(&path)))
            .await?;
        assert!(third.get_blacklist().await.is_empty());

        std::fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn malformed_config_is_not_overwritten_by_blacklist_save() -> Result<()> {
        let path = test_config_path("blacklist-malformed");
        let original = b"this is not valid = toml";
        std::fs::write(&path, original)?;
        let store = CoordinatorConfigBlacklistStore::new(&path);

        assert!(store
            .save(&BlacklistConfig {
                banned_clients: vec!["node-bad".to_string()],
            })
            .is_err());
        assert_eq!(std::fs::read(&path)?, original);

        std::fs::remove_file(path)?;
        Ok(())
    }
}
