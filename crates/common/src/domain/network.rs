use std::collections::BTreeSet;

use crate::crypto::Identity;

use ed25519_dalek::{Signature as DalekSignature, Verifier, VerifyingKey};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::capability::CapabilityKind;
use super::exit_selection::ExitSelectionPolicy;
use super::ids::{ContentHash, NetworkId, NodeId, PublicKey, Signature};
use super::membership::{
    legacy_exit_contribution_scope, validate_exit_contribution_policy, ExitContributionMode,
    ExitContributionObligation, ExitContributionScope, JoinRequestGrantPreview, MembershipRole,
    MembershipTerms,
};
use super::{DomainError, DomainResult};

/// Maximum age of a direct join disclosure. The user-visible acceptance time
/// must fall inside this window and the Coordinator revalidates the authority
/// record before any mutation.
pub const NETWORK_JOIN_PREVIEW_VALIDITY_MS: u64 = 5 * 60 * 1000;

/// Tolerated clock skew when a client validates the preview issue time.
pub const NETWORK_JOIN_PREVIEW_CLOCK_SKEW_MS: u64 = 60 * 1000;

const NETWORK_JOIN_PREVIEW_SIGNING_DOMAIN: &[u8] = b"fuckipv6/network-join-preview/v1\0";

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
    Websocket,
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
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize)]
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
    /// Offline recovery trustees. These are independent public keys escrowed
    /// before a disaster, never member identities and never exposed publicly.
    #[serde(default)]
    pub recovery_authorities: Vec<PublicKey>,
    /// Number of distinct offline trustee approvals required. Zero means no
    /// offline policy is configured and owner/administrator strict majority is
    /// required instead.
    #[serde(default)]
    pub recovery_threshold: u32,
    #[serde(default)]
    pub members: Vec<NodeId>,
    pub revision: u64,
    pub policy_revision: u64,
    /// Revision of the privacy notice shown before joining. Defaults preserve
    /// readable legacy owner records while new records can advance it.
    #[serde(default = "default_policy_revision")]
    pub privacy_policy_revision: u64,
    /// Optional hard cap on active members. `None` means no configured cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_limit: Option<u32>,
    /// Admission obligation disclosed before joining. This is a policy claim,
    /// never proof that this node has enabled its local Exit service.
    #[serde(default)]
    pub exit_contribution: ExitContributionObligation,
    /// Population covered by the admission obligation. Kept separate from the
    /// obligation itself so capability declarations can never imply consent.
    #[serde(default)]
    pub exit_contribution_scope: ExitContributionScope,
    /// Explicit four-mode Exit contribution policy.
    #[serde(default)]
    pub exit_contribution_mode: ExitContributionMode,
    /// Administrator policy deciding which Exits members may select. This is
    /// independent from whether a member contributes an Exit.
    #[serde(default)]
    pub exit_selection_policy: ExitSelectionPolicy,
    /// Plain-language privacy-policy changes disclosed on the join preview.
    #[serde(default)]
    pub privacy_changes: Vec<String>,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    /// Signed public profile. Legacy owner records may omit this field; UI
    /// previews must surface an `UnsignedLegacy` warning instead of treating
    /// the mutable record itself as a verified public document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_profile: Option<NetworkPublicProfileRecord>,
}

impl<'de> Deserialize<'de> for NetworkRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawNetworkRecord {
            network_id: NetworkId,
            public_key: PublicKey,
            name: String,
            purpose: String,
            #[serde(default = "default_network_icon")]
            icon: String,
            #[serde(default = "default_network_language")]
            language: String,
            #[serde(default = "default_network_region")]
            region: String,
            #[serde(default)]
            tags: Vec<String>,
            #[serde(default)]
            capability_declarations: Vec<CapabilityKind>,
            visibility: NetworkVisibility,
            join_mode: NetworkJoinMode,
            owner: NodeId,
            #[serde(default)]
            administrators: Vec<NodeId>,
            #[serde(default)]
            recovery_authorities: Vec<PublicKey>,
            #[serde(default)]
            recovery_threshold: u32,
            #[serde(default)]
            members: Vec<NodeId>,
            revision: u64,
            policy_revision: u64,
            #[serde(default = "default_policy_revision")]
            privacy_policy_revision: u64,
            #[serde(default)]
            member_limit: Option<u32>,
            #[serde(default)]
            exit_contribution: ExitContributionObligation,
            #[serde(default)]
            exit_contribution_scope: Option<ExitContributionScope>,
            #[serde(default)]
            exit_contribution_mode: Option<ExitContributionMode>,
            #[serde(default)]
            exit_selection_policy: ExitSelectionPolicy,
            #[serde(default)]
            privacy_changes: Vec<String>,
            created_at_unix_ms: u64,
            updated_at_unix_ms: u64,
            #[serde(default)]
            signed_profile: Option<NetworkPublicProfileRecord>,
        }

        let raw = RawNetworkRecord::deserialize(deserializer)?;
        Ok(Self {
            network_id: raw.network_id,
            public_key: raw.public_key,
            name: raw.name,
            purpose: raw.purpose,
            icon: raw.icon,
            language: raw.language,
            region: raw.region,
            tags: raw.tags,
            capability_declarations: raw.capability_declarations,
            visibility: raw.visibility,
            join_mode: raw.join_mode,
            owner: raw.owner,
            administrators: raw.administrators,
            recovery_authorities: raw.recovery_authorities,
            recovery_threshold: raw.recovery_threshold,
            members: raw.members,
            revision: raw.revision,
            policy_revision: raw.policy_revision,
            privacy_policy_revision: raw.privacy_policy_revision,
            member_limit: raw.member_limit,
            exit_contribution: raw.exit_contribution,
            exit_contribution_scope: raw
                .exit_contribution_scope
                .unwrap_or_else(|| legacy_exit_contribution_scope(raw.exit_contribution)),
            exit_contribution_mode: raw
                .exit_contribution_mode
                .unwrap_or_else(|| ExitContributionMode::from_legacy(raw.exit_contribution)),
            exit_selection_policy: raw.exit_selection_policy,
            privacy_changes: raw.privacy_changes,
            created_at_unix_ms: raw.created_at_unix_ms,
            updated_at_unix_ms: raw.updated_at_unix_ms,
            signed_profile: raw.signed_profile,
        })
    }
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
        if self.revision == 0 || self.policy_revision == 0 || self.privacy_policy_revision == 0 {
            return Err(DomainError::invalid_value(
                "network, policy and privacy revisions must be positive",
            ));
        }
        if self.member_limit == Some(0) {
            return Err(DomainError::invalid_value(
                "network member limit must be positive when set",
            ));
        }
        if self.privacy_changes.len() > 32
            || self
                .privacy_changes
                .iter()
                .any(|change| change.is_empty() || change.chars().count() > 256)
        {
            return Err(DomainError::invalid_value(
                "network privacy changes are invalid",
            ));
        }
        validate_exit_contribution_policy(
            self.exit_contribution,
            self.exit_contribution_scope,
            self.exit_contribution_mode,
        )?;
        self.exit_selection_policy.validate()?;
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
        let mut recovery_authorities = BTreeSet::new();
        for authority in &self.recovery_authorities {
            if !recovery_authorities.insert(authority) {
                return Err(DomainError::invariant(
                    "duplicate network recovery authority",
                ));
            }
        }
        if (recovery_authorities.is_empty() && self.recovery_threshold != 0)
            || (!recovery_authorities.is_empty()
                && (self.recovery_threshold == 0
                    || self.recovery_threshold as usize > recovery_authorities.len()))
        {
            return Err(DomainError::invariant(
                "network recovery threshold must fit its configured authorities",
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
        if self.exit_contribution != update.exit_contribution
            || self.exit_contribution_scope != update.exit_contribution_scope
            || self.exit_contribution_mode != update.exit_contribution_mode
            || update
                .exit_selection_policy
                .as_ref()
                .is_some_and(|policy| policy != &self.exit_selection_policy)
        {
            self.policy_revision = self.policy_revision.saturating_add(1);
        }
        self.exit_contribution = update.exit_contribution;
        self.exit_contribution_scope = update.exit_contribution_scope;
        self.exit_contribution_mode = update.exit_contribution_mode;
        if let Some(policy) = &update.exit_selection_policy {
            self.exit_selection_policy = policy.clone();
        }
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
            recovery_authorities: Vec::new(),
            recovery_threshold: 0,
            members: vec![owner],
            revision: 1,
            policy_revision: 1,
            privacy_policy_revision: 1,
            member_limit: None,
            exit_contribution: profile.exit_contribution,
            exit_contribution_scope: profile.exit_contribution_scope,
            exit_contribution_mode: profile.exit_contribution_mode,
            exit_selection_policy: profile.exit_selection_policy,
            privacy_changes: Vec::new(),
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

    /// Terms an approval applicant must acknowledge for this exact record.
    /// Keeping this derivation in the domain prevents the Coordinator and both
    /// UIs from inventing separate admission-policy views.
    pub fn membership_terms(&self) -> MembershipTerms {
        MembershipTerms {
            network_policy_revision: self.policy_revision,
            privacy_policy_revision: self.privacy_policy_revision,
            exit_contribution: self.exit_contribution,
            exit_contribution_scope: self.exit_contribution_scope,
            exit_contribution_mode: self.exit_contribution_mode,
            member_limit: self.member_limit,
        }
    }

    /// Authority disclosed before an approval decision. Approval starts from
    /// the ordinary member role; network capability declarations determine the
    /// data-plane capability grant shown to reviewers. Local Exit enablement
    /// remains a separate local-owner decision even when contribution is
    /// required by the network.
    pub fn approval_grant_preview(&self) -> JoinRequestGrantPreview {
        let capabilities = self.capability_declarations.clone();
        JoinRequestGrantPreview {
            role: MembershipRole::Member,
            file_access: capabilities.contains(&CapabilityKind::File),
            capabilities,
            exit_contribution: self.exit_contribution,
            exit_contribution_scope: self.exit_contribution_scope,
            exit_contribution_mode: self.exit_contribution_mode,
            grants_data_plane_access: true,
        }
    }

    /// Add one approved member without changing the signed public profile
    /// revision. Membership IDs are private roster data, so only the roster
    /// mutation clock advances. Duplicate admission is idempotent and a hard
    /// member limit is enforced before mutation.
    pub fn admit_member(&mut self, member: NodeId, now_unix_ms: u64) -> DomainResult<bool> {
        if self.members.contains(&member) {
            return Ok(false);
        }
        if self
            .member_limit
            .is_some_and(|limit| self.members.len() >= limit as usize)
        {
            return Err(DomainError::new(
                "domain.network_member_limit",
                "network member limit has been reached",
            ));
        }
        self.members.push(member);
        self.updated_at_unix_ms = self.updated_at_unix_ms.max(now_unix_ms);
        self.validate()?;
        Ok(true)
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

/// Wire-stable membership terms for direct join disclosure. Unlike the legacy
/// `MembershipTerms` JSON adapter, every field has exactly one binary encoding.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkJoinTerms {
    pub network_policy_revision: u64,
    pub privacy_policy_revision: u64,
    pub exit_contribution: ExitContributionObligation,
    pub exit_contribution_scope: ExitContributionScope,
    pub exit_contribution_mode: ExitContributionMode,
    #[serde(default)]
    pub member_limit: Option<u32>,
}

impl NetworkJoinTerms {
    pub fn validate(&self) -> DomainResult<()> {
        self.to_membership_terms().validate()
    }

    pub fn content_hash(&self) -> DomainResult<ContentHash> {
        self.to_membership_terms().content_hash()
    }

    fn to_membership_terms(&self) -> MembershipTerms {
        MembershipTerms {
            network_policy_revision: self.network_policy_revision,
            privacy_policy_revision: self.privacy_policy_revision,
            exit_contribution: self.exit_contribution,
            exit_contribution_scope: self.exit_contribution_scope,
            exit_contribution_mode: self.exit_contribution_mode,
            member_limit: self.member_limit,
        }
    }
}

impl From<MembershipTerms> for NetworkJoinTerms {
    fn from(terms: MembershipTerms) -> Self {
        Self {
            network_policy_revision: terms.network_policy_revision,
            privacy_policy_revision: terms.privacy_policy_revision,
            exit_contribution: terms.exit_contribution,
            exit_contribution_scope: terms.exit_contribution_scope,
            exit_contribution_mode: terms.exit_contribution_mode,
            member_limit: terms.member_limit,
        }
    }
}

/// Short-lived, authenticated disclosure returned by the Coordinator before a
/// direct network join. It deliberately contains no roster, endpoint topology,
/// invitation credential or member activity.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkJoinPreview {
    pub network_id: NetworkId,
    pub public_key: PublicKey,
    pub network_name: String,
    pub owner: NodeId,
    pub revision: u64,
    pub policy_revision: u64,
    pub privacy_policy_revision: u64,
    pub join_mode: NetworkJoinMode,
    pub terms: NetworkJoinTerms,
    pub terms_hash: ContentHash,
    pub preview_digest: ContentHash,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    /// Signature by the Coordinator/owner identity embedded in the join ticket.
    pub signature: Signature,
}

/// The ticket-derived claims a client sends when asking the Coordinator to
/// disclose its trusted network record. The Coordinator compares these claims
/// with the authoritative record; the ticket itself is never sent or echoed.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkJoinPreviewRequest {
    /// Coordinator host public key embedded in the `fk://` ticket.
    pub coordinator_public_key: [u8; 32],
    pub network_name: String,
}

impl NetworkJoinPreview {
    pub fn for_record(
        record: &NetworkRecord,
        coordinator_identity: &Identity,
        issued_at_unix_ms: u64,
        expires_at_unix_ms: u64,
    ) -> DomainResult<Self> {
        let mut preview = Self::unsigned_for_record(record, issued_at_unix_ms, expires_at_unix_ms)?;
        if NodeId::from_public_key(coordinator_identity.public_key().as_slice()) != record.owner {
            return Err(DomainError::invalid_value(
                "join preview signer does not own the trusted network record",
            ));
        }
        preview.signature = Signature::from_bytes(
            coordinator_identity
                .sign(&preview.signing_payload()?)
                .to_bytes(),
        );
        Ok(preview)
    }

    fn unsigned_for_record(
        record: &NetworkRecord,
        issued_at_unix_ms: u64,
        expires_at_unix_ms: u64,
    ) -> DomainResult<Self> {
        record.validate()?;
        if issued_at_unix_ms == 0 || issued_at_unix_ms >= expires_at_unix_ms {
            return Err(DomainError::invalid_value(
                "network join preview validity window is invalid",
            ));
        }
        if expires_at_unix_ms.saturating_sub(issued_at_unix_ms) > NETWORK_JOIN_PREVIEW_VALIDITY_MS {
            return Err(DomainError::invalid_value(
                "network join preview validity window is too long",
            ));
        }
        let terms = NetworkJoinTerms::from(record.membership_terms());
        let mut preview = Self {
            network_id: record.network_id.clone(),
            public_key: record.public_key.clone(),
            network_name: record.name.clone(),
            owner: record.owner.clone(),
            revision: record.revision,
            policy_revision: record.policy_revision,
            privacy_policy_revision: record.privacy_policy_revision,
            join_mode: record.join_mode,
            terms_hash: terms.content_hash()?,
            terms,
            preview_digest: ContentHash::from_bytes(b"network-join-preview-pending"),
            issued_at_unix_ms,
            expires_at_unix_ms,
            signature: Signature::from_bytes([0; 64]),
        };
        preview.preview_digest = preview.compute_digest()?;
        Ok(preview)
    }

    pub fn compute_digest(&self) -> DomainResult<ContentHash> {
        let mut canonical = self.clone();
        canonical.preview_digest =
            ContentHash::from_bytes(b"network-join-preview-digest-placeholder");
        canonical.signature = Signature::from_bytes([0; 64]);
        let bytes = serde_json::to_vec(&canonical)
            .map_err(|_| DomainError::invalid_value("network join preview cannot be encoded"))?;
        Ok(ContentHash::from_bytes(&bytes))
    }

    /// The signature covers the digest, which in turn covers every preview
    /// field except the signature itself. The fixed domain separator prevents
    /// reuse of this signature in another protocol context.
    pub fn signing_payload(&self) -> DomainResult<Vec<u8>> {
        let mut payload = Vec::with_capacity(
            NETWORK_JOIN_PREVIEW_SIGNING_DOMAIN.len() + self.preview_digest.as_str().len(),
        );
        payload.extend_from_slice(NETWORK_JOIN_PREVIEW_SIGNING_DOMAIN);
        payload.extend_from_slice(self.preview_digest.as_str().as_bytes());
        Ok(payload)
    }

    pub fn signature_is_valid(&self, coordinator_public_key: &[u8]) -> bool {
        let Ok(payload) = self.signing_payload() else {
            return false;
        };
        let Ok(signature) = self.signature.to_bytes() else {
            return false;
        };
        Identity::verify(coordinator_public_key, &payload, &signature)
    }

    pub fn validate_freshness(&self, now_unix_ms: u64) -> DomainResult<()> {
        if self.issued_at_unix_ms == 0 || self.issued_at_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::invalid_value(
                "network join preview validity window is invalid",
            ));
        }
        if self
            .expires_at_unix_ms
            .saturating_sub(self.issued_at_unix_ms)
            > NETWORK_JOIN_PREVIEW_VALIDITY_MS
        {
            return Err(DomainError::invalid_value(
                "network join preview validity window is too long",
            ));
        }
        if self.issued_at_unix_ms > now_unix_ms.saturating_add(NETWORK_JOIN_PREVIEW_CLOCK_SKEW_MS) {
            return Err(DomainError::invalid_value(
                "network join preview issue time is in the future",
            ));
        }
        if self.is_expired(now_unix_ms) {
            return Err(DomainError::new(
                "domain.network_join_preview_expired",
                "network join preview has expired",
            ));
        }
        Ok(())
    }

    pub fn is_expired(&self, now_unix_ms: u64) -> bool {
        now_unix_ms >= self.expires_at_unix_ms
    }

    pub fn validate_for_record(
        &self,
        record: &NetworkRecord,
        coordinator_public_key: &[u8],
        now_unix_ms: u64,
    ) -> DomainResult<()> {
        self.validate_freshness(now_unix_ms)?;
        self.terms.validate()?;
        if self.policy_revision != self.terms.network_policy_revision
            || self.privacy_policy_revision != self.terms.privacy_policy_revision
        {
            return Err(DomainError::invalid_value(
                "network join preview policy revisions do not match its terms",
            ));
        }
        if NodeId::from_public_key(coordinator_public_key) != record.owner
            || !self.signature_is_valid(coordinator_public_key)
        {
            return Err(DomainError::new(
                "domain.invalid_signature",
                "network join preview signature is invalid",
            ));
        }
        record.validate()?;
        let expected =
            Self::unsigned_for_record(record, self.issued_at_unix_ms, self.expires_at_unix_ms)?;
        if self.network_id != expected.network_id
            || self.public_key != expected.public_key
            || self.network_name != expected.network_name
            || self.owner != expected.owner
            || self.revision != expected.revision
            || self.policy_revision != expected.policy_revision
            || self.privacy_policy_revision != expected.privacy_policy_revision
            || self.join_mode != expected.join_mode
            || self.terms_hash != expected.terms_hash
            || self.terms != expected.terms
            || self.preview_digest != expected.preview_digest
            || self.issued_at_unix_ms != expected.issued_at_unix_ms
            || self.expires_at_unix_ms != expected.expires_at_unix_ms
        {
            return Err(DomainError::new(
                "domain.network_join_preview_changed",
                "network join preview no longer matches the trusted record",
            ));
        }
        Ok(())
    }
}

/// Owner-supplied public profile used to create a network. Hostnames, ports and
/// runtime settings deliberately do not participate in this contract.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize)]
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
    #[serde(default)]
    pub exit_contribution: ExitContributionObligation,
    #[serde(default)]
    pub exit_contribution_scope: ExitContributionScope,
    #[serde(default)]
    pub exit_contribution_mode: ExitContributionMode,
    #[serde(default)]
    pub exit_selection_policy: ExitSelectionPolicy,
}

impl<'de> Deserialize<'de> for NetworkCreateProfile {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawNetworkCreateProfile {
            name: String,
            purpose: String,
            icon: String,
            language: String,
            #[serde(default = "default_network_region")]
            region: String,
            #[serde(default)]
            tags: Vec<String>,
            #[serde(default)]
            capability_declarations: Vec<CapabilityKind>,
            visibility: NetworkVisibility,
            join_mode: NetworkJoinMode,
            #[serde(default)]
            exit_contribution: ExitContributionObligation,
            #[serde(default)]
            exit_contribution_scope: Option<ExitContributionScope>,
            #[serde(default)]
            exit_contribution_mode: Option<ExitContributionMode>,
            #[serde(default)]
            exit_selection_policy: ExitSelectionPolicy,
        }

        let raw = RawNetworkCreateProfile::deserialize(deserializer)?;
        Ok(Self {
            name: raw.name,
            purpose: raw.purpose,
            icon: raw.icon,
            language: raw.language,
            region: raw.region,
            tags: raw.tags,
            capability_declarations: raw.capability_declarations,
            visibility: raw.visibility,
            join_mode: raw.join_mode,
            exit_contribution: raw.exit_contribution,
            exit_contribution_scope: raw
                .exit_contribution_scope
                .unwrap_or_else(|| legacy_exit_contribution_scope(raw.exit_contribution)),
            exit_contribution_mode: raw
                .exit_contribution_mode
                .unwrap_or_else(|| ExitContributionMode::from_legacy(raw.exit_contribution)),
            exit_selection_policy: raw.exit_selection_policy,
        })
    }
}

impl NetworkCreateProfile {
    pub fn validate(&self) -> DomainResult<()> {
        validate_network_name(&self.name)?;
        validate_network_purpose(&self.purpose)?;
        validate_network_icon(&self.icon)?;
        validate_network_language(&self.language)?;
        validate_network_region(&self.region)?;
        validate_network_tags(&self.tags)?;
        validate_capability_declarations(&self.capability_declarations)?;
        validate_exit_contribution_policy(
            self.exit_contribution,
            self.exit_contribution_scope,
            self.exit_contribution_mode,
        )?;
        self.exit_selection_policy.validate()
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
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize)]
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
    #[serde(default)]
    pub exit_contribution: ExitContributionObligation,
    #[serde(default)]
    pub exit_contribution_scope: ExitContributionScope,
    #[serde(default)]
    pub exit_contribution_mode: ExitContributionMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_selection_policy: Option<ExitSelectionPolicy>,
}

impl<'de> Deserialize<'de> for NetworkProfileUpdate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawNetworkProfileUpdate {
            purpose: String,
            icon: String,
            language: String,
            region: String,
            #[serde(default)]
            tags: Vec<String>,
            #[serde(default)]
            capability_declarations: Vec<CapabilityKind>,
            visibility: NetworkVisibility,
            join_mode: NetworkJoinMode,
            #[serde(default)]
            exit_contribution: ExitContributionObligation,
            #[serde(default)]
            exit_contribution_scope: Option<ExitContributionScope>,
            #[serde(default)]
            exit_contribution_mode: Option<ExitContributionMode>,
            #[serde(default)]
            exit_selection_policy: Option<ExitSelectionPolicy>,
        }

        let raw = RawNetworkProfileUpdate::deserialize(deserializer)?;
        Ok(Self {
            purpose: raw.purpose,
            icon: raw.icon,
            language: raw.language,
            region: raw.region,
            tags: raw.tags,
            capability_declarations: raw.capability_declarations,
            visibility: raw.visibility,
            join_mode: raw.join_mode,
            exit_contribution: raw.exit_contribution,
            exit_contribution_scope: raw
                .exit_contribution_scope
                .unwrap_or_else(|| legacy_exit_contribution_scope(raw.exit_contribution)),
            exit_contribution_mode: raw
                .exit_contribution_mode
                .unwrap_or_else(|| ExitContributionMode::from_legacy(raw.exit_contribution)),
            exit_selection_policy: raw.exit_selection_policy,
        })
    }
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
            exit_contribution: record.exit_contribution,
            exit_contribution_scope: record.exit_contribution_scope,
            exit_contribution_mode: record.exit_contribution_mode,
            exit_selection_policy: Some(record.exit_selection_policy.clone()),
        }
    }

    pub fn validate(&self) -> DomainResult<()> {
        validate_network_purpose(&self.purpose)?;
        validate_network_icon(&self.icon)?;
        validate_network_language(&self.language)?;
        validate_network_region(&self.region)?;
        validate_network_tags(&self.tags)?;
        validate_capability_declarations(&self.capability_declarations)?;
        validate_exit_contribution_policy(
            self.exit_contribution,
            self.exit_contribution_scope,
            self.exit_contribution_mode,
        )?;
        if let Some(policy) = &self.exit_selection_policy {
            policy.validate()?;
        }
        Ok(())
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
pub const NETWORK_DISCOVERY_SCHEMA_VERSION: u16 = 3;
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

fn default_policy_revision() -> u64 {
    1
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
    compare(
        "exit_contribution",
        format!("{:?}", current.exit_contribution).to_ascii_lowercase(),
        format!("{:?}", update.exit_contribution).to_ascii_lowercase(),
    );
    compare(
        "exit_contribution_scope",
        format!("{:?}", current.exit_contribution_scope).to_ascii_lowercase(),
        format!("{:?}", update.exit_contribution_scope).to_ascii_lowercase(),
    );
    compare(
        "exit_contribution_mode",
        format!("{:?}", current.exit_contribution_mode).to_ascii_lowercase(),
        format!("{:?}", update.exit_contribution_mode).to_ascii_lowercase(),
    );
    compare(
        "exit_selection_policy",
        serde_json::to_string(
            current
                .exit_selection_policy
                .as_ref()
                .unwrap_or(&record.exit_selection_policy),
        )
        .unwrap_or_default(),
        serde_json::to_string(
            update
                .exit_selection_policy
                .as_ref()
                .unwrap_or(&record.exit_selection_policy),
        )
        .unwrap_or_default(),
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
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize)]
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
    /// Public governance roles disclosed before joining. Full member IDs,
    /// endpoints beyond the signed public hints, topology and activity remain
    /// outside this document.
    #[serde(default)]
    pub administrators: Vec<NodeId>,
    pub revision: u64,
    pub policy_revision: u64,
    pub privacy_policy_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_limit: Option<u32>,
    #[serde(default)]
    pub member_count: u32,
    #[serde(default)]
    pub exit_contribution: ExitContributionObligation,
    #[serde(default)]
    pub exit_contribution_scope: ExitContributionScope,
    #[serde(default)]
    pub exit_contribution_mode: ExitContributionMode,
    #[serde(default)]
    pub privacy_changes: Vec<String>,
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

impl<'de> Deserialize<'de> for NetworkDiscoveryDocument {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawNetworkDiscoveryDocument {
            schema_version: u16,
            network_id: NetworkId,
            public_key: PublicKey,
            publisher: NodeId,
            name: String,
            purpose: String,
            #[serde(default = "default_network_language")]
            language: String,
            #[serde(default = "default_network_region")]
            region: String,
            #[serde(default)]
            tags: Vec<String>,
            visibility: NetworkVisibility,
            join_mode: NetworkJoinMode,
            owner: NodeId,
            #[serde(default)]
            administrators: Vec<NodeId>,
            revision: u64,
            policy_revision: u64,
            privacy_policy_revision: u64,
            #[serde(default)]
            member_limit: Option<u32>,
            #[serde(default)]
            member_count: u32,
            #[serde(default)]
            exit_contribution: ExitContributionObligation,
            #[serde(default)]
            exit_contribution_scope: Option<ExitContributionScope>,
            #[serde(default)]
            exit_contribution_mode: Option<ExitContributionMode>,
            #[serde(default)]
            privacy_changes: Vec<String>,
            issued_at_unix_ms: u64,
            expires_at_unix_ms: u64,
            capability_summary: Vec<CapabilityKind>,
            #[serde(default)]
            endpoints: Vec<NetworkEndpoint>,
            #[serde(default)]
            revoked_at_unix_ms: Option<u64>,
        }

        let raw = RawNetworkDiscoveryDocument::deserialize(deserializer)?;
        Ok(Self {
            schema_version: raw.schema_version,
            network_id: raw.network_id,
            public_key: raw.public_key,
            publisher: raw.publisher,
            name: raw.name,
            purpose: raw.purpose,
            language: raw.language,
            region: raw.region,
            tags: raw.tags,
            visibility: raw.visibility,
            join_mode: raw.join_mode,
            owner: raw.owner,
            administrators: raw.administrators,
            revision: raw.revision,
            policy_revision: raw.policy_revision,
            privacy_policy_revision: raw.privacy_policy_revision,
            member_limit: raw.member_limit,
            member_count: raw.member_count,
            exit_contribution: raw.exit_contribution,
            exit_contribution_scope: raw
                .exit_contribution_scope
                .unwrap_or_else(|| legacy_exit_contribution_scope(raw.exit_contribution)),
            exit_contribution_mode: raw
                .exit_contribution_mode
                .unwrap_or_else(|| ExitContributionMode::from_legacy(raw.exit_contribution)),
            privacy_changes: raw.privacy_changes,
            issued_at_unix_ms: raw.issued_at_unix_ms,
            expires_at_unix_ms: raw.expires_at_unix_ms,
            capability_summary: raw.capability_summary,
            endpoints: raw.endpoints,
            revoked_at_unix_ms: raw.revoked_at_unix_ms,
        })
    }
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
        if self.revision == 0 || self.policy_revision == 0 || self.privacy_policy_revision == 0 {
            return Err(DomainError::invalid_value(
                "discovery revisions must be positive",
            ));
        }
        if self.member_limit == Some(0) || self.member_count == 0 {
            return Err(DomainError::invalid_value(
                "discovery member counts are invalid",
            ));
        }
        validate_exit_contribution_policy(
            self.exit_contribution,
            self.exit_contribution_scope,
            self.exit_contribution_mode,
        )?;
        if self
            .member_limit
            .is_some_and(|limit| self.member_count > limit)
        {
            return Err(DomainError::invariant(
                "discovery member count exceeds the configured limit",
            ));
        }
        if self.privacy_changes.len() > 32
            || self
                .privacy_changes
                .iter()
                .any(|change| change.is_empty() || change.chars().count() > 256)
        {
            return Err(DomainError::invalid_value(
                "discovery privacy changes are invalid",
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
        Self::from_record_with_endpoints(
            record,
            revision,
            issued_at_unix_ms,
            expires_at_unix_ms,
            revoked_at_unix_ms,
            Vec::new(),
        )
    }

    pub fn from_record_with_endpoints(
        record: &NetworkRecord,
        revision: u64,
        issued_at_unix_ms: u64,
        expires_at_unix_ms: u64,
        revoked_at_unix_ms: Option<u64>,
        endpoints: Vec<NetworkEndpoint>,
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
            administrators: record.administrators.clone(),
            revision,
            policy_revision: record.policy_revision,
            privacy_policy_revision: record.privacy_policy_revision,
            member_limit: record.member_limit,
            member_count: u32::try_from(record.members.len()).unwrap_or(u32::MAX),
            exit_contribution: record.exit_contribution,
            exit_contribution_scope: record.exit_contribution_scope,
            exit_contribution_mode: record.exit_contribution_mode,
            privacy_changes: record.privacy_changes.clone(),
            issued_at_unix_ms,
            expires_at_unix_ms,
            capability_summary: record.capability_declarations.clone(),
            endpoints,
            revoked_at_unix_ms,
        };
        document.validate_at(issued_at_unix_ms)?;
        Ok(document)
    }

    pub fn signing_payload(&self) -> DomainResult<Vec<u8>> {
        #[derive(Serialize)]
        struct CanonicalDiscoveryDocument<'a> {
            schema_version: u16,
            network_id: &'a NetworkId,
            public_key: &'a PublicKey,
            publisher: &'a NodeId,
            name: &'a str,
            purpose: &'a str,
            language: &'a str,
            region: &'a str,
            tags: &'a [String],
            visibility: NetworkVisibility,
            join_mode: NetworkJoinMode,
            owner: &'a NodeId,
            administrators: &'a [NodeId],
            revision: u64,
            policy_revision: u64,
            privacy_policy_revision: u64,
            #[serde(skip_serializing_if = "Option::is_none")]
            member_limit: Option<u32>,
            member_count: u32,
            exit_contribution: ExitContributionObligation,
            #[serde(skip_serializing_if = "Option::is_none")]
            exit_contribution_scope: Option<ExitContributionScope>,
            #[serde(skip_serializing_if = "Option::is_none")]
            exit_contribution_mode: Option<ExitContributionMode>,
            privacy_changes: &'a [String],
            issued_at_unix_ms: u64,
            expires_at_unix_ms: u64,
            capability_summary: &'a [CapabilityKind],
            endpoints: &'a [NetworkEndpoint],
            #[serde(skip_serializing_if = "Option::is_none")]
            revoked_at_unix_ms: Option<u64>,
        }

        let legacy_scope = self.exit_contribution_scope
            == legacy_exit_contribution_scope(self.exit_contribution)
            && !(self.exit_contribution == ExitContributionObligation::Required
                && self.exit_contribution_scope == ExitContributionScope::AllMembers);
        let legacy_mode = self.exit_contribution_mode
            == ExitContributionMode::from_legacy(self.exit_contribution);
        let canonical = CanonicalDiscoveryDocument {
            schema_version: self.schema_version,
            network_id: &self.network_id,
            public_key: &self.public_key,
            publisher: &self.publisher,
            name: &self.name,
            purpose: &self.purpose,
            language: &self.language,
            region: &self.region,
            tags: &self.tags,
            visibility: self.visibility,
            join_mode: self.join_mode,
            owner: &self.owner,
            administrators: &self.administrators,
            revision: self.revision,
            policy_revision: self.policy_revision,
            privacy_policy_revision: self.privacy_policy_revision,
            member_limit: self.member_limit,
            member_count: self.member_count,
            exit_contribution: self.exit_contribution,
            exit_contribution_scope: (!legacy_scope).then_some(self.exit_contribution_scope),
            exit_contribution_mode: (!legacy_mode).then_some(self.exit_contribution_mode),
            privacy_changes: &self.privacy_changes,
            issued_at_unix_ms: self.issued_at_unix_ms,
            expires_at_unix_ms: self.expires_at_unix_ms,
            capability_summary: &self.capability_summary,
            endpoints: &self.endpoints,
            revoked_at_unix_ms: self.revoked_at_unix_ms,
        };
        serde_json::to_vec(&canonical)
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
            administrators: Vec::new(),
            revision,
            policy_revision: 1,
            privacy_policy_revision: 1,
            member_limit: None,
            member_count: 1,
            exit_contribution: ExitContributionObligation::NotRequired,
            exit_contribution_scope: ExitContributionScope::NotApplicable,
            exit_contribution_mode: ExitContributionMode::Disabled,
            privacy_changes: Vec::new(),
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

    #[test]
    fn legacy_discovery_scope_defaults_and_signatures_remain_compatible() {
        let identity = Identity::generate().unwrap();
        let key = ed25519_dalek::SigningKey::from_bytes(&identity.signing_key_seed());
        let mut signed = signed_record(1, 100, &key);
        let mut legacy_json = serde_json::to_value(&signed.document).unwrap();
        legacy_json
            .as_object_mut()
            .unwrap()
            .remove("exit_contribution_scope");
        legacy_json
            .as_object_mut()
            .unwrap()
            .remove("exit_contribution_mode");
        legacy_json["exit_contribution"] = serde_json::json!("required");

        let required: NetworkDiscoveryDocument =
            serde_json::from_value(legacy_json.clone()).unwrap();
        assert_eq!(
            required.exit_contribution_scope,
            ExitContributionScope::JoiningMembers
        );
        required.validate_at(100).unwrap();

        #[derive(Serialize)]
        struct LegacyDiscovery<'a> {
            schema_version: u16,
            network_id: &'a NetworkId,
            public_key: &'a PublicKey,
            publisher: &'a NodeId,
            name: &'a str,
            purpose: &'a str,
            language: &'a str,
            region: &'a str,
            tags: &'a [String],
            visibility: NetworkVisibility,
            join_mode: NetworkJoinMode,
            owner: &'a NodeId,
            administrators: &'a [NodeId],
            revision: u64,
            policy_revision: u64,
            privacy_policy_revision: u64,
            #[serde(skip_serializing_if = "Option::is_none")]
            member_limit: Option<u32>,
            member_count: u32,
            exit_contribution: ExitContributionObligation,
            privacy_changes: &'a [String],
            issued_at_unix_ms: u64,
            expires_at_unix_ms: u64,
            capability_summary: &'a [CapabilityKind],
            endpoints: &'a [NetworkEndpoint],
            #[serde(skip_serializing_if = "Option::is_none")]
            revoked_at_unix_ms: Option<u64>,
        }
        let legacy = LegacyDiscovery {
            schema_version: required.schema_version,
            network_id: &required.network_id,
            public_key: &required.public_key,
            publisher: &required.publisher,
            name: &required.name,
            purpose: &required.purpose,
            language: &required.language,
            region: &required.region,
            tags: &required.tags,
            visibility: required.visibility,
            join_mode: required.join_mode,
            owner: &required.owner,
            administrators: &required.administrators,
            revision: required.revision,
            policy_revision: required.policy_revision,
            privacy_policy_revision: required.privacy_policy_revision,
            member_limit: required.member_limit,
            member_count: required.member_count,
            exit_contribution: required.exit_contribution,
            privacy_changes: &required.privacy_changes,
            issued_at_unix_ms: required.issued_at_unix_ms,
            expires_at_unix_ms: required.expires_at_unix_ms,
            capability_summary: &required.capability_summary,
            endpoints: &required.endpoints,
            revoked_at_unix_ms: required.revoked_at_unix_ms,
        };
        let legacy_payload = serde_json::to_vec(&legacy).unwrap();
        assert_eq!(required.signing_payload().unwrap(), legacy_payload);
        signed.document = required;
        signed.signature = Signature::from_bytes(key.sign(&legacy_payload).to_bytes());
        signed.validate_at(100).unwrap();
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
            recovery_authorities: Vec::new(),
            recovery_threshold: 0,
            members: vec![owner, admin],
            revision: 1,
            policy_revision: 1,
            privacy_policy_revision: 1,
            member_limit: None,
            exit_contribution: ExitContributionObligation::NotRequired,
            exit_contribution_scope: ExitContributionScope::NotApplicable,
            exit_contribution_mode: ExitContributionMode::Disabled,
            exit_selection_policy: ExitSelectionPolicy::default(),
            privacy_changes: Vec::new(),
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
            exit_contribution: ExitContributionObligation::NotRequired,
            exit_contribution_scope: ExitContributionScope::NotApplicable,
            exit_contribution_mode: ExitContributionMode::Disabled,
            exit_selection_policy: ExitSelectionPolicy::default(),
        }
    }

    fn join_preview_fixture() -> (NetworkRecord, Identity) {
        let coordinator_identity = Identity::generate().unwrap();
        let owner = NodeId::from_public_key(&coordinator_identity.public_key());
        let network = NetworkRecord::create_owned(
            create_profile(),
            PublicKey::from_bytes([33; 32]),
            owner,
            1_000,
        )
        .unwrap();
        (network, coordinator_identity)
    }

    #[test]
    fn network_join_preview_is_signed_private_and_exactly_bound_to_record() {
        let (network, coordinator_identity) = join_preview_fixture();
        let issued_at = 10_000;
        let expires_at = issued_at + NETWORK_JOIN_PREVIEW_VALIDITY_MS;
        let preview =
            NetworkJoinPreview::for_record(&network, &coordinator_identity, issued_at, expires_at)
                .unwrap();

        preview
            .validate_for_record(&network, &coordinator_identity.public_key(), issued_at + 1)
            .unwrap();
        assert!(preview.signature_is_valid(&coordinator_identity.public_key()));
        assert_eq!(preview.preview_digest, preview.compute_digest().unwrap());
        assert_eq!(
            preview.terms.network_policy_revision,
            preview.policy_revision
        );
        assert_eq!(
            preview.terms.privacy_policy_revision,
            preview.privacy_policy_revision
        );
        let encoded = crate::codec::encode(&preview).expect("postcard-encode join preview");
        let decoded: NetworkJoinPreview =
            crate::codec::decode(&encoded).expect("postcard-decode join preview");
        assert_eq!(decoded, preview);
        assert_eq!(decoded.terms.content_hash().unwrap(), preview.terms_hash);

        let value = serde_json::to_value(&preview).unwrap();
        let object = value.as_object().unwrap();
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
            "private_key",
            "signing_key",
            "identity",
            "ticket",
            "invitation_code",
        ] {
            assert!(
                !object.contains_key(forbidden),
                "join preview leaked {forbidden}"
            );
            assert!(!value.to_string().contains(forbidden));
        }
    }

    #[test]
    fn network_join_preview_rejects_tampering_wrong_signer_and_invalid_time() {
        let (network, coordinator_identity) = join_preview_fixture();
        let issued_at = 10_000;
        let expires_at = issued_at + NETWORK_JOIN_PREVIEW_VALIDITY_MS;
        let preview =
            NetworkJoinPreview::for_record(&network, &coordinator_identity, issued_at, expires_at)
                .unwrap();
        let coordinator_public_key = coordinator_identity.public_key();

        let mut changed_terms = preview.clone();
        changed_terms.terms.network_policy_revision = 2;
        assert!(changed_terms
            .validate_for_record(&network, &coordinator_public_key, issued_at + 1)
            .is_err());

        let mut changed_digest = preview.clone();
        changed_digest.preview_digest = ContentHash::from_bytes(b"tampered");
        assert!(changed_digest
            .validate_for_record(&network, &coordinator_public_key, issued_at + 1)
            .is_err());

        let mut changed_signature = preview.clone();
        changed_signature.signature = Signature::from_bytes([7; 64]);
        assert_eq!(
            changed_signature
                .validate_for_record(&network, &coordinator_public_key, issued_at + 1)
                .unwrap_err()
                .code(),
            "domain.invalid_signature"
        );

        let wrong_signer = Identity::generate().unwrap();
        assert!(preview
            .validate_for_record(&network, &wrong_signer.public_key(), issued_at + 1)
            .is_err());
        assert!(
            NetworkJoinPreview::for_record(&network, &wrong_signer, issued_at, expires_at).is_err()
        );

        assert!(preview
            .validate_for_record(&network, &coordinator_public_key, expires_at)
            .is_err());
        let future = NetworkJoinPreview::for_record(
            &network,
            &coordinator_identity,
            issued_at + NETWORK_JOIN_PREVIEW_CLOCK_SKEW_MS + 1,
            issued_at + NETWORK_JOIN_PREVIEW_CLOCK_SKEW_MS + 2,
        )
        .unwrap();
        assert!(future
            .validate_for_record(&network, &coordinator_public_key, issued_at)
            .is_err());
        assert!(NetworkJoinPreview::for_record(
            &network,
            &coordinator_identity,
            issued_at,
            expires_at + 1
        )
        .is_err());
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
    fn exit_contribution_mode_change_is_diffed_and_advances_policy_revision() {
        let identity = Identity::generate().unwrap();
        let public_key = PublicKey::from_bytes(identity.public_key_bytes());
        let owner = NodeId::from_public_key(b"owner");
        let mut network =
            NetworkRecord::create_owned(create_profile(), public_key, owner, 100).unwrap();
        network.sign_public_profile(&identity, 100, 1_000).unwrap();

        let mut update = NetworkProfileUpdate::from_record(&network);
        update.exit_contribution = ExitContributionObligation::Required;
        update.exit_contribution_scope = ExitContributionScope::AllMembers;
        update.exit_contribution_mode = ExitContributionMode::RequiredOnJoin;
        let diff = network.apply_profile_update(&update, 1, 200).unwrap();

        assert!(diff
            .iter()
            .any(|entry| entry.path == "exit_contribution_mode"));
        assert_eq!(
            network.exit_contribution_mode,
            ExitContributionMode::RequiredOnJoin
        );
        assert_eq!(network.policy_revision, 2);

        let mut update = NetworkProfileUpdate::from_record(&network);
        update.exit_contribution_mode = ExitContributionMode::Autonomous;
        update.exit_contribution = ExitContributionObligation::NotRequired;
        update.exit_contribution_scope = ExitContributionScope::NotApplicable;
        let diff = network.apply_profile_update(&update, 2, 300).unwrap();
        assert!(diff
            .iter()
            .any(|entry| entry.path == "exit_contribution_mode"));
        assert_eq!(network.policy_revision, 3);
    }

    #[test]
    fn exit_contribution_policy_rejects_inconsistent_mode_pairs() {
        let mut profile = create_profile();
        profile.exit_contribution = ExitContributionObligation::Required;
        profile.exit_contribution_scope = ExitContributionScope::JoiningMembers;
        profile.exit_contribution_mode = ExitContributionMode::Disabled;
        assert!(profile.validate().is_err());

        profile.exit_contribution_mode = ExitContributionMode::ApprovalRequired;
        assert!(profile.validate().is_err());

        profile.exit_contribution = ExitContributionObligation::NotRequired;
        profile.exit_contribution_scope = ExitContributionScope::NotApplicable;
        profile.exit_contribution_mode = ExitContributionMode::ApprovalRequired;
        profile.validate().unwrap();
    }

    #[test]
    fn exit_selection_policy_change_is_diffed_and_advances_policy_revision() {
        let identity = Identity::generate().unwrap();
        let public_key = PublicKey::from_bytes(identity.public_key_bytes());
        let owner = NodeId::from_public_key(b"owner");
        let mut network =
            NetworkRecord::create_owned(create_profile(), public_key, owner, 100).unwrap();
        network.sign_public_profile(&identity, 100, 1_000).unwrap();
        let exit = NodeId::from_public_key(b"exit-a");
        let mut update = NetworkProfileUpdate::from_record(&network);
        update.exit_selection_policy = Some(ExitSelectionPolicy {
            mode: crate::domain::ExitSelectionMode::Fixed { exit: exit.clone() },
            available_exits: vec![exit.clone()],
            ..ExitSelectionPolicy::default()
        });

        let diff = network.apply_profile_update(&update, 1, 200).unwrap();
        assert!(diff
            .iter()
            .any(|entry| entry.path == "exit_selection_policy"));
        assert_eq!(network.exit_selection_policy.fixed_exit(), Some(&exit));
        assert_eq!(network.policy_revision, 2);
    }

    #[test]
    fn legacy_profile_update_without_exit_selection_policy_preserves_governance() {
        let identity = Identity::generate().unwrap();
        let public_key = PublicKey::from_bytes(identity.public_key_bytes());
        let owner = NodeId::from_public_key(b"owner");
        let exit = NodeId::from_public_key(b"fixed-exit");
        let mut profile = create_profile();
        profile.exit_selection_policy = ExitSelectionPolicy {
            mode: crate::domain::ExitSelectionMode::Fixed { exit: exit.clone() },
            available_exits: vec![exit.clone()],
            ..ExitSelectionPolicy::default()
        };
        let mut network = NetworkRecord::create_owned(profile, public_key, owner, 100).unwrap();
        network.sign_public_profile(&identity, 100, 1_000).unwrap();
        let original_policy_revision = network.policy_revision;

        let mut update = NetworkProfileUpdate::from_record(&network);
        update.exit_selection_policy = None;
        let encoded = serde_json::to_value(&update).unwrap();
        assert!(encoded.get("exit_selection_policy").is_none());
        let decoded: NetworkProfileUpdate = serde_json::from_value(encoded).unwrap();
        assert!(decoded.exit_selection_policy.is_none());

        network.apply_profile_update(&decoded, 1, 200).unwrap();
        assert_eq!(network.exit_selection_policy.fixed_exit(), Some(&exit));
        assert_eq!(network.policy_revision, original_policy_revision);
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
