use std::collections::HashMap;
use std::net::SocketAddr;

use tokio::sync::{mpsc, oneshot};

use super::EmbeddedCoordinator;
use crate::config_manager::ConfigManager;
use crate::node::{NodeBlacklistSnapshot, NodeNetworkAllocation, NodeNetworkAllocationSnapshot};

/// Request to start the embedded signaling and relay services for one network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedServiceStartRequest {
    pub network_name: String,
    pub signaling_port: u16,
    pub relay_port: u16,
    pub host_public_key: Option<[u8; 32]>,
    pub external_host: Option<String>,
    pub ip_pool: Option<String>,
}

impl EmbeddedServiceStartRequest {
    pub fn new(network_name: impl Into<String>, signaling_port: u16, relay_port: u16) -> Self {
        Self {
            network_name: network_name.into(),
            signaling_port,
            relay_port,
            host_public_key: None,
            external_host: None,
            ip_pool: None,
        }
    }

    pub fn with_host_public_key(mut self, host_public_key: [u8; 32]) -> Self {
        self.host_public_key = Some(host_public_key);
        self
    }

    pub fn with_external_host(mut self, external_host: impl Into<String>) -> Self {
        self.external_host = Some(external_host.into());
        self
    }

    pub fn with_ip_pool(mut self, ip_pool: impl Into<String>) -> Self {
        self.ip_pool = Some(ip_pool.into());
        self
    }

    fn validate(&self) -> Result<(), EmbeddedServiceError> {
        let name = self.network_name.trim();
        if name.is_empty() {
            return Err(EmbeddedServiceError::InvalidRequest(
                "网络名称不能为空".into(),
            ));
        }
        if name != self.network_name {
            return Err(EmbeddedServiceError::InvalidRequest(
                "网络名称首尾不能包含空白字符".into(),
            ));
        }
        if self.network_name.chars().any(char::is_control)
            || self.network_name.contains('/')
            || self.network_name.contains('\\')
            || self.network_name == "."
            || self.network_name == ".."
        {
            return Err(EmbeddedServiceError::InvalidRequest(
                "网络名称包含不允许的字符".into(),
            ));
        }
        if self.signaling_port != 0 && self.signaling_port == self.relay_port {
            return Err(EmbeddedServiceError::InvalidRequest(
                "信令端口和中继端口不能相同".into(),
            ));
        }
        if self
            .external_host
            .as_deref()
            .is_some_and(|host| host.trim().is_empty())
        {
            return Err(EmbeddedServiceError::InvalidRequest(
                "外部主机名不能是空字符串".into(),
            ));
        }
        if self
            .ip_pool
            .as_deref()
            .is_some_and(|pool| pool.trim().is_empty())
        {
            return Err(EmbeddedServiceError::InvalidRequest(
                "地址池不能是空字符串".into(),
            ));
        }
        Ok(())
    }
}

/// Transport-neutral view of one running embedded network service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedServiceSnapshot {
    pub network_name: String,
    pub ticket: String,
    pub signaling_addr: SocketAddr,
    pub relay_addr: SocketAddr,
}

impl From<&EmbeddedCoordinator> for EmbeddedServiceSnapshot {
    fn from(coordinator: &EmbeddedCoordinator) -> Self {
        Self {
            network_name: coordinator.ticket.network_name.clone(),
            ticket: coordinator.ticket.to_string(),
            signaling_addr: coordinator.signaling_addr,
            relay_addr: coordinator.relay_addr,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EmbeddedServiceError {
    #[error("内嵌网络服务请求无效: {0}")]
    InvalidRequest(String),
    #[error("网络 [{0}] 的内嵌服务已在运行")]
    AlreadyRunning(String),
    #[error("网络 [{0}] 的内嵌服务未运行")]
    NotFound(String),
    #[error("内嵌网络服务启动失败: {network_name}: {message}")]
    StartFailed {
        network_name: String,
        message: String,
    },
    #[error("内嵌网络服务停止失败: {network_name}: {message}")]
    StopFailed {
        network_name: String,
        message: String,
    },
    #[error("内嵌网络服务关闭未完全成功: {message}")]
    ShutdownFailed { message: String },
    #[error("网络 [{network_name}] 的持久化管理操作失败: {message}")]
    PersistenceFailed {
        network_name: String,
        message: String,
    },
    #[error("网络 [{network_name}] 的管理操作失败: {message}")]
    OperationFailed {
        network_name: String,
        message: String,
    },
    #[error("内嵌网络服务 actor 已关闭")]
    ActorClosed,
}

enum EmbeddedServiceCommand {
    Start {
        request: EmbeddedServiceStartRequest,
        reply: oneshot::Sender<Result<EmbeddedServiceSnapshot, EmbeddedServiceError>>,
    },
    Stop {
        network_name: String,
        reply: oneshot::Sender<Result<EmbeddedServiceSnapshot, EmbeddedServiceError>>,
    },
    List {
        reply: oneshot::Sender<Vec<EmbeddedServiceSnapshot>>,
    },
    GetBlacklist {
        network_name: String,
        reply: oneshot::Sender<Result<NodeBlacklistSnapshot, EmbeddedServiceError>>,
    },
    BanClient {
        network_name: String,
        client_id: String,
        reply: oneshot::Sender<Result<NodeBlacklistSnapshot, EmbeddedServiceError>>,
    },
    UnbanClient {
        network_name: String,
        client_id: String,
        reply: oneshot::Sender<Result<NodeBlacklistSnapshot, EmbeddedServiceError>>,
    },
    GetNetworkAllocation {
        network_name: String,
        reply: oneshot::Sender<Result<NodeNetworkAllocationSnapshot, EmbeddedServiceError>>,
    },
    UpdateNetworkAllocation {
        network_name: String,
        allocation: NodeNetworkAllocation,
        reply: oneshot::Sender<Result<NodeNetworkAllocationSnapshot, EmbeddedServiceError>>,
    },
    Shutdown {
        reply: oneshot::Sender<Result<usize, EmbeddedServiceError>>,
    },
}

/// Cloneable handle to the single actor that owns embedded network services.
///
/// Dropping a handle does not stop services while other handles remain. Calling
/// `shutdown` stops all services and closes the actor to later requests.
#[derive(Clone)]
pub struct EmbeddedServiceManager {
    sender: mpsc::Sender<EmbeddedServiceCommand>,
}

impl EmbeddedServiceManager {
    pub fn spawn(capacity: usize) -> Self {
        Self::spawn_inner(capacity, ConfigManager::new())
    }

    /// Spawn a manager whose embedded services persist state under `configs`.
    /// The node supervisor passes its already-resolved config manager here so
    /// service lifecycle state and the node share one isolated config root.
    pub(crate) fn spawn_with_config(capacity: usize, configs: ConfigManager) -> Self {
        Self::spawn_inner(capacity, Ok(configs))
    }

    fn spawn_inner(capacity: usize, configs: Result<ConfigManager, anyhow::Error>) -> Self {
        let (sender, mut receiver) = mpsc::channel::<EmbeddedServiceCommand>(capacity.max(1));

        tokio::spawn(async move {
            let mut services = HashMap::<String, EmbeddedCoordinator>::new();
            let configs = configs.map_err(|error| format!("无法确定节点配置目录: {error:#}"));

            while let Some(command) = receiver.recv().await {
                match command {
                    EmbeddedServiceCommand::Start { request, reply } => {
                        let result = match configs.as_ref() {
                            Ok(configs) => start_service(&mut services, request, configs).await,
                            Err(message) => Err(EmbeddedServiceError::StartFailed {
                                network_name: request.network_name,
                                message: message.clone(),
                            }),
                        };
                        let _ = reply.send(result);
                    }
                    EmbeddedServiceCommand::Stop {
                        network_name,
                        reply,
                    } => {
                        let result = if let Some(mut coordinator) = services.remove(&network_name) {
                            let snapshot = EmbeddedServiceSnapshot::from(&coordinator);
                            match coordinator.shutdown().await {
                                Ok(()) => Ok(snapshot),
                                Err(error) => Err(EmbeddedServiceError::StopFailed {
                                    network_name,
                                    message: format!("{error:#}"),
                                }),
                            }
                        } else {
                            Err(EmbeddedServiceError::NotFound(network_name))
                        };
                        let _ = reply.send(result);
                    }
                    EmbeddedServiceCommand::List { reply } => {
                        let mut snapshots: Vec<_> = services
                            .values()
                            .map(EmbeddedServiceSnapshot::from)
                            .collect();
                        snapshots.sort_by(|a, b| a.network_name.cmp(&b.network_name));
                        let _ = reply.send(snapshots);
                    }
                    EmbeddedServiceCommand::GetBlacklist {
                        network_name,
                        reply,
                    } => {
                        let result = match services.get(&network_name) {
                            Some(coordinator) => Ok(NodeBlacklistSnapshot {
                                network_name: network_name.clone(),
                                clients: coordinator.session_manager.get_blacklist().await,
                                changed: false,
                            }),
                            None => Err(EmbeddedServiceError::NotFound(network_name)),
                        };
                        let _ = reply.send(result);
                    }
                    EmbeddedServiceCommand::BanClient {
                        network_name,
                        client_id,
                        reply,
                    } => {
                        let result = match services.get(&network_name) {
                            Some(coordinator) => {
                                match coordinator.session_manager.try_ban_client(client_id).await {
                                    Ok(changed) => Ok(blacklist_snapshot(
                                        coordinator,
                                        network_name.clone(),
                                        changed,
                                    )
                                    .await),
                                    Err(error) => Err(EmbeddedServiceError::OperationFailed {
                                        network_name: network_name.clone(),
                                        message: format!("{error:#}"),
                                    }),
                                }
                            }
                            None => Err(EmbeddedServiceError::NotFound(network_name)),
                        };
                        let _ = reply.send(result);
                    }
                    EmbeddedServiceCommand::UnbanClient {
                        network_name,
                        client_id,
                        reply,
                    } => {
                        let result = match services.get(&network_name) {
                            Some(coordinator) => {
                                match coordinator
                                    .session_manager
                                    .try_unban_client(&client_id)
                                    .await
                                {
                                    Ok(changed) => Ok(blacklist_snapshot(
                                        coordinator,
                                        network_name.clone(),
                                        changed,
                                    )
                                    .await),
                                    Err(error) => Err(EmbeddedServiceError::OperationFailed {
                                        network_name: network_name.clone(),
                                        message: format!("{error:#}"),
                                    }),
                                }
                            }
                            None => Err(EmbeddedServiceError::NotFound(network_name)),
                        };
                        let _ = reply.send(result);
                    }
                    EmbeddedServiceCommand::GetNetworkAllocation {
                        network_name,
                        reply,
                    } => {
                        let result = match services.get(&network_name) {
                            Some(coordinator) => {
                                let current = coordinator
                                    .session_manager
                                    .current_network_allocation()
                                    .await;
                                match coordinator.persisted_network_allocation().await {
                                    Ok(persisted) => Ok(NodeNetworkAllocationSnapshot {
                                        network_name: network_name.clone(),
                                        config_saved: persisted == current,
                                        allocation: NodeNetworkAllocation::from_network_allocation(
                                            current,
                                        ),
                                    }),
                                    Err(error) => Err(EmbeddedServiceError::PersistenceFailed {
                                        network_name: network_name.clone(),
                                        message: format!("{error:#}"),
                                    }),
                                }
                            }
                            None => Err(EmbeddedServiceError::NotFound(network_name)),
                        };
                        let _ = reply.send(result);
                    }
                    EmbeddedServiceCommand::UpdateNetworkAllocation {
                        network_name,
                        allocation,
                        reply,
                    } => {
                        let result = match services.get(&network_name) {
                            Some(coordinator) => coordinator
                                .update_network_allocation(allocation.to_network_allocation())
                                .await
                                .map(|()| NodeNetworkAllocationSnapshot {
                                    network_name: network_name.clone(),
                                    allocation,
                                    config_saved: true,
                                })
                                .map_err(|error| EmbeddedServiceError::OperationFailed {
                                    network_name: network_name.clone(),
                                    message: format!("{error:#}"),
                                }),
                            None => Err(EmbeddedServiceError::NotFound(network_name)),
                        };
                        let _ = reply.send(result);
                    }
                    EmbeddedServiceCommand::Shutdown { reply } => {
                        let count = services.len();
                        let mut failures = Vec::new();
                        for (network_name, mut coordinator) in services.drain() {
                            if let Err(error) = coordinator.shutdown().await {
                                failures.push(format!("{network_name}: {error:#}"));
                            }
                        }
                        let result = if failures.is_empty() {
                            Ok(count)
                        } else {
                            Err(EmbeddedServiceError::ShutdownFailed {
                                message: failures.join("; "),
                            })
                        };
                        let _ = reply.send(result);
                        break;
                    }
                }
            }
        });

        Self { sender }
    }

    pub async fn start(
        &self,
        request: EmbeddedServiceStartRequest,
    ) -> Result<EmbeddedServiceSnapshot, EmbeddedServiceError> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(EmbeddedServiceCommand::Start { request, reply })
            .await
            .map_err(|_| EmbeddedServiceError::ActorClosed)?;
        response
            .await
            .map_err(|_| EmbeddedServiceError::ActorClosed)?
    }

    pub async fn stop(
        &self,
        network_name: impl Into<String>,
    ) -> Result<EmbeddedServiceSnapshot, EmbeddedServiceError> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(EmbeddedServiceCommand::Stop {
                network_name: network_name.into(),
                reply,
            })
            .await
            .map_err(|_| EmbeddedServiceError::ActorClosed)?;
        response
            .await
            .map_err(|_| EmbeddedServiceError::ActorClosed)?
    }

    pub async fn list(&self) -> Result<Vec<EmbeddedServiceSnapshot>, EmbeddedServiceError> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(EmbeddedServiceCommand::List { reply })
            .await
            .map_err(|_| EmbeddedServiceError::ActorClosed)?;
        response
            .await
            .map_err(|_| EmbeddedServiceError::ActorClosed)
    }

    pub async fn get_blacklist(
        &self,
        network_name: impl Into<String>,
    ) -> Result<NodeBlacklistSnapshot, EmbeddedServiceError> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(EmbeddedServiceCommand::GetBlacklist {
                network_name: network_name.into(),
                reply,
            })
            .await
            .map_err(|_| EmbeddedServiceError::ActorClosed)?;
        response
            .await
            .map_err(|_| EmbeddedServiceError::ActorClosed)?
    }

    pub async fn ban_client(
        &self,
        network_name: impl Into<String>,
        client_id: impl Into<String>,
    ) -> Result<NodeBlacklistSnapshot, EmbeddedServiceError> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(EmbeddedServiceCommand::BanClient {
                network_name: network_name.into(),
                client_id: client_id.into(),
                reply,
            })
            .await
            .map_err(|_| EmbeddedServiceError::ActorClosed)?;
        response
            .await
            .map_err(|_| EmbeddedServiceError::ActorClosed)?
    }

    pub async fn unban_client(
        &self,
        network_name: impl Into<String>,
        client_id: impl Into<String>,
    ) -> Result<NodeBlacklistSnapshot, EmbeddedServiceError> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(EmbeddedServiceCommand::UnbanClient {
                network_name: network_name.into(),
                client_id: client_id.into(),
                reply,
            })
            .await
            .map_err(|_| EmbeddedServiceError::ActorClosed)?;
        response
            .await
            .map_err(|_| EmbeddedServiceError::ActorClosed)?
    }

    pub async fn get_network_allocation(
        &self,
        network_name: impl Into<String>,
    ) -> Result<NodeNetworkAllocationSnapshot, EmbeddedServiceError> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(EmbeddedServiceCommand::GetNetworkAllocation {
                network_name: network_name.into(),
                reply,
            })
            .await
            .map_err(|_| EmbeddedServiceError::ActorClosed)?;
        response
            .await
            .map_err(|_| EmbeddedServiceError::ActorClosed)?
    }

    pub async fn update_network_allocation(
        &self,
        network_name: impl Into<String>,
        allocation: NodeNetworkAllocation,
    ) -> Result<NodeNetworkAllocationSnapshot, EmbeddedServiceError> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(EmbeddedServiceCommand::UpdateNetworkAllocation {
                network_name: network_name.into(),
                allocation,
                reply,
            })
            .await
            .map_err(|_| EmbeddedServiceError::ActorClosed)?;
        response
            .await
            .map_err(|_| EmbeddedServiceError::ActorClosed)?
    }

    pub async fn shutdown(&self) -> Result<usize, EmbeddedServiceError> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(EmbeddedServiceCommand::Shutdown { reply })
            .await
            .map_err(|_| EmbeddedServiceError::ActorClosed)?;
        response
            .await
            .map_err(|_| EmbeddedServiceError::ActorClosed)?
    }
}

async fn blacklist_snapshot(
    coordinator: &EmbeddedCoordinator,
    network_name: String,
    changed: bool,
) -> NodeBlacklistSnapshot {
    NodeBlacklistSnapshot {
        network_name,
        clients: coordinator.session_manager.get_blacklist().await,
        changed,
    }
}

async fn start_service(
    services: &mut HashMap<String, EmbeddedCoordinator>,
    request: EmbeddedServiceStartRequest,
    configs: &ConfigManager,
) -> Result<EmbeddedServiceSnapshot, EmbeddedServiceError> {
    request.validate()?;
    if services.contains_key(&request.network_name) {
        return Err(EmbeddedServiceError::AlreadyRunning(request.network_name));
    }

    let coordinator = EmbeddedCoordinator::start_with_config_manager(
        &request.network_name,
        request.signaling_port,
        request.relay_port,
        request.host_public_key,
        request.external_host,
        request.ip_pool.as_deref(),
        configs.clone(),
    )
    .await
    .map_err(|error| EmbeddedServiceError::StartFailed {
        network_name: request.network_name.clone(),
        message: format!("{error:#}"),
    })?;

    let snapshot = EmbeddedServiceSnapshot::from(&coordinator);
    services.insert(request.network_name, coordinator);
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ops::Deref;

    struct TestManager {
        services: EmbeddedServiceManager,
        config_dir: std::path::PathBuf,
    }

    impl Deref for TestManager {
        type Target = EmbeddedServiceManager;

        fn deref(&self) -> &Self::Target {
            &self.services
        }
    }

    impl Drop for TestManager {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.config_dir);
        }
    }

    fn test_manager_at(capacity: usize, config_dir: std::path::PathBuf) -> TestManager {
        let configs = ConfigManager::new_for_dir(config_dir.clone()).expect("test config manager");
        TestManager {
            services: EmbeddedServiceManager::spawn_with_config(capacity, configs),
            config_dir,
        }
    }

    fn test_manager(capacity: usize) -> TestManager {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let config_dir = std::env::temp_dir().join(format!(
            "fuckipv6-embedded-services-{}-{id}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&config_dir);
        test_manager_at(capacity, config_dir)
    }

    #[tokio::test]
    async fn actor_owns_service_lifecycle_and_rejects_duplicates() {
        let manager = test_manager(4);
        let request = EmbeddedServiceStartRequest::new("lab", 0, 0)
            .with_external_host("127.0.0.1")
            .with_ip_pool("fd00::1000/120");

        let started = manager.start(request.clone()).await.expect("start service");
        assert_eq!(started.network_name, "lab");
        assert!(started.ticket.starts_with("fk://"));
        assert_eq!(manager.list().await.expect("list services").len(), 1);

        let duplicate = manager.start(request).await.expect_err("duplicate service");
        assert_eq!(
            duplicate,
            EmbeddedServiceError::AlreadyRunning("lab".into())
        );

        let stopped = manager.stop("lab").await.expect("stop service");
        assert_eq!(stopped.network_name, "lab");
        assert!(manager.list().await.expect("list services").is_empty());
        assert_eq!(
            manager
                .stop("lab")
                .await
                .expect_err("service already stopped"),
            EmbeddedServiceError::NotFound("lab".into())
        );
    }

    #[tokio::test]
    async fn actor_blacklist_commands_are_authoritative_and_idempotent() {
        let manager = test_manager(2);
        manager
            .start(EmbeddedServiceStartRequest::new("lab", 0, 0))
            .await
            .expect("start service");

        let empty = manager
            .get_blacklist("lab")
            .await
            .expect("read initial blacklist");
        assert!(empty.clients.is_empty());
        assert!(!empty.changed);

        let banned = manager
            .ban_client("lab", "node-bad")
            .await
            .expect("ban client");
        assert_eq!(banned.clients, vec!["node-bad".to_string()]);
        assert!(banned.changed);

        let repeated = manager
            .ban_client("lab", "node-bad")
            .await
            .expect("idempotent ban");
        assert_eq!(repeated.clients, vec!["node-bad".to_string()]);
        assert!(!repeated.changed);

        let unbanned = manager
            .unban_client("lab", "node-bad")
            .await
            .expect("unban client");
        assert!(unbanned.clients.is_empty());
        assert!(unbanned.changed);

        let repeated = manager
            .unban_client("lab", "node-bad")
            .await
            .expect("idempotent unban");
        assert!(repeated.clients.is_empty());
        assert!(!repeated.changed);

        assert_eq!(
            manager
                .ban_client("missing", "node-bad")
                .await
                .expect_err("unknown network"),
            EmbeddedServiceError::NotFound("missing".into())
        );
    }

    #[tokio::test]
    async fn allocation_update_is_persisted_and_loaded_after_actor_restart() {
        let manager = test_manager(2);
        manager
            .start(EmbeddedServiceStartRequest::new("lab", 0, 0))
            .await
            .expect("start service");
        let updated = NodeNetworkAllocation {
            mode: fuckipv6_common::NetworkMode::Ipv4Only,
            ipv4_pool: "192.168.100.0/24".to_string(),
            ipv6_pool: "fd00::1000/120".to_string(),
        };
        let snapshot = manager
            .update_network_allocation("lab", updated.clone())
            .await
            .expect("persist allocation update");
        assert_eq!(snapshot.allocation, updated);
        assert!(snapshot.config_saved);

        manager.shutdown().await.expect("shutdown first actor");
        let restarted = test_manager_at(2, manager.config_dir.clone());
        restarted
            .start(EmbeddedServiceStartRequest::new("lab", 0, 0))
            .await
            .expect("restart service from persisted config");
        let loaded = restarted
            .get_network_allocation("lab")
            .await
            .expect("read persisted allocation");
        assert_eq!(loaded.allocation, updated);
        assert!(loaded.config_saved);
        restarted
            .shutdown()
            .await
            .expect("shutdown restarted actor");
    }

    #[tokio::test]
    async fn shutdown_releases_services_and_closes_actor() {
        let manager = test_manager(2);
        manager
            .start(EmbeddedServiceStartRequest::new("lab", 0, 0))
            .await
            .expect("start service");

        assert_eq!(manager.shutdown().await.expect("shutdown actor"), 1);
        assert_eq!(
            manager
                .list()
                .await
                .expect_err("actor must be closed after shutdown"),
            EmbeddedServiceError::ActorClosed
        );
    }

    #[tokio::test]
    async fn rejects_unsafe_network_names_before_starting_services() {
        let manager = test_manager(1);
        for name in ["", "../escape", " server", "server "] {
            let error = manager
                .start(EmbeddedServiceStartRequest::new(name, 0, 0))
                .await
                .expect_err("unsafe name must fail");
            assert!(matches!(error, EmbeddedServiceError::InvalidRequest(_)));
        }
        assert!(manager.list().await.expect("list services").is_empty());
    }

    #[tokio::test]
    async fn stop_waits_until_fixed_listener_ports_are_released() {
        let signaling_port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve TCP port");
            listener.local_addr().expect("TCP address").port()
        };
        let relay_port = {
            let socket = std::net::UdpSocket::bind("127.0.0.1:0").expect("reserve UDP port");
            socket.local_addr().expect("UDP address").port()
        };
        let manager = test_manager(2);
        manager
            .start(
                EmbeddedServiceStartRequest::new("port-release", signaling_port, relay_port)
                    .with_external_host("127.0.0.1"),
            )
            .await
            .expect("start service on fixed ports");

        manager.stop("port-release").await.expect("stop service");

        tokio::net::TcpListener::bind(("127.0.0.1", signaling_port))
            .await
            .expect("signaling TCP port must be released after stop");
        tokio::net::UdpSocket::bind(("127.0.0.1", relay_port))
            .await
            .expect("relay UDP port must be released after stop");
    }
}
