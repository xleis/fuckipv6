//! Versioned File application frames carried inside authenticated Secure
//! Sessions.
//!
//! Secure Session authenticates the peer and protects each payload, but it does
//! not distinguish File control/data frames from TUN IP packets. This module
//! supplies that demultiplexing boundary and keeps every decoded payload
//! strictly below the transport limit. Files are split into bounded transfer
//! chunks and then into bounded network fragments; the receiver must reassemble
//! no more than one transfer chunk at a time and must never buffer a complete
//! file in memory.

use fuckipv6_common::codec::{decode, encode};
use std::collections::BTreeMap;

use fuckipv6_common::domain::{
    AccessContext, CapabilityKind, ContentHash, FileShareId, MembershipRole, NetworkId,
    NetworkMemberDirectory, NetworkRole, NodeId, OperationId, ResumeRequest, SharedPath,
    TransferId, TransferManifest, TransferSession, TransferState,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::watch;

use super::{
    FileRuntime, FileRuntimeError, RuntimeUploadTransfer, TransferOperationCancellation,
    TransferProgress, UploadAdmission,
};

const FRAME_MAGIC: &[u8; 8] = b"FKFILE01";
const FRAME_VERSION: u8 = 1;

/// Secure Session uses a `u16` payload length and QUIC caps frames at 64 KiB.
/// Keep enough headroom for both transports and future AEAD overhead.
pub(crate) const MAX_FILE_NETWORK_FRAME_LEN: usize = 60 * 1024;

/// Binary fragment limit. A File domain chunk may be as large as 16 MiB, so the
/// network protocol deliberately chooses a smaller transport chunk and requires
/// bounded reassembly between fragments.
pub(crate) const MAX_FILE_NETWORK_FRAGMENT_BYTES: usize = 48 * 1024;

/// Remote transfer chunks are intentionally smaller than the domain maximum so
/// a manifest with checksums for the maximum 1 GiB file still fits in one frame.
pub(crate) const MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES: usize = 4 * 1024 * 1024;

const MAX_FILE_NETWORK_ERROR_CODE_LEN: usize = 96;
const MAX_ACTIVE_FILE_NETWORK_SESSIONS: usize = 16;
const FILE_NETWORK_TRANSFER_TTL_MS: u64 = 30 * 60 * 1_000;
const FILE_MEMBER_DIRECTORY_MAX_AGE_MS: u64 = 90_000;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub(crate) enum FileNetworkError {
    #[error("File network frame is too large")]
    FrameTooLarge,
    #[error("File network frame has an unknown prefix")]
    UnknownPrefix,
    #[error("File network frame version is unsupported")]
    UnsupportedVersion,
    #[error("File network frame is malformed")]
    Malformed,
    #[error("File network frame could not be encoded")]
    EncodeFailed,
    #[error("File network manifest is not valid for the remote transport")]
    InvalidManifest,
    #[error("File network fragment is empty or out of bounds")]
    InvalidFragment,
    #[error("File network error code is invalid")]
    InvalidErrorCode,
}

/// One peer-visible operation. The authenticated source node is never carried
/// in a request: the receiver derives it from the Secure Session route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum FileNetworkRequest {
    OpenUpload {
        share_id: FileShareId,
        transfer_id: TransferId,
        operation_id: OperationId,
        path: SharedPath,
        manifest: TransferManifest,
    },
    OpenDownload {
        share_id: FileShareId,
        transfer_id: TransferId,
        operation_id: OperationId,
        path: SharedPath,
    },
    Resume {
        request: ResumeRequest,
    },
    Pause {
        transfer_id: TransferId,
        operation_id: OperationId,
    },
    Cancel {
        transfer_id: TransferId,
        operation_id: OperationId,
    },
    Finish {
        transfer_id: TransferId,
        operation_id: OperationId,
    },
}

/// User-safe response values. Filesystem paths, OS errors, roots and peer
/// identities are represented only by stable codes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum FileNetworkResponse {
    Opened {
        session: Box<TransferSession>,
    },
    Progress {
        progress: FileNetworkProgress,
    },
    ChunkAccepted {
        chunk_index: u32,
        chunk_complete: bool,
        progress: FileNetworkProgress,
    },
    Completed {
        progress: FileNetworkProgress,
    },
    Error {
        code: String,
        retry_after_ms: Option<u64>,
    },
}

/// Progress projection safe to send over the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FileNetworkProgress {
    pub state: TransferState,
    pub transferred_bytes: u64,
    pub total_bytes: u64,
    pub received_chunks: u32,
    pub total_chunks: u32,
}

impl From<TransferProgress> for FileNetworkProgress {
    fn from(progress: TransferProgress) -> Self {
        Self {
            state: progress.state,
            transferred_bytes: progress.transferred_bytes,
            total_bytes: progress.total_bytes,
            received_chunks: progress.received_chunks,
            total_chunks: progress.total_chunks,
        }
    }
}

/// Framed File message. A Secure Session datagram contains exactly one frame;
/// trailing bytes are rejected so a mediator cannot smuggle a second command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum FileNetworkFrame {
    Request {
        request_id: [u8; 16],
        request: FileNetworkRequest,
    },
    Response {
        request_id: [u8; 16],
        response: FileNetworkResponse,
    },
    Chunk {
        request_id: [u8; 16],
        transfer_id: TransferId,
        operation_id: OperationId,
        chunk_index: u32,
        offset: u64,
        data: Vec<u8>,
    },
}

impl FileNetworkFrame {
    #[cfg(test)]
    pub(crate) fn request(
        request_id: [u8; 16],
        request: FileNetworkRequest,
    ) -> Result<Self, FileNetworkError> {
        if let FileNetworkRequest::OpenUpload { manifest, .. } = &request {
            validate_network_manifest(manifest)?;
        }
        Ok(Self::Request {
            request_id,
            request,
        })
    }

    pub(crate) fn response(
        request_id: [u8; 16],
        response: FileNetworkResponse,
    ) -> Result<Self, FileNetworkError> {
        if let FileNetworkResponse::Error {
            code,
            retry_after_ms,
        } = &response
        {
            if !valid_error_code(code) {
                return Err(FileNetworkError::InvalidErrorCode);
            }
            if retry_after_ms.is_some_and(|delay| delay > 86_400_000) {
                return Err(FileNetworkError::InvalidErrorCode);
            }
        }
        Ok(Self::Response {
            request_id,
            response,
        })
    }

    pub(crate) fn chunk(
        request_id: [u8; 16],
        transfer_id: TransferId,
        operation_id: OperationId,
        chunk_index: u32,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Self, FileNetworkError> {
        validate_fragment(offset, &data)?;
        Ok(Self::Chunk {
            request_id,
            transfer_id,
            operation_id,
            chunk_index,
            offset,
            data,
        })
    }
}

/// Authenticated upload sessions owned by one network runtime. A session is
/// identified by both transfer and operation IDs, and every chunk is checked
/// against the authenticated peer before it can reach the staging adapter.
pub(crate) struct FileNetworkDataPlane {
    runtime: FileRuntime,
    uploads: BTreeMap<NetworkSessionKey, ActiveUploadSession>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct NetworkSessionKey {
    transfer_id: String,
    operation_id: String,
}

impl NetworkSessionKey {
    fn new(transfer_id: &TransferId, operation_id: &OperationId) -> Self {
        Self {
            transfer_id: transfer_id.as_str().to_owned(),
            operation_id: operation_id.as_str().to_owned(),
        }
    }
}

struct ActiveUploadSession {
    actor: NodeId,
    member_access_epoch: u64,
    cancel: watch::Sender<bool>,
    transfer: RuntimeUploadTransfer,
    assembly: Option<ChunkAssembly>,
}

struct ChunkAssembly {
    chunk_index: u32,
    expected_offset: u64,
    expected_length: u64,
    expected_checksum: ContentHash,
    bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrustedFileMember {
    pub(crate) member_access_epoch: u64,
    pub(crate) context: AccessContext,
}

/// Resolve File authorization exclusively from the authoritative member
/// directory. The authenticated Secure Session supplies only the actor; role,
/// lifecycle, capabilities and the access epoch are never read from the frame.
pub(crate) fn trusted_file_member(
    expected_network_id: &NetworkId,
    directory: Option<&NetworkMemberDirectory>,
    received_at_unix_ms: Option<u64>,
    authenticated_actor: &NodeId,
    now_unix_ms: u64,
) -> Result<TrustedFileMember, FileNetworkResponse> {
    let directory =
        directory.ok_or_else(|| network_error("file_member_directory_unavailable", None))?;
    let received_at_unix_ms = received_at_unix_ms
        .ok_or_else(|| network_error("file_member_directory_unavailable", None))?;
    if directory.network_id != *expected_network_id
        || received_at_unix_ms > now_unix_ms
        || now_unix_ms.saturating_sub(received_at_unix_ms) > FILE_MEMBER_DIRECTORY_MAX_AGE_MS
    {
        return Err(network_error("file_member_directory_unavailable", None));
    }
    let member = directory
        .members
        .iter()
        .find(|member| &member.node_id == authenticated_actor)
        .ok_or_else(|| network_error("file_member_unavailable", None))?;
    if !member.lifecycle_state.grants_network_access() {
        return Err(network_error("file_membership_inactive", None));
    }
    if member.access_epoch == 0 {
        return Err(network_error("file_member_epoch_invalid", None));
    }
    if !member
        .capabilities
        .as_ref()
        .is_some_and(|capabilities| capabilities.contains(&CapabilityKind::File))
    {
        return Err(network_error("file_capability_unavailable", None));
    }
    let role = match member.role {
        MembershipRole::Owner => NetworkRole::Owner,
        MembershipRole::Administrator => NetworkRole::Administrator,
        MembershipRole::Member => NetworkRole::Member,
    };
    Ok(TrustedFileMember {
        member_access_epoch: member.access_epoch,
        context: AccessContext {
            network_id: directory.network_id.clone(),
            actor: authenticated_actor.clone(),
            role,
            // The member directory exposes built-in role groups, not the
            // authoritative dynamic/manual GroupId catalog. Leaving this empty
            // makes group-scoped File ACL rules fail closed.
            groups: Default::default(),
            membership_active: member.lifecycle_state.grants_network_access(),
            recent_reauthentication: false,
            emergency_coordinator: false,
        },
    })
}

impl FileNetworkDataPlane {
    pub(crate) fn new(runtime: FileRuntime) -> Self {
        Self {
            runtime,
            uploads: BTreeMap::new(),
        }
    }

    pub(crate) fn session_count(&self) -> usize {
        self.uploads.len()
    }

    /// Handle one authenticated request. The request ID is deliberately not
    /// accepted here: it belongs to the frame transport, while this boundary
    /// owns only operation state.
    pub(crate) fn handle_request(
        &mut self,
        actor: &NodeId,
        context: &AccessContext,
        network_enabled: bool,
        member_access_epoch: u64,
        request: FileNetworkRequest,
        now_unix_ms: u64,
    ) -> FileNetworkResponse {
        match request {
            FileNetworkRequest::OpenUpload {
                share_id,
                transfer_id,
                operation_id,
                path,
                manifest,
            } => self.open_upload(
                actor,
                context,
                network_enabled,
                member_access_epoch,
                share_id,
                transfer_id,
                operation_id,
                path,
                manifest,
                now_unix_ms,
            ),
            FileNetworkRequest::OpenDownload { .. } => {
                network_error("file_download_runtime_unavailable", None)
            }
            FileNetworkRequest::Resume { request } => self.resume(
                actor,
                member_access_epoch,
                context,
                network_enabled,
                request,
                now_unix_ms,
            ),
            FileNetworkRequest::Pause {
                transfer_id,
                operation_id,
            } => self.pause(
                actor,
                member_access_epoch,
                &transfer_id,
                &operation_id,
                now_unix_ms,
            ),
            FileNetworkRequest::Cancel {
                transfer_id,
                operation_id,
            } => self.cancel(
                actor,
                member_access_epoch,
                &transfer_id,
                &operation_id,
                now_unix_ms,
            ),
            FileNetworkRequest::Finish {
                transfer_id,
                operation_id,
            } => self.finish(
                actor,
                member_access_epoch,
                &transfer_id,
                &operation_id,
                now_unix_ms,
            ),
        }
    }

    /// Feed one structurally valid 48 KiB network fragment into the one
    /// assembly slot bound to this upload. Gaps, overlaps and cross-chunk
    /// fragments fail closed and cancel the transfer.
    pub(crate) fn handle_chunk(
        &mut self,
        actor: &NodeId,
        member_access_epoch: u64,
        transfer_id: &TransferId,
        operation_id: &OperationId,
        chunk_index: u32,
        offset: u64,
        data: &[u8],
        now_unix_ms: u64,
    ) -> Result<(bool, FileNetworkProgress), FileNetworkResponse> {
        let key = NetworkSessionKey::new(transfer_id, operation_id);
        self.ensure_session_authorized(&key, actor, member_access_epoch, now_unix_ms)?;
        let session = self
            .uploads
            .get_mut(&key)
            .ok_or_else(|| network_error("file_transfer_session_unavailable", None))?;
        let manifest = &session.transfer.session().manifest;
        let Some(expected) = manifest.chunks.get(chunk_index as usize) else {
            session.cancel.send_replace(true);
            self.uploads.remove(&key);
            return Err(network_error("file_transfer_chunk_out_of_range", None));
        };
        if expected.index != chunk_index {
            session.cancel.send_replace(true);
            self.uploads.remove(&key);
            return Err(network_error("file_transfer_chunk_out_of_range", None));
        }
        let expected_end = expected.offset.saturating_add(expected.length);
        let incoming_end = offset.saturating_add(data.len() as u64);
        if incoming_end > expected_end {
            session.cancel.send_replace(true);
            self.uploads.remove(&key);
            return Err(network_error("file_transfer_chunk_corrupt", None));
        }

        let assembly = match session.assembly.as_mut() {
            Some(assembly) if assembly.chunk_index == chunk_index => {
                let next_offset = expected.offset.saturating_add(assembly.bytes.len() as u64);
                if offset != next_offset {
                    session.cancel.send_replace(true);
                    self.uploads.remove(&key);
                    return Err(network_error("file_transfer_chunk_out_of_order", None));
                }
                assembly
            }
            Some(_) => {
                session.cancel.send_replace(true);
                self.uploads.remove(&key);
                return Err(network_error("file_transfer_chunk_out_of_order", None));
            }
            None => {
                let received_chunks = session.transfer.session().received_chunk_count();
                if usize::try_from(chunk_index).ok() != Some(received_chunks)
                    || offset != expected.offset
                {
                    session.cancel.send_replace(true);
                    self.uploads.remove(&key);
                    return Err(network_error("file_transfer_chunk_out_of_order", None));
                }
                session.assembly.insert(ChunkAssembly {
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
            return Ok((false, session.transfer.progress().into()));
        }

        let assembly = session.assembly.take().expect("completed chunk assembly");
        let received_offset = assembly.expected_offset;
        let checksum = ContentHash::from_bytes(&assembly.bytes);
        if received_offset != expected.offset || checksum != assembly.expected_checksum {
            session.cancel.send_replace(true);
            self.uploads.remove(&key);
            return Err(network_error("file_transfer_chunk_corrupt", None));
        }
        match session
            .transfer
            .write_chunk(chunk_index, &assembly.bytes, now_unix_ms)
        {
            Ok(advanced) => Ok((advanced, session.transfer.progress().into())),
            Err(error) => {
                let response = adapter_error(&error);
                self.uploads.remove(&key);
                Err(response)
            }
        }
    }

    /// Cancel and remove a session after a frame boundary fails directory,
    /// membership, capability or epoch authorization. Cancellation is best
    /// effort because dropping the runtime transfer also releases admission and
    /// owns temporary-upload cleanup.
    pub(crate) fn cancel_for_authorization_loss(
        &mut self,
        transfer_id: &TransferId,
        operation_id: &OperationId,
        now_unix_ms: u64,
    ) {
        let key = NetworkSessionKey::new(transfer_id, operation_id);
        let Some(session) = self.uploads.remove(&key) else {
            return;
        };
        session.cancel.send_replace(true);
        let _ = session.transfer.cancel(now_unix_ms);
    }

    fn ensure_session_authorized(
        &mut self,
        key: &NetworkSessionKey,
        actor: &NodeId,
        member_access_epoch: u64,
        now_unix_ms: u64,
    ) -> Result<(), FileNetworkResponse> {
        let Some(session) = self.uploads.get(key) else {
            return Err(network_error("file_transfer_session_unavailable", None));
        };
        let failure = if &session.actor != actor {
            Some("file_transfer_peer_mismatch")
        } else if member_access_epoch == 0 || member_access_epoch != session.member_access_epoch {
            Some("file_transfer_access_revoked")
        } else if now_unix_ms >= session.transfer.session().binding.expires_at_unix_ms {
            Some("file_transfer_expired")
        } else {
            None
        };
        let Some(code) = failure else {
            return Ok(());
        };
        let session = self.uploads.remove(key).expect("observed upload session");
        session.cancel.send_replace(true);
        let _ = session.transfer.cancel(now_unix_ms);
        Err(network_error(code, None))
    }

    #[allow(clippy::too_many_arguments)]
    fn open_upload(
        &mut self,
        actor: &NodeId,
        context: &AccessContext,
        network_enabled: bool,
        access_epoch: u64,
        share_id: FileShareId,
        transfer_id: TransferId,
        operation_id: OperationId,
        path: SharedPath,
        manifest: TransferManifest,
        now_unix_ms: u64,
    ) -> FileNetworkResponse {
        if &context.actor != actor || access_epoch == 0 {
            return network_error("file_transfer_peer_mismatch", None);
        }
        if self.uploads.len() >= MAX_ACTIVE_FILE_NETWORK_SESSIONS {
            return network_error("file_network_session_limit", None);
        }
        let key = NetworkSessionKey::new(&transfer_id, &operation_id);
        if self.uploads.contains_key(&key) {
            return network_error("file_transfer_operation_active", None);
        }
        let expires_at_unix_ms = now_unix_ms.saturating_add(FILE_NETWORK_TRANSFER_TTL_MS);
        let session = match self.runtime.prepare_upload_session(
            context,
            access_epoch,
            network_enabled,
            &share_id,
            transfer_id,
            operation_id,
            path,
            manifest,
            expires_at_unix_ms,
            now_unix_ms,
        ) {
            Ok(session) => session,
            Err(error) => return runtime_error(&error),
        };
        let (cancel, cancellation) = watch::channel(false);
        let cancellation =
            TransferOperationCancellation::new(session.binding.operation_id.clone(), cancellation);
        let transfer = match self.runtime.begin_upload_cancellable(
            session,
            context,
            network_enabled,
            UploadAdmission::default(),
            cancellation,
            now_unix_ms,
        ) {
            Ok(transfer) => transfer,
            Err(error) => return runtime_error(&error),
        };
        let opened = FileNetworkResponse::Opened {
            session: Box::new(transfer.session().clone()),
        };
        self.uploads.insert(
            key,
            ActiveUploadSession {
                actor: actor.clone(),
                member_access_epoch: access_epoch,
                cancel,
                transfer,
                assembly: None,
            },
        );
        opened
    }

    fn resume(
        &mut self,
        actor: &NodeId,
        member_access_epoch: u64,
        context: &AccessContext,
        network_enabled: bool,
        request: ResumeRequest,
        now_unix_ms: u64,
    ) -> FileNetworkResponse {
        let key = NetworkSessionKey::new(&request.transfer_id, &request.operation_id);
        if let Err(response) =
            self.ensure_session_authorized(&key, actor, member_access_epoch, now_unix_ms)
        {
            return response;
        }
        let Some(session) = self.uploads.get_mut(&key) else {
            return network_error("file_transfer_session_unavailable", None);
        };
        if request.actor != *actor {
            session.cancel.send_replace(true);
            self.uploads.remove(&key);
            return network_error("file_transfer_peer_mismatch", None);
        }
        session.cancel.send_replace(false);
        session.assembly = None;
        match session.transfer.resume_with_request(
            &request,
            member_access_epoch,
            context,
            network_enabled,
            now_unix_ms,
        ) {
            Ok(()) => FileNetworkResponse::Opened {
                session: Box::new(session.transfer.session().clone()),
            },
            Err(error) => {
                let response = adapter_error(&error);
                if session.transfer.session().state.is_terminal() {
                    self.uploads.remove(&key);
                }
                response
            }
        }
    }

    fn pause(
        &mut self,
        actor: &NodeId,
        member_access_epoch: u64,
        transfer_id: &TransferId,
        operation_id: &OperationId,
        now_unix_ms: u64,
    ) -> FileNetworkResponse {
        let key = NetworkSessionKey::new(transfer_id, operation_id);
        if let Err(response) =
            self.ensure_session_authorized(&key, actor, member_access_epoch, now_unix_ms)
        {
            return response;
        }
        let Some(session) = self.uploads.get_mut(&key) else {
            return network_error("file_transfer_session_unavailable", None);
        };
        session.assembly = None;
        match session.transfer.pause(now_unix_ms) {
            Ok(()) => progress_response(session.transfer.progress()),
            Err(error) => adapter_error(&error),
        }
    }

    fn cancel(
        &mut self,
        actor: &NodeId,
        member_access_epoch: u64,
        transfer_id: &TransferId,
        operation_id: &OperationId,
        now_unix_ms: u64,
    ) -> FileNetworkResponse {
        let key = NetworkSessionKey::new(transfer_id, operation_id);
        if let Err(response) =
            self.ensure_session_authorized(&key, actor, member_access_epoch, now_unix_ms)
        {
            return response;
        }
        let Some(session) = self.uploads.remove(&key) else {
            return network_error("file_transfer_session_unavailable", None);
        };
        let mut progress = session.transfer.progress();
        session.cancel.send_replace(true);
        match session.transfer.cancel(now_unix_ms) {
            Ok(()) => {
                progress.state = TransferState::Cancelled;
                progress_response(progress)
            }
            Err(error) => adapter_error(&error),
        }
    }

    fn finish(
        &mut self,
        actor: &NodeId,
        member_access_epoch: u64,
        transfer_id: &TransferId,
        operation_id: &OperationId,
        now_unix_ms: u64,
    ) -> FileNetworkResponse {
        let key = NetworkSessionKey::new(transfer_id, operation_id);
        if let Err(response) =
            self.ensure_session_authorized(&key, actor, member_access_epoch, now_unix_ms)
        {
            return response;
        }
        let Some(session) = self.uploads.get_mut(&key) else {
            return network_error("file_transfer_session_unavailable", None);
        };
        if session.assembly.is_some() {
            return network_error("file_transfer_incomplete", None);
        }
        match session.transfer.finish(now_unix_ms) {
            Ok(progress) => {
                self.uploads.remove(&key);
                FileNetworkResponse::Completed {
                    progress: progress.into(),
                }
            }
            Err(error) => {
                let response = adapter_error(&error);
                if session.transfer.session().state.is_terminal() {
                    self.uploads.remove(&key);
                }
                response
            }
        }
    }
}

fn progress_response(progress: TransferProgress) -> FileNetworkResponse {
    FileNetworkResponse::Progress {
        progress: progress.into(),
    }
}

fn runtime_error(error: &FileRuntimeError) -> FileNetworkResponse {
    let retry_after_ms = match error {
        FileRuntimeError::Transfer(error) => adapter_retry_after_ms(error),
        _ => None,
    };
    network_error(error.code(), retry_after_ms)
}

fn adapter_error(error: &super::FileTransferAdapterError) -> FileNetworkResponse {
    network_error(error.code(), adapter_retry_after_ms(error))
}

fn adapter_retry_after_ms(error: &super::FileTransferAdapterError) -> Option<u64> {
    match error {
        super::FileTransferAdapterError::RateLimited { retry_after_ms } => Some(*retry_after_ms),
        _ => None,
    }
}

fn network_error(code: &str, retry_after_ms: Option<u64>) -> FileNetworkResponse {
    FileNetworkResponse::Error {
        code: code.to_owned(),
        retry_after_ms,
    }
}

pub(crate) fn is_file_network_frame(payload: &[u8]) -> bool {
    payload.starts_with(FRAME_MAGIC)
}

pub(crate) fn encode_file_network_frame(
    frame: &FileNetworkFrame,
) -> Result<Vec<u8>, FileNetworkError> {
    let body = encode(frame).map_err(|_| FileNetworkError::EncodeFailed)?;
    let total = FRAME_MAGIC
        .len()
        .checked_add(1)
        .and_then(|size| size.checked_add(body.len()))
        .ok_or(FileNetworkError::FrameTooLarge)?;
    if total > MAX_FILE_NETWORK_FRAME_LEN {
        return Err(FileNetworkError::FrameTooLarge);
    }
    let mut encoded = Vec::with_capacity(total);
    encoded.extend_from_slice(FRAME_MAGIC);
    encoded.push(FRAME_VERSION);
    encoded.extend_from_slice(&body);
    Ok(encoded)
}

pub(crate) fn decode_file_network_frame(
    payload: &[u8],
) -> Result<FileNetworkFrame, FileNetworkError> {
    if payload.len() > MAX_FILE_NETWORK_FRAME_LEN {
        return Err(FileNetworkError::FrameTooLarge);
    }
    let payload = payload
        .strip_prefix(FRAME_MAGIC)
        .ok_or(FileNetworkError::UnknownPrefix)?;
    let Some((&version, body)) = payload.split_first() else {
        return Err(FileNetworkError::Malformed);
    };
    if version != FRAME_VERSION {
        return Err(FileNetworkError::UnsupportedVersion);
    }
    if body.is_empty() {
        return Err(FileNetworkError::Malformed);
    }
    let frame: FileNetworkFrame = decode(body).map_err(|_| FileNetworkError::Malformed)?;
    validate_frame(&frame)?;
    Ok(frame)
}

pub(crate) fn validate_network_manifest(
    manifest: &TransferManifest,
) -> Result<(), FileNetworkError> {
    manifest
        .validate()
        .map_err(|_| FileNetworkError::InvalidManifest)?;
    if manifest.chunk_size > MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES as u64 {
        return Err(FileNetworkError::InvalidManifest);
    }
    Ok(())
}

pub(crate) fn validate_fragment(offset: u64, data: &[u8]) -> Result<(), FileNetworkError> {
    let end = offset
        .checked_add(data.len() as u64)
        .ok_or(FileNetworkError::InvalidFragment)?;
    if data.is_empty()
        || data.len() > MAX_FILE_NETWORK_FRAGMENT_BYTES
        || end > MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES as u64
    {
        return Err(FileNetworkError::InvalidFragment);
    }
    Ok(())
}

fn validate_frame(frame: &FileNetworkFrame) -> Result<(), FileNetworkError> {
    match frame {
        FileNetworkFrame::Request { request, .. } => {
            if let FileNetworkRequest::OpenUpload { manifest, .. } = request {
                validate_network_manifest(manifest)?;
            }
        }
        FileNetworkFrame::Response { response, .. } => {
            if let FileNetworkResponse::Error {
                code,
                retry_after_ms,
            } = response
            {
                if !valid_error_code(code) || retry_after_ms.is_some_and(|delay| delay > 86_400_000)
                {
                    return Err(FileNetworkError::InvalidErrorCode);
                }
            }
        }
        FileNetworkFrame::Chunk { offset, data, .. } => validate_fragment(*offset, data)?,
    }
    Ok(())
}

fn valid_error_code(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= MAX_FILE_NETWORK_ERROR_CODE_LEN
        && code.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use fuckipv6_common::domain::{
        CapabilityKind, ContentHash, FileAcl, FileAclEffect, FileAclRule, FileAction, FileChunk,
        FileDeletePolicy, FileId, FileQuota, FileShare, FileShareId, FileShareMode,
        MemberLifecycleStatus, MembershipRole, NetworkId, NetworkMember, NetworkMemberDirectory,
        NetworkMemberPresence, NetworkMemberStatus, NodeId, SubjectSelector, SymlinkPolicy,
        TransferBinding, TransferDirection, TransferManifest, FILE_SHARE_SCHEMA_VERSION,
    };
    use tempfile::TempDir;

    use super::*;
    use crate::file_share::FileShareCapability;

    fn node(byte: u8) -> NodeId {
        NodeId::from_public_key(&[byte])
    }

    fn transfer(byte: u8) -> TransferId {
        TransferId::from_public_key(&[byte])
    }

    fn operation(byte: u8) -> OperationId {
        OperationId::from_public_key(&[byte])
    }

    fn share_id(byte: u8) -> FileShareId {
        FileShareId::from_public_key(&[byte])
    }

    fn allow_acl() -> FileAcl {
        FileAcl {
            rules: vec![FileAclRule {
                id: "allow".into(),
                subjects: BTreeSet::from([SubjectSelector::AnyMember]),
                path_prefix: SharedPath::root(),
                actions: BTreeSet::from([fuckipv6_common::domain::FileAction::Download]),
                effect: FileAclEffect::Allow,
            }],
        }
    }

    fn share() -> FileShare {
        FileShare {
            schema_version: FILE_SHARE_SCHEMA_VERSION,
            id: share_id(1),
            network_id: NetworkId::from_public_key(b"network"),
            owner: node(9),
            display_name: "Team files".into(),
            mode: FileShareMode::ReadOnly,
            revision: 2,
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

    fn manifest(content: &[u8], chunk_size: u64) -> TransferManifest {
        let chunks = content
            .chunks(usize::try_from(chunk_size).expect("chunk size"))
            .enumerate()
            .map(|(index, bytes)| FileChunk {
                index: u32::try_from(index).expect("chunk index"),
                offset: u64::try_from(index).expect("offset") * chunk_size,
                length: bytes.len() as u64,
                checksum: ContentHash::from_bytes(bytes),
            })
            .collect();
        TransferManifest {
            file_id: FileId::from_public_key(b"file"),
            total_bytes: content.len() as u64,
            chunk_size,
            content_hash: ContentHash::from_bytes(content),
            chunks,
        }
    }

    fn upload_share() -> FileShare {
        let mut share = share();
        share.mode = FileShareMode::ReadWrite;
        for acl in [&mut share.network_acl, &mut share.local_acl] {
            acl.rules[0].actions.insert(FileAction::Upload);
        }
        share
    }

    fn upload_context(share: &FileShare, actor: &NodeId) -> AccessContext {
        AccessContext {
            network_id: share.network_id.clone(),
            actor: actor.clone(),
            role: NetworkRole::Administrator,
            groups: BTreeSet::new(),
            membership_active: true,
            recent_reauthentication: false,
            emergency_coordinator: false,
        }
    }

    fn session(manifest: TransferManifest) -> TransferSession {
        let binding = TransferBinding {
            transfer_id: transfer(2),
            operation_id: operation(3),
            share_id: share_id(1),
            actor: node(4),
            access_epoch: 7,
            path: SharedPath::parse("docs/readme.txt").expect("path"),
            direction: TransferDirection::Upload,
            file_version: None,
            share_revision: share().revision,
            expires_at_unix_ms: 10_000,
        };
        TransferSession::new(binding, manifest, 1).expect("session")
    }

    fn member(actor: &NodeId, access_epoch: u64) -> NetworkMember {
        NetworkMember {
            node_id: actor.clone(),
            display_name: None,
            identity_fingerprint: None,
            role: MembershipRole::Administrator,
            groups: Vec::new(),
            capabilities: Some(vec![CapabilityKind::File]),
            status: NetworkMemberStatus::Active,
            lifecycle_state: MemberLifecycleStatus::Active,
            revision: 3,
            access_epoch,
            available_actions: Vec::new(),
            presence: NetworkMemberPresence::unknown(),
            exit_availability: Default::default(),
            joined_at_unix_ms: Some(10),
        }
    }

    fn directory(network_id: NetworkId, member: NetworkMember) -> NetworkMemberDirectory {
        NetworkMemberDirectory {
            network_id,
            generated_at_unix_ms: 1_000,
            viewer_role: MembershipRole::Member,
            can_manage_members: false,
            exit_selection_policy: Default::default(),
            exit_selection_policy_revision: 0,
            members: vec![member],
        }
    }

    fn response_code(response: FileNetworkResponse) -> String {
        match response {
            FileNetworkResponse::Error { code, .. } => code,
            other => panic!("expected error response, got {other:?}"),
        }
    }

    fn round_trip(frame: FileNetworkFrame) {
        let encoded = encode_file_network_frame(&frame).expect("encode");
        assert!(is_file_network_frame(&encoded));
        assert_eq!(decode_file_network_frame(&encoded).expect("decode"), frame);
    }

    #[test]
    fn every_control_message_round_trips() {
        let manifest = manifest(
            &vec![7; MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES + 1],
            MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES as u64,
        );
        let session = session(manifest.clone());
        let request_id = [5; 16];
        let requests = [
            FileNetworkRequest::OpenUpload {
                share_id: share_id(1),
                transfer_id: transfer(2),
                operation_id: operation(3),
                path: SharedPath::parse("docs/new.txt").expect("path"),
                manifest,
            },
            FileNetworkRequest::OpenDownload {
                share_id: share_id(1),
                transfer_id: transfer(2),
                operation_id: operation(3),
                path: SharedPath::parse("docs/readme.txt").expect("path"),
            },
            FileNetworkRequest::Resume {
                request: ResumeRequest {
                    transfer_id: transfer(2),
                    operation_id: operation(3),
                    share_id: share_id(1),
                    actor: node(4),
                    access_epoch: 7,
                    path: SharedPath::parse("docs/readme.txt").expect("path"),
                    direction: TransferDirection::Upload,
                    content_hash: ContentHash::from_bytes(b"hash"),
                    share_revision: 2,
                },
            },
            FileNetworkRequest::Pause {
                transfer_id: transfer(2),
                operation_id: operation(3),
            },
            FileNetworkRequest::Cancel {
                transfer_id: transfer(2),
                operation_id: operation(3),
            },
            FileNetworkRequest::Finish {
                transfer_id: transfer(2),
                operation_id: operation(3),
            },
        ];
        for request in requests {
            round_trip(FileNetworkFrame::request(request_id, request).expect("request"));
        }

        let progress = FileNetworkProgress {
            state: TransferState::Active,
            transferred_bytes: 42,
            total_bytes: 100,
            received_chunks: 1,
            total_chunks: 2,
        };
        let responses = [
            FileNetworkResponse::Opened {
                session: Box::new(session),
            },
            FileNetworkResponse::Progress { progress },
            FileNetworkResponse::ChunkAccepted {
                chunk_index: 0,
                chunk_complete: true,
                progress,
            },
            FileNetworkResponse::Completed { progress },
            FileNetworkResponse::Error {
                code: "file_transfer_rate_limited".into(),
                retry_after_ms: Some(250),
            },
        ];
        for response in responses {
            round_trip(FileNetworkFrame::response(request_id, response).expect("response"));
        }
    }

    #[test]
    fn chunk_fragments_are_bounded_and_round_trip() {
        let frame = FileNetworkFrame::chunk(
            [1; 16],
            transfer(2),
            operation(3),
            4,
            48 * 1024,
            vec![9; MAX_FILE_NETWORK_FRAGMENT_BYTES],
        )
        .expect("fragment");
        round_trip(frame);
    }

    #[test]
    fn fragment_validation_rejects_empty_oversized_and_out_of_range_data() {
        assert_eq!(
            FileNetworkFrame::chunk([1; 16], transfer(1), operation(2), 0, 0, Vec::new()),
            Err(FileNetworkError::InvalidFragment)
        );
        assert_eq!(
            FileNetworkFrame::chunk(
                [1; 16],
                transfer(1),
                operation(2),
                0,
                0,
                vec![0; MAX_FILE_NETWORK_FRAGMENT_BYTES + 1]
            ),
            Err(FileNetworkError::InvalidFragment)
        );
        assert_eq!(
            FileNetworkFrame::chunk(
                [1; 16],
                transfer(1),
                operation(2),
                0,
                MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES as u64,
                vec![0]
            ),
            Err(FileNetworkError::InvalidFragment)
        );
    }

    #[test]
    fn remote_manifests_must_use_bounded_transfer_chunks() {
        let invalid = manifest(b"small", MAX_FILE_NETWORK_TRANSFER_CHUNK_BYTES as u64 + 1);
        assert_eq!(
            FileNetworkFrame::request(
                [1; 16],
                FileNetworkRequest::OpenUpload {
                    share_id: share_id(1),
                    transfer_id: transfer(2),
                    operation_id: operation(3),
                    path: SharedPath::parse("small.bin").expect("path"),
                    manifest: invalid,
                }
            ),
            Err(FileNetworkError::InvalidManifest)
        );
    }

    #[test]
    fn frame_decoder_rejects_unknown_magic_version_and_trailing_bytes() {
        assert_eq!(
            decode_file_network_frame(b"not-a-file-frame"),
            Err(FileNetworkError::UnknownPrefix)
        );

        let frame = FileNetworkFrame::request(
            [1; 16],
            FileNetworkRequest::Pause {
                transfer_id: transfer(2),
                operation_id: operation(3),
            },
        )
        .expect("frame");
        let mut encoded = encode_file_network_frame(&frame).expect("encode");
        encoded[8] = FRAME_VERSION + 1;
        assert_eq!(
            decode_file_network_frame(&encoded),
            Err(FileNetworkError::UnsupportedVersion)
        );

        let mut encoded = encode_file_network_frame(&frame).expect("encode");
        encoded.push(0);
        assert_eq!(
            decode_file_network_frame(&encoded),
            Err(FileNetworkError::Malformed)
        );
    }

    #[test]
    fn frame_decoder_rejects_empty_body_and_oversized_payload() {
        let mut prefix = FRAME_MAGIC.to_vec();
        prefix.push(FRAME_VERSION);
        assert_eq!(
            decode_file_network_frame(&prefix),
            Err(FileNetworkError::Malformed)
        );
        assert_eq!(
            decode_file_network_frame(&vec![0; MAX_FILE_NETWORK_FRAME_LEN + 1]),
            Err(FileNetworkError::FrameTooLarge)
        );
    }

    #[test]
    fn response_errors_use_stable_bounded_codes() {
        assert_eq!(
            FileNetworkFrame::response(
                [1; 16],
                FileNetworkResponse::Error {
                    code: "FILE TRACE /secret".into(),
                    retry_after_ms: None,
                }
            ),
            Err(FileNetworkError::InvalidErrorCode)
        );
        assert_eq!(
            FileNetworkFrame::response(
                [1; 16],
                FileNetworkResponse::Error {
                    code: "file_transfer_rate_limited".into(),
                    retry_after_ms: Some(86_400_001),
                }
            ),
            Err(FileNetworkError::InvalidErrorCode)
        );
    }

    #[test]
    fn encoded_frame_never_exceeds_transport_bound() {
        let frame = FileNetworkFrame::chunk(
            [1; 16],
            transfer(2),
            operation(3),
            7,
            0,
            vec![1; MAX_FILE_NETWORK_FRAGMENT_BYTES],
        )
        .expect("frame");
        let encoded = encode_file_network_frame(&frame).expect("encode");
        assert!(encoded.len() <= MAX_FILE_NETWORK_FRAME_LEN);
        assert!(encoded.len() <= usize::from(u16::MAX));
        assert!(encoded.len() <= 64 * 1024);
    }

    #[test]
    fn trusted_file_member_fails_closed_at_every_authorization_gate() {
        let network_id = NetworkId::from_public_key(b"trusted-network");
        let actor = node(4);
        let authoritative = directory(network_id.clone(), member(&actor, 41));

        let trusted = trusted_file_member(
            &network_id,
            Some(&authoritative),
            Some(1_000),
            &actor,
            2_000,
        )
        .expect("authorized member");
        assert_eq!(trusted.member_access_epoch, 41);
        assert_eq!(trusted.context.actor, actor);
        assert_eq!(trusted.context.role, NetworkRole::Administrator);
        assert!(trusted.context.membership_active);
        assert!(trusted.context.groups.is_empty());

        assert_eq!(
            response_code(
                trusted_file_member(&network_id, None, Some(1_000), &actor, 2_000)
                    .expect_err("missing directory")
            ),
            "file_member_directory_unavailable"
        );
        assert_eq!(
            response_code(
                trusted_file_member(&network_id, Some(&authoritative), None, &actor, 2_000)
                    .expect_err("missing directory timestamp")
            ),
            "file_member_directory_unavailable"
        );
        assert_eq!(
            response_code(
                trusted_file_member(
                    &network_id,
                    Some(&authoritative),
                    Some(1_000),
                    &actor,
                    91_001
                )
                .expect_err("stale directory")
            ),
            "file_member_directory_unavailable"
        );
        assert_eq!(
            response_code(
                trusted_file_member(
                    &network_id,
                    Some(&authoritative),
                    Some(2_001),
                    &actor,
                    2_000
                )
                .expect_err("future directory timestamp")
            ),
            "file_member_directory_unavailable"
        );
        let other_network = NetworkId::from_public_key(b"other-network");
        assert_eq!(
            response_code(
                trusted_file_member(
                    &other_network,
                    Some(&authoritative),
                    Some(1_000),
                    &actor,
                    2_000
                )
                .expect_err("cross-network directory")
            ),
            "file_member_directory_unavailable"
        );
        assert_eq!(
            response_code(
                trusted_file_member(
                    &network_id,
                    Some(&authoritative),
                    Some(1_000),
                    &node(5),
                    2_000
                )
                .expect_err("unknown member")
            ),
            "file_member_unavailable"
        );

        let mut suspended = authoritative.clone();
        suspended.members[0].lifecycle_state = MemberLifecycleStatus::Suspended;
        assert_eq!(
            response_code(
                trusted_file_member(&network_id, Some(&suspended), Some(1_000), &actor, 2_000)
                    .expect_err("suspended member")
            ),
            "file_membership_inactive"
        );

        let mut missing_file = authoritative.clone();
        missing_file.members[0].capabilities = Some(vec![CapabilityKind::Relay]);
        assert_eq!(
            response_code(
                trusted_file_member(&network_id, Some(&missing_file), Some(1_000), &actor, 2_000)
                    .expect_err("member lacks File")
            ),
            "file_capability_unavailable"
        );

        let mut zero_epoch = authoritative.clone();
        zero_epoch.members[0].access_epoch = 0;
        assert_eq!(
            response_code(
                trusted_file_member(&network_id, Some(&zero_epoch), Some(1_000), &actor, 2_000)
                    .expect_err("zero access epoch")
            ),
            "file_member_epoch_invalid"
        );
    }

    #[test]
    fn sessions_are_bound_to_both_member_epoch_and_local_capability_epoch() {
        let temporary = TempDir::new().expect("tempdir");
        let share = upload_share();
        let actor = node(4);
        let runtime = FileRuntime::new(FileShareCapability::new());
        runtime
            .enable(
                &share,
                &node(9),
                temporary.path().to_str().expect("utf8 path"),
                1,
            )
            .expect("enable File runtime");
        let context = upload_context(&share, &actor);
        let content = b"hello";
        let open = |transfer_byte, operation_byte| FileNetworkRequest::OpenUpload {
            share_id: share.id.clone(),
            transfer_id: transfer(transfer_byte),
            operation_id: operation(operation_byte),
            path: SharedPath::parse(&format!("hello-{transfer_byte}.txt")).expect("path"),
            manifest: manifest(content, 16),
        };
        let mut data_plane = FileNetworkDataPlane::new(runtime.clone());

        assert!(matches!(
            data_plane.handle_request(&actor, &context, true, 7, open(2, 3), 10),
            FileNetworkResponse::Opened { .. }
        ));
        let (chunk_complete, _) = data_plane
            .handle_chunk(&actor, 7, &transfer(2), &operation(3), 0, 0, content, 11)
            .expect("accepted fragment");
        assert!(chunk_complete);
        assert!(matches!(
            data_plane.handle_request(
                &actor,
                &context,
                true,
                7,
                FileNetworkRequest::Finish {
                    transfer_id: transfer(2),
                    operation_id: operation(3),
                },
                12,
            ),
            FileNetworkResponse::Completed { .. }
        ));
        assert_eq!(data_plane.session_count(), 0);

        assert!(matches!(
            data_plane.handle_request(&actor, &context, true, 7, open(4, 5), 13),
            FileNetworkResponse::Opened { .. }
        ));
        runtime
            .capability()
            .disable(&node(9), 14)
            .expect("disable File runtime");
        assert_eq!(
            response_code(
                data_plane
                    .handle_chunk(&actor, 7, &transfer(4), &operation(5), 0, 0, content, 15)
                    .expect_err("disabled local capability")
            ),
            "file_capability_revoked"
        );
        assert_eq!(data_plane.session_count(), 0);

        runtime
            .enable(
                &share,
                &node(9),
                temporary.path().to_str().expect("utf8 path"),
                16,
            )
            .expect("re-enable File runtime");
        assert!(matches!(
            data_plane.handle_request(&actor, &context, true, 7, open(6, 7), 17),
            FileNetworkResponse::Opened { .. }
        ));
        assert_eq!(
            response_code(
                data_plane
                    .handle_chunk(&actor, 8, &transfer(6), &operation(7), 0, 0, content, 18)
                    .expect_err("changed member epoch")
            ),
            "file_transfer_access_revoked"
        );
        assert_eq!(data_plane.session_count(), 0);
    }
}
