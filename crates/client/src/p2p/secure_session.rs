//! Experimental client adapter for the transport-independent Secure Session v2 driver.
//!
//! This module provides a dedicated-socket UDP handshake and data channel used
//! to validate the transport-independent driver. The production shared-socket
//! integration lives in [`super::secure_runtime`] and
//! [`super::connection::P2pEngine`], which reuse the same wire protocol and
//! fail-closed data-plane behavior.
//!
//! The adapter fails closed on malformed control frames, unsupported suites,
//! wrong peers, bad key confirmations, authentication failures and replays. A
//! reusable [`SecureSessionAcceptor`] retains the handshake replay cache across
//! repeated accepts for the same peer.

use ed25519_dalek::VerifyingKey;
use fuckipv6_secure_session::driver::{
    DriverError, DriverStats, InitiatorHandshake, ResponderHandshake, SessionDriver,
    SessionInstallConfig,
};
use fuckipv6_secure_session::handshake::{
    ClassicResponder, Ed25519Identity, HandshakeError, Hello,
};
use fuckipv6_secure_session::wire::{
    classic_endpoint_context, decode_control, AcceptFrame, ClassicEndpointContextInput,
    ConfirmFrame, ControlMessage,
};
use fuckipv6_secure_session::{
    DataCipherSuite, AEAD_TAG_LEN, DATA_HEADER_LEN, MAX_DATA_PACKET_LEN,
};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tokio::time::timeout;

/// Default time to wait for a handshake frame from the configured peer.
pub const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(3);
/// Default time to wait for one encrypted data packet.
pub const DEFAULT_DATA_TIMEOUT: Duration = Duration::from_secs(5);
/// Maximum UDP payload used by tokio's receive buffer.
const MAX_UDP_DATAGRAM_LEN: usize = u16::MAX as usize;

/// Ordered data-cipher suites supported by one endpoint.
///
/// The initiator offers its first suite. The responder accepts that offer only
/// when the suite is also present in its own policy. Unknown or unconfigured
/// suites fail before an `ACCEPT` is generated; there is no plaintext fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPolicy {
    preferred_suites: Vec<DataCipherSuite>,
}

impl SessionPolicy {
    /// Build a non-empty policy, removing duplicate suite entries.
    pub fn new(suites: impl IntoIterator<Item = DataCipherSuite>) -> Result<Self, SecureUdpError> {
        let mut preferred_suites = Vec::new();
        for suite in suites {
            if !preferred_suites.contains(&suite) {
                preferred_suites.push(suite);
            }
        }
        if preferred_suites.is_empty() {
            return Err(SecureUdpError::EmptySuitePolicy);
        }
        Ok(Self { preferred_suites })
    }

    /// Suite offered by a new initiator.
    pub fn preferred_suite(&self) -> DataCipherSuite {
        *self
            .preferred_suites
            .first()
            .expect("SessionPolicy is never empty")
    }

    /// Whether this endpoint accepts the wire suite identifier.
    pub fn supports(&self, suite_id: u16) -> bool {
        DataCipherSuite::from_id(suite_id)
            .is_some_and(|suite| self.preferred_suites.contains(&suite))
    }

    /// Suites in preference order.
    pub fn preferred_suites(&self) -> &[DataCipherSuite] {
        &self.preferred_suites
    }
}

impl Default for SessionPolicy {
    fn default() -> Self {
        Self {
            preferred_suites: vec![
                DataCipherSuite::ChaCha20Poly1305,
                DataCipherSuite::Aes256Gcm,
            ],
        }
    }
}

/// Per-connection epoch and receiver-routing labels required by the driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecureLinkConfig {
    pub epoch: u16,
    pub local_receiver_index: u32,
    pub peer_receiver_index: u32,
    pub timeout: Duration,
}

impl SecureLinkConfig {
    /// Construct an initial-epoch configuration for opposite routing labels.
    pub fn new(local_receiver_index: u32, peer_receiver_index: u32) -> Self {
        Self {
            epoch: 1,
            local_receiver_index,
            peer_receiver_index,
            timeout: DEFAULT_HANDSHAKE_TIMEOUT,
        }
    }

    fn install_config(self) -> SessionInstallConfig {
        SessionInstallConfig {
            local_receiver_index: self.local_receiver_index,
            peer_receiver_index: self.peer_receiver_index,
        }
    }
}

/// Network, virtual-address and direct-endpoint sets bound by a v2 handshake.
///
/// The contexts configured on two peers must describe the same two-sided sets
/// with their local and peer lists swapped. The connector and acceptor
/// automatically place those values into the initiator and responder transcript
/// roles. Direct endpoint lists may be empty for a relay-only deployment; both
/// virtual-IP lists must be non-empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecureEndpointContext {
    pub network_id: Vec<u8>,
    pub local_virtual_ips: Vec<IpAddr>,
    pub peer_virtual_ips: Vec<IpAddr>,
    pub local_endpoints: Vec<SocketAddr>,
    pub peer_endpoints: Vec<SocketAddr>,
}

impl SecureEndpointContext {
    /// Construct a relay-only context without direct candidate endpoints.
    pub fn new(
        network_id: impl Into<Vec<u8>>,
        local_virtual_ips: impl IntoIterator<Item = IpAddr>,
        peer_virtual_ips: impl IntoIterator<Item = IpAddr>,
    ) -> Self {
        Self {
            network_id: network_id.into(),
            local_virtual_ips: local_virtual_ips.into_iter().collect(),
            peer_virtual_ips: peer_virtual_ips.into_iter().collect(),
            local_endpoints: Vec::new(),
            peer_endpoints: Vec::new(),
        }
    }

    /// Add candidate direct endpoints for both sides.
    pub fn with_endpoints(
        mut self,
        local_endpoints: impl IntoIterator<Item = SocketAddr>,
        peer_endpoints: impl IntoIterator<Item = SocketAddr>,
    ) -> Self {
        self.local_endpoints = local_endpoints.into_iter().collect();
        self.peer_endpoints = peer_endpoints.into_iter().collect();
        self
    }

    fn initiator_hash(
        &self,
        initiator_public_key: &[u8],
        responder_public_key: &[u8],
    ) -> Result<[u8; 32], SecureUdpError> {
        classic_endpoint_context(&ClassicEndpointContextInput {
            network_id: &self.network_id,
            initiator_public_key,
            responder_public_key,
            initiator_virtual_ips: &self.local_virtual_ips,
            responder_virtual_ips: &self.peer_virtual_ips,
            initiator_endpoints: &self.local_endpoints,
            responder_endpoints: &self.peer_endpoints,
        })
        .map_err(Into::into)
    }

    fn responder_hash(
        &self,
        initiator_public_key: &[u8],
        responder_public_key: &[u8],
    ) -> Result<[u8; 32], SecureUdpError> {
        classic_endpoint_context(&ClassicEndpointContextInput {
            network_id: &self.network_id,
            initiator_public_key,
            responder_public_key,
            initiator_virtual_ips: &self.peer_virtual_ips,
            responder_virtual_ips: &self.local_virtual_ips,
            initiator_endpoints: &self.peer_endpoints,
            responder_endpoints: &self.local_endpoints,
        })
        .map_err(Into::into)
    }
}

/// Fail-closed errors from the UDP Secure Session adapter.
#[derive(Debug, Error)]
pub enum SecureUdpError {
    #[error("secure session UDP I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("secure session handshake control codec failed: {0}")]
    ControlCodec(#[from] fuckipv6_secure_session::wire::ControlCodecError),
    #[error("secure session handshake setup failed: {0}")]
    Handshake(#[from] HandshakeError),
    #[error("secure session driver failed: {0}")]
    Driver(#[from] DriverError),
    #[error("timed out waiting for secure session {0}")]
    Timeout(&'static str),
    #[error("received secure session datagram from {actual}, expected {expected}")]
    UnexpectedPeer {
        expected: SocketAddr,
        actual: SocketAddr,
    },
    #[error("expected secure session {expected} control frame")]
    UnexpectedControl { expected: &'static str },
    #[error("peer selected a suite outside the configured policy: {0:#06x}")]
    UnsupportedSuite(u16),
    #[error("peer public key is not a valid Ed25519 verification key")]
    InvalidPeerPublicKey,
    #[error("session policy must contain at least one suite")]
    EmptySuitePolicy,
    #[error("plaintext is too large for one v2 UDP packet: {len} bytes")]
    PayloadTooLarge { len: usize },
    #[error("system clock is before the Unix epoch")]
    InvalidSystemClock,
}

/// Authenticated v2 session bound to one dedicated UDP peer.
pub struct SecureUdpSession {
    socket: Arc<UdpSocket>,
    peer_addr: SocketAddr,
    driver: Mutex<SessionDriver>,
}

impl SecureUdpSession {
    /// Remote UDP address authenticated by the handshake.
    pub const fn peer_addr(&self) -> SocketAddr {
        self.peer_addr
    }

    /// Active negotiated session identifier.
    pub async fn session_id(&self) -> Option<[u8; fuckipv6_secure_session::SESSION_ID_LEN]> {
        self.driver.lock().await.session_id()
    }

    /// Snapshot of authenticated packet and rejection counters.
    pub async fn stats(&self) -> DriverStats {
        self.driver.lock().await.stats()
    }

    /// Seal one plaintext into a protected datagram without sending it.
    async fn seal_datagram(&self, plaintext: &[u8]) -> Result<Vec<u8>, SecureUdpError> {
        let packet_len = plaintext
            .len()
            .checked_add(DATA_HEADER_LEN + AEAD_TAG_LEN)
            .filter(|len| *len <= MAX_DATA_PACKET_LEN)
            .ok_or(SecureUdpError::PayloadTooLarge {
                len: plaintext.len(),
            })?;
        let mut packet = vec![0u8; packet_len];
        let encoded_len = self
            .driver
            .lock()
            .await
            .seal(plaintext, &mut packet, Instant::now())?;
        packet.truncate(encoded_len);
        Ok(packet)
    }

    /// Encrypt and send one plaintext datagram to the authenticated peer.
    pub async fn send(&self, plaintext: &[u8]) -> Result<usize, SecureUdpError> {
        let packet = self.seal_datagram(plaintext).await?;
        self.socket.send_to(&packet, self.peer_addr).await?;
        Ok(packet.len())
    }

    /// Receive, authenticate and decrypt one packet using an explicit timeout.
    pub async fn recv_with_timeout(&self, wait: Duration) -> Result<Vec<u8>, SecureUdpError> {
        let mut packet = vec![0u8; MAX_UDP_DATAGRAM_LEN];
        let (packet_len, source) = timeout(wait, self.socket.recv_from(&mut packet))
            .await
            .map_err(|_| SecureUdpError::Timeout("data packet"))??;
        if source != self.peer_addr {
            return Err(SecureUdpError::UnexpectedPeer {
                expected: self.peer_addr,
                actual: source,
            });
        }
        packet.truncate(packet_len);
        let plaintext = self
            .driver
            .lock()
            .await
            .open_in_place(&mut packet, Instant::now())?;
        Ok(plaintext.to_vec())
    }

    /// Receive one packet using [`DEFAULT_DATA_TIMEOUT`].
    pub async fn recv(&self) -> Result<Vec<u8>, SecureUdpError> {
        self.recv_with_timeout(DEFAULT_DATA_TIMEOUT).await
    }
}

/// Initiator-side configuration and expectations for one peer.
pub struct SecureSessionConnector {
    socket: Arc<UdpSocket>,
    peer_addr: SocketAddr,
    local_identity: Ed25519Identity,
    expected_peer: VerifyingKey,
    policy: SessionPolicy,
    endpoint_context: SecureEndpointContext,
}

impl SecureSessionConnector {
    /// Create a connector pinned to one peer address, identity and suite policy.
    pub fn new(
        socket: Arc<UdpSocket>,
        peer_addr: SocketAddr,
        local_identity: Ed25519Identity,
        expected_peer_public: [u8; 32],
        policy: SessionPolicy,
        endpoint_context: SecureEndpointContext,
    ) -> Result<Self, SecureUdpError> {
        let expected_peer = VerifyingKey::from_bytes(&expected_peer_public)
            .map_err(|_| SecureUdpError::InvalidPeerPublicKey)?;
        Ok(Self {
            socket,
            peer_addr,
            local_identity,
            expected_peer,
            policy,
            endpoint_context,
        })
    }

    /// Run one authenticated UDP handshake and return a ready data channel.
    pub async fn connect(
        self,
        config: SecureLinkConfig,
    ) -> Result<SecureUdpSession, SecureUdpError> {
        let issued_at = unix_timestamp()?;
        let endpoint_context = self.endpoint_context.initiator_hash(
            self.local_identity.verifying_key().as_bytes(),
            self.expected_peer.as_bytes(),
        )?;
        let (handshake, hello) = InitiatorHandshake::start(
            &self.local_identity,
            self.expected_peer,
            self.policy.preferred_suite().id(),
            endpoint_context,
            config.epoch,
            issued_at,
            config.install_config(),
            None,
        )?;

        self.socket
            .send_to(&hello.encode_control(), self.peer_addr)
            .await?;
        let accept = expect_accept(
            recv_peer_datagram(&self.socket, self.peer_addr, config.timeout, "ACCEPT").await?,
        )?;
        let (driver, confirm) = handshake.complete_wire(&accept, issued_at, Instant::now())?;
        self.socket
            .send_to(&confirm.encode(), self.peer_addr)
            .await?;

        Ok(SecureUdpSession {
            socket: self.socket,
            peer_addr: self.peer_addr,
            driver: Mutex::new(driver),
        })
    }
}

/// Reusable responder for one expected initiator and peer address.
pub struct SecureSessionAcceptor {
    socket: Arc<UdpSocket>,
    peer_addr: SocketAddr,
    responder: ClassicResponder,
    endpoint_context: [u8; fuckipv6_secure_session::transcript::ENDPOINT_CONTEXT_HASH_LEN],
    policy: SessionPolicy,
}

impl SecureSessionAcceptor {
    /// Create a responder pinned to one peer, identity and suite policy.
    pub fn new(
        socket: Arc<UdpSocket>,
        peer_addr: SocketAddr,
        local_identity: Ed25519Identity,
        expected_initiator_public: [u8; 32],
        policy: SessionPolicy,
        endpoint_context: SecureEndpointContext,
    ) -> Result<Self, SecureUdpError> {
        let expected_initiator = VerifyingKey::from_bytes(&expected_initiator_public)
            .map_err(|_| SecureUdpError::InvalidPeerPublicKey)?;
        let endpoint_context = endpoint_context.responder_hash(
            expected_initiator.as_bytes(),
            local_identity.verifying_key().as_bytes(),
        )?;
        let responder = ClassicResponder::new(local_identity, expected_initiator)?;
        Ok(Self {
            socket,
            peer_addr,
            responder,
            endpoint_context,
            policy,
        })
    }

    /// Accept one complete HELLO/ACCEPT/CONFIRM exchange from the pinned peer.
    pub async fn accept_handshake(
        &mut self,
        config: SecureLinkConfig,
    ) -> Result<SecureUdpSession, SecureUdpError> {
        let issued_at = unix_timestamp()?;
        let hello = expect_hello(
            recv_peer_datagram(&self.socket, self.peer_addr, config.timeout, "HELLO").await?,
        )?;
        if !self.policy.supports(hello.suite_id) {
            return Err(SecureUdpError::UnsupportedSuite(hello.suite_id));
        }

        let (handshake, accept) = ResponderHandshake::accept(
            &mut self.responder,
            &hello,
            self.endpoint_context,
            config.epoch,
            issued_at,
            config.install_config(),
            None,
        )?;
        let accept = AcceptFrame {
            accept,
            responder_confirmation: handshake.local_confirmation(),
        };
        self.socket
            .send_to(&accept.encode(), self.peer_addr)
            .await?;

        let confirm = expect_confirm(
            recv_peer_datagram(&self.socket, self.peer_addr, config.timeout, "CONFIRM").await?,
        )?;
        let (driver, _) = handshake.complete_wire(&confirm, Instant::now())?;
        Ok(SecureUdpSession {
            socket: self.socket.clone(),
            peer_addr: self.peer_addr,
            driver: Mutex::new(driver),
        })
    }
}

async fn recv_peer_datagram(
    socket: &UdpSocket,
    peer_addr: SocketAddr,
    wait: Duration,
    expected: &'static str,
) -> Result<Vec<u8>, SecureUdpError> {
    let mut datagram = vec![0u8; MAX_UDP_DATAGRAM_LEN];
    let (len, source) = timeout(wait, socket.recv_from(&mut datagram))
        .await
        .map_err(|_| SecureUdpError::Timeout(expected))??;
    if source != peer_addr {
        return Err(SecureUdpError::UnexpectedPeer {
            expected: peer_addr,
            actual: source,
        });
    }
    datagram.truncate(len);
    Ok(datagram)
}

fn expect_hello(datagram: Vec<u8>) -> Result<Hello, SecureUdpError> {
    match decode_control(&datagram)? {
        ControlMessage::Hello(hello) => Ok(hello),
        _ => Err(SecureUdpError::UnexpectedControl { expected: "HELLO" }),
    }
}

fn expect_accept(datagram: Vec<u8>) -> Result<AcceptFrame, SecureUdpError> {
    match decode_control(&datagram)? {
        ControlMessage::Accept(accept) => Ok(accept),
        _ => Err(SecureUdpError::UnexpectedControl { expected: "ACCEPT" }),
    }
}

fn expect_confirm(datagram: Vec<u8>) -> Result<ConfirmFrame, SecureUdpError> {
    match decode_control(&datagram)? {
        ControlMessage::Confirm(confirm) => Ok(confirm),
        _ => Err(SecureUdpError::UnexpectedControl {
            expected: "CONFIRM",
        }),
    }
}

fn unix_timestamp() -> Result<u64, SecureUdpError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| SecureUdpError::InvalidSystemClock)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuckipv6_secure_session::confirmation::{ConfirmationError, ConfirmationTag};
    use fuckipv6_secure_session::driver::DriverError;
    use fuckipv6_secure_session::handshake::HandshakeError;
    use fuckipv6_secure_session::handshake_epoch::HandshakeEpochError;
    use fuckipv6_secure_session::wire::ControlCodecError;
    use fuckipv6_secure_session::{SessionError, PROTOCOL_VERSION};

    const INITIATOR_INDEX: u32 = 0xa1a2_a3a4;
    const RESPONDER_INDEX: u32 = 0xb1b2_c3c4;

    fn identities() -> (Ed25519Identity, Ed25519Identity) {
        (
            Ed25519Identity::from_seed([0x11; 32]),
            Ed25519Identity::from_seed([0x22; 32]),
        )
    }

    fn link_config(local_receiver_index: u32, peer_receiver_index: u32) -> SecureLinkConfig {
        SecureLinkConfig {
            timeout: Duration::from_secs(1),
            ..SecureLinkConfig::new(local_receiver_index, peer_receiver_index)
        }
    }

    fn endpoint_context_pair() -> (SecureEndpointContext, SecureEndpointContext) {
        let network_id = b"secure-session-test-network".to_vec();
        let initiator_ip: IpAddr = "fd00::2".parse().expect("initiator IP");
        let responder_ip: IpAddr = "fd00::1".parse().expect("responder IP");
        (
            SecureEndpointContext::new(network_id.clone(), [initiator_ip], [responder_ip]),
            SecureEndpointContext::new(network_id, [responder_ip], [initiator_ip]),
        )
    }

    async fn socket_pair() -> (Arc<UdpSocket>, Arc<UdpSocket>) {
        (
            Arc::new(
                UdpSocket::bind("127.0.0.1:0")
                    .await
                    .expect("bind initiator"),
            ),
            Arc::new(
                UdpSocket::bind("127.0.0.1:0")
                    .await
                    .expect("bind responder"),
            ),
        )
    }

    async fn establish_pair() -> (SecureUdpSession, SecureUdpSession) {
        let (initiator, responder) = identities();
        let initiator_public = initiator.verifying_key().to_bytes();
        let responder_public = responder.verifying_key().to_bytes();
        let policy = SessionPolicy::default();
        let (initiator_socket, responder_socket) = socket_pair().await;
        let initiator_addr = initiator_socket.local_addr().expect("initiator addr");
        let responder_addr = responder_socket.local_addr().expect("responder addr");
        let (initiator_context, responder_context) = endpoint_context_pair();

        let mut acceptor = SecureSessionAcceptor::new(
            responder_socket,
            initiator_addr,
            responder,
            initiator_public,
            policy.clone(),
            responder_context,
        )
        .expect("create acceptor");
        let server = tokio::spawn(async move {
            acceptor
                .accept_handshake(link_config(RESPONDER_INDEX, INITIATOR_INDEX))
                .await
        });

        let connector = SecureSessionConnector::new(
            initiator_socket,
            responder_addr,
            initiator,
            responder_public,
            policy,
            initiator_context,
        )
        .expect("create connector");
        let client = connector
            .connect(link_config(INITIATOR_INDEX, RESPONDER_INDEX))
            .await
            .expect("connect initiator");
        let server = server
            .await
            .expect("acceptor task")
            .expect("accept responder");
        (client, server)
    }

    #[tokio::test]
    async fn real_udp_handshake_drives_bidirectional_data() {
        let (client, server) = establish_pair().await;
        assert_eq!(client.session_id().await, server.session_id().await);
        assert_ne!(client.session_id().await, Some([0; 16]));

        client
            .send(b"request over UDP")
            .await
            .expect("send request");
        assert_eq!(
            server.recv().await.expect("receive request"),
            b"request over UDP"
        );

        server
            .send(b"response over UDP")
            .await
            .expect("send response");
        assert_eq!(
            client.recv().await.expect("receive response"),
            b"response over UDP"
        );

        let client_stats = client.stats().await;
        let server_stats = server.stats().await;
        assert_eq!(client_stats.packets_sent, 1);
        assert_eq!(client_stats.packets_received, 1);
        assert_eq!(server_stats.packets_sent, 1);
        assert_eq!(server_stats.packets_received, 1);
    }

    #[tokio::test]
    async fn real_udp_tamper_and_replay_fail_closed_without_poisoning_session() {
        let (client, server) = establish_pair().await;

        let mut tampered = client
            .seal_datagram(b"tampered packet")
            .await
            .expect("seal tampered candidate");
        *tampered.last_mut().expect("AEAD tag") ^= 1;
        client
            .socket
            .send_to(&tampered, client.peer_addr)
            .await
            .expect("send tampered packet");
        assert!(matches!(
            server.recv().await,
            Err(SecureUdpError::Driver(DriverError::Data(
                SessionError::AuthenticationFailed
            )))
        ));
        assert_eq!(server.stats().await.authentication_failures, 1);

        let replayed = client
            .seal_datagram(b"replay me")
            .await
            .expect("seal replay candidate");
        client
            .socket
            .send_to(&replayed, client.peer_addr)
            .await
            .expect("send first copy");
        assert_eq!(server.recv().await.expect("open first copy"), b"replay me");
        client
            .socket
            .send_to(&replayed, client.peer_addr)
            .await
            .expect("send replay");
        assert!(matches!(
            server.recv().await,
            Err(SecureUdpError::Driver(DriverError::Data(
                SessionError::ReplayDetected
            )))
        ));
        assert_eq!(server.stats().await.replay_rejections, 1);

        client
            .send(b"session still healthy")
            .await
            .expect("send valid");
        assert_eq!(
            server.recv().await.expect("receive valid"),
            b"session still healthy"
        );
    }

    #[tokio::test]
    async fn network_rejects_unsupported_suite_and_old_protocol_version() {
        let (initiator, responder) = identities();
        let initiator_public = initiator.verifying_key().to_bytes();
        let responder_public = responder.verifying_key().to_bytes();
        let (initiator_socket, responder_socket) = socket_pair().await;
        let initiator_addr = initiator_socket.local_addr().expect("initiator addr");
        let responder_addr = responder_socket.local_addr().expect("responder addr");
        let policy = SessionPolicy::new([DataCipherSuite::ChaCha20Poly1305]).expect("policy");
        let (initiator_context, responder_context) = endpoint_context_pair();
        let mut acceptor = SecureSessionAcceptor::new(
            responder_socket,
            initiator_addr,
            responder,
            initiator_public,
            policy,
            responder_context,
        )
        .expect("create acceptor");

        let endpoint_context = initiator_context
            .initiator_hash(initiator.verifying_key().as_bytes(), &responder_public)
            .expect("endpoint context");
        let issued_at = unix_timestamp().expect("timestamp");
        let (_, hello) = InitiatorHandshake::start(
            &initiator,
            VerifyingKey::from_bytes(&responder_public).expect("responder key"),
            DataCipherSuite::Aes256Gcm.id(),
            endpoint_context,
            1,
            issued_at,
            SessionInstallConfig {
                local_receiver_index: INITIATOR_INDEX,
                peer_receiver_index: RESPONDER_INDEX,
            },
            None,
        )
        .expect("start initiator");
        let encoded = hello.encode_control();
        initiator_socket
            .send_to(&encoded, responder_addr)
            .await
            .expect("send unsupported hello");
        assert!(matches!(
            acceptor
                .accept_handshake(link_config(RESPONDER_INDEX, INITIATOR_INDEX))
                .await,
            Err(SecureUdpError::UnsupportedSuite(id))
                if id == DataCipherSuite::Aes256Gcm.id()
        ));

        let mut old_version = encoded;
        old_version[4] = PROTOCOL_VERSION.saturating_sub(1);
        initiator_socket
            .send_to(&old_version, responder_addr)
            .await
            .expect("send old version");
        assert!(matches!(
            acceptor
                .accept_handshake(link_config(RESPONDER_INDEX, INITIATOR_INDEX))
                .await,
            Err(SecureUdpError::ControlCodec(
                ControlCodecError::UnsupportedVersion { .. }
            ))
        ));
    }

    #[tokio::test]
    async fn replayed_hello_is_rejected_by_reused_acceptor() {
        let (initiator, responder) = identities();
        let initiator_public = initiator.verifying_key().to_bytes();
        let responder_public = responder.verifying_key().to_bytes();
        let (initiator_socket, responder_socket) = socket_pair().await;
        let initiator_addr = initiator_socket.local_addr().expect("initiator addr");
        let responder_addr = responder_socket.local_addr().expect("responder addr");
        let (initiator_context, responder_context) = endpoint_context_pair();
        let mut acceptor = SecureSessionAcceptor::new(
            responder_socket,
            initiator_addr,
            responder,
            initiator_public,
            SessionPolicy::default(),
            responder_context,
        )
        .expect("create acceptor");
        let endpoint_context = initiator_context
            .initiator_hash(initiator.verifying_key().as_bytes(), &responder_public)
            .expect("endpoint context");
        let (_, hello) = InitiatorHandshake::start(
            &initiator,
            VerifyingKey::from_bytes(&responder_public).expect("responder key"),
            DataCipherSuite::ChaCha20Poly1305.id(),
            endpoint_context,
            1,
            unix_timestamp().expect("timestamp"),
            SessionInstallConfig {
                local_receiver_index: INITIATOR_INDEX,
                peer_receiver_index: RESPONDER_INDEX,
            },
            None,
        )
        .expect("start initiator");
        let encoded = hello.encode_control();
        initiator_socket
            .send_to(&encoded, responder_addr)
            .await
            .expect("send hello");

        let mut short_config = link_config(RESPONDER_INDEX, INITIATOR_INDEX);
        short_config.timeout = Duration::from_millis(30);
        assert!(matches!(
            acceptor.accept_handshake(short_config).await,
            Err(SecureUdpError::Timeout("CONFIRM"))
        ));
        initiator_socket
            .send_to(&encoded, responder_addr)
            .await
            .expect("replay hello");
        assert!(matches!(
            acceptor.accept_handshake(short_config).await,
            Err(SecureUdpError::Driver(DriverError::Handshake(
                HandshakeError::ReplayDetected
            )))
        ));
    }

    #[tokio::test]
    async fn bad_initiator_confirmation_never_installs_a_server_session() {
        let (initiator, responder) = identities();
        let initiator_public = initiator.verifying_key().to_bytes();
        let responder_public = responder.verifying_key().to_bytes();
        let (initiator_socket, responder_socket) = socket_pair().await;
        let initiator_addr = initiator_socket.local_addr().expect("initiator addr");
        let responder_addr = responder_socket.local_addr().expect("responder addr");
        let (initiator_context, responder_context) = endpoint_context_pair();
        let mut acceptor = SecureSessionAcceptor::new(
            responder_socket,
            initiator_addr,
            responder,
            initiator_public,
            SessionPolicy::default(),
            responder_context,
        )
        .expect("create acceptor");
        let endpoint_context = initiator_context
            .initiator_hash(initiator.verifying_key().as_bytes(), &responder_public)
            .expect("endpoint context");
        let issued_at = unix_timestamp().expect("timestamp");
        let (handshake, hello) = InitiatorHandshake::start(
            &initiator,
            VerifyingKey::from_bytes(&responder_public).expect("responder key"),
            DataCipherSuite::ChaCha20Poly1305.id(),
            endpoint_context,
            1,
            issued_at,
            SessionInstallConfig {
                local_receiver_index: INITIATOR_INDEX,
                peer_receiver_index: RESPONDER_INDEX,
            },
            None,
        )
        .expect("start initiator");
        initiator_socket
            .send_to(&hello.encode_control(), responder_addr)
            .await
            .expect("send hello");
        let server = tokio::spawn(async move {
            acceptor
                .accept_handshake(link_config(RESPONDER_INDEX, INITIATOR_INDEX))
                .await
        });

        let accept = expect_accept(
            recv_peer_datagram(
                &initiator_socket,
                responder_addr,
                Duration::from_secs(1),
                "ACCEPT",
            )
            .await
            .expect("receive accept"),
        )
        .expect("decode accept");
        let (_, mut confirm) = handshake
            .complete_wire(&accept, issued_at, Instant::now())
            .expect("complete initiator handshake");
        confirm.initiator_confirmation =
            ConfirmationTag::from_slice(&[0x99; 32]).expect("confirmation");
        initiator_socket
            .send_to(&confirm.encode(), responder_addr)
            .await
            .expect("send bad confirm");
        assert!(matches!(
            server.await.expect("acceptor task"),
            Err(SecureUdpError::Driver(DriverError::EpochInstall(
                HandshakeEpochError::Confirmation(ConfirmationError::Mismatch)
            )))
        ));
    }
}
