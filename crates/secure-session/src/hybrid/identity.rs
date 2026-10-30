//! Ed25519 + ML-DSA-65 composite long-term identities.
//!
//! The composite public key and signature encodings are fixed by
//! `PROTOCOL_V2.md`. Both signature components are mandatory: no operation in
//! this module accepts a classic-only key or signature as a hybrid identity.
//!
//! Every signature is domain-separated with an explicit caller-supplied domain
//! and a canonical length-prefixed encoding. The identity identifier is
//! recomputed from the complete public key and must match the expected value
//! before either signature is accepted.

use crate::pq::{
    MlDsa65Signature, MlDsa65SigningKey, MlDsa65VerifyingKey, PqError, ML_DSA_65_PUBLIC_KEY_LEN,
    ML_DSA_65_SECRET_KEY_LEN, ML_DSA_65_SIGNATURE_LEN,
};
use ed25519_dalek::{
    Signature as Ed25519Signature, Signer as _, SigningKey as Ed25519SigningKey,
    VerifyingKey as Ed25519VerifyingKey, PUBLIC_KEY_LENGTH as ED25519_PUBLIC_KEY_LEN,
    SECRET_KEY_LENGTH as ED25519_SECRET_KEY_LEN, SIGNATURE_LENGTH as ED25519_SIGNATURE_LEN,
};
use std::fmt;
use thiserror::Error;
use zeroize::Zeroizing;

/// Domain used to derive a hybrid identity identifier.
pub const IDENTITY_DOMAIN: &[u8] = b"fuckipv6-v2 identity";
/// Width of each length prefix in the identity identifier derivation.
pub const IDENTITY_LENGTH_PREFIX_LEN: usize = 4;
/// Composite identity identifier length.
pub const IDENTITY_ID_LEN: usize = 32;
/// Composite public-key length: Ed25519 followed by ML-DSA-65.
pub const COMPOSITE_IDENTITY_PUBLIC_KEY_LEN: usize =
    ED25519_PUBLIC_KEY_LEN + ML_DSA_65_PUBLIC_KEY_LEN;
/// Composite signature length: Ed25519 followed by ML-DSA-65.
pub const COMPOSITE_SIGNATURE_LEN: usize = ED25519_SIGNATURE_LEN + ML_DSA_65_SIGNATURE_LEN;

/// Stable 256-bit identifier of a complete composite identity public key.
pub type IdentityId = [u8; IDENTITY_ID_LEN];

/// Errors returned by composite identity operations.
#[derive(Debug, Error)]
pub enum IdentityError {
    /// An underlying ML-DSA operation or fixed-size encoding failed.
    #[error(transparent)]
    Pq(#[from] PqError),
    /// The operating system CSPRNG failed.
    #[error("secure random generation failed")]
    Random,
    /// An Ed25519 public key encoding was invalid.
    #[error("invalid Ed25519 public key")]
    InvalidEd25519PublicKey,
    /// The supplied identity identifier does not match the complete public key.
    #[error("composite identity identifier mismatch")]
    IdentityMismatch,
    /// A signature domain must not be empty.
    #[error("signature domain must not be empty")]
    EmptyDomain,
    /// The canonical signature input exceeded a supported allocation.
    #[error("signature input is too large")]
    InputTooLarge,
    /// Ed25519 or ML-DSA-65 signature verification failed.
    #[error("composite signature verification failed")]
    VerificationFailed,
}

/// A complete Ed25519 + ML-DSA-65 public identity.
#[derive(Clone, PartialEq, Eq)]
pub struct CompositePublicKey {
    ed25519: Ed25519VerifyingKey,
    ml_dsa: MlDsa65VerifyingKey,
}

impl CompositePublicKey {
    /// Generate a fresh composite identity from the OS CSPRNG.
    pub fn generate() -> Result<(CompositeIdentity, Self), IdentityError> {
        let mut ed25519_seed = Zeroizing::new([0u8; ED25519_SECRET_KEY_LEN]);
        getrandom::fill(ed25519_seed.as_mut_slice()).map_err(|_| IdentityError::Random)?;
        let (ml_dsa, ml_dsa_public) = MlDsa65SigningKey::generate()?;
        let ed25519 = Ed25519SigningKey::from_bytes(&ed25519_seed);
        let public = Self {
            ed25519: ed25519.verifying_key(),
            ml_dsa: ml_dsa_public,
        };
        Ok((CompositeIdentity { ed25519, ml_dsa }, public))
    }

    /// Construct a composite identity from independent Ed25519 and ML-DSA seeds.
    ///
    /// This deterministic constructor is intended for persisted identities and
    /// known-answer tests. The caller remains responsible for protecting seeds.
    pub fn from_seeds(
        ed25519_seed: &[u8; ED25519_SECRET_KEY_LEN],
        ml_dsa_seed: &[u8; ML_DSA_65_SECRET_KEY_LEN],
    ) -> Result<(CompositeIdentity, Self), IdentityError> {
        let ed25519 = Ed25519SigningKey::from_bytes(ed25519_seed);
        let ml_dsa = MlDsa65SigningKey::from_seed(ml_dsa_seed)?;
        let public = Self {
            ed25519: ed25519.verifying_key(),
            ml_dsa: ml_dsa.verifying_key(),
        };
        Ok((CompositeIdentity { ed25519, ml_dsa }, public))
    }

    /// Decode the exact canonical `Ed25519PublicKey || ML-DSA-65PublicKey`.
    pub fn decode(bytes: &[u8]) -> Result<Self, IdentityError> {
        if bytes.len() != COMPOSITE_IDENTITY_PUBLIC_KEY_LEN {
            return Err(PqError::InvalidLength {
                item: "composite identity public key",
                expected: COMPOSITE_IDENTITY_PUBLIC_KEY_LEN,
                actual: bytes.len(),
            }
            .into());
        }
        let ed25519_bytes: &[u8; ED25519_PUBLIC_KEY_LEN] =
            bytes[..ED25519_PUBLIC_KEY_LEN].try_into().unwrap();
        let ed25519 = Ed25519VerifyingKey::from_bytes(ed25519_bytes)
            .map_err(|_| IdentityError::InvalidEd25519PublicKey)?;
        let ml_dsa = MlDsa65VerifyingKey::decode(&bytes[ED25519_PUBLIC_KEY_LEN..])?;
        Ok(Self { ed25519, ml_dsa })
    }

    /// Encode the exact canonical public-key representation.
    pub fn encode(&self) -> [u8; COMPOSITE_IDENTITY_PUBLIC_KEY_LEN] {
        let mut encoded = [0u8; COMPOSITE_IDENTITY_PUBLIC_KEY_LEN];
        encoded[..ED25519_PUBLIC_KEY_LEN].copy_from_slice(self.ed25519.as_bytes());
        encoded[ED25519_PUBLIC_KEY_LEN..].copy_from_slice(&self.ml_dsa.encode());
        encoded
    }

    /// Recompute the stable identifier from the complete public key.
    pub fn identity_id(&self) -> IdentityId {
        let encoded = self.encode();
        let mut hasher = blake3::Hasher::new();
        hasher.update(IDENTITY_DOMAIN);
        hasher.update(&(ED25519_PUBLIC_KEY_LEN as u32).to_be_bytes());
        hasher.update(&encoded[..ED25519_PUBLIC_KEY_LEN]);
        hasher.update(&(ML_DSA_65_PUBLIC_KEY_LEN as u32).to_be_bytes());
        hasher.update(&encoded[ED25519_PUBLIC_KEY_LEN..]);
        *hasher.finalize().as_bytes()
    }

    /// Verify both signatures after binding the complete key to `expected_id`.
    ///
    /// All signature fields are mandatory. Ed25519 uses strict verification.
    /// The composite signature must already have passed strict length and
    /// encoding checks through [`CompositeSignature::decode`].
    pub fn verify(
        &self,
        expected_id: IdentityId,
        domain: &[u8],
        message: &[u8],
        signature: &CompositeSignature,
    ) -> Result<(), IdentityError> {
        if self.identity_id() != expected_id {
            return Err(IdentityError::IdentityMismatch);
        }
        let signed = canonical_signature_input(domain, message)?;
        let ed25519_signature = Ed25519Signature::from_bytes(&signature.ed25519);
        let ed25519_valid = self
            .ed25519
            .verify_strict(&signed, &ed25519_signature)
            .is_ok();
        let ml_dsa_valid = self.ml_dsa.verify(&signed, &signature.ml_dsa).is_ok();
        if ed25519_valid && ml_dsa_valid {
            Ok(())
        } else {
            Err(IdentityError::VerificationFailed)
        }
    }
}

impl fmt::Debug for CompositePublicKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CompositePublicKey")
            .field("identity_id", &self.identity_id())
            .finish_non_exhaustive()
    }
}

/// A composite long-term signing identity.
pub struct CompositeIdentity {
    ed25519: Ed25519SigningKey,
    ml_dsa: MlDsa65SigningKey,
}

impl CompositeIdentity {
    /// Return the complete corresponding public identity.
    pub fn public_key(&self) -> CompositePublicKey {
        CompositePublicKey {
            ed25519: self.ed25519.verifying_key(),
            ml_dsa: self.ml_dsa.verifying_key(),
        }
    }

    /// Return the identifier derived from the complete public key.
    pub fn identity_id(&self) -> IdentityId {
        self.public_key().identity_id()
    }

    /// Sign a domain-separated message with both Ed25519 and ML-DSA-65.
    pub fn sign(&self, domain: &[u8], message: &[u8]) -> Result<CompositeSignature, IdentityError> {
        let signed = canonical_signature_input(domain, message)?;
        let ed25519 = self.ed25519.sign(&signed).to_bytes();
        let ml_dsa = self.ml_dsa.sign(&signed)?;
        Ok(CompositeSignature { ed25519, ml_dsa })
    }
}

impl fmt::Debug for CompositeIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CompositeIdentity(<redacted>)")
    }
}

/// A fixed-size `Ed25519Signature || ML-DSA-65Signature` value.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CompositeSignature {
    ed25519: [u8; ED25519_SIGNATURE_LEN],
    ml_dsa: MlDsa65Signature,
}

impl CompositeSignature {
    /// Decode an exact canonical composite signature.
    pub fn decode(bytes: &[u8]) -> Result<Self, IdentityError> {
        if bytes.len() != COMPOSITE_SIGNATURE_LEN {
            return Err(PqError::InvalidLength {
                item: "composite identity signature",
                expected: COMPOSITE_SIGNATURE_LEN,
                actual: bytes.len(),
            }
            .into());
        }
        let ed25519 = bytes[..ED25519_SIGNATURE_LEN]
            .try_into()
            .expect("length was checked");
        let ml_dsa = MlDsa65Signature::decode(&bytes[ED25519_SIGNATURE_LEN..])?;
        Ok(Self { ed25519, ml_dsa })
    }

    /// Encode the exact canonical composite signature.
    pub fn encode(self) -> [u8; COMPOSITE_SIGNATURE_LEN] {
        let mut encoded = [0u8; COMPOSITE_SIGNATURE_LEN];
        encoded[..ED25519_SIGNATURE_LEN].copy_from_slice(&self.ed25519);
        encoded[ED25519_SIGNATURE_LEN..].copy_from_slice(&self.ml_dsa.encode());
        encoded
    }
}

impl fmt::Debug for CompositeSignature {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("CompositeSignature")
            .field(&COMPOSITE_SIGNATURE_LEN)
            .finish()
    }
}

fn canonical_signature_input(domain: &[u8], message: &[u8]) -> Result<Vec<u8>, IdentityError> {
    if domain.is_empty() {
        return Err(IdentityError::EmptyDomain);
    }
    let domain_len = u32::try_from(domain.len()).map_err(|_| IdentityError::InputTooLarge)?;
    let message_len = u32::try_from(message.len()).map_err(|_| IdentityError::InputTooLarge)?;
    let capacity = IDENTITY_LENGTH_PREFIX_LEN
        .checked_add(domain.len())
        .and_then(|len| len.checked_add(IDENTITY_LENGTH_PREFIX_LEN))
        .and_then(|len| len.checked_add(message.len()))
        .ok_or(IdentityError::InputTooLarge)?;
    let mut canonical = Vec::with_capacity(capacity);
    canonical.extend_from_slice(&domain_len.to_be_bytes());
    canonical.extend_from_slice(domain);
    canonical.extend_from_slice(&message_len.to_be_bytes());
    canonical.extend_from_slice(message);
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deterministic_identity() -> (CompositeIdentity, CompositePublicKey) {
        CompositePublicKey::from_seeds(
            &[0x11; ED25519_SECRET_KEY_LEN],
            &[0x22; ML_DSA_65_SECRET_KEY_LEN],
        )
        .unwrap()
    }

    #[test]
    fn composite_identity_round_trips_and_verifies_both_signatures() {
        let (identity, public) = deterministic_identity();
        let encoded = public.encode();
        assert_eq!(encoded.len(), COMPOSITE_IDENTITY_PUBLIC_KEY_LEN);
        let decoded = CompositePublicKey::decode(&encoded).unwrap();
        assert_eq!(decoded, public);
        assert_eq!(decoded.identity_id(), public.identity_id());
        const EXPECTED_ID: IdentityId = [
            0x8e, 0x1d, 0xc6, 0x2b, 0xa7, 0xdb, 0x50, 0x5b, 0xcf, 0x12, 0xc6, 0x69, 0x96, 0xc8,
            0xf1, 0xcd, 0x91, 0xa2, 0x59, 0x25, 0x81, 0xbd, 0xb2, 0xba, 0x27, 0x42, 0xdf, 0xd0,
            0x72, 0x24, 0x03, 0x52,
        ];
        assert_eq!(public.identity_id(), EXPECTED_ID);

        let signature = identity
            .sign(b"fuckipv6-v2 test hello", b"message")
            .unwrap();
        let signature_bytes = signature.encode();
        assert_eq!(signature_bytes.len(), COMPOSITE_SIGNATURE_LEN);
        let decoded_signature = CompositeSignature::decode(&signature_bytes).unwrap();
        decoded
            .verify(
                public.identity_id(),
                b"fuckipv6-v2 test hello",
                b"message",
                &decoded_signature,
            )
            .unwrap();
    }

    #[test]
    fn either_signature_component_is_mandatory() {
        let (identity, public) = deterministic_identity();
        let signature = identity.sign(b"handshake", b"message").unwrap().encode();

        let mut ed25519_tampered = signature;
        ed25519_tampered[0] ^= 1;
        let tampered = CompositeSignature::decode(&ed25519_tampered).unwrap();
        assert!(matches!(
            public.verify(public.identity_id(), b"handshake", b"message", &tampered),
            Err(IdentityError::VerificationFailed)
        ));

        let mut ml_dsa_tampered = signature;
        ml_dsa_tampered[ED25519_SIGNATURE_LEN] ^= 1;
        let tampered = CompositeSignature::decode(&ml_dsa_tampered).unwrap();
        assert!(matches!(
            public.verify(public.identity_id(), b"handshake", b"message", &tampered),
            Err(IdentityError::VerificationFailed)
        ));

        assert!(CompositeSignature::decode(&signature[..ED25519_SIGNATURE_LEN]).is_err());
        assert!(CompositeSignature::decode(&[&signature[..], &[0]].concat()).is_err());
    }

    #[test]
    fn classic_only_material_and_public_key_substitution_fail_closed() {
        let (identity, public) = deterministic_identity();
        let signature = identity.sign(b"handshake", b"message").unwrap();
        assert!(CompositePublicKey::decode(&[0x11; ED25519_PUBLIC_KEY_LEN]).is_err());
        assert!(CompositePublicKey::decode(&[0x11; 1184]).is_err());

        let (_, other_public) = CompositePublicKey::from_seeds(
            &[0x33; ED25519_SECRET_KEY_LEN],
            &[0x44; ML_DSA_65_SECRET_KEY_LEN],
        )
        .unwrap();
        assert!(matches!(
            other_public.verify(public.identity_id(), b"handshake", b"message", &signature),
            Err(IdentityError::IdentityMismatch)
        ));
    }

    #[test]
    fn signature_domains_are_bound_and_empty_domains_are_rejected() {
        let (identity, public) = deterministic_identity();
        let signature = identity.sign(b"hello", b"message").unwrap();
        assert!(matches!(
            public.verify(public.identity_id(), b"accept", b"message", &signature),
            Err(IdentityError::VerificationFailed)
        ));
        assert!(matches!(
            identity.sign(b"", b"message"),
            Err(IdentityError::EmptyDomain)
        ));
    }

    #[test]
    fn identity_id_binds_both_public_key_components() {
        let (_, public) = deterministic_identity();
        let mut encoded = public.encode();
        let ed25519_id = public.identity_id();
        encoded[0] ^= 1;
        if let Ok(changed_ed) = CompositePublicKey::decode(&encoded) {
            assert_ne!(changed_ed.identity_id(), ed25519_id);
        }

        let mut encoded = public.encode();
        encoded[ED25519_PUBLIC_KEY_LEN] ^= 1;
        if let Ok(changed_ml_dsa) = CompositePublicKey::decode(&encoded) {
            assert_ne!(changed_ml_dsa.identity_id(), ed25519_id);
        }
    }

    #[test]
    fn private_keys_and_signatures_do_not_expose_raw_debug_bytes() {
        let (identity, _) = deterministic_identity();
        let signature = identity.sign(b"handshake", b"message").unwrap();
        assert_eq!(format!("{identity:?}"), "CompositeIdentity(<redacted>)");
        assert_eq!(
            format!("{signature:?}"),
            format!("CompositeSignature({})", COMPOSITE_SIGNATURE_LEN)
        );
    }
}
