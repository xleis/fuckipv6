#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use fuckipv6_client::local_ipc::{self, LocalCommand, LocalEndpoint, LocalRequest, LocalResponse};
use fuckipv6_client::node::{NodeCommand, NodeCommandRequest};
use fuckipv6_common::domain::{
    NetworkId, NetworkRole, NodeId, PermissionCondition, PermissionEffect, PermissionRule,
    PolicyAction, RelayAccessPolicy, RelayAuthorizationDecision, RelayExistingSessionPolicy,
    RelayLimitPolicy, RelayMemberGrant, RelayMemberPolicyContext, ResourceKind, SecurityRuleId,
    SubjectSelector,
};
use fuckipv6_common::{CoordinatorConfig, Identity, NatType, QuicRelayClient};
use fuckipv6_coordinator::relay::server::{RelayPolicyAuthority, RelayServer};
use fuckipv6_coordinator::signaling::blacklist::CoordinatorConfigBlacklistStore;
use fuckipv6_coordinator::signaling::session::SessionManager;
use tokio::sync::mpsc;

const SEND_CONCURRENCY: usize = 32;
const PAYLOAD_BYTES: usize = 16 * 1_024;
const LOAD_DURATION: Duration = Duration::from_secs(4);
const PROBE_INTERVAL: Duration = Duration::from_millis(50);
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const DURING_PROBES: usize = 16;
const MAX_PROBE_P95_MS: f64 = 1_000.0;

struct ProcessGuard {
    child: Child,
    runtime_dir: PathBuf,
}

impl ProcessGuard {
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.child.try_wait()
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        cleanup_test_runtime(&self.runtime_dir);
    }
}

fn cleanup_test_runtime(runtime_dir: &Path) {
    for lock_name in ["node.lock", "control.lock"] {
        let Ok(pid) = read_pid(&runtime_dir.join(lock_name)) else {
            continue;
        };
        if process_uses_runtime(pid, runtime_dir) {
            let _ = Command::new("kill")
                .arg("-KILL")
                .arg(pid.to_string())
                .stderr(Stdio::null())
                .status();
        }
    }
}

fn process_uses_runtime(pid: u32, runtime_dir: &Path) -> bool {
    let expected = format!("FUCKIPV6_RUNTIME_DIR={}", runtime_dir.display());
    std::fs::read(format!("/proc/{pid}/environ")).is_ok_and(|environment| {
        environment
            .split(|byte| *byte == 0)
            .any(|entry| entry == expected.as_bytes())
    })
}

fn temp_dir(label: &str) -> Result<PathBuf> {
    let directory = std::env::temp_dir().join(format!(
        "fuckipv6-w13-control-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock before Unix epoch")?
            .as_nanos()
    ));
    std::fs::create_dir_all(directory.join("runtime"))?;
    std::fs::create_dir_all(directory.join("config"))?;
    std::fs::create_dir_all(directory.join("home"))?;
    Ok(directory)
}

fn spawn_control(directory: &Path) -> Result<ProcessGuard> {
    let runtime_dir = directory.join("runtime");
    let child = Command::new(env!("CARGO_BIN_EXE_fuckipv6"))
        .arg("__control")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("XDG_SESSION_TYPE")
        .env_remove("XDG_CURRENT_DESKTOP")
        .env_remove("DESKTOP_SESSION")
        .env("FUCKIPV6_RUNTIME_DIR", &runtime_dir)
        .env("XDG_CONFIG_HOME", directory.join("config"))
        .env("HOME", directory.join("home"))
        .env(
            "FUCKIPV6_INTERNAL_OPTIONS",
            r#"{"routing_mode":null,"exit_node":null,"is_exit_node":null,"webui_port":0}"#,
        )
        .env("RUST_LOG", "error")
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .context("failed to spawn __control process")?;
    Ok(ProcessGuard { child, runtime_dir })
}

fn read_pid(path: &Path) -> Result<u32> {
    std::fs::read_to_string(path)
        .with_context(|| format!("failed to read process lock {}", path.display()))?
        .trim()
        .parse()
        .with_context(|| format!("process lock {} did not contain a pid", path.display()))
}

fn process_alive(pid: u32) -> bool {
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        if stat
            .rsplit_once(") ")
            .and_then(|(_, fields)| fields.chars().next())
            .is_some_and(|state| matches!(state, 'Z' | 'X'))
        {
            return false;
        }
    }
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn send_signal(pid: u32, signal: &str) -> Result<()> {
    let status = Command::new("kill")
        .arg(format!("-{signal}"))
        .arg(pid.to_string())
        .status()
        .context("failed to execute kill")?;
    anyhow::ensure!(status.success(), "kill -{signal} {pid} failed: {status}");
    Ok(())
}

async fn wait_for_process_exit(pid: u32) -> Result<()> {
    for _ in 0..300 {
        if !process_alive(pid) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    anyhow::bail!("process {pid} did not exit before the deadline")
}

async fn wait_for_control_exit(control: &mut ProcessGuard) -> Result<()> {
    for _ in 0..1_200 {
        if control.try_wait()?.is_some() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    anyhow::bail!("__control did not exit before the deadline")
}

async fn wait_until_ready(endpoint: &LocalEndpoint, control: &mut ProcessGuard) -> Result<()> {
    for _ in 0..300 {
        if let Some(status) = control.try_wait()? {
            anyhow::bail!("__control exited before node readiness: {status}");
        }
        let request = local_ipc::request(
            endpoint,
            local_ipc::IpcRole::Cli,
            &LocalRequest {
                request_id: 1,
                command: LocalCommand::Snapshot,
            },
        );
        if matches!(
            tokio::time::timeout(Duration::from_millis(500), request).await,
            Ok(Ok(LocalResponse::Snapshot { .. }))
        ) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    anyhow::bail!("node IPC did not become ready before the deadline")
}

async fn probe_snapshot(endpoint: &LocalEndpoint, request_id: u64) -> Result<f64> {
    let started = Instant::now();
    let response = tokio::time::timeout(
        PROBE_TIMEOUT,
        local_ipc::request(
            endpoint,
            local_ipc::IpcRole::Cli,
            &LocalRequest {
                request_id,
                command: LocalCommand::Snapshot,
            },
        ),
    )
    .await
    .context("node snapshot probe timed out")??;
    anyhow::ensure!(
        matches!(response, LocalResponse::Snapshot { .. }),
        "node snapshot probe returned an unexpected response: {response:?}"
    );
    Ok(started.elapsed().as_secs_f64() * 1_000.0)
}

async fn probe_ui_manifest(endpoint: &LocalEndpoint, request_id: u64) -> Result<f64> {
    let started = Instant::now();
    let response = tokio::time::timeout(
        PROBE_TIMEOUT,
        local_ipc::request(
            endpoint,
            local_ipc::IpcRole::Cli,
            &LocalRequest {
                request_id,
                command: LocalCommand::Execute(NodeCommandRequest::new(
                    request_id,
                    NodeCommand::GetUiManifest,
                )),
            },
        ),
    )
    .await
    .context("UI manifest probe timed out")??;
    let LocalResponse::NodeCommand(result) = response else {
        anyhow::bail!("UI manifest probe returned an unexpected response: {response:?}");
    };
    anyhow::ensure!(
        result.is_success(),
        "UI manifest probe did not succeed: {result:?}"
    );
    Ok(started.elapsed().as_secs_f64() * 1_000.0)
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn network_id() -> NetworkId {
    NetworkId::from_public_key(b"w13-relay-control-network")
}

fn provider_node_id() -> NodeId {
    NodeId::from_public_key(b"w13-relay-control-provider")
}

fn grant(identity: &Identity, nonce_suffix: &str) -> Result<RelayMemberGrant> {
    let now = now_unix_ms();
    Ok(RelayMemberGrant::new(
        network_id(),
        provider_node_id(),
        identity,
        format!("w13-relay-control-grant-{nonce_suffix}"),
        1,
        now.saturating_sub(1_000),
        now.saturating_add(120_000),
        RelayLimitPolicy::default(),
    )?)
}

struct MemoryRelayPolicyAuthority {
    policy: Mutex<RelayAccessPolicy>,
}

impl MemoryRelayPolicyAuthority {
    fn new(identities: &[&Identity]) -> Self {
        let network_id = network_id();
        let provider_node_id = provider_node_id();
        let owner_rule = PermissionRule {
            rule_id: SecurityRuleId::from_public_key(b"w13-relay-control-owner"),
            effect: PermissionEffect::Allow,
            subjects: vec![SubjectSelector::Role(NetworkRole::Owner)],
            resources: vec![ResourceKind::Relay],
            actions: vec![PolicyAction::Use],
            conditions: vec![PermissionCondition::MembershipActive],
        };
        let member_rule = PermissionRule {
            rule_id: SecurityRuleId::from_public_key(b"w13-relay-control-member"),
            effect: PermissionEffect::Allow,
            subjects: vec![SubjectSelector::Role(NetworkRole::Member)],
            resources: vec![ResourceKind::Relay],
            actions: vec![PolicyAction::Use],
            conditions: vec![PermissionCondition::MembershipActive],
        };
        let members = identities
            .iter()
            .map(|identity| {
                (
                    NodeId::from_public_key(&identity.public_key()),
                    RelayMemberPolicyContext {
                        role: NetworkRole::Member,
                        groups: BTreeSet::new(),
                        membership_active: true,
                        recent_reauthentication: true,
                        local_rules: vec![member_rule.clone()],
                        local_access_policy: None,
                        limits: RelayLimitPolicy::default(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        Self {
            policy: Mutex::new(RelayAccessPolicy {
                network_id,
                provider_node_id,
                provider: RelayMemberPolicyContext {
                    role: NetworkRole::Owner,
                    groups: BTreeSet::new(),
                    membership_active: true,
                    recent_reauthentication: true,
                    local_rules: vec![owner_rule.clone()],
                    local_access_policy: None,
                    limits: RelayLimitPolicy::default(),
                },
                existing_session_policy: RelayExistingSessionPolicy::Reauthorize,
                revision: 1,
                updated_at_unix_ms: now_unix_ms().saturating_sub(1_000),
                network_rules: vec![owner_rule, member_rule],
                network_acl: None,
                members,
                limits: RelayLimitPolicy::default(),
                limit_sources: Vec::new(),
            }),
        }
    }
}

impl RelayPolicyAuthority for MemoryRelayPolicyAuthority {
    fn current_policy(&self) -> Result<RelayAccessPolicy> {
        self.policy
            .lock()
            .map(|policy| policy.clone())
            .map_err(|_| anyhow::anyhow!("Relay policy lock poisoned"))
    }

    fn authorize_grant(
        &self,
        grant: &RelayMemberGrant,
        now_unix_ms: u64,
    ) -> Result<(RelayAccessPolicy, RelayAuthorizationDecision)> {
        let mut policy = self.current_policy()?;
        let context = policy.members.get(&grant.consumer_node_id);
        let decision = policy.authorize_signed_grant(
            grant,
            context.map_or(NetworkRole::Member, |context| context.role),
            context.map_or_else(BTreeSet::new, |context| context.groups.clone()),
            context.is_some_and(|context| context.membership_active),
            context.is_some_and(|context| context.recent_reauthentication),
            now_unix_ms,
        )?;
        Ok((policy, decision))
    }
}

async fn register_relay_client(
    session_manager: &SessionManager,
    identity: &Identity,
    udp_port: u16,
    virtual_port: u16,
) -> Result<Ipv6Addr> {
    Ok(session_manager
        .register_client(
            identity.client_id().to_string(),
            identity.public_key(),
            SocketAddr::from((Ipv4Addr::LOCALHOST, udp_port)),
            virtual_port,
            NatType::Open,
            mpsc::unbounded_channel().0,
        )
        .await?
        .primary_ipv6())
}

fn percentile(samples: &[f64], percentile: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let index = ((sorted.len() as f64 * percentile).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len().saturating_sub(1));
    sorted[index]
}

fn mean(samples: &[f64]) -> f64 {
    samples.iter().sum::<f64>() / samples.len().max(1) as f64
}

fn repository_commit() -> String {
    Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

fn repository_dirty() -> bool {
    Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .map_or(true, |output| {
            !output.status.success() || !output.stdout.is_empty()
        })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn relay_saturation_keeps_control_snapshot_and_manifest_available() -> Result<()> {
    let directory = temp_dir("availability")?;
    let runtime_dir = directory.join("runtime");
    let endpoint = LocalEndpoint::Path(runtime_dir.join("node.sock"));
    let mut control = spawn_control(&directory)?;
    wait_until_ready(&endpoint, &mut control).await?;
    let control_pid = control.child.id();
    let node_pid = read_pid(&runtime_dir.join("node.lock"))?;
    assert_ne!(control_pid, node_pid, "control and node must be distinct");

    let identity_a = Identity::generate()?;
    let identity_b = Identity::generate()?;
    let coordinator_config_path = directory.join("coordinator.toml");
    CoordinatorConfig::default().save_to_file(&coordinator_config_path)?;
    let session_manager = Arc::new(SessionManager::new("fd00::1000/120")?);
    session_manager
        .attach_blacklist_store(Arc::new(CoordinatorConfigBlacklistStore::new(
            coordinator_config_path,
        )))
        .await?;
    let virtual_ip = register_relay_client(&session_manager, &identity_a, 41_000, 51_000).await?;
    let peer_ip = register_relay_client(&session_manager, &identity_b, 41_001, 51_001).await?;
    let server = RelayServer::bind_with_generated_certificate(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        session_manager,
        Arc::new(MemoryRelayPolicyAuthority::new(&[&identity_a, &identity_b])),
    )?;
    let relay_addr = server.local_addr();
    let server_pin = server.certificate().public_key_pin;
    let server_task = tokio::spawn(server.clone().run());

    let sender = QuicRelayClient::connect(
        relay_addr,
        server_pin,
        "fuckipv6.local",
        &identity_a,
        virtual_ip,
        &grant(&identity_a, "sender")?,
    )
    .await
    .context("Relay rejected the load sender")?;
    let receiver = QuicRelayClient::connect(
        relay_addr,
        server_pin,
        "fuckipv6.local",
        &identity_b,
        peer_ip,
        &grant(&identity_b, "receiver")?,
    )
    .await
    .context("Relay rejected the load receiver")?;

    let mut request_id = 10;
    let mut baseline_snapshots = Vec::new();
    let mut baseline_manifests = Vec::new();
    for _ in 0..5 {
        baseline_snapshots.push(probe_snapshot(&endpoint, request_id).await?);
        request_id += 1;
        baseline_manifests.push(probe_ui_manifest(&endpoint, request_id).await?);
        request_id += 1;
    }
    let usage_before = server.usage_snapshot().await;

    let stop = Arc::new(AtomicBool::new(false));
    let started = Arc::new(AtomicU64::new(0));
    let sequence = Arc::new(AtomicU64::new(1));
    let traffic_deadline = Instant::now() + LOAD_DURATION;
    let receiver_stop = Arc::new(AtomicBool::new(false));
    let receiver_loop_stop = receiver_stop.clone();
    let receiver_task = tokio::spawn(async move {
        let mut received = 0u64;
        while !receiver_loop_stop.load(Ordering::Acquire) {
            match tokio::time::timeout(Duration::from_millis(50), receiver.recv_packet()).await {
                Ok(Some(_)) => received += 1,
                Ok(None) => break,
                Err(_) => {}
            }
        }
        (receiver, received)
    });

    let mut senders = Vec::with_capacity(SEND_CONCURRENCY);
    for _ in 0..SEND_CONCURRENCY {
        let sender = sender.clone();
        let started = started.clone();
        let sequence = sequence.clone();
        let stop = stop.clone();
        senders.push(async move {
            started.fetch_add(1, Ordering::Release);
            while !stop.load(Ordering::Acquire) && Instant::now() < traffic_deadline {
                let packet_sequence = sequence.fetch_add(1, Ordering::Relaxed);
                let mut payload = vec![0x5a; PAYLOAD_BYTES];
                payload[..8].copy_from_slice(&packet_sequence.to_be_bytes());
                sender
                    .send_packet(
                        packet_sequence,
                        virtual_ip,
                        peer_ip,
                        bytes::Bytes::from(payload),
                    )
                    .await?;
            }
            Ok::<_, anyhow::Error>(())
        });
    }
    let load = futures::future::join_all(senders);
    let load_task = tokio::spawn(load);
    tokio::time::timeout(Duration::from_secs(2), async {
        while started.load(Ordering::Acquire) != SEND_CONCURRENCY as u64 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("Relay senders did not enter the saturation phase")?;

    let mut during_snapshots = Vec::with_capacity(DURING_PROBES);
    let mut during_manifests = Vec::with_capacity(DURING_PROBES);
    for _ in 0..DURING_PROBES {
        during_snapshots.push(probe_snapshot(&endpoint, request_id).await?);
        request_id += 1;
        during_manifests.push(probe_ui_manifest(&endpoint, request_id).await?);
        request_id += 1;
        tokio::time::sleep(PROBE_INTERVAL).await;
    }
    stop.store(true, Ordering::Release);
    let sender_results = load_task
        .await
        .context("Relay load task panicked")?
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    receiver_stop.store(true, Ordering::Release);
    assert!(
        sender_results.len() == SEND_CONCURRENCY,
        "not every Relay sender completed"
    );
    let (receiver, received) = receiver_task
        .await
        .context("Relay receiver task panicked")?;
    let usage_after = server.usage_snapshot().await;
    let forwarded_delta = usage_after
        .forwarded_packets
        .saturating_sub(usage_before.forwarded_packets);

    let mut after_snapshots = Vec::new();
    let mut after_manifests = Vec::new();
    for _ in 0..5 {
        after_snapshots.push(probe_snapshot(&endpoint, request_id).await?);
        request_id += 1;
        after_manifests.push(probe_ui_manifest(&endpoint, request_id).await?);
        request_id += 1;
    }

    assert!(process_alive(control_pid), "__control exited during load");
    assert!(process_alive(node_pid), "__node exited during load");
    assert!(
        forwarded_delta > 0,
        "Relay did not forward saturation traffic"
    );
    assert!(
        received > 0,
        "Relay receiver observed no saturation traffic"
    );
    assert!(
        percentile(&during_snapshots, 0.95) <= MAX_PROBE_P95_MS,
        "node snapshot p95 exceeded {MAX_PROBE_P95_MS} ms under Relay load"
    );
    assert!(
        percentile(&during_manifests, 0.95) <= MAX_PROBE_P95_MS,
        "UI manifest p95 exceeded {MAX_PROBE_P95_MS} ms under Relay load"
    );

    let attempted_packets = sequence.load(Ordering::Relaxed).saturating_sub(1);
    let report = serde_json::json!({
        "schema": "fuckipv6.w13.relay-control-availability.v1",
        "commit": repository_commit(),
        "worktree_dirty": repository_dirty(),
        "test_profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "platform": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "kernel": std::fs::read_to_string("/proc/sys/kernel/osrelease")
                .map(|value| value.trim().to_string())
                .unwrap_or_else(|_| "unknown".into()),
        },
        "topology": "single-host real __control/__node processes; RelayServer and QUIC loopback load execute in the test process",
        "workload": {
            "load_duration_ms": LOAD_DURATION.as_millis(),
            "send_concurrency": SEND_CONCURRENCY,
            "payload_bytes": PAYLOAD_BYTES,
            "attempted_packets": attempted_packets,
            "relay_forwarded_packets": forwarded_delta,
            "receiver_observed_packets": received,
            "probes_per_capability": DURING_PROBES,
            "probe_interval_ms": PROBE_INTERVAL.as_millis(),
            "probe_timeout_ms": PROBE_TIMEOUT.as_millis(),
        },
        "control_plane_latency_ms": {
            "snapshot": {
                "baseline_p95": percentile(&baseline_snapshots, 0.95),
                "during_mean": mean(&during_snapshots),
                "during_p95": percentile(&during_snapshots, 0.95),
                "during_max": during_snapshots.iter().copied().fold(0.0, f64::max),
                "after_p95": percentile(&after_snapshots, 0.95),
            },
            "ui_manifest": {
                "baseline_p95": percentile(&baseline_manifests, 0.95),
                "during_mean": mean(&during_manifests),
                "during_p95": percentile(&during_manifests, 0.95),
                "during_max": during_manifests.iter().copied().fold(0.0, f64::max),
                "after_p95": percentile(&after_manifests, 0.95),
            },
        },
        "control_process_alive": true,
        "node_process_alive": true,
        "recovered_after_load": true,
    });
    let encoded = serde_json::to_string_pretty(&report)?;
    println!("W13_RELAY_CONTROL_AVAILABILITY_REPORT={encoded}");
    if let Some(path) = std::env::var_os("FUCKIPV6_W13_RELAY_CONTROL_REPORT") {
        std::fs::write(path, encoded)?;
    }

    sender.close().await;
    receiver.close().await;
    server.shutdown().await;
    server_task.await.context("Relay server task panicked")??;

    send_signal(control_pid, "TERM")?;
    wait_for_control_exit(&mut control).await?;
    wait_for_process_exit(node_pid).await?;
    std::fs::remove_dir_all(directory)?;
    Ok(())
}
