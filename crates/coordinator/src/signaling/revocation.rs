use anyhow::{bail, Context, Result};
use fuckipv6_common::{Identity, NetworkAuthorizationRevocation};
use std::sync::Arc;
use tokio::sync::RwLock;

use super::blacklist::SharedBlacklistStore;

/// 网络作用域内的持久授权撤销目录。
///
/// 查询直接从持久存储加载，任何读取或解析失败都会返回错误，调用方必须将其
/// 视为“已撤销”并拒绝注册。读操作可并发执行；撤销和解撤销串行执行。实际
/// 单条记录的原子替换由存储实现负责，避免多个协调器实例丢失更新。
#[derive(Clone)]
pub struct NetworkAuthorizationDirectory {
    network_id: String,
    store: SharedBlacklistStore,
    operation_gate: Arc<RwLock<()>>,
}

impl NetworkAuthorizationDirectory {
    pub fn new(network_id: impl Into<String>, store: SharedBlacklistStore) -> Result<Self> {
        let network_id = network_id.into();
        if network_id.trim().is_empty() {
            bail!("授权撤销目录网络标识不能为空");
        }
        Ok(Self {
            network_id,
            store,
            operation_gate: Arc::new(RwLock::new(())),
        })
    }

    pub fn network_id(&self) -> &str {
        &self.network_id
    }

    /// 精确查询当前网络内的撤销记录。
    ///
    /// 客户端 ID 或公钥指纹任一命中即视为已撤销。存储读取失败返回错误，绝不
    /// 降级为空目录；其他网络的记录不会参与当前网络的判定。
    pub async fn is_revoked(&self, client_id: &str, public_key: &[u8]) -> Result<bool> {
        let record = self.record_for(client_id, public_key)?;
        let _guard = self.operation_gate.read().await;
        let store = self.store.clone();
        let network_id = self.network_id.clone();
        tokio::task::spawn_blocking(move || {
            store.load_authorization_revocations().map(|revocations| {
                revocations.iter().any(|existing| {
                    existing.network_id == network_id
                        && (existing.client_id == record.client_id
                            || existing.public_key_fingerprint == record.public_key_fingerprint)
                })
            })
        })
        .await
        .context("等待授权撤销查询任务失败")?
    }

    /// 持久化一条当前网络的撤销记录。
    ///
    /// 返回 `true` 表示新增了撤销记录，`false` 表示该身份已处于撤销状态。
    pub async fn revoke(&self, client_id: &str, public_key: &[u8]) -> Result<bool> {
        let record = self.record_for(client_id, public_key)?;
        let _guard = self.operation_gate.write().await;
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || store.revoke_authorization(&record))
            .await
            .context("等待授权撤销写入任务失败")?
    }

    /// 移除当前网络中与客户端 ID 或公钥指纹匹配的撤销记录。
    pub async fn lift(&self, client_id: &str, public_key: &[u8]) -> Result<bool> {
        let record = self.record_for(client_id, public_key)?;
        let _guard = self.operation_gate.write().await;
        let store = self.store.clone();
        let network_id = self.network_id.clone();
        tokio::task::spawn_blocking(move || {
            store.lift_authorization(
                &network_id,
                &record.client_id,
                &record.public_key_fingerprint,
            )
        })
        .await
        .context("等待授权撤销移除任务失败")?
    }

    /// 返回当前网络的全部撤销记录；读取失败时返回错误。
    pub async fn list(&self) -> Result<Vec<NetworkAuthorizationRevocation>> {
        let _guard = self.operation_gate.read().await;
        let store = self.store.clone();
        let network_id = self.network_id.clone();
        tokio::task::spawn_blocking(move || {
            store.load_authorization_revocations().map(|revocations| {
                revocations
                    .into_iter()
                    .filter(|record| record.network_id == network_id)
                    .collect()
            })
        })
        .await
        .context("等待授权撤销列表任务失败")?
    }

    fn record_for(
        &self,
        client_id: &str,
        public_key: &[u8],
    ) -> Result<NetworkAuthorizationRevocation> {
        let client_id = client_id.trim();
        if client_id.is_empty() {
            bail!("节点 ID 不能为空");
        }
        if public_key.len() != 32 {
            bail!("节点公钥必须为 32 字节");
        }
        Ok(NetworkAuthorizationRevocation {
            network_id: self.network_id.clone(),
            client_id: client_id.to_string(),
            public_key_fingerprint: Identity::public_key_fingerprint(public_key),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signaling::blacklist::{BlacklistStore, CoordinatorConfigBlacklistStore};
    use fuckipv6_common::{BlacklistConfig, CoordinatorConfig};
    use std::path::PathBuf;
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
            "fuckipv6-revocation-{name}-{}-{unique}-{sequence}.toml",
            std::process::id()
        ))
    }

    fn directory(
        path: &std::path::Path,
        network_id: &str,
    ) -> Result<NetworkAuthorizationDirectory> {
        if !path.exists() {
            let config = CoordinatorConfig {
                network_id: network_id.to_string(),
                ..CoordinatorConfig::default()
            };
            config.save_to_file(path)?;
        }
        let concrete = CoordinatorConfigBlacklistStore::new(path);
        assert_eq!(concrete.authorization_network_id()?, network_id);
        let store: SharedBlacklistStore = Arc::new(concrete);
        NetworkAuthorizationDirectory::new(network_id, store)
    }

    #[tokio::test]
    async fn revocations_survive_restart_and_do_not_cross_network_boundaries() -> Result<()> {
        let path_a = test_config_path("network-isolation-a");
        let path_b = test_config_path("network-isolation-b");
        let identity = Identity::generate()?;
        let client_id = identity.client_id();
        let public_key = identity.public_key();

        let network_a = directory(&path_a, "network-a")?;
        let network_b = directory(&path_b, "network-b")?;
        assert!(!network_a.is_revoked(client_id, &public_key).await?);
        assert!(network_a.revoke(client_id, &public_key).await?);
        assert!(network_a.is_revoked(client_id, &public_key).await?);
        assert!(
            !network_b.is_revoked(client_id, &public_key).await?,
            "revocation in one network must not apply to another network"
        );

        let restarted = directory(&path_a, "network-a")?;
        assert!(restarted.is_revoked(client_id, &public_key).await?);
        assert_eq!(restarted.list().await?.len(), 1);

        std::fs::remove_file(path_a)?;
        std::fs::remove_file(path_b)?;
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_revocations_from_independent_store_instances_preserve_all_updates(
    ) -> Result<()> {
        let path = test_config_path("concurrent");
        let config = CoordinatorConfig {
            network_id: "network-a".to_string(),
            ..CoordinatorConfig::default()
        };
        config.save_to_file(&path)?;
        let mut identities = Vec::new();
        let mut tasks = Vec::new();
        for _ in 0..16 {
            let identity = Identity::generate()?;
            let client_id = identity.client_id().to_string();
            let public_key = identity.public_key();
            identities.push((client_id.clone(), public_key.clone()));
            let task_path = path.clone();
            tasks.push(tokio::spawn(async move {
                let store = CoordinatorConfigBlacklistStore::new(task_path);
                let network = NetworkAuthorizationDirectory::new(
                    "network-a",
                    Arc::new(store) as SharedBlacklistStore,
                )?;
                network.revoke(&client_id, &public_key).await
            }));
        }
        for task in tasks {
            assert!(task.await??);
        }

        let persisted = CoordinatorConfig::from_file(&path)?;
        assert_eq!(persisted.authorization_revocations.len(), identities.len());
        let store = CoordinatorConfigBlacklistStore::new(&path);
        let network = NetworkAuthorizationDirectory::new(
            "network-a",
            Arc::new(store) as SharedBlacklistStore,
        )?;
        for (client_id, public_key) in identities {
            assert!(network.is_revoked(&client_id, &public_key).await?);
        }

        std::fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn subprocess_revocation_writer() -> Result<()> {
        let Ok(path) = std::env::var("FUCKIPV6_REVOCATION_CHILD_PATH") else {
            return Ok(());
        };
        let store = CoordinatorConfigBlacklistStore::new(path);
        let network_id = store.authorization_network_id()?;
        let identity = Identity::generate()?;
        let record = NetworkAuthorizationRevocation {
            network_id,
            client_id: identity.client_id().to_string(),
            public_key_fingerprint: Identity::public_key_fingerprint(&identity.public_key()),
        };
        assert!(store.revoke_authorization(&record)?);
        Ok(())
    }

    #[test]
    fn concurrent_revocations_from_separate_processes_preserve_all_updates() -> Result<()> {
        const WRITERS: usize = 12;
        let path = test_config_path("concurrent-processes");
        let config = CoordinatorConfig {
            network_id: "network-a".to_string(),
            ..CoordinatorConfig::default()
        };
        config.save_to_file(&path)?;

        let executable = std::env::current_exe().context("获取测试进程路径失败")?;
        let mut children = Vec::with_capacity(WRITERS);
        for _ in 0..WRITERS {
            children.push(
                std::process::Command::new(&executable)
                    .args([
                        "--exact",
                        "signaling::revocation::tests::subprocess_revocation_writer",
                        "--test-threads=1",
                    ])
                    .env("FUCKIPV6_REVOCATION_CHILD_PATH", &path)
                    .spawn()
                    .context("启动授权撤销子进程失败")?,
            );
        }
        for child in children {
            let output = child.wait_with_output().context("等待授权撤销子进程失败")?;
            assert!(
                output.status.success(),
                "授权撤销子进程失败: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let persisted = CoordinatorConfig::from_file(&path)?;
        assert_eq!(persisted.authorization_revocations.len(), WRITERS);

        std::fs::remove_file(path)?;
        Ok(())
    }

    #[tokio::test]
    async fn missing_config_fails_closed_for_revocation_reads_and_writes() -> Result<()> {
        let path = test_config_path("missing-config");
        let store = CoordinatorConfigBlacklistStore::new(&path);
        assert!(store.load_authorization_revocations().is_err());
        assert!(
            !path.exists(),
            "a failed revocation read must not create a config"
        );

        let directory = NetworkAuthorizationDirectory::new(
            "network-a",
            Arc::new(store) as SharedBlacklistStore,
        )?;
        let identity = Identity::generate()?;
        assert!(directory
            .is_revoked(identity.client_id(), &identity.public_key())
            .await
            .is_err());
        assert!(directory
            .revoke(identity.client_id(), &identity.public_key())
            .await
            .is_err());
        assert!(
            !path.exists(),
            "a failed revocation write must not create a config"
        );
        Ok(())
    }

    #[test]
    fn revocation_updates_preserve_blacklist_and_other_config_fields() -> Result<()> {
        let path = test_config_path("preserve-config");
        let store = CoordinatorConfigBlacklistStore::new(&path);
        store.save(&BlacklistConfig {
            banned_clients: vec!["node-bad".to_string()],
        })?;
        let identity = Identity::generate()?;
        let network_id = store.authorization_network_id()?;
        let record = NetworkAuthorizationRevocation {
            network_id,
            client_id: identity.client_id().to_string(),
            public_key_fingerprint: Identity::public_key_fingerprint(&identity.public_key()),
        };
        assert!(store.revoke_authorization(&record)?);

        let persisted = CoordinatorConfig::from_file(&path)?;
        assert_eq!(persisted.blacklist.banned_clients, vec!["node-bad"]);
        assert_eq!(persisted.authorization_revocations, vec![record]);

        std::fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn coordinator_store_rejects_cross_network_revocation_writes() -> Result<()> {
        let path = test_config_path("cross-network-write");
        let config = CoordinatorConfig {
            network_id: "network-a".to_string(),
            ..CoordinatorConfig::default()
        };
        config.save_to_file(&path)?;
        let store = CoordinatorConfigBlacklistStore::new(&path);
        let identity = Identity::generate()?;
        let record = NetworkAuthorizationRevocation {
            network_id: "network-b".to_string(),
            client_id: identity.client_id().to_string(),
            public_key_fingerprint: Identity::public_key_fingerprint(&identity.public_key()),
        };

        let error = store
            .revoke_authorization(&record)
            .expect_err("cross-network revocation writes must be rejected");
        assert!(error.to_string().contains("当前网络"));
        assert!(CoordinatorConfig::from_file(&path)?
            .authorization_revocations
            .is_empty());

        std::fs::remove_file(path)?;
        Ok(())
    }
}
