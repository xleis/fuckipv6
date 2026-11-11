//! In-process upload and download adapters for the shared File transfer state
//! machine. Network protocols own framing and operation lifecycle; this module
//! owns authorization checkpoints, bounded chunk I/O, staging, and publication.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};

use fuckipv6_common::domain::{
    AccessContext, ConcurrentNamePolicy, ContentHash, FileAccessDenial, FileAccessDenialCode,
    FileAccessRequest, FileAction, FileQuotaError, FileTransferError, ResumeRequest, SharedPath,
    TransferDirection, TransferSession, TransferState,
};

use super::capability::{
    FileCapabilityError, FileCapabilityLease, FileCapabilityPermit, FileShareCapability,
};
use super::error::FileShareError;
use super::quota::{self, QuotaScanLimits};
use super::root::LocalShareRoot;
use super::temp::TemporaryUpload;

/// Authenticated server-side accounting supplied at upload admission. These
/// values must not come from the requesting peer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UploadAdmission {
    pub member_bytes: u64,
    pub concurrent_transfers: u32,
    pub scan_limits: QuotaScanLimits,
}

/// Network-safe transfer progress. It intentionally contains no local paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferProgress {
    pub state: TransferState,
    pub transferred_bytes: u64,
    pub total_bytes: u64,
    pub received_chunks: u32,
    pub total_chunks: u32,
}

/// Stable adapter failures. Callers should project `code()` instead of sending
/// the source error or local filesystem text to peers and UI clients.
#[derive(Debug, thiserror::Error)]
pub enum FileTransferAdapterError {
    #[error(transparent)]
    Transfer(#[from] FileTransferError),
    #[error(transparent)]
    Capability(#[from] FileCapabilityError),
    #[error(transparent)]
    Filesystem(#[from] FileShareError),
    #[error("File quota was exceeded: {0:?}")]
    Quota(FileQuotaError),
    #[error("transfer direction does not match the requested operation")]
    WrongDirection,
    #[error("chunks must be supplied in manifest order")]
    ChunkOutOfOrder,
    #[error("the configured concurrent-name policy is not implemented for uploads")]
    ConcurrentNamePolicyUnsupported,
    #[error("version-checked replacement uploads are not implemented")]
    ReplacementUnsupported,
}

impl From<FileQuotaError> for FileTransferAdapterError {
    fn from(error: FileQuotaError) -> Self {
        Self::Quota(error)
    }
}

impl FileTransferAdapterError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Transfer(error) => error.code(),
            Self::Capability(error) => capability_error_code(error),
            Self::Filesystem(error) => error.code_str(),
            Self::Quota(error) => error.code(),
            Self::WrongDirection => "file_transfer_wrong_direction",
            Self::ChunkOutOfOrder => "file_transfer_chunk_out_of_order",
            Self::ConcurrentNamePolicyUnsupported => {
                "file_transfer_concurrent_name_policy_unsupported"
            }
            Self::ReplacementUnsupported => "file_transfer_replacement_unsupported",
        }
    }
}

/// Starts local transfer adapters against one File capability.
#[derive(Debug, Clone)]
pub struct FileTransferAdapter {
    capability: FileShareCapability,
}

impl FileTransferAdapter {
    pub fn new(capability: FileShareCapability) -> Self {
        Self { capability }
    }

    pub fn begin_upload(
        &self,
        mut session: TransferSession,
        context: &AccessContext,
        network_enabled: bool,
        admission: UploadAdmission,
        now_unix_ms: u64,
    ) -> Result<UploadTransfer, FileTransferAdapterError> {
        if session.binding.direction != TransferDirection::Upload {
            return Err(FileTransferAdapterError::WrongDirection);
        }
        if session.binding.file_version.is_some() {
            return Err(FileTransferAdapterError::ReplacementUnsupported);
        }
        let lease = self.authorize(&session, FileAction::Upload, context, network_enabled)?;
        validate_binding(&session, &lease, context)?;
        ensure_upload_destination(lease.root(), lease.share(), &session.binding.path)?;

        let usage = lease.root().directory_usage(admission.scan_limits)?;
        lease.permit().ensure_current()?;
        quota::check_upload(
            &lease.share().quota,
            &usage,
            session.manifest.total_bytes,
            0,
            admission.member_bytes,
            admission.concurrent_transfers,
        )?;

        let request = resume_request(&session);
        session.resume(
            &request,
            None,
            lease.share().revision,
            true,
            lease.permit().access_epoch(),
            now_unix_ms,
        )?;
        let upload = TemporaryUpload::create(lease.root())?;
        if let Err(error) = lease.permit().ensure_current() {
            let _ = session.cancel(now_unix_ms);
            return Err(error.into());
        }

        Ok(UploadTransfer {
            capability: self.capability.clone(),
            session,
            upload: Some(upload),
            permit: lease.permit().clone(),
            hasher: blake3::Hasher::new(),
            next_chunk: 0,
            transferred_bytes: 0,
        })
    }

    pub fn begin_download(
        &self,
        mut session: TransferSession,
        context: &AccessContext,
        network_enabled: bool,
        now_unix_ms: u64,
    ) -> Result<DownloadTransfer, FileTransferAdapterError> {
        if session.binding.direction != TransferDirection::Download {
            return Err(FileTransferAdapterError::WrongDirection);
        }
        let lease = self.authorize(&session, FileAction::Download, context, network_enabled)?;
        validate_binding(&session, &lease, context)?;
        let expected_version = session
            .binding
            .file_version
            .clone()
            .ok_or(FileTransferError::VersionChanged)?;
        let current_version = lease
            .root()
            .file_version(&session.binding.path, expected_version.revision)?;
        lease.permit().ensure_current()?;
        if current_version != expected_version {
            let _ = session.fail(now_unix_ms);
            return Err(FileTransferError::VersionChanged.into());
        }
        let file = lease.root().open_read(&session.binding.path)?;
        ensure_file_size(
            &file,
            session.manifest.total_bytes,
            now_unix_ms,
            &mut session,
        )?;

        let request = resume_request(&session);
        session.resume(
            &request,
            Some(&current_version),
            lease.share().revision,
            true,
            lease.permit().access_epoch(),
            now_unix_ms,
        )?;
        if let Err(error) = lease.permit().ensure_current() {
            let _ = session.cancel(now_unix_ms);
            return Err(error.into());
        }

        Ok(DownloadTransfer {
            capability: self.capability.clone(),
            session,
            file: Some(file),
            permit: lease.permit().clone(),
            hasher: blake3::Hasher::new(),
            next_chunk: 0,
            transferred_bytes: 0,
        })
    }

    fn authorize(
        &self,
        session: &TransferSession,
        action: FileAction,
        context: &AccessContext,
        network_enabled: bool,
    ) -> Result<FileCapabilityLease, FileTransferAdapterError> {
        Ok(self.capability.authorize(
            &FileAccessRequest {
                path: session.binding.path.clone(),
                action,
            },
            context,
            network_enabled,
        )?)
    }
}

/// A staged upload. Dropping it removes the hidden temporary file; only a
/// successful `finish` makes the object visible in the share namespace.
#[derive(Debug)]
pub struct UploadTransfer {
    capability: FileShareCapability,
    session: TransferSession,
    upload: Option<TemporaryUpload>,
    permit: FileCapabilityPermit,
    hasher: blake3::Hasher,
    next_chunk: u32,
    transferred_bytes: u64,
}

impl UploadTransfer {
    pub fn session(&self) -> &TransferSession {
        &self.session
    }

    pub fn progress(&self) -> TransferProgress {
        progress(&self.session, self.transferred_bytes)
    }

    /// Accept one manifest chunk. Replaying the exact same chunk is idempotent
    /// and does not advance progress or hash it twice.
    pub fn write_chunk(
        &mut self,
        index: u32,
        bytes: &[u8],
        now_unix_ms: u64,
    ) -> Result<bool, FileTransferAdapterError> {
        self.ensure_running(now_unix_ms)?;
        if self.session.has_received_chunk(index) {
            match self.session.record_chunk(index, bytes, now_unix_ms) {
                Ok(()) => return Ok(false),
                Err(error) => {
                    let _ = self.session.fail(now_unix_ms);
                    self.upload.take();
                    return Err(error.into());
                }
            }
        }
        if index != self.next_chunk {
            let _ = self.session.fail(now_unix_ms);
            self.upload.take();
            return Err(FileTransferAdapterError::ChunkOutOfOrder);
        }

        if let Err(error) = self.session.record_chunk(index, bytes, now_unix_ms) {
            let _ = self.session.fail(now_unix_ms);
            self.upload.take();
            return Err(error.into());
        }
        if let Err(error) = self
            .upload
            .as_mut()
            .expect("active upload has staging")
            .write_all(bytes)
        {
            let _ = self.session.fail(now_unix_ms);
            self.upload.take();
            return Err(FileShareError::io(
                super::error::FileShareErrorCode::Io,
                "upload chunk could not be staged",
                error,
            )
            .into());
        }
        self.hasher.update(bytes);
        self.transferred_bytes = self
            .transferred_bytes
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        self.next_chunk = self.next_chunk.saturating_add(1);
        self.check_authorization_boundary(now_unix_ms)?;
        Ok(true)
    }

    pub fn pause(&mut self, now_unix_ms: u64) -> Result<(), FileTransferAdapterError> {
        self.ensure_running(now_unix_ms)?;
        self.session.pause(now_unix_ms)?;
        Ok(())
    }

    /// Reauthorize a paused upload. Any capability epoch or share revision
    /// change invalidates the original binding and fails the transfer closed.
    pub fn resume(
        &mut self,
        context: &AccessContext,
        network_enabled: bool,
        now_unix_ms: u64,
    ) -> Result<(), FileTransferAdapterError> {
        if self.session.state != TransferState::Paused {
            return Err(FileTransferError::InvalidState.into());
        }
        let result = (|| {
            let lease = self.capability.authorize(
                &FileAccessRequest {
                    path: self.session.binding.path.clone(),
                    action: FileAction::Upload,
                },
                context,
                network_enabled,
            )?;
            validate_binding(&self.session, &lease, context)?;
            ensure_upload_destination(lease.root(), lease.share(), &self.session.binding.path)?;
            let request = resume_request(&self.session);
            self.session.resume(
                &request,
                None,
                lease.share().revision,
                true,
                lease.permit().access_epoch(),
                now_unix_ms,
            )?;
            lease.permit().ensure_current()?;
            self.permit = lease.permit().clone();
            Ok(())
        })();
        if let Err(error) = &result {
            self.abandon_after_error(now_unix_ms, error);
        }
        result
    }

    /// Verify the staged bytes, atomically publish without replacing a racing
    /// target, and only then mark the transfer complete. Repeated calls after a
    /// successful completion are idempotent.
    pub fn finish(
        &mut self,
        now_unix_ms: u64,
    ) -> Result<TransferProgress, FileTransferAdapterError> {
        if self.session.state == TransferState::Completed {
            return Ok(self.progress());
        }
        if self.session.state != TransferState::Active {
            return Err(FileTransferError::InvalidState.into());
        }
        self.ensure_running(now_unix_ms)?;
        if usize::try_from(self.next_chunk).ok() != Some(self.session.manifest.chunks.len()) {
            return Err(FileTransferError::Incomplete.into());
        }

        let digest = ContentHash::from_digest_bytes(*self.hasher.clone().finalize().as_bytes());
        if let Err(error) = self
            .session
            .verify_digest(self.transferred_bytes, &digest, now_unix_ms)
        {
            let _ = self.session.fail(now_unix_ms);
            self.upload.take();
            return Err(error.into());
        }
        self.check_authorization_boundary(now_unix_ms)?;

        let upload = self.upload.take().ok_or(FileTransferError::InvalidState)?;
        if let Err(error) = upload.persist_new(&self.session.binding.path) {
            let _ = self.session.fail(now_unix_ms);
            return Err(error.into());
        }
        self.session.complete_verified(now_unix_ms)?;
        Ok(self.progress())
    }

    pub fn cancel(mut self, now_unix_ms: u64) -> Result<(), FileTransferAdapterError> {
        self.session.cancel(now_unix_ms)?;
        self.upload.take();
        Ok(())
    }

    fn ensure_running(&mut self, now_unix_ms: u64) -> Result<(), FileTransferAdapterError> {
        if now_unix_ms >= self.session.binding.expires_at_unix_ms {
            let _ = self.session.expire(now_unix_ms);
            self.upload.take();
            return Err(FileTransferError::Expired.into());
        }
        if let Err(error) = self.permit.ensure_current() {
            let _ = self.session.cancel(now_unix_ms);
            self.upload.take();
            return Err(error.into());
        }
        Ok(())
    }

    fn check_authorization_boundary(
        &mut self,
        now_unix_ms: u64,
    ) -> Result<(), FileTransferAdapterError> {
        if now_unix_ms >= self.session.binding.expires_at_unix_ms {
            let _ = self.session.expire(now_unix_ms);
            self.upload.take();
            return Err(FileTransferError::Expired.into());
        }
        if let Err(error) = self.permit.ensure_current() {
            let _ = self.session.cancel(now_unix_ms);
            self.upload.take();
            return Err(error.into());
        }
        Ok(())
    }

    fn abandon_after_error(&mut self, now_unix_ms: u64, error: &FileTransferAdapterError) {
        if !self.session.state.is_terminal() {
            if authorization_was_revoked(error) {
                let _ = self.session.cancel(now_unix_ms);
            } else {
                let _ = self.session.fail(now_unix_ms);
            }
        }
        self.upload.take();
    }
}

/// A verified, seekable download source. The file handle is anchored while the
/// transfer is active and reopened with a fresh authorization on resume.
#[derive(Debug)]
pub struct DownloadTransfer {
    capability: FileShareCapability,
    session: TransferSession,
    file: Option<File>,
    permit: FileCapabilityPermit,
    hasher: blake3::Hasher,
    next_chunk: u32,
    transferred_bytes: u64,
}

impl DownloadTransfer {
    pub fn session(&self) -> &TransferSession {
        &self.session
    }

    pub fn progress(&self) -> TransferProgress {
        progress(&self.session, self.transferred_bytes)
    }

    /// Read and checksum the next manifest chunk. `Ok(None)` means all chunks
    /// were delivered and `finish` may verify the assembled content.
    pub fn read_next(
        &mut self,
        now_unix_ms: u64,
    ) -> Result<Option<Vec<u8>>, FileTransferAdapterError> {
        self.ensure_running(now_unix_ms)?;
        let index = usize::try_from(self.next_chunk).unwrap_or(usize::MAX);
        let Some(chunk) = self.session.manifest.chunks.get(index).cloned() else {
            return Ok(None);
        };
        let length =
            usize::try_from(chunk.length).map_err(|_| FileTransferError::InvalidManifest)?;
        let file = self.file.as_mut().ok_or(FileTransferError::InvalidState)?;
        if let Err(error) = file.seek(SeekFrom::Start(chunk.offset)) {
            let _ = self.session.fail(now_unix_ms);
            self.file.take();
            return Err(FileShareError::io(
                super::error::FileShareErrorCode::Io,
                "download source could not be positioned",
                error,
            )
            .into());
        }
        let mut bytes = vec![0_u8; length];
        if let Err(error) = file.read_exact(&mut bytes) {
            let _ = self.session.fail(now_unix_ms);
            self.file.take();
            return Err(FileShareError::io(
                super::error::FileShareErrorCode::Io,
                "download source could not be read",
                error,
            )
            .into());
        }
        if let Err(error) = self.session.record_chunk(chunk.index, &bytes, now_unix_ms) {
            let _ = self.session.fail(now_unix_ms);
            self.file.take();
            return Err(error.into());
        }
        self.hasher.update(&bytes);
        self.transferred_bytes = self
            .transferred_bytes
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        self.next_chunk = self.next_chunk.saturating_add(1);
        self.check_authorization_boundary(now_unix_ms)?;
        Ok(Some(bytes))
    }

    pub fn pause(&mut self, now_unix_ms: u64) -> Result<(), FileTransferAdapterError> {
        self.ensure_running(now_unix_ms)?;
        self.session.pause(now_unix_ms)?;
        self.file.take();
        Ok(())
    }

    pub fn resume(
        &mut self,
        context: &AccessContext,
        network_enabled: bool,
        now_unix_ms: u64,
    ) -> Result<(), FileTransferAdapterError> {
        if self.session.state != TransferState::Paused {
            return Err(FileTransferError::InvalidState.into());
        }
        let result = (|| {
            let lease = self.capability.authorize(
                &FileAccessRequest {
                    path: self.session.binding.path.clone(),
                    action: FileAction::Download,
                },
                context,
                network_enabled,
            )?;
            validate_binding(&self.session, &lease, context)?;
            let expected_version = self
                .session
                .binding
                .file_version
                .clone()
                .ok_or(FileTransferError::VersionChanged)?;
            let current_version = lease
                .root()
                .file_version(&self.session.binding.path, expected_version.revision)?;
            if current_version != expected_version {
                return Err(FileTransferError::VersionChanged.into());
            }
            let file = lease.root().open_read(&self.session.binding.path)?;
            let request = resume_request(&self.session);
            self.session.resume(
                &request,
                Some(&current_version),
                lease.share().revision,
                true,
                lease.permit().access_epoch(),
                now_unix_ms,
            )?;
            lease.permit().ensure_current()?;
            self.permit = lease.permit().clone();
            self.file = Some(file);
            Ok(())
        })();
        if let Err(error) = &result {
            self.abandon_after_error(now_unix_ms, error);
        }
        result
    }

    /// Verify the hash of the bytes delivered from this source and complete the
    /// download session. Repeated calls after completion are idempotent.
    pub fn finish(
        &mut self,
        now_unix_ms: u64,
    ) -> Result<TransferProgress, FileTransferAdapterError> {
        if self.session.state == TransferState::Completed {
            return Ok(self.progress());
        }
        if self.session.state != TransferState::Active {
            return Err(FileTransferError::InvalidState.into());
        }
        self.ensure_running(now_unix_ms)?;
        if usize::try_from(self.next_chunk).ok() != Some(self.session.manifest.chunks.len()) {
            return Err(FileTransferError::Incomplete.into());
        }
        let digest = ContentHash::from_digest_bytes(*self.hasher.clone().finalize().as_bytes());
        if let Err(error) =
            self.session
                .verify_and_complete_digest(self.transferred_bytes, &digest, now_unix_ms)
        {
            let _ = self.session.fail(now_unix_ms);
            self.file.take();
            return Err(error.into());
        }
        self.file.take();
        Ok(self.progress())
    }

    pub fn cancel(mut self, now_unix_ms: u64) -> Result<(), FileTransferAdapterError> {
        self.session.cancel(now_unix_ms)?;
        self.file.take();
        Ok(())
    }

    fn ensure_running(&mut self, now_unix_ms: u64) -> Result<(), FileTransferAdapterError> {
        if now_unix_ms >= self.session.binding.expires_at_unix_ms {
            let _ = self.session.expire(now_unix_ms);
            self.file.take();
            return Err(FileTransferError::Expired.into());
        }
        if let Err(error) = self.permit.ensure_current() {
            let _ = self.session.cancel(now_unix_ms);
            self.file.take();
            return Err(error.into());
        }
        Ok(())
    }

    fn check_authorization_boundary(
        &mut self,
        now_unix_ms: u64,
    ) -> Result<(), FileTransferAdapterError> {
        if now_unix_ms >= self.session.binding.expires_at_unix_ms {
            let _ = self.session.expire(now_unix_ms);
            self.file.take();
            return Err(FileTransferError::Expired.into());
        }
        if let Err(error) = self.permit.ensure_current() {
            let _ = self.session.cancel(now_unix_ms);
            self.file.take();
            return Err(error.into());
        }
        Ok(())
    }

    fn abandon_after_error(&mut self, now_unix_ms: u64, error: &FileTransferAdapterError) {
        if !self.session.state.is_terminal() {
            if authorization_was_revoked(error) {
                let _ = self.session.cancel(now_unix_ms);
            } else {
                let _ = self.session.fail(now_unix_ms);
            }
        }
        self.file.take();
    }
}

fn authorization_was_revoked(error: &FileTransferAdapterError) -> bool {
    matches!(
        error,
        FileTransferAdapterError::Transfer(FileTransferError::PermissionRevoked)
            | FileTransferAdapterError::Capability(
                FileCapabilityError::Disabled
                    | FileCapabilityError::AccessDenied(_)
                    | FileCapabilityError::Revoked
            )
    )
}

fn validate_binding(
    session: &TransferSession,
    lease: &FileCapabilityLease,
    context: &AccessContext,
) -> Result<(), FileTransferAdapterError> {
    if session.binding.actor != context.actor || session.binding.share_id != lease.share().id {
        return Err(FileTransferError::BindingMismatch.into());
    }
    if session.binding.access_epoch != lease.permit().access_epoch() {
        return Err(FileTransferError::PermissionRevoked.into());
    }
    if session.binding.share_revision != lease.share().revision {
        return Err(FileTransferError::BindingMismatch.into());
    }
    Ok(())
}

fn ensure_upload_destination(
    root: &LocalShareRoot,
    share: &fuckipv6_common::domain::FileShare,
    path: &SharedPath,
) -> Result<(), FileTransferAdapterError> {
    if share.concurrent_name_policy != ConcurrentNamePolicy::Reject {
        return Err(FileTransferAdapterError::ConcurrentNamePolicyUnsupported);
    }
    if path.is_root() {
        return Err(FileShareError::new(
            super::error::FileShareErrorCode::PathRejected,
            "uploads require a non-root shared path",
        )
        .into());
    }
    if let Some(parent) = path.parent() {
        root.open_directory(&parent)?;
    }
    root.ensure_target_absent(path)?;
    Ok(())
}

fn ensure_file_size(
    file: &File,
    expected: u64,
    now_unix_ms: u64,
    session: &mut TransferSession,
) -> Result<(), FileTransferAdapterError> {
    let size = file
        .metadata()
        .map_err(|error| {
            FileShareError::io(
                super::error::FileShareErrorCode::Io,
                "download source metadata is unavailable",
                error,
            )
        })?
        .len();
    if size != expected {
        let _ = session.fail(now_unix_ms);
        return Err(FileTransferError::VersionChanged.into());
    }
    Ok(())
}

fn resume_request(session: &TransferSession) -> ResumeRequest {
    ResumeRequest {
        transfer_id: session.binding.transfer_id.clone(),
        operation_id: session.binding.operation_id.clone(),
        share_id: session.binding.share_id.clone(),
        actor: session.binding.actor.clone(),
        access_epoch: session.binding.access_epoch,
        path: session.binding.path.clone(),
        direction: session.binding.direction,
        content_hash: session.manifest.content_hash.clone(),
        share_revision: session.binding.share_revision,
    }
}

fn progress(session: &TransferSession, transferred_bytes: u64) -> TransferProgress {
    TransferProgress {
        state: session.state,
        transferred_bytes,
        total_bytes: session.manifest.total_bytes,
        received_chunks: u32::try_from(session.received_chunk_count()).unwrap_or(u32::MAX),
        total_chunks: u32::try_from(session.manifest.chunks.len()).unwrap_or(u32::MAX),
    }
}

fn capability_error_code(error: &FileCapabilityError) -> &'static str {
    match error {
        FileCapabilityError::Disabled => "file_capability_disabled",
        FileCapabilityError::Unavailable => "file_capability_unavailable",
        FileCapabilityError::OwnerRequired => "file_owner_required",
        FileCapabilityError::InvalidShare(_) => "file_share_invalid",
        FileCapabilityError::Filesystem(error) => error.code_str(),
        FileCapabilityError::AccessDenied(denial) => access_denial_code(denial),
        FileCapabilityError::Quota(error) => error.code(),
        FileCapabilityError::Revoked => "file_capability_revoked",
        FileCapabilityError::AuditUnavailable => "file_audit_unavailable",
    }
}

fn access_denial_code(denial: &FileAccessDenial) -> &'static str {
    match denial.code {
        FileAccessDenialCode::NetworkDisabled => "file_network_disabled",
        FileAccessDenialCode::MembershipInactive => "file_membership_inactive",
        FileAccessDenialCode::CrossNetworkDenied => "file_cross_network_denied",
        FileAccessDenialCode::ReadOnlyShare => "file_read_only_share",
        FileAccessDenialCode::AclDenied => "file_acl_denied",
        FileAccessDenialCode::AclNoAllow => "file_acl_no_allow",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use fuckipv6_common::domain::{
        FileAcl, FileAclEffect, FileAclRule, FileChunk, FileDeletePolicy, FileQuota, FileShareId,
        FileShareMode, FileVersion, NetworkId, NetworkRole, NodeId, OperationId, SubjectSelector,
        SymlinkPolicy, TransferBinding, TransferId, TransferManifest, FILE_SHARE_SCHEMA_VERSION,
    };
    use tempfile::TempDir;

    use super::*;
    use crate::file_share::{FileCapabilityState, FILE_TEMP_DIRECTORY};

    fn owner() -> NodeId {
        NodeId::from_public_key(b"owner")
    }

    fn allow_acl() -> FileAcl {
        FileAcl {
            rules: vec![FileAclRule {
                id: "allow-members".into(),
                subjects: BTreeSet::from([SubjectSelector::AnyMember]),
                path_prefix: SharedPath::root(),
                actions: BTreeSet::from([
                    FileAction::List,
                    FileAction::ReadMetadata,
                    FileAction::Download,
                    FileAction::Upload,
                    FileAction::CreateDirectory,
                    FileAction::Rename,
                    FileAction::Delete,
                ]),
                effect: FileAclEffect::Allow,
            }],
        }
    }

    fn share() -> fuckipv6_common::domain::FileShare {
        fuckipv6_common::domain::FileShare {
            schema_version: FILE_SHARE_SCHEMA_VERSION,
            id: FileShareId::from_public_key(b"share"),
            network_id: NetworkId::from_public_key(b"network"),
            owner: owner(),
            display_name: "Team files".into(),
            mode: FileShareMode::ReadWrite,
            revision: 1,
            network_acl: allow_acl(),
            local_acl: allow_acl(),
            quota: FileQuota::default(),
            symlink_policy: SymlinkPolicy::Reject,
            concurrent_name_policy: ConcurrentNamePolicy::Reject,
            delete_policy: FileDeletePolicy::default(),
            audit_enabled: true,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        }
    }

    fn context(share: &fuckipv6_common::domain::FileShare) -> AccessContext {
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

    fn setup() -> (
        TempDir,
        FileShareCapability,
        fuckipv6_common::domain::FileShare,
    ) {
        setup_with(share())
    }

    fn setup_with(
        share: fuckipv6_common::domain::FileShare,
    ) -> (
        TempDir,
        FileShareCapability,
        fuckipv6_common::domain::FileShare,
    ) {
        let temporary = tempfile::tempdir().expect("tempdir");
        let capability = FileShareCapability::new();
        let snapshot = capability
            .enable(
                &share,
                &owner(),
                temporary.path().to_str().expect("utf8 temp path"),
                10,
            )
            .expect("enable File");
        assert_eq!(snapshot.state, FileCapabilityState::Ready);
        (temporary, capability, share)
    }

    fn manifest(bytes: &[u8], chunk_size: u64) -> TransferManifest {
        let chunks = bytes
            .chunks(usize::try_from(chunk_size).expect("chunk size"))
            .enumerate()
            .map(|(index, chunk)| FileChunk {
                index: u32::try_from(index).expect("index"),
                offset: u64::try_from(index).expect("index") * chunk_size,
                length: u64::try_from(chunk.len()).expect("length"),
                checksum: ContentHash::from_bytes(chunk),
            })
            .collect();
        TransferManifest {
            file_id: fuckipv6_common::domain::FileId::from_public_key(b"file"),
            total_bytes: u64::try_from(bytes.len()).expect("length"),
            chunk_size,
            content_hash: ContentHash::from_bytes(bytes),
            chunks,
        }
    }

    fn binding(
        share: &fuckipv6_common::domain::FileShare,
        path: &str,
        direction: TransferDirection,
        file_version: Option<FileVersion>,
        tag: &[u8],
        expires_at_unix_ms: u64,
    ) -> TransferBinding {
        TransferBinding {
            transfer_id: TransferId::from_public_key(tag),
            operation_id: OperationId::from_public_key(tag),
            share_id: share.id.clone(),
            actor: owner(),
            access_epoch: 1,
            path: SharedPath::parse(path).expect("path"),
            direction,
            file_version,
            share_revision: share.revision,
            expires_at_unix_ms,
        }
    }

    fn upload_session(
        share: &fuckipv6_common::domain::FileShare,
        path: &str,
        bytes: &[u8],
        chunk_size: u64,
        tag: &[u8],
        expires_at_unix_ms: u64,
    ) -> TransferSession {
        TransferSession::new(
            binding(
                share,
                path,
                TransferDirection::Upload,
                None,
                tag,
                expires_at_unix_ms,
            ),
            manifest(bytes, chunk_size),
            10,
        )
        .expect("upload session")
    }

    fn download_session(
        share: &fuckipv6_common::domain::FileShare,
        path: &str,
        bytes: &[u8],
        chunk_size: u64,
        version: FileVersion,
        expires_at_unix_ms: u64,
    ) -> TransferSession {
        TransferSession::new(
            binding(
                share,
                path,
                TransferDirection::Download,
                Some(version),
                b"download",
                expires_at_unix_ms,
            ),
            manifest(bytes, chunk_size),
            10,
        )
        .expect("download session")
    }

    fn version(bytes: &[u8]) -> FileVersion {
        FileVersion {
            content_hash: ContentHash::from_bytes(bytes),
            size: u64::try_from(bytes.len()).expect("length"),
            revision: 1,
        }
    }

    fn host_path(temporary: &TempDir, relative: &str) -> std::path::PathBuf {
        temporary.path().join(relative)
    }

    fn temp_file_count(temporary: &TempDir) -> usize {
        let directory = host_path(temporary, FILE_TEMP_DIRECTORY);
        match std::fs::read_dir(directory) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                .count(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => panic!("read temporary directory: {error}"),
        }
    }

    #[test]
    fn upload_streams_chunks_verifies_and_publishes_atomically() {
        let (temporary, capability, share) = setup();
        let content = b"abcdefghij";
        let adapter = FileTransferAdapter::new(capability);
        let mut transfer = adapter
            .begin_upload(
                upload_session(&share, "result.txt", content, 4, b"upload-ok", 1_000),
                &context(&share),
                true,
                UploadAdmission::default(),
                10,
            )
            .expect("begin upload");
        assert!(!host_path(&temporary, "result.txt").exists());

        assert!(transfer.write_chunk(0, b"abcd", 11).expect("first chunk"));
        assert!(!transfer
            .write_chunk(0, b"abcd", 12)
            .expect("replayed chunk"));
        assert_eq!(transfer.progress().transferred_bytes, 4);
        assert_eq!(transfer.progress().received_chunks, 1);
        assert!(transfer.write_chunk(1, b"efgh", 13).expect("second chunk"));
        assert!(transfer.write_chunk(2, b"ij", 14).expect("last chunk"));
        assert!(!host_path(&temporary, "result.txt").exists());

        let progress = transfer.finish(15).expect("finish upload");
        assert_eq!(progress.state, TransferState::Completed);
        assert_eq!(progress.transferred_bytes, 10);
        assert_eq!(
            std::fs::read(host_path(&temporary, "result.txt")).expect("published file"),
            content
        );
        assert_eq!(temp_file_count(&temporary), 0);
        assert_eq!(transfer.finish(16).expect("idempotent finish"), progress);
    }

    #[test]
    fn corrupt_or_out_of_order_upload_chunks_fail_the_session() {
        let (temporary, capability, share) = setup();
        let adapter = FileTransferAdapter::new(capability);

        let mut corrupt = adapter
            .begin_upload(
                upload_session(&share, "corrupt.txt", b"abcdefgh", 4, b"corrupt", 1_000),
                &context(&share),
                true,
                UploadAdmission::default(),
                10,
            )
            .expect("begin corrupt upload");
        let error = corrupt
            .write_chunk(0, b"xxxx", 11)
            .expect_err("corrupt chunk must fail");
        assert_eq!(error.code(), "file_transfer_chunk_corrupt");
        assert_eq!(corrupt.session().state, TransferState::Failed);
        assert!(!host_path(&temporary, "corrupt.txt").exists());

        let mut out_of_order = adapter
            .begin_upload(
                upload_session(&share, "ordered.txt", b"abcdefgh", 4, b"ordered", 1_000),
                &context(&share),
                true,
                UploadAdmission::default(),
                10,
            )
            .expect("begin ordered upload");
        let error = out_of_order
            .write_chunk(1, b"efgh", 11)
            .expect_err("out-of-order chunk must fail");
        assert_eq!(error.code(), "file_transfer_chunk_out_of_order");
        assert_eq!(out_of_order.session().state, TransferState::Failed);
        assert!(!host_path(&temporary, "ordered.txt").exists());
    }

    #[test]
    fn upload_quota_failure_is_structured_and_creates_no_staging() {
        let mut limited = share();
        limited.quota.max_file_bytes = 4;
        let (temporary, capability, share) = setup_with(limited);
        let adapter = FileTransferAdapter::new(capability);

        let error = adapter
            .begin_upload(
                upload_session(&share, "too-large.bin", b"abcdefgh", 4, b"quota", 1_000),
                &context(&share),
                true,
                UploadAdmission::default(),
                10,
            )
            .expect_err("oversized upload must be rejected");
        assert_eq!(error.code(), "file_too_large");
        assert!(!host_path(&temporary, "too-large.bin").exists());
        assert_eq!(temp_file_count(&temporary), 0);
    }

    #[test]
    fn unsupported_upload_policies_fail_closed() {
        let mut non_replacing = share();
        non_replacing.concurrent_name_policy = ConcurrentNamePolicy::IfUnchanged;
        let (_temporary, capability, share) = setup_with(non_replacing);
        let adapter = FileTransferAdapter::new(capability.clone());
        let error = adapter
            .begin_upload(
                upload_session(&share, "policy.txt", b"abcd", 4, b"policy", 1_000),
                &context(&share),
                true,
                UploadAdmission::default(),
                10,
            )
            .expect_err("unimplemented name policy must fail closed");
        assert_eq!(
            error.code(),
            "file_transfer_concurrent_name_policy_unsupported"
        );

        let (_temporary, capability, share) = setup();
        let adapter = FileTransferAdapter::new(capability);
        let mut replacement =
            upload_session(&share, "replace.txt", b"abcd", 4, b"replacement", 1_000);
        replacement.binding.file_version = Some(version(b"old!"));
        let error = adapter
            .begin_upload(
                replacement,
                &context(&share),
                true,
                UploadAdmission::default(),
                10,
            )
            .expect_err("unimplemented replacement must fail closed");
        assert_eq!(error.code(), "file_transfer_replacement_unsupported");
    }

    #[test]
    fn incomplete_upload_finish_is_retryable() {
        let (temporary, capability, share) = setup();
        let adapter = FileTransferAdapter::new(capability);
        let mut transfer = adapter
            .begin_upload(
                upload_session(&share, "retry.txt", b"abcdefgh", 4, b"retry", 1_000),
                &context(&share),
                true,
                UploadAdmission::default(),
                10,
            )
            .expect("begin upload");
        transfer.write_chunk(0, b"abcd", 11).expect("first chunk");
        let error = transfer
            .finish(12)
            .expect_err("finish must wait for all chunks");
        assert_eq!(error.code(), "file_transfer_incomplete");
        assert_eq!(transfer.session().state, TransferState::Active);
        assert!(!host_path(&temporary, "retry.txt").exists());

        transfer.write_chunk(1, b"efgh", 13).expect("second chunk");
        transfer.finish(14).expect("retry succeeds");
        assert_eq!(
            std::fs::read(host_path(&temporary, "retry.txt")).expect("published file"),
            b"abcdefgh"
        );
    }

    #[test]
    fn disabling_capability_cancels_an_active_upload_and_cleans_staging() {
        let (temporary, capability, share) = setup();
        let adapter = FileTransferAdapter::new(capability.clone());
        let mut transfer = adapter
            .begin_upload(
                upload_session(&share, "revoked.txt", b"abcdefgh", 4, b"revoked", 1_000),
                &context(&share),
                true,
                UploadAdmission::default(),
                10,
            )
            .expect("begin upload");
        transfer.write_chunk(0, b"abcd", 11).expect("first chunk");
        assert_eq!(temp_file_count(&temporary), 1);
        capability.disable(&owner(), 12).expect("disable");

        let error = transfer
            .write_chunk(1, b"efgh", 13)
            .expect_err("revoked upload must stop");
        assert_eq!(error.code(), "file_capability_revoked");
        assert_eq!(transfer.session().state, TransferState::Cancelled);
        assert_eq!(temp_file_count(&temporary), 0);
        assert!(!host_path(&temporary, "revoked.txt").exists());
    }

    #[test]
    fn expired_upload_is_rejected_at_the_boundary() {
        let (temporary, capability, share) = setup();
        let adapter = FileTransferAdapter::new(capability);
        let mut transfer = adapter
            .begin_upload(
                upload_session(&share, "expired.txt", b"abcd", 4, b"expired", 20),
                &context(&share),
                true,
                UploadAdmission::default(),
                10,
            )
            .expect("begin upload");

        let error = transfer
            .write_chunk(0, b"abcd", 20)
            .expect_err("expired upload must stop");
        assert_eq!(error.code(), "file_transfer_expired");
        assert_eq!(transfer.session().state, TransferState::Expired);
        assert_eq!(temp_file_count(&temporary), 0);
        assert!(!host_path(&temporary, "expired.txt").exists());
    }

    #[test]
    fn paused_upload_can_resume_and_finish() {
        let (temporary, capability, share) = setup();
        let adapter = FileTransferAdapter::new(capability);
        let mut transfer = adapter
            .begin_upload(
                upload_session(&share, "paused.txt", b"abcdefgh", 4, b"paused", 1_000),
                &context(&share),
                true,
                UploadAdmission::default(),
                10,
            )
            .expect("begin upload");
        transfer.write_chunk(0, b"abcd", 11).expect("first chunk");
        transfer.pause(12).expect("pause");
        assert_eq!(transfer.session().state, TransferState::Paused);
        transfer.resume(&context(&share), true, 13).expect("resume");
        transfer.write_chunk(1, b"efgh", 14).expect("second chunk");
        transfer.finish(15).expect("finish");
        assert_eq!(
            std::fs::read(host_path(&temporary, "paused.txt")).expect("published file"),
            b"abcdefgh"
        );
    }

    #[test]
    fn concurrent_same_name_uploads_publish_only_one_file() {
        let (temporary, capability, share) = setup();
        let adapter = FileTransferAdapter::new(capability);
        let content = b"same-name";

        let mut first = adapter
            .begin_upload(
                upload_session(&share, "race.txt", content, 4, b"race-one", 1_000),
                &context(&share),
                true,
                UploadAdmission::default(),
                10,
            )
            .expect("first upload");
        let mut second = adapter
            .begin_upload(
                upload_session(&share, "race.txt", content, 4, b"race-two", 1_000),
                &context(&share),
                true,
                UploadAdmission::default(),
                10,
            )
            .expect("second upload");
        for (index, chunk) in content.chunks(4).enumerate() {
            let index = u32::try_from(index).expect("index");
            first.write_chunk(index, chunk, 11).expect("first chunk");
            second.write_chunk(index, chunk, 11).expect("second chunk");
        }

        first.finish(20).expect("first publication");
        let error = second.finish(21).expect_err("second publication must fail");
        assert_eq!(error.code(), "file_target_exists");
        assert_eq!(second.session().state, TransferState::Failed);
        assert_eq!(
            std::fs::read(host_path(&temporary, "race.txt")).expect("published file"),
            content
        );
    }

    #[test]
    fn cancelled_upload_leaves_no_visible_or_staged_file() {
        let (temporary, capability, share) = setup();
        let adapter = FileTransferAdapter::new(capability);
        let mut transfer = adapter
            .begin_upload(
                upload_session(&share, "cancelled.txt", b"abcdefgh", 4, b"cancelled", 1_000),
                &context(&share),
                true,
                UploadAdmission::default(),
                10,
            )
            .expect("begin upload");
        transfer.write_chunk(0, b"abcd", 11).expect("first chunk");
        assert_eq!(temp_file_count(&temporary), 1);
        transfer.cancel(12).expect("cancel");
        assert_eq!(temp_file_count(&temporary), 0);
        assert!(!host_path(&temporary, "cancelled.txt").exists());
    }

    #[test]
    fn download_streams_checksummed_chunks_and_completes() {
        let (temporary, capability, share) = setup();
        let content = b"abcdefghij";
        std::fs::write(host_path(&temporary, "source.txt"), content).expect("source");
        let adapter = FileTransferAdapter::new(capability);
        let expected = version(content);
        let mut transfer = adapter
            .begin_download(
                download_session(&share, "source.txt", content, 4, expected, 1_000),
                &context(&share),
                true,
                10,
            )
            .expect("begin download");

        assert_eq!(
            transfer.read_next(11).expect("first"),
            Some(b"abcd".to_vec())
        );
        assert_eq!(
            transfer.read_next(12).expect("second"),
            Some(b"efgh".to_vec())
        );
        assert_eq!(transfer.read_next(13).expect("third"), Some(b"ij".to_vec()));
        assert_eq!(transfer.read_next(14).expect("done"), None);
        let progress = transfer.finish(15).expect("finish");
        assert_eq!(progress.state, TransferState::Completed);
        assert_eq!(progress.transferred_bytes, 10);
        assert_eq!(transfer.finish(16).expect("idempotent finish"), progress);
    }

    #[test]
    fn download_rejects_a_changed_source_version() {
        let (temporary, capability, share) = setup();
        let original = b"original";
        std::fs::write(host_path(&temporary, "source.txt"), original).expect("source");
        let adapter = FileTransferAdapter::new(capability);
        let expected = version(original);
        std::fs::write(host_path(&temporary, "source.txt"), b"changed!").expect("change");

        let error = adapter
            .begin_download(
                download_session(&share, "source.txt", original, 4, expected, 1_000),
                &context(&share),
                true,
                10,
            )
            .expect_err("changed source must be rejected");
        assert_eq!(error.code(), "file_transfer_version_changed");
    }

    #[test]
    fn paused_download_can_resume_and_finish() {
        let (temporary, capability, share) = setup();
        let content = b"abcdefgh";
        std::fs::write(host_path(&temporary, "source.txt"), content).expect("source");
        let adapter = FileTransferAdapter::new(capability);
        let expected = version(content);
        let mut transfer = adapter
            .begin_download(
                download_session(&share, "source.txt", content, 4, expected, 1_000),
                &context(&share),
                true,
                10,
            )
            .expect("begin download");
        assert_eq!(
            transfer.read_next(11).expect("first"),
            Some(b"abcd".to_vec())
        );
        transfer.pause(12).expect("pause");
        assert_eq!(transfer.session().state, TransferState::Paused);
        transfer.resume(&context(&share), true, 13).expect("resume");
        assert_eq!(
            transfer.read_next(14).expect("second"),
            Some(b"efgh".to_vec())
        );
        transfer.finish(15).expect("finish");
        assert_eq!(transfer.session().state, TransferState::Completed);
    }

    #[test]
    fn download_resume_rechecks_the_source_version() {
        let (temporary, capability, share) = setup();
        let original = b"abcdefgh";
        std::fs::write(host_path(&temporary, "source.txt"), original).expect("source");
        let adapter = FileTransferAdapter::new(capability);
        let expected = version(original);
        let mut transfer = adapter
            .begin_download(
                download_session(&share, "source.txt", original, 4, expected, 1_000),
                &context(&share),
                true,
                10,
            )
            .expect("begin download");
        assert_eq!(
            transfer.read_next(11).expect("first"),
            Some(b"abcd".to_vec())
        );
        transfer.pause(12).expect("pause");
        std::fs::write(host_path(&temporary, "source.txt"), b"12345678").expect("replace source");

        let error = transfer
            .resume(&context(&share), true, 13)
            .expect_err("changed source must not resume");
        assert_eq!(error.code(), "file_transfer_version_changed");
        assert_eq!(transfer.session().state, TransferState::Failed);
    }

    #[test]
    fn upload_content_hash_mismatch_fails_and_cleans_staging() {
        let (temporary, capability, share) = setup();
        let adapter = FileTransferAdapter::new(capability);
        let mut session = upload_session(
            &share,
            "bad-hash.txt",
            b"abcdefgh",
            4,
            b"upload-bad-hash",
            1_000,
        );
        session.manifest.content_hash = ContentHash::from_bytes(b"wrong digest");
        let mut transfer = adapter
            .begin_upload(
                session,
                &context(&share),
                true,
                UploadAdmission::default(),
                10,
            )
            .expect("begin upload");
        transfer.write_chunk(0, b"abcd", 11).expect("first chunk");
        transfer.write_chunk(1, b"efgh", 12).expect("second chunk");

        let error = transfer
            .finish(13)
            .expect_err("incorrect total hash must never publish");
        assert_eq!(error.code(), "file_transfer_integrity_mismatch");
        assert_eq!(transfer.session().state, TransferState::Failed);
        assert_eq!(temp_file_count(&temporary), 0);
        assert!(!host_path(&temporary, "bad-hash.txt").exists());
    }

    #[test]
    fn download_content_hash_mismatch_never_completes() {
        let (temporary, capability, share) = setup();
        let content = b"abcdefgh";
        std::fs::write(host_path(&temporary, "source.txt"), content).expect("source");
        let adapter = FileTransferAdapter::new(capability);
        let expected = version(content);
        let mut transfer = adapter
            .begin_download(
                download_session(&share, "source.txt", content, 4, expected, 1_000),
                &context(&share),
                true,
                10,
            )
            .expect("begin download");
        transfer.session.manifest.content_hash = ContentHash::from_bytes(b"wrong digest");
        assert_eq!(
            transfer.read_next(11).expect("first"),
            Some(b"abcd".to_vec())
        );
        assert_eq!(
            transfer.read_next(12).expect("second"),
            Some(b"efgh".to_vec())
        );

        let error = transfer
            .finish(13)
            .expect_err("incorrect total hash must fail the download");
        assert_eq!(error.code(), "file_transfer_integrity_mismatch");
        assert_eq!(transfer.session().state, TransferState::Failed);
    }

    #[test]
    fn disabling_capability_cancels_an_active_download() {
        let (temporary, capability, share) = setup();
        let content = b"abcdefgh";
        std::fs::write(host_path(&temporary, "source.txt"), content).expect("source");
        let adapter = FileTransferAdapter::new(capability.clone());
        let expected = version(content);
        let mut transfer = adapter
            .begin_download(
                download_session(&share, "source.txt", content, 4, expected, 1_000),
                &context(&share),
                true,
                10,
            )
            .expect("begin download");
        assert_eq!(
            transfer.read_next(11).expect("first"),
            Some(b"abcd".to_vec())
        );
        capability.disable(&owner(), 12).expect("disable");

        let error = transfer
            .read_next(13)
            .expect_err("revoked download must stop");
        assert_eq!(error.code(), "file_capability_revoked");
        assert_eq!(transfer.session().state, TransferState::Cancelled);
        assert_eq!(
            std::fs::read(host_path(&temporary, "source.txt")).expect("source remains"),
            content
        );
    }

    #[test]
    fn resume_rejects_a_changed_capability_epoch() {
        let (temporary, capability, share) = setup();
        let content = b"abcdefgh";
        std::fs::write(host_path(&temporary, "source.txt"), content).expect("source");
        let adapter = FileTransferAdapter::new(capability.clone());
        let expected = version(content);
        let mut transfer = adapter
            .begin_download(
                download_session(&share, "source.txt", content, 4, expected, 1_000),
                &context(&share),
                true,
                10,
            )
            .expect("begin download");
        transfer.pause(11).expect("pause");

        let mut revised = share.clone();
        revised.revision = 2;
        revised.updated_at_unix_ms = 12;
        capability
            .enable(
                &revised,
                &owner(),
                temporary.path().to_str().expect("utf8 temp path"),
                12,
            )
            .expect("replace share");
        let error = transfer
            .resume(&context(&revised), true, 13)
            .expect_err("old access epoch must not resume");
        assert_eq!(error.code(), "file_transfer_permission_revoked");
        assert_eq!(transfer.session().state, TransferState::Cancelled);
    }
}
