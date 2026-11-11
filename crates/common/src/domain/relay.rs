//! Relay governance, quota, health, selection and disclosure contracts.
//!
//! Relay is an independent capability. A Relay endpoint is usable only after
//! both the network and the providing node authorize it. Limits are always
//! intersected, never replaced, and a Relay never gains application plaintext
//! or identity secrets merely by forwarding encrypted packets.

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::access_control::{
    evaluate_effective_access, EffectiveAccess, EffectiveAccessReason, NetworkAcl,
    NodeLocalAccessPolicy,
};
use super::ids::{GroupId, NetworkId, NodeId};
use super::permissions::{
    NetworkRole, PermissionIntent, PermissionRequest, PermissionRule, PolicyAction, ResourceKind,
    ResourceRef,
};
use super::{DomainError, DomainResult};

/// Hard limit set for one Relay direction. When a network policy and a node
/// policy both define a value, [`RelayLimitPolicy::intersection`] keeps the
/// stricter one.
#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct RelayLimitPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_connections: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_streams: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bandwidth_kbps: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cpu_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_memory_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_session_duration_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_reset_at_unix_ms: Option<u64>,
}

impl RelayLimitPolicy {
    pub fn validate(&self) -> DomainResult<()> {
        let positive = [
            self.max_connections.map(u64::from),
            self.max_concurrent_streams.map(u64::from),
            self.max_bandwidth_kbps,
            self.max_bytes,
            self.max_memory_bytes,
            self.max_session_duration_secs,
        ];
        if positive.into_iter().flatten().any(|value| value == 0) {
            return Err(DomainError::invalid_value(
                "Relay limits must be positive when present",
            ));
        }
        if self.max_cpu_percent.is_some_and(|value| !(1..=100).contains(&value)) {
            return Err(DomainError::invalid_value(
                "Relay CPU percentage must be between 1 and 100",
            ));
        }
        Ok(())
    }

    /// Combine two independently authored policy layers. Every configured
    /// bound becomes the smaller value; quotas reset at the earliest configured
    /// deadline so a broader policy cannot extend a stricter one.
    pub fn intersection(&self, other: &Self) -> DomainResult<Self> {
        self.validate()?;
        other.validate()?;
        Ok(Self {
            max_connections: min_option(self.max_connections, other.max_connections),
            max_concurrent_streams: min_option(
                self.max_concurrent_streams,
                other.max_concurrent_streams,
            ),
            max_bandwidth_kbps: min_option(self.max_bandwidth_kbps, other.max_bandwidth_kbps),
            max_bytes: min_option(self.max_bytes, other.max_bytes),
            max_cpu_percent: min_option(self.max_cpu_percent, other.max_cpu_percent),
            max_memory_bytes: min_option(self.max_memory_bytes, other.max_memory_bytes),
            max_session_duration_secs: min_option(
                self.max_session_duration_secs,
                other.max_session_duration_secs,
            ),
            quota_reset_at_unix_ms: min_option(
                self.quota_reset_at_unix_ms,
                other.quota_reset_at_unix_ms,
            ),
        })
    }

    pub fn has_quota_reset_due(&self, now_unix_ms: u64) -> bool {
        self.quota_reset_at_unix_ms
            .is_some_and(|reset| now_unix_ms >= reset)
    }
}

fn min_option<T: Ord>(left: Option<T>, right: Option<T>) -> Option<T> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

/// Measured Relay work. This is a projection, not an accounting journal; the
/// authoritative audit remains the append-only audit chain.
#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct RelayUsageSnapshot {
    pub active_connections: u32,
    pub active_streams: u32,
    pub forwarded_packets: u64,
    pub forwarded_bytes: u64,
    pub rejected_packets: u64,
    pub rejected_connections: u64,
    pub active_sessions: u32,
    pub oldest_session_age_secs: u64,
    pub observed_cpu_percent: Option<u8>,
    pub observed_memory_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_started_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_reset_at_unix_ms: Option<u64>,
}

impl RelayUsageSnapshot {
    pub fn quota_limited(&self, limits: &RelayLimitPolicy) -> bool {
        limits
            .max_connections
            .is_some_and(|limit| self.active_connections >= limit)
            || limits
                .max_concurrent_streams
                .is_some_and(|limit| self.active_streams >= limit)
            || limits
                .max_bytes
                .is_some_and(|limit| self.forwarded_bytes >= limit)
            || limits
                .max_cpu_percent
                .zip(self.observed_cpu_percent)
                .is_some_and(|(limit, observed)| observed >= limit)
            || limits
                .max_memory_bytes
                .zip(self.observed_memory_bytes)
                .is_some_and(|(limit, observed)| observed >= limit)
    }

    pub fn reset_quota_counters(&mut self) {
        self.forwarded_packets = 0;
        self.forwarded_bytes = 0;
        self.rejected_packets = 0;
        self.rejected_connections = 0;
        self.quota_started_at_unix_ms = self.quota_reset_at_unix_ms;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayLifecycleState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayHealthStatus {
    Unknown,
    Healthy,
    Degraded,
    Unavailable,
    QuotaExhausted,
}

impl RelayHealthStatus {
    pub const fn is_usable(self) -> bool {
        matches!(self, Self::Healthy | Self::Degraded)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayHealthIssue {
    ListenerUnavailable,
    NetworkUnreachable,
    ProtocolError,
    QuotaExhausted,
    ResourcePressure,
    ForwardingStalled,
    AuthorizationExpired,
}

/// Health is derived from observed state. It cannot be set to healthy by a UI
/// merely because an owner intent or capability declaration is present.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct RelayHealthSnapshot {
    pub status: RelayHealthStatus,
    pub lifecycle: RelayLifecycleState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen_addr: Option<SocketAddr>,
    pub listener_bound: bool,
    pub network_reachable: bool,
    pub forwarding_observed: bool,
    pub authorization_valid: bool,
    pub quota_limited: bool,
    pub limits: RelayLimitPolicy,
    pub usage: RelayUsageSnapshot,
    pub issues: Vec<RelayHealthIssue>,
    pub observed_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
}

impl RelayHealthSnapshot {
    /// Build a health snapshot and derive the status from observable evidence.
    pub fn observe(
        lifecycle: RelayLifecycleState,
        listen_addr: Option<SocketAddr>,
        listener_bound: bool,
        network_reachable: bool,
        forwarding_observed: bool,
        authorization_valid: bool,
        limits: RelayLimitPolicy,
        usage: RelayUsageSnapshot,
        observed_at_unix_ms: u64,
        reason_code: Option<String>,
    ) -> Self {
        let mut issues = Vec::new();
        if !listener_bound || listen_addr.is_none() {
            issues.push(RelayHealthIssue::ListenerUnavailable);
        }
        if !network_reachable {
            issues.push(RelayHealthIssue::NetworkUnreachable);
        }
        if !authorization_valid {
            issues.push(RelayHealthIssue::AuthorizationExpired);
        }
        let quota_limited = usage.quota_limited(&limits);
        if quota_limited {
            issues.push(RelayHealthIssue::QuotaExhausted);
        }
        if limits.max_connections.zip(usage.active_connections).is_some_and(
            |(limit, active)| active.saturating_mul(10) >= limit.saturating_mul(9),
        ) || limits
            .max_concurrent_streams
            .zip(usage.active_streams)
            .is_some_and(|(limit, active)| active.saturating_mul(10) >= limit.saturating_mul(9))
        {
            issues.push(RelayHealthIssue::ResourcePressure);
        }

        let status = match lifecycle {
            RelayLifecycleState::Running => {
                if !listener_bound || !network_reachable || !authorization_valid {
                    RelayHealthStatus::Unavailable
                } else if quota_limited {
                    RelayHealthStatus::QuotaExhausted
                } else if !issues.is_empty() {
                    RelayHealthStatus::Degraded
                } else {
                    RelayHealthStatus::Healthy
                }
            }
            RelayLifecycleState::Starting | RelayLifecycleState::Stopping => {
                RelayHealthStatus::Degraded
            }
            RelayLifecycleState::Stopped
            | RelayLifecycleState::Failed
            | RelayLifecycleState::Unsupported => RelayHealthStatus::Unavailable,
        };

        Self {
            status,
            lifecycle,
            listen_addr: listener_bound.then_some(listen_addr).flatten(),
            listener_bound,
            network_reachable,
            forwarding_observed,
            authorization_valid,
            quota_limited,
            limits,
            usage,
            issues,
            observed_at_unix_ms,
            reason_code,
        }
    }

    pub fn summary_code(&self) -> &'static str {
        match self.status {
            RelayHealthStatus::Unknown => "relay.health.unknown",
            RelayHealthStatus::Healthy => "relay.health.healthy",
            RelayHealthStatus::Degraded => "relay.health.degraded",
            RelayHealthStatus::Unavailable => "relay.health.unavailable",
            RelayHealthStatus::QuotaExhausted => "relay.health.quota_exhausted",
        }
    }
}

/// One member's trusted policy context as resolved by the control plane.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct RelayMemberPolicyContext {
    pub role: NetworkRole,
    #[serde(default)]
    pub groups: BTreeSet<GroupId>,
    pub membership_active: bool,
    pub recent_reauthentication: bool,
    #[serde(default)]
    pub local_rules: Vec<PermissionRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_access_policy: Option<NodeLocalAccessPolicy>,
}

impl RelayMemberPolicyContext {
    pub fn validate_for(&self, node_id: &NodeId, now_unix_ms: u64) -> DomainResult<()> {
        for rule in &self.local_rules {
            rule.validate()?;
        }
        if let Some(policy) = &self.local_access_policy {
            if &policy.node_id != node_id {
                return Err(DomainError::invariant(
                    "Relay member local policy belongs to another node",
                ));
            }
            policy.validate_at(now_unix_ms)?;
        }
        Ok(())
    }
}

/// The complete authorization input needed by a Relay endpoint to decide
/// whether a member may use it. The network ACL and each member's local policy
/// are both evaluated; a denial in either layer wins.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct RelayAccessPolicy {
    pub network_id: NetworkId,
    pub provider_node_id: NodeId,
    pub revision: u64,
    pub updated_at_unix_ms: u64,
    #[serde(default)]
    pub network_rules: Vec<PermissionRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_acl: Option<NetworkAcl>,
    #[serde(default)]
    pub provider_local_rules: Vec<PermissionRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_local_policy: Option<NodeLocalAccessPolicy>,
    #[serde(default)]
    pub members: BTreeMap<NodeId, RelayMemberPolicyContext>,
    pub limits: RelayLimitPolicy,
}

/// Both sides of a Relay authorization decision. Keeping both effective
/// projections lets the UI explain whether the network or the providing node
/// refused service without reimplementing precedence.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct RelayAuthorizationDecision {
    pub allowed: bool,
    pub provider: EffectiveAccess,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumer: Option<EffectiveAccess>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_reason: Option<EffectiveAccessReason>,
}

impl RelayAccessPolicy {
    pub fn validate_at(&self, now_unix_ms: u64) -> DomainResult<()> {
        if self.revision == 0 {
            return Err(DomainError::invalid_value(
                "Relay access policy revision must be positive",
            ));
        }
        self.limits.validate()?;
        for rule in self.network_rules.iter().chain(&self.provider_local_rules) {
            rule.validate()?;
        }
        if let Some(acl) = &self.network_acl {
            if acl.network_id != self.network_id {
                return Err(DomainError::invariant(
                    "Relay network ACL belongs to another network",
                ));
            }
            acl.validate_at(now_unix_ms)?;
        }
        if let Some(policy) = &self.provider_local_policy {
            if policy.node_id != self.provider_node_id {
                return Err(DomainError::invariant(
                    "Relay provider local policy belongs to another node",
                ));
            }
            policy.validate_at(now_unix_ms)?;
        }
        for (node_id, context) in &self.members {
            context.validate_for(node_id, now_unix_ms)?;
        }
        Ok(())
    }

    fn request() -> PermissionRequest {
        PermissionRequest {
            action: PolicyAction::Use,
            resource: ResourceRef {
                kind: ResourceKind::Capability,
                // Replaced by the evaluator with the concrete network.
                network_id: NetworkId::from_public_key(b"relay-policy-placeholder"),
                object_id: Some("relay".into()),
            },
            intent: PermissionIntent::Mutating,
        }
    }

    fn provider_decision(&self, now_unix_ms: u64) -> EffectiveAccess {
        let mut request = Self::request();
        request.resource.network_id = self.network_id.clone();
        let context = super::permissions::AccessContext {
            network_id: self.network_id.clone(),
            actor: self.provider_node_id.clone(),
            role: NetworkRole::Owner,
            groups: BTreeSet::new(),
            membership_active: true,
            recent_reauthentication: true,
            emergency_coordinator: false,
        };
        evaluate_effective_access(
            &self.provider_local_rules,
            &self.network_rules,
            self.network_acl.as_ref(),
            self.provider_local_policy.as_ref(),
            &request,
            &context,
            now_unix_ms,
        )
    }

    /// Authorize one member for Relay use. The provider side is checked first;
    /// an expired or denied provider policy fails closed even when the network
    /// would allow the member.
    pub fn authorize_member(&self, node_id: &NodeId, now_unix_ms: u64) -> RelayAuthorizationDecision {
        let provider = self.provider_decision(now_unix_ms);
        let Some(member) = self.members.get(node_id) else {
            return RelayAuthorizationDecision {
                allowed: false,
                final_reason: Some(EffectiveAccessReason::DefaultDenied),
                provider,
                consumer: None,
            };
        };

        let mut request = Self::request();
        request.resource.network_id = self.network_id.clone();
        let context = super::permissions::AccessContext {
            network_id: self.network_id.clone(),
            actor: node_id.clone(),
            role: member.role,
            groups: member.groups.clone(),
            membership_active: member.membership_active,
            recent_reauthentication: member.recent_reauthentication,
            emergency_coordinator: member.role == NetworkRole::TemporaryCoordinator,
        };
        let consumer = evaluate_effective_access(
            &member.local_rules,
            &self.network_rules,
            self.network_acl.as_ref(),
            member.local_access_policy.as_ref(),
            &request,
            &context,
            now_unix_ms,
        );
        let allowed = provider.allowed && consumer.allowed;
        let final_reason = if allowed {
            None
        } else if !provider.allowed {
            Some(provider.reason)
        } else {
            Some(consumer.reason)
        };
        RelayAuthorizationDecision {
            allowed,
            provider,
            consumer: Some(consumer),
            final_reason,
        }
    }

    /// Recompute all member decisions after a live policy change. Existing
    /// sessions can then follow an explicit continue/migrate/terminate policy.
    pub fn authorization_snapshot(
        &self,
        now_unix_ms: u64,
    ) -> DomainResult<BTreeMap<NodeId, RelayAuthorizationDecision>> {
        self.validate_at(now_unix_ms)?;
        Ok(self
            .members
            .keys()
            .map(|node_id| (node_id.clone(), self.authorize_member(node_id, now_unix_ms)))
            .collect())
    }
}

/// How existing sessions react when Relay authorization changes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayExistingSessionPolicy {
    /// Keep the already-established encrypted QUIC session until it closes.
    Continue,
    /// Move the member to another authorized Relay without dropping its P2P
    /// overlay session.
    Migrate,
    /// Close the relayed session promptly and fail closed.
    Terminate,
    #[default]
    Reauthorize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayAuthorizationDenial {
    Unauthorized,
    StaleHealth,
    Unhealthy,
    QuotaExhausted,
    DuplicateCandidate,
    NoCandidate,
    LoopDetected,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct RelayCandidate {
    /// Stable, non-secret Relay identity used in activity and diagnostics.
    pub relay_id: String,
    pub authorized: bool,
    pub health: RelayHealthStatus,
    pub observed_at_unix_ms: u64,
    pub load_percent: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u32>,
    pub consecutive_failures: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct RelaySelectionPolicy {
    pub max_attempts: u32,
    pub max_health_age_ms: u64,
    pub max_consecutive_failures: u32,
    pub allow_fallback: bool,
    pub switch_cooldown_ms: u64,
}

impl Default for RelaySelectionPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            max_health_age_ms: 30_000,
            max_consecutive_failures: 3,
            allow_fallback: true,
            switch_cooldown_ms: 5_000,
        }
    }
}

impl RelaySelectionPolicy {
    pub fn validate(&self) -> DomainResult<()> {
        if self.max_attempts == 0 || self.max_health_age_ms == 0 {
            return Err(DomainError::invalid_value(
                "Relay selection attempt and health-age limits must be positive",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct RelaySelectionOutcome {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<RelayCandidate>,
    pub attempts_allowed: u32,
    pub denied: Vec<RelaySelectionDenial>,
    pub fail_closed: bool,
    pub reason: String,
}

impl RelaySelectionOutcome {
    pub fn is_usable(&self) -> bool {
        self.selected.is_some() && !self.fail_closed
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct RelaySelectionDenial {
    pub relay_id: String,
    pub reason: RelayAuthorizationDenial,
}

/// Deterministically choose a Relay while excluding unauthorized, stale,
/// unhealthy and repeatedly failing candidates. The caller may perform at most
/// `attempts_allowed` connection attempts and must fail closed rather than
/// scanning an unbounded candidate graph.
pub fn select_relay_candidate(
    candidates: &[RelayCandidate],
    policy: &RelaySelectionPolicy,
    now_unix_ms: u64,
) -> DomainResult<RelaySelectionOutcome> {
    policy.validate()?;
    let mut seen = BTreeSet::new();
    let mut denied = Vec::new();
    let mut eligible = Vec::new();
    for candidate in candidates {
        if candidate.relay_id.is_empty() {
            return Err(DomainError::invalid_value(
                "Relay candidate identity cannot be empty",
            ));
        }
        if !seen.insert(candidate.relay_id.clone()) {
            denied.push(RelaySelectionDenial {
                relay_id: candidate.relay_id.clone(),
                reason: RelayAuthorizationDenial::DuplicateCandidate,
            });
            continue;
        }
        let age = now_unix_ms.saturating_sub(candidate.observed_at_unix_ms);
        let reason = if !candidate.authorized {
            Some(RelayAuthorizationDenial::Unauthorized)
        } else if candidate.observed_at_unix_ms > now_unix_ms
            || age > policy.max_health_age_ms
        {
            Some(RelayAuthorizationDenial::StaleHealth)
        } else if !candidate.health.is_usable()
            || candidate.consecutive_failures > policy.max_consecutive_failures
        {
            Some(RelayAuthorizationDenial::Unhealthy)
        } else {
            None
        };
        if let Some(reason) = reason {
            denied.push(RelaySelectionDenial {
                relay_id: candidate.relay_id.clone(),
                reason,
            });
        } else {
            eligible.push(candidate.clone());
        }
    }

    eligible.sort_by(|left, right| {
        left.latency_ms
            .unwrap_or(u32::MAX)
            .cmp(&right.latency_ms.unwrap_or(u32::MAX))
            .then_with(|| left.load_percent.cmp(&right.load_percent))
            .then_with(|| left.relay_id.cmp(&right.relay_id))
    });

    if eligible.is_empty() {
        return Ok(RelaySelectionOutcome {
            selected: None,
            attempts_allowed: 0,
            denied,
            fail_closed: true,
            reason: "relay.selection.no_eligible_candidate".into(),
        });
    }
    if !policy.allow_fallback && eligible.len() > 1 {
        // A single explicit candidate is still safe. Multiple candidates require
        // fallback to be enabled explicitly, otherwise selection is ambiguous.
        return Ok(RelaySelectionOutcome {
            selected: None,
            attempts_allowed: 0,
            denied,
            fail_closed: true,
            reason: "relay.selection.fallback_disabled".into(),
        });
    }

    let attempts_allowed = policy.max_attempts.min(eligible.len() as u32);
    Ok(RelaySelectionOutcome {
        selected: eligible.first().cloned(),
        attempts_allowed,
        denied,
        fail_closed: false,
        reason: "relay.selection.eligible_candidate".into(),
    })
}

/// Metadata fields that a Relay operator can observe. Encryption protects the
/// payload but cannot hide packet timing or endpoint relationships.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayMetadataField {
    SourceVirtualIp,
    DestinationVirtualIp,
    IdentityRelationship,
    PacketTiming,
    PacketSize,
    TrafficVolume,
    ApplicationPayload,
    PrivateKey,
    SessionKey,
    NetworkName,
    MemberDisplayName,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayMetadataVisibility {
    #[default]
    Visible,
    NotVisible,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct RelayMetadataBoundary {
    pub fields: BTreeMap<RelayMetadataField, RelayMetadataVisibility>,
}

impl Default for RelayMetadataBoundary {
    fn default() -> Self {
        let visible = [
            RelayMetadataField::SourceVirtualIp,
            RelayMetadataField::DestinationVirtualIp,
            RelayMetadataField::IdentityRelationship,
            RelayMetadataField::PacketTiming,
            RelayMetadataField::PacketSize,
            RelayMetadataField::TrafficVolume,
        ];
        let hidden = [
            RelayMetadataField::ApplicationPayload,
            RelayMetadataField::PrivateKey,
            RelayMetadataField::SessionKey,
            RelayMetadataField::NetworkName,
            RelayMetadataField::MemberDisplayName,
        ];
        Self {
            fields: visible
                .into_iter()
                .map(|field| (field, RelayMetadataVisibility::Visible))
                .chain(
                    hidden
                        .into_iter()
                        .map(|field| (field, RelayMetadataVisibility::NotVisible)),
                )
                .collect(),
        }
    }
}

impl RelayMetadataBoundary {
    pub fn validate(&self) -> DomainResult<()> {
        for required in [
            RelayMetadataField::ApplicationPayload,
            RelayMetadataField::PrivateKey,
            RelayMetadataField::SessionKey,
        ] {
            if self.fields.get(&required) != Some(&RelayMetadataVisibility::NotVisible) {
                return Err(DomainError::invariant(
                    "Relay must not claim visibility into payload or key material",
                ));
            }
        }
        Ok(())
    }

    pub fn visibility(&self, field: RelayMetadataField) -> RelayMetadataVisibility {
        self.fields
            .get(&field)
            .copied()
            .unwrap_or(RelayMetadataVisibility::NotVisible)
    }
}

/// Stable governance action names for audit records and activity filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayGovernanceAction {
    Enable,
    Disable,
    AuthorizationChanged,
    QuotaChanged,
    SessionRejected,
    PacketRejected,
    Failover,
    Recovered,
}

impl RelayGovernanceAction {
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Enable => "relay.enable",
            Self::Disable => "relay.disable",
            Self::AuthorizationChanged => "relay.authorization.changed",
            Self::QuotaChanged => "relay.quota.changed",
            Self::SessionRejected => "relay.session.rejected",
            Self::PacketRejected => "relay.packet.rejected",
            Self::Failover => "relay.failover",
            Self::Recovered => "relay.recovered",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        AccessContext, NetworkAclGrant, PermissionCondition, PermissionEffect, SecurityRuleId,
        SubjectSelector,
    };

    fn ids() -> (NetworkId, NodeId, NodeId) {
        (
            NetworkId::from_public_key(b"relay-network"),
            NodeId::from_public_key(b"provider"),
            NodeId::from_public_key(b"member"),
        )
    }

    fn now() -> u64 {
        1_700_000_000_000
    }

    fn role_rule(network_id: &NetworkId, role: NetworkRole) -> PermissionRule {
        PermissionRule {
            rule_id: SecurityRuleId::from_public_key(
                format!("relay-{network_id}-{role:?}").as_bytes(),
            ),
            effect: PermissionEffect::Allow,
            subjects: vec![SubjectSelector::Role(role)],
            resources: vec![ResourceKind::Capability],
            actions: vec![PolicyAction::Use],
            conditions: vec![PermissionCondition::MembershipActive],
        }
    }

    fn policy(network_id: NetworkId, provider: NodeId, member: NodeId) -> RelayAccessPolicy {
        let mut members = BTreeMap::new();
        members.insert(
            member,
            RelayMemberPolicyContext {
                role: NetworkRole::Member,
                groups: BTreeSet::new(),
                membership_active: true,
                recent_reauthentication: true,
                local_rules: vec![role_rule(&network_id, NetworkRole::Member)],
                local_access_policy: None,
            },
        );
        RelayAccessPolicy {
            network_id: network_id.clone(),
            provider_node_id: provider.clone(),
            revision: 1,
            updated_at_unix_ms: now() - 1_000,
            network_rules: vec![
                role_rule(&network_id, NetworkRole::Owner),
                role_rule(&network_id, NetworkRole::Member),
            ],
            network_acl: None,
            provider_local_rules: vec![role_rule(&network_id, NetworkRole::Owner)],
            provider_local_policy: None,
            members,
            limits: RelayLimitPolicy {
                max_connections: Some(32),
                max_bandwidth_kbps: Some(1_024),
                ..RelayLimitPolicy::default()
            },
        }
    }

    #[test]
    fn limits_intersect_by_taking_the_stricter_value() {
        let network = RelayLimitPolicy {
            max_connections: Some(64),
            max_concurrent_streams: Some(128),
            max_bandwidth_kbps: Some(8_192),
            max_bytes: Some(10_000),
            max_cpu_percent: Some(50),
            max_memory_bytes: Some(1_000_000),
            max_session_duration_secs: Some(3_600),
            quota_reset_at_unix_ms: Some(5_000),
        };
        let local = RelayLimitPolicy {
            max_connections: Some(16),
            max_concurrent_streams: Some(256),
            max_bandwidth_kbps: Some(2_048),
            max_bytes: None,
            max_cpu_percent: Some(25),
            max_memory_bytes: Some(2_000_000),
            max_session_duration_secs: Some(1_800),
            quota_reset_at_unix_ms: Some(4_000),
        };
        let effective = network.intersection(&local).unwrap();
        assert_eq!(effective.max_connections, Some(16));
        assert_eq!(effective.max_concurrent_streams, Some(128));
        assert_eq!(effective.max_bandwidth_kbps, Some(2_048));
        assert_eq!(effective.max_bytes, Some(10_000));
        assert_eq!(effective.max_cpu_percent, Some(25));
        assert_eq!(effective.max_memory_bytes, Some(1_000_000));
        assert_eq!(effective.max_session_duration_secs, Some(1_800));
        assert_eq!(effective.quota_reset_at_unix_ms, Some(4_000));
    }

    #[test]
    fn authorization_requires_network_and_provider_allows() {
        let (network_id, provider, member) = ids();
        let policy = policy(network_id.clone(), provider, member.clone());
        assert!(policy.authorize_member(&member, now()).allowed);

        let mut vetoed = policy.clone();
        vetoed.provider_local_policy = Some(NodeLocalAccessPolicy {
            node_id: vetoed.provider_node_id.clone(),
            revision: 1,
            issued_at_unix_ms: now() - 1_000,
            expires_at_unix_ms: now() + 60_000,
            network_scope: crate::domain::NodeNetworkScope::All,
            contribution_vetoes: vec![crate::domain::NodeContributionVeto {
                veto_id: SecurityRuleId::from_public_key(b"relay-veto"),
                network_id: network_id.clone(),
                contributions: vec![crate::domain::ContributionKind::Relay],
            }],
            exit_contribution: None,
            rules: vec![role_rule(&network_id, NetworkRole::Owner)],
        });
        let decision = vetoed.authorize_member(&member, now());
        assert!(!decision.allowed);
        assert_eq!(
            decision.final_reason,
            Some(EffectiveAccessReason::LocalContributionVeto)
        );
    }

    #[test]
    fn network_acl_deny_wins_over_member_allow() {
        let (network_id, provider, member) = ids();
        let mut policy = policy(network_id.clone(), provider, member.clone());
        policy.network_acl = Some(NetworkAcl {
            network_id: network_id.clone(),
            revision: 2,
            issued_at_unix_ms: now() - 1_000,
            expires_at_unix_ms: now() + 60_000,
            rules: vec![PermissionRule {
                rule_id: SecurityRuleId::from_public_key(b"deny-relay"),
                effect: PermissionEffect::Deny,
                subjects: vec![SubjectSelector::Node(member.clone())],
                resources: vec![ResourceKind::Capability],
                actions: vec![PolicyAction::Use],
                conditions: Vec::new(),
            }],
            grants: Vec::<NetworkAclGrant>::new(),
        });
        let decision = policy.authorize_member(&member, now());
        assert!(!decision.allowed);
        assert_eq!(decision.final_reason, Some(EffectiveAccessReason::ExplicitDeny));
    }

    #[test]
    fn temporary_coordinator_cannot_use_relay() {
        let (network_id, provider, member) = ids();
        let mut policy = policy(network_id.clone(), provider, member.clone());
        policy.network_rules.push(role_rule(
            &network_id,
            NetworkRole::TemporaryCoordinator,
        ));
        policy
            .members
            .get_mut(&member)
            .unwrap()
            .role = NetworkRole::TemporaryCoordinator;
        assert!(!policy.authorize_member(&member, now()).allowed);
    }

    #[test]
    fn health_is_derived_from_observation_not_intent() {
        let limits = RelayLimitPolicy {
            max_connections: Some(10),
            ..RelayLimitPolicy::default()
        };
        let usage = RelayUsageSnapshot {
            active_connections: 10,
            ..RelayUsageSnapshot::default()
        };
        let health = RelayHealthSnapshot::observe(
            RelayLifecycleState::Running,
            Some("[::1]:4433".parse().unwrap()),
            true,
            true,
            true,
            true,
            limits,
            usage,
            now(),
            None,
        );
        assert_eq!(health.status, RelayHealthStatus::QuotaExhausted);
        assert!(health.quota_limited);

        let failed = RelayHealthSnapshot::observe(
            RelayLifecycleState::Failed,
            None,
            false,
            false,
            false,
            true,
            RelayLimitPolicy::default(),
            RelayUsageSnapshot::default(),
            now(),
            Some("relay.bind.address_in_use".into()),
        );
        assert_eq!(failed.status, RelayHealthStatus::Unavailable);
    }

    #[test]
    fn selection_is_bounded_and_fails_closed() {
        let candidate = |id: &str, health, failures| RelayCandidate {
            relay_id: id.into(),
            authorized: true,
            health,
            observed_at_unix_ms: now() - 1_000,
            load_percent: 10,
            latency_ms: Some(20),
            consecutive_failures: failures,
        };
        let outcome = select_relay_candidate(
            &[
                candidate("a", RelayHealthStatus::Healthy, 0),
                candidate("b", RelayHealthStatus::Unhealthy, 0),
                candidate("a", RelayHealthStatus::Healthy, 0),
            ],
            &RelaySelectionPolicy {
                max_attempts: 2,
                ..RelaySelectionPolicy::default()
            },
            now(),
        )
        .unwrap();
        assert_eq!(outcome.selected.unwrap().relay_id, "a");
        assert_eq!(outcome.attempts_allowed, 1);
        assert_eq!(
            outcome.denied.iter().map(|item| &item.reason).collect::<Vec<_>>(),
            vec![
                &RelayAuthorizationDenial::Unhealthy,
                &RelayAuthorizationDenial::DuplicateCandidate
            ]
        );

        let none = select_relay_candidate(
            &[candidate("stale", RelayHealthStatus::Healthy, 0)],
            &RelaySelectionPolicy {
                max_health_age_ms: 1,
                ..RelaySelectionPolicy::default()
            },
            now(),
        )
        .unwrap();
        assert!(none.fail_closed);
        assert!(none.selected.is_none());
    }

    #[test]
    fn metadata_boundary_never_exposes_payload_or_keys() {
        let boundary = RelayMetadataBoundary::default();
        boundary.validate().unwrap();
        assert_eq!(
            boundary.visibility(RelayMetadataField::ApplicationPayload),
            RelayMetadataVisibility::NotVisible
        );
        assert_eq!(
            boundary.visibility(RelayMetadataField::SourceVirtualIp),
            RelayMetadataVisibility::Visible
        );
        assert_eq!(
            boundary.visibility(RelayMetadataField::PrivateKey),
            RelayMetadataVisibility::NotVisible
        );
    }

    #[test]
    fn access_context_contract_remains_composeable() {
        // Keep this type exercised so future policy refactors cannot silently
        // change the invariant that member contexts are fully trusted inputs.
        let (network_id, provider, member) = ids();
        let policy = policy(network_id.clone(), provider, member.clone());
        let context = AccessContext {
            network_id,
            actor: member,
            role: NetworkRole::Member,
            groups: BTreeSet::new(),
            membership_active: true,
            recent_reauthentication: true,
            emergency_coordinator: false,
        };
        assert!(context.membership_active);
        assert!(policy.validate_at(now()).is_ok());
    }
}
