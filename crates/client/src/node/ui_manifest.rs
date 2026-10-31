use std::collections::{HashMap, HashSet};

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};

use super::command::NodeCommandKind;
use super::state::NodePermission;

pub const NODE_UI_MANIFEST_SCHEMA_VERSION: u16 = 1;

/// Declarative contract shared by WebUI and the future native GPUI surface.
///
/// Adapters may choose different widgets and rendering, but page availability,
/// commands, fields, validation, confirmations, permission classes and backend
/// maturity come from this manifest rather than duplicated UI business logic.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeUiManifest {
    pub schema_version: u16,
    pub pages: Vec<NodeUiPage>,
}

impl NodeUiManifest {
    /// Validate the cross-surface contract before either WebUI or GPUI uses it.
    ///
    /// The manifest is a compatibility boundary, not trusted configuration.
    /// A malformed manifest must fail closed instead of allowing each renderer
    /// to invent a different page set, field rule, permission, or backend state.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        const REQUIRED_PAGES: [&str; 11] = [
            "overview",
            "networks",
            "peers",
            "topology",
            "traffic",
            "routing",
            "exit_node",
            "relay",
            "logs",
            "identity",
            "settings",
        ];

        let mut violations = Vec::new();
        if self.schema_version != NODE_UI_MANIFEST_SCHEMA_VERSION {
            violations.push(format!(
                "schema_version_mismatch:expected={NODE_UI_MANIFEST_SCHEMA_VERSION}:actual={}",
                self.schema_version
            ));
        }

        let actual_pages = self
            .pages
            .iter()
            .map(|page| page.id.as_str())
            .collect::<Vec<_>>();
        if actual_pages != REQUIRED_PAGES {
            violations.push(format!(
                "page_set_or_order_mismatch:expected={REQUIRED_PAGES:?}:actual={actual_pages:?}"
            ));
        }

        let mut page_ids = HashSet::new();
        let mut action_ids = HashSet::new();
        let mut command_modes = HashMap::new();

        for page in &self.pages {
            if page.id.is_empty() || page.nav_label_key.is_empty() || page.title_key.is_empty() {
                violations.push(format!("page_has_empty_contract_key:{}", page.id));
            }
            if !page_ids.insert(page.id.as_str()) {
                violations.push(format!("duplicate_page_id:{}", page.id));
            }

            for action in &page.actions {
                let context = format!("{}.{}", page.id, action.id);
                if action.id.is_empty() || action.label_key.is_empty() {
                    violations.push(format!("action_has_empty_contract_key:{context}"));
                }
                if !action_ids.insert(action.id.as_str()) {
                    violations.push(format!("duplicate_action_id:{}", action.id));
                }
                if action.command == NodeCommandKind::GetUiManifest {
                    violations.push(format!("bootstrap_command_exposed_as_action:{}", action.id));
                }

                if let Some((backend_status, permission, audit)) = command_modes.insert(
                    action.command,
                    (
                        action.backend_status,
                        action.permission,
                        action.audit.clone(),
                    ),
                ) {
                    // SetNetworkRole is role-scoped: Exit Node is implemented,
                    // while Discovery and Relay hot toggles remain unavailable.
                    let backend_status_is_role_scoped =
                        action.command == NodeCommandKind::SetNetworkRole;
                    if !backend_status_is_role_scoped && backend_status != action.backend_status {
                        violations.push(format!(
                            "inconsistent_backend_status:{}:{context}",
                            action.command.as_str()
                        ));
                    }
                    if permission != action.permission {
                        violations.push(format!(
                            "inconsistent_permission:{}:{context}",
                            action.command.as_str()
                        ));
                    }
                    if audit != action.audit {
                        violations.push(format!(
                            "inconsistent_audit:{}:{context}",
                            action.command.as_str()
                        ));
                    }
                }

                match (&action.audit.policy, &action.audit.action) {
                    (NodeUiAuditPolicy::None, Some(_)) => violations.push(format!(
                        "non_audited_action_has_event:{context}:{}",
                        action.command.as_str()
                    )),
                    (NodeUiAuditPolicy::RecordOutcome, Some(event)) if event.is_empty() => {
                        violations.push(format!(
                            "audited_action_has_empty_event:{context}:{}",
                            action.command.as_str()
                        ));
                    }
                    (NodeUiAuditPolicy::RecordOutcome, None) => violations.push(format!(
                        "audited_action_has_no_event:{context}:{}",
                        action.command.as_str()
                    )),
                    _ => {}
                }
                if action.audit != audit_for_command(action.command) {
                    violations.push(format!(
                        "audit_declaration_mismatch:{}:{context}",
                        action.command.as_str()
                    ));
                }

                if let Some(confirmation) = &action.confirmation {
                    if confirmation.title_key.is_empty() || confirmation.consequence_key.is_empty()
                    {
                        violations.push(format!("confirmation_has_empty_key:{context}"));
                    }
                }
                if action.command == NodeCommandKind::Shutdown
                    && !action
                        .confirmation
                        .as_ref()
                        .is_some_and(|confirmation| confirmation.destructive)
                {
                    violations.push(format!(
                        "shutdown_requires_destructive_confirmation:{context}"
                    ));
                }

                let mut field_ids = HashSet::new();
                for field in &action.fields {
                    if field.id.is_empty() || field.label_key.is_empty() {
                        violations.push(format!("field_has_empty_contract_key:{context}"));
                    }
                    if !field_ids.insert(field.id.as_str()) {
                        violations.push(format!("duplicate_field_id:{context}.{}", field.id));
                    }
                    if field.required
                        && field.input != NodeUiInputKind::Boolean
                        && field.validation == NodeUiValidation::None
                    {
                        violations.push(format!(
                            "required_field_has_no_validation:{context}.{}",
                            field.id
                        ));
                    }
                }
            }
        }

        for command in NodeCommandKind::ACTIONS {
            if !command_modes.contains_key(&command) {
                violations.push(format!("missing_command_action:{}", command.as_str()));
            }
        }

        if violations.is_empty() {
            Ok(())
        } else {
            Err(violations)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeUiPage {
    pub id: String,
    pub nav_label_key: String,
    pub title_key: String,
    pub actions: Vec<NodeUiAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeUiAction {
    pub id: String,
    pub label_key: String,
    #[schemars(schema_with = "ui_action_command_schema")]
    pub command: NodeCommandKind,
    pub backend_status: NodeUiBackendStatus,
    pub permission: NodePermission,
    /// Stable audit requirement for this command. Reads that are too noisy to
    /// record still carry an explicit `none` declaration instead of leaving
    /// adapters to guess.
    pub audit: NodeUiAudit,
    pub availability: NodeUiAvailability,
    pub fields: Vec<NodeUiField>,
    pub confirmation: Option<NodeUiConfirmation>,
}

fn ui_action_command_schema(generator: &mut SchemaGenerator) -> Schema {
    let allowed: Vec<_> = NodeCommandKind::ACTIONS
        .into_iter()
        .map(NodeCommandKind::as_str)
        .collect();
    let mut schema = String::json_schema(generator);
    schema.insert("enum".into(), serde_json::json!(allowed));
    schema
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeUiField {
    pub id: String,
    pub label_key: String,
    pub input: NodeUiInputKind,
    pub required: bool,
    pub validation: NodeUiValidation,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeUiConfirmation {
    pub title_key: String,
    pub consequence_key: String,
    pub destructive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeUiAudit {
    pub policy: NodeUiAuditPolicy,
    /// Stable action name written to the append-only audit log.
    pub action: Option<String>,
}

impl NodeUiAudit {
    fn none() -> Self {
        Self {
            policy: NodeUiAuditPolicy::None,
            action: None,
        }
    }

    fn record(action: &str) -> Self {
        Self {
            policy: NodeUiAuditPolicy::RecordOutcome,
            action: Some(action.into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeUiAuditPolicy {
    None,
    RecordOutcome,
}

/// Canonical audit declaration for a backend command. Keeping this mapping in
/// the command contract prevents WebUI and GPUI from auditing different names
/// for the same operation.
pub fn audit_for_command(command: NodeCommandKind) -> NodeUiAudit {
    match command {
        NodeCommandKind::GetSnapshot | NodeCommandKind::GetUiManifest => NodeUiAudit::none(),
        NodeCommandKind::CreateNetwork => NodeUiAudit::record("network.create"),
        NodeCommandKind::JoinNetwork => NodeUiAudit::record("network.join"),
        NodeCommandKind::StartNetworks => NodeUiAudit::record("network.start"),
        NodeCommandKind::StopNetworks => NodeUiAudit::record("network.stop"),
        NodeCommandKind::RestartNetworks => NodeUiAudit::record("network.restart"),
        NodeCommandKind::UpdateRouting => NodeUiAudit::record("routing.update"),
        NodeCommandKind::SetNetworkRole => NodeUiAudit::record("network.role.set"),
        NodeCommandKind::ProbePeer => NodeUiAudit::record("peer.probe"),
        NodeCommandKind::Shutdown => NodeUiAudit::record("node.shutdown"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeUiBackendStatus {
    /// The production command handler currently accepts this command.
    Implemented,
    /// The shared command exists, but the runtime backend rejects it as unavailable.
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeUiAvailability {
    Always,
    NodeRunning,
    NetworkOnline,
    NetworkSelected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeUiInputKind {
    Boolean,
    ExternalHost,
    NetworkName,
    NetworkSelection,
    PeerTarget,
    Port,
    Role,
    RoutingMode,
    UrlOrTicket,
    TimeoutMs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeUiValidation {
    None,
    NonEmpty,
    NetworkName,
    Port,
    DistinctPorts,
    NetworkSelection,
    Role,
    RoutingMode,
    PositiveTimeout,
}

pub fn node_ui_manifest() -> NodeUiManifest {
    NodeUiManifest {
        schema_version: NODE_UI_MANIFEST_SCHEMA_VERSION,
        pages: vec![
            page(
                "overview",
                "nav.overview",
                "page.overview",
                vec![action(
                    "refresh_snapshot",
                    "action.refresh_snapshot",
                    NodeCommandKind::GetSnapshot,
                    NodeUiBackendStatus::Implemented,
                    NodePermission::ViewState,
                    NodeUiAvailability::Always,
                    vec![],
                    None,
                )],
            ),
            page(
                "networks",
                "nav.networks",
                "page.networks",
                vec![
                    action(
                        "create_network",
                        "action.create_network",
                        NodeCommandKind::CreateNetwork,
                        NodeUiBackendStatus::Implemented,
                        NodePermission::ManageNetworks,
                        NodeUiAvailability::NodeRunning,
                        vec![
                            field(
                                "name",
                                "field.network_name",
                                NodeUiInputKind::NetworkName,
                                true,
                                NodeUiValidation::NetworkName,
                            ),
                            field(
                                "signaling_port",
                                "field.signaling_port",
                                NodeUiInputKind::Port,
                                true,
                                NodeUiValidation::Port,
                            ),
                            field(
                                "relay_port",
                                "field.relay_port",
                                NodeUiInputKind::Port,
                                true,
                                NodeUiValidation::DistinctPorts,
                            ),
                            field(
                                "external_host",
                                "field.external_host",
                                NodeUiInputKind::ExternalHost,
                                false,
                                NodeUiValidation::None,
                            ),
                            field(
                                "auto_connect",
                                "field.auto_connect",
                                NodeUiInputKind::Boolean,
                                true,
                                NodeUiValidation::None,
                            ),
                        ],
                        None,
                    ),
                    action(
                        "join_network",
                        "action.join_network",
                        NodeCommandKind::JoinNetwork,
                        NodeUiBackendStatus::Implemented,
                        NodePermission::ManageNetworks,
                        NodeUiAvailability::NodeRunning,
                        vec![
                            field(
                                "target",
                                "field.ticket_or_url",
                                NodeUiInputKind::UrlOrTicket,
                                true,
                                NodeUiValidation::NonEmpty,
                            ),
                            field(
                                "network_name",
                                "field.network_name",
                                NodeUiInputKind::NetworkName,
                                false,
                                NodeUiValidation::NetworkName,
                            ),
                            field(
                                "start",
                                "field.start_after_join",
                                NodeUiInputKind::Boolean,
                                true,
                                NodeUiValidation::None,
                            ),
                        ],
                        None,
                    ),
                    action(
                        "start_networks",
                        "action.start_networks",
                        NodeCommandKind::StartNetworks,
                        NodeUiBackendStatus::Implemented,
                        NodePermission::ManageNetworks,
                        NodeUiAvailability::NodeRunning,
                        vec![field(
                            "selection",
                            "field.network_selection",
                            NodeUiInputKind::NetworkSelection,
                            true,
                            NodeUiValidation::NetworkSelection,
                        )],
                        None,
                    ),
                    action(
                        "stop_networks",
                        "action.stop_networks",
                        NodeCommandKind::StopNetworks,
                        NodeUiBackendStatus::Implemented,
                        NodePermission::ManageNetworks,
                        NodeUiAvailability::NetworkOnline,
                        vec![
                            field(
                                "networks",
                                "field.network_selection",
                                NodeUiInputKind::NetworkSelection,
                                false,
                                NodeUiValidation::NetworkSelection,
                            ),
                            field(
                                "all",
                                "field.all_networks",
                                NodeUiInputKind::Boolean,
                                false,
                                NodeUiValidation::None,
                            ),
                        ],
                        Some(confirmation(
                            "confirm.stop_networks.title",
                            "confirm.stop_networks.consequence",
                            false,
                        )),
                    ),
                    action(
                        "restart_networks",
                        "action.restart_networks",
                        NodeCommandKind::RestartNetworks,
                        NodeUiBackendStatus::Implemented,
                        NodePermission::ManageNetworks,
                        NodeUiAvailability::NetworkOnline,
                        vec![
                            field(
                                "networks",
                                "field.network_selection",
                                NodeUiInputKind::NetworkSelection,
                                false,
                                NodeUiValidation::NetworkSelection,
                            ),
                            field(
                                "all",
                                "field.all_networks",
                                NodeUiInputKind::Boolean,
                                false,
                                NodeUiValidation::None,
                            ),
                        ],
                        Some(confirmation(
                            "confirm.restart_networks.title",
                            "confirm.restart_networks.consequence",
                            false,
                        )),
                    ),
                ],
            ),
            page(
                "peers",
                "nav.peers",
                "page.peers",
                vec![action(
                    "probe_peer",
                    "action.probe_peer",
                    NodeCommandKind::ProbePeer,
                    NodeUiBackendStatus::Implemented,
                    NodePermission::ProbePeers,
                    NodeUiAvailability::NetworkOnline,
                    vec![
                        field(
                            "network_name",
                            "field.network_name",
                            NodeUiInputKind::NetworkName,
                            true,
                            NodeUiValidation::NetworkName,
                        ),
                        field(
                            "target",
                            "field.peer_target",
                            NodeUiInputKind::PeerTarget,
                            true,
                            NodeUiValidation::NonEmpty,
                        ),
                        field(
                            "timeout_ms",
                            "field.timeout_ms",
                            NodeUiInputKind::TimeoutMs,
                            true,
                            NodeUiValidation::PositiveTimeout,
                        ),
                    ],
                    None,
                )],
            ),
            page("topology", "nav.topology", "page.topology", vec![]),
            page("traffic", "nav.traffic", "page.traffic", vec![]),
            page(
                "routing",
                "nav.routing",
                "page.routing",
                vec![action(
                    "update_routing",
                    "action.update_routing",
                    NodeCommandKind::UpdateRouting,
                    NodeUiBackendStatus::Implemented,
                    NodePermission::ChangeRouting,
                    NodeUiAvailability::NetworkSelected,
                    vec![
                        field(
                            "network_name",
                            "field.network_name",
                            NodeUiInputKind::NetworkName,
                            true,
                            NodeUiValidation::NetworkName,
                        ),
                        field(
                            "mode",
                            "field.routing_mode",
                            NodeUiInputKind::RoutingMode,
                            true,
                            NodeUiValidation::RoutingMode,
                        ),
                        field(
                            "exit_node",
                            "field.exit_node",
                            NodeUiInputKind::PeerTarget,
                            false,
                            NodeUiValidation::None,
                        ),
                        field(
                            "is_exit_node",
                            "field.is_exit_node",
                            NodeUiInputKind::Boolean,
                            true,
                            NodeUiValidation::None,
                        ),
                    ],
                    Some(confirmation(
                        "confirm.update_routing.title",
                        "confirm.update_routing.consequence",
                        false,
                    )),
                )],
            ),
            page(
                "exit_node",
                "nav.exit_node",
                "page.exit_node",
                vec![action(
                    "set_exit_node_role",
                    "action.set_exit_node_role",
                    NodeCommandKind::SetNetworkRole,
                    NodeUiBackendStatus::Implemented,
                    NodePermission::ManageRoles,
                    NodeUiAvailability::NetworkSelected,
                    vec![
                        field(
                            "network_name",
                            "field.network_name",
                            NodeUiInputKind::NetworkName,
                            true,
                            NodeUiValidation::NetworkName,
                        ),
                        field(
                            "role",
                            "field.role",
                            NodeUiInputKind::Role,
                            true,
                            NodeUiValidation::Role,
                        ),
                        field(
                            "enabled",
                            "field.enabled",
                            NodeUiInputKind::Boolean,
                            true,
                            NodeUiValidation::None,
                        ),
                    ],
                    Some(confirmation(
                        "confirm.set_exit_node_role.title",
                        "confirm.set_exit_node_role.consequence",
                        false,
                    )),
                )],
            ),
            page(
                "relay",
                "nav.relay",
                "page.relay",
                vec![action(
                    "set_relay_role",
                    "action.set_relay_role",
                    NodeCommandKind::SetNetworkRole,
                    NodeUiBackendStatus::Unavailable,
                    NodePermission::ManageRoles,
                    NodeUiAvailability::NetworkSelected,
                    vec![
                        field(
                            "network_name",
                            "field.network_name",
                            NodeUiInputKind::NetworkName,
                            true,
                            NodeUiValidation::NetworkName,
                        ),
                        field(
                            "role",
                            "field.role",
                            NodeUiInputKind::Role,
                            true,
                            NodeUiValidation::Role,
                        ),
                        field(
                            "enabled",
                            "field.enabled",
                            NodeUiInputKind::Boolean,
                            true,
                            NodeUiValidation::None,
                        ),
                    ],
                    Some(confirmation(
                        "confirm.set_relay_role.title",
                        "confirm.set_relay_role.consequence",
                        false,
                    )),
                )],
            ),
            page("logs", "nav.logs", "page.logs", vec![]),
            page("identity", "nav.identity", "page.identity", vec![]),
            page(
                "settings",
                "nav.settings",
                "page.settings",
                vec![action(
                    "shutdown_node",
                    "action.shutdown_node",
                    NodeCommandKind::Shutdown,
                    NodeUiBackendStatus::Implemented,
                    NodePermission::StopNode,
                    NodeUiAvailability::NodeRunning,
                    vec![],
                    Some(confirmation(
                        "confirm.shutdown_node.title",
                        "confirm.shutdown_node.consequence",
                        true,
                    )),
                )],
            ),
        ],
    }
}

fn page(id: &str, nav_label_key: &str, title_key: &str, actions: Vec<NodeUiAction>) -> NodeUiPage {
    NodeUiPage {
        id: id.into(),
        nav_label_key: nav_label_key.into(),
        title_key: title_key.into(),
        actions,
    }
}

#[allow(clippy::too_many_arguments)]
fn action(
    id: &str,
    label_key: &str,
    command: NodeCommandKind,
    backend_status: NodeUiBackendStatus,
    permission: NodePermission,
    availability: NodeUiAvailability,
    fields: Vec<NodeUiField>,
    confirmation: Option<NodeUiConfirmation>,
) -> NodeUiAction {
    NodeUiAction {
        id: id.into(),
        label_key: label_key.into(),
        command,
        backend_status,
        permission,
        audit: audit_for_command(command),
        availability,
        fields,
        confirmation,
    }
}

fn field(
    id: &str,
    label_key: &str,
    input: NodeUiInputKind,
    required: bool,
    validation: NodeUiValidation,
) -> NodeUiField {
    NodeUiField {
        id: id.into(),
        label_key: label_key.into(),
        input,
        required,
        validation,
    }
}

fn confirmation(title_key: &str, consequence_key: &str, destructive: bool) -> NodeUiConfirmation {
    NodeUiConfirmation {
        title_key: title_key.into(),
        consequence_key: consequence_key.into(),
        destructive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn canonical_manifest_passes_contract_validation() {
        node_ui_manifest()
            .validate()
            .expect("canonical manifest must be valid");
    }

    #[test]
    fn validation_rejects_page_reordering_and_removal() {
        let mut manifest = node_ui_manifest();
        manifest.pages.swap(0, 1);
        let violations = manifest.validate().expect_err("reordered pages must fail");
        assert!(violations
            .iter()
            .any(|violation| violation.starts_with("page_set_or_order_mismatch:")));

        let mut manifest = node_ui_manifest();
        manifest.pages.pop();
        let violations = manifest.validate().expect_err("missing page must fail");
        assert!(violations
            .iter()
            .any(|violation| violation.starts_with("page_set_or_order_mismatch:")));
    }

    #[test]
    fn validation_rejects_duplicate_fields_and_unvalidated_required_inputs() {
        let mut manifest = node_ui_manifest();
        let action = manifest
            .pages
            .iter_mut()
            .flat_map(|page| &mut page.actions)
            .find(|action| action.command == NodeCommandKind::CreateNetwork)
            .expect("create action");
        action.fields[1].id = action.fields[0].id.clone();
        action.fields[0].validation = NodeUiValidation::None;

        let violations = manifest.validate().expect_err("malformed fields must fail");
        assert!(violations
            .iter()
            .any(|violation| violation.starts_with("duplicate_field_id:")));
        assert!(violations
            .iter()
            .any(|violation| violation.starts_with("required_field_has_no_validation:")));
    }

    #[test]
    fn validation_rejects_inconsistent_command_semantics() {
        let mut manifest = node_ui_manifest();
        let relay = manifest
            .pages
            .iter_mut()
            .find(|page| page.id == "relay")
            .expect("relay page");
        relay.actions[0].permission = NodePermission::ViewState;

        let violations = manifest
            .validate()
            .expect_err("command permissions must agree across surfaces");
        assert!(violations
            .iter()
            .any(|violation| violation.starts_with("inconsistent_permission:set_network_role:")));
    }

    #[test]
    fn every_action_declares_the_canonical_audit_policy() {
        let manifest = node_ui_manifest();
        for action in manifest.pages.iter().flat_map(|page| &page.actions) {
            assert_eq!(action.audit, audit_for_command(action.command));
        }

        let snapshot = manifest
            .pages
            .iter()
            .flat_map(|page| &page.actions)
            .find(|action| action.command == NodeCommandKind::GetSnapshot)
            .expect("snapshot action");
        assert_eq!(snapshot.audit.policy, NodeUiAuditPolicy::None);
        assert!(snapshot.audit.action.is_none());

        let shutdown = manifest
            .pages
            .iter()
            .flat_map(|page| &page.actions)
            .find(|action| action.command == NodeCommandKind::Shutdown)
            .expect("shutdown action");
        assert_eq!(shutdown.audit.policy, NodeUiAuditPolicy::RecordOutcome);
        assert_eq!(shutdown.audit.action.as_deref(), Some("node.shutdown"));
    }

    #[test]
    fn validation_rejects_actions_without_audit_declarations() {
        let mut manifest = node_ui_manifest();
        let action = manifest
            .pages
            .iter_mut()
            .flat_map(|page| &mut page.actions)
            .find(|action| action.command == NodeCommandKind::CreateNetwork)
            .expect("create action");
        action.audit = NodeUiAudit::none();

        let violations = manifest
            .validate()
            .expect_err("mutating actions must declare audit");
        assert!(violations
            .iter()
            .any(|violation| violation.starts_with("audit_declaration_mismatch:create_network:")));
    }

    #[test]
    fn manifest_declares_the_complete_required_page_set_once() {
        let manifest = node_ui_manifest();
        let pages: HashSet<_> = manifest.pages.iter().map(|page| page.id.as_str()).collect();
        let expected = HashSet::from([
            "overview",
            "networks",
            "peers",
            "topology",
            "traffic",
            "routing",
            "exit_node",
            "relay",
            "logs",
            "identity",
            "settings",
        ]);

        assert_eq!(manifest.schema_version, NODE_UI_MANIFEST_SCHEMA_VERSION);
        assert_eq!(pages.len(), manifest.pages.len());
        assert_eq!(pages, expected);
    }

    #[test]
    fn page_and_action_ids_are_unique_and_fields_are_validated() {
        let manifest = node_ui_manifest();
        let mut action_ids = HashSet::new();

        for page in &manifest.pages {
            for action in &page.actions {
                assert!(
                    action_ids.insert(action.id.as_str()),
                    "duplicate {}",
                    action.id
                );
                for field in &action.fields {
                    assert!(
                        !(field.required
                            && field.input != NodeUiInputKind::Boolean
                            && field.validation == NodeUiValidation::None),
                        "required field {} has no validation",
                        field.id
                    );
                }
                if action.backend_status == NodeUiBackendStatus::Unavailable {
                    assert!(!action.fields.is_empty());
                }
            }
        }
    }

    #[test]
    fn visible_actions_exactly_cover_canonical_action_commands() {
        let manifest = node_ui_manifest();
        let visible = manifest
            .pages
            .iter()
            .flat_map(|page| &page.actions)
            .map(|action| action.command)
            .collect::<HashSet<_>>();
        let canonical = NodeCommandKind::ACTIONS.into_iter().collect::<HashSet<_>>();

        assert_eq!(visible, canonical);
        assert!(!visible.contains(&NodeCommandKind::GetUiManifest));
    }

    #[test]
    fn manifest_backend_status_matches_the_live_command_handler() {
        let manifest = node_ui_manifest();
        let status = |command| {
            manifest
                .pages
                .iter()
                .flat_map(|page| &page.actions)
                .filter(|action| action.command == command)
                .map(|action| action.backend_status)
                .collect::<HashSet<_>>()
        };

        assert_eq!(
            status(NodeCommandKind::GetSnapshot),
            HashSet::from([NodeUiBackendStatus::Implemented])
        );
        assert_eq!(
            status(NodeCommandKind::StopNetworks),
            HashSet::from([NodeUiBackendStatus::Implemented])
        );
        assert_eq!(
            status(NodeCommandKind::RestartNetworks),
            HashSet::from([NodeUiBackendStatus::Implemented])
        );
        assert_eq!(
            status(NodeCommandKind::ProbePeer),
            HashSet::from([NodeUiBackendStatus::Implemented])
        );
        assert_eq!(
            status(NodeCommandKind::Shutdown),
            HashSet::from([NodeUiBackendStatus::Implemented])
        );
        for command in [
            NodeCommandKind::StartNetworks,
            NodeCommandKind::CreateNetwork,
            NodeCommandKind::JoinNetwork,
        ] {
            assert_eq!(
                status(command),
                HashSet::from([NodeUiBackendStatus::Implemented])
            );
        }
        assert_eq!(
            status(NodeCommandKind::UpdateRouting),
            HashSet::from([NodeUiBackendStatus::Implemented])
        );
        assert_eq!(
            status(NodeCommandKind::SetNetworkRole),
            HashSet::from([
                NodeUiBackendStatus::Implemented,
                NodeUiBackendStatus::Unavailable,
            ])
        );
    }

    #[test]
    fn destructive_actions_require_explicit_confirmation() {
        let manifest = node_ui_manifest();
        let shutdown = manifest
            .pages
            .iter()
            .flat_map(|page| &page.actions)
            .find(|action| action.command == NodeCommandKind::Shutdown)
            .expect("shutdown action");

        assert!(shutdown
            .confirmation
            .as_ref()
            .is_some_and(|confirmation| confirmation.destructive));
    }

    #[test]
    fn manifest_does_not_introduce_product_client_server_identities() {
        let encoded = serde_json::to_string(&node_ui_manifest()).expect("serialize manifest");

        assert!(!encoded.contains("client_mode"));
        assert!(!encoded.contains("server_mode"));
        assert!(encoded.contains("set_exit_node_role"));
        assert!(encoded.contains("set_relay_role"));
    }

    #[test]
    fn action_permissions_are_part_of_the_shared_permission_snapshot() {
        let manifest = node_ui_manifest();
        let granted = crate::node::runtime_metadata::trusted_control_plane_permissions()
            .permissions
            .into_iter()
            .collect::<HashSet<_>>();

        for action in manifest.pages.iter().flat_map(|page| &page.actions) {
            assert!(
                granted.contains(&action.permission),
                "action {} uses an ungranted permission",
                action.id
            );
        }
    }
}
