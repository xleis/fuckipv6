//! Canonical UDP wire encoding for the experimental classic v2 handshake.
//!
//! Handshake control frames are intentionally separate from encrypted data
//! packets. They use a fixed magic prefix so the client can route them before
//! attempting postcard decoding of legacy datagrams. Every frame has an exact
//! body length and rejects unsupported versions, kinds, flags, reserved bytes,
//! truncation, and trailing data.

use crate::confirmation::{ConfirmationTag, CONFIRMATION_LEN};
use crate::cookie::{Cookie, CookieError, COOKIE_WIRE_LEN};
use crate::handshake::{
    Accept, Hello, ED25519_PUBLIC_KEY_LEN, ED25519_SIGNATURE_LEN, HANDSHAKE_NONCE_LEN,
    X25519_PUBLIC_KEY_LEN,
};
use crate::transcript::{
    TranscriptHash, ENDPOINT_CONTEXT_HASH_LEN, IDENTITY_ID_LEN, TRANSCRIPT_HASH_LEN,
};
use crate::{PROTOCOL_VERSION, SESSION_ID_LEN};
use sha2::{Digest, Sha256};
use std::net::{IpAddr, SocketAddr};
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
pub const CONFIRM_BODY_LEN: usize = SESSION_ID_LEN + TRANSCRIPT_HASH_LEN + CONFIRMATION_LEN;
/// Canonical cookie challenge body length.
pub const COOKIE_CHALLENGE_BODY_LEN: usize = IDENTITY_ID_LEN + COOKIE_WIRE_LEN;
/// Canonical cookie retry HELLO body length.
pub const HELLO_WITH_COOKIE_BODY_LEN: usize = HELLO_BODY_LEN + COOKIE_WIRE_LEN;
/// Largest accepted handshake control frame.
pub const MAX_CONTROL_FRAME_LEN: usize = CONTROL_HEADER_LEN + ACCEPT_BODY_LEN;

const HELLO_KIND: u8 = 0x01;
const ACCEPT_KIND: u8 = 0x02;
const CONFIRM_KIND: u8 = 0x03;
const COOKIE_CHALLENGE_KIND: u8 = 0x04;
const HELLO_WITH_COOKIE_KIND: u8 = 0x05;

/// Recognized handshake control-frame kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlKind {
    Hello,
    Accept,
    Confirm,
    CookieChallenge,
    HelloWithCookie,
}

impl ControlKind {
    const fn wire_value(self) -> u8 {
        match self {
            Self::Hello => HELLO_KIND,
            Self::Accept => ACCEPT_KIND,
            Self::Confirm => CONFIRM_KIND,
            Self::CookieChallenge => COOKIE_CHALLENGE_KIND,
            Self::HelloWithCookie => HELLO_WITH_COOKIE_KIND,
        }
    }

    const fn body_len(self) -> usize {
        match self {
            Self::Hello => HELLO_BODY_LEN,
            Self::Accept => ACCEPT_BODY_LEN,
            Self::Confirm => CONFIRM_BODY_LEN,
            Self::CookieChallenge => COOKIE_CHALLENGE_BODY_LEN,
            Self::HelloWithCookie => HELLO_WITH_COOKIE_BODY_LEN,
        }
    }

    fn from_wire(value: u8) -> Result<Self, ControlCodecError> {
        match value {
            HELLO_KIND => Ok(Self::Hello),
            ACCEPT_KIND => Ok(Self::Accept),
            CONFIRM_KIND => Ok(Self::Confirm),
            COOKIE_CHALLENGE_KIND => Ok(Self::CookieChallenge),
            HELLO_WITH_COOKIE_KIND => Ok(Self::HelloWithCookie),
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
    CookieChallenge(CookieChallengeFrame),
    HelloWithCookie(HelloWithCookieFrame),
}

/// Stateless cookie challenge bound to the responder identity for routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CookieChallengeFrame {
    pub responder_identity: [u8; IDENTITY_ID_LEN],
    pub cookie: Cookie,
}

impl CookieChallengeFrame {
    /// Encode this challenge as a canonical `FKV2` control frame.
    pub fn encode(self) -> Vec<u8> {
        let mut out = new_frame(ControlKind::CookieChallenge);
        out.extend_from_slice(&self.responder_identity);
        out.extend_from_slice(&self.cookie.encode());
        out
    }

    fn decode(body: &[u8]) -> Result<Self, ControlCodecError> {
        let mut cursor = Cursor::new(body);
        let responder_identity = cursor.array::<IDENTITY_ID_LEN>();
        let cookie = Cookie::decode(cursor.bytes(COOKIE_WIRE_LEN))?;
        debug_assert!(cursor.is_finished());
        Ok(Self {
            responder_identity,
            cookie,
        })
    }
}

/// Authenticated HELLO retried with a stateless source-reachability cookie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloWithCookieFrame {
    pub hello: Hello,
    pub cookie: Cookie,
}

impl HelloWithCookieFrame {
    /// Encode this cookie-bound HELLO as a canonical `FKV2` control frame.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = new_frame(ControlKind::HelloWithCookie);
        out.extend_from_slice(&self.hello.encode_body());
        out.extend_from_slice(&self.cookie.encode());
        out
    }

    fn decode(body: &[u8]) -> Result<Self, ControlCodecError> {
        let (hello_body, cookie_body) = body.split_at(HELLO_BODY_LEN);
        Ok(Self {
            hello: Hello::decode_body(hello_body)?,
            cookie: Cookie::decode(cookie_body)?,
        })
    }
}

/// Signed responder acceptance plus the responder's key-confirmation tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptFrame {
    pub accept: Accept,
    pub responder_confirmation: ConfirmationTag,
}

/// Initiator key confirmation bound to the negotiated session and transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfirmFrame {
    pub session_id: [u8; SESSION_ID_LEN],
    pub transcript_hash: TranscriptHash,
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
    #[error("invalid handshake cookie: {0}")]
    Cookie(#[from] CookieError),
    #[error("handshake public key must be exactly {expected} bytes")]
    InvalidPublicKeyLength { expected: usize },
    #[error("endpoint context network ID must not be empty")]
    EmptyNetworkId,
    #[error("endpoint context {0} must contain at least one virtual IP")]
    MissingVirtualIps(&'static str),
    #[error("endpoint context field {0} exceeds the 65535-byte limit")]
    ContextFieldTooLong(&'static str),
}

impl Hello {
    /// Encode this `HELLO` into a canonical handshake control frame.
    pub fn encode_control(&self) -> Vec<u8> {
        let mut out = new_frame(ControlKind::Hello);
        out.extend_from_slice(&self.encode_body());
        out
    }

    /// Decode an exact canonical handshake control frame as `HELLO`.
    pub fn decode_control(bytes: &[u8]) -> Result<Self, ControlCodecError> {
        Self::decode_body(frame_body(bytes, ControlKind::Hello)?)
    }

    fn encode_body(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HELLO_BODY_LEN);
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

    fn decode_body(body: &[u8]) -> Result<Self, ControlCodecError> {
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
        out.extend_from_slice(&self.transcript_hash);
        out.extend_from_slice(self.initiator_confirmation.as_bytes());
        out
    }

    fn decode(body: &[u8]) -> Self {
        let mut cursor = Cursor::new(body);
        let session_id = cursor.array::<SESSION_ID_LEN>();
        let transcript_hash = cursor.array::<TRANSCRIPT_HASH_LEN>();
        let initiator_confirmation = ConfirmationTag::from_slice(cursor.bytes(CONFIRMATION_LEN))
            .expect("fixed-size confirmation");
        debug_assert!(cursor.is_finished());
        Self {
            session_id,
            transcript_hash,
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
        ControlKind::CookieChallenge => {
            let body = frame_body(bytes, ControlKind::CookieChallenge)?;
            Ok(ControlMessage::CookieChallenge(
                CookieChallengeFrame::decode(body)?,
            ))
        }
        ControlKind::HelloWithCookie => {
            let body = frame_body(bytes, ControlKind::HelloWithCookie)?;
            Ok(ControlMessage::HelloWithCookie(
                HelloWithCookieFrame::decode(body)?,
            ))
        }
    }
}

/// Return whether a datagram begins with the encrypted v2 data-packet kind.
pub fn is_data_packet(bytes: &[u8]) -> bool {
    bytes.first() == Some(&crate::DATA_PACKET_KIND)
}

/// Inputs bound by the experimental classic profile's endpoint context.
///
/// Slices are normalized by sorting and removing duplicates, so peers may
/// provide their candidate addresses in any order. Endpoint lists may be empty
/// for relay-only operation, but both virtual IP lists must contain at least
/// one address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassicEndpointContextInput<'a> {
    pub network_id: &'a [u8],
    pub initiator_public_key: &'a [u8],
    pub responder_public_key: &'a [u8],
    pub initiator_virtual_ips: &'a [IpAddr],
    pub responder_virtual_ips: &'a [IpAddr],
    pub initiator_endpoints: &'a [SocketAddr],
    pub responder_endpoints: &'a [SocketAddr],
}

/// Derive a canonical, order-independent classic endpoint context.
///
/// The SHA-256 input is domain separated and length-prefixed. It binds the
/// network, both Ed25519 public keys, both virtual IP sets, and both direct
/// endpoint sets. This context complements the identity signatures; it does
/// not replace identity verification or authenticate a UDP source address.
pub fn classic_endpoint_context(
    input: &ClassicEndpointContextInput<'_>,
) -> Result<[u8; ENDPOINT_CONTEXT_HASH_LEN], ControlCodecError> {
    if input.network_id.is_empty() {
        return Err(ControlCodecError::EmptyNetworkId);
    }
    if input.initiator_virtual_ips.is_empty() {
        return Err(ControlCodecError::MissingVirtualIps("initiator"));
    }
    if input.responder_virtual_ips.is_empty() {
        return Err(ControlCodecError::MissingVirtualIps("responder"));
    }
    validate_context_public_key(input.initiator_public_key)?;
    validate_context_public_key(input.responder_public_key)?;

    let initiator_ips = encode_ip_set(input.initiator_virtual_ips)?;
    let responder_ips = encode_ip_set(input.responder_virtual_ips)?;
    let initiator_endpoints = encode_endpoint_set(input.initiator_endpoints)?;
    let responder_endpoints = encode_endpoint_set(input.responder_endpoints)?;

    let mut hasher = Sha256::new();
    hash_context_field(&mut hasher, b"fuckipv6-v2 classic endpoint context v2")?;
    hash_context_field(&mut hasher, input.network_id)?;
    hash_context_field(&mut hasher, b"initiator")?;
    hash_context_field(&mut hasher, input.initiator_public_key)?;
    hash_context_field(&mut hasher, &initiator_ips)?;
    hash_context_field(&mut hasher, &initiator_endpoints)?;
    hash_context_field(&mut hasher, b"responder")?;
    hash_context_field(&mut hasher, input.responder_public_key)?;
    hash_context_field(&mut hasher, &responder_ips)?;
    hash_context_field(&mut hasher, &responder_endpoints)?;
    Ok(hasher.finalize().into())
}

fn validate_context_public_key(public_key: &[u8]) -> Result<(), ControlCodecError> {
    if public_key.len() != ED25519_PUBLIC_KEY_LEN {
        return Err(ControlCodecError::InvalidPublicKeyLength {
            expected: ED25519_PUBLIC_KEY_LEN,
        });
    }
    Ok(())
}

fn hash_context_field(hasher: &mut Sha256, bytes: &[u8]) -> Result<(), ControlCodecError> {
    let len = u16::try_from(bytes.len())
        .map_err(|_| ControlCodecError::ContextFieldTooLong("component"))?;
    hasher.update(len.to_be_bytes());
    hasher.update(bytes);
    Ok(())
}

fn encode_ip_set(ips: &[IpAddr]) -> Result<Vec<u8>, ControlCodecError> {
    let mut ips = ips.to_vec();
    ips.sort_unstable();
    ips.dedup();
    let mut encoded = Vec::with_capacity(ips.len() * 17);
    for ip in ips {
        match ip {
            IpAddr::V4(ip) => {
                encoded.push(4);
                encoded.extend_from_slice(&ip.octets());
            }
            IpAddr::V6(ip) => {
                encoded.push(6);
                encoded.extend_from_slice(&ip.octets());
            }
        }
    }
    if encoded.len() > u16::MAX as usize {
        return Err(ControlCodecError::ContextFieldTooLong("virtual IP set"));
    }
    Ok(encoded)
}

fn encode_endpoint_set(endpoints: &[SocketAddr]) -> Result<Vec<u8>, ControlCodecError> {
    let mut encoded_endpoints = endpoints
        .iter()
        .copied()
        .map(|endpoint| {
            let mut encoded = Vec::with_capacity(19);
            encoded.extend_from_slice(&endpoint.port().to_be_bytes());
            match endpoint.ip() {
                IpAddr::V4(ip) => {
                    encoded.push(4);
                    encoded.extend_from_slice(&ip.octets());
                }
                IpAddr::V6(ip) => {
                    encoded.push(6);
                    encoded.extend_from_slice(&ip.octets());
                }
            }
            encoded
        })
        .collect::<Vec<_>>();
    encoded_endpoints.sort_unstable();
    encoded_endpoints.dedup();
    let encoded = encoded_endpoints.concat();
    if encoded.len() > u16::MAX as usize {
        return Err(ControlCodecError::ContextFieldTooLong("endpoint set"));
    }
    Ok(encoded)
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
            transcript_hash: [0xdd; TRANSCRIPT_HASH_LEN],
            initiator_confirmation: ConfirmationTag::from_slice(&[0xcc; CONFIRMATION_LEN]).unwrap(),
        }
    }

    fn cookie() -> Cookie {
        let mut encoded = [0x33; COOKIE_WIRE_LEN];
        encoded[0] = crate::cookie::COOKIE_VERSION;
        Cookie::decode(&encoded).expect("canonical cookie")
    }

    fn cookie_challenge() -> CookieChallengeFrame {
        CookieChallengeFrame {
            responder_identity: [0x22; IDENTITY_ID_LEN],
            cookie: cookie(),
        }
    }

    fn hello_with_cookie() -> HelloWithCookieFrame {
        HelloWithCookieFrame {
            hello: hello(),
            cookie: cookie(),
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

        let challenge_bytes = cookie_challenge().encode();
        assert_eq!(
            challenge_bytes.len(),
            CONTROL_HEADER_LEN + COOKIE_CHALLENGE_BODY_LEN
        );
        assert_eq!(
            decode_control(&challenge_bytes),
            Ok(ControlMessage::CookieChallenge(cookie_challenge()))
        );

        let retry_bytes = hello_with_cookie().encode();
        assert_eq!(
            retry_bytes.len(),
            CONTROL_HEADER_LEN + HELLO_WITH_COOKIE_BODY_LEN
        );
        assert_eq!(
            decode_control(&retry_bytes),
            Ok(ControlMessage::HelloWithCookie(hello_with_cookie()))
        );
    }

    #[test]
    fn parser_rejects_truncation_oversize_trailing_and_reserved_bytes() {
        for valid in [
            hello().encode_control(),
            accept().encode(),
            confirm().encode(),
            cookie_challenge().encode(),
            hello_with_cookie().encode(),
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
    fn canonical_control_frames_match_fixed_sha256_vectors() {
        assert_eq!(hello().encode_control().len(), 248);
        assert_eq!(accept().encode().len(), 370);
        assert_eq!(confirm().encode().len(), 124);
        assert_eq!(cookie_challenge().encode().len(), 85);
        assert_eq!(hello_with_cookie().encode().len(), 289);

        for (frame, expected) in [
            (
                hello().encode_control(),
                "09e7cf433b1e34142451ade5e70bca051c92d546eb4672600d63b97dd10c352e",
            ),
            (
                accept().encode(),
                "f817d6316effa499ee96e7698ec58b3ee35e4468d0afa05290b4d74bb80ba77c",
            ),
            (
                confirm().encode(),
                "8c02b61faf32ac3f7181e9d68b25596f77a9586828d7e7a63bf62468a6226e10",
            ),
            (
                cookie_challenge().encode(),
                "282e5e8b5c4c94a8522e3d817afaf16c362fc70c21b471f60c95664981a896c4",
            ),
            (
                hello_with_cookie().encode(),
                "cb10360428e4318dbd3ec58f7cd82c8fc0ccb3602722eaafeae7f4c21f914bed",
            ),
        ] {
            let digest = Sha256::digest(&frame);
            assert_eq!(format!("{digest:x}"), expected);
        }
    }

    #[test]
    fn confirm_frame_binds_session_and_transcript_hash() {
        let original = confirm();
        let mut decoded = match decode_control(&original.encode()).expect("decode") {
            ControlMessage::Confirm(frame) => frame,
            _ => panic!("wrong frame kind"),
        };
        assert_eq!(decoded, original);

        decoded.session_id[0] ^= 1;
        assert_ne!(decoded, original);
        decoded = original;
        decoded.transcript_hash[0] ^= 1;
        assert_ne!(decoded, original);
        decoded = original;
        let mut tag = *decoded.initiator_confirmation.as_bytes();
        tag[0] ^= 1;
        decoded.initiator_confirmation = ConfirmationTag::from_slice(&tag).expect("tag");
        assert_ne!(decoded, original);
    }

    #[test]
    fn endpoint_context_binds_every_context_field() {
        let initiator_ips = ["10.8.0.2".parse().unwrap(), "fd00::2".parse().unwrap()];
        let responder_ips = ["10.8.0.1".parse().unwrap(), "fd00::1".parse().unwrap()];
        let initiator_endpoints = [
            "198.51.100.2:51820".parse().unwrap(),
            "[2001:db8::2]:51820".parse().unwrap(),
        ];
        let responder_endpoints = [
            "198.51.100.1:51820".parse().unwrap(),
            "[2001:db8::1]:51820".parse().unwrap(),
        ];
        let initiator_key = [0x11; ED25519_PUBLIC_KEY_LEN];
        let responder_key = [0x22; ED25519_PUBLIC_KEY_LEN];
        let baseline = ClassicEndpointContextInput {
            network_id: b"network-a",
            initiator_public_key: &initiator_key,
            responder_public_key: &responder_key,
            initiator_virtual_ips: &initiator_ips,
            responder_virtual_ips: &responder_ips,
            initiator_endpoints: &initiator_endpoints,
            responder_endpoints: &responder_endpoints,
        };
        let expected = classic_endpoint_context(&baseline).expect("baseline context");

        let other_initiator_key = [0x12; ED25519_PUBLIC_KEY_LEN];
        let other_responder_key = [0x23; ED25519_PUBLIC_KEY_LEN];
        let other_initiator_ips = ["10.8.0.3".parse().unwrap()];
        let other_responder_ips = ["10.8.0.4".parse().unwrap()];
        let other_initiator_endpoints = ["198.51.100.3:51820".parse().unwrap()];
        let other_responder_endpoints = ["198.51.100.4:51820".parse().unwrap()];
        let variants = [
            ClassicEndpointContextInput {
                network_id: b"network-b",
                ..baseline
            },
            ClassicEndpointContextInput {
                initiator_public_key: &other_initiator_key,
                ..baseline
            },
            ClassicEndpointContextInput {
                responder_public_key: &other_responder_key,
                ..baseline
            },
            ClassicEndpointContextInput {
                initiator_virtual_ips: &other_initiator_ips,
                ..baseline
            },
            ClassicEndpointContextInput {
                responder_virtual_ips: &other_responder_ips,
                ..baseline
            },
            ClassicEndpointContextInput {
                initiator_endpoints: &other_initiator_endpoints,
                ..baseline
            },
            ClassicEndpointContextInput {
                responder_endpoints: &other_responder_endpoints,
                ..baseline
            },
        ];
        for variant in variants {
            assert_ne!(
                classic_endpoint_context(&variant).expect("variant context"),
                expected
            );
        }
    }

    #[test]
    fn endpoint_context_normalizes_set_order_and_duplicates() {
        let initiator_ips = [
            "fd00::2".parse().unwrap(),
            "10.8.0.2".parse().unwrap(),
            "fd00::2".parse().unwrap(),
        ];
        let responder_ips = ["10.8.0.1".parse().unwrap(), "fd00::1".parse().unwrap()];
        let initiator_endpoints = [
            "[2001:db8::2]:51820".parse().unwrap(),
            "198.51.100.2:51820".parse().unwrap(),
            "[2001:db8::2]:51820".parse().unwrap(),
        ];
        let responder_endpoints = [
            "198.51.100.1:51820".parse().unwrap(),
            "[2001:db8::1]:51820".parse().unwrap(),
        ];
        let initiator_key = [0x11; ED25519_PUBLIC_KEY_LEN];
        let responder_key = [0x22; ED25519_PUBLIC_KEY_LEN];
        let a = classic_endpoint_context(&ClassicEndpointContextInput {
            network_id: b"network-a",
            initiator_public_key: &initiator_key,
            responder_public_key: &responder_key,
            initiator_virtual_ips: &initiator_ips,
            responder_virtual_ips: &responder_ips,
            initiator_endpoints: &initiator_endpoints,
            responder_endpoints: &responder_endpoints,
        })
        .expect("context a");

        let reordered_initiator_ips = [initiator_ips[1], initiator_ips[0]];
        let reordered_initiator_endpoints = [initiator_endpoints[1], initiator_endpoints[0]];
        let reordered_responder_endpoints = [responder_endpoints[1], responder_endpoints[0]];
        let b = classic_endpoint_context(&ClassicEndpointContextInput {
            network_id: b"network-a",
            initiator_public_key: &initiator_key,
            responder_public_key: &responder_key,
            initiator_virtual_ips: &reordered_initiator_ips,
            responder_virtual_ips: &responder_ips,
            initiator_endpoints: &reordered_initiator_endpoints,
            responder_endpoints: &reordered_responder_endpoints,
        })
        .expect("context b");
        assert_eq!(a, b);
    }

    #[test]
    fn endpoint_context_rejects_invalid_inputs() {
        let initiator_ips = ["10.8.0.2".parse().unwrap()];
        let responder_ips = ["10.8.0.1".parse().unwrap()];
        let endpoints = ["198.51.100.1:51820".parse().unwrap()];
        let valid_key = [0x11; ED25519_PUBLIC_KEY_LEN];
        let input = ClassicEndpointContextInput {
            network_id: b"network",
            initiator_public_key: &valid_key,
            responder_public_key: &valid_key,
            initiator_virtual_ips: &initiator_ips,
            responder_virtual_ips: &responder_ips,
            initiator_endpoints: &endpoints,
            responder_endpoints: &endpoints,
        };

        assert_eq!(
            classic_endpoint_context(&ClassicEndpointContextInput {
                network_id: &[],
                ..input
            }),
            Err(ControlCodecError::EmptyNetworkId)
        );
        assert_eq!(
            classic_endpoint_context(&ClassicEndpointContextInput {
                initiator_virtual_ips: &[],
                ..input
            }),
            Err(ControlCodecError::MissingVirtualIps("initiator"))
        );
        assert_eq!(
            classic_endpoint_context(&ClassicEndpointContextInput {
                responder_virtual_ips: &[],
                ..input
            }),
            Err(ControlCodecError::MissingVirtualIps("responder"))
        );
        assert_eq!(
            classic_endpoint_context(&ClassicEndpointContextInput {
                initiator_public_key: &valid_key[..31],
                ..input
            }),
            Err(ControlCodecError::InvalidPublicKeyLength {
                expected: ED25519_PUBLIC_KEY_LEN
            })
        );

        // Endpoint lists are optional for relay-only deployments.
        let relay_only = classic_endpoint_context(&ClassicEndpointContextInput {
            initiator_endpoints: &[],
            responder_endpoints: &[],
            ..input
        });
        assert!(relay_only.is_ok());
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
