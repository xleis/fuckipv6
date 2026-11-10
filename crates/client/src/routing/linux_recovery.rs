//! Production resource adapter for Linux recovery.
//!
//! The coordinator owns trigger sequencing and timeouts. This adapter reads the
//! actual kernel routing/DNS state and mutates only the TUN, policy routes and
//! resolver resources owned by the current runtime. OS events remain outside
//! this layer; exit/relay disconnects are represented by the path state passed
//! to each recovery pass.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::future::Future;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use fuckipv6_common::domain::{
    AuditEntry, AuditId, NetworkId, NodeId, TrafficFailureAuditContext, TrafficFailureDecision,
    TrafficFailureRouteContext,
};
use tokio::net::lookup_host;
use tokio::process::Command;
use tokio::time::{timeout, Instant};

use super::doh_dns;
use super::linux_route::LinuxRouteManager;
use super::recovery::{
    DnsRecoverySnapshot, FailureApplication, InvalidatedPath, PathRecoveryState, RecoveryBackend,
    RecoveryOperationError, RecoveryProbeObservation, RecoveryProbeResult, RecoveryProbeStage,
    RecoveryProbeState, RecoveryProbeTarget, RecoveryRepairReport, RecoveryResourceSnapshot,
    RecoveryResourceState, RecoveryTrigger, RouteRecoverySnapshot, TunRecoverySnapshot,
};

const LINK_QUERY_TIMEOUT: Duration = Duration::from_secs(2);
const PROBE_STAGE_TIMEOUT: Duration = Duration::from_secs(2);
const RECOVERY_AUDIT_FILE: &str = "recovery-audit.jsonl";
const GOOGLE_PROBE_HOST: &str = "www.google.com";
const YOUTUBE_PROBE_HOST: &str = "www.youtube.com";

type RecoveryFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, RecoveryOperationError>> + Send + 'a>>;
type TunRecoveryFuture<'a> = RecoveryFuture<'a, ()>;

/// Small lifecycle boundary implemented by the runtime's TUN owner. Snapshot
/// and cleanup remain in the adapter; platform owners only have to recreate the
/// complete data path (device, reader task and buffer channels) atomically.
pub(crate) trait LinuxRecoveryTun: Send {
    fn name(&self) -> &str;

    /// Recreate the owned TUN after its reader has released all clones. The
    /// operation must be idempotent so a later recovery pass can retry it.
    fn recreate(&mut self) -> TunRecoveryFuture<'_>;
}

/// Server-owned identity and routing context needed to make a failure audit
/// durable. Values are typed and therefore cannot be replaced by free-form UI
/// labels or network names.
#[derive(Debug, Clone)]
pub(crate) struct RecoveryAuditContext {
    pub(crate) config_root: PathBuf,
    pub(crate) actor: NodeId,
    pub(crate) network_id: Option<NetworkId>,
    pub(crate) route_context: TrafficFailureRouteContext,
}

/// Real TUN/route/DNS resource coordinator used by Linux runtimes.
pub(crate) struct LinuxRecoveryAdapter<'a> {
    tun: &'a mut dyn LinuxRecoveryTun,
    routes: &'a mut LinuxRouteManager,
    exit_path: PathRecoveryState,
    relay_path: PathRecoveryState,
    audit: RecoveryAuditContext,
    controlled_probes: BTreeMap<String, String>,
}

impl<'a> LinuxRecoveryAdapter<'a> {
    pub(crate) fn new(
        tun: &'a mut dyn LinuxRecoveryTun,
        routes: &'a mut LinuxRouteManager,
        exit_path: PathRecoveryState,
        relay_path: PathRecoveryState,
        audit: RecoveryAuditContext,
        controlled_probes: BTreeMap<String, String>,
    ) -> Self {
        Self {
            tun,
            routes,
            exit_path,
            relay_path,
            audit,
            controlled_probes,
        }
    }

    /// Read a sanitized snapshot from the kernel and live resolver file.
    pub(crate) async fn snapshot(&mut self) -> Result<RecoveryResourceSnapshot> {
        let tun = snapshot_tun(self.tun.name()).await?;
        let routes = self
            .routes
            .recovery_snapshot()
            .await
            .context("读取 Linux 恢复路由快照失败")?;
        let dns = doh_dns::recovery_snapshot().context("读取 Linux 恢复 DNS 快照失败")?;
        Ok(RecoveryResourceSnapshot {
            tun,
            routes,
            dns,
            exit_path: self.exit_path,
            relay_path: self.relay_path,
        })
    }

    /// Reconcile owned resources. Every trigger uses the same safety order:
    /// rebuild the TUN before routes, then re-evaluate routes and DNS together
    /// so a resolver cannot outlive a policy route or vice versa.
    pub(crate) async fn reconcile(
        &mut self,
        trigger: RecoveryTrigger,
    ) -> Result<RecoveryRepairReport> {
        let before = self
            .snapshot()
            .await
            .context("恢复前无法读取 Linux 资源快照")?;
        let tun_healthy = tun_is_healthy(&before.tun);
        let route_healthy = route_is_healthy(&before.routes);
        let dns_healthy = dns_is_healthy(&before.dns);

        let mut repair = RecoveryRepairReport {
            residual_routes_removed: before.routes.residual_routes,
            duplicate_routes_removed: before.routes.duplicate_routes,
            stale_interface_routes_removed: before.routes.stale_interface_routes,
            ..RecoveryRepairReport::default()
        };

        if !tun_healthy {
            self.tun
                .recreate()
                .await
                .map_err(|_| anyhow::anyhow!("重建 Linux TUN 失败"))?;
            repair.tun_recreated = true;
        }

        if repair.tun_recreated || !route_healthy || !dns_healthy {
            self.routes
                .refresh_plan()
                .await
                .context("重新安装 Linux 策略路由和 DNS 失败")?;
            repair.routes_restored = before
                .routes
                .expected_routes
                .saturating_sub(before.routes.installed_routes);
            repair.dns_reconfigured = true;
            tracing::warn!(
                "Linux 网络恢复已协调资源: trigger={trigger:?}, tun_recreated={}, routes_restored={}, dns_reconfigured=true",
                repair.tun_recreated,
                repair.routes_restored
            );
        }

        Ok(repair)
    }

    fn probe_target_host(&self, target: &RecoveryProbeTarget) -> Option<String> {
        probe_host_for_target(target, &self.controlled_probes)
    }
}

impl RecoveryBackend for LinuxRecoveryAdapter<'_> {
    fn snapshot(&mut self) -> RecoveryFuture<'_, RecoveryResourceSnapshot> {
        Box::pin(async move {
            LinuxRecoveryAdapter::snapshot(self)
                .await
                .map_err(|_| RecoveryOperationError::new("recovery.snapshot_failed"))
        })
    }

    fn reconcile(&mut self, trigger: RecoveryTrigger) -> RecoveryFuture<'_, RecoveryRepairReport> {
        Box::pin(async move {
            LinuxRecoveryAdapter::reconcile(self, trigger)
                .await
                .map_err(|_| RecoveryOperationError::new("recovery.reconcile_failed"))
        })
    }

    fn revoke_path(&mut self, path: InvalidatedPath) -> RecoveryFuture<'_, ()> {
        Box::pin(async move {
            match path {
                InvalidatedPath::Exit => self.exit_path = PathRecoveryState::Disconnected,
                InvalidatedPath::Relay => self.relay_path = PathRecoveryState::Disconnected,
            }
            Ok(())
        })
    }

    fn apply_failure_decision(
        &mut self,
        application: FailureApplication,
    ) -> RecoveryFuture<'_, ()> {
        Box::pin(async move {
            let occurred_at_unix_ms = unix_time_ms().max(1);
            let audit_id = AuditId::from_public_key(
                format!(
                    "{}:{}:{}",
                    application.diagnostic_id,
                    application.decision.failure.as_str(),
                    occurred_at_unix_ms
                )
                .as_bytes(),
            );
            let entry = build_failure_audit_entry(
                &application.decision,
                &self.audit,
                audit_id,
                &application.diagnostic_id,
                occurred_at_unix_ms,
            )
            .map_err(|_| RecoveryOperationError::new("recovery.audit_invalid"))?;
            append_recovery_audit(&self.audit.config_root, &entry)
                .await
                .map_err(|_| RecoveryOperationError::new("recovery.audit_write_failed"))
        })
    }

    fn probe(
        &mut self,
        target: RecoveryProbeTarget,
    ) -> RecoveryFuture<'_, RecoveryProbeObservation> {
        Box::pin(async move {
            let Some(host) = self.probe_target_host(&target) else {
                return Ok(RecoveryProbeObservation {
                    state: RecoveryProbeState::Unsupported,
                    failed_stage: None,
                    error_code: Some("recovery.probe_target_unsupported".into()),
                });
            };
            probe_https_host(&host).await
        })
    }
}

fn probe_host_for_target(
    target: &RecoveryProbeTarget,
    controlled_probes: &BTreeMap<String, String>,
) -> Option<String> {
    match target {
        RecoveryProbeTarget::Google => Some(GOOGLE_PROBE_HOST.into()),
        RecoveryProbeTarget::YouTube => Some(YOUTUBE_PROBE_HOST.into()),
        RecoveryProbeTarget::Controlled(id) => controlled_probes.get(id).cloned(),
    }
}

fn build_failure_audit_entry(
    decision: &TrafficFailureDecision,
    audit: &RecoveryAuditContext,
    audit_id: AuditId,
    diagnostic_id: &str,
    occurred_at_unix_ms: u64,
) -> fuckipv6_common::domain::DomainResult<AuditEntry> {
    decision.audit_entry(TrafficFailureAuditContext {
        audit_id,
        actor: audit.actor.clone(),
        network_id: audit.network_id.clone(),
        occurred_at_unix_ms,
        requested_action: audit.route_context.requested_action.clone(),
        fixed_exit: audit.route_context.fixed_exit.clone(),
        fixed_network: audit.route_context.fixed_network.clone(),
        diagnostic_id: Some(diagnostic_id.to_string()),
    })
}

async fn append_recovery_audit(root: &Path, entry: &AuditEntry) -> Result<()> {
    let root = root.to_path_buf();
    let entry = entry.clone();
    tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&root).context("创建恢复审计目录失败")?;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
            .context("设置恢复审计目录权限失败")?;
        let path = root.join(RECOVERY_AUDIT_FILE);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
            .context("打开恢复审计文件失败")?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .context("设置恢复审计文件权限失败")?;
        serde_json::to_writer(&mut file, &entry).context("编码恢复审计记录失败")?;
        file.write_all(b"\n").context("写入恢复审计记录失败")?;
        file.flush().context("刷新恢复审计记录失败")?;
        file.sync_all().context("同步恢复审计记录失败")
    })
    .await
    .context("恢复审计任务失败")?
}

async fn probe_https_host(host: &str) -> Result<RecoveryProbeObservation, RecoveryOperationError> {
    let addresses = match timeout(PROBE_STAGE_TIMEOUT, lookup_host((host, 443))).await {
        Ok(Ok(addresses)) => addresses.collect::<Vec<_>>(),
        Ok(Err(_)) | Err(_) => {
            return Ok(probe_failure(
                RecoveryProbeStage::Dns,
                "recovery.probe_dns_failed",
            ))
        }
    };
    let Some(address) = addresses.first().copied() else {
        return Ok(probe_failure(
            RecoveryProbeStage::Dns,
            "recovery.probe_dns_empty",
        ));
    };

    match timeout(PROBE_STAGE_TIMEOUT, tokio::net::TcpStream::connect(address)).await {
        Ok(Ok(stream)) => drop(stream),
        Ok(Err(_)) | Err(_) => {
            return Ok(probe_failure(
                RecoveryProbeStage::Tcp,
                "recovery.probe_tcp_failed",
            ))
        }
    }

    let client = reqwest::Client::builder()
        .no_proxy()
        .https_only(true)
        .resolve(host, address)
        .timeout(PROBE_STAGE_TIMEOUT)
        .build()
        .map_err(|_| RecoveryOperationError::new("recovery.probe_client_failed"))?;
    let url = format!("https://{host}/");
    match client.get(url).send().await {
        Ok(_) => Ok(RecoveryProbeObservation {
            state: RecoveryProbeState::Passed,
            failed_stage: None,
            error_code: None,
        }),
        Err(_) => Ok(probe_failure(
            RecoveryProbeStage::Tls,
            "recovery.probe_tls_failed",
        )),
    }
}

/// Probe from the Exit's own physical network stack. The returned structured
/// evidence travels back to the client over the authenticated Secure Session.
pub(crate) async fn probe_exit_readiness_target(
    target: RecoveryProbeTarget,
) -> RecoveryProbeResult {
    let started = Instant::now();
    let Some(host) = probe_host_for_target(&target, &BTreeMap::new()) else {
        return RecoveryProbeResult {
            target,
            state: RecoveryProbeState::Unsupported,
            failed_stage: None,
            duration_ms: 0,
            error_code: Some("recovery.probe_target_unsupported".into()),
        };
    };
    let (state, failed_stage, error_code) = match probe_https_host(&host).await {
        Ok(observation) => (
            observation.state,
            observation.failed_stage,
            observation.error_code,
        ),
        Err(error) => (RecoveryProbeState::Failed, None, Some(error.code)),
    };
    RecoveryProbeResult {
        target,
        state,
        failed_stage,
        duration_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        error_code,
    }
}

fn probe_failure(stage: RecoveryProbeStage, error_code: &str) -> RecoveryProbeObservation {
    RecoveryProbeObservation {
        state: RecoveryProbeState::Failed,
        failed_stage: Some(stage),
        error_code: Some(error_code.into()),
    }
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

fn tun_is_healthy(snapshot: &TunRecoverySnapshot) -> bool {
    snapshot.state == RecoveryResourceState::Healthy
        && snapshot.duplicate_interfaces == 0
        && snapshot.residual_interfaces == 0
}

fn route_is_healthy(snapshot: &RouteRecoverySnapshot) -> bool {
    snapshot.expected_routes == snapshot.installed_routes
        && snapshot.missing_routes == 0
        && snapshot.duplicate_routes == 0
        && snapshot.residual_routes == 0
        && snapshot.stale_interface_routes == 0
}

fn dns_is_healthy(snapshot: &DnsRecoverySnapshot) -> bool {
    snapshot.state == RecoveryResourceState::Healthy
        && snapshot.authorized_resolvers > 0
        && snapshot.unsecured_resolvers == 0
        && !snapshot.insecure_fallback_detected
}

async fn snapshot_tun(name: &str) -> Result<TunRecoverySnapshot> {
    validate_interface_name(name)?;

    let mut command = Command::new("ip");
    command.args(["-details", "link", "show", "dev", name]);
    command.kill_on_drop(true);
    let output = timeout(LINK_QUERY_TIMEOUT, command.output())
        .await
        .context("查询 Linux TUN 状态超时")?
        .context("无法执行 Linux TUN 状态查询")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if link_is_missing(&stderr) {
            return Ok(TunRecoverySnapshot {
                state: RecoveryResourceState::Missing,
                duplicate_interfaces: 0,
                residual_interfaces: 0,
            });
        }
        anyhow::bail!("查询 Linux TUN 状态返回失败");
    }

    let details = String::from_utf8_lossy(&output.stdout);
    Ok(TunRecoverySnapshot {
        state: if tun_link_details_are_valid(&details) {
            RecoveryResourceState::Healthy
        } else {
            RecoveryResourceState::Degraded
        },
        duplicate_interfaces: 0,
        residual_interfaces: 0,
    })
}

fn validate_interface_name(name: &str) -> Result<()> {
    if name.is_empty()
        || matches!(name, "." | "..")
        || name.len() > 15
        || name
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')))
    {
        anyhow::bail!("Linux TUN 接口名无效");
    }
    Ok(())
}

fn link_is_missing(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("does not exist")
        || lower.contains("cannot find device")
        || lower.contains("no such device")
}

fn tun_link_details_are_valid(details: &str) -> bool {
    let tokens = details.split_whitespace().collect::<Vec<_>>();
    tokens
        .windows(2)
        .any(|pair| pair[0] == "tun" && pair[1] == "type")
        && tokens
            .windows(3)
            .any(|triple| triple == ["tun", "type", "tun"])
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuckipv6_common::domain::{
        TrafficAction, TrafficFailureDecisionReason, TrafficFailureKind, TrafficFailureOutcome,
        TrafficFailurePolicy,
    };

    fn decision() -> TrafficFailureDecision {
        TrafficFailurePolicy::fail_closed()
            .decide(
                &TrafficFailureRouteContext::new(TrafficAction::Reject),
                TrafficFailureKind::ExitUnavailable,
            )
            .expect("decision")
    }

    fn unique_test_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "fuckipv6-recovery-{label}-{}-{}",
            std::process::id(),
            unix_time_ms()
        ))
    }

    #[test]
    fn interface_validation_rejects_names_that_cannot_be_owned_safely() {
        for valid in ["fuckipv6", "fuckipv6-1", "fip6_recov", "tun.0"] {
            validate_interface_name(valid).unwrap();
        }
        for invalid in [
            "",
            ".",
            "..",
            "with space",
            "with/slash",
            "1234567890123456",
        ] {
            assert!(validate_interface_name(invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn missing_link_errors_are_distinguished_from_permission_errors() {
        assert!(link_is_missing("Device \"fuckipv6\" does not exist."));
        assert!(link_is_missing("Cannot find device \"fuckipv6\""));
        assert!(!link_is_missing("Operation not permitted"));
    }

    #[test]
    fn tun_details_require_a_real_tun_device() {
        assert!(tun_link_details_are_valid(
            "3: fuckipv6: <POINTOPOINT,NOARP,UP> mtu 1420\n    tun type tun pi off vnet_hdr off"
        ));
        assert!(!tun_link_details_are_valid(
            "2: eth0: <BROADCAST,MULTICAST,UP> mtu 1500\n    link/ether 00:11:22:33:44:55"
        ));
        assert!(!tun_link_details_are_valid(
            "3: not-tun: <POINTOPOINT,NOARP,UP> mtu 1420\n    tun_flags 0x1001"
        ));
    }

    #[test]
    fn resource_health_requires_no_residue_or_insecure_dns() {
        let mut tun = TunRecoverySnapshot {
            state: RecoveryResourceState::Healthy,
            duplicate_interfaces: 0,
            residual_interfaces: 0,
        };
        assert!(tun_is_healthy(&tun));
        tun.residual_interfaces = 1;
        assert!(!tun_is_healthy(&tun));

        let mut routes = RouteRecoverySnapshot {
            expected_routes: 1,
            installed_routes: 1,
            missing_routes: 0,
            duplicate_routes: 0,
            residual_routes: 0,
            stale_interface_routes: 0,
        };
        assert!(route_is_healthy(&routes));
        routes.residual_routes = 1;
        assert!(!route_is_healthy(&routes));

        let mut dns = DnsRecoverySnapshot {
            mode: super::super::recovery::DnsRecoveryMode::Doh,
            state: RecoveryResourceState::Healthy,
            authorized_resolvers: 1,
            unsecured_resolvers: 0,
            insecure_fallback_detected: false,
        };
        assert!(dns_is_healthy(&dns));
        dns.insecure_fallback_detected = true;
        assert!(!dns_is_healthy(&dns));
    }

    #[test]
    fn failure_audit_is_typed_redacted_and_secure() {
        let root = unique_test_root("audit");
        let actor = NodeId::from_public_key(b"local-node");
        let context = RecoveryAuditContext {
            config_root: root.clone(),
            actor: actor.clone(),
            network_id: Some(NetworkId::from_public_key(b"network")),
            route_context: TrafficFailureRouteContext::new(TrafficAction::Reject),
        };
        let entry = build_failure_audit_entry(
            &decision(),
            &context,
            AuditId::from_public_key(b"audit"),
            "diag-recovery-test",
            1,
        )
        .expect("audit entry");
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime
            .block_on(append_recovery_audit(&root, &entry))
            .expect("append audit");

        let path = root.join(RECOVERY_AUDIT_FILE);
        assert_eq!(
            std::fs::metadata(&path)
                .expect("audit metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let contents = std::fs::read_to_string(&path).expect("audit content");
        assert_eq!(contents.lines().count(), 1);
        assert!(!contents.contains("www.google.com"));
        assert!(!contents.contains("127.0.0.1"));
        let decoded: AuditEntry = serde_json::from_str(contents.trim()).expect("audit json");
        assert_eq!(decoded.actor, actor);
        assert_eq!(
            decoded.details.get("diagnostic_id").map(String::as_str),
            Some("diag-recovery-test")
        );
        assert_eq!(
            decoded.details.get("reason_code").map(String::as_str),
            Some(TrafficFailureDecisionReason::RejectRule.as_str())
        );
        assert_eq!(
            decoded.details.get("outcome").map(String::as_str),
            Some(TrafficFailureOutcome::FailClosed.as_str())
        );
        std::fs::remove_dir_all(root).expect("remove test root");
    }

    #[test]
    fn probe_targets_use_real_hosts_and_reject_unconfigured_controls() {
        let map = BTreeMap::from([("deployment".into(), "probe.example".into())]);
        assert_eq!(
            probe_host_for_target(&RecoveryProbeTarget::Google, &map),
            Some(GOOGLE_PROBE_HOST.into())
        );
        assert_eq!(
            probe_host_for_target(&RecoveryProbeTarget::Controlled("deployment".into()), &map),
            Some("probe.example".into())
        );
        assert_eq!(
            probe_host_for_target(&RecoveryProbeTarget::Controlled("missing".into()), &map),
            None
        );
    }
}
