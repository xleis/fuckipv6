use ipnet::{IpNet, Ipv4Net, Ipv6Net};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// 路由分流动作
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoutingAction {
    /// 直连（走本地物理网卡默认网关）
    Direct,
    /// 代理（走指定的 Exit Node 出口网关）
    Proxy,
    /// 虚拟局域网（走 P2P / 中继对等网络）
    Mesh,
    /// 阻断（丢弃数据包）
    Reject,
}

/// 客户端智能分流模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RoutingMode {
    /// 仅虚拟局域网（默认模式，不接管公网出口）
    #[default]
    MeshOnly,
    /// 绕过中国大陆（大陆 IP 直连，其余境外与非局域网流量走 Exit Node）
    BypassMainland,
    /// 全局代理（除本地局域网与虚拟网外，所有公网流量均走 Exit Node）
    GlobalProxy,
}

/// 智能分流与出口网关客户端配置
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RoutingConfig {
    /// 路由分流模式 (MeshOnly, BypassMainland, GlobalProxy)
    #[serde(default)]
    pub mode: RoutingMode,

    /// 指定使用的出口节点（支持虚拟 IPv6、虚拟 IPv4 或 ClientId）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_node: Option<String>,

    /// 自定义直连 CIDR 列表
    #[serde(default)]
    pub direct_cidrs: Vec<String>,

    /// 自定义代理 CIDR 列表
    #[serde(default)]
    pub proxy_cidrs: Vec<String>,
}

/// 出口网关（Exit Node）声明配置
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ExitNodeConfig {
    /// 是否开启作为出口网关
    pub enabled: bool,
    /// 允许访问出口的客户端公钥指纹白名单（为空则允许所有同网节点）
    #[serde(default)]
    pub allowed_clients: Vec<String>,
    /// 是否允许访问出口所在节点的局域网设备 (如 192.168.1.0/24)
    #[serde(default)]
    pub allow_lan_access: bool,
    /// 出口节点地理位置标识（如 "HK", "US", "JP" 等，可选）
    #[serde(default)]
    pub region: Option<String>,
}

/// 判断 IP 是否为私有、链路本地或回环地址
pub fn is_private_or_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64) // 100.64.0.0/10 CGNAT
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || ((v6.segments()[0] & 0xfe00) == 0xfc00) // Unique Local (fc00::/7)
                || ((v6.segments()[0] & 0xffc0) == 0xfe80) // Link-Local (fe80::/10)
                || v6.is_multicast()
        }
    }
}

/// 核心大段中国大陆常用 IPv4 前缀基数表（包含三大运营商主要骨干网聚合网段）
/// 纯 Rust 内嵌快速判定，零外部依赖
static CN_IPV4_PREFIXES: &[(u32, u8)] = &[
    (0x01_00_01_00, 24), // 1.0.1.0/24
    (0x01_00_02_00, 23), // 1.0.2.0/23
    (0x0e_00_00_00, 8),  // 14.0.0.0/8 (广东电信等)
    (0x1b_00_00_00, 8),  // 27.0.0.0/8 (中国电信/联通)
    (0x24_00_00_00, 8),  // 36.0.0.0/8 (中国移动/联通)
    (0x27_00_00_00, 8),  // 39.0.0.0/8 (中国移动)
    (0x2a_00_00_00, 8),  // 42.0.0.0/8 (中国电信)
    (0x31_00_00_00, 8),  // 49.0.0.0/8 (中国联通)
    (0x3a_00_00_00, 8),  // 58.0.0.0/8 (中国电信)
    (0x3b_00_00_00, 8),  // 59.0.0.0/8 (中国电信)
    (0x3c_00_00_00, 8),  // 60.0.0.0/8 (中国电信)
    (0x3d_00_00_00, 8),  // 61.0.0.0/8 (中国电信)
    (0x65_00_00_00, 8),  // 101.0.0.0/8 (中国电信/联通)
    (0x67_00_00_00, 8),  // 103.0.0.0/8
    (0x6a_00_00_00, 8),  // 106.0.0.0/8 (中国移动/联通)
    (0x70_00_00_00, 8),  // 112.0.0.0/8 (中国移动)
    (0x71_00_00_00, 8),  // 113.0.0.0/8 (中国电信)
    (0x72_00_00_00, 8),  // 114.0.0.0/8 (中国电信，含 114.114.114.114)
    (0x73_00_00_00, 8),  // 115.0.0.0/8 (中国电信)
    (0x74_00_00_00, 8),  // 116.0.0.0/8 (中国联通)
    (0x75_00_00_00, 8),  // 117.0.0.0/8 (中国移动/电信)
    (0x76_00_00_00, 8),  // 118.0.0.0/8 (中国电信)
    (0x77_00_00_00, 8),  // 119.0.0.0/8 (中国电信)
    (0x78_00_00_00, 8),  // 120.0.0.0/8 (中国移动/电信)
    (0x79_00_00_00, 8),  // 121.0.0.0/8 (中国电信)
    (0x7a_00_00_00, 8),  // 122.0.0.0/8 (中国联通)
    (0x7b_00_00_00, 8),  // 123.0.0.0/8 (中国联通)
    (0x7c_00_00_00, 8),  // 124.0.0.0/8 (中国电信)
    (0x7d_00_00_00, 8),  // 125.0.0.0/8 (中国电信)
    (0xac_00_00_00, 7),  // 171.0.0.0/7 (中国电信/联通 171.0.0.0 - 172.0.0.0 剔除私有段)
    (0xb4_00_00_00, 8),  // 180.0.0.0/8 (中国电信)
    (0xb6_00_00_00, 8),  // 182.0.0.0/8 (中国电信/移动)
    (0xb7_00_00_00, 8),  // 183.0.0.0/8 (中国电信/移动)
    (0xda_00_00_00, 8),  // 218.0.0.0/8 (中国电信)
    (0xdb_00_00_00, 8),  // 219.0.0.0/8 (中国电信)
    (0xdc_00_00_00, 8),  // 220.0.0.0/8 (中国电信)
    (0xdd_00_00_00, 8),  // 221.0.0.0/8 (中国电信)
    (0xde_00_00_00, 8),  // 222.0.0.0/8 (中国电信)
    (0xdf_00_00_00, 8),  // 223.0.0.0/8 (中国电信/移动，含 223.5.5.5 阿里 DNS)
];

/// 核心大段中国大陆常用 IPv6 前缀基数表
static CN_IPV6_PREFIXES: &[(u128, u8)] = &[
    (0x2408_0000_0000_0000_0000_0000_0000_0000, 16), // 2408::/16 (中国联通)
    (0x2409_0000_0000_0000_0000_0000_0000_0000, 16), // 2409::/16 (中国移动)
    (0x240e_0000_0000_0000_0000_0000_0000_0000, 16), // 240e::/16 (中国电信)
    (0x240a_0000_0000_0000_0000_0000_0000_0000, 16), // 240a::/16 (CSTNET/CNNIC)
    (0x2001_0250_0000_0000_0000_0000_0000_0000, 32), // 2001:250::/32 (CERNET 教育网)
    (0x2400_3200_0000_0000_0000_0000_0000_0000, 32), // 2400:3200::/32 (阿里云 DNS)
    (0x2402_4e00_0000_0000_0000_0000_0000_0000, 32), // 2402:4e00::/32 (腾讯云 DNS)
];

/// 返回内置的中国大陆 IPv4/IPv6 直连网段。
///
/// 该列表与 [`is_mainland_china_ip`] 使用同一份前缀数据。系统路由接管
/// 需要把这些网段明确旁路到物理网络，避免仅靠用户态判定时丢包。
pub fn mainland_china_cidrs() -> Vec<IpNet> {
    let mut networks = Vec::with_capacity(CN_IPV4_PREFIXES.len() + CN_IPV6_PREFIXES.len());

    for &(prefix, prefix_len) in CN_IPV4_PREFIXES {
        if let Ok(network) = Ipv4Net::new(Ipv4Addr::from(prefix), prefix_len) {
            networks.push(IpNet::V4(network));
        }
    }
    for &(prefix, prefix_len) in CN_IPV6_PREFIXES {
        if let Ok(network) = Ipv6Net::new(Ipv6Addr::from(prefix), prefix_len) {
            networks.push(IpNet::V6(network));
        }
    }

    networks
}

/// 判断指定 IP 是否属于中国大陆 IP 地址段
pub fn is_mainland_china_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            if is_private_or_local(IpAddr::V4(v4)) {
                return false;
            }
            let ip_u32 = u32::from(v4);
            for &(prefix, mask_len) in CN_IPV4_PREFIXES {
                let mask = if mask_len == 0 {
                    0
                } else {
                    !((1u32 << (32 - mask_len)) - 1)
                };
                if (ip_u32 & mask) == (prefix & mask) {
                    return true;
                }
            }
            false
        }
        IpAddr::V6(v6) => {
            if is_private_or_local(IpAddr::V6(v6)) {
                return false;
            }
            let ip_u128 = u128::from(v6);
            for &(prefix, mask_len) in CN_IPV6_PREFIXES {
                let mask = if mask_len == 0 {
                    0
                } else {
                    !((1u128 << (128 - mask_len)) - 1)
                };
                if (ip_u128 & mask) == (prefix & mask) {
                    return true;
                }
            }
            false
        }
    }
}

/// 智能分流路由规则判定引擎
#[derive(Debug, Clone)]
pub struct SmartRouteEngine {
    mode: RoutingMode,
    virtual_network_v6: Option<Ipv6Net>,
    virtual_network_v4: Option<Ipv4Net>,
    custom_direct_cidrs: Vec<IpNet>,
    custom_proxy_cidrs: Vec<IpNet>,
}

impl SmartRouteEngine {
    /// 创建分流判定引擎
    pub fn new(mode: RoutingMode) -> Self {
        Self {
            mode,
            virtual_network_v6: "fd00::/64".parse().ok(),
            virtual_network_v4: "10.8.0.0/24".parse().ok(),
            custom_direct_cidrs: Vec::new(),
            custom_proxy_cidrs: Vec::new(),
        }
    }

    /// 获取当前路由分流模式
    pub fn routing_mode(&self) -> RoutingMode {
        self.mode
    }

    /// 热更新分流模式，不影响已配置的自定义 CIDR 与虚拟网络前缀。
    pub fn set_routing_mode(&mut self, mode: RoutingMode) {
        self.mode = mode;
    }

    /// 设置虚拟网络前缀（用于准确判定 Mesh 流量）
    pub fn set_virtual_networks(&mut self, v6: Option<Ipv6Net>, v4: Option<Ipv4Net>) {
        self.virtual_network_v6 = v6;
        self.virtual_network_v4 = v4;
    }

    /// 添加自定义直连 CIDR
    pub fn add_direct_cidr(&mut self, cidr: IpNet) {
        self.custom_direct_cidrs.push(cidr);
    }

    /// 添加自定义代理 CIDR
    pub fn add_proxy_cidr(&mut self, cidr: IpNet) {
        self.custom_proxy_cidrs.push(cidr);
    }

    /// 核心分流决策算法
    pub fn evaluate(&self, dst_ip: IpAddr) -> RoutingAction {
        // 1. 本地回环与多播地址直接直连
        if is_private_or_local(dst_ip) {
            // 如果目标属于虚拟内网网段，判定为 Mesh
            if let Some(v6_net) = &self.virtual_network_v6 {
                if let IpAddr::V6(v6) = dst_ip {
                    if v6_net.contains(&v6) {
                        return RoutingAction::Mesh;
                    }
                }
            }
            if let Some(v4_net) = &self.virtual_network_v4 {
                if let IpAddr::V4(v4) = dst_ip {
                    if v4_net.contains(&v4) {
                        return RoutingAction::Mesh;
                    }
                }
            }
            // 其余本地局域网私有地址走本地直连
            return RoutingAction::Direct;
        }

        // 2. 自定义优先规则匹配
        for cidr in &self.custom_direct_cidrs {
            if cidr.contains(&dst_ip) {
                return RoutingAction::Direct;
            }
        }
        for cidr in &self.custom_proxy_cidrs {
            if cidr.contains(&dst_ip) {
                return RoutingAction::Proxy;
            }
        }

        // 3. 根据当前分流模式判定
        match self.mode {
            RoutingMode::MeshOnly => {
                // 仅局域网模式下，公网流量不处理 (Direct)
                RoutingAction::Direct
            }
            RoutingMode::GlobalProxy => {
                // 全局代理模式下，所有公网流量均走 Exit Node
                RoutingAction::Proxy
            }
            RoutingMode::BypassMainland => {
                // 绕过大陆模式：大陆 IP 直连，其余境外公网 IP 走 Exit Node
                if is_mainland_china_ip(dst_ip) {
                    RoutingAction::Direct
                } else {
                    RoutingAction::Proxy
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_mode_can_be_hot_updated() {
        let mut engine = SmartRouteEngine::new(RoutingMode::MeshOnly);
        assert_eq!(
            engine.evaluate("8.8.8.8".parse().unwrap()),
            RoutingAction::Direct
        );
        engine.set_routing_mode(RoutingMode::GlobalProxy);
        assert_eq!(
            engine.evaluate("8.8.8.8".parse().unwrap()),
            RoutingAction::Proxy
        );
    }

    #[test]
    fn test_mainland_china_ip_detection() {
        // 114.114.114.114 (南京电信)
        assert!(is_mainland_china_ip("114.114.114.114".parse().unwrap()));
        // 223.5.5.5 (阿里公共 DNS)
        assert!(is_mainland_china_ip("223.5.5.5".parse().unwrap()));
        // 240e:: (中国电信 IPv6)
        assert!(is_mainland_china_ip("240e:1f:1::1".parse().unwrap()));
        // 2408:: (中国联通 IPv6)
        assert!(is_mainland_china_ip("2408:8220::1".parse().unwrap()));

        // Google DNS 8.8.8.8 (海外)
        assert!(!is_mainland_china_ip("8.8.8.8".parse().unwrap()));
        // Cloudflare DNS 1.1.1.1 (海外)
        assert!(!is_mainland_china_ip("1.1.1.1".parse().unwrap()));
        // Google IPv6 2001:4860:4860::8888 (海外)
        assert!(!is_mainland_china_ip(
            "2001:4860:4860::8888".parse().unwrap()
        ));
    }

    #[test]
    fn mainland_china_cidrs_match_detection() {
        let networks = mainland_china_cidrs();
        assert!(networks
            .iter()
            .any(|network| network.contains(&"114.114.114.114".parse::<IpAddr>().unwrap())));
        assert!(networks
            .iter()
            .any(|network| network.contains(&"240e:1f:1::1".parse::<IpAddr>().unwrap())));
        assert!(!networks
            .iter()
            .any(|network| network.contains(&"8.8.8.8".parse::<IpAddr>().unwrap())));
    }

    #[test]
    fn test_smart_routing_evaluation() {
        let engine = SmartRouteEngine::new(RoutingMode::BypassMainland);

        // 虚拟局域网内的对端 IP -> Mesh
        assert_eq!(
            engine.evaluate("fd00::1001".parse().unwrap()),
            RoutingAction::Mesh
        );

        // 本地私有局域网 IP -> Direct
        assert_eq!(
            engine.evaluate("192.168.1.1".parse().unwrap()),
            RoutingAction::Direct
        );

        // 大陆公网 IP -> Direct
        assert_eq!(
            engine.evaluate("114.114.114.114".parse().unwrap()),
            RoutingAction::Direct
        );

        // 境外公网 IP -> Proxy (走 Exit Node)
        assert_eq!(
            engine.evaluate("8.8.8.8".parse().unwrap()),
            RoutingAction::Proxy
        );
        assert_eq!(
            engine.evaluate("2607:f8b0:4005:800::200e".parse().unwrap()),
            RoutingAction::Proxy
        );
    }

    #[test]
    fn test_global_proxy_mode() {
        let engine = SmartRouteEngine::new(RoutingMode::GlobalProxy);

        // 局域网虚拟 IP 依然走 Mesh
        assert_eq!(
            engine.evaluate("fd00::1005".parse().unwrap()),
            RoutingAction::Mesh
        );

        // 全局模式下，哪怕是国内 IP 也走 Proxy
        assert_eq!(
            engine.evaluate("114.114.114.114".parse().unwrap()),
            RoutingAction::Proxy
        );
        assert_eq!(
            engine.evaluate("8.8.8.8".parse().unwrap()),
            RoutingAction::Proxy
        );
    }
}
