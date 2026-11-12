//! Operation-aware lifecycle for in-process File transfers.
//!
//! The filesystem adapter owns authorization checkpoints and chunk I/O. This
//! runtime adds the process-level admission boundary: one active transfer per
//! transfer/operation identity, a bounded active-transfer table, and handles
//! that release admission on completion, failure, cancellation or drop.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use fuckipv6_common::domain::{
    AccessContext, FileQuotaError, NodeId, OperationId, SharedPath, TransferSession,
};

use super::capability::{FileCapabilityError, FileCapabilitySnapshot, FileShareCapability};
use super::transfer::{
    DownloadTransfer, FileTransferAdapter, FileTransferAdapterError, TransferOperationCancellation,
    TransferProgress, UploadAdmission, UploadTransfer,
};

/// User-safe runtime projection. It contains no local root or peer details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRuntimeSnapshot {
    pub capability: FileCapabilitySnapshot,
    pub active_transfers: u32,
}

/// Stable runtime failures in addition to the adapter's projected errors.
#[derive(Debug, thiserror::Error)]
pub enum FileRuntimeError {
    #[error(transparent)]
    Transfer(#[from] FileTransferAdapterError),
    #[error("the transfer or operation already has an active transfer")]
    AlreadyActive,
    #[error("File runtime state is unavailable")]
    Unavailable,
}

impl FileRuntimeError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Transfer(error) => error.code(),
            Self::AlreadyActive => "file_transfer_operation_active",
            Self::Unavailable => "file_runtime_unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ConcurrencyCounts {
    total: u32,
    member: u32,
    directory: u32,
}

#[derive(Debug, Clone)]
struct ActiveTransfer {
    operation_id: OperationId,
    actor: NodeId,
    directory: SharedPath,
}

/// Coordinates all File transfers admitted by one node process.
#[derive(Clone)]
pub struct FileRuntime {
    capability: FileShareCapability,
    adapter: FileTransferAdapter,
    active: Arc<Mutex<BTreeMap<String, ActiveTransfer>>>,
}

impl std::fmt::Debug for FileRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FileRuntime")
            .field("snapshot", &self.snapshot())
            .finish()
    }
}

impl FileRuntime {
    pub fn new(capability: FileShareCapability) -> Self {
        Self {
            adapter: FileTransferAdapter::new(capability.clone()),
            capability,
            active: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn snapshot(&self) -> FileRuntimeSnapshot {
        FileRuntimeSnapshot {
            capability: self.capability.snapshot(),
            active_transfers: self.active_count(),
        }
    }

    pub fn capability(&self) -> &FileShareCapability {
        &self.capability
    }

    /// Enable the owner-selected root. Any transfer admitted against the prior
    /// capability epoch stays bound to its own revoked permit; it no longer
    /// consumes admission for the replacement configuration.
    pub fn enable(
        &self,
        share: &fuckipv6_common::domain::FileShare,
        actor: &fuckipv6_common::domain::NodeId,
        root_reference: &str,
        now_unix_ms: u64,
    ) -> Result<FileRuntimeSnapshot, FileCapabilityError> {
        self.capability
            .enable(share, actor, root_reference, now_unix_ms)?;
        self.clear_active();
        Ok(self.snapshot())
    }

    /// Disable the capability and clear admission for its now-revoked epoch.
    /// Existing handles still observe the capability epoch at their next
    /// authorization boundary and cannot resume after a re-enable.
    pub fn disable(
        &self,
        actor: &fuckipv6_common::domain::NodeId,
        now_unix_ms: u64,
    ) -> Result<FileRuntimeSnapshot, FileCapabilityError> {
        self.capability.disable(actor, now_unix_ms)?;
        self.clear_active();
        Ok(self.snapshot())
    }

    /// Begin an upload whose cancellation is bound to the authenticated
    /// operation that created the transfer session.
    pub fn begin_upload_cancellable(
        &self,
        session: TransferSession,
        context: &AccessContext,
        network_enabled: bool,
        admission: UploadAdmission,
        cancellation: TransferOperationCancellation,
        now_unix_ms: u64,
    ) -> Result<RuntimeUploadTransfer, FileRuntimeError> {
        let admission_counts = self.reserve(&session)?;
        let admission = UploadAdmission {
            concurrent_transfers: admission_counts.total,
            member_concurrent_transfers: admission_counts.member,
            directory_concurrent_transfers: admission_counts.directory,
            ..admission
        };
        let transfer_id = session.binding.transfer_id.as_str().to_owned();

        match self.adapter.begin_upload_cancellable(
            session,
            context,
            network_enabled,
            admission,
            cancellation,
            now_unix_ms,
        ) {
            Ok(inner) => Ok(RuntimeUploadTransfer {
                runtime: self.clone(),
                transfer_id,
                inner: Some(inner),
            }),
            Err(error) => {
                self.release(&transfer_id);
                Err(error.into())
            }
        }
    }

    /// Begin a download whose cancellation is bound to the authenticated
    /// operation that created the transfer session.
    pub fn begin_download_cancellable(
        &self,
        session: TransferSession,
        context: &AccessContext,
        network_enabled: bool,
        cancellation: TransferOperationCancellation,
        now_unix_ms: u64,
    ) -> Result<RuntimeDownloadTransfer, FileRuntimeError> {
        self.reserve(&session)?;
        let transfer_id = session.binding.transfer_id.as_str().to_owned();

        match self.adapter.begin_download_cancellable(
            session,
            context,
            network_enabled,
            cancellation,
            now_unix_ms,
        ) {
            Ok(inner) => Ok(RuntimeDownloadTransfer {
                runtime: self.clone(),
                transfer_id,
                inner: Some(inner),
            }),
            Err(error) => {
                self.release(&transfer_id);
                Err(error.into())
            }
        }
    }

    pub fn active_transfer_count(&self) -> u32 {
        self.active_count()
    }

    fn reserve(&self, session: &TransferSession) -> Result<ConcurrencyCounts, FileRuntimeError> {
        let share = self
            .capability
            .current_share()
            .ok_or(FileTransferAdapterError::Capability(
                FileCapabilityError::Disabled,
            ))?;
        let mut active = self
            .active
            .lock()
            .map_err(|_| FileRuntimeError::Unavailable)?;
        let transfer_id = session.binding.transfer_id.as_str();
        if active.contains_key(transfer_id)
            || active
                .values()
                .any(|entry| entry.operation_id == session.binding.operation_id)
        {
            return Err(FileRuntimeError::AlreadyActive);
        }
        let directory = session
            .binding
            .path
            .parent()
            .unwrap_or_else(SharedPath::root);
        let member_count = active
            .values()
            .filter(|entry| entry.actor == session.binding.actor)
            .count();
        let directory_count = active
            .values()
            .filter(|entry| entry.directory == directory)
            .count();
        if active.len()
            >= usize::try_from(share.quota.max_concurrent_transfers).unwrap_or(usize::MAX)
        {
            return Err(FileTransferAdapterError::Quota(FileQuotaError::ConcurrencyLimit).into());
        }
        if member_count
            >= usize::try_from(share.quota.max_concurrent_transfers_per_member)
                .unwrap_or(usize::MAX)
        {
            return Err(
                FileTransferAdapterError::Quota(FileQuotaError::MemberConcurrencyLimit).into(),
            );
        }
        if directory_count
            >= usize::try_from(share.quota.max_concurrent_transfers_per_directory)
                .unwrap_or(usize::MAX)
        {
            return Err(
                FileTransferAdapterError::Quota(FileQuotaError::DirectoryConcurrencyLimit).into(),
            );
        }
        let counts = ConcurrencyCounts {
            total: u32::try_from(active.len()).map_err(|_| FileRuntimeError::Unavailable)?,
            member: u32::try_from(member_count).map_err(|_| FileRuntimeError::Unavailable)?,
            directory: u32::try_from(directory_count).map_err(|_| FileRuntimeError::Unavailable)?,
        };
        active.insert(
            transfer_id.to_owned(),
            ActiveTransfer {
                operation_id: session.binding.operation_id.clone(),
                actor: session.binding.actor.clone(),
                directory,
            },
        );
        Ok(counts)
    }

    fn release(&self, transfer_id: &str) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(transfer_id);
        }
    }

    fn clear_active(&self) {
        if let Ok(mut active) = self.active.lock() {
            active.clear();
        }
    }

    fn active_count(&self) -> u32 {
        self.active
            .lock()
            .map(|active| u32::try_from(active.len()).unwrap_or(u32::MAX))
            .unwrap_or(u32::MAX)
    }
}

/// One admitted upload. Dropping the handle always releases its admission slot;
/// the adapter separately removes any unpublished temporary file.
#[derive(Debug)]
pub struct RuntimeUploadTransfer {
    runtime: FileRuntime,
    transfer_id: String,
    inner: Option<UploadTransfer>,
}

impl RuntimeUploadTransfer {
    pub fn session(&self) -> &TransferSession {
        self.inner
            .as_ref()
            .expect("upload transfer is present")
            .session()
    }

    pub fn progress(&self) -> TransferProgress {
        self.inner
            .as_ref()
            .expect("upload transfer is present")
            .progress()
    }

    pub fn write_chunk(
        &mut self,
        index: u32,
        bytes: &[u8],
        now_unix_ms: u64,
    ) -> Result<bool, FileTransferAdapterError> {
        let result = self
            .inner
            .as_mut()
            .expect("upload transfer is present")
            .write_chunk(index, bytes, now_unix_ms);
        self.release_if_terminal();
        result
    }

    pub fn pause(&mut self, now_unix_ms: u64) -> Result<(), FileTransferAdapterError> {
        self.inner
            .as_mut()
            .expect("upload transfer is present")
            .pause(now_unix_ms)
    }

    pub fn resume(
        &mut self,
        context: &AccessContext,
        network_enabled: bool,
        now_unix_ms: u64,
    ) -> Result<(), FileTransferAdapterError> {
        let result = self
            .inner
            .as_mut()
            .expect("upload transfer is present")
            .resume(context, network_enabled, now_unix_ms);
        self.release_if_terminal();
        result
    }

    pub fn finish(
        &mut self,
        now_unix_ms: u64,
    ) -> Result<TransferProgress, FileTransferAdapterError> {
        let result = self
            .inner
            .as_mut()
            .expect("upload transfer is present")
            .finish(now_unix_ms);
        self.release_if_terminal();
        result
    }

    pub fn cancel(mut self, now_unix_ms: u64) -> Result<(), FileTransferAdapterError> {
        let result = self
            .inner
            .take()
            .expect("upload transfer is present")
            .cancel(now_unix_ms);
        self.release();
        result
    }

    fn release_if_terminal(&mut self) {
        if self.session().state.is_terminal() {
            self.release();
        }
    }

    fn release(&mut self) {
        self.runtime.release(&self.transfer_id);
    }
}

impl Drop for RuntimeUploadTransfer {
    fn drop(&mut self) {
        self.release();
    }
}

/// One admitted download. The source file handle remains owned by the adapter
/// and is closed on completion, failure, cancellation or drop.
#[derive(Debug)]
pub struct RuntimeDownloadTransfer {
    runtime: FileRuntime,
    transfer_id: String,
    inner: Option<DownloadTransfer>,
}

impl RuntimeDownloadTransfer {
    pub fn session(&self) -> &TransferSession {
        self.inner
            .as_ref()
            .expect("download transfer is present")
            .session()
    }

    pub fn progress(&self) -> TransferProgress {
        self.inner
            .as_ref()
            .expect("download transfer is present")
            .progress()
    }

    pub fn read_next(
        &mut self,
        now_unix_ms: u64,
    ) -> Result<Option<Vec<u8>>, FileTransferAdapterError> {
        let result = self
            .inner
            .as_mut()
            .expect("download transfer is present")
            .read_next(now_unix_ms);
        self.release_if_terminal();
        result
    }

    pub fn pause(&mut self, now_unix_ms: u64) -> Result<(), FileTransferAdapterError> {
        self.inner
            .as_mut()
            .expect("download transfer is present")
            .pause(now_unix_ms)
    }

    pub fn resume(
        &mut self,
        context: &AccessContext,
        network_enabled: bool,
        now_unix_ms: u64,
    ) -> Result<(), FileTransferAdapterError> {
        let result = self
            .inner
            .as_mut()
            .expect("download transfer is present")
            .resume(context, network_enabled, now_unix_ms);
        self.release_if_terminal();
        result
    }

    pub fn finish(
        &mut self,
        now_unix_ms: u64,
    ) -> Result<TransferProgress, FileTransferAdapterError> {
        let result = self
            .inner
            .as_mut()
            .expect("download transfer is present")
            .finish(now_unix_ms);
        self.release_if_terminal();
        result
    }

    pub fn cancel(mut self, now_unix_ms: u64) -> Result<(), FileTransferAdapterError> {
        let result = self
            .inner
            .take()
            .expect("download transfer is present")
            .cancel(now_unix_ms);
        self.release();
        result
    }

    fn release_if_terminal(&mut self) {
        if self.session().state.is_terminal() {
            self.release();
        }
    }

    fn release(&mut self) {
        self.runtime.release(&self.transfer_id);
    }
}

impl Drop for RuntimeDownloadTransfer {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use fuckipv6_common::domain::{
        ContentHash, FileAcl, FileAclEffect, FileAclRule, FileChunk, FileDeletePolicy, FileId,
        FileQuota, FileShare, FileShareId, FileShareMode, FileVersion, NetworkId, NetworkRole,
        NodeId, OperationId, SharedPath, SubjectSelector, SymlinkPolicy, TransferBinding,
        TransferDirection, TransferId, TransferManifest, FILE_SHARE_SCHEMA_VERSION,
    };
    use tempfile::TempDir;
    use tokio::sync::watch;

    use super::*;
    use crate::file_share::{FileCapabilityState, FILE_TEMP_DIRECTORY};

    fn owner() -> NodeId {
        NodeId::from_public_key(b"runtime-owner")
    }

    fn operation(byte: u8) -> OperationId {
        OperationId::from_public_key(&[byte])
    }

    fn transfer(byte: u8) -> TransferId {
        TransferId::from_public_key(&[byte])
    }

    fn allow_acl() -> FileAcl {
        FileAcl {
            rules: vec![FileAclRule {
                id: "allow-members".into(),
                subjects: BTreeSet::from([SubjectSelector::AnyMember]),
                path_prefix: SharedPath::root(),
                actions: BTreeSet::from([
                    fuckipv6_common::domain::FileAction::List,
                    fuckipv6_common::domain::FileAction::ReadMetadata,
                    fuckipv6_common::domain::FileAction::Download,
                    fuckipv6_common::domain::FileAction::Upload,
                    fuckipv6_common::domain::FileAction::CreateDirectory,
                    fuckipv6_common::domain::FileAction::Rename,
                    fuckipv6_common::domain::FileAction::Delete,
                ]),
                effect: FileAclEffect::Allow,
            }],
        }
    }

    fn share() -> FileShare {
        FileShare {
            schema_version: FILE_SHARE_SCHEMA_VERSION,
            id: FileShareId::from_public_key(b"runtime-share"),
            network_id: NetworkId::from_public_key(b"runtime-network"),
            owner: owner(),
            display_name: "Team files".into(),
            mode: FileShareMode::ReadWrite,
            revision: 1,
            network_acl: allow_acl(),
            local_acl: allow_acl(),
            quota: FileQuota::default(),
            symlink_policy: SymlinkPolicy::Reject,
            concurrent_name_policy: fuckipv6_common::domain::ConcurrentNamePolicy::Reject,
            delete_policy: FileDeletePolicy::default(),
            audit_enabled: true,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        }
    }

    fn context(share: &FileShare) -> AccessContext {
        AccessContext {
            network_id: share.network_id.clone(),
            actor: owner(),
            role: NetworkRole::Owner,
            groups: BTreeSet::new(),
            membership_active: true,
            recent_reauthentication: true,
            emergency_coordinator: false,
        }
    }

    fn member_context(share: &FileShare, actor: NodeId) -> AccessContext {
        AccessContext {
            network_id: share.network_id.clone(),
            actor,
            role: NetworkRole::Member,
            groups: BTreeSet::new(),
            membership_active: true,
            recent_reauthentication: true,
            emergency_coordinator: false,
        }
    }

    fn setup() -> (TempDir, FileRuntime, FileShare) {
        setup_with(share())
    }

    fn setup_with(share: FileShare) -> (TempDir, FileRuntime, FileShare) {
        let temporary = tempfile::tempdir().expect("tempdir");
        let runtime = FileRuntime::new(FileShareCapability::new());
        runtime
            .enable(
                &share,
                &owner(),
                temporary.path().to_str().expect("utf8 path"),
                1,
            )
            .expect("enable File runtime");
        (temporary, runtime, share)
    }

    fn manifest(content: &[u8], chunk_size: u64) -> TransferManifest {
        let chunks = if content.is_empty() {
            Vec::new()
        } else {
            content
                .chunks(usize::try_from(chunk_size).expect("chunk size"))
                .enumerate()
                .map(|(index, bytes)| FileChunk {
                    index: u32::try_from(index).expect("chunk index"),
                    offset: u64::try_from(index).expect("offset") * chunk_size,
                    length: u64::try_from(bytes.len()).expect("chunk length"),
                    checksum: ContentHash::from_bytes(bytes),
                })
                .collect()
        };
        TransferManifest {
            file_id: FileId::from_public_key(b"runtime-file"),
            total_bytes: u64::try_from(content.len()).expect("content length"),
            chunk_size,
            content_hash: ContentHash::from_bytes(content),
            chunks,
        }
    }

    struct UploadSessionSpec<'a> {
        share: &'a FileShare,
        actor: NodeId,
        path: &'a str,
        content: &'a [u8],
        chunk_size: u64,
        transfer_id: TransferId,
        operation_id: OperationId,
        expires_at_unix_ms: u64,
    }

    fn upload_session(
        share: &FileShare,
        path: &str,
        content: &[u8],
        chunk_size: u64,
        transfer_byte: u8,
        operation_byte: u8,
        expires_at_unix_ms: u64,
    ) -> TransferSession {
        upload_session_for(UploadSessionSpec {
            share,
            actor: owner(),
            path,
            content,
            chunk_size,
            transfer_id: transfer(transfer_byte),
            operation_id: operation(operation_byte),
            expires_at_unix_ms,
        })
    }

    fn upload_session_for(spec: UploadSessionSpec<'_>) -> TransferSession {
        TransferSession::new(
            TransferBinding {
                transfer_id: spec.transfer_id,
                operation_id: spec.operation_id,
                share_id: spec.share.id.clone(),
                actor: spec.actor,
                access_epoch: 1,
                path: SharedPath::parse(spec.path).expect("path"),
                direction: TransferDirection::Upload,
                file_version: None,
                share_revision: spec.share.revision,
                expires_at_unix_ms: spec.expires_at_unix_ms,
            },
            manifest(spec.content, spec.chunk_size),
            10,
        )
        .expect("upload session")
    }

    fn download_session(
        share: &FileShare,
        path: &str,
        content: &[u8],
        chunk_size: u64,
        transfer_byte: u8,
        operation_byte: u8,
        expires_at_unix_ms: u64,
    ) -> TransferSession {
        TransferSession::new(
            TransferBinding {
                transfer_id: transfer(transfer_byte),
                operation_id: operation(operation_byte),
                share_id: share.id.clone(),
                actor: owner(),
                access_epoch: 1,
                path: SharedPath::parse(path).expect("path"),
                direction: TransferDirection::Download,
                file_version: Some(FileVersion {
                    content_hash: ContentHash::from_bytes(content),
                    size: u64::try_from(content.len()).expect("content length"),
                    revision: 1,
                }),
                share_revision: share.revision,
                expires_at_unix_ms,
            },
            manifest(content, chunk_size),
            10,
        )
        .expect("download session")
    }

    #[test]
    fn runtime_uses_authoritative_active_count_and_releases_on_finish() {
        let mut share = share();
        share.quota.max_concurrent_transfers = 1;
        share.quota.max_concurrent_transfers_per_member = 1;
        share.quota.max_concurrent_transfers_per_directory = 1;
        let (_temporary, runtime, share) = setup_with(share);
        let (_tx, cancel) = watch::channel(false);
        let mut first = runtime
            .begin_upload_cancellable(
                upload_session(&share, "first.txt", b"abcd", 2, 1, 1, 1_000),
                &context(&share),
                true,
                UploadAdmission::default(),
                TransferOperationCancellation::new(operation(1), cancel),
                20,
            )
            .expect("first upload");
        assert_eq!(runtime.active_transfer_count(), 1);

        let (_tx, cancel) = watch::channel(false);
        let error = runtime
            .begin_upload_cancellable(
                upload_session(&share, "second.txt", b"abcd", 2, 2, 2, 1_000),
                &context(&share),
                true,
                UploadAdmission::default(),
                TransferOperationCancellation::new(operation(2), cancel),
                20,
            )
            .expect_err("concurrency limit");
        assert_eq!(error.code(), "file_concurrency_limit");

        first.write_chunk(0, b"ab", 21).expect("chunk");
        first.write_chunk(1, b"cd", 22).expect("chunk");
        first.finish(23).expect("finish");
        assert_eq!(runtime.active_transfer_count(), 0);
    }

    #[test]
    fn duplicate_transfer_or_operation_is_rejected_until_released() {
        let (temporary, runtime, share) = setup();
        let (_tx, cancel) = watch::channel(false);
        let first = runtime
            .begin_upload_cancellable(
                upload_session(&share, "one.txt", b"abcd", 2, 1, 1, 1_000),
                &context(&share),
                true,
                UploadAdmission::default(),
                TransferOperationCancellation::new(operation(1), cancel),
                20,
            )
            .expect("first upload");

        let (_tx, cancel) = watch::channel(false);
        let error = runtime
            .begin_upload_cancellable(
                upload_session(&share, "two.txt", b"abcd", 2, 1, 2, 1_000),
                &context(&share),
                true,
                UploadAdmission::default(),
                TransferOperationCancellation::new(operation(2), cancel),
                20,
            )
            .expect_err("duplicate transfer");
        assert_eq!(error.code(), "file_transfer_operation_active");

        let (_tx, cancel) = watch::channel(false);
        let error = runtime
            .begin_upload_cancellable(
                upload_session(&share, "three.txt", b"abcd", 2, 3, 1, 1_000),
                &context(&share),
                true,
                UploadAdmission::default(),
                TransferOperationCancellation::new(operation(1), cancel),
                20,
            )
            .expect_err("duplicate operation");
        assert_eq!(error.code(), "file_transfer_operation_active");

        drop(first);
        assert_eq!(runtime.active_transfer_count(), 0);
        assert!(!temporary.path().join("one.txt").exists());
    }

    #[test]
    fn operation_cancellation_mid_upload_cancels_and_releases_admission() {
        let (temporary, runtime, share) = setup();
        let (cancel_tx, cancel) = watch::channel(false);
        let mut transfer = runtime
            .begin_upload_cancellable(
                upload_session(&share, "cancelled.txt", b"abcdefgh", 4, 1, 1, 1_000),
                &context(&share),
                true,
                UploadAdmission::default(),
                TransferOperationCancellation::new(operation(1), cancel),
                20,
            )
            .expect("upload");
        transfer.write_chunk(0, b"abcd", 21).expect("first chunk");
        cancel_tx.send_replace(true);

        let error = transfer
            .write_chunk(1, b"efgh", 22)
            .expect_err("cancelled upload");
        assert_eq!(error.code(), "file_transfer_cancelled");
        assert_eq!(
            transfer.session().state,
            fuckipv6_common::domain::TransferState::Cancelled
        );
        assert_eq!(runtime.active_transfer_count(), 0);
        assert!(!temporary.path().join("cancelled.txt").exists());
        let temporary_directory = temporary.path().join(FILE_TEMP_DIRECTORY);
        assert!(std::fs::read_dir(temporary_directory)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(true));
    }

    #[test]
    fn download_admission_is_bounded_and_released_on_completion() {
        let mut share = share();
        share.quota.max_concurrent_transfers = 1;
        share.quota.max_concurrent_transfers_per_member = 1;
        share.quota.max_concurrent_transfers_per_directory = 1;
        let (temporary, runtime, share) = setup_with(share);
        std::fs::write(temporary.path().join("download.txt"), b"abcdefgh").expect("source");
        let (_tx, cancel) = watch::channel(false);
        let mut download = runtime
            .begin_download_cancellable(
                download_session(&share, "download.txt", b"abcdefgh", 4, 1, 1, 1_000),
                &context(&share),
                true,
                TransferOperationCancellation::new(operation(1), cancel),
                20,
            )
            .expect("download");
        assert_eq!(
            download.read_next(21).expect("first chunk"),
            Some(b"abcd".to_vec())
        );

        let (_tx, cancel) = watch::channel(false);
        let error = runtime
            .begin_download_cancellable(
                download_session(&share, "download.txt", b"abcdefgh", 4, 2, 2, 1_000),
                &context(&share),
                true,
                TransferOperationCancellation::new(operation(2), cancel),
                20,
            )
            .expect_err("concurrency limit");
        assert_eq!(error.code(), "file_concurrency_limit");

        assert_eq!(
            download.read_next(22).expect("second chunk"),
            Some(b"efgh".to_vec())
        );
        download.finish(23).expect("finish");
        assert_eq!(runtime.active_transfer_count(), 0);
    }

    #[test]
    fn member_and_directory_limits_are_independent_within_global_limit() {
        let mut share = share();
        share.quota.max_concurrent_transfers = 3;
        share.quota.max_concurrent_transfers_per_member = 1;
        share.quota.max_concurrent_transfers_per_directory = 2;
        let (temporary, runtime, share) = setup_with(share);
        std::fs::create_dir(temporary.path().join("team")).expect("team directory");

        let (_tx, cancel) = watch::channel(false);
        let first = runtime
            .begin_upload_cancellable(
                upload_session(&share, "team/owner.txt", b"abcd", 4, 1, 1, 1_000),
                &context(&share),
                true,
                UploadAdmission::default(),
                TransferOperationCancellation::new(operation(1), cancel),
                20,
            )
            .expect("owner upload");

        let (_tx, cancel) = watch::channel(false);
        let error = runtime
            .begin_upload_cancellable(
                upload_session(&share, "team/owner-second.txt", b"abcd", 4, 2, 2, 1_000),
                &context(&share),
                true,
                UploadAdmission::default(),
                TransferOperationCancellation::new(operation(2), cancel),
                20,
            )
            .expect_err("same member is limited");
        assert_eq!(error.code(), "file_member_concurrency_limit");

        let member = NodeId::from_public_key(b"runtime-member");
        let (_tx, cancel) = watch::channel(false);
        let _member_transfer = runtime
            .begin_upload_cancellable(
                upload_session_for(UploadSessionSpec {
                    share: &share,
                    actor: member.clone(),
                    path: "team/member.txt",
                    content: b"abcd",
                    chunk_size: 4,
                    transfer_id: transfer(3),
                    operation_id: operation(3),
                    expires_at_unix_ms: 1_000,
                }),
                &member_context(&share, member),
                true,
                UploadAdmission::default(),
                TransferOperationCancellation::new(operation(3), cancel),
                20,
            )
            .expect("different member may use its own slot");

        assert_eq!(runtime.active_transfer_count(), 2);
        drop(first);
        assert_eq!(runtime.active_transfer_count(), 1);
    }

    #[test]
    fn directory_limit_blocks_only_the_same_parent_directory() {
        let mut share = share();
        share.quota.max_concurrent_transfers = 3;
        share.quota.max_concurrent_transfers_per_member = 3;
        share.quota.max_concurrent_transfers_per_directory = 1;
        let (temporary, runtime, share) = setup_with(share);
        std::fs::create_dir(temporary.path().join("alpha")).expect("alpha directory");
        std::fs::create_dir(temporary.path().join("beta")).expect("beta directory");

        let (_tx, cancel) = watch::channel(false);
        let _first = runtime
            .begin_upload_cancellable(
                upload_session(&share, "alpha/one.txt", b"abcd", 4, 1, 1, 1_000),
                &context(&share),
                true,
                UploadAdmission::default(),
                TransferOperationCancellation::new(operation(1), cancel),
                20,
            )
            .expect("first directory upload");

        let (_tx, cancel) = watch::channel(false);
        let error = runtime
            .begin_upload_cancellable(
                upload_session(&share, "alpha/two.txt", b"abcd", 4, 2, 2, 1_000),
                &context(&share),
                true,
                UploadAdmission::default(),
                TransferOperationCancellation::new(operation(2), cancel),
                20,
            )
            .expect_err("same directory is limited");
        assert_eq!(error.code(), "file_directory_concurrency_limit");

        let (_tx, cancel) = watch::channel(false);
        let _other_directory = runtime
            .begin_upload_cancellable(
                upload_session(&share, "beta/one.txt", b"abcd", 4, 3, 3, 1_000),
                &context(&share),
                true,
                UploadAdmission::default(),
                TransferOperationCancellation::new(operation(3), cancel),
                20,
            )
            .expect("different directory may use global capacity");
        assert_eq!(runtime.active_transfer_count(), 2);
    }

    #[test]
    fn repeated_transfer_lifecycles_return_admission_and_staging_to_baseline() {
        let (temporary, runtime, share) = setup();
        for index in 0..512_u32 {
            let transfer_id =
                TransferId::from_public_key(format!("stress-transfer-{index}").as_bytes());
            let operation_id =
                OperationId::from_public_key(format!("stress-operation-{index}").as_bytes());
            let (_tx, cancel) = watch::channel(false);
            let transfer = runtime
                .begin_upload_cancellable(
                    upload_session_for(UploadSessionSpec {
                        share: &share,
                        actor: owner(),
                        path: "stress.txt",
                        content: b"abcd",
                        chunk_size: 4,
                        transfer_id,
                        operation_id: operation_id.clone(),
                        expires_at_unix_ms: 1_000,
                    }),
                    &context(&share),
                    true,
                    UploadAdmission::default(),
                    TransferOperationCancellation::new(operation_id, cancel),
                    20,
                )
                .expect("stress upload");
            assert_eq!(runtime.active_transfer_count(), 1);
            drop(transfer);
            assert_eq!(runtime.active_transfer_count(), 0);
        }
        let staging = temporary.path().join(FILE_TEMP_DIRECTORY);
        assert!(std::fs::read_dir(staging)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(true));
    }

    #[test]
    fn disabling_capability_invalidates_existing_handles_and_clears_admission() {
        let (_temporary, runtime, share) = setup();
        let (_tx, cancel) = watch::channel(false);
        let mut transfer = runtime
            .begin_upload_cancellable(
                upload_session(&share, "disabled.txt", b"abcd", 4, 1, 1, 1_000),
                &context(&share),
                true,
                UploadAdmission::default(),
                TransferOperationCancellation::new(operation(1), cancel),
                20,
            )
            .expect("upload");
        assert_eq!(runtime.active_transfer_count(), 1);
        let snapshot = runtime.disable(&owner(), 21).expect("disable");
        assert_eq!(snapshot.capability.state, FileCapabilityState::Disabled);
        assert_eq!(snapshot.active_transfers, 0);

        let error = transfer
            .write_chunk(0, b"abcd", 22)
            .expect_err("disabled capability");
        assert_eq!(error.code(), "file_capability_revoked");
        assert_eq!(
            transfer.session().state,
            fuckipv6_common::domain::TransferState::Cancelled
        );
    }
}
