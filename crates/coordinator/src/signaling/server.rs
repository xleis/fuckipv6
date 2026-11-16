use anyhow::{Context, Result};
use bytes::Bytes;
use fuckipv6_common::domain::{
    AdminRecoveryRequest, AdminRecoveryRequestId, ContentHash, CoordinatorElectionProposal,
    CoordinatorElectionVote, CoordinatorRecoveryReport, CoordinatorTerm,
    CoordinatorWriteCredential, ExitContributionRequest, ExitContributionRequestId,
    ExitContributionReviewAction, InvitationId, InvitationState, JoinRequest, JoinRequestId,
    JoinRequestState, MemberLifecycleChange, MemberLifecycleChangeRequest,
    MemberRemovalPreviewRequest, NetworkId, NetworkJoinPreview, NetworkMemberPresence,
    NetworkRecord, NodeId,
};
use fuckipv6_common::{
    codec, validate_signaling_message_len, validate_signaling_relay_payload_len, CoordinatorConfig,
    ErrorCode, Identity, JoinRequestBatchReviewItem, RelayEndpoint, SignalingMessage,
    MAX_SIGNALING_MESSAGE_SIZE,
};
use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::{
    accept_async_with_config,
    tungstenite::{protocol::WebSocketConfig, Message},
};

use super::admin_recovery::{AdminRecoveryService, AdminRecoveryStore};
use super::coordinator_candidates::{CoordinatorCandidateStore, GOVERNANCE_FROZEN_ERROR};
use super::exit_availability::ExitAvailabilityObservationRegistry;
use super::exit_contribution::ExitContributionRequestStore;
use super::invitations::{load_trusted_network_record, InvitationStore};
use super::join_requests::JoinRequestStore;
use super::member_lifecycle::MemberLifecycleStore;
use super::members::build_member_directory;
use super::session::SessionManager;

const COORDINATOR_EMERGENCY_RECONCILE_INTERVAL: Duration = Duration::from_secs(5);

fn verify_registration(
    client_id: &str,
    public_key: &[u8],
    requested_ips: Option<&fuckipv6_common::VirtualIps>,
    p2p_port: u16,
    timestamp: u64,
    signature: &[u8],
) -> bool {
    let challenge = match requested_ips {
        Some(ips) => SignalingMessage::register_challenge_with_ips(
            client_id,
            public_key,
            Some(ips),
            p2p_port,
            timestamp,
        ),
        None => SignalingMessage::register_challenge(client_id, public_key, p2p_port, timestamp),
    };

    Identity::verify_peer_fingerprint(public_key, client_id)
        && Identity::verify(public_key, &challenge, signature)
}

/// Thread-safe relay directory shared by the signaling listener and the
/// embedded-service lifecycle. The accept loop does not cache it, so a relay
/// enabled or disabled after signaling starts is reflected by the next
/// registration response without restarting the coordinator.
#[derive(Clone, Debug, Default)]
pub struct RelayEndpointDirectory {
    endpoints: Arc<RwLock<RelayEndpoints>>,
}

#[derive(Clone, Debug, Default)]
struct RelayEndpoints {
    primary: Option<RelayEndpoint>,
    additional: Vec<RelayEndpoint>,
}

impl RelayEndpointDirectory {
    pub fn set_primary(&self, endpoint: Option<RelayEndpoint>) {
        if let Ok(mut endpoints) = self.endpoints.write() {
            endpoints.primary = endpoint;
        }
    }

    pub fn set_additional(&self, endpoints: Vec<RelayEndpoint>) {
        if let Ok(mut current) = self.endpoints.write() {
            current.additional = endpoints;
        }
    }

    pub fn snapshot(&self) -> (Option<RelayEndpoint>, Vec<RelayEndpoint>) {
        self.endpoints
            .read()
            .map(|endpoints| (endpoints.primary.clone(), endpoints.additional.clone()))
            .unwrap_or_default()
    }
}

/// 信令服务器
pub struct SignalingServer {
    bind_addr: String,
    session_manager: Arc<SessionManager>,
    relay_directory: RelayEndpointDirectory,
    join_request_store: Option<Arc<JoinRequestStore>>,
    invitation_store: Option<Arc<InvitationStore>>,
    admin_recovery_store: Option<Arc<AdminRecoveryStore>>,
    admin_recovery_service: Option<Arc<AdminRecoveryService>>,
    member_lifecycle_store: Option<Arc<MemberLifecycleStore>>,
    exit_contribution_store: Option<Arc<ExitContributionRequestStore>>,
    coordinator_candidate_store: Option<Arc<CoordinatorCandidateStore>>,
    exit_availability: Arc<ExitAvailabilityObservationRegistry>,
    network_record_path: Option<PathBuf>,
    node_identity: Option<Arc<Identity>>,
    listener: Option<TcpListener>,
}

impl SignalingServer {
    /// 创建信令服务器
    pub fn new(bind_addr: String, ip_pool: &str) -> Result<Self> {
        let session_manager = Arc::new(SessionManager::new(ip_pool)?);

        Ok(Self {
            bind_addr,
            session_manager,
            relay_directory: RelayEndpointDirectory::default(),
            join_request_store: None,
            invitation_store: None,
            admin_recovery_store: None,
            admin_recovery_service: None,
            member_lifecycle_store: None,
            exit_contribution_store: None,
            coordinator_candidate_store: None,
            exit_availability: Arc::new(ExitAvailabilityObservationRegistry::default()),
            network_record_path: None,
            node_identity: None,
            listener: None,
        })
    }

    pub fn new_with_manager(bind_addr: String, session_manager: Arc<SessionManager>) -> Self {
        Self {
            bind_addr,
            session_manager,
            relay_directory: RelayEndpointDirectory::default(),
            join_request_store: None,
            invitation_store: None,
            admin_recovery_store: None,
            admin_recovery_service: None,
            member_lifecycle_store: None,
            exit_contribution_store: None,
            coordinator_candidate_store: None,
            exit_availability: Arc::new(ExitAvailabilityObservationRegistry::default()),
            network_record_path: None,
            node_identity: None,
            listener: None,
        }
    }

    /// Bind the listening socket immediately so callers can publish the actual
    /// port before the server task starts. This is required when port `0` is
    /// used to request an ephemeral port.
    pub async fn bind(mut self) -> Result<Self> {
        let listener = TcpListener::bind(&self.bind_addr)
            .await
            .with_context(|| format!("无法监听信令服务: {}", self.bind_addr))?;
        self.listener = Some(listener);
        Ok(self)
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.listener
            .as_ref()
            .context("信令服务尚未绑定监听地址")?
            .local_addr()
            .context("无法读取信令服务监听地址")
    }

    pub fn with_relay_endpoint(self, relay_endpoint: RelayEndpoint) -> Self {
        self.relay_directory.set_primary(Some(relay_endpoint));
        self
    }

    pub fn with_relay_endpoints(self, relay_endpoints: Vec<RelayEndpoint>) -> Self {
        self.relay_directory.set_additional(relay_endpoints);
        self
    }

    pub fn relay_endpoint_directory(&self) -> RelayEndpointDirectory {
        self.relay_directory.clone()
    }

    /// Inject the authoritative owner record used for direct join disclosure.
    /// Without it, network preview requests fail closed.
    pub fn with_network_record_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.network_record_path = Some(path.into());
        self
    }

    /// Bind join disclosures to the Coordinator owner identity embedded in join
    /// tickets. Without this identity, preview requests fail closed.
    pub fn with_node_identity(mut self, identity: Identity) -> Self {
        self.node_identity = Some(Arc::new(identity));
        self
    }

    /// Attach the durable approval store. Without one, approval messages fail
    /// closed while registration and data-plane signaling remain available.
    pub fn with_join_request_store(mut self, store: Arc<JoinRequestStore>) -> Self {
        self.join_request_store = Some(store);
        self.refresh_admin_recovery_service();
        self
    }

    /// Build the approval store from the same persisted coordinator config used
    /// for allocation and revocation.
    pub fn with_join_request_store_from_config(
        mut self,
        state_dir: impl AsRef<std::path::Path>,
        config: &CoordinatorConfig,
    ) -> Result<Self> {
        let network_record_path = config
            .network_record_path
            .clone()
            .context("审批操作需要可信 network_record_path")?;
        self.network_record_path = Some(network_record_path.clone());
        let lifecycle = Arc::new(MemberLifecycleStore::new(
            state_dir.as_ref().join("member_lifecycle.json"),
            network_record_path.clone(),
        )?);
        let store = JoinRequestStore::new(
            state_dir.as_ref().join("join_requests.json"),
            Some(network_record_path),
            config.owner_node_id.clone(),
            config.administrator_node_ids.clone(),
        )?
        .with_member_lifecycle_store(lifecycle.clone());
        self.join_request_store = Some(Arc::new(store));
        self.member_lifecycle_store = Some(lifecycle);
        self.refresh_admin_recovery_service();
        Ok(self)
    }

    /// Attach the durable invitation ledger. Without one, invitation messages
    /// fail closed while unrelated signaling continues to work.
    pub fn with_invitation_store(mut self, store: Arc<InvitationStore>) -> Self {
        self.invitation_store = Some(store);
        self.refresh_admin_recovery_service();
        self
    }

    pub fn with_invitation_store_from_config(
        mut self,
        state_dir: impl AsRef<std::path::Path>,
        config: &CoordinatorConfig,
    ) -> Result<Self> {
        let network_record_path = config
            .network_record_path
            .clone()
            .context("邀请兑换需要可信 network_record_path")?;
        self.network_record_path = Some(network_record_path.clone());
        let lifecycle = Arc::new(MemberLifecycleStore::new(
            state_dir.as_ref().join("member_lifecycle.json"),
            network_record_path.clone(),
        )?);
        let store = InvitationStore::new(
            state_dir.as_ref().join("invitations.json"),
            Some(network_record_path),
            config.owner_node_id.clone(),
            config.administrator_node_ids.clone(),
        )?
        .with_member_lifecycle_store(lifecycle.clone());
        self.invitation_store = Some(Arc::new(store));
        self.member_lifecycle_store = Some(lifecycle);
        self.refresh_admin_recovery_service();
        Ok(self)
    }

    /// Attach the durable administrator recovery ledger. When the invitation
    /// and join-request stores are also present, this immediately builds the
    /// single service instance that coordinates roster recovery with local
    /// credential invalidation.
    pub fn with_admin_recovery_store(mut self, store: Arc<AdminRecoveryStore>) -> Self {
        self.admin_recovery_store = Some(store);
        self.refresh_admin_recovery_service();
        self
    }

    pub fn with_admin_recovery_store_from_config(
        mut self,
        state_dir: impl AsRef<std::path::Path>,
        config: &CoordinatorConfig,
    ) -> Result<Self> {
        let network_record_path = config
            .network_record_path
            .clone()
            .context("管理员恢复需要可信 network_record_path")?;
        self.network_record_path = Some(network_record_path.clone());
        let store = Arc::new(AdminRecoveryStore::new(
            state_dir.as_ref().join("admin_recovery.json"),
            network_record_path,
        )?);
        self.admin_recovery_store = Some(store);
        self.refresh_admin_recovery_service();
        Ok(self)
    }

    /// Read the configured recovery ledger and orchestration service. Both are
    /// absent unless explicitly attached, so disaster recovery fails closed
    /// instead of silently creating a hidden authority path.
    pub fn admin_recovery_store(&self) -> Option<Arc<AdminRecoveryStore>> {
        self.admin_recovery_store.clone()
    }

    pub fn admin_recovery_service(&self) -> Option<Arc<AdminRecoveryService>> {
        self.admin_recovery_service.clone()
    }

    fn refresh_admin_recovery_service(&mut self) {
        let (Some(store), Some(invitation_store), Some(join_request_store)) = (
            self.admin_recovery_store.as_ref(),
            self.invitation_store.as_ref(),
            self.join_request_store.as_ref(),
        ) else {
            self.admin_recovery_service = None;
            return;
        };
        self.admin_recovery_service = Some(Arc::new(AdminRecoveryService::new(
            store.clone(),
            invitation_store.clone(),
            join_request_store.clone(),
            self.session_manager.clone(),
        )));
    }

    /// Attach the durable unified membership lifecycle ledger. Management
    /// actions fail closed when it is absent.
    pub fn with_member_lifecycle_store(mut self, store: Arc<MemberLifecycleStore>) -> Self {
        self.member_lifecycle_store = Some(store);
        self
    }

    pub fn with_member_lifecycle_store_from_config(
        mut self,
        state_dir: impl AsRef<std::path::Path>,
        config: &CoordinatorConfig,
    ) -> Result<Self> {
        let network_record_path = config
            .network_record_path
            .clone()
            .context("成员生命周期需要可信 network_record_path")?;
        self.network_record_path = Some(network_record_path.clone());
        let store = MemberLifecycleStore::new(
            state_dir.as_ref().join("member_lifecycle.json"),
            network_record_path,
        )?;
        self.member_lifecycle_store = Some(Arc::new(store));
        Ok(self)
    }

    /// Attach the durable approval-gated Exit contribution ledger. Without one,
    /// Exit governance messages fail closed while unrelated signaling remains
    /// available.
    pub fn with_exit_contribution_store(
        mut self,
        store: Arc<ExitContributionRequestStore>,
    ) -> Self {
        self.exit_contribution_store = Some(store);
        self
    }

    pub fn with_exit_contribution_store_from_config(
        mut self,
        state_dir: impl AsRef<std::path::Path>,
        config: &CoordinatorConfig,
    ) -> Result<Self> {
        let network_record_path = config
            .network_record_path
            .clone()
            .context("Exit 贡献治理需要可信 network_record_path")?;
        self.network_record_path = Some(network_record_path.clone());
        self.exit_contribution_store = Some(Arc::new(ExitContributionRequestStore::new(
            state_dir.as_ref().join("exit_contribution_requests.json"),
            network_record_path,
        )));
        Ok(self)
    }

    /// Attach the durable Coordinator candidate configuration/declaration
    /// ledger. Candidate operations fail closed when it is absent.
    pub fn with_coordinator_candidate_store(
        mut self,
        store: Arc<CoordinatorCandidateStore>,
    ) -> Self {
        self.coordinator_candidate_store = Some(store);
        self
    }

    pub fn with_coordinator_candidate_store_from_config(
        mut self,
        state_dir: impl AsRef<std::path::Path>,
        config: &CoordinatorConfig,
    ) -> Result<Self> {
        let network_record_path = config
            .network_record_path
            .clone()
            .context("Coordinator 候选组需要可信 network_record_path")?;
        self.network_record_path = Some(network_record_path.clone());
        self.coordinator_candidate_store = Some(Arc::new(CoordinatorCandidateStore::new(
            state_dir.as_ref().join("coordinator_candidates.json"),
            network_record_path,
        )?));
        Ok(self)
    }

    pub fn session_manager(&self) -> Arc<SessionManager> {
        self.session_manager.clone()
    }

    /// 启动服务器
    pub async fn run(self) -> Result<()> {
        let listener = match self.listener {
            Some(listener) => listener,
            None => TcpListener::bind(&self.bind_addr)
                .await
                .with_context(|| format!("无法监听信令服务: {}", self.bind_addr))?,
        };
        let local_addr = listener.local_addr().context("无法读取信令服务监听地址")?;
        tracing::info!("信令服务器正在监听: {local_addr}");

        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    tracing::info!("新连接来自: {}", addr);
                    let session_manager = self.session_manager.clone();
                    let relay_directory = self.relay_directory.clone();
                    let join_request_store = self.join_request_store.clone();
                    let invitation_store = self.invitation_store.clone();
                    let admin_recovery_store = self.admin_recovery_store.clone();
                    let admin_recovery_service = self.admin_recovery_service.clone();
                    let member_lifecycle_store = self.member_lifecycle_store.clone();
                    let exit_contribution_store = self.exit_contribution_store.clone();
                    let coordinator_candidate_store = self.coordinator_candidate_store.clone();
                    let exit_availability = self.exit_availability.clone();
                    let network_record_path = self.network_record_path.clone();
                    let node_identity = self.node_identity.clone();

                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(
                            stream,
                            addr,
                            session_manager,
                            relay_directory,
                            join_request_store,
                            invitation_store,
                            admin_recovery_store,
                            admin_recovery_service,
                            member_lifecycle_store,
                            exit_contribution_store,
                            coordinator_candidate_store,
                            exit_availability,
                            network_record_path,
                            node_identity,
                        )
                        .await
                        {
                            tracing::error!("连接错误 {}: {:?}", addr, e);
                        }
                    });
                }
                Err(e) => {
                    tracing::error!("接受连接失败: {:?}", e);
                }
            }
        }
    }

    pub fn spawn_timeout_cleanup(&self, timeout_secs: u64) {
        let session_manager = self.session_manager.clone();
        let timeout = Duration::from_secs(timeout_secs.max(1));
        let tick = Duration::from_secs((timeout_secs / 2).clamp(1, 30));

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tick);
            loop {
                interval.tick().await;
                let stale = session_manager.cleanup_stale_sessions(timeout).await;
                if !stale.is_empty() {
                    tracing::info!("已清理 {} 个心跳超时客户端", stale.len());
                }
            }
        });

        if let Some(store) = self.coordinator_candidate_store.clone() {
            let session_manager = self.session_manager.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(COORDINATOR_EMERGENCY_RECONCILE_INTERVAL);
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    interval.tick().await;
                    if let Err(error) =
                        reconcile_coordinator_emergency_leases(&store, &session_manager).await
                    {
                        tracing::warn!(%error, "协调紧急 Coordinator Leader 在线状态失败");
                    }
                }
            });
        }
    }
}

fn signaling_websocket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_SIGNALING_MESSAGE_SIZE))
        .max_frame_size(Some(MAX_SIGNALING_MESSAGE_SIZE))
}

/// 处理单个WebSocket连接
#[allow(clippy::too_many_arguments)]
async fn handle_connection(
    stream: TcpStream,
    addr: SocketAddr,
    session_manager: Arc<SessionManager>,
    relay_directory: RelayEndpointDirectory,
    join_request_store: Option<Arc<JoinRequestStore>>,
    invitation_store: Option<Arc<InvitationStore>>,
    admin_recovery_store: Option<Arc<AdminRecoveryStore>>,
    admin_recovery_service: Option<Arc<AdminRecoveryService>>,
    member_lifecycle_store: Option<Arc<MemberLifecycleStore>>,
    exit_contribution_store: Option<Arc<ExitContributionRequestStore>>,
    coordinator_candidate_store: Option<Arc<CoordinatorCandidateStore>>,
    exit_availability: Arc<ExitAvailabilityObservationRegistry>,
    network_record_path: Option<PathBuf>,
    node_identity: Option<Arc<Identity>>,
) -> Result<()> {
    let local_addr = stream.local_addr().ok();
    let ws_stream = accept_async_with_config(stream, Some(signaling_websocket_config()))
        .await
        .with_context(|| format!("WebSocket 握手失败: {addr}"))?;
    tracing::info!("WebSocket 连接已建立: {}", addr);

    let (tx, rx) = mpsc::unbounded_channel::<Message>();
    let mut client_id: Option<String> = None;
    let (ws_sender, mut ws_receiver) = ws_stream.split();
    let send_task = spawn_signaling_sender(ws_sender, rx);

    while let Some(msg) = ws_receiver.next().await {
        if !handle_connection_message(
            msg,
            addr,
            &session_manager,
            &tx,
            &mut client_id,
            local_addr,
            &relay_directory,
            join_request_store.as_deref(),
            invitation_store.as_deref(),
            admin_recovery_store.as_deref(),
            admin_recovery_service.as_deref(),
            member_lifecycle_store.as_deref(),
            exit_contribution_store.as_deref(),
            coordinator_candidate_store.as_deref(),
            exit_availability.as_ref(),
            network_record_path.as_deref(),
            node_identity.as_deref(),
        )
        .await
        {
            break;
        }
    }

    unregister_signaling_client(client_id, &session_manager, &tx).await;
    send_task.abort();
    Ok(())
}

fn spawn_signaling_sender<S>(
    mut ws_sender: S,
    mut rx: mpsc::UnboundedReceiver<Message>,
) -> tokio::task::JoinHandle<()>
where
    S: futures::Sink<Message> + Unpin + Send + 'static,
    S::Error: std::fmt::Debug,
{
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if ws_sender.send(msg).await.is_err() {
                break;
            }
        }
    })
}

#[allow(clippy::too_many_arguments)]
async fn handle_connection_message(
    msg: Result<Message, tokio_tungstenite::tungstenite::Error>,
    addr: SocketAddr,
    session_manager: &Arc<SessionManager>,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &mut Option<String>,
    local_addr: Option<SocketAddr>,
    relay_directory: &RelayEndpointDirectory,
    join_request_store: Option<&JoinRequestStore>,
    invitation_store: Option<&InvitationStore>,
    admin_recovery_store: Option<&AdminRecoveryStore>,
    admin_recovery_service: Option<&AdminRecoveryService>,
    member_lifecycle_store: Option<&MemberLifecycleStore>,
    exit_contribution_store: Option<&ExitContributionRequestStore>,
    coordinator_candidate_store: Option<&CoordinatorCandidateStore>,
    exit_availability: &ExitAvailabilityObservationRegistry,
    network_record_path: Option<&std::path::Path>,
    node_identity: Option<&Identity>,
) -> bool {
    match msg {
        Ok(Message::Binary(data)) => {
            handle_binary_message(
                &data,
                addr,
                session_manager,
                tx,
                client_id,
                local_addr,
                relay_directory,
                join_request_store,
                invitation_store,
                admin_recovery_store,
                admin_recovery_service,
                member_lifecycle_store,
                exit_contribution_store,
                coordinator_candidate_store,
                exit_availability,
                network_record_path,
                node_identity,
            )
            .await;
            true
        }
        Ok(Message::Ping(data)) => tx.send(Message::Pong(data)).is_ok(),
        Ok(Message::Close(_)) => {
            tracing::info!("客户端主动关闭 WebSocket 连接: {}", addr);
            false
        }
        Err(e) => {
            tracing::error!("WebSocket 链路异常: {:?}", e);
            false
        }
        _ => true,
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_binary_message(
    data: &[u8],
    addr: SocketAddr,
    session_manager: &Arc<SessionManager>,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &mut Option<String>,
    local_addr: Option<SocketAddr>,
    relay_directory: &RelayEndpointDirectory,
    join_request_store: Option<&JoinRequestStore>,
    invitation_store: Option<&InvitationStore>,
    admin_recovery_store: Option<&AdminRecoveryStore>,
    admin_recovery_service: Option<&AdminRecoveryService>,
    member_lifecycle_store: Option<&MemberLifecycleStore>,
    exit_contribution_store: Option<&ExitContributionRequestStore>,
    coordinator_candidate_store: Option<&CoordinatorCandidateStore>,
    exit_availability: &ExitAvailabilityObservationRegistry,
    network_record_path: Option<&std::path::Path>,
    node_identity: Option<&Identity>,
) {
    tracing::debug!("收到二进制消息: {} 字节 (来自 {})", data.len(), addr);
    session_manager.record_signaling_rx(data.len());
    if let Err(error) = validate_signaling_message_len(data.len()) {
        tracing::warn!("拒绝超限信令帧 (来自 {addr}): {error:#}");
        return;
    }

    match codec::decode::<SignalingMessage>(data) {
        Ok(msg) => {
            if let Err(e) = handle_signaling_message(
                msg,
                &addr,
                session_manager,
                tx,
                client_id,
                local_addr,
                relay_directory,
                join_request_store,
                invitation_store,
                admin_recovery_store,
                admin_recovery_service,
                member_lifecycle_store,
                exit_contribution_store,
                coordinator_candidate_store,
                exit_availability,
                network_record_path,
                node_identity,
            )
            .await
            {
                tracing::error!("处理消息错误: {:?}", e);
                let error_msg = SignalingMessage::protocol_error(
                    ErrorCode::Internal,
                    format!("处理信令消息失败: {e:#}"),
                );
                let _ = send_binary(tx, session_manager, error_msg);
            }
        }
        Err(e) => {
            tracing::error!("反序列化失败: {:?}", e);
            tracing::error!(
                "数据长度: {}, 数据内容: {:?}",
                data.len(),
                &data[..data.len().min(64)]
            );
        }
    }
}

async fn unregister_signaling_client(
    client_id: Option<String>,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
) {
    if let Some(id) = client_id {
        session_manager
            .unregister_client_if_sender_matches(&id, tx)
            .await;
        tracing::info!("信令客户端已下线: {}", id);
    }
}

fn send_binary(
    tx: &mpsc::UnboundedSender<Message>,
    session_manager: &SessionManager,
    msg: SignalingMessage,
) -> Result<()> {
    let data = codec::encode(&msg).context("无法序列化信令消息")?;
    validate_signaling_message_len(data.len()).context("信令消息超过大小上限")?;
    let bytes = data.len();
    tx.send(Message::Binary(data.into()))
        .context("无法发送信令消息到连接任务")?;
    session_manager.record_signaling_tx(bytes);
    Ok(())
}

/// 处理信令消息
#[allow(clippy::too_many_arguments)]
async fn handle_signaling_message(
    msg: SignalingMessage,
    addr: &SocketAddr,
    session_manager: &Arc<SessionManager>,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &mut Option<String>,
    local_addr: Option<SocketAddr>,
    relay_directory: &RelayEndpointDirectory,
    join_request_store: Option<&JoinRequestStore>,
    invitation_store: Option<&InvitationStore>,
    admin_recovery_store: Option<&AdminRecoveryStore>,
    admin_recovery_service: Option<&AdminRecoveryService>,
    member_lifecycle_store: Option<&MemberLifecycleStore>,
    exit_contribution_store: Option<&ExitContributionRequestStore>,
    coordinator_candidate_store: Option<&CoordinatorCandidateStore>,
    exit_availability: &ExitAvailabilityObservationRegistry,
    network_record_path: Option<&std::path::Path>,
    node_identity: Option<&Identity>,
) -> Result<()> {
    match msg {
        SignalingMessage::Register { .. } => {
            handle_register_message(
                msg,
                addr,
                session_manager,
                tx,
                client_id,
                local_addr,
                relay_directory,
            )
            .await?;
            if let Some(store) = coordinator_candidate_store {
                if let Err(error) =
                    reconcile_coordinator_emergency_leases(store, session_manager).await
                {
                    tracing::warn!(%error, "注册后协调紧急 Coordinator Leader 在线状态失败");
                }
            }
        }
        SignalingMessage::ConnectRequest { target_id } => {
            handle_connect_request(target_id, session_manager, tx, client_id).await?;
        }
        SignalingMessage::Heartbeat { timestamp } => {
            handle_heartbeat(timestamp, session_manager, tx, client_id).await?;
        }
        SignalingMessage::RelayPacket {
            src_ip,
            dst_ip,
            payload,
        } => {
            handle_relay_packet(src_ip, dst_ip, payload, session_manager, tx, client_id).await?;
        }
        SignalingMessage::SecureControlPacket {
            src_ip,
            dst_ip,
            payload,
        } => {
            handle_secure_control_packet(src_ip, dst_ip, payload, session_manager, tx, client_id)
                .await?;
        }
        SignalingMessage::JoinRequestSubmit { request } => {
            handle_join_request_submit(
                *request,
                session_manager,
                tx,
                client_id,
                join_request_store,
            )
            .await?;
        }
        SignalingMessage::JoinRequestList { network_id } => {
            handle_join_request_list(
                network_id,
                session_manager,
                tx,
                client_id,
                join_request_store,
            )
            .await?;
        }
        SignalingMessage::JoinRequestReview {
            network_id,
            request_id,
            expected_revision,
            action,
            note,
        } => {
            handle_join_request_review(
                network_id,
                vec![JoinRequestBatchReviewItem {
                    request_id,
                    expected_revision,
                    action,
                    note,
                }],
                session_manager,
                tx,
                client_id,
                join_request_store,
                coordinator_candidate_store,
            )
            .await?;
        }
        SignalingMessage::JoinRequestBatchReview {
            network_id,
            reviews,
        } => {
            handle_join_request_review(
                network_id,
                reviews,
                session_manager,
                tx,
                client_id,
                join_request_store,
                coordinator_candidate_store,
            )
            .await?;
        }
        SignalingMessage::JoinRequestWithdraw {
            network_id,
            request_id,
        } => {
            handle_join_request_withdraw(
                network_id,
                request_id,
                session_manager,
                tx,
                client_id,
                join_request_store,
            )
            .await?;
        }
        SignalingMessage::JoinRequestResubmit {
            network_id,
            request_id,
            expected_revision,
            reason,
        } => {
            handle_join_request_resubmit(
                network_id,
                request_id,
                expected_revision,
                reason,
                session_manager,
                tx,
                client_id,
                join_request_store,
            )
            .await?;
        }
        SignalingMessage::InvitationCreate { request } => {
            handle_invitation_create(
                request,
                session_manager,
                tx,
                client_id,
                invitation_store,
                coordinator_candidate_store,
            )
            .await?;
        }
        SignalingMessage::InvitationList { network_id } => {
            handle_invitation_list(network_id, session_manager, tx, client_id, invitation_store)
                .await?;
        }
        SignalingMessage::InvitationStatus {
            network_id,
            invitation_id,
        } => {
            handle_invitation_status(
                network_id,
                invitation_id,
                session_manager,
                tx,
                client_id,
                invitation_store,
            )
            .await?;
        }
        SignalingMessage::InvitationRevoke {
            network_id,
            invitation_id,
            expected_revision,
        } => {
            handle_invitation_revoke(
                network_id,
                invitation_id,
                expected_revision,
                session_manager,
                tx,
                client_id,
                invitation_store,
                coordinator_candidate_store,
            )
            .await?;
        }
        SignalingMessage::InvitationPreview { request } => {
            handle_invitation_preview(request, session_manager, tx, client_id, invitation_store)
                .await?;
        }
        SignalingMessage::InvitationRedeem { request } => {
            handle_invitation_redeem(
                request,
                session_manager,
                tx,
                client_id,
                invitation_store,
                coordinator_candidate_store,
            )
            .await?;
        }
        SignalingMessage::NetworkJoinPreview { request } => {
            handle_network_join_preview(
                request,
                session_manager,
                tx,
                client_id,
                network_record_path,
                node_identity,
            )
            .await?;
        }
        SignalingMessage::MemberDirectoryList { network_id } => {
            handle_member_directory_list(
                network_id,
                session_manager,
                tx,
                client_id,
                join_request_store,
                invitation_store,
                member_lifecycle_store,
                exit_availability,
            )
            .await?;
        }
        SignalingMessage::MemberRemovalPreview { request } => {
            handle_member_removal_preview(
                request,
                session_manager,
                tx,
                client_id,
                member_lifecycle_store,
            )
            .await?;
        }
        SignalingMessage::MemberLifecycleChange { request } => {
            handle_member_lifecycle_change(
                request,
                session_manager,
                tx,
                client_id,
                member_lifecycle_store,
                coordinator_candidate_store,
            )
            .await?;
        }
        SignalingMessage::ExitContributionSubmit {
            network_id,
            request_id,
            expected_policy_revision,
            expected_terms_hash,
            requested_bandwidth_kbps,
            requested_traffic_quota_bytes,
        } => {
            handle_exit_contribution_submit(
                network_id,
                request_id,
                expected_policy_revision,
                expected_terms_hash,
                requested_bandwidth_kbps,
                requested_traffic_quota_bytes,
                session_manager,
                tx,
                client_id,
                exit_contribution_store,
            )
            .await?;
        }
        SignalingMessage::ExitContributionList { network_id } => {
            handle_exit_contribution_list(
                network_id,
                session_manager,
                tx,
                client_id,
                exit_contribution_store,
            )
            .await?;
        }
        SignalingMessage::ExitContributionStatus {
            network_id,
            request_id,
        } => {
            handle_exit_contribution_status(
                network_id,
                request_id,
                session_manager,
                tx,
                client_id,
                exit_contribution_store,
            )
            .await?;
        }
        SignalingMessage::ExitContributionReview {
            network_id,
            request_id,
            expected_revision,
            action,
            decision_reason_code,
        } => {
            handle_exit_contribution_review(
                network_id,
                request_id,
                expected_revision,
                action,
                decision_reason_code,
                session_manager,
                tx,
                client_id,
                exit_contribution_store,
                coordinator_candidate_store,
            )
            .await?;
        }
        SignalingMessage::ExitContributionWithdraw {
            network_id,
            request_id,
            expected_revision,
        } => {
            handle_exit_contribution_withdraw(
                network_id,
                request_id,
                expected_revision,
                session_manager,
                tx,
                client_id,
                exit_contribution_store,
            )
            .await?;
        }
        SignalingMessage::ExitContributionAuthorizationList { network_id } => {
            handle_exit_contribution_authorization_list(
                network_id,
                session_manager,
                tx,
                client_id,
                exit_contribution_store,
            )
            .await?;
        }
        SignalingMessage::ExitAvailabilityReport {
            network_id,
            available,
            reason_code,
        } => {
            handle_exit_availability_report(
                network_id,
                available,
                reason_code,
                session_manager,
                tx,
                client_id,
                join_request_store,
                member_lifecycle_store,
                exit_contribution_store,
                exit_availability,
            )
            .await?;
        }
        SignalingMessage::CoordinatorCandidateConfigUpdate {
            expected_revision,
            config,
        } => {
            handle_coordinator_candidate_config_update(
                expected_revision,
                config,
                None,
                session_manager,
                tx,
                client_id,
                coordinator_candidate_store,
            )
            .await?;
        }
        SignalingMessage::CoordinatorCandidateDeclarationPublish { declaration } => {
            handle_coordinator_candidate_declaration_publish(
                declaration,
                session_manager,
                tx,
                client_id,
                coordinator_candidate_store,
            )
            .await?;
        }
        SignalingMessage::CoordinatorCandidateStatusRequest { network_id } => {
            handle_coordinator_candidate_status(
                network_id,
                session_manager,
                tx,
                client_id,
                coordinator_candidate_store,
            )
            .await?;
        }
        SignalingMessage::CoordinatorRecoveryRequest { report } => {
            handle_coordinator_recovery(
                report,
                session_manager,
                tx,
                client_id,
                coordinator_candidate_store,
            )
            .await?;
        }
        SignalingMessage::CoordinatorEmergencyLeaderIssue { network_id } => {
            handle_coordinator_emergency_leader_issue(
                network_id,
                session_manager,
                tx,
                client_id,
                coordinator_candidate_store,
            )
            .await?;
        }
        SignalingMessage::CoordinatorEmergencyLeaderRenew {
            network_id,
            credential,
        } => {
            handle_coordinator_emergency_leader_renew(
                network_id,
                credential,
                session_manager,
                tx,
                client_id,
                coordinator_candidate_store,
            )
            .await?;
        }
        SignalingMessage::CoordinatorCandidateConfigUpdateAuthorized {
            expected_revision,
            config,
            credential,
        } => {
            handle_coordinator_candidate_config_update(
                expected_revision,
                config,
                Some(credential),
                session_manager,
                tx,
                client_id,
                coordinator_candidate_store,
            )
            .await?;
        }
        SignalingMessage::CoordinatorTermInstall { term } => {
            handle_coordinator_term_install(
                term,
                session_manager,
                tx,
                client_id,
                coordinator_candidate_store,
            )
            .await?;
        }
        SignalingMessage::CoordinatorTermLeaseRenew {
            credential,
            new_expires_at_unix_ms,
        } => {
            handle_coordinator_term_lease_renew(
                credential,
                new_expires_at_unix_ms,
                session_manager,
                tx,
                client_id,
                coordinator_candidate_store,
            )
            .await?;
        }
        SignalingMessage::CoordinatorElectionPropose { proposal } => {
            handle_coordinator_election_propose(
                proposal,
                session_manager,
                tx,
                client_id,
                coordinator_candidate_store,
            )
            .await?;
        }
        SignalingMessage::CoordinatorElectionVote { proposal_id, vote } => {
            handle_coordinator_election_vote(
                proposal_id,
                vote,
                session_manager,
                tx,
                client_id,
                coordinator_candidate_store,
            )
            .await?;
        }
        SignalingMessage::CoordinatorElectionTimeout {
            network_id,
            proposal_id,
        } => {
            handle_coordinator_election_timeout(
                network_id,
                proposal_id,
                session_manager,
                tx,
                client_id,
                coordinator_candidate_store,
            )
            .await?;
        }
        SignalingMessage::AdminRecoverySubmit { request } => {
            handle_admin_recovery_submit(
                *request,
                session_manager,
                tx,
                client_id,
                admin_recovery_store,
            )
            .await?;
        }
        SignalingMessage::AdminRecoveryStatus {
            network_id,
            request_id,
        } => {
            handle_admin_recovery_status(
                network_id,
                request_id,
                session_manager,
                tx,
                client_id,
                admin_recovery_store,
            )
            .await?;
        }
        SignalingMessage::AdminRecoveryList { network_id } => {
            handle_admin_recovery_list(
                network_id,
                session_manager,
                tx,
                client_id,
                admin_recovery_store,
            )
            .await?;
        }
        SignalingMessage::AdminRecoveryRevoke {
            network_id,
            request_id,
            reason,
        } => {
            handle_admin_recovery_revoke(
                network_id,
                request_id,
                reason,
                session_manager,
                tx,
                client_id,
                admin_recovery_store,
            )
            .await?;
        }
        SignalingMessage::AdminRecoveryExecute {
            network_id,
            request_id,
        } => {
            handle_admin_recovery_execute(
                network_id,
                request_id,
                session_manager,
                tx,
                client_id,
                admin_recovery_store,
                admin_recovery_service,
            )
            .await?;
        }
        // Accepting a server-originated response from a client would let it
        // manufacture approval or recovery state in another view.
        message if is_server_originated_response(&message) => {
            send_binary(
                tx,
                session_manager,
                SignalingMessage::protocol_error(
                    ErrorCode::BadRequest,
                    "This response message is server-originated",
                ),
            )?;
        }
        _ => {
            tracing::warn!("收到未预期的信令消息类型: {:?}", msg);
        }
    }

    Ok(())
}

fn is_server_originated_response(message: &SignalingMessage) -> bool {
    matches!(
        message,
        SignalingMessage::JoinRequestStatus { .. }
            | SignalingMessage::JoinRequestListResponse { .. }
            | SignalingMessage::JoinRequestReviewAck { .. }
            | SignalingMessage::JoinRequestChanged { .. }
            | SignalingMessage::InvitationCreateAck { .. }
            | SignalingMessage::InvitationListAck { .. }
            | SignalingMessage::InvitationStatusAck { .. }
            | SignalingMessage::InvitationRevokeAck { .. }
            | SignalingMessage::InvitationPreviewAck { .. }
            | SignalingMessage::InvitationRedeemAck { .. }
            | SignalingMessage::InvitationChanged { .. }
            | SignalingMessage::MemberDirectoryListAck { .. }
            | SignalingMessage::MemberRemovalPreviewAck { .. }
            | SignalingMessage::MemberLifecycleChangeAck { .. }
            | SignalingMessage::MemberLifecycleChanged { .. }
            | SignalingMessage::ExitContributionSubmitAck { .. }
            | SignalingMessage::ExitContributionListAck { .. }
            | SignalingMessage::ExitContributionStatusAck { .. }
            | SignalingMessage::ExitContributionReviewAck { .. }
            | SignalingMessage::ExitContributionWithdrawAck { .. }
            | SignalingMessage::ExitContributionAuthorizationListAck { .. }
            | SignalingMessage::ExitContributionChanged { .. }
            | SignalingMessage::ExitAvailabilityReportAck { .. }
            | SignalingMessage::NetworkJoinPreviewAck { .. }
            | SignalingMessage::CoordinatorCandidateConfigAck { .. }
            | SignalingMessage::CoordinatorCandidateDeclarationAck { .. }
            | SignalingMessage::CoordinatorCandidateStatusAck { .. }
            | SignalingMessage::CoordinatorCandidateStatusChanged { .. }
            | SignalingMessage::CoordinatorTermInstallAck { .. }
            | SignalingMessage::CoordinatorTermLeaseRenewAck { .. }
            | SignalingMessage::CoordinatorElectionProposeAck { .. }
            | SignalingMessage::CoordinatorElectionVoteAck { .. }
            | SignalingMessage::CoordinatorElectionTimeoutAck { .. }
            | SignalingMessage::CoordinatorElectionChanged { .. }
            | SignalingMessage::CoordinatorRecoveryResponse { .. }
            | SignalingMessage::CoordinatorEmergencyLeaderIssueAck { .. }
            | SignalingMessage::CoordinatorEmergencyLeaderRenewAck { .. }
            | SignalingMessage::AdminRecoverySubmitAck { .. }
            | SignalingMessage::AdminRecoveryStatusAck { .. }
            | SignalingMessage::AdminRecoveryListAck { .. }
            | SignalingMessage::AdminRecoveryRevokeAck { .. }
            | SignalingMessage::AdminRecoveryExecuteAck { .. }
    )
}

fn require_coordinator_candidate_store<'a>(
    store: Option<&'a CoordinatorCandidateStore>,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
) -> Result<Option<&'a CoordinatorCandidateStore>> {
    if let Some(store) = store {
        return Ok(Some(store));
    }
    send_binary(
        tx,
        session_manager,
        SignalingMessage::protocol_error(
            ErrorCode::BadRequest,
            "Coordinator candidate management is not configured on this Coordinator",
        ),
    )?;
    Ok(None)
}

fn guard_emergency_governance_write(
    store: Option<&CoordinatorCandidateStore>,
    actor: &NodeId,
    network_id: &NetworkId,
    action: &str,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
) -> Result<bool> {
    let Some(store) = store else {
        return Ok(false);
    };
    match store.reject_emergency_governance_write(actor, network_id, action, unix_time_ms()) {
        Ok(false) => Ok(false),
        Ok(true) => {
            send_binary(
                tx,
                session_manager,
                SignalingMessage::protocol_error(
                    ErrorCode::Forbidden,
                    "Governance is frozen during emergency Coordinator runtime authority",
                ),
            )?;
            Ok(true)
        }
        Err(error) => {
            tracing::warn!(%error, %actor, %network_id, %action, "紧急 Coordinator 治理冻结检查失败");
            send_binary(
                tx,
                session_manager,
                coordinator_candidate_protocol_error(&error),
            )?;
            Ok(true)
        }
    }
}

fn coordinator_candidate_protocol_error(error: &anyhow::Error) -> SignalingMessage {
    let text = error.to_string();
    let code = if text.contains("permission_denied")
        || text.contains("authenticated_actor_mismatch")
        || text.contains("config_expired")
        || text.contains("declaration_expired")
        || text.contains("declaration_from_future")
    {
        ErrorCode::Forbidden
    } else if text.contains("error.")
        || text.contains("domain.")
        || text.contains("store_lock_poisoned")
    {
        ErrorCode::BadRequest
    } else {
        ErrorCode::Internal
    };
    let message = if code == ErrorCode::Internal {
        "Coordinator candidate operation could not be processed".to_string()
    } else {
        format!("Coordinator candidate operation rejected: {text}")
    };
    SignalingMessage::protocol_error(code, message)
}

async fn broadcast_coordinator_candidate_status_changed(
    store: &CoordinatorCandidateStore,
    session_manager: &SessionManager,
    network_id: &NetworkId,
) -> Result<()> {
    let recipients = store.notification_recipients(network_id)?;
    let message = SignalingMessage::CoordinatorCandidateStatusChanged {
        network_id: network_id.clone(),
    };
    for peer in session_manager.get_all_peers().await {
        let node_id = NodeId::from_public_key(&peer.public_key);
        if recipients.contains(&node_id) {
            session_manager
                .send_to_client(&peer.client_id, message.clone())
                .await?;
        }
    }
    Ok(())
}

async fn reconcile_coordinator_emergency_leases(
    store: &CoordinatorCandidateStore,
    session_manager: &SessionManager,
) -> Result<()> {
    let now_unix_ms = unix_time_ms();
    let presence = session_manager.member_presence(now_unix_ms).await;
    let changed_networks = store.reconcile_emergency_leases(&presence, now_unix_ms)?;
    for network_id in changed_networks {
        broadcast_coordinator_candidate_status_changed(store, session_manager, &network_id).await?;
    }
    Ok(())
}

async fn broadcast_coordinator_election_changed(
    store: &CoordinatorCandidateStore,
    session_manager: &SessionManager,
    status: &fuckipv6_common::domain::CoordinatorCandidateStatus,
) -> Result<()> {
    let Some(election) = status.latest_election.as_ref() else {
        return Ok(());
    };
    let recipients = store.election_notification_recipients(&status.network_id)?;
    let message = SignalingMessage::CoordinatorElectionChanged {
        network_id: status.network_id.clone(),
        proposal_id: election.proposal.proposal_id.clone(),
        stage: election.stage,
    };
    for peer in session_manager.get_all_peers().await {
        let node_id = NodeId::from_public_key(&peer.public_key);
        if recipients.contains(&node_id) {
            session_manager
                .send_to_client(&peer.client_id, message.clone())
                .await?;
        }
    }
    Ok(())
}

async fn handle_coordinator_candidate_config_update(
    expected_revision: u64,
    config: fuckipv6_common::domain::CoordinatorCandidateConfig,
    credential: Option<CoordinatorWriteCredential>,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&CoordinatorCandidateStore>,
) -> Result<()> {
    let Some(store) = require_coordinator_candidate_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let network_id = config.network_id.clone();
    if config.signer != actor {
        send_binary(
            tx,
            session_manager,
            SignalingMessage::protocol_error(
                ErrorCode::Forbidden,
                "Coordinator candidate configuration must be signed by the authenticated actor",
            ),
        )?;
        return Ok(());
    }
    let now_unix_ms = unix_time_ms();
    let outcome = match credential.as_ref() {
        Some(credential) => store.update_config_authorized(
            &actor,
            expected_revision,
            config,
            credential,
            now_unix_ms,
        ),
        None => store.update_config(&actor, expected_revision, config, now_unix_ms),
    };
    match outcome {
        Ok(status) => {
            send_binary(
                tx,
                session_manager,
                SignalingMessage::CoordinatorCandidateConfigAck {
                    status: status.clone(),
                },
            )?;
            broadcast_coordinator_candidate_status_changed(store, session_manager, &network_id)
                .await?;
        }
        Err(error) => {
            tracing::warn!(%error, %actor, %network_id, "拒绝 Coordinator 候选配置更新");
            send_binary(
                tx,
                session_manager,
                coordinator_candidate_protocol_error(&error),
            )?;
        }
    }
    Ok(())
}

async fn handle_coordinator_term_install(
    term: CoordinatorTerm,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&CoordinatorCandidateStore>,
) -> Result<()> {
    let Some(store) = require_coordinator_candidate_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let network_id = term.network_id.clone();
    match store.install_term(&actor, term, unix_time_ms()) {
        Ok(status) => {
            send_binary(
                tx,
                session_manager,
                SignalingMessage::CoordinatorTermInstallAck {
                    status: status.clone(),
                },
            )?;
            broadcast_coordinator_candidate_status_changed(store, session_manager, &network_id)
                .await?;
        }
        Err(error) => {
            tracing::warn!(%error, %actor, %network_id, "拒绝 Coordinator 任期安装");
            send_binary(
                tx,
                session_manager,
                coordinator_candidate_protocol_error(&error),
            )?;
        }
    }
    Ok(())
}

async fn handle_coordinator_term_lease_renew(
    credential: CoordinatorWriteCredential,
    new_expires_at_unix_ms: u64,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&CoordinatorCandidateStore>,
) -> Result<()> {
    let Some(store) = require_coordinator_candidate_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let network_id = credential.network_id.clone();
    match store.renew_term_lease(&actor, &credential, new_expires_at_unix_ms, unix_time_ms()) {
        Ok(status) => {
            send_binary(
                tx,
                session_manager,
                SignalingMessage::CoordinatorTermLeaseRenewAck {
                    status: status.clone(),
                },
            )?;
            broadcast_coordinator_candidate_status_changed(store, session_manager, &network_id)
                .await?;
        }
        Err(error) => {
            tracing::warn!(%error, %actor, %network_id, "拒绝 Coordinator 任期续租");
            send_binary(
                tx,
                session_manager,
                coordinator_candidate_protocol_error(&error),
            )?;
        }
    }
    Ok(())
}

async fn handle_coordinator_election_propose(
    proposal: CoordinatorElectionProposal,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&CoordinatorCandidateStore>,
) -> Result<()> {
    let Some(store) = require_coordinator_candidate_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let network_id = proposal.network_id.clone();
    if proposal.proposer != actor {
        send_binary(
            tx,
            session_manager,
            SignalingMessage::protocol_error(
                ErrorCode::Forbidden,
                "Coordinator election proposals must be signed by the authenticated actor",
            ),
        )?;
        return Ok(());
    }
    match store.propose_election(&actor, proposal, unix_time_ms()) {
        Ok(status) => {
            send_binary(
                tx,
                session_manager,
                SignalingMessage::CoordinatorElectionProposeAck {
                    status: status.clone(),
                },
            )?;
            broadcast_coordinator_candidate_status_changed(store, session_manager, &network_id)
                .await?;
            broadcast_coordinator_election_changed(store, session_manager, &status).await?;
        }
        Err(error) => {
            tracing::warn!(%error, %actor, %network_id, "拒绝 Coordinator 选举提案");
            send_binary(
                tx,
                session_manager,
                coordinator_candidate_protocol_error(&error),
            )?;
        }
    }
    Ok(())
}

async fn handle_coordinator_election_vote(
    proposal_id: ContentHash,
    vote: CoordinatorElectionVote,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&CoordinatorCandidateStore>,
) -> Result<()> {
    let Some(store) = require_coordinator_candidate_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let network_id = vote.network_id.clone();
    if vote.voter != actor {
        send_binary(
            tx,
            session_manager,
            SignalingMessage::protocol_error(
                ErrorCode::Forbidden,
                "Coordinator election votes must be signed by the authenticated actor",
            ),
        )?;
        return Ok(());
    }
    match store.record_election_vote(&actor, &proposal_id, vote, unix_time_ms()) {
        Ok(status) => {
            send_binary(
                tx,
                session_manager,
                SignalingMessage::CoordinatorElectionVoteAck {
                    status: status.clone(),
                },
            )?;
            broadcast_coordinator_candidate_status_changed(store, session_manager, &network_id)
                .await?;
            broadcast_coordinator_election_changed(store, session_manager, &status).await?;
        }
        Err(error) => {
            tracing::warn!(%error, %actor, %network_id, "拒绝 Coordinator 选举投票");
            send_binary(
                tx,
                session_manager,
                coordinator_candidate_protocol_error(&error),
            )?;
        }
    }
    Ok(())
}

async fn handle_coordinator_election_timeout(
    network_id: NetworkId,
    proposal_id: ContentHash,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&CoordinatorCandidateStore>,
) -> Result<()> {
    let Some(store) = require_coordinator_candidate_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    match store.timeout_election(&actor, &network_id, &proposal_id, unix_time_ms()) {
        Ok(status) => {
            send_binary(
                tx,
                session_manager,
                SignalingMessage::CoordinatorElectionTimeoutAck {
                    status: status.clone(),
                },
            )?;
            broadcast_coordinator_candidate_status_changed(store, session_manager, &network_id)
                .await?;
            broadcast_coordinator_election_changed(store, session_manager, &status).await?;
        }
        Err(error) => {
            tracing::warn!(%error, %actor, %network_id, "拒绝 Coordinator 选举超时");
            send_binary(
                tx,
                session_manager,
                coordinator_candidate_protocol_error(&error),
            )?;
        }
    }
    Ok(())
}

async fn handle_coordinator_candidate_declaration_publish(
    declaration: fuckipv6_common::domain::CoordinatorCandidateDeclaration,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&CoordinatorCandidateStore>,
) -> Result<()> {
    let Some(store) = require_coordinator_candidate_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let network_id = declaration.network_id.clone();
    if declaration.node_id != actor {
        send_binary(
            tx,
            session_manager,
            SignalingMessage::protocol_error(
                ErrorCode::Forbidden,
                "Coordinator candidate declarations must be signed by the authenticated actor",
            ),
        )?;
        return Ok(());
    }
    match store.publish_declaration(&actor, declaration, unix_time_ms()) {
        Ok(status) => {
            send_binary(
                tx,
                session_manager,
                SignalingMessage::CoordinatorCandidateDeclarationAck {
                    status: status.clone(),
                },
            )?;
            broadcast_coordinator_candidate_status_changed(store, session_manager, &network_id)
                .await?;
        }
        Err(error) => {
            tracing::warn!(%error, %actor, %network_id, "拒绝 Coordinator 候选健康声明");
            send_binary(
                tx,
                session_manager,
                coordinator_candidate_protocol_error(&error),
            )?;
        }
    }
    Ok(())
}

async fn handle_coordinator_candidate_status(
    network_id: NetworkId,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&CoordinatorCandidateStore>,
) -> Result<()> {
    let Some(store) = require_coordinator_candidate_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    match store.status(&actor, &network_id, unix_time_ms()) {
        Ok(status) => {
            send_binary(
                tx,
                session_manager,
                SignalingMessage::CoordinatorCandidateStatusAck { status },
            )?;
        }
        Err(error) => {
            tracing::warn!(%error, %actor, %network_id, "拒绝 Coordinator 候选状态查询");
            send_binary(
                tx,
                session_manager,
                coordinator_candidate_protocol_error(&error),
            )?;
        }
    }
    Ok(())
}

async fn handle_coordinator_recovery(
    report: CoordinatorRecoveryReport,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&CoordinatorCandidateStore>,
) -> Result<()> {
    let Some(store) = require_coordinator_candidate_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let network_id = report.network_id.clone();
    match store.recover(&actor, report, unix_time_ms()) {
        Ok(response) => {
            send_binary(
                tx,
                session_manager,
                SignalingMessage::CoordinatorRecoveryResponse { response },
            )?;
            broadcast_coordinator_candidate_status_changed(store, session_manager, &network_id)
                .await?;
        }
        Err(error) => {
            tracing::warn!(%error, %actor, %network_id, "拒绝 Coordinator 旧 Leader 恢复请求");
            send_binary(
                tx,
                session_manager,
                coordinator_candidate_protocol_error(&error),
            )?;
        }
    }
    Ok(())
}

async fn handle_coordinator_emergency_leader_issue(
    network_id: NetworkId,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&CoordinatorCandidateStore>,
) -> Result<()> {
    let Some(store) = require_coordinator_candidate_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let now_unix_ms = unix_time_ms();
    let presence = session_manager.member_presence(now_unix_ms).await;
    let online_members = match store.emergency_online_members(&network_id, &presence) {
        Ok(online_members) => online_members,
        Err(error) => {
            tracing::warn!(%error, %actor, %network_id, "无法计算紧急 Leader 在线成员集");
            send_binary(
                tx,
                session_manager,
                coordinator_candidate_protocol_error(&error),
            )?;
            return Ok(());
        }
    };

    match store.issue_emergency_leader(&actor, &network_id, &online_members, now_unix_ms) {
        Ok(response) => {
            send_binary(
                tx,
                session_manager,
                SignalingMessage::CoordinatorEmergencyLeaderIssueAck { response },
            )?;
            broadcast_coordinator_candidate_status_changed(store, session_manager, &network_id)
                .await?;
        }
        Err(error) => {
            tracing::warn!(%error, %actor, %network_id, "拒绝紧急 Coordinator Leader 签发");
            send_binary(
                tx,
                session_manager,
                coordinator_candidate_protocol_error(&error),
            )?;
        }
    }
    Ok(())
}

async fn handle_coordinator_emergency_leader_renew(
    network_id: NetworkId,
    credential: fuckipv6_common::domain::CoordinatorEmergencyWriteCredential,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&CoordinatorCandidateStore>,
) -> Result<()> {
    let Some(store) = require_coordinator_candidate_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let now_unix_ms = unix_time_ms();
    let presence = session_manager.member_presence(now_unix_ms).await;
    let online_members = match store.emergency_online_members(&network_id, &presence) {
        Ok(online_members) => online_members,
        Err(error) => {
            tracing::warn!(%error, %actor, %network_id, "无法计算紧急 Leader 在线成员集");
            send_binary(
                tx,
                session_manager,
                coordinator_candidate_protocol_error(&error),
            )?;
            return Ok(());
        }
    };

    match store.renew_emergency_leader(
        &actor,
        &credential,
        &network_id,
        &online_members,
        now_unix_ms,
    ) {
        Ok(response) => {
            send_binary(
                tx,
                session_manager,
                SignalingMessage::CoordinatorEmergencyLeaderRenewAck { response },
            )?;
            broadcast_coordinator_candidate_status_changed(store, session_manager, &network_id)
                .await?;
        }
        Err(error) => {
            tracing::warn!(%error, %actor, %network_id, "拒绝紧急 Coordinator Leader 续租");
            send_binary(
                tx,
                session_manager,
                coordinator_candidate_protocol_error(&error),
            )?;
            let text = error.to_string();
            if text.contains("domain.coordinator_emergency_candidate_online")
                || text.contains("domain.coordinator_emergency_leader_offline")
            {
                broadcast_coordinator_candidate_status_changed(store, session_manager, &network_id)
                    .await?;
            }
        }
    }
    Ok(())
}

async fn authenticated_join_actor(
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
) -> Result<Option<(NodeId, Vec<u8>)>> {
    let Some(client_id) = client_id.as_deref() else {
        send_binary(
            tx,
            session_manager,
            SignalingMessage::protocol_error(
                ErrorCode::Unauthorized,
                "Client must register before using approval-based joining",
            ),
        )?;
        return Ok(None);
    };
    let Some(public_key) = session_manager.online_client_public_key(client_id).await else {
        send_binary(
            tx,
            session_manager,
            SignalingMessage::protocol_error(
                ErrorCode::Unauthorized,
                "Client registration is no longer active",
            ),
        )?;
        return Ok(None);
    };
    Ok(Some((NodeId::from_public_key(&public_key), public_key)))
}

fn require_admin_recovery_store<'a>(
    store: Option<&'a AdminRecoveryStore>,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
) -> Result<Option<&'a AdminRecoveryStore>> {
    if let Some(store) = store {
        return Ok(Some(store));
    }
    send_binary(
        tx,
        session_manager,
        SignalingMessage::protocol_error(
            ErrorCode::BadRequest,
            "Administrator recovery is not configured on this Coordinator",
        ),
    )?;
    Ok(None)
}

fn require_admin_recovery_service<'a>(
    service: Option<&'a AdminRecoveryService>,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
) -> Result<Option<&'a AdminRecoveryService>> {
    if let Some(service) = service {
        return Ok(Some(service));
    }
    send_binary(
        tx,
        session_manager,
        SignalingMessage::protocol_error(
            ErrorCode::BadRequest,
            "Administrator recovery execution is not configured on this Coordinator",
        ),
    )?;
    Ok(None)
}

async fn handle_admin_recovery_submit(
    request: AdminRecoveryRequest,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&AdminRecoveryStore>,
) -> Result<()> {
    let Some(store) = require_admin_recovery_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    match store.submit_as(&actor, request, unix_time_ms()) {
        Ok(status) => send_binary(
            tx,
            session_manager,
            SignalingMessage::AdminRecoverySubmitAck { status },
        )?,
        Err(error) => {
            tracing::warn!(%error, %actor, "管理员恢复请求提交失败");
            send_binary(tx, session_manager, admin_recovery_protocol_error(&error))?;
        }
    }
    Ok(())
}

async fn handle_admin_recovery_status(
    network_id: NetworkId,
    request_id: AdminRecoveryRequestId,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&AdminRecoveryStore>,
) -> Result<()> {
    let Some(store) = require_admin_recovery_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    match store.status_for_actor(&request_id, &actor, unix_time_ms()) {
        Ok(status) if status.network_id == network_id => send_binary(
            tx,
            session_manager,
            SignalingMessage::AdminRecoveryStatusAck { status },
        )?,
        Ok(_) => send_binary(
            tx,
            session_manager,
            SignalingMessage::protocol_error(
                ErrorCode::BadRequest,
                "admin_recovery.error.network_mismatch",
            ),
        )?,
        Err(error) => {
            tracing::warn!(%error, %actor, %request_id, "管理员恢复状态查询失败");
            send_binary(tx, session_manager, admin_recovery_protocol_error(&error))?;
        }
    }
    Ok(())
}

async fn handle_admin_recovery_list(
    network_id: NetworkId,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&AdminRecoveryStore>,
) -> Result<()> {
    let Some(store) = require_admin_recovery_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    match store.list_for_actor(&network_id, &actor, unix_time_ms()) {
        Ok(requests) => send_binary(
            tx,
            session_manager,
            SignalingMessage::AdminRecoveryListAck { requests },
        )?,
        Err(error) => {
            tracing::warn!(%error, %actor, %network_id, "管理员恢复列表查询失败");
            send_binary(tx, session_manager, admin_recovery_protocol_error(&error))?;
        }
    }
    Ok(())
}

async fn handle_admin_recovery_revoke(
    network_id: NetworkId,
    request_id: AdminRecoveryRequestId,
    reason: String,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&AdminRecoveryStore>,
) -> Result<()> {
    let Some(store) = require_admin_recovery_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let now_unix_ms = unix_time_ms();
    let result = (|| {
        let current = store.status(&request_id, now_unix_ms)?;
        if current.network_id != network_id {
            anyhow::bail!("admin_recovery.error.network_mismatch");
        }
        store.revoke(&request_id, &actor, reason, now_unix_ms)
    })();
    match result {
        Ok(status) => send_binary(
            tx,
            session_manager,
            SignalingMessage::AdminRecoveryRevokeAck { status },
        )?,
        Err(error) => {
            tracing::warn!(%error, %actor, %request_id, "管理员恢复撤销失败");
            send_binary(tx, session_manager, admin_recovery_protocol_error(&error))?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_admin_recovery_execute(
    network_id: NetworkId,
    request_id: AdminRecoveryRequestId,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&AdminRecoveryStore>,
    service: Option<&AdminRecoveryService>,
) -> Result<()> {
    let Some(store) = require_admin_recovery_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let Some(service) = require_admin_recovery_service(service, session_manager, tx)? else {
        return Ok(());
    };
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let Some(actor_client_id) = client_id.as_deref() else {
        return Ok(());
    };
    let now_unix_ms = unix_time_ms();
    let result = async {
        let visible = store.status_for_actor(&request_id, &actor, now_unix_ms)?;
        if visible.network_id != network_id {
            anyhow::bail!("admin_recovery.error.network_mismatch");
        }
        service
            .execute_authorized_before_actor_disconnect(
                &actor,
                &request_id,
                now_unix_ms,
                actor_client_id,
                |status| {
                    send_binary(
                        tx,
                        session_manager,
                        SignalingMessage::AdminRecoveryExecuteAck {
                            status: status.clone(),
                        },
                    )
                },
            )
            .await?;
        Ok(())
    }
    .await;
    match result {
        Ok(()) => {}
        Err(error) => {
            tracing::warn!(%error, %actor, %request_id, "管理员恢复执行失败");
            send_binary(tx, session_manager, admin_recovery_protocol_error(&error))?;
        }
    }
    Ok(())
}

/// Domain validation errors that carry an administrator-recovery code. They
/// fail closed inside the domain layer with a `domain.admin_recovery_*` code,
/// so without this translation a threshold or snapshot rejection would surface
/// to every client as a generic internal error and could not be told apart from
/// a real Coordinator fault. Keeping the mapping explicit means a new domain
/// code is never published to clients by accident.
const ADMIN_RECOVERY_DOMAIN_CODES: [(&str, &str); 4] = [
    (
        "domain.admin_recovery_threshold_not_met",
        "admin_recovery.error.threshold_not_met",
    ),
    (
        "domain.admin_recovery_expired",
        "admin_recovery.error.expired",
    ),
    (
        "domain.admin_recovery_stale_snapshot",
        "admin_recovery.error.stale_snapshot",
    ),
    (
        "domain.admin_recovery_network_mismatch",
        "admin_recovery.error.network_mismatch",
    ),
];

fn admin_recovery_domain_code(text: &str) -> Option<&'static str> {
    ADMIN_RECOVERY_DOMAIN_CODES
        .iter()
        .find(|(needle, _)| text.contains(needle))
        .map(|(_, public_key)| *public_key)
}

fn admin_recovery_protocol_error(error: &anyhow::Error) -> SignalingMessage {
    let text = format!("{error:#}");
    let domain_code = admin_recovery_domain_code(&text);
    let code = if text.contains("admin_recovery.error.permission_denied") {
        ErrorCode::Forbidden
    } else if text.contains("admin recovery request not found") {
        ErrorCode::NotFound
    } else if text.contains("admin_recovery.error.") || domain_code.is_some() {
        ErrorCode::BadRequest
    } else {
        ErrorCode::Internal
    };
    let message = match code {
        ErrorCode::Forbidden => "admin_recovery.error.permission_denied",
        ErrorCode::NotFound => "admin_recovery.error.not_found",
        ErrorCode::BadRequest if domain_code.is_some() => domain_code.unwrap_or_default(),
        ErrorCode::BadRequest => {
            const PUBLIC_KEYS: [&str; 11] = [
                "admin_recovery.error.already_executed",
                "admin_recovery.error.expired",
                "admin_recovery.error.invalid_revocation_reason",
                "admin_recovery.error.network_mismatch",
                "admin_recovery.error.not_executable",
                "admin_recovery.error.replayed",
                "admin_recovery.error.request_id_conflict",
                "admin_recovery.error.revoked",
                "admin_recovery.error.stale_snapshot",
                "admin_recovery.error.threshold_not_met",
                "admin_recovery.error.permission_denied",
            ];
            PUBLIC_KEYS
                .iter()
                .find(|key| text.contains(**key))
                .copied()
                .unwrap_or("admin_recovery.error.rejected")
        }
        _ => "admin_recovery.error.internal",
    };
    SignalingMessage::protocol_error(code, message)
}

fn require_join_request_store<'a>(
    store: Option<&'a JoinRequestStore>,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
) -> Result<Option<&'a JoinRequestStore>> {
    if let Some(store) = store {
        return Ok(Some(store));
    }
    send_binary(
        tx,
        session_manager,
        SignalingMessage::protocol_error(
            ErrorCode::BadRequest,
            "Approval-based joining is not configured on this Coordinator",
        ),
    )?;
    Ok(None)
}

fn join_request_error_message(error: &str) -> SignalingMessage {
    let code = if error.contains(GOVERNANCE_FROZEN_ERROR)
        || error.contains("join_request.error.permission_denied")
        || error.contains("join_request.error.reviewer_required")
        || error.contains("join_request.error.self_review_forbidden")
    {
        ErrorCode::Forbidden
    } else if error.contains("join_request.error.") || error.contains("join request") {
        ErrorCode::BadRequest
    } else {
        ErrorCode::Internal
    };
    let message = if code == ErrorCode::Internal {
        "Join request operation could not be processed"
    } else {
        error
    };
    SignalingMessage::protocol_error(code, message)
}

async fn broadcast_join_request_changed(
    store: &JoinRequestStore,
    session_manager: &SessionManager,
    network_id: &NetworkId,
    applicant: &NodeId,
    request_id: &JoinRequestId,
    state: JoinRequestState,
    revision: u64,
) -> Result<()> {
    let recipients = store.notification_recipients(network_id, applicant)?;
    let message = join_request_changed_message(request_id, state, revision);
    for peer in session_manager.get_all_peers().await {
        let node_id = NodeId::from_public_key(&peer.public_key);
        if recipients.contains(&node_id) {
            session_manager
                .send_to_client(&peer.client_id, message.clone())
                .await?;
        }
    }
    Ok(())
}

fn join_request_changed_message(
    request_id: &JoinRequestId,
    state: JoinRequestState,
    revision: u64,
) -> SignalingMessage {
    SignalingMessage::JoinRequestChanged {
        request_id: request_id.clone(),
        state,
        revision,
    }
}

async fn send_join_request_status(
    store: &JoinRequestStore,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    network_id: &NetworkId,
    actor: &NodeId,
    request_id: &JoinRequestId,
) -> Result<()> {
    let request = store
        .status(network_id, actor, request_id, unix_time_ms())?
        .context("join request status disappeared after a successful transaction")?;
    send_binary(
        tx,
        session_manager,
        SignalingMessage::JoinRequestStatus {
            request_id: request.request_id,
            state: request.state,
            revision: request.revision,
            information_request: request.information_request,
        },
    )
}

async fn handle_join_request_submit(
    request: JoinRequest,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&JoinRequestStore>,
) -> Result<()> {
    let Some((actor, public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let Some(store) = require_join_request_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let network_id = request.network_id.clone();
    let result = store.submit(&network_id, &actor, &public_key, request, unix_time_ms())?;
    send_join_request_status(
        store,
        session_manager,
        tx,
        &network_id,
        &actor,
        &result.request_id,
    )
    .await?;
    broadcast_join_request_changed(
        store,
        session_manager,
        &network_id,
        &actor,
        &result.request_id,
        result.state,
        result.revision,
    )
    .await
}

async fn handle_join_request_list(
    network_id: NetworkId,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&JoinRequestStore>,
) -> Result<()> {
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let Some(store) = require_join_request_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let requests = store.list(&network_id, &actor, unix_time_ms())?;
    send_binary(
        tx,
        session_manager,
        SignalingMessage::JoinRequestListResponse { requests },
    )
}

#[allow(clippy::too_many_arguments)]
async fn handle_member_directory_list(
    network_id: NetworkId,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    join_request_store: Option<&JoinRequestStore>,
    invitation_store: Option<&InvitationStore>,
    member_lifecycle_store: Option<&MemberLifecycleStore>,
    exit_availability: &ExitAvailabilityObservationRegistry,
) -> Result<()> {
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let Some(store) = require_join_request_store(join_request_store, session_manager, tx)? else {
        return Ok(());
    };
    let directory = match build_member_directory(
        &network_id,
        &actor,
        unix_time_ms(),
        store,
        invitation_store,
        member_lifecycle_store,
        session_manager,
        exit_availability,
    )
    .await
    {
        Ok(directory) => directory,
        Err(error) => {
            tracing::warn!(%error, "构建成员目录失败，拒绝返回未过滤数据");
            send_binary(
                tx,
                session_manager,
                member_directory_error_message(&error.to_string()),
            )?;
            return Ok(());
        }
    };
    send_binary(
        tx,
        session_manager,
        SignalingMessage::MemberDirectoryListAck { directory },
    )
}

fn member_directory_error_message(error: &str) -> SignalingMessage {
    let code = if error == "member_directory.error.permission_denied" {
        ErrorCode::Forbidden
    } else {
        ErrorCode::Internal
    };
    SignalingMessage::protocol_error(code, "Member directory is unavailable")
}

fn require_member_lifecycle_store<'a>(
    store: Option<&'a MemberLifecycleStore>,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
) -> Result<Option<&'a MemberLifecycleStore>> {
    if let Some(store) = store {
        return Ok(Some(store));
    }
    send_binary(
        tx,
        session_manager,
        SignalingMessage::protocol_error(
            ErrorCode::BadRequest,
            "Member lifecycle management is not configured on this Coordinator",
        ),
    )?;
    Ok(None)
}

fn member_lifecycle_error_message(error: &str) -> SignalingMessage {
    let code = match error {
        GOVERNANCE_FROZEN_ERROR
        | "member_lifecycle.error.permission_denied"
        | "member_lifecycle.error.owner_protected"
        | "member_lifecycle.error.last_administrator_protected" => ErrorCode::Forbidden,
        "member_lifecycle.error.stale_revision"
        | "member_lifecycle.error.invalid_transition"
        | "member_lifecycle.error.idempotency_conflict" => ErrorCode::BadRequest,
        _ => ErrorCode::Internal,
    };
    SignalingMessage::protocol_error(code, error)
}

async fn handle_member_removal_preview(
    request: MemberRemovalPreviewRequest,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&MemberLifecycleStore>,
) -> Result<()> {
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let Some(store) = require_member_lifecycle_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let now_unix_ms = unix_time_ms();
    let presence = session_manager
        .member_presence(now_unix_ms)
        .await
        .remove(&request.target_node_id)
        .unwrap_or_else(NetworkMemberPresence::unknown);
    let preview = match store.removal_preview(&actor, request, presence, now_unix_ms) {
        Ok(preview) => preview,
        Err(error) => {
            tracing::warn!(%error, "成员移除预览失败");
            send_binary(
                tx,
                session_manager,
                member_lifecycle_error_message(&error.to_string()),
            )?;
            return Ok(());
        }
    };
    send_binary(
        tx,
        session_manager,
        SignalingMessage::MemberRemovalPreviewAck { preview },
    )
}

async fn handle_member_lifecycle_change(
    request: MemberLifecycleChangeRequest,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&MemberLifecycleStore>,
    coordinator_candidate_store: Option<&CoordinatorCandidateStore>,
) -> Result<()> {
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let Some(store) = require_member_lifecycle_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let network_id = request.network_id.clone();
    if guard_emergency_governance_write(
        coordinator_candidate_store,
        &actor,
        &network_id,
        "member_lifecycle_change",
        session_manager,
        tx,
    )? {
        return Ok(());
    }
    let target_node_id = request.target_node_id.clone();
    let change = match store.change(&actor, request, unix_time_ms()) {
        Ok(change) => change,
        Err(error) => {
            tracing::warn!(%error, "成员生命周期变更失败");
            send_binary(
                tx,
                session_manager,
                member_lifecycle_error_message(&error.to_string()),
            )?;
            return Ok(());
        }
    };
    send_binary(
        tx,
        session_manager,
        SignalingMessage::MemberLifecycleChangeAck {
            change: change.clone(),
        },
    )?;
    let access_revoked = change.access_revoked;
    broadcast_member_lifecycle_changed(
        store,
        session_manager,
        &network_id,
        &target_node_id,
        change,
    )
    .await?;
    if access_revoked {
        session_manager
            .disconnect_node_sessions(&target_node_id)
            .await;
    }
    Ok(())
}

async fn broadcast_member_lifecycle_changed(
    store: &MemberLifecycleStore,
    session_manager: &SessionManager,
    network_id: &NetworkId,
    target_node_id: &NodeId,
    change: MemberLifecycleChange,
) -> Result<()> {
    let recipients = store.notification_recipients(network_id, target_node_id)?;
    for peer in session_manager.get_all_peers().await {
        let node_id = NodeId::from_public_key(&peer.public_key);
        if recipients.contains(&node_id) {
            session_manager
                .send_to_client(
                    &peer.client_id,
                    SignalingMessage::MemberLifecycleChanged {
                        change: change.clone(),
                    },
                )
                .await?;
        }
    }
    Ok(())
}

fn trusted_exit_network_record(
    join_request_store: Option<&JoinRequestStore>,
    exit_contribution_store: Option<&ExitContributionRequestStore>,
    network_id: &NetworkId,
) -> Result<NetworkRecord> {
    if let Some(store) = exit_contribution_store {
        return store.network_record(network_id);
    }
    if let Some(store) = join_request_store {
        return store.member_directory_network(network_id);
    }
    anyhow::bail!("Exit availability requires a trusted network record")
}

fn exit_availability_error_message(error: &str) -> SignalingMessage {
    let code = if error.contains("exit_availability.error.member_required") {
        ErrorCode::Forbidden
    } else if error.contains("exit_availability.error.invalid_reason_code") {
        ErrorCode::BadRequest
    } else {
        ErrorCode::Internal
    };
    let message = if code == ErrorCode::Internal {
        "Exit availability report could not be processed"
    } else {
        error
    };
    SignalingMessage::protocol_error(code, message)
}

#[allow(clippy::too_many_arguments)]
async fn handle_exit_availability_report(
    network_id: NetworkId,
    available: bool,
    reason_code: Option<String>,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    join_request_store: Option<&JoinRequestStore>,
    member_lifecycle_store: Option<&MemberLifecycleStore>,
    exit_contribution_store: Option<&ExitContributionRequestStore>,
    exit_availability: &ExitAvailabilityObservationRegistry,
) -> Result<()> {
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let record =
        match trusted_exit_network_record(join_request_store, exit_contribution_store, &network_id)
        {
            Ok(record) => record,
            Err(error) => {
                tracing::warn!(%error, "读取 Exit 可用性权威成员名单失败");
                send_binary(
                    tx,
                    session_manager,
                    exit_availability_error_message("exit_availability.error.member_required"),
                )?;
                return Ok(());
            }
        };
    if !record.members.contains(&actor) {
        send_binary(
            tx,
            session_manager,
            exit_availability_error_message("exit_availability.error.member_required"),
        )?;
        return Ok(());
    }
    if let Some(store) = member_lifecycle_store {
        let lifecycle = match store.record(&network_id, &actor) {
            Ok(lifecycle) => lifecycle,
            Err(error) => {
                tracing::warn!(%error, "读取 Exit 可用性成员生命周期失败");
                send_binary(
                    tx,
                    session_manager,
                    exit_availability_error_message("exit_availability.error.member_required"),
                )?;
                return Ok(());
            }
        };
        if lifecycle.is_some_and(|record| !record.status.grants_network_access()) {
            send_binary(
                tx,
                session_manager,
                exit_availability_error_message("exit_availability.error.member_required"),
            )?;
            return Ok(());
        }
    }
    let observed_at_unix_ms = unix_time_ms();
    if let Err(error) = exit_availability.observe(
        network_id.clone(),
        actor,
        available,
        reason_code,
        observed_at_unix_ms,
    ) {
        send_binary(
            tx,
            session_manager,
            exit_availability_error_message(&error.to_string()),
        )?;
        return Ok(());
    }
    send_binary(
        tx,
        session_manager,
        SignalingMessage::ExitAvailabilityReportAck {
            network_id,
            observed_at_unix_ms,
        },
    )
}

fn require_exit_contribution_store<'a>(
    store: Option<&'a ExitContributionRequestStore>,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
) -> Result<Option<&'a ExitContributionRequestStore>> {
    if let Some(store) = store {
        return Ok(Some(store));
    }
    send_binary(
        tx,
        session_manager,
        SignalingMessage::protocol_error(
            ErrorCode::BadRequest,
            "Exit contribution governance is not configured on this Coordinator",
        ),
    )?;
    Ok(None)
}

fn exit_contribution_error_message(error: &str) -> SignalingMessage {
    let code = if error.contains(GOVERNANCE_FROZEN_ERROR)
        || error.contains("exit_contribution.error.member_required")
        || error.contains("exit_contribution.error.permission_denied")
        || error.contains("exit_contribution.error.reviewer_required")
        || error.contains("exit_contribution.error.self_review_forbidden")
        || error.contains("domain.exit_contribution_request_owner_mismatch")
    {
        ErrorCode::Forbidden
    } else if error.contains("exit_contribution.error.request_not_found") {
        ErrorCode::NotFound
    } else if error.contains("exit_contribution.error.stale_policy")
        || error.contains("exit_contribution.error.mode_not_allowed")
        || error.contains("exit_contribution.error.request_id_collision")
        || error.contains("exit_contribution.error.active_request_exists")
        || error.contains("domain.exit_contribution_request_revision_conflict")
        || error.contains("domain.invalid_transition")
        || error.contains("domain.exit_contribution_request_revision")
    {
        ErrorCode::BadRequest
    } else {
        ErrorCode::Internal
    };
    let message = if code == ErrorCode::Internal {
        "Exit contribution request could not be processed"
    } else {
        error
    };
    SignalingMessage::protocol_error(code, message)
}

fn send_exit_contribution_error(
    tx: &mpsc::UnboundedSender<Message>,
    session_manager: &SessionManager,
    error: &anyhow::Error,
) -> Result<()> {
    tracing::warn!(%error, "Exit contribution request failed");
    send_binary(
        tx,
        session_manager,
        exit_contribution_error_message(&error.to_string()),
    )
}

async fn broadcast_exit_contribution_changed(
    store: &ExitContributionRequestStore,
    session_manager: &SessionManager,
    network_id: &NetworkId,
    request: &ExitContributionRequest,
) -> Result<()> {
    let record = store.network_record(network_id)?;
    let message = SignalingMessage::ExitContributionChanged {
        request_id: request.request_id.clone(),
        node_id: request.node_id.clone(),
        state: request.state,
        revision: request.revision,
    };
    for peer in session_manager.get_all_peers().await {
        let node_id = NodeId::from_public_key(&peer.public_key);
        if record.members.contains(&node_id) {
            session_manager
                .send_to_client(&peer.client_id, message.clone())
                .await?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_exit_contribution_submit(
    network_id: NetworkId,
    request_id: ExitContributionRequestId,
    expected_policy_revision: u64,
    expected_terms_hash: fuckipv6_common::domain::ContentHash,
    requested_bandwidth_kbps: Option<u64>,
    requested_traffic_quota_bytes: Option<u64>,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&ExitContributionRequestStore>,
) -> Result<()> {
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let Some(store) = require_exit_contribution_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let request = match store.submit(
        &network_id,
        &actor,
        request_id,
        expected_policy_revision,
        expected_terms_hash,
        requested_bandwidth_kbps,
        requested_traffic_quota_bytes,
        unix_time_ms(),
    ) {
        Ok(request) => request,
        Err(error) => {
            send_exit_contribution_error(tx, session_manager, &error)?;
            return Ok(());
        }
    };
    if let Err(error) =
        broadcast_exit_contribution_changed(store, session_manager, &network_id, &request).await
    {
        send_exit_contribution_error(tx, session_manager, &error)?;
        return Ok(());
    }
    send_binary(
        tx,
        session_manager,
        SignalingMessage::ExitContributionSubmitAck { request },
    )
}

async fn handle_exit_contribution_list(
    network_id: NetworkId,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&ExitContributionRequestStore>,
) -> Result<()> {
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let Some(store) = require_exit_contribution_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let requests = match store.list(&network_id, &actor, unix_time_ms()) {
        Ok(requests) => requests,
        Err(error) => {
            send_exit_contribution_error(tx, session_manager, &error)?;
            return Ok(());
        }
    };
    send_binary(
        tx,
        session_manager,
        SignalingMessage::ExitContributionListAck { requests },
    )
}

async fn handle_exit_contribution_status(
    network_id: NetworkId,
    request_id: ExitContributionRequestId,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&ExitContributionRequestStore>,
) -> Result<()> {
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let Some(store) = require_exit_contribution_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let request = match store.status(&network_id, &actor, &request_id, unix_time_ms()) {
        Ok(request) => request,
        Err(error) => {
            send_exit_contribution_error(tx, session_manager, &error)?;
            return Ok(());
        }
    };
    send_binary(
        tx,
        session_manager,
        SignalingMessage::ExitContributionStatusAck { request },
    )
}

#[allow(clippy::too_many_arguments)]
async fn handle_exit_contribution_review(
    network_id: NetworkId,
    request_id: ExitContributionRequestId,
    expected_revision: u64,
    action: ExitContributionReviewAction,
    decision_reason_code: Option<String>,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&ExitContributionRequestStore>,
    coordinator_candidate_store: Option<&CoordinatorCandidateStore>,
) -> Result<()> {
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let Some(store) = require_exit_contribution_store(store, session_manager, tx)? else {
        return Ok(());
    };
    if guard_emergency_governance_write(
        coordinator_candidate_store,
        &actor,
        &network_id,
        "exit_contribution_review",
        session_manager,
        tx,
    )? {
        return Ok(());
    }
    let request = match store.review(
        &network_id,
        &actor,
        &request_id,
        expected_revision,
        action,
        decision_reason_code,
        unix_time_ms(),
    ) {
        Ok(request) => request,
        Err(error) => {
            send_exit_contribution_error(tx, session_manager, &error)?;
            return Ok(());
        }
    };
    if let Err(error) =
        broadcast_exit_contribution_changed(store, session_manager, &network_id, &request).await
    {
        send_exit_contribution_error(tx, session_manager, &error)?;
        return Ok(());
    }
    send_binary(
        tx,
        session_manager,
        SignalingMessage::ExitContributionReviewAck { request },
    )
}

async fn handle_exit_contribution_withdraw(
    network_id: NetworkId,
    request_id: ExitContributionRequestId,
    expected_revision: u64,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&ExitContributionRequestStore>,
) -> Result<()> {
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let Some(store) = require_exit_contribution_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let request = match store.withdraw(
        &network_id,
        &actor,
        &request_id,
        expected_revision,
        unix_time_ms(),
    ) {
        Ok(request) => request,
        Err(error) => {
            send_exit_contribution_error(tx, session_manager, &error)?;
            return Ok(());
        }
    };
    if let Err(error) =
        broadcast_exit_contribution_changed(store, session_manager, &network_id, &request).await
    {
        send_exit_contribution_error(tx, session_manager, &error)?;
        return Ok(());
    }
    send_binary(
        tx,
        session_manager,
        SignalingMessage::ExitContributionWithdrawAck { request },
    )
}

async fn handle_exit_contribution_authorization_list(
    network_id: NetworkId,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&ExitContributionRequestStore>,
) -> Result<()> {
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let Some(store) = require_exit_contribution_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let authorizations = match store.authorization_snapshots(&network_id, &actor, unix_time_ms()) {
        Ok(authorizations) => authorizations,
        Err(error) => {
            send_exit_contribution_error(tx, session_manager, &error)?;
            return Ok(());
        }
    };
    send_binary(
        tx,
        session_manager,
        SignalingMessage::ExitContributionAuthorizationListAck { authorizations },
    )
}

async fn handle_join_request_review(
    network_id: NetworkId,
    reviews: Vec<JoinRequestBatchReviewItem>,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&JoinRequestStore>,
    coordinator_candidate_store: Option<&CoordinatorCandidateStore>,
) -> Result<()> {
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let Some(store) = require_join_request_store(store, session_manager, tx)? else {
        return Ok(());
    };
    if guard_emergency_governance_write(
        coordinator_candidate_store,
        &actor,
        &network_id,
        "join_request_review",
        session_manager,
        tx,
    )? {
        return Ok(());
    }

    // Resolve applicants before the atomic decision so online notifications can
    // be delivered without another authorization-dependent read afterwards.
    let mut applicants = Vec::with_capacity(reviews.len());
    for review in &reviews {
        let request = store
            .status(&network_id, &actor, &review.request_id, unix_time_ms())?
            .with_context(|| format!("join request not found: {}", review.request_id))?;
        applicants.push((review.request_id.clone(), request.applicant.node_id));
    }

    let results = match store.review(&network_id, &actor, reviews, unix_time_ms()) {
        Ok(results) => results,
        Err(error) => {
            tracing::warn!(%error, %actor, %network_id, "审批加入请求失败");
            send_binary(
                tx,
                session_manager,
                join_request_error_message(&error.to_string()),
            )?;
            return Ok(());
        }
    };
    send_binary(
        tx,
        session_manager,
        SignalingMessage::JoinRequestReviewAck {
            network_id: network_id.clone(),
            reviews: results.clone(),
        },
    )?;
    for result in results {
        let applicant = applicants
            .iter()
            .find_map(|(request_id, applicant)| {
                (request_id == &result.request_id).then_some(applicant)
            })
            .context("review result has no notification target")?;
        broadcast_join_request_changed(
            store,
            session_manager,
            &network_id,
            applicant,
            &result.request_id,
            result.state,
            result.revision,
        )
        .await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_join_request_resubmit(
    network_id: NetworkId,
    request_id: JoinRequestId,
    expected_revision: u64,
    reason: String,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&JoinRequestStore>,
) -> Result<()> {
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let Some(store) = require_join_request_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let result = store.resubmit(
        &network_id,
        &actor,
        &request_id,
        expected_revision,
        reason,
        unix_time_ms(),
    )?;
    send_join_request_status(store, session_manager, tx, &network_id, &actor, &request_id).await?;
    broadcast_join_request_changed(
        store,
        session_manager,
        &network_id,
        &actor,
        &result.request_id,
        result.state,
        result.revision,
    )
    .await
}

async fn handle_join_request_withdraw(
    network_id: NetworkId,
    request_id: JoinRequestId,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&JoinRequestStore>,
) -> Result<()> {
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let Some(store) = require_join_request_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let result = store.withdraw(&network_id, &actor, &request_id, unix_time_ms())?;
    send_join_request_status(store, session_manager, tx, &network_id, &actor, &request_id).await?;
    broadcast_join_request_changed(
        store,
        session_manager,
        &network_id,
        &actor,
        &result.request_id,
        result.state,
        result.revision,
    )
    .await
}

fn require_invitation_store<'a>(
    store: Option<&'a InvitationStore>,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
) -> Result<Option<&'a InvitationStore>> {
    if let Some(store) = store {
        return Ok(Some(store));
    }
    send_binary(
        tx,
        session_manager,
        SignalingMessage::protocol_error(
            ErrorCode::BadRequest,
            "Private invitation joining is not configured on this Coordinator",
        ),
    )?;
    Ok(None)
}

fn invitation_error_message(error: &str) -> SignalingMessage {
    let code = if error.contains(GOVERNANCE_FROZEN_ERROR)
        || error.contains("invitation.error.permission_denied")
        || error.contains("invitation.error.creator_authority_revoked")
        || error.contains("invitation.error.already_member")
    {
        ErrorCode::Forbidden
    } else if error.contains("invitation.error.") {
        ErrorCode::BadRequest
    } else if error.contains("invitation not found") {
        ErrorCode::NotFound
    } else {
        ErrorCode::Internal
    };
    let message = if code == ErrorCode::Internal {
        "Invitation operation could not be processed"
    } else {
        error
    };
    SignalingMessage::protocol_error(code, message)
}

async fn broadcast_invitation_changed(
    store: &InvitationStore,
    session_manager: &SessionManager,
    network_id: &NetworkId,
    creator: &NodeId,
    invitation_id: &InvitationId,
    state: InvitationState,
    revision: u64,
) -> Result<()> {
    let recipients = store.notification_recipients(network_id, creator)?;
    let message = invitation_changed_message(invitation_id, state, revision);
    for peer in session_manager.get_all_peers().await {
        let node_id = NodeId::from_public_key(&peer.public_key);
        if recipients.contains(&node_id) {
            session_manager
                .send_to_client(&peer.client_id, message.clone())
                .await?;
        }
    }
    Ok(())
}

fn invitation_changed_message(
    invitation_id: &InvitationId,
    state: InvitationState,
    revision: u64,
) -> SignalingMessage {
    SignalingMessage::InvitationChanged {
        invitation_id: invitation_id.clone(),
        state,
        revision,
    }
}

async fn broadcast_invitation_by_id(
    store: &InvitationStore,
    session_manager: &SessionManager,
    invitation_id: &InvitationId,
) -> Result<()> {
    let (network_id, creator, state, revision) =
        store.notification_metadata(invitation_id, unix_time_ms())?;
    broadcast_invitation_changed(
        store,
        session_manager,
        &network_id,
        &creator,
        invitation_id,
        state,
        revision,
    )
    .await
}

async fn handle_invitation_create(
    request: fuckipv6_common::domain::InvitationCreateRequest,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&InvitationStore>,
    coordinator_candidate_store: Option<&CoordinatorCandidateStore>,
) -> Result<()> {
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let Some(store) = require_invitation_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let network_id = request.network_id.clone();
    if guard_emergency_governance_write(
        coordinator_candidate_store,
        &actor,
        &network_id,
        "invitation_create",
        session_manager,
        tx,
    )? {
        return Ok(());
    }
    let issued = match store.create(&network_id, &actor, request, unix_time_ms()) {
        Ok(issued) => issued,
        Err(error) => {
            tracing::warn!(%error, %actor, %network_id, "创建邀请失败");
            send_binary(
                tx,
                session_manager,
                invitation_error_message(&error.to_string()),
            )?;
            return Ok(());
        }
    };
    let invitation_id = issued.status.invitation.invitation_id.clone();
    send_binary(
        tx,
        session_manager,
        SignalingMessage::InvitationCreateAck {
            issued: issued.clone(),
        },
    )?;
    broadcast_invitation_changed(
        store,
        session_manager,
        &network_id,
        &actor,
        &invitation_id,
        issued.status.state,
        issued.status.invitation.revision,
    )
    .await
}

async fn handle_invitation_list(
    network_id: NetworkId,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&InvitationStore>,
) -> Result<()> {
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let Some(store) = require_invitation_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let invitations = store.list(&network_id, &actor, unix_time_ms())?;
    send_binary(
        tx,
        session_manager,
        SignalingMessage::InvitationListAck { invitations },
    )
}

async fn handle_invitation_status(
    network_id: NetworkId,
    invitation_id: InvitationId,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&InvitationStore>,
) -> Result<()> {
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let Some(store) = require_invitation_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let invitation = store.status(&network_id, &actor, &invitation_id, unix_time_ms())?;
    send_binary(
        tx,
        session_manager,
        SignalingMessage::InvitationStatusAck { invitation },
    )
}

#[allow(clippy::too_many_arguments)]
async fn handle_invitation_revoke(
    network_id: NetworkId,
    invitation_id: InvitationId,
    expected_revision: u64,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&InvitationStore>,
    coordinator_candidate_store: Option<&CoordinatorCandidateStore>,
) -> Result<()> {
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let Some(store) = require_invitation_store(store, session_manager, tx)? else {
        return Ok(());
    };
    if guard_emergency_governance_write(
        coordinator_candidate_store,
        &actor,
        &network_id,
        "invitation_revoke",
        session_manager,
        tx,
    )? {
        return Ok(());
    }
    let invitation = match store.revoke(
        &network_id,
        &actor,
        &invitation_id,
        expected_revision,
        unix_time_ms(),
    ) {
        Ok(invitation) => invitation,
        Err(error) => {
            tracing::warn!(%error, %actor, %network_id, "撤销邀请失败");
            send_binary(
                tx,
                session_manager,
                invitation_error_message(&error.to_string()),
            )?;
            return Ok(());
        }
    };
    send_binary(
        tx,
        session_manager,
        SignalingMessage::InvitationRevokeAck {
            invitation: invitation.clone(),
        },
    )?;
    broadcast_invitation_changed(
        store,
        session_manager,
        &network_id,
        &invitation.invitation.created_by,
        &invitation_id,
        invitation.state,
        invitation.invitation.revision,
    )
    .await
}

async fn handle_invitation_preview(
    request: fuckipv6_common::domain::InvitationPreviewRequest,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&InvitationStore>,
) -> Result<()> {
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let Some(store) = require_invitation_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let preview = store.preview(&request.network_id, &actor, &request.code, unix_time_ms())?;
    send_binary(
        tx,
        session_manager,
        SignalingMessage::InvitationPreviewAck { preview },
    )
}

async fn handle_network_join_preview(
    request: fuckipv6_common::domain::NetworkJoinPreviewRequest,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    network_record_path: Option<&std::path::Path>,
    node_identity: Option<&Identity>,
) -> Result<()> {
    let Some((_actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let Some(network_record_path) = network_record_path else {
        send_binary(
            tx,
            session_manager,
            SignalingMessage::protocol_error(
                ErrorCode::Overloaded,
                "Network join preview is unavailable",
            ),
        )?;
        return Ok(());
    };
    let Some(node_identity) = node_identity else {
        send_binary(
            tx,
            session_manager,
            SignalingMessage::protocol_error(
                ErrorCode::Overloaded,
                "Network join preview is unavailable",
            ),
        )?;
        return Ok(());
    };
    let record = match load_trusted_network_record(network_record_path, None) {
        Ok(record) => record,
        Err(error) => {
            tracing::error!(%error, "读取直接加入预览的可信网络记录失败");
            send_binary(
                tx,
                session_manager,
                SignalingMessage::protocol_error(
                    ErrorCode::Overloaded,
                    "Network join preview is unavailable",
                ),
            )?;
            return Ok(());
        }
    };
    if NodeId::from_public_key(&request.coordinator_public_key) != record.owner {
        send_binary(
            tx,
            session_manager,
            SignalingMessage::protocol_error(
                ErrorCode::Forbidden,
                "Join ticket coordinator key does not belong to this network",
            ),
        )?;
        return Ok(());
    }
    if node_identity.public_key() != request.coordinator_public_key {
        send_binary(
            tx,
            session_manager,
            SignalingMessage::protocol_error(
                ErrorCode::Forbidden,
                "Join ticket coordinator key does not match this Coordinator",
            ),
        )?;
        return Ok(());
    }
    if request.network_name != record.name {
        send_binary(
            tx,
            session_manager,
            SignalingMessage::protocol_error(
                ErrorCode::BadRequest,
                "Join ticket network name does not match the trusted record",
            ),
        )?;
        return Ok(());
    }
    let now_unix_ms = unix_time_ms();
    let preview = NetworkJoinPreview::for_record(
        &record,
        node_identity,
        now_unix_ms,
        now_unix_ms.saturating_add(fuckipv6_common::domain::NETWORK_JOIN_PREVIEW_VALIDITY_MS),
    )?;
    send_binary(
        tx,
        session_manager,
        SignalingMessage::NetworkJoinPreviewAck { preview },
    )
}

async fn handle_invitation_redeem(
    request: fuckipv6_common::domain::InvitationRedeemRequest,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
    store: Option<&InvitationStore>,
    coordinator_candidate_store: Option<&CoordinatorCandidateStore>,
) -> Result<()> {
    let Some((actor, _public_key)) =
        authenticated_join_actor(session_manager, tx, client_id).await?
    else {
        return Ok(());
    };
    let Some(store) = require_invitation_store(store, session_manager, tx)? else {
        return Ok(());
    };
    let network_id = request.network_id.clone();
    if guard_emergency_governance_write(
        coordinator_candidate_store,
        &actor,
        &network_id,
        "invitation_redeem",
        session_manager,
        tx,
    )? {
        return Ok(());
    }
    let invitation_id = request.invitation_id.clone();
    let redemption = match store.redeem(request, &actor, unix_time_ms()) {
        Ok(redemption) => redemption,
        Err(error) => {
            tracing::warn!(%error, %actor, "兑换邀请失败");
            send_binary(
                tx,
                session_manager,
                invitation_error_message(&error.to_string()),
            )?;
            return Ok(());
        }
    };
    send_binary(
        tx,
        session_manager,
        SignalingMessage::InvitationRedeemAck {
            redemption: redemption.clone(),
        },
    )?;
    broadcast_invitation_by_id(store, session_manager, &invitation_id).await
}

fn unix_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[allow(clippy::too_many_arguments)]
async fn handle_register_message(
    msg: SignalingMessage,
    addr: &SocketAddr,
    session_manager: &Arc<SessionManager>,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &mut Option<String>,
    local_addr: Option<SocketAddr>,
    relay_directory: &RelayEndpointDirectory,
) -> Result<()> {
    let SignalingMessage::Register {
        client_id: id,
        public_key,
        virtual_ip: _,
        p2p_port,
        nat_type,
        candidate_endpoints,
        timestamp,
        signature,
        requested_ips,
    } = msg
    else {
        unreachable!("handle_register_message only accepts Register messages");
    };

    // Only the v2 field is signed. The legacy virtual_ip hint was never part
    // of the v1 registration challenge, so it must not be trusted as a lease.
    if !verify_registration(
        &id,
        &public_key,
        requested_ips.as_ref(),
        p2p_port,
        timestamp,
        &signature,
    ) {
        let error = SignalingMessage::protocol_error(
            ErrorCode::Unauthorized,
            "Invalid registration signature",
        );
        send_binary(tx, session_manager, error)?;
        return Ok(());
    }

    match session_manager
        .is_registration_revoked(&id, &public_key)
        .await
    {
        Ok(false) => {}
        Ok(true) => {
            tracing::warn!("拒绝已撤销授权节点注册请求: {}", id);
            let error = SignalingMessage::protocol_error(
                ErrorCode::Unauthorized,
                "Node authorization has been revoked for this network",
            );
            send_binary(tx, session_manager, error)?;
            return Ok(());
        }
        Err(error) => {
            tracing::error!("授权撤销目录查询失败，拒绝节点注册 {}: {error:#}", id);
            let error = SignalingMessage::protocol_error(
                ErrorCode::Internal,
                "Authorization revocation check failed",
            );
            send_binary(tx, session_manager, error)?;
            return Ok(());
        }
    }

    if session_manager.is_blacklisted(&id, &public_key).await {
        tracing::warn!("拒绝黑名单客户端注册请求: {}", id);
        let error = SignalingMessage::protocol_error(
            ErrorCode::Unauthorized,
            "Client ID or public key is blacklisted",
        );
        send_binary(tx, session_manager, error)?;
        return Ok(());
    }

    let virtual_ips = session_manager
        .register_client_with_candidates(
            id.clone(),
            public_key,
            *addr,
            p2p_port,
            nat_type,
            candidate_endpoints,
            requested_ips,
            tx.clone(),
        )
        .await
        .with_context(|| format!("注册客户端失败: {id}"))?;

    let peers = session_manager.get_data_plane_peers().await;
    let peers: Vec<_> = peers
        .into_iter()
        .filter(|peer| peer.client_id != id)
        .collect();

    tracing::info!("通知客户端 {} 有 {} 个在线节点", id, peers.len());

    let ipv4_prefix = session_manager.ipv4_prefix().await;
    let ipv6_prefix = session_manager.ipv6_prefix().await;
    let is_data_plane_member = session_manager
        .get_peer_info(&id)
        .await
        .map(|peer| session_manager.is_data_plane_member(&peer.public_key))
        .transpose()?
        .unwrap_or(false);

    let (relay_endpoint, relay_endpoints) = relay_directory.snapshot();
    let response = SignalingMessage::RegisterAck {
        assigned_ip: virtual_ips.primary_ipv6(),
        assigned_ips: Some(virtual_ips),
        network_prefix: ipv6_prefix,
        ipv4_prefix: Some(ipv4_prefix),
        peers,
        relay_endpoint: is_data_plane_member
            .then(|| relay_endpoint.map(|endpoint| endpoint.for_signaling_connection(local_addr)))
            .flatten(),
        relay_endpoints: if is_data_plane_member {
            relay_endpoints
                .iter()
                .cloned()
                .map(|endpoint| endpoint.for_signaling_connection(local_addr))
                .collect()
        } else {
            Vec::new()
        },
    };

    send_binary(tx, session_manager, response)?;
    *client_id = Some(id.clone());

    tracing::info!("客户端 {} 完成注册流程", id);
    Ok(())
}

async fn require_data_plane_session(
    client_id: &Option<String>,
    session_manager: &SessionManager,
    tx: &mpsc::UnboundedSender<Message>,
) -> Result<Option<fuckipv6_common::PeerInfo>> {
    let Some(client_id) = client_id.as_deref() else {
        send_binary(
            tx,
            session_manager,
            SignalingMessage::protocol_error(
                ErrorCode::Unauthorized,
                "Client must register before using the data plane",
            ),
        )?;
        return Ok(None);
    };
    let Some(peer) = session_manager.get_peer_info(&client_id.to_string()).await else {
        send_binary(
            tx,
            session_manager,
            SignalingMessage::protocol_error(
                ErrorCode::Unauthorized,
                "Client registration is no longer active",
            ),
        )?;
        return Ok(None);
    };
    if !session_manager.is_data_plane_member(&peer.public_key)? {
        send_binary(
            tx,
            session_manager,
            SignalingMessage::protocol_error(
                ErrorCode::Forbidden,
                "Node is not a current data-plane member",
            ),
        )?;
        return Ok(None);
    }
    Ok(Some(peer))
}

async fn handle_connect_request(
    target_id: String,
    session_manager: &Arc<SessionManager>,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
) -> Result<()> {
    let Some(requester_peer) = require_data_plane_session(client_id, session_manager, tx).await?
    else {
        return Ok(());
    };
    let Some(peer) = session_manager.get_peer_info(&target_id).await else {
        let error = SignalingMessage::protocol_error(ErrorCode::NotFound, "Peer not found");
        send_binary(tx, session_manager, error)?;
        return Ok(());
    };
    if !session_manager.is_data_plane_member(&peer.public_key)? {
        let error = SignalingMessage::protocol_error(ErrorCode::NotFound, "Peer not found");
        send_binary(tx, session_manager, error)?;
        return Ok(());
    }

    send_binary(
        tx,
        session_manager,
        SignalingMessage::ConnectInfo { peer: peer.clone() },
    )?;

    session_manager
        .send_to_client(
            &target_id,
            SignalingMessage::ConnectNotify {
                peer: requester_peer,
            },
        )
        .await
        .with_context(|| format!("通知目标节点连接请求失败: {target_id}"))?;

    Ok(())
}

async fn handle_heartbeat(
    timestamp: u64,
    session_manager: &Arc<SessionManager>,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
) -> Result<()> {
    if let Some(id) = client_id.as_ref() {
        session_manager.record_heartbeat(id).await;
    }

    send_binary(
        tx,
        session_manager,
        SignalingMessage::HeartbeatAck { timestamp },
    )
}

async fn handle_secure_control_packet(
    src_ip: std::net::IpAddr,
    dst_ip: std::net::IpAddr,
    payload: Bytes,
    session_manager: &Arc<SessionManager>,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
) -> Result<()> {
    let Some(client_id) = client_id.as_deref() else {
        send_binary(
            tx,
            session_manager,
            SignalingMessage::protocol_error(
                ErrorCode::Unauthorized,
                "Client must register before relaying a secure control frame",
            ),
        )?;
        return Ok(());
    };

    let Some(session) = session_manager.get_peer_info(&client_id.to_string()).await else {
        send_binary(
            tx,
            session_manager,
            SignalingMessage::protocol_error(
                ErrorCode::Unauthorized,
                "Secure control source has no active registration",
            ),
        )?;
        return Ok(());
    };
    if !session.matches_ip(src_ip) {
        send_binary(
            tx,
            session_manager,
            SignalingMessage::protocol_error(
                ErrorCode::Unauthorized,
                "Secure control source IP is not assigned to this client",
            ),
        )?;
        return Ok(());
    }
    if !session_manager.is_data_plane_member(&session.public_key)? {
        send_binary(
            tx,
            session_manager,
            SignalingMessage::protocol_error(
                ErrorCode::Forbidden,
                "Secure control source is not a current data-plane member",
            ),
        )?;
        return Ok(());
    }

    let packet = SignalingMessage::secure_control_packet(src_ip, dst_ip, payload);
    session_manager.send_to_virtual_ip(dst_ip, packet).await?;
    Ok(())
}

async fn handle_relay_packet(
    src_ip: std::net::IpAddr,
    dst_ip: std::net::IpAddr,
    payload: Bytes,
    session_manager: &Arc<SessionManager>,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
) -> Result<()> {
    let Some(client_id) = client_id.as_deref() else {
        session_manager.record_relay_error();
        let error = SignalingMessage::protocol_error(
            ErrorCode::Unauthorized,
            "Client must register before relay",
        );
        send_binary(tx, session_manager, error)?;
        return Ok(());
    };

    let Some(session) = session_manager.get_peer_info(&client_id.to_string()).await else {
        session_manager.record_relay_error();
        let error = SignalingMessage::protocol_error(
            ErrorCode::Unauthorized,
            "Relay source has no active registration",
        );
        send_binary(tx, session_manager, error)?;
        return Ok(());
    };
    if !session.matches_ip(src_ip) {
        tracing::warn!(
            "拒绝客户端 {} 伪造中继源地址 {} (已分配: {:?})",
            client_id,
            src_ip,
            session.virtual_ips
        );
        session_manager.record_relay_error();
        let error = SignalingMessage::protocol_error(
            ErrorCode::Unauthorized,
            "Relay source IP is not assigned to this client",
        );
        send_binary(tx, session_manager, error)?;
        return Ok(());
    }
    if !session_manager.is_data_plane_member(&session.public_key)? {
        session_manager.record_relay_error();
        let error = SignalingMessage::protocol_error(
            ErrorCode::Forbidden,
            "Relay source is not a current data-plane member",
        );
        send_binary(tx, session_manager, error)?;
        return Ok(());
    }

    validate_signaling_relay_payload_len(payload.len())?;
    let payload_len = payload.len();
    let packet = SignalingMessage::RelayPacket {
        src_ip,
        dst_ip,
        payload,
    };

    match session_manager.send_to_virtual_ip(dst_ip, packet).await {
        Ok(()) => session_manager.record_relay_forward(payload_len),
        Err(e) => {
            session_manager.record_relay_error();
            return Err(e).with_context(|| format!("信令中继转发失败: {dst_ip}"));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_recovery_service_requires_all_three_durable_stores() -> Result<()> {
        let dir = std::env::temp_dir().join(format!(
            "fuckipv6-admin-recovery-wiring-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let network_record_path = dir.join("network.json");
        let mut server = SignalingServer::new("127.0.0.1:0".into(), "fd00::/120")?;

        assert!(server.admin_recovery_store().is_none());
        assert!(server.admin_recovery_service().is_none());

        server = server.with_admin_recovery_store(Arc::new(AdminRecoveryStore::new(
            dir.join("admin_recovery.json"),
            &network_record_path,
        )?));
        assert!(server.admin_recovery_store().is_some());
        assert!(server.admin_recovery_service().is_none());

        server = server.with_join_request_store(Arc::new(JoinRequestStore::new(
            dir.join("join_requests.json"),
            Some(network_record_path.clone()),
            None,
            Vec::new(),
        )?));
        assert!(server.admin_recovery_service().is_none());

        server = server.with_invitation_store(Arc::new(InvitationStore::new(
            dir.join("invitations.json"),
            Some(network_record_path),
            None,
            Vec::new(),
        )?));
        assert!(server.admin_recovery_service().is_some());

        drop(server);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn admin_recovery_store_from_config_uses_stable_state_path() -> Result<()> {
        let dir = std::env::temp_dir().join(format!(
            "fuckipv6-admin-recovery-config-path-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let config = CoordinatorConfig {
            network_record_path: Some(dir.join("network.json")),
            ..CoordinatorConfig::default()
        };

        let server = SignalingServer::new("127.0.0.1:0".into(), "fd00::/120")?
            .with_admin_recovery_store_from_config(&dir, &config)?;
        let store = server
            .admin_recovery_store()
            .context("configured recovery store is missing")?;
        assert_eq!(store.path(), dir.join("admin_recovery.json"));
        assert!(server.admin_recovery_service().is_none());

        drop(store);
        drop(server);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn coordinator_recovery_response_is_rejected_as_a_client_message() {
        let network_id = NetworkId::from_public_key(b"recovery-response-spoof");
        let leader = NodeId::from_public_key(b"recovery-response-leader");
        let status = fuckipv6_common::domain::CoordinatorCandidateStatus {
            network_id: network_id.clone(),
            generated_at_unix_ms: 1,
            config_state: fuckipv6_common::domain::CoordinatorCandidateConfigState::Missing,
            config: None,
            candidates: Vec::new(),
            current_leader: None,
            latest_term: None,
            lease: fuckipv6_common::domain::CoordinatorLeaseStatus::no_term(),
            control_plane: Default::default(),
            emergency: fuckipv6_common::domain::CoordinatorEmergencyState::NoCandidatesConfigured,
            emergency_leader: None,
            latest_election: None,
        };
        let message = SignalingMessage::CoordinatorRecoveryResponse {
            response: fuckipv6_common::domain::CoordinatorRecoveryResponse {
                status: status.clone(),
                recovery: fuckipv6_common::domain::CoordinatorRecoveryStatus {
                    network_id,
                    stage: fuckipv6_common::domain::CoordinatorRecoveryStage::ReadOnly,
                    role: fuckipv6_common::domain::CoordinatorRecoveryRole::Member,
                    governance_writable: false,
                    health_checks_allowed: true,
                    data_plane_allowed: true,
                    authoritative_term: 0,
                    authoritative_fencing_token: fuckipv6_common::domain::FencingToken(0),
                    authoritative_leader: leader,
                    authoritative_candidate_config_revision: 0,
                    authoritative_audit_sequence: 0,
                    reason_code: "coordinator.recovery.snapshot_behind".into(),
                },
                audit_entries: Vec::new(),
            },
        };

        assert!(is_server_originated_response(&message));
        assert!(is_server_originated_response(
            &SignalingMessage::CoordinatorEmergencyLeaderIssueAck {
                response: fuckipv6_common::domain::CoordinatorEmergencyLeaderResponse {
                    status: status.clone(),
                    renewed: false,
                },
            }
        ));
        assert!(is_server_originated_response(
            &SignalingMessage::CoordinatorEmergencyLeaderRenewAck {
                response: fuckipv6_common::domain::CoordinatorEmergencyLeaderResponse {
                    status,
                    renewed: true,
                },
            }
        ));
    }

    #[test]
    fn administrator_recovery_acknowledgements_are_rejected_as_client_messages() {
        let network_id = NetworkId::from_public_key(b"admin-recovery-ack-network");
        let node = NodeId::from_public_key(b"admin-recovery-ack-node");
        let status = fuckipv6_common::domain::AdminRecoveryStatusView {
            request_id: fuckipv6_common::domain::AdminRecoveryRequestId::from_public_key(
                b"admin-recovery-ack-request",
            ),
            network_id: network_id.clone(),
            state: fuckipv6_common::domain::AdminRecoveryState::Pending,
            impact: fuckipv6_common::domain::AdminRecoveryImpact {
                owner_before: node.clone(),
                owner_after: node.clone(),
                administrators_added: Vec::new(),
                administrators_removed: Vec::new(),
                members_added: Vec::new(),
                members_removed: Vec::new(),
            },
            initiator: node.clone(),
            participants: Vec::new(),
            signature_count: 0,
            required_signatures: 1,
            issued_at_unix_ms: 1,
            expires_at_unix_ms: 2,
            revocation: fuckipv6_common::domain::AdminRecoveryRevocationPlan::default(),
            can_execute: false,
            denial_key: Some("admin_recovery.error.threshold_not_met".into()),
        };
        let messages = [
            SignalingMessage::AdminRecoverySubmitAck {
                status: status.clone(),
            },
            SignalingMessage::AdminRecoveryStatusAck {
                status: status.clone(),
            },
            SignalingMessage::AdminRecoveryListAck {
                requests: vec![status.clone()],
            },
            SignalingMessage::AdminRecoveryRevokeAck {
                status: status.clone(),
            },
            SignalingMessage::AdminRecoveryExecuteAck { status },
        ];
        for message in messages {
            assert!(is_server_originated_response(&message));
        }
    }

    #[test]
    fn emergency_governance_guard_rejects_an_invitee_with_structured_forbidden() -> Result<()> {
        use fuckipv6_common::domain::{
            CapabilityKind, CoordinatorCandidate, CoordinatorCandidateConfig, ExitContributionMode,
            ExitContributionObligation, ExitContributionScope, ExitSelectionPolicy,
            NetworkCreateProfile, NetworkJoinMode, NetworkVisibility, PublicKey,
        };

        let dir = std::env::temp_dir().join(format!(
            "fuckipv6-emergency-governance-guard-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let owner = Identity::generate()?;
        let candidate = Identity::generate()?;
        let ordinary = Identity::generate()?;
        let invitee = Identity::generate()?;
        let owner_id = NodeId::from_public_key(&owner.public_key());
        let candidate_id = NodeId::from_public_key(&candidate.public_key());
        let ordinary_id = NodeId::from_public_key(&ordinary.public_key());
        let invitee_id = NodeId::from_public_key(&invitee.public_key());
        let now = unix_time_ms();
        let mut network = NetworkRecord::create_owned(
            NetworkCreateProfile {
                name: "emergency-governance-guard".into(),
                purpose: "emergency governance guard test".into(),
                icon: "network".into(),
                language: "zh-CN".into(),
                region: "ZZ".into(),
                tags: vec![],
                capability_declarations: vec![CapabilityKind::Coordinator],
                visibility: NetworkVisibility::Private,
                join_mode: NetworkJoinMode::InviteOnly,
                exit_contribution: ExitContributionObligation::NotRequired,
                exit_contribution_scope: ExitContributionScope::NotApplicable,
                exit_contribution_mode: ExitContributionMode::Disabled,
                exit_selection_policy: ExitSelectionPolicy::default(),
            },
            PublicKey::from_bytes([91; 32]),
            owner_id.clone(),
            now.saturating_sub(1_000),
        )?;
        network
            .members
            .extend([candidate_id.clone(), ordinary_id.clone()]);
        network.validate()?;
        let network_path = dir.join("network.json");
        std::fs::write(&network_path, serde_json::to_vec_pretty(&network)?)?;
        let store =
            CoordinatorCandidateStore::new(dir.join("coordinator_candidates.json"), network_path)?;
        let config = CoordinatorCandidateConfig::signed(
            network.network_id.clone(),
            1,
            vec![CoordinatorCandidate {
                node_id: candidate_id,
                priority: 10,
            }],
            now.saturating_sub(1),
            now.saturating_add(3_600_000),
            &owner,
        )?;
        store.update_config(&owner_id, 0, config, now)?;
        store.issue_emergency_leader(
            &owner_id,
            &network.network_id,
            &[owner_id.clone(), ordinary_id],
            now,
        )?;

        let manager = SessionManager::new("fd00::1000/120")?;
        let (tx, mut rx) = mpsc::unbounded_channel();
        assert!(guard_emergency_governance_write(
            Some(&store),
            &invitee_id,
            &network.network_id,
            "invitation_redeem",
            &manager,
            &tx,
        )?);
        let Message::Binary(encoded) = rx.try_recv()? else {
            panic!("expected a binary protocol error");
        };
        let SignalingMessage::Error { code, message } =
            codec::decode::<SignalingMessage>(&encoded)?
        else {
            panic!("expected a protocol error");
        };
        assert_eq!(code, ErrorCode::Forbidden.as_u16());
        assert_eq!(
            message,
            "Governance is frozen during emergency Coordinator runtime authority"
        );
        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn join_request_changed_notification_contains_only_identity_and_state() {
        let request_id = JoinRequestId::from_public_key(b"notification-contract-request");
        let message =
            join_request_changed_message(&request_id, JoinRequestState::NeedsInformation, 7);
        let encoded = serde_json::to_string(&message).expect("serialize notification");
        let decoded: SignalingMessage =
            serde_json::from_str(&encoded).expect("deserialize notification");

        let SignalingMessage::JoinRequestChanged {
            request_id: decoded_request_id,
            state,
            revision,
        } = decoded
        else {
            panic!("expected a join request changed notification");
        };
        assert_eq!(decoded_request_id, request_id);
        assert_eq!(state, JoinRequestState::NeedsInformation);
        assert_eq!(revision, 7);
        assert!(!encoded.contains("reason"));
        assert!(!encoded.contains("SECRET-NOTIFICATION-PROSE"));
    }

    #[test]
    fn invitation_changed_notification_contains_only_identity_and_state() {
        let invitation_id = InvitationId::from_public_key(b"notification-invitation");
        let message = invitation_changed_message(&invitation_id, InvitationState::Active, 9);
        let encoded = serde_json::to_string(&message).expect("serialize notification");
        let decoded: SignalingMessage =
            serde_json::from_str(&encoded).expect("deserialize notification");

        let SignalingMessage::InvitationChanged {
            invitation_id: decoded_invitation_id,
            state,
            revision,
        } = decoded
        else {
            panic!("expected an invitation changed notification");
        };
        assert_eq!(decoded_invitation_id, invitation_id);
        assert_eq!(state, InvitationState::Active);
        assert_eq!(revision, 9);
        assert!(!encoded.contains("code"));
        assert!(!encoded.contains("fkinv_"));
        assert!(!encoded.contains("SECRET-INVITATION-NOTIFICATION"));
    }

    #[test]
    fn member_directory_permission_errors_map_to_forbidden_without_leaking_details() {
        let permission = member_directory_error_message("member_directory.error.permission_denied");
        let SignalingMessage::Error { code, message } = permission else {
            panic!("expected a protocol error");
        };
        assert_eq!(code, ErrorCode::Forbidden.as_u16());
        assert_eq!(message, "Member directory is unavailable");
        assert!(!message.contains("permission"));

        let internal = member_directory_error_message("filesystem path leaked");
        let SignalingMessage::Error { code, message } = internal else {
            panic!("expected a protocol error");
        };
        assert_eq!(code, ErrorCode::Internal.as_u16());
        assert!(!message.contains("filesystem"));
    }

    #[test]
    fn governance_freeze_maps_to_forbidden_on_every_write_surface() {
        let messages = [
            member_lifecycle_error_message(GOVERNANCE_FROZEN_ERROR),
            exit_contribution_error_message(GOVERNANCE_FROZEN_ERROR),
            invitation_error_message(GOVERNANCE_FROZEN_ERROR),
            join_request_error_message(GOVERNANCE_FROZEN_ERROR),
        ];
        for response in messages {
            let SignalingMessage::Error { code, message } = response else {
                panic!("expected a protocol error");
            };
            assert_eq!(code, ErrorCode::Forbidden.as_u16());
            assert_eq!(message, GOVERNANCE_FROZEN_ERROR);
        }
    }

    #[test]
    fn relay_endpoint_directory_reflects_live_enable_and_disable() {
        let directory = RelayEndpointDirectory::default();
        let first = RelayEndpoint {
            addr: "127.0.0.1:41000".parse().expect("first relay address"),
            server_name: "fuckipv6.local".into(),
            public_key_pin: [1; 32],
            provider_node_id: Some(NodeId::from_public_key(b"first-relay-provider")),
        };
        let second = RelayEndpoint {
            addr: "127.0.0.1:41001".parse().expect("second relay address"),
            server_name: "fuckipv6.local".into(),
            public_key_pin: [2; 32],
            provider_node_id: Some(NodeId::from_public_key(b"second-relay-provider")),
        };

        directory.set_primary(Some(first.clone()));
        assert_eq!(directory.snapshot(), (Some(first.clone()), Vec::new()));

        directory.set_additional(vec![second.clone()]);
        assert_eq!(
            directory.snapshot(),
            (Some(first.clone()), vec![second.clone()])
        );

        let secondary = RelayEndpoint {
            addr: "127.0.0.1:41002".parse().expect("secondary relay address"),
            server_name: "fuckipv6.local".into(),
            public_key_pin: [3; 32],
            provider_node_id: Some(NodeId::from_public_key(b"secondary-relay-provider")),
        };
        directory.set_additional(vec![second.clone(), secondary]);
        let (primary, additional) = directory.snapshot();
        assert_eq!(primary, Some(first));
        assert_eq!(additional.len(), 2);
        assert_eq!(additional[0], second);

        directory.set_primary(None);
        directory.set_additional(Vec::new());
        assert_eq!(directory.snapshot(), (None, Vec::new()));
    }

    #[test]
    fn test_verify_registration_signature() -> Result<()> {
        let identity = Identity::generate()?;
        let client_id = identity.client_id().to_string();
        let public_key = identity.public_key();
        let p2p_port = 51820;
        let timestamp = 123;
        let challenge =
            SignalingMessage::register_challenge(&client_id, &public_key, p2p_port, timestamp);
        let signature = identity.sign(&challenge).to_bytes().to_vec();

        assert!(verify_registration(
            &client_id,
            &public_key,
            None,
            p2p_port,
            timestamp,
            &signature
        ));

        assert!(!verify_registration(
            &client_id,
            &public_key,
            None,
            p2p_port + 1,
            timestamp,
            &signature
        ));

        assert!(!verify_registration(
            "wrong-client-id",
            &public_key,
            None,
            p2p_port,
            timestamp,
            &signature
        ));

        let requested_ips =
            fuckipv6_common::VirtualIps::dual_stack("10.8.0.42".parse()?, "fd00::42".parse()?);
        let lease_challenge = SignalingMessage::register_challenge_with_ips(
            &client_id,
            &public_key,
            Some(&requested_ips),
            p2p_port,
            timestamp,
        );
        let lease_signature = identity.sign(&lease_challenge).to_bytes().to_vec();
        assert!(verify_registration(
            &client_id,
            &public_key,
            Some(&requested_ips),
            p2p_port,
            timestamp,
            &lease_signature
        ));

        let tampered =
            fuckipv6_common::VirtualIps::dual_stack("10.8.0.43".parse()?, "fd00::42".parse()?);
        assert!(!verify_registration(
            &client_id,
            &public_key,
            Some(&tampered),
            p2p_port,
            timestamp,
            &lease_signature
        ));
        Ok(())
    }

    #[test]
    fn admin_recovery_domain_rejections_keep_stable_public_keys() {
        let cases = [
            (
                "domain.admin_recovery_threshold_not_met",
                "admin_recovery.error.threshold_not_met",
            ),
            (
                "domain.admin_recovery_expired",
                "admin_recovery.error.expired",
            ),
            (
                "domain.admin_recovery_stale_snapshot",
                "admin_recovery.error.stale_snapshot",
            ),
            (
                "domain.admin_recovery_network_mismatch",
                "admin_recovery.error.network_mismatch",
            ),
        ];
        for (domain_key, public_key) in cases {
            let error = anyhow::anyhow!("{domain_key}");
            match admin_recovery_protocol_error(&error) {
                SignalingMessage::Error { code, message } => {
                    assert_eq!(message, public_key, "wrong public key for {domain_key}");
                    assert_eq!(
                        code,
                        ErrorCode::BadRequest.as_u16(),
                        "wrong status for {domain_key}"
                    );
                }
                other => panic!("unexpected recovery error message: {other:?}"),
            }
        }

        // Unrelated internal failures must not be published as recovery keys.
        let internal = anyhow::anyhow!("disk exploded while persisting recovery state");
        match admin_recovery_protocol_error(&internal) {
            SignalingMessage::Error { code, message } => {
                assert_eq!(code, ErrorCode::Internal.as_u16());
                assert_eq!(message, "admin_recovery.error.internal");
            }
            other => panic!("unexpected internal error message: {other:?}"),
        }
    }
}
