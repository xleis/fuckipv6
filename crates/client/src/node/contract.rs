//! Versioned, machine-readable node contracts generated from the Rust domain
//! model. These artifacts are generated data, never a second hand-written
//! source of commands or state semantics.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use schemars::{generate::SchemaSettings, JsonSchema};
use serde_json::{json, Value};

use super::{
    NodeApiCommandResponse, NodeApiErrorResponse, NodeApiEvent, NodeApiManifestResponse,
    NodeApiObjectResponse, NodeApiOperationResponse, NodeApiSnapshotResponse, NodeCommand,
    NodeCommandHttpRequest, NodeCommandHttpResponse, NodeCommandKind, NodeCommandRequest,
    NodeCommandResult, NodeEvent, NodeHttpErrorResponse, NodeManifestResponse,
    NodeSnapshotResponse, NodeUiManifest, NodeWsMessage,
};

/// Wire-contract version shared by every generated node artifact.
///
/// Additive, optional fields may remain in v1. Renaming/removing fields,
/// changing enum meanings, or changing validation semantics requires a new
/// version and an explicit refusal path in every adapter.
pub const NODE_CONTRACT_SCHEMA_VERSION: u16 = 1;

const CONTRACT_BASE_ID: &str = "https://relay.xlei.cc/contracts/schema";

/// Schema-only envelope for the complete HTTP/WebSocket surface. It is never
/// serialized on the wire; it exists so one root schema can reference every
/// versioned HTTP DTO from the same Rust definitions used at runtime.
#[derive(JsonSchema)]
#[allow(dead_code)]
struct NodeHttpContract {
    command_request: NodeCommandHttpRequest,
    command_response: NodeCommandHttpResponse,
    manifest_response: NodeManifestResponse,
    snapshot_response: NodeSnapshotResponse,
    error_response: NodeHttpErrorResponse,
    websocket_message: NodeWsMessage,
    api_snapshot_response: NodeApiSnapshotResponse,
    api_manifest_response: NodeApiManifestResponse,
    api_command_response: NodeApiCommandResponse,
    api_operation_response: NodeApiOperationResponse,
    api_object_response: NodeApiObjectResponse,
    api_error_response: NodeApiErrorResponse,
    api_event: NodeApiEvent,
}

/// All checked-in schema artifacts keyed by filename. The returned JSON values
/// are deterministic and use JSON Schema draft 2020-12.
pub fn contract_schema_artifacts() -> BTreeMap<&'static str, Value> {
    BTreeMap::from([
        (
            "node-command-v1.schema.json",
            schema_document::<NodeCommand>("node-command-v1", "NodeCommand"),
        ),
        (
            "node-command-request-v1.schema.json",
            schema_document::<NodeCommandRequest>("node-command-request-v1", "NodeCommandRequest"),
        ),
        (
            "node-command-result-v1.schema.json",
            schema_document::<NodeCommandResult>("node-command-result-v1", "NodeCommandResult"),
        ),
        (
            "node-event-v1.schema.json",
            schema_document::<NodeEvent>("node-event-v1", "NodeEvent"),
        ),
        (
            "node-ui-manifest-v1.schema.json",
            schema_document::<NodeUiManifest>("node-ui-manifest-v1", "NodeUiManifest"),
        ),
        (
            "node-http-v1.schema.json",
            schema_document::<NodeHttpContract>("node-http-v1", "NodeHttpContract"),
        ),
        (
            "node-snapshot-v1.schema.json",
            schema_document::<super::NodeSnapshot>("node-snapshot-v1", "NodeSnapshot"),
        ),
    ])
}

fn schema_document<T: ?Sized + JsonSchema>(artifact: &str, title: &str) -> Value {
    let schema = SchemaSettings::draft2020_12()
        .into_generator()
        .into_root_schema_for::<T>();
    let mut value = schema.to_value();
    let object = value.as_object_mut().expect("root schema is an object");
    object.insert(
        "$id".into(),
        json!(format!("{CONTRACT_BASE_ID}/{artifact}.schema.json")),
    );
    object.insert("title".into(), json!(title));
    object.insert(
        "x-fuckipv6-contract-version".into(),
        json!(NODE_CONTRACT_SCHEMA_VERSION),
    );
    object.insert(
        "x-fuckipv6-rust-source".into(),
        json!("fuckipv6-client::node"),
    );
    canonicalize_json_keys(value)
}

/// Sort object keys recursively so contract rendering is independent of
/// `serde_json`'s `preserve_order` feature being enabled by another target in
/// the same dependency graph (for example, the optional GPUI shell).
fn canonicalize_json_keys(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonicalize_json_keys(value)))
                    .collect(),
            )
        }
        Value::Array(values) => {
            Value::Array(values.into_iter().map(canonicalize_json_keys).collect())
        }
        scalar => scalar,
    }
}

/// Serialize every artifact with stable formatting and a trailing newline.
pub fn render_contract_schemas() -> BTreeMap<&'static str, String> {
    contract_schema_artifacts()
        .into_iter()
        .map(|(name, schema)| {
            let rendered = serde_json::to_string_pretty(&schema)
                .expect("JSON Schema serialization cannot fail");
            (name, format!("{rendered}\n"))
        })
        .collect()
}

/// Write all schema artifacts. Intended for the checked-in update workflow,
/// never called by the running node.
pub fn write_contract_schemas(directory: &Path) -> std::io::Result<()> {
    fs::create_dir_all(directory)?;
    for (name, contents) in render_contract_schemas() {
        fs::write(directory.join(name), contents)?;
    }
    Ok(())
}

/// Return command names in their canonical order and prove every manifest
/// action name is present in the payload-bearing command schema.
pub fn canonical_command_names() -> Vec<&'static str> {
    NodeCommandKind::ALL
        .into_iter()
        .map(NodeCommandKind::as_str)
        .collect()
}

#[cfg(test)]
fn command_constants(schema: &Value) -> Vec<String> {
    let mut commands = Vec::new();
    collect_command_constants(schema, &mut commands);
    commands.sort();
    commands.dedup();
    commands
}

#[cfg(test)]
fn collect_command_constants(value: &Value, output: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            if let Some(Value::Object(command)) = object.get("command") {
                if let Some(Value::String(command)) = command.get("const") {
                    output.push(command.clone());
                }
            }
            for child in object.values() {
                collect_command_constants(child, output);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_command_constants(item, output);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
fn enum_string_values(value: &Value, key: &str, output: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            if let Some(Value::Array(values)) = object.get(key) {
                for value in values {
                    if let Some(value) = value.as_str() {
                        output.push(value.to_owned());
                    }
                }
            }
            for child in object.values() {
                enum_string_values(child, key, output);
            }
        }
        Value::Array(items) => {
            for item in items {
                enum_string_values(item, key, output);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{
        NodeApplyConfigurationCommand, NodeChangeMemberLifecycleCommand,
        NodeClientBlacklistCommand, NodeCreateInvitationCommand, NodeCreateNetworkCommand,
        NodeGetInvitationStatusCommand, NodeJoinNetworkCommand, NodeListInvitationsCommand,
        NodeMemberRemovalPreviewCommand, NodeNetworkAllocation, NodeNetworkDiscoveryCommand,
        NodeNetworkProfileCommand, NodeNetworkRole, NodeNetworkSelection,
        NodePreviewConfigurationCommand, NodePreviewInvitationCommand,
        NodePreviewTrafficPolicyCommand, NodePreviewTrafficPolicyRollbackCommand,
        NodeProbePeerCommand, NodeRedeemInvitationCommand, NodeRevokeInvitationCommand,
        NodeRollbackConfigurationCommand, NodeRollbackTrafficPolicyCommand,
        NodeRotateNodeIdentityCommand, NodeSetNetworkRoleCommand, NodeSetNodeDisplayNameCommand,
        NodeUpdateNetworkAllocationCommand, NodeUpdateRoutingCommand,
    };
    use fuckipv6_common::{
        domain::{
            ApplicationIdentity, ContentHash, DomainName, InvitationCode, InvitationId,
            InvitationRedeemRequest, MemberLifecycleAction, MembershipRole, NetworkId,
            NetworkProfileUpdate, NodeId, PortRange, TrafficAction, TrafficDefaultConfirmedBy,
            TrafficDefaultPolicy, TrafficFailurePolicy, TrafficFlow, TrafficMatch, TrafficPolicy,
            TrafficPolicyTestInput, TrafficProtocol, TrafficRule, TrafficRuleId,
        },
        NetworkMode,
    };
    use std::net::{IpAddr, Ipv4Addr};
    use std::path::PathBuf;

    use crate::node::{NodeApplyTrafficPolicyCommand, NodeTestTrafficPolicyCommand};
    use crate::traffic_policy_transaction::TrafficPolicyDraft;

    fn sample_traffic_policy(revision: u64) -> TrafficPolicy {
        TrafficPolicy {
            node_id: NodeId::from_public_key(b"traffic-policy-node"),
            revision,
            default_policy: TrafficDefaultPolicy {
                action: TrafficAction::Direct,
                confirmed_by: TrafficDefaultConfirmedBy::Owner,
                confirmed_at_unix_ms: 1_700_000_000_000,
            },
            failure_policy: TrafficFailurePolicy::fail_closed(),
            rules: vec![TrafficRule {
                rule_id: TrafficRuleId::from_public_key(b"traffic-policy-rule"),
                priority: 1,
                enabled: true,
                matcher: TrafficMatch {
                    exact_domains: vec![DomainName::parse("example.com").unwrap()],
                    domain_suffixes: Vec::new(),
                    destination_cidrs: Vec::new(),
                    destination_ports: vec![PortRange {
                        start: 443,
                        end: 443,
                    }],
                    protocols: vec![TrafficProtocol::Tcp],
                    applications: Vec::new(),
                },
                action: TrafficAction::Reject,
                description: "Reject example traffic".into(),
            }],
        }
    }

    fn sample_traffic_test_input() -> TrafficPolicyTestInput {
        TrafficPolicyTestInput::manual(
            NetworkId::from_public_key(b"traffic-policy-network"),
            TrafficFlow {
                domain: Some(DomainName::parse("example.com").unwrap()),
                destination_ip: IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10)),
                destination_port: 443,
                protocol: TrafficProtocol::Tcp,
                application: ApplicationIdentity::ExecutablePath("/usr/bin/curl".into()),
            },
        )
    }

    fn sample_commands() -> Vec<NodeCommand> {
        vec![
            NodeCommand::GetSnapshot,
            NodeCommand::GetUiManifest,
            NodeCommand::GetDiagnostics,
            NodeCommand::StartNetworks {
                selection: NodeNetworkSelection::None,
            },
            NodeCommand::StopNetworks {
                networks: vec!["home".into()],
                all: false,
            },
            NodeCommand::RestartNetworks {
                networks: vec!["home".into()],
                all: false,
            },
            NodeCommand::RebuildTun {
                network_name: "home".into(),
            },
            NodeCommand::CreateNetwork(NodeCreateNetworkCommand {
                name: "home".into(),
                purpose: "A test network".into(),
                icon: "network".into(),
                language: "und".into(),
                region: "ZZ".into(),
                tags: Vec::new(),
                capability_declarations: Vec::new(),
                visibility: Default::default(),
                join_mode: Default::default(),
                exit_contribution: Default::default(),
                exit_contribution_scope: Default::default(),
                exit_contribution_mode: Default::default(),
                signaling_port: 41000,
                relay_port: 42000,
                external_host: None,
                auto_connect: false,
            }),
            NodeCommand::PreviewNetworkProfile(NodeNetworkProfileCommand {
                network_name: "home".into(),
                expected_revision: 1,
                update: NetworkProfileUpdate {
                    purpose: "A test network".into(),
                    icon: "network".into(),
                    language: "und".into(),
                    region: "ZZ".into(),
                    tags: Vec::new(),
                    capability_declarations: Vec::new(),
                    visibility: Default::default(),
                    join_mode: Default::default(),
                    exit_contribution: Default::default(),
                    exit_contribution_scope: Default::default(),
                    exit_contribution_mode: Default::default(),
                    exit_selection_policy: None,
                },
            }),
            NodeCommand::UpdateNetworkProfile(NodeNetworkProfileCommand {
                network_name: "home".into(),
                expected_revision: 1,
                update: NetworkProfileUpdate {
                    purpose: "A test network".into(),
                    icon: "network".into(),
                    language: "und".into(),
                    region: "ZZ".into(),
                    tags: Vec::new(),
                    capability_declarations: Vec::new(),
                    visibility: Default::default(),
                    join_mode: Default::default(),
                    exit_contribution: Default::default(),
                    exit_contribution_scope: Default::default(),
                    exit_contribution_mode: Default::default(),
                    exit_selection_policy: None,
                },
            }),
            NodeCommand::PublishNetworkDiscovery(NodeNetworkDiscoveryCommand {
                network_name: "home".into(),
                index_url: "https://discovery.example.com".into(),
            }),
            NodeCommand::UpdateNetworkDiscovery(NodeNetworkDiscoveryCommand {
                network_name: "home".into(),
                index_url: "https://discovery.example.com".into(),
            }),
            NodeCommand::UnpublishNetworkDiscovery(NodeNetworkDiscoveryCommand {
                network_name: "home".into(),
                index_url: "https://discovery.example.com".into(),
            }),
            NodeCommand::RefreshDiscoveryIndex {
                index_url: Some("https://discovery.example.com".into()),
            },
            NodeCommand::RetryDiscoveryIndex,
            NodeCommand::PreviewJoinNetwork(crate::node::NodePreviewJoinNetworkCommand {
                network_id: fuckipv6_common::domain::NetworkId::from_public_key(b"preview-network")
                    .to_string(),
            }),
            NodeCommand::JoinNetwork(Box::new(NodeJoinNetworkCommand {
                target: "ticket".into(),
                network_name: None,
                start: false,
                discovery_record: None,
                network_id: None,
                preview_digest: None,
                network_policy_revision: None,
                privacy_policy_revision: None,
                terms_hash: None,
                acknowledged_at_unix_ms: None,
                exit_contribution_acknowledged: None,
            })),
            NodeCommand::SubmitJoinRequest(crate::node::command::NodeSubmitJoinRequestCommand {
                network_id: fuckipv6_common::domain::NetworkId::from_public_key(
                    b"join-request-network",
                )
                .to_string(),
                reason: "Please allow this node to join.".into(),
            }),
            NodeCommand::ListJoinRequests(crate::node::command::NodeListJoinRequestsCommand {
                network_id: fuckipv6_common::domain::NetworkId::from_public_key(
                    b"join-request-network",
                )
                .to_string(),
            }),
            NodeCommand::ListNetworkMembers(crate::node::command::NodeNetworkMembersCommand {
                network_id: fuckipv6_common::domain::NetworkId::from_public_key(
                    b"join-request-network",
                )
                .to_string(),
            }),
            NodeCommand::PreviewMemberRemoval(NodeMemberRemovalPreviewCommand {
                network_id: NetworkId::from_public_key(b"join-request-network").to_string(),
                target_node_id: NodeId::from_public_key(b"member-removal-target").to_string(),
            }),
            NodeCommand::ChangeMemberLifecycle(NodeChangeMemberLifecycleCommand {
                network_id: NetworkId::from_public_key(b"join-request-network").to_string(),
                target_node_id: NodeId::from_public_key(b"member-lifecycle-target").to_string(),
                action: MemberLifecycleAction::Remove,
                role: MembershipRole::Member,
                expected_revision: 2,
                reason: Some("Remove a compromised member identity.".into()),
            }),
            NodeCommand::ReviewJoinRequest(crate::node::command::NodeReviewJoinRequestCommand {
                network_id: fuckipv6_common::domain::NetworkId::from_public_key(
                    b"join-request-network",
                )
                .to_string(),
                review: crate::node::command::NodeJoinRequestReviewItem {
                    request_id: fuckipv6_common::domain::JoinRequestId::from_public_key(
                        b"join-request-review",
                    )
                    .to_string(),
                    expected_revision: 1,
                    action: fuckipv6_common::domain::JoinRequestReviewAction::RequestInformation,
                    note: Some("Please provide the requested details.".into()),
                },
            }),
            NodeCommand::BatchReviewJoinRequests(
                crate::node::command::NodeBatchReviewJoinRequestsCommand {
                    network_id: fuckipv6_common::domain::NetworkId::from_public_key(
                        b"join-request-network",
                    )
                    .to_string(),
                    reviews: vec![crate::node::command::NodeJoinRequestReviewItem {
                        request_id: fuckipv6_common::domain::JoinRequestId::from_public_key(
                            b"join-request-batch-review",
                        )
                        .to_string(),
                        expected_revision: 1,
                        action: fuckipv6_common::domain::JoinRequestReviewAction::Approve,
                        note: None,
                    }],
                },
            ),
            NodeCommand::WithdrawJoinRequest(
                crate::node::command::NodeWithdrawJoinRequestCommand {
                    network_id: fuckipv6_common::domain::NetworkId::from_public_key(
                        b"join-request-network",
                    )
                    .to_string(),
                    request_id: fuckipv6_common::domain::JoinRequestId::from_public_key(
                        b"join-request-withdraw",
                    )
                    .to_string(),
                },
            ),
            NodeCommand::ResubmitJoinRequest(
                crate::node::command::NodeResubmitJoinRequestCommand {
                    network_id: fuckipv6_common::domain::NetworkId::from_public_key(
                        b"join-request-network",
                    )
                    .to_string(),
                    request_id: fuckipv6_common::domain::JoinRequestId::from_public_key(
                        b"join-request-resubmit",
                    )
                    .to_string(),
                    expected_revision: 2,
                    reason: "The requested details are attached.".into(),
                },
            ),
            NodeCommand::CreateInvitation(NodeCreateInvitationCommand {
                network_id: NetworkId::from_public_key(b"invitation-network"),
                role: MembershipRole::Member,
                capabilities: vec![fuckipv6_common::domain::CapabilityKind::File],
                max_uses: 2,
                expires_in_ms: 86_400_000,
            }),
            NodeCommand::ListInvitations(NodeListInvitationsCommand {
                network_id: NetworkId::from_public_key(b"invitation-network"),
            }),
            NodeCommand::GetInvitationStatus(NodeGetInvitationStatusCommand {
                network_id: NetworkId::from_public_key(b"invitation-network"),
                invitation_id: InvitationId::from_public_key(b"invitation-sample"),
            }),
            NodeCommand::RevokeInvitation(NodeRevokeInvitationCommand {
                network_id: NetworkId::from_public_key(b"invitation-network"),
                invitation_id: InvitationId::from_public_key(b"invitation-sample"),
                expected_revision: 1,
            }),
            NodeCommand::PreviewInvitation(NodePreviewInvitationCommand {
                network_id: NetworkId::from_public_key(b"invitation-network"),
                coordinator_url: "wss://coordinator.example.test/signaling".into(),
                code: InvitationCode::parse(format!("fkinv_{}", "A".repeat(43))).unwrap(),
            }),
            NodeCommand::RedeemInvitation(NodeRedeemInvitationCommand {
                coordinator_url: "wss://coordinator.example.test/signaling".into(),
                request: InvitationRedeemRequest {
                    network_id: NetworkId::from_public_key(b"invitation-network"),
                    code: InvitationCode::parse(format!("fkinv_{}", "A".repeat(43))).unwrap(),
                    invitation_id: InvitationId::from_public_key(b"invitation-sample"),
                    expected_invitation_revision: 1,
                    preview_digest: ContentHash::from_bytes(b"invitation-preview"),
                    terms_hash: ContentHash::from_bytes(b"invitation-terms"),
                    acknowledged_at_unix_ms: 1_700_000_000_000,
                    exit_contribution_acknowledged: Some(true),
                },
            }),
            NodeCommand::LeaveNetwork(crate::node::NodeLeaveNetworkCommand {
                network_name: "office".into(),
            }),
            NodeCommand::AcknowledgeMembershipPolicy(
                crate::node::NodeAcknowledgeMembershipPolicyCommand {
                    network_id: NetworkId::from_public_key(b"invitation-network").to_string(),
                    expected_terms_hash: ContentHash::from_bytes(b"membership-terms"),
                    acknowledged_at_unix_ms: 1_700_000_000_000,
                    exit_contribution_acknowledged: Some(true),
                },
            ),
            NodeCommand::UpdateRouting(NodeUpdateRoutingCommand {
                network_name: "home".into(),
                mode: super::super::NodeRoutingMode::MeshOnly,
                exit_node: None,
                is_exit_node: false,
            }),
            NodeCommand::SetNetworkRole(NodeSetNetworkRoleCommand {
                network_name: "home".into(),
                role: NodeNetworkRole::Relay,
                enabled: true,
            }),
            NodeCommand::SetLocalExitCapability(crate::node::NodeSetLocalExitCapabilityCommand {
                enabled: true,
            }),
            NodeCommand::SetNodeDisplayName(NodeSetNodeDisplayNameCommand {
                display_name: Some("edge-gateway".into()),
            }),
            NodeCommand::RotateNodeIdentity(NodeRotateNodeIdentityCommand {
                expected_fingerprint: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
            }),
            NodeCommand::GetBlacklist {
                network_name: "home".into(),
            },
            NodeCommand::BanClient(NodeClientBlacklistCommand {
                network_name: "home".into(),
                client_id: "node-a".into(),
            }),
            NodeCommand::UnbanClient(NodeClientBlacklistCommand {
                network_name: "home".into(),
                client_id: "node-a".into(),
            }),
            NodeCommand::GetNetworkAllocation {
                network_name: "home".into(),
            },
            NodeCommand::UpdateNetworkAllocation(NodeUpdateNetworkAllocationCommand {
                network_name: "home".into(),
                allocation: NodeNetworkAllocation {
                    mode: NetworkMode::DualStack,
                    ipv4_pool: "10.8.0.0/24".into(),
                    ipv6_pool: "fd00::/120".into(),
                },
            }),
            NodeCommand::ProbePeer(NodeProbePeerCommand {
                network_name: "home".into(),
                target: "peer".into(),
                timeout_ms: 1000,
            }),
            NodeCommand::StartNode,
            NodeCommand::StopNode,
            NodeCommand::RestartNode,
            NodeCommand::CleanupResiduals {
                networks: vec!["home".into()],
                all: false,
            },
            NodeCommand::GetConfiguration,
            NodeCommand::PreviewConfiguration(NodePreviewConfigurationCommand {
                expected_revision: 0,
                networks: std::collections::BTreeMap::new(),
            }),
            NodeCommand::ApplyConfiguration(NodeApplyConfigurationCommand {
                expected_revision: 0,
                networks: std::collections::BTreeMap::new(),
            }),
            NodeCommand::RollbackConfiguration(NodeRollbackConfigurationCommand {
                expected_revision: 0,
                target_revision: 0,
            }),
            NodeCommand::GetConfigurationHistory,
            NodeCommand::GetPolicy,
            NodeCommand::PreviewPolicy(crate::node::NodePreviewPolicyCommand {
                expected_revision: 0,
                candidate: crate::policy_transaction::PolicySnapshot::default(),
            }),
            NodeCommand::PreviewPolicyRollback(crate::node::NodePreviewPolicyRollbackCommand {
                expected_revision: 0,
                target_revision: 1,
            }),
            NodeCommand::ApplyPolicy(crate::node::NodeApplyPolicyCommand {
                expected_revision: 0,
                candidate: crate::policy_transaction::PolicySnapshot::default(),
                network_ids: Vec::new(),
                includes_local_policy: false,
                confirm_privilege_expansion: false,
            }),
            NodeCommand::RollbackPolicy(crate::node::NodeRollbackPolicyCommand {
                expected_revision: 0,
                target_revision: 1,
                network_ids: Vec::new(),
                includes_local_policy: false,
                confirm_privilege_expansion: false,
            }),
            NodeCommand::GetPolicyHistory,
            NodeCommand::TestPolicy(fuckipv6_common::domain::PolicyTestInput {
                network_id: fuckipv6_common::domain::NetworkId::from_public_key(
                    b"policy-test-network",
                ),
                actor: fuckipv6_common::domain::NodeId::from_public_key(b"policy-test-actor"),
                role: fuckipv6_common::domain::NetworkRole::Member,
                groups: Default::default(),
                membership_active: true,
                recent_reauthentication: true,
                emergency_coordinator: false,
                request: fuckipv6_common::domain::PolicyTestRequest {
                    action: fuckipv6_common::domain::PolicyAction::Read,
                    resource: fuckipv6_common::domain::ResourceKind::Network,
                    resource_network_id: fuckipv6_common::domain::NetworkId::from_public_key(
                        b"policy-test-network",
                    ),
                    object_id: None,
                    intent: fuckipv6_common::domain::PermissionIntent::ReadOnly,
                },
                local_policy: None,
                network_acl: None,
                now_unix_ms: 1_000,
            }),
            NodeCommand::GetTrafficPolicy,
            NodeCommand::PreviewTrafficPolicy(NodePreviewTrafficPolicyCommand {
                expected_revision: 1,
                draft: TrafficPolicyDraft::from_applied(&sample_traffic_policy(1)),
            }),
            NodeCommand::PreviewTrafficPolicyRollback(NodePreviewTrafficPolicyRollbackCommand {
                expected_revision: 1,
                target_revision: 1,
            }),
            NodeCommand::ApplyTrafficPolicy(NodeApplyTrafficPolicyCommand {
                expected_revision: 1,
                draft: TrafficPolicyDraft::from_applied(&sample_traffic_policy(1)),
                confirm_risk: false,
            }),
            NodeCommand::RollbackTrafficPolicy(NodeRollbackTrafficPolicyCommand {
                expected_revision: 1,
                target_revision: 1,
                confirm_risk: false,
            }),
            NodeCommand::GetTrafficPolicyHistory,
            NodeCommand::TestTrafficPolicy(NodeTestTrafficPolicyCommand {
                current: sample_traffic_policy(1),
                candidate: sample_traffic_policy(2),
                input: sample_traffic_test_input(),
            }),
            NodeCommand::Shutdown,
        ]
    }

    #[test]
    fn every_canonical_command_kind_has_exactly_one_payload_variant() {
        let kinds: Vec<_> = sample_commands()
            .into_iter()
            .map(|command| command.kind())
            .collect();
        assert_eq!(kinds, NodeCommandKind::ALL);
        assert_eq!(
            kinds.iter().map(|kind| kind.as_str()).collect::<Vec<_>>(),
            canonical_command_names()
        );
    }

    #[test]
    fn generated_command_schema_contains_every_canonical_command() {
        let schemas = contract_schema_artifacts();
        let commands = command_constants(&schemas["node-command-v1.schema.json"]);
        let mut expected: Vec<_> = canonical_command_names()
            .into_iter()
            .map(str::to_owned)
            .collect();
        expected.sort();
        assert_eq!(commands, expected);
    }

    #[test]
    fn manifest_exposes_only_action_commands_and_never_bootstrap_manifest() {
        let schemas = contract_schema_artifacts();
        let mut commands = Vec::new();
        enum_string_values(
            &schemas["node-ui-manifest-v1.schema.json"],
            "enum",
            &mut commands,
        );
        let mut actual: Vec<_> = commands
            .into_iter()
            .filter(|value| canonical_command_names().contains(&value.as_str()))
            .collect();
        actual.sort();
        actual.dedup();
        let mut expected: Vec<_> = NodeCommandKind::ACTIONS
            .into_iter()
            .map(|kind| kind.as_str().to_owned())
            .collect();
        expected.sort();
        assert_eq!(actual, expected);
        assert!(!actual.contains(&NodeCommandKind::GetUiManifest.as_str().to_owned()));
    }

    #[test]
    fn checked_in_schema_artifacts_are_current() {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/schema");
        if std::env::var_os("UPDATE_CONTRACT").is_some() {
            write_contract_schemas(&directory).expect("write contract schemas");
            return;
        }

        let expected = render_contract_schemas();
        let mut actual_names = Vec::new();
        for entry in fs::read_dir(&directory).unwrap_or_else(|error| {
            panic!(
                "read checked-in schemas from {}: {error}",
                directory.display()
            )
        }) {
            let entry = entry.expect("schema directory entry");
            if entry.path().extension().and_then(|value| value.to_str()) == Some("json") {
                actual_names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        actual_names.sort();
        let mut expected_names: Vec<_> = expected.keys().map(|name| (*name).to_owned()).collect();
        expected_names.sort();
        assert_eq!(
            actual_names, expected_names,
            "schema artifact set drifted; run scripts/generate-contracts.sh"
        );

        for (name, contents) in expected {
            let path = directory.join(name);
            let actual = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
            assert_eq!(
                actual,
                contents,
                "{} is stale; run scripts/generate-contracts.sh",
                path.display()
            );
        }
    }

    #[test]
    fn generated_schema_objects_have_canonical_key_order() {
        fn assert_sorted(value: &Value, path: &str) {
            match value {
                Value::Object(map) => {
                    let keys: Vec<_> = map.keys().cloned().collect();
                    let mut sorted = keys.clone();
                    sorted.sort();
                    assert_eq!(keys, sorted, "object keys are not sorted at {path}");
                    for (key, child) in map {
                        assert_sorted(child, &format!("{path}.{key}"));
                    }
                }
                Value::Array(values) => {
                    for (index, child) in values.iter().enumerate() {
                        assert_sorted(child, &format!("{path}[{index}]"));
                    }
                }
                _ => {}
            }
        }

        for (name, schema) in contract_schema_artifacts() {
            assert_sorted(&schema, name);
        }
    }

    #[test]
    fn generated_schemas_have_stable_version_and_ids() {
        for (name, schema) in contract_schema_artifacts() {
            assert_eq!(schema["x-fuckipv6-contract-version"], 1, "{name}");
            assert_eq!(
                schema["$schema"],
                "https://json-schema.org/draft/2020-12/schema"
            );
            assert_eq!(
                schema["$id"],
                format!("{CONTRACT_BASE_ID}/{name}"),
                "{name}"
            );
        }
    }

    #[test]
    fn unknown_command_values_fail_closed_on_deserialization() {
        let command: Result<NodeCommand, _> =
            serde_json::from_value(json!({"command": "not_a_command"}));
        assert!(command.is_err());
        let kind: Result<NodeCommandKind, _> = serde_json::from_value(json!("not_a_command"));
        assert!(kind.is_err());
    }
}
