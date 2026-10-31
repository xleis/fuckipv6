use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ids::{ContentHash, NetworkId, NodeId};
use super::{DomainError, DomainResult};

/// A member's governance role. Temporary Coordinator authority is intentionally
/// represented by `CoordinatorTerm`, never by mutating this durable role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipRole {
    Owner,
    Administrator,
    Member,
}

/// The durable state of one node's relationship with one network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipState {
    Discovered,
    Applying,
    PendingApproval,
    Accepted,
    KeyReady,
    Connecting,
    Active,
    Degraded,
    Leaving,
    Left,
    Rejected,
    Suspended,
    Cancelled,
}

impl MembershipState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Left | Self::Rejected | Self::Cancelled)
    }

    /// Data-plane resources are available only after keys are ready and the
    /// node is actually connected. Acceptance alone never grants access.
    pub fn can_access_network_resources(self) -> bool {
        matches!(self, Self::Active | Self::Degraded)
    }

    pub fn can_transition_to(self, target: Self) -> bool {
        matches!(
            (self, target),
            (Self::Discovered, Self::Applying)
                | (Self::Applying, Self::PendingApproval)
                | (Self::Applying, Self::Accepted)
                | (Self::Applying, Self::Cancelled)
                | (Self::PendingApproval, Self::Accepted)
                | (Self::PendingApproval, Self::Rejected)
                | (Self::Accepted, Self::KeyReady)
                | (Self::Accepted, Self::Leaving)
                | (Self::KeyReady, Self::Connecting)
                | (Self::KeyReady, Self::Leaving)
                | (Self::Connecting, Self::Active)
                | (Self::Connecting, Self::Degraded)
                | (Self::Connecting, Self::Leaving)
                | (Self::Active, Self::Degraded)
                | (Self::Active, Self::Leaving)
                | (Self::Active, Self::Suspended)
                | (Self::Degraded, Self::Active)
                | (Self::Degraded, Self::Leaving)
                | (Self::Degraded, Self::Suspended)
                | (Self::Suspended, Self::Active)
                | (Self::Suspended, Self::Leaving)
                | (Self::Leaving, Self::Left)
        )
    }
}

/// Exit contribution is an explicit network admission obligation, not a
/// capability silently enabled by joining.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitContributionObligation {
    Required,
    NotRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct MembershipTerms {
    pub network_policy_revision: u64,
    pub privacy_policy_revision: u64,
    pub exit_contribution: ExitContributionObligation,
}

impl MembershipTerms {
    pub fn content_hash(&self) -> DomainResult<ContentHash> {
        let bytes = serde_json::to_vec(self)
            .map_err(|_| DomainError::invalid_value("membership terms cannot be encoded"))?;
        Ok(ContentHash::from_bytes(&bytes))
    }

    pub fn validate(&self) -> DomainResult<()> {
        if self.network_policy_revision == 0 || self.privacy_policy_revision == 0 {
            return Err(DomainError::invalid_value(
                "membership policy revisions must be positive",
            ));
        }
        Ok(())
    }
}

/// Proof that the owner was shown the exact terms before applying to a network.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct JoinTermsAck {
    pub terms_hash: ContentHash,
    pub acknowledged_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkMembership {
    pub network_id: NetworkId,
    pub node_id: NodeId,
    pub state: MembershipState,
    pub role: MembershipRole,
    pub local_policy_revision: u64,
    pub terms: Option<MembershipTerms>,
    pub terms_ack: Option<JoinTermsAck>,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    pub suspension_reason: Option<String>,
}

impl NetworkMembership {
    pub fn new(
        network_id: NetworkId,
        node_id: NodeId,
        role: MembershipRole,
        now_unix_ms: u64,
    ) -> Self {
        Self {
            network_id,
            node_id,
            state: MembershipState::Discovered,
            role,
            local_policy_revision: 1,
            terms: None,
            terms_ack: None,
            created_at_unix_ms: now_unix_ms,
            updated_at_unix_ms: now_unix_ms,
            suspension_reason: None,
        }
    }

    /// Capture the mandatory disclosure and move to `applying`. The caller must
    /// pass proof that this exact terms document was shown to the owner.
    pub fn begin_apply(
        &mut self,
        terms: MembershipTerms,
        ack: JoinTermsAck,
        now_unix_ms: u64,
    ) -> DomainResult<()> {
        terms.validate()?;
        if ack.terms_hash != terms.content_hash()? {
            return Err(DomainError::new(
                "domain.terms_not_acknowledged",
                "join terms acknowledgement does not match the displayed terms",
            ));
        }
        self.ensure_transition(MembershipState::Applying)?;
        self.terms = Some(terms);
        self.terms_ack = Some(ack);
        self.transition_to(MembershipState::Applying, now_unix_ms)
    }

    pub fn require_approval(&mut self, now_unix_ms: u64) -> DomainResult<()> {
        self.transition_to(MembershipState::PendingApproval, now_unix_ms)
    }

    pub fn accept(&mut self, now_unix_ms: u64) -> DomainResult<()> {
        self.transition_to(MembershipState::Accepted, now_unix_ms)
    }

    pub fn reject(&mut self, now_unix_ms: u64) -> DomainResult<()> {
        self.transition_to(MembershipState::Rejected, now_unix_ms)
    }

    pub fn cancel(&mut self, now_unix_ms: u64) -> DomainResult<()> {
        self.transition_to(MembershipState::Cancelled, now_unix_ms)
    }

    pub fn mark_key_ready(&mut self, now_unix_ms: u64) -> DomainResult<()> {
        if self.terms_ack.is_none() {
            return Err(DomainError::new(
                "domain.terms_not_acknowledged",
                "membership has no accepted join terms",
            ));
        }
        self.transition_to(MembershipState::KeyReady, now_unix_ms)
    }

    pub fn transition_to(&mut self, target: MembershipState, now_unix_ms: u64) -> DomainResult<()> {
        self.ensure_transition(target)?;
        if now_unix_ms < self.updated_at_unix_ms {
            return Err(DomainError::invariant(
                "membership update time moved backwards",
            ));
        }
        self.state = target;
        self.updated_at_unix_ms = now_unix_ms;
        if !matches!(target, MembershipState::Suspended) {
            self.suspension_reason = None;
        }
        Ok(())
    }

    pub fn suspend(&mut self, reason: impl Into<String>, now_unix_ms: u64) -> DomainResult<()> {
        let reason = reason.into();
        if reason.is_empty() || reason.len() > 256 {
            return Err(DomainError::invalid_value(
                "membership suspension reason is invalid",
            ));
        }
        self.transition_to(MembershipState::Suspended, now_unix_ms)?;
        self.suspension_reason = Some(reason);
        Ok(())
    }

    /// Runtime-only states do not survive a process restart as if their old
    /// connections still existed. Durable governance state is preserved.
    pub fn recover_after_restart(&mut self, now_unix_ms: u64) -> DomainResult<()> {
        let recovered = match self.state {
            MembershipState::Applying => MembershipState::Discovered,
            MembershipState::Connecting | MembershipState::Active | MembershipState::Degraded => {
                MembershipState::KeyReady
            }
            state => state,
        };
        if recovered != self.state {
            if now_unix_ms < self.updated_at_unix_ms {
                return Err(DomainError::invariant(
                    "membership recovery time moved backwards",
                ));
            }
            // Recovery is a process-boundary reconciliation, not a normal
            // protocol transition; old live connections are never trusted.
            self.state = recovered;
            self.updated_at_unix_ms = now_unix_ms;
            self.suspension_reason = None;
        }
        Ok(())
    }

    pub fn validate(&self) -> DomainResult<()> {
        if self.local_policy_revision == 0 {
            return Err(DomainError::invalid_value(
                "local membership policy revision must be positive",
            ));
        }
        if self.updated_at_unix_ms < self.created_at_unix_ms {
            return Err(DomainError::invariant(
                "membership update time predates creation",
            ));
        }
        if self.state != MembershipState::Discovered && self.terms_ack.is_none() {
            return Err(DomainError::new(
                "domain.terms_not_acknowledged",
                "membership advanced without accepted join terms",
            ));
        }
        if let Some(terms) = &self.terms {
            terms.validate()?;
            if self.terms_ack.as_ref().map(|ack| &ack.terms_hash) != Some(&terms.content_hash()?) {
                return Err(DomainError::invariant(
                    "membership terms acknowledgement is inconsistent",
                ));
            }
        }
        Ok(())
    }

    fn ensure_transition(&self, target: MembershipState) -> DomainResult<()> {
        if !self.state.can_transition_to(target) {
            return Err(DomainError::new(
                "domain.invalid_membership_transition",
                format!(
                    "membership cannot transition from {:?} to {:?}",
                    self.state, target
                ),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_membership() -> NetworkMembership {
        NetworkMembership::new(
            NetworkId::from_public_key(b"network"),
            NodeId::from_public_key(b"node"),
            MembershipRole::Member,
            10,
        )
    }

    fn terms() -> MembershipTerms {
        MembershipTerms {
            network_policy_revision: 1,
            privacy_policy_revision: 1,
            exit_contribution: ExitContributionObligation::Required,
        }
    }

    fn ack(terms: &MembershipTerms) -> JoinTermsAck {
        JoinTermsAck {
            terms_hash: terms.content_hash().unwrap(),
            acknowledged_at_unix_ms: 10,
        }
    }

    #[test]
    fn cannot_apply_without_exact_terms_acknowledgement() {
        let mut membership = new_membership();
        let terms = terms();
        let wrong = JoinTermsAck {
            terms_hash: ContentHash::from_bytes(b"other"),
            acknowledged_at_unix_ms: 10,
        };
        assert_eq!(
            membership
                .begin_apply(terms.clone(), wrong, 11)
                .unwrap_err()
                .code(),
            "domain.terms_not_acknowledged"
        );
        membership
            .begin_apply(terms.clone(), ack(&terms), 11)
            .unwrap();
        assert_eq!(membership.state, MembershipState::Applying);
    }

    #[test]
    fn accepted_does_not_grant_data_plane_access() {
        let mut membership = new_membership();
        let terms = terms();
        membership
            .begin_apply(terms.clone(), ack(&terms), 11)
            .unwrap();
        membership.accept(12).unwrap();
        assert!(!membership.state.can_access_network_resources());
        membership.mark_key_ready(13).unwrap();
        membership
            .transition_to(MembershipState::Connecting, 14)
            .unwrap();
        membership
            .transition_to(MembershipState::Active, 15)
            .unwrap();
        assert!(membership.state.can_access_network_resources());
        membership.validate().unwrap();
    }

    #[test]
    fn illegal_transitions_and_late_acknowledgements_fail_closed() {
        let mut membership = new_membership();
        assert_eq!(
            membership
                .transition_to(MembershipState::Active, 11)
                .unwrap_err()
                .code(),
            "domain.invalid_membership_transition"
        );
        let terms = terms();
        membership
            .begin_apply(terms.clone(), ack(&terms), 11)
            .unwrap();
        assert!(membership.accept(12).is_ok());

        let mut membership = new_membership();
        membership
            .begin_apply(terms.clone(), ack(&terms), 11)
            .unwrap();
        assert!(membership.mark_key_ready(12).is_err());
    }

    #[test]
    fn restart_recovery_does_not_claim_an_old_connection_is_active() {
        let mut membership = new_membership();
        let terms = terms();
        membership
            .begin_apply(terms.clone(), ack(&terms), 11)
            .unwrap();
        membership.accept(12).unwrap();
        membership.mark_key_ready(13).unwrap();
        membership
            .transition_to(MembershipState::Connecting, 14)
            .unwrap();
        membership
            .transition_to(MembershipState::Active, 15)
            .unwrap();
        membership.recover_after_restart(16).unwrap();
        assert_eq!(membership.state, MembershipState::KeyReady);
        assert!(!membership.state.can_access_network_resources());
    }

    #[test]
    fn suspension_requires_a_reason_and_can_reactivate() {
        let mut membership = new_membership();
        let terms = terms();
        membership
            .begin_apply(terms.clone(), ack(&terms), 11)
            .unwrap();
        membership.accept(12).unwrap();
        membership.mark_key_ready(13).unwrap();
        membership
            .transition_to(MembershipState::Connecting, 14)
            .unwrap();
        membership
            .transition_to(MembershipState::Active, 15)
            .unwrap();
        assert!(membership.suspend("", 16).is_err());
        membership.suspend("policy review", 16).unwrap();
        assert_eq!(
            membership.suspension_reason.as_deref(),
            Some("policy review")
        );
        membership
            .transition_to(MembershipState::Active, 17)
            .unwrap();
        assert!(membership.suspension_reason.is_none());
    }
}
