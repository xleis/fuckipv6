use anyhow::{Context, Result};
use fuckipv6_common::domain::{
    AdminRecoveryApproval, AdminRecoveryProposal, AdminRecoveryReason, AdminRecoveryRequest,
    AdminRecoveryRevocationPlan, AdminRecoveryState, AdminRecoveryTarget, CapabilityKind,
    ExitContributionMode, ExitContributionObligation, ExitContributionScope, ExitSelectionPolicy,
    NetworkCreateProfile, NetworkJoinMode, NetworkRecord, NetworkVisibility, NodeId, PublicKey,
};
use fuckipv6_common::{codec, CoordinatorConfig, Identity, NatType, SignalingMessage};
use fuckipv6_coordinator::signaling::admin_recovery::AdminRecoveryStore;
use fuckipv6_coordinator::signaling::blacklist::CoordinatorConfigBlacklistStore;
use fuckipv6_coordinator::signaling::invitations::InvitationStore;
use fuckipv6_coordinator::signaling::join_requests::JoinRequestStore;
use fuckipv6_coordinator::signaling::server::SignalingServer;
use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio_tungstenite::{connect_async, tungstenite::Message};

type TestSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn websocket_admin_recovery_ack_precedes_old_session_close() -> Result<()> {
    let dir = test_dir("ack-before-close")?;
    let owner = Identity::generate()?;
    let admin = Identity::generate()?;
    let pending_applicant = Identity::generate()?;
    let replacement = Identity::generate()?;
    let owner_id = node_id(&owner);
    let admin_id = node_id(&admin);
    let replacement_id = node_id(&replacement);

    let mut network = NetworkRecord::create_owned(
        NetworkCreateProfile {
            name: "w14-recovery-flow".into(),
            purpose: "verify the administrator recovery WebSocket protocol".into(),
            icon: "network".into(),
            language: "zh-CN".into(),
            region: "ZZ".into(),
            tags: vec![],
            capability_declarations: vec![CapabilityKind::File],
            visibility: NetworkVisibility::Private,
            join_mode: NetworkJoinMode::ApprovalRequired,
            exit_contribution: ExitContributionObligation::default(),
            exit_contribution_scope: ExitContributionScope::default(),
            exit_contribution_mode: ExitContributionMode::default(),
            exit_selection_policy: ExitSelectionPolicy::default(),
        },
        PublicKey::from_bytes([91; 32]),
        owner_id.clone(),
        unix_time_ms().saturating_sub(10_000),
    )?;
    network.administrators = vec![admin_id.clone()];
    network.members.push(admin_id.clone());
    network.validate()?;
    let network_path = dir.join("network.json");
    std::fs::write(&network_path, serde_json::to_vec_pretty(&network)?)?;

    let recovery_store = Arc::new(AdminRecoveryStore::new(
        dir.join("admin_recovery.json"),
        &network_path,
    )?);
    let invitation_store = Arc::new(InvitationStore::new(
        dir.join("invitations.json"),
        Some(network_path.clone()),
        None,
        vec![],
    )?);
    let join_request_store = Arc::new(JoinRequestStore::new(
        dir.join("join_requests.json"),
        Some(network_path.clone()),
        None,
        vec![],
    )?);

    let config_path = dir.join("coordinator.toml");
    CoordinatorConfig::default().save_to_file(&config_path)?;
    let server = SignalingServer::new("127.0.0.1:0".into(), "fd00::1000/120")?
        .with_network_record_path(&network_path)
        .with_invitation_store(invitation_store)
        .with_join_request_store(join_request_store)
        .with_admin_recovery_store(recovery_store.clone())
        .bind()
        .await?;
    let bind_addr = server.local_addr()?;
    let session_manager = server.session_manager();
    session_manager.attach_data_plane_membership(&network_path)?;
    session_manager
        .attach_blacklist_store(Arc::new(CoordinatorConfigBlacklistStore::new(config_path)))
        .await?;
    let server_task = tokio::spawn(server.run());

    let mut owner_ws = connect_with_retry(bind_addr).await?;
    register(&mut owner_ws, &owner, 20_101).await?;
    let mut admin_ws = connect_with_retry(bind_addr).await?;
    register(&mut admin_ws, &admin, 20_102).await?;
    let mut applicant_ws = connect_with_retry(bind_addr).await?;
    register(&mut applicant_ws, &pending_applicant, 20_103).await?;

    let now = unix_time_ms();
    let request = recovery_request(
        &network,
        &owner,
        &admin,
        replacement_id.clone(),
        now.saturating_sub(1_000),
        now + 60_000,
        "websocket-admin-recovery",
    )?;
    let request_id = request.proposal.request_id.clone();
    send_msg(
        &mut owner_ws,
        SignalingMessage::AdminRecoverySubmit {
            request: Box::new(request.clone()),
        },
    )
    .await?;
    let submitted = recv_until(&mut owner_ws, |message| {
        matches!(message, SignalingMessage::AdminRecoverySubmitAck { .. })
    })
    .await
    .context("waiting for admin recovery submit acknowledgement")?;
    let SignalingMessage::AdminRecoverySubmitAck { status } = submitted else {
        unreachable!();
    };
    assert_eq!(status.request_id, request_id);
    assert!(status.can_execute);

    send_msg(
        &mut owner_ws,
        SignalingMessage::AdminRecoveryExecute {
            network_id: network.network_id.clone(),
            request_id: request_id.clone(),
        },
    )
    .await?;

    // The executing owner is also part of the old roster snapshot. Its Ack
    // must be queued before the recovery-triggered Close frame.
    let execute_ack = recv_until(&mut owner_ws, |message| {
        matches!(message, SignalingMessage::AdminRecoveryExecuteAck { .. })
    })
    .await
    .context("waiting for admin recovery execute acknowledgement")?;
    let SignalingMessage::AdminRecoveryExecuteAck { status } = execute_ack else {
        unreachable!();
    };
    assert_eq!(status.state, AdminRecoveryState::Executed);
    assert_eq!(status.request_id, request_id);
    wait_for_close(&mut owner_ws).await?;
    wait_for_close(&mut admin_ws).await?;

    let persisted: NetworkRecord = serde_json::from_slice(&std::fs::read(&network_path)?)?;
    assert_eq!(persisted.owner, replacement_id);
    assert_eq!(persisted.revision, network.revision + 1);
    assert!(persisted.administrators.is_empty());
    assert_eq!(
        recovery_store.status(&request_id, now + 2_000)?.state,
        AdminRecoveryState::Executed
    );
    assert!(recovery_store
        .audit_entries()?
        .iter()
        .any(|entry| { entry.action == "admin_recovery.invalidations_applied" }));

    // A signalling-only pending applicant was not in the approved member
    // snapshot and must survive the recovery. This catches over-broad session
    // invalidation even when the old authority sessions are correctly closed.
    assert!(session_manager
        .get_peer_info(&pending_applicant.client_id().to_string())
        .await
        .is_some());
    send_msg(
        &mut applicant_ws,
        SignalingMessage::AdminRecoveryList {
            network_id: network.network_id.clone(),
        },
    )
    .await?;
    let visible = recv_until(&mut applicant_ws, |message| {
        matches!(message, SignalingMessage::AdminRecoveryListAck { .. })
    })
    .await?;
    let SignalingMessage::AdminRecoveryListAck { requests } = visible else {
        unreachable!();
    };
    assert!(requests.is_empty());

    server_task.abort();
    std::fs::remove_dir_all(dir)?;
    Ok(())
}

fn recovery_request(
    network: &NetworkRecord,
    owner: &Identity,
    admin: &Identity,
    replacement: NodeId,
    issued_at_unix_ms: u64,
    expires_at_unix_ms: u64,
    nonce: &str,
) -> Result<AdminRecoveryRequest> {
    let target = AdminRecoveryTarget::replace_authorities(network, replacement, Vec::new());
    let proposal = AdminRecoveryProposal::new(
        network,
        target,
        node_id(owner),
        AdminRecoveryReason::DisasterRecovery,
        nonce,
        issued_at_unix_ms,
        expires_at_unix_ms,
        AdminRecoveryRevocationPlan::default(),
    )
    .context("build admin recovery proposal")?;
    let approvals = vec![
        AdminRecoveryApproval::signed(&proposal, owner, issued_at_unix_ms + 100)
            .context("sign recovery approval as owner")?,
        AdminRecoveryApproval::signed(&proposal, admin, issued_at_unix_ms + 200)
            .context("sign recovery approval as administrator")?,
    ];
    Ok(AdminRecoveryRequest::with_approvals(proposal, approvals))
}

async fn register(ws: &mut TestSocket, identity: &Identity, p2p_port: u16) -> Result<()> {
    let client_id = identity.client_id().to_string();
    let public_key = identity.public_key();
    let timestamp = 123_456;
    let challenge =
        SignalingMessage::register_challenge(&client_id, &public_key, p2p_port, timestamp);
    let signature = identity.sign(&challenge).to_bytes().to_vec();
    send_msg(
        ws,
        SignalingMessage::register(
            client_id,
            public_key,
            p2p_port,
            NatType::Restricted,
            timestamp,
            signature,
        ),
    )
    .await?;
    let _ = recv_until(ws, |message| {
        matches!(message, SignalingMessage::RegisterAck { .. })
    })
    .await?;
    Ok(())
}

async fn recv_until(
    ws: &mut TestSocket,
    mut predicate: impl FnMut(&SignalingMessage) -> bool,
) -> Result<SignalingMessage> {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let message = ws
                .next()
                .await
                .context("WebSocket closed before expected message")??;
            let Message::Binary(data) = message else {
                if matches!(message, Message::Close(_)) {
                    anyhow::bail!("WebSocket sent Close before the expected response");
                }
                continue;
            };
            let decoded = codec::decode(&data).context("decode signaling message")?;
            if predicate(&decoded) {
                return Ok(decoded);
            }
        }
    })
    .await
    .context("timed out waiting for signaling message")?
}

async fn wait_for_close(ws: &mut TestSocket) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Close(_))) | None => return Ok(()),
                Some(Ok(_)) => {}
                Some(Err(error)) => return Err(anyhow::Error::from(error)),
            }
        }
    })
    .await
    .context("timed out waiting for recovery session close")?
}

async fn send_msg(ws: &mut TestSocket, message: SignalingMessage) -> Result<()> {
    ws.send(Message::Binary(codec::encode(&message)?.into()))
        .await?;
    Ok(())
}

async fn connect_with_retry(bind_addr: SocketAddr) -> Result<TestSocket> {
    let url = format!("ws://{bind_addr}");
    let mut last_error = None;
    for _ in 0..20 {
        match connect_async(&url).await {
            Ok((ws, _)) => return Ok(ws),
            Err(error) => {
                last_error = Some(error);
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
    Err(last_error
        .map(anyhow::Error::from)
        .unwrap_or_else(|| anyhow::anyhow!("failed to connect to test Coordinator")))
}

fn node_id(identity: &Identity) -> NodeId {
    NodeId::from_public_key(&identity.public_key())
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn test_dir(label: &str) -> Result<PathBuf> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "fuckipv6-w14-admin-recovery-{}-{label}-{unique}-{sequence}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path)?;
    Ok(path)
}
