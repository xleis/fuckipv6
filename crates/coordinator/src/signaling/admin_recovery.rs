use anyhow::{bail, Context, Result};
use fs2::FileExt;
use fuckipv6_common::domain::{
    AdminRecoveryExecution, AdminRecoveryRequest, AdminRecoveryRequestId, AdminRecoveryState,
    AdminRecoveryStatusView, AuditEntry, AuditId, AuditLog, AuditOutcome, ContentHash, NetworkId,
    NetworkRecord, NodeId,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;

use super::invitations::InvitationStore;
use super::join_requests::JoinRequestStore;
use super::session::SessionManager;

const ADMIN_RECOVERY_FILE_SCHEMA_VERSION: u16 = 1;

type RecoveryDisconnectCallback<'a> = dyn FnMut(&AdminRecoveryStatusView) -> Result<()> + Send + 'a;
type PreservedRecoverySession<'a> = (&'a str, &'a mut RecoveryDisconnectCallback<'a>);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredAdminRecoveryRequest {
    request: AdminRecoveryRequest,
    received_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    execution: Option<AdminRecoveryExecution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    revoked_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    revoked_by: Option<NodeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    revocation_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminRecoveryFile {
    schema_version: u16,
    #[serde(default)]
    requests: BTreeMap<AdminRecoveryRequestId, StoredAdminRecoveryRequest>,
    #[serde(default)]
    consumed_request_ids: BTreeSet<AdminRecoveryRequestId>,
    #[serde(default)]
    consumed_nonces: BTreeMap<NetworkId, BTreeSet<String>>,
    #[serde(default)]
    audit_log: AuditLog,
}

impl Default for AdminRecoveryFile {
    fn default() -> Self {
        Self {
            schema_version: ADMIN_RECOVERY_FILE_SCHEMA_VERSION,
            requests: BTreeMap::new(),
            consumed_request_ids: BTreeSet::new(),
            consumed_nonces: BTreeMap::new(),
            audit_log: AuditLog::default(),
        }
    }
}

impl AdminRecoveryFile {
    fn validate(&self) -> Result<()> {
        if self.schema_version != ADMIN_RECOVERY_FILE_SCHEMA_VERSION {
            bail!("unsupported admin recovery file schema version");
        }
        let mut nonces = BTreeMap::<NetworkId, BTreeSet<String>>::new();
        for (request_id, stored) in &self.requests {
            let request = &stored.request;
            if request.proposal.request_id != *request_id {
                bail!("admin recovery map key does not match request id");
            }
            request.validate_structure()?;
            if stored.received_at_unix_ms == 0 {
                bail!("admin recovery receipt time is invalid");
            }
            if let Some(execution) = &stored.execution {
                if execution.request_id != *request_id
                    || execution.network_id != request.proposal.network_id
                    || execution.executed_at_unix_ms < stored.received_at_unix_ms
                    || execution.new_revision <= execution.previous_revision
                {
                    bail!("admin recovery execution is inconsistent");
                }
                if !self.consumed_request_ids.contains(request_id) {
                    bail!("executed admin recovery request is not marked consumed");
                }
            }
            if stored.revoked_at_unix_ms.is_some() != stored.revoked_by.is_some() {
                bail!("admin recovery revocation metadata is incomplete");
            }
            if stored.execution.is_some() && stored.revoked_at_unix_ms.is_some() {
                bail!("admin recovery request cannot be both executed and revoked");
            }
            let network_nonces = nonces
                .entry(request.proposal.network_id.clone())
                .or_default();
            if !network_nonces.insert(request.proposal.nonce.clone()) {
                bail!("admin recovery nonce is duplicated");
            }
        }
        if self
            .consumed_request_ids
            .iter()
            .any(|request_id| !self.requests.contains_key(request_id))
        {
            bail!("consumed admin recovery request has no durable record");
        }
        if self.consumed_nonces != nonces {
            bail!("admin recovery consumed nonce index is inconsistent");
        }
        Ok(())
    }
}

/// Durable, append-only administrator recovery ledger.
///
/// Requests are verified against the exact roster snapshot and threshold both
/// at submission and execution. File/profile locks make execution single-writer
/// across threads and processes; the consumed request and nonce indexes prevent
/// replay after restart.
pub struct AdminRecoveryStore {
    path: PathBuf,
    network_record_path: PathBuf,
    process_lock: Mutex<()>,
}

impl AdminRecoveryStore {
    pub fn new(path: impl Into<PathBuf>, network_record_path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let network_record_path = network_record_path.into();
        if path == network_record_path {
            bail!("admin recovery and network record paths must differ");
        }
        Ok(Self {
            path,
            network_record_path,
            process_lock: Mutex::new(()),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Submit on behalf of an authenticated current network member. The
    /// signed request is still independently verified by `submit`; this check
    /// prevents an unrelated registered node from injecting recovery payloads.
    pub fn submit_as(
        &self,
        actor: &NodeId,
        request: AdminRecoveryRequest,
        now_unix_ms: u64,
    ) -> Result<AdminRecoveryStatusView> {
        let network = self.read_network()?;
        if !network.members.contains(actor) {
            bail!("admin_recovery.error.permission_denied");
        }
        self.submit(request, now_unix_ms)
    }

    pub fn is_current_authority(&self, network_id: &NetworkId, actor: &NodeId) -> Result<bool> {
        let network = self.read_network()?;
        if network.network_id != *network_id {
            bail!("admin_recovery.error.network_mismatch");
        }
        Ok(network.owner == *actor || network.administrators.contains(actor))
    }

    /// Verify and durably register a request that already satisfies its current
    /// threshold. Re-submitting the exact same request is idempotent.
    pub fn submit(
        &self,
        request: AdminRecoveryRequest,
        now_unix_ms: u64,
    ) -> Result<AdminRecoveryStatusView> {
        let _process_guard = self.lock_process()?;
        let _file_lock = self.lock_file()?;
        let network = self.read_network()?;
        request.validate_structure()?;
        let network_id = request.proposal.network_id.clone();
        if network_id != network.network_id {
            bail!("admin_recovery.error.network_mismatch");
        }
        let mut file = self.load_unlocked()?;
        if let Some(existing) = file.requests.get(&request.proposal.request_id) {
            if existing.request.proposal.request_digest() != request.proposal.request_digest()
                || existing.request.approvals != request.approvals
            {
                bail!("admin_recovery.error.request_id_conflict");
            }
            return Ok(AdminRecoveryStatusView::new(
                &existing.request,
                &network,
                state_for(existing, now_unix_ms),
                now_unix_ms,
            ));
        }
        if file
            .consumed_request_ids
            .contains(&request.proposal.request_id)
        {
            bail!("admin_recovery.error.replayed");
        }
        if file
            .consumed_nonces
            .get(&network_id)
            .is_some_and(|nonces| nonces.contains(&request.proposal.nonce))
        {
            bail!("admin_recovery.error.replayed");
        }
        let trusted_approvals = request.verify_for_network(&network, now_unix_ms)?;
        if trusted_approvals < request.proposal.required_approvals(&network) {
            bail!("admin_recovery.error.threshold_not_met");
        }
        let view = AdminRecoveryStatusView::new(
            &request,
            &network,
            AdminRecoveryState::Pending,
            now_unix_ms,
        );
        let request_id = request.proposal.request_id.clone();
        let request_digest = request.proposal.request_digest();
        file.requests.insert(
            request_id.clone(),
            StoredAdminRecoveryRequest {
                request,
                received_at_unix_ms: now_unix_ms,
                execution: None,
                revoked_at_unix_ms: None,
                revoked_by: None,
                revocation_reason: None,
            },
        );
        file.consumed_nonces
            .entry(network_id.clone())
            .or_default()
            .insert(
                file.requests
                    .get(&request_id)
                    .expect("inserted admin recovery request")
                    .request
                    .proposal
                    .nonce
                    .clone(),
            );
        append_audit(
            &mut file,
            &network_id,
            &request_id,
            &request_digest,
            None,
            "admin_recovery.request.submitted",
            "admin recovery request submitted",
            AuditOutcome::Succeeded,
            now_unix_ms,
            BTreeMap::from([
                (
                    "required_signatures".into(),
                    view.required_signatures.to_string(),
                ),
                ("signature_count".into(), view.signature_count.to_string()),
            ]),
        )?;
        file.validate()?;
        self.persist(&file)?;
        Ok(view)
    }

    pub fn status(
        &self,
        request_id: &AdminRecoveryRequestId,
        now_unix_ms: u64,
    ) -> Result<AdminRecoveryStatusView> {
        let _process_guard = self.lock_process()?;
        let _file_lock = self.lock_file()?;
        let file = self.load_unlocked()?;
        let stored = file
            .requests
            .get(request_id)
            .context("admin recovery request not found")?;
        let network = self.read_network()?;
        if stored.request.proposal.network_id != network.network_id {
            bail!("admin_recovery.error.network_mismatch");
        }
        Ok(AdminRecoveryStatusView::new(
            &stored.request,
            &network,
            state_for(stored, now_unix_ms),
            now_unix_ms,
        ))
    }

    /// Return a recovery request only to a current owner/administrator, the
    /// request initiator, or an identified approval participant.
    pub fn status_for_actor(
        &self,
        request_id: &AdminRecoveryRequestId,
        actor: &NodeId,
        now_unix_ms: u64,
    ) -> Result<AdminRecoveryStatusView> {
        let _process_guard = self.lock_process()?;
        let _file_lock = self.lock_file()?;
        let file = self.load_unlocked()?;
        let stored = file
            .requests
            .get(request_id)
            .context("admin recovery request not found")?;
        let network = self.read_network()?;
        if stored.request.proposal.network_id != network.network_id {
            bail!("admin_recovery.error.network_mismatch");
        }
        if !can_read_recovery_request(&network, &stored.request, actor) {
            bail!("admin_recovery.error.permission_denied");
        }
        Ok(AdminRecoveryStatusView::new(
            &stored.request,
            &network,
            state_for(stored, now_unix_ms),
            now_unix_ms,
        ))
    }

    pub fn list(
        &self,
        network_id: &NetworkId,
        now_unix_ms: u64,
    ) -> Result<Vec<AdminRecoveryStatusView>> {
        let _process_guard = self.lock_process()?;
        let _file_lock = self.lock_file()?;
        let file = self.load_unlocked()?;
        let network = self.read_network()?;
        if network.network_id != *network_id {
            bail!("admin_recovery.error.network_mismatch");
        }
        let mut views = file
            .requests
            .values()
            .filter(|stored| stored.request.proposal.network_id == *network_id)
            .map(|stored| {
                AdminRecoveryStatusView::new(
                    &stored.request,
                    &network,
                    state_for(stored, now_unix_ms),
                    now_unix_ms,
                )
            })
            .collect::<Vec<_>>();
        views.sort_by(|left, right| {
            right
                .expires_at_unix_ms
                .cmp(&left.expires_at_unix_ms)
                .then_with(|| left.request_id.cmp(&right.request_id))
        });
        Ok(views)
    }

    /// List only the requests visible to the authenticated actor. Current
    /// authorities see the complete network ledger; other actors see requests
    /// they initiated or approved, preventing impact/participant disclosure.
    pub fn list_for_actor(
        &self,
        network_id: &NetworkId,
        actor: &NodeId,
        now_unix_ms: u64,
    ) -> Result<Vec<AdminRecoveryStatusView>> {
        let _process_guard = self.lock_process()?;
        let _file_lock = self.lock_file()?;
        let file = self.load_unlocked()?;
        let network = self.read_network()?;
        if network.network_id != *network_id {
            bail!("admin_recovery.error.network_mismatch");
        }
        let current_authority = network.owner == *actor || network.administrators.contains(actor);
        let mut views = file
            .requests
            .values()
            .filter(|stored| stored.request.proposal.network_id == *network_id)
            .filter(|stored| {
                current_authority || can_read_recovery_request(&network, &stored.request, actor)
            })
            .map(|stored| {
                AdminRecoveryStatusView::new(
                    &stored.request,
                    &network,
                    state_for(stored, now_unix_ms),
                    now_unix_ms,
                )
            })
            .collect::<Vec<_>>();
        views.sort_by(|left, right| {
            right
                .expires_at_unix_ms
                .cmp(&left.expires_at_unix_ms)
                .then_with(|| left.request_id.cmp(&right.request_id))
        });
        Ok(views)
    }

    /// Execute exactly once and update the authoritative roster atomically.
    /// Both the network record and recovery ledger are rolled back if either
    /// durable write fails.
    pub fn execute(
        &self,
        request_id: &AdminRecoveryRequestId,
        now_unix_ms: u64,
    ) -> Result<AdminRecoveryExecution> {
        let _process_guard = self.lock_process()?;
        let _file_lock = self.lock_file()?;
        let _profile_lock = lock_profile_exclusive(&self.network_record_path)?;
        let mut file = self.load_unlocked()?;
        let stored = file
            .requests
            .get(request_id)
            .cloned()
            .context("admin recovery request not found")?;
        if let Some(execution) = stored.execution {
            return Ok(execution);
        }
        if stored.revoked_at_unix_ms.is_some() {
            bail!("admin_recovery.error.revoked");
        }
        let previous_network_bytes = std::fs::read(&self.network_record_path).ok();
        let mut network = self.read_network_unlocked()?;
        if stored.request.proposal.is_expired(now_unix_ms) {
            self.audit_denied(&mut file, &stored, now_unix_ms, "expired")?;
            self.persist(&file)?;
            bail!("admin_recovery.error.expired");
        }
        if let Err(error) = stored.request.verify_for_network(&network, now_unix_ms) {
            self.audit_denied(&mut file, &stored, now_unix_ms, error.code())?;
            self.persist(&file)?;
            return Err(error.into());
        }
        let previous_revision = network.revision;
        let previous_owner = network.owner.clone();
        stored.request.apply_to_network(&mut network, now_unix_ms)?;
        let execution = AdminRecoveryExecution {
            request_id: request_id.clone(),
            network_id: network.network_id.clone(),
            executed_at_unix_ms: now_unix_ms,
            previous_revision,
            new_revision: network.revision,
            previous_owner,
            new_owner: network.owner.clone(),
            invalidated_sessions: stored.request.proposal.snapshot.members.clone(),
            revoke_pending_invitations: stored.request.proposal.revocation.pending_invitations,
            cancel_unconfirmed_writes: stored.request.proposal.revocation.unconfirmed_writes,
        };
        let current = file
            .requests
            .get_mut(request_id)
            .context("admin recovery request disappeared during execution")?;
        current.execution = Some(execution.clone());
        file.consumed_request_ids.insert(request_id.clone());
        file.consumed_nonces
            .entry(stored.request.proposal.network_id.clone())
            .or_default()
            .insert(stored.request.proposal.nonce.clone());
        append_audit(
            &mut file,
            &execution.network_id,
            request_id,
            &stored.request.proposal.request_digest(),
            Some(&execution.new_owner),
            "admin_recovery.executed",
            "admin recovery executed",
            AuditOutcome::Succeeded,
            now_unix_ms,
            BTreeMap::from([
                (
                    "previous_revision".into(),
                    execution.previous_revision.to_string(),
                ),
                ("new_revision".into(), execution.new_revision.to_string()),
                (
                    "invalidated_sessions".into(),
                    execution.invalidated_sessions.len().to_string(),
                ),
                (
                    "revoke_pending_invitations".into(),
                    execution.revoke_pending_invitations.to_string(),
                ),
                (
                    "cancel_unconfirmed_writes".into(),
                    execution.cancel_unconfirmed_writes.to_string(),
                ),
            ]),
        )?;
        file.validate()?;
        if let Err(error) = persist_network_record(&self.network_record_path, &network) {
            restore_file(&self.network_record_path, previous_network_bytes.as_deref())?;
            return Err(error);
        }
        if let Err(error) = self.persist(&file) {
            restore_file(&self.network_record_path, previous_network_bytes.as_deref())?;
            return Err(error);
        }
        Ok(execution)
    }

    /// Cancel a pending request before execution. Only a current owner or
    /// administrator may revoke it, and the attempt is retained in the audit.
    pub fn revoke(
        &self,
        request_id: &AdminRecoveryRequestId,
        actor: &NodeId,
        reason: impl Into<String>,
        now_unix_ms: u64,
    ) -> Result<AdminRecoveryStatusView> {
        let reason = reason.into();
        if reason.is_empty() || reason.chars().count() > 256 {
            bail!("admin_recovery.error.invalid_revocation_reason");
        }
        let _process_guard = self.lock_process()?;
        let _file_lock = self.lock_file()?;
        let mut file = self.load_unlocked()?;
        let network = self.read_network()?;
        if network.owner != *actor && !network.administrators.contains(actor) {
            bail!("admin_recovery.error.permission_denied");
        }
        let stored = file
            .requests
            .get_mut(request_id)
            .context("admin recovery request not found")?;
        if stored.execution.is_some() {
            bail!("admin_recovery.error.already_executed");
        }
        if stored.revoked_at_unix_ms.is_some() {
            return Ok(AdminRecoveryStatusView::new(
                &stored.request,
                &network,
                AdminRecoveryState::Revoked,
                now_unix_ms,
            ));
        }
        stored.revoked_at_unix_ms = Some(now_unix_ms);
        stored.revoked_by = Some(actor.clone());
        stored.revocation_reason = Some(reason.clone());
        let request = stored.request.clone();
        append_audit(
            &mut file,
            &network.network_id,
            request_id,
            &request.proposal.request_digest(),
            Some(actor),
            "admin_recovery.revoked",
            "admin recovery request revoked",
            AuditOutcome::Succeeded,
            now_unix_ms,
            BTreeMap::from([("reason".into(), reason)]),
        )?;
        file.validate()?;
        self.persist(&file)?;
        Ok(AdminRecoveryStatusView::new(
            &request,
            &network,
            AdminRecoveryState::Revoked,
            now_unix_ms,
        ))
    }

    fn record_invalidations_applied(
        &self,
        request_id: &AdminRecoveryRequestId,
        now_unix_ms: u64,
        disconnected_sessions: usize,
        revoked_invitations: usize,
        cancelled_join_requests: usize,
    ) -> Result<()> {
        let _process_guard = self.lock_process()?;
        let _file_lock = self.lock_file()?;
        let mut file = self.load_unlocked()?;
        let stored = file
            .requests
            .get(request_id)
            .cloned()
            .context("admin recovery request not found")?;
        let execution = stored
            .execution
            .clone()
            .context("admin recovery request has not executed")?;
        let resource = format!(
            "network/{}/admin-recovery/{request_id}",
            execution.network_id
        );
        if file.audit_log.entries().iter().any(|entry| {
            entry.action == "admin_recovery.invalidations_applied" && entry.resource == resource
        }) {
            return Ok(());
        }
        append_audit(
            &mut file,
            &execution.network_id,
            request_id,
            &stored.request.proposal.request_digest(),
            Some(&execution.new_owner),
            "admin_recovery.invalidations_applied",
            "old recovery-era sessions, invitations and join requests invalidated",
            AuditOutcome::Succeeded,
            now_unix_ms,
            BTreeMap::from([
                (
                    "disconnected_sessions".into(),
                    disconnected_sessions.to_string(),
                ),
                (
                    "revoked_invitations".into(),
                    revoked_invitations.to_string(),
                ),
                (
                    "cancelled_join_requests".into(),
                    cancelled_join_requests.to_string(),
                ),
                ("new_revision".into(), execution.new_revision.to_string()),
            ]),
        )?;
        file.validate()?;
        self.persist(&file)
    }

    pub fn audit_entries(&self) -> Result<Vec<AuditEntry>> {
        let _process_guard = self.lock_process()?;
        let _file_lock = self.lock_file()?;
        Ok(self.load_unlocked()?.audit_log.entries().to_vec())
    }

    fn audit_denied(
        &self,
        file: &mut AdminRecoveryFile,
        stored: &StoredAdminRecoveryRequest,
        now_unix_ms: u64,
        denial_key: &str,
    ) -> Result<()> {
        append_audit(
            file,
            &stored.request.proposal.network_id,
            &stored.request.proposal.request_id,
            &stored.request.proposal.request_digest(),
            None,
            "admin_recovery.execution.denied",
            "admin recovery execution denied",
            AuditOutcome::Denied,
            now_unix_ms,
            BTreeMap::from([("denial_key".into(), denial_key.to_string())]),
        )
    }

    fn lock_process(&self) -> Result<std::sync::MutexGuard<'_, ()>> {
        self.process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("admin recovery store lock is poisoned"))
    }

    fn lock_file(&self) -> Result<File> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "cannot create admin recovery directory: {}",
                    parent.display()
                )
            })?;
        }
        let lock_path = self.path.with_extension("json.lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("cannot open admin recovery lock: {}", lock_path.display()))?;
        lock.lock_exclusive().with_context(|| {
            format!("cannot lock admin recovery store: {}", lock_path.display())
        })?;
        Ok(lock)
    }

    fn load_unlocked(&self) -> Result<AdminRecoveryFile> {
        if !self.path.exists() {
            return Ok(AdminRecoveryFile::default());
        }
        let content = std::fs::read_to_string(&self.path).with_context(|| {
            format!("cannot read admin recovery store: {}", self.path.display())
        })?;
        let file: AdminRecoveryFile = serde_json::from_str(&content).with_context(|| {
            format!("cannot parse admin recovery store: {}", self.path.display())
        })?;
        file.validate()?;
        Ok(file)
    }

    fn persist(&self, file: &AdminRecoveryFile) -> Result<()> {
        file.validate()?;
        let bytes =
            serde_json::to_vec_pretty(file).context("cannot serialize admin recovery store")?;
        persist_bytes_atomic(&self.path, &bytes)
    }

    fn read_network(&self) -> Result<NetworkRecord> {
        let _lock = lock_profile_shared(&self.network_record_path)?;
        self.read_network_unlocked()
    }

    fn read_network_unlocked(&self) -> Result<NetworkRecord> {
        let content = std::fs::read_to_string(&self.network_record_path).with_context(|| {
            format!(
                "cannot read admin recovery network record: {}",
                self.network_record_path.display()
            )
        })?;
        let record: NetworkRecord = serde_json::from_str(&content)
            .with_context(|| "cannot parse admin recovery network record")?;
        record.validate()?;
        Ok(record)
    }
}

/// Result of a complete recovery transaction including the observable
/// invalidation effects. `execute` only returns after every local stale
/// credential class has been invalidated and the completion audit is durable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminRecoveryCompletion {
    pub execution: AdminRecoveryExecution,
    pub disconnected_sessions: usize,
    pub revoked_invitations: usize,
    pub cancelled_join_requests: usize,
}

/// Coordinates the durable roster transaction with the stores and in-memory
/// sessions that still hold pre-recovery authority. The lock serializes retries
/// so a partial failure can be safely resumed without reporting success early.
pub struct AdminRecoveryService {
    store: Arc<AdminRecoveryStore>,
    invitation_store: Arc<InvitationStore>,
    join_request_store: Arc<JoinRequestStore>,
    session_manager: Arc<SessionManager>,
    execution_lock: AsyncMutex<()>,
}

impl AdminRecoveryService {
    pub fn new(
        store: Arc<AdminRecoveryStore>,
        invitation_store: Arc<InvitationStore>,
        join_request_store: Arc<JoinRequestStore>,
        session_manager: Arc<SessionManager>,
    ) -> Self {
        Self {
            store,
            invitation_store,
            join_request_store,
            session_manager,
            execution_lock: AsyncMutex::new(()),
        }
    }

    /// Execute on behalf of an authenticated actor only when that actor is a
    /// current owner or administrator. The durable store still revalidates the
    /// signed request, threshold, expiry, and exact roster snapshot.
    pub async fn execute_authorized(
        &self,
        actor: &NodeId,
        request_id: &AdminRecoveryRequestId,
        now_unix_ms: u64,
    ) -> Result<AdminRecoveryCompletion> {
        let _execution_guard = self.execution_lock.lock().await;
        let view = self.store.status(request_id, now_unix_ms)?;
        if !self.store.is_current_authority(&view.network_id, actor)? {
            bail!("admin_recovery.error.permission_denied");
        }
        self.execute_locked(request_id, now_unix_ms, None).await
    }

    /// Execute an authenticated recovery while preserving the exact
    /// connection that will carry the success acknowledgement. All other old
    /// sessions are closed immediately. The acknowledgement is queued only
    /// after roster, invitation, join-request and audit persistence succeeds,
    /// then the preserved old session is closed.
    pub async fn execute_authorized_before_actor_disconnect(
        &self,
        actor: &NodeId,
        request_id: &AdminRecoveryRequestId,
        now_unix_ms: u64,
        preserved_client_id: &str,
        mut before_disconnect: impl FnMut(&AdminRecoveryStatusView) -> Result<()> + Send,
    ) -> Result<AdminRecoveryCompletion> {
        let _execution_guard = self.execution_lock.lock().await;
        let view = self.store.status(request_id, now_unix_ms)?;
        if !self.store.is_current_authority(&view.network_id, actor)? {
            bail!("admin_recovery.error.permission_denied");
        }
        self.execute_locked(
            request_id,
            now_unix_ms,
            Some((preserved_client_id, &mut before_disconnect)),
        )
        .await
    }

    /// Apply the approved authority change and invalidate every local artifact
    /// issued under the previous roster. Any error leaves the operation
    /// retryable; success is returned only after all invalidation work and its
    /// audit record are durable. Re-running a completed request is idempotent.
    pub async fn execute(
        &self,
        request_id: &AdminRecoveryRequestId,
        now_unix_ms: u64,
    ) -> Result<AdminRecoveryCompletion> {
        let _execution_guard = self.execution_lock.lock().await;
        self.execute_locked(request_id, now_unix_ms, None).await
    }

    async fn execute_locked(
        &self,
        request_id: &AdminRecoveryRequestId,
        now_unix_ms: u64,
        preserved_session: Option<PreservedRecoverySession<'_>>,
    ) -> Result<AdminRecoveryCompletion> {
        let view = self.store.status(request_id, now_unix_ms)?;
        if view.state == AdminRecoveryState::Pending && !view.can_execute {
            bail!(
                "{}",
                view.denial_key
                    .unwrap_or_else(|| "admin_recovery.error.not_executable".into())
            );
        }

        let execution = self.store.execute(request_id, now_unix_ms)?;
        let revoked_invitations = self.invitation_store.revoke_all_for_recovery(
            &execution.network_id,
            &execution.new_owner,
            now_unix_ms,
        )?;
        let cancelled_join_requests = self.join_request_store.cancel_unconfirmed_for_recovery(
            &execution.network_id,
            &execution.new_owner,
            now_unix_ms,
        )?;
        let snapshot_members = execution.invalidated_sessions.iter().cloned().collect();
        let preserved_client_id = preserved_session.as_ref().map(|(client_id, _)| *client_id);
        let outcome = self
            .session_manager
            .disconnect_sessions_for_nodes_except(&snapshot_members, preserved_client_id)
            .await;
        // A preserved current connection is already part of this invalidation
        // epoch and is disconnected immediately after the acknowledgement is
        // queued, so account for it in the durable completion audit now.
        let disconnected_sessions = outcome.disconnected + outcome.preserved;
        self.store.record_invalidations_applied(
            request_id,
            now_unix_ms,
            disconnected_sessions,
            revoked_invitations,
            cancelled_join_requests,
        )?;
        if let Some((preserved_client_id, before_disconnect)) = preserved_session {
            let status = self.store.status(request_id, now_unix_ms)?;
            let acknowledgement = before_disconnect(&status);
            self.session_manager
                .disconnect_client_session(preserved_client_id)
                .await;
            acknowledgement?;
        }
        Ok(AdminRecoveryCompletion {
            execution,
            disconnected_sessions,
            revoked_invitations,
            cancelled_join_requests,
        })
    }
}

fn can_read_recovery_request(
    network: &NetworkRecord,
    request: &AdminRecoveryRequest,
    actor: &NodeId,
) -> bool {
    network.owner == *actor
        || network.administrators.contains(actor)
        || request.proposal.initiated_by == *actor
        || request
            .approvals
            .iter()
            .any(|approval| approval.signer == *actor)
}

fn state_for(stored: &StoredAdminRecoveryRequest, now_unix_ms: u64) -> AdminRecoveryState {
    if stored.execution.is_some() {
        AdminRecoveryState::Executed
    } else if stored.revoked_at_unix_ms.is_some() {
        AdminRecoveryState::Revoked
    } else if stored.request.proposal.is_expired(now_unix_ms) {
        AdminRecoveryState::Expired
    } else {
        AdminRecoveryState::Pending
    }
}

#[allow(clippy::too_many_arguments)]
fn append_audit(
    file: &mut AdminRecoveryFile,
    network_id: &NetworkId,
    request_id: &AdminRecoveryRequestId,
    request_digest: &ContentHash,
    actor: Option<&NodeId>,
    action: &str,
    summary: &str,
    outcome: AuditOutcome,
    now_unix_ms: u64,
    mut details: BTreeMap<String, String>,
) -> Result<()> {
    details.insert("request_digest".into(), request_digest.as_str().to_string());
    file.audit_log.append(AuditEntry {
        audit_id: AuditId::from_public_key(
            format!(
                "admin-recovery-audit:{request_id}:{action}:{}",
                outcome_code(outcome)
            )
            .as_bytes(),
        ),
        operation_id: None,
        actor: actor.cloned().unwrap_or_else(|| {
            NodeId::from_public_key(format!("admin-recovery:{request_id}").as_bytes())
        }),
        network_id: Some(network_id.clone()),
        action: action.into(),
        resource: format!("network/{network_id}/admin-recovery/{request_id}"),
        outcome,
        occurred_at_unix_ms: now_unix_ms,
        summary: summary.into(),
        details,
        sensitive_keys: BTreeSet::new(),
    })?;
    Ok(())
}

fn outcome_code(outcome: AuditOutcome) -> &'static str {
    match outcome {
        AuditOutcome::Succeeded => "succeeded",
        AuditOutcome::Denied => "denied",
        AuditOutcome::Failed => "failed",
    }
}

fn lock_profile_shared(path: &Path) -> Result<File> {
    lock_profile(path, false)
}

fn lock_profile_exclusive(path: &Path) -> Result<File> {
    lock_profile(path, true)
}

fn lock_profile(path: &Path, exclusive: bool) -> Result<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "cannot create network profile directory: {}",
                parent.display()
            )
        })?;
    }
    let lock_path = path.with_file_name(".profile.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("cannot open profile lock: {}", lock_path.display()))?;
    if exclusive {
        lock.lock_exclusive()
    } else {
        lock.lock_shared()
    }
    .with_context(|| format!("cannot lock network profile: {}", lock_path.display()))?;
    Ok(lock)
}

fn persist_network_record(path: &Path, record: &NetworkRecord) -> Result<()> {
    record.validate()?;
    let bytes = serde_json::to_vec_pretty(record).context("cannot serialize network record")?;
    persist_bytes_atomic(path, &bytes)
}

fn restore_file(path: &Path, bytes: Option<&[u8]>) -> Result<()> {
    match bytes {
        Some(bytes) => persist_bytes_atomic(path, bytes),
        None => std::fs::remove_file(path)
            .with_context(|| format!("cannot remove file during rollback: {}", path.display())),
    }
}

fn persist_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .with_context(|| format!("cannot create persistence directory: {}", parent.display()))?;
    let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
    {
        let mut output = File::create(&temporary).with_context(|| {
            format!(
                "cannot create persistence temporary file: {}",
                temporary.display()
            )
        })?;
        output.write_all(bytes).with_context(|| {
            format!(
                "cannot write persistence temporary file: {}",
                temporary.display()
            )
        })?;
        output.sync_all().with_context(|| {
            format!(
                "cannot sync persistence temporary file: {}",
                temporary.display()
            )
        })?;
    }
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error)
            .with_context(|| format!("cannot replace persistence file: {}", path.display()));
    }
    let directory = File::open(parent)
        .with_context(|| format!("cannot open persistence directory: {}", parent.display()))?;
    directory
        .sync_all()
        .with_context(|| format!("cannot sync persistence directory: {}", parent.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signaling::member_lifecycle::MemberLifecycleStore;
    use fuckipv6_common::crypto::Identity;
    use fuckipv6_common::domain::{
        AdminRecoveryApproval, AdminRecoveryProposal, AdminRecoveryReason,
        AdminRecoveryRevocationPlan, AdminRecoveryTarget, CapabilityKind, ExitContributionMode,
        ExitContributionObligation, ExitContributionScope, ExitSelectionPolicy,
        InvitationCreateRequest, InvitationState, JoinApplicantProfile, JoinRequest,
        JoinRequestState, MembershipRole, NetworkCreateProfile, NetworkJoinMode, NetworkVisibility,
        PublicKey,
    };
    use fuckipv6_common::NatType;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::{Arc, Barrier};

    fn test_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "fuckipv6-admin-recovery-{}-{label}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn network(owner: &Identity, administrators: Vec<NodeId>) -> NetworkRecord {
        let owner_id = NodeId::from_public_key(&owner.public_key());
        let mut network = NetworkRecord::create_owned(
            NetworkCreateProfile {
                name: "Recovery".into(),
                purpose: "Recovery tests".into(),
                icon: "network".into(),
                language: "zh-CN".into(),
                region: "ZZ".into(),
                tags: vec![],
                capability_declarations: vec![CapabilityKind::File],
                visibility: NetworkVisibility::Private,
                join_mode: NetworkJoinMode::ApprovalRequired,
                exit_contribution: ExitContributionObligation::default(),
                exit_contribution_scope: ExitContributionScope::default(),
                exit_contribution_mode: ExitContributionMode::default(),
                exit_selection_policy: ExitSelectionPolicy::default(),
            },
            PublicKey::from_bytes([77; 32]),
            owner_id,
            1_000,
        )
        .unwrap();
        network.members.extend(administrators.clone());
        network.administrators = administrators;
        network
    }

    fn request(
        network: &NetworkRecord,
        signers: &[&Identity],
        replacement: NodeId,
    ) -> AdminRecoveryRequest {
        request_with_nonce(network, signers, replacement, "disaster-1")
    }

    fn request_with_nonce(
        network: &NetworkRecord,
        signers: &[&Identity],
        replacement: NodeId,
        nonce: &str,
    ) -> AdminRecoveryRequest {
        let target = AdminRecoveryTarget::replace_authorities(network, replacement, Vec::new());
        let proposal = AdminRecoveryProposal::new(
            network,
            target,
            NodeId::from_public_key(b"offline-operator"),
            AdminRecoveryReason::DisasterRecovery,
            nonce,
            1_100,
            1_200,
            AdminRecoveryRevocationPlan::default(),
        )
        .unwrap();
        let approvals = signers
            .iter()
            .enumerate()
            .map(|(index, signer)| {
                AdminRecoveryApproval::signed(&proposal, signer, 1_110 + index as u64).unwrap()
            })
            .collect();
        AdminRecoveryRequest::with_approvals(proposal, approvals)
    }

    fn setup(
        label: &str,
    ) -> (
        PathBuf,
        NetworkRecord,
        Identity,
        Identity,
        AdminRecoveryStore,
    ) {
        let dir = test_dir(label);
        let owner = Identity::generate().unwrap();
        let admin = Identity::generate().unwrap();
        let admin_id = NodeId::from_public_key(&admin.public_key());
        let network = network(&owner, vec![admin_id]);
        let network_path = dir.join("network.json");
        persist_network_record(&network_path, &network).unwrap();
        let store =
            AdminRecoveryStore::new(dir.join("admin_recovery.json"), &network_path).unwrap();
        (dir, network, owner, admin, store)
    }

    struct RecoveryServiceFixture {
        dir: PathBuf,
        network: NetworkRecord,
        owner: Identity,
        admin: Identity,
        store: Arc<AdminRecoveryStore>,
        invitation_store: Arc<InvitationStore>,
        join_request_store: Arc<JoinRequestStore>,
        session_manager: Arc<SessionManager>,
        service: Arc<AdminRecoveryService>,
        owner_client: String,
        admin_client: String,
        outsider_client: String,
    }

    fn join_request(
        network: &NetworkRecord,
        applicant: &Identity,
        nonce: &str,
        now_unix_ms: u64,
    ) -> JoinRequest {
        let applicant_id = NodeId::from_public_key(&applicant.public_key());
        JoinRequest::new(
            JoinRequest::derived_id(&network.network_id, &applicant_id, nonce),
            network.network_id.clone(),
            JoinApplicantProfile {
                node_id: applicant_id,
                display_name: Some("Recovery applicant".into()),
                public_key_fingerprint: Identity::public_key_fingerprint(&applicant.public_key()),
            },
            "Please allow this node to join for recovery testing.",
            network.membership_terms(),
            network.approval_grant_preview(),
            now_unix_ms,
            now_unix_ms + 60_000,
        )
        .unwrap()
    }

    async fn recovery_service_fixture(label: &str) -> RecoveryServiceFixture {
        let dir = test_dir(label);
        let owner = Identity::generate().unwrap();
        let admin = Identity::generate().unwrap();
        let outsider = Identity::generate().unwrap();
        let first_applicant = Identity::generate().unwrap();
        let second_applicant = Identity::generate().unwrap();
        let admin_id = NodeId::from_public_key(&admin.public_key());
        let mut network = network(&owner, vec![admin_id]);
        let network_path = dir.join("network.json");

        network.join_mode = NetworkJoinMode::InviteOnly;
        persist_network_record(&network_path, &network).unwrap();
        let invitation_store = Arc::new(
            InvitationStore::new(
                dir.join("invitations.json"),
                Some(network_path.clone()),
                None,
                vec![],
            )
            .unwrap(),
        );
        for index in 0..2 {
            invitation_store
                .create(
                    &network.network_id,
                    &NodeId::from_public_key(&owner.public_key()),
                    InvitationCreateRequest::new(
                        network.network_id.clone(),
                        MembershipRole::Member,
                        vec![CapabilityKind::File],
                        1,
                        60_000,
                    ),
                    1_020 + index,
                )
                .unwrap();
        }

        network.join_mode = NetworkJoinMode::ApprovalRequired;
        persist_network_record(&network_path, &network).unwrap();
        let lifecycle = Arc::new(
            MemberLifecycleStore::new(dir.join("member_lifecycle.json"), network_path.clone())
                .unwrap(),
        );
        let join_request_store = Arc::new(
            JoinRequestStore::new(
                dir.join("join_requests.json"),
                Some(network_path.clone()),
                None,
                vec![],
            )
            .unwrap()
            .with_member_lifecycle_store(lifecycle),
        );
        for (index, applicant) in [&first_applicant, &second_applicant].iter().enumerate() {
            let request = join_request(&network, applicant, &format!("recovery-{index}"), 1_030);
            let applicant_id = NodeId::from_public_key(&applicant.public_key());
            join_request_store
                .submit(
                    &network.network_id,
                    &applicant_id,
                    &applicant.public_key(),
                    request,
                    1_031 + index as u64,
                )
                .unwrap();
        }

        let session_manager = Arc::new(SessionManager::new("fd00::1000/120").unwrap());
        let owner_client = owner.client_id().to_string();
        let admin_client = admin.client_id().to_string();
        let outsider_client = outsider.client_id().to_string();
        for (index, (identity, client_id)) in [
            (&owner, owner_client.clone()),
            (&admin, admin_client.clone()),
            (&outsider, outsider_client.clone()),
        ]
        .into_iter()
        .enumerate()
        {
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            session_manager
                .register_client(
                    client_id,
                    identity.public_key(),
                    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 43_100 + index as u16),
                    53_000 + index as u16,
                    NatType::Open,
                    tx,
                )
                .await
                .unwrap();
        }

        let store = Arc::new(
            AdminRecoveryStore::new(dir.join("admin_recovery.json"), &network_path).unwrap(),
        );
        let service = Arc::new(AdminRecoveryService::new(
            store.clone(),
            invitation_store.clone(),
            join_request_store.clone(),
            session_manager.clone(),
        ));
        RecoveryServiceFixture {
            dir,
            network,
            owner,
            admin,
            store,
            invitation_store,
            join_request_store,
            session_manager,
            service,
            owner_client,
            admin_client,
            outsider_client,
        }
    }

    #[test]
    fn execution_is_atomic_single_use_and_audited() {
        let (_dir, network, owner, admin, store) = setup("execute");
        let replacement = NodeId::from_public_key(b"replacement");
        let request = request(&network, &[&owner, &admin], replacement.clone());
        let request_id = request.proposal.request_id.clone();
        let view = store.submit(request.clone(), 1_120).unwrap();
        assert!(view.can_execute);
        assert_eq!(view.signature_count, 2);
        let execution = store.execute(&request_id, 1_130).unwrap();
        assert_eq!(execution.new_owner, replacement);
        assert_eq!(execution.invalidated_sessions.len(), 2);
        assert_eq!(store.execute(&request_id, 1_131).unwrap(), execution);
        let replayed = store.submit(request, 1_132).unwrap();
        assert_eq!(replayed.state, AdminRecoveryState::Executed);
        assert_eq!(
            replayed.denial_key.as_deref(),
            Some("admin_recovery.error.already_executed")
        );
        assert!(store
            .audit_entries()
            .unwrap()
            .iter()
            .any(|entry| entry.action == "admin_recovery.executed"));
    }

    #[test]
    fn roster_change_after_submission_blocks_execution() {
        let (dir, mut network, owner, admin, store) = setup("stale");
        let replacement = NodeId::from_public_key(b"replacement");
        let request = request(&network, &[&owner, &admin], replacement);
        let request_id = request.proposal.request_id.clone();
        store.submit(request, 1_120).unwrap();
        network
            .members
            .push(NodeId::from_public_key(b"late-member"));
        persist_network_record(&dir.join("network.json"), &network).unwrap();
        assert!(store
            .execute(&request_id, 1_130)
            .unwrap_err()
            .to_string()
            .starts_with("domain.admin_recovery_stale_snapshot"));
        let persisted: NetworkRecord =
            serde_json::from_str(&std::fs::read_to_string(dir.join("network.json")).unwrap())
                .unwrap();
        assert_eq!(
            persisted.owner,
            NodeId::from_public_key(&owner.public_key())
        );
    }

    #[test]
    fn concurrent_execution_has_exactly_one_durable_winner() {
        let (_dir, network, owner, admin, store) = setup("concurrent");
        let replacement = NodeId::from_public_key(b"replacement");
        let request = request(&network, &[&owner, &admin], replacement);
        let request_id = request.proposal.request_id.clone();
        store.submit(request, 1_120).unwrap();
        let store = Arc::new(store);
        let barrier = Arc::new(Barrier::new(2));
        let handles = (0..2)
            .map(|_| {
                let store = store.clone();
                let barrier = barrier.clone();
                let request_id = request_id.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    store.execute(&request_id, 1_130)
                })
            })
            .collect::<Vec<_>>();
        let executions = handles
            .into_iter()
            .map(|handle| handle.join().unwrap().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(executions.len(), 2);
        assert_eq!(executions[0], executions[1]);
        assert_eq!(
            store
                .audit_entries()
                .unwrap()
                .iter()
                .filter(|entry| entry.action == "admin_recovery.executed")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn service_invalidates_old_credentials_atomically_and_idempotently() {
        let fixture = recovery_service_fixture("service-complete").await;
        let replacement = NodeId::from_public_key(b"replacement-service-owner");
        let request = request(
            &fixture.network,
            &[&fixture.owner, &fixture.admin],
            replacement.clone(),
        );
        let request_id = request.proposal.request_id.clone();
        fixture.store.submit(request, 1_120).unwrap();

        let completed = fixture.service.execute(&request_id, 1_130).await.unwrap();
        assert_eq!(completed.execution.new_owner, replacement);
        assert_eq!(completed.disconnected_sessions, 2);
        assert_eq!(completed.revoked_invitations, 2);
        assert_eq!(completed.cancelled_join_requests, 2);
        assert!(fixture
            .session_manager
            .get_peer_info(&fixture.owner_client)
            .await
            .is_none());
        assert!(fixture
            .session_manager
            .get_peer_info(&fixture.admin_client)
            .await
            .is_none());
        assert!(fixture
            .session_manager
            .get_peer_info(&fixture.outsider_client)
            .await
            .is_some());

        let invitations = fixture
            .invitation_store
            .list(&fixture.network.network_id, &replacement, 1_131)
            .unwrap();
        assert_eq!(invitations.len(), 2);
        assert!(invitations
            .iter()
            .all(|invitation| invitation.state == InvitationState::Revoked));
        let joins = fixture
            .join_request_store
            .list(&fixture.network.network_id, &replacement, 1_131)
            .unwrap();
        assert_eq!(joins.len(), 2);
        assert!(joins
            .iter()
            .all(|request| request.state == JoinRequestState::Rejected));
        assert_eq!(
            fixture
                .store
                .audit_entries()
                .unwrap()
                .iter()
                .filter(|entry| entry.action == "admin_recovery.executed")
                .count(),
            1
        );
        assert_eq!(
            fixture
                .store
                .audit_entries()
                .unwrap()
                .iter()
                .filter(|entry| entry.action == "admin_recovery.invalidations_applied")
                .count(),
            1
        );

        let retried = fixture.service.execute(&request_id, 1_132).await.unwrap();
        assert_eq!(retried.execution, completed.execution);
        assert_eq!(retried.disconnected_sessions, 0);
        assert_eq!(retried.revoked_invitations, 0);
        assert_eq!(retried.cancelled_join_requests, 0);
        assert_eq!(
            fixture
                .store
                .audit_entries()
                .unwrap()
                .iter()
                .filter(|entry| entry.action == "admin_recovery.invalidations_applied")
                .count(),
            1,
            "retry must not duplicate completion audit"
        );
        fixture
            .session_manager
            .unregister_client(&fixture.outsider_client)
            .await;
    }

    #[tokio::test]
    async fn service_resumes_after_partial_invalidation_failure() {
        let fixture = recovery_service_fixture("service-resume").await;
        let replacement = NodeId::from_public_key(b"replacement-service-resume");
        let request = request(
            &fixture.network,
            &[&fixture.owner, &fixture.admin],
            replacement.clone(),
        );
        let request_id = request.proposal.request_id.clone();
        fixture.store.submit(request, 1_120).unwrap();
        let invitation_path = fixture.dir.join("invitations.json");
        let invitation_bytes = std::fs::read(&invitation_path).unwrap();
        std::fs::write(&invitation_path, b"not valid json").unwrap();

        let error = fixture
            .service
            .execute(&request_id, 1_130)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("invitations.json"),
            "unexpected partial failure: {error:#}"
        );
        let partial_network: NetworkRecord = serde_json::from_str(
            &std::fs::read_to_string(fixture.dir.join("network.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(partial_network.owner, replacement);
        assert!(fixture
            .store
            .audit_entries()
            .unwrap()
            .iter()
            .all(|entry| entry.action != "admin_recovery.invalidations_applied"));

        std::fs::write(&invitation_path, invitation_bytes).unwrap();
        let completed = fixture.service.execute(&request_id, 1_140).await.unwrap();
        assert_eq!(completed.revoked_invitations, 2);
        assert_eq!(completed.cancelled_join_requests, 2);
        assert_eq!(completed.disconnected_sessions, 2);
        assert_eq!(
            fixture
                .store
                .audit_entries()
                .unwrap()
                .iter()
                .filter(|entry| entry.action == "admin_recovery.executed")
                .count(),
            1
        );
        assert_eq!(
            fixture
                .store
                .audit_entries()
                .unwrap()
                .iter()
                .filter(|entry| entry.action == "admin_recovery.invalidations_applied")
                .count(),
            1
        );

        let retried = fixture.service.execute(&request_id, 1_141).await.unwrap();
        assert_eq!(retried.revoked_invitations, 0);
        assert_eq!(retried.cancelled_join_requests, 0);
        assert_eq!(retried.disconnected_sessions, 0);
        fixture
            .session_manager
            .unregister_client(&fixture.outsider_client)
            .await;
    }

    #[test]
    fn authenticated_non_member_cannot_submit_a_recovery_request() {
        let (_dir, network, owner, admin, store) = setup("submit-member-only");
        let outsider = Identity::generate().unwrap();
        let outsider_id = NodeId::from_public_key(&outsider.public_key());
        let request = request(
            &network,
            &[&owner, &admin],
            NodeId::from_public_key(b"replacement"),
        );
        let error = store.submit_as(&outsider_id, request, 1_120).unwrap_err();
        assert_eq!(error.to_string(), "admin_recovery.error.permission_denied");
        assert!(store.list(&network.network_id, 1_120).unwrap().is_empty());
    }

    #[test]
    fn only_recovery_participants_or_current_authorities_can_read_status() {
        let (_dir, network, owner, admin, store) = setup("status-visibility");
        let outsider = Identity::generate().unwrap();
        let outsider_id = NodeId::from_public_key(&outsider.public_key());
        let owner_id = NodeId::from_public_key(&owner.public_key());
        let request = request(
            &network,
            &[&owner, &admin],
            NodeId::from_public_key(b"replacement"),
        );
        let request_id = request.proposal.request_id.clone();
        store.submit_as(&owner_id, request, 1_120).unwrap();

        let visible = store
            .status_for_actor(&request_id, &owner_id, 1_121)
            .unwrap();
        assert_eq!(visible.request_id, request_id);
        assert_eq!(
            store
                .status_for_actor(&request_id, &outsider_id, 1_121)
                .unwrap_err()
                .to_string(),
            "admin_recovery.error.permission_denied"
        );
        assert!(store
            .list_for_actor(&network.network_id, &outsider_id, 1_121)
            .unwrap()
            .is_empty());
        assert_eq!(
            store
                .list_for_actor(&network.network_id, &owner_id, 1_121)
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn non_authority_cannot_execute_or_revoke_a_recovery_request() {
        let fixture = recovery_service_fixture("authority-only").await;
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let outsider_id = NodeId::from_public_key(&Identity::generate().unwrap().public_key());
        let request = request(
            &fixture.network,
            &[&fixture.owner, &fixture.admin],
            NodeId::from_public_key(b"replacement"),
        );
        let request_id = request.proposal.request_id.clone();
        fixture.store.submit_as(&owner_id, request, 1_120).unwrap();

        assert_eq!(
            fixture
                .service
                .execute_authorized(&outsider_id, &request_id, 1_125)
                .await
                .unwrap_err()
                .to_string(),
            "admin_recovery.error.permission_denied"
        );
        assert_eq!(
            fixture
                .store
                .revoke(&request_id, &outsider_id, "not authorized", 1_126)
                .unwrap_err()
                .to_string(),
            "admin_recovery.error.permission_denied"
        );
        let status = fixture.store.status(&request_id, 1_127).unwrap();
        assert_eq!(status.state, AdminRecoveryState::Pending);

        fixture
            .session_manager
            .unregister_client(&fixture.owner_client)
            .await;
        fixture
            .session_manager
            .unregister_client(&fixture.admin_client)
            .await;
        fixture
            .session_manager
            .unregister_client(&fixture.outsider_client)
            .await;
    }

    #[tokio::test]
    async fn authority_replacement_revokes_old_owner_execution_permission() {
        let fixture = recovery_service_fixture("authority-replacement").await;
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let first = request_with_nonce(
            &fixture.network,
            &[&fixture.owner, &fixture.admin],
            NodeId::from_public_key(b"first-replacement"),
            "disaster-first",
        );
        let second = request_with_nonce(
            &fixture.network,
            &[&fixture.owner, &fixture.admin],
            NodeId::from_public_key(b"second-replacement"),
            "disaster-second",
        );
        let first_id = first.proposal.request_id.clone();
        let second_id = second.proposal.request_id.clone();
        fixture.store.submit_as(&owner_id, first, 1_120).unwrap();
        fixture.store.submit_as(&owner_id, second, 1_121).unwrap();

        fixture
            .service
            .execute_authorized(&owner_id, &first_id, 1_130)
            .await
            .unwrap();
        let error = fixture
            .service
            .execute_authorized(&owner_id, &second_id, 1_131)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "admin_recovery.error.permission_denied");
        assert_eq!(
            fixture.store.status(&second_id, 1_132).unwrap().state,
            AdminRecoveryState::Pending
        );

        fixture
            .session_manager
            .unregister_client(&fixture.owner_client)
            .await;
        fixture
            .session_manager
            .unregister_client(&fixture.admin_client)
            .await;
        fixture
            .session_manager
            .unregister_client(&fixture.outsider_client)
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn queued_old_authority_is_rechecked_after_authority_replacement() {
        let fixture = recovery_service_fixture("queued-authority-replacement").await;
        let owner_id = NodeId::from_public_key(&fixture.owner.public_key());
        let first = request_with_nonce(
            &fixture.network,
            &[&fixture.owner, &fixture.admin],
            NodeId::from_public_key(b"queued-first-replacement"),
            "queued-disaster-first",
        );
        let second = request_with_nonce(
            &fixture.network,
            &[&fixture.owner, &fixture.admin],
            NodeId::from_public_key(b"queued-second-replacement"),
            "queued-disaster-second",
        );
        let first_id = first.proposal.request_id.clone();
        let second_id = second.proposal.request_id.clone();
        fixture.store.submit_as(&owner_id, first, 1_120).unwrap();
        fixture.store.submit_as(&owner_id, second, 1_121).unwrap();

        // Hold the service execution lock so the old owner's authorization
        // attempt queues before the first recovery changes the roster.
        let execution_guard = fixture.service.execution_lock.lock().await;
        let service = fixture.service.clone();
        let queued_owner = owner_id.clone();
        let queued_request_id = second_id.clone();
        let queued = tokio::spawn(async move {
            service
                .execute_authorized(&queued_owner, &queued_request_id, 1_130)
                .await
        });
        tokio::task::yield_now().await;

        fixture.store.execute(&first_id, 1_130).unwrap();
        drop(execution_guard);

        let error = queued.await.unwrap().unwrap_err();
        assert_eq!(error.to_string(), "admin_recovery.error.permission_denied");
        assert_eq!(
            fixture.store.status(&second_id, 1_131).unwrap().state,
            AdminRecoveryState::Pending
        );

        fixture
            .session_manager
            .unregister_client(&fixture.owner_client)
            .await;
        fixture
            .session_manager
            .unregister_client(&fixture.admin_client)
            .await;
        fixture
            .session_manager
            .unregister_client(&fixture.outsider_client)
            .await;
    }

    #[test]
    fn current_administrator_can_revoke_a_pending_request() {
        let (_dir, network, owner, admin, store) = setup("revoke");
        let request = request(
            &network,
            &[&owner, &admin],
            NodeId::from_public_key(b"replacement"),
        );
        let request_id = request.proposal.request_id.clone();
        store.submit(request, 1_120).unwrap();
        let admin_id = NodeId::from_public_key(&admin.public_key());
        let view = store
            .revoke(&request_id, &admin_id, "ceremony aborted", 1_125)
            .unwrap();
        assert_eq!(view.state, AdminRecoveryState::Revoked);
        assert_eq!(
            store.execute(&request_id, 1_130).unwrap_err().to_string(),
            "admin_recovery.error.revoked"
        );
    }
}
