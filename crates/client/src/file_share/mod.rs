//! Local File-share filesystem adapter.
//!
//! Network-visible authorization and transfer state live in
//! `fuckipv6_common::domain::file_share`. This module owns the platform-local
//! root selection, path containment, symlink policy, and temporary upload
//! lifecycle. It deliberately exposes no synchronization, mirroring, or mount
//! behavior.

mod capability;
mod config;
mod error;
mod network;
mod network_browser;
mod network_origin;
mod origin_store;
mod owner_service;
mod quota;
mod rate_limit;
mod root;
mod runtime;
mod temp;
mod transfer;
mod transfer_control;
mod transfer_status;

pub use capability::{
    FileCapabilityError, FileCapabilityLease, FileCapabilityPermit, FileCapabilitySnapshot,
    FileCapabilityState, FileShareCapability,
};
pub use error::{FileShareError, FileShareErrorCode, FileShareResult};
pub(crate) use network::{
    decode_file_network_frame, encode_file_network_frame, is_file_network_frame,
    trusted_file_member, FileNetworkDataPlane, FileNetworkFrame, FileNetworkRequest,
    FileNetworkResponse, TrustedFileMember,
};
pub(crate) use network_browser::FileNetworkBrowser;
pub use network_browser::{
    FileDirectoryEntry, FileDirectoryListing, ListFileDirectoryRequest,
    DEFAULT_FILE_DIRECTORY_LIST_LIMIT,
};
pub(crate) use network_origin::{
    FileNetworkOrigin, FileNetworkOriginError, OriginPeer, OriginTransferEvent,
};
pub(crate) use origin_store::FileOriginStore;
pub(crate) use owner_service::FileOwnerService;
pub use owner_service::{FileOwnerServiceError, FileShareConfiguration, FileSharePreview};
pub use quota::{DirectoryUsage, QuotaScanLimits, UploadQuotaRequest};
pub use root::{
    LocalDirectoryEntry, LocalDirectoryListing, LocalFileMetadata, LocalShareRoot,
    FILE_TEMP_DIRECTORY, MAX_DIRECTORY_LIST_LIMIT,
};
pub use runtime::{
    FileRuntime, FileRuntimeError, FileRuntimeSnapshot, RuntimeDownloadTransfer,
    RuntimeUploadTransfer,
};
pub use temp::TemporaryUpload;
pub use transfer::{
    DownloadTransfer, FileTransferAdapter, FileTransferAdapterError, TransferOperationCancellation,
    TransferProgress, UploadAdmission, UploadTransfer,
};
pub use transfer_control::{
    FileTransferCommand, FileTransferDirection, FileTransferHandle, StartFileTransferRequest,
    DEFAULT_FILE_TRANSFER_CHUNK_SIZE,
};
pub use transfer_status::FileTransferStatus;
pub(crate) use transfer_status::FileTransferStatusCache;
