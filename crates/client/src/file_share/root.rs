#[cfg(unix)]
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use fuckipv6_common::domain::{ShareRoot, SharedPath, SymlinkPolicy};

use super::error::{FileShareError, FileShareErrorCode, FileShareResult};

pub const FILE_TEMP_DIRECTORY: &str = ".fuckipv6-tmp";

/// A node-owner-selected share root anchored for the lifetime of the File
/// capability. The canonical local path is deliberately not serializable and
/// must never be projected into network snapshots, events, audit records or UI
/// logs.
#[derive(Clone)]
pub struct LocalShareRoot {
    canonical_root: PathBuf,
    symlink_policy: SymlinkPolicy,
    #[cfg(unix)]
    root_file: Arc<File>,
    #[cfg(not(unix))]
    _private: (),
}

impl std::fmt::Debug for LocalShareRoot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LocalShareRoot")
            .field("symlink_policy", &self.symlink_policy)
            .finish_non_exhaustive()
    }
}

impl LocalShareRoot {
    /// Validate and anchor an owner-selected directory. Absolute filesystem
    /// roots, the user's home directory itself, and operating-system roots are
    /// rejected to prevent accidental whole-disk sharing.
    pub fn select(root_reference: &str, symlink_policy: SymlinkPolicy) -> FileShareResult<Self> {
        let selected = ShareRoot::new(root_reference).map_err(|error| {
            FileShareError::new(FileShareErrorCode::RootInvalid, error.to_string())
        })?;
        let selected_path = PathBuf::from(selected.local_reference());
        let metadata = fs::symlink_metadata(&selected_path).map_err(|error| {
            FileShareError::io(
                FileShareErrorCode::RootUnavailable,
                "shared root is unavailable",
                error,
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(FileShareError::new(
                FileShareErrorCode::RootSymlink,
                "shared root must not be a symbolic link or reparse point",
            ));
        }
        if !metadata.is_dir() {
            return Err(FileShareError::new(
                FileShareErrorCode::RootNotDirectory,
                "shared root must be a directory",
            ));
        }
        let canonical_root = fs::canonicalize(&selected_path).map_err(|error| {
            FileShareError::io(
                FileShareErrorCode::RootUnavailable,
                "shared root could not be canonicalized",
                error,
            )
        })?;
        if is_sensitive_root(&canonical_root) {
            return Err(FileShareError::new(
                FileShareErrorCode::RootSensitive,
                "whole-disk, home, and operating-system roots cannot be shared",
            ));
        }

        #[cfg(unix)]
        let root_file = {
            use std::os::unix::fs::OpenOptionsExt;
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&canonical_root)
                .map_err(|error| {
                    FileShareError::io(
                        FileShareErrorCode::RootUnavailable,
                        "shared root could not be opened safely",
                        error,
                    )
                })?;
            let opened = file.metadata().map_err(|error| {
                FileShareError::io(
                    FileShareErrorCode::RootUnavailable,
                    "shared root metadata is unavailable",
                    error,
                )
            })?;
            let canonical = fs::metadata(&canonical_root).map_err(|error| {
                FileShareError::io(
                    FileShareErrorCode::RootUnavailable,
                    "shared root changed while being opened",
                    error,
                )
            })?;
            if opened.dev() != canonical.dev() || opened.ino() != canonical.ino() {
                return Err(FileShareError::new(
                    FileShareErrorCode::RootUnavailable,
                    "shared root changed while being opened",
                ));
            }
            Arc::new(file)
        };

        Ok(Self {
            canonical_root,
            symlink_policy,
            #[cfg(unix)]
            root_file,
            #[cfg(not(unix))]
            _private: (),
        })
    }

    /// Canonical root path for local adapter use only. Callers must not put this
    /// value into network-visible DTOs or logs.
    pub fn canonical_path(&self) -> &Path {
        &self.canonical_root
    }

    pub const fn symlink_policy(&self) -> SymlinkPolicy {
        self.symlink_policy
    }

    /// Re-open the configured path to detect removal, replacement, or permission
    /// changes that an already anchored descriptor would otherwise hide.
    pub fn probe(&self) -> FileShareResult<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&self.canonical_root)
                .map_err(|error| {
                    FileShareError::io(
                        FileShareErrorCode::RootUnavailable,
                        "shared root is unavailable",
                        error,
                    )
                })?;
            let opened = file.metadata().map_err(|error| {
                FileShareError::io(
                    FileShareErrorCode::RootUnavailable,
                    "shared root metadata is unavailable",
                    error,
                )
            })?;
            let canonical = fs::metadata(&self.canonical_root).map_err(|error| {
                FileShareError::io(
                    FileShareErrorCode::RootUnavailable,
                    "shared root is unavailable",
                    error,
                )
            })?;
            if opened.dev() != canonical.dev() || opened.ino() != canonical.ino() {
                return Err(FileShareError::new(
                    FileShareErrorCode::RootUnavailable,
                    "shared root changed while being checked",
                ));
            }
        }
        #[cfg(not(unix))]
        {
            let metadata = fs::metadata(&self.canonical_root).map_err(|error| {
                FileShareError::io(
                    FileShareErrorCode::RootUnavailable,
                    "shared root is unavailable",
                    error,
                )
            })?;
            if !metadata.is_dir() {
                return Err(FileShareError::new(
                    FileShareErrorCode::RootNotDirectory,
                    "shared root is no longer a directory",
                ));
            }
        }
        Ok(())
    }

    pub fn open_read(&self, path: &SharedPath) -> FileShareResult<File> {
        self.reject_root_io(path)?;
        match self.symlink_policy {
            #[cfg(unix)]
            SymlinkPolicy::Reject => self.open_without_symlinks(path, libc_read_flags(), 0),
            #[cfg(not(unix))]
            SymlinkPolicy::Reject => self.open_without_symlinks_portable(path),
            SymlinkPolicy::AllowWithinRoot => self.open_following_within_root(path),
        }
    }

    /// Create a new file without following a final symlink and without ever
    /// replacing an existing object.
    pub fn open_write_new(&self, path: &SharedPath) -> FileShareResult<File> {
        self.reject_root_io(path)?;
        #[cfg(unix)]
        {
            self.open_without_symlinks(path, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL, 0o600)
        }
        #[cfg(not(unix))]
        {
            self.open_write_new_portable(path)
        }
    }

    pub fn open_directory(&self, path: &SharedPath) -> FileShareResult<File> {
        #[cfg(unix)]
        {
            if path.is_root() {
                return self.root_file.try_clone().map_err(|error| {
                    FileShareError::io(
                        FileShareErrorCode::Io,
                        "shared root handle could not be cloned",
                        error,
                    )
                });
            }
            self.open_without_symlinks(path, libc::O_RDONLY | libc::O_DIRECTORY, 0)
        }
        #[cfg(not(unix))]
        {
            self.open_directory_portable(path)
        }
    }

    pub(crate) fn temporary_directory_path(&self) -> PathBuf {
        self.canonical_root.join(FILE_TEMP_DIRECTORY)
    }

    pub(crate) fn temporary_file_path(&self, name: &str) -> PathBuf {
        self.temporary_directory_path().join(name)
    }

    pub(crate) fn create_temporary_file(&self, name: &str) -> FileShareResult<File> {
        #[cfg(unix)]
        {
            let directory = self.ensure_temporary_directory_unix()?;
            let name = cstring(name)?;
            let descriptor = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_CLOEXEC
                        | libc::O_NOFOLLOW,
                    0o600,
                )
            };
            if descriptor < 0 {
                return Err(map_io(
                    FileShareErrorCode::TempUnavailable,
                    "temporary upload could not be created",
                    io::Error::last_os_error(),
                ));
            }
            Ok(unsafe { File::from_raw_fd(descriptor) })
        }
        #[cfg(not(unix))]
        {
            self.create_temporary_file_portable(name)
        }
    }

    /// Atomically publish a completed temporary upload under a new target name.
    /// The hard-link operation is component-scoped through anchored directory
    /// handles on Unix and fails rather than replacing an existing object.
    pub(crate) fn link_temporary_file_new(
        &self,
        temporary_name: &str,
        target: &SharedPath,
    ) -> FileShareResult<()> {
        self.reject_root_io(target)?;
        #[cfg(unix)]
        {
            let temporary_directory = self.open_temporary_directory_unix()?;
            let target_parent = self.open_parent_directory_unix(target)?;
            let temporary_name = cstring(temporary_name)?;
            let target_name = final_component(target)?;
            let result = unsafe {
                libc::linkat(
                    temporary_directory.as_raw_fd(),
                    temporary_name.as_ptr(),
                    target_parent.as_raw_fd(),
                    target_name.as_ptr(),
                    0,
                )
            };
            if result != 0 {
                return Err(map_io(
                    FileShareErrorCode::Io,
                    "temporary upload could not be published",
                    io::Error::last_os_error(),
                ));
            }
            target_parent.sync_all().map_err(|error| {
                FileShareError::io(
                    FileShareErrorCode::Io,
                    "target directory could not be synchronized",
                    error,
                )
            })?;
            let removed = unsafe {
                libc::unlinkat(temporary_directory.as_raw_fd(), temporary_name.as_ptr(), 0)
            };
            if removed != 0 && io::Error::last_os_error().kind() != io::ErrorKind::NotFound {
                // Publication is already durable. Leave the hidden temporary
                // name for stale cleanup rather than reporting a false failure.
                tracing::warn!(
                    error = %io::Error::last_os_error(),
                    "published temporary upload could not be unlinked"
                );
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            self.link_temporary_file_new_portable(temporary_name, target)
        }
    }

    pub(crate) fn remove_temporary_file(&self, temporary_name: &str) -> FileShareResult<()> {
        #[cfg(unix)]
        {
            let directory = self.open_temporary_directory_unix()?;
            let name = cstring(temporary_name)?;
            let removed = unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) };
            if removed != 0 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::NotFound {
                    return Err(map_io(
                        FileShareErrorCode::TempUnavailable,
                        "temporary upload could not be removed",
                        error,
                    ));
                }
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            match fs::remove_file(self.temporary_file_path(temporary_name)) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(FileShareError::io(
                    FileShareErrorCode::TempUnavailable,
                    "temporary upload could not be removed",
                    error,
                )),
            }
        }
    }

    fn reject_root_io(&self, path: &SharedPath) -> FileShareResult<()> {
        if path.is_root() {
            return Err(FileShareError::new(
                FileShareErrorCode::PathRejected,
                "file operations require a non-root shared path",
            ));
        }
        if path.components().next() == Some(FILE_TEMP_DIRECTORY) {
            return Err(FileShareError::new(
                FileShareErrorCode::PathRejected,
                "the File staging directory is reserved",
            ));
        }
        Ok(())
    }

    #[cfg(unix)]
    fn open_without_symlinks(
        &self,
        path: &SharedPath,
        flags: libc::c_int,
        mode: libc::mode_t,
    ) -> FileShareResult<File> {
        let components: Vec<&str> = path.components().collect();
        let (final_name, parents) = components.split_last().ok_or_else(|| {
            FileShareError::new(FileShareErrorCode::PathRejected, "shared path is empty")
        })?;
        let parent = self.open_components_unix(parents)?;
        let final_name = cstring(final_name)?;
        let descriptor = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                final_name.as_ptr(),
                flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                mode,
            )
        };
        if descriptor < 0 {
            return Err(map_io(
                FileShareErrorCode::Io,
                "shared path could not be opened safely",
                io::Error::last_os_error(),
            ));
        }
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }

    #[cfg(unix)]
    fn open_components_unix(&self, components: &[&str]) -> FileShareResult<File> {
        let mut current = self.root_file.try_clone().map_err(|error| {
            FileShareError::io(
                FileShareErrorCode::Io,
                "shared root handle could not be cloned",
                error,
            )
        })?;
        for component in components {
            let component = cstring(component)?;
            let descriptor = unsafe {
                libc::openat(
                    current.as_raw_fd(),
                    component.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                )
            };
            if descriptor < 0 {
                let error = io::Error::last_os_error();
                if matches!(
                    error.raw_os_error(),
                    Some(code) if code == libc::ELOOP || code == libc::ENOTDIR
                ) {
                    return Err(FileShareError::io(
                        FileShareErrorCode::SymlinkRejected,
                        "shared directory component is not a directory entry",
                        error,
                    ));
                }
                return Err(map_io(
                    FileShareErrorCode::Io,
                    "shared directory component could not be opened safely",
                    error,
                ));
            }
            current = unsafe { File::from_raw_fd(descriptor) };
        }
        Ok(current)
    }

    #[cfg(unix)]
    fn open_parent_directory_unix(&self, path: &SharedPath) -> FileShareResult<File> {
        let components: Vec<&str> = path.components().collect();
        let (_, parents) = components.split_last().ok_or_else(|| {
            FileShareError::new(FileShareErrorCode::PathRejected, "shared path is empty")
        })?;
        self.open_components_unix(parents)
    }

    #[cfg(unix)]
    fn ensure_temporary_directory_unix(&self) -> FileShareResult<File> {
        let name = cstring(FILE_TEMP_DIRECTORY)?;
        let root_descriptor = self.root_file.as_raw_fd();
        let mut descriptor = unsafe {
            libc::openat(
                root_descriptor,
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if descriptor < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::NotFound {
                return Err(map_io(
                    FileShareErrorCode::TempUnavailable,
                    "temporary upload directory could not be opened",
                    error,
                ));
            }
            let created = unsafe { libc::mkdirat(root_descriptor, name.as_ptr(), 0o700) };
            if created != 0 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::AlreadyExists {
                    return Err(map_io(
                        FileShareErrorCode::TempUnavailable,
                        "temporary upload directory could not be created",
                        error,
                    ));
                }
            }
            descriptor = unsafe {
                libc::openat(
                    root_descriptor,
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                )
            };
            if descriptor < 0 {
                return Err(map_io(
                    FileShareErrorCode::TempUnavailable,
                    "temporary upload directory could not be opened",
                    io::Error::last_os_error(),
                ));
            }
        }
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }

    #[cfg(unix)]
    fn open_temporary_directory_unix(&self) -> FileShareResult<File> {
        let name = cstring(FILE_TEMP_DIRECTORY)?;
        let descriptor = unsafe {
            libc::openat(
                self.root_file.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if descriptor < 0 {
            return Err(map_io(
                FileShareErrorCode::TempUnavailable,
                "temporary upload directory could not be opened",
                io::Error::last_os_error(),
            ));
        }
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }

    fn open_following_within_root(&self, path: &SharedPath) -> FileShareResult<File> {
        let lexical = self.canonical_root.join(path.as_str());
        let before = fs::canonicalize(&lexical).map_err(|error| {
            FileShareError::io(
                FileShareErrorCode::Io,
                "shared path could not be canonicalized",
                error,
            )
        })?;
        self.ensure_inside_root(&before)?;
        let file = OpenOptions::new()
            .read(true)
            .open(&lexical)
            .map_err(|error| {
                FileShareError::io(
                    FileShareErrorCode::Io,
                    "shared path could not be opened",
                    error,
                )
            })?;
        let after = fs::canonicalize(&lexical).map_err(|error| {
            FileShareError::io(
                FileShareErrorCode::Io,
                "shared path changed while being opened",
                error,
            )
        })?;
        self.ensure_inside_root(&after)?;
        if before != after {
            return Err(FileShareError::new(
                FileShareErrorCode::SymlinkRejected,
                "shared path changed while being opened",
            ));
        }
        #[cfg(unix)]
        {
            let opened = file.metadata().map_err(|error| {
                FileShareError::io(
                    FileShareErrorCode::Io,
                    "opened shared path metadata is unavailable",
                    error,
                )
            })?;
            let target = fs::metadata(&after).map_err(|error| {
                FileShareError::io(
                    FileShareErrorCode::Io,
                    "shared path changed while being opened",
                    error,
                )
            })?;
            if opened.dev() != target.dev() || opened.ino() != target.ino() {
                return Err(FileShareError::new(
                    FileShareErrorCode::SymlinkRejected,
                    "shared path changed while being opened",
                ));
            }
        }
        Ok(file)
    }

    fn ensure_inside_root(&self, canonical: &Path) -> FileShareResult<()> {
        canonical.strip_prefix(&self.canonical_root).map_err(|_| {
            FileShareError::new(
                FileShareErrorCode::OutsideRoot,
                "canonical path escapes the selected shared root",
            )
        })?;
        Ok(())
    }

    #[cfg(not(unix))]
    fn open_without_symlinks_portable(&self, path: &SharedPath) -> FileShareResult<File> {
        let mut current = self.canonical_root.clone();
        for component in path.components() {
            current.push(component);
            let metadata = fs::symlink_metadata(&current).map_err(|error| {
                FileShareError::io(
                    FileShareErrorCode::Io,
                    "shared path could not be inspected safely",
                    error,
                )
            })?;
            if metadata.file_type().is_symlink() {
                return Err(FileShareError::new(
                    FileShareErrorCode::SymlinkRejected,
                    "symbolic links and reparse points are disabled for this share",
                ));
            }
        }
        let before = fs::canonicalize(&current).map_err(|error| {
            FileShareError::io(
                FileShareErrorCode::Io,
                "shared path could not be canonicalized",
                error,
            )
        })?;
        self.ensure_inside_root(&before)?;
        let file = OpenOptions::new()
            .read(true)
            .open(&current)
            .map_err(|error| {
                FileShareError::io(
                    FileShareErrorCode::Io,
                    "shared path could not be opened",
                    error,
                )
            })?;
        let after = fs::canonicalize(&current).map_err(|error| {
            FileShareError::io(
                FileShareErrorCode::Io,
                "shared path changed while being opened",
                error,
            )
        })?;
        self.ensure_inside_root(&after)?;
        if before != after {
            return Err(FileShareError::new(
                FileShareErrorCode::SymlinkRejected,
                "shared path changed while being opened",
            ));
        }
        Ok(file)
    }

    #[cfg(not(unix))]
    fn open_write_new_portable(&self, path: &SharedPath) -> FileShareResult<File> {
        self.validate_portable_target_parent(path)?;
        let target = self.canonical_root.join(path.as_str());
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        options.open(target).map_err(|error| {
            FileShareError::io(
                FileShareErrorCode::Io,
                "new shared file could not be created",
                error,
            )
        })
    }

    #[cfg(not(unix))]
    fn open_directory_portable(&self, path: &SharedPath) -> FileShareResult<File> {
        let target = self.canonical_root.join(path.as_str());
        let canonical = fs::canonicalize(target).map_err(|error| {
            FileShareError::io(
                FileShareErrorCode::Io,
                "shared directory could not be canonicalized",
                error,
            )
        })?;
        self.ensure_inside_root(&canonical)?;
        File::open(&canonical).map_err(|error| {
            FileShareError::io(
                FileShareErrorCode::Io,
                "shared directory could not be opened",
                error,
            )
        })
    }

    #[cfg(not(unix))]
    fn validate_portable_target_parent(&self, path: &SharedPath) -> FileShareResult<()> {
        let components: Vec<&str> = path.components().collect();
        let (_, parents) = components.split_last().ok_or_else(|| {
            FileShareError::new(FileShareErrorCode::PathRejected, "shared path is empty")
        })?;
        let parent = parents
            .iter()
            .fold(self.canonical_root.clone(), |path, component| {
                path.join(component)
            });
        let canonical = fs::canonicalize(parent).map_err(|error| {
            FileShareError::io(
                FileShareErrorCode::ParentUnavailable,
                "shared parent directory is unavailable",
                error,
            )
        })?;
        self.ensure_inside_root(&canonical)
    }

    #[cfg(not(unix))]
    fn create_temporary_file_portable(&self, name: &str) -> FileShareResult<File> {
        let directory = self.temporary_directory_path();
        fs::create_dir_all(&directory).map_err(|error| {
            FileShareError::io(
                FileShareErrorCode::TempUnavailable,
                "temporary upload directory could not be created",
                error,
            )
        })?;
        let target = directory.join(name);
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)
            .map_err(|error| {
                FileShareError::io(
                    FileShareErrorCode::TempUnavailable,
                    "temporary upload could not be created",
                    error,
                )
            })
    }

    #[cfg(not(unix))]
    fn link_temporary_file_new_portable(
        &self,
        temporary_name: &str,
        target: &SharedPath,
    ) -> FileShareResult<()> {
        self.validate_portable_target_parent(target)?;
        let target = self.canonical_root.join(target.as_str());
        fs::hard_link(self.temporary_file_path(temporary_name), &target).map_err(|error| {
            FileShareError::io(
                FileShareErrorCode::Io,
                "temporary upload could not be published",
                error,
            )
        })?;
        let _ = fs::remove_file(self.temporary_file_path(temporary_name));
        Ok(())
    }
}

#[cfg(unix)]
fn final_component(path: &SharedPath) -> FileShareResult<CString> {
    path.components()
        .last()
        .ok_or_else(|| {
            FileShareError::new(FileShareErrorCode::PathRejected, "shared path is empty")
        })
        .and_then(cstring)
}

#[cfg(unix)]
fn cstring(value: &str) -> FileShareResult<CString> {
    CString::new(value).map_err(|_| {
        FileShareError::new(
            FileShareErrorCode::PathRejected,
            "shared path contains a NUL byte",
        )
    })
}

#[cfg(unix)]
fn libc_read_flags() -> libc::c_int {
    libc::O_RDONLY
}

#[cfg(unix)]
fn map_io(fallback: FileShareErrorCode, message: &'static str, error: io::Error) -> FileShareError {
    let code = match error.raw_os_error() {
        Some(code) if code == libc::ELOOP => FileShareErrorCode::SymlinkRejected,
        Some(code) if code == libc::EEXIST => FileShareErrorCode::TargetExists,
        Some(code) if code == libc::ENOENT => fallback,
        Some(code) if code == libc::EACCES || code == libc::EPERM => fallback,
        _ => fallback,
    };
    FileShareError::io(code, message, error)
}

fn is_sensitive_root(root: &Path) -> bool {
    if root.parent().is_none() {
        return true;
    }

    #[cfg(unix)]
    {
        const SYSTEM_ROOTS: &[&str] = &[
            "/bin", "/boot", "/dev", "/etc", "/lib", "/lib64", "/proc", "/root", "/sbin", "/sys",
            "/usr", "/var",
        ];
        if SYSTEM_ROOTS
            .iter()
            .any(|candidate| root == Path::new(candidate))
        {
            return true;
        }
    }

    if let Some(home) = home_directory() {
        if let Ok(home) = fs::canonicalize(home) {
            if root == home {
                return true;
            }
        }
    }

    #[cfg(windows)]
    {
        for variable in ["WINDIR", "ProgramFiles", "ProgramFiles(x86)", "ProgramData"] {
            if let Some(system_root) = std::env::var_os(variable) {
                if let Ok(system_root) = fs::canonicalize(system_root) {
                    if root == system_root {
                        return true;
                    }
                }
            }
        }
    }

    false
}

fn home_directory() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn root() -> (tempfile::TempDir, LocalShareRoot) {
        let temporary = tempfile::tempdir().expect("tempdir");
        let selected = LocalShareRoot::select(
            temporary.path().to_str().expect("utf8 temp path"),
            SymlinkPolicy::Reject,
        )
        .expect("share root");
        (temporary, selected)
    }

    #[test]
    fn rejects_regular_file_as_share_root() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let file = temporary.path().join("file");
        fs::write(&file, b"not a directory").expect("file");
        let error = LocalShareRoot::select(file.to_str().expect("utf8"), SymlinkPolicy::Reject)
            .expect_err("regular file must be rejected");
        assert_eq!(error.code(), FileShareErrorCode::RootNotDirectory);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_whole_disk_root() {
        let error = LocalShareRoot::select("/", SymlinkPolicy::Reject)
            .expect_err("filesystem root must be rejected");
        assert_eq!(error.code(), FileShareErrorCode::RootSensitive);
    }

    #[test]
    fn reads_regular_files_inside_root() {
        let (temporary, share) = root();
        fs::create_dir(temporary.path().join("docs")).expect("directory");
        fs::write(temporary.path().join("docs/readme.txt"), b"hello").expect("file");
        let path = SharedPath::parse("docs/readme.txt").expect("path");
        let mut file = share.open_read(&path).expect("open");
        let mut content = String::new();
        file.read_to_string(&mut content).expect("read");
        assert_eq!(content, "hello");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_final_symbolic_link() {
        let (temporary, share) = root();
        let outside = tempfile::NamedTempFile::new().expect("outside");
        std::os::unix::fs::symlink(outside.path(), temporary.path().join("link")).expect("symlink");
        let path = SharedPath::parse("link").expect("path");
        let error = share.open_read(&path).expect_err("symlink must fail");
        assert_eq!(error.code(), FileShareErrorCode::SymlinkRejected);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_intermediate_symbolic_link() {
        let (temporary, share) = root();
        let outside = tempfile::tempdir().expect("outside");
        fs::write(outside.path().join("secret"), b"secret").expect("file");
        std::os::unix::fs::symlink(outside.path(), temporary.path().join("outside"))
            .expect("symlink");
        let path = SharedPath::parse("outside/secret").expect("path");
        let error = share.open_read(&path).expect_err("symlink must fail");
        assert_eq!(error.code(), FileShareErrorCode::SymlinkRejected);
    }

    #[test]
    fn creates_new_files_without_replacing_existing_files() {
        let (temporary, share) = root();
        fs::create_dir(temporary.path().join("docs")).expect("directory");
        let path = SharedPath::parse("docs/new.txt").expect("path");
        let mut file = share.open_write_new(&path).expect("create");
        file.write_all(b"first").expect("write");
        drop(file);

        let error = share
            .open_write_new(&path)
            .expect_err("existing file must not be replaced");
        assert_eq!(error.code(), FileShareErrorCode::TargetExists);
    }

    #[test]
    fn reserves_the_temporary_staging_directory() {
        let (_temporary, share) = root();
        let path = SharedPath::parse(format!("{FILE_TEMP_DIRECTORY}/upload.part")).expect("path");
        let error = share
            .open_read(&path)
            .expect_err("staging path must be inaccessible");
        assert_eq!(error.code(), FileShareErrorCode::PathRejected);
    }
}
