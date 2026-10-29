//! Linux forwarding and NAT resources for an Exit Node.
//!
//! Every firewall rule and host route is owned by this process. Rules live in
//! dedicated iptables/ip6tables chains so existing relay policy is not edited,
//! and cleanup only removes resources recorded by this manager.

use anyhow::{Context, Result};
use fuckipv6_common::{ExitNodeConfig, VirtualIps};
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::path::Path;
use tokio::process::Command;

const ROUTE_PROTOCOL: &str = "99";
const IPTABLES_WAIT_SECONDS: &str = "5";

#[derive(Debug, Clone)]
struct IptablesRule {
    program: &'static str,
    table: Option<&'static str>,
    /// Complete rule arguments beginning with `-A`.
    args: Vec<String>,
}

#[derive(Debug)]
struct ClientForwardState {
    addresses: HashSet<IpAddr>,
    owned_routes: Vec<IpAddr>,
    rules: Vec<IptablesRule>,
}

/// Owns forwarding, NAT and return-route resources for one Exit Node process.
#[derive(Debug)]
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
}

impl LinuxExitForwarder {
    /// Install an Exit Node only when `enabled` is set.
    pub async fn install(config: &ExitNodeConfig, tun_name: &str) -> Result<Option<Self>> {
        if !config.enabled {
            return Ok(None);
        }

        let external_v4_interface = default_route_interface(false)
            .await
            .context("无法检测 Exit Node 的 IPv4 默认出口网卡")?;
        let external_v6_interface = default_route_interface(true).await.ok();
        let chain_seed = format!("{tun_name}:{}", std::process::id());
        let forward_chain = chain_name("F", &chain_seed);
        let nat_chain = chain_name("N", &chain_seed);

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
        };

        if let Err(error) = manager.install_forwarding().await {
            manager.rollback();
            return Err(error).context("安装 Exit Node 转发与 NAT 失败，已回滚");
        }

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
                if ensure_return_route(&self.tun_name, address).await? {
                    state.owned_routes.push(address);
                }
            }

            if let Some(address) = virtual_ips.ipv4 {
                let source = host_prefix(IpAddr::V4(address))?;
                self.add_client_rules(
                    "iptables",
                    &self.external_v4_interface,
                    &source,
                    &mut state.rules,
                )
                .await?;
            }

            if let (Some(address), Some(interface)) =
                (virtual_ips.ipv6, self.external_v6_interface.as_deref())
            {
                let source = host_prefix(IpAddr::V6(address))?;
                self.add_client_rules("ip6tables", interface, &source, &mut state.rules)
                    .await?;
            }

            Ok::<(), anyhow::Error>(())
        }
        .await;

        if let Err(error) = result {
            rollback_client_state_for_tun(&self.tun_name, &mut state).await;
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
        rollback_client_state_for_tun(&self.tun_name, &mut state).await;
        tracing::info!("Exit Node 已撤销客户端 {} 的转发资源", client_id);
        Ok(())
    }

    async fn install_forwarding(&mut self) -> Result<()> {
        self.ipv4_forward_changed = ensure_sysctl("/proc/sys/net/ipv4/ip_forward")?;
        if self.external_v6_interface.is_some() {
            self.ipv6_forward_changed = ensure_sysctl("/proc/sys/net/ipv6/conf/all/forwarding")?;
        }

        self.purge_stale_chains().await?;
        run_iptables(
            "iptables",
            Some("filter"),
            &["-N", self.forward_chain.as_str()],
        )
        .await?;
        run_iptables("iptables", Some("nat"), &["-N", self.nat_chain.as_str()]).await?;
        run_iptables(
            "iptables",
            Some("filter"),
            &["-I", "FORWARD", "1", "-j", self.forward_chain.as_str()],
        )
        .await?;
        run_iptables(
            "iptables",
            Some("nat"),
            &["-I", "POSTROUTING", "1", "-j", self.nat_chain.as_str()],
        )
        .await?;

        if let Some(interface) = self.external_v6_interface.as_deref() {
            run_iptables(
                "ip6tables",
                Some("filter"),
                &["-N", self.forward_chain.as_str()],
            )
            .await?;
            run_iptables("ip6tables", Some("nat"), &["-N", self.nat_chain.as_str()]).await?;
            run_iptables(
                "ip6tables",
                Some("filter"),
                &["-I", "FORWARD", "1", "-j", self.forward_chain.as_str()],
            )
            .await?;
            run_iptables(
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

    async fn add_client_rules(
        &self,
        program: &'static str,
        external_interface: &str,
        source: &str,
        rules: &mut Vec<IptablesRule>,
    ) -> Result<()> {
        let forward_rules = [
            vec![
                "-A".to_string(),
                self.forward_chain.clone(),
                "-i".to_string(),
                self.tun_name.clone(),
                "-o".to_string(),
                external_interface.to_string(),
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
                "-A".to_string(),
                self.forward_chain.clone(),
                "-i".to_string(),
                external_interface.to_string(),
                "-o".to_string(),
                self.tun_name.clone(),
                "-d".to_string(),
                source.to_string(),
                "-m".to_string(),
                "conntrack".to_string(),
                "--ctstate".to_string(),
                "ESTABLISHED,RELATED".to_string(),
                "-j".to_string(),
                "ACCEPT".to_string(),
            ],
        ];
        let nat_rule = vec![
            "-A".to_string(),
            self.nat_chain.clone(),
            "-s".to_string(),
            source.to_string(),
            "-o".to_string(),
            external_interface.to_string(),
            "-j".to_string(),
            "MASQUERADE".to_string(),
        ];

        for args in forward_rules {
            append_iptables_rule(program, Some("filter"), args, rules).await?;
        }
        append_iptables_rule(program, Some("nat"), nat_rule, rules).await?;
        Ok(())
    }

    async fn purge_stale_chains(&mut self) -> Result<()> {
        for (program, table, builtin, chain) in [
            ("iptables", "filter", "FORWARD", self.forward_chain.as_str()),
            ("iptables", "nat", "POSTROUTING", self.nat_chain.as_str()),
        ] {
            purge_iptables_chain(program, table, builtin, chain).await?;
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
                purge_iptables_chain(program, table, builtin, chain).await?;
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
                let _ = run_iptables_sync_delete(rule);
            }
            for address in state.owned_routes.iter().rev() {
                let _ = remove_return_route_sync(&self.tun_name, *address);
            }
        }
        self.clients.clear();

        for (program, table, builtin, chain) in [
            ("iptables", "filter", "FORWARD", self.forward_chain.as_str()),
            ("iptables", "nat", "POSTROUTING", self.nat_chain.as_str()),
        ] {
            cleanup_iptables_chain_sync(program, table, builtin, chain);
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
                cleanup_iptables_chain_sync(program, table, builtin, chain);
            }
        }

        if self.ipv4_forward_changed {
            let _ = restore_sysctl("/proc/sys/net/ipv4/ip_forward");
        }
        if self.ipv6_forward_changed {
            let _ = restore_sysctl("/proc/sys/net/ipv6/conf/all/forwarding");
        }
    }
}

impl Drop for LinuxExitForwarder {
    fn drop(&mut self) {
        self.rollback();
    }
}

async fn append_iptables_rule(
    program: &'static str,
    table: Option<&'static str>,
    args: Vec<String>,
    rules: &mut Vec<IptablesRule>,
) -> Result<()> {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_iptables(program, table, &refs).await?;
    rules.push(IptablesRule {
        program,
        table,
        args,
    });
    Ok(())
}

async fn rollback_client_state_for_tun(tun_name: &str, state: &mut ClientForwardState) {
    for rule in state.rules.iter().rev() {
        if let Err(error) = run_iptables_delete(rule).await {
            tracing::warn!("删除 Exit Node iptables 规则失败: {error:#}");
        }
    }
    state.rules.clear();

    for address in state.owned_routes.iter().rev() {
        if let Err(error) = remove_return_route(tun_name, *address).await {
            tracing::warn!("删除 Exit Node 回程路由失败: {error:#}");
        }
    }
    state.owned_routes.clear();
}

async fn default_route_interface(ipv6: bool) -> Result<String> {
    let mut command = Command::new("ip");
    if ipv6 {
        command.arg("-6");
    }
    let output = command
        .args(["route", "show", "default"])
        .output()
        .await
        .context("无法执行 ip route show default")?;
    if !output.status.success() {
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

async fn ensure_return_route(tun_name: &str, address: IpAddr) -> Result<bool> {
    let prefix = host_prefix(address)?;
    let mut show = Command::new("ip");
    if address.is_ipv6() {
        show.arg("-6");
    }
    let output = show
        .args(["route", "show", "exact", &prefix])
        .output()
        .await
        .with_context(|| format!("检查 Exit Node 回程路由失败: {prefix}"))?;
    if !output.status.success() {
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

    let mut add = Command::new("ip");
    if address.is_ipv6() {
        add.arg("-6");
    }
    let output = add
        .args([
            "route",
            "add",
            &prefix,
            "dev",
            tun_name,
            "proto",
            ROUTE_PROTOCOL,
        ])
        .output()
        .await
        .with_context(|| format!("添加 Exit Node 回程路由失败: {prefix}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "添加 Exit Node 回程路由失败 ({}): {}",
            prefix,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(true)
}

async fn remove_return_route(tun_name: &str, address: IpAddr) -> Result<()> {
    let prefix = host_prefix(address)?;
    let mut command = Command::new("ip");
    if address.is_ipv6() {
        command.arg("-6");
    }
    let output = command
        .args([
            "route",
            "del",
            &prefix,
            "dev",
            tun_name,
            "proto",
            ROUTE_PROTOCOL,
        ])
        .output()
        .await
        .with_context(|| format!("删除 Exit Node 回程路由失败: {prefix}"))?;
    if !output.status.success() {
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

fn remove_return_route_sync(tun_name: &str, address: IpAddr) -> Result<()> {
    let prefix = host_prefix(address)?;
    let mut command = std::process::Command::new("ip");
    if address.is_ipv6() {
        command.arg("-6");
    }
    let output = command
        .args([
            "route",
            "del",
            &prefix,
            "dev",
            tun_name,
            "proto",
            ROUTE_PROTOCOL,
        ])
        .output()
        .with_context(|| format!("删除 Exit Node 回程路由失败: {prefix}"))?;
    if !output.status.success() {
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
    program: &str,
    table: &str,
    builtin: &str,
    chain: &str,
) -> Result<()> {
    for args in [
        vec!["-D", builtin, "-j", chain],
        vec!["-F", chain],
        vec!["-X", chain],
    ] {
        let _ = run_iptables(program, Some(table), &args).await;
    }
    Ok(())
}

fn cleanup_iptables_chain_sync(program: &str, table: &str, builtin: &str, chain: &str) {
    for args in [
        vec!["-D", builtin, "-j", chain],
        vec!["-F", chain],
        vec!["-X", chain],
    ] {
        let _ = run_iptables_sync(program, Some(table), &args);
    }
}

async fn run_iptables(program: &str, table: Option<&str>, args: &[&str]) -> Result<()> {
    let mut command = Command::new(program);
    command.args(["-w", IPTABLES_WAIT_SECONDS]);
    if let Some(table) = table {
        command.args(["-t", table]);
    }
    command.args(args);
    let output = command
        .output()
        .await
        .with_context(|| format!("无法执行 {program}"))?;
    if output.status.success() {
        return Ok(());
    }
    anyhow::bail!(
        "{program} {} 失败: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    )
}

async fn run_iptables_delete(rule: &IptablesRule) -> Result<()> {
    let mut args = rule.args.clone();
    if let Some(action) = args.first_mut() {
        if action == "-A" {
            *action = "-D".to_string();
        }
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_iptables(rule.program, rule.table, &refs).await
}

fn run_iptables_sync(program: &str, table: Option<&str>, args: &[&str]) -> Result<()> {
    let mut command = std::process::Command::new(program);
    command.args(["-w", IPTABLES_WAIT_SECONDS]);
    if let Some(table) = table {
        command.args(["-t", table]);
    }
    command.args(args);
    let output = command
        .output()
        .with_context(|| format!("无法执行 {program}"))?;
    if output.status.success() {
        return Ok(());
    }
    anyhow::bail!(
        "{program} {} 失败: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    )
}

fn run_iptables_sync_delete(rule: &IptablesRule) -> Result<()> {
    let mut args = rule.args.clone();
    if let Some(action) = args.first_mut() {
        if action == "-A" {
            *action = "-D".to_string();
        }
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_iptables_sync(rule.program, rule.table, &refs)
}

fn ensure_sysctl(path: &str) -> Result<bool> {
    let previous = std::fs::read_to_string(path)
        .with_context(|| format!("读取内核参数失败: {path}"))?
        .trim()
        .to_string();
    if previous == "1" {
        return Ok(false);
    }
    std::fs::write(Path::new(path), b"1\n").with_context(|| format!("开启内核转发失败: {path}"))?;
    Ok(true)
}

fn restore_sysctl(path: &str) -> Result<()> {
    std::fs::write(Path::new(path), b"0\n").with_context(|| format!("恢复内核转发参数失败: {path}"))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let first = chain_name("F", "fkexit:1234");
        let second = chain_name("F", "fkexit:1234");
        assert_eq!(first, second);
        assert_eq!(first, "FKV6F-9FE3C880");
        assert!(first.len() <= 28);
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
}
