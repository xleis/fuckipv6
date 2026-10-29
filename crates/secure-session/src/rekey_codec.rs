//! Canonical wire encoding for online rekey control frames.
//!
//! Control frames use an 8-byte versioned header followed by a fixed-size body.
//! The body length is encoded explicitly so parsers can reject truncation and
//! trailing bytes before exposing unauthenticated fields to the state machine.
//! These frames still require authentication and cryptographic validation by
//! the caller; successful decoding is not evidence that a rekey is valid.

use crate::rekey::{RekeyAccept, RekeyConfirm, RekeyInit, REKEY_DIGEST_LEN, REKEY_NONCE_LEN};
use crate::{PROTOCOL_VERSION, SESSION_ID_LEN};
use thiserror::Error;

/// Fixed control-frame header length.
pub const REKEY_CONTROL_HEADER_LEN: usize = 8;
/// Encoded `REKEY_INIT` body length.
pub const REKEY_INIT_BODY_LEN: usize = 2 + 2 + SESSION_ID_LEN + REKEY_NONCE_LEN;
/// Encoded `REKEY_ACCEPT` body length.
pub const REKEY_ACCEPT_BODY_LEN: usize =
    REKEY_INIT_BODY_LEN + REKEY_NONCE_LEN + (3 * REKEY_DIGEST_LEN);
/// Encoded `REKEY_CONFIRM` body length.
pub const REKEY_CONFIRM_BODY_LEN: usize = REKEY_ACCEPT_BODY_LEN + REKEY_DIGEST_LEN;
/// Complete encoded `REKEY_INIT` frame length.
pub const REKEY_INIT_FRAME_LEN: usize = REKEY_CONTROL_HEADER_LEN + REKEY_INIT_BODY_LEN;
/// Complete encoded `REKEY_ACCEPT` frame length.
pub const REKEY_ACCEPT_FRAME_LEN: usize = REKEY_CONTROL_HEADER_LEN + REKEY_ACCEPT_BODY_LEN;
/// Complete encoded `REKEY_CONFIRM` frame length.
pub const REKEY_CONFIRM_FRAME_LEN: usize = REKEY_CONTROL_HEADER_LEN + REKEY_CONFIRM_BODY_LEN;
/// Hard upper bound for any v2 rekey control frame.
pub const MAX_REKEY_CONTROL_FRAME_LEN: usize = REKEY_CONFIRM_FRAME_LEN;

const REKEY_INIT_KIND: u8 = 0x31;
const REKEY_ACCEPT_KIND: u8 = 0x32;
const REKEY_CONFIRM_KIND: u8 = 0x33;

/// A recognized v2 rekey control-frame kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RekeyControlKind {
    Init,
    Accept,
    Confirm,
}

impl RekeyControlKind {
    const fn wire_value(self) -> u8 {
        match self {
            Self::Init => REKEY_INIT_KIND,
            Self::Accept => REKEY_ACCEPT_KIND,
            Self::Confirm => REKEY_CONFIRM_KIND,
        }
    }

    const fn body_len(self) -> usize {
        match self {
            Self::Init => REKEY_INIT_BODY_LEN,
            Self::Accept => REKEY_ACCEPT_BODY_LEN,
            Self::Confirm => REKEY_CONFIRM_BODY_LEN,
        }
    }

    fn from_wire(value: u8) -> Result<Self, RekeyCodecError> {
        match value {
            REKEY_INIT_KIND => Ok(Self::Init),
            REKEY_ACCEPT_KIND => Ok(Self::Accept),
            REKEY_CONFIRM_KIND => Ok(Self::Confirm),
            other => Err(RekeyCodecError::UnknownKind(other)),
        }
    }
}

/// Errors returned while decoding a rekey control frame.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum RekeyCodecError {
    #[error("rekey control frame is shorter than its fixed header")]
    Truncated,
    #[error("rekey control frame exceeds the {max}-byte limit")]
    Oversized { max: usize },
    #[error("unsupported rekey control protocol version {found}; expected {expected}")]
    UnsupportedVersion { found: u8, expected: u8 },
    #[error("unknown rekey control frame kind {0:#04x}")]
    UnknownKind(u8),
    #[error("unexpected rekey control frame kind {found:#04x}; expected {expected:#04x}")]
    UnexpectedKind { found: u8, expected: u8 },
    #[error("rekey control frame has non-zero reserved bytes")]
    NonZeroReserved,
    #[error("rekey control body length is {declared}, expected {expected}")]
    InvalidBodyLength { declared: usize, expected: usize },
    #[error("rekey control frame has {trailing} trailing byte(s)")]
    TrailingBytes { trailing: usize },
}

impl RekeyInit {
    /// Return this message's control-frame kind.
    pub const fn control_kind(&self) -> RekeyControlKind {
        RekeyControlKind::Init
    }

    /// Encode this message using the canonical `REKEY_INIT` wire format.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = new_frame(RekeyControlKind::Init);
        out.extend_from_slice(&self.base_epoch.to_be_bytes());
        out.extend_from_slice(&self.new_epoch.to_be_bytes());
        out.extend_from_slice(&self.new_session_id);
        out.extend_from_slice(&self.initiator_nonce);
        debug_assert_eq!(out.len(), REKEY_INIT_FRAME_LEN);
        out
    }

    /// Decode a canonical `REKEY_INIT` frame.
    pub fn decode(bytes: &[u8]) -> Result<Self, RekeyCodecError> {
        let body = decode_body(bytes, RekeyControlKind::Init)?;
        Ok(decode_init_body(body))
    }
}

impl RekeyAccept {
    /// Return this message's control-frame kind.
    pub const fn control_kind(&self) -> RekeyControlKind {
        RekeyControlKind::Accept
    }

    /// Encode this message using the canonical `REKEY_ACCEPT` wire format.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = new_frame(RekeyControlKind::Accept);
        encode_init_body(&mut out, &self.init);
        out.extend_from_slice(&self.responder_nonce);
        out.extend_from_slice(&self.transcript_hash);
        out.extend_from_slice(&self.responder_confirmation);
        out.extend_from_slice(&self.expected_initiator_confirmation);
        debug_assert_eq!(out.len(), REKEY_ACCEPT_FRAME_LEN);
        out
    }

    /// Decode a canonical `REKEY_ACCEPT` frame.
    pub fn decode(bytes: &[u8]) -> Result<Self, RekeyCodecError> {
        let body = decode_body(bytes, RekeyControlKind::Accept)?;
        let init = decode_init_body(&body[..REKEY_INIT_BODY_LEN]);
        let mut offset = REKEY_INIT_BODY_LEN;

        let responder_nonce = take_array(&body[offset..offset + REKEY_NONCE_LEN]);
        offset += REKEY_NONCE_LEN;
        let transcript_hash = take_array(&body[offset..offset + REKEY_DIGEST_LEN]);
        offset += REKEY_DIGEST_LEN;
        let responder_confirmation = take_array(&body[offset..offset + REKEY_DIGEST_LEN]);
        offset += REKEY_DIGEST_LEN;
        let expected_initiator_confirmation = take_array(&body[offset..offset + REKEY_DIGEST_LEN]);

        Ok(Self {
            init,
            responder_nonce,
            transcript_hash,
            responder_confirmation,
            expected_initiator_confirmation,
        })
    }
}

impl RekeyConfirm {
    /// Return this message's control-frame kind.
    pub const fn control_kind(&self) -> RekeyControlKind {
        RekeyControlKind::Confirm
    }

    /// Encode this message using the canonical `REKEY_CONFIRM` wire format.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = new_frame(RekeyControlKind::Confirm);
        encode_accept_body(&mut out, &self.accept);
        out.extend_from_slice(&self.initiator_confirmation);
        debug_assert_eq!(out.len(), REKEY_CONFIRM_FRAME_LEN);
        out
    }

    /// Decode a canonical `REKEY_CONFIRM` frame.
    pub fn decode(bytes: &[u8]) -> Result<Self, RekeyCodecError> {
        let body = decode_body(bytes, RekeyControlKind::Confirm)?;
        let accept = decode_accept_body(&body[..REKEY_ACCEPT_BODY_LEN]);
        let initiator_confirmation =
            take_array(&body[REKEY_ACCEPT_BODY_LEN..REKEY_ACCEPT_BODY_LEN + REKEY_DIGEST_LEN]);
        Ok(Self {
            accept,
            initiator_confirmation,
        })
    }
}

fn new_frame(kind: RekeyControlKind) -> Vec<u8> {
    let body_len = kind.body_len();
    let mut out = Vec::with_capacity(REKEY_CONTROL_HEADER_LEN + body_len);
    out.push(PROTOCOL_VERSION);
    out.push(kind.wire_value());
    out.push(0);
    out.push(0);
    out.extend_from_slice(&(body_len as u16).to_be_bytes());
    out.push(0);
    out.push(0);
    out
}

fn decode_body(bytes: &[u8], expected_kind: RekeyControlKind) -> Result<&[u8], RekeyCodecError> {
    if bytes.len() > MAX_REKEY_CONTROL_FRAME_LEN {
        return Err(RekeyCodecError::Oversized {
            max: MAX_REKEY_CONTROL_FRAME_LEN,
        });
    }
    if bytes.len() < REKEY_CONTROL_HEADER_LEN {
        return Err(RekeyCodecError::Truncated);
    }
    if bytes[0] != PROTOCOL_VERSION {
        return Err(RekeyCodecError::UnsupportedVersion {
            found: bytes[0],
            expected: PROTOCOL_VERSION,
        });
    }

    let found_kind = RekeyControlKind::from_wire(bytes[1])?;
    if found_kind != expected_kind {
        return Err(RekeyCodecError::UnexpectedKind {
            found: found_kind.wire_value(),
            expected: expected_kind.wire_value(),
        });
    }
    if bytes[2] != 0 || bytes[3] != 0 || bytes[6] != 0 || bytes[7] != 0 {
        return Err(RekeyCodecError::NonZeroReserved);
    }

    let declared = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
    if declared != expected_kind.body_len() {
        return Err(RekeyCodecError::InvalidBodyLength {
            declared,
            expected: expected_kind.body_len(),
        });
    }

    let expected_frame_len = REKEY_CONTROL_HEADER_LEN + declared;
    if bytes.len() < expected_frame_len {
        return Err(RekeyCodecError::Truncated);
    }
    if bytes.len() > expected_frame_len {
        return Err(RekeyCodecError::TrailingBytes {
            trailing: bytes.len() - expected_frame_len,
        });
    }
    Ok(&bytes[REKEY_CONTROL_HEADER_LEN..])
}

fn encode_init_body(out: &mut Vec<u8>, init: &RekeyInit) {
    out.extend_from_slice(&init.base_epoch.to_be_bytes());
    out.extend_from_slice(&init.new_epoch.to_be_bytes());
    out.extend_from_slice(&init.new_session_id);
    out.extend_from_slice(&init.initiator_nonce);
}

fn encode_accept_body(out: &mut Vec<u8>, accept: &RekeyAccept) {
    encode_init_body(out, &accept.init);
    out.extend_from_slice(&accept.responder_nonce);
    out.extend_from_slice(&accept.transcript_hash);
    out.extend_from_slice(&accept.responder_confirmation);
    out.extend_from_slice(&accept.expected_initiator_confirmation);
}

fn decode_init_body(body: &[u8]) -> RekeyInit {
    let base_epoch = u16::from_be_bytes([body[0], body[1]]);
    let new_epoch = u16::from_be_bytes([body[2], body[3]]);
    let new_session_id = take_array(&body[4..4 + SESSION_ID_LEN]);
    let nonce_offset = 4 + SESSION_ID_LEN;
    let initiator_nonce = take_array(&body[nonce_offset..nonce_offset + REKEY_NONCE_LEN]);
    RekeyInit {
        base_epoch,
        new_epoch,
        new_session_id,
        initiator_nonce,
    }
}

fn decode_accept_body(body: &[u8]) -> RekeyAccept {
    let init = decode_init_body(&body[..REKEY_INIT_BODY_LEN]);
    let mut offset = REKEY_INIT_BODY_LEN;
    let responder_nonce = take_array(&body[offset..offset + REKEY_NONCE_LEN]);
    offset += REKEY_NONCE_LEN;
    let transcript_hash = take_array(&body[offset..offset + REKEY_DIGEST_LEN]);
    offset += REKEY_DIGEST_LEN;
    let responder_confirmation = take_array(&body[offset..offset + REKEY_DIGEST_LEN]);
    offset += REKEY_DIGEST_LEN;
    let expected_initiator_confirmation = take_array(&body[offset..offset + REKEY_DIGEST_LEN]);
    RekeyAccept {
        init,
        responder_nonce,
        transcript_hash,
        responder_confirmation,
        expected_initiator_confirmation,
    }
}

fn take_array<const N: usize>(bytes: &[u8]) -> [u8; N] {
    let mut out = [0; N];
    out.copy_from_slice(bytes);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rekey::REKEY_NONCE_LEN;
    use std::time::Instant;

    fn init() -> RekeyInit {
        RekeyInit {
            base_epoch: 7,
            new_epoch: 8,
            new_session_id: [0x11; SESSION_ID_LEN],
            initiator_nonce: [0x22; REKEY_NONCE_LEN],
        }
    }

    fn accept() -> RekeyAccept {
        RekeyAccept {
            init: init(),
            responder_nonce: [0x33; REKEY_NONCE_LEN],
            transcript_hash: [0x44; REKEY_DIGEST_LEN],
            responder_confirmation: [0x55; REKEY_DIGEST_LEN],
            expected_initiator_confirmation: [0x66; REKEY_DIGEST_LEN],
        }
    }

    fn confirm() -> RekeyConfirm {
        RekeyConfirm {
            accept: accept(),
            initiator_confirmation: [0x77; REKEY_DIGEST_LEN],
        }
    }

    #[test]
    fn rekey_frames_round_trip_with_canonical_lengths() {
        let init = init();
        let accept = accept();
        let confirm = confirm();

        let init_bytes = init.encode();
        let accept_bytes = accept.encode();
        let confirm_bytes = confirm.encode();
        assert_eq!(init_bytes.len(), REKEY_INIT_FRAME_LEN);
        assert_eq!(accept_bytes.len(), REKEY_ACCEPT_FRAME_LEN);
        assert_eq!(confirm_bytes.len(), REKEY_CONFIRM_FRAME_LEN);
        assert_eq!(&init_bytes[..2], &[PROTOCOL_VERSION, REKEY_INIT_KIND]);
        assert_eq!(&accept_bytes[..2], &[PROTOCOL_VERSION, REKEY_ACCEPT_KIND]);
        assert_eq!(&confirm_bytes[..2], &[PROTOCOL_VERSION, REKEY_CONFIRM_KIND]);
        assert_eq!(RekeyInit::decode(&init_bytes), Ok(init));
        assert_eq!(RekeyAccept::decode(&accept_bytes), Ok(accept));
        assert_eq!(RekeyConfirm::decode(&confirm_bytes), Ok(confirm));
    }

    #[test]
    fn init_encoding_matches_the_canonical_byte_vector() {
        let mut expected = vec![PROTOCOL_VERSION, REKEY_INIT_KIND, 0, 0, 0, 52, 0, 0];
        expected.extend_from_slice(&[0, 7, 0, 8]);
        expected.extend_from_slice(&[0x11; SESSION_ID_LEN]);
        expected.extend_from_slice(&[0x22; REKEY_NONCE_LEN]);
        assert_eq!(init().encode(), expected);
    }

    #[test]
    fn all_frame_types_reject_truncated_and_trailing_bytes() {
        for valid in [init().encode(), accept().encode(), confirm().encode()] {
            let mut truncated = valid.clone();
            truncated.pop();
            assert_eq!(
                decode_any(&truncated),
                Err(RekeyCodecError::Truncated),
                "truncated frame"
            );
        }

        for valid in [init().encode(), accept().encode()] {
            let mut trailing = valid;
            trailing.push(0);
            assert_eq!(
                decode_any(&trailing),
                Err(RekeyCodecError::TrailingBytes { trailing: 1 }),
                "trailing frame"
            );
        }

        let mut oversized_confirmation = confirm().encode();
        oversized_confirmation.push(0);
        assert_eq!(
            decode_any(&oversized_confirmation),
            Err(RekeyCodecError::Oversized {
                max: MAX_REKEY_CONTROL_FRAME_LEN,
            })
        );
    }

    #[test]
    fn decoder_rejects_oversized_unknown_and_mismatched_frames() {
        assert_eq!(
            decode_any(&vec![0; MAX_REKEY_CONTROL_FRAME_LEN + 1]),
            Err(RekeyCodecError::Oversized {
                max: MAX_REKEY_CONTROL_FRAME_LEN,
            })
        );

        let mut unknown_kind = init().encode();
        unknown_kind[1] = 0x7f;
        assert_eq!(
            decode_any(&unknown_kind),
            Err(RekeyCodecError::UnknownKind(0x7f))
        );

        let accept_frame = accept().encode();
        assert_eq!(
            RekeyInit::decode(&accept_frame),
            Err(RekeyCodecError::UnexpectedKind {
                found: REKEY_ACCEPT_KIND,
                expected: REKEY_INIT_KIND,
            })
        );
    }

    #[test]
    fn decoder_rejects_versions_reserved_bytes_and_body_length_changes() {
        let mut wrong_version = init().encode();
        wrong_version[0] = 1;
        assert_eq!(
            RekeyInit::decode(&wrong_version),
            Err(RekeyCodecError::UnsupportedVersion {
                found: 1,
                expected: PROTOCOL_VERSION,
            })
        );

        for index in [2, 3, 6, 7] {
            let mut reserved = init().encode();
            reserved[index] = 1;
            assert_eq!(
                RekeyInit::decode(&reserved),
                Err(RekeyCodecError::NonZeroReserved)
            );
        }

        let mut wrong_body_len = init().encode();
        wrong_body_len[5] += 1;
        assert_eq!(
            RekeyInit::decode(&wrong_body_len),
            Err(RekeyCodecError::InvalidBodyLength {
                declared: (REKEY_INIT_BODY_LEN + 1) as u16 as usize,
                expected: REKEY_INIT_BODY_LEN,
            })
        );
    }

    #[test]
    fn control_frames_remain_compatible_with_the_rekey_state_fields() {
        let now = Instant::now();
        let init = init();
        let accept = accept();
        let confirm = RekeyConfirm {
            accept,
            initiator_confirmation: accept.expected_initiator_confirmation,
        };
        let mut initiator = crate::rekey::RekeyStateMachine::new(init.base_epoch, [0x01; 16]);
        let mut responder = crate::rekey::RekeyStateMachine::new(init.base_epoch, [0x01; 16]);
        let local_init = initiator
            .initiate(
                init.new_epoch,
                init.new_session_id,
                init.initiator_nonce,
                now,
            )
            .expect("initiate");
        let local_accept = responder
            .accept_init(
                &local_init,
                accept.responder_nonce,
                accept.transcript_hash,
                accept.responder_confirmation,
                accept.expected_initiator_confirmation,
                now,
            )
            .expect("accept");
        let decoded_accept = RekeyAccept::decode(&local_accept.encode()).expect("decode accept");
        let local_confirm = initiator
            .handle_accept(
                &decoded_accept,
                decoded_accept.expected_initiator_confirmation,
                now,
            )
            .expect("confirm");
        responder
            .handle_confirm(&local_confirm, now)
            .expect("activate");
        let decoded_confirm = RekeyConfirm::decode(&confirm.encode()).expect("decode confirm");
        assert_eq!(decoded_confirm, confirm);
    }

    fn decode_any(bytes: &[u8]) -> Result<(), RekeyCodecError> {
        if bytes.len() >= 2 && bytes[1] == REKEY_INIT_KIND {
            RekeyInit::decode(bytes).map(|_| ())
        } else if bytes.len() >= 2 && bytes[1] == REKEY_ACCEPT_KIND {
            RekeyAccept::decode(bytes).map(|_| ())
        } else if bytes.len() >= 2 && bytes[1] == REKEY_CONFIRM_KIND {
            RekeyConfirm::decode(bytes).map(|_| ())
        } else {
            RekeyInit::decode(bytes).map(|_| ())
        }
    }
}
