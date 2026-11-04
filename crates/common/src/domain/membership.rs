use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::capability::CapabilityKind;
use super::ids::{ContentHash, JoinRequestId, NetworkId, NodeId};
use super::{DomainError, DomainResult};

/// A member's governance role. Temporary Coordinator authority is intentionally
/// represented by `CoordinatorTerm`, never by mutating this durable role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipRole {
    Owner,
    Administrator,
    Member,
}

/// The durable state of one node's relationship with one network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipState {
    Discovered,
    Applying,
    PendingApproval,
    Accepted,
    KeyReady,
    Connecting,
    Active,
    Degraded,
    Leaving,
    Left,
    Rejected,
    Suspended,
    Cancelled,
}

impl MembershipState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Left | Self::Rejected | Self::Cancelled)
    }

    /// Data-plane resources are available only after keys are ready and the
    /// node is actually connected. Acceptance alone never grants access.
    pub fn can_access_network_resources(self) -> bool {
        matches!(self, Self::Active | Self::Degraded)
    }

    pub fn can_transition_to(self, target: Self) -> bool {
        matches!(
            (self, target),
            (Self::Discovered, Self::Applying)
                | (Self::Applying, Self::PendingApproval)
                | (Self::Applying, Self::Accepted)
                | (Self::Applying, Self::Cancelled)
                | (Self::PendingApproval, Self::Accepted)
                | (Self::PendingApproval, Self::Rejected)
                | (Self::Accepted, Self::KeyReady)
                | (Self::Accepted, Self::Leaving)
                | (Self::KeyReady, Self::Connecting)
                | (Self::KeyReady, Self::Leaving)
                | (Self::Connecting, Self::Active)
                | (Self::Connecting, Self::Degraded)
                | (Self::Connecting, Self::Leaving)
                | (Self::Active, Self::Degraded)
                | (Self::Active, Self::Leaving)
                | (Self::Active, Self::Suspended)
                | (Self::Degraded, Self::Active)
                | (Self::Degraded, Self::Leaving)
                | (Self::Degraded, Self::Suspended)
                | (Self::Suspended, Self::Active)
                | (Self::Suspended, Self::Leaving)
                | (Self::Leaving, Self::Left)
        )
    }
}

/// Exit contribution is an explicit network admission obligation, not a
/// capability silently enabled by joining.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitContributionObligation {
    Required,
    #[default]
    NotRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct MembershipTerms {
    pub network_policy_revision: u64,
    pub privacy_policy_revision: u64,
    pub exit_contribution: ExitContributionObligation,
    /// Capacity shown when these terms were acknowledged. Omitted for legacy
    /// memberships so their content hash remains stable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_limit: Option<u32>,
}

impl MembershipTerms {
    pub fn content_hash(&self) -> DomainResult<ContentHash> {
        let bytes = serde_json::to_vec(self)
            .map_err(|_| DomainError::invalid_value("membership terms cannot be encoded"))?;
        Ok(ContentHash::from_bytes(&bytes))
    }

    pub fn validate(&self) -> DomainResult<()> {
        if self.network_policy_revision == 0 || self.privacy_policy_revision == 0 {
            return Err(DomainError::invalid_value(
                "membership policy revisions must be positive",
            ));
        }
        if self.member_limit == Some(0) {
            return Err(DomainError::invalid_value(
                "membership member limit must be positive when set",
            ));
        }
        Ok(())
    }
}

/// Proof that the owner was shown the exact terms before applying to a network.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct JoinTermsAck {
    pub terms_hash: ContentHash,
    pub acknowledged_at_unix_ms: u64,
}

/// Bound shown in the UI and enforced by the shared domain model. The count is
/// Unicode scalar values, not UTF-8 bytes, so one emoji cannot consume four
/// characters of the user's allowance.
pub const JOIN_REQUEST_MAX_REASON_CHARS: usize = 1000;
pub const JOIN_REQUEST_MAX_INFORMATION_REQUEST_CHARS: usize = 500;
pub const JOIN_REQUEST_DEFAULT_EXPIRY_MS: u64 = 7 * 24 * 60 * 60 * 1000;
/// Request text is kept only long enough to resolve disputes and appeals. The
/// same deadline is applied to rejected, withdrawn and expired requests.
pub const JOIN_REQUEST_REASON_RETENTION_MS: u64 = 30 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinRequestState {
    Pending,
    NeedsInformation,
    Approved,
    Rejected,
    Withdrawn,
    Expired,
}

impl JoinRequestState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Approved | Self::Rejected | Self::Withdrawn | Self::Expired
        )
    }

    pub fn can_transition_to(self, target: Self) -> bool {
        matches!(
            (self, target),
            (Self::Pending, Self::NeedsInformation)
                | (Self::Pending, Self::Approved)
                | (Self::Pending, Self::Rejected)
                | (Self::Pending, Self::Withdrawn)
                | (Self::Pending, Self::Expired)
                | (Self::NeedsInformation, Self::Pending)
                | (Self::NeedsInformation, Self::Approved)
                | (Self::NeedsInformation, Self::Rejected)
                | (Self::NeedsInformation, Self::Withdrawn)
                | (Self::NeedsInformation, Self::Expired)
        )
    }
}

/// The only supported visibility for request prose. The reason is deliberately
/// represented in the durable request record, not in discovery, snapshots,
/// activities, audit details or event payloads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinRequestReasonVisibility {
    #[default]
    OwnersAndAdministrators,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinRequestReviewAction {
    Approve,
    Reject,
    RequestInformation,
}

/// Public applicant metadata shown to authorized reviewers. It contains no
/// private network address, endpoint, activity or credential material.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct JoinApplicantProfile {
    pub node_id: NodeId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub public_key_fingerprint: String,
}

impl JoinApplicantProfile {
    fn validate(&self) -> DomainResult<()> {
        if self.public_key_fingerprint.is_empty() || self.public_key_fingerprint.len() > 128 {
            return Err(DomainError::invalid_value(
                "join applicant public key fingerprint is invalid",
            ));
        }
        if self.display_name.as_ref().is_some_and(|name| {
            name.is_empty()
                || name.chars().count() > 80
                || name.trim() != name
                || name.chars().any(char::is_control)
        }) {
            return Err(DomainError::invalid_value(
                "join applicant display name is invalid",
            ));
        }
        Ok(())
    }
}

/// Exact authority the member would receive if this request is approved. This
/// is persisted at submission time so reviewers cannot approve terms different
/// from those shown in the approval dialog.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct JoinRequestGrantPreview {
    pub role: MembershipRole,
    #[serde(default)]
    pub capabilities: Vec<CapabilityKind>,
    #[serde(default)]
    pub exit_contribution: ExitContributionObligation,
    #[serde(default)]
    pub file_access: bool,
    #[serde(default)]
    pub grants_data_plane_access: bool,
}

impl JoinRequestGrantPreview {
    fn validate(&self) -> DomainResult<()> {
        if self.role != MembershipRole::Member {
            return Err(DomainError::invalid_value(
                "join request grants must begin with the member role",
            ));
        }
        let mut capabilities = self.capabilities.clone();
        capabilities.sort_by_key(|capability| capability.wire_name());
        capabilities.dedup();
        if capabilities != self.capabilities {
            return Err(DomainError::invalid_value(
                "join request capabilities must be unique and canonical",
            ));
        }
        if self.file_access != self.capabilities.contains(&CapabilityKind::File) {
            return Err(DomainError::invariant(
                "join request file impact does not match granted capabilities",
            ));
        }
        Ok(())
    }
}

/// Durable approval request. The prose reason is optional solely so retention
/// cleanup can redact it after the terminal retention deadline; a live pending
/// request always has a non-empty reason.
#[derive(Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct JoinRequest {
    pub schema_version: u16,
    pub request_id: JoinRequestId,
    pub network_id: NetworkId,
    pub applicant: JoinApplicantProfile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub reason_visibility: JoinRequestReasonVisibility,
    pub state: JoinRequestState,
    /// Optimistic concurrency token. A review must supply the revision it was
    /// displayed with so two administrators cannot both approve stale copies.
    pub revision: u64,
    pub terms: MembershipTerms,
    pub grant_preview: JoinRequestGrantPreview,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_retention_until_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer_node_id: Option<NodeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub information_request: Option<String>,
}

pub const JOIN_REQUEST_SCHEMA_VERSION: u16 = 1;

impl std::fmt::Debug for JoinRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JoinRequest")
            .field("request_id", &self.request_id)
            .field("network_id", &self.network_id)
            .field("applicant", &self.applicant)
            .field("state", &self.state)
            .field("revision", &self.revision)
            .field("reason", &self.reason.as_ref().map(|_| "[redacted]"))
            .field("reason_visibility", &self.reason_visibility)
            .field("created_at_unix_ms", &self.created_at_unix_ms)
            .field("updated_at_unix_ms", &self.updated_at_unix_ms)
            .field("expires_at_unix_ms", &self.expires_at_unix_ms)
            .field(
                "reason_retention_until_unix_ms",
                &self.reason_retention_until_unix_ms,
            )
            .field("reviewer_node_id", &self.reviewer_node_id)
            .field("decided_at_unix_ms", &self.decided_at_unix_ms)
            .field(
                "information_request",
                &self.information_request.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
}

impl JoinRequest {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        request_id: JoinRequestId,
        network_id: NetworkId,
        applicant: JoinApplicantProfile,
        reason: impl Into<String>,
        terms: MembershipTerms,
        grant_preview: JoinRequestGrantPreview,
        created_at_unix_ms: u64,
        expires_at_unix_ms: u64,
    ) -> DomainResult<Self> {
        let request = Self {
            schema_version: JOIN_REQUEST_SCHEMA_VERSION,
            request_id,
            network_id,
            applicant,
            reason: Some(reason.into()),
            reason_visibility: JoinRequestReasonVisibility::OwnersAndAdministrators,
            state: JoinRequestState::Pending,
            revision: 1,
            terms,
            grant_preview,
            created_at_unix_ms,
            updated_at_unix_ms: created_at_unix_ms,
            expires_at_unix_ms,
            reason_retention_until_unix_ms: None,
            reviewer_node_id: None,
            decided_at_unix_ms: None,
            information_request: None,
        };
        request.validate()?;
        Ok(request)
    }

    /// Deterministic identifier makes transport retries idempotent while still
    /// separating requests from the same applicant for different networks.
    pub fn derived_id(
        network_id: &NetworkId,
        applicant: &NodeId,
        submission_nonce: &str,
    ) -> JoinRequestId {
        let bytes = format!("{}:{}:{}", network_id, applicant, submission_nonce);
        JoinRequestId::from_public_key(bytes.as_bytes())
    }

    pub fn review(
        &mut self,
        expected_revision: u64,
        action: JoinRequestReviewAction,
        reviewer: NodeId,
        note: Option<String>,
        now_unix_ms: u64,
    ) -> DomainResult<()> {
        if self.revision != expected_revision {
            return Err(DomainError::new(
                "domain.join_request_revision_conflict",
                "join request changed after it was displayed",
            ));
        }
        self.expire_if_due(now_unix_ms)?;
        if now_unix_ms < self.updated_at_unix_ms {
            return Err(DomainError::invariant(
                "join request update time moved backwards",
            ));
        }
        let note = note.map(|note| note.trim().to_owned());
        let target = match action {
            JoinRequestReviewAction::Approve => {
                if note.is_some() {
                    return Err(DomainError::invalid_value(
                        "approval must not carry an information request or rejection note",
                    ));
                }
                JoinRequestState::Approved
            }
            JoinRequestReviewAction::Reject => {
                validate_optional_note(note.as_deref())?;
                JoinRequestState::Rejected
            }
            JoinRequestReviewAction::RequestInformation => {
                let note = note.as_deref().ok_or_else(|| {
                    DomainError::invalid_value("requesting information requires a note")
                })?;
                validate_information_request(note)?;
                JoinRequestState::NeedsInformation
            }
        };
        self.ensure_transition(target)?;
        self.state = target;
        self.revision = self.revision.saturating_add(1);
        self.updated_at_unix_ms = now_unix_ms;
        self.reviewer_node_id = Some(reviewer);
        self.information_request = (target == JoinRequestState::NeedsInformation).then(|| {
            note.clone()
                .expect("request-information transition validated a note")
        });
        if target.is_terminal() {
            self.decided_at_unix_ms = Some(now_unix_ms);
            self.reason_retention_until_unix_ms =
                Some(now_unix_ms.saturating_add(JOIN_REQUEST_REASON_RETENTION_MS));
        }
        Ok(())
    }

    /// The applicant may answer a request for information without creating a
    /// second request. The revision advances so a reviewer holding the prior
    /// card cannot apply an action against the old submission.
    pub fn resubmit(
        &mut self,
        expected_revision: u64,
        reason: impl Into<String>,
        now_unix_ms: u64,
    ) -> DomainResult<()> {
        if self.revision != expected_revision {
            return Err(DomainError::new(
                "domain.join_request_revision_conflict",
                "join request changed after it was displayed",
            ));
        }
        self.expire_if_due(now_unix_ms)?;
        self.ensure_transition(JoinRequestState::Pending)?;
        if now_unix_ms < self.updated_at_unix_ms {
            return Err(DomainError::invariant(
                "join request update time moved backwards",
            ));
        }
        let reason = reason.into();
        validate_reason(&reason)?;
        self.state = JoinRequestState::Pending;
        self.reason = Some(reason);
        self.information_request = None;
        self.reviewer_node_id = None;
        self.revision = self.revision.saturating_add(1);
        self.updated_at_unix_ms = now_unix_ms;
        Ok(())
    }

    pub fn withdraw(&mut self, applicant: &NodeId, now_unix_ms: u64) -> DomainResult<()> {
        if self.applicant.node_id != *applicant {
            return Err(DomainError::new(
                "domain.join_request_forbidden",
                "only the applicant can withdraw this request",
            ));
        }
        self.expire_if_due(now_unix_ms)?;
        self.ensure_transition(JoinRequestState::Withdrawn)?;
        if now_unix_ms < self.updated_at_unix_ms {
            return Err(DomainError::invariant(
                "join request update time moved backwards",
            ));
        }
        self.state = JoinRequestState::Withdrawn;
        self.revision = self.revision.saturating_add(1);
        self.updated_at_unix_ms = now_unix_ms;
        self.decided_at_unix_ms = Some(now_unix_ms);
        self.reason_retention_until_unix_ms =
            Some(now_unix_ms.saturating_add(JOIN_REQUEST_REASON_RETENTION_MS));
        Ok(())
    }

    pub fn expire_if_due(&mut self, now_unix_ms: u64) -> DomainResult<bool> {
        if !self.state.is_terminal() && now_unix_ms >= self.expires_at_unix_ms {
            if now_unix_ms < self.updated_at_unix_ms {
                return Err(DomainError::invariant(
                    "join request update time moved backwards",
                ));
            }
            self.state = JoinRequestState::Expired;
            self.revision = self.revision.saturating_add(1);
            self.updated_at_unix_ms = now_unix_ms;
            self.decided_at_unix_ms = Some(now_unix_ms);
            self.reason_retention_until_unix_ms =
                Some(now_unix_ms.saturating_add(JOIN_REQUEST_REASON_RETENTION_MS));
            return Ok(true);
        }
        Ok(false)
    }

    pub fn scrub_expired_reason(&mut self, now_unix_ms: u64) -> bool {
        if self.state.is_terminal()
            && self
                .reason_retention_until_unix_ms
                .is_some_and(|deadline| now_unix_ms >= deadline)
            && (self.reason.is_some() || self.information_request.is_some())
        {
            self.reason = None;
            self.information_request = None;
            return true;
        }
        false
    }

    pub fn validate(&self) -> DomainResult<()> {
        if self.schema_version != JOIN_REQUEST_SCHEMA_VERSION {
            return Err(DomainError::new(
                "domain.join_request_unsupported_version",
                "join request schema version is unsupported",
            ));
        }
        if self.revision == 0 || self.created_at_unix_ms == 0 {
            return Err(DomainError::invalid_value(
                "join request revision and creation time must be positive",
            ));
        }
        if self.updated_at_unix_ms < self.created_at_unix_ms
            || self.expires_at_unix_ms <= self.created_at_unix_ms
        {
            return Err(DomainError::invariant(
                "join request timestamps are inconsistent",
            ));
        }
        self.applicant.validate()?;
        self.terms.validate()?;
        self.grant_preview.validate()?;
        if !self.state.is_terminal() && self.reason.as_deref().is_none_or(str::is_empty) {
            return Err(DomainError::invariant(
                "active join request is missing its review reason",
            ));
        }
        if let Some(reason) = &self.reason {
            validate_reason(reason)?;
        }
        if self.state == JoinRequestState::NeedsInformation {
            let note = self.information_request.as_deref().ok_or_else(|| {
                DomainError::invariant("information request state has no explanation")
            })?;
            validate_information_request(note)?;
        }
        if self.state.is_terminal()
            && (self.decided_at_unix_ms.is_none() || self.reason_retention_until_unix_ms.is_none())
        {
            return Err(DomainError::invariant(
                "terminal join request is missing its decision retention deadline",
            ));
        }
        Ok(())
    }

    fn ensure_transition(&self, target: JoinRequestState) -> DomainResult<()> {
        if !self.state.can_transition_to(target) {
            return Err(DomainError::new(
                "domain.invalid_join_request_transition",
                format!(
                    "join request cannot transition from {:?} to {:?}",
                    self.state, target
                ),
            ));
        }
        Ok(())
    }
}

pub fn validate_join_request_reason(reason: &str) -> DomainResult<()> {
    validate_reason(reason)
}

fn validate_reason(reason: &str) -> DomainResult<()> {
    let trimmed = reason.trim();
    if trimmed.is_empty()
        || trimmed != reason
        || reason.chars().count() > JOIN_REQUEST_MAX_REASON_CHARS
        || reason.chars().any(char::is_control)
    {
        return Err(DomainError::invalid_value(format!(
            "join request reason must be non-empty, trimmed, control-free and at most {JOIN_REQUEST_MAX_REASON_CHARS} characters"
        )));
    }
    Ok(())
}

fn validate_information_request(note: &str) -> DomainResult<()> {
    if note.trim().is_empty()
        || note.trim() != note
        || note.chars().count() > JOIN_REQUEST_MAX_INFORMATION_REQUEST_CHARS
        || note.chars().any(char::is_control)
    {
        return Err(DomainError::invalid_value(format!(
            "information request must be non-empty, trimmed, control-free and at most {JOIN_REQUEST_MAX_INFORMATION_REQUEST_CHARS} characters"
        )));
    }
    Ok(())
}

fn validate_optional_note(note: Option<&str>) -> DomainResult<()> {
    if let Some(note) = note {
        validate_information_request(note)?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkMembership {
    pub network_id: NetworkId,
    pub node_id: NodeId,
    pub state: MembershipState,
    pub role: MembershipRole,
    pub local_policy_revision: u64,
    pub terms: Option<MembershipTerms>,
    pub terms_ack: Option<JoinTermsAck>,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    pub suspension_reason: Option<String>,
}

impl NetworkMembership {
    pub fn new(
        network_id: NetworkId,
        node_id: NodeId,
        role: MembershipRole,
        now_unix_ms: u64,
    ) -> Self {
        Self {
            network_id,
            node_id,
            state: MembershipState::Discovered,
            role,
            local_policy_revision: 1,
            terms: None,
            terms_ack: None,
            created_at_unix_ms: now_unix_ms,
            updated_at_unix_ms: now_unix_ms,
            suspension_reason: None,
        }
    }

    /// Capture the mandatory disclosure and move to `applying`. The caller must
    /// pass proof that this exact terms document was shown to the owner.
    pub fn begin_apply(
        &mut self,
        terms: MembershipTerms,
        ack: JoinTermsAck,
        now_unix_ms: u64,
    ) -> DomainResult<()> {
        terms.validate()?;
        if ack.terms_hash != terms.content_hash()? {
            return Err(DomainError::new(
                "domain.terms_not_acknowledged",
                "join terms acknowledgement does not match the displayed terms",
            ));
        }
        self.ensure_transition(MembershipState::Applying)?;
        self.terms = Some(terms);
        self.terms_ack = Some(ack);
        self.transition_to(MembershipState::Applying, now_unix_ms)
    }

    pub fn require_approval(&mut self, now_unix_ms: u64) -> DomainResult<()> {
        self.transition_to(MembershipState::PendingApproval, now_unix_ms)
    }

    pub fn accept(&mut self, now_unix_ms: u64) -> DomainResult<()> {
        self.transition_to(MembershipState::Accepted, now_unix_ms)
    }

    pub fn reject(&mut self, now_unix_ms: u64) -> DomainResult<()> {
        self.transition_to(MembershipState::Rejected, now_unix_ms)
    }

    pub fn cancel(&mut self, now_unix_ms: u64) -> DomainResult<()> {
        self.transition_to(MembershipState::Cancelled, now_unix_ms)
    }

    pub fn mark_key_ready(&mut self, now_unix_ms: u64) -> DomainResult<()> {
        if self.terms_ack.is_none() {
            return Err(DomainError::new(
                "domain.terms_not_acknowledged",
                "membership has no accepted join terms",
            ));
        }
        self.transition_to(MembershipState::KeyReady, now_unix_ms)
    }

    pub fn transition_to(&mut self, target: MembershipState, now_unix_ms: u64) -> DomainResult<()> {
        self.ensure_transition(target)?;
        if now_unix_ms < self.updated_at_unix_ms {
            return Err(DomainError::invariant(
                "membership update time moved backwards",
            ));
        }
        self.state = target;
        self.updated_at_unix_ms = now_unix_ms;
        if !matches!(target, MembershipState::Suspended) {
            self.suspension_reason = None;
        }
        Ok(())
    }

    pub fn suspend(&mut self, reason: impl Into<String>, now_unix_ms: u64) -> DomainResult<()> {
        let reason = reason.into();
        if reason.is_empty() || reason.len() > 256 {
            return Err(DomainError::invalid_value(
                "membership suspension reason is invalid",
            ));
        }
        self.transition_to(MembershipState::Suspended, now_unix_ms)?;
        self.suspension_reason = Some(reason);
        Ok(())
    }

    /// Runtime-only states do not survive a process restart as if their old
    /// connections still existed. Durable governance state is preserved.
    pub fn recover_after_restart(&mut self, now_unix_ms: u64) -> DomainResult<()> {
        let recovered = match self.state {
            MembershipState::Applying => MembershipState::Discovered,
            MembershipState::Connecting | MembershipState::Active | MembershipState::Degraded => {
                MembershipState::KeyReady
            }
            state => state,
        };
        if recovered != self.state {
            if now_unix_ms < self.updated_at_unix_ms {
                return Err(DomainError::invariant(
                    "membership recovery time moved backwards",
                ));
            }
            // Recovery is a process-boundary reconciliation, not a normal
            // protocol transition; old live connections are never trusted.
            self.state = recovered;
            self.updated_at_unix_ms = now_unix_ms;
            self.suspension_reason = None;
        }
        Ok(())
    }

    pub fn validate(&self) -> DomainResult<()> {
        if self.local_policy_revision == 0 {
            return Err(DomainError::invalid_value(
                "local membership policy revision must be positive",
            ));
        }
        if self.updated_at_unix_ms < self.created_at_unix_ms {
            return Err(DomainError::invariant(
                "membership update time predates creation",
            ));
        }
        if self.state != MembershipState::Discovered && self.terms_ack.is_none() {
            return Err(DomainError::new(
                "domain.terms_not_acknowledged",
                "membership advanced without accepted join terms",
            ));
        }
        if let Some(terms) = &self.terms {
            terms.validate()?;
            if self.terms_ack.as_ref().map(|ack| &ack.terms_hash) != Some(&terms.content_hash()?) {
                return Err(DomainError::invariant(
                    "membership terms acknowledgement is inconsistent",
                ));
            }
        }
        Ok(())
    }

    fn ensure_transition(&self, target: MembershipState) -> DomainResult<()> {
        if !self.state.can_transition_to(target) {
            return Err(DomainError::new(
                "domain.invalid_membership_transition",
                format!(
                    "membership cannot transition from {:?} to {:?}",
                    self.state, target
                ),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_membership() -> NetworkMembership {
        NetworkMembership::new(
            NetworkId::from_public_key(b"network"),
            NodeId::from_public_key(b"node"),
            MembershipRole::Member,
            10,
        )
    }

    fn terms() -> MembershipTerms {
        MembershipTerms {
            network_policy_revision: 1,
            privacy_policy_revision: 1,
            exit_contribution: ExitContributionObligation::Required,
            member_limit: None,
        }
    }

    fn ack(terms: &MembershipTerms) -> JoinTermsAck {
        JoinTermsAck {
            terms_hash: terms.content_hash().unwrap(),
            acknowledged_at_unix_ms: 10,
        }
    }

    fn applicant(seed: &str) -> NodeId {
        NodeId::from_public_key(seed.as_bytes())
    }

    fn join_request(now: u64) -> JoinRequest {
        let network_id = NetworkId::from_public_key(b"approval-network");
        let applicant = applicant("applicant");
        JoinRequest::new(
            JoinRequest::derived_id(&network_id, &applicant, "retry-nonce"),
            network_id,
            JoinApplicantProfile {
                node_id: applicant,
                display_name: Some("Applicant".into()),
                public_key_fingerprint: "fingerprint".into(),
            },
            "Need access for the research project",
            terms(),
            JoinRequestGrantPreview {
                role: MembershipRole::Member,
                capabilities: vec![CapabilityKind::File],
                exit_contribution: ExitContributionObligation::Required,
                file_access: true,
                grants_data_plane_access: true,
            },
            now,
            now + JOIN_REQUEST_DEFAULT_EXPIRY_MS,
        )
        .unwrap()
    }

    fn assert_terminal_reason_retention(mut request: JoinRequest) {
        let deadline = request
            .reason_retention_until_unix_ms
            .expect("terminal request must have a retention deadline");
        assert!(!request.scrub_expired_reason(deadline - 1));
        assert!(request.reason.is_some());
        assert!(request.scrub_expired_reason(deadline));
        assert!(request.reason.is_none());
        assert!(request.information_request.is_none());
        request.validate().unwrap();
    }

    #[test]
    fn join_request_reason_limits_count_unicode_scalars_and_reject_control_characters() {
        validate_join_request_reason(&"界".repeat(JOIN_REQUEST_MAX_REASON_CHARS)).unwrap();
        assert!(
            validate_join_request_reason(&"界".repeat(JOIN_REQUEST_MAX_REASON_CHARS + 1)).is_err()
        );
        assert!(validate_join_request_reason("").is_err());
        assert!(validate_join_request_reason(" padded ").is_err());
        assert!(validate_join_request_reason("line\nbreak").is_err());
        assert!(validate_join_request_reason("  \t  ").is_err());
    }

    #[test]
    fn request_id_is_deterministic_for_idempotent_retries() {
        let network_id = NetworkId::from_public_key(b"approval-network");
        let applicant = applicant("applicant");
        assert_eq!(
            JoinRequest::derived_id(&network_id, &applicant, "nonce"),
            JoinRequest::derived_id(&network_id, &applicant, "nonce")
        );
        assert_ne!(
            JoinRequest::derived_id(&network_id, &applicant, "nonce"),
            JoinRequest::derived_id(&network_id, &applicant, "other")
        );
    }

    #[test]
    fn approval_is_revision_checked_and_terminal_requests_cannot_be_replayed() {
        let mut request = join_request(100);
        let reviewer = applicant("reviewer");
        request
            .review(
                1,
                JoinRequestReviewAction::Approve,
                reviewer.clone(),
                None,
                110,
            )
            .unwrap();
        assert_eq!(request.state, JoinRequestState::Approved);
        assert_eq!(request.revision, 2);
        assert_eq!(request.reviewer_node_id, Some(reviewer.clone()));
        assert_eq!(request.decided_at_unix_ms, Some(110));
        assert_eq!(
            request.reason_retention_until_unix_ms,
            Some(110 + JOIN_REQUEST_REASON_RETENTION_MS)
        );
        assert_eq!(
            request
                .review(1, JoinRequestReviewAction::Reject, reviewer, None, 120)
                .unwrap_err()
                .code(),
            "domain.join_request_revision_conflict"
        );
        assert_eq!(
            request
                .review(
                    2,
                    JoinRequestReviewAction::Reject,
                    applicant("other"),
                    None,
                    120
                )
                .unwrap_err()
                .code(),
            "domain.invalid_join_request_transition"
        );
    }

    #[test]
    fn stale_concurrent_review_allows_exactly_one_terminal_decision() {
        let mut request = join_request(100);
        request
            .review(
                1,
                JoinRequestReviewAction::Approve,
                applicant("admin-a"),
                None,
                110,
            )
            .unwrap();
        assert_eq!(request.state, JoinRequestState::Approved);
        assert_eq!(
            request
                .review(
                    1,
                    JoinRequestReviewAction::Approve,
                    applicant("admin-b"),
                    None,
                    111,
                )
                .unwrap_err()
                .code(),
            "domain.join_request_revision_conflict"
        );
        assert_eq!(request.state, JoinRequestState::Approved);
        assert_eq!(request.revision, 2);
    }

    #[test]
    fn expired_request_cannot_later_be_approved() {
        let mut request = join_request(100);
        let expires = request.expires_at_unix_ms;
        assert!(!request.expire_if_due(expires - 1).unwrap());
        assert!(request.expire_if_due(expires).unwrap());
        assert_eq!(request.state, JoinRequestState::Expired);
        assert_eq!(request.revision, 2);
        assert_eq!(
            request
                .review(
                    2,
                    JoinRequestReviewAction::Approve,
                    applicant("admin"),
                    None,
                    expires + 1,
                )
                .unwrap_err()
                .code(),
            "domain.invalid_join_request_transition"
        );
    }

    #[test]
    fn only_the_applicant_can_withdraw_a_request() {
        let mut request = join_request(100);
        assert_eq!(
            request
                .withdraw(&applicant("someone-else"), 110)
                .unwrap_err()
                .code(),
            "domain.join_request_forbidden"
        );
        request.withdraw(&applicant("applicant"), 110).unwrap();
        assert_eq!(request.state, JoinRequestState::Withdrawn);
        assert_eq!(request.decided_at_unix_ms, Some(110));
    }

    #[test]
    fn information_request_requires_bounded_note_and_resubmission_advances_revision() {
        let mut request = join_request(100);
        assert!(request
            .review(
                1,
                JoinRequestReviewAction::RequestInformation,
                applicant("admin"),
                None,
                110,
            )
            .is_err());
        assert!(request
            .review(
                1,
                JoinRequestReviewAction::RequestInformation,
                applicant("admin"),
                Some("x".repeat(JOIN_REQUEST_MAX_INFORMATION_REQUEST_CHARS + 1)),
                110,
            )
            .is_err());
        request
            .review(
                1,
                JoinRequestReviewAction::RequestInformation,
                applicant("admin"),
                Some("Please describe the devices you need to reach.".into()),
                110,
            )
            .unwrap();
        assert_eq!(request.state, JoinRequestState::NeedsInformation);
        assert_eq!(request.revision, 2);
        request
            .resubmit(2, "Updated research project scope", 120)
            .unwrap();
        assert_eq!(request.state, JoinRequestState::Pending);
        assert_eq!(request.revision, 3);
        assert_eq!(
            request.reason.as_deref(),
            Some("Updated research project scope")
        );
        assert!(request.information_request.is_none());
        assert!(request.reviewer_node_id.is_none());
    }

    #[test]
    fn terminal_reason_is_retained_then_scrubbed_at_retention_deadline() {
        let mut approved = join_request(100);
        approved
            .review(
                1,
                JoinRequestReviewAction::Approve,
                applicant("admin"),
                None,
                110,
            )
            .unwrap();
        assert_terminal_reason_retention(approved);

        let mut rejected = join_request(100);
        rejected
            .review(
                1,
                JoinRequestReviewAction::Reject,
                applicant("admin"),
                Some("Does not meet the current policy".into()),
                110,
            )
            .unwrap();
        assert_terminal_reason_retention(rejected);

        let mut withdrawn = join_request(100);
        withdrawn.withdraw(&applicant("applicant"), 110).unwrap();
        assert_terminal_reason_retention(withdrawn);

        let mut expired = join_request(100);
        let expires_at = expired.expires_at_unix_ms;
        assert!(expired.expire_if_due(expires_at).unwrap());
        assert_terminal_reason_retention(expired);
    }

    #[test]
    fn debug_output_redacts_request_prose() {
        let mut request = join_request(100);
        request
            .review(
                1,
                JoinRequestReviewAction::RequestInformation,
                applicant("admin"),
                Some("private reviewer note".into()),
                110,
            )
            .unwrap();
        let debug = format!("{request:?}");
        assert!(!debug.contains("Need access for the research project"));
        assert!(!debug.contains("private reviewer note"));
        assert!(debug.contains("[redacted]"));
    }

    #[test]
    fn cannot_apply_without_exact_terms_acknowledgement() {
        let mut membership = new_membership();
        let terms = terms();
        let wrong = JoinTermsAck {
            terms_hash: ContentHash::from_bytes(b"other"),
            acknowledged_at_unix_ms: 10,
        };
        assert_eq!(
            membership
                .begin_apply(terms.clone(), wrong, 11)
                .unwrap_err()
                .code(),
            "domain.terms_not_acknowledged"
        );
        membership
            .begin_apply(terms.clone(), ack(&terms), 11)
            .unwrap();
        assert_eq!(membership.state, MembershipState::Applying);
    }

    #[test]
    fn accepted_does_not_grant_data_plane_access() {
        let mut membership = new_membership();
        let terms = terms();
        membership
            .begin_apply(terms.clone(), ack(&terms), 11)
            .unwrap();
        membership.accept(12).unwrap();
        assert!(!membership.state.can_access_network_resources());
        membership.mark_key_ready(13).unwrap();
        membership
            .transition_to(MembershipState::Connecting, 14)
            .unwrap();
        membership
            .transition_to(MembershipState::Active, 15)
            .unwrap();
        assert!(membership.state.can_access_network_resources());
        membership.validate().unwrap();
    }

    #[test]
    fn illegal_transitions_and_late_acknowledgements_fail_closed() {
        let mut membership = new_membership();
        assert_eq!(
            membership
                .transition_to(MembershipState::Active, 11)
                .unwrap_err()
                .code(),
            "domain.invalid_membership_transition"
        );
        let terms = terms();
        membership
            .begin_apply(terms.clone(), ack(&terms), 11)
            .unwrap();
        assert!(membership.accept(12).is_ok());

        let mut membership = new_membership();
        membership
            .begin_apply(terms.clone(), ack(&terms), 11)
            .unwrap();
        assert!(membership.mark_key_ready(12).is_err());
    }

    #[test]
    fn restart_recovery_does_not_claim_an_old_connection_is_active() {
        let mut membership = new_membership();
        let terms = terms();
        membership
            .begin_apply(terms.clone(), ack(&terms), 11)
            .unwrap();
        membership.accept(12).unwrap();
        membership.mark_key_ready(13).unwrap();
        membership
            .transition_to(MembershipState::Connecting, 14)
            .unwrap();
        membership
            .transition_to(MembershipState::Active, 15)
            .unwrap();
        membership.recover_after_restart(16).unwrap();
        assert_eq!(membership.state, MembershipState::KeyReady);
        assert!(!membership.state.can_access_network_resources());
    }

    #[test]
    fn suspension_requires_a_reason_and_can_reactivate() {
        let mut membership = new_membership();
        let terms = terms();
        membership
            .begin_apply(terms.clone(), ack(&terms), 11)
            .unwrap();
        membership.accept(12).unwrap();
        membership.mark_key_ready(13).unwrap();
        membership
            .transition_to(MembershipState::Connecting, 14)
            .unwrap();
        membership
            .transition_to(MembershipState::Active, 15)
            .unwrap();
        assert!(membership.suspend("", 16).is_err());
        membership.suspend("policy review", 16).unwrap();
        assert_eq!(
            membership.suspension_reason.as_deref(),
            Some("policy review")
        );
        membership
            .transition_to(MembershipState::Active, 17)
            .unwrap();
        assert!(membership.suspension_reason.is_none());
    }
}
