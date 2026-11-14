#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use fs2::FileExt;
use fuckipv6_common::domain::{
    CapabilityKind, CoordinatorAuthority, CoordinatorCandidate, CoordinatorCandidateConfig,
    CoordinatorCandidateConfigState, CoordinatorCandidateDeclaration,
    CoordinatorCandidateResources, CoordinatorCandidateWillingness, CoordinatorControlPlaneState,
    CoordinatorElectionProposal, CoordinatorElectionStage, CoordinatorElectionVote,
    CoordinatorEmergencyState, CoordinatorEmergencyWriteCredential, CoordinatorPhase,
    CoordinatorPublicReachability, CoordinatorRecoveryReport, CoordinatorRecoveryStage,
    CoordinatorTerm, CoordinatorWriteCredential, CoordinatorWriteKind, ExitContributionMode,
    ExitContributionObligation, ExitContributionScope, FencingToken, NetworkCreateProfile,
    NetworkId, NetworkJoinMode, NetworkRecord, NetworkVisibility, NodeId, PublicKey,
};
use fuckipv6_common::Identity;
use fuckipv6_coordinator::signaling::coordinator_candidates::CoordinatorCandidateStore;
use serde::{Deserialize, Serialize};

const CHILD_ENV: &str = "FUCKIPV6_W14_PROCESS_CHILD";
const JOB_ENV: &str = "FUCKIPV6_W14_PROCESS_JOB";
const REPORT_ENV: &str = "FUCKIPV6_W14_REAL_FAULT_REPORT";
const NETNS_ENV: &str = "FUCKIPV6_W14_NETNS_MODE";
const NETNS_A_ENV: &str = "FUCKIPV6_W14_NETNS_A";
const NETNS_B_ENV: &str = "FUCKIPV6_W14_NETNS_B";
const NETNS_C_ENV: &str = "FUCKIPV6_W14_NETNS_C";
const IP_A_ENV: &str = "FUCKIPV6_W14_IP_A";
const IP_B_ENV: &str = "FUCKIPV6_W14_IP_B";
const IP_C_ENV: &str = "FUCKIPV6_W14_IP_C";
const LEASE_MS: u64 = 5_000;
const PROBE_TIMEOUT: Duration = Duration::from_millis(1_200);
const CHILD_TIMEOUT: Duration = Duration::from_secs(15);
static PATH_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CandidateLetter {
    A,
    B,
    C,
}

impl CandidateLetter {
    fn label(self) -> &'static str {
        match self {
            Self::A => "candidate-a",
            Self::B => "candidate-b",
            Self::C => "candidate-c",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ChildJob {
    Store {
        store_path: PathBuf,
        network_path: PathBuf,
        output_path: PathBuf,
        started_path: PathBuf,
        gate_path: Option<PathBuf>,
        operation: Box<StoreOperation>,
    },
    HoldCandidateLock {
        lock_path: PathBuf,
        ready_path: PathBuf,
        release_path: PathBuf,
        done_path: PathBuf,
    },
    ServeProbe {
        bind_addr: SocketAddr,
        ready_path: PathBuf,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum StoreOperation {
    UpdateConfig {
        actor: NodeId,
        expected_revision: u64,
        config: CoordinatorCandidateConfig,
        now_unix_ms: u64,
    },
    PublishDeclaration {
        actor: NodeId,
        declaration: CoordinatorCandidateDeclaration,
        now_unix_ms: u64,
    },
    ProposeElection {
        actor: NodeId,
        proposal: CoordinatorElectionProposal,
        now_unix_ms: u64,
    },
    RecordElectionVote {
        actor: NodeId,
        proposal_id: fuckipv6_common::domain::ContentHash,
        vote: CoordinatorElectionVote,
        now_unix_ms: u64,
    },
    InstallTerm {
        actor: NodeId,
        term: CoordinatorTerm,
        now_unix_ms: u64,
    },
    Status {
        actor: NodeId,
        network_id: NetworkId,
        now_unix_ms: u64,
    },
    Recover {
        actor: NodeId,
        report: CoordinatorRecoveryReport,
        now_unix_ms: u64,
    },
    AuthorizeGovernance {
        actor: NodeId,
        credential: CoordinatorWriteCredential,
        write: CoordinatorWriteKind,
        now_unix_ms: u64,
    },
    IssueEmergencyLeader {
        actor: NodeId,
        network_id: NetworkId,
        online_members: Vec<NodeId>,
        now_unix_ms: u64,
    },
    RejectEmergencyGovernance {
        actor: NodeId,
        network_id: NetworkId,
        action: String,
        now_unix_ms: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoreOperationResult {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    status: Option<fuckipv6_common::domain::CoordinatorCandidateStatus>,
    #[serde(default)]
    recovery_stage: Option<CoordinatorRecoveryStage>,
    #[serde(default)]
    rejected: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReadyAddr {
    addr: SocketAddr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProbeEvidence {
    target: CandidateLetter,
    addr: SocketAddr,
    label: String,
    reachable: bool,
    latency_ms: Option<u128>,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ProcessEvidence {
    candidate: CandidateLetter,
    node_id: NodeId,
    pid: u32,
    probe_addr: SocketAddr,
    process_state_after_sigstop: Option<String>,
    recovered_after_sigcont: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
struct FaultEvent {
    at_unix_ms: u64,
    kind: String,
    target: String,
    detail: String,
}

#[derive(Debug, Clone, Serialize)]
struct TermEvidence {
    term: u64,
    leader: NodeId,
    fencing_token: u64,
    control_plane_state: CoordinatorControlPlaneState,
    governance_writable: bool,
}

#[derive(Debug, Clone, Serialize)]
struct RealFaultReport {
    schema: String,
    mode: String,
    network_id: NetworkId,
    candidate_group: Vec<NodeId>,
    processes: Vec<ProcessEvidence>,
    probes_before: Vec<ProbeEvidence>,
    probes_during_partition: Vec<ProbeEvidence>,
    probes_after_restore: Vec<ProbeEvidence>,
    faults: Vec<FaultEvent>,
    term_trace: Vec<TermEvidence>,
    emergency: Option<serde_json::Value>,
    emergency_governance_rejected: bool,
    old_leader_credential_rejected: bool,
    stale_rejoin_read_only: bool,
    concurrent_successes: usize,
    concurrent_rejections: usize,
    final_status: fuckipv6_common::domain::CoordinatorCandidateStatus,
    audit_actions: Vec<String>,
}

struct Fixture {
    dir: PathBuf,
    network: NetworkRecord,
    owner: Identity,
    candidates: BTreeMap<CandidateLetter, Identity>,
    ordinary: Identity,
    candidate_config: CoordinatorCandidateConfig,
    store_path: PathBuf,
    network_path: PathBuf,
    placement: Option<NetnsPlacement>,
}

#[derive(Clone)]
struct NetnsPlacement {
    names: BTreeMap<CandidateLetter, String>,
    ips: BTreeMap<CandidateLetter, IpAddr>,
}

impl NetnsPlacement {
    fn from_env() -> Result<Option<Self>> {
        if std::env::var(NETNS_ENV).ok().as_deref() != Some("1") {
            return Ok(None);
        }
        let mut names = BTreeMap::new();
        let mut ips = BTreeMap::new();
        for (letter, netns_env, ip_env) in [
            (CandidateLetter::A, NETNS_A_ENV, IP_A_ENV),
            (CandidateLetter::B, NETNS_B_ENV, IP_B_ENV),
            (CandidateLetter::C, NETNS_C_ENV, IP_C_ENV),
        ] {
            names.insert(
                letter,
                std::env::var(netns_env).with_context(|| {
                    format!("missing required environment variable {netns_env}")
                })?,
            );
            ips.insert(
                letter,
                std::env::var(ip_env)
                    .with_context(|| format!("missing required environment variable {ip_env}"))?
                    .parse()
                    .with_context(|| format!("invalid IP address in {ip_env}"))?,
            );
        }
        Ok(Some(Self { names, ips }))
    }

    fn netns(&self, letter: CandidateLetter) -> &str {
        self.names
            .get(&letter)
            .map(String::as_str)
            .expect("netns placement must be complete")
    }

    fn ip(&self, letter: CandidateLetter) -> IpAddr {
        *self
            .ips
            .get(&letter)
            .expect("netns placement must be complete")
    }
}

impl Fixture {
    fn new(label: &str, placement: Option<NetnsPlacement>) -> Result<Self> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "fuckipv6-w14-real-fault-{}-{}-{unique}",
            std::process::id(),
            label
        ));
        std::fs::create_dir_all(&dir)?;
        let now = unix_time_ms();
        let owner = Identity::generate()?;
        let candidate_a = Identity::generate()?;
        let candidate_b = Identity::generate()?;
        let candidate_c = Identity::generate()?;
        let ordinary = Identity::generate()?;
        let owner_id = node_id(&owner);
        let candidate_a_id = node_id(&candidate_a);
        let candidate_b_id = node_id(&candidate_b);
        let candidate_c_id = node_id(&candidate_c);
        let ordinary_id = node_id(&ordinary);

        let mut network = NetworkRecord::create_owned(
            NetworkCreateProfile {
                name: "w14-real-fault".into(),
                purpose: "real multi-process coordinator fault injection".into(),
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
            owner_id.clone(),
            now.saturating_sub(10_000),
        )?;
        network.members.extend([
            candidate_a_id.clone(),
            candidate_b_id.clone(),
            candidate_c_id.clone(),
            ordinary_id.clone(),
        ]);
        network.validate()?;

        let candidates = vec![
            CoordinatorCandidate {
                node_id: candidate_a_id,
                priority: 1,
            },
            CoordinatorCandidate {
                node_id: candidate_b_id,
                priority: 2,
            },
            CoordinatorCandidate {
                node_id: candidate_c_id,
                priority: 3,
            },
        ];
        let candidate_config = CoordinatorCandidateConfig::signed(
            network.network_id.clone(),
            1,
            candidates,
            now.saturating_sub(1_000),
            now.saturating_add(30 * 60 * 1000),
            &owner,
        )?;
        let network_path = dir.join("network.json");
        std::fs::write(&network_path, serde_json::to_vec_pretty(&network)?)?;

        let mut identities = BTreeMap::new();
        identities.insert(CandidateLetter::A, candidate_a);
        identities.insert(CandidateLetter::B, candidate_b);
        identities.insert(CandidateLetter::C, candidate_c);
        for (letter, identity) in &identities {
            identity.save_to_file(dir.join(format!("{}.identity", letter.label())))?;
        }
        owner.save_to_file(dir.join("owner.identity"))?;
        ordinary.save_to_file(dir.join("ordinary.identity"))?;

        Ok(Self {
            store_path: dir.join("coordinator_candidates.json"),
            network_path,
            dir,
            network,
            owner,
            candidates: identities,
            ordinary,
            candidate_config,
            placement,
        })
    }

    fn owner_id(&self) -> NodeId {
        node_id(&self.owner)
    }

    fn ordinary_id(&self) -> NodeId {
        node_id(&self.ordinary)
    }

    fn candidate_id(&self, letter: CandidateLetter) -> NodeId {
        node_id(
            self.candidates
                .get(&letter)
                .expect("fixture candidate must exist"),
        )
    }

    fn candidate_group(&self) -> Vec<NodeId> {
        [CandidateLetter::A, CandidateLetter::B, CandidateLetter::C]
            .into_iter()
            .map(|letter| self.candidate_id(letter))
            .collect()
    }

    fn bind_addr(&self, letter: CandidateLetter) -> SocketAddr {
        match &self.placement {
            Some(placement) => SocketAddr::new(placement.ip(letter), 0),
            None => "127.0.0.1:0".parse().expect("loopback address is valid"),
        }
    }

    fn store(&self) -> Result<CoordinatorCandidateStore> {
        CoordinatorCandidateStore::new(&self.store_path, &self.network_path)
    }

    fn unique_path(&self, prefix: &str) -> PathBuf {
        self.dir.join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            PATH_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn spawn_store_job(
        &self,
        operation: StoreOperation,
        netns: Option<&str>,
        gate_path: Option<PathBuf>,
    ) -> Result<JobChild> {
        let output_path = self.unique_path("store-result");
        let started_path = self.unique_path("store-started");
        let job = ChildJob::Store {
            store_path: self.store_path.clone(),
            network_path: self.network_path.clone(),
            output_path: output_path.clone(),
            started_path: started_path.clone(),
            gate_path,
            operation: Box::new(operation),
        };
        let child = spawn_child_job(&job, netns)?;
        Ok(JobChild {
            child,
            output_path,
            started_path,
        })
    }

    fn run_store_job(
        &self,
        operation: StoreOperation,
        netns: Option<&str>,
    ) -> Result<StoreOperationResult> {
        let mut child = self.spawn_store_job(operation, netns, None)?;
        child.wait_result()
    }

    fn spawn_probe_server(&self, letter: CandidateLetter) -> Result<ServerChild> {
        let ready_path = self.unique_path(&format!("ready-{}", letter.label()));
        let job = ChildJob::ServeProbe {
            bind_addr: self.bind_addr(letter),
            ready_path: ready_path.clone(),
        };
        let netns = self
            .placement
            .as_ref()
            .map(|placement| placement.netns(letter));
        let child = spawn_child_job(&job, netns)?;
        let ready: ReadyAddr = wait_for_json(&ready_path, CHILD_TIMEOUT)?;
        Ok(ServerChild {
            child,
            addr: ready.addr,
        })
    }

    fn spawn_lock_holder(&self) -> Result<LockHolderChild> {
        let lock_path = self.store_path.with_extension("json.lock");
        let ready_path = self.unique_path("lock-ready");
        let release_path = self.unique_path("lock-release");
        let done_path = self.unique_path("lock-done");
        let job = ChildJob::HoldCandidateLock {
            lock_path,
            ready_path: ready_path.clone(),
            release_path: release_path.clone(),
            done_path,
        };
        let child = spawn_child_job(&job, None)?;
        wait_until_exists(&ready_path, CHILD_TIMEOUT)?;
        Ok(LockHolderChild {
            child,
            release_path,
        })
    }

    fn declaration(
        &self,
        letter: CandidateLetter,
        willingness: CoordinatorCandidateWillingness,
        now: u64,
    ) -> Result<CoordinatorCandidateDeclaration> {
        CoordinatorCandidateDeclaration::signed(
            self.network.network_id.clone(),
            self.candidate_config.revision,
            CoordinatorPublicReachability {
                public_endpoint: Some(format!("198.51.100.{}:443", 10 + letter as u8).parse()?),
                reachable_via_relay: true,
            },
            CoordinatorCandidateResources {
                cpu_cores: 4,
                memory_bytes: 512 * 1024 * 1024,
                max_connections: 10_000,
            },
            willingness,
            now,
            now.saturating_add(10 * 60 * 1000),
            self.candidates
                .get(&letter)
                .expect("fixture candidate must exist"),
        )
        .map_err(Into::into)
    }

    fn proposal(
        &self,
        letter: CandidateLetter,
        term: u64,
        now: u64,
    ) -> Result<CoordinatorElectionProposal> {
        CoordinatorElectionProposal::signed(
            self.network.network_id.clone(),
            self.candidate_config.revision,
            term,
            self.candidate_group(),
            now,
            now.saturating_add(LEASE_MS),
            self.candidates
                .get(&letter)
                .expect("fixture candidate must exist"),
        )
        .map_err(Into::into)
    }

    fn vote(
        &self,
        voter: CandidateLetter,
        proposal: &CoordinatorElectionProposal,
        now: u64,
    ) -> Result<CoordinatorElectionVote> {
        CoordinatorElectionVote::signed(
            self.network.network_id.clone(),
            proposal.candidate_config_revision,
            proposal.term,
            proposal.leader.clone(),
            proposal.candidate_group.clone(),
            now,
            now.saturating_add(LEASE_MS),
            self.candidates
                .get(&voter)
                .expect("fixture candidate must exist"),
        )
        .map_err(Into::into)
    }

    fn manual_term(
        &self,
        term: u64,
        fencing_token: u64,
        leader: CandidateLetter,
        voters: &[CandidateLetter],
        now: u64,
    ) -> Result<CoordinatorTerm> {
        let leader_id = self.candidate_id(leader);
        let candidate_group = self.candidate_group();
        let majority_votes = voters
            .iter()
            .map(|voter| {
                CoordinatorElectionVote::signed(
                    self.network.network_id.clone(),
                    self.candidate_config.revision,
                    term,
                    leader_id.clone(),
                    candidate_group.clone(),
                    now,
                    now.saturating_add(LEASE_MS),
                    self.candidates
                        .get(voter)
                        .expect("fixture voter must exist"),
                )
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(CoordinatorTerm {
            network_id: self.network.network_id.clone(),
            term,
            leader: leader_id,
            candidate_group,
            candidate_config_revision: self.candidate_config.revision,
            majority_proof: majority_votes
                .iter()
                .map(|vote| vote.voter.clone())
                .collect(),
            majority_votes,
            authority: CoordinatorAuthority::Full,
            phase: CoordinatorPhase::Stable,
            fencing_token: FencingToken(fencing_token),
            issued_at_unix_ms: now,
            expires_at_unix_ms: now.saturating_add(LEASE_MS),
        })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.placement.is_none() {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

struct JobChild {
    child: Child,
    output_path: PathBuf,
    started_path: PathBuf,
}

impl JobChild {
    fn wait_started(&self) -> Result<()> {
        wait_until_exists(&self.started_path, CHILD_TIMEOUT)
    }

    fn wait_result(&mut self) -> Result<StoreOperationResult> {
        let result = wait_for_json(&self.output_path, CHILD_TIMEOUT)?;
        let status = self
            .child
            .wait()
            .context("failed waiting for store operation child")?;
        if !status.success() {
            bail!("store operation child exited with {status}");
        }
        Ok(result)
    }
}

impl Drop for JobChild {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

struct ServerChild {
    child: Child,
    addr: SocketAddr,
}

impl ServerChild {
    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn signal(&self, signal: &str) -> Result<()> {
        let status = Command::new("kill")
            .arg(format!("-{signal}"))
            .arg(self.child.id().to_string())
            .status()
            .context("failed to launch kill for candidate process")?;
        if !status.success() {
            bail!(
                "kill -{signal} failed for candidate process {}",
                self.child.id()
            );
        }
        Ok(())
    }

    fn kill_and_wait(&mut self) -> Result<()> {
        if self.child.try_wait()?.is_none() {
            self.child.kill()?;
        }
        let _ = self.child.wait();
        Ok(())
    }
}

impl Drop for ServerChild {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

struct LockHolderChild {
    child: Child,
    release_path: PathBuf,
}

impl LockHolderChild {
    fn release(&self) -> Result<()> {
        std::fs::write(&self.release_path, b"release")?;
        Ok(())
    }

    fn kill_and_wait(&mut self) -> Result<()> {
        if self.child.try_wait()?.is_none() {
            self.child.kill()?;
        }
        let _ = self.child.wait();
        Ok(())
    }
}

impl Drop for LockHolderChild {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn node_id(identity: &Identity) -> NodeId {
    NodeId::from_public_key(&identity.public_key())
}

fn spawn_child_job(job: &ChildJob, netns: Option<&str>) -> Result<Child> {
    let job_path = match job {
        ChildJob::Store { output_path, .. } => output_path.with_extension("job.json"),
        ChildJob::HoldCandidateLock { done_path, .. } => done_path.with_extension("job.json"),
        ChildJob::ServeProbe { ready_path, .. } => ready_path.with_extension("job.json"),
    };
    write_json_atomic(&job_path, job)?;
    let executable = std::env::current_exe()?;
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
            "w14_process_child_fixture",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, "1")
        .env(JOB_ENV, &job_path)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .context("failed to spawn W14 process fixture")
}

fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("JSON path has no parent")?;
    std::fs::create_dir_all(parent)?;
    let temporary = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        PATH_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&temporary, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

fn wait_for_json<T: for<'de> Deserialize<'de>>(path: &Path, timeout: Duration) -> Result<T> {
    let deadline = Instant::now() + timeout;
    loop {
        if path.exists() {
            match std::fs::read(path)
                .with_context(|| format!("failed to read {}", path.display()))
                .and_then(|bytes| serde_json::from_slice(&bytes).context("invalid fixture JSON"))
            {
                Ok(value) => return Ok(value),
                Err(_) if Instant::now() < deadline => {}
                Err(error) => return Err(error),
            }
        }
        if Instant::now() >= deadline {
            bail!("timed out waiting for JSON fixture {}", path.display());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_until_exists(path: &Path, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while !path.exists() {
        if Instant::now() >= deadline {
            bail!("timed out waiting for {}", path.display());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

fn process_state(pid: u32) -> Result<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let close = stat
        .rfind(')')
        .context("process stat has no closing command delimiter")?;
    stat[close + 1..]
        .split_whitespace()
        .next()
        .map(ToOwned::to_owned)
        .context("process stat has no state field")
}

fn wait_process_state(pid: u32, stopped: bool, timeout: Duration) -> Result<String> {
    let deadline = Instant::now() + timeout;
    loop {
        let state = process_state(pid)?;
        if (stopped && matches!(state.as_str(), "T" | "t"))
            || (!stopped && !matches!(state.as_str(), "T" | "t"))
        {
            return Ok(state);
        }
        if Instant::now() >= deadline {
            bail!("timed out waiting for process {pid} state (last state {state})");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn probe(addr: SocketAddr, timeout: Duration) -> ProbeEvidence {
    let start = Instant::now();
    match TcpStream::connect_timeout(&addr, timeout) {
        Ok(mut stream) => {
            let mut response = [0u8; 4];
            let result = (|| -> std::io::Result<()> {
                stream.set_read_timeout(Some(timeout))?;
                std::io::Write::write_all(&mut stream, b"ping")?;
                std::io::Read::read_exact(&mut stream, &mut response)
            })();
            match result {
                Ok(()) if &response == b"pong" => ProbeEvidence {
                    target: CandidateLetter::A,
                    addr,
                    label: String::new(),
                    reachable: true,
                    latency_ms: Some(start.elapsed().as_millis()),
                    error: None,
                },
                Ok(()) => ProbeEvidence {
                    target: CandidateLetter::A,
                    addr,
                    label: String::new(),
                    reachable: false,
                    latency_ms: Some(start.elapsed().as_millis()),
                    error: Some("unexpected probe response".into()),
                },
                Err(error) => ProbeEvidence {
                    target: CandidateLetter::A,
                    addr,
                    label: String::new(),
                    reachable: false,
                    latency_ms: Some(start.elapsed().as_millis()),
                    error: Some(error.to_string()),
                },
            }
        }
        Err(error) => ProbeEvidence {
            target: CandidateLetter::A,
            addr,
            label: String::new(),
            reachable: false,
            latency_ms: Some(start.elapsed().as_millis()),
            error: Some(error.to_string()),
        },
    }
}

fn labeled_probe(
    letter: CandidateLetter,
    addr: SocketAddr,
    label: &str,
    timeout: Duration,
) -> ProbeEvidence {
    let mut evidence = probe(addr, timeout);
    evidence.target = letter;
    evidence.label = label.into();
    evidence
}

fn run_command(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("failed to launch {program}"))?;
    if !status.success() {
        bail!("{program} {} failed with {status}", args.join(" "));
    }
    Ok(())
}

fn apply_netns_partition(netns: &str) -> Result<()> {
    let _ = Command::new("ip")
        .args([
            "netns",
            "exec",
            netns,
            "nft",
            "delete",
            "table",
            "inet",
            "fuckipv6_w14",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    run_command(
        "ip",
        &[
            "netns",
            "exec",
            netns,
            "nft",
            "add",
            "table",
            "inet",
            "fuckipv6_w14",
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
            "chain",
            "inet",
            "fuckipv6_w14",
            "output",
            "{ type filter hook output priority 0; policy accept; }",
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
            "fuckipv6_w14",
            "output",
            "drop",
        ],
    )
}

fn restore_netns_partition(netns: &str) -> Result<()> {
    run_command(
        "ip",
        &[
            "netns",
            "exec",
            netns,
            "nft",
            "delete",
            "table",
            "inet",
            "fuckipv6_w14",
        ],
    )
}

fn status_term_evidence(
    status: &fuckipv6_common::domain::CoordinatorCandidateStatus,
) -> TermEvidence {
    let term = status
        .latest_term
        .as_ref()
        .expect("term evidence requires latest term");
    TermEvidence {
        term: term.term,
        leader: term.leader.clone(),
        fencing_token: term.fencing_token.0,
        control_plane_state: status.control_plane.state,
        governance_writable: status.control_plane.governance_writable,
    }
}

fn publish_declaration(
    fixture: &Fixture,
    letter: CandidateLetter,
    willingness: CoordinatorCandidateWillingness,
) -> Result<StoreOperationResult> {
    let now = unix_time_ms();
    let declaration = fixture.declaration(letter, willingness, now)?;
    fixture.run_store_job(
        StoreOperation::PublishDeclaration {
            actor: fixture.candidate_id(letter),
            declaration,
            now_unix_ms: now,
        },
        fixture
            .placement
            .as_ref()
            .map(|placement| placement.netns(letter)),
    )
}

fn elect(
    fixture: &Fixture,
    proposer: CandidateLetter,
    voters: &[CandidateLetter],
    term: u64,
) -> Result<fuckipv6_common::domain::CoordinatorCandidateStatus> {
    let now = unix_time_ms();
    let proposal = fixture.proposal(proposer, term, now)?;
    let netns = fixture
        .placement
        .as_ref()
        .map(|placement| placement.netns(proposer));
    let proposed = fixture.run_store_job(
        StoreOperation::ProposeElection {
            actor: fixture.candidate_id(proposer),
            proposal: proposal.clone(),
            now_unix_ms: now,
        },
        netns,
    )?;
    if !proposed.ok {
        bail!(
            "candidate {} proposal failed: {}",
            proposer.label(),
            proposed.error.unwrap_or_default()
        );
    }

    let mut latest = proposed.status.context("proposal returned no status")?;
    for voter in voters {
        let now = unix_time_ms();
        let vote = fixture.vote(*voter, &proposal, now)?;
        let result = fixture.run_store_job(
            StoreOperation::RecordElectionVote {
                actor: fixture.candidate_id(*voter),
                proposal_id: proposal.proposal_id.clone(),
                vote,
                now_unix_ms: now,
            },
            fixture
                .placement
                .as_ref()
                .map(|placement| placement.netns(*voter)),
        )?;
        if !result.ok {
            bail!(
                "candidate {} vote failed: {}",
                voter.label(),
                result.error.unwrap_or_default()
            );
        }
        latest = result.status.context("vote returned no status")?;
    }
    if latest
        .latest_election
        .as_ref()
        .is_some_and(|election| election.stage != CoordinatorElectionStage::Committed)
    {
        bail!("election did not commit");
    }
    Ok(latest)
}

fn wait_until_unix_ms(deadline: u64) {
    while unix_time_ms() < deadline {
        let remaining = deadline.saturating_sub(unix_time_ms());
        std::thread::sleep(Duration::from_millis(remaining.min(100)));
    }
}

fn run_real_fault_scenario(mode: &str, require_netns: bool) -> Result<RealFaultReport> {
    let placement = NetnsPlacement::from_env()?;
    if require_netns && placement.is_none() {
        bail!("netns fault scenario requires FUCKIPV6_W14_NETNS_MODE=1");
    }
    let fixture = Fixture::new(mode, placement)?;
    let mut faults = Vec::new();
    let mut term_trace = Vec::new();

    let update_now = unix_time_ms();
    let update = fixture.run_store_job(
        StoreOperation::UpdateConfig {
            actor: fixture.owner_id(),
            expected_revision: 0,
            config: fixture.candidate_config.clone(),
            now_unix_ms: update_now,
        },
        None,
    )?;
    if !update.ok {
        bail!(
            "candidate config bootstrap failed: {}",
            update.error.unwrap_or_default()
        );
    }

    let mut servers = Vec::new();
    for letter in [CandidateLetter::A, CandidateLetter::B, CandidateLetter::C] {
        servers.push((letter, fixture.spawn_probe_server(letter)?));
    }
    let process_pids = servers
        .iter()
        .map(|(letter, server)| (*letter, server.pid()))
        .collect::<BTreeMap<_, _>>();

    let probes_before = servers
        .iter()
        .map(|(letter, server)| labeled_probe(*letter, server.addr, "before-fault", PROBE_TIMEOUT))
        .collect::<Vec<_>>();
    if probes_before.iter().any(|probe| !probe.reachable) {
        bail!("candidate probe servers are not all reachable before fault injection");
    }

    let mut probes_during_partition = Vec::new();
    let mut probes_after_restore = Vec::new();
    if let Some(placement) = &fixture.placement {
        let netns_a = placement.netns(CandidateLetter::A);
        apply_netns_partition(netns_a)?;
        faults.push(FaultEvent {
            at_unix_ms: unix_time_ms(),
            kind: "netns_netfilter_partition".into(),
            target: netns_a.into(),
            detail: "drop all candidate-a namespace egress traffic".into(),
        });
        let server_a_addr = servers
            .iter()
            .find(|(letter, _)| *letter == CandidateLetter::A)
            .map(|(_, server)| server.addr)
            .expect("candidate A server exists");
        probes_during_partition.push(labeled_probe(
            CandidateLetter::A,
            server_a_addr,
            "netfilter-drop-all",
            Duration::from_millis(700),
        ));
        if probes_during_partition[0].reachable {
            bail!("candidate A remained reachable while its namespace egress was dropped");
        }
    }

    for letter in [CandidateLetter::A, CandidateLetter::B, CandidateLetter::C] {
        let result =
            publish_declaration(&fixture, letter, CoordinatorCandidateWillingness::Willing)?;
        if !result.ok {
            bail!(
                "candidate {} declaration failed: {}",
                letter.label(),
                result.error.unwrap_or_default()
            );
        }
    }

    let now = unix_time_ms();
    let term_one = fixture.proposal(CandidateLetter::A, 1, now)?;
    let proposed = fixture.run_store_job(
        StoreOperation::ProposeElection {
            actor: fixture.candidate_id(CandidateLetter::A),
            proposal: term_one.clone(),
            now_unix_ms: now,
        },
        fixture
            .placement
            .as_ref()
            .map(|placement| placement.netns(CandidateLetter::A)),
    )?;
    if !proposed.ok {
        bail!(
            "term 1 proposal failed: {}",
            proposed.error.unwrap_or_default()
        );
    }
    let mut status_one = proposed
        .status
        .context("term 1 proposal returned no status")?;
    for voter in [CandidateLetter::A, CandidateLetter::B] {
        let now = unix_time_ms();
        let vote = fixture.vote(voter, &term_one, now)?;
        let result = fixture.run_store_job(
            StoreOperation::RecordElectionVote {
                actor: fixture.candidate_id(voter),
                proposal_id: term_one.proposal_id.clone(),
                vote,
                now_unix_ms: now,
            },
            fixture
                .placement
                .as_ref()
                .map(|placement| placement.netns(voter)),
        )?;
        if !result.ok {
            bail!(
                "term 1 vote by {} failed: {}",
                voter.label(),
                result.error.unwrap_or_default()
            );
        }
        status_one = result.status.context("term 1 vote returned no status")?;
    }
    let term_one = status_one
        .latest_term
        .clone()
        .context("term 1 did not install")?;
    if term_one.leader != fixture.candidate_id(CandidateLetter::A) || term_one.fencing_token.0 != 1
    {
        bail!("term 1 did not install candidate A with fencing token 1");
    }
    term_trace.push(status_term_evidence(&status_one));
    let old_credential = CoordinatorWriteCredential {
        network_id: fixture.network.network_id.clone(),
        term: term_one.term,
        leader: term_one.leader.clone(),
        fencing_token: term_one.fencing_token,
    };

    let server_a_pid = *process_pids
        .get(&CandidateLetter::A)
        .expect("candidate A process exists");
    servers
        .iter()
        .find(|(letter, _)| *letter == CandidateLetter::A)
        .expect("candidate A server exists")
        .1
        .signal("STOP")?;
    let state_after_stop = wait_process_state(server_a_pid, true, Duration::from_secs(2))?;
    faults.push(FaultEvent {
        at_unix_ms: unix_time_ms(),
        kind: "sigstop".into(),
        target: format!("candidate-a:{server_a_pid}"),
        detail: format!("candidate process state {state_after_stop}"),
    });

    wait_until_unix_ms(term_one.expires_at_unix_ms.saturating_add(100));
    let status_two = elect(
        &fixture,
        CandidateLetter::B,
        &[CandidateLetter::B, CandidateLetter::C],
        2,
    )?;
    let term_two = status_two
        .latest_term
        .clone()
        .context("term 2 did not install")?;
    if term_two.leader != fixture.candidate_id(CandidateLetter::B) || term_two.fencing_token.0 != 2
    {
        bail!("term 2 did not install candidate B with fencing token 2");
    }
    term_trace.push(status_term_evidence(&status_two));

    let old_rejected = fixture.run_store_job(
        StoreOperation::AuthorizeGovernance {
            actor: fixture.candidate_id(CandidateLetter::A),
            credential: old_credential,
            write: CoordinatorWriteKind::Membership,
            now_unix_ms: unix_time_ms(),
        },
        None,
    )?;
    if old_rejected.ok {
        bail!("old term 1 credential still authorized governance after term 2");
    }
    let new_credential = CoordinatorWriteCredential {
        network_id: fixture.network.network_id.clone(),
        term: term_two.term,
        leader: term_two.leader.clone(),
        fencing_token: term_two.fencing_token,
    };
    let new_authorized = fixture.run_store_job(
        StoreOperation::AuthorizeGovernance {
            actor: fixture.candidate_id(CandidateLetter::B),
            credential: new_credential.clone(),
            write: CoordinatorWriteKind::Membership,
            now_unix_ms: unix_time_ms(),
        },
        None,
    )?;
    if !new_authorized.ok {
        bail!(
            "term 2 credential failed to authorize governance: {}",
            new_authorized.error.unwrap_or_default()
        );
    }

    if let Some(placement) = &fixture.placement {
        let netns_a = placement.netns(CandidateLetter::A);
        restore_netns_partition(netns_a)?;
        faults.push(FaultEvent {
            at_unix_ms: unix_time_ms(),
            kind: "netns_netfilter_restore".into(),
            target: netns_a.into(),
            detail: "candidate-a namespace egress restored".into(),
        });
    }

    servers
        .iter()
        .find(|(letter, _)| *letter == CandidateLetter::A)
        .expect("candidate A server exists")
        .1
        .signal("CONT")?;
    wait_process_state(server_a_pid, false, Duration::from_secs(2))?;
    let recovered_after_cont = true;
    faults.push(FaultEvent {
        at_unix_ms: unix_time_ms(),
        kind: "sigcont".into(),
        target: format!("candidate-a:{server_a_pid}"),
        detail: "candidate process resumed from SIGSTOP".into(),
    });

    if let Some(placement) = &fixture.placement {
        let netns_a = placement.netns(CandidateLetter::A);
        let server_a_addr = servers
            .iter()
            .find(|(letter, _)| *letter == CandidateLetter::A)
            .map(|(_, server)| server.addr)
            .expect("candidate A server exists");
        probes_after_restore.push(labeled_probe(
            CandidateLetter::A,
            server_a_addr,
            "after-netfilter-restore",
            PROBE_TIMEOUT,
        ));
        if !probes_after_restore[0].reachable {
            bail!("candidate A did not become reachable after netfilter restore on {netns_a}");
        }
    }

    let recovery = fixture.run_store_job(
        StoreOperation::Recover {
            actor: fixture.candidate_id(CandidateLetter::A),
            report: CoordinatorRecoveryReport {
                network_id: fixture.network.network_id.clone(),
                observed_term: Some(term_one.term),
                observed_fencing_token: Some(term_one.fencing_token),
                observed_leader: Some(term_one.leader.clone()),
                observed_candidate_config_revision: Some(fixture.candidate_config.revision),
                observed_audit_sequence: 0,
            },
            now_unix_ms: unix_time_ms(),
        },
        fixture
            .placement
            .as_ref()
            .map(|placement| placement.netns(CandidateLetter::A)),
    )?;
    if !recovery.ok || recovery.recovery_stage != Some(CoordinatorRecoveryStage::ReadOnly) {
        bail!("stale old Leader did not rejoin in read-only recovery stage");
    }
    let stale_rejoin_read_only = true;

    let unwilling = publish_declaration(
        &fixture,
        CandidateLetter::A,
        CoordinatorCandidateWillingness::Unwilling,
    )?;
    if !unwilling.ok {
        bail!("candidate A unwilling declaration failed");
    }

    for (letter, server) in &mut servers {
        server.kill_and_wait()?;
        faults.push(FaultEvent {
            at_unix_ms: unix_time_ms(),
            kind: "sigkill".into(),
            target: format!("{}:{}", letter.label(), server.pid()),
            detail: "candidate process terminated abruptly".into(),
        });
    }

    wait_until_unix_ms(term_two.expires_at_unix_ms.saturating_add(100));
    let emergency_now = unix_time_ms();
    let emergency_result = fixture.run_store_job(
        StoreOperation::IssueEmergencyLeader {
            actor: fixture.ordinary_id(),
            network_id: fixture.network.network_id.clone(),
            online_members: vec![fixture.ordinary_id()],
            now_unix_ms: emergency_now,
        },
        None,
    )?;
    if !emergency_result.ok {
        bail!(
            "emergency Leader issuance failed: {}",
            emergency_result.error.unwrap_or_default()
        );
    }
    let emergency_status = emergency_result
        .status
        .as_ref()
        .context("emergency issuance returned no status")?;
    let emergency = emergency_status
        .emergency_leader
        .clone()
        .context("emergency issuance returned no lease")?;
    if emergency.leader != fixture.ordinary_id()
        || !emergency.runtime_coordination_allowed
        || emergency.governance_writable
        || !emergency.data_plane_allowed
        || emergency_status.emergency != CoordinatorEmergencyState::AllCandidatesUnavailable
    {
        bail!("emergency authority projection exceeded runtime-only scope");
    }
    faults.push(FaultEvent {
        at_unix_ms: emergency_now,
        kind: "all_candidates_lost".into(),
        target: "candidate-a,b,c".into(),
        detail: format!(
            "emergency epoch {} issued to ordinary member",
            emergency.emergency_epoch
        ),
    });
    let emergency_credential = CoordinatorEmergencyWriteCredential {
        network_id: fixture.network.network_id.clone(),
        leader: emergency.leader.clone(),
        emergency_epoch: emergency.emergency_epoch,
        candidate_config_revision: emergency.candidate_config_revision,
    };
    if emergency
        .authorize_runtime_write(
            &fixture.ordinary_id(),
            &emergency_credential,
            CoordinatorWriteKind::ConnectionCoordination,
            emergency_now.saturating_add(1),
        )
        .is_err()
    {
        bail!("emergency runtime coordination credential was not usable");
    }
    let governance_rejected = fixture.run_store_job(
        StoreOperation::RejectEmergencyGovernance {
            actor: fixture.ordinary_id(),
            network_id: fixture.network.network_id.clone(),
            action: "membership.update".into(),
            now_unix_ms: emergency_now.saturating_add(1),
        },
        None,
    )?;
    if !governance_rejected.ok || governance_rejected.rejected != Some(true) {
        bail!("emergency Leader was not frozen for governance writes");
    }

    let mut restarted_b = fixture.spawn_probe_server(CandidateLetter::B)?;
    let mut restarted_c = fixture.spawn_probe_server(CandidateLetter::C)?;
    for letter in [CandidateLetter::B, CandidateLetter::C] {
        let result =
            publish_declaration(&fixture, letter, CoordinatorCandidateWillingness::Willing)?;
        if !result.ok {
            bail!(
                "recovered candidate {} declaration failed: {}",
                letter.label(),
                result.error.unwrap_or_default()
            );
        }
    }

    let install_now = unix_time_ms();
    let term_b = fixture.manual_term(
        3,
        3,
        CandidateLetter::B,
        &[CandidateLetter::B, CandidateLetter::C],
        install_now,
    )?;
    let term_c = fixture.manual_term(
        3,
        3,
        CandidateLetter::C,
        &[CandidateLetter::B, CandidateLetter::C],
        install_now,
    )?;
    let gate = fixture.unique_path("split-brain-gate");
    let mut install_b = fixture.spawn_store_job(
        StoreOperation::InstallTerm {
            actor: fixture.candidate_id(CandidateLetter::B),
            term: term_b,
            now_unix_ms: install_now,
        },
        fixture
            .placement
            .as_ref()
            .map(|placement| placement.netns(CandidateLetter::B)),
        Some(gate.clone()),
    )?;
    let mut install_c = fixture.spawn_store_job(
        StoreOperation::InstallTerm {
            actor: fixture.candidate_id(CandidateLetter::C),
            term: term_c,
            now_unix_ms: install_now,
        },
        fixture
            .placement
            .as_ref()
            .map(|placement| placement.netns(CandidateLetter::C)),
        Some(gate.clone()),
    )?;
    install_b.wait_started()?;
    install_c.wait_started()?;
    std::fs::write(&gate, b"go")?;
    let install_b = install_b.wait_result()?;
    let install_c = install_c.wait_result()?;
    let concurrent_successes = [install_b.ok, install_c.ok]
        .into_iter()
        .filter(|ok| *ok)
        .count();
    let concurrent_rejections = 2 - concurrent_successes;
    if concurrent_successes != 1 || concurrent_rejections != 1 {
        bail!(
            "concurrent split-brain installs were not single-winner: successes={concurrent_successes}, rejections={concurrent_rejections}, b={:?}, c={:?}",
            install_b.error,
            install_c.error
        );
    }
    let winner_status = if install_b.ok {
        install_b.status.as_ref()
    } else {
        install_c.status.as_ref()
    }
    .context("winning split-brain install returned no status")?;
    let term_three = winner_status
        .latest_term
        .as_ref()
        .context("term 3 did not install")?;
    if term_three.term != 3
        || term_three.fencing_token.0 != 3
        || winner_status.emergency_leader.is_some()
        || !winner_status.control_plane.governance_writable
        || winner_status.control_plane.state != CoordinatorControlPlaneState::Available
    {
        bail!("formal successor term did not restore exclusive writable authority");
    }
    term_trace.push(status_term_evidence(winner_status));
    let winner_credential = CoordinatorWriteCredential {
        network_id: fixture.network.network_id.clone(),
        term: term_three.term,
        leader: term_three.leader.clone(),
        fencing_token: term_three.fencing_token,
    };
    let winner_authorized = fixture.run_store_job(
        StoreOperation::AuthorizeGovernance {
            actor: term_three.leader.clone(),
            credential: winner_credential,
            write: CoordinatorWriteKind::Membership,
            now_unix_ms: unix_time_ms(),
        },
        None,
    )?;
    if !winner_authorized.ok {
        bail!("formal successor credential did not authorize governance");
    }
    let old_term_two_rejected = fixture.run_store_job(
        StoreOperation::AuthorizeGovernance {
            actor: fixture.candidate_id(CandidateLetter::B),
            credential: new_credential,
            write: CoordinatorWriteKind::Membership,
            now_unix_ms: unix_time_ms(),
        },
        None,
    )?;
    if old_term_two_rejected.ok {
        bail!("term 2 credential remained writable after term 3");
    }

    restarted_b.kill_and_wait()?;
    restarted_c.kill_and_wait()?;

    let final_status = fixture
        .run_store_job(
            StoreOperation::Status {
                actor: fixture.candidate_id(CandidateLetter::A),
                network_id: fixture.network.network_id.clone(),
                now_unix_ms: unix_time_ms(),
            },
            None,
        )?
        .status
        .context("final status query returned no status")?;
    if final_status.config_state != CoordinatorCandidateConfigState::Current {
        bail!("final candidate configuration is not current");
    }
    let audit_actions = fixture
        .store()?
        .audit_entries()?
        .into_iter()
        .map(|entry| entry.action)
        .collect::<Vec<_>>();
    for required in [
        "coordinator.election.committed",
        "coordinator.emergency.leader.issued",
        "coordinator.emergency.governance.rejected",
        "coordinator.recovery.old_leader",
        "coordinator.term.install",
        "coordinator.term.install.rejected",
    ] {
        if !audit_actions.iter().any(|action| action == required) {
            bail!("audit trail is missing {required}");
        }
    }

    let processes = process_pids
        .iter()
        .map(|(letter, pid)| ProcessEvidence {
            candidate: *letter,
            node_id: fixture.candidate_id(*letter),
            pid: *pid,
            probe_addr: servers
                .iter()
                .find(|(candidate, _)| candidate == letter)
                .map(|(_, server)| server.addr)
                .expect("server evidence exists"),
            process_state_after_sigstop: (*letter == CandidateLetter::A)
                .then(|| state_after_stop.clone()),
            recovered_after_sigcont: (*letter == CandidateLetter::A)
                .then_some(recovered_after_cont),
        })
        .collect::<Vec<_>>();

    let report = RealFaultReport {
        schema: "fuckipv6.w14.fault-injection.v3".into(),
        mode: mode.into(),
        network_id: fixture.network.network_id.clone(),
        candidate_group: fixture.candidate_group(),
        processes,
        probes_before,
        probes_during_partition,
        probes_after_restore,
        faults,
        term_trace,
        emergency: Some(serde_json::to_value(emergency)?),
        emergency_governance_rejected: true,
        old_leader_credential_rejected: true,
        stale_rejoin_read_only,
        concurrent_successes,
        concurrent_rejections,
        final_status,
        audit_actions,
    };
    if let Ok(path) = std::env::var(REPORT_ENV) {
        write_json_atomic(Path::new(&path), &report)?;
    }
    Ok(report)
}

fn exercise_abrupt_lock_owner_loss(fixture: &Fixture) -> Result<()> {
    let mut holder = fixture.spawn_lock_holder()?;
    let mut blocked = fixture.spawn_store_job(
        StoreOperation::Status {
            actor: fixture.candidate_id(CandidateLetter::A),
            network_id: fixture.network.network_id.clone(),
            now_unix_ms: unix_time_ms(),
        },
        None,
        None,
    )?;
    blocked.wait_started()?;
    std::thread::sleep(Duration::from_millis(300));
    if blocked.output_path.exists() {
        bail!("status unexpectedly bypassed the held cross-process candidate lock");
    }
    holder.kill_and_wait()?;
    let result = blocked.wait_result()?;
    if !result.ok {
        bail!("store did not recover after its lock-owning process was SIGKILLed");
    }

    let holder = fixture.spawn_lock_holder()?;
    let holder_pid = holder.child.id();
    let kill_status = Command::new("kill")
        .arg("-STOP")
        .arg(holder_pid.to_string())
        .status()?;
    if !kill_status.success() {
        bail!("failed to SIGSTOP lock holder");
    }
    wait_process_state(holder_pid, true, Duration::from_secs(2))?;
    let mut blocked = fixture.spawn_store_job(
        StoreOperation::Status {
            actor: fixture.candidate_id(CandidateLetter::B),
            network_id: fixture.network.network_id.clone(),
            now_unix_ms: unix_time_ms(),
        },
        None,
        None,
    )?;
    blocked.wait_started()?;
    std::thread::sleep(Duration::from_millis(300));
    if blocked.output_path.exists() {
        bail!("status unexpectedly completed while the lock owner was SIGSTOPped");
    }
    let cont_status = Command::new("kill")
        .arg("-CONT")
        .arg(holder_pid.to_string())
        .status()?;
    if !cont_status.success() {
        bail!("failed to SIGCONT lock holder");
    }
    wait_process_state(holder_pid, false, Duration::from_secs(2))?;
    holder.release()?;
    let status = std::fs::read(format!("/proc/{holder_pid}/stat")).is_ok();
    if status {
        // Allow the child test harness to observe the release file and exit.
        std::thread::sleep(Duration::from_millis(100));
    }
    let result = blocked.wait_result()?;
    if !result.ok {
        bail!("store did not resume after its lock owner received SIGCONT");
    }
    Ok(())
}

fn assert_netns_report(report: &RealFaultReport) {
    assert_eq!(report.schema, "fuckipv6.w14.fault-injection.v3");
    assert_eq!(report.probes_before.len(), 3);
    assert!(report.probes_before.iter().all(|probe| probe.reachable));
    assert_eq!(report.probes_during_partition.len(), 1);
    assert!(!report.probes_during_partition[0].reachable);
    assert_eq!(report.probes_after_restore.len(), 1);
    assert!(report.probes_after_restore[0].reachable);
    assert!(report.faults.iter().any(|fault| fault.kind == "sigstop"));
    assert!(report.faults.iter().any(|fault| fault.kind == "sigkill"));
    assert!(report
        .faults
        .iter()
        .any(|fault| fault.kind == "netns_netfilter_partition"));
}

#[test]
fn w14_real_process_faults_install_one_authority_and_recover() -> Result<()> {
    let fixture = Fixture::new("lock-crash", None)?;
    exercise_abrupt_lock_owner_loss(&fixture)?;
    let report = run_real_fault_scenario("real-multiprocess", false)?;
    assert_eq!(report.concurrent_successes, 1);
    assert_eq!(report.concurrent_rejections, 1);
    assert!(report.old_leader_credential_rejected);
    assert!(report.stale_rejoin_read_only);
    assert!(report.emergency_governance_rejected);
    Ok(())
}

#[test]
#[ignore = "requires root, three network namespaces, and nftables"]
fn w14_netns_netfilter_fault_injection_records_real_process_evidence() -> Result<()> {
    let report = run_real_fault_scenario("netns-netfilter-multiprocess", true)?;
    assert_netns_report(&report);
    Ok(())
}

#[test]
#[ignore = "prepares the QEMU power-cut fixture through scripts/test-w14-coordinator-powerloss-vm.sh"]
fn w14_vm_powerloss_prepare_fixture() -> Result<()> {
    let fixture_dir = std::env::var_os("FUCKIPV6_W14_VM_FIXTURE_DIR")
        .map(PathBuf::from)
        .context("missing FUCKIPV6_W14_VM_FIXTURE_DIR")?;
    std::fs::create_dir_all(&fixture_dir)?;

    let fixture = Fixture::new("vm-powerloss", None)?;
    let update_now = unix_time_ms();
    let job = ChildJob::Store {
        store_path: PathBuf::from("/work/coordinator_candidates.json"),
        network_path: PathBuf::from("/work/network.json"),
        output_path: PathBuf::from("/work/output.json"),
        started_path: PathBuf::from("/work/started"),
        gate_path: None,
        operation: Box::new(StoreOperation::UpdateConfig {
            actor: fixture.owner_id(),
            expected_revision: 0,
            config: fixture.candidate_config.clone(),
            now_unix_ms: update_now,
        }),
    };

    std::fs::copy(&fixture.network_path, fixture_dir.join("network.json"))?;
    std::fs::write(
        fixture_dir.join("job.json"),
        serde_json::to_vec_pretty(&job)?,
    )?;
    std::fs::write(
        fixture_dir.join("metadata.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "network_id": fixture.network.network_id,
            "actor": fixture.owner_id(),
            "expected_config_revision": 1,
            "candidate_config_revision": fixture.candidate_config.revision,
        }))?,
    )?;
    Ok(())
}

#[test]
fn w14_process_child_fixture() -> Result<()> {
    if std::env::var(CHILD_ENV).ok().as_deref() != Some("1") {
        return Ok(());
    }
    let job_path = std::env::var_os(JOB_ENV).context("missing W14 child job path")?;
    let job: ChildJob = serde_json::from_slice(&std::fs::read(job_path)?)?;
    match job {
        ChildJob::Store {
            store_path,
            network_path,
            output_path,
            started_path,
            gate_path,
            operation,
        } => {
            std::fs::write(&started_path, b"started")?;
            if let Some(gate_path) = gate_path {
                wait_until_exists(&gate_path, CHILD_TIMEOUT)?;
            }
            let store = CoordinatorCandidateStore::new(store_path, network_path)?;
            let result = match *operation {
                StoreOperation::UpdateConfig {
                    actor,
                    expected_revision,
                    config,
                    now_unix_ms,
                } => store
                    .update_config(&actor, expected_revision, config, now_unix_ms)
                    .map(|status| StoreOperationResult {
                        ok: true,
                        error: None,
                        status: Some(status),
                        recovery_stage: None,
                        rejected: None,
                    }),
                StoreOperation::PublishDeclaration {
                    actor,
                    declaration,
                    now_unix_ms,
                } => store
                    .publish_declaration(&actor, declaration, now_unix_ms)
                    .map(|status| StoreOperationResult {
                        ok: true,
                        error: None,
                        status: Some(status),
                        recovery_stage: None,
                        rejected: None,
                    }),
                StoreOperation::ProposeElection {
                    actor,
                    proposal,
                    now_unix_ms,
                } => store
                    .propose_election(&actor, proposal, now_unix_ms)
                    .map(|status| StoreOperationResult {
                        ok: true,
                        error: None,
                        status: Some(status),
                        recovery_stage: None,
                        rejected: None,
                    }),
                StoreOperation::RecordElectionVote {
                    actor,
                    proposal_id,
                    vote,
                    now_unix_ms,
                } => store
                    .record_election_vote(&actor, &proposal_id, vote, now_unix_ms)
                    .map(|status| StoreOperationResult {
                        ok: true,
                        error: None,
                        status: Some(status),
                        recovery_stage: None,
                        rejected: None,
                    }),
                StoreOperation::InstallTerm {
                    actor,
                    term,
                    now_unix_ms,
                } => store.install_term(&actor, term, now_unix_ms).map(|status| {
                    StoreOperationResult {
                        ok: true,
                        error: None,
                        status: Some(status),
                        recovery_stage: None,
                        rejected: None,
                    }
                }),
                StoreOperation::Status {
                    actor,
                    network_id,
                    now_unix_ms,
                } => store
                    .status(&actor, &network_id, now_unix_ms)
                    .map(|status| StoreOperationResult {
                        ok: true,
                        error: None,
                        status: Some(status),
                        recovery_stage: None,
                        rejected: None,
                    }),
                StoreOperation::Recover {
                    actor,
                    report,
                    now_unix_ms,
                } => store.recover(&actor, report, now_unix_ms).map(|response| {
                    StoreOperationResult {
                        ok: true,
                        error: None,
                        status: Some(response.status),
                        recovery_stage: Some(response.recovery.stage),
                        rejected: None,
                    }
                }),
                StoreOperation::AuthorizeGovernance {
                    actor,
                    credential,
                    write,
                    now_unix_ms,
                } => store
                    .authorize_governance_write(&actor, &credential, write, now_unix_ms)
                    .map(|_| StoreOperationResult {
                        ok: true,
                        error: None,
                        status: None,
                        recovery_stage: None,
                        rejected: None,
                    }),
                StoreOperation::IssueEmergencyLeader {
                    actor,
                    network_id,
                    online_members,
                    now_unix_ms,
                } => store
                    .issue_emergency_leader(&actor, &network_id, &online_members, now_unix_ms)
                    .map(|response| StoreOperationResult {
                        ok: true,
                        error: None,
                        status: Some(response.status),
                        recovery_stage: None,
                        rejected: None,
                    }),
                StoreOperation::RejectEmergencyGovernance {
                    actor,
                    network_id,
                    action,
                    now_unix_ms,
                } => store
                    .reject_emergency_governance_write(&actor, &network_id, &action, now_unix_ms)
                    .map(|rejected| StoreOperationResult {
                        ok: true,
                        error: None,
                        status: None,
                        recovery_stage: None,
                        rejected: Some(rejected),
                    }),
            };
            let result = match result {
                Ok(result) => result,
                Err(error) => StoreOperationResult {
                    ok: false,
                    error: Some(error.to_string()),
                    status: None,
                    recovery_stage: None,
                    rejected: None,
                },
            };
            write_json_atomic(&output_path, &result)?;
        }
        ChildJob::HoldCandidateLock {
            lock_path,
            ready_path,
            release_path,
            done_path,
        } => {
            let lock = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(lock_path)?;
            lock.lock_exclusive()?;
            write_json_atomic(&ready_path, &serde_json::json!({"locked": true}))?;
            wait_until_exists(&release_path, Duration::from_secs(60))?;
            FileExt::unlock(&lock)?;
            std::fs::write(done_path, b"done")?;
        }
        ChildJob::ServeProbe {
            bind_addr,
            ready_path,
        } => {
            let listener = TcpListener::bind(bind_addr)?;
            write_json_atomic(
                &ready_path,
                &ReadyAddr {
                    addr: listener.local_addr()?,
                },
            )?;
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else {
                    continue;
                };
                std::thread::spawn(move || {
                    let mut request = [0u8; 4];
                    if std::io::Read::read_exact(&mut stream, &mut request).is_ok()
                        && &request == b"ping"
                    {
                        let _ = std::io::Write::write_all(&mut stream, b"pong");
                    }
                });
            }
        }
    }
    Ok(())
}
