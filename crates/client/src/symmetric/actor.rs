use std::collections::HashMap;
use std::net::SocketAddr;

use tokio::sync::{mpsc, oneshot};

use super::EmbeddedCoordinator;

/// Request to start the embedded signaling and relay services for one network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedServiceStartRequest {
    pub network_name: String,
    pub signaling_port: u16,
    pub relay_port: u16,
    pub external_host: Option<String>,
    pub ip_pool: Option<String>,
}

impl EmbeddedServiceStartRequest {
    pub fn new(network_name: impl Into<String>, signaling_port: u16, relay_port: u16) -> Self {
        Self {
            network_name: network_name.into(),
            signaling_port,
            relay_port,
            external_host: None,
            ip_pool: None,
        }
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
    Shutdown {
        reply: oneshot::Sender<usize>,
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
        let (sender, mut receiver) = mpsc::channel::<EmbeddedServiceCommand>(capacity.max(1));

        tokio::spawn(async move {
            let mut services = HashMap::<String, EmbeddedCoordinator>::new();

            while let Some(command) = receiver.recv().await {
                match command {
                    EmbeddedServiceCommand::Start { request, reply } => {
                        let result = start_service(&mut services, request).await;
                        let _ = reply.send(result);
                    }
                    EmbeddedServiceCommand::Stop {
                        network_name,
                        reply,
                    } => {
                        let result = if let Some(coordinator) = services.remove(&network_name) {
                            Ok(EmbeddedServiceSnapshot::from(&coordinator))
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
                    EmbeddedServiceCommand::Shutdown { reply } => {
                        let count = services.len();
                        services.clear();
                        let _ = reply.send(count);
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

    pub async fn shutdown(&self) -> Result<usize, EmbeddedServiceError> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(EmbeddedServiceCommand::Shutdown { reply })
            .await
            .map_err(|_| EmbeddedServiceError::ActorClosed)?;
        response
            .await
            .map_err(|_| EmbeddedServiceError::ActorClosed)
    }
}

async fn start_service(
    services: &mut HashMap<String, EmbeddedCoordinator>,
    request: EmbeddedServiceStartRequest,
) -> Result<EmbeddedServiceSnapshot, EmbeddedServiceError> {
    request.validate()?;
    if services.contains_key(&request.network_name) {
        return Err(EmbeddedServiceError::AlreadyRunning(request.network_name));
    }

    let coordinator = EmbeddedCoordinator::start(
        &request.network_name,
        request.signaling_port,
        request.relay_port,
        request.external_host,
        request.ip_pool.as_deref(),
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

    #[tokio::test]
    async fn actor_owns_service_lifecycle_and_rejects_duplicates() {
        let manager = EmbeddedServiceManager::spawn(4);
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
    async fn shutdown_releases_services_and_closes_actor() {
        let manager = EmbeddedServiceManager::spawn(2);
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
        let manager = EmbeddedServiceManager::spawn(1);
        for name in ["", "../escape", " server", "server "] {
            let error = manager
                .start(EmbeddedServiceStartRequest::new(name, 0, 0))
                .await
                .expect_err("unsafe name must fail");
            assert!(matches!(error, EmbeddedServiceError::InvalidRequest(_)));
        }
        assert!(manager.list().await.expect("list services").is_empty());
    }
}
