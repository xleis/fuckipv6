//! Derivation of the host VPN configuration from the authoritative network
//! request. The node keeps ownership of addressing policy; this module only
//! translates it into the iOS tunnel contract and rejects unsafe combinations
//! before any system configuration is applied.

use std::net::IpAddr;

use fuckipv6_client::embedded::EmbeddedNetworkRequest;
use fuckipv6_client::node::{NodeProblem, NodeProblemCode};
use fuckipv6_common::RoutingMode;
use ipnet::{Ipv4Net, Ipv6Net};
use serde::Serialize;

/// Default virtual IPv4 network used by the desktop node for every network.
/// Kept identical so a mesh address means the same thing on every platform.
const DEFAULT_IPV4_NETWORK: &str = "10.8.0.0/24";
/// Apple rejects tunnel MTUs below the IPv6 minimum link MTU.
const MIN_MTU: u16 = 1280;
/// Upper bound accepted by `NEPacketTunnelNetworkSettings`.
const MAX_MTU: u16 = 65535;

/// A single host address with its network prefix length.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostAddress {
    pub address: String,
    pub prefix: u8,
}

/// Everything the Packet Tunnel extension needs to configure its session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TunnelSettings {
    pub network: String,
    pub mtu: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv4: Option<HostAddress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv6: Option<HostAddress>,
    /// Mesh-only routes. Never a default route.
    pub routes: Vec<String>,
    /// Resolver addresses to hand to the system.
    pub dns: Vec<String>,
    /// Real transport endpoint, reported for diagnostics only. It is never
    /// routed into the tunnel.
    pub transport: String,
}

impl TunnelSettings {
    /// Translates one network request into host settings.
    ///
    /// Rejections are returned as typed `NodeProblem`s so the extension fails
    /// startup with the same stable codes the desktop adapter uses.
    pub fn derive(request: &EmbeddedNetworkRequest) -> Result<Self, NodeProblem> {
        if request.config.routing.mode != RoutingMode::MeshOnly {
            return Err(problem(
                NodeProblemCode::Unsupported,
                format!(
                    "iOS tunnel supports MeshOnly routing only, got {:?}",
                    request.config.routing.mode
                ),
            ));
        }
        let mtu = request.config.tun.mtu;
        if !(MIN_MTU..=MAX_MTU).contains(&mtu) {
            return Err(problem(
                NodeProblemCode::InvalidRequest,
                format!("iOS tunnel MTU {mtu} is outside {MIN_MTU}..={MAX_MTU}"),
            ));
        }
        let ipv4_network: Ipv4Net = DEFAULT_IPV4_NETWORK.parse().map_err(|error| {
            problem(
                NodeProblemCode::Internal,
                format!("invalid built-in IPv4 pool: {error}"),
            )
        })?;
        let ipv6_network =
            match (
                request.config.network.virtual_network.as_deref(),
                request.assigned_ips.ipv6,
            ) {
                (Some(cidr), _) => Some(cidr.trim().parse::<Ipv6Net>().map_err(|error| {
                    problem(
                        NodeProblemCode::InvalidRequest,
                        format!("invalid virtual IPv6 network {cidr:?}: {error}"),
                    )
                })?),
                (None, Some(_)) => return Err(problem(
                    NodeProblemCode::InvalidRequest,
                    "node assigned an IPv6 address but the network has no virtual_network prefix",
                )),
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
                ))
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
                ))
            }
            (Some(_), None) => unreachable!("handled above"),
            (None, _) => None,
        };
        if ipv4.is_none() && ipv6.is_none() {
            return Err(problem(
                NodeProblemCode::Unavailable,
                "node assigned no virtual address, the tunnel cannot start",
            ));
        }

        let mut routes = Vec::new();
        if ipv4.is_some() {
            routes.push(ipv4_network.to_string());
        }
        if ipv6.is_some() {
            if let Some(network) = ipv6_network {
                routes.push(network.to_string());
            }
        }
        if routes.is_empty() {
            return Err(problem(
                NodeProblemCode::Unavailable,
                "no mesh route could be derived for the tunnel",
            ));
        }

        let transport = request.transport_addr.to_string();
        for route in &routes {
            let network = route.parse::<ipnet::IpNet>().map_err(|error| {
                problem(
                    NodeProblemCode::Internal,
                    format!("invalid route {route}: {error}"),
                )
            })?;
            if network.contains(&request.transport_addr.ip()) {
                return Err(problem(
                    NodeProblemCode::InvalidRequest,
                    format!(
                        "transport endpoint {} falls inside tunnel route {route}; refusing to create a routing loop",
                        request.transport_addr
                    ),
                ));
            }
        }

        Ok(Self {
            network: request.network_name.clone(),
            mtu,
            ipv4,
            ipv6,
            routes,
            dns: request.dns_servers.iter().map(IpAddr::to_string).collect(),
            transport,
        })
    }
}

fn problem(code: NodeProblemCode, message: impl Into<String>) -> NodeProblem {
    NodeProblem::new(code, message)
}
