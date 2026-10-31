//! Ordered node traffic policy with fail-closed evaluation.

use super::foundation::{
    require_non_empty, require_unique, ContractIssue, ContractResult, UnixMillis,
};
use super::ids::{NetworkId, NodeId, PolicyId};
use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum TrafficAction {
    Direct,
    Network {
        network_id: NetworkId,
    },
    Exit {
        network_id: NetworkId,
        exit_node: NodeId,
    },
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum DomainMatcher {
    Exact(String),
    /// Matches the domain itself and all of its subdomains.
    Suffix(String),
}

impl DomainMatcher {
    fn validate(&self, field: &str, issues: &mut Vec<ContractIssue>) {
        let domain = match self {
            Self::Exact(domain) | Self::Suffix(domain) => domain,
        };
        if !is_valid_domain(domain) {
            issues.push(ContractIssue::new(
                "invalid_domain",
                field,
                "domain must be lowercase ASCII without a leading dot",
            ));
        }
    }

    fn matches(&self, candidate: &str) -> bool {
        let candidate = candidate.trim_end_matches('.').to_ascii_lowercase();
        match self {
            Self::Exact(domain) => candidate == *domain,
            Self::Suffix(domain) => {
                candidate == *domain
                    || candidate
                        .strip_suffix(domain)
                        .is_some_and(|prefix| prefix.ends_with('.'))
            }
        }
    }
}

fn is_valid_domain(domain: &str) -> bool {
    !domain.is_empty()
        && domain.len() <= 253
        && domain.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
        })
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains("..")
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct IpCidr(String);

impl IpCidr {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        IpNet::from_str(&value).map_err(|_| format!("invalid IP/CIDR: {value}"))?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn matches(&self, address: IpAddr) -> bool {
        IpNet::from_str(&self.0)
            .map(|network| network.contains(&address))
            .unwrap_or(false)
    }
}

impl Serialize for IpCidr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for IpCidr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for IpCidr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortRange {
    pub start: u16,
    pub end: u16,
}

impl PortRange {
    pub const fn single(port: u16) -> Self {
        Self {
            start: port,
            end: port,
        }
    }

    fn matches(self, port: u16) -> bool {
        self.start <= port && port <= self.end
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportProtocol {
    Tcp,
    Udp,
    Icmp,
    Other(u8),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplicationIdentity {
    pub app_id: String,
    pub executable_hash: Option<String>,
    pub known: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplicationMatcher {
    pub app_id: String,
    pub executable_hash: Option<String>,
}

impl ApplicationMatcher {
    fn matches(&self, application: Option<&ApplicationIdentity>) -> bool {
        let Some(application) = application else {
            return false;
        };
        if !application.known || application.app_id != self.app_id {
            return false;
        }
        self.executable_hash
            .as_ref()
            .is_none_or(|expected| application.executable_hash.as_ref() == Some(expected))
    }
}

/// Fields combine with AND; values inside one field combine with OR. An empty
/// field is a wildcard. This rule is intentionally identical in Rust, WebUI,
/// and Android.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficConditions {
    pub domains: Vec<DomainMatcher>,
    pub destinations: Vec<IpCidr>,
    pub ports: Vec<PortRange>,
    pub protocols: Vec<TransportProtocol>,
    pub applications: Vec<ApplicationMatcher>,
}

impl TrafficConditions {
    fn validate(&self, field: &str, issues: &mut Vec<ContractIssue>) {
        for (index, domain) in self.domains.iter().enumerate() {
            domain.validate(&format!("{field}.domains[{index}]"), issues);
        }
        for (index, range) in self.ports.iter().enumerate() {
            if range.start > range.end {
                issues.push(ContractIssue::new(
                    "invalid_port_range",
                    format!("{field}.ports[{index}]"),
                    "port range start must not exceed end",
                ));
            }
        }
        for (index, application) in self.applications.iter().enumerate() {
            require_non_empty(
                &application.app_id,
                &format!("{field}.applications[{index}].app_id"),
                256,
                issues,
            );
            if let Some(hash) = application.executable_hash.as_deref() {
                require_non_empty(
                    hash,
                    &format!("{field}.applications[{index}].executable_hash"),
                    256,
                    issues,
                );
            }
        }
    }

    fn matches(&self, flow: &TrafficFlow) -> bool {
        let domain_matches = self.domains.is_empty()
            || flow
                .domain
                .as_deref()
                .is_some_and(|domain| self.domains.iter().any(|matcher| matcher.matches(domain)));
        let destination_matches = self.destinations.is_empty()
            || self
                .destinations
                .iter()
                .any(|network| network.matches(flow.destination));
        let port_matches = self.ports.is_empty()
            || flow
                .destination_port
                .is_some_and(|port| self.ports.iter().any(|range| range.matches(port)));
        let protocol_matches = self.protocols.is_empty() || self.protocols.contains(&flow.protocol);
        let application_matches = self.applications.is_empty()
            || self
                .applications
                .iter()
                .any(|matcher| matcher.matches(flow.application.as_ref()));
        domain_matches
            && destination_matches
            && port_matches
            && protocol_matches
            && application_matches
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficFlow {
    pub domain: Option<String>,
    pub destination: IpAddr,
    pub destination_port: Option<u16>,
    pub protocol: TransportProtocol,
    /// `None` means the process has not been resolved to a trusted identity.
    /// Policies never infer an application from a path or process name.
    pub application: Option<ApplicationIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficRule {
    pub id: String,
    pub conditions: TrafficConditions,
    pub action: TrafficAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficPolicy {
    pub id: PolicyId,
    pub revision: u64,
    pub rules: Vec<TrafficRule>,
    pub on_no_match: TrafficAction,
    pub created_at: UnixMillis,
    pub expires_at: Option<UnixMillis>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficDecision {
    pub action: TrafficAction,
    pub matched_rule: Option<String>,
    pub fail_closed: bool,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrafficPolicyError {
    Contract(Vec<ContractIssue>),
    StaleRevision { expected: u64, actual: u64 },
}

impl fmt::Display for TrafficPolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(issues) => write!(f, "invalid traffic policy: {issues:?}"),
            Self::StaleRevision { expected, actual } => {
                write!(
                    f,
                    "stale traffic policy revision: expected {expected}, got {actual}"
                )
            }
        }
    }
}

impl std::error::Error for TrafficPolicyError {}

impl TrafficPolicy {
    pub fn validate(&self) -> ContractResult {
        let mut issues = Vec::new();
        if self.revision == 0 {
            issues.push(ContractIssue::new(
                "invalid_policy_revision",
                "revision",
                "traffic policy revision must be greater than zero",
            ));
        }
        if self.on_no_match != TrafficAction::Reject {
            issues.push(ContractIssue::new(
                "fail_closed_required",
                "on_no_match",
                "no-match traffic must be rejected",
            ));
        }
        if self
            .expires_at
            .is_some_and(|expires_at| !self.created_at.is_before(expires_at))
        {
            issues.push(ContractIssue::new(
                "invalid_policy_expiry",
                "expires_at",
                "policy expiry must be after creation time",
            ));
        }
        let rule_ids = self
            .rules
            .iter()
            .map(|rule| rule.id.as_str())
            .collect::<Vec<_>>();
        require_unique(&rule_ids, "rules", &mut issues);
        for (index, rule) in self.rules.iter().enumerate() {
            require_non_empty(&rule.id, &format!("rules[{index}].id"), 128, &mut issues);
            rule.conditions
                .validate(&format!("rules[{index}].conditions"), &mut issues);
        }
        finish(issues)
    }

    pub fn evaluate(&self, flow: &TrafficFlow, now: UnixMillis) -> TrafficDecision {
        if self
            .expires_at
            .is_some_and(|expires_at| !now.is_before(expires_at))
        {
            return TrafficDecision {
                action: TrafficAction::Reject,
                matched_rule: None,
                fail_closed: true,
                reason: "policy_expired".into(),
            };
        }
        if let Some(rule) = self.rules.iter().find(|rule| rule.conditions.matches(flow)) {
            return TrafficDecision {
                action: rule.action.clone(),
                matched_rule: Some(rule.id.clone()),
                fail_closed: rule.action == TrafficAction::Reject,
                reason: "rule_matched".into(),
            };
        }
        TrafficDecision {
            action: self.on_no_match.clone(),
            matched_rule: None,
            fail_closed: true,
            reason: "no_match_fail_closed".into(),
        }
    }

    /// Atomically replace a policy after validation and optimistic revision
    /// checking. The previous value is returned so the caller can persist it
    /// as the rollback point before acknowledging the update.
    pub fn replace(
        &mut self,
        next: TrafficPolicy,
        expected_revision: u64,
    ) -> Result<TrafficPolicy, TrafficPolicyError> {
        if self.revision != expected_revision {
            return Err(TrafficPolicyError::StaleRevision {
                expected: expected_revision,
                actual: self.revision,
            });
        }
        next.validate().map_err(TrafficPolicyError::Contract)?;
        if next.revision <= self.revision {
            return Err(TrafficPolicyError::StaleRevision {
                expected: self.revision + 1,
                actual: next.revision,
            });
        }
        let previous = std::mem::replace(self, next);
        Ok(previous)
    }

    pub fn rollback(
        &mut self,
        previous: TrafficPolicy,
        expected_revision: u64,
    ) -> Result<TrafficPolicy, TrafficPolicyError> {
        if self.revision != expected_revision {
            return Err(TrafficPolicyError::StaleRevision {
                expected: expected_revision,
                actual: self.revision,
            });
        }
        previous.validate().map_err(TrafficPolicyError::Contract)?;
        let failed = std::mem::replace(self, previous);
        Ok(failed)
    }
}

fn finish(issues: Vec<ContractIssue>) -> ContractResult {
    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn policy(rules: Vec<TrafficRule>) -> TrafficPolicy {
        TrafficPolicy {
            id: PolicyId::from_bytes([1; 16]),
            revision: 1,
            rules,
            on_no_match: TrafficAction::Reject,
            created_at: UnixMillis(1),
            expires_at: None,
        }
    }

    fn flow() -> TrafficFlow {
        TrafficFlow {
            domain: Some("www.example.com".into()),
            destination: IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10)),
            destination_port: Some(443),
            protocol: TransportProtocol::Tcp,
            application: Some(ApplicationIdentity {
                app_id: "browser".into(),
                executable_hash: Some("sha256:browser".into()),
                known: true,
            }),
        }
    }

    #[test]
    fn first_match_wins_and_conditions_combine_with_and() {
        let policy = policy(vec![
            TrafficRule {
                id: "specific".into(),
                conditions: TrafficConditions {
                    domains: vec![DomainMatcher::Suffix("example.com".into())],
                    destinations: vec![IpCidr::new("203.0.113.0/24").expect("cidr")],
                    ports: vec![PortRange::single(443)],
                    protocols: vec![TransportProtocol::Tcp],
                    applications: vec![ApplicationMatcher {
                        app_id: "browser".into(),
                        executable_hash: None,
                    }],
                },
                action: TrafficAction::Direct,
            },
            TrafficRule {
                id: "fallback-rule".into(),
                conditions: TrafficConditions::default(),
                action: TrafficAction::Exit {
                    network_id: NetworkId::from_bytes([2; 32]),
                    exit_node: NodeId::from_bytes([3; 32]),
                },
            },
        ]);
        policy.validate().expect("valid");
        let decision = policy.evaluate(&flow(), UnixMillis(2));
        assert_eq!(decision.matched_rule.as_deref(), Some("specific"));
        assert_eq!(decision.action, TrafficAction::Direct);
    }

    #[test]
    fn no_match_and_expired_policy_fail_closed() {
        let mut policy = policy(vec![TrafficRule {
            id: "not-matching".into(),
            conditions: TrafficConditions {
                domains: vec![DomainMatcher::Exact("blocked.example".into())],
                ..TrafficConditions::default()
            },
            action: TrafficAction::Direct,
        }]);
        let no_match = policy.evaluate(&flow(), UnixMillis(2));
        assert_eq!(no_match.action, TrafficAction::Reject);
        assert!(no_match.fail_closed);

        policy.expires_at = Some(UnixMillis(10));
        let expired = policy.evaluate(&flow(), UnixMillis(10));
        assert_eq!(expired.reason, "policy_expired");
        assert_eq!(expired.action, TrafficAction::Reject);
    }

    #[test]
    fn suffix_domain_and_multi_value_or_are_explicit() {
        let conditions = TrafficConditions {
            domains: vec![
                DomainMatcher::Exact("example.com".into()),
                DomainMatcher::Suffix("corp.test".into()),
            ],
            ports: vec![PortRange::single(80), PortRange::single(443)],
            ..TrafficConditions::default()
        };
        let mut candidate = flow();
        candidate.domain = Some("api.corp.test".into());
        assert!(conditions.matches(&candidate));
        candidate.destination_port = Some(8443);
        assert!(!conditions.matches(&candidate));
    }

    #[test]
    fn unknown_application_never_matches_an_application_rule() {
        let conditions = TrafficConditions {
            applications: vec![ApplicationMatcher {
                app_id: "browser".into(),
                executable_hash: None,
            }],
            ..TrafficConditions::default()
        };
        let mut candidate = flow();
        candidate.application = Some(ApplicationIdentity {
            app_id: "browser".into(),
            executable_hash: None,
            known: false,
        });
        assert!(!conditions.matches(&candidate));
    }

    #[test]
    fn replacement_is_atomic_and_rollback_restores_previous_policy() {
        let mut current = policy(Vec::new());
        let previous = current.clone();
        let mut next = current.clone();
        next.revision = 2;
        next.rules.push(TrafficRule {
            id: "direct".into(),
            conditions: TrafficConditions::default(),
            action: TrafficAction::Direct,
        });
        let rollback = current.replace(next, 1).expect("replace");
        assert_eq!(current.revision, 2);
        assert_eq!(rollback, previous);

        current.rollback(rollback, 2).expect("rollback");
        assert_eq!(current.revision, 1);
        assert!(current.rules.is_empty());
    }

    #[test]
    fn invalid_replacement_does_not_mutate_current_policy() {
        let mut current = policy(Vec::new());
        let mut invalid = current.clone();
        invalid.revision = 2;
        invalid.on_no_match = TrafficAction::Direct;
        let error = current.replace(invalid, 1).expect_err("invalid update");
        assert!(matches!(error, TrafficPolicyError::Contract(_)));
        assert_eq!(current.revision, 1);
    }
}
