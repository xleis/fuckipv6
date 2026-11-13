//! Requester-side File transfer state machine.
//!
//! The owner-side data plane authenticates incoming requests and writes or
//! reads the shared root. This module drives the other half of the same wire
//! protocol: it builds local upload manifests, sends bounded fragments, stages
//! downloads, and accepts only responses and chunks that match the operation's
//! authenticated peer and exact request id.

use std::collections::{HashSet, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::Ipv6Addr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fuckipv6_common::domain::{
    ContentHash, FileChunk, FileId, FileShareId, FileTransferError, NodeId, OperationId,
    ResumeRequest, SharedPath, TransferBinding, TransferDirection, TransferId, TransferManifest,
    TransferSession, TransferState,
};
use thiserror::Error;
use tokio::sync::Notify;

use super::network::{
    download_fragment_request_id, validate_network_manifest, FileNetworkFrame, FileNetworkProgress,
    FileNetworkRequest, FileNetworkResponse, MAX_FILE_NETWORK_FRAGMENT_BYTES,
    MAX_FILE_NETWORK_MANIFEST_CHUNKS, MAX_FILE_NETWORK_TRANSFER_BYTES,
    MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES,
};
use super::origin_store::{
    FileOriginStore, PersistedDownloadAssembly, PersistedDownloadState, PersistedOriginTransfer,
    PersistedUploadState,
};

/// Match the owner's transfer lifetime. Both ends must reject an operation at
/// the same absolute boundary even if one process was suspended.
pub(crate) const FILE_NETWORK_TRANSFER_TTL_MS: u64 = 30 * 60 * 1_000;
pub(crate) const MAX_ORIGIN_TRANSFERS: usize = 16;
const MAX_ORIGIN_EVENTS: usize = 256;

#[derive(Debug, Error)]
pub(crate) enum FileNetworkOriginError {
    #[error("File origin transfer limit reached")]
    TransferLimit,
    #[error("File transfer already exists")]
    DuplicateTransfer,
    #[error("File transfer request is invalid")]
    InvalidRequest,
    #[error("File transfer operation was not found")]
    NotFound,
    #[error("File transfer operation is not in a state that allows this action")]
    InvalidState,
    #[error("File upload source is not a regular seekable file")]
    InvalidSource,
    #[error("File download destination is invalid")]
    InvalidDestination,
    #[error("File response is not associated with an active request")]
    UnknownResponse,
    #[error("File frame came from a peer other than the operation peer")]
    PeerMismatch,
    #[error("File response is not valid for the outstanding request")]
    UnexpectedResponse,
    #[error("File response binding does not match the operation")]
    BindingMismatch,
    #[error("File transfer access was revoked")]
    AccessRevoked,
    #[error("File download chunk is out of order, out of range or replayed")]
    ChunkOutOfOrder,
    #[error("File download chunk checksum or length is invalid")]
    ChunkCorrupt,
    #[error("File transfer filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("File transfer session operation failed: {0}")]
    Transfer(#[from] FileTransferError),
    #[error("File transfer pause persistence failed: {0}")]
    Persistence(#[source] anyhow::Error),
}

impl FileNetworkOriginError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::TransferLimit => "file_transfer_limit",
            Self::DuplicateTransfer => "file_transfer_operation_active",
            Self::InvalidRequest => "file_transfer_invalid_request",
            Self::NotFound => "file_transfer_not_found",
            Self::InvalidState => "file_transfer_invalid_state",
            Self::InvalidSource => "file_transfer_source_invalid",
            Self::InvalidDestination => "file_transfer_destination_invalid",
            Self::UnknownResponse => "file_transfer_response_unknown",
            Self::PeerMismatch => "file_transfer_peer_mismatch",
            Self::UnexpectedResponse => "file_transfer_response_unexpected",
            Self::BindingMismatch => "file_transfer_binding_mismatch",
            Self::AccessRevoked => "file_transfer_access_revoked",
            Self::ChunkOutOfOrder => "file_transfer_chunk_out_of_order",
            Self::ChunkCorrupt => "file_transfer_chunk_corrupt",
            Self::Io(_) => "file_transfer_io",
            Self::Transfer(error) => error.code(),
            Self::Persistence(_) => "file_transfer_storage",
        }
    }
}

/// Stable, UI-safe classification for terminal requester transfer errors.
/// Wire errors remain individual machine codes; this grouping lets local
/// surfaces distinguish the four operator actions required by W15-03 without
/// parsing human-readable messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileTransferFailureKind {
    Network,
    Authorization,
    Quota,
    Storage,
    Other,
}

impl FileTransferFailureKind {
    pub(crate) fn from_code(code: &str) -> Self {
        match code {
            "file_transfer_send_failed"
            | "file_transfer_session_unavailable"
            | "file_transfer_peer_unavailable" => Self::Network,
            "file_transfer_authorization_lost"
            | "file_transfer_access_revoked"
            | "file_transfer_permission_revoked" => Self::Authorization,
            "file_too_large"
            | "file_total_quota_exceeded"
            | "file_member_quota_exceeded"
            | "file_entry_limit"
            | "file_concurrency_limit"
            | "file_member_concurrency_limit"
            | "file_directory_concurrency_limit"
            | "file_transfer_rate_limited" => Self::Quota,
            "file_disk_space_reserved"
            | "file_transfer_io"
            | "file_transfer_storage"
            | "file_io_error"
            | "file_temp_unavailable" => Self::Storage,
            _ => Self::Other,
        }
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Authorization => "authorization",
            Self::Quota => "quota",
            Self::Storage => "storage",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OriginPeer {
    pub(crate) node_id: NodeId,
    pub(crate) virtual_ip: Ipv6Addr,
}

#[derive(Debug)]
pub(crate) struct OriginOutbound {
    pub(crate) peer: OriginPeer,
    pub(crate) transfer_id: TransferId,
    pub(crate) operation_id: OperationId,
    pub(crate) actor_access_epoch: Option<u64>,
    pub(crate) frame: FileNetworkFrame,
}

/// Progress is local-only and intentionally contains no filesystem paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OriginTransferEvent {
    pub(crate) transfer_id: TransferId,
    pub(crate) operation_id: OperationId,
    pub(crate) direction: TransferDirection,
    pub(crate) state: TransferState,
    pub(crate) transferred_bytes: u64,
    pub(crate) total_bytes: u64,
    pub(crate) received_chunks: u32,
    pub(crate) total_chunks: u32,
    pub(crate) error_code: Option<String>,
}

impl OriginTransferEvent {
    pub(crate) fn failure_kind(&self) -> Option<FileTransferFailureKind> {
        self.error_code
            .as_deref()
            .map(FileTransferFailureKind::from_code)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct OriginTransferKey {
    transfer_id: String,
    operation_id: String,
}

impl OriginTransferKey {
    fn new(transfer_id: &TransferId, operation_id: &OperationId) -> Self {
        Self {
            transfer_id: transfer_id.as_str().to_owned(),
            operation_id: operation_id.as_str().to_owned(),
        }
    }
}

#[derive(Debug)]
struct OriginTransfer {
    peer: OriginPeer,
    share_id: FileShareId,
    transfer_id: TransferId,
    operation_id: OperationId,
    path: SharedPath,
    direction: TransferDirection,
    deadline_unix_ms: u64,
    pending: Option<PendingOriginReply>,
    retry_at_unix_ms: Option<u64>,
    binding: Option<TransferBinding>,
    paused: bool,
    pause_after_reply: bool,
    next_request_sequence: u64,
    resume_generation: u64,
    state: OriginTransferState,
}

#[derive(Debug)]
enum OriginTransferState {
    Upload(UploadOriginState),
    Download(Box<DownloadOriginState>),
}

#[derive(Debug)]
struct UploadOriginState {
    source: File,
    source_path: PathBuf,
    manifest: TransferManifest,
    next_chunk: u32,
    next_offset: u64,
    completed_chunks: u32,
}

#[derive(Debug)]
struct DownloadOriginState {
    destination: PathBuf,
    session: Option<Box<TransferSession>>,
    staging: Option<DownloadStaging>,
    assembly: Option<DownloadAssembly>,
    hasher: blake3::Hasher,
    received_bytes: u64,
    received_chunks: u32,
}

#[derive(Debug)]
struct DownloadStaging {
    file: Option<File>,
    path: PathBuf,
    durable: bool,
}

impl DownloadStaging {
    fn create(parent: &Path) -> Result<Self, FileNetworkOriginError> {
        let named = tempfile::Builder::new()
            .prefix(".fkfile-download-")
            .tempfile_in(parent)?;
        let (file, path) = named
            .keep()
            .map_err(|error| FileNetworkOriginError::Io(error.error))?;
        Ok(Self {
            file: Some(file),
            path,
            durable: false,
        })
    }

    fn open_existing(path: &Path) -> Result<Self, FileNetworkOriginError> {
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.file_type().is_file() {
            return Err(FileNetworkOriginError::InvalidDestination);
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        let file = options.open(path)?;
        Ok(Self {
            file: Some(file),
            path: path.to_path_buf(),
            durable: true,
        })
    }

    fn as_file(&self) -> &File {
        self.file
            .as_ref()
            .expect("download staging file is present")
    }

    fn file_mut(&mut self) -> &mut File {
        self.file
            .as_mut()
            .expect("download staging file is present")
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn mark_durable(&mut self) {
        self.durable = true;
    }

    fn mark_transient(&mut self) {
        self.durable = false;
    }

    fn prepare_for_resume(&mut self, received_bytes: u64) -> std::io::Result<()> {
        self.file_mut().set_len(received_bytes)?;
        self.file_mut().seek(SeekFrom::Start(received_bytes))?;
        Ok(())
    }

    fn persist_noclobber(mut self, destination: &Path) -> Result<(), PersistStagingError> {
        let file = self
            .file
            .as_mut()
            .expect("download staging file is present");
        if let Err(error) = file.flush().and_then(|()| file.sync_all()) {
            return Err(PersistStagingError { file: self, error });
        }
        let result = std::fs::hard_link(&self.path, destination)
            .and_then(|()| std::fs::remove_file(&self.path));
        match result {
            Ok(()) => {
                self.file.take();
                Ok(())
            }
            Err(error) => Err(PersistStagingError { file: self, error }),
        }
    }
}

impl Write for DownloadStaging {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.file_mut().write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file_mut().flush()
    }
}

impl Drop for DownloadStaging {
    fn drop(&mut self) {
        if !self.durable {
            self.file.take();
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[derive(Debug)]
struct PersistStagingError {
    file: DownloadStaging,
    error: std::io::Error,
}

#[derive(Debug)]
struct DownloadAssembly {
    chunk_index: u32,
    expected_offset: u64,
    expected_length: u64,
    expected_checksum: ContentHash,
    bytes: Vec<u8>,
}

#[derive(Debug)]
struct PendingOriginReply {
    request_id: [u8; 16],
    kind: OriginRequestKind,
    retry_frame: Option<FileNetworkFrame>,
}

impl PendingOriginReply {
    fn new(request_id: [u8; 16], kind: OriginRequestKind) -> Self {
        Self {
            request_id,
            kind,
            retry_frame: None,
        }
    }

    fn retryable(request_id: [u8; 16], kind: OriginRequestKind, frame: FileNetworkFrame) -> Self {
        Self {
            request_id,
            kind,
            retry_frame: Some(frame),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OriginRequestKind {
    OpenUpload,
    UploadChunk {
        chunk_index: u32,
        offset: u64,
        length: u64,
    },
    FinishUpload,
    OpenDownload,
    FinishDownload,
    Pause,
    ResumeUpload,
    ResumeDownload,
}

/// Bounded requester-side operation table. All filesystem failures fail the
/// affected operation closed and drop its staging file; other operations keep
/// running.
#[derive(Debug)]
pub(crate) struct FileNetworkOrigin {
    local_node_id: NodeId,
    transfers: std::collections::BTreeMap<OriginTransferKey, OriginTransfer>,
    outbound: VecDeque<OriginOutbound>,
    events: VecDeque<OriginTransferEvent>,
    outbound_notify: Arc<Notify>,
    store: Option<FileOriginStore>,
}

impl FileNetworkOrigin {
    pub(crate) fn new(local_node_id: NodeId) -> Self {
        Self {
            local_node_id,
            transfers: std::collections::BTreeMap::new(),
            outbound: VecDeque::new(),
            events: VecDeque::new(),
            outbound_notify: Arc::new(Notify::new()),
            store: None,
        }
    }

    pub(crate) fn with_store(local_node_id: NodeId, store: FileOriginStore) -> Self {
        let mut origin = Self::new(local_node_id);
        origin.store = Some(store);
        origin
    }

    /// Restore only transfers that had a confirmed pause before the process
    /// exited. Every journal entry is revalidated against the local identity,
    /// binding, manifest, and filesystem before it can be resumed.
    pub(crate) fn restore_paused(
        &mut self,
        now_unix_ms: u64,
    ) -> Result<Vec<OriginTransferEvent>, FileNetworkOriginError> {
        let Some(store) = self.store.clone() else {
            return Ok(Vec::new());
        };
        let entries = match store.load() {
            Ok(entries) => entries,
            Err(error) => {
                // Never act on a journal that cannot be authenticated by its
                // owner-only permissions and schema.
                let _ = store.save(&[]);
                return Err(FileNetworkOriginError::Persistence(error));
            }
        };
        let mut events = Vec::new();
        let mut claimed_staging_paths = self
            .transfers
            .values()
            .filter_map(|transfer| match &transfer.state {
                OriginTransferState::Download(download) => download
                    .staging
                    .as_ref()
                    .map(|staging| staging.path().to_path_buf()),
                OriginTransferState::Upload(_) => None,
            })
            .collect::<HashSet<_>>();
        for entry in entries {
            let key = OriginTransferKey::new(&entry.transfer_id, &entry.operation_id);
            let staging_already_claimed = entry
                .download
                .as_ref()
                .is_some_and(|download| claimed_staging_paths.contains(&download.staging_path));
            let restored = if self.transfers.contains_key(&key) || staging_already_claimed {
                Err(FileNetworkOriginError::DuplicateTransfer)
            } else if self.transfers.len() >= MAX_ORIGIN_TRANSFERS {
                Err(FileNetworkOriginError::TransferLimit)
            } else {
                restore_paused_transfer(&entry, &self.local_node_id, now_unix_ms)
            };
            match restored {
                Ok(transfer) => {
                    if let OriginTransferState::Download(download) = &transfer.state {
                        if let Some(staging) = download.staging.as_ref() {
                            claimed_staging_paths.insert(staging.path().to_path_buf());
                        }
                    }
                    let event = transfer.event(TransferState::Paused, None);
                    self.push_event(event.clone());
                    events.push(event);
                    self.transfers.insert(key, transfer);
                }
                Err(error) => {
                    // Duplicate and capacity failures do not prove the staged
                    // file is invalid. In particular, a duplicate journal entry
                    // may point at the staging file owned by the first entry.
                    if !matches!(
                        error,
                        FileNetworkOriginError::DuplicateTransfer
                            | FileNetworkOriginError::TransferLimit
                    ) {
                        cleanup_restore_staging(&entry);
                    }
                    let state = if now_unix_ms >= entry.deadline_unix_ms
                        || now_unix_ms >= entry.binding.expires_at_unix_ms
                        || matches!(
                            error,
                            FileNetworkOriginError::Transfer(FileTransferError::Expired)
                        ) {
                        TransferState::Expired
                    } else {
                        TransferState::Failed
                    };
                    let code = if state == TransferState::Expired {
                        "file_transfer_expired"
                    } else {
                        error.code()
                    };
                    let event = restore_failure_event(&entry, state, code);
                    self.push_event(event.clone());
                    events.push(event);
                }
            }
        }
        self.save_paused(now_unix_ms)?;
        Ok(events)
    }

    fn save_paused(&mut self, now_unix_ms: u64) -> Result<(), FileNetworkOriginError> {
        let Some(store) = self.store.clone() else {
            return Ok(());
        };
        let transfers = self.persisted_transfers(now_unix_ms)?;
        store
            .save(&transfers)
            .map_err(FileNetworkOriginError::Persistence)?;
        for transfer in self.transfers.values_mut() {
            if !transfer.paused || transfer.pending.is_some() || transfer.pause_after_reply {
                continue;
            }
            if let OriginTransferState::Download(download) = &mut transfer.state {
                if let Some(staging) = download.staging.as_mut() {
                    staging.mark_durable();
                }
            }
        }
        Ok(())
    }

    fn persisted_transfers(
        &mut self,
        now_unix_ms: u64,
    ) -> Result<Vec<PersistedOriginTransfer>, FileNetworkOriginError> {
        let mut persisted = Vec::new();
        for transfer in self.transfers.values_mut() {
            if !transfer.paused
                || transfer.pending.is_some()
                || transfer.pause_after_reply
                || now_unix_ms >= transfer.deadline_unix_ms
            {
                continue;
            }
            persisted.push(persisted_transfer(transfer)?);
        }
        Ok(persisted)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start_upload(
        &mut self,
        peer: OriginPeer,
        share_id: FileShareId,
        transfer_id: TransferId,
        operation_id: OperationId,
        path: SharedPath,
        source_path: &Path,
        chunk_size: u64,
        now_unix_ms: u64,
    ) -> Result<(), FileNetworkOriginError> {
        self.ensure_capacity(&transfer_id, &operation_id)?;
        if chunk_size == 0 || chunk_size > MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES as u64 {
            return Err(FileNetworkOriginError::InvalidRequest);
        }
        let (mut source, manifest) = build_upload_manifest(&path, source_path, chunk_size)?;
        source.seek(SeekFrom::Start(0))?;
        let key = OriginTransferKey::new(&transfer_id, &operation_id);
        let deadline_unix_ms = now_unix_ms.saturating_add(FILE_NETWORK_TRANSFER_TTL_MS);
        let mut transfer = OriginTransfer {
            peer,
            share_id: share_id.clone(),
            transfer_id: transfer_id.clone(),
            operation_id: operation_id.clone(),
            path: path.clone(),
            direction: TransferDirection::Upload,
            deadline_unix_ms,
            pending: None,
            retry_at_unix_ms: None,
            binding: None,
            paused: false,
            pause_after_reply: false,
            next_request_sequence: 0,
            resume_generation: 0,
            state: OriginTransferState::Upload(UploadOriginState {
                source,
                source_path: source_path.to_path_buf(),
                manifest,
                next_chunk: 0,
                next_offset: 0,
                completed_chunks: 0,
            }),
        };
        queue_open_upload(
            &mut transfer,
            &mut self.outbound,
            share_id,
            path,
            now_unix_ms,
        )?;
        self.push_event(transfer.event(TransferState::Queued, None));
        self.transfers.insert(key, transfer);
        self.outbound_notify.notify_one();
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start_download(
        &mut self,
        peer: OriginPeer,
        share_id: FileShareId,
        transfer_id: TransferId,
        operation_id: OperationId,
        path: SharedPath,
        destination: PathBuf,
        now_unix_ms: u64,
    ) -> Result<(), FileNetworkOriginError> {
        self.ensure_capacity(&transfer_id, &operation_id)?;
        validate_download_destination(&destination)?;
        let key = OriginTransferKey::new(&transfer_id, &operation_id);
        let deadline_unix_ms = now_unix_ms.saturating_add(FILE_NETWORK_TRANSFER_TTL_MS);
        let request_id =
            origin_request_id(&transfer_id, &operation_id, b"open-download", 0, 0, 0, 0);
        let request = FileNetworkRequest::OpenDownload {
            share_id: share_id.clone(),
            transfer_id: transfer_id.clone(),
            operation_id: operation_id.clone(),
            path: path.clone(),
        };
        let frame = FileNetworkFrame::request(request_id, request)
            .map_err(|_| FileNetworkOriginError::InvalidRequest)?;
        let transfer = OriginTransfer {
            peer: peer.clone(),
            share_id: share_id.clone(),
            transfer_id: transfer_id.clone(),
            operation_id: operation_id.clone(),
            path,
            direction: TransferDirection::Download,
            deadline_unix_ms,
            pending: Some(PendingOriginReply::new(
                request_id,
                OriginRequestKind::OpenDownload,
            )),
            retry_at_unix_ms: None,
            binding: None,
            paused: false,
            pause_after_reply: false,
            next_request_sequence: 1,
            resume_generation: 0,
            state: OriginTransferState::Download(Box::new(DownloadOriginState {
                destination,
                session: None,
                staging: None,
                assembly: None,
                hasher: blake3::Hasher::new(),
                received_bytes: 0,
                received_chunks: 0,
            })),
        };
        self.outbound.push_back(OriginOutbound {
            peer,
            transfer_id: transfer_id.clone(),
            operation_id: operation_id.clone(),
            actor_access_epoch: None,
            frame,
        });
        self.push_event(transfer.event(TransferState::Queued, None));
        self.transfers.insert(key, transfer);
        self.outbound_notify.notify_one();
        Ok(())
    }

    pub(crate) fn pop_outbound(&mut self) -> Option<OriginOutbound> {
        self.outbound.pop_front()
    }

    /// Return the next point at which a delayed upload fragment or the owning
    /// transfer deadline needs runtime attention. Rate-limit responses keep the
    /// exact pending frame so requester-side retries never advance to another
    /// chunk or rebuild different bytes.
    pub(crate) fn next_retry_at_unix_ms(&self) -> Option<u64> {
        self.transfers
            .values()
            .filter_map(|transfer| {
                transfer
                    .retry_at_unix_ms
                    .map(|retry_at| retry_at.min(transfer.deadline_unix_ms))
            })
            .min()
    }

    /// Requeue every rate-limited upload fragment whose retry interval has
    /// elapsed. Expiry always wins over a retry so a delayed fragment cannot
    /// keep a transfer alive beyond its authenticated deadline.
    pub(crate) fn queue_due_retries(&mut self, now_unix_ms: u64) {
        let due = self
            .transfers
            .iter()
            .filter(|(_, transfer)| {
                now_unix_ms >= transfer.deadline_unix_ms
                    || transfer
                        .retry_at_unix_ms
                        .is_some_and(|retry_at| retry_at <= now_unix_ms)
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        let mut queued = false;
        for key in due {
            let Some(transfer) = self.transfers.get_mut(&key) else {
                continue;
            };
            if now_unix_ms >= transfer.deadline_unix_ms {
                self.fail_transfer_at(&key, now_unix_ms, "file_transfer_expired", false);
                continue;
            }
            let Some(pending) = transfer.pending.as_ref() else {
                transfer.retry_at_unix_ms = None;
                continue;
            };
            let Some(frame) = pending.retry_frame.clone() else {
                transfer.retry_at_unix_ms = None;
                self.fail_transfer_at(&key, now_unix_ms, "file_transfer_retry_unavailable", false);
                continue;
            };
            transfer.retry_at_unix_ms = None;
            self.outbound.push_back(OriginOutbound {
                peer: transfer.peer.clone(),
                transfer_id: transfer.transfer_id.clone(),
                operation_id: transfer.operation_id.clone(),
                actor_access_epoch: outbound_access_epoch(transfer),
                frame,
            });
            queued = true;
        }
        if queued {
            self.outbound_notify.notify_one();
        }
    }

    pub(crate) fn outbound_notify(&self) -> Arc<Notify> {
        self.outbound_notify.clone()
    }

    pub(crate) fn local_node_id(&self) -> &NodeId {
        &self.local_node_id
    }

    pub(crate) fn drain_events(&mut self) -> Vec<OriginTransferEvent> {
        self.events.drain(..).collect()
    }

    #[cfg(test)]
    pub(crate) fn active_count(&self) -> usize {
        self.transfers.len()
    }

    pub(crate) fn owns_download(
        &self,
        peer: &NodeId,
        transfer_id: &TransferId,
        operation_id: &OperationId,
    ) -> bool {
        self.transfers
            .get(&OriginTransferKey::new(transfer_id, operation_id))
            .is_some_and(|transfer| {
                &transfer.peer.node_id == peer && transfer.direction == TransferDirection::Download
            })
    }

    /// Route one authenticated response. Unknown correlations are rejected
    /// without touching another transfer. A response correlated to an active
    /// request but misbound to another operation fails that operation closed.
    pub(crate) fn handle_response(
        &mut self,
        peer: &OriginPeer,
        request_id: [u8; 16],
        response: FileNetworkResponse,
        now_unix_ms: u64,
    ) -> Result<(), FileNetworkOriginError> {
        let key = self
            .transfers
            .iter()
            .find(|(_, transfer)| {
                transfer
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.request_id == request_id)
            })
            .map(|(key, _)| key.clone())
            .ok_or(FileNetworkOriginError::UnknownResponse)?;
        let transfer = self
            .transfers
            .get(&key)
            .ok_or(FileNetworkOriginError::UnknownResponse)?;
        if transfer.peer != *peer {
            return Err(FileNetworkOriginError::PeerMismatch);
        }
        // Response processing may queue the next upload fragment, finish
        // request, or cancellation. Wake the runtime pump on every matched
        // response so transfer progress is not tied to a polling interval.
        self.outbound_notify.notify_one();
        if now_unix_ms >= transfer.deadline_unix_ms {
            self.fail_transfer_at(&key, now_unix_ms, "file_transfer_expired", false);
            return Ok(());
        }
        if let FileNetworkResponse::Error {
            code,
            retry_after_ms,
        } = &response
        {
            debug_assert!(!code.is_empty());
            if code == "file_transfer_rate_limited" {
                if let Some(retry_after_ms) = *retry_after_ms {
                    let transfer = self
                        .transfers
                        .get_mut(&key)
                        .ok_or(FileNetworkOriginError::UnknownResponse)?;
                    let retryable_chunk = transfer.pending.as_ref().is_some_and(|pending| {
                        matches!(pending.kind, OriginRequestKind::UploadChunk { .. })
                            && pending.retry_frame.is_some()
                    });
                    if retryable_chunk {
                        transfer.retry_at_unix_ms =
                            Some(now_unix_ms.saturating_add(retry_after_ms));
                        self.outbound_notify.notify_one();
                        return Ok(());
                    }
                }
            }
            self.fail_transfer_at(&key, now_unix_ms, code, false);
            return Ok(());
        }
        let kind = transfer
            .pending
            .as_ref()
            .expect("pending response exists")
            .kind;
        if let Err(error) = self.validate_response(transfer, kind, &response) {
            let code = error.code();
            self.fail_transfer_at(&key, now_unix_ms, code, true);
            return Err(error);
        }
        let transfer = self
            .transfers
            .get_mut(&key)
            .ok_or(FileNetworkOriginError::UnknownResponse)?;
        transfer.retry_at_unix_ms = None;
        let pending = transfer.pending.take().expect("pending response exists");
        debug_assert_eq!(pending.request_id, request_id);
        let result = match (pending.kind, response) {
            (OriginRequestKind::OpenUpload, FileNetworkResponse::Opened { session }) => {
                self.apply_open_upload(&key, *session, now_unix_ms)
            }
            (OriginRequestKind::OpenDownload, FileNetworkResponse::Opened { session }) => {
                self.apply_open_download(&key, *session, now_unix_ms)
            }
            (OriginRequestKind::ResumeUpload, FileNetworkResponse::Opened { session }) => {
                self.apply_resume_upload(&key, *session, now_unix_ms)
            }
            (OriginRequestKind::ResumeDownload, FileNetworkResponse::Opened { session }) => {
                self.apply_resume_download(&key, *session, now_unix_ms)
            }
            (
                OriginRequestKind::UploadChunk {
                    chunk_index,
                    offset,
                    length,
                },
                FileNetworkResponse::ChunkAccepted {
                    chunk_index: acknowledged_chunk,
                    chunk_complete,
                    progress,
                },
            ) => self.apply_upload_ack(
                &key,
                chunk_index,
                offset,
                length,
                acknowledged_chunk,
                chunk_complete,
                progress,
                now_unix_ms,
            ),
            (OriginRequestKind::FinishUpload, FileNetworkResponse::Completed { progress }) => {
                self.apply_finish(&key, progress, now_unix_ms)
            }
            (OriginRequestKind::FinishDownload, FileNetworkResponse::Completed { progress }) => {
                self.apply_finish(&key, progress, now_unix_ms)
            }
            (OriginRequestKind::Pause, FileNetworkResponse::Progress { progress }) => {
                self.apply_pause(&key, progress, now_unix_ms)
            }
            _ => Err(FileNetworkOriginError::UnexpectedResponse),
        };
        match result {
            Ok(()) => {
                if let Err(error) = self.queue_deferred_pause(&key) {
                    let code = error.code();
                    self.fail_transfer_at(&key, now_unix_ms, code, true);
                    return Err(error);
                }
                self.outbound_notify.notify_one();
                Ok(())
            }
            Err(error) => {
                let code = error.code();
                self.fail_transfer_at(&key, now_unix_ms, code, true);
                Err(error)
            }
        }
    }

    /// Feed one authenticated download fragment. The caller must first call
    /// [`Self::owns_download`] so an upload fragment can still reach the local
    /// owner data plane.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn handle_chunk(
        &mut self,
        peer: &OriginPeer,
        request_id: [u8; 16],
        transfer_id: &TransferId,
        operation_id: &OperationId,
        resume_generation: u64,
        chunk_index: u32,
        offset: u64,
        data: &[u8],
        now_unix_ms: u64,
    ) -> Result<(), FileNetworkOriginError> {
        let key = OriginTransferKey::new(transfer_id, operation_id);
        let finished = {
            let transfer = self
                .transfers
                .get_mut(&key)
                .ok_or(FileNetworkOriginError::UnknownResponse)?;
            if transfer.peer != *peer {
                self.outbound_notify.notify_one();
                self.fail_transfer_at(
                    &key,
                    now_unix_ms,
                    FileNetworkOriginError::PeerMismatch.code(),
                    true,
                );
                return Err(FileNetworkOriginError::PeerMismatch);
            }
            if transfer.direction != TransferDirection::Download {
                return Err(FileNetworkOriginError::PeerMismatch);
            }
            if resume_generation < transfer.resume_generation {
                // Pause is a stream barrier. Fragments emitted before the
                // resumed generation may still be queued by the network and
                // must be discarded without weakening current-generation
                // offset and chunk ordering checks.
                return Ok(());
            }
            if resume_generation != transfer.resume_generation {
                self.fail_transfer_at(
                    &key,
                    now_unix_ms,
                    "file_transfer_resume_generation_mismatch",
                    true,
                );
                return Err(FileNetworkOriginError::ChunkOutOfOrder);
            }
            if transfer.paused {
                return Ok(());
            }
            // Receiving the final fragment queues a Finish request. Signal
            // even for intermediate fragments so events are drained promptly.
            self.outbound_notify.notify_one();
            if now_unix_ms >= transfer.deadline_unix_ms {
                self.fail_transfer_at(&key, now_unix_ms, "file_transfer_expired", false);
                return Ok(());
            }
            if request_id != download_fragment_request_id(operation_id, chunk_index, offset) {
                self.fail_transfer_at(
                    &key,
                    now_unix_ms,
                    FileNetworkOriginError::ChunkOutOfOrder.code(),
                    true,
                );
                return Err(FileNetworkOriginError::ChunkOutOfOrder);
            }
            if let Err(error) =
                apply_download_fragment(transfer, chunk_index, offset, data, now_unix_ms)
            {
                let code = error.code();
                self.fail_transfer_at(&key, now_unix_ms, code, true);
                return Err(error);
            }
            transfer.download_finished_receiving()
        };
        if finished {
            if let Err(error) = self.finish_download_receiving(&key, now_unix_ms) {
                let code = error.code();
                self.fail_transfer_at(&key, now_unix_ms, code, true);
                return Err(error);
            }
        }
        let transfer = self
            .transfers
            .get(&key)
            .ok_or(FileNetworkOriginError::UnknownResponse)?;
        let state = if transfer
            .download_session()
            .is_some_and(|session| session.state == TransferState::Verifying)
        {
            TransferState::Verifying
        } else {
            TransferState::Active
        };
        self.push_event(transfer.event(state, None));
        Ok(())
    }

    pub(crate) fn fail_transfer(
        &mut self,
        transfer_id: &TransferId,
        operation_id: &OperationId,
        now_unix_ms: u64,
        code: &str,
    ) -> bool {
        let failed = self.fail_transfer_at(
            &OriginTransferKey::new(transfer_id, operation_id),
            now_unix_ms,
            code,
            false,
        );
        if failed {
            self.outbound_notify.notify_one();
        }
        failed
    }

    /// Fail the operation whose exact authenticated request id is outstanding.
    /// This is used when the runtime can no longer authorize a response before
    /// it reaches the state machine, so the local operation and any staging
    /// file must be closed without trusting the frame's transfer identifiers.
    pub(crate) fn fail_pending_request(
        &mut self,
        request_id: [u8; 16],
        now_unix_ms: u64,
        code: &str,
    ) -> bool {
        let key = self
            .transfers
            .iter()
            .find(|(_, transfer)| {
                transfer
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.request_id == request_id)
            })
            .map(|(key, _)| key.clone());
        let Some(key) = key else {
            return false;
        };
        let failed = self.fail_transfer_at(&key, now_unix_ms, code, false);
        if failed {
            self.outbound_notify.notify_one();
        }
        failed
    }

    pub(crate) fn cancel(
        &mut self,
        transfer_id: &TransferId,
        operation_id: &OperationId,
        now_unix_ms: u64,
    ) -> bool {
        let cancelled = self.fail_transfer_at(
            &OriginTransferKey::new(transfer_id, operation_id),
            now_unix_ms,
            "file_transfer_cancelled",
            true,
        );
        if cancelled {
            self.outbound_notify.notify_one();
        }
        cancelled
    }

    /// Pause at the next safe request boundary. If a response or upload
    /// fragment is already outstanding, the pause request is deferred until
    /// that exact reply has been applied so upload offsets cannot roll back.
    pub(crate) fn pause(
        &mut self,
        transfer_id: &TransferId,
        operation_id: &OperationId,
        now_unix_ms: u64,
    ) -> Result<(), FileNetworkOriginError> {
        let key = OriginTransferKey::new(transfer_id, operation_id);
        let Some(transfer) = self.transfers.get_mut(&key) else {
            return Err(FileNetworkOriginError::NotFound);
        };
        if now_unix_ms >= transfer.deadline_unix_ms {
            self.fail_transfer_at(&key, now_unix_ms, "file_transfer_expired", false);
            self.outbound_notify.notify_one();
            return Err(FileNetworkOriginError::Transfer(FileTransferError::Expired));
        }
        if transfer.paused
            || transfer.pause_after_reply
            || transfer
                .pending
                .as_ref()
                .is_some_and(|pending| pending.kind == OriginRequestKind::Pause)
        {
            return Err(FileNetworkOriginError::InvalidState);
        }
        if transfer.retry_at_unix_ms.is_some()
            && transfer
                .pending
                .as_ref()
                .is_some_and(|pending| pending.retry_frame.is_some())
        {
            // A rate-limit error already released the request correlation even
            // though the exact upload chunk is retained for retry. Pause now by
            // replacing that retry with a control request; the owner discards
            // its complete chunk assembly and the requester rewinds the chunk
            // when the pause response arrives.
            transfer.retry_at_unix_ms = None;
            transfer.pending = None;
            queue_pause(transfer, &mut self.outbound)?;
            self.outbound_notify.notify_one();
            return Ok(());
        }
        if transfer.binding.is_none() {
            if !transfer.pending.as_ref().is_some_and(|pending| {
                matches!(
                    pending.kind,
                    OriginRequestKind::OpenUpload | OriginRequestKind::OpenDownload
                )
            }) {
                return Err(FileNetworkOriginError::InvalidState);
            }
            transfer.pause_after_reply = true;
            return Ok(());
        }
        let active = match &transfer.state {
            OriginTransferState::Upload(_) => transfer
                .pending
                .as_ref()
                .is_none_or(|pending| pending.kind != OriginRequestKind::FinishUpload),
            OriginTransferState::Download(download) => {
                download
                    .session
                    .as_ref()
                    .is_some_and(|session| session.state == TransferState::Active)
                    && transfer
                        .pending
                        .as_ref()
                        .is_none_or(|pending| pending.kind != OriginRequestKind::FinishDownload)
            }
        };
        if !active {
            return Err(FileNetworkOriginError::InvalidState);
        }
        if transfer.pending.is_some() {
            transfer.pause_after_reply = true;
            return Ok(());
        }
        queue_pause(transfer, &mut self.outbound)?;
        self.outbound_notify.notify_one();
        Ok(())
    }

    /// Return the peer and bound access epoch that must be reauthorized before
    /// resuming a paused operation. The live member directory is intentionally
    /// not consulted here; the runtime owns that check and passes the result to
    /// [`Self::resume_authorized`].
    pub(crate) fn resume_scope(
        &self,
        transfer_id: &TransferId,
        operation_id: &OperationId,
    ) -> Result<(OriginPeer, u64), FileNetworkOriginError> {
        let key = OriginTransferKey::new(transfer_id, operation_id);
        let transfer = self
            .transfers
            .get(&key)
            .ok_or(FileNetworkOriginError::NotFound)?;
        if !transfer.paused || transfer.pending.is_some() || transfer.pause_after_reply {
            return Err(FileNetworkOriginError::InvalidState);
        }
        let binding = transfer
            .binding
            .as_ref()
            .ok_or(FileNetworkOriginError::InvalidState)?;
        Ok((transfer.peer.clone(), binding.access_epoch))
    }

    /// Continue a paused operation only if the caller's freshly resolved member
    /// access epoch still matches the binding returned by the owner. A revoked
    /// epoch fails the local operation closed and removes any paused journal or
    /// download staging before a resume frame can be queued.
    pub(crate) fn resume_authorized(
        &mut self,
        transfer_id: &TransferId,
        operation_id: &OperationId,
        current_access_epoch: u64,
        now_unix_ms: u64,
    ) -> Result<(), FileNetworkOriginError> {
        let key = OriginTransferKey::new(transfer_id, operation_id);
        let Some(transfer) = self.transfers.get(&key) else {
            return Err(FileNetworkOriginError::NotFound);
        };
        if now_unix_ms >= transfer.deadline_unix_ms {
            self.fail_transfer_at(&key, now_unix_ms, "file_transfer_expired", false);
            self.outbound_notify.notify_one();
            return Err(FileNetworkOriginError::Transfer(FileTransferError::Expired));
        }
        if !transfer.paused || transfer.pending.is_some() || transfer.pause_after_reply {
            return Err(FileNetworkOriginError::InvalidState);
        }
        let binding = transfer
            .binding
            .as_ref()
            .ok_or(FileNetworkOriginError::InvalidState)?;
        if current_access_epoch == 0 || current_access_epoch != binding.access_epoch {
            self.fail_transfer_at(&key, now_unix_ms, "file_transfer_access_revoked", false);
            self.outbound_notify.notify_one();
            return Err(FileNetworkOriginError::AccessRevoked);
        }
        let transfer = self
            .transfers
            .get_mut(&key)
            .ok_or(FileNetworkOriginError::NotFound)?;
        queue_resume(transfer, &mut self.outbound)?;
        self.outbound_notify.notify_one();
        Ok(())
    }

    /// Continue a paused operation using the exact binding returned when the
    /// owner first opened it. Runtime callers must use
    /// [`Self::resume_authorized`] after resolving the live member epoch.
    #[cfg(test)]
    pub(crate) fn resume(
        &mut self,
        transfer_id: &TransferId,
        operation_id: &OperationId,
        now_unix_ms: u64,
    ) -> Result<(), FileNetworkOriginError> {
        let key = OriginTransferKey::new(transfer_id, operation_id);
        let Some(transfer) = self.transfers.get(&key) else {
            return Err(FileNetworkOriginError::NotFound);
        };
        let access_epoch = transfer
            .binding
            .as_ref()
            .ok_or(FileNetworkOriginError::InvalidState)?
            .access_epoch;
        self.resume_authorized(transfer_id, operation_id, access_epoch, now_unix_ms)
    }

    /// Expire timed-out operations and remove any partial staging file.
    pub(crate) fn expire(&mut self, now_unix_ms: u64) {
        let expired = self
            .transfers
            .iter()
            .filter(|(_, transfer)| now_unix_ms >= transfer.deadline_unix_ms)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        let had_expired = !expired.is_empty();
        for key in expired {
            self.fail_transfer_at(&key, now_unix_ms, "file_transfer_expired", true);
        }
        if had_expired {
            self.outbound_notify.notify_one();
        }
    }

    fn ensure_capacity(
        &self,
        transfer_id: &TransferId,
        operation_id: &OperationId,
    ) -> Result<(), FileNetworkOriginError> {
        if self
            .transfers
            .contains_key(&OriginTransferKey::new(transfer_id, operation_id))
        {
            return Err(FileNetworkOriginError::DuplicateTransfer);
        }
        if self.transfers.len() >= MAX_ORIGIN_TRANSFERS {
            return Err(FileNetworkOriginError::TransferLimit);
        }
        Ok(())
    }

    fn push_event(&mut self, event: OriginTransferEvent) {
        if self.events.len() >= MAX_ORIGIN_EVENTS {
            let _ = self.events.pop_front();
        }
        self.events.push_back(event);
    }

    fn queue_deferred_pause(
        &mut self,
        key: &OriginTransferKey,
    ) -> Result<(), FileNetworkOriginError> {
        let Some(transfer) = self.transfers.get_mut(key) else {
            return Ok(());
        };
        if !transfer.pause_after_reply || transfer.pending.is_some() {
            return Ok(());
        }
        transfer.pause_after_reply = false;
        queue_pause(transfer, &mut self.outbound)
    }

    fn validate_response(
        &self,
        transfer: &OriginTransfer,
        kind: OriginRequestKind,
        response: &FileNetworkResponse,
    ) -> Result<(), FileNetworkOriginError> {
        match (kind, response) {
            (OriginRequestKind::OpenUpload, FileNetworkResponse::Opened { session }) => {
                validate_opened_upload(transfer, session, &self.local_node_id)
            }
            (OriginRequestKind::OpenDownload, FileNetworkResponse::Opened { session }) => {
                validate_opened_download(transfer, session, &self.local_node_id)
            }
            (OriginRequestKind::ResumeUpload, FileNetworkResponse::Opened { session }) => {
                validate_opened_upload(transfer, session, &self.local_node_id)
            }
            (OriginRequestKind::ResumeDownload, FileNetworkResponse::Opened { session }) => {
                validate_opened_download(transfer, session, &self.local_node_id)
            }
            (
                OriginRequestKind::UploadChunk {
                    chunk_index,
                    offset,
                    length,
                },
                FileNetworkResponse::ChunkAccepted {
                    chunk_index: acknowledged_chunk,
                    chunk_complete,
                    progress,
                },
            ) => validate_upload_ack(
                transfer,
                chunk_index,
                offset,
                length,
                *acknowledged_chunk,
                *chunk_complete,
                progress,
            ),
            (OriginRequestKind::FinishUpload, FileNetworkResponse::Completed { progress }) => {
                validate_finish(transfer, progress)
            }
            (OriginRequestKind::FinishDownload, FileNetworkResponse::Completed { progress }) => {
                validate_finish(transfer, progress)
            }
            (OriginRequestKind::Pause, FileNetworkResponse::Progress { progress }) => {
                validate_pause(transfer, progress)
            }
            _ => Err(FileNetworkOriginError::UnexpectedResponse),
        }
    }

    fn apply_open_upload(
        &mut self,
        key: &OriginTransferKey,
        session: TransferSession,
        _now_unix_ms: u64,
    ) -> Result<(), FileNetworkOriginError> {
        let event = {
            let transfer = self
                .transfers
                .get_mut(key)
                .ok_or(FileNetworkOriginError::UnknownResponse)?;
            if !matches!(transfer.state, OriginTransferState::Upload(_)) {
                return Err(FileNetworkOriginError::UnexpectedResponse);
            }
            transfer.binding = Some(session.binding.clone());
            transfer.deadline_unix_ms = transfer
                .deadline_unix_ms
                .min(session.binding.expires_at_unix_ms);
            if !transfer.pause_after_reply {
                queue_next_upload_action(transfer, &mut self.outbound)?;
            }
            transfer.event(TransferState::Active, None)
        };
        self.push_event(event);
        Ok(())
    }

    fn apply_open_download(
        &mut self,
        key: &OriginTransferKey,
        session: TransferSession,
        now_unix_ms: u64,
    ) -> Result<(), FileNetworkOriginError> {
        let transfer = self
            .transfers
            .get_mut(key)
            .ok_or(FileNetworkOriginError::UnknownResponse)?;
        let destination = match &transfer.state {
            OriginTransferState::Download(download) => download.destination.clone(),
            _ => return Err(FileNetworkOriginError::UnexpectedResponse),
        };
        let parent = destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        if !parent.is_dir() {
            return Err(FileNetworkOriginError::InvalidDestination);
        }
        if std::fs::symlink_metadata(&destination).is_ok() {
            return Err(FileNetworkOriginError::InvalidDestination);
        }
        let staging = DownloadStaging::create(parent)?;
        let event = match &mut transfer.state {
            OriginTransferState::Download(download) => {
                transfer.binding = Some(session.binding.clone());
                transfer.deadline_unix_ms = transfer
                    .deadline_unix_ms
                    .min(session.binding.expires_at_unix_ms);
                download.session = Some(Box::new(session));
                download.staging = Some(staging);
                transfer.event(TransferState::Active, None)
            }
            _ => return Err(FileNetworkOriginError::UnexpectedResponse),
        };
        self.push_event(event);
        if self
            .transfers
            .get(key)
            .is_some_and(|transfer| transfer.pause_after_reply)
        {
            return Ok(());
        }
        let finished = self
            .transfers
            .get_mut(key)
            .is_some_and(|transfer| transfer.download_finished_receiving());
        if finished {
            self.finish_download_receiving(key, now_unix_ms)?;
        }
        Ok(())
    }

    fn apply_resume_upload(
        &mut self,
        key: &OriginTransferKey,
        session: TransferSession,
        now_unix_ms: u64,
    ) -> Result<(), FileNetworkOriginError> {
        let event = {
            let transfer = self
                .transfers
                .get_mut(key)
                .ok_or(FileNetworkOriginError::UnknownResponse)?;
            if !matches!(transfer.state, OriginTransferState::Upload(_)) {
                return Err(FileNetworkOriginError::UnexpectedResponse);
            }
            transfer.binding = Some(session.binding.clone());
            transfer.deadline_unix_ms = transfer
                .deadline_unix_ms
                .min(session.binding.expires_at_unix_ms);
            transfer.paused = false;
            queue_next_upload_action(transfer, &mut self.outbound)?;
            transfer.event(TransferState::Active, None)
        };
        self.push_event(event);
        self.save_paused(now_unix_ms)?;
        Ok(())
    }

    fn apply_resume_download(
        &mut self,
        key: &OriginTransferKey,
        session: TransferSession,
        now_unix_ms: u64,
    ) -> Result<(), FileNetworkOriginError> {
        let event = {
            let transfer = self
                .transfers
                .get_mut(key)
                .ok_or(FileNetworkOriginError::UnknownResponse)?;
            let OriginTransferState::Download(download) = &mut transfer.state else {
                return Err(FileNetworkOriginError::UnexpectedResponse);
            };
            if download.staging.is_none() {
                return Err(FileNetworkOriginError::BindingMismatch);
            }
            if session.received_chunk_count() != download.received_chunks as usize
                || (0..download.received_chunks).any(|index| !session.has_received_chunk(index))
            {
                return Err(FileNetworkOriginError::BindingMismatch);
            }
            transfer.binding = Some(session.binding.clone());
            transfer.deadline_unix_ms = transfer
                .deadline_unix_ms
                .min(session.binding.expires_at_unix_ms);
            transfer.paused = false;
            download.session = Some(Box::new(session));
            if let Some(staging) = download.staging.as_mut() {
                staging.prepare_for_resume(download.received_bytes)?;
                staging.mark_transient();
            }
            transfer.event(TransferState::Active, None)
        };
        self.push_event(event);
        let finished = self
            .transfers
            .get_mut(key)
            .is_some_and(|transfer| transfer.download_finished_receiving());
        if finished {
            self.finish_download_receiving(key, now_unix_ms)?;
        }
        self.save_paused(now_unix_ms)?;
        Ok(())
    }

    fn apply_pause(
        &mut self,
        key: &OriginTransferKey,
        progress: FileNetworkProgress,
        now_unix_ms: u64,
    ) -> Result<(), FileNetworkOriginError> {
        let event = {
            let transfer = self
                .transfers
                .get_mut(key)
                .ok_or(FileNetworkOriginError::UnknownResponse)?;
            validate_pause(transfer, &progress)?;
            match &mut transfer.state {
                OriginTransferState::Upload(upload) => {
                    // The owner discards a partially assembled upload chunk on pause.
                    // Rewind that chunk so resume starts at an offset the owner accepts.
                    upload.next_offset = 0;
                }
                OriginTransferState::Download(download) => {
                    if let Some(session) = download.session.as_mut() {
                        session.pause(now_unix_ms)?;
                    }
                }
            }
            transfer.paused = true;
            transfer.pause_after_reply = false;
            transfer.event(TransferState::Paused, None)
        };
        self.push_event(event);
        self.save_paused(now_unix_ms)?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_upload_ack(
        &mut self,
        key: &OriginTransferKey,
        chunk_index: u32,
        _offset: u64,
        _length: u64,
        acknowledged_chunk: u32,
        chunk_complete: bool,
        _progress: FileNetworkProgress,
        _now_unix_ms: u64,
    ) -> Result<(), FileNetworkOriginError> {
        if acknowledged_chunk != chunk_index {
            return Err(FileNetworkOriginError::UnexpectedResponse);
        }
        let event = {
            let transfer = self
                .transfers
                .get_mut(key)
                .ok_or(FileNetworkOriginError::UnknownResponse)?;
            {
                let OriginTransferState::Upload(upload) = &mut transfer.state else {
                    return Err(FileNetworkOriginError::UnexpectedResponse);
                };
                upload
                    .manifest
                    .chunks
                    .get(chunk_index as usize)
                    .ok_or(FileNetworkOriginError::UnexpectedResponse)?;
                if chunk_complete {
                    upload.next_chunk = upload.next_chunk.saturating_add(1);
                    upload.next_offset = 0;
                    upload.completed_chunks = upload.completed_chunks.saturating_add(1);
                }
            }
            if !transfer.pause_after_reply {
                queue_next_upload_action(transfer, &mut self.outbound)?;
            }
            transfer.event(TransferState::Active, None)
        };
        self.push_event(event);
        Ok(())
    }

    fn apply_finish(
        &mut self,
        key: &OriginTransferKey,
        progress: FileNetworkProgress,
        now_unix_ms: u64,
    ) -> Result<(), FileNetworkOriginError> {
        let transfer = self
            .transfers
            .get(key)
            .ok_or(FileNetworkOriginError::UnknownResponse)?;
        validate_finish(transfer, &progress)?;
        if let OriginTransferState::Download(download) = &transfer.state {
            let destination = download.destination.clone();
            download
                .staging
                .as_ref()
                .ok_or(FileNetworkOriginError::BindingMismatch)?;
            // `persist_noclobber` consumes the staging handle. Reconstruct it
            // only long enough to preserve cleanup-on-error semantics.
            let transfer = self
                .transfers
                .get_mut(key)
                .ok_or(FileNetworkOriginError::UnknownResponse)?;
            let OriginTransferState::Download(download) = &mut transfer.state else {
                return Err(FileNetworkOriginError::UnexpectedResponse);
            };
            let staging = download
                .staging
                .take()
                .ok_or(FileNetworkOriginError::BindingMismatch)?;
            if let Err(error) = staging.persist_noclobber(&destination) {
                download.staging = Some(error.file);
                return Err(FileNetworkOriginError::Io(error.error));
            }
            let session = download
                .session
                .as_mut()
                .ok_or(FileNetworkOriginError::BindingMismatch)?;
            session.complete_verified(now_unix_ms)?;
        }
        let transfer = self
            .transfers
            .remove(key)
            .ok_or(FileNetworkOriginError::UnknownResponse)?;
        self.push_event(transfer.event(TransferState::Completed, None));
        self.save_paused(now_unix_ms)?;
        Ok(())
    }

    fn finish_download_receiving(
        &mut self,
        key: &OriginTransferKey,
        now_unix_ms: u64,
    ) -> Result<(), FileNetworkOriginError> {
        let transfer = self
            .transfers
            .get_mut(key)
            .ok_or(FileNetworkOriginError::UnknownResponse)?;
        match &mut transfer.state {
            OriginTransferState::Download(download) => {
                let session = download
                    .session
                    .as_mut()
                    .ok_or(FileNetworkOriginError::BindingMismatch)?;
                let staging = download
                    .staging
                    .as_mut()
                    .ok_or(FileNetworkOriginError::BindingMismatch)?;
                staging.flush()?;
                staging.as_file().sync_all()?;
                let digest =
                    ContentHash::from_digest_bytes(*download.hasher.clone().finalize().as_bytes());
                session.verify_digest(download.received_bytes, &digest, now_unix_ms)?;
                let request_id = next_origin_request_id(transfer, b"finish-download", 0, 0, 0)?;
                let request = FileNetworkRequest::Finish {
                    transfer_id: transfer.transfer_id.clone(),
                    operation_id: transfer.operation_id.clone(),
                };
                let frame = FileNetworkFrame::request(request_id, request)
                    .map_err(|_| FileNetworkOriginError::InvalidRequest)?;
                transfer.pending = Some(PendingOriginReply::new(
                    request_id,
                    OriginRequestKind::FinishDownload,
                ));
                self.outbound.push_back(OriginOutbound {
                    peer: transfer.peer.clone(),
                    transfer_id: transfer.transfer_id.clone(),
                    operation_id: transfer.operation_id.clone(),
                    actor_access_epoch: outbound_access_epoch(transfer),
                    frame,
                });
                Ok(())
            }
            _ => Err(FileNetworkOriginError::UnexpectedResponse),
        }
    }

    fn fail_transfer_at(
        &mut self,
        key: &OriginTransferKey,
        now_unix_ms: u64,
        code: &str,
        notify_remote: bool,
    ) -> bool {
        let Some(mut transfer) = self.transfers.remove(key) else {
            return false;
        };
        if let OriginTransferState::Download(download) = &mut transfer.state {
            if let Some(staging) = download.staging.as_mut() {
                // A terminal transfer must not leave a durable staging file
                // behind even if it had previously been paused.
                staging.mark_transient();
            }
        }
        if notify_remote {
            queue_cancel(&mut transfer, &mut self.outbound);
        }
        let state = if code == "file_transfer_expired" {
            TransferState::Expired
        } else if code == "file_transfer_cancelled" {
            TransferState::Cancelled
        } else {
            TransferState::Failed
        };
        self.push_event(transfer.event(state, Some(code.to_owned())));
        let _ = self.save_paused(now_unix_ms);
        true
    }
}

fn persisted_transfer(
    transfer: &mut OriginTransfer,
) -> Result<PersistedOriginTransfer, FileNetworkOriginError> {
    let binding = transfer
        .binding
        .clone()
        .ok_or(FileNetworkOriginError::InvalidState)?;
    if binding.transfer_id != transfer.transfer_id
        || binding.operation_id != transfer.operation_id
        || binding.share_id != transfer.share_id
        || binding.path != transfer.path
        || binding.direction != transfer.direction
        || binding.access_epoch == 0
        || binding.share_revision == 0
        || binding.expires_at_unix_ms < transfer.deadline_unix_ms
    {
        return Err(FileNetworkOriginError::BindingMismatch);
    }
    let (manifest, upload, download) = match (&mut transfer.state, transfer.direction) {
        (OriginTransferState::Upload(upload_state), TransferDirection::Upload) => (
            upload_state.manifest.clone(),
            Some(PersistedUploadState {
                source_path: upload_state.source_path.clone(),
                next_chunk: upload_state.next_chunk,
                next_offset: upload_state.next_offset,
                completed_chunks: upload_state.completed_chunks,
            }),
            None,
        ),
        (OriginTransferState::Download(download_state), TransferDirection::Download) => (
            download_state
                .session
                .as_ref()
                .ok_or(FileNetworkOriginError::BindingMismatch)?
                .manifest
                .clone(),
            None,
            Some(persisted_download_state(download_state)?),
        ),
        _ => return Err(FileNetworkOriginError::InvalidState),
    };
    manifest
        .validate()
        .map_err(|_| FileNetworkOriginError::InvalidRequest)?;
    validate_network_manifest(&manifest).map_err(|_| FileNetworkOriginError::InvalidRequest)?;
    Ok(PersistedOriginTransfer {
        peer: transfer.peer.clone().into(),
        share_id: transfer.share_id.clone(),
        transfer_id: transfer.transfer_id.clone(),
        operation_id: transfer.operation_id.clone(),
        path: transfer.path.clone(),
        direction: transfer.direction,
        deadline_unix_ms: transfer.deadline_unix_ms,
        binding,
        manifest,
        next_request_sequence: transfer.next_request_sequence,
        resume_generation: transfer.resume_generation,
        upload,
        download,
    })
}

fn persisted_download_state(
    download: &mut DownloadOriginState,
) -> Result<PersistedDownloadState, FileNetworkOriginError> {
    let session = download
        .session
        .as_ref()
        .ok_or(FileNetworkOriginError::BindingMismatch)?;
    if session.state != TransferState::Paused
        || session.received_chunk_count() != download.received_chunks as usize
        || (0..download.received_chunks).any(|index| !session.has_received_chunk(index))
    {
        return Err(FileNetworkOriginError::InvalidState);
    }
    let expected_received_bytes =
        received_bytes_for_chunks(&session.manifest, download.received_chunks)?;
    if expected_received_bytes != download.received_bytes {
        return Err(FileNetworkOriginError::InvalidState);
    }
    let staging = download
        .staging
        .as_mut()
        .ok_or(FileNetworkOriginError::BindingMismatch)?;
    staging.flush()?;
    staging.as_file().set_len(download.received_bytes)?;
    staging
        .as_file()
        .seek(SeekFrom::Start(download.received_bytes))?;
    let assembly = match download.assembly.as_ref() {
        Some(assembly) => {
            let expected_chunk = session
                .manifest
                .chunks
                .get(download.received_chunks as usize)
                .filter(|chunk| chunk.index == download.received_chunks)
                .ok_or(FileNetworkOriginError::ChunkOutOfOrder)?;
            let persisted_length = u64::try_from(assembly.bytes.len())
                .map_err(|_| FileNetworkOriginError::ChunkCorrupt)?;
            if assembly.chunk_index != download.received_chunks
                || assembly.expected_offset != expected_chunk.offset
                || assembly.expected_length != expected_chunk.length
                || assembly.expected_checksum != expected_chunk.checksum
                || persisted_length == 0
                || persisted_length >= assembly.expected_length
            {
                return Err(FileNetworkOriginError::ChunkCorrupt);
            }
            staging.write_all(&assembly.bytes)?;
            Some(PersistedDownloadAssembly {
                chunk_index: assembly.chunk_index,
                expected_offset: assembly.expected_offset,
                expected_length: assembly.expected_length,
                expected_checksum: assembly.expected_checksum.clone(),
                persisted_length,
            })
        }
        None => None,
    };
    staging.flush()?;
    staging.as_file().sync_all()?;
    Ok(PersistedDownloadState {
        destination: download.destination.clone(),
        staging_path: staging.path().to_path_buf(),
        session: (**session).clone(),
        assembly,
        received_bytes: download.received_bytes,
        received_chunks: download.received_chunks,
    })
}

fn restore_paused_transfer(
    entry: &PersistedOriginTransfer,
    local_node_id: &NodeId,
    now_unix_ms: u64,
) -> Result<OriginTransfer, FileNetworkOriginError> {
    entry
        .manifest
        .validate()
        .map_err(|_| FileNetworkOriginError::InvalidRequest)?;
    validate_network_manifest(&entry.manifest)
        .map_err(|_| FileNetworkOriginError::InvalidRequest)?;
    let binding = &entry.binding;
    if binding.actor != *local_node_id
        || binding.transfer_id != entry.transfer_id
        || binding.operation_id != entry.operation_id
        || binding.share_id != entry.share_id
        || binding.path != entry.path
        || binding.direction != entry.direction
        || binding.access_epoch == 0
        || binding.share_revision == 0
        || entry.next_request_sequence == 0
    {
        return Err(FileNetworkOriginError::BindingMismatch);
    }
    if entry.deadline_unix_ms > binding.expires_at_unix_ms {
        return Err(FileNetworkOriginError::BindingMismatch);
    }
    if now_unix_ms >= entry.deadline_unix_ms || now_unix_ms >= binding.expires_at_unix_ms {
        return Err(FileNetworkOriginError::Transfer(FileTransferError::Expired));
    }
    match (entry.direction, &entry.upload, &entry.download) {
        (TransferDirection::Upload, Some(upload), None) => {
            if binding.file_version.is_some() {
                return Err(FileNetworkOriginError::BindingMismatch);
            }
            restore_upload(entry, upload)
        }
        (TransferDirection::Download, None, Some(download)) => restore_download(entry, download),
        _ => Err(FileNetworkOriginError::InvalidRequest),
    }
}

fn restore_upload(
    entry: &PersistedOriginTransfer,
    upload: &PersistedUploadState,
) -> Result<OriginTransfer, FileNetworkOriginError> {
    let (mut source, manifest) =
        build_upload_manifest(&entry.path, &upload.source_path, entry.manifest.chunk_size)?;
    if manifest != entry.manifest {
        return Err(FileNetworkOriginError::InvalidSource);
    }
    let total_chunks =
        u32::try_from(manifest.chunks.len()).map_err(|_| FileNetworkOriginError::InvalidRequest)?;
    if upload.next_chunk > total_chunks || upload.completed_chunks != upload.next_chunk {
        return Err(FileNetworkOriginError::InvalidState);
    }
    if upload.next_chunk < total_chunks {
        let chunk = &manifest.chunks[upload.next_chunk as usize];
        if upload.next_offset != 0 || upload.next_offset >= chunk.length {
            return Err(FileNetworkOriginError::InvalidState);
        }
        source.seek(SeekFrom::Start(chunk.offset))?;
    } else if upload.next_offset != 0 {
        return Err(FileNetworkOriginError::InvalidState);
    }
    Ok(OriginTransfer {
        peer: entry.peer.clone().into(),
        share_id: entry.share_id.clone(),
        transfer_id: entry.transfer_id.clone(),
        operation_id: entry.operation_id.clone(),
        path: entry.path.clone(),
        direction: TransferDirection::Upload,
        deadline_unix_ms: entry.deadline_unix_ms,
        pending: None,
        retry_at_unix_ms: None,
        binding: Some(entry.binding.clone()),
        paused: true,
        pause_after_reply: false,
        next_request_sequence: entry.next_request_sequence,
        resume_generation: entry.resume_generation,
        state: OriginTransferState::Upload(UploadOriginState {
            source,
            source_path: upload.source_path.clone(),
            manifest,
            next_chunk: upload.next_chunk,
            next_offset: upload.next_offset,
            completed_chunks: upload.completed_chunks,
        }),
    })
}

fn restore_download(
    entry: &PersistedOriginTransfer,
    download: &PersistedDownloadState,
) -> Result<OriginTransfer, FileNetworkOriginError> {
    let session = download.session.clone();
    if session.binding != entry.binding
        || session.manifest != entry.manifest
        || session.state != TransferState::Paused
        || session.received_chunk_count() != download.received_chunks as usize
        || (0..download.received_chunks).any(|index| !session.has_received_chunk(index))
        || entry.binding.file_version.as_ref().is_none_or(|version| {
            version.content_hash != entry.manifest.content_hash
                || version.size != entry.manifest.total_bytes
        })
    {
        return Err(FileNetworkOriginError::BindingMismatch);
    }
    if download.received_chunks as usize > entry.manifest.chunks.len() {
        return Err(FileNetworkOriginError::ChunkOutOfOrder);
    }
    let expected_received_bytes =
        received_bytes_for_chunks(&entry.manifest, download.received_chunks)?;
    if expected_received_bytes != download.received_bytes {
        return Err(FileNetworkOriginError::InvalidState);
    }

    validate_download_destination(&download.destination)?;
    let destination_parent = download
        .destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let staging_parent = download
        .staging_path
        .parent()
        .ok_or(FileNetworkOriginError::InvalidDestination)?;
    let staging_name = download
        .staging_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(FileNetworkOriginError::InvalidDestination)?;
    if staging_parent != destination_parent || !staging_name.starts_with(".fkfile-download-") {
        return Err(FileNetworkOriginError::InvalidDestination);
    }

    let assembly_metadata = download
        .assembly
        .as_ref()
        .map(|assembly| {
            let chunk = entry
                .manifest
                .chunks
                .get(download.received_chunks as usize)
                .filter(|chunk| chunk.index == download.received_chunks)
                .ok_or(FileNetworkOriginError::ChunkOutOfOrder)?;
            if assembly.chunk_index != download.received_chunks
                || assembly.expected_offset != chunk.offset
                || assembly.expected_length != chunk.length
                || assembly.expected_checksum != chunk.checksum
                || assembly.persisted_length == 0
                || assembly.persisted_length >= assembly.expected_length
            {
                return Err(FileNetworkOriginError::ChunkCorrupt);
            }
            Ok(assembly.clone())
        })
        .transpose()?;
    let persisted_length = assembly_metadata
        .as_ref()
        .map_or(0, |assembly| assembly.persisted_length);
    let expected_file_length = download
        .received_bytes
        .checked_add(persisted_length)
        .ok_or(FileNetworkOriginError::ChunkCorrupt)?;

    let mut staging = DownloadStaging::open_existing(&download.staging_path)?;
    if staging.as_file().metadata()?.len() != expected_file_length {
        return Err(FileNetworkOriginError::ChunkCorrupt);
    }
    let mut hasher = blake3::Hasher::new();
    for chunk in entry
        .manifest
        .chunks
        .iter()
        .take(download.received_chunks as usize)
    {
        let mut bytes = vec![
            0_u8;
            usize::try_from(chunk.length)
                .map_err(|_| FileNetworkOriginError::ChunkCorrupt)?
        ];
        staging.file_mut().read_exact(&mut bytes)?;
        if ContentHash::from_bytes(&bytes) != chunk.checksum {
            return Err(FileNetworkOriginError::ChunkCorrupt);
        }
        hasher.update(&bytes);
    }
    let assembly_bytes = if let Some(metadata) = &assembly_metadata {
        let mut bytes = vec![
            0_u8;
            usize::try_from(metadata.persisted_length)
                .map_err(|_| FileNetworkOriginError::ChunkCorrupt)?
        ];
        staging.file_mut().read_exact(&mut bytes)?;
        bytes
    } else {
        Vec::new()
    };
    staging.as_file().set_len(download.received_bytes)?;
    staging
        .as_file()
        .seek(SeekFrom::Start(download.received_bytes))?;
    staging.mark_durable();

    Ok(OriginTransfer {
        peer: entry.peer.clone().into(),
        share_id: entry.share_id.clone(),
        transfer_id: entry.transfer_id.clone(),
        operation_id: entry.operation_id.clone(),
        path: entry.path.clone(),
        direction: TransferDirection::Download,
        deadline_unix_ms: entry.deadline_unix_ms,
        pending: None,
        retry_at_unix_ms: None,
        binding: Some(entry.binding.clone()),
        paused: true,
        pause_after_reply: false,
        next_request_sequence: entry.next_request_sequence,
        resume_generation: entry.resume_generation,
        state: OriginTransferState::Download(Box::new(DownloadOriginState {
            destination: download.destination.clone(),
            session: Some(Box::new(session)),
            staging: Some(staging),
            assembly: assembly_metadata.map(|metadata| DownloadAssembly {
                chunk_index: metadata.chunk_index,
                expected_offset: metadata.expected_offset,
                expected_length: metadata.expected_length,
                expected_checksum: metadata.expected_checksum,
                bytes: assembly_bytes,
            }),
            hasher,
            received_bytes: download.received_bytes,
            received_chunks: download.received_chunks,
        })),
    })
}

fn received_bytes_for_chunks(
    manifest: &TransferManifest,
    received_chunks: u32,
) -> Result<u64, FileNetworkOriginError> {
    if received_chunks as usize > manifest.chunks.len() {
        return Err(FileNetworkOriginError::ChunkOutOfOrder);
    }
    manifest
        .chunks
        .iter()
        .take(received_chunks as usize)
        .try_fold(0_u64, |total, chunk| total.checked_add(chunk.length))
        .ok_or(FileNetworkOriginError::ChunkCorrupt)
}

fn restore_failure_event(
    entry: &PersistedOriginTransfer,
    state: TransferState,
    code: &str,
) -> OriginTransferEvent {
    let (transferred_bytes, received_chunks) = match entry.direction {
        TransferDirection::Upload => entry.upload.as_ref().map_or((0, 0), |upload| {
            let transferred = entry
                .manifest
                .chunks
                .get(upload.next_chunk as usize)
                .map_or(entry.manifest.total_bytes, |chunk| chunk.offset);
            (transferred, upload.completed_chunks)
        }),
        TransferDirection::Download => entry.download.as_ref().map_or((0, 0), |download| {
            (download.received_bytes, download.received_chunks)
        }),
    };
    OriginTransferEvent {
        transfer_id: entry.transfer_id.clone(),
        operation_id: entry.operation_id.clone(),
        direction: entry.direction,
        state,
        transferred_bytes,
        total_bytes: entry.manifest.total_bytes,
        received_chunks,
        total_chunks: u32::try_from(entry.manifest.chunks.len()).unwrap_or(u32::MAX),
        error_code: Some(code.to_owned()),
    }
}

fn cleanup_restore_staging(entry: &PersistedOriginTransfer) {
    let Some(download) = entry.download.as_ref() else {
        return;
    };
    let destination_parent = download
        .destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if download.staging_path.parent() != Some(destination_parent) {
        return;
    }
    let Some(name) = download
        .staging_path
        .file_name()
        .and_then(|name| name.to_str())
    else {
        return;
    };
    if !name.starts_with(".fkfile-download-") {
        return;
    }
    if std::fs::symlink_metadata(&download.staging_path)
        .is_ok_and(|metadata| metadata.file_type().is_file() || metadata.file_type().is_symlink())
    {
        let _ = std::fs::remove_file(&download.staging_path);
    }
}

impl OriginTransfer {
    fn event(&self, state: TransferState, error_code: Option<String>) -> OriginTransferEvent {
        let (transferred_bytes, total_bytes, received_chunks, total_chunks) = match &self.state {
            OriginTransferState::Upload(upload) => {
                let chunk = upload.manifest.chunks.get(upload.next_chunk as usize);
                let transferred_bytes = chunk
                    .map(|chunk| chunk.offset.saturating_add(upload.next_offset))
                    .unwrap_or(upload.manifest.total_bytes);
                (
                    transferred_bytes,
                    upload.manifest.total_bytes,
                    upload.completed_chunks,
                    u32::try_from(upload.manifest.chunks.len()).unwrap_or(u32::MAX),
                )
            }
            OriginTransferState::Download(download) => (
                download.received_bytes,
                download
                    .session
                    .as_ref()
                    .map_or(0, |session| session.manifest.total_bytes),
                download.received_chunks,
                download
                    .session
                    .as_ref()
                    .map(|session| u32::try_from(session.manifest.chunks.len()).unwrap_or(u32::MAX))
                    .unwrap_or_default(),
            ),
        };
        OriginTransferEvent {
            transfer_id: self.transfer_id.clone(),
            operation_id: self.operation_id.clone(),
            direction: self.direction,
            state,
            transferred_bytes,
            total_bytes,
            received_chunks,
            total_chunks,
            error_code,
        }
    }

    fn download_finished_receiving(&self) -> bool {
        match &self.state {
            OriginTransferState::Download(download) => {
                download.session.as_ref().is_some_and(|session| {
                    download.received_chunks
                        == u32::try_from(session.manifest.chunks.len()).unwrap_or(u32::MAX)
                }) && download.assembly.is_none()
                    && download.staging.is_some()
            }
            _ => false,
        }
    }

    fn download_session(&self) -> Option<&TransferSession> {
        match &self.state {
            OriginTransferState::Download(download) => download.session.as_deref(),
            _ => None,
        }
    }
}

fn transfer_total_bytes(transfer: &OriginTransfer) -> u64 {
    match &transfer.state {
        OriginTransferState::Upload(upload) => upload.manifest.total_bytes,
        OriginTransferState::Download(download) => download
            .session
            .as_ref()
            .map_or(0, |session| session.manifest.total_bytes),
    }
}

fn transfer_total_chunks(transfer: &OriginTransfer) -> u32 {
    match &transfer.state {
        OriginTransferState::Upload(upload) => {
            u32::try_from(upload.manifest.chunks.len()).unwrap_or(u32::MAX)
        }
        OriginTransferState::Download(download) => download
            .session
            .as_ref()
            .map(|session| u32::try_from(session.manifest.chunks.len()).unwrap_or(u32::MAX))
            .unwrap_or_default(),
    }
}

fn outbound_access_epoch(transfer: &OriginTransfer) -> Option<u64> {
    transfer
        .binding
        .as_ref()
        .map(|binding| binding.access_epoch)
}

fn queue_open_upload(
    transfer: &mut OriginTransfer,
    outbound: &mut VecDeque<OriginOutbound>,
    share_id: FileShareId,
    path: SharedPath,
    _now_unix_ms: u64,
) -> Result<(), FileNetworkOriginError> {
    let manifest = match &transfer.state {
        OriginTransferState::Upload(upload) => upload.manifest.clone(),
        _ => return Err(FileNetworkOriginError::InvalidRequest),
    };
    let request_id = next_origin_request_id(transfer, b"open-upload", 0, 0, 0)?;
    let request = FileNetworkRequest::OpenUpload {
        share_id,
        transfer_id: transfer.transfer_id.clone(),
        operation_id: transfer.operation_id.clone(),
        path,
        manifest,
    };
    let frame = FileNetworkFrame::request(request_id, request)
        .map_err(|_| FileNetworkOriginError::InvalidRequest)?;
    transfer.pending = Some(PendingOriginReply::new(
        request_id,
        OriginRequestKind::OpenUpload,
    ));
    outbound.push_back(OriginOutbound {
        peer: transfer.peer.clone(),
        transfer_id: transfer.transfer_id.clone(),
        operation_id: transfer.operation_id.clone(),
        actor_access_epoch: outbound_access_epoch(transfer),
        frame,
    });
    Ok(())
}

fn queue_next_upload_action(
    transfer: &mut OriginTransfer,
    outbound: &mut VecDeque<OriginOutbound>,
) -> Result<(), FileNetworkOriginError> {
    let next = {
        let OriginTransferState::Upload(upload) = &mut transfer.state else {
            return Err(FileNetworkOriginError::UnexpectedResponse);
        };
        if upload.next_chunk as usize >= upload.manifest.chunks.len() {
            None
        } else {
            let chunk = upload.manifest.chunks[upload.next_chunk as usize].clone();
            if upload.next_offset >= chunk.length {
                return Err(FileNetworkOriginError::UnexpectedResponse);
            }
            let remaining = chunk.length.saturating_sub(upload.next_offset);
            let length = remaining.min(MAX_FILE_NETWORK_FRAGMENT_BYTES as u64);
            let offset = chunk.offset.saturating_add(upload.next_offset);
            upload.source.seek(SeekFrom::Start(offset))?;
            let mut data = vec![0_u8; usize::try_from(length).unwrap_or_default()];
            upload.source.read_exact(&mut data)?;
            upload.next_offset = upload.next_offset.saturating_add(length);
            Some((chunk.index, offset, length, data))
        }
    };
    let Some((chunk_index, offset, length, data)) = next else {
        return queue_finish_upload(transfer, outbound);
    };
    let request_id =
        next_origin_request_id(transfer, b"upload-chunk", chunk_index, offset, length)?;
    let frame = FileNetworkFrame::chunk(
        request_id,
        transfer.transfer_id.clone(),
        transfer.operation_id.clone(),
        transfer.resume_generation,
        chunk_index,
        offset,
        data,
    )
    .map_err(|_| FileNetworkOriginError::InvalidRequest)?;
    let retry_frame = frame.clone();
    transfer.pending = Some(PendingOriginReply::retryable(
        request_id,
        OriginRequestKind::UploadChunk {
            chunk_index,
            offset,
            length,
        },
        retry_frame,
    ));
    outbound.push_back(OriginOutbound {
        peer: transfer.peer.clone(),
        transfer_id: transfer.transfer_id.clone(),
        operation_id: transfer.operation_id.clone(),
        actor_access_epoch: outbound_access_epoch(transfer),
        frame,
    });
    Ok(())
}

fn queue_finish_upload(
    transfer: &mut OriginTransfer,
    outbound: &mut VecDeque<OriginOutbound>,
) -> Result<(), FileNetworkOriginError> {
    let request_id = next_origin_request_id(transfer, b"finish-upload", 0, 0, 0)?;
    let request = FileNetworkRequest::Finish {
        transfer_id: transfer.transfer_id.clone(),
        operation_id: transfer.operation_id.clone(),
    };
    let frame = FileNetworkFrame::request(request_id, request)
        .map_err(|_| FileNetworkOriginError::InvalidRequest)?;
    transfer.pending = Some(PendingOriginReply::new(
        request_id,
        OriginRequestKind::FinishUpload,
    ));
    outbound.push_back(OriginOutbound {
        peer: transfer.peer.clone(),
        transfer_id: transfer.transfer_id.clone(),
        operation_id: transfer.operation_id.clone(),
        actor_access_epoch: outbound_access_epoch(transfer),
        frame,
    });
    Ok(())
}

fn queue_pause(
    transfer: &mut OriginTransfer,
    outbound: &mut VecDeque<OriginOutbound>,
) -> Result<(), FileNetworkOriginError> {
    if transfer.pending.is_some() {
        return Err(FileNetworkOriginError::InvalidState);
    }
    let request_id = next_origin_request_id(transfer, b"pause", 0, 0, 0)?;
    let request = FileNetworkRequest::Pause {
        transfer_id: transfer.transfer_id.clone(),
        operation_id: transfer.operation_id.clone(),
    };
    let frame = FileNetworkFrame::request(request_id, request)
        .map_err(|_| FileNetworkOriginError::InvalidRequest)?;
    transfer.pending = Some(PendingOriginReply::new(
        request_id,
        OriginRequestKind::Pause,
    ));
    outbound.push_back(OriginOutbound {
        peer: transfer.peer.clone(),
        transfer_id: transfer.transfer_id.clone(),
        operation_id: transfer.operation_id.clone(),
        actor_access_epoch: outbound_access_epoch(transfer),
        frame,
    });
    Ok(())
}

fn queue_resume(
    transfer: &mut OriginTransfer,
    outbound: &mut VecDeque<OriginOutbound>,
) -> Result<(), FileNetworkOriginError> {
    let binding = transfer
        .binding
        .clone()
        .ok_or(FileNetworkOriginError::InvalidState)?;
    let (direction, content_hash) = match &transfer.state {
        OriginTransferState::Upload(upload) => (
            TransferDirection::Upload,
            upload.manifest.content_hash.clone(),
        ),
        OriginTransferState::Download(download) => {
            let session = download
                .session
                .as_ref()
                .ok_or(FileNetworkOriginError::InvalidState)?;
            (
                TransferDirection::Download,
                session.manifest.content_hash.clone(),
            )
        }
    };
    if binding.direction != direction
        || binding.transfer_id != transfer.transfer_id
        || binding.operation_id != transfer.operation_id
        || binding.share_id != transfer.share_id
        || binding.path != transfer.path
    {
        return Err(FileNetworkOriginError::BindingMismatch);
    }
    let (resume_chunk_index, resume_offset) = resume_cursor(transfer)?;
    let resume_generation = transfer
        .resume_generation
        .checked_add(1)
        .ok_or(FileNetworkOriginError::InvalidRequest)?;
    let request = ResumeRequest {
        transfer_id: transfer.transfer_id.clone(),
        operation_id: transfer.operation_id.clone(),
        share_id: binding.share_id,
        actor: binding.actor,
        access_epoch: binding.access_epoch,
        path: binding.path,
        direction,
        content_hash,
        share_revision: binding.share_revision,
        resume_chunk_index,
        resume_offset,
        resume_generation,
    };
    let label = match direction {
        TransferDirection::Upload => b"resume-upload".as_slice(),
        TransferDirection::Download => b"resume-download".as_slice(),
    };
    let request_id = next_origin_request_id(transfer, label, 0, 0, 0)?;
    let frame = FileNetworkFrame::request(request_id, FileNetworkRequest::Resume { request })
        .map_err(|_| FileNetworkOriginError::InvalidRequest)?;
    transfer.resume_generation = resume_generation;
    transfer.pending = Some(PendingOriginReply::new(
        request_id,
        match direction {
            TransferDirection::Upload => OriginRequestKind::ResumeUpload,
            TransferDirection::Download => OriginRequestKind::ResumeDownload,
        },
    ));
    outbound.push_back(OriginOutbound {
        peer: transfer.peer.clone(),
        transfer_id: transfer.transfer_id.clone(),
        operation_id: transfer.operation_id.clone(),
        actor_access_epoch: outbound_access_epoch(transfer),
        frame,
    });
    Ok(())
}

fn resume_cursor(transfer: &OriginTransfer) -> Result<(u32, u64), FileNetworkOriginError> {
    match &transfer.state {
        OriginTransferState::Upload(upload) => {
            if let Some(chunk) = upload.manifest.chunks.get(upload.next_chunk as usize) {
                let offset = chunk
                    .offset
                    .checked_add(upload.next_offset)
                    .ok_or(FileNetworkOriginError::ChunkCorrupt)?;
                if upload.next_offset > chunk.length || offset >= chunk.offset + chunk.length {
                    return Err(FileNetworkOriginError::ChunkOutOfOrder);
                }
                Ok((upload.next_chunk, offset))
            } else if upload.next_chunk as usize == upload.manifest.chunks.len() {
                Ok((upload.next_chunk, upload.manifest.total_bytes))
            } else {
                Err(FileNetworkOriginError::ChunkOutOfOrder)
            }
        }
        OriginTransferState::Download(download) => {
            let session = download
                .session
                .as_ref()
                .ok_or(FileNetworkOriginError::InvalidState)?;
            if let Some(assembly) = download.assembly.as_ref() {
                if assembly.chunk_index != download.received_chunks
                    || assembly.expected_offset
                        != session
                            .manifest
                            .chunks
                            .get(download.received_chunks as usize)
                            .map_or(assembly.expected_offset, |chunk| chunk.offset)
                {
                    return Err(FileNetworkOriginError::ChunkOutOfOrder);
                }
                let offset = assembly
                    .expected_offset
                    .checked_add(
                        u64::try_from(assembly.bytes.len())
                            .map_err(|_| FileNetworkOriginError::ChunkCorrupt)?,
                    )
                    .ok_or(FileNetworkOriginError::ChunkCorrupt)?;
                return Ok((download.received_chunks, offset));
            }
            if let Some(chunk) = session
                .manifest
                .chunks
                .get(download.received_chunks as usize)
            {
                Ok((download.received_chunks, chunk.offset))
            } else if download.received_chunks as usize == session.manifest.chunks.len() {
                Ok((download.received_chunks, session.manifest.total_bytes))
            } else {
                Err(FileNetworkOriginError::ChunkOutOfOrder)
            }
        }
    }
}

fn queue_cancel(transfer: &mut OriginTransfer, outbound: &mut VecDeque<OriginOutbound>) {
    let Ok(request_id) = next_origin_request_id(transfer, b"cancel", 0, 0, 0) else {
        return;
    };
    let request = FileNetworkRequest::Cancel {
        transfer_id: transfer.transfer_id.clone(),
        operation_id: transfer.operation_id.clone(),
    };
    if let Ok(frame) = FileNetworkFrame::request(request_id, request) {
        outbound.push_back(OriginOutbound {
            peer: transfer.peer.clone(),
            transfer_id: transfer.transfer_id.clone(),
            operation_id: transfer.operation_id.clone(),
            actor_access_epoch: outbound_access_epoch(transfer),
            frame,
        });
    }
}

fn validate_opened_upload(
    transfer: &OriginTransfer,
    session: &TransferSession,
    local_node_id: &NodeId,
) -> Result<(), FileNetworkOriginError> {
    validate_network_manifest(&session.manifest)
        .map_err(|_| FileNetworkOriginError::BindingMismatch)?;
    let OriginTransferState::Upload(upload) = &transfer.state else {
        return Err(FileNetworkOriginError::UnexpectedResponse);
    };
    validate_session_binding(transfer, session, TransferDirection::Upload, local_node_id)?;
    if session.manifest != upload.manifest
        || session.state != TransferState::Active
        || session.binding.file_version.is_some()
    {
        return Err(FileNetworkOriginError::BindingMismatch);
    }
    Ok(())
}

fn validate_opened_download(
    transfer: &OriginTransfer,
    session: &TransferSession,
    local_node_id: &NodeId,
) -> Result<(), FileNetworkOriginError> {
    validate_network_manifest(&session.manifest)
        .map_err(|_| FileNetworkOriginError::BindingMismatch)?;
    validate_session_binding(
        transfer,
        session,
        TransferDirection::Download,
        local_node_id,
    )?;
    let OriginTransferState::Download(download) = &transfer.state else {
        return Err(FileNetworkOriginError::UnexpectedResponse);
    };
    if download
        .session
        .as_ref()
        .is_some_and(|current| current.manifest != session.manifest)
        || session.received_chunk_count() != download.received_chunks as usize
        || (0..download.received_chunks).any(|index| !session.has_received_chunk(index))
    {
        return Err(FileNetworkOriginError::BindingMismatch);
    }
    if session.state != TransferState::Active || session.binding.file_version.is_none() {
        return Err(FileNetworkOriginError::BindingMismatch);
    }
    Ok(())
}

fn validate_session_binding(
    transfer: &OriginTransfer,
    session: &TransferSession,
    direction: TransferDirection,
    local_node_id: &NodeId,
) -> Result<(), FileNetworkOriginError> {
    if &session.binding.actor != local_node_id
        || session.binding.transfer_id != transfer.transfer_id
        || session.binding.operation_id != transfer.operation_id
        || session.binding.share_id != transfer.share_id
        || session.binding.path != transfer.path
        || session.binding.direction != direction
        || session.binding.access_epoch == 0
        || session.binding.share_revision == 0
        || session.binding.expires_at_unix_ms < transfer.deadline_unix_ms
        || transfer
            .binding
            .as_ref()
            .is_some_and(|binding| binding != &session.binding)
    {
        return Err(FileNetworkOriginError::BindingMismatch);
    }
    session
        .manifest
        .validate()
        .map_err(|_| FileNetworkOriginError::InvalidRequest)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_upload_ack(
    transfer: &OriginTransfer,
    chunk_index: u32,
    offset: u64,
    length: u64,
    acknowledged_chunk: u32,
    chunk_complete: bool,
    progress: &FileNetworkProgress,
) -> Result<(), FileNetworkOriginError> {
    if acknowledged_chunk != chunk_index || progress.state != TransferState::Active {
        return Err(FileNetworkOriginError::UnexpectedResponse);
    }
    let OriginTransferState::Upload(upload) = &transfer.state else {
        return Err(FileNetworkOriginError::UnexpectedResponse);
    };
    let chunk = upload
        .manifest
        .chunks
        .get(chunk_index as usize)
        .filter(|chunk| chunk.index == chunk_index)
        .ok_or(FileNetworkOriginError::UnexpectedResponse)?;
    let end = offset
        .checked_add(length)
        .ok_or(FileNetworkOriginError::UnexpectedResponse)?;
    let expected_end = chunk
        .offset
        .checked_add(upload.next_offset)
        .ok_or(FileNetworkOriginError::UnexpectedResponse)?;
    let chunk_end = chunk
        .offset
        .checked_add(chunk.length)
        .ok_or(FileNetworkOriginError::UnexpectedResponse)?;
    if length == 0
        || offset < chunk.offset
        || end != expected_end
        || end > chunk_end
        || chunk_complete != (end == chunk_end)
        || progress.total_bytes != upload.manifest.total_bytes
        || progress.total_chunks != transfer_total_chunks(transfer)
    {
        return Err(FileNetworkOriginError::UnexpectedResponse);
    }
    Ok(())
}

fn validate_pause(
    transfer: &OriginTransfer,
    progress: &FileNetworkProgress,
) -> Result<(), FileNetworkOriginError> {
    let (local_bytes, local_chunks) = match &transfer.state {
        OriginTransferState::Upload(upload) => {
            let local_bytes = upload
                .manifest
                .chunks
                .get(upload.next_chunk as usize)
                .map_or(upload.manifest.total_bytes, |chunk| chunk.offset);
            (local_bytes, upload.completed_chunks)
        }
        OriginTransferState::Download(download) => {
            (download.received_bytes, download.received_chunks)
        }
    };
    if progress.state != TransferState::Paused
        || progress.total_bytes != transfer_total_bytes(transfer)
        || progress.total_chunks != transfer_total_chunks(transfer)
        || progress.transferred_bytes < local_bytes
        || progress.transferred_bytes > progress.total_bytes
        || progress.received_chunks < local_chunks
        || progress.received_chunks > progress.total_chunks
    {
        return Err(FileNetworkOriginError::UnexpectedResponse);
    }
    Ok(())
}

fn validate_finish(
    transfer: &OriginTransfer,
    progress: &FileNetworkProgress,
) -> Result<(), FileNetworkOriginError> {
    if progress.state != TransferState::Completed
        || progress.transferred_bytes != transfer_total_bytes(transfer)
        || progress.total_bytes != transfer_total_bytes(transfer)
        || progress.received_chunks != transfer_total_chunks(transfer)
        || progress.total_chunks != transfer_total_chunks(transfer)
    {
        return Err(FileNetworkOriginError::UnexpectedResponse);
    }
    match &transfer.state {
        OriginTransferState::Upload(upload) => {
            if upload.next_chunk as usize != upload.manifest.chunks.len()
                || upload.completed_chunks != transfer_total_chunks(transfer)
            {
                return Err(FileNetworkOriginError::UnexpectedResponse);
            }
        }
        OriginTransferState::Download(download) => {
            let session = download
                .session
                .as_ref()
                .ok_or(FileNetworkOriginError::BindingMismatch)?;
            if session.state != TransferState::Verifying
                || download.assembly.is_some()
                || download.staging.is_none()
            {
                return Err(FileNetworkOriginError::UnexpectedResponse);
            }
        }
    }
    Ok(())
}

fn apply_download_fragment(
    transfer: &mut OriginTransfer,
    chunk_index: u32,
    offset: u64,
    data: &[u8],
    now_unix_ms: u64,
) -> Result<(), FileNetworkOriginError> {
    let OriginTransferState::Download(download) = &mut transfer.state else {
        return Err(FileNetworkOriginError::UnexpectedResponse);
    };
    if data.is_empty() || data.len() > MAX_FILE_NETWORK_FRAGMENT_BYTES {
        return Err(FileNetworkOriginError::ChunkCorrupt);
    }
    let session = download
        .session
        .as_ref()
        .ok_or(FileNetworkOriginError::ChunkOutOfOrder)?;
    let expected = session
        .manifest
        .chunks
        .get(chunk_index as usize)
        .filter(|chunk| chunk.index == chunk_index)
        .ok_or(FileNetworkOriginError::ChunkOutOfOrder)?;
    if u32::try_from(session.received_chunk_count()).ok() != Some(download.received_chunks) {
        return Err(FileNetworkOriginError::ChunkOutOfOrder);
    }
    let expected_end = expected.offset.saturating_add(expected.length);
    let incoming_end = offset
        .checked_add(data.len() as u64)
        .ok_or(FileNetworkOriginError::ChunkCorrupt)?;
    if offset < expected.offset || incoming_end > expected_end {
        return Err(FileNetworkOriginError::ChunkCorrupt);
    }
    let assembly = match download.assembly.as_mut() {
        Some(assembly) if assembly.chunk_index == chunk_index => {
            let next_offset = assembly
                .expected_offset
                .saturating_add(assembly.bytes.len() as u64);
            if offset != next_offset {
                return Err(FileNetworkOriginError::ChunkOutOfOrder);
            }
            assembly
        }
        Some(_) => return Err(FileNetworkOriginError::ChunkOutOfOrder),
        None => {
            if chunk_index != download.received_chunks || offset != expected.offset {
                return Err(FileNetworkOriginError::ChunkOutOfOrder);
            }
            download.assembly.insert(DownloadAssembly {
                chunk_index,
                expected_offset: expected.offset,
                expected_length: expected.length,
                expected_checksum: expected.checksum.clone(),
                bytes: Vec::with_capacity(usize::try_from(expected.length).unwrap_or_default()),
            })
        }
    };
    assembly.bytes.extend_from_slice(data);
    if assembly.bytes.len() as u64 != assembly.expected_length {
        return Ok(());
    }
    let assembly = download
        .assembly
        .take()
        .expect("completed download assembly");
    let checksum = ContentHash::from_bytes(&assembly.bytes);
    if checksum != assembly.expected_checksum {
        return Err(FileNetworkOriginError::ChunkCorrupt);
    }
    let staging = download
        .staging
        .as_mut()
        .ok_or(FileNetworkOriginError::BindingMismatch)?;
    staging.write_all(&assembly.bytes)?;
    download.hasher.update(&assembly.bytes);
    download.received_bytes = download
        .received_bytes
        .saturating_add(assembly.expected_length);
    download.received_chunks = download.received_chunks.saturating_add(1);
    let session = download
        .session
        .as_mut()
        .ok_or(FileNetworkOriginError::BindingMismatch)?;
    session.record_chunk(assembly.chunk_index, &assembly.bytes, now_unix_ms)?;
    Ok(())
}

fn origin_request_id(
    transfer_id: &TransferId,
    operation_id: &OperationId,
    label: &[u8],
    chunk_index: u32,
    offset: u64,
    length: u64,
    sequence: u64,
) -> [u8; 16] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"fuckipv6-file-origin-request-v1");
    hasher.update(transfer_id.as_str().as_bytes());
    hasher.update(&[0]);
    hasher.update(operation_id.as_str().as_bytes());
    hasher.update(label);
    hasher.update(&chunk_index.to_be_bytes());
    hasher.update(&offset.to_be_bytes());
    hasher.update(&length.to_be_bytes());
    hasher.update(&sequence.to_be_bytes());
    let digest = hasher.finalize();
    let mut request_id = [0_u8; 16];
    request_id.copy_from_slice(&digest.as_bytes()[..16]);
    request_id
}

fn next_origin_request_id(
    transfer: &mut OriginTransfer,
    label: &[u8],
    chunk_index: u32,
    offset: u64,
    length: u64,
) -> Result<[u8; 16], FileNetworkOriginError> {
    let sequence = transfer.next_request_sequence;
    transfer.next_request_sequence = sequence
        .checked_add(1)
        .ok_or(FileNetworkOriginError::InvalidRequest)?;
    Ok(origin_request_id(
        &transfer.transfer_id,
        &transfer.operation_id,
        label,
        chunk_index,
        offset,
        length,
        sequence,
    ))
}

fn build_upload_manifest(
    path: &SharedPath,
    source_path: &Path,
    chunk_size: u64,
) -> Result<(File, TransferManifest), FileNetworkOriginError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut source = options.open(source_path)?;
    let metadata = source.metadata()?;
    if !metadata.is_file() {
        return Err(FileNetworkOriginError::InvalidSource);
    }
    let total_bytes = metadata.len();
    let chunk_count = upload_manifest_chunk_count(total_bytes, chunk_size)?;
    let mut hasher = blake3::Hasher::new();
    let mut chunks = Vec::with_capacity(chunk_count);
    let mut offset = 0_u64;
    let mut index = 0_u32;
    while offset < total_bytes {
        let length = total_bytes.saturating_sub(offset).min(chunk_size);
        let mut bytes = vec![0_u8; usize::try_from(length).unwrap_or_default()];
        source.read_exact(&mut bytes)?;
        hasher.update(&bytes);
        chunks.push(FileChunk {
            index,
            offset,
            length,
            checksum: ContentHash::from_bytes(&bytes),
        });
        offset = offset.saturating_add(length);
        index = index
            .checked_add(1)
            .ok_or(FileNetworkOriginError::InvalidRequest)?;
    }
    let content_hash = ContentHash::from_digest_bytes(*hasher.finalize().as_bytes());
    let file_id = FileId::from_public_key(
        format!("{}:{}:1", path.as_str(), content_hash.as_str()).as_bytes(),
    );
    let manifest = TransferManifest {
        file_id,
        total_bytes,
        chunk_size,
        content_hash,
        chunks,
    };
    manifest
        .validate()
        .map_err(|_| FileNetworkOriginError::InvalidRequest)?;
    Ok((source, manifest))
}

fn upload_manifest_chunk_count(
    total_bytes: u64,
    chunk_size: u64,
) -> Result<usize, FileNetworkOriginError> {
    if chunk_size == 0
        || chunk_size > MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES as u64
        || total_bytes > MAX_FILE_NETWORK_TRANSFER_BYTES
    {
        return Err(FileNetworkOriginError::InvalidRequest);
    }
    let chunk_count = total_bytes.div_ceil(chunk_size);
    if chunk_count > MAX_FILE_NETWORK_MANIFEST_CHUNKS as u64 {
        return Err(FileNetworkOriginError::InvalidRequest);
    }
    usize::try_from(chunk_count).map_err(|_| FileNetworkOriginError::InvalidRequest)
}

fn validate_download_destination(destination: &Path) -> Result<(), FileNetworkOriginError> {
    let Some(name) = destination.file_name() else {
        return Err(FileNetworkOriginError::InvalidDestination);
    };
    if name.is_empty() || destination == Path::new(".") || destination == Path::new("..") {
        return Err(FileNetworkOriginError::InvalidDestination);
    }
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(FileNetworkOriginError::InvalidDestination);
    }
    match std::fs::symlink_metadata(destination) {
        Ok(_) => return Err(FileNetworkOriginError::InvalidDestination),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(FileNetworkOriginError::Io(error)),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use fuckipv6_common::domain::{FileChunk, FileId, FileVersion, ResumeRequest};
    use tempfile::TempDir;

    use super::*;

    const NOW: u64 = 10_000;

    fn node(byte: u8) -> NodeId {
        NodeId::from_public_key(&[byte])
    }

    fn transfer(byte: u8) -> TransferId {
        TransferId::from_public_key(&[byte])
    }

    fn operation(byte: u8) -> OperationId {
        OperationId::from_public_key(&[byte])
    }

    fn share(byte: u8) -> FileShareId {
        FileShareId::from_public_key(&[byte])
    }

    fn path() -> SharedPath {
        SharedPath::parse("docs/file.bin").expect("shared path")
    }

    fn peer() -> OriginPeer {
        OriginPeer {
            node_id: node(9),
            virtual_ip: Ipv6Addr::LOCALHOST,
        }
    }

    fn manifest(data: &[u8], chunk_size: u64) -> TransferManifest {
        let mut chunks = Vec::new();
        let mut offset = 0_u64;
        let mut index = 0_u32;
        while offset < data.len() as u64 {
            let length = (data.len() as u64).saturating_sub(offset).min(chunk_size);
            let start = usize::try_from(offset).expect("offset fits usize");
            let end = usize::try_from(offset + length).expect("end fits usize");
            chunks.push(FileChunk {
                index,
                offset,
                length,
                checksum: ContentHash::from_bytes(&data[start..end]),
            });
            offset += length;
            index += 1;
        }
        let content_hash = ContentHash::from_bytes(data);
        TransferManifest {
            file_id: FileId::from_public_key(
                format!("{}:{}:1", path().as_str(), content_hash.as_str()).as_bytes(),
            ),
            total_bytes: data.len() as u64,
            chunk_size,
            content_hash,
            chunks,
        }
    }

    fn active_session(
        local_node_id: &NodeId,
        direction: TransferDirection,
        transfer_id: TransferId,
        operation_id: OperationId,
        manifest: TransferManifest,
    ) -> TransferSession {
        let file_version = (direction == TransferDirection::Download).then(|| FileVersion {
            content_hash: manifest.content_hash.clone(),
            size: manifest.total_bytes,
            revision: 1,
        });
        let binding = fuckipv6_common::domain::TransferBinding {
            transfer_id: transfer_id.clone(),
            operation_id: operation_id.clone(),
            share_id: share(1),
            actor: local_node_id.clone(),
            access_epoch: 7,
            path: path(),
            direction,
            file_version: file_version.clone(),
            share_revision: 1,
            expires_at_unix_ms: NOW + FILE_NETWORK_TRANSFER_TTL_MS,
        };
        let mut session = TransferSession::new(binding, manifest.clone(), NOW).expect("session");
        let resume = ResumeRequest {
            transfer_id,
            operation_id,
            share_id: share(1),
            actor: local_node_id.clone(),
            access_epoch: 7,
            path: path(),
            direction,
            content_hash: manifest.content_hash,
            share_revision: 1,
            resume_chunk_index: 0,
            resume_offset: 0,
            resume_generation: 0,
        };
        session
            .resume(&resume, file_version.as_ref(), 1, true, 7, NOW + 1)
            .expect("activate session");
        session
    }

    fn progress(
        state: TransferState,
        transferred_bytes: u64,
        total_bytes: u64,
        received_chunks: u32,
        total_chunks: u32,
    ) -> FileNetworkProgress {
        FileNetworkProgress {
            state,
            transferred_bytes,
            total_bytes,
            received_chunks,
            total_chunks,
        }
    }

    fn take_request(origin: &mut FileNetworkOrigin) -> ([u8; 16], FileNetworkRequest) {
        let outbound = origin.pop_outbound().expect("outbound request");
        match outbound.frame {
            FileNetworkFrame::Request {
                request_id,
                request,
            } => (request_id, request),
            other => panic!("expected request frame, got {other:?}"),
        }
    }

    fn take_chunk(
        origin: &mut FileNetworkOrigin,
    ) -> ([u8; 16], TransferId, OperationId, u32, u64, Vec<u8>) {
        let (request_id, transfer_id, operation_id, _, chunk_index, offset, data) =
            take_chunk_with_generation(origin);
        (
            request_id,
            transfer_id,
            operation_id,
            chunk_index,
            offset,
            data,
        )
    }

    fn take_chunk_with_generation(
        origin: &mut FileNetworkOrigin,
    ) -> ([u8; 16], TransferId, OperationId, u64, u32, u64, Vec<u8>) {
        let outbound = origin.pop_outbound().expect("outbound chunk");
        match outbound.frame {
            FileNetworkFrame::Chunk {
                request_id,
                transfer_id,
                operation_id,
                resume_generation,
                chunk_index,
                offset,
                data,
            } => (
                request_id,
                transfer_id,
                operation_id,
                resume_generation,
                chunk_index,
                offset,
                data,
            ),
            other => panic!("expected chunk frame, got {other:?}"),
        }
    }

    fn start_upload(
        origin: &mut FileNetworkOrigin,
        local_node_id: &NodeId,
        source: &Path,
        byte: u8,
        data: &[u8],
        chunk_size: u64,
    ) -> ([u8; 16], TransferSession) {
        origin
            .start_upload(
                peer(),
                share(1),
                transfer(byte),
                operation(byte),
                path(),
                source,
                chunk_size,
                NOW,
            )
            .expect("start upload");
        let (request_id, request) = take_request(origin);
        let FileNetworkRequest::OpenUpload { manifest, .. } = request else {
            panic!("expected open upload");
        };
        assert_eq!(manifest, self::manifest(data, chunk_size));
        let session = active_session(
            local_node_id,
            TransferDirection::Upload,
            transfer(byte),
            operation(byte),
            manifest,
        );
        (request_id, session)
    }

    fn start_download(
        origin: &mut FileNetworkOrigin,
        local_node_id: &NodeId,
        destination: &Path,
        byte: u8,
        data: &[u8],
        chunk_size: u64,
    ) -> ([u8; 16], TransferSession) {
        origin
            .start_download(
                peer(),
                share(1),
                transfer(byte),
                operation(byte),
                path(),
                destination.to_path_buf(),
                NOW,
            )
            .expect("start download");
        let (request_id, request) = take_request(origin);
        assert!(matches!(request, FileNetworkRequest::OpenDownload { .. }));
        let session = active_session(
            local_node_id,
            TransferDirection::Download,
            transfer(byte),
            operation(byte),
            manifest(data, chunk_size),
        );
        (request_id, session)
    }

    fn paused_download_fixture(
        temporary: &TempDir,
        data: &[u8],
        chunk_size: u64,
        partial_length: usize,
    ) -> (PathBuf, NodeId, FileOriginStore, PathBuf) {
        let destination = temporary.path().join("download.bin");
        let local_node_id = node(4);
        let store = FileOriginStore::at(temporary.path(), "network");
        let mut origin = FileNetworkOrigin::with_store(local_node_id.clone(), store.clone());
        let (open_request_id, session) = start_download(
            &mut origin,
            &local_node_id,
            &destination,
            2,
            data,
            chunk_size,
        );
        origin
            .handle_response(
                &peer(),
                open_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");
        origin
            .handle_chunk(
                &peer(),
                download_fragment_request_id(&operation(2), 0, 0),
                &transfer(2),
                &operation(2),
                0,
                0,
                0,
                &data[..partial_length],
                NOW + 2,
            )
            .expect("partial fragment");
        origin
            .pause(&transfer(2), &operation(2), NOW + 3)
            .expect("pause");
        let (pause_request_id, _) = take_request(&mut origin);
        let total_chunks =
            u32::try_from(data.len().div_ceil(chunk_size as usize)).expect("chunk count fits u32");
        origin
            .handle_response(
                &peer(),
                pause_request_id,
                FileNetworkResponse::Progress {
                    progress: progress(
                        TransferState::Paused,
                        0,
                        data.len() as u64,
                        0,
                        total_chunks,
                    ),
                },
                NOW + 4,
            )
            .expect("pause response");
        drop(origin);
        let staging_path = store.load().expect("journal")[0]
            .download
            .as_ref()
            .expect("download state")
            .staging_path
            .clone();
        (destination, local_node_id, store, staging_path)
    }

    #[test]
    fn upload_manifest_bounds_are_checked_before_allocation() {
        assert_eq!(
            upload_manifest_chunk_count(
                MAX_FILE_NETWORK_TRANSFER_BYTES,
                MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES as u64,
            )
            .expect("maximum transfer fits"),
            MAX_FILE_NETWORK_MANIFEST_CHUNKS
        );
        assert!(matches!(
            upload_manifest_chunk_count(
                MAX_FILE_NETWORK_TRANSFER_BYTES + 1,
                MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES as u64,
            ),
            Err(FileNetworkOriginError::InvalidRequest)
        ));
        assert!(matches!(
            upload_manifest_chunk_count(MAX_FILE_NETWORK_MANIFEST_CHUNKS as u64 + 1, 1),
            Err(FileNetworkOriginError::InvalidRequest)
        ));
    }

    #[test]
    fn oversized_sparse_upload_is_rejected_before_reading_the_source() {
        let temporary = TempDir::new().expect("tempdir");
        let source = temporary.path().join("oversized.bin");
        let file = File::create(&source).expect("sparse source");
        file.set_len(MAX_FILE_NETWORK_TRANSFER_BYTES + 1)
            .expect("sparse length");
        drop(file);
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id);

        assert!(matches!(
            origin.start_upload(
                peer(),
                share(1),
                transfer(2),
                operation(2),
                path(),
                &source,
                MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES as u64,
                NOW,
            ),
            Err(FileNetworkOriginError::InvalidRequest)
        ));
        assert_eq!(origin.active_count(), 0);
        assert!(origin.pop_outbound().is_none());
    }

    #[test]
    fn tiny_chunks_cannot_amplify_the_upload_manifest() {
        let temporary = TempDir::new().expect("tempdir");
        let source = temporary.path().join("small.bin");
        std::fs::write(&source, vec![0_u8; MAX_FILE_NETWORK_MANIFEST_CHUNKS + 1]).expect("source");
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id);

        assert!(matches!(
            origin.start_upload(
                peer(),
                share(1),
                transfer(2),
                operation(2),
                path(),
                &source,
                1,
                NOW,
            ),
            Err(FileNetworkOriginError::InvalidRequest)
        ));
        assert_eq!(origin.active_count(), 0);
        assert!(origin.pop_outbound().is_none());
    }

    #[test]
    fn opened_responses_reject_manifests_outside_network_bounds() {
        let temporary = TempDir::new().expect("tempdir");
        let source = temporary.path().join("source.bin");
        std::fs::write(&source, b"ok").expect("source");
        let destination = temporary.path().join("download.bin");
        let invalid_manifest = manifest(&vec![7_u8; MAX_FILE_NETWORK_MANIFEST_CHUNKS + 1], 1);
        let local_node_id = node(4);

        let mut upload_origin = FileNetworkOrigin::new(local_node_id.clone());
        let (upload_request_id, _) =
            start_upload(&mut upload_origin, &local_node_id, &source, 2, b"ok", 2);
        let key = OriginTransferKey::new(&transfer(2), &operation(2));
        let OriginTransferState::Upload(upload) = &mut upload_origin
            .transfers
            .get_mut(&key)
            .expect("upload transfer")
            .state
        else {
            panic!("expected upload state");
        };
        upload.manifest = invalid_manifest.clone();
        assert!(matches!(
            upload_origin.handle_response(
                &peer(),
                upload_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(active_session(
                        &local_node_id,
                        TransferDirection::Upload,
                        transfer(2),
                        operation(2),
                        invalid_manifest.clone(),
                    )),
                },
                NOW + 1,
            ),
            Err(FileNetworkOriginError::BindingMismatch)
        ));
        assert_eq!(upload_origin.active_count(), 0);

        let mut download_origin = FileNetworkOrigin::new(local_node_id.clone());
        let (download_request_id, _) = start_download(
            &mut download_origin,
            &local_node_id,
            &destination,
            3,
            b"ok",
            2,
        );
        assert!(matches!(
            download_origin.handle_response(
                &peer(),
                download_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(active_session(
                        &local_node_id,
                        TransferDirection::Download,
                        transfer(3),
                        operation(3),
                        invalid_manifest,
                    )),
                },
                NOW + 1,
            ),
            Err(FileNetworkOriginError::BindingMismatch)
        ));
        assert_eq!(download_origin.active_count(), 0);
        assert!(!destination.exists());
    }

    #[test]
    fn upload_sends_bounded_fragments_acknowledges_and_finishes() {
        let temporary = TempDir::new().expect("tempdir");
        let source = temporary.path().join("source.bin");
        let data = vec![0x5a; MAX_FILE_NETWORK_FRAGMENT_BYTES * 2 + 7];
        std::fs::write(&source, &data).expect("source");
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let (request_id, session) = start_upload(
            &mut origin,
            &local_node_id,
            &source,
            2,
            &data,
            MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES as u64,
        );
        let _ = origin.drain_events();

        origin
            .handle_response(
                &peer(),
                request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");

        let mut sent = Vec::new();
        for _ in 0..3 {
            let (request_id, _, _, chunk_index, offset, fragment) = take_chunk(&mut origin);
            assert_eq!(chunk_index, 0);
            assert_eq!(offset, sent.len() as u64);
            sent.extend_from_slice(&fragment);
            let complete = sent.len() == data.len();
            origin
                .handle_response(
                    &peer(),
                    request_id,
                    FileNetworkResponse::ChunkAccepted {
                        chunk_index,
                        chunk_complete: complete,
                        progress: progress(
                            TransferState::Active,
                            sent.len() as u64,
                            data.len() as u64,
                            u32::from(complete),
                            1,
                        ),
                    },
                    NOW + 2,
                )
                .expect("chunk response");
        }
        assert_eq!(sent, data);

        let (finish_request_id, request) = take_request(&mut origin);
        assert!(matches!(request, FileNetworkRequest::Finish { .. }));
        origin
            .handle_response(
                &peer(),
                finish_request_id,
                FileNetworkResponse::Completed {
                    progress: progress(
                        TransferState::Completed,
                        data.len() as u64,
                        data.len() as u64,
                        1,
                        1,
                    ),
                },
                NOW + 3,
            )
            .expect("finish response");
        assert_eq!(origin.active_count(), 0);
        assert_eq!(
            origin.drain_events().last().map(|event| event.state),
            Some(TransferState::Completed)
        );
    }

    #[test]
    fn download_reassembles_verifies_and_atomically_persists() {
        let temporary = TempDir::new().expect("tempdir");
        let destination = temporary.path().join("download.bin");
        let data = vec![0x31; MAX_FILE_NETWORK_FRAGMENT_BYTES + 19];
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let (request_id, session) = start_download(
            &mut origin,
            &local_node_id,
            &destination,
            2,
            &data,
            MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES as u64,
        );
        let _ = origin.drain_events();
        origin
            .handle_response(
                &peer(),
                request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");
        assert!(!destination.exists());

        for (index, fragment) in data.chunks(MAX_FILE_NETWORK_FRAGMENT_BYTES).enumerate() {
            let offset = index as u64 * MAX_FILE_NETWORK_FRAGMENT_BYTES as u64;
            let request_id = download_fragment_request_id(&operation(2), 0, offset);
            origin
                .handle_chunk(
                    &peer(),
                    request_id,
                    &transfer(2),
                    &operation(2),
                    0,
                    0,
                    offset,
                    fragment,
                    NOW + 2,
                )
                .expect("download fragment");
        }
        assert!(!destination.exists());
        let (finish_request_id, request) = take_request(&mut origin);
        assert!(matches!(request, FileNetworkRequest::Finish { .. }));
        origin
            .handle_response(
                &peer(),
                finish_request_id,
                FileNetworkResponse::Completed {
                    progress: progress(
                        TransferState::Completed,
                        data.len() as u64,
                        data.len() as u64,
                        1,
                        1,
                    ),
                },
                NOW + 3,
            )
            .expect("complete download");
        assert_eq!(std::fs::read(&destination).expect("download"), data);
        assert_eq!(origin.active_count(), 0);
        let events = origin.drain_events();
        assert!(events
            .iter()
            .any(|event| event.state == TransferState::Verifying));
        assert_eq!(
            events.last().map(|event| event.state),
            Some(TransferState::Completed)
        );
    }

    #[test]
    fn wrong_peer_and_unknown_response_do_not_destroy_the_correlation() {
        let temporary = TempDir::new().expect("tempdir");
        let source = temporary.path().join("source.bin");
        std::fs::write(&source, b"hello").expect("source");
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let (request_id, session) =
            start_upload(&mut origin, &local_node_id, &source, 2, b"hello", 16);

        let wrong_peer = OriginPeer {
            node_id: node(8),
            virtual_ip: Ipv6Addr::LOCALHOST,
        };
        assert!(matches!(
            origin.handle_response(
                &wrong_peer,
                request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session.clone()),
                },
                NOW + 1,
            ),
            Err(FileNetworkOriginError::PeerMismatch)
        ));
        assert_eq!(origin.active_count(), 1);
        assert!(matches!(
            origin.handle_response(
                &peer(),
                [0xff; 16],
                FileNetworkResponse::Opened {
                    session: Box::new(session.clone()),
                },
                NOW + 1,
            ),
            Err(FileNetworkOriginError::UnknownResponse)
        ));
        assert_eq!(origin.active_count(), 1);

        origin
            .handle_response(
                &peer(),
                request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("correct response");
        assert_eq!(origin.active_count(), 1);
    }

    #[test]
    fn misbound_open_response_fails_closed_and_cancels_remote() {
        let temporary = TempDir::new().expect("tempdir");
        let source = temporary.path().join("source.bin");
        std::fs::write(&source, b"hello").expect("source");
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let (request_id, mut session) =
            start_upload(&mut origin, &local_node_id, &source, 2, b"hello", 16);
        session.binding.path = SharedPath::parse("docs/other.bin").expect("path");

        assert!(matches!(
            origin.handle_response(
                &peer(),
                request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            ),
            Err(FileNetworkOriginError::BindingMismatch)
        ));
        assert_eq!(origin.active_count(), 0);
        let (_, request) = take_request(&mut origin);
        assert!(matches!(request, FileNetworkRequest::Cancel { .. }));
        let event = origin.drain_events().pop().expect("failure event");
        assert_eq!(event.state, TransferState::Failed);
        assert_eq!(
            event.error_code.as_deref(),
            Some("file_transfer_binding_mismatch")
        );
    }

    #[test]
    fn replayed_ack_is_rejected_without_failing_active_transfer() {
        let temporary = TempDir::new().expect("tempdir");
        let source = temporary.path().join("source.bin");
        std::fs::write(&source, b"hello").expect("source");
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let (open_request_id, session) =
            start_upload(&mut origin, &local_node_id, &source, 2, b"hello", 16);
        origin
            .handle_response(
                &peer(),
                open_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");
        let (chunk_request_id, _, _, chunk_index, offset, fragment) = take_chunk(&mut origin);
        origin
            .handle_response(
                &peer(),
                chunk_request_id,
                FileNetworkResponse::ChunkAccepted {
                    chunk_index,
                    chunk_complete: true,
                    progress: progress(TransferState::Active, 5, 5, 1, 1),
                },
                NOW + 2,
            )
            .expect("chunk ack");
        assert_eq!(fragment, b"hello");
        assert_eq!(offset, 0);
        let (_, finish_request) = take_request(&mut origin);
        assert!(matches!(finish_request, FileNetworkRequest::Finish { .. }));
        assert!(matches!(
            origin.handle_response(
                &peer(),
                chunk_request_id,
                FileNetworkResponse::ChunkAccepted {
                    chunk_index,
                    chunk_complete: true,
                    progress: progress(TransferState::Active, 5, 5, 1, 1),
                },
                NOW + 3,
            ),
            Err(FileNetworkOriginError::UnknownResponse)
        ));
        assert_eq!(origin.active_count(), 1);
    }

    #[test]
    fn requester_rate_limit_replays_the_exact_chunk_without_advancing() {
        let temporary = TempDir::new().expect("tempdir");
        let source = temporary.path().join("source.bin");
        let data = b"abcdefgh";
        std::fs::write(&source, data).expect("source");
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let (open_request_id, session) =
            start_upload(&mut origin, &local_node_id, &source, 2, data, 4);
        origin
            .handle_response(
                &peer(),
                open_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");
        let first = take_chunk_with_generation(&mut origin);
        assert_eq!(first.3, 0);
        assert_eq!((first.4, first.5, first.6.as_slice()), (0, 0, &data[..4]));

        origin
            .handle_response(
                &peer(),
                first.0,
                FileNetworkResponse::Error {
                    code: "file_transfer_rate_limited".to_owned(),
                    retry_after_ms: Some(100),
                },
                NOW + 2,
            )
            .expect("rate limit is retryable");
        assert_eq!(origin.active_count(), 1);
        assert_eq!(origin.next_retry_at_unix_ms(), Some(NOW + 102));
        assert!(origin.pop_outbound().is_none());
        origin.queue_due_retries(NOW + 101);
        assert!(origin.pop_outbound().is_none());

        origin.queue_due_retries(NOW + 102);
        let replay = take_chunk_with_generation(&mut origin);
        assert_eq!(replay, first);

        origin
            .handle_response(
                &peer(),
                replay.0,
                FileNetworkResponse::Error {
                    code: "file_transfer_rate_limited".to_owned(),
                    retry_after_ms: Some(200),
                },
                NOW + 103,
            )
            .expect("a repeated rate limit keeps the same pending chunk");
        assert_eq!(origin.active_count(), 1);
        assert_eq!(origin.next_retry_at_unix_ms(), Some(NOW + 303));
        origin.queue_due_retries(NOW + 302);
        assert!(origin.pop_outbound().is_none());
        origin.queue_due_retries(NOW + 303);
        let second_replay = take_chunk_with_generation(&mut origin);
        assert_eq!(second_replay, first);

        origin
            .handle_response(
                &peer(),
                second_replay.0,
                FileNetworkResponse::ChunkAccepted {
                    chunk_index: 0,
                    chunk_complete: true,
                    progress: progress(TransferState::Active, 4, data.len() as u64, 1, 2),
                },
                NOW + 304,
            )
            .expect("accepted chunk");
        let next = take_chunk_with_generation(&mut origin);
        assert_eq!(next.3, 0);
        assert_eq!((next.4, next.5, next.6.as_slice()), (1, 4, &data[4..]));
        origin
            .handle_response(
                &peer(),
                next.0,
                FileNetworkResponse::ChunkAccepted {
                    chunk_index: 1,
                    chunk_complete: true,
                    progress: progress(TransferState::Active, 8, data.len() as u64, 2, 2),
                },
                NOW + 305,
            )
            .expect("accepted second chunk");
        let (_, finish) = take_request(&mut origin);
        assert!(matches!(finish, FileNetworkRequest::Finish { .. }));
    }

    #[test]
    fn requester_rate_limit_without_retry_interval_fails_closed() {
        let temporary = TempDir::new().expect("tempdir");
        let source = temporary.path().join("source.bin");
        std::fs::write(&source, b"abcdefgh").expect("source");
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let (open_request_id, session) =
            start_upload(&mut origin, &local_node_id, &source, 2, b"abcdefgh", 4);
        origin
            .handle_response(
                &peer(),
                open_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");
        let (chunk_request_id, _, _, _, _, _) = take_chunk(&mut origin);
        origin
            .handle_response(
                &peer(),
                chunk_request_id,
                FileNetworkResponse::Error {
                    code: "file_transfer_rate_limited".to_owned(),
                    retry_after_ms: None,
                },
                NOW + 2,
            )
            .expect("fatal response is recorded");
        assert_eq!(origin.active_count(), 0);
        assert!(origin.next_retry_at_unix_ms().is_none());
        let event = origin.drain_events().pop().expect("failure event");
        assert_eq!(event.state, TransferState::Failed);
        assert_eq!(
            event.error_code.as_deref(),
            Some("file_transfer_rate_limited")
        );
        assert_eq!(event.failure_kind(), Some(FileTransferFailureKind::Quota));
        origin.queue_due_retries(NOW + 1_000);
        assert!(origin.pop_outbound().is_none());
    }

    #[test]
    fn requester_pause_cancels_a_delayed_rate_limit_replay() {
        let temporary = TempDir::new().expect("tempdir");
        let source = temporary.path().join("source.bin");
        std::fs::write(&source, b"abcdefgh").expect("source");
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let (open_request_id, session) =
            start_upload(&mut origin, &local_node_id, &source, 2, b"abcdefgh", 4);
        origin
            .handle_response(
                &peer(),
                open_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");
        let (chunk_request_id, _, _, _, _, _) = take_chunk(&mut origin);
        origin
            .handle_response(
                &peer(),
                chunk_request_id,
                FileNetworkResponse::Error {
                    code: "file_transfer_rate_limited".to_owned(),
                    retry_after_ms: Some(100),
                },
                NOW + 2,
            )
            .expect("rate limit is retryable");
        origin
            .pause(&transfer(2), &operation(2), NOW + 3)
            .expect("pause replaces the delayed chunk");
        assert!(origin.next_retry_at_unix_ms().is_none());
        origin.queue_due_retries(NOW + 102);
        let (_, request) = take_request(&mut origin);
        assert!(matches!(request, FileNetworkRequest::Pause { .. }));
        assert!(origin.pop_outbound().is_none());
        assert_eq!(origin.active_count(), 1);
    }

    #[test]
    fn requester_cancel_removes_a_delayed_rate_limit_replay() {
        let temporary = TempDir::new().expect("tempdir");
        let source = temporary.path().join("source.bin");
        std::fs::write(&source, b"abcdefgh").expect("source");
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let (open_request_id, session) =
            start_upload(&mut origin, &local_node_id, &source, 2, b"abcdefgh", 4);
        origin
            .handle_response(
                &peer(),
                open_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");
        let (chunk_request_id, _, _, _, _, _) = take_chunk(&mut origin);
        origin
            .handle_response(
                &peer(),
                chunk_request_id,
                FileNetworkResponse::Error {
                    code: "file_transfer_rate_limited".to_owned(),
                    retry_after_ms: Some(100),
                },
                NOW + 2,
            )
            .expect("rate limit is retryable");
        assert!(origin.cancel(&transfer(2), &operation(2), NOW + 3));
        origin.queue_due_retries(NOW + 102);
        let (_, request) = take_request(&mut origin);
        assert!(matches!(request, FileNetworkRequest::Cancel { .. }));
        assert!(origin.pop_outbound().is_none());
        assert_eq!(origin.active_count(), 0);
    }

    #[test]
    fn requester_transfer_deadline_wins_over_a_delayed_retry() {
        let temporary = TempDir::new().expect("tempdir");
        let source = temporary.path().join("source.bin");
        std::fs::write(&source, b"abcdefgh").expect("source");
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let (open_request_id, session) =
            start_upload(&mut origin, &local_node_id, &source, 2, b"abcdefgh", 4);
        origin
            .handle_response(
                &peer(),
                open_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");
        let (chunk_request_id, _, _, _, _, _) = take_chunk(&mut origin);
        origin
            .handle_response(
                &peer(),
                chunk_request_id,
                FileNetworkResponse::Error {
                    code: "file_transfer_rate_limited".to_owned(),
                    retry_after_ms: Some(FILE_NETWORK_TRANSFER_TTL_MS),
                },
                NOW + 1,
            )
            .expect("rate limit is retryable");
        assert_eq!(
            origin.next_retry_at_unix_ms(),
            Some(NOW + FILE_NETWORK_TRANSFER_TTL_MS)
        );
        origin.queue_due_retries(NOW + FILE_NETWORK_TRANSFER_TTL_MS);
        assert_eq!(origin.active_count(), 0);
        while let Some(outbound) = origin.pop_outbound() {
            assert!(!matches!(outbound.frame, FileNetworkFrame::Chunk { .. }));
        }
        let event = origin.drain_events().pop().expect("expiry event");
        assert_eq!(event.state, TransferState::Expired);
        assert_eq!(event.error_code.as_deref(), Some("file_transfer_expired"));
    }

    #[test]
    fn requester_failures_preserve_network_authorization_quota_and_storage_kinds() {
        let temporary = TempDir::new().expect("tempdir");
        let source = temporary.path().join("source.bin");
        std::fs::write(&source, b"hello").expect("source");
        let local_node_id = node(4);

        for (byte, code, expected_kind) in [
            (
                2,
                "file_transfer_permission_revoked",
                FileTransferFailureKind::Authorization,
            ),
            (
                3,
                "file_total_quota_exceeded",
                FileTransferFailureKind::Quota,
            ),
            (
                4,
                "file_disk_space_reserved",
                FileTransferFailureKind::Storage,
            ),
        ] {
            let mut origin = FileNetworkOrigin::new(local_node_id.clone());
            let (request_id, _) =
                start_upload(&mut origin, &local_node_id, &source, byte, b"hello", 8);
            origin
                .handle_response(
                    &peer(),
                    request_id,
                    FileNetworkResponse::Error {
                        code: code.to_owned(),
                        retry_after_ms: None,
                    },
                    NOW + 1,
                )
                .expect("error response");
            let event = origin.drain_events().pop().expect("failure event");
            assert_eq!(event.state, TransferState::Failed);
            assert_eq!(event.error_code.as_deref(), Some(code));
            assert_eq!(event.failure_kind(), Some(expected_kind));
            assert_eq!(origin.active_count(), 0);
        }

        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        start_upload(&mut origin, &local_node_id, &source, 5, b"hello", 8);
        assert!(origin.fail_transfer(
            &transfer(5),
            &operation(5),
            NOW + 1,
            "file_transfer_send_failed",
        ));
        let event = origin.drain_events().pop().expect("network failure event");
        assert_eq!(event.failure_kind(), Some(FileTransferFailureKind::Network));
    }

    #[test]
    fn replayed_download_fragment_fails_closed_and_cleans_staging() {
        let temporary = TempDir::new().expect("tempdir");
        let destination = temporary.path().join("download.bin");
        let data = b"abcdefgh";
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let (open_request_id, session) =
            start_download(&mut origin, &local_node_id, &destination, 2, data, 4);
        origin
            .handle_response(
                &peer(),
                open_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");

        let request_id = download_fragment_request_id(&operation(2), 0, 0);
        origin
            .handle_chunk(
                &peer(),
                request_id,
                &transfer(2),
                &operation(2),
                0,
                0,
                0,
                &data[..4],
                NOW + 2,
            )
            .expect("first fragment");
        assert_eq!(origin.active_count(), 1);
        assert!(!destination.exists());
        assert_eq!(
            std::fs::read_dir(temporary.path())
                .expect("read dir")
                .count(),
            1
        );

        assert!(matches!(
            origin.handle_chunk(
                &peer(),
                request_id,
                &transfer(2),
                &operation(2),
                0,
                0,
                0,
                &data[..4],
                NOW + 3,
            ),
            Err(FileNetworkOriginError::ChunkOutOfOrder)
        ));
        assert_eq!(origin.active_count(), 0);
        assert!(!destination.exists());
        assert_eq!(
            std::fs::read_dir(temporary.path())
                .expect("read dir")
                .count(),
            0
        );
        assert!(matches!(
            take_request(&mut origin).1,
            FileNetworkRequest::Cancel { .. }
        ));
        let event = origin.drain_events().pop().expect("failure event");
        assert_eq!(event.state, TransferState::Failed);
        assert_eq!(
            event.error_code.as_deref(),
            Some("file_transfer_chunk_out_of_order")
        );
    }

    #[test]
    fn future_download_generation_fails_closed_and_removes_staging() {
        let temporary = TempDir::new().expect("tempdir");
        let destination = temporary.path().join("download.bin");
        let data = b"generation barrier";
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let (open_request_id, session) =
            start_download(&mut origin, &local_node_id, &destination, 2, data, 8);
        origin
            .handle_response(
                &peer(),
                open_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");
        assert_eq!(
            std::fs::read_dir(temporary.path())
                .expect("read dir")
                .count(),
            1
        );

        assert!(matches!(
            origin.handle_chunk(
                &peer(),
                download_fragment_request_id(&operation(2), 0, 0),
                &transfer(2),
                &operation(2),
                2,
                0,
                0,
                data,
                NOW + 2,
            ),
            Err(FileNetworkOriginError::ChunkOutOfOrder)
        ));
        assert_eq!(origin.active_count(), 0);
        assert!(!destination.exists());
        assert_eq!(
            std::fs::read_dir(temporary.path())
                .expect("read dir")
                .count(),
            0
        );
        assert!(matches!(
            take_request(&mut origin).1,
            FileNetworkRequest::Cancel { .. }
        ));
        let event = origin.drain_events().pop().expect("failure event");
        assert_eq!(event.state, TransferState::Failed);
        assert_eq!(
            event.error_code.as_deref(),
            Some("file_transfer_resume_generation_mismatch")
        );
    }

    #[test]
    fn replayed_completion_response_does_not_publish_twice() {
        let temporary = TempDir::new().expect("tempdir");
        let destination = temporary.path().join("download.bin");
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let (open_request_id, session) =
            start_download(&mut origin, &local_node_id, &destination, 2, &[], 8);
        origin
            .handle_response(
                &peer(),
                open_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");
        let (finish_request_id, finish_request) = take_request(&mut origin);
        assert!(matches!(finish_request, FileNetworkRequest::Finish { .. }));
        let completed = FileNetworkResponse::Completed {
            progress: progress(TransferState::Completed, 0, 0, 0, 0),
        };
        origin
            .handle_response(&peer(), finish_request_id, completed.clone(), NOW + 2)
            .expect("finish response");
        assert_eq!(origin.active_count(), 0);
        assert!(std::fs::read(&destination)
            .expect("published file")
            .is_empty());
        assert_eq!(
            std::fs::read_dir(temporary.path())
                .expect("read dir")
                .count(),
            1
        );
        let events = origin.drain_events();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.state == TransferState::Completed)
                .count(),
            1
        );

        assert!(matches!(
            origin.handle_response(&peer(), finish_request_id, completed, NOW + 3),
            Err(FileNetworkOriginError::UnknownResponse)
        ));
        assert_eq!(origin.active_count(), 0);
        assert!(std::fs::read(&destination)
            .expect("published file")
            .is_empty());
        assert_eq!(
            std::fs::read_dir(temporary.path())
                .expect("read dir")
                .count(),
            1
        );
        assert!(origin.drain_events().is_empty());
    }

    #[test]
    fn out_of_order_and_corrupt_download_fragments_fail_closed() {
        let temporary = TempDir::new().expect("tempdir");
        let destination = temporary.path().join("download.bin");
        let data = vec![0x44; MAX_FILE_NETWORK_FRAGMENT_BYTES * 2];
        let local_node_id = node(4);

        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let (request_id, session) = start_download(
            &mut origin,
            &local_node_id,
            &destination,
            2,
            &data,
            MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES as u64,
        );
        origin
            .handle_response(
                &peer(),
                request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");
        let wrong_offset = MAX_FILE_NETWORK_FRAGMENT_BYTES as u64;
        let wrong_request_id = download_fragment_request_id(&operation(2), 0, wrong_offset);
        assert!(matches!(
            origin.handle_chunk(
                &peer(),
                wrong_request_id,
                &transfer(2),
                &operation(2),
                0,
                0,
                wrong_offset,
                &data[MAX_FILE_NETWORK_FRAGMENT_BYTES..],
                NOW + 2,
            ),
            Err(FileNetworkOriginError::ChunkOutOfOrder)
        ));
        assert_eq!(origin.active_count(), 0);
        assert!(!destination.exists());
        assert_eq!(
            std::fs::read_dir(temporary.path())
                .expect("read dir")
                .count(),
            0
        );

        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let (request_id, session) = start_download(
            &mut origin,
            &local_node_id,
            &destination,
            2,
            &data,
            MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES as u64,
        );
        origin
            .handle_response(
                &peer(),
                request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");
        let request_id = download_fragment_request_id(&operation(2), 0, 0);
        origin
            .handle_chunk(
                &peer(),
                request_id,
                &transfer(2),
                &operation(2),
                0,
                0,
                0,
                &data[..MAX_FILE_NETWORK_FRAGMENT_BYTES],
                NOW + 2,
            )
            .expect("first download fragment");
        let request_id =
            download_fragment_request_id(&operation(2), 0, MAX_FILE_NETWORK_FRAGMENT_BYTES as u64);
        let corrupt = vec![0; MAX_FILE_NETWORK_FRAGMENT_BYTES];
        assert!(matches!(
            origin.handle_chunk(
                &peer(),
                request_id,
                &transfer(2),
                &operation(2),
                0,
                0,
                MAX_FILE_NETWORK_FRAGMENT_BYTES as u64,
                &corrupt,
                NOW + 3,
            ),
            Err(FileNetworkOriginError::ChunkCorrupt)
        ));
        assert_eq!(origin.active_count(), 0);
        assert!(!destination.exists());
        assert_eq!(
            std::fs::read_dir(temporary.path())
                .expect("read dir")
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn misattributed_download_fragment_cleans_only_target_staging() {
        let temporary = TempDir::new().expect("tempdir");
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let notify = origin.outbound_notify();
        let (first_request_id, first_session) = start_download(
            &mut origin,
            &local_node_id,
            &temporary.path().join("first.bin"),
            2,
            b"first",
            4,
        );
        expect_outbound_notification(&notify).await;
        let (second_request_id, second_session) = start_download(
            &mut origin,
            &local_node_id,
            &temporary.path().join("second.bin"),
            3,
            b"second",
            4,
        );
        expect_outbound_notification(&notify).await;

        origin
            .handle_response(
                &peer(),
                first_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(first_session),
                },
                NOW + 1,
            )
            .expect("first open response");
        expect_outbound_notification(&notify).await;
        origin
            .handle_response(
                &peer(),
                second_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(second_session),
                },
                NOW + 1,
            )
            .expect("second open response");
        expect_outbound_notification(&notify).await;
        assert_eq!(
            std::fs::read_dir(temporary.path())
                .expect("read dir")
                .count(),
            2
        );

        let wrong_peer = OriginPeer {
            node_id: node(8),
            virtual_ip: Ipv6Addr::LOCALHOST,
        };
        assert!(matches!(
            origin.handle_chunk(
                &wrong_peer,
                download_fragment_request_id(&operation(2), 0, 0),
                &transfer(2),
                &operation(2),
                0,
                0,
                0,
                b"firs",
                NOW + 2,
            ),
            Err(FileNetworkOriginError::PeerMismatch)
        ));
        expect_outbound_notification(&notify).await;
        assert!(!origin.owns_download(&peer().node_id, &transfer(2), &operation(2)));
        assert!(origin.owns_download(&peer().node_id, &transfer(3), &operation(3)));
        assert_eq!(
            std::fs::read_dir(temporary.path())
                .expect("read dir")
                .count(),
            1
        );
        let (_, cancel_request) = take_request(&mut origin);
        assert!(matches!(cancel_request, FileNetworkRequest::Cancel { .. }));

        origin
            .handle_chunk(
                &peer(),
                download_fragment_request_id(&operation(3), 0, 0),
                &transfer(3),
                &operation(3),
                0,
                0,
                0,
                b"seco",
                NOW + 3,
            )
            .expect("unrelated download remains active");
        assert!(origin.owns_download(&peer().node_id, &transfer(3), &operation(3)));
    }

    #[tokio::test]
    async fn wrong_download_request_id_cleans_only_target_staging() {
        let temporary = TempDir::new().expect("tempdir");
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let notify = origin.outbound_notify();
        let (first_request_id, first_session) = start_download(
            &mut origin,
            &local_node_id,
            &temporary.path().join("first.bin"),
            2,
            b"first",
            4,
        );
        expect_outbound_notification(&notify).await;
        let (second_request_id, second_session) = start_download(
            &mut origin,
            &local_node_id,
            &temporary.path().join("second.bin"),
            3,
            b"second",
            4,
        );
        expect_outbound_notification(&notify).await;

        origin
            .handle_response(
                &peer(),
                first_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(first_session),
                },
                NOW + 1,
            )
            .expect("first open response");
        expect_outbound_notification(&notify).await;
        origin
            .handle_response(
                &peer(),
                second_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(second_session),
                },
                NOW + 1,
            )
            .expect("second open response");
        expect_outbound_notification(&notify).await;

        assert!(matches!(
            origin.handle_chunk(
                &peer(),
                [0xff; 16],
                &transfer(2),
                &operation(2),
                0,
                0,
                0,
                b"firs",
                NOW + 2,
            ),
            Err(FileNetworkOriginError::ChunkOutOfOrder)
        ));
        expect_outbound_notification(&notify).await;
        assert!(!origin.owns_download(&peer().node_id, &transfer(2), &operation(2)));
        assert!(origin.owns_download(&peer().node_id, &transfer(3), &operation(3)));
        assert_eq!(
            std::fs::read_dir(temporary.path())
                .expect("read dir")
                .count(),
            1
        );
        let (_, cancel_request) = take_request(&mut origin);
        assert!(matches!(cancel_request, FileNetworkRequest::Cancel { .. }));

        origin
            .handle_chunk(
                &peer(),
                download_fragment_request_id(&operation(3), 0, 0),
                &transfer(3),
                &operation(3),
                0,
                0,
                0,
                b"seco",
                NOW + 3,
            )
            .expect("unrelated download remains active");
        assert!(origin.owns_download(&peer().node_id, &transfer(3), &operation(3)));
    }

    #[test]
    fn empty_files_complete_without_data_fragments() {
        let temporary = TempDir::new().expect("tempdir");
        let source = temporary.path().join("empty-source.bin");
        std::fs::write(&source, []).expect("source");
        let local_node_id = node(4);

        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let (request_id, session) = start_upload(&mut origin, &local_node_id, &source, 2, &[], 16);
        origin
            .handle_response(
                &peer(),
                request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open empty upload");
        let (finish_request_id, request) = take_request(&mut origin);
        assert!(matches!(request, FileNetworkRequest::Finish { .. }));
        origin
            .handle_response(
                &peer(),
                finish_request_id,
                FileNetworkResponse::Completed {
                    progress: progress(TransferState::Completed, 0, 0, 0, 0),
                },
                NOW + 2,
            )
            .expect("finish empty upload");
        assert_eq!(origin.active_count(), 0);

        let destination = temporary.path().join("empty-download.bin");
        let (request_id, session) =
            start_download(&mut origin, &local_node_id, &destination, 3, &[], 16);
        origin
            .handle_response(
                &peer(),
                request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open empty download");
        let (finish_request_id, request) = take_request(&mut origin);
        assert!(matches!(request, FileNetworkRequest::Finish { .. }));
        origin
            .handle_response(
                &peer(),
                finish_request_id,
                FileNetworkResponse::Completed {
                    progress: progress(TransferState::Completed, 0, 0, 0, 0),
                },
                NOW + 2,
            )
            .expect("finish empty download");
        assert!(std::fs::read(&destination)
            .expect("empty download")
            .is_empty());
        assert_eq!(origin.active_count(), 0);
    }

    #[test]
    fn expiry_removes_partial_download_staging() {
        let temporary = TempDir::new().expect("tempdir");
        let destination = temporary.path().join("expired.bin");
        let data = b"partial";
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let (request_id, session) =
            start_download(&mut origin, &local_node_id, &destination, 2, data, 16);
        origin
            .handle_response(
                &peer(),
                request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");
        assert_eq!(
            std::fs::read_dir(temporary.path())
                .expect("read dir")
                .count(),
            1
        );

        origin.expire(NOW + FILE_NETWORK_TRANSFER_TTL_MS);
        assert_eq!(origin.active_count(), 0);
        assert!(!destination.exists());
        assert_eq!(
            std::fs::read_dir(temporary.path())
                .expect("read dir")
                .count(),
            0
        );
        let event = origin.drain_events().pop().expect("expiry event");
        assert_eq!(event.state, TransferState::Expired);
    }

    #[test]
    fn duplicate_and_bounded_transfer_table_fail_without_side_effects() {
        let temporary = TempDir::new().expect("tempdir");
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id);

        origin
            .start_download(
                peer(),
                share(1),
                transfer(1),
                operation(1),
                path(),
                temporary.path().join("one.bin"),
                NOW,
            )
            .expect("first transfer");
        assert!(matches!(
            origin.start_download(
                peer(),
                share(1),
                transfer(1),
                operation(1),
                path(),
                temporary.path().join("duplicate.bin"),
                NOW,
            ),
            Err(FileNetworkOriginError::DuplicateTransfer)
        ));
        for index in 2..=MAX_ORIGIN_TRANSFERS {
            origin
                .start_download(
                    peer(),
                    share(1),
                    transfer(index as u8),
                    operation(index as u8),
                    path(),
                    temporary.path().join(format!("{index}.bin")),
                    NOW,
                )
                .expect("bounded transfer");
        }
        assert_eq!(origin.active_count(), MAX_ORIGIN_TRANSFERS);
        assert!(matches!(
            origin.start_download(
                peer(),
                share(1),
                transfer((MAX_ORIGIN_TRANSFERS + 1) as u8),
                operation((MAX_ORIGIN_TRANSFERS + 1) as u8),
                path(),
                temporary.path().join("overflow.bin"),
                NOW,
            ),
            Err(FileNetworkOriginError::TransferLimit)
        ));
    }

    #[test]
    fn event_queue_is_bounded() {
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let event = OriginTransferEvent {
            transfer_id: transfer(2),
            operation_id: operation(3),
            direction: TransferDirection::Download,
            state: TransferState::Active,
            transferred_bytes: 0,
            total_bytes: 0,
            received_chunks: 0,
            total_chunks: 0,
            error_code: None,
        };
        for _ in 0..MAX_ORIGIN_EVENTS + 10 {
            origin.push_event(event.clone());
        }
        assert_eq!(origin.drain_events().len(), MAX_ORIGIN_EVENTS);
    }

    async fn expect_outbound_notification(notify: &Arc<Notify>) {
        tokio::time::timeout(std::time::Duration::from_millis(100), notify.notified())
            .await
            .expect("outbound notification");
    }

    #[tokio::test]
    async fn outbound_notify_wakes_all_transfer_transitions() {
        let temporary = TempDir::new().expect("tempdir");
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let notify = origin.outbound_notify();
        let data = b"abcdefgh";

        let (request_id, session) = start_download(
            &mut origin,
            &local_node_id,
            &temporary.path().join("download.bin"),
            2,
            data,
            4,
        );
        expect_outbound_notification(&notify).await;

        origin
            .handle_response(
                &peer(),
                request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");
        expect_outbound_notification(&notify).await;

        origin
            .handle_chunk(
                &peer(),
                download_fragment_request_id(&operation(2), 0, 0),
                &transfer(2),
                &operation(2),
                0,
                0,
                0,
                &data[..4],
                NOW + 2,
            )
            .expect("first download chunk");
        expect_outbound_notification(&notify).await;

        assert!(origin.cancel(&transfer(2), &operation(2), NOW + 3));
        expect_outbound_notification(&notify).await;
        let (_, cancel_request) = take_request(&mut origin);
        assert!(matches!(cancel_request, FileNetworkRequest::Cancel { .. }));

        start_download(
            &mut origin,
            &local_node_id,
            &temporary.path().join("failed.bin"),
            3,
            data,
            4,
        );
        assert!(origin.fail_transfer(
            &transfer(3),
            &operation(3),
            NOW + 4,
            "file_transfer_send_failed",
        ));
        expect_outbound_notification(&notify).await;

        start_download(
            &mut origin,
            &local_node_id,
            &temporary.path().join("expired.bin"),
            5,
            data,
            4,
        );
        origin.expire(NOW + FILE_NETWORK_TRANSFER_TTL_MS);
        expect_outbound_notification(&notify).await;
    }

    #[tokio::test]
    async fn fail_pending_request_only_fails_the_correlated_operation_and_wakes_pump() {
        let temporary = TempDir::new().expect("tempdir");
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let notify = origin.outbound_notify();
        let (first_request_id, _) = start_download(
            &mut origin,
            &local_node_id,
            &temporary.path().join("first.bin"),
            2,
            b"first",
            4,
        );
        expect_outbound_notification(&notify).await;
        let (second_request_id, second_session) = start_download(
            &mut origin,
            &local_node_id,
            &temporary.path().join("second.bin"),
            3,
            b"second",
            4,
        );
        expect_outbound_notification(&notify).await;

        assert!(origin.fail_pending_request(
            first_request_id,
            NOW + 1,
            "file_transfer_authorization_lost",
        ));
        expect_outbound_notification(&notify).await;
        assert_eq!(origin.active_count(), 1);
        assert!(!origin.owns_download(&peer().node_id, &transfer(2), &operation(2)));
        assert!(origin.owns_download(&peer().node_id, &transfer(3), &operation(3)));
        assert!(!origin.fail_pending_request(
            first_request_id,
            NOW + 2,
            "file_transfer_authorization_lost",
        ));

        origin
            .handle_response(
                &peer(),
                second_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(second_session),
                },
                NOW + 3,
            )
            .expect("unrelated operation stays correlated");
        let failed = origin
            .drain_events()
            .into_iter()
            .find(|event| {
                event.operation_id == operation(2) && event.state == TransferState::Failed
            })
            .expect("failed event");
        assert_eq!(
            failed.error_code.as_deref(),
            Some("file_transfer_authorization_lost")
        );
    }

    #[test]
    fn upload_pause_waits_for_ack_and_resumes_at_the_next_chunk() {
        let temporary = TempDir::new().expect("tempdir");
        let source = temporary.path().join("source.bin");
        let data = vec![0x5a; 32];
        std::fs::write(&source, &data).expect("source");
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let (open_request_id, session) =
            start_upload(&mut origin, &local_node_id, &source, 2, &data, 16);
        origin
            .handle_response(
                &peer(),
                open_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");
        let _ = origin.drain_events();

        let (chunk_request_id, _, _, chunk_index, offset, fragment) = take_chunk(&mut origin);
        assert_eq!((chunk_index, offset, fragment.len()), (0, 0, 16));
        origin
            .pause(&transfer(2), &operation(2), NOW + 2)
            .expect("defer pause");
        assert!(origin.pop_outbound().is_none());

        origin
            .handle_response(
                &peer(),
                chunk_request_id,
                FileNetworkResponse::ChunkAccepted {
                    chunk_index,
                    chunk_complete: true,
                    progress: progress(TransferState::Active, 16, 32, 1, 2),
                },
                NOW + 3,
            )
            .expect("chunk acknowledgement");
        let (pause_request_id, pause_request) = take_request(&mut origin);
        assert!(matches!(pause_request, FileNetworkRequest::Pause { .. }));
        origin
            .handle_response(
                &peer(),
                pause_request_id,
                FileNetworkResponse::Progress {
                    progress: progress(TransferState::Paused, 16, 32, 1, 2),
                },
                NOW + 4,
            )
            .expect("pause response");
        assert_eq!(
            origin.drain_events().last().map(|event| event.state),
            Some(TransferState::Paused)
        );
        assert!(matches!(
            origin.pause(&transfer(2), &operation(2), NOW + 5),
            Err(FileNetworkOriginError::InvalidState)
        ));

        origin
            .resume(&transfer(2), &operation(2), NOW + 5)
            .expect("queue resume");
        let (resume_request_id, resume_request) = take_request(&mut origin);
        let FileNetworkRequest::Resume { request } = resume_request else {
            panic!("expected resume request");
        };
        assert_eq!(request.transfer_id, transfer(2));
        assert_eq!(request.operation_id, operation(2));
        assert_eq!(request.direction, TransferDirection::Upload);
        assert_eq!(request.content_hash, ContentHash::from_bytes(&data));
        assert!(matches!(
            origin.resume(&transfer(2), &operation(2), NOW + 6),
            Err(FileNetworkOriginError::InvalidState)
        ));

        origin
            .handle_response(
                &peer(),
                resume_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(active_session(
                        &local_node_id,
                        TransferDirection::Upload,
                        transfer(2),
                        operation(2),
                        manifest(&data, 16),
                    )),
                },
                NOW + 6,
            )
            .expect("resume response");
        let (_, _, _, chunk_index, offset, fragment) = take_chunk(&mut origin);
        assert_eq!((chunk_index, offset), (1, 16));
        assert_eq!(fragment, data[16..]);
    }

    #[test]
    fn upload_pause_rewinds_a_partially_sent_chunk() {
        let temporary = TempDir::new().expect("tempdir");
        let source = temporary.path().join("source.bin");
        let data = vec![0x6b; MAX_FILE_NETWORK_FRAGMENT_BYTES + 7];
        std::fs::write(&source, &data).expect("source");
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let (open_request_id, session) = start_upload(
            &mut origin,
            &local_node_id,
            &source,
            2,
            &data,
            MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES as u64,
        );
        origin
            .handle_response(
                &peer(),
                open_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");

        let (chunk_request_id, _, _, chunk_index, offset, fragment) = take_chunk(&mut origin);
        assert_eq!((chunk_index, offset), (0, 0));
        assert_eq!(fragment.len(), MAX_FILE_NETWORK_FRAGMENT_BYTES);
        origin
            .pause(&transfer(2), &operation(2), NOW + 2)
            .expect("defer pause");
        origin
            .handle_response(
                &peer(),
                chunk_request_id,
                FileNetworkResponse::ChunkAccepted {
                    chunk_index,
                    chunk_complete: false,
                    progress: progress(TransferState::Active, 0, data.len() as u64, 0, 1),
                },
                NOW + 3,
            )
            .expect("partial acknowledgement");
        let (pause_request_id, pause_request) = take_request(&mut origin);
        assert!(matches!(pause_request, FileNetworkRequest::Pause { .. }));
        origin
            .handle_response(
                &peer(),
                pause_request_id,
                FileNetworkResponse::Progress {
                    progress: progress(TransferState::Paused, 0, data.len() as u64, 0, 1),
                },
                NOW + 4,
            )
            .expect("pause response");

        origin
            .resume(&transfer(2), &operation(2), NOW + 5)
            .expect("queue resume");
        let (resume_request_id, resume_request) = take_request(&mut origin);
        match resume_request {
            FileNetworkRequest::Resume { request } => {
                assert_eq!(request.resume_chunk_index, 0);
                assert_eq!(request.resume_offset, 0);
            }
            other => panic!("expected resume request, got {other:?}"),
        }
        origin
            .handle_response(
                &peer(),
                resume_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(active_session(
                        &local_node_id,
                        TransferDirection::Upload,
                        transfer(2),
                        operation(2),
                        manifest(&data, MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES as u64),
                    )),
                },
                NOW + 6,
            )
            .expect("resume response");
        let (_, _, _, chunk_index, offset, fragment) = take_chunk(&mut origin);
        assert_eq!((chunk_index, offset), (0, 0));
        assert_eq!(fragment.len(), MAX_FILE_NETWORK_FRAGMENT_BYTES);
    }

    #[test]
    fn download_pause_resumes_the_same_partial_fragment() {
        let temporary = TempDir::new().expect("tempdir");
        let destination = temporary.path().join("download.bin");
        let data: Vec<u8> = (0..(MAX_FILE_NETWORK_FRAGMENT_BYTES + 19))
            .map(|index| (index % 251) as u8)
            .collect();
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let (open_request_id, session) = start_download(
            &mut origin,
            &local_node_id,
            &destination,
            2,
            &data,
            MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES as u64,
        );
        origin
            .handle_response(
                &peer(),
                open_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");
        origin
            .handle_chunk(
                &peer(),
                download_fragment_request_id(&operation(2), 0, 0),
                &transfer(2),
                &operation(2),
                0,
                0,
                0,
                &data[..MAX_FILE_NETWORK_FRAGMENT_BYTES],
                NOW + 2,
            )
            .expect("first fragment");
        assert!(!destination.exists());

        origin
            .pause(&transfer(2), &operation(2), NOW + 3)
            .expect("queue pause");
        let (pause_request_id, pause_request) = take_request(&mut origin);
        assert!(matches!(pause_request, FileNetworkRequest::Pause { .. }));
        origin
            .handle_response(
                &peer(),
                pause_request_id,
                FileNetworkResponse::Progress {
                    progress: progress(TransferState::Paused, 0, data.len() as u64, 0, 1),
                },
                NOW + 4,
            )
            .expect("pause response");
        origin
            .resume(&transfer(2), &operation(2), NOW + 5)
            .expect("queue resume");
        let (resume_request_id, resume_request) = take_request(&mut origin);
        match resume_request {
            FileNetworkRequest::Resume { request } => {
                assert_eq!(request.resume_chunk_index, 0);
                assert_eq!(
                    request.resume_offset,
                    MAX_FILE_NETWORK_FRAGMENT_BYTES as u64
                );
                assert_eq!(request.resume_generation, 1);
            }
            other => panic!("expected resume request, got {other:?}"),
        }
        origin
            .handle_response(
                &peer(),
                resume_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(active_session(
                        &local_node_id,
                        TransferDirection::Download,
                        transfer(2),
                        operation(2),
                        manifest(&data, MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES as u64),
                    )),
                },
                NOW + 6,
            )
            .expect("resume response");
        origin
            .handle_chunk(
                &peer(),
                download_fragment_request_id(
                    &operation(2),
                    0,
                    MAX_FILE_NETWORK_FRAGMENT_BYTES as u64,
                ),
                &transfer(2),
                &operation(2),
                0,
                0,
                MAX_FILE_NETWORK_FRAGMENT_BYTES as u64,
                &data[MAX_FILE_NETWORK_FRAGMENT_BYTES..],
                NOW + 7,
            )
            .expect("stale generation is ignored");
        assert!(!destination.exists());
        origin
            .handle_chunk(
                &peer(),
                download_fragment_request_id(
                    &operation(2),
                    0,
                    MAX_FILE_NETWORK_FRAGMENT_BYTES as u64,
                ),
                &transfer(2),
                &operation(2),
                1,
                0,
                MAX_FILE_NETWORK_FRAGMENT_BYTES as u64,
                &data[MAX_FILE_NETWORK_FRAGMENT_BYTES..],
                NOW + 8,
            )
            .expect("second fragment");
        let (finish_request_id, finish_request) = take_request(&mut origin);
        assert!(matches!(finish_request, FileNetworkRequest::Finish { .. }));
        origin
            .handle_response(
                &peer(),
                finish_request_id,
                FileNetworkResponse::Completed {
                    progress: progress(
                        TransferState::Completed,
                        data.len() as u64,
                        data.len() as u64,
                        1,
                        1,
                    ),
                },
                NOW + 9,
            )
            .expect("finish response");
        assert_eq!(std::fs::read(&destination).expect("download"), data);
        assert_eq!(origin.active_count(), 0);
    }

    #[test]
    fn multi_chunk_download_resume_verifies_and_publishes_content() {
        let temporary = TempDir::new().expect("tempdir");
        let destination = temporary.path().join("download.bin");
        let data: Vec<u8> =
            (0..(MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES + MAX_FILE_NETWORK_FRAGMENT_BYTES + 19))
                .map(|index| (index % 251) as u8)
                .collect();
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let (open_request_id, session) = start_download(
            &mut origin,
            &local_node_id,
            &destination,
            2,
            &data,
            MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES as u64,
        );
        origin
            .handle_response(
                &peer(),
                open_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");

        for (fragment_index, fragment) in data[..MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES]
            .chunks(MAX_FILE_NETWORK_FRAGMENT_BYTES)
            .enumerate()
        {
            let offset = fragment_index * MAX_FILE_NETWORK_FRAGMENT_BYTES;
            origin
                .handle_chunk(
                    &peer(),
                    download_fragment_request_id(&operation(2), 0, offset as u64),
                    &transfer(2),
                    &operation(2),
                    0,
                    0,
                    offset as u64,
                    fragment,
                    NOW + 2,
                )
                .expect("first chunk fragment");
        }
        assert!(!destination.exists());

        origin
            .pause(&transfer(2), &operation(2), NOW + 3)
            .expect("queue pause after first chunk");
        let (pause_request_id, pause_request) = take_request(&mut origin);
        assert!(matches!(pause_request, FileNetworkRequest::Pause { .. }));
        origin
            .handle_response(
                &peer(),
                pause_request_id,
                FileNetworkResponse::Progress {
                    progress: progress(
                        TransferState::Paused,
                        MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES as u64,
                        data.len() as u64,
                        1,
                        2,
                    ),
                },
                NOW + 4,
            )
            .expect("pause response");

        origin
            .resume(&transfer(2), &operation(2), NOW + 5)
            .expect("queue resume");
        let (resume_request_id, _) = take_request(&mut origin);
        let mut resumed_session = active_session(
            &local_node_id,
            TransferDirection::Download,
            transfer(2),
            operation(2),
            manifest(&data, MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES as u64),
        );
        resumed_session
            .record_chunk(0, &data[..MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES], NOW + 6)
            .expect("record completed first chunk");
        origin
            .handle_response(
                &peer(),
                resume_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(resumed_session),
                },
                NOW + 6,
            )
            .expect("resume response");

        let second_offset = MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES as u64;
        for (fragment_index, fragment) in data[MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES..]
            .chunks(MAX_FILE_NETWORK_FRAGMENT_BYTES)
            .enumerate()
        {
            let offset = second_offset + (fragment_index * MAX_FILE_NETWORK_FRAGMENT_BYTES) as u64;
            origin
                .handle_chunk(
                    &peer(),
                    download_fragment_request_id(&operation(2), 1, offset),
                    &transfer(2),
                    &operation(2),
                    1,
                    1,
                    offset,
                    fragment,
                    NOW + 7,
                )
                .expect("second chunk fragment");
        }
        let (finish_request_id, finish_request) = take_request(&mut origin);
        assert!(matches!(finish_request, FileNetworkRequest::Finish { .. }));
        origin
            .handle_response(
                &peer(),
                finish_request_id,
                FileNetworkResponse::Completed {
                    progress: progress(
                        TransferState::Completed,
                        data.len() as u64,
                        data.len() as u64,
                        2,
                        2,
                    ),
                },
                NOW + 8,
            )
            .expect("finish response");

        assert!(
            ContentHash::from_bytes(&data)
                == manifest(&data, MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES as u64).content_hash
        );
        assert_eq!(std::fs::read(&destination).expect("download"), data);
        assert_eq!(origin.active_count(), 0);
    }

    #[test]
    fn repeated_pause_and_resume_use_fresh_request_ids() {
        let temporary = TempDir::new().expect("tempdir");
        let destination = temporary.path().join("download.bin");
        let data = b"repeat pause";
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let (open_request_id, session) =
            start_download(&mut origin, &local_node_id, &destination, 2, data, 32);
        origin
            .handle_response(
                &peer(),
                open_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");

        origin
            .pause(&transfer(2), &operation(2), NOW + 2)
            .expect("first pause");
        let (first_pause_id, _) = take_request(&mut origin);
        origin
            .handle_response(
                &peer(),
                first_pause_id,
                FileNetworkResponse::Progress {
                    progress: progress(TransferState::Paused, 0, data.len() as u64, 0, 1),
                },
                NOW + 3,
            )
            .expect("first pause response");
        origin
            .resume(&transfer(2), &operation(2), NOW + 4)
            .expect("first resume");
        let (first_resume_id, first_resume) = take_request(&mut origin);
        let FileNetworkRequest::Resume {
            request: first_resume,
        } = first_resume
        else {
            panic!("expected first resume request");
        };
        assert_eq!(first_resume.resume_generation, 1);
        origin
            .handle_response(
                &peer(),
                first_resume_id,
                FileNetworkResponse::Opened {
                    session: Box::new(active_session(
                        &local_node_id,
                        TransferDirection::Download,
                        transfer(2),
                        operation(2),
                        manifest(data, 32),
                    )),
                },
                NOW + 5,
            )
            .expect("first resume response");

        origin
            .pause(&transfer(2), &operation(2), NOW + 6)
            .expect("second pause");
        let (second_pause_id, _) = take_request(&mut origin);
        origin
            .handle_response(
                &peer(),
                second_pause_id,
                FileNetworkResponse::Progress {
                    progress: progress(TransferState::Paused, 0, data.len() as u64, 0, 1),
                },
                NOW + 7,
            )
            .expect("second pause response");
        origin
            .resume(&transfer(2), &operation(2), NOW + 8)
            .expect("second resume");
        let (second_resume_id, second_resume) = take_request(&mut origin);
        let FileNetworkRequest::Resume {
            request: second_resume,
        } = second_resume
        else {
            panic!("expected second resume request");
        };
        assert_eq!(second_resume.resume_generation, 2);

        assert_ne!(first_pause_id, second_pause_id);
        assert_ne!(first_resume_id, second_resume_id);
        assert_ne!(first_pause_id, first_resume_id);
        assert_ne!(second_pause_id, second_resume_id);
    }

    #[test]
    fn failed_resume_request_build_does_not_consume_generation() {
        let temporary = TempDir::new().expect("tempdir");
        let destination = temporary.path().join("download.bin");
        let data = b"generation must not advance";
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let (open_request_id, session) =
            start_download(&mut origin, &local_node_id, &destination, 2, data, 32);
        origin
            .handle_response(
                &peer(),
                open_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");
        origin
            .pause(&transfer(2), &operation(2), NOW + 2)
            .expect("queue pause");
        let (pause_request_id, _) = take_request(&mut origin);
        origin
            .handle_response(
                &peer(),
                pause_request_id,
                FileNetworkResponse::Progress {
                    progress: progress(TransferState::Paused, 0, data.len() as u64, 0, 1),
                },
                NOW + 3,
            )
            .expect("pause response");

        let key = OriginTransferKey::new(&transfer(2), &operation(2));
        let paused = origin.transfers.get_mut(&key).expect("paused transfer");
        assert_eq!(paused.resume_generation, 0);
        paused.next_request_sequence = u64::MAX;

        assert!(matches!(
            origin.resume(&transfer(2), &operation(2), NOW + 4),
            Err(FileNetworkOriginError::InvalidRequest)
        ));
        let paused = origin.transfers.get(&key).expect("paused transfer");
        assert_eq!(paused.resume_generation, 0);
        assert!(paused.pending.is_none());
        assert!(origin.pop_outbound().is_none());
    }

    #[test]
    fn paused_transfer_expires_and_cleans_staging() {
        let temporary = TempDir::new().expect("tempdir");
        let destination = temporary.path().join("download.bin");
        let data = b"paused content";
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let (open_request_id, session) =
            start_download(&mut origin, &local_node_id, &destination, 2, data, 32);
        origin
            .handle_response(
                &peer(),
                open_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");
        origin
            .pause(&transfer(2), &operation(2), NOW + 2)
            .expect("queue pause");
        let (pause_request_id, _) = take_request(&mut origin);
        origin
            .handle_response(
                &peer(),
                pause_request_id,
                FileNetworkResponse::Progress {
                    progress: progress(TransferState::Paused, 0, data.len() as u64, 0, 1),
                },
                NOW + 3,
            )
            .expect("pause response");

        origin.expire(NOW + FILE_NETWORK_TRANSFER_TTL_MS);
        assert_eq!(origin.active_count(), 0);
        let events = origin.drain_events();
        assert!(events
            .iter()
            .any(|event| event.state == TransferState::Expired));
        assert!(!destination.exists());
        assert_eq!(
            std::fs::read_dir(temporary.path())
                .expect("read dir")
                .count(),
            0
        );
    }

    #[test]
    fn resume_response_with_a_different_binding_fails_closed() {
        let temporary = TempDir::new().expect("tempdir");
        let destination = temporary.path().join("download.bin");
        let data = b"bound content";
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let (open_request_id, session) =
            start_download(&mut origin, &local_node_id, &destination, 2, data, 32);
        origin
            .handle_response(
                &peer(),
                open_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");
        origin
            .pause(&transfer(2), &operation(2), NOW + 2)
            .expect("queue pause");
        let (pause_request_id, _) = take_request(&mut origin);
        origin
            .handle_response(
                &peer(),
                pause_request_id,
                FileNetworkResponse::Progress {
                    progress: progress(TransferState::Paused, 0, data.len() as u64, 0, 1),
                },
                NOW + 3,
            )
            .expect("pause response");
        origin
            .resume(&transfer(2), &operation(2), NOW + 4)
            .expect("queue resume");
        let (resume_request_id, _) = take_request(&mut origin);
        let mut rebound = active_session(
            &local_node_id,
            TransferDirection::Download,
            transfer(2),
            operation(2),
            manifest(data, 32),
        );
        rebound.binding.path = SharedPath::parse("docs/other.bin").expect("path");
        assert!(matches!(
            origin.handle_response(
                &peer(),
                resume_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(rebound),
                },
                NOW + 5,
            ),
            Err(FileNetworkOriginError::BindingMismatch)
        ));
        assert_eq!(origin.active_count(), 0);
        assert!(matches!(
            take_request(&mut origin).1,
            FileNetworkRequest::Cancel { .. }
        ));
        assert!(!destination.exists());
        assert_eq!(
            std::fs::read_dir(temporary.path())
                .expect("read dir")
                .count(),
            0
        );
    }

    #[test]
    fn resume_response_with_a_different_download_manifest_fails_closed() {
        let temporary = TempDir::new().expect("tempdir");
        let destination = temporary.path().join("download.bin");
        let data = b"manifest-bound content";
        let local_node_id = node(4);
        let mut origin = FileNetworkOrigin::new(local_node_id.clone());
        let (open_request_id, session) =
            start_download(&mut origin, &local_node_id, &destination, 2, data, 32);
        let expected_binding = session.binding.clone();
        origin
            .handle_response(
                &peer(),
                open_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");
        origin
            .pause(&transfer(2), &operation(2), NOW + 2)
            .expect("queue pause");
        let (pause_request_id, _) = take_request(&mut origin);
        origin
            .handle_response(
                &peer(),
                pause_request_id,
                FileNetworkResponse::Progress {
                    progress: progress(TransferState::Paused, 0, data.len() as u64, 0, 1),
                },
                NOW + 3,
            )
            .expect("pause response");
        origin
            .resume(&transfer(2), &operation(2), NOW + 4)
            .expect("queue resume");
        let (resume_request_id, _) = take_request(&mut origin);
        let rebound = active_session(
            &local_node_id,
            TransferDirection::Download,
            transfer(2),
            operation(2),
            manifest(data, 16),
        );
        assert_eq!(rebound.binding, expected_binding);
        assert!(matches!(
            origin.handle_response(
                &peer(),
                resume_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(rebound),
                },
                NOW + 5,
            ),
            Err(FileNetworkOriginError::BindingMismatch)
        ));
        assert_eq!(origin.active_count(), 0);
        assert!(!destination.exists());
    }

    #[test]
    fn paused_upload_is_restored_and_resends_the_next_chunk() {
        let temporary = TempDir::new().expect("tempdir");
        let source = temporary.path().join("source.bin");
        let data = b"0123456789abcdefghij";
        std::fs::write(&source, data).expect("source");
        let local_node_id = node(4);
        let store = FileOriginStore::at(temporary.path(), "network");
        let mut origin = FileNetworkOrigin::with_store(local_node_id.clone(), store.clone());
        let (open_request_id, session) =
            start_upload(&mut origin, &local_node_id, &source, 2, data, 8);
        origin
            .handle_response(
                &peer(),
                open_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");
        let (chunk_request_id, _, _, chunk_index, offset, _) = take_chunk(&mut origin);
        assert_eq!((chunk_index, offset), (0, 0));
        origin
            .pause(&transfer(2), &operation(2), NOW + 2)
            .expect("defer pause");
        origin
            .handle_response(
                &peer(),
                chunk_request_id,
                FileNetworkResponse::ChunkAccepted {
                    chunk_index,
                    chunk_complete: true,
                    progress: progress(TransferState::Active, 8, data.len() as u64, 1, 3),
                },
                NOW + 3,
            )
            .expect("chunk acknowledgement");
        let (pause_request_id, _) = take_request(&mut origin);
        origin
            .handle_response(
                &peer(),
                pause_request_id,
                FileNetworkResponse::Progress {
                    progress: progress(TransferState::Paused, 8, data.len() as u64, 1, 3),
                },
                NOW + 4,
            )
            .expect("pause response");
        drop(origin);
        assert_eq!(store.load().expect("journal").len(), 1);

        let mut restored = FileNetworkOrigin::with_store(local_node_id.clone(), store.clone());
        let events = restored.restore_paused(NOW + 5).expect("restore");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].state, TransferState::Paused);
        assert_eq!(restored.active_count(), 1);
        restored
            .resume(&transfer(2), &operation(2), NOW + 6)
            .expect("resume");
        let (resume_request_id, resume_request) = take_request(&mut restored);
        assert!(matches!(resume_request, FileNetworkRequest::Resume { .. }));
        restored
            .handle_response(
                &peer(),
                resume_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(active_session(
                        &local_node_id,
                        TransferDirection::Upload,
                        transfer(2),
                        operation(2),
                        manifest(data, 8),
                    )),
                },
                NOW + 7,
            )
            .expect("resume response");
        let (_, _, _, chunk_index, offset, fragment) = take_chunk(&mut restored);
        assert_eq!((chunk_index, offset), (1, 8));
        assert_eq!(fragment, data[8..16]);
        assert!(store.load().expect("journal").is_empty());
    }

    #[test]
    fn restored_upload_with_revoked_epoch_fails_closed_without_resume_frame() {
        let temporary = TempDir::new().expect("tempdir");
        let source = temporary.path().join("source.bin");
        let data = b"0123456789abcdefghij";
        std::fs::write(&source, data).expect("source");
        let local_node_id = node(4);
        let store = FileOriginStore::at(temporary.path(), "network");
        let mut origin = FileNetworkOrigin::with_store(local_node_id.clone(), store.clone());
        let (open_request_id, session) =
            start_upload(&mut origin, &local_node_id, &source, 2, data, 8);
        origin
            .handle_response(
                &peer(),
                open_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");
        let (chunk_request_id, _, _, chunk_index, _, _) = take_chunk(&mut origin);
        origin
            .pause(&transfer(2), &operation(2), NOW + 2)
            .expect("defer pause");
        origin
            .handle_response(
                &peer(),
                chunk_request_id,
                FileNetworkResponse::ChunkAccepted {
                    chunk_index,
                    chunk_complete: true,
                    progress: progress(TransferState::Active, 8, data.len() as u64, 1, 3),
                },
                NOW + 3,
            )
            .expect("chunk acknowledgement");
        let (pause_request_id, _) = take_request(&mut origin);
        origin
            .handle_response(
                &peer(),
                pause_request_id,
                FileNetworkResponse::Progress {
                    progress: progress(TransferState::Paused, 8, data.len() as u64, 1, 3),
                },
                NOW + 4,
            )
            .expect("pause response");
        drop(origin);

        let mut restored = FileNetworkOrigin::with_store(local_node_id, store.clone());
        let restored_events = restored.restore_paused(NOW + 5).expect("restore");
        assert_eq!(restored_events[0].state, TransferState::Paused);
        assert_eq!(
            restored
                .resume_scope(&transfer(2), &operation(2))
                .expect("resume scope"),
            (peer(), 7)
        );

        assert!(matches!(
            restored.resume_authorized(&transfer(2), &operation(2), 8, NOW + 6),
            Err(FileNetworkOriginError::AccessRevoked)
        ));
        assert!(restored.pop_outbound().is_none());
        assert_eq!(restored.active_count(), 0);
        assert!(store.load().expect("journal").is_empty());
        assert_eq!(std::fs::read(&source).expect("source remains"), data);
        let events = restored.drain_events();
        assert_eq!(
            events.last().map(|event| event.state),
            Some(TransferState::Failed)
        );
        assert_eq!(
            events.last().and_then(|event| event.error_code.as_deref()),
            Some("file_transfer_access_revoked")
        );
    }

    #[test]
    fn restored_download_with_revoked_epoch_removes_staging_without_resume_frame() {
        let temporary = TempDir::new().expect("tempdir");
        let data = b"restored download content";
        let (destination, local_node_id, store, staging_path) =
            paused_download_fixture(&temporary, data, 32, 5);
        assert!(staging_path.exists());

        let mut restored = FileNetworkOrigin::with_store(local_node_id, store.clone());
        let restored_events = restored.restore_paused(NOW + 5).expect("restore");
        assert_eq!(restored_events[0].state, TransferState::Paused);
        assert_eq!(restored.active_count(), 1);
        assert!(staging_path.exists());

        assert!(matches!(
            restored.resume_authorized(&transfer(2), &operation(2), 8, NOW + 6),
            Err(FileNetworkOriginError::AccessRevoked)
        ));
        assert!(restored.pop_outbound().is_none());
        assert_eq!(restored.active_count(), 0);
        assert!(!staging_path.exists());
        assert!(!destination.exists());
        assert!(store.load().expect("journal").is_empty());
        let events = restored.drain_events();
        assert_eq!(
            events.last().map(|event| event.state),
            Some(TransferState::Failed)
        );
        assert_eq!(
            events.last().and_then(|event| event.error_code.as_deref()),
            Some("file_transfer_access_revoked")
        );
    }

    #[test]
    fn restored_upload_rejects_source_growth_before_manifest_allocation() {
        let temporary = TempDir::new().expect("tempdir");
        let source = temporary.path().join("source.bin");
        let data = b"01234567";
        std::fs::write(&source, data).expect("source");
        let local_node_id = node(4);
        let store = FileOriginStore::at(temporary.path(), "network");
        let mut origin = FileNetworkOrigin::with_store(local_node_id.clone(), store.clone());
        let (open_request_id, session) =
            start_upload(&mut origin, &local_node_id, &source, 2, data, 4);
        origin
            .pause(&transfer(2), &operation(2), NOW + 1)
            .expect("pause before open completes");
        origin
            .handle_response(
                &peer(),
                open_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 2,
            )
            .expect("open response");
        let (pause_request_id, request) = take_request(&mut origin);
        assert!(matches!(request, FileNetworkRequest::Pause { .. }));
        origin
            .handle_response(
                &peer(),
                pause_request_id,
                FileNetworkResponse::Progress {
                    progress: progress(TransferState::Paused, 0, data.len() as u64, 0, 2),
                },
                NOW + 3,
            )
            .expect("pause response");
        drop(origin);
        assert_eq!(store.load().expect("journal").len(), 1);
        OpenOptions::new()
            .write(true)
            .open(&source)
            .expect("open source")
            .set_len(MAX_FILE_NETWORK_TRANSFER_BYTES + 1)
            .expect("grow source");

        let mut restored = FileNetworkOrigin::with_store(local_node_id, store.clone());
        let events = restored.restore_paused(NOW + 4).expect("restore");
        assert_eq!(restored.active_count(), 0);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].state, TransferState::Failed);
        assert_eq!(
            events[0].error_code.as_deref(),
            Some("file_transfer_invalid_request")
        );
        assert!(store.load().expect("journal").is_empty());
    }

    #[test]
    fn paused_download_restores_a_partial_fragment_from_staging() {
        let temporary = TempDir::new().expect("tempdir");
        let destination = temporary.path().join("download.bin");
        let data = b"restart-partial";
        let local_node_id = node(4);
        let store = FileOriginStore::at(temporary.path(), "network");
        let mut origin = FileNetworkOrigin::with_store(local_node_id.clone(), store.clone());
        let (open_request_id, session) =
            start_download(&mut origin, &local_node_id, &destination, 2, data, 32);
        origin
            .handle_response(
                &peer(),
                open_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");
        origin
            .handle_chunk(
                &peer(),
                download_fragment_request_id(&operation(2), 0, 0),
                &transfer(2),
                &operation(2),
                0,
                0,
                0,
                &data[..5],
                NOW + 2,
            )
            .expect("partial fragment");
        origin
            .pause(&transfer(2), &operation(2), NOW + 3)
            .expect("pause");
        let (pause_request_id, _) = take_request(&mut origin);
        origin
            .handle_response(
                &peer(),
                pause_request_id,
                FileNetworkResponse::Progress {
                    progress: progress(TransferState::Paused, 0, data.len() as u64, 0, 1),
                },
                NOW + 4,
            )
            .expect("pause response");
        drop(origin);

        let mut restored = FileNetworkOrigin::with_store(local_node_id.clone(), store.clone());
        let events = restored.restore_paused(NOW + 5).expect("restore");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].state, TransferState::Paused);
        restored
            .resume(&transfer(2), &operation(2), NOW + 6)
            .expect("resume");
        let (resume_request_id, _) = take_request(&mut restored);
        restored
            .handle_response(
                &peer(),
                resume_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(active_session(
                        &local_node_id,
                        TransferDirection::Download,
                        transfer(2),
                        operation(2),
                        manifest(data, 32),
                    )),
                },
                NOW + 7,
            )
            .expect("resume response");
        restored
            .handle_chunk(
                &peer(),
                download_fragment_request_id(&operation(2), 0, 5),
                &transfer(2),
                &operation(2),
                1,
                0,
                5,
                &data[5..],
                NOW + 8,
            )
            .expect("remaining fragment");
        let (finish_request_id, finish_request) = take_request(&mut restored);
        assert!(matches!(finish_request, FileNetworkRequest::Finish { .. }));
        restored
            .handle_response(
                &peer(),
                finish_request_id,
                FileNetworkResponse::Completed {
                    progress: progress(
                        TransferState::Completed,
                        data.len() as u64,
                        data.len() as u64,
                        1,
                        1,
                    ),
                },
                NOW + 9,
            )
            .expect("finish");
        assert_eq!(std::fs::read(&destination).expect("download"), data);
        assert_eq!(restored.active_count(), 0);
        assert!(store.load().expect("journal").is_empty());
    }

    #[test]
    fn expired_restored_download_is_not_resumed_and_staging_is_removed() {
        let temporary = TempDir::new().expect("tempdir");
        let destination = temporary.path().join("download.bin");
        let data = b"expires";
        let local_node_id = node(4);
        let store = FileOriginStore::at(temporary.path(), "network");
        let mut origin = FileNetworkOrigin::with_store(local_node_id.clone(), store.clone());
        let (open_request_id, session) =
            start_download(&mut origin, &local_node_id, &destination, 2, data, 32);
        origin
            .handle_response(
                &peer(),
                open_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");
        origin
            .pause(&transfer(2), &operation(2), NOW + 2)
            .expect("pause");
        let (pause_request_id, _) = take_request(&mut origin);
        origin
            .handle_response(
                &peer(),
                pause_request_id,
                FileNetworkResponse::Progress {
                    progress: progress(TransferState::Paused, 0, data.len() as u64, 0, 1),
                },
                NOW + 3,
            )
            .expect("pause response");
        drop(origin);
        let staging_path = store.load().expect("journal")[0]
            .download
            .as_ref()
            .expect("download state")
            .staging_path
            .clone();
        assert!(staging_path.exists());

        let mut restored = FileNetworkOrigin::with_store(local_node_id, store.clone());
        let events = restored
            .restore_paused(NOW + FILE_NETWORK_TRANSFER_TTL_MS)
            .expect("restore");
        assert_eq!(restored.active_count(), 0);
        assert!(events
            .iter()
            .any(|event| event.state == TransferState::Expired));
        assert!(!staging_path.exists());
        assert!(store.load().expect("journal").is_empty());
    }

    #[test]
    fn restored_upload_fails_closed_when_the_source_changes() {
        let temporary = TempDir::new().expect("tempdir");
        let source = temporary.path().join("source.bin");
        let data = b"source-before";
        std::fs::write(&source, data).expect("source");
        let local_node_id = node(4);
        let store = FileOriginStore::at(temporary.path(), "network");
        let mut origin = FileNetworkOrigin::with_store(local_node_id.clone(), store.clone());
        let (open_request_id, session) =
            start_upload(&mut origin, &local_node_id, &source, 2, data, 4);
        origin
            .handle_response(
                &peer(),
                open_request_id,
                FileNetworkResponse::Opened {
                    session: Box::new(session),
                },
                NOW + 1,
            )
            .expect("open response");
        let (chunk_request_id, _, _, chunk_index, _, _) = take_chunk(&mut origin);
        origin
            .pause(&transfer(2), &operation(2), NOW + 2)
            .expect("defer pause");
        origin
            .handle_response(
                &peer(),
                chunk_request_id,
                FileNetworkResponse::ChunkAccepted {
                    chunk_index,
                    chunk_complete: true,
                    progress: progress(TransferState::Active, 4, data.len() as u64, 1, 4),
                },
                NOW + 3,
            )
            .expect("acknowledgement");
        let (pause_request_id, _) = take_request(&mut origin);
        origin
            .handle_response(
                &peer(),
                pause_request_id,
                FileNetworkResponse::Progress {
                    progress: progress(TransferState::Paused, 4, data.len() as u64, 1, 4),
                },
                NOW + 4,
            )
            .expect("pause response");
        drop(origin);
        std::fs::write(&source, b"source-after!").expect("modify source");

        let mut restored = FileNetworkOrigin::with_store(local_node_id, store.clone());
        let events = restored.restore_paused(NOW + 5).expect("restore");
        assert_eq!(restored.active_count(), 0);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].state, TransferState::Failed);
        assert_eq!(
            events[0].error_code.as_deref(),
            Some("file_transfer_source_invalid")
        );
        assert!(store.load().expect("journal").is_empty());
    }

    #[test]
    fn restored_download_rejects_an_existing_destination_without_overwriting_it() {
        let temporary = TempDir::new().expect("tempdir");
        let data = b"download-data";
        let (destination, local_node_id, store, staging_path) =
            paused_download_fixture(&temporary, data, 32, 5);
        let existing = b"keep-existing";
        std::fs::write(&destination, existing).expect("existing destination");

        let mut restored = FileNetworkOrigin::with_store(local_node_id, store.clone());
        let events = restored.restore_paused(NOW + 5).expect("restore");

        assert_eq!(restored.active_count(), 0);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].state, TransferState::Failed);
        assert_eq!(
            events[0].error_code.as_deref(),
            Some("file_transfer_destination_invalid")
        );
        assert_eq!(std::fs::read(&destination).expect("destination"), existing);
        assert!(!staging_path.exists());
        assert!(store.load().expect("journal").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn restored_download_rejects_a_replaced_staging_symlink_without_touching_its_target() {
        use std::os::unix::fs::symlink;

        let temporary = TempDir::new().expect("tempdir");
        let data = b"download-data";
        let (_destination, local_node_id, store, staging_path) =
            paused_download_fixture(&temporary, data, 32, 5);
        let target = temporary.path().join("symlink-target.bin");
        let target_contents = b"do-not-touch";
        std::fs::write(&target, target_contents).expect("target");
        std::fs::remove_file(&staging_path).expect("remove staging");
        symlink(&target, &staging_path).expect("replace staging with symlink");

        let mut restored = FileNetworkOrigin::with_store(local_node_id, store.clone());
        let events = restored.restore_paused(NOW + 5).expect("restore");

        assert_eq!(restored.active_count(), 0);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].state, TransferState::Failed);
        assert_eq!(
            events[0].error_code.as_deref(),
            Some("file_transfer_destination_invalid")
        );
        assert!(!staging_path.exists());
        assert_eq!(std::fs::read(&target).expect("target"), target_contents);
        assert!(store.load().expect("journal").is_empty());
    }

    #[test]
    fn invalid_restored_staging_path_does_not_delete_an_unrelated_file() {
        let temporary = TempDir::new().expect("tempdir");
        let data = b"download-data";
        let (_, local_node_id, store, _) = paused_download_fixture(&temporary, data, 32, 5);
        let unrelated_directory = temporary.path().join("unrelated");
        std::fs::create_dir(&unrelated_directory).expect("unrelated directory");
        let unrelated = unrelated_directory.join(".fkfile-download-not-ours");
        let unrelated_contents = b"unrelated";
        std::fs::write(&unrelated, unrelated_contents).expect("unrelated file");
        let mut entry = store.load().expect("journal").remove(0);
        entry
            .download
            .as_mut()
            .expect("download state")
            .staging_path = unrelated.clone();
        store.save(&[entry]).expect("save mutated journal");

        let mut restored = FileNetworkOrigin::with_store(local_node_id, store.clone());
        let events = restored.restore_paused(NOW + 5).expect("restore");

        assert_eq!(restored.active_count(), 0);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].state, TransferState::Failed);
        assert_eq!(
            events[0].error_code.as_deref(),
            Some("file_transfer_destination_invalid")
        );
        assert_eq!(
            std::fs::read(&unrelated).expect("unrelated file"),
            unrelated_contents
        );
        assert!(store.load().expect("journal").is_empty());
    }

    #[test]
    fn duplicate_restored_transfer_does_not_remove_the_restored_staging_file() {
        let temporary = TempDir::new().expect("tempdir");
        let data = b"download-data";
        let (_, local_node_id, store, staging_path) =
            paused_download_fixture(&temporary, data, 32, 5);
        let entry = store.load().expect("journal").remove(0);
        store
            .save(&[entry.clone(), entry])
            .expect("save duplicate journal");

        let mut restored = FileNetworkOrigin::with_store(local_node_id, store.clone());
        let events = restored.restore_paused(NOW + 5).expect("restore");

        assert_eq!(restored.active_count(), 1);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].state, TransferState::Paused);
        assert_eq!(events[1].state, TransferState::Failed);
        assert_eq!(
            events[1].error_code.as_deref(),
            Some("file_transfer_operation_active")
        );
        assert!(staging_path.exists());
        assert_eq!(store.load().expect("journal").len(), 1);
    }

    #[test]
    fn shared_restored_staging_path_does_not_remove_or_reuse_the_active_file() {
        let temporary = TempDir::new().expect("tempdir");
        let data = b"download-data";
        let (_, local_node_id, store, staging_path) =
            paused_download_fixture(&temporary, data, 32, 5);
        let first = store.load().expect("journal").remove(0);
        let mut second = first.clone();
        second.transfer_id = transfer(3);
        second.operation_id = operation(3);
        second.binding.transfer_id = transfer(3);
        second.binding.operation_id = operation(3);
        store.save(&[first, second]).expect("save aliased journal");

        let mut restored = FileNetworkOrigin::with_store(local_node_id, store.clone());
        let events = restored.restore_paused(NOW + 5).expect("restore");

        assert_eq!(restored.active_count(), 1);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].state, TransferState::Paused);
        assert_eq!(events[1].state, TransferState::Failed);
        assert_eq!(
            events[1].error_code.as_deref(),
            Some("file_transfer_operation_active")
        );
        assert!(staging_path.exists());
        assert_eq!(store.load().expect("journal").len(), 1);
    }
}
