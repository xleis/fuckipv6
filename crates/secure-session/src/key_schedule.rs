//! HKDF-SHA-512 key schedule for the v2 classic handshake profile.
//!
//! The schedule consumes a successful [`EstablishedHandshake`], including its
//! X25519 shared secret and post-signature transcript hash. It derives four
//! independent 256-bit values: one data key per direction and one key-
//! confirmation key per direction. All labels and context fields are length
//! prefixed with the same canonical encoding used by the handshake.
//!
//! Confirmation values are computed and verified by the adjacent
//! [`crate::confirmation`] module. Callers must not activate the derived data
//! keys until the required directional confirmations have been verified.

use crate::handshake::{EstablishedHandshake, HandshakeIdentity};
use crate::transcript::TranscriptHash;
use crate::{Direction, SESSION_ID_LEN};
use hkdf::Hkdf;
use sha2::{Digest, Sha512};
use thiserror::Error;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

/// 256-bit key length for both data and confirmation keys.
pub const SESSION_KEY_LEN: usize = 32;
/// Domain used to construct the HKDF-Extract salt.
pub const EXTRACT_SALT_DOMAIN: &[u8] = b"fuckipv6-v2 extract";
/// HKDF info label for initiator-to-responder data.
pub const I2R_DATA_KEY_LABEL: &[u8] = b"fuckipv6-v2 i2r key";
/// HKDF info label for responder-to-initiator data.
pub const R2I_DATA_KEY_LABEL: &[u8] = b"fuckipv6-v2 r2i key";
/// HKDF info label for the initiator's confirmation key.
pub const INITIATOR_CONFIRM_KEY_LABEL: &[u8] = b"fuckipv6-v2 confirm i";
/// HKDF info label for the responder's confirmation key.
pub const RESPONDER_CONFIRM_KEY_LABEL: &[u8] = b"fuckipv6-v2 confirm r";

/// Four independent directional keys derived from one handshake.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SessionKeys {
    initiator_to_responder: [u8; SESSION_KEY_LEN],
    responder_to_initiator: [u8; SESSION_KEY_LEN],
    initiator_confirmation: [u8; SESSION_KEY_LEN],
    responder_confirmation: [u8; SESSION_KEY_LEN],
}

impl SessionKeys {
    /// Data key used by Initiator to encrypt and Responder to decrypt.
    pub fn data_key(&self, direction: Direction) -> &[u8; SESSION_KEY_LEN] {
        match direction {
            Direction::InitiatorToResponder => &self.initiator_to_responder,
            Direction::ResponderToInitiator => &self.responder_to_initiator,
        }
    }

    /// Confirmation key owned by the Initiator.
    pub fn initiator_confirmation_key(&self) -> &[u8; SESSION_KEY_LEN] {
        &self.initiator_confirmation
    }

    /// Confirmation key owned by the Responder.
    pub fn responder_confirmation_key(&self) -> &[u8; SESSION_KEY_LEN] {
        &self.responder_confirmation
    }

    /// Explicitly erase all four derived keys.
    pub fn erase(&mut self) {
        self.zeroize();
    }
}

/// Session identity together with its zeroizing directional key material.
pub struct DerivedSession {
    pub identity: HandshakeIdentity,
    pub keys: SessionKeys,
}

/// Derive the classic profile's directional keys from a completed handshake.
///
/// `psk` is an optional 32-byte high-entropy pre-shared key. A copied PSK is
/// placed in a zeroizing buffer immediately and is not retained by the result.
/// The input [`EstablishedHandshake`] is consumed so its shared secret is
/// erased after HKDF-Extract.
pub fn derive_session_keys(
    established: EstablishedHandshake,
    psk: Option<&[u8; SESSION_KEY_LEN]>,
) -> Result<DerivedSession, KeyScheduleError> {
    let identity = established.identity;
    let transcript_hash = established.key_schedule_hash;
    let shared_secret = established.into_shared_secret();
    let keys = derive_from_parts(shared_secret, transcript_hash, identity, psk)?;
    Ok(DerivedSession { identity, keys })
}

pub(crate) fn derive_from_parts(
    shared_secret: Zeroizing<[u8; 32]>,
    transcript_hash: TranscriptHash,
    identity: HandshakeIdentity,
    psk: Option<&[u8; SESSION_KEY_LEN]>,
) -> Result<SessionKeys, KeyScheduleError> {
    let mut psk_buffer = Zeroizing::new([0u8; SESSION_KEY_LEN]);
    let psk_mode = match psk {
        Some(psk) => {
            psk_buffer.copy_from_slice(psk);
            1u8
        }
        None => 0u8,
    };
    let psk_mode = [psk_mode];

    let salt = Sha512::digest(canonical_bytes(EXTRACT_SALT_DOMAIN, &[&transcript_hash]));

    let mut ikm = Zeroizing::new(Vec::with_capacity(2 + 1 + 2 + 32 + 2 + 32));
    push_field(&mut ikm, &psk_mode);
    push_field(&mut ikm, &*psk_buffer);
    push_field(&mut ikm, &*shared_secret);

    let (mut prk_output, hkdf) = Hkdf::<Sha512>::extract(Some(&salt), &ikm);
    let mut prk = Zeroizing::new([0u8; 64]);
    prk.copy_from_slice(&prk_output);
    prk_output.zeroize();

    let initiator_to_responder = expand_key(
        &hkdf,
        I2R_DATA_KEY_LABEL,
        identity.session_id,
        identity.epoch,
    )?;
    let responder_to_initiator = expand_key(
        &hkdf,
        R2I_DATA_KEY_LABEL,
        identity.session_id,
        identity.epoch,
    )?;
    let initiator_confirmation = expand_key(
        &hkdf,
        INITIATOR_CONFIRM_KEY_LABEL,
        identity.session_id,
        identity.epoch,
    )?;
    let responder_confirmation = expand_key(
        &hkdf,
        RESPONDER_CONFIRM_KEY_LABEL,
        identity.session_id,
        identity.epoch,
    )?;

    // Keep the PRK and all intermediate secret copies alive until every expand
    // operation has completed, then explicitly erase them.
    prk.zeroize();
    ikm.zeroize();
    psk_buffer.zeroize();

    Ok(SessionKeys {
        initiator_to_responder,
        responder_to_initiator,
        initiator_confirmation,
        responder_confirmation,
    })
}

fn expand_key(
    hkdf: &Hkdf<Sha512>,
    label: &[u8],
    session_id: [u8; SESSION_ID_LEN],
    epoch: u16,
) -> Result<[u8; SESSION_KEY_LEN], KeyScheduleError> {
    let epoch = epoch.to_be_bytes();
    let info = canonical_bytes(label, &[&session_id, &epoch]);
    let mut output = Zeroizing::new([0u8; SESSION_KEY_LEN]);
    hkdf.expand(&info, &mut *output)
        .map_err(|_| KeyScheduleError::HkdfExpand)?;
    Ok(*output)
}

fn canonical_bytes(domain: &[u8], fields: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(128);
    push_field(&mut out, domain);
    for field in fields {
        push_field(&mut out, field);
    }
    out
}

fn push_field(out: &mut Vec<u8>, value: &[u8]) {
    out.extend_from_slice(&(value.len() as u16).to_be_bytes());
    out.extend_from_slice(value);
}

/// Errors returned while deriving session keys.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum KeyScheduleError {
    #[error("HKDF-SHA-512 expand failed for a fixed 32-byte output")]
    HkdfExpand,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handshake::{ClassicInitiator, ClassicResponder, Ed25519Identity};
    use crate::transcript::{ENDPOINT_CONTEXT_HASH_LEN, SUITE_CHACHA20_POLY1305};

    const NOW: u64 = 1_700_000_000;
    const ENDPOINT: [u8; ENDPOINT_CONTEXT_HASH_LEN] = [0x5a; ENDPOINT_CONTEXT_HASH_LEN];
    const SHARED: [u8; 32] = [0x11; 32];
    const KDF_TRANSCRIPT: TranscriptHash = [0x55; 64];
    const SID: [u8; SESSION_ID_LEN] = [0x22; SESSION_ID_LEN];
    const EPOCH: u16 = 7;

    fn identity_fixture() -> HandshakeIdentity {
        HandshakeIdentity {
            suite_id: SUITE_CHACHA20_POLY1305,
            session_id: SID,
            epoch: EPOCH,
            initiator_identity: [0x33; 32],
            responder_identity: [0x44; 32],
            endpoint_context_hash: ENDPOINT,
            transcript_hash: [0x66; 64],
        }
    }

    fn keys() -> SessionKeys {
        derive_from_parts(
            Zeroizing::new(SHARED),
            KDF_TRANSCRIPT,
            identity_fixture(),
            None,
        )
        .expect("derive keys")
    }

    fn established_handshakes() -> (
        crate::handshake::EstablishedHandshake,
        crate::handshake::EstablishedHandshake,
    ) {
        let initiator_id = Ed25519Identity::from_seed([0x11; 32]);
        let responder_id = Ed25519Identity::from_seed([0x22; 32]);
        let initiator = ClassicInitiator::start(
            &initiator_id,
            responder_id.verifying_key(),
            SUITE_CHACHA20_POLY1305,
            ENDPOINT,
            EPOCH,
            NOW,
        )
        .expect("start");
        let hello = initiator.hello().clone();
        let mut responder =
            ClassicResponder::new(responder_id, initiator_id.verifying_key()).expect("responder");
        let accepted = responder
            .accept(&hello, ENDPOINT, EPOCH, NOW)
            .expect("accept");
        let initiator_established = initiator.finish(&accepted.accept, NOW).expect("finish");
        (initiator_established, accepted.established)
    }

    #[test]
    fn key_schedule_matches_the_fixed_hkdf_sha512_vector() {
        let keys = keys();
        assert_eq!(
            keys.data_key(Direction::InitiatorToResponder),
            &[
                0xea, 0xcd, 0xda, 0x49, 0xaa, 0x53, 0x98, 0x15, 0xd4, 0x03, 0x6a, 0x71, 0x4c, 0x27,
                0xba, 0x09, 0x68, 0x31, 0x65, 0x5f, 0x79, 0xe5, 0x52, 0xb5, 0x32, 0x53, 0xe1, 0x68,
                0xa5, 0x8f, 0xc6, 0xae,
            ]
        );
        assert_eq!(
            keys.data_key(Direction::ResponderToInitiator),
            &[
                0xd7, 0x85, 0x72, 0xc9, 0x6d, 0x8b, 0x2c, 0x6b, 0x13, 0x57, 0x7b, 0xb8, 0xd3, 0xfc,
                0x2e, 0x4d, 0x6e, 0x65, 0xfb, 0x9a, 0x19, 0x2d, 0xe1, 0x24, 0x57, 0x2b, 0xbf, 0xa1,
                0x12, 0x91, 0x6f, 0x88,
            ]
        );
        assert_eq!(
            keys.initiator_confirmation_key(),
            &[
                0xed, 0xfc, 0x41, 0x76, 0x6f, 0xeb, 0xe2, 0x3b, 0xe4, 0xaa, 0x78, 0xc4, 0x99, 0x81,
                0x78, 0x1c, 0x67, 0x88, 0x0a, 0xc3, 0xbc, 0x76, 0x32, 0x4e, 0x52, 0x7f, 0x31, 0xec,
                0xc5, 0x22, 0x0b, 0xe7,
            ]
        );
        assert_eq!(
            keys.responder_confirmation_key(),
            &[
                0x39, 0xd0, 0xe8, 0x2d, 0x8f, 0x64, 0x3c, 0xe7, 0xf0, 0xca, 0xf4, 0x09, 0x96, 0x3d,
                0x3a, 0xa4, 0x45, 0x4d, 0xe9, 0x61, 0x33, 0x44, 0x08, 0x71, 0x4b, 0xd7, 0x52, 0x07,
                0xb5, 0x7e, 0x75, 0x83,
            ]
        );
    }

    #[test]
    fn every_key_has_a_distinct_directional_domain() {
        let keys = keys();
        let values = [
            keys.data_key(Direction::InitiatorToResponder),
            keys.data_key(Direction::ResponderToInitiator),
            keys.initiator_confirmation_key(),
            keys.responder_confirmation_key(),
        ];
        for (index, left) in values.iter().enumerate() {
            assert_ne!(**left, [0; SESSION_KEY_LEN]);
            for right in &values[index + 1..] {
                assert_ne!(left, right);
            }
        }
    }

    #[test]
    fn psk_mode_transcript_sid_and_epoch_are_bound() {
        let base = keys();
        let psk = [0x77; SESSION_KEY_LEN];
        let with_psk = derive_from_parts(
            Zeroizing::new(SHARED),
            KDF_TRANSCRIPT,
            identity_fixture(),
            Some(&psk),
        )
        .expect("psk keys");
        assert_ne!(
            base.data_key(Direction::InitiatorToResponder),
            with_psk.data_key(Direction::InitiatorToResponder)
        );

        let changed_transcript =
            derive_from_parts(Zeroizing::new(SHARED), [0x56; 64], identity_fixture(), None)
                .expect("transcript keys");
        assert_ne!(
            base.data_key(Direction::InitiatorToResponder),
            changed_transcript.data_key(Direction::InitiatorToResponder)
        );

        let mut changed_identity = identity_fixture();
        changed_identity.session_id[0] ^= 1;
        let changed_sid = derive_from_parts(
            Zeroizing::new(SHARED),
            KDF_TRANSCRIPT,
            changed_identity,
            None,
        )
        .expect("sid keys");
        assert_ne!(
            base.data_key(Direction::InitiatorToResponder),
            changed_sid.data_key(Direction::InitiatorToResponder)
        );

        let mut changed_epoch_identity = identity_fixture();
        changed_epoch_identity.epoch += 1;
        let changed_epoch = derive_from_parts(
            Zeroizing::new(SHARED),
            KDF_TRANSCRIPT,
            changed_epoch_identity,
            None,
        )
        .expect("epoch keys");
        assert_ne!(
            base.data_key(Direction::InitiatorToResponder),
            changed_epoch.data_key(Direction::InitiatorToResponder)
        );
    }

    #[test]
    fn both_handshake_endpoints_derive_identical_keys() {
        let (initiator_handshake, responder_handshake) = established_handshakes();
        let initiator = derive_session_keys(initiator_handshake, None).expect("initiator keys");
        let responder = derive_session_keys(responder_handshake, None).expect("responder keys");

        assert_eq!(initiator.identity, responder.identity);
        for direction in [
            Direction::InitiatorToResponder,
            Direction::ResponderToInitiator,
        ] {
            assert_eq!(
                initiator.keys.data_key(direction),
                responder.keys.data_key(direction)
            );
        }
        assert_eq!(
            initiator.keys.initiator_confirmation_key(),
            responder.keys.initiator_confirmation_key()
        );
        assert_eq!(
            initiator.keys.responder_confirmation_key(),
            responder.keys.responder_confirmation_key()
        );
    }

    #[test]
    fn explicit_erase_zeroizes_all_output_keys() {
        let mut keys = keys();
        keys.erase();
        assert_eq!(keys.data_key(Direction::InitiatorToResponder), &[0; 32]);
        assert_eq!(keys.data_key(Direction::ResponderToInitiator), &[0; 32]);
        assert_eq!(keys.initiator_confirmation_key(), &[0; 32]);
        assert_eq!(keys.responder_confirmation_key(), &[0; 32]);
    }
}
