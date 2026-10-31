//! File-share authorization, path, quota, and resumable-transfer contracts.
//!
//! This module performs no filesystem I/O. Platform adapters must still reject
//! symlink races atomically, write uploads to a temporary object, and rename it
//! into place only after the domain contract reports completion.
//!
//! Version one intentionally excludes synchronization, live mirroring, and
//! mounting a remote share as a local filesystem.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ids::{ContentHash, FileId, FileShareId, NetworkId, NodeId, OperationId, TransferId};
use super::permissions::{AccessContext, SubjectSelector};
use super::{DomainError, DomainResult};

pub const FILE_SHARE_SCHEMA_VERSION: u16 = 1;
pub const MAX_PORTABLE_PATH_BYTES: usize = 4_096;
pub const MAX_PORTABLE_COMPONENT_BYTES: usize = 255;
pub const MAX_TRANSFER_CHUNK_BYTES: u64 = 16 * 1024 * 1024;

/// A local-only reference to a directory selected by the node owner.
///
/// This type intentionally does not implement serialization. The real root
/// belongs in encrypted node-local configuration and must never enter network
/// events, audit records, generated schemas, or logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareRoot {
    local_reference: String,
}

impl ShareRoot {
    pub fn new(local_reference: impl Into<String>) -> DomainResult<Self> {
        let local_reference = local_reference.into();
        let bytes = local_reference.as_bytes();
        let absolute = local_reference.starts_with('/')
            || (bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'/' | b'\\'));
        if !absolute
            || local_reference.contains('\0')
            || local_reference.len() > MAX_PORTABLE_PATH_BYTES
        {
            return Err(DomainError::invalid_value(
                "share root must be an absolute local path without NUL bytes",
            ));
        }
        Ok(Self { local_reference })
    }

    pub fn local_reference(&self) -> &str {
        &self.local_reference
    }
}

/// A portable path relative to a share root.
///
/// The empty string denotes the root. Paths use `/` on every platform.
/// Backslashes, drive/UNC prefixes, control bytes, alternate data streams, empty
/// components, `.`/`..`, trailing dots/spaces, and reserved DOS device names are
/// rejected rather than normalized. This keeps every adapter on the same
/// authorization decision.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(try_from = "String")]
pub struct SharedPath(String);

impl SharedPath {
    pub fn root() -> Self {
        Self(String::new())
    }

    pub fn parse(value: impl Into<String>) -> DomainResult<Self> {
        let value = value.into();
        if value.len() > MAX_PORTABLE_PATH_BYTES {
            return Err(DomainError::invalid_value(
                "shared path must not exceed 4096 bytes",
            ));
        }
        if value.is_empty() {
            return Ok(Self::root());
        }
        if value.starts_with('/') || value.starts_with('\\') || value.contains('\\') {
            return Err(DomainError::invalid_value(
                "shared path must be relative and use '/' separators",
            ));
        }
        for component in value.split('/') {
            validate_component(component)?;
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/').filter(|component| !component.is_empty())
    }

    /// Component-aware containment check. It never compares raw prefixes, so
    /// `docs` does not contain `docs-private`.
    pub fn is_same_or_descendant_of(&self, ancestor: &Self) -> bool {
        if ancestor.is_root() || self == ancestor {
            return true;
        }
        let prefix = format!("{}/", ancestor.0);
        self.0.starts_with(&prefix)
    }

    pub fn parent(&self) -> Option<Self> {
        if self.is_root() {
            return None;
        }
        Some(Self(
            self.0
                .rsplit_once('/')
                .map_or_else(String::new, |(parent, _)| parent.to_owned()),
        ))
    }
}

impl TryFrom<String> for SharedPath {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<SharedPath> for String {
    fn from(value: SharedPath) -> Self {
        value.0
    }
}

fn validate_component(component: &str) -> DomainResult<()> {
    if component.is_empty() || component == "." || component == ".." {
        return Err(DomainError::invalid_value(
            "shared path must not contain empty, '.' or '..' components",
        ));
    }
    if component.len() > MAX_PORTABLE_COMPONENT_BYTES {
        return Err(DomainError::invalid_value(
            "shared path components must not exceed 255 bytes",
        ));
    }
    if component.ends_with('.') || component.ends_with(' ') {
        return Err(DomainError::invalid_value(
            "shared path components must not end with a dot or space",
        ));
    }
    if component
        .chars()
        .any(|character| character.is_control() || matches!(character, ':' | '/' | '\\'))
    {
        return Err(DomainError::invalid_value(
            "shared path contains a platform-ambiguous or control character",
        ));
    }
    let stem = component.split('.').next().unwrap_or(component);
    let stem = stem.trim_end_matches([' ', '.']).to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|suffix| {
                matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            });
    if reserved {
        return Err(DomainError::invalid_value(
            "shared path uses a reserved platform device name",
        ));
    }
    Ok(())
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum FileShareMode {
    ReadOnly,
    ReadWrite,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum FileAction {
    List,
    ReadMetadata,
    Download,
    Upload,
    CreateDirectory,
    Rename,
    Delete,
}

impl FileAction {
    pub const fn mutates(self) -> bool {
        matches!(
            self,
            Self::Upload | Self::CreateDirectory | Self::Rename | Self::Delete
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileAclEffect {
    Allow,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct FileAclRule {
    pub id: String,
    pub subjects: BTreeSet<SubjectSelector>,
    pub path_prefix: SharedPath,
    pub actions: BTreeSet<FileAction>,
    pub effect: FileAclEffect,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct FileAcl {
    pub rules: Vec<FileAclRule>,
}

impl FileAcl {
    pub fn validate(&self, field: &str) -> DomainResult<()> {
        let mut ids = BTreeSet::new();
        for rule in &self.rules {
            if rule.id.trim().is_empty() || rule.id.len() > 128 || !ids.insert(rule.id.as_str()) {
                return Err(DomainError::invalid_value(format!(
                    "{field} contains an empty, oversized, or duplicate rule id"
                )));
            }
            if rule.subjects.is_empty() || rule.actions.is_empty() {
                return Err(DomainError::invalid_value(format!(
                    "{field} rules require at least one subject and action"
                )));
            }
        }
        Ok(())
    }

    fn layer_decision(
        &self,
        context: &AccessContext,
        path: &SharedPath,
        action: FileAction,
    ) -> FileLayerDecision {
        let mut allowed = false;
        for rule in &self.rules {
            let path_matches = path.is_same_or_descendant_of(&rule.path_prefix);
            let subject_matches = rule
                .subjects
                .iter()
                .any(|subject| subject_matches(subject, context));
            if path_matches && subject_matches && rule.actions.contains(&action) {
                match rule.effect {
                    FileAclEffect::Deny => return FileLayerDecision::Denied,
                    FileAclEffect::Allow => allowed = true,
                }
            }
        }
        if allowed {
            FileLayerDecision::Allowed
        } else {
            FileLayerDecision::NoAllow
        }
    }
}

fn subject_matches(subject: &SubjectSelector, context: &AccessContext) -> bool {
    match subject {
        SubjectSelector::AnyMember => context.membership_active,
        SubjectSelector::Role(role) => context.role == *role,
        SubjectSelector::Node(node_id) => context.actor == *node_id,
        SubjectSelector::Group(group_id) => context.groups.contains(group_id),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileLayerDecision {
    Allowed,
    Denied,
    NoAllow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileAccessDenialCode {
    NetworkDisabled,
    MembershipInactive,
    CrossNetworkDenied,
    ReadOnlyShare,
    AclDenied,
    AclNoAllow,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct FileAccessDenial {
    pub code: FileAccessDenialCode,
    pub layer: String,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct FileAccessRequest {
    pub path: SharedPath,
    pub action: FileAction,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct FileQuota {
    pub total_bytes: u64,
    pub per_member_bytes: u64,
    pub max_file_bytes: u64,
    pub max_entries: u64,
    pub reserve_bytes: u64,
    pub max_concurrent_transfers: u32,
    /// Zero means unlimited.
    pub max_bandwidth_kbps: u32,
}

impl Default for FileQuota {
    fn default() -> Self {
        Self {
            total_bytes: 10 * 1024 * 1024 * 1024,
            per_member_bytes: 2 * 1024 * 1024 * 1024,
            max_file_bytes: 1024 * 1024 * 1024,
            max_entries: 100_000,
            reserve_bytes: 256 * 1024 * 1024,
            max_concurrent_transfers: 4,
            max_bandwidth_kbps: 0,
        }
    }
}

impl FileQuota {
    pub fn validate(&self) -> DomainResult<()> {
        if self.total_bytes == 0
            || self.per_member_bytes > self.total_bytes
            || self.max_file_bytes > self.per_member_bytes
            || self.reserve_bytes >= self.total_bytes
            || self.max_entries == 0
            || self.max_concurrent_transfers == 0
        {
            return Err(DomainError::invalid_value("file quota is invalid"));
        }
        Ok(())
    }

    /// Check a prospective upload. `replaced_bytes` is non-zero only for a
    /// version-checked replacement.
    pub fn check_upload(
        &self,
        upload_bytes: u64,
        replaced_bytes: u64,
        usage: &FileQuotaUsage,
        concurrent_transfers: u32,
    ) -> Result<(), FileQuotaError> {
        if upload_bytes > self.max_file_bytes {
            return Err(FileQuotaError::FileTooLarge);
        }
        if concurrent_transfers >= self.max_concurrent_transfers {
            return Err(FileQuotaError::ConcurrencyLimit);
        }
        if replaced_bytes == 0 && usage.entries >= self.max_entries {
            return Err(FileQuotaError::EntryLimit);
        }
        let growth = upload_bytes.saturating_sub(replaced_bytes);
        let member = usage
            .member_bytes
            .checked_add(growth)
            .ok_or(FileQuotaError::MemberQuotaExceeded)?;
        let total = usage
            .total_bytes
            .checked_add(growth)
            .ok_or(FileQuotaError::TotalQuotaExceeded)?;
        if member > self.per_member_bytes {
            return Err(FileQuotaError::MemberQuotaExceeded);
        }
        if total
            .checked_add(self.reserve_bytes)
            .is_none_or(|used| used > self.total_bytes)
        {
            return Err(FileQuotaError::TotalQuotaExceeded);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct FileQuotaUsage {
    pub total_bytes: u64,
    pub member_bytes: u64,
    pub entries: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileQuotaError {
    FileTooLarge,
    TotalQuotaExceeded,
    MemberQuotaExceeded,
    EntryLimit,
    ConcurrencyLimit,
}

impl FileQuotaError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::FileTooLarge => "file_too_large",
            Self::TotalQuotaExceeded => "file_total_quota_exceeded",
            Self::MemberQuotaExceeded => "file_member_quota_exceeded",
            Self::EntryLimit => "file_entry_limit",
            Self::ConcurrencyLimit => "file_concurrency_limit",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymlinkPolicy {
    /// Version-one default. Any symbolic link or reparse point is rejected.
    Reject,
    /// An adapter may follow a link only after proving both the pre-open and
    /// opened real paths stay below the selected root.
    AllowWithinRoot,
}

/// Proof that a platform adapter canonicalized a path immediately before and
/// after opening the object. A failed check must close the handle and deny.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct FileOpenValidation {
    pub policy: SymlinkPolicy,
    pub canonical_relative_path: SharedPath,
    pub before_open_inside_root: bool,
    pub after_open_inside_root: bool,
    pub symlink_followed: bool,
    pub validated_at_unix_ms: u64,
}

impl FileOpenValidation {
    pub fn permits_access(&self) -> bool {
        self.before_open_inside_root
            && self.after_open_inside_root
            && (!self.symlink_followed || self.policy == SymlinkPolicy::AllowWithinRoot)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConcurrentNamePolicy {
    /// Fail if the target name already exists.
    Reject,
    /// Apply only if the name and prior version remain unchanged.
    IfUnchanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct FileDeletePolicy {
    pub require_version_match: bool,
    pub allow_recursive_directory: bool,
}

impl Default for FileDeletePolicy {
    fn default() -> Self {
        Self {
            require_version_match: true,
            allow_recursive_directory: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct FileVersion {
    pub content_hash: ContentHash,
    pub size: u64,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct FileShare {
    pub schema_version: u16,
    pub id: FileShareId,
    pub network_id: NetworkId,
    pub owner: NodeId,
    pub display_name: String,
    pub mode: FileShareMode,
    pub revision: u64,
    pub network_acl: FileAcl,
    pub local_acl: FileAcl,
    pub quota: FileQuota,
    pub symlink_policy: SymlinkPolicy,
    pub concurrent_name_policy: ConcurrentNamePolicy,
    pub delete_policy: FileDeletePolicy,
    pub audit_enabled: bool,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

impl FileShare {
    pub fn validate(&self) -> DomainResult<()> {
        if self.schema_version != FILE_SHARE_SCHEMA_VERSION
            || self.display_name.trim().is_empty()
            || self.display_name.chars().count() > 128
            || self.revision == 0
            || !self.audit_enabled
            || self.updated_at_unix_ms < self.created_at_unix_ms
        {
            return Err(DomainError::invalid_value("file share is invalid"));
        }
        self.network_acl.validate("network_acl")?;
        self.local_acl.validate("local_acl")?;
        self.quota.validate()
    }

    /// Network and local ACLs are intersected, and either layer can veto. Network
    /// ownership or administrator role is not a file-content bypass.
    pub fn authorize(
        &self,
        request: &FileAccessRequest,
        context: &AccessContext,
        network_enabled: bool,
    ) -> Result<(), FileAccessDenial> {
        if !network_enabled {
            return Err(file_denial(
                FileAccessDenialCode::NetworkDisabled,
                "network",
            ));
        }
        if context.network_id != self.network_id {
            return Err(file_denial(
                FileAccessDenialCode::CrossNetworkDenied,
                "network",
            ));
        }
        if !context.membership_active {
            return Err(file_denial(
                FileAccessDenialCode::MembershipInactive,
                "membership",
            ));
        }
        if request.action.mutates() && self.mode == FileShareMode::ReadOnly {
            return Err(file_denial(FileAccessDenialCode::ReadOnlyShare, "share"));
        }
        for (layer, acl) in [
            ("network_acl", &self.network_acl),
            ("local_acl", &self.local_acl),
        ] {
            match acl.layer_decision(context, &request.path, request.action) {
                FileLayerDecision::Allowed => {}
                FileLayerDecision::Denied => {
                    return Err(file_denial(FileAccessDenialCode::AclDenied, layer));
                }
                FileLayerDecision::NoAllow => {
                    return Err(file_denial(FileAccessDenialCode::AclNoAllow, layer));
                }
            }
        }
        Ok(())
    }

    pub fn validate_delete(
        &self,
        request: &FileDeleteRequest,
        current_version: Option<&FileVersion>,
    ) -> Result<(), FileDeleteError> {
        if request.path.is_root() {
            return Err(FileDeleteError::RootCannotBeDeleted);
        }
        if self.delete_policy.require_version_match {
            let expected = request
                .expected_version
                .as_ref()
                .ok_or(FileDeleteError::VersionRequired)?;
            if current_version != Some(expected) {
                return Err(FileDeleteError::VersionMismatch);
            }
        } else if request.expected_version.is_some()
            && current_version != request.expected_version.as_ref()
        {
            return Err(FileDeleteError::VersionMismatch);
        }
        if request.is_directory
            && request.recursive
            && !self.delete_policy.allow_recursive_directory
        {
            return Err(FileDeleteError::RecursiveDeleteDisabled);
        }
        Ok(())
    }
}

fn file_denial(code: FileAccessDenialCode, layer: &str) -> FileAccessDenial {
    FileAccessDenial {
        code,
        layer: layer.to_owned(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct FileDeleteRequest {
    pub path: SharedPath,
    pub expected_version: Option<FileVersion>,
    pub is_directory: bool,
    pub recursive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileDeleteError {
    RootCannotBeDeleted,
    VersionRequired,
    VersionMismatch,
    RecursiveDeleteDisabled,
}

impl FileDeleteError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::RootCannotBeDeleted => "file_root_cannot_be_deleted",
            Self::VersionRequired => "file_delete_version_required",
            Self::VersionMismatch => "file_delete_version_mismatch",
            Self::RecursiveDeleteDisabled => "file_recursive_delete_disabled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferDirection {
    Upload,
    Download,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferState {
    Queued,
    Active,
    Paused,
    Verifying,
    Completed,
    Failed,
    Cancelled,
    Expired,
}

impl TransferState {
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Expired
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct FileChunk {
    pub index: u32,
    pub offset: u64,
    pub length: u64,
    pub checksum: ContentHash,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TransferManifest {
    pub file_id: FileId,
    pub total_bytes: u64,
    pub chunk_size: u64,
    pub content_hash: ContentHash,
    pub chunks: Vec<FileChunk>,
}

impl TransferManifest {
    pub fn validate(&self) -> DomainResult<()> {
        if self.chunk_size == 0 || self.chunk_size > MAX_TRANSFER_CHUNK_BYTES {
            return Err(DomainError::invalid_value(
                "file transfer chunk size is outside the supported range",
            ));
        }
        let expected_count = if self.total_bytes == 0 {
            0
        } else {
            usize::try_from(self.total_bytes.div_ceil(self.chunk_size))
                .map_err(|_| DomainError::invalid_value("file transfer is too large"))?
        };
        if self.chunks.len() != expected_count {
            return Err(DomainError::invalid_value(
                "file transfer chunks do not cover the declared size",
            ));
        }
        let mut covered = 0_u64;
        for (expected_index, chunk) in self.chunks.iter().enumerate() {
            let index = u32::try_from(expected_index)
                .map_err(|_| DomainError::invalid_value("too many file transfer chunks"))?;
            let expected_offset = u64::from(index) * self.chunk_size;
            let expected_length = self
                .total_bytes
                .saturating_sub(expected_offset)
                .min(self.chunk_size);
            if chunk.index != index
                || chunk.offset != expected_offset
                || chunk.length != expected_length
                || chunk.length == 0
            {
                return Err(DomainError::invalid_value(
                    "file transfer chunks must be sequential, contiguous, and non-empty",
                ));
            }
            covered = covered.saturating_add(chunk.length);
        }
        if covered != self.total_bytes {
            return Err(DomainError::invalid_value(
                "file transfer chunk lengths do not sum to the declared size",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TransferBinding {
    pub transfer_id: TransferId,
    pub operation_id: OperationId,
    pub share_id: FileShareId,
    pub actor: NodeId,
    pub path: SharedPath,
    pub direction: TransferDirection,
    /// Existing file version for download, or the version being replaced by an
    /// upload. New uploads use `None`.
    pub file_version: Option<FileVersion>,
    pub share_revision: u64,
    pub expires_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ResumeRequest {
    pub transfer_id: TransferId,
    pub operation_id: OperationId,
    pub share_id: FileShareId,
    pub actor: NodeId,
    pub path: SharedPath,
    pub direction: TransferDirection,
    pub content_hash: ContentHash,
    pub share_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileTransferError {
    InvalidManifest,
    Expired,
    PermissionRevoked,
    VersionChanged,
    BindingMismatch,
    InvalidState,
    ChunkOutOfRange,
    ChunkCorrupt,
    Incomplete,
    IntegrityMismatch,
}

impl FileTransferError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidManifest => "file_transfer_invalid_manifest",
            Self::Expired => "file_transfer_expired",
            Self::PermissionRevoked => "file_transfer_permission_revoked",
            Self::VersionChanged => "file_transfer_version_changed",
            Self::BindingMismatch => "file_transfer_binding_mismatch",
            Self::InvalidState => "file_transfer_invalid_state",
            Self::ChunkOutOfRange => "file_transfer_chunk_out_of_range",
            Self::ChunkCorrupt => "file_transfer_chunk_corrupt",
            Self::Incomplete => "file_transfer_incomplete",
            Self::IntegrityMismatch => "file_transfer_integrity_mismatch",
        }
    }
}

impl fmt::Display for FileTransferError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidManifest => "invalid transfer manifest",
            Self::Expired => "transfer resume window expired",
            Self::PermissionRevoked => "file permission was revoked",
            Self::VersionChanged => "file version changed",
            Self::BindingMismatch => "transfer resume binding does not match",
            Self::InvalidState => "transfer state does not allow this operation",
            Self::ChunkOutOfRange => "chunk is not part of the transfer manifest",
            Self::ChunkCorrupt => "chunk checksum or length does not match",
            Self::Incomplete => "not every declared chunk was received",
            Self::IntegrityMismatch => "assembled file hash or size does not match",
        })
    }
}

impl std::error::Error for FileTransferError {}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TransferSession {
    pub binding: TransferBinding,
    pub manifest: TransferManifest,
    pub state: TransferState,
    received_chunks: BTreeMap<u32, ContentHash>,
    pub revision: u64,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

impl TransferSession {
    pub fn new(
        binding: TransferBinding,
        manifest: TransferManifest,
        now_unix_ms: u64,
    ) -> Result<Self, FileTransferError> {
        manifest
            .validate()
            .map_err(|_| FileTransferError::InvalidManifest)?;
        if binding.share_revision == 0 || now_unix_ms >= binding.expires_at_unix_ms {
            return Err(FileTransferError::InvalidManifest);
        }
        if binding.direction == TransferDirection::Download
            && binding.file_version.as_ref().is_none_or(|version| {
                version.content_hash != manifest.content_hash
                    || version.size != manifest.total_bytes
            })
        {
            return Err(FileTransferError::InvalidManifest);
        }
        Ok(Self {
            binding,
            manifest,
            state: TransferState::Queued,
            received_chunks: BTreeMap::new(),
            revision: 1,
            created_at_unix_ms: now_unix_ms,
            updated_at_unix_ms: now_unix_ms,
        })
    }

    pub fn received_chunk_count(&self) -> usize {
        self.received_chunks.len()
    }

    pub fn resume(
        &mut self,
        request: &ResumeRequest,
        current_file_version: Option<&FileVersion>,
        current_share_revision: u64,
        authorized: bool,
        now_unix_ms: u64,
    ) -> Result<(), FileTransferError> {
        if now_unix_ms >= self.binding.expires_at_unix_ms {
            self.state = TransferState::Expired;
            return Err(FileTransferError::Expired);
        }
        if !authorized {
            self.state = TransferState::Cancelled;
            return Err(FileTransferError::PermissionRevoked);
        }
        if request.transfer_id != self.binding.transfer_id
            || request.operation_id != self.binding.operation_id
            || request.share_id != self.binding.share_id
            || request.actor != self.binding.actor
            || request.path != self.binding.path
            || request.direction != self.binding.direction
            || request.content_hash != self.manifest.content_hash
            || request.share_revision != self.binding.share_revision
            || current_share_revision != self.binding.share_revision
        {
            return Err(FileTransferError::BindingMismatch);
        }
        if current_file_version != self.binding.file_version.as_ref() {
            return Err(FileTransferError::VersionChanged);
        }
        if !matches!(self.state, TransferState::Queued | TransferState::Paused) {
            return Err(FileTransferError::InvalidState);
        }
        self.transition(TransferState::Active, now_unix_ms);
        Ok(())
    }

    pub fn pause(&mut self, now_unix_ms: u64) -> Result<(), FileTransferError> {
        if self.state != TransferState::Active {
            return Err(FileTransferError::InvalidState);
        }
        self.transition(TransferState::Paused, now_unix_ms);
        Ok(())
    }

    pub fn cancel(&mut self, now_unix_ms: u64) -> Result<(), FileTransferError> {
        if self.state.is_terminal() {
            return Err(FileTransferError::InvalidState);
        }
        self.transition(TransferState::Cancelled, now_unix_ms);
        Ok(())
    }

    /// Record a chunk after checking its manifest range and digest. Replaying
    /// the same chunk is idempotent; different bytes for a received index are
    /// rejected.
    pub fn record_chunk(
        &mut self,
        index: u32,
        bytes: &[u8],
        now_unix_ms: u64,
    ) -> Result<(), FileTransferError> {
        if self.state != TransferState::Active {
            return Err(FileTransferError::InvalidState);
        }
        let chunk = self
            .manifest
            .chunks
            .get(index as usize)
            .filter(|chunk| chunk.index == index)
            .ok_or(FileTransferError::ChunkOutOfRange)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != chunk.length {
            return Err(FileTransferError::ChunkCorrupt);
        }
        let digest = ContentHash::from_bytes(bytes);
        if digest != chunk.checksum {
            return Err(FileTransferError::ChunkCorrupt);
        }
        if let Some(existing) = self.received_chunks.get(&index) {
            return if existing == &digest {
                Ok(())
            } else {
                Err(FileTransferError::ChunkCorrupt)
            };
        }
        self.received_chunks.insert(index, digest);
        self.revision = self.revision.saturating_add(1);
        self.updated_at_unix_ms = now_unix_ms;
        Ok(())
    }

    /// Verify all chunks and the assembled object before the adapter atomically
    /// renames the temporary object into its final path.
    pub fn verify_and_complete(
        &mut self,
        assembled_bytes: &[u8],
        now_unix_ms: u64,
    ) -> Result<(), FileTransferError> {
        if self.state != TransferState::Active {
            return Err(FileTransferError::InvalidState);
        }
        if self.received_chunks.len() != self.manifest.chunks.len() {
            return Err(FileTransferError::Incomplete);
        }
        self.state = TransferState::Verifying;
        if u64::try_from(assembled_bytes.len()).unwrap_or(u64::MAX) != self.manifest.total_bytes
            || ContentHash::from_bytes(assembled_bytes) != self.manifest.content_hash
        {
            self.state = TransferState::Failed;
            self.revision = self.revision.saturating_add(1);
            self.updated_at_unix_ms = now_unix_ms;
            return Err(FileTransferError::IntegrityMismatch);
        }
        self.transition(TransferState::Completed, now_unix_ms);
        Ok(())
    }

    fn transition(&mut self, state: TransferState, now_unix_ms: u64) {
        self.state = state;
        self.revision = self.revision.saturating_add(1);
        self.updated_at_unix_ms = now_unix_ms;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::permissions::{AccessContext, NetworkRole};

    fn node(byte: u8) -> NodeId {
        NodeId::from_public_key(&[byte])
    }

    fn allow_rule(subject: SubjectSelector) -> FileAclRule {
        FileAclRule {
            id: "allow".into(),
            subjects: BTreeSet::from([subject]),
            path_prefix: SharedPath::root(),
            actions: BTreeSet::from([
                FileAction::List,
                FileAction::ReadMetadata,
                FileAction::Download,
                FileAction::Upload,
                FileAction::Delete,
            ]),
            effect: FileAclEffect::Allow,
        }
    }

    fn share() -> FileShare {
        FileShare {
            schema_version: FILE_SHARE_SCHEMA_VERSION,
            id: FileShareId::from_public_key(b"share"),
            network_id: NetworkId::from_public_key(b"network"),
            owner: node(1),
            display_name: "Team files".into(),
            mode: FileShareMode::ReadWrite,
            revision: 1,
            network_acl: FileAcl {
                rules: vec![allow_rule(SubjectSelector::AnyMember)],
            },
            local_acl: FileAcl {
                rules: vec![allow_rule(SubjectSelector::AnyMember)],
            },
            quota: FileQuota::default(),
            symlink_policy: SymlinkPolicy::Reject,
            concurrent_name_policy: ConcurrentNamePolicy::IfUnchanged,
            delete_policy: FileDeletePolicy::default(),
            audit_enabled: true,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        }
    }

    fn context(share: &FileShare, actor: NodeId) -> AccessContext {
        AccessContext {
            network_id: share.network_id.clone(),
            actor,
            role: NetworkRole::Member,
            groups: BTreeSet::new(),
            membership_active: true,
            recent_reauthentication: false,
            emergency_coordinator: false,
        }
    }

    fn access(path: &str, action: FileAction) -> FileAccessRequest {
        FileAccessRequest {
            path: SharedPath::parse(path).expect("path"),
            action,
        }
    }

    #[test]
    fn portable_paths_reject_traversal_and_platform_ambiguity() {
        assert_eq!(
            SharedPath::parse("docs/readme.md").expect("path").as_str(),
            "docs/readme.md"
        );
        assert_eq!(SharedPath::parse("").expect("root"), SharedPath::root());
        for unsafe_path in [
            "/etc/passwd",
            "../secret",
            "docs/../secret",
            "docs//readme",
            "docs\\readme",
            "C:/secret",
            "docs/name.",
            "docs/name ",
            "docs/CON.txt",
            "docs/COM1",
            "docs/\u{0}name",
            "docs/file:stream",
        ] {
            assert!(SharedPath::parse(unsafe_path).is_err(), "{unsafe_path:?}");
        }
        assert!(!SharedPath::parse("docs-private")
            .expect("path")
            .is_same_or_descendant_of(&SharedPath::parse("docs").expect("documents")));
        assert!(serde_json::from_str::<SharedPath>(r#""../escape""#).is_err());
    }

    #[test]
    fn deny_wins_and_both_network_and_local_layers_must_allow() {
        let member = node(2);
        let mut share = share();
        let member_context = context(&share, member.clone());
        assert!(share
            .authorize(
                &access("docs/readme.md", FileAction::Download),
                &member_context,
                true,
            )
            .is_ok());
        share.network_acl.rules.push(FileAclRule {
            id: "deny-private".into(),
            subjects: BTreeSet::from([SubjectSelector::Node(member)]),
            path_prefix: SharedPath::parse("docs/private").expect("path"),
            actions: BTreeSet::from([FileAction::Download]),
            effect: FileAclEffect::Deny,
        });
        assert_eq!(
            share.authorize(
                &access("docs/private/key.txt", FileAction::Download),
                &member_context,
                true,
            ),
            Err(file_denial(FileAccessDenialCode::AclDenied, "network_acl"))
        );
        share.local_acl.rules.clear();
        assert_eq!(
            share.authorize(
                &access("public.txt", FileAction::Download),
                &member_context,
                true,
            ),
            Err(file_denial(FileAccessDenialCode::AclNoAllow, "local_acl"))
        );
    }

    #[test]
    fn membership_network_and_read_only_are_enforced_before_acls() {
        let member = node(2);
        let mut share = share();
        let mut member_context = context(&share, member.clone());
        member_context.membership_active = false;
        assert_eq!(
            share
                .authorize(
                    &access("public.txt", FileAction::Download),
                    &member_context,
                    true,
                )
                .expect_err("inactive membership")
                .code,
            FileAccessDenialCode::MembershipInactive
        );
        member_context.membership_active = true;
        member_context.network_id = NetworkId::from_public_key(b"other-network");
        assert_eq!(
            share
                .authorize(
                    &access("public.txt", FileAction::Download),
                    &member_context,
                    true,
                )
                .expect_err("cross network")
                .code,
            FileAccessDenialCode::CrossNetworkDenied
        );
        let member_context = context(&share, member);
        share.mode = FileShareMode::ReadOnly;
        assert_eq!(
            share
                .authorize(
                    &access("public.txt", FileAction::Upload),
                    &member_context,
                    true,
                )
                .expect_err("read only")
                .code,
            FileAccessDenialCode::ReadOnlyShare
        );
    }

    #[test]
    fn quota_counts_growth_and_preserves_reserved_space() {
        let quota = FileQuota {
            total_bytes: 100,
            per_member_bytes: 80,
            max_file_bytes: 60,
            max_entries: 2,
            reserve_bytes: 20,
            max_concurrent_transfers: 2,
            max_bandwidth_kbps: 0,
        };
        let usage = FileQuotaUsage {
            total_bytes: 40,
            member_bytes: 30,
            entries: 1,
        };
        assert_eq!(
            quota.check_upload(61, 0, &usage, 0),
            Err(FileQuotaError::FileTooLarge)
        );
        assert_eq!(
            quota.check_upload(51, 0, &usage, 0),
            Err(FileQuotaError::MemberQuotaExceeded)
        );
        assert_eq!(
            quota.check_upload(10, 0, &usage, 2),
            Err(FileQuotaError::ConcurrencyLimit)
        );
        assert_eq!(quota.check_upload(50, 30, &usage, 0), Ok(()));
    }

    #[test]
    fn symlink_and_delete_checks_fail_closed() {
        let validation = FileOpenValidation {
            policy: SymlinkPolicy::AllowWithinRoot,
            canonical_relative_path: SharedPath::parse("dir/file").expect("path"),
            before_open_inside_root: true,
            after_open_inside_root: false,
            symlink_followed: true,
            validated_at_unix_ms: 1,
        };
        assert!(!validation.permits_access());

        let share = share();
        let version = FileVersion {
            content_hash: ContentHash::from_bytes(b"file"),
            size: 4,
            revision: 2,
        };
        let request = FileDeleteRequest {
            path: SharedPath::parse("file").expect("path"),
            expected_version: Some(version.clone()),
            is_directory: false,
            recursive: false,
        };
        assert_eq!(share.validate_delete(&request, Some(&version)), Ok(()));
        assert_eq!(
            share.validate_delete(&request, None),
            Err(FileDeleteError::VersionMismatch)
        );
        let root = FileDeleteRequest {
            path: SharedPath::root(),
            expected_version: None,
            is_directory: true,
            recursive: true,
        };
        assert_eq!(
            share.validate_delete(&root, None),
            Err(FileDeleteError::RootCannotBeDeleted)
        );
    }

    #[test]
    fn manifest_is_sequential_and_exactly_covers_content() {
        let content = b"abcdefghij";
        let manifest = manifest(content, 4);
        assert!(manifest.validate().is_ok());
        let mut invalid = manifest;
        invalid.chunks[1].offset += 1;
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn resume_is_bound_to_identity_share_revision_and_file_version() {
        let actor = node(2);
        let content = b"abcdefgh";
        let mut active = session(content, binding("upload.bin", actor));
        active
            .resume(&request(&active), None, 1, true, 2)
            .expect("start");
        active.pause(3).expect("pause");
        active
            .resume(&request(&active), None, 1, true, 4)
            .expect("resume");

        let mut stale = request(&active);
        stale.actor = node(3);
        assert_eq!(
            active.resume(&stale, None, 1, true, 5),
            Err(FileTransferError::BindingMismatch)
        );
        assert_eq!(
            active.resume(&request(&active), None, 2, true, 5),
            Err(FileTransferError::BindingMismatch)
        );
        let changed_version = FileVersion {
            content_hash: ContentHash::from_bytes(b"old"),
            size: 3,
            revision: 1,
        };
        assert_eq!(
            active.resume(&request(&active), Some(&changed_version), 1, true, 5),
            Err(FileTransferError::VersionChanged)
        );
        assert_eq!(
            active.resume(&request(&active), None, 1, false, 5),
            Err(FileTransferError::PermissionRevoked)
        );
    }

    #[test]
    fn chunks_are_idempotent_and_integrity_is_verified_before_completion() {
        let content = b"abcdefghij";
        let mut active = session(content, binding("upload.bin", node(2)));
        active
            .resume(&request(&active), None, 1, true, 2)
            .expect("start");
        active.record_chunk(0, b"abcd", 3).expect("chunk");
        active.record_chunk(0, b"abcd", 3).expect("replay");
        assert_eq!(
            active.record_chunk(1, b"WXYZ", 4),
            Err(FileTransferError::ChunkCorrupt)
        );
        active.record_chunk(1, b"efgh", 4).expect("chunk");
        assert_eq!(
            active.verify_and_complete(content, 5),
            Err(FileTransferError::Incomplete)
        );
        assert_eq!(active.state, TransferState::Active);
        active.record_chunk(2, b"ij", 6).expect("chunk");
        assert_eq!(
            active.verify_and_complete(b"xxxxxxxxxx", 7),
            Err(FileTransferError::IntegrityMismatch)
        );
        assert_eq!(active.state, TransferState::Failed);
    }

    fn manifest(bytes: &[u8], chunk_size: u64) -> TransferManifest {
        let chunks = bytes
            .chunks(usize::try_from(chunk_size).expect("chunk size"))
            .enumerate()
            .map(|(index, chunk)| FileChunk {
                index: u32::try_from(index).expect("index"),
                offset: u64::try_from(index).expect("index") * chunk_size,
                length: u64::try_from(chunk.len()).expect("length"),
                checksum: ContentHash::from_bytes(chunk),
            })
            .collect();
        TransferManifest {
            file_id: FileId::from_public_key(b"file"),
            total_bytes: u64::try_from(bytes.len()).expect("length"),
            chunk_size,
            content_hash: ContentHash::from_bytes(bytes),
            chunks,
        }
    }

    fn session(bytes: &[u8], binding: TransferBinding) -> TransferSession {
        TransferSession::new(binding, manifest(bytes, 4), 1).expect("session")
    }

    fn binding(path: &str, actor: NodeId) -> TransferBinding {
        TransferBinding {
            transfer_id: TransferId::from_public_key(b"transfer"),
            operation_id: OperationId::from_public_key(b"operation"),
            share_id: FileShareId::from_public_key(b"share"),
            actor,
            path: SharedPath::parse(path).expect("path"),
            direction: TransferDirection::Upload,
            file_version: None,
            share_revision: 1,
            expires_at_unix_ms: 100,
        }
    }

    fn request(session: &TransferSession) -> ResumeRequest {
        ResumeRequest {
            transfer_id: session.binding.transfer_id.clone(),
            operation_id: session.binding.operation_id.clone(),
            share_id: session.binding.share_id.clone(),
            actor: session.binding.actor.clone(),
            path: session.binding.path.clone(),
            direction: session.binding.direction,
            content_hash: session.manifest.content_hash.clone(),
            share_revision: session.binding.share_revision,
        }
    }
}
