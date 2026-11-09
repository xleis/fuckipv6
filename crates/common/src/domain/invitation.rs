use std::collections::BTreeSet;
use std::fmt;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::capability::CapabilityKind;
use super::ids::{ContentHash, InvitationId, NetworkId, NodeId};
use super::membership::{MembershipRole, MembershipTerms};
use super::{DomainError, DomainResult};

pub const INVITATION_CODE_PREFIX: &str = "fkinv_";
pub const INVITATION_CODE_BYTES: usize = 32;
pub const INVITATION_MAX_USES: u32 = 100;
pub const INVITATION_MIN_TTL_MS: u64 = 60_000;
pub const INVITATION_MAX_TTL_MS: u64 = 30 * 24 * 60 * 60 * 1000;
pub const INVITATION_DEFAULT_TTL_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// A high-entropy invitation secret. It is intentionally not a URL and its
/// `Debug` representation is redacted so generic tracing cannot disclose it.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct InvitationCode(String);

impl InvitationCode {
    pub fn generate() -> DomainResult<Self> {
        let mut bytes = [0_u8; INVITATION_CODE_BYTES];
        getrandom::fill(&mut bytes)
            .map_err(|_| DomainError::invalid_value("invitation code generation failed"))?;
        Ok(Self(format!(
            "{INVITATION_CODE_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(bytes)
        )))
    }

    #[cfg(test)]
    pub(crate) fn from_test_bytes(bytes: &[u8]) -> Self {
        Self(format!(
            "{INVITATION_CODE_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(bytes)
        ))
    }

    pub fn parse(value: impl Into<String>) -> DomainResult<Self> {
        let value = value.into();
        let Some(encoded) = value.strip_prefix(INVITATION_CODE_PREFIX) else {
            return Err(DomainError::invalid_id(
                "invitation code has an invalid prefix",
            ));
        };
        if encoded.len() != 43 || encoded.contains(['/', '+', '=']) {
            return Err(DomainError::invalid_id(
                "invitation code has invalid encoding",
            ));
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| DomainError::invalid_id("invitation code has invalid encoding"))?;
        if bytes.len() != INVITATION_CODE_BYTES {
            return Err(DomainError::invalid_id(
                "invitation code has invalid length",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The only value that may be durably stored.
    pub fn digest(&self) -> ContentHash {
        ContentHash::from_bytes(self.0.as_bytes())
    }
}

impl fmt::Debug for InvitationCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("InvitationCode([redacted])")
    }
}

impl TryFrom<String> for InvitationCode {
    type Error = DomainError;

    fn try_from(value: String) -> DomainResult<Self> {
        Self::parse(value)
    }
}

impl From<InvitationCode> for String {
    fn from(value: InvitationCode) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvitationState {
    Active,
    Exhausted,
    Expired,
    Revoked,
}

impl InvitationState {
    pub fn is_active(self) -> bool {
        self == Self::Active
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct InvitationCreateRequest {
    pub network_id: NetworkId,
    pub role: MembershipRole,
    #[serde(default)]
    pub capabilities: Vec<CapabilityKind>,
    #[serde(default = "default_invitation_max_uses")]
    pub max_uses: u32,
    #[serde(default = "default_invitation_ttl_ms")]
    expires_in_ms: u64,
}

impl InvitationCreateRequest {
    pub fn new(
        network_id: NetworkId,
        role: MembershipRole,
        capabilities: Vec<CapabilityKind>,
        max_uses: u32,
        expires_in_ms: u64,
    ) -> Self {
        Self {
            network_id,
            role,
            capabilities,
            max_uses,
            expires_in_ms,
        }
    }

    pub fn validate(&self) -> DomainResult<()> {
        if !matches!(
            self.role,
            MembershipRole::Member | MembershipRole::Administrator
        ) {
            return Err(DomainError::invalid_value(
                "invitations cannot grant network ownership",
            ));
        }
        if self.max_uses == 0 || self.max_uses > INVITATION_MAX_USES {
            return Err(DomainError::invalid_value(
                "invitation use count must be between 1 and 100",
            ));
        }
        if !(INVITATION_MIN_TTL_MS..=INVITATION_MAX_TTL_MS).contains(&self.expires_in_ms) {
            return Err(DomainError::invalid_value(
                "invitation expiry must be between one minute and thirty days",
            ));
        }
        let mut seen = BTreeSet::new();
        if self
            .capabilities
            .iter()
            .any(|capability| !seen.insert(*capability))
        {
            return Err(DomainError::invalid_value(
                "invitation capabilities must be unique",
            ));
        }
        Ok(())
    }

    pub fn expires_in_ms(&self) -> u64 {
        self.expires_in_ms
    }
}

fn default_invitation_max_uses() -> u32 {
    1
}

fn default_invitation_ttl_ms() -> u64 {
    INVITATION_DEFAULT_TTL_MS
}

/// Durable invitation metadata. It never contains plaintext invitation code
/// material; the Coordinator stores only `InvitationCode::digest()` in its
/// private ledger.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct InvitationRecord {
    pub invitation_id: InvitationId,
    pub network_id: NetworkId,
    pub created_by: NodeId,
    pub role: MembershipRole,
    #[serde(default)]
    pub capabilities: Vec<CapabilityKind>,
    pub max_uses: u32,
    pub used_count: u32,
    pub created_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_by: Option<NodeId>,
}

impl InvitationRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        invitation_id: InvitationId,
        network_id: NetworkId,
        created_by: NodeId,
        role: MembershipRole,
        capabilities: Vec<CapabilityKind>,
        max_uses: u32,
        created_at_unix_ms: u64,
        expires_at_unix_ms: u64,
    ) -> DomainResult<Self> {
        let record = Self {
            invitation_id,
            network_id,
            created_by,
            role,
            capabilities,
            max_uses,
            used_count: 0,
            created_at_unix_ms,
            expires_at_unix_ms,
            revision: 1,
            revoked_at_unix_ms: None,
            revoked_by: None,
        };
        record.validate()?;
        Ok(record)
    }

    pub fn validate(&self) -> DomainResult<()> {
        if !matches!(
            self.role,
            MembershipRole::Member | MembershipRole::Administrator
        ) {
            return Err(DomainError::invalid_value(
                "invitation record cannot grant network ownership",
            ));
        }
        if self.max_uses == 0 || self.max_uses > INVITATION_MAX_USES {
            return Err(DomainError::invalid_value(
                "invitation record use count is invalid",
            ));
        }
        if self.used_count > self.max_uses {
            return Err(DomainError::invariant("invitation usage exceeds its bound"));
        }
        if self.expires_at_unix_ms <= self.created_at_unix_ms {
            return Err(DomainError::invariant(
                "invitation expiry must follow creation",
            ));
        }
        if self.revision == 0 {
            return Err(DomainError::invalid_value(
                "invitation revision must be positive",
            ));
        }
        match (self.revoked_at_unix_ms, self.revoked_by.as_ref()) {
            (None, None) => {}
            (Some(revoked_at), Some(_)) if revoked_at >= self.created_at_unix_ms => {}
            _ => {
                return Err(DomainError::invariant(
                    "invitation revocation metadata is inconsistent",
                ))
            }
        }
        let mut seen = BTreeSet::new();
        if self
            .capabilities
            .iter()
            .any(|capability| !seen.insert(*capability))
        {
            return Err(DomainError::invalid_value(
                "invitation record capabilities must be unique",
            ));
        }
        Ok(())
    }

    pub fn state_at(&self, now_unix_ms: u64) -> InvitationState {
        if self.revoked_at_unix_ms.is_some() {
            InvitationState::Revoked
        } else if now_unix_ms >= self.expires_at_unix_ms {
            InvitationState::Expired
        } else if self.used_count >= self.max_uses {
            InvitationState::Exhausted
        } else {
            InvitationState::Active
        }
    }

    pub fn remaining_uses(&self) -> u32 {
        self.max_uses.saturating_sub(self.used_count)
    }

    pub fn revoke(&mut self, actor: NodeId, now_unix_ms: u64) -> DomainResult<bool> {
        if self.revoked_at_unix_ms.is_some() {
            return Ok(false);
        }
        if now_unix_ms < self.created_at_unix_ms {
            return Err(DomainError::invariant(
                "invitation revocation time predates creation",
            ));
        }
        self.revoked_at_unix_ms = Some(now_unix_ms);
        self.revoked_by = Some(actor);
        self.revision = self.revision.saturating_add(1);
        self.validate()?;
        Ok(true)
    }

    /// Validate and consume one use. Callers must perform this only after
    /// checking the invitation digest, current network binding and creator
    /// authority under the same lock.
    pub fn consume_one(&mut self, now_unix_ms: u64) -> DomainResult<()> {
        if !self.state_at(now_unix_ms).is_active() {
            return Err(DomainError::new(
                "domain.invitation_unavailable",
                "invitation cannot be consumed",
            ));
        }
        self.used_count += 1;
        self.revision = self.revision.saturating_add(1);
        self.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct InvitationStatusView {
    pub invitation: InvitationRecord,
    pub state: InvitationState,
    pub remaining_uses: u32,
}

impl InvitationStatusView {
    pub fn new(invitation: InvitationRecord, now_unix_ms: u64) -> Self {
        let state = invitation.state_at(now_unix_ms);
        let remaining_uses = invitation.remaining_uses();
        Self {
            invitation,
            state,
            remaining_uses,
        }
    }
}

/// Safe response from creation. The plaintext code may be displayed/copied once
/// and is deliberately excluded from list/status responses.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct IssuedInvitation {
    pub status: InvitationStatusView,
    pub code: InvitationCode,
}

#[derive(Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct InvitationPreviewRequest {
    pub network_id: NetworkId,
    pub code: InvitationCode,
}

impl fmt::Debug for InvitationPreviewRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InvitationPreviewRequest")
            .field("network_id", &self.network_id)
            .field("code", &"[redacted]")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct InvitationRedeemRequest {
    pub network_id: NetworkId,
    pub code: InvitationCode,
    pub invitation_id: InvitationId,
    pub expected_invitation_revision: u64,
    pub preview_digest: ContentHash,
    pub terms_hash: ContentHash,
    pub acknowledged_at_unix_ms: u64,
    /// Explicit consent to the current Exit contribution obligation. It is
    /// required when the invited network requires contribution and is never
    /// inferred from accepting the invitation itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_contribution_acknowledged: Option<bool>,
}

impl fmt::Debug for InvitationRedeemRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InvitationRedeemRequest")
            .field("network_id", &self.network_id)
            .field("code", &"[redacted]")
            .field("invitation_id", &self.invitation_id)
            .field(
                "expected_invitation_revision",
                &self.expected_invitation_revision,
            )
            .field("preview_digest", &self.preview_digest)
            .field("terms_hash", &self.terms_hash)
            .field("acknowledged_at_unix_ms", &self.acknowledged_at_unix_ms)
            .field(
                "exit_contribution_acknowledged",
                &self.exit_contribution_acknowledged,
            )
            .finish()
    }
}

/// Everything the accepting user must see before confirming. It contains no
/// roster addresses, topology, activity prose or invitation credential.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct InvitationPreview {
    pub invitation_id: InvitationId,
    pub invitation_revision: u64,
    pub network_id: NetworkId,
    pub network_name: String,
    pub network_revision: u64,
    pub role: MembershipRole,
    pub capabilities: Vec<CapabilityKind>,
    pub remaining_uses: u32,
    pub expires_at_unix_ms: u64,
    pub state: InvitationState,
    pub terms: MembershipTerms,
    pub terms_hash: ContentHash,
    pub preview_digest: ContentHash,
    pub permission_changes: Vec<String>,
    /// Redemption creates ordinary membership and never an invitation-owned or
    /// permanent relationship.
    pub creates_permanent_membership: bool,
    pub requires_recent_reauthentication: bool,
    pub can_accept: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denial_key: Option<String>,
}

impl InvitationPreview {
    pub fn compute_digest(&self) -> DomainResult<ContentHash> {
        let mut canonical = self.clone();
        canonical.preview_digest =
            ContentHash::from_bytes(b"invitation-preview-digest-placeholder");
        let bytes = serde_json::to_vec(&canonical)
            .map_err(|_| DomainError::invalid_value("invitation preview cannot be encoded"))?;
        Ok(ContentHash::from_bytes(&bytes))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct InvitationRedemption {
    pub invitation_id: InvitationId,
    pub network_id: NetworkId,
    pub node_id: NodeId,
    pub role: MembershipRole,
    pub capabilities: Vec<CapabilityKind>,
    /// Manager whose active invitation authorized this admission. The
    /// lifecycle ledger revalidates this identity at redemption time.
    pub invited_by: NodeId,
    pub network_revision: u64,
    pub invitation_revision: u64,
    pub joined_at_unix_ms: u64,
    /// True only for an idempotent retry by the same authenticated node.
    pub already_redeemed: bool,
    pub grants_permanent_membership: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct InvitationChanged {
    pub invitation_id: InvitationId,
    pub state: InvitationState,
    pub revision: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> InvitationRecord {
        InvitationRecord::new(
            InvitationId::from_public_key(b"invitation"),
            NetworkId::from_public_key(b"network"),
            NodeId::from_public_key(b"owner"),
            MembershipRole::Member,
            vec![CapabilityKind::Exit, CapabilityKind::File],
            1,
            100,
            1_000,
        )
        .unwrap()
    }

    #[test]
    fn code_debug_and_errors_never_include_plaintext() {
        let code = InvitationCode::from_test_bytes(&[7_u8; INVITATION_CODE_BYTES]);
        let plaintext = code.as_str().to_owned();
        let message = crate::SignalingMessage::InvitationPreview {
            request: InvitationPreviewRequest {
                network_id: NetworkId::from_public_key(b"network"),
                code: code.clone(),
            },
        };

        assert!(!format!("{code:?}").contains(&plaintext));
        assert!(!format!("{message:?}").contains(&plaintext));
        assert_eq!(code.digest(), code.clone().digest());
        assert_eq!(
            InvitationCode::parse("https://example.invalid/?code=secret")
                .unwrap_err()
                .code(),
            "domain.invalid_id"
        );
    }

    #[test]
    fn expiry_revocation_and_use_limit_are_explicit() {
        let mut invitation = record();
        assert_eq!(invitation.state_at(999), InvitationState::Active);
        assert_eq!(invitation.state_at(1_000), InvitationState::Expired);

        invitation.consume_one(500).unwrap();
        assert_eq!(invitation.state_at(500), InvitationState::Exhausted);
        assert_eq!(invitation.remaining_uses(), 0);

        invitation
            .revoke(NodeId::from_public_key(b"admin"), 600)
            .unwrap();
        assert_eq!(invitation.state_at(600), InvitationState::Revoked);
        assert_eq!(invitation.revision, 3);
    }

    #[test]
    fn create_request_rejects_owner_and_invalid_bounds() {
        let request = InvitationCreateRequest {
            network_id: NetworkId::from_public_key(b"network"),
            role: MembershipRole::Owner,
            capabilities: vec![],
            max_uses: 0,
            expires_in_ms: 0,
        };
        assert_eq!(
            request.validate().unwrap_err().code(),
            "domain.invalid_value"
        );
    }

    #[test]
    fn preview_digest_is_stable_and_does_not_include_code() {
        let invitation = record();
        let terms = MembershipTerms {
            network_policy_revision: 1,
            privacy_policy_revision: 1,
            exit_contribution: Default::default(),
            exit_contribution_scope: Default::default(),
            exit_contribution_mode: Default::default(),
            member_limit: None,
        };
        let mut preview = InvitationPreview {
            invitation_id: invitation.invitation_id.clone(),
            invitation_revision: invitation.revision,
            network_id: invitation.network_id.clone(),
            network_name: "network".into(),
            network_revision: 1,
            role: invitation.role,
            capabilities: invitation.capabilities.clone(),
            remaining_uses: 1,
            expires_at_unix_ms: invitation.expires_at_unix_ms,
            state: InvitationState::Active,
            terms_hash: terms.content_hash().unwrap(),
            terms,
            preview_digest: ContentHash::from_bytes(b"temporary"),
            permission_changes: vec!["member".into()],
            creates_permanent_membership: false,
            requires_recent_reauthentication: true,
            can_accept: true,
            denial_key: None,
        };
        let first = preview.compute_digest().unwrap();
        preview.preview_digest = first.clone();
        assert!(!first.as_str().contains(INVITATION_CODE_PREFIX));
        assert_eq!(preview.compute_digest().unwrap(), first);
    }
}
