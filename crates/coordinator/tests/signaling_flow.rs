use anyhow::{Context, Result};
use fuckipv6_common::{codec, Identity, NatType, SignalingMessage};
use fuckipv6_coordinator::signaling::server::SignalingServer;
use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::Message};

#[tokio::test]
async fn websocket_registration_and_relay_flow() -> Result<()> {
    let bind_addr = free_loopback_addr().await?;
    let server = SignalingServer::new(bind_addr.to_string(), "fd00::1000/120")?;
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

    server_task.abort();
    Ok(())
}

#[tokio::test]
async fn websocket_duplicate_registration_refreshes_nat_mapping() -> Result<()> {
    let bind_addr = free_loopback_addr().await?;
    let server = SignalingServer::new(bind_addr.to_string(), "fd00::1000/120")?;
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
    Ok(())
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
