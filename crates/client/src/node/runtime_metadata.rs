use fuckipv6_common::{AggregatedLogEntry, Identity};

use super::{NodeIdentitySnapshot, NodeLogEntry, NodeLogLevel};

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
}
