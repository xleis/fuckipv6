use anyhow::{Context, Result};
use bytes::Bytes;
use fuckipv6_common::domain::{JoinRequest, JoinRequestId, JoinRequestState, NetworkId, NodeId};
use fuckipv6_common::{
    codec, validate_signaling_message_len, validate_signaling_relay_payload_len, CoordinatorConfig,
    ErrorCode, Identity, JoinRequestBatchReviewItem, RelayEndpoint, SignalingMessage,
    MAX_SIGNALING_MESSAGE_SIZE,
};
use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::{
    accept_async_with_config,
    tungstenite::{protocol::WebSocketConfig, Message},
};

use super::join_requests::JoinRequestStore;
use super::session::SessionManager;

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

/// 信令服务器
pub struct SignalingServer {
    bind_addr: String,
    session_manager: Arc<SessionManager>,
    relay_endpoint: Option<RelayEndpoint>,
    relay_endpoints: Arc<Vec<RelayEndpoint>>,
    join_request_store: Option<Arc<JoinRequestStore>>,
    listener: Option<TcpListener>,
}

impl SignalingServer {
    /// 创建信令服务器
    pub fn new(bind_addr: String, ip_pool: &str) -> Result<Self> {
        let session_manager = Arc::new(SessionManager::new(ip_pool)?);

        Ok(Self {
            bind_addr,
            session_manager,
            relay_endpoint: None,
            relay_endpoints: Arc::new(Vec::new()),
            join_request_store: None,
            listener: None,
        })
    }

    pub fn new_with_manager(bind_addr: String, session_manager: Arc<SessionManager>) -> Self {
        Self {
            bind_addr,
            session_manager,
            relay_endpoint: None,
            relay_endpoints: Arc::new(Vec::new()),
            join_request_store: None,
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

    pub fn with_relay_endpoint(mut self, relay_endpoint: RelayEndpoint) -> Self {
        self.relay_endpoint = Some(relay_endpoint);
        self
    }

    pub fn with_relay_endpoints(mut self, relay_endpoints: Vec<RelayEndpoint>) -> Self {
        self.relay_endpoints = Arc::new(relay_endpoints);
        self
    }

    /// Attach the durable approval store. Without one, approval messages fail
    /// closed while registration and data-plane signaling remain available.
    pub fn with_join_request_store(mut self, store: Arc<JoinRequestStore>) -> Self {
        self.join_request_store = Some(store);
        self
    }

    /// Build the approval store from the same persisted coordinator config used
    /// for allocation and revocation.
    pub fn with_join_request_store_from_config(
        mut self,
        state_dir: impl AsRef<std::path::Path>,
        config: &CoordinatorConfig,
    ) -> Result<Self> {
        let store = JoinRequestStore::new(
            state_dir.as_ref().join("join_requests.json"),
            config.network_record_path.clone(),
            config.owner_node_id.clone(),
            config.administrator_node_ids.clone(),
        )?;
        self.join_request_store = Some(Arc::new(store));
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
                    let relay_endpoint = self.relay_endpoint.clone();
                    let relay_endpoints = self.relay_endpoints.clone();
                    let join_request_store = self.join_request_store.clone();

                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(
                            stream,
                            addr,
                            session_manager,
                            relay_endpoint,
                            relay_endpoints,
                            join_request_store,
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
    }
}

fn signaling_websocket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_SIGNALING_MESSAGE_SIZE))
        .max_frame_size(Some(MAX_SIGNALING_MESSAGE_SIZE))
}

/// 处理单个WebSocket连接
async fn handle_connection(
    stream: TcpStream,
    addr: SocketAddr,
    session_manager: Arc<SessionManager>,
    relay_endpoint: Option<RelayEndpoint>,
    relay_endpoints: Arc<Vec<RelayEndpoint>>,
    join_request_store: Option<Arc<JoinRequestStore>>,
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
            relay_endpoint.as_ref(),
            &relay_endpoints,
            join_request_store.as_deref(),
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
    relay_endpoint: Option<&RelayEndpoint>,
    relay_endpoints: &[RelayEndpoint],
    join_request_store: Option<&JoinRequestStore>,
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
                relay_endpoint,
                relay_endpoints,
                join_request_store,
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
    relay_endpoint: Option<&RelayEndpoint>,
    relay_endpoints: &[RelayEndpoint],
    join_request_store: Option<&JoinRequestStore>,
) {
    tracing::debug!("收到二进制消息: {} 字节 (来自 {})", data.len(), addr);
    tracing::trace!("数据前32字节: {:?}", &data[..data.len().min(32)]);
    session_manager.record_signaling_rx(data.len());
    if let Err(error) = validate_signaling_message_len(data.len()) {
        tracing::warn!("拒绝超限信令帧 (来自 {addr}): {error:#}");
        return;
    }

    match codec::decode::<SignalingMessage>(data) {
        Ok(msg) => {
            tracing::trace!("消息解析成功: {:?}", msg);
            if let Err(e) = handle_signaling_message(
                msg,
                &addr,
                session_manager,
                tx,
                client_id,
                local_addr,
                relay_endpoint,
                relay_endpoints,
                join_request_store,
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
    relay_endpoint: Option<&RelayEndpoint>,
    relay_endpoints: &[RelayEndpoint],
    join_request_store: Option<&JoinRequestStore>,
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
                relay_endpoint,
                relay_endpoints,
            )
            .await?;
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
        // These variants are server-to-client events. Accepting one from a
        // client would let it manufacture approval state in another view.
        SignalingMessage::JoinRequestStatus { .. }
        | SignalingMessage::JoinRequestListResponse { .. }
        | SignalingMessage::JoinRequestReviewAck { .. }
        | SignalingMessage::JoinRequestChanged { .. } => {
            send_binary(
                tx,
                session_manager,
                SignalingMessage::protocol_error(
                    ErrorCode::BadRequest,
                    "Approval response messages are server-originated",
                ),
            )?;
        }
        _ => {
            tracing::warn!("收到未预期的信令消息类型: {:?}", msg);
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

async fn handle_join_request_review(
    network_id: NetworkId,
    reviews: Vec<JoinRequestBatchReviewItem>,
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

    // Resolve applicants before the atomic decision so online notifications can
    // be delivered without another authorization-dependent read afterwards.
    let mut applicants = Vec::with_capacity(reviews.len());
    for review in &reviews {
        let request = store
            .status(&network_id, &actor, &review.request_id, unix_time_ms())?
            .with_context(|| format!("join request not found: {}", review.request_id))?;
        applicants.push((review.request_id.clone(), request.applicant.node_id));
    }

    let results = store.review(&network_id, &actor, reviews, unix_time_ms())?;
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
    relay_endpoint: Option<&RelayEndpoint>,
    relay_endpoints: &[RelayEndpoint],
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

    let response = SignalingMessage::RegisterAck {
        assigned_ip: virtual_ips.primary_ipv6(),
        assigned_ips: Some(virtual_ips),
        network_prefix: ipv6_prefix,
        ipv4_prefix: Some(ipv4_prefix),
        peers,
        relay_endpoint: is_data_plane_member
            .then(|| {
                relay_endpoint
                    .cloned()
                    .map(|endpoint| endpoint.for_signaling_connection(local_addr))
            })
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
}
