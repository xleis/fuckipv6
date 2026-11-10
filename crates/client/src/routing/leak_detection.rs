//! Bounded local evidence collection for DNS, IPv4 and IPv6 leak checks.
//!
//! The collector reads host state only to produce sanitized domain evidence.
//! Raw command output, resolver addresses, interface names and stderr never
//! cross this module's public boundary. Controlled and application-behavior
//! probes are injected and remain explicitly `NotTested` when unavailable.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::time::Duration;
use std::time::Instant;

use fuckipv6_common::domain::{
    LeakDetectionDnsMode, LeakDetectionEnvironment, LeakDetectionEvidence,
    LeakDetectionEvidenceState, LeakDetectionIsolationAction, LeakDetectionPlatform,
    LeakDetectionSnapshot, LeakDetectionSource, LeakDetectionTarget,
};
use tokio::process::Command;
use tokio::time::timeout;

pub(crate) const LEAK_DETECTION_COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
pub(crate) const LEAK_DETECTION_PROBE_TIMEOUT: Duration = Duration::from_secs(3);

const LEAK_PROBE_APPLICATION_DNS_URL: &str = "FUCKIPV6_LEAK_PROBE_APPLICATION_DNS_URL";
const LEAK_PROBE_IPV4_CONTROLLED_URL: &str = "FUCKIPV6_LEAK_PROBE_IPV4_CONTROLLED_URL";
const LEAK_PROBE_IPV4_EXPECTED_SOURCE: &str = "FUCKIPV6_LEAK_PROBE_IPV4_EXPECTED_SOURCE";
const LEAK_PROBE_IPV4_EXTERNAL_URL: &str = "FUCKIPV6_LEAK_PROBE_IPV4_EXTERNAL_URL";
const LEAK_PROBE_IPV6_CONTROLLED_URL: &str = "FUCKIPV6_LEAK_PROBE_IPV6_CONTROLLED_URL";
const LEAK_PROBE_IPV6_EXPECTED_SOURCE: &str = "FUCKIPV6_LEAK_PROBE_IPV6_EXPECTED_SOURCE";
const LEAK_PROBE_IPV6_EXTERNAL_URL: &str = "FUCKIPV6_LEAK_PROBE_IPV6_EXTERNAL_URL";
const LEAK_PROBE_RESPONSE_BODY_LIMIT: usize = 256;

type LeakProbeFuture<'a> = Pin<Box<dyn Future<Output = LeakProbeOutcome> + Send + 'a>>;

/// Inputs that must come from authoritative node runtime state rather than UI
/// inference. Interface names are used only for the local comparison and are
/// never serialized into the returned evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LeakDetectionContext {
    pub(crate) protected_path_active: bool,
    pub(crate) protected_interfaces: Vec<String>,
}

impl LeakDetectionContext {
    pub(crate) fn new(protected_path_active: bool, mut protected_interfaces: Vec<String>) -> Self {
        protected_interfaces.retain(|name| valid_interface_name(name));
        protected_interfaces.sort();
        protected_interfaces.dedup();
        Self {
            protected_path_active,
            protected_interfaces,
        }
    }
}

/// Sanitized outcome returned by an injected controlled/application probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LeakProbeOutcome {
    pub(crate) state: LeakDetectionEvidenceState,
    pub(crate) duration_ms: u64,
    pub(crate) error_code: Option<String>,
}

impl LeakProbeOutcome {
    pub(crate) fn not_tested(error_code: impl Into<String>) -> Self {
        Self {
            state: LeakDetectionEvidenceState::NotTested,
            duration_ms: 0,
            error_code: Some(error_code.into()),
        }
    }
}

/// The production probe boundary is deliberately separate from local host
/// inspection. Implementations must not return raw targets, addresses or
/// stderr in `error_code`.
pub(crate) trait LeakDetectionProbeRunner: Send + Sync {
    fn probe<'a>(
        &'a self,
        target: LeakDetectionTarget,
        observed_at_unix_ms: u64,
    ) -> LeakProbeFuture<'a>;
}

#[derive(Debug, Clone)]
enum HttpLeakProbe {
    Reachability {
        url: String,
    },
    ExpectedSource {
        url: String,
        expected_source: IpAddr,
    },
}

/// Deployment-controlled HTTP probes. URLs and expected addresses remain in
/// process configuration and are never copied into public evidence.
#[derive(Debug, Clone, Default)]
pub(crate) struct ConfiguredLeakDetectionProbeRunner {
    probes: BTreeMap<LeakDetectionTarget, HttpLeakProbe>,
}

impl ConfiguredLeakDetectionProbeRunner {
    pub(crate) fn from_env() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    fn from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> Self {
        let mut probes = BTreeMap::new();
        if let Some(url) = non_empty(lookup(LEAK_PROBE_APPLICATION_DNS_URL)) {
            probes.insert(
                LeakDetectionTarget::BrowserApplicationDns,
                HttpLeakProbe::Reachability { url },
            );
        }
        insert_controlled_probe(
            &mut probes,
            LeakDetectionTarget::Ipv4ControlledLeakService,
            non_empty(lookup(LEAK_PROBE_IPV4_CONTROLLED_URL)),
            non_empty(lookup(LEAK_PROBE_IPV4_EXPECTED_SOURCE)),
            IpFamily::Ipv4,
        );
        if let Some(url) = non_empty(lookup(LEAK_PROBE_IPV4_EXTERNAL_URL)) {
            probes.insert(
                LeakDetectionTarget::Ipv4ExternalTarget,
                HttpLeakProbe::Reachability { url },
            );
        }
        insert_controlled_probe(
            &mut probes,
            LeakDetectionTarget::Ipv6ControlledLeakService,
            non_empty(lookup(LEAK_PROBE_IPV6_CONTROLLED_URL)),
            non_empty(lookup(LEAK_PROBE_IPV6_EXPECTED_SOURCE)),
            IpFamily::Ipv6,
        );
        if let Some(url) = non_empty(lookup(LEAK_PROBE_IPV6_EXTERNAL_URL)) {
            probes.insert(
                LeakDetectionTarget::Ipv6ExternalTarget,
                HttpLeakProbe::Reachability { url },
            );
        }
        Self { probes }
    }

    async fn run(&self, target: LeakDetectionTarget) -> LeakProbeOutcome {
        let Some(probe) = self.probes.get(&target) else {
            return LeakProbeOutcome::not_tested("leak_detection.probe_not_configured");
        };
        run_http_probe(probe, target_family(target)).await
    }
}

fn target_family(target: LeakDetectionTarget) -> Option<IpFamily> {
    match target {
        LeakDetectionTarget::Ipv4Carrier
        | LeakDetectionTarget::Ipv4SourceAddress
        | LeakDetectionTarget::Ipv4DefaultRoute
        | LeakDetectionTarget::Ipv4ControlledLeakService
        | LeakDetectionTarget::Ipv4ExternalTarget => Some(IpFamily::Ipv4),
        LeakDetectionTarget::Ipv6Carrier
        | LeakDetectionTarget::Ipv6SourceAddress
        | LeakDetectionTarget::Ipv6DefaultRoute
        | LeakDetectionTarget::Ipv6ControlledLeakService
        | LeakDetectionTarget::Ipv6ExternalTarget => Some(IpFamily::Ipv6),
        LeakDetectionTarget::DnsResolver | LeakDetectionTarget::BrowserApplicationDns => None,
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

fn insert_controlled_probe(
    probes: &mut BTreeMap<LeakDetectionTarget, HttpLeakProbe>,
    target: LeakDetectionTarget,
    url: Option<String>,
    expected_source: Option<String>,
    family: IpFamily,
) {
    let (Some(url), Some(expected_source)) = (url, expected_source) else {
        return;
    };
    let Ok(expected_source) = expected_source.parse::<IpAddr>() else {
        return;
    };
    if family == IpFamily::Ipv4 && !expected_source.is_ipv4()
        || family == IpFamily::Ipv6 && !expected_source.is_ipv6()
    {
        return;
    }
    probes.insert(
        target,
        HttpLeakProbe::ExpectedSource {
            url,
            expected_source,
        },
    );
}

impl LeakDetectionProbeRunner for ConfiguredLeakDetectionProbeRunner {
    fn probe<'a>(
        &'a self,
        target: LeakDetectionTarget,
        _observed_at_unix_ms: u64,
    ) -> LeakProbeFuture<'a> {
        Box::pin(self.run(target))
    }
}

async fn run_http_probe(probe: &HttpLeakProbe, family: Option<IpFamily>) -> LeakProbeOutcome {
    let started = Instant::now();
    let (url_text, expected_source) = match probe {
        HttpLeakProbe::Reachability { url } => (url.as_str(), None),
        HttpLeakProbe::ExpectedSource {
            url,
            expected_source,
        } => (url.as_str(), Some(*expected_source)),
    };
    let parsed = match reqwest::Url::parse(url_text) {
        Ok(url) if matches!(url.scheme(), "http" | "https") => url,
        _ => return probe_failed(started, "leak_detection.probe_configuration_invalid"),
    };
    let Some(host) = parsed.host_str().map(str::to_string) else {
        return probe_failed(started, "leak_detection.probe_configuration_invalid");
    };
    let Some(port) = parsed.port_or_known_default() else {
        return probe_failed(started, "leak_detection.probe_configuration_invalid");
    };
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(LEAK_DETECTION_COMMAND_TIMEOUT);
    if let Some(family) = family {
        let Some(resolved) = resolve_family_address(&host, port, family).await else {
            return probe_failed(started, "leak_detection.probe_dns_failed");
        };
        builder = builder.resolve(&host, resolved);
    }
    let client = match builder.build() {
        Ok(client) => client,
        Err(_) => return probe_failed(started, "leak_detection.probe_client_failed"),
    };
    let response = match client.get(parsed).send().await {
        Ok(response) => response,
        Err(error) if error.is_timeout() => {
            return probe_failed(started, "leak_detection.probe_timeout")
        }
        Err(error) if error.is_connect() => {
            return probe_failed(started, "leak_detection.probe_connect_failed")
        }
        Err(_) => return probe_failed(started, "leak_detection.probe_request_failed"),
    };
    if !response.status().is_success() {
        return probe_failed(started, "leak_detection.probe_http_status");
    }
    if let Some(expected_source) = expected_source {
        let body = match bounded_response_body(response).await {
            Some(body) => body,
            None => return probe_failed(started, "leak_detection.probe_response_invalid"),
        };
        let observed = body.trim().trim_matches(['[', ']']).parse::<IpAddr>().ok();
        if observed != Some(expected_source) {
            return probe_failed(started, "leak_detection.probe_source_mismatch");
        }
    }
    LeakProbeOutcome {
        state: LeakDetectionEvidenceState::Passed,
        duration_ms: elapsed_ms(started),
        error_code: None,
    }
}

async fn resolve_family_address(
    host: &str,
    port: u16,
    family: IpFamily,
) -> Option<std::net::SocketAddr> {
    if let Ok(address) = host.parse::<IpAddr>() {
        return family_matches(address, family).then(|| std::net::SocketAddr::new(address, port));
    }
    tokio::net::lookup_host((host, port))
        .await
        .ok()?
        .find(|address| family_matches(address.ip(), family))
}

fn family_matches(address: IpAddr, family: IpFamily) -> bool {
    match family {
        IpFamily::Ipv4 => address.is_ipv4(),
        IpFamily::Ipv6 => address.is_ipv6(),
    }
}

async fn bounded_response_body(mut response: reqwest::Response) -> Option<String> {
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if body.len().saturating_add(chunk.len()) > LEAK_PROBE_RESPONSE_BODY_LIMIT {
                    return None;
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(_) => return None,
        }
    }
    String::from_utf8(body).ok()
}

fn probe_failed(started: Instant, error_code: &'static str) -> LeakProbeOutcome {
    LeakProbeOutcome {
        state: LeakDetectionEvidenceState::Failed,
        duration_ms: elapsed_ms(started),
        error_code: Some(error_code.into()),
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Collect local evidence with deployment-configured probes. Missing probes
/// remain `NotTested`, so the resulting snapshot cannot be reported as leak-free
/// until every required path has actual evidence.
pub(crate) async fn collect_leak_detection(
    context: LeakDetectionContext,
    observed_at_unix_ms: u64,
) -> LeakDetectionSnapshot {
    let probes = ConfiguredLeakDetectionProbeRunner::from_env();
    collect_leak_detection_with_probes(context, observed_at_unix_ms, &probes).await
}

pub(crate) async fn collect_leak_detection_with_probes(
    context: LeakDetectionContext,
    observed_at_unix_ms: u64,
    probes: &(dyn LeakDetectionProbeRunner + '_),
) -> LeakDetectionSnapshot {
    #[cfg(target_os = "linux")]
    {
        collect_linux_leak_detection(context, observed_at_unix_ms, probes).await
    }
    #[cfg(not(target_os = "linux"))]
    {
        collect_unsupported_leak_detection(context, observed_at_unix_ms, probes).await
    }
}

#[cfg(target_os = "linux")]
async fn collect_linux_leak_detection(
    context: LeakDetectionContext,
    observed_at_unix_ms: u64,
    probes: &(dyn LeakDetectionProbeRunner + '_),
) -> LeakDetectionSnapshot {
    let (v4_addresses, v4_rules, v4_routes, v6_addresses, v6_rules, v6_routes) = tokio::join!(
        run_ip_query(&["-4", "-o", "addr", "show"]),
        run_ip_query(&["-4", "rule", "show"]),
        run_ip_query(&["-4", "route", "show", "table", "all"]),
        run_ip_query(&["-6", "-o", "addr", "show"]),
        run_ip_query(&["-6", "rule", "show"]),
        run_ip_query(&["-6", "route", "show", "table", "all"]),
    );
    let dns = local_dns_observation();

    let v4 = observe_family(
        IpFamily::Ipv4,
        &context.protected_interfaces,
        &v4_addresses,
        &v4_rules,
        &v4_routes,
    );
    let v6 = observe_family(
        IpFamily::Ipv6,
        &context.protected_interfaces,
        &v6_addresses,
        &v6_rules,
        &v6_routes,
    );
    build_snapshot(
        LeakDetectionPlatform::Linux,
        context,
        observed_at_unix_ms,
        dns,
        v4,
        v6,
        probes,
    )
    .await
}

#[cfg(not(target_os = "linux"))]
async fn collect_unsupported_leak_detection(
    context: LeakDetectionContext,
    observed_at_unix_ms: u64,
    probes: &(dyn LeakDetectionProbeRunner + '_),
) -> LeakDetectionSnapshot {
    let platform = if cfg!(target_os = "windows") {
        LeakDetectionPlatform::Windows
    } else if cfg!(target_os = "macos") {
        LeakDetectionPlatform::Macos
    } else {
        LeakDetectionPlatform::Other
    };
    let probe_outcomes = run_probes(
        probes,
        &[LeakDetectionTarget::BrowserApplicationDns],
        observed_at_unix_ms,
    )
    .await;
    let dns = DnsObservation {
        mode: LeakDetectionDnsMode::Unmanaged,
        state: LeakDetectionEvidenceState::Unsupported,
        source: LeakDetectionSource::SystemResolver,
        error_code: Some("leak_detection.platform_unsupported".into()),
    };
    let v4 = FamilyObservation::unsupported(IpFamily::Ipv4);
    let v6 = FamilyObservation::unsupported(IpFamily::Ipv6);
    build_snapshot_with_probe_outcomes(
        platform,
        context,
        observed_at_unix_ms,
        dns,
        v4,
        v6,
        &probe_outcomes,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IpFamily {
    Ipv4,
    Ipv6,
}

impl IpFamily {
    fn carrier_target(self) -> LeakDetectionTarget {
        match self {
            Self::Ipv4 => LeakDetectionTarget::Ipv4Carrier,
            Self::Ipv6 => LeakDetectionTarget::Ipv6Carrier,
        }
    }

    fn source_target(self) -> LeakDetectionTarget {
        match self {
            Self::Ipv4 => LeakDetectionTarget::Ipv4SourceAddress,
            Self::Ipv6 => LeakDetectionTarget::Ipv6SourceAddress,
        }
    }

    fn route_target(self) -> LeakDetectionTarget {
        match self {
            Self::Ipv4 => LeakDetectionTarget::Ipv4DefaultRoute,
            Self::Ipv6 => LeakDetectionTarget::Ipv6DefaultRoute,
        }
    }

    fn controlled_target(self) -> LeakDetectionTarget {
        match self {
            Self::Ipv4 => LeakDetectionTarget::Ipv4ControlledLeakService,
            Self::Ipv6 => LeakDetectionTarget::Ipv6ControlledLeakService,
        }
    }

    fn external_target(self) -> LeakDetectionTarget {
        match self {
            Self::Ipv4 => LeakDetectionTarget::Ipv4ExternalTarget,
            Self::Ipv6 => LeakDetectionTarget::Ipv6ExternalTarget,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Ipv4 => "ipv4",
            Self::Ipv6 => "ipv6",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DnsObservation {
    mode: LeakDetectionDnsMode,
    state: LeakDetectionEvidenceState,
    source: LeakDetectionSource,
    error_code: Option<String>,
}

#[cfg(target_os = "linux")]
fn local_dns_observation() -> DnsObservation {
    use super::doh_dns;
    use super::recovery::{DnsRecoveryMode, RecoveryResourceState};

    let snapshot = match doh_dns::recovery_snapshot() {
        Ok(snapshot) => snapshot,
        Err(_) => {
            return DnsObservation {
                mode: LeakDetectionDnsMode::Unmanaged,
                state: LeakDetectionEvidenceState::NotTested,
                source: LeakDetectionSource::SystemResolver,
                error_code: Some("leak_detection.dns_snapshot_unavailable".into()),
            };
        }
    };

    let mode = match snapshot.mode {
        DnsRecoveryMode::Doh => LeakDetectionDnsMode::Doh,
        DnsRecoveryMode::Dot => LeakDetectionDnsMode::Dot,
        DnsRecoveryMode::SystemResolver => LeakDetectionDnsMode::SystemResolver,
        DnsRecoveryMode::Disabled | DnsRecoveryMode::Unknown => LeakDetectionDnsMode::Unmanaged,
    };
    let source = if mode == LeakDetectionDnsMode::Doh {
        LeakDetectionSource::ManagedDoh
    } else {
        LeakDetectionSource::SystemResolver
    };
    let (state, error_code) = if snapshot.insecure_fallback_detected
        || snapshot.unsecured_resolvers > 0
        || snapshot.state == RecoveryResourceState::Degraded
    {
        (
            LeakDetectionEvidenceState::Failed,
            Some("leak_detection.dns_insecure_fallback".into()),
        )
    } else if snapshot.state == RecoveryResourceState::Healthy
        && snapshot.authorized_resolvers > 0
        && mode != LeakDetectionDnsMode::Unmanaged
    {
        (LeakDetectionEvidenceState::Passed, None)
    } else if matches!(
        snapshot.state,
        RecoveryResourceState::Missing
            | RecoveryResourceState::NotConfigured
            | RecoveryResourceState::Unsupported
    ) {
        (
            LeakDetectionEvidenceState::Unsupported,
            Some("leak_detection.dns_resolver_unavailable".into()),
        )
    } else {
        (
            LeakDetectionEvidenceState::NotTested,
            Some("leak_detection.dns_behavior_not_observed".into()),
        )
    };
    DnsObservation {
        mode,
        state,
        source,
        error_code,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FamilyObservation {
    family: IpFamily,
    carrier_available: bool,
    carrier_state: LeakDetectionEvidenceState,
    carrier_error: Option<String>,
    carrier_isolation: Option<LeakDetectionIsolationAction>,
    source_state: LeakDetectionEvidenceState,
    source_error: Option<String>,
    route_state: LeakDetectionEvidenceState,
    route_error: Option<String>,
}

impl FamilyObservation {
    #[cfg(not(target_os = "linux"))]
    fn unsupported(family: IpFamily) -> Self {
        Self {
            family,
            carrier_available: false,
            carrier_state: LeakDetectionEvidenceState::Unsupported,
            carrier_error: Some(format!(
                "leak_detection.{}_platform_unsupported",
                family.label()
            )),
            carrier_isolation: None,
            source_state: LeakDetectionEvidenceState::NotTested,
            source_error: Some("leak_detection.platform_unsupported".into()),
            route_state: LeakDetectionEvidenceState::NotTested,
            route_error: Some("leak_detection.platform_unsupported".into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum IpCommandOutcome {
    Output(String),
    Unavailable,
    TimedOut,
    Failed,
}

impl IpCommandOutcome {
    fn output(&self) -> Option<&str> {
        match self {
            Self::Output(output) => Some(output),
            Self::Unavailable | Self::TimedOut | Self::Failed => None,
        }
    }

    fn error_code(&self, operation: &str) -> Option<String> {
        match self {
            Self::Output(_) => None,
            Self::Unavailable => Some(format!("leak_detection.ip_{operation}_unavailable")),
            Self::TimedOut => Some(format!("leak_detection.ip_{operation}_timeout")),
            Self::Failed => Some(format!("leak_detection.ip_{operation}_failed")),
        }
    }
}

#[cfg(target_os = "linux")]
async fn run_ip_query(args: &[&str]) -> IpCommandOutcome {
    let mut command = Command::new("ip");
    command.args(args);
    command.kill_on_drop(true);
    match timeout(LEAK_DETECTION_COMMAND_TIMEOUT, command.output()).await {
        Err(_) => IpCommandOutcome::TimedOut,
        Ok(Err(_)) => IpCommandOutcome::Unavailable,
        Ok(Ok(output)) if !output.status.success() => IpCommandOutcome::Failed,
        Ok(Ok(output)) => {
            IpCommandOutcome::Output(String::from_utf8_lossy(&output.stdout).into_owned())
        }
    }
}

#[cfg(target_os = "linux")]
fn observe_family(
    family: IpFamily,
    protected_interfaces: &[String],
    addresses: &IpCommandOutcome,
    rules: &IpCommandOutcome,
    routes: &IpCommandOutcome,
) -> FamilyObservation {
    let address_error = addresses.error_code("address");
    let route_error = routes.error_code("route");
    let rule_error = rules.error_code("rule");
    let protected: BTreeSet<&str> = protected_interfaces.iter().map(String::as_str).collect();
    let has_protected_source = addresses
        .output()
        .is_some_and(|output| has_protected_global_address(output, family, &protected));
    let route_observation = routes
        .output()
        .map(|output| parse_default_route_observation(output, &protected));
    let any_default_route = route_observation
        .as_ref()
        .is_some_and(|observation| observation.any_default_route);
    let protected_default_route = route_observation
        .as_ref()
        .is_some_and(|observation| !observation.protected_default_tables.is_empty());
    let protected_default_selected = match (
        route_observation.as_ref(),
        rules.output(),
        protected_default_route,
    ) {
        (Some(observation), Some(rule_output), true) => {
            protected_default_table_is_catch_all(rule_output, &observation.protected_default_tables)
        }
        _ => false,
    };

    let (carrier_state, carrier_error, carrier_isolation) = if has_protected_source {
        (LeakDetectionEvidenceState::Passed, None, None)
    } else if route_observation.is_some() && any_default_route {
        (
            LeakDetectionEvidenceState::Failed,
            Some(format!(
                "leak_detection.{}_carrier_outside_protected_path",
                family.label()
            )),
            None,
        )
    } else {
        let isolation = (family == IpFamily::Ipv6
            && addresses.output().is_some()
            && route_observation.is_some()
            && !any_default_route)
            .then_some(LeakDetectionIsolationAction::BlockedUnavailableIpv6);
        (
            LeakDetectionEvidenceState::Unsupported,
            address_error.clone().or_else(|| {
                Some(format!(
                    "leak_detection.{}_carrier_unavailable",
                    family.label()
                ))
            }),
            isolation,
        )
    };

    let (source_state, source_error) = if has_protected_source {
        (LeakDetectionEvidenceState::Passed, None)
    } else {
        (
            LeakDetectionEvidenceState::NotTested,
            address_error.clone().or_else(|| {
                Some(format!(
                    "leak_detection.{}_source_not_observed",
                    family.label()
                ))
            }),
        )
    };

    let (route_state, route_error) = if !has_protected_source {
        (
            LeakDetectionEvidenceState::NotTested,
            route_error
                .clone()
                .or_else(|| Some("leak_detection.carrier_not_available".into())),
        )
    } else if protected_default_selected {
        (LeakDetectionEvidenceState::Passed, None)
    } else if route_error.is_some() || rule_error.is_some() {
        (
            LeakDetectionEvidenceState::Unsupported,
            route_error.clone().or(rule_error.clone()),
        )
    } else if any_default_route {
        (
            LeakDetectionEvidenceState::Failed,
            Some(if protected_default_route {
                format!(
                    "leak_detection.{}_protected_route_not_selected",
                    family.label()
                )
            } else {
                format!(
                    "leak_detection.{}_default_route_outside_protected_path",
                    family.label()
                )
            }),
        )
    } else {
        (
            LeakDetectionEvidenceState::Unsupported,
            Some(format!(
                "leak_detection.{}_default_route_unavailable",
                family.label()
            )),
        )
    };

    FamilyObservation {
        family,
        carrier_available: has_protected_source,
        carrier_state,
        carrier_error,
        carrier_isolation,
        source_state,
        source_error,
        route_state,
        route_error,
    }
}

#[cfg(target_os = "linux")]
fn has_protected_global_address(
    output: &str,
    family: IpFamily,
    protected_interfaces: &BTreeSet<&str>,
) -> bool {
    output.lines().any(|line| {
        let tokens = line.split_whitespace().collect::<Vec<_>>();
        let Some(interface) = tokens
            .get(1)
            .and_then(|token| token.strip_suffix(':').or(Some(*token)))
            .and_then(|token| token.split('@').next())
        else {
            return false;
        };
        if !protected_interfaces.contains(interface) {
            return false;
        }
        let family_token = match family {
            IpFamily::Ipv4 => "inet",
            IpFamily::Ipv6 => "inet6",
        };
        let Some(index) = tokens.iter().position(|token| *token == family_token) else {
            return false;
        };
        let Some(address) = tokens
            .get(index + 1)
            .and_then(|value| value.split('/').next())
            .and_then(|value| value.parse::<IpAddr>().ok())
        else {
            return false;
        };
        if address.is_loopback() || address.is_unspecified() {
            return false;
        }
        let scoped_link_local = tokens.windows(2).any(|pair| pair == ["scope", "link"]);
        !scoped_link_local
    })
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct DefaultRouteObservation {
    any_default_route: bool,
    protected_default_tables: BTreeSet<String>,
}

#[cfg(target_os = "linux")]
fn parse_default_route_observation(
    output: &str,
    protected_interfaces: &BTreeSet<&str>,
) -> DefaultRouteObservation {
    let mut observation = DefaultRouteObservation::default();
    for tokens in output
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>())
    {
        if tokens.first() != Some(&"default") {
            continue;
        }
        observation.any_default_route = true;
        let Some(device) = tokens
            .windows(2)
            .find(|pair| pair[0] == "dev")
            .map(|pair| pair[1])
        else {
            continue;
        };
        if !protected_interfaces.contains(device) {
            continue;
        }
        let table = tokens
            .windows(2)
            .find(|pair| pair[0] == "table")
            .map(|pair| pair[1])
            .unwrap_or("254");
        observation
            .protected_default_tables
            .insert(table.to_string());
    }
    observation
}

#[cfg(target_os = "linux")]
fn protected_default_table_is_catch_all(
    rule_output: &str,
    protected_tables: &BTreeSet<String>,
) -> bool {
    rule_output.lines().any(|line| {
        let tokens = line.split_whitespace().collect::<Vec<_>>();
        let table = tokens
            .windows(2)
            .find(|pair| pair[0] == "lookup" || pair[0] == "table")
            .map(|pair| pair[1]);
        let Some(table) = table else {
            return false;
        };
        if !protected_tables.contains(table) {
            return false;
        }
        if !tokens.windows(2).any(|pair| pair == ["from", "all"]) {
            return false;
        }
        !tokens.iter().any(|token| {
            matches!(
                *token,
                "to" | "fwmark" | "iif" | "oif" | "uidrange" | "sport" | "dport" | "ipproto"
            )
        })
    })
}

async fn run_probes(
    probes: &(dyn LeakDetectionProbeRunner + '_),
    targets: &[LeakDetectionTarget],
    observed_at_unix_ms: u64,
) -> BTreeMap<LeakDetectionTarget, LeakProbeOutcome> {
    let results = futures::future::join_all(targets.iter().copied().map(|target| async move {
        let outcome = match timeout(
            LEAK_DETECTION_PROBE_TIMEOUT,
            probes.probe(target, observed_at_unix_ms),
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(_) => LeakProbeOutcome::not_tested("leak_detection.probe_timeout"),
        };
        (target, normalize_probe_outcome(outcome))
    }))
    .await;
    results.into_iter().collect()
}

fn normalize_probe_outcome(mut outcome: LeakProbeOutcome) -> LeakProbeOutcome {
    if outcome.state == LeakDetectionEvidenceState::Failed && outcome.error_code.is_none() {
        outcome.error_code = Some("leak_detection.probe_failed".into());
    }
    outcome
}

#[allow(clippy::too_many_arguments)]
async fn build_snapshot(
    platform: LeakDetectionPlatform,
    context: LeakDetectionContext,
    observed_at_unix_ms: u64,
    dns: DnsObservation,
    v4: FamilyObservation,
    v6: FamilyObservation,
    probes: &(dyn LeakDetectionProbeRunner + '_),
) -> LeakDetectionSnapshot {
    let mut probe_targets = vec![LeakDetectionTarget::BrowserApplicationDns];
    if v4.carrier_available {
        probe_targets.push(v4.family.controlled_target());
        probe_targets.push(v4.family.external_target());
    }
    if v6.carrier_available {
        probe_targets.push(v6.family.controlled_target());
        probe_targets.push(v6.family.external_target());
    }
    let outcomes = run_probes(probes, &probe_targets, observed_at_unix_ms).await;
    build_snapshot_with_probe_outcomes(
        platform,
        context,
        observed_at_unix_ms,
        dns,
        v4,
        v6,
        &outcomes,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_snapshot_with_probe_outcomes(
    platform: LeakDetectionPlatform,
    context: LeakDetectionContext,
    observed_at_unix_ms: u64,
    dns: DnsObservation,
    v4: FamilyObservation,
    v6: FamilyObservation,
    probe_outcomes: &BTreeMap<LeakDetectionTarget, LeakProbeOutcome>,
) -> LeakDetectionSnapshot {
    let observed_at_unix_ms = observed_at_unix_ms.max(1);
    let mut evidence = vec![
        LeakDetectionEvidence {
            target: LeakDetectionTarget::DnsResolver,
            state: dns.state,
            source: dns.source,
            observed_at_unix_ms,
            duration_ms: 0,
            error_code: dns.error_code,
            isolation_action: None,
        },
        probe_evidence(
            LeakDetectionTarget::BrowserApplicationDns,
            LeakDetectionSource::ApplicationBehaviorProbe,
            observed_at_unix_ms,
            probe_outcomes,
        ),
    ];
    evidence.extend(family_evidence(&v4, observed_at_unix_ms, probe_outcomes));
    evidence.extend(family_evidence(&v6, observed_at_unix_ms, probe_outcomes));
    LeakDetectionSnapshot {
        generated_at_unix_ms: observed_at_unix_ms,
        environment: LeakDetectionEnvironment {
            platform,
            dns_mode: dns.mode,
            protected_path_active: context.protected_path_active,
            ipv4_carrier_available: v4.carrier_available,
            ipv6_carrier_available: v6.carrier_available,
        },
        evidence,
        ..LeakDetectionSnapshot::default()
    }
}

fn family_evidence(
    observation: &FamilyObservation,
    observed_at_unix_ms: u64,
    probe_outcomes: &BTreeMap<LeakDetectionTarget, LeakProbeOutcome>,
) -> Vec<LeakDetectionEvidence> {
    let family = observation.family;
    let mut evidence = vec![
        LeakDetectionEvidence {
            target: family.carrier_target(),
            state: observation.carrier_state,
            source: LeakDetectionSource::LinkCapability,
            observed_at_unix_ms,
            duration_ms: 0,
            error_code: observation.carrier_error.clone(),
            isolation_action: observation.carrier_isolation,
        },
        LeakDetectionEvidence {
            target: family.source_target(),
            state: observation.source_state,
            source: LeakDetectionSource::LocalRoutingTable,
            observed_at_unix_ms,
            duration_ms: 0,
            error_code: observation.source_error.clone(),
            isolation_action: None,
        },
        LeakDetectionEvidence {
            target: family.route_target(),
            state: observation.route_state,
            source: LeakDetectionSource::LocalRoutingTable,
            observed_at_unix_ms,
            duration_ms: 0,
            error_code: observation.route_error.clone(),
            isolation_action: None,
        },
    ];
    for (target, source) in [
        (
            family.controlled_target(),
            LeakDetectionSource::ControlledProbe,
        ),
        (
            family.external_target(),
            LeakDetectionSource::ExternalReachabilityProbe,
        ),
    ] {
        if observation.carrier_available {
            evidence.push(probe_evidence(
                target,
                source,
                observed_at_unix_ms,
                probe_outcomes,
            ));
        } else {
            evidence.push(LeakDetectionEvidence {
                target,
                state: LeakDetectionEvidenceState::NotTested,
                source,
                observed_at_unix_ms,
                duration_ms: 0,
                error_code: Some("leak_detection.probe_not_applicable".into()),
                isolation_action: None,
            });
        }
    }
    evidence
}

fn probe_evidence(
    target: LeakDetectionTarget,
    source: LeakDetectionSource,
    observed_at_unix_ms: u64,
    probe_outcomes: &BTreeMap<LeakDetectionTarget, LeakProbeOutcome>,
) -> LeakDetectionEvidence {
    let outcome = probe_outcomes
        .get(&target)
        .cloned()
        .unwrap_or_else(|| LeakProbeOutcome::not_tested("leak_detection.probe_not_executed"));
    LeakDetectionEvidence {
        target,
        state: outcome.state,
        source,
        observed_at_unix_ms,
        duration_ms: outcome.duration_ms,
        error_code: outcome.error_code,
        isolation_action: None,
    }
}

fn valid_interface_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 15
        && !matches!(name, "." | "..")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuckipv6_common::domain::LeakDetectionState;
    use std::collections::BTreeSet;

    const NOW: u64 = 1_700_000_000_000;

    async fn spawn_http_probe(body: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind probe listener");
        let address = listener.local_addr().expect("probe listener address");
        let app = axum::Router::new().route("/", axum::routing::get(move || async move { body }));
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{address}/")
    }

    fn configured_probe(
        ipv4_controlled_url: &str,
        ipv4_expected_source: &str,
        ipv4_external_url: &str,
    ) -> ConfiguredLeakDetectionProbeRunner {
        ConfiguredLeakDetectionProbeRunner::from_lookup(|name| match name {
            LEAK_PROBE_IPV4_CONTROLLED_URL => Some(ipv4_controlled_url.to_string()),
            LEAK_PROBE_IPV4_EXPECTED_SOURCE => Some(ipv4_expected_source.to_string()),
            LEAK_PROBE_IPV4_EXTERNAL_URL => Some(ipv4_external_url.to_string()),
            _ => None,
        })
    }

    fn passing_dns() -> DnsObservation {
        DnsObservation {
            mode: LeakDetectionDnsMode::Doh,
            state: LeakDetectionEvidenceState::Passed,
            source: LeakDetectionSource::ManagedDoh,
            error_code: None,
        }
    }

    fn passing_family(family: IpFamily) -> FamilyObservation {
        FamilyObservation {
            family,
            carrier_available: true,
            carrier_state: LeakDetectionEvidenceState::Passed,
            carrier_error: None,
            carrier_isolation: None,
            source_state: LeakDetectionEvidenceState::Passed,
            source_error: None,
            route_state: LeakDetectionEvidenceState::Passed,
            route_error: None,
        }
    }

    fn probe_map() -> BTreeMap<LeakDetectionTarget, LeakProbeOutcome> {
        [
            LeakDetectionTarget::BrowserApplicationDns,
            LeakDetectionTarget::Ipv4ControlledLeakService,
            LeakDetectionTarget::Ipv4ExternalTarget,
            LeakDetectionTarget::Ipv6ControlledLeakService,
            LeakDetectionTarget::Ipv6ExternalTarget,
        ]
        .into_iter()
        .map(|target| {
            (
                target,
                LeakProbeOutcome {
                    state: LeakDetectionEvidenceState::Passed,
                    duration_ms: 1,
                    error_code: None,
                },
            )
        })
        .collect()
    }

    #[test]
    fn complete_local_and_probe_evidence_can_pass_without_leaking_host_details() {
        let snapshot = build_snapshot_with_probe_outcomes(
            LeakDetectionPlatform::Linux,
            LeakDetectionContext::new(true, vec!["fuckipv6-test".into()]),
            NOW,
            passing_dns(),
            passing_family(IpFamily::Ipv4),
            passing_family(IpFamily::Ipv6),
            &probe_map(),
        );
        assert_eq!(snapshot.assess(NOW).state, LeakDetectionState::LeakFree);
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(!json.contains("fuckipv6-test"));
        assert!(!json.contains("1.1.1.1"));
        assert!(!json.contains("stderr"));
    }

    #[test]
    fn noop_external_and_browser_probes_keep_the_report_incomplete() {
        let noop = BTreeMap::from([
            (
                LeakDetectionTarget::BrowserApplicationDns,
                LeakProbeOutcome::not_tested("leak_detection.probe_not_configured"),
            ),
            (
                LeakDetectionTarget::Ipv4ControlledLeakService,
                LeakProbeOutcome::not_tested("leak_detection.probe_not_configured"),
            ),
            (
                LeakDetectionTarget::Ipv4ExternalTarget,
                LeakProbeOutcome::not_tested("leak_detection.probe_not_configured"),
            ),
            (
                LeakDetectionTarget::Ipv6ControlledLeakService,
                LeakProbeOutcome::not_tested("leak_detection.probe_not_configured"),
            ),
            (
                LeakDetectionTarget::Ipv6ExternalTarget,
                LeakProbeOutcome::not_tested("leak_detection.probe_not_configured"),
            ),
        ]);
        let snapshot = build_snapshot_with_probe_outcomes(
            LeakDetectionPlatform::Linux,
            LeakDetectionContext::new(true, vec!["fuckipv6-test".into()]),
            NOW,
            passing_dns(),
            passing_family(IpFamily::Ipv4),
            passing_family(IpFamily::Ipv6),
            &noop,
        );
        let assessment = snapshot.assess(NOW);
        assert_eq!(assessment.state, LeakDetectionState::InsufficientEvidence);
        assert!(assessment.fail_closed_required);
        assert_eq!(assessment.reason_code, "leak_detection.evidence_not_tested");
    }

    #[tokio::test]
    async fn configured_http_probes_verify_family_reachability_and_expected_source() {
        let url = spawn_http_probe("127.0.0.1\n").await;
        let probes = configured_probe(&url, "127.0.0.1", &url);

        let controlled = probes
            .probe(LeakDetectionTarget::Ipv4ControlledLeakService, NOW)
            .await;
        assert_eq!(controlled.state, LeakDetectionEvidenceState::Passed);
        assert_eq!(controlled.error_code, None);

        let external = probes
            .probe(LeakDetectionTarget::Ipv4ExternalTarget, NOW)
            .await;
        assert_eq!(external.state, LeakDetectionEvidenceState::Passed);
        assert_eq!(external.error_code, None);
    }

    #[tokio::test]
    async fn configured_controlled_probe_fails_closed_on_source_mismatch() {
        let url = spawn_http_probe("127.0.0.2\n").await;
        let probes = configured_probe(&url, "127.0.0.1", &url);

        let outcome = probes
            .probe(LeakDetectionTarget::Ipv4ControlledLeakService, NOW)
            .await;
        assert_eq!(outcome.state, LeakDetectionEvidenceState::Failed);
        assert_eq!(
            outcome.error_code.as_deref(),
            Some("leak_detection.probe_source_mismatch")
        );
    }

    #[tokio::test]
    async fn unconfigured_probe_stays_not_tested() {
        let probes = ConfiguredLeakDetectionProbeRunner::default();
        let outcome = probes
            .probe(LeakDetectionTarget::BrowserApplicationDns, NOW)
            .await;
        assert_eq!(outcome.state, LeakDetectionEvidenceState::NotTested);
        assert_eq!(
            outcome.error_code.as_deref(),
            Some("leak_detection.probe_not_configured")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn address_parser_requires_a_global_address_on_a_protected_interface() {
        let protected = BTreeSet::from(["fuckipv6-test"]);
        let output = "2: eth0    inet 192.0.2.10/24 scope global eth0\n\
                      7: fuckipv6-test    inet 198.51.100.2/32 scope global fuckipv6-test\n";
        assert!(has_protected_global_address(
            output,
            IpFamily::Ipv4,
            &protected
        ));
        assert!(!has_protected_global_address(
            output,
            IpFamily::Ipv6,
            &protected
        ));
        assert!(!has_protected_global_address(
            output,
            IpFamily::Ipv4,
            &BTreeSet::from(["other"])
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn default_route_parser_requires_a_catch_all_rule_for_the_protected_table() {
        let protected = BTreeSet::from(["fuckipv6-test"]);
        let routes = "default via 192.0.2.1 dev eth0\n\
                      default dev fuckipv6-test table 10000\n";
        let parsed = parse_default_route_observation(routes, &protected);
        assert!(parsed.any_default_route);
        assert_eq!(
            parsed.protected_default_tables,
            BTreeSet::from(["10000".into()])
        );
        assert!(protected_default_table_is_catch_all(
            "32000: from all lookup 10000 proto openr\n",
            &parsed.protected_default_tables
        ));
        assert!(!protected_default_table_is_catch_all(
            "32000: from 192.0.2.10 lookup 10000 proto openr\n",
            &parsed.protected_default_tables
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unprotected_default_route_is_reported_as_a_leak_not_a_pass() {
        let observation = observe_family(
            IpFamily::Ipv4,
            &["fuckipv6-test".into()],
            &IpCommandOutcome::Output("2: eth0 inet 192.0.2.10/24 scope global eth0\n".into()),
            &IpCommandOutcome::Output("0: from all lookup local\n".into()),
            &IpCommandOutcome::Output("default via 192.0.2.1 dev eth0\n".into()),
        );
        assert!(!observation.carrier_available);
        assert_eq!(
            observation.carrier_state,
            LeakDetectionEvidenceState::Failed
        );
        assert_eq!(
            observation.carrier_error.as_deref(),
            Some("leak_detection.ipv4_carrier_outside_protected_path")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn absent_ipv6_carrier_and_default_route_is_reported_as_isolated() {
        let observation = observe_family(
            IpFamily::Ipv6,
            &["fuckipv6-test".into()],
            &IpCommandOutcome::Output("2: eth0 inet6 fe80::1/64 scope link eth0\n".into()),
            &IpCommandOutcome::Output("0: from all lookup local\n".into()),
            &IpCommandOutcome::Output("192.0.2.0/24 dev eth0\n".into()),
        );
        assert_eq!(
            observation.carrier_state,
            LeakDetectionEvidenceState::Unsupported
        );
        assert_eq!(
            observation.carrier_isolation,
            Some(LeakDetectionIsolationAction::BlockedUnavailableIpv6)
        );
    }
}
