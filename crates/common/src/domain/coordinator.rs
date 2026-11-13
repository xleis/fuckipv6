use std::collections::BTreeSet;
use std::net::SocketAddr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ids::{NetworkId, NodeId, PublicKey, Signature};
use super::network::NetworkRecord;
use super::{DomainError, DomainResult};
use crate::Identity;

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

/// Maximum tolerated difference between the Coordinator's observed clock and
/// the time asserted by a term. A larger backwards jump is a fail-closed clock
/// skew alert rather than an invitation to extend stale authority.
pub const COORDINATOR_CLOCK_SKEW_LIMIT_MS: u64 = 30_000;

/// Once this much of a lease remains, the Leader must renew it. Writes remain
/// valid during this transition window, but callers must surface the warning.
pub const COORDINATOR_LEASE_RENEWAL_WINDOW_MS: u64 = 30_000;

/// Upper bound for a single lease extension. Longer-lived authority requires a
/// newly validated term rather than repeatedly renewing an old Leader.
pub const COORDINATOR_MAX_LEASE_DURATION_MS: u64 = 5 * 60 * 1000;

/// A vote is intentionally short-lived and cannot be replayed into a later
/// election. The proof must remain valid for at least the term lease it backs.
pub const COORDINATOR_ELECTION_VOTE_SCHEMA_VERSION: u16 = 1;
pub const COORDINATOR_ELECTION_VOTE_MAX_VALIDITY_MS: u64 = 5 * 60 * 1000;

const COORDINATOR_ELECTION_VOTE_SIGNING_DOMAIN: &[u8] = b"fuckipv6.coordinator-election-vote.v1\0";

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

/// The authority facts that every governance write must carry. The write kind
/// is supplied separately so one envelope cannot be silently replayed for a
/// broader operation.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct CoordinatorWriteCredential {
    pub network_id: NetworkId,
    pub term: u64,
    pub leader: NodeId,
    pub fencing_token: FencingToken,
}

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

impl CoordinatorWriteKind {
    /// Long-lived network state. These operations additionally require a Full
    /// authority in the Stable phase; an Emergency Leader is never sufficient.
    pub fn is_governance(self) -> bool {
        matches!(
            self,
            Self::Membership
                | Self::Ownership
                | Self::SecurityPolicy
                | Self::TrafficPolicy
                | Self::Keys
                | Self::CandidateSet
                | Self::AuditDeletion
                | Self::SelfPromotion
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinatorLeaseState {
    /// No term has been admitted yet. Governance remains in bootstrap mode.
    NoTerm,
    Active,
    /// The lease is still writable, but renewal is overdue soon.
    RenewalDue,
    /// Authority is no longer writable until a fresh term is admitted.
    ReadOnly,
}

/// Observable lease projection used by the control plane and UI. `reason_code`
/// is stable so a clock-skew alert cannot be mistaken for an ordinary expiry.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct CoordinatorLeaseStatus {
    pub state: CoordinatorLeaseState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_expires_at_unix_ms: Option<u64>,
    pub remaining_ms: u64,
    pub observed_clock_skew_ms: u64,
    pub max_clock_skew_ms: u64,
    pub reason_code: String,
}

impl CoordinatorLeaseStatus {
    pub fn no_term() -> Self {
        Self {
            state: CoordinatorLeaseState::NoTerm,
            lease_expires_at_unix_ms: None,
            remaining_ms: 0,
            observed_clock_skew_ms: 0,
            max_clock_skew_ms: COORDINATOR_CLOCK_SKEW_LIMIT_MS,
            reason_code: "coordinator.lease.no_term".into(),
        }
    }

    pub fn is_writable(&self) -> bool {
        matches!(
            self.state,
            CoordinatorLeaseState::Active | CoordinatorLeaseState::RenewalDue
        )
    }
}

impl Default for CoordinatorLeaseStatus {
    fn default() -> Self {
        Self::no_term()
    }
}

#[derive(Debug, Clone, Serialize)]
struct CoordinatorElectionVotePayload<'a> {
    schema_version: u16,
    network_id: &'a NetworkId,
    candidate_config_revision: u64,
    term: u64,
    leader: &'a NodeId,
    candidate_group: &'a [NodeId],
    voter: &'a NodeId,
    voter_public_key: &'a PublicKey,
    issued_at_unix_ms: u64,
    expires_at_unix_ms: u64,
}

/// A candidate-signed vote for one exact term and Leader. The signature binds
/// the network, candidate configuration revision, complete candidate group,
/// Leader and validity interval so a vote cannot be replayed for another term.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct CoordinatorElectionVote {
    pub schema_version: u16,
    pub network_id: NetworkId,
    pub candidate_config_revision: u64,
    pub term: u64,
    pub leader: NodeId,
    pub candidate_group: Vec<NodeId>,
    pub voter: NodeId,
    pub voter_public_key: PublicKey,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    pub signature: Signature,
}

impl CoordinatorElectionVote {
    #[allow(clippy::too_many_arguments)]
    pub fn signed(
        network_id: NetworkId,
        candidate_config_revision: u64,
        term: u64,
        leader: NodeId,
        candidate_group: Vec<NodeId>,
        issued_at_unix_ms: u64,
        expires_at_unix_ms: u64,
        voter: &Identity,
    ) -> DomainResult<Self> {
        let mut vote = Self {
            schema_version: COORDINATOR_ELECTION_VOTE_SCHEMA_VERSION,
            network_id,
            candidate_config_revision,
            term,
            leader,
            candidate_group,
            voter: NodeId::from_public_key(&voter.public_key()),
            voter_public_key: PublicKey::from_bytes(voter.public_key_bytes()),
            issued_at_unix_ms,
            expires_at_unix_ms,
            signature: Signature::from_bytes([0; 64]),
        };
        vote.validate_structure()?;
        let signature = voter.sign(&vote.signing_payload()?);
        vote.signature = Signature::from_bytes(signature.to_bytes());
        vote.validate_definition()?;
        Ok(vote)
    }

    fn payload(&self) -> CoordinatorElectionVotePayload<'_> {
        CoordinatorElectionVotePayload {
            schema_version: self.schema_version,
            network_id: &self.network_id,
            candidate_config_revision: self.candidate_config_revision,
            term: self.term,
            leader: &self.leader,
            candidate_group: &self.candidate_group,
            voter: &self.voter,
            voter_public_key: &self.voter_public_key,
            issued_at_unix_ms: self.issued_at_unix_ms,
            expires_at_unix_ms: self.expires_at_unix_ms,
        }
    }

    pub fn signing_payload(&self) -> DomainResult<Vec<u8>> {
        let encoded = serde_json::to_vec(&self.payload()).map_err(|error| {
            DomainError::invalid_value(format!(
                "coordinator election vote is not serializable: {error}"
            ))
        })?;
        let mut payload =
            Vec::with_capacity(COORDINATOR_ELECTION_VOTE_SIGNING_DOMAIN.len() + encoded.len());
        payload.extend_from_slice(COORDINATOR_ELECTION_VOTE_SIGNING_DOMAIN);
        payload.extend_from_slice(&encoded);
        Ok(payload)
    }

    pub fn verify_signature(&self) -> DomainResult<()> {
        let public_key = self.voter_public_key.to_bytes()?;
        if NodeId::from_public_key(&public_key) != self.voter {
            return Err(DomainError::new(
                "domain.coordinator_election_voter_mismatch",
                "coordinator election vote voter does not match its public key",
            ));
        }
        let signature = self.signature.to_bytes()?;
        if !Identity::verify(&public_key, &self.signing_payload()?, signature.as_slice()) {
            return Err(DomainError::new(
                "domain.invalid_signature",
                "coordinator election vote signature is invalid",
            ));
        }
        Ok(())
    }

    pub fn validate_structure(&self) -> DomainResult<()> {
        if self.schema_version != COORDINATOR_ELECTION_VOTE_SCHEMA_VERSION {
            return Err(DomainError::invalid_value(
                "unsupported coordinator election vote schema",
            ));
        }
        if self.candidate_config_revision == 0 || self.term == 0 {
            return Err(DomainError::invalid_value(
                "coordinator election config revision and term must be positive",
            ));
        }
        if self.issued_at_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::invalid_value(
                "coordinator election vote validity interval is empty",
            ));
        }
        if self.expires_at_unix_ms - self.issued_at_unix_ms
            > COORDINATOR_ELECTION_VOTE_MAX_VALIDITY_MS
        {
            return Err(DomainError::invalid_value(
                "coordinator election vote validity exceeds the maximum lease",
            ));
        }
        let candidates = self.candidate_group.iter().collect::<BTreeSet<_>>();
        if candidates.is_empty() || candidates.len() != self.candidate_group.len() {
            return Err(DomainError::invalid_value(
                "coordinator election candidate group must be non-empty and unique",
            ));
        }
        if !candidates.contains(&self.leader) {
            return Err(DomainError::invalid_value(
                "coordinator election Leader must belong to the candidate group",
            ));
        }
        if !candidates.contains(&self.voter) {
            return Err(DomainError::invalid_value(
                "coordinator election voter must belong to the candidate group",
            ));
        }
        Ok(())
    }

    pub fn validate_definition(&self) -> DomainResult<()> {
        self.validate_structure()?;
        self.verify_signature()
    }

    pub fn validate_for_term(&self, term: &CoordinatorTerm) -> DomainResult<()> {
        self.validate_definition()?;
        if self.network_id != term.network_id
            || self.candidate_config_revision != term.candidate_config_revision
            || self.term != term.term
            || self.leader != term.leader
            || self.candidate_group.iter().collect::<BTreeSet<_>>()
                != term.candidate_group.iter().collect::<BTreeSet<_>>()
        {
            return Err(DomainError::new(
                "domain.coordinator_election_vote_mismatch",
                "coordinator election vote does not match the term",
            ));
        }
        if self.issued_at_unix_ms > term.issued_at_unix_ms
            || self.expires_at_unix_ms < term.expires_at_unix_ms
        {
            return Err(DomainError::new(
                "domain.coordinator_election_vote_expiry",
                "coordinator election vote does not cover the term lease",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct CoordinatorTerm {
    pub network_id: NetworkId,
    pub term: u64,
    pub leader: NodeId,
    pub candidate_group: Vec<NodeId>,
    pub candidate_config_revision: u64,
    pub majority_proof: Vec<NodeId>,
    #[serde(default)]
    pub majority_votes: Vec<CoordinatorElectionVote>,
    pub authority: CoordinatorAuthority,
    pub phase: CoordinatorPhase,
    pub fencing_token: FencingToken,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
}

impl CoordinatorTerm {
    pub fn validate_definition(&self) -> DomainResult<()> {
        if self.term == 0 || self.fencing_token.0 == 0 {
            return Err(DomainError::invalid_value(
                "coordinator term and fencing token must be positive",
            ));
        }
        if self.candidate_config_revision == 0 {
            return Err(DomainError::invalid_value(
                "coordinator term candidate configuration revision must be positive",
            ));
        }
        if self.issued_at_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::invariant(
                "coordinator term validity interval is empty",
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
        if self.majority_votes.is_empty() {
            return Err(DomainError::invariant(
                "coordinator majority proof is empty",
            ));
        }
        let mut voters = BTreeSet::new();
        for vote in &self.majority_votes {
            vote.validate_for_term(self)?;
            if !voters.insert(vote.voter.clone()) {
                return Err(DomainError::invariant(
                    "coordinator majority proof contains a duplicate voter",
                ));
            }
        }
        if !voters.contains(&self.leader) {
            return Err(DomainError::invariant(
                "coordinator Leader must vote for its own term",
            ));
        }
        let proofs: BTreeSet<_> = self.majority_proof.iter().collect();
        if proofs.len() != self.majority_proof.len()
            || proofs.iter().any(|node| !candidates.contains(node))
            || proofs != voters.iter().collect()
            || proofs.len() <= candidates.len() / 2
        {
            return Err(DomainError::invariant(
                "coordinator majority proof is invalid",
            ));
        }
        Ok(())
    }

    pub fn validate_at(&self, now_unix_ms: u64) -> DomainResult<()> {
        self.validate_definition()?;
        if now_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::new(
                "domain.coordinator_term_expired",
                "coordinator term has expired",
            ));
        }
        Ok(())
    }

    pub fn lease_status_at(&self, now_unix_ms: u64) -> CoordinatorLeaseStatus {
        if self.validate_definition().is_err() {
            return CoordinatorLeaseStatus {
                state: CoordinatorLeaseState::ReadOnly,
                lease_expires_at_unix_ms: Some(self.expires_at_unix_ms),
                remaining_ms: 0,
                observed_clock_skew_ms: 0,
                max_clock_skew_ms: COORDINATOR_CLOCK_SKEW_LIMIT_MS,
                reason_code: "coordinator.lease.invalid_term".into(),
            };
        }

        // A Coordinator clock behind the term's issue time is tolerated only up
        // to a bounded skew. Beyond that, one node may believe an expired lease
        // is still valid, so authority becomes read-only immediately.
        let observed_clock_skew_ms = self.issued_at_unix_ms.saturating_sub(now_unix_ms);
        if observed_clock_skew_ms > COORDINATOR_CLOCK_SKEW_LIMIT_MS {
            return CoordinatorLeaseStatus {
                state: CoordinatorLeaseState::ReadOnly,
                lease_expires_at_unix_ms: Some(self.expires_at_unix_ms),
                remaining_ms: self.expires_at_unix_ms.saturating_sub(now_unix_ms),
                observed_clock_skew_ms,
                max_clock_skew_ms: COORDINATOR_CLOCK_SKEW_LIMIT_MS,
                reason_code: "coordinator.lease.clock_skew".into(),
            };
        }

        if now_unix_ms >= self.expires_at_unix_ms {
            return CoordinatorLeaseStatus {
                state: CoordinatorLeaseState::ReadOnly,
                lease_expires_at_unix_ms: Some(self.expires_at_unix_ms),
                remaining_ms: 0,
                observed_clock_skew_ms,
                max_clock_skew_ms: COORDINATOR_CLOCK_SKEW_LIMIT_MS,
                reason_code: "coordinator.lease.expired".into(),
            };
        }

        let remaining_ms = self.expires_at_unix_ms - now_unix_ms;
        let renewal_due = remaining_ms <= COORDINATOR_LEASE_RENEWAL_WINDOW_MS;
        CoordinatorLeaseStatus {
            state: if renewal_due {
                CoordinatorLeaseState::RenewalDue
            } else {
                CoordinatorLeaseState::Active
            },
            lease_expires_at_unix_ms: Some(self.expires_at_unix_ms),
            remaining_ms,
            observed_clock_skew_ms,
            max_clock_skew_ms: COORDINATOR_CLOCK_SKEW_LIMIT_MS,
            reason_code: if renewal_due {
                "coordinator.lease.renewal_due"
            } else {
                "coordinator.lease.active"
            }
            .into(),
        }
    }

    pub fn validate_against_candidate_config(
        &self,
        config: &CoordinatorCandidateConfig,
    ) -> DomainResult<()> {
        self.validate_definition()?;
        config.validate_definition()?;
        if self.network_id != config.network_id {
            return Err(DomainError::invariant(
                "coordinator term belongs to another network",
            ));
        }
        if self.candidate_config_revision != config.revision {
            return Err(DomainError::new(
                "domain.coordinator_term_candidate_config_revision_mismatch",
                "coordinator term does not bind the active candidate configuration revision",
            ));
        }
        let configured = config
            .candidates
            .iter()
            .map(|candidate| candidate.node_id.clone())
            .collect::<BTreeSet<_>>();
        let term_candidates = self
            .candidate_group
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        if configured != term_candidates {
            return Err(DomainError::new(
                "domain.coordinator_term_candidate_group_mismatch",
                "coordinator term candidate group does not match the active configuration",
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
        // An expired term still fences its successor. Recovery must be able to
        // advance past it even though the old Leader may no longer write.
        self.validate_definition()?;
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

    /// Validate a governance write against this authoritative term. The caller
    /// must invoke this for every write; term and fencing comparisons are made
    /// before permission or phase checks so stale leaders are always audited as
    /// rollback attempts rather than ordinary authorisation failures.
    pub fn authorize_write(
        &self,
        actor: &NodeId,
        credential: &CoordinatorWriteCredential,
        write: CoordinatorWriteKind,
        now_unix_ms: u64,
    ) -> DomainResult<CoordinatorLeaseStatus> {
        self.validate_definition()?;
        if credential.network_id != self.network_id {
            return Err(DomainError::invariant(
                "coordinator write credential belongs to another network",
            ));
        }
        if credential.term < self.term {
            return Err(DomainError::new(
                "domain.coordinator_term_rollback",
                "coordinator write carries an obsolete term",
            ));
        }
        if credential.term > self.term {
            return Err(DomainError::new(
                "domain.coordinator_term_not_installed",
                "coordinator write term has not been admitted",
            ));
        }
        if credential.fencing_token < self.fencing_token {
            return Err(DomainError::new(
                "domain.fencing_token_rollback",
                "coordinator write carries an obsolete fencing token",
            ));
        }
        if credential.fencing_token > self.fencing_token {
            return Err(DomainError::new(
                "domain.fencing_token_not_installed",
                "coordinator write fencing token has not been admitted",
            ));
        }
        if credential.leader != self.leader {
            return Err(DomainError::new(
                "domain.coordinator_term_conflict",
                "coordinator write leader does not match the authoritative term",
            ));
        }

        let lease = self.lease_status_at(now_unix_ms);
        if !lease.is_writable() {
            let code = match lease.reason_code.as_str() {
                "coordinator.lease.clock_skew" => "domain.coordinator_clock_skew",
                "coordinator.lease.expired" => "domain.coordinator_lease_expired",
                _ => "domain.coordinator_term_invalid",
            };
            return Err(DomainError::new(code, "coordinator authority is read-only"));
        }
        if actor != &self.leader {
            return Err(DomainError::new(
                "domain.coordinator_leader_mismatch",
                "coordinator write actor is not the term leader",
            ));
        }
        if write.is_governance() && self.phase != CoordinatorPhase::Stable {
            return Err(DomainError::new(
                "domain.coordinator_transition_read_only",
                "governance writes are disabled outside the stable phase",
            ));
        }
        if !self.allows_write(write) {
            return Err(DomainError::new(
                "domain.coordinator_write_forbidden",
                "coordinator authority cannot perform this write",
            ));
        }
        Ok(lease)
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

pub const COORDINATOR_CANDIDATE_CONFIG_SCHEMA_VERSION: u16 = 1;
pub const COORDINATOR_CANDIDATE_DECLARATION_SCHEMA_VERSION: u16 = 1;
pub const COORDINATOR_CANDIDATE_MAX_GROUP_SIZE: usize = 32;
pub const COORDINATOR_CANDIDATE_MAX_DECLARATION_VALIDITY_MS: u64 = 15 * 60 * 1000;
pub const COORDINATOR_CANDIDATE_DECLARATION_FUTURE_SKEW_MS: u64 = 60 * 1000;
const COORDINATOR_CANDIDATE_CONFIG_SIGNING_DOMAIN: &[u8] =
    b"fuckipv6.coordinator-candidate-config.v1\0";
const COORDINATOR_CANDIDATE_DECLARATION_SIGNING_DOMAIN: &[u8] =
    b"fuckipv6.coordinator-candidate-declaration.v1\0";

/// One administrator-selected Coordinator candidate. Lower priority values
/// are preferred, with node identity as the deterministic tie-breaker.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, JsonSchema, Serialize, Deserialize)]
pub struct CoordinatorCandidate {
    pub node_id: NodeId,
    pub priority: u32,
}

#[derive(Debug, Clone, Serialize)]
struct CoordinatorCandidateConfigPayload<'a> {
    schema_version: u16,
    network_id: &'a NetworkId,
    revision: u64,
    candidates: &'a [CoordinatorCandidate],
    effective_from_unix_ms: u64,
    expires_at_unix_ms: u64,
    signer: &'a NodeId,
    signer_public_key: &'a PublicKey,
}

/// Administrator-signed candidate group. Possessing this document is not
/// sufficient to become Leader: the signer must be a current network manager
/// when the document is admitted, every candidate must be a network member, and
/// election still requires the majority protocol defined by `CoordinatorTerm`.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct CoordinatorCandidateConfig {
    pub schema_version: u16,
    pub network_id: NetworkId,
    pub revision: u64,
    pub candidates: Vec<CoordinatorCandidate>,
    pub effective_from_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    pub signer: NodeId,
    pub signer_public_key: PublicKey,
    pub signature: Signature,
}

impl CoordinatorCandidateConfig {
    pub fn signed(
        network_id: NetworkId,
        revision: u64,
        mut candidates: Vec<CoordinatorCandidate>,
        effective_from_unix_ms: u64,
        expires_at_unix_ms: u64,
        identity: &Identity,
    ) -> DomainResult<Self> {
        candidates.sort_by(|left, right| {
            left.priority
                .cmp(&right.priority)
                .then_with(|| left.node_id.cmp(&right.node_id))
        });
        let mut config = Self {
            schema_version: COORDINATOR_CANDIDATE_CONFIG_SCHEMA_VERSION,
            network_id,
            revision,
            candidates,
            effective_from_unix_ms,
            expires_at_unix_ms,
            signer: NodeId::from_public_key(&identity.public_key()),
            signer_public_key: PublicKey::from_bytes(identity.public_key_bytes()),
            signature: Signature::from_bytes([0; 64]),
        };
        config.validate_structure()?;
        let signature = identity.sign(&config.signing_payload()?);
        config.signature = Signature::from_bytes(signature.to_bytes());
        config.validate_definition()?;
        Ok(config)
    }

    fn payload(&self) -> CoordinatorCandidateConfigPayload<'_> {
        CoordinatorCandidateConfigPayload {
            schema_version: self.schema_version,
            network_id: &self.network_id,
            revision: self.revision,
            candidates: &self.candidates,
            effective_from_unix_ms: self.effective_from_unix_ms,
            expires_at_unix_ms: self.expires_at_unix_ms,
            signer: &self.signer,
            signer_public_key: &self.signer_public_key,
        }
    }

    /// Domain-separated canonical JSON covered by the administrator signature.
    pub fn signing_payload(&self) -> DomainResult<Vec<u8>> {
        let encoded = serde_json::to_vec(&self.payload()).map_err(|error| {
            DomainError::invalid_value(format!(
                "coordinator candidate config is not serializable: {error}"
            ))
        })?;
        let mut payload =
            Vec::with_capacity(COORDINATOR_CANDIDATE_CONFIG_SIGNING_DOMAIN.len() + encoded.len());
        payload.extend_from_slice(COORDINATOR_CANDIDATE_CONFIG_SIGNING_DOMAIN);
        payload.extend_from_slice(&encoded);
        Ok(payload)
    }

    pub fn verify_signature(&self) -> DomainResult<()> {
        let public_key = self.signer_public_key.to_bytes()?;
        if NodeId::from_public_key(&public_key) != self.signer {
            return Err(DomainError::new(
                "domain.coordinator_candidate_signer_mismatch",
                "candidate configuration signer does not match its public key",
            ));
        }
        let signature = self.signature.to_bytes()?;
        if !Identity::verify(&public_key, &self.signing_payload()?, signature.as_slice()) {
            return Err(DomainError::new(
                "domain.invalid_signature",
                "coordinator candidate configuration signature is invalid",
            ));
        }
        Ok(())
    }

    /// Validate fields that are covered by the administrator signature without
    /// requiring a signature. This is used while constructing a document and
    /// must not be treated as an authority check by adapters.
    pub fn validate_structure(&self) -> DomainResult<()> {
        if self.schema_version != COORDINATOR_CANDIDATE_CONFIG_SCHEMA_VERSION {
            return Err(DomainError::invalid_value(
                "unsupported coordinator candidate configuration schema",
            ));
        }
        if self.revision == 0 {
            return Err(DomainError::invalid_value(
                "coordinator candidate configuration revision must be positive",
            ));
        }
        if self.effective_from_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::invalid_value(
                "coordinator candidate configuration validity interval is empty",
            ));
        }
        if self.candidates.len() > COORDINATOR_CANDIDATE_MAX_GROUP_SIZE {
            return Err(DomainError::invalid_value(format!(
                "coordinator candidate configuration exceeds {COORDINATOR_CANDIDATE_MAX_GROUP_SIZE} candidates"
            )));
        }
        let mut nodes = BTreeSet::new();
        let mut priorities = BTreeSet::new();
        let mut previous_key = None;
        for candidate in &self.candidates {
            if !nodes.insert(candidate.node_id.clone()) {
                return Err(DomainError::invalid_value(
                    "coordinator candidate configuration contains a duplicate node",
                ));
            }
            if !priorities.insert(candidate.priority) {
                return Err(DomainError::invalid_value(
                    "coordinator candidate priorities must be unique",
                ));
            }
            let key = (candidate.priority, candidate.node_id.clone());
            if previous_key
                .as_ref()
                .is_some_and(|previous| previous > &key)
            {
                return Err(DomainError::invalid_value(
                    "coordinator candidates must use canonical priority order",
                ));
            }
            previous_key = Some(key);
        }
        Ok(())
    }

    pub fn validate_definition(&self) -> DomainResult<()> {
        self.validate_structure()?;
        self.verify_signature()
    }

    /// Validate that the signer was a network owner/administrator when the
    /// configuration was admitted and every configured candidate is a member.
    pub fn validate_against_network(&self, network: &NetworkRecord) -> DomainResult<()> {
        self.validate_definition()?;
        if self.network_id != network.network_id {
            return Err(DomainError::new(
                "domain.coordinator_candidate_network_mismatch",
                "candidate configuration belongs to another network",
            ));
        }
        if self.signer != network.owner && !network.administrators.contains(&self.signer) {
            return Err(DomainError::new(
                "domain.coordinator_candidate_permission_denied",
                "candidate configuration signer is not a network owner or administrator",
            ));
        }
        if self
            .candidates
            .iter()
            .any(|candidate| !network.members.contains(&candidate.node_id))
        {
            return Err(DomainError::new(
                "domain.coordinator_candidate_not_member",
                "every coordinator candidate must be a current network member",
            ));
        }
        Ok(())
    }

    pub fn is_effective_at(&self, now_unix_ms: u64) -> bool {
        self.effective_from_unix_ms <= now_unix_ms && now_unix_ms < self.expires_at_unix_ms
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinatorCandidateWillingness {
    Willing,
    Unwilling,
}

/// Public reachability evidence supplied by the candidate itself. Consumers
/// must treat this as a claim until the election protocol verifies a quorum.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct CoordinatorPublicReachability {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_endpoint: Option<SocketAddr>,
    #[serde(default)]
    pub reachable_via_relay: bool,
}

impl CoordinatorPublicReachability {
    pub fn has_usable_path(&self) -> bool {
        self.public_endpoint.is_some() || self.reachable_via_relay
    }

    fn validate(&self) -> DomainResult<()> {
        if self
            .public_endpoint
            .is_some_and(|endpoint| endpoint.ip().is_unspecified() || endpoint.port() == 0)
        {
            return Err(DomainError::invalid_value(
                "coordinator candidate public endpoint is not usable",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct CoordinatorCandidateResources {
    pub cpu_cores: u16,
    pub memory_bytes: u64,
    pub max_connections: u32,
}

impl CoordinatorCandidateResources {
    fn is_sufficient(&self) -> bool {
        self.cpu_cores > 0 && self.memory_bytes >= 64 * 1024 * 1024 && self.max_connections > 0
    }
}

#[derive(Debug, Clone, Serialize)]
struct CoordinatorCandidateDeclarationPayload<'a> {
    schema_version: u16,
    network_id: &'a NetworkId,
    candidate_config_revision: u64,
    node_id: &'a NodeId,
    node_public_key: &'a PublicKey,
    public_reachability: &'a CoordinatorPublicReachability,
    resources: &'a CoordinatorCandidateResources,
    willingness: CoordinatorCandidateWillingness,
    observed_at_unix_ms: u64,
    expires_at_unix_ms: u64,
}

/// A short-lived, node-signed statement used as candidate health/eligibility
/// input. It is never a vote, majority proof or Leader promotion.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct CoordinatorCandidateDeclaration {
    pub schema_version: u16,
    pub network_id: NetworkId,
    pub candidate_config_revision: u64,
    pub node_id: NodeId,
    pub node_public_key: PublicKey,
    pub public_reachability: CoordinatorPublicReachability,
    pub resources: CoordinatorCandidateResources,
    pub willingness: CoordinatorCandidateWillingness,
    pub observed_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    pub signature: Signature,
}

impl CoordinatorCandidateDeclaration {
    #[allow(clippy::too_many_arguments)]
    pub fn signed(
        network_id: NetworkId,
        candidate_config_revision: u64,
        public_reachability: CoordinatorPublicReachability,
        resources: CoordinatorCandidateResources,
        willingness: CoordinatorCandidateWillingness,
        observed_at_unix_ms: u64,
        expires_at_unix_ms: u64,
        identity: &Identity,
    ) -> DomainResult<Self> {
        let mut declaration = Self {
            schema_version: COORDINATOR_CANDIDATE_DECLARATION_SCHEMA_VERSION,
            network_id,
            candidate_config_revision,
            node_id: NodeId::from_public_key(&identity.public_key()),
            node_public_key: PublicKey::from_bytes(identity.public_key_bytes()),
            public_reachability,
            resources,
            willingness,
            observed_at_unix_ms,
            expires_at_unix_ms,
            signature: Signature::from_bytes([0; 64]),
        };
        declaration.validate_structure()?;
        let signature = identity.sign(&declaration.signing_payload()?);
        declaration.signature = Signature::from_bytes(signature.to_bytes());
        declaration.validate_definition()?;
        Ok(declaration)
    }

    fn payload(&self) -> CoordinatorCandidateDeclarationPayload<'_> {
        CoordinatorCandidateDeclarationPayload {
            schema_version: self.schema_version,
            network_id: &self.network_id,
            candidate_config_revision: self.candidate_config_revision,
            node_id: &self.node_id,
            node_public_key: &self.node_public_key,
            public_reachability: &self.public_reachability,
            resources: &self.resources,
            willingness: self.willingness,
            observed_at_unix_ms: self.observed_at_unix_ms,
            expires_at_unix_ms: self.expires_at_unix_ms,
        }
    }

    pub fn signing_payload(&self) -> DomainResult<Vec<u8>> {
        let encoded = serde_json::to_vec(&self.payload()).map_err(|error| {
            DomainError::invalid_value(format!(
                "coordinator candidate declaration is not serializable: {error}"
            ))
        })?;
        let mut payload = Vec::with_capacity(
            COORDINATOR_CANDIDATE_DECLARATION_SIGNING_DOMAIN.len() + encoded.len(),
        );
        payload.extend_from_slice(COORDINATOR_CANDIDATE_DECLARATION_SIGNING_DOMAIN);
        payload.extend_from_slice(&encoded);
        Ok(payload)
    }

    pub fn verify_signature(&self) -> DomainResult<()> {
        let public_key = self.node_public_key.to_bytes()?;
        if NodeId::from_public_key(&public_key) != self.node_id {
            return Err(DomainError::new(
                "domain.coordinator_candidate_node_mismatch",
                "candidate declaration node does not match its public key",
            ));
        }
        let signature = self.signature.to_bytes()?;
        if !Identity::verify(&public_key, &self.signing_payload()?, signature.as_slice()) {
            return Err(DomainError::new(
                "domain.invalid_signature",
                "coordinator candidate declaration signature is invalid",
            ));
        }
        Ok(())
    }

    /// Validate fields that are covered by the node signature without
    /// requiring a signature. This is used while constructing a document.
    pub fn validate_structure(&self) -> DomainResult<()> {
        if self.schema_version != COORDINATOR_CANDIDATE_DECLARATION_SCHEMA_VERSION {
            return Err(DomainError::invalid_value(
                "unsupported coordinator candidate declaration schema",
            ));
        }
        if self.candidate_config_revision == 0 {
            return Err(DomainError::invalid_value(
                "candidate configuration revision must be positive",
            ));
        }
        if self.observed_at_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::invalid_value(
                "candidate declaration validity interval is empty",
            ));
        }
        if self.expires_at_unix_ms - self.observed_at_unix_ms
            > COORDINATOR_CANDIDATE_MAX_DECLARATION_VALIDITY_MS
        {
            return Err(DomainError::invalid_value(
                "candidate declaration validity exceeds the maximum lease",
            ));
        }
        self.public_reachability.validate()?;
        if self.resources.memory_bytes == 0 {
            return Err(DomainError::invalid_value(
                "candidate memory capacity must be positive",
            ));
        }
        Ok(())
    }

    pub fn validate_definition(&self) -> DomainResult<()> {
        self.validate_structure()?;
        self.verify_signature()
    }

    pub fn is_fresh_at(&self, now_unix_ms: u64) -> bool {
        now_unix_ms.saturating_add(COORDINATOR_CANDIDATE_DECLARATION_FUTURE_SKEW_MS)
            >= self.observed_at_unix_ms
            && now_unix_ms < self.expires_at_unix_ms
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinatorCandidateConfigState {
    Missing,
    Pending,
    Current,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinatorCandidateHealth {
    Healthy,
    Degraded,
    Unwilling,
    Stale,
    Missing,
    Ineligible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinatorEmergencyState {
    None,
    NoCandidatesConfigured,
    ConfigurationPending,
    ConfigurationExpired,
    AllCandidatesUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct CoordinatorCandidateStatusEntry {
    pub node_id: NodeId,
    pub priority: u32,
    pub health: CoordinatorCandidateHealth,
    /// Eligibility for the election protocol, never an appointment as Leader.
    pub eligible_for_election: bool,
    pub reason_code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declaration: Option<CoordinatorCandidateDeclaration>,
}

/// Shared UI projection. `latest_term` is retained even after expiry so the UI
/// can show recent tenure without pretending the old leader still has authority.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct CoordinatorCandidateStatus {
    pub network_id: NetworkId,
    pub generated_at_unix_ms: u64,
    pub config_state: CoordinatorCandidateConfigState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<CoordinatorCandidateConfig>,
    pub candidates: Vec<CoordinatorCandidateStatusEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_leader: Option<NodeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_term: Option<CoordinatorTerm>,
    #[serde(default)]
    pub lease: CoordinatorLeaseStatus,
    pub emergency: CoordinatorEmergencyState,
}

impl CoordinatorCandidateStatus {
    pub fn assess(
        network: &NetworkRecord,
        config: Option<&CoordinatorCandidateConfig>,
        declarations: &[CoordinatorCandidateDeclaration],
        latest_term: Option<&CoordinatorTerm>,
        now_unix_ms: u64,
    ) -> DomainResult<Self> {
        if let Some(config) = config {
            config.validate_against_network(network)?;
        }
        let config_state = match config {
            None => CoordinatorCandidateConfigState::Missing,
            Some(config) if now_unix_ms < config.effective_from_unix_ms => {
                CoordinatorCandidateConfigState::Pending
            }
            Some(config) if now_unix_ms >= config.expires_at_unix_ms => {
                CoordinatorCandidateConfigState::Expired
            }
            Some(_) => CoordinatorCandidateConfigState::Current,
        };
        let declarations_by_node = declarations
            .iter()
            .map(|declaration| (declaration.node_id.clone(), declaration))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut candidates = Vec::new();
        if let Some(config) = config {
            for candidate in &config.candidates {
                let declaration = declarations_by_node.get(&candidate.node_id).copied();
                let (health, eligible, reason_code) =
                    if !network.members.contains(&candidate.node_id) {
                        (
                            CoordinatorCandidateHealth::Ineligible,
                            false,
                            "coordinator.candidate.not_member",
                        )
                    } else if config_state != CoordinatorCandidateConfigState::Current {
                        (
                            CoordinatorCandidateHealth::Stale,
                            false,
                            match config_state {
                                CoordinatorCandidateConfigState::Pending => {
                                    "coordinator.candidate.config_pending"
                                }
                                CoordinatorCandidateConfigState::Expired => {
                                    "coordinator.candidate.config_expired"
                                }
                                CoordinatorCandidateConfigState::Missing => {
                                    "coordinator.candidate.config_missing"
                                }
                                CoordinatorCandidateConfigState::Current => unreachable!(),
                            },
                        )
                    } else if let Some(declaration) = declaration {
                        if declaration.validate_definition().is_err() {
                            (
                                CoordinatorCandidateHealth::Ineligible,
                                false,
                                "coordinator.candidate.invalid_declaration",
                            )
                        } else if declaration.network_id != network.network_id
                            || declaration.node_id != candidate.node_id
                            || declaration.candidate_config_revision != config.revision
                        {
                            (
                                CoordinatorCandidateHealth::Stale,
                                false,
                                "coordinator.candidate.declaration_revision_mismatch",
                            )
                        } else if !declaration.is_fresh_at(now_unix_ms) {
                            (
                                CoordinatorCandidateHealth::Stale,
                                false,
                                "coordinator.candidate.declaration_stale",
                            )
                        } else if declaration.willingness
                            == CoordinatorCandidateWillingness::Unwilling
                        {
                            (
                                CoordinatorCandidateHealth::Unwilling,
                                false,
                                "coordinator.candidate.owner_declined",
                            )
                        } else if !declaration.public_reachability.has_usable_path() {
                            (
                                CoordinatorCandidateHealth::Degraded,
                                false,
                                "coordinator.candidate.no_public_path",
                            )
                        } else if !declaration.resources.is_sufficient() {
                            (
                                CoordinatorCandidateHealth::Degraded,
                                false,
                                "coordinator.candidate.resources_insufficient",
                            )
                        } else {
                            (
                                CoordinatorCandidateHealth::Healthy,
                                true,
                                "coordinator.candidate.eligible",
                            )
                        }
                    } else {
                        (
                            CoordinatorCandidateHealth::Missing,
                            false,
                            "coordinator.candidate.declaration_missing",
                        )
                    };
                candidates.push(CoordinatorCandidateStatusEntry {
                    node_id: candidate.node_id.clone(),
                    priority: candidate.priority,
                    health,
                    eligible_for_election: eligible,
                    reason_code: reason_code.into(),
                    declaration: declaration.cloned(),
                });
            }
        }
        let eligible = candidates
            .iter()
            .any(|candidate| candidate.eligible_for_election);
        let emergency = match config_state {
            CoordinatorCandidateConfigState::Missing => {
                CoordinatorEmergencyState::NoCandidatesConfigured
            }
            CoordinatorCandidateConfigState::Pending => {
                CoordinatorEmergencyState::ConfigurationPending
            }
            CoordinatorCandidateConfigState::Expired => {
                CoordinatorEmergencyState::ConfigurationExpired
            }
            CoordinatorCandidateConfigState::Current if candidates.is_empty() => {
                CoordinatorEmergencyState::NoCandidatesConfigured
            }
            CoordinatorCandidateConfigState::Current if !eligible => {
                CoordinatorEmergencyState::AllCandidatesUnavailable
            }
            CoordinatorCandidateConfigState::Current => CoordinatorEmergencyState::None,
        };
        let lease = latest_term
            .map(|term| term.lease_status_at(now_unix_ms))
            .unwrap_or_else(CoordinatorLeaseStatus::no_term);
        let current_leader = latest_term
            .filter(|term| {
                config.is_some_and(|config| term.validate_against_candidate_config(config).is_ok())
                    && config_state == CoordinatorCandidateConfigState::Current
                    && lease.is_writable()
            })
            .map(|term| term.leader.clone());
        Ok(Self {
            network_id: network.network_id.clone(),
            generated_at_unix_ms: now_unix_ms,
            config_state,
            config: config.cloned(),
            candidates,
            current_leader,
            latest_term: latest_term.cloned(),
            lease,
            emergency,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TermFixture {
        network_id: NetworkId,
        candidate_one: Identity,
        candidate_two: Identity,
    }

    impl TermFixture {
        fn new() -> Self {
            Self {
                network_id: NetworkId::from_public_key(b"coordinator-election-test-network"),
                candidate_one: Identity::generate().unwrap(),
                candidate_two: Identity::generate().unwrap(),
            }
        }

        fn candidate_group(&self) -> Vec<NodeId> {
            vec![
                NodeId::from_public_key(&self.candidate_one.public_key()),
                NodeId::from_public_key(&self.candidate_two.public_key()),
            ]
        }

        fn term(&self, term: u64, token: u64, leader: &Identity) -> CoordinatorTerm {
            self.term_with_window(term, token, leader, 10, 1_000)
        }

        fn term_with_window(
            &self,
            term: u64,
            token: u64,
            leader: &Identity,
            issued_at_unix_ms: u64,
            expires_at_unix_ms: u64,
        ) -> CoordinatorTerm {
            let leader = NodeId::from_public_key(&leader.public_key());
            let candidate_group = self.candidate_group();
            let majority_votes = [&self.candidate_one, &self.candidate_two]
                .into_iter()
                .map(|voter| {
                    CoordinatorElectionVote::signed(
                        self.network_id.clone(),
                        1,
                        term,
                        leader.clone(),
                        candidate_group.clone(),
                        issued_at_unix_ms,
                        expires_at_unix_ms,
                        voter,
                    )
                    .unwrap()
                })
                .collect::<Vec<_>>();
            CoordinatorTerm {
                network_id: self.network_id.clone(),
                term,
                leader,
                candidate_group,
                candidate_config_revision: 1,
                majority_proof: majority_votes
                    .iter()
                    .map(|vote| vote.voter.clone())
                    .collect(),
                majority_votes,
                authority: CoordinatorAuthority::Full,
                phase: CoordinatorPhase::Stable,
                fencing_token: FencingToken(token),
                issued_at_unix_ms,
                expires_at_unix_ms,
            }
        }
    }

    #[test]
    fn majority_proof_and_leader_are_checked() {
        let fixture = TermFixture::new();
        let mut valid = fixture.term(1, 1, &fixture.candidate_one);
        valid.validate_at(100).unwrap();
        valid.majority_proof.clear();
        assert!(valid.validate_at(100).is_err());

        let mut invalid = fixture.term(1, 1, &fixture.candidate_one);
        invalid.leader = NodeId::from_public_key(b"outsider");
        assert!(invalid.validate_at(100).is_err());
    }

    #[test]
    fn election_votes_reject_forgery_and_impersonation() {
        let fixture = TermFixture::new();

        let mut forged = fixture.term(1, 1, &fixture.candidate_one);
        forged.majority_votes[0].signature = Signature::from_bytes([0; 64]);
        assert_eq!(
            forged.validate_at(100).unwrap_err().code(),
            "domain.invalid_signature"
        );

        let mut impersonated = fixture.term(1, 1, &fixture.candidate_one);
        impersonated.majority_votes[0].voter_public_key =
            PublicKey::from_bytes(fixture.candidate_two.public_key_bytes());
        assert_eq!(
            impersonated.validate_at(100).unwrap_err().code(),
            "domain.coordinator_election_voter_mismatch"
        );
    }

    #[test]
    fn election_votes_require_unique_voters_and_matching_proof() {
        let fixture = TermFixture::new();

        let mut duplicate = fixture.term(1, 1, &fixture.candidate_one);
        duplicate
            .majority_votes
            .push(duplicate.majority_votes[0].clone());
        assert_eq!(
            duplicate.validate_at(100).unwrap_err().code(),
            "domain.invariant_violation"
        );

        let mut mismatched = fixture.term(1, 1, &fixture.candidate_one);
        mismatched.majority_proof = vec![mismatched.leader.clone()];
        assert_eq!(
            mismatched.validate_at(100).unwrap_err().code(),
            "domain.invariant_violation"
        );

        let mut self_appointed = fixture.term(1, 1, &fixture.candidate_one);
        self_appointed.majority_proof = vec![fixture.candidate_group()[0].clone()];
        self_appointed.majority_votes.remove(1);
        assert_eq!(
            self_appointed.validate_at(100).unwrap_err().code(),
            "domain.invariant_violation"
        );
    }

    #[test]
    fn election_votes_bind_network_revision_term_leader_and_candidate_group() {
        let fixture = TermFixture::new();
        let leader = NodeId::from_public_key(&fixture.candidate_one.public_key());
        let candidate_group = fixture.candidate_group();

        let mut wrong_network = fixture.term(1, 1, &fixture.candidate_one);
        wrong_network.majority_votes[0] = CoordinatorElectionVote::signed(
            NetworkId::from_public_key(b"another-network"),
            1,
            1,
            leader.clone(),
            candidate_group.clone(),
            10,
            1_000,
            &fixture.candidate_one,
        )
        .unwrap();
        assert_eq!(
            wrong_network.validate_at(100).unwrap_err().code(),
            "domain.coordinator_election_vote_mismatch"
        );

        let mut wrong_revision = fixture.term(1, 1, &fixture.candidate_one);
        wrong_revision.majority_votes[0] = CoordinatorElectionVote::signed(
            fixture.network_id.clone(),
            2,
            1,
            leader.clone(),
            candidate_group.clone(),
            10,
            1_000,
            &fixture.candidate_one,
        )
        .unwrap();
        assert_eq!(
            wrong_revision.validate_at(100).unwrap_err().code(),
            "domain.coordinator_election_vote_mismatch"
        );

        let mut wrong_term = fixture.term(1, 1, &fixture.candidate_one);
        wrong_term.majority_votes[0] = CoordinatorElectionVote::signed(
            fixture.network_id.clone(),
            1,
            2,
            leader.clone(),
            candidate_group.clone(),
            10,
            1_000,
            &fixture.candidate_one,
        )
        .unwrap();
        assert_eq!(
            wrong_term.validate_at(100).unwrap_err().code(),
            "domain.coordinator_election_vote_mismatch"
        );

        let other_leader = NodeId::from_public_key(&fixture.candidate_two.public_key());
        let mut wrong_leader = fixture.term(1, 1, &fixture.candidate_one);
        wrong_leader.majority_votes[0] = CoordinatorElectionVote::signed(
            fixture.network_id.clone(),
            1,
            1,
            other_leader,
            candidate_group.clone(),
            10,
            1_000,
            &fixture.candidate_one,
        )
        .unwrap();
        assert_eq!(
            wrong_leader.validate_at(100).unwrap_err().code(),
            "domain.coordinator_election_vote_mismatch"
        );

        let mut wrong_group = fixture.term(1, 1, &fixture.candidate_one);
        wrong_group.majority_votes[0] = CoordinatorElectionVote::signed(
            fixture.network_id.clone(),
            1,
            1,
            leader,
            vec![fixture.candidate_group()[0].clone()],
            10,
            1_000,
            &fixture.candidate_one,
        )
        .unwrap();
        assert_eq!(
            wrong_group.validate_at(100).unwrap_err().code(),
            "domain.coordinator_election_vote_mismatch"
        );
    }

    #[test]
    fn election_vote_must_cover_the_term_lease_and_be_short_lived() {
        let fixture = TermFixture::new();
        let leader = NodeId::from_public_key(&fixture.candidate_one.public_key());
        let candidate_group = fixture.candidate_group();

        let mut expires_early = fixture.term(1, 1, &fixture.candidate_one);
        expires_early.majority_votes[0] = CoordinatorElectionVote::signed(
            fixture.network_id.clone(),
            1,
            1,
            leader.clone(),
            candidate_group.clone(),
            10,
            999,
            &fixture.candidate_one,
        )
        .unwrap();
        assert_eq!(
            expires_early.validate_at(100).unwrap_err().code(),
            "domain.coordinator_election_vote_expiry"
        );

        let mut issued_late = fixture.term(1, 1, &fixture.candidate_one);
        issued_late.majority_votes[0] = CoordinatorElectionVote::signed(
            fixture.network_id.clone(),
            1,
            1,
            leader,
            candidate_group,
            11,
            1_000,
            &fixture.candidate_one,
        )
        .unwrap();
        assert_eq!(
            issued_late.validate_at(100).unwrap_err().code(),
            "domain.coordinator_election_vote_expiry"
        );

        assert_eq!(
            CoordinatorElectionVote::signed(
                fixture.network_id.clone(),
                1,
                1,
                NodeId::from_public_key(&fixture.candidate_one.public_key()),
                fixture.candidate_group(),
                10,
                10 + COORDINATOR_ELECTION_VOTE_MAX_VALIDITY_MS + 1,
                &fixture.candidate_one,
            )
            .unwrap_err()
            .code(),
            "domain.invalid_value"
        );
    }

    #[test]
    fn stale_terms_and_fencing_tokens_are_rejected_after_recovery() {
        let fixture = TermFixture::new();
        let current = fixture.term(3, 30, &fixture.candidate_one);
        let stale = fixture.term(2, 20, &fixture.candidate_one);
        assert_eq!(
            current.validate_successor(&stale, 100).unwrap_err().code(),
            "domain.coordinator_term_rollback"
        );
        let same_term = fixture.term(3, 31, &fixture.candidate_two);
        assert_eq!(
            current
                .validate_successor(&same_term, 100)
                .unwrap_err()
                .code(),
            "domain.coordinator_term_conflict"
        );
        let mut next = fixture.term(4, 30, &fixture.candidate_one);
        assert_eq!(
            current.validate_successor(&next, 100).unwrap_err().code(),
            "domain.fencing_token_rollback"
        );
        next.fencing_token = FencingToken(31);
        current.validate_successor(&next, 100).unwrap();
    }

    #[test]
    fn emergency_leader_cannot_write_governance_or_promote_itself() {
        let fixture = TermFixture::new();
        let mut emergency = fixture.term(4, 40, &fixture.candidate_one);
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
    fn emergency_handoff_keeps_governance_frozen_until_full_successor_recovers() {
        let fixture = TermFixture::new();
        let mut emergency = fixture.term(4, 40, &fixture.candidate_one);
        emergency.authority = CoordinatorAuthority::Emergency;
        emergency.phase = CoordinatorPhase::EmergencyLeader;

        emergency
            .transition_phase(CoordinatorPhase::Handoff, 100)
            .unwrap();
        emergency
            .transition_phase(CoordinatorPhase::Stable, 100)
            .unwrap();
        assert_eq!(emergency.phase, CoordinatorPhase::Stable);
        assert_eq!(emergency.authority, CoordinatorAuthority::Emergency);
        for forbidden in [
            CoordinatorWriteKind::Membership,
            CoordinatorWriteKind::SecurityPolicy,
            CoordinatorWriteKind::Keys,
            CoordinatorWriteKind::AuditDeletion,
            CoordinatorWriteKind::SelfPromotion,
        ] {
            assert!(!emergency.allows_write(forbidden));
        }

        let successor = fixture.term(
            emergency.term + 1,
            emergency.fencing_token.0 + 1,
            &fixture.candidate_one,
        );
        emergency.validate_successor(&successor, 100).unwrap();
        assert_eq!(successor.authority, CoordinatorAuthority::Full);
        assert_eq!(successor.phase, CoordinatorPhase::Stable);
        for recovered in [
            CoordinatorWriteKind::Membership,
            CoordinatorWriteKind::SecurityPolicy,
            CoordinatorWriteKind::Keys,
            CoordinatorWriteKind::AuditDeletion,
        ] {
            assert!(successor.allows_write(recovered));
        }
    }

    #[test]
    fn illegal_phase_transitions_fail_closed() {
        let fixture = TermFixture::new();
        let mut coordinator = fixture.term(1, 1, &fixture.candidate_one);
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
        let fixture = TermFixture::new();
        let coordinator = fixture.term(1, 1, &fixture.candidate_one);
        assert_eq!(
            coordinator.validate_at(1_000).unwrap_err().code(),
            "domain.coordinator_term_expired"
        );
    }

    #[test]
    fn lease_projection_distinguishes_renewal_expiry_and_clock_skew() {
        let fixture = TermFixture::new();
        let coordinator = fixture.term_with_window(1, 1, &fixture.candidate_one, 100_000, 400_000);

        assert_eq!(
            coordinator.lease_status_at(200_000).state,
            CoordinatorLeaseState::Active
        );
        assert_eq!(
            coordinator.lease_status_at(380_000).state,
            CoordinatorLeaseState::RenewalDue
        );
        assert_eq!(
            coordinator.lease_status_at(400_000).state,
            CoordinatorLeaseState::ReadOnly
        );

        let skewed = coordinator.lease_status_at(100_000 - COORDINATOR_CLOCK_SKEW_LIMIT_MS - 1);
        assert_eq!(skewed.state, CoordinatorLeaseState::ReadOnly);
        assert_eq!(skewed.reason_code, "coordinator.lease.clock_skew");
        assert_eq!(
            skewed.observed_clock_skew_ms,
            COORDINATOR_CLOCK_SKEW_LIMIT_MS + 1
        );
        assert!(!skewed.is_writable());
    }

    #[test]
    fn stale_and_expired_write_credentials_fail_closed() {
        let fixture = TermFixture::new();
        let current = fixture.term(3, 30, &fixture.candidate_one);
        let stale = CoordinatorWriteCredential {
            network_id: current.network_id.clone(),
            term: 2,
            leader: current.leader.clone(),
            fencing_token: FencingToken(30),
        };
        assert_eq!(
            current
                .authorize_write(
                    &current.leader,
                    &stale,
                    CoordinatorWriteKind::Membership,
                    100,
                )
                .unwrap_err()
                .code(),
            "domain.coordinator_term_rollback"
        );

        let valid = CoordinatorWriteCredential {
            network_id: current.network_id.clone(),
            term: current.term,
            leader: current.leader.clone(),
            fencing_token: current.fencing_token,
        };
        assert_eq!(
            current
                .authorize_write(
                    &current.leader,
                    &valid,
                    CoordinatorWriteKind::Membership,
                    1_000,
                )
                .unwrap_err()
                .code(),
            "domain.coordinator_lease_expired"
        );
    }

    #[test]
    fn emergency_authority_cannot_use_governance_credential() {
        let fixture = TermFixture::new();
        let mut emergency = fixture.term(4, 40, &fixture.candidate_one);
        let leader = emergency.leader.clone();
        emergency.authority = CoordinatorAuthority::Emergency;
        emergency.phase = CoordinatorPhase::EmergencyLeader;
        let credential = CoordinatorWriteCredential {
            network_id: emergency.network_id.clone(),
            term: emergency.term,
            leader: leader.clone(),
            fencing_token: emergency.fencing_token,
        };

        assert_eq!(
            emergency
                .authorize_write(&leader, &credential, CoordinatorWriteKind::Membership, 100,)
                .unwrap_err()
                .code(),
            "domain.coordinator_transition_read_only"
        );
        assert!(emergency
            .authorize_write(&leader, &credential, CoordinatorWriteKind::HealthCheck, 100,)
            .is_ok());
    }

    struct CandidateFixture {
        network: NetworkRecord,
        owner: Identity,
        administrator: Identity,
        candidate_one: Identity,
        candidate_two: Identity,
        ordinary: Identity,
    }

    fn candidate_fixture() -> CandidateFixture {
        use crate::domain::{
            CapabilityKind, ExitContributionMode, ExitContributionObligation,
            ExitContributionScope, ExitSelectionPolicy, NetworkCreateProfile, NetworkJoinMode,
            NetworkVisibility,
        };

        let owner = Identity::generate().unwrap();
        let administrator = Identity::generate().unwrap();
        let candidate_one = Identity::generate().unwrap();
        let candidate_two = Identity::generate().unwrap();
        let ordinary = Identity::generate().unwrap();
        let owner_id = NodeId::from_public_key(&owner.public_key());
        let administrator_id = NodeId::from_public_key(&administrator.public_key());
        let candidate_one_id = NodeId::from_public_key(&candidate_one.public_key());
        let candidate_two_id = NodeId::from_public_key(&candidate_two.public_key());
        let ordinary_id = NodeId::from_public_key(&ordinary.public_key());
        let profile = NetworkCreateProfile {
            name: "candidate-mesh".into(),
            purpose: "Coordinator candidate contract tests".into(),
            icon: "home".into(),
            language: "zh-CN".into(),
            region: "CN".into(),
            tags: vec!["test".into()],
            capability_declarations: vec![CapabilityKind::Coordinator],
            visibility: NetworkVisibility::Unlisted,
            join_mode: NetworkJoinMode::InviteOnly,
            exit_contribution: ExitContributionObligation::NotRequired,
            exit_contribution_scope: ExitContributionScope::NotApplicable,
            exit_contribution_mode: ExitContributionMode::Disabled,
            exit_selection_policy: ExitSelectionPolicy::default(),
        };
        let mut network =
            NetworkRecord::create_owned(profile, PublicKey::from_bytes([91; 32]), owner_id, 100)
                .unwrap();
        network.administrators.push(administrator_id.clone());
        network.members.extend([
            administrator_id,
            candidate_one_id,
            candidate_two_id,
            ordinary_id,
        ]);
        network.validate().unwrap();
        CandidateFixture {
            network,
            owner,
            administrator,
            candidate_one,
            candidate_two,
            ordinary,
        }
    }

    fn config(
        fixture: &CandidateFixture,
        identity: &Identity,
        candidates: Vec<CoordinatorCandidate>,
    ) -> CoordinatorCandidateConfig {
        CoordinatorCandidateConfig::signed(
            fixture.network.network_id.clone(),
            7,
            candidates,
            1_000,
            10_000,
            identity,
        )
        .unwrap()
    }

    fn declaration(
        fixture: &CandidateFixture,
        candidate: &Identity,
        willingness: CoordinatorCandidateWillingness,
        observed_at_unix_ms: u64,
    ) -> CoordinatorCandidateDeclaration {
        CoordinatorCandidateDeclaration::signed(
            fixture.network.network_id.clone(),
            7,
            CoordinatorPublicReachability {
                public_endpoint: Some("203.0.113.10:443".parse().unwrap()),
                reachable_via_relay: true,
            },
            CoordinatorCandidateResources {
                cpu_cores: 4,
                memory_bytes: 512 * 1024 * 1024,
                max_connections: 1_000,
            },
            willingness,
            observed_at_unix_ms,
            observed_at_unix_ms + 60_000,
            candidate,
        )
        .unwrap()
    }

    #[test]
    fn candidate_config_signatures_are_bound_to_permissions_and_membership() {
        let fixture = candidate_fixture();
        let candidate_one = NodeId::from_public_key(&fixture.candidate_one.public_key());
        let candidate_two = NodeId::from_public_key(&fixture.candidate_two.public_key());
        let signed = config(
            &fixture,
            &fixture.owner,
            vec![
                CoordinatorCandidate {
                    node_id: candidate_one.clone(),
                    priority: 10,
                },
                CoordinatorCandidate {
                    node_id: candidate_two.clone(),
                    priority: 20,
                },
            ],
        );
        signed.validate_against_network(&fixture.network).unwrap();

        let administrator_signed = config(
            &fixture,
            &fixture.administrator,
            vec![CoordinatorCandidate {
                node_id: candidate_one.clone(),
                priority: 10,
            }],
        );
        administrator_signed
            .validate_against_network(&fixture.network)
            .unwrap();

        let non_manager = config(
            &fixture,
            &fixture.candidate_one,
            vec![CoordinatorCandidate {
                node_id: candidate_one.clone(),
                priority: 10,
            }],
        );
        assert_eq!(
            non_manager
                .validate_against_network(&fixture.network)
                .unwrap_err()
                .code(),
            "domain.coordinator_candidate_permission_denied"
        );

        let outsider = NodeId::from_public_key(b"outsider");
        let non_member = config(
            &fixture,
            &fixture.owner,
            vec![CoordinatorCandidate {
                node_id: outsider,
                priority: 10,
            }],
        );
        assert_eq!(
            non_member
                .validate_against_network(&fixture.network)
                .unwrap_err()
                .code(),
            "domain.coordinator_candidate_not_member"
        );

        let mut tampered = signed.clone();
        tampered.revision += 1;
        assert_eq!(
            tampered.validate_definition().unwrap_err().code(),
            "domain.invalid_signature"
        );

        let mut forged = signed.clone();
        forged.signer = candidate_two.clone();
        forged.signer_public_key = PublicKey::from_bytes(fixture.candidate_two.public_key_bytes());
        assert_eq!(
            forged.validate_definition().unwrap_err().code(),
            "domain.invalid_signature"
        );

        let mut mismatched = signed;
        mismatched.signer_public_key =
            PublicKey::from_bytes(fixture.candidate_two.public_key_bytes());
        assert_eq!(
            mismatched.validate_definition().unwrap_err().code(),
            "domain.coordinator_candidate_signer_mismatch"
        );
    }

    #[test]
    fn candidate_config_rejects_duplicates_and_noncanonical_order() {
        let fixture = candidate_fixture();
        let candidate_one = NodeId::from_public_key(&fixture.candidate_one.public_key());
        let candidate_two = NodeId::from_public_key(&fixture.candidate_two.public_key());

        assert!(CoordinatorCandidateConfig::signed(
            fixture.network.network_id.clone(),
            7,
            vec![
                CoordinatorCandidate {
                    node_id: candidate_one.clone(),
                    priority: 10,
                },
                CoordinatorCandidate {
                    node_id: candidate_one.clone(),
                    priority: 20,
                },
            ],
            1_000,
            10_000,
            &fixture.owner,
        )
        .is_err());

        assert!(CoordinatorCandidateConfig::signed(
            fixture.network.network_id.clone(),
            7,
            vec![
                CoordinatorCandidate {
                    node_id: candidate_one.clone(),
                    priority: 10,
                },
                CoordinatorCandidate {
                    node_id: candidate_two.clone(),
                    priority: 10,
                },
            ],
            1_000,
            10_000,
            &fixture.owner,
        )
        .is_err());

        let mut config = config(
            &fixture,
            &fixture.owner,
            vec![
                CoordinatorCandidate {
                    node_id: candidate_one,
                    priority: 10,
                },
                CoordinatorCandidate {
                    node_id: candidate_two,
                    priority: 20,
                },
            ],
        );
        config.candidates.swap(0, 1);
        assert!(config.validate_structure().is_err());
    }

    #[test]
    fn candidate_declarations_are_self_signed_and_revision_bound() {
        let fixture = candidate_fixture();
        let candidate_one = NodeId::from_public_key(&fixture.candidate_one.public_key());
        let candidate_two = NodeId::from_public_key(&fixture.candidate_two.public_key());
        let config = config(
            &fixture,
            &fixture.owner,
            vec![CoordinatorCandidate {
                node_id: candidate_one.clone(),
                priority: 10,
            }],
        );
        let signed = declaration(
            &fixture,
            &fixture.candidate_one,
            CoordinatorCandidateWillingness::Willing,
            2_000,
        );
        signed.validate_definition().unwrap();
        assert!(signed.is_fresh_at(2_000));
        assert!(!signed.is_fresh_at(2_000 + 61_000));

        let mut tampered = signed.clone();
        tampered.resources.max_connections += 1;
        assert_eq!(
            tampered.validate_definition().unwrap_err().code(),
            "domain.invalid_signature"
        );

        let mut impersonated = signed;
        impersonated.node_id = candidate_two.clone();
        assert_eq!(
            impersonated.validate_definition().unwrap_err().code(),
            "domain.coordinator_candidate_node_mismatch"
        );

        assert!(CoordinatorCandidateDeclaration::signed(
            fixture.network.network_id.clone(),
            7,
            CoordinatorPublicReachability {
                public_endpoint: None,
                reachable_via_relay: true,
            },
            CoordinatorCandidateResources {
                cpu_cores: 1,
                memory_bytes: 64 * 1024 * 1024,
                max_connections: 1,
            },
            CoordinatorCandidateWillingness::Willing,
            2_000,
            2_000,
            &fixture.candidate_one,
        )
        .is_err());
        assert!(CoordinatorCandidateDeclaration::signed(
            fixture.network.network_id.clone(),
            7,
            CoordinatorPublicReachability {
                public_endpoint: None,
                reachable_via_relay: true,
            },
            CoordinatorCandidateResources {
                cpu_cores: 1,
                memory_bytes: 64 * 1024 * 1024,
                max_connections: 1,
            },
            CoordinatorCandidateWillingness::Willing,
            2_000,
            2_000 + COORDINATOR_CANDIDATE_MAX_DECLARATION_VALIDITY_MS + 1,
            &fixture.candidate_one,
        )
        .is_err());

        let mut stale = declaration(
            &fixture,
            &fixture.candidate_one,
            CoordinatorCandidateWillingness::Willing,
            2_000,
        );
        stale.candidate_config_revision += 1;
        stale.signature = Signature::from_bytes(
            fixture
                .candidate_one
                .sign(&stale.signing_payload().unwrap())
                .to_bytes(),
        );
        let status = CoordinatorCandidateStatus::assess(
            &fixture.network,
            Some(&config),
            &[stale],
            None,
            3_000,
        )
        .unwrap();
        assert_eq!(
            status.candidates[0].reason_code,
            "coordinator.candidate.declaration_revision_mismatch"
        );
        assert!(!status.candidates[0].eligible_for_election);
    }

    #[test]
    fn unwilling_or_missing_candidates_never_gain_election_eligibility() {
        let fixture = candidate_fixture();
        let candidate_one = NodeId::from_public_key(&fixture.candidate_one.public_key());
        let candidate_two = NodeId::from_public_key(&fixture.candidate_two.public_key());
        let ordinary = NodeId::from_public_key(&fixture.ordinary.public_key());
        let config = config(
            &fixture,
            &fixture.owner,
            vec![
                CoordinatorCandidate {
                    node_id: candidate_one.clone(),
                    priority: 10,
                },
                CoordinatorCandidate {
                    node_id: candidate_two,
                    priority: 20,
                },
            ],
        );
        let unwilling = declaration(
            &fixture,
            &fixture.candidate_one,
            CoordinatorCandidateWillingness::Unwilling,
            2_000,
        );
        let ordinary_declaration = declaration(
            &fixture,
            &fixture.ordinary,
            CoordinatorCandidateWillingness::Willing,
            2_000,
        );
        let status = CoordinatorCandidateStatus::assess(
            &fixture.network,
            Some(&config),
            &[unwilling, ordinary_declaration],
            None,
            3_000,
        )
        .unwrap();
        assert_eq!(
            status.emergency,
            CoordinatorEmergencyState::AllCandidatesUnavailable
        );
        assert_eq!(
            status.candidates[0].health,
            CoordinatorCandidateHealth::Unwilling
        );
        assert!(!status.candidates[0].eligible_for_election);
        assert_eq!(
            status.candidates[1].health,
            CoordinatorCandidateHealth::Missing
        );
        assert!(!status
            .candidates
            .iter()
            .any(|candidate| candidate.node_id == ordinary));
    }

    #[test]
    fn healthy_candidate_is_eligible_but_only_term_validation_establishes_leader() {
        let fixture = candidate_fixture();
        let candidate_one = NodeId::from_public_key(&fixture.candidate_one.public_key());
        let config = config(
            &fixture,
            &fixture.owner,
            vec![CoordinatorCandidate {
                node_id: candidate_one.clone(),
                priority: 10,
            }],
        );
        let declaration = declaration(
            &fixture,
            &fixture.candidate_one,
            CoordinatorCandidateWillingness::Willing,
            2_000,
        );
        let vote = CoordinatorElectionVote::signed(
            fixture.network.network_id.clone(),
            1,
            4,
            candidate_one.clone(),
            vec![candidate_one.clone()],
            1_000,
            2_500,
            &fixture.candidate_one,
        )
        .unwrap();
        let latest_term = CoordinatorTerm {
            network_id: fixture.network.network_id.clone(),
            term: 4,
            leader: candidate_one.clone(),
            candidate_group: vec![candidate_one.clone()],
            candidate_config_revision: 1,
            majority_proof: vec![candidate_one],
            majority_votes: vec![vote],
            authority: CoordinatorAuthority::Full,
            phase: CoordinatorPhase::Stable,
            fencing_token: FencingToken(40),
            issued_at_unix_ms: 1_000,
            expires_at_unix_ms: 2_500,
        };
        let status = CoordinatorCandidateStatus::assess(
            &fixture.network,
            Some(&config),
            &[declaration],
            Some(&latest_term),
            3_000,
        )
        .unwrap();
        assert_eq!(status.emergency, CoordinatorEmergencyState::None);
        assert!(status.candidates[0].eligible_for_election);
        assert_eq!(status.current_leader, None);
        assert_eq!(status.latest_term, Some(latest_term));
    }
}
