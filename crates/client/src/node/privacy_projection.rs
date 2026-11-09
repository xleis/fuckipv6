//! Server-side field projection for node snapshots.
//!
//! The canonical [`NodeSnapshot`] remains the shared, authoritative state used
//! by authorization and local adapters. Every remote serialization boundary
//! must use [`ProjectedNodeSnapshot`] so hidden fields are removed before JSON
//! is produced, rather than masked by a client.

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use fuckipv6_common::domain::{
    field_is_visible, project_activity_entry, project_member_directory, project_topology_snapshot,
    ActivityEntry, FieldPrivacyPolicy, InvitationPreview, InvitationRedemption,
    InvitationStatusView, IssuedInvitation, JoinRequest, MemberLifecycleChange,
    MemberRemovalPreview, MembershipState, NetworkId, NetworkMemberDirectory,
    NetworkProfileApplyResult, NetworkProfilePreview, NodeId, PolicyTestResult, PrivacyField,
    PrivacyObjectState, PrivacyViewer, ProjectedActivityEntry, TopologyConnection, TopologyNode,
    TopologySnapshot, TrafficPolicyTestComparison, TrafficRoutingPageModel,
};
use fuckipv6_common::ExitNodeCapabilities;

use super::authorization::NodeCommandContext;
use super::command::{
    NodeBlacklistSnapshot, NodeCommandOutcome, NodeCommandResult, NodeCreatedNetwork,
    NodeDiagnosticsSnapshot, NodeIdentityRotationSnapshot, NodeJoinRequestListResult,
    NodeJoinRequestReviewAck, NodeNetworkAllocationSnapshot, NodeNetworkJoinPreview,
    NodePeerProbeSnapshot, NodeResidualCleanupResult,
};
use super::event::{NodeEvent, NodeEventKind};
use super::state::{
    LocalExitCapabilitySnapshot, NodeAccessControlSnapshot, NodeCapabilityRegistrySnapshot,
    NodeConfigurationSnapshot, NodeControlHealth, NodeEmbeddedServiceSnapshot,
    NodeEmbeddedServiceStatus, NodeExitSelectionSnapshot, NodeInterfaceAddress, NodeInterfaceKind,
    NodeInterfaceOwnership, NodeInterfaceStatus, NodeLogEntry, NodeNetworkMembershipSnapshot,
    NodeNetworkRole, NodeNetworkSnapshot, NodeNetworkStatus, NodePeerSnapshot,
    NodePermissionSnapshot, NodeProblem, NodeRoutingSummarySnapshot, NodeSnapshot,
    NodeTrafficSnapshot,
};
use super::{NodeLifecycleSnapshot, NodeLogLevel, NodeRoutingMode, NodeUiManifest};
use crate::config_transaction::{
    ConfigurationApplyResult, ConfigurationDocument, ConfigurationHistoryEntry,
    ConfigurationPreview,
};
use crate::discovery_index::NodeDiscoveryIndexSnapshot;
use crate::policy_transaction::{
    PolicyApplyResult, PolicyCurrent, PolicyHistorySnapshot, PolicyPreview, PolicyRollbackResult,
};
use crate::traffic_policy_transaction::{
    TrafficPolicyApplyResult, TrafficPolicyCurrent, TrafficPolicyHistorySnapshot,
    TrafficPolicyPreview, TrafficPolicyRollbackResult,
};
use crate::tun::TunCapabilities;
use crate::ui::UiModeResolution;

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum NodeSnapshotProjectionError {
    #[error("节点字段隐私策略无效")]
    InvalidFieldPolicy,
}

/// A node snapshot whose sensitive fields have already been removed. Optional
/// hidden values are serialized by omission; arrays that would only contain
/// hidden rows are omitted as well.
#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct ProjectedNodeSnapshot {
    pub revision: u64,
    pub lifecycle: NodeLifecycleSnapshot,
    pub control_health: NodeControlHealth,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<ProjectedNodeIdentitySnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<NodeConfigurationSnapshot>,
    #[serde(default)]
    pub exit_node_capabilities: ExitNodeCapabilities,
    #[serde(default)]
    pub local_exit: LocalExitCapabilitySnapshot,
    #[serde(default)]
    pub capabilities: NodeCapabilityRegistrySnapshot,
    #[serde(default)]
    pub permissions: NodePermissionSnapshot,
    #[serde(default, skip_serializing_if = "NodeAccessControlSnapshot::is_empty")]
    pub access_control: NodeAccessControlSnapshot,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub embedded_services: Vec<ProjectedNodeEmbeddedServiceSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_mode: Option<UiModeResolution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tun_capabilities: Option<TunCapabilities>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interfaces: Vec<ProjectedNodeInterfaceSnapshot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interface_problems: Vec<NodeProblem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routing_summaries: Vec<NodeRoutingSummarySnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traffic_routing_page: Option<TrafficRoutingPageModel>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub network_profiles: Vec<super::state::NodeNetworkProfileSnapshot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exit_selections: Vec<NodeExitSelectionSnapshot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub memberships: Vec<NodeNetworkMembershipSnapshot>,
    #[serde(default)]
    pub discovery_index: NodeDiscoveryIndexSnapshot,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub networks: Vec<ProjectedNodeNetworkSnapshot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub peers: Vec<ProjectedNodePeerSnapshot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_activity: Vec<ProjectedActivityEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub logs: Vec<NodeLogEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<NodeProblem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topology: Option<TopologySnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ProjectedNodeIdentitySnapshot {
    pub node_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_key_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct ProjectedNodeNetworkSnapshot {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tun_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<NodeNetworkStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_ipv6: Option<Ipv6Addr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_ipv4: Option<Ipv4Addr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p2p_listen_addr: Option<SocketAddr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uptime_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roles: Option<Vec<NodeNetworkRole>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traffic: Option<NodeTrafficSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ProjectedNodeInterfaceSnapshot {
    pub index: u32,
    pub name: String,
    pub kind: NodeInterfaceKind,
    pub ownership: NodeInterfaceOwnership,
    pub status: NodeInterfaceStatus,
    pub mtu: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addresses: Option<Vec<NodeInterfaceAddress>>,
    pub default_ipv4: bool,
    pub default_ipv6: bool,
}

#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct ProjectedNodePeerSnapshot {
    pub peer_id: String,
    pub network_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub virtual_ipv6: Option<Ipv6Addr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub virtual_ipv4: Option<Ipv4Addr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoints: Option<Vec<SocketAddr>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub online: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p2p_active: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p2p_healthy: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtt_us: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loss_rate: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traffic: Option<NodeTrafficSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ProjectedNodeEmbeddedServiceSnapshot {
    pub network_name: String,
    pub status: NodeEmbeddedServiceStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signaling_addr: Option<SocketAddr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_addr: Option<SocketAddr>,
}

/// A member directory that has already passed field projection. The inner
/// canonical value is intentionally private so adapters cannot accidentally
/// serialize an unprojected directory through this DTO.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProjectedNetworkMemberDirectory(NetworkMemberDirectory);

impl ProjectedNetworkMemberDirectory {
    pub fn as_directory(&self) -> &NetworkMemberDirectory {
        &self.0
    }
}

/// Wire-safe command result. Sensitive variants carry projected DTOs rather
/// than canonical snapshots or member directories.
#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct ProjectedNodeCommandResult {
    pub request_id: u64,
    pub outcome: ProjectedNodeCommandOutcome,
}

#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "status", content = "data", rename_all = "snake_case")]
pub enum ProjectedNodeCommandOutcome {
    Snapshot(Box<ProjectedNodeSnapshot>),
    UiManifest(Box<NodeUiManifest>),
    Diagnostics(Box<NodeDiagnosticsSnapshot>),
    Cleanup(NodeResidualCleanupResult),
    Blacklist(NodeBlacklistSnapshot),
    NetworkAllocation(NodeNetworkAllocationSnapshot),
    Probe(NodePeerProbeSnapshot),
    NetworkCreated(NodeCreatedNetwork),
    NetworkProfilePreview(Box<NetworkProfilePreview>),
    NetworkProfileUpdated(Box<NetworkProfileApplyResult>),
    NetworkJoinPreview(Box<NodeNetworkJoinPreview>),
    NetworkJoined(Box<ProjectedNodeNetworkJoinResult>),
    JoinRequestSubmitted(Box<JoinRequest>),
    JoinRequests(Box<NodeJoinRequestListResult>),
    NetworkMembers(Box<ProjectedNetworkMemberDirectory>),
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
    NetworkLeft(Box<ProjectedNodeNetworkLeaveResult>),
    MembershipPolicyAcknowledged(Box<NodeNetworkMembershipSnapshot>),
    LocalExitCapability(Box<LocalExitCapabilitySnapshot>),
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
    TrafficPolicy(TrafficPolicyCurrent),
    TrafficPolicyPreview(Box<TrafficPolicyPreview>),
    TrafficPolicyApplied(Box<TrafficPolicyApplyResult>),
    TrafficPolicyHistory(Box<TrafficPolicyHistorySnapshot>),
    TrafficPolicyRolledBack(Box<TrafficPolicyRollbackResult>),
    TrafficPolicyTest(Box<TrafficPolicyTestComparison>),
    Accepted { operation_id: String },
    Completed { message: String },
    Rejected(NodeProblem),
}

#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct ProjectedNodeNetworkJoinResult {
    pub membership: NodeNetworkMembershipSnapshot,
    pub snapshot: ProjectedNodeSnapshot,
    pub idempotent: bool,
    pub config_saved: bool,
    pub started: bool,
}

#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct ProjectedNodeNetworkLeaveResult {
    pub network_name: String,
    pub left: bool,
    pub snapshot: ProjectedNodeSnapshot,
}

/// Event with field-level projection applied. Log contents are omitted for
/// non-owner viewers while the sequence and snapshot revision remain intact so
/// cursors still advance without silently skipping an event.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ProjectedNodeEvent {
    pub sequence: u64,
    pub snapshot_revision: u64,
    pub kind: ProjectedNodeEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ProjectedNodeEventKind {
    StateChanged {
        revision: u64,
    },
    NetworkChanged {
        network_name: String,
        status: NodeNetworkStatus,
    },
    EmbeddedServiceChanged {
        network_name: String,
        status: NodeEmbeddedServiceStatus,
    },
    LogAppended {
        level: NodeLogLevel,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    RoutingChanged {
        network_name: String,
        mode: NodeRoutingMode,
        exit_node: Option<String>,
        is_exit_node: bool,
    },
    RoleChanged {
        network_name: String,
        role: NodeNetworkRole,
        enabled: bool,
        roles: Vec<NodeNetworkRole>,
    },
    CommandCompleted {
        request_id: u64,
        success: bool,
    },
    OperationChanged {
        operation_id: String,
        state: fuckipv6_common::domain::OperationState,
        stage: String,
    },
}

/// Project a command result immediately before it crosses a transport
/// boundary. The canonical result remains unchanged for local execution and
/// audit code.
pub fn project_node_command_result(
    result: &NodeCommandResult,
    context: &NodeCommandContext,
    now_unix_ms: u64,
) -> Result<ProjectedNodeCommandResult, NodeSnapshotProjectionError> {
    let outcome = match &result.outcome {
        NodeCommandOutcome::Snapshot(snapshot) => ProjectedNodeCommandOutcome::Snapshot(Box::new(
            project_node_snapshot(snapshot, context, now_unix_ms)?,
        )),
        NodeCommandOutcome::UiManifest(manifest) => {
            ProjectedNodeCommandOutcome::UiManifest(manifest.clone())
        }
        NodeCommandOutcome::Diagnostics(diagnostics) => {
            ProjectedNodeCommandOutcome::Diagnostics(diagnostics.clone())
        }
        NodeCommandOutcome::Cleanup(cleanup) => {
            ProjectedNodeCommandOutcome::Cleanup(cleanup.clone())
        }
        NodeCommandOutcome::Blacklist(blacklist) => {
            ProjectedNodeCommandOutcome::Blacklist(blacklist.clone())
        }
        NodeCommandOutcome::NetworkAllocation(allocation) => {
            ProjectedNodeCommandOutcome::NetworkAllocation(allocation.clone())
        }
        NodeCommandOutcome::Probe(probe) => ProjectedNodeCommandOutcome::Probe(probe.clone()),
        NodeCommandOutcome::NetworkCreated(created) => {
            ProjectedNodeCommandOutcome::NetworkCreated(created.clone())
        }
        NodeCommandOutcome::NetworkProfilePreview(preview) => {
            ProjectedNodeCommandOutcome::NetworkProfilePreview(preview.clone())
        }
        NodeCommandOutcome::NetworkProfileUpdated(updated) => {
            ProjectedNodeCommandOutcome::NetworkProfileUpdated(updated.clone())
        }
        NodeCommandOutcome::NetworkJoinPreview(preview) => {
            ProjectedNodeCommandOutcome::NetworkJoinPreview(preview.clone())
        }
        NodeCommandOutcome::NetworkJoined(joined) => {
            ProjectedNodeCommandOutcome::NetworkJoined(Box::new(ProjectedNodeNetworkJoinResult {
                membership: joined.membership.clone(),
                snapshot: project_node_snapshot(&joined.snapshot, context, now_unix_ms)?,
                idempotent: joined.idempotent,
                config_saved: joined.config_saved,
                started: joined.started,
            }))
        }
        NodeCommandOutcome::JoinRequestSubmitted(request) => {
            ProjectedNodeCommandOutcome::JoinRequestSubmitted(request.clone())
        }
        NodeCommandOutcome::JoinRequests(requests) => {
            ProjectedNodeCommandOutcome::JoinRequests(requests.clone())
        }
        NodeCommandOutcome::NetworkMembers(directory) => {
            if context.privacy_viewer_for(None).local_owner {
                return Ok(ProjectedNodeCommandResult {
                    request_id: result.request_id,
                    outcome: ProjectedNodeCommandOutcome::NetworkMembers(Box::new(
                        ProjectedNetworkMemberDirectory(directory.as_ref().clone()),
                    )),
                });
            }
            let viewer = context.privacy_viewer_for(Some(&directory.network_id));
            let policy = context
                .field_privacy_policy_for(Some(&directory.network_id))
                .map_err(|_| NodeSnapshotProjectionError::InvalidFieldPolicy)?;
            ProjectedNodeCommandOutcome::NetworkMembers(Box::new(ProjectedNetworkMemberDirectory(
                project_member_directory(directory, &viewer, policy, now_unix_ms),
            )))
        }
        NodeCommandOutcome::MemberRemovalPreview(preview) => {
            ProjectedNodeCommandOutcome::MemberRemovalPreview(preview.clone())
        }
        NodeCommandOutcome::MemberLifecycleChanged(change) => {
            ProjectedNodeCommandOutcome::MemberLifecycleChanged(change.clone())
        }
        NodeCommandOutcome::JoinRequestsReviewed(reviewed) => {
            ProjectedNodeCommandOutcome::JoinRequestsReviewed(reviewed.clone())
        }
        NodeCommandOutcome::JoinRequestUpdated(request) => {
            ProjectedNodeCommandOutcome::JoinRequestUpdated(request.clone())
        }
        NodeCommandOutcome::InvitationCreated(invitation) => {
            ProjectedNodeCommandOutcome::InvitationCreated(invitation.clone())
        }
        NodeCommandOutcome::Invitations(invitations) => {
            ProjectedNodeCommandOutcome::Invitations(invitations.clone())
        }
        NodeCommandOutcome::InvitationStatus(status) => {
            ProjectedNodeCommandOutcome::InvitationStatus(status.clone())
        }
        NodeCommandOutcome::InvitationRevoked(status) => {
            ProjectedNodeCommandOutcome::InvitationRevoked(status.clone())
        }
        NodeCommandOutcome::InvitationPreview(preview) => {
            ProjectedNodeCommandOutcome::InvitationPreview(preview.clone())
        }
        NodeCommandOutcome::InvitationRedeemed(redemption) => {
            ProjectedNodeCommandOutcome::InvitationRedeemed(redemption.clone())
        }
        NodeCommandOutcome::NetworkLeft(left) => {
            ProjectedNodeCommandOutcome::NetworkLeft(Box::new(ProjectedNodeNetworkLeaveResult {
                network_name: left.network_name.clone(),
                left: left.left,
                snapshot: project_node_snapshot(&left.snapshot, context, now_unix_ms)?,
            }))
        }
        NodeCommandOutcome::MembershipPolicyAcknowledged(membership) => {
            ProjectedNodeCommandOutcome::MembershipPolicyAcknowledged(membership.clone())
        }
        NodeCommandOutcome::LocalExitCapability(snapshot) => {
            ProjectedNodeCommandOutcome::LocalExitCapability(snapshot.clone())
        }
        NodeCommandOutcome::IdentityRotated(rotation) => {
            ProjectedNodeCommandOutcome::IdentityRotated(rotation.clone())
        }
        NodeCommandOutcome::Configuration(configuration) => {
            ProjectedNodeCommandOutcome::Configuration(configuration.clone())
        }
        NodeCommandOutcome::ConfigurationPreview(preview) => {
            ProjectedNodeCommandOutcome::ConfigurationPreview(preview.clone())
        }
        NodeCommandOutcome::ConfigurationApplied(applied) => {
            ProjectedNodeCommandOutcome::ConfigurationApplied(applied.clone())
        }
        NodeCommandOutcome::ConfigurationHistory(history) => {
            ProjectedNodeCommandOutcome::ConfigurationHistory(history.clone())
        }
        NodeCommandOutcome::Policy(current) => ProjectedNodeCommandOutcome::Policy(current.clone()),
        NodeCommandOutcome::PolicyPreview(preview) => {
            ProjectedNodeCommandOutcome::PolicyPreview(preview.clone())
        }
        NodeCommandOutcome::PolicyApplied(applied) => {
            ProjectedNodeCommandOutcome::PolicyApplied(applied.clone())
        }
        NodeCommandOutcome::PolicyHistory(history) => {
            ProjectedNodeCommandOutcome::PolicyHistory(history.clone())
        }
        NodeCommandOutcome::PolicyRolledBack(rolled_back) => {
            ProjectedNodeCommandOutcome::PolicyRolledBack(rolled_back.clone())
        }
        NodeCommandOutcome::PolicyTest(result) => {
            ProjectedNodeCommandOutcome::PolicyTest(result.clone())
        }
        NodeCommandOutcome::TrafficPolicy(current) => {
            ProjectedNodeCommandOutcome::TrafficPolicy(current.clone())
        }
        NodeCommandOutcome::TrafficPolicyPreview(preview) => {
            ProjectedNodeCommandOutcome::TrafficPolicyPreview(preview.clone())
        }
        NodeCommandOutcome::TrafficPolicyApplied(applied) => {
            ProjectedNodeCommandOutcome::TrafficPolicyApplied(applied.clone())
        }
        NodeCommandOutcome::TrafficPolicyHistory(history) => {
            ProjectedNodeCommandOutcome::TrafficPolicyHistory(history.clone())
        }
        NodeCommandOutcome::TrafficPolicyRolledBack(rolled_back) => {
            ProjectedNodeCommandOutcome::TrafficPolicyRolledBack(rolled_back.clone())
        }
        NodeCommandOutcome::TrafficPolicyTest(result) => {
            ProjectedNodeCommandOutcome::TrafficPolicyTest(result.clone())
        }
        NodeCommandOutcome::Accepted { operation_id } => ProjectedNodeCommandOutcome::Accepted {
            operation_id: operation_id.clone(),
        },
        NodeCommandOutcome::Completed { message } => ProjectedNodeCommandOutcome::Completed {
            message: message.clone(),
        },
        NodeCommandOutcome::Rejected(problem) => {
            ProjectedNodeCommandOutcome::Rejected(problem.clone())
        }
    };
    Ok(ProjectedNodeCommandResult {
        request_id: result.request_id,
        outcome,
    })
}

/// Project an event before fan-out or transport serialization. The event keeps
/// its original cursor identity even when its content is redacted.
pub fn project_node_event(
    event: &NodeEvent,
    context: &NodeCommandContext,
    now_unix_ms: u64,
) -> Result<ProjectedNodeEvent, NodeSnapshotProjectionError> {
    let kind = match &event.kind {
        NodeEventKind::StateChanged { revision } => ProjectedNodeEventKind::StateChanged {
            revision: *revision,
        },
        NodeEventKind::NetworkChanged {
            network_name,
            status,
        } => ProjectedNodeEventKind::NetworkChanged {
            network_name: network_name.clone(),
            status: *status,
        },
        NodeEventKind::EmbeddedServiceChanged {
            network_name,
            status,
        } => ProjectedNodeEventKind::EmbeddedServiceChanged {
            network_name: network_name.clone(),
            status: *status,
        },
        NodeEventKind::LogAppended { level, message } => {
            let viewer = context.privacy_viewer_for(None);
            let policy = context
                .field_privacy_policy_for(None)
                .map_err(|_| NodeSnapshotProjectionError::InvalidFieldPolicy)?;
            let message_visible = viewer.local_owner
                || field_is_visible(
                    policy,
                    PrivacyField::NodeDetailedLogs,
                    &viewer,
                    PrivacyObjectState::Active,
                    None,
                    now_unix_ms,
                );
            ProjectedNodeEventKind::LogAppended {
                level: *level,
                message: message_visible.then(|| message.clone()),
            }
        }
        NodeEventKind::RoutingChanged {
            network_name,
            mode,
            exit_node,
            is_exit_node,
        } => ProjectedNodeEventKind::RoutingChanged {
            network_name: network_name.clone(),
            mode: *mode,
            exit_node: exit_node.clone(),
            is_exit_node: *is_exit_node,
        },
        NodeEventKind::RoleChanged {
            network_name,
            role,
            enabled,
            roles,
        } => ProjectedNodeEventKind::RoleChanged {
            network_name: network_name.clone(),
            role: *role,
            enabled: *enabled,
            roles: roles.clone(),
        },
        NodeEventKind::CommandCompleted {
            request_id,
            success,
        } => ProjectedNodeEventKind::CommandCompleted {
            request_id: *request_id,
            success: *success,
        },
        NodeEventKind::OperationChanged {
            operation_id,
            state,
            stage,
        } => ProjectedNodeEventKind::OperationChanged {
            operation_id: operation_id.clone(),
            state: *state,
            stage: stage.clone(),
        },
    };
    Ok(ProjectedNodeEvent {
        sequence: event.sequence,
        snapshot_revision: event.snapshot_revision,
        kind,
    })
}

struct SnapshotProjector<'a> {
    snapshot: &'a NodeSnapshot,
    context: &'a NodeCommandContext,
    now_unix_ms: u64,
    object_node_id: Option<NodeId>,
}

impl<'a> SnapshotProjector<'a> {
    fn new(snapshot: &'a NodeSnapshot, context: &'a NodeCommandContext, now_unix_ms: u64) -> Self {
        let object_node_id = snapshot
            .identity
            .as_ref()
            .and_then(|identity| NodeId::parse(identity.node_id.clone()).ok());
        Self {
            snapshot,
            context,
            now_unix_ms,
            object_node_id,
        }
    }

    fn local_owner(&self) -> bool {
        self.context.privacy_viewer_for(None).local_owner
    }

    fn viewer(&self, network_id: Option<&NetworkId>) -> PrivacyViewer {
        self.context.privacy_viewer_for(network_id)
    }

    fn policy(
        &self,
        network_id: Option<&NetworkId>,
    ) -> Result<Option<&FieldPrivacyPolicy>, NodeSnapshotProjectionError> {
        self.context
            .field_privacy_policy_for(network_id)
            .map_err(|_| NodeSnapshotProjectionError::InvalidFieldPolicy)
    }

    fn field_visible(
        &self,
        policy: Option<&FieldPrivacyPolicy>,
        field: PrivacyField,
        viewer: &PrivacyViewer,
        state: PrivacyObjectState,
    ) -> bool {
        viewer.local_owner
            || field_is_visible(
                policy,
                field,
                viewer,
                state,
                self.object_node_id.as_ref(),
                self.now_unix_ms,
            )
    }

    fn membership_for_network(&self, network_name: &str) -> Option<&NodeNetworkMembershipSnapshot> {
        self.snapshot
            .memberships
            .iter()
            .find(|membership| membership.network_name == network_name)
    }

    fn network_id_for_name(&self, network_name: &str) -> Option<NetworkId> {
        self.membership_for_network(network_name)
            .and_then(|membership| NetworkId::parse(membership.network_id.clone()).ok())
    }

    fn network_state(state: MembershipState) -> PrivacyObjectState {
        match state {
            MembershipState::Discovered
            | MembershipState::Applying
            | MembershipState::Accepted
            | MembershipState::KeyReady
            | MembershipState::Connecting => PrivacyObjectState::Approved,
            MembershipState::PendingApproval => PrivacyObjectState::PendingApproval,
            MembershipState::Active => PrivacyObjectState::Active,
            MembershipState::Degraded => PrivacyObjectState::Degraded,
            MembershipState::Leaving | MembershipState::Left => PrivacyObjectState::Exited,
            MembershipState::Rejected | MembershipState::Cancelled => PrivacyObjectState::Rejected,
            MembershipState::Suspended => PrivacyObjectState::Suspended,
        }
    }

    /// Determine whether a node-global field is visible at least once. Global
    /// fields are not attributed to one network; when no network grants the
    /// field, the safe result is omission.
    fn node_field_visible_any(
        &self,
        field: PrivacyField,
    ) -> Result<bool, NodeSnapshotProjectionError> {
        if self.local_owner() {
            return Ok(true);
        }
        for membership in &self.snapshot.memberships {
            let Ok(network_id) = NetworkId::parse(membership.network_id.clone()) else {
                continue;
            };
            let viewer = self.viewer(Some(&network_id));
            if !viewer.can_view_network(&network_id) {
                continue;
            }
            let policy = self.policy(Some(&network_id))?;
            if self.field_visible(
                policy,
                field,
                &viewer,
                Self::network_state(membership.state),
            ) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn project_identity(
        &self,
    ) -> Result<Option<ProjectedNodeIdentitySnapshot>, NodeSnapshotProjectionError> {
        let Some(identity) = &self.snapshot.identity else {
            return Ok(None);
        };
        let display_visible = self.node_field_visible_any(PrivacyField::NodeDisplayName)?;
        let public_key_visible = self.node_field_visible_any(PrivacyField::NodePublicKey)?;
        if !display_visible && !public_key_visible {
            return Ok(None);
        }
        Ok(Some(ProjectedNodeIdentitySnapshot {
            node_id: identity.node_id.clone(),
            display_name: display_visible
                .then(|| identity.display_name.clone())
                .flatten(),
            public_key_fingerprint: public_key_visible
                .then(|| identity.public_key_fingerprint.clone()),
            created_at_unix_ms: public_key_visible
                .then_some(identity.created_at_unix_ms)
                .flatten(),
        }))
    }

    fn project_network(
        &self,
        network: &NodeNetworkSnapshot,
    ) -> Result<Option<ProjectedNodeNetworkSnapshot>, NodeSnapshotProjectionError> {
        let membership = self.membership_for_network(&network.name);
        let network_id = self.network_id_for_name(&network.name);
        let viewer = self.viewer(network_id.as_ref());
        let policy = self.policy(network_id.as_ref())?;
        let state = membership
            .map(|membership| Self::network_state(membership.state))
            .unwrap_or(PrivacyObjectState::Active);
        if !viewer.local_owner
            && (network_id.is_none()
                || !viewer.can_view_network(network_id.as_ref().expect("checked above"))
                || !self.field_visible(
                    policy,
                    PrivacyField::TopologyNetworkMembership,
                    &viewer,
                    state,
                ))
        {
            return Ok(None);
        }

        let online = self.field_visible(policy, PrivacyField::NodeOnlineState, &viewer, state);
        let real_addresses = self.field_visible(
            policy,
            PrivacyField::NodeRealNetworkAddresses,
            &viewer,
            state,
        );
        let endpoints = self.field_visible(policy, PrivacyField::NodeEndpoints, &viewer, state);
        let traffic =
            self.field_visible(policy, PrivacyField::NodeTrafficStatistics, &viewer, state);
        let local_paths = self.field_visible(policy, PrivacyField::NodeLocalPaths, &viewer, state);
        let roles = self.field_visible(policy, PrivacyField::TopologyRole, &viewer, state);

        Ok(Some(ProjectedNodeNetworkSnapshot {
            name: network.name.clone(),
            tun_name: local_paths.then(|| network.tun_name.clone()),
            status: online.then_some(network.status),
            assigned_ipv6: real_addresses.then_some(network.assigned_ipv6).flatten(),
            assigned_ipv4: real_addresses.then_some(network.assigned_ipv4).flatten(),
            p2p_listen_addr: endpoints.then_some(network.p2p_listen_addr).flatten(),
            uptime_secs: online.then_some(network.uptime_secs),
            roles: roles.then(|| network.roles.clone()),
            peer_count: online.then_some(network.peer_count),
            traffic: traffic.then(|| network.traffic.clone()),
        }))
    }

    fn project_interfaces(
        &self,
    ) -> Result<Vec<ProjectedNodeInterfaceSnapshot>, NodeSnapshotProjectionError> {
        let addresses_visible =
            self.node_field_visible_any(PrivacyField::NodeRealNetworkAddresses)?;
        if !addresses_visible {
            return Ok(Vec::new());
        }
        Ok(self
            .snapshot
            .interfaces
            .iter()
            .map(|interface| ProjectedNodeInterfaceSnapshot {
                index: interface.index,
                name: interface.name.clone(),
                kind: interface.kind,
                ownership: interface.ownership,
                status: interface.status,
                mtu: interface.mtu,
                addresses: Some(interface.addresses.clone()),
                default_ipv4: interface.default_ipv4,
                default_ipv6: interface.default_ipv6,
            })
            .collect())
    }

    fn project_peer(
        &self,
        peer: &NodePeerSnapshot,
    ) -> Result<Option<ProjectedNodePeerSnapshot>, NodeSnapshotProjectionError> {
        let membership = self.membership_for_network(&peer.network_name);
        let network_id = self.network_id_for_name(&peer.network_name);
        let viewer = self.viewer(network_id.as_ref());
        let policy = self.policy(network_id.as_ref())?;
        let state = membership
            .map(|membership| Self::network_state(membership.state))
            .unwrap_or(PrivacyObjectState::Active);
        if !viewer.local_owner
            && (network_id.is_none()
                || !viewer.can_view_network(network_id.as_ref().expect("checked above"))
                || !self.field_visible(policy, PrivacyField::MemberNodeId, &viewer, state))
        {
            return Ok(None);
        }

        let addresses = self.field_visible(
            policy,
            PrivacyField::NodeRealNetworkAddresses,
            &viewer,
            state,
        );
        let endpoints = self.field_visible(policy, PrivacyField::NodeEndpoints, &viewer, state);
        let online = self.field_visible(policy, PrivacyField::NodeOnlineState, &viewer, state);
        let traffic =
            self.field_visible(policy, PrivacyField::NodeTrafficStatistics, &viewer, state);
        Ok(Some(ProjectedNodePeerSnapshot {
            peer_id: peer.peer_id.clone(),
            network_name: peer.network_name.clone(),
            virtual_ipv6: addresses.then_some(peer.virtual_ipv6),
            virtual_ipv4: addresses.then_some(peer.virtual_ipv4).flatten(),
            endpoints: endpoints.then(|| peer.endpoints.clone()),
            online: online.then_some(peer.online),
            p2p_active: online.then_some(peer.p2p_active),
            p2p_healthy: online.then_some(peer.p2p_healthy),
            rtt_us: online.then_some(peer.rtt_us).flatten(),
            loss_rate: online.then_some(peer.loss_rate),
            traffic: traffic.then(|| peer.traffic.clone()),
        }))
    }

    fn project_embedded_service(
        &self,
        service: &NodeEmbeddedServiceSnapshot,
    ) -> Result<Option<ProjectedNodeEmbeddedServiceSnapshot>, NodeSnapshotProjectionError> {
        let membership = self.membership_for_network(&service.network_name);
        let network_id = self.network_id_for_name(&service.network_name);
        let viewer = self.viewer(network_id.as_ref());
        let policy = self.policy(network_id.as_ref())?;
        let state = membership
            .map(|membership| Self::network_state(membership.state))
            .unwrap_or(PrivacyObjectState::Active);
        if !viewer.local_owner
            && (network_id.is_none()
                || !viewer.can_view_network(network_id.as_ref().expect("checked above"))
                || !self.field_visible(
                    policy,
                    PrivacyField::TopologyNetworkMembership,
                    &viewer,
                    state,
                ))
        {
            return Ok(None);
        }
        let endpoints = self.field_visible(policy, PrivacyField::NodeEndpoints, &viewer, state);
        Ok(Some(ProjectedNodeEmbeddedServiceSnapshot {
            network_name: service.network_name.clone(),
            status: service.status,
            signaling_addr: endpoints.then_some(service.signaling_addr).flatten(),
            relay_addr: endpoints.then_some(service.relay_addr).flatten(),
        }))
    }

    fn project_activity(
        &self,
        activity: &ActivityEntry,
    ) -> Result<Option<ProjectedActivityEntry>, NodeSnapshotProjectionError> {
        if self.local_owner() {
            return Ok(Some(ProjectedActivityEntry {
                audit_id: activity.audit_id.clone(),
                operation_id: activity.operation_id.clone(),
                network_id: activity.network_id.clone(),
                actor: Some(activity.actor.clone()),
                category: activity.category,
                severity: activity.severity,
                actor_role: activity.actor_role.clone(),
                action: Some(activity.action.clone()),
                resource: Some(activity.resource.clone()),
                object_kind: Some(activity.object_kind.clone()),
                object_id: activity.object_id.clone(),
                outcome: Some(activity.outcome),
                occurred_at_unix_ms: Some(activity.occurred_at_unix_ms),
                summary: Some(activity.summary.clone()),
                reason: activity.reason.clone(),
                details: Some(activity.details.clone()),
            }));
        }
        let viewer = self.viewer(activity.network_id.as_ref());
        let policy = self.policy(activity.network_id.as_ref())?;
        Ok(project_activity_entry(
            activity,
            &viewer,
            policy,
            self.now_unix_ms,
        ))
    }

    fn project_topology(&self) -> Result<Option<TopologySnapshot>, NodeSnapshotProjectionError> {
        let Some(topology) = &self.snapshot.topology else {
            return Ok(None);
        };
        if self.local_owner() {
            return Ok(Some(topology.clone()));
        }

        let mut projected = TopologySnapshot {
            revision: topology.revision,
            generated_at_unix_ms: topology.generated_at_unix_ms,
            viewer: topology.viewer.clone(),
            truncated: false,
            boundaries: Vec::new(),
            nodes: Vec::new(),
            connections: Vec::new(),
            clusters: Vec::new(),
        };
        for boundary in &topology.boundaries {
            let viewer = self.viewer(Some(&boundary.network_id));
            let policy = self.policy(Some(&boundary.network_id))?;
            let mut scoped = topology.clone();
            scoped.boundaries = vec![boundary.clone()];
            scoped.nodes = topology
                .nodes
                .iter()
                .filter_map(|node| {
                    let networks = node
                        .networks
                        .iter()
                        .filter(|network| network.network_id == boundary.network_id)
                        .cloned()
                        .collect::<Vec<_>>();
                    (!networks.is_empty()).then(|| TopologyNode {
                        node_id: node.node_id.clone(),
                        display_name: node.display_name.clone(),
                        is_self: node.is_self,
                        health: node.health,
                        capabilities: node.capabilities.clone(),
                        networks,
                    })
                })
                .collect();
            scoped.connections = topology
                .connections
                .iter()
                .filter_map(|connection| {
                    let networks = connection
                        .networks
                        .iter()
                        .filter(|network| **network == boundary.network_id)
                        .cloned()
                        .collect::<Vec<_>>();
                    (!networks.is_empty()).then(|| TopologyConnection {
                        connection_id: connection.connection_id.clone(),
                        from: connection.from.clone(),
                        to: connection.to.clone(),
                        networks,
                        state: connection.state,
                        observed_at_unix_ms: connection.observed_at_unix_ms,
                    })
                })
                .collect();
            scoped.clusters = topology
                .clusters
                .iter()
                .filter(|cluster| cluster.network_id == boundary.network_id)
                .cloned()
                .collect();
            let network_projection =
                project_topology_snapshot(&scoped, &viewer, policy, self.now_unix_ms);
            merge_topology(&mut projected, network_projection);
        }
        Ok(Some(projected))
    }

    fn project_exit_selection(
        &self,
        selection: &NodeExitSelectionSnapshot,
    ) -> Option<NodeExitSelectionSnapshot> {
        let viewer = self.viewer(Some(&selection.network_id));
        if viewer.local_owner
            || (viewer.principal.is_network_manager()
                && viewer.can_view_network(&selection.network_id))
        {
            return Some(selection.clone());
        }
        if !viewer.can_view_network(&selection.network_id) || !viewer.has_active_network_role() {
            return None;
        }

        // Ordinary members get only their own final decision. Group and member
        // policy rows remain hidden, and no member can inspect another node's
        // candidate explanation.
        if selection
            .resolution
            .as_ref()
            .is_some_and(|resolution| resolution.subject != viewer.node_id)
        {
            return None;
        }
        Some(NodeExitSelectionSnapshot {
            policy: None,
            ..selection.clone()
        })
    }
}

/// Project one canonical snapshot for a specific authenticated command context.
/// Invalid, duplicate, expired or not-yet-effective policies fail closed.
pub fn project_node_snapshot(
    snapshot: &NodeSnapshot,
    context: &NodeCommandContext,
    now_unix_ms: u64,
) -> Result<ProjectedNodeSnapshot, NodeSnapshotProjectionError> {
    // Reject an invalid policy set even when a particular object has no policy
    // row, so malformed trusted control-plane data never silently degrades.
    context
        .field_privacy_policy_for(None)
        .map_err(|_| NodeSnapshotProjectionError::InvalidFieldPolicy)?;

    let projector = SnapshotProjector::new(snapshot, context, now_unix_ms);
    let owner = projector.local_owner();
    let identity = projector.project_identity()?;
    let networks = snapshot
        .networks
        .iter()
        .map(|network| projector.project_network(network))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let peers = snapshot
        .peers
        .iter()
        .map(|peer| projector.project_peer(peer))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let embedded_services = snapshot
        .embedded_services
        .iter()
        .map(|service| projector.project_embedded_service(service))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let recent_activity = snapshot
        .recent_activity
        .iter()
        .map(|activity| projector.project_activity(activity))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let exit_selections = snapshot
        .exit_selections
        .iter()
        .filter_map(|selection| projector.project_exit_selection(selection))
        .collect::<Vec<_>>();

    Ok(ProjectedNodeSnapshot {
        revision: snapshot.revision,
        lifecycle: snapshot.lifecycle.clone(),
        control_health: snapshot.control_health,
        identity,
        configuration: owner.then(|| snapshot.configuration.clone()),
        exit_node_capabilities: snapshot.exit_node_capabilities.clone(),
        local_exit: snapshot.local_exit.clone(),
        capabilities: snapshot.capabilities.clone(),
        permissions: snapshot.permissions.clone(),
        access_control: if owner {
            snapshot.access_control.clone()
        } else {
            NodeAccessControlSnapshot::default()
        },
        embedded_services,
        ui_mode: if owner { snapshot.ui_mode } else { None },
        tun_capabilities: owner.then(|| snapshot.tun_capabilities.clone()),
        interfaces: projector.project_interfaces()?,
        interface_problems: if owner {
            snapshot.interface_problems.clone()
        } else {
            Vec::new()
        },
        routing_summaries: if owner {
            snapshot.routing_summaries.clone()
        } else {
            Vec::new()
        },
        traffic_routing_page: owner
            .then(|| snapshot.traffic_routing_page.clone())
            .flatten(),
        network_profiles: snapshot.network_profiles.clone(),
        exit_selections,
        memberships: if owner {
            snapshot.memberships.clone()
        } else {
            Vec::new()
        },
        discovery_index: snapshot.discovery_index.clone(),
        networks,
        peers,
        recent_activity,
        logs: if owner {
            snapshot.logs.clone()
        } else {
            Vec::new()
        },
        last_error: owner.then(|| snapshot.last_error.clone()).flatten(),
        topology: projector.project_topology()?,
    })
}

fn merge_topology(target: &mut TopologySnapshot, source: TopologySnapshot) {
    target.truncated |= source.truncated;
    target.boundaries.extend(source.boundaries);
    target.clusters.extend(source.clusters);

    for node in source.nodes {
        if let Some(existing) = target
            .nodes
            .iter_mut()
            .find(|existing| existing.node_id == node.node_id)
        {
            let existing_networks = existing
                .networks
                .iter()
                .map(|network| network.network_id.clone())
                .collect::<BTreeSet<_>>();
            existing.networks.extend(
                node.networks
                    .into_iter()
                    .filter(|network| !existing_networks.contains(&network.network_id)),
            );
            existing.capabilities.extend(node.capabilities);
            existing.capabilities.sort();
            existing.capabilities.dedup();
            if node.health.severity() > existing.health.severity() {
                existing.health = node.health;
            }
            if existing.display_name.is_none() {
                existing.display_name = node.display_name;
            }
        } else {
            target.nodes.push(node);
        }
    }

    for connection in source.connections {
        if let Some(existing) = target
            .connections
            .iter_mut()
            .find(|existing| existing.connection_id == connection.connection_id)
        {
            existing.networks.extend(connection.networks);
            existing.networks.sort();
            existing.networks.dedup();
            existing.observed_at_unix_ms = existing
                .observed_at_unix_ms
                .max(connection.observed_at_unix_ms);
        } else {
            target.connections.push(connection);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::net::IpAddr;

    use fuckipv6_common::domain::{
        ActivityEntry, AuditId, AuditOutcome, CapabilityKind, ContentHash, ExitSelectionPolicy,
        FieldPrivacyPolicy, FieldVisibility, FieldVisibilityRule, MemberLifecycleStatus,
        MembershipRole, MembershipState, NetworkMember, NetworkMemberDirectory, NetworkMemberGroup,
        NetworkMemberObservationSource, NetworkMemberOnlineState, NetworkMemberPresence,
        NetworkMemberStatus, NetworkRole, TopologyBoundary, TopologyConnection,
        TopologyConnectionState, TopologyNode, TopologyNodeHealth, TopologyNodeNetwork,
        TrafficAction, TrafficDefaultConfirmedBy, TrafficDefaultPolicy, TrafficPolicy,
        TrafficRoutingHardConstraints, TrafficRoutingPageModel, TrafficRoutingTarget,
        TrafficRoutingWorkflowState,
    };

    use super::*;
    use crate::node::{
        NetworkCommandScope, NodeIdentitySnapshot, NodeInterfaceSnapshot, NodeLogLevel,
        NodeNetworkJoinResult, NodeNetworkLeaveResult, NodePermission,
    };

    fn actor() -> NodeId {
        NodeId::from_public_key(b"privacy-projection-viewer")
    }

    fn network(name: &str) -> NetworkId {
        NetworkId::from_public_key(name.as_bytes())
    }

    fn policy(
        network_id: NetworkId,
        rules: impl IntoIterator<Item = (PrivacyField, FieldVisibility)>,
    ) -> FieldPrivacyPolicy {
        FieldPrivacyPolicy {
            network_id: Some(network_id),
            revision: 7,
            issued_at_unix_ms: 0,
            effective_at_unix_ms: 0,
            expires_at_unix_ms: u64::MAX,
            rules: rules
                .into_iter()
                .map(|(field, visibility)| FieldVisibilityRule { field, visibility })
                .collect(),
        }
    }

    fn membership(
        network_name: &str,
        network_id: &NetworkId,
        role: MembershipRole,
        state: MembershipState,
    ) -> NodeNetworkMembershipSnapshot {
        NodeNetworkMembershipSnapshot {
            network_name: network_name.into(),
            network_id: network_id.to_string(),
            node_id: actor().to_string(),
            state,
            role,
            local_policy_revision: 0,
            network_policy_revision: None,
            privacy_policy_revision: Some(7),
            member_limit: None,
            exit_contribution: None,
            exit_contribution_scope: None,
            exit_contribution_mode: None,
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

    fn local_network(network_name: &str, status: NodeNetworkStatus) -> NodeNetworkSnapshot {
        NodeNetworkSnapshot {
            name: network_name.into(),
            tun_name: format!("hidden-tun-{network_name}"),
            status,
            assigned_ipv6: Some("2001:db8::7".parse().unwrap()),
            assigned_ipv4: Some("192.0.2.7".parse().unwrap()),
            p2p_listen_addr: Some("203.0.113.7:443".parse().unwrap()),
            uptime_secs: 42,
            roles: vec![NodeNetworkRole::Relay],
            peer_count: 1,
            traffic: NodeTrafficSnapshot {
                tun_rx_bytes: 11,
                ..NodeTrafficSnapshot::default()
            },
        }
    }

    fn peer(network_name: &str) -> NodePeerSnapshot {
        NodePeerSnapshot {
            peer_id: NodeId::from_public_key(b"remote-peer").to_string(),
            network_name: network_name.into(),
            virtual_ipv6: "2001:db8::8".parse().unwrap(),
            virtual_ipv4: Some("192.0.2.8".parse().unwrap()),
            endpoints: vec!["203.0.113.8:8443".parse().unwrap()],
            online: true,
            p2p_active: true,
            p2p_healthy: true,
            rtt_us: Some(9_000),
            loss_rate: 0.5,
            traffic: NodeTrafficSnapshot {
                p2p_rx_bytes: 13,
                ..NodeTrafficSnapshot::default()
            },
        }
    }

    fn activity() -> ActivityEntry {
        ActivityEntry {
            audit_id: AuditId::from_public_key(b"privacy-activity"),
            operation_id: None,
            actor: actor(),
            network_id: Some(network("home")),
            action: "member.update".into(),
            resource: "member:remote-peer".into(),
            outcome: AuditOutcome::Succeeded,
            occurred_at_unix_ms: 1_500,
            summary: "visible activity summary".into(),
            category: fuckipv6_common::domain::ActivityCategory::UserActivity,
            severity: fuckipv6_common::domain::ActivitySeverity::Info,
            actor_role: Some("member".into()),
            object_kind: "member".into(),
            object_id: Some("remote-peer".into()),
            reason: Some("succeeded".into()),
            details: BTreeMap::from([("private_detail".into(), "hidden-detail".into())]),
        }
    }

    fn topology() -> TopologySnapshot {
        let home = network("home");
        let remote = NodeId::from_public_key(b"remote-peer");
        TopologySnapshot {
            revision: 9,
            generated_at_unix_ms: 1_500,
            viewer: actor(),
            truncated: false,
            boundaries: vec![TopologyBoundary {
                network_id: home.clone(),
                name: "home".into(),
                member_count: 2,
                visible_member_count: 2,
                viewer_role: Some(MembershipRole::Administrator),
                capabilities: Vec::new(),
            }],
            nodes: vec![
                TopologyNode {
                    node_id: actor(),
                    display_name: Some("topology-hidden-display".into()),
                    is_self: true,
                    health: TopologyNodeHealth::Healthy,
                    capabilities: Vec::new(),
                    networks: vec![TopologyNodeNetwork {
                        network_id: home.clone(),
                        role: MembershipRole::Administrator,
                        status: NetworkMemberStatus::Active,
                        presence: NetworkMemberPresence::unknown(),
                        health: TopologyNodeHealth::Healthy,
                        capabilities: Vec::new(),
                    }],
                },
                TopologyNode {
                    node_id: remote.clone(),
                    display_name: Some("remote-topology-name".into()),
                    is_self: false,
                    health: TopologyNodeHealth::Healthy,
                    capabilities: Vec::new(),
                    networks: vec![TopologyNodeNetwork {
                        network_id: home.clone(),
                        role: MembershipRole::Member,
                        status: NetworkMemberStatus::Active,
                        presence: NetworkMemberPresence::unknown(),
                        health: TopologyNodeHealth::Healthy,
                        capabilities: Vec::new(),
                    }],
                },
            ],
            connections: vec![TopologyConnection {
                connection_id: ContentHash::from_bytes(b"privacy-topology-link"),
                from: actor(),
                to: remote,
                networks: vec![home],
                state: TopologyConnectionState::Active,
                observed_at_unix_ms: Some(1_500),
            }],
            clusters: Vec::new(),
        }
    }

    fn routing_page() -> TrafficRoutingPageModel {
        let node_id = actor();
        let policy = TrafficPolicy {
            node_id: node_id.clone(),
            revision: 7,
            default_policy: TrafficDefaultPolicy {
                action: TrafficAction::Direct,
                confirmed_by: TrafficDefaultConfirmedBy::Owner,
                confirmed_at_unix_ms: 1_500,
            },
            failure_policy: Default::default(),
            rules: Vec::new(),
        };
        TrafficRoutingPageModel::from_policy(
            &policy,
            TrafficRoutingWorkflowState::Applied { revision: 7 },
            TrafficRoutingHardConstraints::default(),
            vec![TrafficRoutingTarget::direct()],
            true,
        )
        .expect("routing page")
    }

    fn full_snapshot() -> NodeSnapshot {
        let home = network("home");
        let field_policy = policy(
            home.clone(),
            [
                (PrivacyField::NodeDisplayName, FieldVisibility::OwnerOnly),
                (PrivacyField::NodePublicKey, FieldVisibility::OwnerOnly),
                (PrivacyField::NodeEndpoints, FieldVisibility::OwnerOnly),
                (
                    PrivacyField::NodeRealNetworkAddresses,
                    FieldVisibility::OwnerOnly,
                ),
                (
                    PrivacyField::NodeTrafficStatistics,
                    FieldVisibility::OwnerOnly,
                ),
                (PrivacyField::NodeLocalPaths, FieldVisibility::OwnerOnly),
                (PrivacyField::NodeDetailedLogs, FieldVisibility::OwnerOnly),
                (PrivacyField::ActivityDetails, FieldVisibility::OwnerOnly),
                (
                    PrivacyField::TopologyDisplayName,
                    FieldVisibility::OwnerOnly,
                ),
                (PrivacyField::TopologyNodeId, FieldVisibility::OwnerOnly),
                (
                    PrivacyField::TopologyConnections,
                    FieldVisibility::OwnerOnly,
                ),
            ],
        );
        NodeSnapshot {
            revision: 11,
            lifecycle: NodeLifecycleSnapshot::default(),
            control_health: NodeControlHealth::Available,
            identity: Some(NodeIdentitySnapshot {
                node_id: actor().to_string(),
                display_name: Some("hidden-display-name".into()),
                public_key_fingerprint: "hidden-fingerprint".into(),
                created_at_unix_ms: Some(10),
            }),
            configuration: NodeConfigurationSnapshot::default(),
            exit_node_capabilities: ExitNodeCapabilities::default(),
            local_exit: Default::default(),
            capabilities: NodeCapabilityRegistrySnapshot::default(),
            permissions: NodePermissionSnapshot::default(),
            access_control: NodeAccessControlSnapshot {
                local_policy: None,
                network_acls: Vec::new(),
                field_privacy_policies: vec![field_policy],
            },
            embedded_services: vec![NodeEmbeddedServiceSnapshot {
                network_name: "home".into(),
                status: NodeEmbeddedServiceStatus::Running,
                signaling_addr: Some("203.0.113.9:443".parse().unwrap()),
                relay_addr: Some("203.0.113.10:443".parse().unwrap()),
            }],
            ui_mode: None,
            tun_capabilities: TunCapabilities::default(),
            interfaces: vec![NodeInterfaceSnapshot {
                index: 1,
                name: "fk-home".into(),
                kind: NodeInterfaceKind::Tun,
                ownership: NodeInterfaceOwnership::Product,
                status: NodeInterfaceStatus::Up,
                mtu: 1420,
                addresses: vec![NodeInterfaceAddress {
                    address: "198.51.100.9".parse::<IpAddr>().unwrap(),
                    prefix_len: 24,
                }],
                default_ipv4: false,
                default_ipv6: false,
            }],
            interface_problems: Vec::new(),
            routing_summaries: Vec::new(),
            traffic_routing_page: None,
            network_profiles: Vec::new(),
            exit_selections: vec![exit_selection(home.clone())],
            memberships: vec![membership(
                "home",
                &home,
                MembershipRole::Administrator,
                MembershipState::Active,
            )],
            discovery_index: NodeDiscoveryIndexSnapshot::default(),
            networks: vec![local_network("home", NodeNetworkStatus::Online)],
            peers: vec![peer("home")],
            recent_activity: vec![activity()],
            logs: vec![NodeLogEntry {
                unix_ms: 1_500,
                level: NodeLogLevel::Info,
                target: "hidden-log-target".into(),
                message: "hidden-log-message".into(),
            }],
            last_error: None,
            topology: Some(topology()),
        }
    }

    fn network_context(
        network_id: NetworkId,
        role: NetworkRole,
        state: MembershipState,
        policies: Vec<FieldPrivacyPolicy>,
    ) -> NodeCommandContext {
        let label = if network_id == network("home") {
            "home"
        } else {
            "other"
        };
        NodeCommandContext::network_session(
            actor(),
            "privacy-projection-test",
            [NodePermission::ViewState],
            [(
                label.into(),
                NetworkCommandScope::new(network_id, role, state, Vec::new()),
            )],
            true,
        )
        .with_access_control_snapshot(&NodeAccessControlSnapshot {
            local_policy: None,
            network_acls: Vec::new(),
            field_privacy_policies: policies,
        })
    }

    fn exit_selection(network_id: NetworkId) -> NodeExitSelectionSnapshot {
        let policy = ExitSelectionPolicy::default();
        let resolution = policy
            .resolve(9, &actor(), &std::collections::BTreeMap::new())
            .expect("empty deny-first resolution");
        NodeExitSelectionSnapshot {
            network_id,
            policy_revision: 9,
            policy: Some(policy),
            resolution: Some(resolution),
            directory_generated_at_unix_ms: Some(1_400),
            directory_received_at_unix_ms: Some(1_450),
            evaluated_at_unix_ms: 1_500,
            reason_code: None,
        }
    }

    #[test]
    fn non_owner_serialization_omits_hidden_fields_and_control_plane_data() {
        let snapshot = full_snapshot();
        let original = snapshot.clone();
        let home = network("home");
        let policy = snapshot.access_control.field_privacy_policies[0].clone();
        let context = network_context(
            home,
            NetworkRole::Administrator,
            MembershipState::Active,
            vec![policy],
        );

        let projected = project_node_snapshot(&snapshot, &context, 1_500).unwrap();
        let json = serde_json::to_value(&projected).unwrap();

        assert_eq!(
            snapshot, original,
            "projection must not mutate canonical state"
        );
        assert!(projected.identity.is_none());
        assert!(projected.configuration.is_none());
        assert!(projected.ui_mode.is_none());
        assert!(projected.tun_capabilities.is_none());
        assert!(projected.interfaces.is_empty());
        assert!(projected.memberships.is_empty());
        assert!(projected.logs.is_empty());
        for key in [
            "identity",
            "configuration",
            "access_control",
            "ui_mode",
            "tun_capabilities",
            "memberships",
            "logs",
        ] {
            assert!(json.get(key).is_none(), "hidden key leaked: {key}");
        }

        let network = &json["networks"][0];
        for key in [
            "tun_name",
            "assigned_ipv6",
            "assigned_ipv4",
            "p2p_listen_addr",
            "traffic",
        ] {
            assert!(
                network.get(key).is_none(),
                "hidden network key leaked: {key}"
            );
        }
        let peer = &json["peers"][0];
        for key in ["virtual_ipv6", "virtual_ipv4", "endpoints", "traffic"] {
            assert!(peer.get(key).is_none(), "hidden peer key leaked: {key}");
        }
        assert!(json["recent_activity"][0].get("details").is_none());
        assert!(json["topology"]["nodes"].as_array().unwrap().is_empty());
        assert!(json["topology"]["connections"]
            .as_array()
            .unwrap()
            .is_empty());

        let serialized = json.to_string();
        for secret in [
            "hidden-display-name",
            "hidden-fingerprint",
            "hidden-tun-home",
            "203.0.113.7:443",
            "203.0.113.8:8443",
            "203.0.113.9:443",
            "203.0.113.10:443",
            "198.51.100.9",
            "hidden-detail",
            "hidden-log-target",
            "hidden-log-message",
            "topology-hidden-display",
        ] {
            assert!(
                !serialized.contains(secret),
                "hidden value leaked: {secret}"
            );
        }
    }

    #[test]
    fn exit_selection_projection_hides_policy_rules_from_ordinary_members() {
        let snapshot = full_snapshot();
        let home = network("home");
        let member = network_context(
            home.clone(),
            NetworkRole::Member,
            MembershipState::Active,
            snapshot.access_control.field_privacy_policies.clone(),
        );
        let projected = project_node_snapshot(&snapshot, &member, 1_500).unwrap();
        assert_eq!(projected.exit_selections.len(), 1);
        assert!(projected.exit_selections[0].policy.is_none());
        assert_eq!(
            projected.exit_selections[0]
                .resolution
                .as_ref()
                .expect("member keeps own resolution")
                .subject,
            actor()
        );

        let administrator = network_context(
            home.clone(),
            NetworkRole::Administrator,
            MembershipState::Active,
            snapshot.access_control.field_privacy_policies.clone(),
        );
        let projected = project_node_snapshot(&snapshot, &administrator, 1_500).unwrap();
        assert_eq!(projected.exit_selections[0], snapshot.exit_selections[0]);

        let local_owner = NodeCommandContext::local_owner_for_snapshot(&snapshot, "owner-test");
        let projected = project_node_snapshot(&snapshot, &local_owner, 1_500).unwrap();
        assert_eq!(projected.exit_selections[0], snapshot.exit_selections[0]);

        let other_network = network("other");
        let outsider = network_context(
            other_network,
            NetworkRole::Member,
            MembershipState::Active,
            Vec::new(),
        );
        let projected = project_node_snapshot(&snapshot, &outsider, 1_500).unwrap();
        assert!(projected.exit_selections.is_empty());
    }

    #[test]
    fn exit_selection_projection_never_exposes_another_subjects_resolution() {
        let mut snapshot = full_snapshot();
        snapshot.exit_selections[0]
            .resolution
            .as_mut()
            .expect("resolution")
            .subject = NodeId::from_public_key(b"another-member");
        let home = network("home");
        let member = network_context(
            home,
            NetworkRole::Member,
            MembershipState::Active,
            snapshot.access_control.field_privacy_policies.clone(),
        );

        let projected = project_node_snapshot(&snapshot, &member, 1_500).unwrap();

        assert!(projected.exit_selections.is_empty());
    }

    #[test]
    fn local_owner_keeps_complete_control_plane_snapshot() {
        let mut snapshot = full_snapshot();
        snapshot.traffic_routing_page = Some(routing_page());
        let context = NodeCommandContext::local_owner_for_snapshot(&snapshot, "owner-test");
        let projected = project_node_snapshot(&snapshot, &context, 1_500).unwrap();

        assert_eq!(
            projected.configuration,
            Some(snapshot.configuration.clone())
        );
        assert_eq!(projected.access_control, snapshot.access_control);
        assert_eq!(projected.memberships, snapshot.memberships);
        assert_eq!(projected.logs, snapshot.logs);
        assert_eq!(projected.topology, snapshot.topology);
        assert_eq!(projected.interfaces.len(), snapshot.interfaces.len());
        assert_eq!(
            projected.traffic_routing_page,
            snapshot.traffic_routing_page
        );
        assert!(projected.tun_capabilities.is_some());
    }

    #[test]
    fn traffic_routing_editor_projection_is_omitted_for_remote_members() {
        let home = network("home");
        let mut snapshot = full_snapshot();
        snapshot.traffic_routing_page = Some(routing_page());
        snapshot.access_control.field_privacy_policies = vec![policy(home.clone(), [])];
        let context = network_context(
            home,
            NetworkRole::Administrator,
            MembershipState::Active,
            snapshot.access_control.field_privacy_policies.clone(),
        );

        let projected = project_node_snapshot(&snapshot, &context, 1_500).unwrap();
        assert!(projected.traffic_routing_page.is_none());
        let json = serde_json::to_value(projected).unwrap();
        assert!(json.get("traffic_routing_page").is_none());
    }

    #[test]
    fn network_administrator_does_not_inherit_role_in_another_network() {
        let home = network("home");
        let other = network("other");
        let mut snapshot = full_snapshot();
        snapshot.memberships.push(membership(
            "other",
            &other,
            MembershipRole::Member,
            MembershipState::Active,
        ));
        snapshot
            .networks
            .push(local_network("other", NodeNetworkStatus::Online));
        snapshot.peers.push(peer("other"));
        snapshot.access_control.field_privacy_policies = vec![
            policy(
                home.clone(),
                [(PrivacyField::NodeEndpoints, FieldVisibility::Network)],
            ),
            policy(
                other.clone(),
                [(PrivacyField::NodeEndpoints, FieldVisibility::Network)],
            ),
        ];
        let context = network_context(
            home.clone(),
            NetworkRole::Administrator,
            MembershipState::Active,
            snapshot.access_control.field_privacy_policies.clone(),
        );

        let projected = project_node_snapshot(&snapshot, &context, 1_500).unwrap();
        assert_eq!(projected.networks.len(), 1);
        assert_eq!(projected.networks[0].name, "home");
        assert_eq!(projected.peers.len(), 1);
        assert_eq!(projected.peers[0].network_name, "home");
        assert_eq!(projected.topology.as_ref().unwrap().boundaries.len(), 1);
        assert_eq!(
            projected.topology.as_ref().unwrap().boundaries[0].network_id,
            home
        );
    }

    #[test]
    fn temporary_coordinator_remains_a_regular_member_for_fields() {
        let home = network("home");
        let mut snapshot = full_snapshot();
        let policy = policy(
            home.clone(),
            [
                (PrivacyField::NodeEndpoints, FieldVisibility::Administrators),
                (
                    PrivacyField::TopologyCapabilities,
                    FieldVisibility::Administrators,
                ),
            ],
        );
        snapshot.access_control.field_privacy_policies = vec![policy.clone()];
        let context = network_context(
            home.clone(),
            NetworkRole::TemporaryCoordinator,
            MembershipState::Active,
            vec![policy],
        );

        let projected = project_node_snapshot(&snapshot, &context, 1_500).unwrap();
        assert!(projected.networks[0].p2p_listen_addr.is_none());
        assert!(projected.peers[0].endpoints.is_none());
        assert!(projected.embedded_services[0].signaling_addr.is_none());
        assert!(projected.topology.as_ref().unwrap().boundaries[0]
            .capabilities
            .is_empty());
    }

    #[test]
    fn suspended_membership_hides_network_scoped_objects() {
        let home = network("home");
        let mut snapshot = full_snapshot();
        snapshot.access_control.field_privacy_policies = vec![policy(home.clone(), [])];
        let context = network_context(
            home,
            NetworkRole::Member,
            MembershipState::Suspended,
            snapshot.access_control.field_privacy_policies.clone(),
        );
        let projected = project_node_snapshot(&snapshot, &context, 1_500).unwrap();

        assert!(projected.identity.is_none());
        assert!(projected.networks.is_empty());
        assert!(projected.peers.is_empty());
        assert!(projected.embedded_services.is_empty());
        assert!(projected.recent_activity.is_empty());
        assert!(projected.topology.as_ref().unwrap().boundaries.is_empty());
    }

    #[test]
    fn ambiguous_field_policies_fail_closed_before_projection() {
        let home = network("home");
        let snapshot = full_snapshot();
        let policy = policy(home.clone(), []);
        let context = network_context(
            home,
            NetworkRole::Member,
            MembershipState::Active,
            vec![policy.clone(), policy],
        );
        assert_eq!(
            project_node_snapshot(&snapshot, &context, 1_500),
            Err(NodeSnapshotProjectionError::InvalidFieldPolicy)
        );
    }

    fn directory_member(node_name: &str, display_name: &str, fingerprint: &str) -> NetworkMember {
        NetworkMember {
            node_id: NodeId::from_public_key(node_name.as_bytes()),
            display_name: Some(display_name.into()),
            identity_fingerprint: Some(fingerprint.into()),
            role: MembershipRole::Member,
            groups: vec![NetworkMemberGroup::Member],
            capabilities: Some(vec![CapabilityKind::Relay]),
            status: NetworkMemberStatus::Active,
            lifecycle_state: MemberLifecycleStatus::Active,
            revision: 3,
            access_epoch: 4,
            available_actions: Vec::new(),
            presence: NetworkMemberPresence {
                state: NetworkMemberOnlineState::Online,
                source: NetworkMemberObservationSource::LiveSession,
                observed_at_unix_ms: Some(1_400),
            },
            exit_availability: Default::default(),
            joined_at_unix_ms: Some(1_300),
        }
    }

    fn member_directory(network_id: NetworkId) -> NetworkMemberDirectory {
        NetworkMemberDirectory {
            network_id,
            generated_at_unix_ms: 1_500,
            viewer_role: MembershipRole::Member,
            can_manage_members: false,
            exit_selection_policy: Default::default(),
            exit_selection_policy_revision: 0,
            members: vec![
                directory_member("member-a", "hidden-member-a", "hidden-fingerprint-a"),
                directory_member("member-b", "hidden-member-b", "hidden-fingerprint-b"),
            ],
        }
    }

    #[test]
    fn snapshot_command_result_serialization_omits_hidden_fields_and_values() {
        let snapshot = full_snapshot();
        let home = network("home");
        let context = network_context(
            home,
            NetworkRole::Administrator,
            MembershipState::Active,
            snapshot.access_control.field_privacy_policies.clone(),
        );
        let result = NodeCommandResult::snapshot(17, snapshot);

        let projected = project_node_command_result(&result, &context, 1_500).unwrap();
        let json = serde_json::to_value(&projected).unwrap();
        let data = &json["outcome"]["data"];

        for key in [
            "identity",
            "configuration",
            "access_control",
            "ui_mode",
            "tun_capabilities",
            "memberships",
            "logs",
        ] {
            assert!(data.get(key).is_none(), "hidden key leaked: {key}");
        }
        let serialized = json.to_string();
        for secret in [
            "hidden-display-name",
            "hidden-fingerprint",
            "hidden-tun-home",
            "203.0.113.7:443",
            "hidden-log-message",
        ] {
            assert!(
                !serialized.contains(secret),
                "hidden value leaked: {secret}"
            );
        }
    }

    #[test]
    fn policy_test_results_are_preserved_without_projection_side_effects() {
        let home = network("home");
        let input = fuckipv6_common::domain::parse_policy_test_input(
            &serde_json::json!({
                "network_id": home.to_string(),
                "actor": actor().to_string(),
                "role": "member",
                "groups": [],
                "membership_active": true,
                "recent_reauthentication": true,
                "emergency_coordinator": false,
                "request": {
                    "action": "read",
                    "resource": "network",
                    "resource_network_id": home.to_string(),
                    "intent": "read_only"
                },
                "now_unix_ms": 1_500
            })
            .to_string(),
        )
        .expect("valid policy test input");
        let tested = input.evaluate();
        let result = NodeCommandResult::completed(
            25,
            NodeCommandOutcome::PolicyTest(Box::new(tested.clone())),
        );
        let context = network_context(
            home,
            NetworkRole::Member,
            MembershipState::Active,
            Vec::new(),
        );
        let projected = project_node_command_result(&result, &context, 1_500)
            .expect("policy test result projection");
        let ProjectedNodeCommandOutcome::PolicyTest(projected_result) = projected.outcome else {
            panic!("expected policy test projection");
        };
        assert_eq!(*projected_result, tested);
    }

    #[test]
    fn join_and_leave_command_results_project_nested_snapshots() {
        let snapshot = full_snapshot();
        let home = network("home");
        let context = network_context(
            home.clone(),
            NetworkRole::Member,
            MembershipState::Active,
            snapshot.access_control.field_privacy_policies.clone(),
        );
        let membership = membership(
            "home",
            &home,
            MembershipRole::Member,
            MembershipState::Active,
        );
        let join_result = NodeCommandResult::completed(
            18,
            NodeCommandOutcome::NetworkJoined(Box::new(NodeNetworkJoinResult {
                membership,
                snapshot: snapshot.clone(),
                idempotent: false,
                config_saved: true,
                started: true,
            })),
        );
        let leave_result = NodeCommandResult::completed(
            19,
            NodeCommandOutcome::NetworkLeft(Box::new(NodeNetworkLeaveResult {
                network_name: "home".into(),
                left: true,
                snapshot,
            })),
        );

        let projected_join = project_node_command_result(&join_result, &context, 1_500).unwrap();
        let ProjectedNodeCommandOutcome::NetworkJoined(joined) = projected_join.outcome else {
            panic!("expected projected join result");
        };
        assert!(joined.snapshot.configuration.is_none());
        assert!(joined.snapshot.access_control.is_empty());
        assert!(joined.snapshot.memberships.is_empty());
        assert!(joined.snapshot.logs.is_empty());

        let projected_leave = project_node_command_result(&leave_result, &context, 1_500).unwrap();
        let ProjectedNodeCommandOutcome::NetworkLeft(left) = projected_leave.outcome else {
            panic!("expected projected leave result");
        };
        assert!(left.snapshot.configuration.is_none());
        assert!(left.snapshot.access_control.is_empty());
        assert!(left.snapshot.memberships.is_empty());
        assert!(left.snapshot.logs.is_empty());
    }

    #[test]
    fn non_owner_member_directory_hides_fields_and_counts_only_projected_rows() {
        let home = network("home");
        let directory = member_directory(home.clone());
        let optional_hidden = policy(
            home.clone(),
            [
                (PrivacyField::MemberDisplayName, FieldVisibility::OwnerOnly),
                (
                    PrivacyField::MemberIdentityFingerprint,
                    FieldVisibility::OwnerOnly,
                ),
                (PrivacyField::MemberCapabilities, FieldVisibility::OwnerOnly),
            ],
        );
        let context = network_context(
            home.clone(),
            NetworkRole::Member,
            MembershipState::Active,
            vec![optional_hidden],
        );
        let result = NodeCommandResult::completed(
            20,
            NodeCommandOutcome::NetworkMembers(Box::new(directory.clone())),
        );
        let projected = project_node_command_result(&result, &context, 1_500).unwrap();
        let ProjectedNodeCommandOutcome::NetworkMembers(ref projected_directory) =
            projected.outcome
        else {
            panic!("expected projected member directory");
        };
        let projected_directory = projected_directory.as_directory();
        assert_eq!(projected_directory.members.len(), 2);
        for member in &projected_directory.members {
            assert!(member.display_name.is_none());
            assert!(member.identity_fingerprint.is_none());
            assert!(member.capabilities.is_none());
        }
        let json = serde_json::to_value(&projected).unwrap().to_string();
        for secret in ["hidden-member-a", "hidden-member-b", "hidden-fingerprint-a"] {
            assert!(
                !json.contains(secret),
                "hidden member value leaked: {secret}"
            );
        }

        let required_hidden = policy(
            home.clone(),
            [(PrivacyField::MemberNodeId, FieldVisibility::OwnerOnly)],
        );
        let context = network_context(
            home,
            NetworkRole::Member,
            MembershipState::Active,
            vec![required_hidden],
        );
        let projected = project_node_command_result(&result, &context, 1_500).unwrap();
        let ProjectedNodeCommandOutcome::NetworkMembers(projected_directory) = projected.outcome
        else {
            panic!("expected projected member directory");
        };
        assert_eq!(directory.members.len(), 2);
        assert!(projected_directory.as_directory().members.is_empty());
    }

    #[test]
    fn local_owner_member_directory_remains_complete() {
        let home = network("home");
        let directory = member_directory(home.clone());
        let snapshot = full_snapshot();
        let context = NodeCommandContext::local_owner_for_snapshot(&snapshot, "owner-test");
        let result = NodeCommandResult::completed(
            21,
            NodeCommandOutcome::NetworkMembers(Box::new(directory.clone())),
        );

        let projected = project_node_command_result(&result, &context, 1_500).unwrap();
        let ProjectedNodeCommandOutcome::NetworkMembers(projected_directory) = projected.outcome
        else {
            panic!("expected projected member directory");
        };
        assert_eq!(projected_directory.as_directory(), &directory);
    }

    #[test]
    fn non_owner_log_event_omits_message_but_preserves_cursor_identity() {
        let home = network("home");
        let context = network_context(
            home.clone(),
            NetworkRole::Member,
            MembershipState::Active,
            vec![policy(
                home,
                [(PrivacyField::NodeDetailedLogs, FieldVisibility::OwnerOnly)],
            )],
        );
        let event = NodeEvent {
            sequence: 91,
            snapshot_revision: 12,
            kind: NodeEventKind::LogAppended {
                level: NodeLogLevel::Warn,
                message: "hidden-event-message".into(),
            },
        };

        let projected = project_node_event(&event, &context, 1_500).unwrap();
        assert_eq!(projected.sequence, 91);
        assert_eq!(projected.snapshot_revision, 12);
        let ProjectedNodeEventKind::LogAppended { level, message } = &projected.kind else {
            panic!("expected projected log event");
        };
        assert_eq!(*level, NodeLogLevel::Warn);
        assert!(message.is_none());
        let json = serde_json::to_value(&projected).unwrap();
        assert!(json["kind"]["data"].get("message").is_none());
        assert!(!json.to_string().contains("hidden-event-message"));
    }

    #[test]
    fn local_owner_log_event_keeps_message() {
        let snapshot = full_snapshot();
        let context = NodeCommandContext::local_owner_for_snapshot(&snapshot, "owner-test");
        let event = NodeEvent {
            sequence: 92,
            snapshot_revision: 12,
            kind: NodeEventKind::LogAppended {
                level: NodeLogLevel::Info,
                message: "owner-visible-message".into(),
            },
        };

        let projected = project_node_event(&event, &context, 1_500).unwrap();
        let ProjectedNodeEventKind::LogAppended { message, .. } = projected.kind else {
            panic!("expected projected log event");
        };
        assert_eq!(message.as_deref(), Some("owner-visible-message"));
    }

    #[test]
    fn malformed_duplicate_policies_fail_closed_for_results_and_events() {
        let home = network("home");
        let duplicate = policy(
            home.clone(),
            [(PrivacyField::NodeDetailedLogs, FieldVisibility::OwnerOnly)],
        );
        let context = network_context(
            home,
            NetworkRole::Member,
            MembershipState::Active,
            vec![duplicate.clone(), duplicate],
        );
        let result = NodeCommandResult::snapshot(22, full_snapshot());
        let event = NodeEvent {
            sequence: 93,
            snapshot_revision: 12,
            kind: NodeEventKind::LogAppended {
                level: NodeLogLevel::Info,
                message: "must-not-leak".into(),
            },
        };

        assert_eq!(
            project_node_command_result(&result, &context, 1_500),
            Err(NodeSnapshotProjectionError::InvalidFieldPolicy)
        );
        assert_eq!(
            project_node_event(&event, &context, 1_500),
            Err(NodeSnapshotProjectionError::InvalidFieldPolicy)
        );
    }

    #[test]
    fn command_projection_does_not_mutate_canonical_snapshot_or_directory() {
        let home = network("home");
        let snapshot = full_snapshot();
        let original_snapshot = snapshot.clone();
        let directory = member_directory(home.clone());
        let original_directory = directory.clone();
        let context = network_context(
            home,
            NetworkRole::Member,
            MembershipState::Active,
            snapshot.access_control.field_privacy_policies.clone(),
        );
        let result = NodeCommandResult::completed(
            23,
            NodeCommandOutcome::NetworkJoined(Box::new(NodeNetworkJoinResult {
                membership: membership(
                    "home",
                    &network("home"),
                    MembershipRole::Member,
                    MembershipState::Active,
                ),
                snapshot: snapshot.clone(),
                idempotent: false,
                config_saved: false,
                started: false,
            })),
        );
        let member_result = NodeCommandResult::completed(
            24,
            NodeCommandOutcome::NetworkMembers(Box::new(directory)),
        );

        project_node_command_result(&result, &context, 1_500).unwrap();
        project_node_command_result(&member_result, &context, 1_500).unwrap();
        assert_eq!(snapshot, original_snapshot);
        assert_eq!(
            member_result.outcome,
            NodeCommandOutcome::NetworkMembers(Box::new(original_directory))
        );
    }

    #[test]
    fn public_network_boundary_requires_explicit_topology_visibility() {
        let home = network("home");
        let snapshot = topology();
        let viewer = PrivacyViewer::public(actor(), Some(home.clone()));
        let default_projection = project_topology_snapshot(&snapshot, &viewer, None, 1_500);
        assert!(default_projection.boundaries.is_empty());
        assert!(default_projection.nodes.is_empty());

        let public_policy = policy(
            home,
            [(
                PrivacyField::TopologyNetworkMembership,
                FieldVisibility::Public,
            )],
        );
        let public_projection =
            project_topology_snapshot(&snapshot, &viewer, Some(&public_policy), 1_500);
        assert_eq!(public_projection.boundaries.len(), 1);
    }
}
