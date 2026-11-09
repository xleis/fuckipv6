//! Runtime enforcement of node-owner Exit contribution restrictions.
//!
//! The shared domain evaluator owns the actual rule semantics. This module adds
//! the trusted runtime inputs that a packet datapath needs: the local owner
//! identity, the authoritative member roster, and Exit-only usage accounting.

use fuckipv6_common::domain::{
    evaluate_exit_contribution_access, ExitContributionAccessDecision,
    ExitContributionAccessReason, ExitContributionQuotaPeriod, ExitContributionRestrictions,
    ExitContributionUsage, MemberLifecycleStatus, MembershipRole, NetworkId,
    NetworkMemberDirectory, NodeId, NodeLocalAccessPolicy,
};

/// A roster older than this is not safe evidence for an owner restriction.
pub(crate) const EXIT_MEMBER_DIRECTORY_MAX_AGE_MS: u64 = 90_000;
const EXIT_BANDWIDTH_WINDOW_MS: u64 = 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExitContributionRuntimeDenial {
    MissingNetworkContext,
    PolicyOwnerMismatch,
    InvalidPolicy,
    MemberDirectoryUnavailable,
    RequesterNotInRoster,
    RequesterNotActive,
    AccessDenied(ExitContributionAccessReason),
}

impl ExitContributionRuntimeDenial {
    pub(crate) const fn reason_code(&self) -> &'static str {
        match self {
            Self::MissingNetworkContext => "exit.contribution.missing_network_context",
            Self::PolicyOwnerMismatch => "exit.contribution.policy_owner_mismatch",
            Self::InvalidPolicy => "exit.contribution.invalid_policy",
            Self::MemberDirectoryUnavailable => "exit.contribution.member_directory_unavailable",
            Self::RequesterNotInRoster => "exit.contribution.requester_not_in_roster",
            Self::RequesterNotActive => "exit.contribution.requester_not_active",
            Self::AccessDenied(ExitContributionAccessReason::NetworkNotAllowed) => {
                "exit.contribution.network_not_allowed"
            }
            Self::AccessDenied(ExitContributionAccessReason::LocalVeto) => {
                "exit.contribution.local_veto"
            }
            Self::AccessDenied(ExitContributionAccessReason::MemberNotAllowed) => {
                "exit.contribution.member_not_allowed"
            }
            Self::AccessDenied(ExitContributionAccessReason::OutsideAllowedTime) => {
                "exit.contribution.outside_allowed_time"
            }
            Self::AccessDenied(ExitContributionAccessReason::BandwidthLimitExceeded) => {
                "exit.contribution.bandwidth_limit_exceeded"
            }
            Self::AccessDenied(ExitContributionAccessReason::TrafficQuotaExceeded) => {
                "exit.contribution.traffic_quota_exceeded"
            }
            Self::AccessDenied(ExitContributionAccessReason::InvalidPolicy) => {
                "exit.contribution.invalid_policy"
            }
            Self::AccessDenied(ExitContributionAccessReason::Allowed) => {
                "exit.contribution.allowed"
            }
        }
    }
}

/// Stateful, per-network Exit admission gate. It never reads configuration
/// files on the packet path; policy and roster updates are pushed by the
/// runtime control plane.
#[derive(Debug)]
pub(crate) struct ExitContributionGate {
    owner_node_id: NodeId,
    network_id: Option<NetworkId>,
    policy: Option<NodeLocalAccessPolicy>,
    member_directory: Option<NetworkMemberDirectory>,
    member_directory_received_at_unix_ms: Option<u64>,
    usage_period_started_at_unix_ms: u64,
    bytes_forwarded: u64,
    bandwidth_window_started_at_unix_ms: Option<u64>,
    bandwidth_window_bytes: u64,
}

impl ExitContributionGate {
    pub(crate) fn new(owner_node_id: NodeId, network_id: Option<NetworkId>) -> Self {
        Self {
            owner_node_id,
            network_id,
            policy: None,
            member_directory: None,
            member_directory_received_at_unix_ms: None,
            usage_period_started_at_unix_ms: 0,
            bytes_forwarded: 0,
            bandwidth_window_started_at_unix_ms: None,
            bandwidth_window_bytes: 0,
        }
    }

    /// Replace the authoritative owner policy. A revision or content change
    /// starts new local usage accounting so a newly configured quota cannot be
    /// charged for Exit traffic forwarded before it was activated.
    pub(crate) fn replace_policy(&mut self, policy: Option<NodeLocalAccessPolicy>) {
        if self.policy == policy {
            return;
        }
        self.policy = policy;
        self.reset_usage();
    }

    pub(crate) fn replace_member_directory(
        &mut self,
        directory: NetworkMemberDirectory,
        received_at_unix_ms: u64,
    ) {
        self.member_directory = Some(directory);
        self.member_directory_received_at_unix_ms = Some(received_at_unix_ms);
    }

    pub(crate) fn clear_member_directory(&mut self) {
        self.member_directory = None;
        self.member_directory_received_at_unix_ms = None;
    }

    /// Check one authenticated Exit request without consuming quota. Call
    /// [`record_forwarded`](Self::record_forwarded) only after every remaining
    /// forwarding authorization succeeds.
    pub(crate) fn authorize(
        &self,
        requester_node_id: &NodeId,
        now_unix_ms: u64,
    ) -> Result<Option<ExitContributionAccessDecision>, ExitContributionRuntimeDenial> {
        let Some(policy) = self.policy.as_ref() else {
            // No owner-authored policy means no additional owner restriction.
            // The existing network ACL, capability and Exit role checks remain
            // mandatory and are evaluated independently by the datapath.
            return Ok(None);
        };
        let network_id = self
            .network_id
            .as_ref()
            .ok_or(ExitContributionRuntimeDenial::MissingNetworkContext)?;
        if policy.node_id != self.owner_node_id {
            return Err(ExitContributionRuntimeDenial::PolicyOwnerMismatch);
        }
        if policy.validate_at(now_unix_ms).is_err() {
            return Err(ExitContributionRuntimeDenial::InvalidPolicy);
        }

        let Some(restrictions) = policy.exit_contribution.as_ref() else {
            let decision = evaluate_exit_contribution_access(
                policy,
                requester_node_id,
                MembershipRole::Member,
                &[],
                network_id,
                ExitContributionUsage::default(),
                now_unix_ms,
            );
            return if decision.allowed {
                Ok(Some(decision))
            } else {
                Err(ExitContributionRuntimeDenial::AccessDenied(decision.reason))
            };
        };

        let directory = self
            .member_directory
            .as_ref()
            .ok_or(ExitContributionRuntimeDenial::MemberDirectoryUnavailable)?;
        if directory.network_id != *network_id {
            return Err(ExitContributionRuntimeDenial::MemberDirectoryUnavailable);
        }
        let received_at = self
            .member_directory_received_at_unix_ms
            .ok_or(ExitContributionRuntimeDenial::MemberDirectoryUnavailable)?;
        if now_unix_ms.saturating_sub(received_at) > EXIT_MEMBER_DIRECTORY_MAX_AGE_MS {
            return Err(ExitContributionRuntimeDenial::MemberDirectoryUnavailable);
        }
        let Some(member) = directory
            .members
            .iter()
            .find(|member| member.node_id == *requester_node_id)
        else {
            return Err(ExitContributionRuntimeDenial::RequesterNotInRoster);
        };
        if !member.lifecycle_state.grants_network_access() {
            return Err(ExitContributionRuntimeDenial::RequesterNotActive);
        }

        // Custom-group catalogues are not part of the authoritative runtime
        // roster yet. Passing no GroupId makes group-only selectors fail closed
        // while explicit node and role selectors remain fully enforceable.
        let usage = self.usage_for(now_unix_ms, Some(restrictions));
        let decision = evaluate_exit_contribution_access(
            policy,
            requester_node_id,
            member.role,
            &[],
            network_id,
            usage,
            now_unix_ms,
        );
        if decision.allowed {
            Ok(Some(decision))
        } else {
            Err(ExitContributionRuntimeDenial::AccessDenied(decision.reason))
        }
    }

    /// Account bytes that were accepted by every Exit forwarding check.
    pub(crate) fn record_forwarded(&mut self, bytes: usize, now_unix_ms: u64) {
        let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
        let quit_period_start = self
            .policy
            .as_ref()
            .and_then(|policy| policy.exit_contribution.as_ref())
            .and_then(|restrictions| restrictions.traffic_quota.as_ref())
            .map(|quota| quota.period.period_start(now_unix_ms));
        let period_started_at = quit_period_start
            .unwrap_or_else(|| ExitContributionQuotaPeriod::Daily.period_start(now_unix_ms));
        if self.usage_period_started_at_unix_ms != period_started_at {
            self.usage_period_started_at_unix_ms = period_started_at;
            self.bytes_forwarded = 0;
        }
        self.bytes_forwarded = self.bytes_forwarded.saturating_add(bytes);

        let window_expired = self
            .bandwidth_window_started_at_unix_ms
            .is_none_or(|started_at| {
                now_unix_ms.saturating_sub(started_at) >= EXIT_BANDWIDTH_WINDOW_MS
            });
        if window_expired {
            self.bandwidth_window_started_at_unix_ms = Some(now_unix_ms);
            self.bandwidth_window_bytes = 0;
        }
        self.bandwidth_window_bytes = self.bandwidth_window_bytes.saturating_add(bytes);
    }

    fn usage_for(
        &self,
        now_unix_ms: u64,
        restrictions: Option<&ExitContributionRestrictions>,
    ) -> ExitContributionUsage {
        let quota_period_start = restrictions
            .and_then(|restrictions| restrictions.traffic_quota.as_ref())
            .map(|quota| quota.period.period_start(now_unix_ms));
        let bytes_forwarded = match quota_period_start {
            Some(period_start) if self.usage_period_started_at_unix_ms == period_start => {
                self.bytes_forwarded
            }
            Some(_) => 0,
            None if self.usage_period_started_at_unix_ms
                == ExitContributionQuotaPeriod::Daily.period_start(now_unix_ms) =>
            {
                self.bytes_forwarded
            }
            None => 0,
        };
        let observed_bandwidth_kbps = self
            .bandwidth_window_started_at_unix_ms
            .and_then(|started_at| {
                let elapsed_ms = now_unix_ms.saturating_sub(started_at);
                (elapsed_ms > 0 && elapsed_ms < EXIT_BANDWIDTH_WINDOW_MS).then(|| {
                    self.bandwidth_window_bytes
                        .saturating_mul(8)
                        .checked_div(elapsed_ms)
                        .unwrap_or(u64::MAX)
                })
            })
            .unwrap_or(0);
        ExitContributionUsage {
            period_started_at_unix_ms: quota_period_start
                .unwrap_or(self.usage_period_started_at_unix_ms),
            bytes_forwarded,
            observed_bandwidth_kbps,
        }
    }

    fn reset_usage(&mut self) {
        self.usage_period_started_at_unix_ms = 0;
        self.bytes_forwarded = 0;
        self.bandwidth_window_started_at_unix_ms = None;
        self.bandwidth_window_bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuckipv6_common::domain::{
        ContributionKind, ExitContributionMemberScope, ExitContributionQuotaPeriod,
        ExitContributionTimeWindow, ExitContributionTrafficQuota, NetworkMember,
        NetworkMemberGroup, NetworkMemberOnlineState, NetworkMemberPresence, NetworkMemberStatus,
        NodeContributionVeto, NodeNetworkScope, SecurityRuleId,
    };

    const NOW: u64 = 1_700_006_400_000; // 2023-11-15 00:00 UTC, Wednesday.

    fn network() -> NetworkId {
        NetworkId::from_public_key(b"exit-runtime-network")
    }

    fn owner() -> NodeId {
        NodeId::from_public_key(b"exit-runtime-owner")
    }

    fn member_node() -> NodeId {
        NodeId::from_public_key(b"exit-runtime-member")
    }

    fn policy(restrictions: ExitContributionRestrictions) -> NodeLocalAccessPolicy {
        NodeLocalAccessPolicy {
            node_id: owner(),
            revision: 7,
            issued_at_unix_ms: NOW.saturating_sub(1_000),
            expires_at_unix_ms: NOW + 86_400_000,
            network_scope: NodeNetworkScope::All,
            contribution_vetoes: Vec::new(),
            exit_contribution: Some(restrictions),
            rules: Vec::new(),
        }
    }

    fn member(role: MembershipRole, lifecycle_state: MemberLifecycleStatus) -> NetworkMember {
        NetworkMember {
            node_id: member_node(),
            display_name: None,
            identity_fingerprint: None,
            role,
            groups: vec![match role {
                MembershipRole::Owner => NetworkMemberGroup::Owner,
                MembershipRole::Administrator => NetworkMemberGroup::Administrator,
                MembershipRole::Member => NetworkMemberGroup::Member,
            }],
            capabilities: None,
            status: NetworkMemberStatus::Active,
            lifecycle_state,
            revision: 1,
            access_epoch: 1,
            available_actions: Vec::new(),
            presence: NetworkMemberPresence {
                state: NetworkMemberOnlineState::Online,
                source: fuckipv6_common::domain::NetworkMemberObservationSource::LiveSession,
                observed_at_unix_ms: Some(NOW),
            },
            exit_availability: fuckipv6_common::domain::ExitContributionAvailability::default(),
            joined_at_unix_ms: Some(NOW.saturating_sub(10_000)),
        }
    }

    fn directory(member: NetworkMember) -> NetworkMemberDirectory {
        NetworkMemberDirectory {
            network_id: network(),
            generated_at_unix_ms: NOW,
            viewer_role: MembershipRole::Owner,
            can_manage_members: true,
            members: vec![member],
        }
    }

    fn selected_nodes(nodes: Vec<NodeId>) -> ExitContributionRestrictions {
        ExitContributionRestrictions {
            members: ExitContributionMemberScope::Selected {
                roles: Vec::new(),
                groups: Vec::new(),
                nodes,
            },
            ..Default::default()
        }
    }

    fn gate(restrictions: ExitContributionRestrictions) -> ExitContributionGate {
        let mut gate = ExitContributionGate::new(owner(), Some(network()));
        gate.replace_policy(Some(policy(restrictions)));
        gate.replace_member_directory(
            directory(member(
                MembershipRole::Member,
                MemberLifecycleStatus::Active,
            )),
            NOW,
        );
        gate
    }

    #[test]
    fn remote_member_is_evaluated_against_owner_policy_and_active_roster() {
        let gate = gate(selected_nodes(vec![member_node()]));
        assert_ne!(owner(), member_node());
        assert!(gate.authorize(&member_node(), NOW).unwrap().is_some());
        assert!(matches!(
            gate.authorize(&NodeId::from_public_key(b"unlisted"), NOW),
            Err(ExitContributionRuntimeDenial::RequesterNotInRoster)
        ));
    }

    #[test]
    fn network_scope_and_local_veto_fail_closed() {
        let mut network_policy = policy(selected_nodes(vec![member_node()]));
        network_policy.network_scope =
            NodeNetworkScope::Only(vec![NetworkId::from_public_key(b"other-network")]);
        let mut gate = ExitContributionGate::new(owner(), Some(network()));
        gate.replace_policy(Some(network_policy));
        gate.replace_member_directory(
            directory(member(
                MembershipRole::Member,
                MemberLifecycleStatus::Active,
            )),
            NOW,
        );
        assert_eq!(
            gate.authorize(&member_node(), NOW),
            Err(ExitContributionRuntimeDenial::AccessDenied(
                ExitContributionAccessReason::NetworkNotAllowed
            ))
        );

        let mut veto_policy = policy(selected_nodes(vec![member_node()]));
        veto_policy.contribution_vetoes = vec![NodeContributionVeto {
            veto_id: SecurityRuleId::from_public_key(b"exit-veto"),
            network_id: network(),
            contributions: vec![ContributionKind::Exit],
        }];
        let mut gate = ExitContributionGate::new(owner(), Some(network()));
        gate.replace_policy(Some(veto_policy));
        gate.replace_member_directory(
            directory(member(
                MembershipRole::Member,
                MemberLifecycleStatus::Active,
            )),
            NOW,
        );
        assert_eq!(
            gate.authorize(&member_node(), NOW),
            Err(ExitContributionRuntimeDenial::AccessDenied(
                ExitContributionAccessReason::LocalVeto
            ))
        );
    }

    #[test]
    fn selected_role_is_enforced_and_group_only_scope_fails_closed() {
        let mut role_policy = selected_nodes(Vec::new());
        if let ExitContributionMemberScope::Selected { roles, .. } = &mut role_policy.members {
            roles.push(MembershipRole::Member);
        }
        let mut gate = ExitContributionGate::new(owner(), Some(network()));
        gate.replace_policy(Some(policy(role_policy)));
        gate.replace_member_directory(
            directory(member(
                MembershipRole::Member,
                MemberLifecycleStatus::Active,
            )),
            NOW,
        );
        assert!(gate.authorize(&member_node(), NOW).unwrap().is_some());

        let group_only = ExitContributionRestrictions {
            members: ExitContributionMemberScope::Selected {
                roles: Vec::new(),
                groups: vec![fuckipv6_common::domain::GroupId::from_public_key(
                    b"custom-group",
                )],
                nodes: Vec::new(),
            },
            ..Default::default()
        };
        let mut gate = ExitContributionGate::new(owner(), Some(network()));
        gate.replace_policy(Some(policy(group_only)));
        gate.replace_member_directory(
            directory(member(
                MembershipRole::Member,
                MemberLifecycleStatus::Active,
            )),
            NOW,
        );
        assert_eq!(
            gate.authorize(&member_node(), NOW),
            Err(ExitContributionRuntimeDenial::AccessDenied(
                ExitContributionAccessReason::MemberNotAllowed
            ))
        );
    }

    #[test]
    fn missing_or_stale_roster_and_inactive_member_are_denied() {
        let restrictions = selected_nodes(vec![member_node()]);
        let mut gate = ExitContributionGate::new(owner(), Some(network()));
        gate.replace_policy(Some(policy(restrictions.clone())));
        assert_eq!(
            gate.authorize(&member_node(), NOW),
            Err(ExitContributionRuntimeDenial::MemberDirectoryUnavailable)
        );

        gate.replace_member_directory(
            directory(member(
                MembershipRole::Member,
                MemberLifecycleStatus::Suspended,
            )),
            NOW,
        );
        assert_eq!(
            gate.authorize(&member_node(), NOW),
            Err(ExitContributionRuntimeDenial::RequesterNotActive)
        );

        gate.replace_member_directory(
            directory(member(
                MembershipRole::Member,
                MemberLifecycleStatus::Active,
            )),
            NOW.saturating_sub(EXIT_MEMBER_DIRECTORY_MAX_AGE_MS + 1),
        );
        assert_eq!(
            gate.authorize(&member_node(), NOW),
            Err(ExitContributionRuntimeDenial::MemberDirectoryUnavailable)
        );
    }

    #[test]
    fn policy_owner_mismatch_and_policy_replacement_take_effect_immediately() {
        let mut wrong_owner = policy(selected_nodes(vec![member_node()]));
        wrong_owner.node_id = NodeId::from_public_key(b"another-owner");
        let mut gate = ExitContributionGate::new(owner(), Some(network()));
        gate.replace_policy(Some(wrong_owner));
        assert_eq!(
            gate.authorize(&member_node(), NOW),
            Err(ExitContributionRuntimeDenial::PolicyOwnerMismatch)
        );

        gate.replace_policy(None);
        assert_eq!(gate.authorize(&member_node(), NOW), Ok(None));
        gate.replace_policy(Some(policy(selected_nodes(vec![NodeId::from_public_key(
            b"different-member",
        )]))));
        gate.replace_member_directory(
            directory(member(
                MembershipRole::Member,
                MemberLifecycleStatus::Active,
            )),
            NOW,
        );
        assert_eq!(
            gate.authorize(&member_node(), NOW),
            Err(ExitContributionRuntimeDenial::AccessDenied(
                ExitContributionAccessReason::MemberNotAllowed
            ))
        );
    }

    #[test]
    fn utc_time_bandwidth_and_quota_limits_are_enforced() {
        let restrictions = ExitContributionRestrictions {
            members: ExitContributionMemberScope::AllMembers,
            time_windows_utc: vec![ExitContributionTimeWindow {
                weekdays: vec![2],
                start_minute_utc: 0,
                end_minute_utc: 60,
            }],
            max_bandwidth_kbps: Some(1_000),
            traffic_quota: Some(ExitContributionTrafficQuota {
                max_bytes: 200_000,
                period: ExitContributionQuotaPeriod::Daily,
            }),
        };
        let mut gate = gate(restrictions);
        let outside_window = NOW + 60 * 60 * 1_000;
        gate.replace_member_directory(
            directory(member(
                MembershipRole::Member,
                MemberLifecycleStatus::Active,
            )),
            outside_window,
        );
        assert_eq!(
            gate.authorize(&member_node(), outside_window),
            Err(ExitContributionRuntimeDenial::AccessDenied(
                ExitContributionAccessReason::OutsideAllowedTime
            ))
        );

        gate.replace_member_directory(
            directory(member(
                MembershipRole::Member,
                MemberLifecycleStatus::Active,
            )),
            NOW,
        );
        gate.record_forwarded(100_000, NOW);
        assert_eq!(
            gate.authorize(&member_node(), NOW + 500),
            Err(ExitContributionRuntimeDenial::AccessDenied(
                ExitContributionAccessReason::BandwidthLimitExceeded
            ))
        );

        gate.record_forwarded(100_000, NOW + 501);
        assert_eq!(
            gate.authorize(&member_node(), NOW + 4_000),
            Err(ExitContributionRuntimeDenial::AccessDenied(
                ExitContributionAccessReason::TrafficQuotaExceeded
            ))
        );
    }
}
