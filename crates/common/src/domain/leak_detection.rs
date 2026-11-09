//! Fail-closed DNS, IPv4 and IPv6 leak evidence.
//!
//! A missing, stale, unsupported or unexecuted check is never proof that
//! traffic is leak-free. The snapshot contains only stable targets, sanitized
//! outcomes and stable error tokens; resolver URLs, addresses, interface names
//! and raw command output remain outside this public contract.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{DomainError, DomainResult};

pub const LEAK_DETECTION_DEFAULT_MAX_AGE_MS: u64 = 90_000;
pub const LEAK_DETECTION_MAX_FUTURE_SKEW_MS: u64 = 5_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeakDetectionPlatform {
    Linux,
    Windows,
    Macos,
    #[default]
    Other,
}

/// DNS path that was actually observed. DoT support is not implied merely
/// because this enum can represent it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeakDetectionDnsMode {
    SystemResolver,
    Doh,
    Dot,
    NetworkSpecified,
    #[default]
    Unmanaged,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum LeakDetectionProtocol {
    Dns,
    Ipv4,
    Ipv6,
}

impl LeakDetectionProtocol {
    pub const fn key(self) -> &'static str {
        match self {
            Self::Dns => "dns",
            Self::Ipv4 => "ipv4",
            Self::Ipv6 => "ipv6",
        }
    }
}

/// Sanitized check target. A controlled service is represented only by its
/// stable role, never by a host name, URL or resolved address.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum LeakDetectionTarget {
    DnsResolver,
    BrowserApplicationDns,
    Ipv4Carrier,
    Ipv4SourceAddress,
    Ipv4DefaultRoute,
    Ipv4ControlledLeakService,
    Ipv4ExternalTarget,
    Ipv6Carrier,
    Ipv6SourceAddress,
    Ipv6DefaultRoute,
    Ipv6ControlledLeakService,
    Ipv6ExternalTarget,
}

impl LeakDetectionTarget {
    pub const fn protocol(self) -> LeakDetectionProtocol {
        match self {
            Self::DnsResolver | Self::BrowserApplicationDns => LeakDetectionProtocol::Dns,
            Self::Ipv4Carrier
            | Self::Ipv4SourceAddress
            | Self::Ipv4DefaultRoute
            | Self::Ipv4ControlledLeakService
            | Self::Ipv4ExternalTarget => LeakDetectionProtocol::Ipv4,
            Self::Ipv6Carrier
            | Self::Ipv6SourceAddress
            | Self::Ipv6DefaultRoute
            | Self::Ipv6ControlledLeakService
            | Self::Ipv6ExternalTarget => LeakDetectionProtocol::Ipv6,
        }
    }

    pub const fn key(self) -> &'static str {
        match self {
            Self::DnsResolver => "dns_resolver",
            Self::BrowserApplicationDns => "browser_application_dns",
            Self::Ipv4Carrier => "ipv4_carrier",
            Self::Ipv4SourceAddress => "ipv4_source_address",
            Self::Ipv4DefaultRoute => "ipv4_default_route",
            Self::Ipv4ControlledLeakService => "ipv4_controlled_leak_service",
            Self::Ipv4ExternalTarget => "ipv4_external_target",
            Self::Ipv6Carrier => "ipv6_carrier",
            Self::Ipv6SourceAddress => "ipv6_source_address",
            Self::Ipv6DefaultRoute => "ipv6_default_route",
            Self::Ipv6ControlledLeakService => "ipv6_controlled_leak_service",
            Self::Ipv6ExternalTarget => "ipv6_external_target",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeakDetectionEvidenceState {
    Passed,
    Failed,
    Unsupported,
    NotTested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeakDetectionSource {
    SystemResolver,
    ManagedDoh,
    LocalRoutingTable,
    LinkCapability,
    ControlledProbe,
    ExternalReachabilityProbe,
    ApplicationBehaviorProbe,
}

/// An isolation action that makes an unsupported path unusable rather than
/// silently allowing it to leave outside the protected route.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum LeakDetectionIsolationAction {
    BlockedUnavailableIpv4,
    BlockedUnavailableIpv6,
    FailClosedAffectedTraffic,
    RetainedProtectedRoute,
}

impl LeakDetectionIsolationAction {
    pub const fn blocks_protocol(self) -> bool {
        matches!(
            self,
            Self::BlockedUnavailableIpv4
                | Self::BlockedUnavailableIpv6
                | Self::FailClosedAffectedTraffic
        )
    }
}

/// One sanitized observation. `error_code` is a stable machine token and must
/// never contain raw stderr, resolver URLs, addresses or member identity.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeakDetectionEvidence {
    pub target: LeakDetectionTarget,
    pub state: LeakDetectionEvidenceState,
    pub source: LeakDetectionSource,
    pub observed_at_unix_ms: u64,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation_action: Option<LeakDetectionIsolationAction>,
}

impl LeakDetectionEvidence {
    pub fn validate(&self) -> DomainResult<()> {
        if self.observed_at_unix_ms == 0 {
            return Err(DomainError::invalid_value(
                "leak detection evidence requires a positive timestamp",
            ));
        }
        if self
            .error_code
            .as_ref()
            .is_some_and(|code| !is_stable_token(code))
        {
            return Err(DomainError::invalid_value(
                "leak detection error code is invalid",
            ));
        }
        if self.state == LeakDetectionEvidenceState::Failed && self.error_code.is_none() {
            return Err(DomainError::invalid_value(
                "failed leak detection evidence requires a stable error code",
            ));
        }
        Ok(())
    }

    fn is_fresh(&self, now_unix_ms: u64, max_age_ms: u64) -> bool {
        self.observed_at_unix_ms <= now_unix_ms.saturating_add(LEAK_DETECTION_MAX_FUTURE_SKEW_MS)
            && now_unix_ms.saturating_sub(self.observed_at_unix_ms) <= max_age_ms
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LeakDetectionEnvironment {
    pub platform: LeakDetectionPlatform,
    pub dns_mode: LeakDetectionDnsMode,
    pub protected_path_active: bool,
    pub ipv4_carrier_available: bool,
    pub ipv6_carrier_available: bool,
}

impl Default for LeakDetectionEnvironment {
    fn default() -> Self {
        Self {
            platform: LeakDetectionPlatform::Other,
            dns_mode: LeakDetectionDnsMode::Unmanaged,
            protected_path_active: false,
            ipv4_carrier_available: false,
            ipv6_carrier_available: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LeakDetectionSnapshot {
    pub generated_at_unix_ms: u64,
    pub max_age_ms: u64,
    pub environment: LeakDetectionEnvironment,
    pub evidence: Vec<LeakDetectionEvidence>,
}

impl Default for LeakDetectionSnapshot {
    fn default() -> Self {
        Self {
            generated_at_unix_ms: 0,
            max_age_ms: LEAK_DETECTION_DEFAULT_MAX_AGE_MS,
            environment: LeakDetectionEnvironment::default(),
            evidence: Vec::new(),
        }
    }
}

impl LeakDetectionSnapshot {
    pub fn validate(&self) -> DomainResult<()> {
        if self.generated_at_unix_ms == 0 {
            return Err(DomainError::invalid_value(
                "leak detection snapshot requires a positive timestamp",
            ));
        }
        if self.max_age_ms == 0 || self.max_age_ms > 300_000 {
            return Err(DomainError::invalid_value(
                "leak detection evidence max age must be between 1 and 300000 ms",
            ));
        }
        let mut targets = BTreeSet::new();
        for evidence in &self.evidence {
            evidence.validate()?;
            if !targets.insert(evidence.target) {
                return Err(DomainError::invalid_value(
                    "leak detection evidence targets must be unique",
                ));
            }
        }
        Ok(())
    }

    pub fn evidence_for(&self, target: LeakDetectionTarget) -> Option<&LeakDetectionEvidence> {
        self.evidence
            .iter()
            .find(|evidence| evidence.target == target)
    }

    /// Compute the server-authoritative conclusion at a specific observation
    /// time. Required checks are deliberately stricter than the evidence list:
    /// a caller cannot turn an empty or partial snapshot into `LeakFree`.
    pub fn assess(&self, now_unix_ms: u64) -> LeakDetectionAssessment {
        if self.validate().is_err() {
            return LeakDetectionAssessment::insufficient(
                "leak_detection.invalid_evidence",
                LeakDetectionProtocol::ALL.into_iter().collect(),
                LeakDetectionTarget::ALL.into_iter().collect(),
            );
        }

        let mut required = Vec::new();
        required.extend(LeakDetectionProtocol::Dns.required_targets(false));
        required.extend(
            LeakDetectionProtocol::Ipv4.required_targets(self.environment.ipv4_carrier_available),
        );
        required.extend(
            LeakDetectionProtocol::Ipv6.required_targets(self.environment.ipv6_carrier_available),
        );
        required.sort_unstable();
        required.dedup();

        let stale_or_future = required.iter().find_map(|target| {
            self.evidence_for(*target)
                .filter(|evidence| !evidence.is_fresh(now_unix_ms, self.max_age_ms))
                .map(|_| *target)
        });
        if let Some(target) = stale_or_future {
            return LeakDetectionAssessment::insufficient(
                "leak_detection.evidence_stale",
                vec![target.protocol()],
                vec![target],
            );
        }

        let failed = required
            .iter()
            .filter_map(|target| {
                self.evidence_for(*target)
                    .filter(|evidence| evidence.state == LeakDetectionEvidenceState::Failed)
                    .map(|_| *target)
            })
            .collect::<Vec<_>>();
        if !failed.is_empty() {
            return LeakDetectionAssessment {
                state: LeakDetectionState::LeakDetected,
                reason_code: "leak_detection.leak_detected".into(),
                fail_closed_required: true,
                affected_protocols: failed.iter().map(|target| target.protocol()).collect(),
                affected_targets: failed,
            };
        }

        if !self.environment.protected_path_active {
            return LeakDetectionAssessment::insufficient(
                "leak_detection.protected_path_inactive",
                LeakDetectionProtocol::ALL.into_iter().collect(),
                required,
            );
        }
        if self.environment.dns_mode == LeakDetectionDnsMode::Unmanaged {
            return LeakDetectionAssessment::insufficient(
                "leak_detection.dns_mode_unmanaged",
                vec![LeakDetectionProtocol::Dns],
                LeakDetectionProtocol::Dns.required_targets(false),
            );
        }

        let mut missing = Vec::new();
        let mut unsupported = Vec::new();
        let mut not_tested = Vec::new();
        let mut isolated = false;
        for target in &required {
            let Some(evidence) = self.evidence_for(*target) else {
                missing.push(*target);
                continue;
            };
            match evidence.state {
                LeakDetectionEvidenceState::Passed => {
                    if evidence
                        .isolation_action
                        .is_some_and(LeakDetectionIsolationAction::blocks_protocol)
                    {
                        isolated = true;
                    }
                }
                LeakDetectionEvidenceState::Unsupported => {
                    if unsupported_target_is_isolated(*target, evidence) {
                        isolated = true;
                    } else {
                        unsupported.push(*target);
                    }
                }
                LeakDetectionEvidenceState::NotTested => not_tested.push(*target),
                LeakDetectionEvidenceState::Failed => unreachable!("handled above"),
            }
        }

        if !missing.is_empty() {
            return LeakDetectionAssessment::insufficient(
                "leak_detection.evidence_missing",
                affected_protocols(&missing),
                missing,
            );
        }
        if !not_tested.is_empty() {
            return LeakDetectionAssessment::insufficient(
                "leak_detection.evidence_not_tested",
                affected_protocols(&not_tested),
                not_tested,
            );
        }
        if !unsupported.is_empty() {
            return LeakDetectionAssessment::insufficient(
                "leak_detection.evidence_unsupported",
                affected_protocols(&unsupported),
                unsupported,
            );
        }

        LeakDetectionAssessment {
            state: if isolated {
                LeakDetectionState::Isolated
            } else {
                LeakDetectionState::LeakFree
            },
            reason_code: if isolated {
                "leak_detection.isolated".into()
            } else {
                "leak_detection.leak_free".into()
            },
            fail_closed_required: false,
            affected_protocols: Vec::new(),
            affected_targets: Vec::new(),
        }
    }
}

impl LeakDetectionProtocol {
    pub const ALL: [Self; 3] = [Self::Dns, Self::Ipv4, Self::Ipv6];

    fn required_targets(self, carrier_available: bool) -> Vec<LeakDetectionTarget> {
        match self {
            Self::Dns => vec![
                LeakDetectionTarget::DnsResolver,
                LeakDetectionTarget::BrowserApplicationDns,
            ],
            Self::Ipv4 if carrier_available => vec![
                LeakDetectionTarget::Ipv4Carrier,
                LeakDetectionTarget::Ipv4SourceAddress,
                LeakDetectionTarget::Ipv4DefaultRoute,
                LeakDetectionTarget::Ipv4ControlledLeakService,
                LeakDetectionTarget::Ipv4ExternalTarget,
            ],
            Self::Ipv4 => vec![LeakDetectionTarget::Ipv4Carrier],
            Self::Ipv6 if carrier_available => vec![
                LeakDetectionTarget::Ipv6Carrier,
                LeakDetectionTarget::Ipv6SourceAddress,
                LeakDetectionTarget::Ipv6DefaultRoute,
                LeakDetectionTarget::Ipv6ControlledLeakService,
                LeakDetectionTarget::Ipv6ExternalTarget,
            ],
            Self::Ipv6 => vec![LeakDetectionTarget::Ipv6Carrier],
        }
    }
}

impl LeakDetectionTarget {
    pub const ALL: [Self; 12] = [
        Self::DnsResolver,
        Self::BrowserApplicationDns,
        Self::Ipv4Carrier,
        Self::Ipv4SourceAddress,
        Self::Ipv4DefaultRoute,
        Self::Ipv4ControlledLeakService,
        Self::Ipv4ExternalTarget,
        Self::Ipv6Carrier,
        Self::Ipv6SourceAddress,
        Self::Ipv6DefaultRoute,
        Self::Ipv6ControlledLeakService,
        Self::Ipv6ExternalTarget,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeakDetectionState {
    LeakFree,
    Isolated,
    LeakDetected,
    InsufficientEvidence,
}

impl LeakDetectionState {
    pub const fn key(self) -> &'static str {
        match self {
            Self::LeakFree => "leak_free",
            Self::Isolated => "isolated",
            Self::LeakDetected => "leak_detected",
            Self::InsufficientEvidence => "insufficient_evidence",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeakDetectionAssessment {
    pub state: LeakDetectionState,
    pub reason_code: String,
    pub fail_closed_required: bool,
    pub affected_protocols: Vec<LeakDetectionProtocol>,
    pub affected_targets: Vec<LeakDetectionTarget>,
}

impl Default for LeakDetectionAssessment {
    fn default() -> Self {
        Self::insufficient(
            "leak_detection.evidence_missing",
            LeakDetectionProtocol::ALL.into_iter().collect(),
            LeakDetectionTarget::ALL.into_iter().collect(),
        )
    }
}

impl LeakDetectionAssessment {
    fn insufficient(
        reason_code: &'static str,
        protocols: Vec<LeakDetectionProtocol>,
        targets: Vec<LeakDetectionTarget>,
    ) -> Self {
        Self {
            state: LeakDetectionState::InsufficientEvidence,
            reason_code: reason_code.into(),
            fail_closed_required: true,
            affected_protocols: protocols,
            affected_targets: targets,
        }
    }
}

fn unsupported_target_is_isolated(
    target: LeakDetectionTarget,
    evidence: &LeakDetectionEvidence,
) -> bool {
    matches!(
        (target, evidence.isolation_action),
        (
            LeakDetectionTarget::Ipv4Carrier,
            Some(LeakDetectionIsolationAction::BlockedUnavailableIpv4)
        ) | (
            LeakDetectionTarget::Ipv6Carrier,
            Some(LeakDetectionIsolationAction::BlockedUnavailableIpv6)
        )
    )
}

fn affected_protocols(targets: &[LeakDetectionTarget]) -> Vec<LeakDetectionProtocol> {
    targets
        .iter()
        .map(|target| target.protocol())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn is_stable_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(
        target: LeakDetectionTarget,
        state: LeakDetectionEvidenceState,
        observed_at_unix_ms: u64,
    ) -> LeakDetectionEvidence {
        LeakDetectionEvidence {
            target,
            state,
            source: match target {
                LeakDetectionTarget::DnsResolver => LeakDetectionSource::ManagedDoh,
                LeakDetectionTarget::BrowserApplicationDns => {
                    LeakDetectionSource::ApplicationBehaviorProbe
                }
                LeakDetectionTarget::Ipv4Carrier
                | LeakDetectionTarget::Ipv4SourceAddress
                | LeakDetectionTarget::Ipv4DefaultRoute
                | LeakDetectionTarget::Ipv6Carrier
                | LeakDetectionTarget::Ipv6SourceAddress
                | LeakDetectionTarget::Ipv6DefaultRoute => LeakDetectionSource::LocalRoutingTable,
                LeakDetectionTarget::Ipv4ControlledLeakService
                | LeakDetectionTarget::Ipv6ControlledLeakService => {
                    LeakDetectionSource::ControlledProbe
                }
                LeakDetectionTarget::Ipv4ExternalTarget
                | LeakDetectionTarget::Ipv6ExternalTarget => {
                    LeakDetectionSource::ExternalReachabilityProbe
                }
            },
            observed_at_unix_ms,
            duration_ms: 1,
            error_code: (state == LeakDetectionEvidenceState::Failed)
                .then(|| "leak_detection.check_failed".into()),
            isolation_action: None,
        }
    }

    fn complete_snapshot(now: u64) -> LeakDetectionSnapshot {
        LeakDetectionSnapshot {
            generated_at_unix_ms: now,
            environment: LeakDetectionEnvironment {
                platform: LeakDetectionPlatform::Linux,
                dns_mode: LeakDetectionDnsMode::Doh,
                protected_path_active: true,
                ipv4_carrier_available: true,
                ipv6_carrier_available: true,
            },
            evidence: LeakDetectionTarget::ALL
                .into_iter()
                .map(|target| evidence(target, LeakDetectionEvidenceState::Passed, now))
                .collect(),
            ..LeakDetectionSnapshot::default()
        }
    }

    #[test]
    fn complete_fresh_evidence_is_leak_free() {
        let assessment = complete_snapshot(1_700_000_000_000).assess(1_700_000_000_000);

        assert_eq!(assessment.state, LeakDetectionState::LeakFree);
        assert_eq!(assessment.reason_code, "leak_detection.leak_free");
        assert!(!assessment.fail_closed_required);
        assert!(assessment.affected_protocols.is_empty());
    }

    #[test]
    fn unavailable_ipv6_is_isolated_not_reported_as_a_plain_pass() {
        let now = 1_700_000_000_000;
        let mut snapshot = complete_snapshot(now);
        snapshot.environment.ipv6_carrier_available = false;
        snapshot
            .evidence
            .retain(|item| item.target.protocol() != LeakDetectionProtocol::Ipv6);
        snapshot.evidence.push(LeakDetectionEvidence {
            target: LeakDetectionTarget::Ipv6Carrier,
            state: LeakDetectionEvidenceState::Unsupported,
            source: LeakDetectionSource::LinkCapability,
            observed_at_unix_ms: now,
            duration_ms: 0,
            error_code: None,
            isolation_action: Some(LeakDetectionIsolationAction::BlockedUnavailableIpv6),
        });

        let assessment = snapshot.assess(now);

        assert_eq!(assessment.state, LeakDetectionState::Isolated);
        assert_eq!(assessment.reason_code, "leak_detection.isolated");
        assert!(!assessment.fail_closed_required);
    }

    #[test]
    fn missing_stale_future_and_untested_evidence_fail_closed() {
        let now = 1_700_000_000_000;
        let complete = complete_snapshot(now);

        let mut missing = complete.clone();
        missing
            .evidence
            .retain(|item| item.target != LeakDetectionTarget::BrowserApplicationDns);
        assert_eq!(
            missing.assess(now),
            LeakDetectionAssessment::insufficient(
                "leak_detection.evidence_missing",
                vec![LeakDetectionProtocol::Dns],
                vec![LeakDetectionTarget::BrowserApplicationDns],
            )
        );

        let mut stale = complete.clone();
        stale.evidence[0].observed_at_unix_ms = now - stale.max_age_ms - 1;
        assert_eq!(
            stale.assess(now).reason_code,
            "leak_detection.evidence_stale"
        );

        let mut future = complete.clone();
        future.evidence[0].observed_at_unix_ms = now + LEAK_DETECTION_MAX_FUTURE_SKEW_MS + 1;
        assert_eq!(
            future.assess(now).reason_code,
            "leak_detection.evidence_stale"
        );

        let mut untested = complete;
        untested.evidence[0].state = LeakDetectionEvidenceState::NotTested;
        let assessment = untested.assess(now);
        assert_eq!(assessment.state, LeakDetectionState::InsufficientEvidence);
        assert_eq!(assessment.reason_code, "leak_detection.evidence_not_tested");
        assert!(assessment.fail_closed_required);
    }

    #[test]
    fn a_failed_required_check_is_always_leak_detected() {
        let now = 1_700_000_000_000;
        let mut snapshot = complete_snapshot(now);
        let index = snapshot
            .evidence
            .iter()
            .position(|item| item.target == LeakDetectionTarget::Ipv4DefaultRoute)
            .unwrap();
        snapshot.evidence[index].state = LeakDetectionEvidenceState::Failed;
        snapshot.evidence[index].error_code = Some("leak_detection.default_route_escaped".into());

        let assessment = snapshot.assess(now);

        assert_eq!(assessment.state, LeakDetectionState::LeakDetected);
        assert_eq!(assessment.reason_code, "leak_detection.leak_detected");
        assert_eq!(
            assessment.affected_targets,
            vec![LeakDetectionTarget::Ipv4DefaultRoute]
        );
        assert!(assessment.fail_closed_required);
    }

    #[test]
    fn unsupported_non_carrier_checks_and_unmanaged_dns_are_not_a_pass() {
        let now = 1_700_000_000_000;
        let mut unsupported = complete_snapshot(now);
        unsupported.evidence[1].state = LeakDetectionEvidenceState::Unsupported;
        assert_eq!(
            unsupported.assess(now).reason_code,
            "leak_detection.evidence_unsupported"
        );

        let mut unmanaged = complete_snapshot(now);
        unmanaged.environment.dns_mode = LeakDetectionDnsMode::Unmanaged;
        assert_eq!(
            unmanaged.assess(now).reason_code,
            "leak_detection.dns_mode_unmanaged"
        );
    }

    #[test]
    fn invalid_or_duplicate_evidence_is_rejected() {
        let now = 1_700_000_000_000;
        let mut duplicate = complete_snapshot(now);
        duplicate.evidence.push(duplicate.evidence[0].clone());
        assert_eq!(
            duplicate.validate().unwrap_err().code(),
            "domain.invalid_value"
        );
        assert_eq!(
            duplicate.assess(now).reason_code,
            "leak_detection.invalid_evidence"
        );

        let mut empty = complete_snapshot(now);
        empty.evidence[0].observed_at_unix_ms = 0;
        assert!(empty.validate().is_err());
    }
}
