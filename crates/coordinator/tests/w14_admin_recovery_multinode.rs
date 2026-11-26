//! W14 real multi-node administrator-recovery drill.
//!
//! The drill exercises the acceptance matrix of
//! `docs/W14_ADMIN_RECOVERY_DESIGN.zh-CN.md` with real processes instead of
//! in-process fixtures:
//!
//! * the Coordinator runs in its own child process, started up to three times
//!   from the same durable store (control-plane interruption and restart);
//! * every offline Trustee is a separate child process with its own private
//!   key directory; the process only ever sees the exported request bundle and
//!   writes one detached approval, so one device can contribute one approval;
//! * the recovery operator only holds approvals collected from the Trustees,
//!   the surviving node identity and the exported bundle: no administrator
//!   private key is available;
//! * old authority sessions, a pending applicant and old invitations exist
//!   before the disaster and are checked after the roster rotation;
//! * the privileged variant additionally runs the Coordinator inside a network
//!   namespace and uses nftables to partition it from the operator, proving the
//!   client never infers success from a lost acknowledgement.

use anyhow::{Context, Result};
use fuckipv6_common::domain::{
    AdminRecoveryApproval, AdminRecoveryImpact, AdminRecoveryProposal, AdminRecoveryReason,
    AdminRecoveryRequest, AdminRecoveryRevocationPlan, AdminRecoveryState, AdminRecoveryTarget,
    CapabilityKind, ExitContributionMode, ExitContributionObligation, ExitContributionScope,
    ExitSelectionPolicy, InvitationCreateRequest, MembershipRole, NetworkCreateProfile,
    NetworkJoinMode, NetworkRecord, NetworkVisibility, NodeId, PublicKey,
};
use fuckipv6_common::{codec, CoordinatorConfig, Identity, NatType, SignalingMessage};
use fuckipv6_coordinator::signaling::admin_recovery::AdminRecoveryStore;
use fuckipv6_coordinator::signaling::blacklist::CoordinatorConfigBlacklistStore;
use fuckipv6_coordinator::signaling::invitations::InvitationStore;
use fuckipv6_coordinator::signaling::join_requests::JoinRequestStore;
use fuckipv6_coordinator::signaling::server::SignalingServer;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio_tungstenite::{connect_async, tungstenite::Message};

type TestSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

const CHILD_ENV: &str = "FUCKIPV6_W14_ADMIN_RECOVERY_CHILD";
const JOB_ENV: &str = "FUCKIPV6_W14_ADMIN_RECOVERY_JOB";
const REPORT_ENV: &str = "FUCKIPV6_W14_ADMIN_RECOVERY_REPORT";
const NETNS_ENV: &str = "FUCKIPV6_W14_ADMIN_RECOVERY_NETNS";
const HOST_IP_ENV: &str = "FUCKIPV6_W14_ADMIN_RECOVERY_HOST_IP";
const NF_TABLE: &str = "w14recovery";
const SCHEMA: &str = "fuckipv6.w14.admin-recovery.drill.v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ChildJob {
    /// One offline Trustee device: sign the exported request bundle.
    SignApproval {
        device_dir: PathBuf,
        request_path: PathBuf,
        approval_path: PathBuf,
    },
    /// The Coordinator process: real signaling server over a durable store.
    ServeCoordinator {
        root: PathBuf,
        bind: String,
        ready_path: PathBuf,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CoordinatorReady {
    pid: u32,
    port: u16,
}

#[derive(Debug, Serialize)]
struct DrillReport {
    schema: String,
    mode: String,
    trustee_devices: usize,
    trustee_threshold: u32,
    approvals_collected: usize,
    approvals_required: u32,
    single_approval_rejected: String,
    admin_signature_trusted_alone: usize,
    coordinator_restarts: usize,
    unreachable_while_down: bool,
    durable_pending_after_restart: bool,
    executed_state: String,
    previous_revision: u64,
    new_revision: u64,
    new_owner_matches: bool,
    administrators_cleared: bool,
    closed_old_sessions: Vec<String>,
    pending_applicant_survived: bool,
    revoked_invitations: usize,
    idempotent_reexecute: bool,
    audit_actions: Vec<String>,
    partition_enforced: bool,
    no_ack_while_partitioned: bool,
}

#[derive(Debug, Clone, Default)]
struct DrillOptions {
    netns: Option<String>,
    host_ip: Option<String>,
}

// ---------------------------------------------------------------------------
// Parent-side drill
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn w14_admin_recovery_multinode_offline_trustee_drill() -> Result<()> {
    let report = run_drill(DrillOptions::default()).await?;
    assert_drill_report(&report, false);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires root, a prepared network namespace, and nftables"]
async fn w14_admin_recovery_netns_drill() -> Result<()> {
    let netns = std::env::var(NETNS_ENV).context("missing FUCKIPV6_W14_ADMIN_RECOVERY_NETNS")?;
    let host_ip =
        std::env::var(HOST_IP_ENV).context("missing FUCKIPV6_W14_ADMIN_RECOVERY_HOST_IP")?;
    let report = run_drill(DrillOptions {
        netns: Some(netns),
        host_ip: Some(host_ip),
    })
    .await?;
    assert_drill_report(&report, true);
    Ok(())
}

async fn run_drill(options: DrillOptions) -> Result<DrillReport> {
    let mode = if options.netns.is_some() {
        "multinode-netns-offline-trustee"
    } else {
        "multinode-multiprocess-offline-trustee"
    };
    let root = test_dir(mode)?;
    let survivor_dir = root.join("surviving-node");
    let operator_dir = root.join("operator");
    let medium_dir = root.join("removable-medium");
    let destroyed_dir = root.join("destroyed-administrator-devices");
    let trustee_dirs: Vec<PathBuf> = (0..3)
        .map(|index| root.join(format!("trustee-{index}")))
        .collect();
    for dir in std::iter::once(&survivor_dir)
        .chain(std::iter::once(&operator_dir))
        .chain(std::iter::once(&medium_dir))
        .chain(std::iter::once(&destroyed_dir))
        .chain(trustee_dirs.iter())
    {
        std::fs::create_dir_all(dir)?;
    }
    let coordinator_root = root.join("coordinator");
    std::fs::create_dir_all(&coordinator_root)?;

    // One surviving node keeps the owner identity on local disk: local system
    // access can act as the owner, but per the design it is not an approval.
    let owner = Identity::generate()?;
    owner.save_to_file(survivor_dir.join("identity.key"))?;
    owner.save_to_file(operator_dir.join("owner-identity.key"))?;
    // The administrator devices are lost. Their identities only exist for the
    // pre-disaster sessions the Coordinator still carries in memory.
    let admin = Identity::generate()?;
    let stranger = Identity::generate()?;
    let replacement = Identity::generate()?;
    let replacement_id = node_id(&replacement);
    let owner_id = node_id(&owner);
    let admin_id = node_id(&admin);
    let replacement_public = PublicKey::from_bytes(replacement.public_key_bytes());

    // Offline Trustees: each private key is written only into its own device
    // directory, and only the public key enters the network record.
    let mut trustees = Vec::new();
    for dir in &trustee_dirs {
        let identity = Identity::generate()?;
        identity.save_to_file(dir.join("identity.key"))?;
        trustees.push(identity);
    }
    let trustee_public_keys: Vec<PublicKey> = trustees
        .iter()
        .map(|identity| PublicKey::from_bytes(identity.public_key_bytes()))
        .collect();

    let now = unix_time_ms();
    let mut network = NetworkRecord::create_owned(
        NetworkCreateProfile {
            name: "w14-recovery-drill".into(),
            purpose: "real multi-node administrator threshold recovery drill".into(),
            icon: "network".into(),
            language: "zh-CN".into(),
            region: "ZZ".into(),
            tags: vec![],
            capability_declarations: vec![CapabilityKind::File],
            visibility: NetworkVisibility::Private,
            join_mode: NetworkJoinMode::InviteOnly,
            exit_contribution: ExitContributionObligation::default(),
            exit_contribution_scope: ExitContributionScope::default(),
            exit_contribution_mode: ExitContributionMode::default(),
            exit_selection_policy: ExitSelectionPolicy::default(),
        },
        PublicKey::from_bytes([91; 32]),
        owner_id.clone(),
        now.saturating_sub(10_000),
    )?;
    network.administrators = vec![admin_id.clone()];
    network.members.push(admin_id.clone());
    network.recovery_authorities = trustee_public_keys;
    network.recovery_threshold = 2;
    network.validate()?;
    let network_path = coordinator_root.join("network.json");
    std::fs::write(&network_path, serde_json::to_vec_pretty(&network)?)?;

    // An invitation exists before the disaster so recovery has something real
    // to invalidate.
    let invitation_store = InvitationStore::new(
        coordinator_root.join("invitations.json"),
        Some(network_path.clone()),
        None,
        vec![],
    )?;
    let invitation = invitation_store.create(
        &network.network_id,
        &owner_id,
        InvitationCreateRequest::new(
            network.network_id.clone(),
            MembershipRole::Member,
            vec![],
            1,
            600_000,
        ),
        now,
    )?;
    let invitation_id = invitation.status.invitation.invitation_id.clone();

    // Disaster state: only the exported bundle, the replacement owner identity
    // and the surviving node identity remain usable.
    std::fs::write(
        destroyed_dir.join("LOST-DEVICES.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "administrators_destroyed": [admin_id.to_string()],
            "replacement_owner_public_key": replacement_public.to_bytes()?,
            "note": "administrator private keys are unavailable to the recovery operator",
        }))?,
    )?;

    let issued_at = unix_time_ms().saturating_sub(1_000);
    let expires_at = issued_at + 600_000;
    let target =
        AdminRecoveryTarget::replace_authorities(&network, replacement_id.clone(), Vec::new());
    let impact = AdminRecoveryImpact::from_network(&network, &target);
    assert_eq!(impact.owner_before, owner_id);
    assert_eq!(impact.owner_after, replacement_id);
    assert_eq!(impact.administrators_removed.len(), 1);
    let proposal = AdminRecoveryProposal::new(
        &network,
        target,
        owner_id.clone(),
        AdminRecoveryReason::DisasterRecovery,
        "w14-multinode-drill",
        issued_at,
        expires_at,
        AdminRecoveryRevocationPlan::default(),
    )?;
    let request_id = proposal.request_id.clone();
    let exported = AdminRecoveryRequest::new(proposal.clone());
    let request_path = medium_dir.join("recovery-request.json");
    std::fs::write(&request_path, serde_json::to_vec_pretty(&exported)?)?;

    // Offline Trustee devices sign independently in their own processes.
    let mut approval_paths = Vec::new();
    let mut trustee_children = Vec::new();
    for (index, dir) in trustee_dirs.iter().enumerate() {
        let approval_path = medium_dir.join(format!("approval-{index}.json"));
        let job = ChildJob::SignApproval {
            device_dir: dir.clone(),
            request_path: request_path.clone(),
            approval_path: approval_path.clone(),
        };
        trustee_children.push(spawn_child(
            &job,
            None,
            &root.join(format!("trustee-{index}.log")),
        )?);
        approval_paths.push(approval_path);
    }
    for child in &mut trustee_children {
        let status = child.wait()?;
        assert!(status.success(), "offline trustee signing process failed");
    }
    let mut approvals = Vec::new();
    for path in &approval_paths {
        let approval: AdminRecoveryApproval = serde_json::from_slice(&std::fs::read(path)?)
            .with_context(|| format!("cannot parse trustee approval: {}", path.display()))?;
        approvals.push(approval);
    }
    assert_eq!(approvals.len(), 3);
    let single = AdminRecoveryRequest::with_approvals(proposal.clone(), vec![approvals[0].clone()]);
    let single_error = single
        .verify_for_network(&network, unix_time_ms())
        .expect_err("one trustee approval must not satisfy the threshold")
        .code()
        .to_string();
    let collected: Vec<AdminRecoveryApproval> = approvals[..2].to_vec();
    let request = AdminRecoveryRequest::with_approvals(proposal.clone(), collected.clone());
    assert_eq!(request.verify_for_network(&network, unix_time_ms())?, 2);
    assert_eq!(request.trusted_approval_count(&network), 2);
    // A surviving administrator identity is not part of the trustee policy and
    // cannot be mixed in to relax it.
    let mixed = AdminRecoveryRequest::with_approvals(
        proposal.clone(),
        vec![
            AdminRecoveryApproval::signed(&proposal, &admin, issued_at + 300)?,
            collected[0].clone(),
        ],
    );
    let admin_signature_trusted_alone = mixed.trusted_approval_count(&network) as usize;

    // --- Coordinator process #1 -------------------------------------------
    let bind = if options.netns.is_some() {
        "0.0.0.0:0".to_string()
    } else {
        "127.0.0.1:0".to_string()
    };
    let mut restarts = 0usize;
    let mut coordinator =
        spawn_coordinator(&coordinator_root, &bind, options.netns.as_deref(), 0, &root)?;
    let mut endpoint = coordinator_endpoint(&root, &options, 0)?;
    let mut partition_enforced = false;
    let mut no_ack_while_partitioned = false;
    {
        // Pre-disaster sessions: the old roster is still authoritative and the
        // administrator device is still connected when the drill starts.
        let mut pre_disaster_owner = connect_and_register(&endpoint, &owner, 21_101).await?;
        let mut pre_disaster_admin = connect_and_register(&endpoint, &admin, 21_102).await?;
        let mut pre_disaster_applicant = connect_and_register(&endpoint, &stranger, 21_103).await?;

        // Baseline before any disaster traffic: authorities and a non-member
        // applicant both see no recovery request on the real Coordinator.
        for (label, ws) in [
            ("administrator", &mut pre_disaster_admin),
            ("applicant", &mut pre_disaster_applicant),
        ] {
            let listed = request_list(ws, &network).await?;
            assert!(
                listed.is_empty(),
                "no recovery request may exist before the disaster drill submits one ({label})"
            );
        }

        // Real Coordinator fail-closed check: a request backed by one surviving
        // administrator and one trustee is rejected by the live service with a
        // stable public key instead of a generic internal error.
        let rejected = submit_expect_error(&mut pre_disaster_owner, &mixed, "threshold").await?;
        assert!(
            rejected.contains("threshold_not_met"),
            "unexpected submit rejection: {rejected}"
        );

        // Privileged variant: partition the Coordinator from the operator and
        // prove no acknowledgement is inferred while the transport is down.
        if let Some(netns) = options.netns.as_deref() {
            apply_partition(
                netns,
                options.host_ip.as_deref().context("missing host ip")?,
            )?;
            partition_enforced = true;
            send_msg(
                &mut pre_disaster_owner,
                SignalingMessage::AdminRecoveryList {
                    network_id: network.network_id.clone(),
                },
            )
            .await
            .ok();
            no_ack_while_partitioned =
                expect_no_ack(&mut pre_disaster_owner, Duration::from_secs(3)).await;
            restore_partition(netns)?;
            let ack = recv_until(&mut pre_disaster_owner, |message| {
                matches!(message, SignalingMessage::AdminRecoveryListAck { .. })
            })
            .await;
            if ack.is_err() {
                // The interrupted connection may have been reset; a fresh
                // connection proves the control plane recovered.
                let mut recovered = connect_and_register(&endpoint, &owner, 21_111).await?;
                assert!(request_list(&mut recovered, &network).await?.is_empty());
            }
        }
    }

    // --- First control-plane interruption ---------------------------------
    coordinator.kill_and_wait()?;
    restarts += 1;
    let mut unreachable_while_down = false;
    let url = format!("ws://{endpoint}");
    for _ in 0..5 {
        if connect_async(&url).await.is_err() {
            unreachable_while_down = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        unreachable_while_down,
        "the drill needs an observable control-plane outage"
    );

    coordinator = spawn_coordinator(
        &coordinator_root,
        &bind,
        options.netns.as_deref(),
        restarts,
        &root,
    )?;
    endpoint = coordinator_endpoint(&root, &options, restarts)?;

    // The approved request is submitted, observed by a second authority, then
    // the Coordinator is killed again: the request must stay durable and
    // executable, and no success may be inferred while it is down.
    let mut submit_ws = connect_and_register(&endpoint, &owner, 21_201).await?;
    send_msg(
        &mut submit_ws,
        SignalingMessage::AdminRecoverySubmit {
            request: Box::new(request.clone()),
        },
    )
    .await?;
    let submitted = recv_until(&mut submit_ws, |message| {
        matches!(message, SignalingMessage::AdminRecoverySubmitAck { .. })
    })
    .await
    .context("waiting for recovery submit acknowledgement")?;
    let SignalingMessage::AdminRecoverySubmitAck { status } = submitted else {
        anyhow::bail!("unexpected submit acknowledgement shape");
    };
    assert_eq!(status.request_id, request_id);
    assert_eq!(status.state, AdminRecoveryState::Pending);
    assert!(status.can_execute);
    assert_eq!(status.required_signatures, 2);
    assert_eq!(status.signature_count, 2);

    let mut observing_admin = connect_and_register(&endpoint, &admin, 21_202).await?;
    let observed = request_list(&mut observing_admin, &network).await?;
    assert_eq!(
        observed.len(),
        1,
        "pending request must be visible to authorities"
    );
    assert_eq!(observed[0].state, AdminRecoveryState::Pending);

    coordinator.kill_and_wait()?;
    restarts += 1;
    coordinator = spawn_coordinator(
        &coordinator_root,
        &bind,
        options.netns.as_deref(),
        restarts,
        &root,
    )?;
    endpoint = coordinator_endpoint(&root, &options, restarts)?;
    let mut owner_ws = connect_and_register(&endpoint, &owner, 21_301).await?;
    let mut admin_ws = connect_and_register(&endpoint, &admin, 21_302).await?;
    let mut applicant_ws = connect_and_register(&endpoint, &stranger, 21_303).await?;

    send_msg(
        &mut owner_ws,
        SignalingMessage::AdminRecoveryStatus {
            network_id: network.network_id.clone(),
            request_id: request_id.clone(),
        },
    )
    .await?;
    let status = recv_until(&mut owner_ws, |message| {
        matches!(message, SignalingMessage::AdminRecoveryStatusAck { .. })
    })
    .await?;
    let SignalingMessage::AdminRecoveryStatusAck { status } = status else {
        anyhow::bail!("unexpected status acknowledgement shape");
    };
    let durable_pending_after_restart =
        status.state == AdminRecoveryState::Pending && status.can_execute;

    // --- Execute: Ack before Close for the executor ------------------------
    send_msg(
        &mut owner_ws,
        SignalingMessage::AdminRecoveryExecute {
            network_id: network.network_id.clone(),
            request_id: request_id.clone(),
        },
    )
    .await?;
    let executed = recv_until(&mut owner_ws, |message| {
        matches!(message, SignalingMessage::AdminRecoveryExecuteAck { .. })
    })
    .await
    .context("waiting for recovery execute acknowledgement")?;
    let SignalingMessage::AdminRecoveryExecuteAck { status } = executed else {
        anyhow::bail!("unexpected execute acknowledgement shape");
    };
    assert_eq!(status.state, AdminRecoveryState::Executed);
    assert_eq!(status.request_id, request_id);
    wait_for_close(&mut owner_ws).await?;
    wait_for_close(&mut admin_ws).await?;
    let mut closed_old_sessions = vec![admin_id.to_string()];

    // A pending applicant outside the old member snapshot keeps its signaling
    // connection: over-broad invalidation would close it too.
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
    .await
    .context("pending applicant must keep a working signaling connection")?;
    let SignalingMessage::AdminRecoveryListAck { requests } = visible else {
        anyhow::bail!("unexpected list acknowledgement shape");
    };
    let pending_applicant_survived = requests.is_empty();

    let rotated: NetworkRecord = serde_json::from_slice(&std::fs::read(&network_path)?)?;
    let new_owner_matches = rotated.owner == replacement_id;
    let administrators_cleared = rotated.administrators.is_empty();
    assert!(new_owner_matches, "recovery did not install the new owner");
    assert_eq!(rotated.revision, network.revision + 1);

    let store = AdminRecoveryStore::new(
        coordinator_root.join("admin_recovery.json"),
        network_path.clone(),
    )?;
    let audit_actions: Vec<String> = store
        .audit_entries()?
        .into_iter()
        .map(|entry| entry.action)
        .collect();
    let revoked_invitations = invalidation_count(&store, "revoked_invitations")?;
    let invitations_after = invitation_store
        .list(&network.network_id, &replacement_id, unix_time_ms())
        .context("the new owner must be able to list invitations after recovery")?;
    let invitation_revoked = invitations_after
        .iter()
        .find(|view| view.invitation.invitation_id == invitation_id)
        .map(|view| view.invitation.revoked_at_unix_ms.is_some())
        .unwrap_or(true);
    assert!(
        invitation_revoked,
        "recovery left a pre-disaster invitation active"
    );

    // --- New authority, idempotent re-execution ----------------------------
    let mut replacement_ws = connect_and_register(&endpoint, &replacement, 21_501).await?;
    send_msg(
        &mut replacement_ws,
        SignalingMessage::AdminRecoveryStatus {
            network_id: network.network_id.clone(),
            request_id: request_id.clone(),
        },
    )
    .await?;
    let status = recv_until(&mut replacement_ws, |message| {
        matches!(message, SignalingMessage::AdminRecoveryStatusAck { .. })
    })
    .await?;
    let SignalingMessage::AdminRecoveryStatusAck { status } = status else {
        anyhow::bail!("unexpected status acknowledgement shape");
    };
    assert_eq!(status.state, AdminRecoveryState::Executed);
    send_msg(
        &mut replacement_ws,
        SignalingMessage::AdminRecoveryExecute {
            network_id: network.network_id.clone(),
            request_id: request_id.clone(),
        },
    )
    .await?;
    let replayed = recv_until(&mut replacement_ws, |message| {
        matches!(message, SignalingMessage::AdminRecoveryExecuteAck { .. })
    })
    .await
    .context("waiting for idempotent re-execution acknowledgement")?;
    let SignalingMessage::AdminRecoveryExecuteAck { status } = replayed else {
        anyhow::bail!("unexpected execute acknowledgement shape");
    };
    assert_eq!(status.state, AdminRecoveryState::Executed);
    let after_replay: NetworkRecord = serde_json::from_slice(&std::fs::read(&network_path)?)?;
    let invalidation_entries = store
        .audit_entries()?
        .into_iter()
        .filter(|entry| entry.action == "admin_recovery.invalidations_applied")
        .count();
    let idempotent_reexecute =
        after_replay.revision == rotated.revision && invalidation_entries == 1;
    wait_for_close(&mut replacement_ws).await?;

    closed_old_sessions.push(owner_id.to_string());

    let report = DrillReport {
        schema: SCHEMA.into(),
        mode: mode.into(),
        trustee_devices: trustee_dirs.len(),
        trustee_threshold: network.recovery_threshold,
        approvals_collected: collected.len(),
        approvals_required: 2,
        single_approval_rejected: single_error,
        admin_signature_trusted_alone,
        coordinator_restarts: restarts,
        unreachable_while_down,
        durable_pending_after_restart,
        executed_state: format!("{:?}", status.state),
        previous_revision: network.revision,
        new_revision: after_replay.revision,
        new_owner_matches,
        administrators_cleared,
        closed_old_sessions,
        pending_applicant_survived,
        revoked_invitations,
        idempotent_reexecute,
        audit_actions,
        partition_enforced,
        no_ack_while_partitioned,
    };

    if let Some(path) = std::env::var_os(REPORT_ENV) {
        std::fs::write(path, serde_json::to_vec_pretty(&report)?)?;
    }

    coordinator.kill_and_wait()?;
    Ok(report)
}

fn assert_drill_report(report: &DrillReport, netns: bool) {
    assert_eq!(report.schema, SCHEMA);
    assert_eq!(report.trustee_devices, 3);
    assert_eq!(report.trustee_threshold, 2);
    assert_eq!(report.approvals_collected, 2);
    assert_eq!(
        report.single_approval_rejected,
        "domain.admin_recovery_threshold_not_met"
    );
    assert_eq!(report.admin_signature_trusted_alone, 1);
    assert_eq!(report.coordinator_restarts, 2);
    assert!(report.unreachable_while_down);
    assert!(report.durable_pending_after_restart);
    assert_eq!(report.executed_state, "Executed");
    assert_eq!(report.new_revision, report.previous_revision + 1);
    assert!(report.new_owner_matches);
    assert!(report.administrators_cleared);
    assert_eq!(report.closed_old_sessions.len(), 2);
    assert!(report.pending_applicant_survived);
    assert_eq!(report.revoked_invitations, 1);
    assert!(report.idempotent_reexecute);
    for action in [
        "admin_recovery.request.submitted",
        "admin_recovery.executed",
        "admin_recovery.invalidations_applied",
    ] {
        assert!(
            report.audit_actions.iter().any(|entry| entry == action),
            "missing audit action {action}: {:?}",
            report.audit_actions
        );
    }
    if netns {
        assert!(report.partition_enforced);
        assert!(report.no_ack_while_partitioned);
    }
}

fn invalidation_count(store: &AdminRecoveryStore, key: &str) -> Result<usize> {
    let entry = store
        .audit_entries()?
        .into_iter()
        .find(|entry| entry.action == "admin_recovery.invalidations_applied")
        .context("missing invalidation audit entry")?;
    entry
        .details
        .get(key)
        .context("missing invalidation detail")?
        .parse()
        .context("invalid invalidation detail")
}

// ---------------------------------------------------------------------------
// Coordinator process control
// ---------------------------------------------------------------------------

struct CoordinatorChild {
    child: Child,
}

impl CoordinatorChild {
    fn kill_and_wait(&mut self) -> Result<()> {
        if self.child.try_wait()?.is_none() {
            self.child.kill()?;
        }
        let status = self.child.wait()?;
        assert!(
            !status.success() || status.code().is_none(),
            "unexpected Coordinator exit"
        );
        Ok(())
    }
}

impl Drop for CoordinatorChild {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn spawn_coordinator(
    root: &Path,
    bind: &str,
    netns: Option<&str>,
    attempt: usize,
    drill_root: &Path,
) -> Result<CoordinatorChild> {
    let ready_path = ready_path(drill_root, attempt);
    let _ = std::fs::remove_file(&ready_path);
    let job = ChildJob::ServeCoordinator {
        root: root.to_path_buf(),
        bind: bind.to_string(),
        ready_path: ready_path.clone(),
    };
    let log_path = drill_root.join(format!("coordinator-{attempt}.log"));
    let child = spawn_child(&job, netns, &log_path)?;
    wait_for_ready(&ready_path, Duration::from_secs(20))?;
    Ok(CoordinatorChild { child })
}

fn ready_path(drill_root: &Path, attempt: usize) -> PathBuf {
    drill_root.join(format!("coordinator-ready-{attempt}.json"))
}

fn coordinator_endpoint(
    drill_root: &Path,
    options: &DrillOptions,
    attempt: usize,
) -> Result<SocketAddr> {
    let ready: CoordinatorReady =
        serde_json::from_slice(&std::fs::read(ready_path(drill_root, attempt))?)?;
    let host = options
        .host_ip
        .clone()
        .unwrap_or_else(|| "127.0.0.1".to_string());
    Ok(SocketAddr::new(host.parse()?, ready.port))
}

fn wait_for_ready(path: &Path, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(bytes) = std::fs::read(path) {
            if serde_json::from_slice::<CoordinatorReady>(&bytes).is_ok() {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    anyhow::bail!(
        "Coordinator child never reported readiness: {}",
        path.display()
    );
}

fn spawn_child(job: &ChildJob, netns: Option<&str>, log_path: &Path) -> Result<Child> {
    let job_path = log_path.with_extension("job.json");
    write_json_atomic(&job_path, job)?;
    let executable = std::env::current_exe()?;
    let log = std::fs::File::create(log_path)?;
    let mut command = if let Some(netns) = netns {
        let mut command = Command::new("ip");
        command.args(["netns", "exec", netns]);
        command.arg(executable);
        command
    } else {
        Command::new(executable)
    };
    command
        .args([
            "--exact",
            "w14_admin_recovery_child_fixture",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, "1")
        .env(JOB_ENV, &job_path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .spawn()
        .context("failed to spawn W14 administrator recovery child process")
}

fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("JSON path has no parent")?;
    std::fs::create_dir_all(parent)?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Child process fixtures
// ---------------------------------------------------------------------------

#[test]
fn w14_admin_recovery_child_fixture() -> Result<()> {
    if std::env::var(CHILD_ENV).ok().as_deref() != Some("1") {
        return Ok(());
    }
    let job_path = std::env::var_os(JOB_ENV).context("missing W14 recovery child job path")?;
    let job: ChildJob = serde_json::from_slice(&std::fs::read(job_path)?)?;
    match job {
        ChildJob::SignApproval {
            device_dir,
            request_path,
            approval_path,
        } => {
            let identity = Identity::load_from_file(device_dir.join("identity.key"))?;
            let request: AdminRecoveryRequest =
                serde_json::from_slice(&std::fs::read(request_path)?)?;
            request.proposal.validate_structure()?;
            let approval =
                AdminRecoveryApproval::signed(&request.proposal, &identity, unix_time_ms())?;
            approval.verify(&request.proposal)?;
            std::fs::write(approval_path, serde_json::to_vec_pretty(&approval)?)?;
            Ok(())
        }
        ChildJob::ServeCoordinator {
            root,
            bind,
            ready_path,
        } => run_coordinator_child(&root, &bind, &ready_path),
    }
}

fn run_coordinator_child(root: &Path, bind: &str, ready_path: &Path) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let network_path = root.join("network.json");
        let recovery_store = Arc::new(AdminRecoveryStore::new(
            root.join("admin_recovery.json"),
            &network_path,
        )?);
        let invitation_store = Arc::new(InvitationStore::new(
            root.join("invitations.json"),
            Some(network_path.clone()),
            None,
            vec![],
        )?);
        let join_request_store = Arc::new(JoinRequestStore::new(
            root.join("join_requests.json"),
            Some(network_path.clone()),
            None,
            vec![],
        )?);
        let config_path = root.join("coordinator.toml");
        CoordinatorConfig::default().save_to_file(&config_path)?;
        let server = SignalingServer::new(bind.to_string(), "fd00::1000/120")?
            .with_network_record_path(&network_path)
            .with_invitation_store(invitation_store)
            .with_join_request_store(join_request_store)
            .with_admin_recovery_store(recovery_store)
            .bind()
            .await?;
        let local_addr = server.local_addr()?;
        let session_manager = server.session_manager();
        session_manager.attach_data_plane_membership(&network_path)?;
        session_manager
            .attach_blacklist_store(Arc::new(CoordinatorConfigBlacklistStore::new(config_path)))
            .await?;
        write_json_atomic(
            ready_path,
            &CoordinatorReady {
                pid: std::process::id(),
                port: local_addr.port(),
            },
        )?;
        server.run().await
    })
}

// ---------------------------------------------------------------------------
// WebSocket client helpers
// ---------------------------------------------------------------------------

async fn connect_and_register(
    endpoint: &SocketAddr,
    identity: &Identity,
    p2p_port: u16,
) -> Result<TestSocket> {
    let mut ws = connect_with_retry(*endpoint).await?;
    register(&mut ws, identity, p2p_port).await?;
    Ok(ws)
}

async fn request_list(
    ws: &mut TestSocket,
    network: &NetworkRecord,
) -> Result<Vec<fuckipv6_common::domain::AdminRecoveryStatusView>> {
    send_msg(
        ws,
        SignalingMessage::AdminRecoveryList {
            network_id: network.network_id.clone(),
        },
    )
    .await?;
    let listed = recv_until(ws, |message| {
        matches!(message, SignalingMessage::AdminRecoveryListAck { .. })
    })
    .await?;
    let SignalingMessage::AdminRecoveryListAck { requests } = listed else {
        anyhow::bail!("unexpected list acknowledgement shape");
    };
    Ok(requests)
}

async fn submit_expect_error(
    ws: &mut TestSocket,
    request: &AdminRecoveryRequest,
    context_label: &str,
) -> Result<String> {
    send_msg(
        ws,
        SignalingMessage::AdminRecoverySubmit {
            request: Box::new(request.clone()),
        },
    )
    .await?;
    let message = recv_until(ws, |message| {
        matches!(
            message,
            SignalingMessage::Error { .. } | SignalingMessage::AdminRecoverySubmitAck { .. }
        )
    })
    .await
    .with_context(|| format!("waiting for {context_label} rejection"))?;
    match message {
        SignalingMessage::Error { message, .. } => Ok(message),
        SignalingMessage::AdminRecoverySubmitAck { .. } => {
            anyhow::bail!("Coordinator accepted a request it must reject ({context_label})")
        }
        other => anyhow::bail!("unexpected rejection response shape: {other:?}"),
    }
}

async fn expect_no_ack(ws: &mut TestSocket, window: Duration) -> bool {
    match tokio::time::timeout(window, ws.next()).await {
        Err(_) => true,
        Ok(None) => true,
        Ok(Some(Ok(Message::Close(_)))) => true,
        Ok(Some(Ok(Message::Binary(data)))) => !matches!(
            codec::decode(&data),
            Ok(SignalingMessage::AdminRecoveryListAck { .. })
                | Ok(SignalingMessage::AdminRecoveryStatusAck { .. })
                | Ok(SignalingMessage::AdminRecoverySubmitAck { .. })
        ),
        Ok(Some(Ok(_))) => true,
        Ok(Some(Err(_))) => true,
    }
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
    tokio::time::timeout(Duration::from_secs(10), async {
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
    tokio::time::timeout(Duration::from_secs(10), async {
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

async fn connect_with_retry(endpoint: SocketAddr) -> Result<TestSocket> {
    let url = format!("ws://{endpoint}");
    let mut last_error = None;
    for _ in 0..40 {
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
        .unwrap_or_else(|| anyhow::anyhow!("failed to connect to the drill Coordinator")))
}

// ---------------------------------------------------------------------------
// nftables partition helpers (privileged variant)
// ---------------------------------------------------------------------------

fn apply_partition(netns: &str, coordinator_ip: &str) -> Result<()> {
    let _ = restore_partition(netns);
    run_command(
        "ip",
        &[
            "netns", "exec", netns, "nft", "add", "table", "inet", NF_TABLE,
        ],
    )?;
    // Block both directions for the Coordinator's namespace IP. Filtering on
    // `output` with `ip daddr <coordinator_ip>` matched nothing because replies
    // from the Coordinator carry `<coordinator_ip>` as their source address.
    run_command(
        "ip",
        &[
            "netns", "exec", netns, "nft", "add", "chain", "inet", NF_TABLE, "in", "{", "type",
            "filter", "hook", "input", "priority", "0", ";", "}",
        ],
    )?;
    run_command(
        "ip",
        &[
            "netns",
            "exec",
            netns,
            "nft",
            "add",
            "rule",
            "inet",
            NF_TABLE,
            "in",
            "ip",
            "daddr",
            coordinator_ip,
            "drop",
        ],
    )?;
    run_command(
        "ip",
        &[
            "netns", "exec", netns, "nft", "add", "chain", "inet", NF_TABLE, "out", "{", "type",
            "filter", "hook", "output", "priority", "0", ";", "}",
        ],
    )?;
    run_command(
        "ip",
        &[
            "netns",
            "exec",
            netns,
            "nft",
            "add",
            "rule",
            "inet",
            NF_TABLE,
            "out",
            "ip",
            "saddr",
            coordinator_ip,
            "drop",
        ],
    )?;
    Ok(())
}

fn restore_partition(netns: &str) -> Result<()> {
    run_command(
        "ip",
        &[
            "netns", "exec", netns, "nft", "delete", "table", "inet", NF_TABLE,
        ],
    )
}

fn run_command(program: &str, args: &[&str]) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {program} {args:?}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "{program} {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Misc helpers
// ---------------------------------------------------------------------------

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
    let path = std::env::temp_dir().join(format!(
        "fuckipv6-w14-admin-recovery-drill-{}-{label}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path)?;
    Ok(path)
}
