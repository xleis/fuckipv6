use crate::config_transaction::{
    ConfigurationApplyResult, ConfigurationDocument, ConfigurationHistoryEntry,
    ConfigurationPreview,
};
use fuckipv6_common::domain::{ActivityEntry, AuditEntry, AuditOutcome};
use fuckipv6_common::{validate_ipv4_cidr, validate_ipv6_cidr, NetworkAllocation, NetworkMode};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, oneshot};

use super::authorization::{authorize_command, CommandDenial};
use super::ui_manifest::audit_for_command;
use super::{
    NodeCommandContext, NodeEventBus, NodeEventKind, NodeLifecycleSnapshot, NodeNetworkRole,
    NodeNetworkSelection, NodeProblem, NodeProblemCode, NodeRoutingMode, NodeSnapshot,
    NodeStateStore, NodeUiManifest,
};

#[cfg(test)]
use fuckipv6_common::domain::NodeId;

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
    RebuildTun,
    CreateNetwork,
    JoinNetwork,
    UpdateRouting,
    SetNetworkRole,
    SetNodeDisplayName,
    GetBlacklist,
    BanClient,
    UnbanClient,
    GetNetworkAllocation,
    UpdateNetworkAllocation,
    ProbePeer,
    StartNode,
    StopNode,
    RestartNode,
    #[serde(rename = "cleanup_node_residuals")]
    CleanupResiduals,
    GetConfiguration,
    PreviewConfiguration,
    ApplyConfiguration,
    RollbackConfiguration,
    GetConfigurationHistory,
    Shutdown,
}

impl NodeCommandKind {
    pub const ALL: [Self; 28] = [
        Self::GetSnapshot,
        Self::GetUiManifest,
        Self::GetDiagnostics,
        Self::StartNetworks,
        Self::StopNetworks,
        Self::RestartNetworks,
        Self::RebuildTun,
        Self::CreateNetwork,
        Self::JoinNetwork,
        Self::UpdateRouting,
        Self::SetNetworkRole,
        Self::SetNodeDisplayName,
        Self::GetBlacklist,
        Self::BanClient,
        Self::UnbanClient,
        Self::GetNetworkAllocation,
        Self::UpdateNetworkAllocation,
        Self::ProbePeer,
        Self::StartNode,
        Self::StopNode,
        Self::RestartNode,
        Self::CleanupResiduals,
        Self::GetConfiguration,
        Self::PreviewConfiguration,
        Self::ApplyConfiguration,
        Self::RollbackConfiguration,
        Self::GetConfigurationHistory,
        Self::Shutdown,
    ];

    /// Commands that must be reachable through a user-visible manifest action.
    /// The manifest bootstrap command is deliberately excluded.
    pub const ACTIONS: [Self; 27] = [
        Self::GetSnapshot,
        Self::GetDiagnostics,
        Self::StartNetworks,
        Self::StopNetworks,
        Self::RestartNetworks,
        Self::RebuildTun,
        Self::CreateNetwork,
        Self::JoinNetwork,
        Self::UpdateRouting,
        Self::SetNetworkRole,
        Self::SetNodeDisplayName,
        Self::GetBlacklist,
        Self::BanClient,
        Self::UnbanClient,
        Self::GetNetworkAllocation,
        Self::UpdateNetworkAllocation,
        Self::ProbePeer,
        Self::StartNode,
        Self::StopNode,
        Self::RestartNode,
        Self::CleanupResiduals,
        Self::GetConfiguration,
        Self::PreviewConfiguration,
        Self::ApplyConfiguration,
        Self::RollbackConfiguration,
        Self::GetConfigurationHistory,
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
            Self::RebuildTun => "rebuild_tun",
            Self::CreateNetwork => "create_network",
            Self::JoinNetwork => "join_network",
            Self::UpdateRouting => "update_routing",
            Self::SetNetworkRole => "set_network_role",
            Self::SetNodeDisplayName => "set_node_display_name",
            Self::GetBlacklist => "get_blacklist",
            Self::BanClient => "ban_client",
            Self::UnbanClient => "unban_client",
            Self::GetNetworkAllocation => "get_network_allocation",
            Self::UpdateNetworkAllocation => "update_network_allocation",
            Self::ProbePeer => "probe_peer",
            Self::StartNode => "start_node",
            Self::StopNode => "stop_node",
            Self::RestartNode => "restart_node",
            Self::CleanupResiduals => "cleanup_node_residuals",
            Self::Shutdown => "shutdown",
            Self::GetConfiguration => "get_configuration",
            Self::PreviewConfiguration => "preview_configuration",
            Self::ApplyConfiguration => "apply_configuration",
            Self::RollbackConfiguration => "rollback_configuration",
            Self::GetConfigurationHistory => "get_configuration_history",
        }
    }

    /// Whether this command can change node or network state. Read-only and
    /// diagnostic commands keep the synchronous command response; mutations
    /// must be submitted as idempotent operations by the v1 control API.
    pub const fn is_write(self) -> bool {
        matches!(
            self,
            Self::StartNetworks
                | Self::StopNetworks
                | Self::RestartNetworks
                | Self::RebuildTun
                | Self::CreateNetwork
                | Self::JoinNetwork
                | Self::UpdateRouting
                | Self::SetNetworkRole
                | Self::SetNodeDisplayName
                | Self::BanClient
                | Self::UnbanClient
                | Self::UpdateNetworkAllocation
                | Self::StartNode
                | Self::StopNode
                | Self::RestartNode
                | Self::CleanupResiduals
                | Self::Shutdown
                | Self::ApplyConfiguration
                | Self::RollbackConfiguration
        )
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
    /// Recreate one network's TUN under the actor-owned lifecycle transition.
    RebuildTun {
        network_name: String,
    },
    CreateNetwork(NodeCreateNetworkCommand),
    JoinNetwork(NodeJoinNetworkCommand),
    UpdateRouting(NodeUpdateRoutingCommand),
    SetNetworkRole(NodeSetNetworkRoleCommand),
    SetNodeDisplayName(NodeSetNodeDisplayNameCommand),
    GetBlacklist {
        network_name: String,
    },
    BanClient(NodeClientBlacklistCommand),
    UnbanClient(NodeClientBlacklistCommand),
    GetNetworkAllocation {
        network_name: String,
    },
    UpdateNetworkAllocation(NodeUpdateNetworkAllocationCommand),
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
    /// Complete redacted configuration for every configured network.
    GetConfiguration,
    /// Validate an imported complete configuration without writing it.
    PreviewConfiguration(NodePreviewConfigurationCommand),
    /// Persist all network configurations under one revision transaction.
    ApplyConfiguration(NodeApplyConfigurationCommand),
    /// Restore a previous revision as a new auditable revision.
    RollbackConfiguration(NodeRollbackConfigurationCommand),
    /// Append-only configuration revision history, newest first.
    GetConfigurationHistory,
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
            Self::RebuildTun { .. } => NodeCommandKind::RebuildTun,
            Self::CreateNetwork(_) => NodeCommandKind::CreateNetwork,
            Self::JoinNetwork(_) => NodeCommandKind::JoinNetwork,
            Self::UpdateRouting(_) => NodeCommandKind::UpdateRouting,
            Self::SetNetworkRole(_) => NodeCommandKind::SetNetworkRole,
            Self::SetNodeDisplayName(_) => NodeCommandKind::SetNodeDisplayName,
            Self::GetBlacklist { .. } => NodeCommandKind::GetBlacklist,
            Self::BanClient(_) => NodeCommandKind::BanClient,
            Self::UnbanClient(_) => NodeCommandKind::UnbanClient,
            Self::GetNetworkAllocation { .. } => NodeCommandKind::GetNetworkAllocation,
            Self::UpdateNetworkAllocation(_) => NodeCommandKind::UpdateNetworkAllocation,
            Self::ProbePeer(_) => NodeCommandKind::ProbePeer,
            Self::StartNode => NodeCommandKind::StartNode,
            Self::StopNode => NodeCommandKind::StopNode,
            Self::RestartNode => NodeCommandKind::RestartNode,
            Self::CleanupResiduals { .. } => NodeCommandKind::CleanupResiduals,
            Self::Shutdown => NodeCommandKind::Shutdown,
            Self::GetConfiguration => NodeCommandKind::GetConfiguration,
            Self::PreviewConfiguration(_) => NodeCommandKind::PreviewConfiguration,
            Self::ApplyConfiguration(_) => NodeCommandKind::ApplyConfiguration,
            Self::RollbackConfiguration(_) => NodeCommandKind::RollbackConfiguration,
            Self::GetConfigurationHistory => NodeCommandKind::GetConfigurationHistory,
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
            | Self::Shutdown
            | Self::GetConfiguration
            | Self::GetConfigurationHistory => Ok(()),
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
            Self::RebuildTun { network_name } => validate_network_name(network_name),
            Self::CreateNetwork(command) => command.validate(),
            Self::JoinNetwork(command) => command.validate(),
            Self::UpdateRouting(command) => command.validate(),
            Self::SetNetworkRole(command) => command.validate(),
            Self::SetNodeDisplayName(command) => command.validate(),
            Self::GetBlacklist { network_name } | Self::GetNetworkAllocation { network_name } => {
                validate_network_name(network_name)
            }
            Self::BanClient(command) | Self::UnbanClient(command) => command.validate(),
            Self::UpdateNetworkAllocation(command) => command.validate(),
            Self::ProbePeer(command) => command.validate(),
            Self::PreviewConfiguration(command) => command.validate(),
            Self::ApplyConfiguration(command) => command.validate(),
            Self::RollbackConfiguration(command) => command.validate(),
        }
    }
}

/// Complete document imported by an owner for preview or atomic apply.
/// Actor and request source are intentionally absent: the command bus supplies
/// them from the authenticated transport context.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodePreviewConfigurationCommand {
    pub expected_revision: u64,
    pub networks: BTreeMap<String, serde_json::Value>,
}

impl NodePreviewConfigurationCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeApplyConfigurationCommand {
    pub expected_revision: u64,
    pub networks: BTreeMap<String, serde_json::Value>,
}

impl NodeApplyConfigurationCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeRollbackConfigurationCommand {
    pub expected_revision: u64,
    pub target_revision: u64,
}

impl NodeRollbackConfigurationCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        Ok(())
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
        // Transport/name syntax is rejected before the command reaches the
        // runtime actor. Policy validation intentionally lives in
        // `validate_runtime_policy` so a running network can publish an
        // authoritative Invalid routing summary instead of silently retaining
        // a misleading Current summary.
        validate_network_name(&self.network_name)
    }

    pub(crate) fn validate_runtime_policy(&self) -> Result<(), NodeProblem> {
        if self
            .exit_node
            .as_deref()
            .is_some_and(|exit| exit.trim().is_empty())
        {
            return Err(
                invalid("出口节点不能是空字符串；不设置出口节点时应省略该字段")
                    .for_network(self.network_name.clone()),
            );
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
pub struct NodeSetNodeDisplayNameCommand {
    pub display_name: Option<String>,
}

impl NodeSetNodeDisplayNameCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        let Some(display_name) = self.display_name.as_deref() else {
            return Ok(());
        };
        let display_name = display_name.trim();
        if display_name.chars().any(char::is_control) {
            return Err(invalid("节点显示名不能包含控制字符"));
        }
        if display_name.chars().count() > 80 {
            return Err(invalid("节点显示名不能超过 80 个字符"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeClientBlacklistCommand {
    pub network_name: String,
    pub client_id: String,
}

impl NodeClientBlacklistCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        validate_network_name(&self.network_name)?;
        if self.client_id.trim().is_empty() {
            return Err(invalid("客户端标识不能为空").for_network(self.network_name.clone()));
        }
        Ok(())
    }

    pub fn normalized_client_id(&self) -> String {
        self.client_id.trim().to_string()
    }
}

/// Transport-neutral allocation payload shared by local adapters and the
/// node command protocol. Persistence is owned by the embedded service actor,
/// never by a UI adapter.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeNetworkAllocation {
    pub mode: NetworkMode,
    pub ipv4_pool: String,
    pub ipv6_pool: String,
}

impl NodeNetworkAllocation {
    fn validate(&self) -> Result<(), NodeProblem> {
        if self.mode.is_ipv4_enabled() {
            validate_ipv4_cidr(&self.ipv4_pool, "IPv4 网段")
                .map_err(|error| invalid(format!("IPv4 网段无效: {error:#}")))?;
        }
        if self.mode.is_ipv6_enabled() {
            validate_ipv6_cidr(&self.ipv6_pool, "IPv6 网段")
                .map_err(|error| invalid(format!("IPv6 网段无效: {error:#}")))?;
        }
        Ok(())
    }

    pub(crate) fn to_network_allocation(&self) -> NetworkAllocation {
        NetworkAllocation {
            mode: self.mode,
            ipv4_pool: self.ipv4_pool.clone(),
            ipv6_pool: self.ipv6_pool.clone(),
            ip_pool: Some(self.ipv6_pool.clone()),
        }
    }

    pub(crate) fn from_network_allocation(allocation: NetworkAllocation) -> Self {
        let ipv6_pool = allocation.effective_ipv6_pool().to_string();
        Self {
            mode: allocation.mode,
            ipv4_pool: allocation.ipv4_pool,
            ipv6_pool,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeUpdateNetworkAllocationCommand {
    pub network_name: String,
    pub allocation: NodeNetworkAllocation,
}

impl NodeUpdateNetworkAllocationCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        validate_network_name(&self.network_name)?;
        self.allocation
            .validate()
            .map_err(|problem| problem.for_network(self.network_name.clone()))
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
pub struct NodeBlacklistSnapshot {
    pub network_name: String,
    pub clients: Vec<String>,
    /// True when the operation changed persisted blacklist state. Queries and
    /// idempotent mutations report false.
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeNetworkAllocationSnapshot {
    pub network_name: String,
    pub allocation: NodeNetworkAllocation,
    pub config_saved: bool,
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

/// Owner-local result of creating a network.
///
/// The invitation ticket is deliberately returned only by this command result
/// and is never added to `NodeSnapshot`, events, or logs. Its `Debug` output is
/// redacted so an adapter cannot accidentally leak it through generic error
/// formatting.
#[derive(Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeCreatedNetwork {
    pub network_name: String,
    pub config_saved: bool,
    pub started: bool,
    pub invitation_ticket: String,
}

impl NodeCreatedNetwork {
    /// Human-readable result for an owner-facing CLI or local UI.
    pub fn owner_message(&self) -> String {
        format!(
            "网络 [{}] 已创建并保存；{}；邀请票据: {}",
            self.network_name,
            if self.started {
                "本地运行时已启动"
            } else {
                "本地运行时未自动启动"
            },
            self.invitation_ticket
        )
    }
}

impl std::fmt::Debug for NodeCreatedNetwork {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NodeCreatedNetwork")
            .field("network_name", &self.network_name)
            .field("config_saved", &self.config_saved)
            .field("started", &self.started)
            .field("invitation_ticket", &"<redacted>")
            .finish()
    }
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
    Blacklist(NodeBlacklistSnapshot),
    NetworkAllocation(NodeNetworkAllocationSnapshot),
    Probe(NodePeerProbeSnapshot),
    NetworkCreated(NodeCreatedNetwork),
    Configuration(ConfigurationDocument),
    ConfigurationPreview(Box<ConfigurationPreview>),
    ConfigurationApplied(Box<ConfigurationApplyResult>),
    ConfigurationHistory(Vec<ConfigurationHistoryEntry>),
    Accepted { operation_id: String },
    Completed { message: String },
    Rejected(NodeProblem),
}

pub type NodeCommandFuture = Pin<Box<dyn Future<Output = NodeCommandResult> + Send + 'static>>;

/// UI adapters implement no business logic; they only submit commands here.
pub trait NodeCommandHandler: Send + Sync + 'static {
    fn handle(&self, request: NodeCommandRequest) -> NodeCommandFuture;

    /// Trusted execution context supplied by the command bus. Handlers that
    /// need the authenticated actor/source override this method; adapters that
    /// only proxy commands preserve the original behavior.
    fn handle_authorized(
        &self,
        _context: NodeCommandContext,
        request: NodeCommandRequest,
    ) -> NodeCommandFuture {
        self.handle(request)
    }
}

struct CommandEnvelope {
    context: NodeCommandContext,
    request: NodeCommandRequest,
    response: oneshot::Sender<NodeCommandResult>,
}

#[derive(Default)]
struct CommandAuditTrail {
    entries: Arc<Mutex<Vec<AuditEntry>>>,
    recent_activity: Arc<Mutex<Vec<ActivityEntry>>>,
    next_sequence: Arc<AtomicU64>,
}

impl CommandAuditTrail {
    fn record(
        &self,
        context: &NodeCommandContext,
        request: &NodeCommandRequest,
        outcome: AuditOutcome,
        reason: &str,
    ) {
        let sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
        let audit_id = fuckipv6_common::domain::AuditId::from_public_key(
            format!(
                "{}:{}:{}:{}:{}",
                context.actor(),
                request.request_id,
                request.command.kind().as_str(),
                sequence,
                unix_time_ms()
            )
            .as_bytes(),
        );
        let canonical_action = audit_for_command(request.command.kind()).action;
        let action = canonical_action.as_deref().unwrap_or("node.command");
        let mut details = BTreeMap::new();
        details.insert("command".into(), request.command.kind().as_str().into());
        details.insert("request_id".into(), request.request_id.to_string());
        details.insert("authorization_reason".into(), reason.into());
        details.insert("actor_source".into(), context.source().into());
        details.insert("role".into(), context.roles_for_command(&request.command));
        let network_id = context.network_id_for_command(&request.command);
        if let Some(network_id) = &network_id {
            details.insert("network_id".into(), network_id.to_string());
        }
        let summary = match outcome {
            AuditOutcome::Succeeded => "节点命令执行成功",
            AuditOutcome::Failed => "节点命令执行失败",
            AuditOutcome::Denied => "节点命令授权被拒绝",
        };
        let entry = AuditEntry {
            audit_id,
            operation_id: context.audit_operation_id().cloned(),
            actor: context.actor().clone(),
            network_id,
            action: action.into(),
            resource: format!("node_command:{}", request.command.kind().as_str()),
            outcome,
            occurred_at_unix_ms: unix_time_ms(),
            summary: summary.into(),
            details,
            sensitive_keys: Default::default(),
        };
        if let Ok(mut entries) = self.entries.lock() {
            entries.push(entry.clone());
        }
        if request.command.kind().is_write() || outcome == AuditOutcome::Denied {
            if let Ok(mut activity) = self.recent_activity.lock() {
                activity.insert(0, entry.project_activity());
                activity.truncate(5);
            }
        }
    }

    fn entries(&self) -> Vec<AuditEntry> {
        self.entries
            .lock()
            .map(|entries| entries.clone())
            .unwrap_or_default()
    }

    fn recent_activity(&self) -> Vec<ActivityEntry> {
        self.recent_activity
            .lock()
            .map(|activity| activity.clone())
            .unwrap_or_default()
    }
}

/// Asynchronous command bus shared by CLI, IPC, WebUI and GPUI.
#[derive(Clone)]
pub struct NodeCommandBus {
    sender: mpsc::Sender<CommandEnvelope>,
    events: NodeEventBus,
    audit: Arc<CommandAuditTrail>,
    activity_state: Option<NodeStateStore>,
}

impl NodeCommandBus {
    pub fn spawn<H>(handler: Arc<H>, events: NodeEventBus, capacity: usize) -> Self
    where
        H: NodeCommandHandler,
    {
        Self::spawn_with_activity_state(handler, events, capacity, None)
    }

    /// Spawn a command bus that publishes its sanitized activity feed into the
    /// canonical node snapshot. Transport adapters receive activity through the
    /// same watch/SSE channel as every other node-state change.
    pub(crate) fn spawn_with_state<H>(
        handler: Arc<H>,
        events: NodeEventBus,
        capacity: usize,
        state: NodeStateStore,
    ) -> Self
    where
        H: NodeCommandHandler,
    {
        Self::spawn_with_activity_state(handler, events, capacity, Some(state))
    }

    fn spawn_with_activity_state<H>(
        handler: Arc<H>,
        events: NodeEventBus,
        capacity: usize,
        activity_state: Option<NodeStateStore>,
    ) -> Self
    where
        H: NodeCommandHandler,
    {
        let (sender, mut receiver) = mpsc::channel::<CommandEnvelope>(capacity.max(1));
        let bus_events = events.clone();
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
                            let result = handler
                                .handle_authorized(envelope.context, envelope.request)
                                .await;
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

        Self {
            sender,
            events: bus_events,
            audit: Arc::new(CommandAuditTrail::default()),
            activity_state,
        }
    }

    /// Execute one command after server-side authorization. The context must be
    /// constructed by the authenticated transport adapter, never from request
    /// JSON or UI permission declarations.
    pub async fn execute(
        &self,
        context: NodeCommandContext,
        request: NodeCommandRequest,
    ) -> Result<NodeCommandResult, NodeCommandBusError> {
        request.validate().map_err(NodeCommandBusError::Invalid)?;
        let request_id = request.request_id;
        if let Err(denial) = authorize_command(&context, &request.command).map(|_| ()) {
            let result = denied(request_id, &denial);
            self.events.publish(NodeEventKind::CommandCompleted {
                request_id,
                success: false,
            });
            self.record_and_publish_activity(
                &context,
                &request,
                AuditOutcome::Denied,
                &denial.reason,
            )
            .await;
            return Ok(result);
        }

        let (response, receiver) = oneshot::channel();
        let audit_request = request.clone();
        if self
            .sender
            .send(CommandEnvelope {
                context: context.clone(),
                request,
                response,
            })
            .await
            .is_err()
        {
            return Err(NodeCommandBusError::Closed);
        }
        match receiver.await {
            Ok(result) => {
                self.record_and_publish_activity(
                    &context,
                    &audit_request,
                    if result.is_success() {
                        AuditOutcome::Succeeded
                    } else {
                        AuditOutcome::Failed
                    },
                    "command authorized; handler outcome recorded",
                )
                .await;
                Ok(result)
            }
            Err(_) => {
                self.record_and_publish_activity(
                    &context,
                    &audit_request,
                    AuditOutcome::Failed,
                    "handler response channel closed",
                )
                .await;
                Err(NodeCommandBusError::ResponseDropped(request_id))
            }
        }
    }

    async fn record_and_publish_activity(
        &self,
        context: &NodeCommandContext,
        request: &NodeCommandRequest,
        outcome: AuditOutcome,
        reason: &str,
    ) {
        self.audit.record(context, request, outcome, reason);
        if let Some(state) = &self.activity_state {
            state
                .set_recent_activity(self.audit.recent_activity())
                .await;
        }
    }

    /// Read the sanitized authorization/execution audit trail. This is used by
    /// tests and by future owner-only audit projections.
    pub fn audit_entries(&self) -> Vec<AuditEntry> {
        self.audit.entries()
    }

    #[cfg(test)]
    pub(crate) async fn execute_test(
        &self,
        request: NodeCommandRequest,
    ) -> Result<NodeCommandResult, NodeCommandBusError> {
        self.execute(test_owner_context(), request).await
    }
}

pub(crate) fn denied(request_id: u64, denial: &CommandDenial) -> NodeCommandResult {
    let code = if denial.code == "permission.recent_authentication_required" {
        NodeProblemCode::AuthenticationRequired
    } else {
        NodeProblemCode::PermissionDenied
    };
    NodeCommandResult::rejected(
        request_id,
        NodeProblem::new(code, "执行此节点命令需要重新认证或更高权限")
            .with_param("authorization_code", denial.code.clone()),
    )
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().try_into().unwrap_or(u64::MAX))
        .unwrap_or(1)
        .max(1)
}

#[cfg(test)]
fn test_owner_context() -> NodeCommandContext {
    NodeCommandContext::local_owner(NodeId::from_public_key(b"node-command-bus-test"), "test")
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
    use std::sync::atomic::{AtomicUsize, Ordering};

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
            NodeCommand::RebuildTun {
                network_name: "home".into(),
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
            NodeCommand::SetNodeDisplayName(NodeSetNodeDisplayNameCommand {
                display_name: Some("edge-gateway".into()),
            }),
            NodeCommand::GetBlacklist {
                network_name: "home".into(),
            },
            NodeCommand::BanClient(NodeClientBlacklistCommand {
                network_name: "home".into(),
                client_id: "node-bad".into(),
            }),
            NodeCommand::UnbanClient(NodeClientBlacklistCommand {
                network_name: "home".into(),
                client_id: "node-bad".into(),
            }),
            NodeCommand::GetNetworkAllocation {
                network_name: "home".into(),
            },
            NodeCommand::UpdateNetworkAllocation(NodeUpdateNetworkAllocationCommand {
                network_name: "home".into(),
                allocation: NodeNetworkAllocation {
                    mode: NetworkMode::DualStack,
                    ipv4_pool: "10.8.0.0/24".into(),
                    ipv6_pool: "fd00::1000/120".into(),
                },
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
            NodeCommand::GetConfiguration,
            NodeCommand::PreviewConfiguration(NodePreviewConfigurationCommand {
                expected_revision: 0,
                networks: BTreeMap::new(),
            }),
            NodeCommand::ApplyConfiguration(NodeApplyConfigurationCommand {
                expected_revision: 0,
                networks: BTreeMap::new(),
            }),
            NodeCommand::RollbackConfiguration(NodeRollbackConfigurationCommand {
                expected_revision: 0,
                target_revision: 0,
            }),
            NodeCommand::GetConfigurationHistory,
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

    struct CountingHandler {
        calls: Arc<AtomicUsize>,
    }

    impl NodeCommandHandler for CountingHandler {
        fn handle(&self, request: NodeCommandRequest) -> NodeCommandFuture {
            self.calls.fetch_add(1, Ordering::SeqCst);
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
    fn created_network_debug_redacts_the_invitation_ticket() {
        let created = NodeCreatedNetwork {
            network_name: "home".into(),
            config_saved: true,
            started: true,
            invitation_ticket: "fk://sensitive-owner-secret@vpn.example:6666".into(),
        };

        let debug = format!("{created:?}");
        assert!(debug.contains("home"));
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("sensitive-owner-secret"));
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
            NodeCommand::RebuildTun {
                network_name: "../home".into(),
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
            NodeCommand::GetBlacklist {
                network_name: "../home".into(),
            },
            NodeCommand::BanClient(NodeClientBlacklistCommand {
                network_name: "home".into(),
                client_id: "   ".into(),
            }),
            NodeCommand::UpdateNetworkAllocation(NodeUpdateNetworkAllocationCommand {
                network_name: "home".into(),
                allocation: NodeNetworkAllocation {
                    mode: NetworkMode::Ipv4Only,
                    ipv4_pool: "not-a-cidr".into(),
                    ipv6_pool: String::new(),
                },
            }),
            NodeCommand::UpdateNetworkAllocation(NodeUpdateNetworkAllocationCommand {
                network_name: "home".into(),
                allocation: NodeNetworkAllocation {
                    mode: NetworkMode::Ipv6Only,
                    ipv4_pool: String::new(),
                    ipv6_pool: "fd00::/129".into(),
                },
            }),
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
            NodeCommand::SetNetworkRole(NodeSetNetworkRoleCommand {
                network_name: "home/office".into(),
                role: NodeNetworkRole::Relay,
                enabled: true,
            }),
            NodeCommand::SetNodeDisplayName(NodeSetNodeDisplayNameCommand {
                display_name: Some("bad\nname".into()),
            }),
            NodeCommand::SetNodeDisplayName(NodeSetNodeDisplayNameCommand {
                display_name: Some("界".repeat(81)),
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
    fn routing_policy_validation_is_deferred_to_the_runtime_actor() {
        let command = NodeUpdateRoutingCommand {
            network_name: "home".into(),
            mode: NodeRoutingMode::MeshOnly,
            exit_node: Some("  ".into()),
            is_exit_node: false,
        };

        assert!(NodeCommand::UpdateRouting(command.clone())
            .validate()
            .is_ok());
        let problem = command
            .validate_runtime_policy()
            .expect_err("blank exit node must be rejected as runtime policy");
        assert_eq!(problem.code, NodeProblemCode::InvalidRequest);
        assert_eq!(problem.network_name.as_deref(), Some("home"));
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

    #[test]
    fn mutation_classification_keeps_reads_synchronous() {
        for kind in [
            NodeCommandKind::GetSnapshot,
            NodeCommandKind::GetUiManifest,
            NodeCommandKind::GetDiagnostics,
            NodeCommandKind::GetBlacklist,
            NodeCommandKind::GetNetworkAllocation,
            NodeCommandKind::ProbePeer,
            NodeCommandKind::GetConfiguration,
            NodeCommandKind::PreviewConfiguration,
            NodeCommandKind::GetConfigurationHistory,
        ] {
            assert!(!kind.is_write(), "{kind:?}");
        }

        for kind in NodeCommandKind::ACTIONS {
            if !matches!(
                kind,
                NodeCommandKind::GetSnapshot
                    | NodeCommandKind::GetDiagnostics
                    | NodeCommandKind::GetBlacklist
                    | NodeCommandKind::GetNetworkAllocation
                    | NodeCommandKind::ProbePeer
                    | NodeCommandKind::GetConfiguration
                    | NodeCommandKind::PreviewConfiguration
                    | NodeCommandKind::GetConfigurationHistory
            ) {
                assert!(kind.is_write(), "{kind:?}");
            }
        }
    }

    #[tokio::test]
    async fn command_bus_returns_async_results_and_publishes_completion() {
        let events = NodeEventBus::new(8);
        let mut event_rx = events.subscribe();
        let bus = NodeCommandBus::spawn(Arc::new(EchoHandler), events, 4);
        let result = bus
            .execute_test(NodeCommandRequest::new(42, NodeCommand::GetSnapshot))
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
            .execute_test(NodeCommandRequest::new(
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

    #[tokio::test]
    async fn untrusted_command_is_rejected_audited_and_never_dispatched() {
        let calls = Arc::new(AtomicUsize::new(0));
        let bus = NodeCommandBus::spawn(
            Arc::new(CountingHandler {
                calls: calls.clone(),
            }),
            NodeEventBus::default(),
            4,
        );

        let result = bus
            .execute(
                NodeCommandContext::untrusted("legacy_loopback_tcp"),
                NodeCommandRequest::new(51, NodeCommand::GetSnapshot),
            )
            .await
            .expect("authorization rejection is a command result");

        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let NodeCommandOutcome::Rejected(problem) = result.outcome else {
            panic!("expected authorization rejection");
        };
        assert_eq!(problem.code, NodeProblemCode::PermissionDenied);
        assert_eq!(
            problem.params.get("authorization_code").map(String::as_str),
            Some("permission.authentication_required")
        );
        let audit = bus.audit_entries();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].outcome, AuditOutcome::Denied);
        assert_eq!(audit[0].details["command"], "get_snapshot");
        assert_eq!(audit[0].details["actor_source"], "legacy_loopback_tcp");
    }

    #[tokio::test]
    async fn stale_webui_owner_sensitive_command_maps_to_authentication_required() {
        let calls = Arc::new(AtomicUsize::new(0));
        let state = NodeStateStore::new();
        let snapshot = state.snapshot().await;
        let context =
            NodeCommandContext::owner_session_for_snapshot(&snapshot, "webui-public-owner", false);
        let bus = NodeCommandBus::spawn(
            Arc::new(CountingHandler {
                calls: calls.clone(),
            }),
            NodeEventBus::default(),
            4,
        );

        let result = bus
            .execute(context, NodeCommandRequest::new(52, NodeCommand::Shutdown))
            .await
            .expect("authorization rejection is a command result");

        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let NodeCommandOutcome::Rejected(problem) = result.outcome else {
            panic!("expected authorization rejection");
        };
        assert_eq!(problem.code, NodeProblemCode::AuthenticationRequired);
        assert_eq!(
            problem.params.get("authorization_code").map(String::as_str),
            Some("permission.recent_authentication_required")
        );
        assert_eq!(bus.audit_entries().len(), 1);
        assert_eq!(bus.audit_entries()[0].outcome, AuditOutcome::Denied);
    }

    #[tokio::test]
    async fn scoped_denial_audit_records_actor_role_network_and_reason() {
        let network_id = fuckipv6_common::domain::NetworkId::from_public_key(b"audit-network-home");
        let context = NodeCommandContext::network_session(
            actor(),
            "control-api:member-session",
            [],
            [(
                "home".into(),
                crate::node::NetworkCommandScope::new(
                    network_id.clone(),
                    fuckipv6_common::domain::NetworkRole::Administrator,
                    fuckipv6_common::domain::MembershipState::Active,
                    Vec::new(),
                ),
            )],
            true,
        );
        let state = NodeStateStore::new();
        let bus = NodeCommandBus::spawn_with_state(
            Arc::new(EchoHandler),
            NodeEventBus::default(),
            4,
            state.clone(),
        );

        let result = bus
            .execute(
                context,
                NodeCommandRequest::new(
                    53,
                    NodeCommand::StartNetworks {
                        selection: NodeNetworkSelection::Named(vec!["home".into()]),
                    },
                ),
            )
            .await
            .expect("authorization rejection is a command result");

        let NodeCommandOutcome::Rejected(problem) = result.outcome else {
            panic!("expected authorization rejection");
        };
        assert_eq!(
            problem.params.get("authorization_code").map(String::as_str),
            Some("permission.capability_denied")
        );
        let audit = bus.audit_entries();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].actor, actor());
        assert_eq!(audit[0].network_id.as_ref(), Some(&network_id));
        assert_eq!(audit[0].details["role"], "administrator");
        assert_eq!(audit[0].details["network_id"], network_id.to_string());
        assert_eq!(
            audit[0].details["actor_source"],
            "control-api:member-session"
        );
        assert!(audit[0].details["authorization_reason"].contains("capability"));
        let activity = state.snapshot().await.recent_activity;
        assert_eq!(activity.len(), 1);
        assert_eq!(activity[0].outcome, AuditOutcome::Denied);
        assert_eq!(activity[0].details["command"], "start_networks");
    }

    #[tokio::test]
    async fn authorized_command_and_audit_do_not_persist_payload_secrets() {
        let calls = Arc::new(AtomicUsize::new(0));
        let state = NodeStateStore::new();
        let bus = NodeCommandBus::spawn_with_state(
            Arc::new(CountingHandler {
                calls: calls.clone(),
            }),
            NodeEventBus::default(),
            4,
            state.clone(),
        );
        let invitation = "fk://owner-secret-invitation@example.test:6666";

        let result = bus
            .execute(
                NodeCommandContext::local_owner(actor(), "local-ipc:owner"),
                NodeCommandRequest::new(
                    52,
                    NodeCommand::JoinNetwork(NodeJoinNetworkCommand {
                        target: invitation.into(),
                        network_name: Some("home".into()),
                        start: false,
                    }),
                ),
            )
            .await
            .expect("authorized command result");

        assert!(result.is_success());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let audit = bus.audit_entries();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].outcome, AuditOutcome::Succeeded);
        assert_eq!(audit[0].details["command"], "join_network");
        assert!(!format!("{:?}", audit[0]).contains("owner-secret-invitation"));
        let activity = state.snapshot().await.recent_activity;
        assert_eq!(activity.len(), 1);
        assert_eq!(activity[0].details["command"], "join_network");
        assert!(!format!("{activity:?}").contains("owner-secret-invitation"));
    }

    #[tokio::test]
    async fn audit_records_transport_owned_operation_id() {
        let operation_id =
            fuckipv6_common::domain::OperationId::from_public_key(b"transport-operation");
        let bus = NodeCommandBus::spawn(Arc::new(EchoHandler), NodeEventBus::default(), 4);
        let context = test_owner_context().with_audit_operation_id(operation_id.clone());

        bus.execute(
            context,
            NodeCommandRequest::new(54, NodeCommand::GetSnapshot),
        )
        .await
        .expect("authorized command result");

        let audit = bus.audit_entries();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].operation_id.as_ref(), Some(&operation_id));
        assert_eq!(audit[0].details["request_id"], "54");
        assert_eq!(audit[0].details["actor_source"], "test");
        assert_eq!(audit[0].outcome, AuditOutcome::Succeeded);
    }

    #[tokio::test]
    async fn snapshot_activity_is_newest_first_limited_to_five_and_excludes_reads() {
        let state = NodeStateStore::new();
        let bus = NodeCommandBus::spawn_with_state(
            Arc::new(EchoHandler),
            NodeEventBus::default(),
            8,
            state.clone(),
        );

        for request_id in 60..67 {
            bus.execute_test(NodeCommandRequest::new(request_id, NodeCommand::StartNode))
                .await
                .expect("authorized command");
        }
        bus.execute_test(NodeCommandRequest::new(70, NodeCommand::GetSnapshot))
            .await
            .expect("snapshot query");

        let activity = state.snapshot().await.recent_activity;
        assert_eq!(activity.len(), 5);
        assert!(activity
            .iter()
            .all(|entry| entry.details["command"] == "start_node"));
        assert_eq!(activity[0].details["request_id"], "66");
        assert_eq!(activity[4].details["request_id"], "62");
    }

    fn actor() -> NodeId {
        NodeId::from_public_key(b"command-bus-audit-test")
    }
}
