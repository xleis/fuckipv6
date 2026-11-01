//! Versioned, machine-readable node contracts generated from the Rust domain
//! model. These artifacts are generated data, never a second hand-written
//! source of commands or state semantics.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use schemars::{generate::SchemaSettings, JsonSchema};
use serde_json::{json, Value};

use super::{
    NodeCommand, NodeCommandHttpRequest, NodeCommandHttpResponse, NodeCommandKind,
    NodeCommandRequest, NodeCommandResult, NodeEvent, NodeHttpErrorResponse, NodeManifestResponse,
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
    value
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
        NodeClientBlacklistCommand, NodeCreateNetworkCommand, NodeJoinNetworkCommand,
        NodeNetworkAllocation, NodeNetworkRole, NodeNetworkSelection, NodeProbePeerCommand,
        NodeSetNetworkRoleCommand, NodeUpdateNetworkAllocationCommand, NodeUpdateRoutingCommand,
    };
    use fuckipv6_common::NetworkMode;
    use std::path::PathBuf;

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
            NodeCommand::CreateNetwork(NodeCreateNetworkCommand {
                name: "home".into(),
                signaling_port: 41000,
                relay_port: 42000,
                external_host: None,
                auto_connect: false,
            }),
            NodeCommand::JoinNetwork(NodeJoinNetworkCommand {
                target: "ticket".into(),
                network_name: None,
                start: false,
            }),
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
