//! Server-side authorization for the shared node command bus.
//!
//! Authorization context is transport-owned and deliberately not part of any
//! request body. A UI may hide or show an action, but the command bus always
//! re-evaluates the authenticated actor, capability, network scope and role
//! ceiling before a handler is dispatched.

use std::collections::{BTreeSet, HashSet};

use fuckipv6_common::domain::{role_ceiling_allows, NetworkRole, PolicyAction, ResourceKind};

use super::command::{NodeCommand, NodeCommandKind};
use super::state::{NodePermission, NodeSnapshot};

/// A trusted, adapter-constructed command principal. The fields are private so
/// callers cannot turn untrusted wire data into an owner context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeCommandContext {
    actor: fuckipv6_common::domain::NodeId,
    source: String,
    role: NetworkRole,
    capabilities: HashSet<NodePermission>,
    allowed_networks: Option<BTreeSet<String>>,
    membership_active: bool,
    recent_reauthentication: bool,
    local_owner: bool,
    authenticated: bool,
}

impl NodeCommandContext {
    /// The local OS owner boundary: same-UID IPC, GPUI, CLI or a verified
    /// Passkey owner session. This principal owns the node and therefore may
    /// exercise the node's final local-control authority.
    pub fn local_owner(actor: fuckipv6_common::domain::NodeId, source: impl Into<String>) -> Self {
        Self {
            actor,
            source: source.into(),
            role: NetworkRole::Owner,
            capabilities: NodePermission::ALL.into_iter().collect(),
            allowed_networks: None,
            membership_active: true,
            recent_reauthentication: true,
            local_owner: true,
            authenticated: true,
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

    /// A network-scoped principal. `allowed_networks` is the server-side ACL
    /// already resolved for this session; command payloads cannot widen it.
    pub fn network_session(
        actor: fuckipv6_common::domain::NodeId,
        source: impl Into<String>,
        role: NetworkRole,
        capabilities: impl IntoIterator<Item = NodePermission>,
        allowed_networks: impl IntoIterator<Item = String>,
        membership_active: bool,
        recently_reauthenticated: bool,
    ) -> Self {
        Self {
            actor,
            source: source.into(),
            role,
            capabilities: capabilities.into_iter().collect(),
            allowed_networks: Some(allowed_networks.into_iter().collect()),
            membership_active,
            recent_reauthentication: recently_reauthenticated,
            local_owner: false,
            authenticated: true,
        }
    }

    /// A transport with no verified principal, such as the legacy loopback TCP
    /// compatibility endpoint.
    pub fn untrusted(source: impl Into<String>) -> Self {
        Self {
            actor: fuckipv6_common::domain::NodeId::from_public_key(
                b"fuckipv6-unauthenticated-command-actor",
            ),
            source: source.into(),
            role: NetworkRole::Member,
            capabilities: HashSet::new(),
            allowed_networks: Some(BTreeSet::new()),
            membership_active: false,
            recent_reauthentication: false,
            local_owner: false,
            authenticated: false,
        }
    }

    pub fn actor(&self) -> &fuckipv6_common::domain::NodeId {
        &self.actor
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn role(&self) -> NetworkRole {
        self.role
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
    pub(crate) code: &'static str,
    pub(crate) reason: &'static str,
}

pub(crate) fn authorize_command(
    context: &NodeCommandContext,
    command: &NodeCommand,
) -> Result<(), CommandDenial> {
    if !context.authenticated {
        return Err(CommandDenial {
            code: "permission.authentication_required",
            reason: "actor is not authenticated",
        });
    }
    if !context.membership_active {
        return Err(CommandDenial {
            code: "permission.membership_inactive",
            reason: "actor membership is inactive",
        });
    }

    let kind = command.kind();
    let permission = required_permission(kind);
    if !context.capabilities.contains(&permission) {
        return Err(CommandDenial {
            code: "permission.capability_denied",
            reason: "required node capability is not granted",
        });
    }

    // Local node ownership is the final local-control boundary. Network
    // governance can never revoke it, but it also cannot use this path to act on
    // a different node.
    if is_local_owner_only(kind) && !context.local_owner {
        return Err(CommandDenial {
            code: "permission.local_owner_required",
            reason: "command requires the local node owner",
        });
    }

    if context.role == NetworkRole::TemporaryCoordinator && kind.is_write() {
        return Err(CommandDenial {
            code: "permission.temporary_coordinator_read_only",
            reason: "temporary coordinator is read-only",
        });
    }

    let (action, resource) = policy_target(kind);
    if !role_ceiling_allows(context.role, action, resource) {
        return Err(CommandDenial {
            code: "permission.role_ceiling_exceeded",
            reason: "network role ceiling does not allow the command",
        });
    }

    if !context.local_owner {
        let targets = command_network_targets(command);
        if targets == NetworkTargets::All {
            return Err(CommandDenial {
                code: "permission.network_scope_denied",
                reason: "network-scoped actor cannot select all networks",
            });
        }
        if let NetworkTargets::Named(names) = targets {
            let allowed = context.allowed_networks.as_ref().ok_or(CommandDenial {
                code: "permission.network_scope_denied",
                reason: "network scope is missing",
            })?;
            if names.iter().any(|name| !allowed.contains(name)) {
                return Err(CommandDenial {
                    code: "permission.network_scope_denied",
                    reason: "command target is outside the authenticated network scope",
                });
            }
        }
    }

    Ok(())
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
        | NodeCommandKind::GetNetworkAllocation => NodePermission::ViewState,
        NodeCommandKind::UpdateRouting => NodePermission::ChangeRouting,
        NodeCommandKind::SetNetworkRole => NodePermission::ManageRoles,
        NodeCommandKind::ProbePeer => NodePermission::ProbePeers,
        NodeCommandKind::StopNode | NodeCommandKind::Shutdown => NodePermission::StopNode,
        NodeCommandKind::StartNetworks
        | NodeCommandKind::StopNetworks
        | NodeCommandKind::RestartNetworks
        | NodeCommandKind::CreateNetwork
        | NodeCommandKind::JoinNetwork
        | NodeCommandKind::BanClient
        | NodeCommandKind::UnbanClient
        | NodeCommandKind::UpdateNetworkAllocation
        | NodeCommandKind::StartNode
        | NodeCommandKind::RestartNode
        | NodeCommandKind::CleanupResiduals => NodePermission::ManageNetworks,
    }
}

fn is_local_owner_only(kind: NodeCommandKind) -> bool {
    matches!(
        kind,
        NodeCommandKind::GetDiagnostics
            | NodeCommandKind::CreateNetwork
            | NodeCommandKind::JoinNetwork
            | NodeCommandKind::StartNode
            | NodeCommandKind::StopNode
            | NodeCommandKind::RestartNode
            | NodeCommandKind::CleanupResiduals
            | NodeCommandKind::Shutdown
    )
}

fn policy_target(kind: NodeCommandKind) -> (PolicyAction, ResourceKind) {
    match kind {
        NodeCommandKind::GetSnapshot
        | NodeCommandKind::GetUiManifest
        | NodeCommandKind::GetDiagnostics
        | NodeCommandKind::GetBlacklist
        | NodeCommandKind::GetNetworkAllocation => (PolicyAction::Read, ResourceKind::Network),
        NodeCommandKind::StartNetworks => (PolicyAction::Enable, ResourceKind::Network),
        NodeCommandKind::StopNetworks => (PolicyAction::Disable, ResourceKind::Network),
        NodeCommandKind::RestartNetworks
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
        | NodeCommand::StartNode
        | NodeCommand::StopNode
        | NodeCommand::RestartNode
        | NodeCommand::Shutdown => NetworkTargets::None,
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

    fn actor() -> fuckipv6_common::domain::NodeId {
        fuckipv6_common::domain::NodeId::from_public_key(b"authorization-test-node")
    }

    fn network_context(
        role: NetworkRole,
        capabilities: impl IntoIterator<Item = NodePermission>,
        networks: &[&str],
        membership_active: bool,
    ) -> NodeCommandContext {
        NodeCommandContext::network_session(
            actor(),
            "authorization-test",
            role,
            capabilities,
            networks.iter().map(|network| (*network).to_string()),
            membership_active,
            true,
        )
    }

    fn assert_denied(context: &NodeCommandContext, command: NodeCommand, code: &str) {
        let denial = authorize_command(context, &command).expect_err("command must be denied");
        assert_eq!(denial.code, code);
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
        assert_denied(
            &network_context(
                NetworkRole::Member,
                [NodePermission::ViewState],
                &["home"],
                false,
            ),
            NodeCommand::GetSnapshot,
            "permission.membership_inactive",
        );
        assert_denied(
            &network_context(NetworkRole::Member, [], &["home"], true),
            NodeCommand::GetSnapshot,
            "permission.capability_denied",
        );
    }

    #[test]
    fn local_owner_only_commands_cannot_be_used_by_network_members() {
        assert_denied(
            &network_context(
                NetworkRole::Member,
                [NodePermission::ViewState],
                &["home"],
                true,
            ),
            NodeCommand::GetDiagnostics,
            "permission.local_owner_required",
        );
    }

    #[test]
    fn temporary_coordinator_is_read_only() {
        assert_denied(
            &network_context(
                NetworkRole::TemporaryCoordinator,
                [NodePermission::ManageNetworks],
                &["home"],
                true,
            ),
            NodeCommand::StartNetworks {
                selection: crate::node::NodeNetworkSelection::Named(vec!["home".into()]),
            },
            "permission.temporary_coordinator_read_only",
        );
    }

    #[test]
    fn network_scope_cannot_be_widened_by_the_command_payload() {
        let context = network_context(
            NetworkRole::Administrator,
            [NodePermission::ManageNetworks],
            &["home"],
            true,
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

        authorize_command(
            &network_context(
                NetworkRole::Administrator,
                [NodePermission::ManageNetworks],
                &["home"],
                true,
            ),
            &NodeCommand::StartNetworks {
                selection: crate::node::NodeNetworkSelection::Named(vec!["home".into()]),
            },
        )
        .expect("administrator can act within its authenticated network scope");
    }
}
