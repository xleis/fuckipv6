//! Node identity, ownership, and non-secret credential references.

use super::foundation::{
    require_non_empty, require_unique, ContractIssue, ContractResult, UnixMillis,
};
use super::ids::{CredentialId, NodeId, OwnerId};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::fmt;

pub const NODE_CONTRACT_VERSION: u16 = 1;

/// Base64-encoded public bytes. The representation is deliberately explicit so
/// secret material cannot be confused with public identity data.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Base64Bytes(Vec<u8>);

impl Base64Bytes {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

impl Serialize for Base64Bytes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Base64Bytes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(serde::de::Error::custom)?;
        Ok(Self(bytes))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicKeyAlgorithm {
    Ed25519,
    Other(String),
}

impl PublicKeyAlgorithm {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Ed25519 => "ed25519",
            Self::Other(value) => value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PublicKey {
    pub algorithm: PublicKeyAlgorithm,
    pub key: Base64Bytes,
}

impl PublicKey {
    pub fn ed25519(bytes: [u8; 32]) -> Self {
        Self {
            algorithm: PublicKeyAlgorithm::Ed25519,
            key: Base64Bytes::new(bytes),
        }
    }

    pub fn node_id(&self) -> NodeId {
        NodeId::from_public_key(self.algorithm.as_str(), self.key.as_slice())
    }

    pub fn ed25519_bytes(&self) -> Option<&[u8; 32]> {
        if self.algorithm != PublicKeyAlgorithm::Ed25519 {
            return None;
        }
        self.key.as_slice().try_into().ok()
    }

    pub fn validate(&self, field: &str, issues: &mut Vec<ContractIssue>) {
        require_non_empty(
            self.algorithm.as_str(),
            &format!("{field}.algorithm"),
            64,
            issues,
        );
        if self.key.as_slice().is_empty() {
            issues.push(ContractIssue::new(
                "public_key_required",
                format!("{field}.key"),
                "public key bytes must not be empty",
            ));
        }
        if self.algorithm == PublicKeyAlgorithm::Ed25519
            && self.key.as_slice().len() != ed25519_dalek::PUBLIC_KEY_LENGTH
        {
            issues.push(ContractIssue::new(
                "invalid_public_key_length",
                format!("{field}.key"),
                "Ed25519 public keys must be exactly 32 bytes",
            ));
        }
        if self.key.as_slice().len() > 4096 {
            issues.push(ContractIssue::new(
                "public_key_too_large",
                format!("{field}.key"),
                "public key must not exceed 4096 bytes",
            ));
        }
    }
}

/// A detached Ed25519 signature. Other algorithms must use an explicit
/// signature envelope before they are admitted to a signed contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ed25519Signature([u8; ed25519_dalek::SIGNATURE_LENGTH]);

impl Ed25519Signature {
    pub const fn from_bytes(bytes: [u8; ed25519_dalek::SIGNATURE_LENGTH]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; ed25519_dalek::SIGNATURE_LENGTH] {
        &self.0
    }

    pub fn to_dalek(&self) -> ed25519_dalek::Signature {
        ed25519_dalek::Signature::from_bytes(&self.0)
    }
}

impl Serialize for Ed25519Signature {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(self.0))
    }
}

impl<'de> Deserialize<'de> for Ed25519Signature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(serde::de::Error::custom)?;
        let bytes: [u8; ed25519_dalek::SIGNATURE_LENGTH] = bytes
            .try_into()
            .map_err(|_| serde::de::Error::custom("Ed25519 signatures must be exactly 64 bytes"))?;
        Ok(Self(bytes))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatingSystem {
    Linux,
    Macos,
    Windows,
    Android,
    Ios,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Architecture {
    X86_64,
    Aarch64,
    Armv7,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodePlatform {
    pub os: OperatingSystem,
    pub arch: Architecture,
    pub os_version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeHealthStatus {
    Healthy,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeHealthReason {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeHealth {
    pub status: NodeHealthStatus,
    pub observed_at: UnixMillis,
    pub reasons: Vec<NodeHealthReason>,
}

impl NodeHealth {
    pub fn healthy(observed_at: UnixMillis) -> Self {
        Self {
            status: NodeHealthStatus::Healthy,
            observed_at,
            reasons: Vec::new(),
        }
    }

    fn validate(&self, field: &str, issues: &mut Vec<ContractIssue>) {
        if self.status == NodeHealthStatus::Healthy && !self.reasons.is_empty() {
            issues.push(ContractIssue::new(
                "healthy_with_reasons",
                format!("{field}.reasons"),
                "healthy state must not include failure reasons",
            ));
        }
        if self.status == NodeHealthStatus::Unavailable && self.reasons.is_empty() {
            issues.push(ContractIssue::new(
                "missing_health_reason",
                format!("{field}.reasons"),
                "unavailable state requires at least one reason",
            ));
        }
        for (index, reason) in self.reasons.iter().enumerate() {
            require_non_empty(
                &reason.code,
                &format!("{field}.reasons[{index}].code"),
                128,
                issues,
            );
            require_non_empty(
                &reason.message,
                &format!("{field}.reasons[{index}].message"),
                1024,
                issues,
            );
        }
    }
}

/// Node-local privacy switches. All fields default to the least sharing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodePrivacy {
    pub publish_platform: bool,
    pub publish_version: bool,
    pub publish_addresses: bool,
    pub publish_health_details: bool,
    pub publish_traffic_statistics: bool,
    pub publish_peer_list: bool,
}

/// Public node record. It contains no private key or session token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub schema_version: u16,
    pub id: NodeId,
    pub public_key: PublicKey,
    pub platform: NodePlatform,
    pub app_version: String,
    pub health: NodeHealth,
    pub privacy: NodePrivacy,
}

impl Node {
    pub fn validate(&self) -> ContractResult {
        let mut issues = Vec::new();
        if self.schema_version != NODE_CONTRACT_VERSION {
            issues.push(ContractIssue::new(
                "unsupported_schema_version",
                "schema_version",
                format!("node schema version must be {NODE_CONTRACT_VERSION}"),
            ));
        }
        self.public_key.validate("public_key", &mut issues);
        if self.public_key.node_id() != self.id {
            issues.push(ContractIssue::new(
                "node_id_mismatch",
                "id",
                "node id must be derived from the public key",
            ));
        }
        require_non_empty(&self.app_version, "app_version", 128, &mut issues);
        if let Some(os_version) = self.platform.os_version.as_deref() {
            require_non_empty(os_version, "platform.os_version", 128, &mut issues);
        }
        self.health.validate("health", &mut issues);
        finish(issues)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PasskeyCredentialReference {
    pub credential_id: CredentialId,
    pub label: String,
    /// Opaque relying-party/user-handle reference, never the WebAuthn private key.
    pub user_handle_reference: String,
    pub created_at: UnixMillis,
    pub last_used_at: Option<UnixMillis>,
    pub revoked_at: Option<UnixMillis>,
}

impl PasskeyCredentialReference {
    fn validate(&self, field: &str, issues: &mut Vec<ContractIssue>) {
        require_non_empty(&self.label, &format!("{field}.label"), 128, issues);
        require_non_empty(
            &self.user_handle_reference,
            &format!("{field}.user_handle_reference"),
            256,
            issues,
        );
        if self
            .last_used_at
            .is_some_and(|last_used| last_used.is_before(self.created_at))
        {
            issues.push(ContractIssue::new(
                "invalid_timestamp_order",
                format!("{field}.last_used_at"),
                "last_used_at must not precede created_at",
            ));
        }
        if self
            .revoked_at
            .is_some_and(|revoked| revoked.is_before(self.created_at))
        {
            issues.push(ContractIssue::new(
                "invalid_timestamp_order",
                format!("{field}.revoked_at"),
                "revoked_at must not precede created_at",
            ));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretStorageKind {
    OsKeychain,
    HardwareToken,
    EncryptedFile,
    SessionVault,
}

/// A reference to secret material held by a platform vault. The domain model
/// deliberately has no field capable of carrying the secret itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretReference {
    pub storage: SecretStorageKind,
    pub reference: String,
}

impl SecretReference {
    fn validate(&self, field: &str, issues: &mut Vec<ContractIssue>) {
        require_non_empty(&self.reference, &format!("{field}.reference"), 512, issues);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryMaterialReference {
    pub created_at: UnixMillis,
    pub verifier: SecretReference,
    pub recovery_codes_remaining: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlSessionKind {
    DesktopGui,
    WebBrowser,
    MobileApp,
    Automation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlSessionReference {
    pub session_id: String,
    pub kind: ControlSessionKind,
    pub label: String,
    pub issued_at: UnixMillis,
    pub expires_at: UnixMillis,
}

impl ControlSessionReference {
    fn validate(&self, field: &str, issues: &mut Vec<ContractIssue>) {
        require_non_empty(
            &self.session_id,
            &format!("{field}.session_id"),
            256,
            issues,
        );
        require_non_empty(&self.label, &format!("{field}.label"), 128, issues);
        if !self.issued_at.is_before(self.expires_at) {
            issues.push(ContractIssue::new(
                "invalid_session_expiry",
                format!("{field}.expires_at"),
                "session expiry must be after issue time",
            ));
        }
    }
}

/// The sole owner record for a node. Multiple passkeys may authenticate that
/// owner, but ownership itself is intentionally singular.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeOwner {
    pub node_id: NodeId,
    pub owner_id: OwnerId,
    pub display_name: String,
    pub initialized: bool,
    pub passkeys: Vec<PasskeyCredentialReference>,
    pub recovery: Option<RecoveryMaterialReference>,
    pub control_sessions: Vec<ControlSessionReference>,
}

impl NodeOwner {
    pub fn validate(&self) -> ContractResult {
        let mut issues = Vec::new();
        require_non_empty(&self.display_name, "display_name", 128, &mut issues);

        let passkey_ids = self
            .passkeys
            .iter()
            .map(|credential| credential.credential_id)
            .collect::<Vec<_>>();
        require_unique(&passkey_ids, "passkeys", &mut issues);
        for (index, passkey) in self.passkeys.iter().enumerate() {
            passkey.validate(&format!("passkeys[{index}]"), &mut issues);
        }

        let session_ids = self
            .control_sessions
            .iter()
            .map(|session| session.session_id.as_str())
            .collect::<Vec<_>>();
        require_unique(&session_ids, "control_sessions", &mut issues);
        for (index, session) in self.control_sessions.iter().enumerate() {
            session.validate(&format!("control_sessions[{index}]"), &mut issues);
        }

        if self.initialized && self.passkeys.is_empty() {
            issues.push(ContractIssue::new(
                "passkey_required",
                "passkeys",
                "an initialized owner requires at least one passkey",
            ));
        }
        if let Some(recovery) = &self.recovery {
            recovery.verifier.validate("recovery.verifier", &mut issues);
        }

        finish(issues)
    }
}

fn finish(issues: Vec<ContractIssue>) -> ContractResult {
    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

impl fmt::Display for PublicKeyAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn public_key(byte: u8) -> PublicKey {
        PublicKey::ed25519([byte; 32])
    }

    fn node() -> Node {
        let public_key = public_key(1);
        Node {
            schema_version: NODE_CONTRACT_VERSION,
            id: public_key.node_id(),
            public_key,
            platform: NodePlatform {
                os: OperatingSystem::Linux,
                arch: Architecture::X86_64,
                os_version: Some("test".into()),
            },
            app_version: "0.3.0".into(),
            health: NodeHealth::healthy(UnixMillis(1)),
            privacy: NodePrivacy::default(),
        }
    }

    #[test]
    fn node_round_trips_and_verifies_derived_id() {
        let node = node();
        node.validate().expect("valid node");
        let json = serde_json::to_string(&node).expect("serialize");
        let decoded: Node = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, node);
    }

    #[test]
    fn rejects_mismatched_node_id_and_diagnostics_on_healthy_node() {
        let mut node = node();
        node.id = NodeId::from_bytes([9; 32]);
        node.health.reasons.push(NodeHealthReason {
            code: "x".into(),
            message: "x".into(),
            retryable: true,
        });
        let codes = node
            .validate()
            .expect_err("invalid")
            .into_iter()
            .map(|issue| issue.code)
            .collect::<Vec<_>>();
        assert!(codes.contains(&"node_id_mismatch".to_string()));
        assert!(codes.contains(&"healthy_with_reasons".to_string()));
    }

    #[test]
    fn owner_has_single_identity_with_multiple_passkey_references() {
        let node = node();
        let owner = NodeOwner {
            node_id: node.id,
            owner_id: OwnerId::from_bytes([2; 16]),
            display_name: "Owner".into(),
            initialized: true,
            passkeys: vec![
                PasskeyCredentialReference {
                    credential_id: CredentialId::from_bytes([3; 16]),
                    label: "Laptop".into(),
                    user_handle_reference: "owner".into(),
                    created_at: UnixMillis(1),
                    last_used_at: Some(UnixMillis(2)),
                    revoked_at: None,
                },
                PasskeyCredentialReference {
                    credential_id: CredentialId::from_bytes([4; 16]),
                    label: "Phone".into(),
                    user_handle_reference: "owner".into(),
                    created_at: UnixMillis(1),
                    last_used_at: None,
                    revoked_at: None,
                },
            ],
            recovery: Some(RecoveryMaterialReference {
                created_at: UnixMillis(1),
                verifier: SecretReference {
                    storage: SecretStorageKind::OsKeychain,
                    reference: "vault://recovery/1".into(),
                },
                recovery_codes_remaining: 8,
            }),
            control_sessions: vec![ControlSessionReference {
                session_id: "session-1".into(),
                kind: ControlSessionKind::WebBrowser,
                label: "Browser".into(),
                issued_at: UnixMillis(1),
                expires_at: UnixMillis(2),
            }],
        };
        owner.validate().expect("valid owner");

        let mut duplicate = owner;
        duplicate.passkeys[1].credential_id = duplicate.passkeys[0].credential_id;
        assert!(duplicate.validate().is_err());
    }

    #[test]
    fn owner_initialization_requires_a_passkey() {
        let owner = NodeOwner {
            node_id: NodeId::from_bytes([1; 32]),
            owner_id: OwnerId::from_bytes([2; 16]),
            display_name: "Owner".into(),
            initialized: true,
            passkeys: Vec::new(),
            recovery: None,
            control_sessions: Vec::new(),
        };
        assert!(owner
            .validate()
            .expect_err("missing passkey")
            .iter()
            .any(|issue| issue.code == "passkey_required"));
    }
}
