//! Minimal, allocation-free IPv4/TCP/UDP parser used only by the NX-16 prototype.
//!
//! This parser intentionally accepts the narrow subset needed to exercise flow
//! tracking. It does not reproduce a complete host IP stack.

use std::fmt;
use std::net::Ipv4Addr;

pub(crate) const TCP_PROTOCOL: u8 = 6;
pub(crate) const UDP_PROTOCOL: u8 = 17;

const IPV4_MIN_HEADER_LEN: usize = 20;
const TCP_MIN_HEADER_LEN: usize = 20;
const UDP_HEADER_LEN: usize = 8;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum TransportProtocol {
    Tcp,
    Udp,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TcpFlags(u8);

impl TcpFlags {
    pub const FIN: u8 = 0x01;
    pub const SYN: u8 = 0x02;
    pub const RST: u8 = 0x04;
    pub const ACK: u8 = 0x10;

    pub(crate) const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn fin(self) -> bool {
        self.0 & Self::FIN != 0
    }

    pub const fn syn(self) -> bool {
        self.0 & Self::SYN != 0
    }

    pub const fn rst(self) -> bool {
        self.0 & Self::RST != 0
    }

    pub const fn ack(self) -> bool {
        self.0 & Self::ACK != 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PacketView {
    pub source: Ipv4Addr,
    pub destination: Ipv4Addr,
    pub protocol: TransportProtocol,
    pub source_port: u16,
    pub destination_port: u16,
    pub tcp_flags: TcpFlags,
    pub transport_len: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MalformedPacket {
    TooShort,
    InvalidVersion(u8),
    InvalidHeaderLength(usize),
    InvalidTotalLength(usize),
    TruncatedTotalLength { declared: usize, available: usize },
    InvalidHeaderChecksum,
    FragmentUnsupported,
    UnsupportedProtocol(u8),
    InvalidSourceAddress(Ipv4Addr),
    InvalidDestinationAddress(Ipv4Addr),
    ZeroPort,
    TruncatedTcpHeader(usize),
    InvalidTcpDataOffset(usize),
    TcpSegmentTooLong { header: usize, declared: usize },
    InvalidTcpChecksum,
    TruncatedUdpHeader(usize),
    InvalidUdpLength { declared: usize, available: usize },
    InvalidUdpChecksum,
}

impl fmt::Display for MalformedPacket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort => f.write_str("packet is shorter than an IPv4 header"),
            Self::InvalidVersion(version) => write!(f, "unsupported IPv4 version {version}"),
            Self::InvalidHeaderLength(length) => {
                write!(f, "invalid IPv4 header length {length}")
            }
            Self::InvalidTotalLength(length) => write!(f, "invalid IPv4 total length {length}"),
            Self::TruncatedTotalLength {
                declared,
                available,
            } => write!(
                f,
                "IPv4 total length {declared} exceeds available bytes {available}"
            ),
            Self::InvalidHeaderChecksum => f.write_str("invalid IPv4 header checksum"),
            Self::FragmentUnsupported => f.write_str("IPv4 fragments are not supported"),
            Self::UnsupportedProtocol(protocol) => {
                write!(f, "unsupported IPv4 transport protocol {protocol}")
            }
            Self::InvalidSourceAddress(address) => {
                write!(f, "invalid non-unicast source address {address}")
            }
            Self::InvalidDestinationAddress(address) => {
                write!(f, "invalid non-unicast destination address {address}")
            }
            Self::ZeroPort => f.write_str("TCP/UDP source or destination port is zero"),
            Self::TruncatedTcpHeader(length) => {
                write!(f, "truncated TCP header length {length}")
            }
            Self::InvalidTcpDataOffset(offset) => {
                write!(f, "invalid TCP data offset {offset}")
            }
            Self::TcpSegmentTooLong { header, declared } => write!(
                f,
                "TCP segment header {header} exceeds declared transport length {declared}"
            ),
            Self::InvalidTcpChecksum => f.write_str("invalid TCP checksum"),
            Self::TruncatedUdpHeader(length) => {
                write!(f, "truncated UDP header length {length}")
            }
            Self::InvalidUdpLength {
                declared,
                available,
            } => write!(
                f,
                "UDP length {declared} does not match available length {available}"
            ),
            Self::InvalidUdpChecksum => f.write_str("invalid UDP checksum"),
        }
    }
}

impl std::error::Error for MalformedPacket {}

pub(crate) fn parse_ipv4_packet(packet: &[u8]) -> Result<PacketView, MalformedPacket> {
    if packet.len() < IPV4_MIN_HEADER_LEN {
        return Err(MalformedPacket::TooShort);
    }

    let version = packet[0] >> 4;
    if version != 4 {
        return Err(MalformedPacket::InvalidVersion(version));
    }

    let header_len = usize::from(packet[0] & 0x0f) * 4;
    if !(IPV4_MIN_HEADER_LEN..=packet.len()).contains(&header_len) {
        return Err(MalformedPacket::InvalidHeaderLength(header_len));
    }

    let total_len = usize::from(u16::from_be_bytes([packet[2], packet[3]]));
    if total_len < header_len {
        return Err(MalformedPacket::InvalidTotalLength(total_len));
    }
    if total_len > packet.len() {
        return Err(MalformedPacket::TruncatedTotalLength {
            declared: total_len,
            available: packet.len(),
        });
    }
    if internet_checksum(&packet[..header_len]) != 0 {
        return Err(MalformedPacket::InvalidHeaderChecksum);
    }

    let fragment_field = u16::from_be_bytes([packet[6], packet[7]]);
    if fragment_field & 0x3fff != 0 {
        return Err(MalformedPacket::FragmentUnsupported);
    }

    let source = Ipv4Addr::new(packet[12], packet[13], packet[14], packet[15]);
    let destination = Ipv4Addr::new(packet[16], packet[17], packet[18], packet[19]);
    if !is_valid_source(source) {
        return Err(MalformedPacket::InvalidSourceAddress(source));
    }
    if !is_valid_destination(destination) {
        return Err(MalformedPacket::InvalidDestinationAddress(destination));
    }

    let protocol_number = packet[9];
    let transport = &packet[header_len..total_len];
    match protocol_number {
        TCP_PROTOCOL => parse_tcp(source, destination, transport),
        UDP_PROTOCOL => parse_udp(source, destination, transport),
        other => Err(MalformedPacket::UnsupportedProtocol(other)),
    }
}

fn is_valid_source(address: Ipv4Addr) -> bool {
    !address.is_unspecified() && !address.is_broadcast() && !address.is_multicast()
}

fn is_valid_destination(address: Ipv4Addr) -> bool {
    !address.is_unspecified() && !address.is_broadcast() && !address.is_multicast()
}

fn parse_tcp(
    source: Ipv4Addr,
    destination: Ipv4Addr,
    segment: &[u8],
) -> Result<PacketView, MalformedPacket> {
    if segment.len() < TCP_MIN_HEADER_LEN {
        return Err(MalformedPacket::TruncatedTcpHeader(segment.len()));
    }

    let header_len = usize::from(segment[12] >> 4) * 4;
    if header_len < TCP_MIN_HEADER_LEN {
        return Err(MalformedPacket::InvalidTcpDataOffset(header_len));
    }
    if header_len > segment.len() {
        return Err(MalformedPacket::TcpSegmentTooLong {
            header: header_len,
            declared: segment.len(),
        });
    }

    let source_port = u16::from_be_bytes([segment[0], segment[1]]);
    let destination_port = u16::from_be_bytes([segment[2], segment[3]]);
    if source_port == 0 || destination_port == 0 {
        return Err(MalformedPacket::ZeroPort);
    }
    if !transport_checksum_is_valid(source, destination, TCP_PROTOCOL, segment) {
        return Err(MalformedPacket::InvalidTcpChecksum);
    }

    Ok(PacketView {
        source,
        destination,
        protocol: TransportProtocol::Tcp,
        source_port,
        destination_port,
        tcp_flags: TcpFlags::from_bits(segment[13]),
        transport_len: segment.len(),
    })
}

fn parse_udp(
    source: Ipv4Addr,
    destination: Ipv4Addr,
    datagram: &[u8],
) -> Result<PacketView, MalformedPacket> {
    if datagram.len() < UDP_HEADER_LEN {
        return Err(MalformedPacket::TruncatedUdpHeader(datagram.len()));
    }

    let source_port = u16::from_be_bytes([datagram[0], datagram[1]]);
    let destination_port = u16::from_be_bytes([datagram[2], datagram[3]]);
    if source_port == 0 || destination_port == 0 {
        return Err(MalformedPacket::ZeroPort);
    }

    let declared_len = usize::from(u16::from_be_bytes([datagram[4], datagram[5]]));
    if declared_len != datagram.len() {
        return Err(MalformedPacket::InvalidUdpLength {
            declared: declared_len,
            available: datagram.len(),
        });
    }

    let checksum = u16::from_be_bytes([datagram[6], datagram[7]]);
    if checksum != 0 && !transport_checksum_is_valid(source, destination, UDP_PROTOCOL, datagram) {
        return Err(MalformedPacket::InvalidUdpChecksum);
    }

    Ok(PacketView {
        source,
        destination,
        protocol: TransportProtocol::Udp,
        source_port,
        destination_port,
        tcp_flags: TcpFlags::default(),
        transport_len: datagram.len(),
    })
}

fn transport_checksum_is_valid(
    source: Ipv4Addr,
    destination: Ipv4Addr,
    protocol: u8,
    transport: &[u8],
) -> bool {
    let mut sum = 0u32;
    sum = add_words(sum, &source.octets());
    sum = add_words(sum, &destination.octets());
    sum = sum.wrapping_add(u32::from(protocol));
    sum = sum.wrapping_add(transport.len() as u32);
    sum = add_words(sum, transport);
    fold_sum(sum) == 0xffff
}

fn internet_checksum(bytes: &[u8]) -> u16 {
    !fold_sum(add_words(0, bytes))
}

fn add_words(mut sum: u32, bytes: &[u8]) -> u32 {
    for index in (0..bytes.len()).step_by(2) {
        let word = u16::from(bytes[index]) << 8
            | u16::from(bytes.get(index + 1).copied().unwrap_or_default());
        sum = sum.wrapping_add(u32::from(word));
    }
    sum
}

fn fold_sum(mut sum: u32) -> u16 {
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    sum as u16
}
