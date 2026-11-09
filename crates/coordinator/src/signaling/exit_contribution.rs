use anyhow::{bail, Context, Result};
use fs2::FileExt;
use fuckipv6_common::domain::{
    AuditEntry, AuditId, AuditLog, AuditOutcome, ContentHash,
    ExitContributionAuthorizationSnapshot, ExitContributionMode, ExitContributionRequest,
    ExitContributionRequestId, ExitContributionRequestState, ExitContributionReviewAction,
    NetworkId, NetworkRecord, NodeId,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const EXIT_CONTRIBUTION_REQUEST_FILE_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExitContributionRequestFile {
    schema_version: u16,
    #[serde(default)]
    requests: BTreeMap<ExitContributionRequestId, ExitContributionRequest>,
    #[serde(default)]
    audit_log: AuditLog,
}

impl Default for ExitContributionRequestFile {
    fn default() -> Self {
        Self {
            schema_version: EXIT_CONTRIBUTION_REQUEST_FILE_SCHEMA_VERSION,
            requests: BTreeMap::new(),
            audit_log: AuditLog::default(),
        }
    }
}

impl ExitContributionRequestFile {
    fn validate(&self) -> Result<()> {
        if self.schema_version != EXIT_CONTRIBUTION_REQUEST_FILE_SCHEMA_VERSION {
            bail!("unsupported exit contribution request file schema version");
        }
        for (request_id, request) in &self.requests {
            if request.request_id != *request_id {
                bail!("exit contribution request map key does not match request id");
            }
            request.validate()?;
        }
        for entry in self.audit_log.entries() {
            entry.validate()?;
        }
        Ok(())
    }

    fn expire_and_reconcile(
        &mut self,
        record: &NetworkRecord,
        terms_hash: &ContentHash,
        now_unix_ms: u64,
    ) -> Result<()> {
        let audit_log = &mut self.audit_log;
        for request in self.requests.values_mut() {
            if let Some(change) = request.expire(now_unix_ms)? {
                append_request_audit(
                    audit_log,
                    &request.node_id,
                    "network.exit_contribution.expire",
                    "exit contribution request expired",
                    change.previous_state,
                    &change.request,
                    now_unix_ms,
                )?;
                *request = change.request;
            }
            if let Some(change) =
                request.reconcile_policy(record.policy_revision, terms_hash, now_unix_ms)?
            {
                append_request_audit(
                    audit_log,
                    &record.owner,
                    "network.exit_contribution.supersede",
                    "exit contribution request superseded",
                    change.previous_state,
                    &change.request,
                    now_unix_ms,
                )?;
                *request = change.request;
            }
        }
        Ok(())
    }
}

/// Durable Coordinator authority for approval-gated Exit contribution.
///
/// Every operation reloads the trusted network record and reconciles pending or
/// approved authority against its current policy revision and membership terms.
/// The process mutex and file lock cover the complete read-modify-write
/// transaction; persistence failure leaves the previous file untouched.
pub struct ExitContributionRequestStore {
    path: PathBuf,
    network_record_path: PathBuf,
    process_lock: Mutex<()>,
}

impl ExitContributionRequestStore {
    pub fn new(path: impl Into<PathBuf>, network_record_path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            network_record_path: network_record_path.into(),
            process_lock: Mutex::new(()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn submit(
        &self,
        expected_network_id: &NetworkId,
        requester: &NodeId,
        request_id: ExitContributionRequestId,
        expected_policy_revision: u64,
        expected_terms_hash: fuckipv6_common::domain::ContentHash,
        requested_bandwidth_kbps: Option<u64>,
        requested_traffic_quota_bytes: Option<u64>,
        now_unix_ms: u64,
    ) -> Result<ExitContributionRequest> {
        self.transaction(
            expected_network_id,
            now_unix_ms,
            |file, record, terms_hash| {
                if !record.members.contains(requester) {
                    bail!("exit_contribution.error.member_required");
                }
                if record.exit_contribution_mode != ExitContributionMode::ApprovalRequired {
                    bail!("exit_contribution.error.mode_not_allowed");
                }
                if expected_policy_revision != record.policy_revision
                    || expected_terms_hash != *terms_hash
                {
                    bail!("exit_contribution.error.stale_policy");
                }
                if let Some(existing) = file.requests.get(&request_id) {
                    if existing.network_id == *expected_network_id
                        && existing.node_id == *requester
                        && existing.policy_revision == expected_policy_revision
                        && existing.terms_hash == expected_terms_hash
                    {
                        return Ok(existing.clone());
                    }
                    bail!("exit_contribution.error.request_id_collision");
                }
                if file.requests.values().any(|request| {
                    request.network_id == *expected_network_id
                        && request.node_id == *requester
                        && matches!(
                            request.state,
                            fuckipv6_common::domain::ExitContributionRequestState::Pending
                                | fuckipv6_common::domain::ExitContributionRequestState::Approved
                        )
                }) {
                    bail!("exit_contribution.error.active_request_exists");
                }
                let request = ExitContributionRequest::submit(
                    request_id.clone(),
                    expected_network_id.clone(),
                    requester.clone(),
                    expected_policy_revision,
                    expected_terms_hash,
                    requested_bandwidth_kbps,
                    requested_traffic_quota_bytes,
                    now_unix_ms,
                )?;
                append_request_audit(
                    &mut file.audit_log,
                    requester,
                    "network.exit_contribution.submit",
                    "exit contribution request submitted",
                    ExitContributionRequestState::Pending,
                    &request,
                    now_unix_ms,
                )?;
                file.requests.insert(request_id, request.clone());
                Ok(request)
            },
        )
    }

    /// Reviewers see the full network request ledger. Ordinary members only see
    /// their own requests; non-members receive a fail-closed permission error.
    pub fn list(
        &self,
        network_id: &NetworkId,
        actor: &NodeId,
        now_unix_ms: u64,
    ) -> Result<Vec<ExitContributionRequest>> {
        self.transaction(network_id, now_unix_ms, |file, record, _| {
            let reviewer = record.owner == *actor || record.administrators.contains(actor);
            if !reviewer && !record.members.contains(actor) {
                bail!("exit_contribution.error.permission_denied");
            }
            let mut requests = file
                .requests
                .values()
                .filter(|request| {
                    request.network_id == *network_id && (reviewer || request.node_id == *actor)
                })
                .cloned()
                .collect::<Vec<_>>();
            requests.sort_by(|left, right| {
                right
                    .updated_at_unix_ms
                    .cmp(&left.updated_at_unix_ms)
                    .then_with(|| left.request_id.cmp(&right.request_id))
            });
            Ok(requests)
        })
    }

    pub fn status(
        &self,
        network_id: &NetworkId,
        actor: &NodeId,
        request_id: &ExitContributionRequestId,
        now_unix_ms: u64,
    ) -> Result<ExitContributionRequest> {
        self.transaction(network_id, now_unix_ms, |file, record, _| {
            let reviewer = record.owner == *actor || record.administrators.contains(actor);
            let request = file
                .requests
                .get(request_id)
                .context("exit_contribution.error.request_not_found")?;
            if request.network_id != *network_id {
                bail!("exit_contribution.error.request_not_found");
            }
            if !reviewer && request.node_id != *actor {
                bail!("exit_contribution.error.permission_denied");
            }
            Ok(request.clone())
        })
    }

    /// Return the sanitized authorization evidence visible to every current
    /// network member. Only the newest state per node is projected so a stale
    /// terminal request cannot overwrite newer approval authority.
    pub fn authorization_snapshots(
        &self,
        network_id: &NetworkId,
        actor: &NodeId,
        now_unix_ms: u64,
    ) -> Result<Vec<ExitContributionAuthorizationSnapshot>> {
        self.transaction(network_id, now_unix_ms, |file, record, _| {
            if !record.members.contains(actor) {
                bail!("exit_contribution.error.permission_denied");
            }
            let mut latest = BTreeMap::<NodeId, &ExitContributionRequest>::new();
            for request in file.requests.values().filter(|request| {
                request.network_id == *network_id && record.members.contains(&request.node_id)
            }) {
                let replace = latest.get(&request.node_id).is_none_or(|current| {
                    (
                        request.updated_at_unix_ms,
                        request.revision,
                        &request.request_id,
                    ) > (
                        current.updated_at_unix_ms,
                        current.revision,
                        &current.request_id,
                    )
                });
                if replace {
                    latest.insert(request.node_id.clone(), request);
                }
            }
            Ok(latest
                .into_values()
                .map(ExitContributionRequest::authorization_snapshot)
                .collect())
        })
    }

    /// Read the trusted roster for authenticated availability observations.
    pub(crate) fn network_record(&self, network_id: &NetworkId) -> Result<NetworkRecord> {
        read_network_record(&self.network_record_path, network_id)
    }

    /// Return the immutable, sanitized contribution governance ledger.
    pub fn audit_entries(&self) -> Result<Vec<AuditEntry>> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("无法创建 Exit 贡献申请目录: {}", parent.display()))?;
        }
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("exit contribution request store lock is poisoned"))?;
        let lock_path = self.path.with_extension("json.lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("无法打开 Exit 贡献申请锁: {}", lock_path.display()))?;
        lock.lock_exclusive()
            .with_context(|| format!("无法锁定 Exit 贡献申请存储: {}", lock_path.display()))?;
        let entries = self
            .load_unlocked()
            .map(|file| file.audit_log.entries().to_vec());
        let _ = FileExt::unlock(&lock);
        entries
    }

    #[allow(clippy::too_many_arguments)]
    pub fn review(
        &self,
        network_id: &NetworkId,
        reviewer: &NodeId,
        request_id: &ExitContributionRequestId,
        expected_revision: u64,
        action: ExitContributionReviewAction,
        decision_reason_code: Option<String>,
        now_unix_ms: u64,
    ) -> Result<ExitContributionRequest> {
        self.transaction(network_id, now_unix_ms, |file, record, _| {
            if record.owner != *reviewer && !record.administrators.contains(reviewer) {
                bail!("exit_contribution.error.reviewer_required");
            }
            if record.exit_contribution_mode != ExitContributionMode::ApprovalRequired {
                bail!("exit_contribution.error.mode_not_allowed");
            }
            let request = file
                .requests
                .get_mut(request_id)
                .context("exit_contribution.error.request_not_found")?;
            if request.network_id != *network_id {
                bail!("exit_contribution.error.request_not_found");
            }
            if request.node_id == *reviewer {
                bail!("exit_contribution.error.self_review_forbidden");
            }
            let change = request.review(
                reviewer.clone(),
                expected_revision,
                action,
                decision_reason_code,
                now_unix_ms,
            )?;
            append_request_audit(
                &mut file.audit_log,
                reviewer,
                "network.exit_contribution.review",
                "exit contribution request reviewed",
                change.previous_state,
                &change.request,
                now_unix_ms,
            )?;
            *request = change.request.clone();
            Ok(change.request)
        })
    }

    pub fn withdraw(
        &self,
        network_id: &NetworkId,
        actor: &NodeId,
        request_id: &ExitContributionRequestId,
        expected_revision: u64,
        now_unix_ms: u64,
    ) -> Result<ExitContributionRequest> {
        self.transaction(network_id, now_unix_ms, |file, _, _| {
            let request = file
                .requests
                .get_mut(request_id)
                .context("exit_contribution.error.request_not_found")?;
            if request.network_id != *network_id {
                bail!("exit_contribution.error.request_not_found");
            }
            let change = request.withdraw(actor, expected_revision, now_unix_ms)?;
            append_request_audit(
                &mut file.audit_log,
                actor,
                "network.exit_contribution.withdraw",
                "exit contribution request withdrawn",
                change.previous_state,
                &change.request,
                now_unix_ms,
            )?;
            *request = change.request.clone();
            Ok(change.request)
        })
    }

    fn transaction<T>(
        &self,
        expected_network_id: &NetworkId,
        now_unix_ms: u64,
        mutate: impl FnOnce(
            &mut ExitContributionRequestFile,
            &NetworkRecord,
            &fuckipv6_common::domain::ContentHash,
        ) -> Result<T>,
    ) -> Result<T> {
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("exit contribution request store lock is poisoned"))?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("无法创建 Exit 贡献申请目录: {}", parent.display()))?;
        }
        let lock_path = self.path.with_extension("json.lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("无法打开 Exit 贡献申请锁: {}", lock_path.display()))?;
        lock.lock_exclusive()
            .with_context(|| format!("无法锁定 Exit 贡献申请存储: {}", lock_path.display()))?;
        let result = self.transaction_locked(expected_network_id, now_unix_ms, mutate);
        let _ = FileExt::unlock(&lock);
        result
    }

    fn transaction_locked<T>(
        &self,
        expected_network_id: &NetworkId,
        now_unix_ms: u64,
        mutate: impl FnOnce(
            &mut ExitContributionRequestFile,
            &NetworkRecord,
            &fuckipv6_common::domain::ContentHash,
        ) -> Result<T>,
    ) -> Result<T> {
        let record = read_network_record(&self.network_record_path, expected_network_id)?;
        let terms_hash = record
            .membership_terms()
            .content_hash()
            .context("Exit 贡献成员条款内容哈希失败")?;
        let mut candidate = self.load_unlocked()?;
        candidate.expire_and_reconcile(&record, &terms_hash, now_unix_ms)?;
        let result = mutate(&mut candidate, &record, &terms_hash)?;
        candidate.validate()?;
        self.persist(&candidate)?;
        Ok(result)
    }

    fn load_unlocked(&self) -> Result<ExitContributionRequestFile> {
        if !self.path.exists() {
            return Ok(ExitContributionRequestFile::default());
        }
        let content = std::fs::read_to_string(&self.path)
            .with_context(|| format!("无法读取 Exit 贡献申请存储: {}", self.path.display()))?;
        let file: ExitContributionRequestFile = serde_json::from_str(&content)
            .with_context(|| format!("无法解析 Exit 贡献申请存储: {}", self.path.display()))?;
        file.validate()?;
        Ok(file)
    }

    fn persist(&self, file: &ExitContributionRequestFile) -> Result<()> {
        file.validate()?;
        let content = serde_json::to_vec_pretty(file).context("无法序列化 Exit 贡献申请存储")?;
        let temporary = self
            .path
            .with_extension(format!("json.tmp-{}", std::process::id()));
        {
            let mut output = File::create(&temporary).with_context(|| {
                format!("无法创建 Exit 贡献申请临时文件: {}", temporary.display())
            })?;
            output.write_all(&content).with_context(|| {
                format!("无法写入 Exit 贡献申请临时文件: {}", temporary.display())
            })?;
            output.sync_all().with_context(|| {
                format!("无法同步 Exit 贡献申请临时文件: {}", temporary.display())
            })?;
        }
        if let Err(error) = std::fs::rename(&temporary, &self.path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error)
                .with_context(|| format!("无法替换 Exit 贡献申请存储: {}", self.path.display()));
        }
        if let Some(parent) = self.path.parent() {
            let directory = File::open(parent)
                .with_context(|| format!("无法打开 Exit 贡献申请目录: {}", parent.display()))?;
            directory
                .sync_all()
                .with_context(|| format!("无法同步 Exit 贡献申请目录: {}", parent.display()))?;
        }
        Ok(())
    }
}

fn append_request_audit(
    audit_log: &mut AuditLog,
    actor: &NodeId,
    action: &str,
    summary: &str,
    previous_state: ExitContributionRequestState,
    request: &ExitContributionRequest,
    now_unix_ms: u64,
) -> Result<()> {
    let seed = format!(
        "exit-contribution-audit:{}:{}:{}:{}",
        request.network_id, request.request_id, action, request.revision
    );
    let mut details = BTreeMap::from([
        ("request_id".into(), request.request_id.to_string()),
        ("requested_node_id".into(), request.node_id.to_string()),
        (
            "policy_revision".into(),
            request.policy_revision.to_string(),
        ),
        ("terms_hash".into(), request.terms_hash.as_str().to_string()),
        (
            "previous_state".into(),
            request_state_code(previous_state).into(),
        ),
        ("state".into(), request_state_code(request.state).into()),
        ("revision".into(), request.revision.to_string()),
    ]);
    if let Some(reason) = request.decision_reason_code.as_deref() {
        details.insert("decision_reason_code".into(), reason.to_string());
    }
    audit_log.append(AuditEntry {
        audit_id: AuditId::from_public_key(seed.as_bytes()),
        operation_id: None,
        actor: actor.clone(),
        network_id: Some(request.network_id.clone()),
        action: action.into(),
        resource: format!(
            "network/{}/exit-contribution/{}",
            request.network_id, request.request_id
        ),
        outcome: AuditOutcome::Succeeded,
        occurred_at_unix_ms: now_unix_ms,
        summary: summary.into(),
        details,
        sensitive_keys: Default::default(),
    })?;
    Ok(())
}

const fn request_state_code(state: ExitContributionRequestState) -> &'static str {
    match state {
        ExitContributionRequestState::Pending => "pending",
        ExitContributionRequestState::Approved => "approved",
        ExitContributionRequestState::Rejected => "rejected",
        ExitContributionRequestState::Withdrawn => "withdrawn",
        ExitContributionRequestState::Expired => "expired",
        ExitContributionRequestState::Superseded => "superseded",
    }
}

fn read_network_record(path: &Path, expected_network_id: &NetworkId) -> Result<NetworkRecord> {
    let lock = lock_profile_shared(path)?;
    let result = (|| {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("无法读取 Exit 贡献授权网络资料: {}", path.display()))?;
        let record: NetworkRecord = serde_json::from_str(&content)
            .with_context(|| format!("无法解析 Exit 贡献授权网络资料: {}", path.display()))?;
        record.validate().context("Exit 贡献授权网络资料校验失败")?;
        if record.network_id != *expected_network_id {
            bail!("Exit 贡献申请网络与 owner 网络资料不一致");
        }
        Ok(record)
    })();
    let _ = FileExt::unlock(&lock);
    result
}

fn lock_profile_shared(path: &Path) -> Result<File> {
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
    lock.lock_shared()
        .with_context(|| format!("无法锁定网络资料: {}", lock_path.display()))?;
    Ok(lock)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuckipv6_common::domain::{
        CapabilityKind, ContentHash, ExitContributionObligation, ExitContributionRequestState,
        ExitContributionScope, NetworkCreateProfile, NetworkJoinMode, NetworkVisibility, PublicKey,
    };
    use std::sync::Arc;

    fn test_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fuckipv6-exit-contribution-{}-{label}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn public_key() -> PublicKey {
        PublicKey::from_bytes(*b"01234567890123456789012345678901")
    }

    fn network_id() -> NetworkId {
        NetworkId::from_public_key(public_key().as_str().as_bytes())
    }

    fn node(seed: &[u8]) -> NodeId {
        NodeId::from_public_key(seed)
    }

    fn owner() -> NodeId {
        node(b"exit-store-owner")
    }

    fn admin() -> NodeId {
        node(b"exit-store-admin")
    }

    fn member(seed: &[u8]) -> NodeId {
        node(seed)
    }

    fn record(mode: ExitContributionMode) -> NetworkRecord {
        let (obligation, scope) = if mode == ExitContributionMode::RequiredOnJoin {
            (
                ExitContributionObligation::Required,
                ExitContributionScope::JoiningMembers,
            )
        } else {
            (
                ExitContributionObligation::NotRequired,
                ExitContributionScope::NotApplicable,
            )
        };
        let mut record = NetworkRecord::create_owned(
            NetworkCreateProfile {
                name: "ExitGovernance".into(),
                purpose: "Coordinate approval-gated Exit contribution.".into(),
                icon: "network".into(),
                language: "en".into(),
                region: "ZZ".into(),
                tags: Vec::new(),
                capability_declarations: vec![CapabilityKind::Exit],
                visibility: NetworkVisibility::Private,
                join_mode: NetworkJoinMode::ApprovalRequired,
                exit_contribution: obligation,
                exit_contribution_scope: scope,
                exit_contribution_mode: mode,
            },
            public_key(),
            owner(),
            1_000,
        )
        .unwrap();
        record.members.push(admin());
        record.administrators.push(admin());
        record.members.push(member(b"exit-store-member"));
        record.updated_at_unix_ms = 1_001;
        record.validate().unwrap();
        record
    }

    fn write_record(dir: &Path, record: &NetworkRecord) -> PathBuf {
        let path = dir.join("network.json");
        std::fs::write(&path, serde_json::to_vec_pretty(record).unwrap()).unwrap();
        path
    }

    fn store(dir: &Path, record: &NetworkRecord) -> ExitContributionRequestStore {
        ExitContributionRequestStore::new(
            dir.join("exit_contribution_requests.json"),
            write_record(dir, record),
        )
    }

    fn terms(record: &NetworkRecord) -> ContentHash {
        record.membership_terms().content_hash().unwrap()
    }

    fn submit(
        store: &ExitContributionRequestStore,
        seed: &[u8],
        record: &NetworkRecord,
        now: u64,
    ) -> Result<ExitContributionRequest> {
        store.submit(
            &record.network_id,
            &member(seed),
            ExitContributionRequestId::from_public_key(seed),
            record.policy_revision,
            terms(record),
            Some(20_000),
            Some(1_000_000),
            now,
        )
    }

    #[test]
    fn requests_survive_restart_and_member_visibility_is_filtered() {
        let dir = test_dir("restart");
        let record = record(ExitContributionMode::ApprovalRequired);
        let store = store(&dir, &record);
        let request = submit(&store, b"exit-store-member", &record, 2_000).unwrap();
        assert_eq!(request.state, ExitContributionRequestState::Pending);

        let reopened = ExitContributionRequestStore::new(
            dir.join("exit_contribution_requests.json"),
            dir.join("network.json"),
        );
        assert_eq!(
            reopened
                .list(&network_id(), &member(b"exit-store-member"), 2_001)
                .unwrap()
                .len(),
            1
        );
        assert!(reopened
            .list(&network_id(), &owner(), 2_001)
            .unwrap()
            .iter()
            .any(|candidate| candidate.request_id == request.request_id));
        assert!(reopened
            .list(&network_id(), &member(b"exit-store-outsider"), 2_001)
            .is_err());
    }

    #[test]
    fn members_only_receive_the_latest_sanitized_authorization_projection() {
        let dir = test_dir("authorization-projection");
        let record = record(ExitContributionMode::ApprovalRequired);
        let store = store(&dir, &record);
        let first = submit(&store, b"exit-store-member", &record, 2_000).unwrap();
        store
            .review(
                &network_id(),
                &admin(),
                &first.request_id,
                1,
                ExitContributionReviewAction::Reject,
                Some("exit.rejected.policy".into()),
                2_001,
            )
            .unwrap();
        let second = store
            .submit(
                &record.network_id,
                &member(b"exit-store-member"),
                ExitContributionRequestId::from_public_key(b"exit-store-second"),
                record.policy_revision,
                terms(&record),
                Some(50_000),
                Some(5_000_000),
                2_002,
            )
            .unwrap();

        let snapshots = store
            .authorization_snapshots(&network_id(), &member(b"exit-store-member"), 2_003)
            .unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].request_id, second.request_id);
        assert_eq!(snapshots[0].state, ExitContributionRequestState::Pending);
        let encoded = serde_json::to_value(&snapshots[0]).unwrap();
        assert!(encoded.get("requested_bandwidth_kbps").is_none());
        assert!(encoded.get("requested_traffic_quota_bytes").is_none());
        assert!(encoded.get("reviewer_node_id").is_none());
        assert!(encoded.get("decision_reason_code").is_none());
        assert!(store
            .authorization_snapshots(&network_id(), &member(b"exit-store-outsider"), 2_003)
            .is_err());
        assert!(store
            .network_record(&network_id())
            .unwrap()
            .members
            .contains(&member(b"exit-store-member")));
    }

    #[test]
    fn review_requires_owner_or_admin_and_forbids_self_approval() {
        let dir = test_dir("review-auth");
        let record = record(ExitContributionMode::ApprovalRequired);
        let store = store(&dir, &record);
        let request = submit(&store, b"exit-store-member", &record, 2_000).unwrap();

        assert!(store
            .review(
                &network_id(),
                &member(b"exit-store-member"),
                &request.request_id,
                1,
                ExitContributionReviewAction::Approve,
                None,
                2_001,
            )
            .is_err());
        assert!(store
            .review(
                &network_id(),
                &admin(),
                &request.request_id,
                99,
                ExitContributionReviewAction::Approve,
                None,
                2_001,
            )
            .is_err());
        let approved = store
            .review(
                &network_id(),
                &admin(),
                &request.request_id,
                1,
                ExitContributionReviewAction::Approve,
                Some("exit.approved.standard".into()),
                2_002,
            )
            .unwrap();
        assert_eq!(approved.state, ExitContributionRequestState::Approved);
        assert_eq!(approved.revision, 2);
        assert_eq!(approved.reviewer_node_id.as_ref(), Some(&admin()));
    }

    #[test]
    fn lifecycle_audit_is_persisted_sanitized_and_conflict_safe() {
        let dir = test_dir("lifecycle-audit");
        let mut record = record(ExitContributionMode::ApprovalRequired);
        record.members.push(member(b"exit-store-other"));
        record.updated_at_unix_ms += 1;
        record.validate().unwrap();
        let store = store(&dir, &record);
        let withdrawn = submit(&store, b"exit-store-member", &record, 2_000).unwrap();
        assert_eq!(store.audit_entries().unwrap().len(), 1);
        assert_eq!(
            submit(&store, b"exit-store-member", &record, 2_000).unwrap(),
            withdrawn
        );
        assert_eq!(store.audit_entries().unwrap().len(), 1);
        store
            .withdraw(
                &network_id(),
                &member(b"exit-store-member"),
                &withdrawn.request_id,
                1,
                2_001,
            )
            .unwrap();

        let rejected = store
            .submit(
                &record.network_id,
                &member(b"exit-store-member"),
                ExitContributionRequestId::from_public_key(b"exit-store-rejected"),
                record.policy_revision,
                terms(&record),
                None,
                None,
                2_002,
            )
            .unwrap();
        store
            .review(
                &network_id(),
                &owner(),
                &rejected.request_id,
                1,
                ExitContributionReviewAction::Reject,
                Some("exit.rejected.policy".into()),
                2_003,
            )
            .unwrap();
        let approved = store
            .submit(
                &record.network_id,
                &member(b"exit-store-member"),
                ExitContributionRequestId::from_public_key(b"exit-store-approved"),
                record.policy_revision,
                terms(&record),
                None,
                None,
                2_004,
            )
            .unwrap();
        store
            .review(
                &network_id(),
                &admin(),
                &approved.request_id,
                1,
                ExitContributionReviewAction::Approve,
                Some("exit.approved.standard".into()),
                2_005,
            )
            .unwrap();
        assert!(store
            .review(
                &network_id(),
                &admin(),
                &approved.request_id,
                1,
                ExitContributionReviewAction::Reject,
                None,
                2_006,
            )
            .is_err());
        assert_eq!(store.audit_entries().unwrap().len(), 6);

        assert!(store
            .submit(
                &record.network_id,
                &member(b"exit-store-other"),
                approved.request_id.clone(),
                record.policy_revision,
                terms(&record),
                None,
                None,
                2_006,
            )
            .is_err());

        let reopened = ExitContributionRequestStore::new(
            dir.join("exit_contribution_requests.json"),
            dir.join("network.json"),
        );
        let audits = reopened.audit_entries().unwrap();
        assert_eq!(audits.len(), 6);
        assert_eq!(
            audits
                .iter()
                .filter(|entry| entry.action == "network.exit_contribution.submit")
                .count(),
            3
        );
        assert_eq!(
            audits
                .iter()
                .filter(|entry| entry.action == "network.exit_contribution.review")
                .count(),
            2
        );
        assert_eq!(
            audits
                .iter()
                .filter(|entry| entry.action == "network.exit_contribution.withdraw")
                .count(),
            1
        );
        let approval = audits
            .iter()
            .find(|entry| {
                entry.action == "network.exit_contribution.review"
                    && entry.details.get("state").map(String::as_str) == Some("approved")
            })
            .unwrap();
        assert_eq!(approval.actor, admin());
        assert_eq!(approval.outcome, AuditOutcome::Succeeded);
        assert_eq!(
            approval
                .details
                .get("requested_node_id")
                .map(String::as_str),
            Some(member(b"exit-store-member").to_string().as_str())
        );
        assert_eq!(
            approval
                .details
                .get("decision_reason_code")
                .map(String::as_str),
            Some("exit.approved.standard")
        );
        assert!(!approval.details.contains_key("requested_bandwidth_kbps"));
        assert!(!approval
            .details
            .contains_key("requested_traffic_quota_bytes"));
    }

    #[test]
    fn expiry_and_policy_supersession_append_audit_once() {
        let dir = test_dir("automatic-audit");
        let record = record(ExitContributionMode::ApprovalRequired);
        let store = store(&dir, &record);
        let expiring = submit(&store, b"exit-store-member", &record, 2_000).unwrap();
        let expired = store
            .status(
                &network_id(),
                &member(b"exit-store-member"),
                &expiring.request_id,
                expiring.expires_at_unix_ms,
            )
            .unwrap();
        assert_eq!(expired.state, ExitContributionRequestState::Expired);
        store
            .status(
                &network_id(),
                &member(b"exit-store-member"),
                &expiring.request_id,
                expiring.expires_at_unix_ms + 1,
            )
            .unwrap();

        let approved = store
            .submit(
                &record.network_id,
                &member(b"exit-store-member"),
                ExitContributionRequestId::from_public_key(b"exit-store-superseded"),
                record.policy_revision,
                terms(&record),
                None,
                None,
                expiring.expires_at_unix_ms + 2,
            )
            .unwrap();
        store
            .review(
                &network_id(),
                &admin(),
                &approved.request_id,
                1,
                ExitContributionReviewAction::Approve,
                None,
                expiring.expires_at_unix_ms + 3,
            )
            .unwrap();
        let mut changed = record;
        changed.policy_revision += 1;
        changed.updated_at_unix_ms += 1;
        changed.validate().unwrap();
        write_record(&dir, &changed);
        store
            .list(&network_id(), &owner(), expiring.expires_at_unix_ms + 4)
            .unwrap();
        store
            .list(&network_id(), &owner(), expiring.expires_at_unix_ms + 5)
            .unwrap();

        let reopened = ExitContributionRequestStore::new(
            dir.join("exit_contribution_requests.json"),
            dir.join("network.json"),
        );
        let audits = reopened.audit_entries().unwrap();
        assert_eq!(audits.len(), 5);
        assert_eq!(
            audits
                .iter()
                .filter(|entry| entry.action == "network.exit_contribution.expire")
                .count(),
            1
        );
        assert_eq!(
            audits
                .iter()
                .filter(|entry| entry.action == "network.exit_contribution.supersede")
                .count(),
            1
        );
        let expired = audits
            .iter()
            .find(|entry| entry.action == "network.exit_contribution.expire")
            .unwrap();
        assert_eq!(
            expired.details.get("state").map(String::as_str),
            Some("expired")
        );
        let superseded = audits
            .iter()
            .find(|entry| entry.action == "network.exit_contribution.supersede")
            .unwrap();
        assert_eq!(superseded.actor, owner());
        assert_eq!(
            superseded.details.get("state").map(String::as_str),
            Some("superseded")
        );
    }

    #[test]
    fn only_approval_required_networks_accept_requests() {
        for mode in [
            ExitContributionMode::Disabled,
            ExitContributionMode::Autonomous,
            ExitContributionMode::RequiredOnJoin,
        ] {
            let dir = test_dir(&format!("mode-{mode:?}"));
            let record = record(mode);
            let store = store(&dir, &record);
            assert!(
                submit(&store, b"exit-store-member", &record, 2_000).is_err(),
                "{mode:?} must not persist approval requests"
            );
        }
    }

    #[test]
    fn expiry_and_withdrawal_are_persisted_with_cas() {
        let dir = test_dir("expiry-withdraw");
        let record = record(ExitContributionMode::ApprovalRequired);
        let store = store(&dir, &record);
        let expiring = submit(&store, b"exit-store-member", &record, 2_000).unwrap();
        let expired = store
            .status(
                &network_id(),
                &member(b"exit-store-member"),
                &expiring.request_id,
                expiring.expires_at_unix_ms,
            )
            .unwrap();
        assert_eq!(expired.state, ExitContributionRequestState::Expired);
        assert_eq!(expired.revision, 2);

        assert!(store
            .withdraw(
                &network_id(),
                &member(b"exit-store-member"),
                &expiring.request_id,
                expired.revision,
                expiring.expires_at_unix_ms + 1,
            )
            .is_err());

        let second = store
            .submit(
                &record.network_id,
                &member(b"exit-store-member"),
                ExitContributionRequestId::from_public_key(b"exit-store-withdraw"),
                record.policy_revision,
                terms(&record),
                Some(20_000),
                Some(1_000_000),
                expiring.expires_at_unix_ms + 2,
            )
            .unwrap();
        assert!(store
            .withdraw(
                &network_id(),
                &member(b"exit-store-member"),
                &second.request_id,
                99,
                expiring.expires_at_unix_ms + 3,
            )
            .is_err());
        let withdrawn = store
            .withdraw(
                &network_id(),
                &member(b"exit-store-member"),
                &second.request_id,
                1,
                expiring.expires_at_unix_ms + 3,
            )
            .unwrap();
        assert_eq!(withdrawn.state, ExitContributionRequestState::Withdrawn);

        let reopened = ExitContributionRequestStore::new(
            dir.join("exit_contribution_requests.json"),
            dir.join("network.json"),
        );
        let listed = reopened
            .list(&network_id(), &owner(), expiring.expires_at_unix_ms + 4)
            .unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed
            .iter()
            .all(|request| request.state != ExitContributionRequestState::Pending));
    }

    #[test]
    fn policy_revision_or_terms_change_supersedes_pending_and_approved_authority() {
        let dir = test_dir("supersede");
        let record = record(ExitContributionMode::ApprovalRequired);
        let store = store(&dir, &record);
        let pending = submit(&store, b"exit-store-member", &record, 2_000).unwrap();
        let approved = store
            .review(
                &network_id(),
                &owner(),
                &pending.request_id,
                1,
                ExitContributionReviewAction::Approve,
                None,
                2_001,
            )
            .unwrap();
        assert_eq!(approved.state, ExitContributionRequestState::Approved);

        let mut changed = record;
        changed.policy_revision += 1;
        changed.updated_at_unix_ms += 1;
        changed.validate().unwrap();
        write_record(&dir, &changed);

        let listed = store.list(&network_id(), &owner(), 2_002).unwrap();
        assert_eq!(listed[0].state, ExitContributionRequestState::Superseded);
        assert_eq!(
            listed[0].decision_reason_code.as_deref(),
            Some("exit_contribution.policy_changed")
        );

        let reopened = ExitContributionRequestStore::new(
            dir.join("exit_contribution_requests.json"),
            dir.join("network.json"),
        );
        assert_eq!(
            reopened
                .status(&network_id(), &owner(), &pending.request_id, 2_003)
                .unwrap()
                .state,
            ExitContributionRequestState::Superseded
        );
    }

    #[test]
    fn concurrent_submissions_keep_one_active_request_per_member() {
        let dir = test_dir("concurrent");
        let record = record(ExitContributionMode::ApprovalRequired);
        let store = Arc::new(store(&dir, &record));
        let mut handles = Vec::new();
        for index in 0..8u8 {
            let store = Arc::clone(&store);
            let record = record.clone();
            handles.push(std::thread::spawn(move || {
                store.submit(
                    &record.network_id,
                    &member(b"exit-store-member"),
                    ExitContributionRequestId::from_public_key(&[index]),
                    record.policy_revision,
                    terms(&record),
                    None,
                    None,
                    2_000 + u64::from(index),
                )
            }));
        }
        let successes = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(Result::is_ok)
            .count();
        assert_eq!(successes, 1);
        assert_eq!(store.list(&network_id(), &owner(), 3_000).unwrap().len(), 1);
    }
}
