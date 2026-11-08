//! Production resource adapter for Linux recovery.
//!
//! The coordinator owns trigger sequencing and timeouts. This adapter reads the
//! actual kernel routing/DNS state and mutates only the TUN, policy routes and
//! resolver resources owned by the current runtime. OS events and reachability
//! probes are kept outside this layer so their implementations can be tested
//! independently.

use std::future::Future;
use std::pin::Pin;

use anyhow::{Context, Result};
use tokio::process::Command;
use tokio::time::timeout;

use super::doh_dns;
use super::linux_route::LinuxRouteManager;
use super::recovery::{
    DnsRecoverySnapshot, PathRecoveryState, RecoveryOperationError, RecoveryRepairReport,
    RecoveryResourceSnapshot, RecoveryResourceState, RecoveryTrigger, RouteRecoverySnapshot,
    TunRecoverySnapshot,
};

const LINK_QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

type TunRecoveryFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(), RecoveryOperationError>> + Send + 'a>>;

/// Small lifecycle boundary implemented by the runtime's TUN owner. Snapshot
/// and cleanup remain in the adapter; platform owners only have to recreate the
/// complete data path (device, reader task and buffer channels) atomically.
pub(crate) trait LinuxRecoveryTun: Send {
    fn name(&self) -> &str;

    /// Recreate the owned TUN after its reader has released all clones. The
    /// operation must be idempotent so a later recovery pass can retry it.
    fn recreate(&mut self) -> TunRecoveryFuture<'_>;
}

/// Real TUN/route/DNS resource coordinator used by Linux runtimes.
pub(crate) struct LinuxRecoveryAdapter<'a> {
    tun: &'a mut dyn LinuxRecoveryTun,
    routes: &'a mut LinuxRouteManager,
    exit_path: PathRecoveryState,
    relay_path: PathRecoveryState,
}

impl<'a> LinuxRecoveryAdapter<'a> {
    pub(crate) fn new(
        tun: &'a mut dyn LinuxRecoveryTun,
        routes: &'a mut LinuxRouteManager,
        exit_path: PathRecoveryState,
        relay_path: PathRecoveryState,
    ) -> Self {
        Self {
            tun,
            routes,
            exit_path,
            relay_path,
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
}

fn tun_is_healthy(snapshot: &TunRecoverySnapshot) -> bool {
    snapshot.state == RecoveryResourceState::Healthy
        && snapshot.duplicate_interfaces == 0
        && snapshot.residual_interfaces == 0
}

fn route_is_healthy(snapshot: &RouteRecoverySnapshot) -> bool {
    snapshot.installed_routes >= snapshot.expected_routes
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
}
