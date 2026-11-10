use anyhow::{Context, Result};
use fuckipv6_common::domain::{
    ExitContributionMode, ExitContributionObligation, ExitContributionScope, NetworkCreateProfile,
    NetworkJoinMode, NetworkJoinPreviewRequest, NetworkRecord, NetworkVisibility, NodeId,
    PublicKey,
};
use fuckipv6_common::{codec, CoordinatorConfig, ErrorCode, Identity, NatType, SignalingMessage};
use fuckipv6_coordinator::signaling::blacklist::CoordinatorConfigBlacklistStore;
use fuckipv6_coordinator::signaling::server::SignalingServer;
use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::Message};

#[tokio::test]
async fn websocket_registration_and_relay_flow() -> Result<()> {
    let bind_addr = free_loopback_addr().await?;
    let (server, config_dir) = test_signaling_server(bind_addr).await?;
    let server_task = tokio::spawn(server.run());

    let url = format!("ws://{}", bind_addr);
    let client_a_identity = Identity::generate()?;
    let client_b_identity = Identity::generate()?;

    let mut ws_a = connect_with_retry(&url).await?;
    let register_a = register_message(&client_a_identity, 17101);
    send_msg(&mut ws_a, register_a).await?;

    let ack_a = recv_msg(&mut ws_a).await?;
    let SignalingMessage::RegisterAck {
        assigned_ip: ip_a,
        peers: peers_a,
        relay_endpoint,
        ..
    } = ack_a
    else {
        anyhow::bail!("expected register ack for client A");
    };
    assert!(peers_a.is_empty());
    assert!(relay_endpoint.is_none());

    let mut ws_b = connect_with_retry(&url).await?;
    let register_b = register_message(&client_b_identity, 17102);
    send_msg(&mut ws_b, register_b).await?;

    let ack_b = recv_msg(&mut ws_b).await?;
    let SignalingMessage::RegisterAck {
        assigned_ip: ip_b,
        peers: peers_b,
        ..
    } = ack_b
    else {
        anyhow::bail!("expected register ack for client B");
    };
    assert_eq!(peers_b.len(), 1);
    assert_eq!(peers_b[0].virtual_ip, ip_a);
    assert_eq!(peers_b[0].nat_type, NatType::Restricted);

    let online = recv_msg(&mut ws_a).await?;
    let SignalingMessage::PeerOnline { peer } = online else {
        anyhow::bail!("expected peer online broadcast");
    };
    assert_eq!(peer.virtual_ip, ip_b);
    assert_eq!(peer.nat_type, NatType::Restricted);

    let payload = vec![0x60, 0, 0, 0, 1, 2, 3, 4];
    send_msg(
        &mut ws_a,
        SignalingMessage::relay_packet(ip_a, ip_b, payload.clone()),
    )
    .await?;

    let relayed = recv_msg(&mut ws_b).await?;
    let SignalingMessage::RelayPacket {
        src_ip,
        dst_ip,
        payload: got_payload,
    } = relayed
    else {
        anyhow::bail!("expected relayed packet");
    };

    assert_eq!(src_ip, std::net::IpAddr::V6(ip_a));
    assert_eq!(dst_ip, std::net::IpAddr::V6(ip_b));
    assert_eq!(got_payload, payload);

    // A registered client must not be able to claim another peer's virtual IP.
    send_msg(
        &mut ws_a,
        SignalingMessage::relay_packet(ip_b, ip_a, vec![0xde, 0xad]),
    )
    .await?;
    let rejected = recv_msg(&mut ws_a).await?;
    match rejected {
        SignalingMessage::Error { code, .. } => {
            assert_eq!(code, fuckipv6_common::ErrorCode::Unauthorized.as_u16());
        }
        other => anyhow::bail!("expected spoofed relay rejection, got {other:?}"),
    }

    server_task.abort();
    std::fs::remove_dir_all(config_dir)?;
    Ok(())
}

#[tokio::test]
async fn websocket_duplicate_registration_refreshes_nat_mapping() -> Result<()> {
    let bind_addr = free_loopback_addr().await?;
    let (server, config_dir) = test_signaling_server(bind_addr).await?;
    let server_task = tokio::spawn(server.run());

    let url = format!("ws://{}", bind_addr);
    let client_a_identity = Identity::generate()?;
    let client_b_identity = Identity::generate()?;

    let mut ws_a_old = connect_with_retry(&url).await?;
    send_msg(
        &mut ws_a_old,
        register_message_with_nat(&client_a_identity, 17101, NatType::Restricted),
    )
    .await?;

    let SignalingMessage::RegisterAck {
        assigned_ip: ip_a,
        peers: peers_a,
        ..
    } = recv_msg(&mut ws_a_old).await?
    else {
        anyhow::bail!("expected first register ack for client A");
    };
    assert!(peers_a.is_empty());

    let mut ws_b = connect_with_retry(&url).await?;
    send_msg(
        &mut ws_b,
        register_message_with_nat(&client_b_identity, 17102, NatType::Open),
    )
    .await?;

    let SignalingMessage::RegisterAck { peers: peers_b, .. } = recv_msg(&mut ws_b).await? else {
        anyhow::bail!("expected register ack for client B");
    };
    assert_eq!(peers_b.len(), 1);
    assert_eq!(peers_b[0].virtual_ip, ip_a);
    assert_eq!(peers_b[0].endpoints[0].port(), 17101);
    assert_eq!(peers_b[0].nat_type, NatType::Restricted);

    let SignalingMessage::PeerOnline { peer: peer_b } = recv_msg(&mut ws_a_old).await? else {
        anyhow::bail!("expected peer online broadcast for B");
    };
    assert_eq!(peer_b.nat_type, NatType::Open);

    let mut ws_a_new = connect_with_retry(&url).await?;
    send_msg(
        &mut ws_a_new,
        register_message_with_nat(&client_a_identity, 19101, NatType::Symmetric),
    )
    .await?;

    let SignalingMessage::RegisterAck {
        assigned_ip: refreshed_ip_a,
        peers: refreshed_peers,
        ..
    } = recv_msg(&mut ws_a_new).await?
    else {
        anyhow::bail!("expected refreshed register ack for client A");
    };
    assert_eq!(refreshed_ip_a, ip_a);
    assert_eq!(refreshed_peers.len(), 1);
    assert_eq!(refreshed_peers[0].virtual_ip, peer_b.virtual_ip);

    let SignalingMessage::PeerOnline { peer: refreshed_a } = recv_msg(&mut ws_b).await? else {
        anyhow::bail!("expected refreshed peer online broadcast for A");
    };
    assert_eq!(refreshed_a.virtual_ip, ip_a);
    assert_eq!(refreshed_a.endpoints[0].port(), 19101);
    assert_eq!(refreshed_a.nat_type, NatType::Symmetric);

    ws_a_old.close(None).await?;

    let payload = vec![0x60, 0, 0, 0, 5, 6, 7, 8];
    send_msg(
        &mut ws_b,
        SignalingMessage::relay_packet(peer_b.virtual_ip, ip_a, payload.clone()),
    )
    .await?;

    let SignalingMessage::RelayPacket {
        src_ip,
        dst_ip,
        payload: got_payload,
    } = recv_msg(&mut ws_a_new).await?
    else {
        anyhow::bail!("expected relay packet on refreshed A connection");
    };

    assert_eq!(src_ip, std::net::IpAddr::V6(peer_b.virtual_ip));
    assert_eq!(dst_ip, std::net::IpAddr::V6(ip_a));
    assert_eq!(got_payload, payload);

    server_task.abort();
    std::fs::remove_dir_all(config_dir)?;
    Ok(())
}

#[tokio::test]
async fn websocket_join_preview_is_signed_and_rejects_ticket_mismatch() -> Result<()> {
    let bind_addr = free_loopback_addr().await?;
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let config_dir = std::env::temp_dir().join(format!(
        "fuckipv6-signaling-preview-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&config_dir)?;

    let coordinator_identity = Identity::generate()?;
    let owner = NodeId::from_public_key(&coordinator_identity.public_key());
    let network = NetworkRecord::create_owned(
        NetworkCreateProfile {
            name: "preview-network".into(),
            purpose: "Exercise the direct join disclosure contract".into(),
            icon: "home".into(),
            language: "zh-CN".into(),
            region: "CN".into(),
            tags: Vec::new(),
            capability_declarations: Vec::new(),
            visibility: NetworkVisibility::Unlisted,
            join_mode: NetworkJoinMode::InviteOnly,
            exit_contribution: ExitContributionObligation::NotRequired,
            exit_contribution_scope: ExitContributionScope::NotApplicable,
            exit_contribution_mode: ExitContributionMode::Disabled,
            exit_selection_policy: Default::default(),
        },
        PublicKey::from_bytes([41; 32]),
        owner,
        1,
    )?;
    let record_path = config_dir.join("network_record.json");
    std::fs::write(&record_path, serde_json::to_vec(&network)?)?;

    let config_path = config_dir.join("coordinator.toml");
    CoordinatorConfig::default().save_to_file(&config_path)?;
    let server = SignalingServer::new(bind_addr.to_string(), "fd00::1000/120")?
        .with_network_record_path(record_path)
        .with_node_identity(coordinator_identity.clone());
    server
        .session_manager()
        .attach_blacklist_store(Arc::new(CoordinatorConfigBlacklistStore::new(config_path)))
        .await?;
    let server_task = tokio::spawn(server.run());

    let client_identity = Identity::generate()?;
    let url = format!("ws://{}", bind_addr);
    let mut ws = connect_with_retry(&url).await?;
    send_msg(&mut ws, register_message(&client_identity, 18101)).await?;
    let _ = recv_msg(&mut ws).await?;

    let coordinator_public_key: [u8; 32] = coordinator_identity
        .public_key()
        .try_into()
        .map_err(|_| anyhow::anyhow!("coordinator public key must contain exactly 32 bytes"))?;
    send_msg(
        &mut ws,
        SignalingMessage::NetworkJoinPreview {
            request: NetworkJoinPreviewRequest {
                coordinator_public_key,
                network_name: network.name.clone(),
            },
        },
    )
    .await?;
    let SignalingMessage::NetworkJoinPreviewAck { preview } = recv_msg(&mut ws).await? else {
        anyhow::bail!("expected signed network join preview");
    };
    preview.validate_for_record(&network, &coordinator_public_key, unix_time_ms())?;

    send_msg(
        &mut ws,
        SignalingMessage::NetworkJoinPreview {
            request: NetworkJoinPreviewRequest {
                coordinator_public_key,
                network_name: "wrong-network".into(),
            },
        },
    )
    .await?;
    assert_error_code(&mut ws, ErrorCode::BadRequest).await?;

    let wrong_coordinator = Identity::generate()?;
    send_msg(
        &mut ws,
        SignalingMessage::NetworkJoinPreview {
            request: NetworkJoinPreviewRequest {
                coordinator_public_key: wrong_coordinator.public_key().try_into().map_err(
                    |_| anyhow::anyhow!("wrong coordinator public key must contain 32 bytes"),
                )?,
                network_name: network.name.clone(),
            },
        },
    )
    .await?;
    assert_error_code(&mut ws, ErrorCode::Forbidden).await?;

    server_task.abort();
    std::fs::remove_dir_all(config_dir)?;
    Ok(())
}

static TEST_CONFIG_COUNTER: AtomicU64 = AtomicU64::new(0);

async fn test_signaling_server(bind_addr: SocketAddr) -> Result<(SignalingServer, PathBuf)> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = TEST_CONFIG_COUNTER.fetch_add(1, Ordering::Relaxed);
    let config_dir = std::env::temp_dir().join(format!(
        "fuckipv6-signaling-flow-{}-{unique}-{sequence}",
        std::process::id()
    ));
    let config_path = config_dir.join("coordinator.toml");
    std::fs::create_dir_all(&config_dir)?;
    CoordinatorConfig::default().save_to_file(&config_path)?;
    let server = SignalingServer::new(bind_addr.to_string(), "fd00::1000/120")?;
    server
        .session_manager()
        .attach_blacklist_store(Arc::new(CoordinatorConfigBlacklistStore::new(config_path)))
        .await?;
    Ok((server, config_dir))
}

async fn free_loopback_addr() -> Result<SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    drop(listener);
    Ok(addr)
}

async fn connect_with_retry(
    url: &str,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
> {
    let mut last_error = None;

    for _ in 0..20 {
        match connect_async(url).await {
            Ok((ws, _)) => return Ok(ws),
            Err(e) => {
                last_error = Some(e);
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
    }

    Err(last_error
        .map(anyhow::Error::from)
        .unwrap_or_else(|| anyhow::anyhow!("failed to connect to websocket")))
}

fn register_message(identity: &Identity, p2p_port: u16) -> SignalingMessage {
    register_message_with_nat(identity, p2p_port, NatType::Restricted)
}

fn register_message_with_nat(
    identity: &Identity,
    p2p_port: u16,
    nat_type: NatType,
) -> SignalingMessage {
    let client_id = identity.client_id().to_string();
    let public_key = identity.public_key();
    let timestamp = 123_456;
    let challenge =
        SignalingMessage::register_challenge(&client_id, &public_key, p2p_port, timestamp);
    let signature = identity.sign(&challenge).to_bytes().to_vec();
    SignalingMessage::register(
        client_id, public_key, p2p_port, nat_type, timestamp, signature,
    )
}

async fn send_msg<S>(ws: &mut S, msg: SignalingMessage) -> Result<()>
where
    S: SinkExt<Message> + Unpin,
    <S as futures::Sink<Message>>::Error: std::error::Error + Send + Sync + 'static,
{
    ws.send(Message::Binary(codec::encode(&msg)?.into()))
        .await?;
    Ok(())
}

async fn recv_msg<S>(ws: &mut S) -> Result<SignalingMessage>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let message = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
        .await
        .context("timed out waiting for websocket message")?
        .context("websocket closed")??;

    let Message::Binary(data) = message else {
        anyhow::bail!("expected binary websocket message");
    };

    codec::decode(&data).context("failed to deserialize signaling message")
}

async fn assert_error_code<S>(ws: &mut S, expected: ErrorCode) -> Result<()>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let SignalingMessage::Error { code, .. } = recv_msg(ws).await? else {
        anyhow::bail!("expected signaling error");
    };
    assert_eq!(code, expected.as_u16());
    Ok(())
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
