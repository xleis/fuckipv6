#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
const PORT_ENV: &str = "FUCKIPV6_W13_RELAY_PORT";
const READY_ENV: &str = "FUCKIPV6_W13_RELAY_READY";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Ready {
    addr: SocketAddr,
    server_pin: [u8; 32],
    virtual_ip: Ipv6Addr,
}

struct ChildGuard {
    child: Child,
}

impl ChildGuard {
    fn spawn(identity: &Path, port: u16, ready: &Path) -> Result<Self> {
        let child = Command::new(std::env::current_exe()?)
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
            .stderr(Stdio::inherit())
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
    fn new(identity: &Identity) -> Self {
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
        let mut members = BTreeMap::new();
        members.insert(
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
        );
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
        let server = RelayServer::bind_with_generated_certificate(
            SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
            session_manager,
            Arc::new(MemoryRelayPolicyAuthority::new(&identity)),
        )?;
        let ready = Ready {
            addr: server.local_addr(),
            server_pin: server.certificate().public_key_pin,
            virtual_ip,
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
