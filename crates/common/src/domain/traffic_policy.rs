use std::collections::BTreeSet;

use ipnet::IpNet;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ids::{ContentHash, NodeId, TrafficRuleId};
use super::{DomainError, DomainResult};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum TrafficProtocol {
    Tcp,
    Udp,
    Icmp,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
pub struct PortRange {
    pub start: u16,
    pub end: u16,
}

impl PortRange {
    pub fn contains(self, port: u16) -> bool {
        self.start <= port && port <= self.end
    }

    fn validate(self) -> DomainResult<()> {
        if self.start == 0 || self.end < self.start {
            return Err(DomainError::invalid_value("traffic port range is invalid"));
        }
        Ok(())
    }
}

/// IP/CIDR matcher stored as text at the contract edge for cross-language
/// portability and parsed before policy activation.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(try_from = "String")]
pub struct IpCidr(
    #[schemars(regex(pattern = "^[0-9A-Fa-f:.]+/[0-9]{1,3}$|^[0-9A-Fa-f:.]+$"))] String,
);

impl IpCidr {
    pub fn parse(value: impl Into<String>) -> DomainResult<Self> {
        let value = value.into();
        value
            .parse::<IpNet>()
            .map_err(|_| DomainError::invalid_value("IP/CIDR matcher is invalid"))?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn matches(&self, destination: &str) -> bool {
        let Ok(destination) = destination.parse::<std::net::IpAddr>() else {
            return false;
        };
        let Ok(network) = self.0.parse::<IpNet>() else {
            return false;
        };
        network.contains(&destination)
    }
}

impl TryFrom<String> for IpCidr {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<IpCidr> for String {
    fn from(value: IpCidr) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum TrafficAction {
    Direct,
    Network,
    Exit(NodeId),
    Reject,
}

/// Unknown is a first-class observation. It never matches a positive matcher
/// for a concrete application and therefore cannot silently inherit that
/// application's route.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ApplicationIdentity {
    ExecutablePath(String),
    PackageId(String),
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficMatch {
    /// Domain suffix match, for example `example.com` also matches `www.example.com`.
    #[serde(default)]
    pub domain_suffixes: Vec<String>,
    #[serde(default)]
    pub destination_cidrs: Vec<IpCidr>,
    #[serde(default)]
    pub destination_ports: Vec<PortRange>,
    #[serde(default)]
    pub protocols: Vec<TrafficProtocol>,
    #[serde(default)]
    pub applications: Vec<ApplicationIdentity>,
}

impl TrafficMatch {
    pub fn validate(&self) -> DomainResult<()> {
        if self.domain_suffixes.len() > 256
            || self.destination_cidrs.len() > 256
            || self.destination_ports.len() > 256
            || self.protocols.len() > 8
            || self.applications.len() > 256
        {
            return Err(DomainError::invariant("traffic matcher is too large"));
        }
        if has_duplicates(&self.domain_suffixes)
            || has_duplicates(&self.destination_cidrs)
            || has_duplicates(&self.destination_ports)
            || has_duplicates(&self.protocols)
            || has_duplicates(&self.applications)
        {
            return Err(DomainError::invariant(
                "traffic matcher contains duplicate values",
            ));
        }
        for domain in &self.domain_suffixes {
            validate_domain_suffix(domain)?;
        }
        for port in &self.destination_ports {
            port.validate()?;
        }
        for application in &self.applications {
            application.validate()?;
        }
        Ok(())
    }

    pub fn matches(&self, flow: &TrafficFlow) -> bool {
        let domain_matches = self.domain_suffixes.is_empty()
            || flow.domain.as_ref().is_some_and(|domain| {
                self.domain_suffixes
                    .iter()
                    .any(|suffix| domain_matches_suffix(domain, suffix))
            });
        let cidr_matches = self.destination_cidrs.is_empty()
            || self
                .destination_cidrs
                .iter()
                .any(|cidr| cidr.matches(&flow.destination_ip));
        let port_matches = self.destination_ports.is_empty()
            || self
                .destination_ports
                .iter()
                .any(|range| range.contains(flow.destination_port));
        let protocol_matches = self.protocols.is_empty() || self.protocols.contains(&flow.protocol);
        let application_matches = self.applications.is_empty()
            || self
                .applications
                .iter()
                .any(|application| application.matches(&flow.application));
        domain_matches && cidr_matches && port_matches && protocol_matches && application_matches
    }
}

impl ApplicationIdentity {
    fn validate(&self) -> DomainResult<()> {
        match self {
            Self::ExecutablePath(path) => {
                if path.is_empty() || path.len() > 4096 || path.contains('\0') {
                    return Err(DomainError::invalid_value(
                        "application executable path is invalid",
                    ));
                }
            }
            Self::PackageId(package) => {
                if package.is_empty()
                    || package.len() > 255
                    || !package.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
                    })
                {
                    return Err(DomainError::invalid_value(
                        "application package id is invalid",
                    ));
                }
            }
            Self::Unknown => {}
        }
        Ok(())
    }

    fn matches(&self, observed: &Self) -> bool {
        match (self, observed) {
            (Self::ExecutablePath(expected), Self::ExecutablePath(actual)) => expected == actual,
            (Self::PackageId(expected), Self::PackageId(actual)) => expected == actual,
            (Self::Unknown, Self::Unknown) => true,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficRule {
    pub rule_id: TrafficRuleId,
    pub priority: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub matcher: TrafficMatch,
    pub action: TrafficAction,
    #[serde(default)]
    pub description: String,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficFlow {
    pub domain: Option<String>,
    pub destination_ip: String,
    pub destination_port: u16,
    pub protocol: TrafficProtocol,
    pub application: ApplicationIdentity,
}

impl TrafficFlow {
    fn validate(&self) -> DomainResult<()> {
        if let Some(domain) = &self.domain {
            validate_domain_suffix(domain)?;
        }
        self.destination_ip
            .parse::<std::net::IpAddr>()
            .map_err(|_| DomainError::invalid_value("traffic destination IP is invalid"))?;
        if self.destination_port == 0 {
            return Err(DomainError::invalid_value(
                "traffic destination port must be positive",
            ));
        }
        self.application.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficPolicy {
    pub node_id: NodeId,
    pub revision: u64,
    pub default_action: TrafficAction,
    #[serde(default)]
    pub rules: Vec<TrafficRule>,
}

impl TrafficPolicy {
    pub fn validate(&self) -> DomainResult<()> {
        if self.revision == 0 {
            return Err(DomainError::invalid_value(
                "traffic policy revision must be positive",
            ));
        }
        let mut rule_ids = BTreeSet::new();
        let mut priorities = BTreeSet::new();
        for rule in &self.rules {
            rule.matcher.validate()?;
            if !rule_ids.insert(&rule.rule_id) {
                return Err(DomainError::invariant("duplicate traffic rule id"));
            }
            if !priorities.insert(rule.priority) {
                return Err(DomainError::invariant(
                    "duplicate traffic rule priority makes order ambiguous",
                ));
            }
            if matches!(&rule.action, TrafficAction::Exit(exit) if exit == &self.node_id) {
                return Err(DomainError::invariant(
                    "traffic policy cannot route a node through itself",
                ));
            }
            if rule.description.len() > 256 {
                return Err(DomainError::invalid_value(
                    "traffic rule description is too long",
                ));
            }
        }
        Ok(())
    }

    pub fn evaluate(&self, flow: &TrafficFlow) -> DomainResult<TrafficDecision> {
        self.validate()?;
        flow.validate()?;
        let mut rules: Vec<_> = self.rules.iter().filter(|rule| rule.enabled).collect();
        rules.sort_by_key(|rule| rule.priority);
        for rule in rules {
            if rule.matcher.matches(flow) {
                return Ok(TrafficDecision {
                    action: rule.action.clone(),
                    rule_id: Some(rule.rule_id.clone()),
                    reason: "first_match",
                });
            }
        }
        Ok(TrafficDecision {
            action: self.default_action.clone(),
            rule_id: None,
            reason: "default",
        })
    }

    pub fn content_hash(&self) -> DomainResult<ContentHash> {
        let bytes = serde_json::to_vec(self)
            .map_err(|_| DomainError::invalid_value("traffic policy cannot be encoded"))?;
        Ok(ContentHash::from_bytes(&bytes))
    }

    /// Atomically validate a candidate against the current policy. The caller
    /// commits only on success, so a rejected update leaves the old policy live.
    pub fn validate_successor(&self, candidate: &TrafficPolicy) -> DomainResult<()> {
        self.validate()?;
        candidate.validate()?;
        if candidate.node_id != self.node_id {
            return Err(DomainError::invariant(
                "traffic policy candidate belongs to another node",
            ));
        }
        if candidate.revision <= self.revision {
            return Err(DomainError::new(
                "domain.traffic_policy_rollback",
                "traffic policy revision must increase",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficDecision {
    pub action: TrafficAction,
    pub rule_id: Option<TrafficRuleId>,
    pub reason: &'static str,
}

fn validate_domain_suffix(value: &str) -> DomainResult<()> {
    let value = value.trim_start_matches('.');
    if value.is_empty()
        || value.len() > 253
        || value.starts_with('.')
        || value.ends_with('.')
        || value.contains(['/', ':', '\\', '\0'])
        || value
            .split('.')
            .any(|label| label.is_empty() || label.len() > 63)
    {
        return Err(DomainError::invalid_value(
            "traffic domain suffix is invalid",
        ));
    }
    Ok(())
}

fn domain_matches_suffix(domain: &str, suffix: &str) -> bool {
    let domain = domain.trim_end_matches('.').to_ascii_lowercase();
    let suffix = suffix
        .trim_start_matches('.')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    domain == suffix
        || domain
            .strip_suffix(&suffix)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

fn has_duplicates<T: Ord>(values: &[T]) -> bool {
    let mut seen = BTreeSet::new();
    values.iter().any(|value| !seen.insert(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node() -> NodeId {
        NodeId::from_public_key(b"local")
    }

    fn flow(application: ApplicationIdentity) -> TrafficFlow {
        TrafficFlow {
            domain: Some("api.example.com".into()),
            destination_ip: "203.0.113.9".into(),
            destination_port: 443,
            protocol: TrafficProtocol::Tcp,
            application,
        }
    }

    #[test]
    fn rule_fields_are_and_and_multiple_values_are_or() {
        let matcher = TrafficMatch {
            domain_suffixes: vec!["example.com".into(), "example.net".into()],
            destination_cidrs: vec![IpCidr::parse("203.0.113.0/24").unwrap()],
            destination_ports: vec![PortRange {
                start: 443,
                end: 443,
            }],
            protocols: vec![TrafficProtocol::Tcp],
            applications: vec![
                ApplicationIdentity::PackageId("org.example.browser".into()),
                ApplicationIdentity::ExecutablePath("/usr/bin/browser".into()),
            ],
        };
        assert!(matcher.matches(&flow(ApplicationIdentity::PackageId(
            "org.example.browser".into()
        ))));
        let mut wrong_cidr = flow(ApplicationIdentity::PackageId("org.example.browser".into()));
        wrong_cidr.destination_ip = "198.51.100.1".into();
        assert!(!matcher.matches(&wrong_cidr));
        assert!(!matcher.matches(&flow(ApplicationIdentity::Unknown)));
    }

    #[test]
    fn first_matching_priority_wins_and_unknown_app_does_not_guess_an_application() {
        let policy = TrafficPolicy {
            node_id: node(),
            revision: 1,
            default_action: TrafficAction::Reject,
            rules: vec![
                TrafficRule {
                    rule_id: TrafficRuleId::from_public_key(b"later"),
                    priority: 200,
                    enabled: true,
                    matcher: TrafficMatch {
                        domain_suffixes: vec!["example.com".into()],
                        ..TrafficMatch::default()
                    },
                    action: TrafficAction::Network,
                    description: String::new(),
                },
                TrafficRule {
                    rule_id: TrafficRuleId::from_public_key(b"first"),
                    priority: 10,
                    enabled: true,
                    matcher: TrafficMatch {
                        applications: vec![ApplicationIdentity::PackageId(
                            "org.example.browser".into(),
                        )],
                        ..TrafficMatch::default()
                    },
                    action: TrafficAction::Direct,
                    description: String::new(),
                },
            ],
        };
        let direct = policy
            .evaluate(&flow(ApplicationIdentity::PackageId(
                "org.example.browser".into(),
            )))
            .unwrap();
        assert_eq!(direct.action, TrafficAction::Direct);
        assert_eq!(direct.reason, "first_match");
        let unknown = policy
            .evaluate(&flow(ApplicationIdentity::Unknown))
            .unwrap();
        assert_eq!(unknown.action, TrafficAction::Network);
        assert_eq!(unknown.reason, "first_match");
    }

    #[test]
    fn invalid_or_ambiguous_policy_fails_closed_and_successor_is_atomic() {
        let mut policy = TrafficPolicy {
            node_id: node(),
            revision: 1,
            default_action: TrafficAction::Direct,
            rules: Vec::new(),
        };
        policy.validate().unwrap();
        let mut invalid = policy.clone();
        invalid.revision = 2;
        invalid.rules.push(TrafficRule {
            rule_id: TrafficRuleId::from_public_key(b"self"),
            priority: 1,
            enabled: true,
            matcher: TrafficMatch::default(),
            action: TrafficAction::Exit(node()),
            description: String::new(),
        });
        assert!(policy.validate_successor(&invalid).is_err());

        let mut candidate = policy.clone();
        candidate.revision = 2;
        candidate.default_action = TrafficAction::Reject;
        policy.validate_successor(&candidate).unwrap();
        assert_eq!(policy.default_action, TrafficAction::Direct);
        assert_eq!(candidate.default_action, TrafficAction::Reject);

        policy.revision = 2;
        assert_eq!(
            policy.validate_successor(&candidate).unwrap_err().code(),
            "domain.traffic_policy_rollback"
        );
    }

    #[test]
    fn duplicate_priorities_are_rejected() {
        let rule = TrafficRule {
            rule_id: TrafficRuleId::from_public_key(b"one"),
            priority: 1,
            enabled: true,
            matcher: TrafficMatch::default(),
            action: TrafficAction::Direct,
            description: String::new(),
        };
        let mut second = rule.clone();
        second.rule_id = TrafficRuleId::from_public_key(b"two");
        let policy = TrafficPolicy {
            node_id: node(),
            revision: 1,
            default_action: TrafficAction::Direct,
            rules: vec![rule, second],
        };
        assert!(policy.validate().is_err());
    }
}
