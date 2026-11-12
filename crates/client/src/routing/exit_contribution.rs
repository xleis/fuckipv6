//! Runtime enforcement of node-owner Exit contribution restrictions.
//!
//! The shared domain evaluator owns the actual rule semantics. This module adds
//! the trusted runtime inputs that a packet datapath needs: the local owner
//! identity, the authoritative member roster, and Exit-only usage accounting.

use fuckipv6_common::domain::{
    authorize_exit_contribution, evaluate_exit_contribution_access, ContentHash,
    ExitContributionAccessDecision, ExitContributionAccessReason,
    ExitContributionAuthorizationInput, ExitContributionAuthorizationReason,
    ExitContributionAuthorizationSnapshot, ExitContributionMode, ExitContributionQuotaPeriod,
    ExitContributionRestrictions, ExitContributionUsage, NetworkId, NetworkMemberDirectory, NodeId,
    NodeLocalAccessPolicy,
};
use std::collections::BTreeMap;

/// A roster older than this is not safe evidence for an owner restriction.
pub(crate) const EXIT_MEMBER_DIRECTORY_MAX_AGE_MS: u64 = 90_000;
const EXIT_BANDWIDTH_WINDOW_MS: u64 = 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExitContributionNetworkContext {
    pub(crate) mode: ExitContributionMode,
    pub(crate) policy_revision: u64,
    pub(crate) terms_hash: ContentHash,
    pub(crate) policy_confirmation_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExitContributionRuntimeDenial {
    MissingNetworkContext,
    NetworkContextUnavailable,
    PolicyUnavailable,
    PolicyOwnerMismatch,
    InvalidPolicy,
    PolicyConfirmationRequired,
    MemberDirectoryUnavailable,
    RequesterNotInRoster,
    RequesterNotActive,
    AccessDenied(ExitContributionAccessReason),
    AuthorizationDenied(ExitContributionAuthorizationReason),
}

impl ExitContributionRuntimeDenial {
    pub(crate) const fn reason_code(&self) -> &'static str {
        match self {
            Self::MissingNetworkContext => "exit.contribution.missing_network_context",
            Self::NetworkContextUnavailable => "exit.contribution.network_context_unavailable",
            Self::PolicyUnavailable => "exit.contribution.policy_unavailable",
            Self::PolicyOwnerMismatch => "exit.contribution.policy_owner_mismatch",
            Self::InvalidPolicy => "exit.contribution.invalid_policy",
            Self::PolicyConfirmationRequired => "exit.contribution.policy_confirmation_required",
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
            Self::AuthorizationDenied(ExitContributionAuthorizationReason::Authorized) => {
                "exit.contribution.authorized"
            }
            Self::AuthorizationDenied(ExitContributionAuthorizationReason::NetworkDisabled) => {
                "exit.contribution.network_disabled"
            }
            Self::AuthorizationDenied(
                ExitContributionAuthorizationReason::AuthorizationRequired,
            ) => "exit.contribution.authorization_required",
            Self::AuthorizationDenied(
                ExitContributionAuthorizationReason::AuthorizationPending,
            ) => "exit.contribution.authorization_pending",
            Self::AuthorizationDenied(
                ExitContributionAuthorizationReason::AuthorizationRejected,
            ) => "exit.contribution.authorization_rejected",
            Self::AuthorizationDenied(
                ExitContributionAuthorizationReason::AuthorizationExpired,
            ) => "exit.contribution.authorization_expired",
            Self::AuthorizationDenied(
                ExitContributionAuthorizationReason::AuthorizationSuperseded,
            ) => "exit.contribution.authorization_superseded",
            Self::AuthorizationDenied(ExitContributionAuthorizationReason::LocalVeto) => {
                "exit.contribution.local_veto"
            }
            Self::AuthorizationDenied(ExitContributionAuthorizationReason::LocalRestriction) => {
                "exit.contribution.local_restriction"
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
    network_context: Option<ExitContributionNetworkContext>,
    policy: Option<NodeLocalAccessPolicy>,
    member_directory: Option<NetworkMemberDirectory>,
    member_directory_received_at_unix_ms: Option<u64>,
    authorization_requests: BTreeMap<NodeId, ExitContributionAuthorizationSnapshot>,
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
            network_context: None,
            policy: None,
            member_directory: None,
            member_directory_received_at_unix_ms: None,
            authorization_requests: BTreeMap::new(),
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

    /// Replace the signed network terms that authorize Exit contribution. A
    /// missing context is never equivalent to permission.
    pub(crate) fn replace_network_context(&mut self, context: ExitContributionNetworkContext) {
        self.network_context = Some(context);
    }

    pub(crate) fn has_network_context(&self) -> bool {
        self.network_context.is_some()
    }

    pub(crate) fn clear_network_context(&mut self) {
        self.network_context = None;
        self.clear_authorization_requests();
    }

    pub(crate) fn replace_member_directory(
        &mut self,
        directory: NetworkMemberDirectory,
        received_at_unix_ms: u64,
    ) -> Result<(), ExitContributionRuntimeDenial> {
        let network_id = self
            .network_id
            .as_ref()
            .ok_or(ExitContributionRuntimeDenial::MissingNetworkContext)?;
        if directory.network_id != *network_id {
            return Err(ExitContributionRuntimeDenial::MemberDirectoryUnavailable);
        }
        self.member_directory = Some(directory);
        self.member_directory_received_at_unix_ms = Some(received_at_unix_ms);
        Ok(())
    }

    pub(crate) fn clear_member_directory(&mut self) {
        self.member_directory = None;
        self.member_directory_received_at_unix_ms = None;
    }

    /// Cache the latest Coordinator-reviewed authorization projection for the
    /// authenticated member. Full request bodies never enter the Exit data
    /// plane. Foreign networks and older revisions are ignored rather than
    /// widening this gate's authority.
    #[cfg(test)]
    pub(crate) fn replace_authorization_snapshot(
        &mut self,
        snapshot: ExitContributionAuthorizationSnapshot,
    ) {
        if self.network_id.as_ref() != Some(&snapshot.network_id) {
            return;
        }
        let should_replace = self
            .authorization_requests
            .get(&snapshot.node_id)
            .is_none_or(|current| snapshot.revision >= current.revision);
        if should_replace {
            self.authorization_requests
                .insert(snapshot.node_id.clone(), snapshot);
        }
    }

    #[cfg(test)]
    pub(crate) fn clear_authorization_request(&mut self, requester_node_id: &NodeId) {
        self.authorization_requests.remove(requester_node_id);
    }

    pub(crate) fn clear_authorization_requests(&mut self) {
        self.authorization_requests.clear();
    }

    /// Check one authenticated Exit request without consuming quota. Call
    /// [`record_forwarded`](Self::record_forwarded) only after every remaining
    /// forwarding authorization succeeds.
    pub(crate) fn authorize(
        &self,
        requester_node_id: &NodeId,
        now_unix_ms: u64,
    ) -> Result<ExitContributionAccessDecision, ExitContributionRuntimeDenial> {
        let network_id = self
            .network_id
            .as_ref()
            .ok_or(ExitContributionRuntimeDenial::MissingNetworkContext)?;
        let network_context = self
            .network_context
            .as_ref()
            .ok_or(ExitContributionRuntimeDenial::NetworkContextUnavailable)?;
        if network_context.policy_confirmation_required {
            return Err(ExitContributionRuntimeDenial::PolicyConfirmationRequired);
        }
        let policy = self
            .policy
            .as_ref()
            .ok_or(ExitContributionRuntimeDenial::PolicyUnavailable)?;
        if policy.node_id != self.owner_node_id {
            return Err(ExitContributionRuntimeDenial::PolicyOwnerMismatch);
        }
        if policy.validate_at(now_unix_ms).is_err() {
            return Err(ExitContributionRuntimeDenial::InvalidPolicy);
        }

        let member = self.authoritative_member(requester_node_id, now_unix_ms)?;

        // Custom-group catalogues are not part of the authoritative runtime
        // roster yet. Passing no GroupId makes group-only selectors fail closed
        // while explicit node and role selectors remain fully enforceable.
        let usage = self.usage_for(now_unix_ms, policy.exit_contribution.as_ref());
        let decision = evaluate_exit_contribution_access(
            policy,
            requester_node_id,
            member.role,
            &[],
            network_id,
            usage,
            now_unix_ms,
        );
        if !decision.allowed {
            return Err(ExitContributionRuntimeDenial::AccessDenied(decision.reason));
        }

        let authorization = authorize_exit_contribution(ExitContributionAuthorizationInput {
            mode: network_context.mode,
            policy_revision: network_context.policy_revision,
            terms_hash: &network_context.terms_hash,
            request: self.authorization_requests.get(requester_node_id),
            local_veto: false,
            local_access_allowed: true,
            now_unix_ms,
        });
        if authorization != ExitContributionAuthorizationReason::Authorized {
            return Err(ExitContributionRuntimeDenial::AuthorizationDenied(
                authorization,
            ));
        }
        Ok(decision)
    }

    fn authoritative_member(
        &self,
        requester_node_id: &NodeId,
        now_unix_ms: u64,
    ) -> Result<&fuckipv6_common::domain::NetworkMember, ExitContributionRuntimeDenial> {
        let network_id = self
            .network_id
            .as_ref()
            .ok_or(ExitContributionRuntimeDenial::MissingNetworkContext)?;
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
        let member = directory
            .members
            .iter()
            .find(|member| member.node_id == *requester_node_id)
            .ok_or(ExitContributionRuntimeDenial::RequesterNotInRoster)?;
        if !member.lifecycle_state.grants_network_access() {
            return Err(ExitContributionRuntimeDenial::RequesterNotActive);
        }
        Ok(member)
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
        ExitContributionRequest, ExitContributionRequestId, ExitContributionRequestState,
        ExitContributionReviewAction, ExitContributionTimeWindow, ExitContributionTrafficQuota,
        MemberLifecycleStatus, MembershipRole, NetworkMember, NetworkMemberGroup,
        NetworkMemberOnlineState, NetworkMemberPresence, NetworkMemberStatus, NodeContributionVeto,
        NodeNetworkScope, SecurityRuleId,
    };

    const NOW: u64 = 1_700_006_400_000; // 2023-11-15 00:00 UTC, Wednesday.

    fn network() -> NetworkId {
        NetworkId::from_public_key(b"exit-runtime-network")
    }

    fn other_network() -> NetworkId {
        NetworkId::from_public_key(b"exit-runtime-other-network")
    }

    fn owner() -> NodeId {
        NodeId::from_public_key(b"exit-runtime-owner")
    }

    fn member_node() -> NodeId {
        NodeId::from_public_key(b"exit-runtime-member")
    }

    fn terms_hash() -> ContentHash {
        ContentHash::from_bytes(b"exit-runtime-terms-v1")
    }

    fn context(mode: ExitContributionMode) -> ExitContributionNetworkContext {
        ExitContributionNetworkContext {
            mode,
            policy_revision: 11,
            terms_hash: terms_hash(),
            policy_confirmation_required: false,
        }
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
            relay_limits: Default::default(),
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
            exit_selection_policy: Default::default(),
            exit_selection_policy_revision: 0,
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

    fn gate(
        restrictions: ExitContributionRestrictions,
        mode: ExitContributionMode,
    ) -> ExitContributionGate {
        let mut gate = ExitContributionGate::new(owner(), Some(network()));
        gate.replace_policy(Some(policy(restrictions)));
        gate.replace_network_context(context(mode));
        gate.replace_member_directory(
            directory(member(
                MembershipRole::Member,
                MemberLifecycleStatus::Active,
            )),
            NOW,
        )
        .unwrap();
        gate
    }

    fn pending_request() -> ExitContributionRequest {
        ExitContributionRequest::submit(
            ExitContributionRequestId::from_public_key(b"exit-runtime-request"),
            network(),
            member_node(),
            11,
            terms_hash(),
            Some(20_000),
            Some(1_000_000),
            NOW,
        )
        .unwrap()
    }

    fn approved_request() -> ExitContributionRequest {
        pending_request()
            .review(
                NodeId::from_public_key(b"exit-runtime-reviewer"),
                1,
                ExitContributionReviewAction::Approve,
                Some("exit.approved.standard".into()),
                NOW + 1,
            )
            .unwrap()
            .request
    }

    fn rejected_request() -> ExitContributionRequest {
        pending_request()
            .review(
                NodeId::from_public_key(b"exit-runtime-reviewer"),
                1,
                ExitContributionReviewAction::Reject,
                Some("exit.rejected.policy".into()),
                NOW + 1,
            )
            .unwrap()
            .request
    }

    #[test]
    fn remote_member_is_evaluated_against_owner_policy_and_active_roster() {
        let gate = gate(
            selected_nodes(vec![member_node()]),
            ExitContributionMode::Autonomous,
        );
        assert!(gate.authorize(&member_node(), NOW).is_ok());
        assert_eq!(
            gate.authorize(&NodeId::from_public_key(b"unlisted"), NOW),
            Err(ExitContributionRuntimeDenial::RequesterNotInRoster)
        );
    }

    #[test]
    fn absent_network_context_policy_confirmation_and_roster_fail_closed() {
        let mut gate = ExitContributionGate::new(owner(), Some(network()));
        gate.replace_policy(Some(policy(ExitContributionRestrictions::default())));
        assert_eq!(
            gate.authorize(&member_node(), NOW),
            Err(ExitContributionRuntimeDenial::NetworkContextUnavailable)
        );

        gate.replace_network_context(context(ExitContributionMode::Autonomous));
        assert_eq!(
            gate.authorize(&member_node(), NOW),
            Err(ExitContributionRuntimeDenial::MemberDirectoryUnavailable)
        );

        gate.replace_member_directory(
            directory(member(
                MembershipRole::Member,
                MemberLifecycleStatus::Active,
            )),
            NOW,
        )
        .unwrap();
        gate.replace_policy(None);
        assert_eq!(
            gate.authorize(&member_node(), NOW),
            Err(ExitContributionRuntimeDenial::PolicyUnavailable)
        );

        gate.replace_policy(Some(policy(ExitContributionRestrictions::default())));
        let mut unconfirmed = context(ExitContributionMode::RequiredOnJoin);
        unconfirmed.policy_confirmation_required = true;
        gate.replace_network_context(unconfirmed);
        assert_eq!(
            gate.authorize(&member_node(), NOW),
            Err(ExitContributionRuntimeDenial::PolicyConfirmationRequired)
        );
    }

    #[test]
    fn network_scope_and_local_veto_fail_closed() {
        let mut network_policy = policy(selected_nodes(vec![member_node()]));
        network_policy.network_scope = NodeNetworkScope::Only(vec![other_network()]);
        let mut gate = ExitContributionGate::new(owner(), Some(network()));
        gate.replace_policy(Some(network_policy));
        gate.replace_network_context(context(ExitContributionMode::Autonomous));
        gate.replace_member_directory(
            directory(member(
                MembershipRole::Member,
                MemberLifecycleStatus::Active,
            )),
            NOW,
        )
        .unwrap();
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
        gate.replace_network_context(context(ExitContributionMode::RequiredOnJoin));
        gate.replace_member_directory(
            directory(member(
                MembershipRole::Member,
                MemberLifecycleStatus::Active,
            )),
            NOW,
        )
        .unwrap();
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
        let role_gate = gate(role_policy, ExitContributionMode::Autonomous);
        assert!(role_gate.authorize(&member_node(), NOW).is_ok());

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
        let gate = gate(group_only, ExitContributionMode::Autonomous);
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
        gate.replace_policy(Some(policy(restrictions)));
        gate.replace_network_context(context(ExitContributionMode::Autonomous));
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
        )
        .unwrap();
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
        )
        .unwrap();
        assert_eq!(
            gate.authorize(&member_node(), NOW),
            Err(ExitContributionRuntimeDenial::MemberDirectoryUnavailable)
        );

        let foreign = NetworkMemberDirectory {
            network_id: other_network(),
            ..directory(member(
                MembershipRole::Member,
                MemberLifecycleStatus::Active,
            ))
        };
        assert_eq!(
            gate.replace_member_directory(foreign, NOW),
            Err(ExitContributionRuntimeDenial::MemberDirectoryUnavailable)
        );
    }

    #[test]
    fn policy_owner_mismatch_and_policy_replacement_take_effect_immediately() {
        let mut wrong_owner = policy(selected_nodes(vec![member_node()]));
        wrong_owner.node_id = NodeId::from_public_key(b"another-owner");
        let mut gate = ExitContributionGate::new(owner(), Some(network()));
        gate.replace_policy(Some(wrong_owner));
        gate.replace_network_context(context(ExitContributionMode::Autonomous));
        gate.replace_member_directory(
            directory(member(
                MembershipRole::Member,
                MemberLifecycleStatus::Active,
            )),
            NOW,
        )
        .unwrap();
        assert_eq!(
            gate.authorize(&member_node(), NOW),
            Err(ExitContributionRuntimeDenial::PolicyOwnerMismatch)
        );

        gate.replace_policy(Some(policy(selected_nodes(vec![NodeId::from_public_key(
            b"different-member",
        )]))));
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
        let mut gate = gate(restrictions, ExitContributionMode::Autonomous);
        let outside_window = NOW + 60 * 60 * 1_000;
        gate.replace_member_directory(
            directory(member(
                MembershipRole::Member,
                MemberLifecycleStatus::Active,
            )),
            outside_window,
        )
        .unwrap();
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
        )
        .unwrap();
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

    #[test]
    fn four_network_modes_and_approval_states_are_enforced() {
        assert!(gate(
            ExitContributionRestrictions::default(),
            ExitContributionMode::Autonomous
        )
        .authorize(&member_node(), NOW)
        .is_ok());
        assert!(gate(
            ExitContributionRestrictions::default(),
            ExitContributionMode::RequiredOnJoin
        )
        .authorize(&member_node(), NOW)
        .is_ok());

        let disabled = gate(
            ExitContributionRestrictions::default(),
            ExitContributionMode::Disabled,
        );
        assert_eq!(
            disabled.authorize(&member_node(), NOW),
            Err(ExitContributionRuntimeDenial::AuthorizationDenied(
                ExitContributionAuthorizationReason::NetworkDisabled
            ))
        );

        let mut approval = gate(
            ExitContributionRestrictions::default(),
            ExitContributionMode::ApprovalRequired,
        );
        assert_eq!(
            approval.authorize(&member_node(), NOW),
            Err(ExitContributionRuntimeDenial::AuthorizationDenied(
                ExitContributionAuthorizationReason::AuthorizationRequired
            ))
        );

        approval.replace_authorization_snapshot(pending_request().authorization_snapshot());
        assert_eq!(
            approval.authorize(&member_node(), NOW + 2),
            Err(ExitContributionRuntimeDenial::AuthorizationDenied(
                ExitContributionAuthorizationReason::AuthorizationPending
            ))
        );

        approval.replace_authorization_snapshot(approved_request().authorization_snapshot());
        assert!(approval.authorize(&member_node(), NOW + 2).is_ok());

        let mut rejected = rejected_request();
        rejected.revision = 3;
        approval.replace_authorization_snapshot(rejected.authorization_snapshot());
        assert_eq!(
            approval.authorize(&member_node(), NOW + 2),
            Err(ExitContributionRuntimeDenial::AuthorizationDenied(
                ExitContributionAuthorizationReason::AuthorizationRejected
            ))
        );

        let mut expired = pending_request();
        expired.expires_at_unix_ms = NOW + 3;
        expired.revision = 4;
        approval.replace_authorization_snapshot(expired.authorization_snapshot());
        assert_eq!(
            approval.authorize(&member_node(), NOW + 4),
            Err(ExitContributionRuntimeDenial::AuthorizationDenied(
                ExitContributionAuthorizationReason::AuthorizationExpired
            ))
        );

        let mut stale_context = context(ExitContributionMode::ApprovalRequired);
        stale_context.policy_revision += 1;
        approval.replace_network_context(stale_context);
        let mut stale_approval = approved_request();
        stale_approval.revision = 5;
        approval.replace_authorization_snapshot(stale_approval.authorization_snapshot());
        assert_eq!(
            approval.authorize(&member_node(), NOW + 2),
            Err(ExitContributionRuntimeDenial::AuthorizationDenied(
                ExitContributionAuthorizationReason::AuthorizationSuperseded
            ))
        );
    }

    #[test]
    fn approval_cache_is_scoped_to_the_gate_network_and_can_be_cleared() {
        let mut approval = gate(
            ExitContributionRestrictions::default(),
            ExitContributionMode::ApprovalRequired,
        );
        let mut foreign = approved_request();
        foreign.network_id = other_network();
        approval.replace_authorization_snapshot(foreign.authorization_snapshot());
        assert_eq!(
            approval.authorize(&member_node(), NOW + 2),
            Err(ExitContributionRuntimeDenial::AuthorizationDenied(
                ExitContributionAuthorizationReason::AuthorizationRequired
            ))
        );

        approval.replace_authorization_snapshot(approved_request().authorization_snapshot());
        assert!(approval.authorize(&member_node(), NOW + 2).is_ok());
        approval.replace_authorization_snapshot(pending_request().authorization_snapshot());
        assert!(approval.authorize(&member_node(), NOW + 2).is_ok());
        approval.clear_authorization_request(&member_node());
        assert_eq!(
            approval.authorize(&member_node(), NOW + 2),
            Err(ExitContributionRuntimeDenial::AuthorizationDenied(
                ExitContributionAuthorizationReason::AuthorizationRequired
            ))
        );
    }

    #[test]
    fn denial_reason_codes_are_stable_and_never_report_implicit_permission() {
        assert_eq!(
            ExitContributionRuntimeDenial::NetworkContextUnavailable.reason_code(),
            "exit.contribution.network_context_unavailable"
        );
        assert_eq!(
            ExitContributionRuntimeDenial::PolicyUnavailable.reason_code(),
            "exit.contribution.policy_unavailable"
        );
        assert_eq!(
            ExitContributionRuntimeDenial::PolicyConfirmationRequired.reason_code(),
            "exit.contribution.policy_confirmation_required"
        );
        assert_eq!(
            ExitContributionRuntimeDenial::AuthorizationDenied(
                ExitContributionAuthorizationReason::AuthorizationRequired
            )
            .reason_code(),
            "exit.contribution.authorization_required"
        );
        assert_eq!(
            approved_request().state,
            ExitContributionRequestState::Approved
        );
        assert_eq!(
            pending_request().state,
            ExitContributionRequestState::Pending
        );
    }
}
