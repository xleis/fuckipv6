use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ids::{NetworkId, NodeId};
use super::node::NodeHealth;
use super::permissions::NetworkRole;
use super::{DomainError, DomainResult};

/// The fixed first-version capability set. Unknown wire values fail closed at
/// deserialization and are never inferred from a free-form label.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityKind {
    Coordinator,
    Relay,
    Exit,
    File,
}

impl CapabilityKind {
    /// Ring order is stable and shared by GPUI, WebUI and Android.
    pub const ALL: [Self; 4] = [Self::Coordinator, Self::Relay, Self::Exit, Self::File];

    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Coordinator => "coordinator",
            Self::Relay => "relay",
            Self::Exit => "exit",
            Self::File => "file",
        }
    }

    pub const fn icon_token(self) -> &'static str {
        match self {
            Self::Coordinator => "capability.coordinator",
            Self::Relay => "capability.relay",
            Self::Exit => "capability.exit",
            Self::File => "capability.file",
        }
    }

    /// Colors are semantic tokens, not theme-specific hex values.
    pub const fn color_token(self) -> &'static str {
        match self {
            Self::Coordinator => "capability.coordinator",
            Self::Relay => "capability.relay",
            Self::Exit => "capability.exit",
            Self::File => "capability.file",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    Disabled,
    Starting,
    Enabled,
    Degraded,
    Unavailable,
    Unsupported,
    /// A migrated record may preserve an unreadable state without making the
    /// capability usable. New writes may not create this state.
    Unknown,
}

impl CapabilityState {
    pub fn is_operational(self) -> bool {
        matches!(self, Self::Enabled | Self::Degraded)
    }
}

/// Authorization and disclosure are separate. A member can discover that a
/// capability exists without being allowed to use it.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct CapabilityAuthorization {
    pub discoverable: bool,
    pub usable_by_members: bool,
    pub requires_admin_grant: bool,
}

impl Default for CapabilityAuthorization {
    fn default() -> Self {
        Self {
            discoverable: true,
            usable_by_members: false,
            requires_admin_grant: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct CapabilityLimits {
    pub max_bandwidth_kbps: Option<u64>,
    pub max_connections: Option<u32>,
    pub max_storage_bytes: Option<u64>,
    pub quota_reset_at_unix_ms: Option<u64>,
}

impl CapabilityLimits {
    pub fn validate(&self) -> DomainResult<()> {
        if self.max_bandwidth_kbps == Some(0)
            || self.max_connections == Some(0)
            || self.max_storage_bytes == Some(0)
        {
            return Err(DomainError::invalid_value(
                "capability limits must be positive when present",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct CapabilityRegistration {
    pub network_id: NetworkId,
    pub node_id: NodeId,
    pub kind: CapabilityKind,
    pub state: CapabilityState,
    pub authorization: CapabilityAuthorization,
    pub limits: CapabilityLimits,
    pub health: NodeHealth,
    pub updated_at_unix_ms: u64,
}

impl CapabilityRegistration {
    pub fn validate(&self) -> DomainResult<()> {
        self.limits.validate()?;
        self.health.validate()?;
        if self.state == CapabilityState::Unknown {
            return Err(DomainError::invalid_value(
                "unknown capability state cannot be newly registered",
            ));
        }
        Ok(())
    }

    pub fn is_usable_by(&self, role: NetworkRole, administrator_granted: bool) -> bool {
        if !self.state.is_operational() || !self.authorization.discoverable {
            return false;
        }
        match role {
            NetworkRole::Owner | NetworkRole::Administrator => {
                !self.authorization.requires_admin_grant || administrator_granted
            }
            NetworkRole::Member => {
                self.authorization.usable_by_members
                    && (!self.authorization.requires_admin_grant || administrator_granted)
            }
            // Temporary Coordinators coordinate the existing runtime; they do
            // not gain data-plane or file capability grants.
            NetworkRole::TemporaryCoordinator => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct CapabilityRegistry {
    pub network_id: NetworkId,
    pub registrations: Vec<CapabilityRegistration>,
}

impl CapabilityRegistry {
    pub fn validate(&self) -> DomainResult<()> {
        let mut keys = BTreeSet::new();
        for registration in &self.registrations {
            if registration.network_id != self.network_id {
                return Err(DomainError::invariant(
                    "capability registration belongs to another network",
                ));
            }
            registration.validate()?;
            if !keys.insert((registration.node_id.clone(), registration.kind)) {
                return Err(DomainError::invariant(
                    "duplicate node capability registration",
                ));
            }
        }
        Ok(())
    }

    pub fn find(&self, node_id: &NodeId, kind: CapabilityKind) -> Option<&CapabilityRegistration> {
        self.registrations
            .iter()
            .find(|registration| &registration.node_id == node_id && registration.kind == kind)
    }

    /// Missing and unknown registrations are indistinguishable and unusable.
    pub fn is_usable(
        &self,
        node_id: &NodeId,
        kind: CapabilityKind,
        role: NetworkRole,
        administrator_granted: bool,
    ) -> bool {
        self.find(node_id, kind)
            .is_some_and(|registration| registration.is_usable_by(role, administrator_granted))
    }

    pub fn visible_capabilities(&self, node_id: &NodeId) -> Vec<CapabilityKind> {
        CapabilityKind::ALL
            .into_iter()
            .filter(|kind| {
                self.find(node_id, *kind)
                    .is_some_and(|registration| registration.authorization.discoverable)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn registration(kind: CapabilityKind, state: CapabilityState) -> CapabilityRegistration {
        CapabilityRegistration {
            network_id: NetworkId::from_public_key(b"network"),
            node_id: NodeId::from_public_key(b"node"),
            kind,
            state,
            authorization: CapabilityAuthorization {
                discoverable: true,
                usable_by_members: true,
                requires_admin_grant: false,
            },
            limits: CapabilityLimits::default(),
            health: NodeHealth::default(),
            updated_at_unix_ms: 1,
        }
    }

    #[test]
    fn ring_order_and_semantic_tokens_are_stable() {
        assert_eq!(
            CapabilityKind::ALL.map(CapabilityKind::wire_name),
            ["coordinator", "relay", "exit", "file"]
        );
        assert_eq!(CapabilityKind::Exit.icon_token(), "capability.exit");
        assert_eq!(CapabilityKind::Exit.color_token(), "capability.exit");
    }

    #[test]
    fn unknown_capabilities_fail_closed() {
        assert!(serde_json::from_value::<CapabilityKind>(json!("dns")).is_err());
        let registry = CapabilityRegistry {
            network_id: NetworkId::from_public_key(b"network"),
            registrations: vec![registration(
                CapabilityKind::Relay,
                CapabilityState::Disabled,
            )],
        };
        let node = NodeId::from_public_key(b"node");
        assert!(!registry.is_usable(&node, CapabilityKind::Relay, NetworkRole::Member, false));
        assert!(!registry.is_usable(&node, CapabilityKind::Exit, NetworkRole::Member, false));
        assert_eq!(
            registry.visible_capabilities(&node),
            vec![CapabilityKind::Relay]
        );
    }

    #[test]
    fn member_use_requires_state_authorization_and_optional_grant() {
        let node = NodeId::from_public_key(b"node");
        let mut entry = registration(CapabilityKind::Exit, CapabilityState::Enabled);
        assert!(entry.is_usable_by(NetworkRole::Member, false));
        entry.authorization.usable_by_members = false;
        assert!(!entry.is_usable_by(NetworkRole::Member, true));
        assert!(entry.is_usable_by(NetworkRole::Administrator, false));
        entry.authorization.requires_admin_grant = true;
        assert!(!entry.is_usable_by(NetworkRole::Administrator, false));
        assert!(entry.is_usable_by(NetworkRole::Administrator, true));
        entry.state = CapabilityState::Unknown;
        assert!(entry.validate().is_err());
        assert!(!entry.is_usable_by(NetworkRole::Owner, true));
        let _ = node;
    }

    #[test]
    fn registry_rejects_duplicate_and_cross_network_entries() {
        let registry = CapabilityRegistry {
            network_id: NetworkId::from_public_key(b"network"),
            registrations: vec![
                registration(CapabilityKind::Relay, CapabilityState::Enabled),
                registration(CapabilityKind::Relay, CapabilityState::Disabled),
            ],
        };
        assert!(registry.validate().is_err());
        let mut registry = registry;
        registry.registrations[1].network_id = NetworkId::from_public_key(b"other");
        assert!(registry.validate().is_err());
    }
}
