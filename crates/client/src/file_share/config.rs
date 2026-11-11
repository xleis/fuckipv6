//! Owner-only persistence for the local File share selection.
//!
//! The selected root path is intentionally kept in this local file rather than
//! `NodeMetadata`, command JSON, snapshots, activity records, or logs. The file
//! and its containing directory are private to the node-owner account.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{bail, Context, Result};
use fuckipv6_common::domain::FileShare;
use serde::{Deserialize, Serialize};

pub(crate) const FILE_SHARE_CONFIG_SCHEMA_VERSION: u16 = 1;
pub(crate) const FILE_SHARE_CONFIG_FILE: &str = "file_share.json";
const TEMP_FILE_PREFIX: &str = ".file_share.json.tmp";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PersistedFileShareConfig {
    pub schema_version: u16,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<FileShare>,
    pub updated_at_unix_ms: u64,
}

impl fmt::Debug for PersistedFileShareConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PersistedFileShareConfig")
            .field("schema_version", &self.schema_version)
            .field("enabled", &self.enabled)
            .field(
                "root_reference",
                &self.root_reference.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "share_revision",
                &self.share.as_ref().map(|share| share.revision),
            )
            .field("updated_at_unix_ms", &self.updated_at_unix_ms)
            .finish()
    }
}

impl Default for PersistedFileShareConfig {
    fn default() -> Self {
        Self {
            schema_version: FILE_SHARE_CONFIG_SCHEMA_VERSION,
            enabled: false,
            root_reference: None,
            share: None,
            updated_at_unix_ms: 0,
        }
    }
}

impl PersistedFileShareConfig {
    pub(crate) fn disabled(now_unix_ms: u64) -> Self {
        Self {
            updated_at_unix_ms: now_unix_ms,
            ..Self::default()
        }
    }

    pub(crate) fn enabled(root_reference: String, share: FileShare, now_unix_ms: u64) -> Self {
        Self {
            schema_version: FILE_SHARE_CONFIG_SCHEMA_VERSION,
            enabled: true,
            root_reference: Some(root_reference),
            share: Some(share),
            updated_at_unix_ms: now_unix_ms,
        }
    }

    fn validate(&self) -> Result<()> {
        if self.schema_version != FILE_SHARE_CONFIG_SCHEMA_VERSION {
            bail!("unsupported File share configuration schema");
        }
        if self.enabled {
            let root = self
                .root_reference
                .as_deref()
                .context("enabled File share configuration has no root")?;
            if root.is_empty() {
                bail!("enabled File share configuration has an empty root");
            }
            self.share
                .as_ref()
                .context("enabled File share configuration has no share")?
                .validate()
                .context("persisted File share is invalid")?;
        }
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct FileShareConfigStore {
    path: PathBuf,
}

impl fmt::Debug for FileShareConfigStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FileShareConfigStore")
            .field("path", &self.path.file_name())
            .finish()
    }
}

impl FileShareConfigStore {
    pub(crate) fn at(config_dir: &Path) -> Self {
        Self {
            path: config_dir.join(FILE_SHARE_CONFIG_FILE),
        }
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn load(&self) -> Result<PersistedFileShareConfig> {
        match self.read_private_file() {
            Ok(Some(bytes)) => {
                let config: PersistedFileShareConfig =
                    serde_json::from_slice(&bytes).context("无法解析 File 共享配置")?;
                config.validate()?;
                Ok(config)
            }
            Ok(None) => Ok(PersistedFileShareConfig::default()),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn save(&self, config: &PersistedFileShareConfig) -> Result<()> {
        config.validate()?;
        let bytes = serde_json::to_vec_pretty(config).context("无法序列化 File 共享配置")?;
        self.write_private_file_atomic(&bytes)
    }

    fn read_private_file(&self) -> Result<Option<Vec<u8>>> {
        let Some(parent) = self.path.parent() else {
            bail!("File 共享配置没有父目录");
        };
        secure_directory(parent)?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        let mut file = match options.open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("无法打开 File 共享配置"),
        };
        validate_private_file(&file)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .context("无法读取 File 共享配置")?;
        Ok(Some(bytes))
    }

    fn write_private_file_atomic(&self, content: &[u8]) -> Result<()> {
        let parent = self.path.parent().context("File 共享配置没有父目录")?;
        let file_name = self.path.file_name().context("File 共享配置没有文件名")?;
        secure_directory(parent)?;

        let mut temporary_name = file_name.to_os_string();
        temporary_name.push(format!(
            "{}-{}-{}",
            TEMP_FILE_PREFIX,
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let temporary_path = parent.join(temporary_name);
        let result = (|| -> Result<()> {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
            }
            let mut file = options
                .open(&temporary_path)
                .context("无法创建 File 共享配置临时文件")?;
            set_private_file_permissions(&file)?;
            file.write_all(content)
                .context("无法写入 File 共享配置临时文件")?;
            file.sync_all().context("无法同步 File 共享配置临时文件")?;
            validate_private_file(&file)?;
            drop(file);
            fs::rename(&temporary_path, &self.path).context("无法原子替换 File 共享配置")?;
            sync_directory(parent)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary_path);
        }
        result
    }
}

#[cfg(unix)]
fn secure_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(path).context("无法创建 File 共享配置目录")?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .context("无法保护 File 共享配置目录")
}

#[cfg(not(unix))]
fn secure_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path).context("无法创建 File 共享配置目录")
}

fn validate_private_file(file: &File) -> Result<()> {
    let metadata = file.metadata().context("无法读取 File 共享配置元数据")?;
    if !metadata.is_file() {
        bail!("File 共享配置不是普通文件");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            bail!("File 共享配置权限不是 owner-only");
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions(file: &File) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .context("无法保护 File 共享配置临时文件")
}

#[cfg(not(unix))]
fn set_private_file_permissions(_file: &File) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .context("无法同步 File 共享配置目录")
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use fuckipv6_common::domain::{
        ConcurrentNamePolicy, FileAcl, FileDeletePolicy, FileQuota, FileShareId, FileShareMode,
        NetworkId, NodeId, SymlinkPolicy, FILE_SHARE_SCHEMA_VERSION,
    };

    use super::*;

    fn share() -> FileShare {
        FileShare {
            schema_version: FILE_SHARE_SCHEMA_VERSION,
            id: FileShareId::from_public_key(b"file-share-config-test"),
            network_id: NetworkId::from_public_key(b"file-share-network"),
            owner: NodeId::from_public_key(b"file-share-owner"),
            display_name: "Documents".into(),
            mode: FileShareMode::ReadOnly,
            revision: 1,
            network_acl: FileAcl::default(),
            local_acl: FileAcl::default(),
            quota: FileQuota::default(),
            symlink_policy: SymlinkPolicy::Reject,
            concurrent_name_policy: ConcurrentNamePolicy::Reject,
            delete_policy: FileDeletePolicy::default(),
            audit_enabled: true,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        }
    }

    #[test]
    fn missing_config_is_disabled() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let store = FileShareConfigStore::at(temporary.path());
        let config = store.load().expect("load missing config");
        assert!(!config.enabled);
        assert!(config.root_reference.is_none());
        assert!(config.share.is_none());
    }

    #[test]
    fn round_trips_owner_only_configuration() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let store = FileShareConfigStore::at(temporary.path());
        let expected =
            PersistedFileShareConfig::enabled(temporary.path().display().to_string(), share(), 2);
        store.save(&expected).expect("save");
        assert_eq!(store.load().expect("load"), expected);
        assert!(!format!("{expected:?}").contains(temporary.path().to_str().expect("utf8")));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(store.path())
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn rejects_insecure_permissions() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let store = FileShareConfigStore::at(temporary.path());
        store
            .save(&PersistedFileShareConfig::disabled(1))
            .expect("save");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(store.path(), fs::Permissions::from_mode(0o644)).expect("chmod");
            assert!(store.load().is_err());
        }
    }

    #[test]
    fn rejects_corrupt_or_incomplete_enabled_configuration() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let store = FileShareConfigStore::at(temporary.path());
        fs::write(store.path(), b"not-json").expect("write corrupt");
        assert!(store.load().is_err());

        let incomplete = PersistedFileShareConfig {
            enabled: true,
            ..PersistedFileShareConfig::disabled(1)
        };
        assert!(store.save(&incomplete).is_err());
    }

    #[test]
    fn share_acl_defaults_to_no_members() {
        let share = share();
        assert!(share.network_acl.rules.is_empty());
        assert!(share.local_acl.rules.is_empty());
        assert_eq!(share.mode, FileShareMode::ReadOnly);
        assert_eq!(share.symlink_policy, SymlinkPolicy::Reject);
        assert!(share.audit_enabled);
        let actions: BTreeSet<_> = share
            .network_acl
            .rules
            .iter()
            .flat_map(|rule| rule.actions.iter().copied())
            .collect();
        assert!(actions.is_empty());
    }
}
