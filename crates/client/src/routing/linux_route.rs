//! Linux policy-routing support for proxy modes.
//!
//! Traffic is routed through a dedicated table only after every control-plane,
//! virtual-network and direct-connect destination has a bypass rule. The
//! catch-all rule is therefore installed last, which makes setup failure-safe.

use anyhow::{Context, Result};
use fuckipv6_common::{RoutingMode, VirtualIps};
use ipnet::{IpNet, Ipv4Net, Ipv6Net};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::time::{timeout, Instant};

use super::doh_dns::DohDnsGuard;
use super::linux_route_probe::{
    probe_exit_path as probe_marked_exit_path, ExitPathProbeReport, ProbeFamily,
};
use super::recovery::RouteRecoverySnapshot;
use crate::network_setup::{self, ensure_stage, NetworkSetupError, NetworkSetupStage};

const ROUTE_PROTOCOL: &str = "99";
const STATIC_PREFERENCE_START: u32 = 1_000;
const DYNAMIC_PREFERENCE_START: u32 = 20_000;
const CATCH_ALL_PREFERENCE: u32 = 32_000;
const TABLE_ID_BASE: u32 = 10_000;
const MAIN_TABLE_ID: u32 = 254;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
const ROLLBACK_TIMEOUT: Duration = Duration::from_secs(4);
const SYNC_COMMAND_TIMEOUT: Duration = Duration::from_secs(1);
const HOST_NETWORK_RESTORE_ATTEMPTS: usize = 3;
const HOST_NETWORK_RESTORE_RETRY_DELAY: Duration = Duration::from_millis(50);
const EXIT_PATH_PROBE_MARK: u32 = 0x4650_3601;
const EXIT_PATH_PROBE_PREFERENCE: u32 = 900;
const EXIT_PATH_PROBE_TIMEOUT: Duration = Duration::from_secs(8);
const PROBE_RULE_REMOVE_ATTEMPTS: usize = 3;

type RouteCommandFuture<'a> = Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

trait RouteDeleteRunner: Send + Sync {
    fn delete_rule<'a>(&'a self, rule: &'a RuleSpec) -> RouteCommandFuture<'a>;
    fn delete_table_route<'a>(
        &'a self,
        family: Family,
        table_id: u32,
        tun_name: &'a str,
    ) -> RouteCommandFuture<'a>;
}

struct SystemRouteDeleteRunner;

impl RouteDeleteRunner for SystemRouteDeleteRunner {
    fn delete_rule<'a>(&'a self, rule: &'a RuleSpec) -> RouteCommandFuture<'a> {
        Box::pin(run_ip_rule("del", rule))
    }

    fn delete_table_route<'a>(
        &'a self,
        family: Family,
        table_id: u32,
        tun_name: &'a str,
    ) -> RouteCommandFuture<'a> {
        Box::pin(remove_table_route(family, table_id, tun_name))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Family {
    V4,
    V6,
}

impl Family {
    fn of(ip: IpAddr) -> Self {
        match ip {
            IpAddr::V4(_) => Self::V4,
            IpAddr::V6(_) => Self::V6,
        }
    }

    fn command_flag(self) -> Option<&'static str> {
        match self {
            Self::V4 => None,
            Self::V6 => Some("-6"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuleSpec {
    family: Family,
    destination: Option<IpNet>,
    fwmark: Option<u32>,
    action: RuleAction,
    preference: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuleAction {
    Lookup(u32),
    Unreachable,
}

#[derive(Debug, Clone)]
struct RoutePlan {
    mode: RoutingMode,
    tun_name: String,
    assigned_ips: VirtualIps,
    virtual_cidrs: Vec<IpNet>,
    direct_cidrs: Vec<IpNet>,
    control_endpoints: Vec<SocketAddr>,
}

#[derive(Debug)]
pub struct LinuxRouteManager {
    tun_name: String,
    table_id: u32,
    active_families: HashSet<Family>,
    global_proxy_active: bool,
    static_bypasses: BTreeSet<IpNet>,
    installed_rules: Vec<RuleSpec>,
    installed_table_routes: Vec<Family>,
    dynamic_rules: HashMap<IpAddr, RuleSpec>,
    dynamic_references: HashMap<IpAddr, usize>,
    peer_bypasses: HashMap<String, Vec<IpAddr>>,
    next_v4_preference: u32,
    next_v6_preference: u32,
    dns_guard: Option<DohDnsGuard>,
    plan: Option<RoutePlan>,
}

impl LinuxRouteManager {
    /// Install a fail-closed proxy route for GlobalProxy / BypassMainland.
    ///
    /// MeshOnly returns an inactive manager and never changes host routing.
    pub async fn install(
        mode: RoutingMode,
        tun_name: impl Into<String>,
        assigned_ips: &VirtualIps,
        virtual_cidrs: &[IpNet],
        direct_cidrs: &[IpNet],
        control_endpoints: &[SocketAddr],
    ) -> Result<Self> {
        Self::install_inner(
            mode,
            tun_name,
            assigned_ips,
            virtual_cidrs,
            direct_cidrs,
            control_endpoints,
        )
        .await
        .map_err(|error| ensure_stage(error, NetworkSetupStage::RouteSetup))
    }

    async fn install_inner(
        mode: RoutingMode,
        tun_name: impl Into<String>,
        assigned_ips: &VirtualIps,
        virtual_cidrs: &[IpNet],
        direct_cidrs: &[IpNet],
        control_endpoints: &[SocketAddr],
    ) -> Result<Self> {
        let tun_name = tun_name.into();
        let table_id = table_id_for_interface(&tun_name);
        let active_families = active_families(assigned_ips);
        let plan = RoutePlan {
            mode,
            tun_name: tun_name.clone(),
            assigned_ips: assigned_ips.clone(),
            virtual_cidrs: virtual_cidrs.to_vec(),
            direct_cidrs: direct_cidrs.to_vec(),
            control_endpoints: control_endpoints.to_vec(),
        };
        let mut manager = Self {
            tun_name,
            table_id,
            active_families,
            global_proxy_active: false,
            static_bypasses: BTreeSet::new(),
            installed_rules: Vec::new(),
            installed_table_routes: Vec::new(),
            dynamic_rules: HashMap::new(),
            dynamic_references: HashMap::new(),
            peer_bypasses: HashMap::new(),
            next_v4_preference: DYNAMIC_PREFERENCE_START,
            next_v6_preference: DYNAMIC_PREFERENCE_START,
            dns_guard: None,
            plan: Some(plan),
        };

        if mode == RoutingMode::MeshOnly {
            // MeshOnly must never own dynamic peer routes. Keeping the assigned
            // families here made later peer updates install stale bypass rules.
            manager.active_families.clear();
            return Ok(manager);
        }
        if manager.active_families.is_empty() {
            return Ok(manager);
        }

        let mut bypasses = build_bypass_cidrs(
            mode,
            assigned_ips,
            virtual_cidrs,
            direct_cidrs,
            control_endpoints,
        )?;
        bypasses.retain(|network| manager.family_active(family_of_network(*network)));

        if bypasses.len() >= (DYNAMIC_PREFERENCE_START - STATIC_PREFERENCE_START) as usize {
            anyhow::bail!("旁路网段数量过多，无法安全安装策略路由");
        }

        manager.static_bypasses = bypasses.iter().copied().collect();

        // A previous process may have crashed after activating GlobalProxy.
        // That catch-all points at this TUN's private table and must be removed
        // before a new runtime can safely regain the host network.
        if let Err(error) = manager.remove_stale_catch_all_rules().await {
            return Err(error).context("清理上次异常退出遗留的全局代理兜底路由失败");
        }
        if let Err(error) = manager.remove_stale_probe_rules().await {
            return Err(error).context("清理上次异常退出遗留的 Exit 数据路径探针规则失败");
        }

        let setup_result = async {
            // A table route has no effect until the catch-all rule is installed.
            for family in [Family::V4, Family::V6] {
                if manager.active_families.contains(&family) {
                    let source = match family {
                        Family::V4 => assigned_ips.ipv4.map(IpAddr::V4),
                        Family::V6 => assigned_ips.ipv6.map(IpAddr::V6),
                    }
                    .context("代理默认路由缺少对应地址族的虚拟源地址")?;
                    manager.install_table_route(family, source).await?;
                }
            }

            for (index, network) in bypasses.iter().enumerate() {
                let family = family_of_network(*network);
                if !manager.family_active(family) {
                    continue;
                }
                let rule = RuleSpec {
                    family,
                    destination: Some(*network),
                    fwmark: None,
                    action: RuleAction::Lookup(MAIN_TABLE_ID),
                    preference: STATIC_PREFERENCE_START + index as u32,
                };
                manager.install_rule(&rule).await?;
            }

            Ok::<(), anyhow::Error>(())
        }
        .await;

        if let Err(error) = setup_result {
            let _ = manager.rollback().await;
            return Err(error).context("安装代理策略路由失败，已回滚");
        }

        tracing::info!(
            "代理基础策略路由已安装，等待 Exit 验证后启用: table={}, interface={}, bypasses={}, mode={:?}",
            manager.table_id,
            manager.tun_name,
            manager.static_bypasses.len(),
            mode
        );
        Ok(manager)
    }

    pub fn table_id(&self) -> u32 {
        self.table_id
    }

    pub fn is_active(&self) -> bool {
        !self.active_families.is_empty()
    }

    /// Whether this manager currently owns the host-wide catch-all rules and
    /// resolver override. Base table routes and bypass rules may already exist
    /// while this is false; they do not intercept ordinary host traffic.
    pub fn global_proxy_active(&self) -> bool {
        self.global_proxy_active
    }

    /// Validate the real client -> TUN -> P2P -> Exit data path without
    /// changing ordinary host routing. Only sockets carrying the private probe
    /// mark can match the temporary policy rule.
    pub async fn validate_exit_data_path(&mut self) -> Result<()> {
        let (result_tx, mut result_rx) = mpsc::unbounded_channel();
        self.begin_exit_data_path_validation(result_tx).await?;
        let probe_result = result_rx
            .recv()
            .await
            .unwrap_or_else(|| Err(anyhow::anyhow!("Exit 数据路径探针任务未返回结果")));
        self.complete_exit_data_path_validation(probe_result).await
    }

    /// Install the marked probe route, then run the real sockets in a background
    /// task so the caller can keep servicing TUN packets. The probe intentionally
    /// traverses the same in-process TUN -> P2P -> Exit path it is validating.
    pub(crate) async fn begin_exit_data_path_validation(
        &mut self,
        result_tx: mpsc::UnboundedSender<Result<ExitPathProbeReport>>,
    ) -> Result<()> {
        if !self.is_active() {
            anyhow::bail!("路由管理器未激活，拒绝执行 Exit 数据路径探针");
        }
        if self.global_proxy_active {
            anyhow::bail!("全局代理已启用，拒绝重复执行未激活状态的数据路径探针");
        }

        if let Err(error) = self.remove_probe_rules().await {
            return Err(error).context("开始 Exit 数据路径探针前清理旧探针规则失败");
        }

        let families = [Family::V4, Family::V6]
            .into_iter()
            .filter(|family| self.active_families.contains(family))
            .map(|family| match family {
                Family::V4 => ProbeFamily::V4,
                Family::V6 => ProbeFamily::V6,
            })
            .collect::<Vec<_>>();
        for family in [Family::V4, Family::V6] {
            if !self.active_families.contains(&family) {
                continue;
            }
            let rule = RuleSpec {
                family,
                destination: None,
                fwmark: Some(EXIT_PATH_PROBE_MARK),
                action: RuleAction::Lookup(self.table_id),
                preference: EXIT_PATH_PROBE_PREFERENCE,
            };
            if let Err(error) = self.install_rule(&rule).await {
                let cleanup_error = self.remove_probe_rules().await.err();
                return match cleanup_error {
                    Some(cleanup_error) => Err(anyhow::anyhow!(
                        "安装 Exit 数据路径探针规则失败: {error:#}; 清理探针规则也失败: {cleanup_error:#}"
                    )),
                    None => Err(error).context("安装 Exit 数据路径探针规则失败，已清理"),
                };
            }
        }

        tokio::spawn(async move {
            let probe = tokio::spawn(async move {
                match timeout(
                    EXIT_PATH_PROBE_TIMEOUT,
                    probe_marked_exit_path(&families, EXIT_PATH_PROBE_MARK),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => Err(anyhow::anyhow!(
                        "Exit 数据路径探针超过 {:?}",
                        EXIT_PATH_PROBE_TIMEOUT
                    )),
                }
            });
            let probe_result = match probe.await {
                Ok(result) => result,
                Err(error) => Err(anyhow::anyhow!("Exit 数据路径探针任务异常退出: {error}")),
            };
            if result_tx.send(probe_result).is_err() {
                tracing::debug!("Exit 数据路径探针结果无人接收，丢弃结果");
            }
        });
        Ok(())
    }

    pub(crate) async fn complete_exit_data_path_validation(
        &mut self,
        probe_result: Result<ExitPathProbeReport>,
    ) -> Result<()> {
        let cleanup_result = self.remove_probe_rules().await;

        match (probe_result, cleanup_result) {
            (Ok(report), Ok(())) => {
                self.active_families.retain(|family| match family {
                    Family::V4 => report.ipv4,
                    Family::V6 => report.ipv6,
                });
                if !report.has_usable_family() {
                    anyhow::bail!("Exit 数据路径探针未验证任何可用地址族");
                }
                tracing::info!(
                    ipv4 = report.ipv4,
                    ipv6 = report.ipv6,
                    mark = EXIT_PATH_PROBE_MARK,
                    table = self.table_id,
                    "Exit 数据路径实测通过：经 TUN/P2P/Exit 完成 YouTube DNS、TCP 与 TLS 验证"
                );
                Ok(())
            }
            (Err(probe_error), Ok(())) => Err(probe_error)
                .context("Exit 数据路径实测失败，探针规则已清理，宿主全局路由未启用"),
            (Ok(_), Err(cleanup_error)) => {
                Err(cleanup_error).context("Exit 数据路径实测通过，但探针规则清理失败")
            }
            (Err(probe_error), Err(cleanup_error)) => Err(anyhow::anyhow!(
                "Exit 数据路径实测失败: {probe_error:#}; 探针规则清理也失败: {cleanup_error:#}"
            )),
        }
    }

    /// Enable host-wide proxying after the selected Exit has passed every
    /// authenticated readiness and local-resource check.
    pub async fn activate_global_proxy(&mut self) -> Result<()> {
        if !self.is_active() {
            return Ok(());
        }
        if self.global_proxy_active {
            return Ok(());
        }

        let assigned_ipv6 = self
            .plan
            .as_ref()
            .and_then(|plan| plan.assigned_ips.ipv6)
            .is_some();
        for family in [Family::V4, Family::V6] {
            let action = if self.active_families.contains(&family) {
                RuleAction::Lookup(self.table_id)
            } else if family == Family::V6 && assigned_ipv6 {
                // The Exit cannot carry IPv6. Keep local/link/mesh IPv6 bypass
                // rules, but fail ordinary IPv6 closed instead of leaking it
                // over the physical default route.
                RuleAction::Unreachable
            } else {
                continue;
            };
            if self.owns_catch_all_rule(family, action) {
                continue;
            }
            let rule = RuleSpec {
                family,
                destination: None,
                fwmark: None,
                action,
                preference: CATCH_ALL_PREFERENCE,
            };
            if let Err(error) = self.install_rule(&rule).await {
                let cleanup_error = self.remove_catch_all_rules().await.err();
                return match cleanup_error {
                    Some(cleanup_error) => Err(anyhow::anyhow!(
                        "启用全局代理兜底路由失败: {error:#}; 回滚兜底路由也失败: {cleanup_error:#}"
                    )),
                    None => Err(error).context("启用全局代理兜底路由失败，已回滚"),
                };
            }
        }

        // DNS is changed only after every family's catch-all rule is present,
        // otherwise applications could resolve through the bridge while normal
        // host traffic is still outside the tunnel.
        match DohDnsGuard::install().await {
            Ok(guard) => self.dns_guard = guard,
            Err(error) => {
                let cleanup_error = self.remove_catch_all_rules().await.err();
                return match cleanup_error {
                    Some(cleanup_error) => Err(anyhow::anyhow!(
                        "安装 DoH DNS 代理失败: {error:#}; 回滚兜底路由也失败: {cleanup_error:#}"
                    )),
                    None => Err(error).context("安装 DoH DNS 代理失败，已回滚兜底路由"),
                };
            }
        }

        self.global_proxy_active = true;
        tracing::info!(
            "全局代理已启用: table={}, interface={}",
            self.table_id,
            self.tun_name
        );
        Ok(())
    }

    /// Stop intercepting ordinary host traffic before restoring DNS. Base table
    /// routes and bypass rules remain owned for a later validation retry.
    ///
    /// Cleanup is retried a bounded number of times because a transient `ip`
    /// failure must not leave the host disconnected behind a failed tunnel.
    /// Each attempt is idempotent: already-removed rules are no longer tracked
    /// and a still-live DNS guard continues to be the authoritative owner.
    pub async fn deactivate_global_proxy(&mut self) -> Result<()> {
        let mut last_error = None;
        for attempt in 1..=HOST_NETWORK_RESTORE_ATTEMPTS {
            match self.deactivate_global_proxy_once().await {
                Ok(()) => return Ok(()),
                Err(error) => {
                    tracing::warn!(
                        attempt,
                        attempts = HOST_NETWORK_RESTORE_ATTEMPTS,
                        error = %error,
                        "恢复宿主网络失败，准备重试"
                    );
                    last_error = Some(error);
                }
            }
            if attempt < HOST_NETWORK_RESTORE_ATTEMPTS {
                tokio::time::sleep(HOST_NETWORK_RESTORE_RETRY_DELAY).await;
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("恢复宿主网络失败")))
            .context("连续恢复宿主网络失败")
    }

    async fn deactivate_global_proxy_once(&mut self) -> Result<()> {
        let catch_all_error = self.remove_catch_all_rules().await.err();
        if let Some(error) = catch_all_error {
            self.global_proxy_active = true;
            return Err(error).context("无法移除全局代理兜底路由，DNS 尚未恢复");
        }
        self.global_proxy_active = false;
        if let Err(error) = self.remove_probe_rules().await {
            tracing::warn!(
                error = %error,
                "宿主兜底路由已移除，但 Exit 数据路径探针规则未能清理；该规则仅匹配专用 mark"
            );
        }

        if let Some(guard) = self.dns_guard.as_mut() {
            guard
                .shutdown()
                .await
                .context("全局代理兜底路由已移除，但恢复 DNS 配置失败")?;
            self.dns_guard = None;
        }
        Ok(())
    }

    /// Return the lookup table used by the catch-all proxy rule.
    #[cfg(test)]
    fn proxy_lookup_table(&self) -> u32 {
        self.table_id
    }

    /// Replace the complete policy-routing plan for a live mode switch.
    ///
    /// Existing peer bypasses are restored after the new plan is installed. If
    /// any step fails, the new plan is rolled back and the previous plan is
    /// reinstalled before an error is returned.
    pub async fn reconfigure(
        &mut self,
        mode: RoutingMode,
        tun_name: impl Into<String>,
        assigned_ips: &VirtualIps,
        virtual_cidrs: &[IpNet],
        direct_cidrs: &[IpNet],
        control_endpoints: &[SocketAddr],
    ) -> Result<()> {
        let previous_plan = self
            .plan
            .clone()
            .context("当前 Linux 路由管理器缺少原始策略，无法安全热切换")?;
        let preserved_peers = self
            .peer_bypasses
            .iter()
            .map(|(client_id, addresses)| (client_id.clone(), addresses.clone()))
            .collect::<Vec<_>>();
        let was_global_proxy_active = self.global_proxy_active;

        if let Err(error) = self.rollback_checked().await {
            return Err(error).context("热切换前无法完整移除旧策略路由，未切换新策略");
        }

        let new_plan = RoutePlan {
            mode,
            tun_name: tun_name.into(),
            assigned_ips: assigned_ips.clone(),
            virtual_cidrs: virtual_cidrs.to_vec(),
            direct_cidrs: direct_cidrs.to_vec(),
            control_endpoints: control_endpoints.to_vec(),
        };

        match Self::install_plan_with_peers(new_plan, &preserved_peers, was_global_proxy_active)
            .await
        {
            Ok(replacement) => {
                *self = replacement;
                Ok(())
            }
            Err(update_error) => {
                match Self::install_plan_with_peers(
                    previous_plan,
                    &preserved_peers,
                    was_global_proxy_active,
                )
                .await
                {
                    Ok(restored) => {
                        *self = restored;
                        Err(update_error).context("热切换策略路由失败，已恢复原策略路由")
                    }
                    Err(restore_error) => Err(anyhow::anyhow!(
                        "热切换策略路由失败: {update_error:#}; 恢复原策略路由也失败: {restore_error:#}"
                    )),
                }
            }
        }
    }

    async fn install_plan_with_peers(
        plan: RoutePlan,
        peer_bypasses: &[(String, Vec<IpAddr>)],
        activate_global_proxy: bool,
    ) -> Result<Self> {
        let mut manager = Self::install(
            plan.mode,
            plan.tun_name.clone(),
            &plan.assigned_ips,
            &plan.virtual_cidrs,
            &plan.direct_cidrs,
            &plan.control_endpoints,
        )
        .await?;

        for (client_id, addresses) in peer_bypasses {
            if let Err(error) = manager
                .add_peer_bypass_addresses(client_id, addresses)
                .await
            {
                let rollback_error = manager.rollback_checked().await.err();
                return match rollback_error {
                    Some(rollback_error) => Err(anyhow::anyhow!(
                        "恢复节点 {client_id} 的物理网络旁路失败: {error:#}; 回滚新策略也失败: {rollback_error:#}"
                    )),
                    None => Err(error).with_context(|| {
                        format!("恢复节点 {client_id} 的物理网络旁路失败，新策略已回滚")
                    }),
                };
            }
        }

        if activate_global_proxy {
            if let Err(error) = manager.activate_global_proxy().await {
                let rollback_error = manager.rollback_checked().await.err();
                return match rollback_error {
                    Some(rollback_error) => Err(anyhow::anyhow!(
                        "重新启用全局代理失败: {error:#}; 回滚新策略也失败: {rollback_error:#}"
                    )),
                    None => Err(error).context("重新启用全局代理失败，新策略已回滚"),
                };
            }
        }

        Ok(manager)
    }

    /// Ensure all currently reachable peer endpoints bypass the tunnel. If any
    /// rule cannot be installed, the complete peer update is rolled back.
    pub async fn add_peer_bypasses(
        &mut self,
        client_id: &str,
        endpoints: &[SocketAddr],
    ) -> Result<()> {
        if !self.is_active() {
            return Ok(());
        }

        let addresses = self.dynamic_addresses(endpoints);
        self.add_peer_bypass_addresses(client_id, &addresses).await
    }

    async fn add_peer_bypass_addresses(
        &mut self,
        client_id: &str,
        addresses: &[IpAddr],
    ) -> Result<()> {
        if !self.is_active() {
            return Ok(());
        }

        if self.peer_bypasses.contains_key(client_id) {
            self.remove_peer_bypasses(client_id).await?;
        }

        let mut addresses = addresses.to_vec();
        addresses.sort_unstable();
        addresses.dedup();
        let existing_addresses: HashSet<IpAddr> = addresses
            .iter()
            .copied()
            .filter(|address| self.dynamic_references.contains_key(address))
            .collect();
        let mut new_rules = Vec::new();

        for address in &addresses {
            if existing_addresses.contains(address)
                || self
                    .static_bypasses
                    .iter()
                    .any(|network| network.contains(address))
            {
                continue;
            }

            let family = Family::of(*address);
            let preference = self.next_preference(family);
            let host = host_network(*address)?;
            let rule = RuleSpec {
                family,
                destination: Some(host),
                fwmark: None,
                action: RuleAction::Lookup(MAIN_TABLE_ID),
                preference,
            };
            if let Err(error) = self.install_rule(&rule).await {
                self.uninstall_rules(&new_rules).await;
                return Err(error)
                    .with_context(|| format!("无法为对端 {client_id} 安装物理网络旁路路由"));
            }
            new_rules.push((*address, rule));
        }

        for (address, rule) in new_rules {
            self.dynamic_references.insert(address, 1);
            self.dynamic_rules.insert(address, rule);
        }
        for address in existing_addresses {
            if let Some(references) = self.dynamic_references.get_mut(&address) {
                *references += 1;
            }
        }
        self.peer_bypasses.insert(client_id.to_string(), addresses);
        Ok(())
    }

    pub async fn remove_peer_bypasses(&mut self, client_id: &str) -> Result<()> {
        let Some(addresses) = self.peer_bypasses.remove(client_id) else {
            return Ok(());
        };

        let mut first_error = None;
        for address in addresses {
            let Some(references) = self.dynamic_references.get_mut(&address) else {
                continue;
            };
            *references = references.saturating_sub(1);
            if *references > 0 {
                continue;
            }
            self.dynamic_references.remove(&address);

            let Some(rule) = self.dynamic_rules.remove(&address) else {
                continue;
            };
            if let Err(error) = self.uninstall_rule(&rule).await {
                tracing::warn!("删除对端旁路规则失败 ({}): {error:#}", address);
                // Keep ownership so final rollback can retry the deletion.
                self.installed_rules.push(rule);
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }

        match first_error {
            Some(error) => Err(error).context("部分对端物理网络旁路规则未能删除"),
            None => Ok(()),
        }
    }

    /// Remove only routes, rules and DNS resources owned by this manager.
    /// Failed resources are retained so a later call can retry them.
    pub async fn rollback(&mut self) -> Result<()> {
        self.rollback_with_runner(&SystemRouteDeleteRunner, ROLLBACK_TIMEOUT)
            .await
    }

    pub(crate) async fn rollback_within(&mut self, total_timeout: Duration) -> Result<()> {
        self.rollback_with_runner(&SystemRouteDeleteRunner, total_timeout)
            .await
    }

    async fn rollback_checked(&mut self) -> Result<()> {
        self.rollback_with_runner(&SystemRouteDeleteRunner, ROLLBACK_TIMEOUT)
            .await
    }

    /// Roll back with a bounded command executor while only removing an owned
    /// spec after its deletion has completed. If this future is cancelled by a
    /// caller timeout, all unfinished specs remain in the manager for retry.
    async fn rollback_with_runner(
        &mut self,
        runner: &dyn RouteDeleteRunner,
        total_timeout: Duration,
    ) -> Result<()> {
        let deadline = Instant::now() + total_timeout;
        let mut failures = Vec::new();

        for rule in self.installed_rules.clone().into_iter().rev() {
            let Some(remaining) = rollback_remaining(deadline) else {
                failures.push("策略路由规则回滚超过总期限，剩余规则已保留".to_string());
                break;
            };
            match timeout(remaining.min(COMMAND_TIMEOUT), runner.delete_rule(&rule)).await {
                Ok(Ok(())) => {
                    self.installed_rules.retain(|installed| installed != &rule);
                }
                Ok(Err(error)) => {
                    tracing::warn!("回滚策略规则失败 (pref={}): {error:#}", rule.preference);
                    failures.push(format!(
                        "策略路由规则 pref={} 回滚失败: {error:#}",
                        rule.preference
                    ));
                }
                Err(_) => {
                    failures.push(format!(
                        "策略路由规则 pref={} 删除超过 {:?}",
                        rule.preference, COMMAND_TIMEOUT
                    ));
                }
            }
        }

        for family in self.installed_table_routes.clone().into_iter().rev() {
            let Some(remaining) = rollback_remaining(deadline) else {
                failures.push("代理路由表回滚超过总期限，剩余路由已保留".to_string());
                break;
            };
            match timeout(
                remaining.min(COMMAND_TIMEOUT),
                runner.delete_table_route(family, self.table_id, &self.tun_name),
            )
            .await
            {
                Ok(Ok(())) => {
                    if let Some(index) = self
                        .installed_table_routes
                        .iter()
                        .position(|installed| *installed == family)
                    {
                        self.installed_table_routes.remove(index);
                    }
                }
                Ok(Err(error)) => {
                    tracing::warn!("回滚代理路由表失败 ({family:?}): {error:#}");
                    failures.push(format!("代理路由表 {family:?} 回滚失败: {error:#}"));
                }
                Err(_) => failures.push(format!(
                    "代理路由表 {family:?} 删除超过 {:?}",
                    COMMAND_TIMEOUT
                )),
            }
        }

        if self.dns_guard.is_some() {
            match rollback_remaining(deadline) {
                None => failures.push("DoH DNS 恢复超过总期限，guard 已保留".to_string()),
                Some(remaining) => {
                    let result = timeout(remaining, async {
                        self.dns_guard
                            .as_mut()
                            .expect("DNS guard checked above")
                            .shutdown()
                            .await
                    })
                    .await;
                    match result {
                        Ok(Ok(())) => {
                            self.dns_guard = None;
                        }
                        Ok(Err(error)) => {
                            tracing::warn!("恢复 DoH DNS 配置失败: {error:#}");
                            failures.push(format!("DNS 配置恢复失败: {error:#}"));
                        }
                        Err(_) => {
                            failures.push("DoH DNS 恢复在期限内未完成，guard 已保留".to_string())
                        }
                    }
                }
            }
        }

        if failures.is_empty() {
            self.dynamic_rules.clear();
            self.dynamic_references.clear();
            self.peer_bypasses.clear();
            self.active_families.clear();
            self.global_proxy_active = false;
            self.static_bypasses.clear();
            self.plan = None;
            Ok(())
        } else {
            failures.sort();
            failures.dedup();
            Err(anyhow::anyhow!(
                "{}；失败资源已保留以便重试",
                failures.join("; ")
            ))
        }
    }

    fn family_active(&self, family: Family) -> bool {
        self.active_families.contains(&family)
    }

    fn owns_catch_all_rule(&self, family: Family, action: RuleAction) -> bool {
        self.installed_rules.iter().any(|rule| {
            rule.family == family
                && rule.destination.is_none()
                && rule.action == action
                && rule.preference == CATCH_ALL_PREFERENCE
        })
    }

    async fn remove_stale_catch_all_rules(&self) -> Result<()> {
        let mut removed = 0usize;
        for family in [Family::V4, Family::V6] {
            if !self.family_active(family) {
                continue;
            }
            let rules = run_ip_query(family, &["rule", "show"])
                .await
                .context("读取策略路由规则以清理异常退出残留失败")?;
            let counts = parse_policy_rule_snapshot(&rules, self.table_id);
            let rule = RuleSpec {
                family,
                destination: None,
                fwmark: None,
                action: RuleAction::Lookup(self.table_id),
                preference: CATCH_ALL_PREFERENCE,
            };
            for _ in 0..counts.catch_all {
                run_ip_rule("del", &rule)
                    .await
                    .context("删除异常退出遗留的全局代理兜底路由失败")?;
                removed += 1;
            }
            let rule = RuleSpec {
                family,
                destination: None,
                fwmark: None,
                action: RuleAction::Unreachable,
                preference: CATCH_ALL_PREFERENCE,
            };
            for _ in 0..counts.unreachable {
                run_ip_rule("del", &rule)
                    .await
                    .context("删除异常退出遗留的 IPv6 失败闭合规则失败")?;
                removed += 1;
            }
        }
        if removed > 0 {
            tracing::warn!(
                removed,
                table = self.table_id,
                interface = %self.tun_name,
                "已清理上次异常退出遗留的全局代理兜底路由"
            );
        }
        Ok(())
    }

    async fn remove_stale_probe_rules(&self) -> Result<()> {
        let mut removed = 0usize;
        for family in [Family::V4, Family::V6] {
            if !self.family_active(family) {
                continue;
            }
            let rules = run_ip_query(family, &["rule", "show"])
                .await
                .context("读取策略路由规则以清理残留 Exit 探针失败")?;
            let count = parse_policy_rule_snapshot(&rules, self.table_id).probe;
            let rule = RuleSpec {
                family,
                destination: None,
                fwmark: Some(EXIT_PATH_PROBE_MARK),
                action: RuleAction::Lookup(self.table_id),
                preference: EXIT_PATH_PROBE_PREFERENCE,
            };
            for _ in 0..count {
                run_ip_rule("del", &rule)
                    .await
                    .context("删除残留 Exit 数据路径探针规则失败")?;
                removed += 1;
            }
        }
        if removed > 0 {
            tracing::warn!(
                removed,
                table = self.table_id,
                interface = %self.tun_name,
                "已清理上次异常退出遗留的 Exit 数据路径探针规则"
            );
        }
        Ok(())
    }

    async fn remove_probe_rules(&mut self) -> Result<()> {
        let rules = self
            .installed_rules
            .iter()
            .filter(|rule| {
                rule.fwmark == Some(EXIT_PATH_PROBE_MARK)
                    && rule.preference == EXIT_PATH_PROBE_PREFERENCE
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut failures = Vec::new();
        for rule in rules.into_iter().rev() {
            let mut last_error = None;
            let mut removed = false;
            for attempt in 1..=PROBE_RULE_REMOVE_ATTEMPTS {
                match self.uninstall_rule(&rule).await {
                    Ok(()) => {
                        removed = true;
                        break;
                    }
                    Err(error) => {
                        tracing::warn!(
                            attempt,
                            attempts = PROBE_RULE_REMOVE_ATTEMPTS,
                            family = ?rule.family,
                            error = %error,
                            "删除 Exit 数据路径探针规则失败，准备重试"
                        );
                        last_error = Some(error);
                    }
                }
                if attempt < PROBE_RULE_REMOVE_ATTEMPTS {
                    tokio::time::sleep(HOST_NETWORK_RESTORE_RETRY_DELAY).await;
                }
            }
            if !removed {
                if let Some(error) = last_error {
                    failures.push(format!(
                        "移除 {:?} Exit 数据路径探针规则失败: {error:#}",
                        rule.family
                    ));
                }
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(anyhow::anyhow!(failures.join("; ")))
        }
    }

    async fn remove_catch_all_rules(&mut self) -> Result<()> {
        let catch_all_rules = self
            .installed_rules
            .iter()
            .filter(|rule| {
                rule.destination.is_none()
                    && rule.fwmark.is_none()
                    && rule.preference == CATCH_ALL_PREFERENCE
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut failures = Vec::new();
        for rule in catch_all_rules.into_iter().rev() {
            if let Err(error) = self.uninstall_rule(&rule).await {
                failures.push(format!(
                    "移除 {:?} 全局代理兜底路由失败: {error:#}",
                    rule.family
                ));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(anyhow::anyhow!(failures.join("; ")))
        }
    }

    fn next_preference(&mut self, family: Family) -> u32 {
        let slot = match family {
            Family::V4 => &mut self.next_v4_preference,
            Family::V6 => &mut self.next_v6_preference,
        };
        let preference = *slot;
        *slot = slot.saturating_add(1);
        preference
    }

    fn dynamic_addresses(&self, endpoints: &[SocketAddr]) -> Vec<IpAddr> {
        let mut addresses = BTreeSet::new();
        for endpoint in endpoints {
            let address = endpoint.ip();
            if !is_dynamic_bypass_address(address) || !self.family_active(Family::of(address)) {
                continue;
            }
            addresses.insert(address);
        }
        addresses.into_iter().collect()
    }

    async fn install_table_route(&mut self, family: Family, source: IpAddr) -> Result<()> {
        let mut command = family_command(family);
        command.args(table_route_args(
            "add",
            self.table_id,
            &self.tun_name,
            source,
        ));
        run_command(command, "添加代理默认路由", false).await?;
        self.installed_table_routes.push(family);
        Ok(())
    }

    async fn install_rule(&mut self, rule: &RuleSpec) -> Result<()> {
        run_ip_rule("add", rule).await?;
        self.installed_rules.push(rule.clone());
        Ok(())
    }

    async fn uninstall_rule(&mut self, rule: &RuleSpec) -> Result<()> {
        run_ip_rule("del", rule).await?;
        self.installed_rules.retain(|installed| installed != rule);
        Ok(())
    }

    async fn uninstall_rules(&mut self, rules: &[(IpAddr, RuleSpec)]) {
        for (_, rule) in rules.iter().rev() {
            if let Err(error) = run_ip_rule("del", rule).await {
                tracing::warn!("回滚对端旁路规则失败: {error:#}");
            }
        }
    }

    /// Re-apply the manager's last complete plan after an OS suspend/resume or
    /// network change. Rollback removes any residue or stale-interface routing
    /// before the plan is installed again, and peer bypasses are preserved.
    pub async fn refresh_plan(&mut self) -> Result<()> {
        let plan = self
            .plan
            .clone()
            .context("当前 Linux 路由管理器缺少原始策略，无法执行自动恢复")?;
        self.reconfigure(
            plan.mode,
            plan.tun_name,
            &plan.assigned_ips,
            &plan.virtual_cidrs,
            &plan.direct_cidrs,
            &plan.control_endpoints,
        )
        .await
    }

    /// Read the kernel's current policy-routing state for this manager. The
    /// snapshot contains counts only so it can be persisted without leaking
    /// interface names or addresses.
    pub async fn recovery_snapshot(&self) -> Result<RouteRecoverySnapshot> {
        let mut snapshot = RouteRecoverySnapshot {
            expected_routes: 0,
            installed_routes: 0,
            missing_routes: 0,
            duplicate_routes: 0,
            residual_routes: 0,
            stale_interface_routes: 0,
        };
        let assigned_ipv6 = self
            .plan
            .as_ref()
            .and_then(|plan| plan.assigned_ips.ipv6)
            .is_some();
        for family in [Family::V4, Family::V6] {
            let lookup_active = self.active_families.contains(&family);
            let fail_closed_active =
                family == Family::V6 && !lookup_active && assigned_ipv6 && self.global_proxy_active;
            if !lookup_active && !fail_closed_active {
                continue;
            }
            snapshot.expected_routes += 1;
            let table = self.table_id.to_string();
            let rules = run_ip_query(family, &["rule", "show"])
                .await
                .context("读取 Linux 策略路由规则失败")?;
            let rule_counts = parse_policy_rule_snapshot(&rules, self.table_id);
            snapshot.residual_routes += rule_counts.residual + rule_counts.probe;

            if fail_closed_active {
                snapshot.residual_routes += rule_counts.catch_all;
                snapshot.duplicate_routes += rule_counts.unreachable.saturating_sub(1);
                if rule_counts.unreachable == 0 {
                    snapshot.missing_routes += 1;
                } else {
                    snapshot.installed_routes += 1;
                }
                continue;
            }

            let routes = run_ip_query(family, &["route", "show", "table", &table])
                .await
                .context("读取 Linux 策略路由表失败")?;
            let route_counts = parse_table_route_snapshot(&routes, self.table_id, &self.tun_name);
            snapshot.residual_routes += route_counts.residual;
            snapshot.stale_interface_routes += route_counts.stale_interface;

            snapshot.duplicate_routes += route_counts.matching.saturating_sub(1);
            if self.global_proxy_active {
                snapshot.residual_routes += rule_counts.unreachable;
                snapshot.duplicate_routes += rule_counts.catch_all.saturating_sub(1);
                if rule_counts.catch_all == 0 || route_counts.matching == 0 {
                    snapshot.missing_routes += 1;
                } else {
                    snapshot.installed_routes += 1;
                }
            } else {
                // Before Exit validation only the private table route and its
                // bypass rules are expected. A catch-all in this phase is an
                // unowned takeover and must be removed by recovery.
                snapshot.residual_routes += rule_counts.catch_all + rule_counts.unreachable;
                if route_counts.matching == 0 {
                    snapshot.missing_routes += 1;
                } else {
                    snapshot.installed_routes += 1;
                }
            }
        }
        Ok(snapshot)
    }
}

impl Drop for LinuxRouteManager {
    fn drop(&mut self) {
        // Explicit shutdown calls rollback asynchronously. Drop covers unwinds
        // and early returns where blocking briefly is preferable to leaving a
        // catch-all proxy rule behind.
        for rule in self.installed_rules.drain(..).rev() {
            let _ = run_ip_rule_sync("del", &rule);
        }
        for family in self.installed_table_routes.drain(..).rev() {
            let _ = remove_table_route_sync(family, self.table_id, &self.tun_name);
        }
    }
}

fn build_bypass_cidrs(
    mode: RoutingMode,
    assigned_ips: &VirtualIps,
    virtual_cidrs: &[IpNet],
    direct_cidrs: &[IpNet],
    control_endpoints: &[SocketAddr],
) -> Result<Vec<IpNet>> {
    if mode == RoutingMode::MeshOnly {
        return Ok(Vec::new());
    }

    let mut bypasses = BTreeSet::new();
    if assigned_ips.ipv4.is_some() {
        for network in local_ipv4_networks() {
            bypasses.insert(IpNet::V4(network));
        }
        for network in direct_cidrs
            .iter()
            .filter(|network| matches!(network, IpNet::V4(_)))
        {
            bypasses.insert(*network);
        }
        for network in virtual_cidrs
            .iter()
            .filter(|network| matches!(network, IpNet::V4(_)))
        {
            bypasses.insert(*network);
        }
    }
    if assigned_ips.ipv6.is_some() {
        for network in local_ipv6_networks() {
            bypasses.insert(IpNet::V6(network));
        }
        for network in direct_cidrs
            .iter()
            .filter(|network| matches!(network, IpNet::V6(_)))
        {
            bypasses.insert(*network);
        }
        for network in virtual_cidrs
            .iter()
            .filter(|network| matches!(network, IpNet::V6(_)))
        {
            bypasses.insert(*network);
        }
    }

    if mode == RoutingMode::BypassMainland {
        for network in fuckipv6_common::routing::mainland_china_cidrs() {
            if (matches!(network, IpNet::V4(_)) && assigned_ips.ipv4.is_some())
                || (matches!(network, IpNet::V6(_)) && assigned_ips.ipv6.is_some())
            {
                bypasses.insert(network);
            }
        }
    }

    for endpoint in control_endpoints {
        let address = endpoint.ip();
        if ((address.is_ipv4() && assigned_ips.ipv4.is_some())
            || (address.is_ipv6() && assigned_ips.ipv6.is_some()))
            && !address.is_unspecified()
            && !address.is_multicast()
        {
            bypasses.insert(host_network(address)?);
        }
    }

    Ok(bypasses.into_iter().collect())
}

fn active_families(assigned_ips: &VirtualIps) -> HashSet<Family> {
    let mut families = HashSet::new();
    if assigned_ips.ipv4.is_some() {
        families.insert(Family::V4);
    }
    if assigned_ips.ipv6.is_some() {
        families.insert(Family::V6);
    }
    families
}

fn local_ipv4_networks() -> Vec<Ipv4Net> {
    [
        "10.0.0.0/8",
        "100.64.0.0/10",
        "127.0.0.0/8",
        "169.254.0.0/16",
        "172.16.0.0/12",
        "192.168.0.0/16",
        "224.0.0.0/4",
        "255.255.255.255/32",
    ]
    .into_iter()
    .filter_map(|network| network.parse().ok())
    .collect()
}

fn local_ipv6_networks() -> Vec<Ipv6Net> {
    ["::1/128", "fc00::/7", "fe80::/10", "ff00::/8"]
        .into_iter()
        .filter_map(|network| network.parse().ok())
        .collect()
}

fn host_network(address: IpAddr) -> Result<IpNet> {
    match address {
        IpAddr::V4(address) => Ok(IpNet::V4(
            Ipv4Net::new(address, 32).context("创建 IPv4 主机路由失败")?,
        )),
        IpAddr::V6(address) => Ok(IpNet::V6(
            Ipv6Net::new(address, 128).context("创建 IPv6 主机路由失败")?,
        )),
    }
}

fn is_dynamic_bypass_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            !address.is_unspecified()
                && !address.is_loopback()
                && !address.is_multicast()
                && !address.is_broadcast()
        }
        IpAddr::V6(address) => {
            !address.is_unspecified()
                && !address.is_loopback()
                && !address.is_multicast()
                && !address.is_unicast_link_local()
        }
    }
}

fn family_of_network(network: IpNet) -> Family {
    match network {
        IpNet::V4(_) => Family::V4,
        IpNet::V6(_) => Family::V6,
    }
}

fn table_id_for_interface(interface: &str) -> u32 {
    let mut hash = 2_166_136_261u32;
    for byte in interface.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    TABLE_ID_BASE + hash % 40_000
}

fn table_route_args(action: &str, table_id: u32, tun_name: &str, source: IpAddr) -> Vec<String> {
    vec![
        "route".to_string(),
        action.to_string(),
        "table".to_string(),
        table_id.to_string(),
        "default".to_string(),
        "dev".to_string(),
        tun_name.to_string(),
        "src".to_string(),
        source.to_string(),
        "proto".to_string(),
        ROUTE_PROTOCOL.to_string(),
    ]
}

fn family_command(family: Family) -> Command {
    let mut command = Command::new("ip");
    if let Some(flag) = family.command_flag() {
        command.arg(flag);
    }
    command
}

fn rollback_remaining(deadline: Instant) -> Option<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    (!remaining.is_zero()).then_some(remaining)
}

async fn run_ip_rule(action: &str, rule: &RuleSpec) -> Result<()> {
    let mut command = family_command(rule.family);
    command.args(["rule", action]);
    if let Some(destination) = rule.destination {
        command.args(["to", &destination.to_string()]);
    }
    if let Some(fwmark) = rule.fwmark {
        command.args(["fwmark", &format!("{fwmark:#x}")]);
    }
    match rule.action {
        RuleAction::Lookup(table) => {
            command.arg("lookup").arg(table.to_string());
        }
        RuleAction::Unreachable => {
            command.arg("unreachable");
        }
    }
    command.args([
        "pref",
        &rule.preference.to_string(),
        "protocol",
        ROUTE_PROTOCOL,
    ]);
    run_command(command, "更新策略路由规则", action == "del").await
}

fn run_ip_rule_sync(action: &str, rule: &RuleSpec) -> Result<()> {
    let mut command = std::process::Command::new("ip");
    if let Some(flag) = rule.family.command_flag() {
        command.arg(flag);
    }
    command.args(["rule", action]);
    if let Some(destination) = rule.destination {
        command.args(["to", &destination.to_string()]);
    }
    if let Some(fwmark) = rule.fwmark {
        command.args(["fwmark", &format!("{fwmark:#x}")]);
    }
    match rule.action {
        RuleAction::Lookup(table) => {
            command.arg("lookup").arg(table.to_string());
        }
        RuleAction::Unreachable => {
            command.arg("unreachable");
        }
    }
    command.args([
        "pref",
        &rule.preference.to_string(),
        "protocol",
        ROUTE_PROTOCOL,
    ]);
    run_sync_command(command, "回滚策略路由规则", action == "del")
}

async fn run_ip_query(family: Family, args: &[&str]) -> Result<String> {
    let mut command = family_command(family);
    command.args(args);
    command.kill_on_drop(true);
    let output = timeout(SYNC_COMMAND_TIMEOUT, command.output())
        .await
        .context("Linux 路由查询超时")?
        .context("无法执行 Linux 路由查询")?;
    if !output.status.success() {
        anyhow::bail!("Linux 路由查询返回失败状态");
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct PolicyRuleRecoveryCounts {
    catch_all: usize,
    unreachable: usize,
    probe: usize,
    residual: usize,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct TableRouteRecoveryCounts {
    matching: usize,
    residual: usize,
    stale_interface: usize,
}

fn parse_policy_rule_snapshot(output: &str, table_id: u32) -> PolicyRuleRecoveryCounts {
    let table = table_id.to_string();
    let mut counts = PolicyRuleRecoveryCounts::default();
    for tokens in output
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>())
    {
        if !rule_has_recovery_protocol(&tokens) {
            continue;
        }
        if rule_has_preference(&tokens, CATCH_ALL_PREFERENCE) {
            if rule_uses_lookup_table(&tokens, &table) {
                counts.catch_all += 1;
            } else if rule_is_unreachable(&tokens) {
                counts.unreachable += 1;
            }
        } else if !rule_uses_lookup_table(&tokens, &table) {
            continue;
        } else if rule_has_preference(&tokens, EXIT_PATH_PROBE_PREFERENCE)
            && rule_has_fwmark(&tokens, EXIT_PATH_PROBE_MARK)
        {
            counts.probe += 1;
        } else {
            counts.residual += 1;
        }
    }
    counts
}

fn parse_table_route_snapshot(
    output: &str,
    table_id: u32,
    tun_name: &str,
) -> TableRouteRecoveryCounts {
    let table = table_id.to_string();
    let mut counts = TableRouteRecoveryCounts::default();
    for tokens in output
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>())
    {
        if !route_has_recovery_protocol(&tokens) || !route_uses_table(&tokens, &table) {
            continue;
        }
        match route_device(&tokens) {
            Some(device) if device == tun_name => counts.matching += 1,
            Some(_) => counts.stale_interface += 1,
            None => counts.residual += 1,
        }
    }
    counts
}

fn rule_has_preference(tokens: &[&str], expected: u32) -> bool {
    tokens
        .first()
        .and_then(|token| token.trim_end_matches(':').parse::<u32>().ok())
        == Some(expected)
}

fn rule_has_fwmark(tokens: &[&str], expected: u32) -> bool {
    tokens.iter().enumerate().any(|(index, token)| {
        token.eq_ignore_ascii_case("fwmark")
            && tokens.get(index + 1).and_then(|value| {
                if let Some(hex) = value
                    .strip_prefix("0x")
                    .or_else(|| value.strip_prefix("0X"))
                {
                    u32::from_str_radix(hex, 16).ok()
                } else {
                    value.parse::<u32>().ok()
                }
            }) == Some(expected)
    })
}

fn rule_has_recovery_protocol(tokens: &[&str]) -> bool {
    tokens
        .windows(2)
        .any(|pair| matches_protocol_field(pair[0]) && recovery_protocol_matches(pair[1]))
}

fn rule_uses_lookup_table(tokens: &[&str], table: &str) -> bool {
    tokens
        .windows(2)
        .any(|pair| pair[0].eq_ignore_ascii_case("lookup") && pair[1] == table)
}

fn rule_is_unreachable(tokens: &[&str]) -> bool {
    tokens
        .iter()
        .any(|token| token.eq_ignore_ascii_case("unreachable"))
}

fn route_has_recovery_protocol(tokens: &[&str]) -> bool {
    tokens
        .windows(2)
        .any(|pair| matches_protocol_field(pair[0]) && recovery_protocol_matches(pair[1]))
}

fn route_uses_table(tokens: &[&str], expected: &str) -> bool {
    let explicit = tokens
        .windows(2)
        .filter(|pair| pair[0].eq_ignore_ascii_case("table"))
        .map(|pair| pair[1])
        .collect::<Vec<_>>();
    explicit.is_empty() || explicit.iter().all(|table| *table == expected)
}

fn route_device<'a>(tokens: &[&'a str]) -> Option<&'a str> {
    tokens
        .windows(2)
        .find(|pair| pair[0].eq_ignore_ascii_case("dev"))
        .map(|pair| pair[1])
}

fn matches_protocol_field(field: &str) -> bool {
    field.eq_ignore_ascii_case("proto")
        || field.eq_ignore_ascii_case("protocol")
        || field.eq_ignore_ascii_case("prot")
}

fn recovery_protocol_matches(value: &str) -> bool {
    value
        .split('/')
        .any(|part| part.eq_ignore_ascii_case(ROUTE_PROTOCOL) || part.eq_ignore_ascii_case("openr"))
}

async fn remove_table_route(family: Family, table_id: u32, tun_name: &str) -> Result<()> {
    let mut command = family_command(family);
    command.args([
        "route",
        "del",
        "table",
        &table_id.to_string(),
        "default",
        "dev",
        tun_name,
        "proto",
        ROUTE_PROTOCOL,
    ]);
    run_command(command, "删除代理默认路由", true).await
}

fn remove_table_route_sync(family: Family, table_id: u32, tun_name: &str) -> Result<()> {
    let mut command = std::process::Command::new("ip");
    if let Some(flag) = family.command_flag() {
        command.arg(flag);
    }
    command.args([
        "route",
        "del",
        "table",
        &table_id.to_string(),
        "default",
        "dev",
        tun_name,
        "proto",
        ROUTE_PROTOCOL,
    ]);
    run_sync_command(command, "回滚代理路由表", true)
}

async fn run_command(mut command: Command, operation: &str, missing_ok: bool) -> Result<()> {
    command.kill_on_drop(true);
    let output = timeout(COMMAND_TIMEOUT, command.output())
        .await
        .map_err(|_| {
            anyhow::Error::new(NetworkSetupError::new(
                NetworkSetupStage::RouteSetup,
                format!("{operation}超过 {:?}", COMMAND_TIMEOUT),
            ))
        })?
        .map_err(|error| {
            anyhow::Error::new(NetworkSetupError::new(
                NetworkSetupStage::RouteSetup,
                format!("{operation}: 无法执行 ip 命令: {error}"),
            ))
        })?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if missing_ok && ip_resource_is_missing(&stderr) {
        return Ok(());
    }
    Err(anyhow::Error::new(network_setup::command_failure(
        NetworkSetupStage::RouteSetup,
        stderr.as_ref(),
    )))
}

fn run_sync_command(
    mut command: std::process::Command,
    operation: &str,
    missing_ok: bool,
) -> Result<()> {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::{Duration as StdDuration, Instant as StdInstant};

    command.stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| {
        anyhow::Error::new(NetworkSetupError::new(
            NetworkSetupStage::RouteSetup,
            format!("{operation}: 无法执行 ip 命令: {error}"),
        ))
    })?;
    let deadline = StdInstant::now() + SYNC_COMMAND_TIMEOUT;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .with_context(|| format!("{operation}: 无法读取进程状态"))?
        {
            break status;
        }
        if StdInstant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow::Error::new(NetworkSetupError::new(
                NetworkSetupStage::RouteSetup,
                format!("{operation}超过 {:?}", SYNC_COMMAND_TIMEOUT),
            )));
        }
        std::thread::sleep(StdDuration::from_millis(10));
    };
    if status.success() {
        return Ok(());
    }

    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    if missing_ok && ip_resource_is_missing(&stderr) {
        return Ok(());
    }
    Err(anyhow::Error::new(network_setup::command_failure(
        NetworkSetupStage::RouteSetup,
        stderr,
    )))
}

fn ip_resource_is_missing(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("no such process")
        || lower.contains("cannot find device")
        || lower.contains("does not exist")
        || lower.contains("no such file or directory")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future;
    use std::net::Ipv4Addr;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeDeleteRunner {
        failed_rules: Mutex<HashSet<u32>>,
        failed_routes: Mutex<HashSet<Family>>,
        hanging_rules: Mutex<HashSet<u32>>,
    }

    impl RouteDeleteRunner for FakeDeleteRunner {
        fn delete_rule<'a>(&'a self, rule: &'a RuleSpec) -> RouteCommandFuture<'a> {
            Box::pin(async move {
                if self
                    .hanging_rules
                    .lock()
                    .expect("hanging rules lock")
                    .contains(&rule.preference)
                {
                    future::pending::<()>().await;
                }
                if self
                    .failed_rules
                    .lock()
                    .expect("failed rules lock")
                    .remove(&rule.preference)
                {
                    anyhow::bail!("injected rule deletion failure");
                }
                Ok(())
            })
        }

        fn delete_table_route<'a>(
            &'a self,
            family: Family,
            _table_id: u32,
            _tun_name: &'a str,
        ) -> RouteCommandFuture<'a> {
            Box::pin(async move {
                if self
                    .failed_routes
                    .lock()
                    .expect("failed routes lock")
                    .remove(&family)
                {
                    anyhow::bail!("injected table route deletion failure");
                }
                Ok(())
            })
        }
    }

    fn dual_stack_ips() -> VirtualIps {
        VirtualIps::dual_stack(Ipv4Addr::new(10, 8, 0, 2), "fd00::1002".parse().unwrap())
    }

    fn manager_with_owned_resources() -> LinuxRouteManager {
        let rule = RuleSpec {
            family: Family::V4,
            destination: Some("203.0.113.0/24".parse().unwrap()),
            fwmark: None,
            action: RuleAction::Lookup(MAIN_TABLE_ID),
            preference: 20_000,
        };
        LinuxRouteManager {
            tun_name: "fuckipv6".to_string(),
            table_id: 12345,
            active_families: [Family::V4, Family::V6].into_iter().collect(),
            global_proxy_active: false,
            static_bypasses: ["10.0.0.0/8".parse().unwrap()].into_iter().collect(),
            installed_rules: vec![rule],
            installed_table_routes: vec![Family::V4, Family::V6],
            dynamic_rules: HashMap::new(),
            dynamic_references: HashMap::new(),
            peer_bypasses: HashMap::new(),
            next_v4_preference: DYNAMIC_PREFERENCE_START,
            next_v6_preference: DYNAMIC_PREFERENCE_START,
            dns_guard: None,
            plan: Some(RoutePlan {
                mode: RoutingMode::GlobalProxy,
                tun_name: "fuckipv6".to_string(),
                assigned_ips: dual_stack_ips(),
                virtual_cidrs: Vec::new(),
                direct_cidrs: Vec::new(),
                control_endpoints: Vec::new(),
            }),
        }
    }

    #[test]
    fn global_proxy_bypasses_private_and_custom_but_not_mainland() {
        let direct = vec!["1.2.3.0/24".parse().unwrap()];
        let virtuals = vec!["fd00::/64".parse().unwrap(), "10.8.0.0/24".parse().unwrap()];
        let bypasses = build_bypass_cidrs(
            RoutingMode::GlobalProxy,
            &dual_stack_ips(),
            &virtuals,
            &direct,
            &[],
        )
        .unwrap();

        assert!(bypasses.contains(&"10.8.0.0/24".parse().unwrap()));
        assert!(bypasses.contains(&"1.2.3.0/24".parse().unwrap()));
        assert!(bypasses.contains(&"192.168.0.0/16".parse().unwrap()));
        assert!(!bypasses.contains(&"114.0.0.0/8".parse().unwrap()));
    }

    #[test]
    fn bypass_mainland_adds_builtin_direct_networks() {
        let bypasses = build_bypass_cidrs(
            RoutingMode::BypassMainland,
            &dual_stack_ips(),
            &["fd00::/64".parse().unwrap(), "10.8.0.0/24".parse().unwrap()],
            &[],
            &[],
        )
        .unwrap();

        assert!(bypasses.contains(&"114.0.0.0/8".parse().unwrap()));
        assert!(bypasses.contains(&"240e::/16".parse().unwrap()));
        assert!(!bypasses.contains(&"8.8.8.0/24".parse().unwrap()));
    }

    #[test]
    fn mesh_only_never_builds_a_proxy_plan() {
        assert!(
            build_bypass_cidrs(RoutingMode::MeshOnly, &dual_stack_ips(), &[], &[], &[])
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn mesh_only_manager_does_not_claim_or_install_policy_routes() {
        let mut manager = LinuxRouteManager::install(
            RoutingMode::MeshOnly,
            "fuckipv6",
            &dual_stack_ips(),
            &[],
            &[],
            &[],
        )
        .await
        .unwrap();

        assert!(!manager.is_active());
        assert!(manager.installed_rules.is_empty());
        assert!(manager.installed_table_routes.is_empty());
        manager
            .add_peer_bypasses(
                "peer",
                &["203.0.113.10:7000".parse().expect("valid endpoint")],
            )
            .await
            .expect("MeshOnly peer update should be a no-op");
        assert!(manager.peer_bypasses.is_empty());
        assert_eq!(
            manager.plan.as_ref().map(|plan| plan.mode),
            Some(RoutingMode::MeshOnly)
        );
    }

    #[test]
    fn control_endpoint_becomes_host_route() {
        let bypasses = build_bypass_cidrs(
            RoutingMode::GlobalProxy,
            &dual_stack_ips(),
            &[],
            &[],
            &["185.217.110.15:6666".parse().unwrap()],
        )
        .unwrap();
        assert!(bypasses.contains(&"185.217.110.15/32".parse().unwrap()));
    }

    #[test]
    fn dynamic_addresses_are_deduplicated_and_skip_link_local() {
        let manager = LinuxRouteManager {
            tun_name: "fuckipv6".to_string(),
            table_id: 12345,
            active_families: [Family::V4, Family::V6].into_iter().collect(),
            global_proxy_active: false,
            static_bypasses: BTreeSet::new(),
            installed_rules: Vec::new(),
            installed_table_routes: Vec::new(),
            dynamic_rules: HashMap::new(),
            dynamic_references: HashMap::new(),
            peer_bypasses: HashMap::new(),
            next_v4_preference: DYNAMIC_PREFERENCE_START,
            next_v6_preference: DYNAMIC_PREFERENCE_START,
            dns_guard: None,
            plan: None,
        };
        let addresses = manager.dynamic_addresses(&[
            "185.217.110.15:6667".parse().unwrap(),
            "185.217.110.15:7000".parse().unwrap(),
            "[fe80::1%2]:7000".parse().unwrap(),
            "[2001:db8::2]:7000".parse().unwrap(),
        ]);
        assert_eq!(
            addresses,
            vec![
                "185.217.110.15".parse::<IpAddr>().unwrap(),
                "2001:db8::2".parse::<IpAddr>().unwrap()
            ]
        );
    }

    #[test]
    fn proxy_catch_all_uses_private_table_while_bypasses_use_main() {
        let manager = LinuxRouteManager {
            tun_name: "fuckipv6".to_string(),
            table_id: 12345,
            active_families: [Family::V4, Family::V6].into_iter().collect(),
            global_proxy_active: false,
            static_bypasses: BTreeSet::new(),
            installed_rules: Vec::new(),
            installed_table_routes: Vec::new(),
            dynamic_rules: HashMap::new(),
            dynamic_references: HashMap::new(),
            peer_bypasses: HashMap::new(),
            next_v4_preference: DYNAMIC_PREFERENCE_START,
            next_v6_preference: DYNAMIC_PREFERENCE_START,
            dns_guard: None,
            plan: None,
        };
        assert_eq!(manager.proxy_lookup_table(), 12345);
        assert_eq!(MAIN_TABLE_ID, 254);
    }

    #[test]
    fn proxy_table_route_uses_assigned_virtual_source() {
        let ipv4 = table_route_args("add", 12345, "fuckipv6", "10.8.0.2".parse().unwrap());
        assert!(ipv4.windows(2).any(|pair| pair == ["src", "10.8.0.2"]));
        assert!(ipv4
            .windows(2)
            .any(|pair| pair == ["proto", ROUTE_PROTOCOL]));

        let ipv6 = table_route_args("add", 12345, "fuckipv6", "fd00::1002".parse().unwrap());
        assert!(ipv6.windows(2).any(|pair| pair == ["src", "fd00::1002"]));
    }

    #[test]
    fn interface_table_ids_are_stable_and_distinct() {
        assert_eq!(
            table_id_for_interface("fuckipv6"),
            table_id_for_interface("fuckipv6")
        );
        assert_ne!(
            table_id_for_interface("fuckipv6-a"),
            table_id_for_interface("fuckipv6-b")
        );
        assert!(
            (TABLE_ID_BASE..=TABLE_ID_BASE + 39_999).contains(&table_id_for_interface("fuckipv6"))
        );
    }

    #[tokio::test]
    async fn rollback_removes_owners_only_after_successful_deletion() {
        let runner = FakeDeleteRunner::default();
        let mut manager = manager_with_owned_resources();

        manager
            .rollback_with_runner(&runner, Duration::from_secs(1))
            .await
            .expect("all fake deletions must succeed");

        assert!(manager.installed_rules.is_empty());
        assert!(manager.installed_table_routes.is_empty());
        assert!(manager.static_bypasses.is_empty());
        assert!(manager.active_families.is_empty());
        assert!(manager.plan.is_none());
    }

    #[tokio::test]
    async fn failed_deletion_retains_owner_and_can_be_retried() {
        let runner = FakeDeleteRunner::default();
        runner
            .failed_rules
            .lock()
            .expect("failed rules lock")
            .insert(20_000);
        let mut manager = manager_with_owned_resources();

        manager
            .rollback_with_runner(&runner, Duration::from_secs(1))
            .await
            .expect_err("injected rule failure must be reported");
        assert_eq!(manager.installed_rules.len(), 1);
        assert!(manager.plan.is_some());

        manager
            .rollback_with_runner(&runner, Duration::from_secs(1))
            .await
            .expect("the retained rule must be retryable");
        assert!(manager.installed_rules.is_empty());
        assert!(manager.installed_table_routes.is_empty());
        assert!(manager.plan.is_none());
    }

    #[tokio::test]
    async fn hanging_deletion_hits_deadline_and_retains_owner() {
        let runner = FakeDeleteRunner::default();
        runner
            .hanging_rules
            .lock()
            .expect("hanging rules lock")
            .insert(20_000);
        let mut manager = manager_with_owned_resources();
        let started = Instant::now();

        manager
            .rollback_with_runner(&runner, Duration::from_millis(25))
            .await
            .expect_err("a hanging deletion must fail bounded");

        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(manager.installed_rules.len(), 1);
        assert_eq!(manager.installed_table_routes.len(), 2);
        assert!(manager.plan.is_some());
    }

    #[test]
    fn recovery_rule_parser_owns_only_matching_protocol_and_table() {
        let output = "0: from all lookup local
32000: from all lookup 10000 proto openr
32000: from all unreachable proto 99
32001: from all lookup 10000 protocol 99
32002: from all lookup 10000 prot 99/openr
32003: from all lookup 10000 proto 98
32004: from all lookup main proto openr
32005: from all lookup 10000 proto 99
900: from all fwmark 0x46503601 lookup 10000 proto 99
";
        assert_eq!(
            parse_policy_rule_snapshot(output, 10_000),
            PolicyRuleRecoveryCounts {
                catch_all: 1,
                unreachable: 1,
                probe: 1,
                residual: 3,
            }
        );
    }

    #[test]
    fn recovery_rule_parser_accepts_preference_without_colon() {
        let output = "32000 from all lookup 10000 protocol 99/openr\n";
        assert_eq!(
            parse_policy_rule_snapshot(output, 10_000),
            PolicyRuleRecoveryCounts {
                catch_all: 1,
                unreachable: 0,
                probe: 0,
                residual: 0,
            }
        );
    }

    #[test]
    fn recovery_route_parser_requires_owned_protocol_table_and_interface() {
        let output = "default dev fuckipv6 proto openr scope link src 10.8.0.2
default dev fuckipv6 proto 99 scope link src 10.8.0.2
default dev stale0 proto openr scope link
unreachable default proto openr
default dev fuckipv6 proto 98 scope link
default dev fuckipv6 scope link
default dev fuckipv6 proto openr table 20000
default dev fuckipv6 proto openr table 10000
";
        assert_eq!(
            parse_table_route_snapshot(output, 10_000, "fuckipv6"),
            TableRouteRecoveryCounts {
                matching: 3,
                residual: 1,
                stale_interface: 1,
            }
        );
    }

    #[test]
    fn missing_ip_resources_are_complete_deletions() {
        for stderr in [
            "RTNETLINK answers: No such process",
            "Cannot find device \"fuckipv6\"",
            "object does not exist",
            "No such file or directory",
        ] {
            assert!(ip_resource_is_missing(stderr), "not recognized: {stderr}");
        }
        assert!(!ip_resource_is_missing("Operation not permitted"));
    }
}
