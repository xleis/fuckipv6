//! Stable, transport-neutral identifiers used by node and network contracts.
//!
//! Identifiers serialize as lowercase hexadecimal strings. Their byte length is
//! part of the wire contract; accepting variable-length aliases would make
//! signatures and database keys ambiguous.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// A rejected identifier encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdError {
    pub expected_bytes: usize,
    pub actual_bytes: usize,
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid identifier length: expected {} bytes, got {}",
            self.expected_bytes, self.actual_bytes
        )
    }
}

impl std::error::Error for IdError {}

fn decode_hex(value: &str, expected_bytes: usize) -> Result<Vec<u8>, IdError> {
    if value.len() != expected_bytes.saturating_mul(2) {
        return Err(IdError {
            expected_bytes,
            actual_bytes: value.len() / 2,
        });
    }

    let mut decoded = Vec::with_capacity(expected_bytes);
    let raw = value.as_bytes();
    for pair in raw.as_chunks::<2>().0 {
        let high = hex_nibble(pair[0]).ok_or(IdError {
            expected_bytes,
            actual_bytes: expected_bytes,
        })?;
        let low = hex_nibble(pair[1]).ok_or(IdError {
            expected_bytes,
            actual_bytes: expected_bytes,
        })?;
        decoded.push((high << 4) | low);
    }
    Ok(decoded)
}

fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

macro_rules! define_id {
    ($name:ident, $bytes:literal) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name([u8; $bytes]);

        impl $name {
            pub const BYTE_LEN: usize = $bytes;

            pub const fn from_bytes(bytes: [u8; $bytes]) -> Self {
                Self(bytes)
            }

            pub fn random() -> Result<Self, IdError> {
                let mut bytes = [0_u8; $bytes];
                getrandom::fill(&mut bytes).map_err(|_| IdError {
                    expected_bytes: $bytes,
                    actual_bytes: 0,
                })?;
                Ok(Self(bytes))
            }

            pub const fn as_bytes(&self) -> &[u8; $bytes] {
                &self.0
            }

            pub fn to_hex(self) -> String {
                encode_hex(&self.0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.to_hex())
            }
        }

        impl FromStr for $name {
            type Err = IdError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                let decoded = decode_hex(value, $bytes)?;
                let mut bytes = [0_u8; $bytes];
                bytes.copy_from_slice(&decoded);
                Ok(Self(bytes))
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str(&self.to_hex())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                value.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

define_id!(NodeId, 32);
define_id!(NetworkId, 32);
define_id!(OwnerId, 16);
define_id!(CredentialId, 16);
define_id!(GroupId, 16);
define_id!(PolicyId, 16);
define_id!(OperationId, 16);
define_id!(EventId, 16);
define_id!(AuditId, 16);
define_id!(FileShareId, 16);

impl NodeId {
    /// Derive the stable node identity from a public verification key.
    ///
    /// Private key material must never be passed to this function.
    pub fn from_public_key(algorithm: &str, public_key: &[u8]) -> Self {
        Self::from_bytes(derive_hash_id(
            b"fuckipv6.node-id.v1",
            algorithm,
            public_key,
        ))
    }
}

impl NetworkId {
    /// Derive the signed-network identity independently from any Coordinator
    /// address. Changing discovery endpoints must not change this value.
    pub fn from_public_key(algorithm: &str, public_key: &[u8]) -> Self {
        Self::from_bytes(derive_hash_id(
            b"fuckipv6.network-id.v1",
            algorithm,
            public_key,
        ))
    }
}

fn derive_hash_id(domain: &[u8], algorithm: &str, public_key: &[u8]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hasher.update(&[0]);
    hasher.update(algorithm.as_bytes());
    hasher.update(&[0]);
    hasher.update(public_key);
    *hasher.finalize().as_bytes()
}

impl From<NetworkId> for Vec<u8> {
    fn from(value: NetworkId) -> Self {
        value.0.to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_use_canonical_fixed_length_hex() {
        let id = NodeId::from_bytes([0xab; 32]);
        let json = serde_json::to_string(&id).expect("serialize");
        assert_eq!(json, format!("\"{}\"", "ab".repeat(32)));
        assert_eq!(
            serde_json::from_str::<NodeId>(&json).expect("deserialize"),
            id
        );

        let uppercase = "AB".repeat(32);
        assert_eq!(uppercase.parse::<NodeId>().expect("parse"), id);
        assert!("ab".repeat(31).parse::<NodeId>().is_err());
        assert!("zz".repeat(32).parse::<NodeId>().is_err());
    }

    #[test]
    fn public_key_derivation_is_stable_and_domain_separated() {
        let key = [7_u8; 32];
        let node_a = NodeId::from_public_key("ed25519", &key);
        let node_b = NodeId::from_public_key("ed25519", &key);
        let network = NetworkId::from_public_key("ed25519", &key);
        assert_eq!(node_a, node_b);
        assert_ne!(node_a.as_bytes(), network.as_bytes());
    }
}
