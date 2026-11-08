//! Failure handling for the traffic data plane.
//!
//! Routing failures are security decisions, not implicit hints to use the host
//! route. The default is failure-closed. Direct fallback exists only when an
//! owner/administrator has explicitly confirmed the exact failure classes that
//! may use it, and route-level hard constraints always win.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::audit::{AuditEntry, AuditOutcome};
use super::ids::{AuditId, NetworkId, NodeId};
use super::traffic_policy::{TrafficAction, TrafficDefaultConfirmedBy};
use super::{DomainError, DomainResult};

/// A condition that prevents the selected route from being used.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum TrafficFailureKind {
    /// Process identity could not be established for an application condition.
    UnknownApplication,
    /// DNS resolution did not produce an answer before the deadline.
    DnsResolutionTimeout,
    /// DNS answers failed authenticity/consistency checks and may be poisoned.
    DnsPollutionDetected,
    /// The selected network or concrete Exit is no longer available.
    ExitUnavailable,
    /// No authorized network path could be selected.
    NetworkSelectionUnavailable,
    /// A connection did not establish before the policy deadline.
    ConnectionTimeout,
    /// Some attempts completed and others failed, so switching routes would be
    /// ambiguous or could leak an already-established flow.
    PartialConnectionFailure,
}

impl TrafficFailureKind {
    pub const ALL: [Self; 7] = [
        Self::UnknownApplication,
        Self::DnsResolutionTimeout,
        Self::DnsPollutionDetected,
        Self::ExitUnavailable,
        Self::NetworkSelectionUnavailable,
        Self::ConnectionTimeout,
        Self::PartialConnectionFailure,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnknownApplication => "unknown_application",
            Self::DnsResolutionTimeout => "dns_resolution_timeout",
            Self::DnsPollutionDetected => "dns_pollution_detected",
            Self::ExitUnavailable => "exit_unavailable",
            Self::NetworkSelectionUnavailable => "network_selection_unavailable",
            Self::ConnectionTimeout => "connection_timeout",
            Self::PartialConnectionFailure => "partial_connection_failure",
        }
    }

    /// Failure classes for which the user may explicitly opt into direct
    /// fallback. DNS failures cannot safely fall back because doing so would
    /// change the security properties of name resolution. A partial failure is
    /// also never switched automatically because an established flow may
    /// already have used the failed path.
    pub const fn can_explicitly_fallback_to_direct(self) -> bool {
        matches!(
            self,
            Self::UnknownApplication
                | Self::ExitUnavailable
                | Self::NetworkSelectionUnavailable
                | Self::ConnectionTimeout
        )
    }
}

/// Explicit, acknowledged permission to fall back to the operating-system
/// route for a bounded set of failure classes. The policy is fail-closed unless
/// this value is present and valid.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmedTrafficDirectFallback {
    pub allowed_failures: BTreeSet<TrafficFailureKind>,
    pub confirmed_by: TrafficDefaultConfirmedBy,
    pub confirmed_at_unix_ms: u64,
}

impl ConfirmedTrafficDirectFallback {
    pub fn validate(&self) -> DomainResult<()> {
        if self.allowed_failures.is_empty() {
            return Err(DomainError::invalid_value(
                "direct traffic fallback must allow at least one failure class",
            ));
        }
        if self.confirmed_at_unix_ms == 0 {
            return Err(DomainError::new(
                "domain.traffic_direct_fallback_unconfirmed",
                "direct traffic fallback must be explicitly confirmed",
            ));
        }
        if self
            .allowed_failures
            .iter()
            .any(|failure| !failure.can_explicitly_fallback_to_direct())
        {
            return Err(DomainError::new(
                "domain.traffic_direct_fallback_unsafe",
                "one or more traffic failures must always fail closed",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TrafficFailurePolicy {
    /// Absence is the fail-closed default and is not shown as confirmation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direct_fallback: Option<ConfirmedTrafficDirectFallback>,
}

impl TrafficFailurePolicy {
    pub fn fail_closed() -> Self {
        Self::default()
    }

    pub fn with_direct_fallback(
        allowed_failures: impl IntoIterator<Item = TrafficFailureKind>,
        confirmed_by: TrafficDefaultConfirmedBy,
        confirmed_at_unix_ms: u64,
    ) -> DomainResult<Self> {
        let policy = Self {
            direct_fallback: Some(ConfirmedTrafficDirectFallback {
                allowed_failures: allowed_failures.into_iter().collect(),
                confirmed_by,
                confirmed_at_unix_ms,
            }),
        };
        policy.validate()?;
        Ok(policy)
    }

    pub fn validate(&self) -> DomainResult<()> {
        if let Some(fallback) = &self.direct_fallback {
            fallback.validate()?;
        }
        Ok(())
    }

    /// Deterministically evaluate one runtime failure. UI previews use this
    /// same method for every failure kind, so the displayed consequence and
    /// the runtime result cannot diverge.
    pub fn decide(
        &self,
        route: &TrafficFailureRouteContext,
        failure: TrafficFailureKind,
    ) -> DomainResult<TrafficFailureDecision> {
        self.validate()?;
        route.validate()?;

        let (outcome, reason) = if matches!(route.requested_action, TrafficAction::Reject) {
            (
                TrafficFailureOutcome::FailClosed,
                TrafficFailureDecisionReason::RejectRule,
            )
        } else if route.fixed_exit.is_some() {
            (
                TrafficFailureOutcome::FailClosed,
                TrafficFailureDecisionReason::FixedExit,
            )
        } else if route.fixed_network.is_some() {
            (
                TrafficFailureOutcome::FailClosed,
                TrafficFailureDecisionReason::FixedNetwork,
            )
        } else if matches!(route.requested_action, TrafficAction::Direct) {
            (
                TrafficFailureOutcome::FailClosed,
                TrafficFailureDecisionReason::DirectActionHasNoAlternate,
            )
        } else if !failure.can_explicitly_fallback_to_direct() {
            (
                TrafficFailureOutcome::FailClosed,
                TrafficFailureDecisionReason::UnsafeFailureClass,
            )
        } else if let Some(fallback) = &self.direct_fallback {
            if fallback.allowed_failures.contains(&failure) {
                (
                    TrafficFailureOutcome::DirectFallback,
                    TrafficFailureDecisionReason::ExplicitDirectFallback,
                )
            } else {
                (
                    TrafficFailureOutcome::FailClosed,
                    TrafficFailureDecisionReason::NotAllowedByPolicy,
                )
            }
        } else {
            (
                TrafficFailureOutcome::FailClosed,
                TrafficFailureDecisionReason::DefaultFailClosed,
            )
        };

        Ok(TrafficFailureDecision {
            failure,
            outcome,
            reason,
            consequence_key: consequence_key(outcome, reason).to_owned(),
            direct_fallback: if outcome == TrafficFailureOutcome::DirectFallback {
                self.direct_fallback.clone()
            } else {
                None
            },
            recovery_action: TrafficFailureRecoveryAction::RetryRequestedRoute,
        })
    }

    /// Preview every failure class before applying a policy. Each result must be
    /// shown to the user; an explicit direct fallback is never a hidden mode.
    pub fn preview(
        &self,
        route: &TrafficFailureRouteContext,
    ) -> DomainResult<Vec<TrafficFailureDecision>> {
        self.validate()?;
        TrafficFailureKind::ALL
            .into_iter()
            .map(|failure| self.decide(route, failure))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrafficFailureRouteContext {
    /// The action selected by the ordered policy before runtime failure.
    pub requested_action: TrafficAction,
    /// A network administrator's fixed Exit. Any direct fallback would bypass
    /// this hard constraint and is therefore forbidden.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_exit: Option<NodeId>,
    /// A fixed network selection with the same non-bypass semantics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_network: Option<NetworkId>,
}

impl TrafficFailureRouteContext {
    pub fn new(requested_action: TrafficAction) -> Self {
        Self {
            requested_action,
            fixed_exit: None,
            fixed_network: None,
        }
    }

    pub fn validate(&self) -> DomainResult<()> {
        if let Some(fixed_exit) = &self.fixed_exit {
            if matches!(&self.requested_action, TrafficAction::Exit(exit) if exit != fixed_exit) {
                return Err(DomainError::invariant(
                    "requested Exit conflicts with the fixed Exit",
                ));
            }
        }
        if let Some(fixed_network) = &self.fixed_network {
            if matches!(
                &self.requested_action,
                TrafficAction::Network(network) if network != fixed_network
            ) {
                return Err(DomainError::invariant(
                    "requested network conflicts with the fixed network",
                ));
            }
        }
        Ok(())
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum TrafficFailureOutcome {
    FailClosed,
    DirectFallback,
}

impl TrafficFailureOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FailClosed => "fail_closed",
            Self::DirectFallback => "direct_fallback",
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum TrafficFailureDecisionReason {
    DefaultFailClosed,
    NotAllowedByPolicy,
    ExplicitDirectFallback,
    UnsafeFailureClass,
    RejectRule,
    FixedExit,
    FixedNetwork,
    DirectActionHasNoAlternate,
}

impl TrafficFailureDecisionReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DefaultFailClosed => "default_fail_closed",
            Self::NotAllowedByPolicy => "not_allowed_by_policy",
            Self::ExplicitDirectFallback => "explicit_direct_fallback",
            Self::UnsafeFailureClass => "unsafe_failure_class",
            Self::RejectRule => "reject_rule",
            Self::FixedExit => "fixed_exit",
            Self::FixedNetwork => "fixed_network",
            Self::DirectActionHasNoAlternate => "direct_action_has_no_alternate",
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum TrafficFailureRecoveryAction {
    RetryRequestedRoute,
}

/// A complete, serializable failure decision. `consequence_key` is a stable
/// localization key that both GPUI and WebUI must display before policy apply;
/// no localized prose is duplicated in the domain layer.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficFailureDecision {
    pub failure: TrafficFailureKind,
    pub outcome: TrafficFailureOutcome,
    pub reason: TrafficFailureDecisionReason,
    pub consequence_key: String,
    /// Present only for an explicitly confirmed direct fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct_fallback: Option<ConfirmedTrafficDirectFallback>,
    pub recovery_action: TrafficFailureRecoveryAction,
}

impl TrafficFailureDecision {
    pub fn requires_audit(&self) -> bool {
        // Every failure decision is auditable. In particular, every direct
        // fallback record is mandatory and cannot be represented as an
        // ephemeral UI-only event.
        true
    }

    pub fn runtime_degradation(&self) -> TrafficRuntimeDegradation {
        TrafficRuntimeDegradation {
            state: match self.outcome {
                TrafficFailureOutcome::FailClosed => {
                    TrafficRuntimeDegradationState::DegradedFailClosed
                }
                TrafficFailureOutcome::DirectFallback => {
                    TrafficRuntimeDegradationState::DegradedDirectFallback
                }
            },
            failure: self.failure,
            consequence_key: self.consequence_key.clone(),
            recovery_action: self.recovery_action,
            audit_required: self.requires_audit(),
        }
    }

    /// Build the durable audit record for this decompensation. Callers must
    /// append the returned entry before presenting the degraded state as
    /// complete.
    pub fn audit_entry(&self, context: TrafficFailureAuditContext) -> DomainResult<AuditEntry> {
        context.validate()?;
        let mut details = BTreeMap::new();
        details.insert("failure_kind".into(), self.failure.as_str().into());
        details.insert("outcome".into(), self.outcome.as_str().into());
        details.insert("reason_code".into(), self.reason.as_str().into());
        details.insert("consequence_key".into(), self.consequence_key.clone());
        details.insert(
            "route_action".into(),
            serde_json::to_string(&context.requested_action).map_err(|_| {
                DomainError::invalid_value("traffic failure route action cannot be encoded")
            })?,
        );
        details.insert("audit_required".into(), "true".into());
        if let Some(exit) = &context.fixed_exit {
            details.insert("fixed_exit".into(), exit.to_string());
        }
        if let Some(network) = &context.fixed_network {
            details.insert("fixed_network".into(), network.to_string());
        }
        if let Some(fallback) = &self.direct_fallback {
            details.insert(
                "direct_fallback_confirmed_by".into(),
                match fallback.confirmed_by {
                    TrafficDefaultConfirmedBy::Owner => "owner".into(),
                    TrafficDefaultConfirmedBy::Administrator => "administrator".into(),
                },
            );
            details.insert(
                "direct_fallback_confirmed_at_unix_ms".into(),
                fallback.confirmed_at_unix_ms.to_string(),
            );
        }
        if let Some(diagnostic_id) = &context.diagnostic_id {
            details.insert("diagnostic_id".into(), diagnostic_id.clone());
        }

        let entry = AuditEntry {
            audit_id: context.audit_id,
            operation_id: None,
            actor: context.actor,
            network_id: context.network_id,
            action: match self.outcome {
                TrafficFailureOutcome::FailClosed => "traffic.failure.fail_closed",
                TrafficFailureOutcome::DirectFallback => "traffic.failure.direct_fallback",
            }
            .into(),
            resource: "traffic-policy:local".into(),
            outcome: match self.outcome {
                TrafficFailureOutcome::FailClosed => AuditOutcome::Failed,
                TrafficFailureOutcome::DirectFallback => AuditOutcome::Succeeded,
            },
            occurred_at_unix_ms: context.occurred_at_unix_ms,
            summary: match self.outcome {
                TrafficFailureOutcome::FailClosed => {
                    "Traffic was stopped because its selected route failed"
                }
                TrafficFailureOutcome::DirectFallback => {
                    "Traffic used an explicitly confirmed direct fallback"
                }
            }
            .into(),
            details,
            sensitive_keys: BTreeSet::new(),
        };
        entry.validate()?;
        Ok(entry)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrafficFailureAuditContext {
    pub audit_id: AuditId,
    pub actor: NodeId,
    pub network_id: Option<NetworkId>,
    pub occurred_at_unix_ms: u64,
    pub requested_action: TrafficAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_exit: Option<NodeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_network: Option<NetworkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic_id: Option<String>,
}

impl TrafficFailureAuditContext {
    fn validate(&self) -> DomainResult<()> {
        TrafficFailureRouteContext {
            requested_action: self.requested_action.clone(),
            fixed_exit: self.fixed_exit.clone(),
            fixed_network: self.fixed_network.clone(),
        }
        .validate()?;
        if self.occurred_at_unix_ms == 0 {
            return Err(DomainError::invalid_value(
                "traffic failure occurrence time must be positive",
            ));
        }
        if self
            .diagnostic_id
            .as_ref()
            .is_some_and(|id| id.is_empty() || id.len() > 128)
        {
            return Err(DomainError::invalid_value(
                "traffic failure diagnostic id is invalid",
            ));
        }
        Ok(())
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum TrafficRuntimeDegradationState {
    DegradedFailClosed,
    DegradedDirectFallback,
}

/// A visible runtime state that must accompany any dynamic downgrade. It is
/// intentionally separate from `TrafficPolicy` so a UI cannot mistake
/// configured policy for current health.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficRuntimeDegradation {
    pub state: TrafficRuntimeDegradationState,
    pub failure: TrafficFailureKind,
    pub consequence_key: String,
    pub recovery_action: TrafficFailureRecoveryAction,
    pub audit_required: bool,
}

fn consequence_key(
    outcome: TrafficFailureOutcome,
    reason: TrafficFailureDecisionReason,
) -> &'static str {
    match (outcome, reason) {
        (TrafficFailureOutcome::DirectFallback, _) => "traffic.failure.consequence.direct_fallback",
        (TrafficFailureOutcome::FailClosed, TrafficFailureDecisionReason::RejectRule) => {
            "traffic.failure.consequence.reject_rule"
        }
        (TrafficFailureOutcome::FailClosed, TrafficFailureDecisionReason::FixedExit) => {
            "traffic.failure.consequence.fixed_exit"
        }
        (TrafficFailureOutcome::FailClosed, TrafficFailureDecisionReason::FixedNetwork) => {
            "traffic.failure.consequence.fixed_network"
        }
        (TrafficFailureOutcome::FailClosed, TrafficFailureDecisionReason::UnsafeFailureClass) => {
            "traffic.failure.consequence.unsafe_failure"
        }
        (
            TrafficFailureOutcome::FailClosed,
            TrafficFailureDecisionReason::DirectActionHasNoAlternate,
        ) => "traffic.failure.consequence.direct_action",
        (TrafficFailureOutcome::FailClosed, TrafficFailureDecisionReason::NotAllowedByPolicy) => {
            "traffic.failure.consequence.not_allowed"
        }
        (TrafficFailureOutcome::FailClosed, TrafficFailureDecisionReason::DefaultFailClosed) => {
            "traffic.failure.consequence.default_fail_closed"
        }
        (
            TrafficFailureOutcome::FailClosed,
            TrafficFailureDecisionReason::ExplicitDirectFallback,
        ) => {
            unreachable!("explicit direct fallback must use the direct outcome")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node() -> NodeId {
        NodeId::from_public_key(b"traffic-failure-node")
    }

    fn network() -> NetworkId {
        NetworkId::from_public_key(b"traffic-failure-network")
    }

    fn exit() -> NodeId {
        NodeId::from_public_key(b"traffic-failure-exit")
    }

    fn direct_policy(
        failures: impl IntoIterator<Item = TrafficFailureKind>,
    ) -> TrafficFailurePolicy {
        TrafficFailurePolicy::with_direct_fallback(
            failures,
            TrafficDefaultConfirmedBy::Owner,
            1_700_000_000_000,
        )
        .unwrap()
    }

    fn route(action: TrafficAction) -> TrafficFailureRouteContext {
        TrafficFailureRouteContext::new(action)
    }

    #[test]
    fn default_policy_fails_closed_for_all_failure_classes() {
        let policy = TrafficFailurePolicy::fail_closed();
        let route = route(TrafficAction::Exit(exit()));
        for failure in TrafficFailureKind::ALL {
            let decision = policy.decide(&route, failure).unwrap();
            assert_eq!(decision.outcome, TrafficFailureOutcome::FailClosed);
            assert!(decision.direct_fallback.is_none());
            assert!(decision.requires_audit());
        }
        assert_eq!(
            policy
                .decide(&route, TrafficFailureKind::UnknownApplication)
                .unwrap()
                .reason,
            TrafficFailureDecisionReason::DefaultFailClosed
        );
    }

    #[test]
    fn direct_fallback_requires_explicit_confirmation_and_only_allowed_failures() {
        let empty = TrafficFailurePolicy::with_direct_fallback(
            [],
            TrafficDefaultConfirmedBy::Administrator,
            1_700_000_000_000,
        );
        assert_eq!(empty.unwrap_err().code(), "domain.invalid_value");

        let unconfirmed = TrafficFailurePolicy::with_direct_fallback(
            [TrafficFailureKind::ExitUnavailable],
            TrafficDefaultConfirmedBy::Owner,
            0,
        );
        assert_eq!(
            unconfirmed.unwrap_err().code(),
            "domain.traffic_direct_fallback_unconfirmed"
        );

        let policy = direct_policy([TrafficFailureKind::ExitUnavailable]);
        let allowed = policy
            .decide(
                &route(TrafficAction::Exit(exit())),
                TrafficFailureKind::ExitUnavailable,
            )
            .unwrap();
        assert_eq!(allowed.outcome, TrafficFailureOutcome::DirectFallback);
        assert_eq!(
            allowed.reason,
            TrafficFailureDecisionReason::ExplicitDirectFallback
        );
        assert!(allowed.direct_fallback.is_some());

        let denied = policy
            .decide(
                &route(TrafficAction::Exit(exit())),
                TrafficFailureKind::ConnectionTimeout,
            )
            .unwrap();
        assert_eq!(denied.outcome, TrafficFailureOutcome::FailClosed);
        assert_eq!(
            denied.reason,
            TrafficFailureDecisionReason::NotAllowedByPolicy
        );
    }

    #[test]
    fn unsafe_failure_classes_can_never_be_configured_for_direct_fallback() {
        for failure in [
            TrafficFailureKind::DnsResolutionTimeout,
            TrafficFailureKind::DnsPollutionDetected,
            TrafficFailureKind::PartialConnectionFailure,
        ] {
            assert!(!failure.can_explicitly_fallback_to_direct());
            let result = TrafficFailurePolicy::with_direct_fallback(
                [failure],
                TrafficDefaultConfirmedBy::Owner,
                1_700_000_000_000,
            );
            assert_eq!(
                result.unwrap_err().code(),
                "domain.traffic_direct_fallback_unsafe"
            );
        }
    }

    #[test]
    fn fixed_exit_reject_and_fixed_network_cannot_be_bypassed_by_fallback() {
        let policy = direct_policy([
            TrafficFailureKind::ExitUnavailable,
            TrafficFailureKind::NetworkSelectionUnavailable,
            TrafficFailureKind::ConnectionTimeout,
        ]);

        let mut fixed_exit = route(TrafficAction::Exit(exit()));
        fixed_exit.fixed_exit = Some(exit());
        let fixed_exit_decision = policy
            .decide(&fixed_exit, TrafficFailureKind::ExitUnavailable)
            .unwrap();
        assert_eq!(
            fixed_exit_decision.outcome,
            TrafficFailureOutcome::FailClosed
        );
        assert_eq!(
            fixed_exit_decision.reason,
            TrafficFailureDecisionReason::FixedExit
        );

        let reject = route(TrafficAction::Reject);
        let reject_decision = policy
            .decide(&reject, TrafficFailureKind::ConnectionTimeout)
            .unwrap();
        assert_eq!(reject_decision.outcome, TrafficFailureOutcome::FailClosed);
        assert_eq!(
            reject_decision.reason,
            TrafficFailureDecisionReason::RejectRule
        );

        let mut fixed_network = route(TrafficAction::Network(network()));
        fixed_network.fixed_network = Some(network());
        let network_decision = policy
            .decide(
                &fixed_network,
                TrafficFailureKind::NetworkSelectionUnavailable,
            )
            .unwrap();
        assert_eq!(
            network_decision.reason,
            TrafficFailureDecisionReason::FixedNetwork
        );
        assert_eq!(network_decision.outcome, TrafficFailureOutcome::FailClosed);
    }

    #[test]
    fn ui_preview_exposes_direct_fallback_and_hard_constraints_before_apply() {
        let policy = direct_policy([
            TrafficFailureKind::ExitUnavailable,
            TrafficFailureKind::ConnectionTimeout,
        ]);
        let preview = policy.preview(&route(TrafficAction::Exit(exit()))).unwrap();
        assert_eq!(preview.len(), TrafficFailureKind::ALL.len());
        assert!(preview.iter().all(TrafficFailureDecision::requires_audit));
        assert!(preview.iter().all(|decision| {
            decision
                .consequence_key
                .starts_with("traffic.failure.consequence.")
        }));
        assert_eq!(
            preview
                .iter()
                .find(|decision| decision.failure == TrafficFailureKind::ExitUnavailable)
                .unwrap()
                .outcome,
            TrafficFailureOutcome::DirectFallback
        );
        assert_eq!(
            preview
                .iter()
                .find(|decision| decision.failure == TrafficFailureKind::DnsPollutionDetected)
                .unwrap()
                .outcome,
            TrafficFailureOutcome::FailClosed
        );

        let mut fixed_exit = route(TrafficAction::Exit(exit()));
        fixed_exit.fixed_exit = Some(exit());
        let constrained = policy.preview(&fixed_exit).unwrap();
        assert!(constrained
            .iter()
            .all(|decision| decision.outcome == TrafficFailureOutcome::FailClosed));
    }

    #[test]
    fn dns_pollution_timeout_and_partial_connection_failure_are_distinguished() {
        let policy = direct_policy([TrafficFailureKind::ConnectionTimeout]);
        let route = route(TrafficAction::Exit(exit()));

        let timeout = policy
            .decide(&route, TrafficFailureKind::ConnectionTimeout)
            .unwrap();
        assert_eq!(timeout.outcome, TrafficFailureOutcome::DirectFallback);
        assert_eq!(
            timeout.consequence_key,
            "traffic.failure.consequence.direct_fallback"
        );

        for failure in [
            TrafficFailureKind::DnsPollutionDetected,
            TrafficFailureKind::PartialConnectionFailure,
        ] {
            let decision = policy.decide(&route, failure).unwrap();
            assert_eq!(decision.outcome, TrafficFailureOutcome::FailClosed);
            assert_eq!(
                decision.consequence_key,
                "traffic.failure.consequence.unsafe_failure"
            );
        }
    }

    #[test]
    fn direct_fallback_and_fail_closed_events_become_visible_activity_audit() {
        let policy = direct_policy([TrafficFailureKind::ExitUnavailable]);
        let route = route(TrafficAction::Exit(exit()));
        let fallback = policy
            .decide(&route, TrafficFailureKind::ExitUnavailable)
            .unwrap();
        let fallback_entry = fallback
            .audit_entry(TrafficFailureAuditContext {
                audit_id: AuditId::from_public_key(b"fallback-audit"),
                actor: node(),
                network_id: Some(network()),
                occurred_at_unix_ms: 1_700_000_000_001,
                requested_action: route.requested_action.clone(),
                fixed_exit: None,
                fixed_network: None,
                diagnostic_id: Some("diag-fallback-1".into()),
            })
            .unwrap();
        assert_eq!(fallback_entry.action, "traffic.failure.direct_fallback");
        assert_eq!(fallback_entry.outcome, AuditOutcome::Succeeded);
        assert_eq!(
            fallback_entry
                .details
                .get("failure_kind")
                .map(String::as_str),
            Some("exit_unavailable")
        );
        let activity = fallback_entry.project_activity();
        assert_eq!(
            activity.category,
            super::super::audit::ActivityCategory::SystemEvent
        );
        assert_eq!(
            activity.severity,
            super::super::audit::ActivitySeverity::Warning
        );

        let closed = policy
            .decide(&route, TrafficFailureKind::DnsPollutionDetected)
            .unwrap();
        let closed_entry = closed
            .audit_entry(TrafficFailureAuditContext {
                audit_id: AuditId::from_public_key(b"closed-audit"),
                actor: node(),
                network_id: Some(network()),
                occurred_at_unix_ms: 1_700_000_000_002,
                requested_action: route.requested_action,
                fixed_exit: None,
                fixed_network: None,
                diagnostic_id: None,
            })
            .unwrap();
        assert_eq!(closed_entry.action, "traffic.failure.fail_closed");
        assert_eq!(closed_entry.outcome, AuditOutcome::Failed);
        assert!(closed_entry.details.contains_key("reason_code"));
        let closed_activity = closed_entry.project_activity();
        assert_eq!(
            closed_activity.category,
            super::super::audit::ActivityCategory::SystemEvent
        );
        assert_eq!(
            closed_activity.severity,
            super::super::audit::ActivitySeverity::Warning
        );
    }

    #[test]
    fn runtime_degradation_is_visible_and_recoverable() {
        let direct = direct_policy([TrafficFailureKind::ExitUnavailable])
            .decide(
                &route(TrafficAction::Exit(exit())),
                TrafficFailureKind::ExitUnavailable,
            )
            .unwrap()
            .runtime_degradation();
        assert_eq!(
            direct.state,
            TrafficRuntimeDegradationState::DegradedDirectFallback
        );
        assert!(direct.audit_required);
        assert_eq!(
            direct.recovery_action,
            TrafficFailureRecoveryAction::RetryRequestedRoute
        );

        let closed = TrafficFailurePolicy::fail_closed()
            .decide(
                &route(TrafficAction::Exit(exit())),
                TrafficFailureKind::ExitUnavailable,
            )
            .unwrap()
            .runtime_degradation();
        assert_eq!(
            closed.state,
            TrafficRuntimeDegradationState::DegradedFailClosed
        );
        assert_eq!(
            closed.consequence_key,
            "traffic.failure.consequence.default_fail_closed"
        );
    }
}
