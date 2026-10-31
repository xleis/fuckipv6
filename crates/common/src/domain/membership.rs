//! Per-network membership and lifecycle state machine.

use super::capability::CapabilityId;
use super::foundation::{require_non_empty, ContractIssue, ContractResult, UnixMillis};
use super::ids::{GroupId, NetworkId, NodeId, PolicyId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

pub const MEMBERSHIP_CONTRACT_VERSION: u16 = 1;

/// Primary governance role plus an optional temporary Coordinator grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkRole {
    Owner,
    Administrator,
    Member,
    TemporaryCoordinator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
}

impl MembershipState {
    pub const fn can_access_network_resources(self) -> bool {
        matches!(self, Self::Active | Self::Degraded)
    }

    pub const fn has_accepted_terms(self) -> bool {
        matches!(
            self,
            Self::Accepted
                | Self::KeyReady
                | Self::Connecting
                | Self::Active
                | Self::Degraded
                | Self::Leaving
                | Self::Left
                | Self::Suspended
        )
    }

    fn allowed_next(self) -> &'static [Self] {
        match self {
            Self::Discovered => &[Self::Applying, Self::Rejected],
            Self::Applying => &[
                Self::PendingApproval,
                Self::Accepted,
                Self::Rejected,
                Self::Left,
            ],
            Self::PendingApproval => &[Self::Accepted, Self::Rejected, Self::Left],
            Self::Accepted => &[
                Self::KeyReady,
                Self::Leaving,
                Self::Rejected,
                Self::Suspended,
            ],
            Self::KeyReady => &[Self::Connecting, Self::Leaving, Self::Suspended],
            Self::Connecting => &[Self::Active, Self::Leaving, Self::Suspended],
            Self::Active => &[Self::Degraded, Self::Leaving, Self::Suspended],
            Self::Degraded => &[Self::Active, Self::Leaving, Self::Suspended],
            Self::Leaving => &[Self::Left],
            Self::Left => &[Self::Applying],
            Self::Rejected => &[Self::Applying],
            Self::Suspended => &[Self::KeyReady, Self::Leaving, Self::Rejected],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinTerms {
    pub terms_version: String,
    pub terms_digest: String,
    pub require_exit_contribution: bool,
    pub privacy_notice_version: String,
    pub accepted_at: UnixMillis,
}

impl JoinTerms {
    fn validate(&self, issues: &mut Vec<ContractIssue>) {
        require_non_empty(&self.terms_version, "join_terms.terms_version", 128, issues);
        require_non_empty(&self.terms_digest, "join_terms.terms_digest", 256, issues);
        require_non_empty(
            &self.privacy_notice_version,
            "join_terms.privacy_notice_version",
            128,
            issues,
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkMembership {
    pub schema_version: u16,
    pub network_id: NetworkId,
    pub node_id: NodeId,
    pub state: MembershipState,
    pub roles: BTreeSet<NetworkRole>,
    pub capabilities: BTreeSet<CapabilityId>,
    pub local_policy_id: Option<PolicyId>,
    pub group_ids: BTreeSet<GroupId>,
    pub join_terms: Option<JoinTerms>,
    pub revision: u64,
    pub updated_at: UnixMillis,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MembershipError {
    Contract(Vec<ContractIssue>),
    InvalidTransition {
        current: MembershipState,
        requested: MembershipState,
    },
    TermsNotAccepted {
        requested: MembershipState,
    },
    StaleRevision {
        expected: u64,
        actual: u64,
    },
}

impl fmt::Display for MembershipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(issues) => write!(f, "invalid membership contract: {issues:?}"),
            Self::InvalidTransition { current, requested } => {
                write!(
                    f,
                    "invalid membership transition: {current:?} -> {requested:?}"
                )
            }
            Self::TermsNotAccepted { requested } => {
                write!(f, "cannot enter {requested:?} before accepting join terms")
            }
            Self::StaleRevision { expected, actual } => {
                write!(
                    f,
                    "stale membership revision: expected {expected}, got {actual}"
                )
            }
        }
    }
}

impl std::error::Error for MembershipError {}

impl NetworkMembership {
    pub fn validate(&self) -> ContractResult {
        let mut issues = Vec::new();
        if self.schema_version != MEMBERSHIP_CONTRACT_VERSION {
            issues.push(ContractIssue::new(
                "unsupported_schema_version",
                "schema_version",
                format!("membership schema version must be {MEMBERSHIP_CONTRACT_VERSION}"),
            ));
        }
        if self.revision == 0 {
            issues.push(ContractIssue::new(
                "invalid_revision",
                "revision",
                "membership revision must be greater than zero",
            ));
        }

        let primary_roles = self
            .roles
            .iter()
            .filter(|role| !matches!(role, NetworkRole::TemporaryCoordinator))
            .count();
        if primary_roles > 1 {
            issues.push(ContractIssue::new(
                "multiple_primary_roles",
                "roles",
                "a membership may have only one owner, administrator, or member role",
            ));
        }
        if self.roles.contains(&NetworkRole::TemporaryCoordinator) && primary_roles == 0 {
            issues.push(ContractIssue::new(
                "missing_primary_role",
                "roles",
                "temporary Coordinator must accompany a primary membership role",
            ));
        }
        if self.state.has_accepted_terms() && self.join_terms.is_none() {
            issues.push(ContractIssue::new(
                "join_terms_required",
                "join_terms",
                "accepted membership states require the accepted join terms",
            ));
        }
        if let Some(terms) = &self.join_terms {
            terms.validate(&mut issues);
        }
        if self.state.can_access_network_resources() && self.join_terms.is_none() {
            issues.push(ContractIssue::new(
                "resource_access_without_terms",
                "state",
                "active memberships require accepted join terms",
            ));
        }
        finish(issues)
    }

    /// Apply a lifecycle transition. Optimistic concurrency is explicit so a
    /// stale background worker cannot overwrite a newer membership state.
    pub fn transition(
        &mut self,
        next: MembershipState,
        expected_revision: u64,
        at: UnixMillis,
    ) -> Result<(), MembershipError> {
        if self.revision != expected_revision {
            return Err(MembershipError::StaleRevision {
                expected: expected_revision,
                actual: self.revision,
            });
        }
        if !self.state.allowed_next().contains(&next) {
            return Err(MembershipError::InvalidTransition {
                current: self.state,
                requested: next,
            });
        }
        if next.has_accepted_terms() && self.join_terms.is_none() {
            return Err(MembershipError::TermsNotAccepted { requested: next });
        }
        self.state = next;
        self.revision = self.revision.saturating_add(1);
        self.updated_at = at;
        self.validate().map_err(MembershipError::Contract)
    }

    pub fn can_access_network_resources(&self) -> bool {
        self.join_terms.is_some() && self.state.can_access_network_resources()
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

    fn membership() -> NetworkMembership {
        NetworkMembership {
            schema_version: MEMBERSHIP_CONTRACT_VERSION,
            network_id: NetworkId::from_bytes([1; 32]),
            node_id: NodeId::from_bytes([2; 32]),
            state: MembershipState::Discovered,
            roles: BTreeSet::from([NetworkRole::Member]),
            capabilities: BTreeSet::new(),
            local_policy_id: None,
            group_ids: BTreeSet::new(),
            join_terms: None,
            revision: 1,
            updated_at: UnixMillis(1),
        }
    }

    fn terms() -> JoinTerms {
        JoinTerms {
            terms_version: "1".into(),
            terms_digest: "sha256:abc".into(),
            require_exit_contribution: true,
            privacy_notice_version: "1".into(),
            accepted_at: UnixMillis(2),
        }
    }

    #[test]
    fn requires_terms_before_accepted_or_resource_access() {
        let mut membership = membership();
        assert!(!membership.can_access_network_resources());
        membership
            .transition(MembershipState::Applying, 1, UnixMillis(2))
            .expect("apply");
        let error = membership
            .transition(MembershipState::Accepted, 2, UnixMillis(3))
            .expect_err("accepted without terms");
        assert_eq!(
            error,
            MembershipError::TermsNotAccepted {
                requested: MembershipState::Accepted
            }
        );
    }

    #[test]
    fn accepted_is_not_enough_for_network_resource_access() {
        let mut membership = membership();
        membership.join_terms = Some(terms());
        membership
            .transition(MembershipState::Applying, 1, UnixMillis(2))
            .expect("apply");
        membership
            .transition(MembershipState::Accepted, 2, UnixMillis(3))
            .expect("accept");
        assert!(!membership.can_access_network_resources());
        membership
            .transition(MembershipState::KeyReady, 3, UnixMillis(4))
            .expect("key");
        membership
            .transition(MembershipState::Connecting, 4, UnixMillis(5))
            .expect("connect");
        membership
            .transition(MembershipState::Active, 5, UnixMillis(6))
            .expect("active");
        assert!(membership.can_access_network_resources());
    }

    #[test]
    fn rejects_illegal_transitions_and_stale_writes() {
        let mut membership = membership();
        assert_eq!(
            membership
                .transition(MembershipState::Active, 1, UnixMillis(2))
                .expect_err("illegal transition"),
            MembershipError::InvalidTransition {
                current: MembershipState::Discovered,
                requested: MembershipState::Active,
            }
        );
        assert_eq!(
            membership
                .transition(MembershipState::Applying, 0, UnixMillis(2))
                .expect_err("stale revision"),
            MembershipError::StaleRevision {
                expected: 0,
                actual: 1,
            }
        );
    }

    #[test]
    fn active_membership_survives_serialized_restart() {
        let mut membership = membership();
        membership.join_terms = Some(terms());
        for (next, revision) in [
            (MembershipState::Applying, 1),
            (MembershipState::Accepted, 2),
            (MembershipState::KeyReady, 3),
            (MembershipState::Connecting, 4),
            (MembershipState::Active, 5),
        ] {
            membership
                .transition(next, revision, UnixMillis(revision + 1))
                .expect("transition");
        }

        let json = serde_json::to_string(&membership).expect("serialize");
        let restored: NetworkMembership = serde_json::from_str(&json).expect("deserialize");
        restored.validate().expect("valid restored membership");
        assert!(restored.can_access_network_resources());
    }

    #[test]
    fn temporary_coordinator_cannot_be_the_only_role() {
        let mut membership = membership();
        membership.roles = BTreeSet::from([NetworkRole::TemporaryCoordinator]);
        assert!(membership
            .validate()
            .expect_err("missing primary role")
            .iter()
            .any(|issue| issue.code == "missing_primary_role"));
    }
}
