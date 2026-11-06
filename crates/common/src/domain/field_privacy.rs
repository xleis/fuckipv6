//! Versioned, server-side field disclosure policy.
//!
//! This module models disclosure as an audience selected by a versioned policy.
//! Product defaults apply when no explicit rule exists. Node-local process,
//! path and log fields are permanently locked to the local owner. Serialization
//! happens after this projection; a client-side hide is never a privacy boundary.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::audit::{ActivityEntry, AuditOutcome};
use super::file_share::{
    ConcurrentNamePolicy, FileAcl, FileDeletePolicy, FileQuota, FileShare, FileShareMode,
    SymlinkPolicy,
};
use super::ids::{AuditId, FileShareId, NetworkId, NodeId, OperationId};
use super::membership::{
    MembershipRole, MembershipState, NetworkMember, NetworkMemberDirectory, NetworkMemberStatus,
};
use super::membership_lifecycle::MemberLifecycleStatus;
use super::topology::{TopologyConnection, TopologyNodeNetwork, TopologySnapshot};
use super::{DomainError, DomainResult};

/// Audience ceiling for one field.
///
/// The ordering is from least to most restrictive. `serde` aliases preserve
/// compatibility with the pre-W10 three-level node privacy documents.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum FieldVisibility {
    Public,
    #[serde(alias = "network_members")]
    Network,
    #[serde(
        alias = "network_admins",
        alias = "administrator_only",
        alias = "administrators_only"
    )]
    Administrators,
    #[serde(alias = "node_owner", alias = "owner_only")]
    OwnerOnly,
}

impl FieldVisibility {
    /// A value is visible when the viewer has the configured network audience.
    /// Owner-only fields depend on the local owner relation, not on network
    /// governance, so a network owner cannot observe another node's local data.
    pub const fn allows(self, viewer: &PrivacyViewer) -> bool {
        match self {
            Self::Public => true,
            Self::Network => viewer.has_active_network_role(),
            Self::Administrators => {
                viewer.has_active_network_role() && viewer.principal.is_administrator()
            }
            Self::OwnerOnly => viewer.local_owner,
        }
    }

    /// Use the stricter of a local floor and a network-authored setting.
    pub const fn stricter(self, other: Self) -> Self {
        if self as u8 >= other as u8 {
            self
        } else {
            other
        }
    }
}

/// Verified viewer class. Node ownership and network governance never imply
/// one another; `local_owner` is kept separately from the network role.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyPrincipal {
    UnauthenticatedPublic,
    DiscoveryGuest,
    NetworkMember,
    NetworkAdministrator,
    NetworkOwner,
    NodeOwner,
}

impl PrivacyPrincipal {
    pub const fn is_network_member(self) -> bool {
        matches!(
            self,
            Self::NetworkMember | Self::NetworkAdministrator | Self::NetworkOwner
        )
    }

    pub const fn is_administrator(self) -> bool {
        matches!(self, Self::NetworkAdministrator | Self::NetworkOwner)
    }

    pub const fn is_network_manager(self) -> bool {
        matches!(self, Self::NetworkAdministrator | Self::NetworkOwner)
    }
}

/// Durable object lifecycle used by the disclosure matrix. A hidden or
/// ineligible object is removed before any field projection, so counts and
/// search indexes cannot infer its existence.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyObjectState {
    Invited,
    PendingApproval,
    Approved,
    Active,
    Degraded,
    Suspended,
    Exited,
    Removed,
    Expired,
    Rejected,
    Public,
}

impl PrivacyObjectState {
    pub const ALL: [Self; 11] = [
        Self::Invited,
        Self::PendingApproval,
        Self::Approved,
        Self::Active,
        Self::Degraded,
        Self::Suspended,
        Self::Exited,
        Self::Removed,
        Self::Expired,
        Self::Rejected,
        Self::Public,
    ];

    pub const fn is_publicly_listable(self) -> bool {
        matches!(
            self,
            Self::Approved | Self::Active | Self::Degraded | Self::Public
        )
    }
}

impl From<MemberLifecycleStatus> for PrivacyObjectState {
    fn from(value: MemberLifecycleStatus) -> Self {
        match value {
            MemberLifecycleStatus::Invited => Self::Invited,
            MemberLifecycleStatus::PendingApproval => Self::PendingApproval,
            MemberLifecycleStatus::Approved => Self::Approved,
            MemberLifecycleStatus::Active => Self::Active,
            MemberLifecycleStatus::Degraded => Self::Degraded,
            MemberLifecycleStatus::Suspended => Self::Suspended,
            MemberLifecycleStatus::Exited => Self::Exited,
            MemberLifecycleStatus::Removed => Self::Removed,
            MemberLifecycleStatus::Expired => Self::Expired,
            MemberLifecycleStatus::Rejected => Self::Rejected,
        }
    }
}

/// The five object families covered by W10-03.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyObjectKind {
    Member,
    Node,
    Topology,
    Activity,
    FileMetadata,
}

/// Stable field identifiers. Serialized policies and public previews use these
/// names verbatim; hidden values are never represented by a masked placeholder.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyField {
    MemberNodeId,
    MemberDisplayName,
    MemberIdentityFingerprint,
    MemberRole,
    MemberGroups,
    MemberCapabilities,
    MemberStatus,
    MemberLifecycleState,
    MemberRevision,
    MemberAccessEpoch,
    MemberAvailableActions,
    MemberPresence,
    MemberJoinedAt,

    NodeDisplayName,
    NodePublicKey,
    NodePlatformAndVersion,
    NodeOnlineState,
    NodeEndpoints,
    NodeRealNetworkAddresses,
    NodeTrafficStatistics,
    NodeLocalProcesses,
    NodeLocalPaths,
    NodeDetailedLogs,

    TopologyNodeId,
    TopologyDisplayName,
    TopologyRole,
    TopologyStatus,
    TopologyPresence,
    TopologyHealth,
    TopologyCapabilities,
    TopologyNetworkMembership,
    TopologyConnections,

    ActivityActor,
    ActivityAction,
    ActivityResource,
    ActivityOutcome,
    ActivityOccurredAt,
    ActivitySummary,
    ActivityDetails,

    FileId,
    FileNetworkId,
    FileDisplayName,
    FileOwner,
    FileMode,
    FileRevision,
    FileNetworkAcl,
    FileLocalAcl,
    FileQuota,
    FilePathPolicy,
    FileTimestamps,
    FileAuditSettings,
}

impl PrivacyField {
    pub const ALL: [Self; 51] = [
        Self::MemberNodeId,
        Self::MemberDisplayName,
        Self::MemberIdentityFingerprint,
        Self::MemberRole,
        Self::MemberGroups,
        Self::MemberCapabilities,
        Self::MemberStatus,
        Self::MemberLifecycleState,
        Self::MemberRevision,
        Self::MemberAccessEpoch,
        Self::MemberAvailableActions,
        Self::MemberPresence,
        Self::MemberJoinedAt,
        Self::NodeDisplayName,
        Self::NodePublicKey,
        Self::NodePlatformAndVersion,
        Self::NodeOnlineState,
        Self::NodeEndpoints,
        Self::NodeRealNetworkAddresses,
        Self::NodeTrafficStatistics,
        Self::NodeLocalProcesses,
        Self::NodeLocalPaths,
        Self::NodeDetailedLogs,
        Self::TopologyNodeId,
        Self::TopologyDisplayName,
        Self::TopologyRole,
        Self::TopologyStatus,
        Self::TopologyPresence,
        Self::TopologyHealth,
        Self::TopologyCapabilities,
        Self::TopologyNetworkMembership,
        Self::TopologyConnections,
        Self::ActivityActor,
        Self::ActivityAction,
        Self::ActivityResource,
        Self::ActivityOutcome,
        Self::ActivityOccurredAt,
        Self::ActivitySummary,
        Self::ActivityDetails,
        Self::FileId,
        Self::FileNetworkId,
        Self::FileDisplayName,
        Self::FileOwner,
        Self::FileMode,
        Self::FileRevision,
        Self::FileNetworkAcl,
        Self::FileLocalAcl,
        Self::FileQuota,
        Self::FilePathPolicy,
        Self::FileTimestamps,
        Self::FileAuditSettings,
    ];

    pub const fn object(self) -> PrivacyObjectKind {
        match self {
            Self::MemberNodeId
            | Self::MemberDisplayName
            | Self::MemberIdentityFingerprint
            | Self::MemberRole
            | Self::MemberGroups
            | Self::MemberCapabilities
            | Self::MemberStatus
            | Self::MemberLifecycleState
            | Self::MemberRevision
            | Self::MemberAccessEpoch
            | Self::MemberAvailableActions
            | Self::MemberPresence
            | Self::MemberJoinedAt => PrivacyObjectKind::Member,
            Self::NodeDisplayName
            | Self::NodePublicKey
            | Self::NodePlatformAndVersion
            | Self::NodeOnlineState
            | Self::NodeEndpoints
            | Self::NodeRealNetworkAddresses
            | Self::NodeTrafficStatistics
            | Self::NodeLocalProcesses
            | Self::NodeLocalPaths
            | Self::NodeDetailedLogs => PrivacyObjectKind::Node,
            Self::TopologyNodeId
            | Self::TopologyDisplayName
            | Self::TopologyRole
            | Self::TopologyStatus
            | Self::TopologyPresence
            | Self::TopologyHealth
            | Self::TopologyCapabilities
            | Self::TopologyNetworkMembership
            | Self::TopologyConnections => PrivacyObjectKind::Topology,
            Self::ActivityActor
            | Self::ActivityAction
            | Self::ActivityResource
            | Self::ActivityOutcome
            | Self::ActivityOccurredAt
            | Self::ActivitySummary
            | Self::ActivityDetails => PrivacyObjectKind::Activity,
            Self::FileId
            | Self::FileNetworkId
            | Self::FileDisplayName
            | Self::FileOwner
            | Self::FileMode
            | Self::FileRevision
            | Self::FileNetworkAcl
            | Self::FileLocalAcl
            | Self::FileQuota
            | Self::FilePathPolicy
            | Self::FileTimestamps
            | Self::FileAuditSettings => PrivacyObjectKind::FileMetadata,
        }
    }

    /// Conservative product defaults. These are floors, not client hints:
    /// policies can tighten them but can never reveal a field more broadly.
    pub const fn default_visibility(self) -> FieldVisibility {
        match self {
            Self::MemberDisplayName | Self::MemberPresence => FieldVisibility::Network,
            Self::MemberNodeId
            | Self::MemberRole
            | Self::MemberGroups
            | Self::MemberStatus
            | Self::MemberLifecycleState
            | Self::MemberRevision
            | Self::MemberAccessEpoch
            | Self::MemberAvailableActions => FieldVisibility::Network,
            Self::MemberIdentityFingerprint | Self::MemberCapabilities | Self::MemberJoinedAt => {
                FieldVisibility::Administrators
            }

            Self::NodeDisplayName
            | Self::NodePublicKey
            | Self::NodePlatformAndVersion
            | Self::NodeOnlineState
            | Self::NodeEndpoints => FieldVisibility::Network,
            Self::NodeRealNetworkAddresses | Self::NodeTrafficStatistics => {
                FieldVisibility::Administrators
            }
            Self::NodeLocalProcesses | Self::NodeLocalPaths | Self::NodeDetailedLogs => {
                FieldVisibility::OwnerOnly
            }

            Self::TopologyNodeId
            | Self::TopologyDisplayName
            | Self::TopologyRole
            | Self::TopologyStatus
            | Self::TopologyPresence
            | Self::TopologyHealth
            | Self::TopologyNetworkMembership
            | Self::TopologyConnections => FieldVisibility::Network,
            Self::TopologyCapabilities => FieldVisibility::Administrators,

            Self::ActivityActor
            | Self::ActivityAction
            | Self::ActivityResource
            | Self::ActivityOutcome
            | Self::ActivityOccurredAt
            | Self::ActivitySummary => FieldVisibility::Network,
            Self::ActivityDetails => FieldVisibility::Administrators,

            Self::FileId
            | Self::FileNetworkId
            | Self::FileDisplayName
            | Self::FileMode
            | Self::FileRevision
            | Self::FileTimestamps => FieldVisibility::Network,
            Self::FileOwner => FieldVisibility::Administrators,
            Self::FileNetworkAcl
            | Self::FileLocalAcl
            | Self::FileQuota
            | Self::FilePathPolicy
            | Self::FileAuditSettings => FieldVisibility::Administrators,
        }
    }
}

/// Authenticated viewer plus the network scope of the requested object.
///
/// `local_owner` describes the viewer's relation to the node producing this
/// response. The network principal describes only this network's role. Keeping
/// them separate prevents a node owner in one network from inheriting another
/// network's member or administrator visibility.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct PrivacyViewer {
    pub node_id: NodeId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_id: Option<NetworkId>,
    pub principal: PrivacyPrincipal,
    pub local_owner: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub membership_state: Option<MembershipState>,
}

impl PrivacyViewer {
    pub fn node_owner(node_id: NodeId, network_id: Option<NetworkId>) -> Self {
        Self {
            node_id,
            network_id,
            principal: PrivacyPrincipal::NodeOwner,
            local_owner: true,
            membership_state: None,
        }
    }

    /// A local owner who also has a verified role in this network.
    pub fn node_owner_in_network(
        node_id: NodeId,
        network_id: NetworkId,
        role: MembershipRole,
        membership_state: MembershipState,
    ) -> Self {
        let mut viewer = Self::network(node_id, network_id, role, membership_state);
        viewer.local_owner = true;
        viewer.principal = match role {
            MembershipRole::Owner => PrivacyPrincipal::NetworkOwner,
            MembershipRole::Administrator => PrivacyPrincipal::NetworkAdministrator,
            MembershipRole::Member => PrivacyPrincipal::NetworkMember,
        };
        viewer
    }

    pub fn network(
        node_id: NodeId,
        network_id: NetworkId,
        role: MembershipRole,
        membership_state: MembershipState,
    ) -> Self {
        let principal = match role {
            MembershipRole::Owner => PrivacyPrincipal::NetworkOwner,
            MembershipRole::Administrator => PrivacyPrincipal::NetworkAdministrator,
            MembershipRole::Member => PrivacyPrincipal::NetworkMember,
        };
        Self {
            node_id,
            network_id: Some(network_id),
            principal,
            local_owner: false,
            membership_state: Some(membership_state),
        }
    }

    pub fn public(node_id: NodeId, network_id: Option<NetworkId>) -> Self {
        Self {
            node_id,
            network_id,
            principal: PrivacyPrincipal::UnauthenticatedPublic,
            local_owner: false,
            membership_state: None,
        }
    }

    pub fn guest(node_id: NodeId, network_id: Option<NetworkId>) -> Self {
        Self {
            node_id,
            network_id,
            principal: PrivacyPrincipal::DiscoveryGuest,
            local_owner: false,
            membership_state: None,
        }
    }

    /// Scope an existing viewer to another network. A verified network role is
    /// never transplanted across networks; a mismatch becomes an unauthenticated
    /// viewer for that boundary.
    pub fn with_network(&self, network_id: NetworkId) -> Self {
        if self.network_id.as_ref() == Some(&network_id) || self.network_id.is_none() {
            let mut viewer = self.clone();
            viewer.network_id = Some(network_id);
            return viewer;
        }
        Self {
            node_id: self.node_id.clone(),
            network_id: Some(network_id),
            principal: PrivacyPrincipal::UnauthenticatedPublic,
            local_owner: false,
            membership_state: None,
        }
    }

    pub const fn has_active_network_role(&self) -> bool {
        self.principal.is_network_member()
            && matches!(
                self.membership_state,
                Some(MembershipState::Active | MembershipState::Degraded)
            )
    }

    pub fn can_view_network(&self, network_id: &NetworkId) -> bool {
        if self.network_id.as_ref() != Some(network_id) {
            return false;
        }
        match self.principal {
            PrivacyPrincipal::UnauthenticatedPublic | PrivacyPrincipal::DiscoveryGuest => true,
            principal => principal.is_network_member() && self.has_active_network_role(),
        }
    }

    pub fn can_view_object(
        &self,
        state: PrivacyObjectState,
        object_node_id: Option<&NodeId>,
    ) -> bool {
        if self.local_owner && object_node_id == Some(&self.node_id) {
            return true;
        }
        if state.is_publicly_listable()
            && matches!(
                self.principal,
                PrivacyPrincipal::UnauthenticatedPublic
                    | PrivacyPrincipal::DiscoveryGuest
                    | PrivacyPrincipal::NetworkMember
                    | PrivacyPrincipal::NetworkAdministrator
                    | PrivacyPrincipal::NetworkOwner
            )
        {
            return true;
        }
        if !self.has_active_network_role() {
            return false;
        }
        if self.principal.is_network_manager() {
            return true;
        }
        object_node_id == Some(&self.node_id)
    }
}

/// One explicit field override. A policy can only tighten the supplied floor.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct FieldVisibilityRule {
    pub field: PrivacyField,
    pub visibility: FieldVisibility,
}

impl FieldVisibilityRule {
    pub fn validate(&self) -> DomainResult<()> {
        if self.field.object() == PrivacyObjectKind::Node
            && matches!(
                self.field,
                PrivacyField::NodeLocalProcesses
                    | PrivacyField::NodeLocalPaths
                    | PrivacyField::NodeDetailedLogs
            )
            && self.visibility != FieldVisibility::OwnerOnly
        {
            return Err(DomainError::invariant(
                "network policy cannot widen a node owner-only field",
            ));
        }
        Ok(())
    }
}

/// Immutable network-authored field policy. Revisions are monotonic and carry
/// both issuance and effective time. A malformed, duplicate, expired or
/// not-yet-effective policy fails closed.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct FieldPrivacyPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_id: Option<NetworkId>,
    pub revision: u64,
    pub issued_at_unix_ms: u64,
    pub effective_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    #[serde(default)]
    pub rules: Vec<FieldVisibilityRule>,
}

impl FieldPrivacyPolicy {
    pub fn validate_at(&self, now_unix_ms: u64) -> DomainResult<()> {
        if self.revision == 0 {
            return Err(DomainError::invalid_value(
                "field privacy policy revision must be positive",
            ));
        }
        if self.issued_at_unix_ms > self.effective_at_unix_ms
            || self.effective_at_unix_ms >= self.expires_at_unix_ms
        {
            return Err(DomainError::invariant(
                "field privacy policy validity interval is invalid",
            ));
        }
        if now_unix_ms < self.effective_at_unix_ms {
            return Err(DomainError::new(
                "domain.field_privacy_not_effective",
                "field privacy policy is not yet effective",
            ));
        }
        if now_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::new(
                "domain.field_privacy_expired",
                "field privacy policy has expired",
            ));
        }
        let mut fields = BTreeSet::new();
        for rule in &self.rules {
            rule.validate()?;
            if !fields.insert(rule.field) {
                return Err(DomainError::invariant(
                    "field privacy policy contains duplicate field rules",
                ));
            }
        }
        Ok(())
    }

    pub fn validate_successor(
        &self,
        candidate: &FieldPrivacyPolicy,
        now_unix_ms: u64,
    ) -> DomainResult<()> {
        self.validate_at(now_unix_ms)?;
        candidate.validate_at(now_unix_ms)?;
        if self.network_id != candidate.network_id {
            return Err(DomainError::invariant(
                "field privacy policy belongs to another network",
            ));
        }
        if candidate.revision <= self.revision {
            return Err(DomainError::new(
                "domain.field_privacy_rollback",
                "field privacy policy revision must increase",
            ));
        }
        if candidate.effective_at_unix_ms < self.effective_at_unix_ms {
            return Err(DomainError::new(
                "domain.field_privacy_effective_time_rollback",
                "field privacy effective time must not move backwards",
            ));
        }
        Ok(())
    }

    /// Effective disclosure is an explicit rule, or the product default when no
    /// rule exists. Node-local process/path/log fields are validated separately
    /// and can only remain owner-only.
    pub fn effective_visibility(
        &self,
        field: PrivacyField,
        default_visibility: FieldVisibility,
    ) -> FieldVisibility {
        self.rules
            .iter()
            .find(|rule| rule.field == field)
            .map(|rule| rule.visibility)
            .unwrap_or(default_visibility)
    }

    pub fn decision(
        &self,
        field: PrivacyField,
        viewer: &PrivacyViewer,
        object_state: PrivacyObjectState,
        object_node_id: Option<&NodeId>,
        default_visibility: FieldVisibility,
        now_unix_ms: u64,
    ) -> FieldPrivacyDecision {
        if self.validate_at(now_unix_ms).is_err() {
            return FieldPrivacyDecision::denied(
                default_visibility,
                FieldPrivacyDenialCode::PolicyInvalid,
            );
        }
        if !viewer.can_view_object(object_state, object_node_id) {
            return FieldPrivacyDecision::denied(
                default_visibility,
                FieldPrivacyDenialCode::ObjectHidden,
            );
        }
        let visibility = self.effective_visibility(field, default_visibility);
        if visibility.allows(viewer) {
            FieldPrivacyDecision::allowed(visibility)
        } else {
            FieldPrivacyDecision::denied(visibility, FieldPrivacyDenialCode::FieldDenied)
        }
    }

    /// Compute the exact field set a Public viewer can observe. This result
    /// contains field identifiers only, never hidden values, and is intended
    /// for the administrator preview surface.
    pub fn preview_public(
        &self,
        object: PrivacyObjectKind,
        state: PrivacyObjectState,
        now_unix_ms: u64,
    ) -> DomainResult<PublicFieldPreview> {
        self.validate_at(now_unix_ms)?;
        let viewer = PrivacyViewer::public(
            NodeId::from_public_key(b"field-privacy-public-preview"),
            self.network_id.clone(),
        );
        let mut visible_fields = Vec::new();
        let mut hidden_fields = Vec::new();
        for field in PrivacyField::ALL
            .into_iter()
            .filter(|field| field.object() == object)
        {
            let visibility = self.effective_visibility(field, field.default_visibility());
            if viewer.can_view_object(state, None) && visibility.allows(&viewer) {
                visible_fields.push(field);
            } else {
                hidden_fields.push(field);
            }
        }
        Ok(PublicFieldPreview {
            policy_revision: self.revision,
            effective_at_unix_ms: self.effective_at_unix_ms,
            object,
            state,
            visible_fields,
            hidden_fields,
        })
    }

    pub fn change_record(
        &self,
        candidate: &FieldPrivacyPolicy,
        now_unix_ms: u64,
    ) -> DomainResult<FieldPrivacyChangeRecord> {
        self.validate_successor(candidate, now_unix_ms)?;
        let previous: BTreeMap<_, _> = self
            .rules
            .iter()
            .map(|rule| (rule.field, rule.visibility))
            .collect();
        let next: BTreeMap<_, _> = candidate
            .rules
            .iter()
            .map(|rule| (rule.field, rule.visibility))
            .collect();
        let fields: BTreeSet<_> = previous.keys().chain(next.keys()).copied().collect();
        let changes = fields
            .into_iter()
            .filter_map(|field| {
                let before = previous
                    .get(&field)
                    .copied()
                    .unwrap_or_else(|| field.default_visibility());
                let after = next
                    .get(&field)
                    .copied()
                    .unwrap_or_else(|| field.default_visibility());
                (before != after).then_some(FieldVisibilityChange {
                    field,
                    previous: before,
                    next: after,
                })
            })
            .collect();
        Ok(FieldPrivacyChangeRecord {
            previous_revision: self.revision,
            revision: candidate.revision,
            effective_at_unix_ms: candidate.effective_at_unix_ms,
            changed_fields: changes,
        })
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum FieldPrivacyDenialCode {
    PolicyInvalid,
    ObjectHidden,
    FieldDenied,
}

impl FieldPrivacyDenialCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PolicyInvalid => "permission.field_policy_invalid",
            Self::ObjectHidden => "permission.field_object_hidden",
            Self::FieldDenied => "permission.field_denied",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct FieldPrivacyDecision {
    pub allowed: bool,
    pub visibility: FieldVisibility,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denial_code: Option<FieldPrivacyDenialCode>,
}

impl FieldPrivacyDecision {
    pub const fn allowed(visibility: FieldVisibility) -> Self {
        Self {
            allowed: true,
            visibility,
            denial_code: None,
        }
    }

    pub const fn denied(visibility: FieldVisibility, code: FieldPrivacyDenialCode) -> Self {
        Self {
            allowed: false,
            visibility,
            denial_code: Some(code),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct PublicFieldPreview {
    pub policy_revision: u64,
    pub effective_at_unix_ms: u64,
    pub object: PrivacyObjectKind,
    pub state: PrivacyObjectState,
    pub visible_fields: Vec<PrivacyField>,
    pub hidden_fields: Vec<PrivacyField>,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct FieldVisibilityChange {
    pub field: PrivacyField,
    pub previous: FieldVisibility,
    pub next: FieldVisibility,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct FieldPrivacyChangeRecord {
    pub previous_revision: u64,
    pub revision: u64,
    pub effective_at_unix_ms: u64,
    pub changed_fields: Vec<FieldVisibilityChange>,
}

/// Resolve one active policy for a network. Duplicate network policies are
/// ambiguous and therefore fail closed.
pub fn resolve_field_privacy_policy<'a>(
    policies: &'a [FieldPrivacyPolicy],
    network_id: Option<&NetworkId>,
    now_unix_ms: u64,
) -> Result<Option<&'a FieldPrivacyPolicy>, ()> {
    let mut matching = policies
        .iter()
        .filter(|policy| policy.network_id.as_ref() == network_id);
    let first = matching.next();
    if matching.next().is_some() {
        return Err(());
    }
    match first {
        Some(policy) if policy.validate_at(now_unix_ms).is_ok() => Ok(Some(policy)),
        Some(_) => Err(()),
        None => Ok(None),
    }
}

/// Resolve one field decision without serializing either the policy or the
/// hidden value. Transport adapters use this before constructing their own
/// wire DTOs, so field policy remains a server-side serialization boundary.
pub fn field_is_visible(
    policy: Option<&FieldPrivacyPolicy>,
    field: PrivacyField,
    viewer: &PrivacyViewer,
    state: PrivacyObjectState,
    object_node_id: Option<&NodeId>,
    now_unix_ms: u64,
) -> bool {
    field_visible(
        policy,
        field,
        viewer,
        state,
        object_node_id,
        field.default_visibility(),
        now_unix_ms,
    )
}

fn field_visible(
    policy: Option<&FieldPrivacyPolicy>,
    field: PrivacyField,
    viewer: &PrivacyViewer,
    state: PrivacyObjectState,
    object_node_id: Option<&NodeId>,
    default_visibility: FieldVisibility,
    now_unix_ms: u64,
) -> bool {
    match policy {
        Some(policy) => {
            policy
                .decision(
                    field,
                    viewer,
                    state,
                    object_node_id,
                    default_visibility,
                    now_unix_ms,
                )
                .allowed
        }
        None => viewer.can_view_object(state, object_node_id) && default_visibility.allows(viewer),
    }
}

/// Project an authoritative member directory. Hidden optional values become
/// `None`, required governance fields either remain visible or the entire row
/// is removed. Sorting and count calculations run only on this projected value.
pub fn project_member_directory(
    directory: &NetworkMemberDirectory,
    viewer: &PrivacyViewer,
    policy: Option<&FieldPrivacyPolicy>,
    now_unix_ms: u64,
) -> NetworkMemberDirectory {
    let policy_valid = policy
        .map(|policy| policy.validate_at(now_unix_ms).is_ok())
        .unwrap_or(true);
    if !policy_valid || !viewer.can_view_network(&directory.network_id) {
        return NetworkMemberDirectory {
            network_id: directory.network_id.clone(),
            generated_at_unix_ms: directory.generated_at_unix_ms,
            viewer_role: directory.viewer_role,
            can_manage_members: false,
            members: Vec::new(),
        };
    }

    let mut members: Vec<NetworkMember> = directory
        .members
        .iter()
        .filter_map(|member| {
            let state = PrivacyObjectState::from(member.lifecycle_state);
            let required = [
                PrivacyField::MemberNodeId,
                PrivacyField::MemberRole,
                PrivacyField::MemberStatus,
                PrivacyField::MemberLifecycleState,
                PrivacyField::MemberRevision,
                PrivacyField::MemberAccessEpoch,
                PrivacyField::MemberPresence,
            ];
            if required.iter().any(|field| {
                !field_visible(
                    policy,
                    *field,
                    viewer,
                    state,
                    Some(&member.node_id),
                    field.default_visibility(),
                    now_unix_ms,
                )
            }) {
                return None;
            }
            let mut projected = member.clone();
            if !field_visible(
                policy,
                PrivacyField::MemberDisplayName,
                viewer,
                state,
                Some(&member.node_id),
                PrivacyField::MemberDisplayName.default_visibility(),
                now_unix_ms,
            ) {
                projected.display_name = None;
            }
            if !field_visible(
                policy,
                PrivacyField::MemberIdentityFingerprint,
                viewer,
                state,
                Some(&member.node_id),
                PrivacyField::MemberIdentityFingerprint.default_visibility(),
                now_unix_ms,
            ) {
                projected.identity_fingerprint = None;
            }
            if !field_visible(
                policy,
                PrivacyField::MemberCapabilities,
                viewer,
                state,
                Some(&member.node_id),
                PrivacyField::MemberCapabilities.default_visibility(),
                now_unix_ms,
            ) {
                projected.capabilities = None;
            }
            if !field_visible(
                policy,
                PrivacyField::MemberPresence,
                viewer,
                state,
                Some(&member.node_id),
                PrivacyField::MemberPresence.default_visibility(),
                now_unix_ms,
            ) {
                return None;
            }
            if !field_visible(
                policy,
                PrivacyField::MemberGroups,
                viewer,
                state,
                Some(&member.node_id),
                PrivacyField::MemberGroups.default_visibility(),
                now_unix_ms,
            ) {
                projected.groups.clear();
            }
            if !field_visible(
                policy,
                PrivacyField::MemberAvailableActions,
                viewer,
                state,
                Some(&member.node_id),
                PrivacyField::MemberAvailableActions.default_visibility(),
                now_unix_ms,
            ) {
                projected.available_actions.clear();
            }
            if !field_visible(
                policy,
                PrivacyField::MemberJoinedAt,
                viewer,
                state,
                Some(&member.node_id),
                PrivacyField::MemberJoinedAt.default_visibility(),
                now_unix_ms,
            ) {
                projected.joined_at_unix_ms = None;
            }
            Some(projected)
        })
        .collect();
    members.sort_by(|left, right| {
        left.display_name
            .as_deref()
            .unwrap_or("")
            .cmp(right.display_name.as_deref().unwrap_or(""))
            .then_with(|| left.node_id.cmp(&right.node_id))
    });
    NetworkMemberDirectory {
        network_id: directory.network_id.clone(),
        generated_at_unix_ms: directory.generated_at_unix_ms,
        viewer_role: directory.viewer_role,
        can_manage_members: directory.can_manage_members && viewer.principal.is_network_manager(),
        members,
    }
}

/// Project an already built topology snapshot. This second projection is
/// required because the canonical snapshot is shared across sessions; never
/// mutate it or rely on the UI to hide values.
pub fn project_topology_snapshot(
    snapshot: &TopologySnapshot,
    viewer: &PrivacyViewer,
    policy: Option<&FieldPrivacyPolicy>,
    now_unix_ms: u64,
) -> TopologySnapshot {
    let policy_valid = policy
        .map(|policy| policy.validate_at(now_unix_ms).is_ok())
        .unwrap_or(true);
    if !policy_valid {
        return TopologySnapshot {
            boundaries: Vec::new(),
            nodes: Vec::new(),
            connections: Vec::new(),
            clusters: Vec::new(),
            truncated: false,
            ..snapshot.clone()
        };
    }
    let state = PrivacyObjectState::Active;
    let mut kept_networks = BTreeSet::new();
    for boundary in &snapshot.boundaries {
        let network_viewer = viewer.with_network(boundary.network_id.clone());
        if network_viewer.can_view_network(&boundary.network_id)
            && field_visible(
                policy,
                PrivacyField::TopologyNetworkMembership,
                &network_viewer,
                state,
                None,
                PrivacyField::TopologyNetworkMembership.default_visibility(),
                now_unix_ms,
            )
        {
            kept_networks.insert(boundary.network_id.clone());
        }
    }

    let mut projected_nodes = Vec::new();
    for node in &snapshot.nodes {
        let mut networks = Vec::new();
        for network in &node.networks {
            if !kept_networks.contains(&network.network_id) {
                continue;
            }
            let network_viewer = viewer.with_network(network.network_id.clone());
            let object_state = match network.status {
                NetworkMemberStatus::Pending => PrivacyObjectState::PendingApproval,
                NetworkMemberStatus::Approved => PrivacyObjectState::Approved,
                NetworkMemberStatus::Active => PrivacyObjectState::Active,
                NetworkMemberStatus::Degraded => PrivacyObjectState::Degraded,
                NetworkMemberStatus::Suspended => PrivacyObjectState::Suspended,
                NetworkMemberStatus::Exited => PrivacyObjectState::Exited,
                NetworkMemberStatus::Removed => PrivacyObjectState::Removed,
                NetworkMemberStatus::Expired => PrivacyObjectState::Expired,
            };
            if !network_viewer.can_view_object(object_state, Some(&node.node_id)) {
                continue;
            }
            let required_fields = [
                PrivacyField::TopologyNodeId,
                PrivacyField::TopologyRole,
                PrivacyField::TopologyStatus,
                PrivacyField::TopologyPresence,
                PrivacyField::TopologyNetworkMembership,
            ];
            if required_fields.iter().any(|field| {
                !field_visible(
                    policy,
                    *field,
                    &network_viewer,
                    object_state,
                    Some(&node.node_id),
                    field.default_visibility(),
                    now_unix_ms,
                )
            }) {
                continue;
            }
            let mut projected = network.clone();
            if !field_visible(
                policy,
                PrivacyField::TopologyCapabilities,
                &network_viewer,
                object_state,
                Some(&node.node_id),
                PrivacyField::TopologyCapabilities.default_visibility(),
                now_unix_ms,
            ) {
                projected.capabilities.clear();
            }
            if !field_visible(
                policy,
                PrivacyField::TopologyHealth,
                &network_viewer,
                object_state,
                Some(&node.node_id),
                PrivacyField::TopologyHealth.default_visibility(),
                now_unix_ms,
            ) {
                projected.health = super::topology::TopologyNodeHealth::Unknown;
            }
            networks.push(projected);
        }
        if networks.is_empty() {
            continue;
        }
        let first_viewer = viewer.with_network(networks[0].network_id.clone());
        let mut projected = node.clone();
        projected.networks = networks;
        projected.capabilities.clear();
        for network in &projected.networks {
            projected
                .capabilities
                .extend(network.capabilities.iter().copied());
        }
        projected.capabilities.sort();
        projected.capabilities.dedup();
        projected.health = projected
            .networks
            .iter()
            .map(|network| network.health)
            .max_by_key(|health| health.severity())
            .unwrap_or(super::topology::TopologyNodeHealth::Unknown);
        if !field_visible(
            policy,
            PrivacyField::TopologyDisplayName,
            &first_viewer,
            state,
            Some(&node.node_id),
            PrivacyField::TopologyDisplayName.default_visibility(),
            now_unix_ms,
        ) {
            projected.display_name = None;
        }
        projected_nodes.push(projected);
    }
    let kept_node_ids: BTreeSet<_> = projected_nodes.iter().map(|node| &node.node_id).collect();

    let mut boundaries = Vec::new();
    for boundary in &snapshot.boundaries {
        if !kept_networks.contains(&boundary.network_id) {
            continue;
        }
        let network_viewer = viewer.with_network(boundary.network_id.clone());
        let node_networks: Vec<&TopologyNodeNetwork> = projected_nodes
            .iter()
            .flat_map(|node| node.networks.iter())
            .filter(|network| network.network_id == boundary.network_id)
            .collect();
        let member_count = node_networks.len() as u32;
        let capabilities = node_networks
            .iter()
            .flat_map(|network| network.capabilities.iter().copied())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let viewer_role = if field_visible(
            policy,
            PrivacyField::TopologyRole,
            &network_viewer,
            state,
            None,
            PrivacyField::TopologyRole.default_visibility(),
            now_unix_ms,
        ) {
            boundary.viewer_role
        } else {
            None
        };
        boundaries.push(super::topology::TopologyBoundary {
            network_id: boundary.network_id.clone(),
            name: boundary.name.clone(),
            member_count,
            visible_member_count: member_count,
            viewer_role,
            capabilities,
        });
    }

    let connections = if field_visible(
        policy,
        PrivacyField::TopologyConnections,
        viewer,
        state,
        None,
        PrivacyField::TopologyConnections.default_visibility(),
        now_unix_ms,
    ) {
        snapshot
            .connections
            .iter()
            .filter(|connection| {
                kept_node_ids.contains(&connection.from)
                    && kept_node_ids.contains(&connection.to)
                    && connection
                        .networks
                        .iter()
                        .any(|network| kept_networks.contains(network))
            })
            .cloned()
            .map(|mut connection| {
                connection
                    .networks
                    .retain(|network| kept_networks.contains(network));
                connection
            })
            .collect()
    } else {
        Vec::new()
    };

    // Aggregation counts and samples are themselves hidden-member metadata.
    // Only a verified network manager may receive them, and only for a network
    // that survived the same projection.
    let clusters = if viewer.has_active_network_role() && viewer.principal.is_network_manager() {
        snapshot
            .clusters
            .iter()
            .filter(|cluster| kept_networks.contains(&cluster.network_id))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };

    TopologySnapshot {
        revision: snapshot.revision,
        generated_at_unix_ms: snapshot.generated_at_unix_ms,
        viewer: snapshot.viewer.clone(),
        truncated: false,
        boundaries,
        nodes: projected_nodes,
        connections,
        clusters,
    }
}

/// Filter topology connections before a query/export. Kept public for callers
/// that construct a topology snapshot incrementally.
pub fn filter_topology_connections(
    connections: Vec<TopologyConnection>,
    visible_networks: &BTreeSet<NetworkId>,
) -> Vec<TopologyConnection> {
    connections
        .into_iter()
        .filter_map(|mut connection| {
            connection
                .networks
                .retain(|network| visible_networks.contains(network));
            (!connection.networks.is_empty()).then_some(connection)
        })
        .collect()
}

/// Activity projection. Required identity/timeline fields remove the whole
/// entry when hidden; optional prose fields are omitted from JSON entirely.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ProjectedActivityEntry {
    pub audit_id: AuditId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<OperationId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_id: Option<NetworkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<NodeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<AuditOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurred_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<BTreeMap<String, String>>,
}

pub fn project_activity_entry(
    activity: &ActivityEntry,
    viewer: &PrivacyViewer,
    policy: Option<&FieldPrivacyPolicy>,
    now_unix_ms: u64,
) -> Option<ProjectedActivityEntry> {
    if policy.is_some_and(|policy| policy.validate_at(now_unix_ms).is_err()) {
        return None;
    }
    let network_visible = match activity.network_id.as_ref() {
        Some(network_id) => viewer.can_view_network(network_id),
        None => viewer.local_owner && activity.actor == viewer.node_id,
    };
    if !network_visible {
        return None;
    }
    let state = PrivacyObjectState::Active;
    let visible = |field| {
        field_visible(
            policy,
            field,
            viewer,
            state,
            Some(&activity.actor),
            field.default_visibility(),
            now_unix_ms,
        )
    };
    let required = [
        PrivacyField::ActivityActor,
        PrivacyField::ActivityAction,
        PrivacyField::ActivityResource,
        PrivacyField::ActivityOutcome,
        PrivacyField::ActivityOccurredAt,
    ];
    if required.iter().any(|field| !visible(*field)) {
        return None;
    }
    Some(ProjectedActivityEntry {
        audit_id: activity.audit_id.clone(),
        operation_id: activity.operation_id.clone(),
        network_id: activity.network_id.clone(),
        actor: Some(activity.actor.clone()),
        action: Some(activity.action.clone()),
        resource: Some(activity.resource.clone()),
        outcome: Some(activity.outcome),
        occurred_at_unix_ms: Some(activity.occurred_at_unix_ms),
        summary: visible(PrivacyField::ActivitySummary).then(|| activity.summary.clone()),
        details: visible(PrivacyField::ActivityDetails).then(|| activity.details.clone()),
    })
}

/// File metadata is deliberately a separate DTO. The canonical `FileShare`
/// remains available to authorization code but is never serialized directly to
/// a viewer without this field projection.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ProjectedFileShareMetadata {
    pub schema_version: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<FileShareId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_id: Option<NetworkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<NodeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<FileShareMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_acl: Option<FileAcl>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_acl: Option<FileAcl>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota: Option<FileQuota>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symlink_policy: Option<SymlinkPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrent_name_policy: Option<ConcurrentNamePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete_policy: Option<FileDeletePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at_unix_ms: Option<u64>,
}

pub fn project_file_share_metadata(
    share: &FileShare,
    viewer: &PrivacyViewer,
    policy: Option<&FieldPrivacyPolicy>,
    now_unix_ms: u64,
) -> ProjectedFileShareMetadata {
    let empty = ProjectedFileShareMetadata {
        schema_version: share.schema_version,
        id: None,
        network_id: None,
        owner: None,
        display_name: None,
        mode: None,
        revision: None,
        network_acl: None,
        local_acl: None,
        quota: None,
        symlink_policy: None,
        concurrent_name_policy: None,
        delete_policy: None,
        audit_enabled: None,
        created_at_unix_ms: None,
        updated_at_unix_ms: None,
    };
    if policy.is_some_and(|policy| policy.validate_at(now_unix_ms).is_err())
        || !viewer.can_view_network(&share.network_id)
        || !viewer.can_view_object(PrivacyObjectState::Active, Some(&share.owner))
    {
        return empty;
    }
    let visible = |field| {
        field_visible(
            policy,
            field,
            viewer,
            PrivacyObjectState::Active,
            Some(&share.owner),
            field.default_visibility(),
            now_unix_ms,
        )
    };
    ProjectedFileShareMetadata {
        schema_version: share.schema_version,
        id: visible(PrivacyField::FileId).then(|| share.id.clone()),
        network_id: visible(PrivacyField::FileNetworkId).then(|| share.network_id.clone()),
        owner: visible(PrivacyField::FileOwner).then(|| share.owner.clone()),
        display_name: visible(PrivacyField::FileDisplayName).then(|| share.display_name.clone()),
        mode: visible(PrivacyField::FileMode).then_some(share.mode),
        revision: visible(PrivacyField::FileRevision).then_some(share.revision),
        network_acl: visible(PrivacyField::FileNetworkAcl).then(|| share.network_acl.clone()),
        local_acl: visible(PrivacyField::FileLocalAcl).then(|| share.local_acl.clone()),
        quota: visible(PrivacyField::FileQuota).then(|| share.quota.clone()),
        symlink_policy: visible(PrivacyField::FilePathPolicy).then_some(share.symlink_policy),
        concurrent_name_policy: visible(PrivacyField::FilePathPolicy)
            .then_some(share.concurrent_name_policy),
        delete_policy: visible(PrivacyField::FilePathPolicy).then_some(share.delete_policy),
        audit_enabled: visible(PrivacyField::FileAuditSettings).then_some(share.audit_enabled),
        created_at_unix_ms: visible(PrivacyField::FileTimestamps)
            .then_some(share.created_at_unix_ms),
        updated_at_unix_ms: visible(PrivacyField::FileTimestamps)
            .then_some(share.updated_at_unix_ms),
    }
}

/// Determine whether a member status can be listed for a viewer. Used by
/// search/export adapters before they construct any query result.
pub fn member_status_visible(status: NetworkMemberStatus, viewer: &PrivacyViewer) -> bool {
    let state = match status {
        NetworkMemberStatus::Pending => PrivacyObjectState::PendingApproval,
        NetworkMemberStatus::Approved => PrivacyObjectState::Approved,
        NetworkMemberStatus::Active => PrivacyObjectState::Active,
        NetworkMemberStatus::Degraded => PrivacyObjectState::Degraded,
        NetworkMemberStatus::Suspended => PrivacyObjectState::Suspended,
        NetworkMemberStatus::Exited => PrivacyObjectState::Exited,
        NetworkMemberStatus::Removed => PrivacyObjectState::Removed,
        NetworkMemberStatus::Expired => PrivacyObjectState::Expired,
    };
    viewer.can_view_object(state, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        CapabilityKind, MemberLifecycleAction, NetworkMemberGroup, NetworkMemberOnlineState,
        NetworkMemberPresence, TopologyNode,
    };

    fn network_id() -> NetworkId {
        NetworkId::from_public_key(b"privacy-network")
    }

    fn node_id(label: &[u8]) -> NodeId {
        NodeId::from_public_key(label)
    }

    fn policy(
        rules: impl IntoIterator<Item = (PrivacyField, FieldVisibility)>,
    ) -> FieldPrivacyPolicy {
        FieldPrivacyPolicy {
            network_id: Some(network_id()),
            revision: 2,
            issued_at_unix_ms: 1_000,
            effective_at_unix_ms: 1_000,
            expires_at_unix_ms: 10_000,
            rules: rules
                .into_iter()
                .map(|(field, visibility)| FieldVisibilityRule { field, visibility })
                .collect(),
        }
    }

    fn member(lifecycle_state: MemberLifecycleStatus, name: &str) -> NetworkMember {
        NetworkMember {
            node_id: node_id(name.as_bytes()),
            display_name: Some(name.into()),
            identity_fingerprint: Some(format!("fingerprint-{name}")),
            role: MembershipRole::Member,
            groups: vec![NetworkMemberGroup::Member],
            capabilities: Some(vec![CapabilityKind::Relay]),
            status: lifecycle_state.to_member_status(NetworkMemberPresence::live(1_100)),
            lifecycle_state,
            revision: 3,
            access_epoch: 4,
            available_actions: vec![MemberLifecycleAction::Exit],
            presence: NetworkMemberPresence {
                state: NetworkMemberOnlineState::Online,
                source: super::super::membership::NetworkMemberObservationSource::LiveSession,
                observed_at_unix_ms: Some(1_100),
            },
            joined_at_unix_ms: Some(1_050),
        }
    }

    fn directory(members: Vec<NetworkMember>) -> NetworkMemberDirectory {
        NetworkMemberDirectory {
            network_id: network_id(),
            generated_at_unix_ms: 1_200,
            viewer_role: MembershipRole::Member,
            can_manage_members: false,
            members,
        }
    }

    fn activity() -> ActivityEntry {
        ActivityEntry {
            audit_id: AuditId::from_public_key(b"activity"),
            operation_id: Some(OperationId::from_public_key(b"operation")),
            actor: node_id(b"actor"),
            network_id: Some(network_id()),
            action: "file.share_update".into(),
            resource: "file/share".into(),
            outcome: AuditOutcome::Succeeded,
            occurred_at_unix_ms: 1_100,
            summary: "private activity summary".into(),
            details: BTreeMap::from([("secret_detail".into(), "private details".into())]),
        }
    }

    fn file_share() -> FileShare {
        FileShare {
            schema_version: super::super::file_share::FILE_SHARE_SCHEMA_VERSION,
            id: FileShareId::from_public_key(b"share"),
            network_id: network_id(),
            owner: node_id(b"owner"),
            display_name: "private file share".into(),
            mode: FileShareMode::ReadWrite,
            revision: 7,
            network_acl: FileAcl::default(),
            local_acl: FileAcl::default(),
            quota: FileQuota::default(),
            symlink_policy: SymlinkPolicy::Reject,
            concurrent_name_policy: ConcurrentNamePolicy::IfUnchanged,
            delete_policy: FileDeletePolicy::default(),
            audit_enabled: true,
            created_at_unix_ms: 1_000,
            updated_at_unix_ms: 1_100,
        }
    }

    fn viewer(role: MembershipRole) -> PrivacyViewer {
        PrivacyViewer::network(
            node_id(b"viewer"),
            network_id(),
            role,
            MembershipState::Active,
        )
    }

    #[test]
    fn legacy_visibility_names_deserialize_as_four_levels() {
        assert_eq!(
            serde_json::from_str::<FieldVisibility>("\"network_members\"").unwrap(),
            FieldVisibility::Network
        );
        assert_eq!(
            serde_json::from_str::<FieldVisibility>("\"network_admins\"").unwrap(),
            FieldVisibility::Administrators
        );
        assert_eq!(
            serde_json::from_str::<FieldVisibility>("\"node_owner\"").unwrap(),
            FieldVisibility::OwnerOnly
        );
    }

    #[test]
    fn explicit_rules_override_defaults_owner_only_remains_locked() {
        let policy = policy([
            (PrivacyField::MemberDisplayName, FieldVisibility::Public),
            (
                PrivacyField::MemberIdentityFingerprint,
                FieldVisibility::Administrators,
            ),
        ]);
        assert_eq!(
            policy.effective_visibility(PrivacyField::MemberDisplayName, FieldVisibility::Network),
            FieldVisibility::Public
        );
        assert_eq!(
            policy.effective_visibility(PrivacyField::NodeEndpoints, FieldVisibility::Network),
            FieldVisibility::Network
        );
        assert_eq!(
            policy.effective_visibility(
                PrivacyField::MemberIdentityFingerprint,
                FieldVisibility::Network
            ),
            FieldVisibility::Administrators
        );
    }

    #[test]
    fn owner_only_node_fields_cannot_be_widened() {
        let invalid = policy([(
            PrivacyField::NodeLocalPaths,
            FieldVisibility::Administrators,
        )]);
        assert_eq!(
            invalid.validate_at(1_100).unwrap_err().code(),
            "domain.invariant_violation"
        );
    }

    #[test]
    fn malformed_expired_and_duplicate_policies_fail_closed() {
        let mut expired = policy([]);
        expired.expires_at_unix_ms = 1_000;
        assert!(expired.validate_at(1_000).is_err());

        let mut duplicate = policy([]);
        duplicate.rules = vec![
            FieldVisibilityRule {
                field: PrivacyField::MemberDisplayName,
                visibility: FieldVisibility::Network,
            },
            FieldVisibilityRule {
                field: PrivacyField::MemberDisplayName,
                visibility: FieldVisibility::Administrators,
            },
        ];
        assert!(duplicate.validate_at(1_100).is_err());

        let mut docs = vec![policy([]), policy([])];
        docs[1].revision = 3;
        assert_eq!(
            resolve_field_privacy_policy(&docs, Some(&network_id()), 1_100),
            Err(())
        );
    }

    #[test]
    fn revision_and_effective_time_are_monotonic() {
        let base = policy([]);
        let mut candidate = base.clone();
        candidate.revision = 3;
        candidate.effective_at_unix_ms = 2_000;
        candidate.expires_at_unix_ms = 20_000;
        base.validate_successor(&candidate, 2_500).unwrap();
        let record = base.change_record(&candidate, 2_500).unwrap();
        assert_eq!(record.previous_revision, 2);
        assert_eq!(record.revision, 3);

        candidate.effective_at_unix_ms = 900;
        assert_eq!(
            base.validate_successor(&candidate, 1_500)
                .unwrap_err()
                .code(),
            "domain.invariant_violation"
        );
    }

    #[test]
    fn member_projection_omits_hidden_values_before_serialization() {
        let policy = policy([
            (
                PrivacyField::MemberDisplayName,
                FieldVisibility::Administrators,
            ),
            (
                PrivacyField::MemberIdentityFingerprint,
                FieldVisibility::OwnerOnly,
            ),
            (PrivacyField::MemberCapabilities, FieldVisibility::OwnerOnly),
            (
                PrivacyField::MemberJoinedAt,
                FieldVisibility::Administrators,
            ),
        ]);
        let projected = project_member_directory(
            &directory(vec![member(MemberLifecycleStatus::Active, "alice-secret")]),
            &viewer(MembershipRole::Member),
            Some(&policy),
            1_500,
        );
        assert_eq!(projected.members.len(), 1);
        let json = serde_json::to_value(&projected).unwrap();
        assert!(json["members"][0].get("display_name").is_none());
        assert!(json["members"][0].get("identity_fingerprint").is_none());
        assert!(json["members"][0].get("capabilities").is_none());
        assert!(json["members"][0].get("joined_at_unix_ms").is_none());
        assert!(!json.to_string().contains("alice-secret"));
    }

    #[test]
    fn hidden_members_do_not_contribute_to_count_or_search() {
        let projected = project_member_directory(
            &directory(vec![
                member(MemberLifecycleStatus::Active, "visible"),
                member(MemberLifecycleStatus::PendingApproval, "pending-secret"),
            ]),
            &viewer(MembershipRole::Member),
            None,
            1_500,
        );
        assert_eq!(projected.members.len(), 1);
        assert!(!serde_json::to_string(&projected)
            .unwrap()
            .contains("pending-secret"));
    }

    #[test]
    fn topology_projection_recomputes_counts_and_search_scope() {
        let nodes = vec![
            TopologyNode {
                node_id: node_id(b"visible"),
                display_name: Some("visible".into()),
                is_self: false,
                health: super::super::topology::TopologyNodeHealth::Healthy,
                capabilities: vec![CapabilityKind::Relay],
                networks: vec![TopologyNodeNetwork {
                    network_id: network_id(),
                    role: MembershipRole::Member,
                    status: NetworkMemberStatus::Active,
                    presence: NetworkMemberPresence::live(1_100),
                    health: super::super::topology::TopologyNodeHealth::Healthy,
                    capabilities: vec![CapabilityKind::Relay],
                }],
            },
            TopologyNode {
                node_id: node_id(b"hidden"),
                display_name: Some("hidden-secret".into()),
                is_self: false,
                health: super::super::topology::TopologyNodeHealth::Healthy,
                capabilities: vec![],
                networks: vec![TopologyNodeNetwork {
                    network_id: network_id(),
                    role: MembershipRole::Member,
                    status: NetworkMemberStatus::Suspended,
                    presence: NetworkMemberPresence::unknown(),
                    health: super::super::topology::TopologyNodeHealth::Unreachable,
                    capabilities: vec![],
                }],
            },
        ];
        let snapshot = TopologySnapshot {
            revision: 1,
            generated_at_unix_ms: 1_200,
            viewer: node_id(b"viewer"),
            truncated: false,
            boundaries: vec![super::super::topology::TopologyBoundary {
                network_id: network_id(),
                name: "network".into(),
                member_count: 2,
                visible_member_count: 2,
                viewer_role: Some(MembershipRole::Member),
                capabilities: vec![CapabilityKind::Relay],
            }],
            nodes,
            connections: vec![],
            clusters: vec![],
        };
        let policy = policy([(
            PrivacyField::TopologyDisplayName,
            FieldVisibility::Administrators,
        )]);
        let projected = project_topology_snapshot(
            &snapshot,
            &viewer(MembershipRole::Member),
            Some(&policy),
            1_500,
        );
        assert_eq!(projected.boundaries[0].member_count, 1);
        assert_eq!(projected.boundaries[0].visible_member_count, 1);
        assert!(projected.nodes[0].display_name.is_none());
        assert!(projected
            .query(&super::super::topology::TopologyQuery::default().search("hidden-secret"))
            .is_empty());
    }

    #[test]
    fn public_preview_matches_actual_public_member_projection() {
        let policy = policy([
            (PrivacyField::MemberNodeId, FieldVisibility::Public),
            (PrivacyField::MemberRole, FieldVisibility::Public),
            (PrivacyField::MemberStatus, FieldVisibility::Public),
            (PrivacyField::MemberLifecycleState, FieldVisibility::Public),
            (PrivacyField::MemberRevision, FieldVisibility::Public),
            (PrivacyField::MemberAccessEpoch, FieldVisibility::Public),
            (PrivacyField::MemberPresence, FieldVisibility::Public),
            (PrivacyField::MemberDisplayName, FieldVisibility::Public),
        ]);
        let preview = policy
            .preview_public(PrivacyObjectKind::Member, PrivacyObjectState::Active, 1_500)
            .unwrap();
        let public_viewer = PrivacyViewer::public(node_id(b"public"), Some(network_id()));
        assert!(public_viewer.can_view_network(&network_id()));
        let projected = project_member_directory(
            &directory(vec![member(MemberLifecycleStatus::Active, "public")]),
            &public_viewer,
            Some(&policy),
            1_500,
        );
        assert_eq!(projected.members.len(), 1);
        assert!(preview
            .visible_fields
            .contains(&PrivacyField::MemberDisplayName));
        assert!(preview
            .hidden_fields
            .contains(&PrivacyField::MemberIdentityFingerprint));
    }

    #[test]
    fn activity_projection_omits_hidden_text_and_drops_hidden_required_rows() {
        let activity_policy = policy([
            (
                PrivacyField::ActivitySummary,
                FieldVisibility::Administrators,
            ),
            (PrivacyField::ActivityDetails, FieldVisibility::OwnerOnly),
        ]);
        let projected = project_activity_entry(
            &activity(),
            &viewer(MembershipRole::Member),
            Some(&activity_policy),
            1_500,
        )
        .unwrap();
        let json = serde_json::to_value(projected).unwrap();
        assert!(json.get("summary").is_none());
        assert!(json.get("details").is_none());
        assert!(!json.to_string().contains("private activity summary"));
        assert!(!json.to_string().contains("private details"));

        let hidden_actor = policy([(PrivacyField::ActivityActor, FieldVisibility::OwnerOnly)]);
        assert!(project_activity_entry(
            &activity(),
            &viewer(MembershipRole::Administrator),
            Some(&hidden_actor),
            1_500,
        )
        .is_none());
    }

    #[test]
    fn file_metadata_projection_omits_hidden_acl_quota_paths_and_timestamps() {
        let file_policy = policy([
            (PrivacyField::FileOwner, FieldVisibility::Administrators),
            (
                PrivacyField::FileNetworkAcl,
                FieldVisibility::Administrators,
            ),
            (PrivacyField::FileLocalAcl, FieldVisibility::Administrators),
            (PrivacyField::FileQuota, FieldVisibility::Administrators),
            (
                PrivacyField::FilePathPolicy,
                FieldVisibility::Administrators,
            ),
            (
                PrivacyField::FileTimestamps,
                FieldVisibility::Administrators,
            ),
            (
                PrivacyField::FileAuditSettings,
                FieldVisibility::Administrators,
            ),
        ]);
        let projected = project_file_share_metadata(
            &file_share(),
            &viewer(MembershipRole::Member),
            Some(&file_policy),
            1_500,
        );
        let json = serde_json::to_value(projected).unwrap();
        for key in [
            "owner",
            "network_acl",
            "local_acl",
            "quota",
            "symlink_policy",
            "concurrent_name_policy",
            "delete_policy",
            "audit_enabled",
            "created_at_unix_ms",
            "updated_at_unix_ms",
        ] {
            assert!(json.get(key).is_none(), "hidden key leaked: {key}");
        }
        assert_eq!(json["display_name"], "private file share");
        assert_eq!(json["revision"], 7);
    }

    #[test]
    fn role_field_object_state_matrix_is_fail_closed() {
        let states = PrivacyObjectState::ALL;
        let viewers = [
            (
                "public",
                PrivacyViewer::public(node_id(b"public"), Some(network_id())),
            ),
            (
                "guest",
                PrivacyViewer::guest(node_id(b"guest"), Some(network_id())),
            ),
            ("member", viewer(MembershipRole::Member)),
            ("administrator", viewer(MembershipRole::Administrator)),
            ("network_owner", viewer(MembershipRole::Owner)),
            (
                "node_owner",
                PrivacyViewer::node_owner(node_id(b"owner"), Some(network_id())),
            ),
        ];
        let rules = PrivacyField::ALL
            .into_iter()
            .enumerate()
            .map(|(index, field)| {
                let visibility = if matches!(
                    field,
                    PrivacyField::NodeLocalProcesses
                        | PrivacyField::NodeLocalPaths
                        | PrivacyField::NodeDetailedLogs
                ) {
                    FieldVisibility::OwnerOnly
                } else {
                    [
                        FieldVisibility::Public,
                        FieldVisibility::Network,
                        FieldVisibility::Administrators,
                        FieldVisibility::OwnerOnly,
                    ][index % 4]
                };
                (field, visibility)
            });
        let policy = policy(rules);
        policy.validate_at(1_500).unwrap();

        for state in states {
            for (viewer_name, viewer) in &viewers {
                let object_node_id = Some(&viewer.node_id);
                let object_visible = if viewer.local_owner {
                    viewer.can_view_object(state, object_node_id)
                } else if matches!(
                    viewer.principal,
                    PrivacyPrincipal::UnauthenticatedPublic | PrivacyPrincipal::DiscoveryGuest
                ) {
                    state.is_publicly_listable()
                } else if viewer.principal.is_network_manager() {
                    viewer.has_active_network_role()
                } else {
                    viewer.has_active_network_role()
                        && (state.is_publicly_listable() || object_node_id == Some(&viewer.node_id))
                };
                assert_eq!(
                    viewer.can_view_object(state, object_node_id),
                    object_visible,
                    "{viewer_name} / {state:?}"
                );

                for field in PrivacyField::ALL {
                    let visibility = policy.effective_visibility(field, field.default_visibility());
                    let audience_visible = match visibility {
                        FieldVisibility::Public => true,
                        FieldVisibility::Network => viewer.has_active_network_role(),
                        FieldVisibility::Administrators => {
                            viewer.has_active_network_role() && viewer.principal.is_administrator()
                        }
                        FieldVisibility::OwnerOnly => viewer.local_owner,
                    };
                    let decision = policy.decision(
                        field,
                        viewer,
                        state,
                        object_node_id,
                        field.default_visibility(),
                        1_500,
                    );
                    assert_eq!(
                        decision.allowed,
                        object_visible && audience_visible,
                        "{viewer_name} / {state:?} / {field:?} / {visibility:?}"
                    );
                    if !decision.allowed {
                        assert!(decision.denial_code.is_some());
                    }
                }
            }
        }
    }

    #[test]
    fn cross_network_roles_and_node_ownership_do_not_imply_each_other() {
        let other_network = NetworkId::from_public_key(b"other-network");
        let admin = viewer(MembershipRole::Administrator);
        let scoped_elsewhere = admin.with_network(other_network.clone());
        assert!(scoped_elsewhere.can_view_network(&other_network));
        assert_eq!(
            scoped_elsewhere.principal,
            PrivacyPrincipal::UnauthenticatedPublic
        );
        assert!(!scoped_elsewhere
            .can_view_object(PrivacyObjectState::Suspended, Some(&node_id(b"remote"))));
        assert!(!FieldVisibility::Network.allows(&scoped_elsewhere));
        assert!(!FieldVisibility::Administrators.allows(&scoped_elsewhere));

        let owner = PrivacyViewer::node_owner(node_id(b"owner"), Some(network_id()));
        assert!(owner.can_view_object(PrivacyObjectState::Active, Some(&owner.node_id)));
        assert!(!owner.can_view_object(PrivacyObjectState::Active, Some(&node_id(b"remote"))));
        assert!(!FieldVisibility::Network.allows(&owner));
        assert!(!FieldVisibility::Administrators.allows(&owner));
        assert!(FieldVisibility::OwnerOnly.allows(&owner));
    }

    #[test]
    fn non_managers_never_receive_hidden_member_aggregation() {
        let snapshot = TopologySnapshot {
            revision: 1,
            generated_at_unix_ms: 1_200,
            viewer: node_id(b"viewer"),
            truncated: true,
            boundaries: vec![super::super::topology::TopologyBoundary {
                network_id: network_id(),
                name: "network".into(),
                member_count: 80,
                visible_member_count: 64,
                viewer_role: Some(MembershipRole::Member),
                capabilities: vec![],
            }],
            nodes: vec![],
            connections: vec![],
            clusters: vec![super::super::topology::TopologyCluster {
                network_id: network_id(),
                hidden_member_count: 16,
                sample: vec![node_id(b"hidden-member")],
                expandable: true,
            }],
        };
        let member =
            project_topology_snapshot(&snapshot, &viewer(MembershipRole::Member), None, 1_500);
        assert!(member.clusters.is_empty());
        assert!(!serde_json::to_string(&member)
            .unwrap()
            .contains("hidden-member"));

        let administrator = project_topology_snapshot(
            &snapshot,
            &viewer(MembershipRole::Administrator),
            None,
            1_500,
        );
        assert_eq!(administrator.clusters, snapshot.clusters);
    }
}
