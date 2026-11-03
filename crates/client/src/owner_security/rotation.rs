//! Crash-recoverable node identity rotation.
//!
//! The rotation journal deliberately contains public identifiers only. The new
//! private key is written exactly once by `Identity::save_to_file`; recovery
//! never needs to persist or reconstruct secret material.

use super::store::{OwnerNodeMigration, OwnerPersistence};
use crate::config_manager::ConfigManager;
use anyhow::{Context, Result};
use fs2::FileExt;
use fuckipv6_common::domain::NodeId;
use fuckipv6_common::Identity;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const ROTATION_SCHEMA_VERSION: u32 = 1;
const ROTATION_LOCK_FILE: &str = ".identity-rotation.lock";
const ROTATION_JOURNAL_FILE: &str = ".identity-rotation.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NodeIdentityRotation {
    pub(crate) old_node_id: String,
    pub(crate) old_fingerprint: String,
    pub(crate) new_node_id: String,
    pub(crate) new_fingerprint: String,
    pub(crate) new_created_at_unix_ms: u64,
    pub(crate) owner_initialized: bool,
    pub(crate) owner_sessions_invalidated: bool,
    pub(crate) affected_network_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RotationJournal {
    schema_version: u32,
    old_node_id: String,
    old_fingerprint: String,
    new_node_id: String,
    new_fingerprint: String,
    new_created_at_unix_ms: u64,
}

impl RotationJournal {
    fn new(old: &Identity, new: &Identity, created_at_unix_ms: u64) -> Self {
        Self {
            schema_version: ROTATION_SCHEMA_VERSION,
            old_node_id: NodeId::from_public_key(&old.public_key_bytes()).to_string(),
            old_fingerprint: Identity::public_key_fingerprint(&old.public_key_bytes()),
            new_node_id: NodeId::from_public_key(&new.public_key_bytes()).to_string(),
            new_fingerprint: Identity::public_key_fingerprint(&new.public_key_bytes()),
            new_created_at_unix_ms: created_at_unix_ms,
        }
    }

    fn validate(&self) -> Result<()> {
        if self.schema_version != ROTATION_SCHEMA_VERSION {
            anyhow::bail!("不支持的节点身份轮换日志版本");
        }
        if self.old_node_id == self.new_node_id
            || self.old_fingerprint == self.new_fingerprint
            || self.old_node_id.is_empty()
            || self.old_fingerprint.is_empty()
            || self.new_node_id.is_empty()
            || self.new_fingerprint.is_empty()
        {
            anyhow::bail!("节点身份轮换日志内容无效");
        }
        Ok(())
    }

    fn matches_old(&self, identity: &Identity) -> bool {
        NodeId::from_public_key(&identity.public_key_bytes()).as_str() == self.old_node_id
            && Identity::public_key_fingerprint(&identity.public_key_bytes())
                == self.old_fingerprint
    }

    fn matches_new(&self, identity: &Identity) -> bool {
        NodeId::from_public_key(&identity.public_key_bytes()).as_str() == self.new_node_id
            && Identity::public_key_fingerprint(&identity.public_key_bytes())
                == self.new_fingerprint
    }
}

/// Load the current identity while holding the rotation lock and completing any
/// journaled migration. Every identity consumer uses this path so a WebUI owner
/// service can never observe the new key while the owner store still trusts the
/// old node.
pub(crate) fn load_reconciled_identity(configs: &ConfigManager) -> Result<Identity> {
    let _lock = acquire_rotation_lock(configs)?;
    reconcile_locked(configs)?;
    let path = configs.node_identity_path();
    if path.exists() {
        Identity::load_from_file(&path).context("无法加载节点身份")
    } else {
        let identity = Identity::generate().context("无法生成节点身份")?;
        identity
            .save_to_file(&path)
            .context("无法保存新生成的节点身份")?;
        Ok(identity)
    }
}

/// Rotate the node identity after verifying the public identity expected by the
/// caller. Owner credentials are preserved; all owner sessions are invalidated.
pub(crate) fn rotate_node_identity(
    configs: &ConfigManager,
    expected_fingerprint: &str,
) -> Result<NodeIdentityRotation> {
    let _lock = acquire_rotation_lock(configs)?;
    reconcile_locked(configs)?;

    let identity_path = configs.node_identity_path();
    if !identity_path.exists() {
        anyhow::bail!("节点身份尚未初始化");
    }
    let old = Identity::load_from_file(&identity_path).context("无法读取当前节点身份")?;
    let old_fingerprint = Identity::public_key_fingerprint(&old.public_key_bytes());
    if old_fingerprint != expected_fingerprint {
        anyhow::bail!("节点身份已变化，请刷新后重新确认轮换");
    }
    let old_node = NodeId::from_public_key(&old.public_key_bytes());

    let persistence = OwnerPersistence::new(configs.config_dir().to_path_buf())?;
    let preflight = persistence.validate_node_for_rotation(&old_node)?;

    let replacement = Identity::generate().context("无法生成新的节点身份")?;
    if replacement.public_key_bytes() == old.public_key_bytes() {
        anyhow::bail!("新节点身份与当前身份相同，拒绝轮换");
    }
    let created_at = unix_time_ms()?;
    let journal = RotationJournal::new(&old, &replacement, created_at);

    // Persist the public-only journal before changing either store. A crash at
    // any following point is completed by `load_reconciled_identity`.
    write_journal(configs, &journal)?;
    if let Err(error) = replacement.save_to_file(&identity_path) {
        // The atomic key write either kept the old identity or installed the
        // replacement. Only discard the journal when the old identity remains.
        if Identity::load_from_file(&identity_path)
            .is_ok_and(|identity| journal.matches_old(&identity))
        {
            let _ = remove_journal(configs);
        }
        return Err(error).context("无法原子替换节点身份密钥");
    }

    if let Err(error) = complete_journal_locked(configs, &persistence, &journal) {
        return Err(error)
            .context("新节点身份已写入，但所有者迁移或清理尚未完成；后续加载会继续恢复该事务");
    }

    Ok(NodeIdentityRotation {
        old_node_id: journal.old_node_id,
        old_fingerprint: journal.old_fingerprint,
        new_node_id: journal.new_node_id,
        new_fingerprint: journal.new_fingerprint,
        new_created_at_unix_ms: journal.new_created_at_unix_ms,
        owner_initialized: preflight.owner_initialized,
        owner_sessions_invalidated: preflight.had_owner_sessions,
        affected_network_count: configs.list_networks().map_or(0, |networks| networks.len()),
    })
}

fn reconcile_locked(configs: &ConfigManager) -> Result<()> {
    let path = journal_path(configs);
    let content = match std::fs::read(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("无法读取节点身份轮换日志"),
    };
    let journal: RotationJournal =
        serde_json::from_slice(&content).context("无法解析节点身份轮换日志")?;
    journal.validate()?;
    let identity = Identity::load_from_file(configs.node_identity_path())
        .context("无法读取轮换后的节点身份")?;

    if journal.matches_old(&identity) {
        let old_node = NodeId::from_public_key(&identity.public_key_bytes());
        let _ = OwnerPersistence::new(configs.config_dir().to_path_buf())?
            .validate_node_for_rotation(&old_node)?;
        remove_journal(configs)?;
        return Ok(());
    }
    if !journal.matches_new(&identity) {
        anyhow::bail!("节点身份与未完成轮换日志不匹配，拒绝继续启动");
    }

    let persistence = OwnerPersistence::new(configs.config_dir().to_path_buf())?;
    complete_journal_locked(configs, &persistence, &journal)
}

fn complete_journal_locked(
    configs: &ConfigManager,
    persistence: &OwnerPersistence,
    journal: &RotationJournal,
) -> Result<()> {
    configs.set_node_identity_created_at_unix_ms(journal.new_created_at_unix_ms)?;
    let old_node = NodeId::parse(&journal.old_node_id).context("轮换日志中的旧 Node ID 无效")?;
    let new_node = NodeId::parse(&journal.new_node_id).context("轮换日志中的新 Node ID 无效")?;
    match persistence.migrate_owner_node(&old_node, &new_node)? {
        OwnerNodeMigration::Uninitialized
        | OwnerNodeMigration::Migrated
        | OwnerNodeMigration::AlreadyMigrated => {}
    }
    remove_journal(configs)
}

fn acquire_rotation_lock(configs: &ConfigManager) -> Result<File> {
    let path = configs.config_dir().join(ROTATION_LOCK_FILE);
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(&path)
        .with_context(|| format!("无法打开节点身份轮换锁: {}", path.display()))?;
    secure_private_file(&path)?;
    FileExt::lock_exclusive(&file).context("无法锁定节点身份轮换事务")?;
    Ok(file)
}

fn write_journal(configs: &ConfigManager, journal: &RotationJournal) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(journal).context("无法序列化节点身份轮换日志")?;
    write_private_atomic(&journal_path(configs), &bytes)
}

fn remove_journal(configs: &ConfigManager) -> Result<()> {
    let path = journal_path(configs);
    match std::fs::remove_file(&path) {
        Ok(()) => sync_directory(configs.config_dir()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("无法删除节点身份轮换日志"),
    }
}

fn journal_path(configs: &ConfigManager) -> std::path::PathBuf {
    configs.config_dir().join(ROTATION_JOURNAL_FILE)
}

fn write_private_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().context("轮换状态文件缺少父目录")?;
    let temporary = parent.join(format!(
        ".tmp-rotation-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let write_result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .with_context(|| format!("无法创建轮换状态文件: {}", temporary.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("无法写入轮换状态文件: {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("无法同步轮换状态文件: {}", temporary.display()))?;
        std::fs::rename(&temporary, path).context("无法原子替换节点身份轮换日志")?;
        sync_directory(parent)
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    write_result
}

#[cfg(unix)]
fn secure_private_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("无法收紧轮换状态文件权限: {}", path.display()))
}

#[cfg(not(unix))]
fn secure_private_file(_path: &Path) -> Result<()> {
    Ok(())
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|directory| directory.sync_all())
            .with_context(|| format!("无法同步轮换状态目录: {}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn unix_time_ms() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("系统时间早于 Unix epoch")?
        .as_millis()
        .try_into()
        .context("系统时间超出 u64 范围")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::owner_security::store::{fixture_passkey, OwnerLoad, SessionChannel, StoredSession};
    use fuckipv6_common::domain::SessionRef;

    fn temp_dir(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "fuckipv6-identity-rotation-{label}-{}-{}",
            std::process::id(),
            unix_time_ms().unwrap_or_default()
        ))
    }

    fn initialized_owner(configs: &ConfigManager, identity: &Identity) -> Result<()> {
        let node_id = NodeId::from_public_key(&identity.public_key_bytes());
        let credential_id = fuckipv6_common::domain::PasskeyCredentialId::from_public_key(
            fixture_passkey().cred_id(),
        );
        let mut store = super::super::store::OwnerStore::new(
            node_id,
            "Owner".to_string(),
            fixture_passkey(),
            false,
            false,
            1_700_000_000_000,
        )?;
        store.insert_session(StoredSession {
            reference: SessionRef::new([5u8; 32]),
            credential_id,
            channel: SessionChannel::Local,
            rp_id: "localhost".to_string(),
            origin: "http://localhost:8080".to_string(),
            created_at_unix_ms: 1_700_000_000_100,
            expires_at_unix_ms: 1_700_000_000_900,
            last_authenticated_at_unix_ms: 1_700_000_000_100,
        })?;
        OwnerPersistence::new(configs.config_dir().to_path_buf())?.commit(&store, &[9u8; 32])?;
        Ok(())
    }

    #[test]
    fn rotation_replaces_key_migrates_owner_and_invalidates_sessions() -> Result<()> {
        let dir = temp_dir("complete");
        let configs = ConfigManager::new_for_dir(dir.clone())?;
        let old_identity = configs.load_node_identity()?;
        let old_fingerprint = Identity::public_key_fingerprint(&old_identity.public_key_bytes());
        let old_node = NodeId::from_public_key(&old_identity.public_key_bytes());
        configs.set_node_display_name(Some("edge-gateway"))?;
        configs.set_node_language_preference(fuckipv6_common::i18n::LanguagePreference::ZhCn)?;
        initialized_owner(&configs, &old_identity)?;
        let verifier_path = dir.join("owner").join("recovery.verifier");
        let verifier_before = std::fs::read_to_string(&verifier_path)?;

        let rotation = rotate_node_identity(&configs, &old_fingerprint)?;
        assert_eq!(rotation.old_node_id, old_node.as_str());
        assert_eq!(rotation.old_fingerprint, old_fingerprint);
        assert!(rotation.owner_initialized);
        assert!(rotation.owner_sessions_invalidated);
        assert_ne!(rotation.old_node_id, rotation.new_node_id);
        assert_ne!(rotation.old_fingerprint, rotation.new_fingerprint);

        let new_identity = configs.load_node_identity()?;
        let new_node = NodeId::from_public_key(&new_identity.public_key_bytes());
        assert_eq!(new_node.as_str(), rotation.new_node_id);
        assert_eq!(
            Identity::public_key_fingerprint(&new_identity.public_key_bytes()),
            rotation.new_fingerprint
        );
        assert_eq!(
            configs.node_display_name()?.as_deref(),
            Some("edge-gateway")
        );
        assert_eq!(
            configs.node_language_preference()?,
            fuckipv6_common::i18n::LanguagePreference::ZhCn
        );
        assert_eq!(
            configs.node_identity_created_at_unix_ms()?,
            rotation.new_created_at_unix_ms
        );
        assert_eq!(std::fs::read_to_string(verifier_path)?, verifier_before);

        let OwnerLoad::Complete(owner) = OwnerPersistence::new(dir.clone())?.load()? else {
            panic!("owner must remain complete");
        };
        assert_eq!(owner.owner.node_id, new_node);
        assert_eq!(owner.owner.passkeys.len(), 1);
        assert!(owner.sessions.is_empty());
        assert!(owner.owner.sessions.is_empty());
        assert!(!dir.join(ROTATION_JOURNAL_FILE).exists());
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn wrong_expected_fingerprint_has_no_side_effects() -> Result<()> {
        let dir = temp_dir("wrong-fingerprint");
        let configs = ConfigManager::new_for_dir(dir.clone())?;
        let identity = configs.load_node_identity()?;
        let before = std::fs::read(configs.node_identity_path())?;
        let error = rotate_node_identity(&configs, "not-the-current-fingerprint")
            .expect_err("stale fingerprint must fail");
        assert!(error.to_string().contains("身份已变化"));
        assert_eq!(std::fs::read(configs.node_identity_path())?, before);
        assert_eq!(
            Identity::public_key_fingerprint(&identity.public_key_bytes()),
            Identity::public_key_fingerprint(
                &Identity::load_from_file(configs.node_identity_path())?.public_key_bytes()
            )
        );
        assert!(!dir.join(ROTATION_JOURNAL_FILE).exists());
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn journal_recovery_finishes_owner_migration_after_key_replacement() -> Result<()> {
        let dir = temp_dir("journal-recovery");
        let configs = ConfigManager::new_for_dir(dir.clone())?;
        let old_identity = configs.load_node_identity()?;
        initialized_owner(&configs, &old_identity)?;
        let replacement = Identity::generate()?;
        let created_at = unix_time_ms()?;
        let journal = RotationJournal::new(&old_identity, &replacement, created_at);
        write_journal(&configs, &journal)?;
        replacement.save_to_file(configs.node_identity_path())?;

        let recovered = load_reconciled_identity(&configs)?;
        assert_eq!(
            NodeId::from_public_key(&recovered.public_key_bytes()).as_str(),
            journal.new_node_id
        );
        assert_eq!(
            Identity::public_key_fingerprint(&recovered.public_key_bytes()),
            journal.new_fingerprint
        );
        assert_eq!(configs.node_identity_created_at_unix_ms()?, created_at);
        let OwnerLoad::Complete(owner) = OwnerPersistence::new(dir.clone())?.load()? else {
            panic!("owner must remain complete");
        };
        assert_eq!(owner.owner.node_id.as_str(), journal.new_node_id);
        assert!(owner.sessions.is_empty());
        assert!(!dir.join(ROTATION_JOURNAL_FILE).exists());
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
}
