#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use bytes::Bytes;
use fuckipv6_client::routing::linux_exit::LinuxExitForwarder;
use fuckipv6_common::domain::{
    NetworkId, NetworkRole, NodeId, PermissionCondition, PermissionEffect, PermissionRule,
    PolicyAction, RelayAccessPolicy, RelayAuthorizationDecision, RelayExistingSessionPolicy,
    RelayHealthStatus, RelayLifecycleState, RelayLimitPolicy, RelayMemberGrant,
    RelayMemberPolicyContext, ResourceKind, SecurityRuleId, SubjectSelector,
};
use fuckipv6_common::{
    CoordinatorConfig, ExitNodeConfig, Identity, NatType, QuicRelayClient, VirtualIps,
};
use fuckipv6_coordinator::relay::server::{RelayPolicyAuthority, RelayServer};
use fuckipv6_coordinator::signaling::blacklist::CoordinatorConfigBlacklistStore;
use fuckipv6_coordinator::signaling::server::SignalingServer;
use fuckipv6_coordinator::signaling::session::SessionManager;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{timeout, Instant};

const TOPOLOGY_ENV: &str = "FUCKIPV6_W13_FAILURE_NETNS_TOPOLOGY";
const CLIENT_NETNS_ENV: &str = "FUCKIPV6_W13_FAILURE_NETNS_CLIENT";
const REPORT_ENV: &str = "FUCKIPV6_W13_FAILURE_REPORT";
const TUN_NAME: &str = "fip6tun";
const EXIT_CLIENT_ID: &str = "w13-netns-client";
const EXIT_CLIENT_V4: &str = "10.0.0.2";
const UPSTREAM_V4: &str = "198.51.100.10";
const RELAY_PROBE_PAYLOAD: &[u8] = b"w13-three-component-failure-probe";

struct MemoryRelayPolicyAuthority {
    policy: Mutex<RelayAccessPolicy>,
}

impl MemoryRelayPolicyAuthority {
    fn new(identities: &[&Identity]) -> Self {
        let network_id = network_id();
        let provider_node_id = provider_node_id();
        let owner_rule = PermissionRule {
            rule_id: SecurityRuleId::from_public_key(b"w13-failure-owner"),
            effect: PermissionEffect::Allow,
            subjects: vec![SubjectSelector::Role(NetworkRole::Owner)],
            resources: vec![ResourceKind::Relay],
            actions: vec![PolicyAction::Use],
            conditions: vec![PermissionCondition::MembershipActive],
        };
        let member_rule = PermissionRule {
            rule_id: SecurityRuleId::from_public_key(b"w13-failure-member"),
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

fn require_isolated_topology() {
    assert_eq!(
        std::env::var(TOPOLOGY_ENV).as_deref(),
        Ok("1"),
        "run through scripts/test-w13-relay-failure-netns.sh"
    );
    // SAFETY: geteuid has no preconditions and does not access memory.
    assert_eq!(
        unsafe { libc::geteuid() },
        0,
        "the isolated W13 failure test must run as root"
    );
}

fn command_output(program: &str, args: &[&str]) -> Output {
    Command::new(program)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("failed to execute {program}: {error}"))
}

fn command_stdout(program: &str, args: &[&str]) -> String {
    let output = command_output(program, args);
    assert!(
        output.status.success(),
        "{program} {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn iptables_stdout(table: &str, args: &[&str]) -> String {
    let mut command_args = vec!["-w", "5", "-t", table];
    command_args.extend_from_slice(args);
    command_stdout("iptables", &command_args)
}

fn ipv4_forwarding() -> String {
    std::fs::read_to_string("/proc/sys/net/ipv4/ip_forward")
        .expect("read IPv4 forwarding sysctl")
        .trim()
        .to_string()
}

fn ping_upstream() -> Output {
    let client_netns = std::env::var(CLIENT_NETNS_ENV)
        .unwrap_or_else(|_| panic!("{CLIENT_NETNS_ENV} must be set"));
    command_output(
        "ip",
        &[
            "netns",
            "exec",
            &client_netns,
            "ping",
            "-c",
            "1",
            "-W",
            "1",
            UPSTREAM_V4,
        ],
    )
}

fn client_default_still_uses_virtual_link() -> bool {
    let client_netns = std::env::var(CLIENT_NETNS_ENV)
        .unwrap_or_else(|_| panic!("{CLIENT_NETNS_ENV} must be set"));
    let route = command_stdout(
        "ip",
        &[
            "netns",
            "exec",
            &client_netns,
            "ip",
            "route",
            "show",
            "default",
        ],
    );
    route.contains("fip6cli") && route.contains("10.0.0.1")
}

fn assert_exit_resources_absent() {
    let return_route = command_stdout("ip", &["route", "show", "exact", "10.0.0.2/32"]);
    assert!(
        !return_route.contains(&format!("dev {TUN_NAME}")),
        "Exit return route survived rollback: {return_route:?}"
    );
    assert!(
        !iptables_stdout("filter", &["-S", "FORWARD"]).contains("FKV6F-"),
        "Exit filter chain survived rollback"
    );
    assert!(
        !iptables_stdout("nat", &["-S", "POSTROUTING"]).contains("FKV6N-"),
        "Exit NAT chain survived rollback"
    );
    assert_eq!(ipv4_forwarding(), "0");
}

async fn start_signaling() -> Result<(SocketAddr, Arc<SessionManager>, JoinHandle<Result<()>>)> {
    let config_path = signaling_config_path();
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).context("create Signaling test config directory")?;
    }
    CoordinatorConfig::default()
        .save_to_file(&config_path)
        .context("persist Signaling test CoordinatorConfig")?;

    let server = SignalingServer::new("127.0.0.1:0".into(), "fd00::1000/120")?
        .bind()
        .await
        .context("bind real SignalingServer")?;
    server
        .session_manager()
        .attach_blacklist_store(Arc::new(CoordinatorConfigBlacklistStore::new(config_path)))
        .await
        .context("attach Signaling test blacklist and revocation store")?;
    let addr = server.local_addr()?;
    let session_manager = server.session_manager();
    let task = tokio::spawn(server.run());
    for _ in 0..80 {
        if tcp_reachable(addr).await {
            return Ok((addr, session_manager, task));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    task.abort();
    anyhow::bail!("SignalingServer did not accept loopback traffic")
}

fn signaling_config_path() -> std::path::PathBuf {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let base = std::env::var_os("FUCKIPV6_W13_FAILURE_NETNS_WORK")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join(format!(
        "w13-failure-signaling-{}-{timestamp}-{sequence}.toml",
        std::process::id()
    ))
}

async fn tcp_reachable(addr: SocketAddr) -> bool {
    matches!(
        timeout(Duration::from_millis(150), TcpStream::connect(addr)).await,
        Ok(Ok(_))
    )
}

async fn wait_for_tcp_closed(addr: SocketAddr) -> bool {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if !tcp_reachable(addr).await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

async fn start_relay(
    session_manager: Arc<SessionManager>,
    identities: &[&Identity],
) -> Result<(RelayServer, JoinHandle<Result<()>>)> {
    let relay = RelayServer::bind_with_generated_certificate(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        session_manager,
        Arc::new(MemoryRelayPolicyAuthority::new(identities)),
    )?;
    let task = tokio::spawn(relay.clone().run());
    Ok((relay, task))
}

async fn connect_relay_pair(
    relay: &RelayServer,
    session_manager: &SessionManager,
    sender_identity: &Identity,
    receiver_identity: &Identity,
) -> Result<(QuicRelayClient, QuicRelayClient, Ipv6Addr, Ipv6Addr)> {
    let sender_ip = register_relay_client(session_manager, sender_identity, 41_000, 51_000).await?;
    let receiver_ip =
        register_relay_client(session_manager, receiver_identity, 41_001, 51_001).await?;
    let sender = QuicRelayClient::connect(
        relay.local_addr(),
        relay.certificate().public_key_pin,
        "fuckipv6.local",
        sender_identity,
        sender_ip,
        &grant(sender_identity, "sender")?,
    )
    .await
    .context("connect real Relay sender")?;
    let receiver = QuicRelayClient::connect(
        relay.local_addr(),
        relay.certificate().public_key_pin,
        "fuckipv6.local",
        receiver_identity,
        receiver_ip,
        &grant(receiver_identity, "receiver")?,
    )
    .await
    .context("connect real Relay receiver")?;
    Ok((sender, receiver, sender_ip, receiver_ip))
}

async fn verify_relay_forwarding(
    sender: &QuicRelayClient,
    receiver: &QuicRelayClient,
    sender_ip: Ipv6Addr,
    receiver_ip: Ipv6Addr,
    stream_id: u64,
) -> Result<()> {
    sender
        .send_packet(
            stream_id,
            sender_ip,
            receiver_ip,
            Bytes::from_static(RELAY_PROBE_PAYLOAD),
        )
        .await
        .context("send through real Relay")?;
    let packet = timeout(Duration::from_secs(2), receiver.recv_packet())
        .await
        .context("Relay receiver timed out")?
        .context("Relay receiver closed before delivery")?;
    anyhow::ensure!(
        packet.src_ip == sender_ip,
        "Relay exposed a source other than the assigned virtual address"
    );
    anyhow::ensure!(
        packet.dst_ip == receiver_ip,
        "Relay changed the destination"
    );
    anyhow::ensure!(
        packet.payload.as_ref() == RELAY_PROBE_PAYLOAD,
        "Relay changed the probe payload"
    );
    Ok(())
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

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn network_id() -> NetworkId {
    NetworkId::from_public_key(b"w13-relay-failure-netns-network")
}

fn provider_node_id() -> NodeId {
    NodeId::from_public_key(b"w13-relay-failure-netns-provider")
}

fn grant(identity: &Identity, nonce_suffix: &str) -> Result<RelayMemberGrant> {
    let now = now_unix_ms();
    Ok(RelayMemberGrant::new(
        network_id(),
        provider_node_id(),
        identity,
        format!("w13-relay-failure-netns-{nonce_suffix}"),
        1,
        now.saturating_sub(1_000),
        now.saturating_add(120_000),
        RelayLimitPolicy::default(),
    )?)
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

fn kernel_release() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires the isolated privileged topology from scripts/test-w13-relay-failure-netns.sh"]
async fn relay_coordinator_exit_simultaneous_failure_is_bounded_fail_closed_and_recoverable(
) -> Result<()> {
    require_isolated_topology();
    assert_eq!(
        ipv4_forwarding(),
        "0",
        "test topology must start fail-closed"
    );

    let (signaling_addr, session_manager, signaling_task) = start_signaling().await?;
    let identity_a = Identity::generate()?;
    let identity_b = Identity::generate()?;
    let (relay, relay_task) =
        start_relay(session_manager.clone(), &[&identity_a, &identity_b]).await?;
    let (sender, receiver, sender_ip, receiver_ip) =
        connect_relay_pair(&relay, &session_manager, &identity_a, &identity_b).await?;
    verify_relay_forwarding(&sender, &receiver, sender_ip, receiver_ip, 1).await?;
    let relay_health_before = relay.health_snapshot().await;
    assert!(relay_health_before.listener_bound);
    assert!(relay_health_before.forwarding_observed);
    assert!(relay_health_before.status.is_usable());

    let config = ExitNodeConfig {
        enabled: true,
        ..ExitNodeConfig::default()
    };
    let mut forwarder = LinuxExitForwarder::install(&config, TUN_NAME)
        .await
        .context("install real Linux Exit forwarding")?
        .context("enabled Exit configuration produced no forwarder")?;
    forwarder
        .allow_client(
            EXIT_CLIENT_ID,
            &VirtualIps::from_ipv4(EXIT_CLIENT_V4.parse().expect("valid test IPv4")),
        )
        .await
        .context("authorize the Exit test client")?;
    let exit_output = ping_upstream();
    assert!(
        exit_output.status.success(),
        "real Exit forwarding did not work before failure: {}{}",
        String::from_utf8_lossy(&exit_output.stdout),
        String::from_utf8_lossy(&exit_output.stderr)
    );
    assert!(tcp_reachable(signaling_addr).await);
    verify_relay_forwarding(&sender, &receiver, sender_ip, receiver_ip, 2).await?;

    let relay_health_after = relay.clone();
    let fault_started = Instant::now();
    let signaling_fault = async move {
        signaling_task.abort();
        match signaling_task.await {
            Err(error) if error.is_cancelled() => Ok(()),
            Err(error) => Err(error).context("SignalingServer task failed during abort"),
            Ok(result) => result.context("SignalingServer exited unexpectedly before abort"),
        }
    };
    let relay_fault = async move {
        relay.shutdown().await;
        match relay_task.await {
            Ok(result) => result.context("RelayServer run task failed during shutdown"),
            Err(error) => Err(error).context("RelayServer task failed during shutdown"),
        }
    };
    let (signaling_result, relay_result, exit_result) = timeout(Duration::from_secs(8), async {
        tokio::join!(signaling_fault, relay_fault, forwarder.rollback_async())
    })
    .await
    .context("simultaneous Relay/Coordinator/Exit failure exceeded its bound")?;
    signaling_result?;
    relay_result?;
    exit_result.context("Exit forwarding rollback failed")?;
    let fault_elapsed_ms = fault_started.elapsed().as_millis();

    sender.close().await;
    receiver.close().await;

    assert!(
        wait_for_tcp_closed(signaling_addr).await,
        "Coordinator listener remained reachable after simultaneous failure"
    );
    let health_after = relay_health_after.health_snapshot().await;
    assert_eq!(health_after.lifecycle, RelayLifecycleState::Stopped);
    assert!(!health_after.listener_bound);
    assert_ne!(health_after.status, RelayHealthStatus::Healthy);
    assert!(!health_after.status.is_usable());
    assert_eq!(health_after.usage.active_connections, 0);

    assert!(!ping_upstream().status.success());
    assert!(
        client_default_still_uses_virtual_link(),
        "client silently fell back to a direct route"
    );
    assert_exit_resources_absent();

    let mut failure_probe_ms = Vec::new();
    for _ in 0..3 {
        let started = Instant::now();
        let output = ping_upstream();
        failure_probe_ms.push(started.elapsed().as_millis() as u64);
        assert!(
            !output.status.success(),
            "traffic resumed without a recovered Relay/Coordinator/Exit stack"
        );
    }
    let max_failure_probe_ms = failure_probe_ms.iter().copied().max().unwrap_or(0);
    assert!(
        max_failure_probe_ms < 2_500,
        "fail-closed probes were not bounded: {failure_probe_ms:?}"
    );

    let (recovery_signaling_addr, recovery_session_manager, recovery_signaling_task) =
        start_signaling().await?;
    let recovery_identity_a = Identity::generate()?;
    let recovery_identity_b = Identity::generate()?;
    let (recovery_relay, recovery_relay_task) = start_relay(
        recovery_session_manager.clone(),
        &[&recovery_identity_a, &recovery_identity_b],
    )
    .await?;
    let (recovery_sender, recovery_receiver, recovery_sender_ip, recovery_receiver_ip) =
        connect_relay_pair(
            &recovery_relay,
            &recovery_session_manager,
            &recovery_identity_a,
            &recovery_identity_b,
        )
        .await?;
    verify_relay_forwarding(
        &recovery_sender,
        &recovery_receiver,
        recovery_sender_ip,
        recovery_receiver_ip,
        3,
    )
    .await?;
    let mut recovery_forwarder = LinuxExitForwarder::install(&config, TUN_NAME)
        .await
        .context("reinstall Exit forwarding after simultaneous failure")?
        .context("enabled Exit configuration produced no recovery forwarder")?;
    recovery_forwarder
        .allow_client(
            EXIT_CLIENT_ID,
            &VirtualIps::from_ipv4(EXIT_CLIENT_V4.parse().expect("valid test IPv4")),
        )
        .await?;
    let recovery_ping = ping_upstream();
    assert!(
        recovery_ping.status.success(),
        "Exit did not recover after simultaneous failure: {}{}",
        String::from_utf8_lossy(&recovery_ping.stdout),
        String::from_utf8_lossy(&recovery_ping.stderr)
    );
    assert!(tcp_reachable(recovery_signaling_addr).await);
    let recovery_health = recovery_relay.health_snapshot().await;
    assert!(recovery_health.listener_bound);
    assert!(recovery_health.forwarding_observed);
    assert!(recovery_health.status.is_usable());

    recovery_sender.close().await;
    recovery_receiver.close().await;
    recovery_relay.shutdown().await;
    recovery_relay_task
        .await
        .context("recovery Relay task panicked")?
        .context("recovery Relay task failed")?;
    recovery_signaling_task.abort();
    let _ = recovery_signaling_task.await;
    recovery_forwarder.rollback_async().await?;
    assert!(!ping_upstream().status.success());
    assert_exit_resources_absent();

    let report = serde_json::json!({
        "schema": "fuckipv6.w13.relay-coordinator-exit-failure.v1",
        "commit": repository_commit(),
        "worktree_dirty": repository_dirty(),
        "test_profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "platform": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "kernel": kernel_release(),
        },
        "topology": {
            "kind": "private Linux network namespace",
            "client_netns": std::env::var(CLIENT_NETNS_ENV).unwrap_or_else(|_| "unknown".into()),
            "virtual_client": "10.0.0.2 via fip6cli -> fip6tun",
            "upstream": "198.51.100.10 via fip6out/fip6up",
            "components": [
                "real SignalingServer over loopback",
                "real RelayServer with two QUIC clients",
                "real LinuxExitForwarder with iptables NAT and kernel forwarding",
            ],
        },
        "fault_injection": {
            "mode": "tokio::join! simultaneous shutdown",
            "actions": [
                "abort SignalingServer listener task",
                "shutdown RelayServer QUIC endpoint and drain sessions",
                "rollback LinuxExitForwarder rules, routes and sysctl",
            ],
            "bounded_completion_ms": fault_elapsed_ms,
        },
        "before_failure": {
            "signaling_reachable": true,
            "relay_listener_bound": relay_health_before.listener_bound,
            "relay_forwarding_observed": relay_health_before.forwarding_observed,
            "relay_health_usable": relay_health_before.status.is_usable(),
            "exit_forwarding_observed": true,
            "relay_source_address_was_assigned_virtual_ip": true,
        },
        "after_failure": {
            "signaling_listener_closed": true,
            "relay_lifecycle": health_after.lifecycle,
            "relay_listener_bound": health_after.listener_bound,
            "relay_health_usable": health_after.status.is_usable(),
            "active_relay_connections": health_after.usage.active_connections,
            "exit_resources_absent": true,
            "client_route_stayed_on_virtual_link": true,
            "direct_fallback_observed": false,
            "bounded_failure_probe_ms": failure_probe_ms,
            "max_bounded_failure_probe_ms": max_failure_probe_ms,
        },
        "recovery": {
            "new_signaling_listener_reachable": true,
            "new_relay_forwarded_packet": true,
            "new_exit_forwarded_packet": true,
            "recovered_after_failure": true,
            "cleanup_after_recovery": true,
        },
    });
    let encoded = serde_json::to_string_pretty(&report)?;
    println!("W13_RELAY_COORDINATOR_EXIT_FAILURE_REPORT={encoded}");
    if let Some(path) = std::env::var_os(REPORT_ENV) {
        std::fs::write(path, encoded)?;
    }
    Ok(())
}
