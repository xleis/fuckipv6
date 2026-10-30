use super::*;
use std::net::Ipv4Addr;

const INTERNAL_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 2);
const INTERNAL_IP_2: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 3);
const EXTERNAL_IP: Ipv4Addr = Ipv4Addr::new(203, 0, 113, 1);
const REMOTE_IP: Ipv4Addr = Ipv4Addr::new(198, 51, 100, 10);

fn config() -> NatConfig {
    NatConfig {
        max_flows: 16,
        max_flows_per_source: 8,
        external_port_start: 50_000,
        external_port_end: 50_015,
        tcp_syn_timeout_ms: 2_000,
        tcp_established_timeout_ms: 10_000,
        tcp_closing_timeout_ms: 1_000,
        udp_timeout_ms: 5_000,
        memory_budget: MemoryBudget::default(),
    }
}

fn udp_packet(
    source: Ipv4Addr,
    source_port: u16,
    destination: Ipv4Addr,
    destination_port: u16,
    payload: &[u8],
) -> Vec<u8> {
    transport_packet(
        TransportProtocol::Udp,
        source,
        source_port,
        destination,
        destination_port,
        0,
        payload,
    )
}

fn tcp_packet(
    source: Ipv4Addr,
    source_port: u16,
    destination: Ipv4Addr,
    destination_port: u16,
    flags: u8,
    payload: &[u8],
) -> Vec<u8> {
    transport_packet(
        TransportProtocol::Tcp,
        source,
        source_port,
        destination,
        destination_port,
        flags,
        payload,
    )
}

fn transport_packet(
    protocol: TransportProtocol,
    source: Ipv4Addr,
    source_port: u16,
    destination: Ipv4Addr,
    destination_port: u16,
    flags: u8,
    payload: &[u8],
) -> Vec<u8> {
    let transport = match protocol {
        TransportProtocol::Tcp => {
            let mut segment = vec![0u8; 20 + payload.len()];
            segment[0..2].copy_from_slice(&source_port.to_be_bytes());
            segment[2..4].copy_from_slice(&destination_port.to_be_bytes());
            segment[4..8].copy_from_slice(&1u32.to_be_bytes());
            if flags & TcpFlags::ACK != 0 {
                segment[8..12].copy_from_slice(&1u32.to_be_bytes());
            }
            segment[12] = 5 << 4;
            segment[13] = flags;
            segment[14..16].copy_from_slice(&65_535u16.to_be_bytes());
            segment[20..].copy_from_slice(payload);
            let checksum = transport_checksum(source, destination, protocol, &segment);
            segment[16..18].copy_from_slice(&checksum.to_be_bytes());
            segment
        }
        TransportProtocol::Udp => {
            let mut datagram = vec![0u8; 8 + payload.len()];
            datagram[0..2].copy_from_slice(&source_port.to_be_bytes());
            datagram[2..4].copy_from_slice(&destination_port.to_be_bytes());
            let datagram_len = datagram.len() as u16;
            datagram[4..6].copy_from_slice(&datagram_len.to_be_bytes());
            datagram[8..].copy_from_slice(payload);
            let checksum = transport_checksum(source, destination, protocol, &datagram);
            datagram[6..8].copy_from_slice(&checksum.to_be_bytes());
            datagram
        }
    };

    let total_len = 20 + transport.len();
    let mut packet = vec![0u8; total_len];
    packet[0] = 0x45;
    packet[2..4].copy_from_slice(&(total_len as u16).to_be_bytes());
    packet[8] = 64;
    packet[9] = protocol_number(protocol);
    packet[12..16].copy_from_slice(&source.octets());
    packet[16..20].copy_from_slice(&destination.octets());
    let header_checksum = checksum(&packet[..20]);
    packet[10..12].copy_from_slice(&header_checksum.to_be_bytes());
    packet[20..].copy_from_slice(&transport);
    packet
}

fn transport_checksum(
    source: Ipv4Addr,
    destination: Ipv4Addr,
    protocol: TransportProtocol,
    transport: &[u8],
) -> u16 {
    let mut pseudo = Vec::with_capacity(12 + transport.len());
    pseudo.extend_from_slice(&source.octets());
    pseudo.extend_from_slice(&destination.octets());
    pseudo.push(0);
    pseudo.push(protocol_number(protocol));
    pseudo.extend_from_slice(&(transport.len() as u16).to_be_bytes());
    pseudo.extend_from_slice(transport);
    let checksum = checksum(&pseudo);
    if checksum == 0 {
        0xffff
    } else {
        checksum
    }
}

fn protocol_number(protocol: TransportProtocol) -> u8 {
    match protocol {
        TransportProtocol::Tcp => 6,
        TransportProtocol::Udp => 17,
    }
}

fn checksum(bytes: &[u8]) -> u16 {
    let mut sum = 0u32;
    for chunk in bytes.chunks(2) {
        let word = if chunk.len() == 2 {
            u16::from_be_bytes([chunk[0], chunk[1]])
        } else {
            u16::from(chunk[0]) << 8
        };
        sum = sum.wrapping_add(u32::from(word));
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn repair_ipv4_checksum(packet: &mut [u8]) {
    packet[10] = 0;
    packet[11] = 0;
    let checksum = checksum(&packet[..20]);
    packet[10..12].copy_from_slice(&checksum.to_be_bytes());
}

#[test]
fn default_budget_is_bounded_and_invalid_config_is_rejected() {
    let config = NatConfig::default();
    config.validate().unwrap();

    assert_eq!(config.max_flows, 4_096);
    assert_eq!(
        config.memory_budget.estimated_bytes(config.max_flows),
        2 * 1024 * 1024 + 64 * 1024
    );
    assert!(config.memory_budget.allows(config.max_flows));
    let mut invalid = config;
    invalid.memory_budget.fixed_bytes = 0;
    assert!(NatTable::new(invalid).is_err());
}

#[test]
fn udp_flow_is_bidirectional_and_expires_at_its_deadline() {
    let mut table = NatTable::new(config()).unwrap();
    let outbound = udp_packet(INTERNAL_IP, 40_000, REMOTE_IP, 53, b"query");
    let mapping = table.process_outbound(&outbound, 1_000).unwrap();

    assert_eq!(mapping.external_port, 50_000);
    assert_eq!(mapping.direction, Direction::Outbound);
    assert_eq!(mapping.tcp_state, None);

    let inbound = udp_packet(
        REMOTE_IP,
        53,
        EXTERNAL_IP,
        mapping.external_port,
        b"response",
    );
    let reply = table.process_inbound(&inbound, 2_000).unwrap();
    assert_eq!(reply.flow, mapping.flow);
    assert_eq!(reply.direction, Direction::Inbound);

    let snapshot = table.flow_snapshots().pop().unwrap();
    assert_eq!(snapshot.outbound_packets, 1);
    assert_eq!(snapshot.inbound_packets, 1);
    assert_eq!(snapshot.expires_at_ms, 7_000);

    assert_eq!(table.expire(6_999).unwrap(), 0);
    assert_eq!(table.active_flows(), 1);
    assert_eq!(table.expire(7_000).unwrap(), 1);
    assert_eq!(table.active_flows(), 0);
    assert_eq!(table.stats().flows_expired, 1);
}

#[test]
fn tcp_tracks_syn_establishment_fin_and_closing_timeout() {
    let mut table = NatTable::new(config()).unwrap();
    let syn = tcp_packet(INTERNAL_IP, 40_001, REMOTE_IP, 443, TcpFlags::SYN, b"");
    let mapping = table.process_outbound(&syn, 100).unwrap();
    assert_eq!(mapping.tcp_state, Some(TcpConnectionState::SynSent));

    let syn_ack = tcp_packet(
        REMOTE_IP,
        443,
        EXTERNAL_IP,
        mapping.external_port,
        TcpFlags::SYN | TcpFlags::ACK,
        b"",
    );
    let established = table.process_inbound(&syn_ack, 200).unwrap();
    assert_eq!(established.tcp_state, Some(TcpConnectionState::Established));

    let ack = tcp_packet(
        INTERNAL_IP,
        40_001,
        REMOTE_IP,
        443,
        TcpFlags::ACK,
        b"request",
    );
    assert_eq!(
        table.process_outbound(&ack, 300).unwrap().tcp_state,
        Some(TcpConnectionState::Established)
    );

    let fin = tcp_packet(
        INTERNAL_IP,
        40_001,
        REMOTE_IP,
        443,
        TcpFlags::FIN | TcpFlags::ACK,
        b"",
    );
    assert_eq!(
        table.process_outbound(&fin, 400).unwrap().tcp_state,
        Some(TcpConnectionState::Closing)
    );
    assert_eq!(table.expire(1_399).unwrap(), 0);
    assert_eq!(table.expire(1_400).unwrap(), 1);
}

#[test]
fn expired_external_port_is_reused_before_advancing() {
    let mut table = NatTable::new(config()).unwrap();
    let first = table
        .process_outbound(
            &udp_packet(INTERNAL_IP, 40_010, REMOTE_IP, 1000, b"a"),
            1_000,
        )
        .unwrap();
    let second = table
        .process_outbound(
            &udp_packet(INTERNAL_IP, 40_011, REMOTE_IP, 1001, b"b"),
            2_000,
        )
        .unwrap();
    assert_eq!(first.external_port, 50_000);
    assert_eq!(second.external_port, 50_001);

    table.expire(6_000).unwrap();
    let replacement = table
        .process_outbound(
            &udp_packet(INTERNAL_IP, 40_012, REMOTE_IP, 1002, b"c"),
            6_000,
        )
        .unwrap();
    assert_eq!(replacement.external_port, first.external_port);
}

#[test]
fn per_source_limit_rejects_new_flow_without_evicting_existing_flow() {
    let mut config = config();
    config.max_flows_per_source = 1;
    let mut table = NatTable::new(config).unwrap();
    table
        .process_outbound(
            &udp_packet(INTERNAL_IP, 40_020, REMOTE_IP, 1000, b"first"),
            100,
        )
        .unwrap();

    let error = table
        .process_outbound(
            &udp_packet(INTERNAL_IP, 40_021, REMOTE_IP, 1001, b"second"),
            101,
        )
        .unwrap_err();
    assert!(matches!(error, NatError::SourceFlowLimit { .. }));
    assert_eq!(table.active_flows(), 1);
    assert_eq!(table.stats().source_limit_rejections, 1);
    assert_eq!(table.stats().flows_created, 1);

    table
        .process_outbound(
            &udp_packet(INTERNAL_IP_2, 40_022, REMOTE_IP, 1002, b"other"),
            102,
        )
        .unwrap();
    assert_eq!(table.active_flows(), 2);
}

#[test]
fn global_limit_rejects_new_flow_without_evicting_live_state() {
    let mut config = config();
    config.max_flows = 1;
    let mut table = NatTable::new(config).unwrap();
    let original = table
        .process_outbound(
            &udp_packet(INTERNAL_IP, 40_030, REMOTE_IP, 1000, b"first"),
            100,
        )
        .unwrap();

    let error = table
        .process_outbound(
            &udp_packet(INTERNAL_IP_2, 40_031, REMOTE_IP, 1001, b"second"),
            101,
        )
        .unwrap_err();
    assert!(matches!(error, NatError::CapacityExceeded { .. }));
    assert_eq!(table.active_flows(), 1);
    assert_eq!(table.stats().capacity_rejections, 1);
    assert_eq!(table.flow_snapshots()[0].flow, original.flow);
}

#[test]
fn port_exhaustion_fails_closed_when_other_limits_still_have_room() {
    let mut config = config();
    config.max_flows = 4;
    config.max_flows_per_source = 4;
    config.external_port_start = 51_000;
    config.external_port_end = 51_000;
    let mut table = NatTable::new(config).unwrap();
    table
        .process_outbound(
            &udp_packet(INTERNAL_IP, 40_040, REMOTE_IP, 1000, b"first"),
            100,
        )
        .unwrap();

    let error = table
        .process_outbound(
            &udp_packet(INTERNAL_IP_2, 40_041, REMOTE_IP, 1001, b"second"),
            101,
        )
        .unwrap_err();
    assert!(matches!(error, NatError::PortExhausted { .. }));
    assert_eq!(table.active_flows(), 1);
    assert_eq!(table.stats().port_exhaustions, 1);
}

#[test]
fn malformed_and_unknown_inbound_packets_are_rejected_without_state_change() {
    let mut table = NatTable::new(config()).unwrap();
    table
        .process_outbound(
            &udp_packet(INTERNAL_IP, 40_050, REMOTE_IP, 1000, b"valid"),
            100,
        )
        .unwrap();

    let mut bad_checksum = udp_packet(INTERNAL_IP, 40_051, REMOTE_IP, 1001, b"bad");
    let transport_checksum_offset = 20 + 6;
    bad_checksum[transport_checksum_offset] ^= 0xff;
    let error = table.process_outbound(&bad_checksum, 101).unwrap_err();
    assert!(matches!(
        error,
        NatError::MalformedPacket(MalformedPacket::InvalidUdpChecksum)
    ));

    let unknown = udp_packet(REMOTE_IP, 9999, EXTERNAL_IP, 49_999, b"orphan");
    let error = table.process_inbound(&unknown, 102).unwrap_err();
    assert_eq!(error, NatError::UnknownInbound);

    assert_eq!(table.active_flows(), 1);
    assert_eq!(table.stats().malformed_packets, 1);
    assert_eq!(table.stats().orphan_inbound, 1);
    assert_eq!(table.stats().flows_created, 1);
}

#[test]
fn fragments_and_invalid_tcp_starts_fail_closed() {
    let mut table = NatTable::new(config()).unwrap();
    let mut fragment = udp_packet(INTERNAL_IP, 40_060, REMOTE_IP, 1000, b"fragment");
    fragment[6] = 0x20;
    repair_ipv4_checksum(&mut fragment);
    let error = table.process_outbound(&fragment, 100).unwrap_err();
    assert_eq!(
        error,
        NatError::MalformedPacket(MalformedPacket::FragmentUnsupported)
    );

    let ack_without_syn = tcp_packet(
        INTERNAL_IP,
        40_061,
        REMOTE_IP,
        443,
        TcpFlags::ACK,
        b"invalid",
    );
    let error = table.process_outbound(&ack_without_syn, 101).unwrap_err();
    assert!(matches!(
        error,
        NatError::InvalidTcpTransition {
            direction: Direction::Outbound,
            ..
        }
    ));
    assert_eq!(table.active_flows(), 0);
    assert_eq!(table.stats().invalid_tcp_transitions, 1);
}

#[test]
fn reset_removes_mapping_immediately() {
    let mut table = NatTable::new(config()).unwrap();
    let syn = tcp_packet(INTERNAL_IP, 40_070, REMOTE_IP, 443, TcpFlags::SYN, b"");
    let mapping = table.process_outbound(&syn, 100).unwrap();
    let reset = tcp_packet(INTERNAL_IP, 40_070, REMOTE_IP, 443, TcpFlags::RST, b"");
    table.process_outbound(&reset, 101).unwrap();
    assert_eq!(table.active_flows(), 0);
    assert_eq!(table.stats().flows_reset, 1);
    assert!(mapping.external_port >= 50_000);
}

#[test]
fn clock_rollback_is_rejected_without_mutation() {
    let mut table = NatTable::new(config()).unwrap();
    table.expire(100).unwrap();
    let error = table.expire(99).unwrap_err();
    assert_eq!(
        error,
        NatError::ClockWentBackwards {
            last_ms: 100,
            now_ms: 99
        }
    );
}

#[test]
fn logical_memory_budget_tracks_active_flow_limit() {
    let mut config = config();
    config.max_flows = 4;
    config.memory_budget = MemoryBudget {
        fixed_bytes: 1_024,
        bytes_per_flow: 256,
        max_bytes: 2_048,
    };
    let mut table = NatTable::new(config).unwrap();

    for index in 0..4u16 {
        table
            .process_outbound(
                &udp_packet(INTERNAL_IP, 41_000 + index, REMOTE_IP, 2_000 + index, b"x"),
                100,
            )
            .unwrap();
    }

    assert_eq!(table.active_flows(), 4);
    assert_eq!(table.estimated_memory_bytes(), 2_048);
    assert!(table.estimated_memory_bytes() <= table.config().memory_budget.max_bytes);
}
