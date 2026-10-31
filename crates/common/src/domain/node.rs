use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ids::{NetworkId, NodeId, PasskeyCredentialId, PublicKey, SecretRef, SessionRef};
use super::{DomainError, DomainResult};

/// Host platform exposed by a node. This is descriptive data, not a security
/// boundary; capability availability must still come from runtime probes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodePlatform {
    Linux,
    Windows,
    Macos,
    Android,
    Other,
}

/// Semantic version string used in discovery and compatibility checks.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(try_from = "String")]
pub struct NodeVersion(
    #[schemars(regex(pattern = "^[0-9]+\\.[0-9]+\\.[0-9]+(?:[-+][A-Za-z0-9.-]+)?$"))] String,
);

impl NodeVersion {
    pub fn parse(value: impl Into<String>) -> DomainResult<Self> {
        let value = value.into();
        let core = value
            .split(['-', '+'])
            .next()
            .ok_or_else(|| DomainError::invalid_value("version is empty"))?;
        let mut parts = core.split('.');
        let valid = parts.next().is_some_and(is_numeric)
            && parts.next().is_some_and(is_numeric)
            && parts.next().is_some_and(is_numeric)
            && parts.next().is_none()
            && value.len() <= 64;
        if !valid {
            return Err(DomainError::invalid_value("version must be semantic x.y.z"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_numeric(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

impl TryFrom<String> for NodeVersion {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<NodeVersion> for String {
    fn from(value: NodeVersion) -> Self {
        value.0
    }
}

/// Outcome of the latest health probe. `Unknown` is never projected as healthy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeHealthStatus {
    Healthy,
    Degraded,
    Unreachable,
    #[default]
    Unknown,
}

/// Health observation with a bounded reason code list. Free-form diagnostics
/// belong in a privacy-filtered diagnostic object, not in this shared field.
#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeHealth {
    pub status: NodeHealthStatus,
    pub observed_at_unix_ms: Option<u64>,
    #[serde(default)]
    pub reason_codes: Vec<String>,
}

impl NodeHealth {
    pub fn is_healthy(&self) -> bool {
        self.status == NodeHealthStatus::Healthy
    }

    pub fn validate(&self) -> DomainResult<()> {
        if self.reason_codes.len() > 16 {
            return Err(DomainError::invariant("node health has too many reasons"));
        }
        if self
            .reason_codes
            .iter()
            .any(|reason| reason.is_empty() || reason.len() > 96)
        {
            return Err(DomainError::invariant("node health reason is invalid"));
        }
        Ok(())
    }
}

/// Reference to a WebAuthn credential. No credential private material is ever
/// accepted by this type.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct PasskeyCredentialRef {
    pub credential_id: PasskeyCredentialId,
    pub label: String,
    pub created_at_unix_ms: u64,
    pub last_used_at_unix_ms: Option<u64>,
    pub revoked_at_unix_ms: Option<u64>,
    pub backup_eligible: bool,
    pub backup_state: bool,
}

impl PasskeyCredentialRef {
    pub fn is_active(&self) -> bool {
        self.revoked_at_unix_ms.is_none()
    }

    pub fn validate(&self) -> DomainResult<()> {
        if self.label.is_empty() || self.label.len() > 80 {
            return Err(DomainError::invalid_value("passkey label is invalid"));
        }
        if self
            .last_used_at_unix_ms
            .is_some_and(|used| used < self.created_at_unix_ms)
        {
            return Err(DomainError::invariant(
                "passkey last-used time predates creation",
            ));
        }
        if self
            .revoked_at_unix_ms
            .is_some_and(|revoked| revoked < self.created_at_unix_ms)
        {
            return Err(DomainError::invariant(
                "passkey revocation time predates creation",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "state", content = "data", rename_all = "snake_case")]
pub enum RecoveryState {
    NotConfigured,
    Configured {
        secret_ref: SecretRef,
        configured_at_unix_ms: u64,
    },
    RotationRequired {
        secret_ref: SecretRef,
        reason: String,
    },
}

impl RecoveryState {
    pub fn secret_ref(&self) -> Option<&SecretRef> {
        match self {
            Self::NotConfigured => None,
            Self::Configured { secret_ref, .. } | Self::RotationRequired { secret_ref, .. } => {
                Some(secret_ref)
            }
        }
    }
}

/// The single local owner of a node and its credential/session references.
///
/// Network administrators are deliberately absent: they cannot become the
/// owner of another node through network governance.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeOwner {
    pub node_id: NodeId,
    pub display_name: String,
    pub passkeys: Vec<PasskeyCredentialRef>,
    pub recovery: RecoveryState,
    #[serde(default)]
    pub sessions: Vec<SessionRef>,
}

impl NodeOwner {
    pub fn active_passkeys(&self) -> impl Iterator<Item = &PasskeyCredentialRef> {
        self.passkeys.iter().filter(|passkey| passkey.is_active())
    }

    pub fn validate_for_node(&self, node_id: &NodeId) -> DomainResult<()> {
        if &self.node_id != node_id {
            return Err(DomainError::invariant("owner belongs to another node"));
        }
        if self.display_name.is_empty() || self.display_name.len() > 80 {
            return Err(DomainError::invalid_value("owner display name is invalid"));
        }
        if self.active_passkeys().next().is_none() {
            return Err(DomainError::invariant(
                "owned node must retain an active passkey",
            ));
        }
        let mut credentials = std::collections::HashSet::new();
        for passkey in &self.passkeys {
            passkey.validate()?;
            if !credentials.insert(&passkey.credential_id) {
                return Err(DomainError::invariant("duplicate passkey credential"));
            }
        }
        let mut sessions = std::collections::HashSet::new();
        for session in &self.sessions {
            if !sessions.insert(session) {
                return Err(DomainError::invariant("duplicate owner session"));
            }
        }
        Ok(())
    }
}

/// Visibility scope for a node-local field. These are labels over a server-side
/// authorization result, never client-side enforcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldVisibility {
    NetworkMembers,
    NetworkAdmins,
    NodeOwner,
}

/// Privacy defaults for node-local fields. Secrets and traffic contents are
/// intentionally unrepresentable and therefore can never be made visible here.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodePrivacySettings {
    pub display_name: FieldVisibility,
    pub public_key: FieldVisibility,
    pub platform_and_version: FieldVisibility,
    pub online_state: FieldVisibility,
    pub endpoints: FieldVisibility,
    pub real_network_addresses: FieldVisibility,
    pub traffic_statistics: FieldVisibility,
    pub local_processes: FieldVisibility,
    pub local_paths: FieldVisibility,
    pub detailed_logs: FieldVisibility,
}

impl Default for NodePrivacySettings {
    fn default() -> Self {
        Self {
            display_name: FieldVisibility::NetworkMembers,
            public_key: FieldVisibility::NetworkMembers,
            platform_and_version: FieldVisibility::NetworkMembers,
            online_state: FieldVisibility::NetworkMembers,
            endpoints: FieldVisibility::NetworkMembers,
            real_network_addresses: FieldVisibility::NetworkAdmins,
            traffic_statistics: FieldVisibility::NetworkAdmins,
            local_processes: FieldVisibility::NodeOwner,
            local_paths: FieldVisibility::NodeOwner,
            detailed_logs: FieldVisibility::NodeOwner,
        }
    }
}

impl NodePrivacySettings {
    pub fn validate(&self) -> DomainResult<()> {
        if self.real_network_addresses == FieldVisibility::NetworkMembers {
            return Err(DomainError::invariant(
                "real network addresses cannot be visible to all members",
            ));
        }
        if self.local_processes != FieldVisibility::NodeOwner
            || self.local_paths != FieldVisibility::NodeOwner
            || self.detailed_logs != FieldVisibility::NodeOwner
        {
            return Err(DomainError::invariant(
                "local-only fields can only be visible to the node owner",
            ));
        }
        Ok(())
    }
}

/// A node may act as a local owner in its own control plane and, separately, as
/// a member of any number of networks. These scopes never imply one another.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "scope", content = "network_id", rename_all = "snake_case")]
pub enum NodeRoleBinding {
    LocalOwner,
    NetworkMember(NetworkId),
}

impl NodeRoleBinding {
    pub fn is_local_owner(&self) -> bool {
        matches!(self, Self::LocalOwner)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeRecord {
    pub node_id: NodeId,
    pub public_key: PublicKey,
    pub platform: NodePlatform,
    pub version: NodeVersion,
    pub created_at_unix_ms: u64,
    pub health: NodeHealth,
    /// Missing only during the uninitialized bootstrap window.
    pub owner: Option<NodeOwner>,
    pub privacy: NodePrivacySettings,
    #[serde(default)]
    pub role_bindings: Vec<NodeRoleBinding>,
}

impl NodeRecord {
    pub fn validate(&self) -> DomainResult<()> {
        let expected = NodeId::from_public_key(self.public_key.as_str().as_bytes());
        if self.node_id != expected {
            return Err(DomainError::invariant(
                "node id does not match the public key",
            ));
        }
        self.health.validate()?;
        self.privacy.validate()?;
        if let Some(owner) = &self.owner {
            owner.validate_for_node(&self.node_id)?;
        }
        let mut bindings = std::collections::HashSet::new();
        for binding in &self.role_bindings {
            let key = match binding {
                NodeRoleBinding::LocalOwner => "local_owner".to_owned(),
                NodeRoleBinding::NetworkMember(network_id) => {
                    format!("network_member:{}", network_id.as_str())
                }
            };
            if !bindings.insert(key) {
                return Err(DomainError::invariant("duplicate node role binding"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn passkey(byte: u8) -> PasskeyCredentialRef {
        PasskeyCredentialRef {
            credential_id: PasskeyCredentialId::from_public_key(&[byte; 32]),
            label: format!("device-{byte}"),
            created_at_unix_ms: 100,
            last_used_at_unix_ms: None,
            revoked_at_unix_ms: None,
            backup_eligible: true,
            backup_state: false,
        }
    }

    fn node() -> NodeRecord {
        let public_key = PublicKey::from_bytes([1; 32]);
        let node_id = NodeId::from_public_key(public_key.as_str().as_bytes());
        NodeRecord {
            node_id: node_id.clone(),
            public_key,
            platform: NodePlatform::Linux,
            version: NodeVersion::parse("0.3.0").unwrap(),
            created_at_unix_ms: 10,
            health: NodeHealth {
                status: NodeHealthStatus::Healthy,
                observed_at_unix_ms: Some(20),
                reason_codes: Vec::new(),
            },
            owner: Some(NodeOwner {
                node_id,
                display_name: "Owner".into(),
                passkeys: vec![passkey(2), passkey(3)],
                recovery: RecoveryState::Configured {
                    secret_ref: SecretRef::parse("keychain:relay/recovery").unwrap(),
                    configured_at_unix_ms: 30,
                },
                sessions: Vec::new(),
            }),
            privacy: NodePrivacySettings::default(),
            role_bindings: vec![NodeRoleBinding::LocalOwner],
        }
    }

    #[test]
    fn node_owner_supports_multiple_passkeys_and_round_trips() {
        let node = node();
        node.validate().unwrap();
        let encoded = serde_json::to_value(&node).unwrap();
        assert_eq!(encoded["owner"]["passkeys"].as_array().unwrap().len(), 2);
        let decoded: NodeRecord = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(decoded, node);

        let secret = encoded.to_string().to_ascii_lowercase();
        assert!(!secret.contains("private_key"));
        assert!(!secret.contains("credential_secret"));
    }

    #[test]
    fn owner_and_network_member_roles_are_separate_scopes() {
        let node = node();
        let network_id = NetworkId::from_public_key(b"network");
        let mut member = node.clone();
        member
            .role_bindings
            .push(NodeRoleBinding::NetworkMember(network_id));
        assert!(member.role_bindings[0].is_local_owner());
        assert!(!member.role_bindings[1].is_local_owner());
    }

    #[test]
    fn node_id_must_be_derived_from_public_key() {
        let mut node = node();
        node.node_id = NodeId::from_public_key(b"another key");
        assert_eq!(
            node.validate().unwrap_err().code(),
            "domain.invariant_violation"
        );
    }

    #[test]
    fn owned_node_requires_an_active_passkey() {
        let mut node = node();
        for passkey in &mut node.owner.as_mut().unwrap().passkeys {
            passkey.revoked_at_unix_ms = Some(200);
        }
        assert!(node.validate().is_err());
    }

    #[test]
    fn privacy_cannot_expose_local_only_or_real_address_fields() {
        let privacy = NodePrivacySettings {
            local_processes: FieldVisibility::NetworkMembers,
            ..NodePrivacySettings::default()
        };
        assert!(privacy.validate().is_err());

        let privacy = NodePrivacySettings {
            real_network_addresses: FieldVisibility::NetworkMembers,
            ..NodePrivacySettings::default()
        };
        assert!(privacy.validate().is_err());
    }

    #[test]
    fn malformed_version_and_recovery_reference_fail_closed() {
        assert!(NodeVersion::parse("1.2").is_err());
        assert!(NodeVersion::parse("1.2.3.4").is_err());
        assert!(serde_json::from_value::<NodeVersion>(json!("latest")).is_err());
        assert!(SecretRef::parse("literal:password").is_err());
    }
}
