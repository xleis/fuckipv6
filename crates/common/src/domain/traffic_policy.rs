use std::collections::BTreeSet;

use ipnet::IpNet;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ids::{ContentHash, NetworkId, NodeId, TrafficRuleId};
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

/// Exactly one traffic decision. Network and Exit decisions carry the concrete
/// target in the variant, so a rule can never return an ambiguous set of exits.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum TrafficAction {
    Direct,
    Network(NetworkId),
    Exit(NodeId),
    Reject,
}

/// Who explicitly confirmed the fallback decision for traffic that matches no
/// rule. There is intentionally no default confirmation value.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum TrafficDefaultConfirmedBy {
    Owner,
    Administrator,
}

/// A visible, acknowledged fallback. Runtime evaluation must never infer a
/// fallback from a mode, recommendation, or absent rule.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficDefaultPolicy {
    pub action: TrafficAction,
    pub confirmed_by: TrafficDefaultConfirmedBy,
    pub confirmed_at_unix_ms: u64,
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
    pub default_policy: TrafficDefaultPolicy,
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
        self.validate_default_policy()?;
        let mut rule_ids = BTreeSet::new();
        for (index, rule) in self.rules.iter().enumerate() {
            rule.matcher.validate()?;
            if !rule_ids.insert(&rule.rule_id) {
                return Err(DomainError::invariant("duplicate traffic rule id"));
            }
            let expected_priority = u32::try_from(index + 1).map_err(|_| {
                DomainError::invalid_value("traffic policy contains too many rules")
            })?;
            if rule.priority != expected_priority {
                return Err(DomainError::invariant(
                    "traffic rule priority must match its visible order",
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

    fn validate_default_policy(&self) -> DomainResult<()> {
        if self.default_policy.confirmed_at_unix_ms == 0 {
            return Err(DomainError::new(
                "domain.traffic_default_unconfirmed",
                "traffic default policy must be explicitly confirmed",
            ));
        }
        if matches!(
            &self.default_policy.action,
            TrafficAction::Exit(exit) if exit == &self.node_id
        ) {
            return Err(DomainError::invariant(
                "traffic policy cannot route a node through itself",
            ));
        }
        Ok(())
    }

    pub fn evaluate(&self, flow: &TrafficFlow) -> DomainResult<TrafficDecision> {
        self.validate()?;
        flow.validate()?;
        // Vector order is the exact order shown by every UI and serialized by
        // the API. Priority validation above prevents a second hidden order.
        for rule in self.rules.iter().filter(|rule| rule.enabled) {
            if rule.matcher.matches(flow) {
                return Ok(TrafficDecision {
                    action: rule.action.clone(),
                    matched_rule: Some(rule.rule_id.clone()),
                    reason: TrafficDecisionReason::FirstMatch,
                });
            }
        }
        Ok(TrafficDecision {
            action: self.default_policy.action.clone(),
            matched_rule: None,
            reason: TrafficDecisionReason::Default,
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

    /// Reorder rules by stable IDs using optimistic concurrency. The expected
    /// revision must still equal the current revision, and every rule ID must
    /// appear exactly once. The returned policy is a validated successor; this
    /// method never mutates the live policy in place.
    pub fn with_reordered_rules(
        &self,
        expected_revision: u64,
        ordered_rule_ids: &[TrafficRuleId],
    ) -> DomainResult<TrafficPolicy> {
        self.validate()?;
        if expected_revision != self.revision {
            return Err(DomainError::new(
                "domain.traffic_policy_concurrent_update",
                "traffic policy was modified after it was read",
            ));
        }
        if ordered_rule_ids.len() != self.rules.len() {
            return Err(DomainError::invalid_value(
                "reordered traffic rules must include every existing rule",
            ));
        }
        let mut seen = BTreeSet::new();
        let mut ordered = Vec::with_capacity(self.rules.len());
        for rule_id in ordered_rule_ids {
            if !seen.insert(rule_id) {
                return Err(DomainError::invariant(
                    "reordered traffic rules contain a duplicate id",
                ));
            }
            let rule = self
                .rules
                .iter()
                .find(|rule| &rule.rule_id == rule_id)
                .ok_or_else(|| {
                    DomainError::invalid_value("reordered traffic rule id is unknown")
                })?;
            ordered.push(rule.clone());
        }
        for (index, rule) in ordered.iter_mut().enumerate() {
            rule.priority = u32::try_from(index + 1).map_err(|_| {
                DomainError::invalid_value("traffic policy contains too many rules")
            })?;
        }
        let candidate = TrafficPolicy {
            node_id: self.node_id.clone(),
            revision: self
                .revision
                .checked_add(1)
                .ok_or_else(|| DomainError::invalid_value("traffic policy revision overflowed"))?,
            default_policy: self.default_policy.clone(),
            rules: ordered,
        };
        self.validate_successor(&candidate)?;
        Ok(candidate)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrafficDecisionReason {
    FirstMatch,
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficDecision {
    pub action: TrafficAction,
    pub matched_rule: Option<TrafficRuleId>,
    pub reason: TrafficDecisionReason,
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
    use proptest::prelude::*;

    fn node() -> NodeId {
        NodeId::from_public_key(b"local")
    }

    fn network() -> NetworkId {
        NetworkId::from_public_key(b"network")
    }

    fn default_policy(action: TrafficAction) -> TrafficDefaultPolicy {
        TrafficDefaultPolicy {
            action,
            confirmed_by: TrafficDefaultConfirmedBy::Owner,
            confirmed_at_unix_ms: 1_700_000_000_000,
        }
    }

    fn rule(index: usize, action: TrafficAction) -> TrafficRule {
        TrafficRule {
            rule_id: TrafficRuleId::from_public_key(&(index as u64).to_le_bytes()),
            priority: u32::try_from(index + 1).unwrap(),
            enabled: true,
            matcher: TrafficMatch::default(),
            action,
            description: String::new(),
        }
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

    fn property_action(index: usize) -> TrafficAction {
        match index % 4 {
            0 => TrafficAction::Direct,
            1 => TrafficAction::Network(NetworkId::from_public_key(&(index as u64).to_le_bytes())),
            2 => TrafficAction::Exit(NodeId::from_public_key(&(index as u64).to_le_bytes())),
            _ => TrafficAction::Reject,
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
    fn first_matching_visible_rule_wins_and_unknown_app_does_not_guess_an_application() {
        let first_id = TrafficRuleId::from_public_key(b"first");
        let policy = TrafficPolicy {
            node_id: node(),
            revision: 1,
            default_policy: default_policy(TrafficAction::Reject),
            rules: vec![
                TrafficRule {
                    rule_id: first_id.clone(),
                    priority: 1,
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
                TrafficRule {
                    rule_id: TrafficRuleId::from_public_key(b"later"),
                    priority: 2,
                    enabled: true,
                    matcher: TrafficMatch {
                        domain_suffixes: vec!["example.com".into()],
                        ..TrafficMatch::default()
                    },
                    action: TrafficAction::Network(network()),
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
        assert_eq!(direct.matched_rule.as_ref(), Some(&first_id));
        assert_eq!(direct.reason, TrafficDecisionReason::FirstMatch);

        let unknown = policy
            .evaluate(&flow(ApplicationIdentity::Unknown))
            .unwrap();
        assert_eq!(unknown.action, TrafficAction::Network(network()));
        assert_eq!(unknown.reason, TrafficDecisionReason::FirstMatch);
    }

    #[test]
    fn unconfirmed_default_and_self_exit_fail_closed() {
        let mut policy = TrafficPolicy {
            node_id: node(),
            revision: 1,
            default_policy: default_policy(TrafficAction::Direct),
            rules: Vec::new(),
        };
        policy.validate().unwrap();

        let mut unconfirmed = policy.clone();
        unconfirmed.default_policy.confirmed_at_unix_ms = 0;
        assert_eq!(
            unconfirmed.validate().unwrap_err().code(),
            "domain.traffic_default_unconfirmed"
        );

        policy.default_policy = default_policy(TrafficAction::Exit(node()));
        assert!(policy.validate().is_err());
    }

    #[test]
    fn successor_is_atomic_and_stale_revision_wins_no_race() {
        let policy = TrafficPolicy {
            node_id: node(),
            revision: 1,
            default_policy: default_policy(TrafficAction::Direct),
            rules: Vec::new(),
        };
        let mut candidate = policy.clone();
        candidate.revision = 2;
        candidate.default_policy = default_policy(TrafficAction::Reject);
        policy.validate_successor(&candidate).unwrap();
        assert_eq!(policy.default_policy.action, TrafficAction::Direct);
        assert_eq!(candidate.default_policy.action, TrafficAction::Reject);

        let mut stale = policy.clone();
        stale.revision = 1;
        assert_eq!(
            policy.validate_successor(&stale).unwrap_err().code(),
            "domain.traffic_policy_rollback"
        );
    }

    #[test]
    fn reorder_uses_stable_ids_and_rejects_stale_concurrency() {
        let first = rule(0, TrafficAction::Direct);
        let second = rule(1, TrafficAction::Reject);
        let policy = TrafficPolicy {
            node_id: node(),
            revision: 7,
            default_policy: default_policy(TrafficAction::Direct),
            rules: vec![first.clone(), second.clone()],
        };
        let reordered = policy
            .with_reordered_rules(7, &[second.rule_id.clone(), first.rule_id.clone()])
            .unwrap();
        assert_eq!(reordered.revision, 8);
        assert_eq!(reordered.rules[0].rule_id, second.rule_id);
        assert_eq!(reordered.rules[0].priority, 1);
        assert_eq!(reordered.rules[1].rule_id, first.rule_id);
        assert_eq!(reordered.rules[1].priority, 2);

        assert_eq!(
            policy
                .with_reordered_rules(6, &[first.rule_id.clone(), second.rule_id.clone()])
                .unwrap_err()
                .code(),
            "domain.traffic_policy_concurrent_update"
        );
    }

    #[test]
    fn priorities_must_match_the_visible_vector_order() {
        let mut second = rule(1, TrafficAction::Direct);
        second.priority = 1;
        let policy = TrafficPolicy {
            node_id: node(),
            revision: 1,
            default_policy: default_policy(TrafficAction::Direct),
            rules: vec![rule(0, TrafficAction::Direct), second],
        };
        assert_eq!(
            policy.validate().unwrap_err().code(),
            "domain.invariant_violation"
        );
    }

    #[test]
    fn every_action_serializes_as_one_explicit_decision() {
        let network = network();
        let exit = NodeId::from_public_key(b"exit");
        let actions = [
            TrafficAction::Direct,
            TrafficAction::Network(network.clone()),
            TrafficAction::Exit(exit.clone()),
            TrafficAction::Reject,
        ];
        let expected = [
            serde_json::json!({"kind": "direct"}),
            serde_json::json!({"kind": "network", "value": network.as_str()}),
            serde_json::json!({"kind": "exit", "value": exit.as_str()}),
            serde_json::json!({"kind": "reject"}),
        ];
        for (action, expected) in actions.into_iter().zip(expected) {
            assert_eq!(serde_json::to_value(action).unwrap(), expected);
        }
    }

    #[test]
    fn api_order_round_trips_to_the_same_runtime_decision() {
        let policy = TrafficPolicy {
            node_id: node(),
            revision: 1,
            default_policy: default_policy(TrafficAction::Reject),
            rules: vec![
                rule(0, TrafficAction::Direct),
                rule(1, TrafficAction::Network(network())),
            ],
        };
        let encoded = serde_json::to_value(&policy).unwrap();
        assert_eq!(encoded["rules"][0]["priority"], 1);
        assert_eq!(encoded["rules"][1]["priority"], 2);
        let decoded: TrafficPolicy = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, policy);
        assert_eq!(
            decoded
                .evaluate(&flow(ApplicationIdentity::Unknown))
                .unwrap(),
            policy
                .evaluate(&flow(ApplicationIdentity::Unknown))
                .unwrap()
        );
    }

    proptest! {
        #[test]
        fn first_enabled_rule_is_the_only_and_stable_decision(
            enabled in proptest::collection::vec(any::<bool>(), 1..24),
        ) {
            let rules: Vec<_> = enabled
                .iter()
                .enumerate()
                .map(|(index, enabled)| TrafficRule {
                    enabled: *enabled,
                    ..rule(index, property_action(index))
                })
                .collect();
            let expected_action = enabled
                .iter()
                .position(|enabled| *enabled)
                .map_or_else(
                    || TrafficAction::Reject,
                    property_action,
                );
            let expected_rule = enabled
                .iter()
                .position(|enabled| *enabled)
                .map(|index| rules[index].rule_id.clone());
            let policy = TrafficPolicy {
                node_id: node(),
                revision: 1,
                default_policy: default_policy(TrafficAction::Reject),
                rules,
            };

            let first = policy.evaluate(&flow(ApplicationIdentity::Unknown)).unwrap();
            let second = policy.evaluate(&flow(ApplicationIdentity::Unknown)).unwrap();
            prop_assert_eq!(&first, &second);
            prop_assert_eq!(&first.action, &expected_action);
            prop_assert_eq!(&first.matched_rule, &expected_rule);
            if first.matched_rule.is_some() {
                prop_assert_eq!(first.reason, TrafficDecisionReason::FirstMatch);
            } else {
                prop_assert_eq!(first.reason, TrafficDecisionReason::Default);
            }
        }
    }
}
