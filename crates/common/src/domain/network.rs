use std::collections::BTreeSet;

use ed25519_dalek::{Signature as DalekSignature, Verifier, VerifyingKey};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ids::{NetworkId, NodeId, PublicKey, Signature};
use super::{DomainError, DomainResult};

/// How much of a network's public profile is discoverable without membership.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkVisibility {
    Public,
    #[default]
    Unlisted,
    Private,
}

/// Admission policy selected by the network owner.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkJoinMode {
    /// Requests are accepted immediately.
    Open,
    /// Requests enter `pending_approval`.
    ApprovalRequired,
    /// The node must first consume a signed, single-use invitation.
    #[default]
    InviteOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryTransport {
    Quic,
    Tcp,
    Local,
}

/// A decoupled discovery hint. It is never part of the stable network identity.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkEndpoint {
    pub transport: DiscoveryTransport,
    pub host: String,
    pub port: u16,
}

impl NetworkEndpoint {
    fn validate(&self) -> DomainResult<()> {
        if self.host.is_empty() || self.host.len() > 253 || self.port == 0 {
            return Err(DomainError::invalid_value("network endpoint is invalid"));
        }
        Ok(())
    }
}

/// Stable network metadata. The identifier is derived from `public_key`, not
/// from a Coordinator hostname, endpoint or mutable display name.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkRecord {
    pub network_id: NetworkId,
    pub public_key: PublicKey,
    pub name: String,
    pub purpose: String,
    /// Stable, machine-readable icon token. It is presentation metadata and is
    /// not part of the network identifier.
    #[serde(default = "default_network_icon")]
    pub icon: String,
    /// BCP-47-like language tag for the network's public profile.
    #[serde(default = "default_network_language")]
    pub language: String,
    pub visibility: NetworkVisibility,
    pub join_mode: NetworkJoinMode,
    pub owner: NodeId,
    #[serde(default)]
    pub administrators: Vec<NodeId>,
    #[serde(default)]
    pub members: Vec<NodeId>,
    pub revision: u64,
    pub policy_revision: u64,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

impl NetworkRecord {
    pub fn validate(&self) -> DomainResult<()> {
        let expected = NetworkId::from_public_key(self.public_key.as_str().as_bytes());
        if self.network_id != expected {
            return Err(DomainError::invariant(
                "network id does not match the network public key",
            ));
        }
        validate_network_name(&self.name)?;
        validate_network_purpose(&self.purpose)?;
        validate_network_icon(&self.icon)?;
        validate_network_language(&self.language)?;
        if self.revision == 0 || self.policy_revision == 0 {
            return Err(DomainError::invalid_value(
                "network and policy revisions must be positive",
            ));
        }
        if self.updated_at_unix_ms < self.created_at_unix_ms {
            return Err(DomainError::invariant(
                "network update time predates creation",
            ));
        }
        let mut administrators = BTreeSet::new();
        for administrator in &self.administrators {
            if !administrators.insert(administrator) {
                return Err(DomainError::invariant("duplicate network administrator"));
            }
        }
        if administrators.contains(&self.owner) {
            return Err(DomainError::invariant(
                "network owner must not also be listed as administrator",
            ));
        }
        let mut members = BTreeSet::new();
        for member in &self.members {
            if !members.insert(member) {
                return Err(DomainError::invariant("duplicate network member"));
            }
        }
        if !members.contains(&self.owner)
            || self
                .administrators
                .iter()
                .any(|administrator| !members.contains(administrator))
        {
            return Err(DomainError::invariant(
                "network owner and administrators must be members",
            ));
        }
        Ok(())
    }

    /// Create the first revision owned by the identity that created the
    /// network. The owner is a member with full owner authority; the separate
    /// administrator list starts empty because the domain forbids duplicating
    /// the owner there.
    pub fn create_owned(
        profile: NetworkCreateProfile,
        public_key: PublicKey,
        owner: NodeId,
        now_unix_ms: u64,
    ) -> DomainResult<Self> {
        let record = Self {
            network_id: NetworkId::from_public_key(public_key.as_str().as_bytes()),
            public_key,
            name: profile.name,
            purpose: profile.purpose,
            icon: profile.icon,
            language: profile.language,
            visibility: profile.visibility,
            join_mode: profile.join_mode,
            owner: owner.clone(),
            administrators: Vec::new(),
            members: vec![owner],
            revision: 1,
            policy_revision: 1,
            created_at_unix_ms: now_unix_ms,
            updated_at_unix_ms: now_unix_ms,
        };
        record.validate()?;
        Ok(record)
    }

    /// Rename mutable profile metadata without changing the identifier derived
    /// from the immutable network public key.
    pub fn rename(&mut self, new_name: impl Into<String>, now_unix_ms: u64) -> DomainResult<()> {
        let new_name = new_name.into();
        validate_network_name(&new_name)?;
        if now_unix_ms < self.created_at_unix_ms {
            return Err(DomainError::invariant(
                "network update time predates creation",
            ));
        }
        if self.name != new_name {
            self.name = new_name;
            self.revision = self.revision.saturating_add(1);
            self.updated_at_unix_ms = self.updated_at_unix_ms.max(now_unix_ms);
        }
        self.validate()
    }
}

/// Owner-supplied public profile used to create a network. Hostnames, ports and
/// runtime settings deliberately do not participate in this contract.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkCreateProfile {
    pub name: String,
    pub purpose: String,
    pub icon: String,
    pub language: String,
    pub visibility: NetworkVisibility,
    pub join_mode: NetworkJoinMode,
}

impl NetworkCreateProfile {
    pub fn validate(&self) -> DomainResult<()> {
        validate_network_name(&self.name)?;
        validate_network_purpose(&self.purpose)?;
        validate_network_icon(&self.icon)?;
        validate_network_language(&self.language)
    }
}

pub const DEFAULT_NETWORK_PURPOSE: &str = "Owner-created network";
pub const DEFAULT_NETWORK_ICON: &str = "network";
pub const DEFAULT_NETWORK_LANGUAGE: &str = "und";

pub fn default_network_purpose() -> String {
    DEFAULT_NETWORK_PURPOSE.into()
}

pub fn default_network_icon() -> String {
    DEFAULT_NETWORK_ICON.into()
}

pub fn default_network_language() -> String {
    DEFAULT_NETWORK_LANGUAGE.into()
}

fn validate_network_name(name: &str) -> DomainResult<()> {
    if name.is_empty() || name.len() > 80 || name.trim() != name || name == "." || name == ".." {
        return Err(DomainError::invalid_value("network name is invalid"));
    }
    if name.chars().any(|character| {
        !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
    }) {
        return Err(DomainError::invalid_value("network name is invalid"));
    }
    Ok(())
}

fn validate_network_purpose(purpose: &str) -> DomainResult<()> {
    if purpose.is_empty()
        || purpose.chars().count() > 512
        || purpose.trim() != purpose
        || purpose.chars().any(char::is_control)
    {
        return Err(DomainError::invalid_value("network purpose is invalid"));
    }
    Ok(())
}

fn validate_network_icon(icon: &str) -> DomainResult<()> {
    if icon.is_empty()
        || icon.len() > 64
        || icon.trim() != icon
        || icon.chars().any(|character| {
            !(character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_' | '.'))
        })
    {
        return Err(DomainError::invalid_value("network icon is invalid"));
    }
    Ok(())
}

fn validate_network_language(language: &str) -> DomainResult<()> {
    let mut segments = language.split('-');
    let Some(primary) = segments.next() else {
        return Err(DomainError::invalid_value("network language is invalid"));
    };
    if language.len() > 35
        || !(2..=8).contains(&primary.len())
        || !primary
            .chars()
            .all(|character| character.is_ascii_alphabetic())
        || segments.any(|segment| {
            !(1..=8).contains(&segment.len())
                || !segment
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric())
        })
    {
        return Err(DomainError::invalid_value("network language is invalid"));
    }
    Ok(())
}

/// Signed discovery data. Endpoints may change between revisions; the stable
/// network ID remains bound to the immutable network public key.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkDiscoveryDocument {
    pub network_id: NetworkId,
    pub public_key: PublicKey,
    pub name: String,
    pub purpose: String,
    pub visibility: NetworkVisibility,
    pub join_mode: NetworkJoinMode,
    pub owner: NodeId,
    pub revision: u64,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    #[serde(default)]
    pub endpoints: Vec<NetworkEndpoint>,
}

impl NetworkDiscoveryDocument {
    pub fn validate_at(&self, now_unix_ms: u64) -> DomainResult<()> {
        let expected = NetworkId::from_public_key(self.public_key.as_str().as_bytes());
        if self.network_id != expected {
            return Err(DomainError::invariant(
                "discovery network id does not match its public key",
            ));
        }
        if self.name.is_empty() || self.name.len() > 80 {
            return Err(DomainError::invalid_value(
                "discovery network name is invalid",
            ));
        }
        if self.purpose.is_empty() || self.purpose.len() > 512 {
            return Err(DomainError::invalid_value("discovery purpose is invalid"));
        }
        if self.revision == 0 {
            return Err(DomainError::invalid_value(
                "discovery revision must be positive",
            ));
        }
        if self.issued_at_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::invariant(
                "discovery validity interval is empty",
            ));
        }
        if now_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::new(
                "domain.discovery_expired",
                "network discovery record has expired",
            ));
        }
        if self.endpoints.len() > 32 {
            return Err(DomainError::invariant(
                "discovery record has too many endpoints",
            ));
        }
        for endpoint in &self.endpoints {
            endpoint.validate()?;
        }
        Ok(())
    }

    pub fn signing_payload(&self) -> DomainResult<Vec<u8>> {
        serde_json::to_vec(self)
            .map_err(|_| DomainError::invalid_value("discovery payload cannot be encoded"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NetworkDiscoveryRecord {
    pub document: NetworkDiscoveryDocument,
    pub signature: Signature,
}

impl NetworkDiscoveryRecord {
    pub fn validate_at(&self, now_unix_ms: u64) -> DomainResult<()> {
        self.document.validate_at(now_unix_ms)?;
        let key = VerifyingKey::from_bytes(&self.document.public_key.to_bytes()?)
            .map_err(|_| DomainError::invalid_value("network public key is invalid"))?;
        let signature = DalekSignature::from_bytes(&self.signature.to_bytes()?);
        key.verify(&self.document.signing_payload()?, &signature)
            .map_err(|_| {
                DomainError::new(
                    "domain.invalid_signature",
                    "network discovery signature is invalid",
                )
            })
    }

    /// Accept only a signed, unexpired, strictly newer record. Equal revisions
    /// with different bytes are treated as an equivocation attempt.
    pub fn validate_successor(
        &self,
        current_revision: u64,
        current_digest: Option<&str>,
        now_unix_ms: u64,
    ) -> DomainResult<()> {
        self.validate_at(now_unix_ms)?;
        if self.document.revision < current_revision {
            return Err(DomainError::new(
                "domain.discovery_rollback",
                "network discovery revision moved backwards",
            ));
        }
        if self.document.revision == current_revision {
            let digest = blake3::hash(&self.document.signing_payload()?)
                .to_hex()
                .to_string();
            if current_digest != Some(digest.as_str()) {
                return Err(DomainError::new(
                    "domain.discovery_conflict",
                    "network discovery revision conflicts with the accepted record",
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signer, SigningKey};

    use super::*;

    fn signed_record(revision: u64, now: u64, signing_key: &SigningKey) -> NetworkDiscoveryRecord {
        let public_key = PublicKey::from_bytes(signing_key.verifying_key().to_bytes());
        let document = NetworkDiscoveryDocument {
            network_id: NetworkId::from_public_key(public_key.as_str().as_bytes()),
            public_key,
            name: "home".into(),
            purpose: "private mesh".into(),
            visibility: NetworkVisibility::Unlisted,
            join_mode: NetworkJoinMode::ApprovalRequired,
            owner: NodeId::from_public_key(b"owner"),
            revision,
            issued_at_unix_ms: now.saturating_sub(1),
            expires_at_unix_ms: now + 60_000,
            endpoints: vec![NetworkEndpoint {
                transport: DiscoveryTransport::Quic,
                host: "relay.example".into(),
                port: 443,
            }],
        };
        let signature = signing_key.sign(&document.signing_payload().unwrap());
        NetworkDiscoveryRecord {
            document,
            signature: Signature::from_bytes(signature.to_bytes()),
        }
    }

    #[test]
    fn network_identity_is_independent_from_endpoints_and_coordinator() {
        let key = SigningKey::from_bytes(&[4; 32]);
        let first = signed_record(1, 100, &key);
        let mut second = first.clone();
        second.document.endpoints[0].host = "new-coordinator.example".into();
        assert_eq!(first.document.network_id, second.document.network_id);
        first.document.validate_at(100).unwrap();
    }

    #[test]
    fn discovery_rejects_impersonation_and_bad_signatures() {
        let key = SigningKey::from_bytes(&[5; 32]);
        let mut record = signed_record(2, 100, &key);
        record.document.name = "tampered".into();
        assert!(record.validate_at(100).is_err());

        let other = SigningKey::from_bytes(&[6; 32]);
        let mut record = signed_record(2, 100, &key);
        record.document.public_key = PublicKey::from_bytes(other.verifying_key().to_bytes());
        record.document.network_id =
            NetworkId::from_public_key(record.document.public_key.as_str().as_bytes());
        assert!(record.validate_at(100).is_err());
    }

    #[test]
    fn discovery_rejects_rollback_conflict_and_expiry() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let record = signed_record(2, 100, &key);
        assert_eq!(
            record.validate_successor(3, None, 100).unwrap_err().code(),
            "domain.discovery_rollback"
        );
        assert!(record
            .validate_successor(2, Some("different"), 100)
            .is_err());
        let digest = blake3::hash(&record.document.signing_payload().unwrap())
            .to_hex()
            .to_string();
        record.validate_successor(2, Some(&digest), 100).unwrap();
        assert_eq!(
            record.validate_at(60_100).unwrap_err().code(),
            "domain.discovery_expired"
        );
    }

    #[test]
    fn network_record_checks_owner_admin_and_member_sets() {
        let public_key = PublicKey::from_bytes([8; 32]);
        let owner = NodeId::from_public_key(b"owner");
        let admin = NodeId::from_public_key(b"admin");
        let mut network = NetworkRecord {
            network_id: NetworkId::from_public_key(public_key.as_str().as_bytes()),
            public_key,
            name: "mesh".into(),
            purpose: "test".into(),
            icon: "network".into(),
            language: "en".into(),
            visibility: NetworkVisibility::Private,
            join_mode: NetworkJoinMode::InviteOnly,
            owner: owner.clone(),
            administrators: vec![admin.clone()],
            members: vec![owner, admin],
            revision: 1,
            policy_revision: 1,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        network.validate().unwrap();
        network.members.pop();
        assert!(network.validate().is_err());
    }

    fn create_profile() -> NetworkCreateProfile {
        NetworkCreateProfile {
            name: "home-mesh".into(),
            purpose: "Family devices across two homes".into(),
            icon: "home".into(),
            language: "zh-CN".into(),
            visibility: NetworkVisibility::Unlisted,
            join_mode: NetworkJoinMode::InviteOnly,
        }
    }

    #[test]
    fn owner_creation_sets_governance_and_derives_stable_id() {
        let public_key = PublicKey::from_bytes([9; 32]);
        let owner = NodeId::from_public_key(b"owner");
        let network = NetworkRecord::create_owned(create_profile(), public_key, owner.clone(), 100)
            .expect("valid owner-created network");

        assert_eq!(
            network.network_id,
            NetworkId::from_public_key(network.public_key.as_str().as_bytes())
        );
        assert_eq!(network.owner, owner);
        assert_eq!(network.members, vec![owner]);
        assert!(network.administrators.is_empty());
        assert_eq!(network.revision, 1);
        assert_eq!(network.policy_revision, 1);
        assert_eq!(network.icon, "home");
        assert_eq!(network.language, "zh-CN");
    }

    #[test]
    fn rename_and_json_reload_preserve_network_id() {
        let public_key = PublicKey::from_bytes([10; 32]);
        let owner = NodeId::from_public_key(b"owner");
        let mut network =
            NetworkRecord::create_owned(create_profile(), public_key, owner, 100).unwrap();
        let network_id = network.network_id.clone();

        network.rename("renamed-mesh", 200).unwrap();
        assert_eq!(network.network_id, network_id);
        assert_eq!(network.revision, 2);

        let encoded = serde_json::to_string(&network).unwrap();
        let reloaded: NetworkRecord = serde_json::from_str(&encoded).unwrap();
        assert_eq!(reloaded.network_id, network_id);
        assert_eq!(reloaded.name, "renamed-mesh");
        reloaded.validate().unwrap();
    }

    #[test]
    fn create_profile_rejects_unsafe_or_malformed_public_fields() {
        let invalid_name = NetworkCreateProfile {
            name: "home mesh".into(),
            ..create_profile()
        };
        assert!(invalid_name.validate().is_err());

        let empty_purpose = NetworkCreateProfile {
            purpose: "   ".into(),
            ..create_profile()
        };
        assert!(empty_purpose.validate().is_err());

        let invalid_icon = NetworkCreateProfile {
            icon: "Home Icon".into(),
            ..create_profile()
        };
        assert!(invalid_icon.validate().is_err());

        let invalid_language = NetworkCreateProfile {
            language: "zh_CN".into(),
            ..create_profile()
        };
        assert!(invalid_language.validate().is_err());
    }

    #[test]
    fn legacy_network_record_uses_default_public_profile_fields() {
        let value = serde_json::json!({
            "network_id": NetworkId::from_public_key(PublicKey::from_bytes([11; 32]).as_str().as_bytes()),
            "public_key": PublicKey::from_bytes([11; 32]),
            "name": "legacy",
            "purpose": "Legacy network",
            "visibility": "private",
            "join_mode": "invite_only",
            "owner": NodeId::from_public_key(b"owner"),
            "administrators": [],
            "members": [NodeId::from_public_key(b"owner")],
            "revision": 1,
            "policy_revision": 1,
            "created_at_unix_ms": 1,
            "updated_at_unix_ms": 1
        });
        let record: NetworkRecord = serde_json::from_value(value).unwrap();
        assert_eq!(record.icon, DEFAULT_NETWORK_ICON);
        assert_eq!(record.language, DEFAULT_NETWORK_LANGUAGE);
        record.validate().unwrap();
    }
}
