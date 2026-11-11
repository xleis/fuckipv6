//! Owner-only orchestration for the local File capability.
//!
//! This is the only layer allowed to combine the selected filesystem root,
//! durable owner settings, runtime capability state, and the durable security
//! audit. Network-visible projections contain stable status codes only.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{SystemTime, UNIX_EPOCH};

use fuckipv6_common::domain::{
    AuditEntry, AuditId, AuditOutcome, CapabilityState, ConcurrentNamePolicy, FileAcl,
    FileAclEffect, FileAclRule, FileAction, FileDeletePolicy, FileQuota, FileShare, FileShareId,
    FileShareMode, NetworkId, NodeId, SharedPath, SubjectSelector, SymlinkPolicy,
    FILE_SHARE_SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::node::{NodeFileCapabilitySnapshot, NodeStateStore};
use crate::owner_security::audit::OwnerSecurityAudit;

use super::config::{FileShareConfigStore, PersistedFileShareConfig};
use super::runtime::{FileRuntime, FileRuntimeSnapshot};
use super::{
    FileCapabilityError, FileCapabilityState, FileShareErrorCode, LocalShareRoot, QuotaScanLimits,
};

static AUDIT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Stable owner-service failures. The code is safe for local IPC errors; no
/// filesystem path or raw OS error is included.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FileOwnerServiceError {
    #[error("file_owner_configuration_invalid")]
    InvalidConfiguration,
    #[error("file_owner_required")]
    OwnerRequired,
    #[error("file_root_invalid")]
    RootInvalid,
    #[error("file_root_sensitive")]
    RootSensitive,
    #[error("file_root_not_directory")]
    RootNotDirectory,
    #[error("file_root_symlink")]
    RootSymlink,
    #[error("file_root_unavailable")]
    RootUnavailable,
    #[error("file_configuration_persistence_unavailable")]
    PersistenceUnavailable,
    #[error("file_configuration_audit_unavailable")]
    AuditUnavailable,
    #[error("file_runtime_unavailable")]
    RuntimeUnavailable,
    #[error("file_owner_service_unavailable")]
    Unavailable,
}

impl FileOwnerServiceError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidConfiguration => "file_owner_configuration_invalid",
            Self::OwnerRequired => "file_owner_required",
            Self::RootInvalid => "file_root_invalid",
            Self::RootSensitive => "file_root_sensitive",
            Self::RootNotDirectory => "file_root_not_directory",
            Self::RootSymlink => "file_root_symlink",
            Self::RootUnavailable => "file_root_unavailable",
            Self::PersistenceUnavailable => "file_configuration_persistence_unavailable",
            Self::AuditUnavailable => "file_configuration_audit_unavailable",
            Self::RuntimeUnavailable => "file_runtime_unavailable",
            Self::Unavailable => "file_owner_service_unavailable",
        }
    }
}

/// Configuration accepted from owner-only local IPC. The service owns the
/// stable share id, revision, owner, timestamps, and mandatory audit flag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileShareConfiguration {
    pub network_id: NetworkId,
    pub display_name: String,
    #[serde(default = "default_file_share_mode")]
    pub mode: FileShareMode,
    #[serde(default)]
    pub network_acl: FileAcl,
    #[serde(default)]
    pub local_acl: FileAcl,
    #[serde(default)]
    pub quota: FileQuota,
    #[serde(default = "default_symlink_policy")]
    pub symlink_policy: SymlinkPolicy,
    #[serde(default = "default_concurrent_name_policy")]
    pub concurrent_name_policy: ConcurrentNamePolicy,
    #[serde(default)]
    pub delete_policy: FileDeletePolicy,
}

const fn default_file_share_mode() -> FileShareMode {
    FileShareMode::ReadOnly
}

const fn default_symlink_policy() -> SymlinkPolicy {
    SymlinkPolicy::Reject
}

const fn default_concurrent_name_policy() -> ConcurrentNamePolicy {
    ConcurrentNamePolicy::Reject
}

impl FileShareConfiguration {
    pub fn readonly(network_id: NetworkId, display_name: impl Into<String>) -> Self {
        Self {
            network_id,
            display_name: display_name.into(),
            mode: FileShareMode::ReadOnly,
            network_acl: FileAcl::default(),
            local_acl: FileAcl::default(),
            quota: FileQuota::default(),
            symlink_policy: SymlinkPolicy::Reject,
            concurrent_name_policy: ConcurrentNamePolicy::Reject,
            delete_policy: FileDeletePolicy::default(),
        }
    }

    /// Grant all active members the minimum read-only actions on the shared
    /// root. Both ACL layers receive the same narrow rule because File access
    /// is their intersection and either layer may veto.
    pub fn allow_active_members_readonly(mut self) -> Self {
        let rule = FileAclRule {
            id: "active-members-readonly".into(),
            subjects: std::collections::BTreeSet::from([SubjectSelector::AnyMember]),
            path_prefix: SharedPath::root(),
            actions: std::collections::BTreeSet::from([
                FileAction::List,
                FileAction::ReadMetadata,
                FileAction::Download,
            ]),
            effect: FileAclEffect::Allow,
        };
        self.network_acl.rules.push(rule.clone());
        self.local_acl.rules.push(rule);
        self
    }

    fn validate(&self) -> Result<(), FileOwnerServiceError> {
        if self.display_name.trim().is_empty() {
            return Err(FileOwnerServiceError::InvalidConfiguration);
        }
        self.network_acl
            .validate("network_acl")
            .map_err(|_| FileOwnerServiceError::InvalidConfiguration)?;
        self.local_acl
            .validate("local_acl")
            .map_err(|_| FileOwnerServiceError::InvalidConfiguration)?;
        self.quota
            .validate()
            .map_err(|_| FileOwnerServiceError::InvalidConfiguration)?;
        Ok(())
    }
}

/// Owner-only preflight response. The selected absolute root is deliberately
/// omitted; the local caller already knows the path it selected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSharePreview {
    pub network_id: NetworkId,
    pub display_name: String,
    pub mode: FileShareMode,
    pub quota: FileQuota,
    pub network_acl_rule_count: u32,
    pub local_acl_rule_count: u32,
    pub symlink_policy: SymlinkPolicy,
    pub audit_enabled: bool,
    pub available_bytes: u64,
    pub directory_bytes: u64,
    pub directory_entries: u64,
    pub symlinks_skipped: u64,
    pub special_files_skipped: u64,
    /// No File ACL grants access by default; access is the intersection of the
    /// network and local ACL layers.
    pub deny_by_default: bool,
}

/// Durable audit sink used by the owner service. Production uses the shared
/// owner-security ledger; tests can inject a failing sink to prove fail-closed
/// behavior without manufacturing an unreadable user configuration directory.
pub(crate) trait FileOwnerAuditSink: Send + Sync {
    fn append_entry(&self, entry: AuditEntry) -> Result<(), ()>;
}

impl FileOwnerAuditSink for OwnerSecurityAudit {
    fn append_entry(&self, entry: AuditEntry) -> Result<(), ()> {
        OwnerSecurityAudit::append_entry(self, entry)
            .map(|_| ())
            .map_err(|_| ())
    }
}

/// Coordinates one node's File settings. Clones share the same runtime,
/// state, mutation lock, and audit sequence.
#[derive(Clone)]
pub(crate) struct FileOwnerService {
    runtime: FileRuntime,
    config: FileShareConfigStore,
    audit: Arc<dyn FileOwnerAuditSink>,
    state: NodeStateStore,
    owner: NodeId,
    mutation: Arc<Mutex<()>>,
    last_restore_error: Arc<StdMutex<Option<String>>>,
}

impl std::fmt::Debug for FileOwnerService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FileOwnerService")
            .field("owner", &self.owner)
            .field("snapshot", &self.runtime.snapshot())
            .finish_non_exhaustive()
    }
}

impl FileOwnerService {
    pub(crate) fn new(
        config_dir: &Path,
        owner: NodeId,
        state: NodeStateStore,
    ) -> anyhow::Result<Self> {
        let audit = Arc::new(OwnerSecurityAudit::new(config_dir, owner.clone())?);
        Ok(Self::with_audit(
            FileShareConfigStore::at(config_dir),
            owner,
            state,
            audit,
        ))
    }

    fn with_audit(
        config: FileShareConfigStore,
        owner: NodeId,
        state: NodeStateStore,
        audit: Arc<dyn FileOwnerAuditSink>,
    ) -> Self {
        Self {
            runtime: FileRuntime::new(Default::default()),
            config,
            audit,
            state,
            owner,
            mutation: Arc::new(Mutex::new(())),
            last_restore_error: Arc::new(StdMutex::new(None)),
        }
    }

    /// Validate and inspect a prospective share without persisting or enabling
    /// it. The bounded directory scan never follows symlinks.
    pub(crate) async fn preview(
        &self,
        root_reference: &str,
        configuration: &FileShareConfiguration,
    ) -> Result<FileSharePreview, FileOwnerServiceError> {
        configuration.validate()?;
        let _guard = self.mutation.lock().await;
        let root = self.select_root(root_reference, configuration.symlink_policy)?;
        let usage = root
            .directory_usage(QuotaScanLimits::default())
            .map_err(root_error)?;
        let available_bytes = fs2::available_space(root.canonical_path())
            .map_err(|_| FileOwnerServiceError::RootUnavailable)?;
        let deny_by_default =
            configuration.network_acl.rules.is_empty() || configuration.local_acl.rules.is_empty();
        let preview = FileSharePreview {
            network_id: configuration.network_id.clone(),
            display_name: configuration.display_name.clone(),
            mode: configuration.mode,
            quota: configuration.quota.clone(),
            network_acl_rule_count: u32::try_from(configuration.network_acl.rules.len())
                .unwrap_or(u32::MAX),
            local_acl_rule_count: u32::try_from(configuration.local_acl.rules.len())
                .unwrap_or(u32::MAX),
            symlink_policy: configuration.symlink_policy,
            audit_enabled: true,
            available_bytes,
            directory_bytes: usage.total_bytes,
            directory_entries: usage.entries,
            symlinks_skipped: usage.symlinks_skipped,
            special_files_skipped: usage.special_files_skipped,
            deny_by_default,
        };
        self.append_audit(
            "file.share.preview",
            AuditOutcome::Succeeded,
            None,
            BTreeMap::from([
                ("network_id".into(), configuration.network_id.to_string()),
                ("available_bytes".into(), available_bytes.to_string()),
                (
                    "acl_rule_count".into(),
                    preview.network_acl_rule_count.to_string(),
                ),
                (
                    "local_acl_rule_count".into(),
                    preview.local_acl_rule_count.to_string(),
                ),
            ]),
            "File share configuration previewed",
        )?;
        Ok(preview)
    }

    /// Enable or update the local share. Durable audit is prepared before any
    /// runtime permission change, and a final audit failure rolls back both
    /// runtime and persisted intent.
    pub(crate) async fn enable_or_configure(
        &self,
        root_reference: &str,
        configuration: FileShareConfiguration,
    ) -> Result<NodeFileCapabilitySnapshot, FileOwnerServiceError> {
        configuration.validate()?;
        let _guard = self.mutation.lock().await;
        let previous_config = self
            .config
            .load()
            .map_err(|_| FileOwnerServiceError::PersistenceUnavailable)?;
        let now = unix_time_ms();
        let share = self.build_share(&configuration, previous_config.share.as_ref(), now)?;
        Self::validate_owned_share(&share, &self.owner)?;
        let prepared = self.prepare_audit("file.share.enable", &share)?;
        let previous_runtime = self.runtime.snapshot();

        let enabled = match self
            .runtime
            .enable(&share, &self.owner, root_reference, now)
        {
            Ok(snapshot) => snapshot,
            Err(error) => {
                let _ = prepared;
                let _ = self.append_audit(
                    "file.share.enable",
                    AuditOutcome::Failed,
                    Some((&share, &self.owner)),
                    BTreeMap::from([("reason_code".into(), capability_error_code(&error).into())]),
                    "File share enable failed",
                );
                return Err(service_error_from_capability(error));
            }
        };

        let next_config =
            PersistedFileShareConfig::enabled(root_reference.to_owned(), share.clone(), now);
        if self.config.save(&next_config).is_err() {
            self.restore_runtime(&previous_config, &previous_runtime);
            let _ = self.append_audit(
                "file.share.enable",
                AuditOutcome::Failed,
                Some((&share, &self.owner)),
                BTreeMap::from([(
                    "reason_code".into(),
                    FileOwnerServiceError::PersistenceUnavailable.code().into(),
                )]),
                "File share configuration could not be persisted",
            );
            self.publish_runtime_projection().await;
            return Err(FileOwnerServiceError::PersistenceUnavailable);
        }

        if self
            .append_audit(
                "file.share.enable",
                AuditOutcome::Succeeded,
                Some((&share, &self.owner)),
                BTreeMap::from([
                    (
                        "access_epoch".into(),
                        enabled
                            .capability
                            .access_epoch
                            .unwrap_or_default()
                            .to_string(),
                    ),
                    ("share_revision".into(), share.revision.to_string()),
                ]),
                "File capability enabled",
            )
            .is_err()
        {
            let rollback_ok = self.config.save(&previous_config).is_ok();
            self.restore_runtime(&previous_config, &previous_runtime);
            if !rollback_ok {
                let _ = self
                    .config
                    .save(&PersistedFileShareConfig::disabled(unix_time_ms()));
            }
            let _ = self.append_audit(
                "file.share.enable",
                AuditOutcome::Failed,
                Some((&share, &self.owner)),
                BTreeMap::from([(
                    "reason_code".into(),
                    FileOwnerServiceError::AuditUnavailable.code().into(),
                )]),
                "File share configuration audit failed and the change was rolled back",
            );
            self.publish_runtime_projection().await;
            return Err(FileOwnerServiceError::AuditUnavailable);
        }

        *self
            .last_restore_error
            .lock()
            .map_err(|_| FileOwnerServiceError::Unavailable)? = None;
        let projection = projection_from_runtime(&self.runtime.snapshot());
        self.publish_projection(projection.clone()).await;
        Ok(projection)
    }

    /// Disable the capability, revoke existing permits, and preserve all owner
    /// files. The persisted intent is changed only when durable audit can be
    /// prepared; final audit failure rolls everything back.
    pub(crate) async fn disable(
        &self,
    ) -> Result<NodeFileCapabilitySnapshot, FileOwnerServiceError> {
        let _guard = self.mutation.lock().await;
        let previous_config = self
            .config
            .load()
            .map_err(|_| FileOwnerServiceError::PersistenceUnavailable)?;
        let previous_runtime = self.runtime.snapshot();
        let share = previous_config
            .share
            .clone()
            .or_else(|| self.runtime.capability().current_share());
        let Some(share) = share else {
            let projection = projection_from_runtime(&self.runtime.snapshot());
            self.publish_projection(projection.clone()).await;
            return Ok(projection);
        };
        let prepared = self.prepare_audit("file.share.disable", &share)?;
        let now = unix_time_ms();
        if let Err(error) = self.runtime.disable(&self.owner, now) {
            if !matches!(error, FileCapabilityError::Disabled) {
                let _ = prepared;
                let _ = self.append_audit(
                    "file.share.disable",
                    AuditOutcome::Failed,
                    Some((&share, &self.owner)),
                    BTreeMap::from([("reason_code".into(), capability_error_code(&error).into())]),
                    "File share disable failed",
                );
                return Err(service_error_from_capability(error));
            }
        }
        let disabled = PersistedFileShareConfig::disabled(now);
        if self.config.save(&disabled).is_err() {
            self.restore_runtime(&previous_config, &previous_runtime);
            let _ = self.append_audit(
                "file.share.disable",
                AuditOutcome::Failed,
                Some((&share, &self.owner)),
                BTreeMap::from([(
                    "reason_code".into(),
                    FileOwnerServiceError::PersistenceUnavailable.code().into(),
                )]),
                "File share disable could not be persisted",
            );
            self.publish_runtime_projection().await;
            return Err(FileOwnerServiceError::PersistenceUnavailable);
        }
        if self
            .append_audit(
                "file.share.disable",
                AuditOutcome::Succeeded,
                Some((&share, &self.owner)),
                BTreeMap::from([("share_revision".into(), share.revision.to_string())]),
                "File capability disabled",
            )
            .is_err()
        {
            let rollback_ok = self.config.save(&previous_config).is_ok();
            self.restore_runtime(&previous_config, &previous_runtime);
            if !rollback_ok {
                let _ = self
                    .config
                    .save(&PersistedFileShareConfig::disabled(unix_time_ms()));
                let _ = self.runtime.disable(&self.owner, unix_time_ms());
            }
            self.publish_runtime_projection().await;
            return Err(FileOwnerServiceError::AuditUnavailable);
        }
        *self
            .last_restore_error
            .lock()
            .map_err(|_| FileOwnerServiceError::Unavailable)? = None;
        let projection = projection_from_runtime(&self.runtime.snapshot());
        self.publish_projection(projection.clone()).await;
        Ok(projection)
    }

    /// Restore persisted owner intent after process startup. An unavailable
    /// root or audit ledger never crashes the node; the capability stays
    /// unavailable and the UI receives a stable reason code.
    pub(crate) async fn restore_persisted(&self) -> Result<(), FileOwnerServiceError> {
        let _guard = self.mutation.lock().await;
        self.restore_persisted_locked().await
    }

    async fn restore_persisted_locked(&self) -> Result<(), FileOwnerServiceError> {
        let config = match self.config.load() {
            Ok(config) => config,
            Err(_) => {
                self.publish_unavailable_without_share("file_config_unavailable")
                    .await;
                return Ok(());
            }
        };
        if !config.enabled {
            let current = self.runtime.snapshot();
            if current.capability.state != FileCapabilityState::Disabled {
                let _ = self.runtime.disable(&self.owner, unix_time_ms());
            }
            *self
                .last_restore_error
                .lock()
                .map_err(|_| FileOwnerServiceError::Unavailable)? = None;
            self.publish_projection(projection_from_runtime(&self.runtime.snapshot()))
                .await;
            return Ok(());
        }
        let Some(root_reference) = config.root_reference.as_deref() else {
            self.publish_unavailable(config.share.as_ref(), "file_root_unavailable")
                .await;
            return Ok(());
        };
        let Some(share) = config.share.as_ref() else {
            self.publish_unavailable(None, "file_configuration_invalid")
                .await;
            return Ok(());
        };
        if let Err(error) = Self::validate_owned_share(share, &self.owner) {
            self.publish_unavailable(Some(share), error.code()).await;
            return Ok(());
        }
        let now = unix_time_ms();
        let runtime = match self.runtime.enable(share, &self.owner, root_reference, now) {
            Ok(runtime) => runtime,
            Err(error) => {
                let code = capability_error_code(&error);
                self.note_restore_failure(code, share).await;
                let projection = unavailable_projection(Some(share), code, 0);
                self.publish_projection(projection).await;
                return Ok(());
            }
        };
        if self
            .append_audit(
                "file.share.restore",
                AuditOutcome::Succeeded,
                Some((share, &self.owner)),
                BTreeMap::from([
                    (
                        "access_epoch".into(),
                        runtime
                            .capability
                            .access_epoch
                            .map_or_else(|| "none".into(), |epoch| epoch.to_string()),
                    ),
                    ("share_revision".into(), share.revision.to_string()),
                ]),
                "Persisted File capability restored",
            )
            .is_err()
        {
            let _ = self.runtime.disable(&self.owner, unix_time_ms());
            let projection = unavailable_projection(
                Some(share),
                FileOwnerServiceError::AuditUnavailable.code(),
                0,
            );
            self.publish_projection(projection).await;
            return Ok(());
        }
        *self
            .last_restore_error
            .lock()
            .map_err(|_| FileOwnerServiceError::Unavailable)? = None;
        self.publish_projection(projection_from_runtime(&self.runtime.snapshot()))
            .await;
        Ok(())
    }

    /// Refresh user-safe health and retry an interrupted root restore. State
    /// updates are skipped when the projection is unchanged.
    pub(crate) async fn refresh(&self) -> Result<(), FileOwnerServiceError> {
        let _guard = self.mutation.lock().await;
        let config = self.config.load();
        match config {
            Ok(config) if config.enabled => {
                let runtime = self.runtime.snapshot();
                if runtime.capability.state == FileCapabilityState::Disabled {
                    self.restore_persisted_locked().await?;
                    return Ok(());
                }
                self.publish_projection(projection_from_runtime(&runtime))
                    .await;
            }
            Ok(_) => {
                let runtime = self.runtime.snapshot();
                if runtime.capability.state != FileCapabilityState::Disabled {
                    let _ = self.runtime.disable(&self.owner, unix_time_ms());
                }
                self.publish_projection(projection_from_runtime(&self.runtime.snapshot()))
                    .await;
            }
            Err(_) => {
                self.publish_unavailable_without_share("file_config_unavailable")
                    .await;
            }
        }
        Ok(())
    }

    /// Revoke runtime access during process shutdown while preserving the
    /// durable `enabled` intent and every owner file.
    pub(crate) async fn shutdown(&self) {
        let _guard = self.mutation.lock().await;
        if self.runtime.snapshot().capability.state != FileCapabilityState::Disabled {
            let _ = self.runtime.disable(&self.owner, unix_time_ms());
        }
        self.publish_projection(projection_from_runtime(&self.runtime.snapshot()))
            .await;
    }

    fn select_root(
        &self,
        root_reference: &str,
        symlink_policy: SymlinkPolicy,
    ) -> Result<LocalShareRoot, FileOwnerServiceError> {
        if root_reference.is_empty() {
            return Err(FileOwnerServiceError::RootInvalid);
        }
        LocalShareRoot::select(root_reference, symlink_policy).map_err(root_error)
    }

    fn build_share(
        &self,
        configuration: &FileShareConfiguration,
        previous: Option<&FileShare>,
        now: u64,
    ) -> Result<FileShare, FileOwnerServiceError> {
        let share_id = FileShareId::from_public_key(
            format!("file-share:{}:{}", self.owner, configuration.network_id).as_bytes(),
        );
        let revision = previous
            .filter(|share| share.id == share_id)
            .map_or(1, |share| share.revision.saturating_add(1));
        let created_at_unix_ms = previous
            .filter(|share| share.id == share_id)
            .map_or(now.max(1), |share| share.created_at_unix_ms);
        let share = FileShare {
            schema_version: FILE_SHARE_SCHEMA_VERSION,
            id: share_id,
            network_id: configuration.network_id.clone(),
            owner: self.owner.clone(),
            display_name: configuration.display_name.trim().to_owned(),
            mode: configuration.mode,
            revision,
            network_acl: configuration.network_acl.clone(),
            local_acl: configuration.local_acl.clone(),
            quota: configuration.quota.clone(),
            symlink_policy: configuration.symlink_policy,
            concurrent_name_policy: configuration.concurrent_name_policy,
            delete_policy: configuration.delete_policy,
            audit_enabled: true,
            created_at_unix_ms,
            updated_at_unix_ms: now.max(created_at_unix_ms),
        };
        share
            .validate()
            .map_err(|_| FileOwnerServiceError::InvalidConfiguration)?;
        Ok(share)
    }

    fn validate_owned_share(
        share: &FileShare,
        owner: &NodeId,
    ) -> Result<(), FileOwnerServiceError> {
        if share.owner != *owner {
            return Err(FileOwnerServiceError::OwnerRequired);
        }
        share
            .validate()
            .map_err(|_| FileOwnerServiceError::InvalidConfiguration)
    }

    fn prepare_audit(
        &self,
        action: &str,
        share: &FileShare,
    ) -> Result<AuditId, FileOwnerServiceError> {
        self.append_audit(
            &format!("{action}.prepare"),
            AuditOutcome::Succeeded,
            Some((share, &self.owner)),
            BTreeMap::from([
                ("share_revision".into(), share.revision.to_string()),
                ("network_id".into(), share.network_id.to_string()),
            ]),
            "File configuration change prepared",
        )
    }

    fn append_audit(
        &self,
        action: &str,
        outcome: AuditOutcome,
        share: Option<(&FileShare, &NodeId)>,
        mut details: BTreeMap<String, String>,
        summary: &str,
    ) -> Result<AuditId, FileOwnerServiceError> {
        let now = unix_time_ms().max(1);
        let sequence = AUDIT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let audit_id = AuditId::from_public_key(
            format!(
                "file-owner:{}:{action}:{now}:{sequence}",
                self.owner.as_str()
            )
            .as_bytes(),
        );
        let (network_id, resource) = share.map_or_else(
            || (None, "file_share:local".to_owned()),
            |(share, _)| {
                details.insert("object_kind".into(), "file_share".into());
                details.insert("object_id".into(), share.id.as_str().into());
                details.insert("capability".into(), "file".into());
                (
                    Some(share.network_id.clone()),
                    format!("file_share:{}", share.id),
                )
            },
        );
        let entry = AuditEntry {
            audit_id: audit_id.clone(),
            operation_id: None,
            actor: self.owner.clone(),
            network_id,
            action: action.to_owned(),
            resource,
            outcome,
            occurred_at_unix_ms: now,
            summary: summary.to_owned(),
            details,
            sensitive_keys: Default::default(),
        };
        self.audit
            .append_entry(entry)
            .map_err(|_| FileOwnerServiceError::AuditUnavailable)?;
        Ok(audit_id)
    }

    fn restore_runtime(
        &self,
        previous_config: &PersistedFileShareConfig,
        previous_runtime: &FileRuntimeSnapshot,
    ) {
        if previous_config.enabled {
            if let (Some(root), Some(share)) = (
                previous_config.root_reference.as_deref(),
                previous_config.share.as_ref(),
            ) {
                if self
                    .runtime
                    .enable(share, &self.owner, root, unix_time_ms())
                    .is_ok()
                {
                    return;
                }
            }
        }
        if previous_runtime.capability.state != FileCapabilityState::Disabled {
            let _ = self.runtime.disable(&self.owner, unix_time_ms());
        }
    }

    async fn note_restore_failure(&self, code: &'static str, share: &FileShare) {
        let changed = self
            .last_restore_error
            .lock()
            .map(|mut last| {
                if last.as_deref() == Some(code) {
                    false
                } else {
                    *last = Some(code.to_owned());
                    true
                }
            })
            .unwrap_or(false);
        if changed {
            let _ = self.append_audit(
                "file.share.restore",
                AuditOutcome::Failed,
                Some((share, &self.owner)),
                BTreeMap::from([("reason_code".into(), code.into())]),
                "Persisted File capability is unavailable",
            );
        }
    }

    async fn publish_runtime_projection(&self) {
        self.publish_projection(projection_from_runtime(&self.runtime.snapshot()))
            .await;
    }

    async fn publish_unavailable_without_share(&self, code: &str) {
        let current = self.state.snapshot().await.file_capability;
        let projection = NodeFileCapabilitySnapshot {
            state: CapabilityState::Unavailable,
            network_id: current.network_id,
            access_epoch: current.access_epoch,
            share_revision: current.share_revision,
            available_bytes: None,
            root_error: Some(code.to_owned()),
            active_transfers: current.active_transfers,
        };
        self.publish_projection(projection).await;
    }

    async fn publish_unavailable(&self, share: Option<&FileShare>, code: &str) {
        self.publish_projection(unavailable_projection(
            share,
            code,
            self.runtime.snapshot().active_transfers,
        ))
        .await;
    }

    async fn publish_projection(&self, projection: NodeFileCapabilitySnapshot) {
        if self.state.snapshot().await.file_capability != projection {
            self.state
                .update(|snapshot| {
                    snapshot.file_capability = projection;
                })
                .await;
        }
    }
}

fn projection_from_runtime(runtime: &FileRuntimeSnapshot) -> NodeFileCapabilitySnapshot {
    let capability = &runtime.capability;
    NodeFileCapabilitySnapshot {
        state: match capability.state {
            FileCapabilityState::Disabled => CapabilityState::Disabled,
            FileCapabilityState::Ready => CapabilityState::Enabled,
            FileCapabilityState::LowSpace => CapabilityState::Degraded,
            FileCapabilityState::RootUnavailable | FileCapabilityState::DiskFull => {
                CapabilityState::Unavailable
            }
        },
        network_id: capability.network_id.clone(),
        access_epoch: capability.access_epoch,
        share_revision: capability.share_revision,
        available_bytes: capability.available_bytes,
        root_error: capability.root_error.map(|code| code.as_str().to_owned()),
        active_transfers: runtime.active_transfers,
    }
}

fn unavailable_projection(
    share: Option<&FileShare>,
    code: &str,
    active_transfers: u32,
) -> NodeFileCapabilitySnapshot {
    NodeFileCapabilitySnapshot {
        state: CapabilityState::Unavailable,
        network_id: share.map(|share| share.network_id.clone()),
        access_epoch: None,
        share_revision: share.map(|share| share.revision),
        available_bytes: None,
        root_error: Some(code.to_owned()),
        active_transfers,
    }
}

fn root_error(error: super::FileShareError) -> FileOwnerServiceError {
    match error.code() {
        FileShareErrorCode::RootInvalid => FileOwnerServiceError::RootInvalid,
        FileShareErrorCode::RootSensitive => FileOwnerServiceError::RootSensitive,
        FileShareErrorCode::RootNotDirectory => FileOwnerServiceError::RootNotDirectory,
        FileShareErrorCode::RootSymlink => FileOwnerServiceError::RootSymlink,
        FileShareErrorCode::RootUnavailable => FileOwnerServiceError::RootUnavailable,
        _ => FileOwnerServiceError::RootUnavailable,
    }
}

fn service_error_from_capability(error: FileCapabilityError) -> FileOwnerServiceError {
    match error {
        FileCapabilityError::Disabled | FileCapabilityError::Unavailable => {
            FileOwnerServiceError::RuntimeUnavailable
        }
        FileCapabilityError::OwnerRequired => FileOwnerServiceError::OwnerRequired,
        FileCapabilityError::InvalidShare(_) => FileOwnerServiceError::InvalidConfiguration,
        FileCapabilityError::Filesystem(error) => root_error(error),
        FileCapabilityError::AuditUnavailable => FileOwnerServiceError::AuditUnavailable,
        FileCapabilityError::AccessDenied(_)
        | FileCapabilityError::Quota(_)
        | FileCapabilityError::Revoked => FileOwnerServiceError::RuntimeUnavailable,
    }
}

fn capability_error_code(error: &FileCapabilityError) -> &'static str {
    match error {
        FileCapabilityError::Disabled => "file_capability_disabled",
        FileCapabilityError::Unavailable => "file_capability_unavailable",
        FileCapabilityError::OwnerRequired => "file_owner_required",
        FileCapabilityError::InvalidShare(_) => "file_share_invalid",
        FileCapabilityError::Filesystem(error) => error.code_str(),
        FileCapabilityError::AccessDenied(_) => "file_access_denied",
        FileCapabilityError::Quota(_) => "file_quota_exceeded",
        FileCapabilityError::Revoked => "file_capability_revoked",
        FileCapabilityError::AuditUnavailable => "file_audit_unavailable",
    }
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Mutex;

    use fuckipv6_common::domain::{
        FileAclEffect, FileAclRule, FileAction, SharedPath, SubjectSelector,
    };
    use tempfile::TempDir;

    use super::*;

    #[derive(Default)]
    struct RecordingAudit {
        entries: Mutex<Vec<AuditEntry>>,
    }

    impl FileOwnerAuditSink for RecordingAudit {
        fn append_entry(&self, entry: AuditEntry) -> Result<(), ()> {
            self.entries.lock().unwrap().push(entry);
            Ok(())
        }
    }

    struct FailingAudit;

    impl FileOwnerAuditSink for FailingAudit {
        fn append_entry(&self, _entry: AuditEntry) -> Result<(), ()> {
            Err(())
        }
    }

    fn fixture(name: &str) -> (TempDir, TempDir, NodeId, NodeStateStore, String) {
        let config = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        std::fs::write(root.path().join(format!("{name}.txt")), b"owner file").unwrap();
        let owner = NodeId::from_public_key(format!("owner-{name}").as_bytes());
        let state = NodeStateStore::new();
        let root_reference = root.path().to_string_lossy().to_string();
        (config, root, owner, state, root_reference)
    }

    fn configuration(name: &str) -> FileShareConfiguration {
        let network_id = NetworkId::from_public_key(format!("network-{name}").as_bytes());
        let mut configuration = FileShareConfiguration::readonly(network_id, "Team files");
        let rule = FileAclRule {
            id: "members-read".into(),
            subjects: BTreeSet::from([SubjectSelector::AnyMember]),
            path_prefix: SharedPath::root(),
            actions: BTreeSet::from([
                FileAction::List,
                FileAction::ReadMetadata,
                FileAction::Download,
            ]),
            effect: FileAclEffect::Allow,
        };
        configuration.network_acl.rules.push(rule.clone());
        configuration.local_acl.rules.push(rule);
        configuration
    }

    fn service(
        directory: &TempDir,
        owner: NodeId,
        state: NodeStateStore,
        audit: Arc<dyn FileOwnerAuditSink>,
    ) -> FileOwnerService {
        FileOwnerService::with_audit(
            FileShareConfigStore::at(directory.path()),
            owner,
            state,
            audit,
        )
    }

    #[tokio::test]
    async fn preview_does_not_persist_or_enable() {
        let (config, _root, owner, state, root) = fixture("preview");
        let audit = Arc::new(RecordingAudit::default());
        let service = service(&config, owner, state.clone(), audit.clone());
        let preview = service
            .preview(&root, &configuration("preview"))
            .await
            .unwrap();

        assert_eq!(preview.directory_entries, 1);
        assert!(!preview.deny_by_default);
        assert!(!FileShareConfigStore::at(config.path()).path().exists());
        assert_eq!(
            state.snapshot().await.file_capability.state,
            CapabilityState::Disabled
        );
        assert!(audit
            .entries
            .lock()
            .unwrap()
            .iter()
            .any(|entry| entry.action == "file.share.preview"));
    }

    #[tokio::test]
    async fn enable_persists_and_projects_without_exposing_root() {
        let (config, _root, owner, state, root) = fixture("enable");
        let audit = Arc::new(RecordingAudit::default());
        let service = service(&config, owner, state.clone(), audit.clone());
        let projection = service
            .enable_or_configure(&root, configuration("enable"))
            .await
            .unwrap();

        assert_eq!(projection.state, CapabilityState::Enabled);
        assert!(projection.access_epoch.unwrap() > 0);
        let persisted = FileShareConfigStore::at(config.path()).load().unwrap();
        assert!(persisted.enabled);
        assert_eq!(persisted.root_reference.as_deref(), Some(root.as_str()));
        let encoded = serde_json::to_string(&state.snapshot().await).unwrap();
        assert!(!encoded.contains(&root));
        assert!(!encoded.contains(config.path().to_string_lossy().as_ref()));
        let audit = audit.entries.lock().unwrap();
        assert!(audit
            .iter()
            .any(|entry| entry.action == "file.share.enable"));
        assert!(!serde_json::to_string(&*audit).unwrap().contains(&root));
    }

    #[tokio::test]
    async fn disable_preserves_owner_files_and_persisted_disabled_intent() {
        let (config, root, owner, state, root_reference) = fixture("disable");
        let audit = Arc::new(RecordingAudit::default());
        let service = service(&config, owner, state.clone(), audit);
        service
            .enable_or_configure(&root_reference, configuration("disable"))
            .await
            .unwrap();
        service.disable().await.unwrap();

        assert_eq!(
            state.snapshot().await.file_capability.state,
            CapabilityState::Disabled
        );
        assert!(
            !FileShareConfigStore::at(config.path())
                .load()
                .unwrap()
                .enabled
        );
        assert!(root.path().join("disable.txt").exists());
    }

    #[tokio::test]
    async fn restore_marks_unavailable_when_root_disappears() {
        let (config, root, owner, state, root_reference) = fixture("restore-missing");
        let audit = Arc::new(RecordingAudit::default());
        let first = service(&config, owner.clone(), state.clone(), audit.clone());
        first
            .enable_or_configure(&root_reference, configuration("restore-missing"))
            .await
            .unwrap();
        drop(first);
        drop(root);

        let restarted = service(&config, owner, state.clone(), audit);
        restarted.restore_persisted().await.unwrap();
        let snapshot = state.snapshot().await.file_capability;
        assert_eq!(snapshot.state, CapabilityState::Unavailable);
        assert_eq!(
            snapshot.root_error.as_deref(),
            Some("file_root_unavailable")
        );
        assert_eq!(snapshot.active_transfers, 0);
    }

    #[tokio::test]
    async fn shutdown_revokes_runtime_but_preserves_enabled_intent() {
        let (config, root, owner, state, root_reference) = fixture("shutdown");
        let audit = Arc::new(RecordingAudit::default());
        let service = service(&config, owner, state.clone(), audit);
        service
            .enable_or_configure(&root_reference, configuration("shutdown"))
            .await
            .unwrap();
        service.shutdown().await;

        assert!(
            FileShareConfigStore::at(config.path())
                .load()
                .unwrap()
                .enabled
        );
        assert_eq!(
            state.snapshot().await.file_capability.state,
            CapabilityState::Disabled
        );
        assert!(root.path().join("shutdown.txt").exists());
    }

    #[tokio::test]
    async fn audit_failure_prevents_enable_and_persistence() {
        let (config, _root, owner, state, root) = fixture("audit-failure");
        let service = service(&config, owner, state.clone(), Arc::new(FailingAudit));
        let error = service
            .enable_or_configure(&root, configuration("audit-failure"))
            .await
            .unwrap_err();

        assert_eq!(error, FileOwnerServiceError::AuditUnavailable);
        assert_eq!(
            state.snapshot().await.file_capability.state,
            CapabilityState::Disabled
        );
        assert!(
            !FileShareConfigStore::at(config.path())
                .load()
                .unwrap()
                .enabled
        );
    }
}
