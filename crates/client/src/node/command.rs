use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

use super::{
    NodeEventBus, NodeEventKind, NodeLifecycleSnapshot, NodeNetworkRole, NodeNetworkSelection,
    NodeProblem, NodeProblemCode, NodeRoutingMode, NodeSnapshot, NodeUiManifest,
};

/// One request accepted by the node command layer from CLI, IPC, WebUI or GPUI.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeCommandRequest {
    pub request_id: u64,
    pub command: NodeCommand,
}

impl NodeCommandRequest {
    pub fn new(request_id: u64, command: NodeCommand) -> Self {
        Self {
            request_id,
            command,
        }
    }

    pub fn validate(&self) -> Result<(), NodeProblem> {
        self.command.validate()
    }
}

/// Canonical command identity shared by the wire protocol, UI manifest, schema
/// generation and adapters. Payload-bearing variants remain in `NodeCommand`;
/// this enum is the single command-name source for every generated artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeCommandKind {
    GetSnapshot,
    GetUiManifest,
    #[serde(rename = "get_node_diagnostics")]
    GetDiagnostics,
    StartNetworks,
    StopNetworks,
    RestartNetworks,
    CreateNetwork,
    JoinNetwork,
    UpdateRouting,
    SetNetworkRole,
    ProbePeer,
    StartNode,
    StopNode,
    RestartNode,
    #[serde(rename = "cleanup_node_residuals")]
    CleanupResiduals,
    Shutdown,
}

impl NodeCommandKind {
    pub const ALL: [Self; 16] = [
        Self::GetSnapshot,
        Self::GetUiManifest,
        Self::GetDiagnostics,
        Self::StartNetworks,
        Self::StopNetworks,
        Self::RestartNetworks,
        Self::CreateNetwork,
        Self::JoinNetwork,
        Self::UpdateRouting,
        Self::SetNetworkRole,
        Self::ProbePeer,
        Self::StartNode,
        Self::StopNode,
        Self::RestartNode,
        Self::CleanupResiduals,
        Self::Shutdown,
    ];

    /// Commands that must be reachable through a user-visible manifest action.
    /// The manifest bootstrap command is deliberately excluded.
    pub const ACTIONS: [Self; 15] = [
        Self::GetSnapshot,
        Self::GetDiagnostics,
        Self::StartNetworks,
        Self::StopNetworks,
        Self::RestartNetworks,
        Self::CreateNetwork,
        Self::JoinNetwork,
        Self::UpdateRouting,
        Self::SetNetworkRole,
        Self::ProbePeer,
        Self::StartNode,
        Self::StopNode,
        Self::RestartNode,
        Self::CleanupResiduals,
        Self::Shutdown,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GetSnapshot => "get_snapshot",
            Self::GetUiManifest => "get_ui_manifest",
            Self::GetDiagnostics => "get_node_diagnostics",
            Self::StartNetworks => "start_networks",
            Self::StopNetworks => "stop_networks",
            Self::RestartNetworks => "restart_networks",
            Self::CreateNetwork => "create_network",
            Self::JoinNetwork => "join_network",
            Self::UpdateRouting => "update_routing",
            Self::SetNetworkRole => "set_network_role",
            Self::ProbePeer => "probe_peer",
            Self::StartNode => "start_node",
            Self::StopNode => "stop_node",
            Self::RestartNode => "restart_node",
            Self::CleanupResiduals => "cleanup_node_residuals",
            Self::Shutdown => "shutdown",
        }
    }
}

/// Product-neutral node operations. A node creates, joins or serves a network;
/// there are deliberately no product-level client/server command variants.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "command", content = "payload", rename_all = "snake_case")]
pub enum NodeCommand {
    GetSnapshot,
    /// Bootstrap query for the declarative UI contract. This is not rendered as
    /// a page action and should be called before an adapter builds its UI.
    GetUiManifest,
    /// Query live runtime residue and cleanup state from the actor that owns
    /// network sessions, embedded services and retained resource owners.
    #[serde(rename = "get_node_diagnostics")]
    GetDiagnostics,
    StartNetworks {
        selection: NodeNetworkSelection,
    },
    StopNetworks {
        networks: Vec<String>,
        all: bool,
    },
    RestartNetworks {
        networks: Vec<String>,
        all: bool,
    },
    CreateNetwork(NodeCreateNetworkCommand),
    JoinNetwork(NodeJoinNetworkCommand),
    UpdateRouting(NodeUpdateRoutingCommand),
    SetNetworkRole(NodeSetNetworkRoleCommand),
    ProbePeer(NodeProbePeerCommand),
    /// Start all persistently configured networks while keeping this node IPC
    /// and the control/UI processes alive.
    StartNode,
    /// Stop all data-plane sessions and owned embedded services while keeping
    /// this node IPC and the control/UI processes alive.
    StopNode,
    /// Reload every persisted network and replace the complete data plane under
    /// one single-flight lifecycle operation.
    RestartNode,
    /// Retry cleanup for runtime resources whose previous owner reported a
    /// deferred or failed shutdown.
    #[serde(rename = "cleanup_node_residuals")]
    CleanupResiduals {
        networks: Vec<String>,
        all: bool,
    },
    Shutdown,
}

impl NodeCommand {
    pub const fn kind(&self) -> NodeCommandKind {
        match self {
            Self::GetSnapshot => NodeCommandKind::GetSnapshot,
            Self::GetUiManifest => NodeCommandKind::GetUiManifest,
            Self::GetDiagnostics => NodeCommandKind::GetDiagnostics,
            Self::StartNetworks { .. } => NodeCommandKind::StartNetworks,
            Self::StopNetworks { .. } => NodeCommandKind::StopNetworks,
            Self::RestartNetworks { .. } => NodeCommandKind::RestartNetworks,
            Self::CreateNetwork(_) => NodeCommandKind::CreateNetwork,
            Self::JoinNetwork(_) => NodeCommandKind::JoinNetwork,
            Self::UpdateRouting(_) => NodeCommandKind::UpdateRouting,
            Self::SetNetworkRole(_) => NodeCommandKind::SetNetworkRole,
            Self::ProbePeer(_) => NodeCommandKind::ProbePeer,
            Self::StartNode => NodeCommandKind::StartNode,
            Self::StopNode => NodeCommandKind::StopNode,
            Self::RestartNode => NodeCommandKind::RestartNode,
            Self::CleanupResiduals { .. } => NodeCommandKind::CleanupResiduals,
            Self::Shutdown => NodeCommandKind::Shutdown,
        }
    }

    pub fn validate(&self) -> Result<(), NodeProblem> {
        match self {
            Self::GetSnapshot
            | Self::GetUiManifest
            | Self::GetDiagnostics
            | Self::StartNode
            | Self::StopNode
            | Self::RestartNode
            | Self::Shutdown => Ok(()),
            Self::StartNetworks { selection } => validate_selection(selection),
            Self::StopNetworks { networks, all } => {
                if *all {
                    if !networks.is_empty() {
                        return Err(invalid(
                            "停止全部网络时不能同时指定网络名称，请只设置 all=true",
                        ));
                    }
                    return Ok(());
                }
                if networks.is_empty() {
                    return Err(invalid("停止网络时必须指定网络名称，或设置 all=true"));
                }
                validate_network_names(networks)
            }
            Self::RestartNetworks { networks, all } => {
                if *all {
                    if !networks.is_empty() {
                        return Err(invalid(
                            "重启全部网络时不能同时指定网络名称，请只设置 all=true",
                        ));
                    }
                    return Ok(());
                }
                if networks.is_empty() {
                    return Err(invalid("重启网络时必须指定网络名称，或设置 all=true"));
                }
                validate_network_names(networks)
            }
            Self::CleanupResiduals { networks, all } => {
                if *all {
                    if !networks.is_empty() {
                        return Err(invalid(
                            "清理全部网络残留时不能同时指定网络名称，请只设置 all=true",
                        ));
                    }
                    return Ok(());
                }
                if networks.is_empty() {
                    return Err(invalid("清理网络残留时必须指定网络名称，或设置 all=true"));
                }
                validate_network_names(networks)
            }
            Self::CreateNetwork(command) => command.validate(),
            Self::JoinNetwork(command) => command.validate(),
            Self::UpdateRouting(command) => command.validate(),
            Self::SetNetworkRole(command) => command.validate(),
            Self::ProbePeer(command) => command.validate(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeCreateNetworkCommand {
    pub name: String,
    pub signaling_port: u16,
    pub relay_port: u16,
    pub external_host: Option<String>,
    pub auto_connect: bool,
}

impl NodeCreateNetworkCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        validate_network_name(&self.name)?;
        if self.signaling_port == 0 {
            return Err(invalid("信令端口不能为 0"));
        }
        if self.relay_port == 0 {
            return Err(invalid("中继端口不能为 0"));
        }
        if self.signaling_port == self.relay_port {
            return Err(invalid("信令端口和中继端口不能相同"));
        }
        if self
            .external_host
            .as_deref()
            .is_some_and(|host| host.trim().is_empty())
        {
            return Err(invalid("外部主机名不能是空字符串"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeJoinNetworkCommand {
    pub target: String,
    pub network_name: Option<String>,
    pub start: bool,
}

impl NodeJoinNetworkCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        if self.target.trim().is_empty() {
            return Err(invalid("加入网络的目标地址或票据不能为空"));
        }
        if let Some(name) = self.network_name.as_deref() {
            validate_network_name(name)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeUpdateRoutingCommand {
    pub network_name: String,
    pub mode: NodeRoutingMode,
    pub exit_node: Option<String>,
    pub is_exit_node: bool,
}

impl NodeUpdateRoutingCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        validate_network_name(&self.network_name)?;
        if self
            .exit_node
            .as_deref()
            .is_some_and(|exit| exit.trim().is_empty())
        {
            return Err(invalid(
                "出口节点不能是空字符串；不设置出口节点时应省略该字段",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeSetNetworkRoleCommand {
    pub network_name: String,
    pub role: NodeNetworkRole,
    pub enabled: bool,
}

impl NodeSetNetworkRoleCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        validate_network_name(&self.network_name)?;
        if self.role == NodeNetworkRole::Member && !self.enabled {
            return Err(invalid("成员角色是节点网络内的基线能力，不能禁用")
                .for_network(self.network_name.clone()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeProbePeerCommand {
    pub network_name: String,
    pub target: String,
    pub timeout_ms: u64,
}

impl NodeProbePeerCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        validate_network_name(&self.network_name)?;
        if self.target.trim().is_empty() {
            return Err(invalid("探测目标不能为空"));
        }
        if self.timeout_ms == 0 {
            return Err(invalid("探测超时时间必须大于 0"));
        }
        Ok(())
    }
}

fn validate_selection(selection: &NodeNetworkSelection) -> Result<(), NodeProblem> {
    match selection {
        NodeNetworkSelection::None | NodeNetworkSelection::Default | NodeNetworkSelection::All => {
            Ok(())
        }
        NodeNetworkSelection::Named(names) => {
            if names.is_empty() {
                return Err(invalid("至少需要指定一个网络名称"));
            }
            validate_network_names(names)
        }
    }
}

fn validate_network_names(names: &[String]) -> Result<(), NodeProblem> {
    let mut seen = HashSet::new();
    for name in names {
        validate_network_name(name)?;
        if !seen.insert(name.as_str()) {
            return Err(invalid(format!("网络重复: {name}")));
        }
    }
    Ok(())
}

/// Shared network-name validation used before any adapter invokes a command.
pub fn validate_network_name(name: &str) -> Result<(), NodeProblem> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(invalid("网络名称不能为空"));
    }
    if trimmed != name {
        return Err(invalid("网络名称首尾不能包含空白字符"));
    }
    if name == "." || name == ".." {
        return Err(invalid("网络名称不能为 '.' 或 '..'"));
    }
    if name.chars().any(|character| {
        !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
    }) {
        return Err(invalid(
            "网络名称只能包含 ASCII 字母、数字、'.'、'-' 和 '_'",
        ));
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> NodeProblem {
    NodeProblem::new(NodeProblemCode::InvalidRequest, message)
}

#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct NodeCommandResult {
    pub request_id: u64,
    pub outcome: NodeCommandOutcome,
}

impl NodeCommandResult {
    pub fn completed(request_id: u64, outcome: NodeCommandOutcome) -> Self {
        Self {
            request_id,
            outcome,
        }
    }

    pub fn snapshot(request_id: u64, snapshot: NodeSnapshot) -> Self {
        Self::completed(request_id, NodeCommandOutcome::Snapshot(Box::new(snapshot)))
    }

    pub fn ui_manifest(request_id: u64, manifest: NodeUiManifest) -> Self {
        Self::completed(
            request_id,
            NodeCommandOutcome::UiManifest(Box::new(manifest)),
        )
    }

    pub fn rejected(request_id: u64, problem: NodeProblem) -> Self {
        Self::completed(request_id, NodeCommandOutcome::Rejected(problem))
    }

    pub fn is_success(&self) -> bool {
        !matches!(self.outcome, NodeCommandOutcome::Rejected(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodePeerProbeSnapshot {
    pub peer_id: String,
    pub virtual_ipv6: std::net::Ipv6Addr,
    pub endpoint: Option<std::net::SocketAddr>,
    pub rtt_us: Option<u64>,
    pub p2p_active: bool,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeCleanupStepStatus {
    Removed,
    NotOwned,
    Deferred,
    Failed,
    Unknown,
}

impl NodeCleanupStepStatus {
    pub const fn is_clean(self) -> bool {
        matches!(self, Self::Removed | Self::NotOwned)
    }
}

/// One sanitized runtime cleanup step. Host paths and raw operating-system
/// errors are intentionally excluded from this public contract.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeCleanupStepSnapshot {
    pub network_name: String,
    pub resource: String,
    pub status: NodeCleanupStepStatus,
    pub detail: Option<String>,
}

/// Transport-neutral residue report shared by diagnostics and explicit cleanup.
#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeRuntimeResidueSnapshot {
    pub sessions_remaining: usize,
    pub listeners_remaining: usize,
    pub peers_remaining: usize,
    pub embedded_services_remaining: usize,
    pub configuration_retained: bool,
    pub residual_networks: Vec<String>,
    pub cleanup_steps: Vec<NodeCleanupStepSnapshot>,
    pub issues: Vec<String>,
}

impl NodeRuntimeResidueSnapshot {
    pub fn is_clean(&self) -> bool {
        self.sessions_remaining == 0
            && self.listeners_remaining == 0
            && self.peers_remaining == 0
            && self.embedded_services_remaining == 0
            && self.residual_networks.is_empty()
            && self.issues.is_empty()
    }
}

/// Live actor-owned diagnostics. Counts describe observed runtime ownership,
/// not merely configuration or UI snapshot projections.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeDiagnosticsSnapshot {
    pub lifecycle: NodeLifecycleSnapshot,
    pub retained_owner_networks: Vec<String>,
    pub cleanup_candidate_networks: Vec<String>,
    pub residue: NodeRuntimeResidueSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeCleanupFailureReason {
    StillRunning,
    OwnerUnavailable,
    TimedOut,
    Incomplete,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeCleanupFailureSnapshot {
    pub network_name: String,
    pub reason: NodeCleanupFailureReason,
}

/// Partial-success aware result for explicit residual cleanup.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeResidualCleanupResult {
    pub cleaned_networks: Vec<String>,
    pub failed_networks: Vec<NodeCleanupFailureSnapshot>,
    pub residue: NodeRuntimeResidueSnapshot,
}

#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "status", content = "data", rename_all = "snake_case")]
pub enum NodeCommandOutcome {
    Snapshot(Box<NodeSnapshot>),
    UiManifest(Box<NodeUiManifest>),
    Diagnostics(NodeDiagnosticsSnapshot),
    Cleanup(NodeResidualCleanupResult),
    Probe(NodePeerProbeSnapshot),
    Accepted { operation_id: String },
    Completed { message: String },
    Rejected(NodeProblem),
}

pub type NodeCommandFuture = Pin<Box<dyn Future<Output = NodeCommandResult> + Send + 'static>>;

/// UI adapters implement no business logic; they only submit commands here.
pub trait NodeCommandHandler: Send + Sync + 'static {
    fn handle(&self, request: NodeCommandRequest) -> NodeCommandFuture;
}

struct CommandEnvelope {
    request: NodeCommandRequest,
    response: oneshot::Sender<NodeCommandResult>,
}

/// Asynchronous command bus shared by CLI, IPC, WebUI and GPUI.
#[derive(Clone)]
pub struct NodeCommandBus {
    sender: mpsc::Sender<CommandEnvelope>,
}

impl NodeCommandBus {
    pub fn spawn<H>(handler: Arc<H>, events: NodeEventBus, capacity: usize) -> Self
    where
        H: NodeCommandHandler,
    {
        let (sender, mut receiver) = mpsc::channel::<CommandEnvelope>(capacity.max(1));
        tokio::spawn(async move {
            let mut in_flight = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    envelope = receiver.recv() => {
                        let Some(envelope) = envelope else {
                            break;
                        };
                        let handler = handler.clone();
                        let events = events.clone();
                        in_flight.spawn(async move {
                            let request_id = envelope.request.request_id;
                            let result = handler.handle(envelope.request).await;
                            events.publish(NodeEventKind::CommandCompleted {
                                request_id,
                                success: result.is_success(),
                            });
                            let _ = envelope.response.send(result);
                        });
                    }
                    Some(_) = in_flight.join_next(), if !in_flight.is_empty() => {}
                }
            }
            while in_flight.join_next().await.is_some() {}
        });

        Self { sender }
    }

    pub async fn execute(
        &self,
        request: NodeCommandRequest,
    ) -> Result<NodeCommandResult, NodeCommandBusError> {
        request.validate().map_err(NodeCommandBusError::Invalid)?;
        let request_id = request.request_id;
        let (response, receiver) = oneshot::channel();
        self.sender
            .send(CommandEnvelope { request, response })
            .await
            .map_err(|_| NodeCommandBusError::Closed)?;
        receiver
            .await
            .map_err(|_| NodeCommandBusError::ResponseDropped(request_id))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NodeCommandBusError {
    #[error("节点命令校验失败: {0:?}")]
    Invalid(NodeProblem),
    #[error("节点命令总线已关闭")]
    Closed,
    #[error("节点命令 {0} 的结果通道已关闭")]
    ResponseDropped(u64),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_commands() -> Vec<NodeCommand> {
        vec![
            NodeCommand::GetSnapshot,
            NodeCommand::GetUiManifest,
            NodeCommand::GetDiagnostics,
            NodeCommand::StartNetworks {
                selection: NodeNetworkSelection::Named(vec!["home".into()]),
            },
            NodeCommand::StopNetworks {
                networks: Vec::new(),
                all: true,
            },
            NodeCommand::RestartNetworks {
                networks: vec!["home".into()],
                all: false,
            },
            NodeCommand::CreateNetwork(NodeCreateNetworkCommand {
                name: "home".into(),
                signaling_port: 6666,
                relay_port: 6667,
                external_host: Some("vpn.example.com".into()),
                auto_connect: true,
            }),
            NodeCommand::JoinNetwork(NodeJoinNetworkCommand {
                target: "fk://invite".into(),
                network_name: Some("office".into()),
                start: true,
            }),
            NodeCommand::UpdateRouting(NodeUpdateRoutingCommand {
                network_name: "home".into(),
                mode: NodeRoutingMode::GlobalProxy,
                exit_node: Some("office".into()),
                is_exit_node: false,
            }),
            NodeCommand::SetNetworkRole(NodeSetNetworkRoleCommand {
                network_name: "home".into(),
                role: NodeNetworkRole::ExitNode,
                enabled: true,
            }),
            NodeCommand::ProbePeer(NodeProbePeerCommand {
                network_name: "home".into(),
                target: "fd00::2".into(),
                timeout_ms: 1_000,
            }),
            NodeCommand::StartNode,
            NodeCommand::StopNode,
            NodeCommand::RestartNode,
            NodeCommand::CleanupResiduals {
                networks: vec!["home".into()],
                all: false,
            },
            NodeCommand::Shutdown,
        ]
    }

    fn assert_invalid(command: NodeCommand) {
        let problem = command.validate().expect_err("command must be rejected");
        assert_eq!(problem.code, NodeProblemCode::InvalidRequest);
        assert!(!problem.retryable);
        assert!(!problem.message.is_empty());
    }

    struct EchoHandler;

    impl NodeCommandHandler for EchoHandler {
        fn handle(&self, request: NodeCommandRequest) -> NodeCommandFuture {
            Box::pin(async move {
                NodeCommandResult::completed(
                    request.request_id,
                    NodeCommandOutcome::Completed {
                        message: "ok".into(),
                    },
                )
            })
        }
    }

    #[test]
    fn rejects_product_identity_and_unsafe_network_names() {
        let command = NodeCommand::CreateNetwork(NodeCreateNetworkCommand {
            name: "../server".into(),
            signaling_port: 6666,
            relay_port: 6667,
            external_host: None,
            auto_connect: true,
        });
        let problem = command.validate().expect_err("unsafe name must fail");
        assert_eq!(problem.code, NodeProblemCode::InvalidRequest);

        let command = NodeCommand::StopNetworks {
            networks: vec!["home".into(), "home".into()],
            all: false,
        };
        assert!(command.validate().is_err());
        let command = NodeCommand::RestartNetworks {
            networks: vec!["home".into(), "home".into()],
            all: false,
        };
        assert!(command.validate().is_err());

        assert_invalid(NodeCommand::CreateNetwork(NodeCreateNetworkCommand {
            name: "局域网".into(),
            signaling_port: 6666,
            relay_port: 6667,
            external_host: None,
            auto_connect: true,
        }));
    }

    #[test]
    fn every_command_validates_and_round_trips_through_json() {
        for command in valid_commands() {
            command.validate().expect("valid command");
            let encoded = serde_json::to_string(&command).expect("serialize command");
            let decoded: NodeCommand = serde_json::from_str(&encoded).expect("deserialize command");
            assert_eq!(decoded, command);
        }
    }

    #[test]
    fn serialized_command_names_match_the_canonical_wire_names() {
        let mut expected = NodeCommandKind::ALL
            .iter()
            .map(|kind| kind.as_str())
            .collect::<Vec<_>>();
        let mut actual = valid_commands()
            .into_iter()
            .map(|command| {
                let value = serde_json::to_value(command).expect("serialize command");
                value["command"]
                    .as_str()
                    .expect("serialized command name")
                    .to_owned()
            })
            .collect::<Vec<_>>();
        expected.sort_unstable();
        actual.sort_unstable();
        assert_eq!(actual, expected);
    }

    #[test]
    fn every_payload_command_rejects_invalid_input() {
        let commands = vec![
            NodeCommand::StartNetworks {
                selection: NodeNetworkSelection::Named(Vec::new()),
            },
            NodeCommand::StartNetworks {
                selection: NodeNetworkSelection::Named(vec!["home".into(), "home".into()]),
            },
            NodeCommand::StopNetworks {
                networks: Vec::new(),
                all: false,
            },
            NodeCommand::StopNetworks {
                networks: vec!["home".into()],
                all: true,
            },
            NodeCommand::RestartNetworks {
                networks: Vec::new(),
                all: false,
            },
            NodeCommand::RestartNetworks {
                networks: vec!["home".into()],
                all: true,
            },
            NodeCommand::CleanupResiduals {
                networks: Vec::new(),
                all: false,
            },
            NodeCommand::CleanupResiduals {
                networks: vec!["home".into()],
                all: true,
            },
            NodeCommand::CleanupResiduals {
                networks: vec!["home".into(), "home".into()],
                all: false,
            },
            NodeCommand::CreateNetwork(NodeCreateNetworkCommand {
                name: "home".into(),
                signaling_port: 0,
                relay_port: 6667,
                external_host: None,
                auto_connect: true,
            }),
            NodeCommand::CreateNetwork(NodeCreateNetworkCommand {
                name: "home".into(),
                signaling_port: 6666,
                relay_port: 6666,
                external_host: None,
                auto_connect: true,
            }),
            NodeCommand::CreateNetwork(NodeCreateNetworkCommand {
                name: "home".into(),
                signaling_port: 6666,
                relay_port: 6667,
                external_host: Some("  ".into()),
                auto_connect: true,
            }),
            NodeCommand::JoinNetwork(NodeJoinNetworkCommand {
                target: "  ".into(),
                network_name: None,
                start: false,
            }),
            NodeCommand::JoinNetwork(NodeJoinNetworkCommand {
                target: "ws://127.0.0.1:6666".into(),
                network_name: Some("../office".into()),
                start: false,
            }),
            NodeCommand::UpdateRouting(NodeUpdateRoutingCommand {
                network_name: "home".into(),
                mode: NodeRoutingMode::MeshOnly,
                exit_node: Some("  ".into()),
                is_exit_node: false,
            }),
            NodeCommand::SetNetworkRole(NodeSetNetworkRoleCommand {
                network_name: "home/office".into(),
                role: NodeNetworkRole::Relay,
                enabled: true,
            }),
            NodeCommand::ProbePeer(NodeProbePeerCommand {
                network_name: "home".into(),
                target: "  ".into(),
                timeout_ms: 1_000,
            }),
            NodeCommand::ProbePeer(NodeProbePeerCommand {
                network_name: "home".into(),
                target: "fd00::2".into(),
                timeout_ms: 0,
            }),
        ];

        for command in commands {
            assert_invalid(command);
        }
    }

    #[test]
    fn network_roles_are_capabilities_not_product_identities() {
        let command = NodeCommand::SetNetworkRole(NodeSetNetworkRoleCommand {
            network_name: "home".into(),
            role: NodeNetworkRole::ExitNode,
            enabled: true,
        });
        command.validate().expect("valid role command");

        let encoded = serde_json::to_string(&command).expect("serialize command");
        assert!(encoded.contains("exit_node"));
        assert!(!encoded.contains("client_mode"));
        assert!(!encoded.contains("server_mode"));
    }

    #[tokio::test]
    async fn command_bus_returns_async_results_and_publishes_completion() {
        let events = NodeEventBus::new(8);
        let mut event_rx = events.subscribe();
        let bus = NodeCommandBus::spawn(Arc::new(EchoHandler), events, 4);
        let result = bus
            .execute(NodeCommandRequest::new(42, NodeCommand::GetSnapshot))
            .await
            .expect("command result");

        assert_eq!(result.request_id, 42);
        assert!(result.is_success());
        let event = event_rx.recv().await.expect("completion event");
        assert_eq!(
            event.kind,
            NodeEventKind::CommandCompleted {
                request_id: 42,
                success: true,
            }
        );
    }

    #[tokio::test]
    async fn command_bus_rejects_invalid_command_before_dispatch() {
        let bus = NodeCommandBus::spawn(Arc::new(EchoHandler), NodeEventBus::default(), 4);
        let result = bus
            .execute(NodeCommandRequest::new(
                7,
                NodeCommand::ProbePeer(NodeProbePeerCommand {
                    network_name: "home".into(),
                    target: "fd00::2".into(),
                    timeout_ms: 0,
                }),
            ))
            .await;
        assert!(matches!(result, Err(NodeCommandBusError::Invalid(_))));
    }
}
