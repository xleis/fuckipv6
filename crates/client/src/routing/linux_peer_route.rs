//! Per-peer overlay host routes for the Linux TUN.
//!
//! An address assigned to the TUN creates an on-link prefix route, which is not
//! sufficient when more than one virtual network shares the same address pool
//! on one host. Pin every authenticated peer address to this TUN with an exact
//! route so replies and locally generated packets cannot be steered through a
//! different interface.

use anyhow::{Context, Result};
use fuckipv6_common::VirtualIps;
use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::{timeout, Instant};

const ROUTE_PROTOCOL: &str = "99";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandInvocation {
    program: &'static str,
    args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandResult {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

type CommandFuture<'a> = Pin<Box<dyn Future<Output = Result<CommandResult>> + Send + 'a>>;

trait PeerRouteRunner: Send + Sync {
    fn run(&self, invocation: CommandInvocation) -> CommandFuture<'_>;
}

#[derive(Debug)]
struct SystemPeerRouteRunner;

impl PeerRouteRunner for SystemPeerRouteRunner {
    fn run(&self, invocation: CommandInvocation) -> CommandFuture<'_> {
        Box::pin(async move {
            let mut command = tokio::process::Command::new(invocation.program);
            command.args(&invocation.args).kill_on_drop(true);
            let output = timeout(COMMAND_TIMEOUT, command.output())
                .await
                .map_err(|_| {
                    anyhow::anyhow!("{} 执行超过 {:?}", invocation.program, COMMAND_TIMEOUT)
                })?
                .with_context(|| format!("无法执行 {}", invocation.program))?;
            Ok(CommandResult {
                success: output.status.success(),
                stdout: output.stdout,
                stderr: output.stderr,
            })
        })
    }
}

#[derive(Debug)]
struct RouteLease {
    owners: BTreeSet<String>,
    owned: bool,
}

/// Owns exact host routes for authenticated peers attached to one TUN.
pub(crate) struct LinuxPeerRouteManager {
    tun_name: String,
    peers: HashMap<String, BTreeSet<IpAddr>>,
    routes: HashMap<IpAddr, RouteLease>,
    runner: Arc<dyn PeerRouteRunner>,
}

impl LinuxPeerRouteManager {
    pub(crate) fn new(tun_name: impl Into<String>) -> Self {
        Self::with_runner(tun_name, Arc::new(SystemPeerRouteRunner))
    }

    fn with_runner(tun_name: impl Into<String>, runner: Arc<dyn PeerRouteRunner>) -> Self {
        Self {
            tun_name: tun_name.into(),
            peers: HashMap::new(),
            routes: HashMap::new(),
            runner,
        }
    }

    /// Reconcile the exact routes owned for one authenticated peer.
    pub(crate) async fn sync_peer(
        &mut self,
        client_id: &str,
        virtual_ips: &VirtualIps,
    ) -> Result<()> {
        let desired = peer_route_addresses(virtual_ips);
        let previous = self.peers.get(client_id).cloned().unwrap_or_default();
        let mut acquired = Vec::new();

        for address in desired.difference(&previous).copied() {
            match self.acquire(client_id, address).await {
                Ok(owner_added) => acquired.push((address, owner_added)),
                Err(error) => {
                    let rollback_error = self.rollback_acquired(client_id, &acquired).await.err();
                    return match rollback_error {
                        Some(rollback_error) => Err(anyhow::anyhow!(
                            "为节点 {client_id} 安装虚拟地址精确定向路由失败: {error:#}; 回滚失败: {rollback_error:#}"
                        )),
                        None => Err(error)
                            .with_context(|| format!("为节点 {client_id} 安装虚拟地址精确定向路由失败")),
                    };
                }
            }
        }

        let mut retained = BTreeSet::new();
        for address in previous.difference(&desired).copied() {
            if let Err(error) = self.release(client_id, address).await {
                tracing::warn!(
                    client = client_id,
                    address = %address,
                    "无法撤销节点旧虚拟地址精确定向路由，将保留所有权等待重试: {error:#}"
                );
                retained.insert(address);
            }
        }

        let mut current = desired;
        current.extend(retained);
        if current.is_empty() {
            self.peers.remove(client_id);
        } else {
            self.peers.insert(client_id.to_string(), current);
        }
        Ok(())
    }

    pub(crate) async fn remove_peer(&mut self, client_id: &str) -> Result<()> {
        let Some(addresses) = self.peers.get(client_id).cloned() else {
            return Ok(());
        };
        let mut failed_addresses = BTreeSet::new();
        let mut failures = Vec::new();
        for address in addresses {
            if let Err(error) = self.release(client_id, address).await {
                failed_addresses.insert(address);
                failures.push(format!("{address}: {error:#}"));
            }
        }
        if failures.is_empty() {
            self.peers.remove(client_id);
            Ok(())
        } else {
            self.peers.insert(client_id.to_string(), failed_addresses);
            Err(anyhow::anyhow!(
                "撤销节点 {client_id} 的部分虚拟地址路由失败: {}",
                failures.join("; ")
            ))
        }
    }

    /// Remove every route this process created. Exact routes that already
    /// belonged to this TUN are left alone and will disappear with the link.
    pub(crate) async fn rollback_within(&mut self, total_timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + total_timeout;
        let addresses = self.routes.keys().copied().collect::<Vec<_>>();
        let mut failures = Vec::new();

        for address in addresses {
            let Some(remaining) = remaining(deadline) else {
                failures.push("撤销虚拟地址精确定向路由超过总期限".to_string());
                break;
            };
            let Some(lease) = self.routes.get(&address) else {
                continue;
            };
            if !lease.owned {
                self.routes.remove(&address);
                continue;
            }
            match self.delete_route_for(address, remaining).await {
                Ok(()) => {
                    self.routes.remove(&address);
                }
                Err(error) => failures.push(format!("{address}: {error:#}")),
            }
        }
        self.peers.clear();

        if failures.is_empty() {
            Ok(())
        } else {
            failures.sort();
            failures.dedup();
            Err(anyhow::anyhow!("{}", failures.join("; ")))
        }
    }

    async fn acquire(&mut self, client_id: &str, address: IpAddr) -> Result<bool> {
        if let Some(lease) = self.routes.get_mut(&address) {
            return Ok(lease.owners.insert(client_id.to_string()));
        }

        let owned = self.ensure_route(address).await?;
        self.routes.insert(
            address,
            RouteLease {
                owners: BTreeSet::from([client_id.to_string()]),
                owned,
            },
        );
        Ok(true)
    }

    async fn release(&mut self, client_id: &str, address: IpAddr) -> Result<()> {
        let remove_route = {
            let Some(lease) = self.routes.get_mut(&address) else {
                return Ok(());
            };
            lease.owners.remove(client_id);
            if !lease.owners.is_empty() {
                return Ok(());
            }
            lease.owned
        };

        if remove_route {
            if let Err(error) = self.delete_route(address).await {
                if let Some(lease) = self.routes.get_mut(&address) {
                    lease.owners.insert(client_id.to_string());
                }
                return Err(error);
            }
        }
        self.routes.remove(&address);
        Ok(())
    }

    async fn rollback_acquired(
        &mut self,
        client_id: &str,
        acquired: &[(IpAddr, bool)],
    ) -> Result<()> {
        let mut failures = Vec::new();
        for (address, owner_added) in acquired.iter().rev() {
            if *owner_added {
                if let Err(error) = self.release(client_id, *address).await {
                    failures.push(format!("{address}: {error:#}"));
                }
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(anyhow::anyhow!("{}", failures.join("; ")))
        }
    }

    async fn ensure_route(&self, address: IpAddr) -> Result<bool> {
        let prefix = host_prefix(address);
        let existing = self
            .run_ip(address, &["route", "show", "exact", &prefix])
            .await
            .with_context(|| format!("检查虚拟地址精确定向路由失败: {prefix}"))?;
        if !existing.success {
            anyhow::bail!(
                "检查虚拟地址精确定向路由失败 ({prefix}): {}",
                String::from_utf8_lossy(&existing.stderr).trim()
            );
        }
        let existing = String::from_utf8_lossy(&existing.stdout);
        if !existing.trim().is_empty() {
            if route_uses_device(&existing, &self.tun_name) {
                return Ok(false);
            }
            anyhow::bail!("虚拟地址 {prefix} 已被其他网卡占用: {}", existing.trim());
        }

        let output = self
            .run_ip(
                address,
                &[
                    "route",
                    "add",
                    &prefix,
                    "dev",
                    &self.tun_name,
                    "proto",
                    ROUTE_PROTOCOL,
                ],
            )
            .await
            .with_context(|| format!("添加虚拟地址精确定向路由失败: {prefix}"))?;
        if !output.success {
            anyhow::bail!(
                "添加虚拟地址精确定向路由失败 ({prefix}): {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(true)
    }

    async fn delete_route(&self, address: IpAddr) -> Result<()> {
        let remaining = COMMAND_TIMEOUT;
        self.delete_route_for(address, remaining).await
    }

    async fn delete_route_for(&self, address: IpAddr, remaining: Duration) -> Result<()> {
        let prefix = host_prefix(address);
        let output = timeout(
            remaining.min(COMMAND_TIMEOUT),
            self.runner.run(ip_invocation(
                address,
                &[
                    "route",
                    "del",
                    &prefix,
                    "dev",
                    &self.tun_name,
                    "proto",
                    ROUTE_PROTOCOL,
                ],
            )),
        )
        .await
        .map_err(|_| anyhow::anyhow!("删除虚拟地址精确定向路由 {prefix} 超过期限"))?
        .with_context(|| format!("删除虚拟地址精确定向路由失败: {prefix}"))?;
        if !output.success {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if route_is_already_missing(&stderr) {
                return Ok(());
            }
            anyhow::bail!("删除虚拟地址精确定向路由失败 ({prefix}): {}", stderr.trim());
        }
        Ok(())
    }

    async fn run_ip(&self, address: IpAddr, args: &[&str]) -> Result<CommandResult> {
        self.runner.run(ip_invocation(address, args)).await
    }
}

fn peer_route_addresses(virtual_ips: &VirtualIps) -> BTreeSet<IpAddr> {
    [
        virtual_ips.ipv4.map(IpAddr::V4),
        virtual_ips.ipv6.map(IpAddr::V6),
    ]
    .into_iter()
    .flatten()
    .filter(|address| !address.is_unspecified())
    .collect()
}

fn ip_invocation(address: IpAddr, args: &[&str]) -> CommandInvocation {
    let mut command_args = Vec::with_capacity(args.len() + usize::from(address.is_ipv6()));
    if address.is_ipv6() {
        command_args.push("-6".to_string());
    }
    command_args.extend(args.iter().map(|arg| (*arg).to_string()));
    CommandInvocation {
        program: "ip",
        args: command_args,
    }
}

fn host_prefix(address: IpAddr) -> String {
    match address {
        IpAddr::V4(address) => format!("{address}/32"),
        IpAddr::V6(address) => format!("{address}/128"),
    }
}

fn route_uses_device(route: &str, tun_name: &str) -> bool {
    route.lines().any(|line| {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        fields
            .windows(2)
            .any(|window| window[0] == "dev" && window[1] == tun_name)
    })
}

fn route_is_already_missing(stderr: &str) -> bool {
    stderr.contains("No such process")
        || stderr.contains("Cannot find device")
        || stderr.contains("No such file or directory")
}

fn remaining(deadline: Instant) -> Option<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    (!remaining.is_zero()).then_some(remaining)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::net::{Ipv4Addr, Ipv6Addr};
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeRunner {
        routes: Mutex<HashMap<(bool, String), String>>,
        failed_deletes: Mutex<HashSet<(bool, String)>>,
        calls: Mutex<Vec<CommandInvocation>>,
    }

    impl FakeRunner {
        fn insert_route(&self, ipv6: bool, prefix: &str, route: &str) {
            self.routes
                .lock()
                .unwrap()
                .insert((ipv6, prefix.to_string()), route.to_string());
        }

        fn route_count(&self) -> usize {
            self.routes.lock().unwrap().len()
        }

        fn fail_delete(&self, ipv6: bool, prefix: &str) {
            self.failed_deletes
                .lock()
                .unwrap()
                .insert((ipv6, prefix.to_string()));
        }

        fn allow_delete(&self, ipv6: bool, prefix: &str) {
            self.failed_deletes
                .lock()
                .unwrap()
                .remove(&(ipv6, prefix.to_string()));
        }
    }

    impl PeerRouteRunner for FakeRunner {
        fn run(&self, invocation: CommandInvocation) -> CommandFuture<'_> {
            Box::pin(async move {
                self.calls.lock().unwrap().push(invocation.clone());
                let ipv6 = invocation.args.first().is_some_and(|arg| arg == "-6");
                let args = if ipv6 {
                    &invocation.args[1..]
                } else {
                    invocation.args.as_slice()
                };
                let args = args.iter().map(String::as_str).collect::<Vec<_>>();
                let mut routes = self.routes.lock().unwrap();
                match args.as_slice() {
                    ["route", "show", "exact", prefix] => Ok(CommandResult {
                        success: true,
                        stdout: routes
                            .get(&(ipv6, (*prefix).to_string()))
                            .cloned()
                            .unwrap_or_default()
                            .into_bytes(),
                        stderr: Vec::new(),
                    }),
                    ["route", "add", prefix, "dev", tun, "proto", "99"] => {
                        let key = (ipv6, (*prefix).to_string());
                        if routes.contains_key(&key) {
                            return Ok(CommandResult {
                                success: false,
                                stdout: Vec::new(),
                                stderr: b"File exists".to_vec(),
                            });
                        }
                        routes.insert(key, format!("{prefix} dev {tun} proto 99"));
                        Ok(CommandResult {
                            success: true,
                            stdout: Vec::new(),
                            stderr: Vec::new(),
                        })
                    }
                    ["route", "del", prefix, "dev", tun, "proto", "99"] => {
                        let key = (ipv6, (*prefix).to_string());
                        if self.failed_deletes.lock().unwrap().contains(&key) {
                            return Ok(CommandResult {
                                success: false,
                                stdout: Vec::new(),
                                stderr: b"Operation not permitted".to_vec(),
                            });
                        }
                        let matches = routes
                            .get(&key)
                            .is_some_and(|route| route.contains(&format!("dev {tun}")));
                        if matches {
                            routes.remove(&key);
                            Ok(CommandResult {
                                success: true,
                                stdout: Vec::new(),
                                stderr: Vec::new(),
                            })
                        } else {
                            Ok(CommandResult {
                                success: false,
                                stdout: Vec::new(),
                                stderr: b"No such process".to_vec(),
                            })
                        }
                    }
                    other => panic!("unexpected ip invocation: {other:?}"),
                }
            })
        }
    }

    fn test_peer() -> VirtualIps {
        VirtualIps::dual_stack(
            "10.8.0.2".parse::<Ipv4Addr>().unwrap(),
            "fd00::1002".parse::<Ipv6Addr>().unwrap(),
        )
    }

    #[tokio::test]
    async fn installs_and_removes_exact_routes_for_both_families() {
        let runner = Arc::new(FakeRunner::default());
        let mut manager = LinuxPeerRouteManager::with_runner("fk-test", runner.clone());

        manager.sync_peer("peer-1", &test_peer()).await.unwrap();
        assert_eq!(runner.route_count(), 2);
        assert!(runner
            .routes
            .lock()
            .unwrap()
            .contains_key(&(false, "10.8.0.2/32".to_string())));
        assert!(runner
            .routes
            .lock()
            .unwrap()
            .contains_key(&(true, "fd00::1002/128".to_string())));

        manager.remove_peer("peer-1").await.unwrap();
        assert_eq!(runner.route_count(), 0);
    }

    #[tokio::test]
    async fn refuses_to_steal_an_exact_route_owned_by_another_tun() {
        let runner = Arc::new(FakeRunner::default());
        runner.insert_route(false, "10.8.0.2/32", "10.8.0.2/32 dev fk-other proto 99");
        let mut manager = LinuxPeerRouteManager::with_runner("fk-test", runner.clone());

        let error = manager.sync_peer("peer-1", &test_peer()).await.unwrap_err();
        assert!(format!("{error:#}").contains("已被其他网卡占用"));
        assert!(!runner
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| { call.args.iter().any(|arg| arg == "add") }));
    }

    #[tokio::test]
    async fn leaves_an_existing_exact_route_on_the_same_tun_untouched() {
        let runner = Arc::new(FakeRunner::default());
        runner.insert_route(false, "10.8.0.2/32", "10.8.0.2/32 dev fk-test scope link");
        let mut manager = LinuxPeerRouteManager::with_runner("fk-test", runner.clone());

        manager.sync_peer("peer-1", &test_peer()).await.unwrap();
        manager.remove_peer("peer-1").await.unwrap();

        assert!(runner
            .routes
            .lock()
            .unwrap()
            .contains_key(&(false, "10.8.0.2/32".to_string())));
    }

    #[tokio::test]
    async fn repeated_sync_does_not_reinstall_existing_routes() {
        let runner = Arc::new(FakeRunner::default());
        let mut manager = LinuxPeerRouteManager::with_runner("fk-test", runner.clone());

        manager.sync_peer("peer-1", &test_peer()).await.unwrap();
        let add_calls = runner
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.args.iter().any(|arg| arg == "add"))
            .count();
        manager.sync_peer("peer-1", &test_peer()).await.unwrap();
        let add_calls_after = runner
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.args.iter().any(|arg| arg == "add"))
            .count();

        assert_eq!(add_calls, 2);
        assert_eq!(add_calls_after, add_calls);
    }

    #[tokio::test]
    async fn failed_peer_removal_retains_ownership_for_retry() {
        let runner = Arc::new(FakeRunner::default());
        let mut manager = LinuxPeerRouteManager::with_runner("fk-test", runner.clone());

        manager.sync_peer("peer-1", &test_peer()).await.unwrap();
        runner.fail_delete(false, "10.8.0.2/32");

        let error = manager.remove_peer("peer-1").await.unwrap_err();
        assert!(format!("{error:#}").contains("10.8.0.2"));
        assert!(manager.peers.contains_key("peer-1"));
        assert_eq!(runner.route_count(), 1);

        runner.allow_delete(false, "10.8.0.2/32");
        manager.remove_peer("peer-1").await.unwrap();
        assert!(!manager.peers.contains_key("peer-1"));
        assert_eq!(runner.route_count(), 0);
    }

    #[tokio::test]
    async fn rollback_removes_routes_created_by_the_manager() {
        let runner = Arc::new(FakeRunner::default());
        let mut manager = LinuxPeerRouteManager::with_runner("fk-test", runner.clone());

        manager.sync_peer("peer-1", &test_peer()).await.unwrap();
        manager
            .rollback_within(Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(runner.route_count(), 0);
    }
}
