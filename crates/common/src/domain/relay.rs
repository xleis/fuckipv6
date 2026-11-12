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
use super::ids::{GroupId, NetworkId, NodeId, PublicKey, SecurityRuleId, Signature};
use super::permissions::{
    NetworkRole, PermissionCondition, PermissionEffect, PermissionIntent, PermissionRequest,
    PermissionRule, PolicyAction, ResourceKind, ResourceRef, SubjectSelector,
};
use super::{DomainError, DomainResult};
use crate::crypto::Identity;

const RELAY_MEMBER_GRANT_SCHEMA_VERSION: u16 = 1;
const RELAY_MEMBER_GRANT_SIGNING_DOMAIN: &[u8] = b"fuckipv6/relay-member-grant/v1\0";
/// A successful forwarding operation is the only built-in evidence that the
/// Relay path is currently usable. Older evidence is not treated as healthy.
pub const RELAY_FORWARDING_FRESHNESS_MS: u64 = 30_000;

/// Hard limit set for one Relay direction. When a network policy and a node
/// policy both define a value, [`RelayLimitPolicy::intersection`] keeps the
/// stricter one.
#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct RelayLimitPolicy {
    #[serde(default)]
    pub max_connections: Option<u32>,
    #[serde(default)]
    pub max_concurrent_streams: Option<u32>,
    #[serde(default)]
    pub max_bandwidth_kbps: Option<u64>,
    #[serde(default)]
    pub max_bytes: Option<u64>,
    #[serde(default)]
    pub max_cpu_percent: Option<u8>,
    #[serde(default)]
    pub max_memory_bytes: Option<u64>,
    #[serde(default)]
    pub max_session_duration_secs: Option<u64>,
    #[serde(default)]
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
        if self
            .max_cpu_percent
            .is_some_and(|value| !(1..=100).contains(&value))
        {
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

    /// Whether this policy layer contributes any configured value. An empty
    /// layer is valid (it inherits the other layers), but it must not be
    /// reported as a source in Relay health diagnostics.
    pub fn has_configured_values(&self) -> bool {
        self.max_connections.is_some()
            || self.max_concurrent_streams.is_some()
            || self.max_bandwidth_kbps.is_some()
            || self.max_bytes.is_some()
            || self.max_cpu_percent.is_some()
            || self.max_memory_bytes.is_some()
            || self.max_session_duration_secs.is_some()
            || self.quota_reset_at_unix_ms.is_some()
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
    #[serde(default)]
    pub cpu_measurement: RelayMeasurementState,
    #[serde(default)]
    pub memory_measurement: RelayMeasurementState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_forwarded_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_forwarding_failure_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_started_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_reset_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayMeasurementState {
    Unsupported,
    #[default]
    NotMeasured,
    Measured,
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
pub enum RelayReachabilityState {
    Unknown,
    Reachable,
    Unreachable,
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
    NetworkReachabilityUnknown,
    NetworkUnreachable,
    ProtocolError,
    QuotaExhausted,
    ResourcePressure,
    ForwardingStalled,
    ForwardingNotObserved,
    AuthorizationExpired,
    CpuMeasurementUnsupported,
    MemoryMeasurementUnsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayLimitSource {
    NetworkAcl,
    NodeLocal,
    ProviderDefault,
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
    pub network_reachability: RelayReachabilityState,
    pub forwarding_observed: bool,
    pub authorization_valid: bool,
    pub quota_limited: bool,
    pub limits: RelayLimitPolicy,
    #[serde(default)]
    pub limit_sources: Vec<RelayLimitSource>,
    pub usage: RelayUsageSnapshot,
    pub issues: Vec<RelayHealthIssue>,
    pub observed_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
}

impl RelayHealthSnapshot {
    /// Build a health snapshot and derive the status from observable evidence.
    #[allow(clippy::too_many_arguments)]
    pub fn observe(
        lifecycle: RelayLifecycleState,
        listen_addr: Option<SocketAddr>,
        listener_bound: bool,
        network_reachability: RelayReachabilityState,
        forwarding_observed: bool,
        authorization_valid: bool,
        limits: RelayLimitPolicy,
        limit_sources: Vec<RelayLimitSource>,
        usage: RelayUsageSnapshot,
        observed_at_unix_ms: u64,
        reason_code: Option<String>,
    ) -> Self {
        let mut issues = Vec::new();
        let forwarding_is_fresh = forwarding_observed
            && usage.last_forwarded_at_unix_ms.is_some_and(|forwarded_at| {
                forwarded_at <= observed_at_unix_ms
                    && observed_at_unix_ms.saturating_sub(forwarded_at)
                        <= RELAY_FORWARDING_FRESHNESS_MS
            });
        let forwarding_stalled = usage
            .last_forwarding_failure_at_unix_ms
            .is_some_and(|failure| {
                usage
                    .last_forwarded_at_unix_ms
                    .is_none_or(|success| failure > success)
            });
        if !listener_bound || listen_addr.is_none() {
            issues.push(RelayHealthIssue::ListenerUnavailable);
        }
        match network_reachability {
            RelayReachabilityState::Unknown => {
                issues.push(RelayHealthIssue::NetworkReachabilityUnknown)
            }
            RelayReachabilityState::Unreachable => {
                issues.push(RelayHealthIssue::NetworkUnreachable)
            }
            RelayReachabilityState::Reachable => {}
        }
        if listener_bound && !forwarding_is_fresh {
            issues.push(RelayHealthIssue::ForwardingNotObserved);
        }
        if forwarding_stalled {
            issues.push(RelayHealthIssue::ForwardingStalled);
        }
        if !authorization_valid {
            issues.push(RelayHealthIssue::AuthorizationExpired);
        }
        let quota_limited = usage.quota_limited(&limits);
        if quota_limited {
            issues.push(RelayHealthIssue::QuotaExhausted);
        }
        if limits.max_cpu_percent.is_some()
            && usage.cpu_measurement == RelayMeasurementState::Unsupported
        {
            issues.push(RelayHealthIssue::CpuMeasurementUnsupported);
        }
        if limits.max_memory_bytes.is_some()
            && usage.memory_measurement == RelayMeasurementState::Unsupported
        {
            issues.push(RelayHealthIssue::MemoryMeasurementUnsupported);
        }
        if limits
            .max_connections
            .zip(Some(usage.active_connections))
            .is_some_and(|(limit, active)| active.saturating_mul(10) >= limit.saturating_mul(9))
            || limits
                .max_concurrent_streams
                .zip(Some(usage.active_streams))
                .is_some_and(|(limit, active)| active.saturating_mul(10) >= limit.saturating_mul(9))
        {
            issues.push(RelayHealthIssue::ResourcePressure);
        }

        let status = match lifecycle {
            RelayLifecycleState::Running => {
                if !listener_bound
                    || network_reachability == RelayReachabilityState::Unreachable
                    || !authorization_valid
                {
                    RelayHealthStatus::Unavailable
                } else if quota_limited {
                    RelayHealthStatus::QuotaExhausted
                } else if forwarding_stalled {
                    RelayHealthStatus::Degraded
                } else if network_reachability == RelayReachabilityState::Unknown
                    || !forwarding_is_fresh
                {
                    RelayHealthStatus::Unknown
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
            network_reachability,
            forwarding_observed: forwarding_is_fresh,
            authorization_valid,
            quota_limited,
            limits,
            limit_sources,
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

/// A consumer-authored, verifiable grant allowing this node to use Relay from
/// one provider in one network. Possession of network membership or an entry in
/// the network ACL is not sufficient: the provider must verify this signature
/// before treating the member's local layer as allowing Relay.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct RelayMemberGrant {
    pub schema_version: u16,
    pub network_id: NetworkId,
    pub provider_node_id: NodeId,
    pub consumer_node_id: NodeId,
    pub consumer_public_key: PublicKey,
    /// Unique, non-secret value that prevents two grants from being confused
    /// during import, audit, rotation or revocation.
    pub nonce: String,
    pub revision: u64,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    #[serde(default)]
    pub limits: RelayLimitPolicy,
    pub signature: Signature,
}

#[derive(Debug, Clone, Serialize)]
struct RelayMemberGrantPayload<'a> {
    schema_version: u16,
    network_id: &'a NetworkId,
    provider_node_id: &'a NodeId,
    consumer_node_id: &'a NodeId,
    consumer_public_key: &'a PublicKey,
    nonce: &'a str,
    revision: u64,
    issued_at_unix_ms: u64,
    expires_at_unix_ms: u64,
    limits: &'a RelayLimitPolicy,
}

impl RelayMemberGrant {
    /// Create and sign a grant using the consumer's durable node identity.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        network_id: NetworkId,
        provider_node_id: NodeId,
        identity: &Identity,
        nonce: impl Into<String>,
        revision: u64,
        issued_at_unix_ms: u64,
        expires_at_unix_ms: u64,
        limits: RelayLimitPolicy,
    ) -> DomainResult<Self> {
        let mut grant = Self {
            schema_version: RELAY_MEMBER_GRANT_SCHEMA_VERSION,
            network_id,
            provider_node_id,
            consumer_node_id: NodeId::from_public_key(&identity.public_key()),
            consumer_public_key: PublicKey::from_bytes(identity.public_key_bytes()),
            nonce: nonce.into(),
            revision,
            issued_at_unix_ms,
            expires_at_unix_ms,
            limits,
            // Replaced immediately below. Keeping this constructor total avoids
            // callers having to reproduce the signed representation.
            signature: Signature::from_bytes([0; 64]),
        };
        grant.validate_definition()?;
        let signature = identity.sign(&grant.signing_payload()?);
        grant.signature = Signature::from_bytes(signature.to_bytes());
        Ok(grant)
    }

    fn payload(&self) -> RelayMemberGrantPayload<'_> {
        RelayMemberGrantPayload {
            schema_version: self.schema_version,
            network_id: &self.network_id,
            provider_node_id: &self.provider_node_id,
            consumer_node_id: &self.consumer_node_id,
            consumer_public_key: &self.consumer_public_key,
            nonce: &self.nonce,
            revision: self.revision,
            issued_at_unix_ms: self.issued_at_unix_ms,
            expires_at_unix_ms: self.expires_at_unix_ms,
            limits: &self.limits,
        }
    }

    /// Domain-separated canonical JSON covered by the consumer signature.
    pub fn signing_payload(&self) -> DomainResult<Vec<u8>> {
        let encoded = serde_json::to_vec(&self.payload()).map_err(|error| {
            DomainError::invalid_value(format!("Relay member grant is not serializable: {error}"))
        })?;
        let mut payload =
            Vec::with_capacity(RELAY_MEMBER_GRANT_SIGNING_DOMAIN.len() + encoded.len());
        payload.extend_from_slice(RELAY_MEMBER_GRANT_SIGNING_DOMAIN);
        payload.extend_from_slice(&encoded);
        Ok(payload)
    }

    fn validate_definition(&self) -> DomainResult<()> {
        if self.schema_version != RELAY_MEMBER_GRANT_SCHEMA_VERSION {
            return Err(DomainError::invalid_value(
                "unsupported Relay member grant schema",
            ));
        }
        if self.revision == 0 {
            return Err(DomainError::invalid_value(
                "Relay member grant revision must be positive",
            ));
        }
        if self.nonce.is_empty()
            || self.nonce.len() > 128
            || self.nonce.chars().any(char::is_control)
        {
            return Err(DomainError::invalid_value(
                "Relay member grant nonce is invalid",
            ));
        }
        if self.issued_at_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::invariant(
                "Relay member grant validity interval is empty",
            ));
        }
        self.limits.validate()?;
        let expected_consumer = NodeId::from_public_key(&self.consumer_public_key.to_bytes()?);
        if expected_consumer != self.consumer_node_id {
            return Err(DomainError::invariant(
                "Relay member grant consumer id does not match its public key",
            ));
        }
        Ok(())
    }

    /// Verify signature, validity and identity binding before importing a grant.
    pub fn validate_at(&self, now_unix_ms: u64) -> DomainResult<()> {
        self.validate_definition()?;
        if now_unix_ms < self.issued_at_unix_ms {
            return Err(DomainError::new(
                "domain.relay_member_grant_not_yet_valid",
                "Relay member grant is not yet valid",
            ));
        }
        if now_unix_ms >= self.expires_at_unix_ms {
            return Err(DomainError::new(
                "domain.relay_member_grant_expired",
                "Relay member grant has expired",
            ));
        }
        let public_key = self.consumer_public_key.to_bytes()?;
        let signature = self.signature.to_bytes()?;
        if !Identity::verify(&public_key, &self.signing_payload()?, &signature) {
            return Err(DomainError::new(
                "domain.invalid_signature",
                "Relay member grant signature is invalid",
            ));
        }
        Ok(())
    }

    /// Validate the grant for the exact network, provider and consumer being
    /// authorized. Cross-network and cross-provider replay fails closed.
    pub fn validate_binding(
        &self,
        network_id: &NetworkId,
        provider_node_id: &NodeId,
        consumer_node_id: &NodeId,
        now_unix_ms: u64,
    ) -> DomainResult<()> {
        if &self.network_id != network_id {
            return Err(DomainError::invariant(
                "Relay member grant belongs to another network",
            ));
        }
        if &self.provider_node_id != provider_node_id {
            return Err(DomainError::invariant(
                "Relay member grant belongs to another provider",
            ));
        }
        if &self.consumer_node_id != consumer_node_id {
            return Err(DomainError::invariant(
                "Relay member grant belongs to another consumer",
            ));
        }
        self.validate_at(now_unix_ms)
    }

    /// Local consumer-side allow rule distilled from the signed grant. The
    /// network ACL and provider local policy still have independent vetoes.
    pub fn local_rules(&self) -> Vec<PermissionRule> {
        vec![PermissionRule {
            rule_id: SecurityRuleId::from_public_key(
                format!(
                    "relay-member-grant:{}:{}:{}",
                    self.network_id, self.provider_node_id, self.nonce
                )
                .as_bytes(),
            ),
            effect: PermissionEffect::Allow,
            subjects: vec![SubjectSelector::Node(self.consumer_node_id.clone())],
            resources: vec![ResourceKind::Relay],
            actions: vec![PolicyAction::Use],
            conditions: vec![
                PermissionCondition::MembershipActive,
                PermissionCondition::ResourceInNetwork(self.network_id.clone()),
            ],
        }]
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
    /// Consumer-authored (or, for the provider context, owner-authored)
    /// resource limits. These are intersected with the policy-wide limits.
    #[serde(default)]
    pub limits: RelayLimitPolicy,
}

impl RelayMemberPolicyContext {
    pub fn validate_for(&self, node_id: &NodeId, now_unix_ms: u64) -> DomainResult<()> {
        for rule in &self.local_rules {
            rule.validate()?;
        }
        self.limits.validate()?;
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
    /// Trusted role, membership and local-policy context for the node that
    /// provides Relay service. This is deliberately separate from `members`:
    /// the provider may itself be a consumer, but its contribution permission
    /// must never be inferred as an owner permission.
    pub provider: RelayMemberPolicyContext,
    #[serde(default)]
    pub existing_session_policy: RelayExistingSessionPolicy,
    pub revision: u64,
    pub updated_at_unix_ms: u64,
    #[serde(default)]
    pub network_rules: Vec<PermissionRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_acl: Option<NetworkAcl>,
    #[serde(default)]
    pub members: BTreeMap<NodeId, RelayMemberPolicyContext>,
    pub limits: RelayLimitPolicy,
    /// Policy layers that contributed at least one configured value to
    /// `limits`. This is control-plane metadata and is never inferred from an
    /// untrusted Relay registration.
    #[serde(default)]
    pub limit_sources: Vec<RelayLimitSource>,
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
    /// Stricter of the provider-wide and consumer-authored limits. It is
    /// present even on denial so diagnostics can explain the effective bound.
    pub limits: RelayLimitPolicy,
}

impl RelayAccessPolicy {
    pub fn validate_at(&self, now_unix_ms: u64) -> DomainResult<()> {
        if self.revision == 0 {
            return Err(DomainError::invalid_value(
                "Relay access policy revision must be positive",
            ));
        }
        self.limits.validate()?;
        for rule in &self.network_rules {
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
        self.provider
            .validate_for(&self.provider_node_id, now_unix_ms)?;
        for (node_id, context) in &self.members {
            context.validate_for(node_id, now_unix_ms)?;
        }
        Ok(())
    }

    fn request() -> PermissionRequest {
        PermissionRequest {
            action: PolicyAction::Use,
            resource: ResourceRef {
                kind: ResourceKind::Relay,
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
            role: self.provider.role,
            groups: self.provider.groups.clone(),
            membership_active: self.provider.membership_active,
            recent_reauthentication: self.provider.recent_reauthentication,
            emergency_coordinator: self.provider.role == NetworkRole::TemporaryCoordinator,
        };
        evaluate_effective_access(
            &self.provider.local_rules,
            &self.network_rules,
            self.network_acl.as_ref(),
            self.provider.local_access_policy.as_ref(),
            &request,
            &context,
            now_unix_ms,
        )
    }

    /// Authorize one member for Relay use. The provider side is checked first;
    /// an expired or denied provider policy fails closed even when the network
    /// would allow the member.
    pub fn authorize_member(
        &self,
        node_id: &NodeId,
        now_unix_ms: u64,
    ) -> DomainResult<RelayAuthorizationDecision> {
        let provider = self.provider_decision(now_unix_ms);
        let Some(member) = self.members.get(node_id) else {
            return Ok(RelayAuthorizationDecision {
                allowed: false,
                final_reason: Some(EffectiveAccessReason::DefaultDenied),
                provider,
                consumer: None,
                limits: self.limits.clone(),
            });
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
        let limits = member.limits.intersection(&self.limits).map_err(|error| {
            DomainError::new(
                error.code(),
                format!("Relay effective limit intersection failed: {error}"),
            )
        })?;
        Ok(RelayAuthorizationDecision {
            allowed,
            provider,
            consumer: Some(consumer),
            final_reason,
            limits,
        })
    }

    /// Authorize the exact member named by a signed consumer grant. The caller
    /// supplies only identity/role/membership facts sourced from the trusted
    /// network roster; all grant-authored authorization and limits are copied
    /// from inside the signed envelope. The grant's local allow never replaces
    /// the provider local policy or the network ACL veto.
    #[allow(clippy::too_many_arguments)]
    pub fn authorize_signed_grant(
        &mut self,
        grant: &RelayMemberGrant,
        role: NetworkRole,
        groups: BTreeSet<GroupId>,
        membership_active: bool,
        recent_reauthentication: bool,
        now_unix_ms: u64,
    ) -> DomainResult<RelayAuthorizationDecision> {
        grant.validate_binding(
            &self.network_id,
            &self.provider_node_id,
            &grant.consumer_node_id,
            now_unix_ms,
        )?;
        self.members.insert(
            grant.consumer_node_id.clone(),
            RelayMemberPolicyContext {
                role,
                groups,
                membership_active,
                recent_reauthentication,
                local_rules: grant.local_rules(),
                local_access_policy: None,
                limits: grant.limits.clone(),
            },
        );
        self.authorize_member(&grant.consumer_node_id, now_unix_ms)
    }

    /// Recompute all member decisions after a live policy change. Existing
    /// sessions can then follow an explicit continue/migrate/terminate policy.
    pub fn authorization_snapshot(
        &self,
        now_unix_ms: u64,
    ) -> DomainResult<BTreeMap<NodeId, RelayAuthorizationDecision>> {
        self.validate_at(now_unix_ms)?;
        self.members
            .keys()
            .map(|node_id| {
                self.authorize_member(node_id, now_unix_ms)
                    .map(|decision| (node_id.clone(), decision))
            })
            .collect()
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
    Cooldown,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_attempt_at_unix_ms: Option<u64>,
    #[serde(default)]
    pub loop_detected: bool,
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
    /// Eligible candidates in deterministic attempt order. Never contains more
    /// than `max_attempts` entries and is empty when selection fails closed.
    #[serde(default)]
    pub candidates: Vec<RelayCandidate>,
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
        } else if candidate.observed_at_unix_ms > now_unix_ms || age > policy.max_health_age_ms {
            Some(RelayAuthorizationDenial::StaleHealth)
        } else if candidate.loop_detected {
            Some(RelayAuthorizationDenial::LoopDetected)
        } else if candidate
            .last_attempt_at_unix_ms
            .is_some_and(|last_attempt| {
                now_unix_ms.saturating_sub(last_attempt) < policy.switch_cooldown_ms
            })
        {
            Some(RelayAuthorizationDenial::Cooldown)
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
            candidates: Vec::new(),
            attempts_allowed: 0,
            denied,
            fail_closed: true,
            reason: "relay.selection.no_eligible_candidate".into(),
        });
    }
    let attempts_allowed = if policy.allow_fallback {
        policy.max_attempts.min(eligible.len() as u32)
    } else {
        1
    };
    eligible.truncate(attempts_allowed as usize);
    Ok(RelaySelectionOutcome {
        selected: eligible.first().cloned(),
        candidates: eligible,
        attempts_allowed,
        denied,
        fail_closed: false,
        reason: "relay.selection.eligible_candidate".into(),
    })
}

/// Metadata fields that a Relay operator can observe. Encryption protects the
/// payload but cannot hide packet timing or endpoint relationships.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, JsonSchema, Serialize, Deserialize,
)]
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

/// One bounded, payload-free Relay governance observation.
///
/// Events intentionally carry no source/destination address, peer display
/// name, packet payload, key material or session secret. `node_id` identifies
/// the consumer whose request was rejected or whose authorization changed so
/// an owner can filter the audit without exposing the Relay topology.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct RelayGovernanceEvent {
    pub action: RelayGovernanceAction,
    pub occurred_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<NodeId>,
    pub reason_code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_at_unix_ms: Option<u64>,
}

impl RelayGovernanceEvent {
    pub fn validate(&self) -> DomainResult<()> {
        if self.reason_code.is_empty()
            || self.reason_code.chars().any(char::is_control)
            || self.reason_code.len() > 96
        {
            return Err(DomainError::invalid_value(
                "Relay governance reason code must be a bounded stable identifier",
            ));
        }
        Ok(())
    }
}

/// Snapshot returned by the Relay governance sink. Older events are evicted
/// first; `dropped_events` makes queue pressure observable rather than silent.
#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct RelayGovernanceAuditSnapshot {
    pub events: Vec<RelayGovernanceEvent>,
    pub dropped_events: u64,
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
            resources: vec![ResourceKind::Relay],
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
                limits: RelayLimitPolicy::default(),
            },
        );
        RelayAccessPolicy {
            network_id: network_id.clone(),
            provider_node_id: provider.clone(),
            provider: RelayMemberPolicyContext {
                role: NetworkRole::Owner,
                groups: BTreeSet::new(),
                membership_active: true,
                recent_reauthentication: true,
                local_rules: vec![role_rule(&network_id, NetworkRole::Owner)],
                local_access_policy: None,
                limits: RelayLimitPolicy::default(),
            },
            existing_session_policy: RelayExistingSessionPolicy::Reauthorize,
            revision: 1,
            updated_at_unix_ms: now() - 1_000,
            network_rules: vec![
                role_rule(&network_id, NetworkRole::Owner),
                role_rule(&network_id, NetworkRole::Member),
            ],
            network_acl: None,
            members,
            limits: RelayLimitPolicy {
                max_connections: Some(32),
                max_bandwidth_kbps: Some(1_024),
                ..RelayLimitPolicy::default()
            },
            limit_sources: vec![RelayLimitSource::ProviderDefault],
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
        assert!(
            policy
                .authorize_member(&member, now())
                .expect("authorization")
                .allowed
        );

        let mut vetoed = policy.clone();
        vetoed.provider.local_access_policy = Some(NodeLocalAccessPolicy {
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
            relay_limits: Box::default(),
            rules: vec![role_rule(&network_id, NetworkRole::Owner)],
        });
        let decision = vetoed
            .authorize_member(&member, now())
            .expect("authorization");
        assert!(!decision.allowed);
        assert_eq!(
            decision.final_reason,
            Some(EffectiveAccessReason::LocalContributionVeto)
        );
    }

    #[test]
    fn relay_requires_explicit_local_and_network_allows_on_both_sides() {
        let (network_id, provider, member) = ids();
        let policy = policy(network_id.clone(), provider, member.clone());

        let mut missing_provider_local = policy.clone();
        missing_provider_local.provider.local_rules.clear();
        let denied = missing_provider_local
            .authorize_member(&member, now())
            .expect("authorization");
        assert!(!denied.allowed);
        assert_eq!(
            denied.final_reason,
            Some(EffectiveAccessReason::DefaultDenied)
        );

        let mut missing_consumer_local = policy.clone();
        missing_consumer_local
            .members
            .get_mut(&member)
            .unwrap()
            .local_rules
            .clear();
        let denied = missing_consumer_local
            .authorize_member(&member, now())
            .expect("authorization");
        assert!(!denied.allowed);
        assert_eq!(
            denied.final_reason,
            Some(EffectiveAccessReason::DefaultDenied)
        );

        let mut missing_network = policy;
        missing_network.network_rules.clear();
        let denied = missing_network
            .authorize_member(&member, now())
            .expect("authorization");
        assert!(!denied.allowed);
        assert_eq!(
            denied.final_reason,
            Some(EffectiveAccessReason::DefaultDenied)
        );
    }

    #[test]
    fn provider_authorization_uses_trusted_member_context() {
        let (network_id, provider, member) = ids();
        let mut policy = policy(network_id.clone(), provider, member.clone());
        // A non-owner may provide Relay when both network and node-local rules
        // explicitly allow that role. The old owner fallback would fail closed
        // here because no Owner allow exists.
        policy.network_rules = vec![role_rule(&network_id, NetworkRole::Member)];
        policy.provider.role = NetworkRole::Member;
        policy.provider.local_rules = vec![role_rule(&network_id, NetworkRole::Member)];
        assert!(
            policy
                .authorize_member(&member, now())
                .expect("authorization")
                .allowed
        );

        policy.provider.membership_active = false;
        let denied = policy
            .authorize_member(&member, now())
            .expect("authorization");
        assert!(!denied.allowed);
        assert_eq!(
            denied.final_reason,
            Some(EffectiveAccessReason::DefaultDenied)
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
                resources: vec![ResourceKind::Relay],
                actions: vec![PolicyAction::Use],
                conditions: Vec::new(),
            }],
            grants: Vec::<NetworkAclGrant>::new(),
            relay_limits: Box::default(),
        });
        let decision = policy
            .authorize_member(&member, now())
            .expect("authorization");
        assert!(!decision.allowed);
        assert_eq!(
            decision.final_reason,
            Some(EffectiveAccessReason::ExplicitDeny)
        );
    }

    #[test]
    fn temporary_coordinator_cannot_use_relay() {
        let (network_id, provider, member) = ids();
        let mut policy = policy(network_id.clone(), provider, member.clone());
        policy
            .network_rules
            .push(role_rule(&network_id, NetworkRole::TemporaryCoordinator));
        policy.members.get_mut(&member).unwrap().role = NetworkRole::TemporaryCoordinator;
        assert!(
            !policy
                .authorize_member(&member, now())
                .expect("authorization")
                .allowed
        );
    }

    #[test]
    fn relay_register_with_nested_grant_is_postcard_safe() -> anyhow::Result<()> {
        let consumer = Identity::generate()?;
        let provider = NodeId::from_public_key(b"postcard-provider");
        let network = NetworkId::from_public_key(b"postcard-network");
        let grant = RelayMemberGrant::new(
            network,
            provider,
            &consumer,
            "postcard-grant",
            7,
            now(),
            now() + 60_000,
            RelayLimitPolicy {
                max_connections: Some(4),
                max_concurrent_streams: None,
                max_bandwidth_kbps: Some(2_048),
                max_bytes: None,
                max_cpu_percent: Some(50),
                max_memory_bytes: None,
                max_session_duration_secs: Some(600),
                quota_reset_at_unix_ms: None,
            },
        )?;
        let message = crate::RelayMessage::Register {
            client_id: consumer.client_id().to_string(),
            virtual_ip: "fd00::2".parse()?,
            public_key: consumer.public_key(),
            timestamp: now() / 1_000,
            server_public_key_pin: [1; 32],
            client_certificate_public_key_pin: [2; 32],
            tls_channel_binding: [3; 32],
            signature: vec![4; 64],
            member_grant: Some(Box::new(grant.clone())),
        };

        let encoded = crate::codec::encode(&message)?;
        let decoded: crate::RelayMessage = crate::codec::decode(&encoded)?;

        assert_eq!(decoded, message);

        let update = crate::RelayMessage::UpdateAuthorization {
            member_grant: Box::new(grant),
        };
        let encoded = crate::codec::encode(&update)?;
        let decoded: crate::RelayMessage = crate::codec::decode(&encoded)?;
        assert_eq!(decoded, update);
        Ok(())
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
            RelayReachabilityState::Reachable,
            true,
            true,
            limits,
            vec![RelayLimitSource::NetworkAcl],
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
            RelayReachabilityState::Unknown,
            false,
            true,
            RelayLimitPolicy::default(),
            Vec::new(),
            RelayUsageSnapshot::default(),
            now(),
            Some("relay.bind.address_in_use".into()),
        );
        assert_eq!(failed.status, RelayHealthStatus::Unavailable);
    }

    #[test]
    fn bound_listener_alone_is_not_healthy() {
        let health = RelayHealthSnapshot::observe(
            RelayLifecycleState::Running,
            Some("[::1]:4433".parse().unwrap()),
            true,
            RelayReachabilityState::Unknown,
            false,
            true,
            RelayLimitPolicy::default(),
            Vec::new(),
            RelayUsageSnapshot::default(),
            100_000,
            None,
        );

        assert_eq!(health.status, RelayHealthStatus::Unknown);
        assert!(!health.forwarding_observed);
        assert!(health
            .issues
            .contains(&RelayHealthIssue::NetworkReachabilityUnknown));
        assert!(health
            .issues
            .contains(&RelayHealthIssue::ForwardingNotObserved));
        assert_ne!(health.status, RelayHealthStatus::Healthy);
    }

    #[test]
    fn fresh_forwarding_proves_health() {
        let usage = RelayUsageSnapshot {
            last_forwarded_at_unix_ms: Some(99_000),
            cpu_measurement: RelayMeasurementState::Unsupported,
            memory_measurement: RelayMeasurementState::Unsupported,
            ..RelayUsageSnapshot::default()
        };
        let health = RelayHealthSnapshot::observe(
            RelayLifecycleState::Running,
            Some("[::1]:4433".parse().unwrap()),
            true,
            RelayReachabilityState::Reachable,
            true,
            true,
            RelayLimitPolicy::default(),
            Vec::new(),
            usage,
            100_000,
            None,
        );

        assert_eq!(health.status, RelayHealthStatus::Healthy);
        assert!(health.forwarding_observed);
        assert!(!health
            .issues
            .contains(&RelayHealthIssue::ForwardingNotObserved));
    }

    #[test]
    fn stale_forwarding_and_newer_failure_are_degraded() {
        let fresh_limit = 100_000;
        let stale = RelayUsageSnapshot {
            last_forwarded_at_unix_ms: Some(fresh_limit - RELAY_FORWARDING_FRESHNESS_MS - 1),
            ..RelayUsageSnapshot::default()
        };
        let health = RelayHealthSnapshot::observe(
            RelayLifecycleState::Running,
            Some("[::1]:4433".parse().unwrap()),
            true,
            RelayReachabilityState::Reachable,
            true,
            true,
            RelayLimitPolicy::default(),
            Vec::new(),
            stale,
            fresh_limit,
            None,
        );
        assert_eq!(health.status, RelayHealthStatus::Unknown);
        assert!(health
            .issues
            .contains(&RelayHealthIssue::ForwardingNotObserved));

        let stalled = RelayUsageSnapshot {
            last_forwarded_at_unix_ms: Some(99_000),
            last_forwarding_failure_at_unix_ms: Some(99_500),
            ..RelayUsageSnapshot::default()
        };
        let health = RelayHealthSnapshot::observe(
            RelayLifecycleState::Running,
            Some("[::1]:4433".parse().unwrap()),
            true,
            RelayReachabilityState::Reachable,
            true,
            true,
            RelayLimitPolicy::default(),
            Vec::new(),
            stalled,
            100_000,
            None,
        );
        assert_eq!(health.status, RelayHealthStatus::Degraded);
        assert!(health.forwarding_observed);
        assert!(health.issues.contains(&RelayHealthIssue::ForwardingStalled));
    }

    #[test]
    fn unmeasurable_resource_limits_are_explicitly_unsupported() {
        let limits = RelayLimitPolicy {
            max_cpu_percent: Some(80),
            max_memory_bytes: Some(1024),
            ..RelayLimitPolicy::default()
        };
        let usage = RelayUsageSnapshot {
            last_forwarded_at_unix_ms: Some(99_000),
            cpu_measurement: RelayMeasurementState::Unsupported,
            memory_measurement: RelayMeasurementState::Unsupported,
            ..RelayUsageSnapshot::default()
        };
        let health = RelayHealthSnapshot::observe(
            RelayLifecycleState::Running,
            Some("[::1]:4433".parse().unwrap()),
            true,
            RelayReachabilityState::Reachable,
            true,
            true,
            limits,
            vec![
                RelayLimitSource::NetworkAcl,
                RelayLimitSource::ProviderDefault,
            ],
            usage,
            100_000,
            None,
        );

        assert_eq!(health.status, RelayHealthStatus::Degraded);
        assert!(health
            .issues
            .contains(&RelayHealthIssue::CpuMeasurementUnsupported));
        assert!(health
            .issues
            .contains(&RelayHealthIssue::MemoryMeasurementUnsupported));
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
            last_attempt_at_unix_ms: None,
            loop_detected: false,
        };
        let outcome = select_relay_candidate(
            &[
                candidate("a", RelayHealthStatus::Healthy, 0),
                candidate("b", RelayHealthStatus::Unavailable, 0),
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
            outcome
                .denied
                .iter()
                .map(|item| &item.reason)
                .collect::<Vec<_>>(),
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
    fn selection_returns_ordered_attempts_and_honors_cooldown_and_loops() {
        let candidate = |id: &str,
                         latency_ms: u32,
                         last_attempt_at_unix_ms: Option<u64>,
                         loop_detected: bool| RelayCandidate {
            relay_id: id.into(),
            authorized: true,
            health: RelayHealthStatus::Healthy,
            observed_at_unix_ms: now() - 1_000,
            load_percent: 10,
            latency_ms: Some(latency_ms),
            consecutive_failures: 0,
            last_attempt_at_unix_ms,
            loop_detected,
        };
        let outcome = select_relay_candidate(
            &[
                candidate("slow", 80, None, false),
                candidate("fast", 10, None, false),
                candidate("cooling", 5, Some(now() - 100), false),
                candidate("loop", 1, None, true),
            ],
            &RelaySelectionPolicy {
                max_attempts: 2,
                switch_cooldown_ms: 5_000,
                ..RelaySelectionPolicy::default()
            },
            now(),
        )
        .unwrap();

        assert_eq!(
            outcome
                .candidates
                .iter()
                .map(|candidate| candidate.relay_id.as_str())
                .collect::<Vec<_>>(),
            vec!["fast", "slow"]
        );
        assert_eq!(outcome.selected.unwrap().relay_id, "fast");
        assert_eq!(outcome.attempts_allowed, 2);
        assert!(outcome.denied.iter().any(|denial| {
            denial.relay_id == "cooling" && denial.reason == RelayAuthorizationDenial::Cooldown
        }));
        assert!(outcome.denied.iter().any(|denial| {
            denial.relay_id == "loop" && denial.reason == RelayAuthorizationDenial::LoopDetected
        }));

        let no_fallback = select_relay_candidate(
            &[
                candidate("first", 20, None, false),
                candidate("second", 10, None, false),
            ],
            &RelaySelectionPolicy {
                max_attempts: 2,
                allow_fallback: false,
                ..RelaySelectionPolicy::default()
            },
            now(),
        )
        .unwrap();
        assert_eq!(no_fallback.attempts_allowed, 1);
        assert_eq!(no_fallback.candidates.len(), 1);
        assert_eq!(no_fallback.selected.unwrap().relay_id, "second");
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
