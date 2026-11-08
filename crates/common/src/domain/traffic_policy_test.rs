//! Side-effect-free traffic policy tests for the routing editor.
//!
//! The tester deliberately borrows the same [`TrafficPolicy::evaluate`] and
//! failure-policy methods used by runtime callers. It has no connection,
//! storage, audit or activity handle, so evaluating a candidate cannot install
//! routes or contact a target. Callers choose a projection based on their
//! authorization: route-only users can run a test without receiving hidden rule
//! identifiers, matcher details or failure-policy consequences.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ids::{NetworkId, NodeId, TrafficRuleId};
use super::traffic_failure::{
    TrafficFailureDecision, TrafficFailurePolicy, TrafficFailureRouteContext,
};
use super::traffic_policy::{TrafficAction, TrafficDecisionReason, TrafficFlow, TrafficPolicy};
use super::{DomainError, DomainResult};

/// Where the prefilled test context came from. Activity and diagnostic
/// identifiers are stable object references, allowing the UI to implement
/// one-click prefill without copying localized display text into the contract.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TrafficPolicyTestContextSource {
    Manual,
    Activity { activity_id: String },
    Diagnostic { diagnostic_id: String },
}

impl TrafficPolicyTestContextSource {
    fn validate(&self) -> DomainResult<()> {
        match self {
            Self::Manual => Ok(()),
            Self::Activity { activity_id } => validate_object_id("activity", activity_id),
            Self::Diagnostic { diagnostic_id } => validate_object_id("diagnostic", diagnostic_id),
        }
    }
}

fn validate_object_id(kind: &str, value: &str) -> DomainResult<()> {
    if value.is_empty()
        || value.len() > 128
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(DomainError::new(
            "domain.traffic_test_context_source_invalid",
            format!("traffic test {kind} id is invalid"),
        ));
    }
    Ok(())
}

/// Complete input for a side-effect-free traffic policy test. `network_id` is
/// the network membership context in which the hypothetical flow occurs;
/// `fixed_exit` and `fixed_network` model administrator-visible hard routing
/// constraints that direct fallback must never bypass.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrafficPolicyTestInput {
    pub network_id: NetworkId,
    pub flow: TrafficFlow,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_exit: Option<NodeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_network: Option<NetworkId>,
    pub source: TrafficPolicyTestContextSource,
}

impl TrafficPolicyTestInput {
    pub fn manual(network_id: NetworkId, flow: TrafficFlow) -> Self {
        Self {
            network_id,
            flow,
            fixed_exit: None,
            fixed_network: None,
            source: TrafficPolicyTestContextSource::Manual,
        }
    }

    pub fn from_activity(
        activity_id: impl Into<String>,
        network_id: NetworkId,
        flow: TrafficFlow,
    ) -> DomainResult<Self> {
        Self::from_source(
            TrafficPolicyTestContextSource::Activity {
                activity_id: activity_id.into(),
            },
            network_id,
            flow,
        )
    }

    pub fn from_diagnostic(
        diagnostic_id: impl Into<String>,
        network_id: NetworkId,
        flow: TrafficFlow,
    ) -> DomainResult<Self> {
        Self::from_source(
            TrafficPolicyTestContextSource::Diagnostic {
                diagnostic_id: diagnostic_id.into(),
            },
            network_id,
            flow,
        )
    }

    fn from_source(
        source: TrafficPolicyTestContextSource,
        network_id: NetworkId,
        flow: TrafficFlow,
    ) -> DomainResult<Self> {
        let input = Self {
            network_id,
            flow,
            fixed_exit: None,
            fixed_network: None,
            source,
        };
        input.validate()?;
        Ok(input)
    }

    pub fn validate(&self) -> DomainResult<()> {
        if self.fixed_exit.is_some() && self.fixed_network.is_some() {
            return Err(DomainError::invalid_value(
                "traffic test cannot require both a fixed Exit and a fixed network",
            ));
        }
        self.source.validate()
    }

    /// Evaluate one policy with the production matcher and first-match
    /// evaluator. The result is projected according to `visibility` before it
    /// leaves the shared domain boundary.
    pub fn evaluate(
        &self,
        policy: &TrafficPolicy,
        visibility: TrafficPolicyTestVisibility,
    ) -> DomainResult<TrafficPolicyTestDecision> {
        self.validate()?;
        let decision = policy.evaluate(&self.flow)?;
        let route = self.failure_route(&decision.action)?;
        let failure_decisions = policy.preview_failure_policy(&route)?;
        Ok(match visibility {
            TrafficPolicyTestVisibility::Full => TrafficPolicyTestDecision::Full {
                action: decision.action,
                matched_rule: decision.matched_rule,
                reason: decision.reason,
                failure_decisions,
            },
            TrafficPolicyTestVisibility::RouteSummary => TrafficPolicyTestDecision::RouteSummary {
                action: decision.action,
            },
        })
    }

    /// Evaluate the live policy and a validated successor against the same
    /// fixed flow, then return both projections and a machine-readable delta.
    pub fn compare(
        &self,
        current: &TrafficPolicy,
        candidate: &TrafficPolicy,
        visibility: TrafficPolicyTestVisibility,
    ) -> DomainResult<TrafficPolicyTestComparison> {
        current.validate_successor(candidate)?;
        let before = self.evaluate(current, visibility)?;
        let after = self.evaluate(candidate, visibility)?;
        let route_changed = before.action() != after.action();
        let matched_rule_changed = match (&before, &after) {
            (
                TrafficPolicyTestDecision::Full {
                    matched_rule: before,
                    ..
                },
                TrafficPolicyTestDecision::Full {
                    matched_rule: after,
                    ..
                },
            ) => Some(before != after),
            _ => None,
        };
        let failure_outcomes_changed = match (&before, &after) {
            (
                TrafficPolicyTestDecision::Full {
                    failure_decisions: before,
                    ..
                },
                TrafficPolicyTestDecision::Full {
                    failure_decisions: after,
                    ..
                },
            ) => Some(before != after),
            _ => None,
        };
        let changed = route_changed
            || matched_rule_changed.unwrap_or(false)
            || failure_outcomes_changed.unwrap_or(false);
        Ok(TrafficPolicyTestComparison {
            network_id: self.network_id.clone(),
            source: self.source.clone(),
            before,
            after,
            delta: TrafficPolicyTestDelta {
                changed,
                route_changed,
                matched_rule_changed,
                failure_outcomes_changed,
            },
        })
    }

    fn failure_route(&self, action: &TrafficAction) -> DomainResult<TrafficFailureRouteContext> {
        let route = TrafficFailureRouteContext {
            requested_action: action.clone(),
            fixed_exit: self.fixed_exit.clone(),
            fixed_network: self.fixed_network.clone(),
        };
        route.validate()?;
        Ok(route)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrafficPolicyTestVisibility {
    /// Owner or administrator projection: every rule and failure consequence.
    Full,
    /// Authorized non-owner projection: final action only. Rule IDs, matcher
    /// explanations and failure-policy details are absent from serialization.
    RouteSummary,
}

/// A serialization-safe result projection. The route-summary variant physically
/// omits hidden details instead of relying on frontend masking.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "visibility", rename_all = "snake_case")]
pub enum TrafficPolicyTestDecision {
    Full {
        action: TrafficAction,
        matched_rule: Option<TrafficRuleId>,
        reason: TrafficDecisionReason,
        failure_decisions: Vec<TrafficFailureDecision>,
    },
    RouteSummary {
        action: TrafficAction,
    },
}

impl TrafficPolicyTestDecision {
    pub fn action(&self) -> &TrafficAction {
        match self {
            Self::Full { action, .. } | Self::RouteSummary { action } => action,
        }
    }

    pub fn matched_rule(&self) -> Option<&TrafficRuleId> {
        match self {
            Self::Full { matched_rule, .. } => matched_rule.as_ref(),
            Self::RouteSummary { .. } => None,
        }
    }

    pub fn failure_decisions(&self) -> Option<&[TrafficFailureDecision]> {
        match self {
            Self::Full {
                failure_decisions, ..
            } => Some(failure_decisions),
            Self::RouteSummary { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficPolicyTestDelta {
    pub changed: bool,
    pub route_changed: bool,
    /// Hidden in the route-summary projection because it would reveal whether
    /// the same private rule matched before and after the edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_rule_changed: Option<bool>,
    /// Hidden in the route-summary projection for the same reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_outcomes_changed: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficPolicyTestComparison {
    pub network_id: NetworkId,
    pub source: TrafficPolicyTestContextSource,
    pub before: TrafficPolicyTestDecision,
    pub after: TrafficPolicyTestDecision,
    pub delta: TrafficPolicyTestDelta,
}

/// Convenience type for UI previews that need to show the configured fallback
/// consequences independently of a flow decision.
pub fn preview_failure_policy(
    policy: &TrafficPolicy,
    input: &TrafficPolicyTestInput,
) -> DomainResult<Vec<TrafficFailureDecision>> {
    input.validate()?;
    let decision = policy.evaluate(&input.flow)?;
    let route = input.failure_route(&decision.action)?;
    let failure_policy: &TrafficFailurePolicy = &policy.failure_policy;
    failure_policy.preview(&route)
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use serde_json::json;

    use super::*;
    use crate::domain::application_identity::ApplicationIdentity;
    use crate::domain::traffic_policy::{
        DomainName, PortRange, TrafficDefaultConfirmedBy, TrafficDefaultPolicy, TrafficMatch,
        TrafficProtocol, TrafficRule,
    };

    fn node() -> NodeId {
        NodeId::from_public_key(b"traffic-policy-test-node")
    }

    fn network() -> NetworkId {
        NetworkId::from_public_key(b"traffic-policy-test-network")
    }

    fn exit() -> NodeId {
        NodeId::from_public_key(b"traffic-policy-test-exit")
    }

    fn rule_id() -> TrafficRuleId {
        TrafficRuleId::from_public_key(b"traffic-policy-test-rule")
    }

    fn flow() -> TrafficFlow {
        TrafficFlow {
            domain: Some(DomainName::parse("www.example.com").unwrap()),
            destination_ip: IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10)),
            destination_port: 443,
            protocol: TrafficProtocol::Tcp,
            application: ApplicationIdentity::ExecutablePath("/usr/bin/curl".into()),
        }
    }

    fn default_policy(action: TrafficAction) -> TrafficDefaultPolicy {
        TrafficDefaultPolicy {
            action,
            confirmed_by: TrafficDefaultConfirmedBy::Owner,
            confirmed_at_unix_ms: 1_700_000_000_000,
        }
    }

    fn current_policy() -> TrafficPolicy {
        TrafficPolicy {
            node_id: node(),
            revision: 1,
            default_policy: default_policy(TrafficAction::Direct),
            failure_policy: TrafficFailurePolicy::fail_closed(),
            rules: Vec::new(),
        }
    }

    fn candidate_policy() -> TrafficPolicy {
        TrafficPolicy {
            node_id: node(),
            revision: 2,
            default_policy: default_policy(TrafficAction::Direct),
            failure_policy: TrafficFailurePolicy::with_direct_fallback(
                [super::super::traffic_failure::TrafficFailureKind::ExitUnavailable],
                TrafficDefaultConfirmedBy::Administrator,
                1_700_000_000_100,
            )
            .unwrap(),
            rules: vec![TrafficRule {
                rule_id: rule_id(),
                priority: 1,
                enabled: true,
                matcher: TrafficMatch {
                    exact_domains: vec![DomainName::parse("www.example.com").unwrap()],
                    domain_suffixes: Vec::new(),
                    destination_cidrs: Vec::new(),
                    destination_ports: vec![PortRange {
                        start: 443,
                        end: 443,
                    }],
                    protocols: vec![TrafficProtocol::Tcp],
                    applications: Vec::new(),
                },
                action: TrafficAction::Exit(exit()),
                description: "snapshot route".into(),
            }],
        }
    }

    #[test]
    fn full_result_reuses_the_production_evaluator_and_failure_policy() {
        let policy = candidate_policy();
        let expected = policy.evaluate(&flow()).unwrap();
        let input = TrafficPolicyTestInput::manual(network(), flow());
        let tested = input
            .evaluate(&policy, TrafficPolicyTestVisibility::Full)
            .unwrap();

        assert_eq!(tested.action(), &expected.action);
        assert_eq!(tested.matched_rule(), expected.matched_rule.as_ref());
        let TrafficPolicyTestDecision::Full {
            reason,
            failure_decisions,
            ..
        } = tested
        else {
            panic!("full visibility must return rule and failure details");
        };
        assert_eq!(reason, expected.reason);
        assert_eq!(
            failure_decisions.len(),
            super::super::traffic_failure::TrafficFailureKind::ALL.len()
        );
        assert!(failure_decisions
            .iter()
            .all(|decision| decision.requires_audit()));
    }

    #[test]
    fn route_summary_can_test_without_serializing_hidden_policy_details() {
        let input = TrafficPolicyTestInput::manual(network(), flow());
        let tested = input
            .evaluate(
                &candidate_policy(),
                TrafficPolicyTestVisibility::RouteSummary,
            )
            .unwrap();
        assert_eq!(tested.action(), &TrafficAction::Exit(exit()));
        assert!(tested.matched_rule().is_none());
        assert!(tested.failure_decisions().is_none());

        let value = serde_json::to_value(tested).unwrap();
        assert_eq!(value["visibility"], "route_summary");
        assert!(value.get("matched_rule").is_none());
        assert!(value.get("failure_decisions").is_none());
        assert!(value.get("reason").is_none());
    }

    #[test]
    fn activity_and_diagnostic_sources_support_one_click_context_with_stable_ids() {
        let activity = TrafficPolicyTestInput::from_activity("activity-42", network(), flow())
            .expect("activity context");
        assert_eq!(
            activity.source,
            TrafficPolicyTestContextSource::Activity {
                activity_id: "activity-42".into()
            }
        );

        let diagnostic = TrafficPolicyTestInput::from_diagnostic("diag-42", network(), flow())
            .expect("diagnostic context");
        assert_eq!(
            diagnostic.source,
            TrafficPolicyTestContextSource::Diagnostic {
                diagnostic_id: "diag-42".into()
            }
        );

        let error = TrafficPolicyTestInput::from_activity("", network(), flow())
            .expect_err("empty activity id must fail");
        assert_eq!(error.code(), "domain.traffic_test_context_source_invalid");
    }

    #[test]
    fn comparison_reports_route_rule_and_failure_policy_changes() {
        let input = TrafficPolicyTestInput::manual(network(), flow());
        let comparison = input
            .compare(
                &current_policy(),
                &candidate_policy(),
                TrafficPolicyTestVisibility::Full,
            )
            .unwrap();
        assert!(comparison.delta.changed);
        assert!(comparison.delta.route_changed);
        assert_eq!(comparison.delta.matched_rule_changed, Some(true));
        assert_eq!(comparison.delta.failure_outcomes_changed, Some(true));
        assert_eq!(comparison.before.matched_rule(), None);
        assert_eq!(comparison.after.matched_rule(), Some(&rule_id()));
    }

    #[test]
    fn invalid_context_and_non_successor_candidates_fail_before_evaluation() {
        let mut input = TrafficPolicyTestInput::manual(network(), flow());
        input.fixed_exit = Some(exit());
        input.fixed_network = Some(network());
        assert_eq!(input.validate().unwrap_err().code(), "domain.invalid_value");

        let mut invalid = candidate_policy();
        invalid.revision = current_policy().revision;
        let error = TrafficPolicyTestInput::manual(network(), flow())
            .compare(
                &current_policy(),
                &invalid,
                TrafficPolicyTestVisibility::Full,
            )
            .unwrap_err();
        assert_eq!(error.code(), "domain.traffic_policy_rollback");
    }

    #[test]
    fn fixed_route_constraints_remain_visible_in_the_tester() {
        let mut input = TrafficPolicyTestInput::manual(network(), flow());
        input.fixed_exit = Some(exit());
        let tested = input
            .evaluate(&candidate_policy(), TrafficPolicyTestVisibility::Full)
            .unwrap();
        let failure_decisions = tested.failure_decisions().unwrap();
        assert!(failure_decisions.iter().all(|decision| decision.outcome
            == super::super::traffic_failure::TrafficFailureOutcome::FailClosed));
        assert!(failure_decisions.iter().all(|decision| {
            decision.reason
                == super::super::traffic_failure::TrafficFailureDecisionReason::FixedExit
        }));
    }

    #[test]
    fn fixed_sample_matches_the_regression_snapshot() {
        let input = TrafficPolicyTestInput::from_diagnostic("diag-42", network(), flow()).unwrap();
        let comparison = input
            .compare(
                &current_policy(),
                &candidate_policy(),
                TrafficPolicyTestVisibility::RouteSummary,
            )
            .unwrap();
        let actual = serde_json::to_value(comparison).unwrap();
        let expected = json!({
            "network_id": network().to_string(),
            "source": {
                "kind": "diagnostic",
                "diagnostic_id": "diag-42"
            },
            "before": {
                "visibility": "route_summary",
                "action": {
                    "kind": "direct"
                }
            },
            "after": {
                "visibility": "route_summary",
                "action": {
                    "kind": "exit",
                    "value": exit().to_string()
                }
            },
            "delta": {
                "changed": true,
                "route_changed": true
            }
        });
        assert_eq!(actual, expected);
    }
}
