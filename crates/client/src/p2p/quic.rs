use anyhow::{Context, Result};
use fuckipv6_common::{
    build_pinned_quic_client_config, build_quic_server_config,
    generate_self_signed_quic_certificate, recv_quic_frame, send_quic_frame, QuicCertificate,
    QuicFrame, ZeroRttSelfTestReport,
};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

pub fn build_server_config(certificate: &QuicCertificate) -> Result<quinn::ServerConfig> {
    build_quic_server_config(certificate)
}

pub fn build_client_config(expected_server_pin: [u8; 32]) -> Result<quinn::ClientConfig> {
    build_pinned_quic_client_config(expected_server_pin)
}

pub fn generate_node_certificate() -> Result<QuicCertificate> {
    generate_self_signed_quic_certificate(vec!["fuckipv6.local".to_string()])
}

pub async fn send_frame(stream: &mut quinn::SendStream, frame: &QuicFrame) -> Result<()> {
    send_quic_frame(stream, frame).await
}

pub async fn recv_frame(stream: &mut quinn::RecvStream) -> Result<QuicFrame> {
    recv_quic_frame(stream).await
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuicSelfTestReport {
    pub server_addr: SocketAddr,
    pub echoed: QuicFrame,
}

pub async fn run_loopback_self_test(certificate: &QuicCertificate) -> Result<QuicSelfTestReport> {
    let server_config = build_server_config(certificate)?;
    let server = quinn::Endpoint::server(
        server_config,
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
    )
    .context("无法启动 QUIC loopback 服务端")?;
    let server_addr = server
        .local_addr()
        .context("无法读取 QUIC loopback 服务端地址")?;

    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

    let server_future = async move {
        let incoming = server
            .accept()
            .await
            .ok_or_else(|| anyhow::anyhow!("QUIC loopback 服务端未收到连接"))?;
        let connection = incoming.await.context("QUIC loopback 服务端握手失败")?;
        let (mut send, mut recv) = connection
            .accept_bi()
            .await
            .context("QUIC loopback 服务端无法接受双向流")?;
        let frame = recv_frame(&mut recv).await?;
        send_frame(&mut send, &frame).await?;
        let _ = done_rx.await;
        Ok::<(), anyhow::Error>(())
    };

    let client_future = async {
        let mut client =
            quinn::Endpoint::client(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
                .context("无法启动 QUIC loopback 客户端")?;
        client.set_default_client_config(build_client_config(certificate.public_key_pin)?);
        let connection = client
            .connect(server_addr, "fuckipv6.local")
            .context("无法发起 QUIC loopback 连接")?
            .await
            .context("QUIC loopback 客户端握手失败")?;

        let (mut send, mut recv) = connection
            .open_bi()
            .await
            .context("QUIC loopback 客户端无法打开双向流")?;
        let frame = QuicFrame {
            stream_id: 0,
            payload: b"quic-self-test".to_vec(),
        };
        send_frame(&mut send, &frame).await?;
        let echoed = recv_frame(&mut recv).await?;
        let _ = done_tx.send(());
        connection.close(0u32.into(), b"done");
        client.wait_idle().await;
        Ok::<QuicFrame, anyhow::Error>(echoed)
    };

    let (server_result, client_result) = tokio::join!(server_future, client_future);
    server_result?;
    let echoed = client_result?;

    Ok(QuicSelfTestReport {
        server_addr,
        echoed,
    })
}

pub async fn run_zero_rtt_reconnect_self_test(
    certificate: &QuicCertificate,
) -> Result<ZeroRttSelfTestReport> {
    fuckipv6_common::run_quic_zero_rtt_reconnect_self_test(certificate).await
}

use std::collections::HashMap;
use std::net::Ipv6Addr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

/// 端到端直连 P2P QUIC 网格引擎
#[derive(Clone)]
pub struct QuicP2pMesh {
    endpoint: quinn::Endpoint,
    certificate: QuicCertificate,
    active_peers: Arc<RwLock<HashMap<Ipv6Addr, quinn::Connection>>>,
}

impl QuicP2pMesh {
    pub fn new(bind_addr: SocketAddr) -> Result<Self> {
        let certificate = generate_node_certificate()?;
        let server_config = build_server_config(&certificate)?;
        let endpoint = quinn::Endpoint::server(server_config, bind_addr)
            .context("无法创建 QUIC P2P 直连端点")?;
        Ok(Self {
            endpoint,
            certificate,
            active_peers: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.endpoint.local_addr().context("无法获取本地 QUIC 地址")
    }

    pub fn certificate(&self) -> &QuicCertificate {
        &self.certificate
    }

    pub async fn connect_peer(
        &self,
        peer_ip: Ipv6Addr,
        remote_addr: SocketAddr,
        expected_pin: [u8; 32],
    ) -> Result<quinn::Connection> {
        let client_config = build_client_config(expected_pin)?;
        let mut client_endpoint = self.endpoint.clone();
        client_endpoint.set_default_client_config(client_config);

        let connection = client_endpoint
            .connect(remote_addr, "fuckipv6.local")
            .with_context(|| format!("发起 P2P QUIC 直连失败: {remote_addr}"))?
            .await
            .with_context(|| format!("P2P QUIC 握手失败: {remote_addr}"))?;

        self.active_peers
            .write()
            .await
            .insert(peer_ip, connection.clone());
        tracing::info!("P2P QUIC 直连已建立: {} -> {}", peer_ip, remote_addr);
        Ok(connection)
    }

    pub async fn send_packet(&self, peer_ip: &Ipv6Addr, payload: &[u8]) -> Result<()> {
        let conn = {
            let peers = self.active_peers.read().await;
            peers.get(peer_ip).cloned()
        };

        let Some(connection) = conn else {
            anyhow::bail!("No active QUIC P2P connection to {}", peer_ip);
        };

        let (mut send, _recv) = connection
            .open_bi()
            .await
            .context("无法打开 P2P QUIC 发送流")?;

        let frame = QuicFrame {
            stream_id: 1,
            payload: payload.to_vec(),
        };
        send_frame(&mut send, &frame).await?;
        Ok(())
    }

    pub async fn has_peer(&self, peer_ip: &Ipv6Addr) -> bool {
        let peers = self.active_peers.read().await;
        if let Some(conn) = peers.get(peer_ip) {
            conn.close_reason().is_none()
        } else {
            false
        }
    }

    pub fn spawn_incoming_listener(
        &self,
        incoming_tx: mpsc::UnboundedSender<(Ipv6Addr, Vec<u8>)>,
        default_peer_ip: Option<Ipv6Addr>,
    ) -> tokio::task::JoinHandle<()> {
        let endpoint = self.endpoint.clone();
        tokio::spawn(async move {
            while let Some(incoming) = endpoint.accept().await {
                let tx = incoming_tx.clone();
                tokio::spawn(async move {
                    let Ok(connection) = incoming.await else {
                        return;
                    };
                    loop {
                        let Ok((mut send, mut recv)) = connection.accept_bi().await else {
                            break;
                        };
                        let Ok(frame) = recv_frame(&mut recv).await else {
                            break;
                        };
                        let _ = send.finish();
                        let src_ip = default_peer_ip.unwrap_or(Ipv6Addr::UNSPECIFIED);
                        let _ = tx.send((src_ip, frame.payload));
                    }
                });
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pinned_quic_connection_accepts_matching_certificate() -> Result<()> {
        let cert = generate_node_certificate()?;
        let server_config = build_server_config(&cert)?;
        let server = quinn::Endpoint::server(
            server_config,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        )?;
        let server_addr = server.local_addr()?;

        let server_task = tokio::spawn(async move {
            let incoming = server.accept().await.expect("server should accept client");
            incoming.await.expect("client handshake should complete");
        });

        let mut client =
            quinn::Endpoint::client(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;
        client.set_default_client_config(build_client_config(cert.public_key_pin)?);

        let connection = client
            .connect(server_addr, "fuckipv6.local")
            .context("failed to start QUIC connect")?
            .await
            .context("failed to complete QUIC connect")?;
        connection.close(0u32.into(), b"done");
        client.wait_idle().await;
        server_task.await?;
        Ok(())
    }

    #[tokio::test]
    async fn pinned_quic_connection_rejects_wrong_certificate() -> Result<()> {
        let cert = generate_node_certificate()?;
        let other_cert = generate_node_certificate()?;
        let server_config = build_server_config(&cert)?;
        let server = quinn::Endpoint::server(
            server_config,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        )?;
        let server_addr = server.local_addr()?;

        let server_task = tokio::spawn(async move {
            if let Some(incoming) = server.accept().await {
                let _ = incoming.await;
            }
        });

        let mut client =
            quinn::Endpoint::client(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;
        client.set_default_client_config(build_client_config(other_cert.public_key_pin)?);

        let result = client
            .connect(server_addr, "fuckipv6.local")
            .context("failed to start QUIC connect")?
            .await;
        assert!(result.is_err());
        client.wait_idle().await;
        server_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn quic_bidirectional_streams_are_multiplexed() -> Result<()> {
        let cert = generate_node_certificate()?;
        let server_config = build_server_config(&cert)?;
        let server = quinn::Endpoint::server(
            server_config,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        )?;
        let server_addr = server.local_addr()?;

        let server_task = tokio::spawn(async move {
            let incoming = server.accept().await.expect("server should accept client");
            let connection = incoming.await?;
            let mut tasks = Vec::new();
            for _ in 0..4 {
                let (mut send, mut recv) = connection.accept_bi().await?;
                tasks.push(tokio::spawn(async move {
                    let frame = recv_frame(&mut recv).await?;
                    send_frame(&mut send, &frame).await?;
                    Ok::<(), anyhow::Error>(())
                }));
            }

            for task in tasks {
                task.await??;
            }
            Ok::<(), anyhow::Error>(())
        });

        let mut client =
            quinn::Endpoint::client(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;
        client.set_default_client_config(build_client_config(cert.public_key_pin)?);
        let connection = client
            .connect(server_addr, "fuckipv6.local")
            .context("failed to start QUIC connect")?
            .await
            .context("failed to complete QUIC connect")?;

        let mut tasks = Vec::new();
        for index in 0..4u64 {
            let connection = connection.clone();
            tasks.push(tokio::spawn(async move {
                let (mut send, mut recv) = connection.open_bi().await?;
                let frame = QuicFrame {
                    stream_id: index,
                    payload: format!("payload-{index}").into_bytes(),
                };
                send_frame(&mut send, &frame).await?;
                let echoed = recv_frame(&mut recv).await?;
                assert_eq!(echoed, frame);
                Ok::<(), anyhow::Error>(())
            }));
        }

        for task in tasks {
            task.await??;
        }

        connection.close(0u32.into(), b"done");
        client.wait_idle().await;
        server_task.await??;
        Ok(())
    }

    #[tokio::test]
    async fn loopback_self_test_echoes_frame() -> Result<()> {
        let cert = generate_node_certificate()?;
        let report = run_loopback_self_test(&cert).await?;
        assert_eq!(report.echoed.payload, b"quic-self-test");
        assert!(report.server_addr.port() > 0);
        Ok(())
    }

    #[tokio::test]
    async fn zero_rtt_reconnect_self_test_accepts_early_data() -> Result<()> {
        let cert = generate_node_certificate()?;
        let report = run_zero_rtt_reconnect_self_test(&cert).await?;
        assert!(report.accepted);
        assert_eq!(report.echoed_payload, b"zero-rtt");
        Ok(())
    }

    #[tokio::test]
    async fn quic_p2p_mesh_direct_connection_and_data_transfer() -> Result<()> {
        let node_a = QuicP2pMesh::new(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;
        let node_b = QuicP2pMesh::new(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;

        let addr_a = node_a.local_addr()?;
        let pin_a = node_a.certificate().public_key_pin;

        let ip_a: Ipv6Addr = "fd00::1".parse().unwrap();
        let ip_b: Ipv6Addr = "fd00::2".parse().unwrap();

        let (rx_a_tx, mut rx_a_rx) = mpsc::unbounded_channel();
        let _listener = node_a.spawn_incoming_listener(rx_a_tx, Some(ip_b));

        // Node B 发起 P2P 直连握手连接到 Node A
        let _conn = node_b.connect_peer(ip_a, addr_a, pin_a).await?;
        assert!(node_b.has_peer(&ip_a).await);

        // Node B 发送数据
        node_b.send_packet(&ip_a, b"quic-p2p-mesh-packet").await?;

        // Node A 接收数据
        let (src_ip, payload) =
            tokio::time::timeout(std::time::Duration::from_secs(2), rx_a_rx.recv())
                .await?
                .expect("Node A should receive packet");
        assert_eq!(src_ip, ip_b);
        assert_eq!(payload, b"quic-p2p-mesh-packet");
        Ok(())
    }
}
