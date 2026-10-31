//! File-share authorization, path, quota, and resumable-transfer contracts.
//!
//! This module deliberately does not perform filesystem I/O. It freezes the
//! security decisions that the platform adapter must enforce: portable relative
//! paths, deny-first ACLs in both network and local layers, explicit symlink and
//! TOCTOU revalidation, bounded resources, and version-bound transfer resumes.

use super::foundation::{
    require_non_empty, require_unique, ContractIssue, ContractResult, UnixMillis,
};
use super::ids::{FileShareId, NetworkId, NodeId, OperationId};
use super::permission::{AccessContext, PermissionPrincipal, SubjectSelector};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub const FILE_SHARE_CONTRACT_VERSION: u16 = 1;
pub const MAX_PORTABLE_PATH_BYTES: usize = 4096;
pub const MAX_PORTABLE_COMPONENT_BYTES: usize = 255;
pub const MAX_TRANSFER_CHUNK_BYTES: u64 = 16 * 1024 * 1024;

/// A local-only reference to a directory selected by the node owner.
///
/// This type intentionally does not serialize: the real root belongs in
/// encrypted node-local configuration and must never be included in network
/// events, WebUI metadata, or logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareRoot {
    local_reference: String,
}

impl ShareRoot {
    pub fn new(local_reference: impl Into<String>) -> Result<Self, ContractIssue> {
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
            return Err(ContractIssue::new(
                "invalid_share_root",
                "share_root",
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
/// `""` denotes the root itself. Paths use `/` on every platform. Backslashes,
/// drive/UNC prefixes, NUL/control bytes, Windows alternate-data-stream colons,
/// empty components, `.`/`..`, trailing dots/spaces, and reserved DOS device
/// names are rejected rather than normalized. Refusing ambiguous inputs keeps
/// every adapter on the same authorization decision.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SharedPath(String);

impl SharedPath {
    pub fn root() -> Self {
        Self(String::new())
    }

    pub fn parse(value: impl Into<String>) -> Result<Self, ContractIssue> {
        let value = value.into();
        if value.len() > MAX_PORTABLE_PATH_BYTES {
            return Err(path_error(
                "path_too_long",
                "path must not exceed 4096 bytes",
            ));
        }
        if value.is_empty() {
            return Ok(Self::root());
        }
        if value.starts_with('/') || value.starts_with('\\') || value.contains('\\') {
            return Err(path_error(
                "path_not_relative",
                "path must be relative and use '/' separators",
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

    /// Component-aware containment check. It never compares raw string prefixes,
    /// so `docs` does not contain `docs-private`.
    pub fn is_same_or_descendant_of(&self, ancestor: &Self) -> bool {
        if ancestor.is_root() {
            return true;
        }
        let mut path = self.components();
        ancestor
            .components()
            .all(|expected| path.next() == Some(expected))
            && (path.next().is_some() || self == ancestor)
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

fn path_error(code: &str, message: &str) -> ContractIssue {
    ContractIssue::new(code, "path", message)
}

fn validate_component(component: &str) -> Result<(), ContractIssue> {
    if component.is_empty() || component == "." || component == ".." {
        return Err(path_error(
            "unsafe_path_component",
            "path must not contain empty, '.' or '..' components",
        ));
    }
    if component.len() > MAX_PORTABLE_COMPONENT_BYTES {
        return Err(path_error(
            "path_component_too_long",
            "path components must not exceed 255 bytes",
        ));
    }
    if component.ends_with('.') || component.ends_with(' ') {
        return Err(path_error(
            "unsafe_path_component",
            "path components must not end with a dot or space",
        ));
    }
    if component
        .chars()
        .any(|character| character.is_control() || matches!(character, ':' | '/' | '\\'))
    {
        return Err(path_error(
            "unsafe_path_character",
            "path contains a platform-ambiguous or control character",
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
        return Err(path_error(
            "reserved_platform_name",
            "path uses a reserved platform device name",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileShareMode {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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
    fn mutates(self) -> bool {
        matches!(
            self,
            Self::Upload | Self::CreateDirectory | Self::Rename | Self::Delete
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileAclEffect {
    Allow,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileAclRule {
    pub id: String,
    pub subjects: BTreeSet<SubjectSelector>,
    pub path_prefix: SharedPath,
    pub actions: BTreeSet<FileAction>,
    pub effect: FileAclEffect,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileAcl {
    pub rules: Vec<FileAclRule>,
}

impl FileAcl {
    pub fn validate(&self, field: &str) -> ContractResult {
        let mut issues = Vec::new();
        let ids = self
            .rules
            .iter()
            .map(|rule| rule.id.as_str())
            .collect::<Vec<_>>();
        require_unique(&ids, &format!("{field}.rules"), &mut issues);
        for (index, rule) in self.rules.iter().enumerate() {
            require_non_empty(
                &rule.id,
                &format!("{field}.rules[{index}].id"),
                128,
                &mut issues,
            );
            if rule.subjects.is_empty() {
                issues.push(ContractIssue::new(
                    "file_acl_subjects_required",
                    format!("{field}.rules[{index}].subjects"),
                    "a file ACL rule requires at least one subject",
                ));
            }
            if rule.actions.is_empty() {
                issues.push(ContractIssue::new(
                    "file_acl_actions_required",
                    format!("{field}.rules[{index}].actions"),
                    "a file ACL rule requires at least one action",
                ));
            }
        }
        finish(issues)
    }

    fn layer_decision(
        &self,
        principal: &PermissionPrincipal,
        path: &SharedPath,
        action: FileAction,
    ) -> FileLayerDecision {
        let mut allowed = false;
        for rule in &self.rules {
            let path_matches = path.is_same_or_descendant_of(&rule.path_prefix);
            let subject_matches = rule
                .subjects
                .iter()
                .any(|subject| file_subject_matches(subject, principal));
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

fn file_subject_matches(subject: &SubjectSelector, principal: &PermissionPrincipal) -> bool {
    match subject {
        SubjectSelector::Node(node_id) => principal.node_id == *node_id,
        SubjectSelector::Owner => principal.is_owner,
        SubjectSelector::Role(role) => principal.roles.contains(role),
        SubjectSelector::Group(group_id) => principal.groups.contains(group_id),
        SubjectSelector::Capability(capability_id) => {
            principal.capabilities.contains(capability_id)
        }
        SubjectSelector::AnyMember => true,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileLayerDecision {
    Allowed,
    Denied,
    NoAllow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileAccessDenialCode {
    NetworkDisabled,
    ReadOnlyShare,
    AclDenied,
    AclNoAllow,
    AuthenticatedMemberRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileAccessDenial {
    pub code: FileAccessDenialCode,
    pub layer: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileAccessRequest {
    pub path: SharedPath,
    pub action: FileAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileQuota {
    pub total_bytes: u64,
    pub per_member_bytes: u64,
    pub max_file_bytes: u64,
    pub max_entries: u64,
    pub reserve_bytes: u64,
    pub max_concurrent_transfers: u32,
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
    fn validate(&self) -> ContractResult {
        let mut issues = Vec::new();
        if self.total_bytes == 0 {
            issues.push(ContractIssue::new(
                "invalid_file_quota",
                "quota.total_bytes",
                "total quota must be greater than zero",
            ));
        }
        if self.per_member_bytes > self.total_bytes {
            issues.push(ContractIssue::new(
                "invalid_file_quota",
                "quota.per_member_bytes",
                "member quota cannot exceed total quota",
            ));
        }
        if self.max_file_bytes > self.per_member_bytes {
            issues.push(ContractIssue::new(
                "invalid_file_quota",
                "quota.max_file_bytes",
                "maximum file size cannot exceed the member quota",
            ));
        }
        if self.reserve_bytes >= self.total_bytes {
            issues.push(ContractIssue::new(
                "invalid_file_quota",
                "quota.reserve_bytes",
                "reserved space must be less than total quota",
            ));
        }
        finish(issues)
    }

    /// Check a prospective upload. `replaced_bytes` is non-zero only for a
    /// version-checked replacement, and entry/transfer counts are current usage.
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileQuotaUsage {
    pub total_bytes: u64,
    pub member_bytes: u64,
    pub entries: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymlinkPolicy {
    /// Version one default. Any symbolic link/reparse point is rejected.
    Reject,
    /// An adapter may follow a link only after proving both the pre-open and
    /// opened real paths stay below the selected root.
    AllowWithinRoot,
}

/// Proof that a platform adapter canonicalized the path immediately before and
/// after opening the object. A failed check must close the handle and deny.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileOpenValidation {
    pub policy: SymlinkPolicy,
    pub canonical_relative_path: SharedPath,
    pub before_open_inside_root: bool,
    pub after_open_inside_root: bool,
    pub symlink_followed: bool,
    pub validated_at: UnixMillis,
}

impl FileOpenValidation {
    pub fn permits_access(&self) -> bool {
        self.before_open_inside_root
            && self.after_open_inside_root
            && (!self.symlink_followed || self.policy == SymlinkPolicy::AllowWithinRoot)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConcurrentNamePolicy {
    Reject,
    IfUnchanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    pub created_at: UnixMillis,
    pub updated_at: UnixMillis,
}

impl FileShare {
    pub fn validate(&self) -> ContractResult {
        let mut issues = Vec::new();
        if self.schema_version != FILE_SHARE_CONTRACT_VERSION {
            issues.push(ContractIssue::new(
                "unsupported_schema_version",
                "schema_version",
                format!("file share schema version must be {FILE_SHARE_CONTRACT_VERSION}"),
            ));
        }
        require_non_empty(&self.display_name, "display_name", 128, &mut issues);
        if self.revision == 0 {
            issues.push(ContractIssue::new(
                "invalid_file_share_revision",
                "revision",
                "file share revision must be greater than zero",
            ));
        }
        if !self.audit_enabled {
            issues.push(ContractIssue::new(
                "file_audit_required",
                "audit_enabled",
                "file share operations must be auditable",
            ));
        }
        if self.updated_at.is_before(self.created_at) {
            issues.push(ContractIssue::new(
                "invalid_file_share_time",
                "updated_at",
                "update time must not precede creation time",
            ));
        }
        append_issues(&mut issues, self.network_acl.validate("network_acl"));
        append_issues(&mut issues, self.local_acl.validate("local_acl"));
        append_issues(&mut issues, self.quota.validate());
        finish(issues)
    }

    /// Network and local ACLs are intersected, and either layer can veto. Network
    /// ownership or administrator role is not a file-content bypass.
    pub fn authorize(
        &self,
        request: &FileAccessRequest,
        principal: &PermissionPrincipal,
        context: &AccessContext,
        network_enabled: bool,
    ) -> Result<(), FileAccessDenial> {
        if !network_enabled {
            return Err(FileAccessDenial {
                code: FileAccessDenialCode::NetworkDisabled,
                layer: "network".into(),
            });
        }
        if context
            .network_id
            .is_some_and(|network_id| network_id != self.network_id)
        {
            return Err(FileAccessDenial {
                code: FileAccessDenialCode::AuthenticatedMemberRequired,
                layer: "network".into(),
            });
        }
        if request.action.mutates() && self.mode == FileShareMode::ReadOnly {
            return Err(FileAccessDenial {
                code: FileAccessDenialCode::ReadOnlyShare,
                layer: "share".into(),
            });
        }
        for (layer, acl) in [
            ("network_acl", &self.network_acl),
            ("local_acl", &self.local_acl),
        ] {
            match acl.layer_decision(principal, &request.path, request.action) {
                FileLayerDecision::Allowed => {}
                FileLayerDecision::Denied => {
                    return Err(FileAccessDenial {
                        code: FileAccessDenialCode::AclDenied,
                        layer: layer.into(),
                    });
                }
                FileLayerDecision::NoAllow => {
                    return Err(FileAccessDenial {
                        code: FileAccessDenialCode::AclNoAllow,
                        layer: layer.into(),
                    });
                }
            }
        }
        Ok(())
    }
}

fn append_issues(target: &mut Vec<ContractIssue>, result: ContractResult) {
    if let Err(issues) = result {
        target.extend(issues);
    }
}

fn finish(issues: Vec<ContractIssue>) -> ContractResult {
    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentHash(String);

impl ContentHash {
    pub fn parse(value: impl Into<String>) -> Result<Self, ContractIssue> {
        let value = value.into();
        let valid = value.strip_prefix("sha256:").is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        });
        if !valid {
            return Err(ContractIssue::new(
                "invalid_content_hash",
                "content_hash",
                "content hash must use sha256:<64 lowercase hex characters>",
            ));
        }
        Ok(Self(value))
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(format!("sha256:{:x}", Sha256::digest(bytes)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileVersion {
    pub content_hash: ContentHash,
    pub size: u64,
    pub revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferDirection {
    Upload,
    Download,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChunk {
    pub index: u32,
    pub offset: u64,
    pub length: u64,
    pub checksum: ContentHash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferManifest {
    pub total_bytes: u64,
    pub chunk_size: u64,
    pub content_hash: ContentHash,
    pub chunks: Vec<FileChunk>,
}

impl TransferManifest {
    pub fn validate(&self) -> ContractResult {
        let mut issues = Vec::new();
        if self.chunk_size == 0 || self.chunk_size > MAX_TRANSFER_CHUNK_BYTES {
            issues.push(ContractIssue::new(
                "invalid_transfer_chunk_size",
                "manifest.chunk_size",
                format!("chunk size must be between 1 and {MAX_TRANSFER_CHUNK_BYTES} bytes"),
            ));
        }
        let expected_count = if self.total_bytes == 0 {
            0
        } else {
            usize::try_from(self.total_bytes.div_ceil(self.chunk_size.max(1))).unwrap_or(usize::MAX)
        };
        if self.chunks.len() != expected_count {
            issues.push(ContractIssue::new(
                "invalid_transfer_chunk_count",
                "manifest.chunks",
                "chunk count does not cover the declared file size",
            ));
        }
        let mut covered = 0_u64;
        for (expected_index, chunk) in self.chunks.iter().enumerate() {
            let index = u32::try_from(expected_index).unwrap_or(u32::MAX);
            let expected_offset = u64::from(index).saturating_mul(self.chunk_size);
            let expected_length = self
                .total_bytes
                .saturating_sub(expected_offset)
                .min(self.chunk_size);
            if chunk.index != index
                || chunk.offset != expected_offset
                || chunk.length != expected_length
                || chunk.length == 0
            {
                issues.push(ContractIssue::new(
                    "invalid_transfer_chunk_range",
                    format!("manifest.chunks[{expected_index}]"),
                    "chunks must be sequential, contiguous, and exactly cover the file",
                ));
            }
            covered = covered.saturating_add(chunk.length);
        }
        if covered != self.total_bytes {
            issues.push(ContractIssue::new(
                "invalid_transfer_size",
                "manifest.total_bytes",
                "chunk lengths must sum to the declared file size",
            ));
        }
        finish(issues)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferBinding {
    pub transfer_id: OperationId,
    pub operation_id: OperationId,
    pub share_id: FileShareId,
    pub actor: NodeId,
    pub path: SharedPath,
    pub direction: TransferDirection,
    pub file_version: Option<FileVersion>,
    pub share_revision: u64,
    pub expires_at: UnixMillis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeRequest {
    pub transfer_id: OperationId,
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
    Contract(Vec<ContractIssue>),
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

impl fmt::Display for FileTransferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(issues) => write!(f, "invalid transfer contract: {issues:?}"),
            Self::Expired => f.write_str("transfer resume window expired"),
            Self::PermissionRevoked => f.write_str("file permission was revoked"),
            Self::VersionChanged => f.write_str("file version changed"),
            Self::BindingMismatch => f.write_str("transfer resume binding does not match"),
            Self::InvalidState => f.write_str("transfer state does not allow this operation"),
            Self::ChunkOutOfRange => f.write_str("chunk is not part of the transfer manifest"),
            Self::ChunkCorrupt => f.write_str("chunk checksum does not match"),
            Self::Incomplete => f.write_str("not every declared chunk was received"),
            Self::IntegrityMismatch => f.write_str("assembled file hash or size does not match"),
        }
    }
}

impl std::error::Error for FileTransferError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferSession {
    pub binding: TransferBinding,
    pub manifest: TransferManifest,
    pub state: TransferState,
    pub received_chunks: BTreeMap<u32, ContentHash>,
    pub revision: u64,
    pub updated_at: UnixMillis,
}

impl TransferSession {
    pub fn new(
        binding: TransferBinding,
        manifest: TransferManifest,
        now: UnixMillis,
    ) -> Result<Self, FileTransferError> {
        manifest.validate().map_err(FileTransferError::Contract)?;
        if binding.share_revision == 0 {
            return Err(FileTransferError::Contract(vec![ContractIssue::new(
                "invalid_share_revision",
                "binding.share_revision",
                "share revision must be greater than zero",
            )]));
        }
        Ok(Self {
            binding,
            manifest,
            state: TransferState::Queued,
            received_chunks: BTreeMap::new(),
            revision: 1,
            updated_at: now,
        })
    }

    /// Resume is valid only for the same actor, operation, share, path,
    /// direction, content hash, share policy revision, and current file version.
    /// It never clips or repairs a mismatched binding.
    pub fn resume(
        &mut self,
        request: &ResumeRequest,
        current_file_version: Option<&FileVersion>,
        share_revision: u64,
        authorized: bool,
        now: UnixMillis,
    ) -> Result<(), FileTransferError> {
        if now.0 >= self.binding.expires_at.0 {
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
            || share_revision != self.binding.share_revision
        {
            return Err(FileTransferError::BindingMismatch);
        }
        if current_file_version != self.binding.file_version.as_ref() {
            return Err(FileTransferError::VersionChanged);
        }
        if self.state.is_terminal() || self.state == TransferState::Verifying {
            return Err(FileTransferError::InvalidState);
        }
        self.state = TransferState::Active;
        self.revision = self.revision.saturating_add(1);
        self.updated_at = now;
        Ok(())
    }

    pub fn pause(&mut self, now: UnixMillis) -> Result<(), FileTransferError> {
        if self.state != TransferState::Active {
            return Err(FileTransferError::InvalidState);
        }
        self.state = TransferState::Paused;
        self.revision = self.revision.saturating_add(1);
        self.updated_at = now;
        Ok(())
    }

    /// Record a chunk after checking its manifest range and SHA-256 digest.
    /// Replaying the same chunk is idempotent; different bytes for an already
    /// received index are rejected.
    pub fn record_chunk(
        &mut self,
        index: u32,
        bytes: &[u8],
        now: UnixMillis,
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
        self.updated_at = now;
        Ok(())
    }

    /// Verify all chunks and the assembled content before the runtime atomically
    /// renames the temporary object into its final path.
    pub fn verify_and_complete(
        &mut self,
        assembled_bytes: &[u8],
        now: UnixMillis,
    ) -> Result<(), FileTransferError> {
        if self.state != TransferState::Active && self.state != TransferState::Verifying {
            return Err(FileTransferError::InvalidState);
        }
        if self.received_chunks.len() != self.manifest.chunks.len() {
            self.state = TransferState::Failed;
            return Err(FileTransferError::Incomplete);
        }
        self.state = TransferState::Verifying;
        if u64::try_from(assembled_bytes.len()).unwrap_or(u64::MAX) != self.manifest.total_bytes
            || ContentHash::from_bytes(assembled_bytes) != self.manifest.content_hash
        {
            self.state = TransferState::Failed;
            self.revision = self.revision.saturating_add(1);
            self.updated_at = now;
            return Err(FileTransferError::IntegrityMismatch);
        }
        self.state = TransferState::Completed;
        self.revision = self.revision.saturating_add(1);
        self.updated_at = now;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::membership::NetworkRole;

    fn node(byte: u8) -> NodeId {
        NodeId::from_bytes([byte; 32])
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
            ]),
            effect: FileAclEffect::Allow,
        }
    }

    fn share(owner: NodeId) -> FileShare {
        FileShare {
            schema_version: FILE_SHARE_CONTRACT_VERSION,
            id: FileShareId::from_bytes([9; 16]),
            network_id: NetworkId::from_bytes([7; 32]),
            owner,
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
            created_at: UnixMillis(1),
            updated_at: UnixMillis(1),
        }
    }

    fn principal(node_id: NodeId) -> PermissionPrincipal {
        let mut principal = PermissionPrincipal::new(node_id);
        principal.roles.insert(NetworkRole::Member);
        principal
    }

    fn context(network_id: NetworkId) -> AccessContext {
        AccessContext {
            network_id: Some(network_id),
            ..AccessContext::default()
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
    }

    #[test]
    fn deny_wins_and_both_network_and_local_layers_must_allow() {
        let owner = node(1);
        let member = node(2);
        let mut share = share(owner);
        assert!(share
            .authorize(
                &FileAccessRequest {
                    path: SharedPath::parse("docs/readme.md").expect("path"),
                    action: FileAction::Download,
                },
                &principal(member),
                &context(share.network_id),
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
                &FileAccessRequest {
                    path: SharedPath::parse("docs/private/key.txt").expect("path"),
                    action: FileAction::Download,
                },
                &principal(member),
                &context(share.network_id),
                true,
            ),
            Err(FileAccessDenial {
                code: FileAccessDenialCode::AclDenied,
                layer: "network_acl".into(),
            })
        );
        share.local_acl.rules.clear();
        assert_eq!(
            share.authorize(
                &FileAccessRequest {
                    path: SharedPath::parse("public.txt").expect("path"),
                    action: FileAction::Download,
                },
                &principal(member),
                &context(share.network_id),
                true,
            ),
            Err(FileAccessDenial {
                code: FileAccessDenialCode::AclNoAllow,
                layer: "local_acl".into(),
            })
        );
    }

    #[test]
    fn read_only_and_quota_are_enforced_before_mutation() {
        let owner = node(1);
        let member = node(2);
        let mut share = share(owner);
        share.mode = FileShareMode::ReadOnly;
        assert_eq!(
            share.authorize(
                &FileAccessRequest {
                    path: SharedPath::parse("new.txt").expect("path"),
                    action: FileAction::Upload,
                },
                &principal(member),
                &context(share.network_id),
                true,
            ),
            Err(FileAccessDenial {
                code: FileAccessDenialCode::ReadOnlyShare,
                layer: "share".into(),
            })
        );

        let quota = FileQuota {
            total_bytes: 1_000,
            per_member_bytes: 800,
            max_file_bytes: 400,
            max_entries: 10,
            reserve_bytes: 100,
            max_concurrent_transfers: 2,
            max_bandwidth_kbps: 0,
        };
        let usage = FileQuotaUsage {
            total_bytes: 750,
            member_bytes: 300,
            entries: 3,
        };
        assert_eq!(
            quota.check_upload(400, 0, &usage, 0),
            Err(FileQuotaError::TotalQuotaExceeded)
        );
        assert_eq!(
            quota.check_upload(401, 0, &usage, 0),
            Err(FileQuotaError::FileTooLarge)
        );
        assert!(quota.check_upload(150, 0, &usage, 1).is_ok());
    }

    fn manifest(bytes: &[u8], chunk_size: u64) -> TransferManifest {
        let chunks = bytes
            .chunks(chunk_size as usize)
            .enumerate()
            .map(|(index, chunk)| FileChunk {
                index: u32::try_from(index).expect("index"),
                offset: index as u64 * chunk_size,
                length: chunk.len() as u64,
                checksum: ContentHash::from_bytes(chunk),
            })
            .collect();
        TransferManifest {
            total_bytes: bytes.len() as u64,
            chunk_size,
            content_hash: ContentHash::from_bytes(bytes),
            chunks,
        }
    }

    fn session(bytes: &[u8], binding: TransferBinding) -> TransferSession {
        TransferSession::new(binding, manifest(bytes, 4), UnixMillis(1)).expect("session")
    }

    fn binding(path: &str, actor: NodeId) -> TransferBinding {
        TransferBinding {
            transfer_id: OperationId::from_bytes([1; 16]),
            operation_id: OperationId::from_bytes([2; 16]),
            share_id: FileShareId::from_bytes([3; 16]),
            actor,
            path: SharedPath::parse(path).expect("path"),
            direction: TransferDirection::Upload,
            file_version: None,
            share_revision: 1,
            expires_at: UnixMillis(100),
        }
    }

    fn request(session: &TransferSession) -> ResumeRequest {
        ResumeRequest {
            transfer_id: session.binding.transfer_id,
            operation_id: session.binding.operation_id,
            share_id: session.binding.share_id,
            actor: session.binding.actor,
            path: session.binding.path.clone(),
            direction: session.binding.direction,
            content_hash: session.manifest.content_hash.clone(),
            share_revision: session.binding.share_revision,
        }
    }

    #[test]
    fn resume_is_bound_to_identity_share_revision_and_file_version() {
        let actor = node(2);
        let content = b"abcdefgh";
        let mut active = session(content, binding("upload.bin", actor));
        active
            .resume(&request(&active), None, 1, true, UnixMillis(2))
            .expect("resume");
        active.pause(UnixMillis(3)).expect("pause");
        active
            .resume(&request(&active), None, 1, true, UnixMillis(4))
            .expect("resume after pause");

        let mut stale = request(&active);
        stale.actor = node(3);
        assert_eq!(
            active.resume(&stale, None, 1, true, UnixMillis(5)),
            Err(FileTransferError::BindingMismatch)
        );
        assert_eq!(
            active.resume(&request(&active), None, 2, true, UnixMillis(5)),
            Err(FileTransferError::BindingMismatch)
        );
        let changed_version = FileVersion {
            content_hash: ContentHash::from_bytes(b"old"),
            size: 3,
            revision: 1,
        };
        assert_eq!(
            active.resume(
                &request(&active),
                Some(&changed_version),
                1,
                true,
                UnixMillis(5)
            ),
            Err(FileTransferError::VersionChanged)
        );
    }

    #[test]
    fn chunks_must_be_complete_and_content_must_match_before_completion() {
        let content = b"abcdefghij";
        let mut active = session(content, binding("upload.bin", node(2)));
        active
            .resume(&request(&active), None, 1, true, UnixMillis(2))
            .expect("resume");
        active
            .record_chunk(0, b"abcd", UnixMillis(3))
            .expect("chunk");
        active
            .record_chunk(0, b"abcd", UnixMillis(3))
            .expect("replay");
        assert_eq!(
            active.record_chunk(1, b"WXYZ", UnixMillis(4)),
            Err(FileTransferError::ChunkCorrupt)
        );
        active
            .record_chunk(1, b"efgh", UnixMillis(4))
            .expect("chunk");
        assert_eq!(
            active.verify_and_complete(content, UnixMillis(5)),
            Err(FileTransferError::Incomplete)
        );

        let mut complete = session(content, binding("upload.bin", node(2)));
        complete
            .resume(&request(&complete), None, 1, true, UnixMillis(2))
            .expect("resume");
        for (index, chunk) in content.chunks(4).enumerate() {
            complete
                .record_chunk(u32::try_from(index).expect("index"), chunk, UnixMillis(3))
                .expect("chunk");
        }
        assert_eq!(
            complete.verify_and_complete(b"xxxxxxxxxx", UnixMillis(4)),
            Err(FileTransferError::IntegrityMismatch)
        );
        assert_eq!(complete.state, TransferState::Failed);
    }

    #[test]
    fn open_validation_requires_pre_and_post_root_checks() {
        let validation = FileOpenValidation {
            policy: SymlinkPolicy::AllowWithinRoot,
            canonical_relative_path: SharedPath::parse("dir/file").expect("path"),
            before_open_inside_root: true,
            after_open_inside_root: false,
            symlink_followed: true,
            validated_at: UnixMillis(1),
        };
        assert!(!validation.permits_access());
    }
}
