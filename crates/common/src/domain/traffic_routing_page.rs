//! Transport-neutral page model for the authoritative traffic-routing editor.
//!
//! GPUI, WebUI and Android consume this model instead of independently
//! interpreting legacy network configuration. The model keeps the applied
//! revision, draft/apply workflow, structured matchers, stable target
//! references, ordering affordances and removal impact in one contract. It
//! performs no host networking and cannot claim that a runtime adapter has
//! installed a policy.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ids::{NetworkId, NodeId, TrafficRuleId};
use super::traffic_failure::TrafficFailurePolicy;
use super::traffic_policy::{
    TrafficAction, TrafficDecisionReason, TrafficDefaultConfirmedBy, TrafficFlow, TrafficMatch,
    TrafficMatchRisk, TrafficPolicy, TrafficRule,
};
use super::{DomainError, DomainResult};

/// The editor workflow state. `Draft`, `Applying`, `Applied`, `Failed` and
/// `Stale` are intentionally separate so an adapter cannot infer success from
/// an editable document or from the mere presence of a revision.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TrafficRoutingWorkflowState {
    /// No authoritative traffic policy has been loaded yet.
    Unconfigured {
        reason_code: String,
    },
    Draft {
        base_revision: u64,
    },
    Applying {
        base_revision: u64,
        candidate_revision: u64,
        operation_id: String,
    },
    Applied {
        revision: u64,
    },
    Failed {
        base_revision: u64,
        candidate_revision: u64,
        stage: String,
        reason_code: String,
    },
    Stale {
        revision: u64,
        reason_code: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrafficRoutingWorkflowStatus {
    Unconfigured,
    Draft,
    Applying,
    Applied,
    Failed,
    Stale,
}

impl TrafficRoutingWorkflowState {
    pub const fn status(&self) -> TrafficRoutingWorkflowStatus {
        match self {
            Self::Unconfigured { .. } => TrafficRoutingWorkflowStatus::Unconfigured,
            Self::Draft { .. } => TrafficRoutingWorkflowStatus::Draft,
            Self::Applying { .. } => TrafficRoutingWorkflowStatus::Applying,
            Self::Applied { .. } => TrafficRoutingWorkflowStatus::Applied,
            Self::Failed { .. } => TrafficRoutingWorkflowStatus::Failed,
            Self::Stale { .. } => TrafficRoutingWorkflowStatus::Stale,
        }
    }

    pub fn validate(&self) -> DomainResult<()> {
        match self {
            Self::Unconfigured { reason_code } => validate_code(reason_code),
            Self::Draft { base_revision } => validate_revision(*base_revision),
            Self::Applying {
                base_revision,
                candidate_revision,
                operation_id,
            } => {
                validate_revision(*base_revision)?;
                validate_candidate_revision(*base_revision, *candidate_revision)?;
                validate_identifier("operation", operation_id)
            }
            Self::Applied { revision } => validate_revision(*revision),
            Self::Failed {
                base_revision,
                candidate_revision,
                stage,
                reason_code,
            } => {
                validate_revision(*base_revision)?;
                validate_candidate_revision(*base_revision, *candidate_revision)?;
                validate_identifier("failure stage", stage)?;
                validate_code(reason_code)
            }
            Self::Stale {
                revision,
                reason_code,
            } => {
                validate_revision(*revision)?;
                validate_code(reason_code)
            }
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum TrafficRoutingTargetKind {
    Direct,
    Network,
    Exit,
    Reject,
}

/// A navigable reference to a routing target. Labels are display hints; stable
/// IDs and `navigation` are the authority used by every adapter.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficRoutingTarget {
    pub kind: TrafficRoutingTargetKind,
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub search_terms: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub navigation: Option<TrafficRoutingNavigation>,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficRoutingNavigation {
    pub page_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_name: Option<String>,
    pub object_id: String,
}

impl TrafficRoutingTarget {
    pub fn direct() -> Self {
        Self {
            kind: TrafficRoutingTargetKind::Direct,
            id: "direct".into(),
            label: "Direct".into(),
            search_terms: vec!["internet".into(), "local".into()],
            navigation: None,
        }
    }

    pub fn reject() -> Self {
        Self {
            kind: TrafficRoutingTargetKind::Reject,
            id: "reject".into(),
            label: "Reject".into(),
            search_terms: vec!["block".into(), "deny".into()],
            navigation: None,
        }
    }

    pub fn network(network_id: NetworkId, network_name: impl Into<String>) -> Self {
        let network_name = network_name.into();
        Self {
            kind: TrafficRoutingTargetKind::Network,
            id: network_id.as_str().to_owned(),
            label: network_name.clone(),
            search_terms: vec![network_name.clone()],
            navigation: Some(TrafficRoutingNavigation {
                page_id: "routing".into(),
                network_name: Some(network_name),
                object_id: network_id.as_str().to_owned(),
            }),
        }
    }

    pub fn exit(node_id: NodeId, label: impl Into<String>) -> Self {
        let label = label.into();
        Self {
            kind: TrafficRoutingTargetKind::Exit,
            id: node_id.as_str().to_owned(),
            label: label.clone(),
            search_terms: vec![label],
            navigation: Some(TrafficRoutingNavigation {
                page_id: "topology".into(),
                network_name: None,
                object_id: node_id.as_str().to_owned(),
            }),
        }
    }

    fn matches_query(&self, query: &str) -> bool {
        self.id.to_lowercase().contains(query)
            || self.label.to_lowercase().contains(query)
            || self
                .search_terms
                .iter()
                .any(|term| term.to_lowercase().contains(query))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficRoutingTargetRef {
    pub kind: TrafficRoutingTargetKind,
    pub id: String,
    pub resolved: bool,
}

impl TrafficRoutingTargetRef {
    fn resolve(action: &TrafficAction, targets: &[TrafficRoutingTarget]) -> Self {
        let (kind, id) = match action {
            TrafficAction::Direct => (TrafficRoutingTargetKind::Direct, "direct".to_owned()),
            TrafficAction::Network(network_id) => (
                TrafficRoutingTargetKind::Network,
                network_id.as_str().to_owned(),
            ),
            TrafficAction::Exit(node_id) => {
                (TrafficRoutingTargetKind::Exit, node_id.as_str().to_owned())
            }
            TrafficAction::Reject => (TrafficRoutingTargetKind::Reject, "reject".to_owned()),
        };
        Self {
            resolved: targets
                .iter()
                .any(|target| target.kind == kind && target.id == id),
            kind,
            id,
        }
    }
}

/// Hard routing constraints shown alongside the policy. They are not editable
/// policy rules and must never be bypassed by direct fallback.
#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficRoutingHardConstraints {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_exit: Option<NodeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_network: Option<NetworkId>,
}

impl TrafficRoutingHardConstraints {
    pub fn validate(&self) -> DomainResult<()> {
        if self.fixed_exit.is_some() && self.fixed_network.is_some() {
            return Err(DomainError::invalid_value(
                "traffic routing cannot fix both an Exit and a network",
            ));
        }
        Ok(())
    }

    pub fn target_ref(&self, targets: &[TrafficRoutingTarget]) -> Option<TrafficRoutingTargetRef> {
        self.fixed_exit
            .as_ref()
            .map(|node_id| {
                TrafficRoutingTargetRef::resolve(&TrafficAction::Exit(node_id.clone()), targets)
            })
            .or_else(|| {
                self.fixed_network.as_ref().map(|network_id| {
                    TrafficRoutingTargetRef::resolve(
                        &TrafficAction::Network(network_id.clone()),
                        targets,
                    )
                })
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrafficRoutingRuleDirection {
    Up,
    Down,
}

impl TrafficRoutingRuleDirection {
    pub const fn action_id(self) -> &'static str {
        match self {
            Self::Up => "move_traffic_rule_up",
            Self::Down => "move_traffic_rule_down",
        }
    }

    pub const fn keyboard_shortcut(self) -> &'static str {
        match self {
            Self::Up => "Alt+ArrowUp",
            Self::Down => "Alt+ArrowDown",
        }
    }
}

/// One equivalent pointer/keyboard/button action. Adapters bind drag-and-drop
/// and keyboard gestures to the same action ID rather than implementing a
/// second ordering path.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficRoutingMoveAction {
    pub action_id: String,
    pub rule_id: TrafficRuleId,
    pub direction: TrafficRoutingRuleDirection,
    pub from_index: usize,
    pub target_index: usize,
    pub keyboard_shortcut: String,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficRoutingRuleView {
    pub rule_id: TrafficRuleId,
    pub priority: u32,
    pub enabled: bool,
    pub description: String,
    pub matcher: TrafficMatch,
    pub action: TrafficAction,
    pub target: TrafficRoutingTargetRef,
    pub risks: Vec<TrafficMatchRisk>,
    /// Authoritative consequence of disabling this rule. The default keeps old
    /// snapshots honest: absence of observation data is unavailable, not zero
    /// hits.
    #[serde(default)]
    pub disable_impact: TrafficRoutingRuleImpactView,
    /// Authoritative consequence of deleting this rule.
    #[serde(default)]
    pub delete_impact: TrafficRoutingRuleImpactView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub move_up: Option<TrafficRoutingMoveAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub move_down: Option<TrafficRoutingMoveAction>,
}

/// A bounded observation window produced by the real traffic data plane. Only
/// samples that the runtime actually observed are present here; adapters must
/// never synthesize hits from tests, fixtures or configuration.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TrafficRoutingObservation {
    Available {
        window_start_unix_ms: u64,
        window_end_unix_ms: u64,
        /// Total observed flows in the window, including flows outside the
        /// bounded sample list.
        observed_flow_count: usize,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        hits: Vec<TrafficRoutingObservedHit>,
    },
    Unavailable {
        reason_code: String,
    },
}

impl Default for TrafficRoutingObservation {
    fn default() -> Self {
        Self::unavailable("runtime.traffic_observation_unavailable")
    }
}

impl TrafficRoutingObservation {
    pub fn unavailable(reason_code: impl Into<String>) -> Self {
        Self::Unavailable {
            reason_code: reason_code.into(),
        }
    }

    fn validate(&self) -> DomainResult<()> {
        match self {
            Self::Available {
                window_start_unix_ms,
                window_end_unix_ms,
                observed_flow_count,
                hits,
            } => {
                if *window_start_unix_ms == 0 || *window_end_unix_ms < *window_start_unix_ms {
                    return Err(DomainError::invalid_value(
                        "traffic observation window is invalid",
                    ));
                }
                if *observed_flow_count < hits.len() {
                    return Err(DomainError::invalid_value(
                        "traffic observation sample count exceeds observed flows",
                    ));
                }
                let mut sample_ids = std::collections::BTreeSet::new();
                for hit in hits {
                    validate_identifier("hit sample", &hit.sample_id)?;
                    if !sample_ids.insert(&hit.sample_id) {
                        return Err(DomainError::invariant(
                            "traffic observation contains duplicate hit samples",
                        ));
                    }
                    if let Some(activity_id) = &hit.activity_id {
                        validate_identifier("activity", activity_id)?;
                    }
                    if let Some(diagnostic_id) = &hit.diagnostic_id {
                        validate_identifier("diagnostic", diagnostic_id)?;
                    }
                }
                Ok(())
            }
            Self::Unavailable { reason_code } => validate_code(reason_code),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TrafficRoutingRuleImpactView {
    Available { impact: TrafficRoutingRuleImpact },
    Unavailable { reason_code: String },
}

impl Default for TrafficRoutingRuleImpactView {
    fn default() -> Self {
        Self::Unavailable {
            reason_code: "runtime.traffic_observation_unavailable".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficRoutingDefaultPolicyView {
    pub action: TrafficAction,
    pub target: TrafficRoutingTargetRef,
    pub confirmed_by: TrafficDefaultConfirmedBy,
    pub confirmed_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficRoutingPageModel {
    pub node_id: NodeId,
    /// Applied authority revision, not a local form counter.
    pub authority_revision: u64,
    pub workflow: TrafficRoutingWorkflowState,
    pub default_policy: TrafficRoutingDefaultPolicyView,
    #[serde(default)]
    pub hard_constraints: TrafficRoutingHardConstraints,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hard_constraint_target: Option<TrafficRoutingTargetRef>,
    pub failure_policy: TrafficFailurePolicy,
    pub rules: Vec<TrafficRoutingRuleView>,
    pub targets: Vec<TrafficRoutingTarget>,
    /// Observation availability is part of the authoritative page so the UI
    /// can distinguish an observed zero-hit window from missing data.
    #[serde(default)]
    pub observation: TrafficRoutingObservation,
    /// Whether the current caller may create a draft or apply it. Read-only
    /// callers still receive the authoritative page and target navigation.
    pub can_edit: bool,
}

impl TrafficRoutingPageModel {
    pub fn from_policy(
        policy: &TrafficPolicy,
        workflow: TrafficRoutingWorkflowState,
        hard_constraints: TrafficRoutingHardConstraints,
        targets: Vec<TrafficRoutingTarget>,
        can_edit: bool,
    ) -> DomainResult<Self> {
        Self::from_policy_with_observation(
            policy,
            workflow,
            hard_constraints,
            targets,
            can_edit,
            TrafficRoutingObservation::default(),
        )
    }

    pub fn from_policy_with_observation(
        policy: &TrafficPolicy,
        workflow: TrafficRoutingWorkflowState,
        hard_constraints: TrafficRoutingHardConstraints,
        targets: Vec<TrafficRoutingTarget>,
        can_edit: bool,
        observation: TrafficRoutingObservation,
    ) -> DomainResult<Self> {
        policy.validate()?;
        workflow.validate()?;
        hard_constraints.validate()?;
        observation.validate()?;
        validate_targets(&targets)?;
        let mut normalized_targets = targets;
        normalized_targets.sort_by(|left, right| {
            left.kind
                .cmp(&right.kind)
                .then_with(|| left.label.cmp(&right.label))
                .then_with(|| left.id.cmp(&right.id))
        });

        let rules = policy
            .rules
            .iter()
            .enumerate()
            .map(|(index, rule)| {
                let mut view = rule_view(rule, index, policy.rules.len(), &normalized_targets);
                view.disable_impact = rule_impact_view(
                    policy,
                    rule,
                    TrafficRoutingRuleChange::Disable,
                    &observation,
                )?;
                view.delete_impact =
                    rule_impact_view(policy, rule, TrafficRoutingRuleChange::Delete, &observation)?;
                Ok(view)
            })
            .collect::<DomainResult<Vec<_>>>()?;
        let hard_constraint_target = hard_constraints.target_ref(&normalized_targets);
        Ok(Self {
            node_id: policy.node_id.clone(),
            authority_revision: policy.revision,
            workflow,
            default_policy: TrafficRoutingDefaultPolicyView {
                action: policy.default_policy.action.clone(),
                target: TrafficRoutingTargetRef::resolve(
                    &policy.default_policy.action,
                    &normalized_targets,
                ),
                confirmed_by: policy.default_policy.confirmed_by,
                confirmed_at_unix_ms: policy.default_policy.confirmed_at_unix_ms,
            },
            hard_constraints,
            hard_constraint_target,
            failure_policy: policy.failure_policy.clone(),
            rules,
            targets: normalized_targets,
            observation,
            can_edit,
        })
    }

    /// Search target IDs, labels and adapter-supplied aliases. Results are
    /// deterministic and capped to avoid unbounded UI work.
    pub fn search_targets(&self, query: &str) -> Vec<&TrafficRoutingTarget> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        self.targets
            .iter()
            .filter(|target| target.matches_query(&query))
            .take(50)
            .collect()
    }

    pub fn target(
        &self,
        kind: TrafficRoutingTargetKind,
        id: &str,
    ) -> Option<&TrafficRoutingTarget> {
        self.targets
            .iter()
            .find(|target| target.kind == kind && target.id == id)
    }

    pub fn move_action(
        &self,
        rule_id: &TrafficRuleId,
        direction: TrafficRoutingRuleDirection,
    ) -> Option<&TrafficRoutingMoveAction> {
        self.rules.iter().find_map(|view| {
            (&view.rule_id == rule_id).then_some(match direction {
                TrafficRoutingRuleDirection::Up => view.move_up.as_ref(),
                TrafficRoutingRuleDirection::Down => view.move_down.as_ref(),
            })?
        })
    }
}

fn validate_targets(targets: &[TrafficRoutingTarget]) -> DomainResult<()> {
    let mut seen = std::collections::BTreeSet::new();
    for target in targets {
        validate_identifier("target", &target.id)?;
        if target.label.is_empty()
            || target.label.trim() != target.label
            || target.label.chars().any(char::is_control)
        {
            return Err(DomainError::invalid_value(
                "traffic routing target label is invalid",
            ));
        }
        if !seen.insert((target.kind, target.id.as_str())) {
            return Err(DomainError::invalid_value(
                "traffic routing targets contain a duplicate id",
            ));
        }
        if let Some(navigation) = &target.navigation {
            validate_identifier("navigation page", &navigation.page_id)?;
            validate_identifier("navigation object", &navigation.object_id)?;
        }
    }
    Ok(())
}

fn rule_view(
    rule: &TrafficRule,
    index: usize,
    rule_count: usize,
    targets: &[TrafficRoutingTarget],
) -> TrafficRoutingRuleView {
    let move_action = |direction| {
        let target_index = match direction {
            TrafficRoutingRuleDirection::Up => index.checked_sub(1)?,
            TrafficRoutingRuleDirection::Down => {
                let candidate = index.checked_add(1)?;
                (candidate < rule_count).then_some(candidate)?
            }
        };
        Some(TrafficRoutingMoveAction {
            action_id: direction.action_id().into(),
            rule_id: rule.rule_id.clone(),
            direction,
            from_index: index,
            target_index,
            keyboard_shortcut: direction.keyboard_shortcut().into(),
        })
    };
    TrafficRoutingRuleView {
        rule_id: rule.rule_id.clone(),
        priority: rule.priority,
        enabled: rule.enabled,
        description: rule.description.clone(),
        matcher: rule.matcher.clone(),
        action: rule.action.clone(),
        target: TrafficRoutingTargetRef::resolve(&rule.action, targets),
        risks: rule.matcher.risks(),
        disable_impact: TrafficRoutingRuleImpactView::default(),
        delete_impact: TrafficRoutingRuleImpactView::default(),
        move_up: move_action(TrafficRoutingRuleDirection::Up),
        move_down: move_action(TrafficRoutingRuleDirection::Down),
    }
}

/// Move one rule by stable ID and return the complete normalized order. Draft
/// editing uses this pure function; it neither increments revision nor mutates
/// the applied policy.
pub fn move_rule(
    rules: &[TrafficRule],
    rule_id: &TrafficRuleId,
    direction: TrafficRoutingRuleDirection,
) -> DomainResult<Vec<TrafficRuleId>> {
    let mut ids = rules
        .iter()
        .map(|rule| rule.rule_id.clone())
        .collect::<Vec<_>>();
    let index = ids.iter().position(|id| id == rule_id).ok_or_else(|| {
        DomainError::new(
            "domain.traffic_routing_rule_not_found",
            "traffic routing rule is not in the current draft",
        )
    })?;
    let target = match direction {
        TrafficRoutingRuleDirection::Up => index.checked_sub(1),
        TrafficRoutingRuleDirection::Down => (index + 1 < ids.len()).then_some(index + 1),
    }
    .ok_or_else(|| {
        DomainError::new(
            "domain.traffic_routing_move_boundary",
            "traffic routing rule cannot move beyond the current boundary",
        )
    })?;
    ids.swap(index, target);
    Ok(ids)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrafficRoutingRuleChange {
    Disable,
    Delete,
}

/// One real observed flow that matched the rule. `activity_id` or
/// `diagnostic_id` provides the stable one-click source for the tester.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficRoutingObservedHit {
    pub sample_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic_id: Option<String>,
    pub flow: TrafficFlow,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficRoutingHitSample {
    pub sample_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic_id: Option<String>,
    pub flow: TrafficFlow,
    pub before_action: TrafficAction,
    pub after_action: TrafficAction,
    pub after_reason: TrafficDecisionReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_rule_id: Option<TrafficRuleId>,
}

/// The observable consequence of disabling or deleting a rule. `NoObservedTraffic`
/// means an available observation window confirmed zero matching flows; it is
/// never used when the observation source is unavailable.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TrafficRoutingRuleActivityEffect {
    ObservedTrafficReassigned { sample_count: usize },
    NoObservedTraffic,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficRoutingRuleImpact {
    pub rule_id: TrafficRuleId,
    pub change: TrafficRoutingRuleChange,
    pub previous_action: TrafficAction,
    pub activity_effect: TrafficRoutingRuleActivityEffect,
    pub activity_ids: Vec<String>,
    pub diagnostic_ids: Vec<String>,
    pub hit_samples: Vec<TrafficRoutingHitSample>,
}

/// Compute before/after routing for observed hits of one rule. This is a
/// presentation/confirmation helper; callers still apply through the atomic
/// transaction and never treat this result as proof of runtime completion.
pub fn preview_rule_impact(
    policy: &TrafficPolicy,
    rule_id: &TrafficRuleId,
    change: TrafficRoutingRuleChange,
    observed_hits: &[TrafficRoutingObservedHit],
) -> DomainResult<TrafficRoutingRuleImpact> {
    policy.validate()?;
    let rule = policy
        .rules
        .iter()
        .find(|rule| &rule.rule_id == rule_id)
        .ok_or_else(|| {
            DomainError::new(
                "domain.traffic_routing_rule_not_found",
                "traffic routing rule is not in the current policy",
            )
        })?;
    let mut candidate = policy.clone();
    match change {
        TrafficRoutingRuleChange::Disable => {
            if let Some(candidate_rule) = candidate
                .rules
                .iter_mut()
                .find(|candidate_rule| &candidate_rule.rule_id == rule_id)
            {
                candidate_rule.enabled = false;
            }
        }
        TrafficRoutingRuleChange::Delete => {
            candidate.rules.retain(|rule| &rule.rule_id != rule_id);
            for (index, rule) in candidate.rules.iter_mut().enumerate() {
                rule.priority = u32::try_from(index + 1).map_err(|_| {
                    DomainError::invalid_value("traffic routing has too many rules")
                })?;
            }
        }
    }

    let mut hit_samples = Vec::new();
    let mut activity_ids = std::collections::BTreeSet::new();
    let mut diagnostic_ids = std::collections::BTreeSet::new();
    for hit in observed_hits {
        validate_identifier("hit sample", &hit.sample_id)?;
        if let Some(activity_id) = &hit.activity_id {
            validate_identifier("activity", activity_id)?;
        }
        if let Some(diagnostic_id) = &hit.diagnostic_id {
            validate_identifier("diagnostic", diagnostic_id)?;
        }
        let before = policy.evaluate(&hit.flow)?;
        if before.matched_rule.as_ref() != Some(rule_id) {
            continue;
        }
        let after = candidate.evaluate(&hit.flow)?;
        if let Some(activity_id) = &hit.activity_id {
            activity_ids.insert(activity_id.clone());
        }
        if let Some(diagnostic_id) = &hit.diagnostic_id {
            diagnostic_ids.insert(diagnostic_id.clone());
        }
        hit_samples.push(TrafficRoutingHitSample {
            sample_id: hit.sample_id.clone(),
            activity_id: hit.activity_id.clone(),
            diagnostic_id: hit.diagnostic_id.clone(),
            flow: hit.flow.clone(),
            before_action: before.action,
            after_action: after.action,
            after_reason: after.reason,
            after_rule_id: after.matched_rule,
        });
    }
    let activity_effect = if hit_samples.is_empty() {
        TrafficRoutingRuleActivityEffect::NoObservedTraffic
    } else {
        TrafficRoutingRuleActivityEffect::ObservedTrafficReassigned {
            sample_count: hit_samples.len(),
        }
    };
    Ok(TrafficRoutingRuleImpact {
        rule_id: rule.rule_id.clone(),
        change,
        previous_action: rule.action.clone(),
        activity_effect,
        activity_ids: activity_ids.into_iter().collect(),
        diagnostic_ids: diagnostic_ids.into_iter().collect(),
        hit_samples,
    })
}

fn rule_impact_view(
    policy: &TrafficPolicy,
    rule: &TrafficRule,
    change: TrafficRoutingRuleChange,
    observation: &TrafficRoutingObservation,
) -> DomainResult<TrafficRoutingRuleImpactView> {
    match observation {
        TrafficRoutingObservation::Available { hits, .. } => {
            Ok(TrafficRoutingRuleImpactView::Available {
                impact: preview_rule_impact(policy, &rule.rule_id, change, hits)?,
            })
        }
        TrafficRoutingObservation::Unavailable { reason_code } => {
            Ok(TrafficRoutingRuleImpactView::Unavailable {
                reason_code: reason_code.clone(),
            })
        }
    }
}

fn validate_revision(revision: u64) -> DomainResult<()> {
    if revision == 0 {
        return Err(DomainError::invalid_value(
            "traffic routing revision must be positive",
        ));
    }
    Ok(())
}

fn validate_candidate_revision(base_revision: u64, candidate_revision: u64) -> DomainResult<()> {
    if candidate_revision <= base_revision {
        return Err(DomainError::invalid_value(
            "traffic routing candidate revision must increase",
        ));
    }
    Ok(())
}

fn validate_identifier(kind: &str, value: &str) -> DomainResult<()> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(DomainError::invalid_value(format!(
            "traffic routing {kind} identifier is invalid"
        )));
    }
    Ok(())
}

fn validate_code(value: &str) -> DomainResult<()> {
    if value.is_empty()
        || value.len() > 128
        || value.chars().any(|character| {
            !(character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '_' | '.' | '-'))
        })
    {
        return Err(DomainError::invalid_value(
            "traffic routing status code is invalid",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        ApplicationIdentity, DomainName, PortRange, TrafficDefaultPolicy, TrafficProtocol,
        TrafficRule,
    };

    fn node() -> NodeId {
        NodeId::from_public_key(b"routing-page-node")
    }

    fn network() -> NetworkId {
        NetworkId::from_public_key(b"routing-page-network")
    }

    fn exit() -> NodeId {
        NodeId::from_public_key(b"routing-page-exit")
    }

    fn targets() -> Vec<TrafficRoutingTarget> {
        vec![
            TrafficRoutingTarget::direct(),
            TrafficRoutingTarget::reject(),
            TrafficRoutingTarget::network(network(), "Home"),
            TrafficRoutingTarget::exit(exit(), "Tokyo Exit"),
        ]
    }

    fn rule(index: u8, action: TrafficAction, enabled: bool) -> TrafficRule {
        TrafficRule {
            rule_id: TrafficRuleId::from_public_key(&[index]),
            priority: u32::from(index) + 1,
            enabled,
            matcher: TrafficMatch {
                domain_suffixes: vec![crate::domain::DomainSuffix::parse(if index == 0 {
                    "example.com"
                } else {
                    "example.net"
                })
                .unwrap()],
                destination_ports: vec![PortRange {
                    start: 443,
                    end: 443,
                }],
                protocols: vec![TrafficProtocol::Tcp],
                ..TrafficMatch::default()
            },
            action,
            description: format!("rule {index}"),
        }
    }

    fn policy() -> TrafficPolicy {
        TrafficPolicy {
            node_id: node(),
            revision: 7,
            default_policy: TrafficDefaultPolicy {
                action: TrafficAction::Exit(exit()),
                confirmed_by: TrafficDefaultConfirmedBy::Owner,
                confirmed_at_unix_ms: 1_700_000_000_000,
            },
            failure_policy: TrafficFailurePolicy::fail_closed(),
            rules: vec![
                rule(0, TrafficAction::Direct, true),
                rule(1, TrafficAction::Network(network()), true),
            ],
        }
    }

    fn flow(domain: &str) -> TrafficFlow {
        TrafficFlow {
            domain: Some(DomainName::parse(domain).unwrap()),
            destination_ip: "203.0.113.10".parse().unwrap(),
            destination_port: 443,
            protocol: TrafficProtocol::Tcp,
            application: ApplicationIdentity::Unknown,
        }
    }

    #[test]
    fn page_uses_authority_revision_and_exposes_every_required_surface() {
        let model = TrafficRoutingPageModel::from_policy(
            &policy(),
            TrafficRoutingWorkflowState::Applied { revision: 7 },
            TrafficRoutingHardConstraints {
                fixed_exit: Some(exit()),
                fixed_network: None,
            },
            targets(),
            true,
        )
        .unwrap();
        assert_eq!(model.authority_revision, 7);
        assert_eq!(
            model.workflow.status(),
            TrafficRoutingWorkflowStatus::Applied
        );
        assert_eq!(
            model.default_policy.target.kind,
            TrafficRoutingTargetKind::Exit
        );
        assert!(model.default_policy.target.resolved);
        assert_eq!(
            model.hard_constraint_target.as_ref().unwrap().kind,
            TrafficRoutingTargetKind::Exit
        );
        assert_eq!(model.rules.len(), 2);
        assert!(model.rules[0].matcher.applications.is_empty());
        assert!(model.can_edit);
    }

    #[test]
    fn page_carries_available_impact_and_hit_samples() {
        let policy = policy();
        let model = TrafficRoutingPageModel::from_policy_with_observation(
            &policy,
            TrafficRoutingWorkflowState::Applied { revision: 7 },
            TrafficRoutingHardConstraints::default(),
            targets(),
            true,
            TrafficRoutingObservation::Available {
                window_start_unix_ms: 1_700_000_000_000,
                window_end_unix_ms: 1_700_000_060_000,
                observed_flow_count: 12,
                hits: vec![TrafficRoutingObservedHit {
                    sample_id: "sample-1".into(),
                    activity_id: Some("activity-1".into()),
                    diagnostic_id: Some("diagnostic-1".into()),
                    flow: flow("api.example.com"),
                }],
            },
        )
        .unwrap();

        assert!(matches!(
            model.observation,
            TrafficRoutingObservation::Available {
                observed_flow_count: 12,
                ..
            }
        ));
        let TrafficRoutingRuleImpactView::Available { impact } = &model.rules[0].disable_impact
        else {
            panic!("available observation must expose disable impact");
        };
        assert_eq!(impact.hit_samples.len(), 1);
        assert_eq!(impact.hit_samples[0].sample_id, "sample-1");
        assert_eq!(impact.activity_ids, vec!["activity-1"]);
        assert_eq!(impact.diagnostic_ids, vec!["diagnostic-1"]);
    }

    #[test]
    fn available_zero_hits_and_unavailable_are_distinct() {
        let zero_hits = TrafficRoutingPageModel::from_policy_with_observation(
            &policy(),
            TrafficRoutingWorkflowState::Applied { revision: 7 },
            TrafficRoutingHardConstraints::default(),
            targets(),
            true,
            TrafficRoutingObservation::Available {
                window_start_unix_ms: 1_700_000_000_000,
                window_end_unix_ms: 1_700_000_060_000,
                observed_flow_count: 0,
                hits: Vec::new(),
            },
        )
        .unwrap();
        let TrafficRoutingRuleImpactView::Available { impact } = &zero_hits.rules[0].disable_impact
        else {
            panic!("zero-hit observation must still be available");
        };
        assert_eq!(
            impact.activity_effect,
            TrafficRoutingRuleActivityEffect::NoObservedTraffic
        );

        let unavailable = TrafficRoutingPageModel::from_policy_with_observation(
            &policy(),
            TrafficRoutingWorkflowState::Applied { revision: 7 },
            TrafficRoutingHardConstraints::default(),
            targets(),
            true,
            TrafficRoutingObservation::unavailable("runtime.collector_offline"),
        )
        .unwrap();
        assert!(matches!(
            unavailable.rules[0].delete_impact,
            TrafficRoutingRuleImpactView::Unavailable { .. }
        ));
    }

    #[test]
    fn workflow_distinguishes_draft_applying_applied_failed_and_stale() {
        for (state, expected) in [
            (
                TrafficRoutingWorkflowState::Draft { base_revision: 7 },
                TrafficRoutingWorkflowStatus::Draft,
            ),
            (
                TrafficRoutingWorkflowState::Applying {
                    base_revision: 7,
                    candidate_revision: 8,
                    operation_id: "op_routing".into(),
                },
                TrafficRoutingWorkflowStatus::Applying,
            ),
            (
                TrafficRoutingWorkflowState::Applied { revision: 8 },
                TrafficRoutingWorkflowStatus::Applied,
            ),
            (
                TrafficRoutingWorkflowState::Failed {
                    base_revision: 7,
                    candidate_revision: 8,
                    stage: "verify".into(),
                    reason_code: "runtime.dns_unreachable".into(),
                },
                TrafficRoutingWorkflowStatus::Failed,
            ),
            (
                TrafficRoutingWorkflowState::Stale {
                    revision: 7,
                    reason_code: "runtime.stopped".into(),
                },
                TrafficRoutingWorkflowStatus::Stale,
            ),
        ] {
            state.validate().unwrap();
            assert_eq!(state.status(), expected);
        }
    }

    #[test]
    fn move_actions_and_equivalent_button_ids_follow_first_match_order() {
        let model = TrafficRoutingPageModel::from_policy(
            &policy(),
            TrafficRoutingWorkflowState::Draft { base_revision: 7 },
            TrafficRoutingHardConstraints::default(),
            targets(),
            true,
        )
        .unwrap();
        assert!(model.rules[0].move_up.is_none());
        let down = model.rules[0].move_down.as_ref().unwrap();
        assert_eq!(down.action_id, "move_traffic_rule_down");
        assert_eq!(down.keyboard_shortcut, "Alt+ArrowDown");
        assert_eq!(down.target_index, 1);
        assert!(model.rules[1].move_down.is_none());

        let ordered = move_rule(
            &policy().rules,
            &policy().rules[0].rule_id,
            TrafficRoutingRuleDirection::Down,
        )
        .unwrap();
        assert_eq!(ordered[0], policy().rules[1].rule_id);
        assert_eq!(ordered[1], policy().rules[0].rule_id);
        assert_eq!(
            move_rule(
                &policy().rules,
                &policy().rules[0].rule_id,
                TrafficRoutingRuleDirection::Up,
            )
            .unwrap_err()
            .code(),
            "domain.traffic_routing_move_boundary"
        );
    }

    #[test]
    fn targets_are_searchable_and_navigation_is_stable() {
        let model = TrafficRoutingPageModel::from_policy(
            &policy(),
            TrafficRoutingWorkflowState::Applied { revision: 7 },
            TrafficRoutingHardConstraints::default(),
            targets(),
            false,
        )
        .unwrap();
        let results = model.search_targets("tokyo");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, TrafficRoutingTargetKind::Exit);
        let navigation = results[0].navigation.as_ref().unwrap();
        assert_eq!(navigation.page_id, "topology");
        assert_eq!(navigation.object_id, exit().as_str());
        assert_eq!(
            model.search_targets("home")[0].kind,
            TrafficRoutingTargetKind::Network
        );
        assert!(model.search_targets(" ").is_empty());
        assert!(model.search_targets("missing").is_empty());
    }

    #[test]
    fn delete_and_disable_show_observed_impact_and_fallthrough_decision() {
        let policy = policy();
        let observed = vec![TrafficRoutingObservedHit {
            sample_id: "sample-1".into(),
            activity_id: Some("activity-1".into()),
            diagnostic_id: Some("diagnostic-1".into()),
            flow: flow("api.example.com"),
        }];
        for change in [
            TrafficRoutingRuleChange::Disable,
            TrafficRoutingRuleChange::Delete,
        ] {
            let impact =
                preview_rule_impact(&policy, &policy.rules[0].rule_id, change, &observed).unwrap();
            assert_eq!(impact.change, change);
            assert_eq!(impact.previous_action, TrafficAction::Direct);
            assert_eq!(impact.activity_ids, vec!["activity-1"]);
            assert_eq!(impact.diagnostic_ids, vec!["diagnostic-1"]);
            assert_eq!(
                impact.activity_effect,
                TrafficRoutingRuleActivityEffect::ObservedTrafficReassigned { sample_count: 1 }
            );
            assert_eq!(impact.hit_samples.len(), 1);
            assert_eq!(
                impact.hit_samples[0].after_action,
                TrafficAction::Exit(exit())
            );
            assert_eq!(
                impact.hit_samples[0].after_reason,
                TrafficDecisionReason::Default
            );
        }
    }

    #[test]
    fn no_observed_hits_never_manufacture_activity() {
        let impact = preview_rule_impact(
            &policy(),
            &policy().rules[0].rule_id,
            TrafficRoutingRuleChange::Disable,
            &[],
        )
        .unwrap();
        assert_eq!(
            impact.activity_effect,
            TrafficRoutingRuleActivityEffect::NoObservedTraffic
        );
        assert!(impact.hit_samples.is_empty());
        assert!(impact.activity_ids.is_empty());
    }

    #[test]
    fn missing_target_stays_explicitly_unresolved() {
        let model = TrafficRoutingPageModel::from_policy(
            &policy(),
            TrafficRoutingWorkflowState::Stale {
                revision: 7,
                reason_code: "runtime.adapter_unavailable".into(),
            },
            TrafficRoutingHardConstraints {
                fixed_exit: Some(NodeId::from_public_key(b"missing-exit")),
                fixed_network: None,
            },
            vec![
                TrafficRoutingTarget::direct(),
                TrafficRoutingTarget::reject(),
            ],
            true,
        )
        .unwrap();
        assert!(!model.default_policy.target.resolved);
        assert!(!model.hard_constraint_target.as_ref().unwrap().resolved);
        assert!(model.rules[0].target.resolved);
        assert!(!model.rules[1].target.resolved);
    }
}
