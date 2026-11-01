use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use fuckipv6_common::{ClientConfig, NetworkTicket};
use futures::future::join_all;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{timeout, Instant};

use super::{
    NodeCleanupFailureReason, NodeCleanupFailureSnapshot, NodeCleanupStepSnapshot,
    NodeCleanupStepStatus, NodeCreateNetworkCommand, NodeDiagnosticsSnapshot,
    NodeEmbeddedServiceSnapshot, NodeEmbeddedServiceStatus, NodeEventBus, NodeEventKind,
    NodeIdentitySnapshot, NodeJoinNetworkCommand, NodeLifecycle, NodeNetworkConfigSnapshot,
    NodeNetworkRole, NodeNetworkSelection, NodeNetworkSnapshot, NodeNetworkStatus, NodePhase,
    NodeProblem, NodeProblemCode, NodeResidualCleanupResult, NodeRoutingMode, NodeRuntimePlan,
    NodeRuntimeResidueSnapshot, NodeSetNetworkRoleCommand, NodeStateStore, NodeTrafficSnapshot,
    NodeUpdateRoutingCommand,
};
use crate::cli::commands::{self, CleanupStatus, RuntimeCleanupOutcome, RuntimeOptions};
use crate::config_manager::{ConfigManager, NetworkConfig};
use crate::ipc::{self, MultiNetworkRegistry, NetworkSessionHandle};
use crate::node::runtime_metadata;
use crate::symmetric::{
    EmbeddedServiceError, EmbeddedServiceManager, EmbeddedServiceSnapshot,
    EmbeddedServiceStartRequest,
};

pub(crate) const STOP_TIMEOUT: Duration = Duration::from_secs(15);
pub(crate) const START_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const RESTART_TIMEOUT: Duration = Duration::from_secs(60);
const ROUTING_UPDATE_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RuntimeTimeouts {
    stop: Duration,
    start: Duration,
    restart: Duration,
    routing_update: Duration,
}

impl Default for RuntimeTimeouts {
    fn default() -> Self {
        Self {
            stop: STOP_TIMEOUT,
            start: START_TIMEOUT,
            restart: RESTART_TIMEOUT,
            routing_update: ROUTING_UPDATE_TIMEOUT,
        }
    }
}

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
    #[error("{0}")]
    Timeout(String),
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
pub(crate) struct StopFailure {
    pub network_name: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StopSummary {
    pub stopped: Vec<String>,
    pub failed: Vec<StopFailure>,
    pub cleanup: RuntimeCleanupReport,
}

impl StopSummary {
    pub fn is_success(&self) -> bool {
        self.failed.is_empty() && self.cleanup.is_clean_for_stop()
    }

    pub fn message(&self) -> String {
        let mut parts = vec![format!("已停止网络: {}", self.stopped.len())];
        if !self.stopped.is_empty() {
            parts.push(self.stopped.join(", "));
        }
        if !self.failed.is_empty() {
            let failures = self
                .failed
                .iter()
                .map(|failure| format!("{}: {}", failure.network_name, failure.message))
                .collect::<Vec<_>>()
                .join("; ");
            parts.push(format!("{} 个网络停止失败 ({failures})", self.failed.len()));
        }
        let cleanup_issues = self.cleanup.stop_issues();
        if !cleanup_issues.is_empty() {
            parts.push(format!("资源清理未完成 ({})", cleanup_issues.join("; ")));
        }
        parts.join("；")
    }

    pub(crate) fn to_public_cleanup_result(&self) -> NodeResidualCleanupResult {
        NodeResidualCleanupResult {
            cleaned_networks: self.stopped.clone(),
            failed_networks: self
                .failed
                .iter()
                .map(|failure| NodeCleanupFailureSnapshot {
                    network_name: failure.network_name.clone(),
                    reason: cleanup_failure_reason(&failure.message),
                })
                .collect(),
            residue: self.cleanup.to_public(),
        }
    }
}

fn cleanup_failure_reason(message: &str) -> NodeCleanupFailureReason {
    if message.contains("运行会话尚未退出") {
        NodeCleanupFailureReason::StillRunning
    } else if message.contains("运行时资源所有者已丢失") {
        NodeCleanupFailureReason::OwnerUnavailable
    } else if message.contains("cleanup/timeout") {
        NodeCleanupFailureReason::TimedOut
    } else if message.contains("资源清理重试未完成") {
        NodeCleanupFailureReason::Incomplete
    } else {
        NodeCleanupFailureReason::Unknown
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestartPhase {
    PersistLoad,
    Stop,
    Cleanup,
    Start,
}

impl RestartPhase {
    const fn as_str(self) -> &'static str {
        match self {
            Self::PersistLoad => "persist_load",
            Self::Stop => "stop",
            Self::Cleanup => "cleanup",
            Self::Start => "start",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestartFailure {
    pub network_name: String,
    pub phase: RestartPhase,
    pub message: String,
}

/// Registry/state/service checks performed after an explicit stop. These counts
/// do not claim that kernel TUN, route, policy-route or DNS state was audited.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RuntimeCleanupReport {
    pub sessions_remaining: usize,
    pub listeners_remaining: usize,
    pub peers_remaining: usize,
    pub embedded_services_remaining: usize,
    pub configuration_retained: bool,
    /// Per-network runtime cleanup results returned by the owner of the TUN,
    /// routes, DNS, relay and P2P resources.
    pub runtime_outcomes: BTreeMap<String, RuntimeCleanupOutcome>,
    /// Networks with a session, listener or peer residue. Embedded services
    /// are intentionally retained across a network restart and never appear
    /// here.
    pub residual_networks: Vec<String>,
}

impl RuntimeCleanupReport {
    fn is_clean_for_stop(&self) -> bool {
        self.sessions_remaining == 0
            && self.listeners_remaining == 0
            && self.peers_remaining == 0
            && self.embedded_services_remaining == 0
            && self.residual_networks.is_empty()
            && self.resource_failures().is_empty()
    }

    fn stop_issues(&self) -> Vec<String> {
        let mut issues = Vec::new();
        if !self.residual_networks.is_empty() {
            issues.push(format!("残留网络: {}", self.residual_networks.join(", ")));
        }
        if self.sessions_remaining > 0 {
            issues.push(format!("运行时任务残留 {} 个", self.sessions_remaining));
        }
        if self.listeners_remaining > 0 {
            issues.push(format!("监听器残留 {} 个", self.listeners_remaining));
        }
        if self.peers_remaining > 0 {
            issues.push(format!("Peer 记录残留 {} 个", self.peers_remaining));
        }
        if self.embedded_services_remaining > 0 {
            issues.push(format!(
                "内嵌服务残留 {} 个",
                self.embedded_services_remaining
            ));
        }
        issues.extend(self.resource_failures());
        issues
    }

    fn blocks_restart(&self, network_name: &str) -> bool {
        self.residual_networks
            .iter()
            .any(|name| name == network_name)
            || self
                .runtime_outcomes
                .get(network_name)
                .is_some_and(|outcome| !outcome.is_clean())
    }

    fn resource_failures(&self) -> Vec<String> {
        self.runtime_outcomes
            .iter()
            .filter_map(|(network, outcome)| {
                let issues = outcome.issues();
                (!issues.is_empty()).then(|| format!("{network}: {}", issues.join("; ")))
            })
            .collect()
    }

    fn to_public(&self) -> NodeRuntimeResidueSnapshot {
        let cleanup_steps = self
            .runtime_outcomes
            .iter()
            .flat_map(|(network_name, outcome)| {
                if outcome.steps.is_empty() {
                    return vec![NodeCleanupStepSnapshot {
                        network_name: network_name.clone(),
                        resource: "运行时清理".into(),
                        status: NodeCleanupStepStatus::Unknown,
                        detail: Some("运行时未返回清理结果".into()),
                    }];
                }
                outcome
                    .steps
                    .iter()
                    .map(|step| {
                        let (status, detail) = match &step.status {
                            CleanupStatus::Removed => (NodeCleanupStepStatus::Removed, None),
                            CleanupStatus::NotOwned => (NodeCleanupStepStatus::NotOwned, None),
                            CleanupStatus::Deferred(_) => {
                                (NodeCleanupStepStatus::Deferred, Some("清理已延迟".into()))
                            }
                            CleanupStatus::Failed(_) => {
                                (NodeCleanupStepStatus::Failed, Some("资源清理失败".into()))
                            }
                            CleanupStatus::Unknown(_) => {
                                (NodeCleanupStepStatus::Unknown, Some("清理状态未知".into()))
                            }
                        };
                        NodeCleanupStepSnapshot {
                            network_name: network_name.clone(),
                            resource: step.resource.into(),
                            status,
                            detail,
                        }
                    })
                    .collect()
            })
            .collect::<Vec<_>>();

        let mut issues = Vec::new();
        if !self.residual_networks.is_empty() {
            issues.push(format!("残留网络: {}", self.residual_networks.join(", ")));
        }
        if self.sessions_remaining > 0 {
            issues.push(format!("运行时任务残留 {} 个", self.sessions_remaining));
        }
        if self.listeners_remaining > 0 {
            issues.push(format!("监听器残留 {} 个", self.listeners_remaining));
        }
        if self.peers_remaining > 0 {
            issues.push(format!("Peer 记录残留 {} 个", self.peers_remaining));
        }
        if self.embedded_services_remaining > 0 {
            issues.push(format!(
                "内嵌服务残留 {} 个",
                self.embedded_services_remaining
            ));
        }
        for step in &cleanup_steps {
            let issue = match step.status {
                NodeCleanupStepStatus::Removed | NodeCleanupStepStatus::NotOwned => continue,
                NodeCleanupStepStatus::Deferred => "清理已延迟",
                NodeCleanupStepStatus::Failed => "资源清理失败",
                NodeCleanupStepStatus::Unknown => "清理状态未知",
            };
            issues.push(format!("{}: {} {issue}", step.network_name, step.resource));
        }
        issues.sort();
        issues.dedup();

        NodeRuntimeResidueSnapshot {
            sessions_remaining: self.sessions_remaining,
            listeners_remaining: self.listeners_remaining,
            peers_remaining: self.peers_remaining,
            embedded_services_remaining: self.embedded_services_remaining,
            configuration_retained: self.configuration_retained,
            residual_networks: self.residual_networks.clone(),
            cleanup_steps,
            issues,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RestartSummary {
    pub requested: usize,
    pub restarted: Vec<String>,
    pub failed: Vec<RestartFailure>,
    pub cleanup: RuntimeCleanupReport,
}

impl RestartSummary {
    pub fn message(&self) -> String {
        let mut parts = vec![format!(
            "重启网络: 已重启 {} / {} 个网络",
            self.restarted.len(),
            self.requested
        )];
        if !self.failed.is_empty() {
            let failures = self
                .failed
                .iter()
                .map(|failure| {
                    format!(
                        "{} [{}]: {}",
                        failure.network_name,
                        failure.phase.as_str(),
                        failure.message
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            parts.push(format!("{} 个网络重启失败 ({failures})", self.failed.len()));
        }
        let residue = if self.cleanup.residual_networks.is_empty()
            && self.cleanup.resource_failures().is_empty()
        {
            "none".to_string()
        } else {
            let mut residue = self.cleanup.residual_networks.clone();
            residue.extend(self.cleanup.resource_failures());
            residue.join(", ")
        };
        parts.push(format!(
            "清理检查: sessions={}, listeners={}, peers={}, embedded_services={}, residual_networks=[{}]; 持久化配置{}",
            self.cleanup.sessions_remaining,
            self.cleanup.listeners_remaining,
            self.cleanup.peers_remaining,
            self.cleanup.embedded_services_remaining,
            residue,
            if self.cleanup.configuration_retained {
                "已保留"
            } else {
                "未完整保留"
            }
        ));
        parts.join("；")
    }
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
    handle: JoinHandle<NetworkRuntimeExit>,
}

/// Result returned by a network task after its event loop has exited.
///
/// A failed cleanup must not destroy the only object capable of retrying the
/// rollback. The task transfers that owner back to the actor, which keeps it
/// alive until a later cleanup attempt succeeds.
struct NetworkRuntimeExit {
    cleanup: RuntimeCleanupOutcome,
    retained_owner: Option<Box<dyn ResidualRuntimeOwner>>,
}

trait ResidualRuntimeOwner: Send + Sync {
    fn shutdown(&mut self) -> Pin<Box<dyn Future<Output = RuntimeCleanupOutcome> + Send + '_>>;
}

struct LiveResidualRuntimeOwner {
    runtime: commands::ClientRuntime,
    registry: MultiNetworkRegistry,
    network_name: String,
}

impl ResidualRuntimeOwner for LiveResidualRuntimeOwner {
    fn shutdown(&mut self) -> Pin<Box<dyn Future<Output = RuntimeCleanupOutcome> + Send + '_>> {
        Box::pin(async move {
            let outcome = self.runtime.shutdown().await;
            if outcome.is_clean() {
                self.registry.unregister(&self.network_name).await;
            }
            outcome
        })
    }
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
                let (disconnect_reply, event_error) =
                    match runtime.run_with_shutdown(&mut shutdown_rx).await {
                        Ok(reply) => (reply, None),
                        Err(error) => {
                            tracing::error!("网络 [{network_name}] 事件循环异常退出: {error:#}");
                            (None, Some(format!("{error:#}")))
                        }
                    };
                tracing::info!("网络 [{network_name}] 正在清理资源");
                let cleanup = runtime.shutdown().await;
                let cleanup_clean = cleanup.is_clean();
                let retained_owner = if cleanup_clean {
                    drop(runtime);
                    None
                } else {
                    Some(Box::new(LiveResidualRuntimeOwner {
                        runtime,
                        registry: registry.clone(),
                        network_name: network_name.clone(),
                    }) as Box<dyn ResidualRuntimeOwner>)
                };
                let cleanup_issues = cleanup.issues();
                let status = if cleanup_clean && event_error.is_none() {
                    NodeNetworkStatus::Offline
                } else {
                    NodeNetworkStatus::Failed
                };
                let failure_message = event_error
                    .iter()
                    .chain(cleanup_issues.iter())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; ");

                if cleanup_clean {
                    registry.unregister(&network_name).await;
                    state
                        .update(|snapshot| {
                            if let Some(network) = snapshot
                                .networks
                                .iter_mut()
                                .find(|network| network.name == network_name)
                            {
                                network.status = status;
                                network.assigned_ipv6 = None;
                                network.assigned_ipv4 = None;
                                network.p2p_listen_addr = None;
                                network.uptime_secs = 0;
                                network.peer_count = 0;
                            }
                            snapshot
                                .peers
                                .retain(|peer| peer.network_name != network_name);
                            if !failure_message.is_empty() {
                                snapshot.last_error = Some(
                                    NodeProblem::new(
                                        NodeProblemCode::Unavailable,
                                        failure_message.clone(),
                                    )
                                    .for_network(&network_name)
                                    .retryable(true),
                                );
                            }
                        })
                        .await;
                } else {
                    state
                        .update(|snapshot| {
                            if let Some(network) = snapshot
                                .networks
                                .iter_mut()
                                .find(|network| network.name == network_name)
                            {
                                network.status = NodeNetworkStatus::Failed;
                            }
                            snapshot.last_error = Some(
                                NodeProblem::new(
                                    NodeProblemCode::Unavailable,
                                    format!(
                                        "网络 [{network_name}] 资源清理未完成: {}",
                                        cleanup.summary()
                                    ),
                                )
                                .for_network(&network_name)
                                .retryable(true),
                            );
                        })
                        .await;
                }
                events.publish(NodeEventKind::NetworkChanged {
                    network_name: network_name.clone(),
                    status,
                });
                if let Some(reply) = disconnect_reply {
                    let result = if cleanup_clean {
                        Ok(())
                    } else {
                        Err(cleanup.summary())
                    };
                    let _ = reply.send(result);
                }
                if cleanup_clean {
                    tracing::info!("网络 [{network_name}] 已安全下线");
                } else {
                    tracing::error!(
                        "网络 [{network_name}] 清理未完成，保留残留状态: {}",
                        cleanup.summary()
                    );
                }
                NetworkRuntimeExit {
                    cleanup,
                    retained_owner,
                }
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
    handle: JoinHandle<NetworkRuntimeExit>,
}

/// A manager-wide single-flight gate for lifecycle operations. The permit is
/// moved into the actor command, so cancellation of the caller cannot release
/// it while the actor is still changing runtime state.
struct OperationGate {
    busy: AtomicBool,
}

struct OperationGuard {
    gate: Arc<OperationGate>,
}

impl OperationGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            busy: AtomicBool::new(false),
        })
    }

    fn try_acquire(self: &Arc<Self>) -> Result<OperationGuard, RuntimeControlError> {
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| OperationGuard { gate: self.clone() })
            .map_err(|_| {
                RuntimeControlError::Conflict(
                    "已有网络启动、创建、加入、停止或重启操作正在进行".into(),
                )
            })
    }
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        self.gate.busy.store(false, Ordering::Release);
    }
}

enum RuntimeControlCommand {
    Diagnostics {
        reply: oneshot::Sender<Result<NodeDiagnosticsSnapshot, RuntimeControlError>>,
    },
    StartPlan {
        plan: NodeRuntimePlan,
        _operation: OperationGuard,
        reply: oneshot::Sender<Result<StartSummary, RuntimeControlError>>,
    },
    StartSelection {
        selection: NodeNetworkSelection,
        _operation: OperationGuard,
        reply: oneshot::Sender<Result<StartSummary, RuntimeControlError>>,
    },
    Create {
        command: NodeCreateNetworkCommand,
        _operation: OperationGuard,
        reply: oneshot::Sender<Result<CreateResult, RuntimeControlError>>,
    },
    Join {
        command: NodeJoinNetworkCommand,
        _operation: OperationGuard,
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
        _operation: OperationGuard,
        reply: oneshot::Sender<Result<StopSummary, RuntimeControlError>>,
    },
    Restart {
        networks: Vec<String>,
        all: bool,
        _operation: OperationGuard,
        reply: oneshot::Sender<Result<RestartSummary, RuntimeControlError>>,
    },
    StartNode {
        _operation: OperationGuard,
        reply: oneshot::Sender<Result<StartSummary, RuntimeControlError>>,
    },
    StopNode {
        _operation: OperationGuard,
        reply: oneshot::Sender<Result<StopSummary, RuntimeControlError>>,
    },
    RestartNode {
        _operation: OperationGuard,
        reply: oneshot::Sender<Result<RestartSummary, RuntimeControlError>>,
    },
    CleanupResiduals {
        networks: Vec<String>,
        all: bool,
        _operation: OperationGuard,
        reply: oneshot::Sender<Result<StopSummary, RuntimeControlError>>,
    },
    Shutdown {
        _operation: OperationGuard,
        reply: oneshot::Sender<Result<StopSummary, RuntimeControlError>>,
    },
}

/// Cloneable control handle for the actor that owns live network runtimes.
#[derive(Clone)]
pub(crate) struct NetworkRuntimeManager {
    sender: mpsc::Sender<RuntimeControlCommand>,
    operation_gate: Arc<OperationGate>,
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
            configs, options, state, events, lifecycle, registry, services, launcher,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_with_launcher(
        configs: ConfigManager,
        options: RuntimeOptions,
        state: NodeStateStore,
        events: NodeEventBus,
        lifecycle: NodeLifecycle,
        registry: MultiNetworkRegistry,
        services: EmbeddedServiceManager,
        launcher: Arc<dyn NetworkRuntimeLauncher>,
    ) -> Self {
        Self::spawn_with_launcher_and_timeout(
            configs,
            options,
            state,
            events,
            lifecycle,
            registry,
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
        registry: MultiNetworkRegistry,
        services: EmbeddedServiceManager,
        launcher: Arc<dyn NetworkRuntimeLauncher>,
        routing_update_timeout: Duration,
    ) -> Self {
        Self::spawn_with_launcher_and_timeouts(
            configs,
            options,
            state,
            events,
            lifecycle,
            registry,
            services,
            launcher,
            RuntimeTimeouts {
                routing_update: routing_update_timeout,
                ..RuntimeTimeouts::default()
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_with_launcher_and_timeouts(
        configs: ConfigManager,
        options: RuntimeOptions,
        state: NodeStateStore,
        events: NodeEventBus,
        lifecycle: NodeLifecycle,
        registry: MultiNetworkRegistry,
        services: EmbeddedServiceManager,
        launcher: Arc<dyn NetworkRuntimeLauncher>,
        timeouts: RuntimeTimeouts,
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
                registry,
                services,
                launcher,
                sessions: HashMap::new(),
                retained_owners: HashMap::new(),
                cleanup_failures: BTreeMap::new(),
                completed_cleanup: BTreeMap::new(),
                timeouts,
            },
        ));
        Self {
            sender,
            operation_gate: OperationGate::new(),
        }
    }

    pub async fn start_plan(
        &self,
        plan: NodeRuntimePlan,
    ) -> Result<StartSummary, RuntimeControlError> {
        let operation = self.operation_gate.try_acquire()?;
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::StartPlan {
            plan,
            _operation: operation,
            reply,
        })
        .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn start_selection(
        &self,
        selection: NodeNetworkSelection,
    ) -> Result<StartSummary, RuntimeControlError> {
        let operation = self.operation_gate.try_acquire()?;
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::StartSelection {
            selection,
            _operation: operation,
            reply,
        })
        .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn create(
        &self,
        command: NodeCreateNetworkCommand,
    ) -> Result<CreateResult, RuntimeControlError> {
        let operation = self.operation_gate.try_acquire()?;
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::Create {
            command,
            _operation: operation,
            reply,
        })
        .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn join(
        &self,
        command: NodeJoinNetworkCommand,
    ) -> Result<JoinResult, RuntimeControlError> {
        let operation = self.operation_gate.try_acquire()?;
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::Join {
            command,
            _operation: operation,
            reply,
        })
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
        let operation = self.operation_gate.try_acquire()?;
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::Stop {
            networks,
            all,
            _operation: operation,
            reply,
        })
        .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn restart(
        &self,
        networks: Vec<String>,
        all: bool,
    ) -> Result<RestartSummary, RuntimeControlError> {
        let operation = self.operation_gate.try_acquire()?;
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::Restart {
            networks,
            all,
            _operation: operation,
            reply,
        })
        .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    /// Restore every persisted network while retaining the node IPC process.
    pub async fn start_node(&self) -> Result<StartSummary, RuntimeControlError> {
        let operation = self.operation_gate.try_acquire()?;
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::StartNode {
            _operation: operation,
            reply,
        })
        .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    /// Stop every data-plane session and owned embedded service while retaining
    /// the node IPC process so a later start can restore persisted networks.
    pub async fn stop_node(&self) -> Result<StopSummary, RuntimeControlError> {
        let operation = self.operation_gate.try_acquire()?;
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::StopNode {
            _operation: operation,
            reply,
        })
        .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    /// Replace the complete data plane from persisted configuration under one
    /// operation permit and one restart deadline.
    pub async fn restart_node(&self) -> Result<RestartSummary, RuntimeControlError> {
        let operation = self.operation_gate.try_acquire()?;
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::RestartNode {
            _operation: operation,
            reply,
        })
        .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn cleanup_residuals(
        &self,
        networks: Vec<String>,
        all: bool,
    ) -> Result<StopSummary, RuntimeControlError> {
        let operation = self.operation_gate.try_acquire()?;
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::CleanupResiduals {
            networks,
            all,
            _operation: operation,
            reply,
        })
        .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn diagnostics(&self) -> Result<NodeDiagnosticsSnapshot, RuntimeControlError> {
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::Diagnostics { reply })
            .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn shutdown(&self) -> Result<StopSummary, RuntimeControlError> {
        let operation = self.operation_gate.try_acquire()?;
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::Shutdown {
            _operation: operation,
            reply,
        })
        .await?;
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
            RuntimeControlCommand::Diagnostics { reply } => {
                let result = actor.diagnostics().await;
                let _ = reply.send(result);
            }
            RuntimeControlCommand::StartPlan {
                plan,
                _operation,
                reply,
            } => {
                let result = actor.start_plan(plan).await;
                let _ = reply.send(result);
            }
            RuntimeControlCommand::StartSelection {
                selection,
                _operation,
                reply,
            } => {
                let result = actor.start_selection(selection).await;
                let _ = reply.send(result);
            }
            RuntimeControlCommand::Create {
                command,
                _operation,
                reply,
            } => {
                let result = actor.create(command).await;
                let _ = reply.send(result);
            }
            RuntimeControlCommand::Join {
                command,
                _operation,
                reply,
            } => {
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
                _operation,
                reply,
            } => {
                let result = actor.stop(networks, all).await;
                let _ = reply.send(result);
            }
            RuntimeControlCommand::Restart {
                networks,
                all,
                _operation,
                reply,
            } => {
                let result = actor.restart(networks, all).await;
                let _ = reply.send(result);
            }
            RuntimeControlCommand::StartNode { _operation, reply } => {
                let result = actor.start_node().await;
                let _ = reply.send(result);
            }
            RuntimeControlCommand::StopNode { _operation, reply } => {
                let result = actor.stop_node().await;
                let _ = reply.send(result);
            }
            RuntimeControlCommand::RestartNode { _operation, reply } => {
                let result = actor.restart_node().await;
                let _ = reply.send(result);
            }
            RuntimeControlCommand::CleanupResiduals {
                networks,
                all,
                _operation,
                reply,
            } => {
                let result = actor.cleanup_residuals(networks, all).await;
                let _ = reply.send(result);
            }
            RuntimeControlCommand::Shutdown { _operation, reply } => {
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
    registry: MultiNetworkRegistry,
    services: EmbeddedServiceManager,
    launcher: Arc<dyn NetworkRuntimeLauncher>,
    sessions: HashMap<String, NetworkRuntimeSession>,
    retained_owners: HashMap<String, Box<dyn ResidualRuntimeOwner>>,
    cleanup_failures: BTreeMap<String, RuntimeCleanupOutcome>,
    completed_cleanup: BTreeMap<String, RuntimeCleanupOutcome>,
    timeouts: RuntimeTimeouts,
}

impl NetworkRuntimeActor {
    async fn start_node(&mut self) -> Result<StartSummary, RuntimeControlError> {
        self.start_node_until(Instant::now() + self.timeouts.start)
            .await
    }

    async fn fail_lifecycle(&self, error: RuntimeControlError) -> RuntimeControlError {
        let lifecycle = self.lifecycle.failed().await;
        self.state.set_lifecycle(lifecycle).await;
        error
    }

    async fn start_node_until(
        &mut self,
        deadline: Instant,
    ) -> Result<StartSummary, RuntimeControlError> {
        let networks = self.configs.list_networks().map_err(|error| {
            RuntimeControlError::Persistence(format!("加载持久化网络列表失败: {error:#}"))
        })?;
        let configured = networks.len();
        self.state
            .update(|snapshot| {
                snapshot.permissions = runtime_metadata::trusted_control_plane_permissions();
                snapshot.configuration.networks.clear();
            })
            .await;
        let lifecycle = self.lifecycle.begin_start(configured, Vec::new()).await;
        self.state.set_lifecycle(lifecycle).await;

        let plan = match NodeRuntimePlan::load(&self.configs, NodeNetworkSelection::All) {
            Ok(plan) => plan,
            Err(error) => {
                let lifecycle = self.lifecycle.failed().await;
                self.state.set_lifecycle(lifecycle).await;
                return Err(RuntimeControlError::Persistence(format!(
                    "加载节点启动计划失败: {error:#}"
                )));
            }
        };
        match self
            .start_persisted_plan_until(networks, plan, deadline, &BTreeSet::new())
            .await
        {
            Ok(summary) => Ok(summary),
            Err(error) => Err(self.fail_lifecycle(error).await),
        }
    }

    async fn start_persisted_plan_until(
        &mut self,
        networks: Vec<NetworkConfig>,
        plan: NodeRuntimePlan,
        deadline: Instant,
        skip_networks: &BTreeSet<String>,
    ) -> Result<StartSummary, RuntimeControlError> {
        self.reconcile_finished_sessions().await;
        let configured = plan.len();
        let mut summary = StartSummary {
            requested: configured,
            ..StartSummary::default()
        };

        let running_services = self
            .services
            .list()
            .await
            .map_err(map_service_error)?
            .into_iter()
            .map(|service| service.network_name)
            .collect::<BTreeSet<_>>();
        let mut service_failures = HashMap::new();
        for network in networks.iter().filter(|network| {
            network.owns_embedded_services && !skip_networks.contains(&network.name)
        }) {
            if let Err(error) = self
                .restore_owned_embedded_service(network, &running_services, deadline)
                .await
            {
                service_failures.insert(network.name.clone(), error.to_string());
            }
        }

        for spec in plan.into_networks() {
            if skip_networks.contains(&spec.name) {
                continue;
            }
            if self.sessions.contains_key(&spec.name) {
                summary.already_running.push(spec.name);
                continue;
            }
            if let Some(message) = service_failures.remove(&spec.name) {
                let error = RuntimeControlError::Launch(format!(
                    "内嵌服务恢复失败，未启动数据面: {message}"
                ));
                self.record_launch_failure(&spec.name, &error).await;
                summary.failed.push(StartFailure {
                    network_name: spec.name,
                    message: error.to_string(),
                });
                continue;
            }
            let network_name = spec.name.clone();
            match self.start_spec_until(spec, deadline).await {
                Ok(()) => summary.started.push(network_name),
                Err(error) => summary.failed.push(StartFailure {
                    network_name,
                    message: error.to_string(),
                }),
            }
        }

        self.reconcile_finished_sessions().await;
        let active = self.sessions.len();
        let failed = summary
            .failed
            .iter()
            .map(|failure| failure.network_name.clone())
            .collect::<Vec<_>>();
        let lifecycle = if configured > 0 && active == 0 {
            self.lifecycle
                .failed_with_configuration(configured, failed)
                .await
        } else {
            self.lifecycle.running(active, failed).await
        };
        self.state.set_lifecycle(lifecycle).await;
        Ok(summary)
    }

    async fn restore_owned_embedded_service(
        &self,
        network: &NetworkConfig,
        running_services: &BTreeSet<String>,
        deadline: Instant,
    ) -> Result<(), RuntimeControlError> {
        if running_services.contains(&network.name) {
            if let Some(service) = self
                .services
                .list()
                .await
                .map_err(map_service_error)?
                .into_iter()
                .find(|service| service.network_name == network.name)
            {
                self.record_running_embedded_service(&service).await;
                return Ok(());
            }
        }

        self.mark_embedded_service_status(&network.name, NodeEmbeddedServiceStatus::Starting)
            .await;
        let host_public_key = self
            .configs
            .load_node_identity()
            .map_err(|error| {
                RuntimeControlError::Persistence(format!(
                    "网络 [{}] 加载节点身份失败: {error:#}",
                    network.name
                ))
            })?
            .public_key_bytes();
        let request = EmbeddedServiceStartRequest::new(
            network.name.clone(),
            network.signaling_port.unwrap_or(0),
            network.relay_port.unwrap_or(0),
        )
        .with_host_public_key(host_public_key);
        let request = match network.external_host.clone() {
            Some(host) => request.with_external_host(host),
            None => request.with_external_host("127.0.0.1"),
        };
        let request = match network.ip_pool.clone() {
            Some(pool) => request.with_ip_pool(pool),
            None => request,
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            self.mark_embedded_service_status(&network.name, NodeEmbeddedServiceStatus::Failed)
                .await;
            return Err(RuntimeControlError::Timeout(format!(
                "start/timeout: 网络 [{}] 的内嵌服务未在启动期限内恢复",
                network.name
            )));
        }
        let service = match timeout(remaining, self.services.start(request)).await {
            Ok(Ok(service)) => service,
            Ok(Err(error)) => {
                self.mark_embedded_service_status(&network.name, NodeEmbeddedServiceStatus::Failed)
                    .await;
                return Err(map_service_error(error));
            }
            Err(_) => {
                self.mark_embedded_service_status(&network.name, NodeEmbeddedServiceStatus::Failed)
                    .await;
                return Err(RuntimeControlError::Timeout(format!(
                    "start/timeout: 网络 [{}] 的内嵌服务未在期限内恢复",
                    network.name
                )));
            }
        };
        self.record_running_embedded_service(&service).await;

        if let Err(error) = self.configs.set_embedded_service_metadata(
            &network.name,
            service.signaling_addr.port(),
            service.relay_addr.port(),
            network.external_host.clone(),
            network.ip_pool.clone(),
        ) {
            self.stop_embedded_service(&network.name).await;
            return Err(RuntimeControlError::Persistence(format!(
                "网络 [{}] 的内嵌服务已启动，但端点持久化失败: {error:#}",
                network.name
            )));
        }
        Ok(())
    }

    async fn stop_node(&mut self) -> Result<StopSummary, RuntimeControlError> {
        let deadline = Instant::now() + self.timeouts.stop;
        let lifecycle = self.lifecycle.begin_stop().await;
        self.state.set_lifecycle(lifecycle).await;
        let summary = match self.stop_until(Vec::new(), true, true, deadline).await {
            Ok(summary) => summary,
            Err(error) => return Err(self.fail_lifecycle(error).await),
        };
        let cleanly_stopped = summary.is_success();
        let lifecycle = if cleanly_stopped {
            self.lifecycle.stopped().await
        } else {
            self.lifecycle.failed().await
        };
        self.state.set_lifecycle(lifecycle).await;
        Ok(summary)
    }

    async fn restart_node(&mut self) -> Result<RestartSummary, RuntimeControlError> {
        let deadline = Instant::now() + self.timeouts.restart;
        let networks = self.configs.list_networks().map_err(|error| {
            RuntimeControlError::Persistence(format!(
                "restart/{}: 加载持久化网络列表失败，现有运行态未改变: {error:#}",
                RestartPhase::PersistLoad.as_str()
            ))
        })?;
        let plan =
            NodeRuntimePlan::load(&self.configs, NodeNetworkSelection::All).map_err(|error| {
                RuntimeControlError::Persistence(format!(
                    "restart/{}: 加载重启配置失败，现有运行态未改变: {error:#}",
                    RestartPhase::PersistLoad.as_str()
                ))
            })?;
        let names = plan
            .networks()
            .iter()
            .map(|network| network.name.clone())
            .collect::<Vec<_>>();

        let lifecycle = self.lifecycle.begin_stop().await;
        self.state.set_lifecycle(lifecycle).await;
        let stopped = match self.stop_until(Vec::new(), true, true, deadline).await {
            Ok(summary) => summary,
            Err(error) => return Err(self.fail_lifecycle(error).await),
        };

        let mut summary = RestartSummary {
            requested: plan.len(),
            ..RestartSummary::default()
        };
        for failure in stopped.failed {
            summary.failed.push(RestartFailure {
                network_name: failure.network_name,
                phase: RestartPhase::Stop,
                message: failure.message,
            });
        }

        let stopped_runtime_outcomes = stopped.cleanup.runtime_outcomes;
        summary.cleanup = match self.inspect_cleanup(&names).await {
            Ok(cleanup) => cleanup,
            Err(error) => return Err(self.fail_lifecycle(error).await),
        };
        summary.cleanup.runtime_outcomes = stopped_runtime_outcomes.clone();

        let mut blocked = summary
            .failed
            .iter()
            .map(|failure| failure.network_name.clone())
            .collect::<BTreeSet<_>>();
        for name in &names {
            if summary.cleanup.blocks_restart(name) && !blocked.contains(name) {
                summary.failed.push(RestartFailure {
                    network_name: name.clone(),
                    phase: RestartPhase::Cleanup,
                    message: cleanup_block_message(&summary.cleanup, name),
                });
                blocked.insert(name.clone());
            }
        }

        let lifecycle = self.lifecycle.begin_start(plan.len(), Vec::new()).await;
        self.state.set_lifecycle(lifecycle).await;
        let started = match self
            .start_persisted_plan_until(networks, plan, deadline, &blocked)
            .await
        {
            Ok(summary) => summary,
            Err(error) => return Err(self.fail_lifecycle(error).await),
        };
        summary.restarted = started.started;
        for failure in started.failed {
            summary.failed.push(RestartFailure {
                network_name: failure.network_name,
                phase: RestartPhase::Start,
                message: failure.message,
            });
        }
        summary.cleanup = match self.inspect_cleanup(&names).await {
            Ok(mut cleanup) => {
                cleanup.runtime_outcomes = stopped_runtime_outcomes;
                cleanup
            }
            Err(error) => return Err(self.fail_lifecycle(error).await),
        };
        self.publish_restart_lifecycle(&summary).await;
        Ok(summary)
    }

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
        let deadline = Instant::now() + self.timeouts.start;
        self.start_plan_until(plan, deadline).await
    }

    async fn start_plan_until(
        &mut self,
        plan: NodeRuntimePlan,
        deadline: Instant,
    ) -> Result<StartSummary, RuntimeControlError> {
        self.reconcile_finished_sessions().await;
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
            match self.start_spec_until(spec, deadline).await {
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
        spec: super::NodeNetworkSpec,
    ) -> Result<(), RuntimeControlError> {
        self.start_spec_until(spec, Instant::now() + self.timeouts.start)
            .await
    }

    async fn start_spec_until(
        &mut self,
        mut spec: super::NodeNetworkSpec,
        deadline: Instant,
    ) -> Result<(), RuntimeControlError> {
        if let Some(outcome) = self.cleanup_failures.get(&spec.name) {
            return Err(RuntimeControlError::Unavailable(format!(
                "start/cleanup: 网络 [{}] 的既有运行资源尚未清理，禁止重新启动: {}",
                spec.name,
                outcome.summary()
            )));
        }
        if self.retained_owners.contains_key(&spec.name) {
            return Err(RuntimeControlError::Unavailable(format!(
                "start/cleanup: 网络 [{}] 仍保留可重试的运行时资源所有者，禁止重新启动",
                spec.name
            )));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(RuntimeControlError::Timeout(format!(
                "start/timeout: 网络 [{}] 未在启动期限内开始",
                spec.name
            )));
        }
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
        let launch = self.launcher.launch(NetworkLaunchRequest {
            network_name: network_name.clone(),
            config_path: spec.config_path.to_string_lossy().into_owned(),
            config: spec.config,
        });
        let started = match timeout(remaining.min(self.timeouts.start), launch).await {
            Ok(started) => started,
            Err(_) => {
                let error = RuntimeControlError::Timeout(format!(
                    "start/timeout: 网络 [{network_name}] 未在启动期限内完成"
                ));
                self.record_launch_failure(&network_name, &error).await;
                return Err(error);
            }
        };
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
        self.cleanup_failures.remove(&network_name);
        self.retained_owners.remove(&network_name);
        self.completed_cleanup.remove(&network_name);
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

        let host_public_key = self
            .configs
            .load_node_identity()
            .map_err(|error| {
                RuntimeControlError::Persistence(format!(
                    "网络 [{}] 加载节点身份失败: {error:#}",
                    command.name
                ))
            })?
            .public_key_bytes();
        let service_request = EmbeddedServiceStartRequest::new(
            command.name.clone(),
            command.signaling_port,
            command.relay_port,
        )
        .with_host_public_key(host_public_key);
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
        if let Err(error) = self.configs.set_embedded_service_metadata(
            &command.name,
            service.signaling_addr.port(),
            service.relay_addr.port(),
            command.external_host.clone(),
            None,
        ) {
            self.rollback_created_network(&command.name).await;
            return Err(RuntimeControlError::Persistence(format!(
                "网络 [{}] 的内嵌服务已启动，但端点持久化失败，已回滚: {error:#}",
                command.name
            )));
        }
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
        self.reconcile_finished_sessions().await;

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
            self.timeouts.routing_update,
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
                    self.timeouts.routing_update,
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
                rollback_runtime_routing(&routing_control, &previous, self.timeouts.routing_update)
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
        self.reconcile_finished_sessions().await;

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
            self.timeouts.routing_update,
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
                    self.timeouts.routing_update,
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
                self.timeouts.routing_update,
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
        let deadline = Instant::now() + self.timeouts.stop;
        self.stop_until(networks, all, true, deadline).await
    }

    async fn stop_until(
        &mut self,
        networks: Vec<String>,
        all: bool,
        stop_embedded_services: bool,
        deadline: Instant,
    ) -> Result<StopSummary, RuntimeControlError> {
        self.reconcile_finished_sessions().await;
        let services = if stop_embedded_services {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(RuntimeControlError::Timeout(
                    "stop/timeout: 在枚举待停止网络前已超过期限".into(),
                ));
            }
            match timeout(remaining, self.services.list()).await {
                Ok(Ok(services)) => services
                    .into_iter()
                    .map(|service| service.network_name)
                    .collect::<BTreeSet<_>>(),
                Ok(Err(error)) => return Err(map_service_error(error)),
                Err(_) => {
                    return Err(RuntimeControlError::Timeout(
                        "stop/timeout: 枚举内嵌服务超过停止期限".into(),
                    ));
                }
            }
        } else {
            BTreeSet::new()
        };
        let targets = if all {
            self.sessions
                .keys()
                .cloned()
                .chain(services.iter().cloned())
                .chain(self.cleanup_failures.keys().cloned())
                .chain(self.completed_cleanup.keys().cloned())
                .collect::<BTreeSet<_>>()
        } else {
            let requested = networks.into_iter().collect::<BTreeSet<_>>();
            let missing = requested
                .iter()
                .filter(|name| {
                    !self.sessions.contains_key(*name)
                        && !services.contains(*name)
                        && !self.cleanup_failures.contains_key(*name)
                        && !self.completed_cleanup.contains_key(*name)
                })
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

        // Send every shutdown signal before waiting. This lets independent
        // network actors and owned services stop concurrently under one
        // manager-wide deadline instead of multiplying the timeout by target
        // count.
        let mut pending_sessions = Vec::new();
        let mut service_targets = Vec::new();
        for name in &targets {
            if let Some(session) = self.sessions.remove(name) {
                self.mark_network_status(Some(name), NodeNetworkStatus::Stopping)
                    .await;
                let accepted = session.shutdown.send(true).is_ok();
                pending_sessions.push((name.clone(), session, accepted));
            }
            let has_embedded_service = services.contains(name);
            if has_embedded_service {
                self.mark_embedded_service_status(name, NodeEmbeddedServiceStatus::Stopping)
                    .await;
                service_targets.push(name.clone());
            }
        }

        let services = self.services.clone();
        let session_waits =
            pending_sessions
                .into_iter()
                .map(|(network_name, session, accepted)| async move {
                    wait_for_session_shutdown(network_name.clone(), session, accepted, deadline)
                        .await
                });
        let service_waits = service_targets.into_iter().map(|network_name| {
            let services = services.clone();
            async move {
                let result = stop_embedded_service_until(&services, &network_name, deadline).await;
                (network_name, result)
            }
        });
        let (session_results, service_results) =
            tokio::join!(join_all(session_waits), join_all(service_waits));

        let mut failures = BTreeMap::<String, Vec<String>>::new();
        let mut runtime_outcomes = BTreeMap::new();
        let mut clean_session_names = BTreeSet::new();
        let mut observed_session_names = BTreeSet::new();
        for session_result in session_results {
            let network_name = session_result.network_name;
            observed_session_names.insert(network_name.clone());
            if let Some(session) = session_result.retry_session {
                // The task is still running its bounded cleanup. Keep the
                // session so a later stop can observe the real outcome.
                self.sessions.insert(network_name.clone(), session);
                let message = session_result.result.err().map_or_else(
                    || "停止任务未返回清理结果".into(),
                    |error| error.to_string(),
                );
                failures
                    .entry(network_name.clone())
                    .or_default()
                    .push(message.clone());
                let mut outcome = RuntimeCleanupOutcome::default();
                outcome.record("运行时清理", CleanupStatus::Unknown(message));
                self.cleanup_failures
                    .insert(network_name.clone(), outcome.clone());
                runtime_outcomes.insert(network_name.clone(), outcome);
                self.mark_network_status(Some(&network_name), NodeNetworkStatus::Failed)
                    .await;
                continue;
            }

            match session_result.result {
                Ok(exit) => {
                    if exit.cleanup.is_clean() {
                        clean_session_names.insert(network_name.clone());
                        self.cleanup_failures.remove(&network_name);
                        self.retained_owners.remove(&network_name);
                    } else {
                        if let Some(owner) = exit.retained_owner {
                            self.retained_owners.insert(network_name.clone(), owner);
                        }
                        failures
                            .entry(network_name.clone())
                            .or_default()
                            .push(format!("资源清理未完成: {}", exit.cleanup.summary()));
                        self.cleanup_failures
                            .insert(network_name.clone(), exit.cleanup.clone());
                        self.mark_network_status(Some(&network_name), NodeNetworkStatus::Failed)
                            .await;
                    }
                    runtime_outcomes.insert(network_name, exit.cleanup);
                }
                Err(error) => {
                    let message = error.to_string();
                    failures
                        .entry(network_name.clone())
                        .or_default()
                        .push(message.clone());
                    let mut outcome = RuntimeCleanupOutcome::default();
                    outcome.record("运行时清理", CleanupStatus::Unknown(message));
                    self.cleanup_failures
                        .insert(network_name.clone(), outcome.clone());
                    runtime_outcomes.insert(network_name.clone(), outcome);
                    self.mark_network_status(Some(&network_name), NodeNetworkStatus::Failed)
                        .await;
                }
            }
        }
        for name in &targets {
            if observed_session_names.contains(name) {
                continue;
            }
            if let Some(outcome) = self.completed_cleanup.remove(name) {
                clean_session_names.insert(name.clone());
                runtime_outcomes.insert(name.clone(), outcome);
                continue;
            }
            if let Some(outcome) = self.cleanup_failures.get(name).cloned() {
                failures
                    .entry(name.clone())
                    .or_default()
                    .push(format!("资源清理仍未完成: {}", outcome.summary()));
                runtime_outcomes.insert(name.clone(), outcome);
                self.mark_network_status(Some(name), NodeNetworkStatus::Failed)
                    .await;
            }
        }

        self.finish_clean_networks(&clean_session_names).await;
        for (network_name, result) in service_results {
            match result {
                Ok(()) => {
                    self.mark_embedded_service_status(
                        &network_name,
                        NodeEmbeddedServiceStatus::Stopped,
                    )
                    .await;
                }
                Err(error) => {
                    self.mark_embedded_service_status(
                        &network_name,
                        NodeEmbeddedServiceStatus::Failed,
                    )
                    .await;
                    failures
                        .entry(network_name)
                        .or_default()
                        .push(error.to_string());
                }
            }
        }

        let target_names = targets.iter().cloned().collect::<Vec<_>>();
        let mut stopped = Vec::with_capacity(targets.len());
        let mut failed = Vec::new();
        for name in targets {
            if let Some(mut messages) = failures.remove(&name) {
                messages.sort();
                messages.dedup();
                failed.push(StopFailure {
                    network_name: name,
                    message: messages.join("; "),
                });
            } else {
                stopped.push(name);
            }
        }

        let mut cleanup = self.inspect_cleanup(&target_names).await?;
        cleanup.runtime_outcomes = runtime_outcomes;
        self.publish_stop_lifecycle(&failed).await;
        Ok(StopSummary {
            stopped,
            failed,
            cleanup,
        })
    }

    async fn restart(
        &mut self,
        networks: Vec<String>,
        all: bool,
    ) -> Result<RestartSummary, RuntimeControlError> {
        self.reconcile_finished_sessions().await;
        let deadline = Instant::now() + self.timeouts.restart;
        let targets = if all {
            self.sessions
                .keys()
                .chain(self.cleanup_failures.keys())
                .chain(self.completed_cleanup.keys())
                .cloned()
                .collect::<BTreeSet<_>>()
        } else {
            let requested = networks.into_iter().collect::<BTreeSet<_>>();
            let missing = requested
                .iter()
                .filter(|name| {
                    !self.sessions.contains_key(*name)
                        && !self.cleanup_failures.contains_key(*name)
                        && !self.completed_cleanup.contains_key(*name)
                })
                .cloned()
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                return Err(RuntimeControlError::NotFound(format!(
                    "网络未运行，无法重启: {}",
                    missing.join(", ")
                )));
            }
            requested
        };

        if targets.is_empty() {
            return Ok(RestartSummary::default());
        }
        let names = targets.into_iter().collect::<Vec<_>>();

        // Load and validate every target before changing the live runtime. A
        // malformed persisted file therefore leaves the old sessions untouched.
        let plan = NodeRuntimePlan::load(&self.configs, NodeNetworkSelection::Named(names.clone()))
            .map_err(|error| {
                RuntimeControlError::Persistence(format!(
                    "restart/{}: 加载重启配置失败，现有运行态未改变: {error:#}",
                    RestartPhase::PersistLoad.as_str()
                ))
            })?;
        if Instant::now() >= deadline {
            return Err(RuntimeControlError::Timeout(
                "restart/timeout: 在 persist_load 阶段超过期限".into(),
            ));
        }

        let mut summary = RestartSummary {
            requested: names.len(),
            ..RestartSummary::default()
        };
        let stopped_summary = self
            .stop_until(names.clone(), false, false, deadline)
            .await?;
        let mut stopped = stopped_summary
            .stopped
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        for failure in stopped_summary.failed {
            summary.failed.push(RestartFailure {
                network_name: failure.network_name,
                phase: RestartPhase::Stop,
                message: failure.message,
            });
        }

        summary.cleanup = self.inspect_cleanup(&names).await?;
        summary.cleanup.runtime_outcomes = stopped_summary.cleanup.runtime_outcomes;
        for name in &names {
            if !summary.cleanup.blocks_restart(name) {
                continue;
            }
            stopped.remove(name);
            summary
                .failed
                .retain(|failure| failure.network_name != *name);
            summary.failed.push(RestartFailure {
                network_name: name.clone(),
                phase: RestartPhase::Cleanup,
                message: cleanup_block_message(&summary.cleanup, name),
            });
        }

        for spec in plan.into_networks() {
            let network_name = spec.name.clone();
            if !stopped.contains(&network_name) {
                continue;
            }
            match self.start_spec_until(spec, deadline).await {
                Ok(()) => summary.restarted.push(network_name),
                Err(error) => summary.failed.push(RestartFailure {
                    network_name,
                    phase: RestartPhase::Start,
                    message: error.to_string(),
                }),
            }
        }

        self.publish_restart_lifecycle(&summary).await;
        Ok(summary)
    }

    async fn cleanup_residuals(
        &mut self,
        networks: Vec<String>,
        all: bool,
    ) -> Result<StopSummary, RuntimeControlError> {
        self.reconcile_finished_sessions().await;
        let requested = networks.into_iter().collect::<BTreeSet<_>>();
        let targets = if all {
            self.retained_owners
                .keys()
                .chain(self.cleanup_failures.keys())
                .cloned()
                .collect::<BTreeSet<_>>()
        } else {
            let running = requested
                .iter()
                .filter(|name| self.sessions.contains_key(*name))
                .cloned()
                .collect::<Vec<_>>();
            if !running.is_empty() {
                return Err(RuntimeControlError::Conflict(format!(
                    "以下网络仍在运行，必须先等待运行会话退出再清理残留: {}",
                    running.join(", ")
                )));
            }
            let missing = requested
                .iter()
                .filter(|name| {
                    !self.retained_owners.contains_key(*name)
                        && !self.cleanup_failures.contains_key(*name)
                })
                .cloned()
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                return Err(RuntimeControlError::NotFound(format!(
                    "以下网络没有可重试的运行时残留所有者: {}",
                    missing.join(", ")
                )));
            }
            requested
        };

        let deadline = Instant::now() + self.timeouts.stop;
        let target_names = targets.iter().cloned().collect::<Vec<_>>();
        let mut cleaned = BTreeSet::new();
        let mut failed = Vec::new();
        let mut runtime_outcomes = BTreeMap::new();

        for network_name in targets {
            if self.sessions.contains_key(&network_name) {
                failed.push(StopFailure {
                    network_name: network_name.clone(),
                    message: "运行会话尚未退出，拒绝绕过会话所有权直接清理残留".into(),
                });
                continue;
            }
            let Some(owner) = self.retained_owners.get_mut(&network_name) else {
                let message = self.cleanup_failures.get(&network_name).map_or_else(
                    || "运行时资源所有者已丢失，无法安全重试清理".into(),
                    |outcome| {
                        format!(
                            "运行时资源所有者已丢失，进程重启后不可恢复；最后结果: {}",
                            outcome.summary()
                        )
                    },
                );
                failed.push(StopFailure {
                    network_name: network_name.clone(),
                    message,
                });
                continue;
            };

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                failed.push(StopFailure {
                    network_name: network_name.clone(),
                    message: "cleanup/timeout: 在清理该网络前已超过期限".into(),
                });
                continue;
            }

            let (outcome, timed_out) = match timeout(remaining, owner.shutdown()).await {
                Ok(outcome) => (outcome, false),
                Err(_) => {
                    let mut outcome = RuntimeCleanupOutcome::default();
                    outcome.record(
                        "运行时清理",
                        CleanupStatus::Deferred("重试在停止期限内未完成".into()),
                    );
                    (outcome, true)
                }
            };
            runtime_outcomes.insert(network_name.clone(), outcome.clone());
            if outcome.is_clean() {
                self.retained_owners.remove(&network_name);
                self.cleanup_failures.remove(&network_name);
                self.completed_cleanup.insert(network_name.clone(), outcome);
                cleaned.insert(network_name);
            } else {
                self.cleanup_failures
                    .insert(network_name.clone(), outcome.clone());
                self.completed_cleanup.remove(&network_name);
                failed.push(StopFailure {
                    network_name: network_name.clone(),
                    message: if timed_out {
                        format!("cleanup/timeout: 资源清理重试未完成: {}", outcome.summary())
                    } else {
                        format!("资源清理重试未完成: {}", outcome.summary())
                    },
                });
                self.mark_network_status(Some(&network_name), NodeNetworkStatus::Failed)
                    .await;
            }
        }

        self.finish_clean_networks(&cleaned).await;
        let mut cleanup = self.inspect_cleanup(&target_names).await?;
        cleanup.runtime_outcomes = runtime_outcomes;
        self.publish_dynamic_lifecycle().await;
        Ok(StopSummary {
            stopped: cleaned.into_iter().collect(),
            failed,
            cleanup,
        })
    }

    async fn diagnostics(&mut self) -> Result<NodeDiagnosticsSnapshot, RuntimeControlError> {
        self.reconcile_finished_sessions().await;
        let lifecycle = self.lifecycle.snapshot().await;
        let snapshot = self.state.snapshot().await;
        let mut names = self
            .sessions
            .keys()
            .chain(self.retained_owners.keys())
            .chain(self.cleanup_failures.keys())
            .chain(self.completed_cleanup.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        names.extend(
            self.registry
                .get_all()
                .await
                .into_iter()
                .map(|session| session.network_name),
        );
        names.extend(snapshot.networks.iter().map(|network| network.name.clone()));
        names.extend(
            snapshot
                .configuration
                .networks
                .iter()
                .map(|network| network.network_name.clone()),
        );
        names.extend(snapshot.peers.iter().map(|peer| peer.network_name.clone()));
        names.extend(
            self.services
                .list()
                .await
                .map_err(map_service_error)?
                .into_iter()
                .map(|service| service.network_name),
        );
        let names = names.into_iter().collect::<Vec<_>>();
        let mut cleanup = self.inspect_cleanup(&names).await?;
        let mut outcomes = self.completed_cleanup.clone();
        outcomes.extend(self.cleanup_failures.clone());
        cleanup.runtime_outcomes = outcomes;

        let retained_owner_networks = self.retained_owners.keys().cloned().collect::<Vec<_>>();
        let cleanup_candidate_networks = self
            .retained_owners
            .keys()
            .chain(self.cleanup_failures.keys())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        Ok(NodeDiagnosticsSnapshot {
            lifecycle,
            retained_owner_networks,
            cleanup_candidate_networks,
            residue: cleanup.to_public(),
        })
    }

    async fn finish_clean_networks(&self, clean_networks: &BTreeSet<String>) {
        if clean_networks.is_empty() {
            return;
        }
        join_all(
            clean_networks
                .iter()
                .map(|name| self.registry.unregister(name)),
        )
        .await;
        self.state
            .update(|snapshot| {
                for network in snapshot
                    .networks
                    .iter_mut()
                    .filter(|network| clean_networks.contains(network.name.as_str()))
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
                    .retain(|peer| !clean_networks.contains(peer.network_name.as_str()));
            })
            .await;
    }

    async fn inspect_cleanup(
        &self,
        names: &[String],
    ) -> Result<RuntimeCleanupReport, RuntimeControlError> {
        let name_set = names.iter().map(String::as_str).collect::<BTreeSet<_>>();
        let listener_names = self
            .registry
            .get_all()
            .await
            .iter()
            .filter(|session| name_set.contains(session.network_name.as_str()))
            .map(|session| session.network_name.clone())
            .collect::<BTreeSet<_>>();
        let peer_names = self
            .state
            .snapshot()
            .await
            .peers
            .iter()
            .filter(|peer| name_set.contains(peer.network_name.as_str()))
            .map(|peer| peer.network_name.clone())
            .collect::<BTreeSet<_>>();
        let session_names = names
            .iter()
            .filter(|name| self.sessions.contains_key(*name))
            .cloned()
            .collect::<BTreeSet<_>>();
        let embedded_services = self
            .services
            .list()
            .await
            .map_err(map_service_error)?
            .iter()
            .filter(|service| name_set.contains(service.network_name.as_str()))
            .count();
        let residual_networks = session_names
            .iter()
            .chain(listener_names.iter())
            .chain(peer_names.iter())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        Ok(RuntimeCleanupReport {
            sessions_remaining: session_names.len(),
            listeners_remaining: listener_names.len(),
            peers_remaining: peer_names.len(),
            embedded_services_remaining: embedded_services,
            configuration_retained: names.iter().all(|name| self.configs.network_exists(name)),
            runtime_outcomes: self
                .cleanup_failures
                .iter()
                .filter(|(name, _)| name_set.contains(name.as_str()))
                .map(|(name, outcome)| (name.clone(), outcome.clone()))
                .collect(),
            residual_networks,
        })
    }

    async fn shutdown(&mut self) -> Result<StopSummary, RuntimeControlError> {
        let summary = self.stop(Vec::new(), true).await?;
        match self.services.shutdown().await {
            Ok(_) | Err(EmbeddedServiceError::ActorClosed) => Ok(summary),
            Err(error) => Err(map_service_error(error)),
        }
    }

    async fn reconcile_finished_sessions(&mut self) {
        let finished = self
            .sessions
            .iter()
            .filter(|(_, session)| session.handle.is_finished())
            .map(|(network_name, _)| network_name.clone())
            .collect::<Vec<_>>();

        for network_name in finished {
            let Some(session) = self.sessions.remove(&network_name) else {
                continue;
            };
            match session.handle.await {
                Ok(exit) if exit.cleanup.is_clean() => {
                    self.cleanup_failures.remove(&network_name);
                    self.retained_owners.remove(&network_name);
                    self.completed_cleanup
                        .insert(network_name.clone(), exit.cleanup);
                    self.registry.unregister(&network_name).await;
                }
                Ok(exit) => {
                    if let Some(owner) = exit.retained_owner {
                        self.retained_owners.insert(network_name.clone(), owner);
                    }
                    self.completed_cleanup.remove(&network_name);
                    self.cleanup_failures
                        .insert(network_name.clone(), exit.cleanup);
                    self.mark_network_status(Some(&network_name), NodeNetworkStatus::Failed)
                        .await;
                }
                Err(error) => {
                    let message = format!(
                        "网络 [{network_name}] 的运行时任务异常退出，清理结果未知: {error}"
                    );
                    let mut outcome = RuntimeCleanupOutcome::default();
                    outcome.record("运行时清理", CleanupStatus::Unknown(message));
                    self.completed_cleanup.remove(&network_name);
                    self.cleanup_failures.insert(network_name.clone(), outcome);
                    self.mark_network_status(Some(&network_name), NodeNetworkStatus::Failed)
                        .await;
                }
            }
        }
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

    async fn publish_restart_lifecycle(&self, summary: &RestartSummary) {
        let failed = summary
            .failed
            .iter()
            .map(|failure| failure.network_name.clone())
            .collect();
        let lifecycle = self
            .lifecycle
            .running_with_configuration(summary.requested, self.sessions.len(), failed)
            .await;
        self.state.set_lifecycle(lifecycle).await;
    }

    async fn publish_stop_lifecycle(&self, failures: &[StopFailure]) {
        if self.lifecycle.snapshot().await.phase != NodePhase::Running {
            return;
        }
        let configured = self
            .configs
            .list_networks()
            .map_or(0, |networks| networks.len());
        let failed = failures
            .iter()
            .map(|failure| failure.network_name.clone())
            .collect();
        let lifecycle = self
            .lifecycle
            .running_with_configuration(configured, self.sessions.len(), failed)
            .await;
        self.state.set_lifecycle(lifecycle).await;
    }
}

struct SessionShutdownResult {
    network_name: String,
    result: Result<NetworkRuntimeExit, RuntimeControlError>,
    retry_session: Option<NetworkRuntimeSession>,
}

async fn wait_for_session_shutdown(
    network_name: String,
    session: NetworkRuntimeSession,
    accepted: bool,
    deadline: Instant,
) -> SessionShutdownResult {
    let NetworkRuntimeSession {
        routing_control,
        shutdown,
        mut handle,
    } = session;
    if !accepted {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return SessionShutdownResult {
                network_name: network_name.clone(),
                result: Err(RuntimeControlError::Timeout(format!(
                    "stop/timeout: 网络 [{network_name}] 的关闭通道已断开且任务未退出；清理结果未知"
                ))),
                retry_session: Some(NetworkRuntimeSession {
                    routing_control,
                    shutdown,
                    handle,
                }),
            };
        }
        return match timeout(remaining, &mut handle).await {
            Ok(Ok(outcome)) => SessionShutdownResult {
                network_name: network_name.clone(),
                result: Ok(outcome),
                retry_session: None,
            },
            Ok(Err(error)) => SessionShutdownResult {
                network_name: network_name.clone(),
                result: Err(RuntimeControlError::Unavailable(format!(
                    "stop/task_failed: 网络 [{network_name}] 的运行时任务异常退出: {error}"
                ))),
                retry_session: None,
            },
            Err(_) => SessionShutdownResult {
                network_name: network_name.clone(),
                result: Err(RuntimeControlError::Timeout(format!(
                    "stop/timeout: 网络 [{network_name}] 的关闭通道已断开且任务未退出；清理结果未知"
                ))),
                retry_session: Some(NetworkRuntimeSession {
                    routing_control,
                    shutdown,
                    handle,
                }),
            },
        };
    }

    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return SessionShutdownResult {
            network_name: network_name.clone(),
            result: Err(RuntimeControlError::Timeout(format!(
                "stop/timeout: 网络 [{network_name}] 未在期限内退出；清理结果未知，任务继续执行清理"
            ))),
            retry_session: Some(NetworkRuntimeSession {
                routing_control,
                shutdown,
                handle,
            }),
        };
    }

    match timeout(remaining, &mut handle).await {
        Ok(Ok(outcome)) => SessionShutdownResult {
            network_name: network_name.clone(),
            result: Ok(outcome),
            retry_session: None,
        },
        Ok(Err(error)) => SessionShutdownResult {
            network_name: network_name.clone(),
            result: Err(RuntimeControlError::Unavailable(format!(
                "stop/task_failed: 网络 [{network_name}] 的运行时任务异常退出，清理结果未知: {error}"
            ))),
            retry_session: None,
        },
        Err(_) => SessionShutdownResult {
            network_name: network_name.clone(),
            result: Err(RuntimeControlError::Timeout(format!(
                "stop/timeout: 网络 [{network_name}] 未在期限内退出；清理结果未知，任务继续执行清理"
            ))),
            retry_session: Some(NetworkRuntimeSession {
                routing_control,
                shutdown,
                handle,
            }),
        },
    }
}

async fn stop_embedded_service_until(
    services: &EmbeddedServiceManager,
    network_name: &str,
    deadline: Instant,
) -> Result<(), RuntimeControlError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(RuntimeControlError::Timeout(format!(
            "stop/timeout: 网络 [{network_name}] 的内嵌服务未在停止期限内退出"
        )));
    }

    match timeout(remaining, services.stop(network_name)).await {
        Ok(Ok(_)) | Ok(Err(EmbeddedServiceError::NotFound(_))) => Ok(()),
        Ok(Err(error)) => Err(map_service_error(error)),
        Err(_) => Err(RuntimeControlError::Timeout(format!(
            "stop/timeout: 网络 [{network_name}] 的内嵌服务未在停止期限内退出"
        ))),
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

fn cleanup_block_message(report: &RuntimeCleanupReport, network_name: &str) -> String {
    let mut reasons = Vec::new();
    if report
        .residual_networks
        .iter()
        .any(|name| name == network_name)
    {
        reasons.push("仍检测到 session、listener 或 peer 残留".to_string());
    }
    if let Some(outcome) = report.runtime_outcomes.get(network_name) {
        reasons.extend(outcome.issues());
    }
    if reasons.is_empty() {
        "停止后的资源清理结果不干净".into()
    } else {
        reasons.join("; ")
    }
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
        EmbeddedServiceError::StopFailed {
            network_name,
            message,
        } => RuntimeControlError::Unavailable(format!(
            "网络 [{network_name}] 的内嵌服务停止失败: {message}"
        )),
        EmbeddedServiceError::ShutdownFailed { message } => {
            RuntimeControlError::Unavailable(format!("内嵌网络服务关闭未完全成功: {message}"))
        }
        EmbeddedServiceError::PersistenceFailed {
            network_name,
            message,
        } => RuntimeControlError::Unavailable(format!(
            "网络 [{network_name}] 的持久化管理操作失败: {message}"
        )),
        EmbeddedServiceError::OperationFailed {
            network_name,
            message,
        } => RuntimeControlError::Unavailable(format!(
            "网络 [{network_name}] 的管理操作失败: {message}"
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
    use crate::p2p::connection::P2pEngine;
    use crate::stats::StatsCollector;
    use std::collections::HashSet;
    use std::net::{Ipv4Addr, Ipv6Addr};
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
        registry: Option<MultiNetworkRegistry>,
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
        launch_delay_ms: Arc<AtomicU64>,
        launch_attempts: Arc<AtomicUsize>,
        shutdown_delay_ms: Arc<AtomicU64>,
        shutdown_delays_ms: Arc<Mutex<HashMap<String, u64>>>,
        cleanup_failures: Arc<Mutex<HashSet<String>>>,
        residual_cleanup_delays_ms: Arc<Mutex<HashMap<String, u64>>>,
        lost_owners: Arc<Mutex<HashSet<String>>>,
        launched_tun_names: Arc<Mutex<Vec<(String, String)>>>,
    }

    struct FakeResidualRuntimeOwner {
        network_name: String,
        p2p: P2pEngine,
        registry: Option<MultiNetworkRegistry>,
        cleanup_failures: Arc<Mutex<HashSet<String>>>,
        cleanup_delays_ms: Arc<Mutex<HashMap<String, u64>>>,
        shutdowns: Arc<AtomicUsize>,
    }

    impl ResidualRuntimeOwner for FakeResidualRuntimeOwner {
        fn shutdown(&mut self) -> Pin<Box<dyn Future<Output = RuntimeCleanupOutcome> + Send + '_>> {
            Box::pin(async move {
                self.shutdowns.fetch_add(1, Ordering::SeqCst);
                let delay = self
                    .cleanup_delays_ms
                    .lock()
                    .expect("cleanup delays lock poisoned")
                    .get(&self.network_name)
                    .copied()
                    .unwrap_or_default();
                if delay > 0 {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
                let mut cleanup = RuntimeCleanupOutcome::default();
                let status = if self
                    .cleanup_failures
                    .lock()
                    .expect("cleanup failure lock poisoned")
                    .contains(&self.network_name)
                {
                    CleanupStatus::Failed("测试清理失败".into())
                } else {
                    CleanupStatus::Removed
                };
                cleanup.record("测试运行时", status);
                if cleanup.is_clean() {
                    if let Err(error) = self.p2p.shutdown().await {
                        cleanup.record("测试 P2P", CleanupStatus::Failed(format!("{error:#}")));
                    }
                }
                if cleanup.is_clean() {
                    if let Some(registry) = &self.registry {
                        registry.unregister(&self.network_name).await;
                    }
                }
                cleanup
            })
        }
    }

    impl FakeLauncher {
        fn with_registry(registry: MultiNetworkRegistry) -> Self {
            Self {
                registry: Some(registry),
                ..Self::default()
            }
        }

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

        fn set_launch_delay(&self, delay: Duration) {
            self.launch_delay_ms
                .store(delay.as_millis() as u64, Ordering::SeqCst);
        }

        fn launch_attempts(&self) -> usize {
            self.launch_attempts.load(Ordering::SeqCst)
        }

        fn set_shutdown_delay(&self, delay: Duration) {
            self.shutdown_delay_ms
                .store(delay.as_millis() as u64, Ordering::SeqCst);
        }

        fn set_shutdown_delay_for(&self, network_name: &str, delay: Duration) {
            self.shutdown_delays_ms
                .lock()
                .expect("shutdown delays lock poisoned")
                .insert(network_name.into(), delay.as_millis() as u64);
        }

        fn fail_cleanup(&self, network_name: &str) {
            self.cleanup_failures
                .lock()
                .expect("cleanup failure lock poisoned")
                .insert(network_name.into());
        }

        fn recover_cleanup(&self, network_name: &str) {
            self.cleanup_failures
                .lock()
                .expect("cleanup failure lock poisoned")
                .remove(network_name);
        }

        fn lose_owner(&self, network_name: &str) {
            self.lost_owners
                .lock()
                .expect("lost owner lock poisoned")
                .insert(network_name.into());
        }

        fn set_residual_cleanup_delay(&self, network_name: &str, delay: Duration) {
            self.residual_cleanup_delays_ms
                .lock()
                .expect("residual cleanup delays lock poisoned")
                .insert(network_name.into(), delay.as_millis() as u64);
        }

        fn launched_tun_names(&self) -> Vec<(String, String)> {
            self.launched_tun_names
                .lock()
                .expect("launched tun names lock poisoned")
                .clone()
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
            let launch_delay_ms = self.launch_delay_ms.clone();
            let launch_attempts = self.launch_attempts.clone();
            let shutdown_delay_ms = self.shutdown_delay_ms.clone();
            let shutdown_delays_ms = self.shutdown_delays_ms.clone();
            let cleanup_failures = self.cleanup_failures.clone();
            let residual_cleanup_delays_ms = self.residual_cleanup_delays_ms.clone();
            let lost_owners = self.lost_owners.clone();
            let launched_tun_names = self.launched_tun_names.clone();
            let registry = self.registry.clone();

            Box::pin(async move {
                launch_attempts.fetch_add(1, Ordering::SeqCst);
                let launch_delay = launch_delay_ms.load(Ordering::SeqCst);
                if launch_delay > 0 {
                    tokio::time::sleep(Duration::from_millis(launch_delay)).await;
                }
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
                launched_tun_names
                    .lock()
                    .expect("launched tun names lock poisoned")
                    .push((
                        request.network_name.clone(),
                        request.config.tun.name.clone(),
                    ));
                let network_name = request.network_name.clone();
                let routing_network_name = network_name.clone();
                let shutdown_network_name = network_name.clone();
                let config_path = PathBuf::from(request.config_path);
                let mut current_config = request.config;
                let p2p = P2pEngine::new_loopback_for_test()
                    .await
                    .expect("create loopback p2p engine");
                let p2p_listen_addr = p2p.local_addr();
                if let Some(registry) = &registry {
                    registry
                        .register(NetworkSessionHandle {
                            network_name: network_name.clone(),
                            tun_name: tun_name.clone(),
                            assigned_ip: Ipv6Addr::LOCALHOST,
                            assigned_ipv4: Some(Ipv4Addr::LOCALHOST),
                            p2p: p2p.clone(),
                            stats: StatsCollector::new(),
                        })
                        .await;
                }
                let cleanup_registry = registry.clone();
                let cleanup_p2p = p2p.clone();
                let (routing_control, mut routing_control_rx) = mpsc::unbounded_channel();
                let (shutdown, mut shutdown_rx) = watch::channel(false);
                let handle = tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            changed = shutdown_rx.changed() => {
                                if changed.is_err() || *shutdown_rx.borrow() {
                                    let shutdown_delay = shutdown_delays_ms
                                        .lock()
                                        .expect("shutdown delays lock poisoned")
                                        .get(&shutdown_network_name)
                                        .copied()
                                        .unwrap_or_else(|| shutdown_delay_ms.load(Ordering::SeqCst));
                                    if shutdown_delay > 0 {
                                        tokio::time::sleep(Duration::from_millis(shutdown_delay)).await;
                                    }
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
                    let mut owner = FakeResidualRuntimeOwner {
                        network_name: shutdown_network_name.clone(),
                        p2p: cleanup_p2p,
                        registry: cleanup_registry,
                        cleanup_failures,
                        cleanup_delays_ms: residual_cleanup_delays_ms,
                        shutdowns,
                    };
                    let cleanup = owner.shutdown().await;
                    let owner_lost = lost_owners
                        .lock()
                        .expect("lost owner lock poisoned")
                        .contains(&shutdown_network_name);
                    let retained_owner = if cleanup.is_clean() || owner_lost {
                        None
                    } else {
                        Some(Box::new(owner) as Box<dyn ResidualRuntimeOwner>)
                    };
                    NetworkRuntimeExit {
                        cleanup,
                        retained_owner,
                    }
                });

                Ok(StartedNetworkRuntime {
                    tun_name,
                    assigned_ipv6: Ipv6Addr::LOCALHOST,
                    assigned_ipv4: Some(Ipv4Addr::LOCALHOST),
                    p2p_listen_addr,
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
        registry: MultiNetworkRegistry,
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
            Self::new_with_timeouts(
                label,
                RuntimeTimeouts {
                    routing_update: routing_update_timeout,
                    ..RuntimeTimeouts::default()
                },
            )
            .await
        }

        async fn new_with_timeouts(label: &str, timeouts: RuntimeTimeouts) -> Self {
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
            let services = EmbeddedServiceManager::spawn_with_config(4, configs.clone());
            let registry = MultiNetworkRegistry::new();
            let fake = FakeLauncher::with_registry(registry.clone());
            let runtime = NetworkRuntimeManager::spawn_with_launcher_and_timeouts(
                configs.clone(),
                RuntimeOptions::default(),
                state.clone(),
                events.clone(),
                lifecycle.clone(),
                registry.clone(),
                services.clone(),
                Arc::new(fake.clone()),
                timeouts,
            );

            Self {
                configs,
                state,
                events,
                lifecycle,
                registry,
                services,
                runtime,
                fake,
                config_dir,
            }
        }

        async fn shutdown(&self) {
            let _ = self.runtime.shutdown().await;
            for session in self.registry.get_all().await {
                let _ = session.p2p.shutdown().await;
                self.registry.unregister(&session.network_name).await;
            }
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

    async fn wait_for_launch_attempts(fake: &FakeLauncher, expected: usize) {
        timeout(Duration::from_secs(2), async {
            loop {
                if fake.launch_attempts() >= expected {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("runtime launch was not attempted");
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
            .execute_test(NodeCommandRequest::new(
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
            .execute_test(NodeCommandRequest::new(
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
            .execute_test(NodeCommandRequest::new(
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
                .execute_test(NodeCommandRequest::new(
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
    async fn start_node_without_networks_reaches_running() {
        let harness = TestHarness::new("start-node-empty").await;

        let summary = harness
            .runtime
            .start_node()
            .await
            .expect("start empty node");
        assert_eq!(summary.requested, 0);
        assert!(summary.started.is_empty());
        assert!(summary.failed.is_empty());

        let lifecycle = harness.lifecycle.snapshot().await;
        assert_eq!(lifecycle.phase, NodePhase::Running);
        assert_eq!(lifecycle.configured_networks, 0);
        assert_eq!(lifecycle.active_networks, 0);
        assert!(lifecycle.failed_networks.is_empty());

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn start_node_with_partial_failure_stays_running_and_reports_failed_network() {
        let harness = TestHarness::new("start-node-partial").await;
        harness
            .configs
            .create_network("home", "ws://home.example:6666", false)
            .expect("create home");
        harness
            .configs
            .create_network("office", "ws://office.example:6666", false)
            .expect("create office");
        harness.fake.fail("office");

        let summary = harness.runtime.start_node().await.expect("start node");
        assert_eq!(summary.requested, 2);
        assert_eq!(summary.started, vec!["home"]);
        assert_eq!(summary.failed.len(), 1);
        assert_eq!(summary.failed[0].network_name, "office");

        let lifecycle = harness.lifecycle.snapshot().await;
        assert_eq!(lifecycle.phase, NodePhase::Running);
        assert_eq!(lifecycle.active_networks, 1);
        assert_eq!(lifecycle.failed_networks, vec!["office"]);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn start_node_with_only_failures_reaches_failed_with_network_context() {
        let harness = TestHarness::new("start-node-failed").await;
        harness
            .configs
            .create_network("broken", "ws://broken.example:6666", false)
            .expect("create broken");
        harness.fake.fail("broken");

        let summary = harness.runtime.start_node().await.expect("start node");
        assert_eq!(summary.requested, 1);
        assert!(summary.started.is_empty());
        assert_eq!(summary.failed.len(), 1);
        assert_eq!(summary.failed[0].network_name, "broken");

        let lifecycle = harness.lifecycle.snapshot().await;
        assert_eq!(lifecycle.phase, NodePhase::Failed);
        assert_eq!(lifecycle.configured_networks, 1);
        assert_eq!(lifecycle.active_networks, 0);
        assert_eq!(lifecycle.failed_networks, vec!["broken"]);

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
        let signaling_addr = service.signaling_addr.expect("signaling address");
        let relay_addr = service.relay_addr.expect("relay address");
        let persisted = harness
            .configs
            .list_networks()
            .expect("load persisted networks")
            .into_iter()
            .find(|network| network.name == "local")
            .expect("persisted network metadata");
        assert!(persisted.owns_embedded_services);
        assert_eq!(persisted.signaling_port, Some(signaling_addr.port()));
        assert_eq!(persisted.relay_port, Some(relay_addr.port()));
        assert_eq!(persisted.external_host, None);
        assert_eq!(persisted.ip_pool, None);
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
    async fn stop_then_start_node_restores_owned_embedded_service_endpoints() {
        let harness = TestHarness::new("node-lifecycle-owned").await;
        harness
            .runtime
            .create(create_command("owned", false))
            .await
            .expect("create detached network");

        let before = harness.services.list().await.expect("list services");
        assert_eq!(before.len(), 1);
        let persisted_before = harness
            .configs
            .list_networks()
            .expect("load networks before stop")
            .into_iter()
            .find(|network| network.name == "owned")
            .expect("owned network metadata");
        assert_eq!(
            persisted_before.signaling_port,
            Some(before[0].signaling_addr.port())
        );
        assert_eq!(
            persisted_before.relay_port,
            Some(before[0].relay_addr.port())
        );

        let stopped = harness
            .runtime
            .stop_node()
            .await
            .expect("stop node data plane");
        assert_eq!(stopped.stopped, vec!["owned"]);
        assert!(stopped.failed.is_empty());
        assert!(stopped.is_success());
        assert!(harness
            .services
            .list()
            .await
            .expect("list stopped services")
            .is_empty());

        let started = harness.runtime.start_node().await.expect("start node");
        assert_eq!(started.started, vec!["owned"]);
        let after = harness
            .services
            .list()
            .await
            .expect("list restored services");
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].signaling_addr, before[0].signaling_addr);
        assert_eq!(after[0].relay_addr, before[0].relay_addr);
        assert_eq!(after[0].ticket, before[0].ticket);

        let persisted_after = harness
            .configs
            .list_networks()
            .expect("load networks after restart")
            .into_iter()
            .find(|network| network.name == "owned")
            .expect("restored network metadata");
        assert_eq!(
            persisted_after.signaling_port,
            Some(after[0].signaling_addr.port())
        );
        assert_eq!(persisted_after.relay_port, Some(after[0].relay_addr.port()));

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

    #[tokio::test]
    async fn restart_reloads_persisted_config_before_relaunch() {
        let harness = TestHarness::new("restart-reload").await;
        harness
            .configs
            .create_network("reload", "ws://127.0.0.1:6666", true)
            .expect("create test config");
        let selection = NodeNetworkSelection::Named(vec!["reload".into()]);
        harness
            .runtime
            .start_selection(selection.clone())
            .await
            .expect("start original runtime");

        let config_path = harness
            .configs
            .get_network_config("reload")
            .expect("persisted network config");
        let mut config = ClientConfig::from_file(&config_path).expect("load config");
        let original_tun_name = config.tun.name.clone();
        config.tun.name = "restart-reloaded".into();
        config.save_to_file(&config_path).expect("rewrite config");

        let summary = harness
            .runtime
            .restart(vec!["reload".into()], false)
            .await
            .expect("restart network");
        assert_eq!(summary.requested, 1);
        assert_eq!(summary.restarted, vec!["reload"]);
        assert!(summary.failed.is_empty());
        assert!(summary.cleanup.residual_networks.is_empty());
        assert!(summary.cleanup.configuration_retained);
        assert_eq!(
            harness.fake.launched_tun_names(),
            vec![
                ("reload".into(), original_tun_name),
                ("reload".into(), "restart-reloaded".into()),
            ]
        );
        assert_eq!(harness.fake.shutdown_count(), 1);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn lifecycle_conflicts_cover_start_stop_restart_create_and_join() {
        let harness = TestHarness::new("lifecycle-conflict").await;
        harness
            .configs
            .create_network("inflight", "ws://127.0.0.1:6666", true)
            .expect("create test config");
        let selection = NodeNetworkSelection::Named(vec!["inflight".into()]);
        let plan = NodeRuntimePlan::load(&harness.configs, selection.clone()).expect("load plan");
        harness.fake.set_launch_delay(Duration::from_millis(250));
        let runtime = harness.runtime.clone();
        let start = tokio::spawn(async move { runtime.start_plan(plan).await });
        wait_for_launch_attempts(&harness.fake, 1).await;

        assert!(matches!(
            harness.runtime.stop(vec!["inflight".into()], false).await,
            Err(RuntimeControlError::Conflict(_))
        ));
        assert!(matches!(
            harness.runtime.restart(Vec::new(), true).await,
            Err(RuntimeControlError::Conflict(_))
        ));
        assert!(matches!(
            harness.runtime.start_node().await,
            Err(RuntimeControlError::Conflict(_))
        ));
        assert!(matches!(
            harness.runtime.stop_node().await,
            Err(RuntimeControlError::Conflict(_))
        ));
        assert!(matches!(
            harness.runtime.restart_node().await,
            Err(RuntimeControlError::Conflict(_))
        ));
        assert!(matches!(
            harness
                .runtime
                .create(create_command("blocked-create", false))
                .await,
            Err(RuntimeControlError::Conflict(_))
        ));
        let ticket = NetworkTicket::new([9u8; 32], "127.0.0.1:6666", "blocked-join").encode();
        assert!(matches!(
            harness
                .runtime
                .join(NodeJoinNetworkCommand {
                    target: ticket,
                    network_name: None,
                    start: false,
                })
                .await,
            Err(RuntimeControlError::Conflict(_))
        ));

        let summary = timeout(Duration::from_secs(2), start)
            .await
            .expect("start operation completed")
            .expect("start task")
            .expect("start plan");
        assert_eq!(summary.started, vec!["inflight"]);
        assert!(!harness.configs.network_exists("blocked-create"));
        assert!(!harness.configs.network_exists("blocked-join"));

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn one_network_stop_failure_does_not_block_other_networks() {
        let harness = TestHarness::new_with_timeouts(
            "stop-isolation",
            RuntimeTimeouts {
                stop: Duration::from_millis(30),
                ..RuntimeTimeouts::default()
            },
        )
        .await;
        for name in ["fast-stop", "slow-stop"] {
            harness
                .configs
                .create_network(name, "ws://127.0.0.1:6666", false)
                .expect("create test config");
        }
        let plan = NodeRuntimePlan::load(
            &harness.configs,
            NodeNetworkSelection::Named(vec!["fast-stop".into(), "slow-stop".into()]),
        )
        .expect("load test plan");
        harness
            .runtime
            .start_plan(plan)
            .await
            .expect("start networks");
        harness
            .fake
            .set_shutdown_delay_for("slow-stop", Duration::from_millis(250));

        let summary = harness
            .runtime
            .stop(Vec::new(), true)
            .await
            .expect("partial stop returns a summary");
        assert_eq!(summary.stopped, vec!["fast-stop"]);
        assert_eq!(summary.failed.len(), 1);
        assert_eq!(summary.failed[0].network_name, "slow-stop");
        assert!(summary.failed[0].message.contains("stop/timeout"));
        let snapshot = harness.state.snapshot().await;
        assert_eq!(
            snapshot
                .networks
                .iter()
                .find(|network| network.name == "slow-stop")
                .expect("failed network")
                .status,
            NodeNetworkStatus::Failed
        );

        let lifecycle = harness.lifecycle.snapshot().await;
        assert_eq!(lifecycle.phase, NodePhase::Running);
        assert_eq!(lifecycle.active_networks, 1);
        assert_eq!(lifecycle.failed_networks, vec!["slow-stop"]);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn multi_network_stop_uses_one_shared_deadline() {
        let harness = TestHarness::new_with_timeouts(
            "stop-shared-deadline",
            RuntimeTimeouts {
                stop: Duration::from_millis(50),
                ..RuntimeTimeouts::default()
            },
        )
        .await;
        let names = (0..12)
            .map(|index| format!("parallel-{index:02}"))
            .collect::<Vec<_>>();
        for name in &names {
            harness
                .configs
                .create_network(name, "ws://127.0.0.1:6666", false)
                .expect("create test config");
        }
        let plan =
            NodeRuntimePlan::load(&harness.configs, NodeNetworkSelection::Named(names.clone()))
                .expect("load test plan");
        harness
            .runtime
            .start_plan(plan)
            .await
            .expect("start networks");
        harness.fake.set_shutdown_delay(Duration::from_secs(2));

        let started = Instant::now();
        let summary = timeout(Duration::from_secs(2), harness.runtime.stop(names, false))
            .await
            .expect("parallel stop must finish")
            .expect("stop returns a summary");
        assert!(summary.stopped.is_empty());
        assert_eq!(summary.failed.len(), 12);
        assert!(
            started.elapsed() < Duration::from_millis(400),
            "network shutdowns were serialized: {:?}",
            started.elapsed()
        );
        assert_eq!(summary.cleanup.runtime_outcomes.len(), 12);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn stop_timeout_keeps_residue_until_the_runtime_reports_cleanup() {
        let harness = TestHarness::new_with_timeouts(
            "stop-timeout",
            RuntimeTimeouts {
                stop: Duration::from_millis(20),
                ..RuntimeTimeouts::default()
            },
        )
        .await;
        start_fake_network(&harness, "slow-stop").await;
        harness.fake.set_shutdown_delay(Duration::from_millis(250));

        let summary = harness
            .runtime
            .stop(vec!["slow-stop".into()], false)
            .await
            .expect("slow stop must return a partial-failure summary");
        assert!(summary.stopped.is_empty());
        assert_eq!(summary.failed.len(), 1);
        assert_eq!(summary.failed[0].network_name, "slow-stop");
        assert!(summary.failed[0].message.contains("stop/timeout"));
        assert!(summary.failed[0].message.contains("清理结果未知"));
        assert_eq!(summary.cleanup.sessions_remaining, 1);
        assert_eq!(summary.cleanup.listeners_remaining, 1);
        assert_eq!(summary.cleanup.residual_networks, vec!["slow-stop"]);
        assert_eq!(harness.registry.get_all().await.len(), 1);
        let snapshot = harness.state.snapshot().await;
        let network = snapshot
            .networks
            .iter()
            .find(|network| network.name == "slow-stop")
            .expect("failed network state");
        assert_eq!(network.status, NodeNetworkStatus::Failed);

        let restarted = harness
            .runtime
            .restart(vec!["slow-stop".into()], false)
            .await
            .expect("restart waits for the real cleanup outcome");
        assert_eq!(restarted.restarted, vec!["slow-stop"]);
        assert!(restarted.failed.is_empty());
        assert!(restarted
            .cleanup
            .runtime_outcomes
            .get("slow-stop")
            .expect("runtime cleanup outcome")
            .is_clean());
        let snapshot = harness.state.snapshot().await;
        let network = snapshot
            .networks
            .iter()
            .find(|network| network.name == "slow-stop")
            .expect("restarted network state");
        assert_eq!(network.status, NodeNetworkStatus::Online);
        assert_eq!(harness.fake.launch_attempts(), 2);

        harness.fake.set_shutdown_delay(Duration::ZERO);
        let retried = harness
            .runtime
            .stop(vec!["slow-stop".into()], false)
            .await
            .expect("stop after restart cleans the replacement runtime");
        assert_eq!(retried.stopped, vec!["slow-stop"]);
        assert!(retried.failed.is_empty());
        assert!(harness.registry.get_all().await.is_empty());
        let snapshot = harness.state.snapshot().await;
        let network = snapshot
            .networks
            .iter()
            .find(|network| network.name == "slow-stop")
            .expect("offline network state");
        assert_eq!(network.status, NodeNetworkStatus::Offline);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn cleanup_failure_keeps_residue_and_blocks_restart_before_launch() {
        let harness = TestHarness::new("cleanup-failure").await;
        start_fake_network(&harness, "broken-cleanup").await;
        harness
            .state
            .update(|snapshot| {
                if let Some(network) = snapshot
                    .networks
                    .iter_mut()
                    .find(|network| network.name == "broken-cleanup")
                {
                    network.peer_count = 1;
                }
                snapshot.peers.push(peer_snapshot("broken-cleanup"));
            })
            .await;
        harness.fake.fail_cleanup("broken-cleanup");

        let summary = harness
            .runtime
            .stop(vec!["broken-cleanup".into()], false)
            .await
            .expect("cleanup failure returns a partial-failure summary");
        assert!(summary.stopped.is_empty());
        assert_eq!(summary.failed.len(), 1);
        let outcome = summary
            .cleanup
            .runtime_outcomes
            .get("broken-cleanup")
            .expect("runtime cleanup outcome");
        assert!(!outcome.is_clean());
        assert!(outcome.summary().contains("测试清理失败"));
        assert_eq!(summary.cleanup.peers_remaining, 1);
        assert_eq!(summary.cleanup.listeners_remaining, 1);
        assert_eq!(summary.cleanup.residual_networks, vec!["broken-cleanup"]);
        assert_eq!(harness.registry.get_all().await.len(), 1);
        let snapshot = harness.state.snapshot().await;
        let network = snapshot
            .networks
            .iter()
            .find(|network| network.name == "broken-cleanup")
            .expect("failed network state");
        assert_eq!(network.status, NodeNetworkStatus::Failed);
        assert_eq!(network.peer_count, 1);
        assert!(snapshot
            .peers
            .iter()
            .any(|peer| peer.network_name == "broken-cleanup"));

        let launches_before_restart = harness.fake.launch_attempts();
        let restart = harness
            .runtime
            .restart(vec!["broken-cleanup".into()], false)
            .await
            .expect("restart returns a cleanup-phase failure");
        assert!(restart.restarted.is_empty());
        assert_eq!(restart.failed.len(), 1);
        assert_eq!(restart.failed[0].phase, RestartPhase::Cleanup);
        assert!(restart.failed[0].message.contains("测试清理失败"));
        assert_eq!(harness.fake.launch_attempts(), launches_before_restart);
        assert_eq!(restart.cleanup.residual_networks, vec!["broken-cleanup"]);
        assert_eq!(harness.registry.get_all().await.len(), 1);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn diagnostics_track_retained_owner_and_successful_cleanup_retry() {
        let harness = TestHarness::new("diagnostics-cleanup-retry").await;
        start_fake_network(&harness, "diagnostics-retry").await;
        harness.fake.fail_cleanup("diagnostics-retry");

        let stopped = harness
            .runtime
            .stop(vec!["diagnostics-retry".into()], false)
            .await
            .expect("cleanup failure returns a stop summary");
        assert_eq!(stopped.failed.len(), 1);

        let diagnostics = harness
            .runtime
            .diagnostics()
            .await
            .expect("read runtime diagnostics");
        assert_eq!(
            diagnostics.retained_owner_networks,
            vec!["diagnostics-retry"]
        );
        assert_eq!(
            diagnostics.cleanup_candidate_networks,
            vec!["diagnostics-retry"]
        );
        assert_eq!(
            diagnostics.residue.residual_networks,
            vec!["diagnostics-retry"]
        );
        assert_eq!(diagnostics.residue.cleanup_steps.len(), 1);
        assert_eq!(
            diagnostics.residue.cleanup_steps[0].status,
            NodeCleanupStepStatus::Failed
        );
        assert!(!diagnostics.residue.is_clean());

        let shutdowns_before_retry = harness.fake.shutdown_count();
        harness.fake.recover_cleanup("diagnostics-retry");
        let cleanup = harness
            .runtime
            .cleanup_residuals(vec!["diagnostics-retry".into()], false)
            .await
            .expect("retry retained runtime cleanup");
        assert_eq!(cleanup.stopped, vec!["diagnostics-retry"]);
        assert!(cleanup.failed.is_empty());
        assert!(cleanup.cleanup.is_clean_for_stop());
        assert!(harness.registry.get_all().await.is_empty());
        assert!(harness.fake.shutdown_count() > shutdowns_before_retry);

        let diagnostics = harness
            .runtime
            .diagnostics()
            .await
            .expect("read post-cleanup diagnostics");
        assert!(diagnostics.retained_owner_networks.is_empty());
        assert!(diagnostics.cleanup_candidate_networks.is_empty());
        assert!(diagnostics.residue.is_clean());
        assert_eq!(
            diagnostics.residue.cleanup_steps[0].status,
            NodeCleanupStepStatus::Removed
        );

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn cleanup_never_bypasses_a_running_session_owner() {
        let harness = TestHarness::new("cleanup-running-session").await;
        start_fake_network(&harness, "running-cleanup").await;
        let shutdowns_before = harness.fake.shutdown_count();

        let error = harness
            .runtime
            .cleanup_residuals(vec!["running-cleanup".into()], false)
            .await
            .expect_err("running sessions must not be cleaned behind their owner");
        assert!(matches!(error, RuntimeControlError::Conflict(_)));
        assert_eq!(harness.registry.get_all().await.len(), 1);
        assert_eq!(harness.fake.shutdown_count(), shutdowns_before);
        assert!(harness
            .runtime
            .diagnostics()
            .await
            .expect("read running diagnostics")
            .residue
            .residual_networks
            .contains(&"running-cleanup".into()));

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn cleanup_reports_an_unavailable_owner_without_dropping_the_candidate() {
        let harness = TestHarness::new("cleanup-owner-unavailable").await;
        start_fake_network(&harness, "lost-owner").await;
        harness.fake.fail_cleanup("lost-owner");
        harness.fake.lose_owner("lost-owner");

        harness
            .runtime
            .stop(vec!["lost-owner".into()], false)
            .await
            .expect("stop records failed cleanup");
        let diagnostics = harness
            .runtime
            .diagnostics()
            .await
            .expect("read lost-owner diagnostics");
        assert!(diagnostics.retained_owner_networks.is_empty());
        assert_eq!(diagnostics.cleanup_candidate_networks, vec!["lost-owner"]);

        let cleanup = harness
            .runtime
            .cleanup_residuals(vec!["lost-owner".into()], false)
            .await
            .expect("owner loss is a partial cleanup failure");
        assert!(cleanup.stopped.is_empty());
        assert_eq!(cleanup.failed.len(), 1);
        assert_eq!(
            cleanup.to_public_cleanup_result().failed_networks[0].reason,
            NodeCleanupFailureReason::OwnerUnavailable
        );
        assert!(harness
            .runtime
            .diagnostics()
            .await
            .expect("read remaining diagnostics")
            .cleanup_candidate_networks
            .contains(&"lost-owner".into()));

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn cleanup_timeout_retains_the_owner_and_can_be_retried() {
        let harness = TestHarness::new_with_timeouts(
            "cleanup-timeout",
            RuntimeTimeouts {
                stop: Duration::from_millis(40),
                ..RuntimeTimeouts::default()
            },
        )
        .await;
        start_fake_network(&harness, "slow-cleanup").await;
        harness.fake.fail_cleanup("slow-cleanup");
        harness
            .runtime
            .stop(vec!["slow-cleanup".into()], false)
            .await
            .expect("initial stop records failed cleanup");

        harness
            .fake
            .set_residual_cleanup_delay("slow-cleanup", Duration::from_millis(200));
        let timed_out = harness
            .runtime
            .cleanup_residuals(vec!["slow-cleanup".into()], false)
            .await
            .expect("cleanup timeout is a partial failure");
        assert!(timed_out.stopped.is_empty());
        assert_eq!(timed_out.failed.len(), 1);
        assert_eq!(
            timed_out.to_public_cleanup_result().failed_networks[0].reason,
            NodeCleanupFailureReason::TimedOut
        );
        let diagnostics = harness
            .runtime
            .diagnostics()
            .await
            .expect("read timed-out diagnostics");
        assert_eq!(diagnostics.retained_owner_networks, vec!["slow-cleanup"]);
        assert!(diagnostics
            .residue
            .cleanup_steps
            .iter()
            .any(|step| step.status == NodeCleanupStepStatus::Deferred));

        harness
            .fake
            .set_residual_cleanup_delay("slow-cleanup", Duration::ZERO);
        harness.fake.recover_cleanup("slow-cleanup");
        let retried = harness
            .runtime
            .cleanup_residuals(vec!["slow-cleanup".into()], false)
            .await
            .expect("retry cleanup after timeout");
        assert_eq!(retried.stopped, vec!["slow-cleanup"]);
        assert!(retried.failed.is_empty());

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn cleanup_all_returns_partial_success_and_retains_failed_owners() {
        let harness = TestHarness::new("cleanup-all-partial").await;
        start_fake_network(&harness, "cleanup-alpha").await;
        start_fake_network(&harness, "cleanup-beta").await;
        harness.fake.fail_cleanup("cleanup-alpha");
        harness.fake.fail_cleanup("cleanup-beta");
        harness
            .runtime
            .stop(Vec::new(), true)
            .await
            .expect("initial stop leaves two retained owners");
        harness.fake.recover_cleanup("cleanup-beta");

        let cleanup = harness
            .runtime
            .cleanup_residuals(Vec::new(), true)
            .await
            .expect("cleanup all returns partial success");
        assert_eq!(cleanup.stopped, vec!["cleanup-beta"]);
        assert_eq!(cleanup.failed.len(), 1);
        assert_eq!(cleanup.failed[0].network_name, "cleanup-alpha");
        let public = cleanup.to_public_cleanup_result();
        assert_eq!(public.cleaned_networks, vec!["cleanup-beta"]);
        assert_eq!(public.failed_networks.len(), 1);
        assert_eq!(
            public.failed_networks[0].reason,
            NodeCleanupFailureReason::Incomplete
        );

        let diagnostics = harness
            .runtime
            .diagnostics()
            .await
            .expect("read partial cleanup diagnostics");
        assert_eq!(diagnostics.retained_owner_networks, vec!["cleanup-alpha"]);
        assert_eq!(
            diagnostics.cleanup_candidate_networks,
            vec!["cleanup-alpha"]
        );
        assert_eq!(diagnostics.residue.residual_networks, vec!["cleanup-alpha"]);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn stop_node_with_cleanup_residue_reports_a_failed_lifecycle() {
        let harness = TestHarness::new("stop-node-cleanup-failure").await;
        start_fake_network(&harness, "broken-stop-node").await;
        harness.fake.fail_cleanup("broken-stop-node");

        let summary = harness
            .runtime
            .stop_node()
            .await
            .expect("cleanup failure returns a partial-failure summary");
        assert!(!summary.is_success());
        assert!(!summary.failed.is_empty());
        assert!(!summary.cleanup.residual_networks.is_empty());
        assert_eq!(harness.lifecycle.snapshot().await.phase, NodePhase::Failed);
        assert_eq!(harness.registry.get_all().await.len(), 1);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn start_timeout_records_failed_network_and_error() {
        let harness = TestHarness::new_with_timeouts(
            "start-timeout",
            RuntimeTimeouts {
                start: Duration::from_millis(20),
                ..RuntimeTimeouts::default()
            },
        )
        .await;
        harness
            .configs
            .create_network("slow-start", "ws://127.0.0.1:6666", true)
            .expect("create test config");
        harness.fake.set_launch_delay(Duration::from_millis(250));

        let summary = harness
            .runtime
            .start_selection(NodeNetworkSelection::Named(vec!["slow-start".into()]))
            .await
            .expect("start returns a per-network summary");
        assert!(summary.started.is_empty());
        assert_eq!(summary.failed.len(), 1);
        assert_eq!(summary.failed[0].network_name, "slow-start");
        assert!(summary.failed[0].message.contains("start/timeout"));

        let snapshot = harness.state.snapshot().await;
        let network = snapshot
            .networks
            .iter()
            .find(|network| network.name == "slow-start")
            .expect("failed network state");
        assert_eq!(network.status, NodeNetworkStatus::Failed);
        let problem = snapshot.last_error.as_ref().expect("start failure problem");
        assert_eq!(problem.network_name.as_deref(), Some("slow-start"));
        assert!(problem.message.contains("start/timeout"));
        assert!(harness.registry.get_all().await.is_empty());

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn restart_launch_failure_is_reported_in_start_phase() {
        let harness = TestHarness::new("restart-launch-failure").await;
        start_fake_network(&harness, "restart-failure").await;
        harness.fake.fail("restart-failure");

        let summary = harness
            .runtime
            .restart(vec!["restart-failure".into()], false)
            .await
            .expect("restart returns a per-network summary");
        assert!(summary.restarted.is_empty());
        assert_eq!(summary.failed.len(), 1);
        assert_eq!(summary.failed[0].network_name, "restart-failure");
        assert_eq!(summary.failed[0].phase, RestartPhase::Start);
        assert!(summary.failed[0].message.contains("测试启动失败"));
        assert!(summary.cleanup.residual_networks.is_empty());
        let snapshot = harness.state.snapshot().await;
        assert_eq!(
            snapshot
                .networks
                .iter()
                .find(|network| network.name == "restart-failure")
                .expect("failed network state")
                .status,
            NodeNetworkStatus::Failed
        );

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn restart_node_failure_converges_to_failed_instead_of_stopping() {
        let harness = TestHarness::new("restart-node-failed").await;
        harness
            .configs
            .create_network("restart-node", "ws://127.0.0.1:6666", false)
            .expect("create test config");
        let plan = NodeRuntimePlan::load(
            &harness.configs,
            NodeNetworkSelection::Named(vec!["restart-node".into()]),
        )
        .expect("load test plan");
        harness
            .runtime
            .start_plan(plan)
            .await
            .expect("start test network");

        // Closing the service actor forces the restart to fail after entering
        // the stopping phase, exercising lifecycle error convergence.
        harness
            .services
            .shutdown()
            .await
            .expect("close service manager");
        let error = harness
            .runtime
            .restart_node()
            .await
            .expect_err("restart must report the unavailable service manager");
        assert!(matches!(error, RuntimeControlError::ActorClosed));

        let lifecycle = harness.lifecycle.snapshot().await;
        assert_eq!(lifecycle.phase, NodePhase::Failed);
        assert_ne!(lifecycle.phase, NodePhase::Stopping);

        let _ = harness
            .runtime
            .stop(vec!["restart-node".into()], false)
            .await;
        harness.shutdown().await;
    }

    #[test]
    fn cleanup_residue_blocks_only_the_affected_network() {
        let report = RuntimeCleanupReport {
            residual_networks: vec!["alpha".into()],
            ..RuntimeCleanupReport::default()
        };
        assert!(report.blocks_restart("alpha"));
        assert!(!report.blocks_restart("beta"));
    }

    #[test]
    fn stop_success_requires_a_clean_cleanup_report() {
        let mut clean_outcome = RuntimeCleanupOutcome::default();
        clean_outcome.record("TUN 设备", CleanupStatus::Removed);
        clean_outcome.record("Exit 转发规则", CleanupStatus::NotOwned);
        let clean = StopSummary {
            stopped: vec!["alpha".into()],
            cleanup: RuntimeCleanupReport {
                runtime_outcomes: BTreeMap::from([("alpha".into(), clean_outcome)]),
                ..RuntimeCleanupReport::default()
            },
            ..StopSummary::default()
        };
        assert!(clean.is_success());

        let residue_reports = [
            RuntimeCleanupReport {
                sessions_remaining: 1,
                residual_networks: vec!["alpha".into()],
                ..RuntimeCleanupReport::default()
            },
            RuntimeCleanupReport {
                listeners_remaining: 1,
                residual_networks: vec!["alpha".into()],
                ..RuntimeCleanupReport::default()
            },
            RuntimeCleanupReport {
                peers_remaining: 1,
                residual_networks: vec!["alpha".into()],
                ..RuntimeCleanupReport::default()
            },
            RuntimeCleanupReport {
                embedded_services_remaining: 1,
                ..RuntimeCleanupReport::default()
            },
            RuntimeCleanupReport {
                residual_networks: vec!["alpha".into()],
                ..RuntimeCleanupReport::default()
            },
        ];
        for report in residue_reports {
            let summary = StopSummary {
                stopped: vec!["alpha".into()],
                cleanup: report,
                ..StopSummary::default()
            };
            assert!(!summary.is_success(), "residue must fail the stop");
        }

        let mut failed_outcome = RuntimeCleanupOutcome::default();
        failed_outcome.record("TUN 设备", CleanupStatus::Failed("删除失败".into()));
        let runtime_failure = StopSummary {
            stopped: vec!["alpha".into()],
            cleanup: RuntimeCleanupReport {
                runtime_outcomes: BTreeMap::from([("alpha".into(), failed_outcome)]),
                ..RuntimeCleanupReport::default()
            },
            ..StopSummary::default()
        };
        assert!(!runtime_failure.is_success());
    }

    #[test]
    fn stop_message_lists_every_cleanup_issue() {
        let mut outcome = RuntimeCleanupOutcome::default();
        outcome.record("TUN 设备", CleanupStatus::Failed("TUN 删除失败".into()));
        let summary = StopSummary {
            stopped: vec!["alpha".into()],
            cleanup: RuntimeCleanupReport {
                sessions_remaining: 1,
                listeners_remaining: 2,
                peers_remaining: 3,
                embedded_services_remaining: 4,
                runtime_outcomes: BTreeMap::from([("alpha".into(), outcome)]),
                residual_networks: vec!["alpha".into()],
                ..RuntimeCleanupReport::default()
            },
            ..StopSummary::default()
        };

        let message = summary.message();
        assert!(!summary.is_success());
        assert!(message.contains("残留网络: alpha"));
        assert!(message.contains("运行时任务残留 1 个"));
        assert!(message.contains("监听器残留 2 个"));
        assert!(message.contains("Peer 记录残留 3 个"));
        assert!(message.contains("内嵌服务残留 4 个"));
        assert!(message.contains("TUN 删除失败"));
    }

    #[tokio::test]
    async fn restart_retains_running_embedded_services_without_blocking() {
        let harness = TestHarness::new("restart-service").await;
        harness
            .runtime
            .create(create_command("service-network", true))
            .await
            .expect("create connected network with embedded service");
        let before = harness.services.list().await.expect("list services");
        assert_eq!(before.len(), 1);
        assert_eq!(
            harness.state.snapshot().await.embedded_services[0].status,
            NodeEmbeddedServiceStatus::Running
        );

        let summary = harness
            .runtime
            .restart(vec!["service-network".into()], false)
            .await
            .expect("restart connected network");
        assert_eq!(summary.restarted, vec!["service-network"]);
        assert!(summary.failed.is_empty());
        assert_eq!(summary.cleanup.embedded_services_remaining, 1);
        assert!(summary.cleanup.residual_networks.is_empty());
        let after = harness.services.list().await.expect("list services");
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].signaling_addr, before[0].signaling_addr);
        assert_eq!(after[0].relay_addr, before[0].relay_addr);
        assert_eq!(after[0].ticket, before[0].ticket);
        assert_eq!(
            harness.state.snapshot().await.embedded_services[0].status,
            NodeEmbeddedServiceStatus::Running
        );
        assert_eq!(harness.fake.shutdown_count(), 1);
        assert_eq!(harness.fake.launches().len(), 2);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn cancelling_start_caller_keeps_operation_gate_until_actor_finishes() {
        let harness = TestHarness::new_with_timeouts(
            "start-cancellation",
            RuntimeTimeouts {
                start: Duration::from_secs(1),
                ..RuntimeTimeouts::default()
            },
        )
        .await;
        harness
            .configs
            .create_network("cancelled", "ws://127.0.0.1:6666", true)
            .expect("create test config");
        let selection = NodeNetworkSelection::Named(vec!["cancelled".into()]);
        let plan = NodeRuntimePlan::load(&harness.configs, selection.clone()).expect("load plan");
        harness.fake.set_launch_delay(Duration::from_millis(200));
        let runtime = harness.runtime.clone();
        let caller = tokio::spawn(async move { runtime.start_plan(plan).await });
        wait_for_launch_attempts(&harness.fake, 1).await;
        caller.abort();
        let _ = caller.await;

        assert!(matches!(
            harness.runtime.start_selection(selection.clone()).await,
            Err(RuntimeControlError::Conflict(_))
        ));
        timeout(Duration::from_secs(2), async {
            loop {
                match harness.runtime.start_selection(selection.clone()).await {
                    Ok(summary) => {
                        assert_eq!(summary.already_running, vec!["cancelled"]);
                        assert!(summary.started.is_empty());
                        break;
                    }
                    Err(RuntimeControlError::Conflict(_)) => {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Err(error) => panic!("unexpected start error: {error}"),
                }
            }
        })
        .await
        .expect("actor did not release the cancelled start operation");

        harness.shutdown().await;
    }
}
