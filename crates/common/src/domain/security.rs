//! Network security policy, field-level privacy, and atomic policy rollout.

use super::foundation::{
    require_non_empty, require_unique, ContractIssue, ContractResult, UnixMillis,
};
use super::ids::PolicyId;
use super::permission::{
    AccessContext, PermissionAction, PermissionPrincipal, PolicyCondition, RuleEffect,
    SubjectSelector,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityTarget {
    File,
    Exit,
    Relay,
    Topology,
    Audit,
    Network,
    Membership,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityField {
    Exists,
    Name,
    Path,
    Size,
    Checksum,
    Owner,
    Endpoint,
    Address,
    Region,
    Statistics,
    Identity,
    Status,
    MembershipList,
    TopologyDetails,
    AuditDetails,
    ActivityDetails,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyAudience {
    Private,
    OwnerOnly,
    Administrators,
    Members,
    Public,
}

impl PrivacyAudience {
    pub const fn rank(self) -> u8 {
        match self {
            Self::Private => 0,
            Self::OwnerOnly => 1,
            Self::Administrators => 2,
            Self::Members => 3,
            Self::Public => 4,
        }
    }

    /// A caller may see a field only when the field's audience is at least as
    /// broad as the caller's visibility tier.
    pub const fn allows(self, caller: Self) -> bool {
        self.rank() >= caller.rank()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivacyPolicy {
    pub default_audience: PrivacyAudience,
    /// Maximum audience an administrator may select for a field. This prevents
    /// a network administrator from publishing fields the owner marked local.
    pub administrator_max_audience: PrivacyAudience,
    pub fields: BTreeMap<SecurityField, PrivacyAudience>,
}

impl Default for PrivacyPolicy {
    fn default() -> Self {
        Self {
            default_audience: PrivacyAudience::Private,
            administrator_max_audience: PrivacyAudience::Administrators,
            fields: BTreeMap::new(),
        }
    }
}

impl PrivacyPolicy {
    pub fn audience(&self, field: SecurityField) -> PrivacyAudience {
        self.fields
            .get(&field)
            .copied()
            .unwrap_or(self.default_audience)
    }

    pub fn set_field(
        &mut self,
        field: SecurityField,
        audience: PrivacyAudience,
    ) -> Result<(), ContractIssue> {
        if audience.rank() > self.administrator_max_audience.rank() {
            return Err(ContractIssue::new(
                "privacy_ceiling_exceeded",
                format!("privacy.fields.{field:?}"),
                "audience exceeds the administrator-controlled privacy ceiling",
            ));
        }
        self.fields.insert(field, audience);
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityRule {
    pub id: String,
    pub subjects: BTreeSet<SubjectSelector>,
    pub targets: BTreeSet<SecurityTarget>,
    pub fields: BTreeSet<SecurityField>,
    pub actions: BTreeSet<PermissionAction>,
    pub effect: RuleEffect,
    pub condition: PolicyCondition,
}

impl SecurityRule {
    fn matches(
        &self,
        principal: &PermissionPrincipal,
        target: SecurityTarget,
        field: SecurityField,
        action: PermissionAction,
        context: &AccessContext,
    ) -> bool {
        self.targets.contains(&target)
            && self.fields.contains(&field)
            && self.actions.contains(&action)
            && self
                .subjects
                .iter()
                .any(|subject| subject_matches(subject, principal))
            && self.condition.matches(principal, context)
    }
}

fn subject_matches(subject: &SubjectSelector, principal: &PermissionPrincipal) -> bool {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityPolicy {
    pub id: PolicyId,
    pub revision: u64,
    pub rules: Vec<SecurityRule>,
    pub privacy: PrivacyPolicy,
    pub created_at: UnixMillis,
    pub expires_at: Option<UnixMillis>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecurityDecision {
    Allow {
        matched_rules: Vec<String>,
    },
    Deny {
        matched_rules: Vec<String>,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityPolicyDiff {
    pub from_revision: u64,
    pub to_revision: u64,
    pub added_rules: Vec<String>,
    pub removed_rules: Vec<String>,
    pub changed_rules: Vec<String>,
    pub privacy_changes: Vec<SecurityField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecurityPolicyError {
    Contract(Vec<ContractIssue>),
    StaleRevision { expected: u64, actual: u64 },
    NonAdvancingRevision { current: u64, next: u64 },
}

impl fmt::Display for SecurityPolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(issues) => write!(f, "invalid security policy: {issues:?}"),
            Self::StaleRevision { expected, actual } => {
                write!(
                    f,
                    "stale security policy revision: expected {expected}, got {actual}"
                )
            }
            Self::NonAdvancingRevision { current, next } => {
                write!(
                    f,
                    "security policy revision must advance: {current} -> {next}"
                )
            }
        }
    }
}

impl std::error::Error for SecurityPolicyError {}

impl SecurityPolicy {
    pub fn validate(&self) -> ContractResult {
        let mut issues = Vec::new();
        if self.revision == 0 {
            issues.push(ContractIssue::new(
                "invalid_policy_revision",
                "revision",
                "security policy revision must be greater than zero",
            ));
        }
        if self
            .expires_at
            .is_some_and(|expires_at| !self.created_at.is_before(expires_at))
        {
            issues.push(ContractIssue::new(
                "invalid_policy_expiry",
                "expires_at",
                "policy expiry must be after creation time",
            ));
        }
        if self.privacy.administrator_max_audience.rank() < self.privacy.default_audience.rank() {
            issues.push(ContractIssue::new(
                "privacy_default_exceeds_ceiling",
                "privacy.default_audience",
                "default audience exceeds administrator privacy ceiling",
            ));
        }
        for (field, audience) in &self.privacy.fields {
            if audience.rank() > self.privacy.administrator_max_audience.rank() {
                issues.push(ContractIssue::new(
                    "privacy_ceiling_exceeded",
                    format!("privacy.fields.{field:?}"),
                    "field audience exceeds administrator privacy ceiling",
                ));
            }
        }
        let rule_ids = self
            .rules
            .iter()
            .map(|rule| rule.id.as_str())
            .collect::<Vec<_>>();
        require_unique(&rule_ids, "rules", &mut issues);
        for (index, rule) in self.rules.iter().enumerate() {
            require_non_empty(&rule.id, &format!("rules[{index}].id"), 128, &mut issues);
            if rule.subjects.is_empty()
                || rule.targets.is_empty()
                || rule.fields.is_empty()
                || rule.actions.is_empty()
            {
                issues.push(ContractIssue::new(
                    "incomplete_security_rule",
                    format!("rules[{index}]"),
                    "security rules require subjects, targets, fields, and actions",
                ));
            }
        }
        finish(issues)
    }

    pub fn evaluate(
        &self,
        principal: &PermissionPrincipal,
        target: SecurityTarget,
        field: SecurityField,
        action: PermissionAction,
        context: &AccessContext,
        now: UnixMillis,
    ) -> SecurityDecision {
        if self
            .expires_at
            .is_some_and(|expires_at| !now.is_before(expires_at))
        {
            return SecurityDecision::Deny {
                matched_rules: Vec::new(),
                reason: "policy_expired".into(),
            };
        }
        let matched = self
            .rules
            .iter()
            .filter(|rule| rule.matches(principal, target, field, action, context))
            .collect::<Vec<_>>();
        if matched.iter().any(|rule| rule.effect == RuleEffect::Deny) {
            return SecurityDecision::Deny {
                matched_rules: matched
                    .iter()
                    .filter(|rule| rule.effect == RuleEffect::Deny)
                    .map(|rule| rule.id.clone())
                    .collect(),
                reason: "explicit_deny".into(),
            };
        }
        let allowed = matched
            .iter()
            .filter(|rule| rule.effect == RuleEffect::Allow)
            .map(|rule| rule.id.clone())
            .collect::<Vec<_>>();
        if allowed.is_empty() {
            SecurityDecision::Deny {
                matched_rules: Vec::new(),
                reason: "default_deny".into(),
            }
        } else {
            SecurityDecision::Allow {
                matched_rules: allowed,
            }
        }
    }

    pub fn diff(&self, next: &SecurityPolicy) -> SecurityPolicyDiff {
        let current_rules = self
            .rules
            .iter()
            .map(|rule| (rule.id.clone(), rule))
            .collect::<BTreeMap<_, _>>();
        let next_rules = next
            .rules
            .iter()
            .map(|rule| (rule.id.clone(), rule))
            .collect::<BTreeMap<_, _>>();
        let added_rules = next_rules
            .keys()
            .filter(|id| !current_rules.contains_key(*id))
            .cloned()
            .collect();
        let removed_rules = current_rules
            .keys()
            .filter(|id| !next_rules.contains_key(*id))
            .cloned()
            .collect();
        let changed_rules = next_rules
            .iter()
            .filter(|(id, rule)| {
                current_rules
                    .get(*id)
                    .is_some_and(|current| current != *rule)
            })
            .map(|(id, _)| id.clone())
            .collect();
        let fields = self
            .privacy
            .fields
            .keys()
            .chain(next.privacy.fields.keys())
            .copied()
            .collect::<BTreeSet<_>>();
        let privacy_changes = fields
            .into_iter()
            .filter(|field| self.privacy.audience(*field) != next.privacy.audience(*field))
            .collect();
        SecurityPolicyDiff {
            from_revision: self.revision,
            to_revision: next.revision,
            added_rules,
            removed_rules,
            changed_rules,
            privacy_changes,
        }
    }

    /// Validate and apply a full policy update atomically. The returned previous
    /// policy is the durable rollback point.
    pub fn replace(
        &mut self,
        next: SecurityPolicy,
        expected_revision: u64,
        now: UnixMillis,
    ) -> Result<(SecurityPolicy, SecurityPolicyDiff), SecurityPolicyError> {
        if self.revision != expected_revision {
            return Err(SecurityPolicyError::StaleRevision {
                expected: expected_revision,
                actual: self.revision,
            });
        }
        if next.revision <= self.revision {
            return Err(SecurityPolicyError::NonAdvancingRevision {
                current: self.revision,
                next: next.revision,
            });
        }
        next.validate().map_err(SecurityPolicyError::Contract)?;
        if next
            .expires_at
            .is_some_and(|expires_at| !now.is_before(expires_at))
        {
            return Err(SecurityPolicyError::Contract(vec![ContractIssue::new(
                "policy_already_expired",
                "expires_at",
                "new policy is already expired",
            )]));
        }
        let diff = self.diff(&next);
        let previous = std::mem::replace(self, next);
        Ok((previous, diff))
    }

    pub fn rollback(
        &mut self,
        previous: SecurityPolicy,
        expected_revision: u64,
    ) -> Result<SecurityPolicy, SecurityPolicyError> {
        if self.revision != expected_revision {
            return Err(SecurityPolicyError::StaleRevision {
                expected: expected_revision,
                actual: self.revision,
            });
        }
        previous.validate().map_err(SecurityPolicyError::Contract)?;
        Ok(std::mem::replace(self, previous))
    }
}

fn finish(issues: Vec<ContractIssue>) -> ContractResult {
    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

#[cfg(test)]
mod tests {
    use super::super::ids::{GroupId, NodeId};
    use super::super::membership::NetworkRole;
    use super::*;

    fn rule(
        id: &str,
        subject: SubjectSelector,
        target: SecurityTarget,
        field: SecurityField,
        action: PermissionAction,
        effect: RuleEffect,
    ) -> SecurityRule {
        SecurityRule {
            id: id.into(),
            subjects: BTreeSet::from([subject]),
            targets: BTreeSet::from([target]),
            fields: BTreeSet::from([field]),
            actions: BTreeSet::from([action]),
            effect,
            condition: PolicyCondition::Always,
        }
    }

    fn policy() -> SecurityPolicy {
        SecurityPolicy {
            id: PolicyId::from_bytes([1; 16]),
            revision: 1,
            rules: vec![
                rule(
                    "member-file-name",
                    SubjectSelector::AnyMember,
                    SecurityTarget::File,
                    SecurityField::Name,
                    PermissionAction::View,
                    RuleEffect::Allow,
                ),
                rule(
                    "member-can-not-see-path",
                    SubjectSelector::AnyMember,
                    SecurityTarget::File,
                    SecurityField::Path,
                    PermissionAction::View,
                    RuleEffect::Deny,
                ),
                rule(
                    "admin-file-path",
                    SubjectSelector::Role(NetworkRole::Administrator),
                    SecurityTarget::File,
                    SecurityField::Path,
                    PermissionAction::View,
                    RuleEffect::Allow,
                ),
            ],
            privacy: PrivacyPolicy::default(),
            created_at: UnixMillis(1),
            expires_at: None,
        }
    }

    fn principal(role: NetworkRole, group: Option<GroupId>) -> PermissionPrincipal {
        let mut principal = PermissionPrincipal::new(NodeId::from_bytes([2; 32]));
        principal.roles.insert(role);
        principal.is_owner = role == NetworkRole::Owner;
        if let Some(group) = group {
            principal.groups.insert(group);
        }
        principal
    }

    #[test]
    fn deny_wins_for_field_level_access() {
        let policy = policy();
        let admin = policy.evaluate(
            &principal(NetworkRole::Administrator, None),
            SecurityTarget::File,
            SecurityField::Path,
            PermissionAction::View,
            &AccessContext::default(),
            UnixMillis(2),
        );
        assert!(matches!(
            admin,
            SecurityDecision::Deny { reason, .. } if reason == "explicit_deny"
        ));

        let name = policy.evaluate(
            &principal(NetworkRole::Member, None),
            SecurityTarget::File,
            SecurityField::Name,
            PermissionAction::View,
            &AccessContext::default(),
            UnixMillis(2),
        );
        assert!(matches!(name, SecurityDecision::Allow { .. }));
    }

    #[test]
    fn revocation_takes_effect_on_next_revision() {
        let mut policy = policy();
        let mut revoked = policy.clone();
        revoked.revision = 2;
        revoked.rules.retain(|rule| rule.id != "member-file-name");
        let (previous, diff) = policy.replace(revoked, 1, UnixMillis(2)).expect("replace");
        assert_eq!(diff.removed_rules, ["member-file-name"]);
        let decision = policy.evaluate(
            &principal(NetworkRole::Member, None),
            SecurityTarget::File,
            SecurityField::Name,
            PermissionAction::View,
            &AccessContext::default(),
            UnixMillis(3),
        );
        assert!(matches!(
            decision,
            SecurityDecision::Deny { reason, .. } if reason == "default_deny"
        ));
        policy.rollback(previous, 2).expect("rollback");
        assert_eq!(policy.revision, 1);
    }

    #[test]
    fn privacy_ceiling_prevents_admin_from_publishing_local_fields() {
        let mut privacy = PrivacyPolicy {
            administrator_max_audience: PrivacyAudience::Administrators,
            ..PrivacyPolicy::default()
        };
        assert!(privacy
            .set_field(SecurityField::Path, PrivacyAudience::Public)
            .is_err());
        privacy
            .set_field(SecurityField::Path, PrivacyAudience::Administrators)
            .expect("within ceiling");
        assert_eq!(
            privacy.audience(SecurityField::Path),
            PrivacyAudience::Administrators
        );
        assert_eq!(
            privacy.audience(SecurityField::Checksum),
            PrivacyAudience::Private
        );
    }

    #[test]
    fn replacement_and_rollback_are_atomic_on_validation_failure() {
        let mut policy = policy();
        let mut invalid = policy.clone();
        invalid.revision = 2;
        invalid.rules[0].subjects.clear();
        let error = policy
            .replace(invalid, 1, UnixMillis(2))
            .expect_err("invalid policy");
        assert!(matches!(error, SecurityPolicyError::Contract(_)));
        assert_eq!(policy.revision, 1);
    }
}
