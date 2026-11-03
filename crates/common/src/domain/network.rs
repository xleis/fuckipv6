use std::collections::BTreeSet;

use crate::crypto::Identity;

use ed25519_dalek::{Signature as DalekSignature, Verifier, VerifyingKey};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::capability::CapabilityKind;
use super::ids::{NetworkId, NodeId, PublicKey, Signature};
use super::{DomainError, DomainResult};

/// How much of a network's public profile is discoverable without membership.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkVisibility {
    Public,
    #[default]
    Unlisted,
    Private,
}

/// Admission policy selected by the network owner.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkJoinMode {
    /// Requests are accepted immediately.
    Open,
    /// Requests enter `pending_approval`.
    ApprovalRequired,
    /// The node must first consume a signed, single-use invitation.
    #[default]
    InviteOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryTransport {
    Quic,
    Tcp,
    Local,
}

/// A decoupled discovery hint. It is never part of the stable network identity.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkEndpoint {
    pub transport: DiscoveryTransport,
    pub host: String,
    pub port: u16,
}

impl NetworkEndpoint {
    fn validate(&self) -> DomainResult<()> {
        if self.host.is_empty() || self.host.len() > 253 || self.port == 0 {
            return Err(DomainError::invalid_value("network endpoint is invalid"));
        }
        Ok(())
    }
}

/// Stable network metadata. The identifier is derived from `public_key`, not
/// from a Coordinator hostname, endpoint or mutable display name.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkRecord {
    pub network_id: NetworkId,
    pub public_key: PublicKey,
    pub name: String,
    pub purpose: String,
    /// Stable, machine-readable icon token. It is presentation metadata and is
    /// not part of the network identifier.
    #[serde(default = "default_network_icon")]
    pub icon: String,
    /// BCP-47-like language tag for the network's public profile.
    #[serde(default = "default_network_language")]
    pub language: String,
    /// ISO 3166-1 alpha-2 region, or `ZZ` when the owner does not disclose one.
    #[serde(default = "default_network_region")]
    pub region: String,
    /// Stable discovery tags. The domain validates uniqueness after canonical
    /// normalization; adapters should submit normalized or raw values and rely
    /// on the shared preview/apply path.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Owner-declared capabilities. A declaration is discoverability metadata,
    /// not proof that the capability is operational on any particular node.
    #[serde(default)]
    pub capability_declarations: Vec<CapabilityKind>,
    pub visibility: NetworkVisibility,
    pub join_mode: NetworkJoinMode,
    pub owner: NodeId,
    #[serde(default)]
    pub administrators: Vec<NodeId>,
    #[serde(default)]
    pub members: Vec<NodeId>,
    pub revision: u64,
    pub policy_revision: u64,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    /// Signed public profile. Legacy owner records may omit this field; UI
    /// previews must surface an `UnsignedLegacy` warning instead of treating
    /// the mutable record itself as a verified public document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_profile: Option<NetworkPublicProfileRecord>,
}

impl NetworkRecord {
    pub fn validate(&self) -> DomainResult<()> {
        let expected = NetworkId::from_public_key(self.public_key.as_str().as_bytes());
        if self.network_id != expected {
            return Err(DomainError::invariant(
                "network id does not match the network public key",
            ));
        }
        validate_network_name(&self.name)?;
        validate_network_purpose(&self.purpose)?;
        validate_network_icon(&self.icon)?;
        validate_network_language(&self.language)?;
        validate_network_region(&self.region)?;
        validate_network_tags(&self.tags)?;
        validate_capability_declarations(&self.capability_declarations)?;
        if self.revision == 0 || self.policy_revision == 0 {
            return Err(DomainError::invalid_value(
                "network and policy revisions must be positive",
            ));
        }
        if self.updated_at_unix_ms < self.created_at_unix_ms {
            return Err(DomainError::invariant(
                "network update time predates creation",
            ));
        }
        let mut administrators = BTreeSet::new();
        for administrator in &self.administrators {
            if !administrators.insert(administrator) {
                return Err(DomainError::invariant("duplicate network administrator"));
            }
        }
        if administrators.contains(&self.owner) {
            return Err(DomainError::invariant(
                "network owner must not also be listed as administrator",
            ));
        }
        let mut members = BTreeSet::new();
        for member in &self.members {
            if !members.insert(member) {
                return Err(DomainError::invariant("duplicate network member"));
            }
        }
        if !members.contains(&self.owner)
            || self
                .administrators
                .iter()
                .any(|administrator| !members.contains(administrator))
        {
            return Err(DomainError::invariant(
                "network owner and administrators must be members",
            ));
        }
        if let Some(profile) = &self.signed_profile {
            profile.validate()?;
            profile.validate_against_record(self)?;
        }
        Ok(())
    }

    /// Build the exact fields that are allowed to cross the public profile
    /// boundary. Member IDs, addresses, endpoints, topology, activity and
    /// secrets are intentionally absent from this type.
    pub fn public_profile_document(
        &self,
        issued_at_unix_ms: u64,
        expires_at_unix_ms: u64,
    ) -> DomainResult<NetworkPublicProfileDocument> {
        let document = NetworkPublicProfileDocument {
            schema_version: NETWORK_PUBLIC_PROFILE_SCHEMA_VERSION,
            network_id: self.network_id.clone(),
            public_key: self.public_key.clone(),
            owner: self.owner.clone(),
            name: self.name.clone(),
            purpose: self.purpose.clone(),
            icon: self.icon.clone(),
            language: self.language.clone(),
            region: self.region.clone(),
            tags: self.tags.clone(),
            capability_declarations: self.capability_declarations.clone(),
            visibility: self.visibility,
            join_mode: self.join_mode,
            revision: self.revision,
            issued_at_unix_ms,
            expires_at_unix_ms,
        };
        document.validate()?;
        Ok(document)
    }

    /// Issue a fresh signed profile using the network's independent signing
    /// identity. The identity's public key must match the record.
    pub fn sign_public_profile(
        &mut self,
        identity: &Identity,
        issued_at_unix_ms: u64,
        expires_at_unix_ms: u64,
    ) -> DomainResult<()> {
        let identity_public_key = PublicKey::from_bytes(identity.public_key_bytes());
        if identity_public_key != self.public_key {
            return Err(DomainError::invariant(
                "network signing identity does not match the network record",
            ));
        }
        let document = self.public_profile_document(issued_at_unix_ms, expires_at_unix_ms)?;
        self.signed_profile = Some(NetworkPublicProfileRecord::sign(document, identity));
        self.validate()
    }

    /// Apply only public-profile fields. Governance membership and policy
    /// revisions require separate audited operations.
    pub fn apply_profile_update(
        &mut self,
        update: &NetworkProfileUpdate,
        expected_revision: u64,
        now_unix_ms: u64,
    ) -> DomainResult<Vec<NetworkProfileDiffEntry>> {
        update.validate()?;
        if self.revision != expected_revision {
            return Err(DomainError::new(
                "domain.network_profile_revision_conflict",
                "network profile revision no longer matches the preview",
            ));
        }
        let diff = network_profile_diff(self, update);
        if diff.is_empty() {
            return Ok(diff);
        }
        if now_unix_ms < self.created_at_unix_ms {
            return Err(DomainError::invariant(
                "network update time predates creation",
            ));
        }
        self.purpose = update.purpose.clone();
        self.icon = update.icon.clone();
        self.language = update.language.clone();
        self.region = update.region.clone();
        self.tags = canonical_tags(&update.tags)?;
        self.capability_declarations = canonical_capabilities(&update.capability_declarations)?;
        self.visibility = update.visibility;
        self.join_mode = update.join_mode;
        self.revision = self.revision.saturating_add(1);
        self.updated_at_unix_ms = self.updated_at_unix_ms.max(now_unix_ms);
        // The old signature no longer covers the mutated fields. The caller
        // must issue a new signature before persisting the record.
        self.signed_profile = None;
        self.validate()?;
        Ok(diff)
    }

    /// Create the first revision owned by the identity that created the
    /// network. The owner is a member with full owner authority; the separate
    /// administrator list starts empty because the domain forbids duplicating
    /// the owner there.
    pub fn create_owned(
        profile: NetworkCreateProfile,
        public_key: PublicKey,
        owner: NodeId,
        now_unix_ms: u64,
    ) -> DomainResult<Self> {
        let record = Self {
            network_id: NetworkId::from_public_key(public_key.as_str().as_bytes()),
            public_key,
            name: profile.name,
            purpose: profile.purpose,
            icon: profile.icon,
            language: profile.language,
            region: profile.region,
            tags: profile.tags,
            capability_declarations: profile.capability_declarations,
            visibility: profile.visibility,
            join_mode: profile.join_mode,
            owner: owner.clone(),
            administrators: Vec::new(),
            members: vec![owner],
            revision: 1,
            policy_revision: 1,
            created_at_unix_ms: now_unix_ms,
            updated_at_unix_ms: now_unix_ms,
            signed_profile: None,
        };
        let mut record = record;
        record.tags = canonical_tags(&record.tags)?;
        record.capability_declarations = canonical_capabilities(&record.capability_declarations)?;
        record.validate()?;
        Ok(record)
    }

    /// Rename mutable profile metadata without changing the identifier derived
    /// from the immutable network public key.
    pub fn rename(&mut self, new_name: impl Into<String>, now_unix_ms: u64) -> DomainResult<()> {
        let new_name = new_name.into();
        validate_network_name(&new_name)?;
        if now_unix_ms < self.created_at_unix_ms {
            return Err(DomainError::invariant(
                "network update time predates creation",
            ));
        }
        if self.name != new_name {
            self.name = new_name;
            self.revision = self.revision.saturating_add(1);
            self.updated_at_unix_ms = self.updated_at_unix_ms.max(now_unix_ms);
        }
        self.validate()
    }
}

/// Owner-supplied public profile used to create a network. Hostnames, ports and
/// runtime settings deliberately do not participate in this contract.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkCreateProfile {
    pub name: String,
    pub purpose: String,
    pub icon: String,
    pub language: String,
    #[serde(default = "default_network_region")]
    pub region: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub capability_declarations: Vec<CapabilityKind>,
    pub visibility: NetworkVisibility,
    pub join_mode: NetworkJoinMode,
}

impl NetworkCreateProfile {
    pub fn validate(&self) -> DomainResult<()> {
        validate_network_name(&self.name)?;
        validate_network_purpose(&self.purpose)?;
        validate_network_icon(&self.icon)?;
        validate_network_language(&self.language)?;
        validate_network_region(&self.region)?;
        validate_network_tags(&self.tags)?;
        validate_capability_declarations(&self.capability_declarations)
    }

    pub fn normalized(mut self) -> DomainResult<Self> {
        self.tags = canonical_tags(&self.tags)?;
        self.capability_declarations = canonical_capabilities(&self.capability_declarations)?;
        self.validate()?;
        Ok(self)
    }
}

/// Owner-editable public fields for an existing network. The network name and
/// stable identity deliberately cannot be changed through this command.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkProfileUpdate {
    pub purpose: String,
    pub icon: String,
    pub language: String,
    pub region: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub capability_declarations: Vec<CapabilityKind>,
    pub visibility: NetworkVisibility,
    pub join_mode: NetworkJoinMode,
}

impl NetworkProfileUpdate {
    pub fn from_record(record: &NetworkRecord) -> Self {
        Self {
            purpose: record.purpose.clone(),
            icon: record.icon.clone(),
            language: record.language.clone(),
            region: record.region.clone(),
            tags: record.tags.clone(),
            capability_declarations: record.capability_declarations.clone(),
            visibility: record.visibility,
            join_mode: record.join_mode,
        }
    }

    pub fn validate(&self) -> DomainResult<()> {
        validate_network_purpose(&self.purpose)?;
        validate_network_icon(&self.icon)?;
        validate_network_language(&self.language)?;
        validate_network_region(&self.region)?;
        validate_network_tags(&self.tags)?;
        validate_capability_declarations(&self.capability_declarations)
    }

    pub fn normalized(mut self) -> DomainResult<Self> {
        self.tags = canonical_tags(&self.tags)?;
        self.capability_declarations = canonical_capabilities(&self.capability_declarations)?;
        self.validate()?;
        Ok(self)
    }
}

pub const DEFAULT_NETWORK_PURPOSE: &str = "Owner-created network";
pub const DEFAULT_NETWORK_ICON: &str = "network";
pub const DEFAULT_NETWORK_LANGUAGE: &str = "und";
pub const DEFAULT_NETWORK_REGION: &str = "ZZ";
pub const NETWORK_PUBLIC_PROFILE_SCHEMA_VERSION: u16 = 1;
pub const NETWORK_PUBLIC_PROFILE_VALIDITY_MS: u64 = 30 * 24 * 60 * 60 * 1000;
pub const NETWORK_DISCOVERY_SCHEMA_VERSION: u16 = 2;
pub const NETWORK_DISCOVERY_VALIDITY_MS: u64 = 7 * 24 * 60 * 60 * 1000;
pub const NETWORK_PROFILE_MAX_TAGS: usize = 12;
pub const NETWORK_PROFILE_MAX_TAG_CHARS: usize = 32;

pub fn default_network_purpose() -> String {
    DEFAULT_NETWORK_PURPOSE.into()
}

pub fn default_network_icon() -> String {
    DEFAULT_NETWORK_ICON.into()
}

pub fn default_network_language() -> String {
    DEFAULT_NETWORK_LANGUAGE.into()
}

pub fn default_network_region() -> String {
    DEFAULT_NETWORK_REGION.into()
}

fn validate_network_name(name: &str) -> DomainResult<()> {
    if name.is_empty() || name.len() > 80 || name.trim() != name || name == "." || name == ".." {
        return Err(DomainError::invalid_value("network name is invalid"));
    }
    if name.chars().any(|character| {
        !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
    }) {
        return Err(DomainError::invalid_value("network name is invalid"));
    }
    Ok(())
}

fn validate_network_purpose(purpose: &str) -> DomainResult<()> {
    if purpose.is_empty()
        || purpose.chars().count() > 512
        || purpose.trim() != purpose
        || purpose.chars().any(char::is_control)
    {
        return Err(DomainError::invalid_value("network purpose is invalid"));
    }
    Ok(())
}

fn validate_network_icon(icon: &str) -> DomainResult<()> {
    if icon.is_empty()
        || icon.len() > 64
        || icon.trim() != icon
        || icon.chars().any(|character| {
            !(character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_' | '.'))
        })
    {
        return Err(DomainError::invalid_value("network icon is invalid"));
    }
    Ok(())
}

fn validate_network_region(region: &str) -> DomainResult<()> {
    if region.len() != 2
        || !region
            .chars()
            .all(|character| character.is_ascii_uppercase())
    {
        return Err(DomainError::invalid_value("network region is invalid"));
    }
    Ok(())
}

fn validate_network_tags(tags: &[String]) -> DomainResult<()> {
    if tags.len() > NETWORK_PROFILE_MAX_TAGS {
        return Err(DomainError::invalid_value("network has too many tags"));
    }
    let mut normalized = BTreeSet::new();
    for tag in tags {
        if tag.is_empty()
            || tag.chars().count() > NETWORK_PROFILE_MAX_TAG_CHARS
            || tag.trim() != tag
            || tag.chars().any(|character| {
                character.is_control()
                    || character.is_whitespace()
                    || !(character.is_alphanumeric() || matches!(character, '-' | '_' | '.'))
            })
        {
            return Err(DomainError::invalid_value("network tag is invalid"));
        }
        if !normalized.insert(tag.to_ascii_lowercase()) {
            return Err(DomainError::invalid_value("network tags must be unique"));
        }
    }
    Ok(())
}

fn canonical_tags(tags: &[String]) -> DomainResult<Vec<String>> {
    validate_network_tags(tags)?;
    let mut normalized = tags
        .iter()
        .map(|tag| tag.to_ascii_lowercase())
        .collect::<Vec<_>>();
    normalized.sort();
    normalized.dedup();
    Ok(normalized)
}

fn validate_capability_declarations(capabilities: &[CapabilityKind]) -> DomainResult<()> {
    if capabilities.len() > CapabilityKind::ALL.len() {
        return Err(DomainError::invalid_value(
            "network has too many capability declarations",
        ));
    }
    let mut unique = BTreeSet::new();
    for capability in capabilities {
        if !unique.insert(*capability) {
            return Err(DomainError::invalid_value(
                "network capability declarations must be unique",
            ));
        }
    }
    Ok(())
}

fn canonical_capabilities(capabilities: &[CapabilityKind]) -> DomainResult<Vec<CapabilityKind>> {
    validate_capability_declarations(capabilities)?;
    let mut normalized = capabilities.to_vec();
    normalized.sort();
    Ok(normalized)
}

fn validate_network_language(language: &str) -> DomainResult<()> {
    let mut segments = language.split('-');
    let Some(primary) = segments.next() else {
        return Err(DomainError::invalid_value("network language is invalid"));
    };
    if language.len() > 35
        || !(2..=8).contains(&primary.len())
        || !primary
            .chars()
            .all(|character| character.is_ascii_alphabetic())
        || segments.any(|segment| {
            !(1..=8).contains(&segment.len())
                || !segment
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric())
        })
    {
        return Err(DomainError::invalid_value("network language is invalid"));
    }
    Ok(())
}

/// Public profile data that is safe to show before membership. This type is
/// an allowlist: it has no member identities, assigned addresses, endpoints,
/// topology, activity history or secret references.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkPublicProfileDocument {
    pub schema_version: u16,
    pub network_id: NetworkId,
    pub public_key: PublicKey,
    pub owner: NodeId,
    pub name: String,
    pub purpose: String,
    pub icon: String,
    pub language: String,
    pub region: String,
    pub tags: Vec<String>,
    pub capability_declarations: Vec<CapabilityKind>,
    pub visibility: NetworkVisibility,
    pub join_mode: NetworkJoinMode,
    pub revision: u64,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
}

impl NetworkPublicProfileDocument {
    pub fn validate(&self) -> DomainResult<()> {
        if self.schema_version != NETWORK_PUBLIC_PROFILE_SCHEMA_VERSION {
            return Err(DomainError::invalid_value(
                "network public profile schema version is unsupported",
            ));
        }
        let expected = NetworkId::from_public_key(self.public_key.as_str().as_bytes());
        if self.network_id != expected {
            return Err(DomainError::invariant(
                "network public profile id does not match its public key",
            ));
        }
        validate_network_name(&self.name)?;
        validate_network_purpose(&self.purpose)?;
        validate_network_icon(&self.icon)?;
        validate_network_language(&self.language)?;
        validate_network_region(&self.region)?;
        validate_network_tags(&self.tags)?;
        validate_capability_declarations(&self.capability_declarations)?;
        if self.revision == 0 {
            return Err(DomainError::invalid_value(
                "network public profile revision must be positive",
            ));
        }
        if self.issued_at_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::invariant(
                "network public profile validity interval is empty",
            ));
        }
        Ok(())
    }

    pub fn is_expired_at(&self, now_unix_ms: u64) -> bool {
        now_unix_ms >= self.expires_at_unix_ms
    }

    pub fn signing_payload(&self) -> DomainResult<Vec<u8>> {
        serde_json::to_vec(self)
            .map_err(|_| DomainError::invalid_value("network public profile cannot be encoded"))
    }

    fn validate_against_record(&self, record: &NetworkRecord) -> DomainResult<()> {
        self.validate()?;
        if self.schema_version != NETWORK_PUBLIC_PROFILE_SCHEMA_VERSION
            || self.network_id != record.network_id
            || self.public_key != record.public_key
            || self.owner != record.owner
            || self.name != record.name
            || self.purpose != record.purpose
            || self.icon != record.icon
            || self.language != record.language
            || self.region != record.region
            || self.tags != record.tags
            || self.capability_declarations != record.capability_declarations
            || self.visibility != record.visibility
            || self.join_mode != record.join_mode
            || self.revision != record.revision
        {
            return Err(DomainError::invariant(
                "network public profile does not match the signed record",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkPublicProfileRecord {
    pub document: NetworkPublicProfileDocument,
    pub signature: Signature,
}

impl NetworkPublicProfileRecord {
    pub fn sign(document: NetworkPublicProfileDocument, identity: &Identity) -> Self {
        let signature = identity.sign(&document.signing_payload().unwrap_or_default());
        Self {
            document,
            signature: Signature::from_bytes(signature.to_bytes()),
        }
    }

    pub fn validate(&self) -> DomainResult<()> {
        self.document.validate()?;
        let key = VerifyingKey::from_bytes(&self.document.public_key.to_bytes()?)
            .map_err(|_| DomainError::invalid_value("network public key is invalid"))?;
        let signature = DalekSignature::from_bytes(&self.signature.to_bytes()?);
        key.verify(&self.document.signing_payload()?, &signature)
            .map_err(|_| {
                DomainError::new(
                    "domain.invalid_signature",
                    "network public profile signature is invalid",
                )
            })
    }

    fn validate_against_record(&self, record: &NetworkRecord) -> DomainResult<()> {
        self.validate()?;
        self.document.validate_against_record(record)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkProfileWarningCode {
    UnsignedLegacy,
    ProfileExpired,
    RevisionConflict,
    CapabilityNotObserved,
    CapabilityUndeclared,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkProfileWarning {
    pub code: NetworkProfileWarningCode,
    pub field: Option<String>,
    pub capability: Option<CapabilityKind>,
    pub message: String,
}

impl NetworkProfileWarning {
    pub fn new(code: NetworkProfileWarningCode, message: impl Into<String>) -> Self {
        Self {
            code,
            field: None,
            capability: None,
            message: message.into(),
        }
    }

    pub fn capability(
        code: NetworkProfileWarningCode,
        capability: CapabilityKind,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            field: Some("capability_declarations".into()),
            capability: Some(capability),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkProfileDiffEntry {
    pub path: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

/// Exact public-card projection plus the categories that are withheld. The
/// withheld values themselves never cross this boundary.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkPublicProfileProjection {
    pub candidate: NetworkPublicProfileDocument,
    pub currently_published: Option<NetworkPublicProfileRecord>,
    pub withheld_categories: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkMemberProfileProjection {
    pub profile: NetworkPublicProfileDocument,
    pub member_count: usize,
    pub administrator_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkAdminProfileProjection {
    pub profile: NetworkPublicProfileDocument,
    pub member_count: usize,
    pub administrator_count: usize,
    pub revision: u64,
    pub policy_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkLocalSecretProjection {
    pub identity_present: bool,
    pub public_key_fingerprint: String,
    pub storage: String,
    pub secret_material_included: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkProfileProjections {
    pub public_card: NetworkPublicProfileProjection,
    pub member_view: NetworkMemberProfileProjection,
    pub administrator_view: NetworkAdminProfileProjection,
    pub local_secret: NetworkLocalSecretProjection,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkProfilePreview {
    pub network_name: String,
    pub expected_revision: u64,
    pub current_revision: u64,
    pub candidate_revision: u64,
    pub conflict: bool,
    pub can_apply: bool,
    pub projections: NetworkProfileProjections,
    pub diff: Vec<NetworkProfileDiffEntry>,
    pub warnings: Vec<NetworkProfileWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkProfileApplyResult {
    pub network_name: String,
    pub revision: u64,
    pub previous_revision: u64,
    pub applied_at_unix_ms: u64,
    pub changed_paths: Vec<String>,
    pub signed_profile: NetworkPublicProfileRecord,
    pub projections: NetworkProfileProjections,
    pub warnings: Vec<NetworkProfileWarning>,
    pub audit_action: String,
}

fn network_profile_diff(
    record: &NetworkRecord,
    update: &NetworkProfileUpdate,
) -> Vec<NetworkProfileDiffEntry> {
    let current = NetworkProfileUpdate::from_record(record);
    let mut entries = Vec::new();
    let mut compare = |path: &str, before: String, after: String| {
        if before != after {
            entries.push(NetworkProfileDiffEntry {
                path: path.into(),
                before: Some(before),
                after: Some(after),
            });
        }
    };
    compare("purpose", current.purpose, update.purpose.clone());
    compare("icon", current.icon, update.icon.clone());
    compare("language", current.language, update.language.clone());
    compare("region", current.region, update.region.clone());
    compare(
        "tags",
        current.tags.join(","),
        canonical_tags(&update.tags)
            .unwrap_or_else(|_| update.tags.clone())
            .join(","),
    );
    compare(
        "capability_declarations",
        current
            .capability_declarations
            .iter()
            .map(|capability| capability.wire_name())
            .collect::<Vec<_>>()
            .join(","),
        canonical_capabilities(&update.capability_declarations)
            .unwrap_or_else(|_| update.capability_declarations.clone())
            .iter()
            .map(|capability| capability.wire_name())
            .collect::<Vec<_>>()
            .join(","),
    );
    compare(
        "visibility",
        format!("{:?}", current.visibility).to_ascii_lowercase(),
        format!("{:?}", update.visibility).to_ascii_lowercase(),
    );
    compare(
        "join_mode",
        format!("{:?}", current.join_mode).to_ascii_lowercase(),
        format!("{:?}", update.join_mode).to_ascii_lowercase(),
    );
    entries
}

/// Build the truthful four-layer preview used by creation and editing. The
/// caller may append capability warnings after comparing declarations with the
/// live `NodeCapabilityRegistrySnapshot`.
pub fn preview_network_profile(
    network_name: &str,
    record: &NetworkRecord,
    expected_revision: u64,
    update: &NetworkProfileUpdate,
    now_unix_ms: u64,
) -> DomainResult<NetworkProfilePreview> {
    update.validate()?;
    let conflict = record.revision != expected_revision;
    let diff = network_profile_diff(record, update);
    let candidate_revision = if diff.is_empty() {
        record.revision
    } else {
        record.revision.saturating_add(1)
    };
    let candidate_document = NetworkPublicProfileDocument {
        schema_version: NETWORK_PUBLIC_PROFILE_SCHEMA_VERSION,
        network_id: record.network_id.clone(),
        public_key: record.public_key.clone(),
        owner: record.owner.clone(),
        name: record.name.clone(),
        purpose: update.purpose.clone(),
        icon: update.icon.clone(),
        language: update.language.clone(),
        region: update.region.clone(),
        tags: canonical_tags(&update.tags)?,
        capability_declarations: canonical_capabilities(&update.capability_declarations)?,
        visibility: update.visibility,
        join_mode: update.join_mode,
        revision: candidate_revision,
        issued_at_unix_ms: now_unix_ms,
        expires_at_unix_ms: now_unix_ms.saturating_add(NETWORK_PUBLIC_PROFILE_VALIDITY_MS),
    };
    candidate_document.validate()?;

    let mut warnings = Vec::new();
    if record.signed_profile.is_none() {
        warnings.push(NetworkProfileWarning::new(
            NetworkProfileWarningCode::UnsignedLegacy,
            "该网络资料来自旧版记录，尚无可验证签名；应用变更后会签发新资料",
        ));
    }
    if record
        .signed_profile
        .as_ref()
        .is_some_and(|profile| profile.document.is_expired_at(now_unix_ms))
    {
        warnings.push(NetworkProfileWarning::new(
            NetworkProfileWarningCode::ProfileExpired,
            "当前公网资料已超过有效期，请重新签发",
        ));
    }
    if conflict {
        warnings.push(NetworkProfileWarning::new(
            NetworkProfileWarningCode::RevisionConflict,
            "网络资料已在别处更新；请重新加载后再预览",
        ));
    }

    let projections =
        network_profile_projections(record, candidate_document, record.signed_profile.clone());
    Ok(NetworkProfilePreview {
        network_name: network_name.into(),
        expected_revision,
        current_revision: record.revision,
        candidate_revision,
        conflict,
        can_apply: !conflict,
        projections,
        diff,
        warnings,
    })
}

pub fn network_profile_projections(
    record: &NetworkRecord,
    candidate: NetworkPublicProfileDocument,
    currently_published: Option<NetworkPublicProfileRecord>,
) -> NetworkProfileProjections {
    let public_profile = candidate.clone();
    let candidate_revision = candidate.revision;
    let fingerprint = blake3::hash(record.public_key.as_str().as_bytes())
        .to_hex()
        .to_string();
    NetworkProfileProjections {
        public_card: NetworkPublicProfileProjection {
            candidate,
            currently_published,
            withheld_categories: vec![
                "member_identity".into(),
                "network_addresses".into(),
                "internal_topology".into(),
                "activity_history".into(),
                "secret_material".into(),
                "invitation_tickets".into(),
            ],
        },
        member_view: NetworkMemberProfileProjection {
            profile: public_profile.clone(),
            member_count: record.members.len(),
            administrator_count: record.administrators.len(),
        },
        administrator_view: NetworkAdminProfileProjection {
            profile: public_profile,
            member_count: record.members.len(),
            administrator_count: record.administrators.len(),
            revision: candidate_revision,
            policy_revision: record.policy_revision,
        },
        local_secret: NetworkLocalSecretProjection {
            identity_present: true,
            public_key_fingerprint: fingerprint,
            storage: "network_identity_file".into(),
            secret_material_included: false,
        },
    }
}

/// Signed discovery data. Endpoints may change between revisions; the stable
/// network ID remains bound to the immutable network public key.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkDiscoveryDocument {
    pub schema_version: u16,
    pub network_id: NetworkId,
    pub public_key: PublicKey,
    /// Node that published this revision. Schema v1 binds the publisher to the
    /// network owner so another member cannot publish an owner-authorized card.
    pub publisher: NodeId,
    pub name: String,
    pub purpose: String,
    /// Public discovery facets. Schema v2 includes these fields in the signed
    /// payload. Defaults keep older local JSON readable while validation still
    /// rejects records created under a different document schema.
    #[serde(default = "default_network_language")]
    pub language: String,
    #[serde(default = "default_network_region")]
    pub region: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub visibility: NetworkVisibility,
    pub join_mode: NetworkJoinMode,
    pub owner: NodeId,
    pub revision: u64,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    /// Canonical capability declaration summary. This is discoverability
    /// metadata, not proof that a capability is currently operational.
    pub capability_summary: Vec<CapabilityKind>,
    #[serde(default)]
    pub endpoints: Vec<NetworkEndpoint>,
    /// A signed tombstone is a normal revision with this timestamp set. Once a
    /// client accepts one, every older or non-identical successor is rejected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at_unix_ms: Option<u64>,
}

impl NetworkDiscoveryDocument {
    pub fn validate_at(&self, now_unix_ms: u64) -> DomainResult<()> {
        if self.schema_version != NETWORK_DISCOVERY_SCHEMA_VERSION {
            return Err(DomainError::new(
                "domain.discovery_unsupported_version",
                "network discovery schema version is unsupported",
            ));
        }
        let expected = NetworkId::from_public_key(self.public_key.as_str().as_bytes());
        if self.network_id != expected {
            return Err(DomainError::invariant(
                "discovery network id does not match its public key",
            ));
        }
        if self.publisher != self.owner {
            return Err(DomainError::new(
                "domain.discovery_publisher_mismatch",
                "network discovery publisher does not match the network owner",
            ));
        }
        if self.name.is_empty() || self.name.len() > 80 {
            return Err(DomainError::invalid_value(
                "discovery network name is invalid",
            ));
        }
        if self.purpose.is_empty() || self.purpose.len() > 512 {
            return Err(DomainError::invalid_value("discovery purpose is invalid"));
        }
        validate_network_language(&self.language)?;
        validate_network_region(&self.region)?;
        if canonical_tags(&self.tags)? != self.tags {
            return Err(DomainError::invalid_value(
                "discovery tags must use canonical order",
            ));
        }
        if self.revision == 0 {
            return Err(DomainError::invalid_value(
                "discovery revision must be positive",
            ));
        }
        if self.issued_at_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::invariant(
                "discovery validity interval is empty",
            ));
        }
        validate_capability_declarations(&self.capability_summary)?;
        let mut canonical_capabilities = self.capability_summary.clone();
        canonical_capabilities.sort();
        if canonical_capabilities != self.capability_summary {
            return Err(DomainError::invalid_value(
                "discovery capability summary must use canonical order",
            ));
        }
        if let Some(revoked_at_unix_ms) = self.revoked_at_unix_ms {
            if revoked_at_unix_ms < self.issued_at_unix_ms
                || revoked_at_unix_ms > now_unix_ms
                || revoked_at_unix_ms >= self.expires_at_unix_ms
            {
                return Err(DomainError::invalid_value(
                    "discovery revocation timestamp is invalid",
                ));
            }
        }
        if now_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::new(
                "domain.discovery_expired",
                "network discovery record has expired",
            ));
        }
        if self.endpoints.len() > 32 {
            return Err(DomainError::invariant(
                "discovery record has too many endpoints",
            ));
        }
        for endpoint in &self.endpoints {
            endpoint.validate()?;
        }
        Ok(())
    }

    /// Build a discovery document from the exact signed public profile that is
    /// persisted for an owner-created network. Discovery revision is managed by
    /// the index and may advance independently of the profile revision.
    pub fn from_record(
        record: &NetworkRecord,
        revision: u64,
        issued_at_unix_ms: u64,
        expires_at_unix_ms: u64,
        revoked_at_unix_ms: Option<u64>,
    ) -> DomainResult<Self> {
        record.validate()?;
        let signed_profile = record.signed_profile.as_ref().ok_or_else(|| {
            DomainError::new(
                "domain.discovery_profile_unsigned",
                "network discovery requires a signed public profile",
            )
        })?;
        signed_profile.validate_against_record(record)?;
        if signed_profile.document.is_expired_at(issued_at_unix_ms) {
            return Err(DomainError::new(
                "domain.discovery_profile_expired",
                "network public profile has expired",
            ));
        }
        let document = Self {
            schema_version: NETWORK_DISCOVERY_SCHEMA_VERSION,
            network_id: record.network_id.clone(),
            public_key: record.public_key.clone(),
            publisher: record.owner.clone(),
            name: record.name.clone(),
            purpose: record.purpose.clone(),
            language: record.language.clone(),
            region: record.region.clone(),
            tags: record.tags.clone(),
            visibility: record.visibility,
            join_mode: record.join_mode,
            owner: record.owner.clone(),
            revision,
            issued_at_unix_ms,
            expires_at_unix_ms,
            capability_summary: record.capability_declarations.clone(),
            endpoints: Vec::new(),
            revoked_at_unix_ms,
        };
        document.validate_at(issued_at_unix_ms)?;
        Ok(document)
    }

    pub fn signing_payload(&self) -> DomainResult<Vec<u8>> {
        serde_json::to_vec(self)
            .map_err(|_| DomainError::invalid_value("discovery payload cannot be encoded"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkDiscoveryRecord {
    pub document: NetworkDiscoveryDocument,
    pub signature: Signature,
}

impl NetworkDiscoveryRecord {
    pub fn digest(&self) -> DomainResult<String> {
        Ok(blake3::hash(&self.document.signing_payload()?)
            .to_hex()
            .to_string())
    }

    pub fn sign(document: NetworkDiscoveryDocument, identity: &Identity) -> DomainResult<Self> {
        let public_key = PublicKey::from_bytes(identity.public_key_bytes());
        if public_key != document.public_key {
            return Err(DomainError::invariant(
                "network discovery signing identity does not match the network public key",
            ));
        }
        let signature = identity.sign(&document.signing_payload()?);
        Ok(Self {
            document,
            signature: Signature::from_bytes(signature.to_bytes()),
        })
    }

    pub fn validate_at(&self, now_unix_ms: u64) -> DomainResult<()> {
        self.document.validate_at(now_unix_ms)?;
        let key = VerifyingKey::from_bytes(&self.document.public_key.to_bytes()?)
            .map_err(|_| DomainError::invalid_value("network public key is invalid"))?;
        let signature = DalekSignature::from_bytes(&self.signature.to_bytes()?);
        key.verify(&self.document.signing_payload()?, &signature)
            .map_err(|_| {
                DomainError::new(
                    "domain.invalid_signature",
                    "network discovery signature is invalid",
                )
            })
    }

    /// Accept only a signed, unexpired, strictly newer record. Equal revisions
    /// with different bytes are treated as an equivocation attempt.
    pub fn validate_successor(
        &self,
        current_revision: u64,
        current_digest: Option<&str>,
        current_revoked: bool,
        now_unix_ms: u64,
    ) -> DomainResult<()> {
        self.validate_at(now_unix_ms)?;
        let digest = self.digest()?;
        if current_revoked {
            if self.document.revoked_at_unix_ms.is_some()
                && self.document.revision == current_revision
                && current_digest == Some(digest.as_str())
            {
                return Ok(());
            }
            return Err(DomainError::new(
                "domain.discovery_already_revoked",
                "network discovery record has already been revoked",
            ));
        }
        if self.document.revision < current_revision {
            return Err(DomainError::new(
                "domain.discovery_rollback",
                "network discovery revision moved backwards",
            ));
        }
        if self.document.revision == current_revision && current_digest != Some(digest.as_str()) {
            return Err(DomainError::new(
                "domain.discovery_conflict",
                "network discovery revision conflicts with the accepted record",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signer, SigningKey};

    use super::*;

    fn signed_record(revision: u64, now: u64, signing_key: &SigningKey) -> NetworkDiscoveryRecord {
        let public_key = PublicKey::from_bytes(signing_key.verifying_key().to_bytes());
        let owner = NodeId::from_public_key(b"owner");
        let document = NetworkDiscoveryDocument {
            schema_version: NETWORK_DISCOVERY_SCHEMA_VERSION,
            network_id: NetworkId::from_public_key(public_key.as_str().as_bytes()),
            public_key,
            publisher: owner.clone(),
            name: "home".into(),
            purpose: "private mesh".into(),
            language: "en".into(),
            region: "US".into(),
            tags: vec!["home".into(), "privacy".into()],
            visibility: NetworkVisibility::Unlisted,
            join_mode: NetworkJoinMode::ApprovalRequired,
            owner,
            revision,
            issued_at_unix_ms: now.saturating_sub(1),
            expires_at_unix_ms: now + 60_000,
            capability_summary: vec![CapabilityKind::Relay],
            endpoints: vec![NetworkEndpoint {
                transport: DiscoveryTransport::Quic,
                host: "relay.example".into(),
                port: 443,
            }],
            revoked_at_unix_ms: None,
        };
        let signature = signing_key.sign(&document.signing_payload().unwrap());
        NetworkDiscoveryRecord {
            document,
            signature: Signature::from_bytes(signature.to_bytes()),
        }
    }

    fn signed_network_record(identity: &Identity, issued_at_unix_ms: u64) -> NetworkRecord {
        let public_key = PublicKey::from_bytes(identity.public_key_bytes());
        let owner = NodeId::from_public_key(b"owner");
        let mut network =
            NetworkRecord::create_owned(create_profile(), public_key, owner, issued_at_unix_ms)
                .unwrap();
        network
            .sign_public_profile(
                identity,
                issued_at_unix_ms,
                issued_at_unix_ms + NETWORK_PUBLIC_PROFILE_VALIDITY_MS,
            )
            .unwrap();
        network
    }

    #[test]
    fn discovery_from_signed_profile_is_verifiable_and_bound_to_network() {
        let identity = Identity::generate().unwrap();
        let network = signed_network_record(&identity, 100);
        let document = NetworkDiscoveryDocument::from_record(&network, 3, 200, 5_000, None)
            .expect("signed profile should produce discovery metadata");
        assert_eq!(document.network_id, network.network_id);
        assert_eq!(document.public_key, network.public_key);
        assert_eq!(document.publisher, network.owner);
        assert_eq!(document.language, network.language);
        assert_eq!(document.region, network.region);
        assert_eq!(document.tags, network.tags);
        assert_eq!(document.owner, network.owner);
        assert_eq!(document.name, network.name);
        assert_eq!(document.capability_summary, network.capability_declarations);
        assert!(document.endpoints.is_empty());

        let discovery = NetworkDiscoveryRecord::sign(document, &identity).unwrap();
        discovery.validate_at(500).unwrap();
        assert_eq!(discovery.document.revision, 3);
    }

    #[test]
    fn discovery_from_record_rejects_unsigned_expired_and_mismatched_profiles() {
        let identity = Identity::generate().unwrap();
        let public_key = PublicKey::from_bytes(identity.public_key_bytes());
        let owner = NodeId::from_public_key(b"owner");
        let unsigned =
            NetworkRecord::create_owned(create_profile(), public_key, owner, 100).unwrap();
        assert_eq!(
            NetworkDiscoveryDocument::from_record(&unsigned, 1, 100, 5_000, None)
                .unwrap_err()
                .code(),
            "domain.discovery_profile_unsigned"
        );

        let mut expired = signed_network_record(&identity, 100);
        expired.sign_public_profile(&identity, 100, 200).unwrap();
        assert_eq!(
            NetworkDiscoveryDocument::from_record(&expired, 1, 200, 5_000, None)
                .unwrap_err()
                .code(),
            "domain.discovery_profile_expired"
        );

        let mut mismatched = signed_network_record(&identity, 100);
        mismatched.name = "different-network".into();
        assert!(NetworkDiscoveryDocument::from_record(&mismatched, 1, 200, 5_000, None).is_err());
    }

    #[test]
    fn network_identity_is_independent_from_endpoints_and_coordinator() {
        let key = SigningKey::from_bytes(&[4; 32]);
        let first = signed_record(1, 100, &key);
        let mut second = first.clone();
        second.document.endpoints[0].host = "new-coordinator.example".into();
        assert_eq!(first.document.network_id, second.document.network_id);
        first.document.validate_at(100).unwrap();
    }

    #[test]
    fn discovery_rejects_impersonation_and_bad_signatures() {
        let key = SigningKey::from_bytes(&[5; 32]);
        let mut record = signed_record(2, 100, &key);
        record.document.name = "tampered".into();
        assert!(record.validate_at(100).is_err());

        let other = SigningKey::from_bytes(&[6; 32]);
        let mut record = signed_record(2, 100, &key);
        record.document.public_key = PublicKey::from_bytes(other.verifying_key().to_bytes());
        record.document.network_id =
            NetworkId::from_public_key(record.document.public_key.as_str().as_bytes());
        assert!(record.validate_at(100).is_err());
    }

    #[test]
    fn discovery_rejects_rollback_conflict_and_expiry() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let record = signed_record(2, 100, &key);
        assert_eq!(
            record
                .validate_successor(3, None, false, 100)
                .unwrap_err()
                .code(),
            "domain.discovery_rollback"
        );
        assert!(record
            .validate_successor(2, Some("different"), false, 100)
            .is_err());
        let digest = blake3::hash(&record.document.signing_payload().unwrap())
            .to_hex()
            .to_string();
        record
            .validate_successor(2, Some(&digest), false, 100)
            .unwrap();
        assert_eq!(
            record.validate_at(60_100).unwrap_err().code(),
            "domain.discovery_expired"
        );
    }

    #[test]
    fn discovery_rejects_unsupported_schema_publisher_and_noncanonical_capabilities() {
        let key = SigningKey::from_bytes(&[9; 32]);
        let mut record = signed_record(1, 100, &key);
        record.document.schema_version = NETWORK_DISCOVERY_SCHEMA_VERSION + 1;
        assert_eq!(
            record.validate_at(100).unwrap_err().code(),
            "domain.discovery_unsupported_version"
        );

        let mut record = signed_record(1, 100, &key);
        record.document.publisher = NodeId::from_public_key(b"attacker");
        assert_eq!(
            record.validate_at(100).unwrap_err().code(),
            "domain.discovery_publisher_mismatch"
        );

        let mut record = signed_record(1, 100, &key);
        record.document.capability_summary =
            vec![CapabilityKind::Relay, CapabilityKind::Coordinator];
        assert!(record.validate_at(100).is_err());
        record.document.capability_summary = vec![CapabilityKind::Relay, CapabilityKind::Relay];
        assert!(record.validate_at(100).is_err());

        let mut record = signed_record(1, 100, &key);
        record.document.language = "not a language".into();
        assert!(record.validate_at(100).is_err());

        let mut record = signed_record(1, 100, &key);
        record.document.tags = vec!["privacy".into(), "home".into()];
        assert!(record.validate_at(100).is_err());
    }

    #[test]
    fn tombstone_is_terminal_and_identical_retry_is_idempotent() {
        let key = SigningKey::from_bytes(&[10; 32]);
        let mut tombstone = signed_record(3, 100, &key);
        tombstone.document.revoked_at_unix_ms = Some(100);
        let signature = key.sign(&tombstone.document.signing_payload().unwrap());
        tombstone.signature = Signature::from_bytes(signature.to_bytes());
        let digest = tombstone.digest().unwrap();
        tombstone
            .validate_successor(3, Some(&digest), true, 100)
            .unwrap();

        let replay = signed_record(4, 100, &key);
        assert_eq!(
            replay
                .validate_successor(3, Some(&digest), true, 100)
                .unwrap_err()
                .code(),
            "domain.discovery_already_revoked"
        );
    }

    #[test]
    fn network_record_checks_owner_admin_and_member_sets() {
        let public_key = PublicKey::from_bytes([8; 32]);
        let owner = NodeId::from_public_key(b"owner");
        let admin = NodeId::from_public_key(b"admin");
        let mut network = NetworkRecord {
            network_id: NetworkId::from_public_key(public_key.as_str().as_bytes()),
            public_key,
            name: "mesh".into(),
            purpose: "test".into(),
            icon: "network".into(),
            language: "en".into(),
            region: "US".into(),
            tags: vec!["home".into()],
            capability_declarations: vec![CapabilityKind::Relay],
            visibility: NetworkVisibility::Private,
            join_mode: NetworkJoinMode::InviteOnly,
            owner: owner.clone(),
            administrators: vec![admin.clone()],
            members: vec![owner, admin],
            revision: 1,
            policy_revision: 1,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
            signed_profile: None,
        };
        network.validate().unwrap();
        network.members.pop();
        assert!(network.validate().is_err());
    }

    fn create_profile() -> NetworkCreateProfile {
        NetworkCreateProfile {
            name: "home-mesh".into(),
            purpose: "Family devices across two homes".into(),
            icon: "home".into(),
            language: "zh-CN".into(),
            region: "CN".into(),
            tags: vec!["Family".into(), "home".into()],
            capability_declarations: vec![CapabilityKind::Relay, CapabilityKind::Coordinator],
            visibility: NetworkVisibility::Unlisted,
            join_mode: NetworkJoinMode::InviteOnly,
        }
    }

    #[test]
    fn owner_creation_sets_governance_and_derives_stable_id() {
        let public_key = PublicKey::from_bytes([9; 32]);
        let owner = NodeId::from_public_key(b"owner");
        let network = NetworkRecord::create_owned(create_profile(), public_key, owner.clone(), 100)
            .expect("valid owner-created network");

        assert_eq!(
            network.network_id,
            NetworkId::from_public_key(network.public_key.as_str().as_bytes())
        );
        assert_eq!(network.owner, owner);
        assert_eq!(network.members, vec![owner]);
        assert!(network.administrators.is_empty());
        assert_eq!(network.revision, 1);
        assert_eq!(network.policy_revision, 1);
        assert_eq!(network.icon, "home");
        assert_eq!(network.language, "zh-CN");
        assert_eq!(network.region, "CN");
        assert_eq!(network.tags, vec!["family", "home"]);
        assert_eq!(
            network.capability_declarations,
            vec![CapabilityKind::Coordinator, CapabilityKind::Relay]
        );
    }

    #[test]
    fn rename_and_json_reload_preserve_network_id() {
        let public_key = PublicKey::from_bytes([10; 32]);
        let owner = NodeId::from_public_key(b"owner");
        let mut network =
            NetworkRecord::create_owned(create_profile(), public_key, owner, 100).unwrap();
        let network_id = network.network_id.clone();

        network.rename("renamed-mesh", 200).unwrap();
        assert_eq!(network.network_id, network_id);
        assert_eq!(network.revision, 2);

        let encoded = serde_json::to_string(&network).unwrap();
        let reloaded: NetworkRecord = serde_json::from_str(&encoded).unwrap();
        assert_eq!(reloaded.network_id, network_id);
        assert_eq!(reloaded.name, "renamed-mesh");
        reloaded.validate().unwrap();
    }

    #[test]
    fn create_profile_rejects_unsafe_or_malformed_public_fields() {
        let invalid_name = NetworkCreateProfile {
            name: "home mesh".into(),
            ..create_profile()
        };
        assert!(invalid_name.validate().is_err());

        let empty_purpose = NetworkCreateProfile {
            purpose: "   ".into(),
            ..create_profile()
        };
        assert!(empty_purpose.validate().is_err());

        let invalid_icon = NetworkCreateProfile {
            icon: "Home Icon".into(),
            ..create_profile()
        };
        assert!(invalid_icon.validate().is_err());

        let invalid_language = NetworkCreateProfile {
            language: "zh_CN".into(),
            ..create_profile()
        };
        assert!(invalid_language.validate().is_err());
    }

    #[test]
    fn signed_public_profile_is_exact_and_private_fields_are_absent() {
        let identity = Identity::generate().unwrap();
        let public_key = PublicKey::from_bytes(identity.public_key_bytes());
        let owner = NodeId::from_public_key(b"owner");
        let mut network =
            NetworkRecord::create_owned(create_profile(), public_key, owner, 100).unwrap();
        network
            .sign_public_profile(&identity, 100, 100 + NETWORK_PUBLIC_PROFILE_VALIDITY_MS)
            .unwrap();
        let profile = network.signed_profile.as_ref().unwrap();
        profile.validate().unwrap();
        let value = serde_json::to_value(&profile.document).unwrap();
        let object = value.as_object().unwrap();
        let allowed = BTreeSet::from([
            "schema_version",
            "network_id",
            "public_key",
            "owner",
            "name",
            "purpose",
            "icon",
            "language",
            "region",
            "tags",
            "capability_declarations",
            "visibility",
            "join_mode",
            "revision",
            "issued_at_unix_ms",
            "expires_at_unix_ms",
        ]);
        assert_eq!(
            object.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            allowed
        );
        for forbidden in [
            "members",
            "administrators",
            "assigned_ipv4",
            "assigned_ipv6",
            "p2p_listen_addr",
            "endpoints",
            "tunnel",
            "topology",
            "activity",
            "audit",
            "private_key",
            "signing_key",
            "identity",
            "ticket",
        ] {
            assert!(
                !object.contains_key(forbidden),
                "public profile leaked {forbidden}"
            );
            assert!(!value.to_string().contains(forbidden));
        }
    }

    #[test]
    fn signed_public_profile_rejects_tampering_schema_and_revision_mismatch() {
        let identity = Identity::generate().unwrap();
        let public_key = PublicKey::from_bytes(identity.public_key_bytes());
        let owner = NodeId::from_public_key(b"owner");
        let mut network =
            NetworkRecord::create_owned(create_profile(), public_key, owner, 100).unwrap();
        network.sign_public_profile(&identity, 100, 200).unwrap();

        let mut tampered = network.clone();
        tampered.signed_profile.as_mut().unwrap().document.purpose = "tampered".into();
        assert_eq!(
            tampered.validate().unwrap_err().code(),
            "domain.invalid_signature"
        );

        let mut unsupported = network.clone();
        unsupported
            .signed_profile
            .as_mut()
            .unwrap()
            .document
            .schema_version = 99;
        assert!(unsupported.validate().is_err());

        let mut revision = network;
        revision.signed_profile.as_mut().unwrap().document.revision = 2;
        assert!(revision.validate().is_err());
    }

    #[test]
    fn profile_preview_is_four_layered_and_reports_expiry_conflict_and_diff() {
        let identity = Identity::generate().unwrap();
        let public_key = PublicKey::from_bytes(identity.public_key_bytes());
        let owner = NodeId::from_public_key(b"owner");
        let mut network =
            NetworkRecord::create_owned(create_profile(), public_key, owner, 100).unwrap();
        network.sign_public_profile(&identity, 100, 200).unwrap();
        let update = NetworkProfileUpdate {
            purpose: "Updated purpose".into(),
            region: "JP".into(),
            tags: vec!["Tokyo".into(), "asia".into()],
            capability_declarations: vec![CapabilityKind::Exit],
            ..NetworkProfileUpdate::from_record(&network)
        };

        let preview = preview_network_profile("home-mesh", &network, 1, &update, 300).unwrap();
        assert_eq!(preview.current_revision, 1);
        assert_eq!(preview.candidate_revision, 2);
        assert!(!preview.conflict);
        assert_eq!(preview.projections.public_card.candidate.region, "JP");
        assert_eq!(preview.projections.member_view.member_count, 1);
        assert_eq!(
            preview.projections.administrator_view.revision,
            preview.candidate_revision
        );
        assert_eq!(preview.projections.administrator_view.policy_revision, 1);
        assert!(!preview.projections.local_secret.secret_material_included);
        assert!(preview.diff.iter().any(|entry| entry.path == "purpose"));
        assert!(preview
            .warnings
            .iter()
            .any(|warning| warning.code == NetworkProfileWarningCode::ProfileExpired));

        let conflict = preview_network_profile("home-mesh", &network, 0, &update, 300).unwrap();
        assert!(conflict.conflict);
        assert!(!conflict.can_apply);
        assert!(conflict
            .warnings
            .iter()
            .any(|warning| warning.code == NetworkProfileWarningCode::RevisionConflict));
    }

    #[test]
    fn profile_update_applies_atomically_in_memory_and_requires_resigning() {
        let identity = Identity::generate().unwrap();
        let public_key = PublicKey::from_bytes(identity.public_key_bytes());
        let owner = NodeId::from_public_key(b"owner");
        let mut network =
            NetworkRecord::create_owned(create_profile(), public_key, owner, 100).unwrap();
        network.sign_public_profile(&identity, 100, 1_000).unwrap();
        let update = NetworkProfileUpdate {
            purpose: "Changed".into(),
            ..NetworkProfileUpdate::from_record(&network)
        };
        let diff = network.apply_profile_update(&update, 1, 200).unwrap();
        assert_eq!(diff.len(), 1);
        assert_eq!(network.revision, 2);
        assert!(network.signed_profile.is_none());
        network.sign_public_profile(&identity, 200, 300).unwrap();
        network.validate().unwrap();
        assert!(network.apply_profile_update(&update, 1, 300).is_err());
    }

    #[test]
    fn legacy_network_record_uses_default_public_profile_fields() {
        let value = serde_json::json!({
            "network_id": NetworkId::from_public_key(PublicKey::from_bytes([11; 32]).as_str().as_bytes()),
            "public_key": PublicKey::from_bytes([11; 32]),
            "name": "legacy",
            "purpose": "Legacy network",
            "visibility": "private",
            "join_mode": "invite_only",
            "owner": NodeId::from_public_key(b"owner"),
            "administrators": [],
            "members": [NodeId::from_public_key(b"owner")],
            "revision": 1,
            "policy_revision": 1,
            "created_at_unix_ms": 1,
            "updated_at_unix_ms": 1
        });
        let record: NetworkRecord = serde_json::from_value(value).unwrap();
        assert_eq!(record.icon, DEFAULT_NETWORK_ICON);
        assert_eq!(record.language, DEFAULT_NETWORK_LANGUAGE);
        assert_eq!(record.region, DEFAULT_NETWORK_REGION);
        assert!(record.tags.is_empty());
        assert!(record.capability_declarations.is_empty());
        assert!(record.signed_profile.is_none());
        record.validate().unwrap();
    }
}
