use anyhow::{Context, Result};
use fuckipv6_common::{
    codec, validate_signaling_message_len, Identity, NatType, PeerInfo, RelayEndpoint,
    SignalingMessage, VirtualIps, MAX_SIGNALING_MESSAGE_SIZE,
};
use futures::{SinkExt, StreamExt};
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use tokio::sync::{mpsc, watch, RwLock};
use tokio::time::{sleep, Duration};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{protocol::WebSocketConfig, Message},
};

const INITIAL_RECONNECT_DELAY: Duration = Duration::from_secs(1);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);

fn signaling_websocket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_SIGNALING_MESSAGE_SIZE))
        .max_frame_size(Some(MAX_SIGNALING_MESSAGE_SIZE))
}

/// 信令客户端
pub struct SignalingClient {
    tx: mpsc::UnboundedSender<SignalingMessage>,
    event_rx: Arc<RwLock<mpsc::UnboundedReceiver<SignalingEvent>>>,
    assigned_ip_rx: watch::Receiver<Option<Ipv6Addr>>,
    assigned_virtual_ips_rx: watch::Receiver<Option<VirtualIps>>,
    relay_endpoint_rx: watch::Receiver<Option<RelayEndpoint>>,
    relay_endpoints_rx: watch::Receiver<Vec<RelayEndpoint>>,
    known_peers_rx: watch::Receiver<Vec<PeerInfo>>,
    shutdown_tx: watch::Sender<bool>,
}

#[derive(Clone)]
struct RegistrationIdentity {
    client_id: String,
    public_key: Vec<u8>,
    private_identity: Arc<Identity>,
}

struct ConnectionConfig {
    url: String,
    registration: RegistrationIdentity,
    p2p_port: u16,
    nat_type: NatType,
    candidate_endpoints: Vec<SocketAddr>,
    initial_reconnect_delay: Duration,
    max_reconnect_delay: Duration,
}

struct ConnectionRuntime<'a> {
    outbound_rx: &'a mut mpsc::UnboundedReceiver<SignalingMessage>,
    pending_outbound: &'a mut Vec<SignalingMessage>,
    channels: ConnectionChannels<'a>,
    shutdown_rx: &'a mut watch::Receiver<bool>,
}

#[derive(Clone, Copy)]
struct ConnectionChannels<'a> {
    event_tx: &'a mpsc::UnboundedSender<SignalingEvent>,
    assigned_ip_tx: &'a watch::Sender<Option<Ipv6Addr>>,
    assigned_virtual_ips_tx: &'a watch::Sender<Option<VirtualIps>>,
    relay_endpoint_tx: &'a watch::Sender<Option<RelayEndpoint>>,
    relay_endpoints_tx: &'a watch::Sender<Vec<RelayEndpoint>>,
    known_peers_tx: &'a watch::Sender<Vec<PeerInfo>>,
}

struct ConnectionState {
    reconnect_delay: Duration,
    has_connected_once: bool,
    assigned_ips: Option<VirtualIps>,
}

fn normalize_ws_url(url: &str) -> String {
    let trimmed = url.trim();
    if trimmed.contains("://") {
        trimmed.to_string()
    } else if trimmed.contains(':') {
        format!("ws://{}", trimmed)
    } else {
        format!("ws://{}:6666", trimmed)
    }
}

impl SignalingClient {
    /// 连接到协调服务器，并在连接断开后自动重连。
    pub async fn connect(
        url: &str,
        identity: Identity,
        p2p_port: u16,
        nat_type: NatType,
    ) -> Result<Self> {
        Self::connect_with_candidates(url, identity, p2p_port, nat_type, Vec::new()).await
    }

    /// 连接到协调服务器并上报 ICE 候选端点，在连接断开后自动重连。
    pub async fn connect_with_candidates(
        url: &str,
        identity: Identity,
        p2p_port: u16,
        nat_type: NatType,
        candidate_endpoints: Vec<SocketAddr>,
    ) -> Result<Self> {
        let (outbound_tx, outbound_rx) = mpsc::unbounded_channel::<SignalingMessage>();
        let (event_tx, event_rx) = mpsc::unbounded_channel::<SignalingEvent>();
        let (assigned_ip_tx, assigned_ip_rx) = watch::channel(None);
        let (assigned_virtual_ips_tx, assigned_virtual_ips_rx) = watch::channel(None);
        let (relay_endpoint_tx, relay_endpoint_rx) = watch::channel(None);
        let (relay_endpoints_tx, relay_endpoints_rx) = watch::channel(Vec::new());
        let (known_peers_tx, known_peers_rx) = watch::channel(Vec::new());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let registration = RegistrationIdentity {
            client_id: identity.client_id().to_string(),
            public_key: identity.public_key(),
            private_identity: Arc::new(identity),
        };

        let connection_config = ConnectionConfig {
            url: normalize_ws_url(url),
            registration,
            p2p_port,
            nat_type,
            candidate_endpoints,
            initial_reconnect_delay: INITIAL_RECONNECT_DELAY,
            max_reconnect_delay: MAX_RECONNECT_DELAY,
        };

        tokio::spawn(connection_manager(
            connection_config,
            outbound_rx,
            event_tx,
            assigned_ip_tx,
            assigned_virtual_ips_tx,
            relay_endpoint_tx,
            relay_endpoints_tx,
            known_peers_tx,
            shutdown_rx,
        ));

        let client = Self {
            tx: outbound_tx,
            event_rx: Arc::new(RwLock::new(event_rx)),
            assigned_ip_rx,
            assigned_virtual_ips_rx,
            relay_endpoint_rx,
            relay_endpoints_rx,
            known_peers_rx,
            shutdown_tx,
        };

        client.wait_for_initial_registration().await?;
        Ok(client)
    }

    /// 获取分配的虚拟IP (IPv6)
    pub async fn assigned_ip(&self) -> Option<Ipv6Addr> {
        *self.assigned_ip_rx.borrow()
    }

    /// 获取分配的双栈虚拟IP
    pub async fn assigned_virtual_ips(&self) -> Option<VirtualIps> {
        self.assigned_virtual_ips_rx.borrow().clone()
    }

    pub async fn relay_endpoint(&self) -> Option<RelayEndpoint> {
        self.relay_endpoint_rx.borrow().clone()
    }

    pub async fn relay_endpoints(&self) -> Vec<RelayEndpoint> {
        self.relay_endpoints_rx.borrow().clone()
    }

    /// 获取信令层已知的在线节点快照。
    pub async fn known_peers(&self) -> Vec<PeerInfo> {
        self.known_peers_rx.borrow().clone()
    }

    /// 发送消息
    pub fn send(&self, msg: SignalingMessage) -> Result<()> {
        self.tx.send(msg)?;
        Ok(())
    }

    /// 发送心跳
    pub fn send_heartbeat(&self) -> Result<()> {
        self.send(SignalingMessage::heartbeat())
    }

    /// 关闭信令连接
    pub async fn close(&self) -> Result<()> {
        tracing::info!("正在关闭信令连接...");
        self.shutdown();
        Ok(())
    }

    /// 同步触发信令管理器关闭
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// 接收事件
    pub async fn recv_event(&self) -> Option<SignalingEvent> {
        self.event_rx.write().await.recv().await
    }

    async fn wait_for_initial_registration(&self) -> Result<()> {
        let mut assigned_ip_rx = self.assigned_ip_rx.clone();
        tokio::time::timeout(Duration::from_secs(5), async move {
            loop {
                if assigned_ip_rx.borrow().is_some() {
                    return Ok(());
                }

                if assigned_ip_rx.changed().await.is_err() {
                    anyhow::bail!("信令连接管理器已停止");
                }
            }
        })
        .await
        .context("等待虚拟 IP 分配超时")?
    }
}

impl Drop for SignalingClient {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[allow(clippy::too_many_arguments)]
async fn connection_manager(
    config: ConnectionConfig,
    mut outbound_rx: mpsc::UnboundedReceiver<SignalingMessage>,
    event_tx: mpsc::UnboundedSender<SignalingEvent>,
    assigned_ip_tx: watch::Sender<Option<Ipv6Addr>>,
    assigned_virtual_ips_tx: watch::Sender<Option<VirtualIps>>,
    relay_endpoint_tx: watch::Sender<Option<RelayEndpoint>>,
    relay_endpoints_tx: watch::Sender<Vec<RelayEndpoint>>,
    known_peers_tx: watch::Sender<Vec<PeerInfo>>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let channels = ConnectionChannels {
        event_tx: &event_tx,
        assigned_ip_tx: &assigned_ip_tx,
        assigned_virtual_ips_tx: &assigned_virtual_ips_tx,
        relay_endpoint_tx: &relay_endpoint_tx,
        relay_endpoints_tx: &relay_endpoints_tx,
        known_peers_tx: &known_peers_tx,
    };
    let mut pending_outbound = Vec::new();
    let mut state = ConnectionState {
        reconnect_delay: config.initial_reconnect_delay,
        has_connected_once: false,
        assigned_ips: None,
    };

    loop {
        if *shutdown_rx.borrow() {
            break;
        }

        tracing::info!("正在连接协调服务器: {}", config.url);
        match connect_async_with_config(&config.url, Some(signaling_websocket_config()), true).await
        {
            Ok((ws_stream, _)) => {
                handle_connected_signaling(
                    ws_stream,
                    &config,
                    &mut state,
                    &mut outbound_rx,
                    &mut pending_outbound,
                    channels,
                    &mut shutdown_rx,
                )
                .await;
            }
            Err(e) => {
                tracing::warn!("连接协调服务器失败: {:?}", e);
                notify_disconnected(channels, format!("{:?}", e));
            }
        }

        if *shutdown_rx.borrow() {
            break;
        }

        tokio::select! {
            _ = sleep(state.reconnect_delay) => {}
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
        }

        state.reconnect_delay = (state.reconnect_delay * 2).min(config.max_reconnect_delay);
    }
}

async fn handle_connected_signaling<S>(
    ws_stream: tokio_tungstenite::WebSocketStream<S>,
    config: &ConnectionConfig,
    state: &mut ConnectionState,
    outbound_rx: &mut mpsc::UnboundedReceiver<SignalingMessage>,
    pending_outbound: &mut Vec<SignalingMessage>,
    channels: ConnectionChannels<'_>,
    shutdown_rx: &mut watch::Receiver<bool>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    tracing::info!("WebSocket 信令链路已建立");
    state.reconnect_delay = config.initial_reconnect_delay;

    if state.has_connected_once {
        let _ = channels.event_tx.send(SignalingEvent::Reconnected);
    }
    state.has_connected_once = true;

    let mut runtime = ConnectionRuntime {
        outbound_rx,
        pending_outbound,
        channels,
        shutdown_rx,
    };

    if let Err(e) = run_connection(
        ws_stream,
        &config.registration,
        config.p2p_port,
        config.nat_type,
        &config.candidate_endpoints,
        &mut state.assigned_ips,
        &mut runtime,
    )
    .await
    {
        tracing::warn!("信令通信链路中断: {:?}", e);
        notify_disconnected(channels, format!("{:?}", e));
    }
}

fn notify_disconnected(channels: ConnectionChannels<'_>, reason: String) {
    let _ = channels.assigned_ip_tx.send(None);
    let _ = channels.relay_endpoint_tx.send(None);
    let _ = channels
        .event_tx
        .send(SignalingEvent::Disconnected { reason });
}

async fn run_connection<S>(
    ws_stream: tokio_tungstenite::WebSocketStream<S>,
    registration: &RegistrationIdentity,
    p2p_port: u16,
    nat_type: NatType,
    candidate_endpoints: &[SocketAddr],
    assigned_ips: &mut Option<VirtualIps>,
    runtime: &mut ConnectionRuntime<'_>,
) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (mut ws_sender, mut ws_receiver) = ws_stream.split();

    send_signaling_message(
        &mut ws_sender,
        build_register_message(
            registration,
            p2p_port,
            nat_type,
            candidate_endpoints,
            assigned_ips.as_ref(),
        ),
    )
    .await
    .context("failed to send initial signaling registration")?;
    flush_pending(&mut ws_sender, runtime.pending_outbound)
        .await
        .context("failed to flush pending signaling messages")?;

    loop {
        tokio::select! {
            _ = runtime.shutdown_rx.changed() => {
                if *runtime.shutdown_rx.borrow() {
                    let _ = ws_sender.send(Message::Close(None)).await;
                    return Ok(());
                }
            }

            maybe_msg = runtime.outbound_rx.recv() => {
                let Some(msg) = maybe_msg else {
                    return Ok(());
                };

                if let Err(e) = send_signaling_message(&mut ws_sender, msg.clone()).await {
                    runtime.pending_outbound.push(msg);
                    return Err(e);
                }
            }

            maybe_ws_msg = ws_receiver.next() => {
                let Some(ws_msg) = maybe_ws_msg else {
                    anyhow::bail!("coordinator closed signaling stream");
                };

                handle_ws_message(ws_msg, &mut ws_sender, runtime, assigned_ips).await?;
            }
        }
    }
}

async fn handle_ws_message<S>(
    ws_msg: Result<Message, tokio_tungstenite::tungstenite::Error>,
    ws_sender: &mut S,
    runtime: &ConnectionRuntime<'_>,
    assigned_ips: &mut Option<VirtualIps>,
) -> Result<()>
where
    S: SinkExt<Message> + Unpin,
    <S as futures::Sink<Message>>::Error: std::error::Error + Send + Sync + 'static,
{
    match ws_msg {
        Ok(Message::Binary(data)) => {
            validate_signaling_message_len(data.len())
                .context("signaling websocket frame exceeds the configured limit")?;
            let msg = codec::decode::<SignalingMessage>(&data)
                .context("failed to deserialize signaling message")?;
            handle_incoming_message(msg, runtime.channels, assigned_ips)?;
        }
        Ok(Message::Ping(data)) => {
            ws_sender
                .send(Message::Pong(data))
                .await
                .context("failed to send signaling pong")?;
        }
        Ok(Message::Close(_)) => {
            anyhow::bail!("coordinator closed signaling connection");
        }
        Err(e) => {
            return Err(e).context("signaling websocket receive failed");
        }
        _ => {}
    }

    Ok(())
}

async fn flush_pending<S>(
    ws_sender: &mut S,
    pending_outbound: &mut Vec<SignalingMessage>,
) -> Result<()>
where
    S: SinkExt<Message> + Unpin,
    <S as futures::Sink<Message>>::Error: std::error::Error + Send + Sync + 'static,
{
    let pending = std::mem::take(pending_outbound);
    for msg in pending {
        if let Err(e) = send_signaling_message(ws_sender, msg.clone()).await {
            pending_outbound.push(msg);
            return Err(e).context("failed to flush pending signaling message");
        }
    }

    Ok(())
}

async fn send_signaling_message<S>(ws_sender: &mut S, msg: SignalingMessage) -> Result<()>
where
    S: SinkExt<Message> + Unpin,
    <S as futures::Sink<Message>>::Error: std::error::Error + Send + Sync + 'static,
{
    let data = codec::encode(&msg).context("failed to serialize signaling message")?;
    validate_signaling_message_len(data.len())
        .context("serialized signaling message exceeds the configured limit")?;
    ws_sender
        .send(Message::Binary(data.into()))
        .await
        .context("failed to send signaling message")?;
    Ok(())
}

fn handle_incoming_message(
    msg: SignalingMessage,
    channels: ConnectionChannels<'_>,
    assigned_ips_state: &mut Option<VirtualIps>,
) -> Result<()> {
    if let SignalingMessage::RegisterAck {
        assigned_ip,
        assigned_ips,
        relay_endpoint,
        relay_endpoints,
        ..
    } = &msg
    {
        let _ = channels.assigned_ip_tx.send(Some(*assigned_ip));
        let ips = assigned_ips
            .clone()
            .unwrap_or_else(|| VirtualIps::from_ipv6(*assigned_ip));
        *assigned_ips_state = Some(ips.clone());
        let _ = channels.assigned_virtual_ips_tx.send(Some(ips.clone()));
        let _ = channels.relay_endpoint_tx.send(relay_endpoint.clone());
        let all_relays =
            RelayEndpoint::combine_relay_endpoints(relay_endpoint.as_ref(), relay_endpoints);
        let _ = channels.relay_endpoints_tx.send(all_relays);
        tracing::info!("分配的虚拟 IP: 主 IPv6={}, 双栈={:?}", assigned_ip, ips);
        if let Some(endpoint) = relay_endpoint {
            tracing::info!("QUIC 中继服务端点: {}", endpoint.addr);
        }
    }

    update_known_peers(&msg, channels.known_peers_tx);

    if let Some(event) = signaling_message_to_event(msg) {
        let _ = channels.event_tx.send(event);
    }

    Ok(())
}

fn update_known_peers(msg: &SignalingMessage, known_peers_tx: &watch::Sender<Vec<PeerInfo>>) {
    match msg {
        SignalingMessage::RegisterAck { peers, .. } => {
            let _ = known_peers_tx.send(peers.clone());
        }
        SignalingMessage::PeerOnline { peer }
        | SignalingMessage::ConnectInfo { peer }
        | SignalingMessage::ConnectNotify { peer } => {
            let mut peers = known_peers_tx.borrow().clone();
            if let Some(existing) = peers
                .iter_mut()
                .find(|known| known.client_id == peer.client_id)
            {
                *existing = peer.clone();
            } else {
                peers.push(peer.clone());
            }
            let _ = known_peers_tx.send(peers);
        }
        SignalingMessage::PeerOffline { client_id } => {
            let mut peers = known_peers_tx.borrow().clone();
            peers.retain(|peer| peer.client_id != *client_id);
            let _ = known_peers_tx.send(peers);
        }
        _ => {}
    }
}

fn build_register_message(
    registration: &RegistrationIdentity,
    p2p_port: u16,
    nat_type: NatType,
    candidate_endpoints: &[SocketAddr],
    requested_ips: Option<&VirtualIps>,
) -> SignalingMessage {
    let timestamp = current_timestamp_secs();
    let challenge = match requested_ips {
        Some(ips) => SignalingMessage::register_challenge_with_ips(
            &registration.client_id,
            &registration.public_key,
            Some(ips),
            p2p_port,
            timestamp,
        ),
        None => SignalingMessage::register_challenge(
            &registration.client_id,
            &registration.public_key,
            p2p_port,
            timestamp,
        ),
    };
    let signature = registration
        .private_identity
        .sign(&challenge)
        .to_bytes()
        .to_vec();

    match requested_ips {
        Some(ips) => SignalingMessage::register_with_ips(
            registration.client_id.clone(),
            registration.public_key.clone(),
            Some(ips.clone()),
            p2p_port,
            nat_type,
            candidate_endpoints.to_vec(),
            timestamp,
            signature,
        ),
        None => SignalingMessage::register_with_candidates(
            registration.client_id.clone(),
            registration.public_key.clone(),
            p2p_port,
            nat_type,
            candidate_endpoints.to_vec(),
            timestamp,
            signature,
        ),
    }
}

fn current_timestamp_secs() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn signaling_message_to_event(msg: SignalingMessage) -> Option<SignalingEvent> {
    match msg {
        SignalingMessage::RegisterAck {
            assigned_ip,
            assigned_ips,
            network_prefix,
            ipv4_prefix,
            peers,
            relay_endpoint,
            relay_endpoints,
        } => Some(SignalingEvent::Registered {
            assigned_ip,
            assigned_ips: assigned_ips.or_else(|| Some(VirtualIps::from_ipv6(assigned_ip))),
            network_prefix,
            ipv4_prefix,
            peers,
            relay_endpoint,
            relay_endpoints,
        }),

        SignalingMessage::PeerOnline { peer } => Some(SignalingEvent::PeerOnline { peer }),

        SignalingMessage::PeerOffline { client_id } => {
            Some(SignalingEvent::PeerOffline { client_id })
        }

        SignalingMessage::RelayPacket {
            src_ip,
            dst_ip,
            payload,
        } => Some(SignalingEvent::RelayPacket {
            src_ip,
            dst_ip,
            // Compatibility boundary: the protected CLI owner still consumes
            // Vec here. Move this event to Bytes when that change lands.
            payload: payload.to_vec(),
        }),

        SignalingMessage::ConnectInfo { peer } => Some(SignalingEvent::ConnectInfo { peer }),

        SignalingMessage::ConnectNotify { peer } => Some(SignalingEvent::ConnectNotify { peer }),

        SignalingMessage::HeartbeatAck { timestamp } => {
            Some(SignalingEvent::HeartbeatAck { timestamp })
        }

        SignalingMessage::Error { code, message } => Some(SignalingEvent::Error { code, message }),

        _ => None,
    }
}

/// 信令事件
#[derive(Debug, Clone)]
pub enum SignalingEvent {
    /// 注册成功
    Registered {
        assigned_ip: Ipv6Addr,
        assigned_ips: Option<VirtualIps>,
        network_prefix: u8,
        ipv4_prefix: Option<u8>,
        peers: Vec<PeerInfo>,
        relay_endpoint: Option<RelayEndpoint>,
        relay_endpoints: Vec<RelayEndpoint>,
    },

    /// 新节点上线
    PeerOnline { peer: PeerInfo },

    /// 节点下线
    PeerOffline { client_id: String },

    /// 通过协调服务器中继收到的数据包 (支持 IPv4 / IPv6)
    RelayPacket {
        src_ip: IpAddr,
        dst_ip: IpAddr,
        payload: Vec<u8>,
    },

    /// 连接信息（用于建立P2P）
    ConnectInfo { peer: PeerInfo },

    /// 连接通知（对方想连接我）
    ConnectNotify { peer: PeerInfo },

    /// 心跳响应
    HeartbeatAck { timestamp: u64 },

    /// 信令连接已断开，会自动重连
    Disconnected { reason: String },

    /// 信令连接已重新建立并已重新发送注册消息
    Reconnected,

    /// 错误
    Error { code: u16, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::SinkExt;
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    #[test]
    fn register_message_has_valid_signature() -> Result<()> {
        let identity = Identity::generate()?;
        let registration = RegistrationIdentity {
            client_id: identity.client_id().to_string(),
            public_key: identity.public_key(),
            private_identity: Arc::new(identity),
        };
        let msg = build_register_message(&registration, 51820, NatType::PortRestricted, &[], None);

        let SignalingMessage::Register {
            client_id,
            public_key,
            p2p_port,
            nat_type,
            timestamp,
            signature,
            ..
        } = msg
        else {
            panic!("expected register message");
        };

        let challenge =
            SignalingMessage::register_challenge(&client_id, &public_key, p2p_port, timestamp);
        assert!(Identity::verify(&public_key, &challenge, &signature));
        assert_eq!(nat_type, NatType::PortRestricted);
        Ok(())
    }

    #[tokio::test]
    async fn connection_manager_reregisters_after_reconnect_and_preserves_ip() -> Result<()> {
        let identity = Identity::generate()?;
        let registration = RegistrationIdentity {
            client_id: identity.client_id().to_string(),
            public_key: identity.public_key(),
            private_identity: Arc::new(identity),
        };

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let url = format!("ws://{}", addr);

        let (_outbound_tx, outbound_rx) = mpsc::unbounded_channel();
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let (assigned_ip_tx, _assigned_ip_rx) = watch::channel(None);
        let (assigned_virtual_ips_tx, _assigned_virtual_ips_rx) = watch::channel(None);
        let (relay_endpoint_tx, _relay_endpoint_rx) = watch::channel(None);
        let (relay_endpoints_tx, _relay_endpoints_rx) = watch::channel(Vec::new());
        let (known_peers_tx, _known_peers_rx) = watch::channel(Vec::new());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let assigned_ip = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0x10, 1);
        let assigned_ips = VirtualIps::from_ipv6(assigned_ip);
        let (close_first_tx, close_first_rx) = tokio::sync::oneshot::channel();
        let (release_second_tx, release_second_rx) = tokio::sync::oneshot::channel();
        let server_task = tokio::spawn(async move {
            serve_register_once(&listener, assigned_ip, None, Some(close_first_rx)).await?;
            serve_register_once(
                &listener,
                assigned_ip,
                Some(assigned_ips),
                Some(release_second_rx),
            )
            .await
        });

        let connection_config = ConnectionConfig {
            url,
            registration,
            p2p_port: 51820,
            nat_type: NatType::FullCone,
            candidate_endpoints: Vec::new(),
            initial_reconnect_delay: Duration::from_millis(20),
            max_reconnect_delay: Duration::from_millis(50),
        };

        let manager_task = tokio::spawn(connection_manager(
            connection_config,
            outbound_rx,
            event_tx,
            assigned_ip_tx,
            assigned_virtual_ips_tx,
            relay_endpoint_tx,
            relay_endpoints_tx,
            known_peers_tx,
            shutdown_rx,
        ));

        let first = tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
            .await?
            .expect("first registration event should arrive");
        assert!(
            matches!(first, SignalingEvent::Registered { .. }),
            "first event was {first:?}"
        );
        close_first_tx
            .send(())
            .expect("test server should still be waiting to close the first connection");

        let mut saw_disconnect = false;
        let mut saw_reconnect = false;
        let mut saw_restored_register = false;

        for _ in 0..5 {
            let event = tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
                .await?
                .expect("reconnect event should arrive");

            match event {
                SignalingEvent::Disconnected { .. } => saw_disconnect = true,
                SignalingEvent::Reconnected => saw_reconnect = true,
                SignalingEvent::Registered { assigned_ip, .. }
                    if assigned_ip == Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0x10, 1) =>
                {
                    saw_restored_register = true;
                    break;
                }
                _ => {}
            }
        }

        release_second_tx
            .send(())
            .expect("test server should still be waiting after the second registration");
        assert!(saw_disconnect);
        assert!(saw_reconnect);
        assert!(saw_restored_register);

        let _ = shutdown_tx.send(true);
        manager_task.abort();
        server_task.await??;
        Ok(())
    }

    async fn serve_register_once(
        listener: &TcpListener,
        assigned_ip: Ipv6Addr,
        expected_requested_ips: Option<VirtualIps>,
        close_after_ack: Option<tokio::sync::oneshot::Receiver<()>>,
    ) -> Result<()> {
        let (stream, _) = listener.accept().await?;
        let mut ws = accept_async(stream).await?;

        let msg = ws
            .next()
            .await
            .expect("client should send register message")?;
        let Message::Binary(data) = msg else {
            panic!("expected binary register");
        };
        let register = codec::decode::<SignalingMessage>(&data)?;
        let SignalingMessage::Register {
            client_id,
            public_key,
            p2p_port,
            timestamp,
            signature,
            requested_ips,
            ..
        } = register
        else {
            panic!("expected register message");
        };

        assert_eq!(requested_ips, expected_requested_ips);
        let challenge = match requested_ips.as_ref() {
            Some(ips) => SignalingMessage::register_challenge_with_ips(
                &client_id,
                &public_key,
                Some(ips),
                p2p_port,
                timestamp,
            ),
            None => {
                SignalingMessage::register_challenge(&client_id, &public_key, p2p_port, timestamp)
            }
        };
        assert!(Identity::verify(&public_key, &challenge, &signature));

        let ack = SignalingMessage::RegisterAck {
            assigned_ip,
            assigned_ips: Some(VirtualIps::from_ipv6(assigned_ip)),
            network_prefix: 64,
            ipv4_prefix: None,
            peers: Vec::new(),
            relay_endpoint: None,
            relay_endpoints: Vec::new(),
        };
        ws.send(Message::Binary(codec::encode(&ack)?.into()))
            .await?;

        if let Some(close_after_ack) = close_after_ack {
            let _ = close_after_ack.await;
            let _ = ws.close(None).await;
        }
        Ok(())
    }
}
