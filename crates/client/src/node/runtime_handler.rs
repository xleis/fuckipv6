use std::sync::Arc;
use std::time::Duration;

use super::{NetworkRuntimeManager, RuntimeControlError, StartSummary};
use super::{
    NodeCommandFuture, NodeCommandHandler, NodeCommandOutcome, NodeCommandRequest,
    NodeCommandResult, NodePeerProbeSnapshot, NodeProblem, NodeProblemCode, NodeStateStore,
};
use crate::ipc::MultiNetworkRegistry;
use crate::symmetric::EmbeddedServiceManager;

/// Production handler for operations backed by the live node runtime.
///
/// Every UI adapter submits the same commands through this handler. Commands
/// whose runtime backend is not wired yet are rejected with a stable error
/// instead of reporting a false success.
pub struct NodeRuntimeHandler {
    state: NodeStateStore,
    registry: MultiNetworkRegistry,
    services: EmbeddedServiceManager,
    runtime: Option<NetworkRuntimeManager>,
}

impl NodeRuntimeHandler {
    pub fn new(state: NodeStateStore, registry: MultiNetworkRegistry) -> Self {
        Self::with_services(state, registry, EmbeddedServiceManager::spawn(8))
    }

    fn with_services(
        state: NodeStateStore,
        registry: MultiNetworkRegistry,
        services: EmbeddedServiceManager,
    ) -> Self {
        Self {
            state,
            registry,
            services,
            runtime: None,
        }
    }

    fn with_runtime(
        state: NodeStateStore,
        registry: MultiNetworkRegistry,
        services: EmbeddedServiceManager,
        runtime: NetworkRuntimeManager,
    ) -> Self {
        Self {
            state,
            registry,
            services,
            runtime: Some(runtime),
        }
    }
}

impl NodeCommandHandler for NodeRuntimeHandler {
    fn handle(&self, request: NodeCommandRequest) -> NodeCommandFuture {
        let state = self.state.clone();
        let registry = self.registry.clone();
        let services = self.services.clone();
        let runtime = self.runtime.clone();

        Box::pin(async move {
            let request_id = request.request_id;
            if let Err(problem) = request.validate() {
                return NodeCommandResult::rejected(request_id, problem);
            }

            match request.command {
                super::NodeCommand::GetSnapshot => {
                    NodeCommandResult::snapshot(request_id, state.snapshot().await)
                }
                super::NodeCommand::GetUiManifest => {
                    NodeCommandResult::ui_manifest(request_id, super::node_ui_manifest())
                }
                super::NodeCommand::StopNetworks { networks, all } => match runtime {
                    Some(runtime) => {
                        stop_runtime_networks(&runtime, request_id, networks, all).await
                    }
                    None => stop_networks(&registry, request_id, &networks, all).await,
                },
                super::NodeCommand::ProbePeer(command) => {
                    probe_peer(&registry, request_id, command).await
                }
                super::NodeCommand::Shutdown => {
                    let result = stop_all(&registry, request_id).await;
                    if !result.is_success() {
                        return result;
                    }
                    // The supervisor owns the runtime manager and will stop
                    // all sessions and embedded services after receiving the
                    // shutdown signal. Do not close the shared service actor
                    // from this request first.
                    if runtime.is_some() {
                        return result;
                    }
                    match services.shutdown().await {
                        Ok(_) => result,
                        Err(error) => unavailable(
                            request_id,
                            format!("网络停止指令已下发，但释放内嵌网络服务失败: {error}"),
                        ),
                    }
                }
                super::NodeCommand::StartNetworks { selection } => match runtime {
                    Some(runtime) => start_runtime_networks(&runtime, request_id, selection).await,
                    None => unavailable(
                        request_id,
                        "动态启动网络尚未接入监督器；请先完成运行时 actor 化",
                    ),
                },
                super::NodeCommand::CreateNetwork(command) => match runtime {
                    Some(runtime) => create_runtime_network(&runtime, request_id, command).await,
                    None => unavailable(request_id, "创建网络尚未接入内嵌网络服务 actor"),
                },
                super::NodeCommand::JoinNetwork(command) => match runtime {
                    Some(runtime) => join_runtime_network(&runtime, request_id, command).await,
                    None => unavailable(request_id, "加入网络尚未接入内嵌网络服务 actor"),
                },
                super::NodeCommand::UpdateRouting(_) => {
                    unavailable(request_id, "路由热更新尚未接入多网络监督器控制通道")
                }
                super::NodeCommand::SetNetworkRole(_) => {
                    unavailable(request_id, "网络角色变更尚未接入持久化与运行时重配置")
                }
            }
        })
    }
}

async fn start_runtime_networks(
    runtime: &NetworkRuntimeManager,
    request_id: u64,
    selection: super::NodeNetworkSelection,
) -> NodeCommandResult {
    match runtime.start_selection(selection).await {
        Ok(summary) if summary.requested == 0 => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::Completed {
                message: "没有需要启动的网络".into(),
            },
        ),
        Ok(summary) if summary.started.is_empty() && !summary.failed.is_empty() => {
            NodeCommandResult::rejected(
                request_id,
                NodeProblem::new(
                    NodeProblemCode::Unavailable,
                    summary.message("启动网络失败"),
                )
                .retryable(true),
            )
        }
        Ok(summary) => completed_summary(request_id, summary, "启动网络"),
        Err(error) => runtime_rejection(request_id, error),
    }
}

async fn create_runtime_network(
    runtime: &NetworkRuntimeManager,
    request_id: u64,
    command: super::NodeCreateNetworkCommand,
) -> NodeCommandResult {
    match runtime.create(command).await {
        Ok(result) => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::Completed {
                message: format!(
                    "网络 [{}] 已创建并保存；{}；邀请票据: {}",
                    result.network_name,
                    if result.started {
                        "本地运行时已启动"
                    } else {
                        "本地运行时未自动启动"
                    },
                    result.ticket
                ),
            },
        ),
        Err(error) => runtime_rejection(request_id, error),
    }
}

async fn join_runtime_network(
    runtime: &NetworkRuntimeManager,
    request_id: u64,
    command: super::NodeJoinNetworkCommand,
) -> NodeCommandResult {
    match runtime.join(command).await {
        Ok(result) => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::Completed {
                message: format!(
                    "网络 [{}] 配置{}；{}；协调端点: {}",
                    result.network_name,
                    if result.config_saved {
                        "已保存"
                    } else {
                        "已存在"
                    },
                    if result.started {
                        "运行时已启动"
                    } else {
                        "按请求未自动启动"
                    },
                    result.coordinator_url
                ),
            },
        ),
        Err(error) => runtime_rejection(request_id, error),
    }
}

async fn stop_runtime_networks(
    runtime: &NetworkRuntimeManager,
    request_id: u64,
    networks: Vec<String>,
    all: bool,
) -> NodeCommandResult {
    match runtime.stop(networks, all).await {
        Ok(summary) => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::Completed {
                message: if summary.stopped.is_empty() {
                    "没有正在运行或托管服务的网络".into()
                } else {
                    format!("已停止网络: {}", summary.stopped.join(", "))
                },
            },
        ),
        Err(error) => runtime_rejection(request_id, error),
    }
}

fn completed_summary(request_id: u64, summary: StartSummary, action: &str) -> NodeCommandResult {
    NodeCommandResult::completed(
        request_id,
        NodeCommandOutcome::Completed {
            message: summary.message(action),
        },
    )
}

fn runtime_rejection(request_id: u64, error: RuntimeControlError) -> NodeCommandResult {
    let code = match &error {
        RuntimeControlError::Invalid(_) => NodeProblemCode::InvalidRequest,
        RuntimeControlError::Conflict(_) => NodeProblemCode::Conflict,
        RuntimeControlError::NotFound(_) => NodeProblemCode::NotFound,
        RuntimeControlError::ActorClosed
        | RuntimeControlError::Persistence(_)
        | RuntimeControlError::Launch(_) => NodeProblemCode::Unavailable,
    };
    NodeCommandResult::rejected(
        request_id,
        NodeProblem::new(code, error.to_string())
            .retryable(matches!(code, NodeProblemCode::Unavailable)),
    )
}

async fn stop_networks(
    registry: &MultiNetworkRegistry,
    request_id: u64,
    networks: &[String],
    all: bool,
) -> NodeCommandResult {
    if all {
        return stop_all(registry, request_id).await;
    }

    let sessions = registry.get_all().await;
    let live_names: std::collections::HashSet<_> = sessions
        .iter()
        .map(|session| session.network_name.as_str())
        .collect();
    let missing: Vec<_> = networks
        .iter()
        .filter(|name| !live_names.contains(name.as_str()))
        .cloned()
        .collect();
    if !missing.is_empty() {
        let message = format!("网络未运行: {}", missing.join(", "));
        return NodeCommandResult::rejected(
            request_id,
            NodeProblem::new(NodeProblemCode::NotFound, message),
        );
    }

    for name in networks {
        if !registry.send_stop(Some(name.clone())) {
            return unavailable(request_id, "节点停止通道未就绪，未能下发停止指令");
        }
    }

    NodeCommandResult::completed(
        request_id,
        NodeCommandOutcome::Completed {
            message: format!("已下发停止 {} 个网络指令", networks.len()),
        },
    )
}

async fn stop_all(registry: &MultiNetworkRegistry, request_id: u64) -> NodeCommandResult {
    if !registry.send_stop(None) {
        return unavailable(request_id, "节点停止通道未就绪，未能下发停止指令");
    }

    NodeCommandResult::completed(
        request_id,
        NodeCommandOutcome::Completed {
            message: "已下发停止全部网络指令".into(),
        },
    )
}

async fn probe_peer(
    registry: &MultiNetworkRegistry,
    request_id: u64,
    command: super::NodeProbePeerCommand,
) -> NodeCommandResult {
    let sessions = registry.get_all().await;
    let Some(session) = sessions
        .iter()
        .find(|session| session.network_name == command.network_name)
    else {
        return NodeCommandResult::rejected(
            request_id,
            NodeProblem::new(
                NodeProblemCode::NotFound,
                format!("网络未运行: {}", command.network_name),
            )
            .for_network(command.network_name),
        );
    };

    let timeout_ms = command.timeout_ms.clamp(100, 5_000);
    let Some(outcome) = session
        .p2p
        .probe_peer_now(&command.target, Duration::from_millis(timeout_ms))
        .await
    else {
        return NodeCommandResult::rejected(
            request_id,
            NodeProblem::new(
                NodeProblemCode::NotFound,
                format!("未找到在线 Peer: {}", command.target),
            )
            .for_network(command.network_name),
        );
    };

    NodeCommandResult::completed(
        request_id,
        NodeCommandOutcome::Probe(NodePeerProbeSnapshot {
            peer_id: outcome.client_id,
            virtual_ipv6: outcome.virtual_ip,
            endpoint: outcome.endpoint,
            rtt_us: outcome.rtt_us,
            p2p_active: outcome.p2p_active,
            timed_out: outcome.timed_out,
        }),
    )
}

fn unavailable(request_id: u64, message: impl Into<String>) -> NodeCommandResult {
    NodeCommandResult::rejected(
        request_id,
        NodeProblem::new(NodeProblemCode::Unavailable, message),
    )
}

/// Shared command bus backed by a live node runtime.
pub fn spawn_command_bus(
    state: NodeStateStore,
    registry: MultiNetworkRegistry,
    events: super::NodeEventBus,
) -> super::NodeCommandBus {
    let handler = NodeRuntimeHandler::new(state, registry);
    super::NodeCommandBus::spawn(Arc::new(handler), events, 32)
}

/// Spawn the command bus while sharing embedded service ownership with the
/// node supervisor. This lets UI commands and the supervisor act on the same
/// signaling/relay lifecycle instead of creating competing service owners.
pub fn spawn_command_bus_with_services(
    state: NodeStateStore,
    registry: MultiNetworkRegistry,
    events: super::NodeEventBus,
    services: EmbeddedServiceManager,
) -> super::NodeCommandBus {
    let handler = NodeRuntimeHandler::with_services(state, registry, services);
    super::NodeCommandBus::spawn(Arc::new(handler), events, 32)
}

pub(crate) fn spawn_command_bus_with_runtime(
    state: NodeStateStore,
    registry: MultiNetworkRegistry,
    events: super::NodeEventBus,
    services: EmbeddedServiceManager,
    runtime: NetworkRuntimeManager,
) -> super::NodeCommandBus {
    let handler = NodeRuntimeHandler::with_runtime(state, registry, services, runtime);
    super::NodeCommandBus::spawn(Arc::new(handler), events, 32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{NodeCommand, NodeCommandBus, NodeEventBus};
    use crate::symmetric::EmbeddedServiceStartRequest;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn get_snapshot_reads_the_shared_store() {
        let state = NodeStateStore::new();
        state
            .update(|snapshot| {
                snapshot.logs.push(crate::node::NodeLogEntry {
                    unix_ms: 42,
                    level: crate::node::NodeLogLevel::Info,
                    target: "test".into(),
                    message: "shared".into(),
                })
            })
            .await;
        let registry = MultiNetworkRegistry::new();
        let bus = spawn_command_bus(state, registry, NodeEventBus::default());

        let result = bus
            .execute(NodeCommandRequest::new(1, NodeCommand::GetSnapshot))
            .await
            .expect("snapshot result");

        match result.outcome {
            NodeCommandOutcome::Snapshot(snapshot) => {
                assert_eq!(snapshot.revision, 1);
                assert_eq!(snapshot.logs[0].message, "shared");
            }
            outcome => panic!("unexpected outcome: {outcome:?}"),
        }
    }

    #[tokio::test]
    async fn get_ui_manifest_round_trips_and_matches_the_shared_contract() {
        let bus = spawn_command_bus(
            NodeStateStore::new(),
            MultiNetworkRegistry::new(),
            NodeEventBus::default(),
        );
        let request = NodeCommandRequest::new(21, NodeCommand::GetUiManifest);
        let encoded = serde_json::to_string(&request).expect("serialize manifest request");
        let decoded: NodeCommandRequest =
            serde_json::from_str(&encoded).expect("deserialize manifest request");
        assert_eq!(decoded, request);

        let result = bus.execute(decoded).await.expect("manifest result");
        match result.outcome {
            NodeCommandOutcome::UiManifest(manifest) => {
                assert_eq!(
                    manifest.schema_version,
                    crate::node::NODE_UI_MANIFEST_SCHEMA_VERSION
                );
                assert_eq!(manifest.pages.len(), 11);
                assert_eq!(*manifest, crate::node::node_ui_manifest());
            }
            outcome => panic!("unexpected outcome: {outcome:?}"),
        }
    }

    #[tokio::test]
    async fn shutdown_uses_the_live_stop_channel() {
        let registry = MultiNetworkRegistry::new();
        let (stop_tx, mut stop_rx) = mpsc::unbounded_channel();
        registry.set_stop_sender(stop_tx);
        let bus = spawn_command_bus(NodeStateStore::new(), registry, NodeEventBus::default());

        let result = bus
            .execute(NodeCommandRequest::new(2, NodeCommand::Shutdown))
            .await
            .expect("shutdown result");

        assert!(result.is_success());
        assert_eq!(stop_rx.recv().await, Some(None));
    }

    #[tokio::test]
    async fn shutdown_releases_shared_embedded_services() {
        let registry = MultiNetworkRegistry::new();
        let (stop_tx, mut stop_rx) = mpsc::unbounded_channel();
        registry.set_stop_sender(stop_tx);
        let services = EmbeddedServiceManager::spawn(2);
        services
            .start(EmbeddedServiceStartRequest::new("lab", 0, 0))
            .await
            .expect("start embedded service");
        let bus = spawn_command_bus_with_services(
            NodeStateStore::new(),
            registry,
            NodeEventBus::default(),
            services.clone(),
        );

        let result = bus
            .execute(NodeCommandRequest::new(20, NodeCommand::Shutdown))
            .await
            .expect("shutdown result");

        assert!(result.is_success());
        assert_eq!(stop_rx.recv().await, Some(None));
        assert!(services.list().await.is_err());
    }

    #[tokio::test]
    async fn stop_named_network_rejects_when_it_is_not_running() {
        let bus = spawn_command_bus(
            NodeStateStore::new(),
            MultiNetworkRegistry::new(),
            NodeEventBus::default(),
        );

        let result = bus
            .execute(NodeCommandRequest::new(
                3,
                NodeCommand::StopNetworks {
                    networks: vec!["home".into()],
                    all: false,
                },
            ))
            .await
            .expect("stop result");

        match result.outcome {
            NodeCommandOutcome::Rejected(problem) => {
                assert_eq!(problem.code, NodeProblemCode::NotFound);
            }
            outcome => panic!("unexpected outcome: {outcome:?}"),
        }
    }

    #[tokio::test]
    async fn unsupported_dynamic_start_fails_explicitly() {
        let bus = spawn_command_bus(
            NodeStateStore::new(),
            MultiNetworkRegistry::new(),
            NodeEventBus::default(),
        );

        let result = bus
            .execute(NodeCommandRequest::new(
                4,
                NodeCommand::StartNetworks {
                    selection: crate::node::NodeNetworkSelection::All,
                },
            ))
            .await
            .expect("start result");

        match result.outcome {
            NodeCommandOutcome::Rejected(problem) => {
                assert_eq!(problem.code, NodeProblemCode::Unavailable);
                assert!(problem.message.contains("监督器"));
            }
            outcome => panic!("unexpected outcome: {outcome:?}"),
        }
    }

    #[tokio::test]
    async fn command_bus_type_is_usable_without_output_adapters() {
        let _: NodeCommandBus = spawn_command_bus(
            NodeStateStore::new(),
            MultiNetworkRegistry::new(),
            NodeEventBus::default(),
        );
    }
}
