//! Optional FIPS 203 ML-KEM-768 and FIPS 204 ML-DSA-65 primitive adapters.
//!
//! This module is compiled only with the `pq` feature. It provides strict,
//! fixed-size byte interfaces around the RustCrypto implementations and keeps
//! private material in zeroizing containers. The separate [`crate::hybrid`]
//! module builds X-Wing from these adapters. This module does not implement a
//! composite Ed25519 + ML-DSA identity, a post-quantum wire handshake, or any
//! control-channel messages.
//!
//! The upstream `ml-kem` and `ml-dsa` crates state that their implementations
//! have never been independently audited. These adapters therefore inherit that
//! limitation and must not be used to claim production security.

use getrandom::fill as fill_random;
use ml_dsa::{
    KeyExport as _, KeyInit as _, Keypair as _, MlDsa65, Signature as NativeMlDsaSignature,
    Signer as _, SigningKey, VerifyingKey,
};
use ml_kem::{
    Ciphertext as NativeMlKemCiphertext, Decapsulate as _, FromSeed as _, MlKem768, TryKeyInit as _,
};
use std::fmt;
use thiserror::Error;
use zeroize::Zeroizing;

/// Encoded ML-KEM-768 encapsulation (public) key length.
pub const ML_KEM_768_PUBLIC_KEY_LEN: usize = 1184;
/// Encoded ML-KEM-768 decapsulation seed length.
pub const ML_KEM_768_SECRET_KEY_LEN: usize = 64;
/// Encoded ML-KEM-768 ciphertext length.
pub const ML_KEM_768_CIPHERTEXT_LEN: usize = 1088;
/// ML-KEM-768 shared-secret length.
pub const ML_KEM_768_SHARED_SECRET_LEN: usize = 32;
/// Encoded ML-DSA-65 signing seed length.
pub const ML_DSA_65_SECRET_KEY_LEN: usize = 32;
/// Encoded ML-DSA-65 verifying (public) key length.
pub const ML_DSA_65_PUBLIC_KEY_LEN: usize = 1952;
/// Encoded ML-DSA-65 signature length.
pub const ML_DSA_65_SIGNATURE_LEN: usize = 3309;

/// Error returned by the post-quantum primitive adapters.
#[derive(Debug, Error)]
pub enum PqError {
    /// An encoded value did not have its required fixed size.
    #[error("{item} must be exactly {expected} bytes, got {actual}")]
    InvalidLength {
        /// Name of the malformed value.
        item: &'static str,
        /// Required byte length.
        expected: usize,
        /// Supplied byte length.
        actual: usize,
    },
    /// The OS random number generator failed.
    #[error("secure random generation failed")]
    Random,
    /// An encoded ML-KEM-768 public key was rejected by the implementation.
    #[error("invalid ML-KEM-768 public key encoding")]
    InvalidKemPublicKey,
    /// The ML-DSA-65 implementation failed to produce a signature.
    #[error("ML-DSA-65 signing failed")]
    SigningFailed,
    /// An encoded ML-DSA-65 public key was rejected by the implementation.
    #[error("invalid ML-DSA-65 public key encoding")]
    InvalidDsaPublicKey,
    /// An encoded ML-DSA-65 signature was rejected.
    #[error("invalid ML-DSA-65 signature encoding")]
    InvalidDsaSignature,
    /// ML-DSA-65 signature verification failed.
    #[error("ML-DSA-65 signature verification failed")]
    SignatureVerificationFailed,
}

fn exact<const N: usize>(bytes: &[u8], item: &'static str) -> Result<[u8; N], PqError> {
    if bytes.len() != N {
        return Err(PqError::InvalidLength {
            item,
            expected: N,
            actual: bytes.len(),
        });
    }
    let mut output = [0u8; N];
    output.copy_from_slice(bytes);
    Ok(output)
}

/// An ML-KEM-768 public key in canonical FIPS 203 encoding.
#[derive(Clone, PartialEq, Eq)]
pub struct MlKem768PublicKey {
    key: ml_kem::EncapsulationKey<MlKem768>,
}

impl MlKem768PublicKey {
    /// Decode an exact 1184-byte ML-KEM-768 public key.
    pub fn decode(bytes: &[u8]) -> Result<Self, PqError> {
        exact::<ML_KEM_768_PUBLIC_KEY_LEN>(bytes, "ML-KEM-768 public key")?;
        let key = ml_kem::EncapsulationKey::<MlKem768>::new_from_slice(bytes)
            .map_err(|_| PqError::InvalidKemPublicKey)?;
        Ok(Self { key })
    }

    /// Encode this public key in its exact FIPS 203 representation.
    pub fn encode(&self) -> [u8; ML_KEM_768_PUBLIC_KEY_LEN] {
        let bytes = self.key.to_bytes();
        exact(bytes.as_slice(), "ML-KEM-768 public key")
            .expect("fixed-size public key encoding changed")
    }

    /// Generate a fresh shared secret and encapsulation ciphertext.
    ///
    /// The random 32-byte encapsulation message is held in a zeroizing buffer.
    pub fn encapsulate(&self) -> Result<(MlKem768Ciphertext, MlKem768SharedSecret), PqError> {
        let mut message = Zeroizing::new([0u8; 32]);
        fill_random(message.as_mut_slice()).map_err(|_| PqError::Random)?;
        Ok(self.encapsulate_with_message(&message))
    }

    pub(crate) fn encapsulate_with_message(
        &self,
        message: &[u8; 32],
    ) -> (MlKem768Ciphertext, MlKem768SharedSecret) {
        let message = ml_kem::B32::from(*message);
        let (ciphertext, shared_secret) = self.key.encapsulate_deterministic(&message);
        (
            MlKem768Ciphertext {
                bytes: exact(ciphertext.as_slice(), "ML-KEM-768 ciphertext")
                    .expect("fixed-size ciphertext encoding changed"),
            },
            MlKem768SharedSecret::from_native(shared_secret.as_slice()),
        )
    }
}

impl fmt::Debug for MlKem768PublicKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MlKem768PublicKey(..)")
    }
}

/// An ML-KEM-768 decapsulation key represented by its 64-byte FIPS 203 seed.
///
/// The upstream implementation expands this seed internally and zeroizes both
/// the stored seed and expanded key material on drop.
pub struct MlKem768SecretKey {
    key: ml_kem::DecapsulationKey<MlKem768>,
}

impl MlKem768SecretKey {
    /// Decode an exact 64-byte ML-KEM-768 decapsulation seed.
    pub fn from_seed(seed: &[u8]) -> Result<Self, PqError> {
        exact::<ML_KEM_768_SECRET_KEY_LEN>(seed, "ML-KEM-768 decapsulation seed")?;
        let seed = ml_kem::Seed::try_from(seed).map_err(|_| PqError::InvalidLength {
            item: "ML-KEM-768 decapsulation seed",
            expected: ML_KEM_768_SECRET_KEY_LEN,
            actual: seed.len(),
        })?;
        let (key, _) = MlKem768::from_seed(&seed);
        Ok(Self { key })
    }

    /// Generate a fresh ML-KEM-768 keypair using the OS random number generator.
    pub fn generate() -> Result<(Self, MlKem768PublicKey), PqError> {
        let mut seed = Zeroizing::new([0u8; ML_KEM_768_SECRET_KEY_LEN]);
        fill_random(seed.as_mut_slice()).map_err(|_| PqError::Random)?;
        Self::from_seed_value(seed.as_slice())
    }

    fn from_seed_value(seed: &[u8]) -> Result<(Self, MlKem768PublicKey), PqError> {
        let secret_key = Self::from_seed(seed)?;
        let public_key = MlKem768PublicKey {
            key: secret_key.key.encapsulation_key().clone(),
        };
        Ok((secret_key, public_key))
    }

    /// Return the corresponding ML-KEM-768 encapsulation key.
    pub(crate) fn encapsulation_key(&self) -> MlKem768PublicKey {
        MlKem768PublicKey {
            key: self.key.encapsulation_key().clone(),
        }
    }

    /// Export a zeroizing copy of the 64-byte decapsulation seed.
    pub fn to_seed(&self) -> Zeroizing<[u8; ML_KEM_768_SECRET_KEY_LEN]> {
        let seed = self
            .key
            .to_seed()
            .expect("secret key is always initialized from a seed");
        Zeroizing::new(
            exact(seed.as_slice(), "ML-KEM-768 decapsulation seed")
                .expect("fixed-size seed encoding changed"),
        )
    }

    /// Decapsulate a fixed-size ML-KEM-768 ciphertext.
    ///
    /// ML-KEM uses implicit rejection for malformed or modified ciphertexts, so
    /// this operation always returns a shared secret. Callers must authenticate
    /// the handshake transcript to detect a substituted ciphertext.
    pub fn decapsulate(&self, ciphertext: &MlKem768Ciphertext) -> MlKem768SharedSecret {
        let ciphertext = NativeMlKemCiphertext::<MlKem768>::try_from(ciphertext.bytes.as_slice())
            .expect("ciphertext length was validated at construction");
        let shared_secret = self.key.decapsulate(&ciphertext);
        MlKem768SharedSecret::from_native(shared_secret.as_slice())
    }
}

impl fmt::Debug for MlKem768SecretKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MlKem768SecretKey(<redacted>)")
    }
}

/// An ML-KEM-768 ciphertext in canonical FIPS 203 encoding.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct MlKem768Ciphertext {
    bytes: [u8; ML_KEM_768_CIPHERTEXT_LEN],
}

impl MlKem768Ciphertext {
    /// Decode an exact 1088-byte ML-KEM-768 ciphertext.
    pub fn decode(bytes: &[u8]) -> Result<Self, PqError> {
        Ok(Self {
            bytes: exact(bytes, "ML-KEM-768 ciphertext")?,
        })
    }

    /// Encode this ciphertext in its exact FIPS 203 representation.
    pub const fn encode(self) -> [u8; ML_KEM_768_CIPHERTEXT_LEN] {
        self.bytes
    }
}

impl fmt::Debug for MlKem768Ciphertext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("MlKem768Ciphertext")
            .field(&self.bytes.len())
            .finish()
    }
}

/// A zeroizing ML-KEM-768 shared secret.
pub struct MlKem768SharedSecret {
    bytes: Zeroizing<[u8; ML_KEM_768_SHARED_SECRET_LEN]>,
}

impl MlKem768SharedSecret {
    fn from_native(bytes: &[u8]) -> Self {
        Self {
            bytes: Zeroizing::new(
                exact(bytes, "ML-KEM-768 shared secret").expect("fixed-size KEM output changed"),
            ),
        }
    }

    /// Borrow the raw shared secret for domain-separated key derivation.
    pub fn expose_secret(&self) -> &[u8; ML_KEM_768_SHARED_SECRET_LEN] {
        &self.bytes
    }
}

impl fmt::Debug for MlKem768SharedSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MlKem768SharedSecret(<redacted>)")
    }
}

/// An ML-DSA-65 verifying (public) key in canonical FIPS 204 encoding.
#[derive(Clone)]
pub struct MlDsa65VerifyingKey {
    key: VerifyingKey<MlDsa65>,
}

impl PartialEq for MlDsa65VerifyingKey {
    fn eq(&self, other: &Self) -> bool {
        self.encode() == other.encode()
    }
}

impl Eq for MlDsa65VerifyingKey {}

impl MlDsa65VerifyingKey {
    /// Decode an exact 1952-byte ML-DSA-65 verifying key.
    pub fn decode(bytes: &[u8]) -> Result<Self, PqError> {
        exact::<ML_DSA_65_PUBLIC_KEY_LEN>(bytes, "ML-DSA-65 verifying key")?;
        let key = VerifyingKey::<MlDsa65>::new_from_slice(bytes)
            .map_err(|_| PqError::InvalidDsaPublicKey)?;
        Ok(Self { key })
    }

    /// Encode this verifying key in its exact FIPS 204 representation.
    pub fn encode(&self) -> [u8; ML_DSA_65_PUBLIC_KEY_LEN] {
        let bytes = self.key.to_bytes();
        exact(bytes.as_slice(), "ML-DSA-65 verifying key")
            .expect("fixed-size verifying key encoding changed")
    }

    /// Verify one ML-DSA-65 signature over an application message.
    ///
    /// This adapter deliberately uses ML-DSA's empty-context form. Protocol
    /// callers must put all domain separation into `message`.
    pub fn verify(&self, message: &[u8], signature: &MlDsa65Signature) -> Result<(), PqError> {
        let signature = NativeMlDsaSignature::<MlDsa65>::try_from(signature.bytes.as_slice())
            .map_err(|_| PqError::InvalidDsaSignature)?;
        ml_dsa::Verifier::verify(&self.key, message, &signature)
            .map_err(|_| PqError::SignatureVerificationFailed)
    }
}

impl fmt::Debug for MlDsa65VerifyingKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MlDsa65VerifyingKey(..)")
    }
}

/// An ML-DSA-65 signing key represented by its 32-byte FIPS 204 seed.
///
/// The upstream implementation stores expanded secret components and zeroizes
/// them together with the seed on drop.
pub struct MlDsa65SigningKey {
    key: SigningKey<MlDsa65>,
}

impl MlDsa65SigningKey {
    /// Decode an exact 32-byte ML-DSA-65 signing seed.
    pub fn from_seed(seed: &[u8]) -> Result<Self, PqError> {
        exact::<ML_DSA_65_SECRET_KEY_LEN>(seed, "ML-DSA-65 signing seed")?;
        let seed = ml_dsa::Seed::try_from(seed).map_err(|_| PqError::InvalidLength {
            item: "ML-DSA-65 signing seed",
            expected: ML_DSA_65_SECRET_KEY_LEN,
            actual: seed.len(),
        })?;
        Ok(Self {
            key: SigningKey::<MlDsa65>::from_seed(&seed),
        })
    }

    /// Generate a fresh ML-DSA-65 keypair using the OS random number generator.
    pub fn generate() -> Result<(Self, MlDsa65VerifyingKey), PqError> {
        let mut seed = Zeroizing::new([0u8; ML_DSA_65_SECRET_KEY_LEN]);
        fill_random(seed.as_mut_slice()).map_err(|_| PqError::Random)?;
        Self::from_seed_value(seed.as_slice())
    }

    fn from_seed_value(seed: &[u8]) -> Result<(Self, MlDsa65VerifyingKey), PqError> {
        let signing_key = Self::from_seed(seed)?;
        let verifying_key = MlDsa65VerifyingKey {
            key: signing_key.key.verifying_key(),
        };
        Ok((signing_key, verifying_key))
    }

    /// Export a zeroizing copy of the 32-byte signing seed.
    pub fn to_seed(&self) -> Zeroizing<[u8; ML_DSA_65_SECRET_KEY_LEN]> {
        let seed = self.key.to_seed();
        Zeroizing::new(
            exact(seed.as_slice(), "ML-DSA-65 signing seed")
                .expect("fixed-size seed encoding changed"),
        )
    }

    /// Return the corresponding verifying key.
    pub fn verifying_key(&self) -> MlDsa65VerifyingKey {
        MlDsa65VerifyingKey {
            key: self.key.verifying_key(),
        }
    }

    /// Produce a deterministic ML-DSA-65 signature over `message`.
    ///
    /// The caller is responsible for domain-separating `message`.
    pub fn sign(&self, message: &[u8]) -> Result<MlDsa65Signature, PqError> {
        let signature = self
            .key
            .try_sign(message)
            .map_err(|_| PqError::SigningFailed)?;
        Ok(MlDsa65Signature {
            bytes: exact(signature.encode().as_slice(), "ML-DSA-65 signature")
                .expect("fixed-size signature encoding changed"),
        })
    }
}

impl fmt::Debug for MlDsa65SigningKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MlDsa65SigningKey(<redacted>)")
    }
}

/// An ML-DSA-65 signature in canonical FIPS 204 encoding.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct MlDsa65Signature {
    bytes: [u8; ML_DSA_65_SIGNATURE_LEN],
}

impl MlDsa65Signature {
    /// Decode an exact 3309-byte ML-DSA-65 signature.
    pub fn decode(bytes: &[u8]) -> Result<Self, PqError> {
        exact::<ML_DSA_65_SIGNATURE_LEN>(bytes, "ML-DSA-65 signature")?;
        NativeMlDsaSignature::<MlDsa65>::try_from(bytes)
            .map_err(|_| PqError::InvalidDsaSignature)?;
        Ok(Self {
            bytes: exact(bytes, "ML-DSA-65 signature")?,
        })
    }

    /// Encode this signature in its exact FIPS 204 representation.
    pub const fn encode(self) -> [u8; ML_DSA_65_SIGNATURE_LEN] {
        self.bytes
    }
}

impl fmt::Debug for MlDsa65Signature {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("MlDsa65Signature")
            .field(&self.bytes.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ml_dsa::ExpandedSigningKey;
    use sha2::{Digest, Sha256};

    const ML_KEM_ACVP_SEED: &str = concat!(
        "E34A701C4C87582F42264EE422D3C684D97611F2523EFE0C998AF05056D693DC",
        "A85768F3486BD32A01BF9A8F21EA938E648EAE4E5448C34C3EB88820B159EEDD",
    );
    const ML_KEM_ACVP_PUBLIC_KEY_SHA256: &str =
        "7799c9d8eef172aa78c073514f2f039c240de8c5cb61bca82ba0bc46041ce279";
    const ML_KEM_ENCAP_CIPHERTEXT_SHA256: &str =
        "ac57163b80ead205b8323e1402b8ca66bece40d8df9994b12d43bbb4f6e19bf4";
    const ML_KEM_ENCAP_SHARED_SECRET_SHA256: &str =
        "112e96545282948046d8bbd675b5554d0343999ba3d9c73de2a25f7fd9adbe60";
    const ML_DSA_ACVP_SEED: &str =
        "70CEFB9AED5B68E018B079DA8284B9D5CAD5499ED9C265FF73588005D85C225C";
    const ML_DSA_ACVP_PUBLIC_KEY_SHA256: &str =
        "646b26b8d09dbc9e865b6a006c693a3127b065e62fab5fbe8b159c416462feb6";
    const ML_DSA_SIGNATURE_SHA256: &str =
        "3ad5f23abe9811091f76fce1b08a52049a4f4fc25f72a1c33f84d04fb10bb366";

    fn hex_fixed<const N: usize>(hex: &str) -> [u8; N] {
        assert_eq!(hex.len(), N * 2);
        let mut bytes = [0u8; N];
        for (index, byte) in bytes.iter_mut().enumerate() {
            let offset = index * 2;
            *byte = u8::from_str_radix(&hex[offset..offset + 2], 16).unwrap();
        }
        bytes
    }

    fn sha256(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    #[test]
    fn ml_kem_768_keygen_matches_nist_acvp_vector() {
        let seed = hex_fixed::<ML_KEM_768_SECRET_KEY_LEN>(ML_KEM_ACVP_SEED);
        let (secret_key, public_key) = MlKem768SecretKey::from_seed_value(&seed).unwrap();
        assert_eq!(sha256(&public_key.encode()), ML_KEM_ACVP_PUBLIC_KEY_SHA256);
        assert_eq!(secret_key.to_seed().as_slice(), seed);
    }

    #[test]
    fn ml_kem_768_encapsulation_matches_nist_acvp_vector() {
        const FIXTURE: &[u8] = include_bytes!("../testdata/acvp/ml-kem-768-encap-tg2-tc26.bin");
        assert_eq!(
            FIXTURE.len(),
            ML_KEM_768_PUBLIC_KEY_LEN + ML_KEM_768_SHARED_SECRET_LEN
        );
        let public_key = MlKem768PublicKey::decode(&FIXTURE[..ML_KEM_768_PUBLIC_KEY_LEN]).unwrap();
        let message: [u8; ML_KEM_768_SHARED_SECRET_LEN] =
            FIXTURE[ML_KEM_768_PUBLIC_KEY_LEN..].try_into().unwrap();
        let (ciphertext, shared_secret) = public_key.encapsulate_with_message(&message);
        assert_eq!(sha256(&ciphertext.encode()), ML_KEM_ENCAP_CIPHERTEXT_SHA256);
        assert_eq!(
            sha256(shared_secret.expose_secret()),
            ML_KEM_ENCAP_SHARED_SECRET_SHA256
        );
    }

    #[test]
    fn ml_kem_768_round_trip_and_implicit_rejection() {
        let (secret_key, public_key) = MlKem768SecretKey::generate().unwrap();
        let (ciphertext, sender_secret) = public_key.encapsulate().unwrap();
        let receiver_secret = secret_key.decapsulate(&ciphertext);
        assert_eq!(
            sender_secret.expose_secret(),
            receiver_secret.expose_secret()
        );

        let mut tampered_bytes = ciphertext.encode();
        tampered_bytes[0] ^= 1;
        let tampered = MlKem768Ciphertext::decode(&tampered_bytes).unwrap();
        let rejected_secret = secret_key.decapsulate(&tampered);
        assert_ne!(
            receiver_secret.expose_secret(),
            rejected_secret.expose_secret()
        );
    }

    #[test]
    fn ml_dsa_65_keygen_matches_nist_acvp_vector() {
        let seed = hex_fixed::<ML_DSA_65_SECRET_KEY_LEN>(ML_DSA_ACVP_SEED);
        let (signing_key, verifying_key) = MlDsa65SigningKey::from_seed_value(&seed).unwrap();
        assert_eq!(
            sha256(&verifying_key.encode()),
            ML_DSA_ACVP_PUBLIC_KEY_SHA256
        );
        assert_eq!(signing_key.to_seed().as_slice(), seed);
    }

    #[test]
    fn ml_dsa_65_siggen_matches_nist_acvp_vector() {
        const FIXTURE: &[u8] = include_bytes!("../testdata/acvp/ml-dsa-65-siggen-tg3-tc21.bin");
        const EXPANDED_KEY_LEN: usize = 4032;
        const MESSAGE_LEN: usize = 6333;
        assert_eq!(
            FIXTURE.len(),
            EXPANDED_KEY_LEN + MESSAGE_LEN + ML_DSA_65_SIGNATURE_LEN
        );
        let expanded_bytes =
            ml_dsa::ExpandedSigningKeyBytes::<MlDsa65>::try_from(&FIXTURE[..EXPANDED_KEY_LEN])
                .unwrap();
        #[allow(deprecated)]
        let signing_key = ExpandedSigningKey::<MlDsa65>::from_expanded(&expanded_bytes);
        let message = &FIXTURE[EXPANDED_KEY_LEN..EXPANDED_KEY_LEN + MESSAGE_LEN];
        let expected_signature = &FIXTURE[EXPANDED_KEY_LEN + MESSAGE_LEN..];
        let signature = signing_key.sign_internal(&[message], &ml_dsa::B32::default());
        assert!(signing_key
            .verifying_key()
            .verify_internal(message, &signature));
        assert_eq!(
            sha256(signature.encode().as_slice()),
            ML_DSA_SIGNATURE_SHA256
        );
        assert_eq!(signature.encode().as_slice(), expected_signature);
    }

    #[test]
    fn ml_dsa_65_round_trip_and_negative_cases() {
        let (signing_key, verifying_key) = MlDsa65SigningKey::generate().unwrap();
        let message = b"fuckipv6-v2 pq primitive adapter test";
        let signature = signing_key.sign(message).unwrap();
        verifying_key.verify(message, &signature).unwrap();
        assert!(matches!(
            verifying_key.verify(b"different message", &signature),
            Err(PqError::SignatureVerificationFailed)
        ));

        let mut tampered = signature.encode();
        tampered[0] ^= 1;
        let tampered = MlDsa65Signature::decode(&tampered).unwrap();
        assert!(matches!(
            verifying_key.verify(message, &tampered),
            Err(PqError::SignatureVerificationFailed)
        ));
    }

    #[test]
    fn fixed_size_decoders_reject_truncation_and_trailing_bytes() {
        let kem_public = [0u8; ML_KEM_768_PUBLIC_KEY_LEN];
        assert!(matches!(
            MlKem768PublicKey::decode(&kem_public[..ML_KEM_768_PUBLIC_KEY_LEN - 1]),
            Err(PqError::InvalidLength { .. })
        ));
        let mut kem_public_extra = vec![0u8; ML_KEM_768_PUBLIC_KEY_LEN + 1];
        assert!(matches!(
            MlKem768PublicKey::decode(&kem_public_extra),
            Err(PqError::InvalidLength { .. })
        ));
        kem_public_extra.pop();

        let ciphertext = [0u8; ML_KEM_768_CIPHERTEXT_LEN];
        assert!(matches!(
            MlKem768Ciphertext::decode(&ciphertext[..ML_KEM_768_CIPHERTEXT_LEN - 1]),
            Err(PqError::InvalidLength { .. })
        ));
        assert!(matches!(
            MlKem768Ciphertext::decode(&[0u8; ML_KEM_768_CIPHERTEXT_LEN + 1]),
            Err(PqError::InvalidLength { .. })
        ));

        let seed = [0u8; ML_KEM_768_SECRET_KEY_LEN];
        assert!(matches!(
            MlKem768SecretKey::from_seed(&seed[..ML_KEM_768_SECRET_KEY_LEN - 1]),
            Err(PqError::InvalidLength { .. })
        ));
        assert!(matches!(
            MlKem768SecretKey::from_seed(&[0u8; ML_KEM_768_SECRET_KEY_LEN + 1]),
            Err(PqError::InvalidLength { .. })
        ));

        let dsa_public = [0u8; ML_DSA_65_PUBLIC_KEY_LEN];
        assert!(matches!(
            MlDsa65VerifyingKey::decode(&dsa_public[..ML_DSA_65_PUBLIC_KEY_LEN - 1]),
            Err(PqError::InvalidLength { .. })
        ));
        assert!(matches!(
            MlDsa65VerifyingKey::decode(&[0u8; ML_DSA_65_PUBLIC_KEY_LEN + 1]),
            Err(PqError::InvalidLength { .. })
        ));

        let dsa_seed = [0u8; ML_DSA_65_SECRET_KEY_LEN];
        assert!(matches!(
            MlDsa65SigningKey::from_seed(&dsa_seed[..ML_DSA_65_SECRET_KEY_LEN - 1]),
            Err(PqError::InvalidLength { .. })
        ));
        assert!(matches!(
            MlDsa65SigningKey::from_seed(&[0u8; ML_DSA_65_SECRET_KEY_LEN + 1]),
            Err(PqError::InvalidLength { .. })
        ));

        let signature = [0u8; ML_DSA_65_SIGNATURE_LEN];
        assert!(matches!(
            MlDsa65Signature::decode(&signature[..ML_DSA_65_SIGNATURE_LEN - 1]),
            Err(PqError::InvalidLength { .. })
        ));
        assert!(matches!(
            MlDsa65Signature::decode(&[0u8; ML_DSA_65_SIGNATURE_LEN + 1]),
            Err(PqError::InvalidLength { .. })
        ));
    }

    #[test]
    fn secret_debug_output_is_redacted() {
        let (kem_secret, _) = MlKem768SecretKey::generate().unwrap();
        let (dsa_secret, _) = MlDsa65SigningKey::generate().unwrap();
        assert_eq!(format!("{kem_secret:?}"), "MlKem768SecretKey(<redacted>)");
        assert_eq!(format!("{dsa_secret:?}"), "MlDsa65SigningKey(<redacted>)");
    }
}
