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
#[cfg(test)]
use fuckipv6_secure_session::driver::DriverStats;
use fuckipv6_secure_session::driver::{
    DriverError, InitiatorHandshake, ResponderHandshake, SessionDriver, SessionInstallConfig,
};
use fuckipv6_secure_session::handshake::{
    ClassicResponder, Ed25519Identity, HandshakeError, Hello,
};
use fuckipv6_secure_session::wire::{
    classic_endpoint_context, AcceptFrame, ClassicEndpointContextInput, ControlCodecError,
    ControlMessage,
};
use fuckipv6_secure_session::{AEAD_TAG_LEN, DATA_HEADER_LEN, MAX_DATA_PACKET_LEN};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::sync::Mutex;

use super::secure_session::SessionPolicy;

/// Network-separation domain bound into the v2 endpoint context.
const P2P_NETWORK_CONTEXT: &[u8] = b"fuckipv6-p2p-v2";
/// Replace an initiator HELLO this long before the responder freshness window
/// rejects its timestamp. The retry task sends the same HELLO until this age.
const HELLO_REFRESH_AFTER: Duration = Duration::from_secs(30);

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
    #[error("Secure Session v2 driver failed: {0}")]
    Driver(#[from] DriverError),
    #[error("Secure Session v2 is not established for this peer")]
    NotEstablished,
    #[error("Secure Session v2 handshake has no compatible initiator/responder role")]
    InvalidRole,
    #[error("peer has no registered UDP endpoint")]
    EmptyEndpointSet,
    #[error("datagram source {actual} is not a registered endpoint for this peer")]
    UnregisteredEndpoint { actual: SocketAddr },
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
}

struct PendingInitiator {
    handshake: InitiatorHandshake,
    issued_at: u64,
    hello_frame: Vec<u8>,
}

struct PendingResponder {
    handshake: ResponderHandshake,
    hello: Hello,
    accept_frame: Vec<u8>,
}

#[derive(Default)]
struct SecureState {
    driver: Option<SessionDriver>,
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
    initiator: bool,
    registered_endpoints: Vec<SocketAddr>,
    state: Mutex<SecureState>,
}

impl SecurePeerState {
    /// Build per-peer state after checking that the local and remote identities
    /// are distinct and that the registered endpoint set is non-empty.
    pub(crate) fn new(
        local_identity: Ed25519Identity,
        local_virtual_ips: &[IpAddr],
        peer: &PeerInfo,
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

        let mut peer_virtual_ips = vec![IpAddr::V6(peer.virtual_ip)];
        if let Some(ipv6) = peer.virtual_ips.ipv6 {
            push_unique(&mut peer_virtual_ips, IpAddr::V6(ipv6));
        }
        if let Some(ipv4) = peer.virtual_ips.ipv4 {
            push_unique(&mut peer_virtual_ips, IpAddr::V4(ipv4));
        }
        if peer_virtual_ips.is_empty() {
            return Err(SecureRuntimeError::Handshake(
                HandshakeError::InvalidEndpointContext,
            ));
        }

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
            (local_virtual_ips, peer_virtual_ips.as_slice())
        } else {
            (peer_virtual_ips.as_slice(), local_virtual_ips)
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

        Ok(Arc::new(Self {
            local_identity,
            expected_peer,
            endpoint_context,
            policy: SessionPolicy::default(),
            install_config,
            initiator,
            registered_endpoints,
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
        self.state.lock().await.driver.is_some()
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
        if !self.initiator {
            return Ok(None);
        }

        let mut state = self.state.lock().await;
        if state.driver.is_some() {
            return Ok(None);
        }
        let now = unix_timestamp()?;
        if let Some(pending) = state.pending_initiator.as_ref() {
            if now.saturating_sub(pending.issued_at) < HELLO_REFRESH_AFTER.as_secs() {
                return Ok(Some(pending.hello_frame.clone()));
            }
        }

        let (handshake, hello) = InitiatorHandshake::start(
            &self.local_identity,
            self.expected_peer,
            self.policy.preferred_suite().id(),
            self.endpoint_context,
            1,
            now,
            self.install_config,
            None,
        )?;
        let hello_frame = hello.encode_control();
        state.pending_initiator = Some(PendingInitiator {
            handshake,
            issued_at: now,
            hello_frame: hello_frame.clone(),
        });
        Ok(Some(hello_frame))
    }

    /// Process one authenticated or rejectable v2 control frame.
    pub(crate) async fn handle_control(
        &self,
        source: SocketAddr,
        message: ControlMessage,
    ) -> Result<Vec<SecureControlAction>, SecureRuntimeError> {
        self.ensure_registered(source)?;
        let mut state = self.state.lock().await;
        match message {
            ControlMessage::Hello(hello) => {
                if !self.initiator {
                    return self.accept_hello(&mut state, source, hello);
                }
                Err(SecureRuntimeError::UnexpectedFrame {
                    expected: "ACCEPT or CONFIRM",
                })
            }
            ControlMessage::Accept(accept) => {
                if !self.initiator {
                    return Err(SecureRuntimeError::UnexpectedFrame {
                        expected: "HELLO or CONFIRM",
                    });
                }
                let Some(pending) = state.pending_initiator.take() else {
                    return Err(SecureRuntimeError::UnexpectedFrame { expected: "HELLO" });
                };
                let (driver, confirm) =
                    pending
                        .handshake
                        .complete_wire(&accept, pending.issued_at, Instant::now())?;
                let confirm_frame = confirm.encode();
                state.driver = Some(driver);
                state.authenticated_peer = Some(source);
                Ok(vec![SecureControlAction {
                    destination: source,
                    frame: confirm_frame,
                    kind: "CONFIRM",
                }])
            }
            ControlMessage::Confirm(confirm) => {
                if self.initiator {
                    return Err(SecureRuntimeError::UnexpectedFrame { expected: "ACCEPT" });
                }
                if state.authenticated_peer == Some(source) && state.driver.is_some() {
                    // CONFIRM retransmissions are harmless after installation.
                    return Ok(Vec::new());
                }
                let Some(pending) = state.pending_responder.take() else {
                    return Err(SecureRuntimeError::UnexpectedFrame { expected: "HELLO" });
                };
                let (driver, _) = pending.handshake.complete_wire(&confirm, Instant::now())?;
                state.driver = Some(driver);
                state.authenticated_peer = Some(source);
                Ok(Vec::new())
            }
        }
    }

    fn accept_hello(
        &self,
        state: &mut SecureState,
        source: SocketAddr,
        hello: Hello,
    ) -> Result<Vec<SecureControlAction>, SecureRuntimeError> {
        if state.driver.is_some() {
            return Ok(Vec::new());
        }
        if let Some(pending) = state.pending_responder.as_ref() {
            if pending.hello == hello {
                return Ok(vec![SecureControlAction {
                    destination: source,
                    frame: pending.accept_frame.clone(),
                    kind: "ACCEPT",
                }]);
            }
        }

        let responder = state
            .responder
            .as_mut()
            .ok_or(SecureRuntimeError::InvalidRole)?;
        let issued_at = unix_timestamp()?;
        let (handshake, accept) = ResponderHandshake::accept(
            responder,
            &hello,
            self.endpoint_context,
            1,
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
            accept_frame: accept_frame.clone(),
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
        mut packet: Vec<u8>,
    ) -> Result<Vec<u8>, SecureRuntimeError> {
        let mut state = self.state.lock().await;
        let expected = state
            .authenticated_peer
            .ok_or(SecureRuntimeError::NotEstablished)?;
        if source != expected {
            return Err(SecureRuntimeError::SessionEndpointMismatch {
                expected,
                actual: source,
            });
        }
        let driver = state
            .driver
            .as_mut()
            .ok_or(SecureRuntimeError::NotEstablished)?;
        Ok(driver.open_in_place(&mut packet, Instant::now())?.to_vec())
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

    fn ensure_registered(&self, source: SocketAddr) -> Result<(), SecureRuntimeError> {
        let source = canonical_endpoint(source);
        if self.registered_endpoints.contains(&source) {
            Ok(())
        } else {
            Err(SecureRuntimeError::UnregisteredEndpoint { actual: source })
        }
    }
}

fn open_with_driver(
    state: &mut SecureState,
    mut packet: Vec<u8>,
) -> Result<Vec<u8>, SecureRuntimeError> {
    let driver = state
        .driver
        .as_mut()
        .ok_or(SecureRuntimeError::NotEstablished)?;
    Ok(driver.open_in_place(&mut packet, Instant::now())?.to_vec())
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

fn unix_timestamp() -> Result<u64, SecureRuntimeError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| SecureRuntimeError::InvalidSystemClock)
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn pair() -> (
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
        let state_a = SecurePeerState::new(
            initiator_identity.clone(),
            &[initiator_ip],
            &peer(
                "b-responder",
                &responder_identity,
                "fd00::1",
                responder_addr,
            ),
        )
        .expect("state A");
        let state_b = SecurePeerState::new(
            responder_identity,
            &[responder_ip],
            &peer(
                "a-initiator",
                &initiator_identity,
                "fd00::2",
                initiator_addr,
            ),
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
        let hello = initiator
            .initiate_frame()
            .await
            .expect("create hello")
            .expect("initiator");
        let accept = responder
            .handle_control(
                initiator_addr,
                fuckipv6_secure_session::wire::decode_control(&hello).expect("decode hello"),
            )
            .await
            .expect("accept hello");
        assert_eq!(accept.len(), 1);
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
