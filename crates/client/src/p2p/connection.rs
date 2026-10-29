use anyhow::{Context, Result};
use fuckipv6_common::crypto::Identity;
use fuckipv6_common::{
    codec, ControlMessage, DataPacket, FallbackThreshold, P2pDatagram, PeerInfo,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, RwLock};
use tokio::time::{sleep, Duration, Instant};

use super::health::{ConnectionHealthSnapshot, PeerHealth};

const HEALTH_WINDOW_SIZE: usize = 20;
const PROBE_RESPONSE_GRACE: Duration = Duration::from_secs(3);
const FAST_PROBE_ATTEMPTS: usize = 20;
const FAST_PROBE_INTERVAL: Duration = Duration::from_millis(500);
const HEALTH_PROBE_INTERVAL: Duration = Duration::from_secs(30);

type IncomingPacket = (IpAddr, Vec<u8>);
type IncomingReceiver = mpsc::UnboundedReceiver<IncomingPacket>;

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
    session_key: Option<[u8; 32]>,
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
    fallback_threshold: Arc<RwLock<FallbackThreshold>>,
    quic_mesh: Arc<RwLock<Option<super::quic::QuicP2pMesh>>>,
    pub overlay: Arc<super::overlay::P2pOverlay>,
}

impl P2pEngine {
    /// 创建P2P引擎
    pub async fn new(listen_port: u16) -> Result<Self> {
        let socket = bind_dual_stack_udp(listen_port).context("无法绑定 UDP socket")?;
        let local_addr = socket.local_addr()?;
        tracing::info!("P2P 引擎监听地址: {}", local_addr);

        let socket = Arc::new(socket);
        let connections = Arc::new(RwLock::new(HashMap::new()));
        let ip_index = Arc::new(RwLock::new(HashMap::new()));
        let local_ip = Arc::new(RwLock::new(None));
        let local_identity = Arc::new(RwLock::new(None));
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
        );

        Ok(Self {
            local_addr,
            socket,
            connections,
            ip_index,
            incoming_rx: Arc::new(RwLock::new(incoming_rx)),
            local_ip,
            local_identity,
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
        let endpoints =
            peer_info
                .endpoints
                .iter()
                .copied()
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

        let session_key = if let Some(local_identity) = &*self.local_identity.read().await {
            match local_identity.derive_shared_secret(&peer_info.public_key) {
                Ok(key) => {
                    tracing::info!(
                        "已与节点 {} 建立 WireGuard-grade E2EE 对称会话秘钥",
                        client_id
                    );
                    Some(key)
                }
                Err(e) => {
                    tracing::warn!("与节点 {} 秘钥协商失败: {e:#}", client_id);
                    None
                }
            }
        } else {
            None
        };

        let connection = PeerConnection {
            peer_info: peer_info.clone(),
            active_endpoint: None,
            endpoints,
            health: PeerHealth::new(HEALTH_WINDOW_SIZE),
            session_key,
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
        self.start_endpoint_probing(client_id, primary_ip);

        Ok(())
    }

    /// 对指定对等节点的载荷进行端到端加密（用于中继转发或测试）
    pub async fn encrypt_for_peer(
        &self,
        dst_ip: impl crate::stats::ToIpAddr,
        payload: &[u8],
    ) -> Vec<u8> {
        let dst_ip = dst_ip.to_ip_addr();
        let client_id = {
            let idx = self.ip_index.read().await;
            idx.get(&dst_ip).cloned()
        };

        if let Some(client_id) = client_id {
            let connections = self.connections.read().await;
            if let Some(conn) = connections.get(&client_id) {
                if let Some(key) = conn.session_key {
                    if let Ok(encrypted) = fuckipv6_common::crypto::encrypt_payload(&key, payload) {
                        return encrypted;
                    }
                }
            }
        }
        payload.to_vec()
    }

    /// 对来自指定对等节点的载荷进行端到端解密（用于中继接收或测试）
    pub async fn decrypt_from_peer(
        &self,
        src_ip: impl crate::stats::ToIpAddr,
        payload: &[u8],
    ) -> Vec<u8> {
        let src_ip = src_ip.to_ip_addr();
        let client_id = {
            let idx = self.ip_index.read().await;
            idx.get(&src_ip).cloned()
        };

        if let Some(client_id) = client_id {
            let connections = self.connections.read().await;
            if let Some(conn) = connections.get(&client_id) {
                if let Some(key) = conn.session_key {
                    if let Ok(decrypted) = fuckipv6_common::crypto::decrypt_payload(&key, payload) {
                        return decrypted;
                    }
                }
            }
        }
        payload.to_vec()
    }

    /// 发送数据包到对等节点 (支持 IPv4 / IPv6)
    pub async fn send_to(
        &self,
        dst_ip: impl Into<IpAddr>,
        src_ip: impl Into<IpAddr>,
        payload: &[u8],
    ) -> Result<()> {
        let dst_ip = dst_ip.into();
        let src_ip = src_ip.into();

        let client_id = {
            let idx = self.ip_index.read().await;
            idx.get(&dst_ip).cloned()
        };

        let Some(client_id) = client_id else {
            anyhow::bail!("No connection to peer {}", dst_ip);
        };

        let connections = self.connections.read().await;
        if let Some(conn) = connections.get(&client_id) {
            let endpoints = endpoint_attempt_order(conn);
            let has_active_endpoint = conn.active_endpoint.is_some();
            let threshold = self.fallback_threshold.read().await.clone();
            let p2p_healthy =
                conn.health
                    .is_healthy(now_micros(), PROBE_RESPONSE_GRACE, &threshold);
            let session_key = conn.session_key;
            drop(connections);

            let payload_to_send = if let Some(key) = session_key {
                fuckipv6_common::crypto::encrypt_payload(&key, payload)
                    .context("无法对出站数据包进行端到端加密")?
            } else {
                payload.to_vec()
            };

            // 优先尝试端到端 QUIC 直连通道 (如果对端为 IPv6 且已就绪)
            if let IpAddr::V6(dst_v6) = dst_ip {
                if let Some(mesh) = self.quic_mesh.read().await.as_ref() {
                    if mesh.has_peer(&dst_v6).await
                        && mesh.send_packet(&dst_v6, &payload_to_send).await.is_ok()
                    {
                        record_peer_payload(&self.connections, &client_id, payload_to_send.len())
                            .await;
                        return Ok(());
                    }
                }
            }

            if has_active_endpoint {
                if !p2p_healthy {
                    anyhow::bail!("P2P endpoint for peer {} is unhealthy", dst_ip);
                }
                let packet = DataPacket::new(src_ip, dst_ip, payload_to_send);
                let data = codec::encode(&P2pDatagram::Data(packet))?;
                send_to_first_available(&self.socket, &data, &endpoints).await?;
                record_peer_payload(&self.connections, &client_id, data.len()).await;
                Ok(())
            } else {
                let packet = DataPacket::new(src_ip, dst_ip, payload_to_send);
                let data = codec::encode(&P2pDatagram::Data(packet))?;
                send_to_all_candidates(&self.socket, &data, &endpoints).await?;
                anyhow::bail!("No validated P2P endpoint for peer {}", dst_ip)
            }
        } else {
            anyhow::bail!("No connection to peer {}", dst_ip)
        }
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
            let _ = self.socket.send_to(&data, endpoint).await;
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
                    let _ = socket.send_to(&data, endpoint).await;
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

fn spawn_udp_receiver(
    socket: Arc<UdpSocket>,
    connections: Arc<RwLock<HashMap<String, PeerConnection>>>,
    ip_index: Arc<RwLock<HashMap<IpAddr, String>>>,
    local_ip: Arc<RwLock<Option<IpAddr>>>,
    overlay: Arc<super::overlay::P2pOverlay>,
    incoming_tx: mpsc::UnboundedSender<IncomingPacket>,
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
    addr: SocketAddr,
    bytes: &[u8],
) {
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

async fn dispatch_legacy_data_packet(
    connections: &Arc<RwLock<HashMap<String, PeerConnection>>>,
    ip_index: &Arc<RwLock<HashMap<IpAddr, String>>>,
    incoming_tx: &mpsc::UnboundedSender<IncomingPacket>,
    addr: SocketAddr,
    packet: DataPacket,
) {
    let client_id = {
        let idx = ip_index.read().await;
        idx.get(&packet.src_ip).cloned()
    };
    if let Some(ref id) = client_id {
        update_peer_endpoint(connections, id, addr).await;
        record_peer_payload(connections, id, packet.payload.len()).await;
    }
    let _ = incoming_tx.send((packet.src_ip, packet.payload));
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
        P2pDatagram::Data(packet) => {
            let client_id = {
                let idx = ip_index.read().await;
                idx.get(&packet.src_ip).cloned()
            };
            if let Some(ref id) = client_id {
                update_peer_endpoint(connections, id, addr).await;
                record_peer_payload(connections, id, packet.payload.len()).await;
            }
            tracing::debug!(
                "Received packet from {} -> {}",
                packet.src_ip,
                packet.dst_ip
            );
            let session_key = {
                if let Some(ref id) = client_id {
                    let conns = connections.read().await;
                    conns.get(id).and_then(|c| c.session_key)
                } else {
                    None
                }
            };
            let payload = if let Some(key) = session_key {
                fuckipv6_common::crypto::decrypt_payload(&key, &packet.payload)
                    .unwrap_or(packet.payload)
            } else {
                packet.payload
            };
            let _ = incoming_tx.send((packet.src_ip, payload));
        }
        P2pDatagram::Control { src_ip, message } => {
            let client_id = {
                let idx = ip_index.read().await;
                idx.get(&src_ip).cloned()
            };
            if let Some(ref id) = client_id {
                update_peer_endpoint(connections, id, addr).await;
                handle_control_message(connections, id, message).await;
            }
        }
        P2pDatagram::Ping { src_ip, timestamp } => {
            let client_id = {
                let idx = ip_index.read().await;
                idx.get(&src_ip).cloned()
            };
            if let Some(ref id) = client_id {
                handle_ping_datagram(socket, connections, local_ip, addr, id, src_ip, timestamp)
                    .await;
            }
        }
        P2pDatagram::Pong { src_ip, timestamp } => {
            let client_id = {
                let idx = ip_index.read().await;
                idx.get(&src_ip).cloned()
            };
            if let Some(ref id) = client_id {
                handle_pong_datagram(socket, connections, local_ip, addr, id, src_ip, timestamp)
                    .await;
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
            let _ = socket.send_to(&data, addr).await;
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
        let _ = socket.send_to(&data, endpoint).await;
    }
}

fn endpoint_attempt_order(conn: &PeerConnection) -> Vec<SocketAddr> {
    let mut endpoints = Vec::with_capacity(conn.endpoints.len());

    if let Some(active) = conn.active_endpoint {
        endpoints.push(active);
    }

    for endpoint in &conn.endpoints {
        if !endpoints.contains(endpoint) {
            endpoints.push(*endpoint);
        }
    }

    endpoints
}

async fn send_to_first_available(
    socket: &UdpSocket,
    data: &[u8],
    endpoints: &[SocketAddr],
) -> Result<()> {
    let mut last_error = None;

    for endpoint in endpoints {
        match socket.send_to(data, endpoint).await {
            Ok(_) => return Ok(()),
            Err(e) => {
                last_error = Some(e);
            }
        }
    }

    if let Some(e) = last_error {
        Err(e).context("无法发送 UDP 数据包到任何候选端点")
    } else {
        anyhow::bail!("没有可用的候选端点")
    }
}

async fn send_to_all_candidates(
    socket: &UdpSocket,
    data: &[u8],
    endpoints: &[SocketAddr],
) -> Result<()> {
    let mut sent = false;
    let mut last_error = None;

    for endpoint in endpoints {
        match socket.send_to(data, endpoint).await {
            Ok(_) => {
                sent = true;
            }
            Err(e) => {
                last_error = Some(e);
            }
        }
    }

    if sent {
        Ok(())
    } else if let Some(e) = last_error {
        Err(e).context("无法发送 UDP 数据包到任何候选端点")
    } else {
        anyhow::bail!("没有可用的候选端点")
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
    async fn test_p2p_engine_data_transfer() -> Result<()> {
        let engine_a = P2pEngine::new(0).await?;
        let engine_b = P2pEngine::new(0).await?;

        let ip_a = Ipv6Addr::LOCALHOST;
        let ip_b = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 2);

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
                vec![2; 32],
                vec![endpoint_b],
                fuckipv6_common::NatType::Open,
            ))
            .await?;

        engine_b
            .add_peer(PeerInfo::new(
                "a".to_string(),
                ip_a,
                vec![1; 32],
                vec![endpoint_a],
                fuckipv6_common::NatType::Open,
            ))
            .await?;

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if engine_a.has_active_peer(&ip_b).await && engine_b.has_active_peer(&ip_a).await {
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
        assert_eq!(received.1, payload);

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
}
