#![cfg(target_os = "linux")]

use anyhow::{bail, ensure, Context, Result};
use fuckipv6_client::routing::{
    TrafficNetworkCleanupReport, TrafficNetworkCleanupStatus, TrafficNetworkFailClosedReport,
    TrafficNetworkFlowKind, TrafficNetworkFlowReport, TrafficNetworkFlowResult,
    TrafficNetworkReport, TrafficNetworkRollbackReport, TrafficNetworkRollbackResult,
};
use fuckipv6_common::domain::{
    ApplicationIdentity, DomainName, NetworkId, NodeId, TrafficAction, TrafficDefaultConfirmedBy,
    TrafficDefaultPolicy, TrafficFailureKind, TrafficFailureOutcome, TrafficFailurePolicy,
    TrafficFailureRouteContext, TrafficFlow, TrafficMatch, TrafficPolicy, TrafficProtocol,
    TrafficRule, TrafficRuleId,
};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::net::{IpAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output};
use std::time::{Duration, Instant};

const TOPOLOGY_ENV: &str = "FUCKIPV6_TRAFFIC_E2E_TOPOLOGY";
const REPORT_ENV: &str = "FUCKIPV6_TRAFFIC_E2E_REPORT";
const OBSERVATIONS_ENV: &str = "FUCKIPV6_TRAFFIC_E2E_OBSERVATIONS";
const SECOND_UID_ENV: &str = "FUCKIPV6_TRAFFIC_E2E_SECOND_UID";
const UPSTREAM_NS_ENV: &str = "FUCKIPV6_TRAFFIC_E2E_UPSTREAM_NS";
const ROLE_ENV: &str = "FUCKIPV6_TRAFFIC_E2E_ROLE";
const TARGET_ENV: &str = "FUCKIPV6_TRAFFIC_E2E_TARGET";
const TEST_NAME: &str = "traffic_policy_netns_evidence_has_no_source_leak";

const DOMAIN_TARGET: &str = "198.51.100.20";
const APPLICATION_TARGET: &str = "203.0.113.30";
const FAILURE_TARGET: &str = "203.0.113.40";
const BLOCKED_TARGET: &str = "203.0.113.50";
const DOMAIN_SOURCE: &str = "10.10.0.2";
const APPLICATION_SOURCE: &str = "10.20.0.2";
const DIRECT_SOURCE: &str = "10.30.0.2";

#[derive(Debug, Deserialize)]
struct ObservedConnection {
    source_ip: IpAddr,
    destination_ip: IpAddr,
}

#[tokio::test]
#[ignore = "requires the isolated privileged topology from scripts/test-traffic-routing-netns.sh"]
async fn traffic_policy_netns_evidence_has_no_source_leak() -> Result<()> {
    if std::env::var(ROLE_ENV).ok().as_deref() == Some("connect") {
        return run_connector_helper();
    }
    require_isolated_topology();

    let report_path = env_path(REPORT_ENV);
    let observations_path = env_path(OBSERVATIONS_ENV);
    let upstream_namespace = std::env::var(UPSTREAM_NS_ENV)?;
    let second_uid = std::env::var(SECOND_UID_ENV)?
        .parse::<u32>()
        .context("second application UID is invalid")?;
    let executable = std::env::current_exe().context("locate traffic E2E test binary")?;
    let executable_identity = ApplicationIdentity::ExecutablePath(
        executable
            .to_str()
            .context("test binary path is not UTF-8")?
            .to_owned(),
    );

    resolve_target("domain.test", DOMAIN_TARGET)?;
    resolve_target("app.test", APPLICATION_TARGET)?;
    resolve_target("fail.test", FAILURE_TARGET)?;
    resolve_target("blocked.test", BLOCKED_TARGET)?;

    let node_id = NodeId::from_public_key(b"traffic-e2e-node");
    let domain_network = NetworkId::from_public_key(b"traffic-e2e-domain-path");
    let application_network = NetworkId::from_public_key(b"traffic-e2e-application-path");
    let domain_rule = TrafficRuleId::from_public_key(b"traffic-e2e-domain-rule");
    let application_rule = TrafficRuleId::from_public_key(b"traffic-e2e-application-rule");

    let prior_policy = traffic_policy(
        &node_id,
        1,
        TrafficAction::Direct,
        &domain_rule,
        &application_rule,
        &application_network,
        &executable_identity,
    )?;
    let applied_policy = traffic_policy(
        &node_id,
        2,
        TrafficAction::Network(domain_network.clone()),
        &domain_rule,
        &application_rule,
        &application_network,
        &executable_identity,
    )?;
    prior_policy.validate_successor(&applied_policy)?;

    install_policy_routes(second_uid)?;
    ensure_domain_policy_route()?;
    ensure_application_policy_route(second_uid)?;
    install_nft_route()?;

    let domain_flow = TrafficFlow {
        domain: Some(DomainName::parse("domain.test")?),
        destination_ip: DOMAIN_TARGET.parse()?,
        destination_port: 443,
        protocol: TrafficProtocol::Tcp,
        application: ApplicationIdentity::Unknown,
    };
    let application_flow = TrafficFlow {
        domain: Some(DomainName::parse("app.test")?),
        destination_ip: APPLICATION_TARGET.parse()?,
        destination_port: 443,
        protocol: TrafficProtocol::Tcp,
        application: executable_identity.clone(),
    };
    let domain_decision = applied_policy.evaluate(&domain_flow)?;
    ensure_eq_decision(
        &domain_decision.action,
        &TrafficAction::Network(domain_network.clone()),
        "domain flow",
    )?;
    ensure!(
        domain_decision.matched_rule.as_ref() == Some(&domain_rule),
        "domain flow did not match the domain rule"
    );
    let application_decision = applied_policy.evaluate(&application_flow)?;
    ensure_eq_decision(
        &application_decision.action,
        &TrafficAction::Network(application_network.clone()),
        "application flow",
    )?;
    ensure!(
        application_decision.matched_rule.as_ref() == Some(&application_rule),
        "application flow did not match the application rule"
    );

    connect_expect_success("domain.test", false, &executable, second_uid)?;
    let domain_source = wait_for_source(&observations_path, DOMAIN_TARGET, DOMAIN_SOURCE).await?;
    connect_expect_success("app.test", true, &executable, second_uid)?;
    let application_source =
        wait_for_source(&observations_path, APPLICATION_TARGET, APPLICATION_SOURCE).await?;

    // Roll back from the applied domain network policy to the previous direct
    // policy. The same domain must now be observed through the direct source.
    rollback_domain_policy_route()?;
    let rolled_back_decision = prior_policy.evaluate(&domain_flow)?;
    ensure_eq_decision(
        &rolled_back_decision.action,
        &TrafficAction::Direct,
        "rolled-back domain flow",
    )?;
    connect_expect_success("domain.test", false, &executable, second_uid)?;
    let rolled_back_source =
        wait_for_source(&observations_path, DOMAIN_TARGET, DIRECT_SOURCE).await?;
    ensure!(
        domain_source != rolled_back_source,
        "rollback did not change the observed source path"
    );

    let blocked_flow = TrafficFlow {
        domain: Some(DomainName::parse("blocked.test")?),
        destination_ip: BLOCKED_TARGET.parse()?,
        destination_port: 443,
        protocol: TrafficProtocol::Tcp,
        application: ApplicationIdentity::Unknown,
    };
    let blocked_decision = prior_policy.evaluate(&blocked_flow)?;
    ensure_eq_decision(
        &blocked_decision.action,
        &TrafficAction::Reject,
        "default-rejected flow",
    )?;
    connect_expect_failure("blocked.test", false, &executable, second_uid)?;

    let failure_flow = TrafficFlow {
        domain: Some(DomainName::parse("fail.test")?),
        destination_ip: FAILURE_TARGET.parse()?,
        destination_port: 443,
        protocol: TrafficProtocol::Tcp,
        application: executable_identity.clone(),
    };
    let failure_decision = prior_policy.evaluate(&failure_flow)?;
    let failure_route = TrafficFailureRouteContext::new(failure_decision.action.clone());
    let failure_outcome =
        prior_policy.decide_failure(&failure_route, TrafficFailureKind::ConnectionTimeout)?;
    ensure!(
        failure_outcome.outcome == TrafficFailureOutcome::FailClosed,
        "application path failure did not fail closed"
    );
    fail_application_path()?;
    connect_expect_failure("fail.test", true, &executable, second_uid)?;

    // Both blocked attempts have terminated before this point. If either one
    // fell through to the real main-table path, the upstream observer would
    // have recorded the direct source and the connector would have succeeded.
    tokio::time::sleep(Duration::from_millis(250)).await;
    let observations = read_observations(&observations_path)?;
    let leaked_ips: Vec<IpAddr> = observations
        .iter()
        .filter(|observation| {
            observation.destination_ip == FAILURE_TARGET.parse::<IpAddr>().unwrap()
                || observation.destination_ip == BLOCKED_TARGET.parse::<IpAddr>().unwrap()
        })
        .map(|observation| observation.source_ip)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    ensure!(
        leaked_ips.is_empty(),
        "rejected or failed-closed traffic reached the peer from {leaked_ips:?}"
    );

    let flows = vec![
        TrafficNetworkFlowReport {
            kind: TrafficNetworkFlowKind::Domain,
            rule_id: domain_decision.matched_rule,
            domain: Some("domain.test".into()),
            destination_ip: DOMAIN_TARGET.parse()?,
            destination_port: 443,
            target: domain_decision.action,
            reason: domain_decision.reason,
            result: TrafficNetworkFlowResult::Connected,
            observed_source_ip: Some(domain_source),
        },
        TrafficNetworkFlowReport {
            kind: TrafficNetworkFlowKind::Application,
            rule_id: application_decision.matched_rule,
            domain: Some("app.test".into()),
            destination_ip: APPLICATION_TARGET.parse()?,
            destination_port: 443,
            target: application_decision.action,
            reason: application_decision.reason,
            result: TrafficNetworkFlowResult::Connected,
            observed_source_ip: Some(application_source),
        },
        TrafficNetworkFlowReport {
            kind: TrafficNetworkFlowKind::Domain,
            rule_id: blocked_decision.matched_rule,
            domain: Some("blocked.test".into()),
            destination_ip: BLOCKED_TARGET.parse()?,
            destination_port: 443,
            target: blocked_decision.action,
            reason: blocked_decision.reason,
            result: TrafficNetworkFlowResult::Blocked,
            observed_source_ip: None,
        },
        TrafficNetworkFlowReport {
            kind: TrafficNetworkFlowKind::Application,
            rule_id: failure_decision.matched_rule,
            domain: Some("fail.test".into()),
            destination_ip: FAILURE_TARGET.parse()?,
            destination_port: 443,
            target: failure_decision.action,
            reason: failure_decision.reason,
            result: TrafficNetworkFlowResult::Blocked,
            observed_source_ip: None,
        },
    ];

    let cleanup = cleanup_topology(&upstream_namespace, second_uid)?;

    let report = TrafficNetworkReport {
        schema_version: 1,
        policy_revision: 1,
        flows,
        fail_closed: TrafficNetworkFailClosedReport {
            attempted_connections: 2,
            blocked_connections: 2,
            leaked_connections: 0,
            observed_leaked_source_ips: leaked_ips,
        },
        rollback: TrafficNetworkRollbackReport {
            from_revision: 2,
            target_revision: 1,
            applied_revision: 1,
            before_source_ip: domain_source,
            after_source_ip: rolled_back_source,
            result: TrafficNetworkRollbackResult::Restored,
        },
        cleanup: cleanup.clone(),
    };
    std::fs::write(
        &report_path,
        serde_json::to_vec_pretty(&report).context("encode traffic E2E report")?,
    )
    .with_context(|| format!("write traffic E2E report to {}", report_path.display()))?;
    println!("traffic E2E report: {}", report_path.display());
    ensure!(
        cleanup.status == TrafficNetworkCleanupStatus::Complete,
        "traffic E2E cleanup left residual resources: {cleanup:?}"
    );
    Ok(())
}

fn require_isolated_topology() {
    assert_eq!(
        std::env::var(TOPOLOGY_ENV).as_deref(),
        Ok("1"),
        "run through scripts/test-traffic-routing-netns.sh"
    );
    // SAFETY: geteuid has no preconditions and does not access memory.
    assert_eq!(
        unsafe { libc::geteuid() },
        0,
        "the isolated traffic E2E test must run as namespace root"
    );
}

fn run_connector_helper() -> Result<()> {
    let target = std::env::var(TARGET_ENV).context("connector target is missing")?;
    let result = (|| {
        let addresses = (target.as_str(), 443)
            .to_socket_addrs()
            .with_context(|| format!("resolve connector target {target}"))?;
        let mut last_error = None;
        for address in addresses {
            match std::net::TcpStream::connect_timeout(&address, Duration::from_secs(2)) {
                Ok(_stream) => return Ok(()),
                Err(error) => last_error = Some(error),
            }
        }
        let error = last_error
            .map(anyhow::Error::from)
            .unwrap_or_else(|| anyhow::anyhow!("connector target resolved to no addresses"));
        Err(error.context("connection attempt failed"))
    })();
    if let Err(error) = result {
        eprintln!("TRAFFIC_E2E_CONNECT_FAILED: {error:#}");
        return Err(error);
    }
    println!("TRAFFIC_E2E_CONNECTED");
    Ok(())
}

fn traffic_policy(
    node_id: &NodeId,
    revision: u64,
    domain_action: TrafficAction,
    domain_rule: &TrafficRuleId,
    application_rule: &TrafficRuleId,
    application_network: &NetworkId,
    application: &ApplicationIdentity,
) -> Result<TrafficPolicy> {
    let policy = TrafficPolicy {
        node_id: node_id.clone(),
        revision,
        default_policy: TrafficDefaultPolicy {
            action: TrafficAction::Reject,
            confirmed_by: TrafficDefaultConfirmedBy::Owner,
            confirmed_at_unix_ms: 1,
        },
        failure_policy: TrafficFailurePolicy::fail_closed(),
        rules: vec![
            TrafficRule {
                rule_id: application_rule.clone(),
                priority: 1,
                enabled: true,
                matcher: TrafficMatch {
                    applications: vec![application.clone()],
                    ..TrafficMatch::default()
                },
                action: TrafficAction::Network(application_network.clone()),
                description: "E2E application identity path".into(),
            },
            TrafficRule {
                rule_id: domain_rule.clone(),
                priority: 2,
                enabled: true,
                matcher: TrafficMatch {
                    exact_domains: vec![DomainName::parse("domain.test")?],
                    ..TrafficMatch::default()
                },
                action: domain_action,
                description: "E2E domain path".into(),
            },
        ],
    };
    policy.validate()?;
    Ok(policy)
}

fn ensure_eq_decision(actual: &TrafficAction, expected: &TrafficAction, label: &str) -> Result<()> {
    ensure!(
        actual == expected,
        "{label} selected {actual:?}, expected {expected:?}"
    );
    Ok(())
}

fn resolve_target(name: &str, expected: &str) -> Result<()> {
    let addresses: Vec<IpAddr> = (name, 443)
        .to_socket_addrs()
        .with_context(|| format!("resolve isolated DNS name {name}"))?
        .map(|address| address.ip())
        .collect();
    ensure!(
        addresses
            .iter()
            .any(|address| address.to_string() == expected),
        "DNS name {name} resolved to {addresses:?}, expected {expected}"
    );
    Ok(())
}

fn install_policy_routes(second_uid: u32) -> Result<()> {
    let uid = second_uid.to_string();
    let uid_range = format!("{second_uid}-{second_uid}");
    let domain_route = format!("{DOMAIN_TARGET}/32");
    let _ = command(
        "ip",
        &["rule", "del", "uidrange", &uid_range, "lookup", "110"],
    );
    let _ = command("ip", &["rule", "del", "to", &domain_route, "lookup", "120"]);
    for mark in ["0x10", "0x20", "0x30"] {
        let table = match mark {
            "0x10" => "110",
            "0x20" => "120",
            _ => "130",
        };
        let _ = command("ip", &["rule", "del", "fwmark", mark, "lookup", table]);
    }
    command_ok(
        "ip",
        &[
            "rule", "add", "uidrange", &uid_range, "lookup", "110", "priority", "100",
        ],
    )?;
    command_ok(
        "ip",
        &[
            "rule",
            "add",
            "to",
            &domain_route,
            "lookup",
            "120",
            "priority",
            "110",
        ],
    )?;
    for table in ["110", "120", "130"] {
        let _ = command("ip", &["route", "flush", "table", table]);
    }
    command_ok(
        "ip",
        &[
            "route",
            "replace",
            &format!("{APPLICATION_TARGET}/32"),
            "via",
            "10.20.0.1",
            "dev",
            "fip6app",
            "table",
            "110",
        ],
    )?;
    command_ok(
        "ip",
        &[
            "route",
            "replace",
            &format!("{FAILURE_TARGET}/32"),
            "via",
            "10.20.0.1",
            "dev",
            "fip6app",
            "table",
            "110",
        ],
    )?;
    command_ok(
        "ip",
        &[
            "route",
            "replace",
            &format!("{DOMAIN_TARGET}/32"),
            "via",
            "10.10.0.1",
            "dev",
            "fip6dom",
            "table",
            "120",
        ],
    )?;
    eprintln!("installed UID policy route for application identity uid {uid}");
    Ok(())
}

fn rollback_domain_policy_route() -> Result<()> {
    command_ok(
        "ip",
        &[
            "rule",
            "del",
            "to",
            &format!("{DOMAIN_TARGET}/32"),
            "lookup",
            "120",
        ],
    )
}

fn ensure_domain_policy_route() -> Result<()> {
    ensure_route_output(
        command("ip", &["route", "get", DOMAIN_TARGET])?,
        "fip6dom",
        "domain destination policy route",
    )
}

fn ensure_application_policy_route(second_uid: u32) -> Result<()> {
    let uid = second_uid.to_string();
    ensure_route_output(
        command("ip", &["route", "get", APPLICATION_TARGET, "uid", &uid])?,
        "fip6app",
        "application UID policy route",
    )
}

fn ensure_route_output(output: Output, device: &str, label: &str) -> Result<()> {
    let rendered = String::from_utf8_lossy(&output.stdout);
    ensure!(
        output.status.success() && rendered.contains(&format!("dev {device} ")),
        "{label} did not select {device}: {rendered}"
    );
    Ok(())
}

fn install_nft_route() -> Result<()> {
    let second_uid = std::env::var(SECOND_UID_ENV)?;
    let _ = command("nft", &["delete", "table", "ip", "fip6e2e"]);
    let work = env_path(REPORT_ENV)
        .parent()
        .context("report path has no parent")?
        .to_path_buf();
    let ruleset = format!(
        "table ip fip6e2e {{
           chain output {{
             type filter hook output priority mangle; policy accept;
             udp dport 53 accept
             meta skuid {second_uid} accept
             ip daddr {DOMAIN_TARGET} accept
             reject
           }}
         }}
"
    );
    let path = work.join("nft-policy.rules");
    std::fs::write(&path, ruleset).context("write nftables E2E rules")?;
    command_ok(
        "nft",
        &["-f", path.to_str().context("nft path is not UTF-8")?],
    )?;
    Ok(())
}

fn fail_application_path() -> Result<()> {
    command_ok(
        "ip",
        &[
            "route",
            "replace",
            "blackhole",
            &format!("{FAILURE_TARGET}/32"),
            "table",
            "110",
        ],
    )
}

fn connect_expect_success(
    target: &str,
    second_uid: bool,
    executable: &Path,
    application_uid: u32,
) -> Result<()> {
    let output = run_connector(target, second_uid, executable, application_uid)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    ensure!(
        output.status.success() && stdout.contains("TRAFFIC_E2E_CONNECTED"),
        "expected {target} to connect, status={}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status
    );
    Ok(())
}

fn connect_expect_failure(
    target: &str,
    second_uid: bool,
    executable: &Path,
    application_uid: u32,
) -> Result<()> {
    let output = run_connector(target, second_uid, executable, application_uid)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    ensure!(
        !output.status.success() && stderr.contains("TRAFFIC_E2E_CONNECT_FAILED"),
        "expected {target} to be blocked, status={}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status
    );
    ensure!(
        !stdout.contains("TRAFFIC_E2E_CONNECTED"),
        "blocked target {target} reported a connected path"
    );
    Ok(())
}

fn run_connector(
    target: &str,
    second_uid: bool,
    executable: &Path,
    application_uid: u32,
) -> Result<Output> {
    let mut command = if second_uid {
        let mut command = Command::new("setpriv");
        command.args([
            "--reuid",
            &application_uid.to_string(),
            "--regid",
            &application_uid.to_string(),
            "--clear-groups",
            "--",
        ]);
        command.arg(executable);
        command
    } else {
        Command::new(executable)
    };
    command
        .arg(TEST_NAME)
        .arg("--exact")
        .arg("--ignored")
        .arg("--nocapture")
        .env(ROLE_ENV, "connect")
        .env(TARGET_ENV, target);
    command
        .output()
        .with_context(|| format!("run isolated connector for {target}"))
}

async fn wait_for_source(path: &Path, destination: &str, source: &str) -> Result<IpAddr> {
    let destination: IpAddr = destination.parse()?;
    let source: IpAddr = source.parse()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if read_observations(path)?.iter().any(|observation| {
            observation.destination_ip == destination && observation.source_ip == source
        }) {
            return Ok(source);
        }
        if Instant::now() >= deadline {
            bail!("peer never observed destination {destination} from expected source {source}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn read_observations(path: &Path) -> Result<Vec<ObservedConnection>> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("read peer observations from {}", path.display()))?;
    contents
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).context("decode peer observation"))
        .collect()
}

fn cleanup_topology(
    upstream_namespace: &str,
    second_uid: u32,
) -> Result<TrafficNetworkCleanupReport> {
    let _ = command("nft", &["delete", "table", "ip", "fip6e2e"]);
    let uid_range = format!("{second_uid}-{second_uid}");
    let domain_route = format!("{DOMAIN_TARGET}/32");
    let _ = command(
        "ip",
        &["rule", "del", "uidrange", &uid_range, "lookup", "110"],
    );
    let _ = command("ip", &["rule", "del", "to", &domain_route, "lookup", "120"]);
    for (mark, table) in [("0x10", "110"), ("0x20", "120"), ("0x30", "130")] {
        let _ = command("ip", &["rule", "del", "fwmark", mark, "lookup", table]);
    }
    for table in ["110", "120", "130"] {
        let _ = command("ip", &["route", "flush", "table", table]);
    }

    if let Ok(output) = command("ip", &["netns", "pids", upstream_namespace]) {
        for pid in String::from_utf8_lossy(&output.stdout).split_whitespace() {
            let _ = command("kill", &["-9", pid]);
        }
    }
    let _ = command("ip", &["netns", "delete", upstream_namespace]);
    for link in ["fip6dom", "fip6app", "fip6raw", "fip6dns"] {
        let _ = command("ip", &["link", "delete", link]);
    }

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut residual = u32::MAX;
    while Instant::now() < deadline {
        let nft_present = command_succeeded("nft", &["list", "table", "ip", "fip6e2e"]);
        let rules = command("ip", &["rule", "show"])?;
        let rules_text = String::from_utf8_lossy(&rules.stdout);
        let rules_present = rules_text.contains(&format!("uidrange {uid_range} lookup 110"))
            || rules_text.contains(&format!("to {DOMAIN_TARGET} lookup 120"));
        let namespaces = command("ip", &["netns", "list"])?;
        let namespace_present = String::from_utf8_lossy(&namespaces.stdout)
            .split_whitespace()
            .any(|name| name == upstream_namespace);
        let links_present = ["fip6dom", "fip6app", "fip6raw", "fip6dns"]
            .iter()
            .any(|link| command_succeeded("ip", &["link", "show", "dev", link]));
        residual = u32::from(nft_present)
            + u32::from(rules_present)
            + u32::from(namespace_present)
            + u32::from(links_present);
        if residual == 0 {
            return Ok(TrafficNetworkCleanupReport {
                nft_table_removed: true,
                policy_rule_removed: true,
                namespaces_removed: true,
                residual_resources: 0,
                status: TrafficNetworkCleanupStatus::Complete,
            });
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(TrafficNetworkCleanupReport {
        nft_table_removed: !command_succeeded("nft", &["list", "table", "ip", "fip6e2e"]),
        policy_rule_removed: {
            let rules = command("ip", &["rule", "show"])?;
            let rendered = String::from_utf8_lossy(&rules.stdout);
            !rendered.contains(&format!("uidrange {uid_range} lookup 110"))
                && !rendered.contains(&format!("to {DOMAIN_TARGET} lookup 120"))
        },
        namespaces_removed: !String::from_utf8_lossy(&command("ip", &["netns", "list"])?.stdout)
            .split_whitespace()
            .any(|name| name == upstream_namespace),
        residual_resources: residual,
        status: TrafficNetworkCleanupStatus::Incomplete,
    })
}

fn env_path(name: &str) -> PathBuf {
    PathBuf::from(std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set")))
}

fn command_succeeded(program: &str, arguments: &[&str]) -> bool {
    command(program, arguments)
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn command(program: &str, arguments: &[&str]) -> Result<Output> {
    Command::new(program)
        .args(arguments)
        .output()
        .with_context(|| format!("execute {program} {arguments:?}"))
}

fn command_ok(program: &str, arguments: &[&str]) -> Result<()> {
    let output = command(program, arguments)?;
    ensure!(
        output.status.success(),
        "{program} {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[allow(dead_code)]
fn ensure_status_success(status: ExitStatus, label: &str) -> Result<()> {
    ensure!(status.success(), "{label} failed with {status}");
    Ok(())
}
