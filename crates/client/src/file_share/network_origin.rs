//! Requester-side File transfer state machine.
//!
//! The owner-side data plane authenticates incoming requests and writes or
//! reads the shared root. This module drives the other half of the same wire
//! protocol: it builds local upload manifests, sends bounded fragments, stages
//! downloads, and accepts only responses and chunks that match the operation's
//! authenticated peer and exact request id.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::Ipv6Addr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fuckipv6_common::domain::{
    ContentHash, FileChunk, FileId, FileShareId, FileTransferError, NodeId, OperationId,
    ResumeRequest, SharedPath, TransferBinding, TransferDirection, TransferId, TransferManifest,
    TransferSession, TransferState,
};
use tempfile::NamedTempFile;
use thiserror::Error;
use tokio::sync::Notify;

use super::network::{
    download_fragment_request_id, FileNetworkFrame, FileNetworkProgress, FileNetworkRequest,
    FileNetworkResponse, MAX_FILE_NETWORK_FRAGMENT_BYTES, MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES,
};

/// Match the owner's transfer lifetime. Both ends must reject an operation at
/// the same absolute boundary even if one process was suspended.
pub(crate) const FILE_NETWORK_TRANSFER_TTL_MS: u64 = 30 * 60 * 1_000;
const MAX_ORIGIN_TRANSFERS: usize = 16;
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
    #[error("File download chunk is out of order, out of range or replayed")]
    ChunkOutOfOrder,
    #[error("File download chunk checksum or length is invalid")]
    ChunkCorrupt,
    #[error("File transfer filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("File transfer session operation failed: {0}")]
    Transfer(#[from] FileTransferError),
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
            Self::ChunkOutOfOrder => "file_transfer_chunk_out_of_order",
            Self::ChunkCorrupt => "file_transfer_chunk_corrupt",
            Self::Io(_) => "file_transfer_io",
            Self::Transfer(error) => error.code(),
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
    binding: Option<TransferBinding>,
    paused: bool,
    pause_after_reply: bool,
    next_request_sequence: u64,
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
    manifest: TransferManifest,
    next_chunk: u32,
    next_offset: u64,
    completed_chunks: u32,
}

#[derive(Debug)]
struct DownloadOriginState {
    destination: PathBuf,
    session: Option<Box<TransferSession>>,
    staging: Option<NamedTempFile>,
    assembly: Option<DownloadAssembly>,
    hasher: blake3::Hasher,
    received_bytes: u64,
    received_chunks: u32,
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
}

impl FileNetworkOrigin {
    pub(crate) fn new(local_node_id: NodeId) -> Self {
        Self {
            local_node_id,
            transfers: std::collections::BTreeMap::new(),
            outbound: VecDeque::new(),
            events: VecDeque::new(),
            outbound_notify: Arc::new(Notify::new()),
        }
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
            binding: None,
            paused: false,
            pause_after_reply: false,
            next_request_sequence: 0,
            state: OriginTransferState::Upload(UploadOriginState {
                source,
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
            pending: Some(PendingOriginReply {
                request_id,
                kind: OriginRequestKind::OpenDownload,
            }),
            binding: None,
            paused: false,
            pause_after_reply: false,
            next_request_sequence: 1,
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
            retry_after_ms: _,
        } = &response
        {
            debug_assert!(!code.is_empty());
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
        let pending = self
            .transfers
            .get_mut(&key)
            .ok_or(FileNetworkOriginError::UnknownResponse)?
            .pending
            .take()
            .expect("pending response exists");
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
                self.apply_pause(&key, progress)
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

    /// Reauthorize and continue a paused operation using the exact binding
    /// returned when the owner first opened it.
    pub(crate) fn resume(
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
        if !transfer.paused || transfer.pending.is_some() || transfer.pause_after_reply {
            return Err(FileNetworkOriginError::InvalidState);
        }
        queue_resume(transfer, &mut self.outbound)?;
        self.outbound_notify.notify_one();
        Ok(())
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
        let staging = NamedTempFile::new_in(parent)?;
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
            transfer.paused = false;
            queue_next_upload_action(transfer, &mut self.outbound)?;
            transfer.event(TransferState::Active, None)
        };
        self.push_event(event);
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
            transfer.binding = Some(session.binding.clone());
            transfer.deadline_unix_ms = transfer
                .deadline_unix_ms
                .min(session.binding.expires_at_unix_ms);
            transfer.paused = false;
            download.session = Some(Box::new(session));
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
        Ok(())
    }

    fn apply_pause(
        &mut self,
        key: &OriginTransferKey,
        progress: FileNetworkProgress,
    ) -> Result<(), FileNetworkOriginError> {
        let transfer = self
            .transfers
            .get_mut(key)
            .ok_or(FileNetworkOriginError::UnknownResponse)?;
        validate_pause(transfer, &progress)?;
        if let OriginTransferState::Upload(upload) = &mut transfer.state {
            // The owner discards a partially assembled upload chunk on pause.
            // Rewind that chunk so resume starts at an offset the owner accepts.
            upload.next_offset = 0;
        }
        transfer.paused = true;
        transfer.pause_after_reply = false;
        let event = transfer.event(TransferState::Paused, None);
        self.push_event(event);
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
                transfer.pending = Some(PendingOriginReply {
                    request_id,
                    kind: OriginRequestKind::FinishDownload,
                });
                self.outbound.push_back(OriginOutbound {
                    peer: transfer.peer.clone(),
                    transfer_id: transfer.transfer_id.clone(),
                    operation_id: transfer.operation_id.clone(),
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
        let _ = now_unix_ms;
        true
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
    transfer.pending = Some(PendingOriginReply {
        request_id,
        kind: OriginRequestKind::OpenUpload,
    });
    outbound.push_back(OriginOutbound {
        peer: transfer.peer.clone(),
        transfer_id: transfer.transfer_id.clone(),
        operation_id: transfer.operation_id.clone(),
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
        chunk_index,
        offset,
        data,
    )
    .map_err(|_| FileNetworkOriginError::InvalidRequest)?;
    transfer.pending = Some(PendingOriginReply {
        request_id,
        kind: OriginRequestKind::UploadChunk {
            chunk_index,
            offset,
            length,
        },
    });
    outbound.push_back(OriginOutbound {
        peer: transfer.peer.clone(),
        transfer_id: transfer.transfer_id.clone(),
        operation_id: transfer.operation_id.clone(),
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
    transfer.pending = Some(PendingOriginReply {
        request_id,
        kind: OriginRequestKind::FinishUpload,
    });
    outbound.push_back(OriginOutbound {
        peer: transfer.peer.clone(),
        transfer_id: transfer.transfer_id.clone(),
        operation_id: transfer.operation_id.clone(),
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
    transfer.pending = Some(PendingOriginReply {
        request_id,
        kind: OriginRequestKind::Pause,
    });
    outbound.push_back(OriginOutbound {
        peer: transfer.peer.clone(),
        transfer_id: transfer.transfer_id.clone(),
        operation_id: transfer.operation_id.clone(),
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
    };
    let label = match direction {
        TransferDirection::Upload => b"resume-upload".as_slice(),
        TransferDirection::Download => b"resume-download".as_slice(),
    };
    let request_id = next_origin_request_id(transfer, label, 0, 0, 0)?;
    let frame = FileNetworkFrame::request(request_id, FileNetworkRequest::Resume { request })
        .map_err(|_| FileNetworkOriginError::InvalidRequest)?;
    transfer.pending = Some(PendingOriginReply {
        request_id,
        kind: match direction {
            TransferDirection::Upload => OriginRequestKind::ResumeUpload,
            TransferDirection::Download => OriginRequestKind::ResumeDownload,
        },
    });
    outbound.push_back(OriginOutbound {
        peer: transfer.peer.clone(),
        transfer_id: transfer.transfer_id.clone(),
        operation_id: transfer.operation_id.clone(),
        frame,
    });
    Ok(())
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
            frame,
        });
    }
}

fn validate_opened_upload(
    transfer: &OriginTransfer,
    session: &TransferSession,
    local_node_id: &NodeId,
) -> Result<(), FileNetworkOriginError> {
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
    let chunk_end = chunk
        .offset
        .checked_add(chunk.length)
        .ok_or(FileNetworkOriginError::UnexpectedResponse)?;
    if length == 0
        || offset < chunk.offset
        || upload.next_offset != end
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
    let mut source = File::open(source_path)?;
    let metadata = source.metadata()?;
    if !metadata.is_file() {
        return Err(FileNetworkOriginError::InvalidSource);
    }
    let total_bytes = metadata.len();
    let mut hasher = blake3::Hasher::new();
    let mut chunks =
        Vec::with_capacity(usize::try_from(total_bytes.div_ceil(chunk_size)).unwrap_or_default());
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
    if std::fs::symlink_metadata(destination).is_ok() {
        return Err(FileNetworkOriginError::InvalidDestination);
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
        let outbound = origin.pop_outbound().expect("outbound chunk");
        match outbound.frame {
            FileNetworkFrame::Chunk {
                request_id,
                transfer_id,
                operation_id,
                chunk_index,
                offset,
                data,
            } => (
                request_id,
                transfer_id,
                operation_id,
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
        let (resume_request_id, _) = take_request(&mut origin);
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
        let (resume_request_id, _) = take_request(&mut origin);
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
                MAX_FILE_NETWORK_FRAGMENT_BYTES as u64,
                &data[MAX_FILE_NETWORK_FRAGMENT_BYTES..],
                NOW + 7,
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
                NOW + 8,
            )
            .expect("finish response");
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
        let (first_resume_id, _) = take_request(&mut origin);
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
        let (second_resume_id, _) = take_request(&mut origin);

        assert_ne!(first_pause_id, second_pause_id);
        assert_ne!(first_resume_id, second_resume_id);
        assert_ne!(first_pause_id, first_resume_id);
        assert_ne!(second_pause_id, second_resume_id);
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
}
