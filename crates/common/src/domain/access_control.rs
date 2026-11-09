//! Versioned governance ACLs, node-owner restrictions and their effective
//! authorization projection.
//!
//! This module is intentionally separate from the packet-filter ACL in
//! `crate::acl`. The packet filter decides traffic disposition; these types
//! decide whether a network principal may exercise a governance/capability
//! action on a node.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::audit::{AuditAppendError, AuditEntry, AuditLog, AuditOutcome};
use super::ids::{AuditId, ContentHash, NetworkId, NodeId, SecurityRuleId};
use super::permissions::{
    evaluate_permissions, AccessContext, PermissionCondition, PermissionDecision,
    PermissionDenialCode, PermissionEffect, PermissionIntent, PermissionRequest, PermissionRule,
    PolicyAction, ResourceKind, SubjectSelector,
};
use super::{DomainError, DomainResult};

/// Node-local contributions that the owner may veto independently of network
/// governance. Coordinator participation is deliberately not a local veto:
/// Coordinator terms are governed by the separate coordinator contract.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ContributionKind {
    Relay,
    Exit,
    File,
}

impl ContributionKind {
    pub const ALL: [Self; 3] = [Self::Relay, Self::Exit, Self::File];

    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Relay => "relay",
            Self::Exit => "exit",
            Self::File => "file",
        }
    }

    pub const fn resource_kind(self) -> ResourceKind {
        match self {
            Self::Relay => ResourceKind::Capability,
            Self::Exit => ResourceKind::ExitGrant,
            Self::File => ResourceKind::FileShare,
        }
    }
}

/// A node owner can either participate in every authenticated network or
/// explicitly limit participation to a fixed set. An empty `Only` set is
/// invalid rather than an accidental allow-all sentinel.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", content = "network_ids", rename_all = "snake_case")]
pub enum NodeNetworkScope {
    All,
    Only(Vec<NetworkId>),
}

impl NodeNetworkScope {
    pub fn validate(&self) -> DomainResult<()> {
        if let Self::Only(network_ids) = self {
            if network_ids.is_empty() {
                return Err(DomainError::invalid_value(
                    "node network scope cannot contain an empty allow-list",
                ));
            }
            let mut seen = BTreeSet::new();
            if network_ids
                .iter()
                .any(|network_id| !seen.insert(network_id))
            {
                return Err(DomainError::invariant(
                    "node network scope contains duplicate networks",
                ));
            }
        }
        Ok(())
    }

    pub fn allows(&self, network_id: &NetworkId) -> bool {
        match self {
            Self::All => true,
            Self::Only(network_ids) => network_ids.contains(network_id),
        }
    }
}

/// A non-overridable local contribution veto for one network.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeContributionVeto {
    pub veto_id: SecurityRuleId,
    pub network_id: NetworkId,
    pub contributions: Vec<ContributionKind>,
}

impl NodeContributionVeto {
    pub fn validate(&self) -> DomainResult<()> {
        if self.contributions.is_empty() {
            return Err(DomainError::invalid_value(
                "node contribution veto cannot be empty",
            ));
        }
        let mut seen = BTreeSet::new();
        if self
            .contributions
            .iter()
            .any(|contribution| !seen.insert(*contribution))
        {
            return Err(DomainError::invariant(
                "node contribution veto contains duplicates",
            ));
        }
        Ok(())
    }

    pub(crate) fn matches(
        &self,
        request: &PermissionRequest,
        network_id: &NetworkId,
    ) -> Option<ContributionKind> {
        if self.network_id != *network_id
            || request.action != PolicyAction::Use
            || request.intent != PermissionIntent::Mutating
        {
            return None;
        }
        self.contributions.iter().copied().find(|contribution| {
            contribution.resource_kind() == request.resource.kind
                && request.resource.network_id == self.network_id
        })
    }
}

/// The trusted, owner-authored local authorization layer. A network
/// administrator can add further restrictions but can never replace this
/// document or any of its vetoes.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeLocalAccessPolicy {
    pub node_id: NodeId,
    pub revision: u64,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    pub network_scope: NodeNetworkScope,
    #[serde(default)]
    pub contribution_vetoes: Vec<NodeContributionVeto>,
    /// Owner restrictions specific to serving as an Exit for a network.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_contribution: Option<super::exit_contribution::ExitContributionRestrictions>,
    #[serde(default)]
    pub rules: Vec<PermissionRule>,
}

impl NodeLocalAccessPolicy {
    pub fn validate_at(&self, now_unix_ms: u64) -> DomainResult<()> {
        self.validate_definition()?;
        if now_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::new(
                "domain.node_access_policy_expired",
                "node local access policy has expired",
            ));
        }
        Ok(())
    }

    fn validate_definition(&self) -> DomainResult<()> {
        if self.revision == 0 {
            return Err(DomainError::invalid_value(
                "node local access policy revision must be positive",
            ));
        }
        if self.issued_at_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::invariant(
                "node local access policy validity interval is empty",
            ));
        }
        self.network_scope.validate()?;
        if let Some(restrictions) = &self.exit_contribution {
            restrictions.validate()?;
        }
        let mut rule_ids = BTreeSet::new();
        for rule in &self.rules {
            rule.validate()?;
            if !rule_ids.insert(&rule.rule_id) {
                return Err(DomainError::invariant(
                    "duplicate node local access rule id",
                ));
            }
        }
        let mut veto_ids = BTreeSet::new();
        for veto in &self.contribution_vetoes {
            veto.validate()?;
            if !veto_ids.insert(&veto.veto_id) {
                return Err(DomainError::invariant(
                    "duplicate node contribution veto id",
                ));
            }
        }
        Ok(())
    }

    pub fn content_hash(&self) -> DomainResult<ContentHash> {
        content_hash(self, "node local access policy")
    }

    pub fn prepare_update(
        &self,
        candidate: NodeLocalAccessPolicy,
        now_unix_ms: u64,
    ) -> DomainResult<LocalAccessApplyPlan> {
        self.validate_at(now_unix_ms)?;
        candidate.validate_at(now_unix_ms)?;
        if candidate.node_id != self.node_id {
            return Err(DomainError::invariant(
                "node local access policy candidate belongs to another node",
            ));
        }
        if candidate.revision != self.revision.saturating_add(1) {
            return Err(revision_conflict(self.revision, candidate.revision));
        }
        Ok(LocalAccessApplyPlan {
            base_revision: self.revision,
            candidate_revision: candidate.revision,
            candidate_digest: candidate.content_hash()?,
            tested_at_unix_ms: now_unix_ms,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct LocalAccessApplyPlan {
    pub base_revision: u64,
    pub candidate_revision: u64,
    pub candidate_digest: ContentHash,
    pub tested_at_unix_ms: u64,
}

impl LocalAccessApplyPlan {
    pub fn validate_commit(
        &self,
        current: &NodeLocalAccessPolicy,
        candidate: &NodeLocalAccessPolicy,
        now_unix_ms: u64,
    ) -> DomainResult<()> {
        if current.revision != self.base_revision {
            return Err(revision_conflict(self.base_revision, current.revision));
        }
        if candidate.revision != self.candidate_revision
            || candidate.content_hash()? != self.candidate_digest
        {
            return Err(DomainError::new(
                "domain.access_control_changed_after_review",
                "node local access policy changed after it was reviewed",
            ));
        }
        candidate.validate_at(now_unix_ms)
    }
}

/// One explicit direct grant. Grants are additive allow entries; they never
/// override a matching deny in either authorization layer.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkAclGrant {
    pub grant_id: SecurityRuleId,
    pub subjects: Vec<SubjectSelector>,
    pub resources: Vec<ResourceKind>,
    pub actions: Vec<PolicyAction>,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_by: Option<NodeId>,
    #[serde(default)]
    pub requires_recent_reauthentication: bool,
}

impl NetworkAclGrant {
    pub fn validate(&self) -> DomainResult<()> {
        if self.subjects.is_empty() || self.resources.is_empty() || self.actions.is_empty() {
            return Err(DomainError::invalid_value(
                "network ACL grant selectors cannot be empty",
            ));
        }
        if has_duplicates(&self.subjects)
            || has_duplicates(&self.resources)
            || has_duplicates(&self.actions)
        {
            return Err(DomainError::invariant(
                "network ACL grant contains duplicate selectors",
            ));
        }
        if self.issued_at_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::invariant(
                "network ACL grant validity interval is empty",
            ));
        }
        match (self.revoked_at_unix_ms, self.revoked_by.as_ref()) {
            (Some(revoked_at), Some(_)) if revoked_at >= self.issued_at_unix_ms => {}
            (None, None) => {}
            _ => {
                return Err(DomainError::invariant(
                    "network ACL grant revocation metadata is incomplete",
                ));
            }
        }
        Ok(())
    }

    pub fn is_revoked(&self) -> bool {
        self.revoked_at_unix_ms.is_some()
    }

    pub fn is_expired(&self, now_unix_ms: u64) -> bool {
        now_unix_ms >= self.expires_at_unix_ms
    }

    pub fn is_active(&self, now_unix_ms: u64) -> bool {
        !self.is_revoked() && !self.is_expired(now_unix_ms)
    }

    pub(crate) fn matches(&self, request: &PermissionRequest, context: &AccessContext) -> bool {
        self.actions.contains(&request.action)
            && self.resources.contains(&request.resource.kind)
            && self.subjects.iter().any(|subject| subject.matches(context))
    }
}

/// Revisioned network ACL. Explicit deny rules live in `rules`; direct
/// role/group/node grants live in `grants` and are subject to expiry,
/// revocation and reauthentication freshness.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkAcl {
    pub network_id: NetworkId,
    pub revision: u64,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    #[serde(default)]
    pub rules: Vec<PermissionRule>,
    #[serde(default)]
    pub grants: Vec<NetworkAclGrant>,
}

impl NetworkAcl {
    pub fn validate_at(&self, now_unix_ms: u64) -> DomainResult<()> {
        self.validate_definition()?;
        if now_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::new(
                "domain.network_acl_expired",
                "network ACL has expired",
            ));
        }
        Ok(())
    }

    fn validate_definition(&self) -> DomainResult<()> {
        if self.revision == 0 {
            return Err(DomainError::invalid_value(
                "network ACL revision must be positive",
            ));
        }
        if self.issued_at_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::invariant(
                "network ACL validity interval is empty",
            ));
        }
        let mut rule_ids = BTreeSet::new();
        for rule in &self.rules {
            rule.validate()?;
            if !rule_ids.insert(&rule.rule_id) {
                return Err(DomainError::invariant("duplicate network ACL rule id"));
            }
            if rule.conditions.iter().any(|condition| {
                matches!(
                    condition,
                    PermissionCondition::ResourceInNetwork(id) if id != &self.network_id
                )
            }) {
                return Err(DomainError::invariant(
                    "network ACL rule references another network",
                ));
            }
        }
        for grant in &self.grants {
            grant.validate()?;
            if !rule_ids.insert(&grant.grant_id) {
                return Err(DomainError::invariant(
                    "duplicate network ACL rule or grant id",
                ));
            }
            if grant.issued_at_unix_ms < self.issued_at_unix_ms {
                return Err(DomainError::invariant(
                    "network ACL grant predates its containing revision",
                ));
            }
        }
        Ok(())
    }

    pub fn content_hash(&self) -> DomainResult<ContentHash> {
        content_hash(self, "network ACL")
    }

    pub fn prepare_update(
        &self,
        candidate: NetworkAcl,
        now_unix_ms: u64,
    ) -> DomainResult<NetworkAclApplyPlan> {
        self.validate_at(now_unix_ms)?;
        candidate.validate_at(now_unix_ms)?;
        if candidate.network_id != self.network_id {
            return Err(DomainError::invariant(
                "network ACL candidate belongs to another network",
            ));
        }
        if candidate.revision != self.revision.saturating_add(1) {
            return Err(revision_conflict(self.revision, candidate.revision));
        }
        Ok(NetworkAclApplyPlan {
            base_revision: self.revision,
            candidate_revision: candidate.revision,
            candidate_digest: candidate.content_hash()?,
            tested_at_unix_ms: now_unix_ms,
        })
    }

    fn grant(&self, grant_id: &SecurityRuleId) -> Option<&NetworkAclGrant> {
        self.grants.iter().find(|grant| &grant.grant_id == grant_id)
    }

    fn grant_mut(&mut self, grant_id: &SecurityRuleId) -> Option<&mut NetworkAclGrant> {
        self.grants
            .iter_mut()
            .find(|grant| &grant.grant_id == grant_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkAclApplyPlan {
    pub base_revision: u64,
    pub candidate_revision: u64,
    pub candidate_digest: ContentHash,
    pub tested_at_unix_ms: u64,
}

impl NetworkAclApplyPlan {
    pub fn validate_commit(
        &self,
        current: &NetworkAcl,
        candidate: &NetworkAcl,
        now_unix_ms: u64,
    ) -> DomainResult<()> {
        if current.revision != self.base_revision {
            return Err(revision_conflict(self.base_revision, current.revision));
        }
        if candidate.revision != self.candidate_revision
            || candidate.content_hash()? != self.candidate_digest
        {
            return Err(DomainError::new(
                "domain.access_control_changed_after_review",
                "network ACL changed after it was reviewed",
            ));
        }
        candidate.validate_at(now_unix_ms)
    }
}

/// In-memory authoritative ACL state used by domain tests and as the storage
/// contract for transports. Every successful mutation also appends an audit
/// record; stale expected revisions fail before either state or audit changes.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkAclState {
    current: NetworkAcl,
    audit: AuditLog,
}

impl NetworkAclState {
    pub fn new(current: NetworkAcl) -> DomainResult<Self> {
        current.validate_at(current.issued_at_unix_ms)?;
        Ok(Self {
            current,
            audit: AuditLog::default(),
        })
    }

    pub fn current(&self) -> &NetworkAcl {
        &self.current
    }

    pub fn audit_entries(&self) -> &[AuditEntry] {
        self.audit.entries()
    }

    pub fn apply_update(
        &mut self,
        actor: NodeId,
        expected_revision: u64,
        candidate: NetworkAcl,
        now_unix_ms: u64,
    ) -> DomainResult<AuditEntry> {
        if self.current.revision != expected_revision {
            return Err(revision_conflict(expected_revision, self.current.revision));
        }
        let plan = self
            .current
            .prepare_update(candidate.clone(), now_unix_ms)?;
        plan.validate_commit(&self.current, &candidate, now_unix_ms)?;
        let audit = network_acl_change_audit(
            &actor,
            &self.current,
            &candidate,
            "network_acl.updated",
            now_unix_ms,
        )?;
        append_audit(&mut self.audit, audit.clone())?;
        self.current = candidate;
        Ok(audit)
    }

    pub fn revoke_grant(
        &mut self,
        actor: NodeId,
        expected_revision: u64,
        grant_id: &SecurityRuleId,
        now_unix_ms: u64,
    ) -> DomainResult<AuditEntry> {
        if self.current.revision != expected_revision {
            return Err(revision_conflict(expected_revision, self.current.revision));
        }
        self.current.validate_at(now_unix_ms)?;
        if self.current.grant(grant_id).is_none() {
            return Err(DomainError::new(
                "domain.network_acl_grant_not_found",
                "network ACL grant does not exist",
            ));
        }
        let mut candidate = self.current.clone();
        candidate.revision = candidate.revision.saturating_add(1);
        let grant = candidate
            .grant_mut(grant_id)
            .ok_or_else(|| DomainError::invariant("network ACL grant disappeared"))?;
        if grant.revoked_at_unix_ms.is_some() {
            return Err(DomainError::new(
                "domain.network_acl_grant_already_revoked",
                "network ACL grant is already revoked",
            ));
        }
        grant.revoked_at_unix_ms = Some(now_unix_ms);
        grant.revoked_by = Some(actor.clone());
        candidate.validate_at(now_unix_ms)?;
        let audit = network_acl_change_audit(
            &actor,
            &self.current,
            &candidate,
            "network_acl.grant_revoked",
            now_unix_ms,
        )?;
        append_audit(&mut self.audit, audit.clone())?;
        self.current = candidate;
        Ok(audit)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeLocalAccessPolicyState {
    current: NodeLocalAccessPolicy,
    audit: AuditLog,
}

impl NodeLocalAccessPolicyState {
    pub fn new(current: NodeLocalAccessPolicy) -> DomainResult<Self> {
        current.validate_at(current.issued_at_unix_ms)?;
        Ok(Self {
            current,
            audit: AuditLog::default(),
        })
    }

    pub fn current(&self) -> &NodeLocalAccessPolicy {
        &self.current
    }

    pub fn audit_entries(&self) -> &[AuditEntry] {
        self.audit.entries()
    }

    pub fn apply_update(
        &mut self,
        actor: NodeId,
        expected_revision: u64,
        candidate: NodeLocalAccessPolicy,
        now_unix_ms: u64,
    ) -> DomainResult<AuditEntry> {
        if actor != self.current.node_id {
            return Err(DomainError::new(
                "domain.local_owner_required",
                "only the node owner may change local access restrictions",
            ));
        }
        if self.current.revision != expected_revision {
            return Err(revision_conflict(expected_revision, self.current.revision));
        }
        let plan = self
            .current
            .prepare_update(candidate.clone(), now_unix_ms)?;
        plan.validate_commit(&self.current, &candidate, now_unix_ms)?;
        let audit = local_policy_change_audit(&actor, &self.current, &candidate, now_unix_ms)?;
        append_audit(&mut self.audit, audit.clone())?;
        self.current = candidate;
        Ok(audit)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectivePolicyLayer {
    Local,
    Network,
    Request,
}

impl EffectivePolicyLayer {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Network => "network",
            Self::Request => "request",
        }
    }
}

/// Stable explanation of the final effect. UI surfaces consume this
/// projection instead of re-implementing rule precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveAccessReason {
    Allowed,
    CrossNetworkDenied,
    NetworkScopeRestricted,
    LocalContributionVeto,
    AuthorizationExpired,
    AuthorizationRevoked,
    RecentReauthenticationRequired,
    ExplicitDeny,
    RoleCeilingExceeded,
    TemporaryCoordinatorReadOnly,
    DefaultDenied,
    InvalidPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct EffectiveAccess {
    pub allowed: bool,
    pub code: Option<String>,
    pub reason: EffectiveAccessReason,
    pub layer: Option<EffectivePolicyLayer>,
    pub rule_id: Option<SecurityRuleId>,
    pub network_acl_revision: u64,
    pub local_policy_revision: u64,
    pub matched_allow_rule_ids: Vec<SecurityRuleId>,
    pub matched_deny_rule_ids: Vec<SecurityRuleId>,
    pub expired_grant_ids: Vec<SecurityRuleId>,
    pub revoked_grant_ids: Vec<SecurityRuleId>,
    pub reauthentication_required_grant_ids: Vec<SecurityRuleId>,
    pub reauthentication_satisfied: bool,
    pub evaluated_at_unix_ms: u64,
}

impl EffectiveAccess {
    pub fn explanation(&self, network_name: &str) -> String {
        if self.allowed {
            return format!("policy allows request: {network_name}");
        }
        let reason = match self.reason {
            EffectiveAccessReason::Allowed => "allowed",
            EffectiveAccessReason::CrossNetworkDenied => "cross_network_denied",
            EffectiveAccessReason::NetworkScopeRestricted => "network_scope_restricted",
            EffectiveAccessReason::LocalContributionVeto => "local_contribution_veto",
            EffectiveAccessReason::AuthorizationExpired => "authorization_expired",
            EffectiveAccessReason::AuthorizationRevoked => "authorization_revoked",
            EffectiveAccessReason::RecentReauthenticationRequired => {
                "recent_reauthentication_required"
            }
            EffectiveAccessReason::ExplicitDeny => "explicit_deny",
            EffectiveAccessReason::RoleCeilingExceeded => "role_ceiling_exceeded",
            EffectiveAccessReason::TemporaryCoordinatorReadOnly => {
                "temporary_coordinator_read_only"
            }
            EffectiveAccessReason::DefaultDenied => "default_denied",
            EffectiveAccessReason::InvalidPolicy => "invalid_policy",
        };
        let layer = self
            .layer
            .map(EffectivePolicyLayer::as_str)
            .unwrap_or("request");
        match &self.rule_id {
            Some(rule_id) => format!(
                "policy denied by {layer} layer rule {rule_id}: {network_name}; reason: {reason}"
            ),
            None => format!("policy denied by {layer} layer: {network_name}; reason: {reason}"),
        }
    }
}

/// Evaluate the final effect of baseline network policy, the versioned network
/// ACL, the trusted node-local policy and matching direct grants. Any explicit
/// deny wins; node-local restrictions then apply before network allows. Both
/// layers must allow.
pub fn evaluate_effective_access(
    local_rules: &[PermissionRule],
    network_rules: &[PermissionRule],
    network_acl: Option<&NetworkAcl>,
    local_access_policy: Option<&NodeLocalAccessPolicy>,
    request: &PermissionRequest,
    context: &AccessContext,
    now_unix_ms: u64,
) -> EffectiveAccess {
    let mut effective = EffectiveAccess {
        allowed: false,
        code: None,
        reason: EffectiveAccessReason::DefaultDenied,
        layer: None,
        rule_id: None,
        network_acl_revision: network_acl.map_or(0, |acl| acl.revision),
        local_policy_revision: local_access_policy.map_or(0, |policy| policy.revision),
        matched_allow_rule_ids: Vec::new(),
        matched_deny_rule_ids: Vec::new(),
        expired_grant_ids: Vec::new(),
        revoked_grant_ids: Vec::new(),
        reauthentication_required_grant_ids: Vec::new(),
        reauthentication_satisfied: context.recent_reauthentication,
        evaluated_at_unix_ms: now_unix_ms,
    };

    if request.resource.network_id != context.network_id {
        return denied_effective(
            effective,
            PermissionDenialCode::CrossNetworkDenied,
            EffectiveAccessReason::CrossNetworkDenied,
            EffectivePolicyLayer::Request,
            None,
        );
    }

    if let Some(acl) = network_acl {
        if acl.network_id != context.network_id {
            return denied_effective(
                effective,
                PermissionDenialCode::CrossNetworkDenied,
                EffectiveAccessReason::CrossNetworkDenied,
                EffectivePolicyLayer::Network,
                None,
            );
        }
        if let Err(error) = acl.validate_at(now_unix_ms) {
            let reason = if error.code() == "domain.network_acl_expired" {
                EffectiveAccessReason::AuthorizationExpired
            } else {
                EffectiveAccessReason::InvalidPolicy
            };
            return denied_effective(
                effective,
                PermissionDenialCode::PolicyDenied,
                reason,
                EffectivePolicyLayer::Network,
                None,
            );
        }
    }

    let local_policy_rules = local_access_policy.map_or(&[][..], |policy| policy.rules.as_slice());
    if let Some(policy) = local_access_policy {
        if policy.node_id != context.actor {
            return denied_effective(
                effective,
                PermissionDenialCode::PolicyDenied,
                EffectiveAccessReason::InvalidPolicy,
                EffectivePolicyLayer::Local,
                None,
            );
        }
        if let Err(error) = policy.validate_at(now_unix_ms) {
            let reason = if error.code() == "domain.node_access_policy_expired" {
                EffectiveAccessReason::AuthorizationExpired
            } else {
                EffectiveAccessReason::InvalidPolicy
            };
            return denied_effective(
                effective,
                PermissionDenialCode::PolicyDenied,
                reason,
                EffectivePolicyLayer::Local,
                None,
            );
        }
        if !policy.network_scope.allows(&context.network_id) {
            return denied_effective(
                effective,
                PermissionDenialCode::NetworkScopeDenied,
                EffectiveAccessReason::NetworkScopeRestricted,
                EffectivePolicyLayer::Local,
                None,
            );
        }
        if let Some((veto_id, _contribution)) = policy.contribution_vetoes.iter().find_map(|veto| {
            veto.matches(request, &context.network_id)
                .map(|kind| (&veto.veto_id, kind))
        }) {
            return denied_effective(
                effective,
                PermissionDenialCode::PolicyDenied,
                EffectiveAccessReason::LocalContributionVeto,
                EffectivePolicyLayer::Local,
                Some(veto_id.clone()),
            );
        }
    }

    let mut effective_local_rules = local_rules.to_vec();
    effective_local_rules.extend(local_policy_rules.iter().cloned());

    let mut effective_network_rules = network_rules.to_vec();
    if let Some(acl) = network_acl {
        effective_network_rules.extend(acl.rules.iter().cloned());
    }

    if let Some(acl) = network_acl {
        for grant in &acl.grants {
            if !grant.matches(request, context) {
                continue;
            }
            if grant.is_revoked() {
                effective.revoked_grant_ids.push(grant.grant_id.clone());
                continue;
            }
            if grant.is_expired(now_unix_ms) {
                effective.expired_grant_ids.push(grant.grant_id.clone());
                continue;
            }
            if grant.requires_recent_reauthentication && !context.recent_reauthentication {
                effective
                    .reauthentication_required_grant_ids
                    .push(grant.grant_id.clone());
                continue;
            }
            effective
                .matched_allow_rule_ids
                .push(grant.grant_id.clone());
            effective_network_rules.push(PermissionRule {
                rule_id: grant.grant_id.clone(),
                effect: PermissionEffect::Allow,
                subjects: vec![SubjectSelector::AnyMember],
                resources: vec![request.resource.kind],
                actions: vec![request.action],
                conditions: vec![PermissionCondition::MembershipActive],
            });
        }
    }

    for (layer, rules) in [
        (
            EffectivePolicyLayer::Local,
            effective_local_rules.as_slice(),
        ),
        (
            EffectivePolicyLayer::Network,
            effective_network_rules.as_slice(),
        ),
    ] {
        for rule in rules.iter().filter(|rule| rule.matches(request, context)) {
            let target = match rule.effect {
                PermissionEffect::Allow => &mut effective.matched_allow_rule_ids,
                PermissionEffect::Deny => &mut effective.matched_deny_rule_ids,
            };
            if !target.contains(&rule.rule_id) {
                target.push(rule.rule_id.clone());
            }
            if rule.effect == PermissionEffect::Deny && effective.rule_id.is_none() {
                effective.rule_id = Some(rule.rule_id.clone());
                effective.layer = Some(layer);
            }
        }
    }

    let decision = evaluate_permissions(
        &effective_local_rules,
        &effective_network_rules,
        request,
        context,
    );
    match decision {
        PermissionDecision::Allow => EffectiveAccess {
            allowed: true,
            code: None,
            reason: EffectiveAccessReason::Allowed,
            layer: None,
            rule_id: None,
            ..effective
        },
        PermissionDecision::Deny {
            code,
            layer,
            rule_id,
        } => {
            if code == PermissionDenialCode::DefaultDenied.as_str() {
                let lifecycle_denial = effective
                    .revoked_grant_ids
                    .first()
                    .cloned()
                    .map(|grant_id| {
                        (
                            PermissionDenialCode::PolicyDenied,
                            EffectiveAccessReason::AuthorizationRevoked,
                            grant_id,
                        )
                    })
                    .or_else(|| {
                        effective
                            .expired_grant_ids
                            .first()
                            .cloned()
                            .map(|grant_id| {
                                (
                                    PermissionDenialCode::PolicyDenied,
                                    EffectiveAccessReason::AuthorizationExpired,
                                    grant_id,
                                )
                            })
                    })
                    .or_else(|| {
                        effective
                            .reauthentication_required_grant_ids
                            .first()
                            .cloned()
                            .map(|grant_id| {
                                (
                                    PermissionDenialCode::RecentAuthenticationRequired,
                                    EffectiveAccessReason::RecentReauthenticationRequired,
                                    grant_id,
                                )
                            })
                    });
                if let Some((code, reason, grant_id)) = lifecycle_denial {
                    return denied_effective(
                        effective,
                        code,
                        reason,
                        EffectivePolicyLayer::Network,
                        Some(grant_id),
                    );
                }
            }
            let reason = reason_for_decision(&code);
            let projected_layer = match layer {
                "local" => Some(EffectivePolicyLayer::Local),
                "network" => Some(EffectivePolicyLayer::Network),
                _ => Some(EffectivePolicyLayer::Request),
            };
            EffectiveAccess {
                allowed: false,
                code: Some(code),
                reason,
                layer: projected_layer,
                rule_id,
                ..effective
            }
        }
    }
}

fn denied_effective(
    mut effective: EffectiveAccess,
    code: PermissionDenialCode,
    reason: EffectiveAccessReason,
    layer: EffectivePolicyLayer,
    rule_id: Option<SecurityRuleId>,
) -> EffectiveAccess {
    effective.allowed = false;
    effective.code = Some(code.as_str().to_owned());
    effective.reason = reason;
    effective.layer = Some(layer);
    effective.rule_id = rule_id;
    effective
}

fn reason_for_decision(code: &str) -> EffectiveAccessReason {
    if code == PermissionDenialCode::Denied.as_str() {
        EffectiveAccessReason::ExplicitDeny
    } else if code == PermissionDenialCode::RoleCeilingExceeded.as_str() {
        EffectiveAccessReason::RoleCeilingExceeded
    } else if code == PermissionDenialCode::TemporaryCoordinatorReadOnly.as_str() {
        EffectiveAccessReason::TemporaryCoordinatorReadOnly
    } else if code == PermissionDenialCode::RecentAuthenticationRequired.as_str() {
        EffectiveAccessReason::RecentReauthenticationRequired
    } else if code == PermissionDenialCode::CrossNetworkDenied.as_str() {
        EffectiveAccessReason::CrossNetworkDenied
    } else {
        EffectiveAccessReason::DefaultDenied
    }
}

fn content_hash<T: Serialize>(value: &T, label: &str) -> DomainResult<ContentHash> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| DomainError::invalid_value(format!("{label} cannot be encoded")))?;
    Ok(ContentHash::from_bytes(&bytes))
}

fn revision_conflict(expected: u64, actual: u64) -> DomainError {
    DomainError::new(
        "domain.access_control_revision_conflict",
        format!("access control revision conflict: expected {expected}, actual {actual}"),
    )
}

fn has_duplicates<T: Ord>(values: &[T]) -> bool {
    let mut seen = BTreeSet::new();
    values.iter().any(|value| !seen.insert(value))
}

fn append_audit(log: &mut AuditLog, entry: AuditEntry) -> DomainResult<()> {
    log.append(entry).map(|_| ()).map_err(|error| match error {
        AuditAppendError::Invalid(error) => error,
        AuditAppendError::DuplicateIdConflict => {
            DomainError::invariant("access control audit id was reused with different content")
        }
    })
}

fn network_acl_change_audit(
    actor: &NodeId,
    previous: &NetworkAcl,
    candidate: &NetworkAcl,
    action: &str,
    now_unix_ms: u64,
) -> DomainResult<AuditEntry> {
    let mut details = BTreeMap::new();
    details.insert("previous_revision".into(), previous.revision.to_string());
    details.insert("candidate_revision".into(), candidate.revision.to_string());
    details.insert(
        "candidate_digest".into(),
        candidate.content_hash()?.as_str().into(),
    );
    details.insert("rule_count".into(), candidate.rules.len().to_string());
    details.insert("grant_count".into(), candidate.grants.len().to_string());
    let audit_id = AuditId::from_public_key(
        format!(
            "{}:{}:{}:{}:{}",
            action, actor, previous.revision, candidate.revision, now_unix_ms
        )
        .as_bytes(),
    );
    Ok(AuditEntry {
        audit_id,
        operation_id: None,
        actor: actor.clone(),
        network_id: Some(candidate.network_id.clone()),
        action: action.into(),
        resource: format!("network_acl:{}", candidate.network_id),
        outcome: AuditOutcome::Succeeded,
        occurred_at_unix_ms: now_unix_ms,
        summary: "网络安全 ACL 已更新".into(),
        details,
        sensitive_keys: BTreeSet::new(),
    })
}

fn local_policy_change_audit(
    actor: &NodeId,
    previous: &NodeLocalAccessPolicy,
    candidate: &NodeLocalAccessPolicy,
    now_unix_ms: u64,
) -> DomainResult<AuditEntry> {
    let mut details = BTreeMap::new();
    details.insert("previous_revision".into(), previous.revision.to_string());
    details.insert("candidate_revision".into(), candidate.revision.to_string());
    details.insert(
        "candidate_digest".into(),
        candidate.content_hash()?.as_str().into(),
    );
    details.insert(
        "network_scope".into(),
        match &candidate.network_scope {
            NodeNetworkScope::All => "all".into(),
            NodeNetworkScope::Only(ids) => format!("only:{}", ids.len()),
        },
    );
    details.insert(
        "contribution_veto_count".into(),
        candidate.contribution_vetoes.len().to_string(),
    );
    let audit_id = AuditId::from_public_key(
        format!(
            "{}:{}:{}:{}:{}",
            "node_access_policy.updated", actor, previous.revision, candidate.revision, now_unix_ms
        )
        .as_bytes(),
    );
    Ok(AuditEntry {
        audit_id,
        operation_id: None,
        actor: actor.clone(),
        network_id: None,
        action: "node_access_policy.updated".into(),
        resource: format!("node_access_policy:{}", candidate.node_id),
        outcome: AuditOutcome::Succeeded,
        occurred_at_unix_ms: now_unix_ms,
        summary: "节点本地访问限制已更新".into(),
        details,
        sensitive_keys: BTreeSet::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{GroupId, NetworkRole};

    fn network_id(seed: &[u8]) -> NetworkId {
        NetworkId::from_public_key(seed)
    }

    fn actor() -> NodeId {
        NodeId::from_public_key(b"access-control-node")
    }

    fn group() -> GroupId {
        GroupId::from_public_key(b"access-control-group")
    }

    fn context(network_id: NetworkId, role: NetworkRole) -> AccessContext {
        AccessContext {
            network_id,
            actor: actor(),
            role,
            groups: [group()].into_iter().collect(),
            membership_active: true,
            recent_reauthentication: true,
            emergency_coordinator: false,
        }
    }

    fn request(
        network_id: NetworkId,
        action: PolicyAction,
        resource: ResourceKind,
    ) -> PermissionRequest {
        PermissionRequest {
            action,
            resource: super::super::permissions::ResourceRef {
                kind: resource,
                network_id,
                object_id: None,
            },
            intent: PermissionIntent::Mutating,
        }
    }

    fn allow_rule(
        seed: &[u8],
        subject: SubjectSelector,
        action: PolicyAction,
        resource: ResourceKind,
        network_id: NetworkId,
    ) -> PermissionRule {
        PermissionRule {
            rule_id: SecurityRuleId::from_public_key(seed),
            effect: PermissionEffect::Allow,
            subjects: vec![subject],
            resources: vec![resource],
            actions: vec![action],
            conditions: vec![
                PermissionCondition::MembershipActive,
                PermissionCondition::ResourceInNetwork(network_id),
            ],
        }
    }

    fn deny_rule(
        seed: &[u8],
        subject: SubjectSelector,
        action: PolicyAction,
        resource: ResourceKind,
    ) -> PermissionRule {
        PermissionRule {
            rule_id: SecurityRuleId::from_public_key(seed),
            effect: PermissionEffect::Deny,
            subjects: vec![subject],
            resources: vec![resource],
            actions: vec![action],
            conditions: vec![PermissionCondition::MembershipActive],
        }
    }

    fn grant(seed: &[u8], expires_at_unix_ms: u64) -> NetworkAclGrant {
        NetworkAclGrant {
            grant_id: SecurityRuleId::from_public_key(seed),
            subjects: vec![
                SubjectSelector::Role(NetworkRole::Administrator),
                SubjectSelector::Group(group()),
                SubjectSelector::Node(actor()),
            ],
            resources: vec![ResourceKind::SecurityPolicy],
            actions: vec![PolicyAction::Update],
            issued_at_unix_ms: 10,
            expires_at_unix_ms,
            revoked_at_unix_ms: None,
            revoked_by: None,
            requires_recent_reauthentication: false,
        }
    }

    fn acl(
        network_id: NetworkId,
        rules: Vec<PermissionRule>,
        grants: Vec<NetworkAclGrant>,
    ) -> NetworkAcl {
        NetworkAcl {
            network_id,
            revision: 1,
            issued_at_unix_ms: 10,
            expires_at_unix_ms: 10_000,
            rules,
            grants,
        }
    }

    fn local_policy(network_id: NetworkId) -> NodeLocalAccessPolicy {
        NodeLocalAccessPolicy {
            node_id: actor(),
            revision: 1,
            issued_at_unix_ms: 10,
            expires_at_unix_ms: 10_000,
            network_scope: NodeNetworkScope::All,
            contribution_vetoes: Vec::new(),
            exit_contribution: None,
            rules: vec![allow_rule(
                b"local-allow",
                SubjectSelector::AnyMember,
                PolicyAction::Update,
                ResourceKind::SecurityPolicy,
                network_id,
            )],
        }
    }

    #[test]
    fn explicit_deny_overrides_role_group_and_direct_allow() {
        let network_id = network_id(b"network-deny");
        let context = context(network_id.clone(), NetworkRole::Administrator);
        let request = request(
            network_id.clone(),
            PolicyAction::Update,
            ResourceKind::SecurityPolicy,
        );
        let role_allow = allow_rule(
            b"role-allow",
            SubjectSelector::Role(NetworkRole::Administrator),
            PolicyAction::Update,
            ResourceKind::SecurityPolicy,
            network_id.clone(),
        );
        let baseline = vec![role_allow.clone()];
        let direct_grant = grant(b"direct-grant", 5_000);
        let deny = deny_rule(
            b"explicit-deny",
            SubjectSelector::AnyMember,
            PolicyAction::Update,
            ResourceKind::SecurityPolicy,
        );
        let decision = evaluate_effective_access(
            &[],
            &baseline,
            Some(&acl(
                network_id.clone(),
                vec![deny.clone()],
                vec![direct_grant.clone()],
            )),
            Some(&local_policy(network_id)),
            &request,
            &context,
            100,
        );
        assert!(!decision.allowed);
        assert_eq!(decision.reason, EffectiveAccessReason::ExplicitDeny);
        assert_eq!(decision.layer, Some(EffectivePolicyLayer::Network));
        assert_eq!(decision.rule_id.as_ref(), Some(&deny.rule_id));
        assert!(decision.matched_deny_rule_ids.contains(&deny.rule_id));
        assert!(decision
            .matched_allow_rule_ids
            .contains(&role_allow.rule_id));
        assert!(decision
            .matched_allow_rule_ids
            .contains(&direct_grant.grant_id));
        assert_eq!(decision.network_acl_revision, 1);
        assert_eq!(decision.local_policy_revision, 1);
    }

    #[test]
    fn node_owner_contribution_veto_cannot_be_overridden_by_network_admin() {
        let network_id = network_id(b"network-veto");
        let context = context(network_id.clone(), NetworkRole::Administrator);
        let request = request(
            network_id.clone(),
            PolicyAction::Use,
            ResourceKind::ExitGrant,
        );
        let veto_id = SecurityRuleId::from_public_key(b"veto-exit");
        let mut local = local_policy(network_id.clone());
        local.contribution_vetoes.push(NodeContributionVeto {
            veto_id: veto_id.clone(),
            network_id: network_id.clone(),
            contributions: vec![ContributionKind::Exit],
        });
        let network_allow = vec![allow_rule(
            b"network-admin-allow",
            SubjectSelector::Role(NetworkRole::Administrator),
            PolicyAction::Use,
            ResourceKind::ExitGrant,
            network_id.clone(),
        )];
        let direct = NetworkAclGrant {
            grant_id: SecurityRuleId::from_public_key(b"admin-exit-grant"),
            subjects: vec![
                SubjectSelector::Role(NetworkRole::Administrator),
                SubjectSelector::Group(group()),
                SubjectSelector::Node(actor()),
            ],
            resources: vec![ResourceKind::ExitGrant],
            actions: vec![PolicyAction::Use],
            issued_at_unix_ms: 10,
            expires_at_unix_ms: 5_000,
            revoked_at_unix_ms: None,
            revoked_by: None,
            requires_recent_reauthentication: false,
        };
        let decision = evaluate_effective_access(
            &[],
            &network_allow,
            Some(&acl(network_id, Vec::new(), vec![direct])),
            Some(&local),
            &request,
            &context,
            100,
        );
        assert!(!decision.allowed);
        assert_eq!(
            decision.reason,
            EffectiveAccessReason::LocalContributionVeto
        );
        assert_eq!(decision.rule_id.as_ref(), Some(&veto_id));
        assert_eq!(decision.layer, Some(EffectivePolicyLayer::Local));
        assert_eq!(decision.local_policy_revision, 1);
        assert_eq!(decision.network_acl_revision, 1);
    }

    #[test]
    fn node_owner_network_scope_restriction_fails_closed() {
        let home = network_id(b"scope-home");
        let office = network_id(b"scope-office");
        let context = context(office.clone(), NetworkRole::Owner);
        let request = request(
            office.clone(),
            PolicyAction::Update,
            ResourceKind::SecurityPolicy,
        );
        let mut local = local_policy(office.clone());
        local.network_scope = NodeNetworkScope::Only(vec![home]);
        let decision = evaluate_effective_access(
            &[],
            &[allow_rule(
                b"owner-network-allow",
                SubjectSelector::Role(NetworkRole::Owner),
                PolicyAction::Update,
                ResourceKind::SecurityPolicy,
                office,
            )],
            None,
            Some(&local),
            &request,
            &context,
            100,
        );
        assert!(!decision.allowed);
        assert_eq!(
            decision.reason,
            EffectiveAccessReason::NetworkScopeRestricted
        );
        assert_eq!(
            decision.code.as_deref(),
            Some(PermissionDenialCode::NetworkScopeDenied.as_str())
        );
    }

    #[test]
    fn expired_and_revoked_grants_never_authorize() {
        let network = network_id(b"grant-lifecycle");
        let context = context(network.clone(), NetworkRole::Administrator);
        let request = request(
            network.clone(),
            PolicyAction::Update,
            ResourceKind::SecurityPolicy,
        );
        let local = local_policy(network.clone());
        let expired = grant(b"expired-grant", 100);
        let decision = evaluate_effective_access(
            &[],
            &[],
            Some(&acl(network.clone(), Vec::new(), vec![expired.clone()])),
            Some(&local),
            &request,
            &context,
            100,
        );
        assert!(!decision.allowed);
        assert_eq!(decision.reason, EffectiveAccessReason::AuthorizationExpired);

        let mut revoked = grant(b"revoked-grant", 5_000);
        revoked.revoked_at_unix_ms = Some(50);
        revoked.revoked_by = Some(actor());
        let decision = evaluate_effective_access(
            &[],
            &[],
            Some(&acl(network.clone(), Vec::new(), vec![revoked.clone()])),
            Some(&local_policy(network.clone())),
            &request,
            &context,
            100,
        );
        assert!(!decision.allowed);
        assert_eq!(decision.reason, EffectiveAccessReason::AuthorizationRevoked);
        assert!(decision.revoked_grant_ids.contains(&revoked.grant_id));
    }

    #[test]
    fn reauthentication_requirement_and_effective_projection_use_same_evaluator() {
        let network = network_id(b"reauth-grant");
        let mut context = context(network.clone(), NetworkRole::Administrator);
        context.recent_reauthentication = false;
        let request = request(
            network.clone(),
            PolicyAction::Update,
            ResourceKind::SecurityPolicy,
        );
        let mut required = grant(b"reauth-grant-id", 5_000);
        required.requires_recent_reauthentication = true;
        let decision = evaluate_effective_access(
            &[],
            &[],
            Some(&acl(network.clone(), Vec::new(), vec![required.clone()])),
            Some(&local_policy(network.clone())),
            &request,
            &context,
            100,
        );
        assert!(!decision.allowed);
        assert_eq!(
            decision.reason,
            EffectiveAccessReason::RecentReauthenticationRequired
        );
        assert_eq!(
            decision.code.as_deref(),
            Some(PermissionDenialCode::RecentAuthenticationRequired.as_str())
        );
        assert!(!decision.reauthentication_satisfied);

        context.recent_reauthentication = true;
        let decision = evaluate_effective_access(
            &[],
            &[],
            Some(&acl(network.clone(), Vec::new(), vec![required.clone()])),
            Some(&local_policy(network.clone())),
            &request,
            &context,
            100,
        );
        assert!(decision.allowed);
        assert_eq!(decision.reason, EffectiveAccessReason::Allowed);
        assert_eq!(decision.network_acl_revision, 1);
        assert_eq!(decision.local_policy_revision, 1);
        assert!(decision.matched_allow_rule_ids.contains(&required.grant_id));
    }

    #[test]
    fn stale_acl_update_and_revocation_race_fail_closed_and_are_audited() {
        let network_id = network_id(b"acl-race");
        let initial = acl(
            network_id.clone(),
            Vec::new(),
            vec![grant(b"race-grant", 5_000)],
        );
        let mut state = NetworkAclState::new(initial).unwrap();
        let mut revision_two = state.current().clone();
        revision_two.revision = 2;
        revision_two.rules.push(deny_rule(
            b"race-deny",
            SubjectSelector::AnyMember,
            PolicyAction::Update,
            ResourceKind::SecurityPolicy,
        ));
        let audit = state
            .apply_update(actor(), 1, revision_two.clone(), 100)
            .unwrap();
        assert_eq!(audit.action, "network_acl.updated");
        assert_eq!(state.audit_entries().len(), 1);

        let mut stale = revision_two;
        stale.revision = 3;
        let error = state.apply_update(actor(), 1, stale, 101).unwrap_err();
        assert_eq!(error.code(), "domain.access_control_revision_conflict");
        assert_eq!(state.current().revision, 2);
        assert_eq!(state.audit_entries().len(), 1);

        let grant_id = SecurityRuleId::from_public_key(b"race-grant");
        let error = state.revoke_grant(actor(), 1, &grant_id, 102).unwrap_err();
        assert_eq!(error.code(), "domain.access_control_revision_conflict");
        assert_eq!(state.current().revision, 2);
        assert_eq!(state.audit_entries().len(), 1);

        let revoke_audit = state.revoke_grant(actor(), 2, &grant_id, 103).unwrap();
        assert_eq!(revoke_audit.action, "network_acl.grant_revoked");
        assert_eq!(state.current().revision, 3);
        assert!(state.current().grants[0].is_revoked());
        assert_eq!(state.audit_entries().len(), 2);
    }

    #[test]
    fn local_policy_changes_require_owner_and_append_audit() {
        let network_id = network_id(b"local-audit");
        let mut state = NodeLocalAccessPolicyState::new(local_policy(network_id.clone())).unwrap();
        let mut candidate = state.current().clone();
        candidate.revision = 2;
        candidate.contribution_vetoes.push(NodeContributionVeto {
            veto_id: SecurityRuleId::from_public_key(b"local-audit-veto"),
            network_id,
            contributions: vec![ContributionKind::Relay],
        });
        let non_owner = NodeId::from_public_key(b"network-admin");
        let error = state
            .apply_update(non_owner, 1, candidate.clone(), 100)
            .unwrap_err();
        assert_eq!(error.code(), "domain.local_owner_required");
        assert_eq!(state.current().revision, 1);
        assert!(state.audit_entries().is_empty());

        let audit = state.apply_update(actor(), 1, candidate, 100).unwrap();
        assert_eq!(audit.action, "node_access_policy.updated");
        assert_eq!(audit.actor, actor());
        assert_eq!(state.current().revision, 2);
        assert_eq!(state.audit_entries().len(), 1);
    }

    #[test]
    fn expired_and_malformed_policies_fail_closed() {
        let network_id = network_id(b"invalid-acl");
        let context = context(network_id.clone(), NetworkRole::Owner);
        let request = request(
            network_id.clone(),
            PolicyAction::Update,
            ResourceKind::SecurityPolicy,
        );
        let mut expired_acl = acl(network_id.clone(), Vec::new(), Vec::new());
        expired_acl.expires_at_unix_ms = 100;
        let decision = evaluate_effective_access(
            &[],
            &[allow_rule(
                b"owner-allow",
                SubjectSelector::Role(NetworkRole::Owner),
                PolicyAction::Update,
                ResourceKind::SecurityPolicy,
                network_id.clone(),
            )],
            Some(&expired_acl),
            None,
            &request,
            &context,
            100,
        );
        assert!(!decision.allowed);
        assert_eq!(decision.reason, EffectiveAccessReason::AuthorizationExpired);

        let mut malformed = acl(network_id, Vec::new(), Vec::new());
        malformed.revision = 0;
        let decision =
            evaluate_effective_access(&[], &[], Some(&malformed), None, &request, &context, 100);
        assert!(!decision.allowed);
        assert_eq!(decision.reason, EffectiveAccessReason::InvalidPolicy);
    }
}
