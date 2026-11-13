use std::collections::BTreeSet;
use std::net::SocketAddr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ids::{ContentHash, NetworkId, NodeId, PublicKey, Signature};
use super::network::NetworkRecord;
use super::{AuditEntry, DomainError, DomainResult};
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
pub const COORDINATOR_ELECTION_PROPOSAL_SCHEMA_VERSION: u16 = 1;
pub const COORDINATOR_ELECTION_SESSION_SCHEMA_VERSION: u16 = 1;

const COORDINATOR_ELECTION_VOTE_SIGNING_DOMAIN: &[u8] = b"fuckipv6.coordinator-election-vote.v1\0";
const COORDINATOR_ELECTION_PROPOSAL_SIGNING_DOMAIN: &[u8] =
    b"fuckipv6.coordinator-election-proposal.v1\0";

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
    #[serde(default)]
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

    pub fn validate_for_proposal(
        &self,
        proposal: &CoordinatorElectionProposal,
    ) -> DomainResult<()> {
        self.validate_definition()?;
        proposal.validate_definition()?;
        if self.network_id != proposal.network_id
            || self.candidate_config_revision != proposal.candidate_config_revision
            || self.term != proposal.term
            || self.leader != proposal.leader
            || self.candidate_group.iter().collect::<BTreeSet<_>>()
                != proposal.candidate_group.iter().collect::<BTreeSet<_>>()
        {
            return Err(DomainError::new(
                "domain.coordinator_election_vote_mismatch",
                "coordinator election vote does not match the proposal",
            ));
        }
        if self.issued_at_unix_ms < proposal.proposed_at_unix_ms
            || self.expires_at_unix_ms < proposal.expires_at_unix_ms
        {
            return Err(DomainError::new(
                "domain.coordinator_election_vote_expiry",
                "coordinator election vote does not cover the proposal window",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
struct CoordinatorElectionProposalPayload<'a> {
    schema_version: u16,
    network_id: &'a NetworkId,
    candidate_config_revision: u64,
    term: u64,
    leader: &'a NodeId,
    candidate_group: &'a [NodeId],
    proposer: &'a NodeId,
    proposer_public_key: &'a PublicKey,
    proposed_at_unix_ms: u64,
    expires_at_unix_ms: u64,
}

/// A candidate-signed request to elect one deterministic Leader. A proposal is
/// not authority: it can only create a persisted election session and the
/// resulting term is admitted after independently verified strict-majority
/// votes. The proposer must be the preferred eligible candidate and cannot
/// appoint another node or itself outside the configured ordering.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct CoordinatorElectionProposal {
    pub schema_version: u16,
    pub proposal_id: ContentHash,
    pub network_id: NetworkId,
    pub candidate_config_revision: u64,
    pub term: u64,
    pub leader: NodeId,
    pub candidate_group: Vec<NodeId>,
    pub proposer: NodeId,
    pub proposer_public_key: PublicKey,
    pub proposed_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    pub signature: Signature,
}

impl CoordinatorElectionProposal {
    #[allow(clippy::too_many_arguments)]
    pub fn signed(
        network_id: NetworkId,
        candidate_config_revision: u64,
        term: u64,
        mut candidate_group: Vec<NodeId>,
        proposed_at_unix_ms: u64,
        expires_at_unix_ms: u64,
        identity: &Identity,
    ) -> DomainResult<Self> {
        candidate_group.sort();
        let proposer = NodeId::from_public_key(&identity.public_key());
        let mut proposal = Self {
            schema_version: COORDINATOR_ELECTION_PROPOSAL_SCHEMA_VERSION,
            proposal_id: ContentHash::from_bytes(b"pending-coordinator-election-proposal"),
            network_id,
            candidate_config_revision,
            term,
            leader: proposer.clone(),
            candidate_group,
            proposer,
            proposer_public_key: PublicKey::from_bytes(identity.public_key_bytes()),
            proposed_at_unix_ms,
            expires_at_unix_ms,
            signature: Signature::from_bytes([0; 64]),
        };
        proposal.validate_structure()?;
        let signature = identity.sign(&proposal.signing_payload()?);
        let signature = Signature::from_bytes(signature.to_bytes());
        proposal.proposal_id = ContentHash::from_bytes(&signature.to_bytes()?);
        proposal.signature = signature;
        proposal.validate_definition()?;
        Ok(proposal)
    }

    fn payload(&self) -> CoordinatorElectionProposalPayload<'_> {
        CoordinatorElectionProposalPayload {
            schema_version: self.schema_version,
            network_id: &self.network_id,
            candidate_config_revision: self.candidate_config_revision,
            term: self.term,
            leader: &self.leader,
            candidate_group: &self.candidate_group,
            proposer: &self.proposer,
            proposer_public_key: &self.proposer_public_key,
            proposed_at_unix_ms: self.proposed_at_unix_ms,
            expires_at_unix_ms: self.expires_at_unix_ms,
        }
    }

    pub fn signing_payload(&self) -> DomainResult<Vec<u8>> {
        let encoded = serde_json::to_vec(&self.payload()).map_err(|error| {
            DomainError::invalid_value(format!(
                "coordinator election proposal is not serializable: {error}"
            ))
        })?;
        let mut payload =
            Vec::with_capacity(COORDINATOR_ELECTION_PROPOSAL_SIGNING_DOMAIN.len() + encoded.len());
        payload.extend_from_slice(COORDINATOR_ELECTION_PROPOSAL_SIGNING_DOMAIN);
        payload.extend_from_slice(&encoded);
        Ok(payload)
    }

    pub fn validate_structure(&self) -> DomainResult<()> {
        if self.schema_version != COORDINATOR_ELECTION_PROPOSAL_SCHEMA_VERSION {
            return Err(DomainError::invalid_value(
                "unsupported coordinator election proposal schema",
            ));
        }
        if self.candidate_config_revision == 0 || self.term == 0 {
            return Err(DomainError::invalid_value(
                "coordinator election proposal revision and term must be positive",
            ));
        }
        if self.proposed_at_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::invalid_value(
                "coordinator election proposal validity interval is empty",
            ));
        }
        if self.expires_at_unix_ms - self.proposed_at_unix_ms
            > COORDINATOR_ELECTION_VOTE_MAX_VALIDITY_MS
        {
            return Err(DomainError::invalid_value(
                "coordinator election proposal validity exceeds the maximum lease",
            ));
        }
        let candidates = self.candidate_group.iter().collect::<BTreeSet<_>>();
        if candidates.is_empty() || candidates.len() != self.candidate_group.len() {
            return Err(DomainError::invalid_value(
                "coordinator election proposal candidate group must be non-empty and unique",
            ));
        }
        if !candidates.contains(&self.leader) || self.leader != self.proposer {
            return Err(DomainError::new(
                "domain.coordinator_election_self_appointment",
                "coordinator election proposer must be the proposed Leader",
            ));
        }
        Ok(())
    }

    pub fn verify_signature(&self) -> DomainResult<()> {
        let public_key = self.proposer_public_key.to_bytes()?;
        if NodeId::from_public_key(&public_key) != self.proposer {
            return Err(DomainError::new(
                "domain.coordinator_election_proposer_mismatch",
                "coordinator election proposer does not match its public key",
            ));
        }
        let signature = self.signature.to_bytes()?;
        if !Identity::verify(&public_key, &self.signing_payload()?, signature.as_slice()) {
            return Err(DomainError::new(
                "domain.invalid_signature",
                "coordinator election proposal signature is invalid",
            ));
        }
        if self.proposal_id != ContentHash::from_bytes(&signature) {
            return Err(DomainError::new(
                "domain.coordinator_election_proposal_id_mismatch",
                "coordinator election proposal ID does not match its signature",
            ));
        }
        Ok(())
    }

    pub fn validate_definition(&self) -> DomainResult<()> {
        self.validate_structure()?;
        self.verify_signature()
    }

    pub fn validate_at(&self, now_unix_ms: u64) -> DomainResult<()> {
        self.validate_definition()?;
        if now_unix_ms < self.proposed_at_unix_ms || now_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::new(
                "domain.coordinator_election_proposal_expired",
                "coordinator election proposal is not currently valid",
            ));
        }
        Ok(())
    }

    pub fn majority_threshold(&self) -> usize {
        self.candidate_group.len() / 2 + 1
    }

    /// Validate a proposal against the Coordinator's authoritative candidate
    /// status projection. Runtimes use this before signing a vote so local or
    /// stale network snapshots cannot nominate an ineligible Leader.
    pub fn validate_for_candidate_status(
        &self,
        status: &CoordinatorCandidateStatus,
        now_unix_ms: u64,
    ) -> DomainResult<()> {
        self.validate_at(now_unix_ms)?;
        if status.network_id != self.network_id
            || status.config_state != CoordinatorCandidateConfigState::Current
        {
            return Err(DomainError::new(
                "domain.coordinator_election_candidate_group_mismatch",
                "coordinator election proposal does not bind the current candidate group",
            ));
        }
        let config = status
            .config
            .as_ref()
            .ok_or_else(|| DomainError::invariant("current candidate status has no config"))?;
        if self.candidate_config_revision != config.revision
            || self.candidate_group.len() != config.candidates.len()
            || self.candidate_group.iter().collect::<BTreeSet<_>>()
                != config
                    .candidates
                    .iter()
                    .map(|candidate| &candidate.node_id)
                    .collect::<BTreeSet<_>>()
        {
            return Err(DomainError::new(
                "domain.coordinator_election_candidate_group_mismatch",
                "coordinator election proposal does not bind the active candidate group",
            ));
        }

        let expected_term = match &status.latest_term {
            Some(term) => {
                term.validate_definition()?;
                if term.network_id != self.network_id {
                    return Err(DomainError::new(
                        "domain.coordinator_election_term_mismatch",
                        "coordinator election proposal conflicts with another network term",
                    ));
                }
                if term.lease_status_at(now_unix_ms).is_writable() {
                    return Err(DomainError::new(
                        "domain.coordinator_election_lease_active",
                        "coordinator election cannot start while the current lease is writable",
                    ));
                }
                term.term
                    .checked_add(1)
                    .ok_or_else(|| DomainError::invariant("coordinator term overflow"))?
            }
            None => 1,
        };
        if self.term != expected_term {
            return Err(DomainError::new(
                "domain.coordinator_election_term_not_next",
                "coordinator election term must be the next durable high-water mark",
            ));
        }

        let Some(preferred) = status.preferred_election_candidate() else {
            return Err(DomainError::new(
                "domain.coordinator_election_no_eligible_candidate",
                "coordinator election has no healthy willing candidate",
            ));
        };
        if self.proposer != preferred.node_id || self.leader != preferred.node_id {
            return Err(DomainError::new(
                "domain.coordinator_election_leader_not_preferred",
                "coordinator election Leader is not the preferred eligible candidate",
            ));
        }
        Ok(())
    }

    /// Validate the proposal against the active administrator-signed candidate
    /// group and current health evidence. A new election is allowed only after
    /// the current lease is no longer writable, and the old Leader is excluded
    /// from preferred-candidate selection at that point.
    pub fn validate_for_election(
        &self,
        network: &NetworkRecord,
        config: &CoordinatorCandidateConfig,
        declarations: &[CoordinatorCandidateDeclaration],
        latest_term: Option<&CoordinatorTerm>,
        now_unix_ms: u64,
    ) -> DomainResult<()> {
        self.validate_at(now_unix_ms)?;
        config.validate_against_network(network)?;
        if self.network_id != config.network_id
            || self.candidate_config_revision != config.revision
            || self.candidate_group.len() != config.candidates.len()
            || self.candidate_group.iter().collect::<BTreeSet<_>>()
                != config
                    .candidates
                    .iter()
                    .map(|candidate| &candidate.node_id)
                    .collect::<BTreeSet<_>>()
        {
            return Err(DomainError::new(
                "domain.coordinator_election_candidate_group_mismatch",
                "coordinator election proposal does not bind the active candidate group",
            ));
        }

        let expected_term = match latest_term {
            Some(term) => {
                term.validate_definition()?;
                if term.network_id != self.network_id {
                    return Err(DomainError::new(
                        "domain.coordinator_election_term_mismatch",
                        "coordinator election proposal conflicts with another network term",
                    ));
                }
                if term.lease_status_at(now_unix_ms).is_writable() {
                    return Err(DomainError::new(
                        "domain.coordinator_election_lease_active",
                        "coordinator election cannot start while the current lease is writable",
                    ));
                }
                term.term
                    .checked_add(1)
                    .ok_or_else(|| DomainError::invariant("coordinator term overflow"))?
            }
            None => 1,
        };
        if self.term != expected_term {
            return Err(DomainError::new(
                "domain.coordinator_election_term_not_next",
                "coordinator election term must be the next durable high-water mark",
            ));
        }

        let status = CoordinatorCandidateStatus::assess(
            network,
            Some(config),
            declarations,
            latest_term,
            now_unix_ms,
        )?;
        self.validate_for_status(&status, now_unix_ms)
    }

    /// Validate a proposal against the authoritative status projection. This is
    /// the same decision used by runtimes before they sign a vote or start an
    /// election; the Coordinator still revalidates the live store and health
    /// evidence before admitting the request.
    pub fn validate_for_status(
        &self,
        status: &CoordinatorCandidateStatus,
        now_unix_ms: u64,
    ) -> DomainResult<()> {
        self.validate_at(now_unix_ms)?;
        if status.network_id != self.network_id
            || status.config_state != CoordinatorCandidateConfigState::Current
        {
            return Err(DomainError::new(
                "domain.coordinator_election_candidate_group_mismatch",
                "coordinator election proposal does not bind the current candidate group",
            ));
        }
        let config = status
            .config
            .as_ref()
            .ok_or_else(|| DomainError::invariant("current candidate status has no config"))?;
        if self.candidate_config_revision != config.revision
            || self.candidate_group.len() != config.candidates.len()
            || self.candidate_group.iter().collect::<BTreeSet<_>>()
                != config
                    .candidates
                    .iter()
                    .map(|candidate| &candidate.node_id)
                    .collect::<BTreeSet<_>>()
        {
            return Err(DomainError::new(
                "domain.coordinator_election_candidate_group_mismatch",
                "coordinator election proposal does not bind the active candidate group",
            ));
        }

        let expected_term = match &status.latest_term {
            Some(term) => {
                term.validate_definition()?;
                if term.network_id != self.network_id {
                    return Err(DomainError::new(
                        "domain.coordinator_election_term_mismatch",
                        "coordinator election proposal conflicts with another network term",
                    ));
                }
                if term.lease_status_at(now_unix_ms).is_writable() {
                    return Err(DomainError::new(
                        "domain.coordinator_election_lease_active",
                        "coordinator election cannot start while the current lease is writable",
                    ));
                }
                term.term
                    .checked_add(1)
                    .ok_or_else(|| DomainError::invariant("coordinator term overflow"))?
            }
            None => 1,
        };
        if self.term != expected_term {
            return Err(DomainError::new(
                "domain.coordinator_election_term_not_next",
                "coordinator election term must be the next durable high-water mark",
            ));
        }

        let Some(preferred) = status.preferred_election_candidate() else {
            return Err(DomainError::new(
                "domain.coordinator_election_no_eligible_candidate",
                "coordinator election has no healthy willing candidate",
            ));
        };
        if self.leader != preferred.node_id {
            return Err(DomainError::new(
                "domain.coordinator_election_leader_not_preferred",
                "coordinator election Leader is not the preferred eligible candidate",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinatorElectionStage {
    Started,
    Voting,
    Committed,
    TimedOut,
    Rejected,
}

/// Durable election state. Votes are retained for audit and the committed term
/// is reconstructed from this exact session, preventing a later caller from
/// substituting a different proof or Leader.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct CoordinatorElectionSession {
    pub schema_version: u16,
    pub proposal: CoordinatorElectionProposal,
    pub stage: CoordinatorElectionStage,
    pub participants: Vec<NodeId>,
    pub votes: Vec<CoordinatorElectionVote>,
    pub started_at_unix_ms: u64,
    pub deadline_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    #[serde(default)]
    pub result_term: Option<CoordinatorTerm>,
    #[serde(default)]
    pub rejection_reason: Option<String>,
}

impl CoordinatorElectionSession {
    pub fn started(proposal: CoordinatorElectionProposal, now_unix_ms: u64) -> DomainResult<Self> {
        proposal.validate_at(now_unix_ms)?;
        let participants = proposal.candidate_group.clone();
        let deadline_unix_ms = proposal.expires_at_unix_ms;
        Ok(Self {
            schema_version: COORDINATOR_ELECTION_SESSION_SCHEMA_VERSION,
            proposal,
            stage: CoordinatorElectionStage::Started,
            participants,
            votes: Vec::new(),
            started_at_unix_ms: now_unix_ms,
            deadline_unix_ms,
            updated_at_unix_ms: now_unix_ms,
            result_term: None,
            rejection_reason: None,
        })
    }

    pub fn validate_definition(&self) -> DomainResult<()> {
        if self.schema_version != COORDINATOR_ELECTION_SESSION_SCHEMA_VERSION {
            return Err(DomainError::invalid_value(
                "unsupported coordinator election session schema",
            ));
        }
        self.proposal.validate_definition()?;
        if self.participants.iter().collect::<BTreeSet<_>>()
            != self
                .proposal
                .candidate_group
                .iter()
                .collect::<BTreeSet<_>>()
            || self.participants.len() != self.proposal.candidate_group.len()
        {
            return Err(DomainError::invariant(
                "coordinator election participants do not match the proposal",
            ));
        }
        if self.started_at_unix_ms >= self.deadline_unix_ms
            || self.deadline_unix_ms > self.proposal.expires_at_unix_ms
        {
            return Err(DomainError::invariant(
                "coordinator election session deadline is invalid",
            ));
        }
        let mut voters = BTreeSet::new();
        for vote in &self.votes {
            vote.validate_for_proposal(&self.proposal)?;
            if !voters.insert(vote.voter.clone()) {
                return Err(DomainError::invariant(
                    "coordinator election session contains duplicate voters",
                ));
            }
        }
        match self.stage {
            CoordinatorElectionStage::Committed => {
                let term = self
                    .result_term
                    .as_ref()
                    .ok_or_else(|| DomainError::invariant("committed election has no term"))?;
                term.validate_definition()?;
                if voters.len() < self.proposal.majority_threshold()
                    || term.network_id != self.proposal.network_id
                    || term.term != self.proposal.term
                    || term.leader != self.proposal.leader
                    || term.majority_proof
                        != self
                            .votes
                            .iter()
                            .map(|vote| vote.voter.clone())
                            .collect::<Vec<_>>()
                {
                    return Err(DomainError::invariant(
                        "committed election term does not match its quorum",
                    ));
                }
            }
            CoordinatorElectionStage::Started
            | CoordinatorElectionStage::Voting
            | CoordinatorElectionStage::TimedOut
            | CoordinatorElectionStage::Rejected => {
                if self.result_term.is_some() {
                    return Err(DomainError::invariant(
                        "uncommitted election cannot contain a result term",
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn begin_voting(&mut self, now_unix_ms: u64) -> DomainResult<()> {
        self.validate_definition()?;
        if self.stage != CoordinatorElectionStage::Started {
            return Err(DomainError::new(
                "domain.coordinator_election_invalid_stage",
                "coordinator election is not in the started stage",
            ));
        }
        self.stage = CoordinatorElectionStage::Voting;
        self.updated_at_unix_ms = now_unix_ms;
        self.validate_definition()
    }

    /// Record one signed vote. Repeating the exact same vote is idempotent;
    /// replacing a voter's choice, voting after the deadline, or voting in a
    /// terminal stage is rejected.
    pub fn record_vote(
        &mut self,
        vote: CoordinatorElectionVote,
        now_unix_ms: u64,
    ) -> DomainResult<bool> {
        self.validate_definition()?;
        if let Some(existing) = self
            .votes
            .iter()
            .find(|existing| existing.voter == vote.voter)
        {
            if existing == &vote {
                return Ok(false);
            }
            return Err(DomainError::new(
                "domain.coordinator_election_conflicting_vote",
                "coordinator election voter already selected another Leader",
            ));
        }
        if self.stage != CoordinatorElectionStage::Voting {
            return Err(DomainError::new(
                "domain.coordinator_election_invalid_stage",
                "coordinator election is not accepting votes",
            ));
        }
        if now_unix_ms >= self.deadline_unix_ms {
            return Err(DomainError::new(
                "domain.coordinator_election_expired",
                "coordinator election deadline has passed",
            ));
        }
        vote.validate_for_proposal(&self.proposal)?;
        if vote.issued_at_unix_ms > now_unix_ms.saturating_add(COORDINATOR_CLOCK_SKEW_LIMIT_MS) {
            return Err(DomainError::new(
                "domain.coordinator_election_vote_from_future",
                "coordinator election vote was issued too far in the future",
            ));
        }
        if now_unix_ms >= vote.expires_at_unix_ms {
            return Err(DomainError::new(
                "domain.coordinator_election_vote_expired",
                "coordinator election vote has expired",
            ));
        }
        self.votes.push(vote);
        self.votes
            .sort_by(|left, right| left.voter.cmp(&right.voter));
        self.updated_at_unix_ms = now_unix_ms;
        self.validate_definition()?;
        Ok(true)
    }

    /// Commit only with a strict majority. The resulting term lease is bounded
    /// by the proposal window and every retained vote must cover that window.
    pub fn commit(
        &mut self,
        fencing_token: FencingToken,
        now_unix_ms: u64,
    ) -> DomainResult<CoordinatorTerm> {
        self.validate_definition()?;
        if self.stage != CoordinatorElectionStage::Voting {
            return Err(DomainError::new(
                "domain.coordinator_election_invalid_stage",
                "coordinator election is not in the voting stage",
            ));
        }
        self.proposal.validate_at(now_unix_ms)?;
        if self.votes.len() < self.proposal.majority_threshold() {
            return Err(DomainError::new(
                "domain.coordinator_election_no_majority",
                "coordinator election has not reached a strict majority",
            ));
        }
        if fencing_token.0 == 0 {
            return Err(DomainError::invalid_value(
                "coordinator election fencing token must be positive",
            ));
        }
        let term = CoordinatorTerm {
            network_id: self.proposal.network_id.clone(),
            term: self.proposal.term,
            leader: self.proposal.leader.clone(),
            candidate_group: self.proposal.candidate_group.clone(),
            candidate_config_revision: self.proposal.candidate_config_revision,
            majority_proof: self.votes.iter().map(|vote| vote.voter.clone()).collect(),
            majority_votes: self.votes.clone(),
            authority: CoordinatorAuthority::Full,
            phase: CoordinatorPhase::Stable,
            fencing_token,
            issued_at_unix_ms: now_unix_ms,
            expires_at_unix_ms: self.deadline_unix_ms,
        };
        term.validate_definition()?;
        self.stage = CoordinatorElectionStage::Committed;
        self.result_term = Some(term.clone());
        self.updated_at_unix_ms = now_unix_ms;
        self.validate_definition()?;
        Ok(term)
    }

    pub fn timed_out(&mut self, now_unix_ms: u64) -> DomainResult<()> {
        self.validate_definition()?;
        if self.stage != CoordinatorElectionStage::Voting || now_unix_ms < self.deadline_unix_ms {
            return Err(DomainError::new(
                "domain.coordinator_election_not_timed_out",
                "coordinator election is not eligible to time out",
            ));
        }
        self.stage = CoordinatorElectionStage::TimedOut;
        self.updated_at_unix_ms = now_unix_ms;
        self.validate_definition()
    }

    pub fn reject(&mut self, reason: impl Into<String>, now_unix_ms: u64) -> DomainResult<()> {
        self.validate_definition()?;
        if matches!(
            self.stage,
            CoordinatorElectionStage::Committed
                | CoordinatorElectionStage::TimedOut
                | CoordinatorElectionStage::Rejected
        ) {
            return Err(DomainError::new(
                "domain.coordinator_election_invalid_stage",
                "coordinator election is already terminal",
            ));
        }
        self.stage = CoordinatorElectionStage::Rejected;
        self.rejection_reason = Some(reason.into());
        self.updated_at_unix_ms = now_unix_ms;
        self.validate_definition()
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
    #[serde(default)]
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
    #[serde(default)]
    pub declaration: Option<CoordinatorCandidateDeclaration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinatorControlPlaneState {
    /// A full-authority Stable term has a writable lease.
    Available,
    /// An election is active but has not yet committed a term.
    ElectionInProgress,
    /// The election ended without a strict majority, or its deadline elapsed
    /// before the durable term could be installed.
    QuorumUnavailable,
    /// The candidate configuration is current, but no term is writable.
    NoLeader,
    /// The candidate configuration is missing, pending, or expired.
    ConfigurationUnavailable,
    /// Authority is present but cannot safely perform governance writes.
    ReadOnly,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct CoordinatorQuorumStatus {
    #[serde(with = "usize_as_u64")]
    #[schemars(with = "u64")]
    pub received_votes: usize,
    #[serde(with = "usize_as_u64")]
    #[schemars(with = "u64")]
    pub required_votes: usize,
    #[serde(with = "usize_as_u64")]
    #[schemars(with = "u64")]
    pub participant_count: usize,
}

mod usize_as_u64 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &usize, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(*value as u64)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<usize, D::Error>
    where
        D: Deserializer<'de>,
    {
        usize::try_from(u64::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// Fail-closed projection intended for clients and operators. A missing leader
/// is not a data-plane failure: `governance_writable = false` only pauses
/// governance until a strict-majority election installs a fresh term.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct CoordinatorControlPlaneStatus {
    pub state: CoordinatorControlPlaneState,
    pub governance_writable: bool,
    pub reason_code: String,
    pub quorum: CoordinatorQuorumStatus,
}

impl CoordinatorControlPlaneStatus {
    pub fn is_available(&self) -> bool {
        self.state == CoordinatorControlPlaneState::Available
    }
}

impl Default for CoordinatorControlPlaneStatus {
    fn default() -> Self {
        Self {
            // Old persisted/client projections did not carry this field. Never
            // infer writable authority from its absence.
            state: CoordinatorControlPlaneState::ReadOnly,
            governance_writable: false,
            reason_code: "coordinator.control_plane.unobserved".into(),
            quorum: CoordinatorQuorumStatus::default(),
        }
    }
}

/// Shared UI projection. `latest_term` is retained even after expiry so the UI
/// can show recent tenure without pretending the old leader still has authority.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct CoordinatorCandidateStatus {
    pub network_id: NetworkId,
    pub generated_at_unix_ms: u64,
    pub config_state: CoordinatorCandidateConfigState,
    #[serde(default)]
    pub config: Option<CoordinatorCandidateConfig>,
    pub candidates: Vec<CoordinatorCandidateStatusEntry>,
    #[serde(default)]
    pub current_leader: Option<NodeId>,
    #[serde(default)]
    pub latest_term: Option<CoordinatorTerm>,
    #[serde(default)]
    pub lease: CoordinatorLeaseStatus,
    #[serde(default)]
    pub control_plane: CoordinatorControlPlaneStatus,
    pub emergency: CoordinatorEmergencyState,
    /// Most recent persisted election, including terminal outcomes. Keeping the
    /// full signed proposal and vote set makes takeover decisions auditable from
    /// the same projection used by clients and operators.
    #[serde(default)]
    pub latest_election: Option<CoordinatorElectionSession>,
}

/// A stale or restarted Coordinator candidate must prove which authority it
/// observed before it may receive the current snapshot. Audit sequence is a
/// separate cursor because a valid term and snapshot can still have a stale
/// control-plane history.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct CoordinatorRecoveryReport {
    pub network_id: NetworkId,
    pub observed_term: Option<u64>,
    pub observed_fencing_token: Option<FencingToken>,
    pub observed_leader: Option<NodeId>,
    pub observed_candidate_config_revision: Option<u64>,
    pub observed_audit_sequence: u64,
}

impl CoordinatorRecoveryReport {
    pub fn from_status(status: &CoordinatorCandidateStatus, observed_audit_sequence: u64) -> Self {
        Self {
            network_id: status.network_id.clone(),
            observed_term: status.latest_term.as_ref().map(|term| term.term),
            observed_fencing_token: status.latest_term.as_ref().map(|term| term.fencing_token),
            observed_leader: status.latest_term.as_ref().map(|term| term.leader.clone()),
            observed_candidate_config_revision: status
                .config
                .as_ref()
                .map(|config| config.revision),
            observed_audit_sequence,
        }
    }

    pub fn validate_definition(&self) -> DomainResult<()> {
        match (
            self.observed_term,
            self.observed_fencing_token,
            self.observed_leader.as_ref(),
        ) {
            (None, None, None) => {}
            (Some(term), Some(_), Some(_)) if term > 0 => {}
            _ => {
                return Err(DomainError::invariant(
                    "coordinator recovery report has an incomplete observed term",
                ));
            }
        }
        if self
            .observed_fencing_token
            .is_some_and(|token| token.0 == 0)
        {
            return Err(DomainError::invalid_value(
                "coordinator recovery fencing token must be positive",
            ));
        }
        if self
            .observed_candidate_config_revision
            .is_some_and(|revision| revision == 0)
        {
            return Err(DomainError::invalid_value(
                "coordinator recovery candidate revision must be positive",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinatorRecoveryStage {
    /// The reported term or snapshot is stale. Governance stays disabled until
    /// the authoritative snapshot is applied.
    ReadOnly,
    /// The snapshot and term match, but the audit cursor still needs replay.
    CatchingUp,
    /// Snapshot and audit cursors match. The node has rejoined only with the
    /// role it derives from the current candidate/member configuration.
    Rejoined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinatorRecoveryRole {
    Candidate,
    Member,
}

/// Fail-closed recovery projection. Health checks and the existing data plane
/// are independent from governance authority and remain available while the
/// control-plane cursors catch up.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct CoordinatorRecoveryStatus {
    pub network_id: NetworkId,
    pub stage: CoordinatorRecoveryStage,
    pub role: CoordinatorRecoveryRole,
    pub governance_writable: bool,
    pub health_checks_allowed: bool,
    pub data_plane_allowed: bool,
    pub authoritative_term: u64,
    pub authoritative_fencing_token: FencingToken,
    pub authoritative_leader: NodeId,
    pub authoritative_candidate_config_revision: u64,
    pub authoritative_audit_sequence: u64,
    pub reason_code: String,
}

impl CoordinatorRecoveryStatus {
    /// Recovery is never a back door to governance. A rejoined old Leader can
    /// perform health/runtime work and may later win a fresh election, but this
    /// status alone never restores writable authority.
    pub fn rejects_governance_write(&self) -> bool {
        !self.governance_writable
    }

    /// Validate the recovery record against the persisted authority snapshot.
    /// Recovery progress is allowed to lag, but its authority projection and
    /// safety flags must never diverge from the current term/configuration.
    pub fn validate_against_authority(
        &self,
        network_id: &NetworkId,
        term: &CoordinatorTerm,
        config: &CoordinatorCandidateConfig,
        is_candidate: bool,
    ) -> DomainResult<()> {
        if self.network_id != *network_id
            || self.authoritative_term != term.term
            || self.authoritative_fencing_token != term.fencing_token
            || self.authoritative_leader != term.leader
            || self.authoritative_candidate_config_revision != config.revision
        {
            return Err(DomainError::invariant(
                "coordinator recovery status does not match persisted authority",
            ));
        }
        if self.governance_writable || !self.health_checks_allowed || !self.data_plane_allowed {
            return Err(DomainError::invariant(
                "coordinator recovery status has unsafe capability flags",
            ));
        }
        let expected_role = if is_candidate {
            CoordinatorRecoveryRole::Candidate
        } else {
            CoordinatorRecoveryRole::Member
        };
        if self.role != expected_role || self.reason_code.is_empty() {
            return Err(DomainError::invalid_value(
                "coordinator recovery status has an invalid role or reason",
            ));
        }
        Ok(())
    }
}

/// A governance notification projected by a Leader. Consumers validate the
/// complete authority tuple against their authoritative candidate status before
/// applying it; an event from the previous Leader is never merged.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct CoordinatorGovernanceEvent {
    pub network_id: NetworkId,
    pub leader: NodeId,
    pub term: u64,
    pub fencing_token: FencingToken,
    pub candidate_config_revision: u64,
}

impl CoordinatorGovernanceEvent {
    pub fn validate_for_status(
        &self,
        status: &CoordinatorCandidateStatus,
        now_unix_ms: u64,
    ) -> DomainResult<()> {
        if self.network_id != status.network_id {
            return Err(DomainError::new(
                "domain.coordinator_governance_event_network_mismatch",
                "governance event belongs to another network",
            ));
        }
        let term = status.latest_term.as_ref().ok_or_else(|| {
            DomainError::new(
                "domain.coordinator_governance_event_without_term",
                "governance event arrived without an authoritative term",
            )
        })?;
        if self.term < term.term
            || self.fencing_token < term.fencing_token
            || self.leader != term.leader
            || status
                .config
                .as_ref()
                .is_some_and(|config| self.candidate_config_revision < config.revision)
        {
            return Err(DomainError::new(
                "domain.coordinator_stale_leader_event",
                "governance event was projected by an obsolete Leader authority",
            ));
        }
        if self.term > term.term
            || self.fencing_token > term.fencing_token
            || self.leader != term.leader
            || status
                .config
                .as_ref()
                .is_some_and(|config| self.candidate_config_revision > config.revision)
        {
            return Err(DomainError::new(
                "domain.coordinator_governance_event_not_installed",
                "governance event carries authority not present in the snapshot",
            ));
        }
        if status
            .config
            .as_ref()
            .is_none_or(|config| self.candidate_config_revision != config.revision)
            || self.candidate_config_revision != term.candidate_config_revision
        {
            return Err(DomainError::new(
                "domain.coordinator_governance_event_config_mismatch",
                "governance event does not bind the authoritative candidate configuration",
            ));
        }
        if !status.control_plane.is_available() || !term.lease_status_at(now_unix_ms).is_writable()
        {
            return Err(DomainError::new(
                "domain.coordinator_governance_event_read_only",
                "governance event arrived while control-plane authority is read-only",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct CoordinatorRecoveryResponse {
    pub status: CoordinatorCandidateStatus,
    pub recovery: CoordinatorRecoveryStatus,
    /// Sanitized audit records after the caller's cursor. This is the bounded
    /// control-plane journal replay that accompanies the authoritative snapshot.
    pub audit_entries: Vec<AuditEntry>,
}

impl CoordinatorCandidateStatus {
    /// Deterministic preferred candidate after the current lease expires. The
    /// previous Leader is excluded so a stale process cannot immediately win a
    /// new term without the rest of the group first observing the failure.
    pub fn preferred_election_candidate(&self) -> Option<&CoordinatorCandidateStatusEntry> {
        let previous_leader = self.latest_term.as_ref().map(|term| &term.leader);
        self.candidates
            .iter()
            .filter(|candidate| candidate.eligible_for_election)
            .filter(|candidate| previous_leader != Some(&candidate.node_id))
            .min_by(|left, right| {
                left.priority
                    .cmp(&right.priority)
                    .then_with(|| left.node_id.cmp(&right.node_id))
            })
    }

    /// Recompute the control-plane projection after attaching a persisted
    /// election session. This method only projects authoritative fields already
    /// present in the status; it never promotes a candidate to Leader.
    pub fn refresh_control_plane(&mut self) {
        self.control_plane = self.project_control_plane();
    }

    fn project_control_plane(&self) -> CoordinatorControlPlaneStatus {
        let quorum = self.project_quorum();
        let limited = |state, reason_code: &str| CoordinatorControlPlaneStatus {
            state,
            governance_writable: false,
            reason_code: reason_code.into(),
            quorum: quorum.clone(),
        };

        match self.config_state {
            CoordinatorCandidateConfigState::Missing => {
                return limited(
                    CoordinatorControlPlaneState::ConfigurationUnavailable,
                    "coordinator.control_plane.config_missing",
                );
            }
            CoordinatorCandidateConfigState::Pending => {
                return limited(
                    CoordinatorControlPlaneState::ConfigurationUnavailable,
                    "coordinator.control_plane.config_pending",
                );
            }
            CoordinatorCandidateConfigState::Expired => {
                return limited(
                    CoordinatorControlPlaneState::ConfigurationUnavailable,
                    "coordinator.control_plane.config_expired",
                );
            }
            CoordinatorCandidateConfigState::Current => {}
        }

        if self.lease.is_writable() && self.current_leader.is_some() {
            let full_stable_authority = self.latest_term.as_ref().is_some_and(|term| {
                term.authority == CoordinatorAuthority::Full
                    && term.phase == CoordinatorPhase::Stable
            });
            if full_stable_authority {
                return CoordinatorControlPlaneStatus {
                    state: CoordinatorControlPlaneState::Available,
                    governance_writable: true,
                    reason_code: "coordinator.control_plane.available".into(),
                    quorum,
                };
            }
            return limited(
                CoordinatorControlPlaneState::ReadOnly,
                "coordinator.control_plane.stable_full_authority_required",
            );
        }

        if self.lease.is_writable() {
            return limited(
                CoordinatorControlPlaneState::ReadOnly,
                "coordinator.control_plane.term_config_mismatch",
            );
        }

        if matches!(
            self.lease.reason_code.as_str(),
            "coordinator.lease.clock_skew" | "coordinator.lease.invalid_term"
        ) {
            return limited(
                CoordinatorControlPlaneState::ReadOnly,
                self.lease.reason_code.as_str(),
            );
        }

        if let Some(election) = &self.latest_election {
            let no_majority = quorum.received_votes < quorum.required_votes;
            return match election.stage {
                CoordinatorElectionStage::Started | CoordinatorElectionStage::Voting
                    if self.generated_at_unix_ms >= election.deadline_unix_ms =>
                {
                    limited(
                        CoordinatorControlPlaneState::QuorumUnavailable,
                        if no_majority {
                            "coordinator.control_plane.no_majority"
                        } else {
                            "coordinator.control_plane.commit_pending"
                        },
                    )
                }
                CoordinatorElectionStage::Started | CoordinatorElectionStage::Voting => limited(
                    CoordinatorControlPlaneState::ElectionInProgress,
                    "coordinator.control_plane.awaiting_quorum",
                ),
                CoordinatorElectionStage::TimedOut => limited(
                    CoordinatorControlPlaneState::QuorumUnavailable,
                    if no_majority {
                        "coordinator.control_plane.no_majority"
                    } else {
                        "coordinator.control_plane.election_timed_out"
                    },
                ),
                CoordinatorElectionStage::Rejected => limited(
                    CoordinatorControlPlaneState::QuorumUnavailable,
                    "coordinator.control_plane.election_rejected",
                ),
                CoordinatorElectionStage::Committed => limited(
                    CoordinatorControlPlaneState::NoLeader,
                    "coordinator.control_plane.committed_term_not_writable",
                ),
            };
        }

        let reason_code = match self.lease.reason_code.as_str() {
            "coordinator.lease.expired" => "coordinator.control_plane.lease_expired",
            "coordinator.lease.no_term" => "coordinator.control_plane.no_term",
            _ => "coordinator.control_plane.leader_unavailable",
        };
        limited(CoordinatorControlPlaneState::NoLeader, reason_code)
    }

    fn project_quorum(&self) -> CoordinatorQuorumStatus {
        let (received_votes, participant_count) = if let Some(election) = &self.latest_election {
            (election.votes.len(), election.participants.len())
        } else if let Some(term) = &self.latest_term {
            (term.majority_proof.len(), term.candidate_group.len())
        } else if let Some(config) = &self.config {
            (0, config.candidates.len())
        } else {
            (0, 0)
        };
        CoordinatorQuorumStatus {
            received_votes,
            required_votes: if participant_count == 0 {
                0
            } else {
                participant_count / 2 + 1
            },
            participant_count,
        }
    }

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
        let mut status = Self {
            network_id: network.network_id.clone(),
            generated_at_unix_ms: now_unix_ms,
            config_state,
            config: config.cloned(),
            candidates,
            current_leader,
            latest_term: latest_term.cloned(),
            lease,
            control_plane: CoordinatorControlPlaneStatus::default(),
            emergency,
            latest_election: None,
        };
        status.refresh_control_plane();
        Ok(status)
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
    fn election_proposal_is_signed_and_bound_to_its_identity() {
        let fixture = TermFixture::new();
        let mut proposal = CoordinatorElectionProposal::signed(
            fixture.network_id.clone(),
            1,
            1,
            fixture.candidate_group(),
            1_000,
            2_000,
            &fixture.candidate_one,
        )
        .unwrap();
        proposal.validate_at(1_500).unwrap();

        proposal.term = 2;
        assert_eq!(
            proposal.validate_definition().unwrap_err().code(),
            "domain.invalid_signature"
        );
    }

    #[test]
    fn election_session_requires_strict_majority_and_is_idempotent() {
        let fixture = TermFixture::new();
        let proposal = CoordinatorElectionProposal::signed(
            fixture.network_id.clone(),
            1,
            1,
            fixture.candidate_group(),
            1_000,
            2_000,
            &fixture.candidate_one,
        )
        .unwrap();
        let mut session = CoordinatorElectionSession::started(proposal.clone(), 1_000).unwrap();
        session.begin_voting(1_000).unwrap();
        let first_vote = CoordinatorElectionVote::signed(
            fixture.network_id.clone(),
            1,
            1,
            proposal.leader.clone(),
            fixture.candidate_group(),
            1_000,
            2_000,
            &fixture.candidate_one,
        )
        .unwrap();
        assert!(session.record_vote(first_vote.clone(), 1_100).unwrap());
        assert!(!session.record_vote(first_vote, 1_100).unwrap());
        assert_eq!(
            session.commit(FencingToken(1), 1_200).unwrap_err().code(),
            "domain.coordinator_election_no_majority"
        );

        let second_vote = CoordinatorElectionVote::signed(
            fixture.network_id.clone(),
            1,
            1,
            proposal.leader.clone(),
            fixture.candidate_group(),
            1_000,
            2_000,
            &fixture.candidate_two,
        )
        .unwrap();
        session.record_vote(second_vote, 1_200).unwrap();
        let term = session.commit(FencingToken(1), 1_300).unwrap();
        assert_eq!(term.term, 1);
        assert_eq!(term.leader, proposal.leader);
        assert_eq!(term.majority_votes.len(), 2);
        assert_eq!(session.stage, CoordinatorElectionStage::Committed);
    }

    #[test]
    fn election_session_rejects_conflicting_duplicate_votes_and_expiry() {
        let fixture = TermFixture::new();
        let proposal = CoordinatorElectionProposal::signed(
            fixture.network_id.clone(),
            1,
            1,
            fixture.candidate_group(),
            1_000,
            2_000,
            &fixture.candidate_one,
        )
        .unwrap();
        let mut session = CoordinatorElectionSession::started(proposal.clone(), 1_000).unwrap();
        session.begin_voting(1_000).unwrap();
        let first = CoordinatorElectionVote::signed(
            fixture.network_id.clone(),
            1,
            1,
            proposal.leader.clone(),
            fixture.candidate_group(),
            1_000,
            2_000,
            &fixture.candidate_one,
        )
        .unwrap();
        session.record_vote(first, 1_100).unwrap();

        let conflicting = CoordinatorElectionVote::signed(
            fixture.network_id.clone(),
            1,
            1,
            proposal.leader.clone(),
            fixture.candidate_group(),
            1_100,
            2_000,
            &fixture.candidate_one,
        )
        .unwrap();
        assert_eq!(
            session.record_vote(conflicting, 1_200).unwrap_err().code(),
            "domain.coordinator_election_conflicting_vote"
        );
        session.timed_out(2_000).unwrap();
        assert_eq!(session.stage, CoordinatorElectionStage::TimedOut);
        assert!(session.result_term.is_none());
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
    fn stale_leader_governance_events_are_rejected_against_authoritative_status() {
        let fixture = TermFixture::new();
        let current_term = fixture.term_with_window(2, 20, &fixture.candidate_one, 1_000, 200_000);
        let config = CoordinatorCandidateConfig::signed(
            fixture.network_id.clone(),
            1,
            fixture
                .candidate_group()
                .into_iter()
                .enumerate()
                .map(|(index, node_id)| CoordinatorCandidate {
                    node_id,
                    priority: index as u32,
                })
                .collect(),
            1_000,
            200_000,
            &fixture.candidate_one,
        )
        .unwrap();
        let status = CoordinatorCandidateStatus {
            network_id: fixture.network_id.clone(),
            generated_at_unix_ms: 2_000,
            config_state: CoordinatorCandidateConfigState::Current,
            config: Some(config),
            candidates: Vec::new(),
            current_leader: Some(current_term.leader.clone()),
            lease: current_term.lease_status_at(2_000),
            latest_term: Some(current_term.clone()),
            control_plane: CoordinatorControlPlaneStatus {
                state: CoordinatorControlPlaneState::Available,
                governance_writable: true,
                reason_code: "coordinator.control_plane.available".into(),
                quorum: CoordinatorQuorumStatus::default(),
            },
            emergency: CoordinatorEmergencyState::NoCandidatesConfigured,
            latest_election: None,
        };
        let current = CoordinatorGovernanceEvent {
            network_id: fixture.network_id.clone(),
            leader: current_term.leader.clone(),
            term: current_term.term,
            fencing_token: current_term.fencing_token,
            candidate_config_revision: 1,
        };
        current.validate_for_status(&status, 2_000).unwrap();

        let mut stale = current.clone();
        stale.leader = NodeId::from_public_key(&fixture.candidate_two.public_key());
        stale.term = 1;
        stale.fencing_token = FencingToken(10);
        assert_eq!(
            stale
                .validate_for_status(&status, 2_000)
                .unwrap_err()
                .code(),
            "domain.coordinator_stale_leader_event"
        );

        let mut future = current;
        future.term = 3;
        assert_eq!(
            future
                .validate_for_status(&status, 2_000)
                .unwrap_err()
                .code(),
            "domain.coordinator_governance_event_not_installed"
        );
    }

    #[test]
    fn recovery_status_keeps_runtime_available_but_never_writable() {
        let fixture = TermFixture::new();
        let term = fixture.term_with_window(2, 20, &fixture.candidate_one, 1_000, 200_000);
        let config = CoordinatorCandidateConfig::signed(
            fixture.network_id.clone(),
            1,
            fixture
                .candidate_group()
                .into_iter()
                .enumerate()
                .map(|(index, node_id)| CoordinatorCandidate {
                    node_id,
                    priority: index as u32,
                })
                .collect(),
            1_000,
            200_000,
            &fixture.candidate_one,
        )
        .unwrap();
        let recovery = CoordinatorRecoveryStatus {
            network_id: fixture.network_id.clone(),
            stage: CoordinatorRecoveryStage::Rejoined,
            role: CoordinatorRecoveryRole::Candidate,
            governance_writable: false,
            health_checks_allowed: true,
            data_plane_allowed: true,
            authoritative_term: term.term,
            authoritative_fencing_token: term.fencing_token,
            authoritative_leader: term.leader.clone(),
            authoritative_candidate_config_revision: config.revision,
            authoritative_audit_sequence: 4,
            reason_code: "coordinator.recovery.rejoined_candidate".into(),
        };
        recovery
            .validate_against_authority(&fixture.network_id, &term, &config, true)
            .unwrap();
        assert!(recovery.rejects_governance_write());

        let mut escalated = recovery.clone();
        escalated.governance_writable = true;
        assert_eq!(
            escalated
                .validate_against_authority(&fixture.network_id, &term, &config, true)
                .unwrap_err()
                .code(),
            "domain.invariant_violation"
        );

        let mut wrong_role = recovery;
        wrong_role.role = CoordinatorRecoveryRole::Member;
        assert_eq!(
            wrong_role
                .validate_against_authority(&fixture.network_id, &term, &config, true)
                .unwrap_err()
                .code(),
            "domain.invalid_value"
        );
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
    fn proposals_validate_against_authoritative_candidate_status_before_voting() {
        let fixture = candidate_fixture();
        let candidate_one = NodeId::from_public_key(&fixture.candidate_one.public_key());
        let candidate_two = NodeId::from_public_key(&fixture.candidate_two.public_key());
        let config = config(
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
        let declarations = vec![
            declaration(
                &fixture,
                &fixture.candidate_one,
                CoordinatorCandidateWillingness::Willing,
                2_000,
            ),
            declaration(
                &fixture,
                &fixture.candidate_two,
                CoordinatorCandidateWillingness::Willing,
                2_000,
            ),
        ];
        let candidate_group = vec![candidate_one.clone(), candidate_two.clone()];
        let majority_votes = [&fixture.candidate_one, &fixture.candidate_two]
            .into_iter()
            .map(|voter| {
                CoordinatorElectionVote::signed(
                    fixture.network.network_id.clone(),
                    7,
                    1,
                    candidate_one.clone(),
                    candidate_group.clone(),
                    1_000,
                    2_500,
                    voter,
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        let latest_term = CoordinatorTerm {
            network_id: fixture.network.network_id.clone(),
            term: 1,
            leader: candidate_one.clone(),
            candidate_group,
            candidate_config_revision: 7,
            majority_proof: majority_votes
                .iter()
                .map(|vote| vote.voter.clone())
                .collect(),
            majority_votes,
            authority: CoordinatorAuthority::Full,
            phase: CoordinatorPhase::Stable,
            fencing_token: FencingToken(40),
            issued_at_unix_ms: 1_000,
            expires_at_unix_ms: 2_500,
        };
        let expired_status = CoordinatorCandidateStatus::assess(
            &fixture.network,
            Some(&config),
            &declarations,
            Some(&latest_term),
            3_000,
        )
        .unwrap();
        let proposal = CoordinatorElectionProposal::signed(
            fixture.network.network_id.clone(),
            7,
            2,
            vec![candidate_one.clone(), candidate_two.clone()],
            3_000,
            4_000,
            &fixture.candidate_two,
        )
        .unwrap();
        proposal
            .validate_for_candidate_status(&expired_status, 3_000)
            .unwrap();

        let active_status = CoordinatorCandidateStatus::assess(
            &fixture.network,
            Some(&config),
            &declarations,
            Some(&latest_term),
            2_000,
        )
        .unwrap();
        let active_proposal = CoordinatorElectionProposal::signed(
            fixture.network.network_id.clone(),
            7,
            2,
            vec![candidate_one.clone(), candidate_two.clone()],
            2_000,
            2_200,
            &fixture.candidate_two,
        )
        .unwrap();
        assert_eq!(
            active_proposal
                .validate_for_candidate_status(&active_status, 2_000)
                .unwrap_err()
                .code(),
            "domain.coordinator_election_lease_active"
        );

        let non_preferred = CoordinatorElectionProposal::signed(
            fixture.network.network_id.clone(),
            7,
            2,
            vec![candidate_one, candidate_two],
            3_000,
            4_000,
            &fixture.candidate_one,
        )
        .unwrap();
        assert_eq!(
            non_preferred
                .validate_for_candidate_status(&expired_status, 3_000)
                .unwrap_err()
                .code(),
            "domain.coordinator_election_leader_not_preferred"
        );
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

    #[test]
    fn control_plane_requires_strict_majority_before_enabling_governance() {
        let fixture = candidate_fixture();
        let candidate_one = NodeId::from_public_key(&fixture.candidate_one.public_key());
        let candidate_two = NodeId::from_public_key(&fixture.candidate_two.public_key());
        let ordinary = NodeId::from_public_key(&fixture.ordinary.public_key());
        let candidates = vec![
            CoordinatorCandidate {
                node_id: candidate_one.clone(),
                priority: 10,
            },
            CoordinatorCandidate {
                node_id: candidate_two.clone(),
                priority: 20,
            },
            CoordinatorCandidate {
                node_id: ordinary.clone(),
                priority: 30,
            },
        ];
        let config = config(&fixture, &fixture.owner, candidates);
        let declarations = vec![
            declaration(
                &fixture,
                &fixture.candidate_one,
                CoordinatorCandidateWillingness::Willing,
                2_000,
            ),
            declaration(
                &fixture,
                &fixture.candidate_two,
                CoordinatorCandidateWillingness::Willing,
                2_000,
            ),
            declaration(
                &fixture,
                &fixture.ordinary,
                CoordinatorCandidateWillingness::Willing,
                2_000,
            ),
        ];
        let initial = CoordinatorCandidateStatus::assess(
            &fixture.network,
            Some(&config),
            &declarations,
            None,
            3_000,
        )
        .unwrap();
        assert_eq!(
            initial.control_plane.state,
            CoordinatorControlPlaneState::NoLeader
        );
        assert!(!initial.control_plane.governance_writable);
        assert_eq!(initial.control_plane.quorum.received_votes, 0);
        assert_eq!(initial.control_plane.quorum.required_votes, 2);
        assert_eq!(initial.control_plane.quorum.participant_count, 3);

        let candidate_group = vec![candidate_one.clone(), candidate_two.clone(), ordinary];
        let proposal = CoordinatorElectionProposal::signed(
            fixture.network.network_id.clone(),
            7,
            1,
            candidate_group.clone(),
            3_000,
            8_000,
            &fixture.candidate_one,
        )
        .unwrap();
        let mut session = CoordinatorElectionSession::started(proposal, 3_000).unwrap();
        session.begin_voting(3_000).unwrap();
        let first_vote = CoordinatorElectionVote::signed(
            fixture.network.network_id.clone(),
            7,
            1,
            candidate_one.clone(),
            candidate_group.clone(),
            3_000,
            8_000,
            &fixture.candidate_one,
        )
        .unwrap();
        session.record_vote(first_vote, 3_001).unwrap();

        let mut voting = CoordinatorCandidateStatus::assess(
            &fixture.network,
            Some(&config),
            &declarations,
            None,
            3_001,
        )
        .unwrap();
        voting.latest_election = Some(session.clone());
        voting.refresh_control_plane();
        assert_eq!(
            voting.control_plane.state,
            CoordinatorControlPlaneState::ElectionInProgress
        );
        assert!(!voting.control_plane.governance_writable);
        assert_eq!(voting.control_plane.quorum.received_votes, 1);
        assert_eq!(voting.control_plane.quorum.required_votes, 2);

        let second_vote = CoordinatorElectionVote::signed(
            fixture.network.network_id.clone(),
            7,
            1,
            candidate_one.clone(),
            candidate_group,
            3_000,
            8_000,
            &fixture.candidate_two,
        )
        .unwrap();
        session.record_vote(second_vote, 3_002).unwrap();
        let term = session.commit(FencingToken(1), 3_003).unwrap();
        let mut available = CoordinatorCandidateStatus::assess(
            &fixture.network,
            Some(&config),
            &declarations,
            Some(&term),
            3_003,
        )
        .unwrap();
        available.latest_election = Some(session);
        available.refresh_control_plane();
        assert_eq!(
            available.control_plane.state,
            CoordinatorControlPlaneState::Available
        );
        assert!(available.control_plane.governance_writable);
        assert_eq!(available.control_plane.quorum.received_votes, 2);
        assert_eq!(available.control_plane.quorum.required_votes, 2);
        assert_eq!(available.current_leader, Some(candidate_one));
        assert_eq!(available.latest_term.as_ref().unwrap().term, 1);
        assert_eq!(
            available.latest_term.as_ref().unwrap().fencing_token,
            FencingToken(1)
        );

        let restored = CoordinatorCandidateStatus::assess(
            &fixture.network,
            Some(&config),
            &declarations,
            available.latest_term.as_ref(),
            3_004,
        )
        .unwrap();
        assert_eq!(
            restored.control_plane.state,
            CoordinatorControlPlaneState::Available
        );
        assert!(restored.control_plane.governance_writable);
        assert_eq!(restored.latest_term, available.latest_term);
    }

    #[test]
    fn control_plane_projects_timeout_configuration_and_clock_limits() {
        let fixture = candidate_fixture();
        let candidate_one = NodeId::from_public_key(&fixture.candidate_one.public_key());
        let candidate_two = NodeId::from_public_key(&fixture.candidate_two.public_key());

        let missing =
            CoordinatorCandidateStatus::assess(&fixture.network, None, &[], None, 3_000).unwrap();
        assert_eq!(
            missing.control_plane.state,
            CoordinatorControlPlaneState::ConfigurationUnavailable
        );
        assert_eq!(
            missing.control_plane.reason_code,
            "coordinator.control_plane.config_missing"
        );

        let config = config(
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
        let declarations = vec![
            declaration(
                &fixture,
                &fixture.candidate_one,
                CoordinatorCandidateWillingness::Willing,
                2_000,
            ),
            declaration(
                &fixture,
                &fixture.candidate_two,
                CoordinatorCandidateWillingness::Willing,
                2_000,
            ),
        ];
        let expired = CoordinatorCandidateStatus::assess(
            &fixture.network,
            Some(&config),
            &declarations,
            None,
            10_000,
        )
        .unwrap();
        assert_eq!(
            expired.control_plane.state,
            CoordinatorControlPlaneState::ConfigurationUnavailable
        );
        assert_eq!(
            expired.control_plane.reason_code,
            "coordinator.control_plane.config_expired"
        );

        let candidate_group = vec![candidate_one.clone(), candidate_two.clone()];
        let proposal = CoordinatorElectionProposal::signed(
            fixture.network.network_id.clone(),
            7,
            1,
            candidate_group.clone(),
            3_000,
            6_000,
            &fixture.candidate_one,
        )
        .unwrap();
        let mut timed_out_session = CoordinatorElectionSession::started(proposal, 3_000).unwrap();
        timed_out_session.begin_voting(3_000).unwrap();
        let vote = CoordinatorElectionVote::signed(
            fixture.network.network_id.clone(),
            7,
            1,
            candidate_one.clone(),
            candidate_group.clone(),
            3_000,
            6_000,
            &fixture.candidate_one,
        )
        .unwrap();
        timed_out_session.record_vote(vote, 3_001).unwrap();
        timed_out_session.timed_out(6_000).unwrap();
        let mut timed_out = CoordinatorCandidateStatus::assess(
            &fixture.network,
            Some(&config),
            &declarations,
            None,
            6_000,
        )
        .unwrap();
        timed_out.latest_election = Some(timed_out_session);
        timed_out.refresh_control_plane();
        assert_eq!(
            timed_out.control_plane.state,
            CoordinatorControlPlaneState::QuorumUnavailable
        );
        assert_eq!(
            timed_out.control_plane.reason_code,
            "coordinator.control_plane.no_majority"
        );
        assert_eq!(timed_out.control_plane.quorum.received_votes, 1);
        assert_eq!(timed_out.control_plane.quorum.required_votes, 2);
        assert_eq!(timed_out.latest_term, None);
        assert_eq!(timed_out.current_leader, None);

        let now_unix_ms = 3_000;
        let issued_at_unix_ms = now_unix_ms + COORDINATOR_CLOCK_SKEW_LIMIT_MS + 1;
        let expires_at_unix_ms = issued_at_unix_ms + COORDINATOR_ELECTION_VOTE_MAX_VALIDITY_MS;
        let majority_votes = [&fixture.candidate_one, &fixture.candidate_two]
            .into_iter()
            .map(|voter| {
                CoordinatorElectionVote::signed(
                    fixture.network.network_id.clone(),
                    7,
                    1,
                    candidate_one.clone(),
                    candidate_group.clone(),
                    issued_at_unix_ms,
                    expires_at_unix_ms,
                    voter,
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        let skewed_term = CoordinatorTerm {
            network_id: fixture.network.network_id.clone(),
            term: 1,
            leader: candidate_one.clone(),
            candidate_group,
            candidate_config_revision: 7,
            majority_proof: majority_votes
                .iter()
                .map(|vote| vote.voter.clone())
                .collect(),
            majority_votes,
            authority: CoordinatorAuthority::Full,
            phase: CoordinatorPhase::Stable,
            fencing_token: FencingToken(1),
            issued_at_unix_ms,
            expires_at_unix_ms,
        };
        let skewed = CoordinatorCandidateStatus::assess(
            &fixture.network,
            Some(&config),
            &declarations,
            Some(&skewed_term),
            now_unix_ms,
        )
        .unwrap();
        assert_eq!(
            skewed.control_plane.state,
            CoordinatorControlPlaneState::ReadOnly
        );
        assert_eq!(
            skewed.control_plane.reason_code,
            "coordinator.lease.clock_skew"
        );
        assert!(!skewed.control_plane.governance_writable);
    }
}
