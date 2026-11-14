use anyhow::{bail, Context, Result};
use fs2::FileExt;
use fuckipv6_common::domain::{
    AuditEntry, AuditId, AuditLog, AuditOutcome, ContentHash, CoordinatorCandidateConfig,
    CoordinatorCandidateDeclaration, CoordinatorCandidateStatus, CoordinatorElectionProposal,
    CoordinatorElectionSession, CoordinatorElectionStage, CoordinatorElectionVote,
    CoordinatorEmergencyLeaderResponse, CoordinatorEmergencyLeaderStatus,
    CoordinatorEmergencyWriteCredential, CoordinatorRecoveryReport, CoordinatorRecoveryResponse,
    CoordinatorRecoveryRole, CoordinatorRecoveryStage, CoordinatorRecoveryStatus, CoordinatorTerm,
    CoordinatorWriteCredential, CoordinatorWriteKind, NetworkId, NetworkMemberOnlineState,
    NetworkMemberPresence, NetworkRecord, NodeId, COORDINATOR_MAX_LEASE_DURATION_MS,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

const COORDINATOR_CANDIDATE_FILE_SCHEMA_VERSION: u16 = 3;
const COORDINATOR_CANDIDATE_FILE_SCHEMA_VERSION_V2: u16 = 2;
const COORDINATOR_CANDIDATE_FILE_SCHEMA_VERSION_V1: u16 = 1;
const COORDINATOR_RECOVERY_AUDIT_REPLAY_LIMIT: usize = 128;
pub(crate) const GOVERNANCE_FROZEN_ERROR: &str = "coordinator_candidate.error.governance_frozen";
static AUDIT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct ConfigUpdate<'a> {
    actor: &'a NodeId,
    expected_revision: u64,
    config: CoordinatorCandidateConfig,
    credential: Option<&'a CoordinatorWriteCredential>,
    network: &'a NetworkRecord,
    now_unix_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCoordinatorCandidates {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    config: Option<CoordinatorCandidateConfig>,
    #[serde(default)]
    declarations: BTreeMap<NodeId, CoordinatorCandidateDeclaration>,
    /// Durable high-water mark. It is retained after lease expiry so a restart
    /// can never make an older term appear current again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    latest_term: Option<CoordinatorTerm>,
    /// The most recent election is retained in full so status readers can
    /// verify its proposal, votes and terminal outcome without trusting an
    /// in-memory quorum calculation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    latest_election: Option<CoordinatorElectionSession>,
    /// Last validated recovery cursor for each node. This is progress state,
    /// not authority: it can never make a node writable or install a term.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    recoveries: BTreeMap<NodeId, CoordinatorRecoveryStatus>,
    /// A bounded emergency lease issued only after the server observed every
    /// configured candidate offline. This is independent from `latest_term`
    /// and can never authorize a governance write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    emergency_leader: Option<CoordinatorEmergencyLeaderStatus>,
    /// Durable emergency epoch high-water mark. It is retained after renewal,
    /// expiry and revocation so a restart can never reuse emergency authority.
    #[serde(default)]
    emergency_epoch: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CoordinatorCandidateFile {
    schema_version: u16,
    #[serde(default)]
    networks: BTreeMap<NetworkId, StoredCoordinatorCandidates>,
    #[serde(default)]
    audit_log: AuditLog,
}

impl Default for CoordinatorCandidateFile {
    fn default() -> Self {
        Self {
            schema_version: COORDINATOR_CANDIDATE_FILE_SCHEMA_VERSION,
            networks: BTreeMap::new(),
            audit_log: AuditLog::default(),
        }
    }
}

impl CoordinatorCandidateFile {
    fn validate(&self) -> Result<()> {
        if !matches!(
            self.schema_version,
            COORDINATOR_CANDIDATE_FILE_SCHEMA_VERSION_V1
                | COORDINATOR_CANDIDATE_FILE_SCHEMA_VERSION_V2
                | COORDINATOR_CANDIDATE_FILE_SCHEMA_VERSION
        ) {
            bail!("coordinator_candidate.error.unsupported_schema");
        }
        for (network_id, stored) in &self.networks {
            if let Some(config) = &stored.config {
                config.validate_definition()?;
                if config.network_id != *network_id {
                    bail!("coordinator_candidate.error.network_mismatch");
                }
            }
            if let Some(term) = &stored.latest_term {
                term.validate_definition()?;
                if term.network_id != *network_id {
                    bail!("coordinator_candidate.error.term_network_mismatch");
                }
            }
            if let Some(election) = &stored.latest_election {
                election.validate_definition()?;
                if election.proposal.network_id != *network_id {
                    bail!("coordinator_candidate.error.election_network_mismatch");
                }
                if let Some(term) = &election.result_term {
                    term.validate_definition()?;
                    if term.network_id != *network_id {
                        bail!("coordinator_candidate.error.election_term_network_mismatch");
                    }
                }
            }
            if let Some(emergency) = &stored.emergency_leader {
                emergency.validate_definition()?;
                let config = stored
                    .config
                    .as_ref()
                    .context("coordinator_candidate.error.emergency_config_missing")?;
                if emergency.network_id != *network_id
                    || emergency.candidate_config_revision != config.revision
                    || emergency.emergency_epoch > stored.emergency_epoch
                {
                    bail!("coordinator_candidate.error.emergency_context_mismatch");
                }
                let configured = config
                    .candidates
                    .iter()
                    .map(|candidate| candidate.node_id.clone())
                    .collect::<std::collections::BTreeSet<_>>();
                let unavailable = emergency
                    .unavailable_candidates
                    .iter()
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>();
                if configured != unavailable || configured.contains(&emergency.leader) {
                    bail!("coordinator_candidate.error.emergency_proof_mismatch");
                }
            }
            if stored.emergency_leader.is_some() && stored.emergency_epoch == 0 {
                bail!("coordinator_candidate.error.emergency_epoch_missing");
            }
            for (node_id, declaration) in &stored.declarations {
                declaration.validate_definition()?;
                if declaration.network_id != *network_id || declaration.node_id != *node_id {
                    bail!("coordinator_candidate.error.declaration_identity_mismatch");
                }
                let config = stored
                    .config
                    .as_ref()
                    .context("coordinator_candidate.error.config_missing")?;
                if declaration.candidate_config_revision != config.revision {
                    bail!("coordinator_candidate.error.declaration_revision_mismatch");
                }
                if !config
                    .candidates
                    .iter()
                    .any(|candidate| candidate.node_id == *node_id)
                {
                    bail!("coordinator_candidate.error.declaration_not_candidate");
                }
            }
            let network_audit_sequence = self
                .audit_log
                .entries()
                .iter()
                .filter(|entry| entry.network_id.as_ref() == Some(network_id))
                .count() as u64;
            for (node_id, recovery) in &stored.recoveries {
                let term = stored
                    .latest_term
                    .as_ref()
                    .context("coordinator_candidate.error.recovery_term_missing")?;
                let config = stored
                    .config
                    .as_ref()
                    .context("coordinator_candidate.error.recovery_config_missing")?;
                let is_candidate = config
                    .candidates
                    .iter()
                    .any(|candidate| candidate.node_id == *node_id);
                recovery.validate_against_authority(network_id, term, config, is_candidate)?;
                if recovery.authoritative_audit_sequence > network_audit_sequence {
                    bail!("coordinator_candidate.error.recovery_audit_cursor_ahead");
                }
            }
        }
        Ok(())
    }
}

/// Durable, revision-checked candidate configuration and declaration ledger.
///
/// The network record is read and validated under the shared profile lock for
/// every mutation. Candidate configuration is accepted only from its verified
/// signer and must advance the current revision by exactly one. Declarations
/// are self-signed, bound to the current configuration revision, and can never
/// mutate candidate membership or appoint a Leader.
pub struct CoordinatorCandidateStore {
    path: PathBuf,
    network_record_path: PathBuf,
    process_lock: Mutex<()>,
}

/// Proof that the cross-process governance gate is held for one write.
///
/// Nested stores use this permit instead of trying to acquire the same gate
/// again. That keeps compound admission transactions atomic without creating a
/// self-deadlock between the join-request/invitation ledgers and the lifecycle
/// ledger.
pub(crate) struct GovernancePermit<'a> {
    _process_guard: MutexGuard<'a, ()>,
    _gate: File,
}

impl CoordinatorCandidateStore {
    pub fn new(path: impl Into<PathBuf>, network_record_path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let network_record_path = network_record_path.into();
        if path == network_record_path {
            bail!("coordinator candidate and network record paths must differ");
        }
        Ok(Self {
            path,
            network_record_path,
            process_lock: Mutex::new(()),
        })
    }

    /// Admit a new authoritative term. This is the persistence primitive used
    /// by the election/handoff protocol; it validates the term against the
    /// active candidate configuration and advances the durable high-water mark.
    pub fn install_term(
        &self,
        actor: &NodeId,
        term: CoordinatorTerm,
        now_unix_ms: u64,
    ) -> Result<CoordinatorCandidateStatus> {
        let network_id = term.network_id.clone();
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("coordinator_candidate.error.store_lock_poisoned"))?;
        let _profile_lock = lock_profile(&self.network_record_path, false)?;
        self.ensure_parent()?;
        let _file_lock = lock_candidate_file(&self.path)?;
        let mut file = self.load_unlocked()?;
        let network = read_network_record(&self.network_record_path, &network_id)?;
        let revoked_emergency = file
            .networks
            .get(&network_id)
            .and_then(|stored| stored.emergency_leader.clone());

        let outcome = self.apply_term_install(&mut file, actor, term, &network, now_unix_ms);
        match outcome {
            Ok(status) => {
                append_audit(
                    &mut file.audit_log,
                    actor,
                    &network.network_id,
                    "coordinator.term.install",
                    format!("network/{}/coordinator-term", network.network_id),
                    AuditOutcome::Succeeded,
                    now_unix_ms,
                    "coordinator term admitted",
                    term_audit_details(status.latest_term.as_ref()),
                )?;
                if let Some(lease) = revoked_emergency {
                    append_audit(
                        &mut file.audit_log,
                        actor,
                        &network.network_id,
                        "coordinator.emergency.leader.revoked",
                        format!("network/{}/coordinator-emergency", network.network_id),
                        AuditOutcome::Succeeded,
                        now_unix_ms,
                        "emergency runtime Leader revoked after authoritative term handoff",
                        BTreeMap::from([
                            (
                                "reason_code".into(),
                                "coordinator.emergency.reason.successor_term_installed".into(),
                            ),
                            ("emergency_epoch".into(), lease.emergency_epoch.to_string()),
                            ("leader".into(), lease.leader.to_string()),
                            (
                                "successor_term".into(),
                                status
                                    .latest_term
                                    .as_ref()
                                    .map(|term| term.term.to_string())
                                    .unwrap_or_default(),
                            ),
                            (
                                "successor_leader".into(),
                                status
                                    .latest_term
                                    .as_ref()
                                    .map(|term| term.leader.to_string())
                                    .unwrap_or_default(),
                            ),
                            (
                                "successor_fencing_token".into(),
                                status
                                    .latest_term
                                    .as_ref()
                                    .map(|term| term.fencing_token.0.to_string())
                                    .unwrap_or_default(),
                            ),
                        ]),
                    )?;
                }
                self.persist(&file)?;
                Ok(status)
            }
            Err(error) => {
                append_audit(
                    &mut file.audit_log,
                    actor,
                    &network.network_id,
                    "coordinator.term.install.rejected",
                    format!("network/{}/coordinator-term", network.network_id),
                    audit_outcome_for_error(&error),
                    now_unix_ms,
                    "coordinator term rejected",
                    BTreeMap::from([("error".into(), error.to_string())]),
                )?;
                self.persist(&file)?;
                Err(error)
            }
        }
    }

    /// Persist a candidate-signed proposal and open its voting window. The
    /// proposal is validated against the current configuration, health
    /// declarations and durable term high-water mark before any vote is
    /// accepted.
    pub fn propose_election(
        &self,
        actor: &NodeId,
        proposal: CoordinatorElectionProposal,
        now_unix_ms: u64,
    ) -> Result<CoordinatorCandidateStatus> {
        let network_id = proposal.network_id.clone();
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("coordinator_candidate.error.store_lock_poisoned"))?;
        let _profile_lock = lock_profile(&self.network_record_path, false)?;
        self.ensure_parent()?;
        let _file_lock = lock_candidate_file(&self.path)?;
        let mut file = self.load_unlocked()?;
        let network = read_network_record(&self.network_record_path, &network_id)?;

        let outcome =
            self.apply_election_proposal(&mut file, actor, proposal, &network, now_unix_ms);
        match outcome {
            Ok(status) => {
                append_audit(
                    &mut file.audit_log,
                    actor,
                    &network.network_id,
                    "coordinator.election.started",
                    format!("network/{}/coordinator-election", network.network_id),
                    AuditOutcome::Succeeded,
                    now_unix_ms,
                    "coordinator election proposal accepted",
                    election_audit_details(status.latest_election.as_ref()),
                )?;
                self.persist(&file)?;
                Ok(status)
            }
            Err(error) => {
                append_audit(
                    &mut file.audit_log,
                    actor,
                    &network.network_id,
                    "coordinator.election.started",
                    format!("network/{}/coordinator-election", network.network_id),
                    audit_outcome_for_error(&error),
                    now_unix_ms,
                    "coordinator election proposal rejected",
                    BTreeMap::from([("error".into(), error.to_string())]),
                )?;
                self.persist(&file)?;
                Err(error)
            }
        }
    }

    /// Record one independently signed election vote. When the strict-majority
    /// threshold is reached, the committed term and fencing token are installed
    /// in the same file-locked transaction as the deciding vote.
    pub fn record_election_vote(
        &self,
        actor: &NodeId,
        proposal_id: &ContentHash,
        vote: CoordinatorElectionVote,
        now_unix_ms: u64,
    ) -> Result<CoordinatorCandidateStatus> {
        let network_id = vote.network_id.clone();
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("coordinator_candidate.error.store_lock_poisoned"))?;
        let _profile_lock = lock_profile(&self.network_record_path, false)?;
        self.ensure_parent()?;
        let _file_lock = lock_candidate_file(&self.path)?;
        let mut file = self.load_unlocked()?;
        let network = read_network_record(&self.network_record_path, &network_id)?;

        let outcome =
            self.apply_election_vote(&mut file, actor, proposal_id, vote, &network, now_unix_ms);
        match outcome {
            Ok((status, committed)) => {
                append_audit(
                    &mut file.audit_log,
                    actor,
                    &network.network_id,
                    "coordinator.election.vote.accepted",
                    format!(
                        "network/{}/coordinator-election/{proposal_id:?}",
                        network.network_id
                    ),
                    AuditOutcome::Succeeded,
                    now_unix_ms,
                    "coordinator election vote accepted",
                    election_audit_details(status.latest_election.as_ref()),
                )?;
                if committed {
                    append_audit(
                        &mut file.audit_log,
                        actor,
                        &network.network_id,
                        "coordinator.election.committed",
                        format!("network/{}/coordinator-term", network.network_id),
                        AuditOutcome::Succeeded,
                        now_unix_ms,
                        "coordinator election reached a strict majority",
                        election_audit_details(status.latest_election.as_ref()),
                    )?;
                }
                self.persist(&file)?;
                Ok(status)
            }
            Err(error) => {
                append_audit(
                    &mut file.audit_log,
                    actor,
                    &network.network_id,
                    "coordinator.election.vote.rejected",
                    format!(
                        "network/{}/coordinator-election/{proposal_id:?}",
                        network.network_id
                    ),
                    audit_outcome_for_error(&error),
                    now_unix_ms,
                    "coordinator election vote rejected",
                    BTreeMap::from([("error".into(), error.to_string())]),
                )?;
                self.persist(&file)?;
                Err(error)
            }
        }
    }

    /// Mark an expired election as timed out. Any participant may drive the
    /// transition, but ordinary network members cannot mutate election state.
    pub fn timeout_election(
        &self,
        actor: &NodeId,
        network_id: &NetworkId,
        proposal_id: &ContentHash,
        now_unix_ms: u64,
    ) -> Result<CoordinatorCandidateStatus> {
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("coordinator_candidate.error.store_lock_poisoned"))?;
        let _profile_lock = lock_profile(&self.network_record_path, false)?;
        self.ensure_parent()?;
        let _file_lock = lock_candidate_file(&self.path)?;
        let mut file = self.load_unlocked()?;
        let network = read_network_record(&self.network_record_path, network_id)?;

        let outcome =
            self.apply_election_timeout(&mut file, actor, proposal_id, &network, now_unix_ms);
        match outcome {
            Ok((status, newly_timed_out)) => {
                if !newly_timed_out {
                    return Ok(status);
                }
                let details = election_audit_details(status.latest_election.as_ref());
                append_audit(
                    &mut file.audit_log,
                    actor,
                    &network.network_id,
                    "coordinator.election.timed_out",
                    format!(
                        "network/{}/coordinator-election/{proposal_id:?}",
                        network.network_id
                    ),
                    AuditOutcome::Succeeded,
                    now_unix_ms,
                    "coordinator election timed out",
                    details.clone(),
                )?;
                let no_majority = status.latest_election.as_ref().is_some_and(|election| {
                    election.votes.len() < election.proposal.majority_threshold()
                });
                if no_majority {
                    append_audit(
                        &mut file.audit_log,
                        actor,
                        &network.network_id,
                        "coordinator.election.no_majority",
                        format!(
                            "network/{}/coordinator-election/{proposal_id:?}",
                            network.network_id
                        ),
                        AuditOutcome::Succeeded,
                        now_unix_ms,
                        "coordinator election timed out without a strict majority",
                        details,
                    )?;
                }
                self.persist(&file)?;
                Ok(status)
            }
            Err(error) => {
                append_audit(
                    &mut file.audit_log,
                    actor,
                    &network.network_id,
                    "coordinator.election.timed_out",
                    format!(
                        "network/{}/coordinator-election/{proposal_id:?}",
                        network.network_id
                    ),
                    audit_outcome_for_error(&error),
                    now_unix_ms,
                    "coordinator election timeout rejected",
                    BTreeMap::from([("error".into(), error.to_string())]),
                )?;
                self.persist(&file)?;
                Err(error)
            }
        }
    }

    /// Renew the current Leader lease without changing term or fencing token.
    /// Expired or clock-skewed authority remains read-only and the refusal is
    /// appended to the durable audit chain.
    pub fn renew_term_lease(
        &self,
        actor: &NodeId,
        credential: &CoordinatorWriteCredential,
        new_expires_at_unix_ms: u64,
        now_unix_ms: u64,
    ) -> Result<CoordinatorCandidateStatus> {
        let network_id = credential.network_id.clone();
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("coordinator_candidate.error.store_lock_poisoned"))?;
        let _profile_lock = lock_profile(&self.network_record_path, false)?;
        self.ensure_parent()?;
        let _file_lock = lock_candidate_file(&self.path)?;
        let mut file = self.load_unlocked()?;
        let network = read_network_record(&self.network_record_path, &network_id)?;

        let outcome = self.apply_term_renewal(
            &mut file,
            actor,
            credential,
            new_expires_at_unix_ms,
            &network,
            now_unix_ms,
        );
        match outcome {
            Ok(status) => {
                append_audit(
                    &mut file.audit_log,
                    actor,
                    &network.network_id,
                    "coordinator.term.lease.renew",
                    format!("network/{}/coordinator-term", network.network_id),
                    AuditOutcome::Succeeded,
                    now_unix_ms,
                    "coordinator term lease renewed",
                    term_audit_details(status.latest_term.as_ref()),
                )?;
                self.persist(&file)?;
                Ok(status)
            }
            Err(error) => {
                append_audit(
                    &mut file.audit_log,
                    actor,
                    &network.network_id,
                    "coordinator.term.lease.renew.rejected",
                    format!("network/{}/coordinator-term", network.network_id),
                    audit_outcome_for_error(&error),
                    now_unix_ms,
                    "coordinator term lease renewal rejected",
                    BTreeMap::from([
                        ("error".into(), error.to_string()),
                        ("term".into(), credential.term.to_string()),
                        (
                            "fencing_token".into(),
                            credential.fencing_token.0.to_string(),
                        ),
                    ]),
                )?;
                self.persist(&file)?;
                Err(error)
            }
        }
    }

    /// Issue or idempotently observe the bounded emergency runtime authority.
    /// `online_members` must come from the Coordinator's authenticated session
    /// snapshot and is never accepted from a client message.
    pub fn issue_emergency_leader(
        &self,
        actor: &NodeId,
        network_id: &NetworkId,
        online_members: &[NodeId],
        now_unix_ms: u64,
    ) -> Result<CoordinatorEmergencyLeaderResponse> {
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("coordinator_candidate.error.store_lock_poisoned"))?;
        let _profile_lock = lock_profile(&self.network_record_path, false)?;
        self.ensure_parent()?;
        let _file_lock = lock_candidate_file(&self.path)?;
        let mut file = self.load_unlocked()?;
        let network = read_network_record(&self.network_record_path, network_id)?;

        let outcome =
            self.apply_emergency_issue(&mut file, actor, &network, online_members, now_unix_ms);
        match outcome {
            Ok((response, changed)) => {
                if changed {
                    append_audit(
                        &mut file.audit_log,
                        actor,
                        &network.network_id,
                        "coordinator.emergency.leader.issued",
                        format!("network/{}/coordinator-emergency", network.network_id),
                        AuditOutcome::Succeeded,
                        now_unix_ms,
                        "bounded emergency runtime Leader issued",
                        emergency_audit_details(response.status.emergency_leader.as_ref()),
                    )?;
                    self.persist(&file)?;
                }
                Ok(response)
            }
            Err(error) => {
                append_audit(
                    &mut file.audit_log,
                    actor,
                    &network.network_id,
                    "coordinator.emergency.leader.issue.rejected",
                    format!("network/{}/coordinator-emergency", network.network_id),
                    audit_outcome_for_error(&error),
                    now_unix_ms,
                    "emergency runtime Leader issuance rejected",
                    BTreeMap::from([("error".into(), error.to_string())]),
                )?;
                self.persist(&file)?;
                Err(error)
            }
        }
    }

    /// Renew one live emergency lease. Candidate recovery or Leader disconnect
    /// revokes the lease in the same file-locked transaction as the rejection.
    pub fn renew_emergency_leader(
        &self,
        actor: &NodeId,
        credential: &CoordinatorEmergencyWriteCredential,
        network_id: &NetworkId,
        online_members: &[NodeId],
        now_unix_ms: u64,
    ) -> Result<CoordinatorEmergencyLeaderResponse> {
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("coordinator_candidate.error.store_lock_poisoned"))?;
        let _profile_lock = lock_profile(&self.network_record_path, false)?;
        self.ensure_parent()?;
        let _file_lock = lock_candidate_file(&self.path)?;
        let mut file = self.load_unlocked()?;
        let network = read_network_record(&self.network_record_path, network_id)?;

        let outcome = self.apply_emergency_renew(
            &mut file,
            actor,
            credential,
            &network,
            online_members,
            now_unix_ms,
        );
        match outcome {
            Ok(response) => {
                append_audit(
                    &mut file.audit_log,
                    actor,
                    &network.network_id,
                    "coordinator.emergency.leader.renewed",
                    format!("network/{}/coordinator-emergency", network.network_id),
                    AuditOutcome::Succeeded,
                    now_unix_ms,
                    "bounded emergency runtime Leader renewed",
                    emergency_audit_details(response.status.emergency_leader.as_ref()),
                )?;
                self.persist(&file)?;
                Ok(response)
            }
            Err(error) => {
                append_audit(
                    &mut file.audit_log,
                    actor,
                    &network.network_id,
                    "coordinator.emergency.leader.renew.rejected",
                    format!("network/{}/coordinator-emergency", network.network_id),
                    audit_outcome_for_error(&error),
                    now_unix_ms,
                    "emergency runtime Leader renewal rejected",
                    BTreeMap::from([
                        ("error".into(), error.to_string()),
                        (
                            "emergency_epoch".into(),
                            credential.emergency_epoch.to_string(),
                        ),
                        (
                            "candidate_config_revision".into(),
                            credential.candidate_config_revision.to_string(),
                        ),
                    ]),
                )?;
                if file
                    .networks
                    .get(network_id)
                    .is_some_and(|stored| stored.emergency_leader.is_none())
                    && error
                        .to_string()
                        .contains("domain.coordinator_emergency_candidate_online")
                {
                    append_audit(
                        &mut file.audit_log,
                        actor,
                        &network.network_id,
                        "coordinator.emergency.leader.revoked",
                        format!("network/{}/coordinator-emergency", network.network_id),
                        AuditOutcome::Succeeded,
                        now_unix_ms,
                        "emergency runtime Leader revoked after candidate recovery",
                        BTreeMap::from([
                            (
                                "reason_code".into(),
                                "coordinator.emergency.reason.candidate_recovered".into(),
                            ),
                            (
                                "emergency_epoch".into(),
                                credential.emergency_epoch.to_string(),
                            ),
                        ]),
                    )?;
                }
                self.persist(&file)?;
                Err(error)
            }
        }
    }

    /// Revoke emergency authority without changing the durable epoch. This is
    /// safe to call from the presence reconciler or an authenticated operator.
    pub fn revoke_emergency_leader(
        &self,
        actor: &NodeId,
        network_id: &NetworkId,
        reason_code: &str,
        now_unix_ms: u64,
    ) -> Result<CoordinatorCandidateStatus> {
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("coordinator_candidate.error.store_lock_poisoned"))?;
        let _profile_lock = lock_profile(&self.network_record_path, false)?;
        self.ensure_parent()?;
        let _file_lock = lock_candidate_file(&self.path)?;
        let mut file = self.load_unlocked()?;
        let network = read_network_record(&self.network_record_path, network_id)?;
        if !network.members.contains(actor) {
            bail!("coordinator_candidate.error.permission_denied");
        }

        let stored = file.networks.get_mut(network_id);
        let lease = stored
            .as_ref()
            .and_then(|stored| stored.emergency_leader.as_ref())
            .cloned();
        if let Some(lease) = lease {
            stored
                .expect("lease requires stored state")
                .emergency_leader = None;
            append_audit(
                &mut file.audit_log,
                actor,
                &network.network_id,
                "coordinator.emergency.leader.revoked",
                format!("network/{}/coordinator-emergency", network.network_id),
                AuditOutcome::Succeeded,
                now_unix_ms,
                "bounded emergency runtime Leader revoked",
                BTreeMap::from([
                    ("reason_code".into(), reason_code.into()),
                    ("emergency_epoch".into(), lease.emergency_epoch.to_string()),
                    ("leader".into(), lease.leader.to_string()),
                ]),
            )?;
            self.persist(&file)?;
        }
        status_from_file(&network, file.networks.get(network_id), now_unix_ms)
    }

    /// Reject and audit a long-lived governance mutation while an emergency
    /// runtime lease is active. The actor is already authenticated by the
    /// signaling layer; this gate deliberately does not require current
    /// membership because invitation redemption is a governance write by an
    /// invitee who is not yet a member. The caller must invoke this before
    /// touching a governance store; the candidate-store write path applies the
    /// same rule atomically under its own file lock.
    pub fn reject_emergency_governance_write(
        &self,
        actor: &NodeId,
        network_id: &NetworkId,
        action: &str,
        now_unix_ms: u64,
    ) -> Result<bool> {
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("coordinator_candidate.error.store_lock_poisoned"))?;
        let _profile_lock = lock_profile(&self.network_record_path, false)?;
        self.ensure_parent()?;
        let _file_lock = lock_candidate_file(&self.path)?;
        let mut file = self.load_unlocked()?;
        let lease = file
            .networks
            .get_mut(network_id)
            .and_then(|stored| stored.emergency_leader.as_mut());
        let Some(lease) = lease else {
            return Ok(false);
        };
        lease.refresh_at(now_unix_ms)?;
        if !lease.is_active() {
            return Ok(false);
        }

        append_emergency_governance_rejected(
            &mut file.audit_log,
            actor,
            network_id,
            action,
            lease,
            now_unix_ms,
        )?;
        self.persist(&file)?;
        Ok(true)
    }

    /// Central write gate for callers that already have a governance payload.
    /// Rejections are durably audited; the caller must not mutate its store when
    /// this method returns an error.
    pub fn authorize_governance_write(
        &self,
        actor: &NodeId,
        credential: &CoordinatorWriteCredential,
        write: CoordinatorWriteKind,
        now_unix_ms: u64,
    ) -> Result<CoordinatorTerm> {
        let network_id = credential.network_id.clone();
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("coordinator_candidate.error.store_lock_poisoned"))?;
        let _profile_lock = lock_profile(&self.network_record_path, false)?;
        self.ensure_parent()?;
        let _file_lock = lock_candidate_file(&self.path)?;
        let mut file = self.load_unlocked()?;
        let network = read_network_record(&self.network_record_path, &network_id)?;
        let stored = file.networks.get(&network_id);
        let result = stored
            .and_then(|stored| stored.latest_term.as_ref())
            .context("coordinator_term.error.missing")
            .and_then(|term| {
                term.authorize_write(actor, credential, write, now_unix_ms)?;
                Ok(term.clone())
            });
        if let Err(error) = &result {
            append_audit(
                &mut file.audit_log,
                actor,
                &network.network_id,
                "coordinator.term.write.rejected",
                format!("network/{}/coordinator-term", network.network_id),
                audit_outcome_for_error(error),
                now_unix_ms,
                "coordinator governance write rejected",
                BTreeMap::from([
                    ("error".into(), error.to_string()),
                    ("term".into(), credential.term.to_string()),
                    (
                        "fencing_token".into(),
                        credential.fencing_token.0.to_string(),
                    ),
                    ("write".into(), format!("{write:?}")),
                ]),
            )?;
            self.persist(&file)?;
        }
        result
    }

    /// Validate a rejoining node against the current authoritative snapshot and
    /// return a bounded audit replay. A stale term or snapshot is admitted only
    /// into `ReadOnly`; matching cursors produce `Rejoined`, which is still not
    /// a governance credential.
    pub fn recover(
        &self,
        actor: &NodeId,
        report: CoordinatorRecoveryReport,
        now_unix_ms: u64,
    ) -> Result<CoordinatorRecoveryResponse> {
        let network_id = report.network_id.clone();
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("coordinator_candidate.error.store_lock_poisoned"))?;
        let _profile_lock = lock_profile(&self.network_record_path, false)?;
        self.ensure_parent()?;
        let _file_lock = lock_candidate_file(&self.path)?;
        let mut file = self.load_unlocked()?;
        let network = read_network_record(&self.network_record_path, &network_id)?;
        let audit_sequence_before = network_audit_sequence(&file, &network_id);

        let current = (|| -> Result<(CoordinatorTerm, CoordinatorCandidateConfig)> {
            report.validate_definition()?;
            if !network.members.contains(actor) {
                bail!("coordinator_candidate.error.permission_denied");
            }
            let stored = file
                .networks
                .get(&network_id)
                .context("coordinator_term.error.missing")?;
            let term = stored
                .latest_term
                .as_ref()
                .context("coordinator_term.error.missing")?
                .clone();
            let config = stored
                .config
                .as_ref()
                .context("coordinator_term.error.config_missing")?
                .clone();
            config.validate_against_network(&network)?;
            term.validate_against_candidate_config(&config)?;
            if report.observed_audit_sequence > audit_sequence_before {
                bail!("coordinator_recovery.error.audit_cursor_ahead");
            }
            match report.observed_term {
                Some(observed_term) if observed_term > term.term => {
                    bail!("coordinator_term.error.not_installed");
                }
                Some(observed_term)
                    if observed_term == term.term
                        && (report.observed_fencing_token != Some(term.fencing_token)
                            || report.observed_leader.as_ref() != Some(&term.leader)) =>
                {
                    bail!("coordinator_term.error.conflict");
                }
                _ => {}
            }
            if report
                .observed_candidate_config_revision
                .is_some_and(|revision| revision > config.revision)
            {
                bail!("coordinator_candidate.error.revision_rollback");
            }
            Ok((term, config))
        })();
        let (term, config) = match current {
            Ok(current) => current,
            Err(error) => {
                append_audit(
                    &mut file.audit_log,
                    actor,
                    &network_id,
                    "coordinator.recovery.rejected",
                    format!("network/{network_id}/recovery/{actor}"),
                    audit_outcome_for_error(&error),
                    now_unix_ms,
                    "coordinator recovery validation rejected",
                    recovery_audit_details(actor, &report, None, None),
                )?;
                self.persist(&file)?;
                return Err(error);
            }
        };

        let snapshot_current = report.observed_term == Some(term.term)
            && report.observed_fencing_token == Some(term.fencing_token)
            && report.observed_leader.as_ref() == Some(&term.leader)
            && report.observed_candidate_config_revision == Some(config.revision);
        let audit_current = report.observed_audit_sequence == audit_sequence_before;
        let role = if config
            .candidates
            .iter()
            .any(|candidate| candidate.node_id == *actor)
        {
            CoordinatorRecoveryRole::Candidate
        } else {
            CoordinatorRecoveryRole::Member
        };
        let old_leader = report
            .observed_term
            .is_some_and(|observed| observed < term.term)
            && report.observed_leader.as_ref() == Some(actor);
        let (stage, reason_code) = if !snapshot_current {
            (
                CoordinatorRecoveryStage::ReadOnly,
                if old_leader {
                    "coordinator.recovery.stale_old_leader"
                } else {
                    "coordinator.recovery.snapshot_behind"
                },
            )
        } else if !audit_current {
            (
                CoordinatorRecoveryStage::CatchingUp,
                "coordinator.recovery.audit_behind",
            )
        } else {
            (
                CoordinatorRecoveryStage::Rejoined,
                if role == CoordinatorRecoveryRole::Candidate {
                    "coordinator.recovery.rejoined_candidate"
                } else {
                    "coordinator.recovery.rejoined_member"
                },
            )
        };

        let recovery_before_audit = CoordinatorRecoveryStatus {
            network_id: network_id.clone(),
            stage,
            role,
            governance_writable: false,
            health_checks_allowed: true,
            data_plane_allowed: true,
            authoritative_term: term.term,
            authoritative_fencing_token: term.fencing_token,
            authoritative_leader: term.leader.clone(),
            authoritative_candidate_config_revision: config.revision,
            authoritative_audit_sequence: audit_sequence_before,
            reason_code: reason_code.into(),
        };
        let previous = file
            .networks
            .get(&network_id)
            .and_then(|stored| stored.recoveries.get(actor));
        if previous != Some(&recovery_before_audit) {
            append_audit(
                &mut file.audit_log,
                actor,
                &network_id,
                if old_leader {
                    "coordinator.recovery.old_leader"
                } else {
                    "coordinator.recovery.validated"
                },
                format!("network/{network_id}/recovery/{actor}"),
                AuditOutcome::Succeeded,
                now_unix_ms,
                "coordinator recovery validated without restoring governance write authority",
                recovery_audit_details(actor, &report, Some(stage), Some(&term)),
            )?;
        }

        let audit_sequence_after = network_audit_sequence(&file, &network_id);
        let replay_start = usize::try_from(report.observed_audit_sequence)
            .context("coordinator_recovery.error.audit_cursor_overflow")?
            .min(audit_sequence_after as usize);
        let audit_entries = network_audit_entries(&file, &network_id)
            .skip(replay_start)
            .take(COORDINATOR_RECOVERY_AUDIT_REPLAY_LIMIT)
            .cloned()
            .collect::<Vec<_>>();
        let recovered_through = report
            .observed_audit_sequence
            .saturating_add(audit_entries.len() as u64);
        let mut recovery = recovery_before_audit;
        recovery.authoritative_audit_sequence = recovered_through;
        file.networks
            .entry(network_id.clone())
            .or_default()
            .recoveries
            .insert(actor.clone(), recovery.clone());
        let status = status_from_file(&network, file.networks.get(&network_id), now_unix_ms)?;
        self.persist(&file)?;
        Ok(CoordinatorRecoveryResponse {
            status,
            recovery,
            audit_entries,
        })
    }

    pub fn update_config(
        &self,
        actor: &NodeId,
        expected_revision: u64,
        config: CoordinatorCandidateConfig,
        now_unix_ms: u64,
    ) -> Result<CoordinatorCandidateStatus> {
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("coordinator_candidate.error.store_lock_poisoned"))?;
        self.ensure_parent()?;
        let _governance_gate = lock_governance_gate(&self.path)?;
        let _profile_lock = lock_profile(&self.network_record_path, false)?;
        let _file_lock = lock_candidate_file(&self.path)?;
        let mut file = self.load_unlocked()?;
        let network = read_network_record(&self.network_record_path, &config.network_id)?;

        let outcome = self.apply_config_update(
            &mut file,
            ConfigUpdate {
                actor,
                expected_revision,
                config,
                credential: None,
                network: &network,
                now_unix_ms,
            },
        );
        match outcome {
            Ok(status) => {
                append_audit(
                    &mut file.audit_log,
                    actor,
                    &network.network_id,
                    "coordinator.candidate.config.update",
                    format!("network/{}/coordinator-candidates", network.network_id),
                    AuditOutcome::Succeeded,
                    now_unix_ms,
                    "coordinator candidate configuration updated",
                    BTreeMap::from([
                        (
                            "revision".into(),
                            status
                                .config
                                .as_ref()
                                .map(|config| config.revision.to_string())
                                .unwrap_or_else(|| "0".into()),
                        ),
                        ("expected_revision".into(), expected_revision.to_string()),
                    ]),
                )?;
                self.persist(&file)?;
                Ok(status)
            }
            Err(error) => {
                append_audit(
                    &mut file.audit_log,
                    actor,
                    &network.network_id,
                    "coordinator.candidate.config.update",
                    format!("network/{}/coordinator-candidates", network.network_id),
                    audit_outcome_for_error(&error),
                    now_unix_ms,
                    "coordinator candidate configuration rejected",
                    BTreeMap::from([
                        ("expected_revision".into(), expected_revision.to_string()),
                        ("error".into(), error.to_string()),
                    ]),
                )?;
                self.persist(&file)?;
                Err(error)
            }
        }
    }

    /// Post-election candidate configuration write. Unlike the bootstrap path,
    /// this method requires an authoritative term and fencing token.
    pub fn update_config_authorized(
        &self,
        actor: &NodeId,
        expected_revision: u64,
        config: CoordinatorCandidateConfig,
        credential: &CoordinatorWriteCredential,
        now_unix_ms: u64,
    ) -> Result<CoordinatorCandidateStatus> {
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("coordinator_candidate.error.store_lock_poisoned"))?;
        let _profile_lock = lock_profile(&self.network_record_path, false)?;
        self.ensure_parent()?;
        let _file_lock = lock_candidate_file(&self.path)?;
        let mut file = self.load_unlocked()?;
        let network = read_network_record(&self.network_record_path, &config.network_id)?;

        let outcome = self.apply_config_update(
            &mut file,
            ConfigUpdate {
                actor,
                expected_revision,
                config,
                credential: Some(credential),
                network: &network,
                now_unix_ms,
            },
        );
        match outcome {
            Ok(status) => {
                append_audit(
                    &mut file.audit_log,
                    actor,
                    &network.network_id,
                    "coordinator.candidate.config.update.authorized",
                    format!("network/{}/coordinator-candidates", network.network_id),
                    AuditOutcome::Succeeded,
                    now_unix_ms,
                    "coordinator candidate configuration updated under term authority",
                    BTreeMap::from([
                        (
                            "revision".into(),
                            status
                                .config
                                .as_ref()
                                .map(|config| config.revision.to_string())
                                .unwrap_or_else(|| "0".into()),
                        ),
                        ("expected_revision".into(), expected_revision.to_string()),
                        ("term".into(), credential.term.to_string()),
                        (
                            "fencing_token".into(),
                            credential.fencing_token.0.to_string(),
                        ),
                    ]),
                )?;
                self.persist(&file)?;
                Ok(status)
            }
            Err(error) => {
                append_audit(
                    &mut file.audit_log,
                    actor,
                    &network.network_id,
                    "coordinator.candidate.config.update.authorized",
                    format!("network/{}/coordinator-candidates", network.network_id),
                    audit_outcome_for_error(&error),
                    now_unix_ms,
                    "coordinator candidate configuration update rejected",
                    BTreeMap::from([
                        ("expected_revision".into(), expected_revision.to_string()),
                        ("term".into(), credential.term.to_string()),
                        (
                            "fencing_token".into(),
                            credential.fencing_token.0.to_string(),
                        ),
                        ("error".into(), error.to_string()),
                    ]),
                )?;
                self.persist(&file)?;
                Err(error)
            }
        }
    }

    pub fn publish_declaration(
        &self,
        actor: &NodeId,
        declaration: CoordinatorCandidateDeclaration,
        now_unix_ms: u64,
    ) -> Result<CoordinatorCandidateStatus> {
        let network_id = declaration.network_id.clone();
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("coordinator_candidate.error.store_lock_poisoned"))?;
        self.ensure_parent()?;
        let _governance_gate = lock_governance_gate(&self.path)?;
        let _profile_lock = lock_profile(&self.network_record_path, false)?;
        let _file_lock = lock_candidate_file(&self.path)?;
        let mut file = self.load_unlocked()?;
        let network = read_network_record(&self.network_record_path, &network_id)?;

        let outcome = self.apply_declaration(&mut file, actor, declaration, &network, now_unix_ms);
        match outcome {
            Ok(status) => {
                append_audit(
                    &mut file.audit_log,
                    actor,
                    &network.network_id,
                    "coordinator.candidate.declaration.publish",
                    format!("network/{}/candidate/{}", network.network_id, actor),
                    AuditOutcome::Succeeded,
                    now_unix_ms,
                    "coordinator candidate declaration published",
                    BTreeMap::from([(
                        "config_revision".into(),
                        status
                            .config
                            .as_ref()
                            .map(|config| config.revision.to_string())
                            .unwrap_or_else(|| "0".into()),
                    )]),
                )?;
                self.persist(&file)?;
                Ok(status)
            }
            Err(error) => {
                append_audit(
                    &mut file.audit_log,
                    actor,
                    &network.network_id,
                    "coordinator.candidate.declaration.publish",
                    format!("network/{}/candidate/{}", network.network_id, actor),
                    audit_outcome_for_error(&error),
                    now_unix_ms,
                    "coordinator candidate declaration rejected",
                    BTreeMap::from([("error".into(), error.to_string())]),
                )?;
                self.persist(&file)?;
                Err(error)
            }
        }
    }

    pub fn status(
        &self,
        actor: &NodeId,
        network_id: &NetworkId,
        now_unix_ms: u64,
    ) -> Result<CoordinatorCandidateStatus> {
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("coordinator_candidate.error.store_lock_poisoned"))?;
        let _profile_lock = lock_profile(&self.network_record_path, false)?;
        self.ensure_parent()?;
        let _file_lock = lock_candidate_file(&self.path)?;
        let file = self.load_unlocked()?;
        let network = read_network_record(&self.network_record_path, network_id)?;
        if !network.members.contains(actor) {
            bail!("coordinator_candidate.error.permission_denied");
        }
        status_from_file(&network, file.networks.get(network_id), now_unix_ms)
    }

    /// Authoritative write gate for governance operations.
    ///
    /// This deliberately does not require the caller to be a member: invitation
    /// redemption and other admission bridges must execute the same freeze
    /// check before they add the caller to the roster. A missing candidate file
    /// is treated as `NoCandidatesConfigured`, so callers fail closed whenever
    /// candidate governance is attached.
    pub fn ensure_governance_allowed(
        &self,
        network_id: &NetworkId,
        now_unix_ms: u64,
    ) -> Result<()> {
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("coordinator_candidate.error.store_lock_poisoned"))?;
        self.ensure_parent()?;
        let _governance_gate = lock_governance_gate(&self.path)?;
        self.ensure_governance_allowed_locked(network_id, now_unix_ms)
    }

    /// Run one governance write while excluding candidate configuration and
    /// declaration changes.
    ///
    /// The candidate state is re-read after taking the gate and the gate stays
    /// held until `write` returns. Candidate updates therefore cannot turn an
    /// allowed check into a frozen write (or vice versa).
    pub(crate) fn with_governance_allowed<T>(
        &self,
        network_id: &NetworkId,
        now_unix_ms: u64,
        write: impl FnOnce(&GovernancePermit<'_>) -> Result<T>,
    ) -> Result<T> {
        let process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("coordinator_candidate.error.store_lock_poisoned"))?;
        self.ensure_parent()?;
        let gate = lock_governance_gate(&self.path)?;
        self.ensure_governance_allowed_locked(network_id, now_unix_ms)?;
        let permit = GovernancePermit {
            _process_guard: process_guard,
            _gate: gate,
        };
        write(&permit)
    }

    fn ensure_governance_allowed_locked(
        &self,
        network_id: &NetworkId,
        now_unix_ms: u64,
    ) -> Result<()> {
        let _profile_lock = lock_profile(&self.network_record_path, false)?;
        let _file_lock = lock_candidate_file(&self.path)?;
        let file = self.load_unlocked()?;
        let network = read_network_record(&self.network_record_path, network_id)?;
        let status = status_from_file(&network, file.networks.get(network_id), now_unix_ms)?;
        if status.emergency != fuckipv6_common::domain::CoordinatorEmergencyState::None {
            tracing::warn!(
                %network_id,
                emergency = ?status.emergency,
                "Coordinator candidate state has frozen governance writes"
            );
            bail!(GOVERNANCE_FROZEN_ERROR);
        }
        Ok(())
    }

    pub(crate) fn notification_recipients(&self, network_id: &NetworkId) -> Result<Vec<NodeId>> {
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("coordinator_candidate.error.store_lock_poisoned"))?;
        let _profile_lock = lock_profile(&self.network_record_path, false)?;
        let network = read_network_record(&self.network_record_path, network_id)?;
        Ok(network.members)
    }

    /// Intersect the session-derived presence snapshot with the authoritative
    /// network roster. A missing session is offline, and a session for a node
    /// outside this network can never justify emergency authority.
    pub(crate) fn emergency_online_members(
        &self,
        network_id: &NetworkId,
        presence: &BTreeMap<NodeId, NetworkMemberPresence>,
    ) -> Result<Vec<NodeId>> {
        let _profile_lock = lock_profile(&self.network_record_path, false)?;
        let network = read_network_record(&self.network_record_path, network_id)?;
        Ok(emergency_online_members_for_network(&network, presence))
    }

    /// Reconcile every persisted emergency lease against one authenticated
    /// presence snapshot. Only leases whose authority has become invalid are
    /// removed, so an idle ticker does not create repeated audit records.
    pub(crate) fn reconcile_emergency_leases(
        &self,
        presence: &BTreeMap<NodeId, NetworkMemberPresence>,
        now_unix_ms: u64,
    ) -> Result<Vec<NetworkId>> {
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("coordinator_candidate.error.store_lock_poisoned"))?;
        let _profile_lock = lock_profile(&self.network_record_path, false)?;
        self.ensure_parent()?;
        let _file_lock = lock_candidate_file(&self.path)?;
        let mut file = self.load_unlocked()?;
        let leased_networks = file
            .networks
            .iter()
            .filter(|(_, stored)| stored.emergency_leader.is_some())
            .map(|(network_id, _)| network_id.clone())
            .collect::<Vec<_>>();
        let mut changed = Vec::new();

        for network_id in leased_networks {
            let network = read_network_record(&self.network_record_path, &network_id)?;
            let online_members = emergency_online_members_for_network(&network, presence);
            let stored = file
                .networks
                .get_mut(&network_id)
                .expect("leased network must remain in candidate file");
            let lease = stored
                .emergency_leader
                .as_ref()
                .expect("leased network must retain emergency lease")
                .clone();
            let mut refreshed = lease.clone();
            refreshed.refresh_at(now_unix_ms)?;

            let config = stored.config.as_ref();
            let candidate_recovered = config.is_some_and(|config| {
                config
                    .candidates
                    .iter()
                    .any(|candidate| online_members.contains(&candidate.node_id))
            });
            let leader_offline =
                !network.members.contains(&lease.leader) || !online_members.contains(&lease.leader);
            let config_unavailable =
                config.is_none_or(|config| !config.is_effective_at(now_unix_ms));
            let context_invalid = config
                .is_some_and(|config| lease.validate_against_context(&network, config).is_err());
            let reason_code = if candidate_recovered {
                Some("coordinator.emergency.reason.candidate_recovered")
            } else if leader_offline {
                Some("coordinator.emergency.reason.leader_offline")
            } else if now_unix_ms >= lease.hard_expires_at_unix_ms {
                Some("coordinator.emergency.reason.hard_deadline_reached")
            } else if config_unavailable {
                Some("coordinator.emergency.reason.configuration_unavailable")
            } else if now_unix_ms >= lease.expires_at_unix_ms {
                Some("coordinator.emergency.reason.lease_expired")
            } else if context_invalid {
                Some("coordinator.emergency.reason.context_invalid")
            } else {
                None
            };
            let Some(reason_code) = reason_code else {
                continue;
            };

            stored.emergency_leader = None;
            append_audit(
                &mut file.audit_log,
                &emergency_reconciler_actor(),
                &network.network_id,
                "coordinator.emergency.leader.revoked",
                format!("network/{}/coordinator-emergency", network.network_id),
                AuditOutcome::Succeeded,
                now_unix_ms,
                "emergency runtime Leader revoked by presence reconciliation",
                BTreeMap::from([
                    ("actor_role".into(), "system".into()),
                    (
                        "observation".into(),
                        "authenticated_session_presence".into(),
                    ),
                    ("reason_code".into(), reason_code.into()),
                    ("emergency_epoch".into(), lease.emergency_epoch.to_string()),
                    ("leader".into(), lease.leader.to_string()),
                ]),
            )?;
            changed.push(network_id);
        }

        if !changed.is_empty() {
            self.persist(&file)?;
        }
        Ok(changed)
    }

    /// Election events are control-plane traffic for current candidates only.
    /// Membership is intersected again because a candidate may have been
    /// removed after the administrator-signed configuration was admitted.
    pub(crate) fn election_notification_recipients(
        &self,
        network_id: &NetworkId,
    ) -> Result<Vec<NodeId>> {
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("coordinator_candidate.error.store_lock_poisoned"))?;
        let _profile_lock = lock_profile(&self.network_record_path, false)?;
        self.ensure_parent()?;
        let _file_lock = lock_candidate_file(&self.path)?;
        let file = self.load_unlocked()?;
        let network = read_network_record(&self.network_record_path, network_id)?;
        let candidates = file
            .networks
            .get(network_id)
            .and_then(|stored| stored.config.as_ref())
            .map(|config| config.candidates.as_slice())
            .unwrap_or_default();
        Ok(candidates
            .iter()
            .map(|candidate| candidate.node_id.clone())
            .filter(|candidate| network.members.contains(candidate))
            .collect())
    }

    pub fn audit_entries(&self) -> Result<Vec<AuditEntry>> {
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("coordinator_candidate.error.store_lock_poisoned"))?;
        self.ensure_parent()?;
        let _file_lock = lock_candidate_file(&self.path)?;
        Ok(self.load_unlocked()?.audit_log.entries().to_vec())
    }

    fn apply_emergency_issue(
        &self,
        file: &mut CoordinatorCandidateFile,
        actor: &NodeId,
        network: &NetworkRecord,
        online_members: &[NodeId],
        now_unix_ms: u64,
    ) -> Result<(CoordinatorEmergencyLeaderResponse, bool)> {
        if !network.members.contains(actor) {
            bail!("coordinator_candidate.error.permission_denied");
        }
        let stored = file.networks.entry(network.network_id.clone()).or_default();
        let config = stored
            .config
            .as_ref()
            .context("coordinator_candidate.error.config_missing")?
            .clone();
        config.validate_against_network(network)?;
        let online = online_members
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();

        if let Some(existing) = stored.emergency_leader.clone() {
            let mut refreshed = existing;
            refreshed.refresh_at(now_unix_ms)?;
            if refreshed.is_active() && online.contains(&refreshed.leader) {
                if stored
                    .latest_term
                    .as_ref()
                    .is_some_and(|term| term.lease_status_at(now_unix_ms).is_writable())
                {
                    bail!("coordinator_candidate.error.emergency_term_conflict");
                }
                let status = status_from_file(network, Some(stored), now_unix_ms)?;
                return Ok((
                    CoordinatorEmergencyLeaderResponse {
                        status,
                        renewed: false,
                    },
                    false,
                ));
            }
        }

        if stored
            .latest_term
            .as_ref()
            .is_some_and(|term| term.lease_status_at(now_unix_ms).is_writable())
        {
            bail!("coordinator_candidate.error.emergency_term_conflict");
        }
        let emergency_epoch = stored
            .emergency_epoch
            .checked_add(1)
            .context("coordinator_candidate.error.emergency_epoch_overflow")?;
        let lease = CoordinatorEmergencyLeaderStatus::issue(
            network,
            &config,
            online_members,
            emergency_epoch,
            now_unix_ms,
        )?;
        stored.emergency_epoch = emergency_epoch;
        stored.emergency_leader = Some(lease);
        let status = status_from_file(network, Some(stored), now_unix_ms)?;
        Ok((
            CoordinatorEmergencyLeaderResponse {
                status,
                renewed: false,
            },
            true,
        ))
    }

    fn apply_emergency_renew(
        &self,
        file: &mut CoordinatorCandidateFile,
        actor: &NodeId,
        credential: &CoordinatorEmergencyWriteCredential,
        network: &NetworkRecord,
        online_members: &[NodeId],
        now_unix_ms: u64,
    ) -> Result<CoordinatorEmergencyLeaderResponse> {
        if !network.members.contains(actor) {
            bail!("coordinator_candidate.error.permission_denied");
        }
        if credential.network_id != network.network_id {
            bail!("coordinator_candidate.error.emergency_context_mismatch");
        }
        let stored = file
            .networks
            .get_mut(&network.network_id)
            .context("coordinator_candidate.error.emergency_missing")?;
        let config = stored
            .config
            .as_ref()
            .context("coordinator_candidate.error.config_missing")?
            .clone();
        let mut lease = stored
            .emergency_leader
            .as_ref()
            .context("coordinator_candidate.error.emergency_missing")?
            .clone();
        lease.refresh_at(now_unix_ms)?;

        let online = online_members
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        if config
            .candidates
            .iter()
            .any(|candidate| online.contains(&candidate.node_id))
        {
            stored.emergency_leader = None;
            bail!("domain.coordinator_emergency_candidate_online");
        }
        if credential.leader != lease.leader
            || credential.emergency_epoch != lease.emergency_epoch
            || credential.candidate_config_revision != config.revision
            || actor != &lease.leader
        {
            bail!("domain.coordinator_emergency_credential_mismatch");
        }
        if !online.contains(&lease.leader) {
            stored.emergency_leader = None;
            bail!("domain.coordinator_emergency_leader_offline");
        }
        let unavailable = config
            .candidates
            .iter()
            .map(|candidate| candidate.node_id.clone())
            .collect::<Vec<_>>();
        lease.renew(config.revision, online_members, &unavailable, now_unix_ms)?;
        stored.emergency_leader = Some(lease);
        let status = status_from_file(network, Some(stored), now_unix_ms)?;
        Ok(CoordinatorEmergencyLeaderResponse {
            status,
            renewed: true,
        })
    }

    fn apply_election_proposal(
        &self,
        file: &mut CoordinatorCandidateFile,
        actor: &NodeId,
        proposal: CoordinatorElectionProposal,
        network: &NetworkRecord,
        now_unix_ms: u64,
    ) -> Result<CoordinatorCandidateStatus> {
        proposal.validate_definition()?;
        if proposal.proposer != *actor {
            bail!("coordinator_candidate.error.authenticated_actor_mismatch");
        }
        if proposal.network_id != network.network_id {
            bail!("coordinator_candidate.error.network_mismatch");
        }
        if !network.members.contains(actor) {
            bail!("coordinator_candidate.error.permission_denied");
        }

        let stored = file.networks.entry(network.network_id.clone()).or_default();
        let config = stored
            .config
            .clone()
            .context("coordinator_candidate.error.config_missing")?;
        let declarations = stored.declarations.values().cloned().collect::<Vec<_>>();
        proposal.validate_for_election(
            network,
            &config,
            &declarations,
            stored.latest_term.as_ref(),
            now_unix_ms,
        )?;

        if let Some(existing) = &stored.latest_election {
            if existing.proposal.proposal_id == proposal.proposal_id {
                return status_from_file(network, Some(stored), now_unix_ms);
            }
            if matches!(
                existing.stage,
                CoordinatorElectionStage::Started | CoordinatorElectionStage::Voting
            ) {
                bail!("coordinator_candidate.error.election_in_progress");
            }
        }

        let mut session = CoordinatorElectionSession::started(proposal, now_unix_ms)?;
        session.begin_voting(now_unix_ms)?;
        stored.latest_election = Some(session);
        status_from_file(network, Some(stored), now_unix_ms)
    }

    fn apply_election_vote(
        &self,
        file: &mut CoordinatorCandidateFile,
        actor: &NodeId,
        proposal_id: &ContentHash,
        vote: CoordinatorElectionVote,
        network: &NetworkRecord,
        now_unix_ms: u64,
    ) -> Result<(CoordinatorCandidateStatus, bool)> {
        vote.validate_definition()?;
        if vote.voter != *actor {
            bail!("coordinator_candidate.error.authenticated_actor_mismatch");
        }
        if vote.network_id != network.network_id {
            bail!("coordinator_candidate.error.network_mismatch");
        }
        if !network.members.contains(actor) {
            bail!("coordinator_candidate.error.permission_denied");
        }

        let stored = file.networks.entry(network.network_id.clone()).or_default();
        let config = stored
            .config
            .as_ref()
            .context("coordinator_candidate.error.config_missing")?;
        let session = stored
            .latest_election
            .as_ref()
            .context("coordinator_candidate.error.election_missing")?;
        if session.proposal.proposal_id != *proposal_id {
            bail!("coordinator_candidate.error.election_proposal_mismatch");
        }
        if session.proposal.candidate_config_revision != config.revision {
            bail!("coordinator_candidate.error.election_config_stale");
        }

        let mut updated = session.clone();
        updated.record_vote(vote, now_unix_ms)?;
        let mut committed = false;
        if updated.stage == CoordinatorElectionStage::Voting
            && updated.votes.len() >= updated.proposal.majority_threshold()
        {
            let fencing_token = stored
                .latest_term
                .as_ref()
                .map(|term| {
                    term.fencing_token
                        .0
                        .checked_add(1)
                        .context("coordinator_term.error.fencing_token_overflow")
                })
                .transpose()?
                .unwrap_or(1);
            let mut candidate = updated.clone();
            let term = candidate.commit(
                fuckipv6_common::domain::FencingToken(fencing_token),
                now_unix_ms,
            )?;
            term.validate_against_candidate_config(config)?;
            if let Some(current) = &stored.latest_term {
                current.validate_successor(&term, now_unix_ms)?;
            }
            updated = candidate;
            stored.latest_term = Some(term);
            committed = true;
        }
        stored.latest_election = Some(updated);
        let status = status_from_file(network, Some(stored), now_unix_ms)?;
        Ok((status, committed))
    }

    fn apply_election_timeout(
        &self,
        file: &mut CoordinatorCandidateFile,
        actor: &NodeId,
        proposal_id: &ContentHash,
        network: &NetworkRecord,
        now_unix_ms: u64,
    ) -> Result<(CoordinatorCandidateStatus, bool)> {
        if !network.members.contains(actor) {
            bail!("coordinator_candidate.error.permission_denied");
        }
        let stored = file.networks.entry(network.network_id.clone()).or_default();
        let session = stored
            .latest_election
            .as_ref()
            .context("coordinator_candidate.error.election_missing")?;
        if session.proposal.proposal_id != *proposal_id {
            bail!("coordinator_candidate.error.election_proposal_mismatch");
        }
        if !session.participants.contains(actor) {
            bail!("coordinator_candidate.error.permission_denied");
        }
        if session.stage == CoordinatorElectionStage::TimedOut {
            return Ok((status_from_file(network, Some(stored), now_unix_ms)?, false));
        }

        let mut updated = session.clone();
        if updated.stage == CoordinatorElectionStage::Started {
            updated.begin_voting(now_unix_ms)?;
        }
        updated.timed_out(now_unix_ms)?;
        stored.latest_election = Some(updated);
        Ok((status_from_file(network, Some(stored), now_unix_ms)?, true))
    }

    fn apply_config_update(
        &self,
        file: &mut CoordinatorCandidateFile,
        update: ConfigUpdate<'_>,
    ) -> Result<CoordinatorCandidateStatus> {
        let ConfigUpdate {
            actor,
            expected_revision,
            config,
            credential,
            network,
            now_unix_ms,
        } = update;
        config.validate_against_network(network)?;
        if config.signer != *actor {
            bail!("coordinator_candidate.error.authenticated_actor_mismatch");
        }
        if config.expires_at_unix_ms <= now_unix_ms {
            bail!("coordinator_candidate.error.config_expired");
        }

        let stored = file.networks.entry(network.network_id.clone()).or_default();
        match (&stored.latest_term, credential) {
            (None, None) => {}
            (Some(term), Some(credential)) => {
                term.authorize_write(
                    actor,
                    credential,
                    CoordinatorWriteKind::CandidateSet,
                    now_unix_ms,
                )?;
            }
            (Some(_), None) => bail!("coordinator_term.error.write_requires_term"),
            (None, Some(_)) => bail!("coordinator_term.error.missing"),
        }
        if let Some(lease) = stored.emergency_leader.as_mut() {
            lease.refresh_at(now_unix_ms)?;
            if lease.is_active() {
                let lease = lease.clone();
                append_emergency_governance_rejected(
                    &mut file.audit_log,
                    actor,
                    &network.network_id,
                    "candidate_config_update",
                    &lease,
                    now_unix_ms,
                )?;
                bail!("domain.coordinator_emergency_governance_forbidden");
            }
        }
        let current_revision = stored
            .config
            .as_ref()
            .map(|config| config.revision)
            .unwrap_or(0);
        if config.revision == current_revision {
            if stored.config.as_ref() == Some(&config) {
                return status_from_file(network, Some(stored), now_unix_ms);
            }
            bail!("coordinator_candidate.error.revision_conflict");
        }
        if config.revision < current_revision {
            bail!("coordinator_candidate.error.revision_rollback");
        }
        if expected_revision != current_revision {
            bail!("coordinator_candidate.error.stale_revision");
        }
        let next_revision = current_revision
            .checked_add(1)
            .context("coordinator_candidate.error.revision_overflow")?;
        if config.revision != next_revision {
            bail!("coordinator_candidate.error.revision_gap");
        }

        // A declaration for the previous revision cannot be promoted to the new
        // configuration. Keeping it would make offline status look fresh.
        stored.declarations.clear();
        // Emergency proof is bound to the exact candidate revision. A new
        // configuration invalidates it even when the candidate identities match.
        stored.emergency_leader = None;
        stored.config = Some(config);
        let status = status_from_file(network, Some(stored), now_unix_ms)?;
        self.persist(file)?;
        Ok(status)
    }

    fn apply_term_install(
        &self,
        file: &mut CoordinatorCandidateFile,
        actor: &NodeId,
        term: CoordinatorTerm,
        network: &NetworkRecord,
        now_unix_ms: u64,
    ) -> Result<CoordinatorCandidateStatus> {
        if !network.members.contains(actor) {
            bail!("coordinator_term.error.permission_denied");
        }
        if term.leader != *actor {
            bail!("coordinator_term.error.leader_mismatch");
        }
        term.validate_at(now_unix_ms)
            .with_context(|| "coordinator_term.error.invalid")?;
        let lease = term.lease_status_at(now_unix_ms);
        if !lease.is_writable() {
            bail!(
                "{}",
                lease
                    .reason_code
                    .replace("coordinator.lease", "coordinator_term.error")
            );
        }
        if term.expires_at_unix_ms.saturating_sub(now_unix_ms) > COORDINATOR_MAX_LEASE_DURATION_MS {
            bail!("coordinator_term.error.lease_too_long");
        }

        let stored = file
            .networks
            .get_mut(&network.network_id)
            .context("coordinator_term.error.config_missing")?;
        let config = stored
            .config
            .as_ref()
            .context("coordinator_term.error.config_missing")?;
        config.validate_against_network(network)?;
        if now_unix_ms < config.effective_from_unix_ms || now_unix_ms >= config.expires_at_unix_ms {
            bail!("coordinator_term.error.config_not_current");
        }
        term.validate_against_candidate_config(config)?;

        if let Some(current) = &stored.latest_term {
            if current == &term {
                stored.emergency_leader = None;
                return status_from_file(network, Some(stored), now_unix_ms);
            }
            current.validate_successor(&term, now_unix_ms)?;
        }
        stored.emergency_leader = None;
        stored.latest_term = Some(term);
        let status = status_from_file(network, Some(stored), now_unix_ms)?;
        self.persist(file)?;
        Ok(status)
    }

    fn apply_term_renewal(
        &self,
        file: &mut CoordinatorCandidateFile,
        actor: &NodeId,
        credential: &CoordinatorWriteCredential,
        new_expires_at_unix_ms: u64,
        network: &NetworkRecord,
        now_unix_ms: u64,
    ) -> Result<CoordinatorCandidateStatus> {
        let stored = file
            .networks
            .get_mut(&network.network_id)
            .context("coordinator_term.error.missing")?;
        let current = stored
            .latest_term
            .as_ref()
            .context("coordinator_term.error.missing")?;
        current.authorize_write(
            actor,
            credential,
            CoordinatorWriteKind::PublishOwnTerm,
            now_unix_ms,
        )?;
        let config = stored
            .config
            .as_ref()
            .context("coordinator_term.error.config_missing")?;
        current.validate_against_candidate_config(config)?;
        if new_expires_at_unix_ms <= current.expires_at_unix_ms {
            bail!("coordinator_term.error.lease_not_extended");
        }
        let maximum_expiry = now_unix_ms
            .checked_add(COORDINATOR_MAX_LEASE_DURATION_MS)
            .context("coordinator_term.error.lease_overflow")?;
        if new_expires_at_unix_ms > maximum_expiry {
            bail!("coordinator_term.error.lease_too_long");
        }
        let election_expiry = current
            .majority_votes
            .iter()
            .map(|vote| vote.expires_at_unix_ms)
            .min()
            .context("coordinator_term.error.election_vote_missing")?;
        if new_expires_at_unix_ms > election_expiry {
            bail!("coordinator_term.error.election_vote_expiry");
        }

        let mut renewed = current.clone();
        renewed.expires_at_unix_ms = new_expires_at_unix_ms;
        renewed.validate_definition()?;
        stored.latest_term = Some(renewed);
        let status = status_from_file(network, Some(stored), now_unix_ms)?;
        self.persist(file)?;
        Ok(status)
    }

    fn apply_declaration(
        &self,
        file: &mut CoordinatorCandidateFile,
        actor: &NodeId,
        declaration: CoordinatorCandidateDeclaration,
        network: &NetworkRecord,
        now_unix_ms: u64,
    ) -> Result<CoordinatorCandidateStatus> {
        declaration.validate_definition()?;
        if declaration.node_id != *actor {
            bail!("coordinator_candidate.error.authenticated_actor_mismatch");
        }
        if declaration.network_id != network.network_id {
            bail!("coordinator_candidate.error.network_mismatch");
        }
        if !network.members.contains(actor) {
            bail!("coordinator_candidate.error.permission_denied");
        }
        if now_unix_ms >= declaration.expires_at_unix_ms {
            bail!("coordinator_candidate.error.declaration_expired");
        }
        if declaration.observed_at_unix_ms
            > now_unix_ms.saturating_add(
                fuckipv6_common::domain::COORDINATOR_CANDIDATE_DECLARATION_FUTURE_SKEW_MS,
            )
        {
            bail!("coordinator_candidate.error.declaration_from_future");
        }

        let stored = file.networks.entry(network.network_id.clone()).or_default();
        let config = stored
            .config
            .as_ref()
            .context("coordinator_candidate.error.config_missing")?;
        if declaration.candidate_config_revision != config.revision {
            bail!("coordinator_candidate.error.declaration_revision_mismatch");
        }
        if !config
            .candidates
            .iter()
            .any(|candidate| candidate.node_id == *actor)
        {
            bail!("coordinator_candidate.error.declaration_not_candidate");
        }
        stored.declarations.insert(actor.clone(), declaration);
        let status = status_from_file(network, Some(stored), now_unix_ms)?;
        self.persist(file)?;
        Ok(status)
    }

    fn ensure_parent(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("无法创建协调器候选目录: {}", parent.display()))?;
        }
        Ok(())
    }

    fn load_unlocked(&self) -> Result<CoordinatorCandidateFile> {
        if !self.path.exists() {
            return Ok(CoordinatorCandidateFile::default());
        }
        let content = std::fs::read(&self.path)
            .with_context(|| format!("无法读取协调器候选存储: {}", self.path.display()))?;
        let mut file: CoordinatorCandidateFile = serde_json::from_slice(&content)
            .with_context(|| format!("无法解析协调器候选存储: {}", self.path.display()))?;
        if matches!(
            file.schema_version,
            COORDINATOR_CANDIDATE_FILE_SCHEMA_VERSION_V1
                | COORDINATOR_CANDIDATE_FILE_SCHEMA_VERSION_V2
        ) {
            file.schema_version = COORDINATOR_CANDIDATE_FILE_SCHEMA_VERSION;
        }
        file.validate()?;
        Ok(file)
    }

    fn persist(&self, file: &CoordinatorCandidateFile) -> Result<()> {
        file.validate()?;
        let content = serde_json::to_vec_pretty(file).context("无法序列化协调器候选存储")?;
        persist_bytes_atomic(&self.path, &content)
    }
}

fn status_from_file(
    network: &NetworkRecord,
    stored: Option<&StoredCoordinatorCandidates>,
    now_unix_ms: u64,
) -> Result<CoordinatorCandidateStatus> {
    let config = stored.and_then(|stored| stored.config.as_ref());
    let latest_term = stored.and_then(|stored| stored.latest_term.as_ref());
    let declarations = stored
        .map(|stored| stored.declarations.values().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let mut status = CoordinatorCandidateStatus::assess(
        network,
        config,
        &declarations,
        latest_term,
        now_unix_ms,
    )
    .context("coordinator_candidate.error.status_projection_failed")?;
    status.latest_election = stored.and_then(|stored| stored.latest_election.clone());
    if config.is_some_and(|config| config.is_effective_at(now_unix_ms)) {
        status.attach_emergency_leader(
            stored.and_then(|stored| stored.emergency_leader.clone()),
            now_unix_ms,
        )?;
    }
    status.refresh_control_plane();
    Ok(status)
}

fn network_audit_entries<'a>(
    file: &'a CoordinatorCandidateFile,
    network_id: &'a NetworkId,
) -> impl Iterator<Item = &'a AuditEntry> + 'a {
    file.audit_log
        .entries()
        .iter()
        .filter(move |entry| entry.network_id.as_ref() == Some(network_id))
}

fn network_audit_sequence(file: &CoordinatorCandidateFile, network_id: &NetworkId) -> u64 {
    network_audit_entries(file, network_id).count() as u64
}

fn emergency_online_members_for_network(
    network: &NetworkRecord,
    presence: &BTreeMap<NodeId, NetworkMemberPresence>,
) -> Vec<NodeId> {
    presence
        .iter()
        .filter(|(node_id, presence)| {
            presence.state == NetworkMemberOnlineState::Online && network.members.contains(*node_id)
        })
        .map(|(node_id, _)| node_id.clone())
        .collect()
}

fn emergency_reconciler_actor() -> NodeId {
    NodeId::from_public_key(b"fuckipv6-coordinator-emergency-reconciler")
}

fn recovery_audit_details(
    actor: &NodeId,
    report: &CoordinatorRecoveryReport,
    stage: Option<CoordinatorRecoveryStage>,
    term: Option<&CoordinatorTerm>,
) -> BTreeMap<String, String> {
    let mut details = BTreeMap::from([
        ("actor".into(), actor.to_string()),
        (
            "observed_term".into(),
            report
                .observed_term
                .map(|term| term.to_string())
                .unwrap_or_else(|| "none".into()),
        ),
        (
            "observed_fencing_token".into(),
            report
                .observed_fencing_token
                .map(|token| token.0.to_string())
                .unwrap_or_else(|| "none".into()),
        ),
        (
            "observed_leader".into(),
            report
                .observed_leader
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| "none".into()),
        ),
        (
            "observed_candidate_config_revision".into(),
            report
                .observed_candidate_config_revision
                .map(|revision| revision.to_string())
                .unwrap_or_else(|| "none".into()),
        ),
        (
            "observed_audit_sequence".into(),
            report.observed_audit_sequence.to_string(),
        ),
        ("governance_writable".into(), "false".into()),
    ]);
    if let Some(stage) = stage {
        details.insert("recovery_stage".into(), format!("{stage:?}"));
    }
    if let Some(term) = term {
        details.insert("authoritative_term".into(), term.term.to_string());
        details.insert(
            "authoritative_fencing_token".into(),
            term.fencing_token.0.to_string(),
        );
        details.insert("authoritative_leader".into(), term.leader.to_string());
        details.insert(
            "authoritative_candidate_config_revision".into(),
            term.candidate_config_revision.to_string(),
        );
    }
    details
}

fn election_audit_details(
    election: Option<&CoordinatorElectionSession>,
) -> BTreeMap<String, String> {
    let Some(election) = election else {
        return BTreeMap::new();
    };
    let mut details = BTreeMap::from([
        (
            "proposal_id".into(),
            format!("{:?}", election.proposal.proposal_id),
        ),
        ("stage".into(), format!("{:?}", election.stage)),
        ("term".into(), election.proposal.term.to_string()),
        ("leader".into(), election.proposal.leader.to_string()),
        (
            "participant_count".into(),
            election.participants.len().to_string(),
        ),
        ("vote_count".into(), election.votes.len().to_string()),
        (
            "majority_threshold".into(),
            election.proposal.majority_threshold().to_string(),
        ),
    ]);
    if let Some(term) = &election.result_term {
        details.insert("fencing_token".into(), term.fencing_token.0.to_string());
        details.insert(
            "lease_expires_at_unix_ms".into(),
            term.expires_at_unix_ms.to_string(),
        );
    }
    if let Some(reason) = &election.rejection_reason {
        details.insert("rejection_reason".into(), reason.clone());
    }
    details
}

fn term_audit_details(term: Option<&CoordinatorTerm>) -> BTreeMap<String, String> {
    let Some(term) = term else {
        return BTreeMap::new();
    };
    BTreeMap::from([
        ("term".into(), term.term.to_string()),
        ("leader".into(), term.leader.to_string()),
        ("fencing_token".into(), term.fencing_token.0.to_string()),
        (
            "lease_expires_at_unix_ms".into(),
            term.expires_at_unix_ms.to_string(),
        ),
    ])
}

fn emergency_audit_details(
    emergency: Option<&CoordinatorEmergencyLeaderStatus>,
) -> BTreeMap<String, String> {
    let Some(emergency) = emergency else {
        return BTreeMap::new();
    };
    BTreeMap::from([
        ("leader".into(), emergency.leader.to_string()),
        (
            "emergency_epoch".into(),
            emergency.emergency_epoch.to_string(),
        ),
        (
            "candidate_config_revision".into(),
            emergency.candidate_config_revision.to_string(),
        ),
        (
            "issued_at_unix_ms".into(),
            emergency.issued_at_unix_ms.to_string(),
        ),
        (
            "expires_at_unix_ms".into(),
            emergency.expires_at_unix_ms.to_string(),
        ),
        (
            "hard_expires_at_unix_ms".into(),
            emergency.hard_expires_at_unix_ms.to_string(),
        ),
        ("governance_writable".into(), "false".into()),
        ("data_plane_allowed".into(), "true".into()),
    ])
}

fn append_emergency_governance_rejected(
    audit_log: &mut AuditLog,
    actor: &NodeId,
    network_id: &NetworkId,
    action: &str,
    lease: &CoordinatorEmergencyLeaderStatus,
    now_unix_ms: u64,
) -> Result<()> {
    append_audit(
        audit_log,
        actor,
        network_id,
        "coordinator.emergency.governance.rejected",
        format!("network/{network_id}/coordinator-emergency"),
        AuditOutcome::Denied,
        now_unix_ms,
        "governance mutation rejected during emergency runtime authority",
        BTreeMap::from([
            ("action".into(), action.into()),
            ("leader".into(), lease.leader.to_string()),
            ("emergency_epoch".into(), lease.emergency_epoch.to_string()),
            (
                "candidate_config_revision".into(),
                lease.candidate_config_revision.to_string(),
            ),
        ]),
    )
}

fn audit_outcome_for_error(error: &anyhow::Error) -> AuditOutcome {
    let code = error.to_string();
    if code.contains("permission_denied")
        || code.contains("actor_mismatch")
        || code.contains("leader_mismatch")
        || code.contains("not_candidate")
        || code.contains("config_missing")
        || code.contains("term_rollback")
        || code.contains("term_conflict")
        || code.contains("fencing_token_rollback")
        || code.contains("not_installed")
        || code.contains("audit_cursor_ahead")
        || code.contains("write_forbidden")
        || code.contains("transition_read_only")
        || code.contains("clock_skew")
        || code.contains("lease_expired")
        || code.contains("write_requires_term")
        || code.contains("election_")
        || code.contains("self_appointment")
        || code.contains("no_eligible_candidate")
        || code.contains("leader_not_preferred")
        || code.contains("conflicting_vote")
        || code.contains("invalid_stage")
    {
        AuditOutcome::Denied
    } else {
        AuditOutcome::Failed
    }
}

#[allow(clippy::too_many_arguments)]
fn append_audit(
    audit_log: &mut AuditLog,
    actor: &NodeId,
    network_id: &NetworkId,
    action: &str,
    resource: String,
    outcome: AuditOutcome,
    now_unix_ms: u64,
    summary: &str,
    details: BTreeMap<String, String>,
) -> Result<()> {
    let sequence = AUDIT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let seed = format!(
        "coordinator-candidate-audit:{}:{}:{}:{}:{}",
        network_id, actor, action, now_unix_ms, sequence
    );
    audit_log.append(AuditEntry {
        audit_id: AuditId::from_public_key(seed.as_bytes()),
        operation_id: None,
        actor: actor.clone(),
        network_id: Some(network_id.clone()),
        action: action.into(),
        resource,
        outcome,
        occurred_at_unix_ms: now_unix_ms,
        summary: summary.into(),
        details,
        sensitive_keys: Default::default(),
    })?;
    Ok(())
}

fn lock_profile(path: &Path, exclusive: bool) -> Result<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("无法创建网络资料目录: {}", parent.display()))?;
    }
    let lock_path = path.with_file_name(".profile.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("无法打开网络资料锁: {}", lock_path.display()))?;
    if exclusive {
        lock.lock_exclusive()?;
    } else {
        lock.lock_shared()?;
    }
    Ok(lock)
}

fn lock_candidate_file(path: &Path) -> Result<File> {
    let lock_path = path.with_extension("json.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("无法打开协调器候选存储锁: {}", lock_path.display()))?;
    lock.lock_exclusive()?;
    Ok(lock)
}

fn lock_governance_gate(path: &Path) -> Result<File> {
    let lock_path = path.with_extension("governance.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("无法打开协调器治理门禁锁: {}", lock_path.display()))?;
    lock.lock_exclusive()?;
    Ok(lock)
}

fn read_network_record(path: &Path, expected_network_id: &NetworkId) -> Result<NetworkRecord> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("无法读取协调器候选网络资料: {}", path.display()))?;
    let record: NetworkRecord = serde_json::from_str(&content)
        .with_context(|| format!("无法解析协调器候选网络资料: {}", path.display()))?;
    record.validate()?;
    if record.network_id != *expected_network_id {
        bail!("coordinator_candidate.error.network_mismatch");
    }
    Ok(record)
}

fn persist_bytes_atomic(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path.parent().context("持久化路径没有父目录")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("无法创建持久化目录: {}", parent.display()))?;
    let temporary = path.with_extension(format!(
        "json.tmp-{}-{}",
        std::process::id(),
        AUDIT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    {
        let mut output = File::create(&temporary)
            .with_context(|| format!("无法创建持久化临时文件: {}", temporary.display()))?;
        output
            .write_all(content)
            .with_context(|| format!("无法写入持久化临时文件: {}", temporary.display()))?;
        output
            .sync_all()
            .with_context(|| format!("无法同步持久化临时文件: {}", temporary.display()))?;
    }
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error).with_context(|| format!("无法替换持久化文件: {}", path.display()));
    }
    let directory =
        File::open(parent).with_context(|| format!("无法打开持久化目录: {}", parent.display()))?;
    directory
        .sync_all()
        .with_context(|| format!("无法同步持久化目录: {}", parent.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuckipv6_common::domain::{
        CapabilityKind, CoordinatorAuthority, CoordinatorCandidate, CoordinatorCandidateResources,
        CoordinatorCandidateWillingness, CoordinatorControlPlaneState, CoordinatorPhase,
        CoordinatorPublicReachability, ExitContributionMode, ExitContributionObligation,
        ExitContributionScope, ExitSelectionPolicy, FencingToken, NetworkCreateProfile,
        NetworkJoinMode, NetworkVisibility, PublicKey, COORDINATOR_ELECTION_VOTE_MAX_VALIDITY_MS,
    };
    use fuckipv6_common::Identity;
    use std::sync::mpsc;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::Duration;

    const NOW: u64 = 2_000_000;

    struct Fixture {
        dir: PathBuf,
        network: NetworkRecord,
        owner: Identity,
        candidate_one: Identity,
        candidate_two: Identity,
        ordinary: Identity,
    }

    impl Fixture {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "fuckipv6-coordinator-candidates-{}-{label}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let owner = Identity::generate().unwrap();
            let candidate_one = Identity::generate().unwrap();
            let candidate_two = Identity::generate().unwrap();
            let ordinary = Identity::generate().unwrap();
            let owner_id = NodeId::from_public_key(&owner.public_key());
            let candidate_one_id = NodeId::from_public_key(&candidate_one.public_key());
            let candidate_two_id = NodeId::from_public_key(&candidate_two.public_key());
            let ordinary_id = NodeId::from_public_key(&ordinary.public_key());
            let mut network = NetworkRecord::create_owned(
                NetworkCreateProfile {
                    name: "candidate-store".into(),
                    purpose: "Coordinator candidate persistence tests".into(),
                    icon: "network".into(),
                    language: "zh-CN".into(),
                    region: "ZZ".into(),
                    tags: vec![],
                    capability_declarations: vec![CapabilityKind::Coordinator],
                    visibility: NetworkVisibility::Private,
                    join_mode: NetworkJoinMode::InviteOnly,
                    exit_contribution: ExitContributionObligation::NotRequired,
                    exit_contribution_scope: ExitContributionScope::NotApplicable,
                    exit_contribution_mode: ExitContributionMode::Disabled,
                    exit_selection_policy: ExitSelectionPolicy::default(),
                },
                PublicKey::from_bytes([73; 32]),
                owner_id,
                NOW - 10_000,
            )
            .unwrap();
            network
                .members
                .extend([candidate_one_id, candidate_two_id, ordinary_id]);
            network.validate().unwrap();
            let network_path = dir.join("network.json");
            std::fs::write(&network_path, serde_json::to_vec_pretty(&network).unwrap()).unwrap();
            Self {
                dir,
                network,
                owner,
                candidate_one,
                candidate_two,
                ordinary,
            }
        }

        fn store(&self) -> CoordinatorCandidateStore {
            CoordinatorCandidateStore::new(
                self.dir.join("coordinator_candidates.json"),
                self.dir.join("network.json"),
            )
            .unwrap()
        }

        fn config(
            &self,
            revision: u64,
            candidates: Vec<CoordinatorCandidate>,
        ) -> CoordinatorCandidateConfig {
            CoordinatorCandidateConfig::signed(
                self.network.network_id.clone(),
                revision,
                candidates,
                NOW,
                NOW + 3_600_000,
                &self.owner,
            )
            .unwrap()
        }

        fn config_two(&self, revision: u64) -> CoordinatorCandidateConfig {
            self.config(
                revision,
                vec![
                    CoordinatorCandidate {
                        node_id: NodeId::from_public_key(&self.candidate_one.public_key()),
                        priority: 10,
                    },
                    CoordinatorCandidate {
                        node_id: NodeId::from_public_key(&self.candidate_two.public_key()),
                        priority: 20,
                    },
                ],
            )
        }

        fn config_three(&self, revision: u64) -> CoordinatorCandidateConfig {
            self.config(
                revision,
                vec![
                    CoordinatorCandidate {
                        node_id: NodeId::from_public_key(&self.candidate_one.public_key()),
                        priority: 10,
                    },
                    CoordinatorCandidate {
                        node_id: NodeId::from_public_key(&self.candidate_two.public_key()),
                        priority: 20,
                    },
                    CoordinatorCandidate {
                        node_id: NodeId::from_public_key(&self.ordinary.public_key()),
                        priority: 30,
                    },
                ],
            )
        }

        fn declaration(
            &self,
            candidate: &Identity,
            revision: u64,
        ) -> CoordinatorCandidateDeclaration {
            CoordinatorCandidateDeclaration::signed(
                self.network.network_id.clone(),
                revision,
                CoordinatorPublicReachability {
                    public_endpoint: Some("198.51.100.8:443".parse().unwrap()),
                    reachable_via_relay: true,
                },
                CoordinatorCandidateResources {
                    cpu_cores: 4,
                    memory_bytes: 512 * 1024 * 1024,
                    max_connections: 1_000,
                },
                CoordinatorCandidateWillingness::Willing,
                NOW,
                NOW + 60_000,
                candidate,
            )
            .unwrap()
        }

        fn term(
            &self,
            term: u64,
            fencing_token: u64,
            leader: &Identity,
            issued_at_unix_ms: u64,
            expires_at_unix_ms: u64,
        ) -> CoordinatorTerm {
            let candidate_group = vec![
                NodeId::from_public_key(&self.candidate_one.public_key()),
                NodeId::from_public_key(&self.candidate_two.public_key()),
            ];
            self.term_with_revision_and_group(
                term,
                fencing_token,
                leader,
                1,
                candidate_group,
                &[&self.candidate_one, &self.candidate_two],
                issued_at_unix_ms,
                expires_at_unix_ms,
            )
        }

        #[allow(clippy::too_many_arguments)]
        fn term_with_revision_and_group(
            &self,
            term: u64,
            fencing_token: u64,
            leader: &Identity,
            candidate_config_revision: u64,
            candidate_group: Vec<NodeId>,
            voters: &[&Identity],
            issued_at_unix_ms: u64,
            expires_at_unix_ms: u64,
        ) -> CoordinatorTerm {
            let leader = NodeId::from_public_key(&leader.public_key());
            let election_expiry = issued_at_unix_ms
                .saturating_add(COORDINATOR_ELECTION_VOTE_MAX_VALIDITY_MS)
                .max(expires_at_unix_ms);
            let majority_votes = voters
                .iter()
                .copied()
                .map(|voter| {
                    fuckipv6_common::domain::CoordinatorElectionVote::signed(
                        self.network.network_id.clone(),
                        candidate_config_revision,
                        term,
                        leader.clone(),
                        candidate_group.clone(),
                        issued_at_unix_ms,
                        election_expiry,
                        voter,
                    )
                    .unwrap()
                })
                .collect::<Vec<_>>();
            CoordinatorTerm {
                network_id: self.network.network_id.clone(),
                term,
                leader,
                candidate_group,
                candidate_config_revision,
                majority_proof: majority_votes
                    .iter()
                    .map(|vote| vote.voter.clone())
                    .collect(),
                majority_votes,
                authority: CoordinatorAuthority::Full,
                phase: CoordinatorPhase::Stable,
                fencing_token: FencingToken(fencing_token),
                issued_at_unix_ms,
                expires_at_unix_ms,
            }
        }

        fn term_three(
            &self,
            term: u64,
            fencing_token: u64,
            leader: &Identity,
            issued_at_unix_ms: u64,
            expires_at_unix_ms: u64,
        ) -> CoordinatorTerm {
            let candidate_group = vec![
                NodeId::from_public_key(&self.candidate_one.public_key()),
                NodeId::from_public_key(&self.candidate_two.public_key()),
                NodeId::from_public_key(&self.ordinary.public_key()),
            ];
            let leader = NodeId::from_public_key(&leader.public_key());
            let election_expiry = issued_at_unix_ms
                .saturating_add(COORDINATOR_ELECTION_VOTE_MAX_VALIDITY_MS)
                .max(expires_at_unix_ms);
            let majority_votes = [&self.candidate_one, &self.candidate_two]
                .into_iter()
                .map(|voter| {
                    CoordinatorElectionVote::signed(
                        self.network.network_id.clone(),
                        1,
                        term,
                        leader.clone(),
                        candidate_group.clone(),
                        issued_at_unix_ms,
                        election_expiry,
                        voter,
                    )
                    .unwrap()
                })
                .collect::<Vec<_>>();
            CoordinatorTerm {
                network_id: self.network.network_id.clone(),
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
                fencing_token: FencingToken(fencing_token),
                issued_at_unix_ms,
                expires_at_unix_ms,
            }
        }

        fn credential(&self, term: &CoordinatorTerm) -> CoordinatorWriteCredential {
            CoordinatorWriteCredential {
                network_id: term.network_id.clone(),
                term: term.term,
                leader: term.leader.clone(),
                fencing_token: term.fencing_token,
            }
        }

        fn election_proposal(
            &self,
            term: u64,
            leader: &Identity,
            expires_at_unix_ms: u64,
        ) -> CoordinatorElectionProposal {
            CoordinatorElectionProposal::signed(
                self.network.network_id.clone(),
                1,
                term,
                vec![
                    NodeId::from_public_key(&self.candidate_one.public_key()),
                    NodeId::from_public_key(&self.candidate_two.public_key()),
                ],
                NOW,
                expires_at_unix_ms,
                leader,
            )
            .unwrap()
        }

        fn election_vote(
            &self,
            term: u64,
            leader: &Identity,
            voter: &Identity,
            expires_at_unix_ms: u64,
        ) -> CoordinatorElectionVote {
            CoordinatorElectionVote::signed(
                self.network.network_id.clone(),
                1,
                term,
                NodeId::from_public_key(&leader.public_key()),
                vec![
                    NodeId::from_public_key(&self.candidate_one.public_key()),
                    NodeId::from_public_key(&self.candidate_two.public_key()),
                ],
                NOW,
                expires_at_unix_ms,
                voter,
            )
            .unwrap()
        }

        fn election_proposal_three(
            &self,
            term: u64,
            leader: &Identity,
            proposed_at_unix_ms: u64,
            expires_at_unix_ms: u64,
        ) -> CoordinatorElectionProposal {
            CoordinatorElectionProposal::signed(
                self.network.network_id.clone(),
                1,
                term,
                vec![
                    NodeId::from_public_key(&self.candidate_one.public_key()),
                    NodeId::from_public_key(&self.candidate_two.public_key()),
                    NodeId::from_public_key(&self.ordinary.public_key()),
                ],
                proposed_at_unix_ms,
                expires_at_unix_ms,
                leader,
            )
            .unwrap()
        }

        fn election_vote_three(
            &self,
            term: u64,
            leader: &Identity,
            voter: &Identity,
            signed_at_unix_ms: u64,
            expires_at_unix_ms: u64,
        ) -> CoordinatorElectionVote {
            CoordinatorElectionVote::signed(
                self.network.network_id.clone(),
                1,
                term,
                NodeId::from_public_key(&leader.public_key()),
                vec![
                    NodeId::from_public_key(&self.candidate_one.public_key()),
                    NodeId::from_public_key(&self.candidate_two.public_key()),
                    NodeId::from_public_key(&self.ordinary.public_key()),
                ],
                signed_at_unix_ms,
                expires_at_unix_ms,
                voter,
            )
            .unwrap()
        }
    }

    #[test]
    fn config_cas_idempotency_and_restart_are_durable() {
        let fixture = Fixture::new("config-cas");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let config = fixture.config_two(1);
        let status = store
            .update_config(&owner_id, 0, config.clone(), NOW)
            .unwrap();
        assert_eq!(status.config.as_ref().unwrap().revision, 1);
        assert_eq!(
            status.emergency,
            fuckipv6_common::domain::CoordinatorEmergencyState::AllCandidatesUnavailable
        );

        let retry = store
            .update_config(&owner_id, 0, config.clone(), NOW + 1)
            .unwrap();
        assert_eq!(retry.config.as_ref().unwrap(), &config);

        let restarted = fixture.store();
        let status = restarted
            .status(&owner_id, &fixture.network.network_id, NOW)
            .unwrap();
        assert_eq!(status.config.as_ref().unwrap(), &config);

        assert_eq!(
            store
                .update_config(&owner_id, 1, config, NOW + 2)
                .unwrap()
                .config
                .as_ref()
                .unwrap()
                .revision,
            1
        );
        let conflicting = fixture.config(
            1,
            vec![CoordinatorCandidate {
                node_id: NodeId::from_public_key(&fixture.owner.public_key()),
                priority: 10,
            }],
        );
        assert_eq!(
            store
                .update_config(&owner_id, 1, conflicting, NOW + 2)
                .unwrap_err()
                .to_string(),
            "coordinator_candidate.error.revision_conflict"
        );
        assert_eq!(
            store
                .update_config(&owner_id, 0, fixture.config_two(2), NOW + 3)
                .unwrap_err()
                .to_string(),
            "coordinator_candidate.error.stale_revision"
        );
        assert_eq!(
            store
                .update_config(&owner_id, 1, fixture.config_two(3), NOW + 4)
                .unwrap_err()
                .to_string(),
            "coordinator_candidate.error.revision_gap"
        );
    }

    #[test]
    fn election_notifications_target_only_current_candidates() {
        let fixture = Fixture::new("election-notification-recipients");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        store
            .update_config(&owner_id, 0, fixture.config_two(1), NOW)
            .unwrap();

        let recipients = store
            .election_notification_recipients(&fixture.network.network_id)
            .unwrap();
        assert_eq!(
            recipients
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            [
                NodeId::from_public_key(&fixture.candidate_one.public_key()),
                NodeId::from_public_key(&fixture.candidate_two.public_key()),
            ]
            .into_iter()
            .collect()
        );
    }

    #[test]
    fn declarations_require_self_signature_current_candidate_and_revision() {
        let fixture = Fixture::new("declarations");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let candidate_one_id = NodeId::from_public_key(&fixture.candidate_one.public_key());
        let ordinary_id = NodeId::from_public_key(&fixture.ordinary.public_key());
        store
            .update_config(&owner_id, 0, fixture.config_two(1), NOW)
            .unwrap();

        let declaration = fixture.declaration(&fixture.candidate_one, 1);
        let status = store
            .publish_declaration(&candidate_one_id, declaration.clone(), NOW)
            .unwrap();
        assert_eq!(
            status.candidates[0].health,
            fuckipv6_common::domain::CoordinatorCandidateHealth::Healthy
        );
        assert!(status.candidates[0].eligible_for_election);
        assert_eq!(status.current_leader, None);

        assert_eq!(
            store
                .publish_declaration(&ordinary_id, declaration.clone(), NOW)
                .unwrap_err()
                .to_string(),
            "coordinator_candidate.error.authenticated_actor_mismatch"
        );
        assert_eq!(
            store
                .publish_declaration(
                    &candidate_one_id,
                    fixture.declaration(&fixture.ordinary, 1),
                    NOW,
                )
                .unwrap_err()
                .to_string(),
            "coordinator_candidate.error.authenticated_actor_mismatch"
        );
        assert_eq!(
            store
                .publish_declaration(
                    &candidate_one_id,
                    fixture.declaration(&fixture.candidate_one, 2),
                    NOW,
                )
                .unwrap_err()
                .to_string(),
            "coordinator_candidate.error.declaration_revision_mismatch"
        );

        store
            .update_config(&owner_id, 1, fixture.config_two(2), NOW + 1)
            .unwrap();
        let status = store
            .status(&owner_id, &fixture.network.network_id, NOW + 1)
            .unwrap();
        assert_eq!(
            status.candidates[0].health,
            fuckipv6_common::domain::CoordinatorCandidateHealth::Missing
        );
    }

    #[test]
    fn governance_gate_fails_closed_until_a_current_candidate_is_healthy() {
        let fixture = Fixture::new("governance-gate");
        let store = fixture.store();
        let network_id = fixture.network.network_id.clone();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let candidate_id = NodeId::from_public_key(&fixture.candidate_one.public_key());

        assert_eq!(
            store
                .ensure_governance_allowed(&network_id, NOW)
                .unwrap_err()
                .to_string(),
            GOVERNANCE_FROZEN_ERROR
        );

        store
            .update_config(&owner_id, 0, fixture.config_two(1), NOW)
            .unwrap();
        assert_eq!(
            store
                .ensure_governance_allowed(&network_id, NOW)
                .unwrap_err()
                .to_string(),
            GOVERNANCE_FROZEN_ERROR
        );

        store
            .publish_declaration(
                &candidate_id,
                fixture.declaration(&fixture.candidate_one, 1),
                NOW,
            )
            .unwrap();
        store
            .ensure_governance_allowed(&network_id, NOW)
            .expect("one healthy current candidate must release governance");

        assert_eq!(
            store
                .ensure_governance_allowed(&network_id, NOW + 3_600_000)
                .unwrap_err()
                .to_string(),
            GOVERNANCE_FROZEN_ERROR
        );
    }

    #[test]
    fn governance_gate_freezes_pending_configuration_without_actor_membership() {
        let fixture = Fixture::new("governance-gate-pending");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let config = CoordinatorCandidateConfig::signed(
            fixture.network.network_id.clone(),
            1,
            vec![CoordinatorCandidate {
                node_id: NodeId::from_public_key(&fixture.candidate_one.public_key()),
                priority: 10,
            }],
            NOW + 1,
            NOW + 3_600_000,
            &fixture.owner,
        )
        .unwrap();
        store.update_config(&owner_id, 0, config, NOW).unwrap();

        assert_eq!(
            store
                .ensure_governance_allowed(&fixture.network.network_id, NOW)
                .unwrap_err()
                .to_string(),
            GOVERNANCE_FROZEN_ERROR
        );
    }

    #[test]
    fn governance_gate_serializes_governance_writes_with_candidate_updates() {
        let fixture = Fixture::new("governance-gate-race");
        let network_id = fixture.network.network_id.clone();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let candidate_id = NodeId::from_public_key(&fixture.candidate_one.public_key());
        let setup_store = fixture.store();
        setup_store
            .update_config(&owner_id, 0, fixture.config_two(1), NOW)
            .unwrap();
        setup_store
            .publish_declaration(
                &candidate_id,
                fixture.declaration(&fixture.candidate_one, 1),
                NOW,
            )
            .unwrap();

        let governance_store = Arc::new(fixture.store());
        let update_store = Arc::new(fixture.store());
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let governance = {
            let store = governance_store.clone();
            let network_id = network_id.clone();
            thread::spawn(move || {
                store.with_governance_allowed(&network_id, NOW, |_| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    Ok(())
                })
            })
        };
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        let (started_tx, started_rx) = mpsc::channel();
        let (updated_tx, updated_rx) = mpsc::channel();
        let updater = {
            let store = update_store.clone();
            let owner_id = owner_id.clone();
            let config = fixture.config(
                2,
                vec![CoordinatorCandidate {
                    node_id: candidate_id,
                    priority: 10,
                }],
            );
            thread::spawn(move || {
                started_tx.send(()).unwrap();
                let result = store.update_config(&owner_id, 1, config, NOW + 1);
                updated_tx.send(result.is_ok()).unwrap();
            })
        };
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            updated_rx.recv_timeout(Duration::from_millis(150)).is_err(),
            "candidate configuration update crossed the active governance gate"
        );

        release_tx.send(()).unwrap();
        governance.join().unwrap().unwrap();
        assert!(updated_rx.recv_timeout(Duration::from_secs(5)).unwrap());
        updater.join().unwrap();
        assert_eq!(
            governance_store
                .ensure_governance_allowed(&network_id, NOW + 1)
                .unwrap_err()
                .to_string(),
            GOVERNANCE_FROZEN_ERROR
        );
    }

    #[test]
    fn persisted_election_commits_only_after_strict_majority() {
        let fixture = Fixture::new("persisted-election");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let candidate_one_id = NodeId::from_public_key(&fixture.candidate_one.public_key());
        let candidate_two_id = NodeId::from_public_key(&fixture.candidate_two.public_key());
        store
            .update_config(&owner_id, 0, fixture.config_two(1), NOW)
            .unwrap();
        for candidate in [&fixture.candidate_one, &fixture.candidate_two] {
            store
                .publish_declaration(
                    &NodeId::from_public_key(&candidate.public_key()),
                    fixture.declaration(candidate, 1),
                    NOW,
                )
                .unwrap();
        }

        let proposal = fixture.election_proposal(1, &fixture.candidate_one, NOW + 30_000);
        let proposal_id = proposal.proposal_id.clone();
        let proposed = store
            .propose_election(&candidate_one_id, proposal.clone(), NOW)
            .unwrap();
        assert_eq!(
            proposed.latest_election.as_ref().unwrap().stage,
            CoordinatorElectionStage::Voting
        );

        let first_vote = fixture.election_vote(
            1,
            &fixture.candidate_one,
            &fixture.candidate_one,
            NOW + 30_000,
        );
        let voting = store
            .record_election_vote(&candidate_one_id, &proposal_id, first_vote.clone(), NOW + 1)
            .unwrap();
        assert_eq!(voting.latest_term, None);
        assert_eq!(voting.latest_election.as_ref().unwrap().votes.len(), 1);
        let retry = store
            .record_election_vote(&candidate_one_id, &proposal_id, first_vote, NOW + 2)
            .unwrap();
        assert_eq!(retry.latest_election.as_ref().unwrap().votes.len(), 1);

        let deciding_vote = fixture.election_vote(
            1,
            &fixture.candidate_one,
            &fixture.candidate_two,
            NOW + 30_000,
        );
        let committed = store
            .record_election_vote(&candidate_two_id, &proposal_id, deciding_vote, NOW + 3)
            .unwrap();
        let election = committed.latest_election.as_ref().unwrap();
        assert_eq!(election.stage, CoordinatorElectionStage::Committed);
        assert_eq!(election.votes.len(), 2);
        let term = committed.latest_term.as_ref().unwrap();
        assert_eq!(term.term, 1);
        assert_eq!(term.leader, candidate_one_id);
        assert_eq!(term.fencing_token, FencingToken(1));
        assert_eq!(term.expires_at_unix_ms, NOW + 30_000);

        let restored = fixture
            .store()
            .status(&owner_id, &fixture.network.network_id, NOW + 4)
            .unwrap();
        assert_eq!(restored.latest_election.as_ref(), Some(election));
        assert_eq!(restored.latest_term.as_ref(), Some(term));
        let audit = store.audit_entries().unwrap();
        for action in [
            "coordinator.election.started",
            "coordinator.election.vote.accepted",
            "coordinator.election.committed",
        ] {
            assert!(
                audit
                    .iter()
                    .any(|entry| entry.action == action && entry.outcome == AuditOutcome::Succeeded),
                "missing {action}: {audit:#?}"
            );
        }
    }

    #[test]
    fn election_timeout_is_durable_and_rejects_late_votes() {
        let fixture = Fixture::new("election-timeout");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let candidate_one_id = NodeId::from_public_key(&fixture.candidate_one.public_key());
        store
            .update_config(&owner_id, 0, fixture.config_two(1), NOW)
            .unwrap();
        for candidate in [&fixture.candidate_one, &fixture.candidate_two] {
            store
                .publish_declaration(
                    &NodeId::from_public_key(&candidate.public_key()),
                    fixture.declaration(candidate, 1),
                    NOW,
                )
                .unwrap();
        }
        let proposal = fixture.election_proposal(1, &fixture.candidate_one, NOW + 30_000);
        let proposal_id = proposal.proposal_id.clone();
        store
            .propose_election(&candidate_one_id, proposal, NOW)
            .unwrap();
        let early_timeout_error = store
            .timeout_election(
                &candidate_one_id,
                &fixture.network.network_id,
                &proposal_id,
                NOW + 29_999,
            )
            .unwrap_err()
            .to_string();
        assert!(
            early_timeout_error.contains("domain.coordinator_election_not_timed_out"),
            "{early_timeout_error}"
        );
        let timed_out = store
            .timeout_election(
                &candidate_one_id,
                &fixture.network.network_id,
                &proposal_id,
                NOW + 30_000,
            )
            .unwrap();
        assert_eq!(
            timed_out.latest_election.as_ref().unwrap().stage,
            CoordinatorElectionStage::TimedOut
        );
        assert_eq!(
            timed_out.control_plane.state,
            CoordinatorControlPlaneState::QuorumUnavailable
        );
        assert_eq!(
            timed_out.control_plane.reason_code,
            "coordinator.control_plane.no_majority"
        );
        assert_eq!(timed_out.control_plane.quorum.received_votes, 0);
        assert_eq!(timed_out.control_plane.quorum.required_votes, 2);
        assert!(!timed_out.control_plane.governance_writable);
        assert_eq!(timed_out.latest_term, None);
        assert_eq!(timed_out.current_leader, None);

        let denied = store
            .authorize_governance_write(
                &candidate_one_id,
                &CoordinatorWriteCredential {
                    network_id: fixture.network.network_id.clone(),
                    term: 1,
                    leader: candidate_one_id.clone(),
                    fencing_token: FencingToken(1),
                },
                CoordinatorWriteKind::Membership,
                NOW + 30_001,
            )
            .unwrap_err()
            .to_string();
        assert_eq!(denied, "coordinator_term.error.missing");

        let repeated = store
            .timeout_election(
                &candidate_one_id,
                &fixture.network.network_id,
                &proposal_id,
                NOW + 30_002,
            )
            .unwrap();
        assert_eq!(
            repeated.latest_election.as_ref().unwrap().stage,
            CoordinatorElectionStage::TimedOut
        );
        let audit = store.audit_entries().unwrap();
        assert_eq!(
            audit
                .iter()
                .filter(|entry| entry.action == "coordinator.election.no_majority")
                .count(),
            1,
            "{audit:#?}"
        );
        let no_majority = audit
            .iter()
            .find(|entry| entry.action == "coordinator.election.no_majority")
            .unwrap();
        assert_eq!(no_majority.details.get("vote_count"), Some(&"0".into()));
        assert_eq!(
            no_majority.details.get("majority_threshold"),
            Some(&"2".into())
        );
        let late_vote = fixture.election_vote(
            1,
            &fixture.candidate_one,
            &fixture.candidate_one,
            NOW + 30_000,
        );
        let error = store
            .record_election_vote(&candidate_one_id, &proposal_id, late_vote, NOW + 30_001)
            .unwrap_err()
            .to_string();
        assert!(error.contains("invalid_stage"), "{error}");
        let restored = fixture
            .store()
            .status(&owner_id, &fixture.network.network_id, NOW + 30_001)
            .unwrap();
        assert_eq!(
            restored.latest_election.as_ref().unwrap().stage,
            CoordinatorElectionStage::TimedOut
        );
    }

    #[test]
    fn three_candidate_election_commits_only_after_strict_majority() {
        let fixture = Fixture::new("three-candidate-majority");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let candidate_one_id = NodeId::from_public_key(&fixture.candidate_one.public_key());
        let candidate_two_id = NodeId::from_public_key(&fixture.candidate_two.public_key());
        let ordinary_id = NodeId::from_public_key(&fixture.ordinary.public_key());
        store
            .update_config(&owner_id, 0, fixture.config_three(1), NOW)
            .unwrap();
        for candidate in [
            &fixture.candidate_one,
            &fixture.candidate_two,
            &fixture.ordinary,
        ] {
            store
                .publish_declaration(
                    &NodeId::from_public_key(&candidate.public_key()),
                    fixture.declaration(candidate, 1),
                    NOW,
                )
                .unwrap();
        }

        let candidate_group = vec![
            candidate_one_id.clone(),
            candidate_two_id.clone(),
            ordinary_id,
        ];
        let proposal = CoordinatorElectionProposal::signed(
            fixture.network.network_id.clone(),
            1,
            1,
            candidate_group.clone(),
            NOW,
            NOW + 30_000,
            &fixture.candidate_one,
        )
        .unwrap();
        let proposal_id = proposal.proposal_id.clone();
        store
            .propose_election(&candidate_one_id, proposal, NOW)
            .unwrap();
        let first_vote = CoordinatorElectionVote::signed(
            fixture.network.network_id.clone(),
            1,
            1,
            candidate_one_id.clone(),
            candidate_group.clone(),
            NOW,
            NOW + 30_000,
            &fixture.candidate_one,
        )
        .unwrap();
        let voting = store
            .record_election_vote(&candidate_one_id, &proposal_id, first_vote, NOW + 1)
            .unwrap();
        assert_eq!(voting.latest_term, None);
        assert_eq!(
            voting.control_plane.state,
            CoordinatorControlPlaneState::ElectionInProgress
        );
        assert_eq!(voting.control_plane.quorum.received_votes, 1);
        assert_eq!(voting.control_plane.quorum.required_votes, 2);
        assert_eq!(voting.control_plane.quorum.participant_count, 3);

        let deciding_vote = CoordinatorElectionVote::signed(
            fixture.network.network_id.clone(),
            1,
            1,
            candidate_one_id.clone(),
            candidate_group,
            NOW,
            NOW + 30_000,
            &fixture.candidate_two,
        )
        .unwrap();
        let committed = store
            .record_election_vote(&candidate_two_id, &proposal_id, deciding_vote, NOW + 2)
            .unwrap();
        assert_eq!(
            committed.latest_election.as_ref().unwrap().stage,
            CoordinatorElectionStage::Committed
        );
        assert_eq!(committed.latest_election.as_ref().unwrap().votes.len(), 2);
        assert_eq!(
            committed.control_plane.state,
            CoordinatorControlPlaneState::Available
        );
        assert!(committed.control_plane.governance_writable);
        assert_eq!(committed.current_leader, Some(candidate_one_id));
        assert_eq!(
            committed.latest_term.as_ref().unwrap().fencing_token,
            FencingToken(1)
        );
    }

    #[test]
    fn minority_loss_and_delay_recover_without_rolling_back_the_term() {
        let fixture = Fixture::new("partition-recovery");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let candidate_one_id = NodeId::from_public_key(&fixture.candidate_one.public_key());
        let candidate_two_id = NodeId::from_public_key(&fixture.candidate_two.public_key());
        let ordinary_id = NodeId::from_public_key(&fixture.ordinary.public_key());
        store
            .update_config(&owner_id, 0, fixture.config_three(1), NOW)
            .unwrap();
        for candidate in [
            &fixture.candidate_one,
            &fixture.candidate_two,
            &fixture.ordinary,
        ] {
            store
                .publish_declaration(
                    &NodeId::from_public_key(&candidate.public_key()),
                    fixture.declaration(candidate, 1),
                    NOW,
                )
                .unwrap();
        }

        let lease_expires_at_unix_ms = NOW + 30_000;
        let initial_term =
            fixture.term_three(1, 1, &fixture.candidate_one, NOW, lease_expires_at_unix_ms);
        let old_credential = fixture.credential(&initial_term);
        let initial = store
            .install_term(&candidate_one_id, initial_term.clone(), NOW)
            .unwrap();
        assert_eq!(
            initial.control_plane.state,
            CoordinatorControlPlaneState::Available
        );
        assert!(initial.control_plane.governance_writable);

        // Candidate one is isolated with only its own vote delivered. Candidate
        // two's deciding vote is delayed and the third participant is dropped.
        let first_proposal = fixture.election_proposal_three(
            2,
            &fixture.candidate_two,
            lease_expires_at_unix_ms,
            lease_expires_at_unix_ms + 15_000,
        );
        let first_proposal_id = first_proposal.proposal_id.clone();
        store
            .propose_election(
                &candidate_two_id,
                first_proposal.clone(),
                lease_expires_at_unix_ms,
            )
            .unwrap();
        let only_vote = fixture.election_vote_three(
            2,
            &fixture.candidate_two,
            &fixture.candidate_one,
            lease_expires_at_unix_ms,
            first_proposal.expires_at_unix_ms,
        );
        store
            .record_election_vote(
                &candidate_one_id,
                &first_proposal_id,
                only_vote.clone(),
                lease_expires_at_unix_ms + 1,
            )
            .unwrap();
        let retried = store
            .record_election_vote(
                &candidate_one_id,
                &first_proposal_id,
                only_vote,
                lease_expires_at_unix_ms + 2,
            )
            .unwrap();
        assert_eq!(retried.latest_election.as_ref().unwrap().votes.len(), 1);

        let timed_out = store
            .timeout_election(
                &candidate_one_id,
                &fixture.network.network_id,
                &first_proposal_id,
                first_proposal.expires_at_unix_ms,
            )
            .unwrap();
        assert_eq!(
            timed_out.latest_election.as_ref().unwrap().stage,
            CoordinatorElectionStage::TimedOut
        );
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
        assert!(!timed_out.control_plane.governance_writable);
        assert_eq!(timed_out.latest_term.as_ref(), Some(&initial_term));
        assert_eq!(timed_out.current_leader, None);
        assert_eq!(timed_out.config.as_ref().unwrap().revision, 1);

        let denied = store
            .authorize_governance_write(
                &candidate_one_id,
                &old_credential,
                CoordinatorWriteKind::SecurityPolicy,
                first_proposal.expires_at_unix_ms + 1,
            )
            .unwrap_err()
            .to_string();
        assert!(
            denied.contains("domain.coordinator_lease_expired"),
            "{denied}"
        );

        let delayed_vote = fixture.election_vote_three(
            2,
            &fixture.candidate_two,
            &fixture.candidate_two,
            lease_expires_at_unix_ms,
            first_proposal.expires_at_unix_ms,
        );
        let late_vote_error = store
            .record_election_vote(
                &candidate_two_id,
                &first_proposal_id,
                delayed_vote,
                first_proposal.expires_at_unix_ms + 1,
            )
            .unwrap_err()
            .to_string();
        assert!(
            late_vote_error.contains("invalid_stage"),
            "{late_vote_error}"
        );

        // Connectivity is restored. The delayed partition cannot contribute a
        // vote to this new election, which must independently collect a strict
        // majority and advance the durable term and fencing token.
        let recovery_at_unix_ms = first_proposal.expires_at_unix_ms + 2;
        let recovery_proposal = fixture.election_proposal_three(
            2,
            &fixture.candidate_two,
            recovery_at_unix_ms,
            recovery_at_unix_ms + 15_000,
        );
        let recovery_proposal_id = recovery_proposal.proposal_id.clone();
        store
            .propose_election(
                &candidate_two_id,
                recovery_proposal.clone(),
                recovery_at_unix_ms,
            )
            .unwrap();
        let recovery_vote = fixture.election_vote_three(
            2,
            &fixture.candidate_two,
            &fixture.candidate_two,
            recovery_at_unix_ms,
            recovery_proposal.expires_at_unix_ms,
        );
        store
            .record_election_vote(
                &candidate_two_id,
                &recovery_proposal_id,
                recovery_vote,
                recovery_at_unix_ms + 1,
            )
            .unwrap();
        let deciding_vote = fixture.election_vote_three(
            2,
            &fixture.candidate_two,
            &fixture.ordinary,
            recovery_at_unix_ms,
            recovery_proposal.expires_at_unix_ms,
        );
        let recovered = store
            .record_election_vote(
                &ordinary_id,
                &recovery_proposal_id,
                deciding_vote,
                recovery_at_unix_ms + 2,
            )
            .unwrap();
        assert_eq!(
            recovered.control_plane.state,
            CoordinatorControlPlaneState::Available
        );
        assert!(recovered.control_plane.governance_writable);
        assert_eq!(recovered.current_leader, Some(candidate_two_id.clone()));
        let recovered_term = recovered.latest_term.as_ref().unwrap();
        assert_eq!(recovered_term.term, 2);
        assert_eq!(recovered_term.fencing_token, FencingToken(2));
        assert_eq!(recovered.config.as_ref().unwrap().revision, 1);

        let stale_error = store
            .authorize_governance_write(
                &candidate_one_id,
                &old_credential,
                CoordinatorWriteKind::Membership,
                recovery_at_unix_ms + 3,
            )
            .unwrap_err()
            .to_string();
        assert!(
            stale_error.contains("domain.coordinator_term_rollback"),
            "{stale_error}"
        );

        let restored = fixture
            .store()
            .status(
                &owner_id,
                &fixture.network.network_id,
                recovery_at_unix_ms + 3,
            )
            .unwrap();
        assert_eq!(restored.latest_term.as_ref(), Some(recovered_term));
        assert_eq!(restored.current_leader, Some(candidate_two_id));

        let audit = store.audit_entries().unwrap();
        for action in [
            "coordinator.election.started",
            "coordinator.election.vote.accepted",
            "coordinator.election.timed_out",
            "coordinator.election.no_majority",
            "coordinator.election.vote.rejected",
            "coordinator.election.committed",
            "coordinator.term.write.rejected",
        ] {
            assert!(
                audit
                    .iter()
                    .any(|entry| entry.action == action && entry.outcome != AuditOutcome::Failed),
                "missing {action}: {audit:#?}"
            );
        }
        let no_majority = audit
            .iter()
            .find(|entry| entry.action == "coordinator.election.no_majority")
            .unwrap();
        assert_eq!(no_majority.details.get("term"), Some(&"2".into()));
        assert_eq!(no_majority.details.get("vote_count"), Some(&"1".into()));
        assert_eq!(
            no_majority.details.get("majority_threshold"),
            Some(&"2".into())
        );
        assert_eq!(
            no_majority.details.get("participant_count"),
            Some(&"3".into())
        );
        assert!(audit.iter().any(|entry| {
            entry.action == "coordinator.election.vote.rejected"
                && entry.outcome == AuditOutcome::Denied
                && entry
                    .details
                    .get("error")
                    .is_some_and(|error| error.contains("invalid_stage"))
        }));
        assert!(audit.iter().any(|entry| {
            entry.action == "coordinator.term.write.rejected"
                && entry.outcome == AuditOutcome::Denied
                && entry.details.get("term") == Some(&"1".into())
        }));
    }

    #[test]
    fn concurrent_revision_updates_have_exactly_one_winner() {
        let fixture = Fixture::new("concurrent");
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let store = Arc::new(fixture.store());
        store
            .update_config(&owner_id, 0, fixture.config_two(1), NOW)
            .unwrap();

        let first = fixture.config_two(2);
        let second = fixture.config(
            2,
            vec![CoordinatorCandidate {
                node_id: NodeId::from_public_key(&fixture.candidate_one.public_key()),
                priority: 1,
            }],
        );
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let handles = [first, second].map(|config| {
            let store = store.clone();
            let owner_id = owner_id.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                store.update_config(&owner_id, 1, config, NOW + 1)
            })
        });
        let results = handles.map(|handle| handle.join().unwrap());
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter_map(|result| result.as_ref().err())
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            vec!["coordinator_candidate.error.revision_conflict".to_string()]
        );
        let status = store
            .status(&owner_id, &fixture.network.network_id, NOW + 1)
            .unwrap();
        assert_eq!(status.config.as_ref().unwrap().revision, 2);
        let audit = store.audit_entries().unwrap();
        assert!(audit
            .iter()
            .any(|entry| entry.outcome == AuditOutcome::Succeeded));
        assert!(audit
            .iter()
            .any(|entry| entry.outcome == AuditOutcome::Failed));
    }

    #[test]
    fn non_manager_records_cannot_mutate_candidate_configuration() {
        let fixture = Fixture::new("permissions");
        let store = fixture.store();
        let candidate_id = NodeId::from_public_key(&fixture.candidate_one.public_key());
        let config = CoordinatorCandidateConfig::signed(
            fixture.network.network_id.clone(),
            1,
            vec![CoordinatorCandidate {
                node_id: candidate_id.clone(),
                priority: 10,
            }],
            NOW,
            NOW + 3_600_000,
            &fixture.candidate_one,
        )
        .unwrap();
        let error = store
            .update_config(&candidate_id, 0, config, NOW)
            .unwrap_err()
            .to_string();
        assert!(
            error.starts_with("domain.coordinator_candidate_permission_denied"),
            "{error}"
        );
        assert_eq!(
            store
                .status(
                    &NodeId::from_public_key(b"outsider"),
                    &fixture.network.network_id,
                    NOW,
                )
                .unwrap_err()
                .to_string(),
            "coordinator_candidate.error.permission_denied"
        );
    }

    #[test]
    fn terms_survive_restart_and_old_writes_are_rejected_and_audited() {
        let fixture = Fixture::new("term-high-water-mark");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let candidate_one_id = NodeId::from_public_key(&fixture.candidate_one.public_key());
        let candidate_two_id = NodeId::from_public_key(&fixture.candidate_two.public_key());
        store
            .update_config(&owner_id, 0, fixture.config_two(1), NOW)
            .unwrap();

        let first = fixture.term(1, 1, &fixture.candidate_one, NOW, NOW + 60_000);
        let first_credential = fixture.credential(&first);
        let status = store
            .install_term(&candidate_one_id, first.clone(), NOW)
            .unwrap();
        assert_eq!(status.current_leader, Some(candidate_one_id.clone()));
        assert_eq!(status.latest_term.as_ref(), Some(&first));
        assert!(status.lease.is_writable());

        let restarted = fixture.store();
        let restored = restarted
            .status(&owner_id, &fixture.network.network_id, NOW + 1)
            .unwrap();
        assert_eq!(restored.latest_term.as_ref(), Some(&first));
        assert_eq!(restored.current_leader, Some(candidate_one_id.clone()));

        let second = fixture.term(2, 2, &fixture.candidate_two, NOW + 2, NOW + 60_000);
        store
            .install_term(&candidate_two_id, second.clone(), NOW + 2)
            .unwrap();
        assert_eq!(
            store
                .install_term(&candidate_one_id, first.clone(), NOW + 3)
                .unwrap_err()
                .to_string()
                .split(':')
                .next(),
            Some("domain.coordinator_term_rollback")
        );
        let stale_write_error = store
            .authorize_governance_write(
                &candidate_one_id,
                &first_credential,
                CoordinatorWriteKind::Membership,
                NOW + 3,
            )
            .unwrap_err()
            .to_string();
        assert!(
            stale_write_error.contains("domain.coordinator_term_rollback"),
            "{stale_write_error}"
        );

        let audit = store.audit_entries().unwrap();
        assert!(audit.iter().any(|entry| {
            entry.action == "coordinator.term.install.rejected"
                && entry.outcome == AuditOutcome::Denied
        }));
        assert!(
            audit.iter().any(|entry| {
                entry.action == "coordinator.term.write.rejected"
                    && entry.outcome == AuditOutcome::Denied
                    && entry.details.get("term") == Some(&"1".into())
                    && entry.details.get("fencing_token") == Some(&"[REDACTED]".into())
            }),
            "{audit:#?}"
        );
    }

    #[test]
    fn lease_renewal_extends_authority_and_expiry_enters_read_only() {
        let fixture = Fixture::new("term-lease");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let candidate_one_id = NodeId::from_public_key(&fixture.candidate_one.public_key());
        store
            .update_config(&owner_id, 0, fixture.config_two(1), NOW)
            .unwrap();
        let first = fixture.term(1, 1, &fixture.candidate_one, NOW, NOW + 60_000);
        let credential = fixture.credential(&first);
        store.install_term(&candidate_one_id, first, NOW).unwrap();

        let renewed = store
            .renew_term_lease(&candidate_one_id, &credential, NOW + 120_000, NOW + 40_000)
            .unwrap();
        assert_eq!(
            renewed.latest_term.as_ref().unwrap().expires_at_unix_ms,
            NOW + 120_000
        );
        assert_eq!(renewed.current_leader, Some(candidate_one_id.clone()));

        let error = store
            .renew_term_lease(&candidate_one_id, &credential, NOW + 180_000, NOW + 120_000)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("domain.coordinator_lease_expired"),
            "{error}"
        );
        let expired = store
            .status(&owner_id, &fixture.network.network_id, NOW + 120_000)
            .unwrap();
        assert_eq!(expired.current_leader, None);
        assert_eq!(
            expired.lease.state,
            fuckipv6_common::domain::CoordinatorLeaseState::ReadOnly
        );
        assert_eq!(expired.lease.reason_code, "coordinator.lease.expired");
        assert!(store.audit_entries().unwrap().iter().any(|entry| {
            entry.action == "coordinator.term.lease.renew.rejected"
                && entry.outcome == AuditOutcome::Denied
        }));
    }

    #[test]
    fn emergency_online_members_only_uses_live_sessions_for_current_members() {
        let fixture = Fixture::new("emergency-online-members");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let candidate_one_id = NodeId::from_public_key(&fixture.candidate_one.public_key());
        let candidate_two_id = NodeId::from_public_key(&fixture.candidate_two.public_key());
        let ordinary_id = NodeId::from_public_key(&fixture.ordinary.public_key());
        let outsider_id = NodeId::from_public_key(&Identity::generate().unwrap().public_key());
        let presence = BTreeMap::from([
            (owner_id.clone(), NetworkMemberPresence::live(NOW)),
            (
                candidate_one_id.clone(),
                NetworkMemberPresence::disconnected(NOW),
            ),
            (candidate_two_id.clone(), NetworkMemberPresence::live(NOW)),
            (ordinary_id.clone(), NetworkMemberPresence::unknown()),
            (outsider_id, NetworkMemberPresence::live(NOW)),
        ]);

        let online = store
            .emergency_online_members(&fixture.network.network_id, &presence)
            .unwrap();
        let mut expected = vec![owner_id, candidate_two_id];
        expected.sort();
        assert_eq!(online, expected);
    }

    #[test]
    fn presence_reconciliation_revokes_each_invalid_emergency_authority() {
        let fixture = Fixture::new("emergency-presence-reconciliation");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let candidate_one_id = NodeId::from_public_key(&fixture.candidate_one.public_key());
        let ordinary_id = NodeId::from_public_key(&fixture.ordinary.public_key());
        store
            .update_config(&owner_id, 0, fixture.config_two(1), NOW)
            .unwrap();
        let both_non_candidates = [owner_id.clone(), ordinary_id.clone()];

        let issued = store
            .issue_emergency_leader(
                &owner_id,
                &fixture.network.network_id,
                &both_non_candidates,
                NOW,
            )
            .unwrap();
        let first_lease = issued.status.emergency_leader.unwrap();
        let candidate_recovered = BTreeMap::from([
            (owner_id.clone(), NetworkMemberPresence::live(NOW + 1)),
            (ordinary_id.clone(), NetworkMemberPresence::live(NOW + 1)),
            (candidate_one_id, NetworkMemberPresence::live(NOW + 1)),
        ]);
        assert_eq!(
            store
                .reconcile_emergency_leases(&candidate_recovered, NOW + 1)
                .unwrap(),
            vec![fixture.network.network_id.clone()]
        );

        let issued = store
            .issue_emergency_leader(
                &owner_id,
                &fixture.network.network_id,
                &both_non_candidates,
                NOW + 2,
            )
            .unwrap();
        let second_lease = issued.status.emergency_leader.unwrap();
        let surviving_non_candidate = if second_lease.leader == owner_id {
            ordinary_id.clone()
        } else {
            owner_id.clone()
        };
        let leader_offline = BTreeMap::from([(
            surviving_non_candidate,
            NetworkMemberPresence::live(NOW + 3),
        )]);
        assert_eq!(
            store
                .reconcile_emergency_leases(&leader_offline, NOW + 3)
                .unwrap(),
            vec![fixture.network.network_id.clone()]
        );

        let issued = store
            .issue_emergency_leader(
                &owner_id,
                &fixture.network.network_id,
                &both_non_candidates,
                NOW + 4,
            )
            .unwrap();
        let third_lease = issued.status.emergency_leader.unwrap();
        let live_non_candidates = BTreeMap::from([
            (owner_id, NetworkMemberPresence::live(NOW + 4)),
            (ordinary_id, NetworkMemberPresence::live(NOW + 4)),
        ]);
        assert_eq!(
            store
                .reconcile_emergency_leases(&live_non_candidates, third_lease.expires_at_unix_ms,)
                .unwrap(),
            vec![fixture.network.network_id.clone()]
        );

        let status = store
            .status(
                &NodeId::from_public_key(&fixture.owner.public_key()),
                &fixture.network.network_id,
                third_lease.expires_at_unix_ms,
            )
            .unwrap();
        assert!(status.emergency_leader.is_none());

        let audit = store.audit_entries().unwrap();
        for (epoch, reason_code) in [
            (
                first_lease.emergency_epoch,
                "coordinator.emergency.reason.candidate_recovered",
            ),
            (
                second_lease.emergency_epoch,
                "coordinator.emergency.reason.leader_offline",
            ),
            (
                third_lease.emergency_epoch,
                "coordinator.emergency.reason.lease_expired",
            ),
        ] {
            assert!(audit.iter().any(|entry| {
                entry.action == "coordinator.emergency.leader.revoked"
                    && entry.details.get("reason_code") == Some(&reason_code.into())
                    && entry.details.get("emergency_epoch") == Some(&epoch.to_string())
                    && entry.details.get("actor_role") == Some(&"system".into())
            }));
        }
    }

    #[test]
    fn emergency_lease_is_bounded_and_restart_never_extends_it() {
        let fixture = Fixture::new("emergency-bounded");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let ordinary_id = NodeId::from_public_key(&fixture.ordinary.public_key());
        store
            .update_config(&owner_id, 0, fixture.config_two(1), NOW)
            .unwrap();
        let online = vec![owner_id.clone(), ordinary_id.clone()];
        let issued = store
            .issue_emergency_leader(&owner_id, &fixture.network.network_id, &online, NOW)
            .unwrap();
        let lease = issued.status.emergency_leader.clone().unwrap();
        assert!(!issued.renewed);
        assert_eq!(lease.issued_at_unix_ms, NOW);
        assert_eq!(
            lease.hard_expires_at_unix_ms - lease.issued_at_unix_ms,
            fuckipv6_common::domain::COORDINATOR_EMERGENCY_MAX_DURATION_MS
        );
        assert!(!issued.status.control_plane.governance_writable);
        assert_eq!(
            issued.status.control_plane.reason_code,
            "coordinator.control_plane.emergency_runtime_only"
        );

        let credential = CoordinatorEmergencyWriteCredential {
            network_id: fixture.network.network_id.clone(),
            leader: lease.leader.clone(),
            emergency_epoch: lease.emergency_epoch,
            candidate_config_revision: lease.candidate_config_revision,
        };
        let renewed = store
            .renew_emergency_leader(
                &lease.leader,
                &credential,
                &fixture.network.network_id,
                &online,
                NOW + 20_000,
            )
            .unwrap();
        assert!(renewed.renewed);
        let renewed_lease = renewed.status.emergency_leader.unwrap();
        assert_eq!(renewed_lease.expires_at_unix_ms, NOW + 50_000);
        assert_eq!(
            renewed_lease.hard_expires_at_unix_ms,
            lease.hard_expires_at_unix_ms
        );

        let restored = fixture
            .store()
            .status(&owner_id, &fixture.network.network_id, NOW + 30_000)
            .unwrap();
        let restored_lease = restored.emergency_leader.unwrap();
        assert!(restored_lease.is_active());
        assert_eq!(restored_lease.remaining_ms, 20_000);
        assert_eq!(
            restored_lease.hard_expires_at_unix_ms,
            lease.hard_expires_at_unix_ms
        );
    }

    #[test]
    fn emergency_lease_expiry_cannot_be_renewed_or_revived_by_restart() {
        let fixture = Fixture::new("emergency-expiry");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let ordinary_id = NodeId::from_public_key(&fixture.ordinary.public_key());
        store
            .update_config(&owner_id, 0, fixture.config_two(1), NOW)
            .unwrap();
        let online = vec![owner_id.clone(), ordinary_id];
        let issued = store
            .issue_emergency_leader(&owner_id, &fixture.network.network_id, &online, NOW)
            .unwrap();
        let lease = issued.status.emergency_leader.unwrap();
        let credential = CoordinatorEmergencyWriteCredential {
            network_id: fixture.network.network_id.clone(),
            leader: lease.leader.clone(),
            emergency_epoch: lease.emergency_epoch,
            candidate_config_revision: lease.candidate_config_revision,
        };
        let error = store
            .renew_emergency_leader(
                &lease.leader,
                &credential,
                &fixture.network.network_id,
                &online,
                lease.expires_at_unix_ms,
            )
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("domain.coordinator_emergency_lease_expired"),
            "{error}"
        );

        let restored = fixture
            .store()
            .status(
                &owner_id,
                &fixture.network.network_id,
                lease.expires_at_unix_ms + 1,
            )
            .unwrap();
        let restored_lease = restored.emergency_leader.unwrap();
        assert_eq!(
            restored_lease.state,
            fuckipv6_common::domain::CoordinatorEmergencyLeaderState::Expired
        );
        assert!(!restored_lease.is_active());
        assert!(!restored.control_plane.governance_writable);
    }

    #[test]
    fn emergency_lease_revocation_retains_epoch_high_water_mark() {
        let fixture = Fixture::new("emergency-epoch");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let ordinary_id = NodeId::from_public_key(&fixture.ordinary.public_key());
        store
            .update_config(&owner_id, 0, fixture.config_two(1), NOW)
            .unwrap();
        let online = vec![owner_id.clone(), ordinary_id];
        let first = store
            .issue_emergency_leader(&owner_id, &fixture.network.network_id, &online, NOW)
            .unwrap();
        let first_epoch = first
            .status
            .emergency_leader
            .as_ref()
            .unwrap()
            .emergency_epoch;
        store
            .revoke_emergency_leader(
                &owner_id,
                &fixture.network.network_id,
                "coordinator.emergency.reason.operator_revoked",
                NOW + 1,
            )
            .unwrap();

        let second = fixture
            .store()
            .issue_emergency_leader(&owner_id, &fixture.network.network_id, &online, NOW + 2)
            .unwrap();
        let second_epoch = second
            .status
            .emergency_leader
            .as_ref()
            .unwrap()
            .emergency_epoch;
        assert!(second_epoch > first_epoch);
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&store.path).unwrap()).unwrap();
        assert_eq!(raw["schema_version"], 3);
        assert_eq!(
            raw["networks"][fixture.network.network_id.to_string()]["emergency_epoch"],
            second_epoch
        );
    }

    #[test]
    fn candidate_recovery_revokes_emergency_lease_and_is_audited() {
        let fixture = Fixture::new("emergency-candidate-recovery");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let ordinary_id = NodeId::from_public_key(&fixture.ordinary.public_key());
        let candidate_one_id = NodeId::from_public_key(&fixture.candidate_one.public_key());
        store
            .update_config(&owner_id, 0, fixture.config_two(1), NOW)
            .unwrap();
        let issued = store
            .issue_emergency_leader(
                &owner_id,
                &fixture.network.network_id,
                &[owner_id.clone(), ordinary_id.clone()],
                NOW,
            )
            .unwrap();
        let lease = issued.status.emergency_leader.unwrap();
        let credential = CoordinatorEmergencyWriteCredential {
            network_id: fixture.network.network_id.clone(),
            leader: lease.leader.clone(),
            emergency_epoch: lease.emergency_epoch,
            candidate_config_revision: lease.candidate_config_revision,
        };
        let error = store
            .renew_emergency_leader(
                &lease.leader,
                &credential,
                &fixture.network.network_id,
                &[owner_id.clone(), ordinary_id, candidate_one_id],
                NOW + 1_000,
            )
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("domain.coordinator_emergency_candidate_online"),
            "{error}"
        );
        let status = store
            .status(&owner_id, &fixture.network.network_id, NOW + 1_001)
            .unwrap();
        assert!(status.emergency_leader.is_none());
        assert!(store.audit_entries().unwrap().iter().any(|entry| {
            entry.action == "coordinator.emergency.leader.revoked"
                && entry.outcome == AuditOutcome::Succeeded
                && entry.details.get("reason_code")
                    == Some(&"coordinator.emergency.reason.candidate_recovered".into())
        }));
    }

    #[test]
    fn successor_term_install_atomically_revokes_emergency_authority() {
        let fixture = Fixture::new("emergency-successor-term");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let ordinary_id = NodeId::from_public_key(&fixture.ordinary.public_key());
        let candidate_one_id = NodeId::from_public_key(&fixture.candidate_one.public_key());
        store
            .update_config(&owner_id, 0, fixture.config_two(1), NOW)
            .unwrap();
        let issued = store
            .issue_emergency_leader(
                &owner_id,
                &fixture.network.network_id,
                &[owner_id.clone(), ordinary_id.clone()],
                NOW,
            )
            .unwrap();
        let lease = issued.status.emergency_leader.unwrap();
        let emergency_credential = CoordinatorEmergencyWriteCredential {
            network_id: fixture.network.network_id.clone(),
            leader: lease.leader.clone(),
            emergency_epoch: lease.emergency_epoch,
            candidate_config_revision: lease.candidate_config_revision,
        };

        let term = fixture.term(1, 1, &fixture.candidate_one, NOW, NOW + 60_000);
        let handed_off = store
            .install_term(&candidate_one_id, term.clone(), NOW + 1)
            .unwrap();
        assert_eq!(
            handed_off.latest_term.as_ref().map(|term| term.term),
            Some(1)
        );
        assert_eq!(
            handed_off
                .latest_term
                .as_ref()
                .map(|term| term.fencing_token),
            Some(FencingToken(1))
        );
        assert_eq!(handed_off.current_leader, Some(candidate_one_id.clone()));
        assert!(handed_off.control_plane.governance_writable);
        assert!(handed_off.emergency_leader.is_none());

        store
            .authorize_governance_write(
                &candidate_one_id,
                &fixture.credential(&term),
                CoordinatorWriteKind::Membership,
                NOW + 2,
            )
            .unwrap();
        let stale_emergency_error = store
            .renew_emergency_leader(
                &lease.leader,
                &emergency_credential,
                &fixture.network.network_id,
                &[owner_id.clone(), ordinary_id],
                NOW + 3,
            )
            .unwrap_err()
            .to_string();
        assert!(
            stale_emergency_error.contains("coordinator_candidate.error.emergency_missing"),
            "{stale_emergency_error}"
        );

        let audit = store.audit_entries().unwrap();
        assert!(
            audit.iter().any(|entry| {
                entry.action == "coordinator.emergency.leader.revoked"
                    && entry.outcome == AuditOutcome::Succeeded
                    && entry.details.get("reason_code")
                        == Some(&"coordinator.emergency.reason.successor_term_installed".into())
                    && entry.details.get("successor_term") == Some(&"1".into())
                    && entry.details.contains_key("successor_fencing_token")
            }),
            "{audit:#?}"
        );

        let restarted = fixture.store();
        let status = restarted
            .status(&candidate_one_id, &fixture.network.network_id, NOW + 4)
            .unwrap();
        assert_eq!(status.current_leader, Some(candidate_one_id));
        assert!(status.emergency_leader.is_none());
        assert!(status.control_plane.governance_writable);
    }

    #[test]
    fn failed_successor_install_preserves_emergency_lease_and_freeze() {
        let fixture = Fixture::new("emergency-failed-handoff");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let ordinary_id = NodeId::from_public_key(&fixture.ordinary.public_key());
        let candidate_one_id = NodeId::from_public_key(&fixture.candidate_one.public_key());
        let candidate_two_id = NodeId::from_public_key(&fixture.candidate_two.public_key());
        store
            .update_config(&owner_id, 0, fixture.config_two(1), NOW)
            .unwrap();
        let issued = store
            .issue_emergency_leader(
                &owner_id,
                &fixture.network.network_id,
                &[owner_id.clone(), ordinary_id.clone()],
                NOW,
            )
            .unwrap();
        let lease = issued.status.emergency_leader.unwrap();
        let emergency_credential = CoordinatorEmergencyWriteCredential {
            network_id: fixture.network.network_id.clone(),
            leader: lease.leader.clone(),
            emergency_epoch: lease.emergency_epoch,
            candidate_config_revision: lease.candidate_config_revision,
        };

        // A single proof is not a strict majority of the two-candidate group.
        let invalid_term = fixture.term_with_revision_and_group(
            1,
            1,
            &fixture.candidate_one,
            1,
            vec![candidate_one_id.clone(), candidate_two_id],
            &[&fixture.candidate_one],
            NOW,
            NOW + 60_000,
        );
        let error = store
            .install_term(&candidate_one_id, invalid_term, NOW + 1)
            .unwrap_err()
            .to_string();
        assert!(error.contains("coordinator_term.error.invalid"), "{error}");

        let rejected = store
            .status(&owner_id, &fixture.network.network_id, NOW + 1)
            .unwrap();
        assert!(rejected.latest_term.is_none());
        assert!(!rejected.control_plane.governance_writable);
        let still_active = rejected.emergency_leader.as_ref().unwrap();
        assert!(still_active.is_active());
        assert_eq!(still_active.emergency_epoch, lease.emergency_epoch);

        let governance_error = store
            .update_config(&owner_id, 1, fixture.config_two(2), NOW + 2)
            .unwrap_err()
            .to_string();
        assert_eq!(
            governance_error,
            "domain.coordinator_emergency_governance_forbidden"
        );
        let renewed = store
            .renew_emergency_leader(
                &lease.leader,
                &emergency_credential,
                &fixture.network.network_id,
                &[owner_id, ordinary_id],
                NOW + 3,
            )
            .unwrap();
        assert!(renewed.renewed);
        assert!(!renewed.status.control_plane.governance_writable);
        assert!(renewed
            .status
            .emergency_leader
            .as_ref()
            .unwrap()
            .is_active());
        assert!(store.audit_entries().unwrap().iter().any(|entry| {
            entry.action == "coordinator.term.install.rejected"
                && entry.outcome != AuditOutcome::Succeeded
        }));
    }

    #[test]
    fn concurrent_split_brain_successors_install_exactly_one_authority() {
        let fixture = Fixture::new("emergency-concurrent-successors");
        let store = Arc::new(fixture.store());
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let ordinary_id = NodeId::from_public_key(&fixture.ordinary.public_key());
        let candidate_one_id = NodeId::from_public_key(&fixture.candidate_one.public_key());
        let candidate_two_id = NodeId::from_public_key(&fixture.candidate_two.public_key());
        store
            .update_config(&owner_id, 0, fixture.config_two(1), NOW)
            .unwrap();
        store
            .issue_emergency_leader(
                &owner_id,
                &fixture.network.network_id,
                &[owner_id.clone(), ordinary_id],
                NOW,
            )
            .unwrap();

        let first_term = fixture.term(1, 1, &fixture.candidate_one, NOW, NOW + 60_000);
        let second_term = fixture.term(1, 2, &fixture.candidate_two, NOW, NOW + 60_000);
        let barrier = Arc::new(Barrier::new(2));
        let first_handle = {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            let term = first_term.clone();
            let actor = candidate_one_id.clone();
            thread::spawn(move || {
                barrier.wait();
                store.install_term(&actor, term, NOW + 1)
            })
        };
        let second_handle = {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            let term = second_term.clone();
            let actor = candidate_two_id.clone();
            thread::spawn(move || {
                barrier.wait();
                store.install_term(&actor, term, NOW + 1)
            })
        };
        let results = [first_handle.join().unwrap(), second_handle.join().unwrap()];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        let conflict = results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assert_eq!(conflict.len(), 1);
        assert!(conflict[0].contains("domain.coordinator_term_conflict"));

        let status = store
            .status(&owner_id, &fixture.network.network_id, NOW + 2)
            .unwrap();
        let installed = status.latest_term.as_ref().unwrap();
        assert_eq!(installed.term, 1);
        assert!(status.emergency_leader.is_none());
        assert!(status.control_plane.governance_writable);
        let (winner, winner_term, loser_term) = if installed.leader == candidate_one_id {
            (candidate_one_id, first_term, second_term)
        } else {
            (candidate_two_id, second_term, first_term)
        };
        assert_eq!(status.current_leader, Some(winner.clone()));
        store
            .authorize_governance_write(
                &winner,
                &fixture.credential(&winner_term),
                CoordinatorWriteKind::Membership,
                NOW + 3,
            )
            .unwrap();
        let loser_error = store
            .authorize_governance_write(
                &loser_term.leader,
                &fixture.credential(&loser_term),
                CoordinatorWriteKind::Membership,
                NOW + 3,
            )
            .unwrap_err()
            .to_string();
        assert!(
            loser_error.contains("domain.coordinator_term_conflict")
                || loser_error.contains("domain.fencing_token"),
            "{loser_error}"
        );
        assert_eq!(
            store
                .audit_entries()
                .unwrap()
                .iter()
                .filter(|entry| {
                    entry.action == "coordinator.emergency.leader.revoked"
                        && entry.details.get("reason_code")
                            == Some(&"coordinator.emergency.reason.successor_term_installed".into())
                })
                .count(),
            1
        );
    }

    #[test]
    fn active_emergency_lease_freezes_candidate_configuration_bootstrap() {
        let fixture = Fixture::new("emergency-config-freeze");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let ordinary_id = NodeId::from_public_key(&fixture.ordinary.public_key());
        store
            .update_config(&owner_id, 0, fixture.config_two(1), NOW)
            .unwrap();
        let issued = store
            .issue_emergency_leader(
                &owner_id,
                &fixture.network.network_id,
                &[owner_id.clone(), ordinary_id],
                NOW,
            )
            .unwrap();
        let lease = issued.status.emergency_leader.unwrap();

        let error = store
            .update_config(&owner_id, 1, fixture.config_two(2), NOW + 1)
            .unwrap_err()
            .to_string();
        assert_eq!(error, "domain.coordinator_emergency_governance_forbidden");

        let status = store
            .status(&owner_id, &fixture.network.network_id, NOW + 1)
            .unwrap();
        assert_eq!(status.config.as_ref().unwrap().revision, 1);
        let active = status.emergency_leader.as_ref().unwrap();
        assert!(active.is_active());
        assert_eq!(active.emergency_epoch, lease.emergency_epoch);

        let audit = store
            .audit_entries()
            .unwrap()
            .into_iter()
            .find(|entry| entry.action == "coordinator.emergency.governance.rejected")
            .expect("emergency governance rejection must be audited");
        assert_eq!(audit.outcome, AuditOutcome::Denied);
        assert_eq!(
            audit.details.get("action").map(String::as_str),
            Some("candidate_config_update")
        );
        assert_eq!(audit.details.get("leader"), Some(&lease.leader.to_string()));
        assert_eq!(
            audit.details.get("emergency_epoch"),
            Some(&lease.emergency_epoch.to_string())
        );
        assert_eq!(
            audit.details.get("candidate_config_revision"),
            Some(&lease.candidate_config_revision.to_string())
        );
    }

    #[test]
    fn expired_emergency_lease_does_not_block_candidate_configuration_bootstrap() {
        let fixture = Fixture::new("expired-emergency-config-freeze");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let ordinary_id = NodeId::from_public_key(&fixture.ordinary.public_key());
        store
            .update_config(&owner_id, 0, fixture.config_two(1), NOW)
            .unwrap();
        store
            .issue_emergency_leader(
                &owner_id,
                &fixture.network.network_id,
                &[owner_id.clone(), ordinary_id],
                NOW,
            )
            .unwrap();

        let after_expiry = NOW + fuckipv6_common::domain::COORDINATOR_EMERGENCY_LEASE_DURATION_MS;
        let updated = store
            .update_config(&owner_id, 1, fixture.config_two(2), after_expiry)
            .unwrap();
        assert_eq!(updated.config.as_ref().unwrap().revision, 2);
        assert!(updated.emergency_leader.is_none());
        assert!(!store.audit_entries().unwrap().iter().any(|entry| {
            entry.action == "coordinator.emergency.governance.rejected"
                && entry.details.get("action").map(String::as_str)
                    == Some("candidate_config_update")
        }));
    }

    #[test]
    fn governance_write_gate_reports_active_leases_and_is_noop_without_them() {
        let fixture = Fixture::new("emergency-governance-write-gate");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let ordinary_id = NodeId::from_public_key(&fixture.ordinary.public_key());
        let invitee_id = NodeId::from_public_key(&Identity::generate().unwrap().public_key());

        assert!(!store
            .reject_emergency_governance_write(
                &invitee_id,
                &fixture.network.network_id,
                "invitation_create",
                NOW,
            )
            .unwrap());
        assert!(!store
            .audit_entries()
            .unwrap()
            .iter()
            .any(|entry| { entry.action == "coordinator.emergency.governance.rejected" }));

        store
            .update_config(&owner_id, 0, fixture.config_two(1), NOW)
            .unwrap();
        let issued = store
            .issue_emergency_leader(
                &owner_id,
                &fixture.network.network_id,
                &[owner_id.clone(), ordinary_id],
                NOW,
            )
            .unwrap();
        let lease = issued.status.emergency_leader.unwrap();
        assert!(store
            .reject_emergency_governance_write(
                &invitee_id,
                &fixture.network.network_id,
                "invitation_redeem",
                NOW + 1,
            )
            .unwrap());

        let audit = store
            .audit_entries()
            .unwrap()
            .into_iter()
            .find(|entry| {
                entry.action == "coordinator.emergency.governance.rejected"
                    && entry.details.get("action").map(String::as_str) == Some("invitation_redeem")
            })
            .expect("governance rejection must be audited");
        assert_eq!(audit.outcome, AuditOutcome::Denied);
        assert_eq!(audit.details.get("leader"), Some(&lease.leader.to_string()));
        assert_eq!(
            audit.details.get("emergency_epoch"),
            Some(&lease.emergency_epoch.to_string())
        );
        assert_eq!(
            audit.details.get("candidate_config_revision"),
            Some(&lease.candidate_config_revision.to_string())
        );
        let persisted = store
            .status(&owner_id, &fixture.network.network_id, NOW + 1)
            .unwrap();
        assert!(persisted.emergency_leader.unwrap().is_active());
    }

    #[test]
    fn legacy_v2_candidate_store_loads_and_upgrades_on_next_write() {
        let fixture = Fixture::new("emergency-v2-migration");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        store
            .update_config(&owner_id, 0, fixture.config_two(1), NOW)
            .unwrap();
        let mut raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&store.path).unwrap()).unwrap();
        raw["schema_version"] = 2.into();
        let stored = raw["networks"][fixture.network.network_id.to_string()]
            .as_object_mut()
            .unwrap();
        stored.remove("emergency_leader");
        stored.remove("emergency_epoch");
        std::fs::write(&store.path, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();

        fixture
            .store()
            .status(&owner_id, &fixture.network.network_id, NOW)
            .unwrap();
        fixture
            .store()
            .update_config(&owner_id, 1, fixture.config_two(2), NOW + 1)
            .unwrap();
        let upgraded: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&store.path).unwrap()).unwrap();
        assert_eq!(upgraded["schema_version"], 3);
    }

    #[test]
    fn bounded_clock_rollback_marks_authority_read_only() {
        let fixture = Fixture::new("term-clock");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let candidate_one_id = NodeId::from_public_key(&fixture.candidate_one.public_key());
        store
            .update_config(&owner_id, 0, fixture.config_two(1), NOW)
            .unwrap();
        store
            .install_term(
                &candidate_one_id,
                fixture.term(1, 1, &fixture.candidate_one, NOW, NOW + 60_000),
                NOW,
            )
            .unwrap();

        let status = store
            .status(
                &owner_id,
                &fixture.network.network_id,
                NOW - fuckipv6_common::domain::COORDINATOR_CLOCK_SKEW_LIMIT_MS - 1,
            )
            .unwrap();
        assert_eq!(status.current_leader, None);
        assert_eq!(status.lease.reason_code, "coordinator.lease.clock_skew");
        assert_eq!(
            status.lease.observed_clock_skew_ms,
            fuckipv6_common::domain::COORDINATOR_CLOCK_SKEW_LIMIT_MS + 1
        );
    }

    #[test]
    fn post_term_configuration_write_requires_matching_authority() {
        let fixture = Fixture::new("term-config-write");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let candidate_one_id = NodeId::from_public_key(&fixture.candidate_one.public_key());
        let config_one = fixture.config(
            1,
            vec![
                CoordinatorCandidate {
                    node_id: owner_id.clone(),
                    priority: 10,
                },
                CoordinatorCandidate {
                    node_id: candidate_one_id.clone(),
                    priority: 20,
                },
            ],
        );
        store.update_config(&owner_id, 0, config_one, NOW).unwrap();
        let candidate_group = vec![owner_id.clone(), candidate_one_id.clone()];
        let majority_votes = [&fixture.owner, &fixture.candidate_one]
            .into_iter()
            .map(|voter| {
                fuckipv6_common::domain::CoordinatorElectionVote::signed(
                    fixture.network.network_id.clone(),
                    1,
                    1,
                    owner_id.clone(),
                    candidate_group.clone(),
                    NOW,
                    NOW + COORDINATOR_ELECTION_VOTE_MAX_VALIDITY_MS,
                    voter,
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        let term = CoordinatorTerm {
            network_id: fixture.network.network_id.clone(),
            term: 1,
            leader: owner_id.clone(),
            candidate_group,
            candidate_config_revision: 1,
            majority_proof: vec![owner_id.clone(), candidate_one_id.clone()],
            majority_votes,
            authority: fuckipv6_common::domain::CoordinatorAuthority::Full,
            phase: fuckipv6_common::domain::CoordinatorPhase::Stable,
            fencing_token: fuckipv6_common::domain::FencingToken(10),
            issued_at_unix_ms: NOW,
            expires_at_unix_ms: NOW + 60_000,
        };
        let credential = fixture.credential(&term);
        store.install_term(&owner_id, term, NOW).unwrap();

        let config_two = fixture.config(
            2,
            vec![
                CoordinatorCandidate {
                    node_id: owner_id.clone(),
                    priority: 10,
                },
                CoordinatorCandidate {
                    node_id: candidate_one_id.clone(),
                    priority: 20,
                },
            ],
        );

        assert_eq!(
            store
                .update_config(&owner_id, 1, config_two.clone(), NOW + 1)
                .unwrap_err()
                .to_string(),
            "coordinator_term.error.write_requires_term"
        );
        store
            .update_config_authorized(&owner_id, 1, config_two, &credential, NOW + 2)
            .unwrap();
        let mut stale = credential;
        stale.fencing_token = FencingToken(9);
        let stale_write_error = store
            .authorize_governance_write(
                &owner_id,
                &stale,
                CoordinatorWriteKind::CandidateSet,
                NOW + 3,
            )
            .unwrap_err()
            .to_string();
        assert!(
            stale_write_error.contains("domain.fencing_token_rollback"),
            "{stale_write_error}"
        );
        let audit = store.audit_entries().unwrap();
        assert!(
            audit.iter().any(|entry| {
                entry.action == "coordinator.candidate.config.update"
                    && entry.outcome == AuditOutcome::Denied
                    && entry.details.get("error")
                        == Some(&"coordinator_term.error.write_requires_term".into())
            }),
            "{audit:#?}"
        );
    }

    fn network_audit_count(store: &CoordinatorCandidateStore, network_id: &NetworkId) -> u64 {
        store
            .audit_entries()
            .unwrap()
            .iter()
            .filter(|entry| entry.network_id.as_ref() == Some(network_id))
            .count() as u64
    }

    #[test]
    fn old_leader_recovery_enters_read_only_before_rejoining() {
        let fixture = Fixture::new("old-leader-read-only");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let candidate_one_id = NodeId::from_public_key(&fixture.candidate_one.public_key());
        let candidate_two_id = NodeId::from_public_key(&fixture.candidate_two.public_key());
        store
            .update_config(&owner_id, 0, fixture.config_two(1), NOW)
            .unwrap();
        store
            .install_term(
                &candidate_one_id,
                fixture.term(1, 10, &fixture.candidate_one, NOW, NOW + 60_000),
                NOW,
            )
            .unwrap();
        store
            .install_term(
                &candidate_two_id,
                fixture.term(2, 20, &fixture.candidate_two, NOW, NOW + 60_000),
                NOW,
            )
            .unwrap();

        let response = store
            .recover(
                &candidate_one_id,
                CoordinatorRecoveryReport {
                    network_id: fixture.network.network_id.clone(),
                    observed_term: Some(1),
                    observed_fencing_token: Some(FencingToken(10)),
                    observed_leader: Some(candidate_one_id.clone()),
                    observed_candidate_config_revision: Some(1),
                    observed_audit_sequence: network_audit_count(
                        &store,
                        &fixture.network.network_id,
                    ),
                },
                NOW + 1,
            )
            .unwrap();

        assert_eq!(response.recovery.stage, CoordinatorRecoveryStage::ReadOnly);
        assert_eq!(response.recovery.role, CoordinatorRecoveryRole::Candidate);
        assert_eq!(
            response.recovery.reason_code,
            "coordinator.recovery.stale_old_leader"
        );
        assert!(!response.recovery.governance_writable);
        assert!(response.recovery.health_checks_allowed);
        assert!(response.recovery.data_plane_allowed);
        assert_eq!(response.recovery.authoritative_term, 2);
        assert_eq!(response.recovery.authoritative_leader, candidate_two_id);
        assert!(response.recovery.rejects_governance_write());

        let audit = store.audit_entries().unwrap();
        let recovered = audit
            .iter()
            .find(|entry| entry.action == "coordinator.recovery.old_leader")
            .expect("old leader recovery must be audited");
        assert_eq!(recovered.outcome, AuditOutcome::Succeeded);
        assert_eq!(
            recovered.details.get("governance_writable"),
            Some(&"false".into())
        );
    }

    #[test]
    fn recovery_catches_up_audit_then_rejoins_without_write_authority() {
        let fixture = Fixture::new("recovery-audit-catchup");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let candidate_one_id = NodeId::from_public_key(&fixture.candidate_one.public_key());
        store
            .update_config(&owner_id, 0, fixture.config_two(1), NOW)
            .unwrap();
        store
            .install_term(
                &candidate_one_id,
                fixture.term(2, 20, &fixture.candidate_one, NOW, NOW + 60_000),
                NOW,
            )
            .unwrap();
        let report = CoordinatorRecoveryReport {
            network_id: fixture.network.network_id.clone(),
            observed_term: Some(2),
            observed_fencing_token: Some(FencingToken(20)),
            observed_leader: Some(candidate_one_id.clone()),
            observed_candidate_config_revision: Some(1),
            observed_audit_sequence: 0,
        };

        let catching_up = store
            .recover(&candidate_one_id, report.clone(), NOW + 1)
            .unwrap();
        assert_eq!(
            catching_up.recovery.stage,
            CoordinatorRecoveryStage::CatchingUp
        );
        assert_eq!(
            catching_up.recovery.reason_code,
            "coordinator.recovery.audit_behind"
        );
        assert!(!catching_up.recovery.governance_writable);
        assert!(!catching_up.audit_entries.is_empty());
        assert!(catching_up.audit_entries.len() <= COORDINATOR_RECOVERY_AUDIT_REPLAY_LIMIT);
        assert!(catching_up.recovery.authoritative_audit_sequence > 0);

        let mut current = report;
        current.observed_audit_sequence = catching_up.recovery.authoritative_audit_sequence;
        let rejoined = store.recover(&candidate_one_id, current, NOW + 2).unwrap();
        assert_eq!(rejoined.recovery.stage, CoordinatorRecoveryStage::Rejoined);
        assert_eq!(
            rejoined.recovery.reason_code,
            "coordinator.recovery.rejoined_candidate"
        );
        assert!(!rejoined.recovery.governance_writable);
        assert!(rejoined.recovery.health_checks_allowed);
        assert!(rejoined.recovery.data_plane_allowed);
    }

    #[test]
    fn rejoin_as_member_when_config_changes_candidate_group() {
        let fixture = Fixture::new("recovery-member-role");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let candidate_one_id = NodeId::from_public_key(&fixture.candidate_one.public_key());
        let candidate_two_id = NodeId::from_public_key(&fixture.candidate_two.public_key());
        let ordinary_id = NodeId::from_public_key(&fixture.ordinary.public_key());
        store
            .update_config(
                &owner_id,
                0,
                fixture.config(
                    1,
                    vec![
                        CoordinatorCandidate {
                            node_id: owner_id.clone(),
                            priority: 10,
                        },
                        CoordinatorCandidate {
                            node_id: candidate_one_id.clone(),
                            priority: 20,
                        },
                    ],
                ),
                NOW,
            )
            .unwrap();
        let old_term = fixture.term_with_revision_and_group(
            1,
            10,
            &fixture.owner,
            1,
            vec![owner_id.clone(), candidate_one_id.clone()],
            &[&fixture.owner, &fixture.candidate_one],
            NOW,
            NOW + 60_000,
        );
        let old_credential = fixture.credential(&old_term);
        store.install_term(&owner_id, old_term, NOW).unwrap();

        let config_two = CoordinatorCandidateConfig::signed(
            fixture.network.network_id.clone(),
            2,
            vec![
                CoordinatorCandidate {
                    node_id: candidate_two_id.clone(),
                    priority: 10,
                },
                CoordinatorCandidate {
                    node_id: ordinary_id.clone(),
                    priority: 20,
                },
            ],
            NOW,
            NOW + 3_600_000,
            &fixture.owner,
        )
        .unwrap();
        store
            .update_config_authorized(&owner_id, 1, config_two, &old_credential, NOW + 1)
            .unwrap();
        let current_term = fixture.term_with_revision_and_group(
            2,
            20,
            &fixture.candidate_two,
            2,
            vec![candidate_two_id.clone(), ordinary_id.clone()],
            &[&fixture.candidate_two, &fixture.ordinary],
            NOW,
            NOW + 60_000,
        );
        store
            .install_term(&candidate_two_id, current_term, NOW + 2)
            .unwrap();

        let response = store
            .recover(
                &candidate_one_id,
                CoordinatorRecoveryReport {
                    network_id: fixture.network.network_id.clone(),
                    observed_term: Some(2),
                    observed_fencing_token: Some(FencingToken(20)),
                    observed_leader: Some(candidate_two_id),
                    observed_candidate_config_revision: Some(2),
                    observed_audit_sequence: network_audit_count(
                        &store,
                        &fixture.network.network_id,
                    ),
                },
                NOW + 3,
            )
            .unwrap();

        assert_eq!(response.recovery.stage, CoordinatorRecoveryStage::Rejoined);
        assert_eq!(response.recovery.role, CoordinatorRecoveryRole::Member);
        assert_eq!(
            response.recovery.reason_code,
            "coordinator.recovery.rejoined_member"
        );
        assert!(!response.recovery.governance_writable);
        assert!(response.recovery.health_checks_allowed);
        assert!(response.recovery.data_plane_allowed);
    }

    #[test]
    fn recovery_rejects_future_term_and_audit_cursor() {
        let fixture = Fixture::new("recovery-future-cursor");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let candidate_one_id = NodeId::from_public_key(&fixture.candidate_one.public_key());
        store
            .update_config(&owner_id, 0, fixture.config_two(1), NOW)
            .unwrap();
        store
            .install_term(
                &candidate_one_id,
                fixture.term(1, 10, &fixture.candidate_one, NOW, NOW + 60_000),
                NOW,
            )
            .unwrap();
        let current_report = CoordinatorRecoveryReport {
            network_id: fixture.network.network_id.clone(),
            observed_term: Some(1),
            observed_fencing_token: Some(FencingToken(10)),
            observed_leader: Some(candidate_one_id.clone()),
            observed_candidate_config_revision: Some(1),
            observed_audit_sequence: 0,
        };
        let mut future = current_report.clone();
        future.observed_term = Some(2);
        future.observed_fencing_token = Some(FencingToken(20));
        assert_eq!(
            store
                .recover(&candidate_one_id, future, NOW + 1)
                .unwrap_err()
                .to_string(),
            "coordinator_term.error.not_installed"
        );

        let mut cursor_ahead = current_report;
        cursor_ahead.observed_audit_sequence =
            network_audit_count(&store, &fixture.network.network_id) + 1;
        assert_eq!(
            store
                .recover(&candidate_one_id, cursor_ahead, NOW + 2)
                .unwrap_err()
                .to_string(),
            "coordinator_recovery.error.audit_cursor_ahead"
        );

        let rejected = store
            .audit_entries()
            .unwrap()
            .into_iter()
            .filter(|entry| entry.action == "coordinator.recovery.rejected")
            .collect::<Vec<_>>();
        assert_eq!(rejected.len(), 2);
        assert!(rejected
            .iter()
            .all(|entry| entry.outcome == AuditOutcome::Denied));
    }

    #[test]
    fn concurrent_recovery_is_durable_and_audited_once() {
        let fixture = Fixture::new("recovery-concurrent");
        let store = Arc::new(fixture.store());
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let candidate_one_id = NodeId::from_public_key(&fixture.candidate_one.public_key());
        let candidate_two_id = NodeId::from_public_key(&fixture.candidate_two.public_key());
        store
            .update_config(&owner_id, 0, fixture.config_two(1), NOW)
            .unwrap();
        store
            .install_term(
                &candidate_one_id,
                fixture.term(1, 10, &fixture.candidate_one, NOW, NOW + 60_000),
                NOW,
            )
            .unwrap();
        store
            .install_term(
                &candidate_two_id,
                fixture.term(2, 20, &fixture.candidate_two, NOW, NOW + 60_000),
                NOW,
            )
            .unwrap();
        let report = CoordinatorRecoveryReport {
            network_id: fixture.network.network_id.clone(),
            observed_term: Some(1),
            observed_fencing_token: Some(FencingToken(10)),
            observed_leader: Some(candidate_one_id.clone()),
            observed_candidate_config_revision: Some(1),
            observed_audit_sequence: network_audit_count(&store, &fixture.network.network_id),
        };
        let barrier = Arc::new(Barrier::new(12));
        let mut handles = Vec::new();
        for index in 0..12 {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            let report = report.clone();
            let actor = candidate_one_id.clone();
            handles.push(thread::spawn(move || {
                barrier.wait();
                store
                    .recover(&actor, report, NOW + 1 + index)
                    .map(|response| response.recovery)
            }));
        }
        for handle in handles {
            let recovery = handle.join().unwrap().unwrap();
            assert_eq!(recovery.stage, CoordinatorRecoveryStage::ReadOnly);
            assert!(!recovery.governance_writable);
            assert!(recovery.health_checks_allowed);
            assert!(recovery.data_plane_allowed);
        }

        let restarted = fixture.store();
        let audit = restarted.audit_entries().unwrap();
        assert_eq!(
            audit
                .iter()
                .filter(|entry| entry.action == "coordinator.recovery.old_leader")
                .count(),
            1,
            "{audit:#?}"
        );
        let status = restarted
            .status(&candidate_one_id, &fixture.network.network_id, NOW + 20)
            .unwrap();
        assert_eq!(status.latest_term.as_ref().map(|term| term.term), Some(2));
    }

    #[test]
    fn recovered_old_credential_remains_rejected() {
        let fixture = Fixture::new("recovered-old-credential");
        let store = fixture.store();
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let candidate_one_id = NodeId::from_public_key(&fixture.candidate_one.public_key());
        let candidate_two_id = NodeId::from_public_key(&fixture.candidate_two.public_key());
        store
            .update_config(&owner_id, 0, fixture.config_two(1), NOW)
            .unwrap();
        let old_term = fixture.term(1, 10, &fixture.candidate_one, NOW, NOW + 60_000);
        let old_credential = fixture.credential(&old_term);
        store
            .install_term(&candidate_one_id, old_term, NOW)
            .unwrap();
        store
            .install_term(
                &candidate_two_id,
                fixture.term(2, 20, &fixture.candidate_two, NOW, NOW + 60_000),
                NOW,
            )
            .unwrap();
        store
            .recover(
                &candidate_one_id,
                CoordinatorRecoveryReport {
                    network_id: fixture.network.network_id.clone(),
                    observed_term: Some(1),
                    observed_fencing_token: Some(FencingToken(10)),
                    observed_leader: Some(candidate_one_id.clone()),
                    observed_candidate_config_revision: Some(1),
                    observed_audit_sequence: network_audit_count(
                        &store,
                        &fixture.network.network_id,
                    ),
                },
                NOW + 1,
            )
            .unwrap();

        let error = store
            .authorize_governance_write(
                &candidate_one_id,
                &old_credential,
                CoordinatorWriteKind::Membership,
                NOW + 2,
            )
            .unwrap_err()
            .to_string();
        assert!(
            error.starts_with("domain.coordinator_term_rollback"),
            "{error}"
        );
        let audit = store.audit_entries().unwrap();
        assert!(audit.iter().any(|entry| {
            entry.action == "coordinator.term.write.rejected"
                && entry.outcome == AuditOutcome::Denied
                && entry.details.get("error") == Some(&error)
        }));
    }
}
