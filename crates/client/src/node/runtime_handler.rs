use std::sync::Arc;
use std::time::Duration;

use super::{
    NodeCommandFuture, NodeCommandHandler, NodeCommandOutcome, NodeCommandRequest,
    NodeCommandResult, NodePeerProbeSnapshot, NodeProblem, NodeProblemCode, NodeStateStore,
};
use crate::ipc::MultiNetworkRegistry;

/// Production handler for operations backed by the live node runtime.
///
/// Every UI adapter submits the same commands through this handler. Commands
/// whose runtime backend is not wired yet are rejected with a stable error
/// instead of reporting a false success.
pub struct NodeRuntimeHandler {
    state: NodeStateStore,
    registry: MultiNetworkRegistry,
}

impl NodeRuntimeHandler {
    pub fn new(state: NodeStateStore, registry: MultiNetworkRegistry) -> Self {
        Self { state, registry }
    }
}

impl NodeCommandHandler for NodeRuntimeHandler {
    fn handle(&self, request: NodeCommandRequest) -> NodeCommandFuture {
        let state = self.state.clone();
        let registry = self.registry.clone();

        Box::pin(async move {
            let request_id = request.request_id;
            if let Err(problem) = request.validate() {
                return NodeCommandResult::rejected(request_id, problem);
            }

            match request.command {
                super::NodeCommand::GetSnapshot => {
                    NodeCommandResult::snapshot(request_id, state.snapshot().await)
                }
                super::NodeCommand::StopNetworks { networks, all } => {
                    stop_networks(&registry, request_id, &networks, all).await
                }
                super::NodeCommand::ProbePeer(command) => {
                    probe_peer(&registry, request_id, command).await
                }
                super::NodeCommand::Shutdown => stop_all(&registry, request_id).await,
                super::NodeCommand::StartNetworks { .. } => unavailable(
                    request_id,
                    "动态启动网络尚未接入监督器；请先完成运行时 actor 化",
                ),
                super::NodeCommand::CreateNetwork(_) => {
                    unavailable(request_id, "创建网络尚未接入内嵌网络服务 actor")
                }
                super::NodeCommand::JoinNetwork(_) => {
                    unavailable(request_id, "加入网络尚未接入内嵌网络服务 actor")
                }
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
    super::NodeCommandBus::spawn(
        Arc::new(NodeRuntimeHandler::new(state, registry)),
        events,
        32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{NodeCommand, NodeCommandBus, NodeEventBus};
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
