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
        let current_leader = latest_term
            .filter(|term| term.validate_at(now_unix_ms).is_ok())
            .map(|term| term.leader.clone());
        Ok(Self {
            network_id: network.network_id.clone(),
            generated_at_unix_ms: now_unix_ms,
            config_state,
            config: config.cloned(),
            candidates,
            current_leader,
            latest_term: latest_term.cloned(),
            emergency,
        })
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
    fn emergency_handoff_keeps_governance_frozen_until_full_successor_recovers() {
        let leader = NodeId::from_public_key(b"candidate-1");
        let mut emergency = term(4, 40, leader.clone());
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

        let successor = term(emergency.term + 1, emergency.fencing_token.0 + 1, leader);
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
        let mut latest_term = term(4, 40, candidate_one);
        latest_term.network_id = fixture.network.network_id.clone();
        latest_term.issued_at_unix_ms = 1_000;
        latest_term.expires_at_unix_ms = 2_500;
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
