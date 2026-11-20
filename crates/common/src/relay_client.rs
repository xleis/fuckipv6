use anyhow::{Context, Result};
use bytes::Bytes;
use std::net::{Ipv6Addr, SocketAddr};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, RwLock};

use crate::crypto::Identity;
use crate::domain::{NodeId, RelayMemberGrant};
use crate::protocol::RelayEndpoint;
use crate::quic::{
    build_pinned_quic_client_config_with_auth, export_quic_relay_channel_binding,
    recv_relay_message, send_relay_message, QuicCertificate, RelayMessage,
    RelayRegistrationBinding,
};

const RELAY_REGISTER_STREAM_ID: u64 = 0;
const RELAY_AUTHORIZATION_STREAM_ID: u64 = 1;
/// Bounded application ingress for packets received from a Relay.
///
/// A slow consumer applies backpressure to the forwarding stream instead of
/// allowing the client process to grow an unbounded packet queue.
pub const RELAY_INCOMING_QUEUE_CAPACITY: usize = 256;

#[derive(Clone)]
pub struct QuicRelayClient {
    endpoint: quinn::Endpoint,
    connection: quinn::Connection,
    incoming_rx: Arc<Mutex<mpsc::Receiver<RelayPacket>>>,
    member_grant: Arc<RwLock<RelayMemberGrant>>,
    incoming_task: Arc<IncomingTask>,
}

struct IncomingTask(Mutex<Option<tokio::task::JoinHandle<()>>>);

impl IncomingTask {
    async fn stop(&self) {
        let mut slot = self.0.lock().await;
        if let Some(task) = slot.as_mut() {
            task.abort();
            let result = task.await;
            slot.take();
            if let Err(error) = result {
                if !error.is_cancelled() {
                    tracing::error!(%error, "Relay receive task failed");
                }
            }
        }
    }
}

impl Drop for IncomingTask {
    fn drop(&mut self) {
        if let Some(task) = self.0.get_mut().take() {
            task.abort();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayPacket {
    pub src_ip: Ipv6Addr,
    pub dst_ip: Ipv6Addr,
    pub payload: Bytes,
}

/// Payload accepted by [`QuicRelayClient::send_packet`].
///
/// An owned [`Bytes`] is forwarded without copying. Borrowed byte slices are a
/// compatibility path for callers that only have transient buffers and pay one
/// explicit copy at this boundary.
pub trait IntoRelayPayload {
    fn into_relay_payload(self) -> Bytes;
}

impl IntoRelayPayload for Bytes {
    fn into_relay_payload(self) -> Bytes {
        self
    }
}

impl IntoRelayPayload for &[u8] {
    fn into_relay_payload(self) -> Bytes {
        Bytes::copy_from_slice(self)
    }
}

impl IntoRelayPayload for &Vec<u8> {
    fn into_relay_payload(self) -> Bytes {
        Bytes::copy_from_slice(self)
    }
}

impl QuicRelayClient {
    pub async fn connect(
        relay_addr: SocketAddr,
        expected_server_pin: [u8; 32],
        server_name: &str,
        identity: &Identity,
        virtual_ip: Ipv6Addr,
        member_grant: &RelayMemberGrant,
    ) -> Result<Self> {
        validate_member_grant(identity, member_grant)?;
        let bind_addr = if relay_addr.is_ipv6() {
            SocketAddr::new(std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 0)
        } else {
            SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0)
        };
        let client_certificate = QuicCertificate::from_identity(identity)?;
        let client_certificate_public_key_pin = client_certificate.public_key_pin;
        let mut endpoint =
            quinn::Endpoint::client(bind_addr).context("无法创建 QUIC relay 客户端 endpoint")?;
        endpoint.set_default_client_config(build_pinned_quic_client_config_with_auth(
            expected_server_pin,
            &client_certificate,
        )?);

        let connection = endpoint
            .connect(relay_addr, server_name)
            .with_context(|| format!("无法连接 QUIC relay: {relay_addr}"))?
            .await
            .context("QUIC relay 握手失败")?;

        register_relay_connection(
            &connection,
            identity,
            virtual_ip,
            expected_server_pin,
            client_certificate_public_key_pin,
            member_grant,
        )
        .await?;
        let (incoming_tx, incoming_rx) = mpsc::channel(RELAY_INCOMING_QUEUE_CAPACITY);
        let incoming_task = spawn_incoming_task(connection.clone(), incoming_tx);

        Ok(Self {
            endpoint,
            connection,
            incoming_rx: Arc::new(Mutex::new(incoming_rx)),
            member_grant: Arc::new(RwLock::new(member_grant.clone())),
            incoming_task: Arc::new(IncomingTask(Mutex::new(Some(incoming_task)))),
        })
    }

    /// Return the signed grant currently accepted by the Relay.
    pub async fn current_member_grant(&self) -> RelayMemberGrant {
        self.member_grant.read().await.clone()
    }

    /// Replace the signed authorization on the live connection before the
    /// current grant expires. A rejected update leaves both the local copy and
    /// the Relay registration unchanged.
    pub async fn update_authorization(
        &self,
        identity: &Identity,
        member_grant: &RelayMemberGrant,
    ) -> Result<()> {
        validate_member_grant(identity, member_grant)?;
        let current = self.member_grant.read().await.clone();
        member_grant
            .validate_binding(
                &current.network_id,
                &current.provider_node_id,
                &current.consumer_node_id,
                now_unix_ms(),
            )
            .map_err(anyhow::Error::new)?;
        if member_grant.issued_at_unix_ms < current.issued_at_unix_ms
            || member_grant.expires_at_unix_ms <= current.expires_at_unix_ms
            || member_grant.nonce == current.nonce
        {
            anyhow::bail!("Relay 授权更新没有延长有效期或复用了 nonce");
        }

        let update = RelayMessage::UpdateAuthorization {
            member_grant: Box::new(member_grant.clone()),
        };
        let (mut send, mut recv) = self
            .connection
            .open_bi()
            .await
            .context("无法打开 QUIC relay 授权更新流")?;
        send_relay_message(&mut send, RELAY_AUTHORIZATION_STREAM_ID, &update)
            .await
            .context("无法发送 QUIC relay 授权更新")?;
        let (_, response) = recv_relay_message(&mut recv)
            .await
            .context("无法读取 QUIC relay 授权更新响应")?;
        match response {
            RelayMessage::AuthorizationAck { accepted: true, .. } => {
                *self.member_grant.write().await = member_grant.clone();
                Ok(())
            }
            RelayMessage::AuthorizationAck { message, .. } => {
                anyhow::bail!("QUIC relay 授权更新被拒绝: {message}")
            }
            RelayMessage::Error { code, message } => {
                anyhow::bail!("QUIC relay 授权更新失败: {code} {message}")
            }
            other => anyhow::bail!("QUIC relay 授权更新返回了非预期响应: {:?}", other),
        }
    }

    /// 从多个候选 QUIC 中继节点中并发探测并选择延迟最低的最佳中继
    pub async fn connect_optimal(
        relays: &[RelayEndpoint],
        identity: &Identity,
        virtual_ip: Ipv6Addr,
        member_grants: &[RelayMemberGrant],
    ) -> Result<Self> {
        if relays.is_empty() {
            anyhow::bail!("No candidate relay endpoints provided");
        }
        if relays.len() == 1 {
            let ep = &relays[0];
            let grant = member_grant_for_endpoint(ep, identity, member_grants)?;
            return Self::connect(
                ep.addr,
                ep.public_key_pin,
                &ep.server_name,
                identity,
                virtual_ip,
                grant,
            )
            .await;
        }

        let mut tasks = tokio::task::JoinSet::new();
        for ep in relays.iter().cloned() {
            let grant = match member_grant_for_endpoint(&ep, identity, member_grants) {
                Ok(grant) => grant.clone(),
                Err(error) => {
                    tracing::debug!(
                        "QUIC 中继 {} ({}) 缺少匹配的签名授权: {error:#}",
                        ep.server_name,
                        ep.addr
                    );
                    continue;
                }
            };
            let id = identity.clone();
            tasks.spawn(async move {
                let start = std::time::Instant::now();
                match Self::connect(
                    ep.addr,
                    ep.public_key_pin,
                    &ep.server_name,
                    &id,
                    virtual_ip,
                    &grant,
                )
                .await
                {
                    Ok(client) => {
                        let rtt = client.connection.rtt();
                        let elapsed = start.elapsed();
                        let score = rtt.min(elapsed);
                        tracing::info!(
                            "QUIC 中继 {} ({}) 握手成功, 评分延迟: {:?}",
                            ep.server_name,
                            ep.addr,
                            score
                        );
                        Ok((client, score, ep))
                    }
                    Err(e) => {
                        tracing::debug!(
                            "QUIC 中继 {} ({}) 连接失败: {:?}",
                            ep.server_name,
                            ep.addr,
                            e
                        );
                        Err(e)
                    }
                }
            });
        }

        let mut best: Option<(Self, std::time::Duration, RelayEndpoint)> = None;
        while let Some(res) = tasks.join_next().await {
            if let Ok(Ok((client, score, ep))) = res {
                if let Some((best_client, best_score, _)) = &best {
                    if score < *best_score {
                        best_client.close().await;
                        best = Some((client, score, ep));
                    } else {
                        client.close().await;
                    }
                } else {
                    best = Some((client, score, ep));
                }
            }
        }

        if let Some((client, score, ep)) = best {
            tracing::info!(
                "选定最佳 QUIC 中继节点: {} ({}), 延迟: {:?}",
                ep.server_name,
                ep.addr,
                score
            );
            Ok(client)
        } else {
            anyhow::bail!("所有候选 QUIC 中继节点连接均失败");
        }
    }

    pub async fn send_packet(
        &self,
        stream_id: u64,
        src_ip: Ipv6Addr,
        dst_ip: Ipv6Addr,
        payload: impl IntoRelayPayload,
    ) -> Result<()> {
        let packet = RelayMessage::Packet {
            src_ip,
            dst_ip,
            payload: payload.into_relay_payload(),
        };
        let (mut send, mut recv) = self
            .connection
            .open_bi()
            .await
            .context("无法打开 QUIC relay 发送流")?;
        send_relay_message(&mut send, stream_id, &packet).await?;

        let (_, response) = recv_relay_message(&mut recv)
            .await
            .context("无法读取 QUIC relay 发送响应")?;
        match response {
            RelayMessage::RegisterAck { accepted: true, .. } => Ok(()),
            RelayMessage::Error { code, message } => {
                anyhow::bail!("QUIC relay 转发失败: {code} {message}")
            }
            other => anyhow::bail!("QUIC relay 返回了非预期响应: {:?}", other),
        }
    }

    pub async fn recv_packet(&self) -> Option<RelayPacket> {
        self.incoming_rx.lock().await.recv().await
    }

    pub async fn close(&self) {
        self.connection.close(0u32.into(), b"done");
        self.endpoint.close(0u32.into(), b"done");
        self.incoming_task.stop().await;
        let mut incoming = self.incoming_rx.lock().await;
        incoming.close();
        while incoming.try_recv().is_ok() {}
    }
}

async fn register_relay_connection(
    connection: &quinn::Connection,
    identity: &Identity,
    virtual_ip: Ipv6Addr,
    server_public_key_pin: [u8; 32],
    client_certificate_public_key_pin: [u8; 32],
    member_grant: &RelayMemberGrant,
) -> Result<()> {
    let timestamp = now_secs();
    let public_key = identity.public_key();
    let registration = RelayRegistrationBinding {
        client_id: identity.client_id(),
        virtual_ip,
        public_key: &public_key,
        timestamp,
        server_public_key_pin,
        client_certificate_public_key_pin,
        tls_channel_binding: export_quic_relay_channel_binding(connection)?,
    };
    let signature = identity.sign(&registration.challenge()).to_bytes().to_vec();
    let register = RelayMessage::Register {
        client_id: identity.client_id().to_string(),
        virtual_ip,
        public_key: public_key.clone(),
        timestamp,
        server_public_key_pin,
        client_certificate_public_key_pin,
        tls_channel_binding: registration.tls_channel_binding,
        signature,
        member_grant: Some(Box::new(member_grant.clone())),
    };

    let (mut send, mut recv) = connection
        .open_bi()
        .await
        .context("无法打开 QUIC relay 注册流")?;
    send_relay_message(&mut send, RELAY_REGISTER_STREAM_ID, &register)
        .await
        .context("无法发送 QUIC relay 注册消息")?;
    let (_, response) = recv_relay_message(&mut recv)
        .await
        .context("无法读取 QUIC relay 注册响应")?;

    match response {
        RelayMessage::RegisterAck { accepted: true, .. } => Ok(()),
        RelayMessage::RegisterAck { message, .. } => {
            anyhow::bail!("QUIC relay 注册被拒绝: {message}")
        }
        RelayMessage::Error { code, message } => {
            anyhow::bail!("QUIC relay 注册失败: {code} {message}")
        }
        other => anyhow::bail!("QUIC relay 注册返回了非预期响应: {:?}", other),
    }
}

fn spawn_incoming_task(
    connection: quinn::Connection,
    incoming_tx: mpsc::Sender<RelayPacket>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let (mut send, mut recv) = match connection.accept_bi().await {
                Ok(stream) => stream,
                Err(err) => {
                    tracing::debug!("QUIC relay 接收流结束: {err:#}");
                    break;
                }
            };

            let result = async {
                let (stream_id, message) = recv_relay_message(&mut recv).await?;
                match message {
                    RelayMessage::Packet {
                        src_ip,
                        dst_ip,
                        payload,
                    } => {
                        incoming_tx
                            .send(RelayPacket {
                                src_ip,
                                dst_ip,
                                payload,
                            })
                            .await?;
                        let ack = RelayMessage::RegisterAck {
                            accepted: true,
                            message: "accepted".to_string(),
                        };
                        send_relay_message(&mut send, stream_id, &ack).await?;
                    }
                    RelayMessage::Error { code, message } => {
                        tracing::warn!("QUIC relay 服务端返回错误: {code} {message}");
                    }
                    other => {
                        tracing::warn!("QUIC relay 接收流忽略非 Packet 消息: {:?}", other);
                    }
                }
                Ok::<(), anyhow::Error>(())
            }
            .await;

            if let Err(err) = result {
                tracing::warn!("QUIC relay 接收流处理失败: {err:#}");
            }
        }
    })
}

fn now_secs() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn validate_member_grant(identity: &Identity, grant: &RelayMemberGrant) -> Result<()> {
    let consumer_node_id = NodeId::from_public_key(&identity.public_key());
    if grant.consumer_node_id != consumer_node_id {
        anyhow::bail!("Relay 签名授权属于其他消费者身份");
    }
    let grant_public_key = grant
        .consumer_public_key
        .to_bytes()
        .map_err(anyhow::Error::new)?;
    if grant_public_key != identity.public_key_bytes() {
        anyhow::bail!("Relay 签名授权公钥与当前身份不一致");
    }
    grant.validate_at(now_unix_ms()).map_err(anyhow::Error::new)
}

fn member_grant_for_endpoint<'a>(
    endpoint: &RelayEndpoint,
    identity: &Identity,
    grants: &'a [RelayMemberGrant],
) -> Result<&'a RelayMemberGrant> {
    let provider_node_id = endpoint
        .provider_node_id
        .as_ref()
        .context("Relay endpoint missing provider identity")?;
    let consumer_node_id = NodeId::from_public_key(&identity.public_key());
    grants
        .iter()
        .find(|grant| {
            &grant.provider_node_id == provider_node_id
                && grant.consumer_node_id == consumer_node_id
        })
        .context("Relay endpoint has no matching signed member grant")
}

fn now_unix_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod incoming_task_tests {
    use super::*;

    #[tokio::test]
    async fn relay_incoming_task_stop_joins_and_releases_owned_resources() {
        let resource = Arc::new(());
        let weak = Arc::downgrade(&resource);
        let task = tokio::spawn(async move {
            let _resource = resource;
            std::future::pending::<()>().await;
        });
        let owner = Arc::new(IncomingTask(Mutex::new(Some(task))));
        let clone = owner.clone();
        owner.stop().await;
        clone.stop().await;
        assert!(weak.upgrade().is_none());
        assert!(owner.0.lock().await.is_none());
    }
}
