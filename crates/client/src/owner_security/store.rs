use anyhow::{Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use fs2::FileExt;
use fuckipv6_common::domain::{
    NodeId, NodeOwner, PasskeyCredentialId, PasskeyCredentialRef, RecoveryState, SecretRef,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use subtle::ConstantTimeEq;
use webauthn_rs::prelude::Passkey;

const OWNER_SCHEMA_VERSION: u32 = 1;
const OWNER_DIR: &str = "owner";
const OWNER_FILE: &str = "owner.json";
const RECOVERY_VERIFIER_FILE: &str = "recovery.verifier";
const INIT_LOCK_FILE: &str = ".owner-init.lock";
const BOOTSTRAP_VERIFIER_FILE: &str = "owner-bootstrap.verifier";
const RECOVERY_SECRET_REF: &str = "protected_file:owner/recovery.verifier";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredPasskey {
    pub(crate) reference: PasskeyCredentialRef,
    pub(crate) credential: Passkey,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OwnerStore {
    pub(crate) schema_version: u32,
    pub(crate) owner: NodeOwner,
    pub(crate) passkeys: Vec<StoredPasskey>,
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

        match &self.owner.recovery {
            RecoveryState::Configured { secret_ref, .. }
                if secret_ref.as_str() == RECOVERY_SECRET_REF => {}
            _ => anyhow::bail!("owner recovery verifier reference is invalid"),
        }
        Ok(())
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
mod tests {
    use super::*;
    use std::io::Read;

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
}
