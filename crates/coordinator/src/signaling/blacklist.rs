use anyhow::{bail, Context, Result};
use fs2::FileExt;
use fuckipv6_common::{BlacklistConfig, CoordinatorConfig, NetworkAuthorizationRevocation};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// 黑名单持久化边界。
///
/// 实现必须在 `save` 返回成功后保证新状态已经持久化；失败时不得修改目标存储。
pub trait BlacklistStore: Send + Sync {
    fn load(&self) -> Result<BlacklistConfig>;
    fn save(&self, blacklist: &BlacklistConfig) -> Result<()>;

    /// 是否实现网络作用域授权撤销。未实现的存储仍可用于黑名单，但会话注册
    /// 必须因缺少撤销目录而失败关闭。
    fn supports_authorization_revocations(&self) -> bool {
        false
    }

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

    /// 返回当前协调器配置所属的持久网络作用域。
    ///
    /// 该方法必须成功初始化或读取稳定网络标识；不支持授权撤销的存储返回错误，
    /// 注册路径必须失败关闭。
    fn authorization_network_id(&self) -> Result<String> {
        bail!("当前持久化存储不支持稳定网络标识")
    }
}

/// 将协调端黑名单和授权撤销持久化到同一份 `CoordinatorConfig`。
///
/// 进程内使用 `update_lock` 串行化，进程间使用配置旁路锁文件的排他锁串行化
/// 完整读-改-写事务。写入由 `CoordinatorConfig::save_to_file` 原子替换。
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

    fn load_config_or_default(&self) -> Result<CoordinatorConfig> {
        match CoordinatorConfig::from_file(&self.path) {
            Ok(config) => Ok(config),
            Err(error) if is_not_found(&error) => Ok(self.default_config.clone()),
            Err(error) => Err(error).with_context(|| {
                format!("读取协调端持久配置失败: {}", self.path.to_string_lossy())
            }),
        }
    }

    fn load_existing_config(&self) -> Result<CoordinatorConfig> {
        match CoordinatorConfig::from_file(&self.path) {
            Ok(config) => Ok(config),
            Err(error) if is_not_found(&error) => bail!(
                "协调端持久配置不存在，拒绝访问授权撤销目录: {}",
                self.path.to_string_lossy()
            ),
            Err(error) => Err(error).with_context(|| {
                format!("读取协调端持久配置失败: {}", self.path.to_string_lossy())
            }),
        }
    }

    fn save_config(&self, config: &CoordinatorConfig) -> Result<()> {
        config.validate().context("持久协调端配置校验失败")?;
        if let Some(parent) = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
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

    fn lock_path(&self) -> PathBuf {
        let file_name = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "coordinator.toml".to_string());
        self.path
            .with_file_name(format!(".{file_name}.revocation.lock"))
    }

    fn open_lock_file(&self) -> Result<File> {
        if let Some(parent) = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建协调端配置目录失败: {}", parent.to_string_lossy()))?;
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(self.lock_path()).with_context(|| {
            format!(
                "打开协调端配置锁文件失败: {}",
                self.lock_path().to_string_lossy()
            )
        })
    }

    fn with_update_lock<T>(&self, operation: impl FnOnce() -> Result<T>) -> Result<T> {
        let _process_guard = self.update_guard()?;
        let lock_file = self.open_lock_file()?;
        FileExt::lock_exclusive(&lock_file).with_context(|| {
            format!(
                "获取协调端配置排他锁失败: {}",
                self.lock_path().to_string_lossy()
            )
        })?;

        let result = operation();
        let unlock_result = FileExt::unlock(&lock_file);
        match (result, unlock_result) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error).with_context(|| {
                format!(
                    "释放协调端配置排他锁失败: {}",
                    self.lock_path().to_string_lossy()
                )
            }),
        }
    }

    /// 在配置排他锁内生成或读取稳定网络标识，并迁移旧版路径哈希记录。
    ///
    /// 旧实现把配置路径哈希写进撤销记录。若配置中只有一个旧标识，可安全地把
    /// 它迁移到新生成的随机稳定标识（配置移动后仍可兼容）；若存在多个旧标识，
    /// 仅迁移与当前路径精确匹配的标识，否则失败关闭，禁止把其他网络的撤销记录
    /// 合并进当前网络。
    fn initialize_network_identity_locked(
        &self,
        allow_missing: bool,
    ) -> Result<(CoordinatorConfig, String)> {
        let existed = self.path.exists();
        let mut config = if allow_missing && !existed {
            self.load_config_or_default()?
        } else {
            self.load_existing_config()?
        };
        let mut changed = false;

        if config.network_id.trim().is_empty() {
            config.network_id = generate_network_id()?;
            changed = true;
        }
        let stable_network_id = config.network_id.clone();

        // 只在首次引入稳定标识时迁移旧路径哈希。已经写入稳定标识的配置不再
        // 解释旧记录，避免后续把其他网络的旧数据纳入当前目录。
        if !existed || changed {
            let legacy_ids = config
                .authorization_revocations
                .iter()
                .filter(|record| is_legacy_path_network_id(&record.network_id))
                .map(|record| record.network_id.clone())
                .collect::<BTreeSet<_>>();

            if !legacy_ids.is_empty() {
                let current_path_legacy_id = legacy_path_network_id(&self.path);
                let migration_id = if legacy_ids.len() == 1 {
                    legacy_ids.iter().next().cloned()
                } else if legacy_ids.contains(&current_path_legacy_id) {
                    Some(current_path_legacy_id)
                } else {
                    bail!(
                        "协调端配置包含多个无法判定的旧网络标识，拒绝迁移授权撤销记录: {}",
                        self.path.to_string_lossy()
                    );
                };

                if let Some(migration_id) = migration_id {
                    for revocation in &mut config.authorization_revocations {
                        if revocation.network_id == migration_id {
                            revocation.network_id = stable_network_id.clone();
                            changed = true;
                        }
                    }
                }
            }
        }
        if changed {
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
        }

        if changed || !existed {
            self.save_config(&config)
                .context("初始化协调端稳定网络标识失败")?;
        }
        Ok((config, stable_network_id))
    }

    fn initialize_network_identity(&self) -> Result<(CoordinatorConfig, String)> {
        self.with_update_lock(|| self.initialize_network_identity_locked(false))
    }
}

impl BlacklistStore for CoordinatorConfigBlacklistStore {
    fn supports_authorization_revocations(&self) -> bool {
        true
    }

    fn load(&self) -> Result<BlacklistConfig> {
        self.load_config_or_default()
            .map(|config| config.blacklist)
            .with_context(|| format!("读取协调端持久黑名单失败: {}", self.path.to_string_lossy()))
    }

    fn save(&self, blacklist: &BlacklistConfig) -> Result<()> {
        self.with_update_lock(|| {
            let (mut config, _) = self.initialize_network_identity_locked(true)?;
            if config.blacklist == *blacklist {
                return Ok(());
            }
            config.blacklist = blacklist.clone();
            self.save_config(&config).context("持久黑名单配置写入失败")
        })
    }

    fn load_authorization_revocations(&self) -> Result<Vec<NetworkAuthorizationRevocation>> {
        self.load_existing_config()
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
        self.with_update_lock(|| {
            if !self.path.exists() {
                bail!(
                    "协调端持久配置不存在，拒绝写入授权撤销记录: {}",
                    self.path.to_string_lossy()
                );
            }
            let (mut config, _) = self.initialize_network_identity_locked(false)?;
            if record.network_id != config.network_id {
                bail!(
                    "拒绝把网络 {} 的授权撤销写入当前网络 {}",
                    record.network_id,
                    config.network_id
                );
            }
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
        })
    }

    fn lift_authorization(
        &self,
        network_id: &str,
        client_id: &str,
        public_key_fingerprint: &str,
    ) -> Result<bool> {
        self.with_update_lock(|| {
            if !self.path.exists() {
                bail!(
                    "协调端持久配置不存在，拒绝移除授权撤销记录: {}",
                    self.path.to_string_lossy()
                );
            }
            let (mut config, _) = self.initialize_network_identity_locked(false)?;
            if network_id != config.network_id {
                bail!(
                    "拒绝移除其他网络 {} 的授权撤销记录（当前网络 {}）",
                    network_id,
                    config.network_id
                );
            }
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
        })
    }

    fn authorization_network_id(&self) -> Result<String> {
        self.initialize_network_identity().map(|(_, id)| id)
    }
}

fn generate_network_id() -> Result<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).context("生成协调器稳定网络标识失败")?;
    let encoded = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("network-{encoded}"))
}

fn legacy_path_network_id(path: &Path) -> String {
    let digest = Sha256::digest(path.to_string_lossy().as_bytes());
    let encoded = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("coordinator-config:{encoded}")
}

fn is_legacy_path_network_id(network_id: &str) -> bool {
    network_id
        .strip_prefix("coordinator-config:")
        .is_some_and(|digest| {
            digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
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
            network_id: String::new(),
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

    #[tokio::test]
    async fn stable_network_id_survives_config_move_and_migrates_legacy_records() -> Result<()> {
        let source = test_config_path("stable-network-source");
        let moved = test_config_path("stable-network-moved");
        let identity = fuckipv6_common::Identity::generate()?;
        let legacy_network_id = legacy_path_network_id(&source);
        let mut config = coordinator_config();
        config.authorization_revocations = vec![NetworkAuthorizationRevocation {
            network_id: legacy_network_id.clone(),
            client_id: identity.client_id().to_string(),
            public_key_fingerprint: fuckipv6_common::Identity::public_key_fingerprint(
                &identity.public_key(),
            ),
        }];
        config.save_to_file(&source)?;
        assert!(CoordinatorConfig::from_file(&source)?.network_id.is_empty());

        let source_store = CoordinatorConfigBlacklistStore::new(&source);
        let stable_network_id = source_store.authorization_network_id()?;
        assert_ne!(stable_network_id, legacy_network_id);
        let migrated = CoordinatorConfig::from_file(&source)?;
        assert_eq!(migrated.network_id, stable_network_id);
        assert_eq!(
            migrated.authorization_revocations[0].network_id,
            stable_network_id
        );
        let directory = crate::signaling::revocation::NetworkAuthorizationDirectory::new(
            stable_network_id.clone(),
            Arc::new(source_store) as SharedBlacklistStore,
        )?;
        assert!(
            directory
                .is_revoked(identity.client_id(), &identity.public_key())
                .await?
        );

        std::fs::rename(&source, &moved)?;
        let moved_store = CoordinatorConfigBlacklistStore::new(&moved);
        assert_eq!(moved_store.authorization_network_id()?, stable_network_id);

        std::fs::remove_file(&moved)?;
        let _ = std::fs::remove_file(moved_store.lock_path());
        let _ = std::fs::remove_file(source.with_file_name(".coordinator.toml.revocation.lock"));
        Ok(())
    }

    #[test]
    fn independently_initialized_networks_receive_distinct_stable_ids() -> Result<()> {
        let first_path = test_config_path("stable-network-first");
        let second_path = test_config_path("stable-network-second");
        coordinator_config().save_to_file(&first_path)?;
        coordinator_config().save_to_file(&second_path)?;
        let first = CoordinatorConfigBlacklistStore::new(&first_path);
        let second = CoordinatorConfigBlacklistStore::new(&second_path);

        let first_id = first.authorization_network_id()?;
        let second_id = second.authorization_network_id()?;
        assert_ne!(first_id, second_id);
        assert!(!first_id.starts_with("coordinator-config:"));

        std::fs::remove_file(&first_path)?;
        std::fs::remove_file(&second_path)?;
        let _ = std::fs::remove_file(first.lock_path());
        let _ = std::fs::remove_file(second.lock_path());
        Ok(())
    }

    #[test]
    fn missing_config_never_bootstraps_an_authorization_network_identity() -> Result<()> {
        let path = test_config_path("missing-authorization-config");
        let store = CoordinatorConfigBlacklistStore::new(&path);

        let error = store
            .authorization_network_id()
            .expect_err("a missing config must fail closed for authorization");
        assert!(format!("{error:#}").contains("持久配置不存在"));
        assert!(
            !path.exists(),
            "authorization lookup must not create a config"
        );
        Ok(())
    }

    #[test]
    fn legacy_migration_does_not_reuse_an_unrelated_network_identifier() -> Result<()> {
        let path = test_config_path("legacy-migration-isolation");
        let identity = fuckipv6_common::Identity::generate()?;
        let current_legacy_id = legacy_path_network_id(&path);
        let unrelated_legacy_id = legacy_path_network_id(Path::new("/another/network.toml"));
        let mut config = coordinator_config();
        config.authorization_revocations = vec![
            NetworkAuthorizationRevocation {
                network_id: current_legacy_id,
                client_id: identity.client_id().to_string(),
                public_key_fingerprint: fuckipv6_common::Identity::public_key_fingerprint(
                    &identity.public_key(),
                ),
            },
            NetworkAuthorizationRevocation {
                network_id: unrelated_legacy_id.clone(),
                client_id: "unrelated-node".to_string(),
                public_key_fingerprint: "00".repeat(32),
            },
        ];
        config.save_to_file(&path)?;
        let store = CoordinatorConfigBlacklistStore::new(&path);

        let stable_network_id = store.authorization_network_id()?;
        let persisted = CoordinatorConfig::from_file(&path)?;
        assert!(persisted
            .authorization_revocations
            .iter()
            .any(|record| record.network_id == stable_network_id));
        assert!(persisted
            .authorization_revocations
            .iter()
            .any(|record| record.network_id == unrelated_legacy_id));
        assert!(!persisted
            .authorization_revocations
            .iter()
            .any(|record| record.network_id == stable_network_id
                && record.client_id == "unrelated-node"));

        std::fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn ambiguous_legacy_network_identifiers_fail_without_rewriting_config() -> Result<()> {
        let path = test_config_path("legacy-migration-ambiguous");
        let mut config = coordinator_config();
        config.authorization_revocations = vec![
            NetworkAuthorizationRevocation {
                network_id: legacy_path_network_id(Path::new("/network/one.toml")),
                client_id: "node-one".to_string(),
                public_key_fingerprint: "11".repeat(32),
            },
            NetworkAuthorizationRevocation {
                network_id: legacy_path_network_id(Path::new("/network/two.toml")),
                client_id: "node-two".to_string(),
                public_key_fingerprint: "22".repeat(32),
            },
        ];
        config.save_to_file(&path)?;
        let original = std::fs::read(&path)?;
        let store = CoordinatorConfigBlacklistStore::new(&path);

        let error = store
            .authorization_network_id()
            .expect_err("ambiguous legacy records must fail closed");
        assert!(error.to_string().contains("多个无法判定的旧网络标识"));
        assert_eq!(std::fs::read(&path)?, original);

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

    #[tokio::test]
    async fn deleting_config_after_attach_makes_registration_fail_closed() -> Result<()> {
        let path = test_config_path("deleted-authorization-config");
        coordinator_config().save_to_file(&path)?;
        let manager = SessionManager::new("fd00::1000/120")?;
        let store = CoordinatorConfigBlacklistStore::new(&path);
        manager
            .attach_blacklist_store(Arc::new(store.clone()))
            .await?;

        std::fs::remove_file(&path)?;
        let identity = fuckipv6_common::Identity::generate()?;
        let client_id = identity.client_id().to_string();
        let public_key = identity.public_key();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let error = manager
            .register_client(
                client_id.clone(),
                public_key,
                std::net::SocketAddr::new(
                    std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                    41000,
                ),
                51820,
                fuckipv6_common::NatType::Open,
                tx,
            )
            .await
            .expect_err("deleted authorization config must reject registration");
        assert!(format!("{error:#}").contains("持久配置不存在"));
        assert!(manager.get_peer_info(&client_id).await.is_none());

        let _ = std::fs::remove_file(store.lock_path());
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
