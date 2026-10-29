//! Canonical handshake transcript construction for protocol v2.
//!
//! Every transcript value is encoded as a two-byte big-endian length followed
//! by the exact bytes. Initiator and responder roles are inserted in a fixed
//! order, so both endpoints hash the same canonical byte stream. A length-
//! prefixed protocol domain separates this use of SHA-512 from signatures,
//! key derivation, cookies, and future transcript formats.

use crate::PROTOCOL_VERSION;
use sha2::{Digest, Sha512};
use thiserror::Error;

/// Protocol domain for the canonical v2 handshake transcript.
pub const TRANSCRIPT_DOMAIN: &[u8] = b"fuckipv6-v2 handshake transcript";
/// Size of every length prefix in a canonical transcript.
pub const TRANSCRIPT_LENGTH_PREFIX_LEN: usize = 2;
/// Output size of the SHA-512 transcript hash.
pub const TRANSCRIPT_HASH_LEN: usize = 64;
/// Fixed IdentityId size from the v2 identity construction.
pub const IDENTITY_ID_LEN: usize = 32;
/// Fixed endpoint-context hash size.
pub const ENDPOINT_CONTEXT_HASH_LEN: usize = 32;
/// Maximum encoded length of one transcript field.
pub const MAX_TRANSCRIPT_FIELD_LEN: usize = u16::MAX as usize;
/// Defensive allocation limit for one encoded transcript.
pub const MAX_CANONICAL_TRANSCRIPT_LEN: usize = 512 * 1024;
/// AES-256-GCM data-suite identifier.
pub const SUITE_AES_256_GCM: u16 = 0x0201;
/// ChaCha20-Poly1305 data-suite identifier.
pub const SUITE_CHACHA20_POLY1305: u16 = 0x0202;

/// A 256-bit identity digest used in the handshake transcript.
pub type IdentityId = [u8; IDENTITY_ID_LEN];
/// A SHA-512 transcript digest.
pub type TranscriptHash = [u8; TRANSCRIPT_HASH_LEN];

/// A protocol role label included in canonical transcript order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeRole {
    Initiator,
    Responder,
}

impl HandshakeRole {
    const fn wire_value(self) -> u8 {
        match self {
            Self::Initiator => 1,
            Self::Responder => 2,
        }
    }
}

/// Public inputs bound by one v2 handshake transcript.
///
/// The initiator and responder fields are always encoded in protocol order,
/// regardless of which endpoint performs the computation. Callers must not
/// substitute their local role for these labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandshakeTranscript<'a> {
    /// Negotiated data suite. Only fully specified v2 suites are accepted.
    pub suite_id: u16,
    /// Stable initiator identity digest.
    pub initiator_identity: IdentityId,
    /// Stable responder identity digest.
    pub responder_identity: IdentityId,
    /// Initiator's fresh ephemeral public key or X-Wing encapsulation key.
    pub initiator_ephemeral_public: &'a [u8],
    /// Responder's fresh ephemeral public key when the suite uses one.
    pub responder_ephemeral_public: &'a [u8],
    /// Session identifier bound to this handshake.
    pub session_id: [u8; crate::SESSION_ID_LEN],
    /// Data epoch established by the handshake.
    pub epoch: u16,
    /// Hash of network ID, both virtual IPs, and the expected endpoint set.
    pub endpoint_context_hash: [u8; ENDPOINT_CONTEXT_HASH_LEN],
}

impl HandshakeTranscript<'_> {
    /// Encode the full canonical transcript byte stream.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, TranscriptError> {
        self.validate()?;

        let mut out = Vec::with_capacity(256);
        push_field(&mut out, "domain", TRANSCRIPT_DOMAIN)?;
        push_field(&mut out, "version", &[PROTOCOL_VERSION])?;
        push_field(&mut out, "suite_id", &self.suite_id.to_be_bytes())?;

        push_field(
            &mut out,
            "initiator_role",
            &[HandshakeRole::Initiator.wire_value()],
        )?;
        push_field(&mut out, "initiator_identity", &self.initiator_identity)?;
        push_field(
            &mut out,
            "initiator_ephemeral_public",
            self.initiator_ephemeral_public,
        )?;

        push_field(
            &mut out,
            "responder_role",
            &[HandshakeRole::Responder.wire_value()],
        )?;
        push_field(&mut out, "responder_identity", &self.responder_identity)?;
        push_field(
            &mut out,
            "responder_ephemeral_public",
            self.responder_ephemeral_public,
        )?;

        push_field(&mut out, "session_id", &self.session_id)?;
        push_field(&mut out, "epoch", &self.epoch.to_be_bytes())?;
        push_field(
            &mut out,
            "endpoint_context_hash",
            &self.endpoint_context_hash,
        )?;
        Ok(out)
    }

    /// Hash the canonical transcript with SHA-512.
    pub fn hash(&self) -> Result<TranscriptHash, TranscriptError> {
        let canonical = self.canonical_bytes()?;
        let digest = Sha512::digest(canonical);
        let mut hash = [0; TRANSCRIPT_HASH_LEN];
        hash.copy_from_slice(&digest);
        Ok(hash)
    }

    fn validate(&self) -> Result<(), TranscriptError> {
        if !matches!(self.suite_id, SUITE_AES_256_GCM | SUITE_CHACHA20_POLY1305) {
            return Err(TranscriptError::UnsupportedSuite(self.suite_id));
        }
        if self.initiator_identity == [0; IDENTITY_ID_LEN] {
            return Err(TranscriptError::InvalidIdentity("initiator"));
        }
        if self.responder_identity == [0; IDENTITY_ID_LEN] {
            return Err(TranscriptError::InvalidIdentity("responder"));
        }
        if self.initiator_identity == self.responder_identity {
            return Err(TranscriptError::DuplicateIdentity);
        }
        if self.initiator_ephemeral_public.is_empty() {
            return Err(TranscriptError::EmptyEphemeralPublic("initiator"));
        }
        if self.responder_ephemeral_public.is_empty() {
            return Err(TranscriptError::EmptyEphemeralPublic("responder"));
        }
        if self.endpoint_context_hash == [0; ENDPOINT_CONTEXT_HASH_LEN] {
            return Err(TranscriptError::InvalidEndpointContext);
        }
        Ok(())
    }
}

/// Errors returned while validating or encoding a transcript.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptError {
    #[error("unsupported v2 handshake suite {0:#06x}")]
    UnsupportedSuite(u16),
    #[error("{0} identity must not be all zero")]
    InvalidIdentity(&'static str),
    #[error("initiator and responder identities must differ")]
    DuplicateIdentity,
    #[error("{0} ephemeral public key must not be empty")]
    EmptyEphemeralPublic(&'static str),
    #[error("endpoint context hash must not be all zero")]
    InvalidEndpointContext,
    #[error("transcript field {field} is {len} bytes; maximum is {max}")]
    FieldTooLong {
        field: &'static str,
        len: usize,
        max: usize,
    },
    #[error("canonical transcript exceeds the {max}-byte allocation limit")]
    TranscriptTooLong { max: usize },
}

fn push_field(out: &mut Vec<u8>, field: &'static str, value: &[u8]) -> Result<(), TranscriptError> {
    if value.len() > MAX_TRANSCRIPT_FIELD_LEN {
        return Err(TranscriptError::FieldTooLong {
            field,
            len: value.len(),
            max: MAX_TRANSCRIPT_FIELD_LEN,
        });
    }
    let encoded_len = TRANSCRIPT_LENGTH_PREFIX_LEN + value.len();
    if out
        .len()
        .checked_add(encoded_len)
        .is_none_or(|len| len > MAX_CANONICAL_TRANSCRIPT_LEN)
    {
        return Err(TranscriptError::TranscriptTooLong {
            max: MAX_CANONICAL_TRANSCRIPT_LEN,
        });
    }
    out.extend_from_slice(&(value.len() as u16).to_be_bytes());
    out.extend_from_slice(value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPK_I: [u8; 32] = [0xaa; 32];
    const EPK_R: [u8; 32] = [0x33; 32];
    const OTHER_EPK_I: [u8; 32] = [0xab; 32];
    const OTHER_EPK_R: [u8; 32] = [0x34; 32];
    const CI: IdentityId = [0x11; IDENTITY_ID_LEN];
    const CR: IdentityId = [0x22; IDENTITY_ID_LEN];
    const SID: [u8; crate::SESSION_ID_LEN] = [0x44; crate::SESSION_ID_LEN];
    const ENDPOINT: [u8; ENDPOINT_CONTEXT_HASH_LEN] = [0x55; ENDPOINT_CONTEXT_HASH_LEN];

    fn fixture() -> HandshakeTranscript<'static> {
        HandshakeTranscript {
            suite_id: SUITE_CHACHA20_POLY1305,
            initiator_identity: CI,
            responder_identity: CR,
            initiator_ephemeral_public: &EPK_I,
            responder_ephemeral_public: &EPK_R,
            session_id: SID,
            epoch: 1,
            endpoint_context_hash: ENDPOINT,
        }
    }

    #[test]
    fn canonical_transcript_matches_the_fixed_vector() {
        let transcript = fixture();
        let canonical = transcript.canonical_bytes().expect("canonical transcript");
        assert_eq!(canonical.len(), 239);
        assert_eq!(&canonical[..2], &[0, 32]);
        assert_eq!(
            transcript.hash().expect("transcript hash"),
            [
                0xb0, 0xbf, 0xa0, 0x26, 0x0a, 0x00, 0x58, 0x86, 0xca, 0xc7, 0x9c, 0xe6, 0x45, 0x63,
                0x94, 0x6e, 0xdd, 0x2d, 0x6b, 0x90, 0x54, 0x81, 0x51, 0x38, 0x1d, 0x64, 0x5a, 0x4e,
                0x11, 0x67, 0xc1, 0x39, 0x0a, 0x3e, 0x54, 0x56, 0xde, 0xf5, 0x2a, 0x3e, 0xbc, 0x76,
                0xef, 0x7c, 0x13, 0x27, 0x1d, 0x1a, 0xa4, 0xae, 0x5c, 0x21, 0xcd, 0x4e, 0x4a, 0x04,
                0x98, 0xbe, 0x87, 0x95, 0x63, 0xe8, 0xad, 0x07,
            ]
        );
    }

    #[test]
    fn both_endpoints_construct_the_same_transcript() {
        let initiator_view = fixture();
        let responder_view = HandshakeTranscript {
            initiator_identity: CI,
            responder_identity: CR,
            initiator_ephemeral_public: &EPK_I,
            responder_ephemeral_public: &EPK_R,
            ..initiator_view
        };
        assert_eq!(
            initiator_view.canonical_bytes().expect("initiator bytes"),
            responder_view.canonical_bytes().expect("responder bytes")
        );
        assert_eq!(
            initiator_view.hash().expect("initiator hash"),
            responder_view.hash().expect("responder hash")
        );
    }

    #[test]
    fn every_bound_field_changes_the_hash() {
        let base = fixture();
        let base_hash = base.hash().expect("base hash");
        let variants = [
            HandshakeTranscript {
                suite_id: SUITE_AES_256_GCM,
                ..base
            },
            HandshakeTranscript {
                initiator_identity: [0x12; IDENTITY_ID_LEN],
                ..base
            },
            HandshakeTranscript {
                responder_identity: [0x23; IDENTITY_ID_LEN],
                ..base
            },
            HandshakeTranscript {
                initiator_ephemeral_public: &OTHER_EPK_I,
                ..base
            },
            HandshakeTranscript {
                responder_ephemeral_public: &OTHER_EPK_R,
                ..base
            },
            HandshakeTranscript {
                session_id: [0x45; crate::SESSION_ID_LEN],
                ..base
            },
            HandshakeTranscript { epoch: 2, ..base },
            HandshakeTranscript {
                endpoint_context_hash: [0x56; ENDPOINT_CONTEXT_HASH_LEN],
                ..base
            },
        ];
        for variant in variants {
            assert_ne!(variant.hash().expect("variant hash"), base_hash);
        }
    }

    #[test]
    fn length_prefixes_prevent_field_boundary_ambiguity() {
        let split_a = HandshakeTranscript {
            initiator_ephemeral_public: b"ab",
            responder_ephemeral_public: b"c",
            ..fixture()
        };
        let split_b = HandshakeTranscript {
            initiator_ephemeral_public: b"a",
            responder_ephemeral_public: b"bc",
            ..fixture()
        };
        assert_ne!(
            split_a.canonical_bytes().expect("split a"),
            split_b.canonical_bytes().expect("split b")
        );
        assert_ne!(
            split_a.hash().expect("hash a"),
            split_b.hash().expect("hash b")
        );
    }

    #[test]
    fn invalid_fields_are_rejected_before_hashing() {
        assert_eq!(
            HandshakeTranscript {
                suite_id: 0x9999,
                ..fixture()
            }
            .hash(),
            Err(TranscriptError::UnsupportedSuite(0x9999))
        );
        assert_eq!(
            HandshakeTranscript {
                initiator_identity: [0; IDENTITY_ID_LEN],
                ..fixture()
            }
            .hash(),
            Err(TranscriptError::InvalidIdentity("initiator"))
        );
        assert_eq!(
            HandshakeTranscript {
                responder_identity: CI,
                ..fixture()
            }
            .hash(),
            Err(TranscriptError::DuplicateIdentity)
        );
        assert_eq!(
            HandshakeTranscript {
                initiator_ephemeral_public: &[],
                ..fixture()
            }
            .hash(),
            Err(TranscriptError::EmptyEphemeralPublic("initiator"))
        );
        assert_eq!(
            HandshakeTranscript {
                endpoint_context_hash: [0; ENDPOINT_CONTEXT_HASH_LEN],
                ..fixture()
            }
            .hash(),
            Err(TranscriptError::InvalidEndpointContext)
        );

        let oversized = vec![0x42; MAX_TRANSCRIPT_FIELD_LEN + 1];
        assert_eq!(
            HandshakeTranscript {
                initiator_ephemeral_public: &oversized,
                ..fixture()
            }
            .hash(),
            Err(TranscriptError::FieldTooLong {
                field: "initiator_ephemeral_public",
                len: MAX_TRANSCRIPT_FIELD_LEN + 1,
                max: MAX_TRANSCRIPT_FIELD_LEN,
            })
        );
    }
}
