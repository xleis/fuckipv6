//! Fresh, layered Exit health evidence.
//!
//! A configured Exit is not healthy by declaration. Candidate selection may
//! only use fresh evidence for process, listener, network, DNS and Internet
//! reachability. Missing or stale evidence remains unknown and therefore
//! ineligible for the default candidate set.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::exit_contribution::{
    ExitAvailabilitySource, ExitAvailabilityState, ExitContributionAvailability,
};
use super::{DomainError, DomainResult};

pub const EXIT_HEALTH_DEFAULT_MAX_AGE_MS: u64 = 90_000;
pub const EXIT_HEALTH_MAX_FUTURE_SKEW_MS: u64 = 5_000;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ExitHealthLayer {
    Process,
    Listening,
    Network,
    Dns,
    InternetReachability,
}

impl ExitHealthLayer {
    pub const ALL: [Self; 5] = [
        Self::Process,
        Self::Listening,
        Self::Network,
        Self::Dns,
        Self::InternetReachability,
    ];

    pub const fn key(self) -> &'static str {
        match self {
            Self::Process => "process",
            Self::Listening => "listening",
            Self::Network => "network",
            Self::Dns => "dns",
            Self::InternetReachability => "internet_reachability",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitHealthEvidenceState {
    Passed,
    Degraded,
    Failed,
    Unsupported,
    NotTested,
    Unknown,
}

impl ExitHealthEvidenceState {
    pub const fn passed(self) -> bool {
        matches!(self, Self::Passed)
    }

    pub const fn blocks_default_candidate(self) -> bool {
        !self.passed()
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ExitHealthSource {
    LocalProcess,
    LocalSocket,
    LocalNetwork,
    LocalDns,
    ControlledProbe,
    CoordinatorHealth,
    RemotePeerReport,
}

impl ExitHealthSource {
    const fn availability_source(self) -> ExitAvailabilitySource {
        match self {
            Self::CoordinatorHealth => ExitAvailabilitySource::CoordinatorHealth,
            Self::RemotePeerReport => ExitAvailabilitySource::RemotePeerReport,
            Self::LocalProcess
            | Self::LocalSocket
            | Self::LocalNetwork
            | Self::LocalDns
            | Self::ControlledProbe => ExitAvailabilitySource::LocalRuntime,
        }
    }
}

/// Stable, non-identifying probe targets. A deployment-controlled target may be
/// named by an opaque ID, but host names, resolver addresses and member
/// identities are never stored in this health contract.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum ExitHealthTarget {
    DnsResolver,
    Ipv4Internet,
    Ipv6Internet,
    TcpInternet,
    UdpInternet,
    QuicInternet,
    Controlled(String),
}

impl ExitHealthTarget {
    pub fn key(&self) -> String {
        match self {
            Self::DnsResolver => "dns_resolver".into(),
            Self::Ipv4Internet => "ipv4_internet".into(),
            Self::Ipv6Internet => "ipv6_internet".into(),
            Self::TcpInternet => "tcp_internet".into(),
            Self::UdpInternet => "udp_internet".into(),
            Self::QuicInternet => "quic_internet".into(),
            Self::Controlled(id) => format!("controlled:{id}"),
        }
    }

    fn validate(&self) -> bool {
        match self {
            Self::Controlled(id) => {
                !id.is_empty()
                    && id.len() <= 128
                    && id.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
                    })
            }
            _ => true,
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ExitHealthProtocol {
    Dns,
    Tcp,
    Udp,
    Quic,
    Ipv4,
    Ipv6,
}

/// One sanitized observation. `error_code` is a stable machine token and must
/// not contain raw command output, addresses or member identity.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExitHealthEvidence {
    pub layer: ExitHealthLayer,
    pub state: ExitHealthEvidenceState,
    pub source: ExitHealthSource,
    pub observed_at_unix_ms: u64,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ExitHealthTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<ExitHealthProtocol>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

fn is_stable_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

impl ExitHealthEvidence {
    pub fn is_fresh(&self, now_unix_ms: u64, max_age_ms: u64) -> bool {
        self.observed_at_unix_ms <= now_unix_ms.saturating_add(EXIT_HEALTH_MAX_FUTURE_SKEW_MS)
            && now_unix_ms <= self.observed_at_unix_ms.saturating_add(max_age_ms)
    }

    pub fn validate(&self) -> DomainResult<()> {
        if self.observed_at_unix_ms == 0 {
            return Err(DomainError::invalid_value(
                "Exit health evidence requires a positive timestamp",
            ));
        }
        if let Some(target) = &self.target {
            if !target.validate() {
                return Err(DomainError::invalid_value(
                    "Exit health target identifier is invalid",
                ));
            }
        }
        if self
            .error_code
            .as_ref()
            .is_some_and(|code| !is_stable_token(code))
        {
            return Err(DomainError::invalid_value(
                "Exit health error code is invalid",
            ));
        }
        Ok(())
    }
}

/// Bounded probing policy. These values are intentionally small because health
/// checks run on rotating background schedules and must not become a traffic
/// amplifier against an Exit or a third-party target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExitHealthProbePolicy {
    pub timeout_ms: u64,
    pub max_concurrency: usize,
    pub max_attempts: u8,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
    /// Deterministic jitter percentage in `[0, 500]` per mille.
    pub jitter_permille: u16,
}

impl Default for ExitHealthProbePolicy {
    fn default() -> Self {
        Self {
            timeout_ms: 1_500,
            max_concurrency: 3,
            max_attempts: 3,
            initial_backoff_ms: 250,
            max_backoff_ms: 4_000,
            jitter_permille: 200,
        }
    }
}

impl ExitHealthProbePolicy {
    pub fn validate(&self) -> DomainResult<()> {
        if self.timeout_ms == 0 || self.timeout_ms > 5_000 {
            return Err(DomainError::invalid_value(
                "Exit health probe timeout must be between 1 and 5000 ms",
            ));
        }
        if self.max_concurrency == 0 || self.max_concurrency > 8 {
            return Err(DomainError::invalid_value(
                "Exit health probe concurrency must be between 1 and 8",
            ));
        }
        if self.max_attempts == 0 || self.max_attempts > 5 {
            return Err(DomainError::invalid_value(
                "Exit health probe attempts must be between 1 and 5",
            ));
        }
        if self.initial_backoff_ms == 0
            || self.initial_backoff_ms > self.max_backoff_ms
            || self.max_backoff_ms > 30_000
        {
            return Err(DomainError::invalid_value(
                "Exit health probe backoff bounds are invalid",
            ));
        }
        if self.jitter_permille > 500 {
            return Err(DomainError::invalid_value(
                "Exit health probe jitter must not exceed 50 percent",
            ));
        }
        Ok(())
    }

    /// Delay before the next attempt. `attempt` starts at one. The deterministic
    /// component is exponential; the bounded jitter prevents synchronized
    /// probes after a Coordinator or process restart.
    pub fn retry_delay_ms(&self, attempt: u8, jitter_seed: u64) -> DomainResult<u64> {
        self.validate()?;
        if attempt == 0 || attempt >= self.max_attempts {
            return Ok(0);
        }
        let factor = 1u64 << u32::from(attempt.saturating_sub(1));
        let exponential = self
            .initial_backoff_ms
            .saturating_mul(factor)
            .min(self.max_backoff_ms);
        if self.jitter_permille == 0 {
            return Ok(exponential);
        }
        let jitter_cap = exponential.saturating_mul(u64::from(self.jitter_permille)) / 1_000;
        let jitter = jitter_seed % jitter_cap.saturating_add(1);
        Ok(exponential
            .saturating_add(jitter)
            .min(self.max_backoff_ms + jitter_cap))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitHealthRetestPhase {
    Idle,
    Requested,
    Running,
    Cancelled,
    Completed,
    Failed,
}

impl ExitHealthRetestPhase {
    pub const fn key(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Requested => "requested",
            Self::Running => "running",
            Self::Cancelled => "cancelled",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Cancelled | Self::Completed | Self::Failed)
    }

    pub const fn can_transition_to(self, target: Self) -> bool {
        matches!(
            (self, target),
            (Self::Idle, Self::Requested)
                | (Self::Requested, Self::Running | Self::Cancelled)
                | (
                    Self::Running,
                    Self::Cancelled | Self::Completed | Self::Failed
                )
        )
    }
}

/// Explicit retest lifecycle. Cancellation is terminal and can never be
/// rewritten as a successful completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExitHealthRetest {
    pub phase: ExitHealthRetestPhase,
    pub requested_at_unix_ms: u64,
    pub started_at_unix_ms: Option<u64>,
    pub finished_at_unix_ms: Option<u64>,
}

impl Default for ExitHealthRetest {
    fn default() -> Self {
        Self {
            phase: ExitHealthRetestPhase::Idle,
            requested_at_unix_ms: 0,
            started_at_unix_ms: None,
            finished_at_unix_ms: None,
        }
    }
}

impl ExitHealthRetest {
    pub fn request(now_unix_ms: u64) -> DomainResult<Self> {
        let mut retest = Self::default();
        retest.transition(ExitHealthRetestPhase::Requested, now_unix_ms)?;
        retest.requested_at_unix_ms = now_unix_ms;
        Ok(retest)
    }

    pub fn transition(
        &mut self,
        target: ExitHealthRetestPhase,
        now_unix_ms: u64,
    ) -> DomainResult<()> {
        if now_unix_ms == 0 || !self.phase.can_transition_to(target) {
            return Err(DomainError::invalid_transition(
                "Exit health retest transition is not allowed",
            ));
        }
        if target == ExitHealthRetestPhase::Running {
            self.started_at_unix_ms = Some(now_unix_ms);
        }
        if target.is_terminal() {
            self.finished_at_unix_ms = Some(now_unix_ms);
        }
        self.phase = target;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExitHealthSnapshot {
    #[serde(default)]
    pub evidence: Vec<ExitHealthEvidence>,
    #[serde(default = "default_exit_health_max_age_ms")]
    pub max_age_ms: u64,
    #[serde(default)]
    pub retest: ExitHealthRetest,
}

const fn default_exit_health_max_age_ms() -> u64 {
    EXIT_HEALTH_DEFAULT_MAX_AGE_MS
}

impl Default for ExitHealthSnapshot {
    fn default() -> Self {
        Self {
            evidence: Vec::new(),
            max_age_ms: EXIT_HEALTH_DEFAULT_MAX_AGE_MS,
            retest: ExitHealthRetest::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitHealthState {
    Unknown,
    Available,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExitHealthAssessment {
    pub state: ExitHealthState,
    pub default_candidate: bool,
    pub reason_code: String,
    pub affected_layers: Vec<ExitHealthLayer>,
    pub affected_targets: Vec<ExitHealthTarget>,
    pub affected_protocols: Vec<ExitHealthProtocol>,
    pub fresh_evidence_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ExitHealthSource>,
}

impl Default for ExitHealthAssessment {
    fn default() -> Self {
        Self {
            state: ExitHealthState::Unknown,
            default_candidate: false,
            reason_code: "exit.health.not_observed".into(),
            affected_layers: ExitHealthLayer::ALL.to_vec(),
            affected_targets: Vec::new(),
            affected_protocols: Vec::new(),
            fresh_evidence_count: 0,
            observed_at_unix_ms: None,
            source: None,
        }
    }
}

impl ExitHealthSnapshot {
    pub fn validate(&self) -> DomainResult<()> {
        if self.max_age_ms == 0 || self.max_age_ms > 10 * 60 * 1000 {
            return Err(DomainError::invalid_value(
                "Exit health evidence max age must be between 1 ms and 10 minutes",
            ));
        }
        for evidence in &self.evidence {
            evidence.validate()?;
        }
        Ok(())
    }

    /// Assess health using only the newest fresh observation for each layer.
    /// Stale failures and stale successes both remain unknown: neither may
    /// dominate a newer observation or silently become healthy.
    pub fn assess(&self, now_unix_ms: u64) -> ExitHealthAssessment {
        let mut latest = std::collections::BTreeMap::new();
        let mut saw_any_evidence = false;
        for evidence in &self.evidence {
            saw_any_evidence = true;
            if !evidence.is_fresh(now_unix_ms, self.max_age_ms) {
                continue;
            }
            let key = (evidence.layer, evidence.target.clone(), evidence.protocol);
            latest
                .entry(key)
                .and_modify(|current: &mut &ExitHealthEvidence| {
                    if evidence.observed_at_unix_ms >= current.observed_at_unix_ms {
                        *current = evidence;
                    }
                })
                .or_insert(evidence);
        }

        let observed_layers = latest
            .keys()
            .map(|(layer, _, _)| *layer)
            .collect::<BTreeSet<_>>();
        let missing_layers = ExitHealthLayer::ALL
            .into_iter()
            .filter(|layer| !observed_layers.contains(layer))
            .collect::<Vec<_>>();
        let fresh_evidence_count = latest.len();
        let observed_at_unix_ms = latest
            .values()
            .map(|evidence| evidence.observed_at_unix_ms)
            .max();
        let mut sources = latest
            .values()
            .map(|evidence| evidence.source)
            .collect::<BTreeSet<_>>();
        let source = (sources.len() == 1).then(|| sources.pop_first()).flatten();

        if !missing_layers.is_empty() {
            return ExitHealthAssessment {
                state: ExitHealthState::Unknown,
                default_candidate: false,
                reason_code: if saw_any_evidence {
                    "exit.health.stale_or_incomplete".into()
                } else {
                    "exit.health.not_observed".into()
                },
                affected_layers: missing_layers,
                affected_targets: Vec::new(),
                affected_protocols: Vec::new(),
                fresh_evidence_count,
                observed_at_unix_ms,
                source,
            };
        }

        let affected = latest
            .values()
            .filter(|evidence| evidence.state.blocks_default_candidate())
            .copied()
            .collect::<Vec<_>>();
        if affected.is_empty() {
            return ExitHealthAssessment {
                state: ExitHealthState::Available,
                default_candidate: true,
                reason_code: "exit.health.available".into(),
                affected_layers: Vec::new(),
                affected_targets: Vec::new(),
                affected_protocols: Vec::new(),
                fresh_evidence_count,
                observed_at_unix_ms,
                source,
            };
        }

        let base_failed = affected.iter().any(|evidence| {
            matches!(
                evidence.layer,
                ExitHealthLayer::Process | ExitHealthLayer::Listening | ExitHealthLayer::Network
            ) && matches!(
                evidence.state,
                ExitHealthEvidenceState::Failed | ExitHealthEvidenceState::Unsupported
            )
        });
        let unknown = affected.iter().any(|evidence| {
            matches!(
                evidence.state,
                ExitHealthEvidenceState::Unknown | ExitHealthEvidenceState::NotTested
            )
        });
        let state = if unknown {
            ExitHealthState::Unknown
        } else if base_failed {
            ExitHealthState::Unavailable
        } else {
            ExitHealthState::Degraded
        };
        let affected_layers = affected
            .iter()
            .map(|evidence| evidence.layer)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let affected_targets = affected
            .iter()
            .filter_map(|evidence| evidence.target.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let affected_protocols = affected
            .iter()
            .filter_map(|evidence| evidence.protocol)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();

        ExitHealthAssessment {
            state,
            default_candidate: false,
            reason_code: match state {
                ExitHealthState::Unknown => "exit.health.incomplete".into(),
                ExitHealthState::Degraded => "exit.health.partially_available".into(),
                ExitHealthState::Unavailable => "exit.health.unavailable".into(),
                ExitHealthState::Available => unreachable!(),
            },
            affected_layers,
            affected_targets,
            affected_protocols,
            fresh_evidence_count,
            observed_at_unix_ms,
            source,
        }
    }

    /// Project the same evidence into the Exit-selection availability contract.
    /// A declaration can never override failed, unknown or stale evidence.
    pub fn availability(
        &self,
        now_unix_ms: u64,
        declared_available: Option<bool>,
    ) -> ExitContributionAvailability {
        let assessment = self.assess(now_unix_ms);
        let source = assessment
            .source
            .map(ExitHealthSource::availability_source)
            .unwrap_or(ExitAvailabilitySource::NotObserved);
        let observed_at_unix_ms = assessment.observed_at_unix_ms;
        match assessment.state {
            ExitHealthState::Available => ExitContributionAvailability {
                state: ExitAvailabilityState::Available,
                source,
                observed_at_unix_ms,
                declared_available,
                reason_code: None,
            },
            ExitHealthState::Degraded => ExitContributionAvailability {
                state: ExitAvailabilityState::Degraded,
                source,
                observed_at_unix_ms,
                declared_available,
                reason_code: Some(assessment.reason_code),
            },
            ExitHealthState::Unavailable => ExitContributionAvailability {
                state: ExitAvailabilityState::Unavailable,
                source,
                observed_at_unix_ms,
                declared_available,
                reason_code: Some(assessment.reason_code),
            },
            ExitHealthState::Unknown => ExitContributionAvailability {
                state: ExitAvailabilityState::Unknown,
                source,
                observed_at_unix_ms,
                declared_available,
                reason_code: Some(assessment.reason_code),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_700_000_100_000;

    fn evidence(
        layer: ExitHealthLayer,
        state: ExitHealthEvidenceState,
        observed_at_unix_ms: u64,
    ) -> ExitHealthEvidence {
        ExitHealthEvidence {
            layer,
            state,
            source: ExitHealthSource::ControlledProbe,
            observed_at_unix_ms,
            duration_ms: 5,
            target: None,
            protocol: None,
            error_code: None,
        }
    }

    fn healthy_snapshot() -> ExitHealthSnapshot {
        ExitHealthSnapshot {
            evidence: ExitHealthLayer::ALL
                .into_iter()
                .map(|layer| evidence(layer, ExitHealthEvidenceState::Passed, NOW - 1_000))
                .collect(),
            ..ExitHealthSnapshot::default()
        }
    }

    #[test]
    fn only_fresh_passed_evidence_for_every_layer_is_available() {
        let assessment = healthy_snapshot().assess(NOW);
        assert_eq!(assessment.state, ExitHealthState::Available);
        assert!(assessment.default_candidate);
        assert_eq!(assessment.fresh_evidence_count, 5);
    }

    #[test]
    fn missing_or_stale_evidence_remains_unknown_and_ineligible() {
        let mut snapshot = healthy_snapshot();
        snapshot
            .evidence
            .retain(|item| item.layer != ExitHealthLayer::Dns);
        let missing = snapshot.assess(NOW);
        assert_eq!(missing.state, ExitHealthState::Unknown);
        assert!(!missing.default_candidate);
        assert_eq!(missing.affected_layers, vec![ExitHealthLayer::Dns]);

        let mut stale = healthy_snapshot();
        stale.max_age_ms = 1_000;
        for item in &mut stale.evidence {
            item.observed_at_unix_ms = NOW - 1_001;
        }
        let stale_assessment = stale.assess(NOW);
        assert_eq!(stale_assessment.state, ExitHealthState::Unknown);
        assert!(!stale_assessment.default_candidate);
        assert_eq!(
            stale_assessment.reason_code,
            "exit.health.stale_or_incomplete"
        );
    }

    #[test]
    fn partial_failure_reports_affected_target_and_protocol() {
        let mut snapshot = healthy_snapshot();
        let dns = snapshot
            .evidence
            .iter_mut()
            .find(|item| item.layer == ExitHealthLayer::Dns)
            .unwrap();
        dns.state = ExitHealthEvidenceState::Degraded;
        dns.target = Some(ExitHealthTarget::DnsResolver);
        dns.protocol = Some(ExitHealthProtocol::Dns);
        let assessment = snapshot.assess(NOW);
        assert_eq!(assessment.state, ExitHealthState::Degraded);
        assert!(!assessment.default_candidate);
        assert_eq!(assessment.affected_layers, vec![ExitHealthLayer::Dns]);
        assert_eq!(
            assessment.affected_targets,
            vec![ExitHealthTarget::DnsResolver]
        );
        assert_eq!(assessment.affected_protocols, vec![ExitHealthProtocol::Dns]);
    }

    #[test]
    fn newer_recovery_evidence_replaces_false_negative_and_false_positive() {
        let mut snapshot = healthy_snapshot();
        snapshot.evidence.push(evidence(
            ExitHealthLayer::Process,
            ExitHealthEvidenceState::Failed,
            NOW - 2_000,
        ));
        assert_eq!(snapshot.assess(NOW).state, ExitHealthState::Available);

        snapshot.evidence.push(ExitHealthEvidence {
            layer: ExitHealthLayer::Process,
            state: ExitHealthEvidenceState::Failed,
            source: ExitHealthSource::LocalProcess,
            observed_at_unix_ms: NOW - 500,
            duration_ms: 1,
            target: None,
            protocol: None,
            error_code: Some("exit.health.process_exited".into()),
        });
        assert_eq!(snapshot.assess(NOW).state, ExitHealthState::Unavailable);
    }

    #[test]
    fn evidence_rejects_non_opaque_targets_and_raw_error_output() {
        let mut item = evidence(
            ExitHealthLayer::InternetReachability,
            ExitHealthEvidenceState::Failed,
            NOW,
        );
        item.target = Some(ExitHealthTarget::Controlled("member@example.com".into()));
        assert_eq!(item.validate().unwrap_err().code(), "domain.invalid_value");

        item.target = Some(ExitHealthTarget::Controlled("probe-target-7".into()));
        item.error_code = Some("curl failed talking to member alice".into());
        assert_eq!(item.validate().unwrap_err().code(), "domain.invalid_value");

        item.error_code = Some("exit.health.probe_timeout".into());
        assert!(item.validate().is_ok());
    }

    #[test]
    fn availability_projection_never_promotes_unknown_or_stale() {
        let mut snapshot = healthy_snapshot();
        snapshot.evidence.pop();
        let availability = snapshot.availability(NOW, Some(true));
        assert_eq!(availability.state, ExitAvailabilityState::Unknown);
        assert_eq!(availability.declared_available, Some(true));
        assert_eq!(
            availability.reason_code.as_deref(),
            Some("exit.health.stale_or_incomplete")
        );
    }

    #[test]
    fn bounded_probe_policy_uses_exponential_backoff_and_jitter() {
        let policy = ExitHealthProbePolicy::default();
        assert!(policy.validate().is_ok());
        assert_eq!(policy.retry_delay_ms(1, 0).unwrap(), 250);
        assert_eq!(policy.retry_delay_ms(2, 0).unwrap(), 500);
        assert!(policy.retry_delay_ms(2, 99).unwrap() >= 500);

        let mut invalid = policy;
        invalid.max_concurrency = 0;
        assert_eq!(
            invalid.validate().unwrap_err().code(),
            "domain.invalid_value"
        );
        invalid = policy;
        invalid.timeout_ms = 5_001;
        assert_eq!(
            invalid.validate().unwrap_err().code(),
            "domain.invalid_value"
        );
    }

    #[test]
    fn cancelled_retest_is_terminal_and_cannot_report_completion() {
        let mut retest = ExitHealthRetest::request(NOW).unwrap();
        retest
            .transition(ExitHealthRetestPhase::Running, NOW + 1)
            .unwrap();
        retest
            .transition(ExitHealthRetestPhase::Cancelled, NOW + 2)
            .unwrap();
        assert!(retest.phase.is_terminal());
        assert_eq!(retest.finished_at_unix_ms, Some(NOW + 2));
        assert!(retest
            .transition(ExitHealthRetestPhase::Completed, NOW + 3)
            .is_err());
    }
}
