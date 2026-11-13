//! Exit contribution governance shared by every node UI and runtime.
//!
//! Network policy decides whether contribution is disabled, voluntary,
//! mandatory on join or approval-gated. Node-owner restrictions are evaluated
//! independently and always win. Availability is a three-state observation:
//! missing evidence is `unknown`, never healthy.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ids::{ContentHash, ExitContributionRequestId, GroupId, NetworkId, NodeId};
use super::membership::{ExitContributionMode, MembershipRole};
use super::{DomainError, DomainResult};

pub const EXIT_CONTRIBUTION_REQUEST_SCHEMA_VERSION: u16 = 1;
pub const EXIT_CONTRIBUTION_REQUEST_DEFAULT_EXPIRY_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// Structured disclosure rendered before joining and whenever the signed Exit
/// policy changes. The enum values are stable contract tokens; localized copy
/// belongs to the UI layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitContributionBandwidthImpact {
    None,
    OptionalMemberCapacity,
    RequiredMemberCapacity,
    AfterApproval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitContributionTrafficImpact {
    None,
    OptionalForwarding,
    RequiredForwarding,
    ForwardingAfterApproval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitContributionPrivacyImpact {
    NoExitTraffic,
    ThisNodeObservesForwardedTraffic,
    ThisNodeObservesForwardedTrafficAfterApproval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitContributionOperationalImpact {
    None,
    OwnerControlsAvailability,
    RequiredWhileMember,
    OwnerControlsAfterApproval,
}

/// All four impact axes are present together so an adapter cannot display only
/// bandwidth and hide privacy or operational consequences.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ExitContributionDisclosure {
    pub bandwidth: ExitContributionBandwidthImpact,
    pub traffic: ExitContributionTrafficImpact,
    pub privacy: ExitContributionPrivacyImpact,
    pub operations: ExitContributionOperationalImpact,
    /// The disclosure must be reviewed before joining and whenever the signed
    /// Exit policy changes. This applies even when contribution is disabled.
    pub requires_join_confirmation: bool,
    /// Only `RequiredOnJoin` asks the member to make a separate contribution
    /// commitment. Voluntary and approval-based modes require an explicit
    /// opt-in/request later, but joining itself does not create a duty.
    #[serde(default)]
    pub requires_explicit_commitment: bool,
    pub requires_authorization: bool,
}

impl ExitContributionDisclosure {
    pub const fn for_mode(mode: ExitContributionMode) -> Self {
        match mode {
            ExitContributionMode::Disabled => Self {
                bandwidth: ExitContributionBandwidthImpact::None,
                traffic: ExitContributionTrafficImpact::None,
                privacy: ExitContributionPrivacyImpact::NoExitTraffic,
                operations: ExitContributionOperationalImpact::None,
                requires_join_confirmation: true,
                requires_explicit_commitment: false,
                requires_authorization: false,
            },
            ExitContributionMode::Autonomous => Self {
                bandwidth: ExitContributionBandwidthImpact::OptionalMemberCapacity,
                traffic: ExitContributionTrafficImpact::OptionalForwarding,
                privacy: ExitContributionPrivacyImpact::ThisNodeObservesForwardedTraffic,
                operations: ExitContributionOperationalImpact::OwnerControlsAvailability,
                requires_join_confirmation: true,
                requires_explicit_commitment: false,
                requires_authorization: false,
            },
            ExitContributionMode::RequiredOnJoin => Self {
                bandwidth: ExitContributionBandwidthImpact::RequiredMemberCapacity,
                traffic: ExitContributionTrafficImpact::RequiredForwarding,
                privacy: ExitContributionPrivacyImpact::ThisNodeObservesForwardedTraffic,
                operations: ExitContributionOperationalImpact::RequiredWhileMember,
                requires_join_confirmation: true,
                requires_explicit_commitment: true,
                requires_authorization: false,
            },
            ExitContributionMode::ApprovalRequired => Self {
                bandwidth: ExitContributionBandwidthImpact::AfterApproval,
                traffic: ExitContributionTrafficImpact::ForwardingAfterApproval,
                privacy:
                    ExitContributionPrivacyImpact::ThisNodeObservesForwardedTrafficAfterApproval,
                operations: ExitContributionOperationalImpact::OwnerControlsAfterApproval,
                requires_join_confirmation: true,
                requires_explicit_commitment: false,
                requires_authorization: true,
            },
        }
    }
}

/// Owner-selected members that may use this node's Exit. The default allows all
/// active network members; an explicit selection requires at least one subject.
#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExitContributionMemberScope {
    #[default]
    AllMembers,
    Selected {
        #[serde(default)]
        roles: Vec<MembershipRole>,
        #[serde(default)]
        groups: Vec<GroupId>,
        #[serde(default)]
        nodes: Vec<NodeId>,
    },
}

impl ExitContributionMemberScope {
    pub fn validate(&self) -> DomainResult<()> {
        let Self::Selected {
            roles,
            groups,
            nodes,
        } = self
        else {
            return Ok(());
        };
        if roles.is_empty() && groups.is_empty() && nodes.is_empty() {
            return Err(DomainError::invalid_value(
                "selected exit member scope cannot be empty",
            ));
        }
        for (index, role) in roles.iter().enumerate() {
            if roles[..index].contains(role) {
                return Err(DomainError::invariant(
                    "exit member scope contains duplicate roles",
                ));
            }
        }
        for (index, group) in groups.iter().enumerate() {
            if groups[..index].contains(group) {
                return Err(DomainError::invariant(
                    "exit member scope contains duplicate groups",
                ));
            }
        }
        for (index, node) in nodes.iter().enumerate() {
            if nodes[..index].contains(node) {
                return Err(DomainError::invariant(
                    "exit member scope contains duplicate nodes",
                ));
            }
        }
        Ok(())
    }

    pub fn allows(&self, actor: &NodeId, role: MembershipRole, groups: &[GroupId]) -> bool {
        match self {
            Self::AllMembers => true,
            Self::Selected {
                roles: allowed_roles,
                groups: allowed_groups,
                nodes: allowed_nodes,
            } => {
                allowed_roles.contains(&role)
                    || allowed_nodes.contains(actor)
                    || groups.iter().any(|group| allowed_groups.contains(group))
            }
        }
    }
}

/// UTC half-open minute window. `weekdays` uses Monday=0 through Sunday=6.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ExitContributionTimeWindow {
    pub weekdays: Vec<u8>,
    pub start_minute_utc: u16,
    pub end_minute_utc: u16,
}

impl ExitContributionTimeWindow {
    pub fn validate(&self) -> DomainResult<()> {
        if self.weekdays.is_empty()
            || self.weekdays.iter().any(|day| *day > 6)
            || self
                .weekdays
                .windows(2)
                .any(|window| window[0] >= window[1])
        {
            return Err(DomainError::invalid_value(
                "exit time-window weekdays must be unique, sorted values from 0 to 6",
            ));
        }
        if self.start_minute_utc >= self.end_minute_utc || self.end_minute_utc > 24 * 60 {
            return Err(DomainError::invalid_value(
                "exit time-window minute range is invalid",
            ));
        }
        Ok(())
    }

    pub fn contains(&self, now_unix_ms: u64) -> bool {
        let minute_of_day = (now_unix_ms / 60_000) % (24 * 60);
        let day = (now_unix_ms / (24 * 60 * 60 * 1000) + 3) % 7;
        self.weekdays.contains(&(day as u8))
            && minute_of_day >= u64::from(self.start_minute_utc)
            && minute_of_day < u64::from(self.end_minute_utc)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitContributionQuotaPeriod {
    Daily,
    Weekly,
}

impl ExitContributionQuotaPeriod {
    pub fn period_start(self, now_unix_ms: u64) -> u64 {
        let day_ms = 24 * 60 * 60 * 1000;
        match self {
            Self::Daily => now_unix_ms / day_ms * day_ms,
            Self::Weekly => {
                let day = now_unix_ms / day_ms;
                let monday_based_day = (day + 3) % 7;
                (day - monday_based_day) * day_ms
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ExitContributionTrafficQuota {
    pub max_bytes: u64,
    pub period: ExitContributionQuotaPeriod,
}

/// Owner-authored restrictions beyond the global network allow-list and local
/// Exit veto. The network allow-list remains `NodeLocalAccessPolicy::network_scope`.
#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ExitContributionRestrictions {
    #[serde(default)]
    pub members: ExitContributionMemberScope,
    #[serde(default)]
    pub time_windows_utc: Vec<ExitContributionTimeWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bandwidth_kbps: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traffic_quota: Option<ExitContributionTrafficQuota>,
}

impl ExitContributionRestrictions {
    pub fn validate(&self) -> DomainResult<()> {
        self.members.validate()?;
        for window in &self.time_windows_utc {
            window.validate()?;
        }
        if self.max_bandwidth_kbps == Some(0) {
            return Err(DomainError::invalid_value(
                "exit bandwidth restriction must be positive",
            ));
        }
        if self
            .traffic_quota
            .as_ref()
            .is_some_and(|quota| quota.max_bytes == 0)
        {
            return Err(DomainError::invalid_value(
                "exit traffic quota must be positive",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ExitContributionUsage {
    /// Counter epoch. A value from an earlier quota period is treated as zero.
    pub period_started_at_unix_ms: u64,
    pub bytes_forwarded: u64,
    pub observed_bandwidth_kbps: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitContributionAccessReason {
    Allowed,
    NetworkNotAllowed,
    LocalVeto,
    MemberNotAllowed,
    OutsideAllowedTime,
    BandwidthLimitExceeded,
    TrafficQuotaExceeded,
    InvalidPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ExitContributionAccessDecision {
    pub allowed: bool,
    pub reason: ExitContributionAccessReason,
    pub evaluated_at_unix_ms: u64,
    pub network_id: NetworkId,
    pub local_policy_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traffic_quota_reset_at_unix_ms: Option<u64>,
}

/// Evaluate owner restrictions for one concrete Exit-serving request. This is
/// separate from network authorization: both sides must allow contribution.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_exit_contribution_access(
    policy: &super::access_control::NodeLocalAccessPolicy,
    actor: &NodeId,
    role: MembershipRole,
    groups: &[GroupId],
    network_id: &NetworkId,
    usage: ExitContributionUsage,
    now_unix_ms: u64,
) -> ExitContributionAccessDecision {
    let denied = |reason| ExitContributionAccessDecision {
        allowed: false,
        reason,
        evaluated_at_unix_ms: now_unix_ms,
        network_id: network_id.clone(),
        local_policy_revision: policy.revision,
        traffic_quota_reset_at_unix_ms: None,
    };
    // `policy.node_id` identifies the Exit-serving owner. `actor` is the
    // remote member whose request is being evaluated, so these identities are
    // intentionally different.
    if policy.validate_at(now_unix_ms).is_err() {
        return denied(ExitContributionAccessReason::InvalidPolicy);
    }
    if !policy.network_scope.allows(network_id) {
        return denied(ExitContributionAccessReason::NetworkNotAllowed);
    }
    let contribution_vetoed = policy.contribution_vetoes.iter().any(|veto| {
        veto.network_id == *network_id
            && veto
                .contributions
                .contains(&super::access_control::ContributionKind::Exit)
    });
    if contribution_vetoed {
        return denied(ExitContributionAccessReason::LocalVeto);
    }
    let Some(restrictions) = policy.exit_contribution.as_ref() else {
        return ExitContributionAccessDecision {
            allowed: true,
            reason: ExitContributionAccessReason::Allowed,
            evaluated_at_unix_ms: now_unix_ms,
            network_id: network_id.clone(),
            local_policy_revision: policy.revision,
            traffic_quota_reset_at_unix_ms: None,
        };
    };
    if restrictions.validate().is_err() {
        return denied(ExitContributionAccessReason::InvalidPolicy);
    }
    if !restrictions.members.allows(actor, role, groups) {
        return denied(ExitContributionAccessReason::MemberNotAllowed);
    }
    if !restrictions.time_windows_utc.is_empty()
        && !restrictions
            .time_windows_utc
            .iter()
            .any(|window| window.contains(now_unix_ms))
    {
        return denied(ExitContributionAccessReason::OutsideAllowedTime);
    }
    if restrictions
        .max_bandwidth_kbps
        .is_some_and(|limit| usage.observed_bandwidth_kbps > limit)
    {
        return denied(ExitContributionAccessReason::BandwidthLimitExceeded);
    }
    let mut quota_reset = None;
    if let Some(quota) = restrictions.traffic_quota.as_ref() {
        let period_start = quota.period.period_start(now_unix_ms);
        quota_reset = Some(period_start.saturating_add(match quota.period {
            ExitContributionQuotaPeriod::Daily => 24 * 60 * 60 * 1000,
            ExitContributionQuotaPeriod::Weekly => 7 * 24 * 60 * 60 * 1000,
        }));
        let used = if usage.period_started_at_unix_ms == period_start {
            usage.bytes_forwarded
        } else {
            0
        };
        if used >= quota.max_bytes {
            return denied(ExitContributionAccessReason::TrafficQuotaExceeded);
        }
    }
    ExitContributionAccessDecision {
        allowed: true,
        reason: ExitContributionAccessReason::Allowed,
        evaluated_at_unix_ms: now_unix_ms,
        network_id: network_id.clone(),
        local_policy_revision: policy.revision,
        traffic_quota_reset_at_unix_ms: quota_reset,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitAvailabilityState {
    Unknown,
    Available,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitAvailabilitySource {
    NotObserved,
    LocalRuntime,
    CoordinatorHealth,
    RemotePeerReport,
}

/// Availability projected beside a member's declared capability. Unknown or
/// stale evidence is never promoted to `available`.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ExitContributionAvailability {
    pub state: ExitAvailabilityState,
    pub source: ExitAvailabilitySource,
    #[serde(default)]
    pub observed_at_unix_ms: Option<u64>,
    #[serde(default)]
    pub declared_available: Option<bool>,
    #[serde(default)]
    pub reason_code: Option<String>,
}

impl Default for ExitContributionAvailability {
    fn default() -> Self {
        Self {
            state: ExitAvailabilityState::Unknown,
            source: ExitAvailabilitySource::NotObserved,
            observed_at_unix_ms: None,
            declared_available: None,
            reason_code: Some("exit.availability.not_observed".into()),
        }
    }
}

impl ExitContributionAvailability {
    pub fn observed(
        declared_available: bool,
        observed_available: bool,
        source: ExitAvailabilitySource,
        observed_at_unix_ms: u64,
    ) -> Self {
        Self::observed_with_declaration(
            Some(declared_available),
            observed_available,
            source,
            observed_at_unix_ms,
        )
    }

    /// Project an observation when the member's capability declaration may be
    /// absent. Missing declaration is not a mismatch; it stays explicit as
    /// `declared_available: None` while observed health remains visible.
    pub fn observed_with_declaration(
        declared_available: Option<bool>,
        observed_available: bool,
        source: ExitAvailabilitySource,
        observed_at_unix_ms: u64,
    ) -> Self {
        if source == ExitAvailabilitySource::NotObserved {
            return Self::declared_unknown(declared_available);
        }
        let (state, reason_code) = match (declared_available, observed_available) {
            (Some(true), true) | (None, true) => (ExitAvailabilityState::Available, None),
            (Some(false), true) => (
                ExitAvailabilityState::Degraded,
                Some("exit.availability.available_but_not_declared"),
            ),
            (Some(true), false) => (
                ExitAvailabilityState::Degraded,
                Some("exit.availability.declared_but_unavailable"),
            ),
            (Some(false), false) | (None, false) => (ExitAvailabilityState::Unavailable, None),
        };
        Self {
            state,
            source,
            observed_at_unix_ms: Some(observed_at_unix_ms),
            declared_available,
            reason_code: reason_code.map(str::to_owned),
        }
    }

    pub fn declared_unknown(declared_available: Option<bool>) -> Self {
        Self {
            declared_available,
            ..Self::default()
        }
    }

    pub fn stale(declared_available: bool, observed_at_unix_ms: u64) -> Self {
        Self::stale_with_declaration(Some(declared_available), observed_at_unix_ms)
    }

    /// Mark a known observation as too old to use as health evidence without
    /// inventing a capability declaration when one was not observed.
    pub fn stale_with_declaration(
        declared_available: Option<bool>,
        observed_at_unix_ms: u64,
    ) -> Self {
        Self {
            state: ExitAvailabilityState::Unknown,
            source: ExitAvailabilitySource::RemotePeerReport,
            observed_at_unix_ms: Some(observed_at_unix_ms),
            declared_available,
            reason_code: Some("exit.availability.stale".into()),
        }
    }

    pub fn is_fresh(&self, now_unix_ms: u64, max_age_ms: u64) -> bool {
        self.observed_at_unix_ms
            .is_some_and(|observed| now_unix_ms <= observed.saturating_add(max_age_ms))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitContributionRequestState {
    Pending,
    Approved,
    Rejected,
    Withdrawn,
    Expired,
    Superseded,
}

impl ExitContributionRequestState {
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Approved | Self::Rejected | Self::Withdrawn | Self::Expired | Self::Superseded
        )
    }

    pub const fn can_transition_to(self, target: Self) -> bool {
        matches!(
            (self, target),
            (Self::Pending, Self::Approved)
                | (Self::Pending, Self::Rejected)
                | (Self::Pending, Self::Withdrawn)
                | (Self::Pending, Self::Expired)
                | (Self::Pending, Self::Superseded)
                | (Self::Approved, Self::Superseded)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitContributionReviewAction {
    Approve,
    Reject,
}

/// Durable, CAS-protected request for network Exit contribution authority.
/// Approval never overrides the node owner's local veto.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ExitContributionRequest {
    pub schema_version: u16,
    pub request_id: ExitContributionRequestId,
    pub network_id: NetworkId,
    pub node_id: NodeId,
    pub policy_revision: u64,
    pub terms_hash: ContentHash,
    pub requested_bandwidth_kbps: Option<u64>,
    pub requested_traffic_quota_bytes: Option<u64>,
    pub state: ExitContributionRequestState,
    pub revision: u64,
    pub requested_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    #[serde(default)]
    pub decided_at_unix_ms: Option<u64>,
    #[serde(default)]
    pub reviewer_node_id: Option<NodeId>,
    #[serde(default)]
    pub decision_reason_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ExitContributionRequestChange {
    pub action: String,
    pub previous_state: ExitContributionRequestState,
    pub request: ExitContributionRequest,
}

/// Sanitized authorization evidence safe to disclose to every network member.
/// It deliberately omits request prose, member-supplied limits and reviewer
/// metadata while retaining everything needed to make a fail-closed decision.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ExitContributionAuthorizationSnapshot {
    pub request_id: ExitContributionRequestId,
    pub network_id: NetworkId,
    pub node_id: NodeId,
    pub policy_revision: u64,
    pub terms_hash: ContentHash,
    pub state: ExitContributionRequestState,
    pub revision: u64,
    pub expires_at_unix_ms: u64,
}

impl ExitContributionAuthorizationSnapshot {
    pub fn permits_contribution(
        &self,
        policy_revision: u64,
        terms_hash: &ContentHash,
        now_unix_ms: u64,
    ) -> bool {
        self.state == ExitContributionRequestState::Approved
            && self.policy_revision == policy_revision
            && &self.terms_hash == terms_hash
            && now_unix_ms < self.expires_at_unix_ms
    }
}

impl ExitContributionRequest {
    #[allow(clippy::too_many_arguments)]
    pub fn submit(
        request_id: ExitContributionRequestId,
        network_id: NetworkId,
        node_id: NodeId,
        policy_revision: u64,
        terms_hash: ContentHash,
        requested_bandwidth_kbps: Option<u64>,
        requested_traffic_quota_bytes: Option<u64>,
        now_unix_ms: u64,
    ) -> DomainResult<Self> {
        let request = Self {
            schema_version: EXIT_CONTRIBUTION_REQUEST_SCHEMA_VERSION,
            request_id,
            network_id,
            node_id,
            policy_revision,
            terms_hash,
            requested_bandwidth_kbps,
            requested_traffic_quota_bytes,
            state: ExitContributionRequestState::Pending,
            revision: 1,
            requested_at_unix_ms: now_unix_ms,
            updated_at_unix_ms: now_unix_ms,
            expires_at_unix_ms: now_unix_ms
                .saturating_add(EXIT_CONTRIBUTION_REQUEST_DEFAULT_EXPIRY_MS),
            decided_at_unix_ms: None,
            reviewer_node_id: None,
            decision_reason_code: None,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> DomainResult<()> {
        if self.schema_version != EXIT_CONTRIBUTION_REQUEST_SCHEMA_VERSION {
            return Err(DomainError::invalid_value(
                "unsupported exit contribution request schema version",
            ));
        }
        if self.policy_revision == 0 || self.revision == 0 {
            return Err(DomainError::invalid_value(
                "exit contribution request revisions must be positive",
            ));
        }
        if self.requested_at_unix_ms >= self.expires_at_unix_ms
            || self.updated_at_unix_ms < self.requested_at_unix_ms
        {
            return Err(DomainError::invalid_value(
                "exit contribution request timestamps are invalid",
            ));
        }
        if self.requested_bandwidth_kbps == Some(0) || self.requested_traffic_quota_bytes == Some(0)
        {
            return Err(DomainError::invalid_value(
                "exit contribution request limits must be positive",
            ));
        }
        if let Some(reason) = self.decision_reason_code.as_deref() {
            if reason.is_empty()
                || reason.len() > 96
                || !reason
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
            {
                return Err(DomainError::invalid_value(
                    "exit contribution decision reason code is invalid",
                ));
            }
        }
        let decided = self.decided_at_unix_ms.is_some()
            || self.reviewer_node_id.is_some()
            || self.decision_reason_code.is_some();
        match self.state {
            ExitContributionRequestState::Pending => {
                if decided {
                    return Err(DomainError::invariant(
                        "pending exit contribution request contains a decision",
                    ));
                }
            }
            ExitContributionRequestState::Approved | ExitContributionRequestState::Rejected => {
                if self.decided_at_unix_ms.is_none() || self.reviewer_node_id.is_none() {
                    return Err(DomainError::invariant(
                        "reviewed exit contribution request is missing reviewer metadata",
                    ));
                }
            }
            ExitContributionRequestState::Withdrawn
            | ExitContributionRequestState::Expired
            | ExitContributionRequestState::Superseded => {}
        }
        Ok(())
    }

    pub fn review(
        &self,
        reviewer_node_id: NodeId,
        expected_revision: u64,
        action: ExitContributionReviewAction,
        decision_reason_code: Option<String>,
        now_unix_ms: u64,
    ) -> DomainResult<ExitContributionRequestChange> {
        self.ensure_revision(expected_revision)?;
        if self.state != ExitContributionRequestState::Pending {
            return Err(DomainError::invalid_transition(
                "only pending exit contribution requests can be reviewed",
            ));
        }
        if now_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::new(
                "domain.exit_contribution_request_expired",
                "exit contribution request expired before review",
            ));
        }
        let target = match action {
            ExitContributionReviewAction::Approve => ExitContributionRequestState::Approved,
            ExitContributionReviewAction::Reject => ExitContributionRequestState::Rejected,
        };
        self.transition(
            target,
            now_unix_ms,
            Some(reviewer_node_id),
            decision_reason_code,
            match action {
                ExitContributionReviewAction::Approve => "exit_contribution.approved",
                ExitContributionReviewAction::Reject => "exit_contribution.rejected",
            },
        )
    }

    pub fn withdraw(
        &self,
        actor: &NodeId,
        expected_revision: u64,
        now_unix_ms: u64,
    ) -> DomainResult<ExitContributionRequestChange> {
        self.ensure_revision(expected_revision)?;
        if &self.node_id != actor {
            return Err(DomainError::new(
                "domain.exit_contribution_request_owner_mismatch",
                "only the requesting node can withdraw its exit contribution request",
            ));
        }
        if self.state != ExitContributionRequestState::Pending {
            return Err(DomainError::invalid_transition(
                "only pending exit contribution requests can be withdrawn",
            ));
        }
        self.transition(
            ExitContributionRequestState::Withdrawn,
            now_unix_ms,
            None,
            None,
            "exit_contribution.withdrawn",
        )
    }

    pub fn expire(&self, now_unix_ms: u64) -> DomainResult<Option<ExitContributionRequestChange>> {
        if self.state == ExitContributionRequestState::Pending
            && now_unix_ms >= self.expires_at_unix_ms
        {
            return self
                .transition(
                    ExitContributionRequestState::Expired,
                    now_unix_ms,
                    None,
                    Some("exit_contribution.expired".into()),
                    "exit_contribution.expired",
                )
                .map(Some);
        }
        Ok(None)
    }

    /// A policy revision or terms change invalidates pending and approved
    /// authority. The caller cannot carry an old approval into new terms.
    pub fn reconcile_policy(
        &self,
        policy_revision: u64,
        terms_hash: &ContentHash,
        now_unix_ms: u64,
    ) -> DomainResult<Option<ExitContributionRequestChange>> {
        if matches!(
            self.state,
            ExitContributionRequestState::Pending | ExitContributionRequestState::Approved
        ) && (self.policy_revision != policy_revision || &self.terms_hash != terms_hash)
        {
            return self
                .transition(
                    ExitContributionRequestState::Superseded,
                    now_unix_ms,
                    None,
                    Some("exit_contribution.policy_changed".into()),
                    "exit_contribution.superseded",
                )
                .map(Some);
        }
        Ok(None)
    }

    pub fn permits_contribution(&self, policy_revision: u64, terms_hash: &ContentHash) -> bool {
        self.state == ExitContributionRequestState::Approved
            && self.policy_revision == policy_revision
            && &self.terms_hash == terms_hash
    }

    pub fn authorization_snapshot(&self) -> ExitContributionAuthorizationSnapshot {
        ExitContributionAuthorizationSnapshot {
            request_id: self.request_id.clone(),
            network_id: self.network_id.clone(),
            node_id: self.node_id.clone(),
            policy_revision: self.policy_revision,
            terms_hash: self.terms_hash.clone(),
            state: self.state,
            revision: self.revision,
            expires_at_unix_ms: self.expires_at_unix_ms,
        }
    }

    fn ensure_revision(&self, expected_revision: u64) -> DomainResult<()> {
        if self.revision != expected_revision {
            return Err(DomainError::new(
                "domain.exit_contribution_request_revision_conflict",
                format!(
                    "exit contribution request revision conflict: expected {expected_revision}, actual {}",
                    self.revision
                ),
            ));
        }
        Ok(())
    }

    fn transition(
        &self,
        target: ExitContributionRequestState,
        now_unix_ms: u64,
        reviewer_node_id: Option<NodeId>,
        decision_reason_code: Option<String>,
        action: &str,
    ) -> DomainResult<ExitContributionRequestChange> {
        if !self.state.can_transition_to(target) {
            return Err(DomainError::invalid_transition(format!(
                "exit contribution request cannot transition from {} to {}",
                self.state.as_str(),
                target.as_str()
            )));
        }
        if now_unix_ms < self.updated_at_unix_ms {
            return Err(DomainError::invariant(
                "exit contribution request time moved backwards",
            ));
        }
        let mut request = self.clone();
        request.state = target;
        request.revision = request.revision.saturating_add(1);
        request.updated_at_unix_ms = now_unix_ms;
        if matches!(
            target,
            ExitContributionRequestState::Approved | ExitContributionRequestState::Rejected
        ) {
            request.decided_at_unix_ms = Some(now_unix_ms);
            request.reviewer_node_id = reviewer_node_id;
            request.decision_reason_code = decision_reason_code;
        } else if target == ExitContributionRequestState::Superseded {
            request.decision_reason_code = decision_reason_code;
        }
        request.validate()?;
        Ok(ExitContributionRequestChange {
            action: action.into(),
            previous_state: self.state,
            request,
        })
    }
}

impl ExitContributionRequestState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Withdrawn => "withdrawn",
            Self::Expired => "expired",
            Self::Superseded => "superseded",
        }
    }
}

/// Final contribution decision after network mode, request state and local
/// owner veto are all combined. Administrators cannot change the local veto.
pub struct ExitContributionAuthorizationInput<'a> {
    pub mode: ExitContributionMode,
    pub policy_revision: u64,
    pub terms_hash: &'a ContentHash,
    pub request: Option<&'a ExitContributionAuthorizationSnapshot>,
    pub local_veto: bool,
    pub local_access_allowed: bool,
    pub now_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitContributionAuthorizationReason {
    Authorized,
    NetworkDisabled,
    AuthorizationRequired,
    AuthorizationPending,
    AuthorizationRejected,
    AuthorizationExpired,
    AuthorizationSuperseded,
    LocalVeto,
    LocalRestriction,
}

pub fn authorize_exit_contribution(
    input: ExitContributionAuthorizationInput<'_>,
) -> ExitContributionAuthorizationReason {
    if input.local_veto {
        return ExitContributionAuthorizationReason::LocalVeto;
    }
    if !input.local_access_allowed {
        return ExitContributionAuthorizationReason::LocalRestriction;
    }
    match input.mode {
        ExitContributionMode::Disabled => ExitContributionAuthorizationReason::NetworkDisabled,
        ExitContributionMode::Autonomous | ExitContributionMode::RequiredOnJoin => {
            ExitContributionAuthorizationReason::Authorized
        }
        ExitContributionMode::ApprovalRequired => {
            let Some(request) = input.request else {
                return ExitContributionAuthorizationReason::AuthorizationRequired;
            };
            if request.permits_contribution(
                input.policy_revision,
                input.terms_hash,
                input.now_unix_ms,
            ) {
                ExitContributionAuthorizationReason::Authorized
            } else if request.state == ExitContributionRequestState::Pending {
                if input.now_unix_ms >= request.expires_at_unix_ms {
                    ExitContributionAuthorizationReason::AuthorizationExpired
                } else {
                    ExitContributionAuthorizationReason::AuthorizationPending
                }
            } else if request.state == ExitContributionRequestState::Rejected {
                ExitContributionAuthorizationReason::AuthorizationRejected
            } else if request.state == ExitContributionRequestState::Expired {
                ExitContributionAuthorizationReason::AuthorizationExpired
            } else {
                ExitContributionAuthorizationReason::AuthorizationSuperseded
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        ContributionKind, NodeContributionVeto, NodeLocalAccessPolicy, NodeNetworkScope,
        SecurityRuleId,
    };

    fn network() -> NetworkId {
        NetworkId::from_public_key(b"exit-governance-network")
    }

    fn node(seed: &[u8]) -> NodeId {
        NodeId::from_public_key(seed)
    }

    fn terms_hash() -> ContentHash {
        ContentHash::from_bytes(b"exit-terms")
    }

    fn policy(restrictions: ExitContributionRestrictions) -> NodeLocalAccessPolicy {
        NodeLocalAccessPolicy {
            node_id: node(b"owner"),
            revision: 1,
            issued_at_unix_ms: 1,
            expires_at_unix_ms: u64::MAX,
            network_scope: NodeNetworkScope::All,
            contribution_vetoes: Vec::new(),
            exit_contribution: Some(restrictions),
            relay_limits: Default::default(),
            rules: Vec::new(),
        }
    }

    #[test]
    fn disclosure_covers_every_mode_without_implying_local_enablement() {
        for mode in [
            ExitContributionMode::Autonomous,
            ExitContributionMode::RequiredOnJoin,
            ExitContributionMode::Disabled,
            ExitContributionMode::ApprovalRequired,
        ] {
            let disclosure = ExitContributionDisclosure::for_mode(mode);
            assert!(disclosure.requires_join_confirmation);
            assert_eq!(
                disclosure.requires_explicit_commitment,
                mode == ExitContributionMode::RequiredOnJoin
            );
            assert_eq!(
                disclosure.requires_authorization,
                mode == ExitContributionMode::ApprovalRequired
            );
        }
    }

    #[test]
    fn owner_restrictions_enforce_member_time_bandwidth_and_quota() {
        let network_id = network();
        let actor = node(b"remote-member");
        let group = GroupId::from_public_key(b"selected-group");
        let now = 1_700_006_400_000; // 2023-11-15 00:00 UTC, Wednesday.
        let restrictions = ExitContributionRestrictions {
            members: ExitContributionMemberScope::Selected {
                roles: Vec::new(),
                groups: vec![group.clone()],
                nodes: Vec::new(),
            },
            time_windows_utc: vec![ExitContributionTimeWindow {
                weekdays: vec![2],
                start_minute_utc: 0,
                end_minute_utc: 60,
            }],
            max_bandwidth_kbps: Some(1_000),
            traffic_quota: Some(ExitContributionTrafficQuota {
                max_bytes: 10,
                period: ExitContributionQuotaPeriod::Daily,
            }),
        };
        let policy = policy(restrictions);
        assert_ne!(policy.node_id, actor);
        let allowed = evaluate_exit_contribution_access(
            &policy,
            &actor,
            MembershipRole::Owner,
            std::slice::from_ref(&group),
            &network_id,
            ExitContributionUsage::default(),
            now,
        );
        assert!(allowed.allowed);
        let wrong_group = evaluate_exit_contribution_access(
            &policy,
            &actor,
            MembershipRole::Owner,
            &[],
            &network_id,
            ExitContributionUsage::default(),
            now,
        );
        assert_eq!(
            wrong_group.reason,
            ExitContributionAccessReason::MemberNotAllowed
        );
        let outside = evaluate_exit_contribution_access(
            &policy,
            &actor,
            MembershipRole::Owner,
            std::slice::from_ref(&group),
            &network_id,
            ExitContributionUsage::default(),
            now + 60 * 60 * 1000,
        );
        assert_eq!(
            outside.reason,
            ExitContributionAccessReason::OutsideAllowedTime
        );
        let over_quota = evaluate_exit_contribution_access(
            &policy,
            &actor,
            MembershipRole::Owner,
            &[group],
            &network_id,
            ExitContributionUsage {
                period_started_at_unix_ms: now,
                bytes_forwarded: 10,
                observed_bandwidth_kbps: 0,
            },
            now,
        );
        assert_eq!(
            over_quota.reason,
            ExitContributionAccessReason::TrafficQuotaExceeded
        );
    }

    #[test]
    fn local_veto_wins_even_with_an_approved_network_request() {
        let network_id = network();
        let actor = node(b"remote-member");
        let request = ExitContributionRequest::submit(
            ExitContributionRequestId::from_public_key(b"request"),
            network_id.clone(),
            actor.clone(),
            4,
            terms_hash(),
            None,
            None,
            10,
        )
        .unwrap()
        .review(
            node(b"admin"),
            1,
            ExitContributionReviewAction::Approve,
            None,
            11,
        )
        .unwrap()
        .request;
        let mut local_policy = policy(ExitContributionRestrictions::default());
        local_policy.contribution_vetoes = vec![NodeContributionVeto {
            veto_id: SecurityRuleId::from_public_key(b"veto"),
            network_id,
            contributions: vec![ContributionKind::Exit],
        }];
        let decision = evaluate_exit_contribution_access(
            &local_policy,
            &actor,
            MembershipRole::Owner,
            &[],
            &request.network_id,
            ExitContributionUsage::default(),
            12,
        );
        assert_eq!(decision.reason, ExitContributionAccessReason::LocalVeto);
        let snapshot = request.authorization_snapshot();
        let reason = authorize_exit_contribution(ExitContributionAuthorizationInput {
            mode: ExitContributionMode::ApprovalRequired,
            policy_revision: 4,
            terms_hash: &terms_hash(),
            request: Some(&snapshot),
            local_veto: true,
            local_access_allowed: false,
            now_unix_ms: 12,
        });
        assert_eq!(reason, ExitContributionAuthorizationReason::LocalVeto);
    }

    #[test]
    fn request_lifecycle_is_cas_protected_and_policy_changes_supersede_approval() {
        let actor = node(b"requester");
        let request = ExitContributionRequest::submit(
            ExitContributionRequestId::from_public_key(b"request"),
            network(),
            actor.clone(),
            1,
            terms_hash(),
            Some(20_000),
            None,
            100,
        )
        .unwrap();
        assert!(request
            .review(
                node(b"admin"),
                99,
                ExitContributionReviewAction::Approve,
                None,
                101,
            )
            .is_err());
        let approved = request
            .review(
                node(b"admin"),
                1,
                ExitContributionReviewAction::Approve,
                Some("exit.approved.standard".into()),
                101,
            )
            .unwrap()
            .request;
        assert!(approved.permits_contribution(1, &terms_hash()));
        let superseded = approved
            .reconcile_policy(2, &ContentHash::from_bytes(b"new"), 102)
            .unwrap()
            .unwrap()
            .request;
        assert_eq!(superseded.state, ExitContributionRequestState::Superseded);
        assert!(!superseded.permits_contribution(2, &ContentHash::from_bytes(b"new")));
        assert!(request.withdraw(&actor, 1, 103).is_ok());
    }

    #[test]
    fn availability_marks_declared_observation_mismatch_as_degraded() {
        let declared_only = ExitContributionAvailability::declared_unknown(Some(true));
        assert_eq!(declared_only.state, ExitAvailabilityState::Unknown);
        let mismatch = ExitContributionAvailability::observed(
            true,
            false,
            ExitAvailabilitySource::LocalRuntime,
            10,
        );
        assert_eq!(mismatch.state, ExitAvailabilityState::Degraded);
        assert_eq!(
            mismatch.reason_code.as_deref(),
            Some("exit.availability.declared_but_unavailable")
        );
        let healthy = ExitContributionAvailability::observed(
            true,
            true,
            ExitAvailabilitySource::LocalRuntime,
            10,
        );
        assert_eq!(healthy.state, ExitAvailabilityState::Available);
        let stale = ExitContributionAvailability::stale(true, 10);
        assert_eq!(stale.state, ExitAvailabilityState::Unknown);
        assert_eq!(
            stale.reason_code.as_deref(),
            Some("exit.availability.stale")
        );
        let undeclared_stale = ExitContributionAvailability::stale_with_declaration(None, 10);
        assert_eq!(undeclared_stale.state, ExitAvailabilityState::Unknown);
        assert_eq!(undeclared_stale.declared_available, None);
    }

    #[test]
    fn authorization_snapshot_excludes_request_prose_and_tracks_approval_revision() {
        let actor = node(b"requester");
        let request = ExitContributionRequest::submit(
            ExitContributionRequestId::from_public_key(b"snapshot-request"),
            network(),
            actor.clone(),
            4,
            terms_hash(),
            Some(123_456),
            Some(987_654),
            100,
        )
        .unwrap();
        let snapshot = request.authorization_snapshot();
        assert_eq!(snapshot.node_id, actor);
        assert_eq!(snapshot.state, ExitContributionRequestState::Pending);
        assert_eq!(snapshot.revision, 1);
        let encoded = serde_json::to_value(&snapshot).unwrap();
        assert!(encoded.get("requested_bandwidth_kbps").is_none());
        assert!(encoded.get("requested_traffic_quota_bytes").is_none());
        assert!(encoded.get("reviewer_node_id").is_none());
        assert!(encoded.get("decision_reason_code").is_none());

        let approved = request
            .review(
                node(b"admin"),
                1,
                ExitContributionReviewAction::Approve,
                Some("exit.approved".into()),
                101,
            )
            .unwrap()
            .request;
        let approved_snapshot = approved.authorization_snapshot();
        assert_eq!(approved_snapshot.revision, 2);
        assert!(approved_snapshot.permits_contribution(4, &terms_hash(), 102));
        assert!(!approved_snapshot.permits_contribution(5, &terms_hash(), 102));
        assert!(!approved_snapshot.permits_contribution(4, &ContentHash::from_bytes(b"new"), 102,));
        assert!(!approved_snapshot.permits_contribution(
            4,
            &terms_hash(),
            approved_snapshot.expires_at_unix_ms,
        ));
    }
}
