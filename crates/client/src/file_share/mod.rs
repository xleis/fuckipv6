//! Local File-share filesystem adapter.
//!
//! Network-visible authorization and transfer state live in
//! `fuckipv6_common::domain::file_share`. This module owns the platform-local
//! root selection, path containment, symlink policy, and temporary upload
//! lifecycle. It deliberately exposes no synchronization, mirroring, or mount
//! behavior.

mod capability;
mod error;
mod quota;
mod root;
mod runtime;
mod temp;
mod transfer;

pub use capability::{
    FileCapabilityError, FileCapabilityLease, FileCapabilityPermit, FileCapabilitySnapshot,
    FileCapabilityState, FileShareCapability,
};
pub use error::{FileShareError, FileShareErrorCode, FileShareResult};
pub use quota::{DirectoryUsage, QuotaScanLimits, UploadQuotaRequest};
pub use root::{LocalShareRoot, FILE_TEMP_DIRECTORY};
pub use runtime::{
    FileRuntime, FileRuntimeError, FileRuntimeSnapshot, RuntimeDownloadTransfer,
    RuntimeUploadTransfer,
};
pub use temp::TemporaryUpload;
pub use transfer::{
    DownloadTransfer, FileTransferAdapter, FileTransferAdapterError, TransferOperationCancellation,
    TransferProgress, UploadAdmission, UploadTransfer,
};
