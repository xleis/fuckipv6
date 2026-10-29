//! Canonical UDP wire encoding for the experimental classic v2 handshake.
//!
//! Handshake control frames are intentionally separate from encrypted data
//! packets. They use a fixed magic prefix so the client can route them before
//! attempting postcard decoding of legacy datagrams. Every frame has an exact
//! body length and rejects unsupported versions, kinds, flags, reserved bytes,
//! truncation, and trailing data.

use crate::confirmation::{ConfirmationTag, CONFIRMATION_LEN};
use crate::handshake::{
    Accept, Hello, ED25519_PUBLIC_KEY_LEN, ED25519_SIGNATURE_LEN, HANDSHAKE_NONCE_LEN,
    X25519_PUBLIC_KEY_LEN,
};
use crate::transcript::{ENDPOINT_CONTEXT_HASH_LEN, IDENTITY_ID_LEN};
use crate::{PROTOCOL_VERSION, SESSION_ID_LEN};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Four-byte prefix identifying a handshake control datagram.
pub const CONTROL_MAGIC: [u8; 4] = *b"FKV2";
/// Fixed handshake control-frame header length.
pub const CONTROL_HEADER_LEN: usize = 12;
/// Canonical encoded `HELLO` body length.
pub const HELLO_BODY_LEN: usize = 1
    + 1
    + 2
    + (2 * IDENTITY_ID_LEN)
    + X25519_PUBLIC_KEY_LEN
    + HANDSHAKE_NONCE_LEN
    + 8
    + ENDPOINT_CONTEXT_HASH_LEN
    + ED25519_SIGNATURE_LEN;
/// Canonical encoded `ACCEPT` plus responder confirmation body length.
pub const ACCEPT_BODY_LEN: usize = 1
    + 1
    + 2
    + SESSION_ID_LEN
    + 2
    + (2 * IDENTITY_ID_LEN)
    + (2 * X25519_PUBLIC_KEY_LEN)
    + (2 * HANDSHAKE_NONCE_LEN)
    + 8
    + 8
    + ENDPOINT_CONTEXT_HASH_LEN
    + ED25519_SIGNATURE_LEN
    + CONFIRMATION_LEN;
/// Canonical encoded initiator confirmation body length.
pub const CONFIRM_BODY_LEN: usize = SESSION_ID_LEN + CONFIRMATION_LEN;
/// Largest accepted handshake control frame.
pub const MAX_CONTROL_FRAME_LEN: usize = CONTROL_HEADER_LEN + ACCEPT_BODY_LEN;

const HELLO_KIND: u8 = 0x01;
const ACCEPT_KIND: u8 = 0x02;
const CONFIRM_KIND: u8 = 0x03;

/// Recognized handshake control-frame kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlKind {
    Hello,
    Accept,
    Confirm,
}

impl ControlKind {
    const fn wire_value(self) -> u8 {
        match self {
            Self::Hello => HELLO_KIND,
            Self::Accept => ACCEPT_KIND,
            Self::Confirm => CONFIRM_KIND,
        }
    }

    const fn body_len(self) -> usize {
        match self {
            Self::Hello => HELLO_BODY_LEN,
            Self::Accept => ACCEPT_BODY_LEN,
            Self::Confirm => CONFIRM_BODY_LEN,
        }
    }

    fn from_wire(value: u8) -> Result<Self, ControlCodecError> {
        match value {
            HELLO_KIND => Ok(Self::Hello),
            ACCEPT_KIND => Ok(Self::Accept),
            CONFIRM_KIND => Ok(Self::Confirm),
            other => Err(ControlCodecError::UnknownKind(other)),
        }
    }
}

/// A decoded handshake control frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlMessage {
    Hello(Hello),
    Accept(AcceptFrame),
    Confirm(ConfirmFrame),
}

/// Signed responder acceptance plus the responder's key-confirmation tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptFrame {
    pub accept: Accept,
    pub responder_confirmation: ConfirmationTag,
}

/// Initiator key confirmation bound to the negotiated session identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfirmFrame {
    pub session_id: [u8; SESSION_ID_LEN],
    pub initiator_confirmation: ConfirmationTag,
}

/// Errors returned by the handshake control-frame parser.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ControlCodecError {
    #[error("handshake control frame is shorter than its fixed header")]
    Truncated,
    #[error("handshake control frame exceeds the {max}-byte limit")]
    Oversized { max: usize },
    #[error("invalid handshake control magic")]
    InvalidMagic,
    #[error("unsupported handshake control version {found}; expected {expected}")]
    UnsupportedVersion { found: u8, expected: u8 },
    #[error("unknown handshake control frame kind {0:#04x}")]
    UnknownKind(u8),
    #[error("handshake control frame has non-zero flags or reserved bytes")]
    NonZeroReserved,
    #[error("handshake control body length is {declared}, expected {expected}")]
    InvalidBodyLength { declared: usize, expected: usize },
    #[error("handshake control frame has {trailing} trailing byte(s)")]
    TrailingBytes { trailing: usize },
    #[error("handshake public key must be exactly {expected} bytes")]
    InvalidPublicKeyLength { expected: usize },
}

impl Hello {
    /// Encode this `HELLO` into a canonical handshake control frame.
    pub fn encode_control(&self) -> Vec<u8> {
        let mut out = new_frame(ControlKind::Hello);
        out.push(self.version);
        out.push(self.profile);
        out.extend_from_slice(&self.suite_id.to_be_bytes());
        out.extend_from_slice(&self.initiator_identity);
        out.extend_from_slice(&self.responder_identity);
        out.extend_from_slice(&self.initiator_ephemeral_public);
        out.extend_from_slice(&self.initiator_nonce);
        out.extend_from_slice(&self.issued_at.to_be_bytes());
        out.extend_from_slice(&self.endpoint_context_hash);
        out.extend_from_slice(&self.signature);
        out
    }

    /// Decode an exact canonical handshake control frame as `HELLO`.
    pub fn decode_control(bytes: &[u8]) -> Result<Self, ControlCodecError> {
        let body = frame_body(bytes, ControlKind::Hello)?;
        let mut cursor = Cursor::new(body);
        let version = cursor.u8();
        let profile = cursor.u8();
        let suite_id = cursor.u16();
        let initiator_identity = cursor.array::<IDENTITY_ID_LEN>();
        let responder_identity = cursor.array::<IDENTITY_ID_LEN>();
        let initiator_ephemeral_public = cursor.array::<X25519_PUBLIC_KEY_LEN>();
        let initiator_nonce = cursor.array::<HANDSHAKE_NONCE_LEN>();
        let issued_at = cursor.u64();
        let endpoint_context_hash = cursor.array::<ENDPOINT_CONTEXT_HASH_LEN>();
        let signature = cursor.array::<ED25519_SIGNATURE_LEN>();
        debug_assert!(cursor.is_finished());
        Ok(Self {
            version,
            profile,
            suite_id,
            initiator_identity,
            responder_identity,
            initiator_ephemeral_public,
            initiator_nonce,
            issued_at,
            endpoint_context_hash,
            signature,
        })
    }
}

impl AcceptFrame {
    /// Encode this acceptance and responder confirmation into a canonical frame.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = new_frame(ControlKind::Accept);
        let accept = &self.accept;
        out.push(accept.version);
        out.push(accept.profile);
        out.extend_from_slice(&accept.suite_id.to_be_bytes());
        out.extend_from_slice(&accept.session_id);
        out.extend_from_slice(&accept.epoch.to_be_bytes());
        out.extend_from_slice(&accept.initiator_identity);
        out.extend_from_slice(&accept.responder_identity);
        out.extend_from_slice(&accept.initiator_ephemeral_public);
        out.extend_from_slice(&accept.responder_ephemeral_public);
        out.extend_from_slice(&accept.initiator_nonce);
        out.extend_from_slice(&accept.responder_nonce);
        out.extend_from_slice(&accept.hello_issued_at.to_be_bytes());
        out.extend_from_slice(&accept.issued_at.to_be_bytes());
        out.extend_from_slice(&accept.endpoint_context_hash);
        out.extend_from_slice(&accept.signature);
        out.extend_from_slice(self.responder_confirmation.as_bytes());
        out
    }

    fn decode(body: &[u8]) -> Self {
        let mut cursor = Cursor::new(body);
        let version = cursor.u8();
        let profile = cursor.u8();
        let suite_id = cursor.u16();
        let session_id = cursor.array::<SESSION_ID_LEN>();
        let epoch = cursor.u16();
        let initiator_identity = cursor.array::<IDENTITY_ID_LEN>();
        let responder_identity = cursor.array::<IDENTITY_ID_LEN>();
        let initiator_ephemeral_public = cursor.array::<X25519_PUBLIC_KEY_LEN>();
        let responder_ephemeral_public = cursor.array::<X25519_PUBLIC_KEY_LEN>();
        let initiator_nonce = cursor.array::<HANDSHAKE_NONCE_LEN>();
        let responder_nonce = cursor.array::<HANDSHAKE_NONCE_LEN>();
        let hello_issued_at = cursor.u64();
        let issued_at = cursor.u64();
        let endpoint_context_hash = cursor.array::<ENDPOINT_CONTEXT_HASH_LEN>();
        let signature = cursor.array::<ED25519_SIGNATURE_LEN>();
        let responder_confirmation = ConfirmationTag::from_slice(cursor.bytes(CONFIRMATION_LEN))
            .expect("fixed-size confirmation");
        debug_assert!(cursor.is_finished());
        Self {
            accept: Accept {
                version,
                profile,
                suite_id,
                session_id,
                epoch,
                initiator_identity,
                responder_identity,
                initiator_ephemeral_public,
                responder_ephemeral_public,
                initiator_nonce,
                responder_nonce,
                hello_issued_at,
                issued_at,
                endpoint_context_hash,
                signature,
            },
            responder_confirmation,
        }
    }
}

impl ConfirmFrame {
    /// Encode this key confirmation into a canonical handshake control frame.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = new_frame(ControlKind::Confirm);
        out.extend_from_slice(&self.session_id);
        out.extend_from_slice(self.initiator_confirmation.as_bytes());
        out
    }

    fn decode(body: &[u8]) -> Self {
        let mut cursor = Cursor::new(body);
        let session_id = cursor.array::<SESSION_ID_LEN>();
        let initiator_confirmation = ConfirmationTag::from_slice(cursor.bytes(CONFIRMATION_LEN))
            .expect("fixed-size confirmation");
        debug_assert!(cursor.is_finished());
        Self {
            session_id,
            initiator_confirmation,
        }
    }
}

/// Decode one canonical handshake control frame.
pub fn decode_control(bytes: &[u8]) -> Result<ControlMessage, ControlCodecError> {
    let kind = parse_header(bytes)?;
    match kind {
        ControlKind::Hello => Hello::decode_control(bytes).map(ControlMessage::Hello),
        ControlKind::Accept => {
            let body = frame_body(bytes, ControlKind::Accept)?;
            Ok(ControlMessage::Accept(AcceptFrame::decode(body)))
        }
        ControlKind::Confirm => {
            let body = frame_body(bytes, ControlKind::Confirm)?;
            Ok(ControlMessage::Confirm(ConfirmFrame::decode(body)))
        }
    }
}

/// Return whether a datagram begins with the encrypted v2 data-packet kind.
pub fn is_data_packet(bytes: &[u8]) -> bool {
    bytes.first() == Some(&crate::DATA_PACKET_KIND)
}

/// Derive an order-independent endpoint context from the two Ed25519 public keys.
///
/// The classic migration profile binds both identities and sorts their public
/// keys before hashing, so both endpoints derive the same non-zero context even
/// when they have not yet learned each other's candidate socket addresses.
pub fn classic_endpoint_context(
    first_public_key: &[u8],
    second_public_key: &[u8],
) -> Result<[u8; ENDPOINT_CONTEXT_HASH_LEN], ControlCodecError> {
    if first_public_key.len() != ED25519_PUBLIC_KEY_LEN
        || second_public_key.len() != ED25519_PUBLIC_KEY_LEN
    {
        return Err(ControlCodecError::InvalidPublicKeyLength {
            expected: ED25519_PUBLIC_KEY_LEN,
        });
    }

    let (first, second) = if first_public_key <= second_public_key {
        (first_public_key, second_public_key)
    } else {
        (second_public_key, first_public_key)
    };
    let mut hasher = Sha256::new();
    hasher.update(b"fuckipv6-v2 classic endpoint context");
    hasher.update((ED25519_PUBLIC_KEY_LEN as u16).to_be_bytes());
    hasher.update(first);
    hasher.update((ED25519_PUBLIC_KEY_LEN as u16).to_be_bytes());
    hasher.update(second);
    Ok(hasher.finalize().into())
}

fn new_frame(kind: ControlKind) -> Vec<u8> {
    let mut out = Vec::with_capacity(CONTROL_HEADER_LEN + kind.body_len());
    out.extend_from_slice(&CONTROL_MAGIC);
    out.push(PROTOCOL_VERSION);
    out.push(kind.wire_value());
    out.push(0);
    out.push(0);
    out.extend_from_slice(&(kind.body_len() as u16).to_be_bytes());
    out.extend_from_slice(&[0, 0]);
    out
}

fn parse_header(bytes: &[u8]) -> Result<ControlKind, ControlCodecError> {
    if bytes.len() < CONTROL_HEADER_LEN {
        return Err(ControlCodecError::Truncated);
    }
    if bytes.len() > MAX_CONTROL_FRAME_LEN {
        return Err(ControlCodecError::Oversized {
            max: MAX_CONTROL_FRAME_LEN,
        });
    }
    if bytes[..4] != CONTROL_MAGIC {
        return Err(ControlCodecError::InvalidMagic);
    }
    if bytes[4] != PROTOCOL_VERSION {
        return Err(ControlCodecError::UnsupportedVersion {
            found: bytes[4],
            expected: PROTOCOL_VERSION,
        });
    }
    let kind = ControlKind::from_wire(bytes[5])?;
    if bytes[6] != 0 || bytes[7] != 0 || bytes[10] != 0 || bytes[11] != 0 {
        return Err(ControlCodecError::NonZeroReserved);
    }
    let declared = u16::from_be_bytes([bytes[8], bytes[9]]) as usize;
    if declared != kind.body_len() {
        return Err(ControlCodecError::InvalidBodyLength {
            declared,
            expected: kind.body_len(),
        });
    }
    let expected_len = CONTROL_HEADER_LEN + declared;
    if bytes.len() < expected_len {
        return Err(ControlCodecError::Truncated);
    }
    if bytes.len() > expected_len {
        return Err(ControlCodecError::TrailingBytes {
            trailing: bytes.len() - expected_len,
        });
    }
    Ok(kind)
}

fn frame_body(bytes: &[u8], expected: ControlKind) -> Result<&[u8], ControlCodecError> {
    let found = parse_header(bytes)?;
    if found != expected {
        return Err(ControlCodecError::UnknownKind(
            CONTROL_HEADER_LEN
                .checked_sub(6)
                .and_then(|_| bytes.get(5).copied())
                .unwrap_or(0),
        ));
    }
    Ok(&bytes[CONTROL_HEADER_LEN..])
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn bytes(&mut self, len: usize) -> &'a [u8] {
        let start = self.offset;
        let end = start + len;
        self.offset = end;
        &self.bytes[start..end]
    }

    fn array<const N: usize>(&mut self) -> [u8; N] {
        self.bytes(N).try_into().expect("canonical fixed-size body")
    }

    fn u8(&mut self) -> u8 {
        self.bytes(1)[0]
    }

    fn u16(&mut self) -> u16 {
        u16::from_be_bytes(self.array())
    }

    fn u64(&mut self) -> u64 {
        u64::from_be_bytes(self.array())
    }

    fn is_finished(&self) -> bool {
        self.offset == self.bytes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello() -> Hello {
        Hello {
            version: PROTOCOL_VERSION,
            profile: crate::handshake::CLASSIC_PROFILE_ID,
            suite_id: crate::transcript::SUITE_CHACHA20_POLY1305,
            initiator_identity: [0x11; IDENTITY_ID_LEN],
            responder_identity: [0x22; IDENTITY_ID_LEN],
            initiator_ephemeral_public: [0x33; X25519_PUBLIC_KEY_LEN],
            initiator_nonce: [0x44; HANDSHAKE_NONCE_LEN],
            issued_at: 1_700_000_000,
            endpoint_context_hash: [0x55; ENDPOINT_CONTEXT_HASH_LEN],
            signature: [0x66; ED25519_SIGNATURE_LEN],
        }
    }

    fn accept() -> AcceptFrame {
        AcceptFrame {
            accept: Accept {
                version: PROTOCOL_VERSION,
                profile: crate::handshake::CLASSIC_PROFILE_ID,
                suite_id: crate::transcript::SUITE_CHACHA20_POLY1305,
                session_id: [0x77; SESSION_ID_LEN],
                epoch: 9,
                initiator_identity: [0x11; IDENTITY_ID_LEN],
                responder_identity: [0x22; IDENTITY_ID_LEN],
                initiator_ephemeral_public: [0x33; X25519_PUBLIC_KEY_LEN],
                responder_ephemeral_public: [0x88; X25519_PUBLIC_KEY_LEN],
                initiator_nonce: [0x44; HANDSHAKE_NONCE_LEN],
                responder_nonce: [0x99; HANDSHAKE_NONCE_LEN],
                hello_issued_at: 1_700_000_000,
                issued_at: 1_700_000_001,
                endpoint_context_hash: [0x55; ENDPOINT_CONTEXT_HASH_LEN],
                signature: [0xaa; ED25519_SIGNATURE_LEN],
            },
            responder_confirmation: ConfirmationTag::from_slice(&[0xbb; CONFIRMATION_LEN]).unwrap(),
        }
    }

    fn confirm() -> ConfirmFrame {
        ConfirmFrame {
            session_id: [0x77; SESSION_ID_LEN],
            initiator_confirmation: ConfirmationTag::from_slice(&[0xcc; CONFIRMATION_LEN]).unwrap(),
        }
    }

    #[test]
    fn canonical_control_frames_round_trip() {
        let hello = hello();
        let hello_bytes = hello.encode_control();
        assert_eq!(&hello_bytes[..4], &CONTROL_MAGIC);
        assert_eq!(hello_bytes.len(), CONTROL_HEADER_LEN + HELLO_BODY_LEN);
        assert_eq!(
            decode_control(&hello_bytes),
            Ok(ControlMessage::Hello(hello))
        );

        let accept_bytes = accept().encode();
        assert_eq!(accept_bytes.len(), CONTROL_HEADER_LEN + ACCEPT_BODY_LEN);
        assert_eq!(
            decode_control(&accept_bytes),
            Ok(ControlMessage::Accept(accept()))
        );

        let confirm_bytes = confirm().encode();
        assert_eq!(confirm_bytes.len(), CONTROL_HEADER_LEN + CONFIRM_BODY_LEN);
        assert_eq!(
            decode_control(&confirm_bytes),
            Ok(ControlMessage::Confirm(confirm()))
        );
    }

    #[test]
    fn parser_rejects_truncation_oversize_trailing_and_reserved_bytes() {
        for valid in [
            hello().encode_control(),
            accept().encode(),
            confirm().encode(),
        ] {
            assert_eq!(
                decode_control(&valid[..valid.len() - 1]),
                Err(ControlCodecError::Truncated)
            );
        }

        let mut trailing = hello().encode_control();
        trailing.push(0);
        assert_eq!(
            decode_control(&trailing),
            Err(ControlCodecError::TrailingBytes { trailing: 1 })
        );

        let mut max_frame_trailing = accept().encode();
        max_frame_trailing.push(0);
        assert_eq!(
            decode_control(&max_frame_trailing),
            Err(ControlCodecError::Oversized {
                max: MAX_CONTROL_FRAME_LEN
            })
        );

        assert_eq!(
            decode_control(&vec![0; MAX_CONTROL_FRAME_LEN + 1]),
            Err(ControlCodecError::Oversized {
                max: MAX_CONTROL_FRAME_LEN
            })
        );

        let mut bad_flags = hello().encode_control();
        bad_flags[6] = 1;
        assert_eq!(
            decode_control(&bad_flags),
            Err(ControlCodecError::NonZeroReserved)
        );

        let mut bad_reserved = confirm().encode();
        bad_reserved[11] = 1;
        assert_eq!(
            decode_control(&bad_reserved),
            Err(ControlCodecError::NonZeroReserved)
        );
    }

    #[test]
    fn parser_rejects_magic_version_kind_and_body_length_mismatch() {
        let mut wrong_magic = hello().encode_control();
        wrong_magic[0] ^= 1;
        assert_eq!(
            decode_control(&wrong_magic),
            Err(ControlCodecError::InvalidMagic)
        );

        let mut wrong_version = hello().encode_control();
        wrong_version[4] = 3;
        assert_eq!(
            decode_control(&wrong_version),
            Err(ControlCodecError::UnsupportedVersion {
                found: 3,
                expected: PROTOCOL_VERSION
            })
        );

        let mut wrong_kind = hello().encode_control();
        wrong_kind[5] = 0x7f;
        assert_eq!(
            decode_control(&wrong_kind),
            Err(ControlCodecError::UnknownKind(0x7f))
        );

        let mut wrong_length = accept().encode();
        wrong_length[8..10].copy_from_slice(&(ACCEPT_BODY_LEN as u16 - 1).to_be_bytes());
        assert_eq!(
            decode_control(&wrong_length),
            Err(ControlCodecError::InvalidBodyLength {
                declared: ACCEPT_BODY_LEN - 1,
                expected: ACCEPT_BODY_LEN
            })
        );
    }

    #[test]
    fn endpoint_context_is_order_independent_and_rejects_bad_keys() {
        let first = [0x11; ED25519_PUBLIC_KEY_LEN];
        let second = [0x22; ED25519_PUBLIC_KEY_LEN];
        let a = classic_endpoint_context(&first, &second).unwrap();
        let b = classic_endpoint_context(&second, &first).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, [0; ENDPOINT_CONTEXT_HASH_LEN]);
        assert_eq!(
            classic_endpoint_context(&first[..31], &second),
            Err(ControlCodecError::InvalidPublicKeyLength {
                expected: ED25519_PUBLIC_KEY_LEN
            })
        );
    }

    #[test]
    fn arbitrary_inputs_never_panic() {
        let mut state = 0x1234_5678_9abc_def0u64;
        for len in 0..=512 {
            let mut input = vec![0u8; len];
            for byte in &mut input {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *byte = state as u8;
            }
            let _ = decode_control(&input);
        }
    }
}
