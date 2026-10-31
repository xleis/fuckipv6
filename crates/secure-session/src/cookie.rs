//! Stateless source-reachability cookies for protocol v2.
//!
//! A cookie is an HMAC-SHA-256 capability bound to the observed source
//! address, both handshake identity IDs, a hash of the initiator ephemeral
//! public key, and its issue time. The responder can therefore issue and later
//! validate a challenge without retaining per-client pre-handshake state.
//!
//! Cookies prove source reachability; they do not authenticate a peer and do
//! not make an exact `HELLO` replay impossible. A valid cookie must still pass
//! signature verification, the bounded handshake replay cache, and the
//! configured per-source/per-identity rate limits. Reusing a cookie for a
//! different source, identity, or ephemeral key fails the MAC.

use crate::handshake::Hello;
use crate::transcript::IDENTITY_ID_LEN;
use crate::PROTOCOL_VERSION;
use getrandom::fill as fill_random;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use thiserror::Error;
use zeroize::Zeroizing;

type HmacSha256 = Hmac<Sha256>;
type ZeroizingCookieKey = Zeroizing<[u8; COOKIE_KEY_LEN]>;

/// Domain separating cookie MACs from all other v2 keyed operations.
pub const COOKIE_MAC_DOMAIN: &[u8] = b"fuckipv6-v2 stateless cookie mac";
/// Domain separating ephemeral-key hashes from identity and transcript hashes.
pub const EPHEMERAL_KEY_HASH_DOMAIN: &[u8] = b"fuckipv6-v2 ephemeral key hash";
/// Domain separating epoch-derived cookie keys from other HMAC uses.
const COOKIE_ROTATION_KEY_DOMAIN: &[u8] = b"fuckipv6-v2 cookie rotation key";
/// Version of the stateless cookie wire format.
pub const COOKIE_VERSION: u8 = 1;
/// Required cookie master-key length.
pub const COOKIE_KEY_LEN: usize = 32;
/// Size of the full HMAC-SHA-256 cookie tag.
pub const COOKIE_MAC_LEN: usize = 32;
/// Size of the big-endian issuance timestamp.
pub const COOKIE_TIMESTAMP_LEN: usize = 8;
/// Exact encoded cookie length: version, timestamp, and MAC.
pub const COOKIE_WIRE_LEN: usize = 1 + COOKIE_TIMESTAMP_LEN + COOKIE_MAC_LEN;
/// Default maximum age of a valid cookie.
pub const COOKIE_LIFETIME_SECS: u64 = 30;
/// Default maximum accepted future skew for a cookie timestamp.
pub const COOKIE_FUTURE_SKEW_SECS: u64 = 2;
/// Production cookie-key rotation interval.
///
/// The value must not be shorter than the cookie lifetime so that an
/// unexpired cookie can cross at most one rotation boundary and remain
/// verifiable with the current or immediately previous generation.
pub const COOKIE_KEY_ROTATION_SECS: u64 = 60;
/// Defensive upper bound for a configurable cookie lifetime.
pub const MAX_COOKIE_LIFETIME_SECS: u64 = 300;
/// Challenge packet kind. It is distinct from v2 data and rekey frames.
pub const COOKIE_CHALLENGE_KIND: u8 = 0x21;
/// Challenge prefix containing kind and protocol version.
pub const COOKIE_CHALLENGE_HEADER_LEN: usize = 2;
/// Exact challenge response length.
pub const COOKIE_CHALLENGE_WIRE_LEN: usize = COOKIE_CHALLENGE_HEADER_LEN + COOKIE_WIRE_LEN;

/// A canonical network source address bound into a cookie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceAddress {
    /// IPv4 address and UDP/control source port.
    Ipv4 { address: [u8; 4], port: u16 },
    /// IPv6 address and UDP/control source port.
    Ipv6 { address: [u8; 16], port: u16 },
}

impl SourceAddress {
    fn encode(self) -> [u8; 19] {
        let mut encoded = [0u8; 19];
        match self {
            Self::Ipv4 { address, port } => {
                encoded[0] = 4;
                encoded[1..3].copy_from_slice(&port.to_be_bytes());
                encoded[3..7].copy_from_slice(&address);
            }
            Self::Ipv6 { address, port } => {
                encoded[0] = 6;
                encoded[1..3].copy_from_slice(&port.to_be_bytes());
                encoded[3..19].copy_from_slice(&address);
            }
        }
        encoded
    }
}

impl From<SocketAddr> for SourceAddress {
    fn from(value: SocketAddr) -> Self {
        match value.ip() {
            IpAddr::V4(address) => Self::Ipv4 {
                address: address.octets(),
                port: value.port(),
            },
            IpAddr::V6(address) => Self::Ipv6 {
                address: address.octets(),
                port: value.port(),
            },
        }
    }
}

/// Public fields bound by one cookie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CookieBinding {
    source: SourceAddress,
    initiator_identity: [u8; IDENTITY_ID_LEN],
    responder_identity: [u8; IDENTITY_ID_LEN],
    ephemeral_key_hash: [u8; 32],
}

impl CookieBinding {
    /// Bind an observed source address, both identity IDs, and an ephemeral
    /// public-key hash.
    pub fn new(
        source: SourceAddress,
        initiator_identity: [u8; IDENTITY_ID_LEN],
        responder_identity: [u8; IDENTITY_ID_LEN],
        ephemeral_key_hash: [u8; 32],
    ) -> Result<Self, CookieError> {
        if initiator_identity == [0; IDENTITY_ID_LEN] {
            return Err(CookieError::InvalidIdentity("initiator"));
        }
        if responder_identity == [0; IDENTITY_ID_LEN] {
            return Err(CookieError::InvalidIdentity("responder"));
        }
        if initiator_identity == responder_identity {
            return Err(CookieError::DuplicateIdentity);
        }
        if ephemeral_key_hash == [0; 32] {
            return Err(CookieError::InvalidEphemeralKeyHash);
        }
        Ok(Self {
            source,
            initiator_identity,
            responder_identity,
            ephemeral_key_hash,
        })
    }

    /// Build a binding directly from a parsed classic `HELLO`.
    pub fn from_hello(source: SocketAddr, hello: &Hello) -> Result<Self, CookieError> {
        Self::new(
            source.into(),
            hello.initiator_identity,
            hello.responder_identity,
            ephemeral_key_hash(&hello.initiator_ephemeral_public),
        )
    }

    /// Source address bound into this cookie.
    pub const fn source(self) -> SourceAddress {
        self.source
    }
}

/// Domain-separated SHA-256 hash of an initiator ephemeral public key.
///
/// The API accepts a byte slice because the future X-Wing public key is larger
/// than the classic X25519 key; it does not interpret key-group encoding.
pub fn ephemeral_key_hash(public_key: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    update_field(|bytes| hasher.update(bytes), EPHEMERAL_KEY_HASH_DOMAIN);
    update_field(|bytes| hasher.update(bytes), public_key);
    hasher.finalize().into()
}

/// A stateless cookie in canonical wire representation.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Cookie {
    issued_at: u64,
    mac: [u8; COOKIE_MAC_LEN],
}

impl Cookie {
    /// Unix issue time authenticated by this cookie.
    pub const fn issued_at(self) -> u64 {
        self.issued_at
    }

    /// Encode the exact 41-byte cookie representation.
    pub fn encode(self) -> [u8; COOKIE_WIRE_LEN] {
        let mut encoded = [0u8; COOKIE_WIRE_LEN];
        encoded[0] = COOKIE_VERSION;
        encoded[1..1 + COOKIE_TIMESTAMP_LEN].copy_from_slice(&self.issued_at.to_be_bytes());
        encoded[1 + COOKIE_TIMESTAMP_LEN..].copy_from_slice(&self.mac);
        encoded
    }

    /// Decode an exact cookie representation, rejecting truncation and trailing bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, CookieError> {
        if bytes.len() < COOKIE_WIRE_LEN {
            return Err(CookieError::Truncated {
                expected: COOKIE_WIRE_LEN,
                actual: bytes.len(),
            });
        }
        if bytes.len() > COOKIE_WIRE_LEN {
            return Err(CookieError::TrailingBytes {
                trailing: bytes.len() - COOKIE_WIRE_LEN,
            });
        }
        if bytes[0] != COOKIE_VERSION {
            return Err(CookieError::UnsupportedCookieVersion(bytes[0]));
        }

        let mut issued_at = [0u8; COOKIE_TIMESTAMP_LEN];
        issued_at.copy_from_slice(&bytes[1..1 + COOKIE_TIMESTAMP_LEN]);
        let mut mac = [0u8; COOKIE_MAC_LEN];
        mac.copy_from_slice(&bytes[1 + COOKIE_TIMESTAMP_LEN..]);
        Ok(Self {
            issued_at: u64::from_be_bytes(issued_at),
            mac,
        })
    }
}

impl fmt::Debug for Cookie {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Cookie")
            .field("issued_at", &self.issued_at)
            .field("mac", &"<redacted>")
            .finish()
    }
}

/// Successful stateless cookie validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedCookie {
    /// Authenticated Unix issue time.
    pub issued_at: u64,
}

/// Validity policy applied by a cookie verifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CookiePolicy {
    /// Maximum age from the authenticated issue time.
    pub lifetime_secs: u64,
    /// Maximum timestamp lead accepted for clock skew.
    pub future_skew_secs: u64,
}

impl CookiePolicy {
    /// Construct a bounded policy.
    pub fn new(lifetime_secs: u64, future_skew_secs: u64) -> Result<Self, CookieError> {
        if lifetime_secs == 0 || lifetime_secs > MAX_COOKIE_LIFETIME_SECS {
            return Err(CookieError::InvalidCookiePolicy);
        }
        if future_skew_secs > lifetime_secs {
            return Err(CookieError::InvalidCookiePolicy);
        }
        Ok(Self {
            lifetime_secs,
            future_skew_secs,
        })
    }
}

impl Default for CookiePolicy {
    fn default() -> Self {
        Self {
            lifetime_secs: COOKIE_LIFETIME_SECS,
            future_skew_secs: COOKIE_FUTURE_SKEW_SECS,
        }
    }
}

enum CookieKeyMaterial {
    Static(ZeroizingCookieKey),
    Rotating {
        root_secret: ZeroizingCookieKey,
        rotation_interval_secs: u64,
        state: Mutex<Option<RotationState>>,
    },
}

struct RotationState {
    epoch: u64,
    current: ZeroizingCookieKey,
    previous: Option<ZeroizingCookieKey>,
}

/// Secret key and validity policy used to issue and validate stateless cookies.
pub struct CookieKey {
    material: CookieKeyMaterial,
    policy: CookiePolicy,
}

impl CookieKey {
    /// Construct a key from caller-provided entropy with the default policy.
    pub fn from_bytes(bytes: [u8; COOKIE_KEY_LEN]) -> Result<Self, CookieError> {
        Self::from_bytes_with_policy(bytes, CookiePolicy::default())
    }

    /// Construct a key from caller-provided entropy and an explicit policy.
    pub fn from_bytes_with_policy(
        bytes: [u8; COOKIE_KEY_LEN],
        policy: CookiePolicy,
    ) -> Result<Self, CookieError> {
        let bytes = Zeroizing::new(bytes);
        if bytes.iter().all(|byte| *byte == 0) {
            return Err(CookieError::InvalidCookieKey);
        }
        Ok(Self {
            material: CookieKeyMaterial::Static(bytes),
            policy,
        })
    }

    /// Construct a deterministic rotating cookie key for tests and deployments
    /// that provide their own root secret.
    ///
    /// The root secret is expanded with HMAC-SHA-256 into the current and
    /// previous epoch keys. Only those two derived keys are retained. The
    /// rotation interval must be at least the cookie lifetime so an unexpired
    /// cookie can cross at most one epoch boundary.
    pub fn from_bytes_with_rotation(
        bytes: [u8; COOKIE_KEY_LEN],
        policy: CookiePolicy,
        rotation_interval_secs: u64,
    ) -> Result<Self, CookieError> {
        let bytes = Zeroizing::new(bytes);
        if bytes.iter().all(|byte| *byte == 0) {
            return Err(CookieError::InvalidCookieKey);
        }
        if rotation_interval_secs < policy.lifetime_secs {
            return Err(CookieError::InvalidCookieRotationInterval);
        }
        Ok(Self {
            material: CookieKeyMaterial::Rotating {
                root_secret: bytes,
                rotation_interval_secs,
                state: Mutex::new(None),
            },
            policy,
        })
    }

    /// Generate a random rotating cookie key and the default validity policy
    /// from OS entropy.
    pub fn generate() -> Result<Self, CookieError> {
        let mut bytes = [0u8; COOKIE_KEY_LEN];
        fill_random(&mut bytes).map_err(|_| CookieError::RandomnessUnavailable)?;
        Self::from_bytes_with_rotation(bytes, CookiePolicy::default(), COOKIE_KEY_ROTATION_SECS)
    }

    /// Validity policy enforced by this key.
    pub const fn policy(&self) -> CookiePolicy {
        self.policy
    }

    /// Issue a cookie for one binding at `now` (Unix seconds).
    pub fn issue(&self, binding: &CookieBinding, now: u64) -> Cookie {
        let (current, _) = self.current_and_previous_keys(now);
        let mac = cookie_mac(&current, binding, now);
        Cookie {
            issued_at: now,
            mac,
        }
    }

    /// Validate a cookie against the exact binding and local trusted time.
    ///
    /// The MAC is checked before the timestamp. Acceptance always means the
    /// caller should continue with signature verification and handshake replay
    /// detection; a stateless cookie is not an authentication credential.
    pub fn verify(
        &self,
        binding: &CookieBinding,
        encoded: &[u8],
        now: u64,
    ) -> Result<VerifiedCookie, CookieError> {
        let cookie = Cookie::decode(encoded)?;
        let (current, previous) = self.current_and_previous_keys(now);
        let previous_matches = previous
            .as_ref()
            .map(|key| cookie_mac_matches(key, binding, &cookie))
            .unwrap_or(false);
        if !cookie_mac_matches(&current, binding, &cookie) && !previous_matches {
            return Err(CookieError::InvalidMac);
        }

        if cookie.issued_at > now.saturating_add(self.policy.future_skew_secs) {
            return Err(CookieError::NotYetValid);
        }
        if now.saturating_sub(cookie.issued_at) > self.policy.lifetime_secs {
            return Err(CookieError::Expired);
        }
        Ok(VerifiedCookie {
            issued_at: cookie.issued_at,
        })
    }

    fn current_and_previous_keys(
        &self,
        now: u64,
    ) -> (ZeroizingCookieKey, Option<ZeroizingCookieKey>) {
        match &self.material {
            CookieKeyMaterial::Static(bytes) => (bytes.clone(), None),
            CookieKeyMaterial::Rotating {
                root_secret,
                rotation_interval_secs,
                state,
            } => {
                let target_epoch = now / rotation_interval_secs;
                let mut state = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if state
                    .as_ref()
                    .map(|rotation| target_epoch > rotation.epoch)
                    .unwrap_or(true)
                {
                    let previous = target_epoch
                        .checked_sub(1)
                        .map(|epoch| derive_rotation_key(root_secret, epoch));
                    *state = Some(RotationState {
                        epoch: target_epoch,
                        current: derive_rotation_key(root_secret, target_epoch),
                        previous,
                    });
                }

                let rotation = state.as_ref().expect("rotation state is initialized");
                (rotation.current.clone(), rotation.previous.clone())
            }
        }
    }
}

impl fmt::Debug for CookieKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CookieKey")
            .field("material", &"<redacted>")
            .field("policy", &self.policy)
            .finish()
    }
}

/// Encode a cookie challenge only when it cannot amplify the triggering packet.
///
/// `trigger_len` must be the complete observed request length. If the challenge
/// is larger, no bytes are written and the caller must silently drop the request.
pub fn encode_cookie_challenge(
    cookie: &Cookie,
    trigger_len: usize,
    output: &mut [u8],
) -> Result<usize, CookieError> {
    if trigger_len < COOKIE_CHALLENGE_WIRE_LEN {
        return Err(CookieError::AmplificationBlocked {
            trigger_len,
            response_len: COOKIE_CHALLENGE_WIRE_LEN,
        });
    }
    if output.len() < COOKIE_CHALLENGE_WIRE_LEN {
        return Err(CookieError::BufferTooSmall {
            needed: COOKIE_CHALLENGE_WIRE_LEN,
            actual: output.len(),
        });
    }

    output[0] = COOKIE_CHALLENGE_KIND;
    output[1] = PROTOCOL_VERSION;
    output[COOKIE_CHALLENGE_HEADER_LEN..COOKIE_CHALLENGE_WIRE_LEN]
        .copy_from_slice(&cookie.encode());
    Ok(COOKIE_CHALLENGE_WIRE_LEN)
}

/// Decode one exact cookie challenge frame.
pub fn decode_cookie_challenge(bytes: &[u8]) -> Result<Cookie, CookieError> {
    if bytes.len() < COOKIE_CHALLENGE_HEADER_LEN {
        return Err(CookieError::Truncated {
            expected: COOKIE_CHALLENGE_WIRE_LEN,
            actual: bytes.len(),
        });
    }
    if bytes.len() > COOKIE_CHALLENGE_WIRE_LEN {
        return Err(CookieError::TrailingBytes {
            trailing: bytes.len() - COOKIE_CHALLENGE_WIRE_LEN,
        });
    }
    if bytes.len() < COOKIE_CHALLENGE_WIRE_LEN {
        return Err(CookieError::Truncated {
            expected: COOKIE_CHALLENGE_WIRE_LEN,
            actual: bytes.len(),
        });
    }
    if bytes[0] != COOKIE_CHALLENGE_KIND {
        return Err(CookieError::UnexpectedChallengeKind(bytes[0]));
    }
    if bytes[1] != PROTOCOL_VERSION {
        return Err(CookieError::UnsupportedProtocolVersion(bytes[1]));
    }
    Cookie::decode(&bytes[COOKIE_CHALLENGE_HEADER_LEN..])
}

/// Errors returned while issuing, encoding, or validating stateless cookies.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum CookieError {
    #[error("operating system randomness is unavailable")]
    RandomnessUnavailable,
    #[error("cookie key must not be all zero")]
    InvalidCookieKey,
    #[error("cookie lifetime must be 1..=300 seconds and future skew must not exceed it")]
    InvalidCookiePolicy,
    #[error("cookie rotation interval must be at least the cookie lifetime")]
    InvalidCookieRotationInterval,
    #[error("{0} identity must not be all zero")]
    InvalidIdentity(&'static str),
    #[error("initiator and responder identities must differ")]
    DuplicateIdentity,
    #[error("ephemeral key hash must not be all zero")]
    InvalidEphemeralKeyHash,
    #[error("cookie is {actual} bytes; expected exactly {expected}")]
    Truncated { expected: usize, actual: usize },
    #[error("cookie has {trailing} trailing byte(s)")]
    TrailingBytes { trailing: usize },
    #[error("unsupported cookie version {0}")]
    UnsupportedCookieVersion(u8),
    #[error("cookie MAC is invalid")]
    InvalidMac,
    #[error("cookie timestamp is too far in the future")]
    NotYetValid,
    #[error("cookie has expired")]
    Expired,
    #[error("unsupported cookie challenge protocol version {0}")]
    UnsupportedProtocolVersion(u8),
    #[error("unexpected cookie challenge kind {0:#04x}")]
    UnexpectedChallengeKind(u8),
    #[error(
        "refusing {response_len}-byte cookie challenge for {trigger_len}-byte trigger (amplification)"
    )]
    AmplificationBlocked {
        trigger_len: usize,
        response_len: usize,
    },
    #[error("output buffer is {actual} bytes; {needed} bytes are required")]
    BufferTooSmall { needed: usize, actual: usize },
}

fn cookie_mac(
    key: &[u8; COOKIE_KEY_LEN],
    binding: &CookieBinding,
    issued_at: u64,
) -> [u8; COOKIE_MAC_LEN] {
    let mut mac =
        HmacSha256::new_from_slice(key).expect("cookie keys always have a valid HMAC key length");
    update_cookie_fields(&mut mac, binding, issued_at);
    mac.finalize().into_bytes().into()
}

fn cookie_mac_matches(
    key: &[u8; COOKIE_KEY_LEN],
    binding: &CookieBinding,
    cookie: &Cookie,
) -> bool {
    let mut verifier =
        HmacSha256::new_from_slice(key).expect("cookie keys always have a valid HMAC key length");
    update_cookie_fields(&mut verifier, binding, cookie.issued_at);
    verifier.verify_slice(&cookie.mac).is_ok()
}

fn derive_rotation_key(root_secret: &[u8; COOKIE_KEY_LEN], epoch: u64) -> ZeroizingCookieKey {
    let mut derivation = HmacSha256::new_from_slice(root_secret)
        .expect("cookie root secrets always have a valid HMAC key length");
    update_field(|bytes| derivation.update(bytes), COOKIE_ROTATION_KEY_DOMAIN);
    derivation.update(&epoch.to_be_bytes());
    Zeroizing::new(derivation.finalize().into_bytes().into())
}

fn update_cookie_fields(mac: &mut HmacSha256, binding: &CookieBinding, issued_at: u64) {
    update_field(|bytes| mac.update(bytes), COOKIE_MAC_DOMAIN);
    update_field(|bytes| mac.update(bytes), &[PROTOCOL_VERSION]);
    update_field(|bytes| mac.update(bytes), &[COOKIE_VERSION]);
    update_field(|bytes| mac.update(bytes), &binding.source.encode());
    update_field(|bytes| mac.update(bytes), &binding.initiator_identity);
    update_field(|bytes| mac.update(bytes), &binding.responder_identity);
    update_field(|bytes| mac.update(bytes), &binding.ephemeral_key_hash);
    update_field(|bytes| mac.update(bytes), &issued_at.to_be_bytes());
}

fn update_field(mut update: impl FnMut(&[u8]), value: &[u8]) {
    let len = u16::try_from(value.len()).expect("cookie canonical fields fit in u16");
    update(&len.to_be_bytes());
    update(value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const KEY: [u8; COOKIE_KEY_LEN] = [0x11; COOKIE_KEY_LEN];
    const INITIATOR: [u8; IDENTITY_ID_LEN] = [0x22; IDENTITY_ID_LEN];
    const RESPONDER: [u8; IDENTITY_ID_LEN] = [0x44; IDENTITY_ID_LEN];
    const EPHEMERAL_PUBLIC: [u8; 32] = [0x33; 32];
    const NOW: u64 = 1_700_000_000;

    fn source() -> SourceAddress {
        SourceAddress::Ipv4 {
            address: [192, 0, 2, 10],
            port: 51820,
        }
    }

    fn binding() -> CookieBinding {
        CookieBinding::new(
            source(),
            INITIATOR,
            RESPONDER,
            ephemeral_key_hash(&EPHEMERAL_PUBLIC),
        )
        .expect("valid cookie binding")
    }

    fn key() -> CookieKey {
        CookieKey::from_bytes(KEY).expect("valid cookie key")
    }

    #[test]
    fn cookie_matches_a_fixed_vector() {
        let cookie = key().issue(&binding(), NOW);
        assert_eq!(
            cookie.encode(),
            [
                0x01, 0x00, 0x00, 0x00, 0x00, 0x65, 0x53, 0xf1, 0x00, 0x9e, 0xfd, 0xbf, 0xde, 0x11,
                0x2b, 0x36, 0x16, 0x55, 0x95, 0xe7, 0x79, 0x8a, 0x7f, 0xa4, 0xea, 0x4b, 0xaf, 0xf7,
                0xa4, 0xe9, 0x76, 0xf7, 0x53, 0x58, 0x25, 0x1b, 0xa8, 0xfb, 0x29, 0xe6, 0x7a,
            ]
        );
        assert_eq!(
            ephemeral_key_hash(&EPHEMERAL_PUBLIC),
            [
                0x11, 0xbd, 0xf1, 0x26, 0xed, 0x18, 0x61, 0xb0, 0x72, 0x54, 0x8a, 0xd3, 0x24, 0x12,
                0xbf, 0xcc, 0x36, 0x20, 0xc7, 0x59, 0x5a, 0x76, 0x81, 0x20, 0xa0, 0x28, 0x43, 0x84,
                0xc9, 0x9c, 0x3b, 0x43,
            ]
        );
    }

    #[test]
    fn cookie_validates_only_the_original_source_identity_and_ephemeral_key() {
        let key = key();
        let original = binding();
        let encoded = key.issue(&original, NOW).encode();
        assert_eq!(
            key.verify(&original, &encoded, NOW + 1)
                .expect("original binding"),
            VerifiedCookie { issued_at: NOW }
        );

        let changed_source = CookieBinding::new(
            SourceAddress::Ipv4 {
                address: [192, 0, 2, 11],
                port: 51820,
            },
            INITIATOR,
            RESPONDER,
            original.ephemeral_key_hash,
        )
        .expect("changed source binding");
        let changed_identity = CookieBinding::new(
            original.source,
            [0x23; IDENTITY_ID_LEN],
            RESPONDER,
            original.ephemeral_key_hash,
        )
        .expect("changed identity binding");
        let changed_responder = CookieBinding::new(
            original.source,
            INITIATOR,
            [0x45; IDENTITY_ID_LEN],
            original.ephemeral_key_hash,
        )
        .expect("changed responder identity binding");
        let changed_ephemeral = CookieBinding::new(
            original.source,
            INITIATOR,
            RESPONDER,
            ephemeral_key_hash(&[0x34; 32]),
        )
        .expect("changed ephemeral binding");
        let ipv6_source = CookieBinding::new(
            SourceAddress::Ipv6 {
                address: [0; 16],
                port: 51820,
            },
            INITIATOR,
            RESPONDER,
            original.ephemeral_key_hash,
        )
        .expect("changed address family binding");

        for retargeted in [
            changed_source,
            changed_identity,
            changed_responder,
            changed_ephemeral,
            ipv6_source,
        ] {
            assert_eq!(
                key.verify(&retargeted, &encoded, NOW + 1),
                Err(CookieError::InvalidMac)
            );
        }
    }

    #[test]
    fn cookie_rejects_expired_future_and_wrong_key_values() {
        let key = key();
        let cookie = key.issue(&binding(), NOW).encode();
        assert!(key
            .verify(&binding(), &cookie, NOW + COOKIE_LIFETIME_SECS)
            .is_ok());
        assert_eq!(
            key.verify(&binding(), &cookie, NOW + COOKIE_LIFETIME_SECS + 1),
            Err(CookieError::Expired)
        );
        assert!(key
            .verify(&binding(), &cookie, NOW - COOKIE_FUTURE_SKEW_SECS)
            .is_ok());
        assert_eq!(
            key.verify(&binding(), &cookie, NOW - COOKIE_FUTURE_SKEW_SECS - 1),
            Err(CookieError::NotYetValid)
        );

        let future = key.issue(&binding(), NOW + COOKIE_FUTURE_SKEW_SECS + 1);
        assert_eq!(
            key.verify(&binding(), &future.encode(), NOW),
            Err(CookieError::NotYetValid)
        );

        let wrong_key = CookieKey::from_bytes([0x12; COOKIE_KEY_LEN]).expect("second key");
        assert_eq!(
            wrong_key.verify(&binding(), &cookie, NOW),
            Err(CookieError::InvalidMac)
        );

        // Authentication is checked before freshness, so a forged expired
        // cookie never receives a distinguishable time-related error.
        let mut forged_expired = cookie;
        forged_expired[1] ^= 1;
        assert_eq!(
            key.verify(&binding(), &forged_expired, NOW + COOKIE_LIFETIME_SECS + 1),
            Err(CookieError::InvalidMac)
        );
    }

    #[test]
    fn cookie_parser_rejects_version_truncation_trailing_bytes_and_tampering() {
        let encoded = key().issue(&binding(), NOW).encode();

        let mut wrong_version = encoded;
        wrong_version[0] = COOKIE_VERSION + 1;
        assert_eq!(
            Cookie::decode(&wrong_version),
            Err(CookieError::UnsupportedCookieVersion(COOKIE_VERSION + 1))
        );
        assert_eq!(
            Cookie::decode(&encoded[..COOKIE_WIRE_LEN - 1]),
            Err(CookieError::Truncated {
                expected: COOKIE_WIRE_LEN,
                actual: COOKIE_WIRE_LEN - 1,
            })
        );
        let mut trailing = encoded.to_vec();
        trailing.push(0);
        assert_eq!(
            Cookie::decode(&trailing),
            Err(CookieError::TrailingBytes { trailing: 1 })
        );

        let mut tampered = encoded;
        tampered[COOKIE_WIRE_LEN - 1] ^= 1;
        assert_eq!(
            key().verify(&binding(), &tampered, NOW),
            Err(CookieError::InvalidMac)
        );
    }

    #[test]
    fn challenge_never_amplifies_and_has_a_canonical_frame() {
        let cookie = key().issue(&binding(), NOW);
        let mut output = [0u8; COOKIE_CHALLENGE_WIRE_LEN];
        assert_eq!(
            encode_cookie_challenge(&cookie, COOKIE_CHALLENGE_WIRE_LEN - 1, &mut output),
            Err(CookieError::AmplificationBlocked {
                trigger_len: COOKIE_CHALLENGE_WIRE_LEN - 1,
                response_len: COOKIE_CHALLENGE_WIRE_LEN,
            })
        );
        assert_eq!(output, [0u8; COOKIE_CHALLENGE_WIRE_LEN]);

        assert_eq!(
            encode_cookie_challenge(&cookie, COOKIE_CHALLENGE_WIRE_LEN, &mut output),
            Ok(COOKIE_CHALLENGE_WIRE_LEN)
        );
        assert_eq!(output[0], COOKIE_CHALLENGE_KIND);
        assert_eq!(output[1], PROTOCOL_VERSION);
        assert_eq!(
            decode_cookie_challenge(&output).expect("decode challenge"),
            cookie
        );
        assert_eq!(
            decode_cookie_challenge(&output[..COOKIE_CHALLENGE_WIRE_LEN - 1]),
            Err(CookieError::Truncated {
                expected: COOKIE_CHALLENGE_WIRE_LEN,
                actual: COOKIE_CHALLENGE_WIRE_LEN - 1,
            })
        );

        let mut wrong_kind = output;
        wrong_kind[0] ^= 1;
        assert_eq!(
            decode_cookie_challenge(&wrong_kind),
            Err(CookieError::UnexpectedChallengeKind(
                COOKIE_CHALLENGE_KIND ^ 1
            ))
        );

        let mut wrong_protocol = output;
        wrong_protocol[1] ^= 1;
        assert_eq!(
            decode_cookie_challenge(&wrong_protocol),
            Err(CookieError::UnsupportedProtocolVersion(
                PROTOCOL_VERSION ^ 1
            ))
        );
    }

    #[test]
    fn invalid_bindings_and_cookie_keys_fail_closed() {
        assert_eq!(
            CookieKey::from_bytes([0; COOKIE_KEY_LEN]).unwrap_err(),
            CookieError::InvalidCookieKey
        );
        assert_eq!(
            CookieBinding::new(source(), [0; IDENTITY_ID_LEN], RESPONDER, [1; 32]).unwrap_err(),
            CookieError::InvalidIdentity("initiator")
        );
        assert_eq!(
            CookieBinding::new(source(), INITIATOR, RESPONDER, [0; 32]).unwrap_err(),
            CookieError::InvalidEphemeralKeyHash
        );
        assert_eq!(
            CookieBinding::new(source(), INITIATOR, INITIATOR, [1; 32]).unwrap_err(),
            CookieError::DuplicateIdentity
        );
    }

    #[test]
    fn cookie_policy_is_bounded_configurable_and_enforced() {
        assert_eq!(
            CookiePolicy::new(0, 0).unwrap_err(),
            CookieError::InvalidCookiePolicy
        );
        assert_eq!(
            CookiePolicy::new(MAX_COOKIE_LIFETIME_SECS + 1, 0).unwrap_err(),
            CookieError::InvalidCookiePolicy
        );
        assert_eq!(
            CookiePolicy::new(10, 11).unwrap_err(),
            CookieError::InvalidCookiePolicy
        );

        let policy = CookiePolicy::new(5, 1).expect("custom policy");
        let key = CookieKey::from_bytes_with_policy(KEY, policy).expect("policy key");
        let cookie = key.issue(&binding(), NOW).encode();
        assert_eq!(key.policy(), policy);
        assert!(key.verify(&binding(), &cookie, NOW + 5).is_ok());
        assert_eq!(
            key.verify(&binding(), &cookie, NOW + 6),
            Err(CookieError::Expired)
        );
        assert_eq!(
            key.verify(&binding(), &cookie, NOW - 2),
            Err(CookieError::NotYetValid)
        );
    }

    #[test]
    fn rotating_cookie_key_accepts_one_previous_generation_and_rejects_two_generations_old() {
        let policy = CookiePolicy::new(2, 0).expect("short deterministic policy");
        let key = CookieKey::from_bytes_with_rotation(KEY, policy, 2)
            .expect("deterministic rotating key");
        let cookie = key.issue(&binding(), 0).encode();

        assert_eq!(
            key.verify(&binding(), &cookie, 1)
                .expect("current generation"),
            VerifiedCookie { issued_at: 0 }
        );
        assert_eq!(
            key.verify(&binding(), &cookie, 2)
                .expect("previous generation"),
            VerifiedCookie { issued_at: 0 }
        );
        assert_eq!(
            key.verify(&binding(), &cookie, 3)
                .expect_err("cookie lifetime still applies within the previous generation"),
            CookieError::Expired
        );
        assert_eq!(
            key.verify(&binding(), &cookie, 4),
            Err(CookieError::InvalidMac)
        );
    }

    #[test]
    fn rotation_does_not_rewind_discarded_generations_after_clock_rollback() {
        let policy = CookiePolicy::new(2, 0).expect("short deterministic policy");
        let key = CookieKey::from_bytes_with_rotation(KEY, policy, 2)
            .expect("deterministic rotating key");
        let oldest = key.issue(&binding(), 0).encode();
        let current = key.issue(&binding(), 4).encode();

        // Advancing to epoch 2 dropped epoch 0. A later wall-clock rollback to
        // epoch 0 must not restore that key or make the current state rewind.
        assert_eq!(
            key.verify(&binding(), &oldest, 0),
            Err(CookieError::InvalidMac)
        );
        assert_eq!(
            key.verify(&binding(), &current, 4)
                .expect("monotonic current generation"),
            VerifiedCookie { issued_at: 4 }
        );
    }

    #[test]
    fn rotation_interval_must_not_be_shorter_than_cookie_lifetime() {
        let policy = CookiePolicy::new(10, 0).expect("deterministic policy");

        assert_eq!(
            CookieKey::from_bytes_with_rotation(KEY, policy, 9).unwrap_err(),
            CookieError::InvalidCookieRotationInterval
        );
        assert!(CookieKey::from_bytes_with_rotation(KEY, policy, 10).is_ok());
    }

    #[test]
    fn rotating_cookie_key_can_be_shared_across_threads() {
        let policy = CookiePolicy::new(2, 0).expect("short deterministic policy");
        let key = Arc::new(
            CookieKey::from_bytes_with_rotation(KEY, policy, 2)
                .expect("deterministic rotating key"),
        );
        let binding = binding();
        let encoded = key.issue(&binding, NOW).encode();

        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    for _ in 0..64 {
                        assert_eq!(
                            key.verify(&binding, &encoded, NOW)
                                .expect("concurrent verification"),
                            VerifiedCookie { issued_at: NOW }
                        );
                    }
                });
            }
        });
    }

    #[test]
    fn generated_key_uses_the_documented_rotation_interval() {
        let key = CookieKey::generate().expect("generated rotating key");
        let CookieKeyMaterial::Rotating {
            rotation_interval_secs,
            ..
        } = key.material
        else {
            panic!("generated cookie keys must rotate");
        };
        assert_eq!(rotation_interval_secs, COOKIE_KEY_ROTATION_SECS);
        assert!(COOKIE_KEY_ROTATION_SECS >= CookiePolicy::default().lifetime_secs);
    }

    #[test]
    fn hello_binding_uses_both_identities_and_the_ephemeral_key_hash() {
        let hello = Hello {
            version: PROTOCOL_VERSION,
            profile: crate::handshake::CLASSIC_PROFILE_ID,
            suite_id: crate::transcript::SUITE_CHACHA20_POLY1305,
            initiator_identity: INITIATOR,
            responder_identity: RESPONDER,
            initiator_ephemeral_public: EPHEMERAL_PUBLIC,
            initiator_nonce: [0x55; 32],
            issued_at: NOW,
            endpoint_context_hash: [0x66; 32],
            signature: [0x77; 64],
        };
        let source: SocketAddr = "192.0.2.10:51820".parse().expect("source address");
        assert_eq!(
            CookieBinding::from_hello(source, &hello).expect("HELLO binding"),
            binding()
        );
    }
}
