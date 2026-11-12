#![cfg(target_os = "linux")]

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::time::Instant;

use fuckipv6_client::file_share::{
    FileRuntime, FileShareCapability, TransferOperationCancellation, UploadAdmission,
    FILE_TEMP_DIRECTORY,
};
use fuckipv6_common::domain::{
    AccessContext, ContentHash, FileAcl, FileAclEffect, FileAclRule, FileAction, FileChunk,
    FileDeletePolicy, FileId, FileQuota, FileShare, FileShareId, FileShareMode, FileVersion,
    NetworkId, NetworkRole, NodeId, OperationId, SharedPath, SubjectSelector, SymlinkPolicy,
    TransferBinding, TransferDirection, TransferId, TransferManifest, TransferSession,
    FILE_SHARE_SCHEMA_VERSION,
};
use tokio::sync::watch;

const CONCURRENT_TRANSFERS: usize = 4;
const UPLOAD_WARMUP_BATCHES: usize = 16;
const UPLOAD_STRESS_BATCHES: usize = 1_024;
const DOWNLOAD_STRESS_BATCHES: usize = 512;
const COMPLETE_UPLOAD_EVERY: usize = 32;
const EXPECTED_ACTIVE_DESCRIPTORS: usize = CONCURRENT_TRANSFERS;
const RSS_RETENTION_LIMIT_KIB: u64 = 4 * 1024;
const RSS_GROWTH_LIMIT_KIB: u64 = 4 * 1024;

#[derive(Debug, Clone, Copy)]
struct ProcessResources {
    descriptors: usize,
    threads: usize,
    rss_kib: u64,
}

impl ProcessResources {
    fn sample() -> Self {
        Self {
            descriptors: count_entries("/proc/self/fd"),
            threads: count_entries("/proc/self/task"),
            rss_kib: resident_set_kib(),
        }
    }
}

fn count_entries(path: &str) -> usize {
    fs::read_dir(path)
        .unwrap_or_else(|error| panic!("read {path}: {error}"))
        .count()
}

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
        manifest(
            content,
            u64::try_from(content.len()).expect("content length"),
        ),
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
        manifest(
            content,
            u64::try_from(content.len()).expect("content length"),
        ),
        10,
    )
    .expect("download session")
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
        restored.descriptors, baseline.descriptors,
        "File handles did not return to the process baseline"
    );
    assert_eq!(
        restored.threads, baseline.threads,
        "File workload leaked threads"
    );
}

#[test]
fn file_runtime_stress_restores_linux_process_resources() {
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
        assert_eq!(
            loaded.descriptors,
            baseline.descriptors + EXPECTED_ACTIVE_DESCRIPTORS,
            "active File transfers must remain descriptor-bounded"
        );
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
        assert_eq!(
            loaded.descriptors,
            baseline.descriptors + EXPECTED_ACTIVE_DESCRIPTORS,
            "active File downloads must remain descriptor-bounded"
        );
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

    eprintln!(
        "File resource baseline: fds={}, threads={}, rss={} KiB; peak rss={} KiB; final rss={} KiB; elapsed={:?}",
        baseline.descriptors,
        baseline.threads,
        baseline.rss_kib,
        peak_rss_kib,
        final_rss_kib,
        started.elapsed()
    );

    assert_eq!(runtime.active_transfer_count(), 0);
    assert_eq!(staging_entries(temporary.path()), 0);
    let restored = ProcessResources::sample();
    assert_eq!(restored.descriptors, baseline.descriptors);
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
