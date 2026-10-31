//! Coordinator leases, quorum proofs, and fencing tokens.

use super::foundation::{ContractIssue, ContractResult, UnixMillis};
use super::ids::{NetworkId, NodeId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

pub const COORDINATOR_TERM_CONTRACT_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MajorityProof {
    pub candidate_group: BTreeSet<NodeId>,
    pub approvals: BTreeSet<NodeId>,
    /// An emergency election outside the preferred candidate group is only
    /// valid after the candidate group has been proven unavailable.
    pub candidate_group_exhausted: bool,
}

impl MajorityProof {
    pub fn required_approvals(&self) -> usize {
        self.candidate_group.len() / 2 + 1
    }

    pub fn has_majority(&self) -> bool {
        self.approvals.len() >= self.required_approvals()
            && self.approvals.is_subset(&self.candidate_group)
    }

    fn validate(&self, emergency: bool, issues: &mut Vec<ContractIssue>) {
        if self.candidate_group.is_empty() {
            issues.push(ContractIssue::new(
                "candidate_group_required",
                "majority_proof.candidate_group",
                "candidate group must not be empty",
            ));
        }
        if !self.approvals.is_subset(&self.candidate_group) {
            issues.push(ContractIssue::new(
                "approval_outside_candidate_group",
                "majority_proof.approvals",
                "all approvals must belong to the candidate group",
            ));
        }
        if !self.has_majority() {
            issues.push(ContractIssue::new(
                "majority_proof_insufficient",
                "majority_proof.approvals",
                "quorum proof requires a strict majority of the candidate group",
            ));
        }
        if emergency && !self.candidate_group_exhausted {
            issues.push(ContractIssue::new(
                "candidate_group_not_exhausted",
                "majority_proof.candidate_group_exhausted",
                "emergency election requires proof the candidate group is exhausted",
            ));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FencingToken(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmergencyWrite {
    CoordinatorHandover,
    MembershipRecovery,
    IdentityRevocation,
    SecurityPolicyRollback,
    AuditCheckpoint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CoordinatorAuthority {
    Full,
    Emergency {
        allowed_writes: BTreeSet<EmergencyWrite>,
    },
}

impl CoordinatorAuthority {
    pub fn permits(&self, write: EmergencyWrite) -> bool {
        match self {
            Self::Full => true,
            Self::Emergency { allowed_writes } => allowed_writes.contains(&write),
        }
    }

    pub fn is_emergency(&self) -> bool {
        matches!(self, Self::Emergency { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinatorTerm {
    pub schema_version: u16,
    pub network_id: NetworkId,
    pub term: u64,
    pub leader: NodeId,
    pub majority_proof: MajorityProof,
    pub authority: CoordinatorAuthority,
    pub fencing_token: FencingToken,
    pub issued_at: UnixMillis,
    pub expires_at: UnixMillis,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoordinatorTermError {
    Contract(Vec<ContractIssue>),
    NetworkMismatch,
    TermNotAdvancing {
        current: u64,
        next: u64,
    },
    FencingTokenNotAdvancing {
        current: u64,
        next: u64,
    },
    ConflictingLeader {
        term: u64,
        current_leader: NodeId,
        conflicting_leader: NodeId,
    },
    Expired,
    NotYetValid,
    WriteNotPermitted,
}

impl fmt::Display for CoordinatorTermError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(issues) => write!(f, "invalid Coordinator term: {issues:?}"),
            Self::NetworkMismatch => f.write_str("Coordinator term belongs to another network"),
            Self::TermNotAdvancing { current, next } => {
                write!(f, "Coordinator term must advance: {current} -> {next}")
            }
            Self::FencingTokenNotAdvancing { current, next } => {
                write!(f, "fencing token must advance: {current} -> {next}")
            }
            Self::ConflictingLeader {
                term,
                current_leader,
                conflicting_leader,
            } => write!(
                f,
                "conflicting leaders for term {term}: {current_leader} and {conflicting_leader}"
            ),
            Self::Expired => f.write_str("Coordinator term has expired"),
            Self::NotYetValid => f.write_str("Coordinator term is not yet valid"),
            Self::WriteNotPermitted => {
                f.write_str("write is not permitted for emergency authority")
            }
        }
    }
}

impl std::error::Error for CoordinatorTermError {}

impl CoordinatorTerm {
    pub fn validate(&self, now: UnixMillis) -> ContractResult {
        let mut issues = Vec::new();
        if self.schema_version != COORDINATOR_TERM_CONTRACT_VERSION {
            issues.push(ContractIssue::new(
                "unsupported_schema_version",
                "schema_version",
                format!(
                    "Coordinator term schema version must be {COORDINATOR_TERM_CONTRACT_VERSION}"
                ),
            ));
        }
        if self.term == 0 {
            issues.push(ContractIssue::new(
                "invalid_term",
                "term",
                "Coordinator term must be greater than zero",
            ));
        } else if self.term > i64::MAX as u64 {
            issues.push(ContractIssue::new(
                "invalid_term",
                "term",
                "Coordinator term exceeds the signed 64-bit wire limit",
            ));
        }
        if self.fencing_token.0 == 0 {
            issues.push(ContractIssue::new(
                "invalid_fencing_token",
                "fencing_token",
                "fencing token must be greater than zero",
            ));
        }
        if !self.issued_at.is_before(self.expires_at) {
            issues.push(ContractIssue::new(
                "invalid_term_expiry",
                "expires_at",
                "term expiry must be after issue time",
            ));
        }
        self.majority_proof
            .validate(self.authority.is_emergency(), &mut issues);
        if let CoordinatorAuthority::Emergency { allowed_writes } = &self.authority {
            if allowed_writes.is_empty() {
                issues.push(ContractIssue::new(
                    "emergency_writes_required",
                    "authority.allowed_writes",
                    "emergency authority requires an explicit write allowlist",
                ));
            }
            if allowed_writes.contains(&EmergencyWrite::CoordinatorHandover)
                && !allowed_writes.contains(&EmergencyWrite::AuditCheckpoint)
            {
                issues.push(ContractIssue::new(
                    "audit_checkpoint_required",
                    "authority.allowed_writes",
                    "Coordinator handover must accompany an audit checkpoint",
                ));
            }
        }
        if !now.is_before(self.expires_at) && self.validate_without_clock().is_ok() {
            issues.push(ContractIssue::new(
                "term_expired",
                "expires_at",
                "Coordinator term has expired",
            ));
        }
        finish(issues)
    }

    fn validate_without_clock(&self) -> ContractResult {
        let mut issues = Vec::new();
        if self.term == 0 {
            issues.push(ContractIssue::new("invalid_term", "term", "term is zero"));
        }
        if self.fencing_token.0 == 0 {
            issues.push(ContractIssue::new(
                "invalid_fencing_token",
                "fencing_token",
                "fencing token is zero",
            ));
        }
        finish(issues)
    }

    pub fn is_live(&self, now: UnixMillis) -> bool {
        !now.is_before(self.issued_at) && now.is_before(self.expires_at)
    }

    pub fn authorize_write(
        &self,
        write: EmergencyWrite,
        now: UnixMillis,
    ) -> Result<(), CoordinatorTermError> {
        if now.is_before(self.issued_at) {
            return Err(CoordinatorTermError::NotYetValid);
        }
        if !now.is_before(self.expires_at) {
            return Err(CoordinatorTermError::Expired);
        }
        if !self.authority.permits(write) {
            return Err(CoordinatorTermError::WriteNotPermitted);
        }
        Ok(())
    }

    /// Reject the old leader immediately after a newer term is observed.
    pub fn supersedes(&self, previous: &CoordinatorTerm) -> Result<(), CoordinatorTermError> {
        if self.network_id != previous.network_id {
            return Err(CoordinatorTermError::NetworkMismatch);
        }
        if self.term <= previous.term {
            if self.term == previous.term && self.leader != previous.leader {
                return Err(CoordinatorTermError::ConflictingLeader {
                    term: self.term,
                    current_leader: previous.leader,
                    conflicting_leader: self.leader,
                });
            }
            return Err(CoordinatorTermError::TermNotAdvancing {
                current: previous.term,
                next: self.term,
            });
        }
        if self.fencing_token.0 <= previous.fencing_token.0 {
            return Err(CoordinatorTermError::FencingTokenNotAdvancing {
                current: previous.fencing_token.0,
                next: self.fencing_token.0,
            });
        }
        Ok(())
    }

    pub fn validate_replacement(
        &self,
        next: &CoordinatorTerm,
        now: UnixMillis,
    ) -> Result<(), CoordinatorTermError> {
        next.validate(now).map_err(CoordinatorTermError::Contract)?;
        next.supersedes(self)
    }
}

fn finish(issues: Vec<ContractIssue>) -> ContractResult {
    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nodes(count: u8) -> BTreeSet<NodeId> {
        (1..=count)
            .map(|byte| NodeId::from_bytes([byte; 32]))
            .collect()
    }

    fn term(
        term: u64,
        leader_byte: u8,
        fence: u64,
        authority: CoordinatorAuthority,
    ) -> CoordinatorTerm {
        let candidates = nodes(3);
        let approvals = candidates.iter().copied().take(2).collect();
        CoordinatorTerm {
            schema_version: COORDINATOR_TERM_CONTRACT_VERSION,
            network_id: NetworkId::from_bytes([8; 32]),
            term,
            leader: NodeId::from_bytes([leader_byte; 32]),
            majority_proof: MajorityProof {
                candidate_group: candidates,
                approvals,
                candidate_group_exhausted: authority.is_emergency(),
            },
            authority,
            fencing_token: FencingToken(fence),
            issued_at: UnixMillis(1),
            expires_at: UnixMillis(9_000),
        }
    }

    #[test]
    fn validates_full_and_emergency_authority() {
        let full = term(1, 9, 1, CoordinatorAuthority::Full);
        full.validate(UnixMillis(2)).expect("full term");
        let emergency = term(
            2,
            9,
            2,
            CoordinatorAuthority::Emergency {
                allowed_writes: BTreeSet::from([
                    EmergencyWrite::CoordinatorHandover,
                    EmergencyWrite::AuditCheckpoint,
                ]),
            },
        );
        emergency.validate(UnixMillis(2)).expect("emergency term");
        assert!(emergency
            .authorize_write(EmergencyWrite::AuditCheckpoint, UnixMillis(2))
            .is_ok());
        assert_eq!(
            emergency.authorize_write(EmergencyWrite::IdentityRevocation, UnixMillis(2)),
            Err(CoordinatorTermError::WriteNotPermitted)
        );
    }

    #[test]
    fn rejects_old_term_and_stale_fencing_token() {
        let current = term(2, 9, 2, CoordinatorAuthority::Full);
        let old = term(1, 8, 1, CoordinatorAuthority::Full);
        assert_eq!(
            old.supersedes(&current),
            Err(CoordinatorTermError::TermNotAdvancing {
                current: 2,
                next: 1,
            })
        );
        let stale_fence = term(3, 9, 2, CoordinatorAuthority::Full);
        assert_eq!(
            stale_fence.supersedes(&current),
            Err(CoordinatorTermError::FencingTokenNotAdvancing {
                current: 2,
                next: 2,
            })
        );
    }

    #[test]
    fn rejects_split_brain_same_term_conflict() {
        let leader_a = term(2, 9, 2, CoordinatorAuthority::Full);
        let leader_b = term(2, 10, 3, CoordinatorAuthority::Full);
        assert_eq!(
            leader_b.supersedes(&leader_a),
            Err(CoordinatorTermError::ConflictingLeader {
                term: 2,
                current_leader: leader_a.leader,
                conflicting_leader: leader_b.leader,
            })
        );
    }

    #[test]
    fn expired_term_rejects_writes() {
        let term = term(1, 9, 1, CoordinatorAuthority::Full);
        assert_eq!(
            term.authorize_write(EmergencyWrite::AuditCheckpoint, UnixMillis(9_000)),
            Err(CoordinatorTermError::Expired)
        );
        assert!(term.validate(UnixMillis(9_000)).is_err());
    }

    #[test]
    fn insufficient_majority_is_rejected() {
        let mut term = term(1, 9, 1, CoordinatorAuthority::Full);
        term.majority_proof.approvals = term
            .majority_proof
            .approvals
            .iter()
            .copied()
            .take(1)
            .collect();
        assert!(term
            .validate(UnixMillis(2))
            .expect_err("insufficient quorum")
            .iter()
            .any(|issue| issue.code == "majority_proof_insufficient"));
    }
}
