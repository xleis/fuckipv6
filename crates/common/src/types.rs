use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// 客户端ID (基于公钥哈希的base64编码)
pub type ClientId = String;

/// 虚拟网络工作模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMode {
    /// 仅使用 IPv4 虚拟内网
    Ipv4Only,
    /// 仅使用 IPv6 虚拟内网
    Ipv6Only,
    /// 同时支持 IPv4 和 IPv6 虚拟双栈
    #[default]
    DualStack,
}

impl NetworkMode {
    pub fn is_ipv4_enabled(&self) -> bool {
        matches!(self, Self::Ipv4Only | Self::DualStack)
    }

    pub fn is_ipv6_enabled(&self) -> bool {
        matches!(self, Self::Ipv6Only | Self::DualStack)
    }
}

/// 客户端双栈虚拟 IP 配置
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct VirtualIps {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv4: Option<Ipv4Addr>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv6: Option<Ipv6Addr>,
}

impl VirtualIps {
    pub fn new(ipv4: Option<Ipv4Addr>, ipv6: Option<Ipv6Addr>) -> Self {
        Self { ipv4, ipv6 }
    }

    pub fn from_ipv6(ipv6: Ipv6Addr) -> Self {
        Self {
            ipv4: None,
            ipv6: Some(ipv6),
        }
    }

    pub fn from_ipv4(ipv4: Ipv4Addr) -> Self {
        Self {
            ipv4: Some(ipv4),
            ipv6: None,
        }
    }

    pub fn dual_stack(ipv4: Ipv4Addr, ipv6: Ipv6Addr) -> Self {
        Self {
            ipv4: Some(ipv4),
            ipv6: Some(ipv6),
        }
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        match ip {
            IpAddr::V4(v4) => self.ipv4 == Some(v4),
            IpAddr::V6(v6) => self.ipv6 == Some(v6),
        }
    }

    pub fn primary_ipv6(&self) -> Ipv6Addr {
        self.ipv6.unwrap_or(Ipv6Addr::UNSPECIFIED)
    }

    pub fn primary_ip(&self) -> Option<IpAddr> {
        self.ipv4
            .map(IpAddr::V4)
            .or_else(|| self.ipv6.map(IpAddr::V6))
    }
}

/// 对等节点信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    /// 客户端ID
    pub client_id: ClientId,

    /// 虚拟IP地址 (兼容原有 IPv6 字段)
    pub virtual_ip: Ipv6Addr,

    /// Ed25519公钥
    pub public_key: Vec<u8>,

    /// 公网地址和端口列表 (支持IPv4和IPv6)
    pub endpoints: Vec<SocketAddr>,

    /// NAT类型
    pub nat_type: NatType,

    /// 双栈虚拟 IP 地址 (放在末尾保持 postcard 旧版本兼容)
    #[serde(default)]
    pub virtual_ips: VirtualIps,
}

impl PeerInfo {
    /// 创建新的对等节点信息
    pub fn new(
        client_id: ClientId,
        virtual_ip: Ipv6Addr,
        public_key: Vec<u8>,
        endpoints: Vec<SocketAddr>,
        nat_type: NatType,
    ) -> Self {
        Self {
            client_id,
            virtual_ip,
            virtual_ips: VirtualIps::from_ipv6(virtual_ip),
            public_key,
            endpoints,
            nat_type,
        }
    }

    /// 创建包含完整双栈信息的对等节点信息
    pub fn new_with_ips(
        client_id: ClientId,
        virtual_ips: VirtualIps,
        public_key: Vec<u8>,
        endpoints: Vec<SocketAddr>,
        nat_type: NatType,
    ) -> Self {
        let virtual_ip = virtual_ips.primary_ipv6();
        Self {
            client_id,
            virtual_ip,
            virtual_ips,
            public_key,
            endpoints,
            nat_type,
        }
    }

    /// 检查指定 IP 是否属于该节点
    pub fn matches_ip(&self, ip: IpAddr) -> bool {
        if self.virtual_ips.contains(ip) {
            return true;
        }
        match ip {
            IpAddr::V6(v6) => self.virtual_ip == v6,
            _ => false,
        }
    }
}

/// 路由条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteEntry {
    pub destination: Ipv6Addr,
    pub prefix_len: u8,
    pub next_hop: Ipv6Addr,
}

/// NAT类型
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NatType {
    /// 无NAT/防火墙
    Open,

    /// 完全锥型NAT
    FullCone,

    /// 受限锥型NAT
    Restricted,

    /// 端口受限锥型NAT
    PortRestricted,

    /// 对称型NAT
    Symmetric,

    /// 未知
    #[default]
    Unknown,
}

impl NatType {
    /// 判断两个NAT类型是否可以建立P2P连接
    pub fn can_punch_through(&self, other: &NatType) -> bool {
        use NatType::*;

        match (self, other) {
            // Open可以和任何类型建立连接
            (Open, _) | (_, Open) => true,

            // FullCone可以和大部分类型建立连接
            (FullCone, _) | (_, FullCone) => true,

            // Restricted和PortRestricted之间可以
            (Restricted, Restricted)
            | (Restricted, PortRestricted)
            | (PortRestricted, Restricted)
            | (PortRestricted, PortRestricted) => true,

            // Symmetric很难穿透
            (Symmetric, Symmetric) => false,
            (Symmetric, _) | (_, Symmetric) => false,

            // Unknown保守处理
            (Unknown, _) | (_, Unknown) => false,
        }
    }

    /// 获取NAT类型的字符串描述
    pub fn description(&self) -> &'static str {
        match self {
            NatType::Open => "Open (No NAT/Firewall)",
            NatType::FullCone => "Full Cone NAT",
            NatType::Restricted => "Restricted NAT",
            NatType::PortRestricted => "Port Restricted NAT",
            NatType::Symmetric => "Symmetric NAT",
            NatType::Unknown => "Unknown",
        }
    }

    /// 获取NAT类型的中文描述
    pub fn description_zh(&self) -> &'static str {
        match self {
            NatType::Open => "公网开放 (无NAT/防火墙)",
            NatType::FullCone => "全锥型 NAT (Full Cone)",
            NatType::Restricted => "受限锥型 NAT (Restricted)",
            NatType::PortRestricted => "端口受限锥型 NAT (Port Restricted)",
            NatType::Symmetric => "对称型 NAT (Symmetric)",
            NatType::Unknown => "未知 NAT",
        }
    }
}

/// 连接状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// 断开连接
    Disconnected,

    /// 正在连接
    Connecting,

    /// P2P直连
    P2pConnected,

    /// 中继连接
    RelayConnected,

    /// 连接失败
    Failed,
}

impl ConnectionState {
    /// 是否已连接
    pub fn is_connected(&self) -> bool {
        matches!(self, Self::P2pConnected | Self::RelayConnected)
    }

    /// 是否使用P2P
    pub fn is_p2p(&self) -> bool {
        matches!(self, Self::P2pConnected)
    }
}

/// 连接质量指标
#[derive(Debug, Clone)]
pub struct ConnectionQuality {
    /// 往返时延 (微秒)
    pub rtt_us: u64,

    /// 丢包率 (0.0-1.0)
    pub loss_rate: f32,

    /// 带宽估计 (bps)
    pub bandwidth_bps: u64,

    /// 最后活动时间
    pub last_activity: std::time::Instant,
}

impl ConnectionQuality {
    /// 创建新的连接质量对象
    pub fn new() -> Self {
        Self {
            rtt_us: 0,
            loss_rate: 0.0,
            bandwidth_bps: 0,
            last_activity: std::time::Instant::now(),
        }
    }

    /// 更新RTT
    pub fn update_rtt(&mut self, rtt_us: u64) {
        // 使用指数加权移动平均 (EWMA)
        if self.rtt_us == 0 {
            self.rtt_us = rtt_us;
        } else {
            self.rtt_us = (self.rtt_us * 7 + rtt_us) / 8;
        }
        self.last_activity = std::time::Instant::now();
    }

    /// 判断连接质量是否足够好
    pub fn is_good_quality(&self, max_rtt_ms: u64, max_loss_rate: f32) -> bool {
        let rtt_ms = self.rtt_us / 1000;
        rtt_ms <= max_rtt_ms && self.loss_rate <= max_loss_rate
    }

    /// 判断连接是否超时
    pub fn is_timeout(&self, timeout_secs: u64) -> bool {
        self.last_activity.elapsed().as_secs() > timeout_secs
    }
}

impl Default for ConnectionQuality {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nat_type_punch_through() {
        assert!(NatType::Open.can_punch_through(&NatType::Symmetric));
        assert!(NatType::FullCone.can_punch_through(&NatType::Restricted));
        assert!(NatType::Restricted.can_punch_through(&NatType::PortRestricted));
        assert!(!NatType::Symmetric.can_punch_through(&NatType::Symmetric));
    }

    #[test]
    fn test_connection_quality() {
        let mut quality = ConnectionQuality::new();
        quality.update_rtt(1000);
        quality.update_rtt(2000);

        assert!(quality.rtt_us > 0);
        assert!(quality.is_good_quality(200, 0.05)); // 200ms, 5%丢包率
    }
}
