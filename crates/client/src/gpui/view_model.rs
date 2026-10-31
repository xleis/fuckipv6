use std::collections::{HashMap, HashSet};

use crate::node::{
    NodeCommand, NodeCreateNetworkCommand, NodeJoinNetworkCommand, NodeNetworkRole,
    NodeNetworkSelection, NodePhase, NodeProbePeerCommand, NodeRoutingMode,
    NodeSetNetworkRoleCommand, NodeSnapshot, NodeUiAction, NodeUiAvailability, NodeUiBackendStatus,
    NodeUiCommandKind, NodeUiManifest, NodeUiPage, NodeUiPermission, NodeUpdateRoutingCommand,
};

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
    CreateNetwork,
    JoinNetwork,
    StartNetworks,
    StopNetworks,
    ProbePeer,
    UpdateRouting,
    SetExitNodeRole,
    SetRelayRole,
    ShutdownNode,
}

impl ActionBinding {
    pub const ALL: [Self; 10] = [
        Self::RefreshSnapshot,
        Self::CreateNetwork,
        Self::JoinNetwork,
        Self::StartNetworks,
        Self::StopNetworks,
        Self::ProbePeer,
        Self::UpdateRouting,
        Self::SetExitNodeRole,
        Self::SetRelayRole,
        Self::ShutdownNode,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::RefreshSnapshot => "refresh_snapshot",
            Self::CreateNetwork => "create_network",
            Self::JoinNetwork => "join_network",
            Self::StartNetworks => "start_networks",
            Self::StopNetworks => "stop_networks",
            Self::ProbePeer => "probe_peer",
            Self::UpdateRouting => "update_routing",
            Self::SetExitNodeRole => "set_exit_node_role",
            Self::SetRelayRole => "set_relay_role",
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
    pub fn message(self) -> &'static str {
        match self {
            Self::NodeNotRunning => "节点尚未运行",
            Self::NoOnlineNetwork => "没有在线网络",
            Self::NoNetworkSelected => "请先选择网络",
            Self::PermissionDenied => "共享权限快照未授予此动作",
            Self::ExitNodeUnsupported => "当前平台或主机不满足 Exit Node 能力要求",
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
        expected: NodeUiCommandKind,
        actual: NodeUiCommandKind,
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
) -> Result<PageModel, ModelError> {
    let page = manifest
        .pages
        .iter()
        .find(|page| page.id == page_id)
        .ok_or_else(|| ModelError::UnknownPage {
            page_id: page_id.into(),
        })?;
    build_page_model_for_page(page, snapshot)
}

pub fn build_page_model_for_page(
    page: &NodeUiPage,
    snapshot: &NodeSnapshot,
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

    let (metrics, sections) = project_page(kind, snapshot);
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
        "refresh_snapshot" => (
            ActionBinding::RefreshSnapshot,
            NodeUiCommandKind::GetSnapshot,
        ),
        "create_network" => (
            ActionBinding::CreateNetwork,
            NodeUiCommandKind::CreateNetwork,
        ),
        "join_network" => (ActionBinding::JoinNetwork, NodeUiCommandKind::JoinNetwork),
        "start_networks" => (
            ActionBinding::StartNetworks,
            NodeUiCommandKind::StartNetworks,
        ),
        "stop_networks" => (ActionBinding::StopNetworks, NodeUiCommandKind::StopNetworks),
        "probe_peer" => (ActionBinding::ProbePeer, NodeUiCommandKind::ProbePeer),
        "update_routing" => (
            ActionBinding::UpdateRouting,
            NodeUiCommandKind::UpdateRouting,
        ),
        "set_exit_node_role" => (
            ActionBinding::SetExitNodeRole,
            NodeUiCommandKind::SetNetworkRole,
        ),
        "set_relay_role" => (
            ActionBinding::SetRelayRole,
            NodeUiCommandKind::SetNetworkRole,
        ),
        "shutdown_node" => (ActionBinding::ShutdownNode, NodeUiCommandKind::Shutdown),
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

    if action.id == "set_exit_node_role" && !snapshot.exit_node_capabilities.supported {
        return Some(ActionBlockReason::ExitNodeUnsupported);
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
                ("update_routing", "mode") => "mesh_only".into(),
                ("update_routing", "is_exit_node") => "false".into(),
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
        Ok(ActionBinding::ProbePeer) => Ok(NodeCommand::ProbePeer(NodeProbePeerCommand {
            network_name: field("network_name"),
            target: field("target"),
            timeout_ms: parse_number(action, "timeout_ms", &required("timeout_ms")?)?,
        })),
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
        "probe_peer" => HashSet::from(["network_name", "target", "timeout_ms"]),
        "update_routing" => HashSet::from(["network_name", "mode", "exit_node", "is_exit_node"]),
        "set_exit_node_role" | "set_relay_role" => {
            HashSet::from(["network_name", "role", "enabled"])
        }
        "shutdown_node" => HashSet::from([]),
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

fn project_page(kind: PageKind, snapshot: &NodeSnapshot) -> (Vec<MetricModel>, Vec<SectionModel>) {
    match kind {
        PageKind::Overview => project_overview(snapshot),
        PageKind::Networks => project_networks(snapshot),
        PageKind::Peers => project_peers(snapshot),
        PageKind::Topology => project_topology(snapshot),
        PageKind::Traffic => project_traffic(snapshot),
        PageKind::Routing => project_routing(snapshot),
        PageKind::ExitNode => project_exit_node(snapshot),
        PageKind::Relay => project_relay(snapshot),
        PageKind::Logs => project_logs(snapshot),
        PageKind::Identity => project_identity(snapshot),
        PageKind::Settings => project_settings(snapshot),
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

fn project_overview(snapshot: &NodeSnapshot) -> (Vec<MetricModel>, Vec<SectionModel>) {
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
        .unwrap_or_else(|| "尚未发布节点身份".into());
    let problem = snapshot
        .last_error
        .as_ref()
        .map(|problem| format!("{:?}: {}", problem.code, problem.message))
        .unwrap_or_else(|| "无".into());
    (
        vec![
            metric(
                "节点阶段",
                format!("{:?}", snapshot.lifecycle.phase),
                "NodeSnapshot.lifecycle",
            ),
            metric("网络", snapshot.networks.len(), "NodeSnapshot.networks"),
            metric("Peer", snapshot.peers.len(), "NodeSnapshot.peers"),
            metric("Revision", snapshot.revision, "NodeSnapshot.revision"),
        ],
        vec![
            section(
                "节点身份",
                "尚未发布节点身份",
                vec![row("当前身份", identity)],
            ),
            section(
                "最近问题",
                "没有记录到节点问题",
                vec![row("last_error", problem)],
            ),
        ],
    )
}

fn project_networks(snapshot: &NodeSnapshot) -> (Vec<MetricModel>, Vec<SectionModel>) {
    let rows = snapshot
        .networks
        .iter()
        .map(|network| {
            row(
                network.name.clone(),
                format!(
                    "{:?} · TUN {} · IPv6 {} · IPv4 {} · {} peers · {}s · roles {:?}",
                    network.status,
                    network.tun_name,
                    display_option(network.assigned_ipv6),
                    display_option(network.assigned_ipv4),
                    network.peer_count,
                    network.uptime_secs,
                    network.roles
                ),
            )
        })
        .collect::<Vec<_>>();
    let config_rows = snapshot
        .configuration
        .networks
        .iter()
        .map(|network| {
            row(
                network.network_name.clone(),
                format!(
                    "{} · TUN {} · MTU {} · P2P {} · {:?} · exit={}",
                    network.coordinator_url,
                    network.tun_name,
                    network.mtu,
                    network.p2p_listen_port,
                    network.routing_mode,
                    network.exit_node.as_deref().unwrap_or("none")
                ),
            )
        })
        .collect::<Vec<_>>();
    (
        vec![metric(
            "运行网络",
            snapshot.networks.len(),
            "NodeSnapshot.networks",
        )],
        vec![
            section("运行网络", "没有运行中的网络", rows),
            section("已保存配置", "没有已保存的网络配置", config_rows),
        ],
    )
}

fn project_peers(snapshot: &NodeSnapshot) -> (Vec<MetricModel>, Vec<SectionModel>) {
    let rows = snapshot
        .peers
        .iter()
        .map(|peer| {
            row(
                format!("{} / {}", peer.network_name, peer.peer_id),
                format!(
                    "{} · online={} · p2p={} healthy={} · rtt={} · loss={:.3} · endpoints={:?}",
                    peer.virtual_ipv6,
                    peer.online,
                    peer.p2p_active,
                    peer.p2p_healthy,
                    peer.rtt_us
                        .map(|rtt| format!("{rtt}us"))
                        .unwrap_or_else(|| "n/a".into()),
                    peer.loss_rate,
                    peer.endpoints
                ),
            )
        })
        .collect::<Vec<_>>();
    (
        vec![
            metric("可见 Peer", snapshot.peers.len(), "NodeSnapshot.peers"),
            metric(
                "P2P 健康",
                snapshot
                    .peers
                    .iter()
                    .filter(|peer| peer.p2p_healthy)
                    .count(),
                "NodePeerSnapshot.p2p_healthy",
            ),
        ],
        vec![section("Peer 列表", "当前没有 Peer", rows)],
    )
}

fn project_topology(snapshot: &NodeSnapshot) -> (Vec<MetricModel>, Vec<SectionModel>) {
    let sections = snapshot
        .networks
        .iter()
        .map(|network| {
            let rows = snapshot
                .peers
                .iter()
                .filter(|peer| peer.network_name == network.name)
                .map(|peer| {
                    row(
                        peer.peer_id.clone(),
                        format!(
                            "{} · {}",
                            peer.virtual_ipv6,
                            if peer.p2p_active {
                                "P2P"
                            } else {
                                "relay/unknown"
                            }
                        ),
                    )
                })
                .collect::<Vec<_>>();
            section(
                format!("{} [{:?}]", network.name, network.status),
                "此网络没有可见 Peer",
                rows,
            )
        })
        .collect::<Vec<_>>();
    (
        vec![metric(
            "拓扑边",
            snapshot.peers.len(),
            "network-scoped peers",
        )],
        if sections.is_empty() {
            vec![section("拓扑", "没有网络或 Peer，无法绘制拓扑", Vec::new())]
        } else {
            sections
        },
    )
}

fn project_traffic(snapshot: &NodeSnapshot) -> (Vec<MetricModel>, Vec<SectionModel>) {
    let network_rows = snapshot
        .networks
        .iter()
        .map(|network| {
            row(
                network.name.clone(),
                format!(
                    "TUN rx {} pkt / {} B · tx {} pkt / {} B · P2P rx {} pkt / {} B · tx {} pkt / {} B · errors {}",
                    network.traffic.tun_rx_packets,
                    network.traffic.tun_rx_bytes,
                    network.traffic.tun_tx_packets,
                    network.traffic.tun_tx_bytes,
                    network.traffic.p2p_rx_packets,
                    network.traffic.p2p_rx_bytes,
                    network.traffic.p2p_tx_packets,
                    network.traffic.p2p_tx_bytes,
                    network.traffic.error_count
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
            metric("累计字节", total, "NodeTrafficSnapshot"),
            metric(
                "错误计数",
                snapshot
                    .networks
                    .iter()
                    .map(|network| network.traffic.error_count)
                    .fold(0_u64, u64::saturating_add),
                "NodeTrafficSnapshot.error_count",
            ),
        ],
        vec![section("按网络", "没有流量快照", network_rows)],
    )
}

fn project_routing(snapshot: &NodeSnapshot) -> (Vec<MetricModel>, Vec<SectionModel>) {
    let rows = snapshot
        .configuration
        .networks
        .iter()
        .map(|network| {
            row(
                network.network_name.clone(),
                format!(
                    "{:?} · exit={} · direct={:?} · proxy={:?} · exit_enabled={} · allowed_clients={:?} · allow_lan={} · region={}",
                    network.routing_mode,
                    network.exit_node.as_deref().unwrap_or("none"),
                    network.direct_cidrs,
                    network.proxy_cidrs,
                    network.exit_node_enabled,
                    network.exit_node_allowed_clients,
                    network.exit_node_allow_lan_access,
                    network.exit_node_region.as_deref().unwrap_or("none")
                ),
            )
        })
        .collect::<Vec<_>>();
    (
        vec![
            metric(
                "配置网络",
                snapshot.configuration.networks.len(),
                "NodeConfigurationSnapshot",
            ),
            metric(
                "Exit Node 启用",
                snapshot
                    .configuration
                    .networks
                    .iter()
                    .filter(|network| network.exit_node_enabled)
                    .count(),
                "exit_node_enabled",
            ),
        ],
        vec![section("路由配置", "没有网络路由配置", rows)],
    )
}

fn project_exit_node(snapshot: &NodeSnapshot) -> (Vec<MetricModel>, Vec<SectionModel>) {
    let capabilities = &snapshot.exit_node_capabilities;
    let capability_rows = vec![row(
        "平台能力",
        format!(
            "platform={} · supported={} · ipv4_forwarding={} · ipv6_forwarding={} · nat={} · cleanup={}",
            exit_node_platform_label(capabilities.platform),
            capabilities.supported,
            capabilities.ipv4_forwarding,
            capabilities.ipv6_forwarding,
            capabilities.nat,
            capabilities.cleanup
        ),
    )];
    let unsupported_rows = (!capabilities.unsupported_reasons.is_empty())
        .then(|| {
            row(
                "不受支持的原因",
                capabilities
                    .unsupported_reasons
                    .iter()
                    .map(|reason| format!("{reason:?}: {}", reason.message()))
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
            row(
                network.network_name.clone(),
                format!(
                    "enabled={} · region={} · allow_lan={} · allowed_clients={:?} · selected_exit={}",
                    network.exit_node_enabled,
                    network.exit_node_region.as_deref().unwrap_or("none"),
                    network.exit_node_allow_lan_access,
                    network.exit_node_allowed_clients,
                    network.exit_node.as_deref().unwrap_or("none")
                ),
            )
        })
        .collect::<Vec<_>>();
    let role_rows = snapshot
        .networks
        .iter()
        .filter(|network| network.roles.contains(&NodeNetworkRole::ExitNode))
        .map(|network| row(network.name.clone(), "当前节点承担 Exit Node 角色"))
        .collect::<Vec<_>>();
    (
        vec![
            metric("Exit 配置", rows.len(), "exit_node_enabled"),
            metric("Exit 角色", role_rows.len(), "NodeNetworkRole::ExitNode"),
        ],
        vec![
            section("平台能力", "没有可用的 Exit Node 能力快照", capability_rows),
            section(
                "能力限制",
                "当前主机满足全部 Exit Node 能力要求",
                unsupported_rows,
            ),
            section("Exit Node 配置", "没有 Exit Node 配置", rows),
            section("当前角色", "当前没有网络承担 Exit Node 角色", role_rows),
        ],
    )
}

fn exit_node_platform_label(platform: fuckipv6_common::ExitNodePlatform) -> &'static str {
    use fuckipv6_common::ExitNodePlatform;

    match platform {
        ExitNodePlatform::Linux => "Linux",
        ExitNodePlatform::Windows => "Windows",
        ExitNodePlatform::Macos => "macOS",
        ExitNodePlatform::Other => "Other",
    }
}

fn project_relay(snapshot: &NodeSnapshot) -> (Vec<MetricModel>, Vec<SectionModel>) {
    let services = snapshot
        .embedded_services
        .iter()
        .map(|service| {
            row(
                service.network_name.clone(),
                format!(
                    "{:?} · signaling={} · relay={}",
                    service.status,
                    display_option(service.signaling_addr),
                    display_option(service.relay_addr)
                ),
            )
        })
        .collect::<Vec<_>>();
    let roles = snapshot
        .networks
        .iter()
        .filter(|network| network.roles.contains(&NodeNetworkRole::Relay))
        .map(|network| row(network.name.clone(), "当前节点承担 Relay 角色"))
        .collect::<Vec<_>>();
    (
        vec![
            metric(
                "内嵌服务",
                snapshot.embedded_services.len(),
                "NodeSnapshot.embedded_services",
            ),
            metric("Relay 角色", roles.len(), "NodeNetworkRole::Relay"),
        ],
        vec![
            section("内嵌信令/中继", "没有内嵌服务状态", services),
            section("当前角色", "当前没有网络承担 Relay 角色", roles),
        ],
    )
}

fn project_logs(snapshot: &NodeSnapshot) -> (Vec<MetricModel>, Vec<SectionModel>) {
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
        vec![metric("日志缓冲", snapshot.logs.len(), "NodeSnapshot.logs")],
        vec![section("有界日志", "没有 WARN/ERROR 日志", rows)],
    )
}

fn project_identity(snapshot: &NodeSnapshot) -> (Vec<MetricModel>, Vec<SectionModel>) {
    let identity_rows = snapshot
        .identity
        .as_ref()
        .map(|identity| {
            vec![
                row(
                    "显示名",
                    identity
                        .display_name
                        .clone()
                        .unwrap_or_else(|| "未设置".into()),
                ),
                row("节点 ID", identity.node_id.clone()),
                row("公钥指纹", identity.public_key_fingerprint.clone()),
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
                format!("identity_configured={}", network.identity_configured),
            )
        })
        .collect::<Vec<_>>();
    (
        vec![metric(
            "身份状态",
            if snapshot.identity.is_some() {
                "已发布"
            } else {
                "未发布"
            },
            "NodeSnapshot.identity",
        )],
        vec![
            section("公开身份", "尚未发布节点身份", identity_rows),
            section("网络身份配置", "没有网络配置", config_rows),
        ],
    )
}

fn project_settings(snapshot: &NodeSnapshot) -> (Vec<MetricModel>, Vec<SectionModel>) {
    let permission_rows = snapshot
        .permissions
        .permissions
        .iter()
        .map(|permission| row(format!("{permission:?}"), "已授予"))
        .collect::<Vec<_>>();
    let config_rows = snapshot
        .configuration
        .networks
        .iter()
        .map(|network| {
            row(
                network.network_name.clone(),
                format!(
                    "log_level={} · log_file={} · ACL enabled={} default={:?} rules={}",
                    network.log_level,
                    network.log_file_configured,
                    network.acl_enabled,
                    network.acl_default_action,
                    network.acl_rule_count
                ),
            )
        })
        .collect::<Vec<_>>();
    let service_rows = snapshot
        .embedded_services
        .iter()
        .map(|service| {
            row(
                service.network_name.clone(),
                format!(
                    "{:?} · signaling={} · relay={}",
                    service.status,
                    display_option(service.signaling_addr),
                    display_option(service.relay_addr)
                ),
            )
        })
        .collect::<Vec<_>>();
    (
        vec![
            metric(
                "节点阶段",
                format!("{:?}", snapshot.lifecycle.phase),
                "NodeLifecycleSnapshot.phase",
            ),
            metric(
                "访问模型",
                format!("{:?}", snapshot.permissions.access_model),
                "NodePermissionSnapshot.access_model",
            ),
            metric(
                "UI 模式",
                snapshot
                    .ui_mode
                    .as_ref()
                    .map(|mode| format!("{:?}", mode.effective))
                    .unwrap_or_else(|| "unknown".into()),
                "NodeSnapshot.ui_mode",
            ),
        ],
        vec![
            section("权限快照", "没有授予任何权限", permission_rows),
            section("网络设置", "没有网络设置", config_rows),
            section("内嵌服务", "没有内嵌服务状态", service_rows),
        ],
    )
}

fn display_option<T: std::fmt::Display>(value: Option<T>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "n/a".into())
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
                let model = build_page_model_for_page(page, &snapshot).expect("page model");
                assert_eq!(model.page_id, page.id);
                assert!(!model.actions.is_empty() || page.actions.is_empty());
                model.kind
            })
            .collect::<HashSet<_>>();
        assert_eq!(kinds, PageKind::ALL.into_iter().collect());
    }

    #[test]
    fn every_manifest_page_renders_populated_snapshot_fields() {
        let manifest = node_ui_manifest();
        let snapshot = populated_snapshot();
        for page in &manifest.pages {
            let model = build_page_model_for_page(page, &snapshot).expect("page model");
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
        assert!(build_page_model(&manifest, &NodeSnapshot::default(), "unknown").is_err());

        let mut manifest = node_ui_manifest();
        manifest.pages[0].actions[0].id = "unknown_action".into();
        assert!(build_page_model_for_page(&manifest.pages[0], &NodeSnapshot::default()).is_err());
        assert!(validate_manifest_bindings(&manifest).is_err());
    }

    #[test]
    fn action_command_mismatch_fails_closed() {
        let mut action = node_ui_manifest().pages[1].actions[0].clone();
        action.command = NodeUiCommandKind::Shutdown;
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

        let model = build_page_model_for_page(page, &snapshot).expect("exit node model");
        assert!(model.sections[0].rows[0]
            .detail
            .contains("platform=Windows"));
        assert!(model.sections[0].rows[0].detail.contains("supported=false"));
        assert!(model.sections[1].rows[0]
            .detail
            .contains("PlatformNotLinux"));
        assert!(model.sections[1].rows[0]
            .detail
            .contains("only supported on Linux"));
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
            let model = build_page_model_for_page(page, &NodeSnapshot::default())
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
        let single = build_page_model(&manifest, &snapshot, "networks").expect("single network");
        let single_section = &single.sections[0];
        assert_eq!(single_section.rows.len(), 1);
        assert!(single_section.rows[0].detail.contains("fd00::2"));

        let mut second = snapshot.networks[0].clone();
        second.name = "office".into();
        second.tun_name = "fk-office".into();
        second.assigned_ipv6 = Some("fd00::4".parse().expect("IPv6"));
        snapshot.networks.push(second);
        let dual = build_page_model(&manifest, &snapshot, "networks").expect("dual network");
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
