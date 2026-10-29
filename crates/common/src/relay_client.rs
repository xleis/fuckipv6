use anyhow::{Context, Result};
use std::net::{Ipv6Addr, SocketAddr};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::crypto::Identity;
use crate::protocol::RelayEndpoint;
use crate::quic::{build_pinned_quic_client_config, recv_relay_message, send_relay_message, RelayMessage};

const RELAY_REGISTER_STREAM_ID: u64 = 0;

#[derive(Clone)]
pub struct QuicRelayClient {
    endpoint: quinn::Endpoint,
    connection: quinn::Connection,
    incoming_rx: Arc<RwLock<mpsc::UnboundedReceiver<RelayPacket>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayPacket {
    pub src_ip: Ipv6Addr,
    pub dst_ip: Ipv6Addr,
    pub payload: Vec<u8>,
}

impl QuicRelayClient {
    pub async fn connect(
        relay_addr: SocketAddr,
        expected_server_pin: [u8; 32],
        server_name: &str,
        identity: &Identity,
        virtual_ip: Ipv6Addr,
    ) -> Result<Self> {
        let bind_addr = if relay_addr.is_ipv6() {
            SocketAddr::new(std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 0)
        } else {
            SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0)
        };
        let mut endpoint =
            quinn::Endpoint::client(bind_addr).context("无法创建 QUIC relay 客户端 endpoint")?;
        endpoint.set_default_client_config(build_pinned_quic_client_config(expected_server_pin)?);

        let connection = endpoint
            .connect(relay_addr, server_name)
            .with_context(|| format!("无法连接 QUIC relay: {relay_addr}"))?
            .await
            .context("QUIC relay 握手失败")?;

        register_relay_connection(&connection, identity, virtual_ip).await?;
        let (incoming_tx, incoming_rx) = mpsc::unbounded_channel();
        spawn_incoming_task(connection.clone(), incoming_tx);

        Ok(Self {
            endpoint,
            connection,
            incoming_rx: Arc::new(RwLock::new(incoming_rx)),
        })
    }

    /// 从多个候选 QUIC 中继节点中并发探测并选择延迟最低的最佳中继
    pub async fn connect_optimal(
        relays: &[RelayEndpoint],
        identity: &Identity,
        virtual_ip: Ipv6Addr,
    ) -> Result<Self> {
        if relays.is_empty() {
            anyhow::bail!("No candidate relay endpoints provided");
        }
        if relays.len() == 1 {
            let ep = &relays[0];
            return Self::connect(ep.addr, ep.public_key_pin, &ep.server_name, identity, virtual_ip).await;
        }

        let mut tasks = tokio::task::JoinSet::new();
        for ep in relays.iter().cloned() {
            let id = identity.clone();
            tasks.spawn(async move {
                let start = std::time::Instant::now();
                match Self::connect(ep.addr, ep.public_key_pin, &ep.server_name, &id, virtual_ip).await {
                    Ok(client) => {
                        let rtt = client.connection.rtt();
                        let elapsed = start.elapsed();
                        let score = rtt.min(elapsed);
                        tracing::info!("QUIC 中继 {} ({}) 握手成功, 评分延迟: {:?}", ep.server_name, ep.addr, score);
                        Ok((client, score, ep))
                    }
                    Err(e) => {
                        tracing::debug!("QUIC 中继 {} ({}) 连接失败: {:?}", ep.server_name, ep.addr, e);
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
            tracing::info!("选定最佳 QUIC 中继节点: {} ({}), 延迟: {:?}", ep.server_name, ep.addr, score);
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
        payload: &[u8],
    ) -> Result<()> {
        let packet = RelayMessage::Packet {
            src_ip,
            dst_ip,
            payload: payload.to_vec(),
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
        self.incoming_rx.write().await.recv().await
    }

    pub async fn close(&self) {
        self.connection.close(0u32.into(), b"done");
        self.endpoint.close(0u32.into(), b"done");
    }
}

async fn register_relay_connection(
    connection: &quinn::Connection,
    identity: &Identity,
    virtual_ip: Ipv6Addr,
) -> Result<()> {
    let timestamp = now_secs();
    let public_key = identity.public_key();
    let challenge =
        RelayMessage::register_challenge(identity.client_id(), &virtual_ip, &public_key, timestamp);
    let signature = identity.sign(&challenge).to_bytes().to_vec();
    let register = RelayMessage::Register {
        client_id: identity.client_id().to_string(),
        virtual_ip,
        public_key,
        timestamp,
        signature,
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
    incoming_tx: mpsc::UnboundedSender<RelayPacket>,
) {
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
                        incoming_tx.send(RelayPacket {
                            src_ip,
                            dst_ip,
                            payload,
                        })?;
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
    });
}

fn now_secs() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
