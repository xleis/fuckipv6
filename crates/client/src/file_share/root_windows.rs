use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};
use std::path::Path;
use std::ptr::{addr_of, addr_of_mut, null, null_mut};
use std::slice;

use fuckipv6_common::domain::SharedPath;
use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::{
    FileDispositionInformation, FileIdBothDirectoryInformation, FileLinkInformation, NtCreateFile,
    NtQueryDirectoryFile, NtSetInformationFile, FILE_CREATE, FILE_DIRECTORY_FILE,
    FILE_LINK_INFORMATION, FILE_OPEN, FILE_OPEN_IF, FILE_OPEN_REPARSE_POINT,
    FILE_SYNCHRONOUS_IO_NONALERT,
};
use windows_sys::Win32::Foundation::{
    BOOLEAN, FALSE, HANDLE, INVALID_HANDLE_VALUE, NTSTATUS, STATUS_BUFFER_OVERFLOW,
    STATUS_NOT_A_DIRECTORY, STATUS_NO_MORE_FILES, STATUS_OBJECT_NAME_COLLISION,
    STATUS_REPARSE_POINT_ENCOUNTERED, UNICODE_STRING,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, DELETE,
    FILE_ADD_SUBDIRECTORY, FILE_ATTRIBUTE_DEVICE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FILE_TRAVERSE, FILE_WRITE_DATA, OPEN_EXISTING, SYNCHRONIZE,
};
use windows_sys::Win32::System::Kernel::{OBJ_CASE_INSENSITIVE, OBJ_DONT_REPARSE};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

use super::{
    count_scanned_entry, is_generated_temporary_name, DirectoryUsage, FileShareError,
    FileShareErrorCode, FileShareResult, LocalDirectoryEntry, LocalShareRoot, QuotaScanLimits,
    FILE_TEMP_DIRECTORY, MAX_DIRECTORY_LIST_ENUMERATED_ENTRIES,
};

const DIRECTORY_TRAVERSE_ACCESS: u32 = FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE;
const DIRECTORY_LIST_ACCESS: u32 = DIRECTORY_TRAVERSE_ACCESS | FILE_LIST_DIRECTORY;
const DIRECTORY_CREATE_ACCESS: u32 = DIRECTORY_TRAVERSE_ACCESS | FILE_ADD_SUBDIRECTORY;
const FILE_READ_ACCESS: u32 = FILE_READ_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE;
const FILE_WRITE_NEW_ACCESS: u32 = FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE;
const FILE_DELETE_ACCESS: u32 = DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE;
const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
const DIRECTORY_OPTIONS: u32 =
    FILE_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT;
const FILE_OPTIONS: u32 = windows_sys::Wdk::Storage::FileSystem::FILE_NON_DIRECTORY_FILE
    | FILE_OPEN_REPARSE_POINT
    | FILE_SYNCHRONOUS_IO_NONALERT;
const DIRECTORY_ENTRY_BYTES: usize = 64 * 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct WindowsFileIdentity {
    volume_serial: u32,
    file_index: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpectedKind {
    Directory,
    RegularFile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WindowsDirectoryEntry {
    name: String,
    attributes: u32,
    size: u64,
    modified_at_unix_ms: Option<u64>,
}

pub(super) fn open_root(root: &Path) -> FileShareResult<(File, WindowsFileIdentity)> {
    let wide: Vec<u16> = root.as_os_str().encode_wide().chain([0]).collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            DIRECTORY_LIST_ACCESS,
            SHARE_ALL,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(FileShareError::io(
            FileShareErrorCode::RootUnavailable,
            "shared root could not be opened safely",
            io::Error::last_os_error(),
        ));
    }
    let file = unsafe { File::from_raw_handle(handle as RawHandle) };
    let information = file_information(&file).map_err(|error| {
        FileShareError::io(
            FileShareErrorCode::RootUnavailable,
            "shared root metadata is unavailable",
            error,
        )
    })?;
    if information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(FileShareError::new(
            FileShareErrorCode::RootSymlink,
            "shared root must not be a symbolic link or reparse point",
        ));
    }
    if information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(FileShareError::new(
            FileShareErrorCode::RootNotDirectory,
            "shared root must be a directory",
        ));
    }
    let identity = identity_from_information(&information);
    Ok((file, identity))
}

pub(super) fn probe_root(
    root: &Path,
    anchored: &File,
    expected: WindowsFileIdentity,
) -> FileShareResult<()> {
    drop(reopen_checked_root(root, anchored, expected)?);
    Ok(())
}

fn reopen_checked_root(
    root: &Path,
    anchored: &File,
    expected: WindowsFileIdentity,
) -> FileShareResult<File> {
    let anchored_identity = file_identity(anchored).map_err(|error| {
        FileShareError::io(
            FileShareErrorCode::RootUnavailable,
            "anchored shared root metadata is unavailable",
            error,
        )
    })?;
    let (opened, opened_identity) = open_root(root).map_err(|_| {
        FileShareError::new(
            FileShareErrorCode::RootUnavailable,
            "shared root is unavailable",
        )
    })?;
    if anchored_identity != expected || opened_identity != expected {
        return Err(FileShareError::new(
            FileShareErrorCode::RootUnavailable,
            "shared root was removed or replaced",
        ));
    }
    Ok(opened)
}

pub(super) fn enumerate_directory(
    directory: &File,
    parent: &SharedPath,
) -> FileShareResult<Vec<LocalDirectoryEntry>> {
    let entries = enumerate_raw(directory)?;
    let mut projected = Vec::new();
    for entry in entries {
        if parent.is_root() && entry.name.eq_ignore_ascii_case(FILE_TEMP_DIRECTORY) {
            continue;
        }
        if entry.attributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DEVICE) != 0 {
            continue;
        }
        let relative = if parent.is_root() {
            entry.name.clone()
        } else {
            format!("{}/{}", parent.as_str(), entry.name)
        };
        let Ok(path) = SharedPath::parse(relative) else {
            continue;
        };
        let is_directory = entry.attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
        projected.push(LocalDirectoryEntry {
            path,
            is_directory,
            size: if is_directory { 0 } else { entry.size },
            modified_at_unix_ms: entry.modified_at_unix_ms,
        });
    }
    Ok(projected)
}

pub(super) fn scan_directory(
    root_path: &Path,
    anchored: &File,
    expected: WindowsFileIdentity,
    depth: usize,
    limits: QuotaScanLimits,
) -> FileShareResult<DirectoryUsage> {
    let root = reopen_checked_root(root_path, anchored, expected)?;
    let mut usage = DirectoryUsage::default();
    scan_directory_anchored(&root, depth, limits, &mut usage)?;
    Ok(usage)
}

impl LocalShareRoot {
    pub(super) fn open_windows_read(&self, path: &SharedPath) -> FileShareResult<File> {
        let components: Vec<&str> = path.components().collect();
        let (final_name, parents) = components.split_last().ok_or_else(|| {
            FileShareError::new(FileShareErrorCode::PathRejected, "shared path is empty")
        })?;
        let parent = self.open_windows_components(parents)?;
        open_relative_checked(
            &parent,
            final_name,
            FILE_READ_ACCESS,
            FILE_OPEN,
            FILE_OPTIONS,
            ExpectedKind::RegularFile,
            FileShareErrorCode::Io,
            "shared path could not be opened safely",
        )
    }

    pub(super) fn open_windows_write_new(&self, path: &SharedPath) -> FileShareResult<File> {
        let components: Vec<&str> = path.components().collect();
        let (final_name, parents) = components.split_last().ok_or_else(|| {
            FileShareError::new(FileShareErrorCode::PathRejected, "shared path is empty")
        })?;
        let parent = self.open_windows_components(parents)?;
        open_relative_checked(
            &parent,
            final_name,
            FILE_WRITE_NEW_ACCESS,
            FILE_CREATE,
            FILE_OPTIONS,
            ExpectedKind::RegularFile,
            FileShareErrorCode::Io,
            "shared target could not be created safely",
        )
    }

    pub(super) fn open_windows_directory(&self, path: &SharedPath) -> FileShareResult<File> {
        if path.is_root() {
            return reopen_checked_root(&self.canonical_root, &self.root_file, self.root_identity);
        }
        let components: Vec<&str> = path.components().collect();
        let (final_name, parents) = components.split_last().ok_or_else(|| {
            FileShareError::new(FileShareErrorCode::PathRejected, "shared path is empty")
        })?;
        let parent = self.open_windows_components(parents)?;
        open_relative_checked(
            &parent,
            final_name,
            DIRECTORY_LIST_ACCESS,
            FILE_OPEN,
            DIRECTORY_OPTIONS,
            ExpectedKind::Directory,
            FileShareErrorCode::Io,
            "shared directory could not be opened safely",
        )
    }

    pub(super) fn create_temporary_file_windows(&self, name: &str) -> FileShareResult<File> {
        validate_nt_name(name)?;
        let directory = self.ensure_temporary_directory_windows()?;
        open_relative_checked(
            &directory,
            name,
            FILE_WRITE_NEW_ACCESS | DELETE,
            FILE_CREATE,
            FILE_OPTIONS,
            ExpectedKind::RegularFile,
            FileShareErrorCode::TempUnavailable,
            "temporary upload could not be created",
        )
    }

    pub(super) fn link_temporary_file_new_windows(
        &self,
        temporary_name: &str,
        target: &SharedPath,
    ) -> FileShareResult<()> {
        validate_nt_name(temporary_name)?;
        let components: Vec<&str> = target.components().collect();
        let (target_name, parents) = components.split_last().ok_or_else(|| {
            FileShareError::new(FileShareErrorCode::PathRejected, "shared path is empty")
        })?;
        let temporary_directory = self.open_temporary_directory_windows()?;
        let target_parent = self.open_windows_components(parents)?;
        let temporary = open_relative_checked(
            &temporary_directory,
            temporary_name,
            FILE_DELETE_ACCESS,
            FILE_OPEN,
            FILE_OPTIONS,
            ExpectedKind::RegularFile,
            FileShareErrorCode::TempUnavailable,
            "temporary upload could not be opened for publication",
        )?;
        let target_name: Vec<u16> = target_name.encode_utf16().collect();
        let target_name_bytes =
            target_name
                .len()
                .checked_mul(size_of::<u16>())
                .ok_or_else(|| {
                    FileShareError::new(
                        FileShareErrorCode::PathRejected,
                        "shared path is too large",
                    )
                })?;
        let information_length = offset_of!(FILE_LINK_INFORMATION, FileName)
            .checked_add(target_name_bytes)
            .ok_or_else(|| {
                FileShareError::new(FileShareErrorCode::PathRejected, "shared path is too large")
            })?;
        // `Vec<u64>` supplies an allocation aligned for the native information
        // structures while the final length remains the documented byte count.
        let words = information_length.div_ceil(size_of::<u64>());
        let mut link_buffer = vec![0_u64; words];
        let link = link_buffer.as_mut_ptr().cast::<FILE_LINK_INFORMATION>();
        unsafe {
            (*link).Anonymous.ReplaceIfExists = FALSE as u8;
            (*link).RootDirectory = target_parent.as_raw_handle() as HANDLE;
            (*link).FileNameLength = u32::try_from(target_name_bytes).map_err(|_| {
                FileShareError::new(FileShareErrorCode::PathRejected, "shared path is too large")
            })?;
            std::ptr::copy_nonoverlapping(
                target_name.as_ptr(),
                addr_of_mut!((*link).FileName).cast::<u16>(),
                target_name.len(),
            );
        }
        let mut status_block = zeroed_status_block();
        let status = unsafe {
            NtSetInformationFile(
                temporary.as_raw_handle() as HANDLE,
                &mut status_block,
                link.cast::<c_void>(),
                u32::try_from(information_length).map_err(|_| {
                    FileShareError::new(
                        FileShareErrorCode::PathRejected,
                        "shared path is too large",
                    )
                })?,
                FileLinkInformation,
            )
        };
        if status < 0 {
            return Err(nt_status_error(
                status,
                FileShareErrorCode::Io,
                "temporary upload could not be published",
            ));
        }

        match set_delete_on_close(&temporary) {
            Ok(()) => {}
            Err(error) => {
                // The completed target is already durable. Leave the hidden
                // staging name for stale cleanup instead of reporting a false
                // publication failure.
                tracing::warn!(
                    error = %error,
                    "published temporary upload could not be removed"
                );
            }
        }
        Ok(())
    }

    pub(super) fn remove_temporary_file_windows(
        &self,
        temporary_name: &str,
    ) -> FileShareResult<()> {
        validate_nt_name(temporary_name)?;
        let directory = match self.open_temporary_directory_windows() {
            Ok(directory) => directory,
            Err(error) if error.is_io_kind(io::ErrorKind::NotFound) => return Ok(()),
            Err(error) => return Err(error),
        };
        let file = match open_relative_checked(
            &directory,
            temporary_name,
            FILE_DELETE_ACCESS,
            FILE_OPEN,
            FILE_OPTIONS,
            ExpectedKind::RegularFile,
            FileShareErrorCode::TempUnavailable,
            "temporary upload could not be opened for removal",
        ) {
            Ok(file) => file,
            Err(error) if error.is_io_kind(io::ErrorKind::NotFound) => return Ok(()),
            Err(error) => return Err(error),
        };
        set_delete_on_close(&file).map_err(|error| {
            FileShareError::io(
                FileShareErrorCode::TempUnavailable,
                "temporary upload could not be removed",
                error,
            )
        })
    }

    pub(super) fn cleanup_stale_temporary_files_windows(
        &self,
        now_unix_ms: u64,
        max_age_ms: u64,
    ) -> FileShareResult<u64> {
        let directory = match self.open_temporary_directory_windows() {
            Ok(directory) => directory,
            Err(error) if error.is_io_kind(io::ErrorKind::NotFound) => return Ok(0),
            Err(error) => return Err(error),
        };
        let entries = enumerate_raw(&directory)?;
        let mut removed = 0_u64;
        for entry in entries {
            if !is_generated_temporary_name(&entry.name)
                || entry.attributes & FILE_ATTRIBUTE_DIRECTORY != 0
                || entry.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
                || entry.attributes & FILE_ATTRIBUTE_DEVICE != 0
            {
                continue;
            }
            let modified_ms = entry.modified_at_unix_ms.unwrap_or(0);
            if now_unix_ms.saturating_sub(modified_ms) < max_age_ms {
                continue;
            }
            let file = match open_relative_checked(
                &directory,
                &entry.name,
                FILE_DELETE_ACCESS,
                FILE_OPEN,
                FILE_OPTIONS,
                ExpectedKind::RegularFile,
                FileShareErrorCode::TempUnavailable,
                "stale temporary upload could not be opened safely",
            ) {
                Ok(file) => file,
                Err(error) if error.is_io_kind(io::ErrorKind::NotFound) => continue,
                Err(error) => return Err(error),
            };
            match set_delete_on_close(&file) {
                Ok(()) => removed = removed.saturating_add(1),
                Err(error) => {
                    return Err(FileShareError::io(
                        FileShareErrorCode::TempUnavailable,
                        "stale temporary upload could not be removed",
                        error,
                    ))
                }
            }
        }
        Ok(removed)
    }

    fn open_windows_components(&self, components: &[&str]) -> FileShareResult<File> {
        let mut current = self.root_file.try_clone().map_err(|error| {
            FileShareError::io(
                FileShareErrorCode::Io,
                "shared root handle could not be cloned",
                error,
            )
        })?;
        for component in components {
            current = open_relative_checked(
                &current,
                component,
                DIRECTORY_TRAVERSE_ACCESS,
                FILE_OPEN,
                DIRECTORY_OPTIONS,
                ExpectedKind::Directory,
                FileShareErrorCode::Io,
                "shared directory component could not be opened safely",
            )?;
        }
        Ok(current)
    }

    fn ensure_temporary_directory_windows(&self) -> FileShareResult<File> {
        open_relative_checked(
            &self.root_file,
            FILE_TEMP_DIRECTORY,
            DIRECTORY_CREATE_ACCESS,
            FILE_OPEN_IF,
            DIRECTORY_OPTIONS,
            ExpectedKind::Directory,
            FileShareErrorCode::TempUnavailable,
            "temporary upload directory could not be created or opened",
        )
    }

    fn open_temporary_directory_windows(&self) -> FileShareResult<File> {
        open_relative_checked(
            &self.root_file,
            FILE_TEMP_DIRECTORY,
            DIRECTORY_LIST_ACCESS | DELETE,
            FILE_OPEN,
            DIRECTORY_OPTIONS,
            ExpectedKind::Directory,
            FileShareErrorCode::TempUnavailable,
            "temporary upload directory could not be opened",
        )
    }
}

fn scan_directory_anchored(
    directory: &File,
    depth: usize,
    limits: QuotaScanLimits,
    usage: &mut DirectoryUsage,
) -> FileShareResult<()> {
    for entry in enumerate_raw(directory)? {
        if depth == 0 && entry.name.eq_ignore_ascii_case(FILE_TEMP_DIRECTORY) {
            continue;
        }
        count_scanned_entry(limits, usage)?;
        if entry.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            usage.symlinks_skipped = usage.symlinks_skipped.saturating_add(1);
        } else if entry.attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
            if depth >= limits.max_depth {
                return Err(FileShareError::new(
                    FileShareErrorCode::ScanLimitExceeded,
                    "shared directory scan depth limit was reached",
                ));
            }
            let child = open_relative_checked(
                directory,
                &entry.name,
                DIRECTORY_LIST_ACCESS,
                FILE_OPEN,
                DIRECTORY_OPTIONS,
                ExpectedKind::Directory,
                FileShareErrorCode::Io,
                "shared directory changed while being scanned",
            )?;
            scan_directory_anchored(&child, depth + 1, limits, usage)?;
        } else if entry.attributes & FILE_ATTRIBUTE_DEVICE != 0 {
            usage.special_files_skipped = usage.special_files_skipped.saturating_add(1);
        } else {
            usage.total_bytes = usage.total_bytes.saturating_add(entry.size);
        }
    }
    Ok(())
}

fn enumerate_raw(directory: &File) -> FileShareResult<Vec<WindowsDirectoryEntry>> {
    let mut entries = Vec::new();
    let mut buffer = vec![0_u64; DIRECTORY_ENTRY_BYTES.div_ceil(size_of::<u64>())];
    let buffer_bytes = buffer.len() * size_of::<u64>();
    let mut restart_scan: BOOLEAN = 1;
    let mut scanned = 0_usize;
    loop {
        let mut status_block = zeroed_status_block();
        let status = unsafe {
            NtQueryDirectoryFile(
                directory.as_raw_handle() as HANDLE,
                null_mut(),
                None,
                null(),
                &mut status_block,
                buffer.as_mut_ptr().cast::<c_void>(),
                u32::try_from(buffer_bytes).expect("directory buffer length fits u32"),
                FileIdBothDirectoryInformation,
                FALSE as BOOLEAN,
                null(),
                restart_scan,
            )
        };
        restart_scan = 0;
        if status == STATUS_NO_MORE_FILES {
            break;
        }
        if status < 0 {
            return Err(nt_status_error(
                status,
                FileShareErrorCode::Io,
                "shared directory could not be listed",
            ));
        }
        if status != STATUS_BUFFER_OVERFLOW && status != 0 {
            return Err(nt_status_error(
                status,
                FileShareErrorCode::Io,
                "shared directory could not be listed",
            ));
        }
        let information = status_block.Information;
        if information == 0 {
            break;
        }
        if information > buffer_bytes {
            return Err(FileShareError::new(
                FileShareErrorCode::Io,
                "shared directory enumeration returned invalid bounds",
            ));
        }
        let base = buffer.as_ptr().cast::<u8>();
        let mut offset = 0_usize;
        loop {
            let minimum = offset_of!(
                windows_sys::Win32::Storage::FileSystem::FILE_ID_BOTH_DIR_INFO,
                FileName
            );
            if offset
                .checked_add(minimum)
                .is_none_or(|end| end > information)
            {
                return Err(FileShareError::new(
                    FileShareErrorCode::Io,
                    "shared directory enumeration returned an invalid entry",
                ));
            }
            let entry = unsafe { base.add(offset) }
                .cast::<windows_sys::Win32::Storage::FileSystem::FILE_ID_BOTH_DIR_INFO>();
            let file_name_length = unsafe { (*entry).FileNameLength as usize };
            if file_name_length % size_of::<u16>() != 0
                || offset
                    .checked_add(minimum)
                    .and_then(|start| start.checked_add(file_name_length))
                    .is_none_or(|end| end > information)
            {
                return Err(FileShareError::new(
                    FileShareErrorCode::Io,
                    "shared directory enumeration returned an invalid name",
                ));
            }
            let name_start = unsafe { addr_of!((*entry).FileName).cast::<u16>() };
            let name_units = unsafe { slice::from_raw_parts(name_start, file_name_length / 2) };
            let name = String::from_utf16(name_units).ok();
            let next_entry_offset = unsafe { (*entry).NextEntryOffset } as usize;
            let is_dot_entry = matches!(name.as_deref(), Some("." | ".."));
            if !is_dot_entry {
                // Count names that cannot be represented as Rust strings too,
                // so malformed Unicode cannot bypass the enumeration bound.
                scanned = scanned.saturating_add(1);
                if scanned > MAX_DIRECTORY_LIST_ENUMERATED_ENTRIES {
                    return Err(FileShareError::new(
                        FileShareErrorCode::ScanLimitExceeded,
                        "shared directory has too many entries to list safely",
                    ));
                }
                if let Some(name) = name {
                    let attributes = unsafe { (*entry).FileAttributes };
                    let size = unsafe { (*entry).EndOfFile };
                    entries.push(WindowsDirectoryEntry {
                        name,
                        attributes,
                        size: u64::try_from(size).unwrap_or(0),
                        modified_at_unix_ms: filetime_to_unix_ms(unsafe { (*entry).LastWriteTime }),
                    });
                }
            }
            if next_entry_offset == 0 {
                break;
            }
            if next_entry_offset < size_of::<u32>()
                || offset
                    .checked_add(next_entry_offset)
                    .is_none_or(|end| end > information)
            {
                return Err(FileShareError::new(
                    FileShareErrorCode::Io,
                    "shared directory enumeration returned an invalid offset",
                ));
            }
            offset += next_entry_offset;
        }
    }
    Ok(entries)
}

#[allow(clippy::too_many_arguments)]
fn open_relative_checked(
    parent: &File,
    name: &str,
    desired_access: u32,
    disposition: u32,
    create_options: u32,
    expected: ExpectedKind,
    fallback: FileShareErrorCode,
    message: &'static str,
) -> FileShareResult<File> {
    let file = open_relative(
        parent,
        name,
        desired_access,
        disposition,
        create_options,
        fallback,
        message,
    )?;
    let information = file_information(&file).map_err(|error| {
        FileShareError::io(
            fallback,
            "shared directory entry metadata is unavailable",
            error,
        )
    })?;
    if information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(FileShareError::new(
            FileShareErrorCode::SymlinkRejected,
            "shared path contains a symbolic link or reparse point",
        ));
    }
    match expected {
        ExpectedKind::Directory if information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 => {
            Err(FileShareError::new(
                FileShareErrorCode::SymlinkRejected,
                "shared directory component is not a directory",
            ))
        }
        ExpectedKind::RegularFile
            if information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 =>
        {
            Err(FileShareError::new(
                FileShareErrorCode::Io,
                "shared path is not a regular file",
            ))
        }
        _ => Ok(file),
    }
}

fn open_relative(
    parent: &File,
    name: &str,
    desired_access: u32,
    disposition: u32,
    create_options: u32,
    fallback: FileShareErrorCode,
    message: &'static str,
) -> FileShareResult<File> {
    validate_nt_name(name)?;
    let mut name: Vec<u16> = name.encode_utf16().collect();
    let name_bytes = name.len().checked_mul(size_of::<u16>()).ok_or_else(|| {
        FileShareError::new(FileShareErrorCode::PathRejected, "shared path is too large")
    })?;
    let unicode_name = UNICODE_STRING {
        Length: u16::try_from(name_bytes).map_err(|_| {
            FileShareError::new(FileShareErrorCode::PathRejected, "shared path is too large")
        })?,
        MaximumLength: u16::try_from(name_bytes).map_err(|_| {
            FileShareError::new(FileShareErrorCode::PathRejected, "shared path is too large")
        })?,
        Buffer: name.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: u32::try_from(size_of::<OBJECT_ATTRIBUTES>())
            .expect("OBJECT_ATTRIBUTES size fits u32"),
        RootDirectory: parent.as_raw_handle() as HANDLE,
        ObjectName: &unicode_name,
        Attributes: (OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE) as u32,
        SecurityDescriptor: null(),
        SecurityQualityOfService: null(),
    };
    let mut handle: HANDLE = null_mut();
    let mut status_block = zeroed_status_block();
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            desired_access,
            &attributes,
            &mut status_block,
            null(),
            FILE_ATTRIBUTE_NORMAL,
            SHARE_ALL,
            disposition,
            create_options,
            null(),
            0,
        )
    };
    if status < 0 {
        return Err(nt_status_error(status, fallback, message));
    }
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(FileShareError::new(
            fallback,
            "shared path open returned an invalid handle",
        ));
    }
    Ok(unsafe { File::from_raw_handle(handle as RawHandle) })
}

fn nt_status_error(
    status: NTSTATUS,
    fallback: FileShareErrorCode,
    message: &'static str,
) -> FileShareError {
    let code = match status {
        STATUS_OBJECT_NAME_COLLISION => FileShareErrorCode::TargetExists,
        STATUS_REPARSE_POINT_ENCOUNTERED => FileShareErrorCode::SymlinkRejected,
        STATUS_NOT_A_DIRECTORY => FileShareErrorCode::SymlinkRejected,
        _ => fallback,
    };
    let win32 = unsafe { windows_sys::Win32::Foundation::RtlNtStatusToDosError(status) };
    let source = if win32 == 0 {
        io::Error::from_raw_os_error(status)
    } else {
        io::Error::from_raw_os_error(win32 as i32)
    };
    FileShareError::io(code, message, source)
}

fn file_information(file: &File) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
    let mut information = unsafe { std::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
    let success =
        unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, &mut information) };
    if success == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(information)
}

fn file_identity(file: &File) -> io::Result<WindowsFileIdentity> {
    file_information(file).map(|information| identity_from_information(&information))
}

fn identity_from_information(information: &BY_HANDLE_FILE_INFORMATION) -> WindowsFileIdentity {
    WindowsFileIdentity {
        volume_serial: information.dwVolumeSerialNumber,
        file_index: (u64::from(information.nFileIndexHigh) << 32)
            | u64::from(information.nFileIndexLow),
    }
}

fn set_delete_on_close(file: &File) -> io::Result<()> {
    let information =
        windows_sys::Wdk::Storage::FileSystem::FILE_DISPOSITION_INFORMATION { DeleteFile: 1 };
    let mut status_block = zeroed_status_block();
    let status = unsafe {
        NtSetInformationFile(
            file.as_raw_handle() as HANDLE,
            &mut status_block,
            addr_of!(information).cast::<c_void>(),
            u32::try_from(size_of_val(&information))
                .expect("disposition information size fits u32"),
            FileDispositionInformation,
        )
    };
    if status < 0 {
        let win32 = unsafe { windows_sys::Win32::Foundation::RtlNtStatusToDosError(status) };
        Err(io::Error::from_raw_os_error(win32 as i32))
    } else {
        Ok(())
    }
}

fn zeroed_status_block() -> IO_STATUS_BLOCK {
    unsafe { std::mem::zeroed() }
}

fn validate_nt_name(name: &str) -> FileShareResult<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', '\0'])
        || name.encode_utf16().count() > u16::MAX as usize / 2
    {
        return Err(FileShareError::new(
            FileShareErrorCode::PathRejected,
            "shared path contains an invalid component",
        ));
    }
    Ok(())
}

fn filetime_to_unix_ms(filetime: i64) -> Option<u64> {
    const WINDOWS_TO_UNIX_EPOCH_100NS: u64 = 116_444_736_000_000_000;
    let filetime = u64::try_from(filetime).ok()?;
    Some(filetime.checked_sub(WINDOWS_TO_UNIX_EPOCH_100NS)? / 10_000)
}
