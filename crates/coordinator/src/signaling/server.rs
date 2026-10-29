use anyhow::{Context, Result};
use fuckipv6_common::{codec, ErrorCode, Identity, RelayEndpoint, SignalingMessage};
use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::{accept_async, tungstenite::Message};

use super::session::SessionManager;

fn verify_registration(
    client_id: &str,
    public_key: &[u8],
    p2p_port: u16,
    timestamp: u64,
    signature: &[u8],
) -> bool {
    let challenge =
        SignalingMessage::register_challenge(client_id, public_key, p2p_port, timestamp);

    Identity::verify_peer_fingerprint(public_key, client_id)
        && Identity::verify(public_key, &challenge, signature)
}

/// 信令服务器
pub struct SignalingServer {
    bind_addr: String,
    session_manager: Arc<SessionManager>,
    relay_endpoint: Option<RelayEndpoint>,
    relay_endpoints: Arc<Vec<RelayEndpoint>>,
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
        })
    }

    pub fn new_with_manager(bind_addr: String, session_manager: Arc<SessionManager>) -> Self {
        Self {
            bind_addr,
            session_manager,
            relay_endpoint: None,
            relay_endpoints: Arc::new(Vec::new()),
        }
    }

    pub fn with_relay_endpoint(mut self, relay_endpoint: RelayEndpoint) -> Self {
        self.relay_endpoint = Some(relay_endpoint);
        self
    }

    pub fn with_relay_endpoints(mut self, relay_endpoints: Vec<RelayEndpoint>) -> Self {
        self.relay_endpoints = Arc::new(relay_endpoints);
        self
    }

    pub fn session_manager(&self) -> Arc<SessionManager> {
        self.session_manager.clone()
    }

    /// 启动服务器
    pub async fn run(self) -> Result<()> {
        let listener = TcpListener::bind(&self.bind_addr)
            .await
            .with_context(|| format!("无法监听信令服务: {}", self.bind_addr))?;
        tracing::info!("信令服务器正在监听: {}", self.bind_addr);

        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    tracing::info!("新连接来自: {}", addr);
                    let session_manager = self.session_manager.clone();
                    let relay_endpoint = self.relay_endpoint.clone();
                    let relay_endpoints = self.relay_endpoints.clone();

                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(
                            stream,
                            addr,
                            session_manager,
                            relay_endpoint,
                            relay_endpoints,
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

/// 处理单个WebSocket连接
async fn handle_connection(
    stream: TcpStream,
    addr: SocketAddr,
    session_manager: Arc<SessionManager>,
    relay_endpoint: Option<RelayEndpoint>,
    relay_endpoints: Arc<Vec<RelayEndpoint>>,
) -> Result<()> {
    let local_addr = stream.local_addr().ok();
    let ws_stream = accept_async(stream)
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
) -> bool {
    match msg {
        Ok(Message::Binary(data)) => {
            handle_binary_message(
                data.to_vec(),
                addr,
                session_manager,
                tx,
                client_id,
                local_addr,
                relay_endpoint,
                relay_endpoints,
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
    data: Vec<u8>,
    addr: SocketAddr,
    session_manager: &Arc<SessionManager>,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &mut Option<String>,
    local_addr: Option<SocketAddr>,
    relay_endpoint: Option<&RelayEndpoint>,
    relay_endpoints: &[RelayEndpoint],
) {
    tracing::debug!("收到二进制消息: {} 字节 (来自 {})", data.len(), addr);
    tracing::trace!("数据前32字节: {:?}", &data[..data.len().min(32)]);
    session_manager.record_signaling_rx(data.len());

    match codec::decode::<SignalingMessage>(&data) {
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

        _ => {
            tracing::warn!("收到未预期的信令消息类型: {:?}", msg);
        }
    }

    Ok(())
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
    } = msg
    else {
        unreachable!("handle_register_message only accepts Register messages");
    };

    if !verify_registration(&id, &public_key, p2p_port, timestamp, &signature) {
        let error = SignalingMessage::protocol_error(
            ErrorCode::Unauthorized,
            "Invalid registration signature",
        );
        send_binary(tx, session_manager, error)?;
        return Ok(());
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
            tx.clone(),
        )
        .await
        .with_context(|| format!("注册客户端失败: {id}"))?;

    let peers = session_manager.get_all_peers().await;
    let peers: Vec<_> = peers
        .into_iter()
        .filter(|peer| peer.client_id != id)
        .collect();

    tracing::info!("通知客户端 {} 有 {} 个在线节点", id, peers.len());

    let ipv4_prefix = session_manager.ipv4_prefix().await;
    let ipv6_prefix = session_manager.ipv6_prefix().await;

    let response = SignalingMessage::RegisterAck {
        assigned_ip: virtual_ips.primary_ipv6(),
        assigned_ips: Some(virtual_ips),
        network_prefix: ipv6_prefix,
        ipv4_prefix: Some(ipv4_prefix),
        peers,
        relay_endpoint: relay_endpoint
            .cloned()
            .map(|endpoint| endpoint.for_signaling_connection(local_addr)),
        relay_endpoints: relay_endpoints
            .iter()
            .cloned()
            .map(|endpoint| endpoint.for_signaling_connection(local_addr))
            .collect(),
    };

    send_binary(tx, session_manager, response)?;
    *client_id = Some(id.clone());

    tracing::info!("客户端 {} 完成注册流程", id);
    Ok(())
}

async fn handle_connect_request(
    target_id: String,
    session_manager: &Arc<SessionManager>,
    tx: &mpsc::UnboundedSender<Message>,
    client_id: &Option<String>,
) -> Result<()> {
    let Some(peer) = session_manager.get_peer_info(&target_id).await else {
        let error = SignalingMessage::protocol_error(ErrorCode::NotFound, "Peer not found");
        send_binary(tx, session_manager, error)?;
        return Ok(());
    };

    send_binary(
        tx,
        session_manager,
        SignalingMessage::ConnectInfo { peer: peer.clone() },
    )?;

    if let Some(requester_id) = client_id {
        if let Some(requester_peer) = session_manager.get_peer_info(requester_id).await {
            session_manager
                .send_to_client(
                    &target_id,
                    SignalingMessage::ConnectNotify {
                        peer: requester_peer,
                    },
                )
                .await
                .with_context(|| format!("通知目标节点连接请求失败: {target_id}"))?;
        }
    }

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
    payload: Vec<u8>,
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
            p2p_port,
            timestamp,
            &signature
        ));

        assert!(!verify_registration(
            &client_id,
            &public_key,
            p2p_port + 1,
            timestamp,
            &signature
        ));

        assert!(!verify_registration(
            "wrong-client-id",
            &public_key,
            p2p_port,
            timestamp,
            &signature
        ));
        Ok(())
    }
}
