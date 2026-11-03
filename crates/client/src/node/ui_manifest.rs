use std::collections::{HashMap, HashSet};

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};

use super::command::NodeCommandKind;
use super::state::NodePermission;

pub const NODE_UI_MANIFEST_SCHEMA_VERSION: u16 = 1;

/// Canonical node-page states shared by every UI adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeUiPageState {
    InitialLoading,
    Error,
    Maintenance,
    PermissionDenied,
    Unsupported,
    Offline,
    ControlUnavailable,
    NodeFailed,
    Degraded,
    Stale,
    Refreshing,
    Empty,
    Ready,
}

pub const NODE_UI_PAGE_STATES: [NodeUiPageState; 13] = [
    NodeUiPageState::InitialLoading,
    NodeUiPageState::Error,
    NodeUiPageState::Maintenance,
    NodeUiPageState::PermissionDenied,
    NodeUiPageState::Unsupported,
    NodeUiPageState::Offline,
    NodeUiPageState::ControlUnavailable,
    NodeUiPageState::NodeFailed,
    NodeUiPageState::Degraded,
    NodeUiPageState::Stale,
    NodeUiPageState::Refreshing,
    NodeUiPageState::Empty,
    NodeUiPageState::Ready,
];

/// Canonical reasons that can disable a manifest action. The enum is flat so
/// WebUI, GPUI, generated bindings and Android consume the same fail-closed
/// vocabulary even when their local capability snapshots have richer details.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeUiActionBlockReason {
    NodeNotRunning,
    NoOnlineNetwork,
    NoNetworkSelected,
    PermissionDenied,
    PlatformUnsupported,
    ExitNodeUnsupported,
    TunPlatformUnsupported,
    TunPermissionDenied,
    TunDeviceUnavailable,
    TunToolUnavailable,
    TunUnavailable,
    BackendUnavailable,
}

pub const NODE_UI_ACTION_BLOCK_REASONS: [NodeUiActionBlockReason; 12] = [
    NodeUiActionBlockReason::NodeNotRunning,
    NodeUiActionBlockReason::NoOnlineNetwork,
    NodeUiActionBlockReason::NoNetworkSelected,
    NodeUiActionBlockReason::PermissionDenied,
    NodeUiActionBlockReason::PlatformUnsupported,
    NodeUiActionBlockReason::ExitNodeUnsupported,
    NodeUiActionBlockReason::TunPlatformUnsupported,
    NodeUiActionBlockReason::TunPermissionDenied,
    NodeUiActionBlockReason::TunDeviceUnavailable,
    NodeUiActionBlockReason::TunToolUnavailable,
    NodeUiActionBlockReason::TunUnavailable,
    NodeUiActionBlockReason::BackendUnavailable,
];

impl NodeUiActionBlockReason {
    pub const fn message_key(self) -> &'static str {
        match self {
            Self::NodeNotRunning => "blocked.node_not_running",
            Self::NoOnlineNetwork => "blocked.no_online_network",
            Self::NoNetworkSelected => "blocked.no_network_selected",
            Self::PermissionDenied => "blocked.permission_denied",
            Self::PlatformUnsupported => "blocked.platform_unsupported",
            Self::ExitNodeUnsupported => "blocked.exit_node_unsupported",
            Self::TunPlatformUnsupported => "blocked.tun_platform_unsupported",
            Self::TunPermissionDenied => "blocked.tun_permission_denied",
            Self::TunDeviceUnavailable => "blocked.tun_device_unavailable",
            Self::TunToolUnavailable => "blocked.tun_tool_unavailable",
            Self::TunUnavailable => "blocked.tun_unavailable",
            Self::BackendUnavailable => "blocked.backend_unavailable",
        }
    }
}

/// Declarative contract shared by WebUI and the future native GPUI surface.
///
/// Adapters may choose different widgets and rendering, but page availability,
/// commands, fields, validation, confirmations, permission classes and backend
/// maturity come from this manifest rather than duplicated UI business logic.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeUiManifest {
    pub schema_version: u16,
    pub page_states: Vec<NodeUiPageState>,
    pub action_block_reasons: Vec<NodeUiActionBlockReason>,
    pub pages: Vec<NodeUiPage>,
}

impl NodeUiManifest {
    /// Validate the cross-surface contract before either WebUI or GPUI uses it.
    ///
    /// The manifest is a compatibility boundary, not trusted configuration.
    /// A malformed manifest must fail closed instead of allowing each renderer
    /// to invent a different page set, field rule, permission, or backend state.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        const REQUIRED_PAGES: [&str; 12] = [
            "overview",
            "networks",
            "peers",
            "node_runtime",
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

        if self.page_states != NODE_UI_PAGE_STATES {
            violations.push(format!(
                "page_state_set_or_order_mismatch:expected={NODE_UI_PAGE_STATES:?}:actual={:?}",
                self.page_states
            ));
        }
        if self.action_block_reasons != NODE_UI_ACTION_BLOCK_REASONS {
            violations.push(format!(
                "action_block_reason_set_or_order_mismatch:expected={NODE_UI_ACTION_BLOCK_REASONS:?}:actual={:?}",
                self.action_block_reasons
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
                let requires_recent_auth = action.command.requires_recent_reauthentication();
                if requires_recent_auth
                    && !action
                        .confirmation
                        .as_ref()
                        .is_some_and(|confirmation| confirmation.recent_authentication_required)
                {
                    violations.push(format!(
                        "sensitive_command_requires_reauthentication_confirmation:{context}:{}",
                        action.command.as_str()
                    ));
                }
                if let Some(confirmation) = &action.confirmation {
                    if confirmation.recent_authentication_required != requires_recent_auth {
                        violations.push(format!(
                            "confirmation_reauthentication_mismatch:{context}:{}",
                            action.command.as_str()
                        ));
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
    /// Human-readable scope and impact shown before submission.
    pub consequence_key: String,
    pub destructive: bool,
    pub recent_authentication_required: bool,
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
        NodeCommandKind::GetSnapshot
        | NodeCommandKind::GetUiManifest
        | NodeCommandKind::GetDiagnostics
        | NodeCommandKind::GetConfiguration
        | NodeCommandKind::PreviewConfiguration
        | NodeCommandKind::GetConfigurationHistory => NodeUiAudit::none(),
        NodeCommandKind::CreateNetwork => NodeUiAudit::record("network.create"),
        NodeCommandKind::JoinNetwork => NodeUiAudit::record("network.join"),
        NodeCommandKind::StartNetworks => NodeUiAudit::record("network.start"),
        NodeCommandKind::StopNetworks => NodeUiAudit::record("network.stop"),
        NodeCommandKind::RestartNetworks => NodeUiAudit::record("network.restart"),
        NodeCommandKind::RebuildTun => NodeUiAudit::record("node.tun.rebuild"),
        NodeCommandKind::UpdateRouting => NodeUiAudit::record("routing.update"),
        NodeCommandKind::SetNetworkRole => NodeUiAudit::record("network.role.set"),
        NodeCommandKind::SetNodeDisplayName => NodeUiAudit::record("node.identity.rename"),
        NodeCommandKind::RotateNodeIdentity => NodeUiAudit::record("node.identity.rotate"),
        NodeCommandKind::GetBlacklist => NodeUiAudit::none(),
        NodeCommandKind::BanClient => NodeUiAudit::record("network.blacklist.ban"),
        NodeCommandKind::UnbanClient => NodeUiAudit::record("network.blacklist.unban"),
        NodeCommandKind::GetNetworkAllocation => NodeUiAudit::none(),
        NodeCommandKind::UpdateNetworkAllocation => {
            NodeUiAudit::record("network.allocation.update")
        }
        NodeCommandKind::ProbePeer => NodeUiAudit::record("peer.probe"),
        NodeCommandKind::StartNode => NodeUiAudit::record("node.start"),
        NodeCommandKind::StopNode => NodeUiAudit::record("node.stop"),
        NodeCommandKind::RestartNode => NodeUiAudit::record("node.restart"),
        NodeCommandKind::CleanupResiduals => NodeUiAudit::record("node.cleanup_residuals"),
        NodeCommandKind::ApplyConfiguration => NodeUiAudit::record("configuration.apply"),
        NodeCommandKind::RollbackConfiguration => NodeUiAudit::record("configuration.rollback"),
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
    ClientId,
    ConfigurationJson,
    DisplayName,
    ExternalHost,
    Fingerprint,
    NetworkMode,
    NetworkName,
    NetworkSelection,
    PeerTarget,
    Purpose,
    Icon,
    Language,
    Visibility,
    JoinMode,
    Port,
    Revision,
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
    NetworkMode,
    NetworkSelection,
    Role,
    RoutingMode,
    PositiveTimeout,
    Revision,
    ConfigurationJson,
    DisplayName,
    Fingerprint,
    NetworkPurpose,
    NetworkIcon,
    NetworkLanguage,
    Visibility,
    JoinMode,
}

pub fn node_ui_manifest() -> NodeUiManifest {
    NodeUiManifest {
        schema_version: NODE_UI_MANIFEST_SCHEMA_VERSION,
        page_states: NODE_UI_PAGE_STATES.to_vec(),
        action_block_reasons: NODE_UI_ACTION_BLOCK_REASONS.to_vec(),
        pages: vec![
            page("overview", "nav.overview", "page.overview", vec![]),
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
                                "purpose",
                                "field.purpose",
                                NodeUiInputKind::Purpose,
                                true,
                                NodeUiValidation::NetworkPurpose,
                            ),
                            field(
                                "icon",
                                "field.icon",
                                NodeUiInputKind::Icon,
                                true,
                                NodeUiValidation::NetworkIcon,
                            ),
                            field(
                                "language",
                                "field.language",
                                NodeUiInputKind::Language,
                                true,
                                NodeUiValidation::NetworkLanguage,
                            ),
                            field(
                                "visibility",
                                "field.visibility",
                                NodeUiInputKind::Visibility,
                                true,
                                NodeUiValidation::Visibility,
                            ),
                            field(
                                "join_mode",
                                "field.join_mode",
                                NodeUiInputKind::JoinMode,
                                true,
                                NodeUiValidation::JoinMode,
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
                            NodeCommandKind::StopNetworks,
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
                            NodeCommandKind::RestartNetworks,
                            "confirm.restart_networks.title",
                            "confirm.restart_networks.consequence",
                            false,
                        )),
                    ),
                    action(
                        "rebuild_tun",
                        "action.rebuild_tun",
                        NodeCommandKind::RebuildTun,
                        NodeUiBackendStatus::Implemented,
                        NodePermission::ManageNetworks,
                        NodeUiAvailability::NetworkOnline,
                        vec![field(
                            "network_name",
                            "field.network_name",
                            NodeUiInputKind::NetworkName,
                            true,
                            NodeUiValidation::NetworkName,
                        )],
                        Some(confirmation(
                            NodeCommandKind::RebuildTun,
                            "confirm.rebuild_tun.title",
                            "confirm.rebuild_tun.consequence",
                            false,
                        )),
                    ),
                ],
            ),
            page(
                "peers",
                "nav.peers",
                "page.peers",
                vec![
                    action(
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
                    ),
                    action(
                        "get_blacklist",
                        "action.get_blacklist",
                        NodeCommandKind::GetBlacklist,
                        NodeUiBackendStatus::Implemented,
                        NodePermission::ViewState,
                        NodeUiAvailability::Always,
                        vec![field(
                            "network_name",
                            "field.network_name",
                            NodeUiInputKind::NetworkName,
                            true,
                            NodeUiValidation::NetworkName,
                        )],
                        None,
                    ),
                    action(
                        "ban_client",
                        "action.ban_client",
                        NodeCommandKind::BanClient,
                        NodeUiBackendStatus::Implemented,
                        NodePermission::ManageNetworks,
                        NodeUiAvailability::Always,
                        vec![
                            field(
                                "network_name",
                                "field.network_name",
                                NodeUiInputKind::NetworkName,
                                true,
                                NodeUiValidation::NetworkName,
                            ),
                            field(
                                "client_id",
                                "field.client_id",
                                NodeUiInputKind::ClientId,
                                true,
                                NodeUiValidation::NonEmpty,
                            ),
                        ],
                        Some(confirmation(
                            NodeCommandKind::BanClient,
                            "confirm.ban_client.title",
                            "confirm.ban_client.consequence",
                            false,
                        )),
                    ),
                    action(
                        "unban_client",
                        "action.unban_client",
                        NodeCommandKind::UnbanClient,
                        NodeUiBackendStatus::Implemented,
                        NodePermission::ManageNetworks,
                        NodeUiAvailability::Always,
                        vec![
                            field(
                                "network_name",
                                "field.network_name",
                                NodeUiInputKind::NetworkName,
                                true,
                                NodeUiValidation::NetworkName,
                            ),
                            field(
                                "client_id",
                                "field.client_id",
                                NodeUiInputKind::ClientId,
                                true,
                                NodeUiValidation::NonEmpty,
                            ),
                        ],
                        None,
                    ),
                ],
            ),
            page(
                "node_runtime",
                "nav.node_runtime",
                "page.node_runtime",
                vec![
                    action(
                        "refresh_snapshot",
                        "action.refresh_snapshot",
                        NodeCommandKind::GetSnapshot,
                        NodeUiBackendStatus::Implemented,
                        NodePermission::ViewState,
                        NodeUiAvailability::Always,
                        vec![],
                        None,
                    ),
                    action(
                        "get_node_diagnostics",
                        "action.get_node_diagnostics",
                        NodeCommandKind::GetDiagnostics,
                        NodeUiBackendStatus::Implemented,
                        NodePermission::ViewState,
                        NodeUiAvailability::Always,
                        vec![],
                        None,
                    ),
                    action(
                        "start_node",
                        "action.start_node",
                        NodeCommandKind::StartNode,
                        NodeUiBackendStatus::Implemented,
                        NodePermission::ManageNetworks,
                        NodeUiAvailability::Always,
                        vec![],
                        None,
                    ),
                    action(
                        "stop_node",
                        "action.stop_node",
                        NodeCommandKind::StopNode,
                        NodeUiBackendStatus::Implemented,
                        NodePermission::StopNode,
                        NodeUiAvailability::NodeRunning,
                        vec![],
                        None,
                    ),
                    action(
                        "restart_node",
                        "action.restart_node",
                        NodeCommandKind::RestartNode,
                        NodeUiBackendStatus::Implemented,
                        NodePermission::ManageNetworks,
                        NodeUiAvailability::NodeRunning,
                        vec![],
                        Some(confirmation(
                            NodeCommandKind::RestartNode,
                            "confirm.restart_node.title",
                            "confirm.restart_node.consequence",
                            true,
                        )),
                    ),
                    action(
                        "cleanup_node_residuals",
                        "action.cleanup_node_residuals",
                        NodeCommandKind::CleanupResiduals,
                        NodeUiBackendStatus::Implemented,
                        NodePermission::ManageNetworks,
                        NodeUiAvailability::Always,
                        vec![
                            field(
                                "networks",
                                "field.network_names",
                                NodeUiInputKind::NetworkSelection,
                                false,
                                NodeUiValidation::NetworkSelection,
                            ),
                            field(
                                "all",
                                "field.all_networks",
                                NodeUiInputKind::Boolean,
                                true,
                                NodeUiValidation::None,
                            ),
                        ],
                        Some(confirmation(
                            NodeCommandKind::CleanupResiduals,
                            "confirm.cleanup_node_residuals.title",
                            "confirm.cleanup_node_residuals.consequence",
                            false,
                        )),
                    ),
                ],
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
                        NodeCommandKind::UpdateRouting,
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
                        NodeCommandKind::SetNetworkRole,
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
                        NodeCommandKind::SetNetworkRole,
                        "confirm.set_relay_role.title",
                        "confirm.set_relay_role.consequence",
                        false,
                    )),
                )],
            ),
            page("logs", "nav.logs", "page.logs", vec![]),
            page(
                "identity",
                "nav.identity",
                "page.identity",
                vec![
                    action(
                        "set_node_display_name",
                        "action.set_node_display_name",
                        NodeCommandKind::SetNodeDisplayName,
                        NodeUiBackendStatus::Implemented,
                        NodePermission::ManageNetworks,
                        NodeUiAvailability::Always,
                        vec![field(
                            "display_name",
                            "field.display_name",
                            NodeUiInputKind::DisplayName,
                            false,
                            NodeUiValidation::DisplayName,
                        )],
                        Some(confirmation(
                            NodeCommandKind::SetNodeDisplayName,
                            "confirm.set_node_display_name.title",
                            "confirm.set_node_display_name.consequence",
                            false,
                        )),
                    ),
                    action(
                        "rotate_node_identity",
                        "action.rotate_node_identity",
                        NodeCommandKind::RotateNodeIdentity,
                        NodeUiBackendStatus::Implemented,
                        NodePermission::ManageNetworks,
                        NodeUiAvailability::Always,
                        vec![field(
                            "expected_fingerprint",
                            "field.expected_fingerprint",
                            NodeUiInputKind::Fingerprint,
                            true,
                            NodeUiValidation::Fingerprint,
                        )],
                        Some(confirmation(
                            NodeCommandKind::RotateNodeIdentity,
                            "confirm.rotate_node_identity.title",
                            "confirm.rotate_node_identity.consequence",
                            true,
                        )),
                    ),
                ],
            ),
            page(
                "settings",
                "nav.settings",
                "page.settings",
                vec![
                    action(
                        "get_network_allocation",
                        "action.get_network_allocation",
                        NodeCommandKind::GetNetworkAllocation,
                        NodeUiBackendStatus::Implemented,
                        NodePermission::ViewState,
                        NodeUiAvailability::Always,
                        vec![field(
                            "network_name",
                            "field.network_name",
                            NodeUiInputKind::NetworkName,
                            true,
                            NodeUiValidation::NetworkName,
                        )],
                        None,
                    ),
                    action(
                        "update_network_allocation",
                        "action.update_network_allocation",
                        NodeCommandKind::UpdateNetworkAllocation,
                        NodeUiBackendStatus::Implemented,
                        NodePermission::ManageNetworks,
                        NodeUiAvailability::Always,
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
                                "field.network_mode",
                                NodeUiInputKind::NetworkMode,
                                true,
                                NodeUiValidation::NetworkMode,
                            ),
                            field(
                                "ipv4_pool",
                                "field.ipv4_pool",
                                NodeUiInputKind::ExternalHost,
                                false,
                                NodeUiValidation::NonEmpty,
                            ),
                            field(
                                "ipv6_pool",
                                "field.ipv6_pool",
                                NodeUiInputKind::ExternalHost,
                                false,
                                NodeUiValidation::NonEmpty,
                            ),
                        ],
                        Some(confirmation(
                            NodeCommandKind::UpdateNetworkAllocation,
                            "confirm.update_network_allocation.title",
                            "confirm.update_network_allocation.consequence",
                            false,
                        )),
                    ),
                    action(
                        "get_configuration",
                        "action.get_configuration",
                        NodeCommandKind::GetConfiguration,
                        NodeUiBackendStatus::Implemented,
                        NodePermission::ViewState,
                        NodeUiAvailability::Always,
                        vec![],
                        None,
                    ),
                    action(
                        "preview_configuration",
                        "action.preview_configuration",
                        NodeCommandKind::PreviewConfiguration,
                        NodeUiBackendStatus::Implemented,
                        NodePermission::ViewState,
                        NodeUiAvailability::Always,
                        vec![
                            field(
                                "expected_revision",
                                "field.expected_revision",
                                NodeUiInputKind::Revision,
                                true,
                                NodeUiValidation::Revision,
                            ),
                            field(
                                "networks",
                                "field.configuration_networks",
                                NodeUiInputKind::ConfigurationJson,
                                true,
                                NodeUiValidation::ConfigurationJson,
                            ),
                        ],
                        None,
                    ),
                    action(
                        "apply_configuration",
                        "action.apply_configuration",
                        NodeCommandKind::ApplyConfiguration,
                        NodeUiBackendStatus::Implemented,
                        NodePermission::ManageNetworks,
                        NodeUiAvailability::Always,
                        vec![
                            field(
                                "expected_revision",
                                "field.expected_revision",
                                NodeUiInputKind::Revision,
                                true,
                                NodeUiValidation::Revision,
                            ),
                            field(
                                "networks",
                                "field.configuration_networks",
                                NodeUiInputKind::ConfigurationJson,
                                true,
                                NodeUiValidation::ConfigurationJson,
                            ),
                        ],
                        Some(confirmation(
                            NodeCommandKind::ApplyConfiguration,
                            "confirm.apply_configuration.title",
                            "confirm.apply_configuration.consequence",
                            false,
                        )),
                    ),
                    action(
                        "rollback_configuration",
                        "action.rollback_configuration",
                        NodeCommandKind::RollbackConfiguration,
                        NodeUiBackendStatus::Implemented,
                        NodePermission::ManageNetworks,
                        NodeUiAvailability::Always,
                        vec![
                            field(
                                "expected_revision",
                                "field.expected_revision",
                                NodeUiInputKind::Revision,
                                true,
                                NodeUiValidation::Revision,
                            ),
                            field(
                                "target_revision",
                                "field.target_revision",
                                NodeUiInputKind::Revision,
                                true,
                                NodeUiValidation::Revision,
                            ),
                        ],
                        Some(confirmation(
                            NodeCommandKind::RollbackConfiguration,
                            "confirm.rollback_configuration.title",
                            "confirm.rollback_configuration.consequence",
                            true,
                        )),
                    ),
                    action(
                        "get_configuration_history",
                        "action.get_configuration_history",
                        NodeCommandKind::GetConfigurationHistory,
                        NodeUiBackendStatus::Implemented,
                        NodePermission::ViewState,
                        NodeUiAvailability::Always,
                        vec![],
                        None,
                    ),
                    action(
                        "shutdown_node",
                        "action.shutdown_node",
                        NodeCommandKind::Shutdown,
                        NodeUiBackendStatus::Implemented,
                        NodePermission::StopNode,
                        NodeUiAvailability::NodeRunning,
                        vec![],
                        Some(confirmation(
                            NodeCommandKind::Shutdown,
                            "confirm.shutdown_node.title",
                            "confirm.shutdown_node.consequence",
                            true,
                        )),
                    ),
                ],
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

fn confirmation(
    command: NodeCommandKind,
    title_key: &str,
    consequence_key: &str,
    destructive: bool,
) -> NodeUiConfirmation {
    NodeUiConfirmation {
        title_key: title_key.into(),
        consequence_key: consequence_key.into(),
        destructive,
        recent_authentication_required: command.requires_recent_reauthentication(),
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
    fn manifest_publishes_the_canonical_ui_state_vocabularies() {
        let manifest = node_ui_manifest();
        assert_eq!(manifest.page_states, NODE_UI_PAGE_STATES);
        assert_eq!(manifest.action_block_reasons, NODE_UI_ACTION_BLOCK_REASONS);
        assert_eq!(
            manifest
                .action_block_reasons
                .iter()
                .map(|reason| reason.message_key())
                .collect::<Vec<_>>(),
            vec![
                "blocked.node_not_running",
                "blocked.no_online_network",
                "blocked.no_network_selected",
                "blocked.permission_denied",
                "blocked.platform_unsupported",
                "blocked.exit_node_unsupported",
                "blocked.tun_platform_unsupported",
                "blocked.tun_permission_denied",
                "blocked.tun_device_unavailable",
                "blocked.tun_tool_unavailable",
                "blocked.tun_unavailable",
                "blocked.backend_unavailable",
            ]
        );
    }

    #[test]
    fn validation_rejects_ui_state_vocabulary_drift() {
        let mut manifest = node_ui_manifest();
        manifest.page_states.reverse();
        let violations = manifest.validate().expect_err("reordered states must fail");
        assert!(violations
            .iter()
            .any(|violation| violation.starts_with("page_state_set_or_order_mismatch:")));

        let mut manifest = node_ui_manifest();
        manifest.action_block_reasons.pop();
        let violations = manifest
            .validate()
            .expect_err("missing block reason must fail");
        assert!(violations.iter().any(|violation| {
            violation.starts_with("action_block_reason_set_or_order_mismatch:")
        }));
    }

    #[test]
    fn recent_authentication_policy_drives_server_and_manifest_confirmation() {
        let manifest = node_ui_manifest();
        for action in manifest.pages.iter().flat_map(|page| &page.actions) {
            let expected = action.command.requires_recent_reauthentication();
            assert_eq!(
                action
                    .confirmation
                    .as_ref()
                    .is_some_and(|confirmation| confirmation.recent_authentication_required),
                expected,
                "confirmation declaration drifted for {}",
                action.command.as_str()
            );
        }

        let rename = manifest
            .pages
            .iter()
            .flat_map(|page| &page.actions)
            .find(|action| action.command == NodeCommandKind::SetNodeDisplayName)
            .expect("rename action");
        assert!(rename
            .confirmation
            .as_ref()
            .is_some_and(|confirmation| { confirmation.recent_authentication_required }));
    }

    #[test]
    fn validation_rejects_missing_or_mismatched_reauthentication_warning() {
        let mut manifest = node_ui_manifest();
        let restart = manifest
            .pages
            .iter_mut()
            .flat_map(|page| &mut page.actions)
            .find(|action| action.command == NodeCommandKind::RestartNode)
            .expect("restart action")
            .confirmation
            .as_mut()
            .expect("restart confirmation");
        restart.recent_authentication_required = false;
        let violations = manifest
            .validate()
            .expect_err("missing reauthentication warning must fail");
        assert!(violations
            .iter()
            .any(|violation| { violation.starts_with("confirmation_reauthentication_mismatch:") }));

        let mut manifest = node_ui_manifest();
        let shutdown = manifest
            .pages
            .iter_mut()
            .flat_map(|page| &mut page.actions)
            .find(|action| action.command == NodeCommandKind::Shutdown)
            .expect("shutdown action");
        shutdown.confirmation = None;
        let violations = manifest
            .validate()
            .expect_err("sensitive command without confirmation must fail");
        assert!(violations.iter().any(|violation| {
            violation.starts_with("sensitive_command_requires_reauthentication_confirmation:")
        }));
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
            "node_runtime",
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
            NodeCommandKind::RebuildTun,
            NodeCommandKind::CreateNetwork,
            NodeCommandKind::JoinNetwork,
            NodeCommandKind::StartNode,
            NodeCommandKind::StopNode,
            NodeCommandKind::RestartNode,
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
        let restart = manifest
            .pages
            .iter()
            .flat_map(|page| &page.actions)
            .find(|action| action.command == NodeCommandKind::RestartNode)
            .expect("restart node action");
        assert!(restart
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
