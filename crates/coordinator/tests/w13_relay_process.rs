#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
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
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

const CHILD_ENV: &str = "FUCKIPV6_W13_RELAY_CHILD";
const IDENTITY_ENV: &str = "FUCKIPV6_W13_RELAY_IDENTITY";
const PEER_IDENTITY_ENV: &str = "FUCKIPV6_W13_RELAY_PEER_IDENTITY";
const STORM_IDENTITIES_ENV: &str = "FUCKIPV6_W13_RELAY_STORM_IDENTITIES";
const PORT_ENV: &str = "FUCKIPV6_W13_RELAY_PORT";
const READY_ENV: &str = "FUCKIPV6_W13_RELAY_READY";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Ready {
    addr: SocketAddr,
    server_pin: [u8; 32],
    virtual_ip: Ipv6Addr,
    peer_virtual_ip: Option<Ipv6Addr>,
    storm_virtual_ips: Vec<Ipv6Addr>,
}

struct ChildGuard {
    child: Child,
}

impl ChildGuard {
    fn spawn(identity: &Path, port: u16, ready: &Path) -> Result<Self> {
        Self::spawn_with_identities(identity, None, None, port, ready)
    }

    fn spawn_with_identities(
        identity: &Path,
        peer_identity: Option<&Path>,
        storm_identities: Option<&Path>,
        port: u16,
        ready: &Path,
    ) -> Result<Self> {
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args([
                "--exact",
                "relay_process_child_fixture",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_ENV, "1")
            .env(IDENTITY_ENV, identity)
            .env(PORT_ENV, port.to_string())
            .env(READY_ENV, ready)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        if let Some(peer_identity) = peer_identity {
            command.env(PEER_IDENTITY_ENV, peer_identity);
        }
        if let Some(storm_identities) = storm_identities {
            command.env(STORM_IDENTITIES_ENV, storm_identities);
        }
        let child = command
            .spawn()
            .context("failed to spawn Relay process fixture")?;
        Ok(Self { child })
    }

    fn id(&self) -> u32 {
        self.child.id()
    }

    fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>> {
        self.child
            .try_wait()
            .context("failed to poll Relay fixture")
    }

    fn kill_and_wait(&mut self) -> Result<()> {
        match self.child.try_wait()? {
            Some(status) => anyhow::bail!("Relay fixture exited before SIGKILL: {status}"),
            None => {
                self.child
                    .kill()
                    .context("failed to SIGKILL Relay fixture")?;
                let status = self.child.wait().context("failed to reap Relay fixture")?;
                anyhow::ensure!(
                    !status.success(),
                    "SIGKILLed Relay fixture unexpectedly exited successfully"
                );
                Ok(())
            }
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn temp_dir(label: &str) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!(
        "fuckipv6-w13-relay-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock before Unix epoch")?
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn free_udp_port() -> Result<u16> {
    let socket = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
    Ok(socket.local_addr()?.port())
}

async fn wait_for_ready(path: &Path, child: &mut ChildGuard) -> Result<Ready> {
    for _ in 0..200 {
        if let Some(status) = child.try_wait()? {
            anyhow::bail!("Relay fixture exited before readiness: {status}");
        }
        if let Ok(encoded) = std::fs::read(path) {
            if let Ok(ready) = serde_json::from_slice(&encoded) {
                return Ok(ready);
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    anyhow::bail!("Relay fixture did not become ready at {}", path.display())
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn network_id() -> NetworkId {
    NetworkId::from_public_key(b"w13-process-relay-network")
}

fn provider_node_id() -> NodeId {
    NodeId::from_public_key(b"w13-process-relay-provider")
}

fn grant(identity: &Identity, nonce_suffix: &str) -> Result<RelayMemberGrant> {
    let now = now_unix_ms();
    Ok(RelayMemberGrant::new(
        network_id(),
        provider_node_id(),
        identity,
        format!("w13-process-grant-{nonce_suffix}"),
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
            rule_id: SecurityRuleId::from_public_key(b"w13-relay-provider-owner"),
            effect: PermissionEffect::Allow,
            subjects: vec![SubjectSelector::Role(NetworkRole::Owner)],
            resources: vec![ResourceKind::Relay],
            actions: vec![PolicyAction::Use],
            conditions: vec![PermissionCondition::MembershipActive],
        };
        let member_rule = PermissionRule {
            rule_id: SecurityRuleId::from_public_key(b"w13-relay-consumer-member"),
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

#[test]
fn relay_process_child_fixture() -> Result<()> {
    if std::env::var_os(CHILD_ENV).is_none() {
        return Ok(());
    }
    let identity_path =
        PathBuf::from(std::env::var_os(IDENTITY_ENV).context("missing identity path")?);
    let ready_path = PathBuf::from(std::env::var_os(READY_ENV).context("missing ready path")?);
    let port = std::env::var(PORT_ENV)
        .context("missing Relay port")?
        .parse::<u16>()
        .context("invalid Relay port")?;
    let identity = Identity::load_from_file(&identity_path)?;
    let peer_identity = std::env::var_os(PEER_IDENTITY_ENV)
        .map(PathBuf::from)
        .map(Identity::load_from_file)
        .transpose()?;
    let storm_identities = match std::env::var_os(STORM_IDENTITIES_ENV) {
        Some(path) => {
            let mut paths = std::fs::read_dir(path)?
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<std::io::Result<Vec<_>>>()?;
            paths.sort();
            paths
                .into_iter()
                .map(Identity::load_from_file)
                .collect::<Result<Vec<_>>>()?
        }
        None => Vec::new(),
    };
    let coordinator_config_path = identity_path.with_extension("coordinator.toml");
    CoordinatorConfig::default().save_to_file(&coordinator_config_path)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let session_manager = Arc::new(SessionManager::new("fd00::1000/120")?);
        session_manager
            .attach_blacklist_store(Arc::new(CoordinatorConfigBlacklistStore::new(
                &coordinator_config_path,
            )))
            .await?;
        let (tx, _rx) = mpsc::unbounded_channel();
        let virtual_ip = session_manager
            .register_client(
                identity.client_id().to_string(),
                identity.public_key(),
                SocketAddr::from((Ipv4Addr::LOCALHOST, 41_000)),
                51_000,
                NatType::Open,
                tx,
            )
            .await?
            .primary_ipv6();
        let peer_virtual_ip = match peer_identity.as_ref() {
            Some(peer_identity) => Some(
                session_manager
                    .register_client(
                        peer_identity.client_id().to_string(),
                        peer_identity.public_key(),
                        SocketAddr::from((Ipv4Addr::LOCALHOST, 41_001)),
                        51_001,
                        NatType::Open,
                        mpsc::unbounded_channel().0,
                    )
                    .await?
                    .primary_ipv6(),
            ),
            None => None,
        };
        let mut storm_virtual_ips = Vec::with_capacity(storm_identities.len());
        for (index, storm_identity) in storm_identities.iter().enumerate() {
            let index = u16::try_from(index)?;
            storm_virtual_ips.push(
                session_manager
                    .register_client(
                        storm_identity.client_id().to_string(),
                        storm_identity.public_key(),
                        SocketAddr::from((Ipv4Addr::LOCALHOST, 42_000 + index)),
                        52_000 + index,
                        NatType::Open,
                        mpsc::unbounded_channel().0,
                    )
                    .await?
                    .primary_ipv6(),
            );
        }
        let mut identities = vec![&identity];
        if let Some(peer_identity) = peer_identity.as_ref() {
            identities.push(peer_identity);
        }
        identities.extend(storm_identities.iter());
        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
            session_manager,
            Arc::new(MemoryRelayPolicyAuthority::new(&identities)),
        )?;
        let ready = Ready {
            addr: server.local_addr(),
            server_pin: server.certificate().public_key_pin,
            virtual_ip,
            peer_virtual_ip,
            storm_virtual_ips,
        };
        let temporary = ready_path.with_extension("tmp");
        std::fs::write(&temporary, serde_json::to_vec(&ready)?)?;
        std::fs::rename(&temporary, &ready_path)?;
        let server_task = tokio::spawn(server.run());
        tokio::time::sleep(Duration::from_secs(120)).await;
        server_task.abort();
        Ok::<_, anyhow::Error>(())
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sigkill_restart_releases_relay_port_and_invalidates_old_session() -> Result<()> {
    let directory = temp_dir("restart")?;
    let identity_path = directory.join("identity.key");
    let ready_path = directory.join("relay-ready.json");
    let identity = Identity::generate()?;
    identity.save_to_file(&identity_path)?;
    let port = free_udp_port()?;

    let mut first = ChildGuard::spawn(&identity_path, port, &ready_path)?;
    let first_pid = first.id();
    let first_ready = wait_for_ready(&ready_path, &mut first).await?;
    assert_eq!(first_ready.addr.port(), port);

    let first_client = QuicRelayClient::connect(
        first_ready.addr,
        first_ready.server_pin,
        "fuckipv6.local",
        &identity,
        first_ready.virtual_ip,
        &grant(&identity, "first")?,
    )
    .await
    .context("first Relay process did not accept a registered client session")?;

    first.kill_and_wait()?;

    let released = std::net::UdpSocket::bind(first_ready.addr)
        .context("SIGKILLed Relay process left its UDP port leased")?;
    drop(released);
    std::fs::remove_file(&ready_path)?;

    let mut second = ChildGuard::spawn(&identity_path, port, &ready_path)?;
    let second_ready = wait_for_ready(&ready_path, &mut second).await?;
    assert_eq!(second_ready.addr, first_ready.addr);
    assert_ne!(
        second_ready.server_pin, first_ready.server_pin,
        "restart must use a fresh Relay certificate"
    );

    // QUIC itself has a 120-second idle timeout, so the stale client object may
    // remain locally open for a while after the remote process dies. What must
    // never happen is that the replacement Relay adopts or forwards traffic
    // from the old connection. A new handshake is required.
    let stale_send = tokio::time::timeout(
        Duration::from_secs(3),
        first_client.send_packet(
            1,
            first_ready.virtual_ip,
            second_ready.virtual_ip,
            b"stale relay session must not be adopted".as_slice(),
        ),
    )
    .await;
    assert!(
        !matches!(stale_send, Ok(Ok(()))),
        "replacement Relay accepted a packet from the stale QUIC session"
    );
    first_client.close().await;

    let second_client = QuicRelayClient::connect(
        second_ready.addr,
        second_ready.server_pin,
        "fuckipv6.local",
        &identity,
        second_ready.virtual_ip,
        &grant(&identity, "second")?,
    )
    .await
    .context("replacement Relay process could not reuse the stale port and register a client")?;

    second_client.close().await;
    second.kill_and_wait()?;
    assert!(
        !process_exists(first_pid),
        "SIGKILLed first Relay fixture remained present"
    );

    std::fs::remove_dir_all(directory)?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct ProcessMetrics {
    rss_bytes: u64,
    cpu_ticks: u64,
}

fn process_metrics(pid: u32) -> Result<ProcessMetrics> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status"))
        .with_context(|| format!("failed to read /proc/{pid}/status"))?;
    let rss_kib = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|value| value.split_whitespace().next())
        .context("Relay process status did not contain VmRSS")?
        .parse::<u64>()
        .context("invalid Relay process VmRSS")?;

    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .with_context(|| format!("failed to read /proc/{pid}/stat"))?;
    let (_, fields) = stat
        .rsplit_once(") ")
        .context("invalid Relay process stat record")?;
    let fields = fields.split_whitespace().collect::<Vec<_>>();
    let user_ticks = fields
        .get(11)
        .context("Relay process stat lacked utime")?
        .parse::<u64>()
        .context("invalid Relay process utime")?;
    let system_ticks = fields
        .get(12)
        .context("Relay process stat lacked stime")?
        .parse::<u64>()
        .context("invalid Relay process stime")?;

    Ok(ProcessMetrics {
        rss_bytes: rss_kib.saturating_mul(1024),
        cpu_ticks: user_ticks.saturating_add(system_ticks),
    })
}

fn clock_ticks_per_second() -> u64 {
    Command::new("getconf")
        .arg("CLK_TCK")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(100)
}

fn repository_commit() -> String {
    Command::new("git")
        .args(["-C", env!("CARGO_MANIFEST_DIR"), "rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

fn repository_dirty() -> bool {
    Command::new("git")
        .args(["-C", env!("CARGO_MANIFEST_DIR"), "status", "--porcelain"])
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
async fn sustained_traffic_and_connection_storm_keep_relay_resources_bounded() -> Result<()> {
    const TRAFFIC_PACKETS: usize = 2_048;
    const PAYLOAD_BYTES: usize = 1_024;
    const SEND_CONCURRENCY: usize = 32;
    const CONNECTION_CHURN: usize = 32;
    const MAX_RSS_GROWTH_BYTES: u64 = 96 * 1024 * 1024;
    assert_eq!(TRAFFIC_PACKETS % SEND_CONCURRENCY, 0);

    let directory = temp_dir("stress")?;
    let identity_a_path = directory.join("identity-a.key");
    let identity_b_path = directory.join("identity-b.key");
    let ready_path = directory.join("relay-ready.json");
    let identity_a = Identity::generate()?;
    let identity_b = Identity::generate()?;
    identity_a.save_to_file(&identity_a_path)?;
    identity_b.save_to_file(&identity_b_path)?;
    let storm_directory = directory.join("storm-identities");
    std::fs::create_dir_all(&storm_directory)?;
    let storm_identities = (0..CONNECTION_CHURN)
        .map(|_| Identity::generate())
        .collect::<Result<Vec<_>>>()?;
    for (index, identity) in storm_identities.iter().enumerate() {
        identity.save_to_file(storm_directory.join(format!("storm-{index:04}.key")))?;
    }
    let port = free_udp_port()?;

    let mut child = ChildGuard::spawn_with_identities(
        &identity_a_path,
        Some(&identity_b_path),
        Some(&storm_directory),
        port,
        &ready_path,
    )?;
    let ready = wait_for_ready(&ready_path, &mut child).await?;
    let peer_ip = ready
        .peer_virtual_ip
        .context("stress Relay fixture did not register its peer identity")?;
    assert_ne!(ready.virtual_ip, peer_ip);
    anyhow::ensure!(
        ready.storm_virtual_ips.len() == CONNECTION_CHURN,
        "Relay fixture registered {} of {CONNECTION_CHURN} storm identities",
        ready.storm_virtual_ips.len()
    );
    let before = process_metrics(child.id())?;

    // A simultaneous connection storm exercises registration, replacement and
    // teardown without allowing a failed cycle to poison later connections.
    let storm = futures::future::join_all(
        storm_identities
            .iter()
            .zip(&ready.storm_virtual_ips)
            .enumerate()
            .map(|(index, (identity, virtual_ip))| async move {
                QuicRelayClient::connect(
                    ready.addr,
                    ready.server_pin,
                    "fuckipv6.local",
                    identity,
                    *virtual_ip,
                    &grant(identity, &format!("storm-{index}"))?,
                )
                .await
                .with_context(|| format!("Relay connection storm failed at iteration {index}"))
            }),
    )
    .await;
    let storm_clients = storm
        .into_iter()
        .collect::<Result<Vec<_>>>()
        .context("Relay connection storm did not recover every registration")?;
    futures::future::join_all(storm_clients.iter().map(QuicRelayClient::close)).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let sender = QuicRelayClient::connect(
        ready.addr,
        ready.server_pin,
        "fuckipv6.local",
        &identity_a,
        ready.virtual_ip,
        &grant(&identity_a, "stress-sender")?,
    )
    .await
    .context("stress sender could not register after connection churn")?;
    let receiver = QuicRelayClient::connect(
        ready.addr,
        ready.server_pin,
        "fuckipv6.local",
        &identity_b,
        peer_ip,
        &grant(&identity_b, "stress-receiver")?,
    )
    .await
    .context("stress receiver could not register after connection churn")?;

    let traffic_before = process_metrics(child.id())?;
    let started = Instant::now();
    let mut observed_sequences = vec![false; TRAFFIC_PACKETS];
    let mut latency_samples_ms = Vec::with_capacity(TRAFFIC_PACKETS);
    let traffic = async {
        let packets_per_sender = TRAFFIC_PACKETS / SEND_CONCURRENCY;
        let mut senders = Vec::with_capacity(SEND_CONCURRENCY);
        for sender_index in 0..SEND_CONCURRENCY {
            let sender = &sender;
            senders.push(async move {
                for packet_index in 0..packets_per_sender {
                    let sequence = sender_index * packets_per_sender + packet_index;
                    let mut payload = vec![0x5a; PAYLOAD_BYTES];
                    payload[..8].copy_from_slice(&(sequence as u64).to_be_bytes());
                    payload[8..16]
                        .copy_from_slice(&(started.elapsed().as_micros() as u64).to_be_bytes());
                    sender
                        .send_packet(
                            sequence as u64 + 1,
                            ready.virtual_ip,
                            peer_ip,
                            bytes::Bytes::from(payload),
                        )
                        .await
                        .with_context(|| {
                            format!("sustained Relay send failed at packet {sequence}")
                        })?;
                }
                Ok::<_, anyhow::Error>(())
            });
        }
        let send = futures::future::join_all(senders);
        let receive = async {
            for sequence in 1..=TRAFFIC_PACKETS as u64 {
                let packet = tokio::time::timeout(Duration::from_secs(5), receiver.recv_packet())
                    .await
                    .with_context(|| format!("Relay receive timed out at packet {sequence}"))?
                    .with_context(|| format!("Relay closed before packet {sequence}"))?;
                anyhow::ensure!(packet.src_ip == ready.virtual_ip);
                anyhow::ensure!(packet.dst_ip == peer_ip);
                anyhow::ensure!(packet.payload.len() == PAYLOAD_BYTES);
                let sequence = u64::from_be_bytes(packet.payload[..8].try_into()?);
                let sent_at_micros = u64::from_be_bytes(packet.payload[8..16].try_into()?);
                let received_at_micros = started.elapsed().as_micros() as u64;
                anyhow::ensure!(sequence < TRAFFIC_PACKETS as u64);
                anyhow::ensure!(
                    !observed_sequences[sequence as usize],
                    "Relay delivered duplicate packet sequence {sequence}"
                );
                observed_sequences[sequence as usize] = true;
                anyhow::ensure!(packet.payload[16..].iter().all(|byte| *byte == 0x5a));
                latency_samples_ms
                    .push(received_at_micros.saturating_sub(sent_at_micros) as f64 / 1_000.0);
            }
            Ok::<_, anyhow::Error>(())
        };
        let (sent, received) = tokio::join!(send, receive);
        for result in sent {
            result?;
        }
        received?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let elapsed = started.elapsed();
    traffic?;
    anyhow::ensure!(
        observed_sequences.iter().all(|observed| *observed),
        "Relay stress workload did not deliver every packet exactly once"
    );
    let traffic_after = process_metrics(child.id())?;
    sender.close().await;
    receiver.close().await;

    // A completed run must release both registrations and task permits. A new
    // pair can register and forward only after that recovery point.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        child.try_wait()?.is_none(),
        "Relay process exited during sustained traffic"
    );
    let after = process_metrics(child.id())?;
    let rss_growth = after.rss_bytes.saturating_sub(before.rss_bytes);
    assert!(
        rss_growth <= MAX_RSS_GROWTH_BYTES,
        "Relay RSS grew by {rss_growth} bytes after churn and sustained traffic"
    );

    let recovered_sender = QuicRelayClient::connect(
        ready.addr,
        ready.server_pin,
        "fuckipv6.local",
        &identity_a,
        ready.virtual_ip,
        &grant(&identity_a, "recovered-sender")?,
    )
    .await
    .context("Relay did not accept a sender after stress recovery")?;
    let recovered_receiver = QuicRelayClient::connect(
        ready.addr,
        ready.server_pin,
        "fuckipv6.local",
        &identity_b,
        peer_ip,
        &grant(&identity_b, "recovered-receiver")?,
    )
    .await
    .context("Relay did not accept a receiver after stress recovery")?;
    let recovery_payload = bytes::Bytes::from_static(b"w13-relay-stress-recovered");
    let (sent, received) = tokio::join!(
        recovered_sender.send_packet(1, ready.virtual_ip, peer_ip, recovery_payload.clone(),),
        recovered_receiver.recv_packet(),
    );
    sent.context("Relay recovery send failed")?;
    assert_eq!(
        received
            .context("Relay recovery connection closed before delivery")?
            .payload,
        recovery_payload
    );
    recovered_sender.close().await;
    recovered_receiver.close().await;

    let elapsed_secs = elapsed.as_secs_f64();
    let payload_mib = (TRAFFIC_PACKETS * PAYLOAD_BYTES) as f64 / (1024.0 * 1024.0);
    let throughput_mib_per_sec = payload_mib / elapsed_secs;
    let cpu_seconds = traffic_after
        .cpu_ticks
        .saturating_sub(traffic_before.cpu_ticks) as f64
        / clock_ticks_per_second() as f64;
    let cpu_utilization_percent = cpu_seconds / elapsed_secs * 100.0;
    latency_samples_ms.sort_by(f64::total_cmp);
    let latency_min_ms = latency_samples_ms.first().copied().unwrap_or(0.0);
    let latency_max_ms = latency_samples_ms.last().copied().unwrap_or(0.0);
    let latency_mean_ms =
        latency_samples_ms.iter().sum::<f64>() / latency_samples_ms.len().max(1) as f64;
    let latency_percentile_ms = |percentile: f64| {
        let index = ((latency_samples_ms.len() as f64 * percentile).ceil() as usize)
            .saturating_sub(1)
            .min(latency_samples_ms.len().saturating_sub(1));
        latency_samples_ms.get(index).copied().unwrap_or(0.0)
    };
    let report = serde_json::json!({
        "schema": "fuckipv6.w13.relay-stress.v1",
        "commit": repository_commit(),
        "worktree_dirty": repository_dirty(),
        "test_profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "platform": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "kernel": kernel_release(),
        },
        "topology": "single-host parent/child Relay over QUIC loopback",
        "workload": {
            "packets": TRAFFIC_PACKETS,
            "payload_bytes": PAYLOAD_BYTES,
            "send_concurrency": SEND_CONCURRENCY,
            "connection_storm": CONNECTION_CHURN,
            "traffic_payload_bytes": TRAFFIC_PACKETS * PAYLOAD_BYTES,
        },
        "measurements": {
            "elapsed_ms": elapsed.as_millis(),
            "payload_mib_per_sec": throughput_mib_per_sec,
            "latency_ms": {
                "min": latency_min_ms,
                "mean": latency_mean_ms,
                "p50": latency_percentile_ms(0.50),
                "p95": latency_percentile_ms(0.95),
                "p99": latency_percentile_ms(0.99),
                "max": latency_max_ms,
            },
            "relay_cpu_seconds": cpu_seconds,
            "relay_cpu_utilization_percent": cpu_utilization_percent,
            "relay_rss_before_traffic_bytes": traffic_before.rss_bytes,
            "relay_rss_after_traffic_bytes": traffic_after.rss_bytes,
            "relay_rss_before_bytes": before.rss_bytes,
            "relay_rss_after_bytes": after.rss_bytes,
            "relay_rss_growth_bytes": rss_growth,
            "max_allowed_rss_growth_bytes": MAX_RSS_GROWTH_BYTES,
        },
        "recovered_after_churn": true,
    });
    let encoded = serde_json::to_string_pretty(&report)?;
    println!("W13_RELAY_STRESS_REPORT={encoded}");
    if let Some(path) = std::env::var_os("FUCKIPV6_W13_RELAY_STRESS_REPORT") {
        std::fs::write(path, encoded)?;
    }

    child.kill_and_wait()?;
    std::fs::remove_dir_all(directory)?;
    Ok(())
}

fn process_exists(pid: u32) -> bool {
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
