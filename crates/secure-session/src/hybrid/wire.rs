//! Strict framed control messages for the experimental hybrid profile.
//!
//! Hybrid messages are too large for a safe UDP datagram and deliberately use
//! a magic value distinct from the deployed classic `FKV2` control profile.
//! Every frame has an exact body length and an absolute 16 KiB cap. Parsing
//! rejects aliases, unknown versions/kinds, non-zero flags/reserved bytes,
//! truncation, trailing bytes, and unsupported profile/suite identifiers before
//! any handshake or key-derivation work is attempted.
//!
//! [`decode_control`] performs structural decoding and accepts either fully
//! specified hybrid suite. Callers that have already selected a profile must
//! use [`decode_control_for_profile`] instead, which rejects a different
//! version, profile, or suite before decoding the body. Decoding a `CONFIRM`
//! frame does not authenticate it; [`HybridConfirm::verify_accepted`] performs
//! the session/transcript binding checks and the constant-time HMAC check.

use super::handshake::{
    HybridAccept, HybridAccepted, HybridFinished, HybridHandshakeError, HybridHello,
    HYBRID_ACCEPT_LEN, HYBRID_HELLO_LEN, HYBRID_PROFILE_ID, MAX_CONTROL_FRAME_LEN,
};
use super::profile::{HybridSuite, NegotiatedHybridProfile, ProfileNegotiationError};
use crate::confirmation::{
    compute_confirmation, verify_confirmation, ConfirmationError, ConfirmationRole,
    ConfirmationTag, CONFIRMATION_LEN,
};
use crate::key_schedule::DerivedSession;
use crate::transcript::{
    TranscriptHash, SUITE_AES_256_GCM, SUITE_CHACHA20_POLY1305, TRANSCRIPT_HASH_LEN,
};
use crate::{PROTOCOL_VERSION, SESSION_ID_LEN};
use thiserror::Error;

/// Four-byte prefix identifying a hybrid control frame.
pub const HYBRID_CONTROL_MAGIC: [u8; 4] = *b"FKPQ";
/// Fixed hybrid control-frame header length.
pub const HYBRID_CONTROL_HEADER_LEN: usize = 12;
/// Canonical `HELLO` body length.
pub const HYBRID_HELLO_BODY_LEN: usize = HYBRID_HELLO_LEN;
/// Canonical `ACCEPT` body length.
pub const HYBRID_ACCEPT_BODY_LEN: usize = HYBRID_ACCEPT_LEN;
/// Canonical initiator `CONFIRM` body length.
pub const HYBRID_CONFIRM_BODY_LEN: usize =
    1 + 1 + 2 + SESSION_ID_LEN + TRANSCRIPT_HASH_LEN + CONFIRMATION_LEN;
/// Absolute hybrid control-frame limit, including the fixed header.
pub const MAX_HYBRID_CONTROL_FRAME_LEN: usize = MAX_CONTROL_FRAME_LEN;

const HELLO_KIND: u8 = 0x11;
const ACCEPT_KIND: u8 = 0x12;
const CONFIRM_KIND: u8 = 0x13;
const SELECTION_LEN: usize = 4;

const _: () = {
    assert!(HYBRID_CONTROL_HEADER_LEN + HYBRID_HELLO_BODY_LEN <= MAX_HYBRID_CONTROL_FRAME_LEN);
    assert!(HYBRID_CONTROL_HEADER_LEN + HYBRID_ACCEPT_BODY_LEN <= MAX_HYBRID_CONTROL_FRAME_LEN);
    assert!(HYBRID_CONTROL_HEADER_LEN + HYBRID_CONFIRM_BODY_LEN <= MAX_HYBRID_CONTROL_FRAME_LEN);
};

/// Recognized hybrid control-frame kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HybridControlKind {
    Hello,
    Accept,
    Confirm,
}

impl HybridControlKind {
    const fn wire_value(self) -> u8 {
        match self {
            Self::Hello => HELLO_KIND,
            Self::Accept => ACCEPT_KIND,
            Self::Confirm => CONFIRM_KIND,
        }
    }

    const fn body_len(self) -> usize {
        match self {
            Self::Hello => HYBRID_HELLO_BODY_LEN,
            Self::Accept => HYBRID_ACCEPT_BODY_LEN,
            Self::Confirm => HYBRID_CONFIRM_BODY_LEN,
        }
    }

    fn from_wire(value: u8) -> Result<Self, HybridControlCodecError> {
        match value {
            HELLO_KIND => Ok(Self::Hello),
            ACCEPT_KIND => Ok(Self::Accept),
            CONFIRM_KIND => Ok(Self::Confirm),
            other => Err(HybridControlCodecError::UnknownKind(other)),
        }
    }
}

/// A decoded hybrid control message.
///
/// The variants intentionally own fixed-size PQ handshake values. Boxing them
/// would reduce the enum size but add an allocation to the authenticated
/// control path without changing the wire contract.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HybridControlMessage {
    Hello(HybridHello),
    Accept(HybridAccept),
    Confirm(HybridConfirm),
}

impl HybridControlMessage {
    /// Return the wire kind of this message.
    pub const fn kind(&self) -> HybridControlKind {
        match self {
            Self::Hello(_) => HybridControlKind::Hello,
            Self::Accept(_) => HybridControlKind::Accept,
            Self::Confirm(_) => HybridControlKind::Confirm,
        }
    }

    /// Return the protocol version carried in the body.
    pub const fn version(&self) -> u8 {
        match self {
            Self::Hello(message) => message.version,
            Self::Accept(message) => message.version,
            Self::Confirm(message) => message.version,
        }
    }

    /// Return the handshake profile carried in the body.
    pub const fn profile(&self) -> u8 {
        match self {
            Self::Hello(message) => message.profile,
            Self::Accept(message) => message.profile,
            Self::Confirm(message) => message.profile,
        }
    }

    /// Return the data suite carried in the body.
    pub const fn suite_id(&self) -> u16 {
        match self {
            Self::Hello(message) => message.suite_id,
            Self::Accept(message) => message.suite_id,
            Self::Confirm(message) => message.suite_id,
        }
    }

    /// Require the body to match one locally selected hybrid profile.
    pub fn validate_profile(
        &self,
        expected: NegotiatedHybridProfile,
    ) -> Result<(), HybridControlCodecError> {
        validate_expected_profile(expected, self.version(), self.profile(), self.suite_id())
    }

    /// Encode this message after validating all body invariants.
    pub fn encode_control(&self) -> Result<Vec<u8>, HybridControlCodecError> {
        match self {
            Self::Hello(message) => message.encode_control(),
            Self::Accept(message) => message.encode_control(),
            Self::Confirm(message) => message.encode_control(),
        }
    }

    /// Require one exact profile and encode a canonical control frame.
    pub fn encode_control_for_profile(
        &self,
        expected: NegotiatedHybridProfile,
    ) -> Result<Vec<u8>, HybridControlCodecError> {
        self.validate_profile(expected)?;
        self.encode_control()
    }
}

/// Initiator confirmation bound to the complete negotiated hybrid session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HybridConfirm {
    pub version: u8,
    pub profile: u8,
    pub suite_id: u16,
    pub session_id: [u8; SESSION_ID_LEN],
    pub transcript_hash: TranscriptHash,
    pub initiator_confirmation: ConfirmationTag,
}

impl HybridConfirm {
    /// Build the initiator confirmation from a completed initiator handshake.
    ///
    /// This is the preferred constructor: [`HybridFinished`] can only be
    /// produced after the responder signature, KEM secret, and responder
    /// confirmation have all been verified.
    pub fn from_finished(finished: &HybridFinished) -> Result<Self, HybridConfirmError> {
        Self::from_derived(finished.derived())
    }

    /// Build an initiator confirmation from derived hybrid session material.
    ///
    /// The caller must obtain `derived` from a completed hybrid handshake. The
    /// constructor binds the exact suite, session identifier, TH2 transcript,
    /// and initiator-direction confirmation key into the returned message.
    pub fn from_derived(derived: &DerivedSession) -> Result<Self, HybridConfirmError> {
        HybridSuite::from_id(derived.identity.suite_id)
            .map_err(|_| HybridConfirmError::UnsupportedSuite(derived.identity.suite_id))?;
        let initiator_confirmation = compute_confirmation(
            &derived.keys,
            ConfirmationRole::Initiator,
            &derived.identity.transcript_hash,
        );
        Ok(Self {
            version: PROTOCOL_VERSION,
            profile: HYBRID_PROFILE_ID,
            suite_id: derived.identity.suite_id,
            session_id: derived.identity.session_id,
            transcript_hash: derived.identity.transcript_hash,
            initiator_confirmation,
        })
    }

    /// Verify this confirmation against a completed responder-side handshake.
    ///
    /// Version, profile, suite, session identifier, and transcript hash must
    /// all match before the HMAC is checked in constant time. A valid tag from
    /// another session or transcript therefore cannot activate this handshake.
    pub fn verify_accepted(&self, accepted: &HybridAccepted) -> Result<(), HybridConfirmError> {
        self.verify_derived(accepted.derived())
    }

    /// Verify this confirmation against the exact derived hybrid session.
    pub fn verify_derived(&self, derived: &DerivedSession) -> Result<(), HybridConfirmError> {
        if self.version != PROTOCOL_VERSION {
            return Err(HybridConfirmError::VersionMismatch {
                expected: PROTOCOL_VERSION,
                found: self.version,
            });
        }
        if self.profile != HYBRID_PROFILE_ID {
            return Err(HybridConfirmError::ProfileMismatch {
                expected: HYBRID_PROFILE_ID,
                found: self.profile,
            });
        }
        HybridSuite::from_id(derived.identity.suite_id)
            .map_err(|_| HybridConfirmError::UnsupportedSuite(derived.identity.suite_id))?;
        HybridSuite::from_id(self.suite_id)
            .map_err(|_| HybridConfirmError::UnsupportedSuite(self.suite_id))?;
        if self.suite_id != derived.identity.suite_id {
            return Err(HybridConfirmError::SuiteMismatch {
                expected: derived.identity.suite_id,
                found: self.suite_id,
            });
        }
        if self.session_id != derived.identity.session_id {
            return Err(HybridConfirmError::SessionIdMismatch);
        }
        if self.transcript_hash != derived.identity.transcript_hash {
            return Err(HybridConfirmError::TranscriptHashMismatch);
        }
        verify_confirmation(
            &derived.keys,
            ConfirmationRole::Initiator,
            &derived.identity.transcript_hash,
            self.initiator_confirmation.as_bytes(),
        )?;
        Ok(())
    }

    /// Encode the canonical fixed-length `CONFIRM` body.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HYBRID_CONFIRM_BODY_LEN);
        out.push(self.version);
        out.push(self.profile);
        out.extend_from_slice(&self.suite_id.to_be_bytes());
        out.extend_from_slice(&self.session_id);
        out.extend_from_slice(&self.transcript_hash);
        out.extend_from_slice(self.initiator_confirmation.as_bytes());
        debug_assert_eq!(out.len(), HYBRID_CONFIRM_BODY_LEN);
        out
    }

    /// Encode this confirmation into a canonical hybrid control frame.
    ///
    /// Encoding is fallible because the fields are public: a mutated version,
    /// profile, or suite is rejected instead of being emitted as an invalid or
    /// downgraded frame.
    pub fn encode_control(&self) -> Result<Vec<u8>, HybridControlCodecError> {
        let body = self.encode();
        Self::decode(&body)?;
        Ok(frame_from_body(HybridControlKind::Confirm, &body))
    }

    /// Require one exact profile and encode a canonical `CONFIRM` frame.
    pub fn encode_control_for_profile(
        &self,
        expected: NegotiatedHybridProfile,
    ) -> Result<Vec<u8>, HybridControlCodecError> {
        validate_expected_profile(expected, self.version, self.profile, self.suite_id)?;
        self.encode_control()
    }

    /// Decode one exact `CONFIRM` body and reject invalid selection metadata.
    pub fn decode(bytes: &[u8]) -> Result<Self, HybridControlCodecError> {
        require_body_len(HYBRID_CONFIRM_BODY_LEN, bytes)?;
        let version = bytes[0];
        let profile = bytes[1];
        let suite_id = u16::from_be_bytes([bytes[2], bytes[3]]);
        validate_supported_selection(version, profile, suite_id)?;

        let session_start = SELECTION_LEN;
        let transcript_start = session_start + SESSION_ID_LEN;
        let confirmation_start = transcript_start + TRANSCRIPT_HASH_LEN;
        let mut session_id = [0u8; SESSION_ID_LEN];
        session_id.copy_from_slice(&bytes[session_start..transcript_start]);
        let mut transcript_hash = [0u8; TRANSCRIPT_HASH_LEN];
        transcript_hash.copy_from_slice(&bytes[transcript_start..confirmation_start]);
        let initiator_confirmation = ConfirmationTag::from_slice(&bytes[confirmation_start..])?;
        Ok(Self {
            version,
            profile,
            suite_id,
            session_id,
            transcript_hash,
            initiator_confirmation,
        })
    }

    /// Decode a canonical hybrid `CONFIRM` control frame.
    pub fn decode_control(bytes: &[u8]) -> Result<Self, HybridControlCodecError> {
        let body = frame_body(bytes, HybridControlKind::Confirm)?;
        Self::decode(body)
    }

    /// Decode a canonical `CONFIRM` frame and require an exact profile.
    pub fn decode_control_for_profile(
        bytes: &[u8],
        expected: NegotiatedHybridProfile,
    ) -> Result<Self, HybridControlCodecError> {
        let body = frame_body(bytes, HybridControlKind::Confirm)?;
        validate_expected_profile_bytes(body, expected)?;
        Self::decode(body)
    }
}

impl HybridHello {
    /// Encode this message into a canonical hybrid control frame.
    ///
    /// The fully encoded body is decoded again before the frame is returned so
    /// an invalid public field cannot be emitted as a structurally valid frame.
    pub fn encode_control(&self) -> Result<Vec<u8>, HybridControlCodecError> {
        let body = self.encode();
        Self::decode(&body).map_err(HybridControlCodecError::Body)?;
        Ok(frame_from_body(HybridControlKind::Hello, &body))
    }

    /// Require one exact profile and encode a canonical `HELLO` frame.
    pub fn encode_control_for_profile(
        &self,
        expected: NegotiatedHybridProfile,
    ) -> Result<Vec<u8>, HybridControlCodecError> {
        validate_expected_profile(expected, self.version, self.profile, self.suite_id)?;
        self.encode_control()
    }

    /// Decode a canonical hybrid `HELLO` control frame.
    pub fn decode_control(bytes: &[u8]) -> Result<Self, HybridControlCodecError> {
        let body = frame_body(bytes, HybridControlKind::Hello)?;
        Self::decode(body).map_err(HybridControlCodecError::Body)
    }

    /// Decode a canonical `HELLO` frame and require an exact profile.
    pub fn decode_control_for_profile(
        bytes: &[u8],
        expected: NegotiatedHybridProfile,
    ) -> Result<Self, HybridControlCodecError> {
        let body = frame_body(bytes, HybridControlKind::Hello)?;
        validate_expected_profile_bytes(body, expected)?;
        Self::decode(body).map_err(HybridControlCodecError::Body)
    }
}

impl HybridAccept {
    /// Encode this message into a canonical hybrid control frame.
    ///
    /// The fully encoded body is decoded again before the frame is returned so
    /// an invalid public field cannot be emitted as a structurally valid frame.
    pub fn encode_control(&self) -> Result<Vec<u8>, HybridControlCodecError> {
        let body = self.encode();
        Self::decode(&body).map_err(HybridControlCodecError::Body)?;
        Ok(frame_from_body(HybridControlKind::Accept, &body))
    }

    /// Require one exact profile and encode a canonical `ACCEPT` frame.
    pub fn encode_control_for_profile(
        &self,
        expected: NegotiatedHybridProfile,
    ) -> Result<Vec<u8>, HybridControlCodecError> {
        validate_expected_profile(expected, self.version, self.profile, self.suite_id)?;
        self.encode_control()
    }

    /// Decode a canonical hybrid `ACCEPT` control frame.
    pub fn decode_control(bytes: &[u8]) -> Result<Self, HybridControlCodecError> {
        let body = frame_body(bytes, HybridControlKind::Accept)?;
        Self::decode(body).map_err(HybridControlCodecError::Body)
    }

    /// Decode a canonical `ACCEPT` frame and require an exact profile.
    pub fn decode_control_for_profile(
        bytes: &[u8],
        expected: NegotiatedHybridProfile,
    ) -> Result<Self, HybridControlCodecError> {
        let body = frame_body(bytes, HybridControlKind::Accept)?;
        validate_expected_profile_bytes(body, expected)?;
        Self::decode(body).map_err(HybridControlCodecError::Body)
    }
}

/// Decode any recognized hybrid control frame structurally.
///
/// Both fully specified hybrid suites are accepted. Use
/// [`decode_control_for_profile`] when a local suite has already been selected.
pub fn decode_control(bytes: &[u8]) -> Result<HybridControlMessage, HybridControlCodecError> {
    let kind = parse_header(bytes)?;
    decode_body(kind, &bytes[HYBRID_CONTROL_HEADER_LEN..])
}

/// Decode a control frame after requiring the exact negotiated profile.
///
/// The version/profile/suite tuple is checked before the body decoder performs
/// key parsing or any handshake work. A known-but-different suite is treated as
/// a downgrade attempt, not as an alternative selection.
pub fn decode_control_for_profile(
    bytes: &[u8],
    expected: NegotiatedHybridProfile,
) -> Result<HybridControlMessage, HybridControlCodecError> {
    let kind = parse_header(bytes)?;
    let body = &bytes[HYBRID_CONTROL_HEADER_LEN..];
    validate_expected_profile_bytes(body, expected)?;
    decode_body(kind, body)
}

fn decode_body(
    kind: HybridControlKind,
    body: &[u8],
) -> Result<HybridControlMessage, HybridControlCodecError> {
    match kind {
        HybridControlKind::Hello => HybridHello::decode(body)
            .map(HybridControlMessage::Hello)
            .map_err(HybridControlCodecError::Body),
        HybridControlKind::Accept => HybridAccept::decode(body)
            .map(HybridControlMessage::Accept)
            .map_err(HybridControlCodecError::Body),
        HybridControlKind::Confirm => {
            HybridConfirm::decode(body).map(HybridControlMessage::Confirm)
        }
    }
}

/// Errors returned by the hybrid control-frame parser.
#[derive(Debug, Error)]
pub enum HybridControlCodecError {
    #[error("hybrid control frame is truncated: required {required} bytes, received {actual}")]
    Truncated { required: usize, actual: usize },
    #[error("hybrid control frame is {actual} bytes, exceeding the {max}-byte limit")]
    Oversized { max: usize, actual: usize },
    #[error("invalid hybrid control magic")]
    InvalidMagic,
    #[error("unsupported hybrid control version {found}; expected {expected}")]
    UnsupportedVersion { found: u8, expected: u8 },
    #[error("unknown hybrid control frame kind {0:#04x}")]
    UnknownKind(u8),
    #[error("hybrid control byte at offset {offset} must be zero")]
    NonZeroReserved { offset: usize },
    #[error("hybrid control body length is {declared}, expected {expected}")]
    InvalidDeclaredBodyLength { declared: usize, expected: usize },
    #[error("hybrid control body must be {expected} bytes, received {actual}")]
    InvalidBodyLength { expected: usize, actual: usize },
    #[error("hybrid control frame has {trailing} trailing byte(s)")]
    TrailingBytes { trailing: usize },
    #[error("expected hybrid {expected:?} frame, received {found:?}")]
    UnexpectedKind {
        expected: HybridControlKind,
        found: HybridControlKind,
    },
    #[error(transparent)]
    Profile(#[from] ProfileNegotiationError),
    #[error(transparent)]
    Body(#[from] HybridHandshakeError),
    #[error(transparent)]
    Confirmation(#[from] ConfirmationError),
}

/// Errors returned while authenticating an initiator `CONFIRM`.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum HybridConfirmError {
    #[error("CONFIRM version {found} does not match negotiated version {expected}")]
    VersionMismatch { expected: u8, found: u8 },
    #[error("CONFIRM profile {found} does not match negotiated profile {expected}")]
    ProfileMismatch { expected: u8, found: u8 },
    #[error("CONFIRM suite {found:#06x} does not match negotiated suite {expected:#06x}")]
    SuiteMismatch { expected: u16, found: u16 },
    #[error("CONFIRM contains unsupported hybrid suite {0:#06x}")]
    UnsupportedSuite(u16),
    #[error("CONFIRM session identifier does not match the completed handshake")]
    SessionIdMismatch,
    #[error("CONFIRM transcript hash does not match the completed handshake")]
    TranscriptHashMismatch,
    #[error(transparent)]
    Confirmation(#[from] ConfirmationError),
}

fn new_frame(kind: HybridControlKind) -> Vec<u8> {
    let mut out = Vec::with_capacity(HYBRID_CONTROL_HEADER_LEN + kind.body_len());
    out.extend_from_slice(&HYBRID_CONTROL_MAGIC);
    out.push(PROTOCOL_VERSION);
    out.push(kind.wire_value());
    out.push(0);
    out.push(0);
    out.extend_from_slice(&(kind.body_len() as u16).to_be_bytes());
    out.extend_from_slice(&[0, 0]);
    out
}

fn frame_from_body(kind: HybridControlKind, body: &[u8]) -> Vec<u8> {
    debug_assert_eq!(body.len(), kind.body_len());
    let mut out = new_frame(kind);
    out.extend_from_slice(body);
    out
}

fn parse_header(bytes: &[u8]) -> Result<HybridControlKind, HybridControlCodecError> {
    if bytes.len() > MAX_HYBRID_CONTROL_FRAME_LEN {
        return Err(HybridControlCodecError::Oversized {
            max: MAX_HYBRID_CONTROL_FRAME_LEN,
            actual: bytes.len(),
        });
    }
    if bytes.len() < HYBRID_CONTROL_HEADER_LEN {
        return Err(HybridControlCodecError::Truncated {
            required: HYBRID_CONTROL_HEADER_LEN,
            actual: bytes.len(),
        });
    }
    if bytes[..4] != HYBRID_CONTROL_MAGIC {
        return Err(HybridControlCodecError::InvalidMagic);
    }
    if bytes[4] != PROTOCOL_VERSION {
        return Err(HybridControlCodecError::UnsupportedVersion {
            found: bytes[4],
            expected: PROTOCOL_VERSION,
        });
    }
    let kind = HybridControlKind::from_wire(bytes[5])?;
    for offset in [6, 7, 10, 11] {
        if bytes[offset] != 0 {
            return Err(HybridControlCodecError::NonZeroReserved { offset });
        }
    }
    let declared = u16::from_be_bytes([bytes[8], bytes[9]]) as usize;
    if declared != kind.body_len() {
        return Err(HybridControlCodecError::InvalidDeclaredBodyLength {
            declared,
            expected: kind.body_len(),
        });
    }
    let expected_len = HYBRID_CONTROL_HEADER_LEN + declared;
    if bytes.len() < expected_len {
        return Err(HybridControlCodecError::Truncated {
            required: expected_len,
            actual: bytes.len(),
        });
    }
    if bytes.len() > expected_len {
        return Err(HybridControlCodecError::TrailingBytes {
            trailing: bytes.len() - expected_len,
        });
    }
    Ok(kind)
}

fn frame_body(bytes: &[u8], expected: HybridControlKind) -> Result<&[u8], HybridControlCodecError> {
    let found = parse_header(bytes)?;
    if found != expected {
        return Err(HybridControlCodecError::UnexpectedKind { expected, found });
    }
    Ok(&bytes[HYBRID_CONTROL_HEADER_LEN..])
}

fn require_body_len(expected: usize, bytes: &[u8]) -> Result<(), HybridControlCodecError> {
    let actual = bytes.len();
    if actual == expected {
        Ok(())
    } else {
        Err(HybridControlCodecError::InvalidBodyLength { expected, actual })
    }
}

fn validate_supported_selection(
    version: u8,
    profile: u8,
    suite_id: u16,
) -> Result<(), HybridControlCodecError> {
    if version != PROTOCOL_VERSION {
        return Err(HybridHandshakeError::UnsupportedVersion(version).into());
    }
    if profile != HYBRID_PROFILE_ID {
        return Err(HybridHandshakeError::UnsupportedProfile(profile).into());
    }
    if !matches!(suite_id, SUITE_AES_256_GCM | SUITE_CHACHA20_POLY1305) {
        return Err(HybridHandshakeError::UnsupportedSuite(suite_id).into());
    }
    Ok(())
}

fn validate_expected_profile(
    expected: NegotiatedHybridProfile,
    version: u8,
    profile: u8,
    suite_id: u16,
) -> Result<(), HybridControlCodecError> {
    expected
        .validate(version, profile, suite_id)
        .map_err(HybridControlCodecError::Profile)
}

fn validate_expected_profile_bytes(
    body: &[u8],
    expected: NegotiatedHybridProfile,
) -> Result<(), HybridControlCodecError> {
    debug_assert!(body.len() >= SELECTION_LEN);
    let version = body[0];
    let profile = body[1];
    let suite_id = u16::from_be_bytes([body[2], body[3]]);
    validate_expected_profile(expected, version, profile, suite_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confirmation::ConfirmationError;
    use crate::hybrid::identity::CompositeIdentity;
    use crate::hybrid::profile::{HybridProfilePolicy, HybridSuite, NegotiatedHybridProfile};
    use crate::hybrid::{HybridInitiator, HybridResponder, HYBRID_COOKIE_LEN};
    use crate::transcript::ENDPOINT_CONTEXT_HASH_LEN;

    const NOW: u64 = 1_700_000_000;
    const EPOCH: u16 = 7;
    const ENDPOINT: [u8; ENDPOINT_CONTEXT_HASH_LEN] = [0x5a; ENDPOINT_CONTEXT_HASH_LEN];
    const EMPTY_COOKIE: [u8; HYBRID_COOKIE_LEN] = [0u8; HYBRID_COOKIE_LEN];
    const PROFILE: NegotiatedHybridProfile =
        NegotiatedHybridProfile::required(HybridSuite::ChaCha20Poly1305);
    const POLICY: HybridProfilePolicy = HybridProfilePolicy::require(HybridSuite::ChaCha20Poly1305);

    struct Fixture {
        initiator_public: crate::hybrid::CompositePublicKey,
        responder_public: crate::hybrid::CompositePublicKey,
    }

    fn initiator_identity() -> CompositeIdentity {
        crate::hybrid::CompositePublicKey::from_seeds(&[0x11; 32], &[0x22; 32])
            .expect("initiator identity")
            .0
    }

    fn responder_identity() -> CompositeIdentity {
        crate::hybrid::CompositePublicKey::from_seeds(&[0x33; 32], &[0x44; 32])
            .expect("responder identity")
            .0
    }

    fn fixture() -> Fixture {
        Fixture {
            initiator_public: initiator_identity().public_key(),
            responder_public: responder_identity().public_key(),
        }
    }

    fn hello(fixture: &Fixture) -> HybridHello {
        HybridInitiator::start(
            &initiator_identity(),
            fixture.responder_public.clone(),
            PROFILE,
            ENDPOINT,
            EPOCH,
            0,
            EMPTY_COOKIE,
            NOW,
        )
        .expect("start initiator")
        .hello()
        .clone()
    }

    fn accept(fixture: &Fixture, hello: &HybridHello) -> HybridAccept {
        HybridResponder::new(
            responder_identity(),
            fixture.initiator_public.clone(),
            None,
            POLICY,
        )
        .expect("responder")
        .accept(hello, ENDPOINT, EPOCH, NOW)
        .expect("accept")
        .accept()
        .clone()
    }

    fn completed_handshake(fixture: &Fixture) -> (HybridFinished, HybridAccepted) {
        let initiator = HybridInitiator::start(
            &initiator_identity(),
            fixture.responder_public.clone(),
            PROFILE,
            ENDPOINT,
            EPOCH,
            0,
            EMPTY_COOKIE,
            NOW,
        )
        .expect("start initiator");
        let hello = initiator.hello().clone();
        let accepted = HybridResponder::new(
            responder_identity(),
            fixture.initiator_public.clone(),
            None,
            POLICY,
        )
        .expect("responder")
        .accept(&hello, ENDPOINT, EPOCH, NOW)
        .expect("accept");
        let finished = initiator
            .finish(accepted.accept(), NOW, None)
            .expect("finish initiator");
        (finished, accepted)
    }

    fn confirm() -> HybridConfirm {
        HybridConfirm {
            version: PROTOCOL_VERSION,
            profile: HYBRID_PROFILE_ID,
            suite_id: SUITE_CHACHA20_POLY1305,
            session_id: [0x55; SESSION_ID_LEN],
            transcript_hash: [0x66; TRANSCRIPT_HASH_LEN],
            initiator_confirmation: ConfirmationTag::from_slice(&[0x77; 32]).unwrap(),
        }
    }

    fn profile_error(
        result: Result<HybridControlMessage, HybridControlCodecError>,
        expected: ProfileNegotiationError,
    ) {
        assert!(matches!(
            result,
            Err(HybridControlCodecError::Profile(found)) if found == expected
        ));
    }

    #[test]
    fn canonical_hybrid_frames_round_trip_match_the_wire_layout_and_fit_the_budget() {
        let fixture = fixture();
        let hello = hello(&fixture);
        let accept = accept(&fixture, &hello);

        let hello_frame = hello.encode_control().expect("encode hello");
        let accept_frame = accept.encode_control().expect("encode accept");
        let confirm_frame = confirm().encode_control().expect("encode confirm");

        assert_eq!(HYBRID_HELLO_BODY_LEN, 6_755);
        assert_eq!(HYBRID_ACCEPT_BODY_LEN, 6_635);
        assert_eq!(HYBRID_CONFIRM_BODY_LEN, 116);

        for (frame, kind, body_len) in [
            (
                &hello_frame,
                HybridControlKind::Hello,
                HYBRID_HELLO_BODY_LEN,
            ),
            (
                &accept_frame,
                HybridControlKind::Accept,
                HYBRID_ACCEPT_BODY_LEN,
            ),
            (
                &confirm_frame,
                HybridControlKind::Confirm,
                HYBRID_CONFIRM_BODY_LEN,
            ),
        ] {
            assert_eq!(&frame[..4], &HYBRID_CONTROL_MAGIC);
            assert_ne!(&frame[..4], b"FKV2");
            assert_eq!(frame[4], PROTOCOL_VERSION);
            assert_eq!(frame[5], kind.wire_value());
            assert_eq!(&frame[6..8], &[0, 0]);
            assert_eq!(u16::from_be_bytes([frame[8], frame[9]]) as usize, body_len);
            assert_eq!(&frame[10..12], &[0, 0]);
            assert_eq!(frame.len(), HYBRID_CONTROL_HEADER_LEN + body_len);
            assert!(frame.len() < MAX_HYBRID_CONTROL_FRAME_LEN);
        }

        assert!(matches!(
            decode_control(&hello_frame),
            Ok(HybridControlMessage::Hello(decoded)) if decoded == hello
        ));
        assert!(matches!(
            decode_control(&accept_frame),
            Ok(HybridControlMessage::Accept(decoded)) if decoded == accept
        ));
        assert!(matches!(
            decode_control(&confirm_frame),
            Ok(HybridControlMessage::Confirm(decoded)) if decoded == confirm()
        ));
    }

    #[test]
    fn parser_rejects_oversize_before_dispatch_and_every_short_or_long_boundary() {
        let actual = MAX_HYBRID_CONTROL_FRAME_LEN + 1;
        assert!(matches!(
            decode_control(&vec![0; actual]),
            Err(HybridControlCodecError::Oversized {
                max: MAX_HYBRID_CONTROL_FRAME_LEN,
                actual: found
            }) if found == actual
        ));

        for actual in 0..HYBRID_CONTROL_HEADER_LEN {
            assert!(matches!(
                decode_control(&vec![0; actual]),
                Err(HybridControlCodecError::Truncated {
                    required: HYBRID_CONTROL_HEADER_LEN,
                    actual: found
                }) if found == actual
            ));
        }

        let fixture = fixture();
        let hello = hello(&fixture).encode_control().expect("encode hello");
        for required_actual in [HYBRID_CONTROL_HEADER_LEN, hello.len() - 1] {
            assert!(matches!(
                decode_control(&hello[..required_actual]),
                Err(HybridControlCodecError::Truncated {
                    required,
                    actual: found
                }) if required == hello.len() && found == required_actual
            ));
        }

        let mut trailing = hello;
        trailing.extend_from_slice(&[0, 0]);
        assert!(matches!(
            decode_control(&trailing),
            Err(HybridControlCodecError::TrailingBytes { trailing: 2 })
        ));
    }

    #[test]
    fn parser_rejects_magic_version_kind_flags_reserved_and_length_aliases() {
        let fixture = fixture();
        let valid = hello(&fixture).encode_control().expect("encode hello");

        let mut wrong_magic = valid.clone();
        wrong_magic[..4].copy_from_slice(b"FKV2");
        assert!(matches!(
            decode_control(&wrong_magic),
            Err(HybridControlCodecError::InvalidMagic)
        ));

        let mut wrong_version = valid.clone();
        wrong_version[4] = 3;
        assert!(matches!(
            decode_control(&wrong_version),
            Err(HybridControlCodecError::UnsupportedVersion {
                found: 3,
                expected: PROTOCOL_VERSION
            })
        ));

        let mut wrong_kind = valid.clone();
        wrong_kind[5] = 0x7f;
        assert!(matches!(
            decode_control(&wrong_kind),
            Err(HybridControlCodecError::UnknownKind(0x7f))
        ));

        for offset in [6, 7, 10, 11] {
            let mut bad_reserved = valid.clone();
            bad_reserved[offset] = 1;
            assert!(matches!(
                decode_control(&bad_reserved),
                Err(HybridControlCodecError::NonZeroReserved { offset: found }) if found == offset
            ));
        }

        let mut wrong_length = valid;
        wrong_length[8..10].copy_from_slice(&(HYBRID_HELLO_BODY_LEN as u16 - 1).to_be_bytes());
        assert!(matches!(
            decode_control(&wrong_length),
            Err(HybridControlCodecError::InvalidDeclaredBodyLength {
                declared,
                expected: HYBRID_HELLO_BODY_LEN
            }) if declared == HYBRID_HELLO_BODY_LEN - 1
        ));
    }

    #[test]
    fn structural_body_decoding_rejects_unsupported_selection_and_body_semantics() {
        let fixture = fixture();
        let hello_frame = hello(&fixture).encode_control().expect("encode hello");

        let mut profile = hello_frame.clone();
        profile[HYBRID_CONTROL_HEADER_LEN + 1] = 1;
        assert!(matches!(
            decode_control(&profile),
            Err(HybridControlCodecError::Body(
                HybridHandshakeError::UnsupportedProfile(1)
            ))
        ));

        let mut version = hello_frame.clone();
        version[HYBRID_CONTROL_HEADER_LEN] = 3;
        assert!(matches!(
            decode_control(&version),
            Err(HybridControlCodecError::Body(
                HybridHandshakeError::UnsupportedVersion(3)
            ))
        ));

        let mut suite = hello_frame.clone();
        suite[HYBRID_CONTROL_HEADER_LEN + 2..HYBRID_CONTROL_HEADER_LEN + 4]
            .copy_from_slice(&0x9999u16.to_be_bytes());
        assert!(matches!(
            decode_control(&suite),
            Err(HybridControlCodecError::Body(
                HybridHandshakeError::UnsupportedSuite(0x9999)
            ))
        ));

        let psk_mode_offset = HYBRID_CONTROL_HEADER_LEN
            + SELECTION_LEN
            + (2 * 32)
            + crate::hybrid::COMPOSITE_IDENTITY_PUBLIC_KEY_LEN
            + crate::hybrid::XWING_ENCAPSULATION_KEY_LEN
            + crate::hybrid::HYBRID_NONCE_LEN
            + 8
            + ENDPOINT_CONTEXT_HASH_LEN;
        let mut psk_mode = hello_frame;
        psk_mode[psk_mode_offset] = 2;
        assert!(matches!(
            decode_control(&psk_mode),
            Err(HybridControlCodecError::Body(
                HybridHandshakeError::InvalidPskMode
            ))
        ));
    }

    #[test]
    fn selected_profile_rejects_known_suite_downgrade_before_body_decode() {
        let fixture = fixture();
        let hello = hello(&fixture);
        let mut hello_frame = hello.encode_control().expect("encode hello");
        hello_frame[HYBRID_CONTROL_HEADER_LEN + 2..HYBRID_CONTROL_HEADER_LEN + 4]
            .copy_from_slice(&SUITE_AES_256_GCM.to_be_bytes());

        // Structural decoding accepts either complete hybrid suite.
        assert!(matches!(
            decode_control(&hello_frame),
            Ok(HybridControlMessage::Hello(message))
                if message.suite_id == SUITE_AES_256_GCM
        ));

        profile_error(
            decode_control_for_profile(&hello_frame, PROFILE),
            ProfileNegotiationError::SuiteMismatch {
                required: SUITE_CHACHA20_POLY1305,
                found: SUITE_AES_256_GCM,
            },
        );

        let mut classic_profile = hello.encode_control().expect("encode hello");
        classic_profile[HYBRID_CONTROL_HEADER_LEN + 1] = 1;
        profile_error(
            decode_control_for_profile(&classic_profile, PROFILE),
            ProfileNegotiationError::ProfileMismatch {
                expected: HYBRID_PROFILE_ID,
                found: 1,
            },
        );

        let mut wrong_version = hello.encode_control().expect("encode hello");
        wrong_version[HYBRID_CONTROL_HEADER_LEN] = 3;
        profile_error(
            decode_control_for_profile(&wrong_version, PROFILE),
            ProfileNegotiationError::ProtocolVersion {
                expected: PROTOCOL_VERSION,
                found: 3,
            },
        );

        assert!(matches!(
            decode_control_for_profile(&hello.encode_control().expect("encode hello"), PROFILE),
            Ok(HybridControlMessage::Hello(decoded)) if decoded == hello
        ));
    }

    #[test]
    fn strict_encoding_rejects_invalid_public_body_fields() {
        let fixture = fixture();
        let mut invalid_hello = hello(&fixture);
        invalid_hello.profile = 1;
        assert!(matches!(
            invalid_hello.encode_control(),
            Err(HybridControlCodecError::Body(
                HybridHandshakeError::UnsupportedProfile(1)
            ))
        ));

        let source = hello(&fixture);
        let mut accept = accept(&fixture, &source);
        accept.version = 3;
        assert!(matches!(
            accept.encode_control(),
            Err(HybridControlCodecError::Body(
                HybridHandshakeError::UnsupportedVersion(3)
            ))
        ));

        let mut confirm = confirm();
        confirm.suite_id = 0x9999;
        assert!(matches!(
            confirm.encode_control(),
            Err(HybridControlCodecError::Body(
                HybridHandshakeError::UnsupportedSuite(0x9999)
            ))
        ));
    }

    #[test]
    fn confirm_integrity_checks_all_bindings_before_the_constant_time_hmac() {
        let fixture = fixture();
        let (finished, accepted) = completed_handshake(&fixture);
        let confirm = HybridConfirm::from_finished(&finished).expect("build confirm");
        assert_eq!(confirm.version, PROTOCOL_VERSION);
        assert_eq!(confirm.profile, HYBRID_PROFILE_ID);
        assert_eq!(confirm.suite_id, finished.derived().identity.suite_id);
        assert_eq!(confirm.session_id, finished.derived().identity.session_id);
        assert_eq!(
            confirm.transcript_hash,
            finished.derived().identity.transcript_hash
        );
        assert_eq!(
            confirm.initiator_confirmation,
            compute_confirmation(
                &finished.derived().keys,
                ConfirmationRole::Initiator,
                &finished.derived().identity.transcript_hash,
            )
        );
        confirm.verify_accepted(&accepted).expect("verify confirm");

        let encoded = confirm.encode_control().expect("encode confirm");
        let decoded = HybridConfirm::decode_control(&encoded).expect("decode confirm");
        assert_eq!(decoded, confirm);
        decoded
            .verify_accepted(&accepted)
            .expect("decoded confirm verifies");

        let mut wrong_session = confirm;
        wrong_session.session_id[0] ^= 1;
        assert_eq!(
            wrong_session.verify_accepted(&accepted),
            Err(HybridConfirmError::SessionIdMismatch)
        );

        let mut wrong_transcript = confirm;
        wrong_transcript.transcript_hash[0] ^= 1;
        assert_eq!(
            wrong_transcript.verify_accepted(&accepted),
            Err(HybridConfirmError::TranscriptHashMismatch)
        );

        let mut wrong_suite = confirm;
        wrong_suite.suite_id = SUITE_AES_256_GCM;
        assert_eq!(
            wrong_suite.verify_accepted(&accepted),
            Err(HybridConfirmError::SuiteMismatch {
                expected: SUITE_CHACHA20_POLY1305,
                found: SUITE_AES_256_GCM,
            })
        );

        let mut wrong_version = confirm;
        wrong_version.version = 3;
        assert_eq!(
            wrong_version.verify_accepted(&accepted),
            Err(HybridConfirmError::VersionMismatch {
                expected: PROTOCOL_VERSION,
                found: 3,
            })
        );

        let mut wrong_profile = confirm;
        wrong_profile.profile = 1;
        assert_eq!(
            wrong_profile.verify_accepted(&accepted),
            Err(HybridConfirmError::ProfileMismatch {
                expected: HYBRID_PROFILE_ID,
                found: 1,
            })
        );

        let mut wrong_tag = confirm;
        let mut tag = *wrong_tag.initiator_confirmation.as_bytes();
        tag[0] ^= 1;
        wrong_tag.initiator_confirmation = ConfirmationTag::from_slice(&tag).expect("tag");
        assert_eq!(
            wrong_tag.verify_accepted(&accepted),
            Err(HybridConfirmError::Confirmation(
                ConfirmationError::Mismatch
            ))
        );

        let responder_tag = compute_confirmation(
            &accepted.derived().keys,
            ConfirmationRole::Responder,
            &accepted.derived().identity.transcript_hash,
        );
        let mut wrong_role = confirm;
        wrong_role.initiator_confirmation = responder_tag;
        assert_eq!(
            wrong_role.verify_accepted(&accepted),
            Err(HybridConfirmError::Confirmation(
                ConfirmationError::Mismatch
            ))
        );
    }

    #[test]
    fn confirm_from_a_different_handshake_fails_closed_without_consuming_state() {
        let fixture = fixture();
        let (first_finished, _) = completed_handshake(&fixture);
        let (_, second_accepted) = completed_handshake(&fixture);
        let first_confirm =
            HybridConfirm::from_finished(&first_finished).expect("first confirmation");

        assert_eq!(
            first_confirm.verify_accepted(&second_accepted),
            Err(HybridConfirmError::SessionIdMismatch)
        );
    }

    #[test]
    fn direct_decoders_reject_a_different_frame_kind_even_with_valid_body() {
        let confirm_frame = confirm().encode_control().expect("encode confirm");
        assert!(matches!(
            HybridHello::decode_control(&confirm_frame),
            Err(HybridControlCodecError::UnexpectedKind {
                expected: HybridControlKind::Hello,
                found: HybridControlKind::Confirm
            })
        ));
        assert!(matches!(
            HybridAccept::decode_control(&confirm_frame),
            Err(HybridControlCodecError::UnexpectedKind {
                expected: HybridControlKind::Accept,
                found: HybridControlKind::Confirm
            })
        ));
        assert!(matches!(
            HybridConfirm::decode_control(&confirm_frame),
            Ok(decoded) if decoded == confirm()
        ));
    }

    #[test]
    fn direct_body_decoders_reject_truncation_and_trailing_bytes() {
        let body = confirm().encode();
        assert!(matches!(
            HybridConfirm::decode(&body[..body.len() - 1]),
            Err(HybridControlCodecError::InvalidBodyLength {
                expected: HYBRID_CONFIRM_BODY_LEN,
                actual
            }) if actual == HYBRID_CONFIRM_BODY_LEN - 1
        ));

        let mut trailing = body;
        trailing.push(0);
        assert!(matches!(
            HybridConfirm::decode(&trailing),
            Err(HybridControlCodecError::InvalidBodyLength {
                expected: HYBRID_CONFIRM_BODY_LEN,
                actual
            }) if actual == HYBRID_CONFIRM_BODY_LEN + 1
        ));
    }
}
