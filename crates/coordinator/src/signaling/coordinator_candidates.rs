use anyhow::{bail, Context, Result};
use fs2::FileExt;
use fuckipv6_common::domain::{
    AuditEntry, AuditId, AuditLog, AuditOutcome, CoordinatorCandidateConfig,
    CoordinatorCandidateDeclaration, CoordinatorCandidateStatus, CoordinatorTerm,
    CoordinatorWriteCredential, CoordinatorWriteKind, NetworkId, NetworkRecord, NodeId,
    COORDINATOR_MAX_LEASE_DURATION_MS,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

const COORDINATOR_CANDIDATE_FILE_SCHEMA_VERSION: u16 = 1;
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
        if self.schema_version != COORDINATOR_CANDIDATE_FILE_SCHEMA_VERSION {
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
        let _profile_lock = lock_profile(&self.network_record_path, false)?;
        self.ensure_parent()?;
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

    pub(crate) fn notification_recipients(&self, network_id: &NetworkId) -> Result<Vec<NodeId>> {
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("coordinator_candidate.error.store_lock_poisoned"))?;
        let _profile_lock = lock_profile(&self.network_record_path, false)?;
        let network = read_network_record(&self.network_record_path, network_id)?;
        Ok(network.members)
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
                return status_from_file(network, Some(stored), now_unix_ms);
            }
            current.validate_successor(&term, now_unix_ms)?;
        }
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
        let file: CoordinatorCandidateFile = serde_json::from_slice(&content)
            .with_context(|| format!("无法解析协调器候选存储: {}", self.path.display()))?;
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
    CoordinatorCandidateStatus::assess(network, config, &declarations, latest_term, now_unix_ms)
        .context("coordinator_candidate.error.status_projection_failed")
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
        || code.contains("write_forbidden")
        || code.contains("transition_read_only")
        || code.contains("clock_skew")
        || code.contains("lease_expired")
        || code.contains("write_requires_term")
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
        CoordinatorCandidateWillingness, CoordinatorPhase, CoordinatorPublicReachability,
        ExitContributionMode, ExitContributionObligation, ExitContributionScope,
        ExitSelectionPolicy, FencingToken, NetworkCreateProfile, NetworkJoinMode,
        NetworkVisibility, PublicKey,
    };
    use fuckipv6_common::Identity;
    use std::sync::Arc;
    use std::thread;

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
            CoordinatorTerm {
                network_id: self.network.network_id.clone(),
                term,
                leader: NodeId::from_public_key(&leader.public_key()),
                candidate_group: vec![
                    NodeId::from_public_key(&self.candidate_one.public_key()),
                    NodeId::from_public_key(&self.candidate_two.public_key()),
                ],
                majority_proof: vec![
                    NodeId::from_public_key(&self.candidate_one.public_key()),
                    NodeId::from_public_key(&self.candidate_two.public_key()),
                ],
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
        let mut term = fixture.term(1, 10, &fixture.candidate_one, NOW, NOW + 60_000);
        term.leader = owner_id.clone();
        term.candidate_group = vec![owner_id.clone(), candidate_one_id.clone()];
        term.majority_proof = vec![owner_id.clone(), candidate_one_id.clone()];
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
}
