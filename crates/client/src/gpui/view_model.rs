use std::collections::{HashMap, HashSet};

use fuckipv6_common::domain::{CapabilityKind, CapabilityState};
use fuckipv6_common::i18n::{t, tr, Locale};
use fuckipv6_common::ui_layout::navigation_module_for_page;
use fuckipv6_common::NetworkMode;

use crate::node::{
    NodeClientBlacklistCommand, NodeCommand, NodeCommandKind, NodeCreateNetworkCommand,
    NodeEmbeddedServiceStatus, NodeJoinNetworkCommand, NodeNetworkAllocation, NodeNetworkRole,
    NodeNetworkSelection, NodeNetworkStatus, NodePermission, NodePhase, NodePrimaryStatus,
    NodeProbePeerCommand, NodeRoutingMode, NodeSetNetworkRoleCommand, NodeSnapshot, NodeUiAction,
    NodeUiAvailability, NodeUiBackendStatus, NodeUiManifest, NodeUiPage, NodeUiPermission,
    NodeUpdateNetworkAllocationCommand, NodeUpdateRoutingCommand,
};
use crate::ui::{DisplayEnvironment, UiMode};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdvancedSettingState {
    Managed,
    ReadOnly,
    Partial,
    Unsupported,
}

impl AdvancedSettingState {
    const fn label_key(self) -> &'static str {
        match self {
            Self::Managed => "settings.advanced.state.managed",
            Self::ReadOnly => "settings.advanced.state.read_only",
            Self::Partial => "settings.advanced.state.partial",
            Self::Unsupported => "settings.advanced.state.unsupported",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AdvancedSettingRow {
    id: &'static str,
    title: String,
    detail: String,
    state: AdvancedSettingState,
    keywords: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AdvancedSettingGroup {
    title: String,
    rows: Vec<AdvancedSettingRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverviewNetwork {
    pub name: String,
    pub status: NodeNetworkStatus,
    pub roles: Vec<String>,
    pub peer_count: usize,
    pub tun_name: String,
    pub routing: String,
    pub exit: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverviewActivity {
    pub action: String,
    pub detail: String,
    pub occurred_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverviewShortcut {
    pub id: String,
    pub label: String,
    pub action_id: Option<String>,
    pub unavailable_reason: Option<String>,
}

/// Dedicated product projection for the overview. It deliberately does not
/// reuse generic metric cards: the first screen is organized around the one
/// primary status, actionable issues, compact network context, and recent
/// sanitized activity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverviewModel {
    pub node_name: String,
    pub primary_status: NodePrimaryStatus,
    pub primary_action: Option<String>,
    pub issues: Vec<String>,
    pub networks: Vec<OverviewNetwork>,
    pub activity: Vec<OverviewActivity>,
    pub shortcuts: Vec<OverviewShortcut>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageModel {
    pub page_id: String,
    pub kind: PageKind,
    pub metrics: Vec<MetricModel>,
    pub sections: Vec<SectionModel>,
    pub overview: Option<OverviewModel>,
    pub actions: Vec<ActionModel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchResultKind {
    Page,
    Identity,
    Network,
    Peer,
    Setting,
}

impl SearchResultKind {
    pub const fn label_key(self) -> &'static str {
        match self {
            Self::Page => "search.kind.page",
            Self::Identity => "search.kind.identity",
            Self::Network => "search.kind.network",
            Self::Peer => "search.kind.peer",
            Self::Setting => "search.kind.setting",
        }
    }
}

/// One object or destination returned by the first-version global search. It is
/// deliberately a navigation model, not a command palette action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub id: String,
    pub kind: SearchResultKind,
    pub title: String,
    pub detail: String,
    pub page_id: String,
    pub network_name: Option<String>,
}

impl SearchResult {
    /// Stable DOM/GPUI element identifier for the result row.
    pub fn element_id(&self) -> String {
        format!("search-result-{}", self.id)
    }
}

/// Search manifest destinations and the user-visible objects in the current
/// snapshot. Matching is local, deterministic, and never logs the query.
pub fn search_node_snapshot(
    manifest: &NodeUiManifest,
    snapshot: &NodeSnapshot,
    query: &str,
    locale: Locale,
) -> Vec<SearchResult> {
    let normalized = query.trim().to_lowercase();
    if normalized.is_empty() {
        return Vec::new();
    }

    let mut matches = Vec::new();
    let mut push = |result: SearchResult, keywords: String| {
        let haystack = format!(
            "{} {} {} {}",
            result.title, result.detail, result.id, keywords
        )
        .to_lowercase();
        if let Some(rank) = search_match_rank(&haystack, &normalized) {
            matches.push((rank, matches.len(), result));
        }
    };

    for page in &manifest.pages {
        let module = navigation_module_for_page(&page.id);
        push(
            SearchResult {
                id: format!("page:{}", page.id),
                kind: SearchResultKind::Page,
                title: t(locale, &page.nav_label_key).to_string(),
                detail: module
                    .map(|module| t(locale, &module.label_key).to_string())
                    .unwrap_or_else(|| t(locale, "common.unknown").to_string()),
                page_id: page.id.clone(),
                network_name: None,
            },
            module.map(|module| module.id.clone()).unwrap_or_default(),
        );
    }

    if let Some(identity) = &snapshot.identity {
        let title = identity
            .display_name
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&identity.node_id)
            .to_string();
        push(
            SearchResult {
                id: format!("identity:{}", identity.node_id),
                kind: SearchResultKind::Identity,
                title,
                detail: tr(
                    locale,
                    "search.identity_detail",
                    &[
                        ("node_id", &identity.node_id),
                        ("fingerprint", &identity.public_key_fingerprint),
                    ],
                ),
                page_id: "identity".into(),
                network_name: None,
            },
            format!(
                "{} {} {}",
                identity.node_id, identity.public_key_fingerprint, "identity node"
            ),
        );
    }

    for network in &snapshot.networks {
        let status = t(locale, network_status_key(network.status));
        push(
            SearchResult {
                id: format!("network:{}", network.name),
                kind: SearchResultKind::Network,
                title: network.name.clone(),
                detail: tr(
                    locale,
                    "search.network_detail",
                    &[
                        ("status", status),
                        ("tun", &network.tun_name),
                        ("peers", &network.peer_count.to_string()),
                    ],
                ),
                page_id: "networks".into(),
                network_name: Some(network.name.clone()),
            },
            format!(
                "{} {} {} network",
                network.name,
                network
                    .assigned_ipv4
                    .map(|address| address.to_string())
                    .unwrap_or_default(),
                network
                    .assigned_ipv6
                    .map(|address| address.to_string())
                    .unwrap_or_default(),
            ),
        );
    }

    for peer in &snapshot.peers {
        push(
            SearchResult {
                id: format!("peer:{}:{}", peer.network_name, peer.peer_id),
                kind: SearchResultKind::Peer,
                title: peer.peer_id.clone(),
                detail: tr(
                    locale,
                    "search.peer_detail",
                    &[
                        ("network", &peer.network_name),
                        ("address", &peer.virtual_ipv6.to_string()),
                    ],
                ),
                page_id: "peers".into(),
                network_name: Some(peer.network_name.clone()),
            },
            format!(
                "{} {} {} {} peer",
                peer.peer_id,
                peer.network_name,
                peer.virtual_ipv6,
                peer.virtual_ipv4
                    .map(|address| address.to_string())
                    .unwrap_or_default(),
            ),
        );
    }

    for group in advanced_setting_groups(snapshot, locale) {
        for setting in group.rows {
            push(
                SearchResult {
                    id: format!("setting:{}", setting.id),
                    kind: SearchResultKind::Setting,
                    title: setting.title,
                    detail: format!("{} · {}", group.title, setting.detail),
                    page_id: "settings".into(),
                    network_name: None,
                },
                format!("{} setting settings advanced 高级 设置", setting.keywords),
            );
        }
    }

    matches.sort_by_key(|(rank, index, _)| (*rank, *index));
    matches
        .into_iter()
        .take(12)
        .map(|(_, _, result)| result)
        .collect()
}

fn search_match_rank(haystack: &str, query: &str) -> Option<u8> {
    if haystack == query {
        Some(0)
    } else if haystack.starts_with(query) {
        Some(1)
    } else if haystack
        .split(|character: char| !character.is_alphanumeric())
        .any(|word| word.starts_with(query))
    {
        Some(2)
    } else if haystack.contains(query) {
        Some(3)
    } else {
        None
    }
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

    let (metrics, sections, overview) = project_page(kind, snapshot, locale);
    Ok(PageModel {
        page_id: page.id.clone(),
        kind,
        metrics,
        sections,
        overview,
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
) -> (Vec<MetricModel>, Vec<SectionModel>, Option<OverviewModel>) {
    match kind {
        PageKind::Overview => (
            Vec::new(),
            Vec::new(),
            Some(project_overview(snapshot, locale)),
        ),
        PageKind::Networks => {
            let (metrics, sections) = project_networks(snapshot, locale);
            (metrics, sections, None)
        }
        PageKind::Peers => {
            let (metrics, sections) = project_peers(snapshot, locale);
            (metrics, sections, None)
        }
        PageKind::Topology => {
            let (metrics, sections) = project_topology(snapshot, locale);
            (metrics, sections, None)
        }
        PageKind::Traffic => {
            let (metrics, sections) = project_traffic(snapshot, locale);
            (metrics, sections, None)
        }
        PageKind::Routing => {
            let (metrics, sections) = project_routing(snapshot, locale);
            (metrics, sections, None)
        }
        PageKind::ExitNode => {
            let (metrics, sections) = project_exit_node(snapshot, locale);
            (metrics, sections, None)
        }
        PageKind::Relay => {
            let (metrics, sections) = project_relay(snapshot, locale);
            (metrics, sections, None)
        }
        PageKind::Logs => {
            let (metrics, sections) = project_logs(snapshot, locale);
            (metrics, sections, None)
        }
        PageKind::Identity => {
            let (metrics, sections) = project_identity(snapshot, locale);
            (metrics, sections, None)
        }
        PageKind::Settings => {
            let (metrics, sections) = project_settings(snapshot, locale);
            (metrics, sections, None)
        }
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

fn overview_primary_action(status: NodePrimaryStatus) -> Option<&'static str> {
    match status {
        NodePrimaryStatus::Uninitialized | NodePrimaryStatus::Stopped => Some("start_node"),
        NodePrimaryStatus::Running => Some("stop_node"),
        NodePrimaryStatus::Degraded
        | NodePrimaryStatus::Failed
        | NodePrimaryStatus::ControlUnavailable => Some("get_node_diagnostics"),
        NodePrimaryStatus::Starting | NodePrimaryStatus::Stopping => None,
    }
}

fn project_overview(snapshot: &NodeSnapshot, locale: Locale) -> OverviewModel {
    let primary_status = snapshot.primary_status();
    let node_name = snapshot
        .identity
        .as_ref()
        .and_then(|identity| {
            identity
                .display_name
                .as_deref()
                .filter(|name| !name.trim().is_empty())
        })
        .or_else(|| {
            snapshot
                .identity
                .as_ref()
                .map(|identity| identity.node_id.as_str())
        })
        .unwrap_or_else(|| t(locale, "overview.node_name_fallback"));

    let mut issues = Vec::new();
    let mut seen_issues = HashSet::new();
    let mut push_issue = |key: String, text: String, issues: &mut Vec<String>| {
        if issues.len() < 3 && seen_issues.insert(key) {
            issues.push(text);
        }
    };

    for network in &snapshot.lifecycle.failed_networks {
        push_issue(
            format!("network:{network}"),
            tr(
                locale,
                "overview.issue.network",
                &[
                    ("network", network),
                    ("status", t(locale, "status.network.failed")),
                ],
            ),
            &mut issues,
        );
    }

    if let Some(problem) = &snapshot.last_error {
        let localized = tr(locale, &problem.message_key, &[]);
        let text = if localized == problem.message_key {
            t(locale, "status.primary.degraded.detail").to_string()
        } else {
            localized
        };
        push_issue("last_error".into(), text, &mut issues);
    }

    for network in &snapshot.networks {
        if !matches!(
            network.status,
            NodeNetworkStatus::Connecting
                | NodeNetworkStatus::Degraded
                | NodeNetworkStatus::Stopping
                | NodeNetworkStatus::Failed
        ) {
            continue;
        }
        push_issue(
            format!("network:{}", network.name),
            tr(
                locale,
                "overview.issue.network",
                &[
                    ("network", &network.name),
                    ("status", t(locale, network_status_key(network.status))),
                ],
            ),
            &mut issues,
        );
    }

    for service in &snapshot.embedded_services {
        if service.status != NodeEmbeddedServiceStatus::Failed {
            continue;
        }
        push_issue(
            format!("service:{}", service.network_name),
            tr(
                locale,
                "overview.issue.service",
                &[
                    ("network", &service.network_name),
                    ("status", t(locale, "status.service.failed")),
                ],
            ),
            &mut issues,
        );
    }

    let networks = snapshot
        .networks
        .iter()
        .map(|network| {
            let config = snapshot
                .configuration
                .networks
                .iter()
                .find(|config| config.network_name == network.name);
            let routing = config
                .map(|config| t(locale, routing_mode_key(config.routing_mode)).to_string())
                .unwrap_or_else(|| t(locale, "common.not_available").to_string());
            let exit = config
                .and_then(|config| config.exit_node.as_deref())
                .filter(|node| !node.trim().is_empty())
                .map(|node| tr(locale, "overview.exit.selected", &[("node", node)]))
                .unwrap_or_else(|| t(locale, "overview.exit.none").to_string());
            OverviewNetwork {
                name: network.name.clone(),
                status: network.status,
                roles: network
                    .roles
                    .iter()
                    .map(|role| t(locale, role_key(*role)).to_string())
                    .collect(),
                peer_count: network.peer_count,
                tun_name: network.tun_name.clone(),
                routing,
                exit,
            }
        })
        .collect();

    let mut ordered_activity = snapshot.recent_activity.iter().collect::<Vec<_>>();
    ordered_activity.sort_by(|left, right| {
        right
            .occurred_at_unix_ms
            .cmp(&left.occurred_at_unix_ms)
            .then_with(|| right.audit_id.cmp(&left.audit_id))
    });
    let activity = ordered_activity
        .into_iter()
        .take(5)
        .map(|entry| {
            let action_key = format!("activity.action.{}", entry.action);
            let action = t(locale, &action_key);
            let action = if action == action_key {
                t(locale, "activity.action.node.command").to_string()
            } else {
                action.to_string()
            };
            let outcome = match entry.outcome {
                fuckipv6_common::domain::AuditOutcome::Succeeded => {
                    t(locale, "activity.outcome.succeeded")
                }
                fuckipv6_common::domain::AuditOutcome::Failed => {
                    t(locale, "activity.outcome.failed")
                }
                fuckipv6_common::domain::AuditOutcome::Denied => {
                    t(locale, "activity.outcome.denied")
                }
            };
            let mut detail = tr(
                locale,
                "overview.activity.detail",
                &[("outcome", outcome), ("resource", &entry.resource)],
            );
            if let Some(command) = entry.details.get("command") {
                detail.push_str(" · ");
                detail.push_str(command);
            }
            OverviewActivity {
                action,
                detail,
                occurred_at_unix_ms: entry.occurred_at_unix_ms,
            }
        })
        .collect();

    OverviewModel {
        node_name: node_name.to_string(),
        primary_status,
        primary_action: overview_primary_action(primary_status).map(str::to_string),
        issues,
        networks,
        activity,
        shortcuts: vec![
            OverviewShortcut {
                id: "create_network".into(),
                label: t(locale, "action.create_network").to_string(),
                action_id: Some("create_network".into()),
                unavailable_reason: None,
            },
            OverviewShortcut {
                id: "discover_networks".into(),
                label: t(locale, "shortcut.discover_networks").to_string(),
                action_id: None,
                unavailable_reason: Some(t(locale, "shortcut.discovery_unavailable").to_string()),
            },
            OverviewShortcut {
                id: "join_network".into(),
                label: t(locale, "action.join_network").to_string(),
                action_id: Some("join_network".into()),
                unavailable_reason: None,
            },
        ],
    }
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

fn configured_network_values(
    snapshot: &NodeSnapshot,
    locale: Locale,
    mut render: impl FnMut(&crate::node::NodeNetworkConfigSnapshot) -> String,
) -> String {
    let values = snapshot
        .configuration
        .networks
        .iter()
        .map(|network| format!("{} · {}", network.network_name, render(network)))
        .collect::<Vec<_>>();
    if values.is_empty() {
        t(locale, "common.none").to_string()
    } else {
        values.join(t(locale, "common.list_separator"))
    }
}

fn advanced_setting_row(
    locale: Locale,
    id: &'static str,
    title_key: &str,
    detail: impl Into<String>,
    state: AdvancedSettingState,
    keywords: &'static str,
) -> AdvancedSettingRow {
    AdvancedSettingRow {
        id,
        title: t(locale, title_key).to_string(),
        detail: detail.into(),
        state,
        keywords,
    }
}

fn advanced_setting_groups(snapshot: &NodeSnapshot, locale: Locale) -> Vec<AdvancedSettingGroup> {
    let listen_ports = configured_network_values(snapshot, locale, |network| {
        snapshot
            .networks
            .iter()
            .find(|live| live.name == network.network_name)
            .and_then(|live| live.p2p_listen_addr)
            .map(|address| address.to_string())
            .unwrap_or_else(|| {
                if network.p2p_listen_port == 0 {
                    t(locale, "settings.advanced.port_random").to_string()
                } else {
                    format!(":{}", network.p2p_listen_port)
                }
            })
    });
    let keepalive = configured_network_values(snapshot, locale, |network| {
        format!(
            "{} {}",
            network.p2p_heartbeat_interval_secs,
            t(locale, "settings.advanced.unit_seconds")
        )
    });
    let connections = configured_network_values(snapshot, locale, |network| {
        format!(
            "{} {} {}",
            t(locale, "settings.advanced.connect_timeout"),
            network.p2p_connect_timeout_secs,
            t(locale, "settings.advanced.unit_seconds")
        )
    });
    let log_levels = configured_network_values(snapshot, locale, |network| {
        if network.log_level.is_empty() {
            t(locale, "common.unknown").to_string()
        } else {
            network.log_level.clone()
        }
    });

    vec![
        AdvancedSettingGroup {
            title: t(locale, "settings.section.network_listeners").to_string(),
            rows: vec![
                advanced_setting_row(
                    locale,
                    "listen-addresses",
                    "settings.row.listen_addresses",
                    t(locale, "settings.advanced.listen_addresses.detail"),
                    AdvancedSettingState::ReadOnly,
                    "listen address bind ipv4 ipv6 wildcard socket port",
                ),
                advanced_setting_row(
                    locale,
                    "listen-ports",
                    "settings.row.listen_ports",
                    listen_ports,
                    AdvancedSettingState::ReadOnly,
                    "p2p port udp listen random socket",
                ),
                advanced_setting_row(
                    locale,
                    "interface-allowlist",
                    "settings.row.interface_allowlist",
                    t(locale, "settings.advanced.interface_allowlist.detail"),
                    AdvancedSettingState::Unsupported,
                    "interface allowlist denylist network adapter",
                ),
            ],
        },
        AdvancedSettingGroup {
            title: t(locale, "settings.section.dns_privacy").to_string(),
            rows: vec![
                advanced_setting_row(
                    locale,
                    "dns-mode",
                    "settings.row.dns_mode",
                    t(locale, "settings.advanced.dns_mode.detail"),
                    AdvancedSettingState::Partial,
                    "dns mode resolver doh dot split routing",
                ),
                advanced_setting_row(
                    locale,
                    "doh-upstreams",
                    "settings.row.doh_upstreams",
                    t(locale, "settings.advanced.doh_upstreams.detail"),
                    AdvancedSettingState::Managed,
                    "doh dns over https upstream resolver",
                ),
                advanced_setting_row(
                    locale,
                    "dot",
                    "settings.row.dot",
                    t(locale, "settings.advanced.dot.detail"),
                    AdvancedSettingState::Unsupported,
                    "dot dns over tls encrypted resolver",
                ),
                advanced_setting_row(
                    locale,
                    "dns-leak-protection",
                    "settings.row.dns_leak_protection",
                    t(locale, "settings.advanced.dns_leak.detail"),
                    AdvancedSettingState::Partial,
                    "dns leak protection resolv.conf hijack",
                ),
            ],
        },
        AdvancedSettingGroup {
            title: t(locale, "settings.section.transport_limits").to_string(),
            rows: vec![
                advanced_setting_row(
                    locale,
                    "nat-detection",
                    "settings.row.nat_detection",
                    t(locale, "settings.advanced.nat_detection.detail"),
                    AdvancedSettingState::Partial,
                    "nat traversal stun mapping detection",
                ),
                advanced_setting_row(
                    locale,
                    "keepalive",
                    "settings.row.keepalive",
                    keepalive,
                    AdvancedSettingState::ReadOnly,
                    "keepalive heartbeat interval connection",
                ),
                advanced_setting_row(
                    locale,
                    "connections",
                    "settings.row.connections",
                    connections,
                    AdvancedSettingState::ReadOnly,
                    "connection timeout queue concurrency worker batch",
                ),
                advanced_setting_row(
                    locale,
                    "resource-limits",
                    "settings.row.resource_limits",
                    t(locale, "settings.advanced.resource_limits.detail"),
                    AdvancedSettingState::Unsupported,
                    "resource limit bandwidth quota connection count target",
                ),
            ],
        },
        AdvancedSettingGroup {
            title: t(locale, "settings.section.web_security").to_string(),
            rows: vec![
                advanced_setting_row(
                    locale,
                    "webui-listener",
                    "settings.row.webui_listener",
                    t(locale, "settings.advanced.webui_listener.detail"),
                    AdvancedSettingState::Managed,
                    "webui listen bind origin proxy public exposure",
                ),
                advanced_setting_row(
                    locale,
                    "webui-session",
                    "settings.row.webui_session",
                    t(locale, "settings.advanced.webui_session.detail"),
                    AdvancedSettingState::ReadOnly,
                    "session cookie passkey expiry httponly samesite login",
                ),
                advanced_setting_row(
                    locale,
                    "webui-rate-limit",
                    "settings.row.webui_rate_limit",
                    t(locale, "settings.advanced.webui_rate_limit.detail"),
                    AdvancedSettingState::ReadOnly,
                    "rate limit login mutation brute force",
                ),
                advanced_setting_row(
                    locale,
                    "hsts",
                    "settings.row.hsts",
                    t(locale, "settings.advanced.hsts.detail"),
                    AdvancedSettingState::Managed,
                    "hsts https certificate tls header security",
                ),
                advanced_setting_row(
                    locale,
                    "trusted-proxy",
                    "settings.row.trusted_proxy",
                    t(locale, "settings.advanced.trusted_proxy.detail"),
                    AdvancedSettingState::Managed,
                    "trusted proxy forwarded origin ip",
                ),
            ],
        },
        AdvancedSettingGroup {
            title: t(locale, "settings.section.observability_maintenance").to_string(),
            rows: vec![
                advanced_setting_row(
                    locale,
                    "log-levels",
                    "settings.row.log_levels",
                    log_levels,
                    AdvancedSettingState::ReadOnly,
                    "log level trace debug info warn error",
                ),
                advanced_setting_row(
                    locale,
                    "log-retention",
                    "settings.row.log_retention",
                    t(locale, "settings.advanced.log_retention.detail"),
                    AdvancedSettingState::Unsupported,
                    "log retention remote shipping redaction privacy",
                ),
                advanced_setting_row(
                    locale,
                    "metrics",
                    "settings.row.metrics",
                    t(locale, "settings.advanced.metrics.detail"),
                    AdvancedSettingState::Unsupported,
                    "metrics prometheus endpoint monitoring",
                ),
                advanced_setting_row(
                    locale,
                    "updates",
                    "settings.row.updates",
                    t(locale, "settings.advanced.updates.detail"),
                    AdvancedSettingState::Unsupported,
                    "update channel proxy signature rollback",
                ),
                advanced_setting_row(
                    locale,
                    "maintenance-window",
                    "settings.row.maintenance_window",
                    t(locale, "settings.advanced.maintenance.detail"),
                    AdvancedSettingState::Unsupported,
                    "maintenance window schedule",
                ),
            ],
        },
    ]
}

fn project_settings(
    snapshot: &NodeSnapshot,
    locale: Locale,
) -> (Vec<MetricModel>, Vec<SectionModel>) {
    let granted_permissions = snapshot
        .permissions
        .permissions
        .iter()
        .map(|permission| t(locale, permission_key(*permission)))
        .collect::<Vec<_>>()
        .join(t(locale, "common.list_separator"));
    let owner_rows = vec![
        row(
            t(locale, "settings.row.owner_identity"),
            snapshot
                .identity
                .as_ref()
                .and_then(|identity| identity.display_name.as_deref())
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(t(locale, "common.unknown")),
        ),
        row(
            t(locale, "settings.row.node_id"),
            snapshot
                .identity
                .as_ref()
                .map(|identity| identity.node_id.clone())
                .unwrap_or_else(|| t(locale, "common.not_available").into()),
        ),
        row(
            t(locale, "settings.row.public_key_fingerprint"),
            snapshot
                .identity
                .as_ref()
                .map(|identity| identity.public_key_fingerprint.clone())
                .unwrap_or_else(|| t(locale, "common.not_available").into()),
        ),
        row(
            t(locale, "settings.row.access_model"),
            t(locale, access_model_key(snapshot.permissions.access_model)),
        ),
        row(
            t(locale, "settings.row.granted_permissions"),
            if granted_permissions.is_empty() {
                t(locale, "common.none").into()
            } else {
                granted_permissions
            },
        ),
        row(
            t(locale, "settings.row.web_authentication"),
            t(locale, "settings.web_session.detail"),
        ),
        row(
            t(locale, "settings.row.owner_recovery"),
            t(locale, "settings.owner_recovery.detail"),
        ),
    ];
    let remote_rows = vec![
        row(
            t(locale, "settings.row.effective_ui"),
            snapshot
                .ui_mode
                .as_ref()
                .map(|mode| t(locale, ui_mode_key(mode.effective)))
                .unwrap_or_else(|| t(locale, "common.unknown")),
        ),
        row(
            t(locale, "settings.row.display_environment"),
            snapshot
                .ui_mode
                .as_ref()
                .map(|mode| t(locale, display_environment_key(mode.display)))
                .unwrap_or_else(|| t(locale, "common.unknown")),
        ),
        row(
            t(locale, "settings.row.webui_configuration"),
            t(locale, "settings.webui_configuration.detail"),
        ),
        row(
            t(locale, "settings.row.session_security"),
            t(locale, "settings.web_session.detail"),
        ),
    ];
    let mut service_rows = snapshot
        .capabilities
        .capabilities
        .iter()
        .map(|capability| {
            row(
                t(locale, capability_key(capability.kind)),
                t(locale, capability_state_key(capability.state)),
            )
        })
        .collect::<Vec<_>>();
    if service_rows.is_empty() {
        service_rows.push(row(
            t(locale, "settings.row.capabilities"),
            t(locale, "common.unknown"),
        ));
    }
    if snapshot.embedded_services.is_empty() {
        service_rows.push(row(
            t(locale, "settings.row.embedded_services"),
            t(locale, "common.none"),
        ));
    } else {
        service_rows.extend(snapshot.embedded_services.iter().map(|service| {
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
        }));
    }
    service_rows.push(row(
        t(locale, "settings.row.update_configuration"),
        t(locale, "settings.updates.detail"),
    ));
    let unsupported_capabilities = snapshot
        .capabilities
        .capabilities
        .iter()
        .filter(|capability| capability.state == CapabilityState::Unsupported)
        .map(|capability| t(locale, capability_key(capability.kind)))
        .collect::<Vec<_>>()
        .join(t(locale, "common.list_separator"));
    let unsupported_reasons = snapshot
        .exit_node_capabilities
        .unsupported_reasons
        .iter()
        .map(|reason| t(locale, exit_node_unsupported_reason_key(*reason)))
        .collect::<Vec<_>>()
        .join(t(locale, "common.list_separator"));
    let platform_limitations =
        if unsupported_capabilities.is_empty() && unsupported_reasons.is_empty() {
            t(locale, "settings.platform_limitations.none").to_string()
        } else {
            let capabilities = if unsupported_capabilities.is_empty() {
                t(locale, "capability.exit")
            } else {
                &unsupported_capabilities
            };
            let mut detail = tr(
                locale,
                "settings.platform_limitations.detail",
                &[("capabilities", capabilities)],
            );
            if !unsupported_reasons.is_empty() {
                detail.push_str(t(locale, "common.list_separator"));
                detail.push_str(&tr(
                    locale,
                    "settings.platform_limitations.reasons",
                    &[("reasons", &unsupported_reasons)],
                ));
            }
            detail
        };
    service_rows.push(row(
        t(locale, "settings.row.platform_limitations"),
        platform_limitations,
    ));
    let discovery_roles = snapshot
        .networks
        .iter()
        .filter(|network| network.roles.contains(&NodeNetworkRole::Discovery))
        .map(|network| {
            format!(
                "{} · {}",
                network.name,
                t(locale, network_status_key(network.status))
            )
        })
        .collect::<Vec<_>>()
        .join(t(locale, "common.list_separator"));
    let discovery_rows = vec![
        row(
            t(locale, "settings.row.discovery_roles"),
            if discovery_roles.is_empty() {
                t(locale, "settings.discovery_roles.none").into()
            } else {
                discovery_roles
            },
        ),
        row(
            t(locale, "settings.row.discovery_index"),
            t(locale, "settings.discovery_index.detail"),
        ),
        row(
            t(locale, "settings.row.custom_discovery_index"),
            t(locale, "settings.custom_index.detail"),
        ),
    ];
    let data_rows = vec![
        row(
            t(locale, "settings.row.snapshot_revision"),
            snapshot.revision.to_string(),
        ),
        row(
            t(locale, "settings.row.configured_networks"),
            snapshot.configuration.networks.len().to_string(),
        ),
        row(
            t(locale, "settings.row.recent_activity"),
            snapshot.recent_activity.len().to_string(),
        ),
        row(
            t(locale, "settings.row.backup_restore"),
            t(locale, "settings.backup.detail"),
        ),
        row(
            t(locale, "settings.row.write_mode"),
            format!(
                "{} · {}",
                t(locale, "settings.value.read_only"),
                t(locale, "settings.read_only.detail")
            ),
        ),
    ];
    let mut sections = vec![
        section(
            t(locale, "settings.section.owner_security"),
            t(locale, "common.no_data"),
            owner_rows,
        ),
        section(
            t(locale, "settings.section.remote_access"),
            t(locale, "common.not_available"),
            remote_rows,
        ),
        section(
            t(locale, "settings.section.services_updates"),
            t(locale, "gpui.empty.embedded_services"),
            service_rows,
        ),
        section(
            t(locale, "settings.section.discovery_index"),
            t(locale, "settings.discovery_index.detail"),
            discovery_rows,
        ),
        section(
            t(locale, "settings.section.data_recovery"),
            t(locale, "common.no_data"),
            data_rows,
        ),
    ];
    sections.extend(
        advanced_setting_groups(snapshot, locale)
            .into_iter()
            .map(|group| {
                let rows = group
                    .rows
                    .into_iter()
                    .map(|setting| {
                        row(
                            setting.title,
                            format!(
                                "{} · {}",
                                t(locale, setting.state.label_key()),
                                setting.detail
                            ),
                        )
                    })
                    .collect();
                section(group.title, t(locale, "common.no_data"), rows)
            }),
    );
    (Vec::new(), sections)
}

pub(crate) fn network_status_key(status: NodeNetworkStatus) -> &'static str {
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

fn capability_key(kind: CapabilityKind) -> &'static str {
    match kind {
        CapabilityKind::Coordinator => "capability.coordinator",
        CapabilityKind::Relay => "capability.relay",
        CapabilityKind::Exit => "capability.exit",
        CapabilityKind::File => "capability.file",
    }
}

fn capability_state_key(state: CapabilityState) -> &'static str {
    match state {
        CapabilityState::Enabled => "status.capability.enabled",
        CapabilityState::Disabled => "status.capability.disabled",
        CapabilityState::Degraded => "status.capability.degraded",
        CapabilityState::Starting => "status.capability.starting",
        CapabilityState::Unavailable => "status.capability.unavailable",
        CapabilityState::Unsupported => "status.capability.unsupported",
        CapabilityState::Unknown => "status.capability.unknown",
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

fn display_environment_key(display: DisplayEnvironment) -> &'static str {
    match display {
        DisplayEnvironment::LinuxX11 => "choice.display_environment.linux_x11",
        DisplayEnvironment::LinuxWayland => "choice.display_environment.linux_wayland",
        DisplayEnvironment::WindowsDesktop => "choice.display_environment.windows_desktop",
        DisplayEnvironment::MacosDesktop => "choice.display_environment.macos_desktop",
        DisplayEnvironment::Headless => "choice.display_environment.headless",
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
        node_ui_manifest, NodeEmbeddedServiceSnapshot, NodeIdentitySnapshot, NodeNetworkSnapshot,
        NodeNetworkStatus, NodePeerSnapshot, NodeProblem, NodeProblemCode, NodeTrafficSnapshot,
        NodeUiField, NodeUiValidation,
    };
    use fuckipv6_common::domain::{ActivityEntry, AuditId, AuditOutcome, NodeId};
    use std::collections::BTreeMap;

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
    fn search_finds_pages_identity_networks_peers_and_settings() {
        let manifest = node_ui_manifest();
        let snapshot = populated_snapshot();

        let identity = search_node_snapshot(&manifest, &snapshot, "identity", Locale::En);
        assert!(identity
            .iter()
            .any(|result| result.kind == SearchResultKind::Page && result.page_id == "identity"));
        assert!(identity
            .iter()
            .any(|result| result.kind == SearchResultKind::Identity));

        let home = search_node_snapshot(&manifest, &snapshot, "home", Locale::En);
        assert!(home
            .iter()
            .any(|result| result.kind == SearchResultKind::Network
                && result.network_name.as_deref() == Some("home")));
        assert!(home
            .iter()
            .any(|result| result.kind == SearchResultKind::Peer
                && result.network_name.as_deref() == Some("home")));

        let settings = search_node_snapshot(&manifest, &snapshot, "doh", Locale::En);
        assert!(settings.iter().any(|result| {
            result.kind == SearchResultKind::Setting
                && result.id == "setting:doh-upstreams"
                && result.page_id == "settings"
        }));
    }

    #[test]
    fn search_is_local_and_ignores_blank_queries() {
        let manifest = node_ui_manifest();
        let snapshot = populated_snapshot();
        assert!(search_node_snapshot(&manifest, &snapshot, "", Locale::En).is_empty());
        assert!(search_node_snapshot(&manifest, &snapshot, "   ", Locale::En).is_empty());
        assert!(
            search_node_snapshot(&manifest, &snapshot, "no-such-object", Locale::En).is_empty()
        );
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
    fn overview_projects_one_runtime_status_without_generic_metric_cards() {
        let manifest = node_ui_manifest();
        let page = manifest
            .pages
            .iter()
            .find(|page| page.id == "overview")
            .expect("overview page");
        let snapshot = populated_snapshot();
        let model =
            build_page_model_for_page(page, &snapshot, Locale::ZhCn).expect("overview model");
        let overview = model.overview.as_ref().expect("dedicated overview model");

        assert!(model.metrics.is_empty());
        assert!(model.sections.is_empty());
        assert_eq!(overview.node_name, "desktop");
        assert_eq!(overview.primary_status, NodePrimaryStatus::Running);
        assert_eq!(overview.primary_action.as_deref(), Some("stop_node"));
        assert_eq!(overview.networks.len(), 1);
        assert_eq!(overview.networks[0].name, "home");
        assert_eq!(overview.networks[0].peer_count, 1);
        assert_eq!(overview.networks[0].tun_name, "fk-home");
        assert_eq!(
            overview.networks[0].routing,
            t(Locale::ZhCn, "choice.routing.mesh_only")
        );
        assert!(overview.networks[0].exit.contains("peer-1"));
        assert_eq!(
            overview.networks[0].roles,
            vec![
                t(Locale::ZhCn, "choice.role.member").to_string(),
                t(Locale::ZhCn, "choice.role.exit_node").to_string(),
            ]
        );
        assert_eq!(
            overview
                .shortcuts
                .iter()
                .map(|shortcut| shortcut.id.as_str())
                .collect::<Vec<_>>(),
            ["create_network", "discover_networks", "join_network"]
        );
    }

    #[test]
    fn settings_projects_basic_and_advanced_read_only_inventory() {
        let manifest = node_ui_manifest();
        let page = manifest
            .pages
            .iter()
            .find(|page| page.id == "settings")
            .expect("settings page");
        let mut snapshot = populated_snapshot();
        snapshot.ui_mode = Some(crate::ui::UiModeResolution {
            requested: crate::ui::UiModeRequest::Auto,
            effective: UiMode::WebUi,
            display: crate::ui::DisplayEnvironment::Headless,
            gpui_available: false,
        });
        snapshot.networks[0].roles.push(NodeNetworkRole::Discovery);
        snapshot.refresh_capability_states();

        let model =
            build_page_model_for_page(page, &snapshot, Locale::ZhCn).expect("settings model");
        assert!(model.metrics.is_empty());
        assert_eq!(
            model
                .sections
                .iter()
                .map(|section| section.title.as_str())
                .collect::<Vec<_>>(),
            vec![
                t(Locale::ZhCn, "settings.section.owner_security"),
                t(Locale::ZhCn, "settings.section.remote_access"),
                t(Locale::ZhCn, "settings.section.services_updates"),
                t(Locale::ZhCn, "settings.section.discovery_index"),
                t(Locale::ZhCn, "settings.section.data_recovery"),
                t(Locale::ZhCn, "settings.section.network_listeners"),
                t(Locale::ZhCn, "settings.section.dns_privacy"),
                t(Locale::ZhCn, "settings.section.transport_limits"),
                t(Locale::ZhCn, "settings.section.web_security"),
                t(Locale::ZhCn, "settings.section.observability_maintenance"),
            ]
        );
        assert!(model
            .sections
            .iter()
            .all(|section| !section.rows.is_empty()));

        let owner = &model.sections[0];
        assert!(owner.rows.iter().any(|row| {
            row.title == t(Locale::ZhCn, "settings.row.node_id") && row.detail == "node-1"
        }));
        assert!(owner.rows.iter().any(|row| {
            row.title == t(Locale::ZhCn, "settings.row.granted_permissions")
                && row
                    .detail
                    .contains(t(Locale::ZhCn, "choice.permission.manage_networks"))
        }));

        let services = &model.sections[2];
        let platform_limits = services
            .rows
            .iter()
            .find(|row| row.title == t(Locale::ZhCn, "settings.row.platform_limitations"))
            .expect("platform limitations row");
        assert!(platform_limits
            .detail
            .contains(t(Locale::ZhCn, "capability.file")));
        assert!(services
            .rows
            .iter()
            .any(|row| row.detail == t(Locale::ZhCn, "status.capability.unsupported")));

        let discovery = &model.sections[3];
        assert!(discovery.rows.iter().any(|row| {
            row.title == t(Locale::ZhCn, "settings.row.discovery_roles")
                && row.detail.contains("home")
        }));

        let data = &model.sections[4];
        let write_mode = data
            .rows
            .iter()
            .find(|row| row.title == t(Locale::ZhCn, "settings.row.write_mode"))
            .expect("write mode row");
        assert!(write_mode
            .detail
            .contains(t(Locale::ZhCn, "settings.value.read_only")));
        assert!(write_mode
            .detail
            .contains(t(Locale::ZhCn, "settings.read_only.detail")));

        assert_eq!(model.sections[5].rows.len(), 3);
        assert!(model.sections[7].rows.iter().any(|row| {
            row.title == t(Locale::ZhCn, "settings.row.keepalive")
                && row.detail.contains("30")
                && row
                    .detail
                    .contains(t(Locale::ZhCn, "settings.advanced.state.read_only"))
        }));
        assert!(model.sections[6].rows.iter().any(|row| {
            row.title == t(Locale::ZhCn, "settings.row.doh_upstreams")
                && row
                    .detail
                    .contains(t(Locale::ZhCn, "settings.advanced.state.managed"))
        }));
        assert!(model.sections[8].rows.iter().any(|row| {
            row.title == t(Locale::ZhCn, "settings.row.webui_session")
                && row
                    .detail
                    .contains(t(Locale::ZhCn, "settings.advanced.state.read_only"))
        }));
        assert!(model.sections[9].rows.iter().any(|row| {
            row.title == t(Locale::ZhCn, "settings.row.metrics")
                && row
                    .detail
                    .contains(t(Locale::ZhCn, "settings.advanced.state.unsupported"))
        }));
    }

    #[test]
    fn overview_primary_action_maps_every_primary_status() {
        let cases = [
            (NodePrimaryStatus::Uninitialized, Some("start_node")),
            (NodePrimaryStatus::Stopped, Some("start_node")),
            (NodePrimaryStatus::Starting, None),
            (NodePrimaryStatus::Stopping, None),
            (NodePrimaryStatus::Running, Some("stop_node")),
            (NodePrimaryStatus::Degraded, Some("get_node_diagnostics")),
            (NodePrimaryStatus::Failed, Some("get_node_diagnostics")),
            (
                NodePrimaryStatus::ControlUnavailable,
                Some("get_node_diagnostics"),
            ),
        ];

        for (status, expected) in cases {
            assert_eq!(overview_primary_action(status), expected);
        }
    }

    #[test]
    fn overview_issues_prioritize_deduplicate_and_truncate() {
        let mut snapshot = populated_snapshot();
        snapshot.lifecycle.failed_networks = vec!["home".into()];
        snapshot.networks[0].status = NodeNetworkStatus::Failed;
        let mut office = snapshot.networks[0].clone();
        office.name = "office".into();
        office.tun_name = "fk-office".into();
        office.status = NodeNetworkStatus::Degraded;
        snapshot.networks.push(office);
        snapshot.last_error = Some(NodeProblem::new(
            NodeProblemCode::Unavailable,
            "raw backend summary must not be shown",
        ));
        snapshot
            .embedded_services
            .push(NodeEmbeddedServiceSnapshot {
                network_name: "home".into(),
                status: NodeEmbeddedServiceStatus::Failed,
                signaling_addr: None,
                relay_addr: None,
            });

        let overview = project_overview(&snapshot, Locale::En);

        assert_eq!(overview.issues.len(), 3);
        assert_eq!(
            overview.issues[0],
            tr(
                Locale::En,
                "overview.issue.network",
                &[
                    ("network", "home"),
                    ("status", t(Locale::En, "status.network.failed")),
                ],
            )
        );
        assert_eq!(
            overview.issues[1],
            t(Locale::En, "error.operation_unavailable")
        );
        assert_eq!(
            overview.issues[2],
            tr(
                Locale::En,
                "overview.issue.network",
                &[
                    ("network", "office"),
                    ("status", t(Locale::En, "status.network.degraded")),
                ],
            )
        );
        assert!(overview
            .issues
            .iter()
            .all(|issue| !issue.contains("raw backend summary")));
    }

    #[test]
    fn overview_activity_is_sorted_truncated_and_sanitized() {
        let mut snapshot = populated_snapshot();
        snapshot.recent_activity = [3_u8, 6, 1, 5, 2, 4]
            .into_iter()
            .map(|index| ActivityEntry {
                audit_id: AuditId::from_public_key(&[index]),
                operation_id: None,
                actor: NodeId::from_public_key(&[99]),
                network_id: None,
                action: "network.create".into(),
                resource: format!("resource-{index}"),
                outcome: AuditOutcome::Succeeded,
                occurred_at_unix_ms: u64::from(index) * 1_000,
                summary: format!("raw-summary-{index}"),
                details: BTreeMap::from([
                    ("command".into(), format!("command-{index}")),
                    ("token".into(), format!("secret-{index}")),
                ]),
            })
            .collect();

        let overview = project_overview(&snapshot, Locale::En);

        assert_eq!(overview.activity.len(), 5);
        assert_eq!(overview.activity[0].occurred_at_unix_ms, 6_000);
        assert_eq!(overview.activity[4].occurred_at_unix_ms, 2_000);
        assert!(overview.activity[0].detail.contains("command-6"));
        assert!(overview
            .activity
            .iter()
            .all(|activity| !activity.detail.contains("raw-summary")
                && !activity.detail.contains("secret-")));
    }

    #[test]
    fn overview_shortcuts_target_real_actions_and_disable_discovery() {
        let overview = project_overview(&populated_snapshot(), Locale::En);
        let shortcuts = overview
            .shortcuts
            .iter()
            .map(|shortcut| {
                (
                    shortcut.id.as_str(),
                    shortcut.action_id.as_deref(),
                    shortcut.unavailable_reason.is_some(),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            shortcuts,
            vec![
                ("create_network", Some("create_network"), false),
                ("discover_networks", None, true),
                ("join_network", Some("join_network"), false),
            ]
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
                let mut visible_copy = model
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
                    }))
                    .collect::<Vec<_>>();
                if let Some(overview) = model.overview.as_ref() {
                    visible_copy.push(overview.node_name.as_str());
                    visible_copy.extend(overview.issues.iter().map(String::as_str));
                    visible_copy.extend(overview.networks.iter().flat_map(|network| {
                        std::iter::once(network.name.as_str())
                            .chain(network.roles.iter().map(String::as_str))
                            .chain(std::iter::once(network.tun_name.as_str()))
                            .chain(std::iter::once(network.routing.as_str()))
                            .chain(std::iter::once(network.exit.as_str()))
                    }));
                    visible_copy.extend(overview.activity.iter().flat_map(|activity| {
                        std::iter::once(activity.action.as_str())
                            .chain(std::iter::once(activity.detail.as_str()))
                    }));
                    visible_copy.extend(overview.shortcuts.iter().flat_map(|shortcut| {
                        std::iter::once(shortcut.label.as_str())
                            .chain(shortcut.unavailable_reason.as_deref())
                    }));
                }
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
                        .any(|section| !section.rows.is_empty())
                    || model.overview.is_some(),
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
                !model.sections.is_empty() || model.overview.is_some(),
                "{} must expose at least one snapshot section",
                page.id
            );
            assert!(
                model.overview.is_some()
                    || model
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
