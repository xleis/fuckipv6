//! Derivation of Android `VpnService.Builder` settings from the authoritative
//! [`EmbeddedNetworkRequest`].
//!
//! The Rust node retains ownership of addressing, routing mode and DNS policy;
//! Android applies these settings through `VpnService.Builder` and excludes its
//! own package (`addDisallowedApplication`) so the node's own underlay sockets
//! never loop back into the TUN.

use std::net::IpAddr;

use fuckipv6_client::embedded::EmbeddedNetworkRequest;
use fuckipv6_client::node::{NodeProblem, NodeProblemCode};
use fuckipv6_common::RoutingMode;
use ipnet::{Ipv4Net, Ipv6Net};
use serde::{Deserialize, Serialize};

/// Default virtual IPv4 network shared with desktop and iOS nodes.
const DEFAULT_IPV4_NETWORK: &str = "10.8.0.0/24";
/// Minimum dual-stack MTU accepted by Android `VpnService.Builder.setMtu`.
const MIN_MTU: u16 = 1280;
/// Maximum MTU accepted by Android tunnel configuration.
const MAX_MTU: u16 = 9000;

/// Default recursive resolvers applied inside the tunnel when Exit routing is
/// enabled and the operator did not specify custom resolvers, ensuring DNS
/// queries enter the TUN and traverse the Exit rather than leaking via ISP DNS.
const DEFAULT_EXIT_DNS: [&str; 2] = ["1.1.1.1", "8.8.8.8"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostAddress {
    pub address: String,
    pub prefix: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AndroidTunnelSettings {
    pub network: String,
    pub mtu: u16,
    pub routing_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv4: Option<HostAddress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv6: Option<HostAddress>,
    pub routes: Vec<String>,
    pub dns: Vec<String>,
    /// Always true so the app's own QUIC/UDP/TCP/WS sockets bypass Android VPN.
    pub disallow_self_package: bool,
    pub transport: String,
}

impl AndroidTunnelSettings {
    pub fn derive(request: &EmbeddedNetworkRequest) -> Result<Self, NodeProblem> {
        let mtu = request.config.tun.mtu;
        if !(MIN_MTU..=MAX_MTU).contains(&mtu) {
            return Err(problem(
                NodeProblemCode::InvalidRequest,
                format!("Android tunnel MTU {mtu} is outside {MIN_MTU}..={MAX_MTU}"),
            ));
        }

        let ipv4_network: Ipv4Net = DEFAULT_IPV4_NETWORK.parse().map_err(|error| {
            problem(
                NodeProblemCode::Internal,
                format!("invalid built-in IPv4 pool: {error}"),
            )
        })?;
        let ipv6_network = match (
            request.config.network.virtual_network.as_deref(),
            request.assigned_ips.ipv6,
        ) {
            (Some(cidr), _) => Some(cidr.trim().parse::<Ipv6Net>().map_err(|error| {
                problem(
                    NodeProblemCode::InvalidRequest,
                    format!("invalid virtual IPv6 network {cidr:?}: {error}"),
                )
            })?),
            (None, Some(_)) => {
                return Err(problem(
                    NodeProblemCode::InvalidRequest,
                    "node assigned an IPv6 address but the network has no virtual_network prefix",
                ));
            }
            (None, None) => None,
        };

        let ipv4 = match request.assigned_ips.ipv4 {
            Some(address) if ipv4_network.contains(&address) => Some(HostAddress {
                address: address.to_string(),
                prefix: ipv4_network.prefix_len(),
            }),
            Some(address) => {
                return Err(problem(
                    NodeProblemCode::InvalidRequest,
                    format!("assigned IPv4 {address} is outside {ipv4_network}"),
                ));
            }
            None => None,
        };

        let ipv6 = match (request.assigned_ips.ipv6, ipv6_network) {
            (Some(address), Some(network)) if network.contains(&address) => Some(HostAddress {
                address: address.to_string(),
                prefix: network.prefix_len(),
            }),
            (Some(address), Some(network)) => {
                return Err(problem(
                    NodeProblemCode::InvalidRequest,
                    format!("assigned IPv6 {address} is outside {network}"),
                ));
            }
            (Some(_), None) => unreachable!("handled above"),
            (None, _) => None,
        };

        if ipv4.is_none() && ipv6.is_none() {
            return Err(problem(
                NodeProblemCode::Unavailable,
                "node assigned no virtual address, Android tunnel cannot start",
            ));
        }

        let mut routes = Vec::new();
        let exit_routing = request.config.routing.mode != RoutingMode::MeshOnly;
        if exit_routing {
            if ipv4.is_some() {
                routes.push("0.0.0.0/0".to_owned());
            }
            if ipv6.is_some() {
                routes.push("::/0".to_owned());
            }
        } else {
            if ipv4.is_some() {
                routes.push(ipv4_network.to_string());
            }
            if ipv6.is_some() {
                if let Some(network) = ipv6_network {
                    routes.push(network.to_string());
                }
            }
        }

        if routes.is_empty() {
            return Err(problem(
                NodeProblemCode::Unavailable,
                "no tunnel routes could be derived for Android VpnService",
            ));
        }

        let dns = if !request.dns_servers.is_empty() {
            request.dns_servers.iter().map(IpAddr::to_string).collect()
        } else if exit_routing {
            DEFAULT_EXIT_DNS
                .iter()
                .map(|value| (*value).to_owned())
                .collect()
        } else {
            Vec::new()
        };

        let routing_mode = match request.config.routing.mode {
            RoutingMode::MeshOnly => "mesh_only",
            RoutingMode::BypassMainland => "bypass_mainland",
            RoutingMode::GlobalProxy => "global_proxy",
        }
        .to_owned();

        Ok(Self {
            network: request.network_name.clone(),
            mtu,
            routing_mode,
            ipv4,
            ipv6,
            routes,
            dns,
            disallow_self_package: true,
            transport: request.transport_addr.to_string(),
        })
    }
}

fn problem(code: NodeProblemCode, message: impl Into<String>) -> NodeProblem {
    NodeProblem::new(code, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuckipv6_common::{ClientConfig, VirtualIps};
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

    fn sample_request(mode: RoutingMode) -> EmbeddedNetworkRequest {
        let mut config = ClientConfig::from_toml_str(
            r#"
coordinator_url = "ws://127.0.0.1:8080/ws"
[tun]
mtu = 1400
[network]
virtual_network = "fd00:1234::/64"
"#,
        )
        .expect("parse sample client config");
        config.routing.mode = mode;
        EmbeddedNetworkRequest {
            network_name: "home".to_owned(),
            assigned_ips: VirtualIps {
                ipv4: Some(Ipv4Addr::new(10, 8, 0, 5)),
                ipv6: Some("fd00:1234::5".parse::<Ipv6Addr>().unwrap()),
            },
            config,
            transport_addr: "127.0.0.1:41234".parse::<SocketAddr>().unwrap(),
            dns_servers: Vec::new(),
        }
    }

    #[test]
    fn mesh_only_derives_mesh_routes_without_default_route() {
        let settings = AndroidTunnelSettings::derive(&sample_request(RoutingMode::MeshOnly))
            .expect("mesh settings");
        assert_eq!(settings.routing_mode, "mesh_only");
        assert_eq!(settings.routes, vec!["10.8.0.0/24", "fd00:1234::/64"]);
        assert!(settings.dns.is_empty());
        assert!(settings.disallow_self_package);
    }

    #[test]
    fn global_proxy_derives_default_routes_and_tunnel_dns() {
        let settings = AndroidTunnelSettings::derive(&sample_request(RoutingMode::GlobalProxy))
            .expect("global proxy settings");
        assert_eq!(settings.routing_mode, "global_proxy");
        assert_eq!(settings.routes, vec!["0.0.0.0/0", "::/0"]);
        assert_eq!(settings.dns, vec!["1.1.1.1", "8.8.8.8"]);
        assert!(settings.disallow_self_package);
    }

    #[test]
    fn rejects_mtu_outside_bounds() {
        let mut request = sample_request(RoutingMode::MeshOnly);
        request.config.tun.mtu = 1200;
        let error = AndroidTunnelSettings::derive(&request).expect_err("mtu rejected");
        assert_eq!(error.code, NodeProblemCode::InvalidRequest);
    }
}
