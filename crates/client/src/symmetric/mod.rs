use anyhow::{Context, Result};
use fuckipv6_common::{
    CoordinatorConfig, Identity, NetworkAllocation, NetworkTicket, RelayEndpoint,
};
use fuckipv6_coordinator::{
    relay::server::RelayServer, signaling::blacklist::CoordinatorConfigBlacklistStore,
    signaling::server::SignalingServer, signaling::session::SessionManager,
};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::task::JoinHandle;

use crate::config_manager::ConfigManager;

mod actor;

pub use actor::{
    EmbeddedServiceError, EmbeddedServiceManager, EmbeddedServiceSnapshot,
    EmbeddedServiceStartRequest,
};

/// 内嵌协调端 Actor
///
/// 赋予普通客户端节点完整的中心协调与中继服务能力，
/// 使每个节点都能一键创建自己的对等网络。
pub struct EmbeddedCoordinator {
    pub ticket: NetworkTicket,
    pub signaling_addr: SocketAddr,
    pub relay_addr: SocketAddr,
    pub session_manager: Arc<SessionManager>,
    blacklist_store: Arc<CoordinatorConfigBlacklistStore>,
    signaling_task: Option<JoinHandle<Result<()>>>,
    relay_task: Option<JoinHandle<Result<()>>>,
}

impl EmbeddedCoordinator {
    /// 启动内嵌协调 Actor
    pub async fn start(
        network_name: &str,
        signaling_port: u16,
        relay_port: u16,
        host_public_key: Option<[u8; 32]>,
        external_host: Option<String>,
        ip_pool: Option<&str>,
    ) -> Result<Self> {
        let config_manager = ConfigManager::new().context("无法确定节点配置目录")?;
        Self::start_with_config_manager(
            network_name,
            signaling_port,
            relay_port,
            host_public_key,
            external_host,
            ip_pool,
            config_manager,
        )
        .await
    }

    /// Start an embedded coordinator using an explicitly isolated config
    /// directory. Intended for embedders and tests that must not read or write
    /// the user's default config directory.
    pub async fn start_with_config_dir(
        network_name: &str,
        signaling_port: u16,
        relay_port: u16,
        host_public_key: Option<[u8; 32]>,
        external_host: Option<String>,
        ip_pool: Option<&str>,
        config_dir: std::path::PathBuf,
    ) -> Result<Self> {
        let config_manager =
            ConfigManager::new_for_dir(config_dir).context("无法初始化内嵌协调端配置目录")?;
        Self::start_with_config_manager(
            network_name,
            signaling_port,
            relay_port,
            host_public_key,
            external_host,
            ip_pool,
            config_manager,
        )
        .await
    }

    pub(crate) async fn start_with_config_manager(
        network_name: &str,
        signaling_port: u16,
        relay_port: u16,
        host_public_key: Option<[u8; 32]>,
        external_host: Option<String>,
        ip_pool: Option<&str>,
        config_manager: ConfigManager,
    ) -> Result<Self> {
        let pool = ip_pool.unwrap_or("fd00::1000/120");
        let blacklist_path = config_manager
            .coordinator_config_path(network_name)
            .context("无法确定协调端持久化路径")?;
        let mut default_coordinator_config = CoordinatorConfig::default();
        default_coordinator_config.network.ipv6_pool = pool.to_string();
        default_coordinator_config.network.ip_pool = Some(pool.to_string());
        let blacklist_store = CoordinatorConfigBlacklistStore::with_default_config(
            blacklist_path.clone(),
            default_coordinator_config.clone(),
        );
        // Authorization lookups deliberately fail closed when the persistent
        // coordinator config is absent. A newly created embedded network has
        // no config yet, so bootstrap it here before attaching the store. The
        // store will then assign a stable network id under its file lock.
        if !blacklist_path.exists() {
            if let Some(parent) = blacklist_path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("无法创建内嵌协调端配置目录: {}", parent.display()))?;
            }
            default_coordinator_config
                .save_to_file(&blacklist_path)
                .context("无法初始化内嵌协调端持久配置")?;
        }
        let persisted_allocation = blacklist_store
            .load_network_allocation()
            .context("无法加载持久化协调端地址分配")?;
        let session_manager = Arc::new(
            SessionManager::new_with_allocation(&persisted_allocation)
                .context("无法初始化内嵌协调器地址池")?,
        );
        let blacklist_store = Arc::new(blacklist_store);
        session_manager
            .attach_blacklist_store(blacklist_store.clone())
            .await
            .context("无法加载节点持久黑名单")?;

        let signaling_bind = format!("0.0.0.0:{signaling_port}");
        let signaling_server =
            SignalingServer::new_with_manager(signaling_bind, session_manager.clone())
                .bind()
                .await
                .context("无法绑定内嵌信令服务")?;
        let actual_signaling_addr = signaling_server.local_addr()?;

        let relay_bind = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), relay_port);
        let relay_server =
            RelayServer::bind_with_generated_certificate(relay_bind, session_manager.clone())
                .context("无法启动内嵌 QUIC 中继服务")?;

        let actual_relay_addr = relay_server.local_addr();
        let relay_pin = relay_server.certificate().public_key_pin;
        let relay_endpoint = RelayEndpoint {
            addr: actual_relay_addr,
            server_name: "fuckipv6.local".to_string(),
            public_key_pin: relay_pin,
        };

        let signaling_server = signaling_server.with_relay_endpoint(relay_endpoint);
        signaling_server.spawn_timeout_cleanup(120);

        // Runtime-owned services supply the node-wide persisted public key so
        // recreating a service keeps the invitation identity stable. Direct
        // embedders that omit it retain the old ephemeral-ticket behavior.
        let host_pubkey = match host_public_key {
            Some(public_key) => public_key,
            None => Identity::generate()?.public_key_bytes(),
        };

        // 确定票据暴露的接入地址
        let actual_signaling_port = actual_signaling_addr.port();
        let ticket_endpoint = if let Some(host) = external_host {
            if host.contains(':') {
                host
            } else {
                format!("{host}:{actual_signaling_port}")
            }
        } else {
            // 本机探测默认地址
            format!("127.0.0.1:{actual_signaling_port}")
        };

        let mut ticket = NetworkTicket::new(host_pubkey, ticket_endpoint, network_name);
        if persisted_allocation.mode.is_ipv4_enabled() {
            ticket = ticket.with_ipv4(persisted_allocation.ipv4_pool.clone());
        }
        if persisted_allocation.mode.is_ipv6_enabled() {
            ticket = ticket.with_ipv6(persisted_allocation.effective_ipv6_pool());
        }

        let signaling_task = tokio::spawn(async move { signaling_server.run().await });

        let relay_task = tokio::spawn(async move { relay_server.run().await });

        let signaling_addr = actual_signaling_addr;

        tracing::info!(
            "🚀 内嵌协调 Actor 启动成功: 信令={}, QUIC中继={}",
            signaling_addr,
            actual_relay_addr
        );
        tracing::info!(
            "🎫 网络 [{}] 邀请票据已生成（敏感值不写入日志）",
            network_name
        );

        Ok(Self {
            ticket,
            signaling_addr,
            relay_addr: actual_relay_addr,
            session_manager,
            blacklist_store,
            signaling_task: Some(signaling_task),
            relay_task: Some(relay_task),
        })
    }

    pub async fn persisted_network_allocation(&self) -> Result<NetworkAllocation> {
        let store = self.blacklist_store.clone();
        tokio::task::spawn_blocking(move || store.load_network_allocation())
            .await
            .context("等待协调端地址分配读取任务失败")?
    }

    /// Persist the new allocation first, then hot-update the runtime allocator.
    /// If the runtime rejects the update, restore the previous persisted value
    /// and fail closed so callers never report a half-applied configuration.
    pub async fn update_network_allocation(&self, allocation: NetworkAllocation) -> Result<()> {
        let previous = self.session_manager.current_network_allocation().await;
        let store = self.blacklist_store.clone();
        let next = allocation.clone();
        tokio::task::spawn_blocking(move || store.save_network_allocation(&next))
            .await
            .context("等待协调端地址分配持久化任务失败")??;

        if let Err(runtime_error) = self
            .session_manager
            .update_network_allocation(allocation)
            .await
        {
            let store = self.blacklist_store.clone();
            let rollback =
                tokio::task::spawn_blocking(move || store.save_network_allocation(&previous))
                    .await
                    .context("等待协调端地址分配回滚任务失败")?;
            return match rollback {
                Ok(()) => Err(runtime_error).context("运行时拒绝地址分配，持久化配置已回滚"),
                Err(rollback_error) => Err(runtime_error).context(format!(
                    "运行时拒绝地址分配，且持久化配置回滚失败: {rollback_error:#}"
                )),
            };
        }
        Ok(())
    }

    /// Stop and join both listener tasks so their TCP/UDP ports are released
    /// before the caller treats the service as stopped.
    pub async fn shutdown(&mut self) -> Result<()> {
        let mut failures = Vec::new();
        for (name, task) in [
            ("信令", self.signaling_task.take()),
            ("QUIC 中继", self.relay_task.take()),
        ] {
            let Some(task) = task else {
                continue;
            };
            task.abort();
            match task.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => failures.push(format!("{name}服务异常退出: {error:#}")),
                Err(error) if error.is_cancelled() => {}
                Err(error) => failures.push(format!("{name}服务任务异常: {error}")),
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            anyhow::bail!("{}", failures.join("; "))
        }
    }
}

impl Drop for EmbeddedCoordinator {
    fn drop(&mut self) {
        if let Some(task) = self.signaling_task.take() {
            task.abort();
        }
        if let Some(task) = self.relay_task.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn allocation_update_rolls_back_persistence_when_live_lease_blocks_it() {
        let config_dir = std::env::temp_dir().join(format!(
            "fuckipv6-embedded-allocation-rollback-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&config_dir);
        let mut coordinator = EmbeddedCoordinator::start_with_config_dir(
            "rollback-mesh",
            0,
            0,
            None,
            Some("127.0.0.1".to_string()),
            Some("fd00::1000/120"),
            config_dir.clone(),
        )
        .await
        .expect("start embedded coordinator");
        let previous = coordinator
            .session_manager
            .current_network_allocation()
            .await;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        coordinator
            .session_manager
            .register_client(
                "live-client".to_string(),
                vec![7; 32],
                "192.0.2.10:41000".parse().expect("socket address"),
                51820,
                fuckipv6_common::NatType::Open,
                tx,
            )
            .await
            .expect("register live client");

        let error = coordinator
            .update_network_allocation(NetworkAllocation {
                mode: fuckipv6_common::NetworkMode::Ipv4Only,
                ipv4_pool: "192.168.100.0/24".to_string(),
                ipv6_pool: "fd00::1000/120".to_string(),
                ip_pool: Some("fd00::1000/120".to_string()),
            })
            .await
            .expect_err("live IPv6 lease must reject IPv4-only allocation");
        assert!(format!("{error:#}").contains("持久化配置已回滚"));
        assert_eq!(
            coordinator
                .session_manager
                .current_network_allocation()
                .await,
            previous
        );
        assert_eq!(
            coordinator
                .persisted_network_allocation()
                .await
                .expect("read rolled back allocation"),
            previous
        );

        coordinator.shutdown().await.expect("shutdown coordinator");
        let _ = std::fs::remove_dir_all(config_dir);
    }

    #[tokio::test]
    async fn allocation_persistence_failure_keeps_runtime_unchanged() {
        let config_dir = std::env::temp_dir().join(format!(
            "fuckipv6-embedded-allocation-persist-failure-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&config_dir);
        let configs = ConfigManager::new_for_dir(config_dir.clone()).expect("config manager");
        let config_path = configs
            .coordinator_config_path("persist-failure-mesh")
            .expect("coordinator config path");
        let mut coordinator = EmbeddedCoordinator::start_with_config_dir(
            "persist-failure-mesh",
            0,
            0,
            None,
            Some("127.0.0.1".to_string()),
            Some("fd00::1000/120"),
            config_dir.clone(),
        )
        .await
        .expect("start embedded coordinator");
        let previous = coordinator
            .session_manager
            .current_network_allocation()
            .await;
        std::fs::remove_file(&config_path).expect("remove config file");
        std::fs::create_dir_all(&config_path).expect("replace config with directory");

        let error = coordinator
            .update_network_allocation(NetworkAllocation {
                mode: fuckipv6_common::NetworkMode::Ipv4Only,
                ipv4_pool: "192.168.100.0/24".to_string(),
                ipv6_pool: "fd00::1000/120".to_string(),
                ip_pool: Some("fd00::1000/120".to_string()),
            })
            .await
            .expect_err("persistence failure must reject update");
        assert!(format!("{error:#}").contains("读取协调端持久配置失败"));
        assert_eq!(
            coordinator
                .session_manager
                .current_network_allocation()
                .await,
            previous
        );

        coordinator.shutdown().await.expect("shutdown coordinator");
        let _ = std::fs::remove_dir_all(config_dir);
    }

    #[tokio::test]
    async fn test_embedded_coordinator_startup() {
        let config_dir = std::env::temp_dir().join(format!(
            "fuckipv6-embedded-coordinator-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&config_dir);
        // 使用端口 0 避免冲突 (信令使用系统空闲端口)
        let mut coordinator = EmbeddedCoordinator::start_with_config_dir(
            "test-mesh",
            0, // random free port
            0,
            None,
            Some("127.0.0.1".to_string()),
            Some("fd00::1000/120"),
            config_dir.clone(),
        )
        .await
        .expect("start embedded coordinator failed");

        assert_eq!(coordinator.ticket.network_name, "test-mesh");
        assert!(coordinator.ticket.to_string().starts_with("fk://"));
        assert_ne!(coordinator.signaling_addr.port(), 0);
        assert_ne!(coordinator.relay_addr.port(), 0);
        coordinator.shutdown().await.expect("shutdown coordinator");
        let _ = std::fs::remove_dir_all(config_dir);
    }
}
