//! Experimental classic v2 handshake profile.
//!
//! This module implements the classic migration profile only: ephemeral
//! X25519 key agreement authenticated by long-term Ed25519 identities. It is
//! intentionally not a production v2 wire profile. The production suite in
//! `PROTOCOL_V2.md` requires X-Wing and both Ed25519/ML-DSA-65 signatures.
//!
//! `HELLO` and `ACCEPT` are typed control-channel messages. Their signature
//! inputs use length-prefixed, domain-separated SHA-512 transcripts. A final
//! [`HandshakeTranscript`] binds both identities, both ephemeral keys, the
//! suite, session identifier, epoch, and endpoint context.
//!
//! Replay protection has two independent layers:
//!
//! - `HELLO` timestamps must be within a bounded clock-skew window.
//! - A responder remembers every authenticated `HELLO` digest until its
//!   timestamp window expires and rejects duplicates.
//!
//! The replay cache is per responder instance. It is bounded and fails closed
//! when no slot can be reclaimed. Persistent replay state across process
//! restarts, stateless cookies, key confirmation, and key derivation are out of
//! scope for this profile stage and remain tracked by the security sprint.

use crate::transcript::{
    HandshakeTranscript, IdentityId, TranscriptError, TranscriptHash, ENDPOINT_CONTEXT_HASH_LEN,
    IDENTITY_ID_LEN, SUITE_AES_256_GCM, SUITE_CHACHA20_POLY1305, TRANSCRIPT_HASH_LEN,
};
use crate::{PROTOCOL_VERSION, SESSION_ID_LEN};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha512};
use std::collections::VecDeque;
use std::fmt;
use thiserror::Error;
use x25519_dalek::{PublicKey as X25519PublicKey, SharedSecret, StaticSecret};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

/// Marker byte for the experimental X25519 + Ed25519 profile.
pub const CLASSIC_PROFILE_ID: u8 = 1;
/// Domain for deriving a stable identity identifier from an Ed25519 key.
pub const IDENTITY_DOMAIN: &[u8] = b"fuckipv6-v2 classic ed25519 identity";
/// Domain for the initiator `HELLO` signature input.
pub const HELLO_SIGNING_DOMAIN: &[u8] = b"fuckipv6-v2 classic hello";
/// Domain for the responder `ACCEPT` signature input.
pub const ACCEPT_SIGNING_DOMAIN: &[u8] = b"fuckipv6-v2 classic accept";
/// Maximum accepted difference between a handshake timestamp and local time.
pub const HANDSHAKE_TIMESTAMP_WINDOW_SECS: u64 = 60;
/// Default maximum number of authenticated half-open `HELLO` digests retained.
pub const DEFAULT_REPLAY_CACHE_CAPACITY: usize = 4096;
/// Ed25519 public key size used by this classic profile.
pub const ED25519_PUBLIC_KEY_LEN: usize = 32;
/// Ed25519 signature size used by this classic profile.
pub const ED25519_SIGNATURE_LEN: usize = 64;
/// X25519 public key size used by this classic profile.
pub const X25519_PUBLIC_KEY_LEN: usize = 32;
/// Freshness nonce size used by this classic profile.
pub const HANDSHAKE_NONCE_LEN: usize = 32;

/// An Ed25519 long-term identity used for one side of the classic handshake.
#[derive(Clone)]
pub struct Ed25519Identity {
    signing_key: SigningKey,
}

impl Ed25519Identity {
    /// Generate a fresh identity from the operating system CSPRNG.
    pub fn generate() -> Result<Self, HandshakeError> {
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).map_err(|_| HandshakeError::RandomnessUnavailable)?;
        let identity = Self::from_seed(seed);
        seed.zeroize();
        Ok(identity)
    }

    /// Construct an identity from a 32-byte Ed25519 seed.
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self {
            signing_key: SigningKey::from_bytes(&seed),
        }
    }

    /// Return the public verification key.
    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing_key.verifying_key()
    }

    /// Return the stable identity identifier bound into handshake transcripts.
    pub fn identity_id(&self) -> IdentityId {
        identity_id(&self.verifying_key())
    }

    /// Sign a handshake hash. Callers normally use the state-machine APIs.
    pub fn sign(&self, message: &[u8]) -> Signature {
        self.signing_key.sign(message)
    }
}

/// Derive the stable 256-bit identity identifier for an Ed25519 public key.
pub fn identity_id(public_key: &VerifyingKey) -> IdentityId {
    let bytes = public_key.as_bytes();
    let canonical = canonical_bytes(IDENTITY_DOMAIN, &[bytes]);
    let digest = Sha512::digest(canonical);
    let mut identity = [0u8; IDENTITY_ID_LEN];
    identity.copy_from_slice(&digest[..IDENTITY_ID_LEN]);
    identity
}

/// Initiator `HELLO` control message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    pub version: u8,
    pub profile: u8,
    pub suite_id: u16,
    pub initiator_identity: IdentityId,
    pub responder_identity: IdentityId,
    pub initiator_ephemeral_public: [u8; X25519_PUBLIC_KEY_LEN],
    pub initiator_nonce: [u8; HANDSHAKE_NONCE_LEN],
    pub issued_at: u64,
    pub endpoint_context_hash: [u8; ENDPOINT_CONTEXT_HASH_LEN],
    pub signature: [u8; ED25519_SIGNATURE_LEN],
}

impl Hello {
    /// Hash the canonical unsigned `HELLO` fields. The Ed25519 signature signs
    /// this digest, and `ACCEPT` additionally binds the exact signature bytes.
    pub fn signing_hash(&self) -> TranscriptHash {
        let version = [self.version];
        let profile = [self.profile];
        let suite = self.suite_id.to_be_bytes();
        let issued_at = self.issued_at.to_be_bytes();
        let canonical = canonical_bytes(
            HELLO_SIGNING_DOMAIN,
            &[
                &version,
                &profile,
                &suite,
                &self.initiator_identity,
                &self.responder_identity,
                &self.initiator_ephemeral_public,
                &self.initiator_nonce,
                &issued_at,
                &self.endpoint_context_hash,
            ],
        );
        sha512(&canonical)
    }
}

/// Responder `ACCEPT` control message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accept {
    pub version: u8,
    pub profile: u8,
    pub suite_id: u16,
    pub session_id: [u8; SESSION_ID_LEN],
    pub epoch: u16,
    pub initiator_identity: IdentityId,
    pub responder_identity: IdentityId,
    pub initiator_ephemeral_public: [u8; X25519_PUBLIC_KEY_LEN],
    pub responder_ephemeral_public: [u8; X25519_PUBLIC_KEY_LEN],
    pub initiator_nonce: [u8; HANDSHAKE_NONCE_LEN],
    pub responder_nonce: [u8; HANDSHAKE_NONCE_LEN],
    pub hello_issued_at: u64,
    pub issued_at: u64,
    pub endpoint_context_hash: [u8; ENDPOINT_CONTEXT_HASH_LEN],
    pub signature: [u8; ED25519_SIGNATURE_LEN],
}

impl Accept {
    /// Hash the canonical `ACCEPT` signature input. `hello_hash` and
    /// `hello_signature` are supplied from the exact validated `HELLO`.
    pub fn signing_hash(
        &self,
        hello_hash: &TranscriptHash,
        hello_signature: &[u8; ED25519_SIGNATURE_LEN],
    ) -> TranscriptHash {
        let version = [self.version];
        let profile = [self.profile];
        let suite = self.suite_id.to_be_bytes();
        let epoch = self.epoch.to_be_bytes();
        let hello_issued_at = self.hello_issued_at.to_be_bytes();
        let issued_at = self.issued_at.to_be_bytes();
        let canonical = canonical_bytes(
            ACCEPT_SIGNING_DOMAIN,
            &[
                &version,
                &profile,
                &suite,
                hello_hash,
                hello_signature,
                &self.session_id,
                &epoch,
                &self.initiator_identity,
                &self.responder_identity,
                &self.initiator_ephemeral_public,
                &self.responder_ephemeral_public,
                &self.initiator_nonce,
                &self.responder_nonce,
                &hello_issued_at,
                &issued_at,
                &self.endpoint_context_hash,
            ],
        );
        sha512(&canonical)
    }
}

/// Non-secret fields shared by both endpoints after a successful handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandshakeIdentity {
    pub suite_id: u16,
    pub session_id: [u8; SESSION_ID_LEN],
    pub epoch: u16,
    pub initiator_identity: IdentityId,
    pub responder_identity: IdentityId,
    pub endpoint_context_hash: [u8; ENDPOINT_CONTEXT_HASH_LEN],
    pub transcript_hash: TranscriptHash,
}

/// A zeroizing X25519 shared secret produced by the classic handshake.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SharedHandshakeSecret {
    bytes: [u8; 32],
}

impl SharedHandshakeSecret {
    /// Borrow the shared secret for binding into the next KDF stage.
    pub fn expose_secret(&self) -> &[u8; 32] {
        &self.bytes
    }
}

/// Successful classic handshake output.
///
/// The shared secret is not present in the transcript. It must be passed
/// through a domain-separated HKDF in the next stage before use by an AEAD.
pub struct EstablishedHandshake {
    pub identity: HandshakeIdentity,
    pub hello_hash: TranscriptHash,
    pub accept_hash: TranscriptHash,
    secret: SharedHandshakeSecret,
}

impl EstablishedHandshake {
    /// Borrow the X25519 shared secret.
    pub fn shared_secret(&self) -> &SharedHandshakeSecret {
        &self.secret
    }

    /// Consume the handshake and move the shared secret into a zeroizing box.
    pub fn into_shared_secret(self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(*self.secret.expose_secret())
    }
}

impl fmt::Debug for EstablishedHandshake {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EstablishedHandshake")
            .field("identity", &self.identity)
            .field("hello_hash", &self.hello_hash)
            .field("accept_hash", &self.accept_hash)
            .field("shared_secret", &"<redacted>")
            .finish()
    }
}

/// Initiator state for one classic handshake attempt.
pub struct ClassicInitiator {
    hello: Hello,
    ephemeral_secret: StaticSecret,
    expected_responder: VerifyingKey,
    requested_epoch: u16,
}

impl ClassicInitiator {
    /// Create a fresh `HELLO` with a new X25519 ephemeral key and nonce.
    ///
    /// `issued_at` is Unix time in seconds and must be compared against the
    /// responder's trusted clock. `epoch` is the data epoch proposed for the
    /// eventual session.
    pub fn start(
        local_identity: &Ed25519Identity,
        expected_responder: VerifyingKey,
        suite_id: u16,
        endpoint_context_hash: [u8; ENDPOINT_CONTEXT_HASH_LEN],
        epoch: u16,
        issued_at: u64,
    ) -> Result<Self, HandshakeError> {
        validate_suite(suite_id)?;
        validate_endpoint_context(endpoint_context_hash)?;

        let initiator_identity = local_identity.identity_id();
        let responder_identity = identity_id(&expected_responder);
        validate_identity_pair(initiator_identity, responder_identity)?;

        let mut ephemeral_bytes = [0u8; 32];
        getrandom::fill(&mut ephemeral_bytes).map_err(|_| HandshakeError::RandomnessUnavailable)?;
        let ephemeral_secret = StaticSecret::from(ephemeral_bytes);
        ephemeral_bytes.zeroize();
        let ephemeral_public = X25519PublicKey::from(&ephemeral_secret).to_bytes();
        validate_ephemeral_public(&ephemeral_public, "initiator")?;

        let mut initiator_nonce = [0u8; HANDSHAKE_NONCE_LEN];
        getrandom::fill(&mut initiator_nonce).map_err(|_| HandshakeError::RandomnessUnavailable)?;
        validate_nonce(&initiator_nonce, "initiator")?;

        let mut hello = Hello {
            version: PROTOCOL_VERSION,
            profile: CLASSIC_PROFILE_ID,
            suite_id,
            initiator_identity,
            responder_identity,
            initiator_ephemeral_public: ephemeral_public,
            initiator_nonce,
            issued_at,
            endpoint_context_hash,
            signature: [0; ED25519_SIGNATURE_LEN],
        };
        let signing_hash = hello.signing_hash();
        hello.signature = local_identity.sign(&signing_hash).to_bytes();

        Ok(Self {
            hello,
            ephemeral_secret,
            expected_responder,
            requested_epoch: epoch,
        })
    }

    /// Borrow the generated `HELLO`.
    pub const fn hello(&self) -> &Hello {
        &self.hello
    }

    /// Validate and finish the handshake using a responder `ACCEPT`.
    ///
    /// `now` is Unix time in seconds from the initiator's trusted clock.
    pub fn finish(self, accept: &Accept, now: u64) -> Result<EstablishedHandshake, HandshakeError> {
        validate_accept_matches_hello(&self.hello, accept)?;
        if accept.epoch != self.requested_epoch {
            return Err(HandshakeError::AcceptMismatch);
        }
        validate_timestamp(accept.issued_at, now)?;
        if accept.session_id == [0; SESSION_ID_LEN] {
            return Err(HandshakeError::InvalidSessionId);
        }
        validate_ephemeral_public(&accept.responder_ephemeral_public, "responder")?;

        let hello_hash = self.hello.signing_hash();
        let accept_hash = accept.signing_hash(&hello_hash, &self.hello.signature);
        verify_signature(
            &self.expected_responder,
            &accept_hash,
            &accept.signature,
            "responder",
        )?;

        let responder_public = X25519PublicKey::from(accept.responder_ephemeral_public);
        let shared = self.ephemeral_secret.diffie_hellman(&responder_public);
        validate_shared_secret(&shared)?;

        let transcript_hash = transcript_hash(&self.hello, accept)?;
        let identity = handshake_identity(accept, transcript_hash);

        Ok(EstablishedHandshake {
            identity,
            hello_hash,
            accept_hash,
            secret: SharedHandshakeSecret {
                bytes: *shared.as_bytes(),
            },
        })
    }
}

/// Bounded replay cache used by one responder instance.
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

    fn insert(&mut self, digest: TranscriptHash, expires_at: u64) -> Result<(), HandshakeError> {
        if self.entries.len() >= self.capacity {
            return Err(HandshakeError::ReplayCacheFull {
                capacity: self.capacity,
            });
        }
        self.entries.push_back((digest, expires_at));
        Ok(())
    }
}

/// Responder state for the classic handshake profile.
pub struct ClassicResponder {
    local_identity: Ed25519Identity,
    expected_initiator: VerifyingKey,
    replay_cache: ReplayCache,
}

impl ClassicResponder {
    /// Create a responder restricted to one expected initiator identity.
    pub fn new(
        local_identity: Ed25519Identity,
        expected_initiator: VerifyingKey,
    ) -> Result<Self, HandshakeError> {
        let local_id = local_identity.identity_id();
        let peer_id = identity_id(&expected_initiator);
        validate_identity_pair(peer_id, local_id)?;
        Ok(Self {
            local_identity,
            expected_initiator,
            replay_cache: ReplayCache::new(DEFAULT_REPLAY_CACHE_CAPACITY),
        })
    }

    /// Create a responder with an explicit positive replay-cache capacity.
    pub fn with_replay_capacity(
        local_identity: Ed25519Identity,
        expected_initiator: VerifyingKey,
        capacity: usize,
    ) -> Result<Self, HandshakeError> {
        if capacity == 0 {
            return Err(HandshakeError::InvalidReplayCacheCapacity);
        }
        let local_id = local_identity.identity_id();
        let peer_id = identity_id(&expected_initiator);
        validate_identity_pair(peer_id, local_id)?;
        Ok(Self {
            local_identity,
            expected_initiator,
            replay_cache: ReplayCache::new(capacity),
        })
    }

    /// Validate one `HELLO`, consume it, and produce a signed `ACCEPT` plus the
    /// responder-side handshake output.
    ///
    /// `now` is Unix time in seconds from the responder's trusted clock.
    pub fn accept(
        &mut self,
        hello: &Hello,
        expected_endpoint_context: [u8; ENDPOINT_CONTEXT_HASH_LEN],
        epoch: u16,
        now: u64,
    ) -> Result<AcceptedHandshake, HandshakeError> {
        self.validate_hello(hello, expected_endpoint_context, now)?;

        let hello_hash = hello.signing_hash();
        if self.replay_cache.contains(&hello_hash) {
            return Err(HandshakeError::ReplayDetected);
        }
        self.replay_cache.prune(now);
        if self.replay_cache.contains(&hello_hash) {
            return Err(HandshakeError::ReplayDetected);
        }

        let mut ephemeral_bytes = [0u8; 32];
        getrandom::fill(&mut ephemeral_bytes).map_err(|_| HandshakeError::RandomnessUnavailable)?;
        let ephemeral_secret = StaticSecret::from(ephemeral_bytes);
        ephemeral_bytes.zeroize();
        let responder_ephemeral_public = X25519PublicKey::from(&ephemeral_secret).to_bytes();
        validate_ephemeral_public(&responder_ephemeral_public, "responder")?;

        let mut session_id = [0u8; SESSION_ID_LEN];
        getrandom::fill(&mut session_id).map_err(|_| HandshakeError::RandomnessUnavailable)?;
        if session_id == [0; SESSION_ID_LEN] {
            return Err(HandshakeError::InvalidSessionId);
        }

        let mut responder_nonce = [0u8; HANDSHAKE_NONCE_LEN];
        getrandom::fill(&mut responder_nonce).map_err(|_| HandshakeError::RandomnessUnavailable)?;
        validate_nonce(&responder_nonce, "responder")?;

        let initiator_public = X25519PublicKey::from(hello.initiator_ephemeral_public);
        let shared = ephemeral_secret.diffie_hellman(&initiator_public);
        validate_shared_secret(&shared)?;

        let mut accept = Accept {
            version: PROTOCOL_VERSION,
            profile: CLASSIC_PROFILE_ID,
            suite_id: hello.suite_id,
            session_id,
            epoch,
            initiator_identity: hello.initiator_identity,
            responder_identity: hello.responder_identity,
            initiator_ephemeral_public: hello.initiator_ephemeral_public,
            responder_ephemeral_public,
            initiator_nonce: hello.initiator_nonce,
            responder_nonce,
            hello_issued_at: hello.issued_at,
            issued_at: now,
            endpoint_context_hash: hello.endpoint_context_hash,
            signature: [0; ED25519_SIGNATURE_LEN],
        };
        let accept_hash = accept.signing_hash(&hello_hash, &hello.signature);
        accept.signature = self.local_identity.sign(&accept_hash).to_bytes();

        let transcript_hash = transcript_hash(hello, &accept)?;
        let identity = handshake_identity(&accept, transcript_hash);

        self.replay_cache.insert(
            hello_hash,
            now.saturating_add(HANDSHAKE_TIMESTAMP_WINDOW_SECS),
        )?;

        Ok(AcceptedHandshake {
            accept,
            established: EstablishedHandshake {
                identity,
                hello_hash,
                accept_hash,
                secret: SharedHandshakeSecret {
                    bytes: *shared.as_bytes(),
                },
            },
        })
    }

    fn validate_hello(
        &self,
        hello: &Hello,
        expected_endpoint_context: [u8; ENDPOINT_CONTEXT_HASH_LEN],
        now: u64,
    ) -> Result<(), HandshakeError> {
        if hello.version != PROTOCOL_VERSION {
            return Err(HandshakeError::UnsupportedVersion(hello.version));
        }
        if hello.profile != CLASSIC_PROFILE_ID {
            return Err(HandshakeError::UnsupportedProfile(hello.profile));
        }
        validate_suite(hello.suite_id)?;
        if hello.initiator_identity != identity_id(&self.expected_initiator) {
            return Err(HandshakeError::IdentityMismatch("initiator"));
        }
        if hello.responder_identity != self.local_identity.identity_id() {
            return Err(HandshakeError::IdentityMismatch("responder"));
        }
        validate_identity_pair(hello.initiator_identity, hello.responder_identity)?;
        validate_endpoint_context(hello.endpoint_context_hash)?;
        if hello.endpoint_context_hash != expected_endpoint_context {
            return Err(HandshakeError::EndpointContextMismatch);
        }
        validate_timestamp(hello.issued_at, now)?;
        validate_ephemeral_public(&hello.initiator_ephemeral_public, "initiator")?;
        validate_nonce(&hello.initiator_nonce, "initiator")?;
        verify_signature(
            &self.expected_initiator,
            &hello.signing_hash(),
            &hello.signature,
            "initiator",
        )
    }
}

/// Successful responder-side validation result.
#[derive(Debug)]
pub struct AcceptedHandshake {
    pub accept: Accept,
    pub established: EstablishedHandshake,
}

/// Errors returned by the classic handshake profile.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeError {
    #[error("operating system randomness is unavailable")]
    RandomnessUnavailable,
    #[error("unsupported protocol version {0}")]
    UnsupportedVersion(u8),
    #[error("unsupported handshake profile {0}")]
    UnsupportedProfile(u8),
    #[error("unsupported v2 suite {0:#06x}")]
    UnsupportedSuite(u16),
    #[error("{0} identity must not be all zero")]
    InvalidIdentity(&'static str),
    #[error("initiator and responder identities must differ")]
    DuplicateIdentity,
    #[error("{0} identity does not match the configured public key")]
    IdentityMismatch(&'static str),
    #[error("endpoint context hash must not be all zero")]
    InvalidEndpointContext,
    #[error("endpoint context hash does not match the expected context")]
    EndpointContextMismatch,
    #[error("handshake timestamp is outside the accepted clock-skew window")]
    TimestampOutsideWindow,
    #[error("{0} ephemeral public key must not be all zero")]
    InvalidEphemeralPublic(&'static str),
    #[error("{0} freshness nonce must not be all zero")]
    InvalidNonce(&'static str),
    #[error("session identifier must not be all zero")]
    InvalidSessionId,
    #[error("{0} handshake signature is invalid")]
    InvalidSignature(&'static str),
    #[error("X25519 produced a non-contributory shared secret")]
    NonContributoryKeyExchange,
    #[error("HELLO was already consumed by this responder")]
    ReplayDetected,
    #[error("replay cache is full ({capacity} unexpired entries)")]
    ReplayCacheFull { capacity: usize },
    #[error("replay cache capacity must be greater than zero")]
    InvalidReplayCacheCapacity,
    #[error("ACCEPT does not match the originating HELLO")]
    AcceptMismatch,
    #[error("handshake transcript is invalid: {0}")]
    Transcript(#[from] TranscriptError),
}

fn validate_suite(suite_id: u16) -> Result<(), HandshakeError> {
    if matches!(suite_id, SUITE_AES_256_GCM | SUITE_CHACHA20_POLY1305) {
        Ok(())
    } else {
        Err(HandshakeError::UnsupportedSuite(suite_id))
    }
}

fn validate_endpoint_context(
    endpoint_context_hash: [u8; ENDPOINT_CONTEXT_HASH_LEN],
) -> Result<(), HandshakeError> {
    if endpoint_context_hash == [0; ENDPOINT_CONTEXT_HASH_LEN] {
        Err(HandshakeError::InvalidEndpointContext)
    } else {
        Ok(())
    }
}

fn validate_identity_pair(
    initiator: IdentityId,
    responder: IdentityId,
) -> Result<(), HandshakeError> {
    if initiator == [0; IDENTITY_ID_LEN] {
        return Err(HandshakeError::InvalidIdentity("initiator"));
    }
    if responder == [0; IDENTITY_ID_LEN] {
        return Err(HandshakeError::InvalidIdentity("responder"));
    }
    if initiator == responder {
        return Err(HandshakeError::DuplicateIdentity);
    }
    Ok(())
}

fn validate_timestamp(timestamp: u64, now: u64) -> Result<(), HandshakeError> {
    if now.abs_diff(timestamp) > HANDSHAKE_TIMESTAMP_WINDOW_SECS {
        Err(HandshakeError::TimestampOutsideWindow)
    } else {
        Ok(())
    }
}

fn validate_ephemeral_public(
    public: &[u8; X25519_PUBLIC_KEY_LEN],
    role: &'static str,
) -> Result<(), HandshakeError> {
    if *public == [0; X25519_PUBLIC_KEY_LEN] {
        Err(HandshakeError::InvalidEphemeralPublic(role))
    } else {
        Ok(())
    }
}

fn validate_nonce(
    nonce: &[u8; HANDSHAKE_NONCE_LEN],
    role: &'static str,
) -> Result<(), HandshakeError> {
    if *nonce == [0; HANDSHAKE_NONCE_LEN] {
        Err(HandshakeError::InvalidNonce(role))
    } else {
        Ok(())
    }
}

fn validate_shared_secret(shared: &SharedSecret) -> Result<(), HandshakeError> {
    if shared.was_contributory() {
        Ok(())
    } else {
        Err(HandshakeError::NonContributoryKeyExchange)
    }
}

fn verify_signature(
    public_key: &VerifyingKey,
    message: &TranscriptHash,
    signature: &[u8; ED25519_SIGNATURE_LEN],
    role: &'static str,
) -> Result<(), HandshakeError> {
    let signature = Signature::from_bytes(signature);
    public_key
        .verify_strict(message, &signature)
        .map_err(|_| HandshakeError::InvalidSignature(role))
}

fn validate_accept_matches_hello(hello: &Hello, accept: &Accept) -> Result<(), HandshakeError> {
    if accept.version != PROTOCOL_VERSION {
        return Err(HandshakeError::UnsupportedVersion(accept.version));
    }
    if accept.profile != CLASSIC_PROFILE_ID {
        return Err(HandshakeError::UnsupportedProfile(accept.profile));
    }
    validate_suite(accept.suite_id)?;
    if accept.suite_id != hello.suite_id
        || accept.initiator_identity != hello.initiator_identity
        || accept.responder_identity != hello.responder_identity
        || accept.initiator_ephemeral_public != hello.initiator_ephemeral_public
        || accept.initiator_nonce != hello.initiator_nonce
        || accept.hello_issued_at != hello.issued_at
        || accept.endpoint_context_hash != hello.endpoint_context_hash
    {
        return Err(HandshakeError::AcceptMismatch);
    }
    validate_endpoint_context(accept.endpoint_context_hash)?;
    validate_identity_pair(accept.initiator_identity, accept.responder_identity)?;
    validate_nonce(&accept.responder_nonce, "responder")
}

fn transcript_hash(hello: &Hello, accept: &Accept) -> Result<TranscriptHash, TranscriptError> {
    HandshakeTranscript {
        suite_id: accept.suite_id,
        initiator_identity: hello.initiator_identity,
        responder_identity: hello.responder_identity,
        initiator_ephemeral_public: &hello.initiator_ephemeral_public,
        responder_ephemeral_public: &accept.responder_ephemeral_public,
        session_id: accept.session_id,
        epoch: accept.epoch,
        endpoint_context_hash: accept.endpoint_context_hash,
    }
    .hash()
}

fn handshake_identity(accept: &Accept, transcript_hash: TranscriptHash) -> HandshakeIdentity {
    HandshakeIdentity {
        suite_id: accept.suite_id,
        session_id: accept.session_id,
        epoch: accept.epoch,
        initiator_identity: accept.initiator_identity,
        responder_identity: accept.responder_identity,
        endpoint_context_hash: accept.endpoint_context_hash,
        transcript_hash,
    }
}

fn canonical_bytes(domain: &[u8], fields: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(256);
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

fn sha512(bytes: &[u8]) -> TranscriptHash {
    let digest = Sha512::digest(bytes);
    let mut hash = [0u8; TRANSCRIPT_HASH_LEN];
    hash.copy_from_slice(&digest);
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_700_000_000;
    const EPOCH: u16 = 7;
    const ENDPOINT: [u8; ENDPOINT_CONTEXT_HASH_LEN] = [0x5a; ENDPOINT_CONTEXT_HASH_LEN];

    fn initiator_identity() -> Ed25519Identity {
        Ed25519Identity::from_seed([0x11; 32])
    }

    fn responder_identity() -> Ed25519Identity {
        Ed25519Identity::from_seed([0x22; 32])
    }

    fn start() -> ClassicInitiator {
        ClassicInitiator::start(
            &initiator_identity(),
            responder_identity().verifying_key(),
            SUITE_CHACHA20_POLY1305,
            ENDPOINT,
            EPOCH,
            NOW,
        )
        .expect("start handshake")
    }

    fn run() -> (EstablishedHandshake, EstablishedHandshake) {
        let initiator = start();
        let hello = initiator.hello().clone();
        let mut responder =
            ClassicResponder::new(responder_identity(), initiator_identity().verifying_key())
                .expect("responder");
        let accepted = responder
            .accept(&hello, ENDPOINT, EPOCH, NOW)
            .expect("accept handshake");
        let initiator_established = initiator
            .finish(&accepted.accept, NOW)
            .expect("finish handshake");
        (initiator_established, accepted.established)
    }

    #[test]
    fn both_endpoints_establish_the_same_transcript_and_x25519_secret() {
        let (initiator, responder) = run();
        assert_eq!(initiator.identity, responder.identity);
        assert_eq!(initiator.hello_hash, responder.hello_hash);
        assert_eq!(initiator.accept_hash, responder.accept_hash);
        assert_eq!(
            initiator.shared_secret().expose_secret(),
            responder.shared_secret().expose_secret()
        );
        assert_eq!(initiator.identity.epoch, EPOCH);
        assert_eq!(initiator.identity.suite_id, SUITE_CHACHA20_POLY1305);
    }

    #[test]
    fn hello_and_accept_bind_fresh_nonce_key_and_session_id() {
        let first_hello = start().hello().clone();
        let second_hello = start().hello().clone();
        assert_ne!(
            first_hello.initiator_ephemeral_public,
            second_hello.initiator_ephemeral_public
        );
        assert_ne!(first_hello.initiator_nonce, second_hello.initiator_nonce);
        assert_ne!(first_hello.signature, second_hello.signature);

        let (first, _) = run();
        let (second, _) = run();
        assert_ne!(first.identity.session_id, second.identity.session_id);
        assert_ne!(
            first.identity.transcript_hash,
            second.identity.transcript_hash
        );
    }

    #[test]
    fn replayed_hello_is_rejected() {
        let initiator = start();
        let hello = initiator.hello().clone();
        let mut responder =
            ClassicResponder::new(responder_identity(), initiator_identity().verifying_key())
                .expect("responder");
        responder
            .accept(&hello, ENDPOINT, EPOCH, NOW)
            .expect("first hello");
        assert_eq!(
            responder
                .accept(&hello, ENDPOINT, EPOCH, NOW + 1)
                .unwrap_err(),
            HandshakeError::ReplayDetected
        );
    }

    #[test]
    fn stale_hello_is_rejected_even_without_replay_state() {
        let initiator = start();
        let mut responder =
            ClassicResponder::new(responder_identity(), initiator_identity().verifying_key())
                .expect("responder");
        assert_eq!(
            responder
                .accept(
                    initiator.hello(),
                    ENDPOINT,
                    EPOCH,
                    NOW + HANDSHAKE_TIMESTAMP_WINDOW_SECS + 1
                )
                .unwrap_err(),
            HandshakeError::TimestampOutsideWindow
        );
    }

    #[test]
    fn future_accept_is_rejected() {
        let initiator = start();
        let hello = initiator.hello().clone();
        let mut responder =
            ClassicResponder::new(responder_identity(), initiator_identity().verifying_key())
                .expect("responder");
        let accepted = responder
            .accept(&hello, ENDPOINT, EPOCH, NOW)
            .expect("accept");
        assert_eq!(
            initiator
                .finish(&accepted.accept, NOW + HANDSHAKE_TIMESTAMP_WINDOW_SECS + 1)
                .unwrap_err(),
            HandshakeError::TimestampOutsideWindow
        );
    }

    #[test]
    fn hello_signature_tampering_fails_closed() {
        let initiator = start();
        let mut hello = initiator.hello().clone();
        hello.initiator_nonce[0] ^= 1;
        let mut responder =
            ClassicResponder::new(responder_identity(), initiator_identity().verifying_key())
                .expect("responder");
        assert_eq!(
            responder.accept(&hello, ENDPOINT, EPOCH, NOW).unwrap_err(),
            HandshakeError::InvalidSignature("initiator")
        );
    }

    #[test]
    fn accept_signature_tampering_fails_closed() {
        let initiator = start();
        let hello = initiator.hello().clone();
        let mut responder =
            ClassicResponder::new(responder_identity(), initiator_identity().verifying_key())
                .expect("responder");
        let mut accepted = responder
            .accept(&hello, ENDPOINT, EPOCH, NOW)
            .expect("accept");
        accepted.accept.signature[0] ^= 1;
        assert_eq!(
            initiator.finish(&accepted.accept, NOW).unwrap_err(),
            HandshakeError::InvalidSignature("responder")
        );
    }

    #[test]
    fn wrong_identity_and_wrong_endpoint_are_rejected() {
        let initiator = start();
        let hello = initiator.hello().clone();
        let wrong_peer = Ed25519Identity::from_seed([0x33; 32]);
        let mut wrong_identity_responder =
            ClassicResponder::new(responder_identity(), wrong_peer.verifying_key())
                .expect("responder");
        assert_eq!(
            wrong_identity_responder
                .accept(&hello, ENDPOINT, EPOCH, NOW)
                .unwrap_err(),
            HandshakeError::IdentityMismatch("initiator")
        );

        let mut responder =
            ClassicResponder::new(responder_identity(), initiator_identity().verifying_key())
                .expect("responder");
        assert_eq!(
            responder
                .accept(&hello, [0x6b; ENDPOINT_CONTEXT_HASH_LEN], EPOCH, NOW)
                .unwrap_err(),
            HandshakeError::EndpointContextMismatch
        );
    }

    #[test]
    fn unsupported_suite_and_zero_ephemeral_key_are_rejected() {
        assert!(matches!(
            ClassicInitiator::start(
                &initiator_identity(),
                responder_identity().verifying_key(),
                0x9999,
                ENDPOINT,
                EPOCH,
                NOW,
            ),
            Err(HandshakeError::UnsupportedSuite(0x9999))
        ));

        let identity = initiator_identity();
        let mut hello = start().hello().clone();
        hello.initiator_ephemeral_public = [0; X25519_PUBLIC_KEY_LEN];
        hello.signature = identity.sign(&hello.signing_hash()).to_bytes();
        let mut responder = ClassicResponder::new(responder_identity(), identity.verifying_key())
            .expect("responder");
        assert_eq!(
            responder.accept(&hello, ENDPOINT, EPOCH, NOW).unwrap_err(),
            HandshakeError::InvalidEphemeralPublic("initiator")
        );
    }

    #[test]
    fn replay_cache_fails_closed_when_full() {
        let first = start();
        let second = start();
        let mut responder = ClassicResponder::with_replay_capacity(
            responder_identity(),
            initiator_identity().verifying_key(),
            1,
        )
        .expect("responder");
        responder
            .accept(first.hello(), ENDPOINT, EPOCH, NOW)
            .expect("first");
        assert_eq!(
            responder
                .accept(second.hello(), ENDPOINT, EPOCH, NOW)
                .unwrap_err(),
            HandshakeError::ReplayCacheFull { capacity: 1 }
        );
    }

    #[test]
    fn canonical_hello_signing_hash_matches_fixed_vector() {
        let hello = Hello {
            version: PROTOCOL_VERSION,
            profile: CLASSIC_PROFILE_ID,
            suite_id: SUITE_CHACHA20_POLY1305,
            initiator_identity: [0x11; IDENTITY_ID_LEN],
            responder_identity: [0x22; IDENTITY_ID_LEN],
            initiator_ephemeral_public: [0x33; X25519_PUBLIC_KEY_LEN],
            initiator_nonce: [0x44; HANDSHAKE_NONCE_LEN],
            issued_at: NOW,
            endpoint_context_hash: ENDPOINT,
            signature: [0; ED25519_SIGNATURE_LEN],
        };
        assert_eq!(
            hello.signing_hash(),
            [
                97, 25, 186, 193, 188, 152, 129, 236, 85, 195, 8, 111, 15, 61, 70, 239, 57, 130,
                244, 181, 236, 228, 117, 6, 70, 52, 214, 157, 114, 215, 97, 196, 82, 6, 44, 39,
                238, 163, 155, 219, 125, 29, 50, 40, 171, 104, 175, 240, 112, 49, 114, 210, 217,
                20, 63, 2, 145, 72, 22, 115, 83, 16, 111, 241,
            ]
        );
    }
}
