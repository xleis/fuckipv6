use anyhow::{Context, Result};
use ed25519_dalek::VerifyingKey;
use fuckipv6_common::{
    build_pinned_quic_client_config, build_pinned_quic_client_config_with_auth,
    build_quic_server_config, generate_self_signed_quic_certificate, recv_quic_frame,
    send_quic_frame, write_quic_frame, Identity, QuicCertificate, QuicFrame, ZeroRttSelfTestReport,
};
use fuckipv6_secure_session::handshake::identity_id;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Instant;

use super::admission::{AdmissionIdentity, HandshakeAdmission, HandshakeAdmissionMetrics};

pub fn build_server_config(certificate: &QuicCertificate) -> Result<quinn::ServerConfig> {
    build_quic_server_config(certificate)
}

pub fn build_client_config(expected_server_pin: [u8; 32]) -> Result<quinn::ClientConfig> {
    build_pinned_quic_client_config(expected_server_pin)
}

pub fn generate_node_certificate() -> Result<QuicCertificate> {
    generate_self_signed_quic_certificate(vec!["fuckipv6.local".to_string()])
}

fn build_authenticated_client_config(
    expected_server_pin: [u8; 32],
    certificate: &QuicCertificate,
) -> Result<quinn::ClientConfig> {
    build_pinned_quic_client_config_with_auth(expected_server_pin, certificate)
}

fn generate_client_certificate() -> Result<QuicCertificate> {
    QuicCertificate::from_identity(&Identity::generate()?)
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
    let client_certificate = generate_client_certificate()?;

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
        client.set_default_client_config(build_authenticated_client_config(
            certificate.public_key_pin,
            &client_certificate,
        )?);
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
use tokio::sync::{mpsc, Mutex, RwLock};

type PersistentPeerSendStream = Arc<Mutex<Option<quinn::SendStream>>>;

/// 端到端直连 P2P QUIC 网格引擎
#[derive(Clone)]
pub struct QuicP2pMesh {
    endpoint: quinn::Endpoint,
    certificate: QuicCertificate,
    client_certificate: QuicCertificate,
    active_peers: Arc<RwLock<HashMap<Ipv6Addr, quinn::Connection>>>,
    active_streams: Arc<RwLock<HashMap<Ipv6Addr, PersistentPeerSendStream>>>,
    #[cfg(test)]
    outbound_streams_opened: Arc<std::sync::atomic::AtomicU64>,
    handshake_admission: Arc<HandshakeAdmission>,
}

impl QuicP2pMesh {
    pub fn new(bind_addr: SocketAddr) -> Result<Self> {
        Self::new_with_admission(bind_addr, HandshakeAdmission::shared_default())
    }

    /// Build a QUIC mesh that shares handshake capacity with the UDP runtime.
    pub(crate) fn new_with_admission(
        bind_addr: SocketAddr,
        handshake_admission: Arc<HandshakeAdmission>,
    ) -> Result<Self> {
        let certificate = generate_node_certificate()?;
        let client_certificate = generate_client_certificate()?;
        let server_config = build_server_config(&certificate)?;
        let endpoint = quinn::Endpoint::server(server_config, bind_addr)
            .context("无法创建 QUIC P2P 直连端点")?;
        Ok(Self {
            endpoint,
            certificate,
            client_certificate,
            active_peers: Arc::new(RwLock::new(HashMap::new())),
            active_streams: Arc::new(RwLock::new(HashMap::new())),
            #[cfg(test)]
            outbound_streams_opened: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            handshake_admission,
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.endpoint.local_addr().context("无法获取本地 QUIC 地址")
    }

    pub fn certificate(&self) -> &QuicCertificate {
        &self.certificate
    }

    /// Snapshot admission pressure without exposing certificate or ticket data.
    pub fn handshake_admission_metrics(&self) -> HandshakeAdmissionMetrics {
        self.handshake_admission.metrics(Instant::now())
    }

    pub async fn connect_peer(
        &self,
        peer_ip: Ipv6Addr,
        remote_addr: SocketAddr,
        expected_pin: [u8; 32],
    ) -> Result<quinn::Connection> {
        let client_config =
            build_authenticated_client_config(expected_pin, &self.client_certificate)?;
        let mut client_endpoint = self.endpoint.clone();
        client_endpoint.set_default_client_config(client_config);

        let connection = client_endpoint
            .connect(remote_addr, "fuckipv6.local")
            .with_context(|| format!("发起 P2P QUIC 直连失败: {remote_addr}"))?
            .await
            .with_context(|| format!("P2P QUIC 握手失败: {remote_addr}"))?;

        self.active_streams.write().await.remove(&peer_ip);
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

        // Data packets share one bidirectional stream per peer. Keeping the
        // stream open avoids a stream setup round-trip for every media frame
        // and, more importantly, gives control-plane takeovers a stable data
        // path that is independent of signaling session replacement.
        let stream_slot = {
            let mut streams = self.active_streams.write().await;
            streams
                .entry(*peer_ip)
                .or_insert_with(|| Arc::new(Mutex::new(None)))
                .clone()
        };
        let mut stream = stream_slot.lock().await;
        if stream.is_none() {
            let (send, _recv) = connection
                .open_bi()
                .await
                .context("无法打开 P2P QUIC 发送流")?;
            #[cfg(test)]
            self.outbound_streams_opened
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            *stream = Some(send);
        }

        let frame = QuicFrame {
            stream_id: 1,
            payload: payload.to_vec(),
        };
        let Some(send) = stream.as_mut() else {
            anyhow::bail!("P2P QUIC 发送流状态无效");
        };
        if let Err(error) = write_quic_frame(send, &frame).await {
            *stream = None;
            return Err(error);
        }
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
        let admission = self.handshake_admission.clone();
        tokio::spawn(async move {
            while let Some(incoming) = endpoint.accept().await {
                let source = incoming.remote_address().ip();
                let now = Instant::now();
                let mut permit = match admission.try_reserve_source(source, now) {
                    Ok(permit) => permit,
                    Err(error) => {
                        let metrics = admission.metrics(now);
                        tracing::warn!(
                            %source,
                            %error,
                            active = metrics.active_reservations,
                            rejected = metrics.rejected_total(),
                            "QUIC 握手准入已拒绝"
                        );
                        incoming.refuse();
                        continue;
                    }
                };
                let tx = incoming_tx.clone();
                let task_admission = admission.clone();
                tokio::spawn(async move {
                    let Ok(connection) = incoming.await else {
                        return;
                    };
                    let identity = match authenticated_peer_identity(&connection) {
                        Ok(identity) => identity,
                        Err(error) => {
                            connection.close(0u32.into(), b"invalid peer identity");
                            tracing::warn!(%source, %error, "QUIC 客户端身份提取失败");
                            return;
                        }
                    };
                    if let Err(error) = permit.bind_identity(identity, Instant::now()) {
                        let metrics = task_admission.metrics(Instant::now());
                        connection.close(0u32.into(), b"handshake admission rejected");
                        tracing::warn!(
                            %source,
                            %error,
                            active = metrics.active_reservations,
                            rejected = metrics.rejected_total(),
                            "QUIC 客户端身份准入已拒绝"
                        );
                        return;
                    }
                    drop(permit);

                    loop {
                        let Ok((mut send, mut recv)) = connection.accept_bi().await else {
                            break;
                        };
                        let tx = tx.clone();
                        tokio::spawn(async move {
                            while let Ok(frame) = recv_frame(&mut recv).await {
                                let src_ip = default_peer_ip.unwrap_or(Ipv6Addr::UNSPECIFIED);
                                if tx.send((src_ip, frame.payload)).is_err() {
                                    break;
                                }
                            }
                            let _ = send.finish();
                        });
                    }
                });
            }
        })
    }
}

fn authenticated_peer_identity(connection: &quinn::Connection) -> Result<AdmissionIdentity> {
    let peer_identity = connection
        .peer_identity()
        .context("QUIC 连接缺少客户端证书身份")?;
    let certificates = peer_identity
        .downcast::<Vec<quinn::rustls::pki_types::CertificateDer<'static>>>()
        .map_err(|_| anyhow::anyhow!("QUIC 客户端证书身份类型无效"))?;
    if certificates.len() != 1 {
        anyhow::bail!(
            "QUIC 客户端证书链必须恰好包含一个终端证书，实际为 {}",
            certificates.len()
        );
    }
    let public_key = fuckipv6_common::ed25519_public_key_from_certificate(&certificates[0])
        .context("无法从 QUIC 客户端证书提取 Ed25519 公钥")?;
    let verifying_key = VerifyingKey::from_bytes(&public_key)
        .context("QUIC 客户端证书公钥不是有效 Ed25519 身份")?;
    Ok(identity_id(&verifying_key))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authenticated_client_config(server_pin: [u8; 32]) -> Result<quinn::ClientConfig> {
        let client_certificate = generate_client_certificate()?;
        build_authenticated_client_config(server_pin, &client_certificate)
    }

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
        client.set_default_client_config(authenticated_client_config(cert.public_key_pin)?);

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
        client.set_default_client_config(authenticated_client_config(other_cert.public_key_pin)?);

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
        client.set_default_client_config(authenticated_client_config(cert.public_key_pin)?);
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
    async fn zero_rtt_reconnect_self_test_falls_back_to_one_rtt() -> Result<()> {
        let cert = generate_node_certificate()?;
        let report = run_zero_rtt_reconnect_self_test(&cert).await?;
        assert!(!report.accepted);
        assert_eq!(report.echoed_payload, b"one-rtt-fallback");
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
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
        loop {
            let admission = node_a.handshake_admission_metrics();
            if admission.admitted == 1 && admission.active_reservations == 0 {
                assert_eq!(admission.rejected_total(), 0);
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "admission permit was not released"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

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

    const W14_TAKEOVER_LEASE_MS: u64 = 5_000;

    struct W14QuicCoordinatorFixture {
        dir: std::path::PathBuf,
        network: fuckipv6_common::domain::NetworkRecord,
        network_path: std::path::PathBuf,
        owner: Identity,
        candidates: [Identity; 2],
        config: fuckipv6_common::domain::CoordinatorCandidateConfig,
    }

    impl W14QuicCoordinatorFixture {
        fn new(label: &str, now_unix_ms: u64) -> Result<Self> {
            use fuckipv6_common::domain::{
                CapabilityKind, CoordinatorCandidate, CoordinatorCandidateConfig,
                ExitContributionMode, ExitContributionObligation, ExitContributionScope,
                NetworkCreateProfile, NetworkJoinMode, NetworkRecord, NetworkVisibility, NodeId,
                PublicKey,
            };
            use std::sync::atomic::{AtomicU64, Ordering};

            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "fuckipv6-w14-quic-{}-{label}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir)?;

            let owner = Identity::generate()?;
            let candidates = [Identity::generate()?, Identity::generate()?];
            let owner_id = NodeId::from_public_key(&owner.public_key());
            let candidate_ids = candidates
                .iter()
                .map(|identity| NodeId::from_public_key(&identity.public_key()))
                .collect::<Vec<_>>();
            let mut network = NetworkRecord::create_owned(
                NetworkCreateProfile {
                    name: "w14-quic-takeover".into(),
                    purpose: "persistent QUIC stream across a real Coordinator term handoff".into(),
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
                    exit_selection_policy: Default::default(),
                },
                PublicKey::from_bytes([91; 32]),
                owner_id,
                now_unix_ms.saturating_sub(10_000),
            )?;
            network.members.extend(candidate_ids.clone());
            network.validate()?;

            let config = CoordinatorCandidateConfig::signed(
                network.network_id.clone(),
                1,
                vec![
                    CoordinatorCandidate {
                        node_id: candidate_ids[0].clone(),
                        priority: 1,
                    },
                    CoordinatorCandidate {
                        node_id: candidate_ids[1].clone(),
                        priority: 2,
                    },
                ],
                now_unix_ms.saturating_sub(1_000),
                now_unix_ms.saturating_add(10 * 60 * 1000),
                &owner,
            )?;
            let network_path = dir.join("network.json");
            std::fs::write(&network_path, serde_json::to_vec_pretty(&network)?)?;

            Ok(Self {
                dir,
                network,
                network_path,
                owner,
                candidates,
                config,
            })
        }

        fn store(
            &self,
        ) -> Result<
            fuckipv6_coordinator::signaling::coordinator_candidates::CoordinatorCandidateStore,
        > {
            fuckipv6_coordinator::signaling::coordinator_candidates::CoordinatorCandidateStore::new(
                self.dir.join("coordinator_candidates.json"),
                &self.network_path,
            )
        }

        fn candidate_id(&self, index: usize) -> fuckipv6_common::domain::NodeId {
            fuckipv6_common::domain::NodeId::from_public_key(&self.candidates[index].public_key())
        }

        fn owner_id(&self) -> fuckipv6_common::domain::NodeId {
            fuckipv6_common::domain::NodeId::from_public_key(&self.owner.public_key())
        }

        fn bootstrap(
            &self,
            store: &fuckipv6_coordinator::signaling::coordinator_candidates::CoordinatorCandidateStore,
            now_unix_ms: u64,
        ) -> Result<()> {
            use fuckipv6_common::domain::{
                CoordinatorCandidateDeclaration, CoordinatorCandidateResources,
                CoordinatorCandidateWillingness, CoordinatorPublicReachability,
            };

            store.update_config(&self.owner_id(), 0, self.config.clone(), now_unix_ms)?;
            for (index, identity) in self.candidates.iter().enumerate() {
                let declaration = CoordinatorCandidateDeclaration::signed(
                    self.network.network_id.clone(),
                    self.config.revision,
                    CoordinatorPublicReachability {
                        public_endpoint: Some(format!("127.0.0.1:{}", 41_000 + index).parse()?),
                        reachable_via_relay: true,
                    },
                    CoordinatorCandidateResources {
                        cpu_cores: 4,
                        memory_bytes: 512 * 1024 * 1024,
                        max_connections: 10_000,
                    },
                    CoordinatorCandidateWillingness::Willing,
                    now_unix_ms,
                    now_unix_ms.saturating_add(10 * 60 * 1000),
                    identity,
                )?;
                store.publish_declaration(&self.candidate_id(index), declaration, now_unix_ms)?;
            }
            Ok(())
        }

        fn install_election(
            &self,
            store: &fuckipv6_coordinator::signaling::coordinator_candidates::CoordinatorCandidateStore,
            term: u64,
            leader: usize,
            voters: &[usize],
            now_unix_ms: u64,
        ) -> Result<fuckipv6_common::domain::CoordinatorCandidateStatus> {
            use fuckipv6_common::domain::{CoordinatorElectionProposal, CoordinatorElectionVote};

            let candidate_group = (0..self.candidates.len())
                .map(|index| self.candidate_id(index))
                .collect::<Vec<_>>();
            let proposal = CoordinatorElectionProposal::signed(
                self.network.network_id.clone(),
                self.config.revision,
                term,
                candidate_group.clone(),
                now_unix_ms,
                now_unix_ms.saturating_add(W14_TAKEOVER_LEASE_MS),
                &self.candidates[leader],
            )?;
            let proposal_id = proposal.proposal_id.clone();
            let mut status =
                store.propose_election(&self.candidate_id(leader), proposal, now_unix_ms)?;
            for voter in voters {
                let vote = CoordinatorElectionVote::signed(
                    self.network.network_id.clone(),
                    self.config.revision,
                    term,
                    self.candidate_id(leader),
                    candidate_group.clone(),
                    now_unix_ms,
                    now_unix_ms.saturating_add(W14_TAKEOVER_LEASE_MS),
                    &self.candidates[*voter],
                )?;
                status = store.record_election_vote(
                    &self.candidate_id(*voter),
                    &proposal_id,
                    vote,
                    now_unix_ms,
                )?;
            }
            Ok(status)
        }
    }

    impl Drop for W14QuicCoordinatorFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[tokio::test]
    async fn quic_p2p_mesh_long_lived_stream_spans_real_coordinator_term_takeover() -> Result<()> {
        use fuckipv6_common::domain::CoordinatorControlPlaneState;

        let now_unix_ms = 2_000_000u64;
        let takeover_unix_ms = now_unix_ms + W14_TAKEOVER_LEASE_MS + 1;
        let fixture = W14QuicCoordinatorFixture::new("real-term-takeover", now_unix_ms)?;
        let store = fixture.store()?;
        fixture.bootstrap(&store, now_unix_ms)?;
        let term_one = fixture.install_election(&store, 1, 0, &[0, 1], now_unix_ms)?;
        let term_one = term_one.latest_term.context("term 1 was not installed")?;
        assert_eq!(term_one.term, 1);
        assert_eq!(term_one.fencing_token.0, 1);
        assert_eq!(term_one.leader, fixture.candidate_id(0));

        let node_a = QuicP2pMesh::new(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;
        let node_b = QuicP2pMesh::new(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;

        let addr_a = node_a.local_addr()?;
        let pin_a = node_a.certificate().public_key_pin;
        let ip_a: Ipv6Addr = "fd00::11".parse().unwrap();
        let ip_b: Ipv6Addr = "fd00::12".parse().unwrap();

        let (rx_a_tx, mut rx_a_rx) = mpsc::unbounded_channel();
        let listener = node_a.spawn_incoming_listener(rx_a_tx, Some(ip_b));
        let connection = node_b.connect_peer(ip_a, addr_a, pin_a).await?;
        let connection_id = connection.stable_id();

        const FRAME_COUNT: u64 = 96;
        let mut term_two = None;
        let sender = async {
            for sequence in 0..FRAME_COUNT {
                if sequence == 32 {
                    let status =
                        fixture.install_election(&store, 2, 1, &[1, 0], takeover_unix_ms)?;
                    term_two = status.latest_term;
                }

                let mut frame = vec![0x5a; 1_000 + (sequence as usize % 5) * 100];
                frame[..8].copy_from_slice(&sequence.to_be_bytes());
                node_b.send_packet(&ip_a, &frame).await?;
                tokio::time::sleep(std::time::Duration::from_millis(3)).await;
            }
            Ok::<(), anyhow::Error>(())
        };

        let receiver = async {
            for expected in 0..FRAME_COUNT {
                let (source, frame) =
                    tokio::time::timeout(std::time::Duration::from_secs(3), rx_a_rx.recv())
                        .await
                        .context("long-lived QUIC frame timed out")?
                        .context("long-lived QUIC stream closed before all frames arrived")?;
                assert_eq!(source, ip_b);
                assert_eq!(
                    u64::from_be_bytes(frame[..8].try_into().expect("frame sequence")),
                    expected
                );
            }
            Ok::<(), anyhow::Error>(())
        };

        let (sender_result, receiver_result) = tokio::join!(sender, receiver);
        sender_result?;
        receiver_result?;

        let term_two = term_two.context("term 2 was not installed during media transfer")?;
        assert_eq!(term_two.term, 2);
        assert_eq!(term_two.fencing_token.0, 2);
        assert_eq!(term_two.leader, fixture.candidate_id(1));
        let authoritative = store.status(
            &fixture.candidate_id(0),
            &fixture.network.network_id,
            takeover_unix_ms,
        )?;
        assert_eq!(
            authoritative.control_plane.state,
            CoordinatorControlPlaneState::Available
        );
        assert_eq!(
            authoritative
                .latest_election
                .as_ref()
                .map(|election| election.stage),
            Some(fuckipv6_common::domain::CoordinatorElectionStage::Committed)
        );

        assert_eq!(
            node_b
                .outbound_streams_opened
                .load(std::sync::atomic::Ordering::Relaxed),
            1,
            "all media-sized packets must use one persistent QUIC stream"
        );
        assert_eq!(connection.stable_id(), connection_id);
        assert!(node_b.has_peer(&ip_a).await);
        assert!(connection.close_reason().is_none());

        connection.close(0u32.into(), b"done");
        listener.abort();
        Ok(())
    }
}
