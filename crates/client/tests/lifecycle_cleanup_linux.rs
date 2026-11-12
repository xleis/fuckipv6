#![cfg(target_os = "linux")]

use fuckipv6_client::routing::linux_route::LinuxRouteManager;
use fuckipv6_client::tun::TunDevice;
use fuckipv6_common::{RoutingMode, VirtualIps};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

const TOPOLOGY_ENV: &str = "FUCKIPV6_LIFECYCLE_NETNS_TOPOLOGY";
const RESOLV_CONF_ENV: &str = "FUCKIPV6_LIFECYCLE_RESOLV_CONF";
const RULE_FAULT_MARKER_ENV: &str = "FUCKIPV6_LIFECYCLE_RULE_FAULT_MARKER";
const RULE_FAULT_ARM_ENV: &str = "FUCKIPV6_LIFECYCLE_RULE_FAULT_ARM";
const TUN_FAULT_MARKER_ENV: &str = "FUCKIPV6_LIFECYCLE_TUN_FAULT_MARKER";
const TUN_NAME: &str = "fip6clean";
const BACKUP_PATH: &str = "/run/fuckipv6-resolv.conf.backup";

fn require_isolated_topology() {
    assert_eq!(
        std::env::var(TOPOLOGY_ENV).as_deref(),
        Ok("1"),
        "run through scripts/test-lifecycle-cleanup-netns.sh"
    );
    // SAFETY: geteuid has no preconditions and does not access memory.
    assert_eq!(
        unsafe { libc::geteuid() },
        0,
        "the isolated lifecycle test must run as root"
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

fn ip_stdout(args: &[&str]) -> String {
    let output = ip(args);
    assert!(
        output.status.success(),
        "ip {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn link_exists(name: &str) -> bool {
    ip(&["link", "show", "dev", name]).status.success()
}

fn owns_protocol_99_rule(rules: &str) -> bool {
    rules.lines().any(|line| {
        line.split_whitespace()
            .collect::<Vec<_>>()
            .windows(2)
            .any(|pair| matches!(pair, ["proto" | "protocol" | "prot", "99" | "openr"]))
    })
}

fn has_catch_all_rule(rules: &str) -> bool {
    rules
        .lines()
        .any(|line| line.split_whitespace().next() == Some("32000:"))
}

fn has_exit_probe_rule(rules: &str) -> bool {
    rules.lines().any(|line| {
        let tokens = line.split_whitespace().collect::<Vec<_>>();
        tokens.first() == Some(&"900:")
            && tokens
                .windows(2)
                .any(|pair| pair == ["fwmark", "0x46503601"])
    })
}

fn table_id_for_interface(interface: &str) -> u32 {
    let mut hash = 2_166_136_261u32;
    for byte in interface.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    10_000 + hash % 40_000
}

#[tokio::test]
#[ignore = "requires the isolated privileged topology from scripts/test-lifecycle-cleanup-netns.sh"]
async fn native_cleanup_recovery_restores_dns_routes_and_tun() {
    require_isolated_topology();

    let resolv_conf = env_path(RESOLV_CONF_ENV);
    let rule_fault_marker = env_path(RULE_FAULT_MARKER_ENV);
    let rule_fault_arm = env_path(RULE_FAULT_ARM_ENV);
    let tun_fault_marker = env_path(TUN_FAULT_MARKER_ENV);
    let original_resolver =
        std::fs::read(&resolv_conf).expect("read isolated resolver configuration");

    assert!(!link_exists(TUN_NAME), "test TUN name must start unused");
    assert!(!owns_protocol_99_rule(&ip_stdout(&["-4", "rule", "show"])));

    let assigned_ips = VirtualIps::from_ipv4("10.88.0.2".parse().expect("valid test IPv4"));
    let mut tun = TunDevice::create_dual_stack(TUN_NAME, &assigned_ips, 24, 64, 1280)
        .await
        .expect("create isolated TUN device");
    assert!(link_exists(TUN_NAME));

    let table_id = table_id_for_interface(TUN_NAME).to_string();
    assert!(
        ip(&["-4", "rule", "add", "pref", "32000", "lookup", &table_id, "protocol", "99",])
            .status
            .success()
    );
    assert!(has_catch_all_rule(&ip_stdout(&["-4", "rule", "show"])));
    assert!(ip(&[
        "-4",
        "rule",
        "add",
        "pref",
        "900",
        "fwmark",
        "0x46503601",
        "lookup",
        &table_id,
        "protocol",
        "99",
    ])
    .status
    .success());
    assert!(has_exit_probe_rule(&ip_stdout(&["-4", "rule", "show"])));

    let mut routes = LinuxRouteManager::install(
        RoutingMode::GlobalProxy,
        TUN_NAME,
        &assigned_ips,
        &["10.88.0.0/24".parse().expect("valid virtual CIDR")],
        &[],
        &[],
    )
    .await
    .expect("install real base policy routing");
    assert_eq!(table_id, routes.table_id().to_string());
    assert!(!routes.global_proxy_active());
    assert!(!has_catch_all_rule(&ip_stdout(&["-4", "rule", "show"])));
    assert!(!has_exit_probe_rule(&ip_stdout(&["-4", "rule", "show"])));
    assert_eq!(
        std::fs::read(&resolv_conf).expect("read untouched resolver"),
        original_resolver
    );
    assert!(owns_protocol_99_rule(&ip_stdout(&["-4", "rule", "show"])));
    assert!(ip_stdout(&["-4", "route", "show", "table", &table_id]).contains(TUN_NAME));

    std::fs::write(&rule_fault_arm, b"armed").expect("arm probe rule deletion fault");
    routes
        .validate_exit_data_path()
        .await
        .expect_err("a TUN without a live Exit peer must fail closed");
    assert!(
        rule_fault_marker.exists(),
        "probe rule cleanup fault was not injected"
    );
    std::fs::remove_file(&rule_fault_marker).expect("reset the one-shot rule fault marker");
    let after_failed_probe = ip_stdout(&["-4", "rule", "show"]);
    assert!(!has_exit_probe_rule(&after_failed_probe));
    assert!(!has_catch_all_rule(&after_failed_probe));
    assert!(ip_stdout(&["-4", "route", "show", "table", &table_id]).contains(TUN_NAME));
    assert_eq!(
        std::fs::read(&resolv_conf).expect("read resolver after failed probe"),
        original_resolver,
        "a failed Exit data-path probe must not change host DNS"
    );

    routes
        .activate_global_proxy()
        .await
        .expect("activate real policy routing and DNS bridge");
    assert!(routes.global_proxy_active());
    let managed_resolver = std::fs::read_to_string(&resolv_conf).expect("read managed resolver");
    assert!(managed_resolver.contains("nameserver 127.0.0.2"));
    assert!(has_catch_all_rule(&ip_stdout(&["-4", "rule", "show"])));

    std::fs::write(&rule_fault_arm, b"armed").expect("arm rule deletion fault");
    routes
        .deactivate_global_proxy()
        .await
        .expect("transient catch-all deletion failure must be retried");
    assert!(rule_fault_marker.exists(), "route fault was not injected");
    assert!(!routes.global_proxy_active());
    assert!(!has_catch_all_rule(&ip_stdout(&["-4", "rule", "show"])));
    assert_eq!(
        std::fs::read(&resolv_conf).expect("read restored resolver"),
        original_resolver,
        "one transient route cleanup failure must not leave the host DNS redirected"
    );
    assert!(!Path::new(BACKUP_PATH).exists());
    assert!(owns_protocol_99_rule(&ip_stdout(&["-4", "rule", "show"])));

    routes
        .activate_global_proxy()
        .await
        .expect("reactivate global proxy after cleanup retry");
    std::fs::remove_file(&rule_fault_marker).expect("rearm the one-shot route fault");
    routes
        .rollback()
        .await
        .expect_err("the injected first rule deletion must fail cleanup");
    assert!(rule_fault_marker.exists(), "route fault was not injected");
    let partially_cleaned_rules = ip_stdout(&["-4", "rule", "show"]);
    assert!(
        has_catch_all_rule(&partially_cleaned_rules),
        "failed cleanup must retain the catch-all owner: {partially_cleaned_rules}"
    );
    assert_eq!(
        std::fs::read(&resolv_conf).expect("read restored resolver"),
        original_resolver,
        "DNS must be restored even when one route deletion fails"
    );
    assert!(!Path::new(BACKUP_PATH).exists());
    assert!(
        !ip_stdout(&["-4", "route", "show", "table", &table_id]).contains(TUN_NAME),
        "owned table route must be removed before reporting the retained rule"
    );

    routes
        .rollback()
        .await
        .expect("retained policy rule must be removable on retry");
    routes
        .rollback()
        .await
        .expect("successful route cleanup must be idempotent");
    assert!(!owns_protocol_99_rule(&ip_stdout(&["-4", "rule", "show"])));

    tun.delete()
        .await
        .expect_err("the injected first TUN deletion must fail cleanup");
    assert!(tun_fault_marker.exists(), "TUN fault was not injected");
    assert!(
        link_exists(TUN_NAME),
        "failed TUN cleanup must retain the link owner"
    );

    tokio::time::timeout(Duration::from_secs(5), tun.delete())
        .await
        .expect("TUN cleanup retry must be bounded")
        .expect("retained TUN link must be removable on retry");
    assert!(!link_exists(TUN_NAME));
}
