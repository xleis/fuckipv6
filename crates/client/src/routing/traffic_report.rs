//! Structured runtime evidence for traffic-policy E2E checks.
//!
//! The report is intentionally data-only: it records the exercised rule,
//! target, observed result and cleanup outcome without embedding packet
//! payloads, command output, unrelated host identifiers or raw socket data.

use std::net::IpAddr;

use fuckipv6_common::domain::{TrafficAction, TrafficDecisionReason, TrafficRuleId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrafficNetworkFlowKind {
    Domain,
    Application,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrafficNetworkFlowResult {
    Connected,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficNetworkFlowReport {
    pub kind: TrafficNetworkFlowKind,
    /// The matched rule, or `None` when an explicit default policy decided the
    /// flow. A missing value is never replaced with a guessed rule.
    pub rule_id: Option<TrafficRuleId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    pub destination_ip: IpAddr,
    pub destination_port: u16,
    /// The concrete action selected by the ordered policy evaluator.
    pub target: TrafficAction,
    pub reason: TrafficDecisionReason,
    pub result: TrafficNetworkFlowResult,
    /// Source address observed by the isolated peer. This is evidence that the
    /// expected path carried the connection, not a field for UI display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_source_ip: Option<IpAddr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrafficNetworkRollbackResult {
    Restored,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficNetworkRollbackReport {
    pub from_revision: u64,
    pub target_revision: u64,
    pub applied_revision: u64,
    pub before_source_ip: IpAddr,
    pub after_source_ip: IpAddr,
    pub result: TrafficNetworkRollbackResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficNetworkFailClosedReport {
    pub attempted_connections: u32,
    pub blocked_connections: u32,
    pub leaked_connections: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub observed_leaked_source_ips: Vec<IpAddr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrafficNetworkCleanupStatus {
    Complete,
    Incomplete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficNetworkCleanupReport {
    pub nft_table_removed: bool,
    pub policy_rule_removed: bool,
    pub namespaces_removed: bool,
    pub residual_resources: u32,
    pub status: TrafficNetworkCleanupStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficNetworkReport {
    pub schema_version: u16,
    pub policy_revision: u64,
    pub flows: Vec<TrafficNetworkFlowReport>,
    pub fail_closed: TrafficNetworkFailClosedReport,
    pub rollback: TrafficNetworkRollbackReport,
    pub cleanup: TrafficNetworkCleanupReport,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_report_serializes_rule_target_result_and_cleanup_as_data() {
        let rule_id = TrafficRuleId::from_public_key(b"traffic-network-report-rule");
        let network_id =
            fuckipv6_common::domain::NetworkId::from_public_key(b"traffic-network-report-path");
        let report = TrafficNetworkReport {
            schema_version: 1,
            policy_revision: 2,
            flows: vec![TrafficNetworkFlowReport {
                kind: TrafficNetworkFlowKind::Domain,
                rule_id: Some(rule_id),
                domain: Some("domain.test".into()),
                destination_ip: "198.51.100.20".parse().unwrap(),
                destination_port: 443,
                target: TrafficAction::Network(network_id),
                reason: TrafficDecisionReason::FirstMatch,
                result: TrafficNetworkFlowResult::Connected,
                observed_source_ip: Some("10.10.0.2".parse().unwrap()),
            }],
            fail_closed: TrafficNetworkFailClosedReport {
                attempted_connections: 2,
                blocked_connections: 2,
                leaked_connections: 0,
                observed_leaked_source_ips: Vec::new(),
            },
            rollback: TrafficNetworkRollbackReport {
                from_revision: 2,
                target_revision: 1,
                applied_revision: 1,
                before_source_ip: "10.10.0.2".parse().unwrap(),
                after_source_ip: "10.30.0.2".parse().unwrap(),
                result: TrafficNetworkRollbackResult::Restored,
            },
            cleanup: TrafficNetworkCleanupReport {
                nft_table_removed: true,
                policy_rule_removed: true,
                namespaces_removed: true,
                residual_resources: 0,
                status: TrafficNetworkCleanupStatus::Complete,
            },
        };

        let encoded = serde_json::to_string(&report).unwrap();
        assert!(encoded.contains("\"rule_id\""));
        assert!(encoded.contains("\"target\""));
        assert!(encoded.contains("\"result\""));
        assert!(encoded.contains("\"cleanup\""));
        assert!(!encoded.contains("command output"));
        assert_eq!(
            serde_json::from_str::<TrafficNetworkReport>(&encoded).unwrap(),
            report
        );
    }
}
