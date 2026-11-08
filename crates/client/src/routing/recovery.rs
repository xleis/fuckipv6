//! Bounded recovery coordinator for TUN, policy routes, DNS and failed paths.
//!
//! OS suspend/resume and network-change sources are intentionally injected.
//! This module owns the common security-relevant recovery sequence so every
//! platform adapter must take a before/after snapshot, reconcile owned
//! resources, revoke stale paths, apply the versioned failure policy and verify
//! controlled reachability before it can report a healthy recovery.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use fuckipv6_common::domain::{
    TrafficFailureDecision, TrafficFailureKind, TrafficFailurePolicy, TrafficFailureRouteContext,
    TrafficRuntimeDegradation, TrafficRuntimeDegradationState,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::time::{timeout, Duration, Instant};

pub const DEFAULT_RECOVERY_TIMEOUT_MS: u64 = 10_000;

type RecoveryFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, RecoveryOperationError>> + Send + 'a>>;

/// A structured adapter failure. Raw command output and host paths must stay in
/// local logs and never be copied into the recovery report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryOperationError {
    pub code: String,
}

impl RecoveryOperationError {
    pub fn new(code: impl Into<String>) -> Self {
        Self { code: code.into() }
    }
}

/// Reason a bounded recovery pass was requested. The concrete adapter owns the
/// OS event source; the coordinator owns the behavior after the trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryTrigger {
    SleepWake,
    NetworkChanged,
    ExitDisconnected,
    RelayDisconnected,
    Manual,
}

impl RecoveryTrigger {
    fn invalidated_path(self) -> Option<InvalidatedPath> {
        match self {
            Self::ExitDisconnected => Some(InvalidatedPath::Exit),
            Self::RelayDisconnected => Some(InvalidatedPath::Relay),
            Self::SleepWake | Self::NetworkChanged | Self::Manual => None,
        }
    }

    fn failure_kind(self) -> Option<TrafficFailureKind> {
        match self {
            Self::ExitDisconnected => Some(TrafficFailureKind::ExitUnavailable),
            Self::RelayDisconnected => Some(TrafficFailureKind::NetworkSelectionUnavailable),
            Self::SleepWake | Self::NetworkChanged | Self::Manual => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvalidatedPath {
    Exit,
    Relay,
}

/// Expected host resources for this recovery pass. A disabled component is
/// represented explicitly instead of treating a missing unexpected resource as
/// healthy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RecoveryExpectations {
    pub tun_required: bool,
    pub routes_required: bool,
    pub dns_required: bool,
}

impl Default for RecoveryExpectations {
    fn default() -> Self {
        Self {
            tun_required: true,
            routes_required: true,
            dns_required: true,
        }
    }
}

/// Stable target identifiers. Actual host names and resolved addresses remain
/// inside the probe adapter and are not persisted in the public report.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum RecoveryProbeTarget {
    Google,
    YouTube,
    Controlled(String),
}

impl RecoveryProbeTarget {
    pub fn key(&self) -> String {
        match self {
            Self::Google => "google".into(),
            Self::YouTube => "youtube".into(),
            Self::Controlled(id) => format!("controlled:{id}"),
        }
    }

    fn validate(&self) -> bool {
        match self {
            Self::Google | Self::YouTube => true,
            Self::Controlled(id) => {
                !id.is_empty()
                    && id.len() <= 128
                    && id.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
                    })
            }
        }
    }
}

/// The default product check includes both required consumer targets and one
/// deployment-controlled target. The caller may replace the list, but must
/// provide at least one explicit target.
pub fn default_recovery_probes() -> Vec<RecoveryProbeTarget> {
    vec![
        RecoveryProbeTarget::Google,
        RecoveryProbeTarget::YouTube,
        RecoveryProbeTarget::Controlled("product-control-target".into()),
    ]
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryRequest {
    pub trigger: RecoveryTrigger,
    pub route_context: TrafficFailureRouteContext,
    pub failure_policy: TrafficFailurePolicy,
    pub expectations: RecoveryExpectations,
    pub probes: Vec<RecoveryProbeTarget>,
    pub hard_timeout_ms: u64,
}

impl RecoveryRequest {
    pub fn new(
        trigger: RecoveryTrigger,
        route_context: TrafficFailureRouteContext,
        failure_policy: TrafficFailurePolicy,
    ) -> Self {
        Self {
            trigger,
            route_context,
            failure_policy,
            expectations: RecoveryExpectations::default(),
            probes: default_recovery_probes(),
            hard_timeout_ms: DEFAULT_RECOVERY_TIMEOUT_MS,
        }
    }

    pub fn with_expectations(mut self, expectations: RecoveryExpectations) -> Self {
        self.expectations = expectations;
        self
    }

    pub fn with_probes(mut self, probes: Vec<RecoveryProbeTarget>) -> Self {
        self.probes = probes;
        self
    }

    pub fn with_hard_timeout_ms(mut self, hard_timeout_ms: u64) -> Self {
        self.hard_timeout_ms = hard_timeout_ms;
        self
    }

    fn validate(&self) -> Result<(), ()> {
        if self.hard_timeout_ms == 0 || self.probes.is_empty() {
            return Err(());
        }
        if self.probes.iter().any(|target| !target.validate()) {
            return Err(());
        }
        self.failure_policy.validate().map_err(|_| ())?;
        // The common failure-policy method also validates the route context.
        self.failure_policy
            .decide(&self.route_context, TrafficFailureKind::ConnectionTimeout)
            .map_err(|_| ())?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryResourceState {
    Healthy,
    Degraded,
    Missing,
    Failed,
    Unsupported,
    NotConfigured,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TunRecoverySnapshot {
    pub state: RecoveryResourceState,
    pub duplicate_interfaces: usize,
    pub residual_interfaces: usize,
}

impl TunRecoverySnapshot {
    fn is_healthy(&self) -> bool {
        self.state == RecoveryResourceState::Healthy
            && self.duplicate_interfaces == 0
            && self.residual_interfaces == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteRecoverySnapshot {
    pub expected_routes: usize,
    pub installed_routes: usize,
    pub missing_routes: usize,
    pub duplicate_routes: usize,
    pub residual_routes: usize,
    pub stale_interface_routes: usize,
}

impl RouteRecoverySnapshot {
    fn is_healthy(&self) -> bool {
        self.installed_routes >= self.expected_routes
            && self.missing_routes == 0
            && self.duplicate_routes == 0
            && self.residual_routes == 0
            && self.stale_interface_routes == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DnsRecoveryMode {
    SystemResolver,
    Doh,
    Dot,
    Disabled,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DnsRecoverySnapshot {
    pub mode: DnsRecoveryMode,
    pub state: RecoveryResourceState,
    pub authorized_resolvers: usize,
    pub unsecured_resolvers: usize,
    pub insecure_fallback_detected: bool,
}

impl DnsRecoverySnapshot {
    fn is_secure_and_healthy(&self) -> bool {
        self.mode != DnsRecoveryMode::Unknown
            && self.state == RecoveryResourceState::Healthy
            && self.authorized_resolvers > 0
            && self.unsecured_resolvers == 0
            && !self.insecure_fallback_detected
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathRecoveryState {
    Connected,
    Disconnected,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryResourceSnapshot {
    pub tun: TunRecoverySnapshot,
    pub routes: RouteRecoverySnapshot,
    pub dns: DnsRecoverySnapshot,
    pub exit_path: PathRecoveryState,
    pub relay_path: PathRecoveryState,
}

/// Sanitized result of owned-resource reconciliation. The adapter may remove
/// residual and duplicate entries only when they belong to this runtime; an
/// unknown owner must be reported as a failure instead of being claimed.
#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RecoveryRepairReport {
    pub tun_recreated: bool,
    pub routes_restored: usize,
    pub residual_routes_removed: usize,
    pub duplicate_routes_removed: usize,
    pub stale_interface_routes_removed: usize,
    pub dns_reconfigured: bool,
    pub used_insecure_dns_fallback: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryProbeState {
    Passed,
    Failed,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryProbeStage {
    Dns,
    Tcp,
    Tls,
    Exit,
    Relay,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryProbeObservation {
    pub state: RecoveryProbeState,
    pub failed_stage: Option<RecoveryProbeStage>,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryProbeResult {
    pub target: RecoveryProbeTarget,
    pub state: RecoveryProbeState,
    pub failed_stage: Option<RecoveryProbeStage>,
    pub duration_ms: u64,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureApplication {
    pub diagnostic_id: String,
    pub decision: TrafficFailureDecision,
}

/// Side-effect boundary used by platform adapters. Implementations may mutate
/// TUN/routes/DNS, so each method must be cancellation safe and idempotent: a
/// coordinator deadline can cancel a pending future and a later pass must be
/// able to finish cleanup.
pub trait RecoveryBackend: Send {
    fn snapshot(&mut self) -> RecoveryFuture<'_, RecoveryResourceSnapshot>;

    /// Re-evaluate current policy and DNS for this trigger, then reconcile owned
    /// resources. It must remove residue/duplicates/stale-interface routes and
    /// recreate TUN/routes/DNS as needed.
    fn reconcile(&mut self, trigger: RecoveryTrigger) -> RecoveryFuture<'_, RecoveryRepairReport>;

    fn revoke_path(&mut self, path: InvalidatedPath) -> RecoveryFuture<'_, ()>;

    /// The adapter must persist an audit record tied to `diagnostic_id` before
    /// treating the degraded state as complete.
    fn apply_failure_decision(&mut self, application: FailureApplication)
        -> RecoveryFuture<'_, ()>;

    fn probe(
        &mut self,
        target: RecoveryProbeTarget,
    ) -> RecoveryFuture<'_, RecoveryProbeObservation>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryStatus {
    Recovered,
    DegradedFailClosed,
    DegradedDirectFallback,
    TimedOut,
    Failed,
}

impl RecoveryStatus {
    fn message_key(self) -> &'static str {
        match self {
            Self::Recovered => "recovery.passed",
            Self::DegradedFailClosed => "recovery.degraded_fail_closed",
            Self::DegradedDirectFallback => "recovery.degraded_direct_fallback",
            Self::TimedOut => "recovery.timed_out",
            Self::Failed => "recovery.failed",
        }
    }
}

/// Durable, redacted recovery record. It intentionally excludes interface
/// names, addresses, resolver URLs, command output and target host names.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryReport {
    pub diagnostic_id: String,
    pub trigger: RecoveryTrigger,
    pub started_at_unix_ms: u64,
    pub completed_at_unix_ms: u64,
    pub hard_timeout_ms: u64,
    pub timed_out: bool,
    pub status: RecoveryStatus,
    pub message_key: String,
    pub error_code: Option<String>,
    pub fail_closed_required: bool,
    pub before: Option<RecoveryResourceSnapshot>,
    pub after: Option<RecoveryResourceSnapshot>,
    pub repair: RecoveryRepairReport,
    pub revoked_paths: Vec<InvalidatedPath>,
    pub failure_decisions: Vec<TrafficFailureDecision>,
    pub degradation: Option<TrafficRuntimeDegradation>,
    pub probes: Vec<RecoveryProbeResult>,
}

impl RecoveryReport {
    pub fn is_success(&self) -> bool {
        self.status == RecoveryStatus::Recovered
    }

    fn new(request: &RecoveryRequest, started_at_unix_ms: u64, diagnostic_id: String) -> Self {
        Self {
            diagnostic_id,
            trigger: request.trigger,
            started_at_unix_ms,
            completed_at_unix_ms: started_at_unix_ms,
            hard_timeout_ms: request.hard_timeout_ms,
            timed_out: false,
            status: RecoveryStatus::Failed,
            message_key: RecoveryStatus::Failed.message_key().into(),
            error_code: None,
            fail_closed_required: true,
            before: None,
            after: None,
            repair: RecoveryRepairReport::default(),
            revoked_paths: Vec::new(),
            failure_decisions: Vec::new(),
            degradation: None,
            probes: Vec::new(),
        }
    }

    fn finish(
        mut self,
        status: RecoveryStatus,
        error_code: Option<&'static str>,
    ) -> RecoveryReport {
        self.completed_at_unix_ms = unix_time_ms();
        self.status = status;
        self.message_key = status.message_key().into();
        self.timed_out = status == RecoveryStatus::TimedOut;
        self.error_code = error_code.map(str::to_owned);
        self.fail_closed_required = !matches!(
            status,
            RecoveryStatus::Recovered | RecoveryStatus::DegradedDirectFallback
        );
        self
    }
}

#[derive(Debug, Default)]
pub struct RecoveryCoordinator;

impl RecoveryCoordinator {
    pub async fn recover(
        &self,
        backend: &mut dyn RecoveryBackend,
        request: RecoveryRequest,
    ) -> RecoveryReport {
        let started_at = unix_time_ms();
        let diagnostic_id = next_diagnostic_id(started_at);
        let mut report = RecoveryReport::new(&request, started_at, diagnostic_id);
        if request.validate().is_err() {
            return report.finish(RecoveryStatus::Failed, Some("recovery.invalid_request"));
        }

        let deadline = Instant::now() + Duration::from_millis(request.hard_timeout_ms);

        let before = match bounded(deadline, backend.snapshot()).await {
            Ok(snapshot) => snapshot,
            Err(BoundedError::TimedOut) => {
                return report.finish(RecoveryStatus::TimedOut, Some("recovery.timed_out"));
            }
            Err(BoundedError::Failed) => {
                return report.finish(RecoveryStatus::Failed, Some("recovery.snapshot_failed"));
            }
        };
        report.before = Some(before);

        if let Some(path) = request.trigger.invalidated_path() {
            let revoke_ok = match bounded(deadline, backend.revoke_path(path)).await {
                Ok(()) => {
                    report.revoked_paths.push(path);
                    true
                }
                Err(BoundedError::Failed) => false,
                Err(BoundedError::TimedOut) => {
                    return report.finish(RecoveryStatus::TimedOut, Some("recovery.timed_out"));
                }
            };

            let failure = request
                .trigger
                .failure_kind()
                .expect("disconnect triggers have a failure kind");
            let decision = if revoke_ok {
                request
                    .failure_policy
                    .decide(&request.route_context, failure)
            } else {
                fail_closed_decision(&request.route_context, failure)
            };

            let decision = match decision {
                Ok(decision) => decision,
                Err(_) => {
                    return report.finish(
                        RecoveryStatus::Failed,
                        Some("recovery.failure_policy_invalid"),
                    );
                }
            };

            match bounded(
                deadline,
                backend.apply_failure_decision(FailureApplication {
                    diagnostic_id: report.diagnostic_id.clone(),
                    decision: decision.clone(),
                }),
            )
            .await
            {
                Ok(()) => {
                    report.degradation = Some(decision.runtime_degradation());
                    report.failure_decisions.push(decision);
                }
                Err(BoundedError::Failed) => {
                    return report.finish(
                        RecoveryStatus::Failed,
                        Some("recovery.failure_policy_apply_failed"),
                    );
                }
                Err(BoundedError::TimedOut) => {
                    return report.finish(RecoveryStatus::TimedOut, Some("recovery.timed_out"));
                }
            }

            if !revoke_ok {
                return report.finish(RecoveryStatus::Failed, Some("recovery.path_revoke_failed"));
            }
        }

        let repair = match bounded(deadline, backend.reconcile(request.trigger)).await {
            Ok(report) => report,
            Err(BoundedError::TimedOut) => {
                return report.finish(RecoveryStatus::TimedOut, Some("recovery.timed_out"));
            }
            Err(BoundedError::Failed) => {
                return report.finish(RecoveryStatus::Failed, Some("recovery.reconcile_failed"));
            }
        };
        report.repair = repair;

        if report.repair.used_insecure_dns_fallback {
            apply_fail_closed(
                backend,
                deadline,
                &request,
                &mut report,
                TrafficFailureKind::DnsPollutionDetected,
            )
            .await;
            return report.finish(
                RecoveryStatus::Failed,
                Some("recovery.dns_unsafe_fallback_forbidden"),
            );
        }

        let after = match bounded(deadline, backend.snapshot()).await {
            Ok(snapshot) => snapshot,
            Err(BoundedError::TimedOut) => {
                return report.finish(RecoveryStatus::TimedOut, Some("recovery.timed_out"));
            }
            Err(BoundedError::Failed) => {
                return report.finish(
                    RecoveryStatus::Failed,
                    Some("recovery.post_snapshot_failed"),
                );
            }
        };
        report.after = Some(after.clone());

        if let Some((error_code, failure)) = resource_validation_error(&after, request.expectations)
        {
            apply_fail_closed(backend, deadline, &request, &mut report, failure).await;
            return report.finish(RecoveryStatus::Failed, Some(error_code));
        }

        let expected_probe_count = request.probes.len();
        for target in request.probes {
            let started = Instant::now();
            let result = match bounded(deadline, backend.probe(target.clone())).await {
                Ok(observation) => RecoveryProbeResult {
                    target,
                    state: observation.state,
                    failed_stage: observation.failed_stage,
                    duration_ms: elapsed_ms(started),
                    error_code: observation.error_code,
                },
                Err(BoundedError::Failed) => RecoveryProbeResult {
                    target,
                    state: RecoveryProbeState::Failed,
                    failed_stage: None,
                    duration_ms: elapsed_ms(started),
                    error_code: Some("recovery.probe_failed".into()),
                },
                Err(BoundedError::TimedOut) => {
                    return report.finish(RecoveryStatus::TimedOut, Some("recovery.timed_out"));
                }
            };
            report.probes.push(result);
        }

        let probes_passed = expected_probe_count == 0
            || report
                .probes
                .iter()
                .all(|probe| probe.state == RecoveryProbeState::Passed);
        if !probes_passed {
            if report.degradation.as_ref().is_some_and(|degradation| {
                degradation.state == TrafficRuntimeDegradationState::DegradedFailClosed
            }) {
                return report.finish(
                    RecoveryStatus::DegradedFailClosed,
                    Some("recovery.reachability_blocked_by_fail_closed"),
                );
            }
            return report.finish(RecoveryStatus::Failed, Some("recovery.reachability_failed"));
        }

        let status = report
            .degradation
            .as_ref()
            .map_or(RecoveryStatus::Recovered, |degradation| {
                match degradation.state {
                    TrafficRuntimeDegradationState::DegradedFailClosed => {
                        RecoveryStatus::DegradedFailClosed
                    }
                    TrafficRuntimeDegradationState::DegradedDirectFallback => {
                        RecoveryStatus::DegradedDirectFallback
                    }
                }
            });
        report.finish(status, None)
    }
}

fn resource_validation_error(
    snapshot: &RecoveryResourceSnapshot,
    expectations: RecoveryExpectations,
) -> Option<(&'static str, TrafficFailureKind)> {
    if snapshot.dns.insecure_fallback_detected || snapshot.dns.unsecured_resolvers > 0 {
        return Some((
            "recovery.dns_unsafe_fallback_forbidden",
            TrafficFailureKind::DnsPollutionDetected,
        ));
    }
    if expectations.dns_required && !snapshot.dns.is_secure_and_healthy() {
        return Some((
            "recovery.dns_unhealthy",
            TrafficFailureKind::DnsResolutionTimeout,
        ));
    }
    if expectations.tun_required && !snapshot.tun.is_healthy() {
        return Some((
            "recovery.tun_unhealthy",
            TrafficFailureKind::NetworkSelectionUnavailable,
        ));
    }
    if expectations.routes_required && !snapshot.routes.is_healthy() {
        return Some((
            "recovery.routes_unhealthy",
            TrafficFailureKind::NetworkSelectionUnavailable,
        ));
    }
    None
}

async fn apply_fail_closed(
    backend: &mut dyn RecoveryBackend,
    deadline: Instant,
    request: &RecoveryRequest,
    report: &mut RecoveryReport,
    failure: TrafficFailureKind,
) {
    let Ok(decision) = fail_closed_decision(&request.route_context, failure) else {
        return;
    };
    let application = FailureApplication {
        diagnostic_id: report.diagnostic_id.clone(),
        decision: decision.clone(),
    };
    if matches!(
        bounded(deadline, backend.apply_failure_decision(application)).await,
        Ok(())
    ) {
        report.degradation = Some(decision.runtime_degradation());
        report.failure_decisions.push(decision);
    }
}

fn fail_closed_decision(
    route: &TrafficFailureRouteContext,
    failure: TrafficFailureKind,
) -> Result<TrafficFailureDecision, fuckipv6_common::domain::DomainError> {
    TrafficFailurePolicy::fail_closed().decide(route, failure)
}

async fn bounded<T>(
    deadline: Instant,
    future: impl Future<Output = Result<T, RecoveryOperationError>>,
) -> Result<T, BoundedError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(BoundedError::TimedOut);
    }
    match timeout(remaining, future).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(_)) => Err(BoundedError::Failed),
        Err(_) => Err(BoundedError::TimedOut),
    }
}

enum BoundedError {
    Failed,
    TimedOut,
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn next_diagnostic_id(started_at_unix_ms: u64) -> String {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!("diag-recovery-{started_at_unix_ms}-{sequence:08x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuckipv6_common::domain::{
        NetworkId, TrafficAction, TrafficDefaultConfirmedBy, TrafficFailureDecisionReason,
        TrafficFailureOutcome,
    };
    use std::collections::VecDeque;
    use std::sync::Mutex;

    struct FakeBackend {
        state: Mutex<FakeState>,
    }

    struct FakeState {
        snapshots: VecDeque<Result<RecoveryResourceSnapshot, RecoveryOperationError>>,
        repair: Result<RecoveryRepairReport, RecoveryOperationError>,
        revoke: Result<(), RecoveryOperationError>,
        failure_apply: Result<(), RecoveryOperationError>,
        probe: Result<RecoveryProbeObservation, RecoveryOperationError>,
        hang_snapshot: bool,
        calls: Vec<String>,
        applications: Vec<FailureApplication>,
    }

    impl FakeBackend {
        fn new(before: RecoveryResourceSnapshot, after: RecoveryResourceSnapshot) -> Self {
            Self {
                state: Mutex::new(FakeState {
                    snapshots: VecDeque::from([Ok(before), Ok(after)]),
                    repair: Ok(RecoveryRepairReport::default()),
                    revoke: Ok(()),
                    failure_apply: Ok(()),
                    probe: Ok(RecoveryProbeObservation {
                        state: RecoveryProbeState::Passed,
                        failed_stage: None,
                        error_code: None,
                    }),
                    hang_snapshot: false,
                    calls: Vec::new(),
                    applications: Vec::new(),
                }),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.state.lock().unwrap().calls.clone()
        }

        fn applications(&self) -> Vec<FailureApplication> {
            self.state.lock().unwrap().applications.clone()
        }
    }

    impl RecoveryBackend for FakeBackend {
        fn snapshot(&mut self) -> RecoveryFuture<'_, RecoveryResourceSnapshot> {
            Box::pin(async move {
                let (hang, result) = {
                    let mut state = self.state.lock().unwrap();
                    state.calls.push("snapshot".into());
                    (
                        state.hang_snapshot,
                        state.snapshots.pop_front().unwrap_or_else(|| {
                            Err(RecoveryOperationError::new("snapshot.exhausted"))
                        }),
                    )
                };
                if hang {
                    std::future::pending::<()>().await;
                }
                result
            })
        }

        fn reconcile(
            &mut self,
            trigger: RecoveryTrigger,
        ) -> RecoveryFuture<'_, RecoveryRepairReport> {
            Box::pin(async move {
                let mut state = self.state.lock().unwrap();
                state.calls.push(format!("reconcile:{trigger:?}"));
                state.repair.clone()
            })
        }

        fn revoke_path(&mut self, path: InvalidatedPath) -> RecoveryFuture<'_, ()> {
            Box::pin(async move {
                let mut state = self.state.lock().unwrap();
                state.calls.push(format!("revoke:{path:?}"));
                state.revoke.clone()
            })
        }

        fn apply_failure_decision(
            &mut self,
            application: FailureApplication,
        ) -> RecoveryFuture<'_, ()> {
            Box::pin(async move {
                let mut state = self.state.lock().unwrap();
                state
                    .calls
                    .push(format!("apply:{:?}", application.decision.outcome));
                state.applications.push(application);
                state.failure_apply.clone()
            })
        }

        fn probe(
            &mut self,
            target: RecoveryProbeTarget,
        ) -> RecoveryFuture<'_, RecoveryProbeObservation> {
            Box::pin(async move {
                let mut state = self.state.lock().unwrap();
                state.calls.push(format!("probe:{}", target.key()));
                state.probe.clone()
            })
        }
    }

    fn healthy_snapshot() -> RecoveryResourceSnapshot {
        RecoveryResourceSnapshot {
            tun: TunRecoverySnapshot {
                state: RecoveryResourceState::Healthy,
                duplicate_interfaces: 0,
                residual_interfaces: 0,
            },
            routes: RouteRecoverySnapshot {
                expected_routes: 4,
                installed_routes: 4,
                missing_routes: 0,
                duplicate_routes: 0,
                residual_routes: 0,
                stale_interface_routes: 0,
            },
            dns: DnsRecoverySnapshot {
                mode: DnsRecoveryMode::Doh,
                state: RecoveryResourceState::Healthy,
                authorized_resolvers: 1,
                unsecured_resolvers: 0,
                insecure_fallback_detected: false,
            },
            exit_path: PathRecoveryState::Connected,
            relay_path: PathRecoveryState::Connected,
        }
    }

    fn route_context() -> TrafficFailureRouteContext {
        TrafficFailureRouteContext {
            requested_action: TrafficAction::Network(NetworkId::from_public_key(
                b"recovery-network",
            )),
            fixed_exit: None,
            fixed_network: None,
        }
    }

    fn request(trigger: RecoveryTrigger) -> RecoveryRequest {
        RecoveryRequest::new(
            trigger,
            route_context(),
            TrafficFailurePolicy::fail_closed(),
        )
        .with_probes(vec![RecoveryProbeTarget::Controlled("test-target".into())])
        .with_hard_timeout_ms(1_000)
    }

    fn direct_fallback_policy() -> TrafficFailurePolicy {
        TrafficFailurePolicy::with_direct_fallback(
            [TrafficFailureKind::ExitUnavailable],
            TrafficDefaultConfirmedBy::Owner,
            1_700_000_000_000,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn sleep_wake_verifies_tun_routes_dns_and_reachability() {
        let mut backend = FakeBackend::new(healthy_snapshot(), healthy_snapshot());
        let request = request(RecoveryTrigger::SleepWake).with_probes(default_recovery_probes());

        let report = RecoveryCoordinator.recover(&mut backend, request).await;

        assert_eq!(report.status, RecoveryStatus::Recovered);
        assert!(report.is_success());
        assert!(report.before.is_some());
        assert!(report.after.is_some());
        assert_eq!(report.probes.len(), 3);
        assert_eq!(
            report
                .probes
                .iter()
                .map(|probe| probe.target.key())
                .collect::<Vec<_>>(),
            vec!["google", "youtube", "controlled:product-control-target"]
        );
        assert!(report.diagnostic_id.starts_with("diag-recovery-"));
        assert!(report.completed_at_unix_ms >= report.started_at_unix_ms);
        assert_eq!(
            backend.calls(),
            vec![
                "snapshot",
                "reconcile:SleepWake",
                "snapshot",
                "probe:google",
                "probe:youtube",
                "probe:controlled:product-control-target",
            ]
        );
    }

    #[tokio::test]
    async fn network_switch_reevaluates_policy_and_dns_before_verification() {
        let mut before = healthy_snapshot();
        before.dns.mode = DnsRecoveryMode::SystemResolver;
        let mut after = healthy_snapshot();
        after.dns.mode = DnsRecoveryMode::Doh;
        let mut backend = FakeBackend::new(before, after);
        let mut request = request(RecoveryTrigger::NetworkChanged);
        request.probes = vec![RecoveryProbeTarget::Controlled("switch-target".into())];

        let report = RecoveryCoordinator.recover(&mut backend, request).await;

        assert_eq!(report.status, RecoveryStatus::Recovered);
        assert_eq!(
            backend.calls(),
            vec![
                "snapshot",
                "reconcile:NetworkChanged",
                "snapshot",
                "probe:controlled:switch-target",
            ]
        );
        assert_eq!(report.after.unwrap().dns.mode, DnsRecoveryMode::Doh);
    }

    #[tokio::test]
    async fn exit_disconnect_revokes_stale_path_and_applies_direct_fallback_policy() {
        let mut before = healthy_snapshot();
        before.exit_path = PathRecoveryState::Disconnected;
        let mut backend = FakeBackend::new(before, healthy_snapshot());
        let request = RecoveryRequest::new(
            RecoveryTrigger::ExitDisconnected,
            route_context(),
            direct_fallback_policy(),
        )
        .with_probes(vec![RecoveryProbeTarget::Google])
        .with_hard_timeout_ms(1_000);

        let report = RecoveryCoordinator.recover(&mut backend, request).await;

        assert_eq!(report.status, RecoveryStatus::DegradedDirectFallback);
        assert_eq!(report.revoked_paths, vec![InvalidatedPath::Exit]);
        let decision = report.failure_decisions.first().unwrap();
        assert_eq!(decision.failure, TrafficFailureKind::ExitUnavailable);
        assert_eq!(decision.outcome, TrafficFailureOutcome::DirectFallback);
        assert_eq!(
            decision.reason,
            TrafficFailureDecisionReason::ExplicitDirectFallback
        );
        assert_eq!(
            backend.applications()[0].diagnostic_id,
            report.diagnostic_id
        );
        assert!(backend.calls().contains(&"revoke:Exit".to_string()));
    }

    #[tokio::test]
    async fn relay_disconnect_uses_network_failure_and_fail_closed_policy() {
        let mut before = healthy_snapshot();
        before.relay_path = PathRecoveryState::Disconnected;
        let mut backend = FakeBackend::new(before, healthy_snapshot());
        let report = RecoveryCoordinator
            .recover(&mut backend, request(RecoveryTrigger::RelayDisconnected))
            .await;

        assert_eq!(report.status, RecoveryStatus::DegradedFailClosed);
        assert_eq!(report.revoked_paths, vec![InvalidatedPath::Relay]);
        let decision = report.failure_decisions.first().unwrap();
        assert_eq!(
            decision.failure,
            TrafficFailureKind::NetworkSelectionUnavailable
        );
        assert_eq!(decision.outcome, TrafficFailureOutcome::FailClosed);
        assert!(backend.calls().contains(&"revoke:Relay".to_string()));
    }

    #[tokio::test]
    async fn reconciliation_removes_residue_duplicates_and_stale_interface_routes() {
        let mut before = healthy_snapshot();
        before.routes.missing_routes = 2;
        before.routes.installed_routes = 2;
        before.routes.duplicate_routes = 1;
        before.routes.residual_routes = 3;
        before.routes.stale_interface_routes = 1;
        let mut after = healthy_snapshot();
        after.routes.installed_routes = 7;
        let mut backend = FakeBackend::new(before, after);
        let repair = RecoveryRepairReport {
            routes_restored: 2,
            residual_routes_removed: 3,
            duplicate_routes_removed: 1,
            stale_interface_routes_removed: 1,
            dns_reconfigured: true,
            ..RecoveryRepairReport::default()
        };
        backend.state.lock().unwrap().repair = Ok(repair.clone());

        let report = RecoveryCoordinator
            .recover(&mut backend, request(RecoveryTrigger::NetworkChanged))
            .await;

        assert_eq!(report.status, RecoveryStatus::Recovered);
        assert_eq!(report.repair, repair);
    }

    #[tokio::test]
    async fn insecure_dns_fallback_is_never_silent_and_forces_fail_closed() {
        let mut before = healthy_snapshot();
        before.dns.state = RecoveryResourceState::Failed;
        let mut backend = FakeBackend::new(before, healthy_snapshot());
        backend.state.lock().unwrap().repair = Ok(RecoveryRepairReport {
            dns_reconfigured: true,
            used_insecure_dns_fallback: true,
            ..RecoveryRepairReport::default()
        });
        let mut direct = direct_fallback_policy();
        direct.direct_fallback.as_mut().unwrap().allowed_failures =
            [TrafficFailureKind::ConnectionTimeout]
                .into_iter()
                .collect();
        let request =
            RecoveryRequest::new(RecoveryTrigger::NetworkChanged, route_context(), direct)
                .with_probes(vec![RecoveryProbeTarget::Google])
                .with_hard_timeout_ms(1_000);

        let report = RecoveryCoordinator.recover(&mut backend, request).await;

        assert_eq!(report.status, RecoveryStatus::Failed);
        assert_eq!(
            report.error_code.as_deref(),
            Some("recovery.dns_unsafe_fallback_forbidden")
        );
        assert_eq!(report.probes.len(), 0);
        let decision = report.failure_decisions.first().unwrap();
        assert_eq!(decision.outcome, TrafficFailureOutcome::FailClosed);
        assert_eq!(decision.failure, TrafficFailureKind::DnsPollutionDetected);
        assert!(!backend
            .calls()
            .iter()
            .any(|call| call.starts_with("probe:")));
    }

    #[tokio::test]
    async fn exhausted_hard_deadline_returns_timeout_without_unbounded_blocking() {
        let mut backend = FakeBackend::new(healthy_snapshot(), healthy_snapshot());
        backend.state.lock().unwrap().hang_snapshot = true;
        let started = Instant::now();

        let report = RecoveryCoordinator
            .recover(
                &mut backend,
                request(RecoveryTrigger::SleepWake).with_hard_timeout_ms(20),
            )
            .await;

        assert_eq!(report.status, RecoveryStatus::TimedOut);
        assert!(report.timed_out);
        assert!(report.fail_closed_required);
        assert!(report.before.is_none());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn failed_path_revocation_overrides_direct_fallback_with_fail_closed_decision() {
        let mut before = healthy_snapshot();
        before.exit_path = PathRecoveryState::Disconnected;
        let mut backend = FakeBackend::new(before, healthy_snapshot());
        backend.state.lock().unwrap().revoke = Err(RecoveryOperationError::new("revoke.failed"));
        let request = RecoveryRequest::new(
            RecoveryTrigger::ExitDisconnected,
            route_context(),
            direct_fallback_policy(),
        )
        .with_probes(vec![RecoveryProbeTarget::Google])
        .with_hard_timeout_ms(1_000);

        let report = RecoveryCoordinator.recover(&mut backend, request).await;

        assert_eq!(report.status, RecoveryStatus::Failed);
        assert_eq!(
            report.error_code.as_deref(),
            Some("recovery.path_revoke_failed")
        );
        let decision = report.failure_decisions.first().unwrap();
        assert_eq!(decision.outcome, TrafficFailureOutcome::FailClosed);
        assert!(decision.direct_fallback.is_none());
        assert!(report.fail_closed_required);
    }

    #[tokio::test]
    async fn reachability_probe_failure_is_reported_without_claiming_recovery() {
        let mut backend = FakeBackend::new(healthy_snapshot(), healthy_snapshot());
        backend.state.lock().unwrap().probe = Ok(RecoveryProbeObservation {
            state: RecoveryProbeState::Failed,
            failed_stage: Some(RecoveryProbeStage::Tls),
            error_code: Some("probe.tls_failed".into()),
        });
        let request =
            request(RecoveryTrigger::Manual).with_probes(vec![RecoveryProbeTarget::YouTube]);

        let report = RecoveryCoordinator.recover(&mut backend, request).await;

        assert_eq!(report.status, RecoveryStatus::Failed);
        assert_eq!(
            report.error_code.as_deref(),
            Some("recovery.reachability_failed")
        );
        assert_eq!(report.probes[0].state, RecoveryProbeState::Failed);
        assert_eq!(report.probes[0].failed_stage, Some(RecoveryProbeStage::Tls));
        assert!(!report.is_success());
    }

    #[tokio::test]
    async fn unhealthy_post_snapshot_applies_fail_closed_safety_decision() {
        let mut after = healthy_snapshot();
        after.routes.residual_routes = 1;
        let mut backend = FakeBackend::new(healthy_snapshot(), after);

        let report = RecoveryCoordinator
            .recover(&mut backend, request(RecoveryTrigger::SleepWake))
            .await;

        assert_eq!(report.status, RecoveryStatus::Failed);
        assert_eq!(
            report.error_code.as_deref(),
            Some("recovery.routes_unhealthy")
        );
        let decision = report.failure_decisions.first().unwrap();
        assert_eq!(decision.outcome, TrafficFailureOutcome::FailClosed);
        assert_eq!(
            report.degradation.as_ref().unwrap().state,
            TrafficRuntimeDegradationState::DegradedFailClosed
        );
    }
}
