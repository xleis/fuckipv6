use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::crypto::Identity;

use super::ids::{AdminRecoveryRequestId, ContentHash, NetworkId, NodeId, PublicKey, Signature};
use super::network::NetworkRecord;
use super::{DomainError, DomainResult};

pub const ADMIN_RECOVERY_SCHEMA_VERSION: u16 = 1;
pub const ADMIN_RECOVERY_MAX_VALIDITY_MS: u64 = 7 * 24 * 60 * 60 * 1000;
pub const ADMIN_RECOVERY_MAX_APPROVALS: usize = 64;
pub const ADMIN_RECOVERY_MAX_NONCE_CHARS: usize = 128;

const ADMIN_RECOVERY_SIGNING_DOMAIN: &[u8] = b"fuckipv6/admin-recovery/v1\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminRecoveryReason {
    LostCredentials,
    CompromisedAuthority,
    DisasterRecovery,
    Other,
}

/// A complete roster snapshot. The revision is displayed to operators, while
/// the digest makes membership-only changes stale even though roster edits do
/// not advance the signed public-profile revision.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct AdminRecoverySnapshot {
    pub network_revision: u64,
    pub owner: NodeId,
    pub administrators: Vec<NodeId>,
    pub members: Vec<NodeId>,
    pub roster_digest: ContentHash,
}

impl AdminRecoverySnapshot {
    pub fn from_network(network: &NetworkRecord) -> Self {
        let mut administrators = network.administrators.clone();
        let mut members = network.members.clone();
        administrators.sort();
        members.sort();
        let mut snapshot = Self {
            network_revision: network.revision,
            owner: network.owner.clone(),
            administrators,
            members,
            roster_digest: ContentHash::from_bytes(b"pending"),
        };
        snapshot.roster_digest = snapshot.compute_digest();
        snapshot
    }

    fn payload(&self) -> AdminRecoverySnapshotPayload<'_> {
        AdminRecoverySnapshotPayload {
            network_revision: self.network_revision,
            owner: &self.owner,
            administrators: &self.administrators,
            members: &self.members,
        }
    }

    fn compute_digest(&self) -> ContentHash {
        ContentHash::from_bytes(&serde_json::to_vec(&self.payload()).unwrap_or_default())
    }

    pub fn validate(&self) -> DomainResult<()> {
        if self.network_revision == 0 {
            return Err(DomainError::invalid_value(
                "admin recovery snapshot revision must be positive",
            ));
        }
        if self.roster_digest != self.compute_digest() {
            return Err(DomainError::new(
                "domain.admin_recovery_snapshot_digest_mismatch",
                "admin recovery snapshot digest does not match its roster",
            ));
        }
        validate_authority_sets(&self.owner, &self.administrators, &self.members)?;
        Ok(())
    }

    pub fn matches_network(&self, network: &NetworkRecord) -> bool {
        self == &Self::from_network(network)
    }

    pub fn authority_count(&self) -> usize {
        1 + self.administrators.len()
    }

    pub fn authority_ids(&self) -> BTreeSet<NodeId> {
        let mut authorities = self.administrators.iter().cloned().collect::<BTreeSet<_>>();
        authorities.insert(self.owner.clone());
        authorities
    }
}

#[derive(Serialize)]
struct AdminRecoverySnapshotPayload<'a> {
    network_revision: u64,
    owner: &'a NodeId,
    administrators: &'a [NodeId],
    members: &'a [NodeId],
}

/// Exact post-recovery roster shown before approval. The target must remain a
/// valid network record; recovery changes governance, never network identity.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct AdminRecoveryTarget {
    pub owner: NodeId,
    #[serde(default)]
    pub administrators: Vec<NodeId>,
    pub members: Vec<NodeId>,
}

impl AdminRecoveryTarget {
    /// Replace the owner and administrators while retaining every current
    /// member and adding any new authority identities.
    pub fn replace_authorities(
        network: &NetworkRecord,
        owner: NodeId,
        administrators: Vec<NodeId>,
    ) -> Self {
        let mut members = network.members.iter().cloned().collect::<BTreeSet<_>>();
        members.insert(owner.clone());
        members.extend(administrators.iter().cloned());
        let mut administrators = administrators;
        administrators.sort();
        administrators.dedup();
        Self {
            owner,
            administrators,
            members: members.into_iter().collect(),
        }
    }

    pub fn validate(&self) -> DomainResult<()> {
        validate_authority_sets(&self.owner, &self.administrators, &self.members)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct AdminRecoveryRevocationPlan {
    /// Revoke every session that was live when the snapshot was approved.
    pub old_sessions: bool,
    /// Revoke every active or exhausted invitation issued under the old roster.
    pub pending_invitations: bool,
    /// Cancel every pending or information-requested join request.
    pub unconfirmed_writes: bool,
}

impl Default for AdminRecoveryRevocationPlan {
    fn default() -> Self {
        Self {
            old_sessions: true,
            pending_invitations: true,
            unconfirmed_writes: true,
        }
    }
}

impl AdminRecoveryRevocationPlan {
    fn validate(&self) -> DomainResult<()> {
        if !self.old_sessions || !self.pending_invitations || !self.unconfirmed_writes {
            return Err(DomainError::new(
                "domain.admin_recovery_incomplete_revocation",
                "admin recovery must revoke old sessions, invitations and unconfirmed writes",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct AdminRecoveryProposal {
    pub schema_version: u16,
    pub request_id: AdminRecoveryRequestId,
    pub network_id: NetworkId,
    pub snapshot: AdminRecoverySnapshot,
    pub target: AdminRecoveryTarget,
    pub initiated_by: NodeId,
    pub reason: AdminRecoveryReason,
    pub nonce: String,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    #[serde(default)]
    pub revocation: AdminRecoveryRevocationPlan,
}

impl AdminRecoveryProposal {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        network: &NetworkRecord,
        target: AdminRecoveryTarget,
        initiated_by: NodeId,
        reason: AdminRecoveryReason,
        nonce: impl Into<String>,
        issued_at_unix_ms: u64,
        expires_at_unix_ms: u64,
        revocation: AdminRecoveryRevocationPlan,
    ) -> DomainResult<Self> {
        network.validate()?;
        target.validate()?;
        let snapshot = AdminRecoverySnapshot::from_network(network);
        let mut proposal = Self {
            schema_version: ADMIN_RECOVERY_SCHEMA_VERSION,
            request_id: AdminRecoveryRequestId::from_public_key(b"pending"),
            network_id: network.network_id.clone(),
            snapshot,
            target,
            initiated_by,
            reason,
            nonce: nonce.into(),
            issued_at_unix_ms,
            expires_at_unix_ms,
            revocation,
        };
        proposal.request_id = proposal.derive_request_id()?;
        proposal.validate_structure()?;
        Ok(proposal)
    }

    fn derive_request_id(&self) -> DomainResult<AdminRecoveryRequestId> {
        let bytes = serde_json::to_vec(&self.request_id_payload()).map_err(|error| {
            DomainError::invalid_value(format!(
                "admin recovery proposal is not serializable: {error}"
            ))
        })?;
        Ok(AdminRecoveryRequestId::from_public_key(&bytes))
    }

    fn request_id_payload(&self) -> AdminRecoveryRequestIdPayload<'_> {
        AdminRecoveryRequestIdPayload {
            schema_version: self.schema_version,
            network_id: &self.network_id,
            snapshot: &self.snapshot,
            target: &self.target,
            initiated_by: &self.initiated_by,
            reason: self.reason,
            nonce: &self.nonce,
            issued_at_unix_ms: self.issued_at_unix_ms,
            expires_at_unix_ms: self.expires_at_unix_ms,
            revocation: &self.revocation,
        }
    }

    fn signing_payload_value(&self) -> AdminRecoverySigningPayload<'_> {
        AdminRecoverySigningPayload {
            schema_version: self.schema_version,
            request_id: &self.request_id,
            network_id: &self.network_id,
            snapshot: &self.snapshot,
            target: &self.target,
            initiated_by: &self.initiated_by,
            reason: self.reason,
            nonce: &self.nonce,
            issued_at_unix_ms: self.issued_at_unix_ms,
            expires_at_unix_ms: self.expires_at_unix_ms,
            revocation: &self.revocation,
        }
    }

    /// Domain-separated canonical JSON covered by every recovery approval.
    pub fn signing_payload(&self) -> DomainResult<Vec<u8>> {
        let encoded = serde_json::to_vec(&self.signing_payload_value()).map_err(|error| {
            DomainError::invalid_value(format!(
                "admin recovery proposal is not serializable: {error}"
            ))
        })?;
        let mut payload = Vec::with_capacity(ADMIN_RECOVERY_SIGNING_DOMAIN.len() + encoded.len());
        payload.extend_from_slice(ADMIN_RECOVERY_SIGNING_DOMAIN);
        payload.extend_from_slice(&encoded);
        Ok(payload)
    }

    pub fn validate_structure(&self) -> DomainResult<()> {
        if self.schema_version != ADMIN_RECOVERY_SCHEMA_VERSION {
            return Err(DomainError::invalid_value(
                "unsupported admin recovery schema version",
            ));
        }
        if self.request_id != self.derive_request_id()? {
            return Err(DomainError::new(
                "domain.admin_recovery_request_id_mismatch",
                "admin recovery request id does not match its contents",
            ));
        }
        if self.nonce.is_empty()
            || self.nonce.chars().count() > ADMIN_RECOVERY_MAX_NONCE_CHARS
            || self.nonce.trim() != self.nonce
            || self.nonce.chars().any(char::is_control)
        {
            return Err(DomainError::invalid_value(
                "admin recovery nonce is invalid",
            ));
        }
        if self.issued_at_unix_ms == 0 || self.issued_at_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::invalid_value(
                "admin recovery validity interval is empty",
            ));
        }
        if self.expires_at_unix_ms - self.issued_at_unix_ms > ADMIN_RECOVERY_MAX_VALIDITY_MS {
            return Err(DomainError::invalid_value(
                "admin recovery validity exceeds the maximum window",
            ));
        }
        self.snapshot.validate()?;
        self.target.validate()?;
        self.revocation.validate()?;
        Ok(())
    }

    pub fn is_expired(&self, now_unix_ms: u64) -> bool {
        now_unix_ms >= self.expires_at_unix_ms
    }

    pub fn request_digest(&self) -> ContentHash {
        ContentHash::from_bytes(self.request_id.as_str().as_bytes())
    }

    pub fn required_approvals(&self, network: &NetworkRecord) -> u32 {
        if network.recovery_authorities.is_empty() {
            (self.snapshot.authority_count() as u32 / 2) + 1
        } else {
            network.recovery_threshold
        }
    }
}

#[derive(Serialize)]
struct AdminRecoveryRequestIdPayload<'a> {
    schema_version: u16,
    network_id: &'a NetworkId,
    snapshot: &'a AdminRecoverySnapshot,
    target: &'a AdminRecoveryTarget,
    initiated_by: &'a NodeId,
    reason: AdminRecoveryReason,
    nonce: &'a str,
    issued_at_unix_ms: u64,
    expires_at_unix_ms: u64,
    revocation: &'a AdminRecoveryRevocationPlan,
}

#[derive(Serialize)]
struct AdminRecoverySigningPayload<'a> {
    schema_version: u16,
    request_id: &'a AdminRecoveryRequestId,
    network_id: &'a NetworkId,
    snapshot: &'a AdminRecoverySnapshot,
    target: &'a AdminRecoveryTarget,
    initiated_by: &'a NodeId,
    reason: AdminRecoveryReason,
    nonce: &'a str,
    issued_at_unix_ms: u64,
    expires_at_unix_ms: u64,
    revocation: &'a AdminRecoveryRevocationPlan,
}

/// One independently verifiable approval. It carries the public key so an
/// offline trustee does not need to be a network member.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct AdminRecoveryApproval {
    pub signer: NodeId,
    pub signer_public_key: PublicKey,
    pub signed_at_unix_ms: u64,
    pub signature: Signature,
}

impl AdminRecoveryApproval {
    pub fn signed(
        proposal: &AdminRecoveryProposal,
        identity: &Identity,
        signed_at_unix_ms: u64,
    ) -> DomainResult<Self> {
        proposal.validate_structure()?;
        if signed_at_unix_ms < proposal.issued_at_unix_ms
            || signed_at_unix_ms >= proposal.expires_at_unix_ms
        {
            return Err(DomainError::invalid_value(
                "admin recovery approval time is outside the request window",
            ));
        }
        let public_key = PublicKey::from_bytes(identity.public_key_bytes());
        let mut approval = Self {
            signer: NodeId::from_public_key(&identity.public_key()),
            signer_public_key: public_key,
            signed_at_unix_ms,
            signature: Signature::from_bytes([0; 64]),
        };
        let signature = identity.sign(&proposal.signing_payload()?);
        approval.signature = Signature::from_bytes(signature.to_bytes());
        approval.verify(proposal)?;
        Ok(approval)
    }

    pub fn verify(&self, proposal: &AdminRecoveryProposal) -> DomainResult<()> {
        if self.signed_at_unix_ms < proposal.issued_at_unix_ms
            || self.signed_at_unix_ms >= proposal.expires_at_unix_ms
        {
            return Err(DomainError::invalid_value(
                "admin recovery approval time is outside the request window",
            ));
        }
        let public_key = self.signer_public_key.to_bytes()?;
        if NodeId::from_public_key(&public_key) != self.signer {
            return Err(DomainError::new(
                "domain.admin_recovery_signer_mismatch",
                "admin recovery signer does not match its public key",
            ));
        }
        if !Identity::verify(
            &public_key,
            &proposal.signing_payload()?,
            &self.signature.to_bytes()?,
        ) {
            return Err(DomainError::new(
                "domain.invalid_signature",
                "admin recovery approval signature is invalid",
            ));
        }
        Ok(())
    }

    pub fn is_current_authority(&self, snapshot: &AdminRecoverySnapshot) -> bool {
        self.signer == snapshot.owner || snapshot.administrators.contains(&self.signer)
    }

    pub fn is_offline_trustee(&self, network: &NetworkRecord) -> bool {
        network
            .recovery_authorities
            .contains(&self.signer_public_key)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct AdminRecoveryRequest {
    pub proposal: AdminRecoveryProposal,
    #[serde(default)]
    pub approvals: Vec<AdminRecoveryApproval>,
}

impl AdminRecoveryRequest {
    pub fn new(proposal: AdminRecoveryProposal) -> Self {
        Self {
            proposal,
            approvals: Vec::new(),
        }
    }

    pub fn with_approvals(
        proposal: AdminRecoveryProposal,
        approvals: Vec<AdminRecoveryApproval>,
    ) -> Self {
        Self {
            proposal,
            approvals,
        }
    }

    pub fn add_approval(&mut self, approval: AdminRecoveryApproval) -> DomainResult<()> {
        if self
            .approvals
            .iter()
            .any(|existing| existing.signer == approval.signer)
        {
            return Err(DomainError::new(
                "domain.admin_recovery_duplicate_signer",
                "admin recovery approval signer is duplicated",
            ));
        }
        approval.verify(&self.proposal)?;
        self.approvals.push(approval);
        Ok(())
    }

    pub fn validate_structure(&self) -> DomainResult<()> {
        self.proposal.validate_structure()?;
        if self.approvals.len() > ADMIN_RECOVERY_MAX_APPROVALS {
            return Err(DomainError::invalid_value(
                "admin recovery approval count exceeds the maximum",
            ));
        }
        let mut signers = BTreeSet::new();
        let mut public_keys = BTreeSet::new();
        for approval in &self.approvals {
            approval.verify(&self.proposal)?;
            if !signers.insert(approval.signer.clone())
                || !public_keys.insert(approval.signer_public_key.clone())
            {
                return Err(DomainError::new(
                    "domain.admin_recovery_duplicate_signer",
                    "admin recovery approvals contain a duplicate signer",
                ));
            }
        }
        Ok(())
    }

    /// Validate signatures against the exact roster and recovery policy that
    /// was approved. Returns the number of unique trusted approvals.
    pub fn verify_for_network(
        &self,
        network: &NetworkRecord,
        now_unix_ms: u64,
    ) -> DomainResult<u32> {
        self.validate_structure()?;
        if self.proposal.network_id != network.network_id {
            return Err(DomainError::new(
                "domain.admin_recovery_network_mismatch",
                "admin recovery request belongs to another network",
            ));
        }
        if self.proposal.is_expired(now_unix_ms) {
            return Err(DomainError::new(
                "domain.admin_recovery_expired",
                "admin recovery request has expired",
            ));
        }
        if !self.proposal.snapshot.matches_network(network) {
            return Err(DomainError::new(
                "domain.admin_recovery_stale_snapshot",
                "admin recovery request no longer matches the authoritative roster",
            ));
        }
        let required = self.proposal.required_approvals(network);
        let trusted = self.trusted_approval_count(network);
        if trusted < required {
            return Err(DomainError::new(
                "domain.admin_recovery_threshold_not_met",
                "admin recovery does not have enough trusted approvals",
            ));
        }
        Ok(trusted)
    }

    /// Recovery policies are intentionally exclusive. Without an offline
    /// policy, only authorities from the approved roster count. With one, only
    /// the separately escrowed trustees count; normal governance identities
    /// cannot be mixed in to manufacture a recovery majority.
    pub fn trusted_approval_count(&self, network: &NetworkRecord) -> u32 {
        self.approvals
            .iter()
            .filter(|approval| {
                if network.recovery_authorities.is_empty() {
                    approval.is_current_authority(&self.proposal.snapshot)
                } else {
                    approval.is_offline_trustee(network)
                }
            })
            .count() as u32
    }

    pub fn apply_to_network(
        &self,
        network: &mut NetworkRecord,
        now_unix_ms: u64,
    ) -> DomainResult<()> {
        self.verify_for_network(network, now_unix_ms)?;
        let previous_revision = network.revision;
        network.owner = self.proposal.target.owner.clone();
        network.administrators = self.proposal.target.administrators.clone();
        network.members = self.proposal.target.members.clone();
        network.revision = previous_revision.checked_add(1).ok_or_else(|| {
            DomainError::invariant("network revision overflow during admin recovery")
        })?;
        network.updated_at_unix_ms = network.updated_at_unix_ms.max(now_unix_ms);
        network.signed_profile = None;
        network.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct AdminRecoveryImpact {
    pub owner_before: NodeId,
    pub owner_after: NodeId,
    pub administrators_added: Vec<NodeId>,
    pub administrators_removed: Vec<NodeId>,
    pub members_added: Vec<NodeId>,
    pub members_removed: Vec<NodeId>,
}

impl AdminRecoveryImpact {
    pub fn from_network(network: &NetworkRecord, target: &AdminRecoveryTarget) -> Self {
        let before_admins = network
            .administrators
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let after_admins = target
            .administrators
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let before_members = network.members.iter().cloned().collect::<BTreeSet<_>>();
        let after_members = target.members.iter().cloned().collect::<BTreeSet<_>>();
        Self {
            owner_before: network.owner.clone(),
            owner_after: target.owner.clone(),
            administrators_added: after_admins.difference(&before_admins).cloned().collect(),
            administrators_removed: before_admins.difference(&after_admins).cloned().collect(),
            members_added: after_members.difference(&before_members).cloned().collect(),
            members_removed: before_members.difference(&after_members).cloned().collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct AdminRecoveryExecution {
    pub request_id: AdminRecoveryRequestId,
    pub network_id: NetworkId,
    pub executed_at_unix_ms: u64,
    pub previous_revision: u64,
    pub new_revision: u64,
    pub previous_owner: NodeId,
    pub new_owner: NodeId,
    /// Sessions that existed under the approved snapshot must re-authenticate.
    pub invalidated_sessions: Vec<NodeId>,
    pub revoke_pending_invitations: bool,
    pub cancel_unconfirmed_writes: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminRecoveryState {
    Pending,
    Executed,
    Revoked,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct AdminRecoveryStatusView {
    pub request_id: AdminRecoveryRequestId,
    pub network_id: NetworkId,
    pub state: AdminRecoveryState,
    pub impact: AdminRecoveryImpact,
    pub initiator: NodeId,
    pub participants: Vec<NodeId>,
    pub signature_count: u32,
    pub required_signatures: u32,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    pub revocation: AdminRecoveryRevocationPlan,
    pub can_execute: bool,
    pub denial_key: Option<String>,
}

impl AdminRecoveryStatusView {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        request: &AdminRecoveryRequest,
        network: &NetworkRecord,
        state: AdminRecoveryState,
        now_unix_ms: u64,
    ) -> Self {
        let signatures = request.trusted_approval_count(network);
        let required = request.proposal.required_approvals(network);
        let denial_key = match state {
            AdminRecoveryState::Executed => Some("admin_recovery.error.already_executed".into()),
            AdminRecoveryState::Revoked => Some("admin_recovery.error.revoked".into()),
            AdminRecoveryState::Expired => Some("admin_recovery.error.expired".into()),
            AdminRecoveryState::Pending if request.proposal.is_expired(now_unix_ms) => {
                Some("admin_recovery.error.expired".into())
            }
            AdminRecoveryState::Pending if !request.proposal.snapshot.matches_network(network) => {
                Some("admin_recovery.error.stale_snapshot".into())
            }
            AdminRecoveryState::Pending if signatures < required => {
                Some("admin_recovery.error.threshold_not_met".into())
            }
            AdminRecoveryState::Pending => None,
        };
        let mut participants = request
            .approvals
            .iter()
            .map(|approval| approval.signer.clone())
            .collect::<Vec<_>>();
        participants.sort();
        participants.dedup();
        Self {
            request_id: request.proposal.request_id.clone(),
            network_id: request.proposal.network_id.clone(),
            state,
            impact: AdminRecoveryImpact::from_network(network, &request.proposal.target),
            initiator: request.proposal.initiated_by.clone(),
            participants,
            signature_count: signatures,
            required_signatures: required,
            issued_at_unix_ms: request.proposal.issued_at_unix_ms,
            expires_at_unix_ms: request.proposal.expires_at_unix_ms,
            revocation: request.proposal.revocation.clone(),
            can_execute: state == AdminRecoveryState::Pending && denial_key.is_none(),
            denial_key,
        }
    }
}

fn validate_authority_sets(
    owner: &NodeId,
    administrators: &[NodeId],
    members: &[NodeId],
) -> DomainResult<()> {
    let mut admin_set = BTreeSet::new();
    for administrator in administrators {
        if !admin_set.insert(administrator) {
            return Err(DomainError::invariant(
                "duplicate administrator in recovery roster",
            ));
        }
    }
    if admin_set.contains(owner) {
        return Err(DomainError::invariant(
            "recovery owner must not also be an administrator",
        ));
    }
    let mut member_set = BTreeSet::new();
    for member in members {
        if !member_set.insert(member) {
            return Err(DomainError::invariant(
                "duplicate member in recovery roster",
            ));
        }
    }
    if !member_set.contains(owner) || admin_set.iter().any(|admin| !member_set.contains(admin)) {
        return Err(DomainError::invariant(
            "recovery owner and administrators must be members",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        CapabilityKind, ExitContributionMode, ExitContributionObligation, ExitContributionScope,
        ExitSelectionPolicy, NetworkCreateProfile, NetworkJoinMode, NetworkVisibility,
    };

    fn profile() -> NetworkCreateProfile {
        NetworkCreateProfile {
            name: "Recovery".into(),
            purpose: "Recovery tests".into(),
            icon: "network".into(),
            language: "zh-CN".into(),
            region: "ZZ".into(),
            tags: vec![],
            capability_declarations: vec![CapabilityKind::File],
            visibility: NetworkVisibility::Private,
            join_mode: NetworkJoinMode::ApprovalRequired,
            exit_contribution: ExitContributionObligation::default(),
            exit_contribution_scope: ExitContributionScope::default(),
            exit_contribution_mode: ExitContributionMode::default(),
            exit_selection_policy: ExitSelectionPolicy::default(),
        }
    }

    fn network(owner: &Identity, administrators: Vec<NodeId>) -> NetworkRecord {
        let owner_id = NodeId::from_public_key(&owner.public_key());
        let mut network = NetworkRecord::create_owned(
            profile(),
            PublicKey::from_bytes([91; 32]),
            owner_id,
            1_000,
        )
        .unwrap();
        network.members.extend(administrators.clone());
        network.administrators = administrators;
        network
    }

    fn proposal(network: &NetworkRecord, replacement: NodeId) -> AdminRecoveryProposal {
        let target =
            AdminRecoveryTarget::replace_authorities(network, replacement.clone(), Vec::new());
        AdminRecoveryProposal::new(
            network,
            target,
            replacement,
            AdminRecoveryReason::LostCredentials,
            "offline-ceremony-1",
            1_100,
            1_200,
            AdminRecoveryRevocationPlan::default(),
        )
        .unwrap()
    }

    #[test]
    fn strict_majority_signatures_are_required_and_duplicates_are_rejected() {
        let owner = Identity::generate().unwrap();
        let admin_one = Identity::generate().unwrap();
        let admin_two = Identity::generate().unwrap();
        let admin_one_id = NodeId::from_public_key(&admin_one.public_key());
        let admin_two_id = NodeId::from_public_key(&admin_two.public_key());
        let network = network(&owner, vec![admin_one_id, admin_two_id]);
        let replacement = NodeId::from_public_key(b"replacement-owner");
        let proposal = proposal(&network, replacement);
        let first = AdminRecoveryApproval::signed(&proposal, &owner, 1_110).unwrap();
        let second = AdminRecoveryApproval::signed(&proposal, &admin_one, 1_111).unwrap();
        let mut request =
            AdminRecoveryRequest::with_approvals(proposal.clone(), vec![first.clone()]);
        assert_eq!(
            request
                .verify_for_network(&network, 1_120)
                .unwrap_err()
                .code(),
            "domain.admin_recovery_threshold_not_met"
        );
        request.add_approval(first.clone()).unwrap_err();
        request.add_approval(second).unwrap();
        assert_eq!(request.verify_for_network(&network, 1_120).unwrap(), 2);
    }

    #[test]
    fn offline_recovery_policy_can_authorize_without_live_admin_identities() {
        let owner = Identity::generate().unwrap();
        let trustee_one = Identity::generate().unwrap();
        let trustee_two = Identity::generate().unwrap();
        let trustee_three = Identity::generate().unwrap();
        let mut network = network(&owner, vec![]);
        network.recovery_authorities = vec![
            PublicKey::from_bytes(trustee_one.public_key_bytes()),
            PublicKey::from_bytes(trustee_two.public_key_bytes()),
            PublicKey::from_bytes(trustee_three.public_key_bytes()),
        ];
        network.recovery_threshold = 2;
        let replacement = NodeId::from_public_key(b"new-owner");
        let proposal = proposal(&network, replacement);
        let approvals = vec![
            AdminRecoveryApproval::signed(&proposal, &trustee_one, 1_110).unwrap(),
            AdminRecoveryApproval::signed(&proposal, &trustee_two, 1_111).unwrap(),
        ];
        let request = AdminRecoveryRequest::with_approvals(proposal, approvals);
        assert_eq!(request.verify_for_network(&network, 1_120).unwrap(), 2);
    }

    #[test]
    fn offline_trustees_cannot_be_mixed_with_live_governance_signatures() {
        let owner = Identity::generate().unwrap();
        let trustee_one = Identity::generate().unwrap();
        let trustee_two = Identity::generate().unwrap();
        let mut network = network(&owner, vec![]);
        network.recovery_authorities = vec![
            PublicKey::from_bytes(trustee_one.public_key_bytes()),
            PublicKey::from_bytes(trustee_two.public_key_bytes()),
        ];
        network.recovery_threshold = 2;
        let proposal = proposal(&network, NodeId::from_public_key(b"new-owner"));
        let approvals = vec![
            AdminRecoveryApproval::signed(&proposal, &owner, 1_110).unwrap(),
            AdminRecoveryApproval::signed(&proposal, &trustee_one, 1_111).unwrap(),
        ];
        let request = AdminRecoveryRequest::with_approvals(proposal, approvals);
        assert_eq!(request.trusted_approval_count(&network), 1);
        assert_eq!(
            request
                .verify_for_network(&network, 1_120)
                .unwrap_err()
                .code(),
            "domain.admin_recovery_threshold_not_met"
        );
    }

    #[test]
    fn forged_signatures_other_networks_and_stale_members_are_rejected() {
        let owner = Identity::generate().unwrap();
        let admin = Identity::generate().unwrap();
        let admin_id = NodeId::from_public_key(&admin.public_key());
        let mut network = network(&owner, vec![admin_id]);
        let replacement = NodeId::from_public_key(b"new-owner");
        let proposal = proposal(&network, replacement);
        let mut approval = AdminRecoveryApproval::signed(&proposal, &owner, 1_110).unwrap();
        approval.signature = Signature::from_bytes([7; 64]);
        let forged = AdminRecoveryRequest::with_approvals(proposal.clone(), vec![approval]);
        assert_eq!(
            forged
                .verify_for_network(&network, 1_120)
                .unwrap_err()
                .code(),
            "domain.invalid_signature"
        );

        let mut other_network = network.clone();
        other_network.network_id = NetworkId::from_public_key(b"other");
        assert!(forged.verify_for_network(&other_network, 1_120).is_err());

        let approval = AdminRecoveryApproval::signed(&proposal, &owner, 1_110).unwrap();
        let request = AdminRecoveryRequest::with_approvals(proposal, vec![approval]);
        network.members.push(NodeId::from_public_key(b"new-member"));
        assert_eq!(
            request
                .verify_for_network(&network, 1_120)
                .unwrap_err()
                .code(),
            "domain.admin_recovery_stale_snapshot"
        );
    }

    #[test]
    fn applying_recovery_rotates_the_roster_and_advances_revision() {
        let owner = Identity::generate().unwrap();
        let network = network(&owner, vec![]);
        let replacement = NodeId::from_public_key(b"new-owner");
        let proposal = proposal(&network, replacement.clone());
        let admin = Identity::generate().unwrap();
        let admin_id = NodeId::from_public_key(&admin.public_key());
        let target = AdminRecoveryTarget::replace_authorities(
            &network,
            replacement.clone(),
            vec![admin_id.clone()],
        );
        let mut proposal = proposal;
        proposal.target = target;
        proposal.request_id = proposal.derive_request_id().unwrap();
        let approval = AdminRecoveryApproval::signed(&proposal, &owner, 1_110).unwrap();
        let request = AdminRecoveryRequest::with_approvals(proposal, vec![approval]);
        let mut network = network;
        request.apply_to_network(&mut network, 1_120).unwrap();
        assert_eq!(network.owner, replacement);
        assert_eq!(network.administrators, vec![admin_id]);
        assert_eq!(network.revision, 2);
        assert!(network.signed_profile.is_none());
    }
}
