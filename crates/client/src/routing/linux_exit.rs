//! Linux forwarding and NAT resources for an Exit Node.
//!
//! Every firewall rule and host route is owned by this process. Rules live in
//! dedicated iptables/ip6tables chains so existing relay policy is not edited,
//! and cleanup only removes resources recorded by this manager.

use anyhow::{Context, Result};
use fuckipv6_common::{ExitNodeConfig, VirtualIps};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::future::Future;
use std::net::IpAddr;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

const ROUTE_PROTOCOL: &str = "99";
const IPTABLES_WAIT_SECONDS: &str = "5";
const MAX_STALE_JUMP_RULES: usize = 32;

#[derive(Debug, Clone)]
struct IptablesRule {
    program: &'static str,
    table: Option<&'static str>,
    /// Complete rule arguments beginning with `-A`.
    args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CommandInvocation {
    program: String,
    args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandResult {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

type CommandFuture<'a> = Pin<Box<dyn Future<Output = Result<CommandResult>> + Send + 'a>>;

trait ExitCommandRunner: Send + Sync {
    fn run(&self, invocation: CommandInvocation) -> CommandFuture<'_>;
    fn run_sync(&self, invocation: CommandInvocation) -> Result<CommandResult>;
    fn read_file(&self, path: &str) -> Result<String>;
    fn write_file(&self, path: &str, contents: &[u8]) -> Result<()>;
}

#[derive(Debug)]
struct SystemCommandRunner;

impl ExitCommandRunner for SystemCommandRunner {
    fn run(&self, invocation: CommandInvocation) -> CommandFuture<'_> {
        Box::pin(async move {
            let output = tokio::process::Command::new(&invocation.program)
                .args(&invocation.args)
                .output()
                .await
                .with_context(|| format!("无法执行 {}", invocation.program))?;
            Ok(CommandResult {
                success: output.status.success(),
                stdout: output.stdout,
                stderr: output.stderr,
            })
        })
    }

    fn run_sync(&self, invocation: CommandInvocation) -> Result<CommandResult> {
        let output = std::process::Command::new(&invocation.program)
            .args(&invocation.args)
            .output()
            .with_context(|| format!("无法执行 {}", invocation.program))?;
        Ok(CommandResult {
            success: output.status.success(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    fn read_file(&self, path: &str) -> Result<String> {
        Ok(std::fs::read_to_string(path)?)
    }

    fn write_file(&self, path: &str, contents: &[u8]) -> Result<()> {
        Ok(std::fs::write(Path::new(path), contents)?)
    }
}

#[derive(Debug)]
struct ClientForwardState {
    addresses: HashSet<IpAddr>,
    owned_routes: Vec<IpAddr>,
    rules: Vec<IptablesRule>,
}

/// Owns forwarding, NAT and return-route resources for one Exit Node process.
pub struct LinuxExitForwarder {
    tun_name: String,
    external_v4_interface: String,
    external_v6_interface: Option<String>,
    forward_chain: String,
    nat_chain: String,
    clients: HashMap<String, ClientForwardState>,
    ipv4_forward_changed: bool,
    ipv6_forward_changed: bool,
    cleaned: bool,
    runner: Arc<dyn ExitCommandRunner>,
}

impl fmt::Debug for LinuxExitForwarder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LinuxExitForwarder")
            .field("tun_name", &self.tun_name)
            .field("external_v4_interface", &self.external_v4_interface)
            .field("external_v6_interface", &self.external_v6_interface)
            .field("forward_chain", &self.forward_chain)
            .field("nat_chain", &self.nat_chain)
            .field("clients", &self.clients)
            .field("cleaned", &self.cleaned)
            .finish_non_exhaustive()
    }
}

impl LinuxExitForwarder {
    /// Install an Exit Node only when `enabled` is set.
    pub async fn install(config: &ExitNodeConfig, tun_name: &str) -> Result<Option<Self>> {
        if !config.enabled {
            return Ok(None);
        }

        let runner: Arc<dyn ExitCommandRunner> = Arc::new(SystemCommandRunner);
        let external_v4_interface = default_route_interface(runner.as_ref(), false)
            .await
            .context("无法检测 Exit Node 的 IPv4 默认出口网卡")?;
        let external_v6_interface = default_route_interface(runner.as_ref(), true).await.ok();
        // Chain ownership is keyed by TUN name rather than PID so a crash and
        // restart can discover and purge resources left by the previous process.
        let forward_chain = chain_name("F", tun_name);
        let nat_chain = chain_name("N", tun_name);

        let mut manager = Self {
            tun_name: tun_name.to_string(),
            external_v4_interface,
            external_v6_interface,
            forward_chain,
            nat_chain,
            clients: HashMap::new(),
            ipv4_forward_changed: false,
            ipv6_forward_changed: false,
            cleaned: false,
            runner,
        };

        manager
            .install_resources()
            .await
            .context("安装 Exit Node 转发与 NAT 失败，已回滚")?;

        tracing::info!(
            "Exit Node 内核转发已就绪: tun={}, ipv4_out={}, ipv6_out={:?}, chains={}/{}",
            manager.tun_name,
            manager.external_v4_interface,
            manager.external_v6_interface,
            manager.forward_chain,
            manager.nat_chain
        );
        Ok(Some(manager))
    }

    async fn install_resources(&mut self) -> Result<()> {
        if let Err(error) = self.install_forwarding().await {
            self.rollback();
            return Err(error);
        }
        Ok(())
    }

    /// Whether the host has a usable outbound path for this family.
    pub fn supports(&self, destination: IpAddr) -> bool {
        match destination {
            IpAddr::V4(_) => true,
            IpAddr::V6(_) => self.external_v6_interface.is_some(),
        }
    }

    /// Grant forwarding resources to one authenticated client.
    pub async fn allow_client(&mut self, client_id: &str, virtual_ips: &VirtualIps) -> Result<()> {
        let addresses: HashSet<IpAddr> = [
            virtual_ips.ipv4.map(IpAddr::V4),
            virtual_ips.ipv6.map(IpAddr::V6),
        ]
        .into_iter()
        .flatten()
        .filter(|ip| !ip.is_unspecified())
        .collect();

        if let Some(existing) = self.clients.get(client_id) {
            if existing.addresses == addresses {
                return Ok(());
            }
            self.remove_client(client_id).await?;
        }

        let mut state = ClientForwardState {
            addresses: addresses.clone(),
            owned_routes: Vec::new(),
            rules: Vec::new(),
        };

        let result = async {
            let mut ordered_addresses: Vec<_> = addresses.into_iter().collect();
            ordered_addresses.sort_unstable();

            for address in ordered_addresses {
                if ensure_return_route(self.runner.as_ref(), &self.tun_name, address).await? {
                    state.owned_routes.push(address);
                }
            }

            if let Some(address) = virtual_ips.ipv4 {
                let source = host_prefix(IpAddr::V4(address))?;
                self.add_client_rules("iptables", &source, &mut state.rules)
                    .await?;
            }

            if let (Some(address), Some(interface)) =
                (virtual_ips.ipv6, self.external_v6_interface.as_deref())
            {
                let source = host_prefix(IpAddr::V6(address))?;
                self.add_client_rules_for_interface(
                    "ip6tables",
                    interface,
                    &source,
                    &mut state.rules,
                )
                .await?;
            }

            Ok::<(), anyhow::Error>(())
        }
        .await;

        if let Err(error) = result {
            rollback_client_state_for_tun(self.runner.as_ref(), &self.tun_name, &mut state).await;
            return Err(error)
                .with_context(|| format!("无法为客户端 {client_id} 安装 Exit Node 转发资源"));
        }

        self.clients.insert(client_id.to_string(), state);
        tracing::info!(
            "Exit Node 已授权客户端 {}: {:?}",
            client_id,
            self.clients
                .get(client_id)
                .map(|state| state.addresses.iter().copied().collect::<Vec<_>>())
                .unwrap_or_default()
        );
        Ok(())
    }

    /// Revoke all forwarding resources owned for one client.
    pub async fn remove_client(&mut self, client_id: &str) -> Result<()> {
        let Some(mut state) = self.clients.remove(client_id) else {
            return Ok(());
        };
        rollback_client_state_for_tun(self.runner.as_ref(), &self.tun_name, &mut state).await;
        tracing::info!("Exit Node 已撤销客户端 {} 的转发资源", client_id);
        Ok(())
    }

    async fn run_iptables(&self, program: &str, table: Option<&str>, args: &[&str]) -> Result<()> {
        run_iptables(self.runner.as_ref(), program, table, args).await
    }

    async fn install_forwarding(&mut self) -> Result<()> {
        self.ipv4_forward_changed =
            ensure_sysctl(self.runner.as_ref(), "/proc/sys/net/ipv4/ip_forward")?;
        if self.external_v6_interface.is_some() {
            self.ipv6_forward_changed = ensure_sysctl(
                self.runner.as_ref(),
                "/proc/sys/net/ipv6/conf/all/forwarding",
            )?;
        }

        self.purge_stale_chains().await?;
        self.purge_stale_return_routes().await?;
        self.run_iptables(
            "iptables",
            Some("filter"),
            &["-N", self.forward_chain.as_str()],
        )
        .await?;
        self.run_iptables("iptables", Some("nat"), &["-N", self.nat_chain.as_str()])
            .await?;
        self.install_terminal_drops("iptables").await?;
        self.run_iptables(
            "iptables",
            Some("filter"),
            &["-I", "FORWARD", "1", "-j", self.forward_chain.as_str()],
        )
        .await?;
        self.run_iptables(
            "iptables",
            Some("nat"),
            &["-I", "POSTROUTING", "1", "-j", self.nat_chain.as_str()],
        )
        .await?;

        if let Some(interface) = self.external_v6_interface.as_deref() {
            self.run_iptables(
                "ip6tables",
                Some("filter"),
                &["-N", self.forward_chain.as_str()],
            )
            .await?;
            self.run_iptables("ip6tables", Some("nat"), &["-N", self.nat_chain.as_str()])
                .await?;
            self.install_terminal_drops("ip6tables").await?;
            self.run_iptables(
                "ip6tables",
                Some("filter"),
                &["-I", "FORWARD", "1", "-j", self.forward_chain.as_str()],
            )
            .await?;
            self.run_iptables(
                "ip6tables",
                Some("nat"),
                &["-I", "POSTROUTING", "1", "-j", self.nat_chain.as_str()],
            )
            .await?;
            tracing::info!("Exit Node IPv6 出口网卡: {} (NAT66 已启用)", interface);
        } else {
            tracing::info!("Exit Node 未发现 IPv6 默认出口，公网 IPv6 转发不可用");
        }

        Ok(())
    }

    async fn install_terminal_drops(&self, program: &'static str) -> Result<()> {
        for args in terminal_forward_drop_rules(&self.forward_chain, &self.tun_name) {
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            self.run_iptables(program, Some("filter"), &refs).await?;
        }
        Ok(())
    }

    async fn add_client_rules(
        &self,
        program: &'static str,
        source: &str,
        rules: &mut Vec<IptablesRule>,
    ) -> Result<()> {
        self.add_client_rules_for_interface(program, &self.external_v4_interface, source, rules)
            .await
    }

    async fn add_client_rules_for_interface(
        &self,
        program: &'static str,
        external_interface: &str,
        source: &str,
        rules: &mut Vec<IptablesRule>,
    ) -> Result<()> {
        // The terminal DROP rules stay at the bottom of the chain. Insert each
        // authenticated client exception above them so rules added while the
        // Exit Node is live remain reachable.
        for args in client_forward_rules(&self.forward_chain, &self.tun_name, source) {
            install_iptables_rule(self.runner.as_ref(), program, Some("filter"), args, rules)
                .await?;
        }
        let nat_rule = nat_masquerade_rule(&self.nat_chain, source, external_interface);
        install_iptables_rule(self.runner.as_ref(), program, Some("nat"), nat_rule, rules).await?;
        Ok(())
    }

    async fn purge_stale_return_routes(&self) -> Result<()> {
        let mut families = vec![false];
        if self.external_v6_interface.is_some() {
            families.push(true);
        }
        for ipv6 in families {
            let mut args = vec!["route".to_string()];
            if ipv6 {
                args.insert(0, "-6".to_string());
            }
            args.extend(
                [
                    "flush",
                    "table",
                    "main",
                    "proto",
                    ROUTE_PROTOCOL,
                    "dev",
                    &self.tun_name,
                ]
                .into_iter()
                .map(str::to_string),
            );
            let output = self
                .runner
                .run(CommandInvocation {
                    program: "ip".to_string(),
                    args,
                })
                .await
                .context("清理 Exit Node 残留回程路由时无法执行 ip 命令")?;
            if !output.success {
                anyhow::bail!(
                    "清理 Exit Node 残留回程路由失败: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
        }
        Ok(())
    }

    async fn purge_stale_chains(&mut self) -> Result<()> {
        for (program, table, builtin, chain) in [
            ("iptables", "filter", "FORWARD", self.forward_chain.as_str()),
            ("iptables", "nat", "POSTROUTING", self.nat_chain.as_str()),
        ] {
            purge_iptables_chain(self.runner.as_ref(), program, table, builtin, chain).await?;
        }
        if self.external_v6_interface.is_some() {
            for (program, table, builtin, chain) in [
                (
                    "ip6tables",
                    "filter",
                    "FORWARD",
                    self.forward_chain.as_str(),
                ),
                ("ip6tables", "nat", "POSTROUTING", self.nat_chain.as_str()),
            ] {
                purge_iptables_chain(self.runner.as_ref(), program, table, builtin, chain).await?;
            }
        }
        Ok(())
    }

    /// Remove every resource owned by this manager. Safe to call twice.
    pub fn rollback(&mut self) {
        if self.cleaned {
            return;
        }
        self.cleaned = true;

        for state in self.clients.values_mut() {
            for rule in state.rules.iter().rev() {
                let _ = run_iptables_sync_delete(self.runner.as_ref(), rule);
            }
            for address in state.owned_routes.iter().rev() {
                let _ = remove_return_route_sync(self.runner.as_ref(), &self.tun_name, *address);
            }
        }
        self.clients.clear();

        for (program, table, builtin, chain) in [
            ("iptables", "filter", "FORWARD", self.forward_chain.as_str()),
            ("iptables", "nat", "POSTROUTING", self.nat_chain.as_str()),
        ] {
            cleanup_iptables_chain_sync(self.runner.as_ref(), program, table, builtin, chain);
        }
        if self.external_v6_interface.is_some() {
            for (program, table, builtin, chain) in [
                (
                    "ip6tables",
                    "filter",
                    "FORWARD",
                    self.forward_chain.as_str(),
                ),
                ("ip6tables", "nat", "POSTROUTING", self.nat_chain.as_str()),
            ] {
                cleanup_iptables_chain_sync(self.runner.as_ref(), program, table, builtin, chain);
            }
        }

        if self.ipv4_forward_changed {
            let _ = restore_sysctl(self.runner.as_ref(), "/proc/sys/net/ipv4/ip_forward");
        }
        if self.ipv6_forward_changed {
            let _ = restore_sysctl(
                self.runner.as_ref(),
                "/proc/sys/net/ipv6/conf/all/forwarding",
            );
        }
    }
}

impl Drop for LinuxExitForwarder {
    fn drop(&mut self) {
        self.rollback();
    }
}

async fn install_iptables_rule(
    runner: &dyn ExitCommandRunner,
    program: &'static str,
    table: Option<&'static str>,
    args: Vec<String>,
    rules: &mut Vec<IptablesRule>,
) -> Result<()> {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_iptables(runner, program, table, &refs).await?;
    rules.push(IptablesRule {
        program,
        table,
        args,
    });
    Ok(())
}

async fn rollback_client_state_for_tun(
    runner: &dyn ExitCommandRunner,
    tun_name: &str,
    state: &mut ClientForwardState,
) {
    for rule in state.rules.iter().rev() {
        if let Err(error) = run_iptables_delete(runner, rule).await {
            tracing::warn!("删除 Exit Node iptables 规则失败: {error:#}");
        }
    }
    state.rules.clear();

    for address in state.owned_routes.iter().rev() {
        if let Err(error) = remove_return_route(runner, tun_name, *address).await {
            tracing::warn!("删除 Exit Node 回程路由失败: {error:#}");
        }
    }
    state.owned_routes.clear();
}

async fn default_route_interface(runner: &dyn ExitCommandRunner, ipv6: bool) -> Result<String> {
    let mut args = vec!["route".to_string()];
    if ipv6 {
        args.insert(0, "-6".to_string());
    }
    args.extend(["show", "default"].into_iter().map(str::to_string));
    let output = runner
        .run(CommandInvocation {
            program: "ip".to_string(),
            args,
        })
        .await
        .context("无法执行 ip route show default")?;
    if !output.success {
        anyhow::bail!(
            "读取默认路由失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    parse_default_route_interface(&String::from_utf8_lossy(&output.stdout))
        .context("默认路由中没有可用的 dev 网卡")
}

fn parse_default_route_interface(output: &str) -> Option<String> {
    for line in output.lines() {
        let mut fields = line.split_whitespace();
        if fields.next() != Some("default") {
            continue;
        }
        let fields: Vec<_> = fields.collect();
        if let Some(index) = fields.iter().position(|field| *field == "dev") {
            if let Some(interface) = fields.get(index + 1) {
                return Some((*interface).to_string());
            }
        }
    }
    None
}

fn client_forward_rules(forward_chain: &str, tun_name: &str, source: &str) -> [Vec<String>; 2] {
    [
        vec![
            "-I".to_string(),
            forward_chain.to_string(),
            "1".to_string(),
            "-i".to_string(),
            tun_name.to_string(),
            "-s".to_string(),
            source.to_string(),
            "-m".to_string(),
            "conntrack".to_string(),
            "--ctstate".to_string(),
            "NEW,ESTABLISHED,RELATED".to_string(),
            "-j".to_string(),
            "ACCEPT".to_string(),
        ],
        vec![
            "-I".to_string(),
            forward_chain.to_string(),
            "1".to_string(),
            "-o".to_string(),
            tun_name.to_string(),
            "-d".to_string(),
            source.to_string(),
            "-m".to_string(),
            "conntrack".to_string(),
            "--ctstate".to_string(),
            "ESTABLISHED,RELATED".to_string(),
            "-j".to_string(),
            "ACCEPT".to_string(),
        ],
    ]
}

fn terminal_forward_drop_rules(forward_chain: &str, tun_name: &str) -> [Vec<String>; 2] {
    [
        vec![
            "-A".to_string(),
            forward_chain.to_string(),
            "-i".to_string(),
            tun_name.to_string(),
            "-j".to_string(),
            "DROP".to_string(),
        ],
        vec![
            "-A".to_string(),
            forward_chain.to_string(),
            "-o".to_string(),
            tun_name.to_string(),
            "-j".to_string(),
            "DROP".to_string(),
        ],
    ]
}

fn nat_masquerade_rule(nat_chain: &str, source: &str, external_interface: &str) -> Vec<String> {
    vec![
        "-A".to_string(),
        nat_chain.to_string(),
        "-s".to_string(),
        source.to_string(),
        "-o".to_string(),
        external_interface.to_string(),
        "-j".to_string(),
        "MASQUERADE".to_string(),
    ]
}

fn chain_name(kind: &str, seed: &str) -> String {
    let mut hash: u32 = 2_166_136_261;
    for byte in seed.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    format!("FKV6{kind}-{hash:08X}")
}

fn host_prefix(address: IpAddr) -> Result<String> {
    match address {
        IpAddr::V4(address) => Ok(format!("{address}/32")),
        IpAddr::V6(address) => Ok(format!("{address}/128")),
    }
}

fn ip_invocation(ipv6: bool, args: &[&str]) -> CommandInvocation {
    let mut command_args = Vec::with_capacity(args.len() + usize::from(ipv6));
    if ipv6 {
        command_args.push("-6".to_string());
    }
    command_args.extend(args.iter().map(|arg| (*arg).to_string()));
    CommandInvocation {
        program: "ip".to_string(),
        args: command_args,
    }
}

async fn ensure_return_route(
    runner: &dyn ExitCommandRunner,
    tun_name: &str,
    address: IpAddr,
) -> Result<bool> {
    let prefix = host_prefix(address)?;
    let output = runner
        .run(ip_invocation(
            address.is_ipv6(),
            &["route", "show", "exact", &prefix],
        ))
        .await
        .with_context(|| format!("检查 Exit Node 回程路由失败: {prefix}"))?;
    if !output.success {
        anyhow::bail!(
            "检查 Exit Node 回程路由失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let existing = String::from_utf8_lossy(&output.stdout);
    if !existing.trim().is_empty() {
        let expected = format!("dev {tun_name}");
        if existing.contains(&expected) {
            return Ok(false);
        }
        anyhow::bail!("回程地址 {prefix} 已被其他网卡占用: {}", existing.trim());
    }

    let output = runner
        .run(ip_invocation(
            address.is_ipv6(),
            &[
                "route",
                "add",
                &prefix,
                "dev",
                tun_name,
                "proto",
                ROUTE_PROTOCOL,
            ],
        ))
        .await
        .with_context(|| format!("添加 Exit Node 回程路由失败: {prefix}"))?;
    if !output.success {
        anyhow::bail!(
            "添加 Exit Node 回程路由失败 ({}): {}",
            prefix,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(true)
}

async fn remove_return_route(
    runner: &dyn ExitCommandRunner,
    tun_name: &str,
    address: IpAddr,
) -> Result<()> {
    let prefix = host_prefix(address)?;
    let output = runner
        .run(ip_invocation(
            address.is_ipv6(),
            &[
                "route",
                "del",
                &prefix,
                "dev",
                tun_name,
                "proto",
                ROUTE_PROTOCOL,
            ],
        ))
        .await
        .with_context(|| format!("删除 Exit Node 回程路由失败: {prefix}"))?;
    if !output.success {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("No such process")
            || stderr.contains("Cannot find device")
            || stderr.contains("No such file or directory")
        {
            return Ok(());
        }
        anyhow::bail!(
            "删除 Exit Node 回程路由失败 ({}): {}",
            prefix,
            stderr.trim()
        );
    }
    Ok(())
}

fn remove_return_route_sync(
    runner: &dyn ExitCommandRunner,
    tun_name: &str,
    address: IpAddr,
) -> Result<()> {
    let prefix = host_prefix(address)?;
    let output = runner
        .run_sync(ip_invocation(
            address.is_ipv6(),
            &[
                "route",
                "del",
                &prefix,
                "dev",
                tun_name,
                "proto",
                ROUTE_PROTOCOL,
            ],
        ))
        .with_context(|| format!("删除 Exit Node 回程路由失败: {prefix}"))?;
    if !output.success {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("No such process")
            || stderr.contains("Cannot find device")
            || stderr.contains("No such file or directory")
        {
            return Ok(());
        }
        anyhow::bail!(
            "删除 Exit Node 回程路由失败 ({}): {}",
            prefix,
            stderr.trim()
        );
    }
    Ok(())
}

async fn purge_iptables_chain(
    runner: &dyn ExitCommandRunner,
    program: &str,
    table: &str,
    builtin: &str,
    chain: &str,
) -> Result<()> {
    for _ in 0..MAX_STALE_JUMP_RULES {
        if run_iptables(runner, program, Some(table), &["-D", builtin, "-j", chain])
            .await
            .is_err()
        {
            break;
        }
    }
    let _ = run_iptables(runner, program, Some(table), &["-F", chain]).await;
    let _ = run_iptables(runner, program, Some(table), &["-X", chain]).await;
    Ok(())
}

fn cleanup_iptables_chain_sync(
    runner: &dyn ExitCommandRunner,
    program: &str,
    table: &str,
    builtin: &str,
    chain: &str,
) {
    for _ in 0..MAX_STALE_JUMP_RULES {
        if run_iptables_sync(runner, program, Some(table), &["-D", builtin, "-j", chain]).is_err() {
            break;
        }
    }
    let _ = run_iptables_sync(runner, program, Some(table), &["-F", chain]);
    let _ = run_iptables_sync(runner, program, Some(table), &["-X", chain]);
}

async fn run_iptables(
    runner: &dyn ExitCommandRunner,
    program: &str,
    table: Option<&str>,
    args: &[&str],
) -> Result<()> {
    let invocation = iptables_invocation(program, table, args);
    let output = runner.run(invocation).await?;
    if output.success {
        return Ok(());
    }
    anyhow::bail!(
        "{program} {} 失败: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    )
}

fn iptables_invocation(program: &str, table: Option<&str>, args: &[&str]) -> CommandInvocation {
    let mut command_args = vec!["-w".to_string(), IPTABLES_WAIT_SECONDS.to_string()];
    if let Some(table) = table {
        command_args.push("-t".to_string());
        command_args.push(table.to_string());
    }
    command_args.extend(args.iter().map(|arg| (*arg).to_string()));
    CommandInvocation {
        program: program.to_string(),
        args: command_args,
    }
}

fn iptables_delete_args(rule: &IptablesRule) -> Vec<String> {
    let mut args = rule.args.clone();
    match args.first().map(String::as_str) {
        Some("-A") => args[0] = "-D".to_string(),
        Some("-I") if args.len() >= 3 => {
            args[0] = "-D".to_string();
            // -I CHAIN POSITION MATCHES... -> -D CHAIN MATCHES...
            args.remove(2);
        }
        _ => {}
    }
    args
}

async fn run_iptables_delete(runner: &dyn ExitCommandRunner, rule: &IptablesRule) -> Result<()> {
    let args = iptables_delete_args(rule);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_iptables(runner, rule.program, rule.table, &refs).await
}

fn run_iptables_sync(
    runner: &dyn ExitCommandRunner,
    program: &str,
    table: Option<&str>,
    args: &[&str],
) -> Result<()> {
    let output = runner.run_sync(iptables_invocation(program, table, args))?;
    if output.success {
        return Ok(());
    }
    anyhow::bail!(
        "{program} {} 失败: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    )
}

fn run_iptables_sync_delete(runner: &dyn ExitCommandRunner, rule: &IptablesRule) -> Result<()> {
    let args = iptables_delete_args(rule);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_iptables_sync(runner, rule.program, rule.table, &refs)
}

fn ensure_sysctl(runner: &dyn ExitCommandRunner, path: &str) -> Result<bool> {
    let previous = runner
        .read_file(path)
        .with_context(|| format!("读取内核参数失败: {path}"))?
        .trim()
        .to_string();
    if previous == "1" {
        return Ok(false);
    }
    runner
        .write_file(path, b"1\n")
        .with_context(|| format!("开启内核转发失败: {path}"))?;
    Ok(true)
}

fn restore_sysctl(runner: &dyn ExitCommandRunner, path: &str) -> Result<()> {
    runner
        .write_file(path, b"0\n")
        .with_context(|| format!("恢复内核转发参数失败: {path}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeCommandRunner {
        async_runs: Mutex<Vec<CommandInvocation>>,
        sync_runs: Mutex<Vec<CommandInvocation>>,
        files: Mutex<HashMap<String, String>>,
        async_failures: Mutex<HashSet<CommandInvocation>>,
        sync_failures: Mutex<HashSet<CommandInvocation>>,
    }

    impl FakeCommandRunner {
        fn with_sysctl(value: &str) -> Self {
            let runner = Self::default();
            for path in [
                "/proc/sys/net/ipv4/ip_forward",
                "/proc/sys/net/ipv6/conf/all/forwarding",
            ] {
                runner
                    .files
                    .lock()
                    .expect("fake filesystem lock")
                    .insert(path.to_string(), value.to_string());
            }
            runner
        }

        fn fail_async(&self, invocation: CommandInvocation) {
            self.async_failures
                .lock()
                .expect("async failures lock")
                .insert(invocation);
        }

        fn async_runs(&self) -> Vec<CommandInvocation> {
            self.async_runs.lock().expect("async runs lock").clone()
        }

        fn sync_runs(&self) -> Vec<CommandInvocation> {
            self.sync_runs.lock().expect("sync runs lock").clone()
        }

        fn file(&self, path: &str) -> Option<String> {
            self.files
                .lock()
                .expect("fake filesystem lock")
                .get(path)
                .cloned()
        }
    }

    impl ExitCommandRunner for FakeCommandRunner {
        fn run(&self, invocation: CommandInvocation) -> CommandFuture<'_> {
            Box::pin(async move {
                self.async_runs
                    .lock()
                    .expect("async runs lock")
                    .push(invocation.clone());
                let failed = self
                    .async_failures
                    .lock()
                    .expect("async failures lock")
                    .remove(&invocation);
                Ok(CommandResult {
                    success: !failed,
                    stdout: Vec::new(),
                    stderr: if failed {
                        b"injected command failure".to_vec()
                    } else {
                        Vec::new()
                    },
                })
            })
        }

        fn run_sync(&self, invocation: CommandInvocation) -> Result<CommandResult> {
            self.sync_runs
                .lock()
                .expect("sync runs lock")
                .push(invocation.clone());
            let failed = self
                .sync_failures
                .lock()
                .expect("sync failures lock")
                .remove(&invocation);
            Ok(CommandResult {
                success: !failed,
                stdout: Vec::new(),
                stderr: if failed {
                    b"injected command failure".to_vec()
                } else {
                    Vec::new()
                },
            })
        }

        fn read_file(&self, path: &str) -> Result<String> {
            self.files
                .lock()
                .expect("fake filesystem lock")
                .get(path)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("missing fake file: {path}"))
        }

        fn write_file(&self, path: &str, contents: &[u8]) -> Result<()> {
            self.files.lock().expect("fake filesystem lock").insert(
                path.to_string(),
                String::from_utf8(contents.to_vec()).context("fake UTF-8 file contents")?,
            );
            Ok(())
        }
    }

    fn fake_forwarder(
        runner: Arc<FakeCommandRunner>,
        external_v6_interface: Option<&str>,
    ) -> LinuxExitForwarder {
        let tun_name = "fkexit".to_string();
        LinuxExitForwarder {
            forward_chain: chain_name("F", &tun_name),
            nat_chain: chain_name("N", &tun_name),
            tun_name,
            external_v4_interface: "eth0".to_string(),
            external_v6_interface: external_v6_interface.map(str::to_string),
            clients: HashMap::new(),
            ipv4_forward_changed: false,
            ipv6_forward_changed: false,
            cleaned: false,
            runner,
        }
    }

    #[test]
    fn default_route_parser_finds_interface() {
        let output = "default via 185.217.110.1 dev eth0 proto static metric 100\n";
        assert_eq!(
            parse_default_route_interface(output),
            Some("eth0".to_string())
        );
        assert_eq!(parse_default_route_interface("10.0.0.0/8 dev eth0\n"), None);
    }

    #[test]
    fn chain_names_are_stable_and_short() {
        let first = chain_name("F", "fkexit");
        let second = chain_name("F", "fkexit");
        assert_eq!(first, second);
        assert_ne!(first, chain_name("F", "fkexit-2"));
        assert!(first.starts_with("FKV6F-"));
        assert!(first.len() <= 28);
    }

    #[test]
    fn client_rules_precede_terminal_tun_drops() {
        let accepted = client_forward_rules("FKV6F-TEST", "fuckipv6", "10.8.0.2/32");
        assert_eq!(accepted[0][0], "-I");
        assert_eq!(accepted[0][2], "1");
        assert!(accepted[0].windows(2).any(|pair| pair == ["-j", "ACCEPT"]));

        let dropped = terminal_forward_drop_rules("FKV6F-TEST", "fuckipv6");
        assert_eq!(dropped[0][0], "-A");
        assert_eq!(dropped[0][2], "-i");
        assert_eq!(dropped[1][2], "-o");
        assert!(dropped
            .iter()
            .all(|rule| rule.ends_with(&["DROP".to_string()])));
    }

    #[test]
    fn inserted_client_rules_are_deleted_without_rule_position() {
        let rule = IptablesRule {
            program: "iptables",
            table: Some("filter"),
            args: client_forward_rules("FKV6F-TEST", "fuckipv6", "10.8.0.2/32")[0].clone(),
        };
        let args = iptables_delete_args(&rule);
        assert_eq!(&args[..3], ["-D", "FKV6F-TEST", "-i"]);
        assert!(!args.contains(&"1".to_string()));
    }

    #[test]
    fn host_routes_have_family_specific_prefixes() {
        assert_eq!(
            host_prefix("10.8.0.2".parse().unwrap()).unwrap(),
            "10.8.0.2/32"
        );
        assert_eq!(
            host_prefix("fd00::1002".parse().unwrap()).unwrap(),
            "fd00::1002/128"
        );
    }

    #[tokio::test]
    async fn forwarding_nat_and_cleanup_install_the_owned_resources() {
        let runner = Arc::new(FakeCommandRunner::with_sysctl("0\n"));
        let mut forwarder = fake_forwarder(runner.clone(), None);

        forwarder
            .install_resources()
            .await
            .expect("fake host must accept forwarding installation");
        assert_eq!(
            runner.file("/proc/sys/net/ipv4/ip_forward").as_deref(),
            Some("1\n")
        );

        let virtual_ips = VirtualIps::from_ipv4("10.8.0.2".parse().expect("valid IPv4"));
        forwarder
            .allow_client("peer-1", &virtual_ips)
            .await
            .expect("fake host must accept client authorization");

        let async_runs = runner.async_runs();
        for expected in [
            iptables_invocation(
                "iptables",
                Some("filter"),
                &["-N", &forwarder.forward_chain],
            ),
            iptables_invocation("iptables", Some("nat"), &["-N", &forwarder.nat_chain]),
            iptables_invocation(
                "iptables",
                Some("filter"),
                &["-I", "FORWARD", "1", "-j", &forwarder.forward_chain],
            ),
            iptables_invocation(
                "iptables",
                Some("nat"),
                &["-I", "POSTROUTING", "1", "-j", &forwarder.nat_chain],
            ),
            iptables_invocation(
                "iptables",
                Some("nat"),
                &[
                    "-A",
                    &forwarder.nat_chain,
                    "-s",
                    "10.8.0.2/32",
                    "-o",
                    "eth0",
                    "-j",
                    "MASQUERADE",
                ],
            ),
        ] {
            assert!(
                async_runs.contains(&expected),
                "missing installed command: {expected:?}"
            );
        }

        forwarder.rollback();
        assert_eq!(
            runner.file("/proc/sys/net/ipv4/ip_forward").as_deref(),
            Some("0\n")
        );
        let sync_runs = runner.sync_runs();
        let first_client_rule = IptablesRule {
            program: "iptables",
            table: Some("filter"),
            args: client_forward_rules(
                &forwarder.forward_chain,
                &forwarder.tun_name,
                "10.8.0.2/32",
            )[0]
            .clone(),
        };
        let delete_args = iptables_delete_args(&first_client_rule);
        let delete_refs = delete_args.iter().map(String::as_str).collect::<Vec<_>>();
        assert!(sync_runs.contains(&iptables_invocation(
            "iptables",
            Some("filter"),
            &delete_refs,
        )));
        assert!(sync_runs.contains(&ip_invocation(
            false,
            &[
                "route",
                "del",
                "10.8.0.2/32",
                "dev",
                "fkexit",
                "proto",
                "99",
            ],
        )));
        assert!(sync_runs.contains(&iptables_invocation(
            "iptables",
            Some("filter"),
            &["-X", &forwarder.forward_chain],
        )));
        assert!(sync_runs.contains(&iptables_invocation(
            "iptables",
            Some("nat"),
            &["-X", &forwarder.nat_chain],
        )));
    }

    #[tokio::test]
    async fn forwarding_install_failure_rolls_back_chains_and_sysctl_once() {
        let runner = Arc::new(FakeCommandRunner::with_sysctl("0\n"));
        let mut forwarder = fake_forwarder(runner.clone(), None);
        runner.fail_async(iptables_invocation(
            "iptables",
            Some("nat"),
            &["-N", &forwarder.nat_chain],
        ));

        let error = forwarder
            .install_resources()
            .await
            .expect_err("injected NAT chain creation failure must abort installation");
        assert!(format!("{error:#}").contains("injected command failure"));

        let sync_runs = runner.sync_runs();
        for expected in [
            iptables_invocation(
                "iptables",
                Some("filter"),
                &["-D", "FORWARD", "-j", &forwarder.forward_chain],
            ),
            iptables_invocation(
                "iptables",
                Some("filter"),
                &["-F", &forwarder.forward_chain],
            ),
            iptables_invocation(
                "iptables",
                Some("filter"),
                &["-X", &forwarder.forward_chain],
            ),
            iptables_invocation(
                "iptables",
                Some("nat"),
                &["-D", "POSTROUTING", "-j", &forwarder.nat_chain],
            ),
            iptables_invocation("iptables", Some("nat"), &["-F", &forwarder.nat_chain]),
            iptables_invocation("iptables", Some("nat"), &["-X", &forwarder.nat_chain]),
        ] {
            assert!(
                sync_runs.contains(&expected),
                "missing rollback command: {expected:?}"
            );
        }
        assert_eq!(
            runner.file("/proc/sys/net/ipv4/ip_forward").as_deref(),
            Some("0\n")
        );

        let cleanup_count = sync_runs.len();
        forwarder.rollback();
        assert_eq!(runner.sync_runs().len(), cleanup_count);
    }

    #[tokio::test]
    async fn client_rule_failure_rolls_back_prior_rules_and_return_route() {
        let runner = Arc::new(FakeCommandRunner::default());
        let mut forwarder = fake_forwarder(runner.clone(), None);
        let rules =
            client_forward_rules(&forwarder.forward_chain, &forwarder.tun_name, "10.8.0.2/32");
        let second_rule_args = rules[1].iter().map(String::as_str).collect::<Vec<_>>();
        runner.fail_async(iptables_invocation(
            "iptables",
            Some("filter"),
            &second_rule_args,
        ));
        let virtual_ips = VirtualIps::from_ipv4("10.8.0.2".parse().expect("valid IPv4"));

        forwarder
            .allow_client("peer-1", &virtual_ips)
            .await
            .expect_err("injected second client rule failure must abort authorization");

        let first_rule = IptablesRule {
            program: "iptables",
            table: Some("filter"),
            args: rules[0].clone(),
        };
        let delete_args = iptables_delete_args(&first_rule);
        let delete_refs = delete_args.iter().map(String::as_str).collect::<Vec<_>>();
        let expected_delete = iptables_invocation("iptables", Some("filter"), &delete_refs);
        let expected_route_delete = ip_invocation(
            false,
            &[
                "route",
                "del",
                "10.8.0.2/32",
                "dev",
                "fkexit",
                "proto",
                "99",
            ],
        );
        let async_runs = runner.async_runs();
        assert_eq!(
            async_runs.get(async_runs.len().saturating_sub(2)),
            Some(&expected_delete)
        );
        assert_eq!(async_runs.last(), Some(&expected_route_delete));
        assert!(forwarder.clients.is_empty());
    }

    fn require_privileged_netns_topology() {
        assert_eq!(
            std::env::var("FUCKIPV6_EXIT_NETNS_TOPOLOGY").as_deref(),
            Ok("1"),
            "run this ignored test through scripts/test-exit-node-netns.sh"
        );
    }

    fn command_output(program: &str, args: &[&str]) -> std::process::Output {
        std::process::Command::new(program)
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

    fn iptables_succeeds(table: &str, args: &[&str]) -> bool {
        let mut command_args = vec!["-w", "5", "-t", table];
        command_args.extend_from_slice(args);
        command_output("iptables", &command_args).status.success()
    }

    fn ipv4_forwarding() -> String {
        std::fs::read_to_string("/proc/sys/net/ipv4/ip_forward")
            .expect("read IPv4 forwarding sysctl")
            .trim()
            .to_string()
    }

    fn ping_upstream() -> std::process::Output {
        let client_netns = std::env::var("FUCKIPV6_EXIT_NETNS_CLIENT")
            .expect("the privileged netns runner must provide the client namespace name");
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
                "2",
                "198.51.100.10",
            ],
        )
    }

    fn assert_exit_resources_absent(tun_name: &str) {
        let forward_chain = chain_name("F", tun_name);
        let nat_chain = chain_name("N", tun_name);

        assert!(!iptables_succeeds("filter", &["-S", &forward_chain]));
        assert!(!iptables_succeeds("nat", &["-S", &nat_chain]));
        assert!(!iptables_stdout("filter", &["-S", "FORWARD"]).contains(&forward_chain));
        assert!(!iptables_stdout("nat", &["-S", "POSTROUTING"]).contains(&nat_chain));

        let route = command_stdout("ip", &["route", "show", "exact", "10.0.0.2/32"]);
        assert!(!route.contains(&format!("dev {tun_name}")));
        assert_eq!(ipv4_forwarding(), "0");
    }

    #[tokio::test]
    #[ignore = "requires the isolated privileged topology from scripts/test-exit-node-netns.sh"]
    async fn linux_netns_forwarding_nat_revocation_and_cleanup_are_effective() {
        require_privileged_netns_topology();
        assert_eq!(ipv4_forwarding(), "0");

        let config = ExitNodeConfig {
            enabled: true,
            ..ExitNodeConfig::default()
        };
        let mut forwarder = LinuxExitForwarder::install(&config, "fip6tun")
            .await
            .expect("install Exit Node forwarding in the isolated namespace")
            .expect("enabled Exit Node configuration must produce a forwarder");
        assert_eq!(ipv4_forwarding(), "1");
        assert_eq!(forwarder.external_v4_interface, "fip6out");
        assert!(forwarder.supports("198.51.100.10".parse().unwrap()));

        let client = VirtualIps::from_ipv4("10.0.0.2".parse().expect("valid client IPv4"));
        forwarder
            .allow_client("netns-client", &client)
            .await
            .expect("authorize the netns client");

        let filter_rules = iptables_stdout("filter", &["-S", &forwarder.forward_chain]);
        assert_eq!(filter_rules.matches("-j ACCEPT").count(), 2);
        assert!(filter_rules.contains("-i fip6tun"));
        assert!(filter_rules.contains("-o fip6tun"));
        assert!(filter_rules.contains("-s 10.0.0.2/32"));
        assert!(filter_rules.contains("-d 10.0.0.2/32"));

        let nat_rules = iptables_stdout("nat", &["-S", &forwarder.nat_chain]);
        assert!(nat_rules.contains("-s 10.0.0.2/32 -o fip6out -j MASQUERADE"));
        assert!(iptables_stdout("filter", &["-S", "FORWARD"]).contains(&forwarder.forward_chain));
        assert!(iptables_stdout("nat", &["-S", "POSTROUTING"]).contains(&forwarder.nat_chain));
        let return_route = command_stdout("ip", &["route", "show", "exact", "10.0.0.2/32"]);
        let has_owned_route_protocol =
            return_route.contains("proto 99") || return_route.contains("proto openr");
        assert!(
            return_route.contains("10.0.0.2")
                && return_route.contains("dev fip6tun")
                && has_owned_route_protocol,
            "unexpected return route: {return_route:?}"
        );

        let forwarded = ping_upstream();
        assert!(
            forwarded.status.success(),
            "client traffic did not traverse forwarding and NAT: {}{}",
            String::from_utf8_lossy(&forwarded.stdout),
            String::from_utf8_lossy(&forwarded.stderr)
        );

        forwarder
            .remove_client("netns-client")
            .await
            .expect("revoke the netns client");
        assert!(
            !ping_upstream().status.success(),
            "revoked client traffic still traversed the Exit Node"
        );

        forwarder.rollback();
        forwarder.rollback();
        assert_exit_resources_absent("fip6tun");
    }

    #[tokio::test]
    #[ignore = "requires the isolated privileged topology from scripts/test-exit-node-netns.sh"]
    async fn linux_netns_install_failure_rolls_back_kernel_state() {
        require_privileged_netns_topology();
        assert_eq!(ipv4_forwarding(), "0");

        let config = ExitNodeConfig {
            enabled: true,
            ..ExitNodeConfig::default()
        };
        let error = LinuxExitForwarder::install(&config, "fip6tun")
            .await
            .expect_err("the PATH wrapper must fail the POSTROUTING jump installation");
        assert!(
            format!("{error:#}").contains("injected POSTROUTING failure"),
            "unexpected installation error: {error:#}"
        );

        assert_exit_resources_absent("fip6tun");
    }
}
