use std::fmt;

use serde::{Deserialize, Serialize};

/// Default fragment size for a requester-side File transfer.
pub const DEFAULT_FILE_TRANSFER_CHUNK_SIZE: u64 = 4 * 1024 * 1024;

fn default_file_transfer_chunk_size() -> u64 {
    DEFAULT_FILE_TRANSFER_CHUNK_SIZE
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileTransferDirection {
    Upload,
    Download,
}

/// Local-only request to start a requester-side File operation.
///
/// `path` is the peer's shared path. `local_path` is the source file for an
/// upload or the non-existing final destination for a download. Both values
/// are carried only over same-user authenticated local IPC and are never part
/// of the network node-command contract or logs.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartFileTransferRequest {
    pub peer: String,
    pub direction: FileTransferDirection,
    pub share_id: String,
    pub transfer_id: String,
    pub operation_id: String,
    pub path: String,
    pub local_path: String,
    #[serde(default = "default_file_transfer_chunk_size")]
    pub chunk_size: u64,
}

impl fmt::Debug for StartFileTransferRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StartFileTransferRequest")
            .field("peer", &self.peer)
            .field("direction", &self.direction)
            .field("share_id", &self.share_id)
            .field("transfer_id", &self.transfer_id)
            .field("operation_id", &self.operation_id)
            .field("path", &self.path)
            .field("local_path", &"[REDACTED]")
            .field("chunk_size", &self.chunk_size)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTransferHandle {
    pub transfer_id: String,
    pub operation_id: String,
}

/// Requester-side File controls accepted only at the local IPC boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", content = "data", rename_all = "snake_case")]
pub enum FileTransferCommand {
    Start(StartFileTransferRequest),
    Cancel {
        transfer_id: String,
        operation_id: String,
    },
    Pause {
        transfer_id: String,
        operation_id: String,
    },
    Resume {
        transfer_id: String,
        operation_id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_debug_redacts_local_path() {
        let request = StartFileTransferRequest {
            peer: "peer".into(),
            direction: FileTransferDirection::Upload,
            share_id: "share".into(),
            transfer_id: "transfer".into(),
            operation_id: "operation".into(),
            path: "docs/report.bin".into(),
            local_path: "/home/owner/private/report.bin".into(),
            chunk_size: DEFAULT_FILE_TRANSFER_CHUNK_SIZE,
        };

        let debug = format!("{request:?}");
        assert!(!debug.contains("/home/owner/private/report.bin"));
        assert!(debug.contains("[REDACTED]"));
    }
}
