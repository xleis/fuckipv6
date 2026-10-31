use std::collections::BTreeSet;

use ed25519_dalek::{Signature as DalekSignature, Verifier, VerifyingKey};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ids::{NetworkId, NodeId, PublicKey, Signature};
use super::{DomainError, DomainResult};

/// How much of a network's public profile is discoverable without membership.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkVisibility {
    Public,
    Unlisted,
    Private,
}

/// Admission policy selected by the network owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkJoinMode {
    /// Requests are accepted immediately.
    Open,
    /// Requests enter `pending_approval`.
    ApprovalRequired,
    /// The node must first consume a signed, single-use invitation.
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
        if self.name.is_empty() || self.name.len() > 80 {
            return Err(DomainError::invalid_value("network name is invalid"));
        }
        if self.purpose.is_empty() || self.purpose.len() > 512 {
            return Err(DomainError::invalid_value("network purpose is invalid"));
        }
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
}
