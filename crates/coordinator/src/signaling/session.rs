use anyhow::{Context, Result};
use fuckipv6_common::{
    codec, BlacklistConfig, ClientId, NatType, NetworkAllocation, NetworkMode, PeerInfo,
    SignalingMessage, VirtualIps,
};
use serde::Serialize;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::tungstenite::Message;

/// 客户端会话
pub struct ClientSession {
    pub peer_info: PeerInfo,
    pub tx: mpsc::UnboundedSender<Message>,
    pub connected_at: Instant,
    pub last_heartbeat: Instant,
}

/// 会话管理器
pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<ClientId, ClientSession>>>,
    ip_allocator: Arc<RwLock<IpAllocator>>,
    leases: Arc<RwLock<HashMap<ClientId, VirtualIps>>>,
    traffic: Arc<TrafficCounters>,
    blacklist: Arc<RwLock<BlacklistConfig>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CoordinatorSnapshot {
    pub online_clients: usize,
    pub peers: Vec<PeerSnapshot>,
    pub traffic: TrafficSnapshot,
}

#[derive(Debug, Clone, Serialize)]
pub struct PeerSnapshot {
    pub client_id: ClientId,
    pub virtual_ip: Ipv6Addr,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub virtual_ipv4: Option<Ipv4Addr>,
    pub endpoints: Vec<SocketAddr>,
    pub nat_type: String,
    pub connected_secs: u64,
    pub idle_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub networks: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct TrafficSnapshot {
    pub uptime_secs: u64,
    pub signaling_rx_messages: u64,
    pub signaling_rx_bytes: u64,
    pub signaling_tx_messages: u64,
    pub signaling_tx_bytes: u64,
    pub relay_packets: u64,
    pub relay_payload_bytes: u64,
    pub relay_errors: u64,
}

struct TrafficCounters {
    started_at: Instant,
    signaling_rx_messages: AtomicU64,
    signaling_rx_bytes: AtomicU64,
    signaling_tx_messages: AtomicU64,
    signaling_tx_bytes: AtomicU64,
    relay_packets: AtomicU64,
    relay_payload_bytes: AtomicU64,
    relay_errors: AtomicU64,
}

struct RegistrationInput {
    client_id: ClientId,
    public_key: Vec<u8>,
    endpoint: SocketAddr,
    candidate_endpoints: Vec<SocketAddr>,
    nat_type: NatType,
    requested_ips: Option<VirtualIps>,
    tx: mpsc::UnboundedSender<Message>,
    now: Instant,
}

struct RegistrationResult {
    peer_info: PeerInfo,
    is_reconnect: bool,
}

impl TrafficCounters {
    fn new() -> Self {
        Self {
            started_at: Instant::now(),
            signaling_rx_messages: AtomicU64::new(0),
            signaling_rx_bytes: AtomicU64::new(0),
            signaling_tx_messages: AtomicU64::new(0),
            signaling_tx_bytes: AtomicU64::new(0),
            relay_packets: AtomicU64::new(0),
            relay_payload_bytes: AtomicU64::new(0),
            relay_errors: AtomicU64::new(0),
        }
    }

    fn snapshot(&self) -> TrafficSnapshot {
        TrafficSnapshot {
            uptime_secs: self.started_at.elapsed().as_secs(),
            signaling_rx_messages: self.signaling_rx_messages.load(Ordering::Relaxed),
            signaling_rx_bytes: self.signaling_rx_bytes.load(Ordering::Relaxed),
            signaling_tx_messages: self.signaling_tx_messages.load(Ordering::Relaxed),
            signaling_tx_bytes: self.signaling_tx_bytes.load(Ordering::Relaxed),
            relay_packets: self.relay_packets.load(Ordering::Relaxed),
            relay_payload_bytes: self.relay_payload_bytes.load(Ordering::Relaxed),
            relay_errors: self.relay_errors.load(Ordering::Relaxed),
        }
    }

    fn record_signaling_rx(&self, bytes: usize) {
        self.signaling_rx_messages.fetch_add(1, Ordering::Relaxed);
        self.signaling_rx_bytes
            .fetch_add(bytes as u64, Ordering::Relaxed);
    }

    fn record_signaling_tx(&self, bytes: usize) {
        self.record_signaling_tx_many(1, bytes as u64);
    }

    fn record_signaling_tx_many(&self, messages: u64, bytes_per_message: u64) {
        self.signaling_tx_messages
            .fetch_add(messages, Ordering::Relaxed);
        self.signaling_tx_bytes.fetch_add(
            messages.saturating_mul(bytes_per_message),
            Ordering::Relaxed,
        );
    }

    fn record_relay_forward(&self, payload_bytes: usize) {
        self.relay_packets.fetch_add(1, Ordering::Relaxed);
        self.relay_payload_bytes
            .fetch_add(payload_bytes as u64, Ordering::Relaxed);
    }

    fn record_relay_error(&self) {
        self.relay_errors.fetch_add(1, Ordering::Relaxed);
    }
}

fn consolidate_endpoints(primary: SocketAddr, candidates: &[SocketAddr]) -> Vec<SocketAddr> {
    let mut endpoints = vec![primary];
    for ep in candidates {
        if !endpoints.contains(ep) {
            endpoints.push(*ep);
        }
    }
    endpoints
}

fn refresh_session(session: &mut ClientSession, input: RegistrationInput) {
    session.peer_info.public_key = input.public_key;
    session.peer_info.endpoints = consolidate_endpoints(input.endpoint, &input.candidate_endpoints);
    session.peer_info.nat_type = input.nat_type;
    session.tx = input.tx;
    session.last_heartbeat = input.now;
}

fn build_client_session(input: &RegistrationInput, virtual_ips: VirtualIps) -> ClientSession {
    let virtual_ip = virtual_ips.primary_ipv6();
    let peer_info = PeerInfo {
        client_id: input.client_id.clone(),
        virtual_ip,
        virtual_ips,
        public_key: input.public_key.clone(),
        endpoints: consolidate_endpoints(input.endpoint, &input.candidate_endpoints),
        nat_type: input.nat_type,
    };

    ClientSession {
        peer_info,
        tx: input.tx.clone(),
        connected_at: input.now,
        last_heartbeat: input.now,
    }
}

impl SessionManager {
    /// 创建新的会话管理器
    pub fn new(ip_pool: &str) -> Result<Self> {
        let ip_allocator = IpAllocator::new(ip_pool)?;

        Ok(Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            ip_allocator: Arc::new(RwLock::new(ip_allocator)),
            leases: Arc::new(RwLock::new(HashMap::new())),
            traffic: Arc::new(TrafficCounters::new()),
            blacklist: Arc::new(RwLock::new(BlacklistConfig::default())),
        })
    }

    /// 使用完整网络分配配置创建会话管理器
    pub fn new_with_allocation(alloc: &NetworkAllocation) -> Result<Self> {
        Self::new_with_allocation_and_blacklist(alloc, BlacklistConfig::default())
    }

    /// 使用网络分配配置和初始黑名单配置创建会话管理器
    pub fn new_with_allocation_and_blacklist(
        alloc: &NetworkAllocation,
        blacklist: BlacklistConfig,
    ) -> Result<Self> {
        let ip_allocator = IpAllocator::new_with_allocation(alloc)?;

        Ok(Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            ip_allocator: Arc::new(RwLock::new(ip_allocator)),
            leases: Arc::new(RwLock::new(HashMap::new())),
            traffic: Arc::new(TrafficCounters::new()),
            blacklist: Arc::new(RwLock::new(blacklist)),
        })
    }

    /// 检查客户端 ID 或公钥是否在黑名单中
    pub async fn is_blacklisted(&self, client_id: &str, public_key: &[u8]) -> bool {
        let fingerprint = fuckipv6_common::Identity::public_key_fingerprint(public_key);
        self.blacklist
            .read()
            .await
            .is_banned(client_id, &fingerprint)
    }

    /// 将客户端加入黑名单，并强制下线如果当前在线
    pub async fn ban_client(&self, client_id: String) {
        let mut bl = self.blacklist.write().await;
        if !bl.banned_clients.contains(&client_id) {
            bl.banned_clients.push(client_id.clone());
        }
        drop(bl);

        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.remove(&client_id) {
            let _ = session.tx.send(Message::Close(None));
            tracing::warn!("黑名单客户端已强制断开并踢出: {}", client_id);
        }
    }

    /// 从黑名单移除客户端
    pub async fn unban_client(&self, client_id: &str) -> bool {
        let mut bl = self.blacklist.write().await;
        let original_len = bl.banned_clients.len();
        bl.banned_clients.retain(|item| item != client_id);
        bl.banned_clients.len() < original_len
    }

    /// 获取当前生效的黑名单列表
    pub async fn get_blacklist(&self) -> Vec<String> {
        self.blacklist.read().await.banned_clients.clone()
    }

    /// 获取当前网络分配配置
    pub async fn current_network_allocation(&self) -> NetworkAllocation {
        self.ip_allocator.read().await.allocation().clone()
    }

    /// 热更新网络分配配置
    pub async fn update_network_allocation(&self, alloc: NetworkAllocation) -> Result<()> {
        self.ip_allocator.write().await.update_allocation(&alloc)
    }

    pub async fn ipv4_prefix(&self) -> u8 {
        self.ip_allocator.read().await.ipv4_prefix()
    }

    pub async fn ipv6_prefix(&self) -> u8 {
        self.ip_allocator.read().await.ipv6_prefix()
    }

    /// 注册新客户端
    pub async fn register_client(
        &self,
        client_id: ClientId,
        public_key: Vec<u8>,
        observed_addr: SocketAddr,
        p2p_port: u16,
        nat_type: NatType,
        tx: mpsc::UnboundedSender<Message>,
    ) -> Result<VirtualIps> {
        self.register_client_with_candidates(
            client_id,
            public_key,
            observed_addr,
            p2p_port,
            nat_type,
            Vec::new(),
            None,
            tx,
        )
        .await
    }

    /// 注册新客户端并包含 ICE 候选端点列表
    #[allow(clippy::too_many_arguments)]
    pub async fn register_client_with_candidates(
        &self,
        client_id: ClientId,
        public_key: Vec<u8>,
        observed_addr: SocketAddr,
        p2p_port: u16,
        nat_type: NatType,
        candidate_endpoints: Vec<SocketAddr>,
        requested_ips: Option<VirtualIps>,
        tx: mpsc::UnboundedSender<Message>,
    ) -> Result<VirtualIps> {
        tracing::debug!("开始注册客户端: {}", client_id);

        let input = RegistrationInput {
            client_id,
            public_key,
            endpoint: SocketAddr::new(observed_addr.ip(), p2p_port),
            candidate_endpoints,
            nat_type,
            requested_ips,
            tx,
            now: Instant::now(),
        };
        let result = self.register_or_refresh_session(input).await?;

        if result.is_reconnect {
            tracing::info!(
                "客户端重新注册成功: {} (虚拟IP: {}, 双栈: {:?}, 真实地址: {})",
                result.peer_info.client_id,
                result.peer_info.virtual_ip,
                result.peer_info.virtual_ips,
                result.peer_info.endpoints[0]
            );
        } else {
            tracing::info!(
                "客户端注册成功: {} (虚拟IP: {}, 双栈: {:?}, 真实地址: {})",
                result.peer_info.client_id,
                result.peer_info.virtual_ip,
                result.peer_info.virtual_ips,
                result.peer_info.endpoints[0]
            );
        }

        // 首次注册表示上线，重复注册表示 endpoint/NAT 信息刷新。
        tracing::debug!("准备广播节点信息通知...");
        if let Err(err) = self.broadcast_peer_online(result.peer_info.clone()).await {
            tracing::warn!("广播节点信息通知失败: {err:#}");
        }
        tracing::debug!("广播完成");

        Ok(result.peer_info.virtual_ips)
    }

    async fn register_or_refresh_session(
        &self,
        input: RegistrationInput,
    ) -> Result<RegistrationResult> {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(&input.client_id) {
            tracing::warn!("客户端 {} 重复注册，更新现有会话", input.client_id);
            refresh_session(session, input);
            return Ok(RegistrationResult {
                peer_info: session.peer_info.clone(),
                is_reconnect: true,
            });
        }

        drop(sessions);
        self.insert_new_session(input).await
    }

    async fn insert_new_session(&self, input: RegistrationInput) -> Result<RegistrationResult> {
        tracing::debug!("准备分配虚拟IP...");
        let remembered = self.leases.read().await.get(&input.client_id).cloned();
        let requested = input.requested_ips.clone().or(remembered);
        let virtual_ips = self
            .ip_allocator
            .write()
            .await
            .allocate_preferred(requested.as_ref())?;
        tracing::debug!("虚拟IP分配成功: {:?}", virtual_ips);

        let session = build_client_session(&input, virtual_ips.clone());
        let mut sessions = self.sessions.write().await;
        match sessions.entry(input.client_id.clone()) {
            Entry::Vacant(entry) => {
                let peer_info = session.peer_info.clone();
                entry.insert(session);
                self.leases
                    .write()
                    .await
                    .insert(input.client_id.clone(), virtual_ips.clone());
                Ok(RegistrationResult {
                    peer_info,
                    is_reconnect: false,
                })
            }
            Entry::Occupied(mut entry) => {
                self.ip_allocator.write().await.release(&virtual_ips);
                let existing = entry.get_mut();
                refresh_session(existing, input);
                Ok(RegistrationResult {
                    peer_info: existing.peer_info.clone(),
                    is_reconnect: true,
                })
            }
        }
    }

    /// 注销客户端
    pub async fn unregister_client(&self, client_id: &ClientId) {
        tracing::debug!("开始注销客户端: {}", client_id);

        tracing::debug!("获取 sessions write 锁...");
        let session_opt = self.sessions.write().await.remove(client_id);
        tracing::debug!("sessions write 锁已释放");

        if let Some(session) = session_opt {
            // 释放IP
            tracing::debug!("获取 ip_allocator write 锁...");
            self.ip_allocator
                .write()
                .await
                .release(&session.peer_info.virtual_ips);
            tracing::debug!("ip_allocator write 锁已释放");

            tracing::info!("客户端已注销: {}", client_id);

            // 通知所有其他客户端节点下线
            tracing::debug!("准备广播节点下线通知...");
            if let Err(err) = self.broadcast_peer_offline(client_id.clone()).await {
                tracing::warn!("广播节点下线通知失败: {err:#}");
            }
            tracing::debug!("广播下线通知完成");
        }

        tracing::debug!("unregister_client 函数退出");
    }

    pub async fn unregister_client_if_sender_matches(
        &self,
        client_id: &ClientId,
        tx: &mpsc::UnboundedSender<Message>,
    ) {
        tracing::debug!("尝试按连接注销客户端: {}", client_id);

        let should_unregister = {
            let sessions = self.sessions.read().await;
            sessions
                .get(client_id)
                .map(|session| session.tx.same_channel(tx))
                .unwrap_or(false)
        };

        if should_unregister {
            self.unregister_client(client_id).await;
        } else {
            tracing::debug!("跳过旧连接注销，客户端已有更新会话: {}", client_id);
        }
    }

    /// 获取所有在线的对等节点
    pub async fn get_all_peers(&self) -> Vec<PeerInfo> {
        self.sessions
            .read()
            .await
            .values()
            .map(|s| s.peer_info.clone())
            .collect()
    }

    pub async fn snapshot(&self) -> CoordinatorSnapshot {
        let sessions = self.sessions.read().await;
        let mut peers: Vec<_> = sessions
            .values()
            .map(|session| {
                let default_net = "default".to_string();
                PeerSnapshot {
                    client_id: session.peer_info.client_id.clone(),
                    virtual_ip: session.peer_info.virtual_ip,
                    virtual_ipv4: session.peer_info.virtual_ips.ipv4,
                    endpoints: session.peer_info.endpoints.clone(),
                    nat_type: session.peer_info.nat_type.description().to_string(),
                    connected_secs: session.connected_at.elapsed().as_secs(),
                    idle_secs: session.last_heartbeat.elapsed().as_secs(),
                    network_name: Some(default_net.clone()),
                    networks: vec![default_net],
                }
            })
            .collect();

        peers.sort_by_key(|peer| peer.virtual_ip);

        CoordinatorSnapshot {
            online_clients: peers.len(),
            peers,
            traffic: self.traffic.snapshot(),
        }
    }

    pub fn record_signaling_rx(&self, bytes: usize) {
        self.traffic.record_signaling_rx(bytes);
    }

    pub fn record_signaling_tx(&self, bytes: usize) {
        self.traffic.record_signaling_tx(bytes);
    }

    pub fn record_relay_forward(&self, payload_bytes: usize) {
        self.traffic.record_relay_forward(payload_bytes);
    }

    pub fn record_relay_error(&self) {
        self.traffic.record_relay_error();
    }

    /// 获取特定客户端的对等节点信息
    pub async fn get_peer_info(&self, client_id: &ClientId) -> Option<PeerInfo> {
        self.sessions
            .read()
            .await
            .get(client_id)
            .map(|s| s.peer_info.clone())
    }

    pub async fn record_heartbeat(&self, client_id: &ClientId) -> bool {
        let mut sessions = self.sessions.write().await;
        let Some(session) = sessions.get_mut(client_id) else {
            return false;
        };

        session.last_heartbeat = Instant::now();
        true
    }

    pub async fn update_nat_type(&self, client_id: &ClientId, nat_type: NatType) -> bool {
        let mut sessions = self.sessions.write().await;
        let Some(session) = sessions.get_mut(client_id) else {
            return false;
        };

        session.peer_info.nat_type = nat_type;
        true
    }

    pub async fn cleanup_stale_sessions(&self, timeout: Duration) -> Vec<ClientId> {
        let now = Instant::now();
        let stale_clients = {
            let sessions = self.sessions.read().await;
            sessions
                .iter()
                .filter(|(_, session)| now.duration_since(session.last_heartbeat) > timeout)
                .map(|(client_id, _)| client_id.clone())
                .collect::<Vec<_>>()
        };

        for client_id in &stale_clients {
            tracing::warn!("客户端心跳超时，注销: {}", client_id);
            self.unregister_client(client_id).await;
        }

        stale_clients
    }

    /// 向特定客户端发送消息
    pub async fn send_to_client(&self, client_id: &ClientId, msg: SignalingMessage) -> Result<()> {
        if let Some(session) = self.sessions.read().await.get(client_id) {
            let data = codec::encode(&msg).context("序列化定向信令消息失败")?;
            let bytes = data.len();
            session
                .tx
                .send(Message::Binary(data.into()))
                .with_context(|| format!("发送信令消息到客户端失败: {client_id}"))?;
            self.record_signaling_tx(bytes);
        }
        Ok(())
    }

    /// 向指定虚拟 IP 的客户端发送消息 (支持 IPv4 / IPv6)
    pub async fn send_to_virtual_ip(
        &self,
        virtual_ip: IpAddr,
        msg: SignalingMessage,
    ) -> Result<()> {
        let sessions = self.sessions.read().await;
        if let Some(session) = sessions
            .values()
            .find(|session| session.peer_info.matches_ip(virtual_ip))
        {
            let data = codec::encode(&msg).context("序列化虚拟 IP 信令消息失败")?;
            let bytes = data.len();
            session
                .tx
                .send(Message::Binary(data.into()))
                .with_context(|| format!("发送信令消息到虚拟 IP 失败: {virtual_ip}"))?;
            self.record_signaling_tx(bytes);
        } else {
            anyhow::bail!("目标虚拟 IP 不在线: {}", virtual_ip);
        }

        Ok(())
    }

    /// 广播消息给除了指定客户端外的所有客户端
    pub async fn broadcast_except(&self, exclude: &ClientId, msg: SignalingMessage) -> Result<()> {
        tracing::debug!("broadcast_except: 获取 sessions read 锁...");
        let sessions = self.sessions.read().await;
        tracing::debug!(
            "broadcast_except: 已获取 read 锁，会话数: {}",
            sessions.len()
        );

        let data = codec::encode(&msg).context("序列化广播信令消息失败")?;
        let bytes = data.len() as u64;
        let mut sent = 0u64;

        for (id, session) in sessions.iter() {
            if id != exclude
                && session
                    .tx
                    .send(Message::Binary(data.clone().into()))
                    .is_ok()
            {
                sent += 1;
            }
        }

        if sent > 0 {
            self.traffic.record_signaling_tx_many(sent, bytes);
        }

        tracing::debug!("broadcast_except: 准备释放 read 锁");
        Ok(())
    } // read 锁在这里自动释放

    /// 广播节点上线消息
    async fn broadcast_peer_online(&self, peer: PeerInfo) -> Result<()> {
        let excluded_id = peer.client_id.clone();
        let msg = SignalingMessage::PeerOnline { peer };
        self.broadcast_except(&excluded_id, msg).await
    }

    /// 广播节点下线消息
    async fn broadcast_peer_offline(&self, client_id: ClientId) -> Result<()> {
        let msg = SignalingMessage::PeerOffline {
            client_id: client_id.clone(),
        };
        self.broadcast_except(&client_id, msg).await
    }
}

struct Ipv4SubnetAllocator {
    base: Ipv4Addr,
    prefix_len: u8,
    next_offset: u32,
    allocated: HashMap<Ipv4Addr, bool>,
}

impl Ipv4SubnetAllocator {
    fn new(pool: &str) -> Result<Self> {
        let (ip_str, prefix_str) = pool
            .split_once('/')
            .ok_or_else(|| anyhow::anyhow!("Invalid IPv4 pool format: {}", pool))?;
        let base: Ipv4Addr = ip_str
            .parse()
            .with_context(|| format!("Invalid IPv4 pool base address: {}", ip_str))?;
        let prefix_len: u8 = prefix_str
            .parse()
            .with_context(|| format!("Invalid IPv4 pool prefix length: {}", prefix_str))?;

        if prefix_len > 30 {
            anyhow::bail!(
                "IPv4 prefix length must be <= 30 for host allocation, got {}",
                prefix_len
            );
        }

        Ok(Self {
            base,
            prefix_len,
            next_offset: 1,
            allocated: HashMap::new(),
        })
    }

    fn max_hosts(&self) -> u32 {
        let host_bits = 32u8.saturating_sub(self.prefix_len);
        if host_bits <= 1 {
            return 0;
        }
        (1u32 << host_bits).saturating_sub(2)
    }

    fn network_mask(&self) -> u32 {
        if self.prefix_len == 0 {
            0
        } else {
            !((1u32 << (32 - self.prefix_len)) - 1)
        }
    }

    fn offset_to_ip(&self, offset: u32) -> Ipv4Addr {
        let net_base = u32::from(self.base) & self.network_mask();
        Ipv4Addr::from(net_base | offset)
    }

    fn reserve(&mut self, ip: Ipv4Addr) -> bool {
        let mask = self.network_mask();
        let net_base = u32::from(self.base) & mask;
        let offset = u32::from(ip) & !mask;
        if u32::from(ip) & mask != net_base || !(1..=self.max_hosts()).contains(&offset) {
            return false;
        }

        match self.allocated.entry(ip) {
            Entry::Vacant(entry) => {
                entry.insert(true);
                true
            }
            Entry::Occupied(_) => false,
        }
    }

    fn allocate(&mut self) -> Result<Ipv4Addr> {
        let max_hosts = self.max_hosts();
        if max_hosts == 0 {
            anyhow::bail!("IPv4 pool has no usable host addresses");
        }

        let start_offset = self.next_offset;
        loop {
            let ip = self.offset_to_ip(self.next_offset);
            self.next_offset = if self.next_offset >= max_hosts {
                1
            } else {
                self.next_offset + 1
            };

            if let Entry::Vacant(entry) = self.allocated.entry(ip) {
                entry.insert(true);
                return Ok(ip);
            }

            if self.next_offset == start_offset {
                anyhow::bail!("IPv4 pool exhausted");
            }
        }
    }

    fn release(&mut self, ip: Ipv4Addr) {
        self.allocated.remove(&ip);
    }
}

struct Ipv6SubnetAllocator {
    base: Ipv6Addr,
    prefix_len: u8,
    next_offset: u32,
    allocated: HashMap<Ipv6Addr, bool>,
}

impl Ipv6SubnetAllocator {
    fn new(pool: &str) -> Result<Self> {
        let (ip_str, prefix_str) = pool
            .split_once('/')
            .ok_or_else(|| anyhow::anyhow!("Invalid IPv6 pool format: {}", pool))?;
        let base: Ipv6Addr = ip_str
            .parse()
            .with_context(|| format!("Invalid IPv6 pool base address: {}", ip_str))?;
        let prefix_len: u8 = prefix_str
            .parse()
            .with_context(|| format!("Invalid IPv6 pool prefix length: {}", prefix_str))?;

        if prefix_len > 127 {
            anyhow::bail!("IPv6 prefix length must be <= 127, got {}", prefix_len);
        }

        Ok(Self {
            base,
            prefix_len,
            next_offset: 1,
            allocated: HashMap::new(),
        })
    }

    fn max_offsets(&self) -> u32 {
        let host_bits = 128u8.saturating_sub(self.prefix_len);
        if host_bits == 0 {
            0
        } else if host_bits >= 32 {
            u32::MAX
        } else {
            (1u32 << host_bits) - 1
        }
    }

    fn offset_to_ip(&self, offset: u32) -> Ipv6Addr {
        let segments = self.base.segments();
        let mut new_segments = segments;
        let base_low = ((segments[6] as u32) << 16) | (segments[7] as u32);
        let new_low = base_low.wrapping_add(offset);
        new_segments[6] = (new_low >> 16) as u16;
        new_segments[7] = (new_low & 0xFFFF) as u16;

        Ipv6Addr::new(
            new_segments[0],
            new_segments[1],
            new_segments[2],
            new_segments[3],
            new_segments[4],
            new_segments[5],
            new_segments[6],
            new_segments[7],
        )
    }

    fn reserve(&mut self, ip: Ipv6Addr) -> bool {
        let prefix_len = self.prefix_len as u32;
        let mask = if prefix_len == 0 {
            0
        } else {
            u128::MAX << (128 - prefix_len)
        };
        let net_base = u128::from(self.base) & mask;
        let offset = u128::from(ip) & !mask;
        if u128::from(ip) & mask != net_base
            || offset == 0
            || offset > u128::from(self.max_offsets())
        {
            return false;
        }

        match self.allocated.entry(ip) {
            Entry::Vacant(entry) => {
                entry.insert(true);
                true
            }
            Entry::Occupied(_) => false,
        }
    }

    fn allocate(&mut self) -> Result<Ipv6Addr> {
        let max_offsets = self.max_offsets();
        if max_offsets == 0 {
            anyhow::bail!("IPv6 pool has no usable host addresses");
        }

        let start_offset = self.next_offset;
        loop {
            let ip = self.offset_to_ip(self.next_offset);
            self.next_offset = if self.next_offset >= max_offsets {
                1
            } else {
                self.next_offset + 1
            };

            if let Entry::Vacant(entry) = self.allocated.entry(ip) {
                entry.insert(true);
                return Ok(ip);
            }

            if self.next_offset == start_offset {
                anyhow::bail!("IPv6 pool exhausted");
            }
        }
    }

    fn release(&mut self, ip: Ipv6Addr) {
        self.allocated.remove(&ip);
    }
}

/// 双栈 IP 地址分配器
pub struct IpAllocator {
    allocation: NetworkAllocation,
    v4_allocator: Option<Ipv4SubnetAllocator>,
    v6_allocator: Option<Ipv6SubnetAllocator>,
}

impl IpAllocator {
    pub fn new(pool: &str) -> Result<Self> {
        let alloc = NetworkAllocation {
            mode: NetworkMode::DualStack,
            ipv4_pool: "10.8.0.0/24".to_string(),
            ipv6_pool: pool.to_string(),
            ip_pool: Some(pool.to_string()),
        };
        Self::new_with_allocation(&alloc)
    }

    pub fn new_with_allocation(alloc: &NetworkAllocation) -> Result<Self> {
        let v4_allocator = if alloc.mode.is_ipv4_enabled() {
            Some(Ipv4SubnetAllocator::new(&alloc.ipv4_pool)?)
        } else {
            None
        };

        let v6_allocator = if alloc.mode.is_ipv6_enabled() {
            Some(Ipv6SubnetAllocator::new(alloc.effective_ipv6_pool())?)
        } else {
            None
        };

        Ok(Self {
            allocation: alloc.clone(),
            v4_allocator,
            v6_allocator,
        })
    }

    pub fn update_allocation(&mut self, new_alloc: &NetworkAllocation) -> Result<()> {
        let v4_allocator = if new_alloc.mode.is_ipv4_enabled() {
            Some(Ipv4SubnetAllocator::new(&new_alloc.ipv4_pool)?)
        } else {
            None
        };

        let v6_allocator = if new_alloc.mode.is_ipv6_enabled() {
            Some(Ipv6SubnetAllocator::new(new_alloc.effective_ipv6_pool())?)
        } else {
            None
        };

        self.allocation = new_alloc.clone();
        self.v4_allocator = v4_allocator;
        self.v6_allocator = v6_allocator;
        Ok(())
    }

    pub fn allocate(&mut self) -> Result<VirtualIps> {
        self.allocate_preferred(None)
    }

    /// Allocate the requested addresses when they are valid and free. If a
    /// request cannot be honored, fall back to a fresh address so the client
    /// can detect the assignment change and fail closed instead of silently
    /// forwarding traffic from a stale TUN address.
    pub fn allocate_preferred(&mut self, requested: Option<&VirtualIps>) -> Result<VirtualIps> {
        let ipv4 = match (&mut self.v4_allocator, requested.and_then(|ips| ips.ipv4)) {
            (Some(allocator), Some(ip)) => {
                if allocator.reserve(ip) {
                    Some(ip)
                } else {
                    Some(allocator.allocate()?)
                }
            }
            (Some(allocator), None) => Some(allocator.allocate()?),
            (None, _) => None,
        };

        let ipv6 = match (&mut self.v6_allocator, requested.and_then(|ips| ips.ipv6)) {
            (Some(allocator), Some(ip)) => {
                if allocator.reserve(ip) {
                    Some(ip)
                } else {
                    match allocator.allocate() {
                        Ok(ip) => Some(ip),
                        Err(error) => {
                            if let (Some(allocator), Some(ipv4)) = (&mut self.v4_allocator, ipv4) {
                                allocator.release(ipv4);
                            }
                            return Err(error);
                        }
                    }
                }
            }
            (Some(allocator), None) => match allocator.allocate() {
                Ok(ip) => Some(ip),
                Err(error) => {
                    if let (Some(allocator), Some(ipv4)) = (&mut self.v4_allocator, ipv4) {
                        allocator.release(ipv4);
                    }
                    return Err(error);
                }
            },
            (None, _) => None,
        };

        if ipv4.is_none() && ipv6.is_none() {
            anyhow::bail!("未启用任何 IP 分配模式");
        }

        Ok(VirtualIps::new(ipv4, ipv6))
    }

    pub fn release(&mut self, ips: &VirtualIps) {
        if let (Some(v4), Some(ip)) = (&mut self.v4_allocator, ips.ipv4) {
            v4.release(ip);
        }
        if let (Some(v6), Some(ip)) = (&mut self.v6_allocator, ips.ipv6) {
            v6.release(ip);
        }
    }

    pub fn ipv4_prefix(&self) -> u8 {
        self.v4_allocator
            .as_ref()
            .map(|v4| v4.prefix_len)
            .unwrap_or(24)
    }

    pub fn ipv6_prefix(&self) -> u8 {
        self.v6_allocator
            .as_ref()
            .map(|v6| v6.prefix_len)
            .unwrap_or(64)
    }

    pub fn allocation(&self) -> &NetworkAllocation {
        &self.allocation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ip_allocator() -> Result<()> {
        let mut allocator = IpAllocator::new("fd00::1000/120")?;

        let ips1 = allocator.allocate()?;
        let ips2 = allocator.allocate()?;

        assert_ne!(ips1, ips2);

        allocator.release(&ips1);
        let ips3 = allocator.allocate()?;
        assert_ne!(ips2, ips3);
        Ok(())
    }

    #[test]
    fn test_dual_stack_and_ipv4_allocator() -> Result<()> {
        let mut alloc_cfg = NetworkAllocation {
            mode: NetworkMode::Ipv4Only,
            ipv4_pool: "192.168.1.0/24".to_string(),
            ipv6_pool: "fd00::/64".to_string(),
            ip_pool: None,
        };
        let mut allocator = IpAllocator::new_with_allocation(&alloc_cfg)?;
        let ips = allocator.allocate()?;
        assert!(ips.ipv4.is_some());
        assert!(ips.ipv6.is_none());
        assert_eq!(allocator.ipv4_prefix(), 24);

        alloc_cfg.mode = NetworkMode::DualStack;
        allocator.update_allocation(&alloc_cfg)?;
        let ips_dual = allocator.allocate()?;
        assert!(ips_dual.ipv4.is_some());
        assert!(ips_dual.ipv6.is_some());
        Ok(())
    }

    #[test]
    fn allocate_preferred_restores_lease_and_falls_back_on_conflict() -> Result<()> {
        let mut allocator = IpAllocator::new("fd00::1000/120")?;
        let requested = VirtualIps::dual_stack("10.8.0.42".parse()?, "fd00::1042".parse()?);

        let restored = allocator.allocate_preferred(Some(&requested))?;
        assert_eq!(restored, requested);

        let fallback = allocator.allocate_preferred(Some(&requested))?;
        assert_ne!(fallback.ipv4, requested.ipv4);
        assert_ne!(fallback.ipv6, requested.ipv6);

        let outside_pool = VirtualIps::dual_stack("192.0.2.42".parse()?, "fd00:ffff::42".parse()?);
        let fallback = allocator.allocate_preferred(Some(&outside_pool))?;
        assert_ne!(fallback.ipv4, outside_pool.ipv4);
        assert_ne!(fallback.ipv6, outside_pool.ipv6);
        Ok(())
    }

    #[tokio::test]
    async fn signed_lease_survives_coordinator_allocator_restart() -> Result<()> {
        let client_id = "lease-client".to_string();
        let public_key = vec![7; 32];
        let observed: SocketAddr = "192.0.2.10:41000".parse()?;

        let first_manager = SessionManager::new("fd00::1000/120")?;
        let (first_tx, _first_rx) = mpsc::unbounded_channel();
        let assigned = first_manager
            .register_client(
                client_id.clone(),
                public_key.clone(),
                observed,
                51820,
                NatType::Open,
                first_tx,
            )
            .await?;
        drop(first_manager);

        let restarted_manager = SessionManager::new("fd00::1000/120")?;
        let (restarted_tx, _restarted_rx) = mpsc::unbounded_channel();
        let restored = restarted_manager
            .register_client_with_candidates(
                client_id,
                public_key,
                observed,
                51820,
                NatType::Open,
                Vec::new(),
                Some(assigned.clone()),
                restarted_tx,
            )
            .await?;

        assert_eq!(restored, assigned);
        Ok(())
    }

    #[tokio::test]
    async fn test_register_uses_p2p_port_and_relay_by_virtual_ip() -> Result<()> {
        let manager = SessionManager::new("fd00::1000/120")?;
        let (tx_a, _rx_a) = mpsc::unbounded_channel();
        let (tx_b, mut rx_b) = mpsc::unbounded_channel();

        let observed_a: SocketAddr = "192.0.2.10:41000".parse()?;
        let observed_b: SocketAddr = "192.0.2.11:42000".parse()?;

        let ips_a = manager
            .register_client(
                "client-a".to_string(),
                vec![1; 32],
                observed_a,
                51820,
                NatType::Open,
                tx_a,
            )
            .await?;
        let ips_b = manager
            .register_client(
                "client-b".to_string(),
                vec![2; 32],
                observed_b,
                51821,
                NatType::PortRestricted,
                tx_b,
            )
            .await?;
        let ip_a = ips_a.primary_ipv6();
        let ip_b = ips_b.primary_ipv6();

        let peer_b = manager
            .get_peer_info(&"client-b".to_string())
            .await
            .expect("client-b peer should exist");
        assert_eq!(peer_b.endpoints, vec!["192.0.2.11:51821".parse()?]);

        assert!(
            manager
                .update_nat_type(&"client-b".to_string(), fuckipv6_common::NatType::Symmetric)
                .await
        );
        let peer_b = manager
            .get_peer_info(&"client-b".to_string())
            .await
            .expect("client-b peer should exist after NAT update");
        assert_eq!(peer_b.nat_type, fuckipv6_common::NatType::Symmetric);

        let payload = vec![0x60, 0, 0, 0];
        manager
            .send_to_virtual_ip(
                IpAddr::V6(ip_b),
                SignalingMessage::RelayPacket {
                    src_ip: IpAddr::V6(ip_a),
                    dst_ip: IpAddr::V6(ip_b),
                    payload: payload.clone(),
                },
            )
            .await?;

        let message = rx_b.recv().await.expect("client-b should receive message");
        let tokio_tungstenite::tungstenite::Message::Binary(data) = message else {
            panic!("expected binary signaling message");
        };

        match codec::decode::<SignalingMessage>(&data)? {
            SignalingMessage::PeerOnline { .. } => {
                let next = rx_b
                    .recv()
                    .await
                    .expect("client-b should receive relayed packet");
                let tokio_tungstenite::tungstenite::Message::Binary(data) = next else {
                    panic!("expected binary signaling message");
                };
                match codec::decode::<SignalingMessage>(&data)? {
                    SignalingMessage::RelayPacket {
                        src_ip,
                        dst_ip,
                        payload: relayed,
                    } => {
                        assert_eq!(src_ip, IpAddr::V6(ip_a));
                        assert_eq!(dst_ip, IpAddr::V6(ip_b));
                        assert_eq!(relayed, payload);
                    }
                    other => panic!("expected relay packet, got {:?}", other),
                }
            }
            SignalingMessage::RelayPacket {
                src_ip,
                dst_ip,
                payload: relayed,
            } => {
                assert_eq!(src_ip, IpAddr::V6(ip_a));
                assert_eq!(dst_ip, IpAddr::V6(ip_b));
                assert_eq!(relayed, payload);
            }
            other => panic!("expected relay packet, got {:?}", other),
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_duplicate_registration_preserves_ip_and_updates_mapping() -> Result<()> {
        let manager = SessionManager::new("fd00::1000/120")?;
        let client_id = "client-a".to_string();
        let first_observed: SocketAddr = "192.0.2.10:41000".parse()?;
        let second_observed: SocketAddr = "198.51.100.20:42000".parse()?;
        let (old_tx, _old_rx) = mpsc::unbounded_channel();
        let (new_tx, _new_rx) = mpsc::unbounded_channel();
        let (peer_tx, mut peer_rx) = mpsc::unbounded_channel();

        manager
            .register_client(
                "peer-b".to_string(),
                vec![9; 32],
                "192.0.2.30:43000".parse()?,
                51830,
                NatType::Open,
                peer_tx,
            )
            .await?;

        let first_ip = manager
            .register_client(
                client_id.clone(),
                vec![1; 32],
                first_observed,
                51820,
                NatType::Restricted,
                old_tx.clone(),
            )
            .await?;
        let second_ip = manager
            .register_client(
                client_id.clone(),
                vec![2; 32],
                second_observed,
                62000,
                NatType::Symmetric,
                new_tx.clone(),
            )
            .await?;

        assert_eq!(first_ip, second_ip);
        let peer = manager
            .get_peer_info(&client_id)
            .await
            .expect("client-a peer should exist");
        assert_eq!(peer.virtual_ip, first_ip.primary_ipv6());
        assert_eq!(peer.public_key, vec![2; 32]);
        assert_eq!(peer.endpoints, vec!["198.51.100.20:62000".parse()?]);
        assert_eq!(peer.nat_type, NatType::Symmetric);
        assert_eq!(manager.snapshot().await.online_clients, 2);

        let mut saw_updated_peer = false;
        while let Ok(message) = peer_rx.try_recv() {
            let tokio_tungstenite::tungstenite::Message::Binary(data) = message else {
                continue;
            };
            let SignalingMessage::PeerOnline { peer } = codec::decode::<SignalingMessage>(&data)?
            else {
                continue;
            };
            if peer.client_id == client_id
                && peer.virtual_ip == first_ip.primary_ipv6()
                && peer.endpoints == vec!["198.51.100.20:62000".parse()?]
                && peer.nat_type == NatType::Symmetric
            {
                saw_updated_peer = true;
            }
        }
        assert!(saw_updated_peer);

        manager
            .unregister_client_if_sender_matches(&client_id, &old_tx)
            .await;
        assert!(manager.get_peer_info(&client_id).await.is_some());

        manager
            .unregister_client_if_sender_matches(&client_id, &new_tx)
            .await;
        assert!(manager.get_peer_info(&client_id).await.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_traffic_counters_snapshot() -> Result<()> {
        let manager = SessionManager::new("fd00::1000/120")?;
        let (tx_a, _rx_a) = mpsc::unbounded_channel();
        let (tx_b, mut rx_b) = mpsc::unbounded_channel();

        let observed_a: SocketAddr = "192.0.2.10:41000".parse()?;
        let observed_b: SocketAddr = "192.0.2.11:42000".parse()?;

        manager.record_signaling_rx(64);
        manager
            .register_client(
                "client-a".to_string(),
                vec![1; 32],
                observed_a,
                51820,
                NatType::Open,
                tx_a,
            )
            .await?;
        let ip_b = manager
            .register_client(
                "client-b".to_string(),
                vec![2; 32],
                observed_b,
                51821,
                NatType::Restricted,
                tx_b,
            )
            .await?;

        manager.record_relay_forward(128);
        manager.record_relay_error();
        manager
            .send_to_virtual_ip(
                IpAddr::V6(ip_b.primary_ipv6()),
                SignalingMessage::HeartbeatAck { timestamp: 42 },
            )
            .await?;

        while rx_b.try_recv().is_ok() {}

        let traffic = manager.snapshot().await.traffic;
        assert_eq!(traffic.signaling_rx_messages, 1);
        assert_eq!(traffic.signaling_rx_bytes, 64);
        assert!(traffic.signaling_tx_messages >= 2);
        assert!(traffic.signaling_tx_bytes > 0);
        assert_eq!(traffic.relay_packets, 1);
        assert_eq!(traffic.relay_payload_bytes, 128);
        assert_eq!(traffic.relay_errors, 1);
        Ok(())
    }

    #[tokio::test]
    async fn test_heartbeat_and_stale_cleanup() -> Result<()> {
        let manager = SessionManager::new("fd00::1000/120")?;
        let (tx, _rx) = mpsc::unbounded_channel();
        let observed: SocketAddr = "192.0.2.10:41000".parse()?;
        let client_id = "client-a".to_string();

        manager
            .register_client(
                client_id.clone(),
                vec![1; 32],
                observed,
                51820,
                NatType::FullCone,
                tx,
            )
            .await?;

        assert!(manager.record_heartbeat(&client_id).await);
        assert!(!manager.record_heartbeat(&"missing".to_string()).await);

        tokio::time::sleep(Duration::from_millis(5)).await;
        let stale = manager
            .cleanup_stale_sessions(Duration::from_millis(1))
            .await;

        assert_eq!(stale, vec![client_id.clone()]);
        assert!(manager.get_peer_info(&client_id).await.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_blacklist_admission_control() -> Result<()> {
        let manager = SessionManager::new("fd00::1000/120")?;
        let pk = vec![7u8; 32];
        let fingerprint = fuckipv6_common::Identity::public_key_fingerprint(&pk);

        assert!(!manager.is_blacklisted("node-bad", &pk).await);

        manager.ban_client("node-bad".to_string()).await;
        assert!(manager.is_blacklisted("node-bad", &pk).await);

        let unbanned = manager.unban_client("node-bad").await;
        assert!(unbanned);
        assert!(!manager.is_blacklisted("node-bad", &pk).await);

        // 通过指纹前缀封禁
        manager.ban_client(fingerprint[0..8].to_string()).await;
        assert!(manager.is_blacklisted("any-node-id", &pk).await);
        Ok(())
    }
}
