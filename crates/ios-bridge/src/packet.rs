//! IP packet validation at the host boundary.
//!
//! The host hands whole IP packets across the ABI. Rust never re-assembles a
//! stream, so a malformed length or an unknown version has to be rejected here
//! instead of being queued into the data plane, where it would surface as a
//! confusing node-level failure much later.

use crate::error::Fv6Status;

/// IP version carried by a validated packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpVersion {
    V4,
    V6,
}

impl IpVersion {
    /// Version number as it appears in the first nibble of the header.
    pub fn as_i32(self) -> i32 {
        match self {
            IpVersion::V4 => 4,
            IpVersion::V6 => 6,
        }
    }
}

const IPV4_MIN_HEADER: usize = 20;
const IPV6_HEADER: usize = 40;

/// Validates one whole IP packet against the device MTU.
///
/// A packet must be a single IPv4 or IPv6 datagram whose header length agrees
/// with the buffer, and it must not exceed the MTU the host configured.
pub fn validate(packet: &[u8], mtu: u16) -> Result<IpVersion, Fv6Status> {
    if packet.is_empty() || packet.len() > mtu as usize {
        return Err(Fv6Status::InvalidPacket);
    }
    match packet[0] >> 4 {
        4 => {
            if packet.len() < IPV4_MIN_HEADER {
                return Err(Fv6Status::InvalidPacket);
            }
            let header_len = usize::from(packet[0] & 0x0f) * 4;
            if header_len < IPV4_MIN_HEADER || header_len > packet.len() {
                return Err(Fv6Status::InvalidPacket);
            }
            let total_len = usize::from(u16::from_be_bytes([packet[2], packet[3]]));
            if total_len != packet.len() {
                return Err(Fv6Status::InvalidPacket);
            }
            Ok(IpVersion::V4)
        }
        6 => {
            if packet.len() < IPV6_HEADER {
                return Err(Fv6Status::InvalidPacket);
            }
            let payload_len = usize::from(u16::from_be_bytes([packet[4], packet[5]]));
            // Jumbograms (payload length 0) need a hop-by-hop option this
            // host never generates, so they are rejected as unsupported.
            if payload_len == 0 || IPV6_HEADER + payload_len != packet.len() {
                return Err(Fv6Status::InvalidPacket);
            }
            Ok(IpVersion::V6)
        }
        _ => Err(Fv6Status::InvalidPacket),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ipv4(total_len: u16, payload: usize) -> Vec<u8> {
        let mut packet = vec![0u8; 20 + payload];
        packet[0] = 0x45;
        packet[2..4].copy_from_slice(&total_len.to_be_bytes());
        packet
    }

    fn ipv6(payload_len: u16, payload: usize) -> Vec<u8> {
        let mut packet = vec![0u8; 40 + payload];
        packet[0] = 0x60;
        packet[4..6].copy_from_slice(&payload_len.to_be_bytes());
        packet
    }

    #[test]
    fn accepts_minimal_ipv4() {
        assert_eq!(validate(&ipv4(20, 0), 1280), Ok(IpVersion::V4));
    }

    #[test]
    fn accepts_ipv4_with_payload() {
        assert_eq!(validate(&ipv4(140, 120), 1280), Ok(IpVersion::V4));
    }

    #[test]
    fn accepts_minimal_ipv6() {
        assert_eq!(validate(&ipv6(8, 8), 1280), Ok(IpVersion::V6));
    }

    #[test]
    fn rejects_unknown_version() {
        let mut packet = ipv4(20, 0);
        packet[0] = 0x75;
        assert_eq!(validate(&packet, 1280), Err(Fv6Status::InvalidPacket));
    }

    #[test]
    fn rejects_truncated_ipv4() {
        let packet = vec![0x45u8; 12];
        assert_eq!(validate(&packet, 1280), Err(Fv6Status::InvalidPacket));
    }

    #[test]
    fn rejects_ipv4_length_mismatch() {
        assert_eq!(validate(&ipv4(60, 0), 1280), Err(Fv6Status::InvalidPacket));
    }

    #[test]
    fn rejects_ipv4_header_bits_below_minimum() {
        let mut packet = ipv4(20, 0);
        packet[0] = 0x40;
        assert_eq!(validate(&packet, 1280), Err(Fv6Status::InvalidPacket));
    }

    #[test]
    fn rejects_ipv4_header_longer_than_packet() {
        let mut packet = ipv4(20, 0);
        packet[0] = 0x4f;
        assert_eq!(validate(&packet, 1280), Err(Fv6Status::InvalidPacket));
    }

    #[test]
    fn rejects_ipv6_length_mismatch() {
        assert_eq!(validate(&ipv6(8, 0), 1280), Err(Fv6Status::InvalidPacket));
    }

    #[test]
    fn rejects_ipv6_jumbogram() {
        let mut packet = ipv6(0, 8);
        packet[4..6].copy_from_slice(&0u16.to_be_bytes());
        assert_eq!(validate(&packet, 1280), Err(Fv6Status::InvalidPacket));
    }

    #[test]
    fn rejects_packet_larger_than_mtu() {
        assert_eq!(
            validate(&ipv4(1400, 1380), 1280),
            Err(Fv6Status::InvalidPacket)
        );
    }

    #[test]
    fn rejects_empty_packet() {
        assert_eq!(validate(&[], 1280), Err(Fv6Status::InvalidPacket));
    }
}
