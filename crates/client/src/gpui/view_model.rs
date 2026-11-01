use std::collections::{HashMap, HashSet};

use fuckipv6_common::domain::{CapabilityKind, CapabilityState};
use fuckipv6_common::i18n::{t, tr, Locale};
use fuckipv6_common::NetworkMode;

use crate::node::{
    NodeClientBlacklistCommand, NodeCommand, NodeCommandKind, NodeCreateNetworkCommand,
    NodeEmbeddedServiceStatus, NodeJoinNetworkCommand, NodeNetworkAllocation, NodeNetworkRole,
    NodeNetworkSelection, NodeNetworkStatus, NodePermission, NodePhase, NodeProbePeerCommand,
    NodeRoutingMode, NodeSetNetworkRoleCommand, NodeSnapshot, NodeUiAction, NodeUiAvailability,
    NodeUiBackendStatus, NodeUiManifest, NodeUiPage, NodeUiPermission,
    NodeUpdateNetworkAllocationCommand, NodeUpdateRoutingCommand,
};
use crate::ui::UiMode;

/// Typed views supported by the native adapter. Keeping this closed set separate
/// from page strings makes an unknown manifest page fail closed rather than
/// silently rendering a generic or empty surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PageKind {
    Overview,
    Networks,
    Peers,
    Topology,
    Traffic,
    Routing,
    ExitNode,
    Relay,
    Logs,
    Identity,
    Settings,
}

impl PageKind {
    pub const ALL: [Self; 11] = [
        Self::Overview,
        Self::Networks,
        Self::Peers,
        Self::Topology,
        Self::Traffic,
        Self::Routing,
        Self::ExitNode,
        Self::Relay,
        Self::Logs,
        Self::Identity,
        Self::Settings,
    ];

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "overview" => Some(Self::Overview),
            "networks" => Some(Self::Networks),
            "peers" => Some(Self::Peers),
            "topology" => Some(Self::Topology),
            "traffic" => Some(Self::Traffic),
            "routing" => Some(Self::Routing),
            "exit_node" => Some(Self::ExitNode),
            "relay" => Some(Self::Relay),
            "logs" => Some(Self::Logs),
            "identity" => Some(Self::Identity),
            "settings" => Some(Self::Settings),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Networks => "networks",
            Self::Peers => "peers",
            Self::Topology => "topology",
            Self::Traffic => "traffic",
            Self::Routing => "routing",
            Self::ExitNode => "exit_node",
            Self::Relay => "relay",
            Self::Logs => "logs",
            Self::Identity => "identity",
            Self::Settings => "settings",
        }
    }
}

/// Closed action mapping for every action currently advertised by the shared
/// manifest. `SetNetworkRole` intentionally has two bindings because Exit Node
/// and Relay expose different forms for the same typed backend command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionBinding {
    RefreshSnapshot,
    GetDiagnostics,
    CreateNetwork,
    JoinNetwork,
    StartNetworks,
    StopNetworks,
    RestartNetworks,
    ProbePeer,
    GetBlacklist,
    BanClient,
    UnbanClient,
    GetNetworkAllocation,
    UpdateNetworkAllocation,
    UpdateRouting,
    SetExitNodeRole,
    SetRelayRole,
    StartNode,
    StopNode,
    RestartNode,
    CleanupResiduals,
    ShutdownNode,
}

impl ActionBinding {
    pub const ALL: [Self; 21] = [
        Self::RefreshSnapshot,
        Self::GetDiagnostics,
        Self::CreateNetwork,
        Self::JoinNetwork,
        Self::StartNetworks,
        Self::StopNetworks,
        Self::RestartNetworks,
        Self::ProbePeer,
        Self::GetBlacklist,
        Self::BanClient,
        Self::UnbanClient,
        Self::GetNetworkAllocation,
        Self::UpdateNetworkAllocation,
        Self::UpdateRouting,
        Self::SetExitNodeRole,
        Self::SetRelayRole,
        Self::StartNode,
        Self::StopNode,
        Self::RestartNode,
        Self::CleanupResiduals,
        Self::ShutdownNode,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::RefreshSnapshot => "refresh_snapshot",
            Self::GetDiagnostics => "get_node_diagnostics",
            Self::CreateNetwork => "create_network",
            Self::JoinNetwork => "join_network",
            Self::StartNetworks => "start_networks",
            Self::StopNetworks => "stop_networks",
            Self::RestartNetworks => "restart_networks",
            Self::ProbePeer => "probe_peer",
            Self::GetBlacklist => "get_blacklist",
            Self::BanClient => "ban_client",
            Self::UnbanClient => "unban_client",
            Self::GetNetworkAllocation => "get_network_allocation",
            Self::UpdateNetworkAllocation => "update_network_allocation",
            Self::UpdateRouting => "update_routing",
            Self::SetExitNodeRole => "set_exit_node_role",
            Self::SetRelayRole => "set_relay_role",
            Self::StartNode => "start_node",
            Self::StopNode => "stop_node",
            Self::RestartNode => "restart_node",
            Self::CleanupResiduals => "cleanup_node_residuals",
            Self::ShutdownNode => "shutdown_node",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionBlockReason {
    NodeNotRunning,
    NoOnlineNetwork,
    NoNetworkSelected,
    PermissionDenied,
    ExitNodeUnsupported,
}

impl ActionBlockReason {
    /// Stable catalog key. Adapters must translate this at the presentation
    /// boundary instead of treating UI copy as a business identifier.
    pub const fn message_key(self) -> &'static str {
        match self {
            Self::NodeNotRunning => "blocked.node_not_running",
            Self::NoOnlineNetwork => "blocked.no_online_network",
            Self::NoNetworkSelected => "blocked.no_network_selected",
            Self::PermissionDenied => "blocked.permission_denied",
            Self::ExitNodeUnsupported => "blocked.exit_node_unsupported",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionModel {
    pub id: String,
    pub binding: ActionBinding,
    pub backend_status: NodeUiBackendStatus,
    pub permission: NodeUiPermission,
    pub availability: NodeUiAvailability,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricModel {
    pub label: String,
    pub value: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowModel {
    pub title: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionModel {
    pub title: String,
    pub rows: Vec<RowModel>,
    pub empty_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageModel {
    pub page_id: String,
    pub kind: PageKind,
    pub metrics: Vec<MetricModel>,
    pub sections: Vec<SectionModel>,
    pub actions: Vec<ActionModel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    UnknownPage {
        page_id: String,
    },
    UnknownAction {
        page_id: String,
        action_id: String,
    },
    ActionCommandMismatch {
        action_id: String,
        expected: NodeCommandKind,
        actual: NodeCommandKind,
    },
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownPage { page_id } => write!(formatter, "未知 GPUI 页面: {page_id}"),
            Self::UnknownAction { page_id, action_id } => {
                write!(formatter, "页面 {page_id} 包含未知动作: {action_id}")
            }
            Self::ActionCommandMismatch {
                action_id,
                expected,
                actual,
            } => write!(
                formatter,
                "动作 {action_id} 的命令不匹配: expected={expected:?}, actual={actual:?}"
            ),
        }
    }
}

impl std::error::Error for ModelError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandBuildError {
    UnknownAction {
        action_id: String,
    },
    MissingField {
        action_id: String,
        field_id: String,
    },
    InvalidField {
        action_id: String,
        field_id: String,
        value: String,
    },
}

impl std::fmt::Display for CommandBuildError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownAction { action_id } => {
                write!(formatter, "未知动作，拒绝构造命令: {action_id}")
            }
            Self::MissingField {
                action_id,
                field_id,
            } => {
                write!(formatter, "动作 {action_id} 缺少字段 {field_id}")
            }
            Self::InvalidField {
                action_id,
                field_id,
                value,
            } => write!(
                formatter,
                "动作 {action_id} 的字段 {field_id} 无法转换为命令类型: {value:?}"
            ),
        }
    }
}

impl std::error::Error for CommandBuildError {}

pub fn build_page_model(
    manifest: &NodeUiManifest,
    snapshot: &NodeSnapshot,
    page_id: &str,
    locale: Locale,
) -> Result<PageModel, ModelError> {
    let page = manifest
        .pages
        .iter()
        .find(|page| page.id == page_id)
        .ok_or_else(|| ModelError::UnknownPage {
            page_id: page_id.into(),
        })?;
    build_page_model_for_page(page, snapshot, locale)
}

pub fn build_page_model_for_page(
    page: &NodeUiPage,
    snapshot: &NodeSnapshot,
    locale: Locale,
) -> Result<PageModel, ModelError> {
    let kind = PageKind::from_id(&page.id).ok_or_else(|| ModelError::UnknownPage {
        page_id: page.id.clone(),
    })?;
    let actions = page
        .actions
        .iter()
        .map(|action| {
            bind_action(action).map(|binding| ActionModel {
                id: action.id.clone(),
                binding,
                backend_status: action.backend_status,
                permission: action.permission,
                availability: action.availability,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let (metrics, sections) = project_page(kind, snapshot, locale);
    Ok(PageModel {
        page_id: page.id.clone(),
        kind,
        metrics,
        sections,
        actions,
    })
}

pub fn bind_action(action: &NodeUiAction) -> Result<ActionBinding, ModelError> {
    let expected = match action.id.as_str() {
        "refresh_snapshot" => (ActionBinding::RefreshSnapshot, NodeCommandKind::GetSnapshot),
        "get_node_diagnostics" => (
            ActionBinding::GetDiagnostics,
            NodeCommandKind::GetDiagnostics,
        ),
        "create_network" => (ActionBinding::CreateNetwork, NodeCommandKind::CreateNetwork),
        "join_network" => (ActionBinding::JoinNetwork, NodeCommandKind::JoinNetwork),
        "start_networks" => (ActionBinding::StartNetworks, NodeCommandKind::StartNetworks),
        "stop_networks" => (ActionBinding::StopNetworks, NodeCommandKind::StopNetworks),
        "restart_networks" => (
            ActionBinding::RestartNetworks,
            NodeCommandKind::RestartNetworks,
        ),
        "probe_peer" => (ActionBinding::ProbePeer, NodeCommandKind::ProbePeer),
        "get_blacklist" => (ActionBinding::GetBlacklist, NodeCommandKind::GetBlacklist),
        "ban_client" => (ActionBinding::BanClient, NodeCommandKind::BanClient),
        "unban_client" => (ActionBinding::UnbanClient, NodeCommandKind::UnbanClient),
        "get_network_allocation" => (
            ActionBinding::GetNetworkAllocation,
            NodeCommandKind::GetNetworkAllocation,
        ),
        "update_network_allocation" => (
            ActionBinding::UpdateNetworkAllocation,
            NodeCommandKind::UpdateNetworkAllocation,
        ),
        "update_routing" => (ActionBinding::UpdateRouting, NodeCommandKind::UpdateRouting),
        "set_exit_node_role" => (
            ActionBinding::SetExitNodeRole,
            NodeCommandKind::SetNetworkRole,
        ),
        "set_relay_role" => (ActionBinding::SetRelayRole, NodeCommandKind::SetNetworkRole),
        "start_node" => (ActionBinding::StartNode, NodeCommandKind::StartNode),
        "stop_node" => (ActionBinding::StopNode, NodeCommandKind::StopNode),
        "restart_node" => (ActionBinding::RestartNode, NodeCommandKind::RestartNode),
        "cleanup_node_residuals" => (
            ActionBinding::CleanupResiduals,
            NodeCommandKind::CleanupResiduals,
        ),
        "shutdown_node" => (ActionBinding::ShutdownNode, NodeCommandKind::Shutdown),
        _ => {
            return Err(ModelError::UnknownAction {
                page_id: String::new(),
                action_id: action.id.clone(),
            });
        }
    };

    if action.command != expected.1 {
        return Err(ModelError::ActionCommandMismatch {
            action_id: action.id.clone(),
            expected: expected.1,
            actual: action.command,
        });
    }
    Ok(expected.0)
}

pub fn action_block_reason(
    action: &NodeUiAction,
    snapshot: &NodeSnapshot,
    selected_network: Option<&str>,
) -> Option<ActionBlockReason> {
    if !snapshot
        .permissions
        .permissions
        .contains(&action.permission)
    {
        return Some(ActionBlockReason::PermissionDenied);
    }

    if action.id == "set_exit_node_role" {
        let state = snapshot.capabilities.state(CapabilityKind::Exit);
        if state == CapabilityState::Unsupported
            || (state == CapabilityState::Unknown && !snapshot.exit_node_capabilities.supported)
        {
            return Some(ActionBlockReason::ExitNodeUnsupported);
        }
    }

    match action.availability {
        NodeUiAvailability::Always => None,
        NodeUiAvailability::NodeRunning => (snapshot.lifecycle.phase != NodePhase::Running)
            .then_some(ActionBlockReason::NodeNotRunning),
        NodeUiAvailability::NetworkOnline => (!snapshot
            .networks
            .iter()
            .any(|network| network.status == crate::node::NodeNetworkStatus::Online))
        .then_some(ActionBlockReason::NoOnlineNetwork),
        NodeUiAvailability::NetworkSelected => {
            let selected = selected_network?;
            (!snapshot
                .networks
                .iter()
                .any(|network| network.name == selected))
            .then_some(ActionBlockReason::NoNetworkSelected)
        }
    }
}

pub fn default_form_values(
    action: &NodeUiAction,
    selected_network: Option<&str>,
) -> HashMap<String, String> {
    action
        .fields
        .iter()
        .map(|field| {
            let value = match (action.id.as_str(), field.id.as_str()) {
                (_, "signaling_port") => "6666".into(),
                (_, "relay_port") => "6667".into(),
                (_, "timeout_ms") => "1500".into(),
                ("join_network", "start") => "true".into(),
                ("create_network", "auto_connect") => "true".into(),
                ("start_networks", "selection") => "default".into(),
                ("stop_networks", "networks") => selected_network.unwrap_or_default().into(),
                ("stop_networks", "all") => "false".into(),
                ("restart_networks", "networks") => selected_network.unwrap_or_default().into(),
                ("restart_networks", "all") => "false".into(),
                ("cleanup_node_residuals", "networks") => {
                    selected_network.unwrap_or_default().into()
                }
                ("cleanup_node_residuals", "all") => "false".into(),
                ("update_routing", "mode") => "mesh_only".into(),
                ("update_routing", "is_exit_node") => "false".into(),
                ("update_network_allocation", "mode") => "dual_stack".into(),
                ("update_network_allocation", "ipv4_pool") => "10.8.0.0/24".into(),
                ("update_network_allocation", "ipv6_pool") => "fd00::/120".into(),
                ("set_exit_node_role", "role") => "exit_node".into(),
                ("set_relay_role", "role") => "relay".into(),
                (_, "enabled") => "true".into(),
                (_, "network_name") => selected_network.unwrap_or_default().into(),
                (_, "network_selection") => "default".into(),
                (_, "role") => "member".into(),
                (_, "mode") => "mesh_only".into(),
                (_, "selection") => "default".into(),
                _ if field.input == crate::node::NodeUiInputKind::Boolean => "false".into(),
                _ => String::new(),
            };
            (field.id.clone(), value)
        })
        .collect()
}

pub fn build_command(
    action: &NodeUiAction,
    values: &HashMap<String, String>,
) -> Result<NodeCommand, CommandBuildError> {
    let field = |id: &str| values.get(id).cloned().unwrap_or_default();
    let required = |id: &str| {
        values
            .get(id)
            .cloned()
            .ok_or_else(|| CommandBuildError::MissingField {
                action_id: action.id.clone(),
                field_id: id.into(),
            })
    };

    match bind_action(action) {
        Ok(ActionBinding::RefreshSnapshot) => Ok(NodeCommand::GetSnapshot),
        Ok(ActionBinding::GetDiagnostics) => Ok(NodeCommand::GetDiagnostics),
        Ok(ActionBinding::CreateNetwork) => {
            Ok(NodeCommand::CreateNetwork(NodeCreateNetworkCommand {
                name: field("name"),
                signaling_port: parse_number(
                    action,
                    "signaling_port",
                    &required("signaling_port")?,
                )?,
                relay_port: parse_number(action, "relay_port", &required("relay_port")?)?,
                external_host: optional_non_empty(field("external_host")),
                auto_connect: parse_bool(action, "auto_connect", &required("auto_connect")?)?,
            }))
        }
        Ok(ActionBinding::JoinNetwork) => Ok(NodeCommand::JoinNetwork(NodeJoinNetworkCommand {
            target: field("target"),
            network_name: optional_non_empty(field("network_name")),
            start: parse_bool(action, "start", &required("start")?)?,
        })),
        Ok(ActionBinding::StartNetworks) => Ok(NodeCommand::StartNetworks {
            selection: parse_selection(action, "selection", &required("selection")?)?,
        }),
        Ok(ActionBinding::StopNetworks) => Ok(NodeCommand::StopNetworks {
            networks: split_names(&field("networks")),
            all: parse_bool(action, "all", &field("all"))?,
        }),
        Ok(ActionBinding::RestartNetworks) => Ok(NodeCommand::RestartNetworks {
            networks: split_names(&field("networks")),
            all: parse_bool(action, "all", &field("all"))?,
        }),
        Ok(ActionBinding::ProbePeer) => Ok(NodeCommand::ProbePeer(NodeProbePeerCommand {
            network_name: field("network_name"),
            target: field("target"),
            timeout_ms: parse_number(action, "timeout_ms", &required("timeout_ms")?)?,
        })),
        Ok(ActionBinding::GetBlacklist) => Ok(NodeCommand::GetBlacklist {
            network_name: field("network_name"),
        }),
        Ok(ActionBinding::BanClient) => Ok(NodeCommand::BanClient(NodeClientBlacklistCommand {
            network_name: field("network_name"),
            client_id: field("client_id"),
        })),
        Ok(ActionBinding::UnbanClient) => {
            Ok(NodeCommand::UnbanClient(NodeClientBlacklistCommand {
                network_name: field("network_name"),
                client_id: field("client_id"),
            }))
        }
        Ok(ActionBinding::GetNetworkAllocation) => Ok(NodeCommand::GetNetworkAllocation {
            network_name: field("network_name"),
        }),
        Ok(ActionBinding::UpdateNetworkAllocation) => Ok(NodeCommand::UpdateNetworkAllocation(
            NodeUpdateNetworkAllocationCommand {
                network_name: field("network_name"),
                allocation: NodeNetworkAllocation {
                    mode: parse_network_mode(action, "mode", &required("mode")?)?,
                    ipv4_pool: field("ipv4_pool"),
                    ipv6_pool: field("ipv6_pool"),
                },
            },
        )),
        Ok(ActionBinding::UpdateRouting) => {
            Ok(NodeCommand::UpdateRouting(NodeUpdateRoutingCommand {
                network_name: field("network_name"),
                mode: parse_routing_mode(action, "mode", &required("mode")?)?,
                exit_node: optional_non_empty(field("exit_node")),
                is_exit_node: parse_bool(action, "is_exit_node", &required("is_exit_node")?)?,
            }))
        }
        Ok(ActionBinding::SetExitNodeRole | ActionBinding::SetRelayRole) => {
            Ok(NodeCommand::SetNetworkRole(NodeSetNetworkRoleCommand {
                network_name: field("network_name"),
                role: parse_role(action, "role", &required("role")?)?,
                enabled: parse_bool(action, "enabled", &required("enabled")?)?,
            }))
        }
        Ok(ActionBinding::StartNode) => Ok(NodeCommand::StartNode),
        Ok(ActionBinding::StopNode) => Ok(NodeCommand::StopNode),
        Ok(ActionBinding::RestartNode) => Ok(NodeCommand::RestartNode),
        Ok(ActionBinding::CleanupResiduals) => Ok(NodeCommand::CleanupResiduals {
            networks: split_names(&field("networks")),
            all: parse_bool(action, "all", &field("all"))?,
        }),
        Ok(ActionBinding::ShutdownNode) => Ok(NodeCommand::Shutdown),
        Err(ModelError::UnknownAction { action_id, .. }) => {
            Err(CommandBuildError::UnknownAction { action_id })
        }
        Err(error) => Err(CommandBuildError::InvalidField {
            action_id: action.id.clone(),
            field_id: "<manifest>".into(),
            value: error.to_string(),
        }),
    }
}

pub fn validate_form_contract(action: &NodeUiAction) -> Result<(), String> {
    let allowed = match action.id.as_str() {
        "refresh_snapshot" => HashSet::from([]),
        "get_node_diagnostics" => HashSet::from([]),
        "create_network" => HashSet::from([
            "name",
            "signaling_port",
            "relay_port",
            "external_host",
            "auto_connect",
        ]),
        "join_network" => HashSet::from(["target", "network_name", "start"]),
        "start_networks" => HashSet::from(["selection"]),
        "stop_networks" => HashSet::from(["networks", "all"]),
        "restart_networks" => HashSet::from(["networks", "all"]),
        "cleanup_node_residuals" => HashSet::from(["networks", "all"]),
        "probe_peer" => HashSet::from(["network_name", "target", "timeout_ms"]),
        "get_blacklist" => HashSet::from(["network_name"]),
        "ban_client" | "unban_client" => HashSet::from(["network_name", "client_id"]),
        "get_network_allocation" => HashSet::from(["network_name"]),
        "update_network_allocation" => {
            HashSet::from(["network_name", "mode", "ipv4_pool", "ipv6_pool"])
        }
        "update_routing" => HashSet::from(["network_name", "mode", "exit_node", "is_exit_node"]),
        "set_exit_node_role" | "set_relay_role" => {
            HashSet::from(["network_name", "role", "enabled"])
        }
        "start_node" | "stop_node" | "restart_node" | "shutdown_node" => HashSet::from([]),
        _ => return Err(format!("unknown action {}", action.id)),
    };
    let actual = action
        .fields
        .iter()
        .map(|field| field.id.as_str())
        .collect::<HashSet<_>>();
    if actual == allowed {
        Ok(())
    } else {
        Err(format!(
            "action {} field contract mismatch: expected={allowed:?}, actual={actual:?}",
            action.id
        ))
    }
}

fn parse_number<T>(
    action: &NodeUiAction,
    field_id: &str,
    value: &str,
) -> Result<T, CommandBuildError>
where
    T: std::str::FromStr,
{
    value
        .trim()
        .parse()
        .map_err(|_| CommandBuildError::InvalidField {
            action_id: action.id.clone(),
            field_id: field_id.into(),
            value: value.into(),
        })
}

fn parse_bool(
    action: &NodeUiAction,
    field_id: &str,
    value: &str,
) -> Result<bool, CommandBuildError> {
    match value.trim() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" | "" => Ok(false),
        _ => Err(CommandBuildError::InvalidField {
            action_id: action.id.clone(),
            field_id: field_id.into(),
            value: value.into(),
        }),
    }
}

fn parse_selection(
    action: &NodeUiAction,
    field_id: &str,
    value: &str,
) -> Result<NodeNetworkSelection, CommandBuildError> {
    match value.trim() {
        "" | "none" => Ok(NodeNetworkSelection::None),
        "default" => Ok(NodeNetworkSelection::Default),
        "all" => Ok(NodeNetworkSelection::All),
        names => {
            let names = split_names(names);
            if names.is_empty() {
                Err(CommandBuildError::InvalidField {
                    action_id: action.id.clone(),
                    field_id: field_id.into(),
                    value: value.into(),
                })
            } else {
                Ok(NodeNetworkSelection::Named(names))
            }
        }
    }
}

fn parse_routing_mode(
    action: &NodeUiAction,
    field_id: &str,
    value: &str,
) -> Result<NodeRoutingMode, CommandBuildError> {
    match value.trim() {
        "mesh_only" | "mesh-only" | "mesh" => Ok(NodeRoutingMode::MeshOnly),
        "bypass_mainland" | "bypass-mainland" | "bypass" => Ok(NodeRoutingMode::BypassMainland),
        "global_proxy" | "global-proxy" | "global" => Ok(NodeRoutingMode::GlobalProxy),
        _ => Err(CommandBuildError::InvalidField {
            action_id: action.id.clone(),
            field_id: field_id.into(),
            value: value.into(),
        }),
    }
}

fn parse_network_mode(
    action: &NodeUiAction,
    field_id: &str,
    value: &str,
) -> Result<NetworkMode, CommandBuildError> {
    match value.trim() {
        "ipv4_only" | "ipv4-only" | "ipv4" => Ok(NetworkMode::Ipv4Only),
        "ipv6_only" | "ipv6-only" | "ipv6" => Ok(NetworkMode::Ipv6Only),
        "dual_stack" | "dual-stack" | "dual" => Ok(NetworkMode::DualStack),
        _ => Err(CommandBuildError::InvalidField {
            action_id: action.id.clone(),
            field_id: field_id.into(),
            value: value.into(),
        }),
    }
}

fn parse_role(
    action: &NodeUiAction,
    field_id: &str,
    value: &str,
) -> Result<NodeNetworkRole, CommandBuildError> {
    match value.trim() {
        "member" => Ok(NodeNetworkRole::Member),
        "discovery" => Ok(NodeNetworkRole::Discovery),
        "relay" => Ok(NodeNetworkRole::Relay),
        "exit_node" | "exit-node" | "exit" => Ok(NodeNetworkRole::ExitNode),
        _ => Err(CommandBuildError::InvalidField {
            action_id: action.id.clone(),
            field_id: field_id.into(),
            value: value.into(),
        }),
    }
}

fn optional_non_empty(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn split_names(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

fn project_page(
    kind: PageKind,
    snapshot: &NodeSnapshot,
    locale: Locale,
) -> (Vec<MetricModel>, Vec<SectionModel>) {
    match kind {
        PageKind::Overview => project_overview(snapshot, locale),
        PageKind::Networks => project_networks(snapshot, locale),
        PageKind::Peers => project_peers(snapshot, locale),
        PageKind::Topology => project_topology(snapshot, locale),
        PageKind::Traffic => project_traffic(snapshot, locale),
        PageKind::Routing => project_routing(snapshot, locale),
        PageKind::ExitNode => project_exit_node(snapshot, locale),
        PageKind::Relay => project_relay(snapshot, locale),
        PageKind::Logs => project_logs(snapshot, locale),
        PageKind::Identity => project_identity(snapshot, locale),
        PageKind::Settings => project_settings(snapshot, locale),
    }
}

fn metric(label: &str, value: impl ToString, source: &str) -> MetricModel {
    MetricModel {
        label: label.into(),
        value: value.to_string(),
        source: source.into(),
    }
}

fn section(
    title: impl Into<String>,
    empty_text: impl Into<String>,
    rows: Vec<RowModel>,
) -> SectionModel {
    SectionModel {
        title: title.into(),
        rows,
        empty_text: empty_text.into(),
    }
}

fn row(title: impl Into<String>, detail: impl Into<String>) -> RowModel {
    RowModel {
        title: title.into(),
        detail: detail.into(),
    }
}

fn project_overview(
    snapshot: &NodeSnapshot,
    locale: Locale,
) -> (Vec<MetricModel>, Vec<SectionModel>) {
    let primary_status = snapshot.primary_status();
    let identity = snapshot
        .identity
        .as_ref()
        .map(|identity| {
            format!(
                "{} · {}",
                identity
                    .display_name
                    .clone()
                    .unwrap_or_else(|| identity.node_id.clone()),
                identity.public_key_fingerprint
            )
        })
        .unwrap_or_else(|| t(locale, "gpui.empty.node_identity").into());
    let problem = snapshot
        .last_error
        .as_ref()
        .map(|problem| {
            format!(
                "{:?}: {}",
                problem.code,
                t(locale, problem.code.message_key())
            )
        })
        .unwrap_or_else(|| t(locale, "gpui.empty.node_problem").into());
    (
        vec![
            metric(
                t(locale, "gpui.metric.networks"),
                snapshot.networks.len(),
                "NodeSnapshot.networks",
            ),
            metric(
                t(locale, "gpui.metric.peers"),
                snapshot.peers.len(),
                "NodeSnapshot.peers",
            ),
            metric(
                t(locale, "gpui.metric.revision"),
                snapshot.revision,
                "NodeSnapshot.revision",
            ),
        ],
        vec![
            section(
                t(locale, "common.runtime_status"),
                t(locale, primary_status.detail_key()),
                vec![row(
                    t(locale, primary_status.label_key()),
                    t(locale, primary_status.detail_key()),
                )],
            ),
            section(
                t(locale, "gpui.section.node_identity"),
                t(locale, "gpui.empty.node_identity"),
                vec![row(t(locale, "gpui.row.current_identity"), identity)],
            ),
            section(
                t(locale, "gpui.section.last_problem"),
                t(locale, "gpui.empty.node_problem"),
                vec![row("last_error", problem)],
            ),
        ],
    )
}

fn project_networks(
    snapshot: &NodeSnapshot,
    locale: Locale,
) -> (Vec<MetricModel>, Vec<SectionModel>) {
    let rows = snapshot
        .networks
        .iter()
        .map(|network| {
            let status = t(locale, network_status_key(network.status));
            let ipv6 = display_option(locale, network.assigned_ipv6);
            let ipv4 = display_option(locale, network.assigned_ipv4);
            let peers = network.peer_count.to_string();
            let uptime = network.uptime_secs.to_string();
            let roles = display_roles(locale, &network.roles);
            row(
                network.name.clone(),
                tr(
                    locale,
                    "gpui.network_run_detail",
                    &[
                        ("status", status),
                        ("tun", &network.tun_name),
                        ("ipv6", &ipv6),
                        ("ipv4", &ipv4),
                        ("peers", &peers),
                        ("uptime", &uptime),
                        ("roles", &roles),
                    ],
                ),
            )
        })
        .collect::<Vec<_>>();
    let config_rows = snapshot
        .configuration
        .networks
        .iter()
        .map(|network| {
            let mtu = network.mtu.to_string();
            let port = network.p2p_listen_port.to_string();
            let routing = t(locale, routing_mode_key(network.routing_mode));
            let exit = display_optional_text(locale, network.exit_node.as_deref());
            row(
                network.network_name.clone(),
                tr(
                    locale,
                    "gpui.network_config_detail",
                    &[
                        ("coordinator", &network.coordinator_url),
                        ("tun", &network.tun_name),
                        ("mtu", &mtu),
                        ("port", &port),
                        ("routing", routing),
                        ("exit", &exit),
                    ],
                ),
            )
        })
        .collect::<Vec<_>>();
    (
        vec![metric(
            t(locale, "gpui.metric.running_networks"),
            snapshot.networks.len(),
            "NodeSnapshot.networks",
        )],
        vec![
            section(
                t(locale, "gpui.section.running_networks"),
                t(locale, "gpui.empty.running_networks"),
                rows,
            ),
            section(
                t(locale, "gpui.section.saved_config"),
                t(locale, "gpui.empty.saved_config"),
                config_rows,
            ),
        ],
    )
}

fn project_peers(snapshot: &NodeSnapshot, locale: Locale) -> (Vec<MetricModel>, Vec<SectionModel>) {
    let rows = snapshot
        .peers
        .iter()
        .map(|peer| {
            let online = display_bool(locale, peer.online);
            let p2p = display_bool(locale, peer.p2p_active);
            let healthy = display_bool(locale, peer.p2p_healthy);
            let rtt = peer
                .rtt_us
                .map(|rtt| format!("{rtt}us"))
                .unwrap_or_else(|| t(locale, "common.not_available").into());
            let loss = format!("{:.3}", peer.loss_rate);
            let endpoints = format!("{:?}", peer.endpoints);
            row(
                format!("{} / {}", peer.network_name, peer.peer_id),
                tr(
                    locale,
                    "gpui.peer_detail",
                    &[
                        ("address", &peer.virtual_ipv6.to_string()),
                        ("online", online),
                        ("p2p", p2p),
                        ("healthy", healthy),
                        ("rtt", &rtt),
                        ("loss", &loss),
                        ("endpoints", &endpoints),
                    ],
                ),
            )
        })
        .collect::<Vec<_>>();
    (
        vec![
            metric(
                t(locale, "gpui.metric.visible_peers"),
                snapshot.peers.len(),
                "NodeSnapshot.peers",
            ),
            metric(
                t(locale, "gpui.metric.p2p_healthy"),
                snapshot
                    .peers
                    .iter()
                    .filter(|peer| peer.p2p_healthy)
                    .count(),
                "NodePeerSnapshot.p2p_healthy",
            ),
        ],
        vec![section(
            t(locale, "gpui.section.peer_list"),
            t(locale, "gpui.empty.peers"),
            rows,
        )],
    )
}

fn project_topology(
    snapshot: &NodeSnapshot,
    locale: Locale,
) -> (Vec<MetricModel>, Vec<SectionModel>) {
    let sections = snapshot
        .networks
        .iter()
        .map(|network| {
            let rows = snapshot
                .peers
                .iter()
                .filter(|peer| peer.network_name == network.name)
                .map(|peer| {
                    let transport = if peer.p2p_active {
                        t(locale, "peer.transport.p2p")
                    } else {
                        t(locale, "gpui.value.p2p_unknown")
                    };
                    row(
                        peer.peer_id.clone(),
                        tr(
                            locale,
                            "gpui.topology_peer_detail",
                            &[
                                ("address", &peer.virtual_ipv6.to_string()),
                                ("transport", transport),
                            ],
                        ),
                    )
                })
                .collect::<Vec<_>>();
            section(
                tr(
                    locale,
                    "gpui.network_status_title",
                    &[
                        ("network", &network.name),
                        ("status", t(locale, network_status_key(network.status))),
                    ],
                ),
                t(locale, "gpui.empty.network_peers"),
                rows,
            )
        })
        .collect::<Vec<_>>();
    (
        vec![metric(
            t(locale, "gpui.metric.topology_edges"),
            snapshot.peers.len(),
            "network-scoped peers",
        )],
        if sections.is_empty() {
            vec![section(
                t(locale, "gpui.section.topology"),
                t(locale, "gpui.empty.topology"),
                Vec::new(),
            )]
        } else {
            sections
        },
    )
}

fn project_traffic(
    snapshot: &NodeSnapshot,
    locale: Locale,
) -> (Vec<MetricModel>, Vec<SectionModel>) {
    let network_rows = snapshot
        .networks
        .iter()
        .map(|network| {
            let traffic = &network.traffic;
            row(
                network.name.clone(),
                tr(
                    locale,
                    "gpui.traffic_detail",
                    &[
                        ("rx_packets", &traffic.tun_rx_packets.to_string()),
                        ("rx_bytes", &traffic.tun_rx_bytes.to_string()),
                        ("tx_packets", &traffic.tun_tx_packets.to_string()),
                        ("tx_bytes", &traffic.tun_tx_bytes.to_string()),
                        ("p2p_rx_packets", &traffic.p2p_rx_packets.to_string()),
                        ("p2p_rx_bytes", &traffic.p2p_rx_bytes.to_string()),
                        ("p2p_tx_packets", &traffic.p2p_tx_packets.to_string()),
                        ("p2p_tx_bytes", &traffic.p2p_tx_bytes.to_string()),
                        ("errors", &traffic.error_count.to_string()),
                    ],
                ),
            )
        })
        .collect::<Vec<_>>();
    let total = snapshot.networks.iter().fold(0_u64, |sum, network| {
        sum.saturating_add(network.traffic.tun_rx_bytes)
            .saturating_add(network.traffic.tun_tx_bytes)
            .saturating_add(network.traffic.p2p_rx_bytes)
            .saturating_add(network.traffic.p2p_tx_bytes)
    });
    (
        vec![
            metric(
                t(locale, "gpui.metric.total_bytes"),
                total,
                "NodeTrafficSnapshot",
            ),
            metric(
                t(locale, "gpui.metric.error_count"),
                snapshot
                    .networks
                    .iter()
                    .map(|network| network.traffic.error_count)
                    .fold(0_u64, u64::saturating_add),
                "NodeTrafficSnapshot.error_count",
            ),
        ],
        vec![section(
            t(locale, "gpui.section.by_network"),
            t(locale, "gpui.empty.traffic"),
            network_rows,
        )],
    )
}

fn project_routing(
    snapshot: &NodeSnapshot,
    locale: Locale,
) -> (Vec<MetricModel>, Vec<SectionModel>) {
    let rows = snapshot
        .configuration
        .networks
        .iter()
        .map(|network| {
            let routing = t(locale, routing_mode_key(network.routing_mode));
            let exit = display_optional_text(locale, network.exit_node.as_deref());
            let direct = format!("{:?}", network.direct_cidrs);
            let proxy = format!("{:?}", network.proxy_cidrs);
            let enabled = display_bool(locale, network.exit_node_enabled);
            let allowed_clients = format!("{:?}", network.exit_node_allowed_clients);
            let allow_lan = display_bool(locale, network.exit_node_allow_lan_access);
            let region = display_optional_text(locale, network.exit_node_region.as_deref());
            row(
                network.network_name.clone(),
                tr(
                    locale,
                    "gpui.routing_detail",
                    &[
                        ("routing", routing),
                        ("exit", &exit),
                        ("direct", &direct),
                        ("proxy", &proxy),
                        ("enabled", enabled),
                        ("allowed_clients", &allowed_clients),
                        ("allow_lan", allow_lan),
                        ("region", &region),
                    ],
                ),
            )
        })
        .collect::<Vec<_>>();
    (
        vec![
            metric(
                t(locale, "gpui.metric.configured_networks"),
                snapshot.configuration.networks.len(),
                "NodeConfigurationSnapshot",
            ),
            metric(
                t(locale, "gpui.metric.exit_enabled"),
                snapshot
                    .configuration
                    .networks
                    .iter()
                    .filter(|network| network.exit_node_enabled)
                    .count(),
                "exit_node_enabled",
            ),
        ],
        vec![section(
            t(locale, "gpui.section.routing_config"),
            t(locale, "gpui.empty.routing"),
            rows,
        )],
    )
}

fn project_exit_node(
    snapshot: &NodeSnapshot,
    locale: Locale,
) -> (Vec<MetricModel>, Vec<SectionModel>) {
    let capabilities = &snapshot.exit_node_capabilities;
    let capability_rows = vec![row(
        t(locale, "exit_node.row.platform"),
        tr(
            locale,
            "gpui.exit_capability_detail",
            &[
                (
                    "platform",
                    exit_node_platform_label(locale, capabilities.platform),
                ),
                ("supported", display_bool(locale, capabilities.supported)),
                (
                    "ipv4_forwarding",
                    display_bool(locale, capabilities.ipv4_forwarding),
                ),
                (
                    "ipv6_forwarding",
                    display_bool(locale, capabilities.ipv6_forwarding),
                ),
                ("nat", display_bool(locale, capabilities.nat)),
                ("cleanup", display_bool(locale, capabilities.cleanup)),
            ],
        ),
    )];
    let unsupported_rows = (!capabilities.unsupported_reasons.is_empty())
        .then(|| {
            row(
                t(locale, "gpui.row.unsupported_reasons"),
                capabilities
                    .unsupported_reasons
                    .iter()
                    .map(|reason| {
                        format!(
                            "{reason:?}: {}",
                            t(locale, exit_node_unsupported_reason_key(*reason))
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        })
        .into_iter()
        .collect::<Vec<_>>();
    let rows = snapshot
        .configuration
        .networks
        .iter()
        .map(|network| {
            let enabled = display_bool(locale, network.exit_node_enabled);
            let region = display_optional_text(locale, network.exit_node_region.as_deref());
            let allow_lan = display_bool(locale, network.exit_node_allow_lan_access);
            let allowed_clients = format!("{:?}", network.exit_node_allowed_clients);
            let selected_exit = display_optional_text(locale, network.exit_node.as_deref());
            row(
                network.network_name.clone(),
                tr(
                    locale,
                    "gpui.exit_config_detail",
                    &[
                        ("enabled", enabled),
                        ("region", &region),
                        ("allow_lan", allow_lan),
                        ("allowed_clients", &allowed_clients),
                        ("selected_exit", &selected_exit),
                    ],
                ),
            )
        })
        .collect::<Vec<_>>();
    let role_rows = snapshot
        .networks
        .iter()
        .filter(|network| network.roles.contains(&NodeNetworkRole::ExitNode))
        .map(|network| row(network.name.clone(), t(locale, "gpui.value.exit_role")))
        .collect::<Vec<_>>();
    (
        vec![
            metric(
                t(locale, "gpui.metric.exit_configs"),
                rows.len(),
                "exit_node_enabled",
            ),
            metric(
                t(locale, "gpui.metric.exit_roles"),
                role_rows.len(),
                "NodeNetworkRole::ExitNode",
            ),
        ],
        vec![
            section(
                t(locale, "gpui.section.platform_capabilities"),
                t(locale, "gpui.empty.exit_capabilities"),
                capability_rows,
            ),
            section(
                t(locale, "gpui.section.capability_limits"),
                t(locale, "gpui.empty.capability_limits"),
                unsupported_rows,
            ),
            section(
                t(locale, "gpui.section.exit_config"),
                t(locale, "gpui.empty.exit_config"),
                rows,
            ),
            section(
                t(locale, "gpui.section.current_roles"),
                t(locale, "gpui.empty.exit_roles"),
                role_rows,
            ),
        ],
    )
}

fn exit_node_platform_label(
    locale: Locale,
    platform: fuckipv6_common::ExitNodePlatform,
) -> &'static str {
    use fuckipv6_common::ExitNodePlatform;

    match platform {
        ExitNodePlatform::Linux => "Linux",
        ExitNodePlatform::Windows => "Windows",
        ExitNodePlatform::Macos => "macOS",
        ExitNodePlatform::Other => t(locale, "gpui.platform.other"),
    }
}

fn exit_node_unsupported_reason_key(
    reason: fuckipv6_common::ExitNodeUnsupportedReason,
) -> &'static str {
    use fuckipv6_common::ExitNodeUnsupportedReason;

    match reason {
        ExitNodeUnsupportedReason::PlatformNotLinux => {
            "exit_node.unsupported_reason.platform_not_linux"
        }
        ExitNodeUnsupportedReason::PrivilegesRequired => {
            "exit_node.unsupported_reason.privileges_required"
        }
        ExitNodeUnsupportedReason::RoutingToolUnavailable => {
            "exit_node.unsupported_reason.routing_tool_unavailable"
        }
        ExitNodeUnsupportedReason::IptablesUnavailable => {
            "exit_node.unsupported_reason.iptables_unavailable"
        }
        ExitNodeUnsupportedReason::Ipv4ForwardingUnavailable => {
            "exit_node.unsupported_reason.ipv4_forwarding_unavailable"
        }
    }
}

fn project_relay(snapshot: &NodeSnapshot, locale: Locale) -> (Vec<MetricModel>, Vec<SectionModel>) {
    let services = snapshot
        .embedded_services
        .iter()
        .map(|service| {
            let status = t(locale, embedded_service_status_key(service.status));
            let signaling = display_option(locale, service.signaling_addr);
            let relay = display_option(locale, service.relay_addr);
            row(
                service.network_name.clone(),
                tr(
                    locale,
                    "gpui.embedded_service_detail",
                    &[
                        ("status", status),
                        ("signal", &signaling),
                        ("relay", &relay),
                    ],
                ),
            )
        })
        .collect::<Vec<_>>();
    let roles = snapshot
        .networks
        .iter()
        .filter(|network| network.roles.contains(&NodeNetworkRole::Relay))
        .map(|network| row(network.name.clone(), t(locale, "gpui.value.relay_role")))
        .collect::<Vec<_>>();
    (
        vec![
            metric(
                t(locale, "gpui.metric.embedded_services"),
                snapshot.embedded_services.len(),
                "NodeSnapshot.embedded_services",
            ),
            metric(
                t(locale, "gpui.metric.relay_roles"),
                roles.len(),
                "NodeNetworkRole::Relay",
            ),
        ],
        vec![
            section(
                t(locale, "gpui.section.embedded_services"),
                t(locale, "gpui.empty.embedded_services"),
                services,
            ),
            section(
                t(locale, "gpui.section.current_roles"),
                t(locale, "gpui.empty.relay_roles"),
                roles,
            ),
        ],
    )
}

fn project_logs(snapshot: &NodeSnapshot, locale: Locale) -> (Vec<MetricModel>, Vec<SectionModel>) {
    let rows = snapshot
        .logs
        .iter()
        .map(|entry| {
            row(
                format!("{} · {:?} · {}", entry.unix_ms, entry.level, entry.target),
                entry.message.clone(),
            )
        })
        .collect::<Vec<_>>();
    (
        vec![metric(
            t(locale, "gpui.metric.log_buffer"),
            snapshot.logs.len(),
            "NodeSnapshot.logs",
        )],
        vec![section(
            t(locale, "gpui.section.bounded_logs"),
            t(locale, "gpui.empty.logs"),
            rows,
        )],
    )
}

fn project_identity(
    snapshot: &NodeSnapshot,
    locale: Locale,
) -> (Vec<MetricModel>, Vec<SectionModel>) {
    let identity_rows = snapshot
        .identity
        .as_ref()
        .map(|identity| {
            vec![
                row(
                    t(locale, "identity.row.display_name"),
                    identity
                        .display_name
                        .clone()
                        .unwrap_or_else(|| t(locale, "gpui.value.unset").into()),
                ),
                row(t(locale, "identity.row.node_id"), identity.node_id.clone()),
                row(
                    t(locale, "identity.row.fingerprint"),
                    identity.public_key_fingerprint.clone(),
                ),
            ]
        })
        .unwrap_or_default();
    let config_rows = snapshot
        .configuration
        .networks
        .iter()
        .map(|network| {
            row(
                network.network_name.clone(),
                tr(
                    locale,
                    "gpui.identity_config_detail",
                    &[(
                        "configured",
                        display_bool(locale, network.identity_configured),
                    )],
                ),
            )
        })
        .collect::<Vec<_>>();
    (
        vec![metric(
            t(locale, "gpui.metric.identity_status"),
            if snapshot.identity.is_some() {
                t(locale, "gpui.value.published")
            } else {
                t(locale, "gpui.value.unpublished")
            },
            "NodeSnapshot.identity",
        )],
        vec![
            section(
                t(locale, "gpui.section.public_identity"),
                t(locale, "gpui.empty.node_identity"),
                identity_rows,
            ),
            section(
                t(locale, "gpui.section.network_identity_config"),
                t(locale, "gpui.empty.identity_config"),
                config_rows,
            ),
            section(
                t(locale, "owner_auth.identity.section_title"),
                t(locale, "owner_auth.identity.section_empty"),
                vec![
                    row(
                        t(locale, "owner_auth.local_identity.title"),
                        t(locale, "owner_auth.local_identity.detail"),
                    ),
                    row(
                        t(locale, "owner_auth.web_identity.title"),
                        t(locale, "owner_auth.web_identity.detail"),
                    ),
                    row(
                        t(locale, "owner_auth.recovery_row.title"),
                        t(locale, "owner_auth.passkey_recovery.help"),
                    ),
                ],
            ),
        ],
    )
}

fn project_settings(
    snapshot: &NodeSnapshot,
    locale: Locale,
) -> (Vec<MetricModel>, Vec<SectionModel>) {
    let permission_rows = snapshot
        .permissions
        .permissions
        .iter()
        .map(|permission| {
            row(
                t(locale, permission_key(*permission)),
                t(locale, "gpui.value.granted"),
            )
        })
        .collect::<Vec<_>>();
    let config_rows = snapshot
        .configuration
        .networks
        .iter()
        .map(|network| {
            let log_file = display_bool(locale, network.log_file_configured);
            let acl_enabled = display_bool(locale, network.acl_enabled);
            let acl_default = t(locale, acl_action_key(network.acl_default_action));
            let acl_rules = network.acl_rule_count.to_string();
            row(
                network.network_name.clone(),
                tr(
                    locale,
                    "gpui.settings_network_detail",
                    &[
                        ("log_level", &network.log_level),
                        ("log_file", log_file),
                        ("acl_enabled", acl_enabled),
                        ("acl_default", acl_default),
                        ("acl_rules", &acl_rules),
                    ],
                ),
            )
        })
        .collect::<Vec<_>>();
    let service_rows = snapshot
        .embedded_services
        .iter()
        .map(|service| {
            let status = t(locale, embedded_service_status_key(service.status));
            let signaling = display_option(locale, service.signaling_addr);
            let relay = display_option(locale, service.relay_addr);
            row(
                service.network_name.clone(),
                tr(
                    locale,
                    "gpui.embedded_service_detail",
                    &[
                        ("status", status),
                        ("signal", &signaling),
                        ("relay", &relay),
                    ],
                ),
            )
        })
        .collect::<Vec<_>>();
    (
        vec![
            metric(
                t(locale, "gpui.metric.node_phase"),
                t(locale, phase_key(snapshot.lifecycle.phase)),
                "NodeLifecycleSnapshot.phase",
            ),
            metric(
                t(locale, "gpui.metric.access_model"),
                t(locale, access_model_key(snapshot.permissions.access_model)),
                "NodePermissionSnapshot.access_model",
            ),
            metric(
                t(locale, "gpui.metric.ui_mode"),
                snapshot
                    .ui_mode
                    .as_ref()
                    .map(|mode| t(locale, ui_mode_key(mode.effective)))
                    .unwrap_or_else(|| t(locale, "common.unknown")),
                "NodeSnapshot.ui_mode",
            ),
        ],
        vec![
            section(
                t(locale, "gpui.section.permission_snapshot"),
                t(locale, "gpui.empty.permissions"),
                permission_rows,
            ),
            section(
                t(locale, "gpui.section.network_settings"),
                t(locale, "gpui.empty.network_settings"),
                config_rows,
            ),
            section(
                t(locale, "gpui.section.embedded_services"),
                t(locale, "gpui.empty.embedded_services"),
                service_rows,
            ),
        ],
    )
}

fn network_status_key(status: NodeNetworkStatus) -> &'static str {
    match status {
        NodeNetworkStatus::Offline => "status.network.offline",
        NodeNetworkStatus::Connecting => "status.network.connecting",
        NodeNetworkStatus::Online => "status.network.online",
        NodeNetworkStatus::Degraded => "status.network.degraded",
        NodeNetworkStatus::Stopping => "status.network.stopping",
        NodeNetworkStatus::Failed => "status.network.failed",
    }
}

fn embedded_service_status_key(status: NodeEmbeddedServiceStatus) -> &'static str {
    match status {
        NodeEmbeddedServiceStatus::Starting => "status.service.starting",
        NodeEmbeddedServiceStatus::Running => "status.service.running",
        NodeEmbeddedServiceStatus::Stopping => "status.service.stopping",
        NodeEmbeddedServiceStatus::Stopped => "status.service.stopped",
        NodeEmbeddedServiceStatus::Failed => "status.service.failed",
    }
}

fn role_key(role: NodeNetworkRole) -> &'static str {
    match role {
        NodeNetworkRole::Member => "choice.role.member",
        NodeNetworkRole::Discovery => "choice.role.discovery",
        NodeNetworkRole::Relay => "choice.role.relay",
        NodeNetworkRole::ExitNode => "choice.role.exit_node",
    }
}

fn routing_mode_key(mode: NodeRoutingMode) -> &'static str {
    match mode {
        NodeRoutingMode::MeshOnly => "choice.routing.mesh_only",
        NodeRoutingMode::BypassMainland => "choice.routing.bypass_mainland",
        NodeRoutingMode::GlobalProxy => "choice.routing.global_proxy",
    }
}

fn permission_key(permission: NodePermission) -> &'static str {
    match permission {
        NodePermission::ViewState => "choice.permission.view_state",
        NodePermission::ManageNetworks => "choice.permission.manage_networks",
        NodePermission::ChangeRouting => "choice.permission.change_routing",
        NodePermission::ManageRoles => "choice.permission.manage_roles",
        NodePermission::ProbePeers => "choice.permission.probe_peers",
        NodePermission::StopNode => "choice.permission.stop_node",
        NodePermission::ViewLogs => "choice.permission.view_logs",
    }
}

fn phase_key(phase: NodePhase) -> &'static str {
    match phase {
        NodePhase::Stopped => "choice.phase.stopped",
        NodePhase::Starting => "choice.phase.starting",
        NodePhase::Running => "choice.phase.running",
        NodePhase::Stopping => "choice.phase.stopping",
        NodePhase::Failed => "choice.phase.failed",
    }
}

fn access_model_key(model: crate::node::NodeAccessModel) -> &'static str {
    match model {
        crate::node::NodeAccessModel::TrustedControlPlane => {
            "choice.access_model.trusted_control_plane"
        }
    }
}

fn ui_mode_key(mode: UiMode) -> &'static str {
    match mode {
        UiMode::Gpui => "choice.ui_mode.gpui",
        UiMode::WebUi => "choice.ui_mode.web_ui",
        UiMode::Headless => "choice.ui_mode.headless",
    }
}

fn acl_action_key(action: fuckipv6_common::AclAction) -> &'static str {
    match action {
        fuckipv6_common::AclAction::Allow => "choice.acl.allow",
        fuckipv6_common::AclAction::Deny => "choice.acl.deny",
    }
}

fn display_bool(locale: Locale, value: bool) -> &'static str {
    t(
        locale,
        if value {
            "common.enabled"
        } else {
            "common.disabled"
        },
    )
}

fn display_roles(locale: Locale, roles: &[NodeNetworkRole]) -> String {
    roles
        .iter()
        .map(|role| t(locale, role_key(*role)))
        .collect::<Vec<_>>()
        .join(t(locale, "common.list_separator"))
}

fn display_optional_text(locale: Locale, value: Option<&str>) -> String {
    value
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| t(locale, "common.none").into())
}

fn display_option<T: std::fmt::Display>(locale: Locale, value: Option<T>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| t(locale, "common.not_available").into())
}

pub fn validate_manifest_bindings(manifest: &NodeUiManifest) -> Result<(), String> {
    let mut bindings = HashSet::new();
    for page in &manifest.pages {
        let _ = PageKind::from_id(&page.id).ok_or_else(|| format!("unknown page {}", page.id))?;
        for action in &page.actions {
            bind_action(action).map_err(|error| error.to_string())?;
            validate_form_contract(action)?;
            bindings.insert(action.id.as_str());
        }
    }

    for binding in ActionBinding::ALL {
        if !bindings.contains(binding.id()) {
            return Err(format!("manifest missing action binding {}", binding.id()));
        }
    }
    Ok(())
}

// These tests intentionally exercise the pure adapter layer without requiring a
// display server. The GPUI widget rendering consumes the same PageModel.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{
        node_ui_manifest, NodeIdentitySnapshot, NodeNetworkSnapshot, NodeNetworkStatus,
        NodePeerSnapshot, NodeTrafficSnapshot, NodeUiField, NodeUiValidation,
    };

    fn populated_snapshot() -> NodeSnapshot {
        let mut snapshot = NodeSnapshot::default();
        snapshot.lifecycle.phase = NodePhase::Running;
        snapshot.identity = Some(NodeIdentitySnapshot {
            node_id: "node-1".into(),
            display_name: Some("desktop".into()),
            public_key_fingerprint: "aa:bb".into(),
        });
        snapshot
            .permissions
            .permissions
            .extend(crate::node::NodePermission::ALL);
        snapshot.networks.push(NodeNetworkSnapshot {
            name: "home".into(),
            tun_name: "fk-home".into(),
            status: NodeNetworkStatus::Online,
            assigned_ipv6: Some("fd00::2".parse().expect("IPv6")),
            assigned_ipv4: None,
            p2p_listen_addr: Some("127.0.0.1:9000".parse().expect("socket")),
            uptime_secs: 42,
            roles: vec![NodeNetworkRole::Member, NodeNetworkRole::ExitNode],
            peer_count: 1,
            traffic: NodeTrafficSnapshot {
                tun_rx_bytes: 10,
                tun_tx_bytes: 20,
                error_count: 1,
                ..NodeTrafficSnapshot::default()
            },
        });
        snapshot.peers.push(NodePeerSnapshot {
            peer_id: "peer-1".into(),
            network_name: "home".into(),
            virtual_ipv6: "fd00::3".parse().expect("IPv6"),
            virtual_ipv4: None,
            endpoints: vec!["127.0.0.1:9001".parse().expect("socket")],
            online: true,
            p2p_active: true,
            p2p_healthy: true,
            rtt_us: Some(1200),
            loss_rate: 0.0,
            traffic: NodeTrafficSnapshot::default(),
        });
        snapshot
            .configuration
            .networks
            .push(crate::node::NodeNetworkConfigSnapshot {
                network_name: "home".into(),
                coordinator_url: "ws://127.0.0.1:6666".into(),
                virtual_mode: None,
                preferred_ipv4: None,
                preferred_ipv6: None,
                virtual_network: None,
                tun_name: "fk-home".into(),
                mtu: 1280,
                p2p_listen_port: 9000,
                p2p_connect_timeout_secs: 5,
                p2p_heartbeat_interval_secs: 30,
                routing_mode: NodeRoutingMode::MeshOnly,
                exit_node: Some("peer-1".into()),
                direct_cidrs: vec!["10.0.0.0/8".into()],
                proxy_cidrs: vec![],
                exit_node_enabled: true,
                exit_node_allowed_clients: vec!["peer-1".into()],
                exit_node_allow_lan_access: false,
                exit_node_region: Some("local".into()),
                acl_enabled: true,
                acl_default_action: fuckipv6_common::AclAction::Deny,
                acl_rule_count: 2,
                log_level: "info".into(),
                log_file_configured: true,
                identity_configured: true,
            });
        snapshot
    }

    #[test]
    fn every_manifest_page_has_a_concrete_projection() {
        let manifest = node_ui_manifest();
        let snapshot = NodeSnapshot::default();
        let kinds = manifest
            .pages
            .iter()
            .map(|page| {
                let model =
                    build_page_model_for_page(page, &snapshot, Locale::ZhCn).expect("page model");
                assert_eq!(model.page_id, page.id);
                assert!(!model.actions.is_empty() || page.actions.is_empty());
                model.kind
            })
            .collect::<HashSet<_>>();
        assert_eq!(kinds, PageKind::ALL.into_iter().collect());
    }

    #[test]
    fn overview_has_one_runtime_status_and_no_raw_phase_metric() {
        let manifest = node_ui_manifest();
        let page = manifest
            .pages
            .iter()
            .find(|page| page.id == "overview")
            .expect("overview page");
        let snapshot = populated_snapshot();
        let model =
            build_page_model_for_page(page, &snapshot, Locale::ZhCn).expect("overview model");

        assert!(!model
            .metrics
            .iter()
            .any(|metric| metric.label == t(Locale::ZhCn, "gpui.metric.node_phase")));
        assert_eq!(
            model.sections[0].title,
            t(Locale::ZhCn, "common.runtime_status")
        );
        assert_eq!(model.sections[0].rows.len(), 1);
        assert_eq!(
            model.sections[0].rows[0].title,
            t(Locale::ZhCn, snapshot.primary_status().label_key())
        );
        assert_eq!(
            model.sections[0].rows[0].detail,
            t(Locale::ZhCn, snapshot.primary_status().detail_key())
        );
    }

    #[test]
    fn identity_explains_local_gpui_and_webui_authentication_boundaries() {
        let manifest = node_ui_manifest();
        let page = manifest
            .pages
            .iter()
            .find(|page| page.id == "identity")
            .expect("identity page");
        let model = build_page_model_for_page(page, &populated_snapshot(), Locale::ZhCn)
            .expect("identity model");
        let section = model
            .sections
            .iter()
            .find(|section| section.title == t(Locale::ZhCn, "owner_auth.identity.section_title"))
            .expect("authentication boundary section");
        assert_eq!(section.rows.len(), 3);
        assert_eq!(
            section.rows[0].detail,
            t(Locale::ZhCn, "owner_auth.local_identity.detail")
        );
        assert_eq!(
            section.rows[1].detail,
            t(Locale::ZhCn, "owner_auth.web_identity.detail")
        );
        assert!(section.rows[2].detail.contains("recover-owner"));
        assert!(!section
            .rows
            .iter()
            .any(|row| row.detail.contains("已登录 WebUI")));
    }

    #[test]
    fn localized_projection_copy_contains_no_hard_coded_han_characters() {
        let source = include_str!("view_model.rs");
        let projections = source
            .split_once("fn project_overview(")
            .and_then(|(_, tail)| tail.split_once("\npub fn validate_manifest_bindings"))
            .map(|(projections, _)| projections)
            .expect("projection source range");
        assert!(
            !projections.chars().any(|character| {
                matches!(
                    character as u32,
                    0x3400..=0x4DBF
                        | 0x4E00..=0x9FFF
                        | 0xF900..=0xFAFF
                        | 0x20000..=0x2FA1F
                )
            }),
            "GPUI projection copy must come from the shared i18n catalog"
        );
    }

    #[test]
    fn english_projections_never_leak_chinese_catalog_copy() {
        let manifest = node_ui_manifest();
        for snapshot in [NodeSnapshot::default(), populated_snapshot()] {
            for page in &manifest.pages {
                let model = build_page_model_for_page(page, &snapshot, Locale::En)
                    .expect("English page model");
                let visible_copy = model
                    .metrics
                    .iter()
                    .map(|metric| metric.label.as_str())
                    .chain(model.sections.iter().flat_map(|section| {
                        std::iter::once(section.title.as_str())
                            .chain(std::iter::once(section.empty_text.as_str()))
                            .chain(section.rows.iter().flat_map(|row| {
                                std::iter::once(row.title.as_str())
                                    .chain(std::iter::once(row.detail.as_str()))
                            }))
                    }));
                for copy in visible_copy {
                    assert!(
                        !copy.chars().any(|character| {
                            matches!(
                                character as u32,
                                0x3400..=0x4DBF
                                    | 0x4E00..=0x9FFF
                                    | 0xF900..=0xFAFF
                                    | 0x20000..=0x2FA1F
                            )
                        }),
                        "English projection for {} leaked Chinese copy: {copy}",
                        page.id
                    );
                }
            }
        }
    }

    #[test]
    fn every_manifest_page_renders_populated_snapshot_fields() {
        let manifest = node_ui_manifest();
        let snapshot = populated_snapshot();
        for page in &manifest.pages {
            let model =
                build_page_model_for_page(page, &snapshot, Locale::ZhCn).expect("page model");
            assert!(
                !model.metrics.is_empty()
                    || model
                        .sections
                        .iter()
                        .any(|section| !section.rows.is_empty()),
                "{} should project snapshot data",
                page.id
            );
        }
    }

    #[test]
    fn every_manifest_action_has_an_explicit_binding() {
        let manifest = node_ui_manifest();
        let actual = manifest
            .pages
            .iter()
            .flat_map(|page| &page.actions)
            .map(|action| bind_action(action).expect("action binding"))
            .collect::<HashSet<_>>();
        assert_eq!(actual, ActionBinding::ALL.into_iter().collect());
        validate_manifest_bindings(&manifest).expect("manifest bindings");
    }

    #[test]
    fn unknown_page_and_action_fail_closed() {
        let mut manifest = node_ui_manifest();
        manifest.pages[0].id = "unknown".into();
        assert!(
            build_page_model(&manifest, &NodeSnapshot::default(), "unknown", Locale::ZhCn).is_err()
        );

        let mut manifest = node_ui_manifest();
        manifest.pages[0].actions[0].id = "unknown_action".into();
        assert!(build_page_model_for_page(
            &manifest.pages[0],
            &NodeSnapshot::default(),
            Locale::ZhCn
        )
        .is_err());
        assert!(validate_manifest_bindings(&manifest).is_err());
    }

    #[test]
    fn action_command_mismatch_fails_closed() {
        let mut action = node_ui_manifest().pages[1].actions[0].clone();
        action.command = NodeCommandKind::Shutdown;
        assert!(matches!(
            bind_action(&action),
            Err(ModelError::ActionCommandMismatch { .. })
        ));
    }

    #[test]
    fn action_builders_use_typed_node_commands() {
        let manifest = node_ui_manifest();
        for page in &manifest.pages {
            for action in &page.actions {
                let values = default_form_values(action, Some("home"));
                let command = build_command(action, &values).expect("typed command");
                assert_ne!(action.id, "");
                assert!(!format!("{command:?}").is_empty());
            }
        }
    }

    #[test]
    fn unknown_action_never_builds_a_command() {
        let mut action = node_ui_manifest().pages[1].actions[0].clone();
        action.id = "injected".into();
        assert!(matches!(
            build_command(&action, &HashMap::new()),
            Err(CommandBuildError::UnknownAction { .. })
        ));
    }

    #[test]
    fn exit_node_capability_blocks_only_exit_role_actions() {
        let manifest = node_ui_manifest();
        let exit_page = manifest
            .pages
            .iter()
            .find(|page| page.id == "exit_node")
            .expect("exit node page");
        let exit_action = exit_page
            .actions
            .iter()
            .find(|action| action.id == "set_exit_node_role")
            .expect("exit role action")
            .clone();
        let relay_action = manifest
            .pages
            .iter()
            .find(|page| page.id == "relay")
            .expect("relay page")
            .actions
            .iter()
            .find(|action| action.id == "set_relay_role")
            .expect("relay role action")
            .clone();

        let mut unsupported = populated_snapshot();
        unsupported.exit_node_capabilities = fuckipv6_common::ExitNodeCapabilities::for_platform(
            fuckipv6_common::ExitNodePlatform::Windows,
        );
        assert_eq!(
            action_block_reason(&exit_action, &unsupported, Some("home")),
            Some(ActionBlockReason::ExitNodeUnsupported)
        );
        assert_eq!(
            action_block_reason(&relay_action, &unsupported, Some("home")),
            None
        );

        let mut supported = populated_snapshot();
        supported.exit_node_capabilities = fuckipv6_common::ExitNodeCapabilities {
            schema_version: fuckipv6_common::EXIT_NODE_CAPABILITY_SCHEMA_VERSION,
            platform: fuckipv6_common::ExitNodePlatform::Linux,
            supported: true,
            ipv4_forwarding: true,
            ipv6_forwarding: true,
            nat: true,
            cleanup: true,
            unsupported_reasons: Vec::new(),
        };
        assert_eq!(
            action_block_reason(&exit_action, &supported, Some("home")),
            None
        );
    }

    #[test]
    fn exit_node_page_projects_platform_capability_and_typed_reasons() {
        let manifest = node_ui_manifest();
        let page = manifest
            .pages
            .iter()
            .find(|page| page.id == "exit_node")
            .expect("exit node page");
        let mut snapshot = populated_snapshot();
        snapshot.exit_node_capabilities = fuckipv6_common::ExitNodeCapabilities::for_platform(
            fuckipv6_common::ExitNodePlatform::Windows,
        );

        let model =
            build_page_model_for_page(page, &snapshot, Locale::ZhCn).expect("exit node model");
        assert!(model.sections[0].rows[0].detail.contains("Windows"));
        assert!(model.sections[0].rows[0]
            .detail
            .contains(t(Locale::ZhCn, "common.disabled")));
        assert!(model.sections[1].rows[0]
            .detail
            .contains("PlatformNotLinux"));
        assert!(model.sections[1].rows[0].detail.contains(t(
            Locale::ZhCn,
            "exit_node.unsupported_reason.platform_not_linux"
        )));
    }

    #[test]
    fn availability_and_permissions_are_derived_from_shared_snapshot() {
        let action = node_ui_manifest().pages[1].actions[3].clone();
        let snapshot = NodeSnapshot::default();
        assert_eq!(
            action_block_reason(&action, &snapshot, None),
            Some(ActionBlockReason::PermissionDenied)
        );

        let mut snapshot = populated_snapshot();
        assert_eq!(action_block_reason(&action, &snapshot, Some("home")), None);
        snapshot.permissions.permissions.clear();
        assert_eq!(
            action_block_reason(&action, &snapshot, Some("home")),
            Some(ActionBlockReason::PermissionDenied)
        );
    }

    #[test]
    fn empty_snapshot_pages_have_explicit_empty_states() {
        let manifest = node_ui_manifest();
        for page in &manifest.pages {
            let model = build_page_model_for_page(page, &NodeSnapshot::default(), Locale::ZhCn)
                .expect("empty page model");
            assert!(
                !model.sections.is_empty(),
                "{} must expose at least one snapshot section",
                page.id
            );
            assert!(
                model
                    .sections
                    .iter()
                    .all(|section| !section.empty_text.is_empty()),
                "{} must have explicit empty-state text",
                page.id
            );
        }
    }

    #[test]
    fn single_and_dual_network_projection_uses_real_network_rows() {
        let manifest = node_ui_manifest();
        let mut snapshot = populated_snapshot();
        let single = build_page_model(&manifest, &snapshot, "networks", Locale::ZhCn)
            .expect("single network");
        let single_section = &single.sections[0];
        assert_eq!(single_section.rows.len(), 1);
        assert!(single_section.rows[0].detail.contains("fd00::2"));

        let mut second = snapshot.networks[0].clone();
        second.name = "office".into();
        second.tun_name = "fk-office".into();
        second.assigned_ipv6 = Some("fd00::4".parse().expect("IPv6"));
        snapshot.networks.push(second);
        let dual =
            build_page_model(&manifest, &snapshot, "networks", Locale::ZhCn).expect("dual network");
        let dual_section = &dual.sections[0];
        assert_eq!(dual_section.rows.len(), 2);
        assert!(dual_section.rows.iter().any(|row| row.title == "home"));
        assert!(dual_section.rows.iter().any(|row| row.title == "office"));
    }

    #[test]
    fn offline_and_permission_failures_block_manifest_actions() {
        let manifest = node_ui_manifest();
        let peers_page = manifest
            .pages
            .iter()
            .find(|page| page.id == "peers")
            .expect("peers page");
        let action = peers_page.actions[0].clone();

        let mut offline = populated_snapshot();
        offline.networks[0].status = NodeNetworkStatus::Offline;
        assert_eq!(
            action_block_reason(&action, &offline, Some("home")),
            Some(ActionBlockReason::NoOnlineNetwork)
        );

        let mut denied = populated_snapshot();
        denied.permissions.permissions.clear();
        assert_eq!(
            action_block_reason(&action, &denied, Some("home")),
            Some(ActionBlockReason::PermissionDenied)
        );
    }

    #[test]
    fn invalid_command_input_fails_closed() {
        let manifest = node_ui_manifest();
        let action = manifest
            .pages
            .iter()
            .find(|page| page.id == "peers")
            .expect("peers page")
            .actions[0]
            .clone();
        let mut values = default_form_values(&action, Some("home"));
        values.insert("timeout_ms".into(), "not-a-number".into());
        assert!(matches!(
            build_command(&action, &values),
            Err(CommandBuildError::InvalidField { field_id, .. }) if field_id == "timeout_ms"
        ));
    }

    #[test]
    fn form_contract_rejects_undeclared_field_injection() {
        let mut action = node_ui_manifest().pages[1].actions[0].clone();
        action.fields.push(NodeUiField {
            id: "injected".into(),
            label_key: "field.injected".into(),
            input: crate::node::NodeUiInputKind::NetworkName,
            required: false,
            validation: NodeUiValidation::None,
        });
        assert!(validate_form_contract(&action).is_err());
    }
}
