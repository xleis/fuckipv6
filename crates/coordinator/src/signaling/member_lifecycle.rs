use anyhow::{bail, Context, Result};
use fs2::FileExt;
use fuckipv6_common::domain::{
    authorize_member_lifecycle_action, member_lifecycle_target, AuditEntry, AuditId, AuditLog,
    AuditOutcome, CapabilityKind, ContentHash, IdempotencyKey, MemberLifecycleAction,
    MemberLifecycleActorRole, MemberLifecycleChange, MemberLifecycleChangeRequest,
    MemberLifecycleRecord, MemberLifecycleStatus, MemberRemovalPreview,
    MemberRemovalPreviewRequest, MembershipRole, NetworkId, NetworkMemberPresence, NetworkRecord,
    NodeId, OperationId,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const MEMBER_LIFECYCLE_FILE_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredLifecycleResult {
    request_digest: ContentHash,
    change: MemberLifecycleChange,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemberLifecycleFile {
    schema_version: u16,
    #[serde(default)]
    records: BTreeMap<NetworkId, BTreeMap<NodeId, MemberLifecycleRecord>>,
    #[serde(default)]
    idempotency: BTreeMap<NetworkId, BTreeMap<IdempotencyKey, StoredLifecycleResult>>,
    #[serde(default)]
    audit_log: AuditLog,
}

impl Default for MemberLifecycleFile {
    fn default() -> Self {
        Self {
            schema_version: MEMBER_LIFECYCLE_FILE_SCHEMA_VERSION,
            records: BTreeMap::new(),
            idempotency: BTreeMap::new(),
            audit_log: AuditLog::default(),
        }
    }
}

impl MemberLifecycleFile {
    fn validate(&self) -> Result<()> {
        if self.schema_version != MEMBER_LIFECYCLE_FILE_SCHEMA_VERSION {
            bail!("unsupported member lifecycle file schema version");
        }
        for (network_id, records) in &self.records {
            for (node_id, record) in records {
                if record.network_id != *network_id || record.node_id != *node_id {
                    bail!("member lifecycle record identity is inconsistent");
                }
                record.validate()?;
            }
        }
        for (network_id, results) in &self.idempotency {
            for stored in results.values() {
                if stored.change.network_id != *network_id
                    || stored.change.revision == 0
                    || stored.change.access_epoch == 0
                {
                    bail!("member lifecycle idempotency result is inconsistent");
                }
            }
        }
        Ok(())
    }
}

/// Durable governance ledger shared by approval, invitation and command
/// adapters. Every mutation uses the same transition table, actor authorization,
/// CAS revision, idempotency digest and append-only audit projection.
pub struct MemberLifecycleStore {
    path: PathBuf,
    network_record_path: PathBuf,
    process_lock: Mutex<()>,
}

impl MemberLifecycleStore {
    pub fn new(path: impl Into<PathBuf>, network_record_path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let network_record_path = network_record_path.into();
        if path == network_record_path {
            bail!("member lifecycle and network record paths must differ");
        }
        Ok(Self {
            path,
            network_record_path,
            process_lock: Mutex::new(()),
        })
    }

    pub fn change(
        &self,
        actor: &NodeId,
        request: MemberLifecycleChangeRequest,
        now_unix_ms: u64,
    ) -> Result<MemberLifecycleChange> {
        request.validate()?;
        if now_unix_ms == 0 {
            bail!("member lifecycle change time must be positive");
        }
        let request_digest = ContentHash::from_bytes(
            &serde_json::to_vec(&request).context("无法序列化成员生命周期请求摘要")?,
        );
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("成员生命周期存储锁已损坏"))?;
        let _profile_lock = lock_profile(&self.network_record_path, true)?;
        self.ensure_parent()?;
        let _file_lock = lock_lifecycle_file(&self.path)?;
        let mut file = self.load_unlocked()?;

        if let Some(stored) = file
            .idempotency
            .get(&request.network_id)
            .and_then(|entries| entries.get(&request.idempotency_key))
        {
            if stored.request_digest != request_digest {
                bail!("member_lifecycle.error.idempotency_conflict");
            }
            let mut change = stored.change.clone();
            change.idempotent = true;
            return Ok(change);
        }

        let previous_lifecycle_bytes = std::fs::read(&self.path).ok();
        let previous_network_bytes = std::fs::read(&self.network_record_path).ok();
        let network = read_network_record(&self.network_record_path, &request.network_id)?;
        let evaluation = evaluate_change(&file, &network, actor, &request, now_unix_ms);

        let (change, record, next_network) = match evaluation {
            Ok(value) => value,
            Err(error) => {
                append_audit(
                    &mut file,
                    actor,
                    &request,
                    &request_digest,
                    AuditOutcome::Denied,
                    None,
                    now_unix_ms,
                )?;
                file.validate()?;
                self.persist(&file)?;
                return Err(error);
            }
        };

        file.records
            .entry(request.network_id.clone())
            .or_default()
            .insert(request.target_node_id.clone(), record);
        file.idempotency
            .entry(request.network_id.clone())
            .or_default()
            .insert(
                request.idempotency_key.clone(),
                StoredLifecycleResult {
                    request_digest: request_digest.clone(),
                    change: change.clone(),
                },
            );
        append_audit(
            &mut file,
            actor,
            &request,
            &request_digest,
            AuditOutcome::Succeeded,
            Some(&change),
            now_unix_ms,
        )?;
        file.validate()?;

        if let Err(error) = persist_network_record(&self.network_record_path, &next_network) {
            restore_optional_file(&self.network_record_path, previous_network_bytes.as_deref())?;
            return Err(error);
        }
        if let Err(error) = self.persist(&file) {
            restore_optional_file(&self.network_record_path, previous_network_bytes.as_deref())?;
            restore_optional_file(&self.path, previous_lifecycle_bytes.as_deref())?;
            return Err(error);
        }
        Ok(change)
    }

    pub fn removal_preview(
        &self,
        actor: &NodeId,
        request: MemberRemovalPreviewRequest,
        presence: NetworkMemberPresence,
        now_unix_ms: u64,
    ) -> Result<MemberRemovalPreview> {
        request.validate()?;
        let _profile_lock = lock_profile(&self.network_record_path, false)?;
        self.ensure_parent()?;
        let _file_lock = lock_lifecycle_file(&self.path)?;
        let file = self.load_unlocked()?;
        let network = read_network_record(&self.network_record_path, &request.network_id)?;
        if !network.members.contains(actor)
            && network.owner != *actor
            && !network.administrators.contains(actor)
        {
            bail!("member_lifecycle.error.permission_denied");
        }
        let current = lifecycle_for_target(&file, &network, &request.target_node_id);
        let target_role = role_for_target(&network, current.as_ref(), &request.target_node_id);
        let actor_role = role_for_actor(&network, actor);
        let retained_administrators = retained_administrators(&network, &request.target_node_id);
        let authorization = authorize_member_lifecycle_action(
            actor,
            actor_role,
            &request.target_node_id,
            target_role,
            MemberLifecycleAction::Remove,
            retained_administrators,
        );
        let lifecycle_state = current
            .as_ref()
            .map(|record| record.status)
            .unwrap_or_else(|| role_status(&network, &request.target_node_id));
        let revision = current.as_ref().map(|record| record.revision).unwrap_or(1);
        let access_epoch = current
            .as_ref()
            .map(|record| record.access_epoch)
            .unwrap_or(1);
        let status = lifecycle_state.to_member_status(presence.clone());
        let capabilities = if network.members.contains(&request.target_node_id) {
            network.capability_declarations.clone()
        } else {
            Vec::new()
        };
        let file_access_revoked = network.members.contains(&request.target_node_id)
            && capabilities.contains(&CapabilityKind::File);
        let preview = MemberRemovalPreview {
            network_id: request.network_id,
            target_node_id: request.target_node_id,
            display_name: None,
            role: target_role,
            status,
            lifecycle_state,
            revision,
            access_epoch,
            presence,
            active_capabilities: capabilities,
            can_remove: authorization.is_ok(),
            denial_key: authorization.err().map(|error| error.code().to_string()),
            network_access_revoked: true,
            file_access_revoked,
            incomplete_transfers_resume_blocked: true,
            credentials_revoked: true,
            old_access_epoch_invalidated: true,
            local_files_deleted: false,
            safe_undo_requires_reapproval: true,
            safe_undo_restores_old_credentials: false,
            impact_keys: vec![
                "member_removal.impact.network_access".into(),
                "member_removal.impact.file_access".into(),
                "member_removal.impact.incomplete_transfers".into(),
                "member_removal.impact.credentials".into(),
                "member_removal.impact.local_files_kept".into(),
                "member_removal.impact.safe_undo".into(),
            ],
        };
        preview.validate()?;
        let _ = now_unix_ms;
        Ok(preview)
    }

    pub fn is_tombstoned(&self, network_id: &NetworkId, node_id: &NodeId) -> Result<bool> {
        self.ensure_parent()?;
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("成员生命周期存储锁已损坏"))?;
        let _file_lock = lock_lifecycle_file(&self.path)?;
        let file = self.load_unlocked()?;
        Ok(file
            .records
            .get(network_id)
            .and_then(|records| records.get(node_id))
            .is_some_and(|record| record.status.is_tombstone()))
    }

    pub fn record(
        &self,
        network_id: &NetworkId,
        node_id: &NodeId,
    ) -> Result<Option<MemberLifecycleRecord>> {
        self.ensure_parent()?;
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("成员生命周期存储锁已损坏"))?;
        let _file_lock = lock_lifecycle_file(&self.path)?;
        let file = self.load_unlocked()?;
        Ok(file
            .records
            .get(network_id)
            .and_then(|records| records.get(node_id))
            .cloned())
    }

    pub fn records(&self, network_id: &NetworkId) -> Result<Vec<MemberLifecycleRecord>> {
        self.ensure_parent()?;
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("成员生命周期存储锁已损坏"))?;
        let _file_lock = lock_lifecycle_file(&self.path)?;
        let file = self.load_unlocked()?;
        Ok(file
            .records
            .get(network_id)
            .map(|records| records.values().cloned().collect())
            .unwrap_or_default())
    }

    pub fn audit_entries(&self) -> Result<Vec<fuckipv6_common::domain::AuditEntry>> {
        self.ensure_parent()?;
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("成员生命周期存储锁已损坏"))?;
        let _file_lock = lock_lifecycle_file(&self.path)?;
        Ok(self.load_unlocked()?.audit_log.entries().to_vec())
    }

    fn ensure_parent(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("无法创建成员生命周期目录: {}", parent.display()))?;
        }
        Ok(())
    }

    fn load_unlocked(&self) -> Result<MemberLifecycleFile> {
        if !self.path.exists() {
            return Ok(MemberLifecycleFile::default());
        }
        let content = std::fs::read_to_string(&self.path)
            .with_context(|| format!("无法读取成员生命周期存储: {}", self.path.display()))?;
        let file: MemberLifecycleFile = serde_json::from_str(&content)
            .with_context(|| format!("无法解析成员生命周期存储: {}", self.path.display()))?;
        file.validate()?;
        Ok(file)
    }

    fn persist(&self, file: &MemberLifecycleFile) -> Result<()> {
        file.validate()?;
        let bytes = serde_json::to_vec_pretty(file).context("无法序列化成员生命周期存储")?;
        persist_bytes_atomic(&self.path, &bytes)
    }
}

fn evaluate_change(
    file: &MemberLifecycleFile,
    network: &NetworkRecord,
    actor: &NodeId,
    request: &MemberLifecycleChangeRequest,
    now_unix_ms: u64,
) -> Result<(MemberLifecycleChange, MemberLifecycleRecord, NetworkRecord)> {
    let current = lifecycle_for_target(file, network, &request.target_node_id);
    let current_revision = current.as_ref().map(|record| record.revision);
    if current_revision.unwrap_or(0) != request.expected_revision {
        bail!("member_lifecycle.error.stale_revision");
    }
    if current.is_none() && request.action != MemberLifecycleAction::Invite {
        bail!("member_lifecycle.error.invalid_transition");
    }
    if !network.members.contains(actor)
        && network.owner != *actor
        && !network.administrators.contains(actor)
    {
        bail!("member_lifecycle.error.permission_denied");
    }
    let target_role = if request.action == MemberLifecycleAction::Invite {
        request.role
    } else {
        role_for_target(network, current.as_ref(), &request.target_node_id)
    };
    let actor_role = role_for_actor(network, actor);
    let retained = retained_administrators(network, &request.target_node_id);
    authorize_member_lifecycle_action(
        actor,
        actor_role,
        &request.target_node_id,
        target_role,
        request.action,
        retained,
    )
    .map_err(map_lifecycle_domain_error)?;

    let target_status =
        member_lifecycle_target(current.as_ref().map(|record| record.status), request.action)
            .map_err(|_| anyhow::anyhow!("member_lifecycle.error.invalid_transition"))?;
    let access_epoch = current
        .as_ref()
        .map(|record| record.access_epoch)
        .unwrap_or(0)
        .checked_add(1)
        .context("member lifecycle access epoch overflow")?;
    let revision = current
        .as_ref()
        .map(|record| record.revision)
        .unwrap_or(0)
        .checked_add(1)
        .context("member lifecycle revision overflow")?;
    let operation_seed = format!(
        "member-lifecycle:{}:{}:{}",
        request.network_id,
        request.idempotency_key,
        request.action.audit_action()
    );
    let operation_id = OperationId::from_public_key(operation_seed.as_bytes());
    let audit_id = AuditId::from_public_key(
        format!("{operation_seed}:audit:{}", request.target_node_id).as_bytes(),
    );
    let mut record = current.unwrap_or_else(|| {
        MemberLifecycleRecord::new(request.network_id.clone(), request.target_node_id.clone())
    });
    record.role = target_role;
    record.status = target_status;
    record.revision = revision;
    record.access_epoch = access_epoch;
    record.last_operation_id = Some(operation_id.clone());
    record.last_idempotency_key = Some(request.idempotency_key.clone());
    record.updated_at_unix_ms = Some(now_unix_ms);

    let mut next_network = network.clone();
    apply_roster_state(&mut next_network, &record, now_unix_ms)?;

    let requires_reauthentication = target_status.requires_fresh_admission();
    let access_revoked = matches!(
        request.action,
        MemberLifecycleAction::Reject
            | MemberLifecycleAction::Suspend
            | MemberLifecycleAction::Exit
            | MemberLifecycleAction::Remove
    );
    Ok((
        MemberLifecycleChange {
            network_id: request.network_id.clone(),
            target_node_id: request.target_node_id.clone(),
            action: request.action,
            status: target_status,
            revision,
            access_epoch,
            operation_id,
            audit_id,
            idempotent: false,
            access_revoked,
            requires_reauthentication,
            requires_reapproval: matches!(
                target_status,
                MemberLifecycleStatus::Invited
                    | MemberLifecycleStatus::PendingApproval
                    | MemberLifecycleStatus::Rejected
            ),
        },
        record,
        next_network,
    ))
}

fn map_lifecycle_domain_error(error: fuckipv6_common::domain::DomainError) -> anyhow::Error {
    match error.code() {
        "domain.member_lifecycle_forbidden" => {
            anyhow::anyhow!("member_lifecycle.error.permission_denied")
        }
        "domain.member_owner_protected" => {
            anyhow::anyhow!("member_lifecycle.error.owner_protected")
        }
        "domain.member_last_administrator_protected" => {
            anyhow::anyhow!("member_lifecycle.error.last_administrator_protected")
        }
        "domain.invalid_member_lifecycle_transition" => {
            anyhow::anyhow!("member_lifecycle.error.invalid_transition")
        }
        _ => anyhow::anyhow!(error.to_string()),
    }
}

fn apply_roster_state(
    network: &mut NetworkRecord,
    record: &MemberLifecycleRecord,
    now_unix_ms: u64,
) -> Result<()> {
    if network.owner == record.node_id {
        if !record.status.grants_network_access() {
            bail!("member_lifecycle.error.owner_protected");
        }
        return Ok(());
    }
    if record.status.grants_network_access() {
        network.admit_member(record.node_id.clone(), now_unix_ms)?;
        if record.role == MembershipRole::Administrator
            && !network.administrators.contains(&record.node_id)
        {
            network.administrators.push(record.node_id.clone());
        }
        if record.role == MembershipRole::Member {
            network
                .administrators
                .retain(|administrator| administrator != &record.node_id);
        }
    } else {
        network
            .administrators
            .retain(|administrator| administrator != &record.node_id);
        network.members.retain(|member| member != &record.node_id);
    }
    if network.updated_at_unix_ms < now_unix_ms {
        network.updated_at_unix_ms = now_unix_ms;
    }
    network.validate()?;
    Ok(())
}

fn lifecycle_for_target(
    file: &MemberLifecycleFile,
    network: &NetworkRecord,
    target: &NodeId,
) -> Option<MemberLifecycleRecord> {
    file.records
        .get(&network.network_id)
        .and_then(|records| records.get(target))
        .cloned()
        .or_else(|| {
            network.members.contains(target).then(|| {
                let mut record =
                    MemberLifecycleRecord::new(network.network_id.clone(), target.clone());
                record.role = role_for_target(network, None, target);
                record.status = role_status(network, target);
                record
            })
        })
}

fn role_status(network: &NetworkRecord, target: &NodeId) -> MemberLifecycleStatus {
    if network.members.contains(target) {
        MemberLifecycleStatus::Approved
    } else {
        MemberLifecycleStatus::PendingApproval
    }
}

fn role_for_actor(network: &NetworkRecord, actor: &NodeId) -> MemberLifecycleActorRole {
    if network.owner == *actor {
        MemberLifecycleActorRole::Owner
    } else if network.administrators.contains(actor) {
        MemberLifecycleActorRole::Administrator
    } else {
        MemberLifecycleActorRole::Member
    }
}

fn role_for_target(
    network: &NetworkRecord,
    current: Option<&MemberLifecycleRecord>,
    target: &NodeId,
) -> MembershipRole {
    if network.owner == *target {
        MembershipRole::Owner
    } else if network.administrators.contains(target)
        || current.is_some_and(|record| record.role == MembershipRole::Administrator)
    {
        MembershipRole::Administrator
    } else {
        MembershipRole::Member
    }
}

fn retained_administrators(network: &NetworkRecord, target: &NodeId) -> usize {
    network
        .administrators
        .iter()
        .filter(|administrator| *administrator != target)
        .count()
}

fn append_audit(
    file: &mut MemberLifecycleFile,
    actor: &NodeId,
    request: &MemberLifecycleChangeRequest,
    digest: &ContentHash,
    outcome: AuditOutcome,
    change: Option<&MemberLifecycleChange>,
    now_unix_ms: u64,
) -> Result<()> {
    let seed = format!(
        "member-lifecycle-audit:{}:{}:{}:{}",
        request.network_id,
        request.target_node_id,
        request.idempotency_key,
        match outcome {
            AuditOutcome::Succeeded => "succeeded",
            AuditOutcome::Denied => "denied",
            AuditOutcome::Failed => "failed",
        }
    );
    let mut details = BTreeMap::from([
        (
            "action".to_string(),
            request.action.audit_action().to_string(),
        ),
        (
            "target_node_id".to_string(),
            request.target_node_id.to_string(),
        ),
        ("request_digest".to_string(), digest.as_str().to_string()),
    ]);
    if let Some(change) = change {
        details.insert("status".into(), format!("{:?}", change.status));
        details.insert("revision".into(), change.revision.to_string());
        details.insert("access_epoch".into(), change.access_epoch.to_string());
    }
    file.audit_log.append(AuditEntry {
        audit_id: AuditId::from_public_key(seed.as_bytes()),
        operation_id: Some(OperationId::from_public_key(
            format!(
                "member-lifecycle-operation:{}:{}",
                request.network_id, request.idempotency_key
            )
            .as_bytes(),
        )),
        actor: actor.clone(),
        network_id: Some(request.network_id.clone()),
        action: request.action.audit_action().into(),
        resource: format!(
            "network/{}/member/{}",
            request.network_id, request.target_node_id
        ),
        outcome,
        occurred_at_unix_ms: now_unix_ms,
        summary: match outcome {
            AuditOutcome::Succeeded => "member lifecycle changed",
            AuditOutcome::Denied => "member lifecycle denied",
            AuditOutcome::Failed => "member lifecycle failed",
        }
        .into(),
        details,
        sensitive_keys: BTreeSet::new(),
    })?;
    Ok(())
}

fn read_network_record(path: &Path, expected_network_id: &NetworkId) -> Result<NetworkRecord> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("无法读取成员生命周期网络资料: {}", path.display()))?;
    let record: NetworkRecord = serde_json::from_str(&content)
        .with_context(|| format!("无法解析成员生命周期网络资料: {}", path.display()))?;
    record.validate()?;
    if record.network_id != *expected_network_id {
        bail!("member lifecyle network does not match owner network record");
    }
    Ok(record)
}

fn persist_network_record(path: &Path, record: &NetworkRecord) -> Result<()> {
    record.validate()?;
    let bytes = serde_json::to_vec_pretty(record).context("无法序列化成员生命周期网络资料")?;
    persist_bytes_atomic(path, &bytes)
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
        lock.lock_exclusive()
            .with_context(|| format!("无法锁定网络资料: {}", lock_path.display()))?;
    } else {
        lock.lock_shared()
            .with_context(|| format!("无法锁定网络资料: {}", lock_path.display()))?;
    }
    Ok(lock)
}

fn lock_lifecycle_file(path: &Path) -> Result<File> {
    let lock_path = path.with_extension("json.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("无法打开成员生命周期锁: {}", lock_path.display()))?;
    lock.lock_exclusive()
        .with_context(|| format!("无法锁定成员生命周期存储: {}", lock_path.display()))?;
    Ok(lock)
}

fn persist_bytes_atomic(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path.parent().context("持久化路径没有父目录")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("无法创建持久化目录: {}", parent.display()))?;
    let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
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

fn restore_optional_file(path: &Path, bytes: Option<&[u8]>) -> Result<()> {
    match bytes {
        Some(bytes) => persist_bytes_atomic(path, bytes),
        None if path.exists() => std::fs::remove_file(path)
            .with_context(|| format!("无法回滚持久化文件: {}", path.display())),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuckipv6_common::domain::{
        ExitContributionObligation, ExitContributionScope, NetworkJoinMode, NetworkVisibility,
        PublicKey,
    };
    use std::sync::{Arc, Barrier};

    fn test_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "fuckipv6-member-lifecycle-{}-{label}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn owner() -> NodeId {
        NodeId::from_public_key(b"owner")
    }

    fn admin() -> NodeId {
        NodeId::from_public_key(b"admin")
    }

    fn member() -> NodeId {
        NodeId::from_public_key(b"member")
    }

    fn network_record() -> NetworkRecord {
        NetworkRecord::create_owned(
            fuckipv6_common::domain::NetworkCreateProfile {
                name: "home".into(),
                purpose: "test".into(),
                icon: "home".into(),
                language: "en".into(),
                region: "US".into(),
                tags: Vec::new(),
                capability_declarations: vec![CapabilityKind::File],
                visibility: NetworkVisibility::Private,
                join_mode: NetworkJoinMode::ApprovalRequired,
                exit_contribution: ExitContributionObligation::NotRequired,
                exit_contribution_scope: ExitContributionScope::NotApplicable,
            },
            PublicKey::from_bytes([7; 32]),
            owner(),
            100,
        )
        .unwrap()
    }

    fn setup(label: &str) -> (PathBuf, PathBuf, MemberLifecycleStore) {
        let dir = test_dir(label);
        let network_path = dir.join("network.json");
        let mut record = network_record();
        record.admit_member(admin(), 100).unwrap();
        record.administrators.push(admin());
        record.admit_member(member(), 100).unwrap();
        persist_network_record(&network_path, &record).unwrap();
        let store =
            MemberLifecycleStore::new(dir.join("member_lifecycle.json"), &network_path).unwrap();
        (dir, network_path, store)
    }

    fn request(
        action: MemberLifecycleAction,
        target: NodeId,
        expected: u64,
    ) -> MemberLifecycleChangeRequest {
        MemberLifecycleChangeRequest {
            network_id: network_record().network_id,
            target_node_id: target,
            action,
            role: MembershipRole::Member,
            expected_revision: expected,
            idempotency_key: IdempotencyKey::from_public_key(action.audit_action().as_bytes()),
            reason: None,
        }
    }

    #[test]
    fn lifecycle_mutations_are_authorized_and_idempotent() {
        let (_dir, network_path, store) = setup("idempotent");
        let first = store
            .change(
                &owner(),
                request(MemberLifecycleAction::Suspend, member(), 1),
                200,
            )
            .unwrap();
        assert_eq!(first.status, MemberLifecycleStatus::Suspended);
        assert_eq!(first.revision, 2);
        assert_eq!(first.access_epoch, 2);
        assert!(first.access_revoked);
        assert!(!read_network_record(&network_path, &first.network_id)
            .unwrap()
            .members
            .contains(&member()));

        let retried = store
            .change(
                &owner(),
                request(MemberLifecycleAction::Suspend, member(), 1),
                300,
            )
            .unwrap();
        assert!(retried.idempotent);
        assert_eq!(retried.revision, first.revision);
        assert_eq!(retried.access_epoch, first.access_epoch);
        assert_eq!(store.audit_entries().unwrap().len(), 1);
    }

    #[test]
    fn idempotency_key_reuse_with_different_payload_is_rejected() {
        let (_dir, _network_path, store) = setup("conflict");
        let mut first = request(MemberLifecycleAction::Suspend, member(), 1);
        store.change(&owner(), first.clone(), 200).unwrap();
        first.reason = Some("different payload".into());
        assert_eq!(
            store.change(&owner(), first, 300).unwrap_err().to_string(),
            "member_lifecycle.error.idempotency_conflict"
        );
    }

    #[test]
    fn ordinary_members_cannot_operate_on_other_members_and_owner_is_protected() {
        let (_dir, network_path, store) = setup("permissions");
        assert_eq!(
            store
                .change(
                    &member(),
                    request(MemberLifecycleAction::Suspend, admin(), 1),
                    200,
                )
                .unwrap_err()
                .to_string(),
            "member_lifecycle.error.permission_denied"
        );
        assert_eq!(
            store
                .change(
                    &admin(),
                    request(MemberLifecycleAction::Remove, owner(), 1),
                    200,
                )
                .unwrap_err()
                .to_string(),
            "member_lifecycle.error.owner_protected"
        );
        let record = read_network_record(&network_path, &network_record().network_id).unwrap();
        assert!(record.members.contains(&admin()));
        assert_eq!(store.audit_entries().unwrap().len(), 2);
    }

    #[test]
    fn last_administrator_and_owner_cannot_be_removed() {
        let (_dir, _network_path, store) = setup("threshold");
        assert_eq!(
            store
                .change(
                    &owner(),
                    request(MemberLifecycleAction::Remove, admin(), 1),
                    200,
                )
                .unwrap_err()
                .to_string(),
            "member_lifecycle.error.last_administrator_protected"
        );
    }

    #[test]
    fn remove_undo_requires_reapproval_and_never_restores_old_epoch() {
        let (_dir, network_path, store) = setup("undo");
        let removed = store
            .change(
                &owner(),
                request(MemberLifecycleAction::Remove, member(), 1),
                200,
            )
            .unwrap();
        let undone = store
            .change(
                &owner(),
                request(
                    MemberLifecycleAction::UndoRemove,
                    member(),
                    removed.revision,
                ),
                300,
            )
            .unwrap();
        assert_eq!(undone.status, MemberLifecycleStatus::PendingApproval);
        assert!(undone.requires_reapproval);
        assert!(undone.access_epoch > removed.access_epoch);
        assert!(!read_network_record(&network_path, &removed.network_id)
            .unwrap()
            .members
            .contains(&member()));
        let approved = store
            .change(
                &owner(),
                request(MemberLifecycleAction::Approve, member(), undone.revision),
                400,
            )
            .unwrap();
        assert_eq!(approved.status, MemberLifecycleStatus::Approved);
        assert!(approved.access_epoch > undone.access_epoch);
        assert!(read_network_record(&network_path, &removed.network_id)
            .unwrap()
            .members
            .contains(&member()));
    }

    #[test]
    fn concurrent_mutations_use_cas_and_only_one_wins() {
        let (_dir, network_path, store) = setup("cas");
        let store = Arc::new(store);
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for action in [
            MemberLifecycleAction::Suspend,
            MemberLifecycleAction::Remove,
        ] {
            let store = store.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                store.change(&owner(), request(action, member(), 1), 200)
            }));
        }
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| result.is_err())
                .map(|result| result.as_ref().unwrap_err().to_string())
                .collect::<Vec<_>>(),
            vec!["member_lifecycle.error.stale_revision".to_string()]
        );
        let record = read_network_record(&network_path, &network_record().network_id).unwrap();
        assert!(!record.members.contains(&member()));
    }

    #[test]
    fn removal_preview_discloses_revocations_without_deleting_files_or_reviving_credentials() {
        let (_dir, _network_path, store) = setup("preview");
        let preview = store
            .removal_preview(
                &owner(),
                MemberRemovalPreviewRequest {
                    network_id: network_record().network_id,
                    target_node_id: member(),
                },
                NetworkMemberPresence::live(300),
                300,
            )
            .unwrap();
        assert!(preview.can_remove);
        assert!(preview.network_access_revoked);
        assert!(preview.file_access_revoked);
        assert!(preview.incomplete_transfers_resume_blocked);
        assert!(!preview.local_files_deleted);
        assert!(preview.safe_undo_requires_reapproval);
        assert!(!preview.safe_undo_restores_old_credentials);
    }
}
