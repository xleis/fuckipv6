//! Strict IPv4/TCP parsing and checksum rewriting for rescued SYN-ACK packets.

use std::net::Ipv4Addr;

use thiserror::Error;

const ETHERNET_HEADER_LEN: usize = 14;
const IPV4_MIN_HEADER_LEN: usize = 20;
const TCP_MIN_HEADER_LEN: usize = 20;
const ETH_P_IP: u16 = 0x0800;
const ETH_P_8021Q: u16 = 0x8100;
const ETH_P_8021AD: u16 = 0x88a8;
const IPPROTO_TCP: u8 = 6;

const TCP_FIN: u8 = 0x01;
const TCP_SYN: u8 = 0x02;
const TCP_RST: u8 = 0x04;
const TCP_ACK: u8 = 0x10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TcpSegment<'a> {
    pub packet: &'a [u8],
    pub ip_header_len: usize,
    pub source: Ipv4Addr,
    pub destination: Ipv4Addr,
    pub source_port: u16,
    pub destination_port: u16,
    pub sequence: u32,
    pub acknowledgment: u32,
    pub flags: u8,
}

impl TcpSegment<'_> {
    pub fn is_syn(self) -> bool {
        self.flags & (TCP_FIN | TCP_SYN | TCP_RST | TCP_ACK) == TCP_SYN
    }

    pub fn is_syn_ack(self) -> bool {
        self.flags & (TCP_FIN | TCP_SYN | TCP_RST | TCP_ACK) == (TCP_SYN | TCP_ACK)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PacketError {
    #[error("Ethernet frame is too short")]
    ShortEthernetFrame,
    #[error("Ethernet frame is not IPv4")]
    NotIpv4,
    #[error("IPv4 packet is too short")]
    ShortIpv4Packet,
    #[error("IPv4 length fields are inconsistent")]
    InvalidIpv4Length,
    #[error("IPv4 packet is fragmented")]
    FragmentedIpv4,
    #[error("IP protocol is not TCP")]
    NotTcp,
    #[error("TCP segment is too short")]
    ShortTcpSegment,
    #[error("TCP header length is invalid")]
    InvalidTcpHeaderLength,
    #[error("SYN-ACK has an unexpected TCP source port")]
    UnexpectedSourcePort,
    #[error("packet is not a SYN-ACK")]
    NotSynAck,
}

/// Parses an Ethernet II frame, including up to two 802.1Q/802.1ad tags.
pub fn parse_ethernet_ipv4_tcp(frame: &[u8]) -> Result<TcpSegment<'_>, PacketError> {
    if frame.len() < ETHERNET_HEADER_LEN {
        return Err(PacketError::ShortEthernetFrame);
    }

    let mut ether_type = u16::from_be_bytes([frame[12], frame[13]]);
    let mut ip_offset = ETHERNET_HEADER_LEN;
    for _ in 0..2 {
        if ether_type != ETH_P_8021Q && ether_type != ETH_P_8021AD {
            break;
        }
        if frame.len() < ip_offset + 4 {
            return Err(PacketError::ShortEthernetFrame);
        }
        ether_type = u16::from_be_bytes([frame[ip_offset + 2], frame[ip_offset + 3]]);
        ip_offset += 4;
    }
    if ether_type != ETH_P_IP {
        return Err(PacketError::NotIpv4);
    }
    parse_ipv4_tcp(&frame[ip_offset..])
}

/// Parses a raw IPv4/TCP packet as produced by or injected into a Linux TUN.
pub fn parse_ipv4_tcp(packet: &[u8]) -> Result<TcpSegment<'_>, PacketError> {
    if packet.len() < IPV4_MIN_HEADER_LEN {
        return Err(PacketError::ShortIpv4Packet);
    }
    if packet[0] >> 4 != 4 {
        return Err(PacketError::NotIpv4);
    }

    let ip_header_len = usize::from(packet[0] & 0x0f) * 4;
    if ip_header_len < IPV4_MIN_HEADER_LEN {
        return Err(PacketError::ShortIpv4Packet);
    }
    if packet.len() < ip_header_len + TCP_MIN_HEADER_LEN {
        return Err(PacketError::ShortTcpSegment);
    }
    if packet[9] != IPPROTO_TCP {
        return Err(PacketError::NotTcp);
    }

    let total_len = usize::from(u16::from_be_bytes([packet[2], packet[3]]));
    if total_len < ip_header_len + TCP_MIN_HEADER_LEN || total_len > packet.len() {
        return Err(PacketError::InvalidIpv4Length);
    }
    let fragment = u16::from_be_bytes([packet[6], packet[7]]);
    if fragment & 0x3fff != 0 {
        return Err(PacketError::FragmentedIpv4);
    }

    let tcp = &packet[ip_header_len..total_len];
    let tcp_header_len = usize::from(tcp[12] >> 4) * 4;
    if tcp_header_len < TCP_MIN_HEADER_LEN || tcp_header_len > tcp.len() {
        return Err(PacketError::InvalidTcpHeaderLength);
    }

    let packet = &packet[..total_len];
    Ok(TcpSegment {
        packet,
        ip_header_len,
        source: Ipv4Addr::new(packet[12], packet[13], packet[14], packet[15]),
        destination: Ipv4Addr::new(packet[16], packet[17], packet[18], packet[19]),
        source_port: u16::from_be_bytes([tcp[0], tcp[1]]),
        destination_port: u16::from_be_bytes([tcp[2], tcp[3]]),
        sequence: u32::from_be_bytes([tcp[4], tcp[5], tcp[6], tcp[7]]),
        acknowledgment: u32::from_be_bytes([tcp[8], tcp[9], tcp[10], tcp[11]]),
        flags: tcp[13],
    })
}

/// Rewrites a server-generated SYN-ACK so the client kernel accepts it as the
/// reply to its pending connection, then recomputes both checksums.
pub fn rewrite_synack(
    packet: &mut [u8],
    advertised_server: Ipv4Addr,
    local_ip: Ipv4Addr,
    local_port: u16,
    expected_server_port: u16,
) -> Result<(), PacketError> {
    let parsed = parse_ipv4_tcp(packet)?;
    if !parsed.is_syn_ack() {
        return Err(PacketError::NotSynAck);
    }
    if parsed.source_port != expected_server_port {
        return Err(PacketError::UnexpectedSourcePort);
    }

    let ip_header_len = parsed.ip_header_len;
    let total_len = parsed.packet.len();
    packet[12..16].copy_from_slice(&advertised_server.octets());
    packet[16..20].copy_from_slice(&local_ip.octets());
    packet[ip_header_len + 2..ip_header_len + 4].copy_from_slice(&local_port.to_be_bytes());
    fix_ipv4_tcp_checksums(&mut packet[..total_len])
}

fn fix_ipv4_tcp_checksums(packet: &mut [u8]) -> Result<(), PacketError> {
    let parsed = parse_ipv4_tcp(packet)?;
    let ip_header_len = parsed.ip_header_len;
    let total_len = parsed.packet.len();

    packet[10..12].fill(0);
    let ip_checksum = checksum(&packet[..ip_header_len]);
    packet[10..12].copy_from_slice(&ip_checksum.to_be_bytes());

    let tcp_len = total_len - ip_header_len;
    packet[ip_header_len + 16..ip_header_len + 18].fill(0);
    let mut pseudo_header = Vec::with_capacity(12 + tcp_len);
    pseudo_header.extend_from_slice(&packet[12..20]);
    pseudo_header.push(0);
    pseudo_header.push(IPPROTO_TCP);
    pseudo_header.extend_from_slice(&(tcp_len as u16).to_be_bytes());
    pseudo_header.extend_from_slice(&packet[ip_header_len..total_len]);
    let tcp_checksum = checksum(&pseudo_header);
    packet[ip_header_len + 16..ip_header_len + 18].copy_from_slice(&tcp_checksum.to_be_bytes());
    Ok(())
}

fn checksum(data: &[u8]) -> u16 {
    let mut sum = 0_u64;
    for chunk in data.chunks(2) {
        let word = if chunk.len() == 2 {
            u16::from_be_bytes([chunk[0], chunk[1]])
        } else {
            u16::from_be_bytes([chunk[0], 0])
        };
        sum += u64::from(word);
        while sum >> 16 != 0 {
            sum = (sum & 0xffff) + (sum >> 16);
        }
    }
    !(sum as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_synack() -> Vec<u8> {
        let mut packet = vec![0_u8; IPV4_MIN_HEADER_LEN + TCP_MIN_HEADER_LEN];
        packet[0] = 0x45;
        let packet_len = packet.len() as u16;
        packet[2..4].copy_from_slice(&packet_len.to_be_bytes());
        packet[8] = 64;
        packet[9] = IPPROTO_TCP;
        packet[12..16].copy_from_slice(&[10, 0, 0, 2]);
        packet[16..20].copy_from_slice(&[203, 0, 113, 10]);

        let tcp = IPV4_MIN_HEADER_LEN;
        packet[tcp..tcp + 2].copy_from_slice(&39090_u16.to_be_bytes());
        packet[tcp + 2..tcp + 4].copy_from_slice(&40000_u16.to_be_bytes());
        packet[tcp + 4..tcp + 8].copy_from_slice(&1234_u32.to_be_bytes());
        packet[tcp + 8..tcp + 12].copy_from_slice(&7789_u32.to_be_bytes());
        packet[tcp + 12] = 5 << 4;
        packet[tcp + 13] = TCP_SYN | TCP_ACK;
        fix_ipv4_tcp_checksums(&mut packet).unwrap();
        packet
    }

    #[test]
    fn parses_synack_and_classifies_flags() {
        let packet = sample_synack();
        let parsed = parse_ipv4_tcp(&packet).unwrap();
        assert!(parsed.is_syn_ack());
        assert!(!parsed.is_syn());
        assert_eq!(parsed.source_port, 39090);
        assert_eq!(parsed.acknowledgment, 7789);
    }

    #[test]
    fn rewrites_endpoints_and_repairs_checksums() {
        let mut packet = sample_synack();
        rewrite_synack(
            &mut packet,
            Ipv4Addr::new(198, 51, 100, 20),
            Ipv4Addr::new(192, 0, 2, 55),
            51234,
            39090,
        )
        .unwrap();

        let parsed = parse_ipv4_tcp(&packet).unwrap();
        assert_eq!(parsed.source, Ipv4Addr::new(198, 51, 100, 20));
        assert_eq!(parsed.destination, Ipv4Addr::new(192, 0, 2, 55));
        assert_eq!(parsed.destination_port, 51234);
        assert_eq!(checksum(&packet[..parsed.ip_header_len]), 0);

        let mut pseudo = Vec::new();
        pseudo.extend_from_slice(&packet[12..20]);
        pseudo.push(0);
        pseudo.push(IPPROTO_TCP);
        pseudo.extend_from_slice(
            (packet.len() - parsed.ip_header_len)
                .to_be_bytes()
                .as_slice(),
        );
        pseudo.extend_from_slice(&packet[parsed.ip_header_len..]);
        assert_eq!(checksum(&pseudo), 0);
    }

    #[test]
    fn rewrites_only_synack_from_expected_port() {
        let mut packet = sample_synack();
        assert_eq!(
            rewrite_synack(
                &mut packet,
                Ipv4Addr::LOCALHOST,
                Ipv4Addr::LOCALHOST,
                12345,
                39091
            ),
            Err(PacketError::UnexpectedSourcePort)
        );

        packet[IPV4_MIN_HEADER_LEN + 13] = TCP_ACK;
        assert_eq!(
            rewrite_synack(
                &mut packet,
                Ipv4Addr::LOCALHOST,
                Ipv4Addr::LOCALHOST,
                12345,
                39090
            ),
            Err(PacketError::NotSynAck)
        );
    }

    #[test]
    fn parses_vlan_wrapped_ethernet_frame() {
        let packet = sample_synack();
        let mut frame = vec![0_u8; 18];
        frame[12..14].copy_from_slice(&ETH_P_8021Q.to_be_bytes());
        frame[14..16].copy_from_slice(&42_u16.to_be_bytes());
        frame[16..18].copy_from_slice(&ETH_P_IP.to_be_bytes());
        frame.extend_from_slice(&packet);

        let parsed = parse_ethernet_ipv4_tcp(&frame).unwrap();
        assert_eq!(parsed.source_port, 39090);
        assert_eq!(parsed.packet, packet);
    }

    #[test]
    fn rejects_fragments_and_truncation() {
        let mut packet = sample_synack();
        packet[6..8].copy_from_slice(&0x2000_u16.to_be_bytes());
        assert_eq!(parse_ipv4_tcp(&packet), Err(PacketError::FragmentedIpv4));

        packet[6..8].fill(0);
        packet[2..4].copy_from_slice(&39_u16.to_be_bytes());
        assert_eq!(parse_ipv4_tcp(&packet), Err(PacketError::InvalidIpv4Length));
    }
}
