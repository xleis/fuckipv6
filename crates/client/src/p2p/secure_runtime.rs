//! Shared-UDP integration state for the Secure Session v2 classic profile.
//!
//! The dedicated-socket adapter in [`super::secure_session`] validates the wire
//! protocol. This module provides the smaller state machine used by
//! [`super::connection::P2pEngine`], where one UDP receiver must multiplex
//! handshake, encrypted data and legacy control traffic.
//!
//! Role selection is deterministic: the lexicographically smaller long-term
//! Ed25519 public key is the initiator. This avoids simultaneous-open glare and
//! gives both endpoints the same receiver-index mapping without trusting an
//! unauthenticated negotiated role field.

use ed25519_dalek::VerifyingKey;
use fuckipv6_common::PeerInfo;
use fuckipv6_secure_session::cookie::{CookieBinding, CookieError, CookieKey};
#[cfg(test)]
use fuckipv6_secure_session::driver::DriverStats;
use fuckipv6_secure_session::driver::{
    DriverError, InitiatorHandshake, ResponderHandshake, SessionDriver, SessionInstallConfig,
};
use fuckipv6_secure_session::handshake::{
    ClassicResponder, Ed25519Identity, HandshakeError, Hello,
};
use fuckipv6_secure_session::wire::{
    classic_endpoint_context, AcceptFrame, ClassicEndpointContextInput, ConfirmFrame,
    ControlCodecError, ControlMessage, CookieChallengeFrame, HelloWithCookieFrame,
};
use fuckipv6_secure_session::{
    DataHeader, SessionError, AEAD_TAG_LEN, DATA_HEADER_LEN, MAX_DATA_PACKET_LEN,
    MAX_EPOCH_LIFETIME, MAX_RETIRED_RECEIVE_EPOCHS, PROTOCOL_VERSION, RETIRED_EPOCH_GRACE,
};
use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::sync::{Mutex, RwLock};

use super::admission::{HandshakeAdmission, HandshakeAdmissionError, HandshakeAdmissionPermit};
use super::secure_session::SessionPolicy;

/// Network-separation domain bound into the v2 endpoint context.
const P2P_NETWORK_CONTEXT: &[u8] = b"fuckipv6-p2p-v2";
/// Replace an initiator HELLO this long before the responder freshness window
/// rejects its timestamp. The retry task sends the same HELLO until this age.
const HELLO_REFRESH_AFTER: Duration = Duration::from_secs(30);
/// Start a replacement handshake this long before the active epoch expires.
const REKEY_LEAD_TIME: Duration = Duration::from_secs(30);
/// A completed initiator handshake waits for authenticated new-epoch traffic
/// from the responder before its new send keys become active.
const STAGED_GENERATION_TIMEOUT: Duration = Duration::from_secs(30);

/// One frame that must be sent on the shared UDP socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SecureControlAction {
    pub destination: SocketAddr,
    pub frame: Vec<u8>,
    pub kind: &'static str,
}

/// Errors produced while driving a peer through handshake and data states.
#[derive(Debug, Error)]
pub(crate) enum SecureRuntimeError {
    #[error("peer public key is not a valid Ed25519 verification key")]
    InvalidPeerPublicKey,
    #[error("Secure Session v2 handshake setup failed: {0}")]
    Handshake(#[from] HandshakeError),
    #[error("Secure Session v2 handshake codec failed: {0}")]
    ControlCodec(#[from] ControlCodecError),
    #[error("Secure Session v2 cookie failed: {0}")]
    Cookie(#[from] CookieError),
    #[error("Secure Session v2 driver failed: {0}")]
    Driver(#[from] DriverError),
    #[error("handshake admission rejected: {0}")]
    Admission(#[from] HandshakeAdmissionError),
    #[error("Secure Session v2 is not established for this peer")]
    NotEstablished,
    #[error("Secure Session v2 has been revoked for this peer")]
    Revoked,
    #[error("Secure Session v2 handshake has no compatible initiator/responder role")]
    InvalidRole,
    #[error("peer has no registered UDP endpoint")]
    EmptyEndpointSet,
    #[error("Secure Session v2 control frame arrived from untrusted peer endpoint {actual}")]
    UntrustedControlSource { actual: SocketAddr },
    #[cfg(test)]
    #[error(
        "datagram source {actual} does not match the authenticated session endpoint {expected}"
    )]
    SessionEndpointMismatch {
        expected: SocketAddr,
        actual: SocketAddr,
    },
    #[error("expected a Secure Session v2 {expected} frame")]
    UnexpectedFrame { expected: &'static str },
    #[error("plaintext is too large for one Secure Session v2 UDP packet: {len} bytes")]
    PayloadTooLarge { len: usize },
    #[error("system clock is before the Unix epoch")]
    InvalidSystemClock,
    #[error("Secure Session v2 epoch space is exhausted")]
    EpochExhausted,
    #[error("Secure Session v2 rekey attempted to reuse or roll back an epoch")]
    EpochRollback,
    #[error("Secure Session v2 rekey would downgrade a negotiated data suite")]
    SuiteDowngrade,
}

struct PendingInitiator {
    handshake: InitiatorHandshake,
    hello: Hello,
    issued_at: u64,
    epoch: u16,
    expires_at: Instant,
    hello_frame: Vec<u8>,
}

struct PendingResponder {
    handshake: ResponderHandshake,
    hello: Hello,
    epoch: u16,
    accept_frame: Vec<u8>,
    admission: HandshakeAdmissionPermit,
    expires_at: Instant,
}

struct InitiatorConfirmation {
    accept_frame: AcceptFrame,
    confirm_frame: Vec<u8>,
    destination: SocketAddr,
    expires_at: Instant,
}

struct StagedInitiator {
    driver: SessionDriver,
    confirmation: InitiatorConfirmation,
}

struct RetiredGeneration {
    driver: SessionDriver,
    epoch: u16,
    discard_at: Instant,
}

#[derive(Default)]
struct SecureState {
    revoked: bool,
    driver: Option<SessionDriver>,
    active_since: Option<Instant>,
    active_confirm: Option<ConfirmFrame>,
    initial_confirm: Option<InitiatorConfirmation>,
    staged_initiator: Option<StagedInitiator>,
    retired: VecDeque<RetiredGeneration>,
    authenticated_peer: Option<SocketAddr>,
    responder: Option<ClassicResponder>,
    pending_initiator: Option<PendingInitiator>,
    pending_responder: Option<PendingResponder>,
}

/// Per-peer v2 state shared by the UDP receiver and the packet sender.
pub(crate) struct SecurePeerState {
    local_identity: Ed25519Identity,
    expected_peer: VerifyingKey,
    endpoint_context: [u8; 32],
    policy: SessionPolicy,
    install_config: SessionInstallConfig,
    handshake_admission: Arc<HandshakeAdmission>,
    cookie_key: Arc<CookieKey>,
    trusted_endpoints: Vec<SocketAddr>,
    initiator: bool,
    local_receiver_index: u32,
    lifecycle_gate: RwLock<()>,
    state: Mutex<SecureState>,
}

impl SecurePeerState {
    /// Build per-peer state after checking that the local and remote identities
    /// are distinct and that the registered endpoint set is non-empty.
    pub(crate) fn new(
        local_identity: Ed25519Identity,
        local_virtual_ip: IpAddr,
        peer: &PeerInfo,
        handshake_admission: Arc<HandshakeAdmission>,
        cookie_key: Arc<CookieKey>,
    ) -> Result<Arc<Self>, SecureRuntimeError> {
        let peer_public: [u8; 32] = peer
            .public_key
            .as_slice()
            .try_into()
            .map_err(|_| SecureRuntimeError::InvalidPeerPublicKey)?;
        let expected_peer = VerifyingKey::from_bytes(&peer_public)
            .map_err(|_| SecureRuntimeError::InvalidPeerPublicKey)?;
        if local_identity.identity_id()
            == fuckipv6_secure_session::handshake::identity_id(&expected_peer)
        {
            return Err(SecureRuntimeError::Handshake(
                HandshakeError::DuplicateIdentity,
            ));
        }

        // The shared P2P runtime currently tracks one primary virtual address
        // per peer (`P2pEngine::local_ip`). Bind the same canonical address on
        // both sides instead of hashing the remote's full dual-stack metadata
        // while hashing only our local primary address. Otherwise two peers
        // with different `PeerInfo::virtual_ips` populate the transcript
        // asymmetrically and reject each other's HELLO.
        let peer_virtual_ips = [IpAddr::V6(peer.virtual_ip)];
        let local_virtual_ips = [local_virtual_ip];

        let initiator = local_identity.verifying_key().as_bytes() < expected_peer.as_bytes();
        let (initiator_public, responder_public) = if initiator {
            (local_identity.verifying_key(), expected_peer)
        } else {
            (expected_peer, local_identity.verifying_key())
        };
        let (initiator_index, responder_index) =
            derive_receiver_indices(initiator_public.as_bytes(), responder_public.as_bytes());
        let install_config = if initiator {
            SessionInstallConfig {
                local_receiver_index: initiator_index,
                peer_receiver_index: responder_index,
            }
        } else {
            SessionInstallConfig {
                local_receiver_index: responder_index,
                peer_receiver_index: initiator_index,
            }
        };

        let (initiator_virtual_ips, responder_virtual_ips) = if initiator {
            (local_virtual_ips.as_slice(), peer_virtual_ips.as_slice())
        } else {
            (peer_virtual_ips.as_slice(), local_virtual_ips.as_slice())
        };
        let endpoint_context = classic_endpoint_context(&ClassicEndpointContextInput {
            network_id: P2P_NETWORK_CONTEXT,
            initiator_public_key: initiator_public.as_bytes(),
            responder_public_key: responder_public.as_bytes(),
            initiator_virtual_ips,
            responder_virtual_ips,
            initiator_endpoints: &[],
            responder_endpoints: &[],
        })?;

        let registered_endpoints = canonical_endpoints(&peer.endpoints);
        if registered_endpoints.is_empty() {
            return Err(SecureRuntimeError::EmptyEndpointSet);
        }
        let responder = if initiator {
            None
        } else {
            Some(ClassicResponder::new(
                local_identity.clone(),
                expected_peer,
            )?)
        };

        tracing::info!(
            local_identity = %hex_prefix(local_identity.verifying_key().as_bytes()),
            peer_identity = %hex_prefix(expected_peer.as_bytes()),
            role = if initiator { "initiator" } else { "responder" },
            endpoints = ?registered_endpoints,
            "Secure Session v2 对等状态已初始化"
        );

        Ok(Arc::new(Self {
            local_identity,
            expected_peer,
            endpoint_context,
            policy: SessionPolicy::default(),
            install_config,
            handshake_admission,
            cookie_key,
            trusted_endpoints: registered_endpoints.clone(),
            initiator,
            local_receiver_index: install_config.local_receiver_index,
            lifecycle_gate: RwLock::new(()),
            state: Mutex::new(SecureState {
                responder,
                ..SecureState::default()
            }),
        }))
    }

    /// Whether this endpoint is the deterministic initiator.
    pub(crate) const fn is_initiator(&self) -> bool {
        self.initiator
    }

    /// Whether an authenticated v2 driver is installed.
    pub(crate) async fn is_established(&self) -> bool {
        let state = self.state.lock().await;
        !state.revoked && state.driver.is_some()
    }

    /// Prevent new session work and wait for already admitted session work.
    ///
    /// Callers performing transport I/O must hold this permission until the
    /// sealed packet or handshake frame has been handed to the transport.
    /// Callers consuming authenticated inbound plaintext must hold it until the
    /// plaintext has been delivered or cached. This keeps revocation local to
    /// one peer instead of holding the global connection index across slow I/O.
    pub(crate) async fn acquire_session_permission(
        &self,
    ) -> Result<tokio::sync::RwLockReadGuard<'_, ()>, SecureRuntimeError> {
        let permission = self.lifecycle_gate.read().await;
        if self.state.lock().await.revoked {
            return Err(SecureRuntimeError::Revoked);
        }
        Ok(permission)
    }

    /// Permanently revoke this per-peer session instance.
    ///
    /// The terminal flag and all reusable handshake/session material are changed
    /// under the same lock used by `seal`, `open` and `handle_control`. This waits
    /// for admitted session work to finish, so after it returns no in-flight or
    /// future operation on this instance can seal, open, resume a pending
    /// handshake, or install a new driver. Ciphertext already returned to a caller
    /// is outside this session object's control.
    pub(crate) async fn revoke(&self) -> bool {
        let _lifecycle = self.lifecycle_gate.write().await;
        let mut state = self.state.lock().await;
        if state.revoked {
            return false;
        }
        state.revoked = true;
        state.driver = None;
        state.active_since = None;
        state.active_confirm = None;
        state.initial_confirm = None;
        state.staged_initiator = None;
        state.retired.clear();
        state.authenticated_peer = None;
        state.responder = None;
        state.pending_initiator = None;
        state.pending_responder = None;
        true
    }

    /// Endpoint authenticated by the completed handshake.
    #[cfg(test)]
    pub(crate) async fn authenticated_peer(&self) -> Option<SocketAddr> {
        self.state.lock().await.authenticated_peer
    }

    /// Session identifier for diagnostics and tests.
    #[cfg(test)]
    pub(crate) async fn session_id(&self) -> Option<[u8; fuckipv6_secure_session::SESSION_ID_LEN]> {
        self.state
            .lock()
            .await
            .driver
            .as_ref()
            .and_then(SessionDriver::session_id)
    }

    /// Active data epoch for diagnostics and tests.
    #[cfg(test)]
    pub(crate) async fn epoch(&self) -> Option<u16> {
        self.state
            .lock()
            .await
            .driver
            .as_ref()
            .and_then(SessionDriver::epoch)
    }

    /// Move the active epoch close enough to expiry that the next initiator
    /// polling cycle starts a replacement handshake. Test-only so production
    /// time policy remains unchanged.
    #[cfg(test)]
    pub(crate) async fn force_rekey_due(&self) {
        let mut state = self.state.lock().await;
        if state.driver.is_some() {
            let rekey_age = MAX_EPOCH_LIFETIME
                .saturating_sub(REKEY_LEAD_TIME)
                .saturating_add(Duration::from_millis(1));
            state.active_since = Some(Instant::now() - rekey_age);
        }
    }

    /// Driver counters for diagnostics and tests.
    #[cfg(test)]
    pub(crate) async fn stats(&self) -> Option<DriverStats> {
        self.state
            .lock()
            .await
            .driver
            .as_ref()
            .map(SessionDriver::stats)
    }

    /// Return a current or reusable initiator HELLO frame.
    ///
    /// Repeated calls return the exact same frame while it remains fresh so a
    /// responder's replay cache can recognize retransmissions. An abandoned
    /// attempt is replaced after [`HELLO_REFRESH_AFTER`].
    pub(crate) async fn initiate_frame(&self) -> Result<Option<Vec<u8>>, SecureRuntimeError> {
        let mut state = self.state.lock().await;
        let now = Instant::now();
        prune_state(&mut state, now);
        if state.revoked {
            return Err(SecureRuntimeError::Revoked);
        }
        if !self.initiator {
            return Ok(None);
        }
        let needs_rekey = match state.driver.as_ref() {
            Some(driver) => driver_rekey_due(driver, state.active_since, now),
            None => true,
        };
        if !needs_rekey || state.staged_initiator.is_some() {
            return Ok(None);
        }
        let now = unix_timestamp()?;
        let epoch = next_epoch(state.driver.as_ref())?;
        if let Some(pending) = state.pending_initiator.as_ref() {
            if pending.epoch == epoch
                && now.saturating_sub(pending.issued_at) < HELLO_REFRESH_AFTER.as_secs()
            {
                return Ok(Some(pending.hello_frame.clone()));
            }
        }

        let (handshake, hello) = InitiatorHandshake::start(
            &self.local_identity,
            self.expected_peer,
            self.policy.preferred_suite().id(),
            self.endpoint_context,
            epoch,
            now,
            self.install_config,
            None,
        )?;
        let hello_frame = hello.encode_control();
        tracing::debug!(
            peer_identity = %hex_prefix(self.expected_peer.as_bytes()),
            issued_at = now,
            frame_len = hello_frame.len(),
            "已生成 Secure Session v2 HELLO"
        );
        state.pending_initiator = Some(PendingInitiator {
            handshake,
            hello,
            issued_at: now,
            epoch,
            expires_at: Instant::now() + STAGED_GENERATION_TIMEOUT,
            hello_frame: hello_frame.clone(),
        });
        Ok(Some(hello_frame))
    }

    /// Retransmit the CONFIRM for an initiator rekey until authenticated
    /// responder traffic proves that the responder installed the same epoch.
    pub(crate) async fn retry_staged_confirm(
        &self,
    ) -> Result<Option<SecureControlAction>, SecureRuntimeError> {
        let mut state = self.state.lock().await;
        prune_state(&mut state, Instant::now());
        if state.revoked {
            return Err(SecureRuntimeError::Revoked);
        }
        let confirmation = state
            .staged_initiator
            .as_ref()
            .map(|staged| &staged.confirmation)
            .or(state.initial_confirm.as_ref());
        Ok(confirmation.map(confirmation_action))
    }

    /// Process one authenticated or rejectable v2 control frame.
    pub(crate) async fn handle_control(
        &self,
        source: SocketAddr,
        message: ControlMessage,
    ) -> Result<Vec<SecureControlAction>, SecureRuntimeError> {
        let source = canonical_endpoint(source);
        tracing::debug!(
            %source,
            peer_identity = %hex_prefix(self.expected_peer.as_bytes()),
            role = if self.initiator { "initiator" } else { "responder" },
            frame = control_variant(&message),
            "收到 Secure Session v2 控制帧"
        );
        let mut state = self.state.lock().await;
        prune_state(&mut state, Instant::now());
        if state.revoked {
            return Err(SecureRuntimeError::Revoked);
        }
        match message {
            ControlMessage::Hello(hello) => {
                if !self.initiator {
                    return self.challenge_hello(source, hello);
                }
                Err(SecureRuntimeError::UnexpectedFrame {
                    expected: "COOKIE_CHALLENGE, ACCEPT or CONFIRM",
                })
            }
            ControlMessage::CookieChallenge(challenge) => {
                if self.initiator {
                    return self.accept_cookie_challenge(&mut state, source, challenge);
                }
                Err(SecureRuntimeError::UnexpectedFrame {
                    expected: "HELLO or CONFIRM",
                })
            }
            ControlMessage::HelloWithCookie(retry) => {
                if !self.initiator {
                    return self.accept_hello_with_cookie(&mut state, source, retry);
                }
                Err(SecureRuntimeError::UnexpectedFrame {
                    expected: "COOKIE_CHALLENGE, ACCEPT or CONFIRM",
                })
            }
            ControlMessage::Accept(accept) => {
                if !self.initiator {
                    return Err(SecureRuntimeError::UnexpectedFrame {
                        expected: "HELLO or CONFIRM",
                    });
                }
                if let Some(confirmation) = state
                    .staged_initiator
                    .as_ref()
                    .map(|staged| &staged.confirmation)
                    .or(state.initial_confirm.as_ref())
                {
                    if confirmation.accept_frame == accept {
                        return Ok(vec![confirmation_action(confirmation)]);
                    }
                }
                let Some(pending) = state.pending_initiator.as_ref() else {
                    return Err(SecureRuntimeError::UnexpectedFrame { expected: "HELLO" });
                };
                let now = Instant::now();
                if now >= pending.expires_at {
                    state.pending_initiator = None;
                    return Err(HandshakeAdmissionError::Expired.into());
                }
                if let Some(active) = state.driver.as_ref() {
                    if active.epoch().is_some_and(|epoch| pending.epoch <= epoch) {
                        return Err(SecureRuntimeError::EpochRollback);
                    }
                    if active.suite().map(|suite| suite.id()) != Some(accept.accept.suite_id) {
                        return Err(SecureRuntimeError::SuiteDowngrade);
                    }
                }
                pending
                    .handshake
                    .validate_accept_wire(&accept, pending.issued_at)?;
                let pending = state
                    .pending_initiator
                    .take()
                    .expect("validated pending initiator");
                let (driver, confirm) =
                    pending
                        .handshake
                        .complete_wire(&accept, pending.issued_at, now)?;
                if driver.epoch() != Some(pending.epoch) {
                    return Err(SecureRuntimeError::EpochRollback);
                }
                let confirmation = InitiatorConfirmation {
                    accept_frame: accept,
                    confirm_frame: confirm.encode(),
                    destination: source,
                    expires_at: now + STAGED_GENERATION_TIMEOUT,
                };
                let action = confirmation_action(&confirmation);
                if state.driver.is_some() {
                    state.staged_initiator = Some(StagedInitiator {
                        driver,
                        confirmation,
                    });
                } else {
                    install_active_generation(&mut state, driver, now)?;
                    state.initial_confirm = Some(confirmation);
                }
                state.authenticated_peer = Some(source);
                Ok(vec![action])
            }
            ControlMessage::Confirm(confirm) => {
                if self.initiator {
                    return Err(SecureRuntimeError::UnexpectedFrame { expected: "ACCEPT" });
                }
                if state.active_confirm.as_ref() == Some(&confirm) {
                    // A byte-exact retransmission of the CONFIRM that activated
                    // this generation is idempotent. Any other frame, even one
                    // carrying the public active session id, must be verified.
                    return Ok(Vec::new());
                }
                let Some(pending) = state.pending_responder.as_ref() else {
                    return Err(SecureRuntimeError::UnexpectedFrame { expected: "HELLO" });
                };
                let now = Instant::now();
                if now >= pending.expires_at || !pending.admission.is_active(now) {
                    state.pending_responder = None;
                    return Err(HandshakeAdmissionError::Expired.into());
                }
                if state
                    .driver
                    .as_ref()
                    .and_then(SessionDriver::epoch)
                    .is_some_and(|epoch| pending.epoch <= epoch)
                {
                    return Err(SecureRuntimeError::EpochRollback);
                }
                pending.handshake.validate_wire(&confirm)?;
                let pending = state
                    .pending_responder
                    .take()
                    .expect("validated pending responder");
                let (driver, _) = pending.handshake.complete_wire(&confirm, now)?;
                if driver.epoch() != Some(pending.epoch) {
                    return Err(SecureRuntimeError::EpochRollback);
                }
                install_active_generation(&mut state, driver, now)?;
                state.authenticated_peer = Some(source);
                state.active_confirm = Some(confirm);
                Ok(Vec::new())
            }
        }
    }

    fn challenge_hello(
        &self,
        source: SocketAddr,
        hello: Hello,
    ) -> Result<Vec<SecureControlAction>, SecureRuntimeError> {
        if self.initiator {
            return Err(SecureRuntimeError::InvalidRole);
        }
        if hello.version != PROTOCOL_VERSION {
            return Err(HandshakeError::UnsupportedVersion(hello.version).into());
        }
        if hello.profile != fuckipv6_secure_session::handshake::CLASSIC_PROFILE_ID {
            return Err(HandshakeError::UnsupportedProfile(hello.profile).into());
        }
        if !self.policy.supports(hello.suite_id) {
            return Err(HandshakeError::UnsupportedSuite(hello.suite_id).into());
        }
        let now = unix_timestamp()?;
        if now.abs_diff(hello.issued_at)
            > fuckipv6_secure_session::handshake::HANDSHAKE_TIMESTAMP_WINDOW_SECS
        {
            return Err(HandshakeError::TimestampOutsideWindow.into());
        }
        let binding = CookieBinding::from_hello(source, &hello)?;
        let cookie = self.cookie_key.issue(&binding, now);
        let frame = CookieChallengeFrame {
            responder_identity: self.local_identity.identity_id(),
            cookie,
        }
        .encode();
        Ok(vec![SecureControlAction {
            destination: source,
            frame,
            kind: "COOKIE_CHALLENGE",
        }])
    }

    fn accept_cookie_challenge(
        &self,
        state: &mut SecureState,
        source: SocketAddr,
        challenge: CookieChallengeFrame,
    ) -> Result<Vec<SecureControlAction>, SecureRuntimeError> {
        let peer_identity = fuckipv6_secure_session::handshake::identity_id(&self.expected_peer);
        if challenge.responder_identity != peer_identity {
            return Err(SecureRuntimeError::UnexpectedFrame {
                expected: "COOKIE_CHALLENGE for the configured peer",
            });
        }
        if !self.is_trusted_control_source(state, source) {
            return Err(SecureRuntimeError::UntrustedControlSource { actual: source });
        }
        let now = Instant::now();
        let Some(pending) = state.pending_initiator.as_mut() else {
            return Err(SecureRuntimeError::UnexpectedFrame { expected: "HELLO" });
        };
        if now >= pending.expires_at {
            state.pending_initiator = None;
            return Err(HandshakeAdmissionError::Expired.into());
        }
        let frame = HelloWithCookieFrame {
            hello: pending.hello.clone(),
            cookie: challenge.cookie,
        }
        .encode();
        pending.hello_frame = frame.clone();
        Ok(vec![SecureControlAction {
            destination: source,
            frame,
            kind: "HELLO_WITH_COOKIE",
        }])
    }

    fn is_trusted_control_source(&self, state: &SecureState, source: SocketAddr) -> bool {
        let source = canonical_endpoint(source);
        state.authenticated_peer == Some(source) || self.trusted_endpoints.contains(&source)
    }

    fn accept_hello_with_cookie(
        &self,
        state: &mut SecureState,
        source: SocketAddr,
        retry: HelloWithCookieFrame,
    ) -> Result<Vec<SecureControlAction>, SecureRuntimeError> {
        if self.initiator {
            return Err(SecureRuntimeError::InvalidRole);
        }
        let now = unix_timestamp()?;
        let binding = CookieBinding::from_hello(source, &retry.hello)?;
        self.cookie_key
            .verify(&binding, &retry.cookie.encode(), now)?;
        self.accept_authenticated_hello(state, source, retry.hello)
    }

    fn accept_authenticated_hello(
        &self,
        state: &mut SecureState,
        source: SocketAddr,
        hello: Hello,
    ) -> Result<Vec<SecureControlAction>, SecureRuntimeError> {
        let now = Instant::now();
        if state
            .pending_responder
            .as_ref()
            .is_some_and(|pending| pending.hello == hello && pending.admission.is_active(now))
        {
            let pending = state
                .pending_responder
                .as_ref()
                .expect("active pending responder checked above");
            return Ok(vec![SecureControlAction {
                destination: source,
                frame: pending.accept_frame.clone(),
                kind: "ACCEPT",
            }]);
        }
        if state
            .pending_responder
            .as_ref()
            .is_some_and(|pending| now >= pending.expires_at || !pending.admission.is_active(now))
        {
            state.pending_responder = None;
        }

        if !self.policy.supports(hello.suite_id) {
            return Err(HandshakeError::UnsupportedSuite(hello.suite_id).into());
        }
        if state
            .driver
            .as_ref()
            .and_then(SessionDriver::suite)
            .is_some_and(|suite| suite.id() != hello.suite_id)
        {
            return Err(SecureRuntimeError::SuiteDowngrade);
        }

        let epoch = next_epoch(state.driver.as_ref())?;
        let admission =
            self.handshake_admission
                .try_reserve(source.ip(), hello.initiator_identity, now)?;
        let responder = state
            .responder
            .as_mut()
            .ok_or(SecureRuntimeError::InvalidRole)?;
        let issued_at = unix_timestamp()?;
        let (handshake, accept) = ResponderHandshake::accept(
            responder,
            &hello,
            self.endpoint_context,
            epoch,
            issued_at,
            self.install_config,
            None,
        )?;
        let accept = AcceptFrame {
            accept,
            responder_confirmation: handshake.local_confirmation(),
        };
        let accept_frame = accept.encode();
        state.pending_responder = Some(PendingResponder {
            handshake,
            hello,
            epoch,
            accept_frame: accept_frame.clone(),
            admission,
            expires_at: now + STAGED_GENERATION_TIMEOUT,
        });
        Ok(vec![SecureControlAction {
            destination: source,
            frame: accept_frame,
            kind: "ACCEPT",
        }])
    }

    /// Seal one plaintext into a complete v2 UDP datagram and return its
    /// authenticated destination.
    pub(crate) async fn seal(
        &self,
        plaintext: &[u8],
    ) -> Result<(SocketAddr, Vec<u8>), SecureRuntimeError> {
        let mut state = self.state.lock().await;
        if state.revoked {
            return Err(SecureRuntimeError::Revoked);
        }
        let destination = state
            .authenticated_peer
            .ok_or(SecureRuntimeError::NotEstablished)?;
        let driver = state
            .driver
            .as_mut()
            .ok_or(SecureRuntimeError::NotEstablished)?;
        let packet_len = plaintext
            .len()
            .checked_add(DATA_HEADER_LEN + AEAD_TAG_LEN)
            .filter(|len| *len <= MAX_DATA_PACKET_LEN)
            .ok_or(SecureRuntimeError::PayloadTooLarge {
                len: plaintext.len(),
            })?;
        let mut packet = vec![0u8; packet_len];
        let encoded_len = driver.seal(plaintext, &mut packet, Instant::now())?;
        packet.truncate(encoded_len);
        Ok((destination, packet))
    }

    /// Authenticate and decrypt one v2 data datagram from the session endpoint.
    #[cfg(test)]
    pub(crate) async fn open(
        &self,
        source: SocketAddr,
        packet: Vec<u8>,
    ) -> Result<Vec<u8>, SecureRuntimeError> {
        let mut state = self.state.lock().await;
        if state.revoked {
            return Err(SecureRuntimeError::Revoked);
        }
        let expected = state
            .authenticated_peer
            .ok_or(SecureRuntimeError::NotEstablished)?;
        if source != expected {
            return Err(SecureRuntimeError::SessionEndpointMismatch {
                expected,
                actual: source,
            });
        }
        open_with_driver(&mut state, packet)
    }

    /// Authenticate and decrypt a v2 data datagram carried over a relay.
    ///
    /// Relay transports do not expose the peer's original UDP source address,
    /// so callers must resolve the virtual source IP to the registered peer
    /// before invoking this method. AEAD authentication and replay protection
    /// still fail closed.
    pub(crate) async fn open_from_session(
        &self,
        packet: Vec<u8>,
    ) -> Result<Vec<u8>, SecureRuntimeError> {
        let mut state = self.state.lock().await;
        open_with_driver(&mut state, packet)
    }

    /// Authenticate one direct UDP data packet and bind the peer's new source
    /// endpoint only after AEAD and replay validation have succeeded.
    pub(crate) async fn open_from_source(
        &self,
        source: SocketAddr,
        packet: Vec<u8>,
    ) -> Result<Vec<u8>, SecureRuntimeError> {
        let mut state = self.state.lock().await;
        let plaintext = open_with_driver(&mut state, packet)?;
        state.authenticated_peer = Some(canonical_endpoint(source));
        Ok(plaintext)
    }

    /// Return whether this state can own a control frame before its signature
    /// or key confirmation has been checked. The subsequent `handle_control`
    /// call remains responsible for all authentication.
    pub(crate) async fn matches_control(&self, message: &ControlMessage) -> bool {
        let state = self.state.lock().await;
        if state.revoked {
            return false;
        }
        let local_identity = self.local_identity.identity_id();
        let peer_identity = fuckipv6_secure_session::handshake::identity_id(&self.expected_peer);
        match message {
            ControlMessage::Hello(hello) => {
                !self.initiator
                    && hello.initiator_identity == peer_identity
                    && hello.responder_identity == local_identity
            }
            ControlMessage::HelloWithCookie(retry) => {
                !self.initiator
                    && retry.hello.initiator_identity == peer_identity
                    && retry.hello.responder_identity == local_identity
            }
            ControlMessage::CookieChallenge(challenge) => {
                self.initiator && challenge.responder_identity == peer_identity
            }
            ControlMessage::Accept(accept) => {
                self.initiator
                    && accept.accept.initiator_identity == local_identity
                    && accept.accept.responder_identity == peer_identity
            }
            ControlMessage::Confirm(confirm) => {
                if self.initiator {
                    return false;
                }
                state
                    .driver
                    .as_ref()
                    .and_then(SessionDriver::session_id)
                    .is_some_and(|session_id| session_id == confirm.session_id)
                    || state
                        .pending_responder
                        .as_ref()
                        .is_some_and(|pending| pending.handshake.session_id() == confirm.session_id)
            }
        }
    }

    /// Return whether an encrypted frame targets this endpoint's receiver
    /// index. This is only a routing hint; `open_from_source` authenticates the
    /// complete packet before callers may trust it or change its endpoint.
    pub(crate) fn handles_data_packet(&self, packet: &[u8]) -> bool {
        DataHeader::decode(packet)
            .is_ok_and(|header| header.receiver_index == self.local_receiver_index)
    }
}

fn open_with_driver(
    state: &mut SecureState,
    mut packet: Vec<u8>,
) -> Result<Vec<u8>, SecureRuntimeError> {
    if state.revoked {
        return Err(SecureRuntimeError::Revoked);
    }
    let now = Instant::now();
    let active = state
        .driver
        .as_mut()
        .ok_or(SecureRuntimeError::NotEstablished)?
        .open_in_place(&mut packet, now)
        .map(|plaintext| plaintext.to_vec());
    match active {
        Ok(plaintext) => {
            // Authenticated traffic under the active generation proves that the
            // peer accepted this endpoint's CONFIRM. Initial CONFIRM retries can
            // now stop without weakening the established generation.
            state.initial_confirm = None;
            return Ok(plaintext);
        }
        Err(error) if is_unknown_epoch(&error) => {}
        Err(error) => return Err(error.into()),
    }

    if let Some(staged) = state.staged_initiator.as_mut() {
        let opened = staged
            .driver
            .open_in_place(&mut packet, now)
            .map(|plaintext| plaintext.to_vec());
        match opened {
            Ok(plaintext) => {
                // The responder sends first after installing a rekey. Receiving
                // one authenticated packet for the staged epoch is the commit
                // point that lets the initiator switch its send keys too.
                let staged = state
                    .staged_initiator
                    .take()
                    .expect("staged driver opened a packet");
                install_active_generation(state, staged.driver, now)?;
                return Ok(plaintext);
            }
            Err(error) if is_unknown_epoch(&error) => {}
            Err(error) => return Err(error.into()),
        }
    }

    let epoch = DataHeader::decode(&packet)
        .map_err(DriverError::Data)?
        .epoch;
    let retired = state
        .retired
        .iter()
        .position(|retired| retired.epoch == epoch);
    if let Some(index) = retired {
        let opened = state.retired[index]
            .driver
            .open_in_place(&mut packet, now)
            .map(|plaintext| plaintext.to_vec());
        return match opened {
            Ok(plaintext) => Ok(plaintext),
            Err(error) => Err(error.into()),
        };
    }

    Err(DriverError::Data(SessionError::UnknownEpoch).into())
}

fn confirmation_action(confirmation: &InitiatorConfirmation) -> SecureControlAction {
    SecureControlAction {
        destination: confirmation.destination,
        frame: confirmation.confirm_frame.clone(),
        kind: "CONFIRM",
    }
}

fn prune_state(state: &mut SecureState, now: Instant) {
    if state
        .pending_initiator
        .as_ref()
        .is_some_and(|pending| now >= pending.expires_at)
    {
        state.pending_initiator = None;
    }
    if state
        .pending_responder
        .as_ref()
        .is_some_and(|pending| now >= pending.expires_at || !pending.admission.is_active(now))
    {
        state.pending_responder = None;
    }
    if state
        .initial_confirm
        .as_ref()
        .is_some_and(|confirmation| now >= confirmation.expires_at)
    {
        state.initial_confirm = None;
    }
    if state
        .staged_initiator
        .as_ref()
        .is_some_and(|staged| now >= staged.confirmation.expires_at)
    {
        state.staged_initiator = None;
    }
    state
        .retired
        .retain(|generation| now < generation.discard_at);
}

fn driver_rekey_due(driver: &SessionDriver, active_since: Option<Instant>, now: Instant) -> bool {
    driver.rekey_required(now)
        || active_since.is_some_and(|started| {
            now.saturating_duration_since(started)
                >= MAX_EPOCH_LIFETIME.saturating_sub(REKEY_LEAD_TIME)
        })
}

fn next_epoch(active: Option<&SessionDriver>) -> Result<u16, SecureRuntimeError> {
    let Some(active) = active else {
        return Ok(1);
    };
    active
        .epoch()
        .ok_or(SecureRuntimeError::Driver(DriverError::Closed))?
        .checked_add(1)
        .filter(|epoch| *epoch != 0)
        .ok_or(SecureRuntimeError::EpochExhausted)
}

fn install_active_generation(
    state: &mut SecureState,
    driver: SessionDriver,
    now: Instant,
) -> Result<(), SecureRuntimeError> {
    let new_epoch = driver
        .epoch()
        .ok_or(SecureRuntimeError::Driver(DriverError::Closed))?;
    if let Some(old) = state.driver.take() {
        let old_epoch = old
            .epoch()
            .ok_or(SecureRuntimeError::Driver(DriverError::Closed))?;
        if new_epoch <= old_epoch {
            state.driver = Some(old);
            return Err(SecureRuntimeError::EpochRollback);
        }
        state.retired.push_front(RetiredGeneration {
            driver: old,
            epoch: old_epoch,
            discard_at: now + RETIRED_EPOCH_GRACE,
        });
        while state.retired.len() > MAX_RETIRED_RECEIVE_EPOCHS {
            state.retired.pop_back();
        }
    }
    state.driver = Some(driver);
    state.active_since = Some(now);
    state.active_confirm = None;
    state.initial_confirm = None;
    Ok(())
}

fn is_unknown_epoch(error: &DriverError) -> bool {
    matches!(error, DriverError::Data(SessionError::UnknownEpoch))
}

fn derive_receiver_indices(initiator: &[u8; 32], responder: &[u8; 32]) -> (u32, u32) {
    let mut initiator_index = u32::from_be_bytes(initiator[..4].try_into().expect("four bytes"));
    let mut responder_index = u32::from_be_bytes(responder[..4].try_into().expect("four bytes"));
    if initiator_index == 0 {
        initiator_index = 1;
    }
    if responder_index == 0 || responder_index == initiator_index {
        responder_index = initiator_index.wrapping_add(1);
        if responder_index == 0 {
            responder_index = 2;
        }
    }
    (initiator_index, responder_index)
}

fn canonical_endpoint(endpoint: SocketAddr) -> SocketAddr {
    match endpoint {
        SocketAddr::V6(addr) => match addr.ip().to_ipv4_mapped() {
            Some(ipv4) => SocketAddr::new(IpAddr::V4(ipv4), addr.port()),
            None => endpoint,
        },
        SocketAddr::V4(_) => endpoint,
    }
}

fn canonical_endpoints(endpoints: &[SocketAddr]) -> Vec<SocketAddr> {
    let mut result = Vec::with_capacity(endpoints.len());
    for endpoint in endpoints.iter().copied().map(canonical_endpoint) {
        push_unique(&mut result, endpoint);
    }
    result
}

fn push_unique<T: PartialEq>(items: &mut Vec<T>, item: T) {
    if !items.contains(&item) {
        items.push(item);
    }
}

fn hex_prefix(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn control_variant(message: &ControlMessage) -> &'static str {
    match message {
        ControlMessage::Hello(_) => "HELLO",
        ControlMessage::Accept(_) => "ACCEPT",
        ControlMessage::Confirm(_) => "CONFIRM",
        ControlMessage::CookieChallenge(_) => "COOKIE_CHALLENGE",
        ControlMessage::HelloWithCookie(_) => "HELLO_WITH_COOKIE",
    }
}

fn unix_timestamp() -> Result<u64, SecureRuntimeError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| SecureRuntimeError::InvalidSystemClock)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p2p::admission::HandshakeAdmissionConfig;
    use fuckipv6_common::{NatType, PeerInfo};
    use std::net::{Ipv4Addr, Ipv6Addr};

    const INITIATOR_SEED: [u8; 32] = [0x11; 32];
    const RESPONDER_SEED: [u8; 32] = [0x22; 32];

    fn peer(
        client_id: &str,
        identity: &Ed25519Identity,
        virtual_ip: &str,
        endpoint: SocketAddr,
    ) -> PeerInfo {
        PeerInfo::new(
            client_id.to_string(),
            virtual_ip.parse::<Ipv6Addr>().expect("virtual IPv6"),
            identity.verifying_key().to_bytes().to_vec(),
            vec![endpoint],
            NatType::Open,
        )
    }

    fn admission() -> Arc<HandshakeAdmission> {
        HandshakeAdmission::shared_default()
    }

    fn cookie_key() -> Arc<CookieKey> {
        Arc::new(
            CookieKey::from_bytes([0x55; fuckipv6_secure_session::cookie::COOKIE_KEY_LEN])
                .expect("valid cookie key"),
        )
    }

    fn pair() -> (
        Arc<SecurePeerState>,
        Arc<SecurePeerState>,
        SocketAddr,
        SocketAddr,
    ) {
        pair_with_admission(admission())
    }

    fn pair_with_admission(
        admission: Arc<HandshakeAdmission>,
    ) -> (
        Arc<SecurePeerState>,
        Arc<SecurePeerState>,
        SocketAddr,
        SocketAddr,
    ) {
        let initiator_identity = Ed25519Identity::from_seed(INITIATOR_SEED);
        let responder_identity = Ed25519Identity::from_seed(RESPONDER_SEED);
        let initiator_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41001);
        let responder_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41002);
        let initiator_ip: IpAddr = "fd00::2".parse().expect("initiator IP");
        let responder_ip: IpAddr = "fd00::1".parse().expect("responder IP");
        let cookie_key = cookie_key();
        let state_a = SecurePeerState::new(
            initiator_identity.clone(),
            initiator_ip,
            &peer(
                "b-responder",
                &responder_identity,
                "fd00::1",
                responder_addr,
            ),
            admission.clone(),
            cookie_key.clone(),
        )
        .expect("state A");
        let state_b = SecurePeerState::new(
            responder_identity,
            responder_ip,
            &peer(
                "a-initiator",
                &initiator_identity,
                "fd00::2",
                initiator_addr,
            ),
            admission,
            cookie_key,
        )
        .expect("state B");
        if state_a.is_initiator() {
            (state_a, state_b, initiator_addr, responder_addr)
        } else {
            (state_b, state_a, responder_addr, initiator_addr)
        }
    }

    async fn establish(
        initiator: &SecurePeerState,
        responder: &SecurePeerState,
        initiator_addr: SocketAddr,
        responder_addr: SocketAddr,
    ) {
        let accept = accept_with_cookie(initiator, responder, initiator_addr, responder_addr).await;
        let confirm = initiator
            .handle_control(
                responder_addr,
                fuckipv6_secure_session::wire::decode_control(&accept[0].frame)
                    .expect("decode accept"),
            )
            .await
            .expect("accept response");
        assert_eq!(confirm.len(), 1);
        responder
            .handle_control(
                initiator_addr,
                fuckipv6_secure_session::wire::decode_control(&confirm[0].frame)
                    .expect("decode confirm"),
            )
            .await
            .expect("complete responder");
    }

    async fn accept_with_cookie(
        initiator: &SecurePeerState,
        responder: &SecurePeerState,
        initiator_addr: SocketAddr,
        responder_addr: SocketAddr,
    ) -> Vec<SecureControlAction> {
        let hello = initiator
            .initiate_frame()
            .await
            .expect("create hello")
            .expect("initiator");
        let challenge = responder
            .handle_control(
                initiator_addr,
                fuckipv6_secure_session::wire::decode_control(&hello).expect("decode hello"),
            )
            .await
            .expect("challenge hello");
        assert_eq!(challenge.len(), 1);
        assert_eq!(challenge[0].kind, "COOKIE_CHALLENGE");
        let retry = initiator
            .handle_control(
                responder_addr,
                fuckipv6_secure_session::wire::decode_control(&challenge[0].frame)
                    .expect("decode challenge"),
            )
            .await
            .expect("answer cookie challenge");
        assert_eq!(retry.len(), 1);
        assert_eq!(retry[0].kind, "HELLO_WITH_COOKIE");
        assert_eq!(retry[0].destination, responder_addr);
        let accept = responder
            .handle_control(
                initiator_addr,
                fuckipv6_secure_session::wire::decode_control(&retry[0].frame)
                    .expect("decode cookie retry"),
            )
            .await
            .expect("accept authenticated hello");
        assert_eq!(accept.len(), 1);
        assert_eq!(accept[0].kind, "ACCEPT");
        accept
    }

    #[tokio::test]
    async fn online_rekey_stages_confirm_retries_and_switches_epochs() {
        let (initiator, responder, initiator_addr, responder_addr) = pair();
        establish(&initiator, &responder, initiator_addr, responder_addr).await;
        assert_eq!(initiator.epoch().await, Some(1));
        assert_eq!(responder.epoch().await, Some(1));

        initiator.force_rekey_due().await;
        let accept =
            accept_with_cookie(&initiator, &responder, initiator_addr, responder_addr).await;

        let confirm = initiator
            .handle_control(
                responder_addr,
                fuckipv6_secure_session::wire::decode_control(&accept[0].frame)
                    .expect("decode rekey ACCEPT"),
            )
            .await
            .expect("stage rekey generation");
        assert_eq!(confirm.len(), 1);
        assert_eq!(
            initiator.epoch().await,
            Some(1),
            "initiator must retain old send keys until responder traffic proves commit"
        );
        let retry = initiator
            .retry_staged_confirm()
            .await
            .expect("retry staged CONFIRM")
            .expect("CONFIRM action");
        assert_eq!(retry.kind, "CONFIRM");
        assert_eq!(retry.frame, confirm[0].frame);

        responder
            .handle_control(
                initiator_addr,
                fuckipv6_secure_session::wire::decode_control(&retry.frame)
                    .expect("decode retried CONFIRM"),
            )
            .await
            .expect("activate responder generation");
        assert_eq!(responder.epoch().await, Some(2));

        // Direct UDP can still deliver an old-epoch packet during the receive
        // grace window even after the responder installed the replacement.
        let (_, old_packet) = initiator.seal(b"old epoch grace").await.expect("seal old");
        assert_eq!(
            responder
                .open(initiator_addr, old_packet)
                .await
                .expect("open retired epoch"),
            b"old epoch grace"
        );

        // The responder's first authenticated new-epoch packet is the initiator's
        // commit point and retires the old send generation.
        let (_, new_packet) = responder
            .seal(b"switch initiator")
            .await
            .expect("seal new epoch");
        assert_eq!(
            initiator
                .open(responder_addr, new_packet)
                .await
                .expect("switch initiator"),
            b"switch initiator"
        );
        assert_eq!(initiator.epoch().await, Some(2));
        assert!(initiator
            .retry_staged_confirm()
            .await
            .expect("CONFIRM retry state")
            .is_none());

        let (_, packet) = initiator
            .seal(b"new epoch send")
            .await
            .expect("seal new send key");
        assert_eq!(
            responder
                .open(initiator_addr, packet)
                .await
                .expect("open new epoch"),
            b"new epoch send"
        );
    }

    #[tokio::test]
    async fn handshake_admission_reservation_follows_pending_responder_lifecycle() {
        let admission = admission();
        let (initiator, responder, initiator_addr, responder_addr) =
            pair_with_admission(admission.clone());
        let hello = initiator
            .initiate_frame()
            .await
            .expect("create hello")
            .expect("initiator");
        let hello_message =
            fuckipv6_secure_session::wire::decode_control(&hello).expect("decode hello");

        let challenge = responder
            .handle_control(initiator_addr, hello_message.clone())
            .await
            .expect("challenge first hello");
        assert_eq!(challenge[0].kind, "COOKIE_CHALLENGE");
        assert_eq!(admission.metrics(Instant::now()).active_reservations, 0);
        let retry = initiator
            .handle_control(
                responder_addr,
                fuckipv6_secure_session::wire::decode_control(&challenge[0].frame)
                    .expect("decode challenge"),
            )
            .await
            .expect("answer challenge");
        let retry_message =
            fuckipv6_secure_session::wire::decode_control(&retry[0].frame).expect("decode retry");

        let accept = responder
            .handle_control(initiator_addr, retry_message.clone())
            .await
            .expect("authenticated hello");
        responder
            .handle_control(initiator_addr, retry_message)
            .await
            .expect("retransmitted authenticated hello");
        let metrics = admission.metrics(Instant::now());
        assert_eq!(metrics.admitted, 1);
        assert_eq!(metrics.active_reservations, 1);

        let confirm = initiator
            .handle_control(
                responder_addr,
                fuckipv6_secure_session::wire::decode_control(&accept[0].frame)
                    .expect("decode accept"),
            )
            .await
            .expect("accept response");
        responder
            .handle_control(
                initiator_addr,
                fuckipv6_secure_session::wire::decode_control(&confirm[0].frame)
                    .expect("decode confirm"),
            )
            .await
            .expect("complete responder");

        assert!(responder.is_established().await);
        assert_eq!(admission.metrics(Instant::now()).active_reservations, 0);
    }

    #[tokio::test]
    async fn failed_admission_is_fail_closed_and_valid_hello_recovers() {
        let admission = HandshakeAdmission::new(HandshakeAdmissionConfig {
            per_source: 1,
            per_identity: 1,
            global: 1,
            max_entries: 1,
            ttl: Duration::from_secs(30),
        })
        .expect("valid admission config");
        let occupier = admission
            .try_reserve(
                "192.0.2.200".parse().expect("occupier source"),
                [200; 32],
                Instant::now(),
            )
            .expect("occupy global slot");
        let (initiator, responder, initiator_addr, responder_addr) =
            pair_with_admission(admission.clone());
        let hello = initiator
            .initiate_frame()
            .await
            .expect("create hello")
            .expect("initiator");
        let hello_message =
            fuckipv6_secure_session::wire::decode_control(&hello).expect("decode hello");
        let before_challenge = admission.metrics(Instant::now());

        let challenge = responder
            .handle_control(initiator_addr, hello_message)
            .await
            .expect("challenge hello");
        let after_challenge = admission.metrics(Instant::now());
        assert_eq!(
            after_challenge.active_reservations,
            before_challenge.active_reservations
        );
        assert_eq!(after_challenge.admitted, before_challenge.admitted);
        let retry = initiator
            .handle_control(
                responder_addr,
                fuckipv6_secure_session::wire::decode_control(&challenge[0].frame)
                    .expect("decode challenge"),
            )
            .await
            .expect("answer challenge");
        let retry_message =
            fuckipv6_secure_session::wire::decode_control(&retry[0].frame).expect("decode retry");

        assert!(matches!(
            responder
                .handle_control(initiator_addr, retry_message.clone())
                .await,
            Err(SecureRuntimeError::Admission(
                HandshakeAdmissionError::GlobalLimit
            ))
        ));
        assert_eq!(admission.metrics(Instant::now()).rejected_global, 1);
        assert!(!responder.is_established().await);

        drop(occupier);
        responder
            .handle_control(initiator_addr, retry_message)
            .await
            .expect("valid hello progresses once pressure subsides");
        assert_eq!(admission.metrics(Instant::now()).active_reservations, 1);
    }

    #[tokio::test]
    async fn dual_stack_peer_metadata_uses_the_same_single_address_context() {
        let initiator_identity = Ed25519Identity::from_seed(INITIATOR_SEED);
        let responder_identity = Ed25519Identity::from_seed(RESPONDER_SEED);
        let initiator_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41101);
        let responder_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41102);
        let initiator_ip: IpAddr = "fd00::1003".parse().expect("initiator IP");
        let responder_ip: IpAddr = "fd00::1001".parse().expect("responder IP");

        let initiator_peer = PeerInfo::new_with_ips(
            "responder".to_string(),
            fuckipv6_common::VirtualIps::dual_stack(
                "10.66.0.1".parse().expect("responder IPv4"),
                "fd00::1001".parse().expect("responder IPv6"),
            ),
            responder_identity.verifying_key().to_bytes().to_vec(),
            vec![responder_addr],
            NatType::Open,
        );
        let responder_peer = PeerInfo::new_with_ips(
            "initiator".to_string(),
            fuckipv6_common::VirtualIps::dual_stack(
                "10.66.0.2".parse().expect("initiator IPv4"),
                "fd00::1003".parse().expect("initiator IPv6"),
            ),
            initiator_identity.verifying_key().to_bytes().to_vec(),
            vec![initiator_addr],
            NatType::Open,
        );
        let admission = admission();
        let cookie_key = cookie_key();
        let state_a = SecurePeerState::new(
            initiator_identity,
            initiator_ip,
            &initiator_peer,
            admission.clone(),
            cookie_key.clone(),
        )
        .expect("initiator state");
        let state_b = SecurePeerState::new(
            responder_identity,
            responder_ip,
            &responder_peer,
            admission,
            cookie_key,
        )
        .expect("responder state");
        let (initiator, responder, initiator_addr, responder_addr) = if state_a.is_initiator() {
            (state_a, state_b, initiator_addr, responder_addr)
        } else {
            (state_b, state_a, responder_addr, initiator_addr)
        };

        establish(&initiator, &responder, initiator_addr, responder_addr).await;
        assert!(initiator.is_established().await);
        assert!(responder.is_established().await);
    }

    #[tokio::test]
    async fn shared_state_establishes_and_exchanges_encrypted_data() {
        let (initiator, responder, initiator_addr, responder_addr) = pair();
        assert!(initiator.is_initiator());
        assert!(!responder.is_initiator());
        assert!(initiator.seal(b"before").await.is_err());

        establish(&initiator, &responder, initiator_addr, responder_addr).await;
        assert_eq!(initiator.session_id().await, responder.session_id().await);
        assert!(initiator.is_established().await);
        assert!(responder.is_established().await);
        assert_eq!(initiator.authenticated_peer().await, Some(responder_addr));
        assert_eq!(responder.authenticated_peer().await, Some(initiator_addr));

        let (destination, packet) = initiator.seal(b"request").await.expect("seal request");
        assert_eq!(destination, responder_addr);
        assert_ne!(packet, b"request");
        assert_eq!(
            responder
                .open(initiator_addr, packet)
                .await
                .expect("open request"),
            b"request"
        );

        let (destination, packet) = responder.seal(b"response").await.expect("seal response");
        assert_eq!(destination, initiator_addr);
        assert_eq!(
            initiator
                .open(responder_addr, packet)
                .await
                .expect("open response"),
            b"response"
        );
        assert_eq!(initiator.stats().await.expect("stats").packets_sent, 1);
        assert_eq!(responder.stats().await.expect("stats").packets_received, 1);
    }

    #[tokio::test]
    async fn control_frames_follow_authenticated_source_rebinding() {
        let (initiator, responder, registered_initiator_addr, registered_responder_addr) = pair();
        let accept_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 42002);
        let confirm_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 42003);
        assert_ne!(accept_addr, registered_responder_addr);
        assert_ne!(confirm_addr, registered_initiator_addr);

        let hello = initiator
            .initiate_frame()
            .await
            .expect("create hello")
            .expect("initiator");
        let hello_message =
            fuckipv6_secure_session::wire::decode_control(&hello).expect("decode hello");
        assert!(responder.matches_control(&hello_message).await);
        let challenge = responder
            .handle_control(registered_initiator_addr, hello_message)
            .await
            .expect("challenge registered hello");
        assert_eq!(challenge.len(), 1);
        assert_eq!(challenge[0].kind, "COOKIE_CHALLENGE");
        assert_eq!(challenge[0].destination, registered_initiator_addr);
        assert_eq!(initiator.authenticated_peer().await, None);
        assert_eq!(responder.authenticated_peer().await, None);

        let challenge_message = fuckipv6_secure_session::wire::decode_control(&challenge[0].frame)
            .expect("decode challenge");
        assert!(initiator.matches_control(&challenge_message).await);
        let retry = initiator
            .handle_control(registered_responder_addr, challenge_message)
            .await
            .expect("answer registered challenge");
        assert_eq!(retry.len(), 1);
        assert_eq!(retry[0].kind, "HELLO_WITH_COOKIE");
        assert_eq!(retry[0].destination, registered_responder_addr);
        assert_eq!(initiator.authenticated_peer().await, None);

        let accept = responder
            .handle_control(
                registered_initiator_addr,
                fuckipv6_secure_session::wire::decode_control(&retry[0].frame)
                    .expect("decode retry"),
            )
            .await
            .expect("accept registered cookie retry");
        assert_eq!(accept.len(), 1);
        assert_eq!(accept[0].destination, registered_initiator_addr);
        assert_eq!(responder.authenticated_peer().await, None);

        let accept_message =
            fuckipv6_secure_session::wire::decode_control(&accept[0].frame).expect("decode accept");
        assert!(initiator.matches_control(&accept_message).await);
        let confirm = initiator
            .handle_control(accept_addr, accept_message)
            .await
            .expect("accept rebound accept");
        assert_eq!(confirm.len(), 1);
        assert_eq!(confirm[0].destination, accept_addr);
        assert_eq!(initiator.authenticated_peer().await, Some(accept_addr));

        let confirm_message = fuckipv6_secure_session::wire::decode_control(&confirm[0].frame)
            .expect("decode confirm");
        assert!(responder.matches_control(&confirm_message).await);
        responder
            .handle_control(confirm_addr, confirm_message)
            .await
            .expect("accept rebound confirm");
        assert!(responder.is_established().await);
        assert_eq!(responder.authenticated_peer().await, Some(confirm_addr));
    }

    #[tokio::test]
    async fn initial_hello_gets_a_cheap_stateless_challenge() {
        let admission = admission();
        let (initiator, responder, initiator_addr, responder_addr) =
            pair_with_admission(admission.clone());
        let hello = initiator
            .initiate_frame()
            .await
            .expect("create hello")
            .expect("initiator");
        let before = admission.metrics(Instant::now());

        let challenge = responder
            .handle_control(
                initiator_addr,
                fuckipv6_secure_session::wire::decode_control(&hello).expect("decode hello"),
            )
            .await
            .expect("challenge hello");
        assert_eq!(challenge.len(), 1);
        assert_eq!(challenge[0].kind, "COOKIE_CHALLENGE");
        assert_eq!(challenge[0].destination, initiator_addr);

        let after = admission.metrics(Instant::now());
        assert_eq!(after.admitted, before.admitted);
        assert_eq!(after.active_reservations, before.active_reservations);
        let state = responder.state.lock().await;
        assert!(state.pending_responder.is_none());
        assert!(state.authenticated_peer.is_none());
        drop(state);

        let ControlMessage::CookieChallenge(mut wrong_peer) =
            fuckipv6_secure_session::wire::decode_control(&challenge[0].frame)
                .expect("decode challenge")
        else {
            panic!("expected cookie challenge");
        };
        wrong_peer.responder_identity[0] ^= 1;
        let wrong_peer = ControlMessage::CookieChallenge(wrong_peer);
        assert!(!initiator.matches_control(&wrong_peer).await);
        assert!(matches!(
            initiator.handle_control(responder_addr, wrong_peer).await,
            Err(SecureRuntimeError::UnexpectedFrame { .. })
        ));
        assert!(initiator.state.lock().await.pending_initiator.is_some());
    }

    #[tokio::test]
    async fn cookie_challenge_from_untrusted_source_is_rejected_without_rebinding() {
        let (initiator, responder, initiator_addr, responder_addr) = pair();
        let hello = initiator
            .initiate_frame()
            .await
            .expect("create hello")
            .expect("initiator");
        let challenge = responder
            .handle_control(
                initiator_addr,
                fuckipv6_secure_session::wire::decode_control(&hello).expect("decode hello"),
            )
            .await
            .expect("challenge hello");
        let attacker_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 44001);
        assert_ne!(attacker_addr, responder_addr);

        assert!(matches!(
            initiator
                .handle_control(
                    attacker_addr,
                    fuckipv6_secure_session::wire::decode_control(&challenge[0].frame)
                        .expect("decode challenge"),
                )
                .await,
            Err(SecureRuntimeError::UntrustedControlSource { actual }) if actual == attacker_addr
        ));
        let pending = initiator.state.lock().await;
        assert_eq!(
            pending
                .pending_initiator
                .as_ref()
                .expect("pending initiator")
                .hello_frame,
            hello
        );
        drop(pending);

        let retry = initiator
            .handle_control(
                responder_addr,
                fuckipv6_secure_session::wire::decode_control(&challenge[0].frame)
                    .expect("decode challenge"),
            )
            .await
            .expect("accept challenge from registered endpoint");
        assert_eq!(retry[0].kind, "HELLO_WITH_COOKIE");
        assert_eq!(retry[0].destination, responder_addr);
    }

    #[tokio::test]
    async fn cookie_retry_rejects_source_identity_ephemeral_and_mac_changes() {
        let admission = admission();
        let (initiator, responder, initiator_addr, responder_addr) =
            pair_with_admission(admission.clone());
        let hello = initiator
            .initiate_frame()
            .await
            .expect("create hello")
            .expect("initiator");
        let challenge = responder
            .handle_control(
                initiator_addr,
                fuckipv6_secure_session::wire::decode_control(&hello).expect("decode hello"),
            )
            .await
            .expect("challenge hello");
        let retry = initiator
            .handle_control(
                responder_addr,
                fuckipv6_secure_session::wire::decode_control(&challenge[0].frame)
                    .expect("decode challenge"),
            )
            .await
            .expect("answer challenge");
        let valid_frame = retry[0].frame.clone();
        let changed_source = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41999);

        assert!(matches!(
            responder
                .handle_control(
                    changed_source,
                    fuckipv6_secure_session::wire::decode_control(&valid_frame)
                        .expect("decode retry"),
                )
                .await,
            Err(SecureRuntimeError::Cookie(CookieError::InvalidMac))
        ));

        let mut tampered = valid_frame.clone();
        *tampered.last_mut().expect("cookie MAC") ^= 1;
        assert!(matches!(
            responder
                .handle_control(
                    initiator_addr,
                    fuckipv6_secure_session::wire::decode_control(&tampered)
                        .expect("decode tampered retry"),
                )
                .await,
            Err(SecureRuntimeError::Cookie(CookieError::InvalidMac))
        ));

        let ControlMessage::HelloWithCookie(mut changed_initiator) =
            fuckipv6_secure_session::wire::decode_control(&valid_frame).expect("decode retry")
        else {
            panic!("expected hello-with-cookie");
        };
        changed_initiator.hello.initiator_identity[0] ^= 1;
        assert!(matches!(
            responder
                .handle_control(
                    initiator_addr,
                    ControlMessage::HelloWithCookie(changed_initiator),
                )
                .await,
            Err(SecureRuntimeError::Cookie(CookieError::InvalidMac))
        ));

        let ControlMessage::HelloWithCookie(mut changed_responder) =
            fuckipv6_secure_session::wire::decode_control(&valid_frame).expect("decode retry")
        else {
            panic!("expected hello-with-cookie");
        };
        changed_responder.hello.responder_identity[0] ^= 1;
        assert!(matches!(
            responder
                .handle_control(
                    initiator_addr,
                    ControlMessage::HelloWithCookie(changed_responder),
                )
                .await,
            Err(SecureRuntimeError::Cookie(CookieError::InvalidMac))
        ));

        let ControlMessage::HelloWithCookie(mut changed_ephemeral) =
            fuckipv6_secure_session::wire::decode_control(&valid_frame).expect("decode retry")
        else {
            panic!("expected hello-with-cookie");
        };
        changed_ephemeral.hello.initiator_ephemeral_public[0] ^= 1;
        assert!(matches!(
            responder
                .handle_control(
                    initiator_addr,
                    ControlMessage::HelloWithCookie(changed_ephemeral),
                )
                .await,
            Err(SecureRuntimeError::Cookie(CookieError::InvalidMac))
        ));

        assert_eq!(admission.metrics(Instant::now()).active_reservations, 0);
        assert!(responder.state.lock().await.pending_responder.is_none());

        let first_accept = responder
            .handle_control(
                initiator_addr,
                fuckipv6_secure_session::wire::decode_control(&valid_frame)
                    .expect("decode valid retry"),
            )
            .await
            .expect("valid retry after rejected variants");
        let replay_accept = responder
            .handle_control(
                initiator_addr,
                fuckipv6_secure_session::wire::decode_control(&valid_frame)
                    .expect("decode replayed retry"),
            )
            .await
            .expect("byte-identical retry is idempotent while pending");
        assert_eq!(replay_accept, first_accept);
        let metrics = admission.metrics(Instant::now());
        assert_eq!(metrics.admitted, 1);
        assert_eq!(metrics.active_reservations, 1);
    }

    #[tokio::test]
    async fn expired_cookie_is_rejected_before_responder_crypto_and_admission() {
        let admission = admission();
        let (initiator, responder, initiator_addr, _responder_addr) =
            pair_with_admission(admission.clone());
        let hello = initiator
            .initiate_frame()
            .await
            .expect("create hello")
            .expect("initiator");
        let ControlMessage::Hello(hello) =
            fuckipv6_secure_session::wire::decode_control(&hello).expect("decode hello")
        else {
            panic!("expected hello");
        };
        let binding = CookieBinding::from_hello(initiator_addr, &hello).expect("cookie binding");
        let issued_at = unix_timestamp()
            .expect("system time")
            .saturating_sub(fuckipv6_secure_session::cookie::COOKIE_LIFETIME_SECS + 1);
        let frame = HelloWithCookieFrame {
            hello,
            cookie: responder.cookie_key.issue(&binding, issued_at),
        }
        .encode();

        assert!(matches!(
            responder
                .handle_control(
                    initiator_addr,
                    fuckipv6_secure_session::wire::decode_control(&frame)
                        .expect("decode expired retry"),
                )
                .await,
            Err(SecureRuntimeError::Cookie(CookieError::Expired))
        ));
        assert_eq!(admission.metrics(Instant::now()).active_reservations, 0);
        assert!(responder.state.lock().await.pending_responder.is_none());
    }

    #[tokio::test]
    async fn data_authentication_migrates_endpoint_only_after_success() {
        let (initiator, responder, initiator_addr, responder_addr) = pair();
        establish(&initiator, &responder, initiator_addr, responder_addr).await;

        let (_, packet) = initiator.seal(b"rebound").await.expect("seal");
        assert!(responder.handles_data_packet(&packet));
        assert!(!initiator.handles_data_packet(&packet));
        let rebound_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 43001);
        assert_eq!(
            responder
                .open_from_source(rebound_addr, packet.clone())
                .await
                .expect("authenticate rebound packet"),
            b"rebound"
        );
        assert_eq!(responder.authenticated_peer().await, Some(rebound_addr));

        let replay_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 43002);
        assert!(responder
            .open_from_source(replay_addr, packet)
            .await
            .is_err());
        assert_eq!(responder.authenticated_peer().await, Some(rebound_addr));

        let (_, mut tampered) = initiator.seal(b"tampered").await.expect("seal");
        *tampered.last_mut().expect("authentication tag") ^= 1;
        let attacker_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 43003);
        assert!(responder
            .open_from_source(attacker_addr, tampered)
            .await
            .is_err());
        assert_eq!(responder.authenticated_peer().await, Some(rebound_addr));
    }

    #[tokio::test]
    async fn forged_accept_and_confirm_do_not_consume_pending_handshakes() {
        let (initiator, responder, initiator_addr, responder_addr) = pair();
        let accepted =
            accept_with_cookie(&initiator, &responder, initiator_addr, responder_addr).await;
        assert_eq!(accepted.len(), 1);

        let valid_accept = fuckipv6_secure_session::wire::decode_control(&accepted[0].frame)
            .expect("decode accept");
        let forged_accept = match valid_accept.clone() {
            ControlMessage::Accept(mut accept) => {
                accept.responder_confirmation =
                    fuckipv6_secure_session::confirmation::ConfirmationTag::from_slice(
                        &[0x42; fuckipv6_secure_session::confirmation::CONFIRMATION_LEN],
                    )
                    .expect("confirmation tag");
                ControlMessage::Accept(accept)
            }
            _ => panic!("expected accept"),
        };
        assert!(initiator
            .handle_control(responder_addr, forged_accept)
            .await
            .is_err());

        let confirm = initiator
            .handle_control(responder_addr, valid_accept)
            .await
            .expect("forged ACCEPT must not consume pending initiator state");
        assert_eq!(confirm.len(), 1);
        assert_eq!(initiator.authenticated_peer().await, Some(responder_addr));

        let valid_confirm = fuckipv6_secure_session::wire::decode_control(&confirm[0].frame)
            .expect("decode confirm");
        let forged_confirm = match valid_confirm {
            ControlMessage::Confirm(mut confirm) => {
                confirm.initiator_confirmation =
                    fuckipv6_secure_session::confirmation::ConfirmationTag::from_slice(
                        &[0x43; fuckipv6_secure_session::confirmation::CONFIRMATION_LEN],
                    )
                    .expect("confirmation tag");
                ControlMessage::Confirm(confirm)
            }
            _ => panic!("expected confirm"),
        };
        assert!(responder
            .handle_control(initiator_addr, forged_confirm)
            .await
            .is_err());

        responder
            .handle_control(
                initiator_addr,
                fuckipv6_secure_session::wire::decode_control(&confirm[0].frame)
                    .expect("decode valid confirm"),
            )
            .await
            .expect("forged CONFIRM must not consume pending responder state");
        assert!(responder.is_established().await);
        assert_eq!(responder.authenticated_peer().await, Some(initiator_addr));
    }

    #[tokio::test]
    async fn routing_hint_does_not_accept_forged_hello() {
        let (initiator, responder, initiator_addr, responder_addr) = pair();
        let hello = initiator
            .initiate_frame()
            .await
            .expect("create hello")
            .expect("initiator");
        let mut hello_message =
            fuckipv6_secure_session::wire::decode_control(&hello).expect("decode hello");
        let fuckipv6_secure_session::wire::ControlMessage::Hello(ref mut hello) = hello_message
        else {
            panic!("expected hello");
        };
        hello.signature[0] ^= 1;
        assert!(responder.matches_control(&hello_message).await);
        let challenge = responder
            .handle_control(initiator_addr, hello_message)
            .await
            .expect("unauthenticated hello receives a cheap challenge");
        assert_eq!(challenge[0].kind, "COOKIE_CHALLENGE");
        let retry = initiator
            .handle_control(
                responder_addr,
                fuckipv6_secure_session::wire::decode_control(&challenge[0].frame)
                    .expect("decode challenge"),
            )
            .await
            .expect("answer challenge");
        let mut retry_message =
            fuckipv6_secure_session::wire::decode_control(&retry[0].frame).expect("decode retry");
        let ControlMessage::HelloWithCookie(ref mut retry) = retry_message else {
            panic!("expected hello-with-cookie");
        };
        retry.hello.signature[0] ^= 1;
        assert!(responder
            .handle_control(initiator_addr, retry_message)
            .await
            .is_err());
        assert_eq!(responder.authenticated_peer().await, None);
    }

    #[tokio::test]
    async fn revocation_invalidates_pending_handshakes_in_both_roles() {
        let (initiator, responder, initiator_addr, responder_addr) = pair();
        let hello = initiator
            .initiate_frame()
            .await
            .expect("create hello")
            .expect("initiator");
        let challenge = responder
            .handle_control(
                initiator_addr,
                fuckipv6_secure_session::wire::decode_control(&hello).expect("decode hello"),
            )
            .await
            .expect("challenge hello");
        let retry = initiator
            .handle_control(
                responder_addr,
                fuckipv6_secure_session::wire::decode_control(&challenge[0].frame)
                    .expect("decode challenge"),
            )
            .await
            .expect("answer challenge");

        assert!(responder.revoke().await);
        assert!(!responder.revoke().await);
        let retry_message =
            fuckipv6_secure_session::wire::decode_control(&retry[0].frame).expect("decode retry");
        assert!(!responder.matches_control(&retry_message).await);
        assert!(matches!(
            responder
                .handle_control(initiator_addr, retry_message)
                .await,
            Err(SecureRuntimeError::Revoked)
        ));
        assert!(!responder.is_established().await);
        assert_eq!(responder.authenticated_peer().await, None);

        let (initiator, responder, initiator_addr, responder_addr) = pair();
        let hello = initiator
            .initiate_frame()
            .await
            .expect("create hello")
            .expect("initiator");
        let challenge = responder
            .handle_control(
                initiator_addr,
                fuckipv6_secure_session::wire::decode_control(&hello).expect("decode hello"),
            )
            .await
            .expect("challenge hello");
        let retry = initiator
            .handle_control(
                responder_addr,
                fuckipv6_secure_session::wire::decode_control(&challenge[0].frame)
                    .expect("decode challenge"),
            )
            .await
            .expect("answer challenge");
        let accept = responder
            .handle_control(
                initiator_addr,
                fuckipv6_secure_session::wire::decode_control(&retry[0].frame)
                    .expect("decode retry"),
            )
            .await
            .expect("accept authenticated hello");

        assert!(initiator.revoke().await);
        assert!(matches!(
            initiator.initiate_frame().await,
            Err(SecureRuntimeError::Revoked)
        ));
        assert!(matches!(
            initiator
                .handle_control(
                    responder_addr,
                    fuckipv6_secure_session::wire::decode_control(&accept[0].frame)
                        .expect("decode accept"),
                )
                .await,
            Err(SecureRuntimeError::Revoked)
        ));
        assert!(!initiator.is_established().await);
        assert_eq!(initiator.authenticated_peer().await, None);
    }

    #[tokio::test]
    async fn revocation_is_scoped_to_the_session_instance_so_offline_peer_can_rejoin() {
        let (old_initiator, old_responder, initiator_addr, responder_addr) = pair();
        establish(
            &old_initiator,
            &old_responder,
            initiator_addr,
            responder_addr,
        )
        .await;
        assert!(old_initiator.revoke().await);
        assert!(old_responder.revoke().await);

        // PeerOffline revokes the current instance; a later PeerOnline registration
        // creates fresh state with the same node identities and must be usable.
        let (initiator, responder, initiator_addr, responder_addr) = pair();
        establish(&initiator, &responder, initiator_addr, responder_addr).await;
        let (_, packet) = initiator
            .seal(b"rejoined")
            .await
            .expect("seal after rejoin");
        assert_eq!(
            responder
                .open(initiator_addr, packet)
                .await
                .expect("open after rejoin"),
            b"rejoined"
        );
        assert!(matches!(
            old_initiator.seal(b"stale").await,
            Err(SecureRuntimeError::Revoked)
        ));
    }

    #[tokio::test]
    async fn session_permission_defers_revocation_until_transport_handoff() {
        let (initiator, _, _, _) = pair();
        let permission = initiator
            .acquire_session_permission()
            .await
            .expect("outbound permission");
        let revoker = initiator.clone();
        let mut revoke_task = tokio::spawn(async move { revoker.revoke().await });

        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut revoke_task)
                .await
                .is_err(),
            "revocation must wait for an admitted transport handoff"
        );
        drop(permission);
        assert!(tokio::time::timeout(Duration::from_secs(1), revoke_task)
            .await
            .expect("revocation should complete after handoff")
            .expect("revocation task"));
        assert!(matches!(
            initiator.acquire_session_permission().await,
            Err(SecureRuntimeError::Revoked)
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_revocation_stops_inflight_seal_and_receive() {
        let (initiator, responder, initiator_addr, responder_addr) = pair();
        establish(&initiator, &responder, initiator_addr, responder_addr).await;
        let (_, old_packet) = initiator.seal(b"before revoke").await.expect("seal");

        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let send_initiator = initiator.clone();
        let send_task = tokio::spawn(async move {
            let mut started_tx = Some(started_tx);
            loop {
                match send_initiator.seal(b"concurrent").await {
                    Ok(_) => {
                        if let Some(started_tx) = started_tx.take() {
                            let _ = started_tx.send(());
                        }
                        tokio::task::yield_now().await;
                    }
                    Err(SecureRuntimeError::Revoked) => return,
                    Err(error) => panic!("unexpected seal error during revocation: {error}"),
                }
            }
        });
        tokio::time::timeout(Duration::from_secs(1), started_rx)
            .await
            .expect("send task should start")
            .expect("send task should report startup");

        assert!(initiator.revoke().await);
        assert!(responder.revoke().await);
        tokio::time::timeout(Duration::from_secs(1), send_task)
            .await
            .expect("inflight send task should observe revocation")
            .expect("send task should not panic");

        assert!(matches!(
            initiator.seal(b"after revoke").await,
            Err(SecureRuntimeError::Revoked)
        ));
        assert!(matches!(
            responder.open(initiator_addr, old_packet).await,
            Err(SecureRuntimeError::Revoked)
        ));
    }

    #[tokio::test]
    async fn rejects_wrong_endpoint_tampering_and_replays() {
        let (initiator, responder, initiator_addr, responder_addr) = pair();
        establish(&initiator, &responder, initiator_addr, responder_addr).await;

        let (_, packet) = initiator.seal(b"protected").await.expect("seal");
        let wrong_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 49999);
        assert!(matches!(
            responder.open(wrong_addr, packet.clone()).await,
            Err(SecureRuntimeError::SessionEndpointMismatch { .. })
        ));

        assert_eq!(
            responder
                .open(initiator_addr, packet.clone())
                .await
                .expect("open original"),
            b"protected"
        );
        assert!(responder.open(initiator_addr, packet).await.is_err());
        assert_eq!(responder.stats().await.expect("stats").replay_rejections, 1);

        let (_, mut tampered) = initiator.seal(b"tamper me").await.expect("seal");
        *tampered.last_mut().expect("tag") ^= 1;
        assert!(responder.open(initiator_addr, tampered).await.is_err());
        assert_eq!(
            responder
                .stats()
                .await
                .expect("stats")
                .authentication_failures,
            1
        );
    }
}
