use anyhow::{Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use fs2::FileExt;
use fuckipv6_common::domain::{
    NodeId, NodeOwner, PasskeyCredentialId, PasskeyCredentialRef, RecoveryState, SecretRef,
    SessionRef,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use subtle::ConstantTimeEq;
use webauthn_rs::prelude::{AuthenticationResult, Passkey};

const OWNER_SCHEMA_VERSION: u32 = 2;
const MAX_OWNER_SESSIONS: usize = 64;
const OWNER_DIR: &str = "owner";
const OWNER_FILE: &str = "owner.json";
const RECOVERY_VERIFIER_FILE: &str = "recovery.verifier";
const INIT_LOCK_FILE: &str = ".owner-init.lock";
const RECOVERY_LOCK_FILE: &str = ".owner-recovery.lock";
const BOOTSTRAP_VERIFIER_FILE: &str = "owner-bootstrap.verifier";
const RECOVERY_SECRET_REF: &str = "protected_file:owner/recovery.verifier";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredPasskey {
    pub(crate) reference: PasskeyCredentialRef,
    pub(crate) credential: Passkey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionChannel {
    Local,
    Public,
}

impl SessionChannel {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Public => "public",
        }
    }
}

/// Authenticated owner session metadata. The bearer token is deliberately not
/// representable here: `reference` is derived from the token and is the only
/// session identifier persisted on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredSession {
    pub(crate) reference: SessionRef,
    pub(crate) credential_id: PasskeyCredentialId,
    pub(crate) channel: SessionChannel,
    pub(crate) rp_id: String,
    pub(crate) origin: String,
    pub(crate) created_at_unix_ms: u64,
    pub(crate) expires_at_unix_ms: u64,
    pub(crate) last_authenticated_at_unix_ms: u64,
}

impl StoredSession {
    pub(crate) fn recently_authenticated(&self, now_unix_ms: u64) -> bool {
        now_unix_ms.saturating_sub(self.last_authenticated_at_unix_ms) <= 5 * 60 * 1000
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OwnerStore {
    pub(crate) schema_version: u32,
    pub(crate) owner: NodeOwner,
    pub(crate) passkeys: Vec<StoredPasskey>,
    pub(crate) sessions: Vec<StoredSession>,
}

impl OwnerStore {
    pub(crate) fn new(
        node_id: NodeId,
        display_name: String,
        credential: Passkey,
        backup_eligible: bool,
        backup_state: bool,
        created_at_unix_ms: u64,
    ) -> Result<Self> {
        let credential_id = PasskeyCredentialId::from_public_key(credential.cred_id());
        let reference = PasskeyCredentialRef {
            credential_id,
            label: "本机 Passkey".to_string(),
            created_at_unix_ms,
            last_used_at_unix_ms: None,
            revoked_at_unix_ms: None,
            backup_eligible,
            backup_state,
        };
        let owner = NodeOwner {
            node_id,
            display_name,
            passkeys: vec![reference.clone()],
            recovery: RecoveryState::Configured {
                secret_ref: SecretRef::parse(RECOVERY_SECRET_REF).context("恢复凭据引用无效")?,
                configured_at_unix_ms: created_at_unix_ms,
            },
            sessions: Vec::new(),
        };
        let store = Self {
            schema_version: OWNER_SCHEMA_VERSION,
            owner,
            passkeys: vec![StoredPasskey {
                reference,
                credential,
            }],
            sessions: Vec::new(),
        };
        store.validate()?;
        Ok(store)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if self.schema_version != OWNER_SCHEMA_VERSION {
            anyhow::bail!("unsupported owner store schema");
        }
        self.owner
            .validate_for_node(&self.owner.node_id)
            .context("owner domain validation failed")?;
        if self.passkeys.len() != self.owner.passkeys.len() {
            anyhow::bail!("owner passkey references do not match stored credentials");
        }
        if self.sessions.len() != self.owner.sessions.len() {
            anyhow::bail!("owner session references do not match session records");
        }
        if self.sessions.len() > MAX_OWNER_SESSIONS {
            anyhow::bail!("too many owner sessions");
        }

        let mut seen = std::collections::HashSet::new();
        for (stored, reference) in self.passkeys.iter().zip(&self.owner.passkeys) {
            if &stored.reference != reference {
                anyhow::bail!("owner passkey reference mismatch");
            }
            let raw_id = stored.credential.cred_id();
            let derived = PasskeyCredentialId::from_public_key(raw_id);
            if derived != stored.reference.credential_id {
                anyhow::bail!("stored passkey credential id mismatch");
            }
            if !seen.insert(raw_id.clone()) {
                anyhow::bail!("duplicate stored passkey credential");
            }
        }

        let mut seen_sessions = std::collections::HashSet::new();
        for (stored, reference) in self.sessions.iter().zip(&self.owner.sessions) {
            if &stored.reference != reference || !seen_sessions.insert(&stored.reference) {
                anyhow::bail!("owner session reference mismatch");
            }
            if stored.expires_at_unix_ms <= stored.created_at_unix_ms
                || stored.last_authenticated_at_unix_ms < stored.created_at_unix_ms
                || stored.last_authenticated_at_unix_ms > stored.expires_at_unix_ms
            {
                anyhow::bail!("owner session timestamps are invalid");
            }
            if stored.rp_id.is_empty()
                || stored.rp_id.len() > 253
                || stored.origin.is_empty()
                || stored.origin.len() > 512
            {
                anyhow::bail!("owner session origin binding is invalid");
            }
            let Some(passkey) = self
                .passkeys
                .iter()
                .find(|passkey| passkey.reference.credential_id == stored.credential_id)
            else {
                anyhow::bail!("owner session references an unknown credential");
            };
            if !passkey.reference.is_active() {
                anyhow::bail!("owner session references a revoked credential");
            }
        }

        match &self.owner.recovery {
            RecoveryState::Configured { secret_ref, .. }
                if secret_ref.as_str() == RECOVERY_SECRET_REF => {}
            _ => anyhow::bail!("owner recovery verifier reference is invalid"),
        }
        Ok(())
    }

    pub(crate) fn active_credentials(&self) -> Vec<Passkey> {
        self.passkeys
            .iter()
            .filter(|passkey| passkey.reference.is_active())
            .map(|passkey| passkey.credential.clone())
            .collect()
    }

    pub(crate) fn fingerprint(&self) -> Result<[u8; 32]> {
        let bytes = serde_json::to_vec(self).context("无法计算所有者状态指纹")?;
        Ok(Sha256::digest(bytes).into())
    }

    pub(crate) fn all_credentials(&self) -> Vec<Passkey> {
        self.passkeys
            .iter()
            .map(|passkey| passkey.credential.clone())
            .collect()
    }

    pub(crate) fn active_credential(&self, credential_id: &PasskeyCredentialId) -> Option<Passkey> {
        self.passkeys
            .iter()
            .find(|passkey| {
                &passkey.reference.credential_id == credential_id && passkey.reference.is_active()
            })
            .map(|passkey| passkey.credential.clone())
    }

    pub(crate) fn add_passkey(
        &mut self,
        credential: Passkey,
        label: String,
        backup_eligible: bool,
        backup_state: bool,
        created_at_unix_ms: u64,
    ) -> Result<PasskeyCredentialRef> {
        if label.is_empty() || label.len() > 80 {
            anyhow::bail!("Passkey 名称无效");
        }
        if self.passkeys.len() >= 16 {
            anyhow::bail!("Passkey 数量已达到上限");
        }
        let credential_id = PasskeyCredentialId::from_public_key(credential.cred_id());
        if self
            .passkeys
            .iter()
            .any(|passkey| passkey.reference.credential_id == credential_id)
        {
            anyhow::bail!("Passkey 凭据已存在，拒绝替换");
        }
        let reference = PasskeyCredentialRef {
            credential_id,
            label,
            created_at_unix_ms,
            last_used_at_unix_ms: None,
            revoked_at_unix_ms: None,
            backup_eligible,
            backup_state,
        };
        self.owner.passkeys.push(reference.clone());
        self.passkeys.push(StoredPasskey {
            reference: reference.clone(),
            credential,
        });
        Ok(reference)
    }

    pub(crate) fn record_authentication(
        &mut self,
        result: &AuthenticationResult,
        authenticated_at_unix_ms: u64,
    ) -> Result<PasskeyCredentialId> {
        if !result.user_verified() {
            anyhow::bail!("Passkey 认证未完成用户验证");
        }
        let credential_id = PasskeyCredentialId::from_public_key(result.cred_id());
        let updated_reference = {
            let stored = self
                .passkeys
                .iter_mut()
                .find(|passkey| passkey.reference.credential_id == credential_id)
                .ok_or_else(|| anyhow::anyhow!("Passkey 凭据不存在"))?;
            if !stored.reference.is_active() {
                anyhow::bail!("Passkey 凭据已撤销");
            }
            if stored.credential.update_credential(result).is_none() {
                anyhow::bail!("Passkey 凭据替换被拒绝");
            }
            stored.reference.last_used_at_unix_ms = Some(authenticated_at_unix_ms);
            stored.reference.backup_eligible = result.backup_eligible();
            stored.reference.backup_state = result.backup_state();
            stored.reference.clone()
        };
        let owner_reference = self
            .owner
            .passkeys
            .iter_mut()
            .find(|passkey| passkey.credential_id == credential_id)
            .ok_or_else(|| anyhow::anyhow!("Passkey 凭据不存在"))?;
        *owner_reference = updated_reference;
        Ok(credential_id)
    }

    pub(crate) fn insert_session(&mut self, session: StoredSession) -> Result<()> {
        if self.sessions.len() >= MAX_OWNER_SESSIONS {
            anyhow::bail!("所有者会话数量已达到上限");
        }
        if self
            .sessions
            .iter()
            .any(|existing| existing.reference == session.reference)
        {
            anyhow::bail!("所有者会话标识重复");
        }
        let credential_active = self.passkeys.iter().any(|passkey| {
            passkey.reference.credential_id == session.credential_id
                && passkey.reference.is_active()
        });
        if !credential_active {
            anyhow::bail!("所有者会话凭据已失效");
        }
        self.owner.sessions.push(session.reference.clone());
        self.sessions.push(session);
        Ok(())
    }

    pub(crate) fn session(&self, reference: &SessionRef) -> Option<&StoredSession> {
        self.sessions
            .iter()
            .find(|session| &session.reference == reference)
    }

    pub(crate) fn remove_session(&mut self, reference: &SessionRef) -> bool {
        let previous = self.sessions.len();
        self.sessions
            .retain(|session| &session.reference != reference);
        self.owner.sessions.retain(|session| session != reference);
        self.sessions.len() != previous
    }

    pub(crate) fn revoke_passkey(
        &mut self,
        credential_id: &PasskeyCredentialId,
        revoked_at_unix_ms: u64,
    ) -> Result<PasskeyCredentialRef> {
        if self
            .owner
            .passkeys
            .iter()
            .filter(|passkey| passkey.is_active())
            .count()
            <= 1
        {
            anyhow::bail!("不能撤销最后一个可用 Passkey");
        }
        let reference = self
            .owner
            .passkeys
            .iter_mut()
            .find(|passkey| &passkey.credential_id == credential_id)
            .ok_or_else(|| anyhow::anyhow!("Passkey 凭据不存在"))?;
        if !reference.is_active() {
            anyhow::bail!("Passkey 凭据已撤销");
        }
        reference.revoked_at_unix_ms = Some(revoked_at_unix_ms);
        let revoked = reference.clone();
        let stored = self
            .passkeys
            .iter_mut()
            .find(|passkey| passkey.reference.credential_id == *credential_id)
            .ok_or_else(|| anyhow::anyhow!("Passkey 凭据不存在"))?;
        stored.reference = revoked.clone();

        self.sessions
            .retain(|session| &session.credential_id != credential_id);
        self.owner.sessions.clear();
        self.owner.sessions.extend(
            self.sessions
                .iter()
                .map(|session| session.reference.clone()),
        );
        Ok(revoked)
    }
}

pub(crate) enum OwnerLoad {
    Uninitialized,
    Complete(OwnerStore),
    Corrupt(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommitOutcome {
    Committed,
    AlreadyInitialized,
}

#[derive(Debug, Clone)]
pub(crate) struct OwnerPersistence {
    config_dir: PathBuf,
}

impl OwnerPersistence {
    pub(crate) fn new(config_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&config_dir).context("无法创建所有者安全配置目录")?;
        secure_directory(&config_dir)?;
        Ok(Self { config_dir })
    }

    pub(crate) fn bootstrap_verifier_path(&self) -> PathBuf {
        self.config_dir.join(BOOTSTRAP_VERIFIER_FILE)
    }

    /// Acquire the process-wide local recovery lease. The lease is held from
    /// code verification until the recovery page exits, preventing two local
    /// recovery ceremonies from racing even when they use different ports.
    pub(crate) fn acquire_recovery_lock(&self) -> Result<File> {
        let lock = open_private_file(&self.config_dir.join(RECOVERY_LOCK_FILE), true, true)?;
        FileExt::try_lock_exclusive(&lock).context("已有本机所有者恢复流程正在进行")?;
        Ok(lock)
    }

    pub(crate) fn verify_recovery_code(&self, code: &str) -> Result<bool> {
        let stored = self.read_recovery_verifier()?;
        let supplied = Sha256::digest(code.as_bytes());
        Ok(stored.ct_eq(supplied.as_slice()).into())
    }

    /// Provision the one-time bootstrap code without ever persisting the code
    /// itself. Only its fixed-size digest is written to the protected config
    /// directory; the caller must display the plaintext once on the terminal.
    pub(crate) fn provision_bootstrap_code(&self, code: &str) -> Result<()> {
        if code.len() < 20 {
            anyhow::bail!("初始化码长度不足");
        }
        let path = self.bootstrap_verifier_path();
        let verifier = bootstrap_verifier(code);
        write_private_file_atomic(&path, &verifier)
            .with_context(|| format!("无法写入本地初始化码验证器: {}", path.display()))
    }

    pub(crate) fn verify_bootstrap_code(&self, code: &str) -> Result<bool> {
        let path = self.bootstrap_verifier_path();
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error).context("无法检查本地初始化码验证器"),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() != 32 {
            anyhow::bail!("本地初始化码验证器无效");
        }
        secure_file(&path)?;
        let stored = std::fs::read(&path).context("无法读取本地初始化码验证器")?;
        let supplied = bootstrap_verifier(code);
        Ok(stored.ct_eq(&supplied).into())
    }

    pub(crate) fn consume_bootstrap_code(&self) -> Result<()> {
        match std::fs::remove_file(self.bootstrap_verifier_path()) {
            Ok(()) => sync_directory(&self.config_dir),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).context("无法删除已消费的本地初始化码验证器"),
        }
    }

    pub(crate) fn load(&self) -> Result<OwnerLoad> {
        let owner_dir = self.config_dir.join(OWNER_DIR);
        let metadata = match std::fs::symlink_metadata(&owner_dir) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(OwnerLoad::Uninitialized)
            }
            Err(error) => return Err(error).context("无法检查所有者目录"),
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Ok(OwnerLoad::Corrupt("owner_path_invalid"));
        }
        secure_directory(&owner_dir)?;

        let owner_path = owner_dir.join(OWNER_FILE);
        let verifier_path = owner_dir.join(RECOVERY_VERIFIER_FILE);
        let owner_file = match std::fs::symlink_metadata(&owner_path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => metadata,
            Ok(_) => return Ok(OwnerLoad::Corrupt("owner_file_invalid")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(OwnerLoad::Corrupt("owner_file_missing"))
            }
            Err(error) => return Err(error).context("无法检查所有者记录"),
        };
        let verifier_file = match std::fs::symlink_metadata(&verifier_path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => metadata,
            Ok(_) => return Ok(OwnerLoad::Corrupt("recovery_verifier_invalid")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(OwnerLoad::Corrupt("recovery_verifier_missing"))
            }
            Err(error) => return Err(error).context("无法检查恢复码验证器"),
        };
        if owner_file.len() == 0 || verifier_file.len() == 0 {
            return Ok(OwnerLoad::Corrupt("owner_record_empty"));
        }
        secure_file(&owner_path)?;
        secure_file(&verifier_path)?;

        let content = match std::fs::read(&owner_path) {
            Ok(content) => content,
            Err(error) => return Err(error).context("无法读取所有者记录"),
        };
        let store: OwnerStore = match serde_json::from_slice(&content) {
            Ok(store) => store,
            Err(_) => return Ok(OwnerLoad::Corrupt("owner_record_unreadable")),
        };
        if store.validate().is_err() {
            return Ok(OwnerLoad::Corrupt("owner_record_invalid"));
        }
        match std::fs::read_to_string(&verifier_path) {
            Ok(value) if !value.trim().is_empty() => {}
            _ => return Ok(OwnerLoad::Corrupt("recovery_verifier_invalid")),
        }
        Ok(OwnerLoad::Complete(store))
    }

    pub(crate) fn commit(
        &self,
        store: &OwnerStore,
        recovery_verifier: &[u8; 32],
    ) -> Result<CommitOutcome> {
        store.validate()?;
        let lock_path = self.config_dir.join(INIT_LOCK_FILE);
        let lock = open_private_file(&lock_path, true, true)?;
        FileExt::lock_exclusive(&lock).context("无法锁定所有者初始化事务")?;
        let result = self.commit_locked(store, recovery_verifier);
        let unlock = FileExt::unlock(&lock);
        match (result, unlock) {
            (Ok(outcome), Ok(())) => Ok(outcome),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error).context("无法释放所有者初始化锁"),
        }
    }

    /// Atomically mutate an initialized owner record while preserving the
    /// recovery verifier. The lock covers the fresh read, mutation, validation
    /// and durable replacement so concurrent catalog/session operations cannot
    /// overwrite each other with stale snapshots.
    pub(crate) fn update<T>(&self, mutate: impl FnOnce(&mut OwnerStore) -> Result<T>) -> Result<T> {
        let lock_path = self.config_dir.join(INIT_LOCK_FILE);
        let lock = open_private_file(&lock_path, true, true)?;
        FileExt::lock_exclusive(&lock).context("无法锁定所有者更新事务")?;
        let result = (|| -> Result<T> {
            let mut store = match self.load()? {
                OwnerLoad::Complete(store) => store,
                OwnerLoad::Uninitialized => anyhow::bail!("所有者尚未初始化"),
                OwnerLoad::Corrupt(reason) => anyhow::bail!("所有者状态损坏: {reason}"),
            };
            let value = mutate(&mut store)?;
            store.validate()?;
            let owner_json = serde_json::to_vec_pretty(&store).context("无法序列化所有者记录")?;
            let owner_dir = self.config_dir.join(OWNER_DIR);
            write_private_file_atomic(&owner_dir.join(OWNER_FILE), &owner_json)?;
            sync_directory(&owner_dir)?;
            Ok(value)
        })();
        let unlock = FileExt::unlock(&lock);
        match (result, unlock) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error).context("无法释放所有者更新锁"),
        }
    }

    /// Replace every owner credential and invalidate every persisted session
    /// after a local recovery. `expected_fingerprint` rejects a recovery that
    /// raced with a passkey/session mutation and would overwrite newer state.
    pub(crate) fn replace_after_recovery(
        &self,
        expected_fingerprint: &[u8; 32],
        store: &OwnerStore,
        recovery_verifier: &[u8; 32],
    ) -> Result<()> {
        store.validate()?;
        let lock_path = self.config_dir.join(INIT_LOCK_FILE);
        let lock = open_private_file(&lock_path, true, true)?;
        FileExt::lock_exclusive(&lock).context("无法锁定所有者恢复事务")?;
        let result = (|| -> Result<()> {
            let current = match self.load()? {
                OwnerLoad::Complete(store) => store,
                OwnerLoad::Uninitialized => anyhow::bail!("所有者尚未初始化"),
                OwnerLoad::Corrupt(reason) => anyhow::bail!("所有者状态损坏: {reason}"),
            };
            let current_fingerprint = current.fingerprint()?;
            if !bool::from(current_fingerprint.ct_eq(expected_fingerprint)) {
                anyhow::bail!("恢复期间所有者状态已变化，请重新开始恢复");
            }
            if current.owner.node_id != store.owner.node_id {
                anyhow::bail!("恢复记录属于其他节点，拒绝替换");
            }

            let owner_dir = self.config_dir.join(OWNER_DIR);
            let owner_path = owner_dir.join(OWNER_FILE);
            let verifier_path = owner_dir.join(RECOVERY_VERIFIER_FILE);
            let previous_owner =
                serde_json::to_vec_pretty(&current).context("无法序列化恢复前所有者记录")?;
            let replacement_owner =
                serde_json::to_vec_pretty(store).context("无法序列化恢复后所有者记录")?;
            let replacement_verifier = URL_SAFE_NO_PAD.encode(recovery_verifier);

            write_private_file_atomic(&owner_path, &replacement_owner)?;
            if let Err(error) =
                write_private_file_atomic(&verifier_path, replacement_verifier.as_bytes())
            {
                if let Err(rollback_error) = write_private_file_atomic(&owner_path, &previous_owner)
                {
                    return Err(error).context(format!(
                        "恢复码验证器写入失败且所有者记录回滚失败: {rollback_error}"
                    ));
                }
                return Err(error).context("恢复码验证器写入失败，所有者记录已回滚");
            }
            Ok(())
        })();
        let unlock = FileExt::unlock(&lock);
        match (result, unlock) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), _) => Err(error),
            (Ok(()), Err(error)) => Err(error).context("无法释放所有者恢复锁"),
        }
    }

    fn read_recovery_verifier(&self) -> Result<[u8; 32]> {
        let path = self.config_dir.join(OWNER_DIR).join(RECOVERY_VERIFIER_FILE);
        let metadata = std::fs::symlink_metadata(&path).context("无法检查恢复码验证器")?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            anyhow::bail!("恢复码验证器无效");
        }
        secure_file(&path)?;
        let encoded = std::fs::read_to_string(&path).context("无法读取恢复码验证器")?;
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded.trim())
            .context("恢复码验证器编码无效")?;
        bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("恢复码验证器长度无效"))
    }

    fn commit_locked(
        &self,
        store: &OwnerStore,
        recovery_verifier: &[u8; 32],
    ) -> Result<CommitOutcome> {
        let owner_dir = self.config_dir.join(OWNER_DIR);
        match std::fs::symlink_metadata(&owner_dir) {
            Ok(metadata)
                if metadata.is_dir()
                    && !metadata.file_type().is_symlink()
                    && owner_dir.join(OWNER_FILE).is_file() =>
            {
                return Ok(CommitOutcome::AlreadyInitialized)
            }
            Ok(_) => anyhow::bail!("所有者存储已存在但并非完整记录"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("无法检查所有者存储"),
        }

        let staging = self.config_dir.join(format!(
            ".owner-stage-{}-{}",
            std::process::id(),
            random_suffix()
        ));
        std::fs::create_dir(&staging).context("无法创建所有者事务暂存目录")?;
        let commit_result = (|| -> Result<()> {
            secure_directory(&staging)?;
            let owner_json = serde_json::to_vec_pretty(store).context("无法序列化所有者记录")?;
            write_private_file(&staging.join(OWNER_FILE), &owner_json)?;
            write_private_file(
                &staging.join(RECOVERY_VERIFIER_FILE),
                URL_SAFE_NO_PAD.encode(recovery_verifier).as_bytes(),
            )?;
            sync_directory(&staging)?;
            std::fs::rename(&staging, &owner_dir).context("无法原子提交所有者记录")?;
            sync_directory(&self.config_dir)?;
            Ok(())
        })();

        if let Err(error) = commit_result {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
        Ok(CommitOutcome::Committed)
    }
}

fn bootstrap_verifier(code: &str) -> [u8; 32] {
    Sha256::digest(code.as_bytes()).into()
}

fn random_suffix() -> String {
    let mut bytes = [0u8; 12];
    if getrandom::fill(&mut bytes).is_err() {
        // The suffix is not a security boundary. The lock already serializes
        // the transaction; this fallback only avoids a collision in temp names.
        return format!(
            "{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or_default()
        );
    }
    URL_SAFE_NO_PAD.encode(bytes)
}

fn open_private_file(path: &Path, create: bool, write: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(write).create(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(path)
        .with_context(|| format!("无法打开受保护文件: {}", path.display()))?;
    secure_file(path)?;
    Ok(file)
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = open_private_file(path, true, true)?;
    file.write_all(bytes)
        .with_context(|| format!("无法写入受保护文件: {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("无法同步受保护文件: {}", path.display()))?;
    Ok(())
}

fn write_private_file_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("受保护文件缺少父目录")?;
    let temporary = parent.join(format!(".tmp-{}-{}", std::process::id(), random_suffix()));
    let write_result = (|| -> Result<()> {
        write_private_file(&temporary, bytes)?;
        std::fs::rename(&temporary, path).context("无法原子替换受保护文件")?;
        sync_directory(parent)
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    write_result
}

fn secure_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("无法收紧目录权限: {}", path.display()))?;
    }
    let _ = path;
    Ok(())
}

fn secure_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("无法收紧文件权限: {}", path.display()))?;
    }
    let _ = path;
    Ok(())
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|directory| directory.sync_all())
            .with_context(|| format!("无法同步目录: {}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn fixture_passkey() -> Passkey {
    serde_json::from_str(
        r#"{"cred":{"cred_id":"uZcVDBVS68E_MtAgeQpElJxldF_6cY9sSvbWqx_qRh8wiu42lyRBRmh5yFeD_r9k130dMbFHBHI9RTFgdJQIzQ","cred":{"type_":"ES256","key":{"EC_EC2":{"curve":"SECP256R1","x":[194,126,127,109,252,23,131,21,252,6,223,99,44,254,140,27,230,17,94,5,133,28,104,41,144,69,171,149,161,26,200,243],"y":[143,123,183,156,24,178,21,248,117,159,162,69,171,52,188,252,26,59,6,47,103,92,19,58,117,103,249,0,219,8,95,196]}}},"counter":2,"user_verified":false,"backup_eligible":false,"backup_state":false,"registration_policy":"preferred","extensions":{"cred_protect":"NotRequested","hmac_create_secret":"NotRequested"},"attestation":{"data":{"Basic":["MIICvTCCAaWgAwIBAgIEK_F8eDANBgkqhkiG9w0BAQsFADAuMSwwKgYDVQQDEyNZdWJpY28gVTJGIFJvb3QgQ0EgU2VyaWFsIDQ1NzIwMDYzMTAgFw0xNDA4MDEwMDAwMDBaGA8yMDUwMDkwNDAwMDAwMFowbjELMAkGA1UEBhMCU0UxEjAQBgNVBAoMCVl1YmljbyBBQjEiMCAGA1UECwwZQXV0aGVudGljYXRvciBBdHRlc3RhdGlvbjEnMCUGA1UEAwweWXViaWNvIFUyRiBFRSBTZXJpYWwgNzM3MjQ2MzI4MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEdMLHhCPIcS6bSPJZWGb8cECuTN8H13fVha8Ek5nt-pI8vrSflxb59Vp4bDQlH8jzXj3oW1ZwUDjHC6EnGWB5i6NsMGowIgYJKwYBBAGCxAoCBBUxLjMuNi4xLjQuMS40MTQ4Mi4xLjcwEwYLKwYBBAGC5RwCAQEEBAMCAiQwIQYLKwYBBAGC5RwBAQQEEgQQxe9V_62aS5-1gK3rr-Am0DAMBgNVHRMBAf8EAjAAMA0GCSqGSIb3DQEBCwUAA4IBAQCLbpN2nXhNbunZANJxAn_Cd-S4JuZsObnUiLnLLS0FPWa01TY8F7oJ8bE-aFa4kTe6NQQfi8-yiZrQ8N-JL4f7gNdQPSrH-r3iFd4SvroDe1jaJO4J9LeiFjmRdcVa-5cqNF4G1fPCofvw9W4lKnObuPakr0x_icdVq1MXhYdUtQk6Zr5mBnc4FhN9qi7DXqLHD5G7ZFUmGwfIcD2-0m1f1mwQS8yRD5-_aDCf3vutwddoi3crtivzyromwbKklR4qHunJ75LGZLZA8pJ_mXnUQ6TTsgRqPvPXgQPbSyGMf2z_DIPbQqCD_Bmc4dj9o6LozheBdDtcZCAjSPTAd_ui"]},"metadata":"None"},"attestation_format":"Packed"}}"#,
    )
    .expect("fixture passkey")
}

#[cfg(test)]
pub(crate) fn fixture_passkey_with_id(id: &[u8]) -> Passkey {
    let mut value = serde_json::to_value(fixture_passkey()).expect("fixture passkey json");
    value["cred"]["cred_id"] = serde_json::Value::String(URL_SAFE_NO_PAD.encode(id));
    serde_json::from_value(value).expect("fixture passkey with id")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::sync::{Arc, Barrier};

    fn node_id() -> NodeId {
        NodeId::from_public_key(b"owner-store-test")
    }

    fn fixture_store() -> OwnerStore {
        OwnerStore::new(
            node_id(),
            "Owner".to_string(),
            fixture_passkey(),
            false,
            false,
            1_700_000_000_000,
        )
        .expect("fixture owner")
    }

    #[test]
    fn commit_is_atomic_private_and_exactly_once() -> Result<()> {
        let dir = std::env::temp_dir().join(format!(
            "fuckipv6-owner-store-{}-{}",
            std::process::id(),
            random_suffix()
        ));
        let persistence = OwnerPersistence::new(dir.clone())?;
        let store = fixture_store();
        assert!(matches!(
            persistence.commit(&store, &[7u8; 32])?,
            CommitOutcome::Committed
        ));
        assert!(matches!(
            persistence.commit(&store, &[9u8; 32])?,
            CommitOutcome::AlreadyInitialized
        ));
        let loaded = persistence.load()?;
        let OwnerLoad::Complete(loaded) = loaded else {
            panic!("owner must load completely");
        };
        assert_eq!(loaded.owner.node_id, node_id());
        let verifier = std::fs::read_to_string(dir.join(OWNER_DIR).join(RECOVERY_VERIFIER_FILE))?;
        assert_eq!(verifier, URL_SAFE_NO_PAD.encode([7u8; 32]));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&dir)?.permissions().mode() & 0o777, 0o700);
            assert_eq!(
                std::fs::metadata(dir.join(OWNER_DIR))?.permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(dir.join(OWNER_DIR).join(OWNER_FILE))?
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let mut json = String::new();
        File::open(dir.join(OWNER_DIR).join(OWNER_FILE))?.read_to_string(&mut json)?;
        assert!(!json.contains(&URL_SAFE_NO_PAD.encode([7u8; 32])));
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn incomplete_owner_directory_fails_closed() -> Result<()> {
        let dir = std::env::temp_dir().join(format!(
            "fuckipv6-owner-corrupt-{}-{}",
            std::process::id(),
            random_suffix()
        ));
        let persistence = OwnerPersistence::new(dir.clone())?;
        std::fs::create_dir(dir.join(OWNER_DIR))?;
        assert!(matches!(
            persistence.load()?,
            OwnerLoad::Corrupt("owner_file_missing")
        ));
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn passkey_limit_duplicate_and_revocation_are_enforced() -> Result<()> {
        let mut store = fixture_store();
        let first_id = store.owner.passkeys[0].credential_id.clone();
        assert!(store.revoke_passkey(&first_id, 1_700_000_000_001).is_err());

        let second = store.add_passkey(
            fixture_passkey_with_id(b"store-test-secondary-passkey"),
            "备用 Passkey".to_string(),
            false,
            false,
            1_700_000_000_002,
        )?;
        assert!(store
            .add_passkey(
                fixture_passkey_with_id(b"store-test-secondary-passkey"),
                "重复 Passkey".to_string(),
                false,
                false,
                1_700_000_000_003,
            )
            .is_err());

        let session_reference = SessionRef::new([9u8; 32]);
        store.insert_session(StoredSession {
            reference: session_reference.clone(),
            credential_id: second.credential_id.clone(),
            channel: SessionChannel::Local,
            rp_id: "localhost".to_string(),
            origin: "http://localhost:8080".to_string(),
            created_at_unix_ms: 1_700_000_000_400,
            expires_at_unix_ms: 1_700_000_000_900,
            last_authenticated_at_unix_ms: 1_700_000_000_400,
        })?;

        let revoked = store.revoke_passkey(&second.credential_id, 1_700_000_000_500)?;
        assert!(revoked.revoked_at_unix_ms.is_some());
        assert!(store.session(&session_reference).is_none());
        assert!(store.owner.sessions.is_empty());
        store.validate()?;
        Ok(())
    }

    #[test]
    fn concurrent_updates_do_not_lose_passkeys() -> Result<()> {
        let dir = std::env::temp_dir().join(format!(
            "fuckipv6-owner-concurrent-update-{}-{}",
            std::process::id(),
            random_suffix()
        ));
        let persistence = OwnerPersistence::new(dir.clone())?;
        persistence.commit(&fixture_store(), &[7u8; 32])?;
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = [
            (b"store-concurrent-passkey-a".as_slice(), "并发 A"),
            (b"store-concurrent-passkey-b".as_slice(), "并发 B"),
        ]
        .into_iter()
        .map(|(id, label)| {
            let persistence = persistence.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                persistence.update(|store| {
                    store.add_passkey(
                        fixture_passkey_with_id(id),
                        label.to_string(),
                        false,
                        false,
                        1_700_000_000_600,
                    )?;
                    Ok(())
                })
            })
        })
        .collect();

        for handle in handles {
            handle.join().expect("owner update thread")?;
        }
        let OwnerLoad::Complete(store) = persistence.load()? else {
            panic!("owner must remain complete");
        };
        assert_eq!(store.passkeys.len(), 3);
        store.validate()?;
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
}
