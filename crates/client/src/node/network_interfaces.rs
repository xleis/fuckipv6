use std::collections::{BTreeMap, HashSet};
use std::net::IpAddr;

use getifs::{Flags, Interface};

use super::{
    NodeInterfaceAddress, NodeInterfaceKind, NodeInterfaceOwnership, NodeInterfaceSnapshot,
    NodeInterfaceStatus, NodeProblem, NodeProblemCode,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct InterfaceCapture {
    pub interfaces: Vec<NodeInterfaceSnapshot>,
    pub problems: Vec<NodeProblem>,
}

/// Capture and redact the host interface projection used by every UI adapter.
///
/// `live_tun_names` maps network names to TUN names from the live runtime,
/// not from persisted configuration or the previous snapshot. This makes
/// product ownership an exact, live fact rather than a name-prefix guess.
pub(crate) async fn collect_interfaces(
    live_tun_names: BTreeMap<String, String>,
) -> InterfaceCapture {
    let product_tun_names = live_tun_names.into_values().collect::<HashSet<_>>();
    match tokio::task::spawn_blocking(move || collect_blocking(&product_tun_names)).await {
        Ok(capture) => capture,
        Err(_) => collection_failed("workers"),
    }
}

fn collection_failed(operation: &str) -> InterfaceCapture {
    InterfaceCapture {
        interfaces: Vec::new(),
        problems: vec![
            NodeProblem::new(NodeProblemCode::Unavailable, "无法读取主机网络接口")
                .with_param("operation", operation)
                .retryable(true),
        ],
    }
}

fn collect_blocking(product_tun_names: &HashSet<String>) -> InterfaceCapture {
    let interfaces = match getifs::interfaces() {
        Ok(interfaces) => interfaces,
        Err(_) => return collection_failed("interfaces"),
    };
    let (ipv4_defaults, ipv6_defaults, mut problems) = match getifs::default_interfaces() {
        Ok(defaults) => (
            defaults
                .ipv4()
                .iter()
                .map(|interface| interface.name().to_string())
                .collect::<HashSet<_>>(),
            defaults
                .ipv6()
                .iter()
                .map(|interface| interface.name().to_string())
                .collect::<HashSet<_>>(),
            Vec::new(),
        ),
        Err(_) => (
            HashSet::new(),
            HashSet::new(),
            vec![
                NodeProblem::new(NodeProblemCode::Unavailable, "无法读取主机默认路由接口")
                    .with_param("operation", "default_interfaces")
                    .retryable(true),
            ],
        ),
    };

    let mut captured = interfaces
        .iter()
        .map(|interface| {
            capture_interface(
                interface,
                product_tun_names,
                &ipv4_defaults,
                &ipv6_defaults,
                &mut problems,
            )
        })
        .collect::<Vec<_>>();
    captured.sort_by_key(|interface| interface.index);

    InterfaceCapture {
        interfaces: captured,
        problems,
    }
}

fn capture_interface(
    interface: &Interface,
    product_tun_names: &HashSet<String>,
    ipv4_defaults: &HashSet<String>,
    ipv6_defaults: &HashSet<String>,
    problems: &mut Vec<NodeProblem>,
) -> NodeInterfaceSnapshot {
    let name = interface.name().to_string();
    let flags = interface.flags();
    let (kind, ownership) = classify_interface(
        &name,
        flags.contains(Flags::LOOPBACK),
        flags.contains(Flags::POINTOPOINT),
        product_tun_names.contains(&name),
    );
    let status = if flags.contains(Flags::UP) {
        NodeInterfaceStatus::Up
    } else {
        NodeInterfaceStatus::Down
    };
    let mut addresses = match interface.addrs() {
        Ok(addresses) => addresses
            .into_iter()
            .map(|address| NodeInterfaceAddress {
                address: address.addr(),
                prefix_len: address.prefix_len(),
            })
            .collect::<Vec<_>>(),
        Err(_) => {
            problems.push(
                NodeProblem::new(
                    NodeProblemCode::Unavailable,
                    format!("无法读取网络接口 [{name}] 的地址"),
                )
                .with_param("operation", "interface_addresses")
                .retryable(true),
            );
            Vec::new()
        }
    };
    addresses.sort_by(|left, right| {
        ip_sort_key(left.address)
            .cmp(&ip_sort_key(right.address))
            .then(left.prefix_len.cmp(&right.prefix_len))
    });

    NodeInterfaceSnapshot {
        index: interface.index(),
        name: name.clone(),
        kind,
        ownership,
        status,
        mtu: interface.mtu(),
        addresses,
        default_ipv4: ipv4_defaults.contains(&name),
        default_ipv6: ipv6_defaults.contains(&name),
    }
}

fn ip_sort_key(address: IpAddr) -> (u8, [u8; 16]) {
    match address {
        IpAddr::V4(address) => {
            let mut bytes = [0_u8; 16];
            bytes[..4].copy_from_slice(&address.octets());
            (4, bytes)
        }
        IpAddr::V6(address) => (6, address.octets()),
    }
}

fn classify_interface(
    name: &str,
    is_loopback: bool,
    is_point_to_point: bool,
    is_live_product_tun: bool,
) -> (NodeInterfaceKind, NodeInterfaceOwnership) {
    if is_live_product_tun {
        return (NodeInterfaceKind::Tun, NodeInterfaceOwnership::Product);
    }
    if is_loopback {
        return (NodeInterfaceKind::Loopback, NodeInterfaceOwnership::System);
    }

    let normalized = name.to_ascii_lowercase();
    let has_virtual_prefix = ["tun", "tap", "utun", "wg", "veth", "docker", "br-", "virbr"]
        .iter()
        .any(|prefix| normalized.starts_with(prefix));
    let kind = if is_point_to_point || has_virtual_prefix {
        NodeInterfaceKind::Virtual
    } else {
        NodeInterfaceKind::Physical
    };
    (kind, NodeInterfaceOwnership::System)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_live_tun_names_establish_product_ownership() {
        assert_eq!(
            classify_interface("fk-home", false, true, true),
            (NodeInterfaceKind::Tun, NodeInterfaceOwnership::Product)
        );
        assert_eq!(
            classify_interface("fk-home", false, true, false),
            (NodeInterfaceKind::Virtual, NodeInterfaceOwnership::System)
        );
        assert_eq!(
            classify_interface("fuckipv6-home", false, false, false),
            (NodeInterfaceKind::Physical, NodeInterfaceOwnership::System)
        );
    }

    #[test]
    fn loopback_and_virtual_prefixes_are_classified_without_name_guessing_product_ownership() {
        assert_eq!(
            classify_interface("lo", true, false, false),
            (NodeInterfaceKind::Loopback, NodeInterfaceOwnership::System)
        );
        for name in [
            "tun0", "tap0", "utun3", "wg0", "veth12", "docker0", "br-a1", "virbr0",
        ] {
            assert_eq!(
                classify_interface(name, false, false, false),
                (NodeInterfaceKind::Virtual, NodeInterfaceOwnership::System),
                "{name}"
            );
        }
        assert_eq!(
            classify_interface("enp3s0", false, false, false),
            (NodeInterfaceKind::Physical, NodeInterfaceOwnership::System)
        );
    }

    #[test]
    fn interface_projection_serializes_no_hardware_or_route_details() {
        let interface = NodeInterfaceSnapshot {
            index: 2,
            name: "lo".into(),
            kind: NodeInterfaceKind::Loopback,
            ownership: NodeInterfaceOwnership::System,
            status: NodeInterfaceStatus::Up,
            mtu: 65_536,
            addresses: vec![NodeInterfaceAddress {
                address: "127.0.0.1".parse().expect("IPv4 address"),
                prefix_len: 8,
            }],
            default_ipv4: false,
            default_ipv6: false,
        };
        let value = serde_json::to_value(interface).expect("serialize interface");
        let object = value.as_object().expect("interface object");
        let keys = object.keys().cloned().collect::<HashSet<_>>();
        assert_eq!(
            keys,
            [
                "addresses",
                "default_ipv4",
                "default_ipv6",
                "index",
                "kind",
                "mtu",
                "name",
                "ownership",
                "status",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect::<HashSet<_>>()
        );
        assert_eq!(value["kind"], "loopback");
        assert_eq!(value["ownership"], "system");
    }
}
