use anyhow::{Context, Result};
use bytes::Bytes;
use fuckipv6_common::{
    build_quic_server_config_with_client_certificate_authorizer,
    domain::{
        NodeId, RelayAccessPolicy, RelayAuthorizationDecision, RelayExistingSessionPolicy,
        RelayHealthSnapshot, RelayHealthStatus, RelayLifecycleState, RelayLimitPolicy,
        RelayMeasurementState, RelayMemberGrant, RelayReachabilityState, RelayUsageSnapshot,
    },
    export_quic_relay_channel_binding, generate_self_signed_quic_certificate, recv_relay_message,
    send_relay_message, verify_quic_client_identity_binding, ClientId, ErrorCode, Identity,
    QuicCertificate, RelayMessage, RelayRegistrationBinding, MAX_SIGNALING_RELAY_PAYLOAD_SIZE,
};
use std::collections::HashMap;
use std::net::{Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, RwLock, Semaphore};

use crate::signaling::session::SessionManager;

const RELAY_HARD_BANDWIDTH_LIMIT_BPS: u64 = 1_000_000_000;
const RELAY_REGISTER_STREAM_ID: u64 = 0;
const RELAY_AUTHORIZATION_STREAM_ID: u64 = 1;
const MAX_RELAY_CONNECTION_TASKS: usize = 512;
const MAX_RELAY_STREAM_TASKS: usize = 4096;
const RELAY_CONNECTION_LIFECYCLE_INTERVAL: Duration = Duration::from_secs(1);

/// Trusted source of the current Relay policy. Production must inject a source
/// backed by Coordinator-owned files; a missing or invalid source denies every
/// request. Register messages and packet metadata are never used to construct
/// authorization contexts.
pub trait RelayPolicyAuthority: Send + Sync {
    fn current_policy(&self) -> Result<RelayAccessPolicy>;

    /// Validate a consumer-signed grant and rebuild the trusted consumer
    /// context from the current roster and policy snapshot. Implementations
    /// must not trust role, membership or limits outside the signed grant and
    /// their durable control-plane state.
    fn authorize_grant(
        &self,
        grant: &RelayMemberGrant,
        now_unix_ms: u64,
    ) -> Result<(RelayAccessPolicy, RelayAuthorizationDecision)>;
}

#[derive(Clone)]
pub struct RelayServer {
    bind_addr: SocketAddr,
    endpoint: quinn::Endpoint,
    session_manager: Arc<SessionManager>,
    policy_authority: Arc<dyn RelayPolicyAuthority>,
    state: Arc<RelayState>,
    certificate: QuicCertificate,
}

#[derive(Debug)]
struct RelayState {
    clients: RwLock<HashMap<Ipv6Addr, RelayClient>>,
    rate: RwLock<RelayRateState>,
    members: RwLock<HashMap<NodeId, Arc<MemberUsage>>>,
    metrics: RelayMetrics,
    registration_gate: Mutex<()>,
    drain: Arc<RwLock<()>>,
    quota_window: Mutex<QuotaWindow>,
    policy_revision: AtomicU64,
    listener_open: AtomicBool,
    connection_tasks: Arc<Semaphore>,
    stream_tasks: Arc<Semaphore>,
}

impl Default for RelayState {
    fn default() -> Self {
        Self {
            clients: RwLock::new(HashMap::new()),
            rate: RwLock::new(RelayRateState::default()),
            members: RwLock::new(HashMap::new()),
            metrics: RelayMetrics::default(),
            registration_gate: Mutex::new(()),
            drain: Arc::new(RwLock::new(())),
            quota_window: Mutex::new(QuotaWindow::default()),
            policy_revision: AtomicU64::new(0),
            listener_open: AtomicBool::new(true),
            connection_tasks: Arc::new(Semaphore::new(MAX_RELAY_CONNECTION_TASKS)),
            stream_tasks: Arc::new(Semaphore::new(MAX_RELAY_STREAM_TASKS)),
        }
    }
}

#[derive(Debug, Clone)]
struct RelayClient {
    client_id: ClientId,
    node_id: NodeId,
    public_key: Vec<u8>,
    member_grant: RelayMemberGrant,
    effective_limits: RelayLimitPolicy,
    policy_revision: u64,
    connection: quinn::Connection,
    connection_id: usize,
    registered_at: Instant,
}

struct RelayPacketRequest {
    stream_id: u64,
    src_ip: Ipv6Addr,
    dst_ip: Ipv6Addr,
    payload: Bytes,
}

enum RelayStreamRequest {
    Packet(RelayPacketRequest),
    UpdateAuthorization {
        stream_id: u64,
        member_grant: Box<RelayMemberGrant>,
    },
}

#[derive(Debug, Clone)]
struct TokenBucket {
    tokens: u64,
    last_refill: Instant,
    rate_bytes_per_sec: u64,
    burst_bytes: u64,
}

impl TokenBucket {
    fn new(rate_bytes_per_sec: u64) -> Self {
        let rate_bytes_per_sec = rate_bytes_per_sec.max(1);
        let burst_bytes = rate_bytes_per_sec;
        Self {
            tokens: burst_bytes,
            last_refill: Instant::now(),
            rate_bytes_per_sec,
            burst_bytes,
        }
    }

    fn reconfigure(&mut self, rate_bytes_per_sec: u64) {
        let rate_bytes_per_sec = rate_bytes_per_sec.max(1);
        if self.rate_bytes_per_sec == rate_bytes_per_sec {
            return;
        }
        self.refill();
        self.rate_bytes_per_sec = rate_bytes_per_sec;
        self.burst_bytes = rate_bytes_per_sec;
        self.tokens = self.tokens.min(self.burst_bytes);
    }

    fn has_capacity(&mut self, bytes: usize) -> bool {
        self.refill();
        self.tokens >= bytes as u64
    }

    fn consume(&mut self, bytes: usize) {
        self.tokens = self.tokens.saturating_sub(bytes as u64);
    }

    fn refill(&mut self) {
        let elapsed = self.last_refill.elapsed();
        if elapsed.is_zero() {
            return;
        }

        let refill = (elapsed.as_secs_f64() * self.rate_bytes_per_sec as f64) as u64;
        if refill == 0 {
            return;
        }

        self.tokens = self.tokens.saturating_add(refill).min(self.burst_bytes);
        self.last_refill = Instant::now();
    }
}

#[derive(Debug, Default)]
struct RelayRateState {
    global: Option<TokenBucket>,
    members: HashMap<NodeId, TokenBucket>,
}

#[derive(Debug, Default)]
struct MemberUsage {
    active_connections: AtomicU32,
    active_streams: AtomicU32,
    forwarded_packets: AtomicU64,
    forwarded_bytes: AtomicU64,
    reserved_bytes: AtomicU64,
    rejected_packets: AtomicU64,
    rejected_connections: AtomicU64,
}

#[derive(Debug, Default)]
struct RelayMetrics {
    active_connections: AtomicU32,
    active_streams: AtomicU32,
    forwarded_packets: AtomicU64,
    forwarded_bytes: AtomicU64,
    rejected_packets: AtomicU64,
    rejected_connections: AtomicU64,
    reserved_bytes: AtomicU64,
    forwarding_observed: AtomicBool,
    last_forwarded_at_unix_ms: AtomicU64,
    last_forwarding_failure_at_unix_ms: AtomicU64,
    quota_started_at_unix_ms: AtomicU64,
    quota_reset_at_unix_ms: AtomicU64,
}

#[derive(Debug, Default)]
struct QuotaWindow {
    started_at_unix_ms: u64,
    active_reset_at_unix_ms: Option<u64>,
    applied_reset_at_unix_ms: Option<u64>,
}

struct QuotaReservation {
    state: Arc<RelayState>,
    member: Arc<MemberUsage>,
    bytes: u64,
    active: bool,
}

impl QuotaReservation {
    fn commit(mut self) {
        saturating_sub_u64(&self.state.metrics.reserved_bytes, self.bytes);
        saturating_sub_u64(&self.member.reserved_bytes, self.bytes);
        self.state
            .metrics
            .forwarded_bytes
            .fetch_add(self.bytes, Ordering::AcqRel);
        self.state
            .metrics
            .forwarded_packets
            .fetch_add(1, Ordering::AcqRel);
        self.member
            .forwarded_bytes
            .fetch_add(self.bytes, Ordering::AcqRel);
        self.member.forwarded_packets.fetch_add(1, Ordering::AcqRel);
        self.state
            .metrics
            .forwarding_observed
            .store(true, Ordering::Release);
        self.state
            .metrics
            .last_forwarded_at_unix_ms
            .fetch_max(now_unix_ms(), Ordering::AcqRel);
        self.active = false;
    }
}

impl Drop for QuotaReservation {
    fn drop(&mut self) {
        if self.active {
            saturating_sub_u64(&self.state.metrics.reserved_bytes, self.bytes);
            saturating_sub_u64(&self.member.reserved_bytes, self.bytes);
        }
    }
}

struct RelayStreamGuard {
    state: Arc<RelayState>,
    member: Arc<MemberUsage>,
}

impl RelayStreamGuard {
    fn acquire(
        state: Arc<RelayState>,
        member: Arc<MemberUsage>,
        global_limit: Option<u32>,
        member_limit: Option<u32>,
    ) -> std::result::Result<Self, ()> {
        try_increment_bounded(&state.metrics.active_streams, global_limit).map_err(|_| ())?;
        if try_increment_bounded(&member.active_streams, member_limit).is_err() {
            saturating_sub(&state.metrics.active_streams, 1);
            return Err(());
        }
        Ok(Self { state, member })
    }
}

impl Drop for RelayStreamGuard {
    fn drop(&mut self) {
        saturating_sub(&self.state.metrics.active_streams, 1);
        saturating_sub(&self.member.active_streams, 1);
    }
}

impl RelayServer {
    pub fn bind(
        bind_addr: SocketAddr,
        session_manager: Arc<SessionManager>,
        policy_authority: Arc<dyn RelayPolicyAuthority>,
        certificate: QuicCertificate,
    ) -> Result<Self> {
        let server_config = build_quic_server_config_with_client_certificate_authorizer(
            &certificate,
            session_manager.client_certificate_authorizer(),
        )?;
        let endpoint = quinn::Endpoint::server(server_config, bind_addr)
            .with_context(|| format!("无法启动 QUIC 中继服务: {bind_addr}"))?;
        let bind_addr = endpoint
            .local_addr()
            .context("无法读取 QUIC 中继监听地址")?;

        Ok(Self {
            bind_addr,
            endpoint,
            session_manager,
            policy_authority,
            state: Arc::new(RelayState::default()),
            certificate,
        })
    }

    pub fn bind_with_generated_certificate(
        bind_addr: SocketAddr,
        session_manager: Arc<SessionManager>,
        policy_authority: Arc<dyn RelayPolicyAuthority>,
    ) -> Result<Self> {
        let certificate = generate_self_signed_quic_certificate(vec!["fuckipv6.local".to_string()])
            .context("无法生成 QUIC 中继证书")?;
        Self::bind(bind_addr, session_manager, policy_authority, certificate)
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.bind_addr
    }

    pub fn certificate(&self) -> &QuicCertificate {
        &self.certificate
    }

    /// Stop accepting new relay connections and close every registered QUIC
    /// session. The listener remains held by this handle until all clones are
    /// dropped, but the socket is no longer accepting traffic after the close.
    pub async fn shutdown(&self) {
        {
            let _registration = self.state.registration_gate.lock().await;
            self.state.listener_open.store(false, Ordering::Release);
        }
        self.endpoint.close(0u32.into(), b"relay shutdown");

        // Wait for every accepted connection and forwarding task to release
        // its reservations before clearing the authoritative counters.
        let _drained = self.state.drain.write().await;
        self.state.clients.write().await.clear();
        *self.state.rate.write().await = RelayRateState::default();
        self.state.members.write().await.clear();
        self.state.policy_revision.store(0, Ordering::Release);
        reset_metrics(&self.state.metrics);
        let mut quota_window = self.state.quota_window.lock().await;
        *quota_window = QuotaWindow::default();
    }

    /// Snapshot of measured work. CPU and memory stay unset because the Relay
    /// layer has no trustworthy in-process observer for those values.
    pub async fn usage_snapshot(&self) -> RelayUsageSnapshot {
        self.usage_snapshot_inner().await
    }

    async fn usage_snapshot_inner(&self) -> RelayUsageSnapshot {
        let clients = self.state.clients.read().await;
        let oldest_session_age_secs = clients
            .values()
            .map(|client| client.registered_at.elapsed().as_secs())
            .max()
            .unwrap_or(0);
        let quota_window = self.state.quota_window.lock().await;
        let quota_started_at_unix_ms =
            (quota_window.started_at_unix_ms != 0).then_some(quota_window.started_at_unix_ms);
        let quota_reset_at_unix_ms = quota_window.active_reset_at_unix_ms;
        RelayUsageSnapshot {
            active_connections: self
                .state
                .metrics
                .active_connections
                .load(Ordering::Acquire),
            active_streams: self.state.metrics.active_streams.load(Ordering::Acquire),
            forwarded_packets: self.state.metrics.forwarded_packets.load(Ordering::Acquire),
            forwarded_bytes: self.state.metrics.forwarded_bytes.load(Ordering::Acquire),
            rejected_packets: self.state.metrics.rejected_packets.load(Ordering::Acquire),
            rejected_connections: self
                .state
                .metrics
                .rejected_connections
                .load(Ordering::Acquire),
            active_sessions: self
                .state
                .metrics
                .active_connections
                .load(Ordering::Acquire),
            oldest_session_age_secs,
            observed_cpu_percent: None,
            observed_memory_bytes: None,
            cpu_measurement: RelayMeasurementState::Unsupported,
            memory_measurement: RelayMeasurementState::Unsupported,
            last_forwarded_at_unix_ms: nonzero_atomic(
                &self.state.metrics.last_forwarded_at_unix_ms,
            ),
            last_forwarding_failure_at_unix_ms: nonzero_atomic(
                &self.state.metrics.last_forwarding_failure_at_unix_ms,
            ),
            quota_started_at_unix_ms,
            quota_reset_at_unix_ms,
        }
    }

    pub async fn health_snapshot(&self) -> RelayHealthSnapshot {
        let now = now_unix_ms();
        let policy = self.load_policy(now).await;
        let (limits, limit_sources, authorization_valid, reason_code) = match policy {
            Ok(policy) => (policy.limits, policy.limit_sources, true, None),
            Err(error) => {
                tracing::warn!("Relay 健康检查无法读取有效策略: {error:#}");
                (
                    RelayLimitPolicy::default(),
                    Vec::new(),
                    false,
                    Some("relay.policy_unavailable".into()),
                )
            }
        };
        let usage = self.usage_snapshot_inner().await;
        let listener_open = self.state.listener_open.load(Ordering::Acquire);
        let last_forwarded_at_unix_ms =
            nonzero_atomic(&self.state.metrics.last_forwarded_at_unix_ms);
        let lifecycle = if listener_open {
            RelayLifecycleState::Running
        } else {
            RelayLifecycleState::Stopped
        };
        let network_reachability = if last_forwarded_at_unix_ms.is_some() {
            // A successful forwarded packet is evidence that the data path was
            // reachable. Binding a listener alone proves only local socket setup.
            RelayReachabilityState::Reachable
        } else {
            RelayReachabilityState::Unknown
        };
        let mut health = RelayHealthSnapshot::observe(
            lifecycle,
            if listener_open {
                Some(self.bind_addr)
            } else {
                None
            },
            listener_open,
            network_reachability,
            self.state
                .metrics
                .forwarding_observed
                .load(Ordering::Acquire),
            authorization_valid,
            limits,
            limit_sources,
            usage,
            now,
            reason_code,
        );
        if self.state.connection_tasks.available_permits() == 0
            || self.state.stream_tasks.available_permits() == 0
        {
            if !health
                .issues
                .contains(&fuckipv6_common::domain::RelayHealthIssue::ResourcePressure)
            {
                health
                    .issues
                    .push(fuckipv6_common::domain::RelayHealthIssue::ResourcePressure);
            }
            if matches!(
                health.status,
                RelayHealthStatus::Healthy
                    | RelayHealthStatus::Unknown
                    | RelayHealthStatus::Degraded
            ) {
                health.status = RelayHealthStatus::Degraded;
            }
        }
        health
    }

    async fn load_policy(&self, now_unix_ms: u64) -> Result<RelayAccessPolicy> {
        let policy = self
            .policy_authority
            .current_policy()
            .context("无法读取可信 Relay 策略")?;
        policy
            .validate_at(now_unix_ms)
            .context("Relay 策略校验失败")?;
        if policy.updated_at_unix_ms > now_unix_ms {
            anyhow::bail!("Relay 策略更新时间位于未来，拒绝使用");
        }

        let mut observed_revision = self.state.policy_revision.load(Ordering::Acquire);
        loop {
            if policy.revision < observed_revision {
                anyhow::bail!(
                    "Relay 策略 revision 回退: {} < {}",
                    policy.revision,
                    observed_revision
                );
            }
            if policy.revision == observed_revision {
                break;
            }
            match self.state.policy_revision.compare_exchange_weak(
                observed_revision,
                policy.revision,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(current) => observed_revision = current,
            }
        }

        self.apply_quota_window(now_unix_ms, &policy.limits).await;
        Ok(policy)
    }

    async fn apply_quota_window(&self, now_unix_ms: u64, limits: &RelayLimitPolicy) {
        let mut window = self.state.quota_window.lock().await;
        if window.started_at_unix_ms == 0 {
            window.started_at_unix_ms = now_unix_ms;
        }

        let Some(reset_at) = limits.quota_reset_at_unix_ms else {
            window.active_reset_at_unix_ms = None;
            return;
        };
        if window.applied_reset_at_unix_ms == Some(reset_at) {
            window.active_reset_at_unix_ms = None;
            return;
        }
        window.active_reset_at_unix_ms = Some(reset_at);
        if now_unix_ms < reset_at {
            return;
        }

        reset_quota_metrics(&self.state).await;
        window.started_at_unix_ms = reset_at;
        window.applied_reset_at_unix_ms = Some(reset_at);
        window.active_reset_at_unix_ms = None;
        tracing::info!("Relay 配额窗口已重置: reset_at={reset_at}");
    }

    async fn member_usage(&self, node_id: &NodeId) -> Arc<MemberUsage> {
        self.state
            .members
            .write()
            .await
            .entry(node_id.clone())
            .or_insert_with(|| Arc::new(MemberUsage::default()))
            .clone()
    }

    async fn ensure_rate_buckets(
        &self,
        node_id: &NodeId,
        global_limits: &RelayLimitPolicy,
        member_limits: &RelayLimitPolicy,
    ) {
        let global_rate_bytes_per_sec = relay_rate_bytes_per_sec(global_limits);
        let member_rate_bytes_per_sec = relay_rate_bytes_per_sec(member_limits);
        let mut rate = self.state.rate.write().await;
        rate.global
            .get_or_insert_with(|| TokenBucket::new(global_rate_bytes_per_sec))
            .reconfigure(global_rate_bytes_per_sec);
        rate.members
            .entry(node_id.clone())
            .or_insert_with(|| TokenBucket::new(member_rate_bytes_per_sec))
            .reconfigure(member_rate_bytes_per_sec);
    }

    async fn reserve_packet_bytes(
        &self,
        node_id: &NodeId,
        member: Arc<MemberUsage>,
        bytes: usize,
        global_limits: &RelayLimitPolicy,
        member_limits: &RelayLimitPolicy,
    ) -> std::result::Result<QuotaReservation, String> {
        let bytes = u64::try_from(bytes).map_err(|_| "relay packet size overflow".to_string())?;
        let _window = self.state.quota_window.lock().await;
        let global_forwarded = self.state.metrics.forwarded_bytes.load(Ordering::Acquire);
        let global_reserved = self.state.metrics.reserved_bytes.load(Ordering::Acquire);
        let member_forwarded = member.forwarded_bytes.load(Ordering::Acquire);
        let member_reserved = member.reserved_bytes.load(Ordering::Acquire);
        if !has_byte_capacity(global_forwarded, global_reserved, global_limits.max_bytes)
            || !has_byte_capacity(member_forwarded, member_reserved, member_limits.max_bytes)
            || global_limits.max_bytes.is_some_and(|limit| {
                global_forwarded
                    .saturating_add(global_reserved)
                    .saturating_add(bytes)
                    > limit
            })
            || member_limits.max_bytes.is_some_and(|limit| {
                member_forwarded
                    .saturating_add(member_reserved)
                    .saturating_add(bytes)
                    > limit
            })
        {
            return Err(format!("Relay 流量配额已耗尽: node={node_id}"));
        }

        self.state
            .metrics
            .reserved_bytes
            .fetch_add(bytes, Ordering::AcqRel);
        member.reserved_bytes.fetch_add(bytes, Ordering::AcqRel);
        Ok(QuotaReservation {
            state: self.state.clone(),
            member,
            bytes,
            active: true,
        })
    }

    pub async fn run(self) -> Result<()> {
        tracing::info!("QUIC 中继服务器正在监听: {}", self.bind_addr);

        while let Some(incoming) = self.endpoint.accept().await {
            let Ok(permit) = self.state.connection_tasks.clone().try_acquire_owned() else {
                tracing::warn!(
                    "QUIC 中继连接任务达到上限，拒绝: {}",
                    incoming.remote_address()
                );
                record_rejected_connection(&self.state, None);
                self.record_forwarding_failure();
                self.session_manager.record_relay_error();
                continue;
            };
            if !self.state.listener_open.load(Ordering::Acquire) {
                break;
            }
            let drain = self.state.drain.clone().read_owned().await;
            if !self.state.listener_open.load(Ordering::Acquire) {
                break;
            }
            let server = self.clone();
            tokio::spawn(async move {
                let _permit = permit;
                let _drain = drain;
                let remote_addr = incoming.remote_address();
                match incoming.await {
                    Ok(connection) => {
                        if let Err(err) = server.handle_connection(connection).await {
                            tracing::warn!("QUIC 中继连接处理失败 {remote_addr}: {err:#}");
                        }
                    }
                    Err(err) => {
                        tracing::warn!("QUIC 中继握手失败 {remote_addr}: {err:#}");
                    }
                }
            });
        }

        Ok(())
    }

    async fn handle_connection(&self, connection: quinn::Connection) -> Result<()> {
        let remote = connection.remote_address();
        let connection_id = connection.stable_id();
        tracing::info!("QUIC 中继连接已建立: {}", remote);

        let registered = self
            .register_relay_connection_stream(&connection, connection_id)
            .await?;
        let mut lifecycle = tokio::time::interval(RELAY_CONNECTION_LIFECYCLE_INTERVAL);
        lifecycle.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = lifecycle.tick() => {
                    if let Err(error) = self
                        .enforce_connection_lifecycle(&registered, connection_id)
                        .await
                    {
                        tracing::warn!(
                            client_id = %registered.client_id,
                            virtual_ip = %registered.virtual_ip,
                            error = %error,
                            "QUIC 中继连接生命周期检查失败，主动关闭连接"
                        );
                        connection.close(0u32.into(), b"relay authorization lifecycle");
                        break;
                    }
                }
                stream = connection.accept_bi() => {
                    if !self.accept_relay_data_stream(stream, &registered, connection_id).await {
                            break;
                    }
                }
                closed = connection.closed() => {
                    tracing::debug!("QUIC 中继连接关闭 {}: {closed:#}", registered.client_id);
                    break;
                }
            }
        }

        self.unregister_if_current(registered.virtual_ip, connection_id)
            .await;
        Ok(())
    }

    async fn enforce_connection_lifecycle(
        &self,
        registered: &RegisteredRelayClient,
        connection_id: usize,
    ) -> Result<()> {
        let now = now_unix_ms();
        let client = self
            .lookup_client(registered.virtual_ip)
            .await
            .ok_or_else(|| anyhow::anyhow!("Relay 连接注册已失效"))?;
        if client.connection_id != connection_id
            || client.client_id != registered.client_id
            || client.node_id != registered.node_id
        {
            anyhow::bail!("Relay 连接已被替换");
        }

        client
            .member_grant
            .validate_at(now)
            .context("Relay 使用授权已过期或无效")?;
        match self
            .session_manager
            .is_registration_revoked(&client.client_id, &client.public_key)
            .await
        {
            Ok(false) => {}
            Ok(true) => anyhow::bail!("Relay 客户端身份已撤销"),
            Err(error) => {
                anyhow::bail!("Relay 授权撤销目录查询失败，拒绝保持连接: {error:#}")
            }
        }
        if self
            .session_manager
            .is_blacklisted(&client.client_id, &client.public_key)
            .await
        {
            anyhow::bail!("Relay 客户端身份已加入黑名单");
        }
        if !self
            .session_manager
            .is_data_plane_member(&client.public_key)
            .context("Relay 数据面成员检查失败")?
        {
            anyhow::bail!("Relay 客户端已不是当前数据面成员");
        }

        let policy = self.load_policy(now).await?;
        match policy.existing_session_policy {
            RelayExistingSessionPolicy::Continue => {}
            RelayExistingSessionPolicy::Reauthorize => {
                let (authorized_policy, decision) = self
                    .policy_authority
                    .authorize_grant(&client.member_grant, now)
                    .context("Relay 实时授权检查失败")?;
                if !decision.allowed {
                    anyhow::bail!(
                        "Relay 实时授权已拒绝: {}",
                        decision
                            .final_reason
                            .map(|reason| format!("{reason:?}"))
                            .unwrap_or_else(|| "unknown".into())
                    );
                }
                if authorized_policy.revision != policy.revision
                    || authorized_policy.network_id != policy.network_id
                    || authorized_policy.provider_node_id != policy.provider_node_id
                {
                    anyhow::bail!("Relay 实时授权上下文与当前策略不一致");
                }
                if decision
                    .limits
                    .max_session_duration_secs
                    .is_some_and(|limit| {
                        client.registered_at.elapsed() >= Duration::from_secs(limit)
                    })
                {
                    anyhow::bail!("Relay 会话时长配额已耗尽");
                }
            }
            RelayExistingSessionPolicy::Migrate | RelayExistingSessionPolicy::Terminate => {
                if client.policy_revision != policy.revision {
                    anyhow::bail!(
                        "Relay 策略 revision 已变化，关闭既有会话以执行 {:?}",
                        policy.existing_session_policy
                    );
                }
                if client
                    .effective_limits
                    .max_session_duration_secs
                    .is_some_and(|limit| {
                        client.registered_at.elapsed() >= Duration::from_secs(limit)
                    })
                {
                    anyhow::bail!("Relay 会话时长配额已耗尽");
                }
            }
        }
        Ok(())
    }

    async fn register_relay_connection_stream(
        &self,
        connection: &quinn::Connection,
        connection_id: usize,
    ) -> Result<RegisteredRelayClient> {
        let (mut send, mut recv) = connection
            .accept_bi()
            .await
            .context("QUIC 中继注册流打开失败")?;
        let (stream_id, message) = recv_relay_message(&mut recv)
            .await
            .context("QUIC 中继注册消息读取失败")?;
        if stream_id != RELAY_REGISTER_STREAM_ID {
            anyhow::bail!("QUIC 中继注册 stream_id 无效: {}", stream_id);
        }

        match self
            .register_connection(connection.clone(), connection_id, message)
            .await
        {
            Ok(registered) => {
                let ack = RelayMessage::RegisterAck {
                    accepted: true,
                    message: "registered".to_string(),
                };
                send_relay_message(&mut send, RELAY_REGISTER_STREAM_ID, &ack).await?;
                Ok(registered)
            }
            Err(err) => {
                let ack = RelayMessage::RegisterAck {
                    accepted: false,
                    message: err.to_string(),
                };
                if send_relay_message(&mut send, RELAY_REGISTER_STREAM_ID, &ack)
                    .await
                    .is_ok()
                {
                    // `send_relay_message` finishes the stream. Wait until the peer's QUIC
                    // stack acknowledges the rejection frame before this connection handle is
                    // dropped; otherwise the client can observe CONNECTION_CLOSE first and
                    // never surface the concrete registration error.
                    let _ = tokio::time::timeout(Duration::from_secs(1), send.stopped()).await;
                }
                Err(err)
            }
        }
    }

    async fn accept_relay_data_stream(
        &self,
        stream: Result<(quinn::SendStream, quinn::RecvStream), quinn::ConnectionError>,
        registered: &RegisteredRelayClient,
        connection_id: usize,
    ) -> bool {
        let (send, recv) = match stream {
            Ok(stream) => stream,
            Err(err) => {
                tracing::debug!("QUIC 中继连接数据流结束 {}: {err:#}", registered.client_id);
                return false;
            }
        };

        let Ok(permit) = self.state.stream_tasks.clone().try_acquire_owned() else {
            tracing::warn!(
                "QUIC 中继数据流任务达到上限，拒绝: {}",
                registered.client_id
            );
            self.reject_overloaded_stream(send).await;
            let member = self.member_usage(&registered.node_id).await;
            record_rejected_packet(&self.state, &member);
            self.record_forwarding_failure();
            self.session_manager.record_relay_error();
            return true;
        };

        if !self.state.listener_open.load(Ordering::Acquire) {
            return false;
        }
        let drain = self.state.drain.clone().read_owned().await;
        if !self.state.listener_open.load(Ordering::Acquire) {
            return false;
        }
        let server = self.clone();
        let registered = registered.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let _drain = drain;
            if let Err(err) = server
                .handle_incoming_stream(&registered, connection_id, send, recv)
                .await
            {
                tracing::warn!("QUIC 中继数据流处理失败: {err:#}");
            }
        });
        true
    }

    async fn register_connection(
        &self,
        connection: quinn::Connection,
        connection_id: usize,
        message: RelayMessage,
    ) -> Result<RegisteredRelayClient> {
        let RelayMessage::Register {
            client_id,
            virtual_ip,
            public_key,
            timestamp,
            server_public_key_pin,
            client_certificate_public_key_pin,
            tls_channel_binding,
            signature,
            member_grant,
        } = message
        else {
            anyhow::bail!("QUIC 中继首个消息必须是 Register");
        };

        let registration = RelayRegistrationBinding {
            client_id: &client_id,
            virtual_ip,
            public_key: &public_key,
            timestamp,
            server_public_key_pin,
            client_certificate_public_key_pin,
            tls_channel_binding,
        };
        let actual_channel_binding = export_quic_relay_channel_binding(&connection)?;
        verify_quic_client_identity_binding(&connection, &registration)?;
        verify_relay_registration(
            &registration,
            &signature,
            self.certificate.public_key_pin,
            &actual_channel_binding,
        )?;
        self.verify_online_peer(&client_id, virtual_ip, &public_key)
            .await?;

        let node_id = NodeId::from_public_key(&public_key);
        let now = now_unix_ms();
        let Some(member_grant) = member_grant.map(|grant| *grant) else {
            let member_usage = self.member_usage(&node_id).await;
            record_rejected_connection(&self.state, Some(&member_usage));
            anyhow::bail!("Relay 注册缺少使用方签名授权");
        };
        let grant_public_key = member_grant
            .consumer_public_key
            .to_bytes()
            .context("Relay 使用方授权公钥无效")?;
        if grant_public_key.as_slice() != public_key.as_slice() {
            let member_usage = self.member_usage(&node_id).await;
            record_rejected_connection(&self.state, Some(&member_usage));
            anyhow::bail!("Relay 使用方授权公钥与注册身份不一致");
        }
        let (policy, decision) = match self.policy_authority.authorize_grant(&member_grant, now) {
            Ok(result) => result,
            Err(error) => {
                let member_usage = self.member_usage(&node_id).await;
                record_rejected_connection(&self.state, Some(&member_usage));
                return Err(error.context("Relay 签名授权校验失败，拒绝注册"));
            }
        };
        if !decision.allowed {
            let member_usage = self.member_usage(&node_id).await;
            record_rejected_connection(&self.state, Some(&member_usage));
            return Err(anyhow::anyhow!(
                "Relay 注册未同时获得提供方和使用方授权: {}",
                decision
                    .final_reason
                    .map(|reason| format!("{reason:?}"))
                    .unwrap_or_else(|| "unknown".into())
            ));
        }

        let relay_client = RelayClient {
            client_id: client_id.clone(),
            node_id: node_id.clone(),
            public_key: public_key.clone(),
            member_grant,
            effective_limits: decision.limits.clone(),
            policy_revision: policy.revision,
            connection,
            connection_id,
            registered_at: Instant::now(),
        };
        let member_usage = self.member_usage(&node_id).await;
        let registration = self.state.registration_gate.lock().await;
        if !self.state.listener_open.load(Ordering::Acquire) {
            record_rejected_connection(&self.state, Some(&member_usage));
            return Err(anyhow::anyhow!("Relay 已停止，拒绝新连接"));
        }

        let existing_node = self
            .state
            .clients
            .read()
            .await
            .get(&virtual_ip)
            .map(|client| client.node_id.clone());
        let projected_global = self
            .state
            .metrics
            .active_connections
            .load(Ordering::Acquire)
            .saturating_add(1)
            .saturating_sub(u32::from(existing_node.is_some()));
        let projected_member = member_usage
            .active_connections
            .load(Ordering::Acquire)
            .saturating_add(1)
            .saturating_sub(u32::from(existing_node.as_ref() == Some(&node_id)));

        if exceeds_limit(projected_global, policy.limits.max_connections)
            || exceeds_limit(projected_member, decision.limits.max_connections)
        {
            record_rejected_connection(&self.state, Some(&member_usage));
            return Err(anyhow::anyhow!("Relay 连接配额已耗尽"));
        }
        if !has_byte_capacity(
            self.state.metrics.forwarded_bytes.load(Ordering::Acquire),
            self.state.metrics.reserved_bytes.load(Ordering::Acquire),
            policy.limits.max_bytes,
        ) || !has_byte_capacity(
            member_usage.forwarded_bytes.load(Ordering::Acquire),
            member_usage.reserved_bytes.load(Ordering::Acquire),
            decision.limits.max_bytes,
        ) {
            record_rejected_connection(&self.state, Some(&member_usage));
            return Err(anyhow::anyhow!("Relay 流量配额已耗尽"));
        }

        let replaced = self
            .state
            .clients
            .write()
            .await
            .insert(virtual_ip, relay_client);
        if let Some(replaced) = replaced {
            let replaced_member = self.member_usage(&replaced.node_id).await;
            decrement_counter(
                &self.state.metrics.active_connections,
                &replaced_member.active_connections,
            );
        }
        self.state
            .metrics
            .active_connections
            .fetch_add(1, Ordering::AcqRel);
        member_usage
            .active_connections
            .fetch_add(1, Ordering::AcqRel);
        self.ensure_rate_buckets(&node_id, &policy.limits, &decision.limits)
            .await;
        drop(registration);

        tracing::info!("QUIC 中继客户端注册成功: {} ({})", client_id, virtual_ip);

        Ok(RegisteredRelayClient {
            client_id,
            node_id,
            virtual_ip,
            public_key,
        })
    }

    async fn verify_online_peer(
        &self,
        client_id: &ClientId,
        virtual_ip: Ipv6Addr,
        public_key: &[u8],
    ) -> Result<()> {
        match self
            .session_manager
            .is_registration_revoked(client_id, public_key)
            .await
        {
            Ok(false) => {}
            Ok(true) => {
                anyhow::bail!("客户端身份已撤销，拒绝 QUIC 中继注册: {client_id}");
            }
            Err(error) => {
                anyhow::bail!("授权撤销目录查询失败，拒绝 QUIC 中继注册 {client_id}: {error:#}");
            }
        }

        if self
            .session_manager
            .is_blacklisted(client_id, public_key)
            .await
        {
            anyhow::bail!("客户端身份已撤销，拒绝 QUIC 中继注册: {client_id}");
        }

        if !self
            .session_manager
            .is_data_plane_member(public_key)
            .context("数据面成员名册查询失败，拒绝 QUIC 中继注册")?
        {
            anyhow::bail!("节点不是当前数据面成员，拒绝 QUIC 中继注册: {client_id}");
        }

        let peer = self
            .session_manager
            .get_peer_info(client_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("客户端未在信令服务注册: {client_id}"))?;

        if peer.virtual_ip != virtual_ip {
            anyhow::bail!(
                "QUIC 中继虚拟 IP 不匹配: register={}, signaling={}",
                virtual_ip,
                peer.virtual_ip
            );
        }

        if peer.public_key != public_key {
            anyhow::bail!("QUIC 中继公钥与信令注册信息不匹配: {client_id}");
        }

        Ok(())
    }

    async fn handle_incoming_stream(
        &self,
        registered: &RegisteredRelayClient,
        src_connection_id: usize,
        mut send: quinn::SendStream,
        mut recv: quinn::RecvStream,
    ) -> Result<()> {
        let request = match self.read_stream_request(&mut recv).await? {
            Ok(request) => request,
            Err((stream_id, error)) => {
                self.send_relay_error_response(&mut send, stream_id, error)
                    .await?;
                return Ok(());
            }
        };
        let request = match request {
            RelayStreamRequest::Packet(request) => request,
            RelayStreamRequest::UpdateAuthorization {
                stream_id,
                member_grant,
            } => {
                let ack = match self
                    .apply_authorization_update(
                        registered,
                        src_connection_id,
                        member_grant.as_ref(),
                    )
                    .await
                {
                    Ok(()) => RelayMessage::AuthorizationAck {
                        accepted: true,
                        message: "authorization updated".into(),
                    },
                    Err(error) => RelayMessage::AuthorizationAck {
                        accepted: false,
                        message: error.to_string(),
                    },
                };
                send_relay_message(&mut send, stream_id, &ack).await?;
                return Ok(());
            }
        };

        let (dst_client, _stream_guard, reservation) = match self
            .validate_packet_request(
                &request,
                &registered.client_id,
                &registered.node_id,
                &registered.public_key,
                registered.virtual_ip,
                src_connection_id,
            )
            .await
        {
            Ok(validated) => validated,
            Err(error) => {
                self.send_relay_error_response(&mut send, request.stream_id, *error)
                    .await?;
                return Ok(());
            }
        };

        let packet = RelayMessage::Packet {
            src_ip: request.src_ip,
            dst_ip: request.dst_ip,
            payload: request.payload,
        };
        if let Err(err) = self
            .forward_to_client(&dst_client, request.stream_id, &packet)
            .await
        {
            let error = RelayMessage::error(
                ErrorCode::BadGateway,
                format!("relay forwarding failed: {err:#}"),
            );
            send_relay_message(&mut send, request.stream_id, &error).await?;
            return Ok(());
        }
        reservation.commit();

        let ack = RelayMessage::RegisterAck {
            accepted: true,
            message: "forwarded".to_string(),
        };
        send_relay_message(&mut send, request.stream_id, &ack).await?;

        Ok(())
    }

    async fn read_stream_request(
        &self,
        recv: &mut quinn::RecvStream,
    ) -> Result<std::result::Result<RelayStreamRequest, (u64, RelayMessage)>> {
        let (stream_id, message) = recv_relay_message(recv)
            .await
            .context("无法读取 QUIC 中继数据流消息")?;

        match message {
            RelayMessage::Packet {
                src_ip,
                dst_ip,
                payload,
            } => Ok(Ok(RelayStreamRequest::Packet(RelayPacketRequest {
                stream_id,
                src_ip,
                dst_ip,
                payload,
            }))),
            RelayMessage::UpdateAuthorization { member_grant }
                if stream_id == RELAY_AUTHORIZATION_STREAM_ID =>
            {
                Ok(Ok(RelayStreamRequest::UpdateAuthorization {
                    stream_id,
                    member_grant,
                }))
            }
            RelayMessage::UpdateAuthorization { .. } => Ok(Err((
                stream_id,
                RelayMessage::error(
                    ErrorCode::BadRequest,
                    "authorization update used an unexpected stream id",
                ),
            ))),
            _ => Ok(Err((
                stream_id,
                RelayMessage::error(
                    ErrorCode::BadRequest,
                    "expected Packet or authorization update",
                ),
            ))),
        }
    }

    async fn apply_authorization_update(
        &self,
        registered: &RegisteredRelayClient,
        src_connection_id: usize,
        member_grant: &RelayMemberGrant,
    ) -> Result<()> {
        if member_grant.consumer_node_id != registered.node_id {
            anyhow::bail!("Relay 授权更新属于其他消费者节点");
        }
        let grant_public_key = member_grant
            .consumer_public_key
            .to_bytes()
            .context("Relay 授权更新公钥无效")?;
        if grant_public_key.as_slice() != registered.public_key.as_slice() {
            anyhow::bail!("Relay 授权更新公钥与当前连接不一致");
        }

        let now = now_unix_ms();
        let (policy, decision) = self
            .policy_authority
            .authorize_grant(member_grant, now)
            .context("Relay 授权更新校验失败")?;
        if !decision.allowed {
            anyhow::bail!(
                "Relay 授权更新未同时获得提供方和使用方授权: {}",
                decision
                    .final_reason
                    .map(|reason| format!("{reason:?}"))
                    .unwrap_or_else(|| "unknown".into())
            );
        }

        let registration = self.state.registration_gate.lock().await;
        if !self.state.listener_open.load(Ordering::Acquire) {
            anyhow::bail!("Relay 已停止，拒绝授权更新");
        }
        {
            let mut clients = self.state.clients.write().await;
            let current = clients
                .get_mut(&registered.virtual_ip)
                .ok_or_else(|| anyhow::anyhow!("Relay 连接注册已失效"))?;
            if current.connection_id != src_connection_id
                || current.client_id != registered.client_id
                || current.node_id != registered.node_id
            {
                anyhow::bail!("Relay 连接已被替换，拒绝旧连接更新授权");
            }
            if member_grant.issued_at_unix_ms < current.member_grant.issued_at_unix_ms
                || member_grant.expires_at_unix_ms <= current.member_grant.expires_at_unix_ms
                || member_grant.nonce == current.member_grant.nonce
            {
                anyhow::bail!("Relay 授权更新未延长有效期或复用了 nonce");
            }
            current.member_grant = member_grant.clone();
            current.effective_limits = decision.limits.clone();
            current.policy_revision = policy.revision;
        }
        drop(registration);

        self.ensure_rate_buckets(&registered.node_id, &policy.limits, &decision.limits)
            .await;
        tracing::info!(
            client_id = %registered.client_id,
            revision = policy.revision,
            expires_at_unix_ms = member_grant.expires_at_unix_ms,
            "QUIC 中继授权已更新"
        );
        Ok(())
    }

    async fn ensure_packet_authorized(
        &self,
        client_id: &ClientId,
        public_key: &[u8],
        direction: &'static str,
    ) -> std::result::Result<(), Box<RelayMessage>> {
        match self
            .session_manager
            .is_registration_revoked(client_id, public_key)
            .await
        {
            Ok(false) => {}
            Ok(true) => {
                return Err(Box::new(RelayMessage::error(
                    ErrorCode::Forbidden,
                    format!("{direction} client authorization has been revoked"),
                )));
            }
            Err(error) => {
                tracing::error!(
                    "QUIC 中继授权撤销目录查询失败，拒绝{direction}发包 {client_id}: {error:#}"
                );
                return Err(Box::new(RelayMessage::error(
                    ErrorCode::Internal,
                    "authorization revocation check failed",
                )));
            }
        }

        match self.session_manager.is_data_plane_member(public_key) {
            Ok(true) => Ok(()),
            Ok(false) => Err(Box::new(RelayMessage::error(
                ErrorCode::Forbidden,
                format!("{direction} client is not a current data-plane member"),
            ))),
            Err(error) => {
                tracing::error!(
                    "数据面成员名册查询失败，拒绝{direction}发包 {client_id}: {error:#}"
                );
                Err(Box::new(RelayMessage::error(
                    ErrorCode::Internal,
                    "data-plane membership check failed",
                )))
            }
        }
    }

    async fn validate_packet_request(
        &self,
        request: &RelayPacketRequest,
        src_client_id: &ClientId,
        src_node_id: &NodeId,
        src_public_key: &[u8],
        registered_src_ip: Ipv6Addr,
        src_connection_id: usize,
    ) -> std::result::Result<(RelayClient, RelayStreamGuard, QuotaReservation), Box<RelayMessage>>
    {
        let src_usage = self.member_usage(src_node_id).await;
        let reject = |message: RelayMessage| {
            record_rejected_packet(&self.state, &src_usage);
            Box::new(message)
        };

        let now = now_unix_ms();
        let policy = match self.load_policy(now).await {
            Ok(policy) => policy,
            Err(error) => {
                tracing::error!("Relay 数据面包策略不可用: {error:#}");
                return Err(reject(RelayMessage::error(
                    ErrorCode::Internal,
                    "relay policy unavailable",
                )));
            }
        };

        if request.payload.len() > MAX_SIGNALING_RELAY_PAYLOAD_SIZE {
            return Err(reject(RelayMessage::error(
                ErrorCode::BadRequest,
                "relay payload exceeds the protocol limit",
            )));
        }

        if request.src_ip != registered_src_ip {
            return Err(reject(RelayMessage::error(
                ErrorCode::Forbidden,
                "source IP does not match relay registration",
            )));
        }

        let Some(src_client) = self.lookup_client(registered_src_ip).await else {
            return Err(reject(RelayMessage::error(
                ErrorCode::Forbidden,
                "source relay registration is no longer active",
            )));
        };
        if src_client.connection_id != src_connection_id
            || src_client.client_id != *src_client_id
            || src_client.node_id != *src_node_id
        {
            return Err(reject(RelayMessage::error(
                ErrorCode::Forbidden,
                "source relay connection has been replaced",
            )));
        }

        self.ensure_packet_authorized(src_client_id, src_public_key, "source")
            .await
            .inspect_err(|_| {
                record_rejected_packet(&self.state, &src_usage);
            })?;
        if self
            .session_manager
            .is_blacklisted(src_client_id, src_public_key)
            .await
        {
            return Err(reject(RelayMessage::error(
                ErrorCode::Forbidden,
                "source client identity is blacklisted",
            )));
        }

        let Some(dst_client) = self.lookup_client(request.dst_ip).await else {
            return Err(reject(RelayMessage::error(
                ErrorCode::NotFound,
                format!(
                    "destination is not connected to QUIC relay: {}",
                    request.dst_ip
                ),
            )));
        };

        if dst_client.connection_id == src_connection_id && request.src_ip == request.dst_ip {
            return Err(reject(RelayMessage::error(
                ErrorCode::BadRequest,
                "refusing relay packet to self",
            )));
        }

        self.ensure_packet_authorized(&dst_client.client_id, &dst_client.public_key, "destination")
            .await
            .inspect_err(|_| {
                record_rejected_packet(&self.state, &src_usage);
            })?;
        if self
            .session_manager
            .is_blacklisted(&dst_client.client_id, &dst_client.public_key)
            .await
        {
            return Err(reject(RelayMessage::error(
                ErrorCode::Forbidden,
                "destination client identity is blacklisted",
            )));
        }

        let (global_limits, source_limits) = match policy.existing_session_policy {
            RelayExistingSessionPolicy::Continue => {
                if let Some(max_duration) = src_client.effective_limits.max_session_duration_secs {
                    if src_client.registered_at.elapsed() >= Duration::from_secs(max_duration) {
                        return Err(reject(RelayMessage::error(
                            ErrorCode::Forbidden,
                            "relay session duration limit exceeded",
                        )));
                    }
                }
                if let Some(max_duration) = dst_client.effective_limits.max_session_duration_secs {
                    if dst_client.registered_at.elapsed() >= Duration::from_secs(max_duration) {
                        return Err(reject(RelayMessage::error(
                            ErrorCode::Forbidden,
                            "destination relay session duration limit exceeded",
                        )));
                    }
                }
                if src_client.policy_revision != policy.revision {
                    tracing::info!(
                        source_revision = src_client.policy_revision,
                        current_revision = policy.revision,
                        "Relay 策略已变化，但 Continue 策略保留现有会话"
                    );
                }
                (policy.limits.clone(), src_client.effective_limits.clone())
            }
            RelayExistingSessionPolicy::Reauthorize => {
                let (source_policy, source_decision) = match self
                    .policy_authority
                    .authorize_grant(&src_client.member_grant, now)
                {
                    Ok(result) => result,
                    Err(error) => {
                        tracing::error!("Relay 源授权计算失败: {error}");
                        return Err(reject(RelayMessage::error(
                            ErrorCode::Internal,
                            "relay source authorization failed",
                        )));
                    }
                };
                let (destination_policy, destination_decision) = match self
                    .policy_authority
                    .authorize_grant(&dst_client.member_grant, now)
                {
                    Ok(result) => result,
                    Err(error) => {
                        tracing::error!("Relay 目标授权计算失败: {error}");
                        return Err(reject(RelayMessage::error(
                            ErrorCode::Internal,
                            "relay destination authorization failed",
                        )));
                    }
                };
                if !source_decision.allowed || !destination_decision.allowed {
                    tracing::warn!(
                        "Relay 实时授权拒绝: source={} destination={} provider_reason={:?} consumer_reason={:?}",
                        src_node_id,
                        dst_client.node_id,
                        source_decision.final_reason,
                        destination_decision.final_reason
                    );
                    return Err(reject(RelayMessage::error(
                        ErrorCode::Forbidden,
                        "relay authorization changed",
                    )));
                }
                if source_policy.network_id != destination_policy.network_id
                    || source_policy.provider_node_id != destination_policy.provider_node_id
                {
                    return Err(reject(RelayMessage::error(
                        ErrorCode::Forbidden,
                        "relay authorization contexts do not share a provider",
                    )));
                }
                if let Some(max_duration) = source_decision.limits.max_session_duration_secs {
                    if src_client.registered_at.elapsed() >= Duration::from_secs(max_duration) {
                        return Err(reject(RelayMessage::error(
                            ErrorCode::Forbidden,
                            "relay session duration limit exceeded",
                        )));
                    }
                }
                if let Some(max_duration) = destination_decision.limits.max_session_duration_secs {
                    if dst_client.registered_at.elapsed() >= Duration::from_secs(max_duration) {
                        return Err(reject(RelayMessage::error(
                            ErrorCode::Forbidden,
                            "destination relay session duration limit exceeded",
                        )));
                    }
                }
                (source_policy.limits, source_decision.limits)
            }
            RelayExistingSessionPolicy::Migrate | RelayExistingSessionPolicy::Terminate => {
                return Err(reject(RelayMessage::error(
                    ErrorCode::Forbidden,
                    "existing relay session policy requires resumption on another Relay",
                )));
            }
        };

        let _stream_guard = match RelayStreamGuard::acquire(
            self.state.clone(),
            src_usage.clone(),
            global_limits.max_concurrent_streams,
            source_limits.max_concurrent_streams,
        ) {
            Ok(guard) => guard,
            Err(()) => {
                return Err(reject(RelayMessage::error(
                    ErrorCode::Overloaded,
                    "relay stream concurrency limit exceeded",
                )));
            }
        };

        let reservation = match self
            .reserve_packet_bytes(
                src_node_id,
                src_usage.clone(),
                request.payload.len(),
                &global_limits,
                &source_limits,
            )
            .await
        {
            Ok(reservation) => reservation,
            Err(message) => {
                return Err(reject(RelayMessage::error(ErrorCode::Overloaded, message)))
            }
        };

        if !self
            .allow_relay(
                src_node_id,
                request.payload.len(),
                &global_limits,
                &source_limits,
            )
            .await
        {
            return Err(reject(RelayMessage::error(
                ErrorCode::RateLimited,
                "relay bandwidth limit exceeded",
            )));
        }

        Ok((dst_client, _stream_guard, reservation))
    }

    async fn send_relay_error_response(
        &self,
        send: &mut quinn::SendStream,
        stream_id: u64,
        error: RelayMessage,
    ) -> Result<()> {
        send_relay_message(send, stream_id, &error).await?;
        self.record_forwarding_failure();
        self.session_manager.record_relay_error();
        Ok(())
    }

    fn record_forwarding_failure(&self) {
        self.state
            .metrics
            .last_forwarding_failure_at_unix_ms
            .fetch_max(now_unix_ms(), Ordering::AcqRel);
    }

    async fn lookup_client(&self, dst_ip: Ipv6Addr) -> Option<RelayClient> {
        self.state.clients.read().await.get(&dst_ip).cloned()
    }

    async fn allow_relay(
        &self,
        node_id: &NodeId,
        bytes: usize,
        global_limits: &RelayLimitPolicy,
        member_limits: &RelayLimitPolicy,
    ) -> bool {
        let global_rate_bytes_per_sec = relay_rate_bytes_per_sec(global_limits);
        let member_rate_bytes_per_sec = relay_rate_bytes_per_sec(member_limits);
        let mut rate = self.state.rate.write().await;
        let RelayRateState { global, members } = &mut *rate;
        global
            .get_or_insert_with(|| TokenBucket::new(global_rate_bytes_per_sec))
            .reconfigure(global_rate_bytes_per_sec);
        members
            .entry(node_id.clone())
            .or_insert_with(|| TokenBucket::new(member_rate_bytes_per_sec))
            .reconfigure(member_rate_bytes_per_sec);
        let global = global.as_mut().expect("global relay rate bucket");
        let member = members.get_mut(node_id).expect("member relay rate bucket");
        if !global.has_capacity(bytes) || !member.has_capacity(bytes) {
            return false;
        }
        global.consume(bytes);
        member.consume(bytes);
        true
    }

    async fn forward_to_client(
        &self,
        dst_client: &RelayClient,
        stream_id: u64,
        message: &RelayMessage,
    ) -> Result<()> {
        let (mut send, mut recv) = dst_client
            .connection
            .open_bi()
            .await
            .with_context(|| format!("无法打开目标 QUIC 中继流: {}", dst_client.client_id))?;
        send_relay_message(&mut send, stream_id, message).await?;

        match tokio::time::timeout(Duration::from_secs(3), recv_relay_message(&mut recv)).await {
            Ok(Ok((_, RelayMessage::RegisterAck { accepted: true, .. }))) => {
                self.session_manager.record_relay_forward(match message {
                    RelayMessage::Packet { payload, .. } => payload.len(),
                    _ => 0,
                });
                Ok(())
            }
            Ok(Ok((_, RelayMessage::Error { code, message }))) => {
                self.record_forwarding_failure();
                self.session_manager.record_relay_error();
                anyhow::bail!("目标客户端拒绝 relay 包: {code} {message}")
            }
            Ok(Ok((_, other))) => {
                self.record_forwarding_failure();
                self.session_manager.record_relay_error();
                anyhow::bail!("目标客户端返回了非预期 relay 响应: {:?}", other)
            }
            Ok(Err(err)) => {
                self.record_forwarding_failure();
                self.session_manager.record_relay_error();
                Err(err).context("目标客户端 relay 响应读取失败")
            }
            Err(_) => {
                self.record_forwarding_failure();
                self.session_manager.record_relay_error();
                anyhow::bail!("目标客户端 relay 响应超时")
            }
        }
    }

    async fn reject_overloaded_stream(&self, mut send: quinn::SendStream) {
        let error = RelayMessage::error(
            ErrorCode::Overloaded,
            "relay stream concurrency limit exceeded",
        );
        let _ = send_relay_message(&mut send, 0, &error).await;
    }

    async fn unregister_if_current(&self, virtual_ip: Ipv6Addr, connection_id: usize) {
        let removed = {
            let mut clients = self.state.clients.write().await;
            if clients
                .get(&virtual_ip)
                .is_some_and(|client| client.connection_id == connection_id)
            {
                clients.remove(&virtual_ip)
            } else {
                None
            }
        };
        if let Some(client) = removed {
            let member = self.member_usage(&client.node_id).await;
            decrement_counter(
                &self.state.metrics.active_connections,
                &member.active_connections,
            );
            tracing::info!("QUIC 中继客户端已注销: {}", virtual_ip);
        }
    }

    #[cfg(test)]
    fn new_test_with_limits(
        bind_addr: SocketAddr,
        session_manager: Arc<SessionManager>,
        policy_authority: Arc<dyn RelayPolicyAuthority>,
        certificate: QuicCertificate,
        max_connection_tasks: usize,
        max_stream_tasks: usize,
    ) -> Result<Self> {
        let mut server = Self::bind(bind_addr, session_manager, policy_authority, certificate)?;
        server.state = Arc::new(RelayState {
            clients: RwLock::new(HashMap::new()),
            rate: RwLock::new(RelayRateState::default()),
            members: RwLock::new(HashMap::new()),
            metrics: RelayMetrics::default(),
            registration_gate: Mutex::new(()),
            drain: Arc::new(RwLock::new(())),
            quota_window: Mutex::new(QuotaWindow::default()),
            policy_revision: AtomicU64::new(0),
            listener_open: AtomicBool::new(true),
            connection_tasks: Arc::new(Semaphore::new(max_connection_tasks)),
            stream_tasks: Arc::new(Semaphore::new(max_stream_tasks)),
        });
        Ok(server)
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn nonzero_atomic(value: &AtomicU64) -> Option<u64> {
    let value = value.load(Ordering::Acquire);
    (value != 0).then_some(value)
}

fn relay_rate_bytes_per_sec(limits: &RelayLimitPolicy) -> u64 {
    let configured = limits
        .max_bandwidth_kbps
        .unwrap_or(RELAY_HARD_BANDWIDTH_LIMIT_BPS / 1_000)
        .saturating_mul(125);
    configured.clamp(1, RELAY_HARD_BANDWIDTH_LIMIT_BPS / 8)
}

fn try_increment_bounded(counter: &AtomicU32, limit: Option<u32>) -> std::result::Result<(), ()> {
    let mut current = counter.load(Ordering::Acquire);
    loop {
        if limit.is_some_and(|limit| current >= limit) {
            return Err(());
        }
        match counter.compare_exchange_weak(
            current,
            current.saturating_add(1),
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return Ok(()),
            Err(observed) => current = observed,
        }
    }
}

fn exceeds_limit(value: u32, limit: Option<u32>) -> bool {
    limit.is_some_and(|limit| value > limit)
}

fn has_byte_capacity(forwarded: u64, reserved: u64, limit: Option<u64>) -> bool {
    limit.is_none_or(|limit| forwarded.saturating_add(reserved) < limit)
}

fn saturating_sub(counter: &AtomicU32, value: u32) {
    let _ = counter.try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
        Some(current.saturating_sub(value))
    });
}

fn saturating_sub_u64(counter: &AtomicU64, value: u64) {
    let _ = counter.try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
        Some(current.saturating_sub(value))
    });
}

fn decrement_counter(counter: &AtomicU32, member_counter: &AtomicU32) {
    let _ = counter.try_update(Ordering::AcqRel, Ordering::Acquire, |value| {
        Some(value.saturating_sub(1))
    });
    let _ = member_counter.try_update(Ordering::AcqRel, Ordering::Acquire, |value| {
        Some(value.saturating_sub(1))
    });
}

fn record_rejected_connection(state: &RelayState, member: Option<&MemberUsage>) {
    state
        .metrics
        .rejected_connections
        .fetch_add(1, Ordering::Relaxed);
    if let Some(member) = member {
        member.rejected_connections.fetch_add(1, Ordering::Relaxed);
    }
}

fn record_rejected_packet(state: &RelayState, member: &MemberUsage) {
    state
        .metrics
        .rejected_packets
        .fetch_add(1, Ordering::Relaxed);
    member.rejected_packets.fetch_add(1, Ordering::Relaxed);
}

fn reset_metrics(metrics: &RelayMetrics) {
    metrics.active_connections.store(0, Ordering::Release);
    metrics.active_streams.store(0, Ordering::Release);
    metrics.forwarded_packets.store(0, Ordering::Release);
    metrics.forwarded_bytes.store(0, Ordering::Release);
    metrics.rejected_packets.store(0, Ordering::Release);
    metrics.rejected_connections.store(0, Ordering::Release);
    metrics.reserved_bytes.store(0, Ordering::Release);
    metrics.forwarding_observed.store(false, Ordering::Release);
    metrics
        .last_forwarded_at_unix_ms
        .store(0, Ordering::Release);
    metrics
        .last_forwarding_failure_at_unix_ms
        .store(0, Ordering::Release);
    metrics.quota_started_at_unix_ms.store(0, Ordering::Release);
    metrics.quota_reset_at_unix_ms.store(0, Ordering::Release);
}

async fn reset_quota_metrics(state: &RelayState) {
    state.metrics.forwarded_packets.store(0, Ordering::Release);
    state.metrics.forwarded_bytes.store(0, Ordering::Release);
    state.metrics.rejected_packets.store(0, Ordering::Release);
    state
        .metrics
        .rejected_connections
        .store(0, Ordering::Release);
    for member in state.members.read().await.values() {
        member.forwarded_packets.store(0, Ordering::Release);
        member.forwarded_bytes.store(0, Ordering::Release);
        member.rejected_packets.store(0, Ordering::Release);
        member.rejected_connections.store(0, Ordering::Release);
    }
}

#[derive(Debug, Clone)]
struct RegisteredRelayClient {
    client_id: ClientId,
    node_id: NodeId,
    virtual_ip: Ipv6Addr,
    public_key: Vec<u8>,
}

fn verify_relay_registration(
    registration: &RelayRegistrationBinding<'_>,
    signature: &[u8],
    expected_server_public_key_pin: [u8; 32],
    actual_channel_binding: &[u8; 32],
) -> Result<()> {
    if registration.server_public_key_pin != expected_server_public_key_pin {
        anyhow::bail!("QUIC 中继注册绑定了错误的服务端公钥 pin");
    }
    if &registration.tls_channel_binding != actual_channel_binding {
        anyhow::bail!("QUIC 中继注册绑定了其他 TLS 会话");
    }

    let challenge = registration.challenge();
    if Identity::verify_peer_fingerprint(registration.public_key, registration.client_id)
        && Identity::verify(registration.public_key, &challenge, signature)
    {
        Ok(())
    } else {
        anyhow::bail!("QUIC 中继注册签名无效")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signaling::blacklist::BlacklistStore;
    use fuckipv6_common::domain::{
        NetworkId, NetworkRole, PermissionCondition, PermissionEffect, PermissionRule,
        PolicyAction, RelayHealthIssue, RelayHealthStatus, RelayMemberPolicyContext, ResourceKind,
        SecurityRuleId, SubjectSelector,
    };
    use fuckipv6_common::relay_client::QuicRelayClient;
    use fuckipv6_common::{
        build_pinned_quic_client_config_with_auth, export_quic_relay_channel_binding,
        recv_relay_message, send_relay_message, BlacklistConfig, Identity, NatType,
        NetworkAllocation, NetworkAuthorizationRevocation, QuicCertificate, RelayMessage,
        RelayRegistrationBinding,
    };
    use std::collections::{BTreeMap, BTreeSet};
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    use tokio::sync::mpsc;

    #[derive(Default)]
    struct MemoryAuthorizationStore {
        revocations: Mutex<Vec<NetworkAuthorizationRevocation>>,
        fail_revocation_reads: AtomicBool,
    }

    impl MemoryAuthorizationStore {
        fn set_fail_revocation_reads(&self, fail: bool) {
            self.fail_revocation_reads.store(fail, Ordering::SeqCst);
        }
    }

    impl BlacklistStore for MemoryAuthorizationStore {
        fn supports_authorization_revocations(&self) -> bool {
            true
        }

        fn load(&self) -> Result<BlacklistConfig> {
            Ok(BlacklistConfig::default())
        }

        fn save(&self, _blacklist: &BlacklistConfig) -> Result<()> {
            Ok(())
        }

        fn load_authorization_revocations(&self) -> Result<Vec<NetworkAuthorizationRevocation>> {
            if self.fail_revocation_reads.load(Ordering::SeqCst) {
                anyhow::bail!("injected authorization directory read failure");
            }
            self.revocations
                .lock()
                .map(|revocations| revocations.clone())
                .map_err(|_| anyhow::anyhow!("test authorization directory lock poisoned"))
        }

        fn revoke_authorization(&self, record: &NetworkAuthorizationRevocation) -> Result<bool> {
            let mut revocations = self
                .revocations
                .lock()
                .map_err(|_| anyhow::anyhow!("test authorization directory lock poisoned"))?;
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
                .lock()
                .map_err(|_| anyhow::anyhow!("test authorization directory lock poisoned"))?;
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

    struct MemoryRelayPolicyAuthority {
        policy: Mutex<RelayAccessPolicy>,
    }

    impl MemoryRelayPolicyAuthority {
        fn for_identities(identities: &[&Identity]) -> Self {
            Self {
                policy: Mutex::new(test_relay_policy(identities)),
            }
        }

        fn replace_policy(&self, policy: RelayAccessPolicy) {
            *self.policy.lock().expect("relay policy lock") = policy;
        }
    }

    impl RelayPolicyAuthority for MemoryRelayPolicyAuthority {
        fn current_policy(&self) -> Result<RelayAccessPolicy> {
            self.policy
                .lock()
                .map(|policy| policy.clone())
                .map_err(|_| anyhow::anyhow!("test relay policy lock poisoned"))
        }

        fn authorize_grant(
            &self,
            grant: &RelayMemberGrant,
            now_unix_ms: u64,
        ) -> Result<(RelayAccessPolicy, RelayAuthorizationDecision)> {
            let mut policy = self
                .policy
                .lock()
                .map(|policy| policy.clone())
                .map_err(|_| anyhow::anyhow!("test relay policy lock poisoned"))?;
            let context = policy.members.get(&grant.consumer_node_id).cloned();
            let decision = policy
                .authorize_signed_grant(
                    grant,
                    context
                        .as_ref()
                        .map_or(NetworkRole::Member, |context| context.role),
                    context
                        .as_ref()
                        .map_or_else(BTreeSet::new, |context| context.groups.clone()),
                    context
                        .as_ref()
                        .is_some_and(|context| context.membership_active),
                    context
                        .as_ref()
                        .is_some_and(|context| context.recent_reauthentication),
                    now_unix_ms,
                )
                .map_err(anyhow::Error::new)?;
            Ok((policy, decision))
        }
    }

    fn test_relay_network_id() -> NetworkId {
        NetworkId::from_public_key(b"test-relay-policy-network")
    }

    fn test_relay_provider_node_id() -> NodeId {
        NodeId::from_public_key(b"test-relay-provider")
    }

    fn test_member_grant(identity: &Identity, limits: RelayLimitPolicy) -> RelayMemberGrant {
        let now = now_unix_ms();
        RelayMemberGrant::new(
            test_relay_network_id(),
            test_relay_provider_node_id(),
            identity,
            format!("test-grant-{}", identity.client_id()),
            1,
            now.saturating_sub(1_000),
            now.saturating_add(60_000),
            limits,
        )
        .expect("test relay member grant")
    }

    async fn connect_test_relay(
        relay_addr: SocketAddr,
        server_pin: [u8; 32],
        identity: &Identity,
        virtual_ip: Ipv6Addr,
    ) -> Result<QuicRelayClient> {
        let grant = test_member_grant(identity, RelayLimitPolicy::default());
        connect_test_relay_with_grant(relay_addr, server_pin, identity, virtual_ip, &grant).await
    }

    async fn connect_test_relay_with_grant(
        relay_addr: SocketAddr,
        server_pin: [u8; 32],
        identity: &Identity,
        virtual_ip: Ipv6Addr,
        grant: &RelayMemberGrant,
    ) -> Result<QuicRelayClient> {
        QuicRelayClient::connect(
            relay_addr,
            server_pin,
            "fuckipv6.local",
            identity,
            virtual_ip,
            grant,
        )
        .await
    }

    async fn register_relay_test_client(
        session_manager: &Arc<SessionManager>,
        identity: &Identity,
        index: u16,
    ) -> Result<Ipv6Addr> {
        let (tx, _rx) = mpsc::unbounded_channel();
        Ok(session_manager
            .register_client(
                identity.client_id().to_string(),
                identity.public_key(),
                SocketAddr::new(
                    IpAddr::V4(Ipv4Addr::LOCALHOST),
                    40_000u16.saturating_add(index),
                ),
                50_000u16.saturating_add(index),
                NatType::Open,
                tx,
            )
            .await?
            .primary_ipv6())
    }

    async fn wait_for_active_connections(server: &RelayServer, expected: u32) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if server
                    .state
                    .metrics
                    .active_connections
                    .load(Ordering::Acquire)
                    == expected
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .with_context(|| format!("Relay active connection count did not reach {expected}"))
    }

    async fn wait_for_task_permits(
        server: &RelayServer,
        expected_connections: usize,
        expected_streams: usize,
    ) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if server.state.connection_tasks.available_permits() == expected_connections
                    && server.state.stream_tasks.available_permits() == expected_streams
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .with_context(|| {
            format!(
                "Relay task permits did not return to connections={expected_connections}, streams={expected_streams}"
            )
        })
    }

    fn test_relay_policy(identities: &[&Identity]) -> RelayAccessPolicy {
        let network_id = test_relay_network_id();
        let provider_node_id = test_relay_provider_node_id();
        let owner_rule = PermissionRule {
            rule_id: SecurityRuleId::from_public_key(b"test-relay-provider-owner"),
            effect: PermissionEffect::Allow,
            subjects: vec![SubjectSelector::Role(NetworkRole::Owner)],
            resources: vec![ResourceKind::Relay],
            actions: vec![PolicyAction::Use],
            conditions: vec![PermissionCondition::MembershipActive],
        };
        let member_rule = PermissionRule {
            rule_id: SecurityRuleId::from_public_key(b"test-relay-consumer-member"),
            effect: PermissionEffect::Allow,
            subjects: vec![SubjectSelector::Role(NetworkRole::Member)],
            resources: vec![ResourceKind::Relay],
            actions: vec![PolicyAction::Use],
            conditions: vec![PermissionCondition::MembershipActive],
        };
        let members = identities
            .iter()
            .map(|identity| {
                (
                    NodeId::from_public_key(&identity.public_key()),
                    RelayMemberPolicyContext {
                        role: NetworkRole::Member,
                        groups: BTreeSet::new(),
                        membership_active: true,
                        recent_reauthentication: true,
                        local_rules: vec![member_rule.clone()],
                        local_access_policy: None,
                        limits: RelayLimitPolicy::default(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        RelayAccessPolicy {
            network_id: network_id.clone(),
            provider_node_id,
            provider: RelayMemberPolicyContext {
                role: NetworkRole::Owner,
                groups: BTreeSet::new(),
                membership_active: true,
                recent_reauthentication: true,
                local_rules: vec![owner_rule.clone()],
                local_access_policy: None,
                limits: RelayLimitPolicy::default(),
            },
            existing_session_policy: RelayExistingSessionPolicy::Reauthorize,
            revision: 1,
            updated_at_unix_ms: now_unix_ms().saturating_sub(1_000),
            network_rules: vec![owner_rule, member_rule],
            network_acl: None,
            members,
            limits: RelayLimitPolicy::default(),
            limit_sources: Vec::new(),
        }
    }

    #[derive(Debug, Default, Clone, Copy)]
    struct RegisterBindingOverrides {
        server_public_key_pin: Option<[u8; 32]>,
        client_certificate_public_key_pin: Option<[u8; 32]>,
        tls_channel_binding: Option<[u8; 32]>,
    }

    async fn send_raw_registration<F>(
        relay_addr: SocketAddr,
        expected_server_pin: [u8; 32],
        tls_identity: &Identity,
        application_identity: &Identity,
        virtual_ip: Ipv6Addr,
        overrides: RegisterBindingOverrides,
        mutate: F,
    ) -> Result<RelayMessage>
    where
        F: FnOnce(&mut RelayMessage),
    {
        let client_certificate = QuicCertificate::from_identity(tls_identity)?;
        let mut endpoint =
            quinn::Endpoint::client(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;
        endpoint.set_default_client_config(build_pinned_quic_client_config_with_auth(
            expected_server_pin,
            &client_certificate,
        )?);
        let connection = endpoint
            .connect(relay_addr, "fuckipv6.local")?
            .await
            .context("raw registration client handshake failed")?;

        let public_key = application_identity.public_key();
        let registration = RelayRegistrationBinding {
            client_id: application_identity.client_id(),
            virtual_ip,
            public_key: &public_key,
            timestamp: 123,
            server_public_key_pin: overrides
                .server_public_key_pin
                .unwrap_or(expected_server_pin),
            client_certificate_public_key_pin: overrides
                .client_certificate_public_key_pin
                .unwrap_or(client_certificate.public_key_pin),
            tls_channel_binding: overrides
                .tls_channel_binding
                .unwrap_or(export_quic_relay_channel_binding(&connection)?),
        };
        let signature = application_identity
            .sign(&registration.challenge())
            .to_bytes()
            .to_vec();
        let mut message = RelayMessage::Register {
            client_id: application_identity.client_id().to_string(),
            virtual_ip,
            public_key: public_key.clone(),
            timestamp: registration.timestamp,
            server_public_key_pin: registration.server_public_key_pin,
            client_certificate_public_key_pin: registration.client_certificate_public_key_pin,
            tls_channel_binding: registration.tls_channel_binding,
            signature,
            member_grant: Some(Box::new(test_member_grant(
                application_identity,
                RelayLimitPolicy::default(),
            ))),
        };
        mutate(&mut message);

        let (mut send, mut recv) = connection.open_bi().await?;
        send_relay_message(&mut send, RELAY_REGISTER_STREAM_ID, &message).await?;
        let (_, response) = recv_relay_message(&mut recv).await?;
        connection.close(0u32.into(), b"test-done");
        endpoint.wait_idle().await;
        Ok(response)
    }

    fn relay_authority(identities: &[&Identity]) -> Arc<dyn RelayPolicyAuthority> {
        Arc::new(MemoryRelayPolicyAuthority::for_identities(identities))
    }

    async fn assert_no_relay_state(server: &RelayServer, session_manager: &Arc<SessionManager>) {
        assert!(server.state.clients.read().await.is_empty());
        assert!(server.state.rate.read().await.global.is_none());
        assert!(server.state.rate.read().await.members.is_empty());
        assert!(server.state.members.read().await.is_empty());
        assert_eq!(
            server
                .state
                .metrics
                .active_connections
                .load(Ordering::Acquire),
            0
        );
        assert_eq!(
            server.state.metrics.active_streams.load(Ordering::Acquire),
            0
        );
        assert_eq!(
            server
                .state
                .metrics
                .forwarded_packets
                .load(Ordering::Acquire),
            0
        );
        assert_eq!(
            server.state.metrics.forwarded_bytes.load(Ordering::Acquire),
            0
        );
        assert_eq!(
            server
                .state
                .metrics
                .rejected_packets
                .load(Ordering::Acquire),
            0
        );
        assert_eq!(
            server
                .state
                .metrics
                .rejected_connections
                .load(Ordering::Acquire),
            0
        );
        let traffic = session_manager.snapshot().await.traffic;
        assert_eq!(traffic.relay_packets, 0);
        assert_eq!(traffic.relay_payload_bytes, 0);
        assert_eq!(traffic.relay_errors, 0);
    }

    fn assert_registration_rejected(response: RelayMessage, expected_message: &str) {
        match response {
            RelayMessage::RegisterAck {
                accepted: false,
                message,
            } => assert!(
                message.contains(expected_message),
                "unexpected rejection message: {message}"
            ),
            other => panic!("expected rejected RegisterAck, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn quic_relay_rejects_unregistered_tls_certificate_before_registration() -> Result<()> {
        let session_manager = Arc::new(SessionManager::new("fd00::1000/120")?);
        let application_identity = Identity::generate()?;
        let tls_identity = Identity::generate()?;
        let (tx, _rx) = mpsc::unbounded_channel();
        let virtual_ip = session_manager
            .register_client(
                application_identity.client_id().to_string(),
                application_identity.public_key(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000),
                51820,
                NatType::Open,
                tx,
            )
            .await?
            .primary_ipv6();

        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager.clone(),
            relay_authority(&[&application_identity]),
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.clone().run());

        let connect_result = tokio::time::timeout(
            Duration::from_secs(5),
            connect_test_relay(relay_addr, server_pin, &tls_identity, virtual_ip),
        )
        .await
        .context("unregistered TLS certificate handshake timed out")?;
        assert!(
            connect_result.is_err(),
            "certificate not present in the active-session allowlist must fail TLS"
        );
        assert_no_relay_state(&server, &session_manager).await;

        server_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn quic_relay_revokes_certificate_allowlist_after_session_unregister() -> Result<()> {
        let session_manager = Arc::new(SessionManager::new("fd00::1000/120")?);
        let identity = Identity::generate()?;
        let (tx, _rx) = mpsc::unbounded_channel();
        let virtual_ip = session_manager
            .register_client(
                identity.client_id().to_string(),
                identity.public_key(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000),
                51820,
                NatType::Open,
                tx,
            )
            .await?
            .primary_ipv6();

        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager.clone(),
            relay_authority(&[&identity]),
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.clone().run());

        let first_client = connect_test_relay(relay_addr, server_pin, &identity, virtual_ip)
            .await
            .context("registered identity should pass the certificate allowlist")?;
        first_client.close().await;

        session_manager
            .unregister_client(&identity.client_id().to_string())
            .await;
        let reconnect = tokio::time::timeout(
            Duration::from_secs(5),
            connect_test_relay(relay_addr, server_pin, &identity, virtual_ip),
        )
        .await
        .context("revoked certificate handshake timed out")?;
        assert!(
            reconnect.is_err(),
            "unregistered session public key must no longer pass TLS"
        );
        let traffic = session_manager.snapshot().await.traffic;
        assert_eq!(traffic.relay_packets, 0);
        assert_eq!(traffic.relay_payload_bytes, 0);

        server_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn quic_relay_rejects_valid_tls_certificate_with_another_application_identity(
    ) -> Result<()> {
        let session_manager = Arc::new(SessionManager::new("fd00::1000/120")?);
        let application_identity = Identity::generate()?;
        let tls_identity = Identity::generate()?;
        let (tx, _rx) = mpsc::unbounded_channel();
        session_manager
            .register_client(
                tls_identity.client_id().to_string(),
                tls_identity.public_key(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 42000),
                51820,
                NatType::Open,
                tx.clone(),
            )
            .await?;
        let virtual_ip = session_manager
            .register_client(
                application_identity.client_id().to_string(),
                application_identity.public_key(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000),
                51820,
                NatType::Open,
                tx,
            )
            .await?
            .primary_ipv6();

        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager.clone(),
            relay_authority(&[&application_identity]),
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.clone().run());

        let response = send_raw_registration(
            relay_addr,
            server_pin,
            &tls_identity,
            &application_identity,
            virtual_ip,
            RegisterBindingOverrides::default(),
            |_| {},
        )
        .await?;
        assert_registration_rejected(response, "证书公钥");
        assert_no_relay_state(&server, &session_manager).await;

        server_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn quic_relay_rejects_tampered_registration_fields_without_state_changes() -> Result<()> {
        let session_manager = Arc::new(SessionManager::new("fd00::1000/120")?);
        let identity = Identity::generate()?;
        let (tx, _rx) = mpsc::unbounded_channel();
        let virtual_ip = session_manager
            .register_client(
                identity.client_id().to_string(),
                identity.public_key(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000),
                51820,
                NatType::Open,
                tx,
            )
            .await?
            .primary_ipv6();

        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager.clone(),
            relay_authority(&[&identity]),
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.clone().run());

        let response = send_raw_registration(
            relay_addr,
            server_pin,
            &identity,
            &identity,
            virtual_ip,
            RegisterBindingOverrides::default(),
            |message| {
                let RelayMessage::Register { timestamp, .. } = message else {
                    panic!("expected Register message");
                };
                *timestamp ^= 1;
            },
        )
        .await?;
        assert_registration_rejected(response, "签名无效");
        assert_no_relay_state(&server, &session_manager).await;

        server_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn quic_relay_rejects_missing_registration_binding_without_state_changes() -> Result<()> {
        let session_manager = Arc::new(SessionManager::new("fd00::1000/120")?);
        let identity = Identity::generate()?;
        let (tx, _rx) = mpsc::unbounded_channel();
        let virtual_ip = session_manager
            .register_client(
                identity.client_id().to_string(),
                identity.public_key(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000),
                51820,
                NatType::Open,
                tx,
            )
            .await?
            .primary_ipv6();

        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager.clone(),
            relay_authority(&[&identity]),
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.clone().run());

        let response = send_raw_registration(
            relay_addr,
            server_pin,
            &identity,
            &identity,
            virtual_ip,
            RegisterBindingOverrides {
                server_public_key_pin: Some([0; 32]),
                client_certificate_public_key_pin: Some([0; 32]),
                tls_channel_binding: Some([0; 32]),
            },
            |_| {},
        )
        .await?;
        assert_registration_rejected(response, "SPKI pin");
        assert_no_relay_state(&server, &session_manager).await;

        server_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn quic_relay_rejects_blacklisted_registration_without_state_changes() -> Result<()> {
        let identity = Identity::generate()?;
        let revoked_fingerprint = Identity::public_key_fingerprint(&identity.public_key());
        let session_manager = Arc::new(SessionManager::new_with_allocation_and_blacklist(
            &NetworkAllocation::default(),
            BlacklistConfig {
                banned_clients: vec![revoked_fingerprint[..16].to_string()],
            },
        )?);
        let (tx, _rx) = mpsc::unbounded_channel();
        let virtual_ip = session_manager
            .register_client(
                identity.client_id().to_string(),
                identity.public_key(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000),
                51820,
                NatType::Open,
                tx,
            )
            .await?
            .primary_ipv6();

        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager.clone(),
            relay_authority(&[&identity]),
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.clone().run());

        let response = send_raw_registration(
            relay_addr,
            server_pin,
            &identity,
            &identity,
            virtual_ip,
            RegisterBindingOverrides::default(),
            |_| {},
        )
        .await?;
        assert_registration_rejected(response, "已撤销");
        assert_no_relay_state(&server, &session_manager).await;

        server_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn quic_relay_forwards_packets_between_registered_clients() -> Result<()> {
        let session_manager = Arc::new(SessionManager::new("fd00::1000/120")?);
        let identity_a = Identity::generate()?;
        let identity_b = Identity::generate()?;
        let (tx_a, _rx_a) = mpsc::unbounded_channel();
        let (tx_b, _rx_b) = mpsc::unbounded_channel();
        let observed_a = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000);
        let observed_b = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 42000);

        let ip_a = session_manager
            .register_client(
                identity_a.client_id().to_string(),
                identity_a.public_key(),
                observed_a,
                51820,
                NatType::Open,
                tx_a,
            )
            .await?
            .primary_ipv6();
        let ip_b = session_manager
            .register_client(
                identity_b.client_id().to_string(),
                identity_b.public_key(),
                observed_b,
                51821,
                NatType::Open,
                tx_b,
            )
            .await?
            .primary_ipv6();

        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager.clone(),
            relay_authority(&[&identity_a, &identity_b]),
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.clone().run());

        let client_a = connect_test_relay(relay_addr, server_pin, &identity_a, ip_a).await?;
        let client_b = connect_test_relay(relay_addr, server_pin, &identity_b, ip_b).await?;

        {
            let clients = server.state.clients.read().await;
            assert_eq!(clients.len(), 2);
            assert_eq!(
                clients
                    .get(&ip_a)
                    .context("missing first relay client")?
                    .client_id,
                identity_a.client_id()
            );
            assert_eq!(
                clients
                    .get(&ip_b)
                    .context("missing second relay client")?
                    .client_id,
                identity_b.client_id()
            );
        }
        let rate = server.state.rate.read().await;
        assert!(rate.global.is_some());
        assert_eq!(rate.members.len(), 2);
        drop(rate);

        let payload = Bytes::from_static(b"relay-through-quic");
        let (sent, delivered) = tokio::join!(
            client_a.send_packet(7, ip_a, ip_b, payload.clone()),
            client_b.recv_packet(),
        );
        sent?;

        let delivered = delivered.context("relay packet was not delivered")?;
        assert_eq!(delivered.src_ip, ip_a);
        assert_eq!(delivered.dst_ip, ip_b);
        assert_eq!(delivered.payload, payload);

        let traffic = session_manager.snapshot().await.traffic;
        assert_eq!(traffic.relay_packets, 1);
        assert_eq!(
            traffic.relay_payload_bytes,
            b"relay-through-quic".len() as u64
        );
        assert_eq!(traffic.relay_errors, 0);

        let usage = server.usage_snapshot().await;
        assert!(usage.last_forwarded_at_unix_ms.is_some());
        assert_eq!(usage.cpu_measurement, RelayMeasurementState::Unsupported);
        assert_eq!(usage.memory_measurement, RelayMeasurementState::Unsupported);
        let health = server.health_snapshot().await;
        assert_eq!(health.status, RelayHealthStatus::Healthy);
        assert_eq!(
            health.network_reachability,
            RelayReachabilityState::Reachable
        );
        assert!(health.forwarding_observed);

        client_a.close().await;
        client_b.close().await;
        server_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn bound_listener_does_not_report_network_reachability() -> Result<()> {
        let identity = Identity::generate()?;
        let session_manager = Arc::new(SessionManager::new("fd00::1000/120")?);
        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager,
            relay_authority(&[&identity]),
        )?;

        let health = server.health_snapshot().await;

        assert_eq!(health.status, RelayHealthStatus::Unknown);
        assert_eq!(health.network_reachability, RelayReachabilityState::Unknown);
        assert!(!health.forwarding_observed);
        assert!(health
            .issues
            .contains(&RelayHealthIssue::NetworkReachabilityUnknown));
        assert!(health
            .issues
            .contains(&RelayHealthIssue::ForwardingNotObserved));
        Ok(())
    }

    #[tokio::test]
    async fn health_exposes_policy_limit_sources() -> Result<()> {
        let session_manager = Arc::new(SessionManager::new("fd00::1000/120")?);
        let authority = Arc::new(MemoryRelayPolicyAuthority::for_identities(&[]));
        let mut policy = test_relay_policy(&[]);
        policy.limits = RelayLimitPolicy {
            max_connections: Some(8),
            max_bandwidth_kbps: Some(512),
            ..RelayLimitPolicy::default()
        };
        policy.limit_sources = vec![
            fuckipv6_common::domain::RelayLimitSource::NetworkAcl,
            fuckipv6_common::domain::RelayLimitSource::NodeLocal,
            fuckipv6_common::domain::RelayLimitSource::ProviderDefault,
        ];
        authority.replace_policy(policy);

        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager,
            authority,
        )?;
        let health = server.health_snapshot().await;

        assert_eq!(health.limits.max_connections, Some(8));
        assert_eq!(
            health.limit_sources,
            vec![
                fuckipv6_common::domain::RelayLimitSource::NetworkAcl,
                fuckipv6_common::domain::RelayLimitSource::NodeLocal,
                fuckipv6_common::domain::RelayLimitSource::ProviderDefault,
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn quic_relay_renews_authorization_without_reconnecting() -> Result<()> {
        let session_manager = Arc::new(SessionManager::new("fd00::1000/120")?);
        let identity = Identity::generate()?;
        let (tx, _rx) = mpsc::unbounded_channel();
        let virtual_ip = session_manager
            .register_client(
                identity.client_id().to_string(),
                identity.public_key(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000),
                51820,
                NatType::Open,
                tx,
            )
            .await?
            .primary_ipv6();

        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager,
            relay_authority(&[&identity]),
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.clone().run());
        let client = connect_test_relay(relay_addr, server_pin, &identity, virtual_ip).await?;

        let current = client.current_member_grant().await;
        let renewed = RelayMemberGrant::new(
            current.network_id.clone(),
            current.provider_node_id.clone(),
            &identity,
            "renewed-grant-nonce",
            current.revision + 1,
            current.issued_at_unix_ms,
            current.expires_at_unix_ms + 60_000,
            current.limits.clone(),
        )?;
        client.update_authorization(&identity, &renewed).await?;

        assert_eq!(client.current_member_grant().await, renewed);
        let registered = server
            .state
            .clients
            .read()
            .await
            .get(&virtual_ip)
            .cloned()
            .context("missing renewed relay registration")?;
        assert_eq!(registered.member_grant, renewed);
        assert_eq!(registered.policy_revision, 1);
        assert!(
            client
                .update_authorization(&identity, &registered.member_grant)
                .await
                .is_err(),
            "a replay with the same nonce and expiry must not replace the accepted grant"
        );
        assert_eq!(client.current_member_grant().await, registered.member_grant);

        server_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn quic_relay_reauthorizes_existing_sessions_after_policy_change() -> Result<()> {
        let session_manager = Arc::new(SessionManager::new("fd00::1000/120")?);
        let identity_a = Identity::generate()?;
        let identity_b = Identity::generate()?;
        let (tx_a, _rx_a) = mpsc::unbounded_channel();
        let (tx_b, _rx_b) = mpsc::unbounded_channel();
        let ip_a = session_manager
            .register_client(
                identity_a.client_id().to_string(),
                identity_a.public_key(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000),
                51820,
                NatType::Open,
                tx_a,
            )
            .await?
            .primary_ipv6();
        let ip_b = session_manager
            .register_client(
                identity_b.client_id().to_string(),
                identity_b.public_key(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 42000),
                51821,
                NatType::Open,
                tx_b,
            )
            .await?
            .primary_ipv6();

        let authority = Arc::new(MemoryRelayPolicyAuthority::for_identities(&[
            &identity_a,
            &identity_b,
        ]));
        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager.clone(),
            authority.clone(),
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.clone().run());
        let client_a = connect_test_relay(relay_addr, server_pin, &identity_a, ip_a).await?;
        let client_b = connect_test_relay(relay_addr, server_pin, &identity_b, ip_b).await?;

        let mut next_policy = test_relay_policy(&[&identity_a, &identity_b]);
        next_policy
            .members
            .remove(&NodeId::from_public_key(&identity_b.public_key()));
        next_policy.revision += 1;
        next_policy.updated_at_unix_ms = now_unix_ms().saturating_sub(1);
        authority.replace_policy(next_policy);

        let error = client_a
            .send_packet(
                1,
                ip_a,
                ip_b,
                Bytes::from_static(b"denied after policy change"),
            )
            .await
            .expect_err("existing destination session must be reauthorized")
            .to_string();
        assert!(
            error.contains("403") || error.contains("404"),
            "policy revocation must fail closed with authorization denial or a disconnected destination: {error}"
        );
        let usage = server.usage_snapshot().await;
        assert_eq!(usage.forwarded_packets, 0);
        assert_eq!(usage.forwarded_bytes, 0);
        assert_eq!(usage.rejected_packets, 1);

        client_a.close().await;
        client_b.close().await;
        server_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn quic_relay_closes_connection_when_member_grant_expires() -> Result<()> {
        let session_manager = Arc::new(SessionManager::new("fd00::1000/120")?);
        let identity = Identity::generate()?;
        let (tx, _rx) = mpsc::unbounded_channel();
        let virtual_ip = session_manager
            .register_client(
                identity.client_id().to_string(),
                identity.public_key(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000),
                51820,
                NatType::Open,
                tx,
            )
            .await?
            .primary_ipv6();

        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager,
            relay_authority(&[&identity]),
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.clone().run());
        let now = now_unix_ms();
        let grant = RelayMemberGrant::new(
            test_relay_network_id(),
            test_relay_provider_node_id(),
            &identity,
            "expiring-live-grant",
            1,
            now.saturating_sub(1_000),
            now.saturating_add(1_500),
            RelayLimitPolicy::default(),
        )?;
        let client =
            connect_test_relay_with_grant(relay_addr, server_pin, &identity, virtual_ip, &grant)
                .await?;

        let closed = tokio::time::timeout(Duration::from_secs(4), client.recv_packet())
            .await
            .context("client did not observe expired relay grant")?;
        assert!(closed.is_none());
        assert!(server.state.clients.read().await.is_empty());
        assert_eq!(
            server
                .state
                .metrics
                .active_connections
                .load(Ordering::Acquire),
            0
        );

        server_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn quic_relay_terminates_existing_session_on_policy_change_without_next_packet(
    ) -> Result<()> {
        let session_manager = Arc::new(SessionManager::new("fd00::1000/120")?);
        let identity = Identity::generate()?;
        let (tx, _rx) = mpsc::unbounded_channel();
        let virtual_ip = session_manager
            .register_client(
                identity.client_id().to_string(),
                identity.public_key(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000),
                51820,
                NatType::Open,
                tx,
            )
            .await?
            .primary_ipv6();

        let authority = Arc::new(MemoryRelayPolicyAuthority::for_identities(&[&identity]));
        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager,
            authority.clone(),
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.clone().run());
        let client = connect_test_relay(relay_addr, server_pin, &identity, virtual_ip).await?;

        let mut next_policy = test_relay_policy(&[&identity]);
        next_policy.existing_session_policy = RelayExistingSessionPolicy::Terminate;
        next_policy.revision += 1;
        next_policy.updated_at_unix_ms = now_unix_ms().saturating_sub(1);
        authority.replace_policy(next_policy);

        let closed = tokio::time::timeout(Duration::from_secs(3), client.recv_packet())
            .await
            .context("terminate policy did not close the existing relay connection")?;
        assert!(closed.is_none());
        assert!(server.state.clients.read().await.is_empty());
        assert_eq!(
            server
                .state
                .metrics
                .active_connections
                .load(Ordering::Acquire),
            0
        );

        server_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn quic_relay_reauth_rejects_existing_session_without_next_packet() -> Result<()> {
        let session_manager = Arc::new(SessionManager::new("fd00::1000/120")?);
        let identity = Identity::generate()?;
        let (tx, _rx) = mpsc::unbounded_channel();
        let virtual_ip = session_manager
            .register_client(
                identity.client_id().to_string(),
                identity.public_key(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000),
                51820,
                NatType::Open,
                tx,
            )
            .await?
            .primary_ipv6();

        let authority = Arc::new(MemoryRelayPolicyAuthority::for_identities(&[&identity]));
        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager,
            authority.clone(),
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.clone().run());
        let client = connect_test_relay(relay_addr, server_pin, &identity, virtual_ip).await?;

        let mut next_policy = test_relay_policy(&[&identity]);
        next_policy.members.clear();
        next_policy.revision += 1;
        next_policy.updated_at_unix_ms = now_unix_ms().saturating_sub(1);
        authority.replace_policy(next_policy);

        let closed = tokio::time::timeout(Duration::from_secs(3), client.recv_packet())
            .await
            .context("reauthorize policy did not close the revoked relay connection")?;
        assert!(closed.is_none());
        assert!(server.state.clients.read().await.is_empty());

        server_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn quic_relay_continue_policy_keeps_authorized_existing_session() -> Result<()> {
        let session_manager = Arc::new(SessionManager::new("fd00::1000/120")?);
        let identity_a = Identity::generate()?;
        let identity_b = Identity::generate()?;
        let (tx_a, _rx_a) = mpsc::unbounded_channel();
        let (tx_b, _rx_b) = mpsc::unbounded_channel();
        let ip_a = session_manager
            .register_client(
                identity_a.client_id().to_string(),
                identity_a.public_key(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000),
                51820,
                NatType::Open,
                tx_a,
            )
            .await?
            .primary_ipv6();
        let ip_b = session_manager
            .register_client(
                identity_b.client_id().to_string(),
                identity_b.public_key(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 42000),
                51821,
                NatType::Open,
                tx_b,
            )
            .await?
            .primary_ipv6();

        let authority = Arc::new(MemoryRelayPolicyAuthority::for_identities(&[
            &identity_a,
            &identity_b,
        ]));
        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager.clone(),
            authority.clone(),
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.clone().run());
        let client_a = connect_test_relay(relay_addr, server_pin, &identity_a, ip_a).await?;
        let client_b = connect_test_relay(relay_addr, server_pin, &identity_b, ip_b).await?;

        let mut next_policy = test_relay_policy(&[&identity_b]);
        next_policy.existing_session_policy = RelayExistingSessionPolicy::Continue;
        next_policy.revision += 1;
        next_policy.updated_at_unix_ms = now_unix_ms().saturating_sub(1);
        authority.replace_policy(next_policy);
        tokio::time::sleep(Duration::from_millis(1_200)).await;

        client_a
            .send_packet(
                1,
                ip_a,
                ip_b,
                Bytes::from_static(b"existing session continues"),
            )
            .await?;
        let received = tokio::time::timeout(Duration::from_secs(2), client_b.recv_packet())
            .await
            .context("continue session did not receive relayed packet")?
            .context("continue relay connection closed unexpectedly")?;
        assert_eq!(
            received.payload,
            Bytes::from_static(b"existing session continues")
        );
        assert_eq!(server.state.clients.read().await.len(), 2);

        client_a.close().await;
        client_b.close().await;
        server_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn quic_relay_closes_existing_session_when_duration_quota_is_reached() -> Result<()> {
        let session_manager = Arc::new(SessionManager::new("fd00::1000/120")?);
        let identity = Identity::generate()?;
        let (tx, _rx) = mpsc::unbounded_channel();
        let virtual_ip = session_manager
            .register_client(
                identity.client_id().to_string(),
                identity.public_key(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000),
                51820,
                NatType::Open,
                tx,
            )
            .await?
            .primary_ipv6();

        let authority = Arc::new(MemoryRelayPolicyAuthority::for_identities(&[&identity]));
        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager,
            authority.clone(),
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.clone().run());
        let client = connect_test_relay(relay_addr, server_pin, &identity, virtual_ip).await?;

        let mut next_policy = test_relay_policy(&[&identity]);
        next_policy.limits.max_session_duration_secs = Some(1);
        next_policy.revision += 1;
        next_policy.updated_at_unix_ms = now_unix_ms().saturating_sub(1);
        authority.replace_policy(next_policy);

        let closed = tokio::time::timeout(Duration::from_secs(4), client.recv_packet())
            .await
            .context("duration quota did not close the existing relay connection")?;
        assert!(closed.is_none());
        assert!(server.state.clients.read().await.is_empty());
        assert_eq!(
            server
                .state
                .metrics
                .active_connections
                .load(Ordering::Acquire),
            0
        );

        server_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn quic_relay_connection_quota_rejects_full_and_recovers_after_disconnect() -> Result<()>
    {
        let session_manager = Arc::new(SessionManager::new("fd00::1000/120")?);
        let identity_a = Identity::generate()?;
        let identity_b = Identity::generate()?;
        let ip_a = register_relay_test_client(&session_manager, &identity_a, 0).await?;
        let ip_b = register_relay_test_client(&session_manager, &identity_b, 1).await?;

        let authority = Arc::new(MemoryRelayPolicyAuthority::for_identities(&[
            &identity_a,
            &identity_b,
        ]));
        let mut policy = test_relay_policy(&[&identity_a, &identity_b]);
        policy.limits.max_connections = Some(1);
        authority.replace_policy(policy);

        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager,
            authority,
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.clone().run());

        let client_a = connect_test_relay(relay_addr, server_pin, &identity_a, ip_a).await?;
        wait_for_active_connections(&server, 1).await?;

        let rejection = match connect_test_relay(relay_addr, server_pin, &identity_b, ip_b).await {
            Ok(_) => panic!("a full Relay must reject another connection"),
            Err(error) => error.to_string(),
        };
        assert!(rejection.contains("Relay 连接配额已耗尽"), "{rejection}");
        wait_for_task_permits(
            &server,
            MAX_RELAY_CONNECTION_TASKS - 1,
            MAX_RELAY_STREAM_TASKS,
        )
        .await?;

        let usage = server.usage_snapshot().await;
        assert_eq!(usage.active_connections, 1);
        assert_eq!(usage.rejected_connections, 1);
        assert_eq!(usage.active_streams, 0);
        let health = server.health_snapshot().await;
        assert_eq!(health.status, RelayHealthStatus::QuotaExhausted);
        assert!(health.quota_limited);

        client_a.close().await;
        wait_for_active_connections(&server, 0).await?;
        wait_for_task_permits(&server, MAX_RELAY_CONNECTION_TASKS, MAX_RELAY_STREAM_TASKS).await?;
        let recovered_health = server.health_snapshot().await;
        assert!(!recovered_health.quota_limited);

        let client_b = connect_test_relay(relay_addr, server_pin, &identity_b, ip_b).await?;
        wait_for_active_connections(&server, 1).await?;
        assert_eq!(server.usage_snapshot().await.rejected_connections, 1);

        client_b.close().await;
        wait_for_active_connections(&server, 0).await?;
        wait_for_task_permits(&server, MAX_RELAY_CONNECTION_TASKS, MAX_RELAY_STREAM_TASKS).await?;

        server_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn quic_relay_bandwidth_quota_rejects_then_recovers_over_time() -> Result<()> {
        let session_manager = Arc::new(SessionManager::new("fd00::1000/120")?);
        let identity_a = Identity::generate()?;
        let identity_b = Identity::generate()?;
        let ip_a = register_relay_test_client(&session_manager, &identity_a, 0).await?;
        let ip_b = register_relay_test_client(&session_manager, &identity_b, 1).await?;

        let authority = Arc::new(MemoryRelayPolicyAuthority::for_identities(&[
            &identity_a,
            &identity_b,
        ]));
        let mut policy = test_relay_policy(&[&identity_a, &identity_b]);
        policy.limits.max_bandwidth_kbps = Some(1);
        authority.replace_policy(policy);

        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager,
            authority,
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.clone().run());
        let client_a = connect_test_relay(relay_addr, server_pin, &identity_a, ip_a).await?;
        let client_b = connect_test_relay(relay_addr, server_pin, &identity_b, ip_b).await?;
        wait_for_active_connections(&server, 2).await?;

        let payload = Bytes::from(vec![0x5a; 100]);
        client_a.send_packet(1, ip_a, ip_b, payload.clone()).await?;
        let rejection = client_a
            .send_packet(2, ip_a, ip_b, payload.clone())
            .await
            .expect_err("the token bucket must reject traffic above the configured rate")
            .to_string();
        assert!(rejection.contains("429"), "{rejection}");
        wait_for_task_permits(
            &server,
            MAX_RELAY_CONNECTION_TASKS - 2,
            MAX_RELAY_STREAM_TASKS,
        )
        .await?;

        let usage = server.usage_snapshot().await;
        assert_eq!(usage.forwarded_packets, 1);
        assert_eq!(usage.forwarded_bytes, 100);
        assert_eq!(usage.rejected_packets, 1);
        assert_eq!(usage.active_streams, 0);
        let health = server.health_snapshot().await;
        assert_eq!(health.status, RelayHealthStatus::Degraded);
        assert!(health.issues.contains(&RelayHealthIssue::ForwardingStalled));

        tokio::time::sleep(Duration::from_millis(1_200)).await;
        client_a
            .send_packet(3, ip_a, ip_b, payload)
            .await
            .context("bandwidth quota did not refill after the configured rate window")?;
        wait_for_task_permits(
            &server,
            MAX_RELAY_CONNECTION_TASKS - 2,
            MAX_RELAY_STREAM_TASKS,
        )
        .await?;

        let usage = server.usage_snapshot().await;
        assert_eq!(usage.forwarded_packets, 2);
        assert_eq!(usage.forwarded_bytes, 200);
        assert_eq!(usage.rejected_packets, 1);
        assert_eq!(usage.active_streams, 0);
        assert_eq!(
            server.health_snapshot().await.status,
            RelayHealthStatus::Healthy
        );

        client_a.close().await;
        client_b.close().await;
        wait_for_active_connections(&server, 0).await?;
        wait_for_task_permits(&server, MAX_RELAY_CONNECTION_TASKS, MAX_RELAY_STREAM_TASKS).await?;

        server_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn quic_relay_byte_quota_rejects_and_resets_on_schedule() -> Result<()> {
        let session_manager = Arc::new(SessionManager::new("fd00::1000/120")?);
        let identity_a = Identity::generate()?;
        let identity_b = Identity::generate()?;
        let ip_a = register_relay_test_client(&session_manager, &identity_a, 0).await?;
        let ip_b = register_relay_test_client(&session_manager, &identity_b, 1).await?;

        let reset_at = now_unix_ms().saturating_add(2_000);
        let authority = Arc::new(MemoryRelayPolicyAuthority::for_identities(&[
            &identity_a,
            &identity_b,
        ]));
        let mut policy = test_relay_policy(&[&identity_a, &identity_b]);
        policy.limits.max_bytes = Some(100);
        policy.limits.quota_reset_at_unix_ms = Some(reset_at);
        authority.replace_policy(policy);

        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager,
            authority,
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.clone().run());
        let client_a = connect_test_relay(relay_addr, server_pin, &identity_a, ip_a).await?;
        let client_b = connect_test_relay(relay_addr, server_pin, &identity_b, ip_b).await?;
        wait_for_active_connections(&server, 2).await?;

        let full_payload = Bytes::from(vec![0x33; 100]);
        client_a
            .send_packet(1, ip_a, ip_b, full_payload.clone())
            .await?;
        let rejection = client_a
            .send_packet(2, ip_a, ip_b, full_payload)
            .await
            .expect_err("the cumulative byte quota must reject traffic above its limit")
            .to_string();
        assert!(rejection.contains("Relay 流量配额已耗尽"), "{rejection}");
        wait_for_task_permits(
            &server,
            MAX_RELAY_CONNECTION_TASKS - 2,
            MAX_RELAY_STREAM_TASKS,
        )
        .await?;

        let usage = server.usage_snapshot().await;
        assert_eq!(usage.forwarded_packets, 1);
        assert_eq!(usage.forwarded_bytes, 100);
        assert_eq!(usage.rejected_packets, 1);
        assert_eq!(usage.active_streams, 0);
        assert_eq!(usage.quota_reset_at_unix_ms, Some(reset_at));
        let health = server.health_snapshot().await;
        assert_eq!(health.status, RelayHealthStatus::QuotaExhausted);
        assert!(health.quota_limited);

        let wait_ms = reset_at.saturating_sub(now_unix_ms()).saturating_add(200);
        tokio::time::sleep(Duration::from_millis(wait_ms)).await;

        client_a
            .send_packet(3, ip_a, ip_b, Bytes::from(vec![0x44; 40]))
            .await
            .context("byte quota did not reset at the scheduled Unix time")?;
        wait_for_task_permits(
            &server,
            MAX_RELAY_CONNECTION_TASKS - 2,
            MAX_RELAY_STREAM_TASKS,
        )
        .await?;

        let usage = server.usage_snapshot().await;
        assert_eq!(usage.forwarded_packets, 1);
        assert_eq!(usage.forwarded_bytes, 40);
        assert_eq!(usage.rejected_packets, 0);
        assert_eq!(usage.active_streams, 0);
        assert_eq!(usage.quota_started_at_unix_ms, Some(reset_at));
        assert_eq!(usage.quota_reset_at_unix_ms, None);
        assert_eq!(
            server.health_snapshot().await.status,
            RelayHealthStatus::Healthy
        );

        client_a.close().await;
        client_b.close().await;
        wait_for_active_connections(&server, 0).await?;
        wait_for_task_permits(&server, MAX_RELAY_CONNECTION_TASKS, MAX_RELAY_STREAM_TASKS).await?;

        server_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn quic_relay_shutdown_closes_clients_and_releases_udp_port() -> Result<()> {
        let session_manager = Arc::new(SessionManager::new("fd00::1000/120")?);
        let identity = Identity::generate()?;
        let (tx, _rx) = mpsc::unbounded_channel();
        let virtual_ip = session_manager
            .register_client(
                identity.client_id().to_string(),
                identity.public_key(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000),
                51820,
                NatType::Open,
                tx,
            )
            .await?
            .primary_ipv6();

        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager,
            relay_authority(&[&identity]),
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.clone().run());
        let client = connect_test_relay(relay_addr, server_pin, &identity, virtual_ip).await?;

        server.shutdown().await;
        let accept_result = tokio::time::timeout(Duration::from_secs(2), server_task)
            .await
            .context("relay accept task did not stop after shutdown")??;
        accept_result.context("relay accept task failed after shutdown")?;

        let closed = tokio::time::timeout(Duration::from_secs(2), client.recv_packet())
            .await
            .context("client did not observe relay shutdown")?;
        assert!(closed.is_none());
        assert!(server.state.clients.read().await.is_empty());
        let rate = server.state.rate.read().await;
        assert!(rate.global.is_none());
        assert!(rate.members.is_empty());
        drop(rate);

        // The QUIC endpoint owns the UDP socket. The lifecycle owner must drop
        // its final Arc after the accept task exits before the port is reusable.
        drop(server);
        let rebound = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match tokio::net::UdpSocket::bind(relay_addr).await {
                    Ok(socket) => return Ok::<_, std::io::Error>(socket),
                    Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Err(error) => return Err(error),
                }
            }
        })
        .await
        .context("relay UDP port was not released after shutdown")??;
        drop(rebound);
        Ok(())
    }

    #[tokio::test]
    async fn quic_relay_blocks_revoked_clients_in_both_directions() -> Result<()> {
        let session_manager = Arc::new(SessionManager::new("fd00::1000/120")?);
        let identity_a = Identity::generate()?;
        let identity_b = Identity::generate()?;
        let (tx_a, _rx_a) = mpsc::unbounded_channel();
        let (tx_b, _rx_b) = mpsc::unbounded_channel();
        let ip_a = session_manager
            .register_client(
                identity_a.client_id().to_string(),
                identity_a.public_key(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000),
                51820,
                NatType::Open,
                tx_a,
            )
            .await?
            .primary_ipv6();
        let ip_b = session_manager
            .register_client(
                identity_b.client_id().to_string(),
                identity_b.public_key(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 42000),
                51821,
                NatType::Open,
                tx_b,
            )
            .await?
            .primary_ipv6();

        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager.clone(),
            relay_authority(&[&identity_a, &identity_b]),
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.run());
        let client_a = connect_test_relay(relay_addr, server_pin, &identity_a, ip_a).await?;
        let client_b = connect_test_relay(relay_addr, server_pin, &identity_b, ip_b).await?;

        let revoked_fingerprint = Identity::public_key_fingerprint(&identity_b.public_key());
        session_manager
            .ban_client(revoked_fingerprint[..16].to_string())
            .await;
        let destination_error = client_a
            .send_packet(1, ip_a, ip_b, Bytes::from_static(b"revoked destination"))
            .await
            .expect_err("revoked destination must not receive relay packets")
            .to_string();
        assert!(
            destination_error.contains("403") || destination_error.contains("404"),
            "revoked destination must fail closed: {destination_error}"
        );

        session_manager
            .ban_client(identity_a.client_id().to_string())
            .await;
        let source_error = client_a
            .send_packet(2, ip_a, ip_b, Bytes::from_static(b"revoked source"))
            .await
            .expect_err("revoked source must not send relay packets")
            .to_string();
        assert!(source_error.contains("403"), "{source_error}");

        let traffic = session_manager.snapshot().await.traffic;
        assert_eq!(traffic.relay_packets, 0);
        assert_eq!(traffic.relay_payload_bytes, 0);

        client_a.close().await;
        client_b.close().await;
        server_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn quic_relay_rechecks_persistent_revocation_on_established_connections() -> Result<()> {
        let store = Arc::new(MemoryAuthorizationStore::default());
        let session_manager = Arc::new(SessionManager::new("fd00::1000/120")?);
        session_manager
            .attach_blacklist_store(store.clone())
            .await?;
        let identity_a = Identity::generate()?;
        let identity_b = Identity::generate()?;
        let (tx_a, _rx_a) = mpsc::unbounded_channel();
        let (tx_b, _rx_b) = mpsc::unbounded_channel();
        let ip_a = session_manager
            .register_client(
                identity_a.client_id().to_string(),
                identity_a.public_key(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000),
                51820,
                NatType::Open,
                tx_a,
            )
            .await?
            .primary_ipv6();
        let ip_b = session_manager
            .register_client(
                identity_b.client_id().to_string(),
                identity_b.public_key(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 42000),
                51821,
                NatType::Open,
                tx_b,
            )
            .await?
            .primary_ipv6();

        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager.clone(),
            relay_authority(&[&identity_a, &identity_b]),
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.run());
        let client_a = connect_test_relay(relay_addr, server_pin, &identity_a, ip_a).await?;
        let client_b = connect_test_relay(relay_addr, server_pin, &identity_b, ip_b).await?;

        session_manager
            .try_revoke_node(identity_b.client_id(), &identity_b.public_key())
            .await?;
        let destination_error = client_a
            .send_packet(
                1,
                ip_a,
                ip_b,
                Bytes::from_static(b"persistently revoked destination"),
            )
            .await
            .expect_err("persistently revoked destination must not receive relay packets")
            .to_string();
        assert!(
            destination_error.contains("403") || destination_error.contains("404"),
            "revoked destination must fail closed with authorization denial or a disconnected destination: {destination_error}"
        );

        session_manager
            .try_revoke_node(identity_a.client_id(), &identity_a.public_key())
            .await?;
        let source_error = client_a
            .send_packet(
                2,
                ip_a,
                ip_b,
                Bytes::from_static(b"persistently revoked source"),
            )
            .await
            .expect_err("persistently revoked source must not send relay packets")
            .to_string();
        assert!(source_error.contains("403"), "{source_error}");

        let traffic = session_manager.snapshot().await.traffic;
        assert_eq!(traffic.relay_packets, 0);
        assert_eq!(traffic.relay_payload_bytes, 0);
        assert_eq!(traffic.relay_errors, 2);

        client_a.close().await;
        client_b.close().await;
        server_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn quic_relay_fails_closed_when_revocation_storage_is_unavailable() -> Result<()> {
        let store = Arc::new(MemoryAuthorizationStore::default());
        let session_manager = Arc::new(SessionManager::new("fd00::1000/120")?);
        session_manager
            .attach_blacklist_store(store.clone())
            .await?;
        let identity_a = Identity::generate()?;
        let identity_b = Identity::generate()?;
        let (tx_a, _rx_a) = mpsc::unbounded_channel();
        let (tx_b, _rx_b) = mpsc::unbounded_channel();
        let ip_a = session_manager
            .register_client(
                identity_a.client_id().to_string(),
                identity_a.public_key(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000),
                51820,
                NatType::Open,
                tx_a,
            )
            .await?
            .primary_ipv6();
        let ip_b = session_manager
            .register_client(
                identity_b.client_id().to_string(),
                identity_b.public_key(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 42000),
                51821,
                NatType::Open,
                tx_b,
            )
            .await?
            .primary_ipv6();

        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager.clone(),
            relay_authority(&[&identity_a, &identity_b]),
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.run());
        let client_a = connect_test_relay(relay_addr, server_pin, &identity_a, ip_a).await?;
        let client_b = connect_test_relay(relay_addr, server_pin, &identity_b, ip_b).await?;

        store.set_fail_revocation_reads(true);
        let error = client_a
            .send_packet(1, ip_a, ip_b, Bytes::from_static(b"storage unavailable"))
            .await
            .expect_err("authorization storage errors must fail packet forwarding closed")
            .to_string();
        assert!(error.contains("500"), "{error}");

        let traffic = session_manager.snapshot().await.traffic;
        assert_eq!(traffic.relay_packets, 0);
        assert_eq!(traffic.relay_payload_bytes, 0);
        assert_eq!(traffic.relay_errors, 1);

        client_a.close().await;
        client_b.close().await;
        server_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn quic_relay_rejects_unregistered_source_ip() -> Result<()> {
        let session_manager = Arc::new(SessionManager::new("fd00::1000/120")?);
        let identity = Identity::generate()?;
        let (tx, _rx) = mpsc::unbounded_channel();
        let observed = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000);
        let ip = session_manager
            .register_client(
                identity.client_id().to_string(),
                identity.public_key(),
                observed,
                51820,
                NatType::Open,
                tx,
            )
            .await?
            .primary_ipv6();

        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager,
            relay_authority(&[&identity]),
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.clone().run());
        let client = connect_test_relay(relay_addr, server_pin, &identity, ip).await?;

        let wrong_src = "fd00::ffff".parse()?;
        let response = client
            .send_packet(
                1,
                wrong_src,
                "fd00::1002".parse()?,
                Bytes::from_static(b"bad"),
            )
            .await;

        let error = response.unwrap_err().to_string();
        assert!(error.contains("403"));

        client.close().await;
        server_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn quic_relay_rejects_packet_when_stream_task_limit_is_exhausted() -> Result<()> {
        let session_manager = Arc::new(SessionManager::new("fd00::1000/120")?);
        let identity = Identity::generate()?;
        let (tx, _rx) = mpsc::unbounded_channel();
        let observed = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41000);
        let ip = session_manager
            .register_client(
                identity.client_id().to_string(),
                identity.public_key(),
                observed,
                51820,
                NatType::Open,
                tx,
            )
            .await?
            .primary_ipv6();

        let certificate =
            generate_self_signed_quic_certificate(vec!["fuckipv6.local".to_string()])?;
        let server = RelayServer::new_test_with_limits(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager,
            relay_authority(&[&identity]),
            certificate,
            8,
            0,
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.clone().run());
        let client = connect_test_relay(relay_addr, server_pin, &identity, ip).await?;

        let result = client
            .send_packet(
                1,
                ip,
                "fd00::1002".parse()?,
                Bytes::from_static(b"overloaded"),
            )
            .await;

        let error = result.unwrap_err().to_string();
        assert!(error.contains("503"));
        let usage = server.usage_snapshot().await;
        assert_eq!(usage.rejected_packets, 1);
        let health = server.health_snapshot().await;
        assert!(health
            .issues
            .contains(&fuckipv6_common::domain::RelayHealthIssue::ResourcePressure));

        client.close().await;
        server_task.abort();
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn quic_relay_handles_100_registered_clients() -> Result<()> {
        const CLIENTS: usize = 100;

        let session_manager = Arc::new(SessionManager::new("fd00::1000/112")?);
        let mut identities = Vec::with_capacity(CLIENTS);
        let mut virtual_ips = Vec::with_capacity(CLIENTS);

        for index in 0..CLIENTS {
            let identity = Identity::generate()?;
            let (tx, _rx) = mpsc::unbounded_channel();
            let observed = SocketAddr::new(
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                41000 + u16::try_from(index)?,
            );
            let virtual_ip = session_manager
                .register_client(
                    identity.client_id().to_string(),
                    identity.public_key(),
                    observed,
                    51000 + u16::try_from(index)?,
                    NatType::Open,
                    tx,
                )
                .await?
                .primary_ipv6();
            identities.push(identity);
            virtual_ips.push(virtual_ip);
        }

        let policy_identities: Vec<&Identity> = identities.iter().collect();
        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager.clone(),
            relay_authority(&policy_identities),
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.clone().run());

        let mut tasks = Vec::with_capacity(CLIENTS);
        for (identity, virtual_ip) in identities.into_iter().zip(virtual_ips.iter().copied()) {
            tasks.push(tokio::spawn(async move {
                connect_test_relay(relay_addr, server_pin, &identity, virtual_ip).await
            }));
        }

        let mut clients = Vec::with_capacity(CLIENTS);
        for task in tasks {
            clients.push(task.await??);
        }

        assert_eq!(session_manager.snapshot().await.online_clients, CLIENTS);
        assert_eq!(server.state.clients.read().await.len(), CLIENTS);

        for client in clients {
            client.close().await;
        }
        server_task.abort();
        Ok(())
    }
}
