#![cfg(any(target_os = "linux", windows))]

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use fuckipv6_client::file_share::{
    FileRuntime, FileShareCapability, TransferOperationCancellation, UploadAdmission,
    FILE_TEMP_DIRECTORY,
};
use fuckipv6_common::domain::{
    AccessContext, ContentHash, FileAcl, FileAclEffect, FileAclRule, FileAction, FileChunk,
    FileDeletePolicy, FileId, FileQuota, FileShare, FileShareId, FileShareMode, FileVersion,
    NetworkId, NetworkRole, NodeId, OperationId, SharedPath, SubjectSelector, SymlinkPolicy,
    TransferBinding, TransferDirection, TransferId, TransferManifest, TransferSession,
    TransferState, FILE_SHARE_SCHEMA_VERSION,
};
use tokio::sync::watch;

#[cfg(windows)]
use windows_sys::Wdk::System::SystemInformation::{
    NtQuerySystemInformation, SystemProcessInformation,
};
#[cfg(windows)]
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE, STATUS_BUFFER_OVERFLOW,
    STATUS_BUFFER_TOO_SMALL, STATUS_INFO_LENGTH_MISMATCH,
};
#[cfg(windows)]
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
#[cfg(windows)]
use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetProcessHandleCount,
};
#[cfg(windows)]
use windows_sys::Win32::System::WindowsProgramming::SYSTEM_PROCESS_INFORMATION;

const CONCURRENT_TRANSFERS: usize = 4;
const UPLOAD_WARMUP_BATCHES: usize = 16;
const UPLOAD_STRESS_BATCHES: usize = 1_024;
const DOWNLOAD_STRESS_BATCHES: usize = 512;
const COMPLETE_UPLOAD_EVERY: usize = 32;
const EXPECTED_ACTIVE_DESCRIPTORS: usize = CONCURRENT_TRANSFERS;
const RSS_RETENTION_LIMIT_KIB: u64 = 4 * 1024;
const RSS_GROWTH_LIMIT_KIB: u64 = 4 * 1024;
const PERFORMANCE_LARGE_FILE_BYTES: usize = 64 * 1024 * 1024;
const PERFORMANCE_LARGE_CHUNK_BYTES: usize = 4 * 1024 * 1024;
const PERFORMANCE_MULTI_FILE_COUNT: usize = 4;
const PERFORMANCE_MULTI_FILE_BYTES: usize = 8 * 1024 * 1024;
const PERFORMANCE_MULTI_FILE_CHUNK_BYTES: usize = 1024 * 1024;
const PERFORMANCE_PAUSE_AFTER_CHUNKS: usize = 6;

static PROCESS_RESOURCE_PROBE_LOCK: Mutex<()> = Mutex::new(());

fn lock_process_resource_probe() -> std::sync::MutexGuard<'static, ()> {
    PROCESS_RESOURCE_PROBE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Debug, Clone, Copy)]
struct ProcessResources {
    handles: usize,
    handle_count_is_exact: bool,
    threads: usize,
    rss_kib: u64,
}

#[cfg(target_os = "linux")]
impl ProcessResources {
    fn sample() -> Self {
        Self {
            handles: count_entries("/proc/self/fd"),
            handle_count_is_exact: true,
            threads: count_entries("/proc/self/task"),
            rss_kib: resident_set_kib(),
        }
    }
}

#[cfg(windows)]
impl ProcessResources {
    fn sample() -> Self {
        let (handles, handle_count_is_exact) = process_handle_count();
        Self {
            handles,
            handle_count_is_exact,
            threads: process_thread_count(),
            rss_kib: process_working_set_kib(),
        }
    }
}

#[cfg(target_os = "linux")]
fn count_entries(path: &str) -> usize {
    fs::read_dir(path)
        .unwrap_or_else(|error| panic!("read {path}: {error}"))
        .count()
}

#[cfg(target_os = "linux")]
fn resident_set_kib() -> u64 {
    let status = fs::read_to_string("/proc/self/status").expect("read process status");
    let line = status
        .lines()
        .find(|line| line.starts_with("VmRSS:"))
        .expect("VmRSS in process status");
    let value = line
        .split_whitespace()
        .nth(1)
        .expect("VmRSS value")
        .parse::<u64>()
        .expect("numeric VmRSS");
    value
}

#[cfg(windows)]
fn process_handle_count() -> (usize, bool) {
    let mut count = 0_u32;
    let succeeded = unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut count) };
    assert_ne!(succeeded, 0, "GetProcessHandleCount failed");
    if count != 0 {
        return (usize::try_from(count).expect("handle count"), true);
    }

    // Wine implements GetProcessHandleCount as a successful zero-valued stub.
    // Fall back to the system process table so compatibility automation still
    // measures the per-process handle total. Wine's table does not reliably
    // reflect individual File handles, so active-transfer deltas from this
    // fallback are bounded rather than treated as exact native evidence.
    let handles = process_table_resources()
        .map(|resources| resources.handles)
        .expect("process table did not contain the current process");
    (handles, false)
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy)]
struct WindowsProcessTableResources {
    handles: usize,
}

#[cfg(windows)]
fn process_table_resources() -> Option<WindowsProcessTableResources> {
    const INITIAL_BYTES: usize = 1024 * 1024;
    const MAX_ATTEMPTS: usize = 8;
    let process_id = unsafe { GetCurrentProcessId() } as usize;
    let mut requested_bytes = INITIAL_BYTES;

    for _ in 0..MAX_ATTEMPTS {
        let words = requested_bytes.div_ceil(std::mem::size_of::<usize>());
        let mut buffer = vec![0_usize; words];
        let buffer_bytes = buffer
            .len()
            .checked_mul(std::mem::size_of::<usize>())
            .expect("system process table size");
        let mut returned_bytes = 0_u32;
        let status = unsafe {
            NtQuerySystemInformation(
                SystemProcessInformation,
                buffer.as_mut_ptr().cast(),
                u32::try_from(buffer_bytes).expect("system process table length"),
                &mut returned_bytes,
            )
        };
        if matches!(
            status,
            STATUS_INFO_LENGTH_MISMATCH | STATUS_BUFFER_TOO_SMALL | STATUS_BUFFER_OVERFLOW
        ) {
            let needed = usize::try_from(returned_bytes).unwrap_or(requested_bytes);
            requested_bytes = requested_bytes.max(needed).saturating_mul(2);
            continue;
        }
        assert_eq!(status, 0, "NtQuerySystemInformation failed: {status:#x}");

        let available_bytes = if returned_bytes == 0 {
            buffer_bytes
        } else {
            usize::try_from(returned_bytes)
                .expect("returned system process table length")
                .min(buffer_bytes)
        };
        let mut offset = 0_usize;
        while offset + std::mem::size_of::<SYSTEM_PROCESS_INFORMATION>() <= available_bytes {
            let entry = unsafe {
                std::ptr::read_unaligned(
                    buffer
                        .as_ptr()
                        .cast::<u8>()
                        .add(offset)
                        .cast::<SYSTEM_PROCESS_INFORMATION>(),
                )
            };
            if entry.UniqueProcessId as usize == process_id {
                return Some(WindowsProcessTableResources {
                    handles: usize::try_from(entry.HandleCount).expect("handle count"),
                });
            }
            let next_offset = usize::try_from(entry.NextEntryOffset).expect("next entry offset");
            if next_offset == 0 {
                break;
            }
            offset = offset.checked_add(next_offset)?;
        }
        return None;
    }
    None
}

#[cfg(windows)]
fn process_thread_count() -> usize {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    assert_ne!(
        snapshot,
        INVALID_HANDLE_VALUE,
        "CreateToolhelp32Snapshot failed: {}",
        unsafe { GetLastError() }
    );
    let process_id = unsafe { GetCurrentProcessId() };
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = u32::try_from(std::mem::size_of::<THREADENTRY32>()).expect("thread entry size");
    let mut count = 0_usize;
    let mut present = unsafe { Thread32First(snapshot, &mut entry) };
    assert_ne!(present, 0, "Thread32First failed: {}", unsafe {
        GetLastError()
    });
    while present != 0 {
        if entry.th32OwnerProcessID == process_id {
            count = count.saturating_add(1);
        }
        present = unsafe { Thread32Next(snapshot, &mut entry) };
    }
    let error = unsafe { GetLastError() };
    let closed = unsafe { CloseHandle(snapshot) };
    assert_ne!(closed, 0, "CloseHandle for thread snapshot failed");
    assert_eq!(
        error, ERROR_NO_MORE_FILES,
        "Thread32Next failed before enumerating all threads"
    );
    count
}

#[cfg(windows)]
fn process_working_set_kib() -> u64 {
    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    counters.cb =
        u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS>()).expect("memory counter size");
    let succeeded = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
    };
    assert_ne!(succeeded, 0, "GetProcessMemoryInfo failed: {}", unsafe {
        GetLastError()
    });
    u64::try_from(counters.WorkingSetSize / 1024).expect("working set KiB")
}

fn owner() -> NodeId {
    NodeId::from_public_key(b"resource-owner")
}

fn operation(sequence: u64) -> OperationId {
    OperationId::from_public_key(format!("resource-operation-{sequence}").as_bytes())
}

fn transfer(sequence: u64) -> TransferId {
    TransferId::from_public_key(format!("resource-transfer-{sequence}").as_bytes())
}

fn allow_acl() -> FileAcl {
    FileAcl {
        rules: vec![FileAclRule {
            id: "allow-members".into(),
            subjects: BTreeSet::from([SubjectSelector::AnyMember]),
            path_prefix: SharedPath::root(),
            actions: BTreeSet::from([
                FileAction::List,
                FileAction::ReadMetadata,
                FileAction::Download,
                FileAction::Upload,
                FileAction::CreateDirectory,
                FileAction::Rename,
                FileAction::Delete,
            ]),
            effect: FileAclEffect::Allow,
        }],
    }
}

fn share() -> FileShare {
    let quota = FileQuota {
        max_concurrent_transfers: 4,
        max_concurrent_transfers_per_member: 4,
        max_concurrent_transfers_per_directory: 4,
        ..FileQuota::default()
    };
    FileShare {
        schema_version: FILE_SHARE_SCHEMA_VERSION,
        id: FileShareId::from_public_key(b"resource-share"),
        network_id: NetworkId::from_public_key(b"resource-network"),
        owner: owner(),
        display_name: "Resource baseline".into(),
        mode: FileShareMode::ReadWrite,
        revision: 1,
        network_acl: allow_acl(),
        local_acl: allow_acl(),
        quota,
        symlink_policy: SymlinkPolicy::Reject,
        concurrent_name_policy: fuckipv6_common::domain::ConcurrentNamePolicy::Reject,
        delete_policy: FileDeletePolicy::default(),
        audit_enabled: true,
        created_at_unix_ms: 1,
        updated_at_unix_ms: 1,
    }
}

fn context(share: &FileShare) -> AccessContext {
    AccessContext {
        network_id: share.network_id.clone(),
        actor: owner(),
        role: NetworkRole::Owner,
        groups: BTreeSet::new(),
        membership_active: true,
        recent_reauthentication: true,
        emergency_coordinator: false,
    }
}

fn manifest(content: &[u8], chunk_size: u64) -> TransferManifest {
    let chunks = if content.is_empty() {
        Vec::new()
    } else {
        content
            .chunks(usize::try_from(chunk_size).expect("chunk size"))
            .enumerate()
            .map(|(index, bytes)| FileChunk {
                index: u32::try_from(index).expect("chunk index"),
                offset: u64::try_from(index).expect("offset") * chunk_size,
                length: u64::try_from(bytes.len()).expect("chunk length"),
                checksum: ContentHash::from_bytes(bytes),
            })
            .collect()
    };
    TransferManifest {
        file_id: FileId::from_public_key(b"resource-file"),
        total_bytes: u64::try_from(content.len()).expect("content length"),
        chunk_size,
        content_hash: ContentHash::from_bytes(content),
        chunks,
    }
}

fn upload_session(share: &FileShare, sequence: u64, path: &str, content: &[u8]) -> TransferSession {
    let chunk_size = u64::try_from(content.len()).expect("content length");
    upload_session_with_chunk_size(share, sequence, path, content, chunk_size)
}

fn upload_session_with_chunk_size(
    share: &FileShare,
    sequence: u64,
    path: &str,
    content: &[u8],
    chunk_size: u64,
) -> TransferSession {
    TransferSession::new(
        TransferBinding {
            transfer_id: transfer(sequence),
            operation_id: operation(sequence),
            share_id: share.id.clone(),
            actor: owner(),
            access_epoch: 1,
            path: SharedPath::parse(path).expect("upload path"),
            direction: TransferDirection::Upload,
            file_version: None,
            share_revision: share.revision,
            expires_at_unix_ms: u64::MAX,
        },
        manifest(content, chunk_size),
        10,
    )
    .expect("upload session")
}

fn download_session(
    share: &FileShare,
    sequence: u64,
    path: &str,
    content: &[u8],
) -> TransferSession {
    let chunk_size = u64::try_from(content.len()).expect("content length");
    download_session_with_chunk_size(share, sequence, path, content, chunk_size)
}

fn download_session_with_chunk_size(
    share: &FileShare,
    sequence: u64,
    path: &str,
    content: &[u8],
    chunk_size: u64,
) -> TransferSession {
    TransferSession::new(
        TransferBinding {
            transfer_id: transfer(sequence),
            operation_id: operation(sequence),
            share_id: share.id.clone(),
            actor: owner(),
            access_epoch: 1,
            path: SharedPath::parse(path).expect("download path"),
            direction: TransferDirection::Download,
            file_version: Some(FileVersion {
                content_hash: ContentHash::from_bytes(content),
                size: u64::try_from(content.len()).expect("content length"),
                revision: 1,
            }),
            share_revision: share.revision,
            expires_at_unix_ms: u64::MAX,
        },
        manifest(content, chunk_size),
        10,
    )
    .expect("download session")
}

fn patterned_content(seed: u8, length: usize) -> Vec<u8> {
    let mut block = vec![0_u8; 64 * 1024];
    for (index, byte) in block.iter_mut().enumerate() {
        *byte = seed
            .wrapping_add(u8::try_from(index % 251).expect("block index"))
            .wrapping_mul(31);
    }
    let mut content = Vec::with_capacity(length);
    while content.len() < length {
        let remaining = length - content.len();
        content.extend_from_slice(&block[..remaining.min(block.len())]);
    }
    content
}

fn file_hash(path: &Path) -> ContentHash {
    let mut file = File::open(path).expect("open hashed file");
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).expect("read hashed file");
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    ContentHash::from_digest_bytes(*hasher.finalize().as_bytes())
}

fn mib_per_second(bytes: u64, elapsed: Duration) -> f64 {
    let mebibytes = bytes as f64 / (1024.0 * 1024.0);
    mebibytes / elapsed.as_secs_f64().max(f64::EPSILON)
}

fn staging_entries(root: &Path) -> usize {
    match fs::read_dir(root.join(FILE_TEMP_DIRECTORY)) {
        Ok(entries) => entries.count(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => panic!("read staging directory: {error}"),
    }
}

fn assert_restored(runtime: &FileRuntime, root: &Path, baseline: ProcessResources) {
    assert_eq!(runtime.active_transfer_count(), 0);
    assert_eq!(staging_entries(root), 0);
    let restored = ProcessResources::sample();
    assert_eq!(
        restored.handles, baseline.handles,
        "File handles did not return to the process baseline"
    );
    assert_eq!(
        restored.threads, baseline.threads,
        "File workload leaked threads"
    );
}

#[test]
fn file_runtime_stress_restores_process_resources() {
    let _resource_probe = lock_process_resource_probe();
    let temporary = tempfile::tempdir().expect("tempdir");
    let source = vec![0x5a_u8; 64 * 1024];
    fs::write(temporary.path().join("source.bin"), &source).expect("source file");
    let share = share();
    let access = context(&share);
    let runtime = FileRuntime::new(FileShareCapability::new());
    runtime
        .enable(
            &share,
            &owner(),
            temporary.path().to_str().expect("utf8 path"),
            1,
        )
        .expect("enable File runtime");

    let upload_content = vec![0x3c_u8; 4 * 1024];
    for batch in 0..UPLOAD_WARMUP_BATCHES {
        let mut active = Vec::with_capacity(CONCURRENT_TRANSFERS);
        for slot in 0..CONCURRENT_TRANSFERS {
            let sequence = u64::try_from(batch * CONCURRENT_TRANSFERS + slot).expect("sequence");
            let (_tx, cancel) = watch::channel(false);
            active.push(
                runtime
                    .begin_upload_cancellable(
                        upload_session(&share, sequence, "warmup.bin", &upload_content),
                        &access,
                        true,
                        UploadAdmission::default(),
                        TransferOperationCancellation::new(operation(sequence), cancel),
                        20,
                    )
                    .expect("warmup upload"),
            );
        }
        drop(active);
    }
    assert_eq!(runtime.active_transfer_count(), 0);
    assert_eq!(staging_entries(temporary.path()), 0);

    let baseline = ProcessResources::sample();
    let started = Instant::now();
    let mut rss_after_batch = Vec::with_capacity(UPLOAD_STRESS_BATCHES + DOWNLOAD_STRESS_BATCHES);

    for batch in 0..UPLOAD_STRESS_BATCHES {
        let mut active = Vec::with_capacity(CONCURRENT_TRANSFERS);
        for slot in 0..CONCURRENT_TRANSFERS {
            let sequence = u64::try_from(
                UPLOAD_WARMUP_BATCHES * CONCURRENT_TRANSFERS + batch * CONCURRENT_TRANSFERS + slot,
            )
            .expect("sequence");
            let path = if batch % COMPLETE_UPLOAD_EVERY == 0 {
                format!("published-{sequence}.bin")
            } else {
                "scratch.bin".into()
            };
            let (_tx, cancel) = watch::channel(false);
            let mut upload = runtime
                .begin_upload_cancellable(
                    upload_session(&share, sequence, &path, &upload_content),
                    &access,
                    true,
                    UploadAdmission::default(),
                    TransferOperationCancellation::new(operation(sequence), cancel),
                    20,
                )
                .expect("stress upload");
            assert!(upload
                .write_chunk(0, &upload_content, 21)
                .expect("stage upload chunk"));
            active.push(upload);
        }
        assert_eq!(
            runtime.active_transfer_count(),
            u32::try_from(active.len()).expect("active count")
        );
        assert_eq!(
            staging_entries(temporary.path()),
            CONCURRENT_TRANSFERS,
            "each admitted upload must own exactly one staging entry"
        );
        let loaded = ProcessResources::sample();
        if baseline.handle_count_is_exact {
            assert_eq!(
                loaded.handles,
                baseline.handles + EXPECTED_ACTIVE_DESCRIPTORS,
                "active File transfers must remain handle-bounded"
            );
        } else {
            assert!(
                loaded.handles <= baseline.handles + EXPECTED_ACTIVE_DESCRIPTORS,
                "active File transfers exceeded the bounded handle allowance"
            );
        }
        assert_eq!(loaded.threads, baseline.threads);

        if batch % COMPLETE_UPLOAD_EVERY == 0 {
            for mut upload in active {
                let progress = upload.finish(22).expect("finish stress upload");
                assert!(progress.state.is_terminal());
                let published = temporary
                    .path()
                    .join(upload.session().binding.path.as_str());
                assert_eq!(
                    fs::read(&published).expect("published upload"),
                    upload_content
                );
                fs::remove_file(published).expect("remove published upload");
            }
        } else {
            drop(active);
        }
        assert_restored(&runtime, temporary.path(), baseline);
        rss_after_batch.push(ProcessResources::sample().rss_kib);
    }

    for batch in 0..DOWNLOAD_STRESS_BATCHES {
        let mut active = Vec::with_capacity(CONCURRENT_TRANSFERS);
        for slot in 0..CONCURRENT_TRANSFERS {
            let sequence = u64::try_from(
                UPLOAD_WARMUP_BATCHES * CONCURRENT_TRANSFERS
                    + (UPLOAD_STRESS_BATCHES + batch) * CONCURRENT_TRANSFERS
                    + slot,
            )
            .expect("sequence");
            let session = download_session(&share, sequence, "source.bin", &source);
            let (_tx, cancel) = watch::channel(false);
            active.push(
                runtime
                    .begin_download_cancellable(
                        session,
                        &access,
                        true,
                        TransferOperationCancellation::new(operation(sequence), cancel),
                        20,
                    )
                    .expect("stress download"),
            );
        }
        assert_eq!(
            runtime.active_transfer_count(),
            u32::try_from(active.len()).expect("active count")
        );
        assert_eq!(staging_entries(temporary.path()), 0);
        let loaded = ProcessResources::sample();
        if baseline.handle_count_is_exact {
            assert_eq!(
                loaded.handles,
                baseline.handles + EXPECTED_ACTIVE_DESCRIPTORS,
                "active File downloads must remain handle-bounded"
            );
        } else {
            assert!(
                loaded.handles <= baseline.handles + EXPECTED_ACTIVE_DESCRIPTORS,
                "active File downloads exceeded the bounded handle allowance"
            );
        }
        assert_eq!(loaded.threads, baseline.threads);

        for mut download in active {
            let mut received = Vec::with_capacity(source.len());
            while let Some(chunk) = download.read_next(21).expect("read download chunk") {
                received.extend_from_slice(&chunk);
            }
            assert_eq!(received, source);
            download.finish(22).expect("finish download");
        }
        assert_restored(&runtime, temporary.path(), baseline);
        rss_after_batch.push(ProcessResources::sample().rss_kib);
    }

    let peak_rss_kib = rss_after_batch.iter().copied().max().expect("RSS samples");
    let final_rss_kib = ProcessResources::sample().rss_kib;
    let first_half_max = rss_after_batch[..rss_after_batch.len() / 2]
        .iter()
        .copied()
        .max()
        .expect("first-half RSS");
    let second_half_max = rss_after_batch[rss_after_batch.len() / 2..]
        .iter()
        .copied()
        .max()
        .expect("second-half RSS");

    let handle_label = if cfg!(windows) { "handles" } else { "fds" };
    let handle_probe = if baseline.handle_count_is_exact {
        "native"
    } else {
        "system-table-fallback"
    };
    eprintln!(
        "File resource baseline: {handle_label}={}, handle_probe={handle_probe}, threads={}, rss={} KiB; peak rss={} KiB; final rss={} KiB; elapsed={:?}",
        baseline.handles,
        baseline.threads,
        baseline.rss_kib,
        peak_rss_kib,
        final_rss_kib,
        started.elapsed()
    );

    assert_eq!(runtime.active_transfer_count(), 0);
    assert_eq!(staging_entries(temporary.path()), 0);
    let restored = ProcessResources::sample();
    assert_eq!(restored.handles, baseline.handles);
    assert_eq!(restored.threads, baseline.threads);
    assert!(
        final_rss_kib <= baseline.rss_kib + RSS_RETENTION_LIMIT_KIB,
        "final RSS {final_rss_kib} KiB exceeds baseline {} KiB by more than {} KiB",
        baseline.rss_kib,
        RSS_RETENTION_LIMIT_KIB
    );
    assert!(
        second_half_max <= first_half_max + RSS_GROWTH_LIMIT_KIB,
        "RSS kept growing: first-half max={first_half_max} KiB, second-half max={second_half_max} KiB"
    );
}

#[test]
fn file_transfer_large_multifile_and_resume_report() {
    let _resource_probe = lock_process_resource_probe();
    let temporary = tempfile::tempdir().expect("tempdir");
    let share = share();
    let access = context(&share);
    let runtime = FileRuntime::new(FileShareCapability::new());
    runtime
        .enable(
            &share,
            &owner(),
            temporary.path().to_str().expect("utf8 path"),
            1,
        )
        .expect("enable File runtime");

    let baseline = ProcessResources::sample();

    // One large upload pauses after a bounded prefix and resumes at the next
    // manifest chunk. The final hash proves no chunk was skipped or replayed.
    let large_upload = patterned_content(0x31, PERFORMANCE_LARGE_FILE_BYTES);
    let expected_upload_hash = ContentHash::from_bytes(&large_upload);
    let upload_started = Instant::now();
    let (_upload_cancel_tx, upload_cancel) = watch::channel(false);
    let mut upload = runtime
        .begin_upload_cancellable(
            upload_session_with_chunk_size(
                &share,
                1,
                "large-upload.bin",
                &large_upload,
                u64::try_from(PERFORMANCE_LARGE_CHUNK_BYTES).expect("upload chunk size"),
            ),
            &access,
            true,
            UploadAdmission::default(),
            TransferOperationCancellation::new(operation(1), upload_cancel),
            10,
        )
        .expect("begin large upload");
    for (index, chunk) in large_upload
        .chunks(PERFORMANCE_LARGE_CHUNK_BYTES)
        .enumerate()
    {
        if index == PERFORMANCE_PAUSE_AFTER_CHUNKS {
            upload.pause(11).expect("pause large upload");
            assert_eq!(upload.session().state, TransferState::Paused);
            upload
                .resume(&access, true, 12)
                .expect("resume large upload");
        }
        assert!(upload
            .write_chunk(u32::try_from(index).expect("upload chunk index"), chunk, 13)
            .expect("stage large upload chunk"));
    }
    let upload_progress = upload.finish(14).expect("finish large upload");
    let upload_elapsed = upload_started.elapsed();
    assert_eq!(upload_progress.state, TransferState::Completed);
    assert_eq!(
        upload_progress.transferred_bytes,
        u64::try_from(PERFORMANCE_LARGE_FILE_BYTES).expect("large file size")
    );
    assert_eq!(
        file_hash(&temporary.path().join("large-upload.bin")),
        expected_upload_hash
    );
    drop(upload);
    drop(large_upload);
    assert_eq!(runtime.active_transfer_count(), 0);
    assert_eq!(staging_entries(temporary.path()), 0);

    // The download path reopens and reauthorizes the source after pause. Hash
    // the delivered chunks directly so the test also exercises bounded memory
    // streaming rather than materializing a second complete file in memory.
    let large_download = patterned_content(0x72, PERFORMANCE_LARGE_FILE_BYTES);
    let expected_download_hash = ContentHash::from_bytes(&large_download);
    fs::write(temporary.path().join("large-source.bin"), &large_download)
        .expect("large download source");
    let download_started = Instant::now();
    let (_download_cancel_tx, download_cancel) = watch::channel(false);
    let mut download = runtime
        .begin_download_cancellable(
            download_session_with_chunk_size(
                &share,
                2,
                "large-source.bin",
                &large_download,
                u64::try_from(PERFORMANCE_LARGE_CHUNK_BYTES).expect("download chunk size"),
            ),
            &access,
            true,
            TransferOperationCancellation::new(operation(2), download_cancel),
            20,
        )
        .expect("begin large download");
    let mut download_hasher = blake3::Hasher::new();
    let mut downloaded_chunks = 0_usize;
    loop {
        if downloaded_chunks == PERFORMANCE_PAUSE_AFTER_CHUNKS {
            download.pause(21).expect("pause large download");
            assert_eq!(download.session().state, TransferState::Paused);
            download
                .resume(&access, true, 22)
                .expect("resume large download");
        }
        let Some(chunk) = download.read_next(23).expect("read large download") else {
            break;
        };
        download_hasher.update(&chunk);
        downloaded_chunks = downloaded_chunks.saturating_add(1);
    }
    let download_progress = download.finish(24).expect("finish large download");
    let download_elapsed = download_started.elapsed();
    assert_eq!(download_progress.state, TransferState::Completed);
    assert_eq!(
        ContentHash::from_digest_bytes(*download_hasher.finalize().as_bytes()),
        expected_download_hash
    );
    assert_eq!(
        download_progress.transferred_bytes,
        u64::try_from(PERFORMANCE_LARGE_FILE_BYTES).expect("large file size")
    );
    drop(download);
    drop(large_download);
    assert_eq!(runtime.active_transfer_count(), 0);

    // Four independent files are advanced round-robin while all admissions
    // overlap. This exercises multiple handles, manifests and hashers without
    // serializing the workload into four unrelated single-file tests.
    let mut multi_file_transfers = Vec::with_capacity(PERFORMANCE_MULTI_FILE_COUNT);
    let multi_file_started = Instant::now();
    for index in 0..PERFORMANCE_MULTI_FILE_COUNT {
        let sequence = u64::try_from(index).expect("multi-file index") + 10;
        let path = format!("multi-{index}.bin");
        let content = patterned_content(
            u8::try_from(index).expect("multi-file seed") + 0x90,
            PERFORMANCE_MULTI_FILE_BYTES,
        );
        fs::write(temporary.path().join(&path), &content).expect("multi-file source");
        let expected = ContentHash::from_bytes(&content);
        let (_cancel_tx, cancel) = watch::channel(false);
        let transfer = runtime
            .begin_download_cancellable(
                download_session_with_chunk_size(
                    &share,
                    sequence,
                    &path,
                    &content,
                    u64::try_from(PERFORMANCE_MULTI_FILE_CHUNK_BYTES)
                        .expect("multi-file chunk size"),
                ),
                &access,
                true,
                TransferOperationCancellation::new(operation(sequence), cancel),
                30,
            )
            .expect("begin multi-file download");
        multi_file_transfers.push((transfer, expected, blake3::Hasher::new()));
    }
    assert_eq!(
        runtime.active_transfer_count(),
        u32::try_from(PERFORMANCE_MULTI_FILE_COUNT).expect("multi-file count")
    );
    let mut complete = [false; PERFORMANCE_MULTI_FILE_COUNT];
    let mut completed = 0_usize;
    while completed < PERFORMANCE_MULTI_FILE_COUNT {
        for (index, (transfer, expected, hasher)) in multi_file_transfers.iter_mut().enumerate() {
            if complete[index] {
                continue;
            }
            match transfer.read_next(31).expect("read multi-file chunk") {
                Some(chunk) => {
                    hasher.update(&chunk);
                }
                None => {
                    assert_eq!(
                        ContentHash::from_digest_bytes(*hasher.finalize().as_bytes()),
                        *expected
                    );
                    complete[index] = true;
                    completed = completed.saturating_add(1);
                }
            }
        }
    }
    for (transfer, _, _) in &mut multi_file_transfers {
        assert_eq!(
            transfer
                .finish(32)
                .expect("finish multi-file download")
                .state,
            TransferState::Completed
        );
    }
    let multi_file_elapsed = multi_file_started.elapsed();
    drop(multi_file_transfers);
    assert_eq!(runtime.active_transfer_count(), 0);
    assert_eq!(staging_entries(temporary.path()), 0);

    let restart_resources = ProcessResources::sample();
    let handle_label = if cfg!(windows) { "handles" } else { "fds" };
    eprintln!(
        "File performance {}: large_upload={} MiB in {:?} ({:.1} MiB/s, pause_after={} chunks, resumed_bytes={}); large_download={} MiB in {:?} ({:.1} MiB/s, pause_after={} chunks, resumed_bytes={}); multi_file={}x{} MiB interleaved in {:?} ({:.1} MiB/s); {} {}->{}, threads {}->{}",
        std::env::consts::OS,
        PERFORMANCE_LARGE_FILE_BYTES / (1024 * 1024),
        upload_elapsed,
        mib_per_second(
            u64::try_from(PERFORMANCE_LARGE_FILE_BYTES).expect("large upload size"),
            upload_elapsed
        ),
        PERFORMANCE_PAUSE_AFTER_CHUNKS,
        PERFORMANCE_LARGE_FILE_BYTES - PERFORMANCE_PAUSE_AFTER_CHUNKS * PERFORMANCE_LARGE_CHUNK_BYTES,
        PERFORMANCE_LARGE_FILE_BYTES / (1024 * 1024),
        download_elapsed,
        mib_per_second(
            u64::try_from(PERFORMANCE_LARGE_FILE_BYTES).expect("large download size"),
            download_elapsed
        ),
        PERFORMANCE_PAUSE_AFTER_CHUNKS,
        PERFORMANCE_LARGE_FILE_BYTES - PERFORMANCE_PAUSE_AFTER_CHUNKS * PERFORMANCE_LARGE_CHUNK_BYTES,
        PERFORMANCE_MULTI_FILE_COUNT,
        PERFORMANCE_MULTI_FILE_BYTES / (1024 * 1024),
        multi_file_elapsed,
        mib_per_second(
            u64::try_from(PERFORMANCE_MULTI_FILE_COUNT * PERFORMANCE_MULTI_FILE_BYTES)
                .expect("multi-file bytes"),
            multi_file_elapsed
        ),
        handle_label,
        baseline.handles,
        restart_resources.handles,
        baseline.threads,
        restart_resources.threads,
    );
    assert_restored(&runtime, temporary.path(), baseline);
}
