//! Network identity and signed discovery contracts.

use super::foundation::{require_non_empty, ContractIssue, ContractResult, UnixMillis};
use super::identity::{Ed25519Signature, PublicKey};
use super::ids::{NetworkId, NodeId};
use ed25519_dalek::{Signer, Verifier};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

pub const NETWORK_CONTRACT_VERSION: u16 = 1;
pub const DISCOVERY_CLOCK_SKEW_SECS: u64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkPurpose {
    Personal,
    Family,
    Team,
    Community,
    PublicService,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkVisibility {
    Private,
    InviteOnly,
    Discoverable,
    Public,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinMode {
    Closed,
    InviteOnly,
    ApprovalRequired,
    Open,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicNetworkProfile {
    pub display_name: String,
    pub description: String,
    pub region: Option<String>,
}

impl PublicNetworkProfile {
    fn validate(&self, issues: &mut Vec<ContractIssue>) {
        require_non_empty(
            &self.display_name,
            "public_profile.display_name",
            128,
            issues,
        );
        require_non_empty(
            &self.description,
            "public_profile.description",
            4096,
            issues,
        );
        if let Some(region) = self.region.as_deref() {
            require_non_empty(region, "public_profile.region", 128, issues);
        }
    }
}

/// A network is identified by a long-lived public key, not by the Coordinator
/// that currently serves it. This makes Coordinator migration non-identifying.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Network {
    pub schema_version: u16,
    pub id: NetworkId,
    pub public_key: PublicKey,
    pub name: String,
    pub purpose: NetworkPurpose,
    pub visibility: NetworkVisibility,
    pub join_mode: JoinMode,
    pub public_profile: PublicNetworkProfile,
    pub owner: NodeId,
    pub administrators: BTreeSet<NodeId>,
    pub members: BTreeSet<NodeId>,
    pub version: u64,
    pub policy_revision: u64,
}

impl Network {
    pub fn validate(&self) -> ContractResult {
        let mut issues = Vec::new();
        if self.schema_version != NETWORK_CONTRACT_VERSION {
            issues.push(ContractIssue::new(
                "unsupported_schema_version",
                "schema_version",
                format!("network schema version must be {NETWORK_CONTRACT_VERSION}"),
            ));
        }
        self.public_key.validate("public_key", &mut issues);
        // NodeId and NetworkId share a byte width but intentionally use
        // different derivation domains. Always recompute with the network
        // domain so a node identity can never be accepted as a network identity.
        let derived = NetworkId::from_public_key(
            self.public_key.algorithm.as_str(),
            self.public_key.key.as_slice(),
        );
        if derived != self.id {
            issues.push(ContractIssue::new(
                "network_id_mismatch",
                "id",
                "network id must be derived from the signed network public key",
            ));
        }
        require_non_empty(&self.name, "name", 64, &mut issues);
        self.public_profile.validate(&mut issues);
        if self.visibility == NetworkVisibility::Public && self.join_mode == JoinMode::Closed {
            issues.push(ContractIssue::new(
                "public_network_closed",
                "join_mode",
                "a public network cannot be closed",
            ));
        }
        if !self.members.contains(&self.owner) {
            issues.push(ContractIssue::new(
                "owner_not_member",
                "members",
                "network owner must also be a member",
            ));
        }
        if self
            .administrators
            .iter()
            .any(|administrator| !self.members.contains(administrator))
        {
            issues.push(ContractIssue::new(
                "administrator_not_member",
                "administrators",
                "every administrator must be a network member",
            ));
        }
        if self.version == 0 {
            issues.push(ContractIssue::new(
                "invalid_network_version",
                "version",
                "network version must be greater than zero",
            ));
        }
        if self.policy_revision == 0 {
            issues.push(ContractIssue::new(
                "invalid_policy_revision",
                "policy_revision",
                "policy revision must be greater than zero",
            ));
        }
        finish(issues)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinatorEndpoint {
    pub url: String,
}

impl CoordinatorEndpoint {
    fn validate(&self, field: &str, issues: &mut Vec<ContractIssue>) {
        require_non_empty(&self.url, &format!("{field}.url"), 2048, issues);
        let lower = self.url.to_ascii_lowercase();
        let valid_scheme = ["https://", "http://", "wss://", "ws://"]
            .iter()
            .any(|scheme| lower.starts_with(scheme));
        if !valid_scheme {
            issues.push(ContractIssue::new(
                "invalid_coordinator_scheme",
                format!("{field}.url"),
                "Coordinator URL must use http(s) or ws(s)",
            ));
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DiscoveryPayload {
    domain: String,
    network_id: NetworkId,
    coordinator: CoordinatorEndpoint,
    version: u64,
    issued_at: UnixMillis,
    expires_at: UnixMillis,
}

impl DiscoveryPayload {
    fn new(
        network_id: NetworkId,
        coordinator: CoordinatorEndpoint,
        version: u64,
        issued_at: UnixMillis,
        expires_at: UnixMillis,
    ) -> Self {
        Self {
            domain: "fuckipv6.network-discovery.v1".into(),
            network_id,
            coordinator,
            version,
            issued_at,
            expires_at,
        }
    }
}

/// A signed, monotonically versioned Coordinator discovery record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedDiscoveryRecord {
    pub network_id: NetworkId,
    pub coordinator: CoordinatorEndpoint,
    pub version: u64,
    pub issued_at: UnixMillis,
    pub expires_at: UnixMillis,
    pub signature: Ed25519Signature,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryError {
    Contract(Vec<ContractIssue>),
    InvalidSignature,
    Expired,
    IssuedInFuture,
    ReplayedOrRolledBack {
        last_seen_version: u64,
        presented_version: u64,
    },
    Encoding(String),
}

impl fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(issues) => write!(f, "invalid discovery contract: {issues:?}"),
            Self::InvalidSignature => f.write_str("discovery signature verification failed"),
            Self::Expired => f.write_str("discovery record has expired"),
            Self::IssuedInFuture => f.write_str("discovery record was issued in the future"),
            Self::ReplayedOrRolledBack {
                last_seen_version,
                presented_version,
            } => write!(
                f,
                "discovery version {presented_version} does not advance last seen version {last_seen_version}"
            ),
            Self::Encoding(message) => write!(f, "cannot encode discovery payload: {message}"),
        }
    }
}

impl std::error::Error for DiscoveryError {}

impl SignedDiscoveryRecord {
    pub fn sign(
        network: &Network,
        signing_key: &ed25519_dalek::SigningKey,
        coordinator: CoordinatorEndpoint,
        version: u64,
        issued_at: UnixMillis,
        expires_at: UnixMillis,
    ) -> Result<Self, DiscoveryError> {
        let public_key = network.public_key.ed25519_bytes().ok_or_else(|| {
            DiscoveryError::Contract(vec![ContractIssue::new(
                "unsupported_signature_algorithm",
                "network.public_key.algorithm",
                "network discovery currently requires Ed25519",
            )])
        })?;
        if signing_key.verifying_key().to_bytes() != *public_key {
            return Err(DiscoveryError::InvalidSignature);
        }
        if !issued_at.is_before(expires_at) {
            return Err(DiscoveryError::Contract(vec![ContractIssue::new(
                "invalid_discovery_expiry",
                "expires_at",
                "discovery expiry must be after issue time",
            )]));
        }
        if version == 0 {
            return Err(DiscoveryError::Contract(vec![ContractIssue::new(
                "invalid_discovery_version",
                "version",
                "discovery version must be greater than zero",
            )]));
        }

        let payload = DiscoveryPayload::new(
            network.id,
            coordinator.clone(),
            version,
            issued_at,
            expires_at,
        );
        let bytes = signing_bytes(&payload)?;
        let signature = signing_key.sign(&bytes);
        Ok(Self {
            network_id: network.id,
            coordinator,
            version,
            issued_at,
            expires_at,
            signature: Ed25519Signature::from_bytes(signature.to_bytes()),
        })
    }

    /// Verify signature, freshness, and monotonic version before a node applies
    /// the record. `last_seen_version` must come from durable local state.
    pub fn verify(
        &self,
        network: &Network,
        now: UnixMillis,
        last_seen_version: u64,
    ) -> Result<(), DiscoveryError> {
        let mut issues = Vec::new();
        if self.network_id != network.id {
            issues.push(ContractIssue::new(
                "network_id_mismatch",
                "network_id",
                "discovery record belongs to a different network",
            ));
        }
        self.coordinator.validate("coordinator", &mut issues);
        if self.version == 0 {
            issues.push(ContractIssue::new(
                "invalid_discovery_version",
                "version",
                "discovery version must be greater than zero",
            ));
        }
        if !self.issued_at.is_before(self.expires_at) {
            issues.push(ContractIssue::new(
                "invalid_discovery_expiry",
                "expires_at",
                "discovery expiry must be after issue time",
            ));
        }
        if !issues.is_empty() {
            return Err(DiscoveryError::Contract(issues));
        }
        if last_seen_version != 0 && self.version <= last_seen_version {
            return Err(DiscoveryError::ReplayedOrRolledBack {
                last_seen_version,
                presented_version: self.version,
            });
        }
        if !now.is_before(self.expires_at) {
            return Err(DiscoveryError::Expired);
        }
        let skew = UnixMillis(DISCOVERY_CLOCK_SKEW_SECS.saturating_mul(1_000));
        if now.0.saturating_add(skew.0) < self.issued_at.0 {
            return Err(DiscoveryError::IssuedInFuture);
        }

        let public_key = network.public_key.ed25519_bytes().ok_or_else(|| {
            DiscoveryError::Contract(vec![ContractIssue::new(
                "unsupported_signature_algorithm",
                "network.public_key.algorithm",
                "network discovery currently requires Ed25519",
            )])
        })?;
        let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(public_key)
            .map_err(|_| DiscoveryError::InvalidSignature)?;
        let payload = DiscoveryPayload::new(
            self.network_id,
            self.coordinator.clone(),
            self.version,
            self.issued_at,
            self.expires_at,
        );
        let bytes = signing_bytes(&payload)?;
        verifying_key
            .verify(&bytes, &self.signature.to_dalek())
            .map_err(|_| DiscoveryError::InvalidSignature)
    }
}

fn signing_bytes(payload: &DiscoveryPayload) -> Result<Vec<u8>, DiscoveryError> {
    postcard::to_allocvec(payload).map_err(|error| DiscoveryError::Encoding(error.to_string()))
}

fn finish(issues: Vec<ContractIssue>) -> ContractResult {
    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network_fixture() -> (Network, ed25519_dalek::SigningKey) {
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        let public_key = PublicKey::ed25519(signing_key.verifying_key().to_bytes());
        let owner = NodeId::from_public_key("ed25519", &[8; 32]);
        let id =
            NetworkId::from_public_key(public_key.algorithm.as_str(), public_key.key.as_slice());
        let network = Network {
            schema_version: NETWORK_CONTRACT_VERSION,
            id,
            public_key,
            name: "home".into(),
            purpose: NetworkPurpose::Family,
            visibility: NetworkVisibility::Private,
            join_mode: JoinMode::InviteOnly,
            public_profile: PublicNetworkProfile {
                display_name: "Home".into(),
                description: "Home network".into(),
                region: None,
            },
            owner,
            administrators: BTreeSet::from([owner]),
            members: BTreeSet::from([owner]),
            version: 1,
            policy_revision: 1,
        };
        (network, signing_key)
    }

    fn record(
        network: &Network,
        key: &ed25519_dalek::SigningKey,
        version: u64,
    ) -> SignedDiscoveryRecord {
        SignedDiscoveryRecord::sign(
            network,
            key,
            CoordinatorEndpoint {
                url: "wss://relay.example/coordinator".into(),
            },
            version,
            UnixMillis(1_000),
            UnixMillis(10_000),
        )
        .expect("sign discovery record")
    }

    #[test]
    fn network_id_is_independent_of_coordinator_address() {
        let (network, key) = network_fixture();
        network.validate().expect("valid network");
        let first = record(&network, &key, 1);
        let second = SignedDiscoveryRecord::sign(
            &network,
            &key,
            CoordinatorEndpoint {
                url: "wss://another.example/coordinator".into(),
            },
            2,
            UnixMillis(1_000),
            UnixMillis(10_000),
        )
        .expect("sign moved discovery record");
        assert_eq!(first.network_id, second.network_id);
        assert_ne!(first.coordinator, second.coordinator);
    }

    #[test]
    fn rejects_impersonated_signature() {
        let (network, key) = network_fixture();
        let _attacker = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        assert!(SignedDiscoveryRecord::sign(
            &network,
            &_attacker,
            CoordinatorEndpoint {
                url: "wss://relay.example/coordinator".into(),
            },
            1,
            UnixMillis(1_000),
            UnixMillis(10_000),
        )
        .is_err());

        let mut forged = record(&network, &key, 1);
        let payload = DiscoveryPayload::new(
            network.id,
            forged.coordinator.clone(),
            forged.version,
            forged.issued_at,
            forged.expires_at,
        );
        let signature = _attacker.sign(&signing_bytes(&payload).expect("payload"));
        forged.signature = Ed25519Signature::from_bytes(signature.to_bytes());
        assert_eq!(
            forged.verify(&network, UnixMillis(2_000), 0),
            Err(DiscoveryError::InvalidSignature)
        );
    }

    #[test]
    fn rejects_expired_and_rolled_back_records() {
        let (network, key) = network_fixture();
        let current = record(&network, &key, 2);
        current
            .verify(&network, UnixMillis(2_000), 1)
            .expect("fresh");
        assert_eq!(
            current.verify(&network, UnixMillis(10_000), 1),
            Err(DiscoveryError::Expired)
        );
        assert_eq!(
            current.verify(&network, UnixMillis(2_000), 2),
            Err(DiscoveryError::ReplayedOrRolledBack {
                last_seen_version: 2,
                presented_version: 2,
            })
        );
    }

    #[test]
    fn rejects_future_issued_record_beyond_clock_skew() {
        let (network, key) = network_fixture();
        let future = SignedDiscoveryRecord::sign(
            &network,
            &key,
            CoordinatorEndpoint {
                url: "wss://relay.example/coordinator".into(),
            },
            1,
            UnixMillis(400_000),
            UnixMillis(500_000),
        )
        .expect("sign");
        assert_eq!(
            future.verify(&network, UnixMillis(2_000), 0),
            Err(DiscoveryError::IssuedInFuture)
        );
    }
}
