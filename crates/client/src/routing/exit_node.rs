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

    /// 判断目的 IP 是否属于应该由出口网关转发的公网流量
    pub fn should_forward_as_exit(&self, dst_ip: IpAddr) -> bool {
        if !self.config.enabled {
            return false;
        }
        // 目标不能是私有局域网、回环、广播、链路本地地址
        !fuckipv6_common::routing::is_private_or_local(dst_ip)
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
