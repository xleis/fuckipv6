use fuckipv6_common::{ExitNodeConfig, PeerInfo};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};

/// 出口网关管理器
#[derive(Debug, Clone)]
pub struct ExitNodeManager {
    config: ExitNodeConfig,
}

impl ExitNodeManager {
    /// 创建出口网关管理器
    pub fn new(config: ExitNodeConfig) -> Self {
        Self { config }
    }

    /// 当前节点是否声明为出口网关
    pub fn is_exit_node(&self) -> bool {
        self.config.enabled
    }

    /// 当前节点是否启用了出口网关模式
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// 热更新本机出口网关声明，保留白名单与其他出口配置。
    pub fn set_enabled(&mut self, enabled: bool) {
        self.config.enabled = enabled;
    }

    /// 是否允许获准客户端访问出口节点所在物理局域网。
    pub fn allows_lan_access(&self) -> bool {
        self.config.enabled && self.config.allow_lan_access
    }

    /// 校验指定客户端是否有权限通过当前节点作为出口上网
    pub fn is_client_allowed(&self, client_id: &str) -> bool {
        if !self.config.enabled {
            return false;
        }
        // 若白名单为空，默认允许同一虚拟网内所有已认证对等节点访问
        if self.config.allowed_clients.is_empty() {
            return true;
        }
        self.config.allowed_clients.iter().any(|c| c == client_id)
    }

    /// 根据配置匹配指定的出口节点对等虚拟 IP
    pub fn resolve_exit_node_target(
        &self,
        peers: &HashMap<Ipv6Addr, PeerInfo>,
        configured_target: Option<&str>,
    ) -> Option<Ipv6Addr> {
        let target = configured_target?;
        let target = target.trim();
        if target.is_empty() {
            return None;
        }

        // 1. 尝试直接解析为虚拟 IPv6
        if let Ok(ip) = target.parse::<Ipv6Addr>() {
            if peers.contains_key(&ip) {
                return Some(ip);
            }
        }

        // 2. 尝试解析为虚拟 IPv4 并在 peers 中查找对应的主 IPv6
        if let Ok(v4) = target.parse::<std::net::Ipv4Addr>() {
            for (v6, peer) in peers {
                if peer.virtual_ips.ipv4 == Some(v4) {
                    return Some(*v6);
                }
            }
        }

        // 3. 尝试按 ClientId 查找
        for (v6, peer) in peers {
            if peer.client_id == target {
                return Some(*v6);
            }
        }

        // 4. 如果没有找到精确匹配，但配置了 exit_node，尝试查找带有 exit 标记或默认第一个在线 peer
        None
    }

    /// 从切片列表中匹配指定的出口节点对等虚拟 IP
    pub fn resolve_exit_node_from_slice(
        &self,
        peers: &[PeerInfo],
        configured_target: Option<&str>,
    ) -> Option<Ipv6Addr> {
        let target = configured_target?;
        let target = target.trim();
        if target.is_empty() {
            return None;
        }

        if let Ok(ip) = target.parse::<Ipv6Addr>() {
            if peers.iter().any(|p| p.virtual_ip == ip) {
                return Some(ip);
            }
        }

        if let Ok(v4) = target.parse::<std::net::Ipv4Addr>() {
            for peer in peers {
                if peer.virtual_ips.ipv4 == Some(v4) {
                    return Some(peer.virtual_ip);
                }
            }
        }

        for peer in peers {
            if peer.client_id == target {
                return Some(peer.virtual_ip);
            }
        }

        None
    }

    /// 判断目的地址是否允许由出口网关转发。
    ///
    /// 公网地址默认允许；真实私有局域网仅在 `allow_lan_access` 开启时允许。
    /// 无论配置如何，回环、链路本地、组播、广播、未指定及保留地址始终拒绝。
    /// 调用方仍需先排除虚拟网络地址，避免把 Mesh 流量误判为 Exit 流量。
    pub fn should_forward_as_exit(&self, dst_ip: IpAddr) -> bool {
        if !self.config.enabled {
            return false;
        }

        let safe_unicast = match dst_ip {
            IpAddr::V4(v4) => {
                !v4.is_unspecified()
                    && !v4.is_loopback()
                    && !v4.is_link_local()
                    && !v4.is_broadcast()
                    && !v4.is_multicast()
                    && !v4.is_documentation()
                    && v4.octets()[0] != 0
                    && v4.octets()[0] < 240
            }
            IpAddr::V6(v6) => {
                !(v6.is_unspecified()
                    || v6.is_loopback()
                    || v6.is_multicast()
                    || (v6.segments()[0] & 0xffc0) == 0xfe80
                    || (v6.segments()[0] == 0x2001 && v6.segments()[1] == 0x0db8))
            }
        };
        if !safe_unicast {
            return false;
        }

        if !fuckipv6_common::routing::is_private_or_local(dst_ip) {
            return true;
        }

        if !self.config.allow_lan_access {
            return false;
        }

        match dst_ip {
            IpAddr::V4(v4) => v4.is_private(),
            IpAddr::V6(v6) => (v6.segments()[0] & 0xfe00) == 0xfc00,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuckipv6_common::NatType;

    #[test]
    fn test_exit_node_permission_check() {
        let open_manager = ExitNodeManager::new(ExitNodeConfig {
            enabled: true,
            allowed_clients: Vec::new(),
            allow_lan_access: false,
            region: Some("HK".to_string()),
        });
        assert!(open_manager.is_client_allowed("client-alpha"));

        let restricted_manager = ExitNodeManager::new(ExitNodeConfig {
            enabled: true,
            allowed_clients: vec!["client-alpha".to_string()],
            allow_lan_access: false,
            region: Some("JP".to_string()),
        });
        assert!(restricted_manager.is_client_allowed("client-alpha"));
        assert!(!restricted_manager.is_client_allowed("client-beta"));

        let disabled_manager = ExitNodeManager::new(ExitNodeConfig {
            enabled: false,
            allowed_clients: Vec::new(),
            allow_lan_access: false,
            region: None,
        });
        assert!(!disabled_manager.is_client_allowed("client-alpha"));
    }

    #[test]
    fn exit_forwarding_only_allows_safe_public_destinations_by_default() {
        let manager = ExitNodeManager::new(ExitNodeConfig {
            enabled: true,
            allowed_clients: Vec::new(),
            allow_lan_access: false,
            region: None,
        });

        assert!(manager.should_forward_as_exit("1.1.1.1".parse().unwrap()));
        assert!(manager.should_forward_as_exit("2606:4700:4700::1111".parse().unwrap()));
        assert!(!manager.should_forward_as_exit("192.168.1.1".parse().unwrap()));
        assert!(!manager.should_forward_as_exit("fd12::1".parse().unwrap()));
        assert!(!manager.should_forward_as_exit("127.0.0.1".parse().unwrap()));
        assert!(!manager.should_forward_as_exit("224.0.0.1".parse().unwrap()));
        assert!(!manager.should_forward_as_exit("239.255.255.250".parse().unwrap()));
        assert!(!manager.should_forward_as_exit("ff02::1".parse().unwrap()));
        assert!(!manager.should_forward_as_exit("fe80::1".parse().unwrap()));
        assert!(!manager.should_forward_as_exit("2001:db8::1".parse().unwrap()));
        assert!(!manager.should_forward_as_exit("0.0.0.0".parse().unwrap()));
    }

    #[test]
    fn exit_forwarding_allows_private_lan_only_when_configured() {
        let manager = ExitNodeManager::new(ExitNodeConfig {
            enabled: true,
            allowed_clients: Vec::new(),
            allow_lan_access: true,
            region: None,
        });

        assert!(manager.should_forward_as_exit("10.20.30.40".parse().unwrap()));
        assert!(manager.should_forward_as_exit("192.168.1.10".parse().unwrap()));
        assert!(manager.should_forward_as_exit("fd12::1234".parse().unwrap()));
        assert!(!manager.should_forward_as_exit("169.254.1.1".parse().unwrap()));
        assert!(!manager.should_forward_as_exit("fe80::1".parse().unwrap()));
        assert!(!manager.should_forward_as_exit("224.0.0.1".parse().unwrap()));
    }

    #[test]
    fn test_resolve_exit_node_target() {
        let manager = ExitNodeManager::new(ExitNodeConfig::default());
        let mut peers = HashMap::new();
        let peer_v6 = "fd00::1001".parse::<Ipv6Addr>().unwrap();
        peers.insert(
            peer_v6,
            PeerInfo::new_with_ips(
                "node-relay-hk".to_string(),
                fuckipv6_common::VirtualIps {
                    ipv4: "10.8.0.1".parse().ok(),
                    ipv6: Some(peer_v6),
                },
                vec![0u8; 32],
                Vec::new(),
                NatType::FullCone,
            ),
        );

        // 按 IPv6 查找
        assert_eq!(
            manager.resolve_exit_node_target(&peers, Some("fd00::1001")),
            Some(peer_v6)
        );
        // 按 IPv4 查找
        assert_eq!(
            manager.resolve_exit_node_target(&peers, Some("10.8.0.1")),
            Some(peer_v6)
        );
        // 按 ClientId 查找
        assert_eq!(
            manager.resolve_exit_node_target(&peers, Some("node-relay-hk")),
            Some(peer_v6)
        );
        // 不存在的对端
        assert_eq!(
            manager.resolve_exit_node_target(&peers, Some("node-nonexistent")),
            None
        );
    }
}
