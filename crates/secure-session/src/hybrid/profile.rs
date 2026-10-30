//! Fail-closed profile and algorithm policy for the hybrid PQ handshake.
//!
//! The profile identifier, suite identifier, KEM, signature construction, KDF
//! and data AEAD are one indivisible selection. Callers cannot independently
//! substitute an algorithm or fall back to the classic profile after a hybrid
//! negotiation failure.

use super::handshake::HYBRID_PROFILE_ID;
use crate::transcript::{SUITE_AES_256_GCM, SUITE_CHACHA20_POLY1305};
use crate::{DataCipherSuite, PROTOCOL_VERSION};
use thiserror::Error;

/// X-Wing revision currently bound to the hybrid profile.
pub const XWING_DRAFT_VERSION: u8 = 11;

/// Key-encapsulation mechanism bound to the hybrid profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KemAlgorithm {
    /// X-Wing draft-11: X25519 + ML-KEM-768 + SHA3-256.
    XWingDraft11,
}

impl KemAlgorithm {
    pub const fn name(self) -> &'static str {
        match self {
            Self::XWingDraft11 => "X-Wing-draft-11/X25519+ML-KEM-768",
        }
    }
}

/// Long-term signature construction bound to the hybrid profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureAlgorithm {
    /// Both Ed25519 and ML-DSA-65 signatures are mandatory.
    Ed25519MlDsa65,
}

impl SignatureAlgorithm {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ed25519MlDsa65 => "Ed25519+ML-DSA-65",
        }
    }
}

/// Handshake key-derivation function bound to the hybrid profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeKdf {
    HkdfSha512,
}

impl HandshakeKdf {
    pub const fn name(self) -> &'static str {
        match self {
            Self::HkdfSha512 => "HKDF-SHA-512",
        }
    }
}

/// Data-plane AEAD selected by the hybrid suite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataAead {
    Aes256Gcm,
    ChaCha20Poly1305,
}

impl DataAead {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Aes256Gcm => "AES-256-GCM",
            Self::ChaCha20Poly1305 => "ChaCha20-Poly1305",
        }
    }
}

/// Fully specified hybrid data suite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HybridSuite {
    Aes256Gcm,
    ChaCha20Poly1305,
}

impl HybridSuite {
    /// Resolve one exact suite identifier. Unknown identifiers never map to a
    /// default suite.
    pub const fn from_id(suite_id: u16) -> Result<Self, ProfileNegotiationError> {
        match suite_id {
            SUITE_AES_256_GCM => Ok(Self::Aes256Gcm),
            SUITE_CHACHA20_POLY1305 => Ok(Self::ChaCha20Poly1305),
            other => Err(ProfileNegotiationError::UnsupportedSuite { offered: other }),
        }
    }

    pub const fn id(self) -> u16 {
        match self {
            Self::Aes256Gcm => SUITE_AES_256_GCM,
            Self::ChaCha20Poly1305 => SUITE_CHACHA20_POLY1305,
        }
    }

    pub const fn data_aead(self) -> DataAead {
        match self {
            Self::Aes256Gcm => DataAead::Aes256Gcm,
            Self::ChaCha20Poly1305 => DataAead::ChaCha20Poly1305,
        }
    }
}

/// A locally selected, fully bound hybrid profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NegotiatedHybridProfile {
    suite: HybridSuite,
}

impl NegotiatedHybridProfile {
    /// Select a suite from local configuration. This is not peer-driven
    /// negotiation; peer messages must later match this selection exactly.
    pub const fn required(suite: HybridSuite) -> Self {
        Self { suite }
    }

    pub const fn version(self) -> u8 {
        PROTOCOL_VERSION
    }

    pub const fn profile_id(self) -> u8 {
        HYBRID_PROFILE_ID
    }

    pub const fn suite(self) -> HybridSuite {
        self.suite
    }

    pub const fn suite_id(self) -> u16 {
        self.suite.id()
    }

    pub const fn kem(self) -> KemAlgorithm {
        KemAlgorithm::XWingDraft11
    }

    pub const fn signature(self) -> SignatureAlgorithm {
        SignatureAlgorithm::Ed25519MlDsa65
    }

    pub const fn kdf(self) -> HandshakeKdf {
        HandshakeKdf::HkdfSha512
    }

    pub const fn data_aead(self) -> DataAead {
        self.suite.data_aead()
    }

    /// Convert the profile to the existing data-plane suite enum.
    pub const fn data_cipher_suite(self) -> DataCipherSuite {
        match self.suite {
            HybridSuite::Aes256Gcm => DataCipherSuite::Aes256Gcm,
            HybridSuite::ChaCha20Poly1305 => DataCipherSuite::ChaCha20Poly1305,
        }
    }

    /// Require an incoming version/profile/suite tuple to equal this selection.
    pub const fn validate(
        self,
        version: u8,
        profile_id: u8,
        suite_id: u16,
    ) -> Result<(), ProfileNegotiationError> {
        if version != PROTOCOL_VERSION {
            return Err(ProfileNegotiationError::ProtocolVersion {
                expected: PROTOCOL_VERSION,
                found: version,
            });
        }
        if profile_id != HYBRID_PROFILE_ID {
            return Err(ProfileNegotiationError::ProfileMismatch {
                expected: HYBRID_PROFILE_ID,
                found: profile_id,
            });
        }
        if suite_id != self.suite_id() {
            return Err(ProfileNegotiationError::SuiteMismatch {
                required: self.suite_id(),
                found: suite_id,
            });
        }
        Ok(())
    }
}

/// Local responder policy requiring the hybrid profile and one exact suite.
///
/// There is intentionally no fallback flag. A peer offering classic, another
/// hybrid suite, or an unknown algorithm is rejected before KEM work or replay
/// cache mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HybridProfilePolicy {
    required: NegotiatedHybridProfile,
}

impl HybridProfilePolicy {
    pub const fn require(suite: HybridSuite) -> Self {
        Self {
            required: NegotiatedHybridProfile::required(suite),
        }
    }

    pub const fn required(self) -> NegotiatedHybridProfile {
        self.required
    }

    /// Validate the wire selection without selecting an alternative.
    pub const fn negotiate(
        self,
        version: u8,
        profile_id: u8,
        suite_id: u16,
    ) -> Result<NegotiatedHybridProfile, ProfileNegotiationError> {
        match self.required.validate(version, profile_id, suite_id) {
            Ok(()) => Ok(self.required),
            Err(error) => Err(error),
        }
    }
}

/// Fail-closed profile negotiation errors.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ProfileNegotiationError {
    #[error("unsupported protocol version {found}; expected {expected}")]
    ProtocolVersion { expected: u8, found: u8 },
    #[error("hybrid profile required, peer offered profile {found}; expected {expected}")]
    ProfileMismatch { expected: u8, found: u8 },
    #[error("unsupported hybrid data suite {offered:#06x}")]
    UnsupportedSuite { offered: u16 },
    #[error("hybrid suite {found:#06x} does not match required suite {required:#06x}")]
    SuiteMismatch { required: u16, found: u16 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_suite_has_one_complete_algorithm_binding() {
        for (suite, suite_id, aead) in [
            (HybridSuite::Aes256Gcm, 0x0201, DataAead::Aes256Gcm),
            (
                HybridSuite::ChaCha20Poly1305,
                0x0202,
                DataAead::ChaCha20Poly1305,
            ),
        ] {
            let profile = NegotiatedHybridProfile::required(suite);
            assert_eq!(profile.version(), PROTOCOL_VERSION);
            assert_eq!(profile.profile_id(), HYBRID_PROFILE_ID);
            assert_eq!(profile.suite_id(), suite_id);
            assert_eq!(profile.kem(), KemAlgorithm::XWingDraft11);
            assert_eq!(profile.signature(), SignatureAlgorithm::Ed25519MlDsa65);
            assert_eq!(profile.kdf(), HandshakeKdf::HkdfSha512);
            assert_eq!(profile.data_aead(), aead);
            assert_eq!(HybridSuite::from_id(suite_id), Ok(suite));
        }
    }

    #[test]
    fn policy_rejects_profile_and_suite_downgrade() {
        let policy = HybridProfilePolicy::require(HybridSuite::ChaCha20Poly1305);
        assert_eq!(
            policy.negotiate(PROTOCOL_VERSION, 1, SUITE_CHACHA20_POLY1305),
            Err(ProfileNegotiationError::ProfileMismatch {
                expected: HYBRID_PROFILE_ID,
                found: 1,
            })
        );
        assert_eq!(
            policy.negotiate(PROTOCOL_VERSION, HYBRID_PROFILE_ID, SUITE_AES_256_GCM),
            Err(ProfileNegotiationError::SuiteMismatch {
                required: SUITE_CHACHA20_POLY1305,
                found: SUITE_AES_256_GCM,
            })
        );
    }

    #[test]
    fn policy_rejects_unknown_versions_and_algorithms() {
        let policy = HybridProfilePolicy::require(HybridSuite::Aes256Gcm);
        assert!(matches!(
            policy.negotiate(PROTOCOL_VERSION + 1, HYBRID_PROFILE_ID, SUITE_AES_256_GCM),
            Err(ProfileNegotiationError::ProtocolVersion { .. })
        ));
        assert_eq!(
            HybridSuite::from_id(0x0301),
            Err(ProfileNegotiationError::UnsupportedSuite { offered: 0x0301 })
        );
    }
}
