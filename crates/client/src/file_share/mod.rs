//! Local File-share filesystem adapter.
//!
//! Network-visible authorization and transfer state live in
//! `fuckipv6_common::domain::file_share`. This module owns the platform-local
//! root selection, path containment, symlink policy, and temporary upload
//! lifecycle. It deliberately exposes no synchronization, mirroring, or mount
//! behavior.

mod error;
mod root;
mod temp;

pub use error::{FileShareError, FileShareErrorCode, FileShareResult};
pub use root::{LocalShareRoot, FILE_TEMP_DIRECTORY};
pub use temp::TemporaryUpload;
