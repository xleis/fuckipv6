use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ids::{GroupId, NetworkId, NodeId, SecurityRuleId};
use super::{DomainError, DomainResult};

/// Durable network roles plus the runtime-only emergency Coordinator role.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum NetworkRole {
    Owner,
    Administrator,
    Member,
    TemporaryCoordinator,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PolicyAction {
    Read,
    Create,
    Update,
    Delete,
    Approve,
    Reject,
    Remove,
    Enable,
    Disable,
    Use,
    Recover,
    Export,
    Test,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Network,
    Ownership,
    Membership,
    Group,
    SecurityPolicy,
    TrafficPolicy,
    Capability,
    ExitGrant,
    FileShare,
    Audit,
    CoordinatorTerm,
    Keys,
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
pub struct ResourceRef {
    pub kind: ResourceKind,
    pub network_id: NetworkId,
    /// Opaque object identifier. It is not interpreted by the policy engine.
    pub object_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionEffect {
    Allow,
    Deny,
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SubjectSelector {
    AnyMember,
    Role(NetworkRole),
    Node(NodeId),
    Group(GroupId),
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum PermissionCondition {
    MembershipActive,
    RecentReauthentication,
    ResourceInNetwork(NetworkId),
    NonEmergencyCoordinator,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct PermissionRule {
    pub rule_id: SecurityRuleId,
    pub effect: PermissionEffect,
    pub subjects: Vec<SubjectSelector>,
    pub resources: Vec<ResourceKind>,
    pub actions: Vec<PolicyAction>,
    #[serde(default)]
    pub conditions: Vec<PermissionCondition>,
}

impl PermissionRule {
    pub fn validate(&self) -> DomainResult<()> {
        if self.subjects.is_empty() || self.resources.is_empty() || self.actions.is_empty() {
            return Err(DomainError::invalid_value(
                "permission rule selectors cannot be empty",
            ));
        }
        if has_duplicates(&self.subjects)
            || has_duplicates(&self.resources)
            || has_duplicates(&self.actions)
            || has_duplicates(&self.conditions)
        {
            return Err(DomainError::invariant(
                "permission rule contains duplicate selectors",
            ));
        }
        Ok(())
    }

    fn matches(&self, request: &PermissionRequest, context: &AccessContext) -> bool {
        self.actions.contains(&request.action)
            && self.resources.contains(&request.resource.kind)
            && self.subjects.iter().any(|subject| subject.matches(context))
            && self
                .conditions
                .iter()
                .all(|condition| condition.matches(request, context))
    }
}

impl SubjectSelector {
    pub(crate) fn matches(&self, context: &AccessContext) -> bool {
        match self {
            Self::AnyMember => true,
            Self::Role(role) => *role == context.role,
            Self::Node(node_id) => *node_id == context.actor,
            Self::Group(group_id) => context.groups.contains(group_id),
        }
    }
}

impl PermissionCondition {
    fn matches(&self, request: &PermissionRequest, context: &AccessContext) -> bool {
        match self {
            Self::MembershipActive => context.membership_active,
            Self::RecentReauthentication => context.recent_reauthentication,
            Self::ResourceInNetwork(network_id) => request.resource.network_id == *network_id,
            Self::NonEmergencyCoordinator => !context.emergency_coordinator,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupKind {
    Manual,
    Dynamic,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct DynamicGroupSelector {
    #[serde(default)]
    pub roles: Vec<NetworkRole>,
    #[serde(default)]
    pub nodes: Vec<NodeId>,
}

impl DynamicGroupSelector {
    pub fn matches(&self, role: NetworkRole, node_id: &NodeId) -> bool {
        self.roles.contains(&role) || self.nodes.contains(node_id)
    }

    pub fn validate(&self) -> DomainResult<()> {
        if self.roles.is_empty() && self.nodes.is_empty() {
            return Err(DomainError::invalid_value(
                "dynamic group selector cannot be empty",
            ));
        }
        if has_duplicates(&self.roles) || has_duplicates(&self.nodes) {
            return Err(DomainError::invariant(
                "dynamic group selector contains duplicates",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum GroupDefinition {
    Manual { members: Vec<NodeId> },
    Dynamic { selector: DynamicGroupSelector },
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkGroup {
    pub group_id: GroupId,
    pub network_id: NetworkId,
    pub name: String,
    pub definition: GroupDefinition,
}

impl NetworkGroup {
    pub fn validate(&self) -> DomainResult<()> {
        if self.name.is_empty() || self.name.len() > 80 {
            return Err(DomainError::invalid_value("network group name is invalid"));
        }
        match &self.definition {
            GroupDefinition::Manual { members } => {
                if has_duplicates(members) {
                    return Err(DomainError::invariant(
                        "manual group contains duplicate members",
                    ));
                }
            }
            GroupDefinition::Dynamic { selector } => selector.validate()?,
        }
        Ok(())
    }

    pub fn contains(&self, role: NetworkRole, node_id: &NodeId) -> bool {
        match &self.definition {
            GroupDefinition::Manual { members } => members.contains(node_id),
            GroupDefinition::Dynamic { selector } => selector.matches(role, node_id),
        }
    }

    pub fn kind(&self) -> GroupKind {
        match self.definition {
            GroupDefinition::Manual { .. } => GroupKind::Manual,
            GroupDefinition::Dynamic { .. } => GroupKind::Dynamic,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessContext {
    pub network_id: NetworkId,
    pub actor: NodeId,
    pub role: NetworkRole,
    pub groups: BTreeSet<GroupId>,
    pub membership_active: bool,
    pub recent_reauthentication: bool,
    pub emergency_coordinator: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRequest {
    pub action: PolicyAction,
    pub resource: ResourceRef,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum PermissionDecision {
    Allow,
    Deny {
        code: String,
        layer: &'static str,
        rule_id: Option<SecurityRuleId>,
    },
}

impl PermissionDecision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow)
    }

    fn deny(code: &str, layer: &'static str, rule_id: Option<SecurityRuleId>) -> Self {
        Self::Deny {
            code: code.to_owned(),
            layer,
            rule_id,
        }
    }
}

/// Evaluate local and network policy independently. Both layers must contain
/// at least one matching allow, and a matching deny in either layer wins.
/// This makes local and network denials non-overridable by the other layer.
pub fn evaluate_permissions(
    local_rules: &[PermissionRule],
    network_rules: &[PermissionRule],
    request: &PermissionRequest,
    context: &AccessContext,
) -> PermissionDecision {
    if request.resource.network_id != context.network_id {
        return PermissionDecision::deny("permission.cross_network_denied", "request", None);
    }
    let local = evaluate_permission_layer(local_rules, request, context, "local");
    if !local.is_allowed() {
        return local;
    }
    evaluate_permission_layer(network_rules, request, context, "network")
}

/// Evaluate one policy layer. Network-only policy documents use this helper;
/// local node policy is composed with it by `evaluate_permissions`.
pub fn evaluate_network_permissions(
    network_rules: &[PermissionRule],
    request: &PermissionRequest,
    context: &AccessContext,
) -> PermissionDecision {
    if request.resource.network_id != context.network_id {
        return PermissionDecision::deny("permission.cross_network_denied", "request", None);
    }
    evaluate_permission_layer(network_rules, request, context, "network")
}

fn evaluate_permission_layer(
    rules: &[PermissionRule],
    request: &PermissionRequest,
    context: &AccessContext,
    layer: &'static str,
) -> PermissionDecision {
    for rule in rules.iter().filter(|rule| rule.matches(request, context)) {
        if rule.effect == PermissionEffect::Deny {
            return PermissionDecision::deny(
                "permission.denied",
                layer,
                Some(rule.rule_id.clone()),
            );
        }
        if !role_ceiling_allows(context.role, request.action, request.resource.kind) {
            return PermissionDecision::deny(
                "permission.role_ceiling_exceeded",
                layer,
                Some(rule.rule_id.clone()),
            );
        }
    }
    if rules
        .iter()
        .any(|rule| rule.effect == PermissionEffect::Allow && rule.matches(request, context))
    {
        PermissionDecision::Allow
    } else {
        PermissionDecision::deny("permission.default_denied", layer, None)
    }
}

/// Hard upper bounds applied after rule matching. Unknown or newly added
/// roles/resources therefore fail closed until this matrix is explicitly
/// extended and tested.
pub fn role_ceiling_allows(
    role: NetworkRole,
    action: PolicyAction,
    resource: ResourceKind,
) -> bool {
    match role {
        NetworkRole::Owner => true,
        NetworkRole::Administrator => !matches!(
            (action, resource),
            (PolicyAction::Delete, ResourceKind::Network)
                | (_, ResourceKind::Ownership)
                | (_, ResourceKind::Keys)
                | (PolicyAction::Recover, _)
        ),
        NetworkRole::Member => matches!(
            (action, resource),
            (
                PolicyAction::Read,
                ResourceKind::Network
                    | ResourceKind::Membership
                    | ResourceKind::Capability
                    | ResourceKind::FileShare
                    | ResourceKind::TrafficPolicy
            ) | (
                PolicyAction::Use,
                ResourceKind::Capability | ResourceKind::ExitGrant | ResourceKind::FileShare
            )
        ),
        NetworkRole::TemporaryCoordinator => matches!(
            (action, resource),
            (
                PolicyAction::Read,
                ResourceKind::Network
                    | ResourceKind::Membership
                    | ResourceKind::SecurityPolicy
                    | ResourceKind::Capability
                    | ResourceKind::TrafficPolicy
                    | ResourceKind::CoordinatorTerm
            )
        ),
    }
}

fn has_duplicates<T: Ord>(values: &[T]) -> bool {
    let mut seen = BTreeSet::new();
    values.iter().any(|value| !seen.insert(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> (NetworkId, NodeId, GroupId) {
        (
            NetworkId::from_public_key(b"network"),
            NodeId::from_public_key(b"node"),
            GroupId::from_public_key(b"group"),
        )
    }

    fn context(network_id: NetworkId, actor: NodeId, role: NetworkRole) -> AccessContext {
        AccessContext {
            network_id,
            actor,
            role,
            groups: BTreeSet::new(),
            membership_active: true,
            recent_reauthentication: true,
            emergency_coordinator: false,
        }
    }

    fn rule(
        rule_id: &[u8],
        effect: PermissionEffect,
        subject: SubjectSelector,
        action: PolicyAction,
        resource: ResourceKind,
    ) -> PermissionRule {
        PermissionRule {
            rule_id: SecurityRuleId::from_public_key(rule_id),
            effect,
            subjects: vec![subject],
            resources: vec![resource],
            actions: vec![action],
            conditions: vec![PermissionCondition::MembershipActive],
        }
    }

    #[test]
    fn deny_wins_even_when_both_layers_allow() {
        let (network_id, actor, _) = ids();
        let request = PermissionRequest {
            action: PolicyAction::Update,
            resource: ResourceRef {
                kind: ResourceKind::SecurityPolicy,
                network_id: network_id.clone(),
                object_id: None,
            },
        };
        let context = context(
            network_id.clone(),
            actor.clone(),
            NetworkRole::Administrator,
        );
        let local_allow = rule(
            b"local-allow",
            PermissionEffect::Allow,
            SubjectSelector::Node(actor.clone()),
            PolicyAction::Update,
            ResourceKind::SecurityPolicy,
        );
        let network_allow = rule(
            b"network-allow",
            PermissionEffect::Allow,
            SubjectSelector::Role(NetworkRole::Administrator),
            PolicyAction::Update,
            ResourceKind::SecurityPolicy,
        );
        assert_eq!(
            evaluate_permissions(
                std::slice::from_ref(&local_allow),
                std::slice::from_ref(&network_allow),
                &request,
                &context
            ),
            PermissionDecision::Allow
        );
        let network_deny = rule(
            b"network-deny",
            PermissionEffect::Deny,
            SubjectSelector::Role(NetworkRole::Administrator),
            PolicyAction::Update,
            ResourceKind::SecurityPolicy,
        );
        assert!(!evaluate_permissions(
            &[local_allow],
            &[network_allow, network_deny],
            &request,
            &context
        )
        .is_allowed());
    }

    #[test]
    fn local_deny_cannot_be_overridden_and_missing_layer_allows_deny() {
        let (network_id, actor, _) = ids();
        let context = context(network_id.clone(), actor.clone(), NetworkRole::Member);
        let request = PermissionRequest {
            action: PolicyAction::Use,
            resource: ResourceRef {
                kind: ResourceKind::ExitGrant,
                network_id,
                object_id: None,
            },
        };
        let network_allow = rule(
            b"network",
            PermissionEffect::Allow,
            SubjectSelector::AnyMember,
            PolicyAction::Use,
            ResourceKind::ExitGrant,
        );
        assert!(!evaluate_permissions(
            &[],
            std::slice::from_ref(&network_allow),
            &request,
            &context
        )
        .is_allowed());
        let local_deny = rule(
            b"local",
            PermissionEffect::Deny,
            SubjectSelector::Node(actor),
            PolicyAction::Use,
            ResourceKind::ExitGrant,
        );
        assert!(
            !evaluate_permissions(&[local_deny], &[network_allow], &request, &context).is_allowed()
        );
    }

    #[test]
    fn role_ceiling_blocks_escalation_even_with_explicit_allow_rule() {
        let (network_id, actor, _) = ids();
        let context = context(network_id.clone(), actor, NetworkRole::Member);
        let request = PermissionRequest {
            action: PolicyAction::Update,
            resource: ResourceRef {
                kind: ResourceKind::SecurityPolicy,
                network_id,
                object_id: None,
            },
        };
        let allow = rule(
            b"bad-grant",
            PermissionEffect::Allow,
            SubjectSelector::AnyMember,
            PolicyAction::Update,
            ResourceKind::SecurityPolicy,
        );
        let decision = evaluate_permissions(
            std::slice::from_ref(&allow),
            std::slice::from_ref(&allow),
            &request,
            &context,
        );
        assert!(matches!(
            decision,
            PermissionDecision::Deny { code, .. } if code == "permission.role_ceiling_exceeded"
        ));
    }

    #[test]
    fn role_operation_resource_matrix_is_conservative() {
        assert!(role_ceiling_allows(
            NetworkRole::Owner,
            PolicyAction::Delete,
            ResourceKind::Network
        ));
        assert!(role_ceiling_allows(
            NetworkRole::Administrator,
            PolicyAction::Approve,
            ResourceKind::Membership
        ));
        assert!(!role_ceiling_allows(
            NetworkRole::Administrator,
            PolicyAction::Delete,
            ResourceKind::Network
        ));
        assert!(!role_ceiling_allows(
            NetworkRole::Administrator,
            PolicyAction::Update,
            ResourceKind::Ownership
        ));
        assert!(role_ceiling_allows(
            NetworkRole::Member,
            PolicyAction::Read,
            ResourceKind::Network
        ));
        assert!(!role_ceiling_allows(
            NetworkRole::Member,
            PolicyAction::Update,
            ResourceKind::SecurityPolicy
        ));
        assert!(role_ceiling_allows(
            NetworkRole::TemporaryCoordinator,
            PolicyAction::Read,
            ResourceKind::SecurityPolicy
        ));
        assert!(!role_ceiling_allows(
            NetworkRole::TemporaryCoordinator,
            PolicyAction::Update,
            ResourceKind::Membership
        ));
    }

    #[test]
    fn groups_support_manual_and_dynamic_membership() {
        let (network_id, actor, group_id) = ids();
        let manual = NetworkGroup {
            group_id: group_id.clone(),
            network_id: network_id.clone(),
            name: "operators".into(),
            definition: GroupDefinition::Manual {
                members: vec![actor.clone()],
            },
        };
        manual.validate().unwrap();
        assert!(manual.contains(NetworkRole::Member, &actor));

        let dynamic = NetworkGroup {
            group_id,
            network_id,
            name: "admins".into(),
            definition: GroupDefinition::Dynamic {
                selector: DynamicGroupSelector {
                    roles: vec![NetworkRole::Administrator],
                    nodes: Vec::new(),
                },
            },
        };
        dynamic.validate().unwrap();
        assert!(dynamic.contains(NetworkRole::Administrator, &actor));
        assert!(!dynamic.contains(NetworkRole::Member, &actor));
    }
}
