use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use fuckipv6_common::domain::{
    AccessContext, AuditEntry, AuditId, AuditLog, AuditOutcome, DomainError, FileAccessDenial,
    FileAccessRequest, FileAction, FileQuotaError, FileQuotaUsage, FileShare, NetworkId, NodeId,
};
use tokio::sync::watch;

use super::error::{FileShareError, FileShareErrorCode, FileShareResult};
use super::quota::{self, UploadQuotaRequest};
use super::root::LocalShareRoot;

/// Local runtime state for one owner-selected File share. Keeping capability,
/// authorization, and transfer state together prevents a disabled share from
/// leaking stale metadata while its filesystem handle is still being torn down.
#[derive(Clone)]
pub struct FileShareCapability {
    inner: Arc<Inner>,
}

struct Inner {
    state: RwLock<Option<EnabledFileShare>>,
    last_network_id: RwLock<Option<NetworkId>>,
    epoch_tx: watch::Sender<u64>,
    audit: Mutex<AuditLog>,
    audit_sequence: AtomicU64,
}

struct EnabledFileShare {
    root: LocalShareRoot,
    share: FileShare,
    access_epoch: u64,
}

impl std::fmt::Debug for FileShareCapability {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FileShareCapability")
            .field("snapshot", &self.snapshot())
            .finish()
    }
}

impl Default for FileShareCapability {
    fn default() -> Self {
        Self::new()
    }
}

impl FileShareCapability {
    pub fn new() -> Self {
        let (epoch_tx, _) = watch::channel(0);
        Self {
            inner: Arc::new(Inner {
                state: RwLock::new(None),
                last_network_id: RwLock::new(None),
                epoch_tx,
                audit: Mutex::new(AuditLog::default()),
                audit_sequence: AtomicU64::new(0),
            }),
        }
    }

    /// Enable or replace the local File share. The owner identity must match
    /// the share record, and the audit record is committed before the new root
    /// becomes visible to transfer code.
    pub fn enable(
        &self,
        share: &FileShare,
        actor: &NodeId,
        root_reference: &str,
        now_unix_ms: u64,
    ) -> Result<FileCapabilitySnapshot, FileCapabilityError> {
        let Inner {
            state,
            epoch_tx,
            audit,
            audit_sequence,
            ..
        } = &*self.inner;
        let mut state = state
            .write()
            .map_err(|_| FileCapabilityError::Unavailable)?;

        if actor != &share.owner {
            record_audit(
                audit,
                audit_sequence,
                AuditEvent {
                    actor,
                    share,
                    action: "file.share.enable",
                    outcome: AuditOutcome::Denied,
                    now_unix_ms,
                    summary: "Only the share owner may enable File",
                    details: BTreeMap::from([("reason_code".into(), "file_owner_required".into())]),
                },
            )?;
            return Err(FileCapabilityError::OwnerRequired);
        }
        if let Err(error) = share.validate() {
            record_audit(
                audit,
                audit_sequence,
                AuditEvent {
                    actor,
                    share,
                    action: "file.share.enable",
                    outcome: AuditOutcome::Failed,
                    now_unix_ms,
                    summary: "File capability configuration was invalid",
                    details: BTreeMap::from([("reason_code".into(), error.code().into())]),
                },
            )?;
            return Err(FileCapabilityError::InvalidShare(error));
        }

        let root = match LocalShareRoot::select(root_reference, share.symlink_policy) {
            Ok(root) => root,
            Err(error) => {
                record_audit(
                    audit,
                    audit_sequence,
                    AuditEvent {
                        actor,
                        share,
                        action: "file.share.enable",
                        outcome: AuditOutcome::Failed,
                        now_unix_ms,
                        summary: "File share root could not be enabled",
                        details: BTreeMap::from([("reason_code".into(), error.code_str().into())]),
                    },
                )?;
                return Err(FileCapabilityError::Filesystem(error));
            }
        };
        let access_epoch = next_epoch(state.as_ref().map_or(0, |enabled| enabled.access_epoch));
        record_audit(
            audit,
            audit_sequence,
            AuditEvent {
                actor,
                share,
                action: "file.share.enable",
                outcome: AuditOutcome::Succeeded,
                now_unix_ms,
                summary: "File capability enabled",
                details: BTreeMap::from([
                    ("access_epoch".into(), access_epoch.to_string()),
                    ("share_revision".into(), share.revision.to_string()),
                ]),
            },
        )?;
        *state = Some(EnabledFileShare {
            root,
            share: share.clone(),
            access_epoch,
        });
        *self
            .inner
            .last_network_id
            .write()
            .map_err(|_| FileCapabilityError::Unavailable)? = Some(share.network_id.clone());
        epoch_tx.send_replace(access_epoch);
        drop(state);
        Ok(self.snapshot())
    }

    /// Disable the local capability. Existing capability permits observe the
    /// new access epoch and must stop at their next safe transfer boundary.
    pub fn disable(
        &self,
        actor: &NodeId,
        now_unix_ms: u64,
    ) -> Result<FileCapabilitySnapshot, FileCapabilityError> {
        let Inner {
            state,
            epoch_tx,
            audit,
            audit_sequence,
            ..
        } = &*self.inner;
        let mut state = state
            .write()
            .map_err(|_| FileCapabilityError::Unavailable)?;
        let enabled = state.as_ref().ok_or(FileCapabilityError::Disabled)?;
        if actor != &enabled.share.owner {
            record_audit(
                audit,
                audit_sequence,
                AuditEvent {
                    actor,
                    share: &enabled.share,
                    action: "file.share.disable",
                    outcome: AuditOutcome::Denied,
                    now_unix_ms,
                    summary: "Only the share owner may disable File",
                    details: BTreeMap::from([("reason_code".into(), "file_owner_required".into())]),
                },
            )?;
            return Err(FileCapabilityError::OwnerRequired);
        }
        let access_epoch = next_epoch(enabled.access_epoch);
        record_audit(
            audit,
            audit_sequence,
            AuditEvent {
                actor,
                share: &enabled.share,
                action: "file.share.disable",
                outcome: AuditOutcome::Succeeded,
                now_unix_ms,
                summary: "File capability disabled",
                details: BTreeMap::from([
                    ("access_epoch".into(), access_epoch.to_string()),
                    ("share_revision".into(), enabled.share.revision.to_string()),
                ]),
            },
        )?;
        state.take();
        epoch_tx.send_replace(access_epoch);
        drop(state);
        Ok(self.snapshot())
    }

    /// Return user-safe capability health. The canonical root path and raw
    /// filesystem errors never cross this boundary.
    pub fn snapshot(&self) -> FileCapabilitySnapshot {
        let (enabled, access_epoch) = {
            let state = match self.inner.state.read() {
                Ok(state) => state,
                Err(_) => return FileCapabilitySnapshot::unavailable(),
            };
            match state.as_ref() {
                Some(enabled) => (
                    Some((enabled.root.clone(), enabled.share.clone())),
                    Some(enabled.access_epoch),
                ),
                None => (None, None),
            }
        };
        let Some((root, share)) = enabled else {
            let network_id = self
                .inner
                .last_network_id
                .read()
                .ok()
                .and_then(|network_id| network_id.clone());
            return FileCapabilitySnapshot {
                state: FileCapabilityState::Disabled,
                network_id,
                access_epoch: None,
                share_revision: None,
                available_bytes: None,
                root_error: None,
            };
        };
        let share_revision = Some(share.revision);
        let network_id = Some(share.network_id.clone());
        if let Err(error) = root.probe() {
            return FileCapabilitySnapshot {
                state: FileCapabilityState::RootUnavailable,
                network_id,
                access_epoch,
                share_revision,
                available_bytes: None,
                root_error: Some(error.code()),
            };
        }
        let available_bytes = match fs2::available_space(root.canonical_path()) {
            Ok(bytes) => bytes,
            Err(_error) => {
                return FileCapabilitySnapshot {
                    state: FileCapabilityState::RootUnavailable,
                    network_id,
                    access_epoch,
                    share_revision,
                    available_bytes: None,
                    root_error: Some(FileShareErrorCode::RootUnavailable),
                };
            }
        };
        let state = classify_disk_status(available_bytes, share.quota.reserve_bytes);
        FileCapabilitySnapshot {
            state,
            network_id,
            access_epoch,
            share_revision,
            available_bytes: Some(available_bytes),
            root_error: None,
        }
    }

    /// Metadata is returned only while the local owner keeps the capability
    /// enabled. This is the single metadata gate for command and UI adapters.
    pub fn current_share(&self) -> Option<FileShare> {
        self.inner
            .state
            .read()
            .ok()
            .and_then(|state| state.as_ref().map(|enabled| enabled.share.clone()))
    }

    /// Authorize one metadata or transfer operation against the current share
    /// revision before invoking the filesystem adapter. The returned permit
    /// must be checked at each long-running chunk or resume boundary.
    pub fn with_authorized<T>(
        &self,
        request: &FileAccessRequest,
        context: &AccessContext,
        network_enabled: bool,
        operation: impl FnOnce(&LocalShareRoot, &FileShare, FileCapabilityPermit) -> FileShareResult<T>,
    ) -> Result<T, FileCapabilityError> {
        let (root, share, permit) = self
            .authorize(request, context, network_enabled)?
            .into_parts();
        operation(&root, &share, permit).map_err(FileCapabilityError::Filesystem)
    }

    /// Resolve one authorized filesystem lease without holding the capability
    /// state lock for the caller's I/O. The permit must still be rechecked at
    /// each long-running chunk or resume boundary.
    pub fn authorize(
        &self,
        request: &FileAccessRequest,
        context: &AccessContext,
        network_enabled: bool,
    ) -> Result<FileCapabilityLease, FileCapabilityError> {
        let (root, share, permit) = {
            let state = self
                .inner
                .state
                .read()
                .map_err(|_| FileCapabilityError::Unavailable)?;
            let enabled = state.as_ref().ok_or(FileCapabilityError::Disabled)?;
            enabled
                .share
                .authorize(request, context, network_enabled)
                .map_err(FileCapabilityError::AccessDenied)?;
            let permit = FileCapabilityPermit {
                access_epoch: enabled.access_epoch,
                epoch_rx: self.inner.epoch_tx.subscribe(),
            };
            (enabled.root.clone(), enabled.share.clone(), permit)
        };
        Ok(FileCapabilityLease {
            root,
            share,
            permit,
        })
    }

    /// Fail-closed upload admission. The callback-supplied member usage and
    /// active-transfer count must come from authenticated server-side state.
    pub fn check_upload_quota(
        &self,
        request: UploadQuotaRequest,
        context: &AccessContext,
        network_enabled: bool,
    ) -> Result<FileQuotaUsage, FileCapabilityError> {
        if request.path.is_root() {
            return Err(FileCapabilityError::Filesystem(FileShareError::new(
                FileShareErrorCode::PathRejected,
                "uploads require a non-root shared path",
            )));
        }
        let lease = self.authorize(
            &FileAccessRequest {
                path: request.path,
                action: FileAction::Upload,
            },
            context,
            network_enabled,
        )?;
        let usage = lease
            .root()
            .directory_usage(request.scan_limits)
            .map_err(FileCapabilityError::Filesystem)?;
        lease.permit().ensure_current()?;
        quota::check_upload(
            &lease.share().quota,
            &usage,
            request.upload_bytes,
            request.replaced_bytes,
            request.member_bytes,
            request.concurrent_transfers,
        )
        .map_err(FileCapabilityError::Quota)
    }

    /// Start a filesystem operation under a transfer-scoped permit. The
    /// capability lock is not held while the closure performs I/O.
    pub fn with_enabled<T>(
        &self,
        operation: impl FnOnce(&LocalShareRoot, &FileShare, FileCapabilityPermit) -> FileShareResult<T>,
    ) -> Result<T, FileCapabilityError> {
        let (root, share, permit) = {
            let state = self
                .inner
                .state
                .read()
                .map_err(|_| FileCapabilityError::Unavailable)?;
            let enabled = state.as_ref().ok_or(FileCapabilityError::Disabled)?;
            (
                enabled.root.clone(),
                enabled.share.clone(),
                FileCapabilityPermit {
                    access_epoch: enabled.access_epoch,
                    epoch_rx: self.inner.epoch_tx.subscribe(),
                },
            )
        };
        operation(&root, &share, permit).map_err(FileCapabilityError::Filesystem)
    }

    pub fn audit_entries(&self) -> Result<Vec<AuditEntry>, FileCapabilityError> {
        self.inner
            .audit
            .lock()
            .map(|audit| audit.entries().to_vec())
            .map_err(|_| FileCapabilityError::Unavailable)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileCapabilityState {
    Disabled,
    Ready,
    RootUnavailable,
    LowSpace,
    DiskFull,
}

impl FileCapabilityState {
    pub const fn is_enabled(self) -> bool {
        !matches!(self, Self::Disabled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileCapabilitySnapshot {
    pub state: FileCapabilityState,
    pub network_id: Option<NetworkId>,
    pub access_epoch: Option<u64>,
    pub share_revision: Option<u64>,
    pub available_bytes: Option<u64>,
    pub root_error: Option<FileShareErrorCode>,
}

impl FileCapabilitySnapshot {
    fn unavailable() -> Self {
        Self {
            state: FileCapabilityState::RootUnavailable,
            network_id: None,
            access_epoch: None,
            share_revision: None,
            available_bytes: None,
            root_error: Some(FileShareErrorCode::RootUnavailable),
        }
    }
}

/// A transfer-scoped lease. Checking before each chunk/range bounds disable
/// latency without allowing a stale transfer to resume after revocation.
#[derive(Debug, Clone)]
pub struct FileCapabilityPermit {
    access_epoch: u64,
    epoch_rx: watch::Receiver<u64>,
}

/// An authorized, transfer-scoped view of the current File share. The
/// capability state lock is released before this value is returned.
#[derive(Debug, Clone)]
pub struct FileCapabilityLease {
    root: LocalShareRoot,
    share: FileShare,
    permit: FileCapabilityPermit,
}

impl FileCapabilityLease {
    pub fn root(&self) -> &LocalShareRoot {
        &self.root
    }

    pub fn share(&self) -> &FileShare {
        &self.share
    }

    pub fn permit(&self) -> &FileCapabilityPermit {
        &self.permit
    }

    pub fn into_parts(self) -> (LocalShareRoot, FileShare, FileCapabilityPermit) {
        (self.root, self.share, self.permit)
    }
}

impl FileCapabilityPermit {
    pub const fn access_epoch(&self) -> u64 {
        self.access_epoch
    }

    pub fn is_current(&self) -> bool {
        *self.epoch_rx.borrow() == self.access_epoch
    }

    pub fn ensure_current(&self) -> Result<(), FileCapabilityError> {
        if self.is_current() {
            Ok(())
        } else {
            Err(FileCapabilityError::Revoked)
        }
    }

    /// Resolve when enable/disable/reconfigure advances the capability epoch.
    pub async fn revoked(&mut self) -> u64 {
        loop {
            let current = *self.epoch_rx.borrow();
            if current != self.access_epoch {
                return current;
            }
            if self.epoch_rx.changed().await.is_err() {
                return u64::MAX;
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FileCapabilityError {
    #[error("File capability is disabled")]
    Disabled,
    #[error("File capability state is unavailable")]
    Unavailable,
    #[error("only the share owner may change local File settings")]
    OwnerRequired,
    #[error("File share settings are invalid: {0}")]
    InvalidShare(#[source] DomainError),
    #[error(transparent)]
    Filesystem(FileShareError),
    #[error("File access was denied")]
    AccessDenied(FileAccessDenial),
    #[error("File quota was exceeded")]
    Quota(FileQuotaError),
    #[error("File capability access was revoked")]
    Revoked,
    #[error("File configuration audit could not be recorded")]
    AuditUnavailable,
}

fn next_epoch(current: u64) -> u64 {
    current.saturating_add(1).max(1)
}

fn classify_disk_status(available_bytes: u64, reserve_bytes: u64) -> FileCapabilityState {
    if available_bytes == 0 {
        FileCapabilityState::DiskFull
    } else if available_bytes <= reserve_bytes {
        FileCapabilityState::LowSpace
    } else {
        FileCapabilityState::Ready
    }
}

struct AuditEvent<'a> {
    actor: &'a NodeId,
    share: &'a FileShare,
    action: &'a str,
    outcome: AuditOutcome,
    now_unix_ms: u64,
    summary: &'a str,
    details: BTreeMap<String, String>,
}

fn record_audit(
    audit: &Mutex<AuditLog>,
    sequence: &AtomicU64,
    event: AuditEvent<'_>,
) -> Result<AuditId, FileCapabilityError> {
    let sequence = sequence.fetch_add(1, Ordering::Relaxed);
    let audit_id = AuditId::from_public_key(
        format!("file-capability:{}:{sequence}", event.share.id.as_str()).as_bytes(),
    );
    let mut details = event.details;
    details.insert("object_kind".into(), "file_share".into());
    details.insert("object_id".into(), event.share.id.as_str().into());
    details.insert("capability".into(), "file".into());
    let entry = AuditEntry {
        audit_id: audit_id.clone(),
        operation_id: None,
        actor: event.actor.clone(),
        network_id: Some(event.share.network_id.clone()),
        action: event.action.into(),
        resource: format!("file_share:{}", event.share.id),
        outcome: event.outcome,
        occurred_at_unix_ms: event.now_unix_ms.max(1),
        summary: event.summary.into(),
        details,
        sensitive_keys: Default::default(),
    };
    audit
        .lock()
        .map_err(|_| FileCapabilityError::AuditUnavailable)?
        .append(entry)
        .map_err(|_| FileCapabilityError::AuditUnavailable)?;
    Ok(audit_id)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use fuckipv6_common::domain::{
        ConcurrentNamePolicy, FileAcl, FileDeletePolicy, FileQuota, FileShareId, FileShareMode,
        NetworkId, SymlinkPolicy, FILE_SHARE_SCHEMA_VERSION,
    };

    use super::*;

    fn owner() -> NodeId {
        NodeId::from_public_key(b"owner")
    }

    fn share() -> FileShare {
        FileShare {
            schema_version: FILE_SHARE_SCHEMA_VERSION,
            id: FileShareId::from_public_key(b"share"),
            network_id: NetworkId::from_public_key(b"network"),
            owner: owner(),
            display_name: "Team files".into(),
            mode: FileShareMode::ReadWrite,
            revision: 1,
            network_acl: FileAcl::default(),
            local_acl: FileAcl::default(),
            quota: FileQuota::default(),
            symlink_policy: SymlinkPolicy::Reject,
            concurrent_name_policy: ConcurrentNamePolicy::IfUnchanged,
            delete_policy: FileDeletePolicy::default(),
            audit_enabled: true,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        }
    }

    fn enable(
        capability: &FileShareCapability,
        temporary: &tempfile::TempDir,
        share: &FileShare,
        now_unix_ms: u64,
    ) -> FileCapabilitySnapshot {
        capability
            .enable(
                share,
                &owner(),
                temporary.path().to_str().expect("utf8 temp path"),
                now_unix_ms,
            )
            .expect("enable File")
    }

    #[test]
    fn owner_can_enable_and_disable_without_exposing_root_metadata() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let capability = FileShareCapability::new();
        let snapshot = enable(&capability, &temporary, &share(), 10);
        assert_eq!(snapshot.state, FileCapabilityState::Ready);
        assert_eq!(snapshot.access_epoch, Some(1));
        assert_eq!(snapshot.share_revision, Some(1));
        assert!(capability.current_share().is_some());

        let disabled = capability.disable(&owner(), 20).expect("disable");
        assert_eq!(disabled.state, FileCapabilityState::Disabled);
        assert!(capability.current_share().is_none());

        let audit = capability.audit_entries().expect("audit");
        assert_eq!(audit.len(), 2);
        assert_eq!(audit[0].action, "file.share.enable");
        assert_eq!(audit[1].action, "file.share.disable");
        assert!(!format!("{audit:?}").contains(temporary.path().to_str().expect("utf8")));
    }

    #[test]
    fn non_owner_changes_are_denied_and_audited() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let capability = FileShareCapability::new();
        let intruder = NodeId::from_public_key(b"intruder");
        let error = capability
            .enable(
                &share(),
                &intruder,
                temporary.path().to_str().expect("utf8"),
                10,
            )
            .expect_err("non-owner must be denied");
        assert!(matches!(error, FileCapabilityError::OwnerRequired));
        let audit = capability.audit_entries().expect("audit");
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].outcome, AuditOutcome::Denied);
        assert_eq!(
            audit[0].details.get("reason_code").map(String::as_str),
            Some("file_owner_required")
        );
    }

    #[test]
    fn disable_revokes_existing_transfer_permits() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let capability = FileShareCapability::new();
        enable(&capability, &temporary, &share(), 10);
        let permit = capability
            .with_enabled(|_, _, permit| Ok(permit))
            .expect("permit");
        assert!(permit.is_current());

        capability.disable(&owner(), 20).expect("disable");
        assert!(!permit.is_current());
        assert!(matches!(
            permit.ensure_current(),
            Err(FileCapabilityError::Revoked)
        ));
    }

    #[test]
    fn root_removal_is_reported_without_raw_paths() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let root_path = temporary.path().to_path_buf();
        let capability = FileShareCapability::new();
        enable(&capability, &temporary, &share(), 10);
        std::fs::remove_dir_all(&root_path).expect("remove root");

        let snapshot = capability.snapshot();
        assert_eq!(snapshot.state, FileCapabilityState::RootUnavailable);
        assert_eq!(
            snapshot.root_error,
            Some(FileShareErrorCode::RootUnavailable)
        );
        assert!(capability.current_share().is_some());
    }

    #[test]
    fn disk_status_keeps_the_configured_reserve_free() {
        assert_eq!(classify_disk_status(0, 10), FileCapabilityState::DiskFull);
        assert_eq!(classify_disk_status(10, 10), FileCapabilityState::LowSpace);
        assert_eq!(classify_disk_status(11, 10), FileCapabilityState::Ready);
        assert!(!FileCapabilityState::Disabled.is_enabled());
        assert!(FileCapabilityState::LowSpace.is_enabled());
    }

    #[test]
    fn filesystem_work_does_not_hold_the_capability_state_lock() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let capability = FileShareCapability::new();
        enable(&capability, &temporary, &share(), 10);

        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker_capability = capability.clone();
        let worker = std::thread::spawn(move || {
            worker_capability
                .with_enabled(|_, _, _| {
                    started_tx.send(()).expect("signal start");
                    release_rx.recv().expect("wait for disable");
                    Ok(())
                })
                .expect("filesystem operation")
        });

        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("operation started");
        capability.disable(&owner(), 20).expect("disable");
        release_tx.send(()).expect("release operation");
        worker.join().expect("worker");
    }
}
