#![cfg(target_os = "linux")]

use fuckipv6_client::routing::linux_route::LinuxRouteManager;
use fuckipv6_client::routing::recovery::{
    DnsRecoveryMode, FailureApplication, InvalidatedPath, PathRecoveryState, RecoveryBackend,
    RecoveryCoordinator, RecoveryOperationError, RecoveryProbeObservation, RecoveryProbeStage,
    RecoveryProbeState, RecoveryProbeTarget, RecoveryRepairReport, RecoveryRequest,
    RecoveryResourceSnapshot, RecoveryResourceState, RecoveryTrigger, RouteRecoverySnapshot,
    TunRecoverySnapshot,
};
use fuckipv6_client::tun::TunDevice;
use fuckipv6_common::domain::{
    NetworkId, TrafficAction, TrafficFailureKind, TrafficFailurePolicy, TrafficFailureRouteContext,
    TrafficRuntimeDegradationState,
};
use fuckipv6_common::{RoutingMode, VirtualIps};
use ipnet::IpNet;
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Command, Output};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::timeout;

const TOPOLOGY_ENV: &str = "FUCKIPV6_RECOVERY_NETNS_TOPOLOGY";
const RESOLV_CONF_ENV: &str = "FUCKIPV6_RECOVERY_RESOLV_CONF";
const WORK_ENV: &str = "FUCKIPV6_RECOVERY_NETNS_WORK";
const TUN_NAME: &str = "fip6recov";
const UPLINK_NAME: &str = "fip6recup";
const BASELINE_RESOLV_CONF: &str =
    "# isolated recovery baseline\nsearch recovery.invalid\nnameserver 203.0.113.53\n";

type BackendFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, RecoveryOperationError>> + Send + 'a>>;

struct RealNetnsBackend {
    state: Mutex<BackendState>,
    resolv_conf: PathBuf,
    audit_path: PathBuf,
}

struct BackendState {
    tun: Option<TunDevice>,
    routes: Option<LinuxRouteManager>,
    table_id: u32,
    probe_addr: SocketAddr,
    control_task: Option<JoinHandle<()>>,
    exit_path: PathRecoveryState,
    relay_path: PathRecoveryState,
    probe_mode: ProbeMode,
    applications: Vec<FailureApplication>,
    global_proxy_active: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProbeMode {
    Tcp,
    DnsTimeout,
}

fn require_isolated_topology() {
    assert_eq!(
        std::env::var(TOPOLOGY_ENV).as_deref(),
        Ok("1"),
        "run through scripts/test-recovery-netns.sh"
    );
    // SAFETY: geteuid has no preconditions and does not access memory.
    assert_eq!(
        unsafe { libc::geteuid() },
        0,
        "the isolated recovery test must run as root"
    );
}

fn env_path(name: &str) -> PathBuf {
    PathBuf::from(std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set")))
}

fn ip(args: &[&str]) -> Output {
    Command::new("ip")
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("failed to execute ip {args:?}: {error}"))
}

fn link_exists(name: &str) -> bool {
    ip(&["link", "show", "dev", name]).status.success()
}

fn dns_is_managed(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|contents| contents.contains("nameserver 127.0.0.2"))
}

fn force_clear_owned_routes(table_id: u32) {
    // This test owns the private namespace. Flushing rules is deliberate after
    // an injected deletion leaves the route manager with stale ownership.
    let _ = ip(&["-4", "rule", "flush"]);
    let _ = ip(&["-4", "route", "flush", "table", &table_id.to_string()]);
}

fn assigned_ips() -> VirtualIps {
    VirtualIps::from_ipv4("10.88.0.2".parse().expect("valid test IPv4"))
}

fn virtual_cidrs() -> Vec<IpNet> {
    vec!["10.88.0.0/24".parse().expect("valid virtual CIDR")]
}

async fn shutdown_routes(state: &mut BackendState) {
    if let Some(mut routes) = state.routes.take() {
        let table_id = routes.table_id();
        let _ = routes.rollback().await;
        drop(routes);
        force_clear_owned_routes(table_id);
    }
    state.global_proxy_active = false;
}

async fn shutdown_tun(state: &mut BackendState) {
    if let Some(mut tun) = state.tun.take() {
        let _ = tun.delete().await;
    }
}

async fn snapshot_state(state: &BackendState, resolv_conf: &Path) -> RecoveryResourceSnapshot {
    let tun_present = link_exists(TUN_NAME) && state.tun.is_some();
    let routes = match state.routes.as_ref() {
        Some(routes) => routes
            .recovery_snapshot()
            .await
            .expect("isolated route snapshot must be readable"),
        None => RouteRecoverySnapshot {
            expected_routes: 1,
            installed_routes: 0,
            missing_routes: 1,
            duplicate_routes: 0,
            residual_routes: 0,
            stale_interface_routes: 0,
        },
    };
    let dns_present = dns_is_managed(resolv_conf);
    RecoveryResourceSnapshot {
        tun: TunRecoverySnapshot {
            state: if tun_present {
                RecoveryResourceState::Healthy
            } else {
                RecoveryResourceState::Missing
            },
            duplicate_interfaces: 0,
            residual_interfaces: 0,
        },
        routes,
        dns: fuckipv6_client::routing::recovery::DnsRecoverySnapshot {
            mode: if dns_present {
                DnsRecoveryMode::Doh
            } else {
                DnsRecoveryMode::SystemResolver
            },
            state: if dns_present {
                RecoveryResourceState::Healthy
            } else {
                RecoveryResourceState::Degraded
            },
            authorized_resolvers: usize::from(dns_present),
            unsecured_resolvers: usize::from(!dns_present),
            insecure_fallback_detected: false,
        },
        exit_path: state.exit_path,
        relay_path: state.relay_path,
    }
}

async fn start_control_probe() -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind isolated TCP control probe");
    let address = listener.local_addr().expect("read control probe address");
    let task = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            drop(stream);
        }
    });
    (address, task)
}

impl RealNetnsBackend {
    async fn start() -> Self {
        let resolv_conf = env_path(RESOLV_CONF_ENV);
        let work = env_path(WORK_ENV);
        let tun = TunDevice::create_dual_stack(TUN_NAME, &assigned_ips(), 24, 64, 1280)
            .await
            .expect("create recovery TUN");
        let mut routes = LinuxRouteManager::install(
            RoutingMode::GlobalProxy,
            TUN_NAME,
            &assigned_ips(),
            &virtual_cidrs(),
            &[],
            &[],
        )
        .await
        .expect("install recovery base routes");
        let table_id = routes.table_id();
        assert!(
            !routes.global_proxy_active(),
            "base routes must not intercept traffic before Exit validation"
        );
        assert!(
            !dns_is_managed(&resolv_conf),
            "DNS must remain untouched before Exit validation"
        );
        routes
            .activate_global_proxy()
            .await
            .expect("activate recovery proxy after fake Exit validation");
        let (probe_addr, control_task) = start_control_probe().await;
        let backend = Self {
            state: Mutex::new(BackendState {
                tun: Some(tun),
                routes: Some(routes),
                table_id,
                probe_addr,
                control_task: Some(control_task),
                exit_path: PathRecoveryState::Connected,
                relay_path: PathRecoveryState::Connected,
                probe_mode: ProbeMode::Tcp,
                applications: Vec::new(),
                global_proxy_active: true,
            }),
            resolv_conf,
            audit_path: work.join("recovery-audit.log"),
        };
        let snapshot = backend.snapshot_value().await;
        assert_eq!(snapshot.tun.state, RecoveryResourceState::Healthy);
        assert_eq!(snapshot.routes.installed_routes, 1);
        assert_eq!(snapshot.dns.state, RecoveryResourceState::Healthy);
        backend
    }

    async fn snapshot_value(&self) -> RecoveryResourceSnapshot {
        let state = self.state.lock().await;
        snapshot_state(&state, &self.resolv_conf).await
    }

    async fn simulate_sleep_wake(&self) {
        assert!(ip(&["link", "delete", TUN_NAME]).status.success());
    }

    async fn simulate_network_changed(&self) {
        std::fs::write(&self.resolv_conf, BASELINE_RESOLV_CONF)
            .expect("replace managed resolver with unsafe baseline");
        assert!(ip(&["link", "set", UPLINK_NAME, "down"]).status.success());
        let table_id = self.state.lock().await.table_id;
        force_clear_owned_routes(table_id);
    }

    async fn disconnect_exit(&self) {
        let mut state = self.state.lock().await;
        if let Some(task) = state.control_task.take() {
            task.abort();
            let _ = task.await;
        }
        state.exit_path = PathRecoveryState::Disconnected;
    }

    async fn disconnect_relay(&self) {
        self.state.lock().await.relay_path = PathRecoveryState::Disconnected;
    }

    async fn use_dns_timeout_probe(&self) {
        self.state.lock().await.probe_mode = ProbeMode::DnsTimeout;
    }

    async fn audit_contains_exit_disconnect(&self) -> bool {
        tokio::fs::read_to_string(&self.audit_path)
            .await
            .is_ok_and(|contents| contents.contains("exit_unavailable"))
    }

    async fn audit_contains_relay_disconnect(&self) -> bool {
        tokio::fs::read_to_string(&self.audit_path)
            .await
            .is_ok_and(|contents| contents.contains("network_selection_unavailable"))
    }

    async fn shutdown(&self) {
        let mut state = self.state.lock().await;
        shutdown_routes(&mut state).await;
        shutdown_tun(&mut state).await;
        if let Some(task) = state.control_task.take() {
            task.abort();
            let _ = task.await;
        }
        std::fs::write(&self.resolv_conf, BASELINE_RESOLV_CONF)
            .expect("restore isolated resolver baseline");
    }
}

impl RecoveryBackend for RealNetnsBackend {
    fn snapshot(&mut self) -> BackendFuture<'_, RecoveryResourceSnapshot> {
        Box::pin(async move { Ok(self.snapshot_value().await) })
    }

    fn reconcile(&mut self, _trigger: RecoveryTrigger) -> BackendFuture<'_, RecoveryRepairReport> {
        Box::pin(async move {
            let mut state = self.state.lock().await;
            let mut repair = RecoveryRepairReport::default();
            let before = snapshot_state(&state, &self.resolv_conf).await;
            let tun_present = before.tun.state == RecoveryResourceState::Healthy;
            let route_present = before.routes.missing_routes == 0
                && before.routes.duplicate_routes == 0
                && before.routes.residual_routes == 0
                && before.routes.stale_interface_routes == 0;
            let dns_present = before.dns.state == RecoveryResourceState::Healthy;
            let global_proxy_active = state.global_proxy_active;

            if !tun_present {
                shutdown_routes(&mut state).await;
                shutdown_tun(&mut state).await;
                state.tun = Some(
                    TunDevice::create_dual_stack(TUN_NAME, &assigned_ips(), 24, 64, 1280)
                        .await
                        .map_err(|_| RecoveryOperationError::new("recovery.tun_recreate_failed"))?,
                );
                repair.tun_recreated = true;
            }

            if repair.tun_recreated || !route_present || (global_proxy_active && !dns_present) {
                if let Some(routes) = state.routes.as_mut() {
                    routes.refresh_plan().await.map_err(|_| {
                        RecoveryOperationError::new("recovery.route_install_failed")
                    })?;
                } else {
                    let mut routes = LinuxRouteManager::install(
                        RoutingMode::GlobalProxy,
                        TUN_NAME,
                        &assigned_ips(),
                        &virtual_cidrs(),
                        &[],
                        &[],
                    )
                    .await
                    .map_err(|_| RecoveryOperationError::new("recovery.route_install_failed"))?;
                    if global_proxy_active {
                        routes.activate_global_proxy().await.map_err(|_| {
                            RecoveryOperationError::new("recovery.route_activate_failed")
                        })?;
                    }
                    state.table_id = routes.table_id();
                    state.routes = Some(routes);
                }
                state.global_proxy_active = global_proxy_active;
                repair.routes_restored = 1;
                repair.dns_reconfigured = global_proxy_active;
            }

            let _ = ip(&["link", "set", UPLINK_NAME, "up"]);
            Ok(repair)
        })
    }

    fn revoke_path(&mut self, path: InvalidatedPath) -> BackendFuture<'_, ()> {
        Box::pin(async move {
            let mut state = self.state.lock().await;
            match path {
                InvalidatedPath::Exit => {
                    if let Some(task) = state.control_task.take() {
                        task.abort();
                        let _ = task.await;
                    }
                    state.exit_path = PathRecoveryState::Disconnected;
                }
                InvalidatedPath::Relay => {
                    state.relay_path = PathRecoveryState::Disconnected;
                }
            }
            Ok(())
        })
    }

    fn apply_failure_decision(&mut self, application: FailureApplication) -> BackendFuture<'_, ()> {
        Box::pin(async move {
            let mut state = self.state.lock().await;
            let line = format!(
                "{}\t{}\t{:?}\n",
                application.diagnostic_id,
                application.decision.failure.as_str(),
                application.decision.outcome
            );
            let mut file = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.audit_path)
                .await
                .map_err(|_| RecoveryOperationError::new("recovery.audit_open_failed"))?;
            file.write_all(line.as_bytes())
                .await
                .map_err(|_| RecoveryOperationError::new("recovery.audit_write_failed"))?;
            state.applications.push(application);
            Ok(())
        })
    }

    fn probe(
        &mut self,
        _target: RecoveryProbeTarget,
    ) -> BackendFuture<'_, RecoveryProbeObservation> {
        Box::pin(async move {
            let (mode, address) = {
                let state = self.state.lock().await;
                (state.probe_mode, state.probe_addr)
            };
            match mode {
                ProbeMode::Tcp => {
                    match timeout(Duration::from_millis(500), TcpStream::connect(address)).await {
                        Ok(Ok(_stream)) => Ok(RecoveryProbeObservation {
                            state: RecoveryProbeState::Passed,
                            failed_stage: None,
                            error_code: None,
                        }),
                        Ok(Err(_)) => Ok(RecoveryProbeObservation {
                            state: RecoveryProbeState::Failed,
                            failed_stage: Some(RecoveryProbeStage::Tcp),
                            error_code: Some("probe.tcp_connect_failed".into()),
                        }),
                        Err(_) => Ok(RecoveryProbeObservation {
                            state: RecoveryProbeState::Failed,
                            failed_stage: Some(RecoveryProbeStage::Tcp),
                            error_code: Some("probe.tcp_timeout".into()),
                        }),
                    }
                }
                ProbeMode::DnsTimeout => {
                    // Send a syntactically valid DNS query to a local UDP sink
                    // that never responds. This exercises a real transport
                    // deadline rather than returning a synthetic failure.
                    let client = UdpSocket::bind("127.0.0.1:0")
                        .await
                        .map_err(|_| RecoveryOperationError::new("probe.dns_bind_failed"))?;
                    let sink = UdpSocket::bind("127.0.0.1:0")
                        .await
                        .map_err(|_| RecoveryOperationError::new("probe.dns_sink_failed"))?;
                    let destination = sink
                        .local_addr()
                        .map_err(|_| RecoveryOperationError::new("probe.dns_address_failed"))?;
                    let query = [
                        0x12, 0x34, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                        0x07, b't', b'i', b'm', b'e', b'o', b'u', b't', 0x00, 0x00, 0x01, 0x00,
                        0x01,
                    ];
                    client
                        .send_to(&query, destination)
                        .await
                        .map_err(|_| RecoveryOperationError::new("probe.dns_send_failed"))?;
                    let mut response = [0u8; 512];
                    match timeout(Duration::from_millis(120), client.recv_from(&mut response)).await
                    {
                        Ok(Ok(_)) => Ok(RecoveryProbeObservation {
                            state: RecoveryProbeState::Passed,
                            failed_stage: None,
                            error_code: None,
                        }),
                        Ok(Err(_)) => Ok(RecoveryProbeObservation {
                            state: RecoveryProbeState::Failed,
                            failed_stage: Some(RecoveryProbeStage::Dns),
                            error_code: Some("probe.dns_receive_failed".into()),
                        }),
                        Err(_) => Ok(RecoveryProbeObservation {
                            state: RecoveryProbeState::Failed,
                            failed_stage: Some(RecoveryProbeStage::Dns),
                            error_code: Some("probe.dns_timeout".into()),
                        }),
                    }
                }
            }
        })
    }
}

fn route_context() -> TrafficFailureRouteContext {
    TrafficFailureRouteContext::new(TrafficAction::Network(NetworkId::from_public_key(
        b"recovery-netns-network",
    )))
}

fn request(trigger: RecoveryTrigger) -> RecoveryRequest {
    RecoveryRequest::new(
        trigger,
        route_context(),
        TrafficFailurePolicy::fail_closed(),
    )
    .with_probes(vec![RecoveryProbeTarget::Controlled("tcp-control".into())])
    .with_hard_timeout_ms(3_000)
}

#[tokio::test]
#[ignore = "requires the isolated privileged topology from scripts/test-recovery-netns.sh"]
async fn native_sleep_wake_recreates_real_tun_routes_and_dns() {
    require_isolated_topology();
    let mut backend = RealNetnsBackend::start().await;
    backend.simulate_sleep_wake().await;
    let before = backend.snapshot_value().await;
    assert_eq!(before.tun.state, RecoveryResourceState::Missing);
    assert_eq!(before.routes.missing_routes, 1);

    let report = RecoveryCoordinator
        .recover(&mut backend, request(RecoveryTrigger::SleepWake))
        .await;

    assert_eq!(
        report.status,
        fuckipv6_client::routing::recovery::RecoveryStatus::Recovered
    );
    assert!(report.repair.tun_recreated);
    assert_eq!(report.repair.routes_restored, 1);
    assert!(report.repair.dns_reconfigured);
    assert_eq!(report.probes.len(), 1);
    assert_eq!(report.probes[0].state, RecoveryProbeState::Passed);
    backend.shutdown().await;
}

#[tokio::test]
#[ignore = "requires the isolated privileged topology from scripts/test-recovery-netns.sh"]
async fn native_network_change_restores_real_policy_routes_and_dns() {
    require_isolated_topology();
    let mut backend = RealNetnsBackend::start().await;
    backend.simulate_network_changed().await;
    let before = backend.snapshot_value().await;
    assert_eq!(before.routes.missing_routes, 1);
    assert_eq!(before.dns.state, RecoveryResourceState::Degraded);

    let report = RecoveryCoordinator
        .recover(&mut backend, request(RecoveryTrigger::NetworkChanged))
        .await;

    assert_eq!(
        report.status,
        fuckipv6_client::routing::recovery::RecoveryStatus::Recovered
    );
    assert_eq!(report.repair.routes_restored, 1);
    assert!(report.repair.dns_reconfigured);
    assert_eq!(
        report.after.as_ref().unwrap().dns.state,
        RecoveryResourceState::Healthy
    );
    backend.shutdown().await;
}

#[tokio::test]
#[ignore = "requires the isolated privileged topology from scripts/test-recovery-netns.sh"]
async fn native_exit_disconnect_revokes_real_path_and_fails_closed() {
    require_isolated_topology();
    let mut backend = RealNetnsBackend::start().await;
    backend.disconnect_exit().await;

    let report = RecoveryCoordinator
        .recover(&mut backend, request(RecoveryTrigger::ExitDisconnected))
        .await;

    assert_eq!(
        report.status,
        fuckipv6_client::routing::recovery::RecoveryStatus::DegradedFailClosed
    );
    assert_eq!(report.revoked_paths, vec![InvalidatedPath::Exit]);
    let decision = report.failure_decisions.first().expect("failure decision");
    assert_eq!(decision.failure, TrafficFailureKind::ExitUnavailable);
    assert_eq!(
        report.degradation.as_ref().unwrap().state,
        TrafficRuntimeDegradationState::DegradedFailClosed
    );
    assert_eq!(report.probes[0].failed_stage, Some(RecoveryProbeStage::Tcp));
    assert!(backend.audit_contains_exit_disconnect().await);
    backend.shutdown().await;
}

#[tokio::test]
#[ignore = "requires the isolated privileged topology from scripts/test-recovery-netns.sh"]
async fn native_relay_disconnect_revokes_real_path_and_fails_closed() {
    require_isolated_topology();
    let mut backend = RealNetnsBackend::start().await;
    backend.disconnect_relay().await;

    let report = RecoveryCoordinator
        .recover(&mut backend, request(RecoveryTrigger::RelayDisconnected))
        .await;

    assert_eq!(
        report.status,
        fuckipv6_client::routing::recovery::RecoveryStatus::DegradedFailClosed
    );
    assert_eq!(report.revoked_paths, vec![InvalidatedPath::Relay]);
    let decision = report.failure_decisions.first().expect("failure decision");
    assert_eq!(
        decision.failure,
        TrafficFailureKind::NetworkSelectionUnavailable
    );
    assert_eq!(
        report.degradation.as_ref().unwrap().state,
        TrafficRuntimeDegradationState::DegradedFailClosed
    );
    assert!(backend.audit_contains_relay_disconnect().await);
    backend.shutdown().await;
}

#[tokio::test]
#[ignore = "requires the isolated privileged topology from scripts/test-recovery-netns.sh"]
async fn native_dns_timeout_is_distinguished_from_success() {
    require_isolated_topology();
    let mut backend = RealNetnsBackend::start().await;
    backend.use_dns_timeout_probe().await;

    let report = RecoveryCoordinator
        .recover(&mut backend, request(RecoveryTrigger::NetworkChanged))
        .await;

    assert_eq!(
        report.status,
        fuckipv6_client::routing::recovery::RecoveryStatus::DegradedFailClosed
    );
    assert_eq!(
        report.error_code.as_deref(),
        Some("recovery.reachability_blocked_by_fail_closed")
    );
    let probe = report.probes.first().expect("DNS timeout probe");
    assert_eq!(probe.state, RecoveryProbeState::Failed);
    assert_eq!(probe.failed_stage, Some(RecoveryProbeStage::Dns));
    assert_eq!(probe.error_code.as_deref(), Some("probe.dns_timeout"));
    assert!(probe.duration_ms >= 100);
    assert_eq!(
        report.degradation.as_ref().unwrap().state,
        TrafficRuntimeDegradationState::DegradedFailClosed
    );
    let decision = report.failure_decisions.first().expect("failure decision");
    assert_eq!(decision.failure, TrafficFailureKind::ConnectionTimeout);
    backend.shutdown().await;
}
