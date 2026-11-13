//! Server-side authorization for the shared node command bus.
//!
//! Authorization context is transport-owned and deliberately not part of any
//! request body. A UI may hide or show an action, but the command bus always
//! re-evaluates the authenticated actor, capability, network scope and role
//! ceiling before a handler is dispatched.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use fuckipv6_common::domain::{
    evaluate_effective_access, resolve_field_privacy_policy, role_baseline_policy, AccessContext,
    FieldPrivacyPolicy, FieldPrivacyPolicyError, GroupId, MembershipRole, MembershipState,
    NetworkAcl, NetworkId, NetworkRole, NodeLocalAccessPolicy, OperationId, PermissionDenialCode,
    PermissionIntent, PermissionRequest, PermissionRule, PolicyAction, PrivacyViewer, ResourceKind,
    ResourceRef,
};

use super::command::{NodeCommand, NodeCommandKind};
use super::state::{NodeAccessControlSnapshot, NodePermission, NodeSnapshot};

/// A trusted, adapter-constructed command principal. The fields are private so
/// callers cannot turn untrusted wire data into an owner context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeCommandContext {
    actor: fuckipv6_common::domain::NodeId,
    source: String,
    capabilities: HashSet<NodePermission>,
    local_policy: Vec<PermissionRule>,
    local_access_policy: Option<NodeLocalAccessPolicy>,
    access_control_invalid: bool,
    field_privacy_policies: Vec<FieldPrivacyPolicy>,
    field_privacy_invalid: bool,
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
    network_acl: Option<NetworkAcl>,
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
            network_acl: None,
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
            local_access_policy: None,
            access_control_invalid: false,
            field_privacy_policies: Vec::new(),
            field_privacy_invalid: false,
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
        let mut context = Self::local_owner(actor_from_snapshot(snapshot), source);
        for membership in &snapshot.memberships {
            let Ok(network_id) = NetworkId::parse(membership.network_id.as_str()) else {
                continue;
            };
            context.attach_role_baseline(
                membership.network_name.clone(),
                network_id,
                network_role_from_membership(membership.role),
                membership.state,
            );
        }
        context.with_access_control_snapshot(&snapshot.access_control)
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
        let mut network_scopes = BTreeMap::new();
        for (label, scope) in scopes {
            insert_network_scope(&mut network_scopes, label, scope);
        }
        Self {
            actor,
            source: source.into(),
            capabilities: capabilities.into_iter().collect(),
            local_policy: Vec::new(),
            local_access_policy: None,
            access_control_invalid: false,
            field_privacy_policies: Vec::new(),
            field_privacy_invalid: false,
            network_scopes,
            recent_reauthentication: recently_reauthenticated,
            local_owner: false,
            authenticated: true,
            audit_operation_id: None,
        }
    }

    /// Attach trusted network bindings to a local-owner session. Node ownership
    /// alone never creates a network role; transports must resolve these
    /// bindings from authenticated membership state rather than request data.
    pub fn with_network_scopes(
        mut self,
        scopes: impl IntoIterator<Item = (String, NetworkCommandScope)>,
    ) -> Self {
        for (label, scope) in scopes {
            insert_network_scope(&mut self.network_scopes, label, scope);
        }
        self
    }

    /// Attach the trusted baseline gate implied by one authenticated durable
    /// membership. This is deliberately not derived from a command payload or
    /// from node ownership: the network role comes from the membership record,
    /// and the hard ceiling still prevents that role from being widened.
    fn attach_role_baseline(
        &mut self,
        label: String,
        network_id: NetworkId,
        role: NetworkRole,
        membership_state: MembershipState,
    ) {
        let policy = role_baseline_policy(&network_id, role);
        self.local_policy.extend(policy.iter().cloned());
        insert_network_scope(
            &mut self.network_scopes,
            label,
            NetworkCommandScope::new(network_id, role, membership_state, policy),
        );
    }

    /// Test-only helper for command-bus journeys that intentionally start from
    /// a trusted local owner with an authoritative owner scope for each target.
    #[cfg(test)]
    pub(crate) fn with_test_network_authority_for(mut self, command: &NodeCommand) -> Self {
        if let NetworkTargets::Named(names) = command_network_targets(command) {
            for name in names {
                let network_id = NetworkId::from_public_key(name.as_bytes());
                self.attach_role_baseline(
                    name,
                    network_id,
                    NetworkRole::Owner,
                    MembershipState::Active,
                );
            }
        }
        self
    }

    /// Attach trusted local baseline rules. Adapter code supplies these from
    /// authenticated node state; command JSON can never call this method.
    #[cfg(test)]
    pub(crate) fn with_local_policy(mut self, policy: Vec<PermissionRule>) -> Self {
        self.local_policy = policy;
        self
    }

    /// Attach the owner-authored local restrictions and every network ACL from
    /// an authoritative snapshot. Missing ACLs mean "no additional ACL"; an
    /// ambiguous duplicate makes all policy-scoped commands fail closed.
    pub(crate) fn with_access_control_snapshot(
        mut self,
        snapshot: &NodeAccessControlSnapshot,
    ) -> Self {
        self.local_access_policy = snapshot.local_policy.clone();
        let mut invalid = false;
        for scope in self.network_scopes.values_mut() {
            match snapshot.network_acl_for(&scope.network_id) {
                Ok(acl) => scope.network_acl = acl.cloned(),
                Err(()) => {
                    invalid = true;
                    scope.network_acl = None;
                }
            }
        }
        self.access_control_invalid |= invalid;

        self.field_privacy_policies = snapshot.field_privacy_policies.clone();
        let mut seen = BTreeSet::new();
        let now = unix_time_ms();
        self.field_privacy_invalid = self.field_privacy_policies.iter().any(|policy| {
            !seen.insert(policy.network_id.clone()) || policy.validate_at(now).is_err()
        });
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
            local_access_policy: None,
            access_control_invalid: false,
            field_privacy_policies: Vec::new(),
            field_privacy_invalid: false,
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

    /// Build the verified disclosure viewer for one network boundary. A
    /// temporary coordinator remains a network member for read visibility and
    /// is never promoted to a durable administrator.
    pub(crate) fn privacy_viewer_for(&self, network_id: Option<&NetworkId>) -> PrivacyViewer {
        let scoped = network_id.and_then(|network_id| {
            self.network_scopes
                .values()
                .find(|scope| &scope.network_id == network_id)
        });
        if self.local_owner {
            return match (network_id, scoped) {
                (Some(network_id), Some(scope)) => PrivacyViewer::node_owner_in_network(
                    self.actor.clone(),
                    network_id.clone(),
                    membership_role_for_privacy(scope.role),
                    scope.membership_state,
                ),
                (Some(network_id), None) => {
                    PrivacyViewer::node_owner(self.actor.clone(), Some(network_id.clone()))
                }
                (None, _) => PrivacyViewer::node_owner(self.actor.clone(), None),
            };
        }
        match (network_id, scoped) {
            (Some(network_id), Some(scope)) => PrivacyViewer::network(
                self.actor.clone(),
                network_id.clone(),
                membership_role_for_privacy(scope.role),
                scope.membership_state,
            ),
            (Some(network_id), None) => {
                PrivacyViewer::public(self.actor.clone(), Some(network_id.clone()))
            }
            (None, _) => PrivacyViewer::public(self.actor.clone(), None),
        }
    }

    /// Resolve the one active field policy for a network. Ambiguous, expired,
    /// not-yet-effective or malformed policies fail closed.
    pub(crate) fn field_privacy_policy_for(
        &self,
        network_id: Option<&NetworkId>,
    ) -> Result<Option<&FieldPrivacyPolicy>, FieldPrivacyPolicyError> {
        if self.field_privacy_invalid {
            return Err(FieldPrivacyPolicyError::Invalid);
        }
        resolve_field_privacy_policy(&self.field_privacy_policies, network_id, unix_time_ms())
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
            PermissionDenialCode::AuthenticationRequired.as_str(),
            "actor is not authenticated",
        ));
    }

    if context.access_control_invalid {
        return Err(CommandDenial::new(
            PermissionDenialCode::PolicyDenied.as_str(),
            "trusted access control snapshot is ambiguous or invalid",
        ));
    }

    let kind = command.kind();
    let permission = required_permission(kind);
    if !context.capabilities.contains(&permission) {
        return Err(CommandDenial::new(
            PermissionDenialCode::CapabilityDenied.as_str(),
            "required node capability is not granted",
        ));
    }

    // Local node ownership is the final local-control boundary. Network
    // governance can never revoke it, but it also cannot use this path to act on
    // a different node. Owner-only global projections remain owner-only even
    // when a network member has ViewState capability in one network.
    if is_local_owner_only(kind) && !context.local_owner {
        return Err(CommandDenial::new(
            PermissionDenialCode::LocalOwnerRequired.as_str(),
            "command requires the local node owner",
        ));
    }

    let (action, resource) = policy_target_kind(kind);

    // Sensitive commands require a fresh WebUI ceremony regardless of whether
    // they are local host operations or network governance. Local owner status
    // is not a substitute for recent authentication.
    if kind.requires_recent_reauthentication() && !context.recent_reauthentication {
        return Err(CommandDenial::new(
            PermissionDenialCode::RecentAuthenticationRequired.as_str(),
            "recent owner reauthentication is required",
        ));
    }

    // Node ownership is local authority, not an implicit network role. Pure
    // host operations remain available to the owner, while network governance
    // must resolve an authenticated scope and pass both policy layers below.
    if context.local_owner && !is_network_governance(kind) {
        return Ok(());
    }
    let targets = command_network_targets(command);
    let names = match targets {
        NetworkTargets::Named(names) if !names.is_empty() => names,
        NetworkTargets::Named(_) | NetworkTargets::None => {
            return Err(CommandDenial::new(
                PermissionDenialCode::NetworkScopeDenied.as_str(),
                "network-scoped command requires at least one authenticated network target",
            ));
        }
        NetworkTargets::All => {
            return Err(CommandDenial::new(
                PermissionDenialCode::NetworkScopeDenied.as_str(),
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
                PermissionDenialCode::NetworkScopeDenied.as_str(),
                "command target is outside the authenticated network scope",
            )
        })?;
        if !scope.membership_state.can_access_network_resources() {
            return Err(CommandDenial::new(
                PermissionDenialCode::MembershipInactive.as_str(),
                format!(
                    "network membership state does not grant resource access: {}",
                    membership_state_name(scope.membership_state)
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
            intent: if kind.is_write() {
                PermissionIntent::Mutating
            } else {
                PermissionIntent::ReadOnly
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
        let decision = evaluate_effective_access(
            &context.local_policy,
            &scope.policy,
            scope.network_acl.as_ref(),
            context.local_access_policy.as_ref(),
            &request,
            &access,
            unix_time_ms(),
        );
        if !decision.allowed {
            let code = decision
                .code
                .as_deref()
                .unwrap_or(PermissionDenialCode::PolicyDenied.as_str());
            let reason = decision.explanation(&network_name);
            return Err(CommandDenial::new(code, reason));
        }
    }

    Ok(())
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn role_name(role: NetworkRole) -> &'static str {
    match role {
        NetworkRole::Owner => "owner",
        NetworkRole::Administrator => "administrator",
        NetworkRole::Member => "member",
        NetworkRole::TemporaryCoordinator => "temporary_coordinator",
    }
}

fn is_network_governance(kind: NodeCommandKind) -> bool {
    matches!(
        kind,
        NodeCommandKind::UpdateNetworkProfile
            | NodeCommandKind::PublishNetworkDiscovery
            | NodeCommandKind::UpdateNetworkDiscovery
            | NodeCommandKind::UnpublishNetworkDiscovery
            | NodeCommandKind::ReviewJoinRequest
            | NodeCommandKind::BatchReviewJoinRequests
            | NodeCommandKind::ChangeMemberLifecycle
            | NodeCommandKind::CreateInvitation
            | NodeCommandKind::RevokeInvitation
            | NodeCommandKind::SetNetworkRole
            | NodeCommandKind::BanClient
            | NodeCommandKind::UnbanClient
            | NodeCommandKind::UpdateNetworkAllocation
    )
}

fn insert_network_scope(
    scopes: &mut BTreeMap<String, NetworkCommandScope>,
    label: String,
    scope: NetworkCommandScope,
) {
    // Transports historically keyed scopes by the local network name, while
    // typed command payloads carry the stable network ID. Index both aliases
    // so a valid binding cannot fail open or be mistaken for an out-of-scope
    // target based only on which representation the command uses.
    scopes.insert(scope.network_id.to_string(), scope.clone());
    scopes.insert(label, scope);
}

fn network_role_from_membership(role: MembershipRole) -> NetworkRole {
    match role {
        MembershipRole::Owner => NetworkRole::Owner,
        MembershipRole::Administrator => NetworkRole::Administrator,
        MembershipRole::Member => NetworkRole::Member,
    }
}

fn membership_role_for_privacy(role: NetworkRole) -> MembershipRole {
    match role {
        NetworkRole::Owner => MembershipRole::Owner,
        NetworkRole::Administrator => MembershipRole::Administrator,
        // Emergency coordinators are temporary read principals, not durable
        // administrators. Their visibility ceiling is ordinary membership.
        NetworkRole::Member | NetworkRole::TemporaryCoordinator => MembershipRole::Member,
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
        | NodeCommandKind::PreviewNetworkProfile
        | NodeCommandKind::PreviewJoinNetwork
        | NodeCommandKind::ListJoinRequests
        | NodeCommandKind::ListNetworkMembers
        | NodeCommandKind::PreviewMemberRemoval
        | NodeCommandKind::ListInvitations
        | NodeCommandKind::GetInvitationStatus
        | NodeCommandKind::RefreshDiscoveryIndex
        | NodeCommandKind::RetryDiscoveryIndex
        | NodeCommandKind::GetConfigurationHistory
        | NodeCommandKind::GetPolicy
        | NodeCommandKind::PreviewPolicy
        | NodeCommandKind::PreviewPolicyRollback
        | NodeCommandKind::GetPolicyHistory
        | NodeCommandKind::GetTrafficPolicy
        | NodeCommandKind::GetTrafficPolicyHistory => NodePermission::ViewState,
        NodeCommandKind::PreviewTrafficPolicy
        | NodeCommandKind::PreviewTrafficPolicyRollback
        | NodeCommandKind::ApplyTrafficPolicy
        | NodeCommandKind::RollbackTrafficPolicy
        | NodeCommandKind::TestTrafficPolicy => NodePermission::ChangeRouting,
        NodeCommandKind::UpdateRouting => NodePermission::ChangeRouting,
        NodeCommandKind::SetNetworkRole => NodePermission::ManageRoles,
        NodeCommandKind::ProbePeer => NodePermission::ProbePeers,
        NodeCommandKind::StopNode | NodeCommandKind::Shutdown => NodePermission::StopNode,
        NodeCommandKind::StartNetworks
        | NodeCommandKind::StopNetworks
        | NodeCommandKind::RestartNetworks
        | NodeCommandKind::RebuildTun
        | NodeCommandKind::CreateNetwork
        | NodeCommandKind::UpdateNetworkProfile
        | NodeCommandKind::PublishNetworkDiscovery
        | NodeCommandKind::UpdateNetworkDiscovery
        | NodeCommandKind::UnpublishNetworkDiscovery
        | NodeCommandKind::JoinNetwork
        | NodeCommandKind::SubmitJoinRequest
        | NodeCommandKind::ReviewJoinRequest
        | NodeCommandKind::BatchReviewJoinRequests
        | NodeCommandKind::ChangeMemberLifecycle
        | NodeCommandKind::WithdrawJoinRequest
        | NodeCommandKind::ResubmitJoinRequest
        | NodeCommandKind::CreateInvitation
        | NodeCommandKind::RevokeInvitation
        | NodeCommandKind::PreviewInvitation
        | NodeCommandKind::RedeemInvitation
        | NodeCommandKind::LeaveNetwork
        | NodeCommandKind::AcknowledgeMembershipPolicy
        | NodeCommandKind::BanClient
        | NodeCommandKind::UnbanClient
        | NodeCommandKind::UpdateNetworkAllocation
        | NodeCommandKind::SetLocalExitCapability
        | NodeCommandKind::SetNodeDisplayName
        | NodeCommandKind::RotateNodeIdentity
        | NodeCommandKind::StartNode
        | NodeCommandKind::RestartNode
        | NodeCommandKind::CleanupResiduals
        | NodeCommandKind::ApplyConfiguration
        | NodeCommandKind::RollbackConfiguration
        | NodeCommandKind::ApplyPolicy
        | NodeCommandKind::RollbackPolicy
        | NodeCommandKind::TestPolicy => NodePermission::ManageNetworks,
    }
}

fn is_local_owner_only(kind: NodeCommandKind) -> bool {
    matches!(
        kind,
        NodeCommandKind::GetSnapshot
            | NodeCommandKind::GetUiManifest
            | NodeCommandKind::GetDiagnostics
            | NodeCommandKind::RebuildTun
            | NodeCommandKind::UpdateRouting
            | NodeCommandKind::CreateNetwork
            | NodeCommandKind::PreviewNetworkProfile
            | NodeCommandKind::UpdateNetworkProfile
            | NodeCommandKind::PublishNetworkDiscovery
            | NodeCommandKind::UpdateNetworkDiscovery
            | NodeCommandKind::UnpublishNetworkDiscovery
            | NodeCommandKind::RefreshDiscoveryIndex
            | NodeCommandKind::RetryDiscoveryIndex
            | NodeCommandKind::PreviewJoinNetwork
            | NodeCommandKind::JoinNetwork
            | NodeCommandKind::SubmitJoinRequest
            | NodeCommandKind::WithdrawJoinRequest
            | NodeCommandKind::ResubmitJoinRequest
            | NodeCommandKind::PreviewInvitation
            | NodeCommandKind::RedeemInvitation
            | NodeCommandKind::LeaveNetwork
            | NodeCommandKind::AcknowledgeMembershipPolicy
            | NodeCommandKind::SetLocalExitCapability
            | NodeCommandKind::SetNodeDisplayName
            | NodeCommandKind::RotateNodeIdentity
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
            | NodeCommandKind::GetPolicy
            | NodeCommandKind::PreviewPolicy
            | NodeCommandKind::PreviewPolicyRollback
            | NodeCommandKind::ApplyPolicy
            | NodeCommandKind::RollbackPolicy
            | NodeCommandKind::GetPolicyHistory
            | NodeCommandKind::GetTrafficPolicy
            | NodeCommandKind::PreviewTrafficPolicy
            | NodeCommandKind::PreviewTrafficPolicyRollback
            | NodeCommandKind::ApplyTrafficPolicy
            | NodeCommandKind::RollbackTrafficPolicy
            | NodeCommandKind::GetTrafficPolicyHistory
            | NodeCommandKind::TestTrafficPolicy
    )
}

fn policy_target_kind(kind: NodeCommandKind) -> (PolicyAction, ResourceKind) {
    match kind {
        NodeCommandKind::GetSnapshot
        | NodeCommandKind::GetUiManifest
        | NodeCommandKind::GetDiagnostics
        | NodeCommandKind::GetBlacklist
        | NodeCommandKind::GetNetworkAllocation
        | NodeCommandKind::PreviewNetworkProfile
        | NodeCommandKind::PreviewJoinNetwork
        | NodeCommandKind::ListJoinRequests
        | NodeCommandKind::ListNetworkMembers
        | NodeCommandKind::PreviewMemberRemoval
        | NodeCommandKind::RefreshDiscoveryIndex
        | NodeCommandKind::RetryDiscoveryIndex => (PolicyAction::Read, ResourceKind::Network),
        NodeCommandKind::GetConfiguration
        | NodeCommandKind::PreviewConfiguration
        | NodeCommandKind::GetConfigurationHistory => (PolicyAction::Read, ResourceKind::Keys),
        NodeCommandKind::GetPolicy
        | NodeCommandKind::PreviewPolicy
        | NodeCommandKind::PreviewPolicyRollback
        | NodeCommandKind::GetPolicyHistory => (PolicyAction::Read, ResourceKind::SecurityPolicy),
        NodeCommandKind::GetTrafficPolicy | NodeCommandKind::GetTrafficPolicyHistory => {
            (PolicyAction::Read, ResourceKind::TrafficPolicy)
        }
        NodeCommandKind::PreviewTrafficPolicy | NodeCommandKind::PreviewTrafficPolicyRollback => {
            (PolicyAction::Read, ResourceKind::TrafficPolicy)
        }
        NodeCommandKind::ApplyTrafficPolicy => (PolicyAction::Update, ResourceKind::TrafficPolicy),
        NodeCommandKind::RollbackTrafficPolicy => {
            (PolicyAction::Recover, ResourceKind::TrafficPolicy)
        }
        NodeCommandKind::TestTrafficPolicy => (PolicyAction::Test, ResourceKind::TrafficPolicy),
        NodeCommandKind::ApplyConfiguration
        | NodeCommandKind::SetNodeDisplayName
        | NodeCommandKind::RotateNodeIdentity => (PolicyAction::Update, ResourceKind::Keys),
        NodeCommandKind::RollbackConfiguration => (PolicyAction::Recover, ResourceKind::Keys),
        NodeCommandKind::ApplyPolicy => (PolicyAction::Update, ResourceKind::SecurityPolicy),
        NodeCommandKind::RollbackPolicy => (PolicyAction::Recover, ResourceKind::SecurityPolicy),
        NodeCommandKind::StartNetworks => (PolicyAction::Enable, ResourceKind::Network),
        NodeCommandKind::StopNetworks => (PolicyAction::Disable, ResourceKind::Network),
        NodeCommandKind::RestartNetworks
        | NodeCommandKind::RebuildTun
        | NodeCommandKind::UpdateNetworkAllocation
        | NodeCommandKind::UpdateNetworkProfile
        | NodeCommandKind::PublishNetworkDiscovery
        | NodeCommandKind::UpdateNetworkDiscovery
        | NodeCommandKind::UnpublishNetworkDiscovery
        | NodeCommandKind::RestartNode => (PolicyAction::Update, ResourceKind::Network),
        NodeCommandKind::CreateNetwork | NodeCommandKind::StartNode => {
            (PolicyAction::Create, ResourceKind::Network)
        }
        NodeCommandKind::JoinNetwork
        | NodeCommandKind::SubmitJoinRequest
        | NodeCommandKind::CreateInvitation
        | NodeCommandKind::RedeemInvitation => (PolicyAction::Create, ResourceKind::Membership),
        NodeCommandKind::ListInvitations
        | NodeCommandKind::GetInvitationStatus
        | NodeCommandKind::PreviewInvitation => (PolicyAction::Read, ResourceKind::Membership),
        NodeCommandKind::RevokeInvitation => (PolicyAction::Remove, ResourceKind::Membership),
        NodeCommandKind::ReviewJoinRequest
        | NodeCommandKind::BatchReviewJoinRequests
        | NodeCommandKind::ChangeMemberLifecycle
        | NodeCommandKind::WithdrawJoinRequest
        | NodeCommandKind::ResubmitJoinRequest => (PolicyAction::Update, ResourceKind::Membership),
        NodeCommandKind::LeaveNetwork => (PolicyAction::Remove, ResourceKind::Membership),
        NodeCommandKind::AcknowledgeMembershipPolicy => {
            (PolicyAction::Update, ResourceKind::Membership)
        }
        NodeCommandKind::UpdateRouting => (PolicyAction::Update, ResourceKind::TrafficPolicy),
        NodeCommandKind::SetNetworkRole | NodeCommandKind::SetLocalExitCapability => {
            (PolicyAction::Update, ResourceKind::Capability)
        }
        NodeCommandKind::BanClient | NodeCommandKind::UnbanClient => {
            (PolicyAction::Update, ResourceKind::SecurityPolicy)
        }
        NodeCommandKind::ProbePeer => (PolicyAction::Test, ResourceKind::Capability),
        NodeCommandKind::StopNode | NodeCommandKind::Shutdown => {
            (PolicyAction::Disable, ResourceKind::Capability)
        }
        NodeCommandKind::CleanupResiduals => (PolicyAction::Recover, ResourceKind::Network),
        NodeCommandKind::TestPolicy => (PolicyAction::Test, ResourceKind::SecurityPolicy),
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
        | NodeCommand::PreviewJoinNetwork(_)
        | NodeCommand::JoinNetwork(_)
        | NodeCommand::SetLocalExitCapability(_)
        | NodeCommand::SetNodeDisplayName(_)
        | NodeCommand::RotateNodeIdentity(_)
        | NodeCommand::StartNode
        | NodeCommand::StopNode
        | NodeCommand::RestartNode
        | NodeCommand::Shutdown
        | NodeCommand::GetConfiguration
        | NodeCommand::PreviewConfiguration(_)
        | NodeCommand::ApplyConfiguration(_)
        | NodeCommand::RollbackConfiguration(_)
        | NodeCommand::RefreshDiscoveryIndex { .. }
        | NodeCommand::RetryDiscoveryIndex
        | NodeCommand::GetConfigurationHistory
        | NodeCommand::GetPolicy
        | NodeCommand::PreviewPolicy(_)
        | NodeCommand::PreviewPolicyRollback(_)
        | NodeCommand::ApplyPolicy(_)
        | NodeCommand::RollbackPolicy(_)
        | NodeCommand::GetPolicyHistory
        | NodeCommand::GetTrafficPolicy
        | NodeCommand::PreviewTrafficPolicy(_)
        | NodeCommand::PreviewTrafficPolicyRollback(_)
        | NodeCommand::ApplyTrafficPolicy(_)
        | NodeCommand::RollbackTrafficPolicy(_)
        | NodeCommand::GetTrafficPolicyHistory
        | NodeCommand::TestTrafficPolicy(_) => NetworkTargets::None,
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
        NodeCommand::ListJoinRequests(command) => {
            NetworkTargets::Named(vec![command.network_id.clone()])
        }
        NodeCommand::ListNetworkMembers(command) => {
            NetworkTargets::Named(vec![command.network_id.clone()])
        }
        NodeCommand::PreviewMemberRemoval(command) => {
            NetworkTargets::Named(vec![command.network_id.clone()])
        }
        NodeCommand::ChangeMemberLifecycle(command) => {
            NetworkTargets::Named(vec![command.network_id.clone()])
        }
        NodeCommand::SubmitJoinRequest(command) => {
            NetworkTargets::Named(vec![command.network_id.clone()])
        }
        NodeCommand::ReviewJoinRequest(command) => {
            NetworkTargets::Named(vec![command.network_id.clone()])
        }
        NodeCommand::BatchReviewJoinRequests(command) => {
            NetworkTargets::Named(vec![command.network_id.clone()])
        }
        NodeCommand::WithdrawJoinRequest(command) => {
            NetworkTargets::Named(vec![command.network_id.clone()])
        }
        NodeCommand::ResubmitJoinRequest(command) => {
            NetworkTargets::Named(vec![command.network_id.clone()])
        }
        NodeCommand::TestPolicy(command) => {
            NetworkTargets::Named(vec![command.network_id.to_string()])
        }
        NodeCommand::CreateInvitation(command) => {
            NetworkTargets::Named(vec![command.network_id.to_string()])
        }
        NodeCommand::ListInvitations(command) => {
            NetworkTargets::Named(vec![command.network_id.to_string()])
        }
        NodeCommand::GetInvitationStatus(command) => {
            NetworkTargets::Named(vec![command.network_id.to_string()])
        }
        NodeCommand::RevokeInvitation(command) => {
            NetworkTargets::Named(vec![command.network_id.to_string()])
        }
        NodeCommand::PreviewInvitation(command) => {
            NetworkTargets::Named(vec![command.network_id.to_string()])
        }
        NodeCommand::RedeemInvitation(command) => {
            NetworkTargets::Named(vec![command.request.network_id.to_string()])
        }
        NodeCommand::LeaveNetwork(command) => {
            NetworkTargets::Named(vec![command.network_name.clone()])
        }
        NodeCommand::AcknowledgeMembershipPolicy(command) => {
            NetworkTargets::Named(vec![command.network_id.clone()])
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
        NodeCommand::PreviewNetworkProfile(command)
        | NodeCommand::UpdateNetworkProfile(command) => {
            NetworkTargets::Named(vec![command.network_name.clone()])
        }
        NodeCommand::PublishNetworkDiscovery(command)
        | NodeCommand::UpdateNetworkDiscovery(command)
        | NodeCommand::UnpublishNetworkDiscovery(command) => {
            NetworkTargets::Named(vec![command.network_name.clone()])
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
        ContributionKind, FieldVisibility, FieldVisibilityRule, NetworkAclGrant,
        NodeContributionVeto, NodeNetworkScope, PermissionCondition, PermissionEffect,
        PrivacyField, PrivacyPrincipal, SecurityRuleId, SubjectSelector,
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

    fn access_control_acl(
        network_id: NetworkId,
        rules: Vec<PermissionRule>,
        grants: Vec<NetworkAclGrant>,
    ) -> NetworkAcl {
        let now = unix_time_ms();
        NetworkAcl {
            network_id,
            revision: 1,
            issued_at_unix_ms: now.saturating_sub(10_000),
            expires_at_unix_ms: now.saturating_add(60_000),
            rules,
            grants,
            relay_limits: Default::default(),
        }
    }

    fn access_control_grant(
        grant_id: &[u8],
        action: PolicyAction,
        resource: ResourceKind,
        expires_at_unix_ms: u64,
    ) -> NetworkAclGrant {
        let now = unix_time_ms();
        NetworkAclGrant {
            grant_id: SecurityRuleId::from_public_key(grant_id),
            subjects: vec![SubjectSelector::AnyMember],
            resources: vec![resource],
            actions: vec![action],
            issued_at_unix_ms: now.saturating_sub(1_000),
            expires_at_unix_ms,
            revoked_at_unix_ms: None,
            revoked_by: None,
            requires_recent_reauthentication: false,
        }
    }

    fn access_control_local_policy(
        network_scope: NodeNetworkScope,
        contribution_vetoes: Vec<NodeContributionVeto>,
        rules: Vec<PermissionRule>,
    ) -> NodeLocalAccessPolicy {
        let now = unix_time_ms();
        NodeLocalAccessPolicy {
            node_id: actor(),
            revision: 1,
            issued_at_unix_ms: now.saturating_sub(1_000),
            expires_at_unix_ms: now.saturating_add(60_000),
            network_scope,
            contribution_vetoes,
            exit_contribution: None,
            relay_limits: Default::default(),
            rules,
        }
    }

    fn access_control_snapshot(
        local_policy: Option<NodeLocalAccessPolicy>,
        network_acls: Vec<NetworkAcl>,
    ) -> NodeAccessControlSnapshot {
        NodeAccessControlSnapshot {
            local_policy,
            network_acls,
            field_privacy_policies: Vec::new(),
        }
    }

    fn field_privacy_policy(network_id: NetworkId) -> FieldPrivacyPolicy {
        let now = unix_time_ms();
        FieldPrivacyPolicy {
            network_id: Some(network_id),
            revision: 3,
            issued_at_unix_ms: now.saturating_sub(1_000),
            effective_at_unix_ms: now.saturating_sub(500),
            expires_at_unix_ms: now.saturating_add(60_000),
            rules: vec![FieldVisibilityRule {
                field: PrivacyField::NodeEndpoints,
                visibility: FieldVisibility::Administrators,
            }],
        }
    }

    fn ban_home() -> NodeCommand {
        NodeCommand::BanClient(crate::node::NodeClientBlacklistCommand {
            network_name: "home".into(),
            client_id: "bad-client".into(),
        })
    }

    fn owner_scoped_context(
        role: NetworkRole,
        local_policy: Vec<PermissionRule>,
        network_policy: Vec<PermissionRule>,
    ) -> NodeCommandContext {
        NodeCommandContext::local_owner(actor(), "local-owner-test")
            .with_local_policy(local_policy)
            .with_network_scopes([(
                "home".to_string(),
                NetworkCommandScope::new(
                    network_id("home"),
                    role,
                    MembershipState::Active,
                    network_policy,
                ),
            )])
    }

    fn create_home() -> NodeCommand {
        NodeCommand::CreateNetwork(crate::node::NodeCreateNetworkCommand {
            name: "new-home".into(),
            purpose: "A second home network".into(),
            icon: "home".into(),
            language: "en".into(),
            region: "ZZ".into(),
            tags: Vec::new(),
            capability_declarations: Vec::new(),
            visibility: fuckipv6_common::domain::NetworkVisibility::Unlisted,
            join_mode: fuckipv6_common::domain::NetworkJoinMode::InviteOnly,
            exit_contribution: Default::default(),
            exit_contribution_scope: Default::default(),
            exit_contribution_mode: Default::default(),
            exit_selection_policy: Default::default(),
            signaling_port: 6666,
            relay_port: 6667,
            external_host: None,
            auto_connect: false,
        })
    }

    fn membership_policies(action: PolicyAction) -> (Vec<PermissionRule>, Vec<PermissionRule>) {
        (
            vec![allow(
                b"local-membership-policy",
                SubjectSelector::AnyMember,
                action,
                ResourceKind::Membership,
            )],
            vec![allow(
                b"network-membership-policy",
                SubjectSelector::AnyMember,
                action,
                ResourceKind::Membership,
            )],
        )
    }

    fn review_join_request() -> NodeCommand {
        NodeCommand::ReviewJoinRequest(crate::node::command::NodeReviewJoinRequestCommand {
            network_id: "home".into(),
            review: crate::node::command::NodeJoinRequestReviewItem {
                request_id: fuckipv6_common::domain::JoinRequestId::from_public_key(
                    b"authorization-review-request",
                )
                .to_string(),
                expected_revision: 1,
                action: fuckipv6_common::domain::JoinRequestReviewAction::Approve,
                note: None,
            },
        })
    }

    fn invitation_code() -> fuckipv6_common::domain::InvitationCode {
        fuckipv6_common::domain::InvitationCode::parse(format!("fkinv_{}", "A".repeat(43)))
            .expect("valid invitation code")
    }

    fn invitation_network() -> NetworkId {
        network_id("home")
    }

    fn invitation_id() -> fuckipv6_common::domain::InvitationId {
        fuckipv6_common::domain::InvitationId::from_public_key(b"authorization-invitation")
    }

    fn invitation_commands() -> Vec<NodeCommand> {
        vec![
            NodeCommand::CreateInvitation(crate::node::NodeCreateInvitationCommand {
                network_id: invitation_network(),
                role: fuckipv6_common::domain::MembershipRole::Member,
                capabilities: vec![fuckipv6_common::domain::CapabilityKind::File],
                max_uses: 2,
                expires_in_ms: 86_400_000,
            }),
            NodeCommand::ListInvitations(crate::node::NodeListInvitationsCommand {
                network_id: invitation_network(),
            }),
            NodeCommand::GetInvitationStatus(crate::node::NodeGetInvitationStatusCommand {
                network_id: invitation_network(),
                invitation_id: invitation_id(),
            }),
            NodeCommand::RevokeInvitation(crate::node::NodeRevokeInvitationCommand {
                network_id: invitation_network(),
                invitation_id: invitation_id(),
                expected_revision: 1,
            }),
            NodeCommand::PreviewInvitation(crate::node::NodePreviewInvitationCommand {
                network_id: invitation_network(),
                coordinator_url: "wss://coordinator.example.test/signaling".into(),
                code: invitation_code(),
            }),
            NodeCommand::RedeemInvitation(crate::node::NodeRedeemInvitationCommand {
                coordinator_url: "wss://coordinator.example.test/signaling".into(),
                request: fuckipv6_common::domain::InvitationRedeemRequest {
                    network_id: invitation_network(),
                    code: invitation_code(),
                    invitation_id: invitation_id(),
                    expected_invitation_revision: 1,
                    preview_digest: fuckipv6_common::domain::ContentHash::from_bytes(b"preview"),
                    terms_hash: fuckipv6_common::domain::ContentHash::from_bytes(b"terms"),
                    acknowledged_at_unix_ms: 1_700_000_000_000,
                    exit_contribution_acknowledged: Some(true),
                },
            }),
        ]
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
    fn local_exit_capability_can_only_be_changed_by_the_local_owner() {
        let (local_policy, network_policy) = start_policies();
        let commands = [true, false].map(|enabled| {
            NodeCommand::SetLocalExitCapability(crate::node::NodeSetLocalExitCapabilityCommand {
                enabled,
            })
        });

        for role in [NetworkRole::Owner, NetworkRole::Administrator] {
            let remote = network_context(
                role,
                [NodePermission::ManageNetworks],
                &["home"],
                MembershipState::Active,
                local_policy.clone(),
                network_policy.clone(),
            );
            for command in &commands {
                assert_denied(&remote, command.clone(), "permission.local_owner_required");
            }
        }

        let local_owner = NodeCommandContext::local_owner(actor(), "local-owner-exit");
        for command in &commands {
            authorize_command(&local_owner, command)
                .expect("the local owner can enable or disable local Exit");
        }
    }

    #[test]
    fn create_network_is_reserved_for_the_local_node_owner() {
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
            create_home(),
            "permission.local_owner_required",
        );

        authorize_command(
            &NodeCommandContext::local_owner(actor(), "local-owner-create"),
            &create_home(),
        )
        .expect("local node owner can create a network");
    }

    #[test]
    fn join_request_owner_actions_are_reserved_for_the_local_owner() {
        let (local_policy, network_policy) = membership_policies(PolicyAction::Create);
        let admin = network_context(
            NetworkRole::Administrator,
            [NodePermission::ManageNetworks],
            &["home"],
            MembershipState::Active,
            local_policy,
            network_policy,
        );
        let commands = [
            NodeCommand::SubmitJoinRequest(crate::node::command::NodeSubmitJoinRequestCommand {
                network_id: "home".into(),
                reason: "Please allow this node to join.".into(),
            }),
            NodeCommand::WithdrawJoinRequest(
                crate::node::command::NodeWithdrawJoinRequestCommand {
                    network_id: "home".into(),
                    request_id: fuckipv6_common::domain::JoinRequestId::from_public_key(
                        b"authorization-withdraw-request",
                    )
                    .to_string(),
                },
            ),
            NodeCommand::ResubmitJoinRequest(
                crate::node::command::NodeResubmitJoinRequestCommand {
                    network_id: "home".into(),
                    request_id: fuckipv6_common::domain::JoinRequestId::from_public_key(
                        b"authorization-resubmit-request",
                    )
                    .to_string(),
                    expected_revision: 2,
                    reason: "Updated information requested by the reviewer.".into(),
                },
            ),
        ];
        for command in commands {
            assert_denied(&admin, command.clone(), "permission.local_owner_required");
            authorize_command(
                &NodeCommandContext::local_owner(actor(), "local-owner-join-request"),
                &command,
            )
            .expect("the local owner can manage its own join request");
        }
    }

    #[test]
    fn join_request_review_requires_network_membership_management_scope() {
        let (local_policy, network_policy) = membership_policies(PolicyAction::Update);
        authorize_command(
            &network_context(
                NetworkRole::Administrator,
                [NodePermission::ManageNetworks],
                &["home"],
                MembershipState::Active,
                local_policy.clone(),
                network_policy.clone(),
            ),
            &review_join_request(),
        )
        .expect("an active network administrator can review membership requests");

        assert_denied(
            &network_context(
                NetworkRole::Member,
                [NodePermission::ManageNetworks],
                &["home"],
                MembershipState::Active,
                local_policy.clone(),
                network_policy.clone(),
            ),
            review_join_request(),
            "permission.role_ceiling_exceeded",
        );
        assert_denied(
            &network_context(
                NetworkRole::Administrator,
                [NodePermission::ManageNetworks],
                &["office"],
                MembershipState::Active,
                local_policy,
                network_policy,
            ),
            review_join_request(),
            "permission.network_scope_denied",
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
    fn temporary_coordinator_cannot_start_or_stop_relay() {
        let relay_policies = (
            vec![allow(
                b"local-relay-capability",
                SubjectSelector::AnyMember,
                PolicyAction::Update,
                ResourceKind::Capability,
            )],
            vec![allow(
                b"network-relay-capability",
                SubjectSelector::AnyMember,
                PolicyAction::Update,
                ResourceKind::Capability,
            )],
        );
        for enabled in [true, false] {
            assert_denied(
                &network_context(
                    NetworkRole::TemporaryCoordinator,
                    [NodePermission::ManageRoles],
                    &["home"],
                    MembershipState::Active,
                    relay_policies.0.clone(),
                    relay_policies.1.clone(),
                ),
                NodeCommand::SetNetworkRole(crate::node::NodeSetNetworkRoleCommand {
                    network_name: "home".into(),
                    role: crate::node::NodeNetworkRole::Relay,
                    enabled,
                }),
                "permission.temporary_coordinator_read_only",
            );
        }
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
    fn local_owner_host_authority_does_not_bypass_network_governance_scope() {
        let owner = NodeCommandContext::local_owner(actor(), "local-owner-test");
        authorize_command(&owner, &NodeCommand::GetDiagnostics)
            .expect("the local owner can execute a pure host command without a network scope");
        assert_denied(
            &owner,
            ban_home(),
            PermissionDenialCode::NetworkScopeDenied.as_str(),
        );
    }

    #[test]
    fn local_owner_governance_obeys_policy_and_role_gates() {
        let action = PolicyAction::Update;
        let resource = ResourceKind::SecurityPolicy;
        let local_allow = allow(
            b"local-security-update",
            SubjectSelector::AnyMember,
            action,
            resource,
        );
        let network_allow = allow(
            b"network-security-update",
            SubjectSelector::AnyMember,
            action,
            resource,
        );
        let local_deny = deny(
            b"local-security-deny",
            SubjectSelector::AnyMember,
            action,
            resource,
        );

        assert_denied(
            &owner_scoped_context(
                NetworkRole::Owner,
                vec![local_allow.clone(), local_deny],
                vec![network_allow.clone()],
            ),
            ban_home(),
            PermissionDenialCode::Denied.as_str(),
        );
        assert_denied(
            &owner_scoped_context(
                NetworkRole::Member,
                vec![local_allow.clone()],
                vec![network_allow.clone()],
            ),
            ban_home(),
            PermissionDenialCode::RoleCeilingExceeded.as_str(),
        );
        assert_denied(
            &owner_scoped_context(
                NetworkRole::TemporaryCoordinator,
                vec![local_allow],
                vec![network_allow],
            ),
            ban_home(),
            PermissionDenialCode::TemporaryCoordinatorReadOnly.as_str(),
        );
    }

    #[test]
    fn snapshot_membership_cannot_escalate_node_ownership_to_network_owner() {
        fn snapshot_membership(
            network_id: String,
            role: MembershipRole,
        ) -> crate::node::NodeNetworkMembershipSnapshot {
            crate::node::NodeNetworkMembershipSnapshot {
                network_name: "home".into(),
                network_id,
                node_id: actor().to_string(),
                state: MembershipState::Active,
                role,
                local_policy_revision: 1,
                network_policy_revision: Some(1),
                privacy_policy_revision: Some(1),
                member_limit: None,
                exit_contribution: None,
                exit_contribution_scope: None,
                exit_contribution_mode: None,
                terms_hash: None,
                current_terms: None,
                current_terms_hash: None,
                policy_confirmation_required: false,
                acknowledged_at_unix_ms: None,
                created_at_unix_ms: 1,
                updated_at_unix_ms: 1,
                suspension_reason: None,
            }
        }

        let member_snapshot = NodeSnapshot {
            memberships: vec![snapshot_membership(
                network_id("home").to_string(),
                MembershipRole::Member,
            )],
            ..Default::default()
        };
        let member_context = NodeCommandContext::local_owner_for_snapshot(&member_snapshot, "gpui");
        assert_denied(
            &member_context,
            ban_home(),
            PermissionDenialCode::RoleCeilingExceeded.as_str(),
        );

        let malformed_snapshot = NodeSnapshot {
            memberships: vec![snapshot_membership(
                "not-a-stable-network-id".into(),
                MembershipRole::Owner,
            )],
            ..Default::default()
        };
        let malformed_context =
            NodeCommandContext::local_owner_for_snapshot(&malformed_snapshot, "gpui");
        assert_denied(
            &malformed_context,
            ban_home(),
            PermissionDenialCode::NetworkScopeDenied.as_str(),
        );
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
    fn node_identity_rotation_is_owner_only_and_requires_reauthentication() {
        let command = NodeCommand::RotateNodeIdentity(crate::node::NodeRotateNodeIdentityCommand {
            expected_fingerprint: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
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

        authorize_command(&NodeCommandContext::local_owner(actor(), "gpui"), &command)
            .expect("desktop local owner may rotate the node identity");

        let stale_owner = NodeCommandContext::owner_session(actor(), "webui", false);
        assert_denied(
            &stale_owner,
            command.clone(),
            "permission.recent_authentication_required",
        );
        authorize_command(
            &NodeCommandContext::owner_session(actor(), "webui", true),
            &command,
        )
        .expect("recently reauthenticated WebUI owner may rotate the identity");
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
    fn invitation_permissions_are_enforced_at_the_command_boundary() {
        assert_eq!(
            required_permission(NodeCommandKind::CreateInvitation),
            NodePermission::ManageNetworks
        );
        assert_eq!(
            required_permission(NodeCommandKind::RevokeInvitation),
            NodePermission::ManageNetworks
        );
        assert_eq!(
            required_permission(NodeCommandKind::PreviewInvitation),
            NodePermission::ManageNetworks
        );
        assert_eq!(
            required_permission(NodeCommandKind::RedeemInvitation),
            NodePermission::ManageNetworks
        );
        assert_eq!(
            required_permission(NodeCommandKind::ListInvitations),
            NodePermission::ViewState
        );
        assert_eq!(
            required_permission(NodeCommandKind::GetInvitationStatus),
            NodePermission::ViewState
        );

        let (local_policy, network_policy) = (
            vec![allow(
                b"local-read-membership",
                SubjectSelector::AnyMember,
                PolicyAction::Read,
                ResourceKind::Membership,
            )],
            vec![allow(
                b"network-read-membership",
                SubjectSelector::AnyMember,
                PolicyAction::Read,
                ResourceKind::Membership,
            )],
        );
        let active_member = network_context(
            NetworkRole::Member,
            [NodePermission::ViewState],
            &["home"],
            MembershipState::Active,
            local_policy.clone(),
            network_policy.clone(),
        );
        for command in invitation_commands().into_iter().filter(|command| {
            matches!(
                command.kind(),
                NodeCommandKind::ListInvitations | NodeCommandKind::GetInvitationStatus
            )
        }) {
            authorize_command(&active_member, &command)
                .expect("active member with ViewState may inspect invitation metadata");
        }

        let no_view = network_context(
            NetworkRole::Member,
            [],
            &["home"],
            MembershipState::Active,
            local_policy,
            network_policy,
        );
        assert_denied(
            &no_view,
            NodeCommand::ListInvitations(crate::node::NodeListInvitationsCommand {
                network_id: invitation_network(),
            }),
            "permission.capability_denied",
        );
    }

    #[test]
    fn invitation_preview_and_redemption_are_local_owner_only() {
        let (local_policy, network_policy) = (
            vec![
                allow(
                    b"local-invitation-create",
                    SubjectSelector::AnyMember,
                    PolicyAction::Create,
                    ResourceKind::Membership,
                ),
                allow(
                    b"local-invitation-read",
                    SubjectSelector::AnyMember,
                    PolicyAction::Read,
                    ResourceKind::Membership,
                ),
            ],
            vec![
                allow(
                    b"network-invitation-create",
                    SubjectSelector::AnyMember,
                    PolicyAction::Create,
                    ResourceKind::Membership,
                ),
                allow(
                    b"network-invitation-read",
                    SubjectSelector::AnyMember,
                    PolicyAction::Read,
                    ResourceKind::Membership,
                ),
            ],
        );
        let administrator = network_context(
            NetworkRole::Administrator,
            [NodePermission::ManageNetworks],
            &["home"],
            MembershipState::Active,
            local_policy,
            network_policy,
        );
        let commands = invitation_commands();
        for command in [&commands[4], &commands[5]] {
            assert_denied(
                &administrator,
                command.clone(),
                "permission.local_owner_required",
            );
        }

        let owner = NodeCommandContext::local_owner(actor(), "gpui")
            .with_test_network_authority_for(&commands[0]);
        for command in commands {
            authorize_command(&owner, &command)
                .expect("local owner may manage invitation preview and redemption");
        }
    }

    #[test]
    fn stale_invitation_owner_is_forced_to_reauthenticate_before_secret_use() {
        let commands = invitation_commands();
        let stale = NodeCommandContext::owner_session(actor(), "webui", false)
            .with_test_network_authority_for(&commands[0]);
        for command in [&commands[0], &commands[3], &commands[4], &commands[5]] {
            assert_denied(
                &stale,
                command.clone(),
                "permission.recent_authentication_required",
            );
        }
        authorize_command(&stale, &commands[1]).expect("list does not require reauthentication");
        authorize_command(&stale, &commands[2]).expect("status does not require reauthentication");

        let fresh = NodeCommandContext::owner_session(actor(), "webui", true)
            .with_test_network_authority_for(&commands[5]);
        authorize_command(&fresh, &commands[5]).expect("fresh owner may redeem");
    }

    #[test]
    fn invitation_network_target_contains_only_the_stable_network_id() {
        let command = NodeCommand::PreviewInvitation(crate::node::NodePreviewInvitationCommand {
            network_id: invitation_network(),
            coordinator_url: "wss://coordinator.example.test/signaling".into(),
            code: invitation_code(),
        });
        let NetworkTargets::Named(targets) = command_network_targets(&command) else {
            panic!("invitation command must be network-scoped");
        };
        assert_eq!(targets, vec![invitation_network().to_string()]);
        assert!(!format!("{targets:?}").contains("fkinv_"));
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

        for (local_policy, network_policy, expected_code) in [
            (
                vec![local_allow.clone(), local_deny],
                vec![network_allow.clone()],
                "permission.denied",
            ),
            (
                vec![local_allow.clone()],
                vec![network_allow.clone(), network_deny],
                "permission.denied",
            ),
            (
                Vec::new(),
                vec![network_allow.clone()],
                "permission.default_denied",
            ),
            (vec![local_allow], Vec::new(), "permission.default_denied"),
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
                expected_code,
            );
        }
    }

    #[test]
    fn network_acl_and_local_policy_denials_override_admin_allows() {
        let home = network_id("home");
        let (local_policy, network_policy) = start_policies();
        let network_deny = deny(
            b"acl-deny-start",
            SubjectSelector::AnyMember,
            PolicyAction::Enable,
            ResourceKind::Network,
        );
        let network_deny_id = network_deny.rule_id.to_string();
        let acl = access_control_acl(
            home.clone(),
            vec![network_deny],
            vec![access_control_grant(
                b"acl-direct-allow",
                PolicyAction::Enable,
                ResourceKind::Network,
                unix_time_ms().saturating_add(30_000),
            )],
        );
        let context = network_context(
            NetworkRole::Administrator,
            [NodePermission::ManageNetworks],
            &["home"],
            MembershipState::Active,
            local_policy.clone(),
            network_policy.clone(),
        )
        .with_access_control_snapshot(&access_control_snapshot(None, vec![acl]));
        let denial = authorize_command(&context, &start_home()).expect_err("ACL deny must win");
        assert_eq!(denial.code, PermissionDenialCode::Denied.as_str());
        assert!(denial.reason.contains(&network_deny_id), "{denial:?}");
        assert!(denial.reason.contains("network layer"), "{denial:?}");

        let local_deny = deny(
            b"local-policy-deny-start",
            SubjectSelector::AnyMember,
            PolicyAction::Enable,
            ResourceKind::Network,
        );
        let local_deny_id = local_deny.rule_id.to_string();
        let context = network_context(
            NetworkRole::Administrator,
            [NodePermission::ManageNetworks],
            &["home"],
            MembershipState::Active,
            local_policy,
            network_policy,
        )
        .with_access_control_snapshot(&access_control_snapshot(
            Some(access_control_local_policy(
                NodeNetworkScope::All,
                Vec::new(),
                vec![local_deny],
            )),
            Vec::new(),
        ));
        let denial = authorize_command(&context, &start_home()).expect_err("local deny must win");
        assert_eq!(denial.code, PermissionDenialCode::Denied.as_str());
        assert!(denial.reason.contains(&local_deny_id), "{denial:?}");
        assert!(denial.reason.contains("local layer"), "{denial:?}");
    }

    #[test]
    fn node_owner_network_scope_cannot_be_widened_by_network_admin() {
        let (local_policy, network_policy) = start_policies();
        let context = network_context(
            NetworkRole::Administrator,
            [NodePermission::ManageNetworks],
            &["home"],
            MembershipState::Active,
            local_policy,
            network_policy,
        )
        .with_access_control_snapshot(&access_control_snapshot(
            Some(access_control_local_policy(
                NodeNetworkScope::Only(vec![network_id("office")]),
                Vec::new(),
                Vec::new(),
            )),
            Vec::new(),
        ));

        let denial = authorize_command(&context, &start_home())
            .expect_err("the node owner's network restriction must win");
        assert_eq!(
            denial.code,
            PermissionDenialCode::NetworkScopeDenied.as_str()
        );
        assert!(denial.reason.contains("network_scope_restricted"));
    }

    #[test]
    fn contribution_veto_and_network_scope_are_fail_closed() {
        let home = network_id("home");
        let request = PermissionRequest {
            action: PolicyAction::Use,
            resource: ResourceRef {
                kind: ResourceKind::ExitGrant,
                network_id: home.clone(),
                object_id: None,
            },
            intent: PermissionIntent::Mutating,
        };
        let context = AccessContext {
            network_id: home.clone(),
            actor: actor(),
            role: NetworkRole::Administrator,
            groups: BTreeSet::new(),
            membership_active: true,
            recent_reauthentication: true,
            emergency_coordinator: false,
        };
        let veto_id = SecurityRuleId::from_public_key(b"command-veto-exit");
        let local_policy = access_control_local_policy(
            NodeNetworkScope::All,
            vec![NodeContributionVeto {
                veto_id: veto_id.clone(),
                network_id: home,
                contributions: vec![ContributionKind::Exit],
            }],
            Vec::new(),
        );
        let decision = evaluate_effective_access(
            &[],
            &[allow(
                b"network-exit-allow",
                SubjectSelector::Role(NetworkRole::Administrator),
                PolicyAction::Use,
                ResourceKind::ExitGrant,
            )],
            None,
            Some(&local_policy),
            &request,
            &context,
            unix_time_ms(),
        );
        assert!(!decision.allowed);
        assert_eq!(decision.rule_id.as_ref(), Some(&veto_id));
        assert!(decision
            .explanation("home")
            .contains("local_contribution_veto"));
    }

    #[test]
    fn acl_grant_lifecycle_and_reauthentication_are_enforced_before_dispatch() {
        let home = network_id("home");
        let local_policy = vec![allow(
            b"local-grant-allow",
            SubjectSelector::AnyMember,
            PolicyAction::Enable,
            ResourceKind::Network,
        )];
        let now = unix_time_ms();

        let active = access_control_grant(
            b"active-grant",
            PolicyAction::Enable,
            ResourceKind::Network,
            now.saturating_add(30_000),
        );
        let context = network_context(
            NetworkRole::Administrator,
            [NodePermission::ManageNetworks],
            &["home"],
            MembershipState::Active,
            local_policy.clone(),
            Vec::new(),
        )
        .with_access_control_snapshot(&access_control_snapshot(
            None,
            vec![access_control_acl(home.clone(), Vec::new(), vec![active])],
        ));
        authorize_command(&context, &start_home()).expect("active direct grant must allow");

        let expired = access_control_grant(
            b"expired-grant",
            PolicyAction::Enable,
            ResourceKind::Network,
            now.saturating_sub(1),
        );
        let context = network_context(
            NetworkRole::Administrator,
            [NodePermission::ManageNetworks],
            &["home"],
            MembershipState::Active,
            local_policy.clone(),
            Vec::new(),
        )
        .with_access_control_snapshot(&access_control_snapshot(
            None,
            vec![access_control_acl(home.clone(), Vec::new(), vec![expired])],
        ));
        let denial =
            authorize_command(&context, &start_home()).expect_err("expired grant must deny");
        assert_eq!(denial.code, PermissionDenialCode::PolicyDenied.as_str());
        assert!(denial.reason.contains("authorization_expired"));

        let mut revoked = access_control_grant(
            b"revoked-grant",
            PolicyAction::Enable,
            ResourceKind::Network,
            now.saturating_add(30_000),
        );
        revoked.revoked_at_unix_ms = Some(now.saturating_sub(1));
        revoked.revoked_by = Some(actor());
        let context = network_context(
            NetworkRole::Administrator,
            [NodePermission::ManageNetworks],
            &["home"],
            MembershipState::Active,
            local_policy.clone(),
            Vec::new(),
        )
        .with_access_control_snapshot(&access_control_snapshot(
            None,
            vec![access_control_acl(home.clone(), Vec::new(), vec![revoked])],
        ));
        let denial =
            authorize_command(&context, &start_home()).expect_err("revoked grant must deny");
        assert_eq!(denial.code, PermissionDenialCode::PolicyDenied.as_str());
        assert!(denial.reason.contains("authorization_revoked"));

        let mut reauthentication = access_control_grant(
            b"reauth-grant",
            PolicyAction::Enable,
            ResourceKind::Network,
            now.saturating_add(30_000),
        );
        reauthentication.requires_recent_reauthentication = true;
        let context = network_context_with_groups(
            NetworkRole::Administrator,
            [NodePermission::ManageNetworks],
            &["home"],
            MembershipState::Active,
            std::iter::empty(),
            (local_policy, Vec::new()),
            false,
        )
        .with_access_control_snapshot(&access_control_snapshot(
            None,
            vec![access_control_acl(home, Vec::new(), vec![reauthentication])],
        ));
        let denial = authorize_command(&context, &start_home())
            .expect_err("stale authentication must not satisfy a grant");
        assert_eq!(
            denial.code,
            PermissionDenialCode::RecentAuthenticationRequired.as_str()
        );
        assert!(
            denial.reason.contains("recent_reauthentication_required"),
            "{denial:?}"
        );
    }

    #[test]
    fn malformed_and_ambiguous_access_control_fail_closed() {
        let home = network_id("home");
        let (local_policy, network_policy) = start_policies();
        let mut malformed = access_control_acl(home.clone(), Vec::new(), Vec::new());
        malformed.revision = 0;
        let context = network_context(
            NetworkRole::Administrator,
            [NodePermission::ManageNetworks],
            &["home"],
            MembershipState::Active,
            local_policy.clone(),
            network_policy.clone(),
        )
        .with_access_control_snapshot(&access_control_snapshot(None, vec![malformed]));
        let denial =
            authorize_command(&context, &start_home()).expect_err("malformed ACL must fail closed");
        assert_eq!(denial.code, PermissionDenialCode::PolicyDenied.as_str());
        assert!(denial.reason.contains("invalid_policy"));

        let duplicate = access_control_acl(home, Vec::new(), Vec::new());
        let context = network_context(
            NetworkRole::Administrator,
            [NodePermission::ManageNetworks],
            &["home"],
            MembershipState::Active,
            local_policy,
            network_policy,
        )
        .with_access_control_snapshot(&access_control_snapshot(
            None,
            vec![duplicate.clone(), duplicate],
        ));
        let denial = authorize_command(&context, &start_home())
            .expect_err("duplicate ACLs must fail closed");
        assert_eq!(denial.code, PermissionDenialCode::PolicyDenied.as_str());
        assert!(denial.reason.contains("ambiguous"));
    }

    #[test]
    fn field_privacy_snapshot_is_trusted_and_ambiguity_fails_closed() {
        let home = network_id("home");
        let policy = field_privacy_policy(home.clone());
        let context = NodeCommandContext::network_session(
            actor(),
            "network-test",
            [NodePermission::ViewState],
            [(
                "home".into(),
                NetworkCommandScope::new(
                    home.clone(),
                    NetworkRole::TemporaryCoordinator,
                    MembershipState::Active,
                    Vec::new(),
                ),
            )],
            true,
        )
        .with_access_control_snapshot(&NodeAccessControlSnapshot {
            local_policy: None,
            network_acls: Vec::new(),
            field_privacy_policies: vec![policy.clone()],
        });

        let viewer = context.privacy_viewer_for(Some(&home));
        assert_eq!(
            viewer.principal,
            PrivacyPrincipal::NetworkMember,
            "temporary coordinator must not become a durable administrator"
        );
        assert!(viewer.has_active_network_role());
        assert_eq!(
            context
                .field_privacy_policy_for(Some(&home))
                .expect("trusted policy")
                .map(|policy| policy.revision),
            Some(3)
        );

        let duplicate = NodeCommandContext::network_session(
            actor(),
            "network-test",
            [NodePermission::ViewState],
            [(
                "home".into(),
                NetworkCommandScope::new(
                    home.clone(),
                    NetworkRole::Member,
                    MembershipState::Active,
                    Vec::new(),
                ),
            )],
            true,
        )
        .with_access_control_snapshot(&NodeAccessControlSnapshot {
            local_policy: None,
            network_acls: Vec::new(),
            field_privacy_policies: vec![policy.clone(), policy],
        });
        assert!(duplicate.field_privacy_policy_for(Some(&home)).is_err());
    }

    #[test]
    fn snapshot_acl_is_attached_to_the_authoritative_owner_scope() {
        let home = network_id("home");
        let snapshot = NodeSnapshot {
            memberships: vec![crate::node::NodeNetworkMembershipSnapshot {
                network_name: "home".into(),
                network_id: home.to_string(),
                node_id: actor().to_string(),
                state: MembershipState::Active,
                role: MembershipRole::Owner,
                local_policy_revision: 1,
                network_policy_revision: Some(1),
                privacy_policy_revision: Some(1),
                member_limit: None,
                exit_contribution: None,
                exit_contribution_scope: None,
                exit_contribution_mode: None,
                terms_hash: None,
                current_terms: None,
                current_terms_hash: None,
                policy_confirmation_required: false,
                acknowledged_at_unix_ms: None,
                created_at_unix_ms: 1,
                updated_at_unix_ms: 1,
                suspension_reason: None,
            }],
            access_control: access_control_snapshot(
                None,
                vec![access_control_acl(
                    home,
                    vec![deny(
                        b"snapshot-ban-deny",
                        SubjectSelector::AnyMember,
                        PolicyAction::Update,
                        ResourceKind::SecurityPolicy,
                    )],
                    Vec::new(),
                )],
            ),
            ..NodeSnapshot::default()
        };
        let context = NodeCommandContext::local_owner_for_snapshot(&snapshot, "gpui");
        let denial = authorize_command(&context, &ban_home())
            .expect_err("snapshot ACL must constrain network governance");
        assert_eq!(denial.code, PermissionDenialCode::Denied.as_str());
        assert!(denial.reason.contains("network layer"));
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
            "permission.default_denied",
        );
    }

    #[test]
    fn update_routing_is_local_owner_authority_not_network_governance() {
        let home = network_id("home");
        let snapshot = crate::node::NodeSnapshot {
            memberships: vec![crate::node::NodeNetworkMembershipSnapshot {
                network_name: "home".into(),
                network_id: home.to_string(),
                node_id: actor().to_string(),
                state: MembershipState::Active,
                role: MembershipRole::Member,
                local_policy_revision: 1,
                network_policy_revision: Some(1),
                privacy_policy_revision: Some(1),
                member_limit: None,
                exit_contribution: None,
                exit_contribution_scope: None,
                exit_contribution_mode: None,
                terms_hash: None,
                current_terms: None,
                current_terms_hash: None,
                policy_confirmation_required: false,
                acknowledged_at_unix_ms: None,
                created_at_unix_ms: 1,
                updated_at_unix_ms: 1,
                suspension_reason: None,
            }],
            ..Default::default()
        };
        let command = NodeCommand::UpdateRouting(crate::node::NodeUpdateRoutingCommand {
            network_name: "home".into(),
            mode: crate::node::NodeRoutingMode::GlobalProxy,
            exit_node: Some("fd00::1001".into()),
            is_exit_node: false,
        });

        authorize_command(
            &NodeCommandContext::local_owner_for_snapshot(&snapshot, "cli"),
            &command,
        )
        .expect("a local owner may choose this node's routing regardless of network role");

        let (local_policy, network_policy) = start_policies();
        assert_denied(
            &network_context(
                NetworkRole::Member,
                [NodePermission::ChangeRouting],
                &["home"],
                MembershipState::Active,
                local_policy,
                network_policy,
            ),
            command,
            PermissionDenialCode::LocalOwnerRequired.as_str(),
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

    fn policy_test_command() -> NodeCommand {
        let network_id = network_id("home");
        let input = fuckipv6_common::domain::parse_policy_test_input(
            &serde_json::json!({
                "network_id": network_id.to_string(),
                "actor": actor().to_string(),
                "role": "owner",
                "groups": [],
                "membership_active": true,
                "recent_reauthentication": true,
                "emergency_coordinator": false,
                "request": {
                    "action": "test",
                    "resource": "security_policy",
                    "resource_network_id": network_id.to_string(),
                    "intent": "read_only"
                },
                "now_unix_ms": 1_000
            })
            .to_string(),
        )
        .expect("valid policy test command");
        NodeCommand::TestPolicy(input)
    }

    #[test]
    fn policy_test_is_manage_networks_scoped_and_uses_the_security_policy_target() {
        assert_eq!(
            required_permission(NodeCommandKind::TestPolicy),
            NodePermission::ManageNetworks
        );
        assert_eq!(
            policy_target_kind(NodeCommandKind::TestPolicy),
            (PolicyAction::Test, ResourceKind::SecurityPolicy)
        );
        let command = policy_test_command();
        let NodeCommand::TestPolicy(input) = &command else {
            panic!("expected policy test command");
        };
        assert_eq!(
            command_network_targets(&command),
            NetworkTargets::Named(vec![input.network_id.to_string()])
        );

        let local_policy = vec![allow(
            b"local-policy-test-allow",
            SubjectSelector::AnyMember,
            PolicyAction::Test,
            ResourceKind::SecurityPolicy,
        )];
        let network_policy = vec![allow(
            b"network-policy-test-allow",
            SubjectSelector::AnyMember,
            PolicyAction::Test,
            ResourceKind::SecurityPolicy,
        )];
        authorize_command(
            &network_context(
                NetworkRole::Owner,
                [NodePermission::ManageNetworks],
                &["home"],
                MembershipState::Active,
                local_policy,
                network_policy,
            ),
            &command,
        )
        .expect("owner with manage permission can test policies");
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
            "permission.default_denied",
        );
    }
}
