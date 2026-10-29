use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr;
use ipnet::IpNet;

/// 访问控制规则动作
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AclAction {
    #[default]
    Allow,
    Deny,
}

/// 访问控制方向
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AclDirection {
    Inbound,
    Outbound,
    #[default]
    Both,
}

impl AclDirection {
    pub fn matches(&self, target: AclDirection) -> bool {
        match (*self, target) {
            (AclDirection::Both, _) | (_, AclDirection::Both) => true,
            (a, b) => a == b,
        }
    }
}

/// 协议匹配类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AclProtocol {
    #[default]
    Any,
    Tcp,
    Udp,
    Icmp,
    Icmpv6,
}

impl AclProtocol {
    pub fn matches(&self, target: AclProtocol) -> bool {
        match (*self, target) {
            (AclProtocol::Any, _) | (_, AclProtocol::Any) => true,
            (a, b) => a == b,
        }
    }
}

/// ACL 单条规则配置
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AclRule {
    /// 规则描述备注
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// 规则动作: allow 或 deny
    #[serde(default)]
    pub action: AclAction,

    /// 流量方向: inbound, outbound 或 both
    #[serde(default)]
    pub direction: AclDirection,

    /// 源 CIDR (可选，如 "fd00::/64" 或 "10.8.0.0/24")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub src_cidr: Option<String>,

    /// 目的 CIDR (可选)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dst_cidr: Option<String>,

    /// 协议 (tcp, udp, icmp, icmpv6, any)
    #[serde(default)]
    pub protocol: AclProtocol,

    /// 精确目的端口 (可选)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dst_port: Option<u16>,

    /// 目的端口范围 (可选，起始端口和结束端口闭区间 [start, end])
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dst_port_range: Option<(u16, u16)>,
}

/// 客户端 ACL 配置
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AclConfig {
    /// 是否启用 ACL 过滤 (默认 false)
    #[serde(default)]
    pub enabled: bool,

    /// 默认动作 (当没有任何规则匹配时采用的动作，默认 allow)
    #[serde(default)]
    pub default_action: AclAction,

    /// 规则列表 (按声明顺序从上往下依次匹配，先匹配者先生效)
    #[serde(default)]
    pub rules: Vec<AclRule>,
}

impl Default for AclConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            default_action: AclAction::Allow,
            rules: Vec::new(),
        }
    }
}

/// 解析后的 IP 数据包关键元数据
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPacketInfo {
    pub src_ip: IpAddr,
    pub dst_ip: IpAddr,
    pub protocol: AclProtocol,
    pub src_port: Option<u16>,
    pub dst_port: Option<u16>,
}

/// 解析 IPv4 / IPv6 原始数据包头
pub fn parse_packet(packet: &[u8]) -> Option<ParsedPacketInfo> {
    if packet.is_empty() {
        return None;
    }

    let version = packet[0] >> 4;
    match version {
        6 => parse_ipv6_packet(packet),
        4 => parse_ipv4_packet(packet),
        _ => None,
    }
}

fn parse_ipv6_packet(packet: &[u8]) -> Option<ParsedPacketInfo> {
    if packet.len() < 40 {
        return None;
    }

    let next_header = packet[6];
    let src_bytes: [u8; 16] = packet[8..24].try_into().ok()?;
    let dst_bytes: [u8; 16] = packet[24..40].try_into().ok()?;
    let src_ip = IpAddr::V6(Ipv6Addr::from(src_bytes));
    let dst_ip = IpAddr::V6(Ipv6Addr::from(dst_bytes));

    let (protocol, src_port, dst_port) = match next_header {
        6 => {
            // TCP
            if packet.len() >= 44 {
                let sp = u16::from_be_bytes([packet[40], packet[41]]);
                let dp = u16::from_be_bytes([packet[42], packet[43]]);
                (AclProtocol::Tcp, Some(sp), Some(dp))
            } else {
                (AclProtocol::Tcp, None, None)
            }
        }
        17 => {
            // UDP
            if packet.len() >= 44 {
                let sp = u16::from_be_bytes([packet[40], packet[41]]);
                let dp = u16::from_be_bytes([packet[42], packet[43]]);
                (AclProtocol::Udp, Some(sp), Some(dp))
            } else {
                (AclProtocol::Udp, None, None)
            }
        }
        58 => (AclProtocol::Icmpv6, None, None),
        _ => (AclProtocol::Any, None, None),
    };

    Some(ParsedPacketInfo {
        src_ip,
        dst_ip,
        protocol,
        src_port,
        dst_port,
    })
}

fn parse_ipv4_packet(packet: &[u8]) -> Option<ParsedPacketInfo> {
    if packet.len() < 20 {
        return None;
    }

    let ihl = ((packet[0] & 0x0f) as usize) * 4;
    if packet.len() < ihl {
        return None;
    }

    let proto_num = packet[9];
    let src_bytes: [u8; 4] = packet[12..16].try_into().ok()?;
    let dst_bytes: [u8; 4] = packet[16..20].try_into().ok()?;
    let src_ip = IpAddr::V4(Ipv4Addr::from(src_bytes));
    let dst_ip = IpAddr::V4(Ipv4Addr::from(dst_bytes));

    let (protocol, src_port, dst_port) = match proto_num {
        6 => {
            // TCP
            if packet.len() >= ihl + 4 {
                let sp = u16::from_be_bytes([packet[ihl], packet[ihl + 1]]);
                let dp = u16::from_be_bytes([packet[ihl + 2], packet[ihl + 3]]);
                (AclProtocol::Tcp, Some(sp), Some(dp))
            } else {
                (AclProtocol::Tcp, None, None)
            }
        }
        17 => {
            // UDP
            if packet.len() >= ihl + 4 {
                let sp = u16::from_be_bytes([packet[ihl], packet[ihl + 1]]);
                let dp = u16::from_be_bytes([packet[ihl + 2], packet[ihl + 3]]);
                (AclProtocol::Udp, Some(sp), Some(dp))
            } else {
                (AclProtocol::Udp, None, None)
            }
        }
        1 => (AclProtocol::Icmp, None, None),
        _ => (AclProtocol::Any, None, None),
    };

    Some(ParsedPacketInfo {
        src_ip,
        dst_ip,
        protocol,
        src_port,
        dst_port,
    })
}

/// 预编译的高性能 ACL 单条规则
#[derive(Debug, Clone)]
struct CompiledRule {
    action: AclAction,
    direction: AclDirection,
    src_net: Option<IpNet>,
    dst_net: Option<IpNet>,
    protocol: AclProtocol,
    dst_port: Option<u16>,
    dst_port_range: Option<(u16, u16)>,
}

impl CompiledRule {
    fn matches(&self, direction: AclDirection, info: &ParsedPacketInfo) -> bool {
        // 检查方向
        if !self.direction.matches(direction) {
            return false;
        }

        // 检查协议
        if !self.protocol.matches(info.protocol) {
            return false;
        }

        // 检查源地址 CIDR
        if let Some(src_net) = &self.src_net {
            if !src_net.contains(&info.src_ip) {
                return false;
            }
        }

        // 检查目的地址 CIDR
        if let Some(dst_net) = &self.dst_net {
            if !dst_net.contains(&info.dst_ip) {
                return false;
            }
        }

        // 检查端口匹配
        let has_port_constraint = self.dst_port.is_some() || self.dst_port_range.is_some();
        if has_port_constraint {
            let Some(port) = info.dst_port else {
                return false;
            };

            let port_match = match (self.dst_port, self.dst_port_range) {
                (Some(p), Some((start, end))) => p == port || (port >= start && port <= end),
                (Some(p), None) => p == port,
                (None, Some((start, end))) => port >= start && port <= end,
                (None, None) => true,
            };

            if !port_match {
                return false;
            }
        }

        true
    }
}

/// ACL 执行引擎
#[derive(Debug, Clone)]
pub struct AclEngine {
    enabled: bool,
    default_action: AclAction,
    rules: Vec<CompiledRule>,
}

impl AclEngine {
    pub fn new(config: &AclConfig) -> Self {
        let mut rules = Vec::with_capacity(config.rules.len());

        for r in &config.rules {
            let src_net = r.src_cidr.as_deref().and_then(|c| IpNet::from_str(c).ok());
            let dst_net = r.dst_cidr.as_deref().and_then(|c| IpNet::from_str(c).ok());

            rules.push(CompiledRule {
                action: r.action,
                direction: r.direction,
                src_net,
                dst_net,
                protocol: r.protocol,
                dst_port: r.dst_port,
                dst_port_range: r.dst_port_range,
            });
        }

        Self {
            enabled: config.enabled,
            default_action: config.default_action,
            rules,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// 评估原始数据包在指定方向上的放行决策
    pub fn check(&self, direction: AclDirection, packet: &[u8]) -> AclAction {
        if !self.enabled {
            return AclAction::Allow;
        }

        let Some(info) = parse_packet(packet) else {
            // 无法解析的数据包根据默认策略裁决
            return self.default_action;
        };

        self.check_parsed(direction, &info)
    }

    /// 根据已解析的信息进行快速评估
    pub fn check_parsed(&self, direction: AclDirection, info: &ParsedPacketInfo) -> AclAction {
        if !self.enabled {
            return AclAction::Allow;
        }

        for rule in &self.rules {
            if rule.matches(direction, info) {
                return rule.action;
            }
        }

        self.default_action
    }

    /// 检查出站数据包
    pub fn check_outbound(&self, packet: &[u8]) -> AclAction {
        self.check(AclDirection::Outbound, packet)
    }

    /// 检查入站数据包
    pub fn check_inbound(&self, packet: &[u8]) -> AclAction {
        self.check(AclDirection::Inbound, packet)
    }
}

/// 协调端准入黑名单配置
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BlacklistConfig {
    /// 禁用的客户端 ID 或公钥指纹前缀
    #[serde(default)]
    pub banned_clients: Vec<String>,
}

impl BlacklistConfig {
    pub fn is_banned(&self, client_id: &str, public_key_fingerprint: &str) -> bool {
        for banned in &self.banned_clients {
            let banned = banned.trim();
            if banned.is_empty() {
                continue;
            }
            if client_id == banned || public_key_fingerprint == banned {
                return true;
            }
            // 支持前缀匹配
            if client_id.starts_with(banned) || public_key_fingerprint.starts_with(banned) {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_ipv6_packet(next_header: u8, src: Ipv6Addr, dst: Ipv6Addr, payload: &[u8]) -> Vec<u8> {
        let mut pkt = vec![0u8; 40];
        pkt[0] = 0x60; // Version 6
        pkt[6] = next_header;
        pkt[8..24].copy_from_slice(&src.octets());
        pkt[24..40].copy_from_slice(&dst.octets());
        pkt.extend_from_slice(payload);
        pkt
    }

    #[test]
    fn test_acl_default_allow_when_disabled() {
        let engine = AclEngine::new(&AclConfig::default());
        let pkt = make_test_ipv6_packet(6, "fd00::1".parse().unwrap(), "fd00::2".parse().unwrap(), &[0, 22, 0, 80]);
        assert_eq!(engine.check_outbound(&pkt), AclAction::Allow);
        assert_eq!(engine.check_inbound(&pkt), AclAction::Allow);
    }

    #[test]
    fn test_acl_port_and_cidr_filtering() {
        let config = AclConfig {
            enabled: true,
            default_action: AclAction::Deny,
            rules: vec![
                // 阻断发往 22 端口 (SSH)
                AclRule {
                    name: Some("Block SSH".to_string()),
                    action: AclAction::Deny,
                    direction: AclDirection::Both,
                    src_cidr: None,
                    dst_cidr: None,
                    protocol: AclProtocol::Tcp,
                    dst_port: Some(22),
                    dst_port_range: None,
                },
                // 放行发往 Web 端口 (80..=443)
                AclRule {
                    name: Some("Allow Web".to_string()),
                    action: AclAction::Allow,
                    direction: AclDirection::Outbound,
                    src_cidr: Some("fd00::/64".to_string()),
                    dst_cidr: None,
                    protocol: AclProtocol::Tcp,
                    dst_port: None,
                    dst_port_range: Some((80, 443)),
                },
                // 放行 ICMPv6
                AclRule {
                    name: Some("Allow Ping".to_string()),
                    action: AclAction::Allow,
                    direction: AclDirection::Both,
                    src_cidr: None,
                    dst_cidr: None,
                    protocol: AclProtocol::Icmpv6,
                    dst_port: None,
                    dst_port_range: None,
                },
            ],
        };

        let engine = AclEngine::new(&config);

        let src: Ipv6Addr = "fd00::1".parse().unwrap();
        let dst: Ipv6Addr = "fd00::2".parse().unwrap();

        // 1. SSH packet (dst_port = 22) -> Deny
        let mut ssh_payload = vec![0u8; 20];
        ssh_payload[0..2].copy_from_slice(&12345u16.to_be_bytes());
        ssh_payload[2..4].copy_from_slice(&22u16.to_be_bytes());
        let ssh_pkt = make_test_ipv6_packet(6, src, dst, &ssh_payload);
        assert_eq!(engine.check_outbound(&ssh_pkt), AclAction::Deny);

        // 2. HTTP packet (dst_port = 80) -> Allow
        let mut http_payload = vec![0u8; 20];
        http_payload[0..2].copy_from_slice(&12345u16.to_be_bytes());
        http_payload[2..4].copy_from_slice(&80u16.to_be_bytes());
        let http_pkt = make_test_ipv6_packet(6, src, dst, &http_payload);
        assert_eq!(engine.check_outbound(&http_pkt), AclAction::Allow);

        // 3. Inbound HTTP packet -> Denied by default_action because rule was Outbound only
        assert_eq!(engine.check_inbound(&http_pkt), AclAction::Deny);

        // 4. ICMPv6 packet -> Allow
        let icmp_pkt = make_test_ipv6_packet(58, src, dst, &[128, 0, 0, 0]);
        assert_eq!(engine.check_outbound(&icmp_pkt), AclAction::Allow);
        assert_eq!(engine.check_inbound(&icmp_pkt), AclAction::Allow);

        // 5. Random UDP packet (dst_port = 5000) -> Denied by default
        let mut udp_payload = vec![0u8; 8];
        udp_payload[0..2].copy_from_slice(&12345u16.to_be_bytes());
        udp_payload[2..4].copy_from_slice(&5000u16.to_be_bytes());
        let udp_pkt = make_test_ipv6_packet(17, src, dst, &udp_payload);
        assert_eq!(engine.check_outbound(&udp_pkt), AclAction::Deny);
    }

    #[test]
    fn test_blacklist_config() {
        let blacklist = BlacklistConfig {
            banned_clients: vec!["banned_node_1".to_string(), "evil_prefix_".to_string()],
        };

        assert!(blacklist.is_banned("banned_node_1", "some_fingerprint"));
        assert!(blacklist.is_banned("evil_prefix_xyz", "some_fingerprint"));
        assert!(blacklist.is_banned("good_node", "evil_prefix_999"));
        assert!(!blacklist.is_banned("good_node", "safe_fingerprint"));
    }
}
