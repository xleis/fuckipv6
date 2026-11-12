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
use fuckipv6_common::domain::{
    FileShareId, OperationId, ResumeRequest, SharedPath, TransferId, TransferManifest,
    TransferSession, TransferState,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::TransferProgress;

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

    #[cfg(test)]
    pub(crate) fn chunk(
        transfer_id: TransferId,
        operation_id: OperationId,
        chunk_index: u32,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Self, FileNetworkError> {
        validate_fragment(offset, &data)?;
        Ok(Self::Chunk {
            transfer_id,
            operation_id,
            chunk_index,
            offset,
            data,
        })
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
        ContentHash, FileAcl, FileAclEffect, FileAclRule, FileChunk, FileDeletePolicy, FileId,
        FileQuota, FileShare, FileShareId, FileShareMode, NetworkId, NodeId, SubjectSelector,
        SymlinkPolicy, TransferBinding, TransferDirection, TransferManifest,
        FILE_SHARE_SCHEMA_VERSION,
    };

    use super::*;

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
            FileNetworkFrame::chunk(transfer(1), operation(2), 0, 0, Vec::new()),
            Err(FileNetworkError::InvalidFragment)
        );
        assert_eq!(
            FileNetworkFrame::chunk(
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
}
