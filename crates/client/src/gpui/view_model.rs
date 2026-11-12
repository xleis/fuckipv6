use std::collections::{BTreeMap, HashMap, HashSet};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use fuckipv6_common::domain::JoinRequestReviewAction;
use fuckipv6_common::domain::{
    parse_policy_test_input, ActivityCategory, ActivityEntry, ActivitySeverity,
    ApplicationIdentity, AuditOutcome, CapabilityKind, CapabilityState, ContentHash,
    EffectiveAccessReason, EffectivePolicyLayer, ExitAvailabilitySource, ExitAvailabilityState,
    ExitContributionAvailability, ExitContributionMode, ExitContributionObligation,
    ExitContributionScope, ExitSelectionCandidate, ExitSelectionMode, ExitSelectionPolicy,
    ExitSelectionSource, InvitationCode, InvitationId, InvitationRedeemRequest,
    MemberLifecycleAction, MembershipRole, NetworkId, NetworkJoinMode, NetworkMember,
    NetworkMemberDirectory, NetworkMemberGroup, NetworkMemberOnlineState, NetworkMemberStatus,
    NetworkProfileUpdate, NetworkVisibility, NodeId, RelayAuthorizationAuditSnapshot,
    RelayAuthorizationDiagnostic, RelayAuthorizationSourceDiagnostic, RelayExistingSessionPolicy,
    RelayGovernanceAction, RelayGovernanceEvent, RelayHealthIssue, RelayHealthSnapshot,
    RelayHealthStatus, RelayLifecycleState, RelayLimitPolicy, RelayLimitSource,
    RelayMeasurementState, RelayReachabilityState, TopologyNode, TopologyNodeHealth,
    TopologySnapshot, TrafficAction, TrafficDefaultConfirmedBy, TrafficFlow, TrafficMatch,
    TrafficMatchRisk, TrafficPolicy, TrafficPolicyTestInput, TrafficProtectionState,
    TrafficProtocol, TrafficRoutingHitSample, TrafficRoutingObservation, TrafficRoutingPageModel,
    TrafficRoutingRuleActivityEffect, TrafficRoutingRuleImpactView, TrafficRoutingTarget,
    TrafficRoutingTargetKind, TrafficRoutingTargetRef, TrafficRoutingWorkflowState,
    TrafficSessionEventKind,
};
use fuckipv6_common::i18n::{format_integer, t, tr, Locale};
use fuckipv6_common::ui_layout::navigation_module_for_page;
use fuckipv6_common::NetworkMode;

use crate::node::{
    node_ui_manifest, LocalExitCapabilityStage, NodeAcknowledgeMembershipPolicyCommand,
    NodeApplyConfigurationCommand, NodeApplyPolicyCommand, NodeApplyTrafficPolicyCommand,
    NodeBatchReviewJoinRequestsCommand, NodeChangeMemberLifecycleCommand,
    NodeClientBlacklistCommand, NodeCommand, NodeCommandKind, NodeControlHealth,
    NodeCreateInvitationCommand, NodeCreateNetworkCommand, NodeEmbeddedServiceStatus,
    NodeGetInvitationStatusCommand, NodeInterfaceKind, NodeInterfaceOwnership, NodeInterfaceStatus,
    NodeJoinNetworkCommand, NodeJoinRequestReviewItem, NodeLeaveNetworkCommand,
    NodeListInvitationsCommand, NodeListJoinRequestsCommand, NodeMemberRemovalPreviewCommand,
    NodeNetworkAllocation, NodeNetworkDiscoveryCommand, NodeNetworkMembersCommand,
    NodeNetworkProfileCommand, NodeNetworkRole, NodeNetworkSelection, NodeNetworkStatus,
    NodePermission, NodePhase, NodePreviewConfigurationCommand, NodePreviewInvitationCommand,
    NodePreviewJoinNetworkCommand, NodePreviewPolicyCommand, NodePreviewPolicyRollbackCommand,
    NodePreviewTrafficPolicyCommand, NodePreviewTrafficPolicyRollbackCommand, NodePrimaryStatus,
    NodeProbePeerCommand, NodeProblemCode, NodeRedeemInvitationCommand,
    NodeResubmitJoinRequestCommand, NodeReviewJoinRequestCommand, NodeRevokeInvitationCommand,
    NodeRollbackConfigurationCommand, NodeRollbackPolicyCommand, NodeRollbackTrafficPolicyCommand,
    NodeRotateNodeIdentityCommand, NodeRoutingDefaultAction, NodeRoutingMode,
    NodeRoutingSummaryStatus, NodeRoutingTargetSnapshot, NodeSetLocalExitCapabilityCommand,
    NodeSetNetworkRoleCommand, NodeSetNodeDisplayNameCommand, NodeSnapshot,
    NodeSubmitJoinRequestCommand, NodeTestTrafficPolicyCommand, NodeUiAction,
    NodeUiActionBlockReason, NodeUiAvailability, NodeUiBackendStatus, NodeUiManifest, NodeUiPage,
    NodeUiPermission, NodeUpdateNetworkAllocationCommand, NodeUpdateRoutingCommand,
    NodeWithdrawJoinRequestCommand,
};
use crate::policy_transaction::PolicySnapshot;
use crate::traffic_policy_transaction::TrafficPolicyDraft;
use crate::tun::{TunCapabilityReason, TunPlatform};
use crate::ui::{DisplayEnvironment, UiMode};

/// Typed views supported by the native adapter. Keeping this closed set separate
/// from page strings makes an unknown manifest page fail closed rather than
/// silently rendering a generic or empty surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PageKind {
    Overview,
    Networks,
    Peers,
    NodeRuntime,
    Topology,
    Traffic,
    Activity,
    Routing,
    ExitNode,
    Relay,
    Logs,
    Identity,
    Settings,
}

/// Filter value for the member capability column. Missing capabilities are a
/// distinct state from an explicitly empty capability list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MemberCapabilityFilter {
    #[default]
    All,
    Unknown,
    Known(CapabilityKind),
}

/// A single health selector can address either the authoritative membership
/// status or the independently observed presence state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MemberHealthFilter {
    #[default]
    All,
    Status(NetworkMemberStatus),
    Presence(NetworkMemberOnlineState),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemberDirectoryFilters {
    pub role: Option<MembershipRole>,
    pub group: Option<NetworkMemberGroup>,
    pub capability: MemberCapabilityFilter,
    pub health: MemberHealthFilter,
}

/// Stable presentation order. The UI and clipboard export both consume this
/// ordering so a refresh cannot reorder equal logical records.
pub fn sort_network_members(members: &[NetworkMember]) -> Vec<&NetworkMember> {
    let mut sorted = members.iter().collect::<Vec<_>>();
    sorted.sort_by(|left, right| left.node_id.as_str().cmp(right.node_id.as_str()));
    sorted
}

pub fn filter_network_members(
    members: &[NetworkMember],
    filters: MemberDirectoryFilters,
) -> Vec<&NetworkMember> {
    sort_network_members(members)
        .into_iter()
        .filter(|member| {
            if filters.role.is_some_and(|role| member.role != role) {
                return false;
            }
            if filters
                .group
                .is_some_and(|group| !member.groups.contains(&group))
            {
                return false;
            }
            match filters.capability {
                MemberCapabilityFilter::All => {}
                MemberCapabilityFilter::Unknown if member.capabilities.is_some() => return false,
                MemberCapabilityFilter::Unknown => {}
                MemberCapabilityFilter::Known(capability)
                    if !member
                        .capabilities
                        .as_ref()
                        .is_some_and(|capabilities| capabilities.contains(&capability)) =>
                {
                    return false;
                }
                MemberCapabilityFilter::Known(_) => {}
            }
            match filters.health {
                MemberHealthFilter::All => true,
                MemberHealthFilter::Status(status) => member.status == status,
                MemberHealthFilter::Presence(presence) => member.presence.state == presence,
            }
        })
        .collect()
}

pub fn retain_member_selection(
    selected_node_id: Option<&str>,
    members: &[NetworkMember],
) -> Option<String> {
    let selected_node_id = selected_node_id?;
    members
        .iter()
        .any(|member| member.node_id.as_str() == selected_node_id)
        .then(|| selected_node_id.to_owned())
}

/// Filters the authoritative activity projection without interpreting toast
/// copy or raw audit details. Time bounds are inclusive.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActivityFilters {
    pub network_id: Option<String>,
    pub object_kind: Option<String>,
    pub object_id: Option<String>,
    pub actor_role: Option<String>,
    pub category: Option<ActivityCategory>,
    pub severity: Option<ActivitySeverity>,
    pub outcome: Option<AuditOutcome>,
    pub from_unix_ms: Option<u64>,
    pub to_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActivityFilterOptions {
    pub networks: Vec<String>,
    pub object_kinds: Vec<String>,
    pub object_ids: Vec<String>,
    pub actor_roles: Vec<String>,
}

/// Deduplicate replayed snapshot/event history by stable audit ID, with the
/// latest copy of an identity winning, then apply the one product ordering.
pub fn ordered_activity_entries(entries: &[ActivityEntry]) -> Vec<&ActivityEntry> {
    let mut by_id = HashMap::new();
    for entry in entries {
        by_id.insert(entry.audit_id.as_str(), entry);
    }
    let mut ordered = by_id.into_values().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        right
            .occurred_at_unix_ms
            .cmp(&left.occurred_at_unix_ms)
            .then_with(|| right.audit_id.cmp(&left.audit_id))
    });
    ordered
}

pub fn filter_activity<'a>(
    entries: &'a [ActivityEntry],
    filters: &ActivityFilters,
) -> Vec<&'a ActivityEntry> {
    ordered_activity_entries(entries)
        .into_iter()
        .filter(|entry| {
            if filters.network_id.as_deref().is_some_and(|network_id| {
                entry.network_id.as_ref().map(NetworkId::as_str) != Some(network_id)
            }) {
                return false;
            }
            if filters
                .object_kind
                .as_deref()
                .is_some_and(|kind| entry.object_kind != kind)
            {
                return false;
            }
            if filters
                .object_id
                .as_deref()
                .is_some_and(|id| entry.object_id.as_deref() != Some(id))
            {
                return false;
            }
            if filters
                .actor_role
                .as_deref()
                .is_some_and(|role| entry.actor_role.as_deref() != Some(role))
            {
                return false;
            }
            if filters
                .category
                .is_some_and(|category| entry.category != category)
            {
                return false;
            }
            if filters
                .severity
                .is_some_and(|severity| entry.severity != severity)
            {
                return false;
            }
            if filters
                .outcome
                .is_some_and(|outcome| entry.outcome != outcome)
            {
                return false;
            }
            if filters
                .from_unix_ms
                .is_some_and(|from| entry.occurred_at_unix_ms < from)
            {
                return false;
            }
            if filters
                .to_unix_ms
                .is_some_and(|to| entry.occurred_at_unix_ms > to)
            {
                return false;
            }
            true
        })
        .collect()
}

/// Build stable selector values from the same authoritative projection used by
/// the timeline. Unknown/empty values are omitted so the UI never invents a
/// filter option that cannot match an activity row.
pub fn activity_filter_options(entries: &[ActivityEntry]) -> ActivityFilterOptions {
    fn sorted_unique(values: impl IntoIterator<Item = String>) -> Vec<String> {
        let mut values = values
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        values.sort();
        values
    }

    let ordered = ordered_activity_entries(entries);
    ActivityFilterOptions {
        networks: sorted_unique(
            ordered
                .iter()
                .filter_map(|entry| entry.network_id.as_ref().map(NetworkId::to_string)),
        ),
        object_kinds: sorted_unique(
            ordered
                .iter()
                .map(|entry| entry.object_kind.clone())
                .collect::<Vec<_>>(),
        ),
        object_ids: sorted_unique(ordered.iter().filter_map(|entry| entry.object_id.clone())),
        actor_roles: sorted_unique(
            ordered
                .iter()
                .filter_map(|entry| entry.actor_role.clone())
                .collect::<Vec<_>>(),
        ),
    }
}

pub fn network_membership_id<'a>(
    snapshot: &'a NodeSnapshot,
    network_name: &str,
) -> Option<&'a str> {
    snapshot
        .memberships
        .iter()
        .find(|membership| membership.network_name == network_name)
        .map(|membership| membership.network_id.as_str())
}

impl PageKind {
    pub const ALL: [Self; 13] = [
        Self::Overview,
        Self::Networks,
        Self::Peers,
        Self::NodeRuntime,
        Self::Topology,
        Self::Traffic,
        Self::Activity,
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
            "node_runtime" => Some(Self::NodeRuntime),
            "topology" => Some(Self::Topology),
            "traffic" => Some(Self::Traffic),
            "activity" => Some(Self::Activity),
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
            Self::NodeRuntime => "node_runtime",
            Self::Topology => "topology",
            Self::Traffic => "traffic",
            Self::Activity => "activity",
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
    PreviewNetworkProfile,
    UpdateNetworkProfile,
    PublishNetworkDiscovery,
    UpdateNetworkDiscovery,
    UnpublishNetworkDiscovery,
    RefreshDiscoveryIndex,
    RetryDiscoveryIndex,
    PreviewJoinNetwork,
    JoinNetwork,
    SubmitJoinRequest,
    ListJoinRequests,
    ListNetworkMembers,
    PreviewMemberRemoval,
    ChangeMemberLifecycle,
    ReviewJoinRequest,
    BatchReviewJoinRequests,
    WithdrawJoinRequest,
    ResubmitJoinRequest,
    CreateInvitation,
    ListInvitations,
    GetInvitationStatus,
    RevokeInvitation,
    PreviewInvitation,
    RedeemInvitation,
    LeaveNetwork,
    AcknowledgeMembershipPolicy,
    StartNetworks,
    StopNetworks,
    RestartNetworks,
    RebuildTun,
    ProbePeer,
    GetBlacklist,
    BanClient,
    UnbanClient,
    GetNetworkAllocation,
    UpdateNetworkAllocation,
    UpdateRouting,
    SetExitNodeRole,
    SetLocalExitCapability,
    SetRelayRole,
    SetNodeDisplayName,
    RotateNodeIdentity,
    StartNode,
    StopNode,
    RestartNode,
    CleanupResiduals,
    GetConfiguration,
    PreviewConfiguration,
    ApplyConfiguration,
    RollbackConfiguration,
    GetConfigurationHistory,
    GetPolicy,
    PreviewPolicy,
    PreviewPolicyRollback,
    ApplyPolicy,
    RollbackPolicy,
    GetPolicyHistory,
    TestPolicy,
    GetTrafficPolicy,
    PreviewTrafficPolicy,
    PreviewTrafficPolicyRollback,
    ApplyTrafficPolicy,
    RollbackTrafficPolicy,
    GetTrafficPolicyHistory,
    TestTrafficPolicy,
    ShutdownNode,
}

impl ActionBinding {
    pub const ALL: [Self; 69] = [
        Self::RefreshSnapshot,
        Self::GetDiagnostics,
        Self::CreateNetwork,
        Self::PreviewNetworkProfile,
        Self::UpdateNetworkProfile,
        Self::PublishNetworkDiscovery,
        Self::UpdateNetworkDiscovery,
        Self::UnpublishNetworkDiscovery,
        Self::RefreshDiscoveryIndex,
        Self::RetryDiscoveryIndex,
        Self::PreviewJoinNetwork,
        Self::JoinNetwork,
        Self::SubmitJoinRequest,
        Self::ListJoinRequests,
        Self::ListNetworkMembers,
        Self::PreviewMemberRemoval,
        Self::ChangeMemberLifecycle,
        Self::ReviewJoinRequest,
        Self::BatchReviewJoinRequests,
        Self::WithdrawJoinRequest,
        Self::ResubmitJoinRequest,
        Self::CreateInvitation,
        Self::ListInvitations,
        Self::GetInvitationStatus,
        Self::RevokeInvitation,
        Self::PreviewInvitation,
        Self::RedeemInvitation,
        Self::LeaveNetwork,
        Self::AcknowledgeMembershipPolicy,
        Self::StartNetworks,
        Self::StopNetworks,
        Self::RestartNetworks,
        Self::RebuildTun,
        Self::ProbePeer,
        Self::GetBlacklist,
        Self::BanClient,
        Self::UnbanClient,
        Self::GetNetworkAllocation,
        Self::UpdateNetworkAllocation,
        Self::UpdateRouting,
        Self::SetExitNodeRole,
        Self::SetLocalExitCapability,
        Self::SetRelayRole,
        Self::SetNodeDisplayName,
        Self::RotateNodeIdentity,
        Self::StartNode,
        Self::StopNode,
        Self::RestartNode,
        Self::CleanupResiduals,
        Self::GetConfiguration,
        Self::PreviewConfiguration,
        Self::ApplyConfiguration,
        Self::RollbackConfiguration,
        Self::GetConfigurationHistory,
        Self::GetPolicy,
        Self::PreviewPolicy,
        Self::PreviewPolicyRollback,
        Self::ApplyPolicy,
        Self::RollbackPolicy,
        Self::GetPolicyHistory,
        Self::TestPolicy,
        Self::GetTrafficPolicy,
        Self::PreviewTrafficPolicy,
        Self::PreviewTrafficPolicyRollback,
        Self::ApplyTrafficPolicy,
        Self::RollbackTrafficPolicy,
        Self::GetTrafficPolicyHistory,
        Self::TestTrafficPolicy,
        Self::ShutdownNode,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::RefreshSnapshot => "refresh_snapshot",
            Self::GetDiagnostics => "get_node_diagnostics",
            Self::CreateNetwork => "create_network",
            Self::PreviewNetworkProfile => "preview_network_profile",
            Self::UpdateNetworkProfile => "update_network_profile",
            Self::PublishNetworkDiscovery => "publish_network_discovery",
            Self::UpdateNetworkDiscovery => "update_network_discovery",
            Self::UnpublishNetworkDiscovery => "unpublish_network_discovery",
            Self::RefreshDiscoveryIndex => "refresh_discovery_index",
            Self::RetryDiscoveryIndex => "retry_discovery_index",
            Self::PreviewJoinNetwork => "preview_join_network",
            Self::JoinNetwork => "join_network",
            Self::SubmitJoinRequest => "submit_join_request",
            Self::ListJoinRequests => "list_join_requests",
            Self::ListNetworkMembers => "list_network_members",
            Self::PreviewMemberRemoval => "preview_member_removal",
            Self::ChangeMemberLifecycle => "change_member_lifecycle",
            Self::ReviewJoinRequest => "review_join_request",
            Self::BatchReviewJoinRequests => "batch_review_join_requests",
            Self::WithdrawJoinRequest => "withdraw_join_request",
            Self::ResubmitJoinRequest => "resubmit_join_request",
            Self::CreateInvitation => "create_invitation",
            Self::ListInvitations => "list_invitations",
            Self::GetInvitationStatus => "get_invitation_status",
            Self::RevokeInvitation => "revoke_invitation",
            Self::PreviewInvitation => "preview_invitation",
            Self::RedeemInvitation => "redeem_invitation",
            Self::LeaveNetwork => "leave_network",
            Self::AcknowledgeMembershipPolicy => "acknowledge_membership_policy",
            Self::StartNetworks => "start_networks",
            Self::StopNetworks => "stop_networks",
            Self::RestartNetworks => "restart_networks",
            Self::RebuildTun => "rebuild_tun",
            Self::ProbePeer => "probe_peer",
            Self::GetBlacklist => "get_blacklist",
            Self::BanClient => "ban_client",
            Self::UnbanClient => "unban_client",
            Self::GetNetworkAllocation => "get_network_allocation",
            Self::UpdateNetworkAllocation => "update_network_allocation",
            Self::UpdateRouting => "update_routing",
            Self::SetExitNodeRole => "set_exit_node_role",
            Self::SetLocalExitCapability => "set_local_exit_capability",
            Self::SetRelayRole => "set_relay_role",
            Self::SetNodeDisplayName => "set_node_display_name",
            Self::RotateNodeIdentity => "rotate_node_identity",
            Self::StartNode => "start_node",
            Self::StopNode => "stop_node",
            Self::RestartNode => "restart_node",
            Self::CleanupResiduals => "cleanup_node_residuals",
            Self::GetConfiguration => "get_configuration",
            Self::PreviewConfiguration => "preview_configuration",
            Self::ApplyConfiguration => "apply_configuration",
            Self::RollbackConfiguration => "rollback_configuration",
            Self::GetConfigurationHistory => "get_configuration_history",
            Self::GetPolicy => "get_policy",
            Self::PreviewPolicy => "preview_policy",
            Self::PreviewPolicyRollback => "preview_policy_rollback",
            Self::ApplyPolicy => "apply_policy",
            Self::RollbackPolicy => "rollback_policy",
            Self::GetPolicyHistory => "get_policy_history",
            Self::TestPolicy => "test_policy",
            Self::GetTrafficPolicy => "get_traffic_policy",
            Self::PreviewTrafficPolicy => "preview_traffic_policy",
            Self::PreviewTrafficPolicyRollback => "preview_traffic_policy_rollback",
            Self::ApplyTrafficPolicy => "apply_traffic_policy",
            Self::RollbackTrafficPolicy => "rollback_traffic_policy",
            Self::GetTrafficPolicyHistory => "get_traffic_policy_history",
            Self::TestTrafficPolicy => "test_traffic_policy",
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
    TunUnavailable(TunCapabilityReason),
}

impl ActionBlockReason {
    /// Project the adapter-local failure detail onto the canonical manifest
    /// vocabulary before looking up presentation copy.
    pub const fn contract_reason(self) -> NodeUiActionBlockReason {
        match self {
            Self::NodeNotRunning => NodeUiActionBlockReason::NodeNotRunning,
            Self::NoOnlineNetwork => NodeUiActionBlockReason::NoOnlineNetwork,
            Self::NoNetworkSelected => NodeUiActionBlockReason::NoNetworkSelected,
            Self::PermissionDenied => NodeUiActionBlockReason::PermissionDenied,
            Self::ExitNodeUnsupported => NodeUiActionBlockReason::ExitNodeUnsupported,
            Self::TunUnavailable(reason) => match reason {
                TunCapabilityReason::PlatformUnsupported => {
                    NodeUiActionBlockReason::TunPlatformUnsupported
                }
                TunCapabilityReason::PermissionDenied => {
                    NodeUiActionBlockReason::TunPermissionDenied
                }
                TunCapabilityReason::DeviceUnavailable => {
                    NodeUiActionBlockReason::TunDeviceUnavailable
                }
                TunCapabilityReason::ToolUnavailable => NodeUiActionBlockReason::TunToolUnavailable,
                TunCapabilityReason::Unknown => NodeUiActionBlockReason::TunUnavailable,
            },
        }
    }

    /// Stable catalog key. Adapters must translate this at the presentation
    /// boundary instead of treating UI copy as a business identifier.
    pub const fn message_key(self) -> &'static str {
        self.contract_reason().message_key()
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TopologyPresentationCondition {
    Error,
    PermissionDenied,
    InitialLoading,
    Offline,
    Stale,
    Refreshing,
    Partial,
    Unavailable,
    Empty,
    Ready,
}

impl TopologyPresentationCondition {
    pub const fn key(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::PermissionDenied => "permission_denied",
            Self::InitialLoading => "initial_loading",
            Self::Offline => "offline",
            Self::Stale => "stale",
            Self::Refreshing => "refreshing",
            Self::Partial => "partial",
            Self::Unavailable => "unavailable",
            Self::Empty => "empty",
            Self::Ready => "ready",
        }
    }

    pub const fn title_key(self) -> &'static str {
        match self {
            Self::Error => "state.error.title",
            Self::PermissionDenied => "state.permission_denied.title",
            Self::InitialLoading => "state.initial_loading.title",
            Self::Offline => "state.offline.title",
            Self::Stale => "state.stale.title",
            Self::Refreshing => "state.refreshing.title",
            Self::Partial => "topology.state.partial.title",
            Self::Unavailable => "topology.state.unavailable.title",
            Self::Empty => "topology.state.empty.title",
            Self::Ready => "state.ready.title",
        }
    }

    pub const fn detail_key(self) -> &'static str {
        match self {
            Self::Error => "state.error.detail",
            Self::PermissionDenied => "state.permission_denied.detail",
            Self::InitialLoading => "state.initial_loading.detail",
            Self::Offline => "state.offline.detail",
            Self::Stale => "state.stale.detail",
            Self::Refreshing => "state.refreshing.detail",
            Self::Partial => "topology.state.partial.detail",
            Self::Unavailable => "topology.empty.unavailable",
            Self::Empty => "topology.empty.no_networks",
            Self::Ready => "state.ready.detail",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TopologyPresentationSignals {
    pub initial_loading: bool,
    pub refreshing: bool,
    pub offline: bool,
    pub stale: bool,
    pub error: bool,
    pub permission_denied: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyPresentation {
    pub kind: TopologyPresentationCondition,
    pub conditions: Vec<TopologyPresentationCondition>,
    pub has_content: bool,
    pub can_render: bool,
}

fn topology_presentation_condition(
    topology: Option<&TopologySnapshot>,
    signals: TopologyPresentationSignals,
) -> Vec<TopologyPresentationCondition> {
    let mut conditions = Vec::new();
    if signals.error {
        conditions.push(TopologyPresentationCondition::Error);
    }
    if signals.permission_denied {
        conditions.push(TopologyPresentationCondition::PermissionDenied);
    }
    if signals.initial_loading {
        conditions.push(TopologyPresentationCondition::InitialLoading);
    }
    if signals.offline {
        conditions.push(TopologyPresentationCondition::Offline);
    }
    if signals.stale {
        conditions.push(TopologyPresentationCondition::Stale);
    }
    if signals.refreshing {
        conditions.push(TopologyPresentationCondition::Refreshing);
    }
    if topology.is_some_and(|topology| topology.truncated) {
        conditions.push(TopologyPresentationCondition::Partial);
    }
    match topology {
        None => conditions.push(TopologyPresentationCondition::Unavailable),
        Some(topology) if topology.boundaries.is_empty() => {
            conditions.push(TopologyPresentationCondition::Empty);
        }
        Some(_) => {}
    }
    if conditions.is_empty() {
        conditions.push(TopologyPresentationCondition::Ready);
    }
    conditions.sort();
    conditions
}

pub fn topology_presentation(
    topology: Option<&TopologySnapshot>,
    signals: TopologyPresentationSignals,
) -> TopologyPresentation {
    let conditions = topology_presentation_condition(topology, signals);
    let kind = conditions[0];
    let has_content = topology.is_some_and(|topology| !topology.boundaries.is_empty());
    TopologyPresentation {
        kind,
        conditions,
        has_content,
        can_render: has_content
            && !matches!(
                kind,
                TopologyPresentationCondition::PermissionDenied
                    | TopologyPresentationCondition::InitialLoading
            ),
    }
}

pub const MEMBER_EXIT_AVAILABILITY_MAX_AGE_MS: u64 = 90_000;

const EXIT_AVAILABILITY_REASON_CODES: [&str; 11] = [
    "exit.availability.available_but_not_declared",
    "exit.availability.cleaning",
    "exit.availability.declared_but_unavailable",
    "exit.availability.draining",
    "exit.availability.forwarder_unhealthy",
    "exit.availability.health_checking",
    "exit.availability.network_not_allowed",
    "exit.availability.not_observed",
    "exit.availability.owner_disabled",
    "exit.availability.stale",
    "exit.availability.waiting_for_network_authorization",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberExitFreshness {
    Fresh,
    Stale,
    NotObserved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedMemberExitAvailability {
    pub state: ExitAvailabilityState,
    pub source: ExitAvailabilitySource,
    pub declared_available: Option<bool>,
    pub observed_at_unix_ms: Option<u64>,
    pub freshness: MemberExitFreshness,
    pub reason_code: Option<&'static str>,
}

fn allowed_exit_availability_reason(code: Option<&str>) -> Option<&'static str> {
    let code = code?;
    EXIT_AVAILABILITY_REASON_CODES
        .iter()
        .copied()
        .find(|allowed| *allowed == code)
}

/// Fail closed: missing, stale, or contradictory evidence never becomes healthy.
pub fn normalize_member_exit_availability(
    availability: Option<&ExitContributionAvailability>,
    now_unix_ms: u64,
) -> NormalizedMemberExitAvailability {
    let Some(availability) = availability else {
        return NormalizedMemberExitAvailability {
            state: ExitAvailabilityState::Unknown,
            source: ExitAvailabilitySource::NotObserved,
            declared_available: None,
            observed_at_unix_ms: None,
            freshness: MemberExitFreshness::NotObserved,
            reason_code: Some("exit.availability.not_observed"),
        };
    };

    let supplied_reason = allowed_exit_availability_reason(availability.reason_code.as_deref());
    let Some(observed_at_unix_ms) = availability.observed_at_unix_ms else {
        let not_observed = availability.source == ExitAvailabilitySource::NotObserved
            && availability.state == ExitAvailabilityState::Unknown;
        return NormalizedMemberExitAvailability {
            state: ExitAvailabilityState::Unknown,
            source: availability.source,
            declared_available: availability.declared_available,
            observed_at_unix_ms: None,
            freshness: if not_observed {
                MemberExitFreshness::NotObserved
            } else {
                MemberExitFreshness::Stale
            },
            reason_code: supplied_reason.or(Some(if not_observed {
                "exit.availability.not_observed"
            } else {
                "exit.availability.stale"
            })),
        };
    };

    if now_unix_ms > observed_at_unix_ms.saturating_add(MEMBER_EXIT_AVAILABILITY_MAX_AGE_MS) {
        return NormalizedMemberExitAvailability {
            state: ExitAvailabilityState::Unknown,
            source: availability.source,
            declared_available: availability.declared_available,
            observed_at_unix_ms: Some(observed_at_unix_ms),
            freshness: MemberExitFreshness::Stale,
            reason_code: Some("exit.availability.stale"),
        };
    }

    let contradictory_availability = availability.state == ExitAvailabilityState::Available
        && availability.declared_available == Some(false);
    NormalizedMemberExitAvailability {
        state: if contradictory_availability {
            ExitAvailabilityState::Degraded
        } else {
            availability.state
        },
        source: availability.source,
        declared_available: availability.declared_available,
        observed_at_unix_ms: Some(observed_at_unix_ms),
        freshness: MemberExitFreshness::Fresh,
        reason_code: if contradictory_availability {
            Some("exit.availability.available_but_not_declared")
        } else {
            supplied_reason
        },
    }
}

fn member_exit_state_key(state: ExitAvailabilityState) -> &'static str {
    match state {
        ExitAvailabilityState::Unknown => "member_directory.exit.state.unknown",
        ExitAvailabilityState::Available => "member_directory.exit.state.available",
        ExitAvailabilityState::Degraded => "member_directory.exit.state.degraded",
        ExitAvailabilityState::Unavailable => "member_directory.exit.state.unavailable",
    }
}

fn member_exit_source_key(source: ExitAvailabilitySource) -> &'static str {
    match source {
        ExitAvailabilitySource::NotObserved => "member_directory.exit.source.not_observed",
        ExitAvailabilitySource::LocalRuntime => "member_directory.exit.source.local_runtime",
        ExitAvailabilitySource::CoordinatorHealth => {
            "member_directory.exit.source.coordinator_health"
        }
        ExitAvailabilitySource::RemotePeerReport => {
            "member_directory.exit.source.remote_peer_report"
        }
    }
}

pub fn member_exit_state_text(
    locale: Locale,
    availability: Option<&ExitContributionAvailability>,
    now_unix_ms: u64,
) -> String {
    let normalized = normalize_member_exit_availability(availability, now_unix_ms);
    t(locale, member_exit_state_key(normalized.state)).to_string()
}

pub fn member_exit_source_text(
    locale: Locale,
    availability: Option<&ExitContributionAvailability>,
    now_unix_ms: u64,
) -> String {
    let normalized = normalize_member_exit_availability(availability, now_unix_ms);
    t(locale, member_exit_source_key(normalized.source)).to_string()
}

pub fn member_exit_declared_text(
    locale: Locale,
    availability: Option<&ExitContributionAvailability>,
    now_unix_ms: u64,
) -> String {
    let normalized = normalize_member_exit_availability(availability, now_unix_ms);
    let key = match normalized.declared_available {
        Some(true) => "member_directory.exit.declared.available",
        Some(false) => "member_directory.exit.declared.unavailable",
        None => "member_directory.exit.declared.unknown",
    };
    t(locale, key).to_string()
}

pub fn member_exit_freshness_text(
    locale: Locale,
    availability: Option<&ExitContributionAvailability>,
    now_unix_ms: u64,
) -> String {
    let normalized = normalize_member_exit_availability(availability, now_unix_ms);
    match normalized.freshness {
        MemberExitFreshness::NotObserved => {
            t(locale, "member_directory.exit.freshness.not_observed").to_string()
        }
        MemberExitFreshness::Stale => {
            t(locale, "member_directory.exit.freshness.stale").to_string()
        }
        MemberExitFreshness::Fresh => {
            let Some(observed_at_unix_ms) = normalized.observed_at_unix_ms else {
                return t(locale, "common.unknown").to_string();
            };
            let elapsed_seconds = now_unix_ms.saturating_sub(observed_at_unix_ms) / 1_000;
            if elapsed_seconds < 60 {
                return t(locale, "member_directory.freshness.just_now").to_string();
            }
            let minutes = elapsed_seconds / 60;
            if minutes < 60 {
                return tr(
                    locale,
                    "member_directory.freshness.minutes",
                    &[("count", &format_integer(locale, minutes as i64))],
                );
            }
            let hours = minutes / 60;
            if hours < 24 {
                return tr(
                    locale,
                    "member_directory.freshness.hours",
                    &[("count", &format_integer(locale, hours as i64))],
                );
            }
            tr(
                locale,
                "member_directory.freshness.days",
                &[("count", &format_integer(locale, (hours / 24) as i64))],
            )
        }
    }
}

pub fn member_exit_reason_text(
    locale: Locale,
    availability: Option<&ExitContributionAvailability>,
    now_unix_ms: u64,
) -> String {
    let normalized = normalize_member_exit_availability(availability, now_unix_ms);
    let Some(reason_code) = normalized.reason_code else {
        return t(locale, "common.unknown").to_string();
    };
    let key = reason_code
        .strip_prefix("exit.availability.")
        .map(|reason| format!("member_directory.exit.reason.{reason}"));
    key.map(|key| t(locale, &key).to_string())
        .unwrap_or_else(|| t(locale, "common.unknown").to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyNodeDetailModel {
    pub node_id: String,
    pub display_name: String,
    pub health: String,
    pub role: String,
    pub capabilities: String,
    pub status: String,
    pub presence: String,
    pub networks: String,
    pub exit_state: String,
    pub exit_source: String,
    pub exit_declared: String,
    pub exit_observed_at_unix_ms: Option<u64>,
    pub exit_freshness: String,
    pub exit_reason: String,
}

pub fn topology_member_actions(
    directory: Option<&NetworkMemberDirectory>,
    network_id: Option<&str>,
    node_id: &str,
) -> Vec<MemberLifecycleAction> {
    const BATCH_MEMBER_ACTIONS: [MemberLifecycleAction; 6] = [
        MemberLifecycleAction::Approve,
        MemberLifecycleAction::Reject,
        MemberLifecycleAction::Suspend,
        MemberLifecycleAction::Resume,
        MemberLifecycleAction::Remove,
        MemberLifecycleAction::UndoRemove,
    ];
    let (Some(directory), Some(network_id)) = (directory, network_id) else {
        return Vec::new();
    };
    if directory.network_id.as_str() != network_id || !directory.can_manage_members {
        return Vec::new();
    }
    let Some(member) = directory
        .members
        .iter()
        .find(|member| member.node_id.as_str() == node_id)
    else {
        return Vec::new();
    };
    member
        .available_actions
        .iter()
        .copied()
        .filter(|action| BATCH_MEMBER_ACTIONS.contains(action))
        .collect()
}

pub fn topology_node_detail(
    node: &TopologyNode,
    network_id: Option<&str>,
    directory_member: Option<&NetworkMember>,
    locale: Locale,
    generated_at_unix_ms: u64,
) -> TopologyNodeDetailModel {
    let membership = network_id.and_then(|network_id| {
        node.networks
            .iter()
            .find(|network| network.network_id.as_str() == network_id)
    });
    let role = membership
        .map(|membership| t(locale, topology_role_key(membership.role)))
        .unwrap_or_else(|| t(locale, "common.not_available"));
    let capabilities = if node.capabilities.is_empty() {
        t(locale, "status.capability.unknown").to_string()
    } else {
        node.capabilities
            .iter()
            .map(|capability| t(locale, capability_key(*capability)))
            .collect::<Vec<_>>()
            .join(t(locale, "common.list_separator"))
    };
    let status = membership
        .map(|membership| t(locale, topology_member_status_key(membership.status)))
        .or_else(|| {
            directory_member.map(|member| t(locale, topology_member_status_key(member.status)))
        })
        .unwrap_or_else(|| t(locale, "common.unknown"));
    let presence = membership
        .map(|membership| t(locale, topology_presence_key(membership.presence.state)))
        .or_else(|| {
            directory_member.map(|member| t(locale, topology_presence_key(member.presence.state)))
        })
        .unwrap_or_else(|| t(locale, "common.unknown"));
    let networks = node
        .networks
        .iter()
        .map(|network| {
            format!(
                "{} · {}",
                network.network_id,
                t(locale, topology_role_key(network.role))
            )
        })
        .collect::<Vec<_>>()
        .join(t(locale, "common.list_separator"));
    let exit_availability = directory_member.map(|member| &member.exit_availability);
    let normalized_exit =
        normalize_member_exit_availability(exit_availability, generated_at_unix_ms);
    TopologyNodeDetailModel {
        node_id: node.node_id.to_string(),
        display_name: node
            .display_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(node.node_id.as_str())
            .to_string(),
        health: t(locale, topology_health_key(node.health)).to_string(),
        role: role.to_string(),
        capabilities,
        status: status.to_string(),
        presence: presence.to_string(),
        networks,
        exit_state: member_exit_state_text(locale, exit_availability, generated_at_unix_ms),
        exit_source: member_exit_source_text(locale, exit_availability, generated_at_unix_ms),
        exit_declared: member_exit_declared_text(locale, exit_availability, generated_at_unix_ms),
        exit_observed_at_unix_ms: normalized_exit.observed_at_unix_ms,
        exit_freshness: member_exit_freshness_text(locale, exit_availability, generated_at_unix_ms),
        exit_reason: member_exit_reason_text(locale, exit_availability, generated_at_unix_ms),
    }
}

fn topology_health_key(health: TopologyNodeHealth) -> &'static str {
    match health {
        TopologyNodeHealth::Healthy => "topology.health.healthy",
        TopologyNodeHealth::Unknown => "topology.health.unknown",
        TopologyNodeHealth::Degraded => "topology.health.degraded",
        TopologyNodeHealth::Unreachable => "topology.health.unreachable",
    }
}

fn topology_role_key(role: MembershipRole) -> &'static str {
    match role {
        MembershipRole::Owner => "join_request.role.owner",
        MembershipRole::Administrator => "join_request.role.administrator",
        MembershipRole::Member => "join_request.role.member",
    }
}

fn topology_member_status_key(status: NetworkMemberStatus) -> &'static str {
    match status {
        NetworkMemberStatus::Pending => "member_directory.status.pending",
        NetworkMemberStatus::Approved => "member_directory.status.approved",
        NetworkMemberStatus::Active => "member_directory.status.active",
        NetworkMemberStatus::Degraded => "member_directory.status.degraded",
        NetworkMemberStatus::Suspended => "member_directory.status.suspended",
        NetworkMemberStatus::Exited => "member_directory.status.exited",
        NetworkMemberStatus::Removed => "member_directory.status.removed",
        NetworkMemberStatus::Expired => "member_directory.status.expired",
    }
}

fn topology_presence_key(presence: NetworkMemberOnlineState) -> &'static str {
    match presence {
        NetworkMemberOnlineState::Online => "member_directory.presence.online",
        NetworkMemberOnlineState::Offline => "member_directory.presence.offline",
        NetworkMemberOnlineState::Unknown => "member_directory.presence.unknown",
    }
}

pub fn member_lifecycle_action_key(action: MemberLifecycleAction) -> &'static str {
    match action {
        MemberLifecycleAction::Invite => "choice.member_lifecycle.invite",
        MemberLifecycleAction::Approve => "choice.member_lifecycle.approve",
        MemberLifecycleAction::Reject => "choice.member_lifecycle.reject",
        MemberLifecycleAction::Suspend => "choice.member_lifecycle.suspend",
        MemberLifecycleAction::Resume => "choice.member_lifecycle.resume",
        MemberLifecycleAction::Exit => "choice.member_lifecycle.exit",
        MemberLifecycleAction::Remove => "choice.member_lifecycle.remove",
        MemberLifecycleAction::UndoRemove => "choice.member_lifecycle.undo_remove",
    }
}

/// Wire value accepted by `change_member_lifecycle`. This is deliberately
/// separate from the localized label key so presentation copy can never leak
/// into command serialization.
pub const fn member_lifecycle_action_value(action: MemberLifecycleAction) -> &'static str {
    match action {
        MemberLifecycleAction::Invite => "invite",
        MemberLifecycleAction::Approve => "approve",
        MemberLifecycleAction::Reject => "reject",
        MemberLifecycleAction::Suspend => "suspend",
        MemberLifecycleAction::Resume => "resume",
        MemberLifecycleAction::Exit => "exit",
        MemberLifecycleAction::Remove => "remove",
        MemberLifecycleAction::UndoRemove => "undo_remove",
    }
}

/// Wire value accepted by `change_member_lifecycle` for the target member's
/// authoritative role.
pub const fn membership_role_value(role: MembershipRole) -> &'static str {
    match role {
        MembershipRole::Owner => "owner",
        MembershipRole::Administrator => "administrator",
        MembershipRole::Member => "member",
    }
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
    pub navigation: Option<RowNavigation>,
}

/// A typed navigation destination attached to an authoritative summary row.
/// It intentionally carries the object identity so the destination opens the
/// matching object rather than a generic editor with stale context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowNavigation {
    pub page_id: String,
    pub network_name: Option<String>,
    /// Stable object identity used by destinations that can open a concrete
    /// node or network rather than only selecting a page.
    pub object_id: Option<String>,
    pub label: String,
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
    pub id: String,
    pub action: String,
    pub detail: String,
    pub command: Option<String>,
    pub occurred_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverviewShortcut {
    pub id: String,
    pub label: String,
    pub action_id: Option<String>,
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverviewTrafficValidation {
    pub label: String,
    pub passed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverviewTrafficSessionEvent {
    pub kind: TrafficSessionEventKind,
    pub kind_label: String,
    pub state: TrafficProtectionState,
    pub state_label: String,
    pub reason_code: Option<String>,
    pub occurred_at_unix_ms: u64,
}

/// Authoritative protection state rendered separately from node lifecycle.
/// The UI never treats a running node as a protected traffic session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverviewTrafficSession {
    pub state: TrafficProtectionState,
    pub state_label: String,
    pub reason_code: Option<String>,
    pub changed_at_unix_ms: u64,
    pub validation: Vec<OverviewTrafficValidation>,
    pub events: Vec<OverviewTrafficSessionEvent>,
}

/// Dedicated product projection for the overview. It deliberately does not
/// reuse generic metric cards: the first screen is organized around the one
/// primary status, actionable issues, compact network context, and recent
/// sanitized activity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverviewModel {
    pub node_name: String,
    pub primary_status: NodePrimaryStatus,
    pub primary_page_id: Option<String>,
    pub issues: Vec<String>,
    pub traffic_session: OverviewTrafficSession,
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
    build_page_model_with_activity_filters(
        manifest,
        snapshot,
        page_id,
        locale,
        &ActivityFilters::default(),
    )
}

pub fn build_page_model_with_activity_filters(
    manifest: &NodeUiManifest,
    snapshot: &NodeSnapshot,
    page_id: &str,
    locale: Locale,
    activity_filters: &ActivityFilters,
) -> Result<PageModel, ModelError> {
    let page = manifest
        .pages
        .iter()
        .find(|page| page.id == page_id)
        .ok_or_else(|| ModelError::UnknownPage {
            page_id: page_id.into(),
        })?;
    build_page_model_for_page_with_activity_filters(page, snapshot, locale, activity_filters)
}

pub fn build_page_model_for_page(
    page: &NodeUiPage,
    snapshot: &NodeSnapshot,
    locale: Locale,
) -> Result<PageModel, ModelError> {
    build_page_model_for_page_with_activity_filters(
        page,
        snapshot,
        locale,
        &ActivityFilters::default(),
    )
}

pub fn build_page_model_for_page_with_activity_filters(
    page: &NodeUiPage,
    snapshot: &NodeSnapshot,
    locale: Locale,
    activity_filters: &ActivityFilters,
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

    let (metrics, sections, overview) = project_page(kind, snapshot, locale, activity_filters);
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
        "preview_network_profile" => (
            ActionBinding::PreviewNetworkProfile,
            NodeCommandKind::PreviewNetworkProfile,
        ),
        "update_network_profile" => (
            ActionBinding::UpdateNetworkProfile,
            NodeCommandKind::UpdateNetworkProfile,
        ),
        "publish_network_discovery" => (
            ActionBinding::PublishNetworkDiscovery,
            NodeCommandKind::PublishNetworkDiscovery,
        ),
        "update_network_discovery" => (
            ActionBinding::UpdateNetworkDiscovery,
            NodeCommandKind::UpdateNetworkDiscovery,
        ),
        "unpublish_network_discovery" => (
            ActionBinding::UnpublishNetworkDiscovery,
            NodeCommandKind::UnpublishNetworkDiscovery,
        ),
        "refresh_discovery_index" => (
            ActionBinding::RefreshDiscoveryIndex,
            NodeCommandKind::RefreshDiscoveryIndex,
        ),
        "retry_discovery_index" => (
            ActionBinding::RetryDiscoveryIndex,
            NodeCommandKind::RetryDiscoveryIndex,
        ),
        "preview_join_network" => (
            ActionBinding::PreviewJoinNetwork,
            NodeCommandKind::PreviewJoinNetwork,
        ),
        "join_network" => (ActionBinding::JoinNetwork, NodeCommandKind::JoinNetwork),
        "submit_join_request" => (
            ActionBinding::SubmitJoinRequest,
            NodeCommandKind::SubmitJoinRequest,
        ),
        "list_join_requests" => (
            ActionBinding::ListJoinRequests,
            NodeCommandKind::ListJoinRequests,
        ),
        "list_network_members" => (
            ActionBinding::ListNetworkMembers,
            NodeCommandKind::ListNetworkMembers,
        ),
        "preview_member_removal" => (
            ActionBinding::PreviewMemberRemoval,
            NodeCommandKind::PreviewMemberRemoval,
        ),
        "change_member_lifecycle" => (
            ActionBinding::ChangeMemberLifecycle,
            NodeCommandKind::ChangeMemberLifecycle,
        ),
        "review_join_request" => (
            ActionBinding::ReviewJoinRequest,
            NodeCommandKind::ReviewJoinRequest,
        ),
        "batch_review_join_requests" => (
            ActionBinding::BatchReviewJoinRequests,
            NodeCommandKind::BatchReviewJoinRequests,
        ),
        "withdraw_join_request" => (
            ActionBinding::WithdrawJoinRequest,
            NodeCommandKind::WithdrawJoinRequest,
        ),
        "resubmit_join_request" => (
            ActionBinding::ResubmitJoinRequest,
            NodeCommandKind::ResubmitJoinRequest,
        ),
        "create_invitation" => (
            ActionBinding::CreateInvitation,
            NodeCommandKind::CreateInvitation,
        ),
        "list_invitations" => (
            ActionBinding::ListInvitations,
            NodeCommandKind::ListInvitations,
        ),
        "get_invitation_status" => (
            ActionBinding::GetInvitationStatus,
            NodeCommandKind::GetInvitationStatus,
        ),
        "revoke_invitation" => (
            ActionBinding::RevokeInvitation,
            NodeCommandKind::RevokeInvitation,
        ),
        "preview_invitation" => (
            ActionBinding::PreviewInvitation,
            NodeCommandKind::PreviewInvitation,
        ),
        "redeem_invitation" => (
            ActionBinding::RedeemInvitation,
            NodeCommandKind::RedeemInvitation,
        ),
        "leave_network" => (ActionBinding::LeaveNetwork, NodeCommandKind::LeaveNetwork),
        "acknowledge_membership_policy" => (
            ActionBinding::AcknowledgeMembershipPolicy,
            NodeCommandKind::AcknowledgeMembershipPolicy,
        ),
        "start_networks" => (ActionBinding::StartNetworks, NodeCommandKind::StartNetworks),
        "stop_networks" => (ActionBinding::StopNetworks, NodeCommandKind::StopNetworks),
        "restart_networks" => (
            ActionBinding::RestartNetworks,
            NodeCommandKind::RestartNetworks,
        ),
        "rebuild_tun" => (ActionBinding::RebuildTun, NodeCommandKind::RebuildTun),
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
        "set_local_exit_capability" => (
            ActionBinding::SetLocalExitCapability,
            NodeCommandKind::SetLocalExitCapability,
        ),
        "set_relay_role" => (ActionBinding::SetRelayRole, NodeCommandKind::SetNetworkRole),
        "set_node_display_name" => (
            ActionBinding::SetNodeDisplayName,
            NodeCommandKind::SetNodeDisplayName,
        ),
        "rotate_node_identity" => (
            ActionBinding::RotateNodeIdentity,
            NodeCommandKind::RotateNodeIdentity,
        ),
        "start_node" => (ActionBinding::StartNode, NodeCommandKind::StartNode),
        "stop_node" => (ActionBinding::StopNode, NodeCommandKind::StopNode),
        "restart_node" => (ActionBinding::RestartNode, NodeCommandKind::RestartNode),
        "cleanup_node_residuals" => (
            ActionBinding::CleanupResiduals,
            NodeCommandKind::CleanupResiduals,
        ),
        "get_configuration" => (
            ActionBinding::GetConfiguration,
            NodeCommandKind::GetConfiguration,
        ),
        "preview_configuration" => (
            ActionBinding::PreviewConfiguration,
            NodeCommandKind::PreviewConfiguration,
        ),
        "apply_configuration" => (
            ActionBinding::ApplyConfiguration,
            NodeCommandKind::ApplyConfiguration,
        ),
        "rollback_configuration" => (
            ActionBinding::RollbackConfiguration,
            NodeCommandKind::RollbackConfiguration,
        ),
        "get_configuration_history" => (
            ActionBinding::GetConfigurationHistory,
            NodeCommandKind::GetConfigurationHistory,
        ),
        "get_policy" => (ActionBinding::GetPolicy, NodeCommandKind::GetPolicy),
        "preview_policy" => (ActionBinding::PreviewPolicy, NodeCommandKind::PreviewPolicy),
        "preview_policy_rollback" => (
            ActionBinding::PreviewPolicyRollback,
            NodeCommandKind::PreviewPolicyRollback,
        ),
        "apply_policy" => (ActionBinding::ApplyPolicy, NodeCommandKind::ApplyPolicy),
        "rollback_policy" => (
            ActionBinding::RollbackPolicy,
            NodeCommandKind::RollbackPolicy,
        ),
        "get_policy_history" => (
            ActionBinding::GetPolicyHistory,
            NodeCommandKind::GetPolicyHistory,
        ),
        "test_policy" => (ActionBinding::TestPolicy, NodeCommandKind::TestPolicy),
        "get_traffic_policy" => (
            ActionBinding::GetTrafficPolicy,
            NodeCommandKind::GetTrafficPolicy,
        ),
        "preview_traffic_policy" => (
            ActionBinding::PreviewTrafficPolicy,
            NodeCommandKind::PreviewTrafficPolicy,
        ),
        "preview_traffic_policy_rollback" => (
            ActionBinding::PreviewTrafficPolicyRollback,
            NodeCommandKind::PreviewTrafficPolicyRollback,
        ),
        "apply_traffic_policy" => (
            ActionBinding::ApplyTrafficPolicy,
            NodeCommandKind::ApplyTrafficPolicy,
        ),
        "rollback_traffic_policy" => (
            ActionBinding::RollbackTrafficPolicy,
            NodeCommandKind::RollbackTrafficPolicy,
        ),
        "get_traffic_policy_history" => (
            ActionBinding::GetTrafficPolicyHistory,
            NodeCommandKind::GetTrafficPolicyHistory,
        ),
        "test_traffic_policy" => (
            ActionBinding::TestTrafficPolicy,
            NodeCommandKind::TestTrafficPolicy,
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

    if matches!(
        action.id.as_str(),
        "set_exit_node_role" | "set_local_exit_capability"
    ) {
        let state = snapshot.capabilities.state(CapabilityKind::Exit);
        if state == CapabilityState::Unsupported
            || (state == CapabilityState::Unknown && !snapshot.exit_node_capabilities.supported)
        {
            return Some(ActionBlockReason::ExitNodeUnsupported);
        }
    }

    if action.id == "rebuild_tun" && !snapshot.tun_capabilities.can_rebuild {
        return Some(ActionBlockReason::TunUnavailable(
            snapshot
                .tun_capabilities
                .blocking_reason()
                .unwrap_or(TunCapabilityReason::Unknown),
        ));
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
    snapshot: &NodeSnapshot,
    selected_network: Option<&str>,
) -> HashMap<String, String> {
    let profile = selected_network.and_then(|selected| {
        snapshot
            .network_profiles
            .iter()
            .find(|profile| profile.name == selected)
    });
    let exit_selection_policy = profile
        .and_then(|profile| {
            snapshot
                .exit_selections
                .iter()
                .find(|selection| selection.network_id.as_str() == profile.network_id.as_str())
        })
        .and_then(|selection| selection.policy.as_ref())
        .and_then(|policy| serde_json::to_string(policy).ok());
    action
        .fields
        .iter()
        .map(|field| {
            let value = match (action.id.as_str(), field.id.as_str()) {
                (_, "signaling_port") => "6666".into(),
                (_, "relay_port") => "6667".into(),
                (_, "timeout_ms") => "1500".into(),
                (_, "acknowledged_at_unix_ms") => "1700000000000".into(),
                ("join_network", "start") => "true".into(),
                (
                    "list_network_members" | "preview_member_removal" | "change_member_lifecycle",
                    "network_id",
                ) => selected_network
                    .and_then(|network| network_membership_id(snapshot, network))
                    .unwrap_or_default()
                    .into(),
                ("create_network", "purpose") => "Owner-created network".into(),
                ("create_network", "icon") => "network".into(),
                ("create_network", "language") => "und".into(),
                ("create_network", "region") => "ZZ".into(),
                ("create_network", "visibility") => "unlisted".into(),
                ("create_network", "join_mode") => "invite_only".into(),
                ("create_network", "exit_contribution_mode") => "disabled".into(),
                ("create_network", "exit_contribution") => "not_required".into(),
                ("create_network", "exit_contribution_scope") => "not_applicable".into(),
                ("create_network", "exit_selection_policy_json") => "{}".into(),
                ("create_network", "auto_connect") => "true".into(),
                ("preview_network_profile" | "update_network_profile", "purpose") => profile
                    .map(|profile| profile.purpose.clone())
                    .unwrap_or_else(|| "Owner-created network".into()),
                ("preview_network_profile" | "update_network_profile", "icon") => profile
                    .map(|profile| profile.icon.clone())
                    .unwrap_or_else(|| "network".into()),
                ("preview_network_profile" | "update_network_profile", "language") => profile
                    .map(|profile| profile.language.clone())
                    .unwrap_or_else(|| "und".into()),
                ("preview_network_profile" | "update_network_profile", "region") => profile
                    .map(|profile| profile.region.clone())
                    .unwrap_or_else(|| "ZZ".into()),
                ("preview_network_profile" | "update_network_profile", "tags") => profile
                    .map(|profile| profile.tags.join(","))
                    .unwrap_or_default(),
                (
                    "preview_network_profile" | "update_network_profile",
                    "capability_declarations",
                ) => profile
                    .map(|profile| {
                        profile
                            .capability_declarations
                            .iter()
                            .map(|capability| capability.wire_name())
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .unwrap_or_default(),
                ("preview_network_profile" | "update_network_profile", "visibility") => profile
                    .map(|profile| format!("{:?}", profile.visibility).to_ascii_lowercase())
                    .unwrap_or_else(|| "unlisted".into()),
                ("preview_network_profile" | "update_network_profile", "join_mode") => profile
                    .map(|profile| format!("{:?}", profile.join_mode).to_ascii_lowercase())
                    .unwrap_or_else(|| "invite_only".into()),
                (
                    "preview_network_profile" | "update_network_profile",
                    "exit_contribution_mode",
                ) => profile
                    .map(|profile| {
                        exit_contribution_mode_wire_value(profile.exit_contribution_mode).into()
                    })
                    .unwrap_or_else(|| "disabled".into()),
                ("preview_network_profile" | "update_network_profile", "exit_contribution") => {
                    profile
                        .map(|profile| {
                            format!("{:?}", profile.exit_contribution).to_ascii_lowercase()
                        })
                        .unwrap_or_else(|| "not_required".into())
                }
                (
                    "preview_network_profile" | "update_network_profile",
                    "exit_contribution_scope",
                ) => profile
                    .map(|profile| {
                        format!("{:?}", profile.exit_contribution_scope).to_ascii_lowercase()
                    })
                    .unwrap_or_else(|| "not_applicable".into()),
                (
                    "preview_network_profile" | "update_network_profile",
                    "exit_selection_policy_json",
                ) => exit_selection_policy.clone().unwrap_or_default(),
                ("preview_network_profile" | "update_network_profile", "expected_revision") => {
                    profile
                        .map(|profile| profile.revision.to_string())
                        .unwrap_or_else(|| "0".into())
                }
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
                ("preview_configuration" | "apply_configuration", "expected_revision") => {
                    "0".into()
                }
                ("preview_configuration" | "apply_configuration", "networks") => "{}".into(),
                ("rollback_configuration", "expected_revision" | "target_revision") => "0".into(),
                ("set_exit_node_role", "role") => "exit_node".into(),
                ("set_relay_role", "role") => "relay".into(),
                (
                    "review_join_request"
                    | "batch_review_join_requests"
                    | "resubmit_join_request"
                    | "change_member_lifecycle",
                    "expected_revision",
                ) => "1".into(),
                ("change_member_lifecycle", "action") => "approve".into(),
                ("change_member_lifecycle", "role") => "member".into(),
                (_, "network_id") => snapshot
                    .network_profiles
                    .iter()
                    .find(|profile| profile.name == selected_network.unwrap_or_default())
                    .map(|profile| profile.network_id.clone())
                    .unwrap_or_default(),
                (_, "request_id") => {
                    fuckipv6_common::domain::JoinRequestId::from_public_key(
                        b"default-join-request",
                    )
                    .to_string()
                }
                (_, "reason") => "Join request".into(),
                (_, "reviews") => {
                    let request_id = fuckipv6_common::domain::JoinRequestId::from_public_key(
                        b"default-join-request",
                    )
                    .to_string();
                    format!(r#"[{{"request_id":"{request_id}","expected_revision":1,"action":"approve"}}]"#)
                }
                (_, "action") => "approve".into(),
                (_, "enabled") => "true".into(),
                (_, "index_url") => snapshot
                    .discovery_index
                    .index_url
                    .clone()
                    .unwrap_or_default(),
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
            let exit_contribution_mode =
                parse_exit_contribution_mode(action, &required("exit_contribution_mode")?)?;
            let (exit_contribution, exit_contribution_scope) = resolve_exit_contribution_fields(
                action,
                exit_contribution_mode,
                &required("exit_contribution_scope")?,
            )?;
            Ok(NodeCommand::CreateNetwork(NodeCreateNetworkCommand {
                name: field("name"),
                purpose: field("purpose"),
                icon: field("icon"),
                language: field("language"),
                region: field("region"),
                tags: split_names(&field("tags")),
                capability_declarations: parse_capability_declarations(
                    action,
                    "capability_declarations",
                    &field("capability_declarations"),
                )?,
                visibility: parse_visibility(field("visibility"))?,
                join_mode: parse_join_mode(field("join_mode"))?,
                exit_contribution,
                exit_contribution_scope,
                exit_contribution_mode,
                exit_selection_policy: parse_exit_selection_policy(
                    action,
                    "exit_selection_policy_json",
                    &field("exit_selection_policy_json"),
                    true,
                )?
                .unwrap_or_default(),
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
        Ok(ActionBinding::PreviewNetworkProfile | ActionBinding::UpdateNetworkProfile) => {
            let exit_contribution_mode =
                parse_exit_contribution_mode(action, &required("exit_contribution_mode")?)?;
            let (exit_contribution, exit_contribution_scope) = resolve_exit_contribution_fields(
                action,
                exit_contribution_mode,
                &required("exit_contribution_scope")?,
            )?;
            let update = NetworkProfileUpdate {
                purpose: field("purpose"),
                icon: field("icon"),
                language: field("language"),
                region: field("region"),
                tags: split_names(&field("tags")),
                capability_declarations: parse_capability_declarations(
                    action,
                    "capability_declarations",
                    &field("capability_declarations"),
                )?,
                visibility: parse_visibility(field("visibility"))?,
                join_mode: parse_join_mode(field("join_mode"))?,
                exit_contribution,
                exit_contribution_scope,
                exit_contribution_mode,
                exit_selection_policy: parse_exit_selection_policy(
                    action,
                    "exit_selection_policy_json",
                    &field("exit_selection_policy_json"),
                    false,
                )?,
            };
            let command = NodeNetworkProfileCommand {
                network_name: field("network_name"),
                expected_revision: parse_number(
                    action,
                    "expected_revision",
                    &required("expected_revision")?,
                )?,
                update,
            };
            if action.id == "preview_network_profile" {
                Ok(NodeCommand::PreviewNetworkProfile(command))
            } else {
                Ok(NodeCommand::UpdateNetworkProfile(command))
            }
        }
        Ok(ActionBinding::PublishNetworkDiscovery) => Ok(NodeCommand::PublishNetworkDiscovery(
            NodeNetworkDiscoveryCommand {
                network_name: field("network_name"),
                index_url: field("index_url"),
            },
        )),
        Ok(ActionBinding::UpdateNetworkDiscovery) => Ok(NodeCommand::UpdateNetworkDiscovery(
            NodeNetworkDiscoveryCommand {
                network_name: field("network_name"),
                index_url: field("index_url"),
            },
        )),
        Ok(ActionBinding::UnpublishNetworkDiscovery) => Ok(NodeCommand::UnpublishNetworkDiscovery(
            NodeNetworkDiscoveryCommand {
                network_name: field("network_name"),
                index_url: field("index_url"),
            },
        )),
        Ok(ActionBinding::RefreshDiscoveryIndex) => Ok(NodeCommand::RefreshDiscoveryIndex {
            index_url: optional_non_empty(field("index_url")),
        }),
        Ok(ActionBinding::RetryDiscoveryIndex) => Ok(NodeCommand::RetryDiscoveryIndex),
        Ok(ActionBinding::PreviewJoinNetwork) => Ok(NodeCommand::PreviewJoinNetwork(
            NodePreviewJoinNetworkCommand {
                network_id: field("network_id"),
            },
        )),
        Ok(ActionBinding::JoinNetwork) => {
            let target = field("target");
            let network_id = optional_non_empty(field("network_id"));
            let (
                network_id,
                preview_digest,
                network_policy_revision,
                privacy_policy_revision,
                terms_hash,
                acknowledged_at_unix_ms,
            ) = if let Some(network_id) = network_id {
                fuckipv6_common::domain::NetworkId::parse(network_id.clone()).map_err(|_| {
                    CommandBuildError::InvalidField {
                        action_id: action.id.clone(),
                        field_id: "network_id".into(),
                        value: network_id.clone(),
                    }
                })?;
                let preview_digest = required("preview_digest")?;
                if preview_digest.trim().is_empty() {
                    return Err(CommandBuildError::InvalidField {
                        action_id: action.id.clone(),
                        field_id: "preview_digest".into(),
                        value: preview_digest,
                    });
                }
                let terms_hash = required("terms_hash")?;
                let terms_hash = fuckipv6_common::domain::ContentHash::parse(terms_hash.clone())
                    .map_err(|_| CommandBuildError::InvalidField {
                        action_id: action.id.clone(),
                        field_id: "terms_hash".into(),
                        value: terms_hash.clone(),
                    })?;
                (
                    Some(network_id),
                    Some(preview_digest),
                    Some(parse_number::<u64>(
                        action,
                        "network_policy_revision",
                        &required("network_policy_revision")?,
                    )?),
                    Some(parse_number::<u64>(
                        action,
                        "privacy_policy_revision",
                        &required("privacy_policy_revision")?,
                    )?),
                    Some(terms_hash),
                    Some(parse_number::<u64>(
                        action,
                        "acknowledged_at_unix_ms",
                        &required("acknowledged_at_unix_ms")?,
                    )?),
                )
            } else {
                if target.trim().is_empty() {
                    return Err(CommandBuildError::InvalidField {
                        action_id: action.id.clone(),
                        field_id: "target".into(),
                        value: target,
                    });
                }
                (None, None, None, None, None, None)
            };
            Ok(NodeCommand::JoinNetwork(Box::new(NodeJoinNetworkCommand {
                target,
                network_name: optional_non_empty(field("network_name")),
                start: parse_bool(action, "start", &required("start")?)?,
                discovery_record: None,
                network_id,
                preview_digest,
                network_policy_revision,
                privacy_policy_revision,
                terms_hash,
                acknowledged_at_unix_ms,
                exit_contribution_acknowledged: optional_non_empty(field(
                    "exit_contribution_acknowledged",
                ))
                .map(|value| parse_bool(action, "exit_contribution_acknowledged", &value))
                .transpose()?,
                signed_join_preview: None,
            })))
        }
        Ok(ActionBinding::SubmitJoinRequest) => Ok(NodeCommand::SubmitJoinRequest(
            NodeSubmitJoinRequestCommand {
                network_id: field("network_id"),
                reason: field("reason"),
            },
        )),
        Ok(ActionBinding::ListJoinRequests) => {
            Ok(NodeCommand::ListJoinRequests(NodeListJoinRequestsCommand {
                network_id: field("network_id"),
            }))
        }
        Ok(ActionBinding::ListNetworkMembers) => {
            Ok(NodeCommand::ListNetworkMembers(NodeNetworkMembersCommand {
                network_id: field("network_id"),
            }))
        }
        Ok(ActionBinding::PreviewMemberRemoval) => Ok(NodeCommand::PreviewMemberRemoval(
            NodeMemberRemovalPreviewCommand {
                network_id: parse_network_id(action, "network_id", &required("network_id")?)?
                    .to_string(),
                target_node_id: parse_node_id(
                    action,
                    "target_node_id",
                    &required("target_node_id")?,
                )?
                .to_string(),
            },
        )),
        Ok(ActionBinding::ChangeMemberLifecycle) => Ok(NodeCommand::ChangeMemberLifecycle(
            NodeChangeMemberLifecycleCommand {
                network_id: parse_network_id(action, "network_id", &required("network_id")?)?
                    .to_string(),
                target_node_id: parse_node_id(
                    action,
                    "target_node_id",
                    &required("target_node_id")?,
                )?
                .to_string(),
                action: parse_member_lifecycle_action(action, &required("action")?)?,
                role: parse_invitation_role(&action.id, &required("role")?)?,
                expected_revision: parse_number(
                    action,
                    "expected_revision",
                    &required("expected_revision")?,
                )?,
                reason: parse_lifecycle_reason(action, &field("reason"))?,
            },
        )),
        Ok(ActionBinding::ReviewJoinRequest) => {
            let review_action = match field("action").as_str() {
                "approve" => JoinRequestReviewAction::Approve,
                "reject" => JoinRequestReviewAction::Reject,
                "request_information" => JoinRequestReviewAction::RequestInformation,
                _ => {
                    return Err(CommandBuildError::InvalidField {
                        action_id: action.id.clone(),
                        field_id: "action".into(),
                        value: "[invalid review action]".into(),
                    });
                }
            };
            Ok(NodeCommand::ReviewJoinRequest(
                NodeReviewJoinRequestCommand {
                    network_id: field("network_id"),
                    review: NodeJoinRequestReviewItem {
                        request_id: field("request_id"),
                        expected_revision: parse_number::<u64>(
                            action,
                            "expected_revision",
                            &required("expected_revision")?,
                        )?,
                        action: review_action,
                        note: optional_non_empty(field("note")),
                    },
                },
            ))
        }
        Ok(ActionBinding::BatchReviewJoinRequests) => {
            let mut reviews = serde_json::from_str::<Vec<NodeJoinRequestReviewItem>>(&field(
                "reviews",
            ))
            .map_err(|_| CommandBuildError::InvalidField {
                action_id: action.id.clone(),
                field_id: "reviews".into(),
                value: "[invalid review batch]".into(),
            })?;
            if let Some(note) = optional_non_empty(field("note")) {
                for review in &mut reviews {
                    if review.action != JoinRequestReviewAction::Approve {
                        review.note = Some(note.clone());
                    }
                }
            }
            Ok(NodeCommand::BatchReviewJoinRequests(
                NodeBatchReviewJoinRequestsCommand {
                    network_id: field("network_id"),
                    reviews,
                },
            ))
        }
        Ok(ActionBinding::WithdrawJoinRequest) => Ok(NodeCommand::WithdrawJoinRequest(
            NodeWithdrawJoinRequestCommand {
                network_id: field("network_id"),
                request_id: field("request_id"),
            },
        )),
        Ok(ActionBinding::ResubmitJoinRequest) => Ok(NodeCommand::ResubmitJoinRequest(
            NodeResubmitJoinRequestCommand {
                network_id: field("network_id"),
                request_id: field("request_id"),
                expected_revision: parse_number::<u64>(
                    action,
                    "expected_revision",
                    &required("expected_revision")?,
                )?,
                reason: field("reason"),
            },
        )),
        Ok(ActionBinding::CreateInvitation) => {
            Ok(NodeCommand::CreateInvitation(NodeCreateInvitationCommand {
                network_id: parse_network_id(action, "network_id", &field("network_id"))?,
                role: parse_invitation_role(&action.id, &required("role")?)?,
                capabilities: parse_capability_declarations(
                    action,
                    "capabilities",
                    &field("capabilities"),
                )?,
                max_uses: parse_number(action, "max_uses", &required("max_uses")?)?,
                expires_in_ms: parse_number(action, "expires_in_ms", &required("expires_in_ms")?)?,
            }))
        }
        Ok(ActionBinding::ListInvitations) => {
            Ok(NodeCommand::ListInvitations(NodeListInvitationsCommand {
                network_id: parse_network_id(action, "network_id", &field("network_id"))?,
            }))
        }
        Ok(ActionBinding::GetInvitationStatus) => Ok(NodeCommand::GetInvitationStatus(
            NodeGetInvitationStatusCommand {
                network_id: parse_network_id(action, "network_id", &field("network_id"))?,
                invitation_id: parse_invitation_id(
                    action,
                    "invitation_id",
                    &required("invitation_id")?,
                )?,
            },
        )),
        Ok(ActionBinding::RevokeInvitation) => {
            Ok(NodeCommand::RevokeInvitation(NodeRevokeInvitationCommand {
                network_id: parse_network_id(action, "network_id", &field("network_id"))?,
                invitation_id: parse_invitation_id(
                    action,
                    "invitation_id",
                    &required("invitation_id")?,
                )?,
                expected_revision: parse_number(
                    action,
                    "expected_revision",
                    &required("expected_revision")?,
                )?,
            }))
        }
        Ok(ActionBinding::PreviewInvitation) => Ok(NodeCommand::PreviewInvitation(
            NodePreviewInvitationCommand {
                network_id: parse_network_id(action, "network_id", &field("network_id"))?,
                coordinator_url: field("coordinator_url"),
                code: parse_invitation_code(action, &required("code")?)?,
            },
        )),
        Ok(ActionBinding::RedeemInvitation) => {
            Ok(NodeCommand::RedeemInvitation(NodeRedeemInvitationCommand {
                coordinator_url: field("coordinator_url"),
                request: InvitationRedeemRequest {
                    network_id: parse_network_id(action, "network_id", &field("network_id"))?,
                    code: parse_invitation_code(action, &required("code")?)?,
                    invitation_id: parse_invitation_id(
                        action,
                        "invitation_id",
                        &required("invitation_id")?,
                    )?,
                    expected_invitation_revision: parse_number(
                        action,
                        "expected_invitation_revision",
                        &required("expected_invitation_revision")?,
                    )?,
                    preview_digest: parse_content_hash(
                        action,
                        "preview_digest",
                        &required("preview_digest")?,
                    )?,
                    terms_hash: parse_content_hash(action, "terms_hash", &required("terms_hash")?)?,
                    acknowledged_at_unix_ms: parse_number(
                        action,
                        "acknowledged_at_unix_ms",
                        &required("acknowledged_at_unix_ms")?,
                    )?,
                    exit_contribution_acknowledged: Some(parse_bool(
                        action,
                        "exit_contribution_acknowledged",
                        &required("exit_contribution_acknowledged")?,
                    )?),
                },
            }))
        }
        Ok(ActionBinding::LeaveNetwork) => Ok(NodeCommand::LeaveNetwork(NodeLeaveNetworkCommand {
            network_name: field("network_name"),
        })),
        Ok(ActionBinding::AcknowledgeMembershipPolicy) => Ok(
            NodeCommand::AcknowledgeMembershipPolicy(NodeAcknowledgeMembershipPolicyCommand {
                network_id: parse_network_id(action, "network_id", &required("network_id")?)?
                    .to_string(),
                expected_terms_hash: parse_content_hash(
                    action,
                    "expected_terms_hash",
                    &required("expected_terms_hash")?,
                )?,
                acknowledged_at_unix_ms: parse_number(
                    action,
                    "acknowledged_at_unix_ms",
                    &required("acknowledged_at_unix_ms")?,
                )?,
                exit_contribution_acknowledged: optional_non_empty(field(
                    "exit_contribution_acknowledged",
                ))
                .map(|value| parse_bool(action, "exit_contribution_acknowledged", &value))
                .transpose()?,
            }),
        ),
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
        Ok(ActionBinding::RebuildTun) => Ok(NodeCommand::RebuildTun {
            network_name: field("network_name"),
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
        Ok(ActionBinding::SetLocalExitCapability) => Ok(NodeCommand::SetLocalExitCapability(
            NodeSetLocalExitCapabilityCommand {
                enabled: parse_bool(action, "enabled", &required("enabled")?)?,
            },
        )),
        Ok(ActionBinding::SetNodeDisplayName) => Ok(NodeCommand::SetNodeDisplayName(
            NodeSetNodeDisplayNameCommand {
                display_name: optional_non_empty(field("display_name")),
            },
        )),
        Ok(ActionBinding::RotateNodeIdentity) => Ok(NodeCommand::RotateNodeIdentity(
            NodeRotateNodeIdentityCommand {
                expected_fingerprint: parse_fingerprint(
                    action,
                    "expected_fingerprint",
                    &required("expected_fingerprint")?,
                )?,
            },
        )),
        Ok(ActionBinding::StartNode) => Ok(NodeCommand::StartNode),
        Ok(ActionBinding::StopNode) => Ok(NodeCommand::StopNode),
        Ok(ActionBinding::RestartNode) => Ok(NodeCommand::RestartNode),
        Ok(ActionBinding::CleanupResiduals) => Ok(NodeCommand::CleanupResiduals {
            networks: split_names(&field("networks")),
            all: parse_bool(action, "all", &field("all"))?,
        }),
        Ok(ActionBinding::GetConfiguration) => Ok(NodeCommand::GetConfiguration),
        Ok(ActionBinding::PreviewConfiguration) => Ok(NodeCommand::PreviewConfiguration(
            NodePreviewConfigurationCommand {
                expected_revision: parse_number(
                    action,
                    "expected_revision",
                    &required("expected_revision")?,
                )?,
                networks: parse_configuration_networks(action, "networks", &required("networks")?)?,
            },
        )),
        Ok(ActionBinding::ApplyConfiguration) => Ok(NodeCommand::ApplyConfiguration(
            NodeApplyConfigurationCommand {
                expected_revision: parse_number(
                    action,
                    "expected_revision",
                    &required("expected_revision")?,
                )?,
                networks: parse_configuration_networks(action, "networks", &required("networks")?)?,
            },
        )),
        Ok(ActionBinding::RollbackConfiguration) => Ok(NodeCommand::RollbackConfiguration(
            NodeRollbackConfigurationCommand {
                expected_revision: parse_number(
                    action,
                    "expected_revision",
                    &required("expected_revision")?,
                )?,
                target_revision: parse_number(
                    action,
                    "target_revision",
                    &required("target_revision")?,
                )?,
            },
        )),
        Ok(ActionBinding::GetConfigurationHistory) => Ok(NodeCommand::GetConfigurationHistory),
        Ok(ActionBinding::GetPolicy) => Ok(NodeCommand::GetPolicy),
        Ok(ActionBinding::PreviewPolicy) => {
            Ok(NodeCommand::PreviewPolicy(NodePreviewPolicyCommand {
                expected_revision: parse_number(
                    action,
                    "expected_revision",
                    &required("expected_revision")?,
                )?,
                candidate: parse_policy_snapshot(
                    action,
                    "candidate_json",
                    &required("candidate_json")?,
                )?,
            }))
        }
        Ok(ActionBinding::PreviewPolicyRollback) => Ok(NodeCommand::PreviewPolicyRollback(
            NodePreviewPolicyRollbackCommand {
                expected_revision: parse_number(
                    action,
                    "expected_revision",
                    &required("expected_revision")?,
                )?,
                target_revision: parse_number(
                    action,
                    "target_revision",
                    &required("target_revision")?,
                )?,
            },
        )),
        Ok(ActionBinding::ApplyPolicy) => Ok(NodeCommand::ApplyPolicy(NodeApplyPolicyCommand {
            expected_revision: parse_number(
                action,
                "expected_revision",
                &required("expected_revision")?,
            )?,
            candidate: parse_policy_snapshot(
                action,
                "candidate_json",
                &required("candidate_json")?,
            )?,
            network_ids: parse_policy_network_ids(
                action,
                "network_ids",
                &required("network_ids")?,
            )?,
            includes_local_policy: parse_bool(
                action,
                "includes_local_policy",
                &required("includes_local_policy")?,
            )?,
            confirm_privilege_expansion: parse_bool(
                action,
                "confirm_privilege_expansion",
                &required("confirm_privilege_expansion")?,
            )?,
        })),
        Ok(ActionBinding::RollbackPolicy) => {
            Ok(NodeCommand::RollbackPolicy(NodeRollbackPolicyCommand {
                expected_revision: parse_number(
                    action,
                    "expected_revision",
                    &required("expected_revision")?,
                )?,
                target_revision: parse_number(
                    action,
                    "target_revision",
                    &required("target_revision")?,
                )?,
                network_ids: parse_policy_network_ids(
                    action,
                    "network_ids",
                    &required("network_ids")?,
                )?,
                includes_local_policy: parse_bool(
                    action,
                    "includes_local_policy",
                    &required("includes_local_policy")?,
                )?,
                confirm_privilege_expansion: parse_bool(
                    action,
                    "confirm_privilege_expansion",
                    &required("confirm_privilege_expansion")?,
                )?,
            }))
        }
        Ok(ActionBinding::GetPolicyHistory) => Ok(NodeCommand::GetPolicyHistory),
        Ok(ActionBinding::TestPolicy) => {
            let policy_json = required("policy_json")?;
            let input = parse_policy_test_input(&policy_json).map_err(|issue| {
                CommandBuildError::InvalidField {
                    action_id: action.id.clone(),
                    field_id: "policy_json".into(),
                    value: format!("{}: {}", issue.path, issue.message),
                }
            })?;
            Ok(NodeCommand::TestPolicy(input))
        }
        Ok(ActionBinding::GetTrafficPolicy) => Ok(NodeCommand::GetTrafficPolicy),
        Ok(ActionBinding::PreviewTrafficPolicy) => Ok(NodeCommand::PreviewTrafficPolicy(
            NodePreviewTrafficPolicyCommand {
                expected_revision: parse_number(
                    action,
                    "expected_revision",
                    &required("expected_revision")?,
                )?,
                draft: parse_traffic_policy_draft(action, "draft_json", &required("draft_json")?)?,
            },
        )),
        Ok(ActionBinding::PreviewTrafficPolicyRollback) => Ok(
            NodeCommand::PreviewTrafficPolicyRollback(NodePreviewTrafficPolicyRollbackCommand {
                expected_revision: parse_number(
                    action,
                    "expected_revision",
                    &required("expected_revision")?,
                )?,
                target_revision: parse_number(
                    action,
                    "target_revision",
                    &required("target_revision")?,
                )?,
            }),
        ),
        Ok(ActionBinding::ApplyTrafficPolicy) => Ok(NodeCommand::ApplyTrafficPolicy(
            NodeApplyTrafficPolicyCommand {
                expected_revision: parse_number(
                    action,
                    "expected_revision",
                    &required("expected_revision")?,
                )?,
                draft: parse_traffic_policy_draft(action, "draft_json", &required("draft_json")?)?,
                confirm_risk: parse_bool(action, "confirm_risk", &required("confirm_risk")?)?,
            },
        )),
        Ok(ActionBinding::RollbackTrafficPolicy) => Ok(NodeCommand::RollbackTrafficPolicy(
            NodeRollbackTrafficPolicyCommand {
                expected_revision: parse_number(
                    action,
                    "expected_revision",
                    &required("expected_revision")?,
                )?,
                target_revision: parse_number(
                    action,
                    "target_revision",
                    &required("target_revision")?,
                )?,
                confirm_risk: parse_bool(action, "confirm_risk", &required("confirm_risk")?)?,
            },
        )),
        Ok(ActionBinding::GetTrafficPolicyHistory) => Ok(NodeCommand::GetTrafficPolicyHistory),
        Ok(ActionBinding::TestTrafficPolicy) => Ok(NodeCommand::TestTrafficPolicy(
            NodeTestTrafficPolicyCommand {
                current: parse_traffic_policy(action, "current_json", &required("current_json")?)?,
                candidate: parse_traffic_policy(
                    action,
                    "candidate_json",
                    &required("candidate_json")?,
                )?,
                input: parse_traffic_policy_test_input(
                    action,
                    "input_json",
                    &required("input_json")?,
                )?,
            },
        )),
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
            "purpose",
            "icon",
            "language",
            "region",
            "tags",
            "capability_declarations",
            "visibility",
            "join_mode",
            "exit_contribution_mode",
            "exit_contribution",
            "exit_contribution_scope",
            "exit_selection_policy_json",
            "signaling_port",
            "relay_port",
            "external_host",
            "auto_connect",
        ]),
        "preview_network_profile" | "update_network_profile" => HashSet::from([
            "network_name",
            "expected_revision",
            "purpose",
            "icon",
            "language",
            "region",
            "tags",
            "capability_declarations",
            "visibility",
            "join_mode",
            "exit_contribution_mode",
            "exit_contribution",
            "exit_contribution_scope",
            "exit_selection_policy_json",
        ]),
        "publish_network_discovery"
        | "update_network_discovery"
        | "unpublish_network_discovery" => HashSet::from(["network_name", "index_url"]),
        "refresh_discovery_index" => HashSet::from(["index_url"]),
        "retry_discovery_index" => HashSet::from([]),
        "preview_join_network" => HashSet::from(["network_id"]),
        // The signed preview confirmation fields are populated internally by
        // the GPUI workflow rather than declared as ordinary manifest inputs.
        "join_network" => HashSet::from(["target", "network_name", "start"]),
        "submit_join_request" => HashSet::from(["network_id", "reason"]),
        "list_join_requests" => HashSet::from(["network_id"]),
        "list_network_members" => HashSet::from(["network_id"]),
        "preview_member_removal" => HashSet::from(["network_id", "target_node_id"]),
        "change_member_lifecycle" => HashSet::from([
            "network_id",
            "target_node_id",
            "action",
            "role",
            "expected_revision",
            "reason",
        ]),
        "review_join_request" => HashSet::from([
            "network_id",
            "request_id",
            "expected_revision",
            "action",
            "note",
        ]),
        "batch_review_join_requests" => HashSet::from(["network_id", "reviews", "note"]),
        "withdraw_join_request" => HashSet::from(["network_id", "request_id"]),
        "resubmit_join_request" => {
            HashSet::from(["network_id", "request_id", "expected_revision", "reason"])
        }
        "create_invitation" => HashSet::from([
            "network_id",
            "role",
            "capabilities",
            "max_uses",
            "expires_in_ms",
        ]),
        "list_invitations" => HashSet::from(["network_id"]),
        "get_invitation_status" => HashSet::from(["network_id", "invitation_id"]),
        "revoke_invitation" => HashSet::from(["network_id", "invitation_id", "expected_revision"]),
        "preview_invitation" => HashSet::from(["network_id", "coordinator_url", "code"]),
        "redeem_invitation" => HashSet::from([
            "network_id",
            "coordinator_url",
            "code",
            "invitation_id",
            "expected_invitation_revision",
            "preview_digest",
            "terms_hash",
            "acknowledged_at_unix_ms",
        ]),
        "leave_network" => HashSet::from(["network_name"]),
        "acknowledge_membership_policy" => HashSet::from([
            "network_id",
            "expected_terms_hash",
            "acknowledged_at_unix_ms",
            "exit_contribution_acknowledged",
        ]),
        "start_networks" => HashSet::from(["selection"]),
        "stop_networks" => HashSet::from(["networks", "all"]),
        "restart_networks" => HashSet::from(["networks", "all"]),
        "rebuild_tun" => HashSet::from(["network_name"]),
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
        "set_local_exit_capability" => HashSet::from(["enabled"]),
        "set_node_display_name" => HashSet::from(["display_name"]),
        "rotate_node_identity" => HashSet::from(["expected_fingerprint"]),
        "get_configuration" | "get_configuration_history" => HashSet::from([]),
        "preview_configuration" | "apply_configuration" => {
            HashSet::from(["expected_revision", "networks"])
        }
        "rollback_configuration" => HashSet::from(["expected_revision", "target_revision"]),
        "get_policy" | "get_policy_history" => HashSet::from([]),
        "preview_policy" => HashSet::from(["expected_revision", "candidate_json"]),
        "preview_policy_rollback" => HashSet::from(["expected_revision", "target_revision"]),
        "apply_policy" => HashSet::from([
            "expected_revision",
            "candidate_json",
            "network_ids",
            "includes_local_policy",
            "confirm_privilege_expansion",
        ]),
        "rollback_policy" => HashSet::from([
            "expected_revision",
            "target_revision",
            "network_ids",
            "includes_local_policy",
            "confirm_privilege_expansion",
        ]),
        "test_policy" => HashSet::from(["policy_json"]),
        "get_traffic_policy" | "get_traffic_policy_history" => HashSet::from([]),
        "preview_traffic_policy" => HashSet::from(["expected_revision", "draft_json"]),
        "preview_traffic_policy_rollback" => {
            HashSet::from(["expected_revision", "target_revision"])
        }
        "apply_traffic_policy" => {
            HashSet::from(["expected_revision", "draft_json", "confirm_risk"])
        }
        "rollback_traffic_policy" => {
            HashSet::from(["expected_revision", "target_revision", "confirm_risk"])
        }
        "test_traffic_policy" => HashSet::from(["current_json", "candidate_json", "input_json"]),
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

fn parse_configuration_networks(
    action: &NodeUiAction,
    field_id: &str,
    value: &str,
) -> Result<BTreeMap<String, serde_json::Value>, CommandBuildError> {
    let value: serde_json::Value =
        serde_json::from_str(value).map_err(|_| CommandBuildError::InvalidField {
            action_id: action.id.clone(),
            field_id: field_id.into(),
            value: value.into(),
        })?;
    let networks = value
        .as_object()
        .and_then(|object| {
            object
                .get("networks")
                .and_then(serde_json::Value::as_object)
                .or(Some(object))
        })
        .ok_or_else(|| CommandBuildError::InvalidField {
            action_id: action.id.clone(),
            field_id: field_id.into(),
            value: String::new(),
        })?;
    networks
        .iter()
        .map(|(name, config)| {
            if !config.is_object() {
                return Err(CommandBuildError::InvalidField {
                    action_id: action.id.clone(),
                    field_id: field_id.into(),
                    value: name.clone(),
                });
            }
            Ok((name.clone(), config.clone()))
        })
        .collect()
}

fn parse_fingerprint(
    action: &NodeUiAction,
    field_id: &str,
    value: &str,
) -> Result<String, CommandBuildError> {
    let invalid = || CommandBuildError::InvalidField {
        action_id: action.id.clone(),
        field_id: field_id.into(),
        value: value.into(),
    };
    if value.trim() != value {
        return Err(invalid());
    }
    let decoded = URL_SAFE_NO_PAD.decode(value).map_err(|_| invalid())?;
    if decoded.len() != 32 {
        return Err(invalid());
    }
    Ok(value.to_owned())
}

fn parse_visibility(value: String) -> Result<NetworkVisibility, CommandBuildError> {
    match value.as_str() {
        "public" => Ok(NetworkVisibility::Public),
        "unlisted" => Ok(NetworkVisibility::Unlisted),
        "private" => Ok(NetworkVisibility::Private),
        _ => Err(CommandBuildError::InvalidField {
            action_id: "create_network".into(),
            field_id: "visibility".into(),
            value,
        }),
    }
}

fn parse_join_mode(value: String) -> Result<NetworkJoinMode, CommandBuildError> {
    match value.as_str() {
        "open" => Ok(NetworkJoinMode::Open),
        "approval_required" => Ok(NetworkJoinMode::ApprovalRequired),
        "invite_only" => Ok(NetworkJoinMode::InviteOnly),
        _ => Err(CommandBuildError::InvalidField {
            action_id: "create_network".into(),
            field_id: "join_mode".into(),
            value,
        }),
    }
}

fn resolve_exit_contribution_fields(
    action: &NodeUiAction,
    mode: ExitContributionMode,
    requested_scope: &str,
) -> Result<(ExitContributionObligation, ExitContributionScope), CommandBuildError> {
    let requested_scope = parse_exit_contribution_scope(action, requested_scope)?;
    match mode {
        ExitContributionMode::RequiredOnJoin => {
            if requested_scope == ExitContributionScope::NotApplicable {
                return Err(CommandBuildError::InvalidField {
                    action_id: action.id.clone(),
                    field_id: "exit_contribution_scope".into(),
                    value: "not_applicable".into(),
                });
            }
            Ok((ExitContributionObligation::Required, requested_scope))
        }
        ExitContributionMode::Autonomous
        | ExitContributionMode::Disabled
        | ExitContributionMode::ApprovalRequired => Ok((
            ExitContributionObligation::NotRequired,
            ExitContributionScope::NotApplicable,
        )),
    }
}

fn parse_exit_contribution_mode(
    action: &NodeUiAction,
    value: &str,
) -> Result<ExitContributionMode, CommandBuildError> {
    match value {
        "autonomous" => Ok(ExitContributionMode::Autonomous),
        "required_on_join" => Ok(ExitContributionMode::RequiredOnJoin),
        "disabled" => Ok(ExitContributionMode::Disabled),
        "approval_required" => Ok(ExitContributionMode::ApprovalRequired),
        _ => Err(CommandBuildError::InvalidField {
            action_id: action.id.clone(),
            field_id: "exit_contribution_mode".into(),
            value: value.into(),
        }),
    }
}

fn exit_contribution_mode_wire_value(mode: ExitContributionMode) -> &'static str {
    match mode {
        ExitContributionMode::Autonomous => "autonomous",
        ExitContributionMode::RequiredOnJoin => "required_on_join",
        ExitContributionMode::Disabled => "disabled",
        ExitContributionMode::ApprovalRequired => "approval_required",
    }
}

fn parse_exit_contribution_scope(
    action: &NodeUiAction,
    value: &str,
) -> Result<ExitContributionScope, CommandBuildError> {
    match value {
        "not_applicable" => Ok(ExitContributionScope::NotApplicable),
        "joining_members" => Ok(ExitContributionScope::JoiningMembers),
        "all_members" => Ok(ExitContributionScope::AllMembers),
        _ => Err(CommandBuildError::InvalidField {
            action_id: action.id.clone(),
            field_id: "exit_contribution_scope".into(),
            value: value.into(),
        }),
    }
}

fn parse_exit_selection_policy(
    action: &NodeUiAction,
    field_id: &str,
    value: &str,
    empty_is_default: bool,
) -> Result<Option<ExitSelectionPolicy>, CommandBuildError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(empty_is_default.then(ExitSelectionPolicy::default));
    }
    let policy = serde_json::from_str::<ExitSelectionPolicy>(trimmed).map_err(|_| {
        CommandBuildError::InvalidField {
            action_id: action.id.clone(),
            field_id: field_id.into(),
            value: value.into(),
        }
    })?;
    policy
        .validate()
        .map_err(|_| CommandBuildError::InvalidField {
            action_id: action.id.clone(),
            field_id: field_id.into(),
            value: value.into(),
        })?;
    Ok(Some(policy))
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

fn parse_policy_snapshot(
    action: &NodeUiAction,
    field_id: &str,
    value: &str,
) -> Result<PolicySnapshot, CommandBuildError> {
    serde_json::from_str(value).map_err(|_| CommandBuildError::InvalidField {
        action_id: action.id.clone(),
        field_id: field_id.into(),
        value: value.into(),
    })
}

fn parse_policy_network_ids(
    action: &NodeUiAction,
    field_id: &str,
    value: &str,
) -> Result<Vec<String>, CommandBuildError> {
    let ids = value
        .split([',', ' ', '\t', '\n', '\r'])
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let unique = ids.iter().collect::<HashSet<_>>();
    if ids.is_empty()
        || unique.len() != ids.len()
        || ids.iter().any(|id| {
            !id.strip_prefix("net_").is_some_and(|suffix| {
                suffix.len() == 43
                    && suffix
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
            })
        })
    {
        return Err(CommandBuildError::InvalidField {
            action_id: action.id.clone(),
            field_id: field_id.into(),
            value: value.into(),
        });
    }
    Ok(ids)
}

fn parse_traffic_policy_draft(
    action: &NodeUiAction,
    field_id: &str,
    value: &str,
) -> Result<TrafficPolicyDraft, CommandBuildError> {
    serde_json::from_str(value).map_err(|_| CommandBuildError::InvalidField {
        action_id: action.id.clone(),
        field_id: field_id.into(),
        value: value.into(),
    })
}

fn parse_traffic_policy(
    action: &NodeUiAction,
    field_id: &str,
    value: &str,
) -> Result<TrafficPolicy, CommandBuildError> {
    serde_json::from_str(value).map_err(|_| CommandBuildError::InvalidField {
        action_id: action.id.clone(),
        field_id: field_id.into(),
        value: value.into(),
    })
}

fn parse_traffic_policy_test_input(
    action: &NodeUiAction,
    field_id: &str,
    value: &str,
) -> Result<TrafficPolicyTestInput, CommandBuildError> {
    serde_json::from_str(value).map_err(|_| CommandBuildError::InvalidField {
        action_id: action.id.clone(),
        field_id: field_id.into(),
        value: value.into(),
    })
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

fn parse_capability_declarations(
    action: &NodeUiAction,
    field_id: &str,
    value: &str,
) -> Result<Vec<CapabilityKind>, CommandBuildError> {
    split_names(value)
        .into_iter()
        .map(|value| match value.as_str() {
            "coordinator" => Ok(CapabilityKind::Coordinator),
            "relay" => Ok(CapabilityKind::Relay),
            "exit" | "exit_node" | "exit-node" => Ok(CapabilityKind::Exit),
            "file" => Ok(CapabilityKind::File),
            _ => Err(CommandBuildError::InvalidField {
                action_id: action.id.clone(),
                field_id: field_id.into(),
                value,
            }),
        })
        .collect()
}

fn parse_network_id(
    action: &NodeUiAction,
    field_id: &str,
    value: &str,
) -> Result<NetworkId, CommandBuildError> {
    NetworkId::parse(value).map_err(|_| CommandBuildError::InvalidField {
        action_id: action.id.clone(),
        field_id: field_id.into(),
        value: value.into(),
    })
}

fn parse_invitation_id(
    action: &NodeUiAction,
    field_id: &str,
    value: &str,
) -> Result<InvitationId, CommandBuildError> {
    InvitationId::parse(value).map_err(|_| CommandBuildError::InvalidField {
        action_id: action.id.clone(),
        field_id: field_id.into(),
        value: value.into(),
    })
}

fn parse_content_hash(
    action: &NodeUiAction,
    field_id: &str,
    value: &str,
) -> Result<ContentHash, CommandBuildError> {
    ContentHash::parse(value).map_err(|_| CommandBuildError::InvalidField {
        action_id: action.id.clone(),
        field_id: field_id.into(),
        value: value.into(),
    })
}

fn parse_invitation_code(
    action: &NodeUiAction,
    value: &str,
) -> Result<InvitationCode, CommandBuildError> {
    InvitationCode::parse(value).map_err(|_| CommandBuildError::InvalidField {
        action_id: action.id.clone(),
        field_id: "code".into(),
        value: "[redacted]".into(),
    })
}

fn parse_member_lifecycle_action(
    action: &NodeUiAction,
    value: &str,
) -> Result<MemberLifecycleAction, CommandBuildError> {
    match value.trim() {
        "invite" => Ok(MemberLifecycleAction::Invite),
        "approve" => Ok(MemberLifecycleAction::Approve),
        "reject" => Ok(MemberLifecycleAction::Reject),
        "suspend" => Ok(MemberLifecycleAction::Suspend),
        "resume" => Ok(MemberLifecycleAction::Resume),
        "exit" => Ok(MemberLifecycleAction::Exit),
        "remove" => Ok(MemberLifecycleAction::Remove),
        "undo_remove" => Ok(MemberLifecycleAction::UndoRemove),
        _ => Err(CommandBuildError::InvalidField {
            action_id: action.id.clone(),
            field_id: "action".into(),
            value: value.into(),
        }),
    }
}

fn parse_lifecycle_reason(
    action: &NodeUiAction,
    value: &str,
) -> Result<Option<String>, CommandBuildError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().count() > 512 || trimmed.chars().any(char::is_control) {
        return Err(CommandBuildError::InvalidField {
            action_id: action.id.clone(),
            field_id: "reason".into(),
            value: "[invalid lifecycle reason]".into(),
        });
    }
    Ok(Some(trimmed.to_owned()))
}

fn parse_node_id(
    action: &NodeUiAction,
    field_id: &str,
    value: &str,
) -> Result<NodeId, CommandBuildError> {
    NodeId::parse(value).map_err(|_| CommandBuildError::InvalidField {
        action_id: action.id.clone(),
        field_id: field_id.into(),
        value: value.into(),
    })
}

fn parse_invitation_role(
    action_id: &str,
    value: &str,
) -> Result<MembershipRole, CommandBuildError> {
    match value.trim() {
        "owner" => Ok(MembershipRole::Owner),
        "member" => Ok(MembershipRole::Member),
        "administrator" | "admin" => Ok(MembershipRole::Administrator),
        _ => Err(CommandBuildError::InvalidField {
            action_id: action_id.into(),
            field_id: "role".into(),
            value: value.into(),
        }),
    }
}

fn project_page(
    kind: PageKind,
    snapshot: &NodeSnapshot,
    locale: Locale,
    activity_filters: &ActivityFilters,
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
        PageKind::NodeRuntime => {
            let (metrics, sections) = project_node_runtime(snapshot, locale);
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
        PageKind::Activity => {
            let (metrics, sections) = project_activity(snapshot, locale, activity_filters);
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
        navigation: None,
    }
}

fn row_with_navigation(
    title: impl Into<String>,
    detail: impl Into<String>,
    page_id: impl Into<String>,
    network_name: impl Into<String>,
    label: impl Into<String>,
) -> RowModel {
    RowModel {
        title: title.into(),
        detail: detail.into(),
        navigation: Some(RowNavigation {
            page_id: page_id.into(),
            network_name: Some(network_name.into()),
            object_id: None,
            label: label.into(),
        }),
    }
}

fn row_with_page_navigation(
    title: impl Into<String>,
    detail: impl Into<String>,
    page_id: impl Into<String>,
    label: impl Into<String>,
) -> RowModel {
    RowModel {
        title: title.into(),
        detail: detail.into(),
        navigation: Some(RowNavigation {
            page_id: page_id.into(),
            network_name: None,
            object_id: None,
            label: label.into(),
        }),
    }
}

pub(crate) fn traffic_protection_state_key(state: TrafficProtectionState) -> &'static str {
    match state {
        TrafficProtectionState::Idle => "traffic.session.state.idle",
        TrafficProtectionState::Connecting => "traffic.session.state.connecting",
        TrafficProtectionState::Migrating => "traffic.session.state.migrating",
        TrafficProtectionState::Protected => "traffic.session.state.protected",
        TrafficProtectionState::FailClosed => "traffic.session.state.fail_closed",
        TrafficProtectionState::ExplicitDirectFallback => {
            "traffic.session.state.explicit_direct_fallback"
        }
    }
}

fn traffic_session_event_key(kind: TrafficSessionEventKind) -> &'static str {
    match kind {
        TrafficSessionEventKind::Connecting => "traffic.session.event.connecting",
        TrafficSessionEventKind::RecoveryStarted => "traffic.session.event.recovery_started",
        TrafficSessionEventKind::MigrationStarted => "traffic.session.event.migration_started",
        TrafficSessionEventKind::FailureDetected => "traffic.session.event.failure_detected",
        TrafficSessionEventKind::ProtectionEstablished => {
            "traffic.session.event.protection_established"
        }
        TrafficSessionEventKind::FailClosed => "traffic.session.event.fail_closed",
        TrafficSessionEventKind::DirectFallback => "traffic.session.event.direct_fallback",
    }
}

fn project_traffic_session(snapshot: &NodeSnapshot, locale: Locale) -> OverviewTrafficSession {
    let session = &snapshot.traffic_session;
    let validation = session
        .validation
        .as_ref()
        .map(|validation| {
            vec![
                OverviewTrafficValidation {
                    label: t(locale, "traffic.session.validation.tun").to_string(),
                    passed: validation.tun_validated,
                },
                OverviewTrafficValidation {
                    label: t(locale, "traffic.session.validation.routes").to_string(),
                    passed: validation.routes_validated,
                },
                OverviewTrafficValidation {
                    label: t(locale, "traffic.session.validation.dns").to_string(),
                    passed: validation.dns_validated,
                },
                OverviewTrafficValidation {
                    label: t(locale, "traffic.session.validation.reachability").to_string(),
                    passed: validation.reachability_validated,
                },
            ]
        })
        .unwrap_or_default();
    let events = session
        .events
        .iter()
        .rev()
        .take(5)
        .map(|event| OverviewTrafficSessionEvent {
            kind: event.kind,
            kind_label: t(locale, traffic_session_event_key(event.kind)).to_string(),
            state: event.state,
            state_label: t(locale, traffic_protection_state_key(event.state)).to_string(),
            reason_code: event.reason_code.clone(),
            occurred_at_unix_ms: event.occurred_at_unix_ms,
        })
        .collect();

    OverviewTrafficSession {
        state: session.state,
        state_label: t(locale, traffic_protection_state_key(session.state)).to_string(),
        reason_code: session.reason_code.clone(),
        changed_at_unix_ms: session.changed_at_unix_ms,
        validation,
        events,
    }
}

fn project_overview(snapshot: &NodeSnapshot, locale: Locale) -> OverviewModel {
    let primary_status = snapshot.primary_status();
    let traffic_session = project_traffic_session(snapshot, locale);
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

    let activity = ordered_activity_entries(&snapshot.recent_activity)
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
            let detail = tr(
                locale,
                "overview.activity.detail",
                &[("outcome", outcome), ("resource", &entry.resource)],
            );
            OverviewActivity {
                id: entry.audit_id.to_string(),
                action,
                detail,
                command: entry.details.get("command").cloned(),
                occurred_at_unix_ms: entry.occurred_at_unix_ms,
            }
        })
        .collect();

    OverviewModel {
        node_name: node_name.to_string(),
        primary_status,
        primary_page_id: Some("node_runtime".into()),
        issues,
        traffic_session,
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
                action_id: Some("refresh_discovery_index".into()),
                unavailable_reason: None,
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

fn routing_status_key(status: NodeRoutingSummaryStatus) -> &'static str {
    match status {
        NodeRoutingSummaryStatus::Current => "routing.status.current",
        NodeRoutingSummaryStatus::Stale => "routing.status.stale",
        NodeRoutingSummaryStatus::Invalid => "routing.status.invalid",
        NodeRoutingSummaryStatus::ApplyFailed => "routing.status.apply_failed",
    }
}

fn routing_default_action_key(action: &NodeRoutingDefaultAction) -> &'static str {
    match action {
        NodeRoutingDefaultAction::Direct => "routing.action.direct",
        NodeRoutingDefaultAction::Network => "routing.action.network",
        NodeRoutingDefaultAction::Exit(_) => "routing.action.exit",
        NodeRoutingDefaultAction::Reject => "routing.action.reject",
    }
}

fn routing_target_label(target: &Option<NodeRoutingTargetSnapshot>, locale: Locale) -> String {
    match target {
        Some(NodeRoutingTargetSnapshot::Network { network_name }) => tr(
            locale,
            "routing.target.network",
            &[("network", network_name)],
        ),
        Some(NodeRoutingTargetSnapshot::Exit { node_id }) => {
            tr(locale, "routing.target.exit", &[("node", node_id)])
        }
        None => t(locale, "common.none").to_string(),
    }
}

fn routing_timestamp_label(unix_ms: Option<u64>, locale: Locale) -> String {
    unix_ms
        .and_then(|timestamp| {
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(timestamp as i64)
        })
        .map(|timestamp| timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| t(locale, "common.not_available").to_string())
}

fn project_node_runtime(
    snapshot: &NodeSnapshot,
    locale: Locale,
) -> (Vec<MetricModel>, Vec<SectionModel>) {
    let primary_status = snapshot.primary_status();
    let phase = t(locale, node_phase_key(snapshot.lifecycle.phase));
    let control_health = t(locale, control_health_key(snapshot.control_health));
    let failed_networks = if snapshot.lifecycle.failed_networks.is_empty() {
        t(locale, "common.none").to_string()
    } else {
        snapshot
            .lifecycle
            .failed_networks
            .join(t(locale, "common.list_separator"))
    };
    let last_error = snapshot
        .last_error
        .as_ref()
        .map(|problem| {
            let localized = tr(locale, &problem.message_key, &[]);
            if localized == problem.message_key {
                t(locale, "common.unknown").to_string()
            } else {
                localized
            }
        })
        .unwrap_or_else(|| t(locale, "common.none").to_string());
    let network_rows = snapshot
        .networks
        .iter()
        .map(|network| {
            let roles = if network.roles.is_empty() {
                t(locale, "common.none").to_string()
            } else {
                network
                    .roles
                    .iter()
                    .map(|role| t(locale, role_key(*role)).to_string())
                    .collect::<Vec<_>>()
                    .join(t(locale, "common.list_separator"))
            };
            row(
                network.name.clone(),
                tr(
                    locale,
                    "runtime.row.network_detail",
                    &[
                        ("status", t(locale, network_status_key(network.status))),
                        ("peers", &network.peer_count.to_string()),
                        ("roles", &roles),
                    ],
                ),
            )
        })
        .collect::<Vec<_>>();
    let interface_rows = snapshot
        .interfaces
        .iter()
        .map(|interface| {
            let addresses = if interface.addresses.is_empty() {
                t(locale, "common.none").to_string()
            } else {
                interface
                    .addresses
                    .iter()
                    .map(|address| format!("{}/{}", address.address, address.prefix_len))
                    .collect::<Vec<_>>()
                    .join(t(locale, "common.list_separator"))
            };
            let defaults = tr(
                locale,
                "runtime.row.interface_defaults",
                &[
                    (
                        "ipv4",
                        if interface.default_ipv4 {
                            t(locale, "common.enabled")
                        } else {
                            t(locale, "common.disabled")
                        },
                    ),
                    (
                        "ipv6",
                        if interface.default_ipv6 {
                            t(locale, "common.enabled")
                        } else {
                            t(locale, "common.disabled")
                        },
                    ),
                ],
            );
            row(
                interface.name.clone(),
                format!(
                    "{} · {}",
                    tr(
                        locale,
                        "runtime.row.interface_detail",
                        &[
                            ("kind", t(locale, interface_kind_key(interface.kind))),
                            (
                                "ownership",
                                t(locale, interface_ownership_key(interface.ownership)),
                            ),
                            ("status", t(locale, interface_status_key(interface.status))),
                            ("mtu", &interface.mtu.to_string()),
                            ("addresses", &addresses),
                        ],
                    ),
                    defaults,
                ),
            )
        })
        .collect::<Vec<_>>();
    let interface_problem_rows = snapshot
        .interface_problems
        .iter()
        .map(|problem| {
            let params = problem
                .params
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect::<Vec<_>>();
            let localized = tr(locale, &problem.message_key, &params);
            let detail = if localized == problem.message_key {
                problem.message.clone()
            } else {
                localized
            };
            row(
                problem
                    .network_name
                    .clone()
                    .unwrap_or_else(|| t(locale, "runtime.interface.host").to_string()),
                detail,
            )
        })
        .collect::<Vec<_>>();
    let routing_summary_rows = snapshot
        .routing_summaries
        .iter()
        .map(|summary| {
            let revision = summary
                .last_applied_revision
                .map(|revision| revision.to_string())
                .unwrap_or_else(|| t(locale, "common.not_available").to_string());
            let applied = routing_timestamp_label(summary.last_applied_at_unix_ms, locale);
            let target = routing_target_label(&summary.effective_target, locale);
            let mut detail = tr(
                locale,
                "runtime.routing_summary_detail",
                &[
                    ("status", t(locale, routing_status_key(summary.status))),
                    (
                        "action",
                        t(locale, routing_default_action_key(&summary.default_action)),
                    ),
                    ("target", &target),
                    ("rules", &summary.enabled_rule_count.to_string()),
                    ("revision", &revision),
                    ("applied", &applied),
                ],
            );
            if summary.status != NodeRoutingSummaryStatus::Current {
                if let Some(problem) = &summary.problem {
                    let params = problem
                        .params
                        .iter()
                        .map(|(name, value)| (name.as_str(), value.as_str()))
                        .collect::<Vec<_>>();
                    let localized = tr(locale, &problem.message_key, &params);
                    let reason = if localized == problem.message_key {
                        problem.message.clone()
                    } else {
                        localized
                    };
                    detail.push_str(" · ");
                    detail.push_str(&reason);
                }
            }
            row_with_navigation(
                summary.network_name.clone(),
                detail,
                "routing",
                summary.network_name.clone(),
                t(locale, "routing.manage"),
            )
        })
        .collect::<Vec<_>>();
    let service_rows = snapshot
        .embedded_services
        .iter()
        .map(|service| {
            let signaling = service
                .signaling_addr
                .map(|addr| addr.to_string())
                .unwrap_or_else(|| t(locale, "common.not_available").to_string());
            let relay = service
                .relay_addr
                .map(|addr| addr.to_string())
                .unwrap_or_else(|| t(locale, "common.not_available").to_string());
            row(
                service.network_name.clone(),
                tr(
                    locale,
                    "runtime.row.service_detail",
                    &[
                        (
                            "status",
                            t(locale, embedded_service_status_key(service.status)),
                        ),
                        ("signaling", &signaling),
                        ("relay", &relay),
                    ],
                ),
            )
        })
        .collect::<Vec<_>>();
    let tun_capabilities = &snapshot.tun_capabilities;
    let tun_reasons = if tun_capabilities.reasons.is_empty() {
        t(locale, "runtime.value.tun_reason.unknown").to_string()
    } else {
        tun_capabilities
            .reasons
            .iter()
            .map(|reason| t(locale, tun_reason_key(*reason)).to_string())
            .collect::<Vec<_>>()
            .join(t(locale, "common.list_separator"))
    };

    (
        vec![
            metric(
                t(locale, "runtime.metric.status"),
                t(locale, primary_status.label_key()),
                "NodeSnapshot.primary_status",
            ),
            metric(
                t(locale, "runtime.metric.phase"),
                phase,
                "NodeSnapshot.lifecycle.phase",
            ),
            metric(
                t(locale, "runtime.metric.revision"),
                snapshot.revision,
                "NodeSnapshot.revision",
            ),
            metric(
                t(locale, "runtime.metric.active_networks"),
                snapshot.lifecycle.active_networks,
                "NodeSnapshot.lifecycle.active_networks",
            ),
            metric(
                t(locale, "runtime.metric.configured_networks"),
                snapshot.lifecycle.configured_networks,
                "NodeSnapshot.lifecycle.configured_networks",
            ),
        ],
        {
            let mut sections = vec![
                section(
                    t(locale, "runtime.section.state"),
                    t(locale, "common.no_data"),
                    vec![
                        row(
                            t(locale, "runtime.row.primary_status"),
                            t(locale, primary_status.label_key()),
                        ),
                        row(t(locale, "runtime.row.phase"), phase),
                        row(t(locale, "runtime.row.control_health"), control_health),
                        row(
                            t(locale, "runtime.row.revision"),
                            snapshot.revision.to_string(),
                        ),
                        row(t(locale, "runtime.row.failed_networks"), failed_networks),
                        row(t(locale, "runtime.row.last_error"), last_error),
                    ],
                ),
                section(
                    t(locale, "runtime.section.networks"),
                    t(locale, "runtime.empty.networks"),
                    network_rows,
                ),
                section(
                    t(locale, "runtime.section.tun"),
                    t(locale, "common.no_data"),
                    vec![
                        row(
                            t(locale, "runtime.row.tun_platform"),
                            t(locale, tun_platform_key(tun_capabilities.platform)),
                        ),
                        row(
                            t(locale, "runtime.row.tun_supported"),
                            t(
                                locale,
                                if tun_capabilities.supported {
                                    "common.enabled"
                                } else {
                                    "common.disabled"
                                },
                            ),
                        ),
                        row(
                            t(locale, "runtime.row.tun_rebuildable"),
                            t(
                                locale,
                                if tun_capabilities.can_rebuild {
                                    "common.enabled"
                                } else {
                                    "common.disabled"
                                },
                            ),
                        ),
                        row(t(locale, "runtime.row.tun_reason"), tun_reasons),
                    ],
                ),
                section(
                    t(locale, "runtime.section.interfaces"),
                    t(locale, "runtime.empty.interfaces"),
                    interface_rows,
                ),
            ];
            if !interface_problem_rows.is_empty() {
                sections.push(section(
                    t(locale, "runtime.section.interface_problems"),
                    t(locale, "runtime.empty.interface_problems"),
                    interface_problem_rows,
                ));
            }
            sections.push(section(
                t(locale, "runtime.section.routing"),
                t(locale, "runtime.empty.routing"),
                routing_summary_rows,
            ));
            sections.push(section(
                t(locale, "runtime.section.services"),
                t(locale, "runtime.empty.services"),
                service_rows,
            ));
            sections
        },
    )
}

fn project_topology(
    snapshot: &NodeSnapshot,
    locale: Locale,
) -> (Vec<MetricModel>, Vec<SectionModel>) {
    // Topology is a server-owned projection. Adapters must never reconstruct
    // graph objects from the legacy network and peer lists, otherwise a stale
    // peer session can become a fake topology node or connection.
    let Some(topology) = snapshot.topology.as_ref() else {
        return (
            vec![metric(
                t(locale, "gpui.metric.topology_edges"),
                0,
                "TopologySnapshot",
            )],
            vec![section(
                t(locale, "gpui.section.topology"),
                t(locale, "topology.empty.unavailable"),
                Vec::new(),
            )],
        );
    };

    let sections = topology
        .boundaries
        .iter()
        .map(|boundary| {
            let rows = topology
                .nodes
                .iter()
                .filter(|node| node.belongs_to(&boundary.network_id))
                .map(|node| {
                    let name = node
                        .display_name
                        .as_deref()
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .unwrap_or(node.node_id.as_str());
                    let membership = node
                        .networks
                        .iter()
                        .find(|network| network.network_id == boundary.network_id);
                    let relationship = membership
                        .map(|membership| {
                            format!(
                                "{} · {} · {}",
                                t(locale, topology_health_key(node.health)),
                                t(locale, topology_role_key(membership.role)),
                                t(locale, topology_member_status_key(membership.status)),
                            )
                        })
                        .unwrap_or_else(|| t(locale, "common.not_available").to_string());
                    row(name, format!("{} · {relationship}", node.node_id))
                })
                .collect::<Vec<_>>();
            section(
                tr(
                    locale,
                    "topology.boundary.members",
                    &[("count", &boundary.member_count.to_string())],
                ),
                t(locale, "topology.state.empty.title"),
                rows,
            )
        })
        .collect::<Vec<_>>();
    (
        vec![metric(
            t(locale, "gpui.metric.topology_edges"),
            topology.active_connections().len(),
            "TopologySnapshot",
        )],
        if sections.is_empty() {
            vec![section(
                t(locale, "gpui.section.topology"),
                t(locale, "topology.empty.no_networks"),
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

fn activity_outcome_key(outcome: AuditOutcome) -> &'static str {
    match outcome {
        AuditOutcome::Succeeded => "activity.outcome.succeeded",
        AuditOutcome::Failed => "activity.outcome.failed",
        AuditOutcome::Denied => "activity.outcome.denied",
    }
}

fn activity_category_key(category: ActivityCategory) -> &'static str {
    match category {
        ActivityCategory::UserActivity => "activity.category.user_activity",
        ActivityCategory::SystemEvent => "activity.category.system_event",
        ActivityCategory::SecurityEvent => "activity.category.security_event",
        ActivityCategory::OperationFailure => "activity.category.operation_failure",
    }
}

fn activity_severity_key(severity: ActivitySeverity) -> &'static str {
    match severity {
        ActivitySeverity::Info => "activity.severity.info",
        ActivitySeverity::Notice => "activity.severity.notice",
        ActivitySeverity::Warning => "activity.severity.warning",
        ActivitySeverity::Critical => "activity.severity.critical",
    }
}

fn activity_object_kind_label(locale: Locale, kind: &str) -> String {
    let key = format!("activity.object_kind.{kind}");
    let localized = t(locale, &key);
    if localized == key {
        kind.to_string()
    } else {
        localized.to_string()
    }
}

fn activity_role_label(locale: Locale, role: &str) -> String {
    let key = format!("activity.role.{role}");
    let localized = t(locale, &key);
    if localized == key {
        role.to_string()
    } else {
        localized.to_string()
    }
}

fn activity_reason_label(locale: Locale, reason: &str) -> String {
    let key = format!("activity.reason.{reason}");
    let localized = t(locale, &key);
    if localized == key {
        reason.to_string()
    } else {
        localized.to_string()
    }
}

fn activity_network_name(snapshot: &NodeSnapshot, network_id: &str) -> Option<String> {
    snapshot
        .memberships
        .iter()
        .find(|membership| membership.network_id == network_id)
        .map(|membership| membership.network_name.clone())
        .or_else(|| {
            snapshot
                .network_profiles
                .iter()
                .find(|profile| profile.network_id == network_id)
                .map(|profile| profile.name.clone())
        })
}

/// Object links are generated only from the current authoritative snapshot.
/// Historical records never grant access to an object that has disappeared or
/// is no longer visible under the viewer's current permissions.
fn activity_object_navigation(
    snapshot: &NodeSnapshot,
    entry: &ActivityEntry,
    locale: Locale,
) -> Option<RowNavigation> {
    if !snapshot
        .permissions
        .permissions
        .contains(&NodePermission::ViewState)
    {
        return None;
    }
    let object_id = entry.object_id.as_deref()?;
    let label = t(locale, "activity.link.open_object").to_string();
    match entry.object_kind.as_str() {
        "network" => {
            let network_name = activity_network_name(snapshot, object_id)?;
            Some(RowNavigation {
                page_id: "topology".into(),
                network_name: Some(network_name),
                object_id: Some(object_id.to_owned()),
                label,
            })
        }
        "member" | "topology_node" => {
            let node = snapshot
                .topology
                .as_ref()?
                .nodes
                .iter()
                .find(|node| node.node_id.as_str() == object_id)?;
            if let Some(network_id) = entry.network_id.as_ref().map(NetworkId::as_str) {
                if !node
                    .networks
                    .iter()
                    .any(|membership| membership.network_id.as_str() == network_id)
                {
                    return None;
                }
            }
            let network_name = entry
                .network_id
                .as_ref()
                .map(NetworkId::as_str)
                .and_then(|network_id| activity_network_name(snapshot, network_id))
                .or_else(|| {
                    node.networks.iter().find_map(|membership| {
                        activity_network_name(snapshot, membership.network_id.as_str())
                    })
                });
            Some(RowNavigation {
                page_id: "topology".into(),
                network_name,
                object_id: Some(object_id.to_owned()),
                label,
            })
        }
        "node" => {
            let identity = snapshot.identity.as_ref()?;
            (identity.node_id == object_id).then(|| RowNavigation {
                page_id: "identity".into(),
                network_name: None,
                object_id: Some(object_id.to_owned()),
                label,
            })
        }
        // Invitations, join requests, configuration and deleted objects do not
        // have stable inspectable destinations in the first activity surface.
        _ => None,
    }
}

fn project_activity(
    snapshot: &NodeSnapshot,
    locale: Locale,
    filters: &ActivityFilters,
) -> (Vec<MetricModel>, Vec<SectionModel>) {
    let all = ordered_activity_entries(&snapshot.recent_activity);
    let security = all
        .iter()
        .filter(|entry| entry.category == ActivityCategory::SecurityEvent)
        .count();
    let failures = all
        .iter()
        .filter(|entry| entry.category == ActivityCategory::OperationFailure)
        .count();
    let system = all
        .iter()
        .filter(|entry| entry.category == ActivityCategory::SystemEvent)
        .count();
    let rows = filter_activity(&snapshot.recent_activity, filters)
        .into_iter()
        .map(|entry| {
            let action_key = format!("activity.action.{}", entry.action);
            let action = t(locale, &action_key);
            let action = if action == action_key {
                t(locale, "activity.action.node.command").to_string()
            } else {
                action.to_string()
            };
            let object_kind = activity_object_kind_label(locale, &entry.object_kind);
            let object = match entry.object_id.as_deref() {
                Some(object_id) => tr(
                    locale,
                    "activity.object_value",
                    &[("kind", &object_kind), ("id", object_id)],
                ),
                None => object_kind,
            };
            let mut details = vec![
                t(locale, activity_category_key(entry.category)).to_string(),
                t(locale, activity_severity_key(entry.severity)).to_string(),
                t(locale, activity_outcome_key(entry.outcome)).to_string(),
            ];
            if let Some(role) = entry.actor_role.as_deref() {
                details.push(tr(
                    locale,
                    "activity.actor_role_value",
                    &[("role", &activity_role_label(locale, role))],
                ));
            }
            details.push(object);
            if let Some(reason) = entry.reason.as_deref() {
                details.push(tr(
                    locale,
                    "activity.reason_value",
                    &[("reason", &activity_reason_label(locale, reason))],
                ));
            }
            details.push(routing_timestamp_label(
                Some(entry.occurred_at_unix_ms),
                locale,
            ));
            details.push(tr(
                locale,
                "activity.audit_id_value",
                &[("audit", entry.audit_id.as_str())],
            ));
            RowModel {
                title: format!("{} · {action}", entry.actor),
                detail: details.join(" · "),
                navigation: activity_object_navigation(snapshot, entry, locale),
            }
        })
        .collect::<Vec<_>>();
    let result_count = tr(
        locale,
        "activity.result_count",
        &[
            ("filtered", &rows.len().to_string()),
            ("total", &all.len().to_string()),
        ],
    );
    let empty_text = if all.is_empty() {
        t(locale, "gpui.empty.activity")
    } else if rows.is_empty() {
        t(locale, "activity.empty.no_matches")
    } else {
        t(locale, "gpui.empty.activity")
    };
    (
        vec![
            metric(
                t(locale, "gpui.metric.activity_total"),
                all.len(),
                "NodeSnapshot.recent_activity",
            ),
            metric(
                t(locale, "gpui.metric.activity_security"),
                security,
                "ActivityCategory",
            ),
            metric(
                t(locale, "gpui.metric.activity_failures"),
                failures,
                "ActivityCategory",
            ),
            metric(
                t(locale, "gpui.metric.activity_system"),
                system,
                "ActivityCategory",
            ),
        ],
        vec![section(
            format!(
                "{} · {result_count}",
                t(locale, "gpui.section.activity_timeline")
            ),
            empty_text,
            rows,
        )],
    )
}

fn routing_workflow_state_key(workflow: &TrafficRoutingWorkflowState) -> &'static str {
    match workflow {
        TrafficRoutingWorkflowState::Unconfigured { .. } => "traffic.routing.state.unconfigured",
        TrafficRoutingWorkflowState::Draft { .. } => "traffic.routing.state.draft",
        TrafficRoutingWorkflowState::Applying { .. } => "traffic.routing.state.applying",
        TrafficRoutingWorkflowState::Applied { .. } => "traffic.routing.state.applied",
        TrafficRoutingWorkflowState::Failed { .. } => "traffic.routing.state.failed",
        TrafficRoutingWorkflowState::Stale { .. } => "traffic.routing.state.stale",
    }
}

fn routing_workflow_detail(workflow: &TrafficRoutingWorkflowState, locale: Locale) -> String {
    let status = t(locale, routing_workflow_state_key(workflow));
    match workflow {
        TrafficRoutingWorkflowState::Unconfigured { reason_code } => {
            format!("{status} · reason={reason_code}")
        }
        TrafficRoutingWorkflowState::Draft { base_revision } => {
            format!("{status} · base_revision={base_revision}")
        }
        TrafficRoutingWorkflowState::Applying {
            base_revision,
            candidate_revision,
            operation_id,
        } => format!(
            "{status} · base_revision={base_revision} · candidate_revision={candidate_revision} · operation_id={operation_id}"
        ),
        TrafficRoutingWorkflowState::Applied { revision } => {
            format!("{status} · revision={revision}")
        }
        TrafficRoutingWorkflowState::Failed {
            base_revision,
            candidate_revision,
            stage,
            reason_code,
        } => format!(
            "{status} · base_revision={base_revision} · candidate_revision={candidate_revision} · stage={stage} · reason={reason_code}"
        ),
        TrafficRoutingWorkflowState::Stale {
            revision,
            reason_code,
        } => format!("{status} · revision={revision} · reason={reason_code}"),
    }
}

fn routing_target_kind_key(kind: TrafficRoutingTargetKind) -> &'static str {
    match kind {
        TrafficRoutingTargetKind::Direct => "gpui.routing.target_kind.direct",
        TrafficRoutingTargetKind::Network => "gpui.routing.target_kind.network",
        TrafficRoutingTargetKind::Exit => "gpui.routing.target_kind.exit",
        TrafficRoutingTargetKind::Reject => "gpui.routing.target_kind.reject",
    }
}

fn routing_target_kind_value(kind: TrafficRoutingTargetKind) -> &'static str {
    match kind {
        TrafficRoutingTargetKind::Direct => "direct",
        TrafficRoutingTargetKind::Network => "network",
        TrafficRoutingTargetKind::Exit => "exit",
        TrafficRoutingTargetKind::Reject => "reject",
    }
}

fn routing_action_kind_and_id(action: &TrafficAction) -> (TrafficRoutingTargetKind, String) {
    match action {
        TrafficAction::Direct => (TrafficRoutingTargetKind::Direct, "direct".into()),
        TrafficAction::Network(network_id) => (
            TrafficRoutingTargetKind::Network,
            network_id.as_str().to_owned(),
        ),
        TrafficAction::Exit(node_id) => {
            (TrafficRoutingTargetKind::Exit, node_id.as_str().to_owned())
        }
        TrafficAction::Reject => (TrafficRoutingTargetKind::Reject, "reject".into()),
    }
}

fn routing_target_ref_label(
    page: &TrafficRoutingPageModel,
    target: &TrafficRoutingTargetRef,
    locale: Locale,
) -> String {
    let label = page
        .target(target.kind, &target.id)
        .map(|target| target.label.clone())
        .unwrap_or_else(|| format!("{}:{}", routing_target_kind_value(target.kind), target.id));
    if target.resolved {
        label
    } else {
        format!(
            "{label} · {}",
            t(locale, "traffic.routing.target.unresolved")
        )
    }
}

fn routing_action_label(
    page: &TrafficRoutingPageModel,
    action: &TrafficAction,
    locale: Locale,
) -> String {
    let (kind, id) = routing_action_kind_and_id(action);
    let target = TrafficRoutingTargetRef {
        kind,
        id: id.clone(),
        resolved: page.target(kind, &id).is_some(),
    };
    routing_target_ref_label(page, &target, locale)
}

fn routing_target_navigation(
    target: &TrafficRoutingTarget,
    locale: Locale,
) -> Option<RowNavigation> {
    let navigation = target.navigation.as_ref()?;
    Some(RowNavigation {
        page_id: navigation.page_id.clone(),
        network_name: navigation.network_name.clone(),
        object_id: Some(navigation.object_id.clone()),
        label: t(locale, "traffic.routing.targets.jump").to_string(),
    })
}

fn routing_target_row(target: &TrafficRoutingTarget, locale: Locale) -> RowModel {
    let mut detail = tr(
        locale,
        "gpui.routing.target_detail",
        &[
            ("kind", t(locale, routing_target_kind_key(target.kind))),
            ("id", &target.id),
        ],
    );
    let navigation = routing_target_navigation(target, locale);
    if navigation.is_none() {
        detail.push_str(" · ");
        detail.push_str(t(locale, "traffic.routing.targets.no_navigation"));
    }
    RowModel {
        title: target.label.clone(),
        detail,
        navigation,
    }
}

pub(crate) fn routing_target_search_rows(
    snapshot: &NodeSnapshot,
    query: &str,
    locale: Locale,
) -> Vec<RowModel> {
    snapshot
        .traffic_routing_page
        .as_ref()
        .map(|page| {
            page.search_targets(query)
                .into_iter()
                .map(|target| routing_target_row(target, locale))
                .collect()
        })
        .unwrap_or_default()
}

fn routing_list(values: Vec<String>, locale: Locale) -> String {
    if values.is_empty() {
        t(locale, "common.none").to_string()
    } else {
        values.join(t(locale, "common.list_separator"))
    }
}

fn routing_protocol_label(protocol: TrafficProtocol) -> &'static str {
    match protocol {
        TrafficProtocol::Tcp => "TCP",
        TrafficProtocol::Udp => "UDP",
        TrafficProtocol::Icmp => "ICMP",
    }
}

fn routing_application_label(application: &ApplicationIdentity) -> String {
    match application {
        ApplicationIdentity::ExecutablePath(path) => format!("executable_path:{path}"),
        ApplicationIdentity::PackageId(package) => format!("package_id:{package}"),
        ApplicationIdentity::Unknown => "unknown".into(),
    }
}

fn routing_match_detail(matcher: &TrafficMatch, locale: Locale) -> String {
    let exact_domains = routing_list(
        matcher
            .exact_domains
            .iter()
            .map(|domain| domain.as_str().to_owned())
            .collect(),
        locale,
    );
    let domain_suffixes = routing_list(
        matcher
            .domain_suffixes
            .iter()
            .map(|suffix| suffix.as_str().to_owned())
            .collect(),
        locale,
    );
    let destination_cidrs = routing_list(
        matcher
            .destination_cidrs
            .iter()
            .map(|cidr| cidr.as_str().to_owned())
            .collect(),
        locale,
    );
    let destination_ports = routing_list(
        matcher
            .destination_ports
            .iter()
            .map(|range| {
                if range.start == range.end {
                    range.start.to_string()
                } else {
                    format!("{}-{}", range.start, range.end)
                }
            })
            .collect(),
        locale,
    );
    let protocols = routing_list(
        matcher
            .protocols
            .iter()
            .map(|protocol| routing_protocol_label(*protocol).to_owned())
            .collect(),
        locale,
    );
    let applications = routing_list(
        matcher
            .applications
            .iter()
            .map(routing_application_label)
            .collect(),
        locale,
    );
    format!(
        "{}: {exact_domains} · {}: {domain_suffixes} · {}: {destination_cidrs} · {}: {destination_ports} · {}: {protocols} · {}: {applications}",
        t(locale, "traffic.routing.match.exact_domains"),
        t(locale, "traffic.routing.match.domain_suffixes"),
        t(locale, "traffic.routing.match.destination_cidrs"),
        t(locale, "traffic.routing.match.destination_ports"),
        t(locale, "traffic.routing.match.protocols"),
        t(locale, "traffic.routing.match.applications"),
    )
}

fn routing_risk_label(risk: &TrafficMatchRisk, locale: Locale) -> String {
    match risk {
        TrafficMatchRisk::AlwaysMatches => {
            t(locale, "gpui.routing.risk.always_matches").to_string()
        }
        TrafficMatchRisk::AnyDestination => {
            t(locale, "gpui.routing.risk.any_destination").to_string()
        }
        TrafficMatchRisk::AnyPort => t(locale, "gpui.routing.risk.any_port").to_string(),
        TrafficMatchRisk::AnyProtocol => t(locale, "gpui.routing.risk.any_protocol").to_string(),
        TrafficMatchRisk::AnyApplication => {
            t(locale, "gpui.routing.risk.any_application").to_string()
        }
        TrafficMatchRisk::BroadCidr { cidr, prefix_len } => tr(
            locale,
            "gpui.routing.risk.broad_cidr",
            &[("cidr", cidr), ("prefix_len", &prefix_len.to_string())],
        ),
        TrafficMatchRisk::FullPortRange { start, end } => tr(
            locale,
            "gpui.routing.risk.full_port_range",
            &[("start", &start.to_string()), ("end", &end.to_string())],
        ),
    }
}

fn routing_flow_detail(flow: &TrafficFlow) -> String {
    let destination = flow
        .domain
        .as_ref()
        .map(|domain| domain.as_str().to_owned())
        .unwrap_or_else(|| flow.destination_ip.to_string());
    format!(
        "{destination}:{} · {} · {}",
        flow.destination_port,
        routing_protocol_label(flow.protocol),
        routing_application_label(&flow.application)
    )
}

fn routing_hit_sample_detail(
    page: &TrafficRoutingPageModel,
    sample: &TrafficRoutingHitSample,
    locale: Locale,
) -> String {
    let activity = sample
        .activity_id
        .as_deref()
        .unwrap_or_else(|| t(locale, "common.not_available"));
    let diagnostic = sample
        .diagnostic_id
        .as_deref()
        .unwrap_or_else(|| t(locale, "common.not_available"));
    let rule = sample
        .after_rule_id
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_else(|| t(locale, "common.none").to_string());
    format!(
        "{} {activity} · {} {diagnostic} · {} {} · {} {} → {} · {} {rule}",
        t(locale, "traffic.routing.rule.impact_activity"),
        t(locale, "traffic.routing.rule.impact_diagnostic"),
        t(locale, "gpui.routing.flow"),
        routing_flow_detail(&sample.flow),
        t(locale, "traffic.routing.rule.impact_before"),
        routing_action_label(page, &sample.before_action, locale),
        routing_action_label(page, &sample.after_action, locale),
        t(locale, "traffic.routing.rule.impact_after"),
    )
}

fn routing_impact_detail(
    page: &TrafficRoutingPageModel,
    impact: &TrafficRoutingRuleImpactView,
    locale: Locale,
) -> String {
    match impact {
        TrafficRoutingRuleImpactView::Available { impact } => {
            let effect = match &impact.activity_effect {
                TrafficRoutingRuleActivityEffect::ObservedTrafficReassigned { sample_count } => tr(
                    locale,
                    "traffic.routing.rule.impact_reassigned",
                    &[("count", &sample_count.to_string())],
                ),
                TrafficRoutingRuleActivityEffect::NoObservedTraffic => {
                    t(locale, "traffic.routing.rule.impact_no_hits").to_string()
                }
            };
            let samples = impact
                .hit_samples
                .iter()
                .map(|sample| routing_hit_sample_detail(page, sample, locale))
                .collect::<Vec<_>>();
            if samples.is_empty() {
                effect
            } else {
                format!(
                    "{effect} · {}: {}",
                    t(locale, "gpui.routing.hit_samples"),
                    samples.join(t(locale, "common.list_separator"))
                )
            }
        }
        TrafficRoutingRuleImpactView::Unavailable { reason_code } => format!(
            "{} · reason={reason_code}",
            t(locale, "traffic.routing.rule.impact_unavailable")
        ),
    }
}

fn project_routing(
    snapshot: &NodeSnapshot,
    locale: Locale,
) -> (Vec<MetricModel>, Vec<SectionModel>) {
    let Some(page) = snapshot.traffic_routing_page.as_ref() else {
        return (
            Vec::new(),
            vec![section(
                t(locale, "traffic.routing.authority.unavailable"),
                t(locale, "traffic.routing.authority.unavailable_detail"),
                vec![row(
                    t(locale, "traffic.routing.authority.unavailable"),
                    t(locale, "traffic.routing.authority.unavailable_detail"),
                )],
            )],
        );
    };

    let mut authority_rows = vec![row(
        t(locale, "gpui.routing.workflow"),
        routing_workflow_detail(&page.workflow, locale),
    )];

    let confirmed_by = match page.default_policy.confirmed_by {
        TrafficDefaultConfirmedBy::Owner => t(locale, "join_request.role.owner"),
        TrafficDefaultConfirmedBy::Administrator => t(locale, "join_request.role.administrator"),
    };
    let confirmed_at =
        routing_timestamp_label(Some(page.default_policy.confirmed_at_unix_ms), locale);
    authority_rows.push(row(
        t(locale, "traffic.routing.default_policy"),
        format!(
            "{}: {} · {}: {confirmed_by} · confirmed_at: {confirmed_at}",
            t(locale, "traffic.routing.action"),
            routing_action_label(page, &page.default_policy.action, locale),
            t(locale, "traffic.routing.confirmed_by"),
        ),
    ));

    let failure_policy = page
        .failure_policy
        .direct_fallback
        .as_ref()
        .map(|fallback| {
            tr(
                locale,
                "traffic.routing.direct_fallback",
                &[(
                    "failures",
                    &fallback
                        .allowed_failures
                        .iter()
                        .map(|failure| failure.as_str())
                        .collect::<Vec<_>>()
                        .join(t(locale, "common.list_separator")),
                )],
            )
        })
        .unwrap_or_else(|| t(locale, "traffic.routing.fail_closed").to_string());
    authority_rows.push(row(
        t(locale, "traffic.routing.failure_policy"),
        failure_policy,
    ));

    let hard_constraints = page
        .hard_constraint_target
        .as_ref()
        .map(|target| {
            format!(
                "{} · {}",
                routing_target_ref_label(page, target, locale),
                t(locale, "traffic.routing.hard_constraint_note"),
            )
        })
        .unwrap_or_else(|| t(locale, "traffic.routing.no_hard_constraint").to_string());
    authority_rows.push(row(
        t(locale, "traffic.routing.hard_constraints"),
        hard_constraints,
    ));
    if !page.can_edit {
        authority_rows.push(row(
            t(locale, "traffic.routing.read_only"),
            t(locale, "traffic.routing.read_only"),
        ));
    }

    let rule_rows = page
        .rules
        .iter()
        .map(|rule| {
            let risks = if rule.risks.is_empty() {
                t(locale, "common.none").to_string()
            } else {
                rule.risks
                    .iter()
                    .map(|risk| routing_risk_label(risk, locale))
                    .collect::<Vec<_>>()
                    .join(t(locale, "common.list_separator"))
            };
            let title = if rule.description.trim().is_empty() {
                format!("#{} · {}", rule.priority, rule.rule_id)
            } else {
                format!("#{} · {}", rule.priority, rule.description)
            };
            let detail = format!(
                "{}: {} · {}: {} · {}: {} · {}: {risks}",
                t(locale, "traffic.routing.rule.enabled"),
                display_bool(locale, rule.enabled),
                t(locale, "traffic.routing.rule.action"),
                routing_target_ref_label(page, &rule.target, locale),
                t(locale, "traffic.routing.match.exact_domains"),
                routing_match_detail(&rule.matcher, locale),
                t(locale, "gpui.routing.rule_risks"),
            );
            let mut item = row(title, detail);
            let (kind, id) = routing_action_kind_and_id(&rule.action);
            item.navigation = page
                .target(kind, &id)
                .and_then(|target| routing_target_navigation(target, locale));
            item
        })
        .collect::<Vec<_>>();

    let impact_rows = page
        .rules
        .iter()
        .flat_map(|rule| {
            [
                row(
                    format!(
                        "{} · {}",
                        rule.rule_id,
                        t(locale, "policy.choice.action.disable")
                    ),
                    routing_impact_detail(page, &rule.disable_impact, locale),
                ),
                row(
                    format!(
                        "{} · {}",
                        rule.rule_id,
                        t(locale, "policy.choice.action.delete")
                    ),
                    routing_impact_detail(page, &rule.delete_impact, locale),
                ),
            ]
        })
        .collect::<Vec<_>>();

    let observation_rows = match &page.observation {
        TrafficRoutingObservation::Available {
            window_start_unix_ms,
            window_end_unix_ms,
            observed_flow_count,
            hits,
        } => {
            let mut rows = vec![row(
                tr(
                    locale,
                    "gpui.routing.observed_flows",
                    &[("count", &observed_flow_count.to_string())],
                ),
                tr(
                    locale,
                    "traffic.routing.rule.impact_window",
                    &[
                        (
                            "start",
                            &routing_timestamp_label(Some(*window_start_unix_ms), locale),
                        ),
                        (
                            "end",
                            &routing_timestamp_label(Some(*window_end_unix_ms), locale),
                        ),
                    ],
                ),
            )];
            if hits.is_empty() {
                rows.push(row(
                    t(locale, "gpui.routing.no_hits"),
                    t(locale, "traffic.routing.rule.impact_no_hits"),
                ));
            } else {
                rows.extend(hits.iter().map(|hit| {
                    let activity = hit
                        .activity_id
                        .as_deref()
                        .unwrap_or_else(|| t(locale, "common.not_available"));
                    let diagnostic = hit
                        .diagnostic_id
                        .as_deref()
                        .unwrap_or_else(|| t(locale, "common.not_available"));
                    row(
                        hit.sample_id.clone(),
                        format!(
                            "{} {activity} · {} {diagnostic} · {} {}",
                            t(locale, "traffic.routing.rule.impact_activity"),
                            t(locale, "traffic.routing.rule.impact_diagnostic"),
                            t(locale, "gpui.routing.flow"),
                            routing_flow_detail(&hit.flow),
                        ),
                    )
                }));
            }
            rows
        }
        TrafficRoutingObservation::Unavailable { reason_code } => vec![row(
            t(locale, "gpui.routing.observation_unavailable"),
            format!("reason={reason_code}"),
        )],
    };

    let target_rows = page
        .targets
        .iter()
        .map(|target| routing_target_row(target, locale))
        .collect::<Vec<_>>();

    (
        vec![
            metric(
                t(locale, "traffic.routing.authority_revision"),
                page.authority_revision,
                "TrafficRoutingPageModel.authority_revision",
            ),
            metric(
                t(locale, "traffic.routing.rules.title"),
                page.rules.len(),
                "TrafficRoutingPageModel.rules",
            ),
            metric(
                t(locale, "traffic.routing.targets"),
                page.targets.len(),
                "TrafficRoutingPageModel.targets",
            ),
        ],
        vec![
            section(
                t(locale, "traffic.routing.eyebrow"),
                t(locale, "traffic.routing.authority.unavailable_detail"),
                authority_rows,
            ),
            section(
                t(locale, "traffic.routing.rules.title"),
                t(locale, "traffic.routing.rules.empty"),
                rule_rows,
            ),
            section(
                t(locale, "traffic.routing.rule.impact_title"),
                t(locale, "traffic.routing.rules.empty"),
                impact_rows,
            ),
            section(
                t(locale, "gpui.routing.observation"),
                t(locale, "gpui.routing.no_hits"),
                observation_rows,
            ),
            section(
                t(locale, "traffic.routing.targets"),
                t(locale, "traffic.routing.targets.no_results"),
                target_rows,
            ),
        ],
    )
}

fn exit_selection_mode_label(locale: Locale, mode: &ExitSelectionMode) -> &'static str {
    match mode {
        ExitSelectionMode::MemberChoice => t(locale, "exit_governance.mode.member_choice"),
        ExitSelectionMode::Fixed { .. } => t(locale, "exit_governance.mode.fixed"),
    }
}

fn exit_selection_source_label(locale: Locale, source: &ExitSelectionSource) -> String {
    match source {
        ExitSelectionSource::Network => t(locale, "exit_governance.source.network").into(),
        ExitSelectionSource::Group { group_id } => tr(
            locale,
            "exit_governance.source.group",
            &[("group_id", group_id.as_str())],
        ),
        ExitSelectionSource::Member => t(locale, "exit_governance.source.member").into(),
        ExitSelectionSource::SelectionMode => {
            t(locale, "exit_governance.source.selection_mode").into()
        }
        ExitSelectionSource::Availability => {
            t(locale, "exit_governance.source.availability").into()
        }
    }
}

fn exit_selection_reason_label(locale: Locale, code: &str) -> String {
    let suffix = code
        .strip_prefix("domain.exit_selection_")
        .or_else(|| code.strip_prefix("exit.selection."))
        .or_else(|| code.strip_prefix("exit.availability."))
        .unwrap_or(code);
    let key = format!("exit_governance.reason.{suffix}");
    let localized = t(locale, &key);
    if localized == key.as_str() {
        code.into()
    } else {
        localized.into()
    }
}

fn exit_selection_candidate_rows(
    network_id: &str,
    candidate: &ExitSelectionCandidate,
    locale: Locale,
) -> Vec<RowModel> {
    let source = exit_selection_source_label(locale, &candidate.source);
    let reason = exit_selection_reason_label(locale, &candidate.reason_code);
    let availability = t(
        locale,
        match candidate.availability.state {
            ExitAvailabilityState::Unknown => "member_directory.exit.state.unknown",
            ExitAvailabilityState::Available => "member_directory.exit.state.available",
            ExitAvailabilityState::Degraded => "member_directory.exit.state.degraded",
            ExitAvailabilityState::Unavailable => "member_directory.exit.state.unavailable",
        },
    );
    let availability_source = t(
        locale,
        match candidate.availability.source {
            ExitAvailabilitySource::NotObserved => "member_directory.exit.source.not_observed",
            ExitAvailabilitySource::LocalRuntime => "member_directory.exit.source.local_runtime",
            ExitAvailabilitySource::CoordinatorHealth => {
                "member_directory.exit.source.coordinator_health"
            }
            ExitAvailabilitySource::RemotePeerReport => {
                "member_directory.exit.source.remote_peer_report"
            }
        },
    );
    let availability_reason = candidate
        .availability
        .reason_code
        .as_deref()
        .map(|code| format!("{} ({code})", exit_selection_reason_label(locale, code)))
        .unwrap_or_else(|| t(locale, "common.not_available").into());
    let status = t(
        locale,
        if candidate.selectable {
            "exit_governance.selectable"
        } else {
            "exit_governance.blocked"
        },
    );
    vec![row(
        tr(
            locale,
            "exit_governance.candidate_title",
            &[("network", network_id), ("exit", candidate.exit.as_str())],
        ),
        tr(
            locale,
            "exit_governance.candidate_detail",
            &[
                ("status", status),
                ("source", &source),
                ("reason", &reason),
                ("reason_code", &candidate.reason_code),
                ("availability", availability),
                ("availability_source", availability_source),
                ("availability_reason", &availability_reason),
            ],
        ),
    )]
}

fn exit_selection_rows(
    selections: &[crate::node::NodeExitSelectionSnapshot],
    locale: Locale,
) -> Vec<RowModel> {
    let mut rows = Vec::new();
    for selection in selections {
        let resolution = selection.resolution.as_ref();
        let effective = resolution
            .map(|resolution| {
                resolution
                    .selectable_exits
                    .iter()
                    .map(NodeId::as_str)
                    .collect::<Vec<_>>()
                    .join(t(locale, "common.list_separator"))
            })
            .unwrap_or_else(|| t(locale, "exit_governance.no_effective").into());
        let default_exit = resolution
            .and_then(|resolution| resolution.default_exit.as_ref())
            .map_or_else(|| t(locale, "common.none").to_string(), ToString::to_string);
        let fixed_exit = resolution
            .and_then(|resolution| resolution.fixed_exit.as_ref())
            .map_or_else(|| t(locale, "common.none").to_string(), ToString::to_string);
        let mode = resolution.map_or_else(
            || t(locale, "common.not_available").to_string(),
            |resolution| exit_selection_mode_label(locale, &resolution.mode).to_string(),
        );
        let generated_at =
            routing_timestamp_label(selection.directory_generated_at_unix_ms, locale);
        let received_at = routing_timestamp_label(selection.directory_received_at_unix_ms, locale);
        let evaluated_at = routing_timestamp_label(Some(selection.evaluated_at_unix_ms), locale);
        rows.push(row(
            selection.network_id.to_string(),
            tr(
                locale,
                "exit_governance.summary_detail",
                &[
                    ("mode", &mode),
                    ("policy_revision", &selection.policy_revision.to_string()),
                    ("selectable", &effective),
                    ("default_exit", &default_exit),
                    ("fixed_exit", &fixed_exit),
                    ("generated_at", &generated_at),
                    ("received_at", &received_at),
                    ("evaluated_at", &evaluated_at),
                ],
            ),
        ));
        if let Some(reason_code) = selection.reason_code.as_deref() {
            let reason = exit_selection_reason_label(locale, reason_code);
            rows.push(row(
                t(locale, "exit_governance.top_reason"),
                tr(
                    locale,
                    "exit_governance.top_reason_detail",
                    &[("reason", &reason), ("reason_code", reason_code)],
                ),
            ));
        }
        if let Some(resolution) = resolution {
            for candidate in &resolution.candidates {
                rows.extend(exit_selection_candidate_rows(
                    selection.network_id.as_str(),
                    candidate,
                    locale,
                ));
            }
            if selection.policy.is_none() {
                rows.push(row(
                    t(locale, "exit_governance.policy_rules"),
                    t(locale, "exit_governance.policy_hidden"),
                ));
            }
        }
    }
    rows
}

fn project_exit_node(
    snapshot: &NodeSnapshot,
    locale: Locale,
) -> (Vec<MetricModel>, Vec<SectionModel>) {
    let capabilities = &snapshot.exit_node_capabilities;
    let local_exit = &snapshot.local_exit;
    let local_exit_stage = t(locale, local_exit_stage_key(local_exit.stage));
    let local_exit_health = t(
        locale,
        if local_exit.healthy {
            "gpui.local_exit.healthy"
        } else {
            "gpui.local_exit.unhealthy"
        },
    );
    let local_exit_clients = tr(
        locale,
        "gpui.local_exit.clients_detail",
        &[
            ("active_clients", &local_exit.active_clients.to_string()),
            ("total_clients", &local_exit.total_clients.to_string()),
        ],
    );
    let local_exit_transition = routing_timestamp_label(
        (local_exit.last_transition_at_unix_ms > 0)
            .then_some(local_exit.last_transition_at_unix_ms),
        locale,
    );
    let mut local_exit_rows = vec![
        row(
            t(locale, "gpui.local_exit.owner_intent"),
            display_bool(locale, local_exit.owner_exit_enabled),
        ),
        row(t(locale, "gpui.local_exit.stage"), local_exit_stage),
        row(
            t(locale, "gpui.local_exit.network_authorization"),
            display_bool(locale, local_exit.network_allowed),
        ),
        row(t(locale, "gpui.local_exit.health"), local_exit_health),
        row(t(locale, "gpui.local_exit.clients"), local_exit_clients),
        row(
            t(locale, "gpui.local_exit.last_transition"),
            local_exit_transition,
        ),
    ];
    if let Some(problem) = &local_exit.problem {
        let params = problem
            .params
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect::<Vec<_>>();
        let localized = tr(locale, &problem.message_key, &params);
        let message = if localized == problem.message_key {
            problem.message.clone()
        } else {
            localized
        };
        local_exit_rows.push(row(
            t(locale, "gpui.local_exit.problem"),
            tr(
                locale,
                "gpui.local_exit.problem_detail",
                &[
                    ("code", node_problem_code_value(problem.code)),
                    ("message_key", problem.message_key.as_str()),
                    ("message", &message),
                    ("retryable", display_bool(locale, problem.retryable)),
                ],
            ),
        ));
    }
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
    let governance_rows = exit_selection_rows(&snapshot.exit_selections, locale);
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
            metric(
                t(locale, "gpui.metric.exit_active_clients"),
                local_exit.active_clients,
                "NodeSnapshot.local_exit.active_clients",
            ),
        ],
        vec![
            section(
                t(locale, "gpui.section.local_exit_status"),
                t(locale, "gpui.empty.local_exit_status"),
                local_exit_rows,
            ),
            section(
                t(locale, "gpui.section.exit_governance"),
                t(locale, "gpui.empty.exit_governance"),
                governance_rows,
            ),
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

fn local_exit_stage_key(stage: LocalExitCapabilityStage) -> &'static str {
    match stage {
        LocalExitCapabilityStage::Disabled => "gpui.local_exit.stage.disabled",
        LocalExitCapabilityStage::WaitingForNetworkAuthorization => {
            "gpui.local_exit.stage.waiting_for_network_authorization"
        }
        LocalExitCapabilityStage::Listening => "gpui.local_exit.stage.listening",
        LocalExitCapabilityStage::HealthChecking => "gpui.local_exit.stage.health_checking",
        LocalExitCapabilityStage::Enabled => "gpui.local_exit.stage.enabled",
        LocalExitCapabilityStage::Draining => "gpui.local_exit.stage.draining",
        LocalExitCapabilityStage::Cleaning => "gpui.local_exit.stage.cleaning",
        LocalExitCapabilityStage::Failed => "gpui.local_exit.stage.failed",
    }
}

fn node_problem_code_value(code: NodeProblemCode) -> &'static str {
    match code {
        NodeProblemCode::AuthenticationRequired => "authentication_required",
        NodeProblemCode::InvalidRequest => "invalid_request",
        NodeProblemCode::NotFound => "not_found",
        NodeProblemCode::Conflict => "conflict",
        NodeProblemCode::Unavailable => "unavailable",
        NodeProblemCode::ResourceExhausted => "resource_exhausted",
        NodeProblemCode::PermissionDenied => "permission_denied",
        NodeProblemCode::Cancelled => "cancelled",
        NodeProblemCode::TimedOut => "timed_out",
        NodeProblemCode::Unsupported => "unsupported",
        NodeProblemCode::Internal => "internal",
    }
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

const RELAY_GOVERNANCE_RENDER_LIMIT: usize = 50;

fn project_relay(snapshot: &NodeSnapshot, locale: Locale) -> (Vec<MetricModel>, Vec<SectionModel>) {
    let services = snapshot
        .embedded_services
        .iter()
        .map(|service| {
            let status = t(locale, embedded_service_status_key(service.status));
            let signaling = display_option(locale, service.signaling_addr);
            let relay = display_option(locale, service.relay_addr);
            let health = service.relay_health.as_ref().map_or_else(
                || t(locale, "relay.health.unknown").to_owned(),
                |health| relay_health_status_label(locale, health.status),
            );
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
            .with_relay_health(health)
        })
        .collect::<Vec<_>>();
    let roles = snapshot
        .networks
        .iter()
        .filter(|network| network.roles.contains(&NodeNetworkRole::Relay))
        .map(|network| row(network.name.clone(), t(locale, "gpui.value.relay_role")))
        .collect::<Vec<_>>();
    let health_rows = snapshot
        .embedded_services
        .iter()
        .filter_map(|service| {
            service.relay_health.as_ref().map(|health| {
                row(
                    service.network_name.clone(),
                    relay_health_detail(locale, health),
                )
            })
        })
        .collect::<Vec<_>>();
    let issue_rows = snapshot
        .embedded_services
        .iter()
        .flat_map(|service| {
            let mut rows = service
                .relay_health
                .as_ref()
                .map(|health| {
                    health
                        .issues
                        .iter()
                        .map(|issue| {
                            let reason = health
                                .reason_code
                                .as_deref()
                                .unwrap_or_else(|| t(locale, "common.not_available"));
                            row(
                                service.network_name.clone(),
                                tr(
                                    locale,
                                    "relay.issue_detail",
                                    &[
                                        ("issue", relay_issue_label(locale, *issue)),
                                        ("reason_code", reason),
                                    ],
                                ),
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if rows.is_empty() {
                if let Some(reason) = service
                    .relay_health
                    .as_ref()
                    .and_then(|health| health.reason_code.as_deref())
                {
                    rows.push(row(
                        service.network_name.clone(),
                        tr(locale, "relay.reason_code", &[("reason_code", reason)]),
                    ));
                }
            }
            rows
        })
        .collect::<Vec<_>>();
    let limit_rows = snapshot
        .embedded_services
        .iter()
        .filter_map(|service| {
            service.relay_health.as_ref().map(|health| {
                row(
                    service.network_name.clone(),
                    relay_limit_detail(locale, &health.limits, &health.limit_sources),
                )
            })
        })
        .collect::<Vec<_>>();
    let usage_rows = snapshot
        .embedded_services
        .iter()
        .filter_map(|service| {
            service.relay_health.as_ref().map(|health| {
                row(
                    service.network_name.clone(),
                    relay_usage_detail(locale, health),
                )
            })
        })
        .collect::<Vec<_>>();
    let authorization_rows = snapshot
        .embedded_services
        .iter()
        .flat_map(|service| {
            let Some(audit) = service.relay_authorization.as_ref() else {
                return Vec::new();
            };
            let mut rows = vec![row(
                service.network_name.clone(),
                relay_authorization_detail(locale, audit),
            )];
            rows.extend(audit.decisions.iter().map(|decision| {
                row(
                    format!("{} · {}", service.network_name, decision.node_id),
                    relay_authorization_decision_detail(locale, decision),
                )
            }));
            if audit.truncated {
                rows.push(row(
                    service.network_name.clone(),
                    t(locale, "relay.authorization.truncated").to_owned(),
                ));
            }
            rows
        })
        .collect::<Vec<_>>();
    let governance_rows = snapshot
        .embedded_services
        .iter()
        .flat_map(|service| {
            let Some(audit) = service.relay_governance.as_ref() else {
                return Vec::new();
            };
            let visible_events = audit
                .events
                .get(
                    audit
                        .events
                        .len()
                        .saturating_sub(RELAY_GOVERNANCE_RENDER_LIMIT)..,
                )
                .unwrap_or_default();
            let mut rows = visible_events
                .iter()
                .map(|event| {
                    row(
                        format!(
                            "{} · {}",
                            service.network_name,
                            relay_governance_action_label(locale, event.action)
                        ),
                        relay_governance_event_detail(locale, event),
                    )
                })
                .collect::<Vec<_>>();
            if audit.events.len() > visible_events.len() {
                rows.push(row(
                    service.network_name.clone(),
                    tr(
                        locale,
                        "relay.governance.bounded",
                        &[
                            (
                                "shown",
                                &relay_format_u64(locale, visible_events.len() as u64),
                            ),
                            (
                                "total",
                                &relay_format_u64(locale, audit.events.len() as u64),
                            ),
                        ],
                    ),
                ));
            }
            if audit.dropped_events > 0 {
                rows.push(row(
                    service.network_name.clone(),
                    tr(
                        locale,
                        "relay.governance.dropped_detail",
                        &[("dropped", &relay_format_u64(locale, audit.dropped_events))],
                    ),
                ));
            }
            rows
        })
        .collect::<Vec<_>>();
    let healthy_services = snapshot
        .embedded_services
        .iter()
        .filter(|service| {
            service
                .relay_health
                .as_ref()
                .is_some_and(|health| health.status == RelayHealthStatus::Healthy)
        })
        .count();
    (
        vec![
            metric(
                t(locale, "gpui.metric.embedded_services"),
                snapshot.embedded_services.len(),
                "NodeSnapshot.embedded_services",
            ),
            metric(
                t(locale, "gpui.metric.relay_services"),
                snapshot
                    .embedded_services
                    .iter()
                    .filter(|service| {
                        service.relay_status != NodeEmbeddedServiceStatus::Stopped
                            || service.relay_health.is_some()
                    })
                    .count(),
                "NodeEmbeddedServiceSnapshot.relay_status",
            ),
            metric(
                t(locale, "gpui.metric.relay_healthy_services"),
                healthy_services,
                "RelayHealthSnapshot.status",
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
                t(locale, "gpui.section.relay_health"),
                t(locale, "gpui.empty.relay_health"),
                health_rows,
            ),
            section(
                t(locale, "gpui.section.relay_issues"),
                t(locale, "gpui.empty.relay_issues"),
                issue_rows,
            ),
            section(
                t(locale, "gpui.section.relay_limits"),
                t(locale, "gpui.empty.relay_limits"),
                limit_rows,
            ),
            section(
                t(locale, "gpui.section.relay_usage"),
                t(locale, "gpui.empty.relay_usage"),
                usage_rows,
            ),
            section(
                t(locale, "gpui.section.relay_authorization"),
                t(locale, "gpui.empty.relay_authorization"),
                authorization_rows,
            ),
            section(
                t(locale, "gpui.section.relay_governance"),
                t(locale, "gpui.empty.relay_governance"),
                governance_rows,
            ),
            section(
                t(locale, "gpui.section.current_roles"),
                t(locale, "gpui.empty.relay_roles"),
                roles,
            ),
        ],
    )
}

trait RelayHealthRowExt {
    fn with_relay_health(self, health: String) -> Self;
}

impl RelayHealthRowExt for RowModel {
    fn with_relay_health(mut self, health: String) -> Self {
        self.detail.push_str(" · ");
        self.detail.push_str(&health);
        self
    }
}

fn relay_health_status_label(locale: Locale, status: RelayHealthStatus) -> String {
    t(locale, relay_health_status_key(status)).to_owned()
}

fn relay_health_status_key(status: RelayHealthStatus) -> &'static str {
    match status {
        RelayHealthStatus::Unknown => "relay.health.unknown",
        RelayHealthStatus::Healthy => "relay.health.healthy",
        RelayHealthStatus::Degraded => "relay.health.degraded",
        RelayHealthStatus::Unavailable => "relay.health.unavailable",
        RelayHealthStatus::QuotaExhausted => "relay.health.quota_exhausted",
    }
}

fn relay_lifecycle_key(lifecycle: RelayLifecycleState) -> &'static str {
    match lifecycle {
        RelayLifecycleState::Stopped => "relay.lifecycle.stopped",
        RelayLifecycleState::Starting => "relay.lifecycle.starting",
        RelayLifecycleState::Running => "relay.lifecycle.running",
        RelayLifecycleState::Stopping => "relay.lifecycle.stopping",
        RelayLifecycleState::Failed => "relay.lifecycle.failed",
        RelayLifecycleState::Unsupported => "relay.lifecycle.unsupported",
    }
}

fn relay_reachability_key(reachability: RelayReachabilityState) -> &'static str {
    match reachability {
        RelayReachabilityState::Unknown => "relay.reachability.unknown",
        RelayReachabilityState::Reachable => "relay.reachability.reachable",
        RelayReachabilityState::Unreachable => "relay.reachability.unreachable",
    }
}

fn relay_issue_key(issue: RelayHealthIssue) -> &'static str {
    match issue {
        RelayHealthIssue::ListenerUnavailable => "relay.issue.listener_unavailable",
        RelayHealthIssue::NetworkReachabilityUnknown => "relay.issue.network_reachability_unknown",
        RelayHealthIssue::NetworkUnreachable => "relay.issue.network_unreachable",
        RelayHealthIssue::ProtocolError => "relay.issue.protocol_error",
        RelayHealthIssue::QuotaExhausted => "relay.issue.quota_exhausted",
        RelayHealthIssue::ResourcePressure => "relay.issue.resource_pressure",
        RelayHealthIssue::ForwardingStalled => "relay.issue.forwarding_stalled",
        RelayHealthIssue::ForwardingNotObserved => "relay.issue.forwarding_not_observed",
        RelayHealthIssue::AuthorizationExpired => "relay.issue.authorization_expired",
        RelayHealthIssue::CpuMeasurementUnsupported => "relay.issue.cpu_measurement_unsupported",
        RelayHealthIssue::MemoryMeasurementUnsupported => {
            "relay.issue.memory_measurement_unsupported"
        }
    }
}

fn relay_issue_label(locale: Locale, issue: RelayHealthIssue) -> &'static str {
    t(locale, relay_issue_key(issue))
}

fn relay_limit_source_key(source: RelayLimitSource) -> &'static str {
    match source {
        RelayLimitSource::NetworkAcl => "relay.limit_source.network_acl",
        RelayLimitSource::NodeLocal => "relay.limit_source.node_local",
        RelayLimitSource::ProviderDefault => "relay.limit_source.provider_default",
    }
}

fn relay_measurement_key(state: RelayMeasurementState) -> &'static str {
    match state {
        RelayMeasurementState::Unsupported => "relay.measurement.unsupported",
        RelayMeasurementState::NotMeasured => "relay.measurement.not_measured",
        RelayMeasurementState::Measured => "relay.measurement.measured",
    }
}

fn relay_health_detail(locale: Locale, health: &RelayHealthSnapshot) -> String {
    let status = relay_health_status_label(locale, health.status);
    let listener = relay_listener_detail(locale, health);
    let lifecycle = t(locale, relay_lifecycle_key(health.lifecycle));
    let reachability = t(locale, relay_reachability_key(health.network_reachability));
    let forwarding = t(
        locale,
        if health.forwarding_observed {
            "relay.forwarding.observed"
        } else {
            "relay.forwarding.not_observed"
        },
    );
    let authorization = t(
        locale,
        if health.authorization_valid {
            "relay.authorization.valid"
        } else {
            "relay.authorization.invalid"
        },
    );
    let quota = t(
        locale,
        if health.quota_limited {
            "relay.quota.limited"
        } else {
            "relay.quota.available"
        },
    );
    let observed_at = routing_timestamp_label(Some(health.observed_at_unix_ms), locale);
    tr(
        locale,
        "relay.service_detail",
        &[
            ("health", &status),
            ("lifecycle", lifecycle),
            ("listener", &listener),
            ("reachability", reachability),
            ("forwarding", forwarding),
            ("authorization", authorization),
            ("quota", quota),
            ("observed_at", &observed_at),
        ],
    )
}

fn relay_listener_detail(locale: Locale, health: &RelayHealthSnapshot) -> String {
    let state = t(
        locale,
        if health.listener_bound {
            "relay.listener.bound"
        } else {
            "relay.listener.unbound"
        },
    );
    health.listen_addr.map_or_else(
        || state.to_owned(),
        |listen_addr| format!("{state} · {listen_addr}"),
    )
}

fn relay_authorization_detail(locale: Locale, audit: &RelayAuthorizationAuditSnapshot) -> String {
    let updated_at = routing_timestamp_label(Some(audit.policy_updated_at_unix_ms), locale);
    tr(
        locale,
        "relay.authorization.snapshot_detail",
        &[
            ("network", &audit.network_id.to_string()),
            ("provider", &audit.provider_node_id.to_string()),
            ("revision", &relay_format_u64(locale, audit.policy_revision)),
            ("updated_at", &updated_at),
            (
                "existing_sessions",
                t(
                    locale,
                    relay_existing_session_policy_key(audit.existing_session_policy),
                ),
            ),
        ],
    )
}

fn relay_authorization_decision_detail(
    locale: Locale,
    decision: &RelayAuthorizationDiagnostic,
) -> String {
    let final_result = relay_authorization_result(locale, decision.allowed);
    let provider = relay_authorization_source_detail(locale, &decision.provider);
    let consumer = decision
        .consumer
        .as_ref()
        .map(|source| relay_authorization_source_detail(locale, source))
        .unwrap_or_else(|| t(locale, "relay.authorization.consumer_missing").to_owned());
    let final_reason = decision
        .final_reason
        .map(|reason| relay_authorization_reason(locale, reason))
        .unwrap_or_else(|| t(locale, "relay.authorization.reason.allowed").to_owned());
    tr(
        locale,
        "relay.authorization.decision_detail",
        &[
            ("result", &final_result),
            ("final_reason", &final_reason),
            ("provider", &provider),
            ("consumer", &consumer),
        ],
    )
}

fn relay_authorization_source_detail(
    locale: Locale,
    source: &RelayAuthorizationSourceDiagnostic,
) -> String {
    let result = relay_authorization_result(locale, source.allowed);
    let reason = relay_authorization_reason(locale, source.reason);
    let layer = source
        .layer
        .map(|layer| relay_authorization_layer(locale, layer))
        .unwrap_or_else(|| t(locale, "relay.authorization.layer.none").to_owned());
    let reauthentication = if source.reauthentication_satisfied {
        t(locale, "relay.authorization.reauth.satisfied")
    } else {
        t(locale, "relay.authorization.reauth.required")
    };
    tr(
        locale,
        "relay.authorization.source_detail",
        &[
            ("result", &result),
            ("reason", &reason),
            ("layer", &layer),
            (
                "acl_revision",
                &relay_format_u64(locale, source.network_acl_revision),
            ),
            (
                "local_revision",
                &relay_format_u64(locale, source.local_policy_revision),
            ),
            ("reauth", reauthentication),
        ],
    )
}

fn relay_authorization_result(locale: Locale, allowed: bool) -> String {
    t(
        locale,
        if allowed {
            "relay.authorization.allowed"
        } else {
            "relay.authorization.denied"
        },
    )
    .to_owned()
}

fn relay_authorization_reason(locale: Locale, reason: EffectiveAccessReason) -> String {
    t(locale, relay_authorization_reason_key(reason)).to_owned()
}

fn relay_authorization_reason_key(reason: EffectiveAccessReason) -> &'static str {
    match reason {
        EffectiveAccessReason::Allowed => "relay.authorization.reason.allowed",
        EffectiveAccessReason::CrossNetworkDenied => {
            "relay.authorization.reason.cross_network_denied"
        }
        EffectiveAccessReason::NetworkScopeRestricted => {
            "relay.authorization.reason.network_scope_restricted"
        }
        EffectiveAccessReason::LocalContributionVeto => {
            "relay.authorization.reason.local_contribution_veto"
        }
        EffectiveAccessReason::AuthorizationExpired => {
            "relay.authorization.reason.authorization_expired"
        }
        EffectiveAccessReason::AuthorizationRevoked => {
            "relay.authorization.reason.authorization_revoked"
        }
        EffectiveAccessReason::RecentReauthenticationRequired => {
            "relay.authorization.reason.reauthentication_required"
        }
        EffectiveAccessReason::ExplicitDeny => "relay.authorization.reason.explicit_deny",
        EffectiveAccessReason::RoleCeilingExceeded => {
            "relay.authorization.reason.role_ceiling_exceeded"
        }
        EffectiveAccessReason::TemporaryCoordinatorReadOnly => {
            "relay.authorization.reason.temporary_coordinator_read_only"
        }
        EffectiveAccessReason::DefaultDenied => "relay.authorization.reason.default_denied",
        EffectiveAccessReason::InvalidPolicy => "relay.authorization.reason.invalid_policy",
    }
}

fn relay_authorization_layer(locale: Locale, layer: EffectivePolicyLayer) -> String {
    t(
        locale,
        match layer {
            EffectivePolicyLayer::Local => "relay.authorization.layer.local",
            EffectivePolicyLayer::Network => "relay.authorization.layer.network",
            EffectivePolicyLayer::Request => "relay.authorization.layer.request",
        },
    )
    .to_owned()
}

fn relay_existing_session_policy_key(policy: RelayExistingSessionPolicy) -> &'static str {
    match policy {
        RelayExistingSessionPolicy::Continue => "relay.authorization.existing.continue",
        RelayExistingSessionPolicy::Migrate => "relay.authorization.existing.migrate",
        RelayExistingSessionPolicy::Terminate => "relay.authorization.existing.terminate",
        RelayExistingSessionPolicy::Reauthorize => "relay.authorization.existing.reauthorize",
    }
}

fn relay_governance_action_key(action: RelayGovernanceAction) -> &'static str {
    match action {
        RelayGovernanceAction::Enable => "relay.governance.action.enable",
        RelayGovernanceAction::Disable => "relay.governance.action.disable",
        RelayGovernanceAction::AuthorizationChanged => {
            "relay.governance.action.authorization_changed"
        }
        RelayGovernanceAction::QuotaChanged => "relay.governance.action.quota_changed",
        RelayGovernanceAction::SessionRejected => "relay.governance.action.session_rejected",
        RelayGovernanceAction::PacketRejected => "relay.governance.action.packet_rejected",
        RelayGovernanceAction::Failover => "relay.governance.action.failover",
        RelayGovernanceAction::Recovered => "relay.governance.action.recovered",
    }
}

fn relay_governance_action_label(locale: Locale, action: RelayGovernanceAction) -> String {
    t(locale, relay_governance_action_key(action)).to_owned()
}

fn relay_governance_event_detail(locale: Locale, event: &RelayGovernanceEvent) -> String {
    let not_available = t(locale, "common.not_available");
    let node_id = event
        .node_id
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_else(|| not_available.to_owned());
    let observed = event
        .observed
        .map(|value| relay_format_u64(locale, value))
        .unwrap_or_else(|| not_available.to_owned());
    let limit = event
        .limit
        .map(|value| relay_format_u64(locale, value))
        .unwrap_or_else(|| not_available.to_owned());
    let occurred_at = routing_timestamp_label(Some(event.occurred_at_unix_ms), locale);
    let reset_at = routing_timestamp_label(event.reset_at_unix_ms, locale);
    let policy_revision = event
        .policy_revision
        .map(|revision| {
            let previous = event
                .previous_policy_revision
                .map(|previous| relay_format_u64(locale, previous))
                .unwrap_or_else(|| not_available.to_owned());
            let revision = relay_format_u64(locale, revision);
            tr(
                locale,
                "relay.governance.event.policy_revision",
                &[("previous", &previous), ("revision", &revision)],
            )
        })
        .unwrap_or_else(|| not_available.to_owned());
    tr(
        locale,
        "relay.governance.event_detail",
        &[
            ("occurred_at", &occurred_at),
            ("node_id", &node_id),
            ("reason_code", &event.reason_code),
            ("observed", &observed),
            ("limit", &limit),
            ("reset_at", &reset_at),
            ("policy_revision", &policy_revision),
        ],
    )
}

fn relay_limit_detail(
    locale: Locale,
    limits: &RelayLimitPolicy,
    sources: &[RelayLimitSource],
) -> String {
    let none = t(locale, "relay.limit.none");
    let connections = limits
        .max_connections
        .map(|value| format_integer(locale, i64::from(value)))
        .unwrap_or_else(|| none.to_owned());
    let streams = limits
        .max_concurrent_streams
        .map(|value| format_integer(locale, i64::from(value)))
        .unwrap_or_else(|| none.to_owned());
    let bandwidth = limits
        .max_bandwidth_kbps
        .map(|value| format!("{} kbps", relay_format_u64(locale, value)))
        .unwrap_or_else(|| none.to_owned());
    let bytes = limits
        .max_bytes
        .map(|value| format!("{} B", relay_format_u64(locale, value)))
        .unwrap_or_else(|| none.to_owned());
    let cpu = limits
        .max_cpu_percent
        .map(|value| format!("{}%", format_integer(locale, i64::from(value))))
        .unwrap_or_else(|| none.to_owned());
    let memory = limits
        .max_memory_bytes
        .map(|value| format!("{} B", relay_format_u64(locale, value)))
        .unwrap_or_else(|| none.to_owned());
    let duration = limits
        .max_session_duration_secs
        .map(|value| format!("{} s", relay_format_u64(locale, value)))
        .unwrap_or_else(|| none.to_owned());
    let reset_at = limits
        .quota_reset_at_unix_ms
        .map(|value| routing_timestamp_label(Some(value), locale))
        .unwrap_or_else(|| none.to_owned());
    let sources = if sources.is_empty() {
        none.to_owned()
    } else {
        sources
            .iter()
            .map(|source| t(locale, relay_limit_source_key(*source)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    tr(
        locale,
        "relay.limit_detail",
        &[
            ("connections", &connections),
            ("streams", &streams),
            ("bandwidth", &bandwidth),
            ("bytes", &bytes),
            ("cpu", &cpu),
            ("memory", &memory),
            ("duration", &duration),
            ("reset_at", &reset_at),
            ("sources", &sources),
        ],
    )
}

fn relay_usage_detail(locale: Locale, health: &RelayHealthSnapshot) -> String {
    let usage = &health.usage;
    let connections = format_integer(locale, i64::from(usage.active_connections));
    let streams = format_integer(locale, i64::from(usage.active_streams));
    let sessions = format_integer(locale, i64::from(usage.active_sessions));
    let forwarded_packets = relay_format_u64(locale, usage.forwarded_packets);
    let forwarded_bytes = relay_format_u64(locale, usage.forwarded_bytes);
    let rejected_packets = relay_format_u64(locale, usage.rejected_packets);
    let rejected_connections = relay_format_u64(locale, usage.rejected_connections);
    let cpu = relay_measurement_detail(
        locale,
        usage.cpu_measurement,
        usage.observed_cpu_percent.map(u64::from),
        "%",
    );
    let memory = relay_measurement_detail(
        locale,
        usage.memory_measurement,
        usage.observed_memory_bytes,
        " B",
    );
    let last_success = routing_timestamp_label(usage.last_forwarded_at_unix_ms, locale);
    let last_failure = routing_timestamp_label(usage.last_forwarding_failure_at_unix_ms, locale);
    let quota_started = routing_timestamp_label(usage.quota_started_at_unix_ms, locale);
    let quota_reset = routing_timestamp_label(usage.quota_reset_at_unix_ms, locale);
    tr(
        locale,
        "relay.usage_detail",
        &[
            ("connections", &connections),
            ("streams", &streams),
            ("sessions", &sessions),
            ("forwarded_packets", &forwarded_packets),
            ("forwarded_bytes", &forwarded_bytes),
            ("rejected_packets", &rejected_packets),
            ("rejected_connections", &rejected_connections),
            ("cpu", &cpu),
            ("memory", &memory),
            ("last_success", &last_success),
            ("last_failure", &last_failure),
            ("quota_started", &quota_started),
            ("quota_reset", &quota_reset),
        ],
    )
}

fn relay_measurement_detail(
    locale: Locale,
    state: RelayMeasurementState,
    value: Option<u64>,
    suffix: &str,
) -> String {
    let state = t(locale, relay_measurement_key(state));
    value.map_or_else(
        || state.to_owned(),
        |value| format!("{state}: {}{suffix}", relay_format_u64(locale, value)),
    )
}

fn relay_format_u64(locale: Locale, value: u64) -> String {
    i64::try_from(value).map_or_else(|_| value.to_string(), |value| format_integer(locale, value))
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
                row(
                    t(locale, "identity.row.created_at"),
                    routing_timestamp_label(identity.created_at_unix_ms, locale),
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
                t(locale, "identity.security.title"),
                t(locale, "identity.security.detail"),
                vec![row_with_page_navigation(
                    t(locale, "identity.security.title"),
                    t(locale, "identity.security.detail"),
                    "settings",
                    t(locale, "identity.security.settings_button"),
                )],
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
    let mut owner_rows = vec![
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
    ];
    let not_exposed = t(locale, "settings.value.not_exposed");
    let none = t(locale, "common.none");
    let not_available = t(locale, "common.not_available");
    let separator = t(locale, "common.list_separator");
    let access_control_exposed = !snapshot.access_control.is_empty();
    let local_policy = snapshot.access_control.local_policy.as_ref();
    let local_revision = if !access_control_exposed {
        not_exposed.to_string()
    } else {
        local_policy
            .map(|policy| {
                tr(
                    locale,
                    "settings.security.revision",
                    &[("revision", &policy.revision.to_string())],
                )
            })
            .unwrap_or_else(|| none.to_string())
    };
    let local_validity = if !access_control_exposed {
        not_exposed.to_string()
    } else {
        local_policy
            .map(|policy| {
                let issued = routing_timestamp_label(Some(policy.issued_at_unix_ms), locale);
                let expires = routing_timestamp_label(Some(policy.expires_at_unix_ms), locale);
                tr(
                    locale,
                    "settings.security.validity",
                    &[("issued", &issued), ("expires", &expires)],
                )
            })
            .unwrap_or_else(|| none.to_string())
    };
    let local_rule_count = if !access_control_exposed {
        not_exposed.to_string()
    } else {
        local_policy
            .map(|policy| {
                tr(
                    locale,
                    "settings.security.rule_count",
                    &[("count", &policy.rules.len().to_string())],
                )
            })
            .unwrap_or_else(|| none.to_string())
    };
    let local_veto_count = if !access_control_exposed {
        not_exposed.to_string()
    } else {
        local_policy
            .map(|policy| {
                tr(
                    locale,
                    "settings.security.veto_count",
                    &[("count", &policy.contribution_vetoes.len().to_string())],
                )
            })
            .unwrap_or_else(|| none.to_string())
    };
    let network_name = |network_id: &str| {
        snapshot
            .memberships
            .iter()
            .find(|membership| membership.network_id == network_id)
            .map(|membership| membership.network_name.clone())
            .unwrap_or_else(|| network_id.to_string())
    };
    let network_acls = if !access_control_exposed {
        not_exposed.to_string()
    } else if snapshot.access_control.network_acls.is_empty() {
        none.to_string()
    } else {
        snapshot
            .access_control
            .network_acls
            .iter()
            .map(|acl| {
                let network_id = acl.network_id.to_string();
                let network = network_name(&network_id);
                let issued = routing_timestamp_label(Some(acl.issued_at_unix_ms), locale);
                let expires = routing_timestamp_label(Some(acl.expires_at_unix_ms), locale);
                tr(
                    locale,
                    "settings.security.network_acl",
                    &[
                        ("network", &network),
                        ("revision", &acl.revision.to_string()),
                        ("rules", &acl.rules.len().to_string()),
                        ("issued", &issued),
                        ("expires", &expires),
                    ],
                )
            })
            .collect::<Vec<_>>()
            .join(separator)
    };
    let field_privacy_policies = if !access_control_exposed {
        not_exposed.to_string()
    } else if snapshot.access_control.field_privacy_policies.is_empty() {
        none.to_string()
    } else {
        snapshot
            .access_control
            .field_privacy_policies
            .iter()
            .map(|policy| {
                let network = policy
                    .network_id
                    .as_ref()
                    .map(ToString::to_string)
                    .map(|network_id| network_name(&network_id))
                    .unwrap_or_else(|| t(locale, "settings.security.node_wide").to_string());
                let effective = routing_timestamp_label(Some(policy.effective_at_unix_ms), locale);
                let expires = routing_timestamp_label(Some(policy.expires_at_unix_ms), locale);
                tr(
                    locale,
                    "settings.security.field_privacy",
                    &[
                        ("network", &network),
                        ("revision", &policy.revision.to_string()),
                        ("rules", &policy.rules.len().to_string()),
                        ("effective", &effective),
                        ("expires", &expires),
                    ],
                )
            })
            .collect::<Vec<_>>()
            .join(separator)
    };
    let membership_policy_revisions = if snapshot.memberships.is_empty() {
        none.to_string()
    } else {
        snapshot
            .memberships
            .iter()
            .map(|membership| {
                let network_policy = membership
                    .network_policy_revision
                    .map(|revision| revision.to_string())
                    .unwrap_or_else(|| not_available.to_string());
                let privacy = membership
                    .privacy_policy_revision
                    .map(|revision| revision.to_string())
                    .unwrap_or_else(|| not_available.to_string());
                tr(
                    locale,
                    "settings.security.membership_policy",
                    &[
                        ("network", &membership.network_name),
                        ("local", &membership.local_policy_revision.to_string()),
                        ("network_policy", &network_policy),
                        ("privacy", &privacy),
                    ],
                )
            })
            .collect::<Vec<_>>()
            .join(separator)
    };
    let manifest = node_ui_manifest();
    let apply_policy_action = manifest
        .pages
        .iter()
        .flat_map(|page| page.actions.iter())
        .find(|action| action.id == "apply_policy");
    let policy_block_reason =
        apply_policy_action.and_then(|action| action_block_reason(action, snapshot, None));
    let policy_write_access = [
        Some(
            t(
                locale,
                if policy_block_reason.is_some() {
                    "settings.policy_access.read_only"
                } else {
                    "settings.policy_access.writable"
                },
            )
            .to_string(),
        ),
        policy_block_reason.map(|reason| t(locale, reason.message_key()).to_string()),
        Some(tr(
            locale,
            "settings.policy_access.required_permission",
            &[("permission", t(locale, "choice.permission.manage_networks"))],
        )),
        Some(t(locale, "settings.policy_access.request_path").to_string()),
        Some(tr(
            locale,
            "settings.policy_access.command",
            &[("command", "apply_policy")],
        )),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(t(locale, "common.list_separator"));
    owner_rows.extend([
        row(
            t(locale, "settings.row.local_policy_revision"),
            local_revision,
        ),
        row(
            t(locale, "settings.row.local_policy_validity"),
            local_validity,
        ),
        row(
            t(locale, "settings.row.local_policy_rules"),
            local_rule_count,
        ),
        row(
            t(locale, "settings.row.local_policy_vetoes"),
            local_veto_count,
        ),
        row(t(locale, "settings.row.network_acls"), network_acls),
        row(
            t(locale, "settings.row.field_privacy_policies"),
            field_privacy_policies,
        ),
        row(
            t(locale, "settings.row.membership_policy_revisions"),
            membership_policy_revisions,
        ),
        row(
            t(locale, "settings.row.policy_write_access"),
            policy_write_access,
        ),
    ]);
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
            t(locale, "settings.section.local_identity"),
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

fn node_phase_key(phase: NodePhase) -> &'static str {
    match phase {
        NodePhase::Stopped => "status.phase.stopped",
        NodePhase::Starting => "status.phase.starting",
        NodePhase::Running => "status.phase.running",
        NodePhase::Stopping => "status.phase.stopping",
        NodePhase::Failed => "status.phase.failed",
    }
}

fn control_health_key(health: NodeControlHealth) -> &'static str {
    match health {
        NodeControlHealth::Available => "status.control.available",
        NodeControlHealth::Unavailable => "status.control.unavailable",
        NodeControlHealth::Unknown => "status.control.unknown",
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

fn interface_kind_key(kind: NodeInterfaceKind) -> &'static str {
    match kind {
        NodeInterfaceKind::Loopback => "interface.kind.loopback",
        NodeInterfaceKind::Physical => "interface.kind.physical",
        NodeInterfaceKind::Virtual => "interface.kind.virtual",
        NodeInterfaceKind::Tun => "interface.kind.tun",
        NodeInterfaceKind::Unknown => "interface.kind.unknown",
    }
}

fn interface_ownership_key(ownership: NodeInterfaceOwnership) -> &'static str {
    match ownership {
        NodeInterfaceOwnership::System => "interface.ownership.system",
        NodeInterfaceOwnership::Product => "interface.ownership.product",
        NodeInterfaceOwnership::Unknown => "interface.ownership.unknown",
    }
}

fn interface_status_key(status: NodeInterfaceStatus) -> &'static str {
    match status {
        NodeInterfaceStatus::Up => "interface.status.up",
        NodeInterfaceStatus::Down => "interface.status.down",
        NodeInterfaceStatus::Unknown => "interface.status.unknown",
    }
}

fn tun_platform_key(platform: TunPlatform) -> &'static str {
    match platform {
        TunPlatform::Linux => "runtime.value.tun_platform.linux",
        TunPlatform::Macos => "runtime.value.tun_platform.macos",
        TunPlatform::Windows => "runtime.value.tun_platform.windows",
        TunPlatform::Other => "runtime.value.tun_platform.other",
    }
}

fn tun_reason_key(reason: TunCapabilityReason) -> &'static str {
    match reason {
        TunCapabilityReason::PlatformUnsupported => "runtime.value.tun_reason.platform_unsupported",
        TunCapabilityReason::PermissionDenied => "runtime.value.tun_reason.permission_denied",
        TunCapabilityReason::DeviceUnavailable => "runtime.value.tun_reason.device_unavailable",
        TunCapabilityReason::ToolUnavailable => "runtime.value.tun_reason.tool_unavailable",
        TunCapabilityReason::Unknown => "runtime.value.tun_reason.unknown",
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

    fn lifecycle_state_for_status(
        status: NetworkMemberStatus,
    ) -> fuckipv6_common::domain::MemberLifecycleStatus {
        match status {
            NetworkMemberStatus::Pending => {
                fuckipv6_common::domain::MemberLifecycleStatus::PendingApproval
            }
            NetworkMemberStatus::Approved => {
                fuckipv6_common::domain::MemberLifecycleStatus::Approved
            }
            NetworkMemberStatus::Active => fuckipv6_common::domain::MemberLifecycleStatus::Active,
            NetworkMemberStatus::Degraded => {
                fuckipv6_common::domain::MemberLifecycleStatus::Degraded
            }
            NetworkMemberStatus::Suspended => {
                fuckipv6_common::domain::MemberLifecycleStatus::Suspended
            }
            NetworkMemberStatus::Exited => fuckipv6_common::domain::MemberLifecycleStatus::Exited,
            NetworkMemberStatus::Removed => fuckipv6_common::domain::MemberLifecycleStatus::Removed,
            NetworkMemberStatus::Expired => fuckipv6_common::domain::MemberLifecycleStatus::Expired,
        }
    }

    use super::*;
    use crate::node::{
        node_ui_manifest, NodeEmbeddedServiceSnapshot, NodeExitSelectionSnapshot,
        NodeIdentitySnapshot, NodeInterfaceAddress, NodeInterfaceSnapshot,
        NodeNetworkMembershipSnapshot, NodeNetworkSnapshot, NodeNetworkStatus, NodePeerSnapshot,
        NodeProblem, NodeProblemCode, NodeRoutingDefaultAction, NodeRoutingSummarySnapshot,
        NodeRoutingSummaryStatus, NodeRoutingTargetSnapshot, NodeTrafficSnapshot, NodeUiField,
        NodeUiValidation,
    };
    use fuckipv6_common::domain::{
        ActivityEntry, AuditId, AuditOutcome, DomainSuffix, MembershipState, NetworkMemberPresence,
        NodeId, PortRange, RelayGovernanceAuditSnapshot, RelayUsageSnapshot, TopologyBoundary,
        TopologyConnection, TopologyConnectionState, TopologyNode, TopologyNodeNetwork,
        TrafficDefaultPolicy, TrafficFailureKind, TrafficFailurePolicy, TrafficMatch,
        TrafficRoutingHardConstraints, TrafficRoutingObservedHit, TrafficRule, TrafficRuleId,
    };
    use serde::Deserialize;
    use std::collections::BTreeMap;

    #[derive(Debug, Deserialize)]
    struct DiscoveryUiParityFixture {
        schema_version: u32,
        steps: Vec<DiscoveryUiParityStep>,
    }

    #[derive(Debug, Deserialize)]
    struct DiscoveryUiParityStep {
        action_id: String,
        values: HashMap<String, String>,
        wire: serde_json::Value,
    }

    fn replace_discovery_fixture_placeholder(value: &mut serde_json::Value, invitation_code: &str) {
        match value {
            serde_json::Value::String(value) if value == "$INVITATION_CODE" => {
                *value = invitation_code.into();
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    replace_discovery_fixture_placeholder(value, invitation_code);
                }
            }
            serde_json::Value::Object(values) => {
                for value in values.values_mut() {
                    replace_discovery_fixture_placeholder(value, invitation_code);
                }
            }
            _ => {}
        }
    }

    fn routing_summary(
        network_name: &str,
        default_action: NodeRoutingDefaultAction,
        effective_target: Option<NodeRoutingTargetSnapshot>,
        status: NodeRoutingSummaryStatus,
        problem_key: Option<&str>,
    ) -> NodeRoutingSummarySnapshot {
        NodeRoutingSummarySnapshot {
            network_name: network_name.into(),
            default_action,
            effective_target,
            enabled_rule_count: 3,
            last_applied_revision: Some(7),
            last_applied_at_unix_ms: Some(1_700_000_000_000),
            status,
            problem: problem_key.map(|key| {
                NodeProblem::new(NodeProblemCode::Unavailable, format!("raw {key}"))
                    .for_network(network_name)
                    .with_message_key(key)
            }),
        }
    }

    fn membership_snapshot(network_name: &str, network_id: &str) -> NodeNetworkMembershipSnapshot {
        NodeNetworkMembershipSnapshot {
            network_name: network_name.into(),
            network_id: network_id.into(),
            node_id: "local-node".into(),
            state: MembershipState::Active,
            role: MembershipRole::Member,
            local_policy_revision: 1,
            network_policy_revision: Some(1),
            privacy_policy_revision: Some(1),
            member_limit: None,
            exit_contribution: Some(ExitContributionObligation::NotRequired),
            exit_contribution_scope: Some(ExitContributionScope::NotApplicable),
            exit_contribution_mode: Some(ExitContributionMode::Disabled),
            terms_hash: None,
            current_terms: None,
            current_terms_hash: None,
            policy_confirmation_required: false,
            acknowledged_at_unix_ms: None,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
            suspension_reason: None,
        }
    }

    fn member_fixture(
        node_id: &str,
        role: MembershipRole,
        group: NetworkMemberGroup,
        capabilities: Option<Vec<CapabilityKind>>,
        status: NetworkMemberStatus,
        presence: NetworkMemberPresence,
    ) -> NetworkMember {
        NetworkMember {
            node_id: NodeId::from_public_key(node_id.as_bytes()),
            display_name: Some(node_id.into()),
            identity_fingerprint: Some(format!("fingerprint-{node_id}")),
            role,
            groups: vec![group],
            capabilities,
            status,
            lifecycle_state: lifecycle_state_for_status(status),
            revision: 1,
            access_epoch: 1,
            available_actions: Vec::new(),
            presence,
            exit_availability: Default::default(),
            joined_at_unix_ms: Some(1_700_000_000_000),
        }
    }

    #[test]
    fn member_directory_defaults_to_the_authoritative_network_id() {
        let action = node_ui_manifest()
            .pages
            .into_iter()
            .flat_map(|page| page.actions)
            .find(|action| action.id == "list_network_members")
            .expect("list member action");
        let mut snapshot = NodeSnapshot::default();
        snapshot
            .memberships
            .push(membership_snapshot("home", "network-authoritative-id"));

        let values = default_form_values(&action, &snapshot, Some("home"));
        assert_eq!(
            values.get("network_id").map(String::as_str),
            Some("network-authoritative-id")
        );

        let values = default_form_values(&action, &snapshot, Some("not-joined"));
        assert_eq!(values.get("network_id").map(String::as_str), Some(""));
    }

    #[test]
    fn member_directory_filters_are_stable_and_distinguish_unknown_capabilities() {
        let owner = member_fixture(
            "owner-node",
            MembershipRole::Owner,
            NetworkMemberGroup::Owner,
            Some(vec![CapabilityKind::Coordinator]),
            NetworkMemberStatus::Active,
            NetworkMemberPresence::live(1_700_000_000_500),
        );
        let unknown = member_fixture(
            "unknown-node",
            MembershipRole::Member,
            NetworkMemberGroup::Member,
            None,
            NetworkMemberStatus::Suspended,
            NetworkMemberPresence::unknown(),
        );
        let empty = member_fixture(
            "empty-node",
            MembershipRole::Member,
            NetworkMemberGroup::Member,
            Some(Vec::new()),
            NetworkMemberStatus::Exited,
            NetworkMemberPresence::disconnected(1_700_000_000_600),
        );
        let file = member_fixture(
            "file-node",
            MembershipRole::Member,
            NetworkMemberGroup::Member,
            Some(vec![CapabilityKind::File]),
            NetworkMemberStatus::Approved,
            NetworkMemberPresence::unknown(),
        );
        let owner_id = owner.node_id.as_str().to_string();
        let unknown_id = unknown.node_id.as_str().to_string();
        let file_id = file.node_id.as_str().to_string();
        let members = vec![owner, unknown, empty, file];

        let mut expected = members
            .iter()
            .map(|member| member.node_id.as_str().to_string())
            .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(
            sort_network_members(&members)
                .into_iter()
                .map(|member| member.node_id.as_str().to_string())
                .collect::<Vec<_>>(),
            expected
        );

        let no_filters = filter_network_members(&members, MemberDirectoryFilters::default());
        assert_eq!(no_filters.len(), members.len());
        assert_eq!(
            no_filters
                .iter()
                .map(|member| member.node_id.as_str().to_string())
                .collect::<Vec<_>>(),
            expected
        );

        let owners = filter_network_members(
            &members,
            MemberDirectoryFilters {
                role: Some(MembershipRole::Owner),
                group: Some(NetworkMemberGroup::Owner),
                ..MemberDirectoryFilters::default()
            },
        );
        assert_eq!(owners.len(), 1);
        assert_eq!(owners[0].node_id.as_str(), owner_id);

        let unknown_capabilities = filter_network_members(
            &members,
            MemberDirectoryFilters {
                capability: MemberCapabilityFilter::Unknown,
                ..MemberDirectoryFilters::default()
            },
        );
        assert_eq!(unknown_capabilities.len(), 1);
        assert_eq!(unknown_capabilities[0].node_id.as_str(), unknown_id);

        let file_capabilities = filter_network_members(
            &members,
            MemberDirectoryFilters {
                capability: MemberCapabilityFilter::Known(CapabilityKind::File),
                ..MemberDirectoryFilters::default()
            },
        );
        assert_eq!(file_capabilities.len(), 1);
        assert_eq!(file_capabilities[0].node_id.as_str(), file_id);
    }

    #[test]
    fn member_directory_health_filters_cover_statuses_and_unknown_presence() {
        let statuses = [
            NetworkMemberStatus::Pending,
            NetworkMemberStatus::Approved,
            NetworkMemberStatus::Active,
            NetworkMemberStatus::Degraded,
            NetworkMemberStatus::Suspended,
            NetworkMemberStatus::Exited,
            NetworkMemberStatus::Removed,
            NetworkMemberStatus::Expired,
        ];
        let members = statuses
            .into_iter()
            .enumerate()
            .map(|(index, status)| {
                member_fixture(
                    &format!("status-{index}"),
                    MembershipRole::Member,
                    NetworkMemberGroup::Member,
                    Some(Vec::new()),
                    status,
                    NetworkMemberPresence::unknown(),
                )
            })
            .collect::<Vec<_>>();

        for status in statuses {
            let filtered = filter_network_members(
                &members,
                MemberDirectoryFilters {
                    health: MemberHealthFilter::Status(status),
                    ..MemberDirectoryFilters::default()
                },
            );
            assert_eq!(
                filtered.len(),
                1,
                "missing status filter result for {status:?}"
            );
            assert_eq!(filtered[0].status, status);
        }

        let online = member_fixture(
            "online-node",
            MembershipRole::Member,
            NetworkMemberGroup::Member,
            Some(Vec::new()),
            NetworkMemberStatus::Active,
            NetworkMemberPresence::live(1_700_000_000_700),
        );
        let offline = member_fixture(
            "offline-node",
            MembershipRole::Member,
            NetworkMemberGroup::Member,
            Some(Vec::new()),
            NetworkMemberStatus::Active,
            NetworkMemberPresence::disconnected(1_700_000_000_800),
        );
        let unknown = member_fixture(
            "unknown-presence-node",
            MembershipRole::Member,
            NetworkMemberGroup::Member,
            Some(Vec::new()),
            NetworkMemberStatus::Active,
            NetworkMemberPresence::unknown(),
        );
        let presence_members = vec![online, offline, unknown];

        let offline_only = filter_network_members(
            &presence_members,
            MemberDirectoryFilters {
                health: MemberHealthFilter::Presence(NetworkMemberOnlineState::Offline),
                ..MemberDirectoryFilters::default()
            },
        );
        assert_eq!(offline_only.len(), 1);
        assert_eq!(
            offline_only[0].presence.state,
            NetworkMemberOnlineState::Offline
        );
        let unknown_only = filter_network_members(
            &presence_members,
            MemberDirectoryFilters {
                health: MemberHealthFilter::Presence(NetworkMemberOnlineState::Unknown),
                ..MemberDirectoryFilters::default()
            },
        );
        assert_eq!(unknown_only.len(), 1);
        assert_eq!(
            unknown_only[0].presence.state,
            NetworkMemberOnlineState::Unknown
        );
    }

    #[test]
    fn member_directory_selection_survives_refresh_only_while_member_exists() {
        let member = member_fixture(
            "selected-node",
            MembershipRole::Member,
            NetworkMemberGroup::Member,
            Some(Vec::new()),
            NetworkMemberStatus::Active,
            NetworkMemberPresence::unknown(),
        );
        let selected = member.node_id.as_str().to_string();
        let members = vec![member];

        assert_eq!(
            retain_member_selection(Some(&selected), &members),
            Some(selected.clone())
        );
        assert_eq!(
            retain_member_selection(Some("removed-node"), &members),
            None
        );
        assert_eq!(retain_member_selection(None, &members), None);
        assert_eq!(retain_member_selection(Some(&selected), &[]), None);
    }

    fn traffic_routing_page_fixture(
        observation: TrafficRoutingObservation,
    ) -> TrafficRoutingPageModel {
        let node_id = NodeId::from_public_key(b"gpui-routing-node");
        let network_id = NetworkId::from_public_key(b"gpui-routing-network");
        let exit_id = NodeId::from_public_key(b"gpui-routing-tokyo-exit");
        let policy = TrafficPolicy {
            node_id,
            revision: 7,
            default_policy: TrafficDefaultPolicy {
                action: TrafficAction::Exit(exit_id.clone()),
                confirmed_by: TrafficDefaultConfirmedBy::Owner,
                confirmed_at_unix_ms: 1_700_000_000_000,
            },
            failure_policy: TrafficFailurePolicy::fail_closed(),
            rules: vec![TrafficRule {
                rule_id: TrafficRuleId::from_public_key(b"gpui-routing-rule"),
                priority: 1,
                enabled: true,
                matcher: TrafficMatch {
                    domain_suffixes: vec![DomainSuffix::parse("example.com").unwrap()],
                    destination_ports: vec![PortRange {
                        start: 443,
                        end: 443,
                    }],
                    protocols: vec![TrafficProtocol::Tcp],
                    ..TrafficMatch::default()
                },
                action: TrafficAction::Direct,
                description: "Development traffic".into(),
            }],
        };
        TrafficRoutingPageModel::from_policy_with_observation(
            &policy,
            TrafficRoutingWorkflowState::Applied { revision: 7 },
            TrafficRoutingHardConstraints::default(),
            vec![
                TrafficRoutingTarget::direct(),
                TrafficRoutingTarget::reject(),
                TrafficRoutingTarget::network(network_id, "Home"),
                TrafficRoutingTarget::exit(exit_id, "Tokyo Exit"),
            ],
            true,
            observation,
        )
        .expect("traffic routing page fixture")
    }

    fn traffic_routing_observation_with_hit() -> TrafficRoutingObservation {
        TrafficRoutingObservation::Available {
            window_start_unix_ms: 1_700_000_000_000,
            window_end_unix_ms: 1_700_000_060_000,
            observed_flow_count: 12,
            hits: vec![TrafficRoutingObservedHit {
                sample_id: "sample-1".into(),
                activity_id: Some("activity-1".into()),
                diagnostic_id: Some("diagnostic-1".into()),
                flow: TrafficFlow {
                    domain: Some(
                        fuckipv6_common::domain::DomainName::parse("api.example.com").unwrap(),
                    ),
                    destination_ip: "203.0.113.10".parse().unwrap(),
                    destination_port: 443,
                    protocol: TrafficProtocol::Tcp,
                    application: ApplicationIdentity::Unknown,
                },
            }],
        }
    }

    fn populated_snapshot() -> NodeSnapshot {
        let mut snapshot = NodeSnapshot::default();
        snapshot.lifecycle.phase = NodePhase::Running;
        snapshot.identity = Some(NodeIdentitySnapshot {
            node_id: "node-1".into(),
            display_name: Some("desktop".into()),
            public_key_fingerprint: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
            created_at_unix_ms: Some(1_700_000_000_000),
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
    fn traffic_routing_page_missing_is_unavailable_and_never_uses_legacy_config() {
        let mut snapshot = populated_snapshot();
        snapshot.traffic_routing_page = None;
        let (metrics, sections) = project_routing(&snapshot, Locale::En);
        assert!(metrics.is_empty());

        let text = sections
            .iter()
            .flat_map(|section| {
                std::iter::once(section.title.as_str()).chain(
                    section
                        .rows
                        .iter()
                        .flat_map(|row| [row.title.as_str(), row.detail.as_str()]),
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains(t(Locale::En, "traffic.routing.authority.unavailable")));
        assert!(!text.contains("10.0.0.0/8"));
        assert!(!text.contains("peer-1"));
        assert!(!text.contains("Mesh only"));
    }

    #[test]
    fn traffic_routing_page_projects_authority_workflow_rules_and_targets() {
        let mut snapshot = populated_snapshot();
        snapshot.traffic_routing_page = Some(traffic_routing_page_fixture(
            traffic_routing_observation_with_hit(),
        ));
        let (metrics, sections) = project_routing(&snapshot, Locale::En);

        assert_eq!(metrics[0].value, "7");
        assert_eq!(metrics[1].value, "1");
        assert_eq!(metrics[2].value, "4");

        let authority = sections
            .iter()
            .find(|section| section.title == t(Locale::En, "traffic.routing.eyebrow"))
            .expect("routing authority section");
        let authority_text = authority
            .rows
            .iter()
            .map(|row| format!("{} {}", row.title, row.detail))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(authority_text.contains(t(Locale::En, "traffic.routing.state.applied")));
        assert!(authority_text.contains("Tokyo Exit"));
        assert!(authority_text.contains(t(Locale::En, "traffic.routing.fail_closed")));

        let rules = sections
            .iter()
            .find(|section| section.title == t(Locale::En, "traffic.routing.rules.title"))
            .expect("routing rules section");
        assert_eq!(rules.rows.len(), 1);
        assert!(rules.rows[0].title.contains("Development traffic"));
        assert!(rules.rows[0]
            .detail
            .contains(t(Locale::En, "traffic.routing.rule.enabled")));
        assert!(rules.rows[0].detail.contains("Direct"));

        let targets = sections
            .iter()
            .find(|section| section.title == t(Locale::En, "traffic.routing.targets"))
            .expect("routing targets section");
        assert!(targets.rows.iter().any(|row| row.title == "Tokyo Exit"));
        assert!(targets.rows.iter().any(|row| row.title == "Home"));
    }

    #[test]
    fn traffic_routing_impact_distinguishes_observed_zero_hit_and_unavailable() {
        let zero_hits = TrafficRoutingObservation::Available {
            window_start_unix_ms: 1_700_000_000_000,
            window_end_unix_ms: 1_700_000_060_000,
            observed_flow_count: 0,
            hits: Vec::new(),
        };
        let mut snapshot = populated_snapshot();
        snapshot.traffic_routing_page = Some(traffic_routing_page_fixture(zero_hits));
        let (_, sections) = project_routing(&snapshot, Locale::En);
        let impact = sections
            .iter()
            .find(|section| section.title == t(Locale::En, "traffic.routing.rule.impact_title"))
            .expect("zero-hit impact section");
        assert!(impact.rows.iter().all(|row| row
            .detail
            .contains(t(Locale::En, "traffic.routing.rule.impact_no_hits"))));

        snapshot.traffic_routing_page = Some(traffic_routing_page_fixture(
            TrafficRoutingObservation::unavailable("runtime.collector_offline"),
        ));
        let (_, sections) = project_routing(&snapshot, Locale::En);
        let impact = sections
            .iter()
            .find(|section| section.title == t(Locale::En, "traffic.routing.rule.impact_title"))
            .expect("unavailable impact section");
        assert!(impact.rows.iter().all(|row| {
            row.detail
                .contains(t(Locale::En, "traffic.routing.rule.impact_unavailable"))
                && row.detail.contains("runtime.collector_offline")
        }));
    }

    #[test]
    fn traffic_routing_hit_samples_expose_activity_and_diagnostic_links() {
        let mut snapshot = populated_snapshot();
        snapshot.traffic_routing_page = Some(traffic_routing_page_fixture(
            traffic_routing_observation_with_hit(),
        ));
        let (_, sections) = project_routing(&snapshot, Locale::En);
        let impact = sections
            .iter()
            .find(|section| section.title == t(Locale::En, "traffic.routing.rule.impact_title"))
            .expect("routing impact section");
        let text = impact
            .rows
            .iter()
            .map(|row| format!("{} {}", row.title, row.detail))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("activity-1"));
        assert!(text.contains("diagnostic-1"));
        assert!(text.contains("api.example.com:443"));
        assert!(text.contains(t(Locale::En, "gpui.routing.hit_samples")));
    }

    #[test]
    fn traffic_routing_target_search_only_uses_authoritative_aliases() {
        let mut snapshot = populated_snapshot();
        snapshot.traffic_routing_page = Some(traffic_routing_page_fixture(
            TrafficRoutingObservation::default(),
        ));

        let tokyo = routing_target_search_rows(&snapshot, "TOKYO", Locale::En);
        assert_eq!(tokyo.len(), 1);
        assert_eq!(tokyo[0].title, "Tokyo Exit");
        let navigation = tokyo[0].navigation.as_ref().expect("exit navigation");
        assert_eq!(navigation.page_id, "topology");
        assert!(navigation
            .object_id
            .as_deref()
            .is_some_and(|id| !id.is_empty()));

        let home = routing_target_search_rows(&snapshot, "home", Locale::En);
        assert_eq!(home.len(), 1);
        assert_eq!(home[0].title, "Home");
        assert_eq!(
            home[0]
                .navigation
                .as_ref()
                .and_then(|navigation| navigation.network_name.as_deref()),
            Some("Home")
        );

        assert!(routing_target_search_rows(&snapshot, "fk-home", Locale::En).is_empty());
        assert!(routing_target_search_rows(&snapshot, "peer-1", Locale::En).is_empty());
        assert!(routing_target_search_rows(&snapshot, "   ", Locale::En).is_empty());
    }

    fn activity_entry(audit_key: &[u8], occurred_at_unix_ms: u64) -> ActivityEntry {
        ActivityEntry {
            audit_id: AuditId::from_public_key(audit_key),
            operation_id: None,
            actor: NodeId::from_public_key(b"activity-actor"),
            network_id: None,
            action: "network.create".into(),
            resource: "network:created".into(),
            outcome: AuditOutcome::Succeeded,
            occurred_at_unix_ms,
            summary: "private activity summary".into(),
            category: fuckipv6_common::domain::ActivityCategory::UserActivity,
            severity: fuckipv6_common::domain::ActivitySeverity::Info,
            actor_role: Some("owner".into()),
            object_kind: "network".into(),
            object_id: None,
            reason: Some("succeeded".into()),
            details: BTreeMap::from([("secret".into(), "private activity details".into())]),
        }
    }

    #[test]
    fn node_runtime_projects_authoritative_routing_summaries_and_navigation() {
        let mut snapshot = NodeSnapshot::default();
        snapshot.lifecycle.phase = NodePhase::Running;
        snapshot.routing_summaries = vec![
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
                NodeRoutingSummaryStatus::Stale,
                Some("runtime.routing.problem.stopped"),
            ),
            routing_summary(
                "exit",
                NodeRoutingDefaultAction::Exit("peer-1".into()),
                Some(NodeRoutingTargetSnapshot::Exit {
                    node_id: "peer-1".into(),
                }),
                NodeRoutingSummaryStatus::Invalid,
                Some("runtime.routing.problem.invalid"),
            ),
            routing_summary(
                "reject",
                NodeRoutingDefaultAction::Reject,
                None,
                NodeRoutingSummaryStatus::ApplyFailed,
                Some("runtime.routing.problem.rollback_failed"),
            ),
        ];
        let manifest = node_ui_manifest();
        let model = build_page_model(&manifest, &snapshot, "node_runtime", Locale::En)
            .expect("node runtime model");
        let section = model
            .sections
            .iter()
            .find(|section| section.title == t(Locale::En, "runtime.section.routing"))
            .expect("routing summary section");

        assert_eq!(section.rows.len(), 4);
        assert!(section.rows[0]
            .detail
            .contains(t(Locale::En, "routing.action.direct")));
        assert!(section.rows[0]
            .detail
            .contains(t(Locale::En, "routing.status.current")));
        assert!(section.rows[1]
            .detail
            .contains(t(Locale::En, "routing.action.network")));
        assert!(section.rows[1].detail.contains("network"));
        assert!(section.rows[1]
            .detail
            .contains(t(Locale::En, "routing.status.stale")));
        assert!(section.rows[1]
            .detail
            .contains(t(Locale::En, "runtime.routing.problem.stopped")));
        assert!(section.rows[2]
            .detail
            .contains(t(Locale::En, "routing.action.exit")));
        assert!(section.rows[2].detail.contains("peer-1"));
        assert!(section.rows[2]
            .detail
            .contains(t(Locale::En, "routing.status.invalid")));
        assert!(section.rows[3]
            .detail
            .contains(t(Locale::En, "routing.action.reject")));
        assert!(section.rows[3]
            .detail
            .contains(t(Locale::En, "routing.status.apply_failed")));
        assert!(section.rows[3]
            .detail
            .contains(t(Locale::En, "runtime.routing.problem.rollback_failed")));
        for row in &section.rows {
            assert!(row.detail.contains('3'));
            assert!(row.detail.contains("7"));
            assert!(row.detail.contains("2023-11-14 22:13:20 UTC"));
            let navigation = row.navigation.as_ref().expect("typed route navigation");
            assert_eq!(navigation.page_id, "routing");
            assert_eq!(navigation.network_name.as_deref(), Some(row.title.as_str()));
            assert_eq!(navigation.label, t(Locale::En, "routing.manage"));
        }
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
        assert_eq!(overview.primary_page_id.as_deref(), Some("node_runtime"));
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
        let discover = overview
            .shortcuts
            .iter()
            .find(|shortcut| shortcut.id == "discover_networks")
            .expect("discover shortcut");
        assert_eq!(
            discover.action_id.as_deref(),
            Some("refresh_discovery_index")
        );
        assert!(discover.unavailable_reason.is_none());
    }

    #[test]
    fn overview_keeps_node_lifecycle_and_traffic_protection_independent() {
        let manifest = node_ui_manifest();
        let page = manifest
            .pages
            .iter()
            .find(|page| page.id == "overview")
            .expect("overview page");
        let mut snapshot = populated_snapshot();
        snapshot
            .traffic_session
            .enter_fail_closed(
                Some(TrafficFailureKind::HandshakeInterrupted),
                1_700_000_000_500,
                "recovery.handshake_interrupted",
            )
            .expect("project fail-closed traffic session");
        let model =
            build_page_model_for_page(page, &snapshot, Locale::ZhCn).expect("overview model");
        let overview = model.overview.as_ref().expect("dedicated overview model");

        assert_eq!(overview.primary_status, NodePrimaryStatus::Running);
        assert_eq!(
            overview.traffic_session.state,
            TrafficProtectionState::FailClosed
        );
        assert_eq!(
            overview.traffic_session.state_label,
            t(Locale::ZhCn, "traffic.session.state.fail_closed")
        );
        assert_eq!(
            overview.traffic_session.reason_code.as_deref(),
            Some("recovery.handshake_interrupted")
        );
        assert!(overview.traffic_session.validation.is_empty());
        assert_eq!(overview.traffic_session.events.len(), 1);
        assert_eq!(
            overview.traffic_session.events[0].kind,
            TrafficSessionEventKind::FailClosed
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
            effective: UiMode::Gpui,
            display: crate::ui::DisplayEnvironment::LinuxX11,
            gpui_available: true,
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
                t(Locale::ZhCn, "settings.section.local_identity"),
                t(Locale::ZhCn, "settings.section.remote_access"),
                t(Locale::ZhCn, "settings.section.services_updates"),
                t(Locale::ZhCn, "settings.section.discovery_index"),
                t(Locale::ZhCn, "settings.section.data_recovery"),
                t(Locale::ZhCn, "settings.section.network_listeners"),
                t(Locale::ZhCn, "settings.section.dns_privacy"),
                t(Locale::ZhCn, "settings.section.transport_limits"),
                t(Locale::ZhCn, "settings.section.observability_maintenance"),
            ]
        );
        assert!(model
            .sections
            .iter()
            .all(|section| !section.rows.is_empty()));

        let owner = &model.sections[0];
        assert_eq!(
            owner
                .rows
                .iter()
                .map(|row| row.title.as_str())
                .collect::<Vec<_>>(),
            vec![
                t(Locale::ZhCn, "settings.row.owner_identity"),
                t(Locale::ZhCn, "settings.row.node_id"),
                t(Locale::ZhCn, "settings.row.public_key_fingerprint"),
                t(Locale::ZhCn, "settings.row.access_model"),
                t(Locale::ZhCn, "settings.row.granted_permissions"),
                t(Locale::ZhCn, "settings.row.local_policy_revision"),
                t(Locale::ZhCn, "settings.row.local_policy_validity"),
                t(Locale::ZhCn, "settings.row.local_policy_rules"),
                t(Locale::ZhCn, "settings.row.local_policy_vetoes"),
                t(Locale::ZhCn, "settings.row.network_acls"),
                t(Locale::ZhCn, "settings.row.field_privacy_policies"),
                t(Locale::ZhCn, "settings.row.membership_policy_revisions"),
                t(Locale::ZhCn, "settings.row.policy_write_access"),
            ]
        );
        assert_eq!(owner.rows[0].detail, "desktop");
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
            row.title == t(Locale::ZhCn, "settings.row.metrics")
                && row
                    .detail
                    .contains(t(Locale::ZhCn, "settings.advanced.state.unsupported"))
        }));
        let rendered = model
            .sections
            .iter()
            .flat_map(|section| {
                std::iter::once(section.title.as_str()).chain(
                    section
                        .rows
                        .iter()
                        .flat_map(|row| [row.title.as_str(), row.detail.as_str()]),
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        for removed in [
            "settings.section.web_security",
            "settings.row.web_authentication",
            "settings.row.owner_recovery",
            "settings.row.webui_configuration",
            "settings.row.session_security",
            "settings.row.webui_listener",
        ] {
            assert!(
                !rendered.contains(t(Locale::ZhCn, removed)),
                "desktop settings must not expose WebUI-only authentication surface: {removed}"
            );
        }
    }

    #[test]
    fn settings_policy_access_reports_typed_read_only_path() {
        let manifest = node_ui_manifest();
        let page = manifest
            .pages
            .iter()
            .find(|page| page.id == "settings")
            .expect("settings page");
        let mut snapshot = populated_snapshot();
        snapshot
            .permissions
            .permissions
            .retain(|permission| *permission != crate::node::NodePermission::ManageNetworks);

        let model =
            build_page_model_for_page(page, &snapshot, Locale::ZhCn).expect("settings model");
        let access = model.sections[0]
            .rows
            .iter()
            .find(|row| row.title == t(Locale::ZhCn, "settings.row.policy_write_access"))
            .expect("policy write access row");

        assert!(access
            .detail
            .contains(t(Locale::ZhCn, "settings.policy_access.read_only")));
        assert!(access
            .detail
            .contains(t(Locale::ZhCn, "blocked.permission_denied")));
        assert!(access.detail.contains(&tr(
            Locale::ZhCn,
            "settings.policy_access.required_permission",
            &[(
                "permission",
                t(Locale::ZhCn, "choice.permission.manage_networks"),
            )],
        )));
        assert!(access
            .detail
            .contains(t(Locale::ZhCn, "settings.policy_access.request_path")));
        assert!(access.detail.contains(&tr(
            Locale::ZhCn,
            "settings.policy_access.command",
            &[("command", "apply_policy")],
        )));
        assert!(!access
            .detail
            .contains(t(Locale::ZhCn, "settings.policy_access.writable")));
    }

    #[test]
    fn every_gpui_page_omits_webui_authentication_copy() {
        let manifest = node_ui_manifest();
        let snapshot = populated_snapshot();
        let forbidden = [
            "settings.section.owner_security",
            "settings.section.web_security",
            "settings.row.web_authentication",
            "settings.row.owner_recovery",
            "settings.row.session_security",
            "settings.row.webui_configuration",
            "settings.row.webui_listener",
            "settings.advanced.webui_session.detail",
            "owner_auth.identity.section_title",
            "owner_auth.local_identity.title",
            "owner_auth.local_identity.detail",
            "owner_auth.web_identity.title",
            "owner_auth.recovery_row.title",
            "owner_auth.passkey_recovery.help",
        ];

        for locale in [Locale::ZhCn, Locale::En] {
            for page in &manifest.pages {
                let model = build_page_model_for_page(page, &snapshot, locale)
                    .expect("every manifest page must project");
                let mut visible = Vec::new();
                for section in &model.sections {
                    visible.push(section.title.as_str());
                    for row in &section.rows {
                        visible.extend([row.title.as_str(), row.detail.as_str()]);
                    }
                }
                for metric in &model.metrics {
                    visible.extend([metric.label.as_str(), metric.value.as_str()]);
                }
                if let Some(overview) = &model.overview {
                    visible.push(overview.node_name.as_str());
                    visible.extend(overview.issues.iter().map(String::as_str));
                    for network in &overview.networks {
                        visible.extend([
                            network.name.as_str(),
                            network.tun_name.as_str(),
                            network.routing.as_str(),
                            network.exit.as_str(),
                        ]);
                        visible.extend(network.roles.iter().map(String::as_str));
                    }
                    for activity in &overview.activity {
                        visible.extend([activity.action.as_str(), activity.detail.as_str()]);
                        visible.extend(activity.command.as_deref());
                    }
                    for shortcut in &overview.shortcuts {
                        visible.push(shortcut.label.as_str());
                        visible.extend(shortcut.unavailable_reason.as_deref());
                    }
                }
                let rendered = visible.join("\n");
                for key in forbidden {
                    assert!(
                        !rendered.contains(t(locale, key)),
                        "GPUI page {} must not expose WebUI-only authentication copy in {locale:?}: {key}",
                        page.id
                    );
                }
            }
        }
    }

    #[test]
    fn runtime_projects_authoritative_interfaces_and_diagnostics() {
        let mut snapshot = populated_snapshot();
        snapshot.interfaces.push(NodeInterfaceSnapshot {
            index: 7,
            name: "fk-home".into(),
            kind: NodeInterfaceKind::Tun,
            ownership: NodeInterfaceOwnership::Product,
            status: NodeInterfaceStatus::Up,
            mtu: 1_420,
            addresses: vec![NodeInterfaceAddress {
                address: "fd00::2".parse().expect("IPv6"),
                prefix_len: 64,
            }],
            default_ipv4: false,
            default_ipv6: true,
        });
        snapshot.interface_problems.push(
            NodeProblem::new(NodeProblemCode::Unavailable, "raw host interface error")
                .for_network("home"),
        );

        let page = node_ui_manifest()
            .pages
            .into_iter()
            .find(|page| page.id == "node_runtime")
            .expect("runtime page");
        let model =
            build_page_model_for_page(&page, &snapshot, Locale::ZhCn).expect("runtime model");
        let interfaces = model
            .sections
            .iter()
            .find(|section| section.title == t(Locale::ZhCn, "runtime.section.interfaces"))
            .expect("interface section");
        assert_eq!(interfaces.rows.len(), 1);
        assert_eq!(interfaces.rows[0].title, "fk-home");
        assert!(interfaces.rows[0]
            .detail
            .contains(t(Locale::ZhCn, "interface.kind.tun")));
        assert!(interfaces.rows[0]
            .detail
            .contains(t(Locale::ZhCn, "interface.ownership.product")));
        assert!(interfaces.rows[0]
            .detail
            .contains(t(Locale::ZhCn, "interface.status.up")));
        assert!(interfaces.rows[0].detail.contains("fd00::2/64"));
        assert!(interfaces.rows[0].detail.contains(&tr(
            Locale::ZhCn,
            "runtime.row.interface_defaults",
            &[
                ("ipv4", t(Locale::ZhCn, "common.disabled")),
                ("ipv6", t(Locale::ZhCn, "common.enabled")),
            ],
        )));

        let problems = model
            .sections
            .iter()
            .find(|section| section.title == t(Locale::ZhCn, "runtime.section.interface_problems"))
            .expect("interface diagnostics");
        assert_eq!(problems.rows.len(), 1);
        assert_eq!(problems.rows[0].title, "home");
        assert_eq!(
            problems.rows[0].detail,
            t(Locale::ZhCn, "error.operation_unavailable")
        );
        assert!(!problems.rows[0].detail.contains("raw host interface error"));
    }

    #[test]
    fn runtime_hides_empty_interface_diagnostics_section() {
        let snapshot = populated_snapshot();
        let page = node_ui_manifest()
            .pages
            .into_iter()
            .find(|page| page.id == "node_runtime")
            .expect("runtime page");
        let model =
            build_page_model_for_page(&page, &snapshot, Locale::ZhCn).expect("runtime model");

        assert!(model
            .sections
            .iter()
            .all(|section| section.title != t(Locale::ZhCn, "runtime.section.interface_problems")));
    }

    fn relay_health_fixture(
        forwarding_observed: bool,
        usage: RelayUsageSnapshot,
        limits: RelayLimitPolicy,
    ) -> RelayHealthSnapshot {
        RelayHealthSnapshot::observe(
            RelayLifecycleState::Running,
            Some("127.0.0.1:6667".parse().expect("relay socket")),
            true,
            RelayReachabilityState::Reachable,
            forwarding_observed,
            true,
            limits,
            vec![
                RelayLimitSource::NetworkAcl,
                RelayLimitSource::ProviderDefault,
            ],
            usage,
            1_700_000_100_000,
            Some("relay.test".into()),
        )
    }

    fn relay_service(
        network_name: &str,
        health: Option<RelayHealthSnapshot>,
    ) -> NodeEmbeddedServiceSnapshot {
        NodeEmbeddedServiceSnapshot {
            network_name: network_name.into(),
            status: NodeEmbeddedServiceStatus::Stopped,
            signaling_addr: None,
            relay_status: NodeEmbeddedServiceStatus::Running,
            relay_addr: Some("127.0.0.1:6667".parse().expect("relay socket")),
            relay_health: health,
            relay_authorization: None,
            relay_governance: None,
        }
    }

    #[test]
    fn relay_projection_renders_authoritative_health_limits_usage_and_issues() {
        let fresh = RelayUsageSnapshot {
            active_connections: 2,
            active_streams: 3,
            active_sessions: 1,
            forwarded_packets: 12,
            forwarded_bytes: 4_096,
            rejected_packets: 1,
            rejected_connections: 1,
            last_forwarded_at_unix_ms: Some(1_700_000_099_000),
            ..RelayUsageSnapshot::default()
        };
        let mut quota_usage = fresh.clone();
        quota_usage.active_connections = 1;
        quota_usage.forwarded_bytes = 10;
        let mut stalled_usage = fresh.clone();
        stalled_usage.last_forwarding_failure_at_unix_ms = Some(1_700_000_099_500);
        let mut unsupported_usage = fresh.clone();
        unsupported_usage.cpu_measurement = RelayMeasurementState::Unsupported;
        unsupported_usage.memory_measurement = RelayMeasurementState::Unsupported;
        let snapshot = NodeSnapshot {
            embedded_services: vec![
                relay_service(
                    "listener-only",
                    Some(relay_health_fixture(
                        false,
                        RelayUsageSnapshot::default(),
                        RelayLimitPolicy::default(),
                    )),
                ),
                relay_service(
                    "healthy",
                    Some(relay_health_fixture(
                        true,
                        fresh,
                        RelayLimitPolicy::default(),
                    )),
                ),
                relay_service(
                    "stalled",
                    Some(relay_health_fixture(
                        true,
                        stalled_usage,
                        RelayLimitPolicy::default(),
                    )),
                ),
                relay_service(
                    "quota",
                    Some(relay_health_fixture(
                        true,
                        quota_usage,
                        RelayLimitPolicy {
                            max_connections: Some(1),
                            max_bytes: Some(10),
                            ..RelayLimitPolicy::default()
                        },
                    )),
                ),
                relay_service(
                    "unsupported-measurements",
                    Some(relay_health_fixture(
                        true,
                        unsupported_usage,
                        RelayLimitPolicy {
                            max_cpu_percent: Some(80),
                            max_memory_bytes: Some(1_000_000),
                            ..RelayLimitPolicy::default()
                        },
                    )),
                ),
            ],
            ..NodeSnapshot::default()
        };

        let (metrics, sections) = project_relay(&snapshot, Locale::ZhCn);
        assert_eq!(
            metrics
                .iter()
                .find(|metric| metric.label == t(Locale::ZhCn, "gpui.metric.relay_healthy_services"))
                .map(|metric| metric.value.as_str()),
            Some("1")
        );
        let health = sections
            .iter()
            .find(|section| section.title == t(Locale::ZhCn, "gpui.section.relay_health"))
            .expect("relay health section");
        assert_eq!(health.rows.len(), 5);
        assert!(health.rows[0]
            .detail
            .contains(t(Locale::ZhCn, "relay.health.unknown")));
        assert!(health.rows[1]
            .detail
            .contains(t(Locale::ZhCn, "relay.health.healthy")));
        assert!(health.rows[2]
            .detail
            .contains(t(Locale::ZhCn, "relay.health.degraded")));
        assert!(health.rows[3]
            .detail
            .contains(t(Locale::ZhCn, "relay.health.quota_exhausted")));
        assert!(health.rows[4]
            .detail
            .contains(t(Locale::ZhCn, "relay.health.degraded")));
        assert!(health.rows.iter().all(|row| {
            row.detail.contains(t(Locale::ZhCn, "relay.listener.bound"))
                && row.detail.contains("127.0.0.1:6667")
                && row
                    .detail
                    .contains(t(Locale::ZhCn, "relay.reachability.reachable"))
                && row
                    .detail
                    .contains(t(Locale::ZhCn, "relay.authorization.valid"))
        }));

        let issues = sections
            .iter()
            .find(|section| section.title == t(Locale::ZhCn, "gpui.section.relay_issues"))
            .expect("relay issue section");
        let issue_text = issues
            .rows
            .iter()
            .map(|row| row.detail.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(issue_text.contains(t(Locale::ZhCn, "relay.issue.forwarding_not_observed")));
        assert!(issue_text.contains(t(Locale::ZhCn, "relay.issue.forwarding_stalled")));
        assert!(issue_text.contains(t(Locale::ZhCn, "relay.issue.quota_exhausted")));
        assert!(issue_text.contains(t(Locale::ZhCn, "relay.issue.cpu_measurement_unsupported")));
        assert!(issue_text.contains(t(
            Locale::ZhCn,
            "relay.issue.memory_measurement_unsupported"
        )));

        let limits = sections
            .iter()
            .find(|section| section.title == t(Locale::ZhCn, "gpui.section.relay_limits"))
            .expect("relay limits section");
        assert_eq!(limits.rows.len(), 5);
        assert!(limits.rows[3]
            .detail
            .contains(t(Locale::ZhCn, "relay.limit_source.network_acl")));
        assert!(limits.rows[3]
            .detail
            .contains(t(Locale::ZhCn, "relay.limit_source.provider_default")));

        let usage = sections
            .iter()
            .find(|section| section.title == t(Locale::ZhCn, "gpui.section.relay_usage"))
            .expect("relay usage section");
        assert_eq!(usage.rows.len(), 5);
        assert!(usage.rows[1].detail.contains("12"));
        assert!(usage.rows[1]
            .detail
            .contains(&format_integer(Locale::ZhCn, 4_096)));
        assert!(usage.rows[4]
            .detail
            .contains(t(Locale::ZhCn, "relay.measurement.unsupported")));
        assert!(usage.rows[0]
            .detail
            .contains(t(Locale::ZhCn, "relay.measurement.not_measured")));
    }

    #[test]
    fn relay_projection_renders_bounded_owner_governance_without_sensitive_fields() {
        let mut service = relay_service(
            "governed",
            Some(relay_health_fixture(
                true,
                RelayUsageSnapshot::default(),
                RelayLimitPolicy::default(),
            )),
        );
        service.relay_governance = Some(RelayGovernanceAuditSnapshot {
            events: (0..60)
                .map(|index| RelayGovernanceEvent {
                    action: RelayGovernanceAction::SessionRejected,
                    occurred_at_unix_ms: 1_700_000_100_000 + index,
                    node_id: (index == 59).then(|| NodeId::from_public_key(b"relay-member")),
                    reason_code: format!("relay.session.quota_exhausted.{index}"),
                    observed: Some(index),
                    limit: Some(60),
                    reset_at_unix_ms: Some(1_700_000_160_000),
                    previous_policy_revision: None,
                    policy_revision: None,
                })
                .collect(),
            dropped_events: 3,
        });
        let snapshot = NodeSnapshot {
            embedded_services: vec![service],
            ..NodeSnapshot::default()
        };

        let (_, sections) = project_relay(&snapshot, Locale::En);
        let governance = sections
            .iter()
            .find(|section| section.title == t(Locale::En, "gpui.section.relay_governance"))
            .expect("relay governance section");
        assert_eq!(governance.rows.len(), 52);
        assert!(governance.rows[0]
            .title
            .contains(t(Locale::En, "relay.governance.action.session_rejected")));
        assert!(governance.rows[0]
            .detail
            .contains("relay.session.quota_exhausted.10"));
        assert!(!governance.rows[0]
            .detail
            .contains("relay.session.quota_exhausted.9"));
        assert!(governance.rows[49]
            .detail
            .contains("relay.session.quota_exhausted.59"));
        assert!(governance.rows[50].detail.contains(&tr(
            Locale::En,
            "relay.governance.bounded",
            &[("shown", "50"), ("total", "60")],
        )));
        assert!(governance.rows[51].detail.contains(&tr(
            Locale::En,
            "relay.governance.dropped_detail",
            &[("dropped", "3")],
        )));
        let rendered = governance
            .rows
            .iter()
            .map(|row| format!("{} {}", row.title, row.detail))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!rendered.contains("TOP_SECRET_PAYLOAD"));
        assert!(!rendered.contains("2001:db8::secret"));
        assert!(!rendered.contains("private_key"));
    }

    #[test]
    fn relay_listener_detail_uses_authoritative_bound_state_and_address() {
        let mut health = relay_health_fixture(
            true,
            RelayUsageSnapshot::default(),
            RelayLimitPolicy::default(),
        );
        health.listen_addr = Some("127.0.0.1:6667".parse().expect("relay socket"));
        health.listener_bound = false;

        let detail = relay_listener_detail(Locale::En, &health);
        assert!(detail.contains(t(Locale::En, "relay.listener.unbound")));
        assert!(detail.contains("127.0.0.1:6667"));
        assert!(!detail.contains(t(Locale::En, "relay.listener.bound")));
    }

    #[test]
    fn relay_projection_never_infers_health_from_a_running_listener() {
        let snapshot = NodeSnapshot {
            embedded_services: vec![relay_service("listener-only", None)],
            ..NodeSnapshot::default()
        };
        let (metrics, sections) = project_relay(&snapshot, Locale::En);
        assert_eq!(
            metrics
                .iter()
                .find(|metric| metric.label == t(Locale::En, "gpui.metric.relay_healthy_services"))
                .map(|metric| metric.value.as_str()),
            Some("0")
        );
        let services = sections
            .iter()
            .find(|section| section.title == t(Locale::En, "gpui.section.embedded_services"))
            .expect("embedded service section");
        assert!(services.rows[0]
            .detail
            .contains(t(Locale::En, "relay.health.unknown")));
        assert!(!services.rows[0]
            .detail
            .contains(t(Locale::En, "relay.health.healthy")));
        assert!(sections
            .iter()
            .find(|section| section.title == t(Locale::En, "gpui.section.relay_health"))
            .is_some_and(|section| section.rows.is_empty()));
    }

    #[test]
    fn overview_primary_destination_is_the_runtime_page() {
        let overview = project_overview(&NodeSnapshot::default(), Locale::En);
        assert_eq!(overview.primary_page_id.as_deref(), Some("node_runtime"));
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
                relay_status: NodeEmbeddedServiceStatus::Failed,
                signaling_addr: None,
                relay_addr: None,
                relay_health: None,
                relay_authorization: None,
                relay_governance: None,
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
        let mut activity = [3_u8, 6, 1, 5, 2, 4]
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
                category: fuckipv6_common::domain::ActivityCategory::UserActivity,
                severity: fuckipv6_common::domain::ActivitySeverity::Info,
                actor_role: Some("owner".into()),
                object_kind: "network".into(),
                object_id: None,
                reason: Some("succeeded".into()),
                details: BTreeMap::from([
                    ("command".into(), format!("command-{index}")),
                    ("token".into(), format!("secret-{index}")),
                ]),
            })
            .collect::<Vec<_>>();
        let mut latest_six = activity[1].clone();
        latest_six.occurred_at_unix_ms = 6_500;
        latest_six
            .details
            .insert("command".into(), "command-6-latest".into());
        activity.push(latest_six);
        snapshot.recent_activity = activity;

        let overview = project_overview(&snapshot, Locale::En);

        assert_eq!(overview.activity.len(), 5);
        assert_eq!(overview.activity[0].occurred_at_unix_ms, 6_500);
        assert_eq!(overview.activity[4].occurred_at_unix_ms, 2_000);
        assert_eq!(
            overview.activity[0].command.as_deref(),
            Some("command-6-latest")
        );
        assert!(overview.activity.iter().all(|activity| {
            !activity.detail.contains("raw-summary")
                && !activity.detail.contains("secret-")
                && activity
                    .command
                    .as_deref()
                    .is_none_or(|command| !command.contains("secret-"))
        }));
    }

    #[test]
    fn activity_projection_deduplicates_and_filters_every_field_with_closed_bounds() {
        let alpha = NetworkId::from_public_key(b"activity-alpha");
        let beta = NetworkId::from_public_key(b"activity-beta");
        let mut home = activity_entry(b"activity-home", 2_000);
        home.network_id = Some(alpha.clone());
        home.object_kind = "member".into();
        home.object_id = Some("node-relay".into());
        home.category = ActivityCategory::SecurityEvent;
        home.severity = ActivitySeverity::Warning;
        home.outcome = AuditOutcome::Denied;
        home.actor_role = Some("owner".into());
        let mut latest_home = home.clone();
        latest_home.summary = "latest replay".into();
        let mut failure = activity_entry(b"activity-failure", 3_000);
        failure.network_id = Some(beta);
        failure.object_kind = "network".into();
        failure.object_id = Some("network-office".into());
        failure.category = ActivityCategory::OperationFailure;
        failure.severity = ActivitySeverity::Critical;
        failure.outcome = AuditOutcome::Failed;
        failure.actor_role = Some("member".into());
        let home_id = home.audit_id.clone();
        let failure_id = failure.audit_id.clone();
        let entries = vec![home, failure, latest_home];

        let ordered = ordered_activity_entries(&entries);
        assert_eq!(ordered.len(), 2);
        assert_eq!(ordered[0].audit_id, failure_id);
        assert_eq!(ordered[1].summary, "latest replay");

        let filters = [
            ActivityFilters {
                network_id: Some(alpha.to_string()),
                ..ActivityFilters::default()
            },
            ActivityFilters {
                object_kind: Some("member".into()),
                ..ActivityFilters::default()
            },
            ActivityFilters {
                object_id: Some("node-relay".into()),
                ..ActivityFilters::default()
            },
            ActivityFilters {
                actor_role: Some("owner".into()),
                ..ActivityFilters::default()
            },
            ActivityFilters {
                category: Some(ActivityCategory::SecurityEvent),
                ..ActivityFilters::default()
            },
            ActivityFilters {
                severity: Some(ActivitySeverity::Warning),
                ..ActivityFilters::default()
            },
            ActivityFilters {
                outcome: Some(AuditOutcome::Denied),
                ..ActivityFilters::default()
            },
            ActivityFilters {
                from_unix_ms: Some(2_000),
                to_unix_ms: Some(2_000),
                ..ActivityFilters::default()
            },
        ];
        for filter in filters {
            let rows = filter_activity(&entries, &filter);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].audit_id, home_id);
        }

        let combined = filter_activity(
            &entries,
            &ActivityFilters {
                network_id: Some(alpha.to_string()),
                object_kind: Some("member".into()),
                object_id: Some("node-relay".into()),
                actor_role: Some("owner".into()),
                category: Some(ActivityCategory::SecurityEvent),
                severity: Some(ActivitySeverity::Warning),
                outcome: Some(AuditOutcome::Denied),
                from_unix_ms: Some(2_000),
                to_unix_ms: Some(2_000),
            },
        );
        assert_eq!(combined.len(), 1);
        assert_eq!(combined[0].audit_id, home_id);
    }

    #[test]
    fn activity_page_model_applies_filters_and_never_renders_private_audit_copy() {
        let mut snapshot = populated_snapshot();
        let mut succeeded = activity_entry(b"activity-succeeded", 1_000);
        succeeded.object_kind = "network".into();
        let mut failed = activity_entry(b"activity-failed", 2_000);
        failed.category = ActivityCategory::OperationFailure;
        failed.severity = ActivitySeverity::Critical;
        failed.outcome = AuditOutcome::Failed;
        failed.object_kind = "member".into();
        failed.object_id = Some("node-missing".into());
        let failed_id = failed.audit_id.clone();
        snapshot.recent_activity = vec![failed, succeeded];

        let model = build_page_model_with_activity_filters(
            &node_ui_manifest(),
            &snapshot,
            "activity",
            Locale::En,
            &ActivityFilters {
                category: Some(ActivityCategory::OperationFailure),
                severity: Some(ActivitySeverity::Critical),
                outcome: Some(AuditOutcome::Failed),
                ..ActivityFilters::default()
            },
        )
        .expect("activity model");

        let rows = model
            .sections
            .iter()
            .flat_map(|section| section.rows.iter())
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].detail.contains(failed_id.as_str()));
        let rendered = rows
            .iter()
            .flat_map(|row| [row.title.as_str(), row.detail.as_str()])
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!rendered.contains("private activity summary"));
        assert!(!rendered.contains("private activity details"));
    }

    #[test]
    fn activity_object_links_require_current_permission_existence_and_network_ownership() {
        let (topology, alpha, beta, relay) = topology_snapshot_fixture();
        let viewer = topology.viewer.clone();
        let mut snapshot = populated_snapshot();
        snapshot.topology = Some(topology);
        snapshot
            .memberships
            .push(membership_snapshot("alpha", alpha.as_str()));
        snapshot
            .memberships
            .push(membership_snapshot("beta", beta.as_str()));

        let mut member_activity = activity_entry(b"activity-member", 1_000);
        member_activity.network_id = Some(alpha.clone());
        member_activity.object_kind = "member".into();
        member_activity.object_id = Some(relay.to_string());
        let navigation = activity_object_navigation(&snapshot, &member_activity, Locale::En)
            .expect("visible member link");
        assert_eq!(navigation.page_id, "topology");
        assert_eq!(navigation.network_name.as_deref(), Some("alpha"));

        let mut denied = snapshot.clone();
        denied.permissions.permissions.clear();
        assert!(activity_object_navigation(&denied, &member_activity, Locale::En).is_none());

        let mut deleted = snapshot.clone();
        deleted
            .topology
            .as_mut()
            .expect("topology")
            .nodes
            .retain(|node| node.node_id != relay);
        assert!(activity_object_navigation(&deleted, &member_activity, Locale::En).is_none());

        let mut mismatched_network = activity_entry(b"activity-mismatch", 1_000);
        mismatched_network.network_id = Some(beta);
        mismatched_network.object_kind = "member".into();
        mismatched_network.object_id = Some(viewer.to_string());
        assert!(activity_object_navigation(&snapshot, &mismatched_network, Locale::En).is_none());
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
                ("discover_networks", Some("refresh_discovery_index"), false),
                ("join_network", Some("join_network"), false),
            ]
        );
    }

    #[test]
    fn desktop_identity_surface_trusts_the_os_session_without_web_authentication() {
        let manifest = node_ui_manifest();
        let page = manifest
            .pages
            .iter()
            .find(|page| page.id == "identity")
            .expect("identity page");
        let model = build_page_model_for_page(page, &populated_snapshot(), Locale::ZhCn)
            .expect("identity model");
        assert_eq!(model.sections.len(), 3);
        let security_row = model.sections[2]
            .rows
            .first()
            .expect("identity security navigation row");
        let navigation = security_row
            .navigation
            .as_ref()
            .expect("identity security navigation");
        assert_eq!(navigation.page_id, "settings");
        assert_eq!(navigation.network_name, None);
        assert_eq!(
            navigation.label,
            t(Locale::ZhCn, "identity.security.settings_button")
        );
        let rendered = model
            .sections
            .iter()
            .flat_map(|section| {
                std::iter::once(section.title.as_str()).chain(
                    section
                        .rows
                        .iter()
                        .flat_map(|row| [row.title.as_str(), row.detail.as_str()]),
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        for removed in [
            "owner_auth.identity.section_title",
            "owner_auth.local_identity.title",
            "owner_auth.web_identity.title",
            "owner_auth.recovery_row.title",
            "owner_auth.passkey_recovery.help",
        ] {
            assert!(
                !rendered.contains(t(Locale::ZhCn, removed)),
                "desktop identity must not expose WebUI-only authentication surface: {removed}"
            );
        }
        assert!(!rendered.contains("recover-owner"));
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
        assert_eq!(ActionBinding::ALL.len(), 69);
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
        let page = manifest
            .pages
            .iter_mut()
            .find(|page| !page.actions.is_empty())
            .expect("manifest page with actions");
        page.actions[0].id = "unknown_action".into();
        assert!(build_page_model_for_page(page, &NodeSnapshot::default(), Locale::ZhCn).is_err());
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

    fn traffic_policy_command_fixtures() -> (String, String, String, String) {
        use fuckipv6_common::domain::{
            ApplicationIdentity, DomainName, PortRange, TrafficAction, TrafficDefaultConfirmedBy,
            TrafficDefaultPolicy, TrafficFailurePolicy, TrafficFlow, TrafficMatch, TrafficProtocol,
            TrafficRule, TrafficRuleId,
        };
        use std::net::{IpAddr, Ipv4Addr};

        let node_id = NodeId::from_public_key(b"gpui-traffic-command-node");
        let current = TrafficPolicy {
            node_id: node_id.clone(),
            revision: 1,
            default_policy: TrafficDefaultPolicy {
                action: TrafficAction::Direct,
                confirmed_by: TrafficDefaultConfirmedBy::Owner,
                confirmed_at_unix_ms: 1_700_000_000_000,
            },
            failure_policy: TrafficFailurePolicy::fail_closed(),
            rules: vec![TrafficRule {
                rule_id: TrafficRuleId::from_public_key(b"gpui-traffic-command-rule"),
                priority: 1,
                enabled: true,
                matcher: TrafficMatch {
                    exact_domains: vec![DomainName::parse("example.com").unwrap()],
                    domain_suffixes: Vec::new(),
                    destination_cidrs: Vec::new(),
                    destination_ports: vec![PortRange {
                        start: 443,
                        end: 443,
                    }],
                    protocols: vec![TrafficProtocol::Tcp],
                    applications: Vec::new(),
                },
                action: TrafficAction::Reject,
                description: "Reject example traffic".into(),
            }],
        };
        let candidate = TrafficPolicy {
            revision: 2,
            ..current.clone()
        };
        let input = TrafficPolicyTestInput::manual(
            NetworkId::from_public_key(b"gpui-traffic-command-network"),
            TrafficFlow {
                domain: Some(DomainName::parse("example.com").unwrap()),
                destination_ip: IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10)),
                destination_port: 443,
                protocol: TrafficProtocol::Tcp,
                application: ApplicationIdentity::ExecutablePath("/usr/bin/curl".into()),
            },
        );
        (
            serde_json::to_string(&TrafficPolicyDraft::from_applied(&current))
                .expect("traffic policy draft"),
            serde_json::to_string(&current).expect("current traffic policy"),
            serde_json::to_string(&candidate).expect("candidate traffic policy"),
            serde_json::to_string(&input).expect("traffic policy test input"),
        )
    }

    fn policy_test_fixture_json() -> String {
        let network_id = NetworkId::from_public_key(b"gpui-policy-test-network");
        serde_json::json!({
            "network_id": network_id.to_string(),
            "actor": NodeId::from_public_key(b"gpui-policy-test-actor").to_string(),
            "role": "member",
            "groups": [],
            "membership_active": true,
            "recent_reauthentication": true,
            "emergency_coordinator": false,
            "request": {
                "action": "read",
                "resource": "network",
                "resource_network_id": network_id.to_string(),
                "intent": "read_only"
            },
            "now_unix_ms": 1_500
        })
        .to_string()
    }

    #[test]
    fn action_builders_use_typed_node_commands() {
        let manifest = node_ui_manifest();
        let snapshot = populated_snapshot();
        for page in &manifest.pages {
            for action in &page.actions {
                let mut values = default_form_values(action, &snapshot, Some("home"));
                if action.id == "rotate_node_identity" {
                    values.insert(
                        "expected_fingerprint".into(),
                        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
                    );
                }
                if action.id == "join_network" {
                    values.insert(
                        "target".into(),
                        "https://coordinator.example.test/session".into(),
                    );
                }
                if action.id == "acknowledge_membership_policy" {
                    values.insert(
                        "network_id".into(),
                        NetworkId::from_public_key(b"policy-command-network").to_string(),
                    );
                    values.insert(
                        "expected_terms_hash".into(),
                        ContentHash::from_bytes(b"policy-command-terms")
                            .as_str()
                            .to_string(),
                    );
                }
                if action.id == "test_policy" {
                    values.insert("policy_json".into(), policy_test_fixture_json());
                }
                if matches!(
                    action.id.as_str(),
                    "preview_policy" | "apply_policy" | "rollback_policy"
                ) {
                    values.insert("expected_revision".into(), "1".into());
                }
                if matches!(action.id.as_str(), "preview_policy_rollback") {
                    values.insert("expected_revision".into(), "1".into());
                }
                if matches!(
                    action.id.as_str(),
                    "preview_policy_rollback" | "rollback_policy"
                ) {
                    values.insert("target_revision".into(), "1".into());
                }
                if matches!(action.id.as_str(), "preview_policy" | "apply_policy") {
                    values.insert("candidate_json".into(), "{}".into());
                }
                if matches!(action.id.as_str(), "apply_policy" | "rollback_policy") {
                    values.insert(
                        "network_ids".into(),
                        NetworkId::from_public_key(b"policy-command-network").to_string(),
                    );
                    values.insert("includes_local_policy".into(), "false".into());
                    values.insert("confirm_privilege_expansion".into(), "true".into());
                }
                if matches!(
                    action.id.as_str(),
                    "preview_traffic_policy"
                        | "preview_traffic_policy_rollback"
                        | "apply_traffic_policy"
                        | "rollback_traffic_policy"
                ) {
                    values.insert("expected_revision".into(), "1".into());
                }
                if matches!(
                    action.id.as_str(),
                    "preview_traffic_policy_rollback" | "rollback_traffic_policy"
                ) {
                    values.insert("target_revision".into(), "1".into());
                }
                if matches!(
                    action.id.as_str(),
                    "preview_traffic_policy" | "apply_traffic_policy"
                ) {
                    let (draft, _, _, _) = traffic_policy_command_fixtures();
                    values.insert("draft_json".into(), draft);
                }
                if matches!(
                    action.id.as_str(),
                    "apply_traffic_policy" | "rollback_traffic_policy"
                ) {
                    values.insert("confirm_risk".into(), "true".into());
                }
                if action.id == "test_traffic_policy" {
                    let (_, current, candidate, input) = traffic_policy_command_fixtures();
                    values.insert("current_json".into(), current);
                    values.insert("candidate_json".into(), candidate);
                    values.insert("input_json".into(), input);
                }
                if matches!(
                    action.id.as_str(),
                    "preview_member_removal" | "change_member_lifecycle"
                ) {
                    values.insert(
                        "network_id".into(),
                        NetworkId::from_public_key(b"member-command-network").to_string(),
                    );
                    values.insert(
                        "target_node_id".into(),
                        NodeId::from_public_key(b"member-command-target").to_string(),
                    );
                }
                if matches!(
                    action.id.as_str(),
                    "create_invitation"
                        | "list_invitations"
                        | "get_invitation_status"
                        | "revoke_invitation"
                        | "preview_invitation"
                        | "redeem_invitation"
                ) {
                    values.insert(
                        "network_id".into(),
                        NetworkId::from_public_key(b"invitation-command-network").to_string(),
                    );
                    values.insert("role".into(), "member".into());
                    values.insert("max_uses".into(), "1".into());
                    values.insert("expires_in_ms".into(), "86400000".into());
                    values.insert(
                        "invitation_id".into(),
                        InvitationId::from_public_key(b"invitation-command").to_string(),
                    );
                    values.insert("expected_revision".into(), "1".into());
                    values.insert(
                        "coordinator_url".into(),
                        "wss://coordinator.example.test/signaling".into(),
                    );
                    values.insert("code".into(), format!("fkinv_{}", "A".repeat(43)));
                    values.insert("expected_invitation_revision".into(), "1".into());
                    values.insert(
                        "preview_digest".into(),
                        ContentHash::from_bytes(b"preview").as_str().to_string(),
                    );
                    values.insert(
                        "terms_hash".into(),
                        ContentHash::from_bytes(b"terms").as_str().to_string(),
                    );
                    values.insert("acknowledged_at_unix_ms".into(), "1700000000000".into());
                    values.insert("exit_contribution_acknowledged".into(), "true".into());
                }
                let command = build_command(action, &values).expect("typed command");
                assert_ne!(action.id, "");
                assert!(!format!("{command:?}").is_empty());
            }
        }
    }

    #[test]
    fn discovery_flow_parity_fixture_matches_gpui_command_building() {
        let invitation_code = format!("fkinv_{}", "A".repeat(43));
        let mut fixture: DiscoveryUiParityFixture = serde_json::from_str(include_str!(
            "../../../../contracts/typescript/fixtures/discovery-ui-parity.json"
        ))
        .expect("discovery UI parity fixture");
        assert_eq!(fixture.schema_version, 1);
        assert_eq!(
            fixture
                .steps
                .iter()
                .map(|step| step.action_id.as_str())
                .collect::<Vec<_>>(),
            [
                "create_network",
                "publish_network_discovery",
                "preview_join_network",
                "join_network",
                "review_join_request",
                "create_invitation",
                "preview_invitation",
                "redeem_invitation",
                "retry_discovery_index",
            ]
        );

        let manifest = node_ui_manifest();
        for step in &mut fixture.steps {
            for value in step.values.values_mut() {
                if value == "$INVITATION_CODE" {
                    *value = invitation_code.clone();
                }
            }
            replace_discovery_fixture_placeholder(&mut step.wire, &invitation_code);

            let action = manifest
                .pages
                .iter()
                .flat_map(|page| &page.actions)
                .find(|action| action.id == step.action_id)
                .unwrap_or_else(|| panic!("missing manifest action {}", step.action_id));
            assert_eq!(
                action.command.as_str(),
                step.wire["command"]
                    .as_str()
                    .expect("fixture command must be a string"),
                "manifest command mismatch for {}",
                step.action_id
            );
            for field in &action.fields {
                assert!(
                    step.values.contains_key(&field.id),
                    "fixture is missing {} for {}",
                    field.id,
                    step.action_id
                );
            }

            let command = build_command(action, &step.values)
                .unwrap_or_else(|error| panic!("{} failed: {error:?}", step.action_id));
            assert_eq!(
                serde_json::to_value(command).expect("serialize command"),
                step.wire,
                "GPUI command mismatch for {}",
                step.action_id
            );
        }
    }

    #[test]
    fn exit_contribution_mode_canonicalizes_legacy_profile_fields() {
        let manifest = node_ui_manifest();
        let action = manifest
            .pages
            .iter()
            .flat_map(|page| &page.actions)
            .find(|action| action.id == "create_network")
            .expect("create network action");
        let values = HashMap::from([
            ("name".into(), "family".into()),
            ("purpose".into(), "Family devices".into()),
            ("icon".into(), "home".into()),
            ("language".into(), "en".into()),
            ("region".into(), "US".into()),
            ("tags".into(), "family".into()),
            ("capability_declarations".into(), "relay".into()),
            ("visibility".into(), "public".into()),
            ("join_mode".into(), "open".into()),
            ("exit_contribution_scope".into(), "all_members".into()),
            ("signaling_port".into(), "6666".into()),
            ("relay_port".into(), "6667".into()),
            ("external_host".into(), String::new()),
            ("auto_connect".into(), "true".into()),
        ]);

        for (mode, expected_obligation, expected_scope) in [
            (
                "autonomous",
                ExitContributionObligation::NotRequired,
                ExitContributionScope::NotApplicable,
            ),
            (
                "required_on_join",
                ExitContributionObligation::Required,
                ExitContributionScope::AllMembers,
            ),
            (
                "disabled",
                ExitContributionObligation::NotRequired,
                ExitContributionScope::NotApplicable,
            ),
            (
                "approval_required",
                ExitContributionObligation::NotRequired,
                ExitContributionScope::NotApplicable,
            ),
        ] {
            let mut mode_values = values.clone();
            mode_values.insert("exit_contribution_mode".into(), mode.into());
            let command = build_command(action, &mode_values)
                .unwrap_or_else(|error| panic!("{mode} command failed: {error:?}"));
            let NodeCommand::CreateNetwork(command) = command else {
                panic!("expected create network command");
            };
            assert_eq!(command.exit_contribution, expected_obligation, "{mode}");
            assert_eq!(command.exit_contribution_scope, expected_scope, "{mode}");
            assert_eq!(
                command.exit_contribution_mode,
                match mode {
                    "autonomous" => ExitContributionMode::Autonomous,
                    "required_on_join" => ExitContributionMode::RequiredOnJoin,
                    "disabled" => ExitContributionMode::Disabled,
                    "approval_required" => ExitContributionMode::ApprovalRequired,
                    _ => unreachable!(),
                },
                "{mode}"
            );
        }

        let mut invalid = values;
        invalid.insert("exit_contribution_mode".into(), "required_on_join".into());
        invalid.insert("exit_contribution_scope".into(), "not_applicable".into());
        assert!(matches!(
            build_command(action, &invalid),
            Err(CommandBuildError::InvalidField { ref action_id, ref field_id, .. })
                if action_id == "create_network" && field_id == "exit_contribution_scope"
        ));
    }

    #[test]
    fn discovery_join_builder_requires_and_preserves_confirmed_preview_fields() {
        let manifest = node_ui_manifest();
        let action = manifest
            .pages
            .iter()
            .flat_map(|page| &page.actions)
            .find(|action| action.id == "join_network")
            .expect("join action");
        let network_id =
            fuckipv6_common::domain::NetworkId::from_public_key(b"preview-network").to_string();
        let terms_hash = fuckipv6_common::domain::ContentHash::from_bytes(b"join terms")
            .as_str()
            .to_string();
        let values = HashMap::from([
            ("target".into(), String::new()),
            ("network_name".into(), "atlas".into()),
            ("start".into(), "true".into()),
            ("network_id".into(), network_id.clone()),
            ("preview_digest".into(), "signed-preview-digest".into()),
            ("network_policy_revision".into(), "7".into()),
            ("privacy_policy_revision".into(), "3".into()),
            ("terms_hash".into(), terms_hash.clone()),
            ("acknowledged_at_unix_ms".into(), "1700000000000".into()),
        ]);

        let command = build_command(action, &values).expect("confirmed discovery join");
        assert!(matches!(
            command,
            NodeCommand::JoinNetwork(command)
                if command.network_id.as_deref() == Some(network_id.as_str())
                    && command.preview_digest.as_deref() == Some("signed-preview-digest")
                    && command.network_policy_revision == Some(7)
                    && command.privacy_policy_revision == Some(3)
                    && command.terms_hash.as_ref().is_some_and(|hash| hash.as_str() == terms_hash)
                    && command.acknowledged_at_unix_ms == Some(1_700_000_000_000)
        ));

        let mut missing_terms = values;
        missing_terms.remove("terms_hash");
        assert!(matches!(
            build_command(action, &missing_terms),
            Err(CommandBuildError::MissingField { field_id, .. }) if field_id == "terms_hash"
        ));
    }

    #[test]
    fn batch_join_request_note_applies_to_non_approval_actions_only() {
        let manifest = node_ui_manifest();
        let action = manifest
            .pages
            .iter()
            .flat_map(|page| &page.actions)
            .find(|action| action.id == "batch_review_join_requests")
            .expect("batch review action");
        let reviews = serde_json::json!([
            {
                "request_id": "approve-request",
                "expected_revision": 3,
                "action": "approve",
                "note": "stale approval note"
            },
            {
                "request_id": "reject-request",
                "expected_revision": 4,
                "action": "reject"
            },
            {
                "request_id": "information-request",
                "expected_revision": 5,
                "action": "request_information"
            }
        ])
        .to_string();
        let command = build_command(
            action,
            &HashMap::from([
                ("network_id".into(), "network-id".into()),
                ("reviews".into(), reviews),
                ("note".into(), "Explain the requested access".into()),
            ]),
        )
        .expect("batch review command");

        let NodeCommand::BatchReviewJoinRequests(command) = command else {
            panic!("expected batch review command");
        };
        assert_eq!(command.reviews.len(), 3);
        assert_eq!(
            command.reviews[0].note.as_deref(),
            Some("stale approval note")
        );
        assert_eq!(
            command.reviews[1].note.as_deref(),
            Some("Explain the requested access")
        );
        assert_eq!(
            command.reviews[2].note.as_deref(),
            Some("Explain the requested access")
        );
    }

    #[test]
    fn identity_rotation_requires_current_fingerprint_and_manage_permission() {
        let manifest = node_ui_manifest();
        let page = manifest
            .pages
            .iter()
            .find(|page| page.id == "identity")
            .expect("identity page");
        let action = page
            .actions
            .iter()
            .find(|action| action.id == "rotate_node_identity")
            .expect("rotate identity action");
        let valid = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

        let command = build_command(
            action,
            &HashMap::from([("expected_fingerprint".into(), valid.into())]),
        )
        .expect("valid fingerprint command");
        assert!(matches!(
            command,
            NodeCommand::RotateNodeIdentity(ref command)
                if command.expected_fingerprint == valid
        ));

        for invalid in [
            "",
            "aa:bb",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        ] {
            let error = build_command(
                action,
                &HashMap::from([("expected_fingerprint".into(), invalid.into())]),
            )
            .expect_err("invalid fingerprint");
            assert!(matches!(
                error,
                CommandBuildError::InvalidField {
                    ref action_id,
                    ref field_id,
                    ..
                } if action_id == "rotate_node_identity" && field_id == "expected_fingerprint"
            ));
        }

        let mut denied = populated_snapshot();
        denied.permissions.permissions.clear();
        assert_eq!(
            action_block_reason(action, &denied, Some("home")),
            Some(ActionBlockReason::PermissionDenied)
        );
    }

    #[test]
    fn policy_test_builder_uses_strict_json_and_reports_field_paths() {
        let manifest = node_ui_manifest();
        let action = manifest
            .pages
            .iter()
            .flat_map(|page| &page.actions)
            .find(|action| action.id == "test_policy")
            .expect("policy test action");
        validate_form_contract(action).expect("policy test form contract");
        let json = policy_test_fixture_json();
        let expected = parse_policy_test_input(&json).expect("valid policy test input");
        let command = build_command(
            action,
            &HashMap::from([("policy_json".into(), json.clone())]),
        )
        .expect("typed policy test command");
        assert!(matches!(
            command,
            NodeCommand::TestPolicy(ref input) if *input == expected
        ));

        let nested_duplicate = json.replacen(
            "\"intent\":\"read_only\"",
            "\"intent\":\"read_only\",\"intent\":\"mutating\"",
            1,
        );
        let error = build_command(
            action,
            &HashMap::from([("policy_json".into(), nested_duplicate)]),
        )
        .expect_err("duplicate fields must be rejected");
        assert!(matches!(
            error,
            CommandBuildError::InvalidField {
                ref action_id,
                ref field_id,
                ref value,
            } if action_id == "test_policy"
                && field_id == "policy_json"
                && value.contains("/request/intent")
        ));
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
    fn exit_node_capability_blocks_exit_data_plane_actions() {
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
        let local_exit_action = exit_page
            .actions
            .iter()
            .find(|action| action.id == "set_local_exit_capability")
            .expect("local exit action")
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
            action_block_reason(&local_exit_action, &unsupported, Some("home")),
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
        assert_eq!(
            action_block_reason(&local_exit_action, &supported, Some("home")),
            None
        );
    }

    #[test]
    fn tun_capability_blocks_rebuild_and_projects_every_reason() {
        let manifest = node_ui_manifest();
        let rebuild = manifest
            .pages
            .iter()
            .find(|page| page.id == "networks")
            .expect("networks page")
            .actions
            .iter()
            .find(|action| action.id == "rebuild_tun")
            .expect("rebuild action")
            .clone();
        let runtime_page = manifest
            .pages
            .iter()
            .find(|page| page.id == "node_runtime")
            .expect("runtime page");
        let mut snapshot = populated_snapshot();
        snapshot.tun_capabilities = crate::tun::TunCapabilities {
            schema_version: crate::tun::TUN_CAPABILITY_SCHEMA_VERSION,
            platform: crate::tun::TunPlatform::Linux,
            supported: true,
            can_rebuild: false,
            reasons: vec![
                crate::tun::TunCapabilityReason::PermissionDenied,
                crate::tun::TunCapabilityReason::DeviceUnavailable,
            ],
        };

        assert_eq!(
            action_block_reason(&rebuild, &snapshot, Some("home")),
            Some(ActionBlockReason::TunUnavailable(
                crate::tun::TunCapabilityReason::PermissionDenied
            ))
        );
        let model = build_page_model_for_page(runtime_page, &snapshot, Locale::ZhCn)
            .expect("runtime model");
        let tun = model
            .sections
            .iter()
            .find(|section| section.title == t(Locale::ZhCn, "runtime.section.tun"))
            .expect("TUN capability section");
        assert_eq!(tun.rows.len(), 4);
        assert!(tun.rows[0].detail.contains("Linux"));
        assert!(tun.rows[1]
            .detail
            .contains(t(Locale::ZhCn, "common.enabled")));
        assert!(tun.rows[2]
            .detail
            .contains(t(Locale::ZhCn, "common.disabled")));
        assert!(tun.rows[3].detail.contains(t(
            Locale::ZhCn,
            "runtime.value.tun_reason.permission_denied"
        )));
        assert!(tun.rows[3].detail.contains(t(
            Locale::ZhCn,
            "runtime.value.tun_reason.device_unavailable"
        )));
    }

    #[test]
    fn rebuildable_tun_capability_does_not_block_rebuild() {
        let manifest = node_ui_manifest();
        let rebuild = manifest
            .pages
            .iter()
            .find(|page| page.id == "networks")
            .expect("networks page")
            .actions
            .iter()
            .find(|action| action.id == "rebuild_tun")
            .expect("rebuild action")
            .clone();
        let mut snapshot = populated_snapshot();
        snapshot.tun_capabilities = crate::tun::TunCapabilities {
            schema_version: crate::tun::TUN_CAPABILITY_SCHEMA_VERSION,
            platform: crate::tun::TunPlatform::Linux,
            supported: true,
            can_rebuild: true,
            reasons: Vec::new(),
        };

        assert_eq!(action_block_reason(&rebuild, &snapshot, Some("home")), None);
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
        let platform = model
            .sections
            .iter()
            .find(|section| section.title == t(Locale::ZhCn, "gpui.section.platform_capabilities"))
            .expect("platform capability section");
        let limits = model
            .sections
            .iter()
            .find(|section| section.title == t(Locale::ZhCn, "gpui.section.capability_limits"))
            .expect("capability limits section");
        assert!(platform.rows[0].detail.contains("Windows"));
        assert!(platform.rows[0]
            .detail
            .contains(t(Locale::ZhCn, "common.disabled")));
        assert!(limits.rows[0].detail.contains("PlatformNotLinux"));
        assert!(limits.rows[0].detail.contains(t(
            Locale::ZhCn,
            "exit_node.unsupported_reason.platform_not_linux"
        )));
    }

    #[test]
    fn exit_node_page_projects_authoritative_local_exit_lifecycle() {
        let manifest = node_ui_manifest();
        let page = manifest
            .pages
            .iter()
            .find(|page| page.id == "exit_node")
            .expect("exit node page");
        let mut snapshot = populated_snapshot();
        snapshot.local_exit.owner_exit_enabled = true;
        snapshot.local_exit.network_allowed = true;
        snapshot.local_exit.active_clients = 3;
        snapshot.local_exit.total_clients = 9;
        snapshot.local_exit.last_transition_at_unix_ms = 1_700_000_000_123;

        let stages = [
            (
                LocalExitCapabilityStage::Disabled,
                "gpui.local_exit.stage.disabled",
            ),
            (
                LocalExitCapabilityStage::WaitingForNetworkAuthorization,
                "gpui.local_exit.stage.waiting_for_network_authorization",
            ),
            (
                LocalExitCapabilityStage::Listening,
                "gpui.local_exit.stage.listening",
            ),
            (
                LocalExitCapabilityStage::HealthChecking,
                "gpui.local_exit.stage.health_checking",
            ),
            (
                LocalExitCapabilityStage::Enabled,
                "gpui.local_exit.stage.enabled",
            ),
            (
                LocalExitCapabilityStage::Draining,
                "gpui.local_exit.stage.draining",
            ),
            (
                LocalExitCapabilityStage::Cleaning,
                "gpui.local_exit.stage.cleaning",
            ),
            (
                LocalExitCapabilityStage::Failed,
                "gpui.local_exit.stage.failed",
            ),
        ];
        for (stage, key) in stages {
            snapshot.local_exit.stage = stage;
            let model =
                build_page_model_for_page(page, &snapshot, Locale::ZhCn).expect("exit node model");
            let local_exit = model
                .sections
                .iter()
                .find(|section| section.title == t(Locale::ZhCn, "gpui.section.local_exit_status"))
                .expect("local Exit status section");
            let stage_row = local_exit
                .rows
                .iter()
                .find(|row| row.title == t(Locale::ZhCn, "gpui.local_exit.stage"))
                .expect("local Exit stage row");
            assert_eq!(stage_row.detail, t(Locale::ZhCn, key));
        }

        snapshot.local_exit.stage = LocalExitCapabilityStage::Cleaning;
        snapshot.local_exit.healthy = false;
        let model =
            build_page_model_for_page(page, &snapshot, Locale::ZhCn).expect("exit node model");
        let local_exit = model
            .sections
            .iter()
            .find(|section| section.title == t(Locale::ZhCn, "gpui.section.local_exit_status"))
            .expect("local Exit status section");
        let row_detail = |title_key| {
            local_exit
                .rows
                .iter()
                .find(|row| row.title == t(Locale::ZhCn, title_key))
                .map(|row| row.detail.as_str())
                .expect("local Exit row")
        };
        assert_eq!(
            row_detail("gpui.local_exit.owner_intent"),
            t(Locale::ZhCn, "common.enabled")
        );
        assert_eq!(
            row_detail("gpui.local_exit.network_authorization"),
            t(Locale::ZhCn, "common.enabled")
        );
        assert_eq!(
            row_detail("gpui.local_exit.health"),
            t(Locale::ZhCn, "gpui.local_exit.unhealthy")
        );
        assert_eq!(
            row_detail("gpui.local_exit.clients"),
            tr(
                Locale::ZhCn,
                "gpui.local_exit.clients_detail",
                &[("active_clients", "3"), ("total_clients", "9")],
            )
        );
        assert_eq!(
            row_detail("gpui.local_exit.last_transition"),
            routing_timestamp_label(Some(1_700_000_000_123), Locale::ZhCn)
        );
        assert_eq!(
            model
                .metrics
                .iter()
                .find(|metric| {
                    metric.label == t(Locale::ZhCn, "gpui.metric.exit_active_clients")
                })
                .expect("active Exit client metric")
                .value,
            "3"
        );

        snapshot.local_exit.stage = LocalExitCapabilityStage::Failed;
        snapshot.local_exit.problem = Some(
            crate::node::NodeProblem::new(
                NodeProblemCode::Unavailable,
                "raw forwarding resource failure",
            )
            .with_message_key("exit.local.problem.resource_insufficient")
            .retryable(true),
        );
        let model =
            build_page_model_for_page(page, &snapshot, Locale::ZhCn).expect("exit node model");
        let problem = model
            .sections
            .iter()
            .find(|section| section.title == t(Locale::ZhCn, "gpui.section.local_exit_status"))
            .expect("local Exit status section")
            .rows
            .iter()
            .find(|row| row.title == t(Locale::ZhCn, "gpui.local_exit.problem"))
            .expect("local Exit problem row");
        assert!(problem.detail.contains("unavailable"));
        assert!(problem
            .detail
            .contains("exit.local.problem.resource_insufficient"));
        assert!(problem.detail.contains("raw forwarding resource failure"));
        assert!(problem.detail.contains(t(Locale::ZhCn, "common.enabled")));
    }

    #[test]
    fn exit_node_page_projects_authoritative_selection_set_and_evidence() {
        let manifest = node_ui_manifest();
        let page = manifest
            .pages
            .iter()
            .find(|page| page.id == "exit_node")
            .expect("exit node page");
        let mut snapshot = populated_snapshot();
        let subject = NodeId::from_public_key(b"gpui-selection-subject");
        let exit_a = NodeId::from_public_key(b"gpui-selection-exit-a");
        let exit_b = NodeId::from_public_key(b"gpui-selection-exit-b");
        let policy = ExitSelectionPolicy {
            mode: ExitSelectionMode::Fixed {
                exit: exit_a.clone(),
            },
            default_exit: Some(exit_a.clone()),
            available_exits: vec![exit_a.clone(), exit_b.clone()],
            ..ExitSelectionPolicy::default()
        };
        let observed_at = 1_700_000_000_000;
        let resolution = policy
            .resolve(
                12,
                &subject,
                &BTreeMap::from([
                    (
                        exit_a.clone(),
                        ExitContributionAvailability::observed(
                            true,
                            true,
                            ExitAvailabilitySource::CoordinatorHealth,
                            observed_at,
                        ),
                    ),
                    (
                        exit_b.clone(),
                        ExitContributionAvailability::observed(
                            true,
                            true,
                            ExitAvailabilitySource::CoordinatorHealth,
                            observed_at,
                        ),
                    ),
                ]),
            )
            .expect("selection resolution");
        snapshot.exit_selections = vec![NodeExitSelectionSnapshot {
            network_id: fuckipv6_common::domain::NetworkId::from_public_key(
                b"gpui-selection-network",
            ),
            policy_revision: 12,
            policy: Some(policy),
            resolution: Some(resolution),
            directory_generated_at_unix_ms: Some(observed_at),
            directory_received_at_unix_ms: Some(observed_at + 1),
            evaluated_at_unix_ms: observed_at + 2,
            reason_code: None,
        }];

        let model =
            build_page_model_for_page(page, &snapshot, Locale::ZhCn).expect("exit node model");
        let governance = model
            .sections
            .iter()
            .find(|section| section.title == t(Locale::ZhCn, "gpui.section.exit_governance"))
            .expect("Exit governance section");
        let summary = governance.rows.first().expect("governance summary");
        assert!(summary
            .detail
            .contains(t(Locale::ZhCn, "exit_governance.mode.fixed")));
        assert!(summary.detail.contains(exit_a.as_str()));
        assert!(!summary.detail.contains(exit_b.as_str()));
        assert!(summary.detail.contains("12"));

        let blocked_title = tr(
            Locale::ZhCn,
            "exit_governance.candidate_title",
            &[
                ("network", snapshot.exit_selections[0].network_id.as_str()),
                ("exit", exit_b.as_str()),
            ],
        );
        let blocked = governance
            .rows
            .iter()
            .find(|row| row.title == blocked_title)
            .expect("blocked candidate row");
        assert!(blocked
            .detail
            .contains(t(Locale::ZhCn, "exit_governance.blocked")));
        assert!(blocked.detail.contains(t(
            Locale::ZhCn,
            "exit_governance.reason.fixed_exit_required"
        )));
        assert!(blocked
            .detail
            .contains(t(Locale::ZhCn, "exit_governance.source.selection_mode")));
        assert!(blocked
            .detail
            .contains("exit.selection.fixed_exit_required"));

        snapshot.exit_selections[0].resolution = None;
        snapshot.exit_selections[0].reason_code = Some("exit.selection.stale_directory".into());
        let model =
            build_page_model_for_page(page, &snapshot, Locale::ZhCn).expect("exit node model");
        let governance = model
            .sections
            .iter()
            .find(|section| section.title == t(Locale::ZhCn, "gpui.section.exit_governance"))
            .expect("Exit governance section");
        let unavailable = governance
            .rows
            .iter()
            .find(|row| row.title == t(Locale::ZhCn, "exit_governance.top_reason"))
            .expect("unavailable reason row");
        assert!(unavailable
            .detail
            .contains(t(Locale::ZhCn, "exit_governance.reason.stale_directory")));
        assert!(unavailable
            .detail
            .contains("exit.selection.stale_directory"));
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
        let snapshot = populated_snapshot();
        let mut values = default_form_values(&action, &snapshot, Some("home"));
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

    fn topology_node_network(
        network_id: &NetworkId,
        role: MembershipRole,
        status: NetworkMemberStatus,
        presence: NetworkMemberPresence,
    ) -> TopologyNodeNetwork {
        TopologyNodeNetwork {
            network_id: network_id.clone(),
            role,
            status,
            presence,
            health: TopologyNodeHealth::Healthy,
            capabilities: vec![CapabilityKind::File],
        }
    }

    fn topology_snapshot_fixture() -> (TopologySnapshot, NetworkId, NetworkId, NodeId) {
        let alpha = NetworkId::from_public_key(b"topology-alpha");
        let beta = NetworkId::from_public_key(b"topology-beta");
        let relay = NodeId::from_public_key(b"topology-relay");
        let viewer = NodeId::from_public_key(b"topology-viewer");
        let snapshot = TopologySnapshot {
            revision: 9,
            generated_at_unix_ms: 1_700_000_010_000,
            viewer: viewer.clone(),
            truncated: false,
            boundaries: vec![
                TopologyBoundary {
                    network_id: alpha.clone(),
                    name: "alpha".into(),
                    member_count: 2,
                    visible_member_count: 2,
                    viewer_role: Some(MembershipRole::Member),
                    capabilities: vec![CapabilityKind::File],
                },
                TopologyBoundary {
                    network_id: beta.clone(),
                    name: "beta".into(),
                    member_count: 2,
                    visible_member_count: 2,
                    viewer_role: Some(MembershipRole::Owner),
                    capabilities: vec![CapabilityKind::Coordinator],
                },
            ],
            nodes: vec![
                TopologyNode {
                    node_id: relay.clone(),
                    display_name: Some("  Trusted relay  ".into()),
                    is_self: false,
                    health: TopologyNodeHealth::Degraded,
                    capabilities: vec![CapabilityKind::Exit],
                    networks: vec![
                        topology_node_network(
                            &alpha,
                            MembershipRole::Member,
                            NetworkMemberStatus::Active,
                            NetworkMemberPresence::live(1_700_000_009_000),
                        ),
                        topology_node_network(
                            &beta,
                            MembershipRole::Administrator,
                            NetworkMemberStatus::Approved,
                            NetworkMemberPresence::disconnected(1_700_000_009_100),
                        ),
                    ],
                },
                TopologyNode {
                    node_id: viewer.clone(),
                    display_name: None,
                    is_self: true,
                    health: TopologyNodeHealth::Healthy,
                    capabilities: Vec::new(),
                    networks: vec![topology_node_network(
                        &alpha,
                        MembershipRole::Member,
                        NetworkMemberStatus::Active,
                        NetworkMemberPresence::live(1_700_000_009_200),
                    )],
                },
            ],
            connections: vec![
                TopologyConnection {
                    connection_id: ContentHash::from_bytes(b"active"),
                    from: relay.clone(),
                    to: viewer.clone(),
                    networks: vec![alpha.clone()],
                    state: TopologyConnectionState::Active,
                    observed_at_unix_ms: Some(1_700_000_009_000),
                },
                TopologyConnection {
                    connection_id: ContentHash::from_bytes(b"idle"),
                    from: relay.clone(),
                    to: viewer.clone(),
                    networks: vec![alpha.clone()],
                    state: TopologyConnectionState::Idle,
                    observed_at_unix_ms: Some(1_700_000_009_000),
                },
                TopologyConnection {
                    connection_id: ContentHash::from_bytes(b"stale"),
                    from: relay.clone(),
                    to: viewer.clone(),
                    networks: vec![beta.clone()],
                    state: TopologyConnectionState::Stale,
                    observed_at_unix_ms: None,
                },
            ],
            clusters: Vec::new(),
        };
        (snapshot, alpha, beta, relay)
    }

    fn topology_directory_member(
        network_id: &NetworkId,
        node_id: &NodeId,
        status: NetworkMemberStatus,
        presence: NetworkMemberPresence,
        available_actions: Vec<MemberLifecycleAction>,
    ) -> NetworkMemberDirectory {
        NetworkMemberDirectory {
            network_id: network_id.clone(),
            generated_at_unix_ms: 1_700_000_010_000,
            viewer_role: MembershipRole::Administrator,
            can_manage_members: true,
            exit_selection_policy: Default::default(),
            exit_selection_policy_revision: 0,
            members: vec![NetworkMember {
                node_id: node_id.clone(),
                display_name: Some("Directory relay".into()),
                identity_fingerprint: None,
                role: MembershipRole::Administrator,
                groups: vec![NetworkMemberGroup::Administrator],
                capabilities: None,
                status,
                lifecycle_state: lifecycle_state_for_status(status),
                revision: 17,
                access_epoch: 3,
                available_actions,
                presence,
                exit_availability: Default::default(),
                joined_at_unix_ms: Some(1_700_000_000_000),
            }],
        }
    }

    #[test]
    fn topology_projection_uses_only_the_authoritative_snapshot() {
        let (topology, _, _, relay) = topology_snapshot_fixture();
        let mut snapshot = populated_snapshot();
        snapshot.networks[0].name = "legacy-network".into();
        snapshot.peers[0].peer_id = "legacy-peer".into();
        snapshot.peers[0].network_name = "legacy-network".into();
        snapshot.topology = Some(topology);

        let (metrics, sections) = project_topology(&snapshot, Locale::En);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].source, "TopologySnapshot");
        assert_eq!(metrics[0].value, "1");
        assert_eq!(sections.len(), 2);
        let rendered = sections
            .iter()
            .flat_map(|section| {
                section
                    .rows
                    .iter()
                    .flat_map(|row| [row.title.as_str(), row.detail.as_str()])
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("Trusted relay"));
        assert!(rendered.contains(relay.as_str()));
        assert!(!rendered.contains("legacy-peer"));
        assert!(!rendered.contains("legacy-network"));
        assert!(!rendered.contains("fd00::3"));
    }

    #[test]
    fn topology_presentation_matches_webui_priority_including_stale() {
        let (topology, _, _, _) = topology_snapshot_fixture();
        let all = topology_presentation(
            Some(&topology),
            TopologyPresentationSignals {
                initial_loading: true,
                refreshing: true,
                offline: true,
                stale: true,
                error: true,
                permission_denied: true,
            },
        );
        assert_eq!(all.kind, TopologyPresentationCondition::Error);
        assert_eq!(
            all.conditions,
            vec![
                TopologyPresentationCondition::Error,
                TopologyPresentationCondition::PermissionDenied,
                TopologyPresentationCondition::InitialLoading,
                TopologyPresentationCondition::Offline,
                TopologyPresentationCondition::Stale,
                TopologyPresentationCondition::Refreshing,
            ]
        );
        assert!(all.has_content);
        assert!(all.can_render);

        let denied = topology_presentation(
            Some(&topology),
            TopologyPresentationSignals {
                permission_denied: true,
                ..TopologyPresentationSignals::default()
            },
        );
        assert_eq!(denied.kind, TopologyPresentationCondition::PermissionDenied);
        assert!(!denied.can_render);

        let unavailable = topology_presentation(None, TopologyPresentationSignals::default());
        assert_eq!(unavailable.kind, TopologyPresentationCondition::Unavailable);
        assert!(!unavailable.has_content);
        assert!(!unavailable.can_render);

        let empty = topology_presentation(
            Some(&TopologySnapshot {
                boundaries: Vec::new(),
                ..topology.clone()
            }),
            TopologyPresentationSignals::default(),
        );
        assert_eq!(empty.kind, TopologyPresentationCondition::Empty);

        let partial = topology_presentation(
            Some(&TopologySnapshot {
                truncated: true,
                ..topology.clone()
            }),
            TopologyPresentationSignals::default(),
        );
        assert_eq!(partial.kind, TopologyPresentationCondition::Partial);
        assert!(partial.can_render);

        for (signals, expected) in [
            (
                TopologyPresentationSignals {
                    refreshing: true,
                    ..TopologyPresentationSignals::default()
                },
                TopologyPresentationCondition::Refreshing,
            ),
            (
                TopologyPresentationSignals {
                    offline: true,
                    ..TopologyPresentationSignals::default()
                },
                TopologyPresentationCondition::Offline,
            ),
            (
                TopologyPresentationSignals {
                    stale: true,
                    ..TopologyPresentationSignals::default()
                },
                TopologyPresentationCondition::Stale,
            ),
        ] {
            assert_eq!(
                topology_presentation(Some(&topology), signals).kind,
                expected
            );
        }
        assert_eq!(
            topology_presentation(Some(&topology), TopologyPresentationSignals::default()).kind,
            TopologyPresentationCondition::Ready
        );
    }

    #[test]
    fn topology_detail_uses_membership_then_directory_then_unknown() {
        let (topology, alpha, beta, relay) = topology_snapshot_fixture();
        let node = topology.node(&relay).expect("relay node");
        let mut directory = topology_directory_member(
            &beta,
            &relay,
            NetworkMemberStatus::Suspended,
            NetworkMemberPresence::disconnected(1_700_000_009_900),
            vec![MemberLifecycleAction::Resume],
        );
        directory.members[0].exit_availability = ExitContributionAvailability::observed(
            true,
            true,
            ExitAvailabilitySource::CoordinatorHealth,
            1_700_000_009_000,
        );
        let detail = topology_node_detail(
            node,
            Some(beta.as_str()),
            Some(&directory.members[0]),
            Locale::En,
            directory.generated_at_unix_ms,
        );
        assert_eq!(detail.display_name, "Trusted relay");
        assert_eq!(
            detail.role,
            t(Locale::En, "join_request.role.administrator")
        );
        assert_eq!(
            detail.status,
            t(Locale::En, "member_directory.status.approved")
        );
        assert_eq!(
            detail.presence,
            t(Locale::En, "member_directory.presence.offline")
        );
        assert!(detail.networks.contains(alpha.as_str()));
        assert!(detail.networks.contains(beta.as_str()));
        assert!(detail
            .capabilities
            .contains(t(Locale::En, "capability.exit")));
        assert_eq!(
            detail.exit_state,
            t(Locale::En, "member_directory.exit.state.available")
        );
        assert_eq!(
            detail.exit_source,
            t(
                Locale::En,
                "member_directory.exit.source.coordinator_health"
            )
        );
        assert_eq!(
            detail.exit_declared,
            t(Locale::En, "member_directory.exit.declared.available")
        );
        assert_eq!(detail.exit_observed_at_unix_ms, Some(1_700_000_009_000));
        assert_eq!(
            detail.exit_freshness,
            t(Locale::En, "member_directory.freshness.just_now")
        );

        let fallback = topology_node_detail(
            node,
            Some(NetworkId::from_public_key(b"foreign-network").as_str()),
            Some(&directory.members[0]),
            Locale::En,
            directory.generated_at_unix_ms,
        );
        assert_eq!(fallback.role, t(Locale::En, "common.not_available"));
        assert_eq!(
            fallback.status,
            t(Locale::En, "member_directory.status.suspended")
        );
        assert_eq!(
            fallback.presence,
            t(Locale::En, "member_directory.presence.offline")
        );

        let unknown =
            topology_node_detail(node, None, None, Locale::En, topology.generated_at_unix_ms);
        assert_eq!(unknown.status, t(Locale::En, "common.unknown"));
        assert_eq!(unknown.presence, t(Locale::En, "common.unknown"));
        assert_eq!(
            unknown.exit_state,
            t(Locale::En, "member_directory.exit.state.unknown")
        );
        assert_eq!(
            unknown.exit_source,
            t(Locale::En, "member_directory.exit.source.not_observed")
        );
        assert_eq!(unknown.exit_observed_at_unix_ms, None);
        assert_eq!(
            unknown.exit_freshness,
            t(Locale::En, "member_directory.exit.freshness.not_observed")
        );

        let viewer = topology
            .node_ids()
            .into_iter()
            .find(|node_id| node_id != &relay)
            .expect("viewer node");
        let viewer = topology.node(&viewer).expect("viewer node");
        let viewer_detail = topology_node_detail(
            viewer,
            Some(alpha.as_str()),
            None,
            Locale::En,
            topology.generated_at_unix_ms,
        );
        assert_eq!(viewer_detail.display_name, viewer.node_id.as_str());
        assert_eq!(
            viewer_detail.capabilities,
            t(Locale::En, "status.capability.unknown")
        );
    }

    #[test]
    fn member_exit_availability_fails_closed_and_filters_reason_codes() {
        let now = 1_700_000_010_000;
        let missing = normalize_member_exit_availability(None, now);
        assert_eq!(missing.state, ExitAvailabilityState::Unknown);
        assert_eq!(missing.source, ExitAvailabilitySource::NotObserved);
        assert_eq!(missing.freshness, MemberExitFreshness::NotObserved);
        assert_eq!(missing.reason_code, Some("exit.availability.not_observed"));

        let undeclared = ExitContributionAvailability::observed_with_declaration(
            None,
            true,
            ExitAvailabilitySource::LocalRuntime,
            now - 1_000,
        );
        let normalized = normalize_member_exit_availability(Some(&undeclared), now);
        assert_eq!(normalized.state, ExitAvailabilityState::Available);
        assert_eq!(normalized.declared_available, None);
        assert_eq!(normalized.freshness, MemberExitFreshness::Fresh);
        assert_eq!(
            member_exit_declared_text(Locale::En, Some(&undeclared), now),
            t(Locale::En, "member_directory.exit.declared.unknown")
        );

        let contradictory = ExitContributionAvailability {
            state: ExitAvailabilityState::Available,
            source: ExitAvailabilitySource::RemotePeerReport,
            observed_at_unix_ms: Some(now - 1_000),
            declared_available: Some(false),
            reason_code: Some("exit.availability.private_reason".into()),
        };
        let normalized = normalize_member_exit_availability(Some(&contradictory), now);
        assert_eq!(normalized.state, ExitAvailabilityState::Degraded);
        assert_eq!(
            normalized.reason_code,
            Some("exit.availability.available_but_not_declared")
        );
        let reason = member_exit_reason_text(Locale::En, Some(&contradictory), now);
        assert_eq!(
            reason,
            t(
                Locale::En,
                "member_directory.exit.reason.available_but_not_declared"
            )
        );
        assert!(!reason.contains("private_reason"));

        let stale = ExitContributionAvailability::observed(
            true,
            true,
            ExitAvailabilitySource::CoordinatorHealth,
            now - MEMBER_EXIT_AVAILABILITY_MAX_AGE_MS - 1,
        );
        let normalized = normalize_member_exit_availability(Some(&stale), now);
        assert_eq!(normalized.state, ExitAvailabilityState::Unknown);
        assert_eq!(normalized.freshness, MemberExitFreshness::Stale);
        assert_eq!(normalized.reason_code, Some("exit.availability.stale"));
        assert_eq!(
            member_exit_freshness_text(Locale::En, Some(&stale), now),
            t(Locale::En, "member_directory.exit.freshness.stale")
        );

        let missing_timestamp = ExitContributionAvailability {
            state: ExitAvailabilityState::Available,
            source: ExitAvailabilitySource::LocalRuntime,
            observed_at_unix_ms: None,
            declared_available: Some(true),
            reason_code: Some("exit.availability.forwarder_unhealthy".into()),
        };
        let normalized = normalize_member_exit_availability(Some(&missing_timestamp), now);
        assert_eq!(normalized.state, ExitAvailabilityState::Unknown);
        assert_eq!(normalized.freshness, MemberExitFreshness::Stale);

        let unknown_reason = ExitContributionAvailability {
            state: ExitAvailabilityState::Unavailable,
            source: ExitAvailabilitySource::LocalRuntime,
            observed_at_unix_ms: Some(now - 1_000),
            declared_available: Some(false),
            reason_code: Some("exit.availability.private_reason".into()),
        };
        let reason = member_exit_reason_text(Locale::En, Some(&unknown_reason), now);
        assert_eq!(reason, t(Locale::En, "common.unknown"));
        assert!(!reason.contains("private_reason"));
    }

    #[test]
    fn topology_member_actions_require_a_matching_manageable_directory() {
        let (_, alpha, beta, relay) = topology_snapshot_fixture();
        let actions = vec![
            MemberLifecycleAction::Invite,
            MemberLifecycleAction::Approve,
            MemberLifecycleAction::Exit,
            MemberLifecycleAction::Suspend,
            MemberLifecycleAction::Remove,
        ];
        let directory = topology_directory_member(
            &alpha,
            &relay,
            NetworkMemberStatus::Active,
            NetworkMemberPresence::live(1_700_000_009_000),
            actions,
        );
        assert_eq!(
            topology_member_actions(Some(&directory), Some(alpha.as_str()), relay.as_str()),
            vec![
                MemberLifecycleAction::Approve,
                MemberLifecycleAction::Suspend,
                MemberLifecycleAction::Remove,
            ]
        );
        assert!(
            topology_member_actions(Some(&directory), Some(beta.as_str()), relay.as_str())
                .is_empty()
        );
        assert!(topology_member_actions(
            Some(&directory),
            Some(alpha.as_str()),
            NetworkId::from_public_key(b"missing-node").as_str()
        )
        .is_empty());

        let mut non_manager = directory.clone();
        non_manager.can_manage_members = false;
        assert!(
            topology_member_actions(Some(&non_manager), Some(alpha.as_str()), relay.as_str())
                .is_empty()
        );

        let mut member_view = directory.clone();
        member_view.viewer_role = MembershipRole::Member;
        assert_eq!(
            topology_member_actions(Some(&member_view), Some(alpha.as_str()), relay.as_str()),
            vec![
                MemberLifecycleAction::Approve,
                MemberLifecycleAction::Suspend,
                MemberLifecycleAction::Remove,
            ]
        );
        assert!(topology_member_actions(None, Some(alpha.as_str()), relay.as_str()).is_empty());
    }

    #[test]
    fn topology_lifecycle_wire_values_do_not_depend_on_localized_copy() {
        assert_eq!(
            member_lifecycle_action_value(MemberLifecycleAction::UndoRemove),
            "undo_remove"
        );
        assert_eq!(
            membership_role_value(MembershipRole::Administrator),
            "administrator"
        );
        assert_ne!(
            member_lifecycle_action_value(MemberLifecycleAction::Remove),
            t(
                Locale::En,
                member_lifecycle_action_key(MemberLifecycleAction::Remove)
            )
        );
    }

    #[test]
    fn invitation_role_parser_accepts_owner_and_keeps_existing_aliases() {
        assert_eq!(
            parse_invitation_role("create_invitation", "owner").expect("owner invitation role"),
            MembershipRole::Owner
        );
        assert_eq!(
            parse_invitation_role("create_invitation", "admin").expect("admin alias"),
            MembershipRole::Administrator
        );
        assert_eq!(
            parse_invitation_role("create_invitation", "member").expect("member role"),
            MembershipRole::Member
        );
        assert!(parse_invitation_role("create_invitation", "superuser").is_err());
    }
}
