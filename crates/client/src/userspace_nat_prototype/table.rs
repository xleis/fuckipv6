//! Bounded userspace NAT flow tracker for the NX-16 feasibility prototype.

use super::packet::{self, MalformedPacket, PacketView, TcpFlags, TransportProtocol};
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::net::Ipv4Addr;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Endpoint {
    pub address: Ipv4Addr,
    pub port: u16,
}

impl Endpoint {
    pub const fn new(address: Ipv4Addr, port: u16) -> Self {
        Self { address, port }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct FlowKey {
    pub protocol: TransportProtocol,
    pub source: Endpoint,
    pub destination: Endpoint,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    Outbound,
    Inbound,
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Outbound => f.write_str("outbound"),
            Self::Inbound => f.write_str("inbound"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TcpConnectionState {
    SynSent,
    Established,
    Closing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryBudget {
    pub fixed_bytes: usize,
    pub bytes_per_flow: usize,
    pub max_bytes: usize,
}

impl MemoryBudget {
    pub const DEFAULT_FIXED_BYTES: usize = 64 * 1024;
    pub const DEFAULT_BYTES_PER_FLOW: usize = 512;
    pub const DEFAULT_MAX_BYTES: usize = 8 * 1024 * 1024;

    pub const fn estimated_bytes(self, active_flows: usize) -> usize {
        self.fixed_bytes
            .saturating_add(self.bytes_per_flow.saturating_mul(active_flows))
    }

    pub const fn allows(self, active_flows: usize) -> bool {
        self.estimated_bytes(active_flows) <= self.max_bytes
    }

    fn validate(self) -> Result<(), NatError> {
        if self.fixed_bytes == 0 {
            return Err(NatError::InvalidConfig(
                "memory budget fixed_bytes must be non-zero",
            ));
        }
        if self.bytes_per_flow == 0 {
            return Err(NatError::InvalidConfig(
                "memory budget bytes_per_flow must be non-zero",
            ));
        }
        if self.max_bytes < self.fixed_bytes {
            return Err(NatError::InvalidConfig(
                "memory budget max_bytes is smaller than fixed_bytes",
            ));
        }
        Ok(())
    }
}

impl Default for MemoryBudget {
    fn default() -> Self {
        Self {
            fixed_bytes: Self::DEFAULT_FIXED_BYTES,
            bytes_per_flow: Self::DEFAULT_BYTES_PER_FLOW,
            max_bytes: Self::DEFAULT_MAX_BYTES,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NatConfig {
    pub max_flows: usize,
    pub max_flows_per_source: usize,
    pub external_port_start: u16,
    pub external_port_end: u16,
    pub tcp_syn_timeout_ms: u64,
    pub tcp_established_timeout_ms: u64,
    pub tcp_closing_timeout_ms: u64,
    pub udp_timeout_ms: u64,
    pub memory_budget: MemoryBudget,
}

impl Default for NatConfig {
    fn default() -> Self {
        Self {
            max_flows: 4096,
            max_flows_per_source: 256,
            external_port_start: 40_000,
            external_port_end: 60_999,
            tcp_syn_timeout_ms: 60_000,
            tcp_established_timeout_ms: 300_000,
            tcp_closing_timeout_ms: 30_000,
            udp_timeout_ms: 60_000,
            memory_budget: MemoryBudget::default(),
        }
    }
}

impl NatConfig {
    pub fn validate(self) -> Result<(), NatError> {
        if self.max_flows == 0 {
            return Err(NatError::InvalidConfig("max_flows must be non-zero"));
        }
        if self.max_flows_per_source == 0 {
            return Err(NatError::InvalidConfig(
                "max_flows_per_source must be non-zero",
            ));
        }
        if self.external_port_start == 0 || self.external_port_end == 0 {
            return Err(NatError::InvalidConfig(
                "external port range must not contain port zero",
            ));
        }
        if self.external_port_start > self.external_port_end {
            return Err(NatError::InvalidConfig(
                "external_port_start must not exceed external_port_end",
            ));
        }
        if self.tcp_syn_timeout_ms == 0
            || self.tcp_established_timeout_ms == 0
            || self.tcp_closing_timeout_ms == 0
            || self.udp_timeout_ms == 0
        {
            return Err(NatError::InvalidConfig(
                "all flow timeouts must be non-zero",
            ));
        }
        self.memory_budget.validate()?;
        if !self.memory_budget.allows(self.max_flows) {
            return Err(NatError::InvalidConfig(
                "max_flows exceeds the configured memory budget",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NatDecision {
    pub flow: FlowKey,
    pub external_port: u16,
    pub direction: Direction,
    pub tcp_state: Option<TcpConnectionState>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FlowSnapshot {
    pub flow: FlowKey,
    pub external_port: u16,
    pub tcp_state: Option<TcpConnectionState>,
    pub last_seen_ms: u64,
    pub expires_at_ms: u64,
    pub outbound_packets: u64,
    pub inbound_packets: u64,
    pub outbound_transport_bytes: u64,
    pub inbound_transport_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NatStats {
    /// Packets accepted from the internal side, including SYN retransmissions.
    pub outbound_packets: u64,
    /// Packets accepted from the external side for a known mapping.
    pub inbound_packets: u64,
    pub outbound_transport_bytes: u64,
    pub inbound_transport_bytes: u64,
    pub flows_created: u64,
    pub flows_expired: u64,
    pub flows_reset: u64,
    pub malformed_packets: u64,
    pub rejected_packets: u64,
    pub capacity_rejections: u64,
    pub source_limit_rejections: u64,
    pub port_exhaustions: u64,
    pub orphan_inbound: u64,
    pub invalid_tcp_transitions: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NatError {
    InvalidConfig(&'static str),
    ClockWentBackwards {
        last_ms: u64,
        now_ms: u64,
    },
    MalformedPacket(MalformedPacket),
    CapacityExceeded {
        active: usize,
        max: usize,
    },
    SourceFlowLimit {
        source: Ipv4Addr,
        active: usize,
        max: usize,
    },
    PortExhausted {
        first: u16,
        last: u16,
    },
    UnknownInbound,
    InvalidTcpTransition {
        direction: Direction,
        state: TcpConnectionState,
        flags: TcpFlags,
    },
}

impl fmt::Display for NatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(message) => write!(f, "invalid NAT configuration: {message}"),
            Self::ClockWentBackwards { last_ms, now_ms } => write!(
                f,
                "NAT clock moved backwards from {last_ms} ms to {now_ms} ms"
            ),
            Self::MalformedPacket(error) => write!(f, "malformed packet: {error}"),
            Self::CapacityExceeded { active, max } => {
                write!(f, "NAT flow capacity reached: {active}/{max}")
            }
            Self::SourceFlowLimit {
                source,
                active,
                max,
            } => write!(f, "source flow limit reached for {source}: {active}/{max}"),
            Self::PortExhausted { first, last } => {
                write!(f, "external ports exhausted in range {first}..={last}")
            }
            Self::UnknownInbound => f.write_str("inbound packet has no tracked mapping"),
            Self::InvalidTcpTransition {
                direction,
                state,
                flags,
            } => write!(
                f,
                "invalid {direction} TCP transition from {state:?} with flags 0x{:02x}",
                flags.bits()
            ),
        }
    }
}

impl std::error::Error for NatError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::MalformedPacket(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct InboundKey {
    protocol: TransportProtocol,
    remote: Endpoint,
    external_port: u16,
}

#[derive(Clone, Copy, Debug)]
struct FlowRecord {
    flow: FlowKey,
    external_port: u16,
    tcp_state: Option<TcpConnectionState>,
    last_seen_ms: u64,
    expires_at_ms: u64,
    outbound_packets: u64,
    inbound_packets: u64,
    outbound_transport_bytes: u64,
    inbound_transport_bytes: u64,
}

impl FlowRecord {
    fn snapshot(self) -> FlowSnapshot {
        FlowSnapshot {
            flow: self.flow,
            external_port: self.external_port,
            tcp_state: self.tcp_state,
            last_seen_ms: self.last_seen_ms,
            expires_at_ms: self.expires_at_ms,
            outbound_packets: self.outbound_packets,
            inbound_packets: self.inbound_packets,
            outbound_transport_bytes: self.outbound_transport_bytes,
            inbound_transport_bytes: self.inbound_transport_bytes,
        }
    }
}

#[derive(Debug)]
struct PortAllocator {
    first: u16,
    last: u16,
    cursor: u16,
    used: BTreeSet<u16>,
}

impl PortAllocator {
    fn new(first: u16, last: u16) -> Self {
        Self {
            first,
            last,
            cursor: first,
            used: BTreeSet::new(),
        }
    }

    fn allocate(&mut self) -> Result<u16, NatError> {
        let port_count = usize::from(self.last - self.first) + 1;
        for _ in 0..port_count {
            let candidate = self.cursor;
            self.cursor = if candidate == self.last {
                self.first
            } else {
                candidate + 1
            };
            if self.used.insert(candidate) {
                return Ok(candidate);
            }
        }
        Err(NatError::PortExhausted {
            first: self.first,
            last: self.last,
        })
    }

    fn release(&mut self, port: u16) {
        if self.used.remove(&port) {
            // Expired mappings are preferred for reuse before advancing the cursor.
            self.cursor = port;
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RemovalReason {
    Expired,
    Reset,
}

#[derive(Debug)]
pub struct NatTable {
    config: NatConfig,
    flows: BTreeMap<FlowKey, FlowRecord>,
    inbound_index: BTreeMap<InboundKey, FlowKey>,
    expiration_index: BTreeSet<(u64, FlowKey)>,
    flows_per_source: BTreeMap<Ipv4Addr, usize>,
    ports: PortAllocator,
    stats: NatStats,
    last_now_ms: Option<u64>,
}

impl NatTable {
    pub fn new(config: NatConfig) -> Result<Self, NatError> {
        config.validate()?;
        Ok(Self {
            config,
            flows: BTreeMap::new(),
            inbound_index: BTreeMap::new(),
            expiration_index: BTreeSet::new(),
            flows_per_source: BTreeMap::new(),
            ports: PortAllocator::new(config.external_port_start, config.external_port_end),
            stats: NatStats::default(),
            last_now_ms: None,
        })
    }

    pub const fn config(&self) -> NatConfig {
        self.config
    }

    pub const fn stats(&self) -> NatStats {
        self.stats
    }

    pub fn active_flows(&self) -> usize {
        self.flows.len()
    }

    pub fn estimated_memory_bytes(&self) -> usize {
        self.config.memory_budget.estimated_bytes(self.flows.len())
    }

    /// Processes an outbound packet and creates a mapping only when doing so is safe.
    pub fn process_outbound(
        &mut self,
        packet: &[u8],
        now_ms: u64,
    ) -> Result<NatDecision, NatError> {
        self.observe_time(now_ms)?;
        self.expire_trusted(now_ms);

        let view = match packet::parse_ipv4_packet(packet) {
            Ok(view) => view,
            Err(error) => return Err(self.reject_malformed(error)),
        };
        let key = flow_key(&view);

        if self.flows.contains_key(&key) {
            return self.process_existing(key, &view, Direction::Outbound, now_ms);
        }

        self.create_outbound_flow(key, &view, now_ms)
    }

    /// Processes an inbound packet only if it belongs to an established mapping.
    pub fn process_inbound(&mut self, packet: &[u8], now_ms: u64) -> Result<NatDecision, NatError> {
        self.observe_time(now_ms)?;
        self.expire_trusted(now_ms);

        let view = match packet::parse_ipv4_packet(packet) {
            Ok(view) => view,
            Err(error) => return Err(self.reject_malformed(error)),
        };
        let inbound_key = InboundKey {
            protocol: view.protocol,
            remote: Endpoint::new(view.source, view.source_port),
            external_port: view.destination_port,
        };
        let Some(&key) = self.inbound_index.get(&inbound_key) else {
            self.stats.rejected_packets = self.stats.rejected_packets.saturating_add(1);
            self.stats.orphan_inbound = self.stats.orphan_inbound.saturating_add(1);
            return Err(NatError::UnknownInbound);
        };

        self.process_existing(key, &view, Direction::Inbound, now_ms)
    }

    /// Removes all mappings whose timeout is at or before `now_ms`.
    pub fn expire(&mut self, now_ms: u64) -> Result<usize, NatError> {
        self.observe_time(now_ms)?;
        Ok(self.expire_trusted(now_ms))
    }

    pub fn flow_snapshots(&self) -> Vec<FlowSnapshot> {
        self.flows
            .values()
            .copied()
            .map(FlowRecord::snapshot)
            .collect()
    }

    fn create_outbound_flow(
        &mut self,
        flow: FlowKey,
        view: &PacketView,
        now_ms: u64,
    ) -> Result<NatDecision, NatError> {
        if view.protocol == TransportProtocol::Tcp
            && (!view.tcp_flags.syn()
                || view.tcp_flags.ack()
                || view.tcp_flags.fin()
                || view.tcp_flags.rst())
        {
            self.stats.rejected_packets = self.stats.rejected_packets.saturating_add(1);
            self.stats.invalid_tcp_transitions =
                self.stats.invalid_tcp_transitions.saturating_add(1);
            return Err(NatError::InvalidTcpTransition {
                direction: Direction::Outbound,
                state: TcpConnectionState::SynSent,
                flags: view.tcp_flags,
            });
        }

        if self.flows.len() >= self.config.max_flows
            || !self.config.memory_budget.allows(self.flows.len() + 1)
        {
            self.stats.rejected_packets = self.stats.rejected_packets.saturating_add(1);
            self.stats.capacity_rejections = self.stats.capacity_rejections.saturating_add(1);
            return Err(NatError::CapacityExceeded {
                active: self.flows.len(),
                max: self.config.max_flows,
            });
        }

        let source_active = self
            .flows_per_source
            .get(&flow.source.address)
            .copied()
            .unwrap_or(0);
        if source_active >= self.config.max_flows_per_source {
            self.stats.rejected_packets = self.stats.rejected_packets.saturating_add(1);
            self.stats.source_limit_rejections =
                self.stats.source_limit_rejections.saturating_add(1);
            return Err(NatError::SourceFlowLimit {
                source: flow.source.address,
                active: source_active,
                max: self.config.max_flows_per_source,
            });
        }

        let external_port = match self.ports.allocate() {
            Ok(port) => port,
            Err(error) => {
                self.stats.rejected_packets = self.stats.rejected_packets.saturating_add(1);
                self.stats.port_exhaustions = self.stats.port_exhaustions.saturating_add(1);
                return Err(error);
            }
        };
        let tcp_state =
            (view.protocol == TransportProtocol::Tcp).then_some(TcpConnectionState::SynSent);
        let expires_at_ms = now_ms.saturating_add(self.timeout_for(tcp_state));
        let record = FlowRecord {
            flow,
            external_port,
            tcp_state,
            last_seen_ms: now_ms,
            expires_at_ms,
            outbound_packets: 1,
            inbound_packets: 0,
            outbound_transport_bytes: view.transport_len as u64,
            inbound_transport_bytes: 0,
        };

        self.flows.insert(flow, record);
        self.inbound_index.insert(
            InboundKey {
                protocol: flow.protocol,
                remote: flow.destination,
                external_port,
            },
            flow,
        );
        self.expiration_index.insert((expires_at_ms, flow));
        *self
            .flows_per_source
            .entry(flow.source.address)
            .or_insert(0) += 1;
        self.stats.outbound_packets = self.stats.outbound_packets.saturating_add(1);
        self.stats.outbound_transport_bytes = self
            .stats
            .outbound_transport_bytes
            .saturating_add(view.transport_len as u64);
        self.stats.flows_created = self.stats.flows_created.saturating_add(1);

        Ok(NatDecision {
            flow,
            external_port,
            direction: Direction::Outbound,
            tcp_state,
        })
    }

    fn process_existing(
        &mut self,
        key: FlowKey,
        view: &PacketView,
        direction: Direction,
        now_ms: u64,
    ) -> Result<NatDecision, NatError> {
        let old_expiry;
        let next_state;
        {
            let flow = self
                .flows
                .get(&key)
                .expect("inbound index and flow map must be consistent");
            old_expiry = flow.expires_at_ms;
            next_state = match flow.tcp_state {
                None => None,
                Some(state) => match transition_tcp(state, direction, view.tcp_flags) {
                    Ok(next) => Some(next),
                    Err(error) => {
                        self.stats.rejected_packets = self.stats.rejected_packets.saturating_add(1);
                        self.stats.invalid_tcp_transitions =
                            self.stats.invalid_tcp_transitions.saturating_add(1);
                        return Err(error);
                    }
                },
            };
        }

        let expires_at_ms = now_ms.saturating_add(self.timeout_for(next_state));
        self.expiration_index.remove(&(old_expiry, key));
        let flow = self
            .flows
            .get_mut(&key)
            .expect("validated flow must still exist");
        flow.last_seen_ms = now_ms;
        flow.expires_at_ms = expires_at_ms;
        flow.tcp_state = next_state;
        match direction {
            Direction::Outbound => {
                flow.outbound_packets = flow.outbound_packets.saturating_add(1);
                flow.outbound_transport_bytes = flow
                    .outbound_transport_bytes
                    .saturating_add(view.transport_len as u64);
                self.stats.outbound_packets = self.stats.outbound_packets.saturating_add(1);
                self.stats.outbound_transport_bytes = self
                    .stats
                    .outbound_transport_bytes
                    .saturating_add(view.transport_len as u64);
            }
            Direction::Inbound => {
                flow.inbound_packets = flow.inbound_packets.saturating_add(1);
                flow.inbound_transport_bytes = flow
                    .inbound_transport_bytes
                    .saturating_add(view.transport_len as u64);
                self.stats.inbound_packets = self.stats.inbound_packets.saturating_add(1);
                self.stats.inbound_transport_bytes = self
                    .stats
                    .inbound_transport_bytes
                    .saturating_add(view.transport_len as u64);
            }
        }
        self.expiration_index.insert((expires_at_ms, key));
        let external_port = flow.external_port;

        let decision = NatDecision {
            flow: key,
            external_port,
            direction,
            tcp_state: next_state,
        };
        if view.protocol == TransportProtocol::Tcp && view.tcp_flags.rst() {
            self.remove_flow(&key, RemovalReason::Reset);
        }
        Ok(decision)
    }

    fn observe_time(&mut self, now_ms: u64) -> Result<(), NatError> {
        if let Some(last_ms) = self.last_now_ms {
            if now_ms < last_ms {
                return Err(NatError::ClockWentBackwards { last_ms, now_ms });
            }
        }
        self.last_now_ms = Some(now_ms);
        Ok(())
    }

    fn expire_trusted(&mut self, now_ms: u64) -> usize {
        let mut expired = 0;
        while let Some(&(expires_at_ms, key)) = self.expiration_index.first() {
            if expires_at_ms > now_ms {
                break;
            }
            if self.remove_flow(&key, RemovalReason::Expired).is_some() {
                expired += 1;
            }
        }
        expired
    }

    fn remove_flow(&mut self, key: &FlowKey, reason: RemovalReason) -> Option<FlowRecord> {
        let flow = self.flows.remove(key)?;
        self.expiration_index.remove(&(flow.expires_at_ms, *key));
        self.inbound_index.remove(&InboundKey {
            protocol: flow.flow.protocol,
            remote: flow.flow.destination,
            external_port: flow.external_port,
        });
        self.ports.release(flow.external_port);
        if let Entry::Occupied(mut entry) = self.flows_per_source.entry(flow.source_address()) {
            *entry.get_mut() -= 1;
            if *entry.get() == 0 {
                entry.remove();
            }
        }
        match reason {
            RemovalReason::Expired => {
                self.stats.flows_expired = self.stats.flows_expired.saturating_add(1);
            }
            RemovalReason::Reset => {
                self.stats.flows_reset = self.stats.flows_reset.saturating_add(1);
            }
        }
        Some(flow)
    }

    fn timeout_for(&self, tcp_state: Option<TcpConnectionState>) -> u64 {
        match tcp_state {
            None => self.config.udp_timeout_ms,
            Some(TcpConnectionState::SynSent) => self.config.tcp_syn_timeout_ms,
            Some(TcpConnectionState::Established) => self.config.tcp_established_timeout_ms,
            Some(TcpConnectionState::Closing) => self.config.tcp_closing_timeout_ms,
        }
    }

    fn reject_malformed(&mut self, error: MalformedPacket) -> NatError {
        self.stats.malformed_packets = self.stats.malformed_packets.saturating_add(1);
        self.stats.rejected_packets = self.stats.rejected_packets.saturating_add(1);
        NatError::MalformedPacket(error)
    }
}

impl FlowRecord {
    fn source_address(self) -> Ipv4Addr {
        self.flow.source.address
    }
}

fn flow_key(view: &PacketView) -> FlowKey {
    FlowKey {
        protocol: view.protocol,
        source: Endpoint::new(view.source, view.source_port),
        destination: Endpoint::new(view.destination, view.destination_port),
    }
}

fn transition_tcp(
    state: TcpConnectionState,
    direction: Direction,
    flags: TcpFlags,
) -> Result<TcpConnectionState, NatError> {
    let invalid = || NatError::InvalidTcpTransition {
        direction,
        state,
        flags,
    };

    if flags.rst() {
        return Ok(match state {
            TcpConnectionState::SynSent => TcpConnectionState::Closing,
            other => other,
        });
    }

    match (direction, state) {
        (Direction::Outbound, TcpConnectionState::SynSent) if flags.syn() && !flags.ack() => {
            Ok(TcpConnectionState::SynSent)
        }
        (Direction::Inbound, TcpConnectionState::SynSent) if flags.syn() && flags.ack() => {
            Ok(TcpConnectionState::Established)
        }
        (Direction::Outbound | Direction::Inbound, TcpConnectionState::Established)
            if !flags.syn() =>
        {
            Ok(if flags.fin() {
                TcpConnectionState::Closing
            } else {
                TcpConnectionState::Established
            })
        }
        (Direction::Outbound | Direction::Inbound, TcpConnectionState::Closing) if !flags.syn() => {
            Ok(TcpConnectionState::Closing)
        }
        _ => Err(invalid()),
    }
}
