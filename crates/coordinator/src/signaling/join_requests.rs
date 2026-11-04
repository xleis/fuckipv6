use anyhow::{bail, Context, Result};
use fs2::FileExt;
use fuckipv6_common::domain::{
    JoinRequest, JoinRequestId, JoinRequestState, NetworkId, NetworkJoinMode, NetworkRecord, NodeId,
};
use fuckipv6_common::{Identity, JoinRequestBatchReviewItem, JoinRequestReviewResult};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const JOIN_REQUEST_FILE_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JoinRequestFile {
    schema_version: u16,
    #[serde(default)]
    requests: BTreeMap<JoinRequestId, JoinRequest>,
    #[serde(default)]
    accepted_members: BTreeSet<NodeId>,
}

impl Default for JoinRequestFile {
    fn default() -> Self {
        Self {
            schema_version: JOIN_REQUEST_FILE_SCHEMA_VERSION,
            requests: BTreeMap::new(),
            accepted_members: BTreeSet::new(),
        }
    }
}

impl JoinRequestFile {
    fn validate(&self) -> Result<()> {
        if self.schema_version != JOIN_REQUEST_FILE_SCHEMA_VERSION {
            bail!("unsupported join request file schema version");
        }
        let approved = self
            .requests
            .values()
            .filter(|request| request.state == JoinRequestState::Approved)
            .map(|request| request.applicant.node_id.clone())
            .collect::<BTreeSet<_>>();
        for (request_id, request) in &self.requests {
            if request.request_id != *request_id {
                bail!("join request map key does not match request id");
            }
            request.validate()?;
        }
        if approved != self.accepted_members {
            bail!("approved requests and accepted members are inconsistent");
        }
        Ok(())
    }

    fn expire_and_scrub(&mut self, now_unix_ms: u64) -> Result<bool> {
        let mut changed = false;
        for request in self.requests.values_mut() {
            changed |= request.expire_if_due(now_unix_ms)?;
            changed |= request.scrub_expired_reason(now_unix_ms);
        }
        Ok(changed)
    }
}

#[derive(Debug, Clone)]
struct NetworkRoster {
    owner: NodeId,
    administrators: BTreeSet<NodeId>,
}

impl NetworkRoster {
    fn is_reviewer(&self, actor: &NodeId) -> bool {
        self.owner == *actor || self.administrators.contains(actor)
    }
}

/// Durable, atomic approval-request store owned by one Coordinator.
///
/// The file lock covers the complete read-modify-write transaction. A batch is
/// validated and mutated in memory first, then written once; any failure leaves
/// the previous file untouched. Roster data is re-read for every authorization
/// and review so demoting an administrator takes effect immediately.
pub struct JoinRequestStore {
    path: PathBuf,
    network_record_path: Option<PathBuf>,
    fallback_owner: Option<NodeId>,
    fallback_administrators: BTreeSet<NodeId>,
    process_lock: Mutex<()>,
}

impl JoinRequestStore {
    pub fn new(
        path: impl Into<PathBuf>,
        network_record_path: Option<PathBuf>,
        owner_node_id: Option<String>,
        administrator_node_ids: Vec<String>,
    ) -> Result<Self> {
        let fallback_owner = owner_node_id
            .map(NodeId::parse)
            .transpose()
            .context("协调器显式 owner_node_id 无效")?;
        let fallback_administrators = administrator_node_ids
            .into_iter()
            .map(NodeId::parse)
            .collect::<std::result::Result<BTreeSet<_>, _>>()
            .context("协调器显式 administrator_node_ids 无效")?;
        if fallback_owner
            .as_ref()
            .is_some_and(|owner| fallback_administrators.contains(owner))
        {
            bail!("协调器 owner 不能同时列为管理员");
        }
        Ok(Self {
            path: path.into(),
            network_record_path,
            fallback_owner,
            fallback_administrators,
            process_lock: Mutex::new(()),
        })
    }

    pub fn submit(
        &self,
        expected_network_id: &NetworkId,
        applicant: &NodeId,
        authenticated_public_key: &[u8],
        request: JoinRequest,
        now_unix_ms: u64,
    ) -> Result<JoinRequestReviewResult> {
        request.validate()?;
        if request.network_id != *expected_network_id || request.applicant.node_id != *applicant {
            bail!("join request identity does not match the authenticated submission");
        }
        let expected_fingerprint = Identity::public_key_fingerprint(authenticated_public_key);
        if request.applicant.public_key_fingerprint != expected_fingerprint {
            bail!("join request public key fingerprint does not match the authenticated identity");
        }

        let network_record = self.load_network_record_locked(expected_network_id)?;
        if network_record.join_mode != NetworkJoinMode::ApprovalRequired {
            bail!("network is not accepting approval requests");
        }
        if network_record.members.contains(applicant) {
            bail!("applicant is already a network member");
        }
        let mut request = request;
        // The applicant may describe what it wants, but only the current
        // network record can decide the durable terms and granted authority.
        request.terms = network_record.membership_terms();
        request.grant_preview = network_record.approval_grant_preview();
        request.validate()?;

        self.transaction(|file, _| {
            file.expire_and_scrub(now_unix_ms)?;
            if file.accepted_members.contains(applicant) {
                bail!("applicant is already a network member");
            }
            if let Some(existing) = file.requests.get(&request.request_id) {
                if existing.applicant.node_id != *applicant {
                    bail!("join request id collision");
                }
                return Ok(JoinRequestReviewResult {
                    request_id: existing.request_id.clone(),
                    state: existing.state,
                    revision: existing.revision,
                });
            }
            if file.requests.values().any(|existing| {
                existing.applicant.node_id == *applicant && !existing.state.is_terminal()
            }) {
                bail!("applicant already has an active join request");
            }
            let result = JoinRequestReviewResult {
                request_id: request.request_id.clone(),
                state: request.state,
                revision: request.revision,
            };
            file.requests.insert(request.request_id.clone(), request);
            Ok(result)
        })
    }

    /// Reviewers see every request for the network. Other authenticated nodes
    /// only see their own request; no unfiltered list is ever returned.
    pub fn list(
        &self,
        network_id: &NetworkId,
        actor: &NodeId,
        now_unix_ms: u64,
    ) -> Result<Vec<JoinRequest>> {
        self.repair_roster_projection(network_id)?;
        let roster = self.load_roster(network_id)?;
        let reviewer = roster.is_reviewer(actor);
        self.transaction(|file, _| {
            file.expire_and_scrub(now_unix_ms)?;
            let mut requests = file
                .requests
                .values()
                .filter(|request| {
                    request.network_id == *network_id
                        && (reviewer || request.applicant.node_id == *actor)
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

    pub fn review(
        &self,
        network_id: &NetworkId,
        reviewer: &NodeId,
        reviews: Vec<JoinRequestBatchReviewItem>,
        now_unix_ms: u64,
    ) -> Result<Vec<JoinRequestReviewResult>> {
        if reviews.is_empty() {
            bail!("batch review requires at least one request");
        }
        if reviews.len() > 100 {
            bail!("batch review exceeds 100 requests");
        }
        self.repair_roster_projection(network_id)?;
        let mut seen = BTreeSet::new();
        if reviews
            .iter()
            .any(|review| !seen.insert(review.request_id.clone()))
        {
            bail!("batch review contains duplicate request ids");
        }

        // Approval changes two durable files: the request ledger and the
        // authoritative network roster. The same profile lock used by profile
        // edits is held across both writes so a concurrent profile update cannot
        // race the roster snapshot.
        let record_path = self
            .network_record_path
            .as_ref()
            .context("协调器未配置可信 network_record_path，审批操作失败关闭")?;
        let profile_lock = lock_profile(record_path)?;
        let result = (|| {
            let mut network_record = read_network_record(record_path, network_id)?;
            if !network_record.owner.eq(reviewer)
                && !network_record
                    .administrators
                    .iter()
                    .any(|admin| admin == reviewer)
            {
                bail!("actor is not a current owner or administrator");
            }

            self.with_file_lock(|file| {
                let old_request_bytes = std::fs::read(&self.path).ok();
                let mut candidate = file.clone();
                candidate.expire_and_scrub(now_unix_ms)?;
                let mut results = Vec::with_capacity(reviews.len());
                let mut admitted = BTreeSet::new();
                for review in &reviews {
                    let request = candidate
                        .requests
                        .get_mut(&review.request_id)
                        .with_context(|| {
                            format!("join request not found: {}", review.request_id)
                        })?;
                    if request.network_id != *network_id {
                        bail!("join request belongs to another network");
                    }
                    if request.applicant.node_id == *reviewer {
                        bail!("join_request.error.self_review_forbidden");
                    }
                    request.review(
                        review.expected_revision,
                        review.action,
                        reviewer.clone(),
                        review.note.clone(),
                        now_unix_ms,
                    )?;
                    if request.state == JoinRequestState::Approved {
                        let applicant = request.applicant.node_id.clone();
                        if candidate.accepted_members.contains(&applicant) {
                            bail!("applicant already has a final accepted member state");
                        }
                        candidate.accepted_members.insert(applicant.clone());
                        admitted.insert(applicant);
                    }
                    results.push(JoinRequestReviewResult {
                        request_id: request.request_id.clone(),
                        state: request.state,
                        revision: request.revision,
                    });
                }

                // Validate the complete candidate before either file is
                // touched, including the request/roster projection.
                candidate.validate()?;
                for applicant in &admitted {
                    network_record.admit_member(applicant.clone(), now_unix_ms)?;
                }

                // Persist the intent first. If the roster write fails, restore
                // the old request file. A process crash between the two writes
                // is repaired from the approved request projection on the next
                // read/review transaction.
                self.persist(&candidate)?;
                if !admitted.is_empty() {
                    if let Err(error) = persist_network_record(record_path, &network_record) {
                        let restore =
                            restore_request_file(&self.path, old_request_bytes.as_deref());
                        return match restore {
                            Ok(()) => Err(error),
                            Err(restore_error) => Err(anyhow::anyhow!(
                                "{error:#}; 回滚审批请求失败: {restore_error:#}"
                            )),
                        };
                    }
                }
                Ok(results)
            })
        })();
        let _ = FileExt::unlock(&profile_lock);
        result
    }

    pub fn resubmit(
        &self,
        network_id: &NetworkId,
        applicant: &NodeId,
        request_id: &JoinRequestId,
        expected_revision: u64,
        reason: String,
        now_unix_ms: u64,
    ) -> Result<JoinRequestReviewResult> {
        self.transaction(|file, _| {
            file.expire_and_scrub(now_unix_ms)?;
            let request = file
                .requests
                .get_mut(request_id)
                .with_context(|| format!("join request not found: {}", request_id))?;
            if request.network_id != *network_id {
                bail!("join request belongs to another network");
            }
            if request.applicant.node_id != *applicant {
                bail!("only the applicant can resubmit this request");
            }
            request.resubmit(expected_revision, reason, now_unix_ms)?;
            let result = JoinRequestReviewResult {
                request_id: request.request_id.clone(),
                state: request.state,
                revision: request.revision,
            };
            file.validate()?;
            Ok(result)
        })
    }

    pub fn withdraw(
        &self,
        network_id: &NetworkId,
        applicant: &NodeId,
        request_id: &JoinRequestId,
        now_unix_ms: u64,
    ) -> Result<JoinRequestReviewResult> {
        self.transaction(|file, _| {
            file.expire_and_scrub(now_unix_ms)?;
            let request = file
                .requests
                .get_mut(request_id)
                .with_context(|| format!("join request not found: {}", request_id))?;
            if request.network_id != *network_id {
                bail!("join request belongs to another network");
            }
            request.withdraw(applicant, now_unix_ms)?;
            let result = JoinRequestReviewResult {
                request_id: request.request_id.clone(),
                state: request.state,
                revision: request.revision,
            };
            file.validate()?;
            Ok(result)
        })
    }

    /// Online identities that may receive a changed notification for one
    /// request. The applicant is always included; reviewer identities come
    /// from the same live roster used to authorize the action.
    pub fn notification_recipients(
        &self,
        network_id: &NetworkId,
        applicant: &NodeId,
    ) -> Result<Vec<NodeId>> {
        let roster = self.load_roster(network_id)?;
        let mut recipients = BTreeSet::from([applicant.clone()]);
        recipients.insert(roster.owner);
        recipients.extend(roster.administrators);
        Ok(recipients.into_iter().collect())
    }

    pub fn status(
        &self,
        network_id: &NetworkId,
        actor: &NodeId,
        request_id: &JoinRequestId,
        now_unix_ms: u64,
    ) -> Result<Option<JoinRequest>> {
        self.repair_roster_projection(network_id)?;
        let roster = self.load_roster(network_id)?;
        let reviewer = roster.is_reviewer(actor);
        self.transaction(|file, _| {
            file.expire_and_scrub(now_unix_ms)?;
            Ok(file.requests.get(request_id).and_then(|request| {
                (request.network_id == *network_id
                    && (reviewer || request.applicant.node_id == *actor))
                    .then(|| request.clone())
            }))
        })
    }

    /// Complete the one possible crash window between persisting an approved
    /// request ledger and persisting the authoritative roster. The approved
    /// request is the durable intent, so startup/read recovery adds the exact
    /// missing member IDs once and atomically rewrites the network record.
    fn repair_roster_projection(&self, expected_network_id: &NetworkId) -> Result<()> {
        let Some(path) = &self.network_record_path else {
            return Ok(());
        };
        let lock = lock_profile(path)?;
        let result = (|| {
            let mut record = read_network_record(path, expected_network_id)?;
            let changed = self.with_file_lock(|file| {
                let mut changed = false;
                for request in file.requests.values().filter(|request| {
                    request.network_id == *expected_network_id
                        && request.state == JoinRequestState::Approved
                }) {
                    if !record.members.contains(&request.applicant.node_id) {
                        record.members.push(request.applicant.node_id.clone());
                        changed = true;
                    }
                }
                if changed {
                    record.updated_at_unix_ms = record.updated_at_unix_ms.max(unix_time_ms());
                    record.validate()?;
                    persist_network_record(path, &record)?;
                }
                Ok(changed)
            })?;
            let _ = changed;
            Ok(())
        })();
        let _ = FileExt::unlock(&lock);
        result
    }

    fn load_network_record_locked(&self, expected_network_id: &NetworkId) -> Result<NetworkRecord> {
        let path = self
            .network_record_path
            .as_ref()
            .context("协调器未配置可信 network_record_path，审批操作失败关闭")?;
        let lock = lock_profile_shared(path)?;
        let result = read_network_record(path, expected_network_id);
        let _ = FileExt::unlock(&lock);
        result
    }

    fn load_roster(&self, expected_network_id: &NetworkId) -> Result<NetworkRoster> {
        if let Some(path) = &self.network_record_path {
            let lock = lock_profile_shared(path)?;
            let record = read_network_record(path, expected_network_id)?;
            let _ = FileExt::unlock(&lock);
            return Ok(NetworkRoster {
                owner: record.owner,
                administrators: record.administrators.into_iter().collect(),
            });
        }
        let owner = self
            .fallback_owner
            .clone()
            .context("协调器未配置可信 owner roster，审批操作失败关闭")?;
        Ok(NetworkRoster {
            owner,
            administrators: self.fallback_administrators.clone(),
        })
    }

    fn transaction<T>(
        &self,
        mutate: impl FnOnce(&mut JoinRequestFile, u64) -> Result<T>,
    ) -> Result<T> {
        self.with_file_lock(|file| {
            let mut candidate = file.clone();
            let result = mutate(&mut candidate, unix_time_ms())?;
            candidate.validate()?;
            self.persist(&candidate)?;
            Ok(result)
        })
    }

    fn with_file_lock<T>(&self, action: impl FnOnce(&JoinRequestFile) -> Result<T>) -> Result<T> {
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("审批请求存储锁已损坏"))?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("无法创建审批请求存储目录: {}", parent.display()))?;
        }
        let lock_path = self.path.with_extension("json.lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("无法打开审批请求锁: {}", lock_path.display()))?;
        lock.lock_exclusive()
            .with_context(|| format!("无法锁定审批请求存储: {}", lock_path.display()))?;
        let result = self.load_unlocked().and_then(|file| action(&file));
        let _ = FileExt::unlock(&lock);
        result
    }

    fn load_unlocked(&self) -> Result<JoinRequestFile> {
        if !self.path.exists() {
            return Ok(JoinRequestFile::default());
        }
        let content = std::fs::read_to_string(&self.path)
            .with_context(|| format!("无法读取审批请求存储: {}", self.path.display()))?;
        let file: JoinRequestFile = serde_json::from_str(&content)
            .with_context(|| format!("无法解析审批请求存储: {}", self.path.display()))?;
        file.validate()?;
        Ok(file)
    }

    fn persist(&self, file: &JoinRequestFile) -> Result<()> {
        file.validate()?;
        let content = serde_json::to_vec_pretty(file).context("无法序列化审批请求存储")?;
        let temporary = self
            .path
            .with_extension(format!("json.tmp-{}", std::process::id()));
        {
            let mut output = File::create(&temporary)
                .with_context(|| format!("无法创建审批请求临时文件: {}", temporary.display()))?;
            output
                .write_all(&content)
                .with_context(|| format!("无法写入审批请求临时文件: {}", temporary.display()))?;
            output
                .sync_all()
                .with_context(|| format!("无法同步审批请求临时文件: {}", temporary.display()))?;
        }
        if let Err(error) = std::fs::rename(&temporary, &self.path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error)
                .with_context(|| format!("无法替换审批请求存储: {}", self.path.display()));
        }
        if let Some(parent) = self.path.parent() {
            let directory = File::open(parent)
                .with_context(|| format!("无法打开审批请求目录: {}", parent.display()))?;
            directory
                .sync_all()
                .with_context(|| format!("无法同步审批请求目录: {}", parent.display()))?;
        }
        Ok(())
    }
}

fn lock_profile(path: &std::path::Path) -> Result<File> {
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
    lock.lock_exclusive()
        .with_context(|| format!("无法锁定网络资料: {}", lock_path.display()))?;
    Ok(lock)
}

fn lock_profile_shared(path: &std::path::Path) -> Result<File> {
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

fn read_network_record(
    path: &std::path::Path,
    expected_network_id: &NetworkId,
) -> Result<NetworkRecord> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("无法读取审批授权网络资料: {}", path.display()))?;
    let record: NetworkRecord = serde_json::from_str(&content)
        .with_context(|| format!("无法解析审批授权网络资料: {}", path.display()))?;
    record.validate().context("审批授权网络资料校验失败")?;
    if record.network_id != *expected_network_id {
        bail!("approval request network does not match owner network record");
    }
    Ok(record)
}

fn persist_network_record(path: &std::path::Path, record: &NetworkRecord) -> Result<()> {
    record.validate()?;
    let content = serde_json::to_vec_pretty(record).context("无法序列化审批授权网络资料")?;
    persist_bytes_atomic(path, &content)
}

fn restore_request_file(path: &std::path::Path, bytes: Option<&[u8]>) -> Result<()> {
    match bytes {
        Some(bytes) => persist_bytes_atomic(path, bytes),
        None => std::fs::remove_file(path)
            .with_context(|| format!("无法回滚审批请求存储: {}", path.display())),
    }
}

fn persist_bytes_atomic(path: &std::path::Path, content: &[u8]) -> Result<()> {
    let parent = path.parent().context("审批授权网络资料没有父目录")?;
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

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuckipv6_common::domain::{
        CapabilityKind, ExitContributionObligation, JoinApplicantProfile, JoinRequestGrantPreview,
        JoinRequestReviewAction, MembershipRole, MembershipTerms, NetworkCreateProfile,
        NetworkVisibility,
    };
    use std::path::Path;
    use std::sync::{Arc, Barrier};

    fn test_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "fuckipv6-join-requests-{}-{label}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn roster_public_key() -> fuckipv6_common::domain::PublicKey {
        fuckipv6_common::domain::PublicKey::from_bytes(*b"01234567890123456789012345678901")
    }

    fn network_id() -> NetworkId {
        NetworkId::from_public_key(roster_public_key().as_str().as_bytes())
    }

    fn owner() -> NodeId {
        NodeId::from_public_key(b"owner")
    }

    fn admin() -> NodeId {
        NodeId::from_public_key(b"admin")
    }

    fn applicant(seed: &str) -> NodeId {
        NodeId::from_public_key(seed.as_bytes())
    }

    fn network_record(owner: NodeId, administrators: Vec<NodeId>) -> NetworkRecord {
        let mut record = NetworkRecord::create_owned(
            NetworkCreateProfile {
                name: "Approval".into(),
                purpose: "Approval network".into(),
                icon: "network".into(),
                language: "zh-CN".into(),
                region: "ZZ".into(),
                tags: vec![],
                capability_declarations: vec![CapabilityKind::File],
                visibility: NetworkVisibility::Private,
                join_mode: NetworkJoinMode::ApprovalRequired,
            },
            roster_public_key(),
            owner.clone(),
            1_000,
        )
        .unwrap();
        record.members.extend(administrators.clone());
        record.administrators = administrators;
        record
    }

    fn write_network_record(dir: &Path, record: &NetworkRecord) -> PathBuf {
        let path = dir.join("network.json");
        persist_network_record(&path, record).unwrap();
        path
    }

    fn request(applicant_seed: &str, nonce: &str, now: u64) -> JoinRequest {
        let applicant_id = applicant(applicant_seed);
        JoinRequest::new(
            JoinRequest::derived_id(&network_id(), &applicant_id, nonce),
            network_id(),
            JoinApplicantProfile {
                node_id: applicant_id,
                display_name: Some("Applicant".into()),
                public_key_fingerprint: Identity::public_key_fingerprint(applicant_seed.as_bytes()),
            },
            "Please allow this node to join for file collaboration.",
            MembershipTerms {
                network_policy_revision: 1,
                privacy_policy_revision: 1,
                exit_contribution: ExitContributionObligation::NotRequired,
                member_limit: Some(10),
            },
            JoinRequestGrantPreview {
                role: MembershipRole::Member,
                capabilities: vec![CapabilityKind::File],
                exit_contribution: ExitContributionObligation::NotRequired,
                file_access: true,
                grants_data_plane_access: false,
            },
            now,
            now + 60_000,
        )
        .unwrap()
    }

    fn test_store(dir: &Path) -> JoinRequestStore {
        let record = network_record(owner(), vec![admin()]);
        write_network_record(dir, &record);
        JoinRequestStore::new(
            dir.join("join_requests.json"),
            Some(dir.join("network.json")),
            None,
            vec![],
        )
        .unwrap()
    }

    #[test]
    fn submission_and_review_update_the_authoritative_roster_atomically() {
        let dir = test_dir("review");
        let store = test_store(&dir);
        let applicant_seed = "applicant";
        let applicant_id = applicant(applicant_seed);
        let request = request(applicant_seed, "nonce", 1_000);
        store
            .submit(
                &network_id(),
                &applicant_id,
                applicant_seed.as_bytes(),
                request.clone(),
                1_001,
            )
            .unwrap();

        let results = store
            .review(
                &network_id(),
                &admin(),
                vec![JoinRequestBatchReviewItem {
                    request_id: request.request_id.clone(),
                    expected_revision: 1,
                    action: JoinRequestReviewAction::Approve,
                    note: None,
                }],
                1_002,
            )
            .unwrap();
        assert_eq!(results[0].state, JoinRequestState::Approved);

        let roster = read_network_record(&dir.join("network.json"), &network_id()).unwrap();
        assert!(roster.members.contains(&applicant_id));
        let reopened = JoinRequestStore::new(
            dir.join("join_requests.json"),
            Some(dir.join("network.json")),
            None,
            vec![],
        )
        .unwrap();
        let listed = reopened.list(&network_id(), &owner(), 1_003).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].state, JoinRequestState::Approved);
    }

    #[test]
    fn stale_batch_review_rolls_back_every_request_and_the_roster() {
        let dir = test_dir("rollback");
        let store = test_store(&dir);
        let first_seed = "first";
        let second_seed = "second";
        let first_applicant = applicant(first_seed);
        let second_applicant = applicant(second_seed);
        let first = request(first_seed, "first", 1_000);
        let second = request(second_seed, "second", 1_000);
        store
            .submit(
                &network_id(),
                &first_applicant,
                first_seed.as_bytes(),
                first.clone(),
                1_001,
            )
            .unwrap();
        store
            .submit(
                &network_id(),
                &second_applicant,
                second_seed.as_bytes(),
                second.clone(),
                1_001,
            )
            .unwrap();

        let result = store.review(
            &network_id(),
            &owner(),
            vec![
                JoinRequestBatchReviewItem {
                    request_id: first.request_id.clone(),
                    expected_revision: 1,
                    action: JoinRequestReviewAction::Approve,
                    note: None,
                },
                JoinRequestBatchReviewItem {
                    request_id: second.request_id.clone(),
                    expected_revision: 99,
                    action: JoinRequestReviewAction::Approve,
                    note: None,
                },
            ],
            1_002,
        );
        assert!(result.is_err());
        let listed = store.list(&network_id(), &owner(), 1_003).unwrap();
        assert!(listed
            .iter()
            .all(|request| request.state == JoinRequestState::Pending));
        let roster = read_network_record(&dir.join("network.json"), &network_id()).unwrap();
        assert!(!roster.members.contains(&first_applicant));
        assert!(!roster.members.contains(&second_applicant));
    }

    #[test]
    fn concurrent_approval_produces_one_final_member_state() {
        let dir = test_dir("concurrent");
        let store = Arc::new(test_store(&dir));
        let applicant_seed = "concurrent-applicant";
        let applicant_id = applicant(applicant_seed);
        let request = request(applicant_seed, "nonce", 1_000);
        store
            .submit(
                &network_id(),
                &applicant_id,
                applicant_seed.as_bytes(),
                request.clone(),
                1_001,
            )
            .unwrap();

        let barrier = Arc::new(Barrier::new(2));
        let handles = (0..2)
            .map(|_| {
                let store = store.clone();
                let barrier = barrier.clone();
                let request_id = request.request_id.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    store.review(
                        &network_id(),
                        &admin(),
                        vec![JoinRequestBatchReviewItem {
                            request_id,
                            expected_revision: 1,
                            action: JoinRequestReviewAction::Approve,
                            note: None,
                        }],
                        1_002,
                    )
                })
            })
            .collect::<Vec<_>>();
        let successes = handles
            .into_iter()
            .map(|handle| handle.join().expect("approval worker panicked"))
            .filter(std::result::Result::is_ok)
            .count();
        assert_eq!(successes, 1);
        let roster = read_network_record(&dir.join("network.json"), &network_id()).unwrap();
        assert_eq!(
            roster
                .members
                .iter()
                .filter(|member| **member == applicant_id)
                .count(),
            1
        );
    }

    #[test]
    fn non_reviewer_only_sees_own_request_and_revoked_admin_loses_access() {
        let dir = test_dir("privacy");
        let store = test_store(&dir);
        let applicant_seed = "privacy-applicant";
        let applicant_id = applicant(applicant_seed);
        let request = request(applicant_seed, "nonce", 1_000);
        store
            .submit(
                &network_id(),
                &applicant_id,
                applicant_seed.as_bytes(),
                request.clone(),
                1_001,
            )
            .unwrap();

        let outsider_seed = "outsider";
        let outsider = applicant(outsider_seed);
        assert!(store
            .list(&network_id(), &outsider, 1_002)
            .unwrap()
            .is_empty());
        assert_eq!(
            store
                .list(&network_id(), &applicant_id, 1_002)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(store.list(&network_id(), &admin(), 1_002).unwrap().len(), 1);

        // Demoting an administrator takes effect immediately because review
        // reloads the roster under the profile lock.
        write_network_record(&dir, &network_record(owner(), vec![]));
        let result = store.review(
            &network_id(),
            &admin(),
            vec![JoinRequestBatchReviewItem {
                request_id: request.request_id,
                expected_revision: 1,
                action: JoinRequestReviewAction::Approve,
                note: None,
            }],
            1_003,
        );
        assert!(result.is_err());
    }

    #[test]
    fn expiry_withdrawal_and_admin_revocation_keep_state_and_roster_consistent() {
        let dir = test_dir("terminal-lifecycle");
        let store = test_store(&dir);
        let applicant_seed = "lifecycle-applicant";
        let applicant_id = applicant(applicant_seed);

        let expiring = request(applicant_seed, "expiring", 1_000);
        store
            .submit(
                &network_id(),
                &applicant_id,
                applicant_seed.as_bytes(),
                expiring.clone(),
                1_001,
            )
            .unwrap();
        let listed = store
            .list(&network_id(), &owner(), expiring.expires_at_unix_ms)
            .unwrap();
        assert_eq!(listed[0].state, JoinRequestState::Expired);
        assert_eq!(listed[0].revision, 2);
        let late_review = store.review(
            &network_id(),
            &owner(),
            vec![JoinRequestBatchReviewItem {
                request_id: expiring.request_id.clone(),
                expected_revision: listed[0].revision,
                action: JoinRequestReviewAction::Approve,
                note: None,
            }],
            expiring.expires_at_unix_ms,
        );
        assert!(late_review.is_err());
        let roster = read_network_record(&dir.join("network.json"), &network_id()).unwrap();
        assert!(!roster.members.contains(&applicant_id));

        let withdrawn = request(applicant_seed, "withdrawn", expiring.expires_at_unix_ms + 1);
        store
            .submit(
                &network_id(),
                &applicant_id,
                applicant_seed.as_bytes(),
                withdrawn.clone(),
                expiring.expires_at_unix_ms + 2,
            )
            .unwrap();
        let result = store
            .withdraw(
                &network_id(),
                &applicant_id,
                &withdrawn.request_id,
                expiring.expires_at_unix_ms + 3,
            )
            .unwrap();
        assert_eq!(result.state, JoinRequestState::Withdrawn);
        assert_eq!(result.revision, 2);
        let roster = read_network_record(&dir.join("network.json"), &network_id()).unwrap();
        assert!(!roster.members.contains(&applicant_id));

        let pending = request(
            applicant_seed,
            "revoked-admin",
            expiring.expires_at_unix_ms + 4,
        );
        store
            .submit(
                &network_id(),
                &applicant_id,
                applicant_seed.as_bytes(),
                pending.clone(),
                expiring.expires_at_unix_ms + 5,
            )
            .unwrap();
        let admin_view = store
            .list(&network_id(), &admin(), expiring.expires_at_unix_ms + 6)
            .unwrap();
        assert!(admin_view
            .iter()
            .any(|request| request.request_id == pending.request_id));
        let recipients = store
            .notification_recipients(&network_id(), &applicant_id)
            .unwrap();
        assert_eq!(recipients.len(), 3);
        assert!(recipients.contains(&applicant_id));
        assert!(recipients.contains(&owner()));
        assert!(recipients.contains(&admin()));
        assert!(!recipients.contains(&applicant("unrelated-member")));

        write_network_record(&dir, &network_record(owner(), vec![]));
        assert!(store
            .list(&network_id(), &admin(), expiring.expires_at_unix_ms + 7)
            .unwrap()
            .is_empty());
        let recipients = store
            .notification_recipients(&network_id(), &applicant_id)
            .unwrap();
        assert!(!recipients.contains(&admin()));
        assert!(recipients.contains(&owner()));

        let revoked_review = store.review(
            &network_id(),
            &admin(),
            vec![JoinRequestBatchReviewItem {
                request_id: pending.request_id.clone(),
                expected_revision: 1,
                action: JoinRequestReviewAction::Approve,
                note: None,
            }],
            expiring.expires_at_unix_ms + 8,
        );
        assert!(revoked_review.is_err());
        let listed = store
            .list(&network_id(), &owner(), expiring.expires_at_unix_ms + 9)
            .unwrap();
        assert_eq!(listed[0].state, JoinRequestState::Pending);
        assert_eq!(listed[0].revision, 1);

        let approved = store
            .review(
                &network_id(),
                &owner(),
                vec![JoinRequestBatchReviewItem {
                    request_id: pending.request_id,
                    expected_revision: 1,
                    action: JoinRequestReviewAction::Approve,
                    note: None,
                }],
                expiring.expires_at_unix_ms + 10,
            )
            .unwrap();
        assert_eq!(approved[0].state, JoinRequestState::Approved);
        let roster = read_network_record(&dir.join("network.json"), &network_id()).unwrap();
        assert_eq!(
            roster
                .members
                .iter()
                .filter(|member| **member == applicant_id)
                .count(),
            1
        );
    }

    #[test]
    fn reason_visibility_and_retention_are_enforced_in_the_durable_store() {
        let dir = test_dir("reason-retention");
        let store = test_store(&dir);
        let applicant_seed = "private-reason-applicant";
        let applicant_id = applicant(applicant_seed);
        let request = request(applicant_seed, "private-reason", 1_000);
        let private_reason = request.reason.clone().unwrap();
        store
            .submit(
                &network_id(),
                &applicant_id,
                applicant_seed.as_bytes(),
                request.clone(),
                1_001,
            )
            .unwrap();

        let applicant_view = store
            .status(&network_id(), &applicant_id, &request.request_id, 1_002)
            .unwrap()
            .unwrap();
        assert_eq!(
            applicant_view.reason.as_deref(),
            Some(private_reason.as_str())
        );
        for reviewer in [owner(), admin()] {
            let reviewer_view = store
                .status(&network_id(), &reviewer, &request.request_id, 1_002)
                .unwrap()
                .unwrap();
            assert_eq!(
                reviewer_view.reason.as_deref(),
                Some(private_reason.as_str())
            );
        }
        let outsider = applicant("reason-outsider");
        assert!(store
            .status(&network_id(), &outsider, &request.request_id, 1_002)
            .unwrap()
            .is_none());
        assert!(store
            .list(&network_id(), &outsider, 1_002)
            .unwrap()
            .is_empty());

        let decided_at = 2_000;
        store
            .review(
                &network_id(),
                &owner(),
                vec![JoinRequestBatchReviewItem {
                    request_id: request.request_id.clone(),
                    expected_revision: 1,
                    action: JoinRequestReviewAction::Reject,
                    note: Some("Private reviewer note".into()),
                }],
                decided_at,
            )
            .unwrap();
        let deadline = decided_at + fuckipv6_common::domain::JOIN_REQUEST_REASON_RETENTION_MS;
        let retained = store
            .status(&network_id(), &owner(), &request.request_id, deadline - 1)
            .unwrap()
            .unwrap();
        assert_eq!(retained.reason.as_deref(), Some(private_reason.as_str()));
        let durable_before = std::fs::read_to_string(dir.join("join_requests.json")).unwrap();
        assert!(durable_before.contains(&private_reason));

        let scrubbed = store
            .status(&network_id(), &applicant_id, &request.request_id, deadline)
            .unwrap()
            .unwrap();
        assert!(scrubbed.reason.is_none());
        assert!(scrubbed.information_request.is_none());
        let durable_after = std::fs::read_to_string(dir.join("join_requests.json")).unwrap();
        assert!(!durable_after.contains(&private_reason));
        assert!(!durable_after.contains("Private reviewer note"));
        let reopened = JoinRequestStore::new(
            dir.join("join_requests.json"),
            Some(dir.join("network.json")),
            None,
            vec![],
        )
        .unwrap();
        let listed = reopened
            .list(&network_id(), &owner(), deadline + 1)
            .unwrap();
        assert!(listed[0].reason.is_none());
    }

    #[test]
    fn reviewer_cannot_approve_their_own_request() {
        let dir = test_dir("self-review");
        let applicant_seed = "self-reviewer";
        let applicant_id = applicant(applicant_seed);
        let record_path = write_network_record(&dir, &network_record(owner(), vec![]));
        let store = JoinRequestStore::new(
            dir.join("join_requests.json"),
            Some(record_path.clone()),
            None,
            vec![],
        )
        .unwrap();
        let request = request(applicant_seed, "self-review", 1_000);
        store
            .submit(
                &network_id(),
                &applicant_id,
                applicant_seed.as_bytes(),
                request.clone(),
                1_001,
            )
            .unwrap();

        // Simulate the applicant gaining reviewer authority after submitting.
        // The explicit self-review check must still reject the decision.
        write_network_record(&dir, &network_record(owner(), vec![applicant_id.clone()]));

        let error = store
            .review(
                &network_id(),
                &applicant_id,
                vec![JoinRequestBatchReviewItem {
                    request_id: request.request_id.clone(),
                    expected_revision: 1,
                    action: JoinRequestReviewAction::Approve,
                    note: None,
                }],
                1_002,
            )
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "join_request.error.self_review_forbidden"
        );
        let listed = store.list(&network_id(), &owner(), 1_003).unwrap();
        assert_eq!(listed[0].state, JoinRequestState::Pending);
        assert_eq!(listed[0].revision, 1);
    }

    #[test]
    fn submission_rejects_a_forged_public_key_fingerprint() {
        let dir = test_dir("fingerprint");
        let store = test_store(&dir);
        let applicant_seed = "fingerprint-applicant";
        let applicant_id = applicant(applicant_seed);
        let mut request = request(applicant_seed, "nonce", 1_000);
        request.applicant.public_key_fingerprint = "forged".into();
        let error = store
            .submit(
                &network_id(),
                &applicant_id,
                applicant_seed.as_bytes(),
                request,
                1_001,
            )
            .unwrap_err();
        assert!(error.to_string().contains("fingerprint"));
    }

    #[test]
    fn submission_replaces_client_grant_with_server_policy() {
        let dir = test_dir("grant");
        let store = test_store(&dir);
        let applicant_seed = "grant-applicant";
        let applicant_id = applicant(applicant_seed);
        let mut request = request(applicant_seed, "nonce", 1_000);
        request.grant_preview.capabilities.clear();
        request.grant_preview.file_access = false;
        request.grant_preview.grants_data_plane_access = false;
        // Temporarily make the check pass so the store can apply policy.
        request.validate().unwrap();
        store
            .submit(
                &network_id(),
                &applicant_id,
                applicant_seed.as_bytes(),
                request.clone(),
                1_001,
            )
            .unwrap();
        let stored = store
            .status(&network_id(), &applicant_id, &request.request_id, 1_002)
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.grant_preview.capabilities,
            vec![CapabilityKind::File]
        );
        assert!(stored.grant_preview.file_access);
        assert!(stored.grant_preview.grants_data_plane_access);
        assert_eq!(
            stored.terms,
            network_record(owner(), vec![admin()]).membership_terms()
        );
    }

    #[test]
    fn approval_repairs_a_crash_window_where_request_precedes_roster() {
        let dir = test_dir("repair");
        let store = test_store(&dir);
        let applicant_seed = "repair-applicant";
        let applicant_id = applicant(applicant_seed);
        let request = request(applicant_seed, "nonce", 1_000);
        store
            .submit(
                &network_id(),
                &applicant_id,
                applicant_seed.as_bytes(),
                request.clone(),
                1_001,
            )
            .unwrap();
        store
            .review(
                &network_id(),
                &owner(),
                vec![JoinRequestBatchReviewItem {
                    request_id: request.request_id.clone(),
                    expected_revision: 1,
                    action: JoinRequestReviewAction::Approve,
                    note: None,
                }],
                1_002,
            )
            .unwrap();

        // Simulate a crash after the request ledger reached disk but before
        // the roster write by removing the recovered applicant from network.json.
        let path = dir.join("network.json");
        let mut record = read_network_record(&path, &network_id()).unwrap();
        record.members.retain(|member| *member != applicant_id);
        persist_network_record(&path, &record).unwrap();

        store.list(&network_id(), &owner(), 1_003).unwrap();
        let repaired = read_network_record(&path, &network_id()).unwrap();
        assert!(repaired.members.contains(&applicant_id));
    }
}
