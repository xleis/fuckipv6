//! Experimental X-Wing + composite-identity handshake profile.
//!
//! This module implements the complete two-message hybrid handshake primitive
//! for protocol suites `0x0201` and `0x0202`. It is deliberately independent of
//! the production `P2pEngine` control channel: the encoded `HELLO` and `ACCEPT`
//! messages exceed a safe UDP MTU and require an authenticated, length-bounded
//! control channel before they can be deployed.
//!
//! Both endpoints use fresh X-Wing keys. The responder encapsulates to the
//! initiator's X-Wing key and signs the resulting transcript with a composite
//! Ed25519 + ML-DSA-65 identity. Classic-only identities, signatures, suites,
//! and profile downgrades fail closed.

use super::identity::{
    CompositeIdentity, CompositePublicKey, CompositeSignature, IdentityError, IdentityId,
    COMPOSITE_IDENTITY_PUBLIC_KEY_LEN, COMPOSITE_SIGNATURE_LEN,
};
use super::xwing::{
    XWingCiphertext, XWingDecapsulationKey, XWingEncapsulationKey, XWingError, XWingSharedSecret,
    XWING_CIPHERTEXT_LEN, XWING_ENCAPSULATION_KEY_LEN,
};
use crate::confirmation::{
    compute_confirmation, verify_confirmation, ConfirmationError, ConfirmationRole,
};
use crate::key_schedule::{derive_from_parts, DerivedSession, KeyScheduleError};
use crate::transcript::{
    IdentityId as TranscriptIdentityId, TranscriptHash, ENDPOINT_CONTEXT_HASH_LEN,
    SUITE_AES_256_GCM, SUITE_CHACHA20_POLY1305, TRANSCRIPT_HASH_LEN,
};
use crate::{PROTOCOL_VERSION, SESSION_ID_LEN};
use sha2::{Digest, Sha512};
use std::collections::VecDeque;
use std::fmt;
use thiserror::Error;
use zeroize::Zeroizing;

/// Handshake profile marker for X-Wing + Ed25519/ML-DSA-65.
pub const HYBRID_PROFILE_ID: u8 = 2;
/// Domain used for the initiator `TH1` transcript hash.
pub const HELLO_TRANSCRIPT_DOMAIN: &[u8] = b"fuckipv6-v2 hello";
/// Domain used for the responder `TH2_input` hash.
pub const ACCEPT_TRANSCRIPT_DOMAIN: &[u8] = b"fuckipv6-v2 accept";
/// Domain used for the final authenticated `TH2` transcript hash.
pub const FINAL_TRANSCRIPT_DOMAIN: &[u8] = b"fuckipv6-v2 transcript";
/// Composite signature domain for `HELLO`.
pub const HELLO_SIGNING_DOMAIN: &[u8] = b"fuckipv6-v2 hello";
/// Composite signature domain for `ACCEPT`.
pub const ACCEPT_SIGNING_DOMAIN: &[u8] = b"fuckipv6-v2 accept";

/// Maximum accepted control-channel frame from the protocol specification.
pub const MAX_CONTROL_FRAME_LEN: usize = 16 * 1024;
/// Freshness nonce size.
pub const HYBRID_NONCE_LEN: usize = 32;
/// Fixed stateless-cookie encoding size.
pub const HYBRID_COOKIE_LEN: usize = 41;
/// Maximum accepted clock skew for `HELLO` and `ACCEPT` timestamps.
pub const HYBRID_TIMESTAMP_WINDOW_SECS: u64 = 60;
/// Default bounded responder replay-cache capacity.
pub const DEFAULT_HYBRID_REPLAY_CAPACITY: usize = 4096;
/// Exact canonical `HELLO` body size in bytes.
pub const HYBRID_HELLO_LEN: usize = 1
    + 1
    + 2
    + 32
    + 32
    + COMPOSITE_IDENTITY_PUBLIC_KEY_LEN
    + XWING_ENCAPSULATION_KEY_LEN
    + HYBRID_NONCE_LEN
    + 8
    + ENDPOINT_CONTEXT_HASH_LEN
    + 1
    + HYBRID_COOKIE_LEN
    + COMPOSITE_SIGNATURE_LEN;
/// Exact canonical `ACCEPT` body size in bytes.
pub const HYBRID_ACCEPT_LEN: usize = 1
    + 1
    + 2
    + SESSION_ID_LEN
    + 2
    + 32
    + 32
    + COMPOSITE_IDENTITY_PUBLIC_KEY_LEN
    + XWING_CIPHERTEXT_LEN
    + HYBRID_NONCE_LEN
    + 8
    + COMPOSITE_SIGNATURE_LEN
    + 32;

const _: () = {
    assert!(HYBRID_HELLO_LEN < MAX_CONTROL_FRAME_LEN);
    assert!(HYBRID_ACCEPT_LEN < MAX_CONTROL_FRAME_LEN);
    assert!(HYBRID_HELLO_LEN + HYBRID_ACCEPT_LEN < MAX_CONTROL_FRAME_LEN);
};

const ED25519_SIGNATURE_COMPONENT_LEN: usize = 64;
const AUTHENTICATION_TAG_LEN: usize = 32;

/// Initiator `HELLO` for the experimental hybrid profile.
#[derive(Clone, PartialEq, Eq)]
pub struct HybridHello {
    pub version: u8,
    pub profile: u8,
    pub suite_id: u16,
    pub initiator_identity: IdentityId,
    pub responder_identity: IdentityId,
    pub initiator_public_key: CompositePublicKey,
    pub initiator_ephemeral_public: XWingEncapsulationKey,
    pub initiator_nonce: [u8; HYBRID_NONCE_LEN],
    pub issued_at: u64,
    pub endpoint_context_hash: [u8; ENDPOINT_CONTEXT_HASH_LEN],
    pub psk_mode: u8,
    pub cookie: [u8; HYBRID_COOKIE_LEN],
    pub signature: CompositeSignature,
}

impl HybridHello {
    /// Encode the exact canonical fixed-length `HELLO` body.
    pub fn encode(&self) -> Vec<u8> {
        let public_key = self.initiator_public_key.encode();
        let ephemeral = self.initiator_ephemeral_public.encode();
        let signature = self.signature.encode();
        let mut out = Vec::with_capacity(HYBRID_HELLO_LEN);
        out.push(self.version);
        out.push(self.profile);
        out.extend_from_slice(&self.suite_id.to_be_bytes());
        out.extend_from_slice(&self.initiator_identity);
        out.extend_from_slice(&self.responder_identity);
        out.extend_from_slice(&public_key);
        out.extend_from_slice(&ephemeral);
        out.extend_from_slice(&self.initiator_nonce);
        out.extend_from_slice(&self.issued_at.to_be_bytes());
        out.extend_from_slice(&self.endpoint_context_hash);
        out.push(self.psk_mode);
        out.extend_from_slice(&self.cookie);
        out.extend_from_slice(&signature);
        debug_assert_eq!(out.len(), HYBRID_HELLO_LEN);
        out
    }

    /// Decode one exact `HELLO` body and reject truncation or trailing bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, HybridHandshakeError> {
        exact_length("HELLO", HYBRID_HELLO_LEN, bytes.len())?;
        let mut reader = Reader::new(bytes);
        let version = reader.u8()?;
        let profile = reader.u8()?;
        let suite_id = reader.u16()?;
        let initiator_identity = reader.array::<32>()?;
        let responder_identity = reader.array::<32>()?;
        let initiator_public_key =
            CompositePublicKey::decode(reader.take(COMPOSITE_IDENTITY_PUBLIC_KEY_LEN)?)?;
        let initiator_ephemeral_public =
            XWingEncapsulationKey::decode(reader.take(XWING_ENCAPSULATION_KEY_LEN)?)?;
        let initiator_nonce = reader.array::<HYBRID_NONCE_LEN>()?;
        let issued_at = reader.u64()?;
        let endpoint_context_hash = reader.array::<ENDPOINT_CONTEXT_HASH_LEN>()?;
        let psk_mode = reader.u8()?;
        let cookie = reader.array::<HYBRID_COOKIE_LEN>()?;
        let signature = CompositeSignature::decode(reader.take(COMPOSITE_SIGNATURE_LEN)?)?;
        reader.finish()?;

        validate_version(version)?;
        validate_profile(profile)?;
        validate_suite(suite_id)?;
        validate_psk_mode(psk_mode)?;
        validate_identity_binding("initiator", initiator_identity, &initiator_public_key)?;
        validate_identity_pair(initiator_identity, responder_identity)?;
        validate_endpoint_context(endpoint_context_hash)?;
        validate_nonce(&initiator_nonce, "initiator")?;

        Ok(Self {
            version,
            profile,
            suite_id,
            initiator_identity,
            responder_identity,
            initiator_public_key,
            initiator_ephemeral_public,
            initiator_nonce,
            issued_at,
            endpoint_context_hash,
            psk_mode,
            cookie,
            signature,
        })
    }

    /// Compute `TH1` over every unsigned `HELLO` field.
    pub fn transcript_hash(&self) -> TranscriptHash {
        let public_key = self.initiator_public_key.encode();
        let (ed25519_public, ml_dsa_public) = public_key.split_at(32);
        let ephemeral = self.initiator_ephemeral_public.encode();
        let issued_at = self.issued_at.to_be_bytes();
        hash_fields(
            HELLO_TRANSCRIPT_DOMAIN,
            &[
                &[self.version],
                &[self.profile],
                &self.suite_id.to_be_bytes(),
                &self.initiator_identity,
                &self.responder_identity,
                ed25519_public,
                ml_dsa_public,
                &ephemeral,
                &self.initiator_nonce,
                &issued_at,
                &self.endpoint_context_hash,
                &[self.psk_mode],
                &self.cookie,
            ],
        )
    }
}

impl fmt::Debug for HybridHello {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HybridHello")
            .field("version", &self.version)
            .field("profile", &self.profile)
            .field("suite_id", &self.suite_id)
            .field("initiator_identity", &self.initiator_identity)
            .field("responder_identity", &self.responder_identity)
            .field("issued_at", &self.issued_at)
            .field("psk_mode", &self.psk_mode)
            .finish_non_exhaustive()
    }
}

/// Responder `ACCEPT` for the experimental hybrid profile.
#[derive(Clone, PartialEq, Eq)]
pub struct HybridAccept {
    pub version: u8,
    pub profile: u8,
    pub suite_id: u16,
    pub session_id: [u8; SESSION_ID_LEN],
    pub epoch: u16,
    pub initiator_identity: IdentityId,
    pub responder_identity: IdentityId,
    pub responder_public_key: CompositePublicKey,
    pub ciphertext: XWingCiphertext,
    pub responder_nonce: [u8; HYBRID_NONCE_LEN],
    pub issued_at: u64,
    pub signature: CompositeSignature,
    pub responder_confirmation: [u8; AUTHENTICATION_TAG_LEN],
}

impl HybridAccept {
    /// Encode the exact canonical fixed-length `ACCEPT` body.
    pub fn encode(&self) -> Vec<u8> {
        let public_key = self.responder_public_key.encode();
        let ciphertext = self.ciphertext.encode();
        let signature = self.signature.encode();
        let mut out = Vec::with_capacity(HYBRID_ACCEPT_LEN);
        out.push(self.version);
        out.push(self.profile);
        out.extend_from_slice(&self.suite_id.to_be_bytes());
        out.extend_from_slice(&self.session_id);
        out.extend_from_slice(&self.epoch.to_be_bytes());
        out.extend_from_slice(&self.initiator_identity);
        out.extend_from_slice(&self.responder_identity);
        out.extend_from_slice(&public_key);
        out.extend_from_slice(&ciphertext);
        out.extend_from_slice(&self.responder_nonce);
        out.extend_from_slice(&self.issued_at.to_be_bytes());
        out.extend_from_slice(&signature);
        out.extend_from_slice(&self.responder_confirmation);
        debug_assert_eq!(out.len(), HYBRID_ACCEPT_LEN);
        out
    }

    /// Decode one exact `ACCEPT` body and reject truncation or trailing bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, HybridHandshakeError> {
        exact_length("ACCEPT", HYBRID_ACCEPT_LEN, bytes.len())?;
        let mut reader = Reader::new(bytes);
        let version = reader.u8()?;
        let profile = reader.u8()?;
        let suite_id = reader.u16()?;
        let session_id = reader.array::<SESSION_ID_LEN>()?;
        let epoch = reader.u16()?;
        let initiator_identity = reader.array::<32>()?;
        let responder_identity = reader.array::<32>()?;
        let responder_public_key =
            CompositePublicKey::decode(reader.take(COMPOSITE_IDENTITY_PUBLIC_KEY_LEN)?)?;
        let ciphertext = XWingCiphertext::decode(reader.take(XWING_CIPHERTEXT_LEN)?)?;
        let responder_nonce = reader.array::<HYBRID_NONCE_LEN>()?;
        let issued_at = reader.u64()?;
        let signature = CompositeSignature::decode(reader.take(COMPOSITE_SIGNATURE_LEN)?)?;
        let responder_confirmation = reader.array::<AUTHENTICATION_TAG_LEN>()?;
        reader.finish()?;

        validate_version(version)?;
        validate_profile(profile)?;
        validate_suite(suite_id)?;
        validate_session_id(session_id)?;
        validate_identity_binding("responder", responder_identity, &responder_public_key)?;
        validate_identity_pair(initiator_identity, responder_identity)?;
        validate_nonce(&responder_nonce, "responder")?;

        Ok(Self {
            version,
            profile,
            suite_id,
            session_id,
            epoch,
            initiator_identity,
            responder_identity,
            responder_public_key,
            ciphertext,
            responder_nonce,
            issued_at,
            signature,
            responder_confirmation,
        })
    }

    /// Hash the unsigned `ACCEPT` fields as `TH2_input`.
    pub fn signing_hash(
        &self,
        hello_hash: &TranscriptHash,
        hello_signature: &CompositeSignature,
    ) -> TranscriptHash {
        HybridAcceptTranscript {
            hello_hash,
            hello_signature,
            session_id: &self.session_id,
            epoch: self.epoch,
            initiator_identity: &self.initiator_identity,
            responder_identity: &self.responder_identity,
            responder_public_key: &self.responder_public_key,
            ciphertext: &self.ciphertext,
            responder_nonce: &self.responder_nonce,
            issued_at: self.issued_at,
        }
        .hash()
    }
}

impl fmt::Debug for HybridAccept {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HybridAccept")
            .field("version", &self.version)
            .field("profile", &self.profile)
            .field("suite_id", &self.suite_id)
            .field("session_id", &self.session_id)
            .field("epoch", &self.epoch)
            .field("initiator_identity", &self.initiator_identity)
            .field("responder_identity", &self.responder_identity)
            .field("issued_at", &self.issued_at)
            .finish_non_exhaustive()
    }
}

/// Canonical unsigned inputs to the responder `ACCEPT` signature.
pub struct HybridAcceptTranscript<'a> {
    pub hello_hash: &'a TranscriptHash,
    pub hello_signature: &'a CompositeSignature,
    pub session_id: &'a [u8; SESSION_ID_LEN],
    pub epoch: u16,
    pub initiator_identity: &'a IdentityId,
    pub responder_identity: &'a IdentityId,
    pub responder_public_key: &'a CompositePublicKey,
    pub ciphertext: &'a XWingCiphertext,
    pub responder_nonce: &'a [u8; HYBRID_NONCE_LEN],
    pub issued_at: u64,
}

impl HybridAcceptTranscript<'_> {
    /// Compute `TH2_input`.
    pub fn hash(&self) -> TranscriptHash {
        let hello_signature = self.hello_signature.encode();
        let (hello_ed25519, hello_ml_dsa) =
            hello_signature.split_at(ED25519_SIGNATURE_COMPONENT_LEN);
        let responder_public_key = self.responder_public_key.encode();
        let (responder_ed25519, responder_ml_dsa) = responder_public_key.split_at(32);
        let ciphertext = self.ciphertext.encode();
        let issued_at = self.issued_at.to_be_bytes();
        let epoch = self.epoch.to_be_bytes();
        hash_fields(
            ACCEPT_TRANSCRIPT_DOMAIN,
            &[
                self.hello_hash,
                hello_ed25519,
                hello_ml_dsa,
                self.session_id,
                &epoch,
                self.responder_identity,
                self.initiator_identity,
                responder_ed25519,
                responder_ml_dsa,
                &ciphertext,
                self.responder_nonce,
                &issued_at,
            ],
        )
    }

    /// Compute final `TH2` including the responder's composite signature.
    pub fn final_hash(&self, signature: &CompositeSignature) -> TranscriptHash {
        let th2_input = self.hash();
        let signature = signature.encode();
        let (ed25519, ml_dsa) = signature.split_at(ED25519_SIGNATURE_COMPONENT_LEN);
        hash_fields(FINAL_TRANSCRIPT_DOMAIN, &[&th2_input, ed25519, ml_dsa])
    }
}

/// Initiator state for one hybrid handshake attempt.
pub struct HybridInitiator {
    hello: HybridHello,
    ephemeral_secret: XWingDecapsulationKey,
    expected_responder: CompositePublicKey,
    requested_epoch: u16,
}

impl HybridInitiator {
    /// Start a hybrid handshake with a fresh X-Wing key and nonce.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        local_identity: &CompositeIdentity,
        expected_responder: CompositePublicKey,
        suite_id: u16,
        endpoint_context_hash: [u8; ENDPOINT_CONTEXT_HASH_LEN],
        epoch: u16,
        psk_mode: u8,
        cookie: [u8; HYBRID_COOKIE_LEN],
        issued_at: u64,
    ) -> Result<Self, HybridHandshakeError> {
        validate_suite(suite_id)?;
        validate_endpoint_context(endpoint_context_hash)?;
        validate_psk_mode(psk_mode)?;
        let initiator_identity = local_identity.identity_id();
        let responder_identity = expected_responder.identity_id();
        validate_identity_pair(initiator_identity, responder_identity)?;

        let ephemeral_secret = XWingDecapsulationKey::generate()?;
        let initiator_ephemeral_public = ephemeral_secret.encapsulation_key();
        let mut initiator_nonce = [0u8; HYBRID_NONCE_LEN];
        getrandom::fill(&mut initiator_nonce)
            .map_err(|_| HybridHandshakeError::RandomnessUnavailable)?;
        validate_nonce(&initiator_nonce, "initiator")?;

        let mut hello = HybridHello {
            version: PROTOCOL_VERSION,
            profile: HYBRID_PROFILE_ID,
            suite_id,
            initiator_identity,
            responder_identity,
            initiator_public_key: local_identity.public_key(),
            initiator_ephemeral_public,
            initiator_nonce,
            issued_at,
            endpoint_context_hash,
            psk_mode,
            cookie,
            signature: local_identity.sign(HELLO_SIGNING_DOMAIN, &[0u8; TRANSCRIPT_HASH_LEN])?,
        };
        let th1 = hello.transcript_hash();
        hello.signature = local_identity.sign(HELLO_SIGNING_DOMAIN, &th1)?;

        Ok(Self {
            hello,
            ephemeral_secret,
            expected_responder,
            requested_epoch: epoch,
        })
    }

    /// Borrow the generated `HELLO`.
    pub const fn hello(&self) -> &HybridHello {
        &self.hello
    }

    /// Validate `ACCEPT`, prove possession of the encapsulated secret, derive
    /// the directional keys, and verify the responder confirmation.
    pub fn finish(
        self,
        accept: &HybridAccept,
        now: u64,
        psk: Option<&[u8; 32]>,
    ) -> Result<HybridFinished, HybridHandshakeError> {
        self.validate_accept(accept, now)?;
        let psk = psk_for_mode(self.hello.psk_mode, psk)?;

        let th1 = self.hello.transcript_hash();
        let th2_input = accept.signing_hash(&th1, &self.hello.signature);
        accept.responder_public_key.verify(
            accept.responder_identity,
            ACCEPT_SIGNING_DOMAIN,
            &th2_input,
            &accept.signature,
        )?;
        let th2 = final_transcript_hash(&th2_input, &accept.signature);

        let shared_secret = self.ephemeral_secret.decapsulate(&accept.ciphertext)?;
        let derived = derive_hybrid_keys(
            shared_secret,
            th2,
            accept.suite_id,
            accept.session_id,
            accept.epoch,
            accept.initiator_identity,
            accept.responder_identity,
            self.hello.endpoint_context_hash,
            psk,
        )?;
        verify_confirmation(
            &derived.keys,
            ConfirmationRole::Responder,
            &th2,
            &accept.responder_confirmation,
        )?;

        Ok(HybridFinished {
            accept: accept.clone(),
            derived,
        })
    }

    fn validate_accept(&self, accept: &HybridAccept, now: u64) -> Result<(), HybridHandshakeError> {
        if accept.version != PROTOCOL_VERSION {
            return Err(HybridHandshakeError::UnsupportedVersion(accept.version));
        }
        if accept.profile != HYBRID_PROFILE_ID {
            return Err(HybridHandshakeError::UnsupportedProfile(accept.profile));
        }
        if accept.suite_id != self.hello.suite_id {
            return Err(HybridHandshakeError::AcceptMismatch);
        }
        if accept.initiator_identity != self.hello.initiator_identity
            || accept.responder_identity != self.hello.responder_identity
        {
            return Err(HybridHandshakeError::AcceptMismatch);
        }
        if accept.responder_public_key != self.expected_responder {
            return Err(HybridHandshakeError::IdentityMismatch("responder"));
        }
        if accept.epoch != self.requested_epoch {
            return Err(HybridHandshakeError::AcceptMismatch);
        }
        validate_session_id(accept.session_id)?;
        validate_nonce(&accept.responder_nonce, "responder")?;
        validate_timestamp(accept.issued_at, now)
    }
}

impl fmt::Debug for HybridInitiator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HybridInitiator")
            .field("hello", &self.hello)
            .field("requested_epoch", &self.requested_epoch)
            .field("ephemeral_secret", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// Bounded replay cache for authenticated hybrid `HELLO` values.
struct ReplayCache {
    capacity: usize,
    entries: VecDeque<(TranscriptHash, u64)>,
}

impl ReplayCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: VecDeque::with_capacity(capacity),
        }
    }

    fn prune(&mut self, now: u64) {
        while self
            .entries
            .front()
            .is_some_and(|(_, expires_at)| *expires_at <= now)
        {
            self.entries.pop_front();
        }
    }

    fn contains(&self, digest: &TranscriptHash) -> bool {
        self.entries.iter().any(|(seen, _)| seen == digest)
    }

    fn insert(
        &mut self,
        digest: TranscriptHash,
        expires_at: u64,
    ) -> Result<(), HybridHandshakeError> {
        if self.entries.len() >= self.capacity {
            return Err(HybridHandshakeError::ReplayCacheFull {
                capacity: self.capacity,
            });
        }
        self.entries.push_back((digest, expires_at));
        Ok(())
    }
}

/// Responder state for one hybrid identity pair.
pub struct HybridResponder {
    local_identity: CompositeIdentity,
    expected_initiator: CompositePublicKey,
    psk: Option<Zeroizing<[u8; 32]>>,
    replay_cache: ReplayCache,
}

impl HybridResponder {
    /// Create a responder with the default replay-cache capacity.
    pub fn new(
        local_identity: CompositeIdentity,
        expected_initiator: CompositePublicKey,
        psk: Option<[u8; 32]>,
    ) -> Result<Self, HybridHandshakeError> {
        Self::with_replay_capacity(
            local_identity,
            expected_initiator,
            psk,
            DEFAULT_HYBRID_REPLAY_CAPACITY,
        )
    }

    /// Create a responder with an explicit positive replay-cache capacity.
    pub fn with_replay_capacity(
        local_identity: CompositeIdentity,
        expected_initiator: CompositePublicKey,
        psk: Option<[u8; 32]>,
        capacity: usize,
    ) -> Result<Self, HybridHandshakeError> {
        if capacity == 0 {
            return Err(HybridHandshakeError::InvalidReplayCacheCapacity);
        }
        let initiator_identity = expected_initiator.identity_id();
        let responder_identity = local_identity.identity_id();
        validate_identity_pair(initiator_identity, responder_identity)?;
        Ok(Self {
            local_identity,
            expected_initiator,
            psk: psk.map(Zeroizing::new),
            replay_cache: ReplayCache::new(capacity),
        })
    }

    /// Validate and consume one `HELLO`, then return a signed `ACCEPT` and the
    /// responder-side derived keys.
    pub fn accept(
        &mut self,
        hello: &HybridHello,
        expected_endpoint_context: [u8; ENDPOINT_CONTEXT_HASH_LEN],
        epoch: u16,
        now: u64,
    ) -> Result<HybridAccepted, HybridHandshakeError> {
        self.validate_hello(hello, expected_endpoint_context, now)?;
        let th1 = hello.transcript_hash();
        self.replay_cache.prune(now);
        if self.replay_cache.contains(&th1) {
            return Err(HybridHandshakeError::ReplayDetected);
        }

        let (ciphertext, shared_secret) = hello.initiator_ephemeral_public.encapsulate()?;
        let mut session_id = [0u8; SESSION_ID_LEN];
        getrandom::fill(&mut session_id)
            .map_err(|_| HybridHandshakeError::RandomnessUnavailable)?;
        validate_session_id(session_id)?;
        let mut responder_nonce = [0u8; HYBRID_NONCE_LEN];
        getrandom::fill(&mut responder_nonce)
            .map_err(|_| HybridHandshakeError::RandomnessUnavailable)?;
        validate_nonce(&responder_nonce, "responder")?;

        let responder_public_key = self.local_identity.public_key();
        let initiator_identity = hello.initiator_identity;
        let responder_identity = self.local_identity.identity_id();
        let accept_transcript = HybridAcceptTranscript {
            hello_hash: &th1,
            hello_signature: &hello.signature,
            session_id: &session_id,
            epoch,
            initiator_identity: &initiator_identity,
            responder_identity: &responder_identity,
            responder_public_key: &responder_public_key,
            ciphertext: &ciphertext,
            responder_nonce: &responder_nonce,
            issued_at: now,
        };
        let th2_input = accept_transcript.hash();
        let signature = self
            .local_identity
            .sign(ACCEPT_SIGNING_DOMAIN, &th2_input)?;
        let th2 = accept_transcript.final_hash(&signature);
        let psk = self.psk.as_deref();
        let derived = derive_hybrid_keys(
            shared_secret,
            th2,
            hello.suite_id,
            session_id,
            epoch,
            initiator_identity,
            responder_identity,
            hello.endpoint_context_hash,
            psk,
        )?;
        let responder_confirmation =
            compute_confirmation(&derived.keys, ConfirmationRole::Responder, &th2);
        self.replay_cache
            .insert(th1, now.saturating_add(HYBRID_TIMESTAMP_WINDOW_SECS))?;

        Ok(HybridAccepted {
            accept: HybridAccept {
                version: PROTOCOL_VERSION,
                profile: HYBRID_PROFILE_ID,
                suite_id: hello.suite_id,
                session_id,
                epoch,
                initiator_identity,
                responder_identity,
                responder_public_key,
                ciphertext,
                responder_nonce,
                issued_at: now,
                signature,
                responder_confirmation: *responder_confirmation.as_bytes(),
            },
            derived,
        })
    }

    fn validate_hello(
        &self,
        hello: &HybridHello,
        expected_endpoint_context: [u8; ENDPOINT_CONTEXT_HASH_LEN],
        now: u64,
    ) -> Result<(), HybridHandshakeError> {
        if hello.version != PROTOCOL_VERSION {
            return Err(HybridHandshakeError::UnsupportedVersion(hello.version));
        }
        if hello.profile != HYBRID_PROFILE_ID {
            return Err(HybridHandshakeError::UnsupportedProfile(hello.profile));
        }
        validate_suite(hello.suite_id)?;
        if hello.initiator_public_key != self.expected_initiator {
            return Err(HybridHandshakeError::IdentityMismatch("initiator"));
        }
        if hello.responder_identity != self.local_identity.identity_id() {
            return Err(HybridHandshakeError::IdentityMismatch("responder"));
        }
        validate_identity_pair(hello.initiator_identity, hello.responder_identity)?;
        validate_endpoint_context(hello.endpoint_context_hash)?;
        if hello.endpoint_context_hash != expected_endpoint_context {
            return Err(HybridHandshakeError::EndpointContextMismatch);
        }
        validate_timestamp(hello.issued_at, now)?;
        validate_nonce(&hello.initiator_nonce, "initiator")?;
        psk_for_mode(hello.psk_mode, self.psk.as_deref())?;
        let th1 = hello.transcript_hash();
        hello.initiator_public_key.verify(
            hello.initiator_identity,
            HELLO_SIGNING_DOMAIN,
            &th1,
            &hello.signature,
        )?;
        Ok(())
    }
}

impl fmt::Debug for HybridResponder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HybridResponder")
            .field("local_identity", &self.local_identity)
            .field("expected_initiator", &self.expected_initiator)
            .field("psk", &self.psk.as_ref().map(|_| "<redacted>"))
            .field("replay_cache_entries", &self.replay_cache.entries.len())
            .finish_non_exhaustive()
    }
}

/// Successful responder-side hybrid handshake.
pub struct HybridAccepted {
    accept: HybridAccept,
    derived: DerivedSession,
}

impl HybridAccepted {
    /// Borrow the signed acceptance message.
    pub const fn accept(&self) -> &HybridAccept {
        &self.accept
    }

    /// Borrow the responder-side derived session and transcript identity.
    pub const fn derived(&self) -> &DerivedSession {
        &self.derived
    }

    /// Consume the result and return its authenticated components.
    pub fn into_parts(self) -> (HybridAccept, DerivedSession) {
        (self.accept, self.derived)
    }
}

impl fmt::Debug for HybridAccepted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HybridAccepted")
            .field("accept", &self.accept)
            .field("derived", &"<redacted>")
            .finish()
    }
}

/// Successful initiator-side hybrid handshake.
pub struct HybridFinished {
    accept: HybridAccept,
    derived: DerivedSession,
}

impl HybridFinished {
    /// Borrow the accepted responder message.
    pub const fn accept(&self) -> &HybridAccept {
        &self.accept
    }

    /// Borrow the initiator-side derived session and transcript identity.
    pub const fn derived(&self) -> &DerivedSession {
        &self.derived
    }

    /// Consume the result and return its authenticated components.
    pub fn into_parts(self) -> (HybridAccept, DerivedSession) {
        (self.accept, self.derived)
    }
}

impl fmt::Debug for HybridFinished {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HybridFinished")
            .field("accept", &self.accept)
            .field("derived", &"<redacted>")
            .finish()
    }
}

/// Errors returned by the hybrid handshake profile.
#[derive(Debug, Error)]
pub enum HybridHandshakeError {
    #[error("operating system randomness is unavailable")]
    RandomnessUnavailable,
    #[error("unsupported protocol version {0}")]
    UnsupportedVersion(u8),
    #[error("unsupported handshake profile {0}")]
    UnsupportedProfile(u8),
    #[error("unsupported v2 suite {0:#06x}")]
    UnsupportedSuite(u16),
    #[error("PSK mode must be zero or one")]
    InvalidPskMode,
    #[error("PSK mode does not match the configured key")]
    PskModeMismatch,
    #[error("{0} identity must not be all zero")]
    InvalidIdentity(&'static str),
    #[error("initiator and responder identities must differ")]
    DuplicateIdentity,
    #[error("{0} identity does not match the complete public key")]
    IdentityMismatch(&'static str),
    #[error("endpoint context hash must not be all zero")]
    InvalidEndpointContext,
    #[error("endpoint context hash does not match the expected context")]
    EndpointContextMismatch,
    #[error("handshake timestamp is outside the accepted clock-skew window")]
    TimestampOutsideWindow,
    #[error("{0} freshness nonce must not be all zero")]
    InvalidNonce(&'static str),
    #[error("session identifier must not be all zero")]
    InvalidSessionId,
    #[error("{item} encoded body must be exactly {expected} bytes, received {actual}")]
    InvalidLength {
        item: &'static str,
        expected: usize,
        actual: usize,
    },
    #[error("malformed {0} encoding")]
    InvalidEncoding(&'static str),
    #[error("ACCEPT does not match the originating HELLO")]
    AcceptMismatch,
    #[error("HELLO was already consumed by this responder")]
    ReplayDetected,
    #[error("replay cache is full ({capacity} unexpired entries)")]
    ReplayCacheFull { capacity: usize },
    #[error("replay cache capacity must be greater than zero")]
    InvalidReplayCacheCapacity,
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error(transparent)]
    XWing(#[from] XWingError),
    #[error(transparent)]
    KeySchedule(#[from] KeyScheduleError),
    #[error(transparent)]
    Confirmation(#[from] ConfirmationError),
}

#[allow(clippy::too_many_arguments)]
fn derive_hybrid_keys(
    shared_secret: XWingSharedSecret,
    transcript_hash: TranscriptHash,
    suite_id: u16,
    session_id: [u8; SESSION_ID_LEN],
    epoch: u16,
    initiator_identity: IdentityId,
    responder_identity: IdentityId,
    endpoint_context_hash: [u8; ENDPOINT_CONTEXT_HASH_LEN],
    psk: Option<&[u8; 32]>,
) -> Result<DerivedSession, HybridHandshakeError> {
    let identity = crate::handshake::HandshakeIdentity {
        suite_id,
        session_id,
        epoch,
        initiator_identity: TranscriptIdentityId::from(initiator_identity),
        responder_identity: TranscriptIdentityId::from(responder_identity),
        endpoint_context_hash,
        transcript_hash,
    };
    let shared_secret = Zeroizing::new(*shared_secret.expose_secret());
    let keys = derive_from_parts(shared_secret, transcript_hash, identity, psk)?;
    Ok(DerivedSession { identity, keys })
}

fn final_transcript_hash(
    th2_input: &TranscriptHash,
    signature: &CompositeSignature,
) -> TranscriptHash {
    let signature = signature.encode();
    let (ed25519, ml_dsa) = signature.split_at(ED25519_SIGNATURE_COMPONENT_LEN);
    hash_fields(FINAL_TRANSCRIPT_DOMAIN, &[th2_input, ed25519, ml_dsa])
}

fn hash_fields(domain: &[u8], fields: &[&[u8]]) -> TranscriptHash {
    let mut canonical = Vec::new();
    push_field(&mut canonical, domain);
    for field in fields {
        push_field(&mut canonical, field);
    }
    let digest = Sha512::digest(canonical);
    let mut hash = [0u8; TRANSCRIPT_HASH_LEN];
    hash.copy_from_slice(&digest);
    hash
}

fn push_field(out: &mut Vec<u8>, value: &[u8]) {
    let length = u16::try_from(value.len()).expect("handshake fields fit in u16");
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(value);
}

fn validate_version(version: u8) -> Result<(), HybridHandshakeError> {
    if version == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(HybridHandshakeError::UnsupportedVersion(version))
    }
}

fn validate_profile(profile: u8) -> Result<(), HybridHandshakeError> {
    if profile == HYBRID_PROFILE_ID {
        Ok(())
    } else {
        Err(HybridHandshakeError::UnsupportedProfile(profile))
    }
}

fn validate_suite(suite_id: u16) -> Result<(), HybridHandshakeError> {
    if matches!(suite_id, SUITE_AES_256_GCM | SUITE_CHACHA20_POLY1305) {
        Ok(())
    } else {
        Err(HybridHandshakeError::UnsupportedSuite(suite_id))
    }
}

fn validate_psk_mode(psk_mode: u8) -> Result<(), HybridHandshakeError> {
    if psk_mode <= 1 {
        Ok(())
    } else {
        Err(HybridHandshakeError::InvalidPskMode)
    }
}

fn psk_for_mode(
    psk_mode: u8,
    psk: Option<&[u8; 32]>,
) -> Result<Option<&[u8; 32]>, HybridHandshakeError> {
    validate_psk_mode(psk_mode)?;
    match (psk_mode, psk) {
        (0, None) | (1, Some(_)) => Ok(psk),
        _ => Err(HybridHandshakeError::PskModeMismatch),
    }
}

fn validate_identity_binding(
    role: &'static str,
    identity: IdentityId,
    public_key: &CompositePublicKey,
) -> Result<(), HybridHandshakeError> {
    if identity == [0; 32] {
        return Err(HybridHandshakeError::InvalidIdentity(role));
    }
    if public_key.identity_id() != identity {
        return Err(HybridHandshakeError::IdentityMismatch(role));
    }
    Ok(())
}

fn validate_identity_pair(
    initiator: IdentityId,
    responder: IdentityId,
) -> Result<(), HybridHandshakeError> {
    if initiator == [0; 32] {
        return Err(HybridHandshakeError::InvalidIdentity("initiator"));
    }
    if responder == [0; 32] {
        return Err(HybridHandshakeError::InvalidIdentity("responder"));
    }
    if initiator == responder {
        return Err(HybridHandshakeError::DuplicateIdentity);
    }
    Ok(())
}

fn validate_endpoint_context(
    endpoint_context_hash: [u8; ENDPOINT_CONTEXT_HASH_LEN],
) -> Result<(), HybridHandshakeError> {
    if endpoint_context_hash == [0; ENDPOINT_CONTEXT_HASH_LEN] {
        Err(HybridHandshakeError::InvalidEndpointContext)
    } else {
        Ok(())
    }
}

fn validate_timestamp(timestamp: u64, now: u64) -> Result<(), HybridHandshakeError> {
    if now.abs_diff(timestamp) > HYBRID_TIMESTAMP_WINDOW_SECS {
        Err(HybridHandshakeError::TimestampOutsideWindow)
    } else {
        Ok(())
    }
}

fn validate_nonce(
    nonce: &[u8; HYBRID_NONCE_LEN],
    role: &'static str,
) -> Result<(), HybridHandshakeError> {
    if *nonce == [0; HYBRID_NONCE_LEN] {
        Err(HybridHandshakeError::InvalidNonce(role))
    } else {
        Ok(())
    }
}

fn validate_session_id(session_id: [u8; SESSION_ID_LEN]) -> Result<(), HybridHandshakeError> {
    if session_id == [0; SESSION_ID_LEN] {
        Err(HybridHandshakeError::InvalidSessionId)
    } else {
        Ok(())
    }
}

fn exact_length(
    item: &'static str,
    expected: usize,
    actual: usize,
) -> Result<(), HybridHandshakeError> {
    if expected == actual {
        Ok(())
    } else {
        Err(HybridHandshakeError::InvalidLength {
            item,
            expected,
            actual,
        })
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], HybridHandshakeError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(HybridHandshakeError::InvalidEncoding("body"))?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(HybridHandshakeError::InvalidEncoding("body"))?;
        self.offset = end;
        Ok(value)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], HybridHandshakeError> {
        self.take(N)?
            .try_into()
            .map_err(|_| HybridHandshakeError::InvalidEncoding("body"))
    }

    fn u8(&mut self) -> Result<u8, HybridHandshakeError> {
        Ok(self.array::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16, HybridHandshakeError> {
        Ok(u16::from_be_bytes(self.array::<2>()?))
    }

    fn u64(&mut self) -> Result<u64, HybridHandshakeError> {
        Ok(u64::from_be_bytes(self.array::<8>()?))
    }

    fn finish(&self) -> Result<(), HybridHandshakeError> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(HybridHandshakeError::InvalidEncoding("trailing bytes"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{ENDPOINT_CONTEXT_HASH_LEN, SUITE_CHACHA20_POLY1305};
    use crate::Direction;

    const NOW: u64 = 1_700_000_000;
    const EPOCH: u16 = 7;
    const ENDPOINT: [u8; ENDPOINT_CONTEXT_HASH_LEN] = [0x5a; ENDPOINT_CONTEXT_HASH_LEN];
    const EMPTY_COOKIE: [u8; HYBRID_COOKIE_LEN] = [0u8; HYBRID_COOKIE_LEN];

    struct Fixture {
        initiator_public: CompositePublicKey,
        responder_public: CompositePublicKey,
    }

    fn initiator_identity() -> CompositeIdentity {
        CompositePublicKey::from_seeds(&[0x11; 32], &[0x22; 32])
            .expect("initiator identity")
            .0
    }

    fn responder_identity() -> CompositeIdentity {
        CompositePublicKey::from_seeds(&[0x33; 32], &[0x44; 32])
            .expect("responder identity")
            .0
    }

    fn fixture() -> Fixture {
        Fixture {
            initiator_public: initiator_identity().public_key(),
            responder_public: responder_identity().public_key(),
        }
    }

    fn initiator(fixture: &Fixture) -> HybridInitiator {
        let identity = initiator_identity();
        HybridInitiator::start(
            &identity,
            fixture.responder_public.clone(),
            SUITE_CHACHA20_POLY1305,
            ENDPOINT,
            EPOCH,
            0,
            EMPTY_COOKIE,
            NOW,
        )
        .expect("start hybrid initiator")
    }

    fn responder(fixture: &Fixture) -> HybridResponder {
        HybridResponder::new(responder_identity(), fixture.initiator_public.clone(), None)
            .expect("responder")
    }

    fn complete(fixture: &Fixture) -> (HybridAccept, HybridFinished, HybridAccepted) {
        let initiator = initiator(fixture);
        let hello = initiator.hello().clone();
        let accepted = responder(fixture)
            .accept(&hello, ENDPOINT, EPOCH, NOW)
            .expect("accept");
        let accept = accepted.accept().clone();
        let finished = initiator.finish(&accept, NOW, None).expect("finish");
        (accept, finished, accepted)
    }

    #[test]
    fn both_endpoints_derive_the_same_keys_and_transcript() {
        let fixture = fixture();
        let (accept, finished, accepted) = complete(&fixture);
        let initiator = finished.derived();
        let responder = accepted.derived();

        assert_eq!(initiator.identity, responder.identity);
        assert_ne!(
            initiator.identity.transcript_hash,
            [0u8; TRANSCRIPT_HASH_LEN]
        );
        assert_eq!(accept.session_id, initiator.identity.session_id);
        assert_eq!(accept.epoch, EPOCH);
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
    fn hello_and_accept_wire_bodies_round_trip_and_fit_control_budget() {
        let fixture = fixture();
        let initiator = initiator(&fixture);
        let hello = initiator.hello().clone();
        let encoded_hello = hello.encode();
        assert_eq!(HYBRID_HELLO_LEN, 6_755);
        assert_eq!(encoded_hello.len(), HYBRID_HELLO_LEN);
        assert_eq!(HybridHello::decode(&encoded_hello).unwrap(), hello);

        let accepted = responder(&fixture)
            .accept(&hello, ENDPOINT, EPOCH, NOW)
            .unwrap();
        let encoded_accept = accepted.accept().encode();
        assert_eq!(HYBRID_ACCEPT_LEN, 6_635);
        assert_eq!(encoded_accept.len(), HYBRID_ACCEPT_LEN);
        assert_eq!(
            HybridAccept::decode(&encoded_accept).unwrap(),
            *accepted.accept()
        );
        assert_eq!(HYBRID_HELLO_LEN + HYBRID_ACCEPT_LEN, 13_390);
    }

    #[test]
    fn strict_decoders_reject_truncation_and_trailing_bytes() {
        let fixture = fixture();
        let initiator = initiator(&fixture);
        let hello = initiator.hello().encode();
        assert!(HybridHello::decode(&hello[..hello.len() - 1]).is_err());
        assert!(HybridHello::decode(&[hello.as_slice(), &[0]].concat()).is_err());

        let accept = responder(&fixture)
            .accept(initiator.hello(), ENDPOINT, EPOCH, NOW)
            .unwrap();
        let accept = accept.accept().encode();
        assert!(HybridAccept::decode(&accept[..accept.len() - 1]).is_err());
        assert!(HybridAccept::decode(&[accept.as_slice(), &[0]].concat()).is_err());
    }

    #[test]
    fn hello_tampering_and_downgrade_fail_closed() {
        let fixture = fixture();
        let base = initiator(&fixture);
        let hello = base.hello().clone();

        let mut suite = hello.clone();
        suite.suite_id = SUITE_AES_256_GCM;
        assert!(matches!(
            responder(&fixture).accept(&suite, ENDPOINT, EPOCH, NOW),
            Err(HybridHandshakeError::Identity(_))
        ));

        let mut profile = hello.clone();
        profile.profile = 1;
        assert!(matches!(
            responder(&fixture).accept(&profile, ENDPOINT, EPOCH, NOW),
            Err(HybridHandshakeError::UnsupportedProfile(1))
        ));

        let mut endpoint = hello.clone();
        endpoint.endpoint_context_hash[0] ^= 1;
        assert!(matches!(
            responder(&fixture).accept(&endpoint, ENDPOINT, EPOCH, NOW),
            Err(HybridHandshakeError::EndpointContextMismatch)
        ));

        let mut timestamp = hello.clone();
        timestamp.issued_at -= HYBRID_TIMESTAMP_WINDOW_SECS + 1;
        assert!(matches!(
            responder(&fixture).accept(&timestamp, ENDPOINT, EPOCH, NOW),
            Err(HybridHandshakeError::TimestampOutsideWindow)
        ));

        let mut nonce = hello;
        nonce.initiator_nonce[0] ^= 1;
        assert!(matches!(
            responder(&fixture).accept(&nonce, ENDPOINT, EPOCH, NOW),
            Err(HybridHandshakeError::Identity(_))
        ));
    }

    #[test]
    fn accept_tampering_and_validly_signed_downgrade_fail_closed() {
        let fixture1 = fixture();
        let initiator1 = initiator(&fixture1);
        let hello1 = initiator1.hello().clone();
        let accepted = responder(&fixture1)
            .accept(&hello1, ENDPOINT, EPOCH, NOW)
            .unwrap();
        let accept = accepted.accept().clone();

        let mut suite = accept.clone();
        suite.suite_id = SUITE_AES_256_GCM;
        assert!(matches!(
            initiator1.finish(&suite, NOW, None),
            Err(HybridHandshakeError::AcceptMismatch)
        ));

        let fixture2 = fixture();
        let initiator2 = initiator(&fixture2);
        let hello = initiator2.hello().clone();
        let accepted = responder(&fixture2)
            .accept(&hello, ENDPOINT, EPOCH, NOW)
            .unwrap();
        let mut downgraded = accepted.accept().clone();
        downgraded.suite_id = SUITE_AES_256_GCM;
        let th2_input = downgraded.signing_hash(&hello.transcript_hash(), &hello.signature);
        downgraded.signature = responder_identity()
            .sign(ACCEPT_SIGNING_DOMAIN, &th2_input)
            .unwrap();
        assert!(matches!(
            initiator2.finish(&downgraded, NOW, None),
            Err(HybridHandshakeError::AcceptMismatch)
        ));

        let mut signature = accept.clone();
        let mut encoded = signature.signature.encode();
        encoded[0] ^= 1;
        signature.signature = CompositeSignature::decode(&encoded).unwrap();
        let fixture2 = fixture();
        let initiator2 = initiator(&fixture2);
        assert!(matches!(
            initiator2.finish(&signature, NOW, None),
            Err(HybridHandshakeError::Identity(_))
        ));

        let fixture3 = fixture();
        let initiator3 = initiator(&fixture3);
        let hello3 = initiator3.hello().clone();
        let accepted3 = responder(&fixture3)
            .accept(&hello3, ENDPOINT, EPOCH, NOW)
            .unwrap();
        let mut ciphertext = accepted3.accept().clone();
        let mut encoded = ciphertext.ciphertext.encode();
        encoded[0] ^= 1;
        ciphertext.ciphertext = XWingCiphertext::decode(&encoded).unwrap();
        assert!(initiator3.finish(&ciphertext, NOW, None).is_err());
    }

    #[test]
    fn classic_only_and_non_contributory_material_fail_closed() {
        let fixture = fixture();
        let initiator = initiator(&fixture);
        let mut encoded = initiator.hello().encode();
        let ephemeral_start = 1 + 1 + 2 + 32 + 32 + COMPOSITE_IDENTITY_PUBLIC_KEY_LEN;
        let ephemeral_x25519_start = ephemeral_start + XWING_ENCAPSULATION_KEY_LEN - 32;
        encoded[ephemeral_x25519_start..ephemeral_start + XWING_ENCAPSULATION_KEY_LEN].fill(0);
        assert!(matches!(
            HybridHello::decode(&encoded),
            Err(HybridHandshakeError::XWing(XWingError::NonContributory))
        ));

        let signature = initiator.hello().signature.encode();
        let mut classic_signature = initiator.hello().encode();
        classic_signature.truncate(classic_signature.len() - signature.len() + 64);
        assert!(HybridHello::decode(&classic_signature).is_err());

        let accepted = responder(&fixture)
            .accept(initiator.hello(), ENDPOINT, EPOCH, NOW)
            .unwrap();
        let mut encoded = accepted.accept().encode();
        let ciphertext_start =
            1 + 1 + 2 + SESSION_ID_LEN + 2 + 32 + 32 + COMPOSITE_IDENTITY_PUBLIC_KEY_LEN;
        let ciphertext_x25519_start = ciphertext_start + XWING_CIPHERTEXT_LEN - 32;
        encoded[ciphertext_x25519_start..ciphertext_start + XWING_CIPHERTEXT_LEN].fill(0);
        assert!(matches!(
            HybridAccept::decode(&encoded),
            Err(HybridHandshakeError::XWing(XWingError::NonContributory))
        ));
    }

    #[test]
    fn psk_mode_is_required_to_match_on_both_sides() {
        let fixture = fixture();
        let psk = [0x77; 32];
        let initiator = HybridInitiator::start(
            &initiator_identity(),
            fixture.responder_public.clone(),
            SUITE_CHACHA20_POLY1305,
            ENDPOINT,
            EPOCH,
            1,
            EMPTY_COOKIE,
            NOW,
        )
        .unwrap();
        let hello = initiator.hello().clone();
        let mut responder = HybridResponder::new(
            responder_identity(),
            fixture.initiator_public.clone(),
            Some(psk),
        )
        .unwrap();
        let accepted = responder.accept(&hello, ENDPOINT, EPOCH, NOW).unwrap();
        assert!(matches!(
            initiator.finish(accepted.accept(), NOW, None),
            Err(HybridHandshakeError::PskModeMismatch)
        ));

        let initiator = HybridInitiator::start(
            &initiator_identity(),
            fixture.responder_public.clone(),
            SUITE_CHACHA20_POLY1305,
            ENDPOINT,
            EPOCH,
            0,
            EMPTY_COOKIE,
            NOW,
        )
        .unwrap();
        assert!(matches!(
            HybridResponder::new(
                responder_identity(),
                fixture.initiator_public.clone(),
                Some(psk),
            )
            .unwrap()
            .accept(initiator.hello(), ENDPOINT, EPOCH, NOW),
            Err(HybridHandshakeError::PskModeMismatch)
        ));
    }

    #[test]
    fn replayed_hello_is_rejected_without_poisoning_the_cache() {
        let fixture = fixture();
        let initiator = initiator(&fixture);
        let hello = initiator.hello().clone();
        let mut responder = responder(&fixture);
        responder.accept(&hello, ENDPOINT, EPOCH, NOW).unwrap();
        assert!(matches!(
            responder.accept(&hello, ENDPOINT, EPOCH, NOW),
            Err(HybridHandshakeError::ReplayDetected)
        ));
    }

    #[test]
    fn debug_output_redacts_private_keys_and_psk() {
        let fixture = fixture();
        let psk = [0x77; 32];
        let responder = HybridResponder::new(
            responder_identity(),
            fixture.initiator_public.clone(),
            Some(psk),
        )
        .unwrap();
        let debug = format!("{responder:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("119, 119, 119"));
        let initiator = initiator(&fixture);
        assert!(!format!("{initiator:?}").contains("XWingDecapsulationKey("));
    }
}
