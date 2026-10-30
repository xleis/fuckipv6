use anyhow::{anyhow, bail, Context, Result};
use fuckipv6_common::{
    codec, BlacklistConfig, ClientId, NatType, NetworkAllocation, NetworkMode, PeerInfo,
    QuicClientCertificateAuthorizer, SignalingMessage, VirtualIps,
};
use serde::Serialize;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex as TokioMutex, RwLock};
use tokio_tungstenite::tungstenite::Message;

use super::blacklist::SharedBlacklistStore;
use super::revocation::NetworkAuthorizationDirectory;

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
    blacklist_store: StdRwLock<Option<SharedBlacklistStore>>,
    blacklist_write_lock: TokioMutex<()>,
    authorization_directory: StdRwLock<Option<Arc<NetworkAuthorizationDirectory>>>,
    registration_authorization_gate: Arc<RwLock<()>>,
    certificate_allowlist: Arc<StdRwLock<HashMap<ClientId, [u8; 32]>>>,
}

#[derive(Debug, Clone)]
struct SessionClientCertificateAuthorizer {
    allowlist: Arc<StdRwLock<HashMap<ClientId, [u8; 32]>>>,
}

impl QuicClientCertificateAuthorizer for SessionClientCertificateAuthorizer {
    fn is_authorized(&self, public_key: &[u8; 32]) -> bool {
        self.allowlist
            .read()
            .map(|allowlist| {
                allowlist
                    .values()
                    .any(|authorized| authorized == public_key)
            })
            .unwrap_or(false)
    }
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

#[cfg(test)]
#[derive(Debug, Default)]
struct EphemeralAuthorizationStore {
    revocations: StdRwLock<Vec<fuckipv6_common::NetworkAuthorizationRevocation>>,
}

#[cfg(test)]
impl super::blacklist::BlacklistStore for EphemeralAuthorizationStore {
    fn supports_authorization_revocations(&self) -> bool {
        true
    }

    fn load(&self) -> Result<BlacklistConfig> {
        Ok(BlacklistConfig::default())
    }

    fn save(&self, _blacklist: &BlacklistConfig) -> Result<()> {
        Ok(())
    }

    fn load_authorization_revocations(
        &self,
    ) -> Result<Vec<fuckipv6_common::NetworkAuthorizationRevocation>> {
        self.revocations
            .read()
            .map(|records| records.clone())
            .map_err(|_| anyhow!("测试授权撤销存储锁已损坏"))
    }

    fn revoke_authorization(
        &self,
        record: &fuckipv6_common::NetworkAuthorizationRevocation,
    ) -> Result<bool> {
        let mut records = self
            .revocations
            .write()
            .map_err(|_| anyhow!("测试授权撤销存储锁已损坏"))?;
        if records.iter().any(|existing| {
            existing.network_id == record.network_id
                && (existing.client_id == record.client_id
                    || existing.public_key_fingerprint == record.public_key_fingerprint)
        }) {
            return Ok(false);
        }
        records.push(record.clone());
        Ok(true)
    }

    fn lift_authorization(
        &self,
        network_id: &str,
        client_id: &str,
        public_key_fingerprint: &str,
    ) -> Result<bool> {
        let mut records = self
            .revocations
            .write()
            .map_err(|_| anyhow!("测试授权撤销存储锁已损坏"))?;
        let original_len = records.len();
        records.retain(|record| {
            !(record.network_id == network_id
                && (record.client_id == client_id
                    || record.public_key_fingerprint == public_key_fingerprint))
        });
        Ok(records.len() != original_len)
    }

    fn authorization_network_id(&self) -> Result<String> {
        Ok("test-network".to_string())
    }
}

fn initial_authorization_directory() -> Result<Option<Arc<NetworkAuthorizationDirectory>>> {
    #[cfg(test)]
    {
        Ok(Some(Arc::new(NetworkAuthorizationDirectory::new(
            "test-network",
            Arc::new(EphemeralAuthorizationStore::default()),
        )?)))
    }
    #[cfg(not(test))]
    {
        Ok(None)
    }
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
            blacklist_store: StdRwLock::new(None),
            blacklist_write_lock: TokioMutex::new(()),
            authorization_directory: StdRwLock::new(initial_authorization_directory()?),
            registration_authorization_gate: Arc::new(RwLock::new(())),
            certificate_allowlist: Arc::new(StdRwLock::new(HashMap::new())),
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
            blacklist_store: StdRwLock::new(None),
            blacklist_write_lock: TokioMutex::new(()),
            authorization_directory: StdRwLock::new(initial_authorization_directory()?),
            registration_authorization_gate: Arc::new(RwLock::new(())),
            certificate_allowlist: Arc::new(StdRwLock::new(HashMap::new())),
        })
    }

    /// 返回可在同步 TLS 回调中查询的客户端证书公钥授权器。
    ///
    /// allowlist 只包含当前完成签名注册的在线节点；空集合默认拒绝。授权器
    /// 持有独立的同步锁视图，不在 TLS 回调中等待 Tokio 异步会话锁。
    pub fn client_certificate_authorizer(&self) -> Arc<dyn QuicClientCertificateAuthorizer> {
        Arc::new(SessionClientCertificateAuthorizer {
            allowlist: self.certificate_allowlist.clone(),
        })
    }

    fn set_client_certificate_authorization(&self, client_id: &ClientId, public_key: &[u8]) {
        let public_key = <[u8; 32]>::try_from(public_key).ok();
        match self.certificate_allowlist.write() {
            Ok(mut allowlist) => match public_key {
                Some(public_key) => {
                    allowlist.insert(client_id.clone(), public_key);
                }
                None => {
                    allowlist.remove(client_id);
                }
            },
            Err(_) => tracing::error!(
                "客户端证书 allowlist 锁已损坏，后续 QUIC 握手将失败关闭: {client_id}"
            ),
        }
    }

    fn revoke_client_certificate_authorization(&self, client_id: &ClientId) {
        if let Ok(mut allowlist) = self.certificate_allowlist.write() {
            allowlist.remove(client_id);
        }
    }

    /// 检查客户端 ID 或公钥是否在黑名单中
    pub async fn is_blacklisted(&self, client_id: &str, public_key: &[u8]) -> bool {
        let fingerprint = fuckipv6_common::Identity::public_key_fingerprint(public_key);
        self.blacklist
            .read()
            .await
            .is_banned(client_id, &fingerprint)
    }

    /// 挂载持久化存储，并以磁盘配置作为启动时的权威黑名单状态。
    ///
    /// 加载失败时保持当前内存状态不变；加载成功后，已在线且匹配黑名单的会话
    /// 会走正常注销路径释放租约、撤销证书授权并广播离线事件。
    pub async fn attach_blacklist_store(&self, store: SharedBlacklistStore) -> Result<()> {
        let load_store = store.clone();
        let blacklist = tokio::task::spawn_blocking(move || load_store.load())
            .await
            .context("等待持久黑名单加载任务失败")??;

        let authorization_directory = if store.supports_authorization_revocations() {
            let network_id = store.authorization_network_id()?;
            let directory = Arc::new(NetworkAuthorizationDirectory::new(
                network_id,
                store.clone(),
            )?);
            let probe_store = store.clone();
            tokio::task::spawn_blocking(move || probe_store.load_authorization_revocations())
                .await
                .context("等待授权撤销存储探测任务失败")??;
            Some(directory)
        } else {
            tracing::warn!("持久化存储不支持授权撤销，节点注册将保持失败关闭");
            None
        };

        let _authorization_guard = self.registration_authorization_gate.write().await;
        *self.blacklist.write().await = blacklist.clone();
        {
            let mut store_slot = self
                .blacklist_store
                .write()
                .map_err(|_| anyhow!("持久黑名单存储锁已损坏"))?;
            *store_slot = Some(store);
        }
        {
            let mut directory = self
                .authorization_directory
                .write()
                .map_err(|_| anyhow!("授权撤销目录锁已损坏"))?;
            *directory = authorization_directory;
        }

        self.disconnect_blacklisted_sessions(&blacklist).await;
        Ok(())
    }

    #[cfg(test)]
    fn detach_authorization_directory_for_test(&self) -> Result<()> {
        *self
            .authorization_directory
            .write()
            .map_err(|_| anyhow!("授权撤销目录锁已损坏"))? = None;
        Ok(())
    }

    /// 查询当前网络是否已持久撤销该节点身份。
    ///
    /// 存储读取失败会原样返回错误；注册调用方必须失败关闭。
    pub async fn is_registration_revoked(
        &self,
        client_id: &str,
        public_key: &[u8],
    ) -> Result<bool> {
        let directory = self
            .authorization_directory
            .read()
            .map_err(|_| anyhow!("授权撤销目录锁已损坏"))?
            .clone();
        match directory {
            Some(directory) => directory.is_revoked(client_id, public_key).await,
            None => bail!("授权撤销存储尚未配置，拒绝节点注册"),
        }
    }

    async fn ensure_registration_authorized(
        &self,
        client_id: &str,
        public_key: &[u8],
    ) -> Result<()> {
        if self.is_registration_revoked(client_id, public_key).await? {
            bail!("节点身份已在当前网络中撤销");
        }
        Ok(())
    }

    /// 持久撤销节点，并在提交成功后立即断开该节点的现有会话。
    pub async fn try_revoke_node(&self, client_id: &str, public_key: &[u8]) -> Result<bool> {
        let _authorization_guard = self.registration_authorization_gate.write().await;
        let directory = self
            .authorization_directory
            .read()
            .map_err(|_| anyhow!("授权撤销目录锁已损坏"))?
            .clone()
            .ok_or_else(|| anyhow!("授权撤销目录尚未配置"))?;
        let changed = directory.revoke(client_id, public_key).await?;
        self.disconnect_revoked_sessions(client_id, public_key)
            .await;
        Ok(changed)
    }

    /// 从持久撤销目录移除节点；不会自动恢复已断开的会话。
    pub async fn try_lift_revocation(&self, client_id: &str, public_key: &[u8]) -> Result<bool> {
        let _authorization_guard = self.registration_authorization_gate.write().await;
        let directory = self
            .authorization_directory
            .read()
            .map_err(|_| anyhow!("授权撤销目录锁已损坏"))?
            .clone()
            .ok_or_else(|| anyhow!("授权撤销目录尚未配置"))?;
        directory.lift(client_id, public_key).await
    }

    async fn persist_blacklist_state(&self, blacklist: &BlacklistConfig) -> Result<()> {
        let store = self
            .blacklist_store
            .read()
            .map_err(|_| anyhow!("持久黑名单存储锁已损坏"))?
            .clone();
        if let Some(store) = store {
            let blacklist = blacklist.clone();
            tokio::task::spawn_blocking(move || store.save(&blacklist))
                .await
                .context("等待持久黑名单写入任务失败")??;
        }
        Ok(())
    }

    async fn disconnect_blacklisted_sessions(&self, blacklist: &BlacklistConfig) {
        let sessions_to_disconnect: Vec<(ClientId, mpsc::UnboundedSender<Message>)> = {
            let sessions = self.sessions.read().await;
            sessions
                .values()
                .filter(|session| {
                    let fingerprint = fuckipv6_common::Identity::public_key_fingerprint(
                        &session.peer_info.public_key,
                    );
                    blacklist.is_banned(&session.peer_info.client_id, &fingerprint)
                })
                .map(|session| (session.peer_info.client_id.clone(), session.tx.clone()))
                .collect()
        };

        for (client_id, tx) in sessions_to_disconnect {
            let _ = tx.send(Message::Close(None));
            self.unregister_client(&client_id).await;
            tracing::warn!("黑名单客户端已强制断开并踢出: {}", client_id);
        }
    }

    async fn disconnect_revoked_sessions(&self, client_id: &str, public_key: &[u8]) {
        let fingerprint = fuckipv6_common::Identity::public_key_fingerprint(public_key);
        let sessions_to_disconnect: Vec<(ClientId, mpsc::UnboundedSender<Message>)> = {
            let sessions = self.sessions.read().await;
            sessions
                .values()
                .filter(|session| {
                    session.peer_info.client_id == client_id
                        || fuckipv6_common::Identity::public_key_fingerprint(
                            &session.peer_info.public_key,
                        ) == fingerprint
                })
                .map(|session| (session.peer_info.client_id.clone(), session.tx.clone()))
                .collect()
        };

        for (client_id, tx) in sessions_to_disconnect {
            let _ = tx.send(Message::Close(None));
            self.unregister_client(&client_id).await;
            tracing::warn!("授权已撤销节点已强制断开并踢出: {}", client_id);
        }
    }

    /// 将客户端加入黑名单，持久化成功后才更新运行期状态并强制下线。
    ///
    /// 返回 `true` 表示新增了黑名单项；返回 `false` 表示该项已存在。
    /// 持久化失败时运行期黑名单和在线会话均保持不变。
    pub async fn try_ban_client(&self, client_id: String) -> Result<bool> {
        let target = client_id.trim();
        if target.is_empty() {
            bail!("客户端 ID 不能为空");
        }

        let _guard = self.blacklist_write_lock.lock().await;
        let mut next = self.blacklist.read().await.clone();
        let changed = !next
            .banned_clients
            .iter()
            .any(|banned| banned.trim() == target);
        if changed {
            next.banned_clients.push(target.to_string());
            self.persist_blacklist_state(&next).await?;
        }

        *self.blacklist.write().await = next.clone();
        self.disconnect_blacklisted_sessions(&next).await;
        Ok(changed)
    }

    /// 将客户端加入黑名单，并强制下线如果当前在线。
    ///
    /// 无持久化存储时保持原有内存语义；持久化失败会记录错误且不修改运行期状态。
    pub async fn ban_client(&self, client_id: String) {
        if let Err(error) = self.try_ban_client(client_id).await {
            tracing::error!("封禁客户端失败，运行期状态未修改: {error:#}");
        }
    }

    /// 从黑名单移除客户端，持久化成功后才更新运行期状态。
    pub async fn try_unban_client(&self, client_id: &str) -> Result<bool> {
        let target = client_id.trim();
        if target.is_empty() {
            return Ok(false);
        }

        let _guard = self.blacklist_write_lock.lock().await;
        let mut next = self.blacklist.read().await.clone();
        let original_len = next.banned_clients.len();
        next.banned_clients.retain(|item| item.trim() != target);
        if next.banned_clients.len() == original_len {
            return Ok(false);
        }

        self.persist_blacklist_state(&next).await?;
        *self.blacklist.write().await = next;
        Ok(true)
    }

    /// 从黑名单移除客户端。
    ///
    /// 无持久化存储时保持原有内存语义；持久化失败会记录错误且不修改运行期状态。
    pub async fn unban_client(&self, client_id: &str) -> bool {
        match self.try_unban_client(client_id).await {
            Ok(removed) => removed,
            Err(error) => {
                tracing::error!("解封客户端失败，运行期状态未修改: {error:#}");
                false
            }
        }
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

        // 与撤销共享同一进程内读写门。注册释放该门之前，撤销不能提交并清理
        // allowlist；撤销先获得写门时，注册会在首次持久目录查询处失败。
        let _authorization_guard = self.registration_authorization_gate.read().await;
        self.ensure_registration_authorized(&client_id, &public_key)
            .await?;

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

        // 撤销可能发生在首次准入检查与插入之间。插入后再次读取持久目录，
        // 保证撤销完成后不会留下已在线但未授权的会话。
        if let Err(error) = self
            .ensure_registration_authorized(
                &result.peer_info.client_id,
                &result.peer_info.public_key,
            )
            .await
        {
            self.unregister_client(&result.peer_info.client_id).await;
            return Err(error);
        }

        self.set_client_certificate_authorization(
            &result.peer_info.client_id,
            &result.peer_info.public_key,
        );

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
            self.revoke_client_certificate_authorization(client_id);
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
    use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
    use std::sync::RwLock as TestRwLock;

    #[derive(Default)]
    struct MemoryBlacklistStore {
        state: TestRwLock<BlacklistConfig>,
        fail_save: bool,
        revocations: TestRwLock<Vec<fuckipv6_common::NetworkAuthorizationRevocation>>,
        fail_revocation_reads: AtomicBool,
    }

    impl MemoryBlacklistStore {
        fn with(initial: BlacklistConfig, fail_save: bool) -> Self {
            Self {
                state: TestRwLock::new(initial),
                fail_save,
                revocations: TestRwLock::new(Vec::new()),
                fail_revocation_reads: AtomicBool::new(false),
            }
        }

        fn set_fail_revocation_reads(&self, fail: bool) {
            self.fail_revocation_reads
                .store(fail, AtomicOrdering::SeqCst);
        }
    }

    impl super::super::blacklist::BlacklistStore for MemoryBlacklistStore {
        fn supports_authorization_revocations(&self) -> bool {
            true
        }

        fn load(&self) -> Result<BlacklistConfig> {
            self.state
                .read()
                .map(|state| state.clone())
                .map_err(|_| anyhow!("test blacklist store lock poisoned"))
        }

        fn save(&self, blacklist: &BlacklistConfig) -> Result<()> {
            if self.fail_save {
                bail!("injected blacklist persistence failure");
            }
            *self
                .state
                .write()
                .map_err(|_| anyhow!("test blacklist store lock poisoned"))? = blacklist.clone();
            Ok(())
        }

        fn load_authorization_revocations(
            &self,
        ) -> Result<Vec<fuckipv6_common::NetworkAuthorizationRevocation>> {
            if self.fail_revocation_reads.load(AtomicOrdering::SeqCst) {
                bail!("injected authorization directory read failure");
            }
            self.revocations
                .read()
                .map(|revocations| revocations.clone())
                .map_err(|_| anyhow!("test authorization directory lock poisoned"))
        }

        fn revoke_authorization(
            &self,
            record: &fuckipv6_common::NetworkAuthorizationRevocation,
        ) -> Result<bool> {
            let mut revocations = self
                .revocations
                .write()
                .map_err(|_| anyhow!("test authorization directory lock poisoned"))?;
            if revocations.iter().any(|existing| {
                existing.network_id == record.network_id
                    && (existing.client_id == record.client_id
                        || existing.public_key_fingerprint == record.public_key_fingerprint)
            }) {
                return Ok(false);
            }
            revocations.push(record.clone());
            Ok(true)
        }

        fn lift_authorization(
            &self,
            network_id: &str,
            client_id: &str,
            public_key_fingerprint: &str,
        ) -> Result<bool> {
            let mut revocations = self
                .revocations
                .write()
                .map_err(|_| anyhow!("test authorization directory lock poisoned"))?;
            let original_len = revocations.len();
            revocations.retain(|record| {
                !(record.network_id == network_id
                    && (record.client_id == client_id
                        || record.public_key_fingerprint == public_key_fingerprint))
            });
            Ok(revocations.len() != original_len)
        }

        fn authorization_network_id(&self) -> Result<String> {
            Ok("test-network".to_string())
        }
    }

    struct BlacklistOnlyStore;

    impl super::super::blacklist::BlacklistStore for BlacklistOnlyStore {
        fn load(&self) -> Result<BlacklistConfig> {
            Ok(BlacklistConfig::default())
        }

        fn save(&self, _blacklist: &BlacklistConfig) -> Result<()> {
            Ok(())
        }
    }

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
    async fn client_certificate_authorizer_tracks_registration_rekey_unregistration_and_ban(
    ) -> Result<()> {
        let manager = SessionManager::new("fd00::1000/120")?;
        let authorizer = manager.client_certificate_authorizer();
        let first_identity = fuckipv6_common::Identity::generate()?;
        let second_identity = fuckipv6_common::Identity::generate()?;
        let (tx, _rx) = mpsc::unbounded_channel();
        let observed = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000);

        let first_key = first_identity.public_key();
        let first_key: [u8; 32] = first_key
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("first identity public key is not Ed25519"))?;
        assert!(!authorizer.is_authorized(&first_key));

        manager
            .register_client(
                first_identity.client_id().to_string(),
                first_identity.public_key(),
                observed,
                51820,
                NatType::Open,
                tx.clone(),
            )
            .await?;
        assert!(authorizer.is_authorized(&first_key));

        let second_key = second_identity.public_key();
        let second_key: [u8; 32] = second_key
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("second identity public key is not Ed25519"))?;
        manager
            .register_client(
                first_identity.client_id().to_string(),
                second_identity.public_key(),
                observed,
                51820,
                NatType::Open,
                tx,
            )
            .await?;
        assert!(!authorizer.is_authorized(&first_key));
        assert!(authorizer.is_authorized(&second_key));

        manager
            .unregister_client(&first_identity.client_id().to_string())
            .await;
        assert!(!authorizer.is_authorized(&second_key));

        let (tx, _rx) = mpsc::unbounded_channel();
        manager
            .register_client(
                first_identity.client_id().to_string(),
                second_identity.public_key(),
                observed,
                51820,
                NatType::Open,
                tx,
            )
            .await?;
        assert!(authorizer.is_authorized(&second_key));

        manager
            .ban_client(first_identity.client_id().to_string())
            .await;
        assert!(
            manager
                .get_peer_info(&first_identity.client_id().to_string())
                .await
                .is_none(),
            "ban must remove the active session"
        );
        assert!(!authorizer.is_authorized(&second_key));
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

    #[tokio::test]
    async fn persistent_blacklist_survives_manager_restart_and_unban() -> Result<()> {
        let store = Arc::new(MemoryBlacklistStore::default());
        let first = SessionManager::new("fd00::1000/120")?;
        first.attach_blacklist_store(store.clone()).await?;
        assert!(first.try_ban_client("node-bad".to_string()).await?);

        let second = SessionManager::new("fd00::1000/120")?;
        second.attach_blacklist_store(store.clone()).await?;
        assert_eq!(second.get_blacklist().await, vec!["node-bad".to_string()]);
        assert!(second.is_blacklisted("node-bad", &[9; 32]).await);
        assert!(second.try_unban_client("node-bad").await?);

        let third = SessionManager::new("fd00::1000/120")?;
        third.attach_blacklist_store(store).await?;
        assert!(third.get_blacklist().await.is_empty());
        assert!(!third.is_blacklisted("node-bad", &[9; 32]).await);
        Ok(())
    }

    #[tokio::test]
    async fn persistence_failure_keeps_runtime_blacklist_and_session_unchanged() -> Result<()> {
        let store = Arc::new(MemoryBlacklistStore::with(BlacklistConfig::default(), true));
        let manager = SessionManager::new("fd00::1000/120")?;
        manager.attach_blacklist_store(store).await?;

        let authorizer = manager.client_certificate_authorizer();
        let identity = fuckipv6_common::Identity::generate()?;
        let client_id = identity.client_id().to_string();
        let public_key = identity.public_key();
        let public_key: [u8; 32] = public_key
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("identity public key is not Ed25519"))?;
        let (tx, _rx) = mpsc::unbounded_channel();
        manager
            .register_client(
                client_id.clone(),
                identity.public_key(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000),
                51820,
                NatType::Open,
                tx,
            )
            .await?;

        let error = manager
            .try_ban_client(client_id.clone())
            .await
            .expect_err("persistence failure must reject ban");
        assert!(error.to_string().contains("persistence failure"));
        assert!(manager.get_blacklist().await.is_empty());
        assert!(
            !manager
                .is_blacklisted("any-node", &identity.public_key())
                .await
        );
        assert!(manager.get_peer_info(&client_id).await.is_some());
        assert!(authorizer.is_authorized(&public_key));
        Ok(())
    }

    #[tokio::test]
    async fn unban_persistence_failure_keeps_runtime_blacklist_unchanged() -> Result<()> {
        let persisted = BlacklistConfig {
            banned_clients: vec!["node-bad".to_string()],
        };
        let store = Arc::new(MemoryBlacklistStore::with(persisted.clone(), true));
        let manager = SessionManager::new("fd00::1000/120")?;
        manager.attach_blacklist_store(store).await?;

        let error = manager
            .try_unban_client("node-bad")
            .await
            .expect_err("persistence failure must reject unban");
        assert!(error.to_string().contains("persistence failure"));
        assert_eq!(manager.get_blacklist().await, persisted.banned_clients);
        assert!(manager.is_blacklisted("node-bad", &[9; 32]).await);
        Ok(())
    }

    #[tokio::test]
    async fn attach_blacklist_disconnects_and_revokes_existing_match() -> Result<()> {
        let manager = SessionManager::new("fd00::1000/120")?;
        let authorizer = manager.client_certificate_authorizer();
        let identity = fuckipv6_common::Identity::generate()?;
        let client_id = identity.client_id().to_string();
        let public_key = identity.public_key();
        let public_key: [u8; 32] = public_key
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("identity public key is not Ed25519"))?;
        let (tx, mut rx) = mpsc::unbounded_channel();
        manager
            .register_client(
                client_id.clone(),
                identity.public_key(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000),
                51820,
                NatType::Open,
                tx,
            )
            .await?;
        assert!(authorizer.is_authorized(&public_key));

        let store = Arc::new(MemoryBlacklistStore::with(
            BlacklistConfig {
                banned_clients: vec![identity.client_id().to_string()],
            },
            false,
        ));
        manager.attach_blacklist_store(store).await?;

        assert!(manager.get_peer_info(&client_id).await.is_none());
        assert!(!authorizer.is_authorized(&public_key));
        assert!(matches!(
            rx.try_recv(),
            Ok(Message::Close(_)) | Err(mpsc::error::TryRecvError::Empty)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn revoked_identity_is_rejected_after_restart_and_active_session_is_disconnected(
    ) -> Result<()> {
        let store = Arc::new(MemoryBlacklistStore::default());
        let first = SessionManager::new("fd00::1000/120")?;
        first.attach_blacklist_store(store.clone()).await?;
        let authorizer = first.client_certificate_authorizer();
        let identity = fuckipv6_common::Identity::generate()?;
        let client_id = identity.client_id().to_string();
        let public_key = identity.public_key();
        let public_key_bytes: [u8; 32] = public_key
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("identity public key is not Ed25519"))?;
        let (tx, mut rx) = mpsc::unbounded_channel();
        first
            .register_client(
                client_id.clone(),
                public_key.clone(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000),
                51820,
                NatType::Open,
                tx,
            )
            .await?;
        assert!(authorizer.is_authorized(&public_key_bytes));

        assert!(first.try_revoke_node(&client_id, &public_key).await?);
        assert!(first.get_peer_info(&client_id).await.is_none());
        assert!(!authorizer.is_authorized(&public_key_bytes));
        assert!(matches!(rx.try_recv(), Ok(Message::Close(_))));

        let restarted = SessionManager::new("fd00::1000/120")?;
        restarted.attach_blacklist_store(store).await?;
        let (tx, _rx) = mpsc::unbounded_channel();
        let error = restarted
            .register_client(
                client_id,
                public_key,
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41001),
                51821,
                NatType::Open,
                tx,
            )
            .await
            .expect_err("persisted revocation must reject registration after restart");
        assert!(error.to_string().contains("撤销"));
        Ok(())
    }

    #[tokio::test]
    async fn authorization_directory_read_failure_denies_registration_without_state_changes(
    ) -> Result<()> {
        let store = Arc::new(MemoryBlacklistStore::default());
        let manager = SessionManager::new("fd00::1000/120")?;
        manager.attach_blacklist_store(store.clone()).await?;
        store.set_fail_revocation_reads(true);

        let identity = fuckipv6_common::Identity::generate()?;
        let client_id = identity.client_id().to_string();
        let public_key = identity.public_key();
        let public_key_bytes: [u8; 32] = public_key
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("identity public key is not Ed25519"))?;
        let (tx, _rx) = mpsc::unbounded_channel();
        let error = manager
            .register_client(
                client_id.clone(),
                public_key,
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000),
                51820,
                NatType::Open,
                tx,
            )
            .await
            .expect_err("authorization storage errors must fail registration closed");
        assert!(error
            .to_string()
            .contains("authorization directory read failure"));
        assert!(manager.get_peer_info(&client_id).await.is_none());
        assert!(!manager
            .client_certificate_authorizer()
            .is_authorized(&public_key_bytes));
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_registration_and_revocation_leave_no_authorized_session() -> Result<()> {
        let store = Arc::new(MemoryBlacklistStore::default());
        let manager = Arc::new(SessionManager::new("fd00::1000/120")?);
        manager.attach_blacklist_store(store.clone()).await?;
        let identity = fuckipv6_common::Identity::generate()?;
        let client_id = identity.client_id().to_string();
        let public_key = identity.public_key();
        let (tx, _rx) = mpsc::unbounded_channel();
        let registration_manager = manager.clone();
        let registration_id = client_id.clone();
        let registration_key = public_key.clone();
        let revocation_manager = manager.clone();
        let revocation_id = client_id.clone();
        let revocation_key = public_key.clone();

        let (registration, revocation) = tokio::join!(
            async move {
                registration_manager
                    .register_client(
                        registration_id,
                        registration_key,
                        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000),
                        51820,
                        NatType::Open,
                        tx,
                    )
                    .await
            },
            async move {
                revocation_manager
                    .try_revoke_node(&revocation_id, &revocation_key)
                    .await
            }
        );

        let _ = registration;
        assert!(revocation?);
        assert!(
            manager
                .is_registration_revoked(&client_id, &public_key)
                .await?,
            "revocation must be durable after concurrent registration"
        );
        assert!(
            manager.get_peer_info(&client_id).await.is_none(),
            "a registration racing with revocation must not remain active"
        );
        let public_key_bytes: [u8; 32] = public_key
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("identity public key is not Ed25519"))?;
        assert!(
            !manager
                .client_certificate_authorizer()
                .is_authorized(&public_key_bytes),
            "revocation must not be followed by certificate allowlist reinstatement"
        );
        Ok(())
    }

    #[tokio::test]
    async fn missing_authorization_store_rejects_registration_without_state_changes() -> Result<()>
    {
        let manager = SessionManager::new("fd00::1000/120")?;
        manager.detach_authorization_directory_for_test()?;
        let identity = fuckipv6_common::Identity::generate()?;
        let client_id = identity.client_id().to_string();
        let public_key = identity.public_key();
        let public_key_bytes: [u8; 32] = public_key
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("identity public key is not Ed25519"))?;
        let (tx, _rx) = mpsc::unbounded_channel();

        let error = manager
            .register_client(
                client_id.clone(),
                public_key,
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000),
                51820,
                NatType::Open,
                tx,
            )
            .await
            .expect_err("a missing revocation store must fail registration closed");
        assert!(error.to_string().contains("授权撤销存储尚未配置"));
        assert!(manager.get_peer_info(&client_id).await.is_none());
        assert!(!manager
            .client_certificate_authorizer()
            .is_authorized(&public_key_bytes));
        Ok(())
    }

    #[tokio::test]
    async fn blacklist_only_store_keeps_registration_fail_closed() -> Result<()> {
        let manager = SessionManager::new("fd00::1000/120")?;
        manager
            .attach_blacklist_store(Arc::new(BlacklistOnlyStore))
            .await
            .expect("blacklist-only stores must remain usable for blacklist management");

        let identity = fuckipv6_common::Identity::generate()?;
        let client_id = identity.client_id().to_string();
        let public_key = identity.public_key();
        let (tx, _rx) = mpsc::unbounded_channel();
        let error = manager
            .register_client(
                client_id.clone(),
                public_key,
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000),
                51820,
                NatType::Open,
                tx,
            )
            .await
            .expect_err("a missing revocation directory must reject registration");
        assert!(error.to_string().contains("授权撤销存储尚未配置"));
        assert!(manager.get_peer_info(&client_id).await.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn revoked_node_cannot_reenable_certificate_allowlist_by_registering_again() -> Result<()>
    {
        let store = Arc::new(MemoryBlacklistStore::default());
        let manager = SessionManager::new("fd00::1000/120")?;
        manager.attach_blacklist_store(store).await?;
        let identity = fuckipv6_common::Identity::generate()?;
        let client_id = identity.client_id().to_string();
        let public_key = identity.public_key();
        let public_key_bytes: [u8; 32] = public_key
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("identity public key is not Ed25519"))?;
        let (first_tx, _first_rx) = mpsc::unbounded_channel();
        manager
            .register_client(
                client_id.clone(),
                public_key.clone(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000),
                51820,
                NatType::Open,
                first_tx,
            )
            .await?;
        assert!(manager
            .client_certificate_authorizer()
            .is_authorized(&public_key_bytes));

        assert!(manager.try_revoke_node(&client_id, &public_key).await?);
        assert!(!manager
            .client_certificate_authorizer()
            .is_authorized(&public_key_bytes));

        let (second_tx, _second_rx) = mpsc::unbounded_channel();
        let error = manager
            .register_client(
                client_id.clone(),
                public_key,
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41001),
                51821,
                NatType::Open,
                second_tx,
            )
            .await
            .expect_err("a revoked node must not register again");
        assert!(error.to_string().contains("撤销"));
        assert!(manager.get_peer_info(&client_id).await.is_none());
        assert!(!manager
            .client_certificate_authorizer()
            .is_authorized(&public_key_bytes));
        Ok(())
    }
}
