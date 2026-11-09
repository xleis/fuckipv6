use anyhow::{bail, Context, Result};
use fs2::FileExt;
use fuckipv6_common::domain::{
    CapabilityKind, ContentHash, InvitationCode, InvitationCreateRequest, InvitationId,
    InvitationPreview, InvitationRecord, InvitationRedeemRequest, InvitationRedemption,
    InvitationState, InvitationStatusView, IssuedInvitation, MembershipRole, NetworkId,
    NetworkJoinMode, NetworkRecord, NodeId,
};
use serde::{Deserialize, Serialize};

use super::member_lifecycle::MemberLifecycleStore;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const INVITATION_FILE_SCHEMA_VERSION: u16 = 1;
const INVITATION_TERMS_ACK_WINDOW_MS: u64 = 15 * 60 * 1000;
const INVITATION_ACK_FUTURE_SKEW_MS: u64 = 60 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredInvitation {
    digest: ContentHash,
    record: InvitationRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InvitationFile {
    schema_version: u16,
    #[serde(default)]
    invitations: BTreeMap<InvitationId, StoredInvitation>,
    #[serde(default)]
    redemptions: BTreeMap<InvitationId, BTreeMap<NodeId, InvitationRedemption>>,
}

impl Default for InvitationFile {
    fn default() -> Self {
        Self {
            schema_version: INVITATION_FILE_SCHEMA_VERSION,
            invitations: BTreeMap::new(),
            redemptions: BTreeMap::new(),
        }
    }
}

impl InvitationFile {
    fn validate(&self) -> Result<()> {
        if self.schema_version != INVITATION_FILE_SCHEMA_VERSION {
            bail!("unsupported invitation file schema version");
        }
        let mut digests = BTreeSet::new();
        for (invitation_id, stored) in &self.invitations {
            if stored.record.invitation_id != *invitation_id {
                bail!("invitation map key does not match invitation id");
            }
            stored.record.validate()?;
            if !digests.insert(stored.digest.clone()) {
                bail!("invitation digest is duplicated");
            }
            let redeemed = self
                .redemptions
                .get(invitation_id)
                .map(BTreeMap::len)
                .unwrap_or_default() as u32;
            if stored.record.used_count != redeemed {
                bail!("invitation use count does not match its redemption ledger");
            }
        }
        for (invitation_id, redemptions) in &self.redemptions {
            let stored = self
                .invitations
                .get(invitation_id)
                .context("redemption references an unknown invitation")?;
            for (node_id, redemption) in redemptions {
                if redemption.invitation_id != *invitation_id
                    || redemption.node_id != *node_id
                    || redemption.network_id != stored.record.network_id
                {
                    bail!("invitation redemption identity is inconsistent");
                }
                if redemption.grants_permanent_membership {
                    bail!("invitation redemption cannot grant permanent membership");
                }
            }
        }
        Ok(())
    }

    fn stored_by_code(&self, code: &InvitationCode) -> Option<(&InvitationId, &StoredInvitation)> {
        let digest = code.digest();
        self.invitations
            .iter()
            .find(|(_, stored)| stored.digest == digest)
    }
}

#[derive(Debug, Clone)]
struct NetworkRoster {
    owner: NodeId,
    administrators: BTreeSet<NodeId>,
}

impl NetworkRoster {
    fn is_manager(&self, actor: &NodeId) -> bool {
        self.owner == *actor || self.administrators.contains(actor)
    }
}

/// Durable invitation ledger for one Coordinator.
///
/// Plaintext invitation codes are returned exactly once from `create` and are
/// never written to disk. The ledger persists only a content digest plus the
/// bounded invitation metadata. Redemption records the one-use grant and then
/// delegates Invite/Approve to the lifecycle ledger, the only writer of the
/// authoritative roster. Retries repair that hand-off without consuming a use
/// and are rejected when the resulting membership has since been revoked.
pub struct InvitationStore {
    path: PathBuf,
    network_record_path: Option<PathBuf>,
    fallback_owner: Option<NodeId>,
    fallback_administrators: BTreeSet<NodeId>,
    member_lifecycle_store: Option<Arc<MemberLifecycleStore>>,
    process_lock: Mutex<()>,
}

impl InvitationStore {
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
            member_lifecycle_store: None,
            process_lock: Mutex::new(()),
        })
    }

    pub fn with_member_lifecycle_store(mut self, store: Arc<MemberLifecycleStore>) -> Self {
        self.member_lifecycle_store = Some(store);
        self
    }

    pub fn create(
        &self,
        network_id: &NetworkId,
        creator: &NodeId,
        request: InvitationCreateRequest,
        now_unix_ms: u64,
    ) -> Result<IssuedInvitation> {
        request.validate()?;
        if request.network_id != *network_id {
            bail!("invitation network does not match the requested network");
        }
        let network_record = self.load_network_record(network_id)?;
        let roster = NetworkRoster {
            owner: network_record.owner.clone(),
            administrators: network_record.administrators.iter().cloned().collect(),
        };
        if !roster.is_manager(creator) {
            bail!("invitation.error.permission_denied");
        }
        if network_record.join_mode != NetworkJoinMode::InviteOnly {
            bail!("invitation.error.network_not_invite_only");
        }
        if request
            .capabilities
            .iter()
            .any(|capability| !network_record.capability_declarations.contains(capability))
        {
            bail!("invitation.error.capability_not_declared");
        }

        let code = InvitationCode::generate()?;
        let invitation_id = new_invitation_id()?;
        let expires_at_unix_ms = now_unix_ms
            .checked_add(request.expires_in_ms())
            .context("invitation expiry overflow")?;
        let record = InvitationRecord::new(
            invitation_id,
            network_id.clone(),
            creator.clone(),
            request.role,
            request.capabilities,
            request.max_uses,
            now_unix_ms,
            expires_at_unix_ms,
        )?;
        let digest = code.digest();
        self.transaction(|file| {
            if file.invitations.contains_key(&record.invitation_id) {
                bail!("invitation identifier collision");
            }
            if file
                .invitations
                .values()
                .any(|stored| stored.digest == digest)
            {
                bail!("invitation digest collision");
            }
            file.invitations.insert(
                record.invitation_id.clone(),
                StoredInvitation {
                    digest,
                    record: record.clone(),
                },
            );
            Ok(IssuedInvitation {
                status: InvitationStatusView::new(record, now_unix_ms),
                code,
            })
        })
    }

    pub fn list(
        &self,
        network_id: &NetworkId,
        actor: &NodeId,
        now_unix_ms: u64,
    ) -> Result<Vec<InvitationStatusView>> {
        let roster = self.load_roster(network_id)?;
        if !roster.is_manager(actor) {
            bail!("invitation.error.permission_denied");
        }
        self.with_file_lock(|file| {
            let mut invitations = file
                .invitations
                .values()
                .filter(|stored| stored.record.network_id == *network_id)
                .map(|stored| InvitationStatusView::new(stored.record.clone(), now_unix_ms))
                .collect::<Vec<_>>();
            invitations.sort_by(|left, right| {
                right
                    .invitation
                    .created_at_unix_ms
                    .cmp(&left.invitation.created_at_unix_ms)
                    .then_with(|| {
                        left.invitation
                            .invitation_id
                            .cmp(&right.invitation.invitation_id)
                    })
            });
            Ok(invitations)
        })
    }

    pub fn status(
        &self,
        network_id: &NetworkId,
        actor: &NodeId,
        invitation_id: &InvitationId,
        now_unix_ms: u64,
    ) -> Result<InvitationStatusView> {
        let roster = self.load_roster(network_id)?;
        if !roster.is_manager(actor) {
            bail!("invitation.error.permission_denied");
        }
        self.with_file_lock(|file| {
            let stored = file
                .invitations
                .get(invitation_id)
                .context("invitation not found")?;
            if stored.record.network_id != *network_id {
                bail!("invitation belongs to another network");
            }
            Ok(InvitationStatusView::new(
                stored.record.clone(),
                now_unix_ms,
            ))
        })
    }

    pub fn revoke(
        &self,
        network_id: &NetworkId,
        actor: &NodeId,
        invitation_id: &InvitationId,
        expected_revision: u64,
        now_unix_ms: u64,
    ) -> Result<InvitationStatusView> {
        let roster = self.load_roster(network_id)?;
        if !roster.is_manager(actor) {
            bail!("invitation.error.permission_denied");
        }
        self.transaction(|file| {
            let stored = file
                .invitations
                .get_mut(invitation_id)
                .context("invitation not found")?;
            if stored.record.network_id != *network_id {
                bail!("invitation belongs to another network");
            }
            if stored.record.revision != expected_revision {
                bail!("invitation.error.stale_revision");
            }
            stored.record.revoke(actor.clone(), now_unix_ms)?;
            Ok(InvitationStatusView::new(
                stored.record.clone(),
                now_unix_ms,
            ))
        })
    }

    pub fn preview(
        &self,
        request_network_id: &NetworkId,
        actor: &NodeId,
        code: &InvitationCode,
        now_unix_ms: u64,
    ) -> Result<InvitationPreview> {
        let network_record = self.load_network_record(request_network_id)?;
        let roster = NetworkRoster {
            owner: network_record.owner.clone(),
            administrators: network_record.administrators.iter().cloned().collect(),
        };
        self.with_file_lock(|file| {
            let (_, stored) = file
                .stored_by_code(code)
                .context("invitation.error.invalid_code")?;
            if stored.record.network_id != *request_network_id {
                bail!("invitation.error.network_mismatch");
            }
            let already_member = network_record.members.contains(actor);
            build_preview(
                stored,
                &network_record,
                &roster,
                already_member,
                now_unix_ms,
            )
        })
    }

    pub fn redeem(
        &self,
        request: InvitationRedeemRequest,
        actor: &NodeId,
        now_unix_ms: u64,
    ) -> Result<InvitationRedemption> {
        if now_unix_ms.saturating_sub(request.acknowledged_at_unix_ms)
            > INVITATION_TERMS_ACK_WINDOW_MS
            || request.acknowledged_at_unix_ms
                > now_unix_ms.saturating_add(INVITATION_ACK_FUTURE_SKEW_MS)
        {
            bail!("invitation.error.terms_acknowledgement_stale");
        }
        let lifecycle = self
            .member_lifecycle_store
            .as_ref()
            .context("协调器未配置成员生命周期台账，邀请兑换失败关闭")?;
        let record_path = self
            .network_record_path
            .as_ref()
            .context("协调器未配置可信 network_record_path，邀请兑换失败关闭")?;
        let network_id = request.network_id.clone();
        let network_record = read_network_record(record_path, &network_id)?;
        if network_record.join_mode != NetworkJoinMode::InviteOnly {
            bail!("invitation.error.network_not_invite_only");
        }
        let roster = NetworkRoster {
            owner: network_record.owner.clone(),
            administrators: network_record.administrators.iter().cloned().collect(),
        };
        let old_file_bytes = std::fs::read(&self.path).ok();
        let mut redemption = None;
        self.with_file_lock(|file| {
            let mut candidate = file.clone();
            let stored = candidate
                .invitations
                .get(&request.invitation_id)
                .context("invitation not found")?;
            if stored.record.network_id != network_id || stored.digest != request.code.digest() {
                bail!("invitation.error.invalid_code");
            }
            if let Some(existing) = candidate
                .redemptions
                .get(&request.invitation_id)
                .and_then(|entries| entries.get(actor))
                .cloned()
            {
                redemption = Some(with_retry_flag(existing, true));
                return Ok(());
            }
            if network_record.members.contains(actor) {
                bail!("invitation.error.already_member");
            }
            let stored = candidate
                .invitations
                .get_mut(&request.invitation_id)
                .context("invitation not found")?;
            if stored.record.revision != request.expected_invitation_revision {
                bail!("invitation.error.stale_revision");
            }
            if !roster.is_manager(&stored.record.created_by) {
                bail!("invitation.error.creator_authority_revoked");
            }
            if !stored.record.state_at(now_unix_ms).is_active() {
                return Err(anyhow::anyhow!(invitation_state_error(
                    stored.record.state_at(now_unix_ms)
                )));
            }
            if stored
                .record
                .capabilities
                .iter()
                .any(|capability| !network_record.capability_declarations.contains(capability))
            {
                bail!("invitation.error.capability_policy_changed");
            }
            let preview = build_preview(stored, &network_record, &roster, false, now_unix_ms)?;
            if preview.preview_digest != request.preview_digest
                || preview.terms_hash != request.terms_hash
            {
                bail!("invitation.error.preview_changed");
            }

            let created_by = stored.record.created_by.clone();
            let role = stored.record.role;
            stored.record.consume_one(now_unix_ms)?;
            let value = InvitationRedemption {
                invitation_id: stored.record.invitation_id.clone(),
                network_id: network_id.clone(),
                node_id: actor.clone(),
                role,
                capabilities: stored.record.capabilities.clone(),
                network_revision: network_record.revision,
                invitation_revision: stored.record.revision,
                invited_by: created_by,
                joined_at_unix_ms: now_unix_ms,
                already_redeemed: false,
                grants_permanent_membership: false,
            };
            candidate
                .redemptions
                .entry(request.invitation_id.clone())
                .or_default()
                .insert(actor.clone(), value.clone());
            candidate.validate()?;
            self.persist(&candidate)?;
            redemption = Some(value);
            Ok(())
        })?;

        let value = redemption.context("invitation redemption disappeared after commit")?;
        if let Err(error) = lifecycle.admit(
            &value.invited_by,
            &value.network_id,
            &value.node_id,
            value.role,
            &format!(
                "invitation:{}:{}",
                value.invitation_id.as_str(),
                value.node_id.as_str()
            ),
            now_unix_ms,
        ) {
            restore_invitation_file(&self.path, old_file_bytes.as_deref())?;
            return Err(error);
        }
        Ok(value)
    }

    /// Return the identity/state required for a server-originated change
    /// notification. This is intentionally separate from the manager-only
    /// status API because an invited node may trigger the final redemption.
    pub fn notification_metadata(
        &self,
        invitation_id: &InvitationId,
        now_unix_ms: u64,
    ) -> Result<(NetworkId, NodeId, InvitationState, u64)> {
        self.with_file_lock(|file| {
            let stored = file
                .invitations
                .get(invitation_id)
                .context("invitation not found")?;
            Ok((
                stored.record.network_id.clone(),
                stored.record.created_by.clone(),
                stored.record.state_at(now_unix_ms),
                stored.record.revision,
            ))
        })
    }

    /// Return only the sanitized redemption projection needed to badge an
    /// authoritative member directory. Invitation digests, codes and ticket
    /// material are never exposed outside the private ledger.
    pub(crate) fn member_directory_redemptions(
        &self,
        network_id: &NetworkId,
        actor: &NodeId,
    ) -> Result<Vec<InvitationRedemption>> {
        let record = self.load_network_record(network_id)?;
        let is_member = record.members.contains(actor);
        let is_reviewer = record.owner == *actor || record.administrators.contains(actor);
        if !is_member && !is_reviewer {
            bail!("invitation.error.permission_denied");
        }
        self.with_file_lock(|file| {
            let mut redemptions = file
                .redemptions
                .values()
                .flat_map(|entries| entries.values())
                .filter(|redemption| redemption.network_id == *network_id)
                .cloned()
                .collect::<Vec<_>>();
            redemptions.sort_by(|left, right| {
                right
                    .joined_at_unix_ms
                    .cmp(&left.joined_at_unix_ms)
                    .then_with(|| left.node_id.cmp(&right.node_id))
            });
            Ok(redemptions)
        })
    }

    pub fn notification_recipients(
        &self,
        network_id: &NetworkId,
        creator: &NodeId,
    ) -> Result<Vec<NodeId>> {
        let roster = self.load_roster(network_id)?;
        let mut recipients = BTreeSet::from([roster.owner.clone()]);
        recipients.extend(roster.administrators.clone());
        if roster.is_manager(creator) {
            recipients.insert(creator.clone());
        }
        Ok(recipients.into_iter().collect())
    }

    fn load_network_record(&self, expected_network_id: &NetworkId) -> Result<NetworkRecord> {
        let Some(path) = &self.network_record_path else {
            bail!("协调器未配置可信 network_record_path，邀请操作失败关闭");
        };
        let lock = lock_profile_shared(path)?;
        let result = read_network_record(path, expected_network_id);
        let _ = FileExt::unlock(&lock);
        result
    }

    fn load_roster(&self, network_id: &NetworkId) -> Result<NetworkRoster> {
        if self.network_record_path.is_some() {
            let record = self.load_network_record(network_id)?;
            return Ok(NetworkRoster {
                owner: record.owner,
                administrators: record.administrators.into_iter().collect(),
            });
        }
        let owner = self
            .fallback_owner
            .clone()
            .context("协调器未配置可信 owner roster，邀请操作失败关闭")?;
        Ok(NetworkRoster {
            owner,
            administrators: self.fallback_administrators.clone(),
        })
    }

    fn transaction<T>(&self, mutate: impl FnOnce(&mut InvitationFile) -> Result<T>) -> Result<T> {
        self.with_file_lock(|file| {
            let mut candidate = file.clone();
            let result = mutate(&mut candidate)?;
            candidate.validate()?;
            self.persist(&candidate)?;
            Ok(result)
        })
    }

    fn with_file_lock<T>(&self, action: impl FnOnce(&InvitationFile) -> Result<T>) -> Result<T> {
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("邀请存储锁已损坏"))?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("无法创建邀请存储目录: {}", parent.display()))?;
        }
        let lock_path = self.path.with_extension("json.lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("无法打开邀请存储锁: {}", lock_path.display()))?;
        lock.lock_exclusive()
            .with_context(|| format!("无法锁定邀请存储: {}", lock_path.display()))?;
        let result = self.load_unlocked().and_then(|file| action(&file));
        let _ = FileExt::unlock(&lock);
        result
    }

    fn load_unlocked(&self) -> Result<InvitationFile> {
        if !self.path.exists() {
            return Ok(InvitationFile::default());
        }
        let content = std::fs::read_to_string(&self.path)
            .with_context(|| format!("无法读取邀请存储: {}", self.path.display()))?;
        let file: InvitationFile = serde_json::from_str(&content)
            .with_context(|| format!("无法解析邀请存储: {}", self.path.display()))?;
        file.validate()?;
        Ok(file)
    }

    fn persist(&self, file: &InvitationFile) -> Result<()> {
        file.validate()?;
        let content = serde_json::to_vec_pretty(file).context("无法序列化邀请存储")?;
        persist_bytes_atomic(&self.path, &content)
    }
}

fn build_preview(
    stored: &StoredInvitation,
    network_record: &NetworkRecord,
    roster: &NetworkRoster,
    already_member: bool,
    now_unix_ms: u64,
) -> Result<InvitationPreview> {
    let state = stored.record.state_at(now_unix_ms);
    let capability_policy_changed = stored
        .record
        .capabilities
        .iter()
        .any(|capability| !network_record.capability_declarations.contains(capability));
    let member_limit_reached = network_record
        .member_limit
        .is_some_and(|limit| network_record.members.len() >= limit as usize);
    let creator_authorized = roster.is_manager(&stored.record.created_by);
    let denial_key = if !state.is_active() {
        Some(invitation_state_error(state).to_string())
    } else if !creator_authorized {
        Some("invitation.error.creator_authority_revoked".into())
    } else if network_record.join_mode != NetworkJoinMode::InviteOnly {
        Some("invitation.error.network_not_invite_only".into())
    } else if capability_policy_changed {
        Some("invitation.error.capability_policy_changed".into())
    } else if already_member {
        Some("invitation.error.already_member".into())
    } else if member_limit_reached {
        Some("invitation.error.member_limit_reached".into())
    } else {
        None
    };
    let terms = network_record.membership_terms();
    let terms_hash = terms.content_hash()?;
    let mut permission_changes = vec![format!("role.{}", role_key(stored.record.role))];
    permission_changes.extend(
        stored
            .record
            .capabilities
            .iter()
            .map(|capability| format!("capability.{}", capability_key(*capability))),
    );
    let mut preview = InvitationPreview {
        invitation_id: stored.record.invitation_id.clone(),
        invitation_revision: stored.record.revision,
        network_id: stored.record.network_id.clone(),
        network_name: network_record.name.clone(),
        network_revision: network_record.revision,
        role: stored.record.role,
        capabilities: stored.record.capabilities.clone(),
        remaining_uses: stored.record.remaining_uses(),
        expires_at_unix_ms: stored.record.expires_at_unix_ms,
        state,
        terms,
        terms_hash,
        preview_digest: ContentHash::from_bytes(b"pending"),
        permission_changes,
        creates_permanent_membership: false,
        requires_recent_reauthentication: true,
        can_accept: denial_key.is_none(),
        denial_key,
    };
    preview.preview_digest = preview.compute_digest()?;
    Ok(preview)
}

fn with_retry_flag(
    mut redemption: InvitationRedemption,
    already_redeemed: bool,
) -> InvitationRedemption {
    redemption.already_redeemed = already_redeemed;
    redemption
}

fn invitation_state_error(state: InvitationState) -> &'static str {
    match state {
        InvitationState::Active => "invitation.error.unavailable",
        InvitationState::Exhausted => "invitation.error.exhausted",
        InvitationState::Expired => "invitation.error.expired",
        InvitationState::Revoked => "invitation.error.revoked",
    }
}

fn role_key(role: MembershipRole) -> &'static str {
    match role {
        MembershipRole::Owner => "owner",
        MembershipRole::Administrator => "administrator",
        MembershipRole::Member => "member",
    }
}

fn capability_key(capability: CapabilityKind) -> &'static str {
    match capability {
        CapabilityKind::Exit => "exit",
        CapabilityKind::File => "file",
        CapabilityKind::Coordinator => "coordinator",
        CapabilityKind::Relay => "relay",
    }
}

fn new_invitation_id() -> Result<InvitationId> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).context("无法生成邀请标识随机数")?;
    Ok(InvitationId::from_public_key(&bytes))
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

fn read_network_record(path: &Path, expected_network_id: &NetworkId) -> Result<NetworkRecord> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("无法读取邀请授权网络资料: {}", path.display()))?;
    let record: NetworkRecord = serde_json::from_str(&content)
        .with_context(|| format!("无法解析邀请授权网络资料: {}", path.display()))?;
    record.validate().context("邀请授权网络资料校验失败")?;
    if record.network_id != *expected_network_id {
        bail!("invitation network does not match owner network record");
    }
    Ok(record)
}

#[cfg(test)]
fn persist_network_record(path: &Path, record: &NetworkRecord) -> Result<()> {
    record.validate()?;
    let content = serde_json::to_vec_pretty(record).context("无法序列化邀请授权网络资料")?;
    persist_bytes_atomic(path, &content)
}

fn restore_invitation_file(path: &Path, bytes: Option<&[u8]>) -> Result<()> {
    match bytes {
        Some(bytes) => persist_bytes_atomic(path, bytes),
        None => std::fs::remove_file(path).context("无法回滚邀请存储"),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use fuckipv6_common::domain::{
        IdempotencyKey, MemberLifecycleAction, MemberLifecycleChangeRequest, MemberLifecycleStatus,
        NetworkCreateProfile, NetworkVisibility, PublicKey,
    };
    use std::sync::Arc;

    const NOW: u64 = 2_000_000;

    fn test_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "fuckipv6-invitations-{}-{label}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn network_public_key() -> PublicKey {
        PublicKey::from_bytes(*b"01234567890123456789012345678901")
    }

    fn network_id() -> NetworkId {
        NetworkId::from_public_key(network_public_key().as_str().as_bytes())
    }

    fn owner() -> NodeId {
        NodeId::from_public_key(b"owner")
    }

    fn admin() -> NodeId {
        NodeId::from_public_key(b"admin")
    }

    fn invitee(seed: &str) -> NodeId {
        NodeId::from_public_key(seed.as_bytes())
    }

    fn network_record(mut administrators: Vec<NodeId>) -> NetworkRecord {
        let mut record = NetworkRecord::create_owned(
            NetworkCreateProfile {
                name: "Private-invitation-network".into(),
                purpose: "Invitation-only collaboration".into(),
                icon: "network".into(),
                language: "zh-CN".into(),
                region: "ZZ".into(),
                tags: vec![],
                capability_declarations: vec![CapabilityKind::File, CapabilityKind::Exit],
                visibility: NetworkVisibility::Private,
                join_mode: NetworkJoinMode::InviteOnly,
                exit_contribution: Default::default(),
                exit_contribution_scope: Default::default(),
                exit_contribution_mode: Default::default(),
                exit_selection_policy: Default::default(),
            },
            network_public_key(),
            owner(),
            NOW - 10_000,
        )
        .unwrap();
        administrators.sort();
        record.members.extend(administrators.clone());
        record.administrators = administrators;
        record.validate().unwrap();
        record
    }

    fn write_network_record(dir: &Path, record: &NetworkRecord) -> PathBuf {
        let path = dir.join("network.json");
        persist_network_record(&path, record).unwrap();
        path
    }

    fn lifecycle_store(dir: &Path) -> Arc<MemberLifecycleStore> {
        Arc::new(
            MemberLifecycleStore::new(dir.join("member_lifecycle.json"), dir.join("network.json"))
                .unwrap(),
        )
    }

    fn open_store(dir: &Path) -> InvitationStore {
        InvitationStore::new(
            dir.join("invitations.json"),
            Some(dir.join("network.json")),
            None,
            vec![],
        )
        .unwrap()
        .with_member_lifecycle_store(lifecycle_store(dir))
    }

    fn create_request(max_uses: u32) -> InvitationCreateRequest {
        InvitationCreateRequest::new(
            network_id(),
            MembershipRole::Member,
            vec![CapabilityKind::File],
            max_uses,
            60 * 60 * 1000,
        )
    }

    fn redemption_request(
        issued: &IssuedInvitation,
        preview: &InvitationPreview,
        acknowledged_at_unix_ms: u64,
    ) -> InvitationRedeemRequest {
        InvitationRedeemRequest {
            network_id: network_id(),
            code: issued.code.clone(),
            invitation_id: issued.status.invitation.invitation_id.clone(),
            expected_invitation_revision: issued.status.invitation.revision,
            preview_digest: preview.preview_digest.clone(),
            terms_hash: preview.terms_hash.clone(),
            acknowledged_at_unix_ms,
            exit_contribution_acknowledged: None,
        }
    }

    #[test]
    fn durable_store_keeps_only_the_code_digest_and_redacts_debug() {
        let dir = test_dir("digest");
        write_network_record(&dir, &network_record(vec![admin()]));
        let store = open_store(&dir);
        let issued = store
            .create(&network_id(), &owner(), create_request(1), NOW)
            .unwrap();
        let plaintext = issued.code.as_str().to_owned();

        let persisted = std::fs::read_to_string(dir.join("invitations.json")).unwrap();
        assert!(!persisted.contains(&plaintext));
        assert!(!format!("{issued:?}").contains(&plaintext));
        let reopened = open_store(&dir);
        let listed = reopened.list(&network_id(), &admin(), NOW).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].state, InvitationState::Active);
        assert_eq!(listed[0].invitation.role, MembershipRole::Member);
        assert_eq!(
            listed[0].invitation.capabilities,
            vec![CapabilityKind::File]
        );
    }

    #[test]
    fn preview_and_redemption_bind_role_capabilities_and_are_idempotent() {
        let dir = test_dir("redeem");
        write_network_record(&dir, &network_record(vec![admin()]));
        let store = open_store(&dir);
        let issued = store
            .create(
                &network_id(),
                &owner(),
                InvitationCreateRequest::new(
                    network_id(),
                    MembershipRole::Administrator,
                    vec![CapabilityKind::File],
                    2,
                    60 * 60 * 1000,
                ),
                NOW,
            )
            .unwrap();
        let invitee = invitee("invitee");
        let preview = store
            .preview(&network_id(), &invitee, &issued.code, NOW + 1)
            .unwrap();
        assert!(preview.can_accept);
        assert!(!preview.creates_permanent_membership);
        assert!(preview.requires_recent_reauthentication);
        assert_eq!(preview.role, MembershipRole::Administrator);
        assert_eq!(preview.capabilities, vec![CapabilityKind::File]);
        assert!(preview
            .permission_changes
            .contains(&"role.administrator".into()));
        assert!(!format!("{preview:?}").contains(issued.code.as_str()));

        let request = redemption_request(&issued, &preview, NOW + 1);
        let first = store.redeem(request.clone(), &invitee, NOW + 2).unwrap();
        assert!(!first.already_redeemed);
        assert!(!first.grants_permanent_membership);
        assert_eq!(first.role, MembershipRole::Administrator);
        let retried = store.redeem(request, &invitee, NOW + 3).unwrap();
        assert!(retried.already_redeemed);
        assert_eq!(retried.invitation_revision, first.invitation_revision);

        let roster = read_network_record(&dir.join("network.json"), &network_id()).unwrap();
        assert!(roster.members.contains(&invitee));
        assert!(roster.administrators.contains(&invitee));
        let listed = store.list(&network_id(), &owner(), NOW + 4).unwrap();
        assert_eq!(listed[0].invitation.used_count, 1);
        assert_eq!(listed[0].remaining_uses, 1);
    }

    #[test]
    fn redemption_retry_after_removal_cannot_resurrect_membership() {
        let dir = test_dir("redeem-after-removal");
        write_network_record(&dir, &network_record(vec![admin()]));
        let lifecycle = lifecycle_store(&dir);
        let store = InvitationStore::new(
            dir.join("invitations.json"),
            Some(dir.join("network.json")),
            None,
            vec![],
        )
        .unwrap()
        .with_member_lifecycle_store(lifecycle.clone());
        let issued = store
            .create(&network_id(), &owner(), create_request(1), NOW)
            .unwrap();
        let invitee = invitee("removed-redemption-retry");
        let preview = store
            .preview(&network_id(), &invitee, &issued.code, NOW + 1)
            .unwrap();
        let request = redemption_request(&issued, &preview, NOW + 1);
        store.redeem(request.clone(), &invitee, NOW + 2).unwrap();
        let admitted = lifecycle
            .record(&network_id(), &invitee)
            .unwrap()
            .expect("successful redemption must create a lifecycle record");
        assert!(admitted.status.grants_network_access());

        let removal = lifecycle
            .change(
                &owner(),
                MemberLifecycleChangeRequest {
                    network_id: network_id(),
                    target_node_id: invitee.clone(),
                    action: MemberLifecycleAction::Remove,
                    role: MembershipRole::Member,
                    expected_revision: admitted.revision,
                    idempotency_key: IdempotencyKey::from_public_key(b"remove-redemption-retry"),
                    reason: Some("removed after successful admission".into()),
                },
                NOW + 3,
            )
            .unwrap();
        let removed = lifecycle
            .record(&network_id(), &invitee)
            .unwrap()
            .expect("removal must preserve a tombstone");
        assert_eq!(removed.status, MemberLifecycleStatus::Removed);
        assert_eq!(removed.revision, removal.revision);
        assert!(removed.access_epoch > admitted.access_epoch);
        assert!(removal.access_revoked);
        assert!(removal.requires_reauthentication);
        assert_ne!(removal.access_epoch, admitted.access_epoch);

        let error = store.redeem(request, &invitee, NOW + 4).unwrap_err();
        assert_eq!(
            error.to_string(),
            "member_lifecycle.error.admission_revoked"
        );
        let retried = lifecycle
            .record(&network_id(), &invitee)
            .unwrap()
            .expect("failed retry must not delete the tombstone");
        assert_eq!(retried.status, MemberLifecycleStatus::Removed);
        assert_eq!(retried.revision, removed.revision);
        assert_eq!(retried.access_epoch, removed.access_epoch);
        let roster = read_network_record(&dir.join("network.json"), &network_id()).unwrap();
        assert!(!roster.members.contains(&invitee));
        assert!(!roster.administrators.contains(&invitee));
    }

    #[test]
    fn member_directory_redemptions_are_permission_filtered_and_never_expose_ticket_material() {
        let dir = test_dir("member-directory-redemptions");
        write_network_record(&dir, &network_record(vec![admin()]));
        let store = open_store(&dir);
        let issued = store
            .create(&network_id(), &owner(), create_request(1), NOW)
            .unwrap();
        let plaintext = issued.code.as_str().to_owned();
        let invitee_node = invitee("redemption-member");
        let preview = store
            .preview(&network_id(), &invitee_node, &issued.code, NOW + 1)
            .unwrap();
        let request = redemption_request(&issued, &preview, NOW + 1);
        let digest = request.preview_digest.clone();
        store.redeem(request, &invitee_node, NOW + 2).unwrap();

        let redemptions = store
            .member_directory_redemptions(&network_id(), &admin())
            .unwrap();
        assert_eq!(redemptions.len(), 1);
        assert_eq!(redemptions[0].node_id, invitee_node);
        let encoded = serde_json::to_string(&redemptions).unwrap();
        assert!(!encoded.contains(&plaintext));
        assert!(!encoded.contains(digest.as_str()));
        assert!(!encoded.contains("fkinv_"));

        let error = store
            .member_directory_redemptions(&network_id(), &invitee("outsider"))
            .unwrap_err();
        assert_eq!(error.to_string(), "invitation.error.permission_denied");
    }

    #[test]
    fn code_cannot_be_redeemed_for_another_network() {
        let first_dir = test_dir("network-one");
        write_network_record(&first_dir, &network_record(vec![]));
        let first_store = open_store(&first_dir);
        let issued = first_store
            .create(&network_id(), &owner(), create_request(1), NOW)
            .unwrap();

        let second_dir = test_dir("network-two");
        let second_key = PublicKey::from_bytes(*b"abcdefghijklmnopqrstuvwxyz123456");
        let second_id = NetworkId::from_public_key(second_key.as_str().as_bytes());
        let second_record = NetworkRecord::create_owned(
            NetworkCreateProfile {
                name: "Another-network".into(),
                purpose: "No cross-network admits".into(),
                icon: "network".into(),
                language: "zh-CN".into(),
                region: "ZZ".into(),
                tags: vec![],
                capability_declarations: vec![CapabilityKind::File],
                visibility: NetworkVisibility::Private,
                join_mode: NetworkJoinMode::InviteOnly,
                exit_contribution: Default::default(),
                exit_contribution_scope: Default::default(),
                exit_contribution_mode: Default::default(),
                exit_selection_policy: Default::default(),
            },
            second_key,
            NodeId::from_public_key(b"other-owner"),
            NOW - 1_000,
        )
        .unwrap();
        write_network_record(&second_dir, &second_record);
        let second_store = InvitationStore::new(
            second_dir.join("invitations.json"),
            Some(second_dir.join("network.json")),
            None,
            vec![],
        )
        .unwrap();
        let error = second_store
            .preview(&second_id, &invitee("outsider"), &issued.code, NOW + 1)
            .unwrap_err();
        assert_eq!(error.to_string(), "invitation.error.invalid_code");
    }

    #[test]
    fn revocation_expiry_and_exhaustion_are_rejected_without_extra_use() {
        let dir = test_dir("unavailable");
        write_network_record(&dir, &network_record(vec![]));
        let store = open_store(&dir);

        let revoked = store
            .create(&network_id(), &owner(), create_request(2), NOW)
            .unwrap();
        let revoked_view = store
            .revoke(
                &network_id(),
                &owner(),
                &revoked.status.invitation.invitation_id,
                revoked.status.invitation.revision,
                NOW + 1,
            )
            .unwrap();
        assert_eq!(revoked_view.state, InvitationState::Revoked);
        let revoked_preview = store
            .preview(&network_id(), &invitee("revoked"), &revoked.code, NOW + 2)
            .unwrap();
        assert!(!revoked_preview.can_accept);
        assert_eq!(
            revoked_preview.denial_key.as_deref(),
            Some("invitation.error.revoked")
        );

        let expired = store
            .create(&network_id(), &owner(), create_request(1), NOW)
            .unwrap();
        let expired_preview = store
            .preview(
                &network_id(),
                &invitee("expired"),
                &expired.code,
                NOW + 60 * 60 * 1000,
            )
            .unwrap();
        assert_eq!(
            expired_preview.denial_key.as_deref(),
            Some("invitation.error.expired")
        );

        let one_use = store
            .create(&network_id(), &owner(), create_request(1), NOW)
            .unwrap();
        let first = invitee("first-use");
        let preview = store
            .preview(&network_id(), &first, &one_use.code, NOW + 1)
            .unwrap();
        store
            .redeem(
                redemption_request(&one_use, &preview, NOW + 1),
                &first,
                NOW + 2,
            )
            .unwrap();
        let second = invitee("second-use");
        let exhausted_preview = store
            .preview(&network_id(), &second, &one_use.code, NOW + 3)
            .unwrap();
        assert_eq!(
            exhausted_preview.denial_key.as_deref(),
            Some("invitation.error.exhausted")
        );
    }

    #[test]
    fn demoting_the_creating_administrator_stops_future_redemption() {
        let dir = test_dir("demotion");
        let mut record = network_record(vec![admin()]);
        let path = write_network_record(&dir, &record);
        let store = open_store(&dir);
        let issued = store
            .create(&network_id(), &admin(), create_request(2), NOW)
            .unwrap();
        let invitee = invitee("after-demotion");
        let preview = store
            .preview(&network_id(), &invitee, &issued.code, NOW + 1)
            .unwrap();
        assert!(preview.can_accept);

        record.administrators.clear();
        persist_network_record(&path, &record).unwrap();
        let denied = store
            .preview(&network_id(), &invitee, &issued.code, NOW + 2)
            .unwrap();
        assert!(!denied.can_accept);
        assert_eq!(
            denied.denial_key.as_deref(),
            Some("invitation.error.creator_authority_revoked")
        );
        let error = store
            .redeem(
                redemption_request(&issued, &preview, NOW + 1),
                &invitee,
                NOW + 3,
            )
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "invitation.error.creator_authority_revoked"
        );
        let roster = read_network_record(&path, &network_id()).unwrap();
        assert!(!roster.members.contains(&invitee));
    }

    #[test]
    fn invitation_join_rechecks_member_limit_and_capability_policy() {
        let limit_dir = test_dir("member-limit-change");
        let mut limited = network_record(vec![]);
        let limited_path = write_network_record(&limit_dir, &limited);
        let limit_store = open_store(&limit_dir);
        let limited_invitation = limit_store
            .create(&network_id(), &owner(), create_request(1), NOW)
            .unwrap();
        limited.member_limit = Some(limited.members.len() as u32);
        persist_network_record(&limited_path, &limited).unwrap();
        let limited_invitee = invitee("member-limit-change");
        let limited_preview = limit_store
            .preview(
                &network_id(),
                &limited_invitee,
                &limited_invitation.code,
                NOW + 1,
            )
            .unwrap();
        assert!(!limited_preview.can_accept);
        assert_eq!(
            limited_preview.denial_key.as_deref(),
            Some("invitation.error.member_limit_reached")
        );
        let limit_error = limit_store
            .redeem(
                redemption_request(&limited_invitation, &limited_preview, NOW + 1),
                &limited_invitee,
                NOW + 2,
            )
            .unwrap_err();
        assert!(limit_error.to_string().contains("member limit"));
        let limited_roster = read_network_record(&limited_path, &network_id()).unwrap();
        assert!(!limited_roster.members.contains(&limited_invitee));

        let policy_dir = test_dir("capability-policy-change");
        let mut policy_changed = network_record(vec![]);
        let policy_path = write_network_record(&policy_dir, &policy_changed);
        let policy_store = open_store(&policy_dir);
        let policy_invitation = policy_store
            .create(&network_id(), &owner(), create_request(1), NOW)
            .unwrap();
        policy_changed
            .capability_declarations
            .retain(|capability| *capability != CapabilityKind::File);
        persist_network_record(&policy_path, &policy_changed).unwrap();
        let policy_invitee = invitee("capability-policy-change");
        let policy_preview = policy_store
            .preview(
                &network_id(),
                &policy_invitee,
                &policy_invitation.code,
                NOW + 1,
            )
            .unwrap();
        assert!(!policy_preview.can_accept);
        assert_eq!(
            policy_preview.denial_key.as_deref(),
            Some("invitation.error.capability_policy_changed")
        );
        let policy_error = policy_store
            .redeem(
                redemption_request(&policy_invitation, &policy_preview, NOW + 1),
                &policy_invitee,
                NOW + 2,
            )
            .unwrap_err();
        assert_eq!(
            policy_error.to_string(),
            "invitation.error.capability_policy_changed"
        );
        let policy_roster = read_network_record(&policy_path, &network_id()).unwrap();
        assert!(!policy_roster.members.contains(&policy_invitee));
    }

    #[test]
    fn stale_preview_and_concurrent_use_allow_exactly_one_admission() {
        let dir = test_dir("concurrent");
        write_network_record(&dir, &network_record(vec![]));
        let store = Arc::new(open_store(&dir));
        let issued = store
            .create(&network_id(), &owner(), create_request(1), NOW)
            .unwrap();
        let first = invitee("concurrent-one");
        let second = invitee("concurrent-two");
        let first_preview = store
            .preview(&network_id(), &first, &issued.code, NOW + 1)
            .unwrap();
        let second_preview = store
            .preview(&network_id(), &second, &issued.code, NOW + 1)
            .unwrap();

        let mut stale_request = redemption_request(&issued, &first_preview, NOW + 1);
        stale_request.preview_digest = ContentHash::from_bytes(b"stale-preview");
        let stale = store.redeem(stale_request, &first, NOW + 2).unwrap_err();
        assert_eq!(stale.to_string(), "invitation.error.preview_changed");

        let first_request = redemption_request(&issued, &first_preview, NOW + 1);
        let second_request = redemption_request(&issued, &second_preview, NOW + 1);
        let first_store = store.clone();
        let second_store = store.clone();
        let first_for_thread = first.clone();
        let second_for_thread = second.clone();
        let first_thread = std::thread::spawn(move || {
            first_store.redeem(first_request, &first_for_thread, NOW + 2)
        });
        let second_thread = std::thread::spawn(move || {
            second_store.redeem(second_request, &second_for_thread, NOW + 2)
        });
        let results = [first_thread.join().unwrap(), second_thread.join().unwrap()];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        let errors = results
            .iter()
            .filter_map(|result| result.as_ref().err().map(ToString::to_string))
            .collect::<Vec<_>>();
        assert_eq!(errors, vec!["invitation.error.stale_revision"]);
        let roster = read_network_record(&dir.join("network.json"), &network_id()).unwrap();
        assert_ne!(
            roster.members.contains(&first),
            roster.members.contains(&second)
        );
    }

    #[test]
    fn stale_terms_acknowledgement_is_rejected() {
        let dir = test_dir("stale-ack");
        write_network_record(&dir, &network_record(vec![]));
        let store = open_store(&dir);
        let issued = store
            .create(&network_id(), &owner(), create_request(1), NOW)
            .unwrap();
        let invitee = invitee("stale-ack");
        let preview = store
            .preview(&network_id(), &invitee, &issued.code, NOW)
            .unwrap();
        let request = redemption_request(&issued, &preview, NOW - 16 * 60 * 1000);
        let error = store.redeem(request, &invitee, NOW).unwrap_err();
        assert_eq!(
            error.to_string(),
            "invitation.error.terms_acknowledgement_stale"
        );
    }
}
