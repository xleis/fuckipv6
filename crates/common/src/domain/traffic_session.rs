//! Authoritative projection of the local traffic-protection session.
//!
//! This state machine is deliberately evidence-based. A runtime may expose a
//! target Exit while it is connecting or migrating, but it must never project
//! `Protected` until the recovery coordinator has revalidated the TUN, policy
//! routes, DNS and reachability. Failure projections are terminal until a new
//! recovery pass begins.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::traffic_failure::{TrafficFailureDecision, TrafficFailureKind, TrafficFailureOutcome};
use super::{DomainError, DomainResult};

pub const TRAFFIC_SESSION_EVENT_LIMIT: usize = 64;

/// What the runtime can prove about the local traffic path right now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrafficProtectionState {
    /// No protected route has been selected yet.
    #[default]
    Idle,
    /// A route is being established and has not completed validation.
    Connecting,
    /// An existing route is being replaced and traffic is fail-closed until
    /// the replacement completes validation.
    Migrating,
    /// Fresh TUN, route, DNS and reachability evidence all passed.
    Protected,
    /// Traffic must be stopped because the selected route cannot be trusted.
    FailClosed,
    /// The owner/administrator explicitly allowed this exact failure class to
    /// use the operating-system route.
    ExplicitDirectFallback,
}

impl TrafficProtectionState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Connecting => "connecting",
            Self::Migrating => "migrating",
            Self::Protected => "protected",
            Self::FailClosed => "fail_closed",
            Self::ExplicitDirectFallback => "explicit_direct_fallback",
        }
    }

    pub const fn may_carry_traffic(self) -> bool {
        matches!(self, Self::Protected | Self::ExplicitDirectFallback)
    }
}

/// Evidence required before the session may become `Protected`.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrafficSessionValidation {
    pub tun_validated: bool,
    pub routes_validated: bool,
    pub dns_validated: bool,
    pub reachability_validated: bool,
    pub observed_at_unix_ms: u64,
}

impl TrafficSessionValidation {
    pub const fn passed(
        tun_validated: bool,
        routes_validated: bool,
        dns_validated: bool,
        reachability_validated: bool,
        observed_at_unix_ms: u64,
    ) -> Self {
        Self {
            tun_validated,
            routes_validated,
            dns_validated,
            reachability_validated,
            observed_at_unix_ms,
        }
    }

    pub const fn is_complete(&self) -> bool {
        self.tun_validated
            && self.routes_validated
            && self.dns_validated
            && self.reachability_validated
            && self.observed_at_unix_ms > 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrafficSessionEventKind {
    Connecting,
    RecoveryStarted,
    MigrationStarted,
    FailureDetected,
    ProtectionEstablished,
    FailClosed,
    DirectFallback,
}

/// A compact, transport-neutral activity record. It contains stable state and
/// reason codes only; Exit identities, addresses, resolver URLs and command
/// output stay outside this projection.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrafficSessionEvent {
    pub kind: TrafficSessionEventKind,
    pub state: TrafficProtectionState,
    pub occurred_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<TrafficFailureKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
}

/// The single source of truth consumed by GPUI, WebUI and protocol adapters.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TrafficSessionSnapshot {
    pub state: TrafficProtectionState,
    pub changed_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<TrafficFailureKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<TrafficSessionValidation>,
    pub events: Vec<TrafficSessionEvent>,
}

impl Default for TrafficSessionSnapshot {
    fn default() -> Self {
        Self {
            state: TrafficProtectionState::Idle,
            changed_at_unix_ms: 0,
            reason_code: None,
            failure: None,
            validation: None,
            events: Vec::new(),
        }
    }
}

impl TrafficSessionSnapshot {
    pub fn set_connecting(
        &mut self,
        now_unix_ms: u64,
        reason_code: impl Into<String>,
    ) -> DomainResult<()> {
        self.transition(
            TrafficProtectionState::Connecting,
            TrafficSessionEventKind::Connecting,
            now_unix_ms,
            Some(reason_code.into()),
            None,
            None,
        )
    }

    pub fn begin_recovery(
        &mut self,
        migrating: bool,
        now_unix_ms: u64,
        reason_code: impl Into<String>,
    ) -> DomainResult<()> {
        self.transition(
            if migrating {
                TrafficProtectionState::Migrating
            } else {
                TrafficProtectionState::Connecting
            },
            if migrating {
                TrafficSessionEventKind::MigrationStarted
            } else {
                TrafficSessionEventKind::RecoveryStarted
            },
            now_unix_ms,
            Some(reason_code.into()),
            None,
            None,
        )
    }

    pub fn record_failure(
        &mut self,
        failure: TrafficFailureKind,
        now_unix_ms: u64,
        reason_code: impl Into<String>,
    ) -> DomainResult<()> {
        self.transition(
            TrafficProtectionState::FailClosed,
            TrafficSessionEventKind::FailureDetected,
            now_unix_ms,
            Some(reason_code.into()),
            Some(failure),
            None,
        )
    }

    /// Apply a policy decision owned by the shared traffic-failure engine.
    pub fn apply_failure_decision(
        &mut self,
        decision: &TrafficFailureDecision,
        now_unix_ms: u64,
    ) -> DomainResult<()> {
        let (state, kind) = match decision.outcome {
            TrafficFailureOutcome::FailClosed => (
                TrafficProtectionState::FailClosed,
                TrafficSessionEventKind::FailClosed,
            ),
            TrafficFailureOutcome::DirectFallback => (
                TrafficProtectionState::ExplicitDirectFallback,
                TrafficSessionEventKind::DirectFallback,
            ),
        };
        self.transition(
            state,
            kind,
            now_unix_ms,
            Some(decision.reason.as_str().into()),
            Some(decision.failure),
            None,
        )
    }

    /// Commit a recovered route only with fresh validation for every security
    /// layer. An incomplete or future-dated observation fails closed.
    pub fn mark_protected(
        &mut self,
        validation: TrafficSessionValidation,
        now_unix_ms: u64,
    ) -> DomainResult<()> {
        if !matches!(
            self.state,
            TrafficProtectionState::Connecting | TrafficProtectionState::Migrating
        ) {
            return Err(DomainError::invalid_transition(
                "only a connecting or migrating traffic session may become protected",
            ));
        }
        if !validation.is_complete() {
            return Err(DomainError::new(
                "domain.traffic_session_validation_incomplete",
                "TUN, routes, DNS and reachability must all pass before protection is reported",
            ));
        }
        if validation.observed_at_unix_ms > now_unix_ms {
            return Err(DomainError::new(
                "domain.traffic_session_validation_future",
                "traffic-session validation cannot come from the future",
            ));
        }
        self.transition(
            TrafficProtectionState::Protected,
            TrafficSessionEventKind::ProtectionEstablished,
            now_unix_ms,
            Some("traffic.session.validation_passed".into()),
            None,
            Some(validation),
        )
    }

    pub fn enter_fail_closed(
        &mut self,
        failure: Option<TrafficFailureKind>,
        now_unix_ms: u64,
        reason_code: impl Into<String>,
    ) -> DomainResult<()> {
        self.transition(
            TrafficProtectionState::FailClosed,
            TrafficSessionEventKind::FailClosed,
            now_unix_ms,
            Some(reason_code.into()),
            failure,
            None,
        )
    }

    fn transition(
        &mut self,
        state: TrafficProtectionState,
        kind: TrafficSessionEventKind,
        now_unix_ms: u64,
        reason_code: Option<String>,
        failure: Option<TrafficFailureKind>,
        validation: Option<TrafficSessionValidation>,
    ) -> DomainResult<()> {
        if now_unix_ms == 0 {
            return Err(DomainError::invalid_value(
                "traffic-session transition time must be positive",
            ));
        }
        self.state = state;
        self.changed_at_unix_ms = now_unix_ms;
        self.reason_code = reason_code.clone();
        self.failure = failure;
        self.validation = validation;
        self.events.push(TrafficSessionEvent {
            kind,
            state,
            occurred_at_unix_ms: now_unix_ms,
            failure,
            reason_code,
        });
        if self.events.len() > TRAFFIC_SESSION_EVENT_LIMIT {
            let excess = self.events.len() - TRAFFIC_SESSION_EVENT_LIMIT;
            self.events.drain(0..excess);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        NetworkId, TrafficAction, TrafficDefaultConfirmedBy, TrafficFailurePolicy,
        TrafficFailureRouteContext,
    };
    use std::collections::BTreeSet;

    fn route_context() -> TrafficFailureRouteContext {
        TrafficFailureRouteContext::new(TrafficAction::Network(NetworkId::from_public_key(
            b"traffic-session-network",
        )))
    }

    fn decision(failure: TrafficFailureKind) -> TrafficFailureDecision {
        TrafficFailurePolicy::fail_closed()
            .decide(&route_context(), failure)
            .expect("failure decision")
    }

    #[test]
    fn migration_is_visible_and_cannot_claim_protection_before_validation() {
        let mut session = TrafficSessionSnapshot::default();
        session
            .begin_recovery(true, 100, "exit.selection.migration_pending")
            .unwrap();
        assert_eq!(session.state, TrafficProtectionState::Migrating);
        assert!(!session.state.may_carry_traffic());

        let error = session
            .mark_protected(
                TrafficSessionValidation::passed(true, true, false, true, 101),
                102,
            )
            .unwrap_err();
        assert_eq!(error.code(), "domain.traffic_session_validation_incomplete");
        assert_eq!(session.state, TrafficProtectionState::Migrating);
        assert_eq!(session.events.len(), 1);
    }

    #[test]
    fn complete_fresh_validation_is_required_to_establish_protection() {
        let mut session = TrafficSessionSnapshot::default();
        session
            .begin_recovery(false, 100, "recovery.started")
            .unwrap();
        session
            .mark_protected(
                TrafficSessionValidation::passed(true, true, true, true, 101),
                102,
            )
            .unwrap();
        assert_eq!(session.state, TrafficProtectionState::Protected);
        assert!(session.state.may_carry_traffic());
        assert_eq!(
            session.events.last().map(|event| event.kind),
            Some(TrafficSessionEventKind::ProtectionEstablished)
        );

        let mut future = TrafficSessionSnapshot::default();
        future
            .begin_recovery(false, 100, "recovery.started")
            .unwrap();
        let error = future
            .mark_protected(
                TrafficSessionValidation::passed(true, true, true, true, 200),
                150,
            )
            .unwrap_err();
        assert_eq!(error.code(), "domain.traffic_session_validation_future");
    }

    #[test]
    fn failure_decisions_authoritatively_project_fail_closed_or_explicit_direct() {
        let mut closed = TrafficSessionSnapshot::default();
        closed
            .apply_failure_decision(&decision(TrafficFailureKind::HandshakeInterrupted), 100)
            .unwrap();
        assert_eq!(closed.state, TrafficProtectionState::FailClosed);
        assert!(!closed.state.may_carry_traffic());

        let direct = TrafficFailurePolicy::with_direct_fallback(
            BTreeSet::from([TrafficFailureKind::ExitUnavailable]),
            TrafficDefaultConfirmedBy::Owner,
            99,
        )
        .unwrap()
        .decide(&route_context(), TrafficFailureKind::ExitUnavailable)
        .unwrap();
        let mut fallback = TrafficSessionSnapshot::default();
        fallback.apply_failure_decision(&direct, 101).unwrap();
        assert_eq!(
            fallback.state,
            TrafficProtectionState::ExplicitDirectFallback
        );
        assert_eq!(
            fallback.events.last().map(|event| event.kind),
            Some(TrafficSessionEventKind::DirectFallback)
        );
    }

    #[test]
    fn unsafe_new_failure_classes_cannot_enable_direct_fallback() {
        for failure in [
            TrafficFailureKind::HandshakeInterrupted,
            TrafficFailureKind::RuntimeDisconnected,
        ] {
            assert!(!failure.can_explicitly_fallback_to_direct());
            let result = TrafficFailurePolicy::with_direct_fallback(
                [failure],
                TrafficDefaultConfirmedBy::Administrator,
                100,
            );
            assert_eq!(
                result.unwrap_err().code(),
                "domain.traffic_direct_fallback_unsafe"
            );
        }
    }

    #[test]
    fn event_history_is_bounded_without_changing_the_latest_state() {
        let mut session = TrafficSessionSnapshot::default();
        for tick in 1..=(TRAFFIC_SESSION_EVENT_LIMIT as u64 + 5) {
            session
                .record_failure(
                    TrafficFailureKind::RuntimeDisconnected,
                    tick,
                    "runtime.disconnected",
                )
                .unwrap();
        }
        assert_eq!(session.events.len(), TRAFFIC_SESSION_EVENT_LIMIT);
        assert_eq!(
            session.events.last().map(|event| event.occurred_at_unix_ms),
            Some(TRAFFIC_SESSION_EVENT_LIMIT as u64 + 5)
        );
        assert_eq!(session.state, TrafficProtectionState::FailClosed);
    }

    #[test]
    fn projection_contains_no_sensitive_raw_values() {
        let mut session = TrafficSessionSnapshot::default();
        session
            .record_failure(
                TrafficFailureKind::RuntimeDisconnected,
                100,
                "runtime.disconnected",
            )
            .unwrap();
        let encoded = serde_json::to_string(&session).unwrap();
        for forbidden in [
            "203.0.113.7",
            "example.invalid",
            "/etc/resolv.conf",
            "member-private-key",
        ] {
            assert!(!encoded.contains(forbidden));
        }
        assert!(!encoded.contains("exit"));
    }
}
