//! Runtime-selectable AEAD data-cipher suites for protocol v2.
//!
//! Both suites expose the same 96-bit nonce, 128-bit tag, associated-data and
//! in-place APIs. The suite is negotiated by the authenticated handshake and is
//! stored with the directional cipher state; it is never inferred from a data
//! packet.

use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce as AesNonce, Tag as AesTag};
use chacha20poly1305::{ChaCha20Poly1305, Nonce as ChaChaNonce, Tag as ChaChaTag};

use crate::transcript::{SUITE_AES_256_GCM, SUITE_CHACHA20_POLY1305};

/// Authenticated-encryption suite used by a negotiated v2 data epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataCipherSuite {
    /// AES-256-GCM, suite identifier `0x0201`.
    Aes256Gcm,
    /// ChaCha20-Poly1305, suite identifier `0x0202`.
    ChaCha20Poly1305,
}

impl DataCipherSuite {
    /// Wire identifier bound by the handshake transcript.
    pub const fn id(self) -> u16 {
        match self {
            Self::Aes256Gcm => SUITE_AES_256_GCM,
            Self::ChaCha20Poly1305 => SUITE_CHACHA20_POLY1305,
        }
    }

    /// Resolve a negotiated suite identifier.
    pub const fn from_id(id: u16) -> Option<Self> {
        match id {
            SUITE_AES_256_GCM => Some(Self::Aes256Gcm),
            SUITE_CHACHA20_POLY1305 => Some(Self::ChaCha20Poly1305),
            _ => None,
        }
    }
}

pub(crate) enum AeadCipher {
    Aes256Gcm(Box<Aes256Gcm>),
    ChaCha20Poly1305(Box<ChaCha20Poly1305>),
}

impl AeadCipher {
    pub(crate) fn new(suite: DataCipherSuite, key: &[u8; 32]) -> Self {
        match suite {
            DataCipherSuite::Aes256Gcm => Self::Aes256Gcm(Box::new(
                Aes256Gcm::new_from_slice(key).expect("AES-256-GCM requires a 32-byte key"),
            )),
            DataCipherSuite::ChaCha20Poly1305 => Self::ChaCha20Poly1305(Box::new(
                ChaCha20Poly1305::new_from_slice(key)
                    .expect("ChaCha20-Poly1305 requires a 32-byte key"),
            )),
        }
    }

    pub(crate) fn encrypt_in_place_detached(
        &self,
        nonce: &[u8; 12],
        aad: &[u8],
        buffer: &mut [u8],
    ) -> Result<[u8; 16], ()> {
        let tag = match self {
            Self::Aes256Gcm(cipher) => cipher
                .encrypt_in_place_detached(AesNonce::from_slice(nonce), aad, buffer)
                .map_err(|_| ())?,
            Self::ChaCha20Poly1305(cipher) => cipher
                .encrypt_in_place_detached(ChaChaNonce::from_slice(nonce), aad, buffer)
                .map_err(|_| ())?,
        };
        let mut encoded = [0u8; 16];
        encoded.copy_from_slice(&tag);
        Ok(encoded)
    }

    pub(crate) fn decrypt_in_place_detached(
        &self,
        nonce: &[u8; 12],
        aad: &[u8],
        buffer: &mut [u8],
        tag: &[u8; 16],
    ) -> Result<(), ()> {
        match self {
            Self::Aes256Gcm(cipher) => cipher
                .decrypt_in_place_detached(
                    AesNonce::from_slice(nonce),
                    aad,
                    buffer,
                    AesTag::from_slice(tag),
                )
                .map_err(|_| ()),
            Self::ChaCha20Poly1305(cipher) => cipher
                .decrypt_in_place_detached(
                    ChaChaNonce::from_slice(nonce),
                    aad,
                    buffer,
                    ChaChaTag::from_slice(tag),
                )
                .map_err(|_| ()),
        }
    }
}
