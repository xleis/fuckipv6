use std::collections::BTreeSet;
use std::net::IpAddr;

use ipnet::IpNet;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::application_identity::{
    ApplicationIdentity, ApplicationIdentityCapability, ApplicationIdentityCapabilityState,
    ApplicationIdentityFormat,
};
use super::ids::{ContentHash, NetworkId, NodeId, TrafficRuleId};
use super::traffic_failure::{
    TrafficFailureDecision, TrafficFailureKind, TrafficFailurePolicy, TrafficFailureRouteContext,
};
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

/// Canonical, case-insensitive DNS name used for exact host matching. IDNs are
/// normalized to their ASCII A-label form and a single trailing root dot is
/// accepted but not retained.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(try_from = "String")]
pub struct DomainName(
    #[schemars(regex(
        pattern = "^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?(?:\\.[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?)*$"
    ))]
    String,
);

impl DomainName {
    pub fn parse(value: impl Into<String>) -> DomainResult<Self> {
        canonical_domain(value.into(), false).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn matches_exact(&self, observed: &str) -> bool {
        self.0 == observed
    }
}

impl TryFrom<String> for DomainName {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<DomainName> for String {
    fn from(value: DomainName) -> Self {
        value.0
    }
}

/// Canonical DNS suffix. A bare domain matches itself and descendant labels;
/// a leading `*.` is accepted only as an explicit spelling of that same
/// complete-label boundary rule. It never performs substring matching.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(try_from = "String")]
pub struct DomainSuffix(
    #[schemars(regex(
        pattern = "^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?(?:\\.[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?)*$"
    ))]
    String,
);

impl DomainSuffix {
    pub fn parse(value: impl Into<String>) -> DomainResult<Self> {
        canonical_domain(value.into(), true).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn matches(&self, observed: &str) -> bool {
        observed == self.0
            || observed
                .strip_suffix(&self.0)
                .is_some_and(|prefix| prefix.ends_with('.'))
    }
}

impl TryFrom<String> for DomainSuffix {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<DomainSuffix> for String {
    fn from(value: DomainSuffix) -> Self {
        value.0
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
        let network = if let Ok(address) = value.parse::<IpAddr>() {
            IpNet::from(address)
        } else {
            value
                .parse::<IpNet>()
                .map_err(|_| DomainError::invalid_value("IP/CIDR matcher is invalid"))?
        };
        Ok(Self(network.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn matches(&self, destination: IpAddr) -> bool {
        let Ok(network) = self.0.parse::<IpNet>() else {
            return false;
        };
        network.contains(&destination)
    }

    fn prefix_len(&self) -> u8 {
        self.0
            .parse::<IpNet>()
            .map_or(0, |network| network.prefix_len())
    }

    fn is_ipv4(&self) -> bool {
        self.0
            .parse::<IpNet>()
            .is_ok_and(|network| network.addr().is_ipv4())
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

/// A non-blocking warning attached to a matcher. Invalid or ambiguous values
/// are rejected by `validate`; these codes describe valid rules whose scope is
/// intentionally or accidentally broad and must be shown before activation.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TrafficMatchRisk {
    AlwaysMatches,
    AnyDestination,
    AnyPort,
    AnyProtocol,
    AnyApplication,
    BroadCidr { cidr: String, prefix_len: u8 },
    FullPortRange { start: u16, end: u16 },
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficRuleRisk {
    pub rule_id: TrafficRuleId,
    pub priority: u32,
    pub enabled: bool,
    pub risks: Vec<TrafficMatchRisk>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficMatch {
    /// Exact, normalized DNS names. This condition never parses or compares IP
    /// literals and never matches a subdomain.
    #[serde(default)]
    pub exact_domains: Vec<DomainName>,
    /// Complete-label DNS suffix match, for example `example.com` also matches
    /// `www.example.com` but never `notexample.com` or `example.com.evil`.
    #[serde(default)]
    pub domain_suffixes: Vec<DomainSuffix>,
    /// Exact IP values are canonicalized as `/32` or `/128`; CIDRs are matched
    /// only against the observed destination IP.
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
        if self.exact_domains.len() > 256
            || self.domain_suffixes.len() > 256
            || self.destination_cidrs.len() > 256
            || self.destination_ports.len() > 256
            || self.protocols.len() > 8
            || self.applications.len() > 256
        {
            return Err(DomainError::invariant("traffic matcher is too large"));
        }
        if has_duplicates(&self.exact_domains)
            || has_duplicates(&self.domain_suffixes)
            || has_duplicates(&self.destination_cidrs)
            || has_duplicates(&self.destination_ports)
            || has_duplicates(&self.protocols)
            || has_duplicates(&self.applications)
        {
            return Err(DomainError::invariant(
                "traffic matcher contains duplicate values",
            ));
        }
        for port in &self.destination_ports {
            port.validate()?;
        }
        for application in &self.applications {
            application.validate()?;
            if application.is_unknown() {
                return Err(DomainError::new(
                    "domain.application_identity_unknown_matcher",
                    "unknown application identity cannot be used as a positive rule condition",
                ));
            }
        }
        Ok(())
    }

    /// Check the same platform capability used to enable the UI selector.
    /// Disabled capabilities and formats not supported by that platform reject
    /// an application condition instead of silently compiling it into a rule
    /// that can never match.
    pub fn validate_for_application_capability(
        &self,
        capability: &ApplicationIdentityCapability,
    ) -> DomainResult<()> {
        self.validate()?;
        capability.validate()?;
        if self.applications.is_empty() {
            return Ok(());
        }
        if !capability.is_enabled() {
            let explanation = capability
                .disabled_explanation()
                .unwrap_or("application identity capability is unavailable");
            let code = match &capability.state {
                ApplicationIdentityCapabilityState::Unavailable(_) => {
                    "domain.application_identity_unavailable"
                }
                ApplicationIdentityCapabilityState::Unsupported(_) => {
                    "domain.application_identity_unsupported"
                }
                ApplicationIdentityCapabilityState::Available => {
                    unreachable!("enabled capability returned no formats")
                }
            };
            return Err(DomainError::new(code, explanation));
        }
        for application in &self.applications {
            let format = application.format().ok_or_else(|| {
                DomainError::new(
                    "domain.application_identity_unknown_matcher",
                    "unknown application identity cannot be used as a positive rule condition",
                )
            })?;
            if !capability.formats.contains(&format) {
                return Err(DomainError::new(
                    "domain.application_identity_format_unsupported",
                    format!(
                        "{} is not available as an application identity on {:?}",
                        format_name(format),
                        capability.platform
                    ),
                ));
            }
        }
        Ok(())
    }

    pub fn matches(&self, flow: &TrafficFlow) -> bool {
        let domain_matches = (self.exact_domains.is_empty() && self.domain_suffixes.is_empty())
            || flow.domain.as_ref().is_some_and(|domain| {
                self.exact_domains
                    .iter()
                    .any(|exact| exact.matches_exact(domain.as_str()))
                    || self
                        .domain_suffixes
                        .iter()
                        .any(|suffix| suffix.matches(domain.as_str()))
            });
        let cidr_matches = self.destination_cidrs.is_empty()
            || self
                .destination_cidrs
                .iter()
                .any(|cidr| cidr.matches(flow.destination_ip));
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

    pub fn is_empty(&self) -> bool {
        self.exact_domains.is_empty()
            && self.domain_suffixes.is_empty()
            && self.destination_cidrs.is_empty()
            && self.destination_ports.is_empty()
            && self.protocols.is_empty()
            && self.applications.is_empty()
    }

    pub fn risks(&self) -> Vec<TrafficMatchRisk> {
        if self.is_empty() {
            return vec![TrafficMatchRisk::AlwaysMatches];
        }

        let mut risks = Vec::new();
        if self.exact_domains.is_empty()
            && self.domain_suffixes.is_empty()
            && self.destination_cidrs.is_empty()
        {
            risks.push(TrafficMatchRisk::AnyDestination);
        }
        if self.destination_ports.is_empty() {
            risks.push(TrafficMatchRisk::AnyPort);
        }
        if self.protocols.is_empty() {
            risks.push(TrafficMatchRisk::AnyProtocol);
        }
        if self.applications.is_empty() {
            risks.push(TrafficMatchRisk::AnyApplication);
        }
        for cidr in &self.destination_cidrs {
            let prefix_len = cidr.prefix_len();
            if (cidr.is_ipv4() && prefix_len <= 8) || (!cidr.is_ipv4() && prefix_len <= 32) {
                risks.push(TrafficMatchRisk::BroadCidr {
                    cidr: cidr.as_str().to_string(),
                    prefix_len,
                });
            }
        }
        for port in &self.destination_ports {
            if port.start == 1 && port.end == u16::MAX {
                risks.push(TrafficMatchRisk::FullPortRange {
                    start: port.start,
                    end: port.end,
                });
            }
        }
        risks
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
    pub domain: Option<DomainName>,
    pub destination_ip: IpAddr,
    pub destination_port: u16,
    pub protocol: TrafficProtocol,
    pub application: ApplicationIdentity,
}

impl TrafficFlow {
    fn validate(&self) -> DomainResult<()> {
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
    /// Failure handling is part of the policy revision, not an ambient mode.
    /// Missing values deserialize to the fail-closed default.
    #[serde(default)]
    pub failure_policy: TrafficFailurePolicy,
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
        self.failure_policy.validate()?;
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

    /// Return structured, non-blocking scope warnings after validating every
    /// rule. The policy remains applicable, but UI and API callers must surface
    /// these warnings before activation rather than silently accepting an
    /// unconditional or very broad rule.
    pub fn risk_report(&self) -> DomainResult<Vec<TrafficRuleRisk>> {
        self.validate()?;
        Ok(self
            .rules
            .iter()
            .map(|rule| TrafficRuleRisk {
                rule_id: rule.rule_id.clone(),
                priority: rule.priority,
                enabled: rule.enabled,
                risks: rule.matcher.risks(),
            })
            .collect())
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

    /// Evaluate the failure policy that is versioned with this traffic
    /// policy. The UI must call this method for previews and the runtime must
    /// call the same method after a real failure.
    pub fn decide_failure(
        &self,
        route: &TrafficFailureRouteContext,
        failure: TrafficFailureKind,
    ) -> DomainResult<TrafficFailureDecision> {
        self.validate()?;
        self.failure_policy.decide(route, failure)
    }

    pub fn preview_failure_policy(
        &self,
        route: &TrafficFailureRouteContext,
    ) -> DomainResult<Vec<TrafficFailureDecision>> {
        self.validate()?;
        self.failure_policy.preview(route)
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
            failure_policy: self.failure_policy.clone(),
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

fn format_name(format: ApplicationIdentityFormat) -> &'static str {
    match format {
        ApplicationIdentityFormat::ExecutablePath => "executable_path",
        ApplicationIdentityFormat::PackageId => "package_id",
    }
}

fn canonical_domain(value: String, allow_wildcard_suffix: bool) -> DomainResult<String> {
    if value.is_empty() || value.trim() != value || value.contains('\0') {
        return Err(DomainError::invalid_value("traffic domain is invalid"));
    }
    let value = if allow_wildcard_suffix {
        value.strip_prefix("*.").unwrap_or(&value)
    } else {
        &value
    };
    if value.is_empty() || value.contains('*') {
        return Err(DomainError::invalid_value("traffic domain is invalid"));
    }
    let value = value.strip_suffix('.').unwrap_or(value);
    if value.is_empty() || value.ends_with('.') || value.parse::<IpAddr>().is_ok() {
        return Err(DomainError::invalid_value("traffic domain is invalid"));
    }
    let ascii = idna::domain_to_ascii(value)
        .map_err(|_| DomainError::invalid_value("traffic domain is invalid"))?
        .to_ascii_lowercase();
    if ascii.len() > 253
        || ascii.is_empty()
        || ascii.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return Err(DomainError::invalid_value("traffic domain is invalid"));
    }
    Ok(ascii)
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
            domain: Some(DomainName::parse("api.example.com").unwrap()),
            destination_ip: "203.0.113.9".parse().unwrap(),
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
    fn unknown_application_is_not_a_positive_matcher_or_a_match() {
        let matcher = TrafficMatch {
            applications: vec![ApplicationIdentity::Unknown],
            ..TrafficMatch::default()
        };
        assert_eq!(
            matcher.validate().unwrap_err().code(),
            "domain.application_identity_unknown_matcher"
        );
        assert!(!matcher.matches(&flow(ApplicationIdentity::Unknown)));
    }

    #[test]
    fn platform_capability_disables_unsupported_application_conditions() {
        let executable_matcher = TrafficMatch {
            applications: vec![ApplicationIdentity::ExecutablePath(
                "/usr/bin/browser".into(),
            )],
            ..TrafficMatch::default()
        };
        let linux = ApplicationIdentityCapability::available(
            super::super::application_identity::ApplicationIdentityPlatform::Linux,
            [ApplicationIdentityFormat::ExecutablePath],
        )
        .unwrap();
        executable_matcher
            .validate_for_application_capability(&linux)
            .unwrap();

        let disabled = ApplicationIdentityCapability::unsupported(
            super::super::application_identity::ApplicationIdentityPlatform::Linux,
            "process metadata permission was denied",
        )
        .unwrap();
        assert_eq!(
            executable_matcher
                .validate_for_application_capability(&disabled)
                .unwrap_err()
                .code(),
            "domain.application_identity_unsupported"
        );

        let package_matcher = TrafficMatch {
            applications: vec![ApplicationIdentity::PackageId("com.example.app".into())],
            ..TrafficMatch::default()
        };
        assert_eq!(
            package_matcher
                .validate_for_application_capability(&linux)
                .unwrap_err()
                .code(),
            "domain.application_identity_format_unsupported"
        );
    }

    #[test]
    fn rule_fields_are_and_and_multiple_values_are_or() {
        let matcher = TrafficMatch {
            exact_domains: Vec::new(),
            domain_suffixes: vec![
                DomainSuffix::parse("example.com").unwrap(),
                DomainSuffix::parse("example.net").unwrap(),
            ],
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
        wrong_cidr.destination_ip = "198.51.100.1".parse().unwrap();
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
            failure_policy: TrafficFailurePolicy::fail_closed(),
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
                        domain_suffixes: vec![DomainSuffix::parse("example.com").unwrap()],
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
            failure_policy: TrafficFailurePolicy::fail_closed(),
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
            failure_policy: TrafficFailurePolicy::fail_closed(),
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
            failure_policy: TrafficFailurePolicy::fail_closed(),
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
            failure_policy: TrafficFailurePolicy::fail_closed(),
            rules: vec![rule(0, TrafficAction::Direct), second],
        };
        assert_eq!(
            policy.validate().unwrap_err().code(),
            "domain.invariant_violation"
        );
    }

    #[test]
    fn domain_exact_and_suffix_matching_is_normalized_and_label_bounded() {
        let exact = DomainName::parse("BÜCHER.Example.").unwrap();
        assert_eq!(exact.as_str(), "xn--bcher-kva.example");
        assert_eq!(
            DomainName::parse("bücher.example").unwrap().as_str(),
            exact.as_str()
        );

        let exact_matcher = TrafficMatch {
            exact_domains: vec![exact.clone()],
            ..TrafficMatch::default()
        };
        let mut observed = flow(ApplicationIdentity::Unknown);
        observed.domain = Some(exact);
        assert!(exact_matcher.matches(&observed));
        observed.domain = Some(DomainName::parse("www.bücher.example").unwrap());
        assert!(!exact_matcher.matches(&observed));

        let suffix = DomainSuffix::parse("*.Example.COM.").unwrap();
        assert_eq!(suffix.as_str(), "example.com");
        let suffix_matcher = TrafficMatch {
            domain_suffixes: vec![suffix],
            ..TrafficMatch::default()
        };
        for matching in ["example.com", "WWW.Example.Com."] {
            observed.domain = Some(DomainName::parse(matching).unwrap());
            assert!(suffix_matcher.matches(&observed), "{matching}");
        }
        for non_matching in ["notexample.com", "example.com.evil", "example.net"] {
            observed.domain = Some(DomainName::parse(non_matching).unwrap());
            assert!(!suffix_matcher.matches(&observed), "{non_matching}");
        }
    }

    #[test]
    fn exact_ip_and_cidr_matching_supports_v4_v6_and_rejects_invalid_input() {
        assert_eq!(IpCidr::parse("192.0.2.1").unwrap().as_str(), "192.0.2.1/32");
        assert_eq!(
            IpCidr::parse("2001:db8::1").unwrap().as_str(),
            "2001:db8::1/128"
        );

        let exact_v4 = TrafficMatch {
            destination_cidrs: vec![IpCidr::parse("192.0.2.1").unwrap()],
            ..TrafficMatch::default()
        };
        let mut observed = flow(ApplicationIdentity::Unknown);
        observed.destination_ip = "192.0.2.1".parse().unwrap();
        assert!(exact_v4.matches(&observed));
        observed.destination_ip = "192.0.2.2".parse().unwrap();
        assert!(!exact_v4.matches(&observed));

        let cidr_v6 = TrafficMatch {
            destination_cidrs: vec![IpCidr::parse("2001:db8:1234::/48").unwrap()],
            ..TrafficMatch::default()
        };
        observed.destination_ip = "2001:db8:1234:5678::1".parse().unwrap();
        assert!(cidr_v6.matches(&observed));
        observed.destination_ip = "2001:db8:1235::1".parse().unwrap();
        assert!(!cidr_v6.matches(&observed));

        for invalid in [
            "999.1.1.1",
            "192.0.2.1/33",
            "2001:db8::1/129",
            "example.com/24",
            "192.0.2.1/not-a-prefix",
        ] {
            assert_eq!(
                IpCidr::parse(invalid).unwrap_err().code(),
                "domain.invalid_value",
                "{invalid}"
            );
        }
    }

    #[test]
    fn domain_and_ip_conditions_remain_separate_and_compose_with_and() {
        let matcher = TrafficMatch {
            exact_domains: vec![DomainName::parse("api.example.com").unwrap()],
            destination_cidrs: vec![IpCidr::parse("203.0.113.9").unwrap()],
            ..TrafficMatch::default()
        };
        assert!(matcher.matches(&flow(ApplicationIdentity::Unknown)));

        let mut wrong_domain = flow(ApplicationIdentity::Unknown);
        wrong_domain.domain = Some(DomainName::parse("cdn.example.com").unwrap());
        assert!(!matcher.matches(&wrong_domain));

        let mut wrong_ip = flow(ApplicationIdentity::Unknown);
        wrong_ip.destination_ip = "198.51.100.9".parse().unwrap();
        assert!(!matcher.matches(&wrong_ip));

        let ip_only = TrafficMatch {
            destination_cidrs: vec![IpCidr::parse("203.0.113.9").unwrap()],
            ..TrafficMatch::default()
        };
        let mut no_domain = flow(ApplicationIdentity::Unknown);
        no_domain.domain = None;
        assert!(ip_only.matches(&no_domain));
    }

    #[test]
    fn invalid_domains_and_wildcard_boundaries_are_rejected() {
        for invalid in [
            "",
            "example..com",
            "example.com..",
            "-example.com",
            "example-.com",
            "bad_label.example",
            "192.0.2.1",
            "*example.com",
            "*.*.example.com",
        ] {
            assert_eq!(
                DomainName::parse(invalid).unwrap_err().code(),
                "domain.invalid_value",
                "{invalid}"
            );
        }
        assert_eq!(
            DomainSuffix::parse("*.*.example.com").unwrap_err().code(),
            "domain.invalid_value"
        );
    }

    #[test]
    fn unconditional_broad_and_wildcard_fields_have_structured_risks() {
        assert_eq!(
            TrafficMatch::default().risks(),
            vec![TrafficMatchRisk::AlwaysMatches]
        );

        let broad = TrafficMatch {
            destination_cidrs: vec![IpCidr::parse("0.0.0.0/0").unwrap()],
            destination_ports: vec![PortRange {
                start: 1,
                end: u16::MAX,
            }],
            ..TrafficMatch::default()
        };
        broad.validate().unwrap();
        let risks = broad.risks();
        assert!(risks.contains(&TrafficMatchRisk::AnyProtocol));
        assert!(risks.contains(&TrafficMatchRisk::AnyApplication));
        assert!(risks.contains(&TrafficMatchRisk::BroadCidr {
            cidr: "0.0.0.0/0".into(),
            prefix_len: 0,
        }));
        assert!(risks.contains(&TrafficMatchRisk::FullPortRange {
            start: 1,
            end: u16::MAX,
        }));

        let policy = TrafficPolicy {
            node_id: node(),
            revision: 3,
            default_policy: default_policy(TrafficAction::Reject),
            failure_policy: TrafficFailurePolicy::fail_closed(),
            rules: vec![TrafficRule {
                matcher: broad,
                ..rule(0, TrafficAction::Direct)
            }],
        };
        let report = policy.risk_report().unwrap();
        assert_eq!(report.len(), 1);
        assert_eq!(report[0].rule_id, policy.rules[0].rule_id);
        assert!(!report[0].risks.is_empty());
    }

    #[test]
    fn overlapping_visible_rules_keep_first_match_as_the_conflict_winner() {
        let policy = TrafficPolicy {
            node_id: node(),
            revision: 1,
            default_policy: default_policy(TrafficAction::Reject),
            failure_policy: TrafficFailurePolicy::fail_closed(),
            rules: vec![
                TrafficRule {
                    matcher: TrafficMatch {
                        domain_suffixes: vec![DomainSuffix::parse("example.com").unwrap()],
                        ..TrafficMatch::default()
                    },
                    ..rule(0, TrafficAction::Direct)
                },
                TrafficRule {
                    matcher: TrafficMatch {
                        exact_domains: vec![DomainName::parse("api.example.com").unwrap()],
                        ..TrafficMatch::default()
                    },
                    ..rule(1, TrafficAction::Reject)
                },
            ],
        };
        let decision = policy
            .evaluate(&flow(ApplicationIdentity::Unknown))
            .unwrap();
        assert_eq!(decision.action, TrafficAction::Direct);
        assert_eq!(
            decision.matched_rule.as_ref(),
            Some(&policy.rules[0].rule_id)
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
    fn failure_policy_is_versioned_and_defaults_to_fail_closed() {
        let policy = TrafficPolicy {
            node_id: node(),
            revision: 1,
            default_policy: default_policy(TrafficAction::Reject),
            failure_policy: TrafficFailurePolicy::fail_closed(),
            rules: Vec::new(),
        };
        policy.validate().unwrap();
        let route = TrafficFailureRouteContext::new(TrafficAction::Exit(NodeId::from_public_key(
            b"failed-exit",
        )));
        assert_eq!(
            policy
                .decide_failure(&route, TrafficFailureKind::ExitUnavailable)
                .unwrap()
                .outcome,
            super::super::traffic_failure::TrafficFailureOutcome::FailClosed
        );

        let mut encoded = serde_json::to_value(&policy).unwrap();
        encoded
            .as_object_mut()
            .unwrap()
            .remove("failure_policy")
            .expect("serialized traffic policy must expose failure_policy");
        let decoded: TrafficPolicy = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, policy);
        assert_eq!(decoded.failure_policy, TrafficFailurePolicy::fail_closed());
    }

    #[test]
    fn api_order_round_trips_to_the_same_runtime_decision() {
        let policy = TrafficPolicy {
            node_id: node(),
            revision: 1,
            default_policy: default_policy(TrafficAction::Reject),
            failure_policy: TrafficFailurePolicy::fail_closed(),
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
                failure_policy: TrafficFailurePolicy::fail_closed(),
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

        #[test]
        fn domain_normalization_is_case_insensitive_and_idempotent(
            label in "[a-z][a-z0-9-]{0,20}[a-z0-9]",
        ) {
            let lower = format!("{label}.Example.");
            let upper = format!("{}.EXAMPLE.", label.to_ascii_uppercase());
            let normalized = DomainName::parse(lower).unwrap();
            prop_assert_eq!(&normalized, &DomainName::parse(upper).unwrap());
            prop_assert_eq!(
                &normalized,
                &DomainName::parse(format!("{}.", normalized.as_str())).unwrap(),
            );
        }
    }
}
