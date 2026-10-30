use anyhow::{Context, Result};
use fuckipv6_common::crypto::Identity;
use fuckipv6_common::{
    codec, ControlMessage, DataPacket, FallbackThreshold, NatDetectionConfig, NatType, P2pDatagram,
    PeerInfo,
};
use fuckipv6_secure_session::wire::{decode_control, is_data_packet, CONTROL_MAGIC};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, RwLock};
use tokio::time::{sleep, Duration, Instant};

use super::health::{ConnectionHealthSnapshot, PeerHealth};
use super::secure_runtime::SecurePeerState;

const HEALTH_WINDOW_SIZE: usize = 20;
const PROBE_RESPONSE_GRACE: Duration = Duration::from_secs(1);
const FAST_PROBE_ATTEMPTS: usize = 20;
const FAST_PROBE_INTERVAL: Duration = Duration::from_millis(500);
const HEALTH_PROBE_INTERVAL: Duration = Duration::from_secs(2);
const SECURE_HANDSHAKE_RETRY_INTERVAL: Duration = Duration::from_millis(250);
const AUTHENTICATED_INBOUND_CACHE_CAPACITY: usize = 4096;

type IncomingPacket = (IpAddr, Vec<u8>);
type IncomingReceiver = mpsc::UnboundedReceiver<IncomingPacket>;
type AuthenticatedInboundKey = (IpAddr, blake3::Hash);

/// Plaintexts authenticated by the UDP receiver while routing a v2 frame to
/// its session. `decrypt_from_peer` consumes this cache so the shared event
/// loop does not attempt a second AEAD open (and reject it as a replay).
#[derive(Default)]
struct AuthenticatedInboundCache {
    payloads: HashMap<AuthenticatedInboundKey, (u64, Vec<u8>)>,
    order: VecDeque<(AuthenticatedInboundKey, u64)>,
    next_generation: u64,
}

impl AuthenticatedInboundCache {
    fn insert(&mut self, source: IpAddr, ciphertext: &[u8], plaintext: Vec<u8>) {
        self.next_generation = self.next_generation.wrapping_add(1);
        let generation = self.next_generation;
        let key = (source, blake3::hash(ciphertext));
        self.payloads.insert(key, (generation, plaintext));
        self.order.push_back((key, generation));

        while self.payloads.len() > AUTHENTICATED_INBOUND_CACHE_CAPACITY {
            let Some((oldest, oldest_generation)) = self.order.pop_front() else {
                break;
            };
            if self
                .payloads
                .get(&oldest)
                .is_some_and(|(current_generation, _)| *current_generation == oldest_generation)
            {
                self.payloads.remove(&oldest);
            }
        }
    }

    fn take(&mut self, source: IpAddr, ciphertext: &[u8]) -> Option<Vec<u8>> {
        self.payloads
            .remove(&(source, blake3::hash(ciphertext)))
            .map(|(_, plaintext)| plaintext)
    }
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
    health: PeerHealth,
    secure: Arc<SecurePeerState>,
}

/// P2P引擎
#[derive(Clone)]
pub struct P2pEngine {
    local_addr: SocketAddr,
    socket: Arc<UdpSocket>,
    connections: Arc<RwLock<HashMap<String, PeerConnection>>>,
    ip_index: Arc<RwLock<HashMap<IpAddr, String>>>,
    incoming_rx: Arc<RwLock<IncomingReceiver>>,
    local_ip: Arc<RwLock<Option<IpAddr>>>,
    local_identity: Arc<RwLock<Option<Identity>>>,
    authenticated_inbound: Arc<RwLock<AuthenticatedInboundCache>>,
    fallback_threshold: Arc<RwLock<FallbackThreshold>>,
    quic_mesh: Arc<RwLock<Option<super::quic::QuicP2pMesh>>>,
    pub overlay: Arc<super::overlay::P2pOverlay>,
}

impl P2pEngine {
    /// 创建P2P引擎
    pub async fn new(listen_port: u16) -> Result<Self> {
        let socket = bind_dual_stack_udp(listen_port).context("无法绑定 UDP socket")?;
        Self::from_socket(socket)
    }

    /// Bind the P2P transport once, run STUN on that exact socket, then start
    /// the shared UDP receiver. Keeping one socket preserves the NAT mapping
    /// discovered by STUN and prevents a temporary probe port from being
    /// advertised as the P2P endpoint.
    pub async fn new_with_nat_detection(
        config: &NatDetectionConfig,
        listen_port: u16,
    ) -> Result<(Self, super::punching::NatDetectionReport)> {
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
        let engine = Self::from_socket(socket)?;
        Ok((engine, report))
    }

    pub(crate) fn from_socket(socket: UdpSocket) -> Result<Self> {
        let local_addr = socket.local_addr()?;
        tracing::info!("P2P 引擎监听地址: {}", local_addr);

        let socket = Arc::new(socket);
        let connections = Arc::new(RwLock::new(HashMap::new()));
        let ip_index = Arc::new(RwLock::new(HashMap::new()));
        let local_ip = Arc::new(RwLock::new(None));
        let local_identity = Arc::new(RwLock::new(None));
        let authenticated_inbound = Arc::new(RwLock::new(AuthenticatedInboundCache::default()));
        let overlay = Arc::new(super::overlay::P2pOverlay::new(
            fuckipv6_common::NodeId::default(),
        ));
        let (incoming_tx, incoming_rx) = mpsc::unbounded_channel();

        spawn_udp_receiver(
            socket.clone(),
            connections.clone(),
            ip_index.clone(),
            local_ip.clone(),
            overlay.clone(),
            incoming_tx,
            authenticated_inbound.clone(),
        );

        Ok(Self {
            local_addr,
            socket,
            connections,
            ip_index,
            incoming_rx: Arc::new(RwLock::new(incoming_rx)),
            local_ip,
            local_identity,
            authenticated_inbound,
            fallback_threshold: Arc::new(RwLock::new(FallbackThreshold::default())),
            quic_mesh: Arc::new(RwLock::new(None)),
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

    /// 获取本地监听地址
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
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

    /// 启用端到端直连 QUIC P2P 网格能力
    pub async fn enable_quic_mesh(
        &self,
        bind_addr: SocketAddr,
    ) -> Result<fuckipv6_common::QuicCertificate> {
        let mesh = super::quic::QuicP2pMesh::new(bind_addr)?;
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
        )
        .with_context(|| format!("无法初始化与节点 {client_id} 的 Secure Session v2"))?;

        let connection = PeerConnection {
            peer_info: peer_info.clone(),
            active_endpoint: None,
            endpoints,
            health: PeerHealth::new(HEALTH_WINDOW_SIZE),
            secure: secure.clone(),
        };

        // 注册双栈 IP 路由映射
        {
            let mut ip_idx = self.ip_index.write().await;
            ip_idx.insert(IpAddr::V6(peer_info.virtual_ip), client_id.clone());
            if let Some(v6) = peer_info.virtual_ips.ipv6 {
                ip_idx.insert(IpAddr::V6(v6), client_id.clone());
            }
            if let Some(v4) = peer_info.virtual_ips.ipv4 {
                ip_idx.insert(IpAddr::V4(v4), client_id.clone());
            }
        }

        self.connections
            .write()
            .await
            .insert(client_id.clone(), connection);
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
                .get(&client_id)
                .map(|connection| connection.secure.clone())
        };
        let Some(secure) = secure else {
            anyhow::bail!("No connection to peer {dst_ip}");
        };

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
        let src_ip = src_ip.to_ip_addr();
        if let Some(plaintext) = self
            .authenticated_inbound
            .write()
            .await
            .take(src_ip, payload)
        {
            let client_id = {
                let idx = self.ip_index.read().await;
                idx.get(&src_ip).cloned()
            };
            if let Some(client_id) = client_id {
                record_peer_payload(&self.connections, &client_id, payload.len()).await;
                return Ok(plaintext);
            }
            anyhow::bail!("No registered peer for source {src_ip}");
        }

        let client_id = {
            let idx = self.ip_index.read().await;
            idx.get(&src_ip).cloned()
        };
        let Some(client_id) = client_id else {
            anyhow::bail!("No registered peer for source {src_ip}");
        };

        let secure = {
            let connections = self.connections.read().await;
            connections
                .get(&client_id)
                .map(|connection| connection.secure.clone())
        };
        let Some(secure) = secure else {
            anyhow::bail!("No registered peer for source {src_ip}");
        };

        let plaintext = secure
            .open_from_session(payload.to_vec())
            .await
            .with_context(|| format!("来自对等节点 {src_ip} 的数据包认证失败"))?;
        record_peer_payload(&self.connections, &client_id, payload.len()).await;
        Ok(plaintext)
    }

    /// 发送数据包到对等节点 (支持 IPv4 / IPv6)
    pub async fn send_to(
        &self,
        dst_ip: impl Into<IpAddr>,
        _src_ip: impl Into<IpAddr>,
        payload: &[u8],
    ) -> Result<()> {
        let dst_ip = dst_ip.into();

        let client_id = {
            let idx = self.ip_index.read().await;
            idx.get(&dst_ip).cloned()
        };

        let Some(client_id) = client_id else {
            anyhow::bail!("No connection to peer {}", dst_ip);
        };

        let secure = {
            let connections = self.connections.read().await;
            connections
                .get(&client_id)
                .map(|connection| connection.secure.clone())
        }
        .ok_or_else(|| anyhow::anyhow!("No connection to peer {dst_ip}"))?;

        let (destination, data) = secure
            .seal(payload)
            .await
            .context("对等节点尚无已确认的 Secure Session v2，拒绝发送数据")?;

        // QUIC is only a transport here; the same v2 ciphertext is required on
        // every path so relay or direct transport cannot downgrade protection.
        if let IpAddr::V6(dst_v6) = dst_ip {
            if let Some(mesh) = self.quic_mesh.read().await.as_ref() {
                if mesh.has_peer(&dst_v6).await && mesh.send_packet(&dst_v6, &data).await.is_ok() {
                    record_peer_payload(&self.connections, &client_id, data.len()).await;
                    return Ok(());
                }
            }
        }

        self.socket
            .send_to(&data, dual_stack_send_endpoint(destination))
            .await
            .with_context(|| format!("无法向对等节点 {dst_ip} 发送 Secure Session v2 数据包"))?;
        record_peer_payload(&self.connections, &client_id, data.len()).await;
        Ok(())
    }

    /// 接收数据包 (支持 IPv4 / IPv6)
    pub async fn recv(&self) -> Option<(IpAddr, Vec<u8>)> {
        self.incoming_rx.write().await.recv().await
    }

    /// 移除对等节点（通过客户端ID）
    pub async fn remove_peer_by_id(&self, client_id: &str) -> Option<IpAddr> {
        let mut connections = self.connections.write().await;

        if let Some(conn) = connections.remove(client_id) {
            let primary_ip = conn.peer_info.virtual_ips.primary_ip();
            let mut ip_idx = self.ip_index.write().await;
            ip_idx.remove(&IpAddr::V6(conn.peer_info.virtual_ip));
            if let Some(v6) = conn.peer_info.virtual_ips.ipv6 {
                ip_idx.remove(&IpAddr::V6(v6));
            }
            if let Some(v4) = conn.peer_info.virtual_ips.ipv4 {
                ip_idx.remove(&IpAddr::V4(v4));
            }
            tracing::info!("已注销对等节点: {} ({:?})", client_id, primary_ip);
            primary_ip.or(Some(IpAddr::V6(conn.peer_info.virtual_ip)))
        } else {
            None
        }
    }

    /// 将任一虚拟 IP（IPv4 或 IPv6）映射到已认证的客户端 ID。
    pub async fn client_id_for_ip(&self, ip: IpAddr) -> Option<String> {
        self.ip_index.read().await.get(&ip).cloned()
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
            .get(&client_id)
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
                .get(&id)
                .and_then(|conn| conn.active_endpoint)
                .is_some()
        } else {
            false
        }
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
                .get(&client_id)
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
            self.connections.read().await.get(&id).map(|conn| {
                conn.health
                    .snapshot(now_micros(), PROBE_RESPONSE_GRACE, &threshold)
            })
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
            connections.contains_key(target).then(|| target.to_string())
        }?;

        let src_ip = (*self.local_ip.read().await)?;
        let threshold = self.fallback_threshold.read().await.clone();
        let (peer_info, endpoints, active_endpoint, before_acked) = {
            let connections = self.connections.read().await;
            let conn = connections.get(&client_id)?;
            let health = conn
                .health
                .snapshot(now_micros(), PROBE_RESPONSE_GRACE, &threshold);
            (
                conn.peer_info.clone(),
                conn.endpoints.clone(),
                conn.active_endpoint,
                health.probes_acked,
            )
        };

        let ping = P2pDatagram::ping(src_ip);
        let timestamp = match ping {
            P2pDatagram::Ping { timestamp, .. } => timestamp,
            _ => return None,
        };
        let data = codec::encode(&ping).ok()?;

        record_peer_probe_sent(&self.connections, &client_id, timestamp).await;
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
                    .get(&client_id)
                    .and_then(|conn| conn.active_endpoint)
                    .or(active_endpoint);
                return Some(PeerProbeOutcome {
                    client_id,
                    virtual_ip: peer_info.virtual_ip,
                    endpoint: active_endpoint.or_else(|| endpoints.first().copied()),
                    rtt_us: health.rtt_us,
                    p2p_active: active_endpoint.is_some(),
                    timed_out: false,
                });
            }

            if Instant::now() >= deadline {
                return Some(PeerProbeOutcome {
                    client_id,
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

            if secure.is_established().await {
                return;
            }

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
                Ok(None) => return,
                Err(error) => {
                    tracing::warn!("无法为节点 {} 启动 Secure Session v2: {error}", client_id);
                }
            }

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

fn spawn_udp_receiver(
    socket: Arc<UdpSocket>,
    connections: Arc<RwLock<HashMap<String, PeerConnection>>>,
    ip_index: Arc<RwLock<HashMap<IpAddr, String>>>,
    local_ip: Arc<RwLock<Option<IpAddr>>>,
    overlay: Arc<super::overlay::P2pOverlay>,
    incoming_tx: mpsc::UnboundedSender<IncomingPacket>,
    authenticated_inbound: Arc<RwLock<AuthenticatedInboundCache>>,
) {
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        loop {
            match socket.recv_from(&mut buf).await {
                Ok((size, addr)) => {
                    dispatch_udp_packet(
                        &socket,
                        &connections,
                        &ip_index,
                        &local_ip,
                        &overlay,
                        &incoming_tx,
                        &authenticated_inbound,
                        addr,
                        &buf[..size],
                    )
                    .await;
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
async fn dispatch_udp_packet(
    socket: &UdpSocket,
    connections: &Arc<RwLock<HashMap<String, PeerConnection>>>,
    ip_index: &Arc<RwLock<HashMap<IpAddr, String>>>,
    local_ip: &Arc<RwLock<Option<IpAddr>>>,
    overlay: &Arc<super::overlay::P2pOverlay>,
    incoming_tx: &mpsc::UnboundedSender<IncomingPacket>,
    authenticated_inbound: &Arc<RwLock<AuthenticatedInboundCache>>,
    addr: SocketAddr,
    bytes: &[u8],
) {
    if bytes.starts_with(&CONTROL_MAGIC) {
        match decode_control(bytes) {
            Ok(message) => {
                dispatch_secure_control(socket, connections, addr, message).await;
            }
            Err(error) => {
                tracing::debug!("丢弃来自 {addr} 的非法 Secure Session v2 控制帧: {error}");
            }
        }
        return;
    }

    if is_data_packet(bytes) {
        dispatch_secure_data(connections, authenticated_inbound, incoming_tx, addr, bytes).await;
        return;
    }

    if let Ok(datagram) = codec::decode::<P2pDatagram>(bytes) {
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
    } else if let Ok(packet) = codec::decode::<DataPacket>(bytes) {
        dispatch_legacy_data_packet(connections, ip_index, incoming_tx, addr, packet).await;
    } else {
        tracing::warn!("无法反序列化来自 {} 的数据包", addr);
    }
}

async fn dispatch_secure_control(
    socket: &UdpSocket,
    connections: &Arc<RwLock<HashMap<String, PeerConnection>>>,
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

    match secure.handle_control(addr, message).await {
        Ok(actions) => {
            update_peer_endpoint(connections, &client_id, addr).await;
            record_peer_inbound(connections, &client_id).await;
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
    authenticated_inbound: &Arc<RwLock<AuthenticatedInboundCache>>,
    incoming_tx: &mpsc::UnboundedSender<IncomingPacket>,
    addr: SocketAddr,
    bytes: &[u8],
) {
    let candidates = {
        let connections = connections.read().await;
        connections
            .iter()
            .filter(|(_, connection)| connection.secure.handles_data_packet(bytes))
            .map(|(client_id, connection)| {
                let src_ip = connection
                    .peer_info
                    .virtual_ips
                    .primary_ip()
                    .unwrap_or(IpAddr::V6(connection.peer_info.virtual_ip));
                (client_id.clone(), connection.secure.clone(), src_ip)
            })
            .collect::<Vec<_>>()
    };

    if candidates.is_empty() {
        tracing::debug!("丢弃来自 {addr} 的、无匹配 receiver index 的 Secure Session v2 数据帧");
        return;
    }

    for (client_id, secure, src_ip) in candidates {
        match secure.open_from_source(addr, bytes.to_vec()).await {
            Ok(plaintext) => {
                // Authentication happens before endpoint migration. A spoofed,
                // replayed, or tampered datagram must not steer future traffic.
                update_peer_endpoint(connections, &client_id, addr).await;
                authenticated_inbound
                    .write()
                    .await
                    .insert(src_ip, bytes, plaintext);
                let _ = incoming_tx.send((src_ip, bytes.to_vec()));
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
    _ip_index: &Arc<RwLock<HashMap<IpAddr, String>>>,
    _incoming_tx: &mpsc::UnboundedSender<IncomingPacket>,
    addr: SocketAddr,
    _packet: DataPacket,
) {
    tracing::warn!("丢弃来自 {addr} 的未认证旧版数据包");
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_p2p_datagram(
    socket: &UdpSocket,
    connections: &Arc<RwLock<HashMap<String, PeerConnection>>>,
    ip_index: &Arc<RwLock<HashMap<IpAddr, String>>>,
    local_ip: &Arc<RwLock<Option<IpAddr>>>,
    overlay: &Arc<super::overlay::P2pOverlay>,
    incoming_tx: &mpsc::UnboundedSender<IncomingPacket>,
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
                if peer_accepts_endpoint(connections, id, addr).await {
                    update_peer_endpoint(connections, id, addr).await;
                    handle_control_message(connections, id, message).await;
                }
            }
        }
        P2pDatagram::Ping { src_ip, timestamp } => {
            let client_id = {
                let idx = ip_index.read().await;
                idx.get(&src_ip).cloned()
            };
            if let Some(ref id) = client_id {
                if peer_accepts_probe_source(connections, id, addr).await {
                    handle_ping_datagram(
                        socket,
                        connections,
                        local_ip,
                        addr,
                        id,
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
                if peer_accepts_probe_source(connections, id, addr).await {
                    handle_pong_datagram(
                        socket,
                        connections,
                        local_ip,
                        addr,
                        id,
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

async fn record_peer_inbound(
    connections: &Arc<RwLock<HashMap<String, PeerConnection>>>,
    client_id: &str,
) {
    let mut connections = connections.write().await;
    if let Some(conn) = connections.get_mut(client_id) {
        conn.health.record_inbound();
    }
}

async fn record_peer_payload(
    connections: &Arc<RwLock<HashMap<String, PeerConnection>>>,
    client_id: &str,
    bytes: usize,
) {
    let mut connections = connections.write().await;
    if let Some(conn) = connections.get_mut(client_id) {
        conn.health.record_payload(bytes);
    }
}

async fn record_peer_probe_sent(
    connections: &Arc<RwLock<HashMap<String, PeerConnection>>>,
    client_id: &str,
    timestamp: u64,
) {
    let mut connections = connections.write().await;
    if let Some(conn) = connections.get_mut(client_id) {
        conn.health.record_probe_sent(timestamp);
    }
}

async fn record_peer_probe_response(
    connections: &Arc<RwLock<HashMap<String, PeerConnection>>>,
    client_id: &str,
    timestamp: u64,
    rtt_us: u64,
) {
    let mut connections = connections.write().await;
    if let Some(conn) = connections.get_mut(client_id) {
        conn.health.record_probe_response(timestamp, rtt_us);
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
            let mut connections = connections.write().await;
            if let Some(conn) = connections.get_mut(client_id) {
                conn.health
                    .record_remote_quality(rtt_us, loss_rate, bandwidth_bps);
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
        connections.get(client_id).map(|conn| {
            conn.health.snapshot(
                now_micros(),
                PROBE_RESPONSE_GRACE,
                &FallbackThreshold::default(),
            )
        })
    }) else {
        return;
    };

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

        let payload = vec![0x60, 0, 0, 0, 0, 0, 0, 0];
        engine_a.send_to(ip_b, ip_a, &payload).await?;

        let received = tokio::time::timeout(Duration::from_secs(1), engine_b.recv())
            .await?
            .expect("engine B should receive P2P payload");

        assert_eq!(received.0, IpAddr::V6(ip_a));
        assert_ne!(received.1, payload, "data plane must not expose plaintext");
        assert_eq!(
            engine_b.decrypt_from_peer(ip_a, &received.1).await?,
            payload
        );
        assert!(
            engine_b.decrypt_from_peer(ip_a, &payload).await.is_err(),
            "unauthenticated plaintext must be rejected"
        );

        let mut tampered = received.1.clone();
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
            .expect("engine B should receive rebound ciphertext");
        assert_eq!(received.0, IpAddr::V6(ip_a));
        assert_eq!(received.1, packet);
        assert_eq!(
            engine_b.decrypt_from_peer(ip_a, &received.1).await?,
            payload
        );
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
