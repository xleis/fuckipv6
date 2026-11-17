#![cfg(target_os = "linux")]

use std::collections::BTreeSet;
use std::fs::{self, FileTimes};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fuckipv6_client::file_share::{
    FileCapabilityState, FileRuntime, FileShareCapability, TransferOperationCancellation,
    UploadAdmission, FILE_TEMP_DIRECTORY,
};
use fuckipv6_common::domain::{
    AccessContext, ContentHash, FileAcl, FileAclEffect, FileAclRule, FileAction, FileChunk,
    FileDeletePolicy, FileId, FileQuota, FileShare, FileShareId, FileShareMode, NetworkId,
    NetworkRole, NodeId, OperationId, SharedPath, SubjectSelector, SymlinkPolicy, TransferBinding,
    TransferDirection, TransferId, TransferManifest, TransferSession, FILE_SHARE_SCHEMA_VERSION,
};
use tokio::sync::watch;

const CRASH_CHILD_ENV: &str = "FUCKIPV6_FILE_CRASH_CHILD";
const CRASH_CHILD_ROOT_ENV: &str = "FUCKIPV6_FILE_CRASH_ROOT";
const CRASH_CHILD_READY_ENV: &str = "FUCKIPV6_FILE_CRASH_READY";
const CRASH_RECOVERY_TEST_NAME: &str = "file_process_sigkill_staging_is_removed_on_restart";
const PARTIAL_UPLOAD: &[u8] = b"never-published-partial-upload";

struct ChildGuard {
    child: Child,
}

impl ChildGuard {
    fn new(child: Child) -> Self {
        Self { child }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn owner() -> NodeId {
    NodeId::from_public_key(b"crash-owner")
}

fn operation(sequence: u64) -> OperationId {
    OperationId::from_public_key(format!("crash-operation-{sequence}").as_bytes())
}

fn transfer(sequence: u64) -> TransferId {
    TransferId::from_public_key(format!("crash-transfer-{sequence}").as_bytes())
}

fn allow_acl() -> FileAcl {
    FileAcl {
        rules: vec![FileAclRule {
            id: "allow-members".into(),
            subjects: BTreeSet::from([SubjectSelector::AnyMember]),
            path_prefix: SharedPath::root(),
            actions: BTreeSet::from([FileAction::Upload]),
            effect: FileAclEffect::Allow,
        }],
    }
}

fn share() -> FileShare {
    FileShare {
        schema_version: FILE_SHARE_SCHEMA_VERSION,
        id: FileShareId::from_public_key(b"crash-share"),
        network_id: NetworkId::from_public_key(b"crash-network"),
        owner: owner(),
        display_name: "Crash recovery".into(),
        mode: FileShareMode::ReadWrite,
        revision: 1,
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

fn upload_session(share: &FileShare, sequence: u64) -> TransferSession {
    let manifest = TransferManifest {
        file_id: FileId::from_public_key(b"crash-file"),
        total_bytes: u64::try_from(PARTIAL_UPLOAD.len()).expect("partial length"),
        chunk_size: u64::try_from(PARTIAL_UPLOAD.len()).expect("partial length"),
        content_hash: ContentHash::from_bytes(PARTIAL_UPLOAD),
        chunks: vec![FileChunk {
            index: 0,
            offset: 0,
            length: u64::try_from(PARTIAL_UPLOAD.len()).expect("partial length"),
            checksum: ContentHash::from_bytes(PARTIAL_UPLOAD),
        }],
    };
    TransferSession::new(
        TransferBinding {
            transfer_id: transfer(sequence),
            operation_id: operation(sequence),
            share_id: share.id.clone(),
            actor: owner(),
            access_epoch: 1,
            path: SharedPath::parse("crashed-upload.bin").expect("upload path"),
            direction: TransferDirection::Upload,
            file_version: None,
            share_revision: share.revision,
            expires_at_unix_ms: u64::MAX,
        },
        manifest,
        10,
    )
    .expect("upload session")
}

fn staging_entries(root: &Path) -> usize {
    match fs::read_dir(root.join(FILE_TEMP_DIRECTORY)) {
        Ok(entries) => entries.count(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => panic!("read staging directory: {error}"),
    }
}

fn current_unix_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("current time after epoch")
            .as_millis(),
    )
    .expect("timestamp fits u64")
}

fn wait_for_crash_child(guard: &mut ChildGuard, ready: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if ready.exists() {
            return;
        }
        if let Some(status) = guard.child.try_wait().expect("poll crash child") {
            panic!("crash child exited before staging was ready: {status}");
        }
        assert!(
            Instant::now() < deadline,
            "crash child did not signal staging readiness"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn run_crash_child() {
    let root = std::env::var(CRASH_CHILD_ROOT_ENV).expect("crash child root");
    let ready = std::env::var(CRASH_CHILD_READY_ENV).expect("crash child ready path");
    let share = share();
    let access = context(&share);
    let runtime = FileRuntime::new(FileShareCapability::new());
    runtime
        .enable(&share, &owner(), &root, 1)
        .expect("enable crash child runtime");

    let (_tx, cancel) = watch::channel(false);
    let mut upload = runtime
        .begin_upload_cancellable(
            upload_session(&share, 99_999),
            &access,
            true,
            UploadAdmission::default(),
            TransferOperationCancellation::new(operation(99_999), cancel),
            20,
        )
        .expect("begin crash upload");
    upload
        .write_chunk(0, PARTIAL_UPLOAD, 21)
        .expect("write crash upload staging");
    assert_eq!(staging_entries(Path::new(&root)), 1);
    fs::write(ready, b"ready").expect("signal crash child readiness");

    loop {
        thread::sleep(Duration::from_secs(1));
    }
}

#[test]
fn file_process_sigkill_staging_is_removed_on_restart() {
    if std::env::var_os(CRASH_CHILD_ENV).is_some() {
        run_crash_child();
        return;
    }

    use std::os::unix::process::ExitStatusExt;

    let temporary = tempfile::tempdir().expect("tempdir");
    let control = tempfile::tempdir().expect("control tempdir");
    let ready = control.path().join("ready");
    let owner_file = temporary.path().join("owner-file.bin");
    let target = temporary.path().join("crashed-upload.bin");
    fs::write(&owner_file, b"owner-original").expect("owner file");
    assert!(!target.exists());

    let child = Command::new(std::env::current_exe().expect("current test executable"))
        .args(["--exact", CRASH_RECOVERY_TEST_NAME, "--nocapture"])
        .env(CRASH_CHILD_ENV, "1")
        .env(
            CRASH_CHILD_ROOT_ENV,
            temporary.path().to_str().expect("utf8 share root"),
        )
        .env(
            CRASH_CHILD_READY_ENV,
            ready.to_str().expect("utf8 ready path"),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn crash child");
    let mut child = ChildGuard::new(child);
    wait_for_crash_child(&mut child, &ready);

    assert_eq!(staging_entries(temporary.path()), 1);
    let staging_path = fs::read_dir(temporary.path().join(FILE_TEMP_DIRECTORY))
        .expect("read crash staging")
        .next()
        .expect("crash staging entry")
        .expect("crash staging path")
        .path();
    assert!(staging_path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("up-")));
    assert_eq!(
        fs::read(&staging_path).expect("partial staging"),
        PARTIAL_UPLOAD
    );

    child.child.kill().expect("SIGKILL crash child");
    let status = child.child.wait().expect("reap crash child");
    assert_eq!(status.signal(), Some(libc::SIGKILL));
    assert_eq!(
        staging_entries(temporary.path()),
        1,
        "SIGKILL must bypass TemporaryUpload::drop"
    );
    assert!(!target.exists(), "partial upload must never be published");
    assert_eq!(
        fs::read(&owner_file).expect("unchanged owner file"),
        b"owner-original"
    );

    fs::File::options()
        .write(true)
        .open(&staging_path)
        .expect("open abandoned staging")
        .set_times(FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(40 * 60)))
        .expect("age abandoned staging");

    let share = share();
    let restarted = FileRuntime::new(FileShareCapability::new());
    let snapshot = restarted
        .enable(
            &share,
            &owner(),
            temporary.path().to_str().expect("utf8 share root"),
            current_unix_ms(),
        )
        .expect("restart File runtime");

    assert_eq!(snapshot.capability.state, FileCapabilityState::Ready);
    assert_eq!(snapshot.active_transfers, 0);
    assert_eq!(staging_entries(temporary.path()), 0);
    assert!(!target.exists());
    assert_eq!(
        fs::read(&owner_file).expect("preserved owner file"),
        b"owner-original"
    );
}
