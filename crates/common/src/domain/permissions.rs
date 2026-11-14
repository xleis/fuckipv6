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

impl NetworkRole {
    pub const ALL: [Self; 4] = [
        Self::Owner,
        Self::Administrator,
        Self::Member,
        Self::TemporaryCoordinator,
    ];
}

/// One identity from one authorization layer. Node ownership and network
/// governance are deliberately separate variants: holding either never grants
/// the other, and a command may require both. Discovery guests are public-index
/// principals, not network members.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessPrincipal {
    NodeOwner,
    Network(NetworkRole),
    DiscoveryGuest,
    UnauthenticatedPublic,
}

impl AccessPrincipal {
    /// The network-governance role carried by this principal, if any. Node
    /// owners and discovery guests intentionally return `None`.
    pub const fn network_role(self) -> Option<NetworkRole> {
        match self {
            Self::Network(role) => Some(role),
            Self::NodeOwner | Self::DiscoveryGuest | Self::UnauthenticatedPublic => None,
        }
    }

    /// Discovery guests are not durable members and therefore cannot enter the
    /// member/network role matrix.
    pub const fn is_network_member(self) -> bool {
        matches!(self, Self::Network(_))
    }
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

impl PolicyAction {
    pub const ALL: [Self; 13] = [
        Self::Read,
        Self::Create,
        Self::Update,
        Self::Delete,
        Self::Approve,
        Self::Reject,
        Self::Remove,
        Self::Enable,
        Self::Disable,
        Self::Use,
        Self::Recover,
        Self::Export,
        Self::Test,
    ];
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
    Relay,
    ExitGrant,
    FileShare,
    Audit,
    CoordinatorTerm,
    Keys,
}

impl ResourceKind {
    pub const ALL: [Self; 13] = [
        Self::Network,
        Self::Ownership,
        Self::Membership,
        Self::Group,
        Self::SecurityPolicy,
        Self::TrafficPolicy,
        Self::Capability,
        Self::Relay,
        Self::ExitGrant,
        Self::FileShare,
        Self::Audit,
        Self::CoordinatorTerm,
        Self::Keys,
    ];
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
pub struct ResourceRef {
    /// The network authorization scope. Cross-network requests are rejected
    /// before any rule is evaluated.
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

    pub(crate) fn matches(&self, request: &PermissionRequest, context: &AccessContext) -> bool {
        self.actions.contains(&request.action)
            && self.resources.contains(&request.resource.kind)
            && self.subjects.iter().any(|subject| subject.matches(context))
            && self
                .conditions
                .iter()
                .all(|condition| condition.matches(request, context))
    }
}

/// One deterministic baseline rule for a durable membership role. Relay is
/// intentionally excluded: providing or consuming Relay is an independent
/// capability that must be granted explicitly by each authorization layer.
/// The rule grants only within the role ceiling; both the resource network and
/// active membership are conditions, so one network's role cannot authorize
/// another. Runtime authorization and the policy tester consume this same
/// baseline.
pub fn role_baseline_policy(network_id: &NetworkId, role: NetworkRole) -> Vec<PermissionRule> {
    let role_name = match role {
        NetworkRole::Owner => "owner",
        NetworkRole::Administrator => "administrator",
        NetworkRole::Member => "member",
        NetworkRole::TemporaryCoordinator => "temporary_coordinator",
    };
    vec![PermissionRule {
        rule_id: SecurityRuleId::from_public_key(
            format!("builtin-role:{network_id}:{role_name}").as_bytes(),
        ),
        effect: PermissionEffect::Allow,
        subjects: vec![SubjectSelector::Role(role)],
        resources: ResourceKind::ALL
            .into_iter()
            .filter(|kind| *kind != ResourceKind::Relay)
            .collect(),
        actions: PolicyAction::ALL.to_vec(),
        conditions: vec![
            PermissionCondition::MembershipActive,
            PermissionCondition::ResourceInNetwork(network_id.clone()),
        ],
    }]
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
    Nested,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicGroupSelector {
    #[serde(default)]
    pub roles: Vec<NetworkRole>,
    #[serde(default)]
    pub nodes: Vec<NodeId>,
    #[serde(default)]
    pub capabilities: Vec<super::capability::CapabilityKind>,
}

impl DynamicGroupSelector {
    /// Role/Node membership without capability data. Capability-aware callers
    /// must use [`Self::matches_member`].
    pub fn matches(&self, role: NetworkRole, node_id: &NodeId) -> bool {
        self.roles.contains(&role) || self.nodes.contains(node_id)
    }

    /// Structured selector matching. Every field is a declared, typed value;
    /// no expression language or script is ever evaluated.
    pub fn matches_member(
        &self,
        role: NetworkRole,
        node_id: &NodeId,
        capabilities: &BTreeSet<super::capability::CapabilityKind>,
    ) -> bool {
        self.roles.contains(&role)
            || self.nodes.contains(node_id)
            || capabilities
                .iter()
                .any(|capability| self.capabilities.contains(capability))
    }

    pub fn validate(&self) -> DomainResult<()> {
        if self.roles.is_empty() && self.nodes.is_empty() && self.capabilities.is_empty() {
            return Err(DomainError::invalid_value(
                "dynamic group selector cannot be empty",
            ));
        }
        if has_duplicates(&self.roles)
            || has_duplicates(&self.nodes)
            || has_duplicates(&self.capabilities)
        {
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
    Manual {
        members: Vec<NodeId>,
    },
    Dynamic {
        selector: DynamicGroupSelector,
    },
    /// Nested groups resolve to the union of their child groups. References
    /// are checked by `GroupCatalog` for existence, cycles and depth.
    Nested {
        members: Vec<GroupId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkGroup {
    pub group_id: GroupId,
    pub network_id: NetworkId,
    pub name: String,
    pub definition: GroupDefinition,
}

impl GroupDefinition {
    /// Validate the structured definition in isolation. Nested references are
    /// only checked for shape here; the catalogue additionally verifies that
    /// they exist and form a shallow, acyclic graph.
    pub fn validate(&self) -> DomainResult<()> {
        match self {
            Self::Manual { members } => {
                if has_duplicates(members) {
                    return Err(DomainError::invariant(
                        "manual group contains duplicate members",
                    ));
                }
            }
            Self::Dynamic { selector } => selector.validate()?,
            Self::Nested { members } => {
                if members.is_empty() {
                    return Err(DomainError::invalid_value(
                        "nested group must reference at least one group",
                    ));
                }
                if has_duplicates(members) {
                    return Err(DomainError::invariant(
                        "nested group contains duplicate references",
                    ));
                }
            }
        }
        Ok(())
    }
}

impl NetworkGroup {
    pub fn validate(&self) -> DomainResult<()> {
        if self.name.is_empty() || self.name.len() > 80 {
            return Err(DomainError::invalid_value("network group name is invalid"));
        }
        self.definition.validate()
    }

    pub fn contains(&self, role: NetworkRole, node_id: &NodeId) -> bool {
        match &self.definition {
            GroupDefinition::Manual { members } => members.contains(node_id),
            GroupDefinition::Dynamic { selector } => selector.matches(role, node_id),
            // Nested groups can only be resolved through `GroupCatalog`, which
            // has the full graph and cycle/depth checks.
            GroupDefinition::Nested { .. } => false,
        }
    }

    pub fn kind(&self) -> GroupKind {
        match self.definition {
            GroupDefinition::Manual { .. } => GroupKind::Manual,
            GroupDefinition::Dynamic { .. } => GroupKind::Dynamic,
            GroupDefinition::Nested { .. } => GroupKind::Nested,
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

/// The authorization impact of a request.
///
/// `Mutating` covers runtime/local mutations such as starting a network,
/// rebuilding TUN or changing live routes. `GovernanceWrite` is deliberately
/// narrower: it covers changes to long-lived membership, roles, policies,
/// keys, invitations and authorization configuration. A temporary Coordinator
/// may perform runtime coordination, but only governance writes are frozen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionIntent {
    ReadOnly,
    Mutating,
    GovernanceWrite,
}

impl PermissionIntent {
    pub const fn is_mutating(self) -> bool {
        !matches!(self, Self::ReadOnly)
    }

    pub const fn is_governance_write(self) -> bool {
        matches!(self, Self::GovernanceWrite)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRequest {
    pub action: PolicyAction,
    pub resource: ResourceRef,
    pub intent: PermissionIntent,
}

/// Stable authorization denial codes. Every authorization surface must return
/// one of these values rather than constructing ad-hoc strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, Serialize, Deserialize)]
pub enum PermissionDenialCode {
    #[serde(rename = "permission.authentication_required")]
    AuthenticationRequired,
    #[serde(rename = "permission.capability_denied")]
    CapabilityDenied,
    #[serde(rename = "permission.local_owner_required")]
    LocalOwnerRequired,
    #[serde(rename = "permission.recent_authentication_required")]
    RecentAuthenticationRequired,
    #[serde(rename = "permission.network_scope_denied")]
    NetworkScopeDenied,
    #[serde(rename = "permission.membership_inactive")]
    MembershipInactive,
    /// Legacy broad code retained for wire compatibility. New evaluations use
    /// `TemporaryCoordinatorGovernanceFrozen`, which does not imply that all
    /// runtime writes are forbidden.
    #[serde(rename = "permission.temporary_coordinator_read_only")]
    TemporaryCoordinatorReadOnly,
    #[serde(rename = "permission.temporary_coordinator_governance_frozen")]
    TemporaryCoordinatorGovernanceFrozen,
    #[serde(rename = "permission.role_ceiling_exceeded")]
    RoleCeilingExceeded,
    #[serde(rename = "permission.policy_denied")]
    PolicyDenied,
    #[serde(rename = "permission.cross_network_denied")]
    CrossNetworkDenied,
    #[serde(rename = "permission.denied")]
    Denied,
    #[serde(rename = "permission.default_denied")]
    DefaultDenied,
    #[serde(rename = "permission.field_denied")]
    FieldDenied,
    #[serde(rename = "permission.field_default_denied")]
    FieldDefaultDenied,
}

impl PermissionDenialCode {
    pub const ALL: [Self; 15] = [
        Self::AuthenticationRequired,
        Self::CapabilityDenied,
        Self::LocalOwnerRequired,
        Self::RecentAuthenticationRequired,
        Self::NetworkScopeDenied,
        Self::MembershipInactive,
        Self::TemporaryCoordinatorReadOnly,
        Self::TemporaryCoordinatorGovernanceFrozen,
        Self::RoleCeilingExceeded,
        Self::PolicyDenied,
        Self::CrossNetworkDenied,
        Self::Denied,
        Self::DefaultDenied,
        Self::FieldDenied,
        Self::FieldDefaultDenied,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AuthenticationRequired => "permission.authentication_required",
            Self::CapabilityDenied => "permission.capability_denied",
            Self::LocalOwnerRequired => "permission.local_owner_required",
            Self::RecentAuthenticationRequired => "permission.recent_authentication_required",
            Self::NetworkScopeDenied => "permission.network_scope_denied",
            Self::MembershipInactive => "permission.membership_inactive",
            Self::TemporaryCoordinatorReadOnly => "permission.temporary_coordinator_read_only",
            Self::TemporaryCoordinatorGovernanceFrozen => {
                "permission.temporary_coordinator_governance_frozen"
            }
            Self::RoleCeilingExceeded => "permission.role_ceiling_exceeded",
            Self::PolicyDenied => "permission.policy_denied",
            Self::CrossNetworkDenied => "permission.cross_network_denied",
            Self::Denied => "permission.denied",
            Self::DefaultDenied => "permission.default_denied",
            Self::FieldDenied => "permission.field_denied",
            Self::FieldDefaultDenied => "permission.field_default_denied",
        }
    }
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

    pub(crate) fn deny(
        code: PermissionDenialCode,
        layer: &'static str,
        rule_id: Option<SecurityRuleId>,
    ) -> Self {
        Self::Deny {
            code: code.as_str().to_owned(),
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
    compose_permission_layers(local_rules, network_rules, request, context)
}

/// Evaluate one policy layer. Network-only policy documents use this helper;
/// local node policy is composed with it by `evaluate_permissions`.
pub fn evaluate_network_permissions(
    network_rules: &[PermissionRule],
    request: &PermissionRequest,
    context: &AccessContext,
) -> PermissionDecision {
    if request.resource.network_id != context.network_id {
        return PermissionDecision::deny(PermissionDenialCode::CrossNetworkDenied, "request", None);
    }
    evaluate_permission_layer(network_rules, request, context, "network").into_decision()
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LayerDecision {
    Allow,
    ExplicitDeny(SecurityRuleId),
    TemporaryCoordinatorGovernanceFrozen,
    RoleCeilingExceeded(SecurityRuleId),
    DefaultDenied,
}

impl LayerDecision {
    fn into_decision(self) -> PermissionDecision {
        match self {
            Self::Allow => PermissionDecision::Allow,
            Self::ExplicitDeny(rule_id) => {
                PermissionDecision::deny(PermissionDenialCode::Denied, "network", Some(rule_id))
            }
            Self::TemporaryCoordinatorGovernanceFrozen => PermissionDecision::deny(
                PermissionDenialCode::TemporaryCoordinatorGovernanceFrozen,
                "network",
                None,
            ),
            Self::RoleCeilingExceeded(rule_id) => PermissionDecision::deny(
                PermissionDenialCode::RoleCeilingExceeded,
                "network",
                Some(rule_id),
            ),
            Self::DefaultDenied => {
                PermissionDecision::deny(PermissionDenialCode::DefaultDenied, "network", None)
            }
        }
    }
}

fn compose_permission_layers(
    local_rules: &[PermissionRule],
    network_rules: &[PermissionRule],
    request: &PermissionRequest,
    context: &AccessContext,
) -> PermissionDecision {
    if request.resource.network_id != context.network_id {
        return PermissionDecision::deny(PermissionDenialCode::CrossNetworkDenied, "request", None);
    }

    // Evaluate both layers before choosing a result. Returning a local default
    // denial early would hide a more specific network explicit denial or
    // temporary-Coordinator freeze from callers and audits.
    let local = evaluate_permission_layer(local_rules, request, context, "local");
    let network = evaluate_permission_layer(network_rules, request, context, "network");

    for (layer, decision) in [("local", &local), ("network", &network)] {
        if let LayerDecision::ExplicitDeny(rule_id) = decision {
            return PermissionDecision::deny(
                PermissionDenialCode::Denied,
                layer,
                Some(rule_id.clone()),
            );
        }
    }
    for (layer, decision) in [("local", &local), ("network", &network)] {
        if matches!(
            decision,
            LayerDecision::TemporaryCoordinatorGovernanceFrozen
        ) {
            return PermissionDecision::deny(
                PermissionDenialCode::TemporaryCoordinatorGovernanceFrozen,
                layer,
                None,
            );
        }
    }
    for (layer, decision) in [("local", &local), ("network", &network)] {
        if let LayerDecision::RoleCeilingExceeded(rule_id) = decision {
            return PermissionDecision::deny(
                PermissionDenialCode::RoleCeilingExceeded,
                layer,
                Some(rule_id.clone()),
            );
        }
    }
    for (layer, decision) in [("local", &local), ("network", &network)] {
        if matches!(decision, LayerDecision::DefaultDenied) {
            return PermissionDecision::deny(PermissionDenialCode::DefaultDenied, layer, None);
        }
    }
    PermissionDecision::Allow
}

fn evaluate_permission_layer(
    rules: &[PermissionRule],
    request: &PermissionRequest,
    context: &AccessContext,
    _layer: &'static str,
) -> LayerDecision {
    // Explicit denial is evaluated before every other gate. Rule ordering must
    // never turn a denied request into a role-ceiling or temporary-term error.
    for rule in rules.iter().filter(|rule| rule.matches(request, context)) {
        if rule.effect == PermissionEffect::Deny {
            return LayerDecision::ExplicitDeny(rule.rule_id.clone());
        }
    }
    if context.role == NetworkRole::TemporaryCoordinator && request.intent.is_governance_write() {
        return LayerDecision::TemporaryCoordinatorGovernanceFrozen;
    }
    for rule in rules.iter().filter(|rule| rule.matches(request, context)) {
        if rule.effect == PermissionEffect::Allow
            && !role_ceiling_allows(context.role, request.action, request.resource.kind)
        {
            return LayerDecision::RoleCeilingExceeded(rule.rule_id.clone());
        }
    }
    if rules
        .iter()
        .any(|rule| rule.effect == PermissionEffect::Allow && rule.matches(request, context))
    {
        LayerDecision::Allow
    } else {
        LayerDecision::DefaultDenied
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
                ResourceKind::Capability
                    | ResourceKind::Relay
                    | ResourceKind::ExitGrant
                    | ResourceKind::FileShare
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
            ) | (
                PolicyAction::Enable | PolicyAction::Disable | PolicyAction::Update,
                ResourceKind::Network
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
            intent: PermissionIntent::ReadOnly,
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
            intent: PermissionIntent::ReadOnly,
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
            intent: PermissionIntent::ReadOnly,
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

    fn expected_role_ceiling(
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
                    ResourceKind::Capability
                        | ResourceKind::Relay
                        | ResourceKind::ExitGrant
                        | ResourceKind::FileShare
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
                ) | (
                    PolicyAction::Enable | PolicyAction::Disable | PolicyAction::Update,
                    ResourceKind::Network
                )
            ),
        }
    }

    #[test]
    fn role_operation_resource_matrix_covers_every_combination() {
        let mut checked = 0;
        for role in NetworkRole::ALL {
            for action in PolicyAction::ALL {
                for resource in ResourceKind::ALL {
                    checked += 1;
                    assert_eq!(
                        role_ceiling_allows(role, action, resource),
                        expected_role_ceiling(role, action, resource),
                        "unexpected role ceiling for {role:?}/{action:?}/{resource:?}"
                    );
                }
            }
        }
        assert_eq!(checked, 4 * 13 * 13);
        assert_eq!(checked, 676);
    }

    #[test]
    fn durable_role_baseline_does_not_grant_relay_use() {
        let (network_id, actor, _) = ids();
        let request = PermissionRequest {
            action: PolicyAction::Use,
            resource: ResourceRef {
                kind: ResourceKind::Relay,
                network_id: network_id.clone(),
                object_id: Some("relay".into()),
            },
            intent: PermissionIntent::ReadOnly,
        };
        for role in NetworkRole::ALL {
            let baseline = role_baseline_policy(&network_id, role);
            assert!(
                baseline
                    .iter()
                    .all(|rule| !rule.resources.contains(&ResourceKind::Relay)),
                "durable role baseline unexpectedly grants Relay to {role:?}"
            );
            let context = context(network_id.clone(), actor.clone(), role);
            let decision = evaluate_permissions(&baseline, &baseline, &request, &context);
            assert!(!decision.is_allowed());
        }
    }

    #[test]
    fn access_principals_do_not_conflate_node_and_network_authority() {
        assert_eq!(AccessPrincipal::NodeOwner.network_role(), None);
        assert!(!AccessPrincipal::NodeOwner.is_network_member());
        assert_eq!(
            AccessPrincipal::Network(NetworkRole::Owner).network_role(),
            Some(NetworkRole::Owner)
        );
        assert!(AccessPrincipal::Network(NetworkRole::Member).is_network_member());
        assert_eq!(AccessPrincipal::DiscoveryGuest.network_role(), None);
        assert!(!AccessPrincipal::DiscoveryGuest.is_network_member());
        assert_eq!(AccessPrincipal::UnauthenticatedPublic.network_role(), None);
        assert!(!AccessPrincipal::UnauthenticatedPublic.is_network_member());
    }

    #[test]
    fn denial_codes_are_stable_and_unique() {
        let codes = PermissionDenialCode::ALL.map(PermissionDenialCode::as_str);
        assert_eq!(codes.len(), 15);
        assert!(codes.iter().all(|code| code.starts_with("permission.")));
        assert_eq!(
            codes.iter().copied().collect::<BTreeSet<_>>().len(),
            codes.len()
        );
    }

    #[test]
    fn explicit_network_denial_is_not_masked_by_local_default_denial() {
        let (network_id, actor, _) = ids();
        let context = context(
            network_id.clone(),
            actor.clone(),
            NetworkRole::Administrator,
        );
        let request = PermissionRequest {
            action: PolicyAction::Update,
            resource: ResourceRef {
                kind: ResourceKind::SecurityPolicy,
                network_id,
                object_id: None,
            },
            intent: PermissionIntent::Mutating,
        };
        let network_deny = rule(
            b"network-explicit-deny",
            PermissionEffect::Deny,
            SubjectSelector::AnyMember,
            PolicyAction::Update,
            ResourceKind::SecurityPolicy,
        );
        let decision = evaluate_permissions(&[], &[network_deny], &request, &context);
        assert!(matches!(
            decision,
            PermissionDecision::Deny {
                code,
                layer: "network",
                rule_id: Some(_)
            } if code == PermissionDenialCode::Denied.as_str()
        ));
    }

    #[test]
    fn temporary_coordinator_governance_write_is_frozen_after_explicit_denial() {
        let (network_id, actor, _) = ids();
        let context = context(
            network_id.clone(),
            actor.clone(),
            NetworkRole::TemporaryCoordinator,
        );
        let read_request = PermissionRequest {
            action: PolicyAction::Read,
            resource: ResourceRef {
                kind: ResourceKind::SecurityPolicy,
                network_id: network_id.clone(),
                object_id: None,
            },
            intent: PermissionIntent::ReadOnly,
        };
        let read_allow = rule(
            b"temporary-read-policy",
            PermissionEffect::Allow,
            SubjectSelector::AnyMember,
            PolicyAction::Read,
            ResourceKind::SecurityPolicy,
        );
        assert_eq!(
            evaluate_permissions(
                std::slice::from_ref(&read_allow),
                std::slice::from_ref(&read_allow),
                &read_request,
                &context,
            ),
            PermissionDecision::Allow
        );

        let governance_request = PermissionRequest {
            action: PolicyAction::Update,
            resource: ResourceRef {
                kind: ResourceKind::SecurityPolicy,
                network_id: network_id.clone(),
                object_id: None,
            },
            intent: PermissionIntent::GovernanceWrite,
        };
        let governance_allow = rule(
            b"temporary-governance-policy",
            PermissionEffect::Allow,
            SubjectSelector::AnyMember,
            PolicyAction::Update,
            ResourceKind::SecurityPolicy,
        );
        assert!(matches!(
            evaluate_permissions(
                std::slice::from_ref(&governance_allow),
                std::slice::from_ref(&governance_allow),
                &governance_request,
                &context,
            ),
            PermissionDecision::Deny { code, .. }
                if code == PermissionDenialCode::TemporaryCoordinatorGovernanceFrozen.as_str()
        ));

        let runtime_request = PermissionRequest {
            action: PolicyAction::Update,
            resource: ResourceRef {
                kind: ResourceKind::Network,
                network_id,
                object_id: None,
            },
            intent: PermissionIntent::Mutating,
        };
        let runtime_allow = rule(
            b"temporary-runtime-network",
            PermissionEffect::Allow,
            SubjectSelector::AnyMember,
            PolicyAction::Update,
            ResourceKind::Network,
        );
        assert_eq!(
            evaluate_permissions(
                std::slice::from_ref(&runtime_allow),
                std::slice::from_ref(&runtime_allow),
                &runtime_request,
                &context,
            ),
            PermissionDecision::Allow
        );
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
                    capabilities: Vec::new(),
                },
            },
        };
        dynamic.validate().unwrap();
        assert!(dynamic.contains(NetworkRole::Administrator, &actor));
        assert!(!dynamic.contains(NetworkRole::Member, &actor));
    }
}
