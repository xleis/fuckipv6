//! Roles, groups, and deny-first layered authorization.

use super::capability::CapabilityId;
use super::foundation::{
    require_non_empty, require_unique, ContractIssue, ContractResult, UnixMillis,
};
use super::ids::{FileShareId, GroupId, NetworkId, NodeId, PolicyId};
use super::membership::NetworkRole;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyScope {
    NodeLocal,
    Network,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionResource {
    Node,
    Network,
    Membership,
    TrafficPolicy,
    SecurityPolicy,
    CoordinatorTerm,
    Capability(CapabilityId),
    FileShare(FileShareId),
    Audit,
    Activity,
    Topology,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionAction {
    View,
    Create,
    Update,
    Delete,
    Join,
    Leave,
    Approve,
    Suspend,
    Revoke,
    Manage,
    Execute,
    Export,
    Test,
    Rollback,
}

impl PermissionAction {
    pub const ALL: [Self; 14] = [
        Self::View,
        Self::Create,
        Self::Update,
        Self::Delete,
        Self::Join,
        Self::Leave,
        Self::Approve,
        Self::Suspend,
        Self::Revoke,
        Self::Manage,
        Self::Execute,
        Self::Export,
        Self::Test,
        Self::Rollback,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum SubjectSelector {
    Node(NodeId),
    Owner,
    Role(NetworkRole),
    Group(GroupId),
    Capability(CapabilityId),
    AnyMember,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionPrincipal {
    pub node_id: NodeId,
    pub is_owner: bool,
    pub roles: BTreeSet<NetworkRole>,
    pub groups: BTreeSet<GroupId>,
    pub capabilities: BTreeSet<CapabilityId>,
}

impl PermissionPrincipal {
    pub fn new(node_id: NodeId) -> Self {
        Self {
            node_id,
            is_owner: false,
            roles: BTreeSet::new(),
            groups: BTreeSet::new(),
            capabilities: BTreeSet::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessContext {
    pub network_id: Option<NetworkId>,
    pub trusted_device: bool,
    pub capabilities: BTreeSet<CapabilityId>,
    pub attributes: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum PolicyCondition {
    Always,
    DeviceTrusted,
    Network(NetworkId),
    Capability(CapabilityId),
    AttributeEquals { key: String, value: String },
    All(Vec<PolicyCondition>),
    Any(Vec<PolicyCondition>),
}

impl PolicyCondition {
    pub fn matches(&self, principal: &PermissionPrincipal, context: &AccessContext) -> bool {
        match self {
            Self::Always => true,
            Self::DeviceTrusted => context.trusted_device,
            Self::Network(network_id) => context.network_id == Some(*network_id),
            Self::Capability(capability_id) => {
                principal.capabilities.contains(capability_id)
                    || context.capabilities.contains(capability_id)
            }
            Self::AttributeEquals { key, value } => context.attributes.get(key) == Some(value),
            Self::All(conditions) => conditions
                .iter()
                .all(|condition| condition.matches(principal, context)),
            Self::Any(conditions) => conditions
                .iter()
                .any(|condition| condition.matches(principal, context)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleEffect {
    Allow,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRule {
    pub id: String,
    pub subjects: BTreeSet<SubjectSelector>,
    pub resources: BTreeSet<PermissionResource>,
    pub actions: BTreeSet<PermissionAction>,
    pub effect: RuleEffect,
    pub condition: PolicyCondition,
}

impl PermissionRule {
    fn matches(
        &self,
        principal: &PermissionPrincipal,
        resource: &PermissionResource,
        action: PermissionAction,
        context: &AccessContext,
    ) -> bool {
        self.resources.contains(resource)
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
pub struct PermissionPolicy {
    pub id: PolicyId,
    pub scope: PolicyScope,
    pub revision: u64,
    pub rules: Vec<PermissionRule>,
    pub created_at: UnixMillis,
    pub expires_at: Option<UnixMillis>,
}

impl PermissionPolicy {
    pub fn validate(&self) -> ContractResult {
        let mut issues = Vec::new();
        if self.revision == 0 {
            issues.push(ContractIssue::new(
                "invalid_policy_revision",
                "revision",
                "policy revision must be greater than zero",
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
        let rule_ids = self
            .rules
            .iter()
            .map(|rule| rule.id.as_str())
            .collect::<Vec<_>>();
        require_unique(&rule_ids, "rules", &mut issues);
        for (index, rule) in self.rules.iter().enumerate() {
            require_non_empty(&rule.id, &format!("rules[{index}].id"), 128, &mut issues);
            if rule.subjects.is_empty() {
                issues.push(ContractIssue::new(
                    "subjects_required",
                    format!("rules[{index}].subjects"),
                    "a permission rule requires at least one subject",
                ));
            }
            if rule.resources.is_empty() {
                issues.push(ContractIssue::new(
                    "resources_required",
                    format!("rules[{index}].resources"),
                    "a permission rule requires at least one resource",
                ));
            }
            if rule.actions.is_empty() {
                issues.push(ContractIssue::new(
                    "actions_required",
                    format!("rules[{index}].actions"),
                    "a permission rule requires at least one action",
                ));
            }
        }
        finish(issues)
    }

    pub fn evaluate(
        &self,
        principal: &PermissionPrincipal,
        resource: &PermissionResource,
        action: PermissionAction,
        context: &AccessContext,
        now: UnixMillis,
    ) -> PolicyDecision {
        if self
            .expires_at
            .is_some_and(|expires_at| !now.is_before(expires_at))
        {
            return PolicyDecision::Deny {
                matched_rules: Vec::new(),
                reason: "policy_expired".into(),
            };
        }
        let matched = self
            .rules
            .iter()
            .filter(|rule| rule.matches(principal, resource, action, context))
            .collect::<Vec<_>>();
        if matched.iter().any(|rule| rule.effect == RuleEffect::Deny) {
            return PolicyDecision::Deny {
                matched_rules: matched
                    .iter()
                    .filter(|rule| rule.effect == RuleEffect::Deny)
                    .map(|rule| rule.id.clone())
                    .collect(),
                reason: "explicit_deny".into(),
            };
        }
        if matched.iter().any(|rule| rule.effect == RuleEffect::Allow) {
            return PolicyDecision::Allow {
                matched_rules: matched
                    .iter()
                    .filter(|rule| rule.effect == RuleEffect::Allow)
                    .map(|rule| rule.id.clone())
                    .collect(),
            };
        }
        PolicyDecision::NoOpinion
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PolicyDecision {
    Allow {
        matched_rules: Vec<String>,
    },
    Deny {
        matched_rules: Vec<String>,
        reason: String,
    },
    NoOpinion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayeredAuthorizationDecision {
    pub allowed: bool,
    pub reason: String,
    pub node_local: PolicyDecision,
    pub network: PolicyDecision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayeredPermissionPolicy {
    pub node_local: PermissionPolicy,
    pub network: PermissionPolicy,
}

impl LayeredPermissionPolicy {
    pub fn validate(&self) -> ContractResult {
        let mut issues = Vec::new();
        if self.node_local.scope != PolicyScope::NodeLocal {
            issues.push(ContractIssue::new(
                "invalid_policy_scope",
                "node_local.scope",
                "node-local policy must use node-local scope",
            ));
        }
        if self.network.scope != PolicyScope::Network {
            issues.push(ContractIssue::new(
                "invalid_policy_scope",
                "network.scope",
                "network policy must use network scope",
            ));
        }
        if let Err(mut local_issues) = self.node_local.validate() {
            for issue in &mut local_issues {
                issue.field = format!("node_local.{}", issue.field);
            }
            issues.extend(local_issues);
        }
        if let Err(mut network_issues) = self.network.validate() {
            for issue in &mut network_issues {
                issue.field = format!("network.{}", issue.field);
            }
            issues.extend(network_issues);
        }
        finish(issues)
    }

    /// Authorization is an intersection: both layers must explicitly allow and
    /// a denial in either layer always wins. Missing or expired opinions deny.
    pub fn authorize(
        &self,
        principal: &PermissionPrincipal,
        resource: &PermissionResource,
        action: PermissionAction,
        context: &AccessContext,
        now: UnixMillis,
    ) -> LayeredAuthorizationDecision {
        let node_local = self
            .node_local
            .evaluate(principal, resource, action, context, now);
        let network = self
            .network
            .evaluate(principal, resource, action, context, now);
        let (allowed, reason) = match (&node_local, &network) {
            (PolicyDecision::Deny { reason, .. }, _) => (false, format!("node_local_{reason}")),
            (_, PolicyDecision::Deny { reason, .. }) => (false, format!("network_{reason}")),
            (PolicyDecision::Allow { .. }, PolicyDecision::Allow { .. }) => {
                (true, "joint_allow".into())
            }
            _ => (false, "joint_allow_required".into()),
        };
        LayeredAuthorizationDecision {
            allowed,
            reason,
            node_local,
            network,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum GroupRule {
    Static(Vec<SubjectSelector>),
    Role(NetworkRole),
    Capability(CapabilityId),
    Any(Vec<GroupRule>),
    All(Vec<GroupRule>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupDefinition {
    pub id: GroupId,
    pub name: String,
    pub rule: GroupRule,
}

impl GroupDefinition {
    pub fn contains(&self, principal: &PermissionPrincipal) -> bool {
        group_rule_contains(&self.rule, principal)
    }

    pub fn validate(&self) -> ContractResult {
        let mut issues = Vec::new();
        require_non_empty(&self.name, "name", 128, &mut issues);
        finish(issues)
    }
}

fn group_rule_contains(rule: &GroupRule, principal: &PermissionPrincipal) -> bool {
    match rule {
        GroupRule::Static(subjects) => subjects
            .iter()
            .any(|subject| subject_matches(subject, principal)),
        GroupRule::Role(role) => principal.roles.contains(role),
        GroupRule::Capability(capability_id) => principal.capabilities.contains(capability_id),
        GroupRule::Any(rules) => rules
            .iter()
            .any(|rule| group_rule_contains(rule, principal)),
        GroupRule::All(rules) => rules
            .iter()
            .all(|rule| group_rule_contains(rule, principal)),
    }
}

fn finish(issues: Vec<ContractIssue>) -> ContractResult {
    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

impl fmt::Display for PermissionResource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(scope: PolicyScope, rules: Vec<PermissionRule>) -> PermissionPolicy {
        PermissionPolicy {
            id: PolicyId::from_bytes(
                [if scope == PolicyScope::NodeLocal {
                    1
                } else {
                    2
                }; 16],
            ),
            scope,
            revision: 1,
            rules,
            created_at: UnixMillis(1),
            expires_at: None,
        }
    }

    fn rule(
        id: &str,
        subject: SubjectSelector,
        resource: PermissionResource,
        action: PermissionAction,
        effect: RuleEffect,
    ) -> PermissionRule {
        PermissionRule {
            id: id.into(),
            subjects: BTreeSet::from([subject]),
            resources: BTreeSet::from([resource]),
            actions: BTreeSet::from([action]),
            effect,
            condition: PolicyCondition::Always,
        }
    }

    fn principal(role: NetworkRole) -> PermissionPrincipal {
        let mut principal = PermissionPrincipal::new(NodeId::from_bytes([3; 32]));
        principal.roles.insert(role);
        principal.is_owner = role == NetworkRole::Owner;
        principal
    }

    #[test]
    fn deny_wins_inside_one_policy() {
        let policy = policy(
            PolicyScope::Network,
            vec![
                rule(
                    "allow",
                    SubjectSelector::AnyMember,
                    PermissionResource::Network,
                    PermissionAction::View,
                    RuleEffect::Allow,
                ),
                rule(
                    "deny",
                    SubjectSelector::Role(NetworkRole::Member),
                    PermissionResource::Network,
                    PermissionAction::View,
                    RuleEffect::Deny,
                ),
            ],
        );
        let decision = policy.evaluate(
            &principal(NetworkRole::Member),
            &PermissionResource::Network,
            PermissionAction::View,
            &AccessContext::default(),
            UnixMillis(2),
        );
        assert!(matches!(
            decision,
            PolicyDecision::Deny { reason, .. } if reason == "explicit_deny"
        ));
    }

    #[test]
    fn node_deny_cannot_be_overridden_by_network_owner_allow() {
        let node_local = policy(
            PolicyScope::NodeLocal,
            vec![rule(
                "local-deny",
                SubjectSelector::AnyMember,
                PermissionResource::TrafficPolicy,
                PermissionAction::Update,
                RuleEffect::Deny,
            )],
        );
        let network = policy(
            PolicyScope::Network,
            vec![rule(
                "owner-allow",
                SubjectSelector::Owner,
                PermissionResource::TrafficPolicy,
                PermissionAction::Update,
                RuleEffect::Allow,
            )],
        );
        let layered = LayeredPermissionPolicy {
            node_local,
            network,
        };
        let decision = layered.authorize(
            &principal(NetworkRole::Owner),
            &PermissionResource::TrafficPolicy,
            PermissionAction::Update,
            &AccessContext::default(),
            UnixMillis(2),
        );
        assert!(!decision.allowed);
        assert_eq!(decision.reason, "node_local_explicit_deny");
    }

    #[test]
    fn network_deny_cannot_be_overridden_by_local_allow() {
        let node_local = policy(
            PolicyScope::NodeLocal,
            vec![rule(
                "local-allow",
                SubjectSelector::Owner,
                PermissionResource::SecurityPolicy,
                PermissionAction::Update,
                RuleEffect::Allow,
            )],
        );
        let network = policy(
            PolicyScope::Network,
            vec![rule(
                "network-deny",
                SubjectSelector::AnyMember,
                PermissionResource::SecurityPolicy,
                PermissionAction::Update,
                RuleEffect::Deny,
            )],
        );
        let layered = LayeredPermissionPolicy {
            node_local,
            network,
        };
        let decision = layered.authorize(
            &principal(NetworkRole::Owner),
            &PermissionResource::SecurityPolicy,
            PermissionAction::Update,
            &AccessContext::default(),
            UnixMillis(2),
        );
        assert!(!decision.allowed);
        assert_eq!(decision.reason, "network_explicit_deny");
    }

    #[test]
    fn missing_allow_in_either_layer_defaults_to_deny() {
        let node_local = policy(
            PolicyScope::NodeLocal,
            vec![rule(
                "local-allow",
                SubjectSelector::AnyMember,
                PermissionResource::Audit,
                PermissionAction::View,
                RuleEffect::Allow,
            )],
        );
        let network = policy(PolicyScope::Network, Vec::new());
        let decision = LayeredPermissionPolicy {
            node_local,
            network,
        }
        .authorize(
            &principal(NetworkRole::Member),
            &PermissionResource::Audit,
            PermissionAction::View,
            &AccessContext::default(),
            UnixMillis(2),
        );
        assert!(!decision.allowed);
        assert_eq!(decision.reason, "joint_allow_required");
    }

    #[test]
    fn role_action_object_matrix_is_stable() {
        let cases = [
            (
                NetworkRole::Owner,
                PermissionResource::SecurityPolicy,
                PermissionAction::Update,
                true,
            ),
            (
                NetworkRole::Administrator,
                PermissionResource::Membership,
                PermissionAction::Approve,
                true,
            ),
            (
                NetworkRole::Administrator,
                PermissionResource::SecurityPolicy,
                PermissionAction::Update,
                true,
            ),
            (
                NetworkRole::Member,
                PermissionResource::Topology,
                PermissionAction::View,
                true,
            ),
            (
                NetworkRole::Member,
                PermissionResource::Membership,
                PermissionAction::Approve,
                false,
            ),
            (
                NetworkRole::TemporaryCoordinator,
                PermissionResource::CoordinatorTerm,
                PermissionAction::Manage,
                true,
            ),
            (
                NetworkRole::TemporaryCoordinator,
                PermissionResource::SecurityPolicy,
                PermissionAction::Update,
                false,
            ),
        ];

        for (role, resource, action, expected) in cases {
            let mut rules = Vec::new();
            match role {
                NetworkRole::Owner => rules.push(rule(
                    "owner",
                    SubjectSelector::Role(NetworkRole::Owner),
                    PermissionResource::SecurityPolicy,
                    PermissionAction::Update,
                    RuleEffect::Allow,
                )),
                NetworkRole::Administrator => {
                    rules.push(rule(
                        "admin-members",
                        SubjectSelector::Role(NetworkRole::Administrator),
                        PermissionResource::Membership,
                        PermissionAction::Approve,
                        RuleEffect::Allow,
                    ));
                    rules.push(rule(
                        "admin-security",
                        SubjectSelector::Role(NetworkRole::Administrator),
                        PermissionResource::SecurityPolicy,
                        PermissionAction::Update,
                        RuleEffect::Allow,
                    ));
                }
                NetworkRole::Member => rules.push(rule(
                    "member-topology",
                    SubjectSelector::Role(NetworkRole::Member),
                    PermissionResource::Topology,
                    PermissionAction::View,
                    RuleEffect::Allow,
                )),
                NetworkRole::TemporaryCoordinator => rules.push(rule(
                    "temporary-coordinator",
                    SubjectSelector::Role(NetworkRole::TemporaryCoordinator),
                    PermissionResource::CoordinatorTerm,
                    PermissionAction::Manage,
                    RuleEffect::Allow,
                )),
            }
            let decision = policy(PolicyScope::Network, rules).evaluate(
                &principal(role),
                &resource,
                action,
                &AccessContext::default(),
                UnixMillis(2),
            );
            assert_eq!(
                matches!(decision, PolicyDecision::Allow { .. }),
                expected,
                "case: {role:?} {action:?} {resource}"
            );
        }
    }

    #[test]
    fn dynamic_group_tracks_roles_without_copying_membership() {
        let group = GroupDefinition {
            id: GroupId::from_bytes([4; 16]),
            name: "Admins".into(),
            rule: GroupRule::Role(NetworkRole::Administrator),
        };
        assert!(group.contains(&principal(NetworkRole::Administrator)));
        assert!(!group.contains(&principal(NetworkRole::Member)));
    }
}
