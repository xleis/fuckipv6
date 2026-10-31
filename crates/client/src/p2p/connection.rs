use anyhow::{Context, Result};
use bytes::Bytes;
use fuckipv6_common::crypto::Identity;
use fuckipv6_common::{
    codec, ControlMessage, DataPacket, FallbackThreshold, NatDetectionConfig, NatType, P2pDatagram,
    PeerInfo, VirtualIps,
};
use fuckipv6_secure_session::cookie::CookieKey;
use fuckipv6_secure_session::wire::{decode_control, is_data_packet, CONTROL_MAGIC};
use fuckipv6_secure_session::{DataHeader, AEAD_TAG_LEN, DATA_HEADER_LEN, MAX_DATA_PACKET_LEN};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};
#[cfg(test)]
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, RwLock};
use tokio::time::{sleep, Duration, Instant};

use super::admission::{HandshakeAdmission, HandshakeAdmissionMetrics};
use super::buffer_pool::{
    InboundPayload, UdpBufferPool, UdpBufferPoolMetrics, DEFAULT_UDP_BUFFER_POOL_CAPACITY,
};
use super::health::{ConnectionHealthSnapshot, PeerHealth};
use super::inbound_queue::{
    self, InboundQueueMetrics, InboundQueueReceiver, InboundQueueSender,
    DEFAULT_INBOUND_QUEUE_CAPACITY, MAX_INBOUND_QUEUE_CAPACITY,
};
use super::secure_runtime::{SecureLockMetrics, SecurePeerState};
use super::udp_batch::{self, UdpBatchMetrics, UdpBatchStats, DEFAULT_UDP_BATCH_SIZE};

const HEALTH_WINDOW_SIZE: usize = 20;
const PROBE_RESPONSE_GRACE: Duration = Duration::from_secs(1);
const FAST_PROBE_ATTEMPTS: usize = 20;
const FAST_PROBE_INTERVAL: Duration = Duration::from_millis(500);
const HEALTH_PROBE_INTERVAL: Duration = Duration::from_secs(2);
const SECURE_HANDSHAKE_RETRY_INTERVAL: Duration = Duration::from_millis(250);

fn validate_inbound_queue_capacity(capacity: usize) -> Result<()> {
    if capacity == 0 || capacity > MAX_INBOUND_QUEUE_CAPACITY {
        anyhow::bail!(
            "入站队列容量必须在 1..={MAX_INBOUND_QUEUE_CAPACITY} 之间，实际为 {capacity}"
        );
    }
    Ok(())
}

/// A packet entering the client data plane with its authentication state made
/// explicit. UDP receiver traffic is already opened and must never be opened a
/// second time; relay traffic remains sealed until the central handler verifies
/// it against the registered peer session.
#[derive(Debug)]
pub(crate) enum InboundPacket {
    AuthenticatedPlaintext {
        src_ip: IpAddr,
        peer: PeerRouteInfo,
        payload: InboundPayload,
    },
    SealedCiphertext {
        src_ip: IpAddr,
        payload: Bytes,
    },
}

impl InboundPacket {
    /// Backing-allocation bytes retained while queued, used for byte admission.
    ///
    /// Authenticated UDP payloads may expose a short plaintext while retaining
    /// a full pooled receive buffer, so logical payload length would undercount
    /// queue memory by up to 64 KiB per packet.
    pub(crate) fn retained_len(&self) -> usize {
        match self {
            Self::AuthenticatedPlaintext { payload, .. } => payload.retained_len(),
            Self::SealedCiphertext { payload, .. } => payload.len(),
        }
    }
}

/// Result of trying the direct P2P transport for one already-sealed packet.
///
/// A fallback caller may forward `packet` over another transport, but must never
/// seal the plaintext again: all transports for one business packet share one
/// sequence number and one AEAD output.
pub(crate) enum SendToOutcome {
    Sent,
    SealedForFallback {
        #[cfg_attr(not(test), allow(dead_code))]
        packet: Vec<u8>,
        error: anyhow::Error,
    },
}

/// Result of trying a direct transport with ciphertext sealed in the caller's
/// existing packet buffer.
///
/// On fallback, the packet buffer still belongs to the caller and
/// `[..encoded_len]` is the one and only ciphertext allowed on fallback paths.
pub(crate) enum InPlaceSendOutcome {
    Sent,
    SealedForFallback {
        encoded_len: usize,
        error: anyhow::Error,
    },
}

#[cfg(test)]
struct SendRaceGate {
    sealed: tokio::sync::Barrier,
    release: tokio::sync::Barrier,
}

/// 一次主动 P2P 探针的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerProbeOutcome {
    pub client_id: String,
    pub virtual_ip: Ipv6Addr,
    pub endpoint: Option<SocketAddr>,
    pub rtt_us: Option<u64>,
    pub p2p_active: bool,
    pub timed_out: bool,
}

/// P2P连接
struct PeerConnection {
    peer_info: PeerInfo,
    endpoints: Vec<SocketAddr>,
    active_endpoint: Option<SocketAddr>,
    health: Arc<PeerHealth>,
    secure: Arc<SecurePeerState>,
}

/// Receiver-index route cached at peer registration time. The index is only a
/// pre-authentication demultiplexing hint; the packet still has to open against
/// the pointed-to session before any endpoint or payload is trusted.
struct ReceiverRoute {
    peer: PeerRouteInfo,
    secure: Arc<SecurePeerState>,
    health: Arc<PeerHealth>,
    src_ip: IpAddr,
}

/// Collision-safe receiver-index table. A `Vec` is normally length one, but it
/// preserves the old linear-scan behavior if two peer sessions ever derive the
/// same 32-bit index.
#[derive(Default)]
struct ReceiverIndex {
    by_index: HashMap<u32, Vec<Arc<ReceiverRoute>>>,
    by_endpoint: HashMap<(u32, SocketAddr), Vec<Arc<ReceiverRoute>>>,
    by_client: HashMap<String, Arc<ReceiverRoute>>,
    endpoint_keys: HashMap<String, Vec<(u32, SocketAddr)>>,
    #[cfg(test)]
    route_lookups: AtomicU64,
    #[cfg(test)]
    candidates_returned: AtomicU64,
}

impl ReceiverIndex {
    fn insert(
        &mut self,
        peer: PeerRouteInfo,
        secure: Arc<SecurePeerState>,
        health: Arc<PeerHealth>,
        src_ip: IpAddr,
        endpoints: &[SocketAddr],
    ) {
        self.remove_client(peer.client_id.as_ref());
        let receiver_index = secure.local_receiver_index();
        let route = Arc::new(ReceiverRoute {
            peer,
            secure,
            health,
            src_ip,
        });
        self.by_index
            .entry(receiver_index)
            .or_default()
            .push(route.clone());
        self.by_client
            .insert(route.peer.client_id.to_string(), route.clone());
        for endpoint in endpoints {
            self.add_endpoint_key(&route, canonical_udp_endpoint(*endpoint));
        }
    }

    fn remove_client(&mut self, client_id: &str) {
        let Some(route) = self.by_client.remove(client_id) else {
            return;
        };
        let receiver_index = route.secure.local_receiver_index();
        if let Some(routes) = self.by_index.get_mut(&receiver_index) {
            routes.retain(|candidate| !Arc::ptr_eq(candidate, &route));
            if routes.is_empty() {
                self.by_index.remove(&receiver_index);
            }
        }
        if let Some(keys) = self.endpoint_keys.remove(client_id) {
            for key in keys {
                if let Some(routes) = self.by_endpoint.get_mut(&key) {
                    routes.retain(|candidate| !Arc::ptr_eq(candidate, &route));
                    if routes.is_empty() {
                        self.by_endpoint.remove(&key);
                    }
                }
            }
        }
    }

    fn register_endpoint(
        &mut self,
        client_id: &str,
        secure: &Arc<SecurePeerState>,
        endpoint: SocketAddr,
    ) -> bool {
        let Some(route) = self.by_client.get(client_id).cloned() else {
            return false;
        };
        if !Arc::ptr_eq(&route.secure, secure) {
            return false;
        }
        self.add_endpoint_key(&route, canonical_udp_endpoint(endpoint));
        true
    }

    fn has_endpoint(
        &self,
        client_id: &str,
        secure: &Arc<SecurePeerState>,
        endpoint: SocketAddr,
    ) -> bool {
        let Some(route) = self.by_client.get(client_id) else {
            return false;
        };
        if !Arc::ptr_eq(&route.secure, secure) {
            return false;
        }
        let key = (
            route.secure.local_receiver_index(),
            canonical_udp_endpoint(endpoint),
        );
        self.endpoint_keys
            .get(client_id)
            .is_some_and(|keys| keys.contains(&key))
    }

    fn add_endpoint_key(&mut self, route: &Arc<ReceiverRoute>, endpoint: SocketAddr) {
        let key = (route.secure.local_receiver_index(), endpoint);
        let keys = self
            .endpoint_keys
            .entry(route.peer.client_id.to_string())
            .or_default();
        if !keys.contains(&key) {
            keys.push(key);
            self.by_endpoint.entry(key).or_default().push(route.clone());
        }
    }

    #[cfg(test)]
    fn routes(&self, receiver_index: u32) -> &[Arc<ReceiverRoute>] {
        self.by_index
            .get(&receiver_index)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// Prefer the registered endpoint as a non-authoritative routing hint. A
    /// NAT rebind falls back to every session sharing the 32-bit index; the
    /// packet is still accepted only after session authentication.
    fn candidate_snapshot(
        &self,
        receiver_index: u32,
        endpoint: SocketAddr,
    ) -> Option<(Arc<ReceiverRoute>, Vec<Arc<ReceiverRoute>>)> {
        #[cfg(test)]
        self.route_lookups.fetch_add(1, Ordering::Relaxed);
        let endpoint = canonical_udp_endpoint(endpoint);
        let mut routes = Vec::new();
        if let Some(endpoint_routes) = self.by_endpoint.get(&(receiver_index, endpoint)) {
            routes.extend(endpoint_routes.iter().cloned());
        }
        if let Some(index_routes) = self.by_index.get(&receiver_index) {
            for route in index_routes {
                if !routes.iter().any(|candidate| Arc::ptr_eq(candidate, route)) {
                    routes.push(route.clone());
                }
            }
        }
        let (first, rest) = routes.split_first()?;
        #[cfg(test)]
        self.candidates_returned.fetch_add(
            u64::try_from(routes.len()).expect("candidate count fits u64"),
            Ordering::Relaxed,
        );
        Some((first.clone(), rest.to_vec()))
    }
}

/// Immutable identity fields needed by data-plane source validation. Keeping
/// this separate from `PeerInfo` avoids copying public keys and endpoint lists
/// for every received packet.
#[derive(Debug, Clone)]
pub(crate) struct PeerRouteInfo {
    pub client_id: Arc<str>,
    pub virtual_ip: Ipv6Addr,
    pub virtual_ips: VirtualIps,
}

impl PeerRouteInfo {
    pub(crate) fn matches_ip(&self, ip: IpAddr) -> bool {
        self.virtual_ips.contains(ip) || matches!(ip, IpAddr::V6(ipv6) if self.virtual_ip == ipv6)
    }
}

impl From<&PeerInfo> for PeerRouteInfo {
    fn from(peer: &PeerInfo) -> Self {
        Self {
            client_id: Arc::from(peer.client_id.as_str()),
            virtual_ip: peer.virtual_ip,
            virtual_ips: peer.virtual_ips.clone(),
        }
    }
}

/// P2P引擎
#[derive(Clone)]
pub struct P2pEngine {
    local_addr: SocketAddr,
    socket: Arc<UdpSocket>,
    connections: Arc<RwLock<HashMap<String, PeerConnection>>>,
    ip_index: Arc<RwLock<HashMap<IpAddr, Arc<str>>>>,
    receiver_index: Arc<RwLock<ReceiverIndex>>,
    incoming_tx: InboundQueueSender,
    incoming_rx: Arc<Mutex<InboundQueueReceiver>>,
    udp_buffer_pool: Arc<UdpBufferPool>,
    udp_batch_size: usize,
    udp_batch_stats: Arc<UdpBatchStats>,
    local_ip: Arc<RwLock<Option<IpAddr>>>,
    local_identity: Arc<RwLock<Option<Identity>>>,
    handshake_admission: Arc<HandshakeAdmission>,
    cookie_key: Arc<CookieKey>,
    fallback_threshold: Arc<RwLock<FallbackThreshold>>,
    quic_mesh: Arc<RwLock<Option<super::quic::QuicP2pMesh>>>,
    #[cfg(test)]
    send_race_gate: Arc<RwLock<Option<Arc<SendRaceGate>>>>,
    #[cfg(test)]
    direct_seal_count: Arc<AtomicU64>,
    #[cfg(test)]
    fail_direct_transport: Arc<AtomicBool>,
    #[cfg(test)]
    send_batch_short_return: Arc<AtomicUsize>,
    pub overlay: Arc<super::overlay::P2pOverlay>,
}

impl P2pEngine {
    /// 创建P2P引擎
    pub async fn new(listen_port: u16) -> Result<Self> {
        Self::new_with_limits(
            listen_port,
            DEFAULT_INBOUND_QUEUE_CAPACITY,
            DEFAULT_UDP_BATCH_SIZE,
        )
        .await
    }

    /// Create a P2P engine with an explicit bounded ingress queue capacity.
    pub async fn new_with_inbound_queue_capacity(
        listen_port: u16,
        inbound_queue_capacity: usize,
    ) -> Result<Self> {
        Self::new_with_limits(listen_port, inbound_queue_capacity, DEFAULT_UDP_BATCH_SIZE).await
    }

    /// Create a P2P engine with explicit ingress queue and UDP syscall batches.
    pub async fn new_with_limits(
        listen_port: u16,
        inbound_queue_capacity: usize,
        udp_batch_size: usize,
    ) -> Result<Self> {
        validate_inbound_queue_capacity(inbound_queue_capacity)?;
        super::udp_batch::validate_udp_batch_size(udp_batch_size)?;
        let socket = bind_dual_stack_udp(listen_port).context("无法绑定 UDP socket")?;
        Self::from_socket_with_limits(socket, inbound_queue_capacity, udp_batch_size)
    }

    /// Bind the P2P transport once, run STUN on that exact socket, then start
    /// the shared UDP receiver. Keeping one socket preserves the NAT mapping
    /// discovered by STUN and prevents a temporary probe port from being
    /// advertised as the P2P endpoint.
    pub async fn new_with_nat_detection(
        config: &NatDetectionConfig,
        listen_port: u16,
    ) -> Result<(Self, super::punching::NatDetectionReport)> {
        Self::new_with_nat_detection_and_limits(
            config,
            listen_port,
            DEFAULT_INBOUND_QUEUE_CAPACITY,
            DEFAULT_UDP_BATCH_SIZE,
        )
        .await
    }

    /// NAT detection variant used by the runtime configuration path.
    pub async fn new_with_nat_detection_and_inbound_queue_capacity(
        config: &NatDetectionConfig,
        listen_port: u16,
        inbound_queue_capacity: usize,
    ) -> Result<(Self, super::punching::NatDetectionReport)> {
        Self::new_with_nat_detection_and_limits(
            config,
            listen_port,
            inbound_queue_capacity,
            DEFAULT_UDP_BATCH_SIZE,
        )
        .await
    }

    /// NAT detection variant with bounded ingress and UDP batch limits.
    pub async fn new_with_nat_detection_and_limits(
        config: &NatDetectionConfig,
        listen_port: u16,
        inbound_queue_capacity: usize,
        udp_batch_size: usize,
    ) -> Result<(Self, super::punching::NatDetectionReport)> {
        validate_inbound_queue_capacity(inbound_queue_capacity)?;
        super::udp_batch::validate_udp_batch_size(udp_batch_size)?;
        let socket = bind_dual_stack_udp(listen_port).context("无法绑定 UDP socket")?;
        let report = match super::punching::detect_nat_type_report_on_socket(config, &socket).await
        {
            Ok(report) => report,
            Err(error) => {
                tracing::warn!("NAT 类型检测失败，按 Unknown 处理: {error:#}");
                super::punching::NatDetectionReport {
                    nat_type: NatType::Unknown,
                    mapped_addresses: Vec::new(),
                    filtering_allows_changed_addr: false,
                }
            }
        };
        let engine = Self::from_socket_with_limits(socket, inbound_queue_capacity, udp_batch_size)?;
        Ok((engine, report))
    }

    fn from_socket_with_limits(
        socket: UdpSocket,
        inbound_queue_capacity: usize,
        udp_batch_size: usize,
    ) -> Result<Self> {
        validate_inbound_queue_capacity(inbound_queue_capacity)?;
        super::udp_batch::validate_udp_batch_size(udp_batch_size)?;
        let local_addr = socket.local_addr()?;
        tracing::info!("P2P 引擎监听地址: {}", local_addr);

        let socket = Arc::new(socket);
        let connections = Arc::new(RwLock::new(HashMap::new()));
        let ip_index = Arc::new(RwLock::new(HashMap::new()));
        let receiver_index = Arc::new(RwLock::new(ReceiverIndex::default()));
        let local_ip = Arc::new(RwLock::new(None));
        let local_identity = Arc::new(RwLock::new(None));
        let handshake_admission = HandshakeAdmission::shared_default();
        let cookie_key =
            Arc::new(CookieKey::generate().context("无法生成 Secure Session v2 cookie 密钥")?);
        let overlay = Arc::new(super::overlay::P2pOverlay::new(
            fuckipv6_common::NodeId::default(),
        ));
        let udp_buffer_pool = UdpBufferPool::new(DEFAULT_UDP_BUFFER_POOL_CAPACITY);
        let udp_batch_stats = Arc::new(UdpBatchStats::default());
        let (incoming_tx, incoming_rx, _) = inbound_queue::channel(inbound_queue_capacity);

        spawn_udp_receiver(
            socket.clone(),
            connections.clone(),
            ip_index.clone(),
            receiver_index.clone(),
            udp_buffer_pool.clone(),
            udp_batch_size,
            udp_batch_stats.clone(),
            local_ip.clone(),
            overlay.clone(),
            incoming_tx.clone(),
        );

        Ok(Self {
            local_addr,
            socket,
            connections,
            ip_index,
            receiver_index,
            incoming_tx,
            incoming_rx: Arc::new(Mutex::new(incoming_rx)),
            udp_buffer_pool,
            udp_batch_size,
            udp_batch_stats,
            local_ip,
            local_identity,
            handshake_admission,
            cookie_key,
            fallback_threshold: Arc::new(RwLock::new(FallbackThreshold::default())),
            quic_mesh: Arc::new(RwLock::new(None)),
            #[cfg(test)]
            send_race_gate: Arc::new(RwLock::new(None)),
            #[cfg(test)]
            direct_seal_count: Arc::new(AtomicU64::new(0)),
            #[cfg(test)]
            fail_direct_transport: Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            send_batch_short_return: Arc::new(AtomicUsize::new(usize::MAX)),
            overlay,
        })
    }

    /// 设置本节点的加密身份，用于自动派生与各对端节点的对称会话秘钥。
    pub async fn set_identity(&self, identity: Identity) {
        *self.local_identity.write().await = Some(identity.clone());
        self.overlay.set_identity(identity).await;
    }

    /// 使用客户端配置更新 P2P fallback 阈值。
    pub async fn set_fallback_threshold(&self, threshold: FallbackThreshold) {
        *self.fallback_threshold.write().await = threshold;
    }

    #[cfg(test)]
    pub async fn fallback_threshold(&self) -> FallbackThreshold {
        self.fallback_threshold.read().await.clone()
    }

    #[cfg(test)]
    pub fn direct_seal_count(&self) -> u64 {
        self.direct_seal_count.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub fn set_fail_direct_transport(&self, fail: bool) {
        self.fail_direct_transport.store(fail, Ordering::Relaxed);
    }

    #[cfg(test)]
    fn set_send_batch_short_return(&self, sent: usize) {
        self.send_batch_short_return.store(sent, Ordering::Relaxed);
    }

    /// 获取本地监听地址
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Snapshot UDP receive buffer allocation and reuse counters.
    pub fn udp_buffer_pool_metrics(&self) -> UdpBufferPoolMetrics {
        self.udp_buffer_pool.metrics()
    }

    /// Bounded ingress admission and depth counters for this engine.
    pub fn inbound_queue_metrics(&self) -> InboundQueueMetrics {
        self.incoming_tx.metrics()
    }

    /// Snapshot Linux UDP batch syscall counters and configured batch bound.
    pub fn udp_batch_metrics(&self) -> UdpBatchMetrics {
        self.udp_batch_stats.metrics()
    }

    pub fn udp_batch_size(&self) -> usize {
        self.udp_batch_size
    }

    /// 设置本节点虚拟 IP，用于 P2P 探测包。
    pub async fn set_local_ip(&self, ip: impl Into<IpAddr>) {
        let ip = ip.into();
        *self.local_ip.write().await = Some(ip);
        self.overlay.set_local_ip(ip).await;
    }

    /// 获取覆盖网络管理器 (包含 Onion/DHT/Chunk/E2EE)
    pub fn overlay(&self) -> Arc<super::overlay::P2pOverlay> {
        self.overlay.clone()
    }

    /// Snapshot handshake admission pressure without exposing peer secrets.
    pub fn handshake_admission_metrics(&self) -> HandshakeAdmissionMetrics {
        self.handshake_admission.metrics(std::time::Instant::now())
    }

    /// 启用端到端直连 QUIC P2P 网格能力
    pub async fn enable_quic_mesh(
        &self,
        bind_addr: SocketAddr,
    ) -> Result<fuckipv6_common::QuicCertificate> {
        let mesh = super::quic::QuicP2pMesh::new_with_admission(
            bind_addr,
            self.handshake_admission.clone(),
        )?;
        let cert = mesh.certificate().clone();
        *self.quic_mesh.write().await = Some(mesh);
        Ok(cert)
    }

    /// 获取当前 QUIC P2P 网格引擎（如果已启用）
    pub async fn quic_mesh(&self) -> Option<super::quic::QuicP2pMesh> {
        self.quic_mesh.read().await.clone()
    }

    /// 添加对等节点连接
    pub async fn add_peer(&self, peer_info: PeerInfo) -> Result<()> {
        let client_id = peer_info.client_id.clone();
        let client_key: Arc<str> = Arc::from(client_id.as_str());
        let endpoints = peer_info
            .endpoints
            .iter()
            .copied()
            .map(canonical_udp_endpoint)
            .fold(Vec::new(), |mut endpoints, endpoint| {
                if !endpoints.contains(&endpoint) {
                    endpoints.push(endpoint);
                }
                endpoints
            });

        if endpoints.is_empty() {
            anyhow::bail!("对等节点没有可用的连接地址");
        }

        tracing::info!(
            "添加对等节点: {} (主虚拟IP: {}, 双栈: {:?}) (地址: {:?})",
            client_id,
            peer_info.virtual_ip,
            peer_info.virtual_ips,
            endpoints
        );

        let local_identity = self
            .local_identity
            .read()
            .await
            .clone()
            .context("必须先设置本地身份再添加对等节点")?;
        let local_ip =
            (*self.local_ip.read().await).context("必须先设置本地虚拟 IP 再添加对等节点")?;
        let secure = SecurePeerState::new(
            local_identity.secure_session_identity(),
            local_ip,
            &peer_info,
            self.handshake_admission.clone(),
            self.cookie_key.clone(),
        )
        .with_context(|| format!("无法初始化与节点 {client_id} 的 Secure Session v2"))?;

        let connection = PeerConnection {
            peer_info: peer_info.clone(),
            active_endpoint: None,
            endpoints,
            health: Arc::new(PeerHealth::new(HEALTH_WINDOW_SIZE)),
            secure: secure.clone(),
        };

        let previous = {
            let connections = self.connections.read().await;
            connections
                .get(&client_id)
                .map(|connection| (connection.secure.clone(), peer_ips(&connection.peer_info)))
        };
        if let Some((secure, _)) = previous.as_ref() {
            // Retire the old session before publishing its replacement. The
            // per-session gate waits for an admitted send, but this does not hold
            // the global connection index or block unrelated peers.
            secure.revoke().await;
        }

        let mut connections = self.connections.write().await;
        if let Some(current) = connections.get(&client_id) {
            let expected = previous
                .as_ref()
                .is_some_and(|(secure, _)| Arc::ptr_eq(secure, &current.secure));
            if !expected {
                // Another registration completed while the old session waited for
                // in-flight outbound work. Do not clobber that newer instance.
                return Ok(());
            }
            connections.remove(&client_id);
        }
        let previous_ips = previous
            .as_ref()
            .map(|(_, ips)| ips.clone())
            .unwrap_or_default();
        {
            let mut ip_idx = self.ip_index.write().await;
            for ip in previous_ips {
                if ip_idx
                    .get(&ip)
                    .is_some_and(|mapped| mapped.as_ref() == client_id.as_str())
                {
                    ip_idx.remove(&ip);
                }
            }
            ip_idx.insert(IpAddr::V6(peer_info.virtual_ip), client_key.clone());
            if let Some(v6) = peer_info.virtual_ips.ipv6 {
                ip_idx.insert(IpAddr::V6(v6), client_key.clone());
            }
            if let Some(v4) = peer_info.virtual_ips.ipv4 {
                ip_idx.insert(IpAddr::V4(v4), client_key.clone());
            }
        }
        let receiver_endpoints = connection.endpoints.clone();
        let health = connection.health.clone();
        connections.insert(client_id.clone(), connection);
        let src_ip = peer_info
            .virtual_ips
            .primary_ip()
            .unwrap_or(IpAddr::V6(peer_info.virtual_ip));
        self.receiver_index.write().await.insert(
            PeerRouteInfo::from(&peer_info),
            secure.clone(),
            health,
            src_ip,
            &receiver_endpoints,
        );
        drop(connections);
        let primary_ip = peer_info
            .virtual_ips
            .primary_ip()
            .unwrap_or(IpAddr::V6(peer_info.virtual_ip));
        self.start_endpoint_probing(client_id.clone(), primary_ip);
        if secure.is_initiator() {
            spawn_secure_initiator(
                self.socket.clone(),
                self.connections.clone(),
                client_id,
                secure,
            );
        }

        Ok(())
    }

    /// Seal a payload for a registered peer with its established Secure Session v2.
    ///
    /// Relay and Exit Node traffic must never silently fall back to plaintext or
    /// the legacy static-X25519 format when key negotiation is unavailable.
    pub async fn encrypt_for_peer(
        &self,
        dst_ip: impl crate::stats::ToIpAddr,
        payload: &[u8],
    ) -> Result<Vec<u8>> {
        let dst_ip = dst_ip.to_ip_addr();
        let client_id = {
            let idx = self.ip_index.read().await;
            idx.get(&dst_ip).cloned()
        };
        let Some(client_id) = client_id else {
            anyhow::bail!("No connection to peer {dst_ip}");
        };

        let secure = {
            let connections = self.connections.read().await;
            connections
                .get(client_id.as_ref())
                .map(|connection| connection.secure.clone())
        };
        let Some(secure) = secure else {
            anyhow::bail!("No connection to peer {dst_ip}");
        };

        let _session = secure.acquire_session_permission().await?;
        let (_, packet) = secure
            .seal(payload)
            .await
            .with_context(|| format!("无法加密发往对等节点 {dst_ip} 的数据包"))?;
        Ok(packet)
    }

    /// Decrypt and authenticate a payload received from a registered peer.
    ///
    /// Unlike the old compatibility helper, this never returns the original
    /// payload when decryption fails. Callers handling routed or privileged data
    /// must treat any error as a dropped packet.
    pub async fn decrypt_from_peer(
        &self,
        src_ip: impl crate::stats::ToIpAddr,
        payload: &[u8],
    ) -> Result<Vec<u8>> {
        self.decrypt_owned_from_peer(src_ip, payload.to_vec()).await
    }

    /// Owned-payload variant used by the runtime receive queue. The receive
    /// queue already owns the relay ciphertext, so cloning it before in-place
    /// authentication is unnecessary.
    pub(crate) async fn decrypt_owned_from_peer(
        &self,
        src_ip: impl crate::stats::ToIpAddr,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>> {
        let src_ip = src_ip.to_ip_addr();
        let client_id = {
            let idx = self.ip_index.read().await;
            idx.get(&src_ip).cloned()
        };
        let Some(client_id) = client_id else {
            anyhow::bail!("No registered peer for source {src_ip}");
        };

        let (secure, health) = {
            let connections = self.connections.read().await;
            let Some(connection) = connections.get(client_id.as_ref()) else {
                anyhow::bail!("No registered peer for source {src_ip}");
            };
            (connection.secure.clone(), connection.health.clone())
        };
        let _session = secure.acquire_session_permission().await?;

        let payload_len = payload.len();
        let plaintext = secure
            .open_from_session(payload)
            .await
            .with_context(|| format!("来自对等节点 {src_ip} 的数据包认证失败"))?;
        health.record_payload(payload_len);
        Ok(plaintext)
    }

    /// Relay-owned ciphertext variant that preserves the codec allocation.
    ///
    /// The Bytes is moved into the session open path; a unique allocation is
    /// mutated in place, while a shared allocation takes one explicit copy.
    pub(crate) async fn decrypt_bytes_from_peer(
        &self,
        src_ip: impl crate::stats::ToIpAddr,
        payload: Bytes,
    ) -> Result<InboundPayload> {
        let src_ip = src_ip.to_ip_addr();
        let client_id = {
            let idx = self.ip_index.read().await;
            idx.get(&src_ip).cloned()
        };
        let Some(client_id) = client_id else {
            anyhow::bail!("No registered peer for source {src_ip}");
        };

        let (secure, health) = {
            let connections = self.connections.read().await;
            let Some(connection) = connections.get(client_id.as_ref()) else {
                anyhow::bail!("No registered peer for source {src_ip}");
            };
            (connection.secure.clone(), connection.health.clone())
        };
        let _session = secure.acquire_session_permission().await?;

        let payload_len = payload.len();
        let plaintext = secure
            .open_from_session_bytes(payload)
            .await
            .with_context(|| format!("来自对等节点 {src_ip} 的数据包认证失败"))?;
        health.record_payload(payload_len);
        Ok(InboundPayload::from_bytes(plaintext))
    }

    /// Try a direct P2P transport and return the already-sealed packet when a
    /// fallback transport is needed.
    ///
    /// The session permission covers sealing and the direct transport handoff.
    /// The returned ciphertext is the only packet fallback transports may send;
    /// re-encrypting it would burn another nonce for the same business packet.
    pub(crate) async fn send_to_or_fallback(
        &self,
        dst_ip: impl Into<IpAddr>,
        src_ip: impl Into<IpAddr>,
        payload: &[u8],
    ) -> Result<SendToOutcome> {
        let packet_len = payload
            .len()
            .checked_add(DATA_HEADER_LEN + AEAD_TAG_LEN)
            .filter(|len| *len <= MAX_DATA_PACKET_LEN)
            .context("encrypted packet length overflow")?;
        let mut packet = vec![0u8; packet_len];
        packet[DATA_HEADER_LEN..DATA_HEADER_LEN + payload.len()].copy_from_slice(payload);

        match self
            .send_to_or_fallback_in_place(dst_ip, src_ip, &mut packet, payload.len())
            .await?
        {
            InPlaceSendOutcome::Sent => Ok(SendToOutcome::Sent),
            InPlaceSendOutcome::SealedForFallback { encoded_len, error } => {
                packet.truncate(encoded_len);
                Ok(SendToOutcome::SealedForFallback { packet, error })
            }
        }
    }

    /// Seal in the caller's reserved packet buffer and try the direct P2P
    /// transports without allocating a second ciphertext buffer.
    ///
    /// The plaintext must already occupy
    /// `packet[DATA_HEADER_LEN..DATA_HEADER_LEN + plaintext_len]`; the caller
    /// must leave room for the authenticated tag. The session permission covers
    /// sealing and the direct transport handoff. A fallback caller may forward
    /// `packet[..encoded_len]`, but must never seal the plaintext again.
    pub(crate) async fn send_to_or_fallback_in_place(
        &self,
        dst_ip: impl Into<IpAddr>,
        _src_ip: impl Into<IpAddr>,
        packet: &mut [u8],
        plaintext_len: usize,
    ) -> Result<InPlaceSendOutcome> {
        let dst_ip = dst_ip.into();

        let client_id = {
            let idx = self.ip_index.read().await;
            idx.get(&dst_ip).cloned()
        };

        let Some(client_id) = client_id else {
            anyhow::bail!("No connection to peer {}", dst_ip);
        };

        let (secure, health) = {
            let connections = self.connections.read().await;
            connections
                .get(client_id.as_ref())
                .map(|connection| (connection.secure.clone(), connection.health.clone()))
        }
        .ok_or_else(|| anyhow::anyhow!("No connection to peer {dst_ip}"))?;

        // Admission and the actual transport handoff stay inside this per-session
        // gate. Revocation takes the same gate for writing, so it cannot return
        // while an admitted packet is still being sent, without blocking removal
        // of unrelated peers on a slow socket.
        let session = secure.acquire_session_permission().await?;

        let (destination, encoded_len) = secure
            .seal_in_place(packet, plaintext_len)
            .await
            .context("对等节点尚无已确认的 Secure Session v2，拒绝发送数据")?;

        #[cfg(test)]
        self.direct_seal_count.fetch_add(1, Ordering::Relaxed);

        #[cfg(test)]
        if self.fail_direct_transport.load(Ordering::Relaxed) {
            return Ok(InPlaceSendOutcome::SealedForFallback {
                encoded_len,
                error: anyhow::anyhow!("injected direct transport failure"),
            });
        }

        #[cfg(test)]
        if let Some(gate) = self.send_race_gate.read().await.clone() {
            gate.sealed.wait().await;
            gate.release.wait().await;
        }

        // QUIC is only a transport here; the same v2 ciphertext is required on
        // every path so relay or direct transport cannot downgrade protection.
        if let IpAddr::V6(dst_v6) = dst_ip {
            if let Some(mesh) = self.quic_mesh.read().await.as_ref() {
                if mesh.has_peer(&dst_v6).await
                    && mesh
                        .send_packet(&dst_v6, &packet[..encoded_len])
                        .await
                        .is_ok()
                {
                    drop(session);
                    health.record_payload(encoded_len);
                    return Ok(InPlaceSendOutcome::Sent);
                }
            }
        }

        match self
            .socket
            .send_to(
                &packet[..encoded_len],
                dual_stack_send_endpoint(destination),
            )
            .await
        {
            Ok(_) => {
                drop(session);
                health.record_payload(encoded_len);
                Ok(InPlaceSendOutcome::Sent)
            }
            Err(error) => Ok(InPlaceSendOutcome::SealedForFallback {
                encoded_len,
                error: error.into(),
            }),
        }
    }

    /// Seal a contiguous batch for one peer and hand the sealed prefix to a
    /// single Linux `sendmmsg` call. Every buffer is sealed before the syscall,
    /// so accepted packets keep nonce order. If the kernel accepts only a
    /// prefix, only the unsent sealed suffix is returned for fallback.
    pub(crate) async fn send_batch_to_or_fallback_in_place(
        &self,
        dst_ip: IpAddr,
        mut packets: Vec<(&mut [u8], usize)>,
    ) -> Result<Vec<Result<InPlaceSendOutcome>>> {
        if packets.is_empty() {
            return Ok(Vec::new());
        }

        // Preserve the existing QUIC-mesh preference. It cannot be represented
        // by sendmmsg, so use the per-packet path when that transport is active.
        if let IpAddr::V6(dst_v6) = dst_ip {
            if let Some(mesh) = self.quic_mesh.read().await.as_ref() {
                if mesh.has_peer(&dst_v6).await {
                    let mut outcomes = Vec::with_capacity(packets.len());
                    for (buffer, plaintext_len) in packets {
                        outcomes.push(
                            self.send_to_or_fallback_in_place(
                                dst_ip,
                                dst_ip,
                                buffer,
                                plaintext_len,
                            )
                            .await,
                        );
                    }
                    return Ok(outcomes);
                }
            }
        }

        let client_id = {
            let idx = self.ip_index.read().await;
            idx.get(&dst_ip).cloned()
        }
        .ok_or_else(|| anyhow::anyhow!("No connection to peer {dst_ip}"))?;

        let (secure, health) = {
            let connections = self.connections.read().await;
            connections
                .get(client_id.as_ref())
                .map(|connection| (connection.secure.clone(), connection.health.clone()))
        }
        .ok_or_else(|| anyhow::anyhow!("No connection to peer {dst_ip}"))?;

        // One admission covers the seal loop and the batched transport handoff.
        // Revocation cannot return while these nonces are still being sent.
        let session = secure.acquire_session_permission().await?;

        let mut outcomes: Vec<Option<Result<InPlaceSendOutcome>>> =
            packets.iter().map(|_| None).collect();
        let mut sealed: Vec<Option<(usize, std::net::SocketAddr)>> =
            packets.iter().map(|_| None).collect();
        let plaintext_lens: Vec<usize> = packets.iter().map(|(_, len)| *len).collect();
        let mut slots: Vec<&mut [u8]> = Vec::with_capacity(packets.len());
        for (buffer, _) in packets.iter_mut() {
            slots.push(&mut **buffer);
        }

        match secure
            .seal_batch_in_place(&mut slots, &plaintext_lens)
            .await
        {
            Ok(destination) => {
                drop(slots);
                for (index, plaintext_len) in plaintext_lens.iter().copied().enumerate() {
                    let encoded_len = plaintext_len + DATA_HEADER_LEN + AEAD_TAG_LEN;
                    sealed[index] = Some((encoded_len, destination));
                    #[cfg(test)]
                    self.direct_seal_count.fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(error) => {
                let error = anyhow::Error::new(error)
                    .context("对等节点尚无已确认的 Secure Session v2，拒绝发送数据");
                for outcome in &mut outcomes {
                    *outcome = Some(Err(anyhow::anyhow!("{error:#}")));
                }
            }
        }

        #[cfg(test)]
        if self.fail_direct_transport.load(Ordering::Relaxed) {
            for (index, item) in sealed.iter().enumerate() {
                if let Some((encoded_len, _)) = item {
                    self.udp_batch_stats.record_fallback_packet();
                    outcomes[index] = Some(Ok(InPlaceSendOutcome::SealedForFallback {
                        encoded_len: *encoded_len,
                        error: anyhow::anyhow!("injected direct transport failure"),
                    }));
                }
            }
            return Ok(outcomes
                .into_iter()
                .map(|outcome| {
                    outcome.unwrap_or_else(|| {
                        Err(anyhow::anyhow!(
                            "UDP batch stopped after an earlier seal failure"
                        ))
                    })
                })
                .collect());
        }

        #[cfg(test)]
        if let Some(gate) = self.send_race_gate.read().await.clone() {
            gate.sealed.wait().await;
            gate.release.wait().await;
        }

        // Only a contiguous prefix targeting the same endpoint can be sent with
        // one destination per sendmmsg. Any sealed suffix remains usable only
        // as the exact same ciphertext on a fallback transport.
        let mut prefix_len = 0usize;
        if let Some((_, first_destination)) = sealed.first().and_then(Option::as_ref) {
            for item in &sealed {
                match item {
                    Some((_, destination)) if destination == first_destination => {
                        prefix_len += 1;
                    }
                    _ => break,
                }
            }
        }

        if prefix_len > 0 {
            let destination = sealed[0]
                .as_ref()
                .expect("prefix implies a sealed first packet")
                .1;
            let send_refs: Vec<&[u8]> = packets[..prefix_len]
                .iter()
                .zip(&sealed[..prefix_len])
                .map(|((buffer, _), sealed)| {
                    let encoded_len = sealed.as_ref().expect("prefix packets are sealed").0;
                    &buffer[..encoded_len]
                })
                .collect();

            let send_result = {
                #[cfg(test)]
                {
                    let injected = self.send_batch_short_return.load(Ordering::Relaxed);
                    if injected == usize::MAX {
                        udp_batch::send_batch(
                            &self.socket,
                            dual_stack_send_endpoint(destination),
                            &send_refs,
                        )
                        .await
                    } else {
                        Ok(injected.min(send_refs.len()))
                    }
                }
                #[cfg(not(test))]
                {
                    udp_batch::send_batch(
                        &self.socket,
                        dual_stack_send_endpoint(destination),
                        &send_refs,
                    )
                    .await
                }
            };
            match send_result {
                Ok(sent) => {
                    self.udp_batch_stats.record_send(sent);
                    for (index, item) in sealed.iter().enumerate().take(prefix_len) {
                        let encoded_len = item.as_ref().expect("prefix packets are sealed").0;
                        if index < sent {
                            outcomes[index] = Some(Ok(InPlaceSendOutcome::Sent));
                            health.record_payload(encoded_len);
                        } else {
                            self.udp_batch_stats.record_fallback_packet();
                            outcomes[index] = Some(Ok(InPlaceSendOutcome::SealedForFallback {
                                encoded_len,
                                error: anyhow::anyhow!(
                                    "sendmmsg accepted only {sent} of {prefix_len} datagrams"
                                ),
                            }));
                        }
                    }
                }
                Err(error) => {
                    self.udp_batch_stats.record_send(0);
                    for (index, item) in sealed.iter().enumerate().take(prefix_len) {
                        let encoded_len = item.as_ref().expect("prefix packets are sealed").0;
                        self.udp_batch_stats.record_fallback_packet();
                        outcomes[index] = Some(Ok(InPlaceSendOutcome::SealedForFallback {
                            encoded_len,
                            error: anyhow::anyhow!("sendmmsg failed: {error}"),
                        }));
                    }
                }
            }
        }

        for (index, item) in sealed.iter().enumerate().skip(prefix_len) {
            if let Some((encoded_len, _)) = item {
                self.udp_batch_stats.record_fallback_packet();
                outcomes[index] = Some(Ok(InPlaceSendOutcome::SealedForFallback {
                    encoded_len: *encoded_len,
                    error: anyhow::anyhow!(
                        "endpoint changed within UDP batch; preserving nonce order requires fallback"
                    ),
                }));
            }
        }

        drop(session);
        Ok(outcomes
            .into_iter()
            .map(|outcome| {
                outcome.unwrap_or_else(|| {
                    Err(anyhow::anyhow!(
                        "UDP batch stopped after an earlier seal failure"
                    ))
                })
            })
            .collect())
    }

    /// 发送数据包到对等节点 (支持 IPv4 / IPv6)
    pub async fn send_to(
        &self,
        dst_ip: impl Into<IpAddr>,
        src_ip: impl Into<IpAddr>,
        payload: &[u8],
    ) -> Result<()> {
        match self.send_to_or_fallback(dst_ip, src_ip, payload).await? {
            SendToOutcome::Sent => Ok(()),
            SendToOutcome::SealedForFallback { error, .. } => Err(error),
        }
    }

    /// 接收数据包 (支持 IPv4 / IPv6)
    pub(crate) async fn recv(&self) -> Option<InboundPacket> {
        // The queue is single-consumer by protocol. The mutex is held only while
        // waiting for/removing the next packet, never across decryption or I/O.
        self.incoming_rx.lock().await.recv().await
    }

    /// 移除对等节点（通过客户端ID）
    pub async fn remove_peer_by_id(&self, client_id: &str) -> Option<IpAddr> {
        let secure = {
            let connections = self.connections.read().await;
            connections.get(client_id)?.secure.clone()
        };

        // Revoke first. This waits only for this session's admitted transport
        // handoff, not for sends or removals belonging to other peers.
        secure.revoke().await;
        let mut connections = self.connections.write().await;
        let current = connections.get(client_id)?;
        if !Arc::ptr_eq(&current.secure, &secure) {
            // A newer registration replaced this instance while revocation waited;
            // a stale offline event must not remove the replacement session.
            return None;
        }
        let conn = connections
            .remove(client_id)
            .expect("peer cannot disappear while holding write lock");
        self.receiver_index.write().await.remove_client(client_id);
        let primary_ip = conn.peer_info.virtual_ips.primary_ip();
        {
            let mut ip_idx = self.ip_index.write().await;
            for ip in peer_ips(&conn.peer_info) {
                if ip_idx
                    .get(&ip)
                    .is_some_and(|mapped| mapped.as_ref() == client_id)
                {
                    ip_idx.remove(&ip);
                }
            }
        }
        tracing::info!("已注销对等节点: {} ({:?})", client_id, primary_ip);
        primary_ip.or(Some(IpAddr::V6(conn.peer_info.virtual_ip)))
    }

    /// 将任一虚拟 IP（IPv4 或 IPv6）映射到已认证的客户端 ID。
    pub async fn client_id_for_ip(&self, ip: IpAddr) -> Option<String> {
        self.ip_index
            .read()
            .await
            .get(&ip)
            .map(|client_id| client_id.to_string())
    }

    /// Whether `ip` is a registered mesh destination. This preserves the
    /// outbound route semantics: the primary IPv6 must match `virtual_ip`, while
    /// IPv4 matches the dual-stack `virtual_ips.ipv4` field.
    pub(crate) async fn contains_peer_ip(&self, ip: IpAddr) -> bool {
        let client_id = {
            let idx = self.ip_index.read().await;
            idx.get(&ip).cloned()
        };
        let Some(client_id) = client_id else {
            return false;
        };
        let connections = self.connections.read().await;
        let Some(connection) = connections.get(client_id.as_ref()) else {
            return false;
        };
        match ip {
            IpAddr::V6(ipv6) => connection.peer_info.virtual_ip == ipv6,
            IpAddr::V4(ipv4) => connection.peer_info.virtual_ips.ipv4 == Some(ipv4),
        }
    }

    /// Resolve only the identity fields needed for inbound source validation.
    /// This avoids cloning the peer's public key and endpoint vectors per packet.
    pub(crate) async fn peer_route_by_ip(&self, ip: IpAddr) -> Option<PeerRouteInfo> {
        let client_id = {
            let idx = self.ip_index.read().await;
            idx.get(&ip).cloned()
        }?;
        self.connections
            .read()
            .await
            .get(client_id.as_ref())
            .map(|connection| PeerRouteInfo {
                client_id: client_id.clone(),
                virtual_ip: connection.peer_info.virtual_ip,
                virtual_ips: connection.peer_info.virtual_ips.clone(),
            })
    }

    /// Resolve the configured Exit Node without materializing every `PeerInfo`.
    /// IP-form targets use the existing O(1) virtual-IP index; Client ID targets
    /// use the connection map directly.
    pub(crate) async fn resolve_exit_peer(
        &self,
        configured_target: Option<&str>,
    ) -> Option<Ipv6Addr> {
        let target = configured_target?.trim();
        if target.is_empty() {
            return None;
        }

        if let Ok(ipv6) = target.parse::<Ipv6Addr>() {
            let client_id = self.ip_index.read().await.get(&IpAddr::V6(ipv6)).cloned()?;
            let connections = self.connections.read().await;
            let peer = connections.get(client_id.as_ref())?;
            return (peer.peer_info.virtual_ip == ipv6).then_some(ipv6);
        }
        if let Ok(ipv4) = target.parse::<Ipv4Addr>() {
            let client_id = self.ip_index.read().await.get(&IpAddr::V4(ipv4)).cloned()?;
            let connections = self.connections.read().await;
            let peer = connections.get(client_id.as_ref())?;
            return (peer.peer_info.virtual_ips.ipv4 == Some(ipv4))
                .then_some(peer.peer_info.virtual_ip);
        }

        self.connections
            .read()
            .await
            .get(target)
            .map(|connection| connection.peer_info.virtual_ip)
    }

    /// Resolve a registered peer by either its IPv4 or IPv6 virtual address.
    pub async fn peer_by_ip(&self, ip: IpAddr) -> Option<PeerInfo> {
        let client_id = {
            let idx = self.ip_index.read().await;
            idx.get(&ip).cloned()
        }?;
        self.connections
            .read()
            .await
            .get(client_id.as_ref())
            .map(|connection| connection.peer_info.clone())
    }

    /// 获取所有已连接的对等节点
    pub async fn get_peers(&self) -> Vec<PeerInfo> {
        self.connections
            .read()
            .await
            .values()
            .map(|c| c.peer_info.clone())
            .collect()
    }

    pub async fn has_active_peer(&self, peer_ip: &Ipv6Addr) -> bool {
        self.has_active_peer_by_ip(IpAddr::V6(*peer_ip)).await
    }

    pub async fn has_active_peer_by_ip(&self, ip: IpAddr) -> bool {
        let client_id = {
            let idx = self.ip_index.read().await;
            idx.get(&ip).cloned()
        };
        if let Some(id) = client_id {
            self.connections
                .read()
                .await
                .get(id.as_ref())
                .and_then(|conn| conn.active_endpoint)
                .is_some()
        } else {
            false
        }
    }

    /// Return the authenticated endpoint currently selected for a peer.
    ///
    /// Benchmarks use this as positive evidence that traffic is not being
    /// attributed to a relay or signaling fallback merely because the virtual
    /// tunnel is reachable.
    pub async fn active_endpoint_by_ip(&self, ip: IpAddr) -> Option<SocketAddr> {
        let client_id = {
            let idx = self.ip_index.read().await;
            idx.get(&ip).cloned()
        }?;
        self.connections
            .read()
            .await
            .get(client_id.as_ref())
            .and_then(|connection| connection.active_endpoint)
    }

    /// Aggregate per-peer secure-state lock counters without serializing the
    /// data plane. Each counter is atomic and each session owns its own mutex.
    pub(crate) async fn secure_lock_metrics(&self) -> (SecureLockMetrics, u64) {
        let connections = self.connections.read().await;
        let mut aggregate = SecureLockMetrics::default();
        for connection in connections.values() {
            aggregate.merge(connection.secure.lock_metrics());
        }
        (
            aggregate,
            u64::try_from(connections.len()).expect("peer count fits u64"),
        )
    }

    /// Whether the peer has completed Secure Session v2 key confirmation.
    pub async fn has_established_session_by_ip(&self, ip: IpAddr) -> bool {
        let client_id = {
            let idx = self.ip_index.read().await;
            idx.get(&ip).cloned()
        };
        let Some(client_id) = client_id else {
            return false;
        };
        let secure = {
            let connections = self.connections.read().await;
            connections
                .get(client_id.as_ref())
                .map(|connection| connection.secure.clone())
        };
        match secure {
            Some(secure) => secure.is_established().await,
            None => false,
        }
    }

    pub async fn peer_health(&self, peer_ip: &Ipv6Addr) -> Option<ConnectionHealthSnapshot> {
        self.peer_health_by_ip(IpAddr::V6(*peer_ip)).await
    }

    pub async fn peer_health_by_ip(&self, ip: IpAddr) -> Option<ConnectionHealthSnapshot> {
        let client_id = {
            let idx = self.ip_index.read().await;
            idx.get(&ip).cloned()
        };
        if let Some(id) = client_id {
            let threshold = self.fallback_threshold.read().await.clone();
            let health = {
                let connections = self.connections.read().await;
                connections.get(id.as_ref()).map(|conn| conn.health.clone())
            }?;
            Some(health.snapshot(now_micros(), PROBE_RESPONSE_GRACE, &threshold))
        } else {
            None
        }
    }

    /// 立即向指定节点发送 P2P 探针并等待真实 Pong。
    ///
    /// `target` 可以是 Client ID、虚拟 IPv6 或虚拟 IPv4。找不到目标时返回
    /// `None`；目标存在但未在超时内回复时返回 `timed_out = true`。
    pub async fn probe_peer_now(
        &self,
        target: &str,
        timeout: Duration,
    ) -> Option<PeerProbeOutcome> {
        let target = target.trim();
        if target.is_empty() {
            return None;
        }

        let client_id = if let Ok(ip) = target.parse::<IpAddr>() {
            self.ip_index.read().await.get(&ip).cloned()
        } else {
            let connections = self.connections.read().await;
            connections
                .contains_key(target)
                .then(|| Arc::<str>::from(target))
        }?;

        let src_ip = (*self.local_ip.read().await)?;
        let threshold = self.fallback_threshold.read().await.clone();
        let (peer_info, endpoints, active_endpoint, health) = {
            let connections = self.connections.read().await;
            let conn = connections.get(client_id.as_ref())?;
            (
                conn.peer_info.clone(),
                conn.endpoints.clone(),
                conn.active_endpoint,
                conn.health.clone(),
            )
        };
        let before_acked = health
            .snapshot(now_micros(), PROBE_RESPONSE_GRACE, &threshold)
            .probes_acked;

        let ping = P2pDatagram::ping(src_ip);
        let timestamp = match ping {
            P2pDatagram::Ping { timestamp, .. } => timestamp,
            _ => return None,
        };
        let data = codec::encode(&ping).ok()?;

        record_peer_probe_sent(&self.connections, client_id.as_ref(), timestamp).await;
        for endpoint in &endpoints {
            let _ = self
                .socket
                .send_to(&data, dual_stack_send_endpoint(*endpoint))
                .await;
        }

        let deadline = Instant::now() + timeout;
        loop {
            let health = self.peer_health(&peer_info.virtual_ip).await?;
            if health.probes_acked > before_acked {
                let connection = self.connections.read().await;
                let active_endpoint = connection
                    .get(client_id.as_ref())
                    .and_then(|conn| conn.active_endpoint)
                    .or(active_endpoint);
                return Some(PeerProbeOutcome {
                    client_id: client_id.to_string(),
                    virtual_ip: peer_info.virtual_ip,
                    endpoint: active_endpoint.or_else(|| endpoints.first().copied()),
                    rtt_us: health.rtt_us,
                    p2p_active: active_endpoint.is_some(),
                    timed_out: false,
                });
            }

            if Instant::now() >= deadline {
                return Some(PeerProbeOutcome {
                    client_id: client_id.to_string(),
                    virtual_ip: peer_info.virtual_ip,
                    endpoint: active_endpoint.or_else(|| endpoints.first().copied()),
                    rtt_us: None,
                    p2p_active: active_endpoint.is_some(),
                    timed_out: true,
                });
            }
            sleep(Duration::from_millis(25)).await;
        }
    }

    fn start_endpoint_probing(&self, client_id: String, _peer_primary_ip: IpAddr) {
        let socket = self.socket.clone();
        let connections = self.connections.clone();
        let local_ip = self.local_ip.clone();

        tokio::spawn(async move {
            let mut attempts = 0usize;
            loop {
                let Some(src_ip) = *local_ip.read().await else {
                    sleep(FAST_PROBE_INTERVAL).await;
                    continue;
                };

                let (endpoints, ping) = {
                    let connections = connections.read().await;
                    let Some(conn) = connections.get(&client_id) else {
                        return;
                    };
                    (conn.endpoints.clone(), P2pDatagram::ping(src_ip))
                };

                let timestamp = match &ping {
                    P2pDatagram::Ping { timestamp, .. } => *timestamp,
                    _ => return,
                };

                record_peer_probe_sent(&connections, &client_id, timestamp).await;

                let Ok(data) = codec::encode(&ping) else {
                    return;
                };

                for endpoint in endpoints {
                    let _ = socket
                        .send_to(&data, dual_stack_send_endpoint(endpoint))
                        .await;
                }

                attempts += 1;
                let interval = if attempts < FAST_PROBE_ATTEMPTS {
                    FAST_PROBE_INTERVAL
                } else {
                    HEALTH_PROBE_INTERVAL
                };
                sleep(interval).await;
            }
        });
    }
}

fn spawn_secure_initiator(
    socket: Arc<UdpSocket>,
    connections: Arc<RwLock<HashMap<String, PeerConnection>>>,
    client_id: String,
    secure: Arc<SecurePeerState>,
) {
    tokio::spawn(async move {
        loop {
            let endpoints = {
                let connections = connections.read().await;
                let Some(connection) = connections.get(&client_id) else {
                    return;
                };
                if !Arc::ptr_eq(&connection.secure, &secure) {
                    return;
                }
                connection.endpoints.clone()
            };

            let outbound = match secure.acquire_session_permission().await {
                Ok(permission) => permission,
                Err(super::secure_runtime::SecureRuntimeError::Revoked) => return,
                Err(error) => {
                    tracing::warn!("无法为节点 {} 获取会话发送许可: {error}", client_id);
                    sleep(SECURE_HANDSHAKE_RETRY_INTERVAL).await;
                    continue;
                }
            };
            match secure.initiate_frame().await {
                Ok(Some(frame)) => {
                    for endpoint in endpoints {
                        if let Err(error) = socket
                            .send_to(&frame, dual_stack_send_endpoint(endpoint))
                            .await
                        {
                            tracing::debug!(
                                "无法向节点 {} 的候选地址 {} 发送 Secure Session v2 HELLO: {}",
                                client_id,
                                endpoint,
                                error
                            );
                        }
                    }
                }
                Ok(None) => {}
                Err(super::secure_runtime::SecureRuntimeError::Revoked) => return,
                Err(error) => {
                    tracing::warn!("无法为节点 {} 启动 Secure Session v2: {error}", client_id);
                }
            }

            match secure.retry_staged_confirm().await {
                Ok(Some(action)) => {
                    tracing::debug!(
                        "重传 Secure Session v2 CONFIRM 到 {} (节点 {})",
                        action.destination,
                        client_id
                    );
                    if let Err(error) = socket
                        .send_to(&action.frame, dual_stack_send_endpoint(action.destination))
                        .await
                    {
                        tracing::debug!(
                            "无法向节点 {} 重传 Secure Session v2 CONFIRM: {}",
                            client_id,
                            error
                        );
                    }
                }
                Ok(None) => {}
                Err(super::secure_runtime::SecureRuntimeError::Revoked) => return,
                Err(error) => {
                    tracing::debug!(
                        "无法重传节点 {} 的 Secure Session v2 CONFIRM: {error}",
                        client_id
                    );
                }
            }
            drop(outbound);

            sleep(SECURE_HANDSHAKE_RETRY_INTERVAL).await;
        }
    });
}

/// A dual-stack IPv6 UDP socket requires IPv4 destinations in IPv4-mapped
/// IPv6 form. Passing a plain `SocketAddrV4` fails on Linux with
/// `ENETUNREACH`, including inside an otherwise correctly routed namespace.
fn dual_stack_send_endpoint(endpoint: SocketAddr) -> SocketAddr {
    match endpoint {
        SocketAddr::V4(addr) => SocketAddr::V6(SocketAddrV6::new(
            Ipv4Addr::from(addr.ip().octets()).to_ipv6_mapped(),
            addr.port(),
            0,
            0,
        )),
        SocketAddr::V6(_) => endpoint,
    }
}

fn canonical_udp_endpoint(endpoint: SocketAddr) -> SocketAddr {
    match endpoint {
        SocketAddr::V6(addr) => match addr.ip().to_ipv4_mapped() {
            Some(ipv4) => SocketAddr::new(IpAddr::V4(ipv4), addr.port()),
            None => endpoint,
        },
        SocketAddr::V4(_) => endpoint,
    }
}

fn peer_ips(peer: &PeerInfo) -> Vec<IpAddr> {
    let mut ips = vec![IpAddr::V6(peer.virtual_ip)];
    if let Some(v6) = peer.virtual_ips.ipv6 {
        let ip = IpAddr::V6(v6);
        if !ips.contains(&ip) {
            ips.push(ip);
        }
    }
    if let Some(v4) = peer.virtual_ips.ipv4 {
        let ip = IpAddr::V4(v4);
        if !ips.contains(&ip) {
            ips.push(ip);
        }
    }
    ips
}

async fn peer_accepts_endpoint(
    connections: &Arc<RwLock<HashMap<String, PeerConnection>>>,
    client_id: &str,
    endpoint: SocketAddr,
) -> bool {
    let endpoint = canonical_udp_endpoint(endpoint);
    let connections = connections.read().await;
    connections
        .get(client_id)
        .map(|connection| {
            connection
                .endpoints
                .iter()
                .copied()
                .map(canonical_udp_endpoint)
                .any(|candidate| candidate == endpoint)
        })
        .unwrap_or(false)
}

/// Accept the first probe from a different source port on an already-advertised
/// peer IP. Symmetric NAT commonly assigns a destination-specific mapping, so
/// the port observed by the peer is the only port that can complete UDP hole
/// punching. Exact endpoints remain accepted after the active path is pinned;
/// arbitrary same-IP ports do not.
async fn peer_accepts_probe_source(
    connections: &Arc<RwLock<HashMap<String, PeerConnection>>>,
    client_id: &str,
    endpoint: SocketAddr,
) -> bool {
    let endpoint = canonical_udp_endpoint(endpoint);
    let connections = connections.read().await;
    connections.get(client_id).is_some_and(|connection| {
        connection
            .endpoints
            .iter()
            .copied()
            .map(canonical_udp_endpoint)
            .any(|candidate| candidate == endpoint)
            || (connection.active_endpoint.is_none()
                && connection
                    .endpoints
                    .iter()
                    .any(|candidate| candidate.ip() == endpoint.ip()))
    })
}

#[allow(clippy::too_many_arguments)]
fn spawn_udp_receiver(
    socket: Arc<UdpSocket>,
    connections: Arc<RwLock<HashMap<String, PeerConnection>>>,
    ip_index: Arc<RwLock<HashMap<IpAddr, Arc<str>>>>,
    receiver_index: Arc<RwLock<ReceiverIndex>>,
    buffer_pool: Arc<UdpBufferPool>,
    udp_batch_size: usize,
    udp_batch_stats: Arc<UdpBatchStats>,
    local_ip: Arc<RwLock<Option<IpAddr>>>,
    overlay: Arc<super::overlay::P2pOverlay>,
    incoming_tx: InboundQueueSender,
) {
    tokio::spawn(async move {
        let mut receive_scratch = udp_batch::UdpReceiveScratch::default();
        loop {
            let mut payloads: Vec<_> = (0..udp_batch_size).map(|_| buffer_pool.acquire()).collect();
            match udp_batch::receive_batch(&socket, &mut payloads, &mut receive_scratch).await {
                Ok(received) if !received.is_empty() => {
                    udp_batch_stats.record_recv(received.len());
                    let mut datagrams = Vec::with_capacity(received.len());
                    for ((size, addr), mut buffer) in received.into_iter().zip(payloads) {
                        buffer.truncate(size);
                        datagrams.push((addr, buffer));
                    }
                    dispatch_udp_batch(
                        &socket,
                        &connections,
                        &ip_index,
                        &receiver_index,
                        &local_ip,
                        &overlay,
                        &incoming_tx,
                        datagrams,
                    )
                    .await;
                }
                Ok(_) => {
                    // A zero-message result is not expected from the Linux
                    // helper, but must not turn into a busy loop on fallback.
                    tokio::task::yield_now().await;
                }
                Err(e) => {
                    tracing::error!("UDP 接收数据异常: {:?}", e);
                    break;
                }
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_udp_batch(
    socket: &UdpSocket,
    connections: &Arc<RwLock<HashMap<String, PeerConnection>>>,
    ip_index: &Arc<RwLock<HashMap<IpAddr, Arc<str>>>>,
    receiver_index: &Arc<RwLock<ReceiverIndex>>,
    local_ip: &Arc<RwLock<Option<IpAddr>>>,
    overlay: &Arc<super::overlay::P2pOverlay>,
    incoming_tx: &InboundQueueSender,
    datagrams: Vec<(SocketAddr, InboundPayload)>,
) {
    let mut datagrams = datagrams.into_iter().map(Some).collect::<Vec<_>>();
    let mut index = 0;
    while index < datagrams.len() {
        let Some((addr, bytes)) = datagrams[index].as_ref() else {
            index += 1;
            continue;
        };
        let Some(header) = is_data_packet(bytes.as_slice())
            .then(|| DataHeader::decode(bytes.as_slice()).ok())
            .flatten()
        else {
            let (addr, bytes) = datagrams[index]
                .take()
                .expect("current datagram is present");
            dispatch_udp_packet(
                socket,
                connections,
                ip_index,
                receiver_index,
                local_ip,
                overlay,
                incoming_tx,
                addr,
                bytes,
            )
            .await;
            index += 1;
            continue;
        };
        let addr = *addr;

        // Collisions and unknown indexes keep the conservative single-packet
        // path: every candidate is still authenticated before delivery.
        let route = {
            let index_table = receiver_index.read().await;
            match index_table.candidate_snapshot(header.receiver_index, addr) {
                Some((route, rest)) if rest.is_empty() => Some(route),
                _ => None,
            }
        };
        let Some(route) = route else {
            let (addr, bytes) = datagrams[index]
                .take()
                .expect("current datagram is present");
            dispatch_udp_packet(
                socket,
                connections,
                ip_index,
                receiver_index,
                local_ip,
                overlay,
                incoming_tx,
                addr,
                bytes,
            )
            .await;
            index += 1;
            continue;
        };

        // recvmmsg preserves kernel arrival order. Batch only a contiguous run
        // from one source and generation so delivery order is unchanged and no
        // packet can be moved past another datagram from the same flow.
        let mut end = index + 1;
        while end < datagrams.len() {
            let Some((next_addr, next_bytes)) = datagrams[end].as_ref() else {
                break;
            };
            if *next_addr != addr || !is_data_packet(next_bytes.as_slice()) {
                break;
            }
            let Ok(next_header) = DataHeader::decode(next_bytes.as_slice()) else {
                break;
            };
            if next_header.receiver_index != header.receiver_index
                || next_header.epoch != header.epoch
            {
                break;
            }
            end += 1;
        }

        let secure = route.secure.clone();
        let _session = match secure.acquire_session_permission().await {
            Ok(permission) => permission,
            Err(super::secure_runtime::SecureRuntimeError::Revoked) => {
                index = end;
                continue;
            }
            Err(error) => {
                tracing::debug!(
                    "无法为节点 {} 获取入站批处理许可: {error}",
                    route.peer.client_id
                );
                index = end;
                continue;
            }
        };
        let opened = {
            let mut packets = datagrams[index..end]
                .iter_mut()
                .map(|slot| {
                    slot.as_mut()
                        .expect("batch slots are present")
                        .1
                        .as_mut_slice()
                })
                .collect::<Vec<_>>();
            secure
                .open_from_source_batch_per_packet_in_place(addr, &mut packets)
                .await
        };
        let results = match opened {
            Ok(results) => results,
            Err(error) => {
                tracing::debug!(
                    "节点 {} 未通过来自 {addr} 的 Secure Session v2 数据批认证，整组丢弃: {error}",
                    route.peer.client_id
                );
                index = end;
                continue;
            }
        };
        debug_assert_eq!(results.len(), end - index);

        // Endpoint migration is valid after any packet in the batch has been
        // authenticated, but it must be revalidated against the exact session
        // before publishing any plaintext. A mixed batch keeps valid packets
        // and rejects only the slots that failed authentication.
        if !results.iter().any(Result::is_ok) {
            index = end;
            continue;
        }
        if !update_authenticated_peer_endpoint(
            connections,
            route.peer.client_id.as_ref(),
            &secure,
            addr,
        )
        .await
        {
            index = end;
            continue;
        }
        let endpoint_registered =
            receiver_index
                .read()
                .await
                .has_endpoint(route.peer.client_id.as_ref(), &secure, addr);
        if !endpoint_registered
            && !receiver_index.write().await.register_endpoint(
                route.peer.client_id.as_ref(),
                &secure,
                addr,
            )
        {
            index = end;
            continue;
        }

        for (offset, result) in results.into_iter().enumerate() {
            let (_, mut payload) = datagrams[index + offset]
                .take()
                .expect("opened batch slot is present");
            match result {
                Ok(plaintext_len) => {
                    payload.truncate(plaintext_len);
                    let payload_len = payload.len();
                    if incoming_tx.try_send(InboundPacket::AuthenticatedPlaintext {
                        src_ip: route.src_ip,
                        peer: route.peer.clone(),
                        payload,
                    }) {
                        // A full queue is not successful inbound traffic.
                        route.health.record_payload(payload_len);
                    }
                }
                Err(error) => {
                    tracing::debug!(
                        "节点 {} 的批内数据包未通过 Secure Session v2 认证，仅丢弃该包: {error}",
                        route.peer.client_id
                    );
                }
            }
        }
        index = end;
    }
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_udp_packet(
    socket: &UdpSocket,
    connections: &Arc<RwLock<HashMap<String, PeerConnection>>>,
    ip_index: &Arc<RwLock<HashMap<IpAddr, Arc<str>>>>,
    receiver_index: &Arc<RwLock<ReceiverIndex>>,
    local_ip: &Arc<RwLock<Option<IpAddr>>>,
    overlay: &Arc<super::overlay::P2pOverlay>,
    incoming_tx: &InboundQueueSender,
    addr: SocketAddr,
    bytes: InboundPayload,
) {
    if bytes.starts_with(&CONTROL_MAGIC) {
        match decode_control(bytes.as_slice()) {
            Ok(message) => {
                dispatch_secure_control(socket, connections, incoming_tx, addr, message).await;
            }
            Err(error) => {
                tracing::debug!("丢弃来自 {addr} 的非法 Secure Session v2 控制帧: {error}");
            }
        }
        return;
    }

    if is_data_packet(bytes.as_slice()) {
        dispatch_secure_data(connections, receiver_index, incoming_tx, addr, bytes).await;
        return;
    }

    if let Ok(datagram) = codec::decode::<P2pDatagram>(bytes.as_slice()) {
        dispatch_p2p_datagram(
            socket,
            connections,
            ip_index,
            local_ip,
            overlay,
            incoming_tx,
            addr,
            datagram,
        )
        .await;
    } else if let Ok(packet) = codec::decode::<DataPacket>(bytes.as_slice()) {
        dispatch_legacy_data_packet(connections, ip_index, incoming_tx, addr, packet).await;
    } else {
        tracing::warn!("无法反序列化来自 {} 的数据包", addr);
    }
}

async fn dispatch_secure_control(
    socket: &UdpSocket,
    connections: &Arc<RwLock<HashMap<String, PeerConnection>>>,
    incoming_tx: &InboundQueueSender,
    addr: SocketAddr,
    message: fuckipv6_secure_session::wire::ControlMessage,
) {
    let candidates = {
        let connections = connections.read().await;
        connections
            .iter()
            .map(|(client_id, connection)| (client_id.clone(), connection.secure.clone()))
            .collect::<Vec<_>>()
    };

    let mut matches = Vec::new();
    for (client_id, secure) in candidates {
        if secure.matches_control(&message).await {
            matches.push((client_id, secure));
        }
    }
    if matches.len() != 1 {
        tracing::debug!(
            "拒绝来自 {addr} 的 Secure Session v2 控制帧: 匹配到 {} 个对等会话",
            matches.len()
        );
        return;
    }
    let (client_id, secure) = matches.pop().expect("one matching peer");
    let endpoint_authenticated = matches!(
        &message,
        fuckipv6_secure_session::wire::ControlMessage::Accept(_)
            | fuckipv6_secure_session::wire::ControlMessage::Confirm(_)
            | fuckipv6_secure_session::wire::ControlMessage::HelloWithCookie(_)
    );
    let _session = match secure.acquire_session_permission().await {
        Ok(permission) => permission,
        Err(super::secure_runtime::SecureRuntimeError::Revoked) => return,
        Err(error) => {
            tracing::debug!("无法为节点 {client_id} 获取控制帧发送许可: {error}");
            return;
        }
    };

    match secure.handle_control(addr, message).await {
        Ok(actions) => {
            incoming_tx.record_control_frame_bypass();
            if endpoint_authenticated {
                update_peer_endpoint(connections, &client_id, addr).await;
                record_peer_inbound(connections, &client_id).await;
            }
            for action in actions {
                tracing::debug!(
                    "发送 Secure Session v2 {} 到 {} (节点 {})",
                    action.kind,
                    action.destination,
                    client_id
                );
                if let Err(error) = socket
                    .send_to(&action.frame, dual_stack_send_endpoint(action.destination))
                    .await
                {
                    tracing::warn!(
                        "无法向节点 {} 发送 Secure Session v2 {}: {}",
                        client_id,
                        action.kind,
                        error
                    );
                }
            }
            if secure.is_established().await {
                tracing::info!("Secure Session v2 已与节点 {} 建立", client_id);
            }
        }
        Err(error) => {
            tracing::debug!("拒绝来自 {addr} 的 Secure Session v2 控制帧: {error}");
        }
    }
}

async fn dispatch_secure_data(
    connections: &Arc<RwLock<HashMap<String, PeerConnection>>>,
    receiver_index: &Arc<RwLock<ReceiverIndex>>,
    incoming_tx: &InboundQueueSender,
    addr: SocketAddr,
    bytes: InboundPayload,
) {
    let header = match DataHeader::decode(bytes.as_slice()) {
        Ok(header) => header,
        Err(error) => {
            tracing::debug!("丢弃来自 {addr} 的非法 Secure Session v2 数据帧头: {error}");
            return;
        }
    };

    // Snapshot only the colliding candidate routes, never the full connection
    // map. The normal single-candidate case does not allocate. The guard is
    // dropped before waiting on a session or performing AES-GCM.
    let Some((first, rest)) = receiver_index
        .read()
        .await
        .candidate_snapshot(header.receiver_index, addr)
    else {
        tracing::debug!("丢弃来自 {addr} 的、无匹配 receiver index 的 Secure Session v2 数据帧");
        return;
    };

    let candidate_count = 1 + rest.len();
    let mut packet_owner = Some(bytes);
    for (candidate_index, route) in std::iter::once(first).chain(rest).enumerate() {
        let client_id = &route.peer.client_id;
        let secure = &route.secure;
        let _session = match secure.acquire_session_permission().await {
            Ok(permission) => permission,
            Err(super::secure_runtime::SecureRuntimeError::Revoked) => continue,
            Err(error) => {
                tracing::debug!("无法为节点 {client_id} 获取入站数据处理许可: {error}");
                continue;
            }
        };
        // Receiver indexes are collision-safe. An earlier candidate can consume
        // or mutate its input, so only the final candidate may use the pooled
        // buffer directly; the extremely rare collision path gets a fresh copy.
        let mut candidate_packet = if candidate_index + 1 < candidate_count {
            let packet = packet_owner
                .as_ref()
                .expect("collision candidates retain the source packet");
            InboundPayload::from(packet.as_slice().to_vec())
        } else {
            packet_owner
                .take()
                .expect("final collision candidate owns the source packet")
        };
        let opened = secure
            .open_from_source_in_place(addr, candidate_packet.as_mut_slice())
            .await
            .map(|plaintext_len| {
                candidate_packet.truncate(plaintext_len);
                candidate_packet
            });
        match opened {
            Ok(plaintext) => {
                // Authentication happens before endpoint migration. A spoofed,
                // replayed, or tampered datagram must not steer future traffic.
                // Revalidate the exact session before delivering the plaintext.
                if !update_authenticated_peer_endpoint(
                    connections,
                    client_id.as_ref(),
                    secure,
                    addr,
                )
                .await
                {
                    return;
                }
                let endpoint_registered =
                    receiver_index
                        .read()
                        .await
                        .has_endpoint(client_id.as_ref(), secure, addr);
                if !endpoint_registered
                    && !receiver_index.write().await.register_endpoint(
                        client_id.as_ref(),
                        secure,
                        addr,
                    )
                {
                    return;
                }
                let payload_len = plaintext.len();
                if incoming_tx.try_send(InboundPacket::AuthenticatedPlaintext {
                    src_ip: route.src_ip,
                    peer: route.peer.clone(),
                    payload: plaintext,
                }) {
                    route.health.record_payload(payload_len);
                }
                return;
            }
            Err(super::secure_runtime::SecureRuntimeError::NotEstablished) => continue,
            Err(error) => {
                tracing::debug!(
                    "节点 {client_id} 未通过来自 {addr} 的 Secure Session v2 数据帧认证: {error}"
                );
            }
        }
    }
}

async fn dispatch_legacy_data_packet(
    _connections: &Arc<RwLock<HashMap<String, PeerConnection>>>,
    _ip_index: &Arc<RwLock<HashMap<IpAddr, Arc<str>>>>,
    _incoming_tx: &InboundQueueSender,
    addr: SocketAddr,
    _packet: DataPacket,
) {
    tracing::warn!("丢弃来自 {addr} 的未认证旧版数据包");
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_p2p_datagram(
    socket: &UdpSocket,
    connections: &Arc<RwLock<HashMap<String, PeerConnection>>>,
    ip_index: &Arc<RwLock<HashMap<IpAddr, Arc<str>>>>,
    local_ip: &Arc<RwLock<Option<IpAddr>>>,
    overlay: &Arc<super::overlay::P2pOverlay>,
    incoming_tx: &InboundQueueSender,
    addr: SocketAddr,
    datagram: P2pDatagram,
) {
    match datagram {
        P2pDatagram::Data(_) => {
            // Legacy data carried static-X25519 ciphertext without the v2
            // transcript confirmation and replay window. Never feed it into the
            // production receive queue, even if it decodes successfully.
            tracing::debug!("丢弃来自 {addr} 的旧版 P2P 数据包");
        }
        P2pDatagram::Control { src_ip, message } => {
            let client_id = {
                let idx = ip_index.read().await;
                idx.get(&src_ip).cloned()
            };
            if let Some(ref id) = client_id {
                if peer_accepts_endpoint(connections, id.as_ref(), addr).await {
                    update_peer_endpoint(connections, id.as_ref(), addr).await;
                    handle_control_message(connections, id.as_ref(), message).await;
                }
            }
        }
        P2pDatagram::Ping { src_ip, timestamp } => {
            let client_id = {
                let idx = ip_index.read().await;
                idx.get(&src_ip).cloned()
            };
            if let Some(ref id) = client_id {
                if peer_accepts_probe_source(connections, id.as_ref(), addr).await {
                    handle_ping_datagram(
                        socket,
                        connections,
                        local_ip,
                        addr,
                        id.as_ref(),
                        src_ip,
                        timestamp,
                    )
                    .await;
                }
            }
        }
        P2pDatagram::Pong { src_ip, timestamp } => {
            let client_id = {
                let idx = ip_index.read().await;
                idx.get(&src_ip).cloned()
            };
            if let Some(ref id) = client_id {
                if peer_accepts_probe_source(connections, id.as_ref(), addr).await {
                    handle_pong_datagram(
                        socket,
                        connections,
                        local_ip,
                        addr,
                        id.as_ref(),
                        src_ip,
                        timestamp,
                    )
                    .await;
                }
            }
        }
        P2pDatagram::Onion(cell) => {
            overlay
                .handle_onion_cell(cell, addr, socket, incoming_tx)
                .await;
        }
        P2pDatagram::Dht(msg) => {
            overlay.handle_dht_message(msg, addr, socket).await;
        }
        P2pDatagram::Transfer(msg) => {
            overlay.handle_transfer_message(msg, addr, socket).await;
        }
        P2pDatagram::DirectMessage(msg) => {
            let _ = overlay.handle_direct_message(msg).await;
        }
    }
}

async fn handle_ping_datagram(
    socket: &UdpSocket,
    connections: &Arc<RwLock<HashMap<String, PeerConnection>>>,
    local_ip: &Arc<RwLock<Option<IpAddr>>>,
    addr: SocketAddr,
    client_id: &str,
    _src_ip: IpAddr,
    timestamp: u64,
) {
    update_peer_endpoint(connections, client_id, addr).await;
    record_peer_inbound(connections, client_id).await;

    if let Some(local_ip) = *local_ip.read().await {
        let pong = P2pDatagram::pong(local_ip, timestamp);
        if let Ok(data) = codec::encode(&pong) {
            let _ = socket.send_to(&data, dual_stack_send_endpoint(addr)).await;
        }
    }
}

async fn handle_pong_datagram(
    socket: &UdpSocket,
    connections: &Arc<RwLock<HashMap<String, PeerConnection>>>,
    local_ip: &Arc<RwLock<Option<IpAddr>>>,
    addr: SocketAddr,
    client_id: &str,
    src_ip: IpAddr,
    timestamp: u64,
) {
    update_peer_endpoint(connections, client_id, addr).await;
    let rtt_us = DataPacket {
        src_ip,
        dst_ip: src_ip,
        payload: Vec::new(),
        timestamp,
    }
    .latency();
    record_peer_probe_response(connections, client_id, timestamp, rtt_us).await;
    send_quality_report(socket, connections, local_ip, client_id, addr).await;
    tracing::debug!(
        "P2P pong from {} ({}) via {} RTT={}us",
        client_id,
        src_ip,
        addr,
        rtt_us
    );
}

fn bind_dual_stack_udp(port: u16) -> Result<UdpSocket> {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV6,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )
    .context("无法创建双栈 UDP socket")?;
    socket.set_only_v6(false).context("无法启用 UDP 双栈模式")?;
    socket
        .set_nonblocking(true)
        .context("无法设置 UDP socket 为非阻塞")?;

    let addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port);
    socket
        .bind(&addr.into())
        .with_context(|| format!("无法绑定 UDP socket: {addr}"))?;

    let std_socket: std::net::UdpSocket = socket.into();
    UdpSocket::from_std(std_socket).context("无法创建 Tokio UDP socket")
}

async fn update_peer_endpoint(
    connections: &Arc<RwLock<HashMap<String, PeerConnection>>>,
    client_id: &str,
    endpoint: SocketAddr,
) {
    let endpoint = canonical_udp_endpoint(endpoint);
    let needs_update = {
        let connections = connections.read().await;
        let Some(conn) = connections.get(client_id) else {
            return;
        };
        !conn.endpoints.contains(&endpoint) || conn.active_endpoint != Some(endpoint)
    };
    if !needs_update {
        return;
    }

    let mut connections = connections.write().await;
    let Some(conn) = connections.get_mut(client_id) else {
        return;
    };

    if !conn.endpoints.contains(&endpoint) {
        conn.endpoints.push(endpoint);
    }

    if conn.active_endpoint != Some(endpoint) {
        tracing::info!("P2P 通信端点已更新: {} 经由 {}", client_id, endpoint);
        conn.active_endpoint = Some(endpoint);
    }
}

async fn update_authenticated_peer_endpoint(
    connections: &Arc<RwLock<HashMap<String, PeerConnection>>>,
    client_id: &str,
    secure: &Arc<SecurePeerState>,
    endpoint: SocketAddr,
) -> bool {
    let endpoint = canonical_udp_endpoint(endpoint);
    let (exists, needs_update) = {
        let connections = connections.read().await;
        let Some(conn) = connections.get(client_id) else {
            return false;
        };
        if !Arc::ptr_eq(&conn.secure, secure) {
            return false;
        }
        (
            true,
            !conn.endpoints.contains(&endpoint) || conn.active_endpoint != Some(endpoint),
        )
    };
    if !exists || !needs_update {
        return exists;
    }

    let mut connections = connections.write().await;
    let Some(conn) = connections.get_mut(client_id) else {
        return false;
    };
    if !Arc::ptr_eq(&conn.secure, secure) {
        return false;
    }
    if !conn.endpoints.contains(&endpoint) {
        conn.endpoints.push(endpoint);
    }
    if conn.active_endpoint != Some(endpoint) {
        tracing::info!("P2P 通信端点已更新: {} 经由 {}", client_id, endpoint);
        conn.active_endpoint = Some(endpoint);
    }
    true
}

async fn peer_health_state(
    connections: &Arc<RwLock<HashMap<String, PeerConnection>>>,
    client_id: &str,
) -> Option<Arc<PeerHealth>> {
    let connections = connections.read().await;
    connections.get(client_id).map(|conn| conn.health.clone())
}

async fn record_peer_inbound(
    connections: &Arc<RwLock<HashMap<String, PeerConnection>>>,
    client_id: &str,
) {
    if let Some(health) = peer_health_state(connections, client_id).await {
        health.record_inbound();
    }
}

async fn record_peer_probe_sent(
    connections: &Arc<RwLock<HashMap<String, PeerConnection>>>,
    client_id: &str,
    timestamp: u64,
) {
    if let Some(health) = peer_health_state(connections, client_id).await {
        health.record_probe_sent(timestamp);
    }
}

async fn record_peer_probe_response(
    connections: &Arc<RwLock<HashMap<String, PeerConnection>>>,
    client_id: &str,
    timestamp: u64,
    rtt_us: u64,
) {
    if let Some(health) = peer_health_state(connections, client_id).await {
        health.record_probe_response(timestamp, rtt_us);
    }
}

async fn handle_control_message(
    connections: &Arc<RwLock<HashMap<String, PeerConnection>>>,
    client_id: &str,
    message: ControlMessage,
) {
    match message {
        ControlMessage::QualityReport {
            rtt_us,
            loss_rate,
            bandwidth_bps,
        } => {
            if let Some(health) = peer_health_state(connections, client_id).await {
                health.record_remote_quality(rtt_us, loss_rate, bandwidth_bps);
            }
        }
        _ => {
            record_peer_inbound(connections, client_id).await;
        }
    }
}

async fn send_quality_report(
    socket: &UdpSocket,
    connections: &Arc<RwLock<HashMap<String, PeerConnection>>>,
    local_ip: &Arc<RwLock<Option<IpAddr>>>,
    client_id: &str,
    endpoint: SocketAddr,
) {
    let Some(src_ip) = *local_ip.read().await else {
        return;
    };

    let Some(health) = ({
        let connections = connections.read().await;
        connections.get(client_id).map(|conn| conn.health.clone())
    }) else {
        return;
    };
    let health = health.snapshot(
        now_micros(),
        PROBE_RESPONSE_GRACE,
        &FallbackThreshold::default(),
    );

    let report = P2pDatagram::quality_report(
        src_ip,
        health.rtt_us.unwrap_or(0),
        health.loss_rate,
        health.bandwidth_bps,
    );

    if let Ok(data) = codec::encode(&report) {
        let _ = socket
            .send_to(&data, dual_stack_send_endpoint(endpoint))
            .await;
    }
}

fn now_micros() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_micros().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route_info(client_id: &str, ipv6: Ipv6Addr) -> PeerRouteInfo {
        PeerRouteInfo {
            client_id: Arc::from(client_id),
            virtual_ip: ipv6,
            virtual_ips: VirtualIps::from_ipv6(ipv6),
        }
    }

    async fn established_direct_peers() -> Result<(
        P2pEngine,
        P2pEngine,
        Ipv6Addr,
        Ipv6Addr,
        SocketAddr,
        SocketAddr,
    )> {
        let engine_a = P2pEngine::new(0).await?;
        let engine_b = P2pEngine::new(0).await?;
        let identity_a = Identity::generate()?;
        let identity_b = Identity::generate()?;
        let ip_a = Ipv6Addr::LOCALHOST;
        let ip_b = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 2);

        engine_a.set_identity(identity_a.clone()).await;
        engine_b.set_identity(identity_b.clone()).await;
        engine_a.set_local_ip(ip_a).await;
        engine_b.set_local_ip(ip_b).await;

        let endpoint_a = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            engine_a.local_addr().port(),
        );
        let endpoint_b = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            engine_b.local_addr().port(),
        );
        engine_a
            .add_peer(PeerInfo::new(
                "b".to_string(),
                ip_b,
                identity_b.public_key(),
                vec![endpoint_b],
                fuckipv6_common::NatType::Open,
            ))
            .await?;
        engine_b
            .add_peer(PeerInfo::new(
                "a".to_string(),
                ip_a,
                identity_a.public_key(),
                vec![endpoint_a],
                fuckipv6_common::NatType::Open,
            ))
            .await?;

        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if engine_a
                    .has_established_session_by_ip(IpAddr::V6(ip_b))
                    .await
                    && engine_b
                        .has_established_session_by_ip(IpAddr::V6(ip_a))
                        .await
                {
                    return;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;

        Ok((engine_a, engine_b, ip_a, ip_b, endpoint_a, endpoint_b))
    }

    #[tokio::test]
    async fn short_sendmmsg_falls_back_only_unsent_suffix_without_resealing() -> Result<()> {
        let (engine_a, engine_b, ip_a, ip_b, _, _) = established_direct_peers().await?;
        let payloads = (0..4u8)
            .map(|marker| vec![marker; 1_420])
            .collect::<Vec<_>>();
        let encoded_len = payloads[0].len() + DATA_HEADER_LEN + AEAD_TAG_LEN;
        let mut buffers = payloads
            .iter()
            .map(|payload| {
                let mut buffer = vec![0u8; encoded_len];
                buffer[DATA_HEADER_LEN..DATA_HEADER_LEN + payload.len()].copy_from_slice(payload);
                buffer
            })
            .collect::<Vec<_>>();
        let mut refs = buffers
            .iter_mut()
            .zip(&payloads)
            .map(|(buffer, payload)| (buffer.as_mut_slice(), payload.len()))
            .collect::<Vec<_>>();
        let before_batch = engine_a.udp_batch_metrics();
        let before_seals = engine_a.direct_seal_count();
        engine_a.set_send_batch_short_return(2);

        let outcomes = engine_a
            .send_batch_to_or_fallback_in_place(IpAddr::V6(ip_b), std::mem::take(&mut refs))
            .await?;
        engine_a.set_send_batch_short_return(usize::MAX);

        assert_eq!(engine_a.direct_seal_count(), before_seals + 4);
        assert!(matches!(outcomes[0], Ok(InPlaceSendOutcome::Sent)));
        assert!(matches!(outcomes[1], Ok(InPlaceSendOutcome::Sent)));
        for outcome in &outcomes[2..] {
            assert!(matches!(
                outcome,
                Ok(InPlaceSendOutcome::SealedForFallback {
                    encoded_len: actual,
                    ..
                }) if *actual == encoded_len
            ));
        }
        let after_batch = engine_a.udp_batch_metrics();
        assert_eq!(after_batch.send_syscalls, before_batch.send_syscalls + 1);
        assert_eq!(after_batch.send_datagrams, before_batch.send_datagrams + 2);
        assert_eq!(
            after_batch.fallback_packets,
            before_batch.fallback_packets + 2
        );

        for (index, buffer) in buffers.iter().enumerate() {
            let header = DataHeader::decode(&buffer[..encoded_len])?;
            assert_eq!(header.counter, index as u64);
            assert_eq!(
                engine_b
                    .decrypt_from_peer(ip_a, &buffer[..encoded_len])
                    .await?,
                payloads[index],
                "sealed packet {index} must retain ascending nonce and payload order"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn receiver_index_endpoint_hit_preserves_all_colliding_candidates() -> Result<()> {
        let (_, engine, ip_a, _, endpoint_a, _) = established_direct_peers().await?;
        let base_route = {
            let index = engine.receiver_index.read().await;
            index
                .by_index
                .values()
                .next()
                .and_then(|routes| routes.first())
                .cloned()
                .expect("established peer route")
        };
        let receiver_index = base_route.secure.local_receiver_index();
        let endpoint_route = Arc::new(ReceiverRoute {
            peer: route_info("endpoint-route", ip_a),
            secure: base_route.secure.clone(),
            health: base_route.health.clone(),
            src_ip: IpAddr::V6(ip_a),
        });
        let index_route = Arc::new(ReceiverRoute {
            peer: route_info("colliding-index-route", ip_a),
            secure: base_route.secure.clone(),
            health: base_route.health.clone(),
            src_ip: IpAddr::V6(ip_a),
        });
        let mut index = ReceiverIndex::default();
        index.by_endpoint.insert(
            (receiver_index, canonical_udp_endpoint(endpoint_a)),
            vec![endpoint_route.clone()],
        );
        index.by_index.insert(
            receiver_index,
            vec![endpoint_route.clone(), index_route.clone()],
        );

        let (first, rest) = index
            .candidate_snapshot(receiver_index, endpoint_a)
            .expect("endpoint hint should retain index collision candidates");
        assert!(Arc::ptr_eq(&first, &endpoint_route));
        assert_eq!(rest.len(), 1);
        assert!(Arc::ptr_eq(&rest[0], &index_route));
        Ok(())
    }

    #[tokio::test]
    async fn udp_dispatch_batches_out_of_order_direct_plaintext() -> Result<()> {
        let (engine_a, engine_b, ip_a, ip_b, endpoint_a, _) = established_direct_peers().await?;
        let payloads = (0..8u8)
            .map(|marker| vec![marker; 1_420])
            .collect::<Vec<_>>();
        let mut packets = Vec::with_capacity(payloads.len());
        for payload in &payloads {
            packets.push(engine_a.encrypt_for_peer(ip_b, payload).await?);
        }
        packets.reverse();

        let (incoming_tx, mut incoming_rx, _) = inbound_queue::channel(32);
        engine_b
            .receiver_index
            .read()
            .await
            .route_lookups
            .store(0, Ordering::Relaxed);
        let before = fuckipv6_secure_session::crypto_batch_metrics();
        dispatch_udp_batch(
            &engine_b.socket,
            &engine_b.connections,
            &engine_b.ip_index,
            &engine_b.receiver_index,
            &engine_b.local_ip,
            &engine_b.overlay,
            &incoming_tx,
            packets
                .into_iter()
                .map(|packet| (endpoint_a, InboundPayload::from(packet)))
                .collect(),
        )
        .await;
        let after = fuckipv6_secure_session::crypto_batch_metrics();

        assert_eq!(
            engine_b
                .receiver_index
                .read()
                .await
                .route_lookups
                .load(Ordering::Relaxed),
            1,
            "a same-route UDP batch must resolve the receiver route once"
        );
        assert!(
            after.parallel_jobs > before.parallel_jobs,
            "eight 1420-byte packets must use the crypto worker batch"
        );
        assert!(
            after.parallel_packets >= before.parallel_packets + payloads.len() as u64,
            "all batch packets must be accounted as parallel AEAD work"
        );
        for expected in payloads.iter().rev() {
            let received = tokio::time::timeout(Duration::from_secs(1), incoming_rx.recv())
                .await?
                .context("batched packet was not delivered")?;
            let InboundPacket::AuthenticatedPlaintext {
                src_ip, payload, ..
            } = received
            else {
                panic!("batched direct UDP data must be authenticated plaintext");
            };
            assert_eq!(src_ip, IpAddr::V6(ip_a));
            assert_eq!(&payload, expected);
        }
        Ok(())
    }

    #[tokio::test]
    async fn udp_dispatch_tampered_packet_drops_only_itself() -> Result<()> {
        for bad_index in [0usize, 3, 7] {
            let (engine_a, engine_b, _ip_a, ip_b, endpoint_a, _) =
                established_direct_peers().await?;
            let payloads = (0..8u8)
                .map(|marker| vec![0x80 + marker; 1_420])
                .collect::<Vec<_>>();
            let mut clean = Vec::with_capacity(payloads.len());
            for payload in &payloads {
                clean.push(engine_a.encrypt_for_peer(ip_b, payload).await?);
            }
            let mut packets = clean.clone();
            *packets[bad_index].last_mut().expect("ciphertext tag") ^= 0x40;

            let (incoming_tx, mut incoming_rx, _) = inbound_queue::channel(32);
            dispatch_udp_batch(
                &engine_b.socket,
                &engine_b.connections,
                &engine_b.ip_index,
                &engine_b.receiver_index,
                &engine_b.local_ip,
                &engine_b.overlay,
                &incoming_tx,
                packets
                    .into_iter()
                    .map(|packet| (endpoint_a, InboundPayload::from(packet)))
                    .collect(),
            )
            .await;

            for (index, expected) in payloads.iter().enumerate() {
                if index == bad_index {
                    continue;
                }
                let received = tokio::time::timeout(Duration::from_secs(1), incoming_rx.recv())
                    .await?
                    .with_context(|| {
                        format!("valid packet {index} was discarded with bad packet {bad_index}")
                    })?;
                let InboundPacket::AuthenticatedPlaintext { payload, .. } = received else {
                    panic!("clean batch packet must be authenticated plaintext");
                };
                assert_eq!(&payload, expected);
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(50), incoming_rx.recv())
                    .await
                    .is_err(),
                "tampered packet {bad_index} must not be delivered"
            );

            // The bad slot's replay counter was not consumed, so the exact clean
            // replacement must be accepted; a second copy must then be rejected.
            for attempt in 0..2 {
                dispatch_udp_batch(
                    &engine_b.socket,
                    &engine_b.connections,
                    &engine_b.ip_index,
                    &engine_b.receiver_index,
                    &engine_b.local_ip,
                    &engine_b.overlay,
                    &incoming_tx,
                    vec![(endpoint_a, InboundPayload::from(clean[bad_index].clone()))],
                )
                .await;
                if attempt == 0 {
                    let received = tokio::time::timeout(Duration::from_secs(1), incoming_rx.recv())
                        .await?
                        .context("clean replacement for tampered slot was not delivered")?;
                    let InboundPacket::AuthenticatedPlaintext { payload, .. } = received else {
                        panic!("clean replacement must be authenticated plaintext");
                    };
                    assert_eq!(&payload, &payloads[bad_index]);
                } else {
                    assert!(
                        tokio::time::timeout(Duration::from_millis(50), incoming_rx.recv())
                            .await
                            .is_err(),
                        "replayed clean replacement must be rejected"
                    );
                }
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn udp_dispatch_route_collision_falls_back_to_single_candidates() -> Result<()> {
        let (engine_a, engine_b, ip_a, ip_b, endpoint_a, _) = established_direct_peers().await?;
        let (secure, health) = {
            let connections = engine_b.connections.read().await;
            let peer = connections.get("a").expect("peer A");
            (peer.secure.clone(), peer.health.clone())
        };
        let collision_peer_info = {
            let connections = engine_b.connections.read().await;
            connections.get("a").expect("peer A").peer_info.clone()
        };
        engine_b.connections.write().await.insert(
            "collision".to_string(),
            PeerConnection {
                peer_info: collision_peer_info,
                endpoints: vec![endpoint_a],
                active_endpoint: Some(endpoint_a),
                health: health.clone(),
                secure: secure.clone(),
            },
        );
        engine_b.receiver_index.write().await.insert(
            route_info("collision", ip_a),
            secure,
            health,
            IpAddr::V6(ip_a),
            &[endpoint_a],
        );
        engine_b
            .receiver_index
            .read()
            .await
            .route_lookups
            .store(0, Ordering::Relaxed);

        let mut packets = Vec::new();
        for marker in 0..8u8 {
            packets.push(engine_a.encrypt_for_peer(ip_b, &[marker; 1_420]).await?);
        }
        let (incoming_tx, mut incoming_rx, _) = inbound_queue::channel(32);
        dispatch_udp_batch(
            &engine_b.socket,
            &engine_b.connections,
            &engine_b.ip_index,
            &engine_b.receiver_index,
            &engine_b.local_ip,
            &engine_b.overlay,
            &incoming_tx,
            packets
                .into_iter()
                .map(|packet| (endpoint_a, InboundPayload::from(packet)))
                .collect(),
        )
        .await;

        assert_eq!(
            engine_b.receiver_index.read().await.route_lookups.load(Ordering::Relaxed),
            16,
            "each colliding packet takes one dispatch lookup plus one candidate lookup, proving batch bypass"
        );
        for marker in 0..8u8 {
            let received = tokio::time::timeout(Duration::from_secs(1), incoming_rx.recv())
                .await?
                .context("collision fallback packet was not delivered")?;
            let InboundPacket::AuthenticatedPlaintext { payload, .. } = received else {
                panic!("collision fallback must deliver authenticated plaintext");
            };
            assert_eq!(payload, vec![marker; 1_420]);
        }
        Ok(())
    }

    #[tokio::test]
    async fn receiver_index_collision_authenticates_second_candidate_after_bad_hint() -> Result<()>
    {
        let (engine_a, engine_b, ip_a, ip_b, endpoint_a, _) = established_direct_peers().await?;
        let decoy_identity = Identity::generate()?;
        let decoy_ip = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 99);
        engine_b
            .add_peer(PeerInfo::new(
                "decoy".to_string(),
                decoy_ip,
                decoy_identity.public_key(),
                vec![endpoint_a],
                fuckipv6_common::NatType::Open,
            ))
            .await?;

        let (real_secure, real_health) = {
            let connections = engine_b.connections.read().await;
            let peer = connections.get("a").expect("real peer A");
            (peer.secure.clone(), peer.health.clone())
        };
        let (decoy_secure, decoy_health) = {
            let connections = engine_b.connections.read().await;
            let peer = connections.get("decoy").expect("decoy peer");
            (peer.secure.clone(), peer.health.clone())
        };
        let real_route = Arc::new(ReceiverRoute {
            peer: route_info("a", ip_a),
            secure: real_secure.clone(),
            health: real_health,
            src_ip: IpAddr::V6(ip_a),
        });
        let decoy_route = Arc::new(ReceiverRoute {
            peer: route_info("decoy", decoy_ip),
            secure: decoy_secure,
            health: decoy_health,
            src_ip: IpAddr::V6(decoy_ip),
        });

        // Model a 32-bit receiver-index collision directly: the endpoint hint
        // points at the decoy, while the packet was sealed for the real peer at
        // the second slot in the same index bucket.
        {
            let mut index = engine_b.receiver_index.write().await;
            let colliding_index = real_secure.local_receiver_index();
            index.by_index.insert(
                colliding_index,
                vec![decoy_route.clone(), real_route.clone()],
            );
            index.by_endpoint.insert(
                (colliding_index, canonical_udp_endpoint(endpoint_a)),
                vec![decoy_route],
            );
            index.by_client.insert("a".to_string(), real_route);
        }

        let ciphertext = engine_a.encrypt_for_peer(ip_b, b"second candidate").await?;
        let (incoming_tx, mut incoming_rx, _) = inbound_queue::channel(4);
        dispatch_udp_batch(
            &engine_b.socket,
            &engine_b.connections,
            &engine_b.ip_index,
            &engine_b.receiver_index,
            &engine_b.local_ip,
            &engine_b.overlay,
            &incoming_tx,
            vec![(endpoint_a, InboundPayload::from(ciphertext))],
        )
        .await;

        let received = tokio::time::timeout(Duration::from_secs(1), incoming_rx.recv())
            .await?
            .context("real second candidate was not delivered")?;
        let InboundPacket::AuthenticatedPlaintext {
            src_ip,
            peer,
            payload,
        } = received
        else {
            panic!("collision authentication must deliver plaintext");
        };
        assert_eq!(src_ip, IpAddr::V6(ip_a));
        assert_eq!(
            peer.client_id.as_ref(),
            "a",
            "the authenticated candidate's route must travel with the plaintext, not the endpoint hint's route"
        );
        assert_eq!(peer.virtual_ip, ip_a);
        assert_eq!(&payload, b"second candidate");
        Ok(())
    }

    #[tokio::test]
    async fn test_p2p_engine_create() -> Result<()> {
        let engine = P2pEngine::new(0).await?;
        assert!(engine.local_addr().port() > 0);
        Ok(())
    }

    #[tokio::test]
    async fn add_peer_requires_local_identity_and_virtual_ip() -> Result<()> {
        let engine = P2pEngine::new(0).await?;
        let local = Identity::generate()?;
        let peer = Identity::generate()?;
        let peer_info = PeerInfo::new(
            peer.client_id().to_string(),
            "fd00::2".parse()?,
            peer.public_key(),
            vec![SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 49001)],
            fuckipv6_common::NatType::Open,
        );

        assert!(engine.add_peer(peer_info.clone()).await.is_err());
        engine.set_identity(local).await;
        assert!(engine.add_peer(peer_info.clone()).await.is_err());
        engine.set_local_ip("fd00::1".parse::<Ipv6Addr>()?).await;
        engine.add_peer(peer_info).await?;
        Ok(())
    }

    #[tokio::test]
    async fn receiver_index_tracks_one_hundred_peers_and_removes_stale_routes() -> Result<()> {
        let engine = P2pEngine::new(0).await?;
        engine.set_identity(Identity::generate()?).await;
        engine.set_local_ip("fd00::1".parse::<Ipv6Addr>()?).await;

        let mut inserted = Vec::new();
        for number in 1..=100u16 {
            let identity = Identity::generate()?;
            let client_id = format!("peer-{number}");
            let ipv6 = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, number);
            let ipv4 = Ipv4Addr::new(10, 8, 0, number as u8);
            let endpoint =
                SocketAddr::new(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1)), 40_000 + number);
            let virtual_ips = VirtualIps::dual_stack(ipv4, ipv6);
            engine
                .add_peer(PeerInfo::new_with_ips(
                    client_id.clone(),
                    virtual_ips,
                    identity.public_key(),
                    vec![endpoint],
                    NatType::Open,
                ))
                .await?;
            let secure = engine
                .connections
                .read()
                .await
                .get(&client_id)
                .expect("newly inserted peer")
                .secure
                .clone();
            inserted.push((client_id, ipv4, ipv6, secure, endpoint));
        }

        assert_eq!(engine.connections.read().await.len(), 100);
        let mut indices = inserted
            .iter()
            .map(|(_, _, _, secure, _)| secure.local_receiver_index())
            .collect::<Vec<_>>();
        indices.sort_unstable();
        indices.dedup();
        assert_eq!(
            indices.len(),
            100,
            "pair-scoped receiver indices must not collapse to one local static-key prefix"
        );
        {
            let receiver_index = engine.receiver_index.read().await;
            receiver_index.route_lookups.store(0, Ordering::Relaxed);
            receiver_index
                .candidates_returned
                .store(0, Ordering::Relaxed);
            for (_, _, _, secure, endpoint) in &inserted {
                let (route, extra_candidates) = receiver_index
                    .candidate_snapshot(secure.local_receiver_index(), *endpoint)
                    .expect("every registered peer must have a receiver route");
                assert!(Arc::ptr_eq(&route.secure, secure));
                assert!(extra_candidates.is_empty());
            }
            assert_eq!(receiver_index.route_lookups.load(Ordering::Relaxed), 100);
            assert_eq!(
                receiver_index.candidates_returned.load(Ordering::Relaxed),
                100
            );

            // An unknown endpoint (for example after a NAT rebind) must still
            // resolve to one pair-scoped candidate, not all peers on the node.
            receiver_index.route_lookups.store(0, Ordering::Relaxed);
            receiver_index
                .candidates_returned
                .store(0, Ordering::Relaxed);
            let unknown_endpoint =
                SocketAddr::new(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1)), 49_999);
            let lookup_secure = &inserted[0].3;
            let (route, rebound_candidates) = receiver_index
                .candidate_snapshot(lookup_secure.local_receiver_index(), unknown_endpoint)
                .expect("pair-scoped receiver index must resolve from an unknown endpoint");
            assert!(Arc::ptr_eq(&route.secure, lookup_secure));
            assert!(rebound_candidates.is_empty());
            assert_eq!(receiver_index.route_lookups.load(Ordering::Relaxed), 1);
            assert_eq!(
                receiver_index.candidates_returned.load(Ordering::Relaxed),
                1
            );
        }

        let (removed_id, removed_ipv4, removed_ipv6, removed_secure, _) = &inserted[42];
        assert!(engine.contains_peer_ip(IpAddr::V6(*removed_ipv6)).await);
        assert!(engine.contains_peer_ip(IpAddr::V4(*removed_ipv4)).await);
        assert_eq!(
            engine
                .peer_route_by_ip(IpAddr::V4(*removed_ipv4))
                .await
                .expect("registered IPv4 route")
                .client_id
                .as_ref(),
            removed_id
        );
        assert_eq!(
            engine.resolve_exit_peer(Some(removed_id)).await,
            Some(*removed_ipv6)
        );
        assert_eq!(
            engine
                .resolve_exit_peer(Some(&removed_ipv4.to_string()))
                .await,
            Some(*removed_ipv6)
        );

        assert_eq!(
            engine.remove_peer_by_id(removed_id).await,
            Some(IpAddr::V4(*removed_ipv4))
        );
        assert!(
            !engine
                .receiver_index
                .read()
                .await
                .routes(removed_secure.local_receiver_index())
                .iter()
                .any(|route| Arc::ptr_eq(&route.secure, removed_secure)),
            "peer removal must not leave a stale receiver route"
        );
        Ok(())
    }

    #[tokio::test]
    async fn receiver_index_preserves_colliding_candidates() -> Result<()> {
        let engine = P2pEngine::new(0).await?;
        let secure = SecurePeerState::new(
            Identity::generate()?.secure_session_identity(),
            "fd00::1".parse()?,
            &PeerInfo::new(
                "peer-a".to_string(),
                "fd00::2".parse()?,
                Identity::generate()?.public_key(),
                vec![SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 41000)],
                NatType::Open,
            ),
            engine.handshake_admission.clone(),
            engine.cookie_key.clone(),
        )?;
        let mut index = ReceiverIndex::default();
        let receiver_index = secure.local_receiver_index();
        let endpoint_a = SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 41000);
        let endpoint_b = SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 41001);
        index.insert(
            route_info("peer-a", "fd00::2".parse()?),
            secure.clone(),
            Arc::new(PeerHealth::new(HEALTH_WINDOW_SIZE)),
            "fd00::2".parse()?,
            &[endpoint_a],
        );
        index.insert(
            route_info("peer-b", "fd00::3".parse()?),
            secure,
            Arc::new(PeerHealth::new(HEALTH_WINDOW_SIZE)),
            "fd00::3".parse()?,
            &[endpoint_b],
        );

        index.route_lookups.store(0, Ordering::Relaxed);
        index.candidates_returned.store(0, Ordering::Relaxed);
        let unknown_endpoint = SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 41999);
        let (_, extra_candidates) = index
            .candidate_snapshot(receiver_index, unknown_endpoint)
            .expect("colliding route must be indexed");
        assert_eq!(extra_candidates.len(), 1);
        assert_eq!(index.route_lookups.load(Ordering::Relaxed), 1);
        assert_eq!(index.candidates_returned.load(Ordering::Relaxed), 2);
        index.remove_client("peer-a");
        assert_eq!(index.routes(receiver_index).len(), 1);
        assert_eq!(
            index.routes(receiver_index)[0].peer.client_id.as_ref(),
            "peer-b"
        );
        Ok(())
    }

    #[tokio::test]
    async fn unchanged_authenticated_endpoint_takes_read_only_fast_path() -> Result<()> {
        let engine = P2pEngine::new(0).await?;
        let local = Identity::generate()?;
        let local_ip = "fd00::1".parse::<Ipv6Addr>()?;
        let peer_identity = Identity::generate()?;
        let peer_id = "endpoint-fast-path-peer";
        let endpoint = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 20)), 42000);
        let peer_info = PeerInfo::new(
            peer_id.to_string(),
            "fd00::2".parse()?,
            peer_identity.public_key(),
            vec![endpoint],
            NatType::Open,
        );
        let secure = SecurePeerState::new(
            local.secure_session_identity(),
            IpAddr::V6(local_ip),
            &peer_info,
            engine.handshake_admission.clone(),
            engine.cookie_key.clone(),
        )?;
        engine.connections.write().await.insert(
            peer_id.to_string(),
            PeerConnection {
                peer_info,
                endpoints: vec![endpoint],
                active_endpoint: Some(endpoint),
                health: Arc::new(PeerHealth::new(HEALTH_WINDOW_SIZE)),
                secure: secure.clone(),
            },
        );

        let read_guard = engine.connections.read().await;
        let result = tokio::time::timeout(
            Duration::from_millis(500),
            update_authenticated_peer_endpoint(&engine.connections, peer_id, &secure, endpoint),
        )
        .await
        .context("unchanged endpoint must not wait for a connection write lock")?;
        assert!(result);
        drop(read_guard);

        let changed = SocketAddr::new(endpoint.ip(), endpoint.port() + 1);
        assert!(
            update_authenticated_peer_endpoint(&engine.connections, peer_id, &secure, changed)
                .await
        );
        let connections = engine.connections.read().await;
        let connection = connections.get(peer_id).expect("peer remains registered");
        assert_eq!(connection.active_endpoint, Some(changed));
        assert!(connection.endpoints.contains(&changed));
        Ok(())
    }

    #[tokio::test]
    async fn symmetric_nat_probe_learns_one_same_ip_port_then_pins_it() -> Result<()> {
        let engine = P2pEngine::new(0).await?;
        let local = Identity::generate()?;
        let peer = Identity::generate()?;
        let local_ip = "fd00::1".parse::<Ipv6Addr>()?;
        let peer_ip = "fd00::2".parse::<Ipv6Addr>()?;
        let advertised = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10)), 41000);
        let observed = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10)), 52000);
        let unrelated = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 11)), 52000);

        engine.set_identity(local).await;
        engine.set_local_ip(local_ip).await;
        engine
            .add_peer(PeerInfo::new(
                peer.client_id().to_string(),
                peer_ip,
                peer.public_key(),
                vec![advertised],
                fuckipv6_common::NatType::Symmetric,
            ))
            .await?;

        let peer_id = peer.client_id().to_string();
        assert!(peer_accepts_probe_source(&engine.connections, &peer_id, advertised).await);
        assert!(peer_accepts_probe_source(&engine.connections, &peer_id, observed).await);
        assert!(!peer_accepts_probe_source(&engine.connections, &peer_id, unrelated).await);

        update_peer_endpoint(&engine.connections, &peer_id, observed).await;
        let rebound = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10)), 53000);
        assert!(peer_accepts_probe_source(&engine.connections, &peer_id, observed).await);
        assert!(!peer_accepts_probe_source(&engine.connections, &peer_id, rebound).await);
        assert!(!peer_accepts_endpoint(&engine.connections, &peer_id, rebound).await);
        Ok(())
    }

    #[tokio::test]
    async fn send_before_session_confirmation_fails_closed() -> Result<()> {
        let engine = P2pEngine::new(0).await?;
        let local = Identity::generate()?;
        let peer = Identity::generate()?;
        let local_ip = "fd00::1".parse::<Ipv6Addr>()?;
        let peer_ip = "fd00::2".parse::<Ipv6Addr>()?;
        engine.set_identity(local).await;
        engine.set_local_ip(local_ip).await;
        engine
            .add_peer(PeerInfo::new(
                peer.client_id().to_string(),
                peer_ip,
                peer.public_key(),
                vec![SocketAddr::new(
                    IpAddr::V6(Ipv6Addr::LOCALHOST),
                    engine.local_addr().port(),
                )],
                fuckipv6_common::NatType::Open,
            ))
            .await?;

        assert!(engine
            .send_to(peer_ip, local_ip, b"must not leak")
            .await
            .is_err());
        assert!(engine
            .decrypt_from_peer(peer_ip, b"must not be accepted")
            .await
            .is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_p2p_engine_data_transfer() -> Result<()> {
        let engine_a = P2pEngine::new(0).await?;
        let engine_b = P2pEngine::new(0).await?;
        let identity_a = Identity::generate()?;
        let identity_b = Identity::generate()?;

        let ip_a = Ipv6Addr::LOCALHOST;
        let ip_b = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 2);

        engine_a.set_identity(identity_a.clone()).await;
        engine_b.set_identity(identity_b.clone()).await;
        engine_a.set_local_ip(ip_a).await;
        engine_b.set_local_ip(ip_b).await;

        let endpoint_a = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            engine_a.local_addr().port(),
        );
        let endpoint_b = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            engine_b.local_addr().port(),
        );

        engine_a
            .add_peer(PeerInfo::new(
                "b".to_string(),
                ip_b,
                identity_b.public_key(),
                vec![endpoint_b],
                fuckipv6_common::NatType::Open,
            ))
            .await?;

        engine_b
            .add_peer(PeerInfo::new(
                "a".to_string(),
                ip_a,
                identity_a.public_key(),
                vec![endpoint_a],
                fuckipv6_common::NatType::Open,
            ))
            .await?;

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if engine_a
                    .has_established_session_by_ip(IpAddr::V6(ip_b))
                    .await
                    && engine_b
                        .has_established_session_by_ip(IpAddr::V6(ip_a))
                        .await
                {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;

        // A direct transport failure returns the one sealed packet that every
        // fallback transport must reuse. The receiver accepts it once and the
        // replay window rejects a second submission of that exact ciphertext.
        let fallback_payload = b"fallback must not reseal";
        engine_a.set_fail_direct_transport(true);
        let fallback_packet = match engine_a
            .send_to_or_fallback(ip_b, ip_a, fallback_payload)
            .await?
        {
            SendToOutcome::Sent => panic!("injected direct failure must request fallback"),
            SendToOutcome::SealedForFallback { packet, .. } => packet,
        };
        engine_a.set_fail_direct_transport(false);
        assert_eq!(
            engine_a.direct_seal_count(),
            1,
            "one fallback chain must seal exactly once"
        );
        assert_eq!(
            engine_b.decrypt_from_peer(ip_a, &fallback_packet).await?,
            fallback_payload
        );
        assert!(
            engine_b
                .decrypt_from_peer(ip_a, &fallback_packet)
                .await
                .is_err(),
            "reusing ciphertext across transports must not create a replay exception"
        );

        let payload = vec![0x60, 0, 0, 0, 0, 0, 0, 0];
        let mut direct_packet = vec![0u8; DATA_HEADER_LEN + payload.len() + AEAD_TAG_LEN];
        direct_packet[DATA_HEADER_LEN..DATA_HEADER_LEN + payload.len()].copy_from_slice(&payload);
        match engine_a
            .send_to_or_fallback_in_place(ip_b, ip_a, &mut direct_packet, payload.len())
            .await?
        {
            InPlaceSendOutcome::Sent => {}
            InPlaceSendOutcome::SealedForFallback { .. } => {
                panic!("test direct transport unexpectedly required fallback")
            }
        }

        let received = tokio::time::timeout(Duration::from_secs(1), engine_b.recv())
            .await?
            .expect("engine B should receive P2P payload");
        let (received_src, received_peer, received_plaintext) = match received {
            InboundPacket::AuthenticatedPlaintext {
                src_ip,
                peer,
                payload,
            } => (src_ip, peer, payload),
            InboundPacket::SealedCiphertext { .. } => {
                panic!("direct UDP data must enter the client loop as authenticated plaintext")
            }
        };

        assert_eq!(received_src, IpAddr::V6(ip_a));
        assert_eq!(received_peer.client_id.as_ref(), "a");
        assert_eq!(received_peer.virtual_ip, ip_a);
        assert!(
            received_plaintext.is_pooled(),
            "direct UDP plaintext must retain the receiver allocation"
        );
        assert_eq!(received_plaintext, payload);
        assert!(
            engine_b
                .decrypt_from_peer(ip_a, &received_plaintext)
                .await
                .is_err(),
            "already-authenticated plaintext must not be treated as relay ciphertext"
        );

        drop(received_plaintext);
        for marker in 0..64u8 {
            let mut burst_packet = vec![0u8; DATA_HEADER_LEN + payload.len() + AEAD_TAG_LEN];
            burst_packet[DATA_HEADER_LEN..DATA_HEADER_LEN + payload.len()]
                .copy_from_slice(&payload);
            burst_packet[DATA_HEADER_LEN] = marker;
            match engine_a
                .send_to_or_fallback_in_place(ip_b, ip_a, &mut burst_packet, payload.len())
                .await?
            {
                InPlaceSendOutcome::Sent => {}
                InPlaceSendOutcome::SealedForFallback { .. } => {
                    panic!("burst direct transport unexpectedly required fallback")
                }
            }

            let received = tokio::time::timeout(Duration::from_secs(1), engine_b.recv())
                .await?
                .expect("engine B should receive every direct burst packet");
            let InboundPacket::AuthenticatedPlaintext { payload, .. } = received else {
                panic!("direct UDP burst must arrive as authenticated plaintext")
            };
            assert!(payload.is_pooled());
        }
        let pool_metrics = engine_b.udp_buffer_pool_metrics();
        assert!(
            pool_metrics.hit_rate() >= 0.95,
            "steady direct UDP RX should reuse pooled allocations: {pool_metrics:?}"
        );
        assert!(
            pool_metrics.allocations <= engine_b.udp_batch_size() as u64 + 4,
            "steady direct UDP RX may allocate one receive batch, but must not allocate per packet: {pool_metrics:?}"
        );
        assert_eq!(pool_metrics.full_returns, 0);
        let queue_metrics = engine_b.inbound_queue_metrics();
        assert_eq!(queue_metrics.capacity, 1024);
        assert_eq!(queue_metrics.depth, 0);
        assert_eq!(queue_metrics.dequeued, queue_metrics.enqueued);
        assert_eq!(queue_metrics.dropped_full, 0);

        let mut tampered = engine_a.encrypt_for_peer(ip_b, &payload).await?;
        *tampered.last_mut().expect("ciphertext should not be empty") ^= 0x55;
        assert!(engine_b.decrypt_from_peer(ip_a, &tampered).await.is_err());

        let encrypted = engine_a.encrypt_for_peer(ip_b, &payload).await?;
        assert_eq!(engine_b.decrypt_from_peer(ip_a, &encrypted).await?, payload);
        assert!(
            engine_b.decrypt_from_peer(ip_a, &encrypted).await.is_err(),
            "relay ciphertext replay must be rejected"
        );

        let mut relay_tampered = encrypted;
        *relay_tampered
            .last_mut()
            .expect("relay ciphertext should not be empty") ^= 0x80;
        assert!(
            engine_b
                .decrypt_from_peer(ip_a, &relay_tampered)
                .await
                .is_err(),
            "tampered relay ciphertext must be rejected"
        );

        // Regression for the former 4096-entry plaintext cache: direct inbound
        // packets must remain deliverable when the consumer is more than 4096
        // packets behind. Authentication already happened in dispatch_secure_data;
        // the receive queue carries plaintext and must never open it again.
        let (incoming_tx, mut incoming_rx, _) = inbound_queue::channel(8_192);
        for sequence in 0..=4096u32 {
            let payload = sequence.to_be_bytes();
            let ciphertext = engine_a.encrypt_for_peer(ip_b, &payload).await?;
            dispatch_secure_data(
                &engine_b.connections,
                &engine_b.receiver_index,
                &incoming_tx,
                endpoint_a,
                ciphertext.clone().into(),
            )
            .await;
        }
        for sequence in 0..=4096u32 {
            let packet = tokio::time::timeout(Duration::from_secs(1), incoming_rx.recv())
                .await?
                .expect("all authenticated plaintext packets must remain queued");
            let (source, payload) = match packet {
                InboundPacket::AuthenticatedPlaintext {
                    src_ip, payload, ..
                } => (src_ip, payload),
                InboundPacket::SealedCiphertext { .. } => {
                    panic!("direct dispatch must never enqueue sealed ciphertext")
                }
            };
            assert_eq!(source, IpAddr::V6(ip_a));
            assert_eq!(payload, sequence.to_be_bytes());
        }

        let health = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let health = engine_a
                    .peer_health(&ip_b)
                    .await
                    .expect("engine A should have peer health");
                if health.rtt_us.is_some() && health.healthy {
                    return health;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;

        assert!(health.probes_sent > 0);
        assert!(health.probes_acked > 0);
        assert_eq!(health.loss_rate, 0.0);
        assert!(health.bandwidth_bps > 0);

        let probe = engine_a
            .probe_peer_now("b", Duration::from_secs(1))
            .await
            .expect("known peer should produce a probe result");
        assert!(!probe.timed_out);
        assert!(probe.p2p_active);
        assert!(probe.rtt_us.is_some());
        assert_eq!(probe.virtual_ip, ip_b);

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let health = engine_b
                    .peer_health(&ip_a)
                    .await
                    .expect("engine B should have peer health");
                if health.remote_quality.is_some() {
                    return;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn rejects_invalid_inbound_queue_capacity() {
        assert!(P2pEngine::new_with_inbound_queue_capacity(0, 0)
            .await
            .is_err());
        assert!(
            P2pEngine::new_with_inbound_queue_capacity(0, MAX_INBOUND_QUEUE_CAPACITY + 1)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn rejects_invalid_udp_batch_size() -> Result<()> {
        assert!(P2pEngine::new_with_limits(0, 1024, 0).await.is_err());
        assert!(P2pEngine::new_with_limits(0, 1024, 129).await.is_err());
        let engine = P2pEngine::new_with_limits(0, 1024, 32).await?;
        assert_eq!(engine.udp_batch_size(), 32);
        Ok(())
    }

    #[tokio::test]
    async fn secure_data_migrates_endpoint_only_after_authentication() -> Result<()> {
        let engine_a = P2pEngine::new(0).await?;
        let engine_b = P2pEngine::new(0).await?;
        let identity_a = Identity::generate()?;
        let identity_b = Identity::generate()?;
        let ip_a = Ipv6Addr::LOCALHOST;
        let ip_b = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 2);

        engine_a.set_identity(identity_a.clone()).await;
        engine_b.set_identity(identity_b.clone()).await;
        engine_a.set_local_ip(ip_a).await;
        engine_b.set_local_ip(ip_b).await;

        let endpoint_a = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            engine_a.local_addr().port(),
        );
        let endpoint_b = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            engine_b.local_addr().port(),
        );
        engine_a
            .add_peer(PeerInfo::new(
                "b".to_string(),
                ip_b,
                identity_b.public_key(),
                vec![endpoint_b],
                fuckipv6_common::NatType::Open,
            ))
            .await?;
        engine_b
            .add_peer(PeerInfo::new(
                "a".to_string(),
                ip_a,
                identity_a.public_key(),
                vec![endpoint_a],
                fuckipv6_common::NatType::Open,
            ))
            .await?;

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if engine_a
                    .has_established_session_by_ip(IpAddr::V6(ip_b))
                    .await
                    && engine_b
                        .has_established_session_by_ip(IpAddr::V6(ip_a))
                        .await
                {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;

        let rebound_socket = UdpSocket::bind((Ipv6Addr::LOCALHOST, 0)).await?;
        let rebound_endpoint = rebound_socket.local_addr()?;
        assert_ne!(rebound_endpoint.port(), engine_a.local_addr().port());
        let payload = vec![0x60, 0, 0, 0, 0, 0, 0, 1];
        let packet = engine_a.encrypt_for_peer(ip_b, &payload).await?;
        rebound_socket.send_to(&packet, endpoint_b).await?;

        let received = tokio::time::timeout(Duration::from_secs(1), engine_b.recv())
            .await?
            .expect("engine B should receive rebound plaintext");
        let (received_src, received_plaintext) = match received {
            InboundPacket::AuthenticatedPlaintext {
                src_ip, payload, ..
            } => (src_ip, payload),
            InboundPacket::SealedCiphertext { .. } => {
                panic!("authenticated UDP rebound must not re-enter as ciphertext")
            }
        };
        assert_eq!(received_src, IpAddr::V6(ip_a));
        assert_eq!(received_plaintext, payload);
        let active_endpoint = engine_b
            .connections
            .read()
            .await
            .get("a")
            .and_then(|connection| connection.active_endpoint);
        assert_eq!(active_endpoint, Some(rebound_endpoint));

        engine_b.send_to(ip_a, ip_b, b"response").await?;
        let mut response = vec![0u8; 2048];
        let (response_len, response_source) = tokio::time::timeout(
            Duration::from_secs(1),
            rebound_socket.recv_from(&mut response),
        )
        .await??;
        assert_eq!(response_source, endpoint_b);
        assert_eq!(
            engine_a
                .decrypt_from_peer(ip_b, &response[..response_len])
                .await?,
            b"response"
        );

        let replay = packet;
        rebound_socket.send_to(&replay, endpoint_b).await?;
        assert!(
            tokio::time::timeout(Duration::from_millis(150), engine_b.recv())
                .await
                .is_err(),
            "replayed ciphertext must not reach the receive queue"
        );
        let active_endpoint_after_replay = engine_b
            .connections
            .read()
            .await
            .get("a")
            .and_then(|connection| connection.active_endpoint);
        assert_eq!(active_endpoint_after_replay, Some(rebound_endpoint));
        Ok(())
    }

    #[tokio::test]
    async fn peer_removal_revokes_session_and_blocks_rebind_replay() -> Result<()> {
        let engine_a = P2pEngine::new(0).await?;
        let engine_b = P2pEngine::new(0).await?;
        let identity_a = Identity::generate()?;
        let identity_b = Identity::generate()?;
        let ip_a = Ipv6Addr::LOCALHOST;
        let ip_b = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 2);

        engine_a.set_identity(identity_a.clone()).await;
        engine_b.set_identity(identity_b.clone()).await;
        engine_a.set_local_ip(ip_a).await;
        engine_b.set_local_ip(ip_b).await;

        let endpoint_a = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            engine_a.local_addr().port(),
        );
        let endpoint_b = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            engine_b.local_addr().port(),
        );
        engine_a
            .add_peer(PeerInfo::new(
                "b".to_string(),
                ip_b,
                identity_b.public_key(),
                vec![endpoint_b],
                fuckipv6_common::NatType::Open,
            ))
            .await?;
        engine_b
            .add_peer(PeerInfo::new(
                "a".to_string(),
                ip_a,
                identity_a.public_key(),
                vec![endpoint_a],
                fuckipv6_common::NatType::Open,
            ))
            .await?;

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if engine_a
                    .has_established_session_by_ip(IpAddr::V6(ip_b))
                    .await
                    && engine_b
                        .has_established_session_by_ip(IpAddr::V6(ip_a))
                        .await
                {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;

        let payload = b"authenticated before removal";
        let packet = engine_a.encrypt_for_peer(ip_b, payload).await?;
        let old_secure = engine_b
            .connections
            .read()
            .await
            .get("a")
            .expect("peer A")
            .secure
            .clone();

        // Revocation must reject relay ciphertext immediately, even before the
        // connection index is removed by the rest of PeerOffline handling.
        assert!(old_secure.revoke().await);
        assert!(
            engine_b.decrypt_from_peer(ip_a, &packet).await.is_err(),
            "a revoked session must not open new ciphertext"
        );
        assert!(
            engine_b.send_to(ip_a, ip_b, b"after revoke").await.is_err(),
            "a revoked session must reject sends while it is still indexed"
        );

        assert_eq!(
            engine_b.remove_peer_by_id("a").await,
            Some(IpAddr::V6(ip_a))
        );
        assert!(!old_secure.is_established().await);
        assert_eq!(old_secure.authenticated_peer().await, None);
        assert!(matches!(
            old_secure.seal(payload).await,
            Err(super::super::secure_runtime::SecureRuntimeError::Revoked)
        ));
        assert!(matches!(
            old_secure
                .open_from_source(endpoint_a, packet.clone())
                .await,
            Err(super::super::secure_runtime::SecureRuntimeError::Revoked)
        ));

        // A later registration can reuse the same client ID and virtual IP. It
        // must receive a fresh session instance rather than inheriting a terminal
        // tombstone from the previous session.
        engine_b
            .add_peer(PeerInfo::new(
                "a".to_string(),
                ip_a,
                identity_a.public_key(),
                vec![endpoint_a],
                fuckipv6_common::NatType::Open,
            ))
            .await?;
        let replacement_secure = engine_b
            .connections
            .read()
            .await
            .get("a")
            .expect("replacement peer A")
            .secure
            .clone();
        assert!(!Arc::ptr_eq(&old_secure, &replacement_secure));
        assert!(engine_b.decrypt_from_peer(ip_a, &packet).await.is_err());

        let (incoming_tx, mut incoming_rx, _) = inbound_queue::channel(1);
        dispatch_secure_data(
            &engine_b.connections,
            &engine_b.receiver_index,
            &incoming_tx,
            endpoint_a,
            packet.clone().into(),
        )
        .await;
        assert!(
            tokio::time::timeout(Duration::from_millis(100), incoming_rx.recv())
                .await
                .is_err(),
            "a delayed frame from the retired session must not reach the receive queue"
        );
        Ok(())
    }

    async fn wait_for_epoch(engine: &P2pEngine, client_id: &str, epoch: u16) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let current = {
                    let connections = engine.connections.read().await;
                    connections
                        .get(client_id)
                        .map(|connection| connection.secure.clone())
                };
                if let Some(current) = current {
                    if current.epoch().await == Some(epoch) {
                        return;
                    }
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await?;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn initiator_automatically_rekeys_established_udp_session() -> Result<()> {
        let engine_a = P2pEngine::new(0).await?;
        let engine_b = P2pEngine::new(0).await?;
        let identity_a = Identity::generate()?;
        let identity_b = Identity::generate()?;
        let ip_a = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 4);
        let ip_b = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 5);

        engine_a.set_identity(identity_a.clone()).await;
        engine_b.set_identity(identity_b.clone()).await;
        engine_a.set_local_ip(ip_a).await;
        engine_b.set_local_ip(ip_b).await;

        let endpoint_a = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            engine_a.local_addr().port(),
        );
        let endpoint_b = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            engine_b.local_addr().port(),
        );
        engine_a
            .add_peer(PeerInfo::new(
                "b".to_string(),
                ip_b,
                identity_b.public_key(),
                vec![endpoint_b],
                fuckipv6_common::NatType::Open,
            ))
            .await?;
        engine_b
            .add_peer(PeerInfo::new(
                "a".to_string(),
                ip_a,
                identity_a.public_key(),
                vec![endpoint_a],
                fuckipv6_common::NatType::Open,
            ))
            .await?;

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if engine_a
                    .has_established_session_by_ip(IpAddr::V6(ip_b))
                    .await
                    && engine_b
                        .has_established_session_by_ip(IpAddr::V6(ip_a))
                        .await
                {
                    return;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;

        let a_is_initiator = {
            let connections = engine_a.connections.read().await;
            connections.get("b").expect("peer b").secure.is_initiator()
        };
        let (initiator_engine, initiator_ip, responder_engine, responder_ip, responder_id) =
            if a_is_initiator {
                (&engine_a, ip_a, &engine_b, ip_b, "a")
            } else {
                (&engine_b, ip_b, &engine_a, ip_a, "b")
            };
        {
            let connections = initiator_engine.connections.read().await;
            let peer_id = if a_is_initiator { "b" } else { "a" };
            connections
                .get(peer_id)
                .expect("initiator peer")
                .secure
                .force_rekey_due()
                .await;
        }

        // The resident initiator task must send HELLO, install staged keys from
        // ACCEPT, and retry CONFIRM until the responder advances to epoch 2.
        wait_for_epoch(responder_engine, responder_id, 2).await?;

        // Authenticated responder traffic is required for the initiator commit.
        responder_engine
            .send_to(initiator_ip, responder_ip, b"rekey commit")
            .await?;
        let initiator_id = if a_is_initiator { "b" } else { "a" };
        wait_for_epoch(initiator_engine, initiator_id, 2).await?;

        initiator_engine
            .send_to(responder_ip, initiator_ip, b"post-rekey data")
            .await?;
        let packet = tokio::time::timeout(Duration::from_secs(2), responder_engine.recv())
            .await?
            .context("post-rekey packet was not delivered")?;
        let (source, payload) = match packet {
            InboundPacket::AuthenticatedPlaintext {
                src_ip, payload, ..
            } => (src_ip, payload),
            InboundPacket::SealedCiphertext { .. } => {
                panic!("post-rekey direct traffic must enter as authenticated plaintext")
            }
        };
        assert_eq!(source, IpAddr::V6(initiator_ip));
        assert_eq!(payload, b"post-rekey data");

        // The rekeyed session must still have one-shot replay semantics under
        // concurrent relay arrivals. Race many opens of the same valid frame;
        // exactly one may commit the counter and the rest must fail closed.
        let replay_race_packet = initiator_engine
            .encrypt_for_peer(responder_ip, b"post-rekey concurrent replay")
            .await?;
        let mut replay_tasks = Vec::new();
        for _ in 0..16 {
            let engine = responder_engine.clone();
            let packet = replay_race_packet.clone();
            replay_tasks.push(tokio::spawn(async move {
                engine
                    .decrypt_from_peer(initiator_ip, &packet)
                    .await
                    .is_ok()
            }));
        }
        let mut accepted = 0;
        for task in replay_tasks {
            accepted += usize::from(task.await?);
        }
        assert_eq!(
            accepted, 1,
            "a rekeyed session must admit one copy of concurrent replay traffic"
        );

        // Removal must terminate the newly rotated session, not just its old
        // epoch. A valid frame sealed immediately before removal must fail
        // closed after the peer is gone.
        let removal_probe = initiator_engine
            .encrypt_for_peer(responder_ip, b"must not survive removal")
            .await?;
        assert_eq!(
            responder_engine.remove_peer_by_id(responder_id).await,
            Some(IpAddr::V6(initiator_ip))
        );
        assert!(responder_engine
            .decrypt_from_peer(initiator_ip, &removal_probe)
            .await
            .is_err());
        assert!(responder_engine
            .send_to(initiator_ip, responder_ip, b"removed peer")
            .await
            .is_err());
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn peer_removal_waits_for_inflight_send_and_blocks_late_send() -> Result<()> {
        let engine_a = P2pEngine::new(0).await?;
        let engine_b = P2pEngine::new(0).await?;
        let identity_a = Identity::generate()?;
        let identity_b = Identity::generate()?;
        let ip_a = Ipv6Addr::LOCALHOST;
        let ip_b = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 3);

        engine_a.set_identity(identity_a.clone()).await;
        engine_b.set_identity(identity_b.clone()).await;
        engine_a.set_local_ip(ip_a).await;
        engine_b.set_local_ip(ip_b).await;

        let endpoint_a = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            engine_a.local_addr().port(),
        );
        let endpoint_b = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            engine_b.local_addr().port(),
        );
        engine_a
            .add_peer(PeerInfo::new(
                "b".to_string(),
                ip_b,
                identity_b.public_key(),
                vec![endpoint_b],
                fuckipv6_common::NatType::Open,
            ))
            .await?;
        engine_b
            .add_peer(PeerInfo::new(
                "a".to_string(),
                ip_a,
                identity_a.public_key(),
                vec![endpoint_a],
                fuckipv6_common::NatType::Open,
            ))
            .await?;

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if engine_a
                    .has_established_session_by_ip(IpAddr::V6(ip_b))
                    .await
                    && engine_b
                        .has_established_session_by_ip(IpAddr::V6(ip_a))
                        .await
                {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;

        let gate = Arc::new(SendRaceGate {
            sealed: tokio::sync::Barrier::new(2),
            release: tokio::sync::Barrier::new(2),
        });
        *engine_a.send_race_gate.write().await = Some(gate.clone());

        let sender = engine_a.clone();
        let send_task =
            tokio::spawn(async move { sender.send_to(ip_b, ip_a, b"in-flight ciphertext").await });
        tokio::time::timeout(Duration::from_secs(1), gate.sealed.wait()).await?;

        let remover = engine_a.clone();
        let mut remove_task = tokio::spawn(async move { remover.remove_peer_by_id("b").await });
        let removal_completed_early =
            tokio::time::timeout(Duration::from_millis(150), &mut remove_task)
                .await
                .is_ok();

        gate.release.wait().await;
        send_task.await??;
        let removed = tokio::time::timeout(Duration::from_secs(1), remove_task).await??;

        assert!(
            !removal_completed_early,
            "peer removal must not revoke the session while a sealed datagram is between seal and UDP send"
        );
        assert_eq!(removed, Some(IpAddr::V6(ip_b)));
        assert!(
            engine_a
                .send_to(ip_b, ip_a, b"late plaintext")
                .await
                .is_err(),
            "a send started after removal must fail instead of using the retired session mapping"
        );
        Ok(())
    }

    #[tokio::test]
    async fn dual_stack_socket_sends_to_ipv4_loopback() -> Result<()> {
        let sender = bind_dual_stack_udp(0)?;
        let receiver = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let destination = receiver.local_addr()?;

        sender
            .send_to(b"mapped", dual_stack_send_endpoint(destination))
            .await?;

        let mut buffer = [0u8; 16];
        let (size, source) =
            tokio::time::timeout(Duration::from_secs(1), receiver.recv_from(&mut buffer)).await??;
        assert_eq!(&buffer[..size], b"mapped");
        assert_eq!(
            canonical_udp_endpoint(source).port(),
            sender.local_addr()?.port()
        );
        Ok(())
    }
}
