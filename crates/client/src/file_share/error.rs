use std::error::Error;
use std::fmt;
use std::io;

/// Stable File-share adapter errors. UI and HTTP layers must project these
/// codes instead of inventing filesystem-specific messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileShareErrorCode {
    RootInvalid,
    RootSensitive,
    RootNotDirectory,
    RootSymlink,
    RootUnavailable,
    PathRejected,
    SymlinkRejected,
    OutsideRoot,
    ParentUnavailable,
    TargetExists,
    TempUnavailable,
    ScanLimitExceeded,
    Io,
}

impl FileShareErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RootInvalid => "file_root_invalid",
            Self::RootSensitive => "file_root_sensitive",
            Self::RootNotDirectory => "file_root_not_directory",
            Self::RootSymlink => "file_root_symlink",
            Self::RootUnavailable => "file_root_unavailable",
            Self::PathRejected => "file_path_rejected",
            Self::SymlinkRejected => "file_symlink_rejected",
            Self::OutsideRoot => "file_path_outside_root",
            Self::ParentUnavailable => "file_parent_unavailable",
            Self::TargetExists => "file_target_exists",
            Self::TempUnavailable => "file_temp_unavailable",
            Self::ScanLimitExceeded => "file_scan_limit_exceeded",
            Self::Io => "file_io_error",
        }
    }
}

#[derive(Debug)]
pub struct FileShareError {
    code: FileShareErrorCode,
    message: String,
    source: Option<io::Error>,
}

impl FileShareError {
    pub fn new(code: FileShareErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            source: None,
        }
    }

    pub fn io(code: FileShareErrorCode, message: impl Into<String>, source: io::Error) -> Self {
        Self {
            code,
            message: message.into(),
            source: Some(source),
        }
    }

    pub const fn code(&self) -> FileShareErrorCode {
        self.code
    }

    pub const fn code_str(&self) -> &'static str {
        self.code.as_str()
    }

    pub fn is_io_kind(&self, kind: io::ErrorKind) -> bool {
        self.source
            .as_ref()
            .is_some_and(|source| source.kind() == kind)
    }
}

impl fmt::Display for FileShareError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code.as_str(), self.message)?;
        if let Some(source) = &self.source {
            write!(formatter, ": {source}")?;
        }
        Ok(())
    }
}

impl Error for FileShareError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source as &(dyn Error + 'static))
    }
}

pub type FileShareResult<T> = Result<T, FileShareError>;
