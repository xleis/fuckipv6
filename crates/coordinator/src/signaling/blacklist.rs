use anyhow::{Context, Result};
use fuckipv6_common::{BlacklistConfig, CoordinatorConfig};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 黑名单持久化边界。
///
/// 实现必须在 `save` 返回成功后保证新状态已经持久化；失败时不得修改目标存储。
pub trait BlacklistStore: Send + Sync {
    fn load(&self) -> Result<BlacklistConfig>;
    fn save(&self, blacklist: &BlacklistConfig) -> Result<()>;
}

/// 将协调端黑名单持久化到 `CoordinatorConfig` 的 `blacklist` 字段。
#[derive(Debug, Clone)]
pub struct CoordinatorConfigBlacklistStore {
    path: PathBuf,
}

impl CoordinatorConfigBlacklistStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl BlacklistStore for CoordinatorConfigBlacklistStore {
    fn load(&self) -> Result<BlacklistConfig> {
        CoordinatorConfig::from_file(&self.path)
            .map(|config| config.blacklist)
            .with_context(|| format!("读取协调端持久黑名单失败: {}", self.path.to_string_lossy()))
    }

    fn save(&self, blacklist: &BlacklistConfig) -> Result<()> {
        let mut config = CoordinatorConfig::from_file(&self.path).with_context(|| {
            format!(
                "更新持久黑名单前读取协调端配置失败: {}",
                self.path.to_string_lossy()
            )
        })?;
        if config.blacklist == *blacklist {
            return Ok(());
        }
        config.blacklist = blacklist.clone();
        config.validate().context("持久黑名单配置校验失败")?;
        config
            .save_to_file(&self.path)
            .with_context(|| format!("写入协调端持久黑名单失败: {}", self.path.to_string_lossy()))
    }
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
