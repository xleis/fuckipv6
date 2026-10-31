use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use fuckipv6_common::{ClientConfig, NetworkTicket};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use super::{
    NodeCreateNetworkCommand, NodeEmbeddedServiceSnapshot, NodeEmbeddedServiceStatus, NodeEventBus,
    NodeEventKind, NodeIdentitySnapshot, NodeJoinNetworkCommand, NodeLifecycle,
    NodeNetworkConfigSnapshot, NodeNetworkRole, NodeNetworkSelection, NodeNetworkSnapshot,
    NodeNetworkStatus, NodePhase, NodeProblem, NodeProblemCode, NodeRoutingMode, NodeRuntimePlan,
    NodeSetNetworkRoleCommand, NodeStateStore, NodeTrafficSnapshot, NodeUpdateRoutingCommand,
};
use crate::cli::commands::{self, RuntimeOptions};
use crate::config_manager::ConfigManager;
use crate::ipc::{self, MultiNetworkRegistry, NetworkSessionHandle};
use crate::node::runtime_metadata;
use crate::symmetric::{
    EmbeddedServiceError, EmbeddedServiceManager, EmbeddedServiceSnapshot,
    EmbeddedServiceStartRequest,
};

const STOP_TIMEOUT: Duration = Duration::from_secs(15);
const ROUTING_UPDATE_TIMEOUT: Duration = Duration::from_secs(20);

type RuntimeFuture<T> = Pin<Box<dyn Future<Output = Result<T, RuntimeControlError>> + Send>>;

#[derive(Debug, Clone, PartialEq, Eq)]
enum RuntimeRoutingRequestError {
    ControlClosed,
    TimedOut(Duration),
    ResponseClosed,
    Rejected(String),
}

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
    Unavailable(String),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RoleUpdateResult {
    pub network_name: String,
    pub role: NodeNetworkRole,
    pub enabled: bool,
    pub roles: Vec<NodeNetworkRole>,
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
    routing_control: mpsc::UnboundedSender<ipc::RuntimeCommand>,
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
            let identity = runtime_metadata::identity_snapshot(&runtime.identity, None);
            let p2p = runtime.p2p.clone();
            let stats = runtime.stats.clone();

            // Multi-network nodes disable each runtime's private IPC listener.
            // Replace its unused control receiver with the actor-owned channel so
            // routing changes are applied by the existing live event loop.
            let (routing_control, routing_control_rx) = mpsc::unbounded_channel();
            runtime.control_rx = routing_control_rx;

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
                        snapshot
                            .peers
                            .retain(|peer| peer.network_name != network_name);
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
                routing_control,
                shutdown,
                handle,
            })
        })
    }
}

struct NetworkRuntimeSession {
    routing_control: mpsc::UnboundedSender<ipc::RuntimeCommand>,
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
    UpdateRouting {
        command: NodeUpdateRoutingCommand,
        reply: oneshot::Sender<Result<ipc::RoutingUpdateResult, RuntimeControlError>>,
    },
    SetNetworkRole {
        command: NodeSetNetworkRoleCommand,
        reply: oneshot::Sender<Result<RoleUpdateResult, RuntimeControlError>>,
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
        Self::spawn_with_launcher_and_timeout(
            configs,
            options,
            state,
            events,
            lifecycle,
            services,
            launcher,
            ROUTING_UPDATE_TIMEOUT,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_with_launcher_and_timeout(
        configs: ConfigManager,
        options: RuntimeOptions,
        state: NodeStateStore,
        events: NodeEventBus,
        lifecycle: NodeLifecycle,
        services: EmbeddedServiceManager,
        launcher: Arc<dyn NetworkRuntimeLauncher>,
        routing_update_timeout: Duration,
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
                routing_update_timeout,
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

    pub async fn update_routing(
        &self,
        command: NodeUpdateRoutingCommand,
    ) -> Result<ipc::RoutingUpdateResult, RuntimeControlError> {
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::UpdateRouting { command, reply })
            .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn set_network_role(
        &self,
        command: NodeSetNetworkRoleCommand,
    ) -> Result<RoleUpdateResult, RuntimeControlError> {
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::SetNetworkRole { command, reply })
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
            RuntimeControlCommand::UpdateRouting { command, reply } => {
                let result = actor.update_routing(command).await;
                let _ = reply.send(result);
            }
            RuntimeControlCommand::SetNetworkRole { command, reply } => {
                let result = actor.set_network_role(command).await;
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
    routing_update_timeout: Duration,
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
        let initial_roles = roles_from_config(&spec.config);
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
            roles: initial_roles.clone(),
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
            roles: initial_roles,
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
                routing_control: started.routing_control,
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
        self.mark_embedded_service_status(&command.name, NodeEmbeddedServiceStatus::Starting)
            .await;
        let service = match self.services.start(service_request).await {
            Ok(service) => service,
            Err(error) => {
                self.mark_embedded_service_status(&command.name, NodeEmbeddedServiceStatus::Failed)
                    .await;
                return Err(map_service_error(error));
            }
        };
        self.record_running_embedded_service(&service).await;
        let coordinator_url = format!("ws://127.0.0.1:{}", service.signaling_addr.port());

        let config_file = match self
            .configs
            .create_network(&command.name, &coordinator_url, true)
        {
            Ok(path) => path,
            Err(error) => {
                self.stop_embedded_service(&command.name).await;
                self.forget_network(&command.name).await;
                return Err(RuntimeControlError::Persistence(format!(
                    "保存网络 [{}] 配置失败: {error:#}",
                    command.name
                )));
            }
        };
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
        self.record_network_config(runtime_metadata::network_config_snapshot(
            &command.name,
            &config,
        ))
        .await;

        if command.auto_connect {
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

    async fn update_routing(
        &mut self,
        command: NodeUpdateRoutingCommand,
    ) -> Result<ipc::RoutingUpdateResult, RuntimeControlError> {
        self.prune_finished_sessions();

        let network_name = command.network_name.clone();
        let routing_control = self
            .sessions
            .get(&network_name)
            .map(|session| session.routing_control.clone())
            .ok_or_else(|| RuntimeControlError::NotFound(format!("网络未运行: {network_name}")))?;

        let previous = self
            .state
            .snapshot()
            .await
            .configuration
            .networks
            .into_iter()
            .find(|network| network.network_name == network_name)
            .ok_or_else(|| {
                RuntimeControlError::Persistence(format!(
                    "网络 [{network_name}] 的共享配置快照缺失，未应用路由更新"
                ))
            })?;
        let previous_roles = self
            .state
            .snapshot()
            .await
            .networks
            .into_iter()
            .find(|network| network.name == network_name)
            .map(|network| network.roles)
            .ok_or_else(|| {
                RuntimeControlError::Persistence(format!(
                    "网络 [{network_name}] 的共享运行快照缺失，未应用路由更新"
                ))
            })?;
        ensure_member_role(&network_name, &previous_roles)?;

        let result = match request_runtime_routing_update(
            &routing_control,
            &command,
            self.routing_update_timeout,
        )
        .await
        {
            Ok(result) => result,
            Err(RuntimeRoutingRequestError::ControlClosed) => {
                return Err(RuntimeControlError::NotFound(format!(
                    "网络 [{network_name}] 的运行时控制通道已关闭"
                )));
            }
            Err(failure) => {
                let failure_message = describe_routing_request_failure(&failure);
                let rollback = rollback_runtime_routing(
                    &routing_control,
                    &previous,
                    self.routing_update_timeout,
                )
                .await;
                return Err(match rollback {
                    Ok(()) => RuntimeControlError::Persistence(format!(
                        "网络 [{network_name}] 路由热更新失败: {failure_message}；已通过运行时重新应用旧配置，文件和运行态已回滚"
                    )),
                    Err(rollback_error) => RuntimeControlError::Persistence(format!(
                        "网络 [{network_name}] 路由热更新失败: {failure_message}；旧配置回滚未能确认: {rollback_error}"
                    )),
                });
            }
        };

        let mut updated = previous.clone();
        updated.routing_mode = command.mode;
        updated.exit_node = command.exit_node.clone();
        updated.exit_node_enabled = command.is_exit_node;

        let mut updated_roles = previous_roles.clone();
        set_role_enabled(
            &mut updated_roles,
            NodeNetworkRole::ExitNode,
            command.is_exit_node,
        );

        if !self
            .publish_role_projection(updated, updated_roles.clone())
            .await
        {
            let rollback =
                rollback_runtime_routing(&routing_control, &previous, self.routing_update_timeout)
                    .await;
            // The failed publish did not replace an existing entry. Restore the
            // previous projection if a concurrent state transition removed it
            // while the runtime update was in flight.
            self.restore_role_projection(previous, previous_roles).await;
            return Err(match rollback {
                Ok(()) => RuntimeControlError::Persistence(format!(
                    "网络 [{network_name}] 的共享快照发布失败，运行态、持久化配置和共享快照已回滚"
                )),
                Err(rollback_error) => RuntimeControlError::Persistence(format!(
                    "网络 [{network_name}] 的共享快照发布失败，且运行态回滚失败: {rollback_error}"
                )),
            });
        }

        self.events.publish(NodeEventKind::RoutingChanged {
            network_name,
            mode: command.mode,
            exit_node: command.exit_node,
            is_exit_node: command.is_exit_node,
        });
        if updated_roles != previous_roles {
            self.events.publish(NodeEventKind::RoleChanged {
                network_name: command.network_name,
                role: NodeNetworkRole::ExitNode,
                enabled: command.is_exit_node,
                roles: updated_roles,
            });
        }

        Ok(result)
    }

    async fn set_network_role(
        &mut self,
        command: NodeSetNetworkRoleCommand,
    ) -> Result<RoleUpdateResult, RuntimeControlError> {
        self.prune_finished_sessions();

        match command.role {
            NodeNetworkRole::Member => {
                return Err(RuntimeControlError::Conflict(
                    "成员角色是节点网络内的基线能力，不能单独启用或禁用".into(),
                ));
            }
            NodeNetworkRole::Discovery | NodeNetworkRole::Relay => {
                return Err(RuntimeControlError::Unavailable(format!(
                    "{:?} 角色目前只能随内嵌 Discovery+Relay 服务整体创建/停止；尚未提供不重启网络即可独立切换该能力的后端，未修改任何状态",
                    command.role
                )));
            }
            NodeNetworkRole::ExitNode => {}
        }

        let network_name = command.network_name.clone();
        let routing_control = self
            .sessions
            .get(&network_name)
            .map(|session| session.routing_control.clone())
            .ok_or_else(|| RuntimeControlError::NotFound(format!("网络未运行: {network_name}")))?;

        let before = self.state.snapshot().await;
        let previous_config = before
            .configuration
            .networks
            .iter()
            .find(|network| network.network_name == network_name)
            .cloned()
            .ok_or_else(|| {
                RuntimeControlError::Persistence(format!(
                    "网络 [{network_name}] 的共享配置快照缺失，未应用角色更新"
                ))
            })?;
        let previous_roles = before
            .networks
            .iter()
            .find(|network| network.name == network_name)
            .map(|network| network.roles.clone())
            .ok_or_else(|| {
                RuntimeControlError::Persistence(format!(
                    "网络 [{network_name}] 的共享运行快照缺失，未应用角色更新"
                ))
            })?;
        ensure_member_role(&network_name, &previous_roles)?;

        let mut desired_roles = previous_roles.clone();
        set_role_enabled(&mut desired_roles, command.role, command.enabled);
        let routing_command = NodeUpdateRoutingCommand {
            network_name: network_name.clone(),
            mode: previous_config.routing_mode,
            exit_node: previous_config.exit_node.clone(),
            is_exit_node: desired_roles.contains(&NodeNetworkRole::ExitNode),
        };

        match request_runtime_routing_update(
            &routing_control,
            &routing_command,
            self.routing_update_timeout,
        )
        .await
        {
            Ok(_) => {}
            Err(RuntimeRoutingRequestError::ControlClosed) => {
                return Err(RuntimeControlError::NotFound(format!(
                    "网络 [{network_name}] 的运行时控制通道已关闭"
                )));
            }
            Err(failure) => {
                let failure_message = describe_routing_request_failure(&failure);
                let rollback = rollback_runtime_routing(
                    &routing_control,
                    &previous_config,
                    self.routing_update_timeout,
                )
                .await;
                return Err(match rollback {
                    Ok(()) => RuntimeControlError::Persistence(format!(
                        "网络 [{network_name}] 角色运行态更新失败: {failure_message}；已重新应用旧配置，运行态与持久化配置已回滚"
                    )),
                    Err(rollback_error) => RuntimeControlError::Persistence(format!(
                        "网络 [{network_name}] 角色运行态更新失败: {failure_message}；旧配置回滚未能确认: {rollback_error}"
                    )),
                });
            }
        }

        let mut updated_config = previous_config.clone();
        updated_config.exit_node_enabled = desired_roles.contains(&NodeNetworkRole::ExitNode);
        if !self
            .publish_role_projection(updated_config, desired_roles.clone())
            .await
        {
            let rollback = rollback_runtime_routing(
                &routing_control,
                &previous_config,
                self.routing_update_timeout,
            )
            .await;
            self.restore_role_projection(previous_config, previous_roles.clone())
                .await;
            return Err(match rollback {
                Ok(()) => RuntimeControlError::Persistence(format!(
                    "网络 [{network_name}] 的角色共享快照发布失败，运行态、持久化配置和共享快照已回滚"
                )),
                Err(rollback_error) => RuntimeControlError::Persistence(format!(
                    "网络 [{network_name}] 的角色共享快照发布失败，且运行态回滚失败: {rollback_error}"
                )),
            });
        }

        self.events.publish(NodeEventKind::RoleChanged {
            network_name: network_name.clone(),
            role: command.role,
            enabled: command.enabled,
            roles: desired_roles.clone(),
        });

        Ok(RoleUpdateResult {
            network_name,
            role: command.role,
            enabled: command.enabled,
            roles: desired_roles,
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
                .chain(services.iter().cloned())
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
            let has_embedded_service = services.contains(&name);
            if has_embedded_service {
                self.mark_embedded_service_status(&name, NodeEmbeddedServiceStatus::Stopping)
                    .await;
            }
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
            if has_embedded_service {
                match self.services.stop(&name).await {
                    Ok(_) | Err(EmbeddedServiceError::NotFound(_)) => {
                        self.mark_embedded_service_status(
                            &name,
                            NodeEmbeddedServiceStatus::Stopped,
                        )
                        .await;
                    }
                    Err(error) => {
                        self.mark_embedded_service_status(&name, NodeEmbeddedServiceStatus::Failed)
                            .await;
                        return Err(map_service_error(error));
                    }
                }
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
        self.stop_embedded_service(network_name).await;
        self.forget_network(network_name).await;
    }

    async fn stop_embedded_service(&self, network_name: &str) {
        self.mark_embedded_service_status(network_name, NodeEmbeddedServiceStatus::Stopping)
            .await;
        match self.services.stop(network_name).await {
            Ok(_) | Err(EmbeddedServiceError::NotFound(_)) => {
                self.mark_embedded_service_status(network_name, NodeEmbeddedServiceStatus::Stopped)
                    .await;
            }
            Err(error) => {
                self.mark_embedded_service_status(network_name, NodeEmbeddedServiceStatus::Failed)
                    .await;
                tracing::warn!("网络 [{network_name}] 内嵌服务停止失败: {error}");
            }
        }
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
                snapshot
                    .embedded_services
                    .retain(|service| service.network_name != network_name);
                snapshot
                    .peers
                    .retain(|peer| peer.network_name != network_name);
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

    async fn record_running_embedded_service(&self, service: &EmbeddedServiceSnapshot) {
        self.record_embedded_service(NodeEmbeddedServiceSnapshot {
            network_name: service.network_name.clone(),
            status: NodeEmbeddedServiceStatus::Running,
            signaling_addr: Some(service.signaling_addr),
            relay_addr: Some(service.relay_addr),
        })
        .await;
    }

    async fn mark_embedded_service_status(
        &self,
        network_name: &str,
        status: NodeEmbeddedServiceStatus,
    ) {
        self.record_embedded_service(NodeEmbeddedServiceSnapshot {
            network_name: network_name.to_string(),
            status,
            signaling_addr: None,
            relay_addr: None,
        })
        .await;
    }

    async fn record_embedded_service(&self, service: NodeEmbeddedServiceSnapshot) {
        let network_name = service.network_name.clone();
        let status = service.status;
        self.state
            .update(|snapshot| {
                if let Some(existing) = snapshot
                    .embedded_services
                    .iter_mut()
                    .find(|existing| existing.network_name == service.network_name)
                {
                    *existing = service;
                } else {
                    snapshot.embedded_services.push(service);
                }
                snapshot
                    .embedded_services
                    .sort_by(|left, right| left.network_name.cmp(&right.network_name));
            })
            .await;
        self.events.publish(NodeEventKind::EmbeddedServiceChanged {
            network_name,
            status,
        });
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

    async fn publish_role_projection(
        &self,
        config: NodeNetworkConfigSnapshot,
        roles: Vec<NodeNetworkRole>,
    ) -> bool {
        let mut published = false;
        self.state
            .update(|snapshot| {
                let has_config = snapshot
                    .configuration
                    .networks
                    .iter()
                    .any(|network| network.network_name == config.network_name);
                let has_runtime = snapshot
                    .networks
                    .iter()
                    .any(|network| network.name == config.network_name);
                if !has_config || !has_runtime {
                    return;
                }

                if let Some(existing) = snapshot
                    .configuration
                    .networks
                    .iter_mut()
                    .find(|network| network.network_name == config.network_name)
                {
                    *existing = config.clone();
                }
                if let Some(existing) = snapshot
                    .networks
                    .iter_mut()
                    .find(|network| network.name == config.network_name)
                {
                    existing.roles = roles.clone();
                }
                published = true;
            })
            .await;
        published
    }

    async fn restore_role_projection(
        &self,
        config: NodeNetworkConfigSnapshot,
        roles: Vec<NodeNetworkRole>,
    ) {
        let network_name = config.network_name.clone();
        self.state
            .update(|snapshot| {
                if let Some(existing) = snapshot
                    .configuration
                    .networks
                    .iter_mut()
                    .find(|network| network.network_name == network_name)
                {
                    *existing = config;
                } else {
                    snapshot.configuration.networks.push(config);
                }
                if let Some(existing) = snapshot
                    .networks
                    .iter_mut()
                    .find(|network| network.name == network_name)
                {
                    existing.roles = roles;
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
                if status == NodeNetworkStatus::Offline {
                    snapshot
                        .peers
                        .retain(|peer| network_name.is_none_or(|name| peer.network_name != name));
                }
                for network in &mut snapshot.networks {
                    if network_name.is_none_or(|name| network.name == name) {
                        network.status = status;
                        if status == NodeNetworkStatus::Offline {
                            network.peer_count = 0;
                        }
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

async fn request_runtime_routing_update(
    routing_control: &mpsc::UnboundedSender<ipc::RuntimeCommand>,
    command: &NodeUpdateRoutingCommand,
    wait: Duration,
) -> Result<ipc::RoutingUpdateResult, RuntimeRoutingRequestError> {
    let (reply, response) = oneshot::channel();
    routing_control
        .send(ipc::RuntimeCommand::UpdateRouting {
            mode: runtime_routing_mode_name(command.mode).into(),
            exit_node: command.exit_node.clone(),
            is_exit_node: command.is_exit_node,
            reply,
        })
        .map_err(|_| RuntimeRoutingRequestError::ControlClosed)?;

    match timeout(wait, response).await {
        Err(_) => Err(RuntimeRoutingRequestError::TimedOut(wait)),
        Ok(Err(_)) => Err(RuntimeRoutingRequestError::ResponseClosed),
        Ok(Ok(Ok(result))) => Ok(result),
        Ok(Ok(Err(message))) => Err(RuntimeRoutingRequestError::Rejected(message)),
    }
}

async fn rollback_runtime_routing(
    routing_control: &mpsc::UnboundedSender<ipc::RuntimeCommand>,
    previous: &NodeNetworkConfigSnapshot,
    wait: Duration,
) -> Result<(), RuntimeControlError> {
    let command = NodeUpdateRoutingCommand {
        network_name: previous.network_name.clone(),
        mode: previous.routing_mode,
        exit_node: previous.exit_node.clone(),
        is_exit_node: previous.exit_node_enabled,
    };
    request_runtime_routing_update(routing_control, &command, wait)
        .await
        .map(|_| ())
        .map_err(|error| {
            RuntimeControlError::Persistence(format!(
                "网络 [{}] 的旧路由回滚失败: {}",
                previous.network_name,
                describe_routing_request_failure(&error)
            ))
        })
}

fn describe_routing_request_failure(error: &RuntimeRoutingRequestError) -> String {
    match error {
        RuntimeRoutingRequestError::ControlClosed => "运行时控制通道已关闭".into(),
        RuntimeRoutingRequestError::TimedOut(wait) => {
            format!("等待运行时响应超时（{} 秒）", wait.as_secs_f64())
        }
        RuntimeRoutingRequestError::ResponseClosed => "运行时在完成前关闭了响应通道".into(),
        RuntimeRoutingRequestError::Rejected(message) => format!("运行时拒绝更新: {message}"),
    }
}

fn runtime_routing_mode_name(mode: NodeRoutingMode) -> &'static str {
    match mode {
        NodeRoutingMode::MeshOnly => "MeshOnly",
        NodeRoutingMode::BypassMainland => "BypassMainland",
        NodeRoutingMode::GlobalProxy => "GlobalProxy",
    }
}

fn roles_from_config(config: &ClientConfig) -> Vec<NodeNetworkRole> {
    let mut roles = vec![NodeNetworkRole::Member];
    if config.exit_node.enabled {
        roles.push(NodeNetworkRole::ExitNode);
    }
    roles
}

fn ensure_member_role(
    network_name: &str,
    roles: &[NodeNetworkRole],
) -> Result<(), RuntimeControlError> {
    if roles.contains(&NodeNetworkRole::Member) {
        return Ok(());
    }
    Err(RuntimeControlError::Persistence(format!(
        "网络 [{network_name}] 的共享运行快照缺少基线 Member 角色，未应用角色更新"
    )))
}

fn set_role_enabled(roles: &mut Vec<NodeNetworkRole>, role: NodeNetworkRole, enabled: bool) {
    roles.retain(|existing| *existing != role);
    if enabled {
        roles.push(role);
    }
    roles.sort_by_key(|role| match role {
        NodeNetworkRole::Member => 0,
        NodeNetworkRole::Discovery => 1,
        NodeNetworkRole::Relay => 2,
        NodeNetworkRole::ExitNode => 3,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{
        NodeCommand, NodeCommandBusError, NodeCommandOutcome, NodeCommandRequest, NodeEvent,
        NodePeerSnapshot,
    };
    use std::collections::HashSet;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::sync::broadcast;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct RoutingCall {
        network_name: String,
        mode: NodeRoutingMode,
        exit_node: Option<String>,
        is_exit_node: bool,
    }

    #[derive(Clone, Default)]
    struct FakeLauncher {
        launches: Arc<Mutex<Vec<String>>>,
        failures: Arc<Mutex<HashSet<String>>>,
        shutdowns: Arc<AtomicUsize>,
        routing_calls: Arc<Mutex<Vec<RoutingCall>>>,
        routing_failures: Arc<Mutex<HashSet<String>>>,
        next_routing_failures: Arc<Mutex<HashSet<String>>>,
        next_routing_persistence_failures: Arc<Mutex<HashSet<String>>>,
        runtime_exit_nodes: Arc<Mutex<HashMap<String, bool>>>,
        routing_delay_ms: Arc<AtomicU64>,
        routing_in_flight: Arc<AtomicUsize>,
        routing_max_in_flight: Arc<AtomicUsize>,
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

        fn fail_routing(&self, network_name: &str) {
            self.routing_failures
                .lock()
                .expect("routing failure lock poisoned")
                .insert(network_name.to_string());
        }

        fn fail_next_routing(&self, network_name: &str) {
            self.next_routing_failures
                .lock()
                .expect("next routing failure lock poisoned")
                .insert(network_name.to_string());
        }

        fn fail_next_routing_persistence(&self, network_name: &str) {
            self.next_routing_persistence_failures
                .lock()
                .expect("next routing persistence failure lock poisoned")
                .insert(network_name.to_string());
        }

        fn set_routing_delay(&self, delay: Duration) {
            self.routing_delay_ms
                .store(delay.as_millis() as u64, Ordering::SeqCst);
        }

        fn routing_calls(&self) -> Vec<RoutingCall> {
            self.routing_calls
                .lock()
                .expect("routing call lock poisoned")
                .clone()
        }

        fn max_routing_concurrency(&self) -> usize {
            self.routing_max_in_flight.load(Ordering::SeqCst)
        }

        fn runtime_exit_node(&self, network_name: &str) -> Option<bool> {
            self.runtime_exit_nodes
                .lock()
                .expect("runtime exit node lock poisoned")
                .get(network_name)
                .copied()
        }
    }

    impl NetworkRuntimeLauncher for FakeLauncher {
        fn launch(&self, request: NetworkLaunchRequest) -> RuntimeFuture<StartedNetworkRuntime> {
            let launches = self.launches.clone();
            let failures = self.failures.clone();
            let shutdowns = self.shutdowns.clone();
            let routing_calls = self.routing_calls.clone();
            let routing_failures = self.routing_failures.clone();
            let next_routing_failures = self.next_routing_failures.clone();
            let next_routing_persistence_failures = self.next_routing_persistence_failures.clone();
            let runtime_exit_nodes = self.runtime_exit_nodes.clone();
            let routing_delay_ms = self.routing_delay_ms.clone();
            let routing_in_flight = self.routing_in_flight.clone();
            let routing_max_in_flight = self.routing_max_in_flight.clone();

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
                let tun_name = crate::multi_network::derive_tun_name(
                    Some(&request.network_name),
                    &request.config.tun.name,
                );
                let network_name = request.network_name.clone();
                let routing_network_name = network_name.clone();
                let config_path = PathBuf::from(request.config_path);
                let mut current_config = request.config;
                let (routing_control, mut routing_control_rx) = mpsc::unbounded_channel();
                let (shutdown, mut shutdown_rx) = watch::channel(false);
                let handle = tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            changed = shutdown_rx.changed() => {
                                if changed.is_err() || *shutdown_rx.borrow() {
                                    break;
                                }
                            }
                            command = routing_control_rx.recv() => {
                                let Some(command) = command else {
                                    break;
                                };
                                match command {
                                    ipc::RuntimeCommand::UpdateRouting {
                                        mode,
                                        exit_node,
                                        is_exit_node,
                                        reply,
                                    } => {
                                        let node_mode = match mode.as_str() {
                                            "MeshOnly" => NodeRoutingMode::MeshOnly,
                                            "BypassMainland" => NodeRoutingMode::BypassMainland,
                                            "GlobalProxy" => NodeRoutingMode::GlobalProxy,
                                            _ => {
                                                let _ = reply.send(Err(format!("未知路由模式: {mode}")));
                                                continue;
                                            }
                                        };
                                        let in_flight = routing_in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                                        routing_max_in_flight.fetch_max(in_flight, Ordering::SeqCst);
                                        routing_calls
                                            .lock()
                                            .expect("routing call lock poisoned")
                                            .push(RoutingCall {
                                                network_name: routing_network_name.clone(),
                                                mode: node_mode,
                                                exit_node: exit_node.clone(),
                                                is_exit_node,
                                            });

                                        let delay_ms = routing_delay_ms.load(Ordering::SeqCst);
                                        if delay_ms > 0 {
                                            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                                        }

                                        let persistent_failure = routing_failures
                                            .lock()
                                            .expect("routing failure lock poisoned")
                                            .contains(&routing_network_name);
                                        let one_shot_failure = next_routing_failures
                                            .lock()
                                            .expect("next routing failure lock poisoned")
                                            .remove(&routing_network_name);
                                        let failed = persistent_failure || one_shot_failure;
                                        let result = if failed {
                                            Err("测试运行时拒绝路由热更新".to_string())
                                        } else {
                                            let mut updated_config = current_config.clone();
                                            updated_config.routing.mode = match node_mode {
                                                NodeRoutingMode::MeshOnly => {
                                                    fuckipv6_common::RoutingMode::MeshOnly
                                                }
                                                NodeRoutingMode::BypassMainland => {
                                                    fuckipv6_common::RoutingMode::BypassMainland
                                                }
                                                NodeRoutingMode::GlobalProxy => {
                                                    fuckipv6_common::RoutingMode::GlobalProxy
                                                }
                                            };
                                            updated_config.routing.exit_node = exit_node.clone();
                                            updated_config.exit_node.enabled = is_exit_node;
                                            let persistence_failed = next_routing_persistence_failures
                                                .lock()
                                                .expect("routing persistence failure lock poisoned")
                                                .remove(&routing_network_name);
                                            let persisted = if persistence_failed {
                                                Err("测试运行时注入持久化失败".to_string())
                                            } else {
                                                updated_config
                                                    .save_to_file(&config_path)
                                                    .map_err(|error| format!("测试持久化失败: {error:#}"))
                                            };
                                            persisted
                                                .map(|_| {
                                                    current_config = updated_config;
                                                    runtime_exit_nodes
                                                        .lock()
                                                        .expect("runtime exit node lock poisoned")
                                                        .insert(routing_network_name.clone(), is_exit_node);
                                                    ipc::RoutingUpdateResult {
                                                    mode: format!("{node_mode:?}"),
                                                    exit_node,
                                                    is_exit_node,
                                                    }
                                                })
                                        };
                                        routing_in_flight.fetch_sub(1, Ordering::SeqCst);
                                        let _ = reply.send(result);
                                    }
                                    ipc::RuntimeCommand::ProbePeer { reply, .. } => {
                                        let _ = reply.send(Err("测试运行时不支持探测".into()));
                                    }
                                    ipc::RuntimeCommand::Disconnect { reply } => {
                                        let _ = reply.send(Ok(()));
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    shutdowns.fetch_add(1, Ordering::SeqCst);
                });

                Ok(StartedNetworkRuntime {
                    tun_name,
                    assigned_ipv6: Ipv6Addr::LOCALHOST,
                    assigned_ipv4: Some(Ipv4Addr::LOCALHOST),
                    p2p_listen_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                    identity: NodeIdentitySnapshot {
                        node_id: format!("node-{network_name}"),
                        display_name: None,
                        public_key_fingerprint: format!("fingerprint-{network_name}"),
                    },
                    routing_control,
                    shutdown,
                    handle,
                })
            })
        }
    }

    struct TestHarness {
        configs: ConfigManager,
        state: NodeStateStore,
        events: NodeEventBus,
        lifecycle: NodeLifecycle,
        services: EmbeddedServiceManager,
        runtime: NetworkRuntimeManager,
        fake: FakeLauncher,
        config_dir: PathBuf,
    }

    impl TestHarness {
        async fn new(label: &str) -> Self {
            Self::new_with_routing_timeout(label, ROUTING_UPDATE_TIMEOUT).await
        }

        async fn new_with_routing_timeout(label: &str, routing_update_timeout: Duration) -> Self {
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
            let runtime = NetworkRuntimeManager::spawn_with_launcher_and_timeout(
                configs.clone(),
                RuntimeOptions::default(),
                state.clone(),
                events.clone(),
                lifecycle.clone(),
                services.clone(),
                Arc::new(fake.clone()),
                routing_update_timeout,
            );

            Self {
                configs,
                state,
                events,
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

    fn peer_snapshot(network_name: &str) -> NodePeerSnapshot {
        NodePeerSnapshot {
            peer_id: format!("peer-{network_name}"),
            network_name: network_name.into(),
            virtual_ipv6: Ipv6Addr::LOCALHOST,
            virtual_ipv4: None,
            endpoints: Vec::new(),
            online: true,
            p2p_active: true,
            p2p_healthy: true,
            rtt_us: Some(1_000),
            loss_rate: 0.0,
            traffic: NodeTrafficSnapshot::default(),
        }
    }

    fn routing_command(
        network_name: &str,
        mode: NodeRoutingMode,
        exit_node: Option<&str>,
        is_exit_node: bool,
    ) -> NodeUpdateRoutingCommand {
        NodeUpdateRoutingCommand {
            network_name: network_name.into(),
            mode,
            exit_node: exit_node.map(str::to_string),
            is_exit_node,
        }
    }

    fn role_command(
        network_name: &str,
        role: NodeNetworkRole,
        enabled: bool,
    ) -> NodeSetNetworkRoleCommand {
        NodeSetNetworkRoleCommand {
            network_name: network_name.into(),
            role,
            enabled,
        }
    }

    async fn start_fake_network(harness: &TestHarness, network_name: &str) {
        harness
            .configs
            .create_network(network_name, "ws://127.0.0.1:6666", true)
            .expect("create test config");
        let plan = NodeRuntimePlan::load(
            &harness.configs,
            NodeNetworkSelection::Named(vec![network_name.into()]),
        )
        .expect("load test plan");
        let summary = harness
            .runtime
            .start_plan(plan)
            .await
            .expect("start fake network");
        assert_eq!(summary.started, vec![network_name]);
    }

    async fn wait_for_routing_calls(fake: &FakeLauncher, expected: usize) {
        timeout(Duration::from_secs(2), async {
            loop {
                if fake.routing_calls().len() >= expected {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("routing update did not reach the fake runtime");
    }

    fn routing_changed_events(
        receiver: &mut broadcast::Receiver<NodeEvent>,
    ) -> Vec<(String, NodeRoutingMode, Option<String>, bool)> {
        let mut changes = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            if let NodeEventKind::RoutingChanged {
                network_name,
                mode,
                exit_node,
                is_exit_node,
            } = event.kind
            {
                changes.push((network_name, mode, exit_node, is_exit_node));
            }
        }
        changes
    }

    fn role_changed_events(
        receiver: &mut broadcast::Receiver<NodeEvent>,
    ) -> Vec<(String, NodeNetworkRole, bool, Vec<NodeNetworkRole>)> {
        let mut changes = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            if let NodeEventKind::RoleChanged {
                network_name,
                role,
                enabled,
                roles,
            } = event.kind
            {
                changes.push((network_name, role, enabled, roles));
            }
        }
        changes
    }

    fn embedded_service_events(
        receiver: &mut broadcast::Receiver<NodeEvent>,
        network_name: &str,
    ) -> Vec<NodeEmbeddedServiceStatus> {
        let mut statuses = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            if let NodeEventKind::EmbeddedServiceChanged {
                network_name: event_network,
                status,
            } = event.kind
            {
                if event_network == network_name {
                    statuses.push(status);
                }
            }
        }
        statuses
    }

    #[tokio::test]
    async fn command_bus_hot_updates_routing_without_restarting_or_dropping_peers() {
        let harness = TestHarness::new("routing-success").await;
        start_fake_network(&harness, "routing-success").await;
        harness
            .state
            .update(|snapshot| {
                snapshot.exit_node_capabilities = fuckipv6_common::ExitNodeCapabilities {
                    schema_version: fuckipv6_common::EXIT_NODE_CAPABILITY_SCHEMA_VERSION,
                    platform: fuckipv6_common::ExitNodePlatform::Linux,
                    supported: true,
                    ipv4_forwarding: true,
                    ipv6_forwarding: true,
                    nat: true,
                    cleanup: true,
                    unsupported_reasons: Vec::new(),
                };
                let network = snapshot
                    .networks
                    .iter_mut()
                    .find(|network| network.name == "routing-success")
                    .expect("running network");
                network.peer_count = 1;
                snapshot.peers.push(peer_snapshot("routing-success"));
            })
            .await;
        let mut events = harness.events.subscribe();
        let bus = crate::node::spawn_command_bus_with_runtime(
            harness.state.clone(),
            MultiNetworkRegistry::new(),
            harness.events.clone(),
            harness.services.clone(),
            harness.runtime.clone(),
        );

        let result = bus
            .execute(NodeCommandRequest::new(
                600,
                NodeCommand::UpdateRouting(routing_command(
                    "routing-success",
                    NodeRoutingMode::GlobalProxy,
                    Some("office"),
                    true,
                )),
            ))
            .await
            .expect("routing command result");
        assert!(result.is_success());
        match result.outcome {
            NodeCommandOutcome::Completed { message } => {
                assert!(message.contains("GlobalProxy"));
                assert!(message.contains("office"));
            }
            outcome => panic!("unexpected outcome: {outcome:?}"),
        }

        assert_eq!(
            harness.fake.routing_calls(),
            vec![RoutingCall {
                network_name: "routing-success".into(),
                mode: NodeRoutingMode::GlobalProxy,
                exit_node: Some("office".into()),
                is_exit_node: true,
            }]
        );
        let config_path = harness
            .configs
            .get_network_config("routing-success")
            .expect("network config path");
        let persisted = ClientConfig::from_file(config_path).expect("persisted routing config");
        assert_eq!(
            persisted.routing.mode,
            fuckipv6_common::RoutingMode::GlobalProxy
        );
        assert_eq!(persisted.routing.exit_node.as_deref(), Some("office"));
        assert!(persisted.exit_node.enabled);

        let snapshot = harness.state.snapshot().await;
        let configuration = snapshot
            .configuration
            .networks
            .iter()
            .find(|network| network.network_name == "routing-success")
            .expect("shared routing config");
        assert_eq!(configuration.routing_mode, NodeRoutingMode::GlobalProxy);
        assert_eq!(configuration.exit_node.as_deref(), Some("office"));
        assert!(configuration.exit_node_enabled);
        assert_eq!(
            snapshot
                .peers
                .iter()
                .filter(|peer| peer.network_name == "routing-success")
                .count(),
            1
        );
        assert_eq!(
            snapshot
                .networks
                .iter()
                .find(|network| network.name == "routing-success")
                .expect("running network")
                .peer_count,
            1
        );
        assert_eq!(
            routing_changed_events(&mut events),
            vec![(
                "routing-success".into(),
                NodeRoutingMode::GlobalProxy,
                Some("office".into()),
                true,
            )]
        );
        assert_eq!(harness.fake.launches(), vec!["routing-success"]);
        assert_eq!(harness.fake.shutdown_count(), 0);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn command_bus_hot_updates_exit_node_role_without_restarting_or_dropping_peers() {
        let harness = TestHarness::new("role-success").await;
        start_fake_network(&harness, "role-success").await;
        harness
            .state
            .update(|snapshot| {
                snapshot.exit_node_capabilities = fuckipv6_common::ExitNodeCapabilities {
                    schema_version: fuckipv6_common::EXIT_NODE_CAPABILITY_SCHEMA_VERSION,
                    platform: fuckipv6_common::ExitNodePlatform::Linux,
                    supported: true,
                    ipv4_forwarding: true,
                    ipv6_forwarding: true,
                    nat: true,
                    cleanup: true,
                    unsupported_reasons: Vec::new(),
                };
                let network = snapshot
                    .networks
                    .iter_mut()
                    .find(|network| network.name == "role-success")
                    .expect("running network");
                network.peer_count = 1;
                snapshot.peers.push(peer_snapshot("role-success"));
            })
            .await;
        let mut events = harness.events.subscribe();
        let bus = crate::node::spawn_command_bus_with_runtime(
            harness.state.clone(),
            MultiNetworkRegistry::new(),
            harness.events.clone(),
            harness.services.clone(),
            harness.runtime.clone(),
        );

        let result = bus
            .execute(NodeCommandRequest::new(
                700,
                NodeCommand::SetNetworkRole(role_command(
                    "role-success",
                    NodeNetworkRole::ExitNode,
                    true,
                )),
            ))
            .await
            .expect("role command result");
        assert!(result.is_success());
        match result.outcome {
            NodeCommandOutcome::Completed { message } => {
                assert!(message.contains("ExitNode"));
                assert!(message.contains("enabled=true"));
            }
            outcome => panic!("unexpected outcome: {outcome:?}"),
        }

        assert_eq!(
            harness.fake.routing_calls(),
            vec![RoutingCall {
                network_name: "role-success".into(),
                mode: NodeRoutingMode::MeshOnly,
                exit_node: None,
                is_exit_node: true,
            }]
        );
        assert_eq!(harness.fake.runtime_exit_node("role-success"), Some(true));
        let config_path = harness
            .configs
            .get_network_config("role-success")
            .expect("network config path");
        let persisted = ClientConfig::from_file(config_path).expect("persisted role config");
        assert!(persisted.exit_node.enabled);

        let snapshot = harness.state.snapshot().await;
        let network = snapshot
            .networks
            .iter()
            .find(|network| network.name == "role-success")
            .expect("running network");
        assert_eq!(
            network.roles,
            vec![NodeNetworkRole::Member, NodeNetworkRole::ExitNode]
        );
        let configuration = snapshot
            .configuration
            .networks
            .iter()
            .find(|network| network.network_name == "role-success")
            .expect("shared role config");
        assert!(configuration.exit_node_enabled);
        assert_eq!(snapshot.peers, vec![peer_snapshot("role-success")]);
        assert_eq!(
            role_changed_events(&mut events),
            vec![(
                "role-success".into(),
                NodeNetworkRole::ExitNode,
                true,
                vec![NodeNetworkRole::Member, NodeNetworkRole::ExitNode],
            )]
        );
        assert_eq!(harness.fake.launches(), vec!["role-success"]);
        assert_eq!(harness.fake.shutdown_count(), 0);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn persisted_exit_node_role_is_restored_when_the_network_restarts() {
        let harness = TestHarness::new("role-restart").await;
        start_fake_network(&harness, "role-restart").await;
        harness
            .runtime
            .set_network_role(role_command(
                "role-restart",
                NodeNetworkRole::ExitNode,
                true,
            ))
            .await
            .expect("enable persisted exit role");

        let config_path = harness
            .configs
            .get_network_config("role-restart")
            .expect("network config path");
        assert!(
            ClientConfig::from_file(config_path)
                .expect("persisted role config")
                .exit_node
                .enabled
        );

        harness
            .runtime
            .stop(vec!["role-restart".into()], false)
            .await
            .expect("stop network before restart");
        assert_eq!(harness.fake.launches(), vec!["role-restart"]);
        assert_eq!(harness.fake.shutdown_count(), 1);

        let plan = NodeRuntimePlan::load(
            &harness.configs,
            NodeNetworkSelection::Named(vec!["role-restart".into()]),
        )
        .expect("reload persisted network plan");
        let summary = harness
            .runtime
            .start_plan(plan)
            .await
            .expect("restart network from persisted config");
        assert_eq!(summary.started, vec!["role-restart"]);
        assert_eq!(
            harness.fake.launches(),
            vec!["role-restart", "role-restart"]
        );
        assert_eq!(harness.fake.shutdown_count(), 1);

        let snapshot = harness.state.snapshot().await;
        assert_eq!(
            snapshot
                .networks
                .iter()
                .find(|network| network.name == "role-restart")
                .expect("restarted network")
                .roles,
            vec![NodeNetworkRole::Member, NodeNetworkRole::ExitNode]
        );
        assert!(
            snapshot
                .configuration
                .networks
                .iter()
                .find(|network| network.network_name == "role-restart")
                .expect("restored role config")
                .exit_node_enabled
        );

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn invalid_and_unavailable_role_changes_have_no_side_effects() {
        let harness = TestHarness::new("role-rejections").await;
        start_fake_network(&harness, "role-rejections").await;
        let mut events = harness.events.subscribe();
        let bus = crate::node::spawn_command_bus_with_runtime(
            harness.state.clone(),
            MultiNetworkRegistry::new(),
            harness.events.clone(),
            harness.services.clone(),
            harness.runtime.clone(),
        );
        let invalid = bus
            .execute(NodeCommandRequest::new(
                710,
                NodeCommand::SetNetworkRole(role_command(
                    "role-rejections",
                    NodeNetworkRole::Member,
                    false,
                )),
            ))
            .await
            .expect_err("disabling the baseline Member role must fail validation");
        match invalid {
            NodeCommandBusError::Invalid(problem) => {
                assert_eq!(problem.code, NodeProblemCode::InvalidRequest);
                assert!(!problem.retryable);
                assert_eq!(problem.network_name.as_deref(), Some("role-rejections"));
            }
            error => panic!("unexpected command bus error: {error:?}"),
        }

        let cases = [
            (
                role_command("role-rejections", NodeNetworkRole::Member, true),
                NodeProblemCode::Conflict,
                false,
            ),
            (
                role_command("role-rejections", NodeNetworkRole::Discovery, true),
                NodeProblemCode::Unavailable,
                true,
            ),
            (
                role_command("role-rejections", NodeNetworkRole::Relay, true),
                NodeProblemCode::Unavailable,
                true,
            ),
        ];
        for (offset, (command, expected_code, expected_retryable)) in cases.into_iter().enumerate()
        {
            let result = bus
                .execute(NodeCommandRequest::new(
                    711 + offset as u64,
                    NodeCommand::SetNetworkRole(command),
                ))
                .await
                .expect("role rejection result");
            match result.outcome {
                NodeCommandOutcome::Rejected(problem) => {
                    assert_eq!(problem.code, expected_code);
                    assert_eq!(problem.retryable, expected_retryable);
                    assert_eq!(problem.network_name.as_deref(), Some("role-rejections"));
                }
                outcome => panic!("unexpected outcome: {outcome:?}"),
            }
        }

        assert!(harness.fake.routing_calls().is_empty());
        assert_eq!(harness.fake.launches(), vec!["role-rejections"]);
        let snapshot = harness.state.snapshot().await;
        assert_eq!(
            snapshot
                .networks
                .iter()
                .find(|network| network.name == "role-rejections")
                .expect("running network")
                .roles,
            vec![NodeNetworkRole::Member]
        );
        let config_path = harness
            .configs
            .get_network_config("role-rejections")
            .expect("network config path");
        assert!(
            !ClientConfig::from_file(config_path)
                .expect("unchanged role config")
                .exit_node
                .enabled
        );
        assert!(role_changed_events(&mut events).is_empty());
        assert_eq!(harness.fake.shutdown_count(), 0);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn concurrent_role_updates_are_serialized_and_last_write_wins() {
        let harness = TestHarness::new("role-concurrent").await;
        start_fake_network(&harness, "role-concurrent").await;
        harness.fake.set_routing_delay(Duration::from_millis(80));

        let first_runtime = harness.runtime.clone();
        let first = tokio::spawn(async move {
            first_runtime
                .set_network_role(role_command(
                    "role-concurrent",
                    NodeNetworkRole::ExitNode,
                    true,
                ))
                .await
        });
        wait_for_routing_calls(&harness.fake, 1).await;

        harness
            .runtime
            .set_network_role(role_command(
                "role-concurrent",
                NodeNetworkRole::ExitNode,
                false,
            ))
            .await
            .expect("second role update");
        first
            .await
            .expect("first role task")
            .expect("first role update");

        assert_eq!(harness.fake.max_routing_concurrency(), 1);
        assert_eq!(
            harness
                .fake
                .routing_calls()
                .into_iter()
                .map(|call| call.is_exit_node)
                .collect::<Vec<_>>(),
            vec![true, false]
        );
        let snapshot = harness.state.snapshot().await;
        assert_eq!(
            snapshot
                .networks
                .iter()
                .find(|network| network.name == "role-concurrent")
                .expect("running network")
                .roles,
            vec![NodeNetworkRole::Member]
        );
        assert!(
            !snapshot
                .configuration
                .networks
                .iter()
                .find(|network| network.network_name == "role-concurrent")
                .expect("final role config")
                .exit_node_enabled
        );
        assert_eq!(harness.fake.launches(), vec!["role-concurrent"]);
        assert_eq!(harness.fake.shutdown_count(), 0);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn role_runtime_failure_rolls_back_runtime_file_and_snapshot() {
        let harness = TestHarness::new("role-runtime-rollback").await;
        start_fake_network(&harness, "role-runtime").await;
        harness
            .state
            .update(|snapshot| {
                let network = snapshot
                    .networks
                    .iter_mut()
                    .find(|network| network.name == "role-runtime")
                    .expect("running network");
                network.peer_count = 1;
                snapshot.peers.push(peer_snapshot("role-runtime"));
            })
            .await;
        let before = harness.state.snapshot().await;
        let config_path = harness
            .configs
            .get_network_config("role-runtime")
            .expect("network config path");
        harness.fake.fail_next_routing("role-runtime");
        let mut events = harness.events.subscribe();

        let error = harness
            .runtime
            .set_network_role(role_command(
                "role-runtime",
                NodeNetworkRole::ExitNode,
                true,
            ))
            .await
            .expect_err("runtime rejection must propagate");
        assert!(matches!(
            error,
            RuntimeControlError::Persistence(ref message)
                if message.contains("运行态与持久化配置已回滚")
        ));

        let after = harness.state.snapshot().await;
        assert_eq!(after.peers, before.peers);
        assert_eq!(
            after
                .networks
                .iter()
                .find(|network| network.name == "role-runtime")
                .expect("running network")
                .roles,
            vec![NodeNetworkRole::Member]
        );
        assert_eq!(harness.fake.runtime_exit_node("role-runtime"), Some(false));
        assert!(
            !ClientConfig::from_file(config_path)
                .expect("rolled back role config")
                .exit_node
                .enabled
        );
        assert!(role_changed_events(&mut events).is_empty());
        assert_eq!(harness.fake.launches(), vec!["role-runtime"]);
        assert_eq!(harness.fake.shutdown_count(), 0);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn role_persistence_failure_rolls_back_runtime_and_snapshot() {
        let harness = TestHarness::new("role-persistence-rollback").await;
        start_fake_network(&harness, "role-persistence").await;
        let before = harness.state.snapshot().await;
        let config_path = harness
            .configs
            .get_network_config("role-persistence")
            .expect("network config path");
        harness
            .fake
            .fail_next_routing_persistence("role-persistence");
        let mut events = harness.events.subscribe();

        let error = harness
            .runtime
            .set_network_role(role_command(
                "role-persistence",
                NodeNetworkRole::ExitNode,
                true,
            ))
            .await
            .expect_err("persistence rejection must propagate");
        assert!(matches!(
            error,
            RuntimeControlError::Persistence(ref message)
                if message.contains("运行态与持久化配置已回滚")
        ));

        let after = harness.state.snapshot().await;
        assert_eq!(after.peers, before.peers);
        assert!(
            !after
                .configuration
                .networks
                .iter()
                .find(|network| network.network_name == "role-persistence")
                .expect("shared role config")
                .exit_node_enabled
        );
        assert_eq!(
            after
                .networks
                .iter()
                .find(|network| network.name == "role-persistence")
                .expect("running network")
                .roles,
            vec![NodeNetworkRole::Member]
        );
        assert_eq!(
            harness.fake.runtime_exit_node("role-persistence"),
            Some(false)
        );
        assert!(
            !ClientConfig::from_file(config_path)
                .expect("unchanged role config")
                .exit_node
                .enabled
        );
        assert!(role_changed_events(&mut events).is_empty());
        assert_eq!(harness.fake.launches(), vec!["role-persistence"]);
        assert_eq!(harness.fake.shutdown_count(), 0);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn role_snapshot_failure_rolls_back_runtime_file_and_shared_state() {
        let harness = TestHarness::new("role-snapshot-rollback").await;
        start_fake_network(&harness, "role-snapshot").await;
        harness
            .state
            .update(|snapshot| {
                let network = snapshot
                    .networks
                    .iter_mut()
                    .find(|network| network.name == "role-snapshot")
                    .expect("running network");
                network.peer_count = 1;
                snapshot.peers.push(peer_snapshot("role-snapshot"));
            })
            .await;
        let before = harness.state.snapshot().await;
        let before_config = before
            .configuration
            .networks
            .iter()
            .find(|network| network.network_name == "role-snapshot")
            .expect("initial role config")
            .clone();
        let config_path = harness
            .configs
            .get_network_config("role-snapshot")
            .expect("network config path");
        harness.fake.set_routing_delay(Duration::from_millis(80));
        let mut events = harness.events.subscribe();
        let update_runtime = harness.runtime.clone();
        let update = tokio::spawn(async move {
            update_runtime
                .set_network_role(role_command(
                    "role-snapshot",
                    NodeNetworkRole::ExitNode,
                    true,
                ))
                .await
        });
        wait_for_routing_calls(&harness.fake, 1).await;

        harness
            .state
            .update(|snapshot| {
                snapshot
                    .configuration
                    .networks
                    .retain(|network| network.network_name != "role-snapshot");
            })
            .await;
        let error = update
            .await
            .expect("role task")
            .expect_err("snapshot publish failure must reject the role update");
        assert!(matches!(
            error,
            RuntimeControlError::Persistence(ref message)
                if message.contains("角色共享快照发布失败")
        ));

        assert_eq!(harness.fake.runtime_exit_node("role-snapshot"), Some(false));
        assert!(
            !ClientConfig::from_file(config_path)
                .expect("rolled back role config")
                .exit_node
                .enabled
        );
        let after = harness.state.snapshot().await;
        assert_eq!(
            after
                .configuration
                .networks
                .iter()
                .find(|network| network.network_name == "role-snapshot")
                .expect("restored role config"),
            &before_config
        );
        assert_eq!(
            after
                .networks
                .iter()
                .find(|network| network.name == "role-snapshot")
                .expect("running network")
                .roles,
            vec![NodeNetworkRole::Member]
        );
        assert_eq!(after.peers, before.peers);
        assert!(role_changed_events(&mut events).is_empty());
        assert_eq!(harness.fake.launches(), vec!["role-snapshot"]);
        assert_eq!(harness.fake.shutdown_count(), 0);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn stopping_network_after_role_update_releases_embedded_service() {
        let harness = TestHarness::new("role-stop-service").await;
        start_fake_network(&harness, "role-stop").await;
        harness
            .services
            .start(
                EmbeddedServiceStartRequest::new("role-stop", 0, 0).with_external_host("127.0.0.1"),
            )
            .await
            .expect("start role-owned embedded service");
        harness
            .runtime
            .set_network_role(role_command("role-stop", NodeNetworkRole::ExitNode, true))
            .await
            .expect("enable exit role");

        let stopped = harness
            .runtime
            .stop(vec!["role-stop".into()], false)
            .await
            .expect("stop network and role-owned service");
        assert_eq!(stopped.stopped, vec!["role-stop"]);
        assert!(harness
            .services
            .list()
            .await
            .expect("list embedded services")
            .iter()
            .all(|service| service.network_name != "role-stop"));
        assert_eq!(harness.fake.shutdown_count(), 1);
        assert_eq!(harness.fake.launches(), vec!["role-stop"]);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn runtime_routing_failure_leaves_file_snapshot_and_peers_unchanged() {
        let harness = TestHarness::new("routing-failure").await;
        start_fake_network(&harness, "routing-failure").await;
        harness
            .state
            .update(|snapshot| {
                let network = snapshot
                    .networks
                    .iter_mut()
                    .find(|network| network.name == "routing-failure")
                    .expect("running network");
                network.peer_count = 1;
                snapshot.peers.push(peer_snapshot("routing-failure"));
            })
            .await;
        let before = harness.state.snapshot().await;
        let before_config = before
            .configuration
            .networks
            .iter()
            .find(|network| network.network_name == "routing-failure")
            .expect("initial shared routing config")
            .clone();
        let config_path = harness
            .configs
            .get_network_config("routing-failure")
            .expect("network config path");
        let before_file = ClientConfig::from_file(&config_path).expect("initial config");
        harness.fake.fail_routing("routing-failure");
        let mut events = harness.events.subscribe();

        let error = harness
            .runtime
            .update_routing(routing_command(
                "routing-failure",
                NodeRoutingMode::BypassMainland,
                Some("office"),
                true,
            ))
            .await
            .expect_err("runtime rejection must propagate");
        assert!(matches!(
            error,
            RuntimeControlError::Persistence(ref message)
                if message.contains("旧配置回滚未能确认")
        ));

        let after = harness.state.snapshot().await;
        assert_eq!(
            after
                .configuration
                .networks
                .iter()
                .find(|network| network.network_name == "routing-failure")
                .expect("unchanged shared routing config"),
            &before_config
        );
        let after_file = ClientConfig::from_file(&config_path).expect("unchanged config");
        assert_eq!(after_file.routing.mode, before_file.routing.mode);
        assert_eq!(after_file.routing.exit_node, before_file.routing.exit_node);
        assert_eq!(after_file.exit_node.enabled, before_file.exit_node.enabled);
        assert!(routing_changed_events(&mut events).is_empty());
        assert_eq!(after.peers, before.peers);
        assert_eq!(harness.fake.routing_calls().len(), 2);
        assert_eq!(harness.fake.launches(), vec!["routing-failure"]);
        assert_eq!(harness.fake.shutdown_count(), 0);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn concurrent_routing_updates_are_serialized_and_last_write_wins() {
        let harness = TestHarness::new("routing-concurrent").await;
        start_fake_network(&harness, "routing-concurrent").await;
        harness.fake.set_routing_delay(Duration::from_millis(80));

        let first_runtime = harness.runtime.clone();
        let first = tokio::spawn(async move {
            first_runtime
                .update_routing(routing_command(
                    "routing-concurrent",
                    NodeRoutingMode::BypassMainland,
                    Some("first"),
                    false,
                ))
                .await
        });
        wait_for_routing_calls(&harness.fake, 1).await;

        let second = harness
            .runtime
            .update_routing(routing_command(
                "routing-concurrent",
                NodeRoutingMode::GlobalProxy,
                Some("second"),
                true,
            ))
            .await
            .expect("second routing update");
        assert_eq!(second.mode, "GlobalProxy");
        first
            .await
            .expect("first routing task")
            .expect("first routing update");

        assert_eq!(harness.fake.max_routing_concurrency(), 1);
        let calls = harness.fake.routing_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].mode, NodeRoutingMode::BypassMainland);
        assert_eq!(calls[1].mode, NodeRoutingMode::GlobalProxy);
        let snapshot = harness.state.snapshot().await;
        let configuration = snapshot
            .configuration
            .networks
            .iter()
            .find(|network| network.network_name == "routing-concurrent")
            .expect("final shared routing config");
        assert_eq!(configuration.routing_mode, NodeRoutingMode::GlobalProxy);
        assert_eq!(configuration.exit_node.as_deref(), Some("second"));
        assert!(configuration.exit_node_enabled);
        assert_eq!(harness.fake.launches(), vec!["routing-concurrent"]);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn timed_out_update_queues_old_config_rollback_after_late_application() {
        let harness = TestHarness::new_with_routing_timeout(
            "routing-timeout-rollback",
            Duration::from_millis(80),
        )
        .await;
        start_fake_network(&harness, "routing-timeout").await;
        harness
            .state
            .update(|snapshot| {
                let network = snapshot
                    .networks
                    .iter_mut()
                    .find(|network| network.name == "routing-timeout")
                    .expect("running network");
                network.peer_count = 1;
                snapshot.peers.push(peer_snapshot("routing-timeout"));
            })
            .await;
        let before = harness.state.snapshot().await;
        let before_config = before
            .configuration
            .networks
            .iter()
            .find(|network| network.network_name == "routing-timeout")
            .expect("initial shared routing config")
            .clone();
        let config_path = harness
            .configs
            .get_network_config("routing-timeout")
            .expect("network config path");
        harness.fake.set_routing_delay(Duration::from_millis(120));
        let mut events = harness.events.subscribe();

        let update_runtime = harness.runtime.clone();
        let update = tokio::spawn(async move {
            update_runtime
                .update_routing(routing_command(
                    "routing-timeout",
                    NodeRoutingMode::GlobalProxy,
                    Some("office"),
                    true,
                ))
                .await
        });
        wait_for_routing_calls(&harness.fake, 1).await;
        // Only the first application is slow. The queued compensation command
        // must quickly restore the old config after that late application ends.
        harness.fake.set_routing_delay(Duration::ZERO);
        let error = update
            .await
            .expect("timed out routing task")
            .expect_err("timed out update must not report success");
        assert!(matches!(
            error,
            RuntimeControlError::Persistence(ref message)
                if message.contains("超时") && message.contains("已回滚")
        ));

        let calls = harness.fake.routing_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].mode, NodeRoutingMode::GlobalProxy);
        assert_eq!(calls[1].mode, before_config.routing_mode);
        assert_eq!(calls[1].exit_node, before_config.exit_node);
        assert_eq!(calls[1].is_exit_node, before_config.exit_node_enabled);
        assert_eq!(harness.fake.max_routing_concurrency(), 1);

        let persisted = ClientConfig::from_file(config_path).expect("rolled back config");
        assert_eq!(
            persisted.routing.mode,
            fuckipv6_common::RoutingMode::MeshOnly
        );
        assert_eq!(persisted.routing.exit_node, before_config.exit_node);
        assert_eq!(persisted.exit_node.enabled, before_config.exit_node_enabled);
        let after = harness.state.snapshot().await;
        assert_eq!(
            after
                .configuration
                .networks
                .iter()
                .find(|network| network.network_name == "routing-timeout")
                .expect("restored shared routing config"),
            &before_config
        );
        assert_eq!(after.peers, before.peers);
        assert!(routing_changed_events(&mut events).is_empty());
        assert_eq!(harness.fake.launches(), vec!["routing-timeout"]);
        assert_eq!(harness.fake.shutdown_count(), 0);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn snapshot_publish_failure_rolls_back_runtime_file_and_shared_state() {
        let harness = TestHarness::new("routing-snapshot-rollback").await;
        start_fake_network(&harness, "routing-snapshot").await;
        harness
            .state
            .update(|snapshot| {
                let network = snapshot
                    .networks
                    .iter_mut()
                    .find(|network| network.name == "routing-snapshot")
                    .expect("running network");
                network.peer_count = 1;
                snapshot.peers.push(peer_snapshot("routing-snapshot"));
            })
            .await;
        let before = harness.state.snapshot().await;
        let before_config = before
            .configuration
            .networks
            .iter()
            .find(|network| network.network_name == "routing-snapshot")
            .expect("initial shared routing config")
            .clone();
        harness.fake.set_routing_delay(Duration::from_millis(80));
        let mut events = harness.events.subscribe();
        let routing_runtime = harness.runtime.clone();
        let update = tokio::spawn(async move {
            routing_runtime
                .update_routing(routing_command(
                    "routing-snapshot",
                    NodeRoutingMode::GlobalProxy,
                    Some("office"),
                    true,
                ))
                .await
        });
        wait_for_routing_calls(&harness.fake, 1).await;

        harness
            .state
            .update(|snapshot| {
                snapshot
                    .configuration
                    .networks
                    .retain(|network| network.network_name != "routing-snapshot");
            })
            .await;
        let error = update
            .await
            .expect("routing task")
            .expect_err("snapshot publish failure must reject the update");
        assert!(matches!(
            error,
            RuntimeControlError::Persistence(ref message)
                if message.contains("共享快照发布失败")
        ));

        let calls = harness.fake.routing_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].mode, NodeRoutingMode::GlobalProxy);
        assert_eq!(calls[1].mode, before_config.routing_mode);
        assert_eq!(calls[1].exit_node, before_config.exit_node);
        assert_eq!(calls[1].is_exit_node, before_config.exit_node_enabled);
        let config_path = harness
            .configs
            .get_network_config("routing-snapshot")
            .expect("network config path");
        let persisted = ClientConfig::from_file(config_path).expect("rolled back config");
        assert_eq!(
            persisted.routing.mode,
            fuckipv6_common::RoutingMode::MeshOnly
        );
        assert_eq!(persisted.routing.exit_node, before_config.exit_node);
        assert_eq!(persisted.exit_node.enabled, before_config.exit_node_enabled);
        let after = harness.state.snapshot().await;
        assert_eq!(
            after
                .configuration
                .networks
                .iter()
                .find(|network| network.network_name == "routing-snapshot")
                .expect("restored shared routing config"),
            &before_config
        );
        assert_eq!(after.peers, before.peers);
        assert!(routing_changed_events(&mut events).is_empty());
        assert_eq!(harness.fake.launches(), vec!["routing-snapshot"]);
        assert_eq!(harness.fake.shutdown_count(), 0);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn routing_update_for_a_non_running_network_does_not_touch_the_runtime() {
        let harness = TestHarness::new("routing-missing").await;

        let error = harness
            .runtime
            .update_routing(routing_command(
                "missing",
                NodeRoutingMode::GlobalProxy,
                None,
                false,
            ))
            .await
            .expect_err("non-running network must be rejected");
        assert!(matches!(error, RuntimeControlError::NotFound(_)));
        assert!(harness.fake.routing_calls().is_empty());

        harness.shutdown().await;
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
    async fn create_exposes_running_embedded_service_without_secrets_or_paths() {
        let harness = TestHarness::new("service-running").await;
        let mut events = harness.events.subscribe();

        let created = harness
            .runtime
            .create(create_command("local", false))
            .await
            .expect("create detached network");
        assert!(created.ticket.starts_with(NetworkTicket::URI_SCHEME));
        assert_eq!(
            embedded_service_events(&mut events, "local"),
            vec![
                NodeEmbeddedServiceStatus::Starting,
                NodeEmbeddedServiceStatus::Running,
            ]
        );

        let snapshot = harness.state.snapshot().await;
        let service = snapshot
            .embedded_services
            .iter()
            .find(|service| service.network_name == "local")
            .expect("embedded service snapshot");
        assert_eq!(service.status, NodeEmbeddedServiceStatus::Running);
        assert!(service.signaling_addr.is_some());
        assert!(service.relay_addr.is_some());
        assert!(snapshot
            .configuration
            .networks
            .iter()
            .any(|network| network.network_name == "local"));
        assert!(snapshot.invariant_violations().is_empty());

        let encoded = serde_json::to_string(&snapshot).expect("serialize node snapshot");
        assert!(!encoded.contains("ticket"));
        assert!(!encoded.contains("private_key"));
        assert!(!encoded.contains("secret_key"));
        assert!(!encoded.contains("config_path"));

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn embedded_service_start_failure_is_visible_in_snapshot_and_events() {
        let harness = TestHarness::new("service-failure").await;
        let mut events = harness.events.subscribe();
        let mut command = create_command("broken", false);
        command.signaling_port = 12345;
        command.relay_port = 12345;

        let error = harness
            .runtime
            .create(command)
            .await
            .expect_err("invalid embedded service request must fail");
        assert!(matches!(error, RuntimeControlError::Invalid(_)));
        assert_eq!(
            embedded_service_events(&mut events, "broken"),
            vec![
                NodeEmbeddedServiceStatus::Starting,
                NodeEmbeddedServiceStatus::Failed,
            ]
        );

        let snapshot = harness.state.snapshot().await;
        let service = snapshot
            .embedded_services
            .iter()
            .find(|service| service.network_name == "broken")
            .expect("failed embedded service snapshot");
        assert_eq!(service.status, NodeEmbeddedServiceStatus::Failed);
        assert!(service.signaling_addr.is_none());
        assert!(service.relay_addr.is_none());
        assert!(!harness.configs.network_exists("broken"));

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
        harness
            .state
            .update(|snapshot| {
                let network = snapshot
                    .networks
                    .iter_mut()
                    .find(|network| network.name == "owned")
                    .expect("running network state");
                network.peer_count = 1;
                snapshot.peers.push(peer_snapshot("owned"));
            })
            .await;
        assert!(harness
            .state
            .snapshot()
            .await
            .invariant_violations()
            .is_empty());
        let mut events = harness.events.subscribe();

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
            network.name == "owned"
                && network.status == NodeNetworkStatus::Offline
                && network.peer_count == 0
        }));
        assert!(snapshot.peers.is_empty());
        assert!(snapshot.invariant_violations().is_empty());
        let service = snapshot
            .embedded_services
            .iter()
            .find(|service| service.network_name == "owned")
            .expect("stopped embedded service snapshot");
        assert_eq!(service.status, NodeEmbeddedServiceStatus::Stopped);
        assert!(service.signaling_addr.is_none());
        assert!(service.relay_addr.is_none());
        assert_eq!(
            embedded_service_events(&mut events, "owned"),
            vec![
                NodeEmbeddedServiceStatus::Stopping,
                NodeEmbeddedServiceStatus::Stopped,
            ]
        );
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
    async fn shutdown_stops_every_embedded_service_and_publishes_events() {
        let harness = TestHarness::new("shutdown-services").await;
        harness
            .runtime
            .create(create_command("alpha", false))
            .await
            .expect("create alpha");
        harness
            .runtime
            .create(create_command("beta", false))
            .await
            .expect("create beta");
        let mut alpha_events = harness.events.subscribe();
        let mut beta_events = harness.events.subscribe();

        let summary = harness.runtime.shutdown().await.expect("shutdown node");
        assert_eq!(summary.stopped, vec!["alpha", "beta"]);

        let snapshot = harness.state.snapshot().await;
        assert_eq!(snapshot.embedded_services.len(), 2);
        for service in &snapshot.embedded_services {
            assert_eq!(service.status, NodeEmbeddedServiceStatus::Stopped);
            assert!(service.signaling_addr.is_none());
            assert!(service.relay_addr.is_none());
        }
        assert_eq!(
            embedded_service_events(&mut alpha_events, "alpha"),
            vec![
                NodeEmbeddedServiceStatus::Stopping,
                NodeEmbeddedServiceStatus::Stopped,
            ]
        );
        assert_eq!(
            embedded_service_events(&mut beta_events, "beta"),
            vec![
                NodeEmbeddedServiceStatus::Stopping,
                NodeEmbeddedServiceStatus::Stopped,
            ]
        );

        let _ = std::fs::remove_dir_all(&harness.config_dir);
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
        assert!(!snapshot
            .embedded_services
            .iter()
            .any(|service| service.network_name == "broken"));
        assert!(!snapshot
            .peers
            .iter()
            .any(|peer| peer.network_name == "broken"));
        assert!(snapshot.invariant_violations().is_empty());

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
