use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ids::{ContentHash, NetworkId, SecurityRuleId};
use super::permissions::{
    evaluate_network_permissions, AccessContext, PermissionDecision, PermissionEffect,
    PermissionRequest, PermissionRule, SubjectSelector,
};
use super::{DomainError, DomainResult};

/// Fields that require an explicit network disclosure grant. Node-local fields
/// such as process lists, paths and detailed logs are intentionally absent and
/// remain owner-only in `NodePrivacySettings`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ProtectedField {
    MemberDisplayName,
    NodePublicKey,
    Endpoints,
    Health,
    RealNetworkAddresses,
    TrafficStatistics,
    RelayTopology,
    ExitDetails,
    FileList,
    AuditDetails,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct FieldAccessRule {
    pub rule_id: SecurityRuleId,
    pub field: ProtectedField,
    pub effect: PermissionEffect,
    pub subjects: Vec<SubjectSelector>,
}

impl FieldAccessRule {
    pub fn validate(&self) -> DomainResult<()> {
        if self.subjects.is_empty() {
            return Err(DomainError::invalid_value(
                "field access rule subjects cannot be empty",
            ));
        }
        let mut seen = BTreeSet::new();
        if self.subjects.iter().any(|subject| !seen.insert(subject)) {
            return Err(DomainError::invariant(
                "field access rule contains duplicate subjects",
            ));
        }
        Ok(())
    }

    fn matches(&self, field: ProtectedField, context: &AccessContext) -> bool {
        self.field == field && self.subjects.iter().any(|subject| subject.matches(context))
    }
}

/// Default deny field disclosure. Any matching deny wins; otherwise at least
/// one matching allow is required.
pub fn evaluate_field_access(
    rules: &[FieldAccessRule],
    field: ProtectedField,
    context: &AccessContext,
) -> PermissionDecision {
    for rule in rules.iter().filter(|rule| rule.matches(field, context)) {
        if rule.effect == PermissionEffect::Deny {
            return PermissionDecision::Deny {
                code: "permission.field_denied".into(),
                layer: "network",
                rule_id: Some(rule.rule_id.clone()),
            };
        }
    }
    if rules
        .iter()
        .any(|rule| rule.effect == PermissionEffect::Allow && rule.matches(field, context))
    {
        PermissionDecision::Allow
    } else {
        PermissionDecision::Deny {
            code: "permission.field_default_denied".into(),
            layer: "network",
            rule_id: None,
        }
    }
}

/// Versioned network security policy. It is immutable once issued: changes
/// create a higher revision. Expired revisions fail closed.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkSecurityPolicy {
    pub network_id: NetworkId,
    pub revision: u64,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    #[serde(default)]
    pub rules: Vec<PermissionRule>,
    #[serde(default)]
    pub field_rules: Vec<FieldAccessRule>,
}

impl NetworkSecurityPolicy {
    pub fn validate_at(&self, now_unix_ms: u64) -> DomainResult<()> {
        if self.revision == 0 {
            return Err(DomainError::invalid_value(
                "security policy revision must be positive",
            ));
        }
        if self.issued_at_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::invariant(
                "security policy validity interval is empty",
            ));
        }
        if now_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::new(
                "domain.security_policy_expired",
                "network security policy has expired",
            ));
        }
        let mut rule_ids = BTreeSet::new();
        for rule in &self.rules {
            rule.validate()?;
            if !rule_ids.insert(&rule.rule_id) {
                return Err(DomainError::invariant("duplicate network security rule id"));
            }
            if rule.conditions.iter().any(|condition| {
                matches!(
                    condition,
                    super::permissions::PermissionCondition::ResourceInNetwork(id)
                        if id != &self.network_id
                )
            }) {
                return Err(DomainError::invariant(
                    "security rule references another network",
                ));
            }
        }
        for rule in &self.field_rules {
            rule.validate()?;
            if !rule_ids.insert(&rule.rule_id) {
                return Err(DomainError::invariant("duplicate network security rule id"));
            }
        }
        Ok(())
    }

    pub fn evaluate(
        &self,
        request: &PermissionRequest,
        context: &AccessContext,
        now_unix_ms: u64,
    ) -> DomainResult<PermissionDecision> {
        self.validate_at(now_unix_ms)?;
        if request.resource.network_id != self.network_id {
            return Ok(PermissionDecision::Deny {
                code: "permission.cross_network_denied".into(),
                layer: "network",
                rule_id: None,
            });
        }
        Ok(evaluate_network_permissions(&self.rules, request, context))
    }

    pub fn evaluate_field(
        &self,
        field: ProtectedField,
        context: &AccessContext,
        now_unix_ms: u64,
    ) -> DomainResult<PermissionDecision> {
        self.validate_at(now_unix_ms)?;
        Ok(evaluate_field_access(&self.field_rules, field, context))
    }

    pub fn content_hash(&self) -> DomainResult<ContentHash> {
        let bytes = serde_json::to_vec(self)
            .map_err(|_| DomainError::invalid_value("security policy cannot be encoded"))?;
        Ok(ContentHash::from_bytes(&bytes))
    }

    pub fn diff(&self, candidate: &NetworkSecurityPolicy) -> DomainResult<PolicyDiff> {
        if candidate.network_id != self.network_id {
            return Err(DomainError::invariant(
                "security policy candidate belongs to another network",
            ));
        }
        if candidate.revision <= self.revision {
            return Err(DomainError::new(
                "domain.security_policy_rollback",
                "security policy revision must increase",
            ));
        }
        let old_ids: BTreeSet<_> = self.rules.iter().map(|rule| &rule.rule_id).collect();
        let new_ids: BTreeSet<_> = candidate.rules.iter().map(|rule| &rule.rule_id).collect();
        Ok(PolicyDiff {
            base_revision: self.revision,
            candidate_revision: candidate.revision,
            added_rule_ids: new_ids
                .difference(&old_ids)
                .map(|id| (*id).clone())
                .collect(),
            removed_rule_ids: old_ids
                .difference(&new_ids)
                .map(|id| (*id).clone())
                .collect(),
            replaces_all_rules: true,
        })
    }

    /// Produces an immutable apply plan after policy validation. Commit must
    /// re-check the exact candidate digest; failure leaves the old revision live.
    pub fn prepare_apply(
        &self,
        candidate: NetworkSecurityPolicy,
        now_unix_ms: u64,
    ) -> DomainResult<PolicyApplyPlan> {
        self.validate_at(now_unix_ms)?;
        candidate.validate_at(now_unix_ms)?;
        let diff = self.diff(&candidate)?;
        Ok(PolicyApplyPlan {
            base_revision: self.revision,
            candidate_revision: candidate.revision,
            candidate_digest: candidate.content_hash()?,
            tested_at_unix_ms: now_unix_ms,
            expires_at_unix_ms: candidate.expires_at_unix_ms,
            diff,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct PolicyDiff {
    pub base_revision: u64,
    pub candidate_revision: u64,
    pub added_rule_ids: Vec<SecurityRuleId>,
    pub removed_rule_ids: Vec<SecurityRuleId>,
    pub replaces_all_rules: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct PolicyApplyPlan {
    pub base_revision: u64,
    pub candidate_revision: u64,
    pub candidate_digest: ContentHash,
    pub tested_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    pub diff: PolicyDiff,
}

impl PolicyApplyPlan {
    pub fn validate_commit(
        &self,
        candidate: &NetworkSecurityPolicy,
        now_unix_ms: u64,
    ) -> DomainResult<()> {
        if now_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::new(
                "domain.security_policy_plan_expired",
                "security policy apply plan has expired",
            ));
        }
        if candidate.revision != self.candidate_revision
            || candidate.content_hash()? != self.candidate_digest
        {
            return Err(DomainError::new(
                "domain.security_policy_changed_after_test",
                "security policy changed after it was tested",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        AccessContext, NetworkRole, NodeId, PermissionRequest, PolicyAction, ResourceKind,
        ResourceRef,
    };

    fn policy_parts() -> (NetworkId, NodeId) {
        (
            NetworkId::from_public_key(b"network"),
            NodeId::from_public_key(b"node"),
        )
    }

    fn context(network_id: NetworkId, node_id: NodeId, role: NetworkRole) -> AccessContext {
        AccessContext {
            network_id,
            actor: node_id,
            role,
            groups: BTreeSet::new(),
            membership_active: true,
            recent_reauthentication: true,
            emergency_coordinator: false,
        }
    }

    fn permission_rule(
        id: &[u8],
        effect: PermissionEffect,
        action: PolicyAction,
        resource: ResourceKind,
    ) -> PermissionRule {
        PermissionRule {
            rule_id: SecurityRuleId::from_public_key(id),
            effect,
            subjects: vec![SubjectSelector::Role(NetworkRole::Administrator)],
            resources: vec![resource],
            actions: vec![action],
            conditions: vec![],
        }
    }

    fn policy(network_id: NetworkId) -> NetworkSecurityPolicy {
        NetworkSecurityPolicy {
            network_id,
            revision: 1,
            issued_at_unix_ms: 10,
            expires_at_unix_ms: 1_000,
            rules: vec![permission_rule(
                b"allow-policy",
                PermissionEffect::Allow,
                PolicyAction::Update,
                ResourceKind::SecurityPolicy,
            )],
            field_rules: vec![],
        }
    }

    fn request(network_id: NetworkId) -> PermissionRequest {
        PermissionRequest {
            action: PolicyAction::Update,
            resource: ResourceRef {
                kind: ResourceKind::SecurityPolicy,
                network_id,
                object_id: None,
            },
        }
    }

    #[test]
    fn deny_precedence_and_revocation_take_effect() {
        let (network_id, node_id) = policy_parts();
        let context = context(network_id.clone(), node_id, NetworkRole::Administrator);
        let mut policy = policy(network_id.clone());
        assert!(policy
            .evaluate(&request(network_id.clone()), &context, 100)
            .unwrap()
            .is_allowed());
        policy.rules.push(permission_rule(
            b"deny-policy",
            PermissionEffect::Deny,
            PolicyAction::Update,
            ResourceKind::SecurityPolicy,
        ));
        assert!(!policy
            .evaluate(&request(network_id.clone()), &context, 100)
            .unwrap()
            .is_allowed());
        policy
            .rules
            .retain(|rule| rule.effect == PermissionEffect::Allow);
        let mut revoked = policy.clone();
        revoked.revision = 2;
        revoked.rules.clear();
        policy.rules = revoked.rules.clone();
        assert!(!policy
            .evaluate(&request(network_id), &context, 100)
            .unwrap()
            .is_allowed());
    }

    #[test]
    fn explicit_allow_cannot_exceed_member_role_ceiling() {
        let (network_id, node_id) = policy_parts();
        let context = context(network_id.clone(), node_id, NetworkRole::Member);
        let mut policy = policy(network_id.clone());
        policy.rules[0].subjects = vec![SubjectSelector::AnyMember];
        let decision = policy
            .evaluate(&request(network_id), &context, 100)
            .unwrap();
        assert!(matches!(
            decision,
            PermissionDecision::Deny { code, .. }
                if code == "permission.role_ceiling_exceeded"
        ));
    }

    #[test]
    fn sensitive_fields_default_to_deny_and_deny_beats_allow() {
        let (network_id, node_id) = policy_parts();
        let member = context(network_id.clone(), node_id, NetworkRole::Member);
        let admin = context(
            network_id.clone(),
            NodeId::from_public_key(b"admin"),
            NetworkRole::Administrator,
        );
        let mut policy = policy(network_id);
        assert!(!policy
            .evaluate_field(ProtectedField::RealNetworkAddresses, &member, 100)
            .unwrap()
            .is_allowed());
        policy.field_rules.push(FieldAccessRule {
            rule_id: SecurityRuleId::from_public_key(b"admin-addresses"),
            field: ProtectedField::RealNetworkAddresses,
            effect: PermissionEffect::Allow,
            subjects: vec![SubjectSelector::Role(NetworkRole::Administrator)],
        });
        assert!(policy
            .evaluate_field(ProtectedField::RealNetworkAddresses, &admin, 100)
            .unwrap()
            .is_allowed());
        policy.field_rules.push(FieldAccessRule {
            rule_id: SecurityRuleId::from_public_key(b"deny-addresses"),
            field: ProtectedField::RealNetworkAddresses,
            effect: PermissionEffect::Deny,
            subjects: vec![SubjectSelector::AnyMember],
        });
        assert!(!policy
            .evaluate_field(ProtectedField::RealNetworkAddresses, &admin, 100)
            .unwrap()
            .is_allowed());
    }

    #[test]
    fn expired_policy_and_changed_plan_fail_before_commit() {
        let (network_id, _) = policy_parts();
        let current = policy(network_id.clone());
        let mut candidate = current.clone();
        candidate.revision = 2;
        candidate.expires_at_unix_ms = 2_000;
        let plan = current.prepare_apply(candidate.clone(), 200).unwrap();
        assert!(plan.validate_commit(&candidate, 200).is_ok());
        let mut changed = candidate.clone();
        changed.rules.clear();
        assert_eq!(
            plan.validate_commit(&changed, 200).unwrap_err().code(),
            "domain.security_policy_changed_after_test"
        );
        assert_eq!(
            current.validate_at(1_000).unwrap_err().code(),
            "domain.security_policy_expired"
        );
    }
}
