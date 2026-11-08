use crate::config_transaction::{
    ConfigurationApplyResult, ConfigurationDocument, ConfigurationHistoryEntry,
    ConfigurationPreview,
};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use fuckipv6_common::domain::{
    default_network_icon, default_network_language, default_network_purpose,
    default_network_region, ActivityEntry, AuditEntry, AuditOutcome, CapabilityKind, ContentHash,
    ExitContributionObligation, ExitContributionScope, InvitationCode, InvitationCreateRequest,
    InvitationId, InvitationPreview, InvitationRedeemRequest, InvitationRedemption,
    InvitationStatusView, IssuedInvitation, JoinRequest, JoinRequestReviewAction, JoinRequestState,
    MemberLifecycleAction, MemberLifecycleChange, MemberRemovalPreview, MembershipRole,
    NetworkCreateProfile, NetworkDiscoveryRecord, NetworkEndpoint, NetworkId, NetworkJoinMode,
    NetworkMemberDirectory, NetworkProfileApplyResult, NetworkProfilePreview, NetworkProfileUpdate,
    NetworkVisibility, PermissionDenialCode, PolicyTestInput, PolicyTestResult,
    JOIN_REQUEST_MAX_INFORMATION_REQUEST_CHARS, JOIN_REQUEST_MAX_REASON_CHARS,
    NETWORK_PROFILE_MAX_TAG_CHARS,
};
#[cfg(test)]
use fuckipv6_common::domain::{DEFAULT_NETWORK_ICON, DEFAULT_NETWORK_LANGUAGE};
use fuckipv6_common::{validate_ipv4_cidr, validate_ipv6_cidr, NetworkAllocation, NetworkMode};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, oneshot, watch};

use super::authorization::{authorize_command, CommandDenial};
use super::ui_manifest::audit_for_command;
use super::{
    NodeCommandContext, NodeEventBus, NodeEventKind, NodeLifecycleSnapshot, NodeNetworkRole,
    NodeNetworkSelection, NodeProblem, NodeProblemCode, NodeRoutingMode, NodeSnapshot,
    NodeStateStore, NodeUiManifest, NodeValidationIssue,
};
use crate::owner_security::audit::OwnerSecurityAudit;
use crate::policy_transaction::{
    PolicyApplyResult, PolicyCurrent, PolicyHistorySnapshot, PolicyPreview, PolicyRollbackResult,
    PolicySnapshot,
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
    UpdateRouting,
    SetNetworkRole,
    SetNodeDisplayName,
    RotateNodeIdentity,
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
    GetPolicy,
    PreviewPolicy,
    PreviewPolicyRollback,
    ApplyPolicy,
    RollbackPolicy,
    GetPolicyHistory,
    TestPolicy,
    Shutdown,
}

impl NodeCommandKind {
    pub const ALL: [Self; 61] = [
        Self::GetSnapshot,
        Self::GetUiManifest,
        Self::GetDiagnostics,
        Self::StartNetworks,
        Self::StopNetworks,
        Self::RestartNetworks,
        Self::RebuildTun,
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
        Self::UpdateRouting,
        Self::SetNetworkRole,
        Self::SetNodeDisplayName,
        Self::RotateNodeIdentity,
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
        Self::GetPolicy,
        Self::PreviewPolicy,
        Self::PreviewPolicyRollback,
        Self::ApplyPolicy,
        Self::RollbackPolicy,
        Self::GetPolicyHistory,
        Self::TestPolicy,
        Self::Shutdown,
    ];

    /// Commands that must be reachable through a user-visible manifest action.
    /// The manifest bootstrap command is deliberately excluded.
    pub const ACTIONS: [Self; 60] = [
        Self::GetSnapshot,
        Self::GetDiagnostics,
        Self::StartNetworks,
        Self::StopNetworks,
        Self::RestartNetworks,
        Self::RebuildTun,
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
        Self::UpdateRouting,
        Self::SetNetworkRole,
        Self::SetNodeDisplayName,
        Self::RotateNodeIdentity,
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
        Self::GetPolicy,
        Self::PreviewPolicy,
        Self::PreviewPolicyRollback,
        Self::ApplyPolicy,
        Self::RollbackPolicy,
        Self::GetPolicyHistory,
        Self::TestPolicy,
        Self::Shutdown,
    ];

    /// Commands whose blast radius or trust-boundary changes require a recent
    /// WebUI Passkey ceremony. Authorization and the UI manifest both consume
    /// this policy so server enforcement cannot drift from the warning shown to
    /// the owner.
    pub const fn requires_recent_reauthentication(self) -> bool {
        matches!(
            self,
            Self::Shutdown
                | Self::ApplyConfiguration
                | Self::RollbackConfiguration
                | Self::ApplyPolicy
                | Self::RollbackPolicy
                | Self::RestartNode
                | Self::RebuildTun
                | Self::SetNodeDisplayName
                | Self::RotateNodeIdentity
                | Self::CleanupResiduals
                | Self::ChangeMemberLifecycle
                | Self::CreateInvitation
                | Self::RevokeInvitation
                | Self::PreviewInvitation
                | Self::RedeemInvitation
        )
    }

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
            Self::UpdateRouting => "update_routing",
            Self::SetNetworkRole => "set_network_role",
            Self::SetNodeDisplayName => "set_node_display_name",
            Self::RotateNodeIdentity => "rotate_node_identity",
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
            Self::GetPolicy => "get_policy",
            Self::PreviewPolicy => "preview_policy",
            Self::PreviewPolicyRollback => "preview_policy_rollback",
            Self::ApplyPolicy => "apply_policy",
            Self::RollbackPolicy => "rollback_policy",
            Self::GetPolicyHistory => "get_policy_history",
            Self::TestPolicy => "test_policy",
        }
    }

    /// Whether this command can change node or network state. Mutation and
    /// host-fix actions remain subject to the authorization and audit policy
    /// attached to `is_write`; diagnostics are read-only but are long-running
    /// operations so they can be cancelled and observed.
    pub const fn is_write(self) -> bool {
        matches!(
            self,
            Self::StartNetworks
                | Self::StopNetworks
                | Self::RestartNetworks
                | Self::RebuildTun
                | Self::CreateNetwork
                | Self::UpdateNetworkProfile
                | Self::PublishNetworkDiscovery
                | Self::UpdateNetworkDiscovery
                | Self::UnpublishNetworkDiscovery
                | Self::RefreshDiscoveryIndex
                | Self::RetryDiscoveryIndex
                | Self::JoinNetwork
                | Self::SubmitJoinRequest
                | Self::ReviewJoinRequest
                | Self::BatchReviewJoinRequests
                | Self::WithdrawJoinRequest
                | Self::ResubmitJoinRequest
                | Self::ChangeMemberLifecycle
                | Self::CreateInvitation
                | Self::RevokeInvitation
                | Self::RedeemInvitation
                | Self::LeaveNetwork
                | Self::AcknowledgeMembershipPolicy
                | Self::UpdateRouting
                | Self::SetNetworkRole
                | Self::SetNodeDisplayName
                | Self::RotateNodeIdentity
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
                | Self::ApplyPolicy
                | Self::RollbackPolicy
        )
    }

    /// Commands that must be submitted through an operation registry. This
    /// deliberately extends mutation semantics only for diagnostics; it does
    /// not grant write authorization or make diagnostics auditable as changes.
    pub const fn is_operation(self) -> bool {
        self.is_write() || matches!(self, Self::GetDiagnostics)
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
    /// Build the exact public-card preview and field-level diff without writing.
    PreviewNetworkProfile(NodeNetworkProfileCommand),
    /// Atomically apply, re-sign and audit a public profile update.
    UpdateNetworkProfile(NodeNetworkProfileCommand),
    /// Sign and publish the current public profile to a discovery index.
    PublishNetworkDiscovery(NodeNetworkDiscoveryCommand),
    /// Sign a newer discovery revision and publish it.
    UpdateNetworkDiscovery(NodeNetworkDiscoveryCommand),
    /// Sign and publish a terminal revocation for a discovery record.
    UnpublishNetworkDiscovery(NodeNetworkDiscoveryCommand),
    /// Fetch and verify the signed discovery index without changing any data plane.
    RefreshDiscoveryIndex {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index_url: Option<String>,
    },
    /// Retry the last configured discovery index after an outage.
    RetryDiscoveryIndex,
    /// Read-only disclosure preview for a signed open-network discovery record.
    PreviewJoinNetwork(NodePreviewJoinNetworkCommand),
    /// Confirm and directly join a network using signed discovery material that
    /// is revalidated against the persisted revision/revocation watermark.
    JoinNetwork(Box<NodeJoinNetworkCommand>),
    /// Submit an owner-authored request to join an approval-required network.
    SubmitJoinRequest(NodeSubmitJoinRequestCommand),
    /// List the caller's own requests or every request when the caller is a
    /// current network owner/administrator.
    ListJoinRequests(NodeListJoinRequestsCommand),
    /// Read the Coordinator-authoritative, privacy-filtered member directory.
    ListNetworkMembers(NodeNetworkMembersCommand),
    /// Disclose the exact access, File and data impact before member removal.
    PreviewMemberRemoval(NodeMemberRemovalPreviewCommand),
    /// Apply one revision-checked member lifecycle governance action.
    ChangeMemberLifecycle(NodeChangeMemberLifecycleCommand),
    /// Apply one revision-checked approval decision.
    ReviewJoinRequest(NodeReviewJoinRequestCommand),
    /// Apply a revision-checked approval batch atomically.
    BatchReviewJoinRequests(NodeBatchReviewJoinRequestsCommand),
    /// Withdraw the caller's own pending request.
    WithdrawJoinRequest(NodeWithdrawJoinRequestCommand),
    /// Answer an information request and return it to pending review.
    ResubmitJoinRequest(NodeResubmitJoinRequestCommand),
    /// Create a one-time authenticated invitation and reveal its credential once.
    CreateInvitation(NodeCreateInvitationCommand),
    /// List redacted invitation metadata for one managed network.
    ListInvitations(NodeListInvitationsCommand),
    /// Read one invitation's redacted status.
    GetInvitationStatus(NodeGetInvitationStatusCommand),
    /// Revoke an invitation using its current revision.
    RevokeInvitation(NodeRevokeInvitationCommand),
    /// Preview an invitation before local-owner reauthentication and consent.
    PreviewInvitation(NodePreviewInvitationCommand),
    /// Consume an invitation and create ordinary network membership.
    RedeemInvitation(NodeRedeemInvitationCommand),
    /// Remove only this network's local configuration and membership.
    LeaveNetwork(NodeLeaveNetworkCommand),
    /// Re-confirm the exact latest terms after an `AllMembers` policy change.
    AcknowledgeMembershipPolicy(NodeAcknowledgeMembershipPolicyCommand),
    UpdateRouting(NodeUpdateRoutingCommand),
    SetNetworkRole(NodeSetNetworkRoleCommand),
    SetNodeDisplayName(NodeSetNodeDisplayNameCommand),
    RotateNodeIdentity(NodeRotateNodeIdentityCommand),
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
    /// Current authoritative policy revision and normalized policy snapshot.
    GetPolicy,
    /// Compute a policy diff and risk summary without writing it.
    PreviewPolicy(NodePreviewPolicyCommand),
    /// Compute the diff and risk for restoring a previous revision without writing it.
    PreviewPolicyRollback(NodePreviewPolicyRollbackCommand),
    /// Atomically replace the complete policy snapshot under one revision.
    ApplyPolicy(NodeApplyPolicyCommand),
    /// Restore a previous policy generation as a new auditable revision.
    RollbackPolicy(NodeRollbackPolicyCommand),
    /// Append-only policy revision history, newest first.
    GetPolicyHistory,
    /// Evaluate candidate policy documents with the runtime authorization evaluator
    /// without storing them or emitting audit/activity side effects.
    TestPolicy(PolicyTestInput),
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
            Self::PreviewNetworkProfile(_) => NodeCommandKind::PreviewNetworkProfile,
            Self::UpdateNetworkProfile(_) => NodeCommandKind::UpdateNetworkProfile,
            Self::PublishNetworkDiscovery(_) => NodeCommandKind::PublishNetworkDiscovery,
            Self::UpdateNetworkDiscovery(_) => NodeCommandKind::UpdateNetworkDiscovery,
            Self::UnpublishNetworkDiscovery(_) => NodeCommandKind::UnpublishNetworkDiscovery,
            Self::RefreshDiscoveryIndex { .. } => NodeCommandKind::RefreshDiscoveryIndex,
            Self::RetryDiscoveryIndex => NodeCommandKind::RetryDiscoveryIndex,
            Self::PreviewJoinNetwork(_) => NodeCommandKind::PreviewJoinNetwork,
            Self::JoinNetwork(_) => NodeCommandKind::JoinNetwork,
            Self::SubmitJoinRequest(_) => NodeCommandKind::SubmitJoinRequest,
            Self::ListJoinRequests(_) => NodeCommandKind::ListJoinRequests,
            Self::ListNetworkMembers(_) => NodeCommandKind::ListNetworkMembers,
            Self::PreviewMemberRemoval(_) => NodeCommandKind::PreviewMemberRemoval,
            Self::ChangeMemberLifecycle(_) => NodeCommandKind::ChangeMemberLifecycle,
            Self::ReviewJoinRequest(_) => NodeCommandKind::ReviewJoinRequest,
            Self::BatchReviewJoinRequests(_) => NodeCommandKind::BatchReviewJoinRequests,
            Self::WithdrawJoinRequest(_) => NodeCommandKind::WithdrawJoinRequest,
            Self::ResubmitJoinRequest(_) => NodeCommandKind::ResubmitJoinRequest,
            Self::CreateInvitation(_) => NodeCommandKind::CreateInvitation,
            Self::ListInvitations(_) => NodeCommandKind::ListInvitations,
            Self::GetInvitationStatus(_) => NodeCommandKind::GetInvitationStatus,
            Self::RevokeInvitation(_) => NodeCommandKind::RevokeInvitation,
            Self::PreviewInvitation(_) => NodeCommandKind::PreviewInvitation,
            Self::RedeemInvitation(_) => NodeCommandKind::RedeemInvitation,
            Self::LeaveNetwork(_) => NodeCommandKind::LeaveNetwork,
            Self::AcknowledgeMembershipPolicy(_) => NodeCommandKind::AcknowledgeMembershipPolicy,
            Self::UpdateRouting(_) => NodeCommandKind::UpdateRouting,
            Self::SetNetworkRole(_) => NodeCommandKind::SetNetworkRole,
            Self::SetNodeDisplayName(_) => NodeCommandKind::SetNodeDisplayName,
            Self::RotateNodeIdentity(_) => NodeCommandKind::RotateNodeIdentity,
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
            Self::GetPolicy => NodeCommandKind::GetPolicy,
            Self::PreviewPolicy(_) => NodeCommandKind::PreviewPolicy,
            Self::PreviewPolicyRollback(_) => NodeCommandKind::PreviewPolicyRollback,
            Self::ApplyPolicy(_) => NodeCommandKind::ApplyPolicy,
            Self::RollbackPolicy(_) => NodeCommandKind::RollbackPolicy,
            Self::GetPolicyHistory => NodeCommandKind::GetPolicyHistory,
            Self::TestPolicy(_) => NodeCommandKind::TestPolicy,
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
            | Self::GetConfigurationHistory
            | Self::GetPolicy
            | Self::GetPolicyHistory
            | Self::TestPolicy(_) => Ok(()),
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
            Self::PreviewNetworkProfile(command) | Self::UpdateNetworkProfile(command) => {
                command.validate()
            }
            Self::PublishNetworkDiscovery(command)
            | Self::UpdateNetworkDiscovery(command)
            | Self::UnpublishNetworkDiscovery(command) => command.validate(),
            Self::RefreshDiscoveryIndex { index_url } => {
                if index_url
                    .as_deref()
                    .is_some_and(|url| url.trim().is_empty())
                {
                    return Err(invalid(
                        "发现索引 URL 不能为空字符串；使用省略值重试已保存的索引",
                    ));
                }
                Ok(())
            }
            Self::RetryDiscoveryIndex => Ok(()),
            Self::PreviewJoinNetwork(command) => command.validate(),
            Self::JoinNetwork(command) => command.validate(),
            Self::SubmitJoinRequest(command) => command.validate(),
            Self::ListJoinRequests(command) => command.validate(),
            Self::ListNetworkMembers(command) => command.validate(),
            Self::PreviewMemberRemoval(command) => command.validate(),
            Self::ChangeMemberLifecycle(command) => command.validate(),
            Self::ReviewJoinRequest(command) => command.validate(),
            Self::BatchReviewJoinRequests(command) => command.validate(),
            Self::WithdrawJoinRequest(command) => command.validate(),
            Self::ResubmitJoinRequest(command) => command.validate(),
            Self::CreateInvitation(command) => command.validate(),
            Self::ListInvitations(command) => command.validate(),
            Self::GetInvitationStatus(command) => command.validate(),
            Self::RevokeInvitation(command) => command.validate(),
            Self::PreviewInvitation(command) => command.validate(),
            Self::RedeemInvitation(command) => command.validate(),
            Self::LeaveNetwork(command) => command.validate(),
            Self::AcknowledgeMembershipPolicy(command) => command.validate(),
            Self::UpdateRouting(command) => command.validate(),
            Self::SetNetworkRole(command) => command.validate(),
            Self::SetNodeDisplayName(command) => command.validate(),
            Self::RotateNodeIdentity(command) => command.validate(),
            Self::GetBlacklist { network_name } | Self::GetNetworkAllocation { network_name } => {
                validate_network_name(network_name)
            }
            Self::BanClient(command) | Self::UnbanClient(command) => command.validate(),
            Self::UpdateNetworkAllocation(command) => command.validate(),
            Self::ProbePeer(command) => command.validate(),
            Self::PreviewConfiguration(command) => command.validate(),
            Self::ApplyConfiguration(command) => command.validate(),
            Self::RollbackConfiguration(command) => command.validate(),
            Self::PreviewPolicy(command) => command.validate(),
            Self::PreviewPolicyRollback(command) => command.validate(),
            Self::ApplyPolicy(command) => command.validate(),
            Self::RollbackPolicy(command) => command.validate(),
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

/// A complete normalized policy snapshot. The server computes the actual
/// affected scope so the caller can review it before supplying the scope to an
/// apply command.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodePreviewPolicyCommand {
    pub expected_revision: u64,
    pub candidate: PolicySnapshot,
}

impl NodePreviewPolicyCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodePreviewPolicyRollbackCommand {
    pub expected_revision: u64,
    pub target_revision: u64,
}

impl NodePreviewPolicyRollbackCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        if self.target_revision == 0 {
            return Err(invalid("回滚目标 revision 必须大于 0"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeApplyPolicyCommand {
    pub expected_revision: u64,
    pub candidate: PolicySnapshot,
    #[serde(default)]
    pub network_ids: Vec<String>,
    #[serde(default)]
    pub includes_local_policy: bool,
    /// Explicit acknowledgement of the high-risk privilege-expansion summary.
    #[serde(default)]
    pub confirm_privilege_expansion: bool,
}

impl NodeApplyPolicyCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        validate_policy_scope(&self.network_ids)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeRollbackPolicyCommand {
    pub expected_revision: u64,
    pub target_revision: u64,
    #[serde(default)]
    pub network_ids: Vec<String>,
    #[serde(default)]
    pub includes_local_policy: bool,
    #[serde(default)]
    pub confirm_privilege_expansion: bool,
}

impl NodeRollbackPolicyCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        if self.target_revision == 0 {
            return Err(invalid("回滚目标 revision 必须大于 0"));
        }
        validate_policy_scope(&self.network_ids)
    }
}

fn validate_policy_scope(network_ids: &[String]) -> Result<(), NodeProblem> {
    if network_ids
        .iter()
        .any(|network_id| network_id.trim().is_empty())
    {
        return Err(invalid("策略影响范围不能包含空网络 ID"));
    }
    if network_ids
        .iter()
        .map(|network_id| network_id.trim())
        .collect::<HashSet<_>>()
        .len()
        != network_ids.len()
    {
        return Err(invalid("策略影响范围不能包含重复网络 ID"));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeCreateNetworkCommand {
    pub name: String,
    #[serde(default = "default_network_purpose")]
    pub purpose: String,
    #[serde(default = "default_network_icon")]
    pub icon: String,
    #[serde(default = "default_network_language")]
    pub language: String,
    #[serde(default = "default_network_region")]
    pub region: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub capability_declarations: Vec<CapabilityKind>,
    #[serde(default)]
    pub visibility: NetworkVisibility,
    #[serde(default)]
    pub join_mode: NetworkJoinMode,
    #[serde(default)]
    pub exit_contribution: ExitContributionObligation,
    #[serde(default)]
    pub exit_contribution_scope: ExitContributionScope,
    pub signaling_port: u16,
    pub relay_port: u16,
    pub external_host: Option<String>,
    pub auto_connect: bool,
}

impl NodeCreateNetworkCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        let mut issues = Vec::new();
        if let Err(problem) = validate_network_name(&self.name) {
            issues.push(NodeValidationIssue {
                path: "name".into(),
                code: "invalid_network_name".into(),
                message: problem.message,
            });
        }
        if self.purpose.is_empty()
            || self.purpose.chars().count() > 512
            || self.purpose.trim() != self.purpose
            || self.purpose.chars().any(char::is_control)
        {
            issues.push(NodeValidationIssue {
                path: "purpose".into(),
                code: "invalid_network_purpose".into(),
                message: "网络简介不能为空、首尾不能有空白、不能包含控制字符且不能超过 512 个字符"
                    .into(),
            });
        }
        if self.icon.is_empty()
            || self.icon.len() > 64
            || self.icon.trim() != self.icon
            || self.icon.chars().any(|character| {
                !(character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || matches!(character, '-' | '_' | '.'))
            })
        {
            issues.push(NodeValidationIssue {
                path: "icon".into(),
                code: "invalid_network_icon".into(),
                message:
                    "网络图标只能包含小写 ASCII 字母、数字、'.'、'-' 和 '_'，且不能超过 64 个字符"
                        .into(),
            });
        }
        if !valid_language_tag(&self.language) {
            issues.push(NodeValidationIssue {
                path: "language".into(),
                code: "invalid_network_language".into(),
                message: "网络语言必须使用类似 zh-CN、en 或 und 的 BCP-47 标签".into(),
            });
        }
        if !valid_region_code(&self.region) {
            issues.push(NodeValidationIssue {
                path: "region".into(),
                code: "invalid_network_region".into(),
                message: "网络地区必须使用两位大写 ISO 3166-1 代码；不披露时使用 ZZ".into(),
            });
        }
        validate_profile_tags(&self.tags, &mut issues);
        validate_profile_capabilities(&self.capability_declarations, &mut issues);
        if (self.exit_contribution == ExitContributionObligation::NotRequired)
            != (self.exit_contribution_scope == ExitContributionScope::NotApplicable)
        {
            issues.push(NodeValidationIssue {
                path: "exit_contribution_scope".into(),
                code: "invalid_exit_contribution_policy".into(),
                message: "Exit 贡献义务与生效范围必须一致".into(),
            });
        }
        if self.signaling_port == 0 {
            issues.push(NodeValidationIssue {
                path: "signaling_port".into(),
                code: "invalid_port".into(),
                message: "信令端口不能为 0".into(),
            });
        }
        if self.relay_port == 0 {
            issues.push(NodeValidationIssue {
                path: "relay_port".into(),
                code: "invalid_port".into(),
                message: "中继端口不能为 0".into(),
            });
        }
        if self.signaling_port == self.relay_port {
            issues.push(NodeValidationIssue {
                path: "relay_port".into(),
                code: "ports_must_differ".into(),
                message: "信令端口和中继端口不能相同".into(),
            });
        }
        if self.external_host.as_deref().is_some_and(|host| {
            host.trim().is_empty()
                || host.trim() != host
                || host.len() > 253
                || host
                    .chars()
                    .any(|character| character.is_control() || character.is_whitespace())
        }) {
            issues.push(NodeValidationIssue {
                path: "external_host".into(),
                code: "invalid_external_host".into(),
                message: "外部主机名不能为空、不能包含空白或控制字符，且不能超过 253 个字符".into(),
            });
        }
        if issues.is_empty() {
            Ok(())
        } else {
            Err(invalid("网络创建参数校验失败").with_validation_issues(issues))
        }
    }

    pub(crate) fn create_profile(&self) -> NetworkCreateProfile {
        NetworkCreateProfile {
            name: self.name.clone(),
            purpose: self.purpose.clone(),
            icon: self.icon.clone(),
            language: self.language.clone(),
            region: self.region.clone(),
            tags: self.tags.clone(),
            capability_declarations: self.capability_declarations.clone(),
            visibility: self.visibility,
            join_mode: self.join_mode,
            exit_contribution: self.exit_contribution,
            exit_contribution_scope: self.exit_contribution_scope,
        }
    }
}

fn valid_region_code(region: &str) -> bool {
    region.len() == 2
        && region
            .chars()
            .all(|character| character.is_ascii_uppercase())
}

fn validate_profile_tags(tags: &[String], issues: &mut Vec<NodeValidationIssue>) {
    let mut seen = HashSet::new();
    for (index, tag) in tags.iter().enumerate() {
        let valid = !tag.is_empty()
            && tag.chars().count() <= NETWORK_PROFILE_MAX_TAG_CHARS
            && tag.trim() == tag
            && !tag.chars().any(|character| {
                character.is_control()
                    || character.is_whitespace()
                    || !(character.is_alphanumeric() || matches!(character, '-' | '_' | '.'))
            });
        if !valid || !seen.insert(tag.to_ascii_lowercase()) {
            issues.push(NodeValidationIssue {
                path: format!("tags[{index}]"),
                code: if valid {
                    "duplicate_network_tag".into()
                } else {
                    "invalid_network_tag".into()
                },
                message: "标签必须唯一、不超过 32 个字符，且只能包含字母、数字、'.'、'-' 和 '_'"
                    .into(),
            });
        }
    }
}

fn validate_profile_capabilities(
    capabilities: &[CapabilityKind],
    issues: &mut Vec<NodeValidationIssue>,
) {
    let mut seen = HashSet::new();
    for (index, capability) in capabilities.iter().enumerate() {
        if !seen.insert(*capability) {
            issues.push(NodeValidationIssue {
                path: format!("capability_declarations[{index}]"),
                code: "duplicate_capability_declaration".into(),
                message: "能力声明不能重复；声明仅表示网络意图，不代表当前节点已启用该能力".into(),
            });
        }
    }
}

fn valid_language_tag(language: &str) -> bool {
    let mut segments = language.split('-');
    let Some(primary) = segments.next() else {
        return false;
    };
    language.len() <= 35
        && (2..=8).contains(&primary.len())
        && primary
            .chars()
            .all(|character| character.is_ascii_alphabetic())
        && segments.all(|segment| {
            (1..=8).contains(&segment.len())
                && segment
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric())
        })
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeNetworkProfileCommand {
    pub network_name: String,
    pub expected_revision: u64,
    pub update: NetworkProfileUpdate,
}

impl NodeNetworkProfileCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        let mut issues = Vec::new();
        validate_network_name(&self.network_name)?;
        if self.expected_revision == 0 {
            issues.push(NodeValidationIssue {
                path: "expected_revision".into(),
                code: "invalid_profile_revision".into(),
                message: "网络资料 revision 必须为正整数".into(),
            });
        }
        if self.update.purpose.is_empty()
            || self.update.purpose.chars().count() > 512
            || self.update.purpose.trim() != self.update.purpose
            || self.update.purpose.chars().any(char::is_control)
        {
            issues.push(NodeValidationIssue {
                path: "update.purpose".into(),
                code: "invalid_network_purpose".into(),
                message: "网络简介不能为空、长度不能超过 512，且首尾不能包含空白或控制字符".into(),
            });
        }
        if self.update.icon.is_empty()
            || self.update.icon.len() > 64
            || self.update.icon.trim() != self.update.icon
            || self.update.icon.chars().any(|character| {
                !(character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || matches!(character, '-' | '_' | '.'))
            })
        {
            issues.push(NodeValidationIssue {
                path: "update.icon".into(),
                code: "invalid_network_icon".into(),
                message: "网络图标必须是小写 ASCII 标识，只能包含字母、数字、'.'、'-' 和 '_'"
                    .into(),
            });
        }
        if !valid_language_tag(&self.update.language) {
            issues.push(NodeValidationIssue {
                path: "update.language".into(),
                code: "invalid_network_language".into(),
                message: "网络语言必须使用类似 zh-CN、en 或 und 的 BCP-47 标签".into(),
            });
        }
        if !valid_region_code(&self.update.region) {
            issues.push(NodeValidationIssue {
                path: "update.region".into(),
                code: "invalid_network_region".into(),
                message: "网络地区必须使用两位大写 ISO 3166-1 代码；不披露时使用 ZZ".into(),
            });
        }
        if self.update.tags.len() > fuckipv6_common::domain::NETWORK_PROFILE_MAX_TAGS {
            issues.push(NodeValidationIssue {
                path: "update.tags".into(),
                code: "too_many_network_tags".into(),
                message: "网络标签最多 12 个".into(),
            });
        }
        validate_profile_tags(&self.update.tags, &mut issues);
        validate_profile_capabilities(&self.update.capability_declarations, &mut issues);
        if issues.is_empty() {
            Ok(())
        } else {
            Err(invalid("网络公开资料校验失败").with_validation_issues(issues))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeNetworkDiscoveryCommand {
    pub network_name: String,
    pub index_url: String,
}

impl NodeNetworkDiscoveryCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        validate_network_name(&self.network_name)?;
        if self.index_url.trim().is_empty() {
            return Err(invalid("发现索引 URL 不能为空").for_network(self.network_name.clone()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodePreviewJoinNetworkCommand {
    pub network_id: String,
}

impl crate::node::NodePreviewJoinNetworkCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        if self.network_id.trim().is_empty() {
            return Err(invalid("预览加入时必须选择网络 ID"));
        }
        if fuckipv6_common::domain::NetworkId::parse(self.network_id.clone()).is_err() {
            return Err(invalid("网络 ID 格式无效"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeJoinNetworkCommand {
    pub target: String,
    pub network_name: Option<String>,
    pub start: bool,
    /// Public signed discovery material selected by the user. It is never
    /// trusted as cached authority: the handler replaces or revalidates it from
    /// the persisted discovery snapshot immediately before mutation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovery_record: Option<NetworkDiscoveryRecord>,
    /// Stable network selected from a discovery preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_id: Option<String>,
    /// Exact discovery digest shown in the preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_policy_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub privacy_policy_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terms_hash: Option<ContentHash>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acknowledged_at_unix_ms: Option<u64>,
    /// Explicit acknowledgement of the network's Exit contribution obligation.
    /// A network declaration never enables this node's local Exit service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_contribution_acknowledged: Option<bool>,
}

impl NodeJoinNetworkCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        if self.target.trim().is_empty() && self.network_id.is_none() {
            return Err(invalid("加入网络的目标地址、票据或网络 ID 不能为空"));
        }
        if let Some(name) = self.network_name.as_deref() {
            validate_network_name(name)?;
        }
        if let (Some(name), Some(record)) =
            (self.network_name.as_deref(), self.discovery_record.as_ref())
        {
            if name != record.document.name {
                return Err(
                    invalid("加入网络名称与签名发现记录不一致").for_network(name.to_owned())
                );
            }
        }
        if let Some(network_id) = self.network_id.as_deref() {
            fuckipv6_common::domain::NetworkId::parse(network_id.to_owned())
                .map_err(|_| invalid("网络 ID 格式无效"))?;
            if self.preview_digest.as_deref().is_none_or(str::is_empty)
                || self.network_policy_revision.unwrap_or(0) == 0
                || self.privacy_policy_revision.unwrap_or(0) == 0
                || self.terms_hash.is_none()
                || self.acknowledged_at_unix_ms.unwrap_or(0) == 0
            {
                return Err(invalid("加入网络前必须确认完整的预览条款"));
            }
            if let Some(record) = self.discovery_record.as_ref() {
                if record.document.network_id.as_str() != network_id {
                    return Err(invalid("网络 ID 与签名发现记录不一致"));
                }
            }
        } else if self.preview_digest.is_some()
            || self.network_policy_revision.is_some()
            || self.privacy_policy_revision.is_some()
            || self.terms_hash.is_some()
            || self.acknowledged_at_unix_ms.is_some()
        {
            return Err(invalid("确认字段必须和 network_id 一起提交"));
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeSubmitJoinRequestCommand {
    pub network_id: String,
    pub reason: String,
}

impl NodeSubmitJoinRequestCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        validate_network_id_field(&self.network_id)?;
        validate_join_request_reason_field(&self.reason)
    }
}

impl std::fmt::Debug for NodeSubmitJoinRequestCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NodeSubmitJoinRequestCommand")
            .field("network_id", &self.network_id)
            .field("reason", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeListJoinRequestsCommand {
    pub network_id: String,
}

impl NodeListJoinRequestsCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        validate_network_id_field(&self.network_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeNetworkMembersCommand {
    pub network_id: String,
}

impl NodeNetworkMembersCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        validate_network_id_field(&self.network_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeMemberRemovalPreviewCommand {
    pub network_id: String,
    pub target_node_id: String,
}

impl NodeMemberRemovalPreviewCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        validate_network_id_field(&self.network_id)?;
        validate_node_id_field(&self.target_node_id)
    }
}

#[derive(Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeChangeMemberLifecycleCommand {
    pub network_id: String,
    pub target_node_id: String,
    pub action: MemberLifecycleAction,
    #[serde(default = "default_member_lifecycle_role")]
    #[schemars(default = "default_member_lifecycle_role")]
    pub role: MembershipRole,
    pub expected_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl NodeChangeMemberLifecycleCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        validate_network_id_field(&self.network_id)?;
        validate_node_id_field(&self.target_node_id)?;
        if self.expected_revision == 0 && self.action != MemberLifecycleAction::Invite {
            return Err(join_request_invalid(
                "member_lifecycle.error.invalid_transition",
                "只有首次邀请可以使用 revision 0",
            ));
        }
        if let Some(reason) = self.reason.as_deref() {
            let reason = reason.trim();
            if reason.is_empty()
                || reason.chars().count() > 512
                || reason.chars().any(char::is_control)
            {
                return Err(join_request_invalid(
                    "member_lifecycle.error.invalid_reason",
                    "成员治理原因必须为 1 至 512 个字符且不能包含控制字符",
                ));
            }
        }
        Ok(())
    }
}

fn default_member_lifecycle_role() -> MembershipRole {
    MembershipRole::Member
}

impl std::fmt::Debug for NodeChangeMemberLifecycleCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NodeChangeMemberLifecycleCommand")
            .field("network_id", &self.network_id)
            .field("target_node_id", &self.target_node_id)
            .field("action", &self.action)
            .field("role", &self.role)
            .field("expected_revision", &self.expected_revision)
            .field("reason", &self.reason.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeJoinRequestReviewItem {
    pub request_id: String,
    pub expected_revision: u64,
    pub action: JoinRequestReviewAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl std::fmt::Debug for NodeJoinRequestReviewItem {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NodeJoinRequestReviewItem")
            .field("request_id", &self.request_id)
            .field("expected_revision", &self.expected_revision)
            .field("action", &self.action)
            .field("note", &self.note.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

impl NodeJoinRequestReviewItem {
    fn validate(&self) -> Result<(), NodeProblem> {
        validate_join_request_id_field(&self.request_id)?;
        validate_join_request_revision(self.expected_revision)?;
        match self.action {
            JoinRequestReviewAction::Approve => {
                if self.note.is_some() {
                    return Err(join_request_invalid(
                        "join_request.error.invalid_action_note",
                        "批准申请时不能附带补充信息或拒绝说明",
                    ));
                }
            }
            JoinRequestReviewAction::Reject => {
                validate_optional_join_request_note(self.note.as_deref())?;
            }
            JoinRequestReviewAction::RequestInformation => {
                let note = self.note.as_deref().ok_or_else(|| {
                    join_request_invalid(
                        "join_request.error.information_required",
                        "要求申请人补充信息时必须填写说明",
                    )
                })?;
                validate_optional_join_request_note(Some(note))?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeReviewJoinRequestCommand {
    pub network_id: String,
    pub review: NodeJoinRequestReviewItem,
}

impl NodeReviewJoinRequestCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        validate_network_id_field(&self.network_id)?;
        self.review.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeBatchReviewJoinRequestsCommand {
    pub network_id: String,
    pub reviews: Vec<NodeJoinRequestReviewItem>,
}

impl NodeBatchReviewJoinRequestsCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        validate_network_id_field(&self.network_id)?;
        if self.reviews.is_empty() {
            return Err(join_request_invalid(
                "join_request.error.batch_empty",
                "批量审批至少需要选择一条申请",
            ));
        }
        if self.reviews.len() > 100 {
            return Err(join_request_invalid(
                "join_request.error.batch_too_large",
                "批量审批一次最多处理 100 条申请",
            ));
        }
        let mut request_ids = HashSet::new();
        for review in &self.reviews {
            review.validate()?;
            if !request_ids.insert(review.request_id.as_str()) {
                return Err(join_request_invalid(
                    "join_request.error.batch_duplicate",
                    "批量审批不能重复选择同一申请",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeWithdrawJoinRequestCommand {
    pub network_id: String,
    pub request_id: String,
}

impl NodeWithdrawJoinRequestCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        validate_network_id_field(&self.network_id)?;
        validate_join_request_id_field(&self.request_id)
    }
}

#[derive(Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeResubmitJoinRequestCommand {
    pub network_id: String,
    pub request_id: String,
    pub expected_revision: u64,
    pub reason: String,
}

impl NodeResubmitJoinRequestCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        validate_network_id_field(&self.network_id)?;
        validate_join_request_id_field(&self.request_id)?;
        validate_join_request_revision(self.expected_revision)?;
        validate_join_request_reason_field(&self.reason)
    }
}

impl std::fmt::Debug for NodeResubmitJoinRequestCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NodeResubmitJoinRequestCommand")
            .field("network_id", &self.network_id)
            .field("request_id", &self.request_id)
            .field("expected_revision", &self.expected_revision)
            .field("reason", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeCreateInvitationCommand {
    pub network_id: NetworkId,
    pub role: MembershipRole,
    #[serde(default)]
    pub capabilities: Vec<CapabilityKind>,
    pub max_uses: u32,
    /// Requested lifetime in milliseconds. The domain bounds this to one minute
    /// through thirty days.
    pub expires_in_ms: u64,
}

impl NodeCreateInvitationCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        InvitationCreateRequest::new(
            self.network_id.clone(),
            self.role,
            self.capabilities.clone(),
            self.max_uses,
            self.expires_in_ms,
        )
        .validate()
        .map_err(|_| {
            invitation_invalid(
                "invitation.error.invalid_create_request",
                "邀请参数无效：角色只能是成员或管理员，使用次数、有效期或能力声明不符合要求",
            )
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeListInvitationsCommand {
    pub network_id: NetworkId,
}

impl NodeListInvitationsCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeGetInvitationStatusCommand {
    pub network_id: NetworkId,
    pub invitation_id: InvitationId,
}

impl NodeGetInvitationStatusCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeRevokeInvitationCommand {
    pub network_id: NetworkId,
    pub invitation_id: InvitationId,
    pub expected_revision: u64,
}

impl NodeRevokeInvitationCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        if self.expected_revision == 0 {
            return Err(invitation_invalid(
                "invitation.error.invalid_revision",
                "邀请 revision 必须大于零",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodePreviewInvitationCommand {
    pub network_id: NetworkId,
    /// Coordinator endpoint is a separate routing hint, never part of the
    /// invitation credential and never placed in the URL query.
    pub coordinator_url: String,
    pub code: InvitationCode,
}

impl NodePreviewInvitationCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        validate_invitation_coordinator_url(&self.coordinator_url)
    }
}

impl std::fmt::Debug for NodePreviewInvitationCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NodePreviewInvitationCommand")
            .field("network_id", &self.network_id)
            .field("coordinator_url", &self.coordinator_url)
            .field("code", &"[redacted]")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeRedeemInvitationCommand {
    /// Coordinator endpoint is a separate routing hint, never part of the
    /// invitation credential and never placed in the URL query.
    pub coordinator_url: String,
    pub request: InvitationRedeemRequest,
}

impl NodeRedeemInvitationCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        validate_invitation_coordinator_url(&self.coordinator_url)?;
        if self.request.expected_invitation_revision == 0
            || self.request.acknowledged_at_unix_ms == 0
        {
            return Err(invitation_invalid(
                "invitation.error.invalid_consent",
                "接受邀请前必须展示并确认当前邀请条款",
            ));
        }
        Ok(())
    }
}

impl std::fmt::Debug for NodeRedeemInvitationCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NodeRedeemInvitationCommand")
            .field("coordinator_url", &self.coordinator_url)
            .field("request", &self.request)
            .finish()
    }
}

fn validate_invitation_coordinator_url(url: &str) -> Result<(), NodeProblem> {
    if url.is_empty()
        || url.trim() != url
        || url.len() > 2048
        || url.chars().any(char::is_control)
        || (!url.starts_with("ws://") && !url.starts_with("wss://"))
    {
        return Err(invitation_invalid(
            "invitation.error.invalid_coordinator_url",
            "协调服务器地址必须以 ws:// 或 wss:// 开头，且不能包含空白或控制字符",
        ));
    }
    Ok(())
}

fn invitation_invalid(code: &str, message: &str) -> NodeProblem {
    NodeProblem::new(NodeProblemCode::InvalidRequest, message).with_param("reason", code)
}

fn validate_network_id_field(network_id: &str) -> Result<(), NodeProblem> {
    if network_id.trim().is_empty()
        || fuckipv6_common::domain::NetworkId::parse(network_id.to_owned()).is_err()
    {
        return Err(join_request_invalid(
            "join_request.error.invalid_network_id",
            "网络 ID 格式无效",
        ));
    }
    Ok(())
}

fn validate_node_id_field(node_id: &str) -> Result<(), NodeProblem> {
    fuckipv6_common::domain::NodeId::parse(node_id.to_owned())
        .map(|_| ())
        .map_err(|_| {
            join_request_invalid(
                "member_lifecycle.error.invalid_target",
                "目标节点 ID 格式无效",
            )
        })
}

fn validate_join_request_id_field(request_id: &str) -> Result<(), NodeProblem> {
    if request_id.trim().is_empty()
        || fuckipv6_common::domain::JoinRequestId::parse(request_id.to_owned()).is_err()
    {
        return Err(join_request_invalid(
            "join_request.error.invalid_request_id",
            "加入申请 ID 格式无效",
        ));
    }
    Ok(())
}

fn validate_join_request_revision(expected_revision: u64) -> Result<(), NodeProblem> {
    if expected_revision == 0 {
        return Err(join_request_invalid(
            "join_request.error.invalid_revision",
            "加入申请 revision 必须大于零",
        ));
    }
    Ok(())
}

fn validate_join_request_reason_field(reason: &str) -> Result<(), NodeProblem> {
    fuckipv6_common::domain::validate_join_request_reason(reason).map_err(|_| {
        join_request_invalid(
            "join_request.error.invalid_reason",
            format!(
                "申请理由必须非空、去除首尾空白、不含控制字符，且不超过 {JOIN_REQUEST_MAX_REASON_CHARS} 个字符"
            ),
        )
    })
}

fn validate_optional_join_request_note(note: Option<&str>) -> Result<(), NodeProblem> {
    let Some(note) = note else {
        return Ok(());
    };
    if note.trim().is_empty()
        || note.trim() != note
        || note.chars().count() > JOIN_REQUEST_MAX_INFORMATION_REQUEST_CHARS
        || note.chars().any(char::is_control)
    {
        return Err(join_request_invalid(
            "join_request.error.invalid_action_note",
            format!(
                "审批说明必须非空、去除首尾空白、不含控制字符，且不超过 {JOIN_REQUEST_MAX_INFORMATION_REQUEST_CHARS} 个字符"
            ),
        ));
    }
    Ok(())
}

fn join_request_invalid(key: &'static str, message: impl Into<String>) -> NodeProblem {
    invalid(message).with_message_key(key)
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeLeaveNetworkCommand {
    pub network_name: String,
}

impl crate::node::NodeLeaveNetworkCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        validate_network_name(&self.network_name)
    }
}

/// Complete, signed disclosure shown before a direct local join. It contains
/// public governance metadata only: no member addresses, topology, activity,
/// invitation credentials or private keys cross this boundary.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeNetworkJoinPreview {
    pub network_id: String,
    pub network_name: String,
    pub public_key: String,
    pub owner: String,
    pub administrators: Vec<String>,
    pub discovery_revision: u64,
    pub network_policy_revision: u64,
    pub privacy_policy_revision: u64,
    pub join_mode: NetworkJoinMode,
    pub visibility: NetworkVisibility,
    pub capability_summary: Vec<CapabilityKind>,
    pub endpoints: Vec<NetworkEndpoint>,
    pub member_count: u32,
    pub member_limit: Option<u32>,
    pub exit_contribution: ExitContributionObligation,
    pub exit_contribution_scope: ExitContributionScope,
    /// Stable disclosure keys cover what the local owner is consenting to.
    /// They never enable or advertise the local Exit service by themselves.
    pub exit_contribution_purpose_key: String,
    pub exit_contribution_bandwidth_impact_key: String,
    pub exit_contribution_logging_boundary_key: String,
    pub exit_contribution_disable_key: String,
    /// Local host support is separate from the network's declaration.
    pub local_exit_available: bool,
    /// This is the node's actual local role state, never inferred from terms.
    pub local_exit_enabled: bool,
    /// True only when a member can join without promising Exit contribution.
    pub can_join_without_local_exit: bool,
    /// The direct join confirmation must explicitly acknowledge this obligation.
    pub join_requires_exit_consent: bool,
    pub privacy_changes: Vec<String>,
    pub already_member: bool,
    pub can_join: bool,
    pub blocked_reason_key: Option<String>,
    /// Current join terms are hashed exactly as acknowledged by the owner.
    pub terms_hash: ContentHash,
    /// The digest also binds discovery revision, policy disclosures, endpoints,
    /// capability declarations and expiry.
    pub preview_digest: String,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    /// Capabilities remain locally controlled. The only admission obligation
    /// is disclosed separately as Exit contribution.
    pub locally_contributed_capabilities: Vec<CapabilityKind>,
    pub contribution_policy_key: String,
}

/// Re-confirm the exact signed network terms after a policy migration that
/// covers existing members. The hash pins the disclosure the owner saw; the
/// separate Exit flag can never be inferred from accepting the terms.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeAcknowledgeMembershipPolicyCommand {
    pub network_id: String,
    pub expected_terms_hash: ContentHash,
    pub acknowledged_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_contribution_acknowledged: Option<bool>,
}

impl NodeAcknowledgeMembershipPolicyCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        validate_network_id_field(&self.network_id)?;
        if self.acknowledged_at_unix_ms == 0 {
            return Err(invalid("确认成员策略前必须记录有效的知情确认时间")
                .with_message_key("membership.policy.error.invalid_consent"));
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
pub struct NodeRotateNodeIdentityCommand {
    /// The current public-key fingerprint shown to the owner. Requiring it
    /// prevents a stale UI from rotating a newer identity that it has not seen.
    pub expected_fingerprint: String,
}

impl NodeRotateNodeIdentityCommand {
    fn validate(&self) -> Result<(), NodeProblem> {
        if self.expected_fingerprint.trim() != self.expected_fingerprint {
            return Err(invalid("公钥指纹首尾不能包含空白字符"));
        }
        let decoded = URL_SAFE_NO_PAD
            .decode(&self.expected_fingerprint)
            .map_err(|_| invalid("公钥指纹格式无效"))?;
        if decoded.len() != 32 {
            return Err(invalid("公钥指纹必须解码为 32 字节"));
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
    pub network_id: String,
    pub public_key: String,
    pub owner_role: MembershipRole,
    pub config_saved: bool,
    pub started: bool,
    pub next_steps: Vec<NodeNetworkCreationNextStep>,
    pub invitation_ticket: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeNetworkCreationNextStep {
    InviteMembers,
    ConfigurePolicy,
    ContributeCapabilities,
}

impl NodeCreatedNetwork {
    /// Human-readable result for an owner-facing CLI or local UI.
    pub fn owner_message(&self) -> String {
        format!(
            "网络 [{}] 已创建并保存；Network ID: {}；{}；邀请票据: {}",
            self.network_name,
            self.network_id,
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
            .field("network_id", &self.network_id)
            .field("public_key", &self.public_key)
            .field("owner_role", &self.owner_role)
            .field("config_saved", &self.config_saved)
            .field("started", &self.started)
            .field("next_steps", &self.next_steps)
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

/// Stable diagnostic layers shown in a fixed order by every adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeDiagnosticLayer {
    Network,
    Dns,
    Tun,
    Coordinator,
    Relay,
    Exit,
}

impl NodeDiagnosticLayer {
    pub const ALL: [Self; 6] = [
        Self::Network,
        Self::Dns,
        Self::Tun,
        Self::Coordinator,
        Self::Relay,
        Self::Exit,
    ];

    pub const fn key(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Dns => "dns",
            Self::Tun => "tun",
            Self::Coordinator => "coordinator",
            Self::Relay => "relay",
            Self::Exit => "exit",
        }
    }
}

/// Five-state result deliberately richer than a boolean. Adapters must render
/// every state verbatim and must not infer `passed` from missing probe data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeDiagnosticState {
    NotTested,
    Passed,
    Failed,
    Unsupported,
    NoPermission,
}

impl NodeDiagnosticState {
    pub const fn key(self) -> &'static str {
        match self {
            Self::NotTested => "not_tested",
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Unsupported => "unsupported",
            Self::NoPermission => "no_permission",
        }
    }
}

/// One sanitized layered probe. Raw process output, host paths, interface MAC
/// addresses, tokens, keys, full topology and peer addresses are excluded.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeDiagnosticCheck {
    pub layer: NodeDiagnosticLayer,
    pub state: NodeDiagnosticState,
    pub started_at_unix_ms: u64,
    pub duration_ms: u64,
    /// Stable machine code, never raw stderr.
    pub error_code: Option<String>,
    pub message_key: String,
    pub suggestion_key: Option<String>,
}

/// A recovery action that is safe to render from untrusted UI input. The UI
/// must request explicit confirmation when `confirmation_required` is true and
/// show the impact key before dispatching any command.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeDiagnosticRecoveryAction {
    pub action_id: String,
    pub impact_key: String,
    pub confirmation_required: bool,
}

/// Live actor-owned diagnostics plus a bounded layered probe report.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeDiagnosticsSnapshot {
    pub diagnostic_id: String,
    pub started_at_unix_ms: u64,
    pub completed_at_unix_ms: u64,
    pub timed_out: bool,
    pub lifecycle: NodeLifecycleSnapshot,
    pub retained_owner_networks: Vec<String>,
    pub cleanup_candidate_networks: Vec<String>,
    pub residue: NodeRuntimeResidueSnapshot,
    pub checks: Vec<NodeDiagnosticCheck>,
    pub recovery_actions: Vec<NodeDiagnosticRecoveryAction>,
    /// Canonical redacted report suitable for clipboard/export. It is generated
    /// server-side from stable fields, never from raw stderr.
    pub sanitized_report: String,
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

/// Public result of a completed node key rotation. The DTO deliberately
/// contains fingerprints and identifiers only; private keys and owner tokens
/// never cross the command boundary.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeIdentityRotationSnapshot {
    pub old_node_id: String,
    pub old_fingerprint: String,
    pub new_node_id: String,
    pub new_fingerprint: String,
    pub created_at_unix_ms: u64,
    pub owner_initialized: bool,
    pub owner_sessions_invalidated: bool,
    pub affected_configured_network_count: usize,
}

#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct NodeNetworkJoinResult {
    pub membership: crate::node::NodeNetworkMembershipSnapshot,
    pub snapshot: NodeSnapshot,
    pub idempotent: bool,
    pub config_saved: bool,
    pub started: bool,
}

#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct NodeNetworkLeaveResult {
    pub network_name: String,
    pub left: bool,
    pub snapshot: NodeSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeJoinRequestListResult {
    pub network_id: String,
    /// Reviewer callers receive every network request; other callers receive
    /// only their own requests. Reason and notes remain private fields.
    pub requests: Vec<JoinRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeJoinRequestReviewOutcome {
    pub request_id: String,
    pub state: JoinRequestState,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeJoinRequestReviewAck {
    pub network_id: String,
    pub reviews: Vec<NodeJoinRequestReviewOutcome>,
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
    NetworkProfilePreview(Box<NetworkProfilePreview>),
    NetworkProfileUpdated(Box<NetworkProfileApplyResult>),
    NetworkJoinPreview(Box<NodeNetworkJoinPreview>),
    NetworkJoined(Box<NodeNetworkJoinResult>),
    JoinRequestSubmitted(Box<JoinRequest>),
    JoinRequests(Box<NodeJoinRequestListResult>),
    NetworkMembers(Box<NetworkMemberDirectory>),
    MemberRemovalPreview(Box<MemberRemovalPreview>),
    MemberLifecycleChanged(Box<MemberLifecycleChange>),
    JoinRequestsReviewed(Box<NodeJoinRequestReviewAck>),
    JoinRequestUpdated(Box<JoinRequest>),
    InvitationCreated(Box<IssuedInvitation>),
    Invitations(Vec<InvitationStatusView>),
    InvitationStatus(Box<InvitationStatusView>),
    InvitationRevoked(Box<InvitationStatusView>),
    InvitationPreview(Box<InvitationPreview>),
    InvitationRedeemed(Box<InvitationRedemption>),
    NetworkLeft(Box<NodeNetworkLeaveResult>),
    MembershipPolicyAcknowledged(Box<crate::node::NodeNetworkMembershipSnapshot>),
    IdentityRotated(NodeIdentityRotationSnapshot),
    Configuration(ConfigurationDocument),
    ConfigurationPreview(Box<ConfigurationPreview>),
    ConfigurationApplied(Box<ConfigurationApplyResult>),
    ConfigurationHistory(Vec<ConfigurationHistoryEntry>),
    Policy(PolicyCurrent),
    PolicyPreview(Box<PolicyPreview>),
    PolicyApplied(Box<PolicyApplyResult>),
    PolicyHistory(Box<PolicyHistorySnapshot>),
    PolicyRolledBack(Box<PolicyRollbackResult>),
    PolicyTest(Box<PolicyTestResult>),
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

    /// Cancellable variant used by operation execution. The default preserves
    /// compatibility for handlers without cancellable work; the runtime
    /// handler overrides it for diagnostics.
    fn handle_authorized_cancellable(
        &self,
        context: NodeCommandContext,
        request: NodeCommandRequest,
        _cancel: watch::Receiver<bool>,
    ) -> NodeCommandFuture {
        self.handle_authorized(context, request)
    }
}

struct CommandEnvelope {
    context: NodeCommandContext,
    request: NodeCommandRequest,
    cancel: watch::Receiver<bool>,
    response: oneshot::Sender<NodeCommandResult>,
}

const ACTIVITY_RETENTION_LIMIT: usize = 256;

struct CommandAuditTrail {
    entries: Arc<Mutex<Vec<AuditEntry>>>,
    recent_activity: Arc<Mutex<Vec<ActivityEntry>>>,
    next_sequence: Arc<AtomicU64>,
    durable: Option<Arc<OwnerSecurityAudit>>,
}

impl CommandAuditTrail {
    fn new(durable: Option<Arc<OwnerSecurityAudit>>) -> Self {
        Self {
            entries: Arc::new(Mutex::new(Vec::new())),
            recent_activity: Arc::new(Mutex::new(Vec::new())),
            next_sequence: Arc::new(AtomicU64::new(0)),
            durable,
        }
    }

    fn record(
        &self,
        context: &NodeCommandContext,
        request: &NodeCommandRequest,
        result: Option<&NodeCommandResult>,
        snapshot: Option<&NodeSnapshot>,
        outcome: AuditOutcome,
        reason: (&str, Option<&str>),
    ) -> anyhow::Result<Option<ActivityEntry>> {
        let (reason, reason_detail) = reason;
        // A preview is a read-only disclosure. Keep it out of the durable
        // command audit and activity feed; the subsequent confirmed join is the
        // auditable mutation.
        if matches!(
            request.command.kind(),
            NodeCommandKind::PreviewJoinNetwork | NodeCommandKind::TestPolicy
        ) {
            return Ok(None);
        }
        let occurred_at_unix_ms = unix_time_ms();
        let sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
        let audit_id = fuckipv6_common::domain::AuditId::from_public_key(
            format!(
                "{}:{}:{}:{}:{}",
                context.actor(),
                request.request_id,
                request.command.kind().as_str(),
                sequence,
                occurred_at_unix_ms
            )
            .as_bytes(),
        );
        let canonical_action = audit_for_command(request.command.kind()).action;
        let action = canonical_action.as_deref().unwrap_or("node.command");
        let mut details = BTreeMap::new();
        details.insert("command".into(), request.command.kind().as_str().into());
        details.insert("request_id".into(), request.request_id.to_string());
        details.insert("reason_code".into(), reason.into());
        if let Some(reason_detail) = reason_detail {
            details.insert("reason_detail".into(), reason_detail.into());
        }
        details.insert("actor_source".into(), context.source().into());
        details.insert(
            "actor_role".into(),
            context.roles_for_command(&request.command),
        );
        let network_id = activity_network_id(&request.command, context, result, snapshot);
        let (object_kind, object_id) =
            activity_object_identity(&request.command, context, result, snapshot);
        details.insert("object_kind".into(), object_kind);
        if let Some(object_id) = object_id {
            details.insert("object_id".into(), object_id);
        }
        if let Some(network_id) = &network_id {
            details.insert("network_id".into(), network_id.to_string());
        }
        insert_membership_policy_audit_details(&mut details, request, result, snapshot);
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
            occurred_at_unix_ms,
            summary: summary.into(),
            details,
            sensitive_keys: Default::default(),
        };
        if let Some(ledger) = &self.durable {
            let should_persist = outcome == AuditOutcome::Denied
                || audit_for_command(request.command.kind()).action.is_some();
            if should_persist {
                // Free-form denial prose can contain adapter-provided values.
                // The stable code is sufficient evidence; typed object and
                // policy fields below are the only additional details kept.
                let mut durable_entry = entry.clone();
                durable_entry.details.remove("reason_detail");
                ledger
                    .append_entry(durable_entry)
                    .map_err(|error| anyhow::anyhow!("安全审计写入失败: {error:#}"))?;
            }
        }
        if let Ok(mut entries) = self.entries.lock() {
            entries.push(entry.clone());
        }
        if !request.command.kind().is_write() && outcome != AuditOutcome::Denied {
            return Ok(None);
        }
        let projected = entry.project_activity();
        if let Ok(mut activity) = self.recent_activity.lock() {
            // A replayed snapshot or event must not duplicate an activity row.
            // Replace by stable audit identity, then keep the feed deterministic
            // and newest-first before applying the short-term retention bound.
            if let Some(index) = activity
                .iter()
                .position(|existing| existing.audit_id == projected.audit_id)
            {
                activity[index] = projected.clone();
            } else {
                activity.push(projected.clone());
            }
            activity.sort_by(|left, right| {
                right
                    .occurred_at_unix_ms
                    .cmp(&left.occurred_at_unix_ms)
                    .then_with(|| right.audit_id.cmp(&left.audit_id))
            });
            activity.truncate(ACTIVITY_RETENTION_LIMIT);
        }
        Ok(Some(projected))
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

fn activity_network_id(
    command: &NodeCommand,
    context: &NodeCommandContext,
    result: Option<&NodeCommandResult>,
    snapshot: Option<&NodeSnapshot>,
) -> Option<NetworkId> {
    let parse = |id: &str| NetworkId::parse(id.to_owned()).ok();
    let from_snapshot = |name: &str| {
        let snapshot = snapshot?;
        let network_id = snapshot
            .memberships
            .iter()
            .find(|membership| membership.network_name == name)
            .map(|membership| membership.network_id.as_str())
            .or_else(|| {
                snapshot
                    .network_profiles
                    .iter()
                    .find(|profile| profile.name == name)
                    .map(|profile| profile.network_id.as_str())
            })?;
        parse(network_id)
    };

    let command_network_id = match command {
        NodeCommand::PreviewJoinNetwork(command) => Some(command.network_id.as_str()),
        NodeCommand::JoinNetwork(command) => command.network_id.as_deref(),
        NodeCommand::SubmitJoinRequest(command) => Some(command.network_id.as_str()),
        NodeCommand::ListJoinRequests(command) => Some(command.network_id.as_str()),
        NodeCommand::ListNetworkMembers(command) => Some(command.network_id.as_str()),
        NodeCommand::PreviewMemberRemoval(command) => Some(command.network_id.as_str()),
        NodeCommand::ChangeMemberLifecycle(command) => Some(command.network_id.as_str()),
        NodeCommand::ReviewJoinRequest(command) => Some(command.network_id.as_str()),
        NodeCommand::BatchReviewJoinRequests(command) => Some(command.network_id.as_str()),
        NodeCommand::WithdrawJoinRequest(command) => Some(command.network_id.as_str()),
        NodeCommand::ResubmitJoinRequest(command) => Some(command.network_id.as_str()),
        NodeCommand::AcknowledgeMembershipPolicy(command) => Some(command.network_id.as_str()),
        NodeCommand::CreateInvitation(command) => Some(command.network_id.as_str()),
        NodeCommand::ListInvitations(command) => Some(command.network_id.as_str()),
        NodeCommand::GetInvitationStatus(command) => Some(command.network_id.as_str()),
        NodeCommand::RevokeInvitation(command) => Some(command.network_id.as_str()),
        NodeCommand::PreviewInvitation(command) => Some(command.network_id.as_str()),
        NodeCommand::RedeemInvitation(command) => Some(command.request.network_id.as_str()),
        _ => None,
    };
    if let Some(network_id) = command_network_id.and_then(parse) {
        return Some(network_id);
    }

    let command_network_name = match command {
        NodeCommand::RebuildTun { network_name } => Some(network_name.as_str()),
        NodeCommand::PreviewNetworkProfile(command)
        | NodeCommand::UpdateNetworkProfile(command) => Some(command.network_name.as_str()),
        NodeCommand::PublishNetworkDiscovery(command)
        | NodeCommand::UpdateNetworkDiscovery(command)
        | NodeCommand::UnpublishNetworkDiscovery(command) => Some(command.network_name.as_str()),
        NodeCommand::UpdateRouting(command) => Some(command.network_name.as_str()),
        NodeCommand::SetNetworkRole(command) => Some(command.network_name.as_str()),
        NodeCommand::JoinNetwork(command) => command.network_name.as_deref(),
        NodeCommand::LeaveNetwork(command) => Some(command.network_name.as_str()),
        NodeCommand::GetBlacklist { network_name } => Some(network_name.as_str()),
        NodeCommand::GetNetworkAllocation { network_name } => Some(network_name.as_str()),
        NodeCommand::BanClient(command) | NodeCommand::UnbanClient(command) => {
            Some(command.network_name.as_str())
        }
        NodeCommand::UpdateNetworkAllocation(command) => Some(command.network_name.as_str()),
        NodeCommand::ProbePeer(command) => Some(command.network_name.as_str()),
        _ => None,
    };
    if let Some(network_id) = command_network_name.and_then(from_snapshot) {
        return Some(network_id);
    }

    if let Some(NodeCommandResult {
        outcome: NodeCommandOutcome::NetworkCreated(created),
        ..
    }) = result
    {
        if let Some(network_id) = parse(&created.network_id) {
            return Some(network_id);
        }
    }
    if let Some(NodeCommandResult {
        outcome: NodeCommandOutcome::NetworkJoined(joined),
        ..
    }) = result
    {
        if let Some(network_id) = parse(&joined.membership.network_id) {
            return Some(network_id);
        }
    }
    if let Some(NodeCommandResult {
        outcome: NodeCommandOutcome::MemberLifecycleChanged(change),
        ..
    }) = result
    {
        return Some(change.network_id.clone());
    }
    if let Some(NodeCommandResult {
        outcome: NodeCommandOutcome::JoinRequestsReviewed(review),
        ..
    }) = result
    {
        return parse(&review.network_id);
    }
    if let Some(NodeCommandResult {
        outcome: NodeCommandOutcome::InvitationCreated(issued),
        ..
    }) = result
    {
        return Some(issued.status.invitation.network_id.clone());
    }
    if let Some(NodeCommandResult {
        outcome:
            NodeCommandOutcome::InvitationStatus(status) | NodeCommandOutcome::InvitationRevoked(status),
        ..
    }) = result
    {
        return Some(status.invitation.network_id.clone());
    }
    if let Some(NodeCommandResult {
        outcome: NodeCommandOutcome::InvitationRedeemed(redemption),
        ..
    }) = result
    {
        return Some(redemption.network_id.clone());
    }

    context.network_id_for_command(command)
}

fn activity_object_identity(
    command: &NodeCommand,
    context: &NodeCommandContext,
    result: Option<&NodeCommandResult>,
    snapshot: Option<&NodeSnapshot>,
) -> (String, Option<String>) {
    let node = || ("node".into(), Some(context.actor().to_string()));
    let network = || {
        (
            "network".into(),
            activity_network_id(command, context, result, snapshot).map(|id| id.to_string()),
        )
    };
    match command {
        NodeCommand::PreviewMemberRemoval(command) => {
            ("member".into(), Some(command.target_node_id.clone()))
        }
        NodeCommand::ChangeMemberLifecycle(command) => {
            ("member".into(), Some(command.target_node_id.clone()))
        }
        NodeCommand::SubmitJoinRequest(_) => match result {
            Some(NodeCommandResult {
                outcome: NodeCommandOutcome::JoinRequestSubmitted(request),
                ..
            }) => ("join_request".into(), Some(request.request_id.to_string())),
            _ => ("join_request".into(), None),
        },
        NodeCommand::ReviewJoinRequest(command) => (
            "join_request".into(),
            Some(command.review.request_id.clone()),
        ),
        NodeCommand::BatchReviewJoinRequests(_) => ("join_request".into(), None),
        NodeCommand::WithdrawJoinRequest(command) => (
            "join_request".into(),
            Some(match result {
                Some(NodeCommandResult {
                    outcome: NodeCommandOutcome::JoinRequestUpdated(request),
                    ..
                }) => request.request_id.to_string(),
                _ => command.request_id.clone(),
            }),
        ),
        NodeCommand::ResubmitJoinRequest(command) => (
            "join_request".into(),
            Some(match result {
                Some(NodeCommandResult {
                    outcome: NodeCommandOutcome::JoinRequestUpdated(request),
                    ..
                }) => request.request_id.to_string(),
                _ => command.request_id.clone(),
            }),
        ),
        NodeCommand::CreateInvitation(_) => match result {
            Some(NodeCommandResult {
                outcome: NodeCommandOutcome::InvitationCreated(issued),
                ..
            }) => (
                "invitation".into(),
                Some(issued.status.invitation.invitation_id.to_string()),
            ),
            _ => ("invitation".into(), None),
        },
        NodeCommand::GetInvitationStatus(command) => {
            ("invitation".into(), Some(command.invitation_id.to_string()))
        }
        NodeCommand::RevokeInvitation(command) => {
            ("invitation".into(), Some(command.invitation_id.to_string()))
        }
        NodeCommand::PreviewInvitation(_) => ("invitation".into(), None),
        NodeCommand::RedeemInvitation(command) => (
            "invitation".into(),
            Some(command.request.invitation_id.to_string()),
        ),
        // Client identifiers may be user-supplied payload and are not needed
        // to make the activity entry meaningful. Keep the object family only so
        // denials cannot copy payload secrets into the audit trail.
        NodeCommand::BanClient(_) | NodeCommand::UnbanClient(_) => ("client".into(), None),
        NodeCommand::SetNodeDisplayName(_)
        | NodeCommand::RotateNodeIdentity(_)
        | NodeCommand::StartNode
        | NodeCommand::StopNode
        | NodeCommand::RestartNode
        | NodeCommand::CleanupResiduals { .. }
        | NodeCommand::Shutdown
        | NodeCommand::GetSnapshot
        | NodeCommand::GetUiManifest
        | NodeCommand::GetDiagnostics
        | NodeCommand::RefreshDiscoveryIndex { .. }
        | NodeCommand::RetryDiscoveryIndex
        | NodeCommand::GetConfiguration
        | NodeCommand::PreviewConfiguration(_)
        | NodeCommand::GetConfigurationHistory
        | NodeCommand::GetPolicy
        | NodeCommand::PreviewPolicy(_)
        | NodeCommand::PreviewPolicyRollback(_)
        | NodeCommand::GetPolicyHistory
        | NodeCommand::TestPolicy(_) => node(),
        NodeCommand::ApplyConfiguration(_) | NodeCommand::RollbackConfiguration(_) => {
            ("configuration".into(), None)
        }
        NodeCommand::ApplyPolicy(_) | NodeCommand::RollbackPolicy(_) => ("policy".into(), None),
        _ => network(),
    }
}

fn exit_contribution_obligation_audit_value(
    obligation: ExitContributionObligation,
) -> &'static str {
    match obligation {
        ExitContributionObligation::Required => "required",
        ExitContributionObligation::NotRequired => "not_required",
    }
}

fn exit_contribution_scope_audit_value(scope: ExitContributionScope) -> &'static str {
    match scope {
        ExitContributionScope::NotApplicable => "not_applicable",
        ExitContributionScope::JoiningMembers => "joining_members",
        ExitContributionScope::AllMembers => "all_members",
    }
}

fn insert_optional_audit_detail(
    details: &mut BTreeMap<String, String>,
    key: &str,
    value: Option<impl ToString>,
) {
    if let Some(value) = value {
        details.insert(key.into(), value.to_string());
    }
}

/// Record the membership/Exit facts needed to prove that consent, network
/// policy and local runtime state stayed separate. Only allowlisted typed fields
/// are projected here; rejection prose and invitation credentials never enter
/// the audit details.
fn insert_membership_policy_audit_details(
    details: &mut BTreeMap<String, String>,
    request: &NodeCommandRequest,
    result: Option<&NodeCommandResult>,
    snapshot: Option<&NodeSnapshot>,
) {
    let mut network_id = None::<String>;
    let mut network_name = None::<String>;
    let mut consent = None::<&'static str>;
    let mut acknowledgement_time = None::<u64>;

    match &request.command {
        NodeCommand::CreateNetwork(command) => {
            network_name = Some(command.name.clone());
            details.insert(
                "exit_contribution".into(),
                exit_contribution_obligation_audit_value(command.exit_contribution).into(),
            );
            details.insert(
                "exit_contribution_scope".into(),
                exit_contribution_scope_audit_value(command.exit_contribution_scope).into(),
            );
            if let Some(NodeCommandResult {
                outcome: NodeCommandOutcome::NetworkCreated(created),
                ..
            }) = result
            {
                network_id = Some(created.network_id.clone());
                network_name = Some(created.network_name.clone());
            }
            // A newly created network starts at policy revision 1 on every
            // surface. The command result only exposes the public profile
            // revision, so keep this value explicit and verifiable.
            details.insert("network_policy_revision".into(), "1".into());
            details.insert("privacy_policy_revision".into(), "1".into());
        }
        NodeCommand::UpdateNetworkProfile(command) => {
            network_name = Some(command.network_name.clone());
            details.insert(
                "exit_contribution".into(),
                exit_contribution_obligation_audit_value(command.update.exit_contribution).into(),
            );
            details.insert(
                "exit_contribution_scope".into(),
                exit_contribution_scope_audit_value(command.update.exit_contribution_scope).into(),
            );
            if let Some(NodeCommandResult {
                outcome: NodeCommandOutcome::NetworkProfileUpdated(updated),
                ..
            }) = result
            {
                details.insert(
                    "network_policy_revision".into(),
                    updated
                        .projections
                        .administrator_view
                        .policy_revision
                        .to_string(),
                );
            }
        }
        NodeCommand::JoinNetwork(command) => {
            network_id = command.network_id.clone();
            network_name = command.network_name.clone();
            consent = Some(match command.exit_contribution_acknowledged {
                Some(true) => "accepted",
                Some(false) => "declined",
                None => "not_confirmed",
            });
            acknowledgement_time = command.acknowledged_at_unix_ms;
            insert_optional_audit_detail(
                details,
                "network_policy_revision",
                command.network_policy_revision,
            );
            insert_optional_audit_detail(
                details,
                "privacy_policy_revision",
                command.privacy_policy_revision,
            );
            insert_optional_audit_detail(
                details,
                "terms_hash",
                command.terms_hash.as_ref().map(|hash| hash.as_str()),
            );
            insert_optional_audit_detail(
                details,
                "preview_digest",
                command.preview_digest.as_deref(),
            );
        }
        NodeCommand::RedeemInvitation(command) => {
            network_id = Some(command.request.network_id.to_string());
            consent = Some(match command.request.exit_contribution_acknowledged {
                Some(true) => "accepted",
                Some(false) => "declined",
                None => "not_confirmed",
            });
            acknowledgement_time = Some(command.request.acknowledged_at_unix_ms);
            details.insert(
                "invitation_revision".into(),
                command.request.expected_invitation_revision.to_string(),
            );
            details.insert(
                "terms_hash".into(),
                command.request.terms_hash.as_str().to_owned(),
            );
            details.insert(
                "preview_digest".into(),
                command.request.preview_digest.as_str().to_owned(),
            );
        }
        NodeCommand::AcknowledgeMembershipPolicy(command) => {
            network_id = Some(command.network_id.clone());
            consent = Some(match command.exit_contribution_acknowledged {
                Some(true) => "accepted",
                Some(false) => "declined",
                None => "not_confirmed",
            });
            acknowledgement_time = Some(command.acknowledged_at_unix_ms);
            details.insert(
                "terms_hash".into(),
                command.expected_terms_hash.as_str().to_owned(),
            );
        }
        _ => return,
    }

    if let Some(consent) = consent {
        details.insert("exit_contribution_user_consent".into(), consent.into());
    }
    if let Some(time) = acknowledgement_time {
        details.insert(
            "exit_contribution_acknowledged_at_unix_ms".into(),
            time.to_string(),
        );
    }
    if let Some(id) = network_id.as_deref() {
        details.insert("membership_network_id".into(), id.into());
    }

    let result_membership = result.and_then(|result| match &result.outcome {
        NodeCommandOutcome::NetworkJoined(joined) => Some(&joined.membership),
        NodeCommandOutcome::MembershipPolicyAcknowledged(membership) => Some(membership.as_ref()),
        _ => None,
    });
    if let Some(membership) = result_membership {
        network_id = Some(membership.network_id.clone());
        network_name = Some(membership.network_name.clone());
    } else if network_id.is_none() {
        if let Some(snapshot) = snapshot {
            if let Some(membership) = snapshot
                .memberships
                .iter()
                .find(|membership| Some(&membership.network_name) == network_name.as_ref())
            {
                network_id = Some(membership.network_id.clone());
                network_name = Some(membership.network_name.clone());
            }
        }
    }

    let snapshot_membership = snapshot.and_then(|snapshot| {
        snapshot.memberships.iter().find(|membership| {
            network_id
                .as_deref()
                .is_some_and(|id| membership.network_id == id)
                || network_name
                    .as_deref()
                    .is_some_and(|name| membership.network_name == name)
        })
    });
    let membership = result_membership.or(snapshot_membership);
    if let Some(membership) = membership {
        details.insert(
            "membership_local_policy_revision".into(),
            membership.local_policy_revision.to_string(),
        );
        details.insert(
            "policy_confirmation_required".into(),
            membership.policy_confirmation_required.to_string(),
        );
        insert_optional_audit_detail(
            details,
            "network_policy_revision",
            membership.network_policy_revision,
        );
        insert_optional_audit_detail(
            details,
            "privacy_policy_revision",
            membership.privacy_policy_revision,
        );
        insert_optional_audit_detail(details, "terms_hash", membership.terms_hash.as_ref());
        insert_optional_audit_detail(
            details,
            "exit_contribution",
            membership
                .exit_contribution
                .map(exit_contribution_obligation_audit_value),
        );
        insert_optional_audit_detail(
            details,
            "exit_contribution_scope",
            membership
                .exit_contribution_scope
                .map(exit_contribution_scope_audit_value),
        );
        insert_optional_audit_detail(
            details,
            "acknowledged_at_unix_ms",
            membership.acknowledged_at_unix_ms,
        );
    }

    // A rejected command still carries server-validated policy facts through
    // the stable problem params. Copy only the typed allowlist, never prose.
    if let Some(NodeCommandResult {
        outcome: NodeCommandOutcome::Rejected(problem),
        ..
    }) = result
    {
        for (source, target) in [
            ("policy_exit_contribution", "exit_contribution"),
            ("policy_exit_contribution_scope", "exit_contribution_scope"),
            ("policy_network_revision", "network_policy_revision"),
            ("policy_privacy_revision", "privacy_policy_revision"),
            ("policy_terms_hash", "terms_hash"),
        ] {
            if let Some(value) = problem.params.get(source) {
                details.insert(target.into(), value.clone());
            }
        }
    }

    if let Some(snapshot) = snapshot.or_else(|| match result.map(|result| &result.outcome) {
        Some(NodeCommandOutcome::NetworkJoined(joined)) => Some(&joined.snapshot),
        _ => None,
    }) {
        details.insert(
            "local_exit_capability".into(),
            snapshot.exit_node_capabilities.supported.to_string(),
        );
        let effective_name = network_name.as_deref().or_else(|| {
            network_id.as_deref().and_then(|id| {
                snapshot
                    .memberships
                    .iter()
                    .find(|membership| membership.network_id == id)
                    .map(|membership| membership.network_name.as_str())
            })
        });
        let local_exit_enabled = effective_name.is_some_and(|name| {
            snapshot
                .networks
                .iter()
                .find(|network| network.name == name)
                .is_some_and(|network| network.roles.contains(&NodeNetworkRole::ExitNode))
        });
        details.insert("local_exit_enabled".into(), local_exit_enabled.to_string());
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
        Self::spawn_with_activity_state(handler, events, capacity, None, None)
    }

    /// Spawn a command bus whose security-relevant operations are committed to
    /// the durable owner audit chain before a result is returned. Production
    /// interactive sessions use this constructor; in-memory-only buses are
    /// reserved for isolated tests and the internal data-plane executor whose
    /// authenticated front-end bus records the same outcome.
    pub(crate) fn spawn_with_durable_audit<H>(
        handler: Arc<H>,
        events: NodeEventBus,
        capacity: usize,
        audit: Arc<OwnerSecurityAudit>,
    ) -> Self
    where
        H: NodeCommandHandler,
    {
        Self::spawn_with_activity_state(handler, events, capacity, None, Some(audit))
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
        Self::spawn_with_activity_state(handler, events, capacity, Some(state), None)
    }

    fn spawn_with_activity_state<H>(
        handler: Arc<H>,
        events: NodeEventBus,
        capacity: usize,
        activity_state: Option<NodeStateStore>,
        durable_audit: Option<Arc<OwnerSecurityAudit>>,
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
                                .handle_authorized_cancellable(
                                    envelope.context,
                                    envelope.request,
                                    envelope.cancel,
                                )
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
            audit: Arc::new(CommandAuditTrail::new(durable_audit)),
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
        let (_cancel_sender, cancel) = watch::channel(false);
        self.execute_cancellable(context, request, cancel).await
    }

    /// Execute one command with a cancellation signal that reaches the active
    /// handler. Cancellation is cooperative: the operation is not marked
    /// terminal until the handler has released child processes and temp state.
    pub async fn execute_cancellable(
        &self,
        context: NodeCommandContext,
        request: NodeCommandRequest,
        cancel: watch::Receiver<bool>,
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
                Some(&result),
                AuditOutcome::Denied,
                &denial.code,
                Some(&denial.reason),
            )
            .await?;
            return Ok(result);
        }

        let (response, receiver) = oneshot::channel();
        let audit_request = request.clone();
        if self
            .sender
            .send(CommandEnvelope {
                context: context.clone(),
                request,
                cancel,
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
                    Some(&result),
                    if result.is_success() {
                        AuditOutcome::Succeeded
                    } else {
                        AuditOutcome::Failed
                    },
                    "failed",
                    None,
                )
                .await?;
                Ok(result)
            }
            Err(_) => {
                self.record_and_publish_activity(
                    &context,
                    &audit_request,
                    None,
                    AuditOutcome::Failed,
                    "response_dropped",
                    None,
                )
                .await?;
                Err(NodeCommandBusError::ResponseDropped(request_id))
            }
        }
    }

    async fn record_and_publish_activity(
        &self,
        context: &NodeCommandContext,
        request: &NodeCommandRequest,
        result: Option<&NodeCommandResult>,
        outcome: AuditOutcome,
        reason: &str,
        reason_detail: Option<&str>,
    ) -> Result<(), NodeCommandBusError> {
        // Policy tests are read-only simulations. Do not append audit/activity
        // state, and do not republish the snapshot merely because the test
        // command completed. Denials are simulated too: no candidate policy is
        // being applied, so there is no durable security transition to record.
        if request.command.kind() == NodeCommandKind::TestPolicy {
            return Ok(());
        }
        let snapshot = match &self.activity_state {
            Some(state) => Some(state.snapshot().await),
            None => None,
        };
        let reason = activity_reason_code(result, outcome, reason);
        self.audit
            .record(
                context,
                request,
                result,
                snapshot.as_ref(),
                outcome,
                (&reason, reason_detail),
            )
            .map_err(|error| NodeCommandBusError::AuditUnavailable(format!("{error:#}")))?;
        if let Some(state) = &self.activity_state {
            state
                .set_recent_activity(self.audit.recent_activity())
                .await;
        }
        Ok(())
    }

    /// Read the sanitized authorization/execution audit trail. This is used by
    /// tests and by future owner-only audit projections.
    pub fn audit_entries(&self) -> Vec<AuditEntry> {
        self.audit.entries()
    }

    /// Shared durable ledger used by this command bus. Interactive transports
    /// attach it here so owner-security request records and command records use
    /// one global sequence and hash chain.
    pub(crate) fn durable_audit(&self) -> Option<Arc<OwnerSecurityAudit>> {
        self.audit.durable.clone()
    }

    #[cfg(test)]
    pub(crate) async fn execute_test(
        &self,
        request: NodeCommandRequest,
    ) -> Result<NodeCommandResult, NodeCommandBusError> {
        let context = test_owner_context().with_test_network_authority_for(&request.command);
        self.execute(context, request).await
    }
}

fn activity_reason_code(
    result: Option<&NodeCommandResult>,
    outcome: AuditOutcome,
    fallback: &str,
) -> String {
    if outcome == AuditOutcome::Succeeded {
        return "succeeded".into();
    }
    if let Some(NodeCommandResult {
        outcome: NodeCommandOutcome::Rejected(problem),
        ..
    }) = result
    {
        return problem
            .params
            .get("authorization_code")
            .or_else(|| problem.params.get("reason"))
            .filter(|code| !code.is_empty())
            .cloned()
            .or_else(|| (!problem.message_key.is_empty()).then(|| problem.message_key.clone()))
            .unwrap_or_else(|| fallback.into());
    }
    fallback.into()
}

pub(crate) fn denied(request_id: u64, denial: &CommandDenial) -> NodeCommandResult {
    let code = if denial.code == PermissionDenialCode::RecentAuthenticationRequired.as_str() {
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
    #[error("安全审计写入失败，命令结果不可用: {0}")]
    AuditUnavailable(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::NodePermission;
    use crate::owner_security::audit::OwnerSecurityAudit;
    use fuckipv6_common::domain::{
        AuditExportFilter, AuditRetentionPolicy, PermissionCondition, PermissionEffect,
        PermissionRule, PolicyAction, ResourceKind, SecurityRuleId, SubjectSelector,
    };
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    static NEXT_DURABLE_AUDIT_DIR: AtomicU64 = AtomicU64::new(0);

    fn durable_audit_dir(name: &str) -> std::path::PathBuf {
        let sequence = NEXT_DURABLE_AUDIT_DIR.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "fuckipv6-command-audit-{name}-{}-{sequence}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("durable audit directory");
        dir
    }

    fn invitation_code() -> InvitationCode {
        InvitationCode::parse(format!("fkinv_{}", "A".repeat(43))).expect("valid test code")
    }

    fn invitation_network_id() -> NetworkId {
        NetworkId::from_public_key(b"invitation-command-network")
    }

    fn invitation_id() -> InvitationId {
        InvitationId::from_public_key(b"invitation-command")
    }

    fn invitation_hash(label: &[u8]) -> ContentHash {
        ContentHash::from_bytes(label)
    }

    fn valid_redeem_command() -> NodeRedeemInvitationCommand {
        NodeRedeemInvitationCommand {
            coordinator_url: "wss://coordinator.example.test/signaling".into(),
            request: InvitationRedeemRequest {
                network_id: invitation_network_id(),
                code: invitation_code(),
                invitation_id: invitation_id(),
                expected_invitation_revision: 1,
                preview_digest: invitation_hash(b"preview"),
                terms_hash: invitation_hash(b"terms"),
                acknowledged_at_unix_ms: 1_700_000_000_000,
                exit_contribution_acknowledged: None,
            },
        }
    }

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
                purpose: "A test network".into(),
                icon: DEFAULT_NETWORK_ICON.into(),
                language: DEFAULT_NETWORK_LANGUAGE.into(),
                region: default_network_region(),
                tags: Vec::new(),
                capability_declarations: Vec::new(),
                visibility: NetworkVisibility::Unlisted,
                join_mode: NetworkJoinMode::InviteOnly,
                exit_contribution: Default::default(),
                exit_contribution_scope: Default::default(),
                signaling_port: 6666,
                relay_port: 6667,
                external_host: Some("vpn.example.com".into()),
                auto_connect: true,
            }),
            NodeCommand::PreviewNetworkProfile(NodeNetworkProfileCommand {
                network_name: "home".into(),
                expected_revision: 1,
                update: NetworkProfileUpdate {
                    purpose: "A test network".into(),
                    icon: DEFAULT_NETWORK_ICON.into(),
                    language: DEFAULT_NETWORK_LANGUAGE.into(),
                    region: default_network_region(),
                    tags: Vec::new(),
                    capability_declarations: Vec::new(),
                    visibility: NetworkVisibility::Unlisted,
                    join_mode: NetworkJoinMode::InviteOnly,
                    exit_contribution: Default::default(),
                    exit_contribution_scope: Default::default(),
                },
            }),
            NodeCommand::UpdateNetworkProfile(NodeNetworkProfileCommand {
                network_name: "home".into(),
                expected_revision: 1,
                update: NetworkProfileUpdate {
                    purpose: "A test network".into(),
                    icon: DEFAULT_NETWORK_ICON.into(),
                    language: DEFAULT_NETWORK_LANGUAGE.into(),
                    region: default_network_region(),
                    tags: Vec::new(),
                    capability_declarations: Vec::new(),
                    visibility: NetworkVisibility::Unlisted,
                    join_mode: NetworkJoinMode::InviteOnly,
                    exit_contribution: Default::default(),
                    exit_contribution_scope: Default::default(),
                },
            }),
            NodeCommand::PublishNetworkDiscovery(NodeNetworkDiscoveryCommand {
                network_name: "home".into(),
                index_url: "https://discovery.example.com".into(),
            }),
            NodeCommand::UpdateNetworkDiscovery(NodeNetworkDiscoveryCommand {
                network_name: "home".into(),
                index_url: "https://discovery.example.com".into(),
            }),
            NodeCommand::UnpublishNetworkDiscovery(NodeNetworkDiscoveryCommand {
                network_name: "home".into(),
                index_url: "https://discovery.example.com".into(),
            }),
            NodeCommand::RefreshDiscoveryIndex {
                index_url: Some("https://discovery.example.com".into()),
            },
            NodeCommand::RetryDiscoveryIndex,
            NodeCommand::PreviewJoinNetwork(crate::node::NodePreviewJoinNetworkCommand {
                network_id: fuckipv6_common::domain::NetworkId::from_public_key(b"preview-network")
                    .to_string(),
            }),
            NodeCommand::JoinNetwork(Box::new(NodeJoinNetworkCommand {
                target: "fk://invite".into(),
                network_name: Some("office".into()),
                start: true,
                discovery_record: None,
                network_id: None,
                preview_digest: None,
                network_policy_revision: None,
                privacy_policy_revision: None,
                terms_hash: None,
                acknowledged_at_unix_ms: None,
                exit_contribution_acknowledged: None,
            })),
            NodeCommand::SubmitJoinRequest(NodeSubmitJoinRequestCommand {
                network_id: fuckipv6_common::domain::NetworkId::from_public_key(
                    b"join-request-network",
                )
                .to_string(),
                reason: "Please allow this node to join for file collaboration.".into(),
            }),
            NodeCommand::ListJoinRequests(NodeListJoinRequestsCommand {
                network_id: fuckipv6_common::domain::NetworkId::from_public_key(
                    b"join-request-network",
                )
                .to_string(),
            }),
            NodeCommand::ListNetworkMembers(NodeNetworkMembersCommand {
                network_id: fuckipv6_common::domain::NetworkId::from_public_key(
                    b"join-request-network",
                )
                .to_string(),
            }),
            NodeCommand::PreviewMemberRemoval(NodeMemberRemovalPreviewCommand {
                network_id: fuckipv6_common::domain::NetworkId::from_public_key(
                    b"join-request-network",
                )
                .to_string(),
                target_node_id: fuckipv6_common::domain::NodeId::from_public_key(
                    b"member-removal-target",
                )
                .to_string(),
            }),
            NodeCommand::ChangeMemberLifecycle(NodeChangeMemberLifecycleCommand {
                network_id: fuckipv6_common::domain::NetworkId::from_public_key(
                    b"join-request-network",
                )
                .to_string(),
                target_node_id: fuckipv6_common::domain::NodeId::from_public_key(
                    b"member-lifecycle-target",
                )
                .to_string(),
                action: MemberLifecycleAction::Remove,
                role: MembershipRole::Member,
                expected_revision: 1,
                reason: Some("No longer participates in this network.".into()),
            }),
            NodeCommand::ReviewJoinRequest(NodeReviewJoinRequestCommand {
                network_id: fuckipv6_common::domain::NetworkId::from_public_key(
                    b"join-request-network",
                )
                .to_string(),
                review: NodeJoinRequestReviewItem {
                    request_id: fuckipv6_common::domain::JoinRequestId::from_public_key(
                        b"join-request",
                    )
                    .to_string(),
                    expected_revision: 1,
                    action: JoinRequestReviewAction::Approve,
                    note: None,
                },
            }),
            NodeCommand::BatchReviewJoinRequests(NodeBatchReviewJoinRequestsCommand {
                network_id: fuckipv6_common::domain::NetworkId::from_public_key(
                    b"join-request-network",
                )
                .to_string(),
                reviews: vec![NodeJoinRequestReviewItem {
                    request_id: fuckipv6_common::domain::JoinRequestId::from_public_key(
                        b"join-request-batch",
                    )
                    .to_string(),
                    expected_revision: 1,
                    action: JoinRequestReviewAction::RequestInformation,
                    note: Some("Please confirm the intended file-sharing scope.".into()),
                }],
            }),
            NodeCommand::WithdrawJoinRequest(NodeWithdrawJoinRequestCommand {
                network_id: fuckipv6_common::domain::NetworkId::from_public_key(
                    b"join-request-network",
                )
                .to_string(),
                request_id: fuckipv6_common::domain::JoinRequestId::from_public_key(
                    b"withdraw-join-request",
                )
                .to_string(),
            }),
            NodeCommand::ResubmitJoinRequest(NodeResubmitJoinRequestCommand {
                network_id: fuckipv6_common::domain::NetworkId::from_public_key(
                    b"join-request-network",
                )
                .to_string(),
                request_id: fuckipv6_common::domain::JoinRequestId::from_public_key(
                    b"resubmit-join-request",
                )
                .to_string(),
                expected_revision: 2,
                reason: "Updated reason after the reviewer requested clarification.".into(),
            }),
            NodeCommand::CreateInvitation(NodeCreateInvitationCommand {
                network_id: invitation_network_id(),
                role: MembershipRole::Member,
                capabilities: vec![CapabilityKind::File],
                max_uses: 2,
                expires_in_ms: 86_400_000,
            }),
            NodeCommand::ListInvitations(NodeListInvitationsCommand {
                network_id: invitation_network_id(),
            }),
            NodeCommand::GetInvitationStatus(NodeGetInvitationStatusCommand {
                network_id: invitation_network_id(),
                invitation_id: invitation_id(),
            }),
            NodeCommand::RevokeInvitation(NodeRevokeInvitationCommand {
                network_id: invitation_network_id(),
                invitation_id: invitation_id(),
                expected_revision: 1,
            }),
            NodeCommand::PreviewInvitation(NodePreviewInvitationCommand {
                network_id: invitation_network_id(),
                coordinator_url: "wss://coordinator.example.test/signaling".into(),
                code: invitation_code(),
            }),
            NodeCommand::RedeemInvitation(valid_redeem_command()),
            NodeCommand::LeaveNetwork(crate::node::NodeLeaveNetworkCommand {
                network_name: "office".into(),
            }),
            NodeCommand::AcknowledgeMembershipPolicy(NodeAcknowledgeMembershipPolicyCommand {
                network_id: invitation_network_id().to_string(),
                expected_terms_hash: invitation_hash(b"membership policy"),
                acknowledged_at_unix_ms: 1_700_000_000_000,
                exit_contribution_acknowledged: Some(true),
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
            NodeCommand::RotateNodeIdentity(NodeRotateNodeIdentityCommand {
                expected_fingerprint: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
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
            NodeCommand::GetPolicy,
            NodeCommand::PreviewPolicy(NodePreviewPolicyCommand {
                expected_revision: 1,
                candidate: PolicySnapshot::default(),
            }),
            NodeCommand::PreviewPolicyRollback(NodePreviewPolicyRollbackCommand {
                expected_revision: 1,
                target_revision: 1,
            }),
            NodeCommand::ApplyPolicy(NodeApplyPolicyCommand {
                expected_revision: 1,
                candidate: PolicySnapshot::default(),
                network_ids: vec![fuckipv6_common::domain::NetworkId::from_public_key(
                    b"policy-command-network",
                )
                .to_string()],
                includes_local_policy: false,
                confirm_privilege_expansion: true,
            }),
            NodeCommand::RollbackPolicy(NodeRollbackPolicyCommand {
                expected_revision: 1,
                target_revision: 1,
                network_ids: vec![fuckipv6_common::domain::NetworkId::from_public_key(
                    b"policy-command-network",
                )
                .to_string()],
                includes_local_policy: false,
                confirm_privilege_expansion: true,
            }),
            NodeCommand::GetPolicyHistory,
            NodeCommand::TestPolicy(fuckipv6_common::domain::PolicyTestInput {
                network_id: fuckipv6_common::domain::NetworkId::from_public_key(
                    b"policy-test-network",
                ),
                actor: fuckipv6_common::domain::NodeId::from_public_key(b"policy-test-actor"),
                role: fuckipv6_common::domain::NetworkRole::Member,
                groups: Default::default(),
                membership_active: true,
                recent_reauthentication: true,
                emergency_coordinator: false,
                request: fuckipv6_common::domain::PolicyTestRequest {
                    action: fuckipv6_common::domain::PolicyAction::Read,
                    resource: fuckipv6_common::domain::ResourceKind::Network,
                    resource_network_id: fuckipv6_common::domain::NetworkId::from_public_key(
                        b"policy-test-network",
                    ),
                    object_id: None,
                    intent: fuckipv6_common::domain::PermissionIntent::ReadOnly,
                },
                local_policy: None,
                network_acl: None,
                now_unix_ms: 1_000,
            }),
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
    fn command_kind_inventory_and_sensitive_reauth_contract_are_complete() {
        assert_eq!(NodeCommandKind::ALL.len(), 61);
        assert_eq!(NodeCommandKind::ACTIONS.len(), 60);
        for kind in [
            NodeCommandKind::CreateInvitation,
            NodeCommandKind::RevokeInvitation,
            NodeCommandKind::PreviewInvitation,
            NodeCommandKind::RedeemInvitation,
            NodeCommandKind::ChangeMemberLifecycle,
        ] {
            assert!(kind.requires_recent_reauthentication(), "{kind:?}");
        }
        for kind in [
            NodeCommandKind::ListInvitations,
            NodeCommandKind::GetInvitationStatus,
        ] {
            assert!(!kind.requires_recent_reauthentication(), "{kind:?}");
        }
    }

    #[test]
    fn invitation_debug_output_never_contains_the_credential() {
        let code = invitation_code();
        let plaintext = code.as_str().to_owned();
        let preview = NodeCommand::PreviewInvitation(NodePreviewInvitationCommand {
            network_id: invitation_network_id(),
            coordinator_url: "wss://coordinator.example.test/signaling".into(),
            code: code.clone(),
        });
        let redeem = NodeCommand::RedeemInvitation(valid_redeem_command());
        assert!(!format!("{preview:?}").contains(&plaintext));
        assert!(!format!("{redeem:?}").contains(&plaintext));
        assert!(format!("{preview:?}").contains("[redacted]"));
        assert!(format!("{redeem:?}").contains("[redacted]"));
    }

    #[test]
    fn invitation_commands_reject_non_websocket_and_missing_consent() {
        let mut preview = NodePreviewInvitationCommand {
            network_id: invitation_network_id(),
            coordinator_url: "https://coordinator.example.test/?code=leak".into(),
            code: invitation_code(),
        };
        assert_eq!(
            NodeCommand::PreviewInvitation(preview.clone())
                .validate()
                .unwrap_err()
                .code,
            NodeProblemCode::InvalidRequest
        );
        preview.coordinator_url = "ws://coordinator.example.test/signaling".into();
        NodeCommand::PreviewInvitation(preview)
            .validate()
            .expect("websocket coordinator URL is accepted");

        let mut redeem = valid_redeem_command();
        redeem.request.acknowledged_at_unix_ms = 0;
        assert_eq!(
            NodeCommand::RedeemInvitation(redeem)
                .validate()
                .unwrap_err()
                .code,
            NodeProblemCode::InvalidRequest
        );
    }

    #[test]
    fn created_network_debug_redacts_the_invitation_ticket() {
        let created = NodeCreatedNetwork {
            network_name: "home".into(),
            network_id: "net_home".into(),
            public_key: "public-key".into(),
            owner_role: MembershipRole::Owner,
            config_saved: true,
            started: true,
            next_steps: vec![
                NodeNetworkCreationNextStep::InviteMembers,
                NodeNetworkCreationNextStep::ConfigurePolicy,
                NodeNetworkCreationNextStep::ContributeCapabilities,
            ],
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
            purpose: "A test network".into(),
            icon: DEFAULT_NETWORK_ICON.into(),
            language: DEFAULT_NETWORK_LANGUAGE.into(),
            region: default_network_region(),
            tags: Vec::new(),
            capability_declarations: Vec::new(),
            visibility: NetworkVisibility::Unlisted,
            join_mode: NetworkJoinMode::InviteOnly,
            exit_contribution: Default::default(),
            exit_contribution_scope: Default::default(),
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
            purpose: "A test network".into(),
            icon: DEFAULT_NETWORK_ICON.into(),
            language: DEFAULT_NETWORK_LANGUAGE.into(),
            region: default_network_region(),
            tags: Vec::new(),
            capability_declarations: Vec::new(),
            visibility: NetworkVisibility::Unlisted,
            join_mode: NetworkJoinMode::InviteOnly,
            exit_contribution: Default::default(),
            exit_contribution_scope: Default::default(),
            signaling_port: 6666,
            relay_port: 6667,
            external_host: None,
            auto_connect: true,
        }));
    }

    fn valid_create_network_command() -> NodeCreateNetworkCommand {
        NodeCreateNetworkCommand {
            name: "home".into(),
            purpose: "Family devices across two homes".into(),
            icon: "home".into(),
            language: "zh-CN".into(),
            region: "CN".into(),
            tags: Vec::new(),
            capability_declarations: Vec::new(),
            visibility: NetworkVisibility::Private,
            join_mode: NetworkJoinMode::InviteOnly,
            exit_contribution: Default::default(),
            exit_contribution_scope: Default::default(),
            signaling_port: 6666,
            relay_port: 6667,
            external_host: Some("vpn.example.com".into()),
            auto_connect: false,
        }
    }

    #[test]
    fn network_create_profile_round_trips_new_public_fields() {
        let command = NodeCommand::CreateNetwork(valid_create_network_command());
        let encoded = serde_json::to_value(&command).expect("serialize create command");
        let decoded: NodeCommand =
            serde_json::from_value(encoded.clone()).expect("deserialize create command");

        assert_eq!(decoded, command);
        assert_eq!(
            encoded["payload"]["purpose"],
            "Family devices across two homes"
        );
        assert_eq!(encoded["payload"]["icon"], "home");
        assert_eq!(encoded["payload"]["language"], "zh-CN");
        assert_eq!(encoded["payload"]["visibility"], "private");
        assert_eq!(encoded["payload"]["join_mode"], "invite_only");
    }

    #[test]
    fn legacy_network_create_payload_uses_safe_public_defaults() {
        let command: NodeCommand = serde_json::from_value(serde_json::json!({
            "command": "create_network",
            "payload": {
                "name": "legacy",
                "signaling_port": 6666,
                "relay_port": 6667,
                "external_host": null,
                "auto_connect": false
            }
        }))
        .expect("legacy create payload decodes");

        let NodeCommand::CreateNetwork(command) = &command else {
            panic!("expected create command");
        };
        assert_eq!(command.purpose, "Owner-created network");
        assert_eq!(command.icon, DEFAULT_NETWORK_ICON);
        assert_eq!(command.language, DEFAULT_NETWORK_LANGUAGE);
        assert_eq!(command.visibility, NetworkVisibility::Unlisted);
        assert_eq!(command.join_mode, NetworkJoinMode::InviteOnly);
        command.validate().expect("legacy payload remains valid");
    }

    #[test]
    fn malformed_public_profile_fields_return_structured_field_issues() {
        let mut create = valid_create_network_command();
        create.purpose = "  ".into();
        create.icon = "Home Icon".into();
        create.language = "zh_CN".into();

        let problem = NodeCommand::CreateNetwork(create)
            .validate()
            .expect_err("malformed profile must fail");
        assert_eq!(problem.code, NodeProblemCode::InvalidRequest);
        let issues = problem
            .details
            .as_ref()
            .expect("validation details")
            .validation_issues
            .iter()
            .map(|issue| (issue.path.as_str(), issue.code.as_str()))
            .collect::<Vec<_>>();
        assert!(issues.contains(&("purpose", "invalid_network_purpose")));
        assert!(issues.contains(&("icon", "invalid_network_icon")));
        assert!(issues.contains(&("language", "invalid_network_language")));
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
                purpose: "A test network".into(),
                icon: DEFAULT_NETWORK_ICON.into(),
                language: DEFAULT_NETWORK_LANGUAGE.into(),
                region: default_network_region(),
                tags: Vec::new(),
                capability_declarations: Vec::new(),
                visibility: NetworkVisibility::Unlisted,
                join_mode: NetworkJoinMode::InviteOnly,
                exit_contribution: Default::default(),
                exit_contribution_scope: Default::default(),
                signaling_port: 0,
                relay_port: 6667,
                external_host: None,
                auto_connect: true,
            }),
            NodeCommand::CreateNetwork(NodeCreateNetworkCommand {
                name: "home".into(),
                purpose: "A test network".into(),
                icon: DEFAULT_NETWORK_ICON.into(),
                language: DEFAULT_NETWORK_LANGUAGE.into(),
                region: default_network_region(),
                tags: Vec::new(),
                capability_declarations: Vec::new(),
                visibility: NetworkVisibility::Unlisted,
                join_mode: NetworkJoinMode::InviteOnly,
                exit_contribution: Default::default(),
                exit_contribution_scope: Default::default(),
                signaling_port: 6666,
                relay_port: 6666,
                external_host: None,
                auto_connect: true,
            }),
            NodeCommand::CreateNetwork(NodeCreateNetworkCommand {
                name: "home".into(),
                purpose: "A test network".into(),
                icon: DEFAULT_NETWORK_ICON.into(),
                language: DEFAULT_NETWORK_LANGUAGE.into(),
                region: default_network_region(),
                tags: Vec::new(),
                capability_declarations: Vec::new(),
                visibility: NetworkVisibility::Unlisted,
                join_mode: NetworkJoinMode::InviteOnly,
                exit_contribution: Default::default(),
                exit_contribution_scope: Default::default(),
                signaling_port: 6666,
                relay_port: 6667,
                external_host: Some("  ".into()),
                auto_connect: true,
            }),
            NodeCommand::JoinNetwork(Box::new(NodeJoinNetworkCommand {
                target: "  ".into(),
                network_name: None,
                start: false,
                discovery_record: None,
                network_id: None,
                preview_digest: None,
                network_policy_revision: None,
                privacy_policy_revision: None,
                terms_hash: None,
                acknowledged_at_unix_ms: None,
                exit_contribution_acknowledged: None,
            })),
            NodeCommand::JoinNetwork(Box::new(NodeJoinNetworkCommand {
                target: "ws://127.0.0.1:6666".into(),
                network_name: Some("../office".into()),
                start: false,
                discovery_record: None,
                network_id: None,
                preview_digest: None,
                network_policy_revision: None,
                privacy_policy_revision: None,
                terms_hash: None,
                acknowledged_at_unix_ms: None,
                exit_contribution_acknowledged: None,
            })),
            NodeCommand::SubmitJoinRequest(NodeSubmitJoinRequestCommand {
                network_id: "not-a-network-id".into(),
                reason: "A valid reason".into(),
            }),
            NodeCommand::SubmitJoinRequest(NodeSubmitJoinRequestCommand {
                network_id: fuckipv6_common::domain::NetworkId::from_public_key(
                    b"join-request-network",
                )
                .to_string(),
                reason: "  ".into(),
            }),
            NodeCommand::ListJoinRequests(NodeListJoinRequestsCommand {
                network_id: String::new(),
            }),
            NodeCommand::ReviewJoinRequest(NodeReviewJoinRequestCommand {
                network_id: fuckipv6_common::domain::NetworkId::from_public_key(
                    b"join-request-network",
                )
                .to_string(),
                review: NodeJoinRequestReviewItem {
                    request_id: fuckipv6_common::domain::JoinRequestId::from_public_key(
                        b"join-request",
                    )
                    .to_string(),
                    expected_revision: 0,
                    action: JoinRequestReviewAction::Approve,
                    note: None,
                },
            }),
            NodeCommand::BatchReviewJoinRequests(NodeBatchReviewJoinRequestsCommand {
                network_id: fuckipv6_common::domain::NetworkId::from_public_key(
                    b"join-request-network",
                )
                .to_string(),
                reviews: Vec::new(),
            }),
            NodeCommand::WithdrawJoinRequest(NodeWithdrawJoinRequestCommand {
                network_id: fuckipv6_common::domain::NetworkId::from_public_key(
                    b"join-request-network",
                )
                .to_string(),
                request_id: "not-a-request-id".into(),
            }),
            NodeCommand::ResubmitJoinRequest(NodeResubmitJoinRequestCommand {
                network_id: fuckipv6_common::domain::NetworkId::from_public_key(
                    b"join-request-network",
                )
                .to_string(),
                request_id: fuckipv6_common::domain::JoinRequestId::from_public_key(
                    b"resubmit-join-request",
                )
                .to_string(),
                expected_revision: 2,
                reason: "界".repeat(JOIN_REQUEST_MAX_REASON_CHARS + 1),
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
            NodeCommand::RotateNodeIdentity(NodeRotateNodeIdentityCommand {
                expected_fingerprint: "not-a-fingerprint".into(),
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

    fn join_request_network_id() -> String {
        fuckipv6_common::domain::NetworkId::from_public_key(b"join-request-command-network")
            .to_string()
    }

    fn join_request_id(label: &str) -> String {
        fuckipv6_common::domain::JoinRequestId::from_public_key(label.as_bytes()).to_string()
    }

    fn review_item(
        request_id: String,
        action: JoinRequestReviewAction,
        note: Option<&str>,
    ) -> NodeJoinRequestReviewItem {
        NodeJoinRequestReviewItem {
            request_id,
            expected_revision: 1,
            action,
            note: note.map(str::to_string),
        }
    }

    #[test]
    fn join_request_commands_enforce_bounded_review_contract() {
        let network_id = join_request_network_id();
        let request_id = join_request_id("bounded-request");
        let exact_reason = "界".repeat(JOIN_REQUEST_MAX_REASON_CHARS);
        NodeCommand::SubmitJoinRequest(NodeSubmitJoinRequestCommand {
            network_id: network_id.clone(),
            reason: exact_reason,
        })
        .validate()
        .expect("Unicode reason at the scalar-value limit is accepted");

        let problem = NodeCommand::SubmitJoinRequest(NodeSubmitJoinRequestCommand {
            network_id: network_id.clone(),
            reason: "界".repeat(JOIN_REQUEST_MAX_REASON_CHARS + 1),
        })
        .validate()
        .expect_err("Unicode reason above the scalar-value limit is rejected");
        assert_eq!(problem.message_key, "join_request.error.invalid_reason");

        let exact_note = "审".repeat(JOIN_REQUEST_MAX_INFORMATION_REQUEST_CHARS);
        NodeCommand::ReviewJoinRequest(NodeReviewJoinRequestCommand {
            network_id: network_id.clone(),
            review: review_item(
                request_id.clone(),
                JoinRequestReviewAction::RequestInformation,
                Some(&exact_note),
            ),
        })
        .validate()
        .expect("information request at the scalar-value limit is accepted");

        for review in [
            review_item(
                request_id.clone(),
                JoinRequestReviewAction::Approve,
                Some("approval note"),
            ),
            review_item(
                request_id.clone(),
                JoinRequestReviewAction::RequestInformation,
                None,
            ),
            review_item(
                request_id.clone(),
                JoinRequestReviewAction::RequestInformation,
                Some("  padded  "),
            ),
        ] {
            assert!(
                NodeCommand::ReviewJoinRequest(NodeReviewJoinRequestCommand {
                    network_id: network_id.clone(),
                    review,
                })
                .validate()
                .is_err()
            );
        }

        let problem = NodeCommand::ReviewJoinRequest(NodeReviewJoinRequestCommand {
            network_id: network_id.clone(),
            review: NodeJoinRequestReviewItem {
                expected_revision: 0,
                ..review_item(request_id, JoinRequestReviewAction::Approve, None)
            },
        })
        .validate()
        .expect_err("revision zero is rejected");
        assert_eq!(problem.message_key, "join_request.error.invalid_revision");
    }

    #[test]
    fn batch_join_request_review_rejects_duplicates_and_unbounded_batches() {
        let network_id = join_request_network_id();
        let request_id = join_request_id("duplicate-batch-request");
        let duplicate = review_item(request_id.clone(), JoinRequestReviewAction::Approve, None);
        let problem = NodeCommand::BatchReviewJoinRequests(NodeBatchReviewJoinRequestsCommand {
            network_id: network_id.clone(),
            reviews: vec![duplicate.clone(), duplicate],
        })
        .validate()
        .expect_err("duplicate request IDs are rejected");
        assert_eq!(problem.message_key, "join_request.error.batch_duplicate");

        let reviews = (0..101)
            .map(|index| {
                review_item(
                    join_request_id(&format!("large-batch-request-{index}")),
                    JoinRequestReviewAction::Approve,
                    None,
                )
            })
            .collect();
        let problem = NodeCommand::BatchReviewJoinRequests(NodeBatchReviewJoinRequestsCommand {
            network_id,
            reviews,
        })
        .validate()
        .expect_err("batches above 100 requests are rejected");
        assert_eq!(problem.message_key, "join_request.error.batch_too_large");
    }

    #[test]
    fn join_request_command_debug_redacts_prose() {
        let network_id = join_request_network_id();
        let request_id = join_request_id("debug-request");
        let submit = NodeCommand::SubmitJoinRequest(NodeSubmitJoinRequestCommand {
            network_id: network_id.clone(),
            reason: "SECRET-SUBMISSION-REASON".into(),
        });
        let review = NodeCommand::BatchReviewJoinRequests(NodeBatchReviewJoinRequestsCommand {
            network_id: network_id.clone(),
            reviews: vec![review_item(
                request_id.clone(),
                JoinRequestReviewAction::RequestInformation,
                Some("SECRET-REVIEWER-NOTE"),
            )],
        });
        let resubmit = NodeCommand::ResubmitJoinRequest(NodeResubmitJoinRequestCommand {
            network_id,
            request_id,
            expected_revision: 2,
            reason: "SECRET-RESUBMISSION-REASON".into(),
        });

        let debug = format!(
            "{submit:?}
{review:?}
{resubmit:?}"
        );
        assert!(debug.contains("[redacted]"));
        for secret in [
            "SECRET-SUBMISSION-REASON",
            "SECRET-REVIEWER-NOTE",
            "SECRET-RESUBMISSION-REASON",
        ] {
            assert!(!debug.contains(secret), "{debug}");
        }
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
            NodeCommandKind::PreviewNetworkProfile,
            NodeCommandKind::PreviewJoinNetwork,
            NodeCommandKind::ListJoinRequests,
            NodeCommandKind::ListNetworkMembers,
            NodeCommandKind::PreviewMemberRemoval,
            NodeCommandKind::ListInvitations,
            NodeCommandKind::GetInvitationStatus,
            NodeCommandKind::PreviewInvitation,
            NodeCommandKind::PreviewConfiguration,
            NodeCommandKind::GetConfigurationHistory,
            NodeCommandKind::GetPolicy,
            NodeCommandKind::PreviewPolicy,
            NodeCommandKind::PreviewPolicyRollback,
            NodeCommandKind::GetPolicyHistory,
            NodeCommandKind::TestPolicy,
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
                    | NodeCommandKind::PreviewNetworkProfile
                    | NodeCommandKind::PreviewJoinNetwork
                    | NodeCommandKind::ListJoinRequests
                    | NodeCommandKind::ListNetworkMembers
                    | NodeCommandKind::PreviewMemberRemoval
                    | NodeCommandKind::ListInvitations
                    | NodeCommandKind::GetInvitationStatus
                    | NodeCommandKind::PreviewInvitation
                    | NodeCommandKind::PreviewConfiguration
                    | NodeCommandKind::GetConfigurationHistory
                    | NodeCommandKind::GetPolicy
                    | NodeCommandKind::PreviewPolicy
                    | NodeCommandKind::PreviewPolicyRollback
                    | NodeCommandKind::GetPolicyHistory
                    | NodeCommandKind::TestPolicy
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
        assert_eq!(audit[0].details["actor_role"], "administrator");
        assert_eq!(audit[0].details["network_id"], network_id.to_string());
        assert_eq!(
            audit[0].details["actor_source"],
            "control-api:member-session"
        );
        assert_eq!(
            audit[0].details["reason_code"],
            "permission.capability_denied"
        );
        assert!(audit[0].details["reason_detail"].contains("capability"));
        let activity = state.snapshot().await.recent_activity;
        assert_eq!(activity.len(), 1);
        assert_eq!(activity[0].outcome, AuditOutcome::Denied);
        assert_eq!(activity[0].details["command"], "start_networks");
    }

    #[tokio::test]
    async fn command_bus_preserves_stable_policy_denial_code_and_reason() {
        let calls = Arc::new(AtomicUsize::new(0));
        let network_id = fuckipv6_common::domain::NetworkId::from_public_key(b"stable-denial-home");
        let deny_rule = PermissionRule {
            rule_id: SecurityRuleId::from_public_key(b"stable-denial-rule"),
            effect: PermissionEffect::Deny,
            subjects: vec![SubjectSelector::AnyMember],
            resources: vec![ResourceKind::SecurityPolicy],
            actions: vec![PolicyAction::Update],
            conditions: vec![PermissionCondition::MembershipActive],
        };
        let deny_rule_id = deny_rule.rule_id.to_string();
        let context = NodeCommandContext::local_owner(actor(), "control-api:owner-session")
            .with_local_policy(vec![deny_rule])
            .with_network_scopes([(
                "home".into(),
                crate::node::NetworkCommandScope::new(
                    network_id,
                    fuckipv6_common::domain::NetworkRole::Owner,
                    fuckipv6_common::domain::MembershipState::Active,
                    Vec::new(),
                ),
            )]);
        let bus = NodeCommandBus::spawn(
            Arc::new(CountingHandler {
                calls: calls.clone(),
            }),
            NodeEventBus::default(),
            4,
        );
        let command = NodeCommand::BanClient(NodeClientBlacklistCommand {
            network_name: "home".into(),
            client_id: "bad-client".into(),
        });

        let result = bus
            .execute(context, NodeCommandRequest::new(54, command))
            .await
            .expect("authorization denial is a command result");

        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let NodeCommandOutcome::Rejected(problem) = result.outcome else {
            panic!("expected authorization rejection");
        };
        assert_eq!(problem.code, NodeProblemCode::PermissionDenied);
        assert_eq!(
            problem.params.get("authorization_code").map(String::as_str),
            Some(PermissionDenialCode::Denied.as_str())
        );
        let audit = bus.audit_entries();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].outcome, AuditOutcome::Denied);
        assert_eq!(audit[0].details["reason_code"], "permission.denied");
        let reason = &audit[0].details["reason_detail"];
        assert!(reason.contains(&deny_rule_id), "reason: {reason}");
        assert!(reason.contains("local layer"), "reason: {reason}");
    }

    #[tokio::test]
    async fn acl_denial_precedes_handler_and_is_audited_without_payload_secrets() {
        let calls = Arc::new(AtomicUsize::new(0));
        let network_id = fuckipv6_common::domain::NetworkId::from_public_key(b"acl-audit-home");
        let now = unix_time_ms();
        let deny_rule = PermissionRule {
            rule_id: SecurityRuleId::from_public_key(b"acl-audit-deny"),
            effect: PermissionEffect::Deny,
            subjects: vec![SubjectSelector::AnyMember],
            resources: vec![ResourceKind::SecurityPolicy],
            actions: vec![PolicyAction::Update],
            conditions: vec![PermissionCondition::MembershipActive],
        };
        let deny_rule_id = deny_rule.rule_id.to_string();
        let acl = fuckipv6_common::domain::NetworkAcl {
            network_id: network_id.clone(),
            revision: 1,
            issued_at_unix_ms: now.saturating_sub(10_000),
            expires_at_unix_ms: now.saturating_add(60_000),
            rules: vec![deny_rule],
            grants: Vec::new(),
        };
        let context = NodeCommandContext::local_owner(actor(), "gpui:owner-session")
            .with_network_scopes([(
                "home".into(),
                crate::node::NetworkCommandScope::new(
                    network_id,
                    fuckipv6_common::domain::NetworkRole::Owner,
                    fuckipv6_common::domain::MembershipState::Active,
                    Vec::new(),
                ),
            )])
            .with_access_control_snapshot(&crate::node::state::NodeAccessControlSnapshot {
                local_policy: None,
                network_acls: vec![acl],
                field_privacy_policies: Vec::new(),
            });
        let bus = NodeCommandBus::spawn(
            Arc::new(CountingHandler {
                calls: calls.clone(),
            }),
            NodeEventBus::default(),
            4,
        );
        let client_id = "payload-secret-client-id";
        let command = NodeCommand::BanClient(NodeClientBlacklistCommand {
            network_name: "home".into(),
            client_id: client_id.into(),
        });

        let result = bus
            .execute(context, NodeCommandRequest::new(59, command))
            .await
            .expect("authorization denial is a command result");

        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let NodeCommandOutcome::Rejected(problem) = result.outcome else {
            panic!("expected authorization rejection");
        };
        assert_eq!(problem.code, NodeProblemCode::PermissionDenied);
        assert_eq!(
            problem.params.get("authorization_code").map(String::as_str),
            Some(PermissionDenialCode::Denied.as_str())
        );
        let audit = bus.audit_entries();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].outcome, AuditOutcome::Denied);
        assert_eq!(audit[0].action, "network.blacklist.ban");
        assert_eq!(audit[0].details["reason_code"], "permission.denied");
        assert!(audit[0].details["reason_detail"].contains(&deny_rule_id));
        assert!(audit[0].details["reason_detail"].contains("network layer"));
        assert!(!format!("{audit:?}").contains(client_id));
    }

    #[tokio::test]
    async fn create_network_permission_denial_precedes_handler_and_is_audited() {
        let calls = Arc::new(AtomicUsize::new(0));
        let network_id = fuckipv6_common::domain::NetworkId::from_public_key(b"create-denied-home");
        let context = NodeCommandContext::network_session(
            NodeId::from_public_key(b"create-denied-admin"),
            "control-api:admin-session",
            [NodePermission::ManageNetworks],
            [(
                "home".into(),
                crate::node::NetworkCommandScope::new(
                    network_id,
                    fuckipv6_common::domain::NetworkRole::Administrator,
                    fuckipv6_common::domain::MembershipState::Active,
                    Vec::new(),
                ),
            )],
            true,
        );
        let bus = NodeCommandBus::spawn(
            Arc::new(CountingHandler {
                calls: calls.clone(),
            }),
            NodeEventBus::default(),
            4,
        );
        let command = NodeCommand::CreateNetwork(NodeCreateNetworkCommand {
            name: "new-network".into(),
            purpose: "A network the administrator cannot create".into(),
            icon: "network".into(),
            language: "en".into(),
            region: "ZZ".into(),
            tags: Vec::new(),
            capability_declarations: Vec::new(),
            visibility: NetworkVisibility::Private,
            join_mode: NetworkJoinMode::InviteOnly,
            exit_contribution: Default::default(),
            exit_contribution_scope: Default::default(),
            signaling_port: 6666,
            relay_port: 6667,
            external_host: None,
            auto_connect: false,
        });

        let result = bus
            .execute(context, NodeCommandRequest::new(58, command))
            .await
            .expect("authorization denial is a command result");

        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let NodeCommandOutcome::Rejected(problem) = result.outcome else {
            panic!("expected authorization rejection");
        };
        assert_eq!(problem.code, NodeProblemCode::PermissionDenied);
        assert_eq!(
            problem.params.get("authorization_code").map(String::as_str),
            Some("permission.local_owner_required")
        );
        let audit = bus.audit_entries();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].outcome, AuditOutcome::Denied);
        assert_eq!(audit[0].action, "network.create");
        assert_eq!(
            audit[0].details["reason_code"],
            "permission.local_owner_required"
        );
        assert!(audit[0].details["reason_detail"].contains("local node owner"));
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
                    NodeCommand::JoinNetwork(Box::new(NodeJoinNetworkCommand {
                        target: invitation.into(),
                        network_name: Some("home".into()),
                        start: false,
                        discovery_record: None,
                        network_id: None,
                        preview_digest: None,
                        network_policy_revision: None,
                        privacy_policy_revision: None,
                        terms_hash: None,
                        acknowledged_at_unix_ms: None,
                        exit_contribution_acknowledged: None,
                    })),
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
    async fn network_discovery_actions_have_traceable_actor_action_and_operation_ids() {
        let state = NodeStateStore::new();
        let bus = NodeCommandBus::spawn_with_state(
            Arc::new(EchoHandler),
            NodeEventBus::default(),
            8,
            state.clone(),
        );
        let network_id = NetworkId::from_public_key(b"traceable-network");
        let request_id =
            fuckipv6_common::domain::JoinRequestId::from_public_key(b"traceable-join-request");
        let actor = actor();
        let commands = vec![
            NodeCommand::CreateNetwork(valid_create_network_command()),
            NodeCommand::PublishNetworkDiscovery(NodeNetworkDiscoveryCommand {
                network_name: "home".into(),
                index_url: "https://discovery.example.test".into(),
            }),
            NodeCommand::JoinNetwork(Box::new(NodeJoinNetworkCommand {
                target: "fk://SECRET-TRACE-TICKET@example.test:6666".into(),
                network_name: Some("home".into()),
                start: false,
                discovery_record: None,
                network_id: None,
                preview_digest: None,
                network_policy_revision: None,
                privacy_policy_revision: None,
                terms_hash: None,
                acknowledged_at_unix_ms: None,
                exit_contribution_acknowledged: None,
            })),
            NodeCommand::ReviewJoinRequest(NodeReviewJoinRequestCommand {
                network_id: network_id.to_string(),
                review: NodeJoinRequestReviewItem {
                    request_id: request_id.to_string(),
                    expected_revision: 1,
                    action: JoinRequestReviewAction::RequestInformation,
                    note: Some("SECRET-TRACE-REVIEW-NOTE".into()),
                },
            }),
            NodeCommand::CreateInvitation(NodeCreateInvitationCommand {
                network_id,
                role: MembershipRole::Member,
                capabilities: vec![CapabilityKind::File],
                max_uses: 1,
                expires_in_ms: 3_600_000,
            }),
        ];
        let expected_actions = [
            "network.create",
            "network.discovery.publish",
            "network.join",
            "network.join_request.review",
            "network.invitation.create",
        ];
        let mut operation_ids = Vec::new();

        for (offset, command) in commands.into_iter().enumerate() {
            let operation_id = fuckipv6_common::domain::OperationId::from_public_key(
                format!("traceable-network-operation-{offset}").as_bytes(),
            );
            let result = bus
                .execute(
                    NodeCommandContext::local_owner(actor.clone(), "control-api:owner-session")
                        .with_test_network_authority_for(&command)
                        .with_audit_operation_id(operation_id.clone()),
                    NodeCommandRequest::new(80 + offset as u64, command),
                )
                .await
                .expect("traceable network command result");
            assert!(result.is_success());
            operation_ids.push(operation_id);
        }

        let audit = bus.audit_entries();
        assert_eq!(audit.len(), expected_actions.len());
        for (index, (entry, expected_action)) in audit.iter().zip(expected_actions).enumerate() {
            assert_eq!(entry.actor, actor);
            assert_eq!(entry.action, expected_action);
            assert_eq!(entry.operation_id.as_ref(), Some(&operation_ids[index]));
            assert_eq!(entry.outcome, AuditOutcome::Succeeded);
            assert_eq!(entry.details["actor_source"], "control-api:owner-session");
            assert_eq!(entry.details["actor_role"], "owner");
            assert_eq!(entry.details["request_id"], (80 + index as u64).to_string());
        }

        let activity = state.snapshot().await.recent_activity;
        assert_eq!(activity.len(), 5);
        assert!(activity
            .iter()
            .all(|entry| entry.actor == actor && entry.outcome == AuditOutcome::Succeeded));
        let debug = format!("{audit:?}{activity:?}");
        assert!(!debug.contains("SECRET-TRACE-TICKET"));
        assert!(!debug.contains("SECRET-TRACE-REVIEW-NOTE"));
    }

    #[tokio::test]
    async fn join_request_audit_and_activity_cover_every_action_without_prose() {
        let state = NodeStateStore::new();
        let bus = NodeCommandBus::spawn_with_state(
            Arc::new(EchoHandler),
            NodeEventBus::default(),
            8,
            state.clone(),
        );
        let network_id = join_request_network_id();
        let request_id = join_request_id("audit-request");
        let context = |command: &NodeCommand| {
            NodeCommandContext::local_owner(actor(), "local-ipc:owner")
                .with_test_network_authority_for(command)
        };
        let commands = vec![
            NodeCommand::SubmitJoinRequest(NodeSubmitJoinRequestCommand {
                network_id: network_id.clone(),
                reason: "SECRET-AUDIT-REASON".into(),
            }),
            NodeCommand::ReviewJoinRequest(NodeReviewJoinRequestCommand {
                network_id: network_id.clone(),
                review: review_item(request_id.clone(), JoinRequestReviewAction::Approve, None),
            }),
            NodeCommand::ReviewJoinRequest(NodeReviewJoinRequestCommand {
                network_id: network_id.clone(),
                review: review_item(
                    request_id.clone(),
                    JoinRequestReviewAction::Reject,
                    Some("SECRET-AUDIT-REJECTION"),
                ),
            }),
            NodeCommand::ReviewJoinRequest(NodeReviewJoinRequestCommand {
                network_id: network_id.clone(),
                review: review_item(
                    request_id.clone(),
                    JoinRequestReviewAction::RequestInformation,
                    Some("SECRET-AUDIT-INFORMATION"),
                ),
            }),
            NodeCommand::BatchReviewJoinRequests(NodeBatchReviewJoinRequestsCommand {
                network_id: network_id.clone(),
                reviews: vec![
                    review_item(request_id.clone(), JoinRequestReviewAction::Approve, None),
                    review_item(
                        join_request_id("second-audit-request"),
                        JoinRequestReviewAction::Reject,
                        Some("SECRET-AUDIT-BATCH"),
                    ),
                ],
            }),
            NodeCommand::WithdrawJoinRequest(NodeWithdrawJoinRequestCommand {
                network_id: network_id.clone(),
                request_id: request_id.clone(),
            }),
            NodeCommand::ResubmitJoinRequest(NodeResubmitJoinRequestCommand {
                network_id,
                request_id,
                expected_revision: 2,
                reason: "SECRET-AUDIT-RESUBMISSION".into(),
            }),
        ];

        for (offset, command) in commands.into_iter().enumerate() {
            let result = bus
                .execute(
                    context(&command),
                    NodeCommandRequest::new(60 + offset as u64, command),
                )
                .await
                .expect("join request command result");
            assert!(result.is_success());
        }

        let audit = bus.audit_entries();
        assert_eq!(
            audit
                .iter()
                .map(|entry| entry.action.as_str())
                .collect::<Vec<_>>(),
            vec![
                "network.join_request.submit",
                "network.join_request.review",
                "network.join_request.review",
                "network.join_request.review",
                "network.join_request.batch_review",
                "network.join_request.withdraw",
                "network.join_request.resubmit",
            ]
        );
        assert!(audit
            .iter()
            .all(|entry| entry.outcome == AuditOutcome::Succeeded));

        let activity = state.snapshot().await.recent_activity;
        assert_eq!(activity.len(), 7);
        let mut activity_actions = activity
            .iter()
            .map(|entry| entry.action.as_str())
            .collect::<Vec<_>>();
        let mut expected_actions = vec![
            "network.join_request.submit",
            "network.join_request.review",
            "network.join_request.review",
            "network.join_request.review",
            "network.join_request.batch_review",
            "network.join_request.withdraw",
            "network.join_request.resubmit",
        ];
        // Commands recorded in the same millisecond are ordered by stable audit
        // ID, not execution order. This test verifies complete coverage.
        activity_actions.sort_unstable();
        expected_actions.sort_unstable();
        assert_eq!(activity_actions, expected_actions);
        for secret in [
            "SECRET-AUDIT-REASON",
            "SECRET-AUDIT-REJECTION",
            "SECRET-AUDIT-INFORMATION",
            "SECRET-AUDIT-BATCH",
            "SECRET-AUDIT-RESUBMISSION",
        ] {
            assert!(!format!("{audit:?}").contains(secret));
            assert!(!format!("{activity:?}").contains(secret));
        }
    }

    fn policy_membership(
        acknowledged_at_unix_ms: u64,
        terms_hash: ContentHash,
        policy_confirmation_required: bool,
    ) -> crate::node::NodeNetworkMembershipSnapshot {
        let terms = fuckipv6_common::domain::MembershipTerms {
            network_policy_revision: 5,
            privacy_policy_revision: 2,
            exit_contribution: ExitContributionObligation::Required,
            exit_contribution_scope: ExitContributionScope::AllMembers,
            member_limit: Some(16),
        };
        crate::node::NodeNetworkMembershipSnapshot {
            network_name: "home".into(),
            network_id: NetworkId::from_public_key(b"policy-audit-network").to_string(),
            node_id: "node-command-bus-test".into(),
            state: fuckipv6_common::domain::MembershipState::Active,
            role: MembershipRole::Member,
            local_policy_revision: 5,
            network_policy_revision: Some(5),
            privacy_policy_revision: Some(2),
            member_limit: Some(16),
            exit_contribution: Some(ExitContributionObligation::Required),
            exit_contribution_scope: Some(ExitContributionScope::AllMembers),
            terms_hash: Some(terms_hash.as_str().to_owned()),
            current_terms: Some(terms),
            current_terms_hash: Some(terms_hash.as_str().to_owned()),
            policy_confirmation_required,
            acknowledged_at_unix_ms: Some(acknowledged_at_unix_ms),
            created_at_unix_ms: 1_700_000_000_000,
            updated_at_unix_ms: acknowledged_at_unix_ms,
            suspension_reason: None,
        }
    }

    fn policy_audit_snapshot(
        membership: crate::node::NodeNetworkMembershipSnapshot,
        exit_enabled: bool,
    ) -> NodeSnapshot {
        use crate::node::{NodeNetworkSnapshot, NodeNetworkStatus, NodeTrafficSnapshot};

        NodeSnapshot {
            exit_node_capabilities: fuckipv6_common::ExitNodeCapabilities {
                supported: true,
                ..fuckipv6_common::ExitNodeCapabilities::default()
            },
            memberships: vec![membership],
            networks: vec![NodeNetworkSnapshot {
                name: "home".into(),
                tun_name: "fk-home".into(),
                status: NodeNetworkStatus::Online,
                assigned_ipv6: None,
                assigned_ipv4: None,
                p2p_listen_addr: None,
                uptime_secs: 60,
                roles: if exit_enabled {
                    vec![NodeNetworkRole::Member, NodeNetworkRole::ExitNode]
                } else {
                    vec![NodeNetworkRole::Member]
                },
                peer_count: 2,
                traffic: NodeTrafficSnapshot::default(),
            }],
            ..NodeSnapshot::default()
        }
    }

    #[test]
    fn join_audit_separates_consent_policy_and_actual_local_exit_state() {
        let network_id = NetworkId::from_public_key(b"policy-audit-network").to_string();
        let terms_hash = ContentHash::from_bytes(b"policy audit terms");
        let acknowledged_at = 1_700_000_000_123;
        let membership = policy_membership(acknowledged_at, terms_hash.clone(), false);
        let snapshot = policy_audit_snapshot(membership.clone(), true);
        let request = NodeCommandRequest::new(
            77,
            NodeCommand::JoinNetwork(Box::new(NodeJoinNetworkCommand {
                target: "fk://policy-audit".into(),
                network_name: Some("home".into()),
                start: true,
                discovery_record: None,
                network_id: Some(network_id.clone()),
                preview_digest: Some("preview-digest".into()),
                network_policy_revision: Some(5),
                privacy_policy_revision: Some(2),
                terms_hash: Some(terms_hash.clone()),
                acknowledged_at_unix_ms: Some(acknowledged_at),
                exit_contribution_acknowledged: Some(true),
            })),
        );
        let result = NodeCommandResult::completed(
            77,
            NodeCommandOutcome::NetworkJoined(Box::new(NodeNetworkJoinResult {
                membership,
                snapshot: snapshot.clone(),
                idempotent: false,
                config_saved: true,
                started: true,
            })),
        );

        let mut details = BTreeMap::new();
        insert_membership_policy_audit_details(
            &mut details,
            &request,
            Some(&result),
            Some(&snapshot),
        );

        assert_eq!(details["exit_contribution_user_consent"], "accepted");
        assert_eq!(
            details["exit_contribution_acknowledged_at_unix_ms"],
            acknowledged_at.to_string()
        );
        assert_eq!(details["network_policy_revision"], "5");
        assert_eq!(details["privacy_policy_revision"], "2");
        assert_eq!(details["terms_hash"], terms_hash.as_str());
        assert_eq!(details["exit_contribution"], "required");
        assert_eq!(details["exit_contribution_scope"], "all_members");
        assert_eq!(details["local_exit_capability"], "true");
        assert_eq!(details["local_exit_enabled"], "true");
        assert_eq!(details["policy_confirmation_required"], "false");
        assert_eq!(details["membership_network_id"], network_id);
    }

    #[test]
    fn declined_redemption_audit_keeps_typed_policy_facts_and_drops_sensitive_params() {
        let network_id = invitation_network_id();
        let terms_hash = invitation_hash(b"new policy terms");
        let acknowledged_at = 1_700_000_000_456;
        let membership = policy_membership(acknowledged_at, terms_hash.clone(), true);
        let snapshot = policy_audit_snapshot(membership, false);
        let request = NodeCommandRequest::new(
            78,
            NodeCommand::RedeemInvitation(NodeRedeemInvitationCommand {
                coordinator_url: "wss://coordinator.example.test/signaling".into(),
                request: InvitationRedeemRequest {
                    network_id: network_id.clone(),
                    code: invitation_code(),
                    invitation_id: invitation_id(),
                    expected_invitation_revision: 9,
                    preview_digest: invitation_hash(b"policy preview"),
                    terms_hash: terms_hash.clone(),
                    acknowledged_at_unix_ms: acknowledged_at,
                    exit_contribution_acknowledged: Some(false),
                },
            }),
        );
        let secret_code = "fkinv_SECRET_MUST_NOT_APPEAR";
        let mut problem = NodeProblem::new(NodeProblemCode::Conflict, "policy changed")
            .with_param("policy_exit_contribution", "required")
            .with_param("policy_exit_contribution_scope", "all_members")
            .with_param("policy_network_revision", "5")
            .with_param("policy_privacy_revision", "2")
            .with_param("policy_terms_hash", terms_hash.as_str())
            .with_param("invitation_code", secret_code)
            .with_param("reviewer_note", "PRIVATE-FREE-TEXT");
        problem.message_key = "membership.policy.confirmation_required".into();
        let result = NodeCommandResult::rejected(78, problem);

        let mut details = BTreeMap::new();
        insert_membership_policy_audit_details(
            &mut details,
            &request,
            Some(&result),
            Some(&snapshot),
        );

        assert_eq!(details["exit_contribution_user_consent"], "declined");
        assert_eq!(details["exit_contribution"], "required");
        assert_eq!(details["exit_contribution_scope"], "all_members");
        assert_eq!(details["network_policy_revision"], "5");
        assert_eq!(details["privacy_policy_revision"], "2");
        assert_eq!(details["terms_hash"], terms_hash.as_str());
        assert_eq!(details["local_exit_capability"], "true");
        assert_eq!(details["local_exit_enabled"], "false");
        let encoded = format!("{details:?}");
        assert!(!encoded.contains(secret_code));
        assert!(!encoded.contains("PRIVATE-FREE-TEXT"));
    }

    #[test]
    fn policy_reconfirmation_audit_records_the_exact_acknowledged_version() {
        let network_id = NetworkId::from_public_key(b"policy-audit-network").to_string();
        let terms_hash = ContentHash::from_bytes(b"reconfirmed policy terms");
        let acknowledged_at = 1_700_000_000_789;
        let membership = policy_membership(acknowledged_at, terms_hash.clone(), false);
        let snapshot = policy_audit_snapshot(membership.clone(), false);
        let request = NodeCommandRequest::new(
            79,
            NodeCommand::AcknowledgeMembershipPolicy(NodeAcknowledgeMembershipPolicyCommand {
                network_id: network_id.clone(),
                expected_terms_hash: terms_hash.clone(),
                acknowledged_at_unix_ms: acknowledged_at,
                exit_contribution_acknowledged: Some(true),
            }),
        );
        let result = NodeCommandResult::completed(
            79,
            NodeCommandOutcome::MembershipPolicyAcknowledged(Box::new(membership)),
        );

        let mut details = BTreeMap::new();
        insert_membership_policy_audit_details(
            &mut details,
            &request,
            Some(&result),
            Some(&snapshot),
        );

        assert_eq!(details["exit_contribution_user_consent"], "accepted");
        assert_eq!(
            details["exit_contribution_acknowledged_at_unix_ms"],
            acknowledged_at.to_string()
        );
        assert_eq!(details["terms_hash"], terms_hash.as_str());
        assert_eq!(details["network_policy_revision"], "5");
        assert_eq!(details["privacy_policy_revision"], "2");
        assert_eq!(details["exit_contribution"], "required");
        assert_eq!(details["exit_contribution_scope"], "all_members");
        assert_eq!(details["policy_confirmation_required"], "false");
        assert_eq!(details["local_exit_capability"], "true");
        assert_eq!(details["local_exit_enabled"], "false");
        assert_eq!(details["membership_network_id"], network_id);
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
    async fn security_commands_share_chain_skip_reads_and_redact_credentials() {
        let dir = durable_audit_dir("shared-chain");
        let node_id = NodeId::from_public_key(b"command-durable-audit-node");
        let ledger = Arc::new(
            OwnerSecurityAudit::new(&dir, node_id.clone()).expect("durable command audit"),
        );
        let bus = NodeCommandBus::spawn_with_durable_audit(
            Arc::new(EchoHandler),
            NodeEventBus::default(),
            8,
            ledger.clone(),
        );

        bus.execute_test(NodeCommandRequest::new(1, NodeCommand::StartNode))
            .await
            .expect("audited write");
        bus.execute_test(NodeCommandRequest::new(2, NodeCommand::GetSnapshot))
            .await
            .expect("read command");
        bus.execute_test(NodeCommandRequest::new(
            3,
            NodeCommand::RedeemInvitation(valid_redeem_command()),
        ))
        .await
        .expect("invitation command");
        bus.execute(
            NodeCommandContext::untrusted("legacy_loopback_tcp"),
            NodeCommandRequest::new(4, NodeCommand::StartNode),
        )
        .await
        .expect("authorization denial result");

        let bundle = ledger
            .export(&AuditExportFilter::default())
            .expect("export command audit");
        assert_eq!(
            bundle
                .records
                .iter()
                .map(|record| record.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(bundle.records[0].entry.action, "node.start");
        assert_eq!(bundle.records[1].entry.action, "network.invitation.redeem");
        assert_eq!(bundle.records[2].entry.action, "node.start");
        assert_eq!(bundle.records[2].entry.outcome, AuditOutcome::Denied);
        assert!(bundle
            .records
            .iter()
            .all(|record| record.entry.details["command"] != "get_snapshot"));
        assert!(bundle
            .records
            .iter()
            .all(|record| !record.entry.details.contains_key("reason_detail")));
        let serialized = serde_json::to_string(&bundle).expect("serialize audit export");
        assert!(!serialized.contains(invitation_code().as_str()));
        assert!(!serialized.contains("fkinv_"));
        assert_eq!(bundle.manifest.verification.record_count, 3);

        drop(bus);
        drop(ledger);
        #[cfg(unix)]
        std::fs::remove_dir_all(dir).expect("remove audit directory");
    }

    #[tokio::test]
    async fn durable_audit_failure_withholds_successful_command_result() {
        let dir = durable_audit_dir("fail-closed");
        let policy = AuditRetentionPolicy {
            minimum_retention_days: 1,
            segment_records: 1,
            max_segments: 1,
        };
        let ledger = Arc::new(
            OwnerSecurityAudit::new_with_retention_policy_for_test(
                &dir,
                NodeId::from_public_key(b"command-durable-audit-node"),
                policy,
            )
            .expect("capacity-limited audit"),
        );
        let bus = NodeCommandBus::spawn_with_durable_audit(
            Arc::new(EchoHandler),
            NodeEventBus::default(),
            8,
            ledger.clone(),
        );

        bus.execute_test(NodeCommandRequest::new(10, NodeCommand::StartNode))
            .await
            .expect("first audited command");
        let error = bus
            .execute_test(NodeCommandRequest::new(11, NodeCommand::StartNode))
            .await
            .expect_err("audit failure must not return command success");
        assert!(matches!(error, NodeCommandBusError::AuditUnavailable(_)));
        assert_eq!(
            ledger
                .export(&AuditExportFilter::default())
                .expect("existing audit remains verifiable")
                .records
                .len(),
            1
        );
        assert!(!ledger.health().healthy);

        drop(bus);
        drop(ledger);
        #[cfg(unix)]
        std::fs::remove_dir_all(dir).expect("remove audit directory");
    }

    #[tokio::test]
    async fn snapshot_activity_is_stably_ordered_retained_to_256_and_excludes_reads() {
        let state = NodeStateStore::new();
        let bus = NodeCommandBus::spawn_with_state(
            Arc::new(EchoHandler),
            NodeEventBus::default(),
            8,
            state.clone(),
        );

        for request_id in 60..360 {
            bus.execute_test(NodeCommandRequest::new(request_id, NodeCommand::StartNode))
                .await
                .expect("authorized command");
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        bus.execute_test(NodeCommandRequest::new(500, NodeCommand::StartNode))
            .await
            .expect("authorized command");
        bus.execute_test(NodeCommandRequest::new(400, NodeCommand::GetSnapshot))
            .await
            .expect("snapshot query");

        let activity = state.snapshot().await.recent_activity;
        assert_eq!(activity.len(), ACTIVITY_RETENTION_LIMIT);
        assert!(activity
            .iter()
            .all(|entry| entry.details["command"] == "start_node"));
        assert_eq!(activity[0].details["request_id"], "500");
        assert!(activity.windows(2).all(|pair| {
            pair[0].occurred_at_unix_ms > pair[1].occurred_at_unix_ms
                || (pair[0].occurred_at_unix_ms == pair[1].occurred_at_unix_ms
                    && pair[0].audit_id > pair[1].audit_id)
        }));
        let audit_ids = activity
            .iter()
            .map(|entry| entry.audit_id.as_str())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(audit_ids.len(), ACTIVITY_RETENTION_LIMIT);
        assert!(!activity
            .iter()
            .any(|entry| entry.details["command"] == "get_snapshot"));
    }

    fn actor() -> NodeId {
        NodeId::from_public_key(b"command-bus-audit-test")
    }
}
