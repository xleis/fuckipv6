//! Production traffic-policy runtime bridge, packet evaluation and observation.
//!
//! This module connects [`TrafficPolicyTransactionStore`] to the live
//! [`NetworkRuntimeManager`] and `ClientRuntime` TUN packet path without
//! claiming capabilities the host data plane does not have:
//!
//! - Destination CIDR, destination port ranges, protocols (TCP/UDP/ICMP),
//!   ordered first-match rules, default policy, failure policy and Exit/Network/
//!   Direct/Reject actions are enforced on live TUN packets.
//! - Application-identity matchers fail closed via
//!   [`ApplicationIdentityCapability::unsupported`] until a real process
//!   identity capture adapter is wired.
//! - Domain matchers (`exact_domains` / `domain_suffixes`) fail closed until a
//!   live DNS IP-to-domain observation table is wired across routing modes.
//! - Flow observation reports `TrafficRoutingObservation::Unavailable` unless
//!   a live network runtime session is actually observing packets.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::{SystemTime, UNIX_EPOCH};

use fuckipv6_common::domain::{
    ApplicationIdentity, ApplicationIdentityCapability, ApplicationIdentityPlatform,
    CapabilityKind, ContentHash, DomainError, NetworkId, NodeId, TrafficAction, TrafficDecision,
    TrafficDecisionReason, TrafficDefaultConfirmedBy, TrafficDefaultPolicy, TrafficFailurePolicy,
    TrafficFlow, TrafficPolicy, TrafficProtocol, TrafficRoutingHardConstraints,
    TrafficRoutingObservation, TrafficRoutingObservedHit, TrafficRoutingPageModel,
    TrafficRoutingTarget, TrafficRoutingWorkflowState,
};

use crate::node::{NetworkRuntimeManager, NodePermission, NodeRoutingMode, NodeSnapshot};
use crate::traffic_policy_transaction::{
    TrafficPolicyRuntimeAdapter, TrafficPolicyRuntimeFailure, TrafficPolicyRuntimePlan,
    TrafficPolicyTransactionError, TrafficPolicyValidationContext,
};
use crate::tun::TunCapabilityReason;

pub(crate) const MAX_OBSERVED_HITS: usize = 64;
pub(crate) const OBSERVATION_UNAVAILABLE_REASON: &str = "runtime.traffic_observation_unavailable";

/// Bounded per-session collector of real observed TUN packets evaluated against
/// the installed [`TrafficPolicy`].
#[derive(Debug, Default)]
pub(crate) struct TrafficPolicyObservationCollector {
    window_start_unix_ms: Option<u64>,
    window_end_unix_ms: u64,
    observed_flow_count: usize,
    sequence: u64,
    hits: VecDeque<TrafficRoutingObservedHit>,
}

impl TrafficPolicyObservationCollector {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Start or reset an active observation window when a policy becomes live
    /// on a running TUN session.
    pub(crate) fn activate(&mut self, now_ms: u64) {
        let now_ms = now_ms.max(1);
        self.window_start_unix_ms = Some(now_ms);
        self.window_end_unix_ms = now_ms;
        self.observed_flow_count = 0;
        self.hits.clear();
    }

    /// Record one real packet flow observed by the live TUN loop.
    pub(crate) fn record_flow(&mut self, now_ms: u64, flow: TrafficFlow) {
        let now_ms = now_ms.max(1);
        let start = *self.window_start_unix_ms.get_or_insert(now_ms);
        self.window_end_unix_ms = self.window_end_unix_ms.max(now_ms).max(start);
        self.observed_flow_count = self.observed_flow_count.saturating_add(1);
        self.sequence = self.sequence.saturating_add(1);
        if self.hits.len() >= MAX_OBSERVED_HITS {
            self.hits.pop_front();
        }
        self.hits.push_back(TrafficRoutingObservedHit {
            sample_id: format!("flow-{now_ms}-{:08x}", self.sequence),
            activity_id: None,
            diagnostic_id: None,
            flow,
        });
    }

    pub(crate) fn snapshot(&self, session_active: bool) -> TrafficRoutingObservation {
        if !session_active {
            return TrafficRoutingObservation::unavailable(OBSERVATION_UNAVAILABLE_REASON);
        }
        let Some(window_start_unix_ms) = self.window_start_unix_ms else {
            return TrafficRoutingObservation::unavailable(OBSERVATION_UNAVAILABLE_REASON);
        };
        let now_ms = unix_time_ms().max(window_start_unix_ms);
        TrafficRoutingObservation::Available {
            window_start_unix_ms,
            window_end_unix_ms: self.window_end_unix_ms.max(now_ms),
            observed_flow_count: self.observed_flow_count,
            hits: self.hits.iter().cloned().collect(),
        }
    }
}

/// Merge observations from multiple live network sessions without inventing
/// data when no session is actively observing traffic.
pub(crate) fn merge_observations(
    observations: impl IntoIterator<Item = TrafficRoutingObservation>,
) -> TrafficRoutingObservation {
    let mut window_start: Option<u64> = None;
    let mut window_end: u64 = 0;
    let mut observed_flow_count: usize = 0;
    let mut hits: Vec<TrafficRoutingObservedHit> = Vec::new();
    let mut seen_ids = BTreeSet::new();

    for observation in observations {
        if let TrafficRoutingObservation::Available {
            window_start_unix_ms,
            window_end_unix_ms,
            observed_flow_count: count,
            hits: session_hits,
        } = observation
        {
            window_start = Some(match window_start {
                Some(existing) => existing.min(window_start_unix_ms),
                None => window_start_unix_ms,
            });
            window_end = window_end.max(window_end_unix_ms);
            observed_flow_count = observed_flow_count.saturating_add(count);
            for mut hit in session_hits {
                if !seen_ids.insert(hit.sample_id.clone()) {
                    hit.sample_id = format!("{}-{}", hit.sample_id, seen_ids.len());
                    seen_ids.insert(hit.sample_id.clone());
                }
                hits.push(hit);
            }
        }
    }

    let Some(window_start_unix_ms) = window_start else {
        return TrafficRoutingObservation::unavailable(OBSERVATION_UNAVAILABLE_REASON);
    };
    if hits.len() > MAX_OBSERVED_HITS {
        let drop_count = hits.len() - MAX_OBSERVED_HITS;
        hits.drain(..drop_count);
    }
    TrafficRoutingObservation::Available {
        window_start_unix_ms,
        window_end_unix_ms: window_end.max(window_start_unix_ms),
        observed_flow_count: observed_flow_count.max(hits.len()),
        hits,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedTunFlow {
    pub(crate) flow: TrafficFlow,
    pub(crate) has_transport_port: bool,
}

/// Parse a real IPv4/IPv6 packet from the TUN device into a domain
/// [`TrafficFlow`]. Returns `None` for truncated headers, unsupported IP
/// versions, unsupported transport protocols, or zero TCP/UDP destination ports.
pub(crate) fn parse_tun_packet_flow(packet: &[u8]) -> Option<ParsedTunFlow> {
    if packet.is_empty() {
        return None;
    }
    match packet[0] >> 4 {
        4 => {
            if packet.len() < 20 {
                return None;
            }
            let ihl = usize::from(packet[0] & 0x0f) * 4;
            if ihl < 20 || packet.len() < ihl {
                return None;
            }
            let dst_bytes: [u8; 4] = packet[16..20].try_into().ok()?;
            let destination_ip = IpAddr::V4(Ipv4Addr::from(dst_bytes));
            let payload = &packet[ihl..];
            parse_transport(destination_ip, packet[9], payload, false)
        }
        6 => {
            if packet.len() < 40 {
                return None;
            }
            let dst_bytes: [u8; 16] = packet[24..40].try_into().ok()?;
            let destination_ip = IpAddr::V6(Ipv6Addr::from(dst_bytes));
            let payload = &packet[40..];
            parse_transport(destination_ip, packet[6], payload, true)
        }
        _ => None,
    }
}

fn parse_transport(
    destination_ip: IpAddr,
    protocol_number: u8,
    payload: &[u8],
    is_ipv6: bool,
) -> Option<ParsedTunFlow> {
    match protocol_number {
        6 => {
            if payload.len() < 4 {
                return None;
            }
            let destination_port = u16::from_be_bytes([payload[2], payload[3]]);
            if destination_port == 0 {
                return None;
            }
            Some(ParsedTunFlow {
                flow: TrafficFlow {
                    domain: None,
                    destination_ip,
                    destination_port,
                    protocol: TrafficProtocol::Tcp,
                    application: ApplicationIdentity::Unknown,
                },
                has_transport_port: true,
            })
        }
        17 => {
            if payload.len() < 4 {
                return None;
            }
            let destination_port = u16::from_be_bytes([payload[2], payload[3]]);
            if destination_port == 0 {
                return None;
            }
            Some(ParsedTunFlow {
                flow: TrafficFlow {
                    domain: None,
                    destination_ip,
                    destination_port,
                    protocol: TrafficProtocol::Udp,
                    application: ApplicationIdentity::Unknown,
                },
                has_transport_port: true,
            })
        }
        1 if !is_ipv6 => Some(ParsedTunFlow {
            flow: TrafficFlow {
                domain: None,
                destination_ip,
                destination_port: 1,
                protocol: TrafficProtocol::Icmp,
                application: ApplicationIdentity::Unknown,
            },
            has_transport_port: false,
        }),
        58 if is_ipv6 => Some(ParsedTunFlow {
            flow: TrafficFlow {
                domain: None,
                destination_ip,
                destination_port: 1,
                protocol: TrafficProtocol::Icmp,
                application: ApplicationIdentity::Unknown,
            },
            has_transport_port: false,
        }),
        _ => None,
    }
}

/// Evaluate one live TUN packet against an applied [`TrafficPolicy`] using
/// exact ordered first-match semantics.
///
/// For ICMP/ICMPv6 packets (which carry no L4 port), rules specifying
/// `destination_ports` are skipped instead of matching the synthetic sentinel
/// port `1`.
pub(crate) fn evaluate_tun_packet(
    policy: &TrafficPolicy,
    packet: &[u8],
) -> Option<(TrafficFlow, TrafficDecision)> {
    let parsed = parse_tun_packet_flow(packet)?;
    let decision = if parsed.has_transport_port {
        policy.evaluate(&parsed.flow).ok()?
    } else {
        let mut matched = None;
        for rule in policy.rules.iter().filter(|rule| rule.enabled) {
            if !rule.matcher.destination_ports.is_empty() {
                continue;
            }
            if rule.matcher.matches(&parsed.flow) {
                matched = Some(TrafficDecision {
                    action: rule.action.clone(),
                    matched_rule: Some(rule.rule_id.clone()),
                    reason: TrafficDecisionReason::FirstMatch,
                });
                break;
            }
        }
        matched.unwrap_or_else(|| TrafficDecision {
            action: policy.default_policy.action.clone(),
            matched_rule: None,
            reason: TrafficDecisionReason::Default,
        })
    };
    Some((parsed.flow, decision))
}

pub(crate) const fn current_application_identity_platform() -> ApplicationIdentityPlatform {
    #[cfg(target_os = "macos")]
    {
        ApplicationIdentityPlatform::MacOs
    }
    #[cfg(target_os = "windows")]
    {
        ApplicationIdentityPlatform::Windows
    }
    #[cfg(target_os = "android")]
    {
        ApplicationIdentityPlatform::Android
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "android")))]
    {
        ApplicationIdentityPlatform::Linux
    }
}

pub(crate) fn unsupported_application_identity_capability() -> ApplicationIdentityCapability {
    ApplicationIdentityCapability::unsupported(
        current_application_identity_platform(),
        "当前节点运行时尚未接入进程级应用身份捕获适配器，不支持应用条件匹配",
    )
    .expect("static unsupported explanation is valid")
}

pub(crate) fn policy_has_domain_matchers(policy: &TrafficPolicy) -> bool {
    policy.rules.iter().any(|rule| {
        !rule.matcher.exact_domains.is_empty() || !rule.matcher.domain_suffixes.is_empty()
    })
}

pub(crate) fn validate_supported_runtime_matchers(
    policy: &TrafficPolicy,
) -> Result<(), TrafficPolicyTransactionError> {
    if policy_has_domain_matchers(policy) {
        return Err(TrafficPolicyTransactionError::CapabilityUnavailable(
            "当前运行时尚未启用跨模式实时 DNS 域名到 IP 映射观察器，不支持应用域名匹配规则；请使用目标 CIDR、端口或协议规则".into(),
        ));
    }
    Ok(())
}

pub(crate) fn fallback_traffic_policy(
    snapshot: &NodeSnapshot,
) -> Result<TrafficPolicy, TrafficPolicyTransactionError> {
    let node_id = snapshot_node_id(snapshot)?;
    Ok(TrafficPolicy {
        node_id,
        revision: 1,
        default_policy: TrafficDefaultPolicy {
            action: TrafficAction::Direct,
            confirmed_by: TrafficDefaultConfirmedBy::Owner,
            confirmed_at_unix_ms: 1,
        },
        failure_policy: TrafficFailurePolicy::fail_closed(),
        rules: Vec::new(),
    })
}

pub(crate) fn validation_context_from_snapshot(
    snapshot: &NodeSnapshot,
) -> Result<TrafficPolicyValidationContext, TrafficPolicyTransactionError> {
    let node_id = snapshot_node_id(snapshot)?;
    let (known_network_ids, known_exit_node_ids) = authoritative_targets(snapshot, &node_id);
    let blocking_reason = snapshot.tun_capabilities.blocking_reason();
    let tun_available = blocking_reason.is_none();
    let tun_unavailable_reason = blocking_reason
        .map(describe_tun_blocking_reason)
        .unwrap_or_default()
        .to_string();

    Ok(TrafficPolicyValidationContext {
        node_id,
        known_network_ids: known_network_ids.into_keys().collect(),
        known_exit_node_ids: known_exit_node_ids.into_keys().collect(),
        tun_available,
        tun_unavailable_reason,
        application_identity: unsupported_application_identity_capability(),
    })
}

pub(crate) fn build_traffic_routing_page_model(
    snapshot: &NodeSnapshot,
    policy: &TrafficPolicy,
    workflow: TrafficRoutingWorkflowState,
    observation: TrafficRoutingObservation,
    embedded: bool,
) -> Result<TrafficRoutingPageModel, DomainError> {
    let (known_networks, known_exits) = authoritative_targets(snapshot, &policy.node_id);
    let mut targets = Vec::with_capacity(2 + known_networks.len() + known_exits.len());
    targets.push(TrafficRoutingTarget::direct());
    targets.push(TrafficRoutingTarget::reject());
    for (network_id, network_name) in known_networks {
        let label = sanitize_target_label(&network_name, network_id.as_str());
        targets.push(TrafficRoutingTarget::network(network_id, label));
    }
    for (exit_id, label) in known_exits {
        let label = sanitize_target_label(&label, exit_id.as_str());
        targets.push(TrafficRoutingTarget::exit(exit_id, label));
    }

    // Ensure deduplication by `(kind, id)` even if snapshot collections overlap.
    let mut seen = BTreeSet::new();
    targets.retain(|target| seen.insert((target.kind, target.id.clone())));

    let hard_constraints = hard_constraints_from_snapshot(snapshot);
    let can_edit = !embedded
        && snapshot
            .permissions
            .permissions
            .contains(&NodePermission::ChangeRouting);

    TrafficRoutingPageModel::from_policy_with_observation(
        policy,
        workflow,
        hard_constraints,
        targets,
        can_edit,
        observation,
    )
}

fn snapshot_node_id(snapshot: &NodeSnapshot) -> Result<NodeId, TrafficPolicyTransactionError> {
    let raw = snapshot
        .identity
        .as_ref()
        .map(|identity| identity.node_id.as_str())
        .ok_or_else(|| {
            TrafficPolicyTransactionError::CapabilityUnavailable(
                "节点身份尚未初始化，无法建立流量策略权威视图".into(),
            )
        })?;
    NodeId::parse(raw).map_err(|error| {
        TrafficPolicyTransactionError::InvalidCandidate(format!("节点身份标识无效: {error}"))
    })
}

fn authoritative_targets(
    snapshot: &NodeSnapshot,
    self_node_id: &NodeId,
) -> (BTreeMap<NetworkId, String>, BTreeMap<NodeId, String>) {
    let mut networks = BTreeMap::new();
    let mut exits = BTreeMap::new();

    for membership in &snapshot.memberships {
        if !membership.state.can_access_network_resources() {
            continue;
        }
        if let Ok(network_id) = NetworkId::parse(membership.network_id.as_str()) {
            networks.insert(network_id, membership.network_name.clone());
        }
    }
    if let Some(topology) = snapshot.topology.as_ref() {
        for boundary in &topology.boundaries {
            networks
                .entry(boundary.network_id.clone())
                .or_insert_with(|| boundary.name.clone());
        }
        for node in &topology.nodes {
            if node.is_self || &node.node_id == self_node_id {
                continue;
            }
            let has_exit = node.capabilities.contains(&CapabilityKind::Exit)
                || node
                    .networks
                    .iter()
                    .any(|membership| membership.capabilities.contains(&CapabilityKind::Exit));
            if has_exit {
                let label = node
                    .display_name
                    .clone()
                    .unwrap_or_else(|| node.node_id.to_string());
                exits.insert(node.node_id.clone(), label);
            }
        }
    }
    for selection in &snapshot.exit_selections {
        if let Some(resolution) = selection.resolution.as_ref() {
            for exit_id in &resolution.selectable_exits {
                if exit_id != self_node_id {
                    exits
                        .entry(exit_id.clone())
                        .or_insert_with(|| exit_id.to_string());
                }
            }
            if let Some(fixed_exit) = resolution.fixed_exit.as_ref() {
                if fixed_exit != self_node_id {
                    exits
                        .entry(fixed_exit.clone())
                        .or_insert_with(|| fixed_exit.to_string());
                }
            }
        }
    }

    (networks, exits)
}

fn hard_constraints_from_snapshot(snapshot: &NodeSnapshot) -> TrafficRoutingHardConstraints {
    let mut fixed_exits = BTreeSet::new();
    for selection in &snapshot.exit_selections {
        if let Some(fixed_exit) = selection
            .resolution
            .as_ref()
            .and_then(|resolution| resolution.fixed_exit.clone())
        {
            fixed_exits.insert(fixed_exit);
        }
    }
    if fixed_exits.len() == 1 {
        return TrafficRoutingHardConstraints {
            fixed_exit: fixed_exits.into_iter().next(),
            fixed_network: None,
        };
    }

    let mut configured_exits = BTreeSet::new();
    for network in &snapshot.configuration.networks {
        if network.routing_mode == NodeRoutingMode::MeshOnly {
            continue;
        }
        if let Some(raw_exit) = network.exit_node.as_deref() {
            if let Ok(exit_id) = NodeId::parse(raw_exit) {
                configured_exits.insert(exit_id);
            }
        }
    }
    if configured_exits.len() == 1 {
        return TrafficRoutingHardConstraints {
            fixed_exit: configured_exits.into_iter().next(),
            fixed_network: None,
        };
    }

    TrafficRoutingHardConstraints::default()
}

fn sanitize_target_label(raw: &str, fallback: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

fn describe_tun_blocking_reason(reason: TunCapabilityReason) -> &'static str {
    match reason {
        TunCapabilityReason::PlatformUnsupported => "当前操作系统平台不支持生产 TUN 数据面",
        TunCapabilityReason::PermissionDenied => "当前进程缺少创建和配置 TUN 数据面所需的系统权限",
        TunCapabilityReason::DeviceUnavailable => {
            "系统 TUN 设备节点不可用（例如 /dev/net/tun 缺失）"
        }
        TunCapabilityReason::ToolUnavailable => "缺少配置 TUN 与策略路由所需的系统网络工具",
        TunCapabilityReason::Unknown => "当前节点 TUN 数据面未就绪",
    }
}

/// Production runtime adapter bridging synchronous transaction steps into the
/// live [`NetworkRuntimeManager`] actor and its running `ClientRuntime` sessions.
pub(crate) struct ProductionTrafficPolicyRuntimeAdapter {
    runtime: NetworkRuntimeManager,
    handle: tokio::runtime::Handle,
}

impl ProductionTrafficPolicyRuntimeAdapter {
    pub(crate) fn new(runtime: NetworkRuntimeManager, handle: tokio::runtime::Handle) -> Self {
        Self { runtime, handle }
    }
}

impl TrafficPolicyRuntimeAdapter for ProductionTrafficPolicyRuntimeAdapter {
    fn apply(&self, plan: &TrafficPolicyRuntimePlan) -> Result<(), TrafficPolicyRuntimeFailure> {
        if policy_has_domain_matchers(&plan.candidate) {
            return Err(TrafficPolicyRuntimeFailure::new(
                "runtime_apply",
                "domain_matcher_unavailable",
                "当前运行时尚未启用跨模式实时 DNS 域名到 IP 映射观察器，无法应用域名匹配规则",
            ));
        }
        self.handle
            .block_on(self.runtime.apply_traffic_policy(plan.candidate.clone()))
            .map_err(|error| {
                TrafficPolicyRuntimeFailure::new(
                    "runtime_apply",
                    error.message_key(),
                    error.to_string(),
                )
            })
    }

    fn verify(&self, plan: &TrafficPolicyRuntimePlan) -> Result<(), TrafficPolicyRuntimeFailure> {
        let expected_hash: ContentHash = plan.candidate.content_hash().map_err(|error| {
            TrafficPolicyRuntimeFailure::new(
                "runtime_verify",
                "invalid_candidate",
                error.to_string(),
            )
        })?;
        self.handle
            .block_on(
                self.runtime
                    .verify_traffic_policy(plan.candidate.revision, expected_hash),
            )
            .map_err(|error| {
                TrafficPolicyRuntimeFailure::new(
                    "runtime_verify",
                    error.message_key(),
                    error.to_string(),
                )
            })
    }

    fn restore(&self, previous: &TrafficPolicy) -> Result<(), TrafficPolicyRuntimeFailure> {
        self.handle
            .block_on(self.runtime.apply_traffic_policy(previous.clone()))
            .map_err(|error| {
                TrafficPolicyRuntimeFailure::new(
                    "runtime_restore",
                    error.message_key(),
                    error.to_string(),
                )
            })
    }
}

pub(crate) fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

/// Ensure any target kind/id mapping for [`TrafficRoutingTargetKind`] stays
/// aligned with domain serialization in tests.
#[cfg(test)]
mod tests {
    use super::*;
    use fuckipv6_common::domain::{
        DomainSuffix, IpCidr, PortRange, TrafficMatch, TrafficRoutingTargetKind, TrafficRule,
        TrafficRuleId,
    };

    fn test_node_id(byte: u8) -> NodeId {
        NodeId::from_public_key(&[byte; 32])
    }

    fn ipv4_tcp_packet(dst: Ipv4Addr, dst_port: u16) -> Vec<u8> {
        let mut packet = vec![0u8; 24];
        packet[0] = 0x45;
        packet[9] = 6;
        packet[12..16].copy_from_slice(&[10, 8, 0, 2]);
        packet[16..20].copy_from_slice(&dst.octets());
        packet[22..24].copy_from_slice(&dst_port.to_be_bytes());
        packet
    }

    fn ipv4_icmp_packet(dst: Ipv4Addr) -> Vec<u8> {
        let mut packet = vec![0u8; 28];
        packet[0] = 0x45;
        packet[9] = 1;
        packet[12..16].copy_from_slice(&[10, 8, 0, 2]);
        packet[16..20].copy_from_slice(&dst.octets());
        packet
    }

    fn ipv6_udp_packet(dst: Ipv6Addr, dst_port: u16) -> Vec<u8> {
        let mut packet = vec![0u8; 48];
        packet[0] = 0x60;
        packet[6] = 17;
        packet[24..40].copy_from_slice(&dst.octets());
        packet[42..44].copy_from_slice(&dst_port.to_be_bytes());
        packet
    }

    #[test]
    fn evaluates_ipv4_ipv6_cidr_port_protocol_first_match_and_icmp_port_exclusion() {
        let self_id = test_node_id(1);
        let exit_id = test_node_id(2);
        let rule_block_https = TrafficRuleId::from_public_key(b"rule-block-https");
        let rule_port_one = TrafficRuleId::from_public_key(b"rule-port-one");
        let rule_exit_v6_quic = TrafficRuleId::from_public_key(b"rule-exit-v6-quic");
        let policy = TrafficPolicy {
            node_id: self_id,
            revision: 2,
            default_policy: TrafficDefaultPolicy {
                action: TrafficAction::Direct,
                confirmed_by: TrafficDefaultConfirmedBy::Owner,
                confirmed_at_unix_ms: 100,
            },
            failure_policy: TrafficFailurePolicy::fail_closed(),
            rules: vec![
                TrafficRule {
                    rule_id: rule_block_https.clone(),
                    priority: 1,
                    enabled: true,
                    matcher: TrafficMatch {
                        destination_cidrs: vec![IpCidr::parse("203.0.113.0/24").expect("cidr")],
                        destination_ports: vec![PortRange {
                            start: 443,
                            end: 443,
                        }],
                        protocols: vec![TrafficProtocol::Tcp],
                        ..TrafficMatch::default()
                    },
                    action: TrafficAction::Reject,
                    description: "reject https".into(),
                },
                TrafficRule {
                    rule_id: rule_port_one,
                    priority: 2,
                    enabled: true,
                    matcher: TrafficMatch {
                        destination_ports: vec![PortRange { start: 1, end: 1 }],
                        ..TrafficMatch::default()
                    },
                    action: TrafficAction::Reject,
                    description: "port 1 only".into(),
                },
                TrafficRule {
                    rule_id: rule_exit_v6_quic.clone(),
                    priority: 3,
                    enabled: true,
                    matcher: TrafficMatch {
                        destination_cidrs: vec![
                            IpCidr::parse("2606:4700:4700::/48").expect("v6 cidr")
                        ],
                        destination_ports: vec![PortRange {
                            start: 443,
                            end: 443,
                        }],
                        protocols: vec![TrafficProtocol::Udp],
                        ..TrafficMatch::default()
                    },
                    action: TrafficAction::Exit(exit_id.clone()),
                    description: "quic via exit".into(),
                },
            ],
        };

        let (_, tcp_hit) = evaluate_tun_packet(
            &policy,
            &ipv4_tcp_packet("203.0.113.9".parse().unwrap(), 443),
        )
        .expect("tcp flow");
        assert_eq!(tcp_hit.action, TrafficAction::Reject);
        assert_eq!(tcp_hit.matched_rule.as_ref(), Some(&rule_block_https));

        // ICMP has no L4 port and must NOT falsely match the port 1 rule.
        let (_, icmp_decision) =
            evaluate_tun_packet(&policy, &ipv4_icmp_packet("203.0.113.9".parse().unwrap()))
                .expect("icmp flow");
        assert_eq!(icmp_decision.action, TrafficAction::Direct);
        assert_eq!(icmp_decision.matched_rule, None);

        let (_, v6_hit) = evaluate_tun_packet(
            &policy,
            &ipv6_udp_packet("2606:4700:4700::1111".parse().unwrap(), 443),
        )
        .expect("v6 udp flow");
        assert_eq!(v6_hit.action, TrafficAction::Exit(exit_id));
        assert_eq!(v6_hit.matched_rule.as_ref(), Some(&rule_exit_v6_quic));
    }

    #[test]
    fn observation_collector_distinguishes_unavailable_zero_hits_and_bounded_hits() {
        let mut collector = TrafficPolicyObservationCollector::new();
        assert!(matches!(
            collector.snapshot(false),
            TrafficRoutingObservation::Unavailable { .. }
        ));
        assert!(matches!(
            collector.snapshot(true),
            TrafficRoutingObservation::Unavailable { .. }
        ));

        collector.activate(1_000);
        match collector.snapshot(true) {
            TrafficRoutingObservation::Available {
                observed_flow_count,
                hits,
                ..
            } => {
                assert_eq!(observed_flow_count, 0);
                assert!(hits.is_empty());
            }
            other => panic!("expected available zero-hit window, got {other:?}"),
        }

        for index in 0..(MAX_OBSERVED_HITS + 10) {
            collector.record_flow(
                1_000 + index as u64,
                TrafficFlow {
                    domain: None,
                    destination_ip: IpAddr::V4(Ipv4Addr::new(
                        198,
                        51,
                        100,
                        (index % 250 + 1) as u8,
                    )),
                    destination_port: 443,
                    protocol: TrafficProtocol::Tcp,
                    application: ApplicationIdentity::Unknown,
                },
            );
        }

        match collector.snapshot(true) {
            TrafficRoutingObservation::Available {
                observed_flow_count,
                hits,
                ..
            } => {
                assert_eq!(observed_flow_count, MAX_OBSERVED_HITS + 10);
                assert_eq!(hits.len(), MAX_OBSERVED_HITS);
            }
            other => panic!("expected bounded available observation, got {other:?}"),
        }
    }

    #[test]
    fn domain_matchers_fail_closed_when_dns_observer_is_unwired() {
        let policy = TrafficPolicy {
            node_id: test_node_id(3),
            revision: 2,
            default_policy: TrafficDefaultPolicy {
                action: TrafficAction::Direct,
                confirmed_by: TrafficDefaultConfirmedBy::Owner,
                confirmed_at_unix_ms: 100,
            },
            failure_policy: TrafficFailurePolicy::fail_closed(),
            rules: vec![TrafficRule {
                rule_id: TrafficRuleId::from_public_key(b"rule-domain"),
                priority: 1,
                enabled: true,
                matcher: TrafficMatch {
                    domain_suffixes: vec![DomainSuffix::parse("example.com").expect("suffix")],
                    ..TrafficMatch::default()
                },
                action: TrafficAction::Reject,
                description: "domain rule".into(),
            }],
        };

        assert!(policy_has_domain_matchers(&policy));
        let error = validate_supported_runtime_matchers(&policy)
            .expect_err("domain matchers must fail closed without a live DNS observer");
        assert!(matches!(
            error,
            TrafficPolicyTransactionError::CapabilityUnavailable(_)
        ));
    }

    #[test]
    fn traffic_routing_page_projection_keeps_known_targets_and_edit_permission() {
        let self_id = test_node_id(10);
        let mut snapshot = NodeSnapshot {
            identity: Some(crate::node::NodeIdentitySnapshot {
                node_id: self_id.to_string(),
                public_key_fingerprint: "00".repeat(16),
                display_name: Some("Self".into()),
                created_at_unix_ms: Some(1_000),
            }),
            ..NodeSnapshot::default()
        };
        snapshot.permissions.permissions =
            vec![NodePermission::ViewState, NodePermission::ChangeRouting];
        let policy = fallback_traffic_policy(&snapshot).expect("fallback");
        let page = build_traffic_routing_page_model(
            &snapshot,
            &policy,
            TrafficRoutingWorkflowState::Applied { revision: 1 },
            TrafficRoutingObservation::unavailable(OBSERVATION_UNAVAILABLE_REASON),
            false,
        )
        .expect("page model");

        assert_eq!(page.authority_revision, 1);
        assert!(page.can_edit);
        assert!(page
            .targets
            .iter()
            .any(|target| target.kind == TrafficRoutingTargetKind::Direct));
        assert!(page
            .targets
            .iter()
            .any(|target| target.kind == TrafficRoutingTargetKind::Reject));
        assert!(matches!(
            page.observation,
            TrafficRoutingObservation::Unavailable { .. }
        ));

        let embedded_page = build_traffic_routing_page_model(
            &snapshot,
            &policy,
            TrafficRoutingWorkflowState::Applied { revision: 1 },
            TrafficRoutingObservation::unavailable(OBSERVATION_UNAVAILABLE_REASON),
            true,
        )
        .expect("embedded page model");
        assert!(!embedded_page.can_edit);
    }
}
