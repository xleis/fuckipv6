use std::fmt;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{DomainError, DomainResult};

const HASH_BYTES: usize = 32;
const PUBLIC_KEY_BYTES: usize = 32;
const SIGNATURE_BYTES: usize = 64;

fn encode(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn decode(value: &str, prefix: &str, expected_bytes: Option<usize>) -> DomainResult<Vec<u8>> {
    let encoded = value
        .strip_prefix(prefix)
        .ok_or_else(|| DomainError::invalid_id("invalid identifier prefix"))?;
    if encoded.is_empty() || encoded.contains(['/', '+', '=']) {
        return Err(DomainError::invalid_id("invalid identifier encoding"));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| DomainError::invalid_id("invalid identifier encoding"))?;
    if let Some(expected) = expected_bytes {
        if bytes.len() != expected {
            return Err(DomainError::invalid_id("invalid identifier length"));
        }
    }
    Ok(bytes)
}

fn derive_id(prefix: &'static str, bytes: &[u8]) -> String {
    format!("{prefix}{}", encode(blake3::hash(bytes).as_bytes()))
}

macro_rules! prefixed_id {
    ($name:ident, $prefix:literal, $pattern:literal, $label:literal) => {
        #[derive(
            Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
        )]
        #[serde(try_from = "String")]
        pub struct $name(#[schemars(regex(pattern = $pattern))] String);

        impl $name {
            pub fn parse(value: impl Into<String>) -> DomainResult<Self> {
                let value = value.into();
                let encoded = value.strip_prefix($prefix).ok_or_else(|| {
                    DomainError::invalid_id(
                        concat!($label, " prefix must be ").to_owned() + $prefix,
                    )
                })?;
                if encoded.len() != 43 || encoded.contains(['/', '+', '=']) {
                    return Err(DomainError::invalid_id(concat!(
                        $label,
                        " has invalid encoding"
                    )));
                }
                decode(&value, $prefix, Some(HASH_BYTES))?;
                Ok(Self(value))
            }

            pub fn from_public_key(public_key: &[u8]) -> Self {
                Self(derive_id($prefix, public_key))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = DomainError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::parse(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

prefixed_id!(NodeId, "node_", "^node_[A-Za-z0-9_-]{43}$", "node id");
prefixed_id!(NetworkId, "net_", "^net_[A-Za-z0-9_-]{43}$", "network id");
prefixed_id!(OperationId, "op_", "^op_[A-Za-z0-9_-]{43}$", "operation id");
prefixed_id!(
    PasskeyCredentialId,
    "pk_",
    "^pk_[A-Za-z0-9_-]{43}$",
    "passkey credential id"
);
prefixed_id!(EventId, "event_", "^event_[A-Za-z0-9_-]{43}$", "event id");
prefixed_id!(AuditId, "audit_", "^audit_[A-Za-z0-9_-]{43}$", "audit id");
prefixed_id!(GroupId, "group_", "^group_[A-Za-z0-9_-]{43}$", "group id");
prefixed_id!(
    TrafficRuleId,
    "rule_",
    "^rule_[A-Za-z0-9_-]{43}$",
    "traffic rule id"
);
prefixed_id!(
    SecurityRuleId,
    "srule_",
    "^srule_[A-Za-z0-9_-]{43}$",
    "security rule id"
);
prefixed_id!(
    FileShareId,
    "share_",
    "^share_[A-Za-z0-9_-]{43}$",
    "file share id"
);
prefixed_id!(FileId, "file_", "^file_[A-Za-z0-9_-]{43}$", "file id");
prefixed_id!(
    TransferId,
    "transfer_",
    "^transfer_[A-Za-z0-9_-]{43}$",
    "transfer id"
);
prefixed_id!(
    IdempotencyKey,
    "idem_",
    "^idem_[A-Za-z0-9_-]{43}$",
    "idempotency key"
);

/// Ed25519 public key bytes encoded as URL-safe base64 without padding.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(try_from = "String")]
pub struct PublicKey(#[schemars(regex(pattern = "^ed25519_[A-Za-z0-9_-]{43}$"))] String);

impl PublicKey {
    pub fn from_bytes(bytes: [u8; PUBLIC_KEY_BYTES]) -> Self {
        Self(format!("ed25519_{}", encode(&bytes)))
    }

    pub fn parse(value: impl Into<String>) -> DomainResult<Self> {
        let value = value.into();
        decode(&value, "ed25519_", Some(PUBLIC_KEY_BYTES))?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn to_bytes(&self) -> DomainResult<[u8; PUBLIC_KEY_BYTES]> {
        let bytes = decode(&self.0, "ed25519_", Some(PUBLIC_KEY_BYTES))?;
        bytes
            .try_into()
            .map_err(|_| DomainError::invalid_id("invalid public key length"))
    }
}

impl TryFrom<String> for PublicKey {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<PublicKey> for String {
    fn from(value: PublicKey) -> Self {
        value.0
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Signature bytes encoded as URL-safe base64 without padding.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(try_from = "String")]
pub struct Signature(#[schemars(regex(pattern = "^sig_[A-Za-z0-9_-]{86}$"))] String);

impl Signature {
    pub fn from_bytes(bytes: [u8; SIGNATURE_BYTES]) -> Self {
        Self(format!("sig_{}", encode(&bytes)))
    }

    pub fn parse(value: impl Into<String>) -> DomainResult<Self> {
        let value = value.into();
        decode(&value, "sig_", Some(SIGNATURE_BYTES))?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn to_bytes(&self) -> DomainResult<[u8; SIGNATURE_BYTES]> {
        let bytes = decode(&self.0, "sig_", Some(SIGNATURE_BYTES))?;
        bytes
            .try_into()
            .map_err(|_| DomainError::invalid_id("invalid signature length"))
    }
}

impl TryFrom<String> for Signature {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<Signature> for String {
    fn from(value: Signature) -> Self {
        value.0
    }
}

impl fmt::Display for Signature {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// SHA-256/Blake3-style content digest represented without exposing secret input.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(try_from = "String")]
pub struct ContentHash(#[schemars(regex(pattern = "^hash_[A-Za-z0-9_-]{43}$"))] String);

impl ContentHash {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(format!("hash_{}", encode(blake3::hash(bytes).as_bytes())))
    }

    pub fn parse(value: impl Into<String>) -> DomainResult<Self> {
        let value = value.into();
        decode(&value, "hash_", Some(HASH_BYTES))?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ContentHash {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<ContentHash> for String {
    fn from(value: ContentHash) -> Self {
        value.0
    }
}

/// Opaque storage locator for secret material. The referenced secret is never
/// embedded in a domain DTO or generated schema.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(try_from = "String")]
pub struct SecretRef(
    #[schemars(regex(
        pattern = "^(?:(?:keychain|keystore):/?|protected_file:)(?:[A-Za-z0-9_-]+/)*[A-Za-z0-9_-]+$"
    ))]
    String,
);

impl SecretRef {
    pub fn parse(value: impl Into<String>) -> DomainResult<Self> {
        let value = value.into();
        let (scheme, locator) = value.split_once(':').ok_or_else(|| {
            DomainError::invalid_secret_ref("secret reference scheme is required")
        })?;
        if !matches!(scheme, "keychain" | "keystore" | "protected_file") {
            return Err(DomainError::invalid_secret_ref(
                "secret reference scheme is not supported",
            ));
        }
        let path = if matches!(scheme, "keychain" | "keystore") {
            locator.strip_prefix('/').unwrap_or(locator)
        } else {
            locator
        };
        let safe_relative_path = !(scheme == "protected_file" && locator.starts_with('/'))
            && path
                .split('/')
                .all(|segment| !segment.is_empty() && segment != "." && segment != "..");
        if locator.is_empty()
            || locator.len() > 160
            || !safe_relative_path
            || !locator.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'/' | b'-')
            })
        {
            return Err(DomainError::invalid_secret_ref(
                "secret reference locator is invalid",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SecretRef {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<SecretRef> for String {
    fn from(value: SecretRef) -> Self {
        value.0
    }
}

/// Opaque reference to an authenticated session record.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(try_from = "String")]
pub struct SessionRef(#[schemars(regex(pattern = "^session_[A-Za-z0-9_-]{43}$"))] String);

impl SessionRef {
    pub fn new(random_bytes: [u8; HASH_BYTES]) -> Self {
        Self(format!("session_{}", encode(&random_bytes)))
    }

    pub fn parse(value: impl Into<String>) -> DomainResult<Self> {
        let value = value.into();
        decode(&value, "session_", Some(HASH_BYTES))?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SessionRef {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<SessionRef> for String {
    fn from(value: SessionRef) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_ids_are_derived_from_content_and_round_trip() {
        let node = NodeId::from_public_key(b"node-key");
        let network = NetworkId::from_public_key(b"network-key");
        assert_eq!(node, NodeId::parse(node.to_string()).unwrap());
        assert_eq!(network, NetworkId::parse(network.to_string()).unwrap());
        assert_ne!(node.as_str(), network.as_str());

        let encoded = serde_json::to_string(&node).unwrap();
        assert_eq!(serde_json::from_str::<NodeId>(&encoded).unwrap(), node);
    }

    #[test]
    fn malformed_prefixed_ids_fail_closed_on_deserialization() {
        for value in [
            "node_",
            "node_deadbeef",
            "network_deadbeef",
            "node_////////////////////////",
        ] {
            assert!(serde_json::from_value::<NodeId>(serde_json::json!(value)).is_err());
        }
        assert!(NodeId::parse("net_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").is_err());
    }

    #[test]
    fn public_keys_and_signatures_validate_length() {
        let key = PublicKey::from_bytes([7; PUBLIC_KEY_BYTES]);
        assert_eq!(key.to_bytes().unwrap(), [7; PUBLIC_KEY_BYTES]);
        assert!(PublicKey::parse("ed25519_too-short").is_err());
        let signature = Signature::from_bytes([9; SIGNATURE_BYTES]);
        assert_eq!(Signature::parse(signature.to_string()).unwrap(), signature);
    }

    #[test]
    fn secrets_are_only_addressable_by_opaque_storage_locator() {
        assert!(SecretRef::parse("keychain:relay/node-owner").is_ok());
        assert!(SecretRef::parse("keystore:/owner/recovery").is_ok());
        assert!(SecretRef::parse("plaintext:super-secret").is_err());
        assert!(SecretRef::parse("protected_file:").is_err());
        assert!(SecretRef::parse("protected_file:../outside").is_err());
        assert!(SecretRef::parse("protected_file:/absolute/path").is_err());
        assert!(SecretRef::parse("keystore:owner//recovery").is_err());
    }
}
