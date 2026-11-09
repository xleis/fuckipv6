//! Administrator-owned Exit selection policy.
//!
//! This contract is deliberately separate from Exit contribution policy. A
//! member may be required or allowed to contribute an Exit without being
//! allowed to route through every Exit. Selection is deny-first and every
//! candidate carries the source and reason for its final availability so both
//! UIs and runtime recovery can explain the same decision.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::exit_contribution::{ExitAvailabilityState, ExitContributionAvailability};
use super::ids::{GroupId, NodeId};
use super::traffic_failure::TrafficFailureRouteContext;
use super::traffic_policy::TrafficAction;
use super::traffic_routing_page::TrafficRoutingHardConstraints;
use super::{DomainError, DomainResult};

pub const EXIT_SELECTION_MAX_EXITS: usize = 256;
pub const EXIT_SELECTION_MAX_GROUPS: usize = 128;
pub const EXIT_SELECTION_MAX_MEMBERS_PER_GROUP: usize = 4096;
pub const EXIT_SELECTION_MAX_MEMBER_RULES: usize = 4096;

/// Whether administrators pin one Exit or let members select from the final
/// administrator-authorized set.
#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ExitSelectionMode {
    #[default]
    MemberChoice,
    Fixed {
        exit: NodeId,
    },
}

/// What to do with an already established session when its Exit is no longer
/// selectable. `Migrate` may still fail closed if no replacement is available.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitSessionTransitionPolicy {
    Migrate,
    Terminate,
    #[default]
    FailClosed,
}

/// An administrator-defined group scope. Membership is part of this policy so
/// an Exit decision never depends on a client-supplied or stale role label.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ExitSelectionGroupRule {
    pub group_id: GroupId,
    #[serde(default)]
    pub members: Vec<NodeId>,
    #[serde(default)]
    pub allow: Vec<NodeId>,
    #[serde(default)]
    pub deny: Vec<NodeId>,
}

/// A member-specific narrowing or deny. A deny always wins over every allow.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ExitSelectionMemberRule {
    pub node_id: NodeId,
    #[serde(default)]
    pub allow: Vec<NodeId>,
    #[serde(default)]
    pub deny: Vec<NodeId>,
}

/// Versioned administrator policy. `available_exits` is the network-level
/// allow set and therefore the universe of all other scopes. Group `allow`
/// lists narrow that universe when the subject belongs to a matching group;
/// member `allow` lists narrow it further. Any matching deny removes an Exit
/// regardless of allow rules at any scope.
#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExitSelectionPolicy {
    pub mode: ExitSelectionMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_exit: Option<NodeId>,
    pub available_exits: Vec<NodeId>,
    pub denied_exits: Vec<NodeId>,
    pub groups: Vec<ExitSelectionGroupRule>,
    pub members: Vec<ExitSelectionMemberRule>,
    pub session_transition: ExitSessionTransitionPolicy,
}

/// Where the final decision came from.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum ExitSelectionSource {
    Network,
    Group { group_id: GroupId },
    Member,
    SelectionMode,
    Availability,
}

/// Stable reason code and scope for one Exit. Unknown and stale availability is
/// never reported as selectable.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ExitSelectionCandidate {
    pub exit: NodeId,
    pub selectable: bool,
    pub reason_code: String,
    pub source: ExitSelectionSource,
    pub availability: ExitContributionAvailability,
    pub is_default: bool,
}

/// Complete administrator-authorized set for one subject. Candidate ordering is
/// stable to make UI and audit output reproducible.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ExitSelectionResolution {
    pub policy_revision: u64,
    pub subject: NodeId,
    pub mode: ExitSelectionMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_exit: Option<NodeId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixed_exit: Option<NodeId>,
    pub candidates: Vec<ExitSelectionCandidate>,
    pub selectable_exits: Vec<NodeId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_default: Option<NodeId>,
}

impl ExitSelectionResolution {
    pub fn candidate(&self, exit: &NodeId) -> Option<&ExitSelectionCandidate> {
        self.candidates
            .iter()
            .find(|candidate| &candidate.exit == exit)
    }

    /// Enforce the administrator policy for a new or reopened session. A fixed
    /// Exit can never be replaced by a local selection or fallback.
    pub fn select_for_session(&self, requested: Option<&NodeId>) -> DomainResult<NodeId> {
        match &self.mode {
            ExitSelectionMode::Fixed { exit } => {
                if requested.is_some_and(|requested| requested != exit) {
                    return Err(DomainError::new(
                        "domain.exit_selection_fixed_exit",
                        "the administrator fixes this network to a different Exit",
                    ));
                }
                let candidate = self.candidate(exit).ok_or_else(|| {
                    DomainError::new(
                        "domain.exit_selection_exit_not_selectable",
                        "the fixed Exit is outside the administrator-authorized set",
                    )
                })?;
                if !candidate.selectable {
                    return Err(DomainError::new(
                        "domain.exit_selection_exit_unavailable",
                        format!(
                            "the fixed Exit is not selectable: {}",
                            candidate.reason_code
                        ),
                    ));
                }
                Ok(exit.clone())
            }
            ExitSelectionMode::MemberChoice => {
                if let Some(requested) = requested {
                    let candidate = self.candidate(requested).ok_or_else(|| {
                        DomainError::new(
                            "domain.exit_selection_exit_not_allowed",
                            "the requested Exit is not in the network-authorized set",
                        )
                    })?;
                    if !candidate.selectable {
                        return Err(DomainError::new(
                            "domain.exit_selection_exit_not_allowed",
                            format!(
                                "the requested Exit is not selectable: {}",
                                candidate.reason_code
                            ),
                        ));
                    }
                    return Ok(requested.clone());
                }
                self.selected_default
                    .clone()
                    .or_else(|| self.selectable_exits.first().cloned())
                    .ok_or_else(|| {
                        DomainError::new(
                            "domain.exit_selection_no_candidate",
                            "no administrator-authorized Exit is currently selectable",
                        )
                    })
            }
        }
    }

    /// Project the same decision into the hard constraints consumed by routing
    /// and failure handling. Fixed mode is represented in both the requested
    /// action and the non-bypass constraint.
    pub fn failure_route_context(
        &self,
        requested: Option<&NodeId>,
    ) -> DomainResult<TrafficFailureRouteContext> {
        let selected = self.select_for_session(requested)?;
        Ok(TrafficFailureRouteContext {
            requested_action: TrafficAction::Exit(selected),
            fixed_exit: self.fixed_exit.clone(),
            fixed_network: None,
        })
    }

    pub fn routing_hard_constraints(&self) -> TrafficRoutingHardConstraints {
        TrafficRoutingHardConstraints {
            fixed_exit: self.fixed_exit.clone(),
            fixed_network: None,
        }
    }

    /// Explain the transition of a session that was already established. The
    /// caller must stop or migrate it; no result silently keeps a revoked Exit.
    pub fn plan_session_transition(
        &self,
        current_exit: Option<&NodeId>,
        policy: &ExitSelectionPolicy,
    ) -> ExitSessionTransition {
        let Some(current_exit) = current_exit else {
            return ExitSessionTransition::fail_closed(
                None,
                ExitTransitionReason::NoCurrentExit,
                self.policy_revision,
            );
        };
        if self
            .candidate(current_exit)
            .is_some_and(|candidate| candidate.selectable)
        {
            return ExitSessionTransition {
                action: ExitSessionAction::Keep,
                from_exit: Some(current_exit.clone()),
                to_exit: Some(current_exit.clone()),
                reason: ExitTransitionReason::StillAuthorized,
                policy_revision: self.policy_revision,
            };
        }

        let reason = match self.candidate(current_exit) {
            None => ExitTransitionReason::ExitNotAuthorized,
            Some(candidate) if candidate.reason_code == "exit.selection.offline" => {
                ExitTransitionReason::ExitOffline
            }
            Some(candidate) if candidate.reason_code == "exit.selection.availability_unknown" => {
                ExitTransitionReason::AvailabilityUnknown
            }
            Some(candidate) if candidate.reason_code == "exit.selection.degraded" => {
                ExitTransitionReason::ExitDegraded
            }
            Some(_) => ExitTransitionReason::ExitRevoked,
        };

        match policy.session_transition {
            ExitSessionTransitionPolicy::Migrate => match self.select_for_session(None) {
                Ok(target) => ExitSessionTransition {
                    action: ExitSessionAction::Migrate { to: target.clone() },
                    from_exit: Some(current_exit.clone()),
                    to_exit: Some(target),
                    reason,
                    policy_revision: self.policy_revision,
                },
                Err(_) => ExitSessionTransition::fail_closed(
                    Some(current_exit.clone()),
                    ExitTransitionReason::NoReplacementExit,
                    self.policy_revision,
                ),
            },
            ExitSessionTransitionPolicy::Terminate => ExitSessionTransition {
                action: ExitSessionAction::Terminate,
                from_exit: Some(current_exit.clone()),
                to_exit: None,
                reason,
                policy_revision: self.policy_revision,
            },
            ExitSessionTransitionPolicy::FailClosed => ExitSessionTransition::fail_closed(
                Some(current_exit.clone()),
                reason,
                self.policy_revision,
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ExitSessionAction {
    Keep,
    Migrate { to: NodeId },
    Terminate,
    FailClosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitTransitionReason {
    StillAuthorized,
    NoCurrentExit,
    ExitRevoked,
    ExitOffline,
    ExitDegraded,
    AvailabilityUnknown,
    ExitNotAuthorized,
    NoReplacementExit,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ExitSessionTransition {
    pub action: ExitSessionAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_exit: Option<NodeId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_exit: Option<NodeId>,
    pub reason: ExitTransitionReason,
    pub policy_revision: u64,
}

impl ExitSessionTransition {
    fn fail_closed(
        from_exit: Option<NodeId>,
        reason: ExitTransitionReason,
        policy_revision: u64,
    ) -> Self {
        Self {
            action: ExitSessionAction::FailClosed,
            from_exit,
            to_exit: None,
            reason,
            policy_revision,
        }
    }
}

impl ExitSelectionPolicy {
    pub fn fixed_exit(&self) -> Option<&NodeId> {
        match &self.mode {
            ExitSelectionMode::MemberChoice => None,
            ExitSelectionMode::Fixed { exit } => Some(exit),
        }
    }

    pub fn validate(&self) -> DomainResult<()> {
        validate_unique_exits("available_exits", &self.available_exits)?;
        validate_unique_exits("denied_exits", &self.denied_exits)?;
        if self.available_exits.len() > EXIT_SELECTION_MAX_EXITS {
            return Err(DomainError::invalid_value(
                "Exit selection policy contains too many available Exits",
            ));
        }
        let available: BTreeSet<&NodeId> = self.available_exits.iter().collect();
        if let Some(exit) = self.fixed_exit() {
            if !available.contains(exit) {
                return Err(DomainError::invalid_value(
                    "the fixed Exit must be in the network-level available set",
                ));
            }
            if self.denied_exits.contains(exit) {
                return Err(DomainError::invariant(
                    "the fixed Exit cannot also be network-denied",
                ));
            }
        }
        if let Some(default) = &self.default_exit {
            if !available.contains(default) {
                return Err(DomainError::invalid_value(
                    "the default Exit must be in the network-level available set",
                ));
            }
            if self.denied_exits.contains(default) {
                return Err(DomainError::invariant(
                    "the default Exit cannot also be network-denied",
                ));
            }
        }
        if self.groups.len() > EXIT_SELECTION_MAX_GROUPS {
            return Err(DomainError::invalid_value(
                "Exit selection policy contains too many group rules",
            ));
        }
        if self.members.len() > EXIT_SELECTION_MAX_MEMBER_RULES {
            return Err(DomainError::invalid_value(
                "Exit selection policy contains too many member rules",
            ));
        }

        let mut group_ids = BTreeSet::new();
        for group in &self.groups {
            if !group_ids.insert(&group.group_id) {
                return Err(DomainError::invariant(
                    "Exit selection policy contains duplicate group rules",
                ));
            }
            if group.members.len() > EXIT_SELECTION_MAX_MEMBERS_PER_GROUP {
                return Err(DomainError::invalid_value(
                    "Exit selection group contains too many members",
                ));
            }
            validate_unique_nodes("group members", &group.members)?;
            validate_rule_exits("group allow", &group.allow, &available)?;
            validate_rule_exits("group deny", &group.deny, &available)?;
        }

        let mut member_ids = BTreeSet::new();
        for member in &self.members {
            if !member_ids.insert(&member.node_id) {
                return Err(DomainError::invariant(
                    "Exit selection policy contains duplicate member rules",
                ));
            }
            validate_rule_exits("member allow", &member.allow, &available)?;
            validate_rule_exits("member deny", &member.deny, &available)?;
        }
        Ok(())
    }

    /// Resolve one member's final selectable set. Missing availability remains
    /// unknown and therefore not selectable.
    pub fn resolve(
        &self,
        policy_revision: u64,
        subject: &NodeId,
        availability: &BTreeMap<NodeId, ExitContributionAvailability>,
    ) -> DomainResult<ExitSelectionResolution> {
        self.validate()?;
        if policy_revision == 0 {
            return Err(DomainError::invalid_value(
                "Exit selection policy revision must be positive",
            ));
        }
        let matching_groups: Vec<&ExitSelectionGroupRule> = self
            .groups
            .iter()
            .filter(|group| group.members.contains(subject))
            .collect();
        let group_allow: BTreeSet<&NodeId> = matching_groups
            .iter()
            .flat_map(|group| group.allow.iter())
            .collect();
        let has_group_allow = matching_groups.iter().any(|group| !group.allow.is_empty());
        let first_group_allow_source = matching_groups
            .iter()
            .filter(|group| !group.allow.is_empty())
            .min_by_key(|group| &group.group_id);
        let member_rule = self.members.iter().find(|rule| &rule.node_id == subject);
        let has_member_allow = member_rule.is_some_and(|rule| !rule.allow.is_empty());

        let exits: BTreeSet<&NodeId> = self.available_exits.iter().collect();
        let mut candidates = Vec::with_capacity(exits.len());
        for exit in exits {
            let observed = availability
                .get(exit)
                .cloned()
                .unwrap_or_else(|| ExitContributionAvailability::declared_unknown(None));
            let (selectable, reason_code, source) = if self.denied_exits.contains(exit) {
                (
                    false,
                    "exit.selection.network_denied",
                    ExitSelectionSource::Network,
                )
            } else if let Some(group) = matching_groups
                .iter()
                .filter(|group| group.deny.contains(exit))
                .min_by_key(|group| &group.group_id)
            {
                (
                    false,
                    "exit.selection.group_denied",
                    ExitSelectionSource::Group {
                        group_id: group.group_id.clone(),
                    },
                )
            } else if member_rule.is_some_and(|rule| rule.deny.contains(exit)) {
                (
                    false,
                    "exit.selection.member_denied",
                    ExitSelectionSource::Member,
                )
            } else if self.fixed_exit().is_some_and(|fixed| fixed != exit) {
                (
                    false,
                    "exit.selection.fixed_exit_required",
                    ExitSelectionSource::SelectionMode,
                )
            } else if has_group_allow && !group_allow.contains(exit) {
                (
                    false,
                    "exit.selection.group_not_allowed",
                    first_group_allow_source
                        .map(|group| ExitSelectionSource::Group {
                            group_id: group.group_id.clone(),
                        })
                        .unwrap_or(ExitSelectionSource::Network),
                )
            } else if has_member_allow && !member_rule.is_some_and(|rule| rule.allow.contains(exit))
            {
                (
                    false,
                    "exit.selection.member_not_allowed",
                    ExitSelectionSource::Member,
                )
            } else {
                match observed.state {
                    ExitAvailabilityState::Available => {
                        let source = if has_member_allow {
                            ExitSelectionSource::Member
                        } else if has_group_allow {
                            first_group_allow_source
                                .map(|group| ExitSelectionSource::Group {
                                    group_id: group.group_id.clone(),
                                })
                                .unwrap_or(ExitSelectionSource::Network)
                        } else {
                            ExitSelectionSource::Network
                        };
                        (true, "exit.selection.available", source)
                    }
                    ExitAvailabilityState::Unknown => (
                        false,
                        "exit.selection.availability_unknown",
                        ExitSelectionSource::Availability,
                    ),
                    ExitAvailabilityState::Degraded => (
                        false,
                        "exit.selection.degraded",
                        ExitSelectionSource::Availability,
                    ),
                    ExitAvailabilityState::Unavailable => (
                        false,
                        "exit.selection.offline",
                        ExitSelectionSource::Availability,
                    ),
                }
            };
            candidates.push(ExitSelectionCandidate {
                exit: exit.clone(),
                selectable,
                reason_code: reason_code.into(),
                source,
                availability: observed,
                is_default: self.default_exit.as_ref() == Some(exit),
            });
        }
        let selectable_exits = candidates
            .iter()
            .filter(|candidate| candidate.selectable)
            .map(|candidate| candidate.exit.clone())
            .collect::<Vec<_>>();
        let selected_default = self.default_exit.as_ref().and_then(|default| {
            selectable_exits
                .iter()
                .find(|exit| *exit == default)
                .cloned()
        });
        Ok(ExitSelectionResolution {
            policy_revision,
            subject: subject.clone(),
            mode: self.mode.clone(),
            default_exit: self.default_exit.clone(),
            fixed_exit: self.fixed_exit().cloned(),
            candidates,
            selectable_exits,
            selected_default,
        })
    }
}

fn validate_unique_exits(field: &str, exits: &[NodeId]) -> DomainResult<()> {
    validate_unique_nodes(field, exits)
}

fn validate_unique_nodes(field: &str, nodes: &[NodeId]) -> DomainResult<()> {
    let mut seen = BTreeSet::new();
    if nodes.iter().any(|node| !seen.insert(node)) {
        return Err(DomainError::invariant(format!(
            "Exit selection policy contains duplicate {field}"
        )));
    }
    Ok(())
}

fn validate_rule_exits(
    field: &str,
    exits: &[NodeId],
    available: &BTreeSet<&NodeId>,
) -> DomainResult<()> {
    validate_unique_exits(field, exits)?;
    if exits.iter().any(|exit| !available.contains(exit)) {
        return Err(DomainError::invalid_value(format!(
            "{field} references an Exit outside the network-level available set"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        ExitAvailabilitySource, TrafficFailureDecisionReason, TrafficFailureKind,
        TrafficFailurePolicy,
    };

    fn node(name: &str) -> NodeId {
        NodeId::from_public_key(name.as_bytes())
    }

    fn group(name: &str) -> GroupId {
        GroupId::from_public_key(name.as_bytes())
    }

    fn available() -> ExitContributionAvailability {
        ExitContributionAvailability::observed(
            true,
            true,
            ExitAvailabilitySource::RemotePeerReport,
            100,
        )
    }

    fn unavailable() -> ExitContributionAvailability {
        ExitContributionAvailability::observed(
            false,
            false,
            ExitAvailabilitySource::RemotePeerReport,
            100,
        )
    }

    fn evidence(
        entries: impl IntoIterator<Item = (NodeId, ExitContributionAvailability)>,
    ) -> BTreeMap<NodeId, ExitContributionAvailability> {
        entries.into_iter().collect()
    }

    #[test]
    fn deny_wins_across_network_group_and_member_scopes() {
        let exit_a = node("exit-a");
        let exit_b = node("exit-b");
        let exit_c = node("exit-c");
        let subject = node("member-a");
        let policy = ExitSelectionPolicy {
            mode: ExitSelectionMode::MemberChoice,
            default_exit: None,
            available_exits: vec![exit_a.clone(), exit_b.clone(), exit_c.clone()],
            denied_exits: vec![exit_a.clone()],
            groups: vec![ExitSelectionGroupRule {
                group_id: group("staff"),
                members: vec![subject.clone()],
                allow: vec![exit_a.clone(), exit_b.clone()],
                deny: vec![exit_b.clone()],
            }],
            members: vec![ExitSelectionMemberRule {
                node_id: subject.clone(),
                allow: vec![exit_b.clone(), exit_c.clone()],
                deny: vec![exit_c.clone()],
            }],
            session_transition: ExitSessionTransitionPolicy::FailClosed,
        };
        let resolution = policy
            .resolve(
                7,
                &subject,
                &evidence([
                    (exit_a.clone(), available()),
                    (exit_b.clone(), available()),
                    (exit_c.clone(), available()),
                ]),
            )
            .unwrap();
        assert!(resolution.selectable_exits.is_empty());
        assert_eq!(
            resolution.candidate(&exit_a).unwrap().reason_code,
            "exit.selection.network_denied"
        );
        assert_eq!(
            resolution.candidate(&exit_b).unwrap().reason_code,
            "exit.selection.group_denied"
        );
        assert_eq!(
            resolution.candidate(&exit_c).unwrap().reason_code,
            "exit.selection.member_denied"
        );
    }

    #[test]
    fn group_and_member_allow_lists_narrow_the_network_set() {
        let exit_a = node("exit-a");
        let exit_b = node("exit-b");
        let exit_c = node("exit-c");
        let subject = node("member-a");
        let policy = ExitSelectionPolicy {
            default_exit: Some(exit_b.clone()),
            available_exits: vec![exit_a.clone(), exit_b.clone(), exit_c.clone()],
            groups: vec![ExitSelectionGroupRule {
                group_id: group("staff"),
                members: vec![subject.clone()],
                allow: vec![exit_a.clone(), exit_b.clone()],
                deny: vec![],
            }],
            members: vec![ExitSelectionMemberRule {
                node_id: subject.clone(),
                allow: vec![exit_b.clone()],
                deny: vec![],
            }],
            ..ExitSelectionPolicy::default()
        };
        let resolution = policy
            .resolve(
                2,
                &subject,
                &evidence([
                    (exit_a, available()),
                    (exit_b.clone(), available()),
                    (exit_c, available()),
                ]),
            )
            .unwrap();
        assert_eq!(resolution.selectable_exits, vec![exit_b.clone()]);
        assert_eq!(resolution.selected_default, Some(exit_b.clone()));
        assert_eq!(
            resolution.candidate(&exit_b).unwrap().source,
            ExitSelectionSource::Member
        );
    }

    #[test]
    fn fixed_exit_rejects_local_choice_and_projects_hard_constraints() {
        let fixed = node("fixed-exit");
        let other = node("other-exit");
        let subject = node("member-a");
        let policy = ExitSelectionPolicy {
            mode: ExitSelectionMode::Fixed {
                exit: fixed.clone(),
            },
            available_exits: vec![fixed.clone(), other.clone()],
            ..ExitSelectionPolicy::default()
        };
        let resolution = policy
            .resolve(
                3,
                &subject,
                &evidence([(fixed.clone(), available()), (other.clone(), available())]),
            )
            .unwrap();
        assert_eq!(
            resolution
                .select_for_session(Some(&other))
                .unwrap_err()
                .code(),
            "domain.exit_selection_fixed_exit"
        );
        assert_eq!(resolution.select_for_session(None).unwrap(), fixed);
        assert_eq!(
            resolution.routing_hard_constraints().fixed_exit,
            Some(fixed.clone())
        );
        let route = resolution.failure_route_context(None).unwrap();
        assert_eq!(route.fixed_exit, Some(fixed.clone()));
        assert!(matches!(route.requested_action, TrafficAction::Exit(ref exit) if exit == &fixed));
        let decision = TrafficFailurePolicy::fail_closed()
            .decide(&route, TrafficFailureKind::ExitUnavailable)
            .unwrap();
        assert_eq!(decision.reason, TrafficFailureDecisionReason::FixedExit);
    }

    #[test]
    fn offline_unknown_and_stale_evidence_are_not_selectable() {
        let offline = node("offline");
        let unknown = node("unknown");
        let stale = node("stale");
        let subject = node("member-a");
        let policy = ExitSelectionPolicy {
            available_exits: vec![offline.clone(), unknown.clone(), stale.clone()],
            ..ExitSelectionPolicy::default()
        };
        let stale_evidence = ExitContributionAvailability::stale(true, 10);
        let resolution = policy
            .resolve(
                1,
                &subject,
                &evidence([
                    (offline.clone(), unavailable()),
                    (unknown.clone(), ExitContributionAvailability::default()),
                    (stale.clone(), stale_evidence),
                ]),
            )
            .unwrap();
        assert!(resolution.selectable_exits.is_empty());
        assert_eq!(
            resolution.candidate(&offline).unwrap().reason_code,
            "exit.selection.offline"
        );
        assert_eq!(
            resolution.candidate(&unknown).unwrap().reason_code,
            "exit.selection.availability_unknown"
        );
        assert_eq!(
            resolution.candidate(&stale).unwrap().reason_code,
            "exit.selection.availability_unknown"
        );
    }

    #[test]
    fn revoked_exit_disappears_from_new_session_candidates_immediately() {
        let exit_a = node("exit-a");
        let exit_b = node("exit-b");
        let subject = node("member-a");
        let before = ExitSelectionPolicy {
            default_exit: Some(exit_a.clone()),
            available_exits: vec![exit_a.clone(), exit_b.clone()],
            ..ExitSelectionPolicy::default()
        };
        let after = ExitSelectionPolicy {
            denied_exits: vec![exit_a.clone()],
            available_exits: vec![exit_a.clone(), exit_b.clone()],
            ..ExitSelectionPolicy::default()
        };
        let availability = evidence([(exit_a.clone(), available()), (exit_b.clone(), available())]);
        let before = before.resolve(1, &subject, &availability).unwrap();
        let after = after.resolve(2, &subject, &availability).unwrap();
        assert!(before.selectable_exits.contains(&exit_a));
        assert!(!after.selectable_exits.contains(&exit_a));
        assert_eq!(
            after.select_for_session(Some(&exit_a)).unwrap_err().code(),
            "domain.exit_selection_exit_not_allowed"
        );
        assert_eq!(after.select_for_session(None).unwrap(), exit_b);
    }

    #[test]
    fn established_session_plans_migrate_terminate_or_fail_closed() {
        let revoked = node("revoked");
        let replacement = node("replacement");
        let subject = node("member-a");
        let availability = evidence([
            (revoked.clone(), available()),
            (replacement.clone(), available()),
        ]);
        for (transition, expected) in [
            (
                ExitSessionTransitionPolicy::Migrate,
                ExitSessionAction::Migrate {
                    to: replacement.clone(),
                },
            ),
            (
                ExitSessionTransitionPolicy::Terminate,
                ExitSessionAction::Terminate,
            ),
            (
                ExitSessionTransitionPolicy::FailClosed,
                ExitSessionAction::FailClosed,
            ),
        ] {
            let policy = ExitSelectionPolicy {
                denied_exits: vec![revoked.clone()],
                available_exits: vec![revoked.clone(), replacement.clone()],
                session_transition: transition,
                ..ExitSelectionPolicy::default()
            };
            let resolution = policy.resolve(9, &subject, &availability).unwrap();
            let plan = resolution.plan_session_transition(Some(&revoked), &policy);
            assert_eq!(plan.action, expected);
            assert_eq!(plan.reason, ExitTransitionReason::ExitRevoked);
        }
    }

    #[test]
    fn fixed_exit_changed_on_policy_takes_the_same_transition_path() {
        let old = node("old-fixed");
        let new = node("new-fixed");
        let subject = node("member-a");
        let availability = evidence([(old.clone(), available()), (new.clone(), available())]);
        let policy = ExitSelectionPolicy {
            mode: ExitSelectionMode::Fixed { exit: new.clone() },
            available_exits: vec![old.clone(), new.clone()],
            session_transition: ExitSessionTransitionPolicy::Migrate,
            ..ExitSelectionPolicy::default()
        };
        let resolution = policy.resolve(4, &subject, &availability).unwrap();
        let plan = resolution.plan_session_transition(Some(&old), &policy);
        assert_eq!(plan.action, ExitSessionAction::Migrate { to: new });
        assert_eq!(plan.reason, ExitTransitionReason::ExitRevoked);
    }

    #[test]
    fn invalid_policy_rejects_defaults_and_fixed_exits_outside_the_set() {
        let exit = node("exit");
        let outside = node("outside");
        assert!(ExitSelectionPolicy {
            default_exit: Some(outside.clone()),
            available_exits: vec![exit.clone()],
            ..ExitSelectionPolicy::default()
        }
        .validate()
        .is_err());
        assert!(ExitSelectionPolicy {
            mode: ExitSelectionMode::Fixed { exit: outside },
            available_exits: vec![exit],
            ..ExitSelectionPolicy::default()
        }
        .validate()
        .is_err());
    }
}
