//! Server-side authorization for the shared node command bus.
//!
//! Authorization context is transport-owned and deliberately not part of any
//! request body. A UI may hide or show an action, but the command bus always
//! re-evaluates the authenticated actor, capability, network scope and role
//! ceiling before a handler is dispatched.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use fuckipv6_common::domain::{
    evaluate_permissions, role_ceiling_allows, AccessContext, GroupId, MembershipState, NetworkId,
    NetworkRole, OperationId, PermissionDecision, PermissionRequest, PermissionRule, PolicyAction,
    ResourceKind, ResourceRef,
};

use super::command::{NodeCommand, NodeCommandKind};
use super::state::{NodePermission, NodeSnapshot};

/// A trusted, adapter-constructed command principal. The fields are private so
/// callers cannot turn untrusted wire data into an owner context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeCommandContext {
    actor: fuckipv6_common::domain::NodeId,
    source: String,
    capabilities: HashSet<NodePermission>,
    local_policy: Vec<PermissionRule>,
    network_scopes: BTreeMap<String, NetworkCommandScope>,
    recent_reauthentication: bool,
    local_owner: bool,
    authenticated: bool,
    audit_operation_id: Option<OperationId>,
}

/// One network membership resolved by the authenticated transport.
///
/// The policy rules and group memberships are copied from server-side state.
/// They are never accepted from a command payload or a UI capability hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkCommandScope {
    network_id: NetworkId,
    role: NetworkRole,
    membership_state: MembershipState,
    groups: BTreeSet<GroupId>,
    policy: Vec<PermissionRule>,
}

impl NetworkCommandScope {
    pub fn new(
        network_id: NetworkId,
        role: NetworkRole,
        membership_state: MembershipState,
        policy: Vec<PermissionRule>,
    ) -> Self {
        Self {
            network_id,
            role,
            membership_state,
            groups: BTreeSet::new(),
            policy,
        }
    }

    pub fn with_groups(mut self, groups: impl IntoIterator<Item = GroupId>) -> Self {
        self.groups = groups.into_iter().collect();
        self
    }

    pub fn network_id(&self) -> &NetworkId {
        &self.network_id
    }

    pub fn role(&self) -> NetworkRole {
        self.role
    }
}

impl NodeCommandContext {
    /// The local OS owner boundary: same-UID IPC, GPUI, CLI or a verified
    /// Passkey owner session. This principal owns the node and therefore may
    /// exercise the node's final local-control authority.
    pub fn local_owner(actor: fuckipv6_common::domain::NodeId, source: impl Into<String>) -> Self {
        Self {
            actor,
            source: source.into(),
            capabilities: NodePermission::ALL.into_iter().collect(),
            local_policy: Vec::new(),
            network_scopes: BTreeMap::new(),
            recent_reauthentication: true,
            local_owner: true,
            authenticated: true,
            audit_operation_id: None,
        }
    }

    /// A verified owner session with explicit reauthentication freshness.
    pub fn owner_session(
        actor: fuckipv6_common::domain::NodeId,
        source: impl Into<String>,
        recently_reauthenticated: bool,
    ) -> Self {
        let mut context = Self::local_owner(actor, source);
        context.recent_reauthentication = recently_reauthenticated;
        context
    }

    /// Derive the owner actor from the current node identity snapshot.
    pub fn local_owner_for_snapshot(snapshot: &NodeSnapshot, source: impl Into<String>) -> Self {
        Self::local_owner(actor_from_snapshot(snapshot), source)
    }

    /// Derive a verified WebUI owner context from the current node identity
    /// snapshot. Unlike the desktop OS-session boundary, WebUI sessions carry
    /// an explicit Passkey reauthentication freshness bit.
    pub fn owner_session_for_snapshot(
        snapshot: &NodeSnapshot,
        source: impl Into<String>,
        recently_reauthenticated: bool,
    ) -> Self {
        let mut context = Self::local_owner_for_snapshot(snapshot, source);
        context.recent_reauthentication = recently_reauthenticated;
        context
    }

    /// A network-scoped principal. Every map entry is an authenticated network
    /// binding resolved on the server; command payloads cannot widen it.
    pub fn network_session(
        actor: fuckipv6_common::domain::NodeId,
        source: impl Into<String>,
        capabilities: impl IntoIterator<Item = NodePermission>,
        scopes: impl IntoIterator<Item = (String, NetworkCommandScope)>,
        recently_reauthenticated: bool,
    ) -> Self {
        Self {
            actor,
            source: source.into(),
            capabilities: capabilities.into_iter().collect(),
            local_policy: Vec::new(),
            network_scopes: scopes.into_iter().collect(),
            recent_reauthentication: recently_reauthenticated,
            local_owner: false,
            authenticated: true,
            audit_operation_id: None,
        }
    }

    /// Attach the node-local policy layer. Network access succeeds only when
    /// both this layer and the matching network policy contain an allow and
    /// neither contains a deny.
    pub fn with_local_policy(mut self, policy: Vec<PermissionRule>) -> Self {
        self.local_policy = policy;
        self
    }

    /// A transport with no verified principal, such as the legacy loopback TCP
    /// compatibility endpoint.
    pub fn untrusted(source: impl Into<String>) -> Self {
        Self {
            actor: fuckipv6_common::domain::NodeId::from_public_key(
                b"fuckipv6-unauthenticated-command-actor",
            ),
            source: source.into(),
            capabilities: HashSet::new(),
            local_policy: Vec::new(),
            network_scopes: BTreeMap::new(),
            recent_reauthentication: false,
            local_owner: false,
            authenticated: false,
            audit_operation_id: None,
        }
    }

    /// Associate a trusted transport-created operation with the execution
    /// audit record. This value must never be sourced from request JSON.
    pub(crate) fn with_audit_operation_id(mut self, operation_id: OperationId) -> Self {
        self.audit_operation_id = Some(operation_id);
        self
    }

    pub(crate) fn audit_operation_id(&self) -> Option<&OperationId> {
        self.audit_operation_id.as_ref()
    }

    pub fn actor(&self) -> &fuckipv6_common::domain::NodeId {
        &self.actor
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// Render the effective role(s) for audit without inventing a single role
    /// when one session carries peer-specific bindings for several networks.
    pub(crate) fn roles_for_command(&self, command: &NodeCommand) -> String {
        if self.local_owner {
            return "owner".into();
        }
        let roles = match command_network_targets(command) {
            NetworkTargets::Named(names) => names
                .iter()
                .filter_map(|name| self.network_scopes.get(name))
                .map(|scope| role_name(scope.role))
                .collect::<BTreeSet<_>>(),
            NetworkTargets::None | NetworkTargets::All => BTreeSet::new(),
        };
        if roles.is_empty() {
            return "scoped".into();
        }
        roles.into_iter().collect::<Vec<_>>().join(",")
    }

    pub(crate) fn network_id_for_command(&self, command: &NodeCommand) -> Option<NetworkId> {
        let NetworkTargets::Named(names) = command_network_targets(command) else {
            return None;
        };
        let mut ids = names
            .iter()
            .filter_map(|name| self.network_scopes.get(name))
            .map(|scope| scope.network_id.clone());
        let first = ids.next()?;
        if ids.next().is_none() {
            Some(first)
        } else {
            None
        }
    }

    pub fn recently_reauthenticated(&self) -> bool {
        self.recent_reauthentication
    }

    pub fn is_local_owner(&self) -> bool {
        self.local_owner
    }
}

#[derive(Debug)]
pub(crate) struct CommandDenial {
    pub(crate) code: String,
    pub(crate) reason: String,
}

impl CommandDenial {
    fn new(code: &str, reason: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            reason: reason.into(),
        }
    }
}

pub(crate) fn authorize_command(
    context: &NodeCommandContext,
    command: &NodeCommand,
) -> Result<(), CommandDenial> {
    if !context.authenticated {
        return Err(CommandDenial::new(
            "permission.authentication_required",
            "actor is not authenticated",
        ));
    }

    let kind = command.kind();
    let permission = required_permission(kind);
    if !context.capabilities.contains(&permission) {
        return Err(CommandDenial::new(
            "permission.capability_denied",
            "required node capability is not granted",
        ));
    }

    // Local node ownership is the final local-control boundary. Network
    // governance can never revoke it, but it also cannot use this path to act on
    // a different node. Owner-only global projections remain owner-only even
    // when a network member has ViewState capability in one network.
    if is_local_owner_only(kind) && !context.local_owner {
        return Err(CommandDenial::new(
            "permission.local_owner_required",
            "command requires the local node owner",
        ));
    }

    if context.local_owner {
        if requires_recent_reauthentication(kind) && !context.recent_reauthentication {
            return Err(CommandDenial::new(
                "permission.recent_authentication_required",
                "recent owner reauthentication is required",
            ));
        }
        return Ok(());
    }

    let (action, resource) = policy_target(kind);
    let targets = command_network_targets(command);
    let names = match targets {
        NetworkTargets::Named(names) if !names.is_empty() => names,
        NetworkTargets::Named(_) | NetworkTargets::None => {
            return Err(CommandDenial::new(
                "permission.network_scope_denied",
                "network-scoped command requires at least one authenticated network target",
            ));
        }
        NetworkTargets::All => {
            return Err(CommandDenial::new(
                "permission.network_scope_denied",
                "network-scoped actor cannot select all networks",
            ));
        }
    };

    let mut seen = BTreeSet::new();
    for network_name in names {
        if !seen.insert(network_name.clone()) {
            continue;
        }
        let scope = context.network_scopes.get(&network_name).ok_or_else(|| {
            CommandDenial::new(
                "permission.network_scope_denied",
                "command target is outside the authenticated network scope",
            )
        })?;
        if !scope.membership_state.can_access_network_resources() {
            return Err(CommandDenial::new(
                "permission.membership_inactive",
                format!(
                    "network membership state does not grant resource access: {}",
                    membership_state_name(scope.membership_state)
                ),
            ));
        }
        if scope.role == NetworkRole::TemporaryCoordinator && kind.is_write() {
            return Err(CommandDenial::new(
                "permission.temporary_coordinator_read_only",
                "temporary coordinator is read-only",
            ));
        }
        if !role_ceiling_allows(scope.role, action, resource) {
            return Err(CommandDenial::new(
                "permission.role_ceiling_exceeded",
                format!(
                    "role {} exceeds the command policy ceiling",
                    role_name(scope.role)
                ),
            ));
        }

        let request = PermissionRequest {
            action,
            resource: ResourceRef {
                kind: resource,
                network_id: scope.network_id.clone(),
                object_id: None,
            },
        };
        let access = AccessContext {
            network_id: scope.network_id.clone(),
            actor: context.actor.clone(),
            role: scope.role,
            groups: scope.groups.clone(),
            membership_active: true,
            recent_reauthentication: context.recent_reauthentication,
            emergency_coordinator: scope.role == NetworkRole::TemporaryCoordinator,
        };
        let decision =
            evaluate_permissions(&context.local_policy, &scope.policy, &request, &access);
        if let PermissionDecision::Deny {
            code,
            layer,
            rule_id,
        } = decision
        {
            let stable_code = match code.as_str() {
                "permission.role_ceiling_exceeded" => "permission.role_ceiling_exceeded",
                "permission.cross_network_denied" => "permission.network_scope_denied",
                _ => "permission.policy_denied",
            };
            let reason = match rule_id {
                Some(rule_id) => {
                    format!("policy denied by {layer} layer rule {rule_id}: {network_name}")
                }
                None => format!("policy denied by {layer} layer: {network_name} ({code})"),
            };
            return Err(CommandDenial::new(stable_code, reason));
        }
    }

    Ok(())
}

fn role_name(role: NetworkRole) -> &'static str {
    match role {
        NetworkRole::Owner => "owner",
        NetworkRole::Administrator => "administrator",
        NetworkRole::Member => "member",
        NetworkRole::TemporaryCoordinator => "temporary_coordinator",
    }
}

fn membership_state_name(state: MembershipState) -> &'static str {
    match state {
        MembershipState::Discovered => "discovered",
        MembershipState::Applying => "applying",
        MembershipState::PendingApproval => "pending_approval",
        MembershipState::Accepted => "accepted",
        MembershipState::KeyReady => "key_ready",
        MembershipState::Connecting => "connecting",
        MembershipState::Active => "active",
        MembershipState::Degraded => "degraded",
        MembershipState::Leaving => "leaving",
        MembershipState::Left => "left",
        MembershipState::Rejected => "rejected",
        MembershipState::Suspended => "suspended",
        MembershipState::Cancelled => "cancelled",
    }
}

pub(crate) fn actor_from_snapshot(snapshot: &NodeSnapshot) -> fuckipv6_common::domain::NodeId {
    snapshot
        .identity
        .as_ref()
        .and_then(|identity| fuckipv6_common::domain::NodeId::parse(identity.node_id.as_str()).ok())
        .unwrap_or_else(|| {
            fuckipv6_common::domain::NodeId::from_public_key(b"fuckipv6-uninitialized-local-owner")
        })
}

fn required_permission(kind: NodeCommandKind) -> NodePermission {
    match kind {
        NodeCommandKind::GetSnapshot
        | NodeCommandKind::GetUiManifest
        | NodeCommandKind::GetDiagnostics
        | NodeCommandKind::GetBlacklist
        | NodeCommandKind::GetNetworkAllocation
        | NodeCommandKind::GetConfiguration
        | NodeCommandKind::PreviewConfiguration
        | NodeCommandKind::GetConfigurationHistory => NodePermission::ViewState,
        NodeCommandKind::UpdateRouting => NodePermission::ChangeRouting,
        NodeCommandKind::SetNetworkRole => NodePermission::ManageRoles,
        NodeCommandKind::ProbePeer => NodePermission::ProbePeers,
        NodeCommandKind::StopNode | NodeCommandKind::Shutdown => NodePermission::StopNode,
        NodeCommandKind::StartNetworks
        | NodeCommandKind::StopNetworks
        | NodeCommandKind::RestartNetworks
        | NodeCommandKind::RebuildTun
        | NodeCommandKind::CreateNetwork
        | NodeCommandKind::JoinNetwork
        | NodeCommandKind::BanClient
        | NodeCommandKind::UnbanClient
        | NodeCommandKind::UpdateNetworkAllocation
        | NodeCommandKind::SetNodeDisplayName
        | NodeCommandKind::StartNode
        | NodeCommandKind::RestartNode
        | NodeCommandKind::CleanupResiduals
        | NodeCommandKind::ApplyConfiguration
        | NodeCommandKind::RollbackConfiguration => NodePermission::ManageNetworks,
    }
}

/// Operations whose blast radius or ability to alter the trust boundary
/// requires a WebUI owner to have completed a Passkey ceremony recently.
/// Desktop local-owner contexts are always fresh because the OS user session
/// is their authentication boundary.
fn requires_recent_reauthentication(kind: NodeCommandKind) -> bool {
    matches!(
        kind,
        NodeCommandKind::Shutdown
            | NodeCommandKind::ApplyConfiguration
            | NodeCommandKind::RollbackConfiguration
            | NodeCommandKind::RestartNode
            | NodeCommandKind::RebuildTun
            | NodeCommandKind::SetNodeDisplayName
            | NodeCommandKind::CleanupResiduals
    )
}

fn is_local_owner_only(kind: NodeCommandKind) -> bool {
    matches!(
        kind,
        NodeCommandKind::GetSnapshot
            | NodeCommandKind::GetUiManifest
            | NodeCommandKind::GetDiagnostics
            | NodeCommandKind::RebuildTun
            | NodeCommandKind::CreateNetwork
            | NodeCommandKind::JoinNetwork
            | NodeCommandKind::SetNodeDisplayName
            | NodeCommandKind::StartNode
            | NodeCommandKind::StopNode
            | NodeCommandKind::RestartNode
            | NodeCommandKind::CleanupResiduals
            | NodeCommandKind::Shutdown
            | NodeCommandKind::GetConfiguration
            | NodeCommandKind::PreviewConfiguration
            | NodeCommandKind::ApplyConfiguration
            | NodeCommandKind::RollbackConfiguration
            | NodeCommandKind::GetConfigurationHistory
    )
}

fn policy_target(kind: NodeCommandKind) -> (PolicyAction, ResourceKind) {
    match kind {
        NodeCommandKind::GetSnapshot
        | NodeCommandKind::GetUiManifest
        | NodeCommandKind::GetDiagnostics
        | NodeCommandKind::GetBlacklist
        | NodeCommandKind::GetNetworkAllocation => (PolicyAction::Read, ResourceKind::Network),
        NodeCommandKind::GetConfiguration
        | NodeCommandKind::PreviewConfiguration
        | NodeCommandKind::GetConfigurationHistory => (PolicyAction::Read, ResourceKind::Keys),
        NodeCommandKind::ApplyConfiguration | NodeCommandKind::SetNodeDisplayName => {
            (PolicyAction::Update, ResourceKind::Keys)
        }
        NodeCommandKind::RollbackConfiguration => (PolicyAction::Recover, ResourceKind::Keys),
        NodeCommandKind::StartNetworks => (PolicyAction::Enable, ResourceKind::Network),
        NodeCommandKind::StopNetworks => (PolicyAction::Disable, ResourceKind::Network),
        NodeCommandKind::RestartNetworks
        | NodeCommandKind::RebuildTun
        | NodeCommandKind::UpdateNetworkAllocation
        | NodeCommandKind::RestartNode => (PolicyAction::Update, ResourceKind::Network),
        NodeCommandKind::CreateNetwork | NodeCommandKind::StartNode => {
            (PolicyAction::Create, ResourceKind::Network)
        }
        NodeCommandKind::JoinNetwork => (PolicyAction::Create, ResourceKind::Membership),
        NodeCommandKind::UpdateRouting => (PolicyAction::Update, ResourceKind::TrafficPolicy),
        NodeCommandKind::SetNetworkRole => (PolicyAction::Update, ResourceKind::Capability),
        NodeCommandKind::BanClient | NodeCommandKind::UnbanClient => {
            (PolicyAction::Update, ResourceKind::SecurityPolicy)
        }
        NodeCommandKind::ProbePeer => (PolicyAction::Test, ResourceKind::Capability),
        NodeCommandKind::StopNode | NodeCommandKind::Shutdown => {
            (PolicyAction::Disable, ResourceKind::Capability)
        }
        NodeCommandKind::CleanupResiduals => (PolicyAction::Recover, ResourceKind::Network),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NetworkTargets {
    None,
    Named(Vec<String>),
    All,
}

fn command_network_targets(command: &NodeCommand) -> NetworkTargets {
    match command {
        NodeCommand::GetSnapshot
        | NodeCommand::GetUiManifest
        | NodeCommand::GetDiagnostics
        | NodeCommand::CreateNetwork(_)
        | NodeCommand::JoinNetwork(_)
        | NodeCommand::SetNodeDisplayName(_)
        | NodeCommand::StartNode
        | NodeCommand::StopNode
        | NodeCommand::RestartNode
        | NodeCommand::Shutdown
        | NodeCommand::GetConfiguration
        | NodeCommand::PreviewConfiguration(_)
        | NodeCommand::ApplyConfiguration(_)
        | NodeCommand::RollbackConfiguration(_)
        | NodeCommand::GetConfigurationHistory => NetworkTargets::None,
        NodeCommand::StartNetworks { selection } => match selection {
            super::NodeNetworkSelection::Named(names) => NetworkTargets::Named(names.clone()),
            super::NodeNetworkSelection::None => NetworkTargets::None,
            super::NodeNetworkSelection::Default | super::NodeNetworkSelection::All => {
                NetworkTargets::All
            }
        },
        NodeCommand::StopNetworks { networks, all }
        | NodeCommand::RestartNetworks { networks, all } => {
            if *all {
                NetworkTargets::All
            } else {
                NetworkTargets::Named(networks.clone())
            }
        }
        NodeCommand::RebuildTun { network_name } => {
            NetworkTargets::Named(vec![network_name.clone()])
        }
        NodeCommand::UpdateRouting(command) => {
            NetworkTargets::Named(vec![command.network_name.clone()])
        }
        NodeCommand::SetNetworkRole(command) => {
            NetworkTargets::Named(vec![command.network_name.clone()])
        }
        NodeCommand::GetBlacklist { network_name }
        | NodeCommand::GetNetworkAllocation { network_name } => {
            NetworkTargets::Named(vec![network_name.clone()])
        }
        NodeCommand::BanClient(command) | NodeCommand::UnbanClient(command) => {
            NetworkTargets::Named(vec![command.network_name.clone()])
        }
        NodeCommand::UpdateNetworkAllocation(command) => {
            NetworkTargets::Named(vec![command.network_name.clone()])
        }
        NodeCommand::ProbePeer(command) => {
            NetworkTargets::Named(vec![command.network_name.clone()])
        }
        NodeCommand::CleanupResiduals { networks, all } => {
            if *all {
                NetworkTargets::All
            } else {
                NetworkTargets::Named(networks.clone())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuckipv6_common::domain::{
        PermissionCondition, PermissionEffect, SecurityRuleId, SubjectSelector,
    };

    fn actor() -> fuckipv6_common::domain::NodeId {
        fuckipv6_common::domain::NodeId::from_public_key(b"authorization-test-node")
    }

    fn network_id(name: &str) -> NetworkId {
        NetworkId::from_public_key(name.as_bytes())
    }

    fn rule(
        rule_id: &[u8],
        effect: PermissionEffect,
        subject: SubjectSelector,
        action: PolicyAction,
        resource: ResourceKind,
        conditions: Vec<PermissionCondition>,
    ) -> PermissionRule {
        PermissionRule {
            rule_id: SecurityRuleId::from_public_key(rule_id),
            effect,
            subjects: vec![subject],
            resources: vec![resource],
            actions: vec![action],
            conditions,
        }
    }

    fn allow(
        rule_id: &[u8],
        subject: SubjectSelector,
        action: PolicyAction,
        resource: ResourceKind,
    ) -> PermissionRule {
        rule(
            rule_id,
            PermissionEffect::Allow,
            subject,
            action,
            resource,
            vec![PermissionCondition::MembershipActive],
        )
    }

    fn deny(
        rule_id: &[u8],
        subject: SubjectSelector,
        action: PolicyAction,
        resource: ResourceKind,
    ) -> PermissionRule {
        rule(
            rule_id,
            PermissionEffect::Deny,
            subject,
            action,
            resource,
            vec![PermissionCondition::MembershipActive],
        )
    }

    fn network_context_with_groups(
        role: NetworkRole,
        capabilities: impl IntoIterator<Item = NodePermission>,
        networks: &[&str],
        membership_state: MembershipState,
        groups: impl IntoIterator<Item = GroupId>,
        policies: (Vec<PermissionRule>, Vec<PermissionRule>),
        recently_reauthenticated: bool,
    ) -> NodeCommandContext {
        let (local_policy, network_policy) = policies;
        let groups = groups.into_iter().collect::<BTreeSet<_>>();
        let scopes = networks.iter().map(|network| {
            (
                (*network).to_string(),
                NetworkCommandScope::new(
                    network_id(network),
                    role,
                    membership_state,
                    network_policy.clone(),
                )
                .with_groups(groups.iter().cloned()),
            )
        });
        NodeCommandContext::network_session(
            actor(),
            "authorization-test",
            capabilities,
            scopes,
            recently_reauthenticated,
        )
        .with_local_policy(local_policy)
    }

    fn network_context(
        role: NetworkRole,
        capabilities: impl IntoIterator<Item = NodePermission>,
        networks: &[&str],
        membership_state: MembershipState,
        local_policy: Vec<PermissionRule>,
        network_policy: Vec<PermissionRule>,
    ) -> NodeCommandContext {
        network_context_with_groups(
            role,
            capabilities,
            networks,
            membership_state,
            std::iter::empty(),
            (local_policy, network_policy),
            true,
        )
    }

    fn start_home() -> NodeCommand {
        NodeCommand::StartNetworks {
            selection: crate::node::NodeNetworkSelection::Named(vec!["home".into()]),
        }
    }

    fn start_policies() -> (Vec<PermissionRule>, Vec<PermissionRule>) {
        (
            vec![allow(
                b"local-start-networks",
                SubjectSelector::AnyMember,
                PolicyAction::Enable,
                ResourceKind::Network,
            )],
            vec![allow(
                b"network-start-networks",
                SubjectSelector::AnyMember,
                PolicyAction::Enable,
                ResourceKind::Network,
            )],
        )
    }

    fn assert_denied(context: &NodeCommandContext, command: NodeCommand, code: &str) {
        let denial = authorize_command(context, &command).expect_err("command must be denied");
        assert_eq!(denial.code.as_str(), code);
        assert!(!denial.reason.is_empty());
    }

    #[test]
    fn unauthenticated_transport_is_denied_before_capability_checks() {
        assert_denied(
            &NodeCommandContext::untrusted("legacy_loopback_tcp"),
            NodeCommand::GetSnapshot,
            "permission.authentication_required",
        );
    }

    #[test]
    fn inactive_membership_and_missing_capability_are_denied() {
        let (local_policy, network_policy) = start_policies();
        for membership_state in [
            MembershipState::Accepted,
            MembershipState::Connecting,
            MembershipState::Suspended,
        ] {
            assert_denied(
                &network_context(
                    NetworkRole::Administrator,
                    [NodePermission::ManageNetworks],
                    &["home"],
                    membership_state,
                    local_policy.clone(),
                    network_policy.clone(),
                ),
                start_home(),
                "permission.membership_inactive",
            );
        }
        assert_denied(
            &network_context(
                NetworkRole::Administrator,
                [],
                &["home"],
                MembershipState::Active,
                local_policy,
                network_policy,
            ),
            start_home(),
            "permission.capability_denied",
        );
    }

    #[test]
    fn local_owner_only_commands_cannot_be_used_by_network_members() {
        let (local_policy, network_policy) = (
            vec![allow(
                b"local-read-network",
                SubjectSelector::AnyMember,
                PolicyAction::Read,
                ResourceKind::Network,
            )],
            vec![allow(
                b"network-read-network",
                SubjectSelector::AnyMember,
                PolicyAction::Read,
                ResourceKind::Network,
            )],
        );
        assert_denied(
            &network_context(
                NetworkRole::Administrator,
                [NodePermission::ViewState],
                &["home"],
                MembershipState::Active,
                local_policy,
                network_policy,
            ),
            NodeCommand::GetDiagnostics,
            "permission.local_owner_required",
        );
    }

    #[test]
    fn temporary_coordinator_is_read_only() {
        let (local_policy, network_policy) = start_policies();
        assert_denied(
            &network_context(
                NetworkRole::TemporaryCoordinator,
                [NodePermission::ManageNetworks, NodePermission::ViewState],
                &["home"],
                MembershipState::Active,
                local_policy.clone(),
                network_policy,
            ),
            start_home(),
            "permission.temporary_coordinator_read_only",
        );

        authorize_command(
            &network_context(
                NetworkRole::TemporaryCoordinator,
                [NodePermission::ViewState],
                &["home"],
                MembershipState::Active,
                vec![allow(
                    b"local-read-network",
                    SubjectSelector::AnyMember,
                    PolicyAction::Read,
                    ResourceKind::Network,
                )],
                vec![allow(
                    b"network-read-network",
                    SubjectSelector::AnyMember,
                    PolicyAction::Read,
                    ResourceKind::Network,
                )],
            ),
            &NodeCommand::GetBlacklist {
                network_name: "home".into(),
            },
        )
        .expect("temporary coordinator can read network state");
    }

    #[test]
    fn network_scope_cannot_be_widened_by_the_command_payload() {
        let (local_policy, network_policy) = start_policies();
        let context = network_context(
            NetworkRole::Administrator,
            [NodePermission::ManageNetworks],
            &["home"],
            MembershipState::Active,
            local_policy,
            network_policy,
        );
        assert_denied(
            &context,
            NodeCommand::StartNetworks {
                selection: crate::node::NodeNetworkSelection::Named(vec![
                    "home".into(),
                    "office".into(),
                ]),
            },
            "permission.network_scope_denied",
        );
        assert_denied(
            &context,
            NodeCommand::StopNetworks {
                networks: Vec::new(),
                all: true,
            },
            "permission.network_scope_denied",
        );
    }

    #[test]
    fn local_owner_and_in_scope_administrator_are_allowed() {
        authorize_command(
            &NodeCommandContext::local_owner(actor(), "local-owner-test"),
            &NodeCommand::GetDiagnostics,
        )
        .expect("local owner can control the node");

        let (local_policy, network_policy) = start_policies();
        authorize_command(
            &network_context(
                NetworkRole::Administrator,
                [NodePermission::ManageNetworks],
                &["home"],
                MembershipState::Active,
                local_policy,
                network_policy,
            ),
            &start_home(),
        )
        .expect("administrator can act within its authenticated network scope");
    }

    #[test]
    fn tun_rebuild_is_reserved_for_the_local_owner() {
        let (local_policy, network_policy) = start_policies();
        assert_denied(
            &network_context(
                NetworkRole::Administrator,
                [NodePermission::ManageNetworks],
                &["home"],
                MembershipState::Active,
                local_policy,
                network_policy,
            ),
            NodeCommand::RebuildTun {
                network_name: "home".into(),
            },
            "permission.local_owner_required",
        );

        authorize_command(
            &NodeCommandContext::local_owner(actor(), "local-owner-test"),
            &NodeCommand::RebuildTun {
                network_name: "home".into(),
            },
        )
        .expect("local owner can rebuild a network TUN");
    }

    #[test]
    fn node_display_name_changes_are_reserved_for_the_local_owner() {
        let command = NodeCommand::SetNodeDisplayName(crate::node::NodeSetNodeDisplayNameCommand {
            display_name: Some("edge-gateway".into()),
        });
        let (local_policy, network_policy) = start_policies();
        assert_denied(
            &network_context(
                NetworkRole::Administrator,
                [NodePermission::ManageNetworks],
                &["home"],
                MembershipState::Active,
                local_policy,
                network_policy,
            ),
            command.clone(),
            "permission.local_owner_required",
        );

        authorize_command(
            &NodeCommandContext::local_owner(actor(), "local-owner-test"),
            &command,
        )
        .expect("local owner can rename the node identity");
    }

    #[test]
    fn desktop_local_owner_executes_sensitive_commands_without_web_authentication() {
        // GPUI is reachable only after the operating-system user session has
        // authenticated. Its local-owner context therefore carries the OS
        // trust boundary directly and must not depend on a WebUI Passkey session.
        let desktop_context = NodeCommandContext::local_owner(actor(), "gpui");

        assert!(desktop_context.is_local_owner());
        authorize_command(
            &desktop_context,
            &NodeCommand::RebuildTun {
                network_name: "home".into(),
            },
        )
        .expect("desktop owner can execute a local-owner-only command");
        authorize_command(
            &desktop_context,
            &NodeCommand::StartNetworks {
                selection: crate::node::NodeNetworkSelection::All,
            },
        )
        .expect("desktop owner can start networks without a WebUI session");
        authorize_command(
            &desktop_context,
            &NodeCommand::SetNodeDisplayName(crate::node::NodeSetNodeDisplayNameCommand {
                display_name: Some("desktop-owner".into()),
            }),
        )
        .expect("desktop owner can rename the node without a WebUI session");
    }

    #[test]
    fn stale_webui_owner_requires_reauthentication_only_for_sensitive_commands() {
        let mut context = NodeCommandContext::local_owner(actor(), "webui-public-owner");
        context.recent_reauthentication = false;

        assert_denied(
            &context,
            NodeCommand::Shutdown,
            "permission.recent_authentication_required",
        );
        authorize_command(&context, &NodeCommand::GetDiagnostics)
            .expect("stale WebUI owner can still execute ordinary read commands");

        context.recent_reauthentication = true;
        authorize_command(&context, &NodeCommand::Shutdown)
            .expect("recently reauthenticated WebUI owner can execute sensitive commands");
    }

    #[test]
    fn deny_wins_and_both_policy_layers_must_allow() {
        let action = PolicyAction::Enable;
        let resource = ResourceKind::Network;
        let local_allow = allow(b"local-allow", SubjectSelector::AnyMember, action, resource);
        let network_allow = allow(
            b"network-allow",
            SubjectSelector::AnyMember,
            action,
            resource,
        );
        let local_deny = deny(b"local-deny", SubjectSelector::AnyMember, action, resource);
        let network_deny = deny(
            b"network-deny",
            SubjectSelector::AnyMember,
            action,
            resource,
        );

        for (local_policy, network_policy) in [
            (
                vec![local_allow.clone(), local_deny],
                vec![network_allow.clone()],
            ),
            (
                vec![local_allow.clone()],
                vec![network_allow.clone(), network_deny],
            ),
            (Vec::new(), vec![network_allow.clone()]),
            (vec![local_allow], Vec::new()),
        ] {
            assert_denied(
                &network_context(
                    NetworkRole::Administrator,
                    [NodePermission::ManageNetworks],
                    &["home"],
                    MembershipState::Active,
                    local_policy,
                    network_policy,
                ),
                start_home(),
                "permission.policy_denied",
            );
        }
    }

    #[test]
    fn group_membership_and_reauthentication_conditions_are_enforced() {
        let subject = SubjectSelector::Group(GroupId::from_public_key(b"authorization-group"));
        let group = GroupId::from_public_key(b"authorization-group");
        let local_policy = vec![allow(
            b"local-allow",
            SubjectSelector::AnyMember,
            PolicyAction::Enable,
            ResourceKind::Network,
        )];
        let network_policy = vec![rule(
            b"network-group-reauth",
            PermissionEffect::Allow,
            subject,
            PolicyAction::Enable,
            ResourceKind::Network,
            vec![
                PermissionCondition::MembershipActive,
                PermissionCondition::RecentReauthentication,
            ],
        )];

        authorize_command(
            &network_context_with_groups(
                NetworkRole::Administrator,
                [NodePermission::ManageNetworks],
                &["home"],
                MembershipState::Active,
                [group.clone()],
                (local_policy.clone(), network_policy.clone()),
                true,
            ),
            &start_home(),
        )
        .expect("group member with fresh reauthentication is allowed");

        assert_denied(
            &network_context_with_groups(
                NetworkRole::Administrator,
                [NodePermission::ManageNetworks],
                &["home"],
                MembershipState::Active,
                [group],
                (local_policy, network_policy),
                false,
            ),
            start_home(),
            "permission.policy_denied",
        );
    }

    #[test]
    fn explicit_allow_cannot_escalate_the_network_role_ceiling() {
        let (local_policy, network_policy) = start_policies();
        assert_denied(
            &network_context(
                NetworkRole::Member,
                [NodePermission::ManageNetworks],
                &["home"],
                MembershipState::Active,
                local_policy,
                network_policy,
            ),
            start_home(),
            "permission.role_ceiling_exceeded",
        );
    }

    #[test]
    fn policy_condition_for_another_network_fails_closed() {
        let local_policy = vec![allow(
            b"local-allow",
            SubjectSelector::AnyMember,
            PolicyAction::Enable,
            ResourceKind::Network,
        )];
        let network_policy = vec![rule(
            b"network-other-network-only",
            PermissionEffect::Allow,
            SubjectSelector::AnyMember,
            PolicyAction::Enable,
            ResourceKind::Network,
            vec![
                PermissionCondition::MembershipActive,
                PermissionCondition::ResourceInNetwork(network_id("office")),
            ],
        )];
        assert_denied(
            &network_context(
                NetworkRole::Administrator,
                [NodePermission::ManageNetworks],
                &["home"],
                MembershipState::Active,
                local_policy,
                network_policy,
            ),
            start_home(),
            "permission.policy_denied",
        );
    }
}
