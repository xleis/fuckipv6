use anyhow::{Context, Result};
use fuckipv6_common::{
    build_quic_server_config_with_client_certificate_authorizer, export_quic_relay_channel_binding,
    generate_self_signed_quic_certificate, recv_relay_message, send_relay_message,
    verify_quic_client_identity_binding, ClientId, ErrorCode, Identity, QuicCertificate,
    RelayMessage, RelayRegistrationBinding,
};
use std::collections::HashMap;
use std::net::{Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, Semaphore};

use crate::signaling::session::SessionManager;

const RELAY_BANDWIDTH_LIMIT_BPS: u64 = 1_000_000;
const RELAY_BANDWIDTH_LIMIT_BYTES_PER_SEC: u64 = RELAY_BANDWIDTH_LIMIT_BPS / 8;
const RELAY_BURST_BYTES: u64 = RELAY_BANDWIDTH_LIMIT_BYTES_PER_SEC;
const RELAY_REGISTER_STREAM_ID: u64 = 0;
const MAX_RELAY_CONNECTION_TASKS: usize = 512;
const MAX_RELAY_STREAM_TASKS: usize = 4096;

#[derive(Clone)]
pub struct RelayServer {
    bind_addr: SocketAddr,
    endpoint: quinn::Endpoint,
    session_manager: Arc<SessionManager>,
    state: Arc<RelayState>,
    certificate: QuicCertificate,
}

#[derive(Debug)]
struct RelayState {
    clients: RwLock<HashMap<Ipv6Addr, RelayClient>>,
    limits: RwLock<HashMap<Ipv6Addr, TokenBucket>>,
    connection_tasks: Arc<Semaphore>,
    stream_tasks: Arc<Semaphore>,
}

impl Default for RelayState {
    fn default() -> Self {
        Self {
            clients: RwLock::new(HashMap::new()),
            limits: RwLock::new(HashMap::new()),
            connection_tasks: Arc::new(Semaphore::new(MAX_RELAY_CONNECTION_TASKS)),
            stream_tasks: Arc::new(Semaphore::new(MAX_RELAY_STREAM_TASKS)),
        }
    }
}

#[derive(Debug, Clone)]
struct RelayClient {
    client_id: ClientId,
    public_key: Vec<u8>,
    connection: quinn::Connection,
    connection_id: usize,
}

struct RelayPacketRequest {
    stream_id: u64,
    src_ip: Ipv6Addr,
    dst_ip: Ipv6Addr,
    payload: Vec<u8>,
}

#[derive(Debug, Clone)]
struct TokenBucket {
    tokens: u64,
    last_refill: Instant,
}

impl TokenBucket {
    fn new() -> Self {
        Self {
            tokens: RELAY_BURST_BYTES,
            last_refill: Instant::now(),
        }
    }

    fn allow(&mut self, bytes: usize) -> bool {
        self.refill();
        let bytes = bytes as u64;
        if self.tokens < bytes {
            return false;
        }

        self.tokens -= bytes;
        true
    }

    fn refill(&mut self) {
        let elapsed = self.last_refill.elapsed();
        if elapsed.is_zero() {
            return;
        }

        let refill = (elapsed.as_secs_f64() * RELAY_BANDWIDTH_LIMIT_BYTES_PER_SEC as f64) as u64;
        if refill == 0 {
            return;
        }

        self.tokens = self.tokens.saturating_add(refill).min(RELAY_BURST_BYTES);
        self.last_refill = Instant::now();
    }
}

impl RelayServer {
    pub fn bind(
        bind_addr: SocketAddr,
        session_manager: Arc<SessionManager>,
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
            state: Arc::new(RelayState::default()),
            certificate,
        })
    }

    pub fn bind_with_generated_certificate(
        bind_addr: SocketAddr,
        session_manager: Arc<SessionManager>,
    ) -> Result<Self> {
        let certificate = generate_self_signed_quic_certificate(vec!["fuckipv6.local".to_string()])
            .context("无法生成 QUIC 中继证书")?;
        Self::bind(bind_addr, session_manager, certificate)
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.bind_addr
    }

    pub fn certificate(&self) -> &QuicCertificate {
        &self.certificate
    }

    pub async fn run(self) -> Result<()> {
        tracing::info!("QUIC 中继服务器正在监听: {}", self.bind_addr);

        while let Some(incoming) = self.endpoint.accept().await {
            let Ok(permit) = self.state.connection_tasks.clone().try_acquire_owned() else {
                tracing::warn!(
                    "QUIC 中继连接任务达到上限，拒绝: {}",
                    incoming.remote_address()
                );
                self.session_manager.record_relay_error();
                continue;
            };
            let server = self.clone();
            tokio::spawn(async move {
                let _permit = permit;
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

        loop {
            tokio::select! {
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
            self.session_manager.record_relay_error();
            return true;
        };

        let server = self.clone();
        let src_client_id = registered.client_id.clone();
        let src_public_key = registered.public_key.clone();
        let src_ip = registered.virtual_ip;
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(err) = server
                .handle_packet_stream(
                    &src_client_id,
                    &src_public_key,
                    src_ip,
                    connection_id,
                    send,
                    recv,
                )
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

        let relay_client = RelayClient {
            client_id: client_id.clone(),
            public_key: public_key.clone(),
            connection,
            connection_id,
        };

        self.state
            .clients
            .write()
            .await
            .insert(virtual_ip, relay_client);
        self.state
            .limits
            .write()
            .await
            .entry(virtual_ip)
            .or_insert_with(TokenBucket::new);

        tracing::info!("QUIC 中继客户端注册成功: {} ({})", client_id, virtual_ip);

        Ok(RegisteredRelayClient {
            client_id,
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
        if self
            .session_manager
            .is_blacklisted(client_id, public_key)
            .await
        {
            anyhow::bail!("客户端身份已撤销，拒绝 QUIC 中继注册: {client_id}");
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

    async fn handle_packet_stream(
        &self,
        src_client_id: &ClientId,
        src_public_key: &[u8],
        registered_src_ip: Ipv6Addr,
        src_connection_id: usize,
        mut send: quinn::SendStream,
        mut recv: quinn::RecvStream,
    ) -> Result<()> {
        let request = match self.read_packet_request(&mut recv).await? {
            Ok(request) => request,
            Err((stream_id, error)) => {
                self.send_relay_error_response(&mut send, stream_id, error)
                    .await?;
                return Ok(());
            }
        };

        let dst_client = match self
            .validate_packet_request(
                &request,
                src_client_id,
                src_public_key,
                registered_src_ip,
                src_connection_id,
            )
            .await
        {
            Ok(dst_client) => dst_client,
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

        let ack = RelayMessage::RegisterAck {
            accepted: true,
            message: "forwarded".to_string(),
        };
        send_relay_message(&mut send, request.stream_id, &ack).await?;

        Ok(())
    }

    async fn read_packet_request(
        &self,
        recv: &mut quinn::RecvStream,
    ) -> Result<std::result::Result<RelayPacketRequest, (u64, RelayMessage)>> {
        let (stream_id, message) = recv_relay_message(recv)
            .await
            .context("无法读取 QUIC 中继数据包")?;

        let RelayMessage::Packet {
            src_ip,
            dst_ip,
            payload,
        } = message
        else {
            return Ok(Err((
                stream_id,
                RelayMessage::error(ErrorCode::BadRequest, "expected Packet"),
            )));
        };

        Ok(Ok(RelayPacketRequest {
            stream_id,
            src_ip,
            dst_ip,
            payload,
        }))
    }

    async fn validate_packet_request(
        &self,
        request: &RelayPacketRequest,
        src_client_id: &ClientId,
        src_public_key: &[u8],
        registered_src_ip: Ipv6Addr,
        src_connection_id: usize,
    ) -> std::result::Result<RelayClient, Box<RelayMessage>> {
        if self
            .session_manager
            .is_blacklisted(src_client_id, src_public_key)
            .await
        {
            return Err(Box::new(RelayMessage::error(
                ErrorCode::Forbidden,
                "source client identity has been revoked",
            )));
        }

        if request.src_ip != registered_src_ip {
            return Err(Box::new(RelayMessage::error(
                ErrorCode::Forbidden,
                "source IP does not match relay registration",
            )));
        }

        if !self
            .allow_relay(request.src_ip, request.payload.len())
            .await
        {
            return Err(Box::new(RelayMessage::error(
                ErrorCode::RateLimited,
                "relay bandwidth limit exceeded",
            )));
        }

        let Some(dst_client) = self.lookup_client(request.dst_ip).await else {
            return Err(Box::new(RelayMessage::error(
                ErrorCode::NotFound,
                format!(
                    "destination is not connected to QUIC relay: {}",
                    request.dst_ip
                ),
            )));
        };

        if self
            .session_manager
            .is_blacklisted(&dst_client.client_id, &dst_client.public_key)
            .await
        {
            return Err(Box::new(RelayMessage::error(
                ErrorCode::Forbidden,
                "destination client identity has been revoked",
            )));
        }

        if dst_client.connection_id == src_connection_id && request.src_ip == request.dst_ip {
            return Err(Box::new(RelayMessage::error(
                ErrorCode::BadRequest,
                "refusing relay packet to self",
            )));
        }

        Ok(dst_client)
    }

    async fn send_relay_error_response(
        &self,
        send: &mut quinn::SendStream,
        stream_id: u64,
        error: RelayMessage,
    ) -> Result<()> {
        send_relay_message(send, stream_id, &error).await?;
        self.session_manager.record_relay_error();
        Ok(())
    }

    async fn lookup_client(&self, dst_ip: Ipv6Addr) -> Option<RelayClient> {
        self.state.clients.read().await.get(&dst_ip).cloned()
    }

    async fn allow_relay(&self, src_ip: Ipv6Addr, bytes: usize) -> bool {
        self.state
            .limits
            .write()
            .await
            .entry(src_ip)
            .or_insert_with(TokenBucket::new)
            .allow(bytes)
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
                self.session_manager.record_relay_error();
                anyhow::bail!("目标客户端拒绝 relay 包: {code} {message}")
            }
            Ok(Ok((_, other))) => {
                self.session_manager.record_relay_error();
                anyhow::bail!("目标客户端返回了非预期 relay 响应: {:?}", other)
            }
            Ok(Err(err)) => {
                self.session_manager.record_relay_error();
                Err(err).context("目标客户端 relay 响应读取失败")
            }
            Err(_) => {
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
        let mut clients = self.state.clients.write().await;
        if clients
            .get(&virtual_ip)
            .is_some_and(|client| client.connection_id == connection_id)
        {
            clients.remove(&virtual_ip);
            tracing::info!("QUIC 中继客户端已注销: {}", virtual_ip);
        }
    }

    #[cfg(test)]
    fn new_test_with_limits(
        bind_addr: SocketAddr,
        session_manager: Arc<SessionManager>,
        certificate: QuicCertificate,
        max_connection_tasks: usize,
        max_stream_tasks: usize,
    ) -> Result<Self> {
        let mut server = Self::bind(bind_addr, session_manager, certificate)?;
        server.state = Arc::new(RelayState {
            clients: RwLock::new(HashMap::new()),
            limits: RwLock::new(HashMap::new()),
            connection_tasks: Arc::new(Semaphore::new(max_connection_tasks)),
            stream_tasks: Arc::new(Semaphore::new(max_stream_tasks)),
        });
        Ok(server)
    }
}

#[derive(Debug, Clone)]
struct RegisteredRelayClient {
    client_id: ClientId,
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
    use fuckipv6_common::relay_client::QuicRelayClient;
    use fuckipv6_common::{
        build_pinned_quic_client_config_with_auth, export_quic_relay_channel_binding,
        recv_relay_message, send_relay_message, Identity, NatType, QuicCertificate, RelayMessage,
        RelayRegistrationBinding,
    };
    use std::net::{IpAddr, Ipv4Addr};
    use tokio::sync::mpsc;

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
        };
        mutate(&mut message);

        let (mut send, mut recv) = connection.open_bi().await?;
        send_relay_message(&mut send, RELAY_REGISTER_STREAM_ID, &message).await?;
        let (_, response) = recv_relay_message(&mut recv).await?;
        connection.close(0u32.into(), b"test-done");
        endpoint.wait_idle().await;
        Ok(response)
    }

    async fn assert_no_relay_state(server: &RelayServer, session_manager: &Arc<SessionManager>) {
        assert!(server.state.clients.read().await.is_empty());
        assert!(server.state.limits.read().await.is_empty());
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
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.clone().run());

        let connect_result = tokio::time::timeout(
            Duration::from_secs(5),
            QuicRelayClient::connect(
                relay_addr,
                server_pin,
                "fuckipv6.local",
                &tls_identity,
                virtual_ip,
            ),
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
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.clone().run());

        let first_client = QuicRelayClient::connect(
            relay_addr,
            server_pin,
            "fuckipv6.local",
            &identity,
            virtual_ip,
        )
        .await
        .context("registered identity should pass the certificate allowlist")?;
        first_client.close().await;

        session_manager
            .unregister_client(&identity.client_id().to_string())
            .await;
        let reconnect = tokio::time::timeout(
            Duration::from_secs(5),
            QuicRelayClient::connect(
                relay_addr,
                server_pin,
                "fuckipv6.local",
                &identity,
                virtual_ip,
            ),
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
        let revoked_fingerprint = Identity::public_key_fingerprint(&identity.public_key());
        session_manager
            .ban_client(revoked_fingerprint[..16].to_string())
            .await;

        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager.clone(),
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
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.clone().run());

        let client_a =
            QuicRelayClient::connect(relay_addr, server_pin, "fuckipv6.local", &identity_a, ip_a)
                .await?;
        let client_b =
            QuicRelayClient::connect(relay_addr, server_pin, "fuckipv6.local", &identity_b, ip_b)
                .await?;

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
        assert_eq!(server.state.limits.read().await.len(), 2);

        let payload = b"relay-through-quic".to_vec();
        let (sent, delivered) = tokio::join!(
            client_a.send_packet(7, ip_a, ip_b, &payload),
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

        client_a.close().await;
        client_b.close().await;
        server_task.abort();
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
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.run());
        let client_a =
            QuicRelayClient::connect(relay_addr, server_pin, "fuckipv6.local", &identity_a, ip_a)
                .await?;
        let client_b =
            QuicRelayClient::connect(relay_addr, server_pin, "fuckipv6.local", &identity_b, ip_b)
                .await?;

        let revoked_fingerprint = Identity::public_key_fingerprint(&identity_b.public_key());
        session_manager
            .ban_client(revoked_fingerprint[..16].to_string())
            .await;
        let destination_error = client_a
            .send_packet(1, ip_a, ip_b, b"revoked destination")
            .await
            .expect_err("revoked destination must not receive relay packets")
            .to_string();
        assert!(destination_error.contains("403"));

        session_manager
            .ban_client(identity_a.client_id().to_string())
            .await;
        let source_error = client_a
            .send_packet(2, ip_a, ip_b, b"revoked source")
            .await
            .expect_err("revoked source must not send relay packets")
            .to_string();
        assert!(source_error.contains("403"));

        let traffic = session_manager.snapshot().await.traffic;
        assert_eq!(traffic.relay_packets, 0);
        assert_eq!(traffic.relay_payload_bytes, 0);

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
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.clone().run());
        let client =
            QuicRelayClient::connect(relay_addr, server_pin, "fuckipv6.local", &identity, ip)
                .await?;

        let wrong_src = "fd00::ffff".parse()?;
        let response = client
            .send_packet(1, wrong_src, "fd00::1002".parse()?, b"bad")
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
            certificate,
            8,
            0,
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.run());
        let client =
            QuicRelayClient::connect(relay_addr, server_pin, "fuckipv6.local", &identity, ip)
                .await?;

        let result = client
            .send_packet(1, ip, "fd00::1002".parse()?, b"overloaded")
            .await;

        let error = result.unwrap_err().to_string();
        assert!(error.contains("503"));

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

        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            session_manager.clone(),
        )?;
        let relay_addr = server.local_addr();
        let server_pin = server.certificate().public_key_pin;
        let server_task = tokio::spawn(server.clone().run());

        let mut tasks = Vec::with_capacity(CLIENTS);
        for (identity, virtual_ip) in identities.into_iter().zip(virtual_ips.iter().copied()) {
            tasks.push(tokio::spawn(async move {
                QuicRelayClient::connect(
                    relay_addr,
                    server_pin,
                    "fuckipv6.local",
                    &identity,
                    virtual_ip,
                )
                .await
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
