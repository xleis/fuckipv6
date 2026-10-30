use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use fuckipv6_common::{ClientConfig, NetworkTicket};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use super::{
    NodeCreateNetworkCommand, NodeEventBus, NodeEventKind, NodeIdentitySnapshot,
    NodeJoinNetworkCommand, NodeLifecycle, NodeNetworkConfigSnapshot, NodeNetworkRole,
    NodeNetworkSelection, NodeNetworkSnapshot, NodeNetworkStatus, NodePhase, NodeProblem,
    NodeProblemCode, NodeRuntimePlan, NodeStateStore, NodeTrafficSnapshot,
};
use crate::cli::commands::{self, RuntimeOptions};
use crate::config_manager::ConfigManager;
use crate::ipc::{MultiNetworkRegistry, NetworkSessionHandle};
use crate::node::runtime_metadata;
use crate::symmetric::{EmbeddedServiceError, EmbeddedServiceManager, EmbeddedServiceStartRequest};

const STOP_TIMEOUT: Duration = Duration::from_secs(15);

type RuntimeFuture<T> = Pin<Box<dyn Future<Output = Result<T, RuntimeControlError>> + Send>>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RuntimeControlError {
    #[error("节点运行时管理器已关闭")]
    ActorClosed,
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Persistence(String),
    #[error("{0}")]
    Launch(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartFailure {
    pub network_name: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StartSummary {
    pub requested: usize,
    pub started: Vec<String>,
    pub already_running: Vec<String>,
    pub failed: Vec<StartFailure>,
}

impl StartSummary {
    pub fn message(&self, action: &str) -> String {
        let mut parts = vec![format!("{action}: 已启动 {} 个网络", self.started.len())];
        if !self.already_running.is_empty() {
            parts.push(format!(
                "{} 个网络已在运行 ({})",
                self.already_running.len(),
                self.already_running.join(", ")
            ));
        }
        if !self.failed.is_empty() {
            let failures = self
                .failed
                .iter()
                .map(|failure| format!("{}: {}", failure.network_name, failure.message))
                .collect::<Vec<_>>()
                .join("; ");
            parts.push(format!("{} 个网络启动失败 ({failures})", self.failed.len()));
        }
        parts.join("；")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StopSummary {
    pub stopped: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CreateResult {
    pub network_name: String,
    pub config_saved: bool,
    pub started: bool,
    pub ticket: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JoinResult {
    pub network_name: String,
    pub coordinator_url: String,
    pub config_saved: bool,
    pub started: bool,
}

#[derive(Debug, Clone)]
struct NetworkLaunchRequest {
    network_name: String,
    config_path: String,
    config: ClientConfig,
}

struct StartedNetworkRuntime {
    tun_name: String,
    assigned_ipv6: std::net::Ipv6Addr,
    assigned_ipv4: Option<std::net::Ipv4Addr>,
    p2p_listen_addr: std::net::SocketAddr,
    identity: NodeIdentitySnapshot,
    shutdown: watch::Sender<bool>,
    handle: JoinHandle<()>,
}

trait NetworkRuntimeLauncher: Send + Sync + 'static {
    fn launch(&self, request: NetworkLaunchRequest) -> RuntimeFuture<StartedNetworkRuntime>;
}

struct LiveNetworkRuntimeLauncher {
    state: NodeStateStore,
    events: NodeEventBus,
    registry: MultiNetworkRegistry,
}

impl NetworkRuntimeLauncher for LiveNetworkRuntimeLauncher {
    fn launch(&self, request: NetworkLaunchRequest) -> RuntimeFuture<StartedNetworkRuntime> {
        let state = self.state.clone();
        let events = self.events.clone();
        let registry = self.registry.clone();

        Box::pin(async move {
            let mut runtime = commands::prepare_client_runtime_ext(
                &request.config_path,
                request.config,
                Some(&request.network_name),
                false,
            )
            .await
            .map_err(|error| {
                RuntimeControlError::Launch(format!(
                    "网络 [{}] 初始化失败: {error:#}",
                    request.network_name
                ))
            })?;

            let tun_name = runtime.tun_name().to_string();
            let assigned_ipv6 = runtime.assigned_ip;
            let assigned_ipv4 = runtime.assigned_ips.ipv4;
            let p2p_listen_addr = runtime.p2p.local_addr();
            let identity = runtime_metadata::identity_snapshot(&runtime.identity);
            let p2p = runtime.p2p.clone();
            let stats = runtime.stats.clone();

            registry
                .register(NetworkSessionHandle {
                    network_name: request.network_name.clone(),
                    tun_name: tun_name.clone(),
                    assigned_ip: assigned_ipv6,
                    assigned_ipv4,
                    p2p,
                    stats,
                })
                .await;

            let (shutdown, mut shutdown_rx) = watch::channel(false);
            let network_name = request.network_name.clone();
            let handle = tokio::spawn(async move {
                tracing::info!("网络 [{network_name}] 正在启动事件循环");
                if let Err(error) = runtime.run_with_shutdown(&mut shutdown_rx).await {
                    tracing::error!("网络 [{network_name}] 事件循环异常退出: {error:#}");
                }
                tracing::info!("网络 [{network_name}] 正在清理资源");
                runtime.shutdown().await;
                registry.unregister(&network_name).await;
                state
                    .update(|snapshot| {
                        if let Some(network) = snapshot
                            .networks
                            .iter_mut()
                            .find(|network| network.name == network_name)
                        {
                            network.status = NodeNetworkStatus::Offline;
                            network.assigned_ipv6 = None;
                            network.assigned_ipv4 = None;
                            network.p2p_listen_addr = None;
                            network.uptime_secs = 0;
                            network.peer_count = 0;
                        }
                    })
                    .await;
                events.publish(NodeEventKind::NetworkChanged {
                    network_name: network_name.clone(),
                    status: NodeNetworkStatus::Offline,
                });
                tracing::info!("网络 [{network_name}] 已安全下线");
            });

            Ok(StartedNetworkRuntime {
                tun_name,
                assigned_ipv6,
                assigned_ipv4,
                p2p_listen_addr,
                identity,
                shutdown,
                handle,
            })
        })
    }
}

struct NetworkRuntimeSession {
    shutdown: watch::Sender<bool>,
    handle: JoinHandle<()>,
}

enum RuntimeControlCommand {
    StartPlan {
        plan: NodeRuntimePlan,
        reply: oneshot::Sender<Result<StartSummary, RuntimeControlError>>,
    },
    StartSelection {
        selection: NodeNetworkSelection,
        reply: oneshot::Sender<Result<StartSummary, RuntimeControlError>>,
    },
    Create {
        command: NodeCreateNetworkCommand,
        reply: oneshot::Sender<Result<CreateResult, RuntimeControlError>>,
    },
    Join {
        command: NodeJoinNetworkCommand,
        reply: oneshot::Sender<Result<JoinResult, RuntimeControlError>>,
    },
    Stop {
        networks: Vec<String>,
        all: bool,
        reply: oneshot::Sender<Result<StopSummary, RuntimeControlError>>,
    },
    Shutdown {
        reply: oneshot::Sender<Result<StopSummary, RuntimeControlError>>,
    },
}

/// Cloneable control handle for the actor that owns live network runtimes.
#[derive(Clone)]
pub(crate) struct NetworkRuntimeManager {
    sender: mpsc::Sender<RuntimeControlCommand>,
}

impl NetworkRuntimeManager {
    pub fn spawn(
        configs: ConfigManager,
        options: RuntimeOptions,
        state: NodeStateStore,
        events: NodeEventBus,
        lifecycle: NodeLifecycle,
        registry: MultiNetworkRegistry,
        services: EmbeddedServiceManager,
    ) -> Self {
        let launcher = Arc::new(LiveNetworkRuntimeLauncher {
            state: state.clone(),
            events: events.clone(),
            registry: registry.clone(),
        });
        Self::spawn_with_launcher(
            configs, options, state, events, lifecycle, services, launcher,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_with_launcher(
        configs: ConfigManager,
        options: RuntimeOptions,
        state: NodeStateStore,
        events: NodeEventBus,
        lifecycle: NodeLifecycle,
        services: EmbeddedServiceManager,
        launcher: Arc<dyn NetworkRuntimeLauncher>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(16);
        tokio::spawn(run_actor(
            receiver,
            NetworkRuntimeActor {
                configs,
                options,
                state,
                events,
                lifecycle,
                services,
                launcher,
                sessions: HashMap::new(),
            },
        ));
        Self { sender }
    }

    pub async fn start_plan(
        &self,
        plan: NodeRuntimePlan,
    ) -> Result<StartSummary, RuntimeControlError> {
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::StartPlan { plan, reply })
            .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn start_selection(
        &self,
        selection: NodeNetworkSelection,
    ) -> Result<StartSummary, RuntimeControlError> {
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::StartSelection { selection, reply })
            .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn create(
        &self,
        command: NodeCreateNetworkCommand,
    ) -> Result<CreateResult, RuntimeControlError> {
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::Create { command, reply })
            .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn join(
        &self,
        command: NodeJoinNetworkCommand,
    ) -> Result<JoinResult, RuntimeControlError> {
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::Join { command, reply })
            .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn stop(
        &self,
        networks: Vec<String>,
        all: bool,
    ) -> Result<StopSummary, RuntimeControlError> {
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::Stop {
            networks,
            all,
            reply,
        })
        .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn shutdown(&self) -> Result<StopSummary, RuntimeControlError> {
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::Shutdown { reply }).await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    async fn send(&self, command: RuntimeControlCommand) -> Result<(), RuntimeControlError> {
        self.sender
            .send(command)
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)
    }
}

async fn run_actor(
    mut receiver: mpsc::Receiver<RuntimeControlCommand>,
    mut actor: NetworkRuntimeActor,
) {
    while let Some(command) = receiver.recv().await {
        match command {
            RuntimeControlCommand::StartPlan { plan, reply } => {
                let result = actor.start_plan(plan).await;
                let _ = reply.send(result);
            }
            RuntimeControlCommand::StartSelection { selection, reply } => {
                let result = actor.start_selection(selection).await;
                let _ = reply.send(result);
            }
            RuntimeControlCommand::Create { command, reply } => {
                let result = actor.create(command).await;
                let _ = reply.send(result);
            }
            RuntimeControlCommand::Join { command, reply } => {
                let result = actor.join(command).await;
                let _ = reply.send(result);
            }
            RuntimeControlCommand::Stop {
                networks,
                all,
                reply,
            } => {
                let result = actor.stop(networks, all).await;
                let _ = reply.send(result);
            }
            RuntimeControlCommand::Shutdown { reply } => {
                let result = actor.shutdown().await;
                let _ = reply.send(result);
                break;
            }
        }
    }
}

struct NetworkRuntimeActor {
    configs: ConfigManager,
    options: RuntimeOptions,
    state: NodeStateStore,
    events: NodeEventBus,
    lifecycle: NodeLifecycle,
    services: EmbeddedServiceManager,
    launcher: Arc<dyn NetworkRuntimeLauncher>,
    sessions: HashMap<String, NetworkRuntimeSession>,
}

impl NetworkRuntimeActor {
    async fn start_selection(
        &mut self,
        selection: NodeNetworkSelection,
    ) -> Result<StartSummary, RuntimeControlError> {
        let plan = NodeRuntimePlan::load(&self.configs, selection).map_err(|error| {
            RuntimeControlError::Persistence(format!("加载网络启动计划失败: {error:#}"))
        })?;
        self.start_plan(plan).await
    }

    async fn start_plan(
        &mut self,
        plan: NodeRuntimePlan,
    ) -> Result<StartSummary, RuntimeControlError> {
        self.prune_finished_sessions();
        let requested = plan.len();
        let mut summary = StartSummary {
            requested,
            ..StartSummary::default()
        };

        for spec in plan.into_networks() {
            if self.sessions.contains_key(&spec.name) {
                summary.already_running.push(spec.name);
                continue;
            }

            let network_name = spec.name.clone();
            match self.start_spec(spec).await {
                Ok(()) => summary.started.push(network_name),
                Err(error) => {
                    summary.failed.push(StartFailure {
                        network_name,
                        message: error.to_string(),
                    });
                }
            }
        }

        self.publish_dynamic_lifecycle().await;
        Ok(summary)
    }

    async fn start_spec(
        &mut self,
        mut spec: super::NodeNetworkSpec,
    ) -> Result<(), RuntimeControlError> {
        commands::apply_runtime_options_to_config(&mut spec.config, &self.options).map_err(
            |error| {
                RuntimeControlError::Invalid(format!(
                    "应用网络 [{}] 启动覆盖失败: {error:#}",
                    spec.name
                ))
            },
        )?;

        let config_snapshot = runtime_metadata::network_config_snapshot(&spec.name, &spec.config);
        self.record_network_config(config_snapshot).await;
        self.record_network(NodeNetworkSnapshot {
            name: spec.name.clone(),
            tun_name: crate::multi_network::derive_tun_name(
                Some(&spec.name),
                &spec.config.tun.name,
            ),
            status: NodeNetworkStatus::Connecting,
            assigned_ipv6: None,
            assigned_ipv4: None,
            p2p_listen_addr: None,
            uptime_secs: 0,
            roles: vec![NodeNetworkRole::Member],
            peer_count: 0,
            traffic: NodeTrafficSnapshot::default(),
        })
        .await;

        let network_name = spec.name.clone();
        let started = self
            .launcher
            .launch(NetworkLaunchRequest {
                network_name: network_name.clone(),
                config_path: spec.config_path.to_string_lossy().into_owned(),
                config: spec.config,
            })
            .await;
        let started = match started {
            Ok(started) => started,
            Err(error) => {
                self.record_launch_failure(&network_name, &error).await;
                return Err(error);
            }
        };

        self.record_identity(started.identity).await;
        self.record_network(NodeNetworkSnapshot {
            name: network_name.clone(),
            tun_name: started.tun_name,
            status: NodeNetworkStatus::Online,
            assigned_ipv6: Some(started.assigned_ipv6),
            assigned_ipv4: started.assigned_ipv4,
            p2p_listen_addr: Some(started.p2p_listen_addr),
            uptime_secs: 0,
            roles: vec![NodeNetworkRole::Member],
            peer_count: 0,
            traffic: NodeTrafficSnapshot::default(),
        })
        .await;
        self.state
            .update(|snapshot| {
                if snapshot
                    .last_error
                    .as_ref()
                    .is_some_and(|problem| problem.network_name.as_deref() == Some(&network_name))
                {
                    snapshot.last_error = None;
                }
            })
            .await;
        self.sessions.insert(
            network_name,
            NetworkRuntimeSession {
                shutdown: started.shutdown,
                handle: started.handle,
            },
        );
        Ok(())
    }

    async fn create(
        &mut self,
        command: NodeCreateNetworkCommand,
    ) -> Result<CreateResult, RuntimeControlError> {
        if self.configs.network_exists(&command.name)
            || self.sessions.contains_key(&command.name)
            || self
                .services
                .list()
                .await
                .map_err(map_service_error)?
                .iter()
                .any(|service| service.network_name == command.name)
        {
            return Err(RuntimeControlError::Conflict(format!(
                "网络 [{}] 已存在或正在运行",
                command.name
            )));
        }

        let service_request = EmbeddedServiceStartRequest::new(
            command.name.clone(),
            command.signaling_port,
            command.relay_port,
        );
        let service_request = match command.external_host.clone() {
            Some(host) => service_request.with_external_host(host),
            None => service_request.with_external_host("127.0.0.1"),
        };
        let service = self
            .services
            .start(service_request)
            .await
            .map_err(map_service_error)?;
        let coordinator_url = format!("ws://127.0.0.1:{}", service.signaling_addr.port());

        let config_file = match self
            .configs
            .create_network(&command.name, &coordinator_url, true)
        {
            Ok(path) => path,
            Err(error) => {
                let _ = self.services.stop(&command.name).await;
                return Err(RuntimeControlError::Persistence(format!(
                    "保存网络 [{}] 配置失败: {error:#}",
                    command.name
                )));
            }
        };

        if command.auto_connect {
            let config = match ClientConfig::from_file(&config_file) {
                Ok(config) => config,
                Err(error) => {
                    self.rollback_created_network(&command.name).await;
                    return Err(RuntimeControlError::Persistence(format!(
                        "读取新网络 [{}] 配置失败，已回滚: {error:#}",
                        command.name
                    )));
                }
            };
            let spec = super::NodeNetworkSpec {
                name: command.name.clone(),
                config_path: config_file,
                config,
            };
            if let Err(error) = self.start_spec(spec).await {
                self.rollback_created_network(&command.name).await;
                return Err(RuntimeControlError::Launch(format!(
                    "网络 [{}] 运行时启动失败，已回滚配置与内嵌服务: {error}",
                    command.name
                )));
            }
        }

        self.publish_dynamic_lifecycle().await;
        Ok(CreateResult {
            network_name: command.name,
            config_saved: true,
            started: command.auto_connect,
            ticket: service.ticket,
        })
    }

    async fn join(
        &mut self,
        command: NodeJoinNetworkCommand,
    ) -> Result<JoinResult, RuntimeControlError> {
        let (coordinator_url, default_name) = parse_join_target(&command.target)?;
        let network_name = command.network_name.unwrap_or(default_name);

        if self.configs.network_exists(&network_name) {
            if !command.start {
                return Err(RuntimeControlError::Conflict(format!(
                    "网络配置已存在: {network_name}"
                )));
            }
            let summary = self
                .start_selection(NodeNetworkSelection::Named(vec![network_name.clone()]))
                .await?;
            if summary.started.is_empty() && !summary.failed.is_empty() {
                return Err(RuntimeControlError::Launch(summary.message("加入网络")));
            }
            return Ok(JoinResult {
                network_name,
                coordinator_url,
                config_saved: false,
                started: !summary.started.is_empty() || !summary.already_running.is_empty(),
            });
        }

        let config_file = self
            .configs
            .create_network(&network_name, &coordinator_url, true)
            .map_err(|error| {
                RuntimeControlError::Persistence(format!(
                    "保存网络 [{network_name}] 配置失败: {error:#}"
                ))
            })?;

        let mut started = false;
        if command.start {
            let config = ClientConfig::from_file(&config_file).map_err(|error| {
                RuntimeControlError::Persistence(format!(
                    "网络 [{network_name}] 已保存，但读取配置失败: {error:#}"
                ))
            })?;
            let spec = super::NodeNetworkSpec {
                name: network_name.clone(),
                config_path: config_file,
                config,
            };
            if let Err(error) = self.start_spec(spec).await {
                return Err(RuntimeControlError::Launch(format!(
                    "网络 [{network_name}] 配置已保存，但启动失败: {error}"
                )));
            }
            started = true;
        }

        self.publish_dynamic_lifecycle().await;
        Ok(JoinResult {
            network_name,
            coordinator_url,
            config_saved: true,
            started,
        })
    }

    async fn stop(
        &mut self,
        networks: Vec<String>,
        all: bool,
    ) -> Result<StopSummary, RuntimeControlError> {
        self.prune_finished_sessions();
        let services = self
            .services
            .list()
            .await
            .map_err(map_service_error)?
            .into_iter()
            .map(|service| service.network_name)
            .collect::<BTreeSet<_>>();
        let targets = if all {
            self.sessions
                .keys()
                .cloned()
                .chain(services)
                .collect::<BTreeSet<_>>()
        } else {
            let requested = networks.into_iter().collect::<BTreeSet<_>>();
            let missing = requested
                .iter()
                .filter(|name| !self.sessions.contains_key(*name) && !services.contains(*name))
                .cloned()
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                return Err(RuntimeControlError::NotFound(format!(
                    "网络未运行: {}",
                    missing.join(", ")
                )));
            }
            requested
        };

        let mut stopped = Vec::with_capacity(targets.len());
        for name in targets {
            if let Some(mut session) = self.sessions.remove(&name) {
                self.mark_network_status(Some(&name), NodeNetworkStatus::Stopping)
                    .await;
                let _ = session.shutdown.send(true);
                if tokio::time::timeout(STOP_TIMEOUT, &mut session.handle)
                    .await
                    .is_err()
                {
                    session.handle.abort();
                    let _ = session.handle.await;
                }
                self.mark_network_status(Some(&name), NodeNetworkStatus::Offline)
                    .await;
            }
            match self.services.stop(&name).await {
                Ok(_) | Err(EmbeddedServiceError::NotFound(_)) => {}
                Err(error) => return Err(map_service_error(error)),
            }
            stopped.push(name);
        }

        self.publish_dynamic_lifecycle().await;
        Ok(StopSummary { stopped })
    }

    async fn shutdown(&mut self) -> Result<StopSummary, RuntimeControlError> {
        let summary = self.stop(Vec::new(), true).await?;
        match self.services.shutdown().await {
            Ok(_) | Err(EmbeddedServiceError::ActorClosed) => Ok(summary),
            Err(error) => Err(map_service_error(error)),
        }
    }

    fn prune_finished_sessions(&mut self) {
        self.sessions
            .retain(|_, session| !session.handle.is_finished());
    }

    async fn rollback_created_network(&mut self, network_name: &str) {
        let _ = self.configs.delete_network(network_name);
        let _ = self.services.stop(network_name).await;
        self.forget_network(network_name).await;
    }

    async fn forget_network(&self, network_name: &str) {
        self.state
            .update(|snapshot| {
                snapshot
                    .networks
                    .retain(|network| network.name != network_name);
                snapshot
                    .configuration
                    .networks
                    .retain(|network| network.network_name != network_name);
                if snapshot
                    .last_error
                    .as_ref()
                    .is_some_and(|problem| problem.network_name.as_deref() == Some(network_name))
                {
                    snapshot.last_error = None;
                }
            })
            .await;
    }

    async fn record_launch_failure(&self, network_name: &str, error: &RuntimeControlError) {
        self.mark_network_status(Some(network_name), NodeNetworkStatus::Failed)
            .await;
        self.state
            .update(|snapshot| {
                snapshot.last_error = Some(
                    NodeProblem::new(NodeProblemCode::Unavailable, error.to_string())
                        .for_network(network_name)
                        .retryable(true),
                );
            })
            .await;
    }

    async fn record_identity(&self, identity: NodeIdentitySnapshot) {
        if let Some(existing) = self.state.snapshot().await.identity {
            if existing != identity {
                tracing::warn!(
                    "节点运行时检测到不同的网络身份，共享节点快照继续使用首次加载的身份"
                );
            }
            return;
        }

        self.state
            .update(|snapshot| {
                if snapshot.identity.is_none() {
                    snapshot.identity = Some(identity);
                }
            })
            .await;
    }

    async fn record_network_config(&self, config: NodeNetworkConfigSnapshot) {
        self.state
            .update(|snapshot| {
                if let Some(existing) = snapshot
                    .configuration
                    .networks
                    .iter_mut()
                    .find(|existing| existing.network_name == config.network_name)
                {
                    *existing = config;
                } else {
                    snapshot.configuration.networks.push(config);
                }
            })
            .await;
    }

    async fn record_network(&self, network: NodeNetworkSnapshot) {
        let network_name = network.name.clone();
        let status = network.status;
        self.state
            .update(|snapshot| {
                if let Some(existing) = snapshot
                    .networks
                    .iter_mut()
                    .find(|existing| existing.name == network.name)
                {
                    *existing = network;
                } else {
                    snapshot.networks.push(network);
                }
            })
            .await;
        self.events.publish(NodeEventKind::NetworkChanged {
            network_name,
            status,
        });
    }

    async fn mark_network_status(&self, network_name: Option<&str>, status: NodeNetworkStatus) {
        let snapshot = self
            .state
            .update(|snapshot| {
                for network in &mut snapshot.networks {
                    if network_name.is_none_or(|name| network.name == name) {
                        network.status = status;
                    }
                }
            })
            .await;
        for network in snapshot.networks.iter().filter(|network| {
            network_name.is_none_or(|name| network.name == name) && network.status == status
        }) {
            self.events.publish(NodeEventKind::NetworkChanged {
                network_name: network.name.clone(),
                status,
            });
        }
    }

    async fn publish_dynamic_lifecycle(&self) {
        if self.lifecycle.snapshot().await.phase != NodePhase::Running {
            return;
        }
        let configured = self
            .configs
            .list_networks()
            .map_or(0, |networks| networks.len());
        let snapshot = self.state.snapshot().await;
        let failed = snapshot
            .networks
            .iter()
            .filter(|network| network.status == NodeNetworkStatus::Failed)
            .map(|network| network.name.clone())
            .collect();
        let lifecycle = self
            .lifecycle
            .running_with_configuration(configured, self.sessions.len(), failed)
            .await;
        self.state.set_lifecycle(lifecycle).await;
    }
}

fn parse_join_target(target: &str) -> Result<(String, String), RuntimeControlError> {
    let target = target.trim();
    if target.starts_with(NetworkTicket::URI_SCHEME) {
        let ticket = NetworkTicket::parse(target).map_err(|error| {
            RuntimeControlError::Invalid(format!("解析网络邀请票据失败: {error:#}"))
        })?;
        return Ok((ticket.websocket_url(), ticket.network_name));
    }

    if target.starts_with("ws://") || target.starts_with("wss://") {
        return Ok((
            target.to_string(),
            ConfigManager::extract_network_name(target),
        ));
    }

    let coordinator_url = if target.contains(':') {
        format!("ws://{target}")
    } else {
        format!("ws://{target}:6666")
    };
    Ok((
        coordinator_url.clone(),
        ConfigManager::extract_network_name(&coordinator_url),
    ))
}

fn map_service_error(error: EmbeddedServiceError) -> RuntimeControlError {
    match error {
        EmbeddedServiceError::InvalidRequest(message) => RuntimeControlError::Invalid(message),
        EmbeddedServiceError::AlreadyRunning(name) => {
            RuntimeControlError::Conflict(format!("网络 [{name}] 的内嵌服务已在运行"))
        }
        EmbeddedServiceError::NotFound(name) => {
            RuntimeControlError::NotFound(format!("网络 [{name}] 的内嵌服务未运行"))
        }
        EmbeddedServiceError::StartFailed {
            network_name,
            message,
        } => RuntimeControlError::Launch(format!(
            "网络 [{network_name}] 的内嵌服务启动失败: {message}"
        )),
        EmbeddedServiceError::ActorClosed => RuntimeControlError::ActorClosed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::Mutex;

    #[derive(Clone, Default)]
    struct FakeLauncher {
        launches: Arc<Mutex<Vec<String>>>,
        failures: Arc<Mutex<HashSet<String>>>,
        shutdowns: Arc<AtomicUsize>,
    }

    impl FakeLauncher {
        fn fail(&self, network_name: &str) {
            self.failures
                .lock()
                .expect("failure lock poisoned")
                .insert(network_name.to_string());
        }

        fn recover(&self, network_name: &str) {
            self.failures
                .lock()
                .expect("failure lock poisoned")
                .remove(network_name);
        }

        fn launches(&self) -> Vec<String> {
            self.launches.lock().expect("launch lock poisoned").clone()
        }

        fn shutdown_count(&self) -> usize {
            self.shutdowns.load(Ordering::SeqCst)
        }
    }

    impl NetworkRuntimeLauncher for FakeLauncher {
        fn launch(&self, request: NetworkLaunchRequest) -> RuntimeFuture<StartedNetworkRuntime> {
            let launches = self.launches.clone();
            let failures = self.failures.clone();
            let shutdowns = self.shutdowns.clone();

            Box::pin(async move {
                if failures
                    .lock()
                    .expect("failure lock poisoned")
                    .contains(&request.network_name)
                {
                    return Err(RuntimeControlError::Launch(format!(
                        "网络 [{}] 测试启动失败",
                        request.network_name
                    )));
                }

                launches
                    .lock()
                    .expect("launch lock poisoned")
                    .push(request.network_name.clone());
                let (shutdown, mut shutdown_rx) = watch::channel(false);
                let handle = tokio::spawn(async move {
                    let _ = shutdown_rx.changed().await;
                    shutdowns.fetch_add(1, Ordering::SeqCst);
                });

                Ok(StartedNetworkRuntime {
                    tun_name: crate::multi_network::derive_tun_name(
                        Some(&request.network_name),
                        &request.config.tun.name,
                    ),
                    assigned_ipv6: Ipv6Addr::LOCALHOST,
                    assigned_ipv4: Some(Ipv4Addr::LOCALHOST),
                    p2p_listen_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                    identity: NodeIdentitySnapshot {
                        node_id: format!("node-{}", request.network_name),
                        display_name: None,
                        public_key_fingerprint: format!("fingerprint-{}", request.network_name),
                    },
                    shutdown,
                    handle,
                })
            })
        }
    }

    struct TestHarness {
        configs: ConfigManager,
        state: NodeStateStore,
        lifecycle: NodeLifecycle,
        services: EmbeddedServiceManager,
        runtime: NetworkRuntimeManager,
        fake: FakeLauncher,
        config_dir: PathBuf,
    }

    impl TestHarness {
        async fn new(label: &str) -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let config_dir = std::env::temp_dir().join(format!(
                "fuckipv6-runtime-control-{label}-{}-{id}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&config_dir);
            let configs = ConfigManager::new_for_dir(config_dir.clone()).expect("config manager");

            let events = NodeEventBus::default();
            let state = NodeStateStore::with_events(events.clone());
            let lifecycle = NodeLifecycle::new();
            let running = lifecycle.running(0, Vec::new()).await;
            state.set_lifecycle(running).await;
            let services = EmbeddedServiceManager::spawn(4);
            let fake = FakeLauncher::default();
            let runtime = NetworkRuntimeManager::spawn_with_launcher(
                configs.clone(),
                RuntimeOptions::default(),
                state.clone(),
                events,
                lifecycle.clone(),
                services.clone(),
                Arc::new(fake.clone()),
            );

            Self {
                configs,
                state,
                lifecycle,
                services,
                runtime,
                fake,
                config_dir,
            }
        }

        async fn shutdown(&self) {
            let _ = self.runtime.shutdown().await;
            let _ = std::fs::remove_dir_all(&self.config_dir);
        }
    }

    fn create_command(name: &str, auto_connect: bool) -> NodeCreateNetworkCommand {
        NodeCreateNetworkCommand {
            name: name.into(),
            signaling_port: 0,
            relay_port: 0,
            external_host: None,
            auto_connect,
        }
    }

    #[tokio::test]
    async fn start_plan_reports_partial_failure_and_retries_only_failed_networks() {
        let harness = TestHarness::new("start-plan").await;
        harness
            .configs
            .create_network("home", "ws://home.example:6666", true)
            .expect("create home");
        harness
            .configs
            .create_network("office", "ws://office.example:6666", false)
            .expect("create office");
        harness.fake.fail("office");
        let plan = NodeRuntimePlan::load(
            &harness.configs,
            NodeNetworkSelection::Named(vec!["home".into(), "office".into()]),
        )
        .expect("load plan");

        let first = harness
            .runtime
            .start_plan(plan.clone())
            .await
            .expect("start first plan");
        assert_eq!(first.requested, 2);
        assert_eq!(first.started, vec!["home"]);
        assert_eq!(first.failed.len(), 1);
        assert_eq!(first.failed[0].network_name, "office");

        let snapshot = harness.state.snapshot().await;
        assert_eq!(snapshot.lifecycle.active_networks, 1);
        assert_eq!(snapshot.lifecycle.failed_networks, vec!["office"]);
        assert!(snapshot.networks.iter().any(|network| {
            network.name == "home" && network.status == NodeNetworkStatus::Online
        }));
        assert!(snapshot.networks.iter().any(|network| {
            network.name == "office" && network.status == NodeNetworkStatus::Failed
        }));

        harness.fake.recover("office");
        let second = harness
            .runtime
            .start_plan(plan)
            .await
            .expect("retry failed network");
        assert_eq!(second.started, vec!["office"]);
        assert_eq!(second.already_running, vec!["home"]);
        assert!(second.failed.is_empty());
        assert_eq!(harness.fake.launches(), vec!["home", "office"]);

        let snapshot = harness.state.snapshot().await;
        assert_eq!(snapshot.lifecycle.active_networks, 2);
        assert!(snapshot.lifecycle.failed_networks.is_empty());
        assert_eq!(harness.lifecycle.snapshot().await.active_networks, 2);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn stop_network_releases_runtime_and_owned_service() {
        let harness = TestHarness::new("stop-owned").await;
        let created = harness
            .runtime
            .create(create_command("owned", true))
            .await
            .expect("create connected network");
        assert!(created.config_saved);
        assert!(created.started);
        assert_eq!(
            harness.services.list().await.expect("list services").len(),
            1
        );

        let stopped = harness
            .runtime
            .stop(vec!["owned".into()], false)
            .await
            .expect("stop network");
        assert_eq!(stopped.stopped, vec!["owned"]);
        assert_eq!(harness.fake.shutdown_count(), 1);
        assert!(harness
            .services
            .list()
            .await
            .expect("list services")
            .is_empty());
        assert!(harness.configs.network_exists("owned"));

        let snapshot = harness.state.snapshot().await;
        assert!(snapshot.networks.iter().any(|network| {
            network.name == "owned" && network.status == NodeNetworkStatus::Offline
        }));
        assert_eq!(snapshot.lifecycle.active_networks, 0);
        assert_eq!(snapshot.lifecycle.configured_networks, 1);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn create_without_auto_connect_keeps_service_until_network_stop() {
        let harness = TestHarness::new("create-detached").await;
        let created = harness
            .runtime
            .create(create_command("detached", false))
            .await
            .expect("create detached network");
        assert!(created.config_saved);
        assert!(!created.started);
        assert!(created.ticket.starts_with(NetworkTicket::URI_SCHEME));
        assert_eq!(
            harness.services.list().await.expect("list services").len(),
            1
        );
        assert!(harness.state.snapshot().await.networks.is_empty());

        harness
            .runtime
            .stop(vec!["detached".into()], false)
            .await
            .expect("stop detached service");
        assert!(harness
            .services
            .list()
            .await
            .expect("list services")
            .is_empty());
        assert!(harness.configs.network_exists("detached"));

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn create_failure_rolls_back_config_service_and_state() {
        let harness = TestHarness::new("create-rollback").await;
        harness.fake.fail("broken");

        let error = harness
            .runtime
            .create(create_command("broken", true))
            .await
            .expect_err("launch must fail");
        assert!(matches!(error, RuntimeControlError::Launch(_)));
        assert!(!harness.configs.network_exists("broken"));
        assert!(harness
            .services
            .list()
            .await
            .expect("list services")
            .is_empty());

        let snapshot = harness.state.snapshot().await;
        assert!(!snapshot
            .networks
            .iter()
            .any(|network| network.name == "broken"));
        assert!(!snapshot
            .configuration
            .networks
            .iter()
            .any(|network| network.network_name == "broken"));

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn join_ticket_parses_network_and_persists_coordinator() {
        let harness = TestHarness::new("join-ticket").await;
        let ticket = NetworkTicket::new([7u8; 32], "127.0.0.1:6666", "invited").encode();

        let joined = harness
            .runtime
            .join(NodeJoinNetworkCommand {
                target: ticket,
                network_name: None,
                start: false,
            })
            .await
            .expect("join ticket");
        assert_eq!(joined.network_name, "invited");
        assert_eq!(joined.coordinator_url, "ws://127.0.0.1:6666");
        assert!(joined.config_saved);
        assert!(!joined.started);

        let config_path = harness
            .configs
            .get_network_config("invited")
            .expect("persisted network config");
        let config = ClientConfig::from_file(config_path).expect("load persisted config");
        assert_eq!(config.coordinator_url, "ws://127.0.0.1:6666");

        harness.shutdown().await;
    }
}
