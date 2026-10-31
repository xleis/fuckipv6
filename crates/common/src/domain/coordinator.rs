use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ids::{NetworkId, NodeId};
use super::{DomainError, DomainResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinatorAuthority {
    /// Elected by a majority of the configured candidate group.
    Full,
    /// Temporary authority during partition. It cannot perform governance writes.
    Emergency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinatorPhase {
    Stable,
    Election,
    LeaderAck,
    Partition,
    EmergencyProposal,
    EmergencyLeader,
    Handoff,
    RecoveryRequired,
}

impl CoordinatorPhase {
    pub fn can_transition_to(self, target: Self) -> bool {
        matches!(
            (self, target),
            (Self::Stable, Self::Election)
                | (Self::Election, Self::LeaderAck)
                | (Self::Election, Self::Partition)
                | (Self::LeaderAck, Self::Stable)
                | (Self::LeaderAck, Self::Partition)
                | (Self::Stable, Self::Partition)
                | (Self::Partition, Self::EmergencyProposal)
                | (Self::Partition, Self::RecoveryRequired)
                | (Self::EmergencyProposal, Self::EmergencyLeader)
                | (Self::EmergencyProposal, Self::RecoveryRequired)
                | (Self::EmergencyLeader, Self::Handoff)
                | (Self::EmergencyLeader, Self::RecoveryRequired)
                | (Self::Handoff, Self::Stable)
                | (Self::RecoveryRequired, Self::Stable)
        )
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
pub struct FencingToken(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinatorWriteKind {
    PublishOwnTerm,
    SyncLastPolicy,
    ConnectionCoordination,
    HealthCheck,
    RejectStaleState,
    Membership,
    Ownership,
    SecurityPolicy,
    TrafficPolicy,
    Keys,
    CandidateSet,
    AuditDeletion,
    SelfPromotion,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct CoordinatorTerm {
    pub network_id: NetworkId,
    pub term: u64,
    pub leader: NodeId,
    pub candidate_group: Vec<NodeId>,
    pub majority_proof: Vec<NodeId>,
    pub authority: CoordinatorAuthority,
    pub phase: CoordinatorPhase,
    pub fencing_token: FencingToken,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
}

impl CoordinatorTerm {
    pub fn validate_at(&self, now_unix_ms: u64) -> DomainResult<()> {
        if self.term == 0 || self.fencing_token.0 == 0 {
            return Err(DomainError::invalid_value(
                "coordinator term and fencing token must be positive",
            ));
        }
        if self.issued_at_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::invariant(
                "coordinator term validity interval is empty",
            ));
        }
        if now_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::new(
                "domain.coordinator_term_expired",
                "coordinator term has expired",
            ));
        }
        let candidates: BTreeSet<_> = self.candidate_group.iter().collect();
        if candidates.is_empty() || candidates.len() != self.candidate_group.len() {
            return Err(DomainError::invariant(
                "coordinator candidate group must be non-empty and unique",
            ));
        }
        if !candidates.contains(&self.leader) {
            return Err(DomainError::invariant(
                "coordinator leader must belong to the candidate group",
            ));
        }
        let proofs: BTreeSet<_> = self.majority_proof.iter().collect();
        if proofs.len() != self.majority_proof.len()
            || proofs.iter().any(|node| !candidates.contains(node))
            || proofs.len() <= candidates.len() / 2
        {
            return Err(DomainError::invariant(
                "coordinator majority proof is invalid",
            ));
        }
        Ok(())
    }

    pub fn allows_write(&self, write: CoordinatorWriteKind) -> bool {
        match self.authority {
            CoordinatorAuthority::Full => true,
            CoordinatorAuthority::Emergency => matches!(
                write,
                CoordinatorWriteKind::PublishOwnTerm
                    | CoordinatorWriteKind::SyncLastPolicy
                    | CoordinatorWriteKind::ConnectionCoordination
                    | CoordinatorWriteKind::HealthCheck
                    | CoordinatorWriteKind::RejectStaleState
            ),
        }
    }

    /// Terms and fencing tokens are monotonically increasing. Equal-term
    /// different leaders are a split-brain conflict, never a merge.
    pub fn validate_successor(
        &self,
        candidate: &CoordinatorTerm,
        now_unix_ms: u64,
    ) -> DomainResult<()> {
        self.validate_at(now_unix_ms)?;
        candidate.validate_at(now_unix_ms)?;
        if candidate.network_id != self.network_id {
            return Err(DomainError::invariant(
                "coordinator candidate belongs to another network",
            ));
        }
        if candidate.term < self.term {
            return Err(DomainError::new(
                "domain.coordinator_term_rollback",
                "coordinator term moved backwards",
            ));
        }
        if candidate.term == self.term {
            return Err(DomainError::new(
                "domain.coordinator_term_conflict",
                "coordinator term is already occupied",
            ));
        }
        if candidate.fencing_token <= self.fencing_token {
            return Err(DomainError::new(
                "domain.fencing_token_rollback",
                "fencing token must increase with the term",
            ));
        }
        Ok(())
    }

    pub fn transition_phase(
        &mut self,
        target: CoordinatorPhase,
        now_unix_ms: u64,
    ) -> DomainResult<()> {
        self.validate_at(now_unix_ms)?;
        if !self.phase.can_transition_to(target) {
            return Err(DomainError::new(
                "domain.invalid_coordinator_transition",
                format!(
                    "coordinator cannot transition from {:?} to {:?}",
                    self.phase, target
                ),
            ));
        }
        self.phase = target;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(term: u64, token: u64, leader: NodeId) -> CoordinatorTerm {
        let first = NodeId::from_public_key(b"candidate-1");
        let second = NodeId::from_public_key(b"candidate-2");
        CoordinatorTerm {
            network_id: NetworkId::from_public_key(b"network"),
            term,
            leader,
            candidate_group: vec![first.clone(), second.clone()],
            majority_proof: vec![first, second],
            authority: CoordinatorAuthority::Full,
            phase: CoordinatorPhase::Stable,
            fencing_token: FencingToken(token),
            issued_at_unix_ms: 10,
            expires_at_unix_ms: 1_000,
        }
    }

    #[test]
    fn majority_proof_and_leader_are_checked() {
        let leader = NodeId::from_public_key(b"candidate-1");
        let mut valid = term(1, 1, leader.clone());
        valid.validate_at(100).unwrap();
        valid.majority_proof.clear();
        assert!(valid.validate_at(100).is_err());

        let outsider = NodeId::from_public_key(b"outsider");
        let mut invalid = term(1, 1, outsider);
        invalid.majority_proof = vec![NodeId::from_public_key(b"candidate-1")];
        assert!(invalid.validate_at(100).is_err());
    }

    #[test]
    fn stale_terms_and_fencing_tokens_are_rejected_after_recovery() {
        let leader = NodeId::from_public_key(b"candidate-1");
        let current = term(3, 30, leader.clone());
        let stale = term(2, 20, leader.clone());
        assert_eq!(
            current.validate_successor(&stale, 100).unwrap_err().code(),
            "domain.coordinator_term_rollback"
        );
        let same_term = term(3, 31, NodeId::from_public_key(b"candidate-2"));
        assert_eq!(
            current
                .validate_successor(&same_term, 100)
                .unwrap_err()
                .code(),
            "domain.coordinator_term_conflict"
        );
        let mut next = term(4, 30, leader);
        assert_eq!(
            current.validate_successor(&next, 100).unwrap_err().code(),
            "domain.fencing_token_rollback"
        );
        next.fencing_token = FencingToken(31);
        current.validate_successor(&next, 100).unwrap();
    }

    #[test]
    fn emergency_leader_cannot_write_governance_or_promote_itself() {
        let leader = NodeId::from_public_key(b"candidate-1");
        let mut emergency = term(4, 40, leader);
        emergency.authority = CoordinatorAuthority::Emergency;
        emergency.phase = CoordinatorPhase::EmergencyLeader;
        assert!(emergency.allows_write(CoordinatorWriteKind::HealthCheck));
        assert!(emergency.allows_write(CoordinatorWriteKind::RejectStaleState));
        for forbidden in [
            CoordinatorWriteKind::Membership,
            CoordinatorWriteKind::Ownership,
            CoordinatorWriteKind::SecurityPolicy,
            CoordinatorWriteKind::Keys,
            CoordinatorWriteKind::AuditDeletion,
            CoordinatorWriteKind::SelfPromotion,
        ] {
            assert!(!emergency.allows_write(forbidden));
        }
    }

    #[test]
    fn illegal_phase_transitions_fail_closed() {
        let leader = NodeId::from_public_key(b"candidate-1");
        let mut coordinator = term(1, 1, leader);
        assert!(coordinator
            .transition_phase(CoordinatorPhase::EmergencyLeader, 100)
            .is_err());
        coordinator
            .transition_phase(CoordinatorPhase::Election, 100)
            .unwrap();
        coordinator
            .transition_phase(CoordinatorPhase::Partition, 100)
            .unwrap();
        coordinator
            .transition_phase(CoordinatorPhase::EmergencyProposal, 100)
            .unwrap();
        coordinator
            .transition_phase(CoordinatorPhase::EmergencyLeader, 100)
            .unwrap();
        coordinator
            .transition_phase(CoordinatorPhase::Handoff, 100)
            .unwrap();
        coordinator
            .transition_phase(CoordinatorPhase::Stable, 100)
            .unwrap();
    }

    #[test]
    fn expired_term_is_rejected() {
        let leader = NodeId::from_public_key(b"candidate-1");
        let coordinator = term(1, 1, leader);
        assert_eq!(
            coordinator.validate_at(1_000).unwrap_err().code(),
            "domain.coordinator_term_expired"
        );
    }
}
