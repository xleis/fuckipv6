use fuckipv6_common::{AggregatedLogEntry, ClientConfig, Identity, RoutingMode};

use super::{
    NodeAccessModel, NodeIdentitySnapshot, NodeLogEntry, NodeLogLevel, NodeNetworkConfigSnapshot,
    NodePermission, NodePermissionSnapshot, NodeRoutingMode,
};

/// Keep the shared snapshot bounded when logs are refreshed from the runtime.
pub(crate) const NODE_LOG_LIMIT: usize = 256;

/// Project only public identity metadata into the transport-neutral node snapshot.
///
/// The source `Identity` owns signing material, but this DTO deliberately contains
/// only the node id, an optional display name, and the public-key fingerprint.
pub(crate) fn identity_snapshot(identity: &Identity) -> NodeIdentitySnapshot {
    NodeIdentitySnapshot {
        node_id: identity.client_id().to_string(),
        display_name: None,
        public_key_fingerprint: Identity::public_key_fingerprint(&identity.public_key_bytes()),
    }
}

/// Read the current bounded WARN/ERROR aggregation and convert it for state consumers.
pub(crate) fn recent_log_entries() -> Vec<NodeLogEntry> {
    log_entries(fuckipv6_common::recent_error_logs(NODE_LOG_LIMIT))
}

/// Project effective network configuration without exposing host-local paths.
pub(crate) fn network_config_snapshot(
    network_name: &str,
    config: &ClientConfig,
) -> NodeNetworkConfigSnapshot {
    NodeNetworkConfigSnapshot {
        network_name: network_name.to_string(),
        coordinator_url: config.coordinator_url.clone(),
        virtual_mode: config.network.mode,
        preferred_ipv4: config.network.preferred_ipv4,
        preferred_ipv6: config.network.preferred_ipv6,
        virtual_network: config.network.virtual_network.clone(),
        tun_name: config.tun.name.clone(),
        mtu: config.tun.mtu,
        p2p_listen_port: config.p2p.listen_port,
        p2p_connect_timeout_secs: config.p2p.connect_timeout,
        p2p_heartbeat_interval_secs: config.p2p.heartbeat_interval,
        routing_mode: match config.routing.mode {
            RoutingMode::MeshOnly => NodeRoutingMode::MeshOnly,
            RoutingMode::BypassMainland => NodeRoutingMode::BypassMainland,
            RoutingMode::GlobalProxy => NodeRoutingMode::GlobalProxy,
        },
        exit_node: config.routing.exit_node.clone(),
        direct_cidrs: config.routing.direct_cidrs.clone(),
        proxy_cidrs: config.routing.proxy_cidrs.clone(),
        exit_node_enabled: config.exit_node.enabled,
        exit_node_allowed_clients: config.exit_node.allowed_clients.clone(),
        exit_node_allow_lan_access: config.exit_node.allow_lan_access,
        exit_node_region: config.exit_node.region.clone(),
        acl_enabled: config.acl.enabled,
        acl_default_action: config.acl.default_action,
        acl_rule_count: config.acl.rules.len(),
        log_level: config.log.level.clone(),
        log_file_configured: config.log.file.is_some(),
        identity_configured: !config.private_key_path.as_os_str().is_empty(),
    }
}

/// The current control plane has no per-principal authorization. Publish that
/// limitation explicitly instead of implying that remote authentication exists.
pub(crate) fn trusted_control_plane_permissions() -> NodePermissionSnapshot {
    NodePermissionSnapshot {
        access_model: NodeAccessModel::TrustedControlPlane,
        permissions: NodePermission::ALL.to_vec(),
    }
}

fn log_entries(entries: impl IntoIterator<Item = AggregatedLogEntry>) -> Vec<NodeLogEntry> {
    entries
        .into_iter()
        .filter_map(|entry| {
            let level = parse_log_level(&entry.level)?;
            Some(NodeLogEntry {
                unix_ms: u64::try_from(entry.timestamp_ms).unwrap_or(u64::MAX),
                level,
                target: entry.target,
                message: entry.message,
            })
        })
        .collect()
}

fn parse_log_level(level: &str) -> Option<NodeLogLevel> {
    match level.trim().to_ascii_lowercase().as_str() {
        "trace" => Some(NodeLogLevel::Trace),
        "debug" => Some(NodeLogLevel::Debug),
        "info" => Some(NodeLogLevel::Info),
        "warn" | "warning" => Some(NodeLogLevel::Warn),
        "error" => Some(NodeLogLevel::Error),
        // Drop unknown levels rather than assigning a misleading severity.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuckipv6_common::{LogConfig, RoutingConfig};
    use std::path::PathBuf;

    fn log(timestamp_ms: u128, level: &str, message: &str) -> AggregatedLogEntry {
        AggregatedLogEntry {
            timestamp_ms,
            level: level.into(),
            target: "fuckipv6::test".into(),
            message: message.into(),
        }
    }

    #[test]
    fn identity_projection_exposes_only_public_metadata() -> anyhow::Result<()> {
        let identity = Identity::generate()?;
        let snapshot = identity_snapshot(&identity);

        assert_eq!(snapshot.node_id, identity.client_id());
        assert_eq!(snapshot.display_name, None);
        assert_eq!(
            snapshot.public_key_fingerprint,
            Identity::public_key_fingerprint(&identity.public_key_bytes())
        );
        Ok(())
    }

    #[test]
    fn log_projection_maps_levels_and_drops_unknown_values() {
        let entries = log_entries(vec![
            log(1, "trace", "trace"),
            log(2, "DEBUG", "debug"),
            log(3, "info", "info"),
            log(4, "WARN", "warn"),
            log(5, "warning", "warning"),
            log(6, "error", "error"),
            log(7, "critical", "unknown"),
        ]);

        assert_eq!(entries.len(), 6);
        assert_eq!(entries[0].level, NodeLogLevel::Trace);
        assert_eq!(entries[1].level, NodeLogLevel::Debug);
        assert_eq!(entries[2].level, NodeLogLevel::Info);
        assert_eq!(entries[3].level, NodeLogLevel::Warn);
        assert_eq!(entries[4].level, NodeLogLevel::Warn);
        assert_eq!(entries[5].level, NodeLogLevel::Error);
        assert!(entries.iter().all(|entry| entry.target == "fuckipv6::test"));
    }

    #[test]
    fn log_projection_saturates_unrepresentable_timestamps() {
        let entries = log_entries(vec![log(u128::MAX, "error", "overflow")]);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].unix_ms, u64::MAX);
    }

    #[test]
    fn config_projection_maps_effective_values_without_host_paths() {
        let config = ClientConfig {
            client_id: None,
            coordinator_url: "wss://network.example/ws".into(),
            private_key_path: PathBuf::from("/secret/identity.pem"),
            network: Default::default(),
            tun: Default::default(),
            p2p: Default::default(),
            log: LogConfig {
                level: "debug".into(),
                file: Some(PathBuf::from("/secret/client.log")),
            },
            acl: Default::default(),
            routing: RoutingConfig {
                mode: RoutingMode::GlobalProxy,
                exit_node: Some("fd00::2".into()),
                direct_cidrs: vec!["10.0.0.0/8".into()],
                proxy_cidrs: vec!["0.0.0.0/0".into()],
            },
            exit_node: Default::default(),
        };

        let snapshot = network_config_snapshot("home", &config);
        let encoded = serde_json::to_string(&snapshot).expect("serialize config snapshot");

        assert_eq!(snapshot.network_name, "home");
        assert_eq!(snapshot.routing_mode, NodeRoutingMode::GlobalProxy);
        assert_eq!(snapshot.exit_node.as_deref(), Some("fd00::2"));
        assert_eq!(snapshot.direct_cidrs, ["10.0.0.0/8"]);
        assert_eq!(snapshot.proxy_cidrs, ["0.0.0.0/0"]);
        assert!(snapshot.identity_configured);
        assert!(snapshot.log_file_configured);
        assert!(!encoded.contains("/secret"));
        assert!(!encoded.contains("private_key_path"));
    }

    #[test]
    fn permission_projection_matches_the_manifest_capability_set() {
        let snapshot = trusted_control_plane_permissions();
        let permissions = snapshot
            .permissions
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();

        assert_eq!(snapshot.access_model, NodeAccessModel::TrustedControlPlane);
        assert_eq!(permissions.len(), NodePermission::ALL.len());
        assert_eq!(permissions, NodePermission::ALL.into_iter().collect());
    }
}
