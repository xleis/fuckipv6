use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::capability::CapabilityKind;
use super::ids::{AuditId, IdempotencyKey, NetworkId, NodeId, OperationId};
use super::membership::{MembershipRole, NetworkMemberPresence, NetworkMemberStatus};
use super::{DomainError, DomainResult};

/// Every durable governance action uses this vocabulary. Runtime connection
/// states stay in `MembershipState`; this enum is the operation semantics shared
/// by the Coordinator, command bus, audit/activity projections and UI adapters.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum MemberLifecycleAction {
    Invite,
    Approve,
    Reject,
    Suspend,
    Resume,
    Exit,
    Remove,
    UndoRemove,
}

impl MemberLifecycleAction {
    pub const fn audit_action(self) -> &'static str {
        match self {
            Self::Invite => "membership.invite",
            Self::Approve => "membership.approve",
            Self::Reject => "membership.reject",
            Self::Suspend => "membership.suspend",
            Self::Resume => "membership.resume",
            Self::Exit => "membership.exit",
            Self::Remove => "membership.remove",
            Self::UndoRemove => "membership.undo_remove",
        }
    }

    pub const fn is_manager_action(self) -> bool {
        !matches!(self, Self::Exit)
    }
}

/// Authoritative lifecycle state independent of the local client's connection
/// workflow. A live data-plane observation may refine `Approved` to `Active`,
/// but it can never overwrite a suspended, exited or removed tombstone.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum MemberLifecycleStatus {
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
}

impl MemberLifecycleStatus {
    pub const fn grants_network_access(self) -> bool {
        matches!(self, Self::Approved | Self::Active | Self::Degraded)
    }

    pub const fn is_tombstone(self) -> bool {
        matches!(self, Self::Suspended | Self::Exited | Self::Removed)
    }

    pub const fn requires_fresh_admission(self) -> bool {
        matches!(
            self,
            Self::Invited
                | Self::PendingApproval
                | Self::Exited
                | Self::Removed
                | Self::Expired
                | Self::Rejected
        )
    }

    pub fn to_member_status(self, presence: NetworkMemberPresence) -> NetworkMemberStatus {
        match self {
            Self::Invited | Self::PendingApproval => NetworkMemberStatus::Pending,
            Self::Approved
                if presence.state == super::membership::NetworkMemberOnlineState::Online =>
            {
                NetworkMemberStatus::Active
            }
            Self::Approved => NetworkMemberStatus::Approved,
            Self::Active => NetworkMemberStatus::Active,
            Self::Degraded => NetworkMemberStatus::Degraded,
            Self::Suspended => NetworkMemberStatus::Suspended,
            Self::Exited => NetworkMemberStatus::Exited,
            Self::Removed => NetworkMemberStatus::Removed,
            Self::Expired | Self::Rejected => NetworkMemberStatus::Expired,
        }
    }
}

/// Maximum allowed transition table. Reapplying the transition that produced
/// the current state is handled as an exact idempotent retry by the ledger.
pub fn member_lifecycle_target(
    current: Option<MemberLifecycleStatus>,
    action: MemberLifecycleAction,
) -> DomainResult<MemberLifecycleStatus> {
    let target = match action {
        MemberLifecycleAction::Invite => match current {
            None | Some(MemberLifecycleStatus::Exited | MemberLifecycleStatus::Removed) => {
                MemberLifecycleStatus::Invited
            }
            Some(_) => return Err(invalid_transition(action, current)),
        },
        MemberLifecycleAction::Approve => match current {
            Some(MemberLifecycleStatus::Invited | MemberLifecycleStatus::PendingApproval) => {
                MemberLifecycleStatus::Approved
            }
            _ => return Err(invalid_transition(action, current)),
        },
        MemberLifecycleAction::Reject => match current {
            Some(MemberLifecycleStatus::Invited | MemberLifecycleStatus::PendingApproval) => {
                MemberLifecycleStatus::Rejected
            }
            _ => return Err(invalid_transition(action, current)),
        },
        MemberLifecycleAction::Suspend => match current {
            Some(
                MemberLifecycleStatus::Approved
                | MemberLifecycleStatus::Active
                | MemberLifecycleStatus::Degraded,
            ) => MemberLifecycleStatus::Suspended,
            _ => return Err(invalid_transition(action, current)),
        },
        MemberLifecycleAction::Resume => match current {
            Some(MemberLifecycleStatus::Suspended) => MemberLifecycleStatus::Approved,
            _ => return Err(invalid_transition(action, current)),
        },
        MemberLifecycleAction::Exit => match current {
            Some(
                MemberLifecycleStatus::Approved
                | MemberLifecycleStatus::Active
                | MemberLifecycleStatus::Degraded
                | MemberLifecycleStatus::Suspended,
            ) => MemberLifecycleStatus::Exited,
            _ => return Err(invalid_transition(action, current)),
        },
        MemberLifecycleAction::Remove => match current {
            Some(
                MemberLifecycleStatus::Approved
                | MemberLifecycleStatus::Active
                | MemberLifecycleStatus::Degraded
                | MemberLifecycleStatus::Suspended
                | MemberLifecycleStatus::Exited,
            ) => MemberLifecycleStatus::Removed,
            _ => return Err(invalid_transition(action, current)),
        },
        MemberLifecycleAction::UndoRemove => match current {
            Some(MemberLifecycleStatus::Removed) => MemberLifecycleStatus::PendingApproval,
            _ => return Err(invalid_transition(action, current)),
        },
    };
    Ok(target)
}

const fn default_member_lifecycle_role() -> MembershipRole {
    MembershipRole::Member
}

fn invalid_transition(
    action: MemberLifecycleAction,
    current: Option<MemberLifecycleStatus>,
) -> DomainError {
    DomainError::new(
        "domain.invalid_member_lifecycle_transition",
        format!("member lifecycle action {action:?} is not allowed from {current:?}"),
    )
}

/// Server-side actor role resolved from the authoritative owner/administrator
/// roster. It is never accepted from a command payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberLifecycleActorRole {
    Owner,
    Administrator,
    Member,
}

impl From<MembershipRole> for MemberLifecycleActorRole {
    fn from(role: MembershipRole) -> Self {
        match role {
            MembershipRole::Owner => Self::Owner,
            MembershipRole::Administrator => Self::Administrator,
            MembershipRole::Member => Self::Member,
        }
    }
}

/// Enforce who may act on whom and the minimum governance continuity threshold.
/// The ordinary-member denial happens before any state mutation and cannot be
/// bypassed by submitting a stale or fabricated lifecycle revision.
pub fn authorize_member_lifecycle_action(
    actor: &NodeId,
    actor_role: MemberLifecycleActorRole,
    target: &NodeId,
    target_role: MembershipRole,
    action: MemberLifecycleAction,
    retained_administrators: usize,
) -> DomainResult<()> {
    if action == MemberLifecycleAction::Exit {
        if actor != target {
            return Err(DomainError::new(
                "domain.member_lifecycle_forbidden",
                "only the member can exit their own membership",
            ));
        }
        if target_role == MembershipRole::Owner {
            return Err(DomainError::new(
                "domain.member_owner_protected",
                "the network owner cannot exit without transferring ownership",
            ));
        }
        return Ok(());
    }

    if actor_role == MemberLifecycleActorRole::Member {
        return Err(DomainError::new(
            "domain.member_lifecycle_forbidden",
            "ordinary members cannot operate on another member",
        ));
    }
    if actor_role == MemberLifecycleActorRole::Administrator && target_role == MembershipRole::Owner
    {
        return Err(DomainError::new(
            "domain.member_owner_protected",
            "administrators cannot operate on the network owner",
        ));
    }
    if actor == target {
        return Err(DomainError::new(
            "domain.member_lifecycle_forbidden",
            "managers must use the exit action for their own membership",
        ));
    }
    if target_role == MembershipRole::Owner {
        return Err(DomainError::new(
            "domain.member_owner_protected",
            "the network owner is protected from removal, suspension and rejection",
        ));
    }
    if target_role == MembershipRole::Administrator
        && matches!(
            action,
            MemberLifecycleAction::Suspend
                | MemberLifecycleAction::Exit
                | MemberLifecycleAction::Remove
        )
        && retained_administrators == 0
    {
        return Err(DomainError::new(
            "domain.member_last_administrator_protected",
            "the last administrator cannot be suspended, exited or removed",
        ));
    }
    Ok(())
}

/// Sanitized state used by both the durable ledger and wire responses.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct MemberLifecycleRecord {
    pub network_id: NetworkId,
    pub node_id: NodeId,
    #[serde(default = "default_member_lifecycle_role")]
    pub role: MembershipRole,
    pub status: MemberLifecycleStatus,
    pub revision: u64,
    /// Monotonic access generation. Every suspension, exit, removal and undo
    /// advances it. Old certificates and transfer authorization never match a
    /// later generation.
    pub access_epoch: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_operation_id: Option<OperationId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_idempotency_key: Option<IdempotencyKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at_unix_ms: Option<u64>,
}

impl MemberLifecycleRecord {
    pub fn new(network_id: NetworkId, node_id: NodeId) -> Self {
        Self {
            network_id,
            node_id,
            role: MembershipRole::Member,
            status: MemberLifecycleStatus::PendingApproval,
            revision: 1,
            access_epoch: 1,
            last_operation_id: None,
            last_idempotency_key: None,
            updated_at_unix_ms: None,
        }
    }

    pub fn validate(&self) -> DomainResult<()> {
        if self.revision == 0 || self.access_epoch == 0 {
            return Err(DomainError::invalid_value(
                "member lifecycle revision and access epoch must be positive",
            ));
        }
        if self.updated_at_unix_ms == Some(0) {
            return Err(DomainError::invalid_value(
                "member lifecycle update time must be positive",
            ));
        }
        Ok(())
    }
}

/// Read-only request for the removal-impact preview. It intentionally has no
/// idempotency key because it cannot mutate membership or access state.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct MemberRemovalPreviewRequest {
    pub network_id: NetworkId,
    pub target_node_id: NodeId,
}

impl MemberRemovalPreviewRequest {
    pub fn validate(&self) -> DomainResult<()> {
        if self.network_id.as_str().is_empty() || self.target_node_id.as_str().is_empty() {
            return Err(DomainError::invalid_value(
                "member removal preview identity is invalid",
            ));
        }
        Ok(())
    }
}

/// Payload for every lifecycle mutation. Expected revision provides compare-and-
/// swap semantics; the idempotency key makes transport retries exactly-once.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct MemberLifecycleChangeRequest {
    pub network_id: NetworkId,
    pub target_node_id: NodeId,
    pub action: MemberLifecycleAction,
    /// Role to restore when a suspended/removed administrator is approved
    /// again. Ordinary member invitations are the default. The Coordinator
    /// validates manager authority before honoring this field.
    #[serde(default = "default_member_lifecycle_role")]
    pub role: MembershipRole,
    pub expected_revision: u64,
    pub idempotency_key: IdempotencyKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl MemberLifecycleChangeRequest {
    pub fn validate(&self) -> DomainResult<()> {
        // Revision zero means the caller has observed no previous lifecycle
        // record and is attempting the one allowed create action (Invite).
        if self.expected_revision == 0 && self.action != MemberLifecycleAction::Invite {
            return Err(DomainError::invalid_value(
                "only lifecycle creation may use expected revision zero",
            ));
        }
        if let Some(reason) = &self.reason {
            let reason = reason.trim();
            if reason.is_empty()
                || reason.chars().count() > 512
                || reason.chars().any(char::is_control)
            {
                return Err(DomainError::invalid_value(
                    "member lifecycle reason is invalid",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct MemberLifecycleChange {
    pub network_id: NetworkId,
    pub target_node_id: NodeId,
    pub action: MemberLifecycleAction,
    pub status: MemberLifecycleStatus,
    pub revision: u64,
    pub access_epoch: u64,
    pub operation_id: OperationId,
    pub audit_id: AuditId,
    /// True when the response is the durable result of an earlier retry.
    pub idempotent: bool,
    pub access_revoked: bool,
    pub requires_reauthentication: bool,
    pub requires_reapproval: bool,
}

/// Explicit, non-secret disclosure shown before removal. It states what will be
/// revoked without pretending a transfer registry exists where none does.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct MemberRemovalPreview {
    pub network_id: NetworkId,
    pub target_node_id: NodeId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub role: MembershipRole,
    pub status: NetworkMemberStatus,
    pub lifecycle_state: MemberLifecycleStatus,
    pub revision: u64,
    pub access_epoch: u64,
    pub presence: NetworkMemberPresence,
    #[serde(default)]
    pub active_capabilities: Vec<CapabilityKind>,
    pub can_remove: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denial_key: Option<String>,
    pub network_access_revoked: bool,
    pub file_access_revoked: bool,
    pub incomplete_transfers_resume_blocked: bool,
    pub credentials_revoked: bool,
    pub old_access_epoch_invalidated: bool,
    pub local_files_deleted: bool,
    pub safe_undo_requires_reapproval: bool,
    pub safe_undo_restores_old_credentials: bool,
    pub impact_keys: Vec<String>,
}

impl MemberRemovalPreview {
    pub fn validate(&self) -> DomainResult<()> {
        if self.revision == 0 || self.access_epoch == 0 {
            return Err(DomainError::invalid_value(
                "removal preview revision and access epoch must be positive",
            ));
        }
        if self.can_remove && self.denial_key.is_some() {
            return Err(DomainError::invariant(
                "removable member preview cannot carry a denial key",
            ));
        }
        if self.local_files_deleted {
            return Err(DomainError::invariant(
                "removal must not delete local files",
            ));
        }
        if !self.old_access_epoch_invalidated || self.safe_undo_restores_old_credentials {
            return Err(DomainError::invariant(
                "removal must invalidate the old epoch and must not revive credentials",
            ));
        }
        let unique = self.impact_keys.iter().collect::<BTreeSet<_>>();
        if unique.len() != self.impact_keys.len()
            || self
                .impact_keys
                .iter()
                .any(|key| key.is_empty() || key.len() > 128)
        {
            return Err(DomainError::invalid_value(
                "removal impact keys are invalid",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(seed: &str) -> NodeId {
        NodeId::from_public_key(seed.as_bytes())
    }

    #[test]
    fn lifecycle_transition_table_covers_every_governance_action() {
        use MemberLifecycleAction as A;
        use MemberLifecycleStatus as S;

        assert_eq!(
            member_lifecycle_target(None, A::Invite).unwrap(),
            S::Invited
        );
        assert_eq!(
            member_lifecycle_target(Some(S::Invited), A::Approve).unwrap(),
            S::Approved
        );
        assert_eq!(
            member_lifecycle_target(Some(S::PendingApproval), A::Reject).unwrap(),
            S::Rejected
        );
        assert_eq!(
            member_lifecycle_target(Some(S::Active), A::Suspend).unwrap(),
            S::Suspended
        );
        assert_eq!(
            member_lifecycle_target(Some(S::Suspended), A::Resume).unwrap(),
            S::Approved
        );
        assert_eq!(
            member_lifecycle_target(Some(S::Degraded), A::Exit).unwrap(),
            S::Exited
        );
        assert_eq!(
            member_lifecycle_target(Some(S::Suspended), A::Remove).unwrap(),
            S::Removed
        );
        assert_eq!(
            member_lifecycle_target(Some(S::Removed), A::UndoRemove).unwrap(),
            S::PendingApproval
        );
    }

    #[test]
    fn illegal_lifecycle_predecessors_fail_closed() {
        use MemberLifecycleAction as A;
        use MemberLifecycleStatus as S;

        assert_eq!(
            member_lifecycle_target(Some(S::Approved), A::Approve)
                .unwrap_err()
                .code(),
            "domain.invalid_member_lifecycle_transition"
        );
        assert!(member_lifecycle_target(Some(S::Removed), A::Resume).is_err());
        assert!(member_lifecycle_target(None, A::Suspend).is_err());
        assert!(member_lifecycle_target(Some(S::Removed), A::Exit).is_err());
    }

    #[test]
    fn ordinary_members_can_only_exit_themselves() {
        let member = node("member");
        let other = node("other");
        assert!(authorize_member_lifecycle_action(
            &member,
            MemberLifecycleActorRole::Member,
            &member,
            MembershipRole::Member,
            MemberLifecycleAction::Exit,
            1,
        )
        .is_ok());
        assert_eq!(
            authorize_member_lifecycle_action(
                &member,
                MemberLifecycleActorRole::Member,
                &other,
                MembershipRole::Member,
                MemberLifecycleAction::Suspend,
                1,
            )
            .unwrap_err()
            .code(),
            "domain.member_lifecycle_forbidden"
        );
    }

    #[test]
    fn owner_and_last_administrator_protection_cannot_be_bypassed() {
        let owner = node("owner");
        let admin = node("admin");
        let member = node("member");
        assert_eq!(
            authorize_member_lifecycle_action(
                &admin,
                MemberLifecycleActorRole::Administrator,
                &owner,
                MembershipRole::Owner,
                MemberLifecycleAction::Remove,
                1,
            )
            .unwrap_err()
            .code(),
            "domain.member_owner_protected"
        );
        assert_eq!(
            authorize_member_lifecycle_action(
                &owner,
                MemberLifecycleActorRole::Owner,
                &admin,
                MembershipRole::Administrator,
                MemberLifecycleAction::Remove,
                0,
            )
            .unwrap_err()
            .code(),
            "domain.member_last_administrator_protected"
        );
        assert!(authorize_member_lifecycle_action(
            &owner,
            MemberLifecycleActorRole::Owner,
            &member,
            MembershipRole::Member,
            MemberLifecycleAction::Remove,
            1,
        )
        .is_ok());
    }

    #[test]
    fn owner_exit_is_rejected_and_manager_self_action_requires_exit() {
        let owner = node("owner");
        assert_eq!(
            authorize_member_lifecycle_action(
                &owner,
                MemberLifecycleActorRole::Owner,
                &owner,
                MembershipRole::Owner,
                MemberLifecycleAction::Exit,
                1,
            )
            .unwrap_err()
            .code(),
            "domain.member_owner_protected"
        );
        assert_eq!(
            authorize_member_lifecycle_action(
                &owner,
                MemberLifecycleActorRole::Owner,
                &owner,
                MembershipRole::Owner,
                MemberLifecycleAction::Remove,
                1,
            )
            .unwrap_err()
            .code(),
            "domain.member_lifecycle_forbidden"
        );
    }

    #[test]
    fn removal_preview_never_claims_to_delete_files_or_revive_credentials() {
        let preview = MemberRemovalPreview {
            network_id: NetworkId::from_public_key(b"network"),
            target_node_id: node("member"),
            display_name: Some("Member".into()),
            role: MembershipRole::Member,
            status: NetworkMemberStatus::Active,
            lifecycle_state: MemberLifecycleStatus::Active,
            revision: 2,
            access_epoch: 3,
            presence: NetworkMemberPresence::live(10),
            active_capabilities: vec![CapabilityKind::File],
            can_remove: true,
            denial_key: None,
            network_access_revoked: true,
            file_access_revoked: true,
            incomplete_transfers_resume_blocked: true,
            credentials_revoked: true,
            old_access_epoch_invalidated: true,
            local_files_deleted: false,
            safe_undo_requires_reapproval: true,
            safe_undo_restores_old_credentials: false,
            impact_keys: vec![
                "member_removal.impact.network_access".into(),
                "member_removal.impact.file_access".into(),
                "member_removal.impact.incomplete_transfers".into(),
                "member_removal.impact.credentials".into(),
            ],
        };
        preview.validate().unwrap();
    }

    #[test]
    fn live_approved_members_project_as_active_without_overriding_tombstones() {
        use MemberLifecycleStatus as S;

        let live = NetworkMemberPresence::live(10);
        assert_eq!(
            S::Approved.to_member_status(live.clone()),
            NetworkMemberStatus::Active
        );
        assert_eq!(
            S::Approved.to_member_status(NetworkMemberPresence::unknown()),
            NetworkMemberStatus::Approved
        );
        assert_eq!(
            S::Suspended.to_member_status(live),
            NetworkMemberStatus::Suspended
        );
    }
}
