//! X-Wing hybrid KEM as specified by `draft-connolly-cfrg-xwing-kem-11`.
//!
//! X-Wing combines ML-KEM-768 with X25519 and hashes both shared secrets and
//! both X25519 public values with SHA3-256. The implementation uses the
//! RustCrypto ML-KEM adapter in [`crate::pq`] and x25519-dalek. It deliberately
//! fixes the draft revision and byte layout; any draft revision change requires
//! a new protocol suite identifier.

use crate::pq::{
    MlKem768Ciphertext, MlKem768PublicKey, MlKem768SecretKey, MlKem768SharedSecret, PqError,
    ML_KEM_768_CIPHERTEXT_LEN, ML_KEM_768_PUBLIC_KEY_LEN,
};
use sha3::digest::{ExtendableOutput, Update, XofReader};
use sha3::{Digest, Sha3_256, Shake256};
use std::fmt;
use thiserror::Error;
use x25519_dalek::{
    PublicKey as X25519PublicKey, SharedSecret as X25519SharedSecret, StaticSecret,
};
use zeroize::Zeroizing;

/// X-Wing decapsulation-key seed length.
pub const XWING_SEED_LEN: usize = 32;
/// X-Wing encapsulation-key length: ML-KEM-768 public key followed by X25519.
pub const XWING_ENCAPSULATION_KEY_LEN: usize = ML_KEM_768_PUBLIC_KEY_LEN + X25519_PUBLIC_KEY_LEN;
/// X-Wing ciphertext length: ML-KEM-768 ciphertext followed by X25519.
pub const XWING_CIPHERTEXT_LEN: usize = ML_KEM_768_CIPHERTEXT_LEN + X25519_PUBLIC_KEY_LEN;
/// X-Wing shared-secret length.
pub const XWING_SHARED_SECRET_LEN: usize = 32;

const X25519_PUBLIC_KEY_LEN: usize = 32;
const XWING_EXPANDED_SEED_LEN: usize = 96;
const XWING_ENCAPSULATION_SEED_LEN: usize = 64;
const XWING_LABEL: &[u8; 6] = b"\\.//^\\";

/// Errors returned by the X-Wing construction.
#[derive(Debug, Error)]
pub enum XWingError {
    /// An underlying ML-KEM operation or fixed-size encoding failed.
    #[error(transparent)]
    Pq(#[from] PqError),
    /// X25519 produced the all-zero non-contributory shared secret.
    #[error("X-Wing X25519 exchange was non-contributory")]
    NonContributory,
}

/// An X-Wing encapsulation (public) key in draft-11 wire encoding.
#[derive(Clone, PartialEq, Eq)]
pub struct XWingEncapsulationKey {
    ml_kem: MlKem768PublicKey,
    x25519: [u8; X25519_PUBLIC_KEY_LEN],
}

impl XWingEncapsulationKey {
    /// Decode an exact 1216-byte X-Wing encapsulation key.
    ///
    /// The ML-KEM component receives the FIPS 203 encapsulation-key check, and
    /// the X25519 component must not be all zero.
    pub fn decode(bytes: &[u8]) -> Result<Self, XWingError> {
        if bytes.len() != XWING_ENCAPSULATION_KEY_LEN {
            return Err(PqError::InvalidLength {
                item: "X-Wing encapsulation key",
                expected: XWING_ENCAPSULATION_KEY_LEN,
                actual: bytes.len(),
            }
            .into());
        }
        let ml_kem = MlKem768PublicKey::decode(&bytes[..ML_KEM_768_PUBLIC_KEY_LEN])?;
        let mut x25519 = [0u8; X25519_PUBLIC_KEY_LEN];
        x25519.copy_from_slice(&bytes[ML_KEM_768_PUBLIC_KEY_LEN..]);
        if x25519 == [0u8; X25519_PUBLIC_KEY_LEN] {
            return Err(XWingError::NonContributory);
        }
        Ok(Self { ml_kem, x25519 })
    }

    /// Encode this key as ML-KEM-768 public key followed by X25519 public key.
    pub fn encode(&self) -> [u8; XWING_ENCAPSULATION_KEY_LEN] {
        let mut encoded = [0u8; XWING_ENCAPSULATION_KEY_LEN];
        encoded[..ML_KEM_768_PUBLIC_KEY_LEN].copy_from_slice(&self.ml_kem.encode());
        encoded[ML_KEM_768_PUBLIC_KEY_LEN..].copy_from_slice(&self.x25519);
        encoded
    }

    /// Generate a fresh X-Wing shared secret and ciphertext from the OS CSPRNG.
    pub fn encapsulate(&self) -> Result<(XWingCiphertext, XWingSharedSecret), XWingError> {
        let mut seed = Zeroizing::new([0u8; XWING_ENCAPSULATION_SEED_LEN]);
        getrandom::fill(seed.as_mut_slice()).map_err(|_| PqError::Random)?;
        self.encapsulate_derand(&seed)
    }

    /// Deterministically encapsulate using a 64-byte X-Wing encapsulation seed.
    ///
    /// This is intended for interoperability tests and must not be used with a
    /// reused seed in production.
    pub fn encapsulate_derand(
        &self,
        seed: &[u8; XWING_ENCAPSULATION_SEED_LEN],
    ) -> Result<(XWingCiphertext, XWingSharedSecret), XWingError> {
        let mut ml_kem_message = Zeroizing::new([0u8; 32]);
        ml_kem_message.copy_from_slice(&seed[..32]);
        let (ml_kem_ciphertext, ml_kem_secret) =
            self.ml_kem.encapsulate_with_message(&ml_kem_message);

        let mut ephemeral_bytes = Zeroizing::new([0u8; X25519_PUBLIC_KEY_LEN]);
        ephemeral_bytes.copy_from_slice(&seed[32..]);
        let ephemeral_secret = StaticSecret::from(*ephemeral_bytes);
        let ephemeral_public = X25519PublicKey::from(&ephemeral_secret);
        let peer_public = X25519PublicKey::from(self.x25519);
        let x25519_secret = ephemeral_secret.diffie_hellman(&peer_public);
        let x25519_bytes = contributory_secret(&x25519_secret)?;

        let ciphertext = XWingCiphertext {
            ml_kem: ml_kem_ciphertext,
            x25519: ephemeral_public.to_bytes(),
        };
        let shared_secret = combine(
            &ml_kem_secret,
            &x25519_bytes,
            &ciphertext.x25519,
            &self.x25519,
        );
        Ok((ciphertext, shared_secret))
    }
}

impl fmt::Debug for XWingEncapsulationKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("XWingEncapsulationKey")
            .field("ml_kem", &self.ml_kem)
            .field("x25519", &self.x25519)
            .finish()
    }
}

/// An X-Wing decapsulation key derived from the draft-11 32-byte seed.
///
/// The original seed is not retained after expansion. The expanded ML-KEM and
/// X25519 secret components are held by zeroizing container types.
pub struct XWingDecapsulationKey {
    ml_kem: MlKem768SecretKey,
    ml_kem_public: MlKem768PublicKey,
    x25519: StaticSecret,
    x25519_public: [u8; X25519_PUBLIC_KEY_LEN],
}

impl XWingDecapsulationKey {
    /// Derive an X-Wing keypair from an exact 32-byte seed.
    pub fn from_seed(seed: &[u8]) -> Result<Self, XWingError> {
        if seed.len() != XWING_SEED_LEN {
            return Err(PqError::InvalidLength {
                item: "X-Wing decapsulation seed",
                expected: XWING_SEED_LEN,
                actual: seed.len(),
            }
            .into());
        }

        let mut expanded = Zeroizing::new([0u8; XWING_EXPANDED_SEED_LEN]);
        let mut shake = Shake256::default();
        shake.update(seed);
        shake.finalize_xof().read(expanded.as_mut());

        let ml_kem = MlKem768SecretKey::from_seed(&expanded[..64])?;
        let mut x25519_bytes = [0u8; X25519_PUBLIC_KEY_LEN];
        x25519_bytes.copy_from_slice(&expanded[64..]);
        let ml_kem_public = ml_kem.encapsulation_key();
        let x25519 = StaticSecret::from(x25519_bytes);
        let x25519_public = X25519PublicKey::from(&x25519).to_bytes();
        Ok(Self {
            ml_kem,
            ml_kem_public,
            x25519,
            x25519_public,
        })
    }

    /// Generate a fresh X-Wing keypair using the OS CSPRNG.
    pub fn generate() -> Result<Self, XWingError> {
        let mut seed = Zeroizing::new([0u8; XWING_SEED_LEN]);
        getrandom::fill(seed.as_mut_slice()).map_err(|_| PqError::Random)?;
        Self::from_seed(seed.as_slice())
    }

    /// Return the corresponding X-Wing encapsulation key.
    pub fn encapsulation_key(&self) -> XWingEncapsulationKey {
        XWingEncapsulationKey {
            ml_kem: self.ml_kem_public.clone(),
            x25519: self.x25519_public,
        }
    }

    /// Decapsulate an exact X-Wing ciphertext.
    pub fn decapsulate(
        &self,
        ciphertext: &XWingCiphertext,
    ) -> Result<XWingSharedSecret, XWingError> {
        let ml_kem_secret = self.ml_kem.decapsulate(&ciphertext.ml_kem);
        let peer_public = X25519PublicKey::from(ciphertext.x25519);
        let x25519_secret = self.x25519.diffie_hellman(&peer_public);
        let x25519_bytes = contributory_secret(&x25519_secret)?;
        Ok(combine(
            &ml_kem_secret,
            &x25519_bytes,
            &ciphertext.x25519,
            &self.x25519_public,
        ))
    }
}

impl fmt::Debug for XWingDecapsulationKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("XWingDecapsulationKey(<redacted>)")
    }
}

/// An X-Wing ciphertext in draft-11 wire encoding.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct XWingCiphertext {
    ml_kem: MlKem768Ciphertext,
    x25519: [u8; X25519_PUBLIC_KEY_LEN],
}

impl XWingCiphertext {
    /// Decode an exact 1120-byte X-Wing ciphertext.
    pub fn decode(bytes: &[u8]) -> Result<Self, XWingError> {
        if bytes.len() != XWING_CIPHERTEXT_LEN {
            return Err(PqError::InvalidLength {
                item: "X-Wing ciphertext",
                expected: XWING_CIPHERTEXT_LEN,
                actual: bytes.len(),
            }
            .into());
        }
        let ml_kem = MlKem768Ciphertext::decode(&bytes[..ML_KEM_768_CIPHERTEXT_LEN])?;
        let mut x25519 = [0u8; X25519_PUBLIC_KEY_LEN];
        x25519.copy_from_slice(&bytes[ML_KEM_768_CIPHERTEXT_LEN..]);
        if x25519 == [0u8; X25519_PUBLIC_KEY_LEN] {
            return Err(XWingError::NonContributory);
        }
        Ok(Self { ml_kem, x25519 })
    }

    /// Encode this ciphertext as ML-KEM-768 ciphertext followed by X25519.
    pub fn encode(self) -> [u8; XWING_CIPHERTEXT_LEN] {
        let mut encoded = [0u8; XWING_CIPHERTEXT_LEN];
        encoded[..ML_KEM_768_CIPHERTEXT_LEN].copy_from_slice(&self.ml_kem.encode());
        encoded[ML_KEM_768_CIPHERTEXT_LEN..].copy_from_slice(&self.x25519);
        encoded
    }
}

impl fmt::Debug for XWingCiphertext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("XWingCiphertext")
            .field(&XWING_CIPHERTEXT_LEN)
            .finish()
    }
}

/// A zeroizing X-Wing shared secret.
pub struct XWingSharedSecret {
    bytes: Zeroizing<[u8; XWING_SHARED_SECRET_LEN]>,
}

impl XWingSharedSecret {
    /// Borrow the shared secret for an authenticated key schedule.
    pub fn expose_secret(&self) -> &[u8; XWING_SHARED_SECRET_LEN] {
        &self.bytes
    }
}

impl fmt::Debug for XWingSharedSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("XWingSharedSecret(<redacted>)")
    }
}

fn contributory_secret(secret: &X25519SharedSecret) -> Result<[u8; 32], XWingError> {
    if !secret.was_contributory() {
        return Err(XWingError::NonContributory);
    }
    Ok(secret.to_bytes())
}

fn combine(
    ml_kem_secret: &MlKem768SharedSecret,
    x25519_secret: &[u8; XWING_SHARED_SECRET_LEN],
    x25519_ciphertext: &[u8; X25519_PUBLIC_KEY_LEN],
    x25519_public: &[u8; X25519_PUBLIC_KEY_LEN],
) -> XWingSharedSecret {
    let mut hash = Sha3_256::new();
    Digest::update(&mut hash, ml_kem_secret.expose_secret());
    Digest::update(&mut hash, x25519_secret);
    Digest::update(&mut hash, x25519_ciphertext);
    Digest::update(&mut hash, x25519_public);
    Digest::update(&mut hash, XWING_LABEL);
    let combined = hash.finalize();
    let mut bytes = Zeroizing::new([0u8; XWING_SHARED_SECRET_LEN]);
    bytes.copy_from_slice(&combined);
    XWingSharedSecret { bytes }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const VECTORS: &str = include_str!("../../testdata/xwing-draft11-vectors.txt");

    fn parse_vectors(input: &str) -> Vec<BTreeMap<&'static str, Vec<u8>>> {
        let labels = ["seed", "sk", "pk", "eseed", "ct", "ss"];
        let mut records = Vec::new();
        let mut record: BTreeMap<&'static str, String> = BTreeMap::new();
        let mut current: Option<&'static str> = None;

        for line in input.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let mut matched = false;
            for label in labels {
                if trimmed == label {
                    current = Some(label);
                    matched = true;
                    break;
                }
                if let Some(rest) = trimmed.strip_prefix(label) {
                    if rest.as_bytes().first().is_some_and(u8::is_ascii_whitespace) {
                        if label == "seed" && !record.is_empty() {
                            records.push(record);
                            record = BTreeMap::new();
                        }
                        record.insert(label, rest.trim().to_owned());
                        current = None;
                        matched = true;
                        break;
                    }
                }
            }
            if matched {
                continue;
            }

            let label = current.expect("vector continuation must follow a field label");
            record.entry(label).or_default().push_str(trimmed);
        }
        if !record.is_empty() {
            records.push(record);
        }

        records
            .into_iter()
            .map(|record| {
                assert_eq!(record.len(), labels.len());
                record
                    .into_iter()
                    .map(|(label, hex)| (label, decode_hex(&hex)))
                    .collect()
            })
            .collect()
    }

    fn decode_hex(hex: &str) -> Vec<u8> {
        assert_eq!(hex.len() % 2, 0);
        (0..hex.len())
            .step_by(2)
            .map(|offset| u8::from_str_radix(&hex[offset..offset + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn matches_all_draft11_fixed_vectors() {
        let vectors = parse_vectors(VECTORS);
        assert_eq!(vectors.len(), 3);
        for vector in vectors {
            let seed = &vector["seed"];
            let encapsulation_seed = &vector["eseed"];
            assert_eq!(seed.len(), XWING_SEED_LEN);
            assert_eq!(vector["sk"], *seed);
            assert_eq!(encapsulation_seed.len(), XWING_ENCAPSULATION_SEED_LEN);

            let decapsulation = XWingDecapsulationKey::from_seed(seed).unwrap();
            let encapsulation = decapsulation.encapsulation_key();
            assert_eq!(encapsulation.encode().as_slice(), vector["pk"]);

            let deterministic_seed: &[u8; XWING_ENCAPSULATION_SEED_LEN] =
                encapsulation_seed.as_slice().try_into().unwrap();
            let (ciphertext, sender_secret) = encapsulation
                .encapsulate_derand(deterministic_seed)
                .unwrap();
            assert_eq!(ciphertext.encode().as_slice(), vector["ct"]);
            assert_eq!(sender_secret.expose_secret(), vector["ss"].as_slice());

            let receiver_secret = decapsulation.decapsulate(&ciphertext).unwrap();
            assert_eq!(
                receiver_secret.expose_secret(),
                sender_secret.expose_secret()
            );
            assert_eq!(receiver_secret.expose_secret(), vector["ss"].as_slice());
        }
    }

    #[test]
    fn round_trip_randomized_encapsulation_succeeds() {
        let decapsulation = XWingDecapsulationKey::generate().unwrap();
        let encapsulation = decapsulation.encapsulation_key();
        let (ciphertext, sender_secret) = encapsulation.encapsulate().unwrap();
        let receiver_secret = decapsulation.decapsulate(&ciphertext).unwrap();
        assert_eq!(
            receiver_secret.expose_secret(),
            sender_secret.expose_secret()
        );
    }

    #[test]
    fn modified_ciphertext_does_not_derive_the_sender_secret() {
        let decapsulation = XWingDecapsulationKey::generate().unwrap();
        let (ciphertext, sender_secret) = decapsulation.encapsulation_key().encapsulate().unwrap();
        let mut encoded = ciphertext.encode();
        encoded[0] ^= 1;
        let modified = XWingCiphertext::decode(&encoded).unwrap();
        let receiver_secret = decapsulation.decapsulate(&modified).unwrap();
        assert_ne!(
            receiver_secret.expose_secret(),
            sender_secret.expose_secret()
        );
    }

    #[test]
    fn strict_lengths_and_non_contributory_values_fail_closed() {
        let decapsulation = XWingDecapsulationKey::generate().unwrap();
        let encapsulation = decapsulation.encapsulation_key().encode();
        assert!(XWingEncapsulationKey::decode(&encapsulation[..encapsulation.len() - 1]).is_err());
        assert!(XWingEncapsulationKey::decode(&[encapsulation.as_slice(), &[0]].concat()).is_err());

        let mut zero_x25519 = encapsulation;
        zero_x25519[XWING_ENCAPSULATION_KEY_LEN - X25519_PUBLIC_KEY_LEN..].fill(0);
        assert!(matches!(
            XWingEncapsulationKey::decode(&zero_x25519),
            Err(XWingError::NonContributory)
        ));

        assert!(XWingCiphertext::decode(&[0u8; XWING_CIPHERTEXT_LEN - 1]).is_err());
        let mut zero_ciphertext_x25519 = [0u8; XWING_CIPHERTEXT_LEN];
        zero_ciphertext_x25519[ML_KEM_768_CIPHERTEXT_LEN - 1] = 1;
        assert!(matches!(
            XWingCiphertext::decode(&zero_ciphertext_x25519),
            Err(XWingError::NonContributory)
        ));
        assert!(XWingDecapsulationKey::from_seed(&[0u8; XWING_SEED_LEN - 1]).is_err());
    }

    #[test]
    fn secrets_and_private_keys_are_redacted_from_debug_output() {
        let decapsulation = XWingDecapsulationKey::generate().unwrap();
        let (_, secret) = decapsulation.encapsulation_key().encapsulate().unwrap();
        assert_eq!(
            format!("{decapsulation:?}"),
            "XWingDecapsulationKey(<redacted>)"
        );
        assert_eq!(format!("{secret:?}"), "XWingSharedSecret(<redacted>)");
    }
}
