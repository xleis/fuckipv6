//! Private persistence for paused requester-side File transfers.
//!
//! The journal contains local source/destination paths and must never be
//! projected into IPC snapshots, network frames, or logs. Download staging
//! files stay beside the final destination so publication remains an atomic
//! same-filesystem hard-link operation.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{bail, Context, Result};
use fuckipv6_common::domain::{
    ContentHash, FileShareId, NodeId, OperationId, SharedPath, TransferBinding, TransferDirection,
    TransferId, TransferManifest, TransferSession,
};
use serde::{Deserialize, Serialize};

use super::network_origin::OriginPeer;

const FILE_ORIGIN_JOURNAL_SCHEMA_VERSION: u16 = 1;
const FILE_ORIGIN_JOURNAL_FILE: &str = "file_origin_transfers.json";
const TEMP_FILE_PREFIX: &str = ".file_origin_transfers.json.tmp";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PersistedOriginTransfer {
    pub(crate) peer: PersistedOriginPeer,
    pub(crate) share_id: FileShareId,
    pub(crate) transfer_id: TransferId,
    pub(crate) operation_id: OperationId,
    pub(crate) path: SharedPath,
    pub(crate) direction: TransferDirection,
    pub(crate) deadline_unix_ms: u64,
    pub(crate) binding: TransferBinding,
    pub(crate) manifest: TransferManifest,
    pub(crate) next_request_sequence: u64,
    pub(crate) upload: Option<PersistedUploadState>,
    pub(crate) download: Option<PersistedDownloadState>,
}

impl fmt::Debug for PersistedOriginTransfer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PersistedOriginTransfer")
            .field("peer", &self.peer.node_id)
            .field("transfer_id", &self.transfer_id)
            .field("operation_id", &self.operation_id)
            .field("direction", &self.direction)
            .field("deadline_unix_ms", &self.deadline_unix_ms)
            .field("next_request_sequence", &self.next_request_sequence)
            .field(
                "upload",
                &self.upload.as_ref().map(|_| "[LOCAL PATH REDACTED]"),
            )
            .field(
                "download",
                &self.download.as_ref().map(|_| "[LOCAL PATH REDACTED]"),
            )
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PersistedOriginPeer {
    pub(crate) node_id: NodeId,
    pub(crate) virtual_ip: std::net::Ipv6Addr,
}

impl From<OriginPeer> for PersistedOriginPeer {
    fn from(peer: OriginPeer) -> Self {
        Self {
            node_id: peer.node_id,
            virtual_ip: peer.virtual_ip,
        }
    }
}

impl From<PersistedOriginPeer> for OriginPeer {
    fn from(peer: PersistedOriginPeer) -> Self {
        Self {
            node_id: peer.node_id,
            virtual_ip: peer.virtual_ip,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PersistedUploadState {
    pub(crate) source_path: PathBuf,
    pub(crate) next_chunk: u32,
    pub(crate) next_offset: u64,
    pub(crate) completed_chunks: u32,
}

impl fmt::Debug for PersistedUploadState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PersistedUploadState")
            .field("source_path", &"[REDACTED]")
            .field("next_chunk", &self.next_chunk)
            .field("next_offset", &self.next_offset)
            .field("completed_chunks", &self.completed_chunks)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PersistedDownloadState {
    pub(crate) destination: PathBuf,
    pub(crate) staging_path: PathBuf,
    pub(crate) session: TransferSession,
    pub(crate) assembly: Option<PersistedDownloadAssembly>,
    pub(crate) received_bytes: u64,
    pub(crate) received_chunks: u32,
}

impl fmt::Debug for PersistedDownloadState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PersistedDownloadState")
            .field("destination", &"[REDACTED]")
            .field("staging_path", &"[REDACTED]")
            .field("session", &self.session)
            .field("assembly", &self.assembly)
            .field("received_bytes", &self.received_bytes)
            .field("received_chunks", &self.received_chunks)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PersistedDownloadAssembly {
    pub(crate) chunk_index: u32,
    pub(crate) expected_offset: u64,
    pub(crate) expected_length: u64,
    pub(crate) expected_checksum: ContentHash,
    /// Number of assembly bytes currently appended after `received_bytes` in
    /// the staging file. They are truncated back out when the file is reopened.
    pub(crate) persisted_length: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PersistedOriginJournal {
    schema_version: u16,
    transfers: Vec<PersistedOriginTransfer>,
}

impl Default for PersistedOriginJournal {
    fn default() -> Self {
        Self {
            schema_version: FILE_ORIGIN_JOURNAL_SCHEMA_VERSION,
            transfers: Vec::new(),
        }
    }
}

/// Owner-only journal for one network runtime.
#[derive(Clone)]
pub(crate) struct FileOriginStore {
    path: PathBuf,
}

impl fmt::Debug for FileOriginStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FileOriginStore")
            .field("path", &self.path.file_name())
            .finish()
    }
}

impl FileOriginStore {
    pub(crate) fn at(config_dir: &Path, namespace: &str) -> Self {
        let safe_namespace = namespace
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                    character
                } else {
                    '_'
                }
            })
            .collect::<String>();
        Self {
            path: config_dir
                .join("file_origin")
                .join(format!("{safe_namespace}-{FILE_ORIGIN_JOURNAL_FILE}")),
        }
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn load(&self) -> Result<Vec<PersistedOriginTransfer>> {
        let Some(bytes) = self.read_private_file()? else {
            return Ok(Vec::new());
        };
        let journal: PersistedOriginJournal =
            serde_json::from_slice(&bytes).context("无法解析 File 续传日志")?;
        if journal.schema_version != FILE_ORIGIN_JOURNAL_SCHEMA_VERSION {
            bail!("不支持的 File 续传日志版本");
        }
        if journal.transfers.len() > super::network_origin::MAX_ORIGIN_TRANSFERS {
            bail!("File 续传日志超过活动传输上限");
        }
        Ok(journal.transfers)
    }

    pub(crate) fn save(&self, transfers: &[PersistedOriginTransfer]) -> Result<()> {
        if transfers.len() > super::network_origin::MAX_ORIGIN_TRANSFERS {
            bail!("File 续传日志超过活动传输上限");
        }
        let journal = PersistedOriginJournal {
            schema_version: FILE_ORIGIN_JOURNAL_SCHEMA_VERSION,
            transfers: transfers.to_vec(),
        };
        let bytes = serde_json::to_vec_pretty(&journal).context("无法序列化 File 续传日志")?;
        self.write_private_file_atomic(&bytes)
    }

    fn read_private_file(&self) -> Result<Option<Vec<u8>>> {
        let Some(parent) = self.path.parent() else {
            bail!("File 续传日志没有父目录");
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
            Err(error) => return Err(error).context("无法打开 File 续传日志"),
        };
        validate_private_file(&file)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .context("无法读取 File 续传日志")?;
        Ok(Some(bytes))
    }

    fn write_private_file_atomic(&self, content: &[u8]) -> Result<()> {
        let parent = self.path.parent().context("File 续传日志没有父目录")?;
        let file_name = self.path.file_name().context("File 续传日志没有文件名")?;
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
                .context("无法创建 File 续传日志临时文件")?;
            set_private_file_permissions(&file)?;
            file.write_all(content)
                .context("无法写入 File 续传日志临时文件")?;
            file.sync_all().context("无法同步 File 续传日志临时文件")?;
            validate_private_file(&file)?;
            drop(file);
            fs::rename(&temporary_path, &self.path).context("无法原子替换 File 续传日志")?;
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
    fs::create_dir_all(path).context("无法创建 File 续传日志目录")?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .context("无法保护 File 续传日志目录")
}

#[cfg(not(unix))]
fn secure_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path).context("无法创建 File 续传日志目录")
}

fn validate_private_file(file: &File) -> Result<()> {
    let metadata = file.metadata().context("无法读取 File 续传日志元数据")?;
    if !metadata.is_file() {
        bail!("File 续传日志不是普通文件");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            bail!("File 续传日志权限不是 owner-only");
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions(file: &File) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .context("无法保护 File 续传日志临时文件")
}

#[cfg(not(unix))]
fn set_private_file_permissions(_file: &File) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .context("无法同步 File 续传日志目录")
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use fuckipv6_common::domain::{
        FileChunk, FileId, SharedPath, TransferDirection, TransferManifest,
    };
    use tempfile::TempDir;

    use super::*;

    fn persisted_transfer(seed: u8) -> PersistedOriginTransfer {
        let manifest = TransferManifest {
            file_id: FileId::from_public_key(&[seed]),
            total_bytes: 3,
            chunk_size: 3,
            content_hash: ContentHash::from_bytes(b"abc"),
            chunks: vec![FileChunk {
                index: 0,
                offset: 0,
                length: 3,
                checksum: ContentHash::from_bytes(b"abc"),
            }],
        };
        let transfer_id = TransferId::from_public_key(&[seed]);
        let operation_id = OperationId::from_public_key(&[seed]);
        let share_id = FileShareId::from_public_key(&[seed]);
        let actor = NodeId::from_public_key(&[seed]);
        let path = SharedPath::parse("docs/file.bin").expect("shared path");
        PersistedOriginTransfer {
            peer: PersistedOriginPeer {
                node_id: NodeId::from_public_key(&[seed.wrapping_add(1)]),
                virtual_ip: std::net::Ipv6Addr::LOCALHOST,
            },
            share_id: share_id.clone(),
            transfer_id: transfer_id.clone(),
            operation_id: operation_id.clone(),
            path: path.clone(),
            direction: TransferDirection::Upload,
            deadline_unix_ms: 5_000,
            binding: TransferBinding {
                transfer_id,
                operation_id,
                share_id,
                actor,
                access_epoch: 7,
                path,
                direction: TransferDirection::Upload,
                file_version: None,
                share_revision: 1,
                expires_at_unix_ms: 5_000,
            },
            manifest,
            next_request_sequence: 2,
            upload: Some(PersistedUploadState {
                source_path: PathBuf::from("/private/source.bin"),
                next_chunk: 0,
                next_offset: 0,
                completed_chunks: 0,
            }),
            download: None,
        }
    }

    #[test]
    fn journal_round_trips_with_owner_only_permissions() {
        let temporary = TempDir::new().expect("tempdir");
        let store = FileOriginStore::at(temporary.path(), "network/one");
        let expected = vec![persisted_transfer(1)];
        store.save(&expected).expect("save journal");

        assert_eq!(store.load().expect("load journal"), expected);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let file_mode = std::fs::metadata(store.path())
                .expect("journal metadata")
                .permissions()
                .mode()
                & 0o777;
            let directory_mode = std::fs::metadata(store.path().parent().expect("journal parent"))
                .expect("journal directory metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(file_mode, 0o600);
            assert_eq!(directory_mode, 0o700);
        }
    }

    #[test]
    fn journal_rejects_group_or_world_accessible_permissions() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let temporary = TempDir::new().expect("tempdir");
            let store = FileOriginStore::at(temporary.path(), "network");
            store.save(&[]).expect("save journal");
            std::fs::set_permissions(store.path(), fs::Permissions::from_mode(0o644))
                .expect("broaden permissions");
            assert!(store.load().is_err());
        }
    }

    #[test]
    fn journal_rejects_more_than_the_transfer_limit() {
        let temporary = TempDir::new().expect("tempdir");
        let store = FileOriginStore::at(temporary.path(), "network");
        let journal = PersistedOriginJournal {
            schema_version: FILE_ORIGIN_JOURNAL_SCHEMA_VERSION,
            transfers: (0..=super::super::network_origin::MAX_ORIGIN_TRANSFERS)
                .map(|index| persisted_transfer(index as u8))
                .collect(),
        };
        let parent = store.path().parent().expect("journal parent");
        fs::create_dir_all(parent).expect("journal dir");
        let bytes = serde_json::to_vec(&journal).expect("serialize oversized journal");
        fs::write(store.path(), bytes).expect("write oversized journal");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(store.path(), fs::Permissions::from_mode(0o600))
                .expect("private permissions");
        }
        assert!(store.load().is_err());
    }

    #[test]
    fn debug_output_redacts_local_paths() {
        let temporary = TempDir::new().expect("tempdir");
        let store = FileOriginStore::at(temporary.path(), "network");
        let transfer = persisted_transfer(1);
        let store_debug = format!("{store:?}");
        let transfer_debug = format!("{transfer:?}");
        assert!(!store_debug.contains(temporary.path().to_string_lossy().as_ref()));
        assert!(!transfer_debug.contains("/private/source.bin"));
    }
}
