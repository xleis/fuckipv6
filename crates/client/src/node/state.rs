use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use tokio::sync::{watch, RwLock};

use fuckipv6_common::domain::{ActivityEntry, CapabilityKind, CapabilityState, OperationRecovery};
use fuckipv6_common::ExitNodeCapabilities;

use super::{NodeEventBus, NodeEventKind, NodeLifecycleSnapshot, NodePhase};
use crate::tun::TunCapabilities;
use crate::ui::UiModeResolution;

/// Stable, transport-neutral state rendered by every node UI adapter.
#[derive(Debug, Clone, Default, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct NodeSnapshot {
    pub revision: u64,
    pub lifecycle: NodeLifecycleSnapshot,
    /// Health of the UI adapter's control connection. This is local transport
    /// observation, not authoritative node state, so changing it must not
    /// manufacture a new node revision.
    #[serde(default)]
    pub control_health: NodeControlHealth,
    pub identity: Option<NodeIdentitySnapshot>,
    #[serde(default)]
    pub configuration: NodeConfigurationSnapshot,
    /// Runtime capability truth for this host. Adapters must disable Exit Node
    /// actions when `supported` is false and render the typed reasons instead
    /// of inferring success from configuration alone.
    #[serde(default)]
    pub exit_node_capabilities: ExitNodeCapabilities,
    /// Unified four-capability projection shared by every UI adapter. Missing
    /// entries deserialize as `Unknown`; they are never inferred as enabled.
    #[serde(default)]
    pub capabilities: NodeCapabilityRegistrySnapshot,
    #[serde(default)]
    pub permissions: NodePermissionSnapshot,
    #[serde(default)]
    pub embedded_services: Vec<NodeEmbeddedServiceSnapshot>,
    #[serde(default)]
    pub ui_mode: Option<UiModeResolution>,
    /// Redacted TUN readiness for this node process. Missing or malformed
    /// capability data must disable host-network mutations in every adapter.
    #[serde(default)]
    pub tun_capabilities: TunCapabilities,
    /// Authoritative host interface capture for the node page. This is filled
    /// by the node process from OS network state; UI adapters never probe the
    /// host themselves. Missing data deserializes as an empty capture.
    #[serde(default)]
    pub interfaces: Vec<NodeInterfaceSnapshot>,
    /// Redacted problems encountered while collecting or reconciling host
    /// interfaces. Command and runtime failures remain in `last_error` when
    /// they are tied to a network lifecycle transition.
    #[serde(default)]
    pub interface_problems: Vec<NodeProblem>,
    /// Authoritative per-network traffic-policy summaries. The node runtime is
    /// the only writer; UI adapters never derive "applied" state from editable
    /// configuration. Missing data from older peers deserializes as empty.
    #[serde(default)]
    pub routing_summaries: Vec<NodeRoutingSummarySnapshot>,
    pub networks: Vec<NodeNetworkSnapshot>,
    pub peers: Vec<NodePeerSnapshot>,
    /// The five most recent user-meaningful command outcomes. Entries are
    /// already redacted by the domain audit projection and ordered newest
    /// first, so every UI adapter consumes the same activity feed.
    #[serde(default)]
    pub recent_activity: Vec<ActivityEntry>,
    pub logs: Vec<NodeLogEntry>,
    pub last_error: Option<NodeProblem>,
}

impl NodeSnapshot {
    /// Recompute the capability registry from authoritative snapshot fields.
    /// Derived state is never stored independently by adapters.
    pub fn refresh_capability_states(&mut self) {
        self.capabilities = NodeCapabilityRegistrySnapshot::from_snapshot(self);
    }

    /// Return the single user-facing status for this snapshot. Transitions win
    /// over failures, failures over degradation, and unavailable control is
    /// shown only after node-local problems have been ruled out.
    pub fn primary_status(&self) -> NodePrimaryStatus {
        match self.lifecycle.phase {
            NodePhase::Starting => NodePrimaryStatus::Starting,
            NodePhase::Stopping => NodePrimaryStatus::Stopping,
            NodePhase::Failed => NodePrimaryStatus::Failed,
            NodePhase::Running if self.is_degraded() => NodePrimaryStatus::Degraded,
            NodePhase::Running if self.control_health == NodeControlHealth::Unavailable => {
                NodePrimaryStatus::ControlUnavailable
            }
            NodePhase::Running => NodePrimaryStatus::Running,
            NodePhase::Stopped => {
                if self.identity.is_none()
                    && self.configuration.networks.is_empty()
                    && self.networks.is_empty()
                    && self.lifecycle.configured_networks == 0
                {
                    NodePrimaryStatus::Uninitialized
                } else {
                    NodePrimaryStatus::Stopped
                }
            }
        }
    }

    fn is_degraded(&self) -> bool {
        self.last_error.is_some()
            || !self.lifecycle.failed_networks.is_empty()
            || self.networks.iter().any(|network| {
                matches!(
                    network.status,
                    NodeNetworkStatus::Connecting
                        | NodeNetworkStatus::Degraded
                        | NodeNetworkStatus::Stopping
                        | NodeNetworkStatus::Failed
                )
            })
            || self.embedded_services.iter().any(|service| {
                matches!(
                    service.status,
                    NodeEmbeddedServiceStatus::Starting
                        | NodeEmbeddedServiceStatus::Stopping
                        | NodeEmbeddedServiceStatus::Failed
                )
            })
    }

    /// Returns machine-readable consistency violations for diagnostics and
    /// tests. UI adapters may surface these values, but must never repair state
    /// locally or derive a parallel state machine from them.
    pub fn invariant_violations(&self) -> Vec<String> {
        let mut violations = Vec::new();

        collect_duplicate_names(
            self.networks.iter().map(|network| network.name.as_str()),
            "duplicate_network",
            &mut violations,
        );
        collect_duplicate_names(
            self.configuration
                .networks
                .iter()
                .map(|network| network.network_name.as_str()),
            "duplicate_configuration",
            &mut violations,
        );
        collect_duplicate_names(
            self.embedded_services
                .iter()
                .map(|service| service.network_name.as_str()),
            "duplicate_embedded_service",
            &mut violations,
        );
        collect_duplicate_values(
            self.interfaces.iter().map(|interface| interface.index),
            "duplicate_interface_index",
            &mut violations,
        );
        collect_duplicate_names(
            self.interfaces
                .iter()
                .map(|interface| interface.name.as_str()),
            "duplicate_interface_name",
            &mut violations,
        );
        collect_duplicate_names(
            self.routing_summaries
                .iter()
                .map(|summary| summary.network_name.as_str()),
            "duplicate_routing_summary",
            &mut violations,
        );

        let configured = self
            .configuration
            .networks
            .iter()
            .map(|network| network.network_name.as_str())
            .collect::<HashSet<_>>();
        let networks = self
            .networks
            .iter()
            .map(|network| (network.name.as_str(), network))
            .collect::<HashMap<_, _>>();

        for summary in &self.routing_summaries {
            if !configured.contains(summary.network_name.as_str()) {
                violations.push(format!(
                    "routing_summary_for_unconfigured_network:{}",
                    summary.network_name
                ));
            }
            match summary.status {
                NodeRoutingSummaryStatus::Current | NodeRoutingSummaryStatus::Stale
                    if summary.last_applied_revision.is_none()
                        || summary.last_applied_at_unix_ms.is_none() =>
                {
                    violations.push(format!(
                        "applied_routing_summary_missing_history:{}",
                        summary.network_name
                    ));
                }
                NodeRoutingSummaryStatus::Current
                | NodeRoutingSummaryStatus::Stale
                | NodeRoutingSummaryStatus::Invalid
                | NodeRoutingSummaryStatus::ApplyFailed => {}
            }
            if summary.last_applied_revision.is_some() != summary.last_applied_at_unix_ms.is_some()
            {
                violations.push(format!(
                    "incomplete_routing_summary_history:{}",
                    summary.network_name
                ));
            }
            if summary.status == NodeRoutingSummaryStatus::Current && summary.problem.is_some() {
                violations.push(format!(
                    "current_routing_summary_has_problem:{}",
                    summary.network_name
                ));
            }
            if summary.status != NodeRoutingSummaryStatus::Current && summary.problem.is_none() {
                violations.push(format!(
                    "blocked_routing_summary_missing_reason:{}",
                    summary.network_name
                ));
            }
            match (&summary.default_action, &summary.effective_target) {
                (NodeRoutingDefaultAction::Direct | NodeRoutingDefaultAction::Reject, None) => {}
                (NodeRoutingDefaultAction::Direct | NodeRoutingDefaultAction::Reject, Some(_)) => {
                    violations.push(format!(
                        "routing_summary_unexpected_target:{}",
                        summary.network_name
                    ))
                }
                (
                    NodeRoutingDefaultAction::Network,
                    Some(NodeRoutingTargetSnapshot::Network { network_name }),
                ) if network_name == &summary.network_name => {}
                (
                    NodeRoutingDefaultAction::Network,
                    Some(NodeRoutingTargetSnapshot::Network { .. }),
                ) => violations.push(format!(
                    "routing_summary_network_target_mismatch:{}",
                    summary.network_name
                )),
                (
                    NodeRoutingDefaultAction::Network,
                    Some(NodeRoutingTargetSnapshot::Exit { .. }) | None,
                ) => violations.push(format!(
                    "routing_summary_network_target_invalid:{}",
                    summary.network_name
                )),
                (
                    NodeRoutingDefaultAction::Exit(exit_node),
                    Some(NodeRoutingTargetSnapshot::Exit { node_id }),
                ) if exit_node == node_id => {}
                (
                    NodeRoutingDefaultAction::Exit(_),
                    Some(NodeRoutingTargetSnapshot::Exit { .. }),
                ) => violations.push(format!(
                    "routing_summary_exit_target_mismatch:{}",
                    summary.network_name
                )),
                (
                    NodeRoutingDefaultAction::Exit(_),
                    Some(NodeRoutingTargetSnapshot::Network { .. }) | None,
                ) => violations.push(format!(
                    "routing_summary_exit_target_invalid:{}",
                    summary.network_name
                )),
            }
        }

        for network in &self.networks {
            if !configured.contains(network.name.as_str()) {
                violations.push(format!("missing_network_configuration:{}", network.name));
            }

            let expected_peers = self
                .peers
                .iter()
                .filter(|peer| peer.network_name == network.name)
                .count();
            if network.peer_count != expected_peers {
                violations.push(format!(
                    "peer_count_mismatch:{}:expected={expected_peers}:actual={}",
                    network.name, network.peer_count
                ));
            }
            if network.status == NodeNetworkStatus::Offline && expected_peers > 0 {
                violations.push(format!("offline_network_has_peers:{}", network.name));
            }
        }

        for peer in &self.peers {
            match networks.get(peer.network_name.as_str()) {
                None => violations.push(format!("peer_for_unknown_network:{}", peer.network_name)),
                Some(network) if network.status == NodeNetworkStatus::Offline => {
                    violations.push(format!("peer_for_offline_network:{}", peer.network_name));
                }
                Some(_) => {}
            }
        }

        let product_interfaces = self
            .interfaces
            .iter()
            .filter(|interface| interface.ownership == NodeInterfaceOwnership::Product)
            .map(|interface| interface.name.as_str())
            .collect::<HashSet<_>>();
        for interface in self
            .interfaces
            .iter()
            .filter(|interface| interface.ownership == NodeInterfaceOwnership::Product)
        {
            if interface.kind != NodeInterfaceKind::Tun {
                violations.push(format!("product_interface_not_tun:{}", interface.name));
            }
            let has_live_network = self.networks.iter().any(|network| {
                network.status == NodeNetworkStatus::Online && network.tun_name == interface.name
            });
            if !has_live_network {
                violations.push(format!(
                    "product_interface_without_live_network:{}",
                    interface.name
                ));
            }
        }
        for network in self
            .networks
            .iter()
            .filter(|network| network.status == NodeNetworkStatus::Online)
        {
            if !product_interfaces.contains(network.tun_name.as_str()) {
                violations.push(format!(
                    "online_network_without_product_interface:{}",
                    network.name
                ));
            }
        }

        violations
    }
}

fn collect_duplicate_values<T>(
    values: impl IntoIterator<Item = T>,
    prefix: &str,
    violations: &mut Vec<String>,
) where
    T: Copy + std::hash::Hash + Eq + std::fmt::Display,
{
    let mut seen = HashSet::new();
    for value in values {
        if !seen.insert(value) {
            violations.push(format!("{prefix}:{value}"));
        }
    }
}

/// Whether the adapter can currently reach the node control plane.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeControlHealth {
    #[default]
    Unknown,
    Available,
    Unavailable,
}

/// The one status a user should see, independent of how many subsystem states
/// contribute to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodePrimaryStatus {
    Uninitialized,
    Stopped,
    Starting,
    Stopping,
    Running,
    Degraded,
    Failed,
    ControlUnavailable,
}

impl NodePrimaryStatus {
    pub const ALL: [Self; 8] = [
        Self::Uninitialized,
        Self::Stopped,
        Self::Starting,
        Self::Stopping,
        Self::Running,
        Self::Degraded,
        Self::Failed,
        Self::ControlUnavailable,
    ];

    pub const fn label_key(self) -> &'static str {
        match self {
            Self::Uninitialized => "status.primary.uninitialized",
            Self::Stopped => "status.primary.stopped",
            Self::Starting => "status.primary.starting",
            Self::Stopping => "status.primary.stopping",
            Self::Running => "status.primary.running",
            Self::Degraded => "status.primary.degraded",
            Self::Failed => "status.primary.failed",
            Self::ControlUnavailable => "status.primary.control_unavailable",
        }
    }

    pub const fn label_zh(self) -> &'static str {
        match self {
            Self::Uninitialized => "未初始化",
            Self::Stopped => "已停止",
            Self::Starting => "正在启动",
            Self::Stopping => "正在停止",
            Self::Running => "运行正常",
            Self::Degraded => "运行降级",
            Self::Failed => "节点故障",
            Self::ControlUnavailable => "控制服务异常",
        }
    }

    pub const fn detail_key(self) -> &'static str {
        match self {
            Self::Uninitialized => "status.primary.uninitialized.detail",
            Self::Stopped => "status.primary.stopped.detail",
            Self::Starting => "status.primary.starting.detail",
            Self::Stopping => "status.primary.stopping.detail",
            Self::Running => "status.primary.running.detail",
            Self::Degraded => "status.primary.degraded.detail",
            Self::Failed => "status.primary.failed.detail",
            Self::ControlUnavailable => "status.primary.control_unavailable.detail",
        }
    }

    pub const fn detail_zh(self) -> &'static str {
        match self {
            Self::Uninitialized => "完成节点初始化后创建或加入网络。",
            Self::Stopped => "启动节点以恢复网络连接。",
            Self::Starting => "正在启动节点，请稍候。",
            Self::Stopping => "正在停止节点，请稍候。",
            Self::Running => "节点和已配置网络运行正常。",
            Self::Degraded => "检查最近问题并重试受影响网络。",
            Self::Failed => "打开诊断并重启节点。",
            Self::ControlUnavailable => "节点控制服务暂时不可达；数据面不会被自动停止，正在重连。",
        }
    }
}

/// Stable projection of the fixed first-version capability set. Unknown
/// entries fail closed and unsupported capabilities remain visible so the UI
/// can explain why an action is unavailable.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeCapabilityRegistrySnapshot {
    pub capabilities: Vec<NodeCapabilitySnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeCapabilitySnapshot {
    pub kind: CapabilityKind,
    pub state: CapabilityState,
}

impl NodeCapabilityRegistrySnapshot {
    pub fn state(&self, kind: CapabilityKind) -> CapabilityState {
        self.capabilities
            .iter()
            .find(|capability| capability.kind == kind)
            .map_or(CapabilityState::Unknown, |capability| capability.state)
    }

    fn from_snapshot(snapshot: &NodeSnapshot) -> Self {
        let service_state = embedded_service_capability_state(&snapshot.embedded_services);
        let exit_state = exit_node_capability_state(snapshot);
        Self {
            capabilities: vec![
                NodeCapabilitySnapshot {
                    kind: CapabilityKind::Coordinator,
                    state: service_state,
                },
                NodeCapabilitySnapshot {
                    kind: CapabilityKind::Relay,
                    state: service_state,
                },
                NodeCapabilitySnapshot {
                    kind: CapabilityKind::Exit,
                    state: exit_state,
                },
                // File runtime is not wired in this build. Keeping the state
                // explicit prevents an empty card from implying support.
                NodeCapabilitySnapshot {
                    kind: CapabilityKind::File,
                    state: CapabilityState::Unsupported,
                },
            ],
        }
    }
}

impl Default for NodeCapabilityRegistrySnapshot {
    fn default() -> Self {
        Self {
            capabilities: CapabilityKind::ALL
                .into_iter()
                .map(|kind| NodeCapabilitySnapshot {
                    kind,
                    state: CapabilityState::Unknown,
                })
                .collect(),
        }
    }
}

fn embedded_service_capability_state(services: &[NodeEmbeddedServiceSnapshot]) -> CapabilityState {
    if services
        .iter()
        .any(|service| service.status == NodeEmbeddedServiceStatus::Running)
    {
        CapabilityState::Enabled
    } else if services.iter().any(|service| {
        matches!(
            service.status,
            NodeEmbeddedServiceStatus::Starting | NodeEmbeddedServiceStatus::Stopping
        )
    }) {
        CapabilityState::Starting
    } else if services
        .iter()
        .any(|service| service.status == NodeEmbeddedServiceStatus::Failed)
    {
        CapabilityState::Degraded
    } else {
        CapabilityState::Disabled
    }
}

fn exit_node_capability_state(snapshot: &NodeSnapshot) -> CapabilityState {
    if !snapshot.exit_node_capabilities.supported {
        return CapabilityState::Unsupported;
    }
    let exit_networks = snapshot
        .networks
        .iter()
        .filter(|network| network.roles.contains(&NodeNetworkRole::ExitNode))
        .collect::<Vec<_>>();
    if exit_networks.is_empty() {
        return CapabilityState::Disabled;
    }
    if exit_networks
        .iter()
        .any(|network| network.status == NodeNetworkStatus::Failed)
    {
        CapabilityState::Degraded
    } else if exit_networks.iter().any(|network| {
        matches!(
            network.status,
            NodeNetworkStatus::Online | NodeNetworkStatus::Degraded
        )
    }) {
        if exit_networks
            .iter()
            .any(|network| network.status == NodeNetworkStatus::Degraded)
        {
            CapabilityState::Degraded
        } else {
            CapabilityState::Enabled
        }
    } else {
        CapabilityState::Starting
    }
}

fn collect_duplicate_names<'a>(
    names: impl Iterator<Item = &'a str>,
    code: &str,
    violations: &mut Vec<String>,
) {
    let mut seen = HashSet::new();
    for name in names {
        if !seen.insert(name) {
            violations.push(format!("{code}:{name}"));
        }
    }
}

/// Effective, non-secret network configuration exposed to every UI adapter.
#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeConfigurationSnapshot {
    pub networks: Vec<NodeNetworkConfigSnapshot>,
}

/// One network's authoritative traffic-policy state as last applied by the
/// node runtime. Editable configuration remains in `NodeConfigurationSnapshot`.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeRoutingSummarySnapshot {
    pub network_name: String,
    pub default_action: NodeRoutingDefaultAction,
    pub effective_target: Option<NodeRoutingTargetSnapshot>,
    pub enabled_rule_count: usize,
    pub last_applied_revision: Option<u64>,
    pub last_applied_at_unix_ms: Option<u64>,
    pub status: NodeRoutingSummaryStatus,
    pub problem: Option<NodeProblem>,
}

/// The default action currently applied to traffic that does not match a
/// network-specific rule.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum NodeRoutingDefaultAction {
    Direct,
    Network,
    Exit(String),
    Reject,
}

/// Concrete traffic path behind a network or Exit default action.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum NodeRoutingTargetSnapshot {
    Network { network_name: String },
    Exit { node_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeRoutingSummaryStatus {
    /// Runtime accepted the policy and the matching snapshot was published.
    Current,
    /// The last successful application predates the current stopped/runtime
    /// state and must not be treated as active.
    Stale,
    /// Policy validation rejected the configuration before mutation.
    Invalid,
    /// The runtime or snapshot publication failed after an application attempt.
    ApplyFailed,
}

/// One network's effective configuration after loading startup overrides.
///
/// Filesystem paths and other host-local details are deliberately omitted. The
/// boolean fields only state whether optional host-local settings are present.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeNetworkConfigSnapshot {
    pub network_name: String,
    pub coordinator_url: String,
    pub virtual_mode: Option<fuckipv6_common::NetworkMode>,
    pub preferred_ipv4: Option<Ipv4Addr>,
    pub preferred_ipv6: Option<Ipv6Addr>,
    pub virtual_network: Option<String>,
    pub tun_name: String,
    pub mtu: u16,
    pub p2p_listen_port: u16,
    pub p2p_connect_timeout_secs: u64,
    pub p2p_heartbeat_interval_secs: u64,
    pub routing_mode: NodeRoutingMode,
    pub exit_node: Option<String>,
    pub direct_cidrs: Vec<String>,
    pub proxy_cidrs: Vec<String>,
    pub exit_node_enabled: bool,
    pub exit_node_allowed_clients: Vec<String>,
    pub exit_node_allow_lan_access: bool,
    pub exit_node_region: Option<String>,
    pub acl_enabled: bool,
    pub acl_default_action: fuckipv6_common::AclAction,
    pub acl_rule_count: usize,
    pub log_level: String,
    pub log_file_configured: bool,
    pub identity_configured: bool,
}

/// Non-secret runtime state for one network's embedded signaling and relay
/// services. Invitation tickets, identity material, and host-local paths are
/// deliberately excluded from this shared UI contract.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeEmbeddedServiceSnapshot {
    pub network_name: String,
    pub status: NodeEmbeddedServiceStatus,
    pub signaling_addr: Option<SocketAddr>,
    pub relay_addr: Option<SocketAddr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeEmbeddedServiceStatus {
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed,
}

/// Control-plane authorization posture shared with UI adapters.
///
/// `TrustedControlPlane` means the runtime accepts every connected adapter as
/// fully privileged. It does not mean per-user or remote authentication exists.
#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodePermissionSnapshot {
    pub access_model: NodeAccessModel,
    pub permissions: Vec<NodePermission>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeAccessModel {
    #[default]
    TrustedControlPlane,
}

/// Stable capability classes used by both the permission snapshot and UI actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodePermission {
    ViewState,
    ManageNetworks,
    ChangeRouting,
    ManageRoles,
    ProbePeers,
    StopNode,
    ViewLogs,
}

impl NodePermission {
    pub const ALL: [Self; 7] = [
        Self::ViewState,
        Self::ManageNetworks,
        Self::ChangeRouting,
        Self::ManageRoles,
        Self::ProbePeers,
        Self::StopNode,
        Self::ViewLogs,
    ];
}

/// Public node identity metadata. Secret key material must never enter this DTO.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeIdentitySnapshot {
    pub node_id: String,
    pub display_name: Option<String>,
    pub public_key_fingerprint: String,
}

/// One local network and the roles this node currently performs in it.
#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct NodeNetworkSnapshot {
    pub name: String,
    pub tun_name: String,
    pub status: NodeNetworkStatus,
    pub assigned_ipv6: Option<Ipv6Addr>,
    pub assigned_ipv4: Option<Ipv4Addr>,
    pub p2p_listen_addr: Option<SocketAddr>,
    pub uptime_secs: u64,
    pub roles: Vec<NodeNetworkRole>,
    pub peer_count: usize,
    pub traffic: NodeTrafficSnapshot,
}

/// Network roles are capabilities within a network, never product identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeNetworkRole {
    Member,
    Discovery,
    Relay,
    ExitNode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeRoutingMode {
    MeshOnly,
    BypassMainland,
    GlobalProxy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeNetworkStatus {
    Offline,
    Connecting,
    Online,
    Degraded,
    Stopping,
    Failed,
}

/// One redacted host network interface captured by the node process.
///
/// The snapshot intentionally omits hardware addresses, routes, policy rules
/// and operating-system diagnostics. UI adapters render this projection and
/// never query or mutate host networking themselves.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeInterfaceSnapshot {
    pub index: u32,
    pub name: String,
    pub kind: NodeInterfaceKind,
    pub ownership: NodeInterfaceOwnership,
    pub status: NodeInterfaceStatus,
    pub mtu: u32,
    pub addresses: Vec<NodeInterfaceAddress>,
    pub default_ipv4: bool,
    pub default_ipv6: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeInterfaceAddress {
    pub address: IpAddr,
    pub prefix_len: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeInterfaceKind {
    Loopback,
    Physical,
    Virtual,
    Tun,
    Unknown,
}

/// Product ownership is established by an exact match against the live
/// runtime's TUN name. Interface naming conventions never establish ownership.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeInterfaceOwnership {
    System,
    Product,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeInterfaceStatus {
    Up,
    Down,
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeTrafficSnapshot {
    pub tun_rx_packets: u64,
    pub tun_rx_bytes: u64,
    pub tun_tx_packets: u64,
    pub tun_tx_bytes: u64,
    pub p2p_rx_packets: u64,
    pub p2p_rx_bytes: u64,
    pub p2p_tx_packets: u64,
    pub p2p_tx_bytes: u64,
    pub error_count: u64,
}

/// Peer state scoped to a network. The same node may appear in multiple networks.
#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct NodePeerSnapshot {
    pub peer_id: String,
    pub network_name: String,
    pub virtual_ipv6: Ipv6Addr,
    pub virtual_ipv4: Option<Ipv4Addr>,
    pub endpoints: Vec<SocketAddr>,
    pub online: bool,
    pub p2p_active: bool,
    pub p2p_healthy: bool,
    pub rtt_us: Option<u64>,
    pub loss_rate: f32,
    pub traffic: NodeTrafficSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeLogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeLogEntry {
    pub unix_ms: u64,
    pub level: NodeLogLevel,
    pub target: String,
    pub message: String,
}

/// Structured field-level validation issue carried by a command error.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeValidationIssue {
    pub path: String,
    pub code: String,
    pub message: String,
}

/// Stable machine-readable error details. Human-readable messages remain in
/// `NodeProblem::message`, while these fields are safe for adapters to use for
/// field highlighting and revision conflict recovery.
#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeProblemDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub validation_issues: Vec<NodeValidationIssue>,
}

/// UI- and transport-neutral error returned by every node command adapter.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeProblem {
    pub code: NodeProblemCode,
    pub category: NodeErrorCategory,
    pub message: String,
    pub message_key: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: Box<BTreeMap<String, String>>,
    pub network_name: Option<String>,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<Box<OperationRecovery>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Box<NodeProblemDetails>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeProblemCode {
    AuthenticationRequired,
    InvalidRequest,
    NotFound,
    Conflict,
    Unavailable,
    PermissionDenied,
    Cancelled,
    TimedOut,
    /// The operation is valid but unavailable on this platform or host.
    Unsupported,
    Internal,
}

/// Stable error domains shared by command results, HTTP responses and
/// WebSocket failures. UI code must branch on this category rather than parse
/// human-readable messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeErrorCategory {
    Authentication,
    Authorization,
    Validation,
    State,
    Capability,
    Dependency,
    Internal,
}

impl NodeProblemCode {
    pub const fn category(self) -> NodeErrorCategory {
        match self {
            Self::AuthenticationRequired => NodeErrorCategory::Authentication,
            Self::InvalidRequest => NodeErrorCategory::Validation,
            Self::NotFound | Self::Conflict | Self::Cancelled => NodeErrorCategory::State,
            Self::PermissionDenied => NodeErrorCategory::Authorization,
            Self::Unavailable | Self::TimedOut => NodeErrorCategory::Dependency,
            Self::Unsupported => NodeErrorCategory::Capability,
            Self::Internal => NodeErrorCategory::Internal,
        }
    }

    pub const fn message_key(self) -> &'static str {
        match self {
            Self::AuthenticationRequired => "error.authentication_required",
            Self::InvalidRequest => "error.operation_invalid_request",
            Self::NotFound => "error.operation_not_found",
            Self::Conflict => "error.operation_conflict",
            Self::Unavailable => "error.operation_unavailable",
            Self::PermissionDenied => "error.operation_permission_denied",
            Self::Cancelled => "error.operation_cancelled",
            Self::TimedOut => "error.operation_timed_out",
            Self::Unsupported => "error.operation_unsupported",
            Self::Internal => "error.operation_internal",
        }
    }

    pub const fn recovery(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::AuthenticationRequired => Some(("reauth", "recovery.reauthenticate")),
            Self::InvalidRequest => Some(("edit_request", "recovery.fix_request")),
            Self::NotFound | Self::Conflict => Some(("refresh", "recovery.refresh_state")),
            Self::Unavailable | Self::Cancelled | Self::TimedOut => {
                Some(("retry", "recovery.retry"))
            }
            Self::PermissionDenied => Some(("request_access", "recovery.request_access")),
            Self::Unsupported => Some(("change_request", "recovery.use_supported_capability")),
            Self::Internal => Some(("inspect_diagnostics", "recovery.inspect_diagnostics")),
        }
    }
}

impl NodeProblem {
    pub fn new(code: NodeProblemCode, message: impl Into<String>) -> Self {
        let recovery = code.recovery().map(|(kind, hint_key)| {
            Box::new(OperationRecovery {
                kind: kind.into(),
                hint_key: hint_key.into(),
            })
        });
        Self {
            code,
            category: code.category(),
            message: message.into(),
            message_key: code.message_key().into(),
            params: Box::default(),
            network_name: None,
            retryable: false,
            recovery,
            details: None,
        }
    }

    /// Override the stable catalog key while keeping the problem category and
    /// code unchanged. Human-readable `message` is never used as a key.
    pub fn with_message_key(mut self, key: impl Into<String>) -> Self {
        self.message_key = key.into();
        self
    }

    pub fn with_param(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.params.insert(key.into(), value.into());
        self
    }

    pub fn for_network(mut self, network_name: impl Into<String>) -> Self {
        self.network_name = Some(network_name.into());
        self
    }

    pub fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    pub fn with_revision_conflict(mut self, expected: u64, current: u64) -> Self {
        let mut details = self.details.take().unwrap_or_default();
        details.expected_revision = Some(expected);
        details.current_revision = Some(current);
        self.details = Some(details);
        self
    }

    pub fn with_validation_issues(
        mut self,
        issues: impl IntoIterator<Item = NodeValidationIssue>,
    ) -> Self {
        let mut details = self.details.take().unwrap_or_default();
        details.validation_issues.extend(issues);
        self.details = Some(details);
        self
    }
}

/// Single shared state store. Every UI adapter subscribes to this watch channel
/// instead of maintaining a private node/network state machine.
#[derive(Clone)]
pub struct NodeStateStore {
    snapshot: Arc<RwLock<NodeSnapshot>>,
    updates: watch::Sender<NodeSnapshot>,
    events: Option<NodeEventBus>,
}

impl NodeStateStore {
    pub fn new() -> Self {
        let mut snapshot = NodeSnapshot::default();
        snapshot.refresh_capability_states();
        let (updates, _) = watch::channel(snapshot.clone());
        Self {
            snapshot: Arc::new(RwLock::new(snapshot)),
            updates,
            events: None,
        }
    }

    /// Attach the shared event bus so every state revision is also emitted as a
    /// `StateChanged` event for streaming adapters.
    pub fn with_events(events: NodeEventBus) -> Self {
        Self::with_events_identity_and_ui(events, None, None)
    }

    /// Construct the canonical store with node identity and the resolved UI
    /// mode that is actually active for this process.
    pub(crate) fn with_events_identity_and_ui(
        events: NodeEventBus,
        identity: Option<NodeIdentitySnapshot>,
        ui_mode: Option<UiModeResolution>,
    ) -> Self {
        let mut snapshot = NodeSnapshot {
            identity,
            ui_mode,
            tun_capabilities: TunCapabilities::detect(),
            ..NodeSnapshot::default()
        };
        snapshot.refresh_capability_states();
        let (updates, _) = watch::channel(snapshot.clone());
        Self {
            snapshot: Arc::new(RwLock::new(snapshot)),
            updates,
            events: Some(events),
        }
    }

    pub async fn snapshot(&self) -> NodeSnapshot {
        self.snapshot.read().await.clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<NodeSnapshot> {
        self.updates.subscribe()
    }

    /// Apply one atomic state transition and publish the next revision.
    pub async fn update<F>(&self, update: F) -> NodeSnapshot
    where
        F: FnOnce(&mut NodeSnapshot),
    {
        let mut snapshot = self.snapshot.write().await;
        update(&mut snapshot);
        snapshot.refresh_capability_states();
        snapshot.revision = snapshot.revision.saturating_add(1);
        let published = snapshot.clone();
        self.updates.send_replace(published.clone());
        if let Some(events) = &self.events {
            // Make the new canonical revision visible to unrelated events that
            // are published between the state write and StateChanged fan-out.
            events.advance_snapshot_revision(published.revision);
            events.publish(NodeEventKind::StateChanged {
                revision: published.revision,
            });
        }
        published
    }

    pub async fn set_lifecycle(&self, lifecycle: NodeLifecycleSnapshot) -> NodeSnapshot {
        self.update(|snapshot| snapshot.lifecycle = lifecycle).await
    }

    /// Replace the short activity feed after a command has been audited.
    /// Callers provide the domain projection, never raw command payloads.
    pub async fn set_recent_activity(&self, activity: Vec<ActivityEntry>) {
        self.update(|snapshot| snapshot.recent_activity = activity)
            .await;
    }

    /// Refresh the host-interface projection and reconcile live TUN names.
    ///
    /// Interface polling happens on every snapshot request, but an unchanged
    /// capture must not manufacture state revisions or wake every UI renderer.
    pub(crate) async fn refresh_interface_capture(
        &self,
        mut capture: super::network_interfaces::InterfaceCapture,
        live_tun_names: Option<&BTreeMap<String, String>>,
    ) -> NodeSnapshot {
        let mut snapshot = self.snapshot.write().await;
        if let Some(live_tun_names) = live_tun_names {
            for network in &mut snapshot.networks {
                if network.status != NodeNetworkStatus::Online {
                    continue;
                }
                match live_tun_names.get(&network.name) {
                    Some(tun_name) => network.tun_name = tun_name.clone(),
                    None => {
                        network.status = NodeNetworkStatus::Degraded;
                        if !capture.problems.iter().any(|problem| {
                            problem.network_name.as_deref() == Some(network.name.as_str())
                                && problem.message_key == "error.operation_unavailable"
                        }) {
                            capture.problems.push(
                                NodeProblem::new(
                                    NodeProblemCode::Unavailable,
                                    format!(
                                        "网络 [{}] 标记在线，但没有存活的运行时 TUN",
                                        network.name
                                    ),
                                )
                                .for_network(network.name.clone())
                                .retryable(true),
                            );
                        }
                    }
                }
            }
        }

        if snapshot.interfaces == capture.interfaces
            && snapshot.interface_problems == capture.problems
        {
            return snapshot.clone();
        }
        snapshot.interfaces = capture.interfaces;
        snapshot.interface_problems = capture.problems;
        snapshot.refresh_capability_states();
        snapshot.revision = snapshot.revision.saturating_add(1);
        let published = snapshot.clone();
        self.updates.send_replace(published.clone());
        if let Some(events) = &self.events {
            events.advance_snapshot_revision(published.revision);
            events.publish(NodeEventKind::StateChanged {
                revision: published.revision,
            });
        }
        published
    }

    /// Update local control-plane reachability without changing the
    /// authoritative node revision or emitting a node state-change event.
    pub async fn set_control_health(&self, health: NodeControlHealth) {
        let mut current = self.snapshot.write().await;
        if current.control_health == health {
            return;
        }
        current.control_health = health;
        self.updates.send_replace(current.clone());
    }

    /// Replace the local projection with a snapshot produced by the authoritative
    /// node process.
    ///
    /// Remote adapters must not manufacture a new revision: the node process's
    /// revision is the ordering source shared by every attached UI. The watch
    /// notification still wakes all local renderers.
    pub async fn replace_authoritative(&self, mut snapshot: NodeSnapshot) {
        snapshot.refresh_capability_states();
        // Control health belongs to this adapter. A successfully received
        // authoritative snapshot proves the control connection is available.
        snapshot.control_health = NodeControlHealth::Available;
        let mut current = self.snapshot.write().await;
        *current = snapshot.clone();
        self.updates.send_replace(snapshot);
    }
}

impl Default for NodeStateStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuckipv6_common::i18n::{has_key, Locale};

    fn network(name: &str, status: NodeNetworkStatus) -> NodeNetworkSnapshot {
        NodeNetworkSnapshot {
            name: name.into(),
            tun_name: format!("fk-{name}"),
            status,
            assigned_ipv6: None,
            assigned_ipv4: None,
            p2p_listen_addr: None,
            uptime_secs: 0,
            roles: vec![NodeNetworkRole::Member],
            peer_count: 0,
            traffic: NodeTrafficSnapshot::default(),
        }
    }

    fn configuration(name: &str) -> NodeNetworkConfigSnapshot {
        NodeNetworkConfigSnapshot {
            network_name: name.into(),
            coordinator_url: "ws://127.0.0.1:6666".into(),
            virtual_mode: None,
            preferred_ipv4: None,
            preferred_ipv6: None,
            virtual_network: None,
            tun_name: format!("fk-{name}"),
            mtu: 1420,
            p2p_listen_port: 0,
            p2p_connect_timeout_secs: 30,
            p2p_heartbeat_interval_secs: 5,
            routing_mode: NodeRoutingMode::MeshOnly,
            exit_node: None,
            direct_cidrs: Vec::new(),
            proxy_cidrs: Vec::new(),
            exit_node_enabled: false,
            exit_node_allowed_clients: Vec::new(),
            exit_node_allow_lan_access: false,
            exit_node_region: None,
            acl_enabled: false,
            acl_default_action: fuckipv6_common::AclAction::Allow,
            acl_rule_count: 0,
            log_level: "info".into(),
            log_file_configured: false,
            identity_configured: true,
        }
    }

    fn embedded_service(name: &str) -> NodeEmbeddedServiceSnapshot {
        NodeEmbeddedServiceSnapshot {
            network_name: name.into(),
            status: NodeEmbeddedServiceStatus::Stopped,
            signaling_addr: None,
            relay_addr: None,
        }
    }

    fn peer(network_name: &str) -> NodePeerSnapshot {
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

    #[test]
    fn default_snapshot_has_no_invariant_violations() {
        assert!(NodeSnapshot::default().invariant_violations().is_empty());
    }

    fn routing_summary(
        network_name: &str,
        default_action: NodeRoutingDefaultAction,
        effective_target: Option<NodeRoutingTargetSnapshot>,
        status: NodeRoutingSummaryStatus,
        problem: Option<NodeProblem>,
    ) -> NodeRoutingSummarySnapshot {
        let history = matches!(
            status,
            NodeRoutingSummaryStatus::Current | NodeRoutingSummaryStatus::Stale
        );
        NodeRoutingSummarySnapshot {
            network_name: network_name.into(),
            default_action,
            effective_target,
            enabled_rule_count: 2,
            last_applied_revision: history.then_some(1),
            last_applied_at_unix_ms: history.then_some(1_700_000_000_000),
            status,
            problem,
        }
    }

    #[test]
    fn routing_summary_variants_cover_every_default_action_and_target() {
        let names = ["direct", "network", "exit", "reject"];
        let snapshot = NodeSnapshot {
            configuration: NodeConfigurationSnapshot {
                networks: names.into_iter().map(configuration).collect(),
            },
            routing_summaries: vec![
                routing_summary(
                    "direct",
                    NodeRoutingDefaultAction::Direct,
                    None,
                    NodeRoutingSummaryStatus::Current,
                    None,
                ),
                routing_summary(
                    "network",
                    NodeRoutingDefaultAction::Network,
                    Some(NodeRoutingTargetSnapshot::Network {
                        network_name: "network".into(),
                    }),
                    NodeRoutingSummaryStatus::Current,
                    None,
                ),
                routing_summary(
                    "exit",
                    NodeRoutingDefaultAction::Exit("peer-1".into()),
                    Some(NodeRoutingTargetSnapshot::Exit {
                        node_id: "peer-1".into(),
                    }),
                    NodeRoutingSummaryStatus::Current,
                    None,
                ),
                routing_summary(
                    "reject",
                    NodeRoutingDefaultAction::Reject,
                    None,
                    NodeRoutingSummaryStatus::Current,
                    None,
                ),
            ],
            ..NodeSnapshot::default()
        };

        assert!(snapshot.invariant_violations().is_empty());
    }

    #[test]
    fn routing_summary_reports_duplicates_unconfigured_and_missing_history() {
        let problem = NodeProblem::new(NodeProblemCode::InvalidRequest, "invalid");
        let snapshot = NodeSnapshot {
            configuration: NodeConfigurationSnapshot {
                networks: vec![configuration("home")],
            },
            routing_summaries: vec![
                routing_summary(
                    "home",
                    NodeRoutingDefaultAction::Direct,
                    None,
                    NodeRoutingSummaryStatus::Current,
                    Some(problem.clone()),
                ),
                routing_summary(
                    "ghost",
                    NodeRoutingDefaultAction::Direct,
                    None,
                    NodeRoutingSummaryStatus::Invalid,
                    Some(problem.clone()),
                ),
                routing_summary(
                    "home",
                    NodeRoutingDefaultAction::Direct,
                    None,
                    NodeRoutingSummaryStatus::Stale,
                    None,
                ),
            ],
            ..NodeSnapshot::default()
        };

        let violations = snapshot.invariant_violations();
        assert!(violations.contains(&"duplicate_routing_summary:home".into()));
        assert!(violations.contains(&"routing_summary_for_unconfigured_network:ghost".into()));
        assert!(violations.contains(&"current_routing_summary_has_problem:home".into()));
        assert!(violations.contains(&"blocked_routing_summary_missing_reason:home".into()));
    }

    #[test]
    fn routing_summary_reports_target_mismatches_and_incomplete_history() {
        let problem = NodeProblem::new(NodeProblemCode::Unavailable, "blocked");
        let mut incomplete = routing_summary(
            "direct",
            NodeRoutingDefaultAction::Direct,
            None,
            NodeRoutingSummaryStatus::ApplyFailed,
            Some(problem.clone()),
        );
        incomplete.last_applied_revision = Some(3);
        let snapshot = NodeSnapshot {
            configuration: NodeConfigurationSnapshot {
                networks: vec![
                    configuration("direct"),
                    configuration("network"),
                    configuration("exit"),
                ],
            },
            routing_summaries: vec![
                routing_summary(
                    "direct",
                    NodeRoutingDefaultAction::Direct,
                    Some(NodeRoutingTargetSnapshot::Exit {
                        node_id: "unexpected".into(),
                    }),
                    NodeRoutingSummaryStatus::Current,
                    None,
                ),
                routing_summary(
                    "network",
                    NodeRoutingDefaultAction::Network,
                    Some(NodeRoutingTargetSnapshot::Network {
                        network_name: "other".into(),
                    }),
                    NodeRoutingSummaryStatus::Current,
                    None,
                ),
                routing_summary(
                    "exit",
                    NodeRoutingDefaultAction::Exit("peer-1".into()),
                    Some(NodeRoutingTargetSnapshot::Exit {
                        node_id: "peer-2".into(),
                    }),
                    NodeRoutingSummaryStatus::Current,
                    None,
                ),
                incomplete,
            ],
            ..NodeSnapshot::default()
        };

        let violations = snapshot.invariant_violations();
        assert!(violations.contains(&"routing_summary_unexpected_target:direct".into()));
        assert!(violations.contains(&"routing_summary_network_target_mismatch:network".into()));
        assert!(violations.contains(&"routing_summary_exit_target_mismatch:exit".into()));
        assert!(violations.contains(&"incomplete_routing_summary_history:direct".into()));
    }

    #[test]
    fn duplicate_names_are_reported_for_every_snapshot_collection() {
        let snapshot = NodeSnapshot {
            configuration: NodeConfigurationSnapshot {
                networks: vec![configuration("home"), configuration("home")],
            },
            embedded_services: vec![embedded_service("home"), embedded_service("home")],
            networks: vec![network("home", NodeNetworkStatus::Online); 2],
            ..NodeSnapshot::default()
        };

        let violations = snapshot.invariant_violations();
        assert!(violations.contains(&"duplicate_network:home".into()));
        assert!(violations.contains(&"duplicate_configuration:home".into()));
        assert!(violations.contains(&"duplicate_embedded_service:home".into()));
    }

    #[test]
    fn network_configuration_and_peer_relationships_are_checked() {
        let mut offline_without_config = network("offline", NodeNetworkStatus::Offline);
        offline_without_config.peer_count = 1;
        let mut count_mismatch = network("mismatch", NodeNetworkStatus::Online);
        count_mismatch.peer_count = 3;
        let snapshot = NodeSnapshot {
            configuration: NodeConfigurationSnapshot {
                networks: vec![configuration("mismatch")],
            },
            networks: vec![offline_without_config, count_mismatch],
            peers: vec![peer("offline"), peer("unknown")],
            ..NodeSnapshot::default()
        };

        let violations = snapshot.invariant_violations();
        assert!(violations.contains(&"missing_network_configuration:offline".into()));
        assert!(violations.contains(&"peer_count_mismatch:mismatch:expected=0:actual=3".into()));
        assert!(violations.contains(&"offline_network_has_peers:offline".into()));
        assert!(violations.contains(&"peer_for_offline_network:offline".into()));
        assert!(violations.contains(&"peer_for_unknown_network:unknown".into()));
    }

    #[tokio::test]
    async fn state_store_exposes_the_effective_ui_mode() {
        let resolution = crate::ui::resolve_ui_mode(
            crate::ui::UiModeRequest::WebUi,
            crate::ui::UiCapabilities {
                display: crate::ui::DisplayEnvironment::Headless,
                gpui_available: false,
            },
        )
        .expect("headless WebUI should resolve");
        let state = NodeStateStore::with_events_identity_and_ui(
            NodeEventBus::default(),
            None,
            Some(resolution),
        );

        let snapshot = state.snapshot().await;
        assert_eq!(snapshot.ui_mode, Some(resolution));
        let encoded = serde_json::to_value(snapshot).expect("serialize snapshot");
        assert_eq!(encoded["ui_mode"]["effective"], "web_ui");
        assert_eq!(encoded["ui_mode"]["requested"], "web_ui");
    }

    #[tokio::test]
    async fn state_store_publishes_atomic_revisions() {
        let state = NodeStateStore::new();
        let updates = state.subscribe();

        state
            .update(|snapshot| {
                snapshot.networks.push(NodeNetworkSnapshot {
                    name: "home".into(),
                    tun_name: "fk-home".into(),
                    status: NodeNetworkStatus::Online,
                    assigned_ipv6: Some("fd00::2".parse().expect("valid IPv6")),
                    assigned_ipv4: None,
                    p2p_listen_addr: Some("127.0.0.1:9000".parse().expect("valid socket")),
                    uptime_secs: 10,
                    roles: vec![NodeNetworkRole::Member, NodeNetworkRole::ExitNode],
                    peer_count: 1,
                    traffic: NodeTrafficSnapshot::default(),
                });
            })
            .await;

        assert_eq!(updates.borrow().revision, 1);
        assert_eq!(updates.borrow().networks[0].name, "home");
        assert_eq!(
            updates.borrow().networks[0].roles,
            vec![NodeNetworkRole::Member, NodeNetworkRole::ExitNode]
        );
    }

    #[tokio::test]
    async fn unchanged_interface_capture_does_not_create_revisions() {
        let state = NodeStateStore::new();
        let capture = super::super::network_interfaces::InterfaceCapture {
            interfaces: vec![NodeInterfaceSnapshot {
                index: 1,
                name: "lo".into(),
                kind: NodeInterfaceKind::Loopback,
                ownership: NodeInterfaceOwnership::System,
                status: NodeInterfaceStatus::Up,
                mtu: 65_536,
                addresses: vec![NodeInterfaceAddress {
                    address: IpAddr::V4(Ipv4Addr::LOCALHOST),
                    prefix_len: 8,
                }],
                default_ipv4: false,
                default_ipv6: false,
            }],
            problems: Vec::new(),
        };

        let first = state.refresh_interface_capture(capture.clone(), None).await;
        let second = state.refresh_interface_capture(capture, None).await;
        assert_eq!(first.revision, 1);
        assert_eq!(second.revision, 1);
        assert_eq!(second.interfaces.len(), 1);
    }

    #[tokio::test]
    async fn live_tun_reconciliation_marks_stale_online_network_degraded() {
        let state = NodeStateStore::new();
        state
            .update(|snapshot| {
                snapshot.configuration.networks.push(configuration("home"));
                snapshot
                    .networks
                    .push(network("home", NodeNetworkStatus::Online));
            })
            .await;
        let capture = super::super::network_interfaces::InterfaceCapture::default();

        let refreshed = state
            .refresh_interface_capture(capture, Some(&BTreeMap::new()))
            .await;
        assert_eq!(refreshed.networks[0].status, NodeNetworkStatus::Degraded);
        assert!(refreshed
            .interface_problems
            .iter()
            .any(|problem| problem.network_name.as_deref() == Some("home")));
        assert!(refreshed.invariant_violations().is_empty());
    }

    #[tokio::test]
    async fn configured_event_bus_observes_every_state_revision() {
        let events = NodeEventBus::new(8);
        let mut receiver = events.subscribe();
        let state = NodeStateStore::with_events(events);

        state.update(|snapshot| snapshot.last_error = None).await;

        assert_eq!(
            receiver.recv().await.expect("state event").kind,
            NodeEventKind::StateChanged { revision: 1 }
        );
    }

    #[tokio::test]
    async fn revision_saturates_instead_of_wrapping() {
        let state = NodeStateStore::new();
        let updates = state.subscribe();

        state
            .update(|snapshot| snapshot.revision = u64::MAX - 1)
            .await;
        assert_eq!(updates.borrow().revision, u64::MAX);

        state.update(|_| {}).await;
        assert_eq!(updates.borrow().revision, u64::MAX);
    }

    #[test]
    fn node_problem_round_trips_with_stable_codes_and_flags() {
        let cases = [
            (
                NodeProblemCode::AuthenticationRequired,
                "authentication_required",
            ),
            (NodeProblemCode::InvalidRequest, "invalid_request"),
            (NodeProblemCode::NotFound, "not_found"),
            (NodeProblemCode::Conflict, "conflict"),
            (NodeProblemCode::Unavailable, "unavailable"),
            (NodeProblemCode::PermissionDenied, "permission_denied"),
            (NodeProblemCode::Cancelled, "cancelled"),
            (NodeProblemCode::TimedOut, "timed_out"),
            (NodeProblemCode::Unsupported, "unsupported"),
            (NodeProblemCode::Internal, "internal"),
        ];

        for (code, wire_name) in cases {
            let problem = NodeProblem::new(code, "测试错误")
                .for_network("home")
                .retryable(true);
            assert!(
                !wire_name.is_empty()
                    && wire_name.bytes().all(|byte| byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || byte == b'_'),
                "unstable wire code: {wire_name}"
            );
            assert!(
                problem.message_key.starts_with("error.")
                    && problem
                        .message_key
                        .bytes()
                        .all(|byte| byte.is_ascii_lowercase()
                            || byte.is_ascii_digit()
                            || matches!(byte, b'.' | b'_' | b'-')),
                "unstable message key: {}",
                problem.message_key
            );
            for locale in Locale::ALL {
                assert!(
                    has_key(locale, &problem.message_key),
                    "missing {locale:?} translation for {}",
                    problem.message_key
                );
            }
            let encoded = serde_json::to_value(&problem).expect("serialize node problem");
            assert_eq!(encoded["code"], wire_name);
            assert_eq!(encoded["category"], serde_json::json!(problem.category));
            assert_eq!(encoded["message"], "测试错误");
            assert_eq!(encoded["message_key"], problem.message_key);
            assert_eq!(encoded["network_name"], "home");
            assert_eq!(encoded["retryable"], true);
            assert_eq!(
                encoded["recovery"],
                serde_json::to_value(&problem.recovery).expect("serialize recovery")
            );
            assert!(encoded.get("params").is_none());
            let decoded: NodeProblem =
                serde_json::from_value(encoded).expect("deserialize node problem");
            assert_eq!(decoded, problem);
            assert_eq!(decoded.code, code);
            assert_eq!(decoded.message_key, code.message_key());
            assert_ne!(decoded.message_key, decoded.message);
        }
    }

    #[test]
    fn snapshot_round_trips_without_secret_identity_material() {
        let snapshot = NodeSnapshot {
            identity: Some(NodeIdentitySnapshot {
                node_id: "node-1".into(),
                display_name: Some("home-gateway".into()),
                public_key_fingerprint: "b3:fixture".into(),
            }),
            ..NodeSnapshot::default()
        };

        let encoded = serde_json::to_string(&snapshot).expect("serialize node snapshot");
        let decoded: NodeSnapshot = serde_json::from_str(&encoded).expect("deserialize snapshot");
        assert_eq!(decoded, snapshot);
        assert!(!encoded.contains("private_key"));
        assert!(!encoded.contains("secret_key"));
    }

    #[tokio::test]
    async fn state_store_exposes_exit_node_capability_truth() {
        let snapshot = NodeStateStore::new().snapshot().await;

        assert_eq!(
            snapshot.exit_node_capabilities.schema_version,
            fuckipv6_common::EXIT_NODE_CAPABILITY_SCHEMA_VERSION
        );
        assert_eq!(
            snapshot.exit_node_capabilities.platform,
            fuckipv6_common::ExitNodePlatform::current()
        );
        assert_eq!(
            snapshot.exit_node_capabilities.supported,
            snapshot
                .exit_node_capabilities
                .unsupported_reasons
                .is_empty()
        );
    }

    #[test]
    fn exit_node_capabilities_are_required_for_old_snapshots_to_fail_closed() {
        let snapshot = NodeSnapshot::default();
        let mut encoded = serde_json::to_value(&snapshot).expect("serialize snapshot");
        let object = encoded.as_object_mut().expect("snapshot object");
        object.remove("exit_node_capabilities");
        object.remove("capabilities");

        let decoded: NodeSnapshot =
            serde_json::from_value(encoded).expect("deserialize legacy snapshot");
        assert_eq!(
            decoded.exit_node_capabilities,
            ExitNodeCapabilities::detect()
        );
        for capability in CapabilityKind::ALL {
            assert_eq!(
                decoded.capabilities.state(capability),
                CapabilityState::Unknown
            );
        }
    }

    #[tokio::test]
    async fn capability_registry_is_complete_and_derived_from_authoritative_state() {
        let state = NodeStateStore::new();
        let snapshot = state.snapshot().await;
        assert_eq!(snapshot.capabilities.capabilities.len(), 4);
        assert_eq!(
            snapshot
                .capabilities
                .capabilities
                .iter()
                .map(|capability| capability.kind)
                .collect::<HashSet<_>>(),
            CapabilityKind::ALL.into_iter().collect()
        );
        assert_eq!(
            snapshot.capabilities.state(CapabilityKind::Coordinator),
            CapabilityState::Disabled
        );
        assert_eq!(
            snapshot.capabilities.state(CapabilityKind::Relay),
            CapabilityState::Disabled
        );
        assert_eq!(
            snapshot.capabilities.state(CapabilityKind::File),
            CapabilityState::Unsupported
        );

        let exit_network = network("home", NodeNetworkStatus::Online);
        let updated = state
            .update(|snapshot| {
                snapshot.exit_node_capabilities = ExitNodeCapabilities {
                    schema_version: fuckipv6_common::EXIT_NODE_CAPABILITY_SCHEMA_VERSION,
                    platform: fuckipv6_common::ExitNodePlatform::Linux,
                    supported: true,
                    ipv4_forwarding: true,
                    ipv6_forwarding: true,
                    nat: true,
                    cleanup: true,
                    unsupported_reasons: Vec::new(),
                };
                snapshot.networks = vec![exit_network];
                snapshot.embedded_services = vec![embedded_service("home")];
            })
            .await;
        assert_eq!(
            updated.capabilities.state(CapabilityKind::Exit),
            CapabilityState::Disabled
        );

        let updated = state
            .update(|snapshot| {
                snapshot.embedded_services[0].status = NodeEmbeddedServiceStatus::Running;
                snapshot.networks[0].roles = vec![NodeNetworkRole::ExitNode];
            })
            .await;
        assert_eq!(
            updated.capabilities.state(CapabilityKind::Coordinator),
            CapabilityState::Enabled
        );
        assert_eq!(
            updated.capabilities.state(CapabilityKind::Relay),
            CapabilityState::Enabled
        );
        assert_eq!(
            updated.capabilities.state(CapabilityKind::Exit),
            CapabilityState::Enabled
        );
    }

    #[test]
    fn every_lifecycle_and_network_state_has_one_primary_status() {
        let mut snapshot = NodeSnapshot::default();
        assert_eq!(snapshot.primary_status(), NodePrimaryStatus::Uninitialized);

        snapshot.identity = Some(NodeIdentitySnapshot {
            node_id: "node-1".into(),
            display_name: None,
            public_key_fingerprint: "fixture".into(),
        });
        assert_eq!(snapshot.primary_status(), NodePrimaryStatus::Stopped);

        for (phase, expected) in [
            (NodePhase::Starting, NodePrimaryStatus::Starting),
            (NodePhase::Stopping, NodePrimaryStatus::Stopping),
            (NodePhase::Failed, NodePrimaryStatus::Failed),
        ] {
            snapshot.lifecycle.phase = phase;
            assert_eq!(snapshot.primary_status(), expected);
        }

        snapshot.lifecycle.phase = NodePhase::Running;
        snapshot.networks = vec![network("home", NodeNetworkStatus::Online)];
        assert_eq!(snapshot.primary_status(), NodePrimaryStatus::Running);

        for status in [
            NodeNetworkStatus::Connecting,
            NodeNetworkStatus::Degraded,
            NodeNetworkStatus::Stopping,
            NodeNetworkStatus::Failed,
        ] {
            snapshot.networks[0].status = status;
            assert_eq!(snapshot.primary_status(), NodePrimaryStatus::Degraded);
        }

        snapshot.networks[0].status = NodeNetworkStatus::Offline;
        assert_eq!(snapshot.primary_status(), NodePrimaryStatus::Running);
        snapshot.embedded_services = vec![embedded_service("home")];
        snapshot.embedded_services[0].status = NodeEmbeddedServiceStatus::Failed;
        assert_eq!(snapshot.primary_status(), NodePrimaryStatus::Degraded);
    }

    #[test]
    fn primary_status_applies_transition_failure_degraded_then_control_priority() {
        let mut snapshot = NodeSnapshot {
            lifecycle: NodeLifecycleSnapshot {
                phase: NodePhase::Running,
                ..NodeLifecycleSnapshot::default()
            },
            control_health: NodeControlHealth::Unavailable,
            ..NodeSnapshot::default()
        };
        assert_eq!(
            snapshot.primary_status(),
            NodePrimaryStatus::ControlUnavailable
        );

        snapshot.networks = vec![network("home", NodeNetworkStatus::Degraded)];
        assert_eq!(snapshot.primary_status(), NodePrimaryStatus::Degraded);

        snapshot.last_error = Some(NodeProblem::new(
            NodeProblemCode::Internal,
            "fixture failure",
        ));
        assert_eq!(snapshot.primary_status(), NodePrimaryStatus::Degraded);

        snapshot.lifecycle.phase = NodePhase::Starting;
        assert_eq!(snapshot.primary_status(), NodePrimaryStatus::Starting);

        snapshot.lifecycle.phase = NodePhase::Stopping;
        assert_eq!(snapshot.primary_status(), NodePrimaryStatus::Stopping);

        snapshot.lifecycle.phase = NodePhase::Failed;
        assert_eq!(snapshot.primary_status(), NodePrimaryStatus::Failed);

        snapshot.lifecycle.phase = NodePhase::Stopped;
        assert_eq!(snapshot.primary_status(), NodePrimaryStatus::Stopped);
        snapshot.networks.clear();
        snapshot.last_error = None;
        assert_eq!(snapshot.primary_status(), NodePrimaryStatus::Uninitialized);
        snapshot.identity = Some(NodeIdentitySnapshot {
            node_id: "node-1".into(),
            display_name: None,
            public_key_fingerprint: "fixture".into(),
        });
        assert_eq!(snapshot.primary_status(), NodePrimaryStatus::Stopped);
    }

    #[test]
    fn every_primary_status_has_stable_label_and_action_copy() {
        for status in NodePrimaryStatus::ALL {
            assert!(status.label_key().starts_with("status.primary."));
            assert!(!status.label_zh().is_empty());
            assert!(status.detail_key().ends_with(".detail"));
            assert!(!status.detail_zh().is_empty());
        }
    }

    #[tokio::test]
    async fn control_health_observation_does_not_change_revision_or_emit_node_event() {
        let events = NodeEventBus::new(8);
        let state = NodeStateStore::with_events(events.clone());
        let updates = state.subscribe();
        let mut event_receiver = events.subscribe();
        let before = state.snapshot().await;

        state
            .set_control_health(NodeControlHealth::Unavailable)
            .await;

        let after = state.snapshot().await;
        assert_eq!(after.revision, before.revision);
        assert_eq!(after.control_health, NodeControlHealth::Unavailable);
        assert!(updates.has_changed().expect("watch remains open"));
        assert!(event_receiver.try_recv().is_err());

        state.set_control_health(NodeControlHealth::Available).await;
        let restored = state.snapshot().await;
        assert_eq!(restored.revision, before.revision);
        assert_eq!(restored.control_health, NodeControlHealth::Available);
        assert!(event_receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn authoritative_snapshot_marks_the_control_channel_available() {
        let state = NodeStateStore::new();
        state
            .set_control_health(NodeControlHealth::Unavailable)
            .await;
        let authoritative = NodeSnapshot {
            revision: 42,
            ..NodeSnapshot::default()
        };

        state.replace_authoritative(authoritative).await;

        let snapshot = state.snapshot().await;
        assert_eq!(snapshot.revision, 42);
        assert_eq!(snapshot.control_health, NodeControlHealth::Available);
    }
}
