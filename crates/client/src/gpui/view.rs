use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use fuckipv6_common::design_tokens::{DARK_THEME, DIMENSIONS};
use fuckipv6_common::domain::{
    ActivityCategory, ActivitySeverity, AuditOutcome, CapabilityKind, ExitContributionMode,
    ExitHealthEvidenceState, ExitHealthLayer, ExitHealthProtocol, ExitHealthSource,
    ExitHealthState, ExitHealthTarget, IdempotencyKey, InvitationPreview, InvitationState,
    InvitationStatusView, IssuedInvitation, JoinRequest, JoinRequestReviewAction, JoinRequestState,
    LeakDetectionDnsMode, LeakDetectionEvidenceState, LeakDetectionIsolationAction,
    LeakDetectionPlatform, LeakDetectionProtocol, LeakDetectionSource, LeakDetectionState,
    LeakDetectionTarget, MemberLifecycleAction, MembershipRole, NetworkId, NetworkJoinMode,
    NetworkMember, NetworkMemberDirectory, NetworkMemberGroup, NetworkMemberObservationSource,
    NetworkMemberOnlineState, NetworkMemberStatus, NetworkProfileApplyResult,
    NetworkProfilePreview, NetworkProfileProjections, NetworkProfileWarningCode,
    NetworkPublicProfileDocument, NetworkVisibility, OperationError, OperationState,
    TopologyConnectionState, TopologySnapshot, TrafficProtectionState,
    JOIN_REQUEST_MAX_REASON_CHARS,
};
#[cfg(test)]
use fuckipv6_common::domain::{
    ExitContributionObligation, LeakDetectionAssessment, LeakDetectionEnvironment,
    LeakDetectionEvidence, LeakDetectionSnapshot,
};
use fuckipv6_common::i18n::{
    detect_system_locale, format_decimal, format_integer, format_timestamp, resolve_language, t,
    tr, LanguagePreference, Locale,
};
use fuckipv6_common::ui_components::SemanticState;
use fuckipv6_common::ui_layout::{
    navigation_module_for_page, navigation_modules, ActionPanelPresentation, AppRegion,
    LayoutClass, NavigationModule,
};
use gpui::{
    actions, div, prelude::*, px, rgba, size, uniform_list, App, Bounds, ClipboardItem, Context,
    Div, Entity, FocusHandle, KeyBinding, Menu, MenuItem, Render, Role, SharedString, Window,
    WindowBounds, WindowOptions,
};
use gpui_base::input::{InputEvent, Textarea, TextareaState};

use super::components::{
    button, checkbox_control, choice_control, error_state as component_error_state, nav_tab,
    semantic_palette, tabs, toast as component_toast, ButtonVariant,
};
use super::configuration_workflow::{
    is_configuration_action, json_pointer_selection, ConfigurationOperation,
    GpuiConfigurationWorkflow,
};
use super::path_picker::{pick_paths, PathPickerMode};
use super::text_input::{NativeTextInput, TextInputEvent};
#[cfg(test)]
use super::view_model::build_page_model;
use super::view_model::{
    action_block_reason, action_coordinator_governance_freeze_reason, activity_filter_options,
    build_command, build_page_model_with_activity_filters,
    command_coordinator_governance_freeze_reason, coordinator_governance_freeze_reason,
    default_form_values, filter_network_members, member_exit_declared_text,
    member_exit_freshness_text, member_exit_reason_text, member_exit_source_text,
    member_exit_state_text, member_lifecycle_action_key, member_lifecycle_action_value,
    membership_role_value, network_membership_id, network_status_key, retain_member_selection,
    routing_target_search_rows, search_node_snapshot, sort_network_members,
    topology_member_actions, topology_node_detail, topology_presentation,
    traffic_protection_state_key, validate_manifest_bindings, ActionBinding, ActionBlockReason,
    ActivityFilters, CoordinatorGovernanceFreezeReason, MemberCapabilityFilter,
    MemberDirectoryFilters, MemberHealthFilter, OverviewModel, PageKind, RowModel, RowNavigation,
    SearchResult, SectionModel, TopologyPresentationCondition, TopologyPresentationSignals,
};
use crate::config_manager::ConfigManager;
use crate::config_transaction::{
    ConfigurationChangeAction, ConfigurationHistoryEntry, ConfigurationImpact,
};
use crate::discovery_index::NodeDiscoveryIndexErrorCategory;
use crate::network_discovery::{
    project_discovery, DiscoveryFilters, DiscoveryProjection, DiscoveryProjectionState,
    DiscoveryResult, DiscoveryVisibilityFilter,
};
use crate::node::{
    NodeApplyConfigurationCommand, NodeCommand, NodeCommandBus, NodeCommandBusError,
    NodeCommandContext, NodeCommandKind, NodeCommandOutcome, NodeCommandRequest, NodeCommandResult,
    NodeControlHealth, NodeDiagnosticCheck, NodeDiagnosticLayer, NodeDiagnosticState,
    NodeDiagnosticsSnapshot, NodeJoinRequestListResult, NodeJoinRequestReviewAck,
    NodeJoinRequestReviewItem, NodeNetworkCreationNextStep, NodeNetworkJoinPreview,
    NodeNetworkStatus, NodeOperationSnapshot, NodePermission, NodePreviewConfigurationCommand,
    NodePrimaryStatus, NodeProblem, NodeRollbackConfigurationCommand, NodeSnapshot, NodeStateStore,
    NodeUiAction, NodeUiBackendStatus, NodeUiField, NodeUiInputKind, NodeUiManifest, NodeUiPage,
    OperationClientError, NODE_UI_ACTION_GROUPS,
};
use crate::remote_node::RemoteOperationClient;

const WINDOW_WIDTH: f32 = DIMENSIONS.window_default_width_px as f32;
const WINDOW_HEIGHT: f32 = DIMENSIONS.window_default_height_px as f32;
const OPERATION_LOOKUP_DEADLINE: std::time::Duration = std::time::Duration::from_secs(120);
const OPERATION_LOOKUP_FAILURE_LIMIT: u16 = 300;

const COLOR_BG: u32 = DARK_THEME.colors.background.rgba_hex();
const COLOR_SIDEBAR: u32 = DARK_THEME.colors.background_deep.rgba_hex();
const COLOR_PANEL: u32 = DARK_THEME.colors.surface.rgba_hex();
const COLOR_PANEL_RAISED: u32 = DARK_THEME.colors.panel_raised.rgba_hex();
const COLOR_DIVIDER: u32 = DARK_THEME.colors.border.rgba_hex();
const COLOR_TEXT: u32 = DARK_THEME.colors.text.rgba_hex();
const COLOR_TEXT_SECONDARY: u32 = DARK_THEME.colors.text_secondary.rgba_hex();
const COLOR_MUTED: u32 = DARK_THEME.colors.text_muted.rgba_hex();
const COLOR_FAINT: u32 = DARK_THEME.colors.text_faint.rgba_hex();
const COLOR_ACCENT: u32 = DARK_THEME.colors.accent.rgba_hex();
const COLOR_WARNING: u32 = DARK_THEME.colors.warning.rgba_hex();

fn lucide_icon(data: &'static [u8], size: f32, color: u32) -> gpui::AnyElement {
    gpui::svg()
        .data(data)
        .w(px(size))
        .h(px(size))
        .text_color(rgba(color))
        .into_any_element()
}

fn surface_card() -> Div {
    let colors = DARK_THEME.colors;
    div()
        .rounded(px(16.0))
        .border_1()
        .border_color(rgba(COLOR_DIVIDER))
        .bg(gpui::linear_gradient(
            150.0,
            gpui::linear_color_stop(rgba(colors.surface.rgba_hex()), 0.0),
            gpui::linear_color_stop(
                rgba(colors.background_deep.with_alpha(0xeb).rgba_hex()),
                1.0,
            ),
        ))
        .shadow(vec![gpui::BoxShadow::new(
            px(0.0),
            px(1.0),
            rgba(0x00000066).into(),
        )
        .blur_radius(px(3.0))])
}

fn empty_state(text: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .min_h(px(130.0))
        .rounded(px(16.0))
        .text_size(px(12.0))
        .line_height(px(18.0))
        .text_color(rgba(COLOR_MUTED))
        .child(text.into())
}

fn card_heading(title: impl Into<SharedString>, icon: gpui::AnyElement) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .mb_3()
        .child(
            div()
                .text_size(px(14.0))
                .line_height(px(21.7))
                .font_weight(gpui::FontWeight(600.0))
                .text_color(rgba(COLOR_TEXT))
                .child(title.into()),
        )
        .child(icon)
}

fn navigation_icon(module_id: &str, size: f32, color: u32) -> gpui::AnyElement {
    let data = match module_id {
        "overview" => include_bytes!("../../assets/icons/gauge.svg").as_slice(),
        "nodes" => include_bytes!("../../assets/icons/server-cog.svg").as_slice(),
        "networks" => include_bytes!("../../assets/icons/network.svg").as_slice(),
        "activity" => include_bytes!("../../assets/icons/activity.svg").as_slice(),
        "settings" => include_bytes!("../../assets/icons/settings.svg").as_slice(),
        _ => include_bytes!("../../assets/icons/circle-dot.svg").as_slice(),
    };
    lucide_icon(data, size, color)
}

fn page_icon_data(id: &str) -> &'static [u8] {
    match PageKind::from_id(id) {
        Some(PageKind::Overview) => include_bytes!("../../assets/icons/gauge.svg"),
        Some(PageKind::Networks) => include_bytes!("../../assets/icons/network.svg"),
        Some(PageKind::Peers) => include_bytes!("../../assets/icons/waypoints.svg"),
        Some(PageKind::NodeRuntime) => include_bytes!("../../assets/icons/server-cog.svg"),
        Some(PageKind::Topology) => include_bytes!("../../assets/icons/globe-2.svg"),
        Some(PageKind::Traffic) => include_bytes!("../../assets/icons/activity.svg"),
        Some(PageKind::Activity) => include_bytes!("../../assets/icons/workflow.svg"),
        Some(PageKind::Routing) => include_bytes!("../../assets/icons/database.svg"),
        Some(PageKind::ExitNode) => include_bytes!("../../assets/icons/door-open.svg"),
        Some(PageKind::Relay) => include_bytes!("../../assets/icons/radio-tower.svg"),
        Some(PageKind::Logs) => include_bytes!("../../assets/icons/boxes.svg"),
        Some(PageKind::Identity) => include_bytes!("../../assets/icons/key-round.svg"),
        Some(PageKind::Settings) => include_bytes!("../../assets/icons/cpu.svg"),
        None => include_bytes!("../../assets/icons/circle-dot.svg"),
    }
}

actions!(fuckipv6_shell, [RefreshSnapshot, FocusSearch, Quit]);

const A11Y_ROOT_ROLE: Role = Role::Application;
const A11Y_NAVIGATION_ROLE: Role = Role::Navigation;
const A11Y_MAIN_ROLE: Role = Role::Main;
const A11Y_HEADING_ROLE: Role = Role::Heading;
const A11Y_TAB_LIST_ROLE: Role = Role::TabList;
const A11Y_TAB_PANEL_ROLE: Role = Role::TabPanel;

/// The native shell consumes the same ordered region contract as the WebUI.
/// These identifiers are also used as stable GPUI element IDs for tests,
/// keyboard traversal, and future accessibility inspection.
const APP_REGION_ORDER: [AppRegion; 4] = AppRegion::ALL;

fn app_region_id(region: AppRegion) -> SharedString {
    SharedString::from(format!("gpui-region-{}", region.name()))
}

struct NetworkProfileResultSections<'a> {
    status: gpui::AnyElement,
    details: Div,
    warnings: &'a [fuckipv6_common::domain::NetworkProfileWarning],
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TopologyDirectoryRequest {
    request_id: u64,
    network_id: NetworkId,
    boundary: fuckipv6_common::domain::TopologyBoundary,
    topology_revision: u64,
    generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TopologyDirectoryReady {
    directory: NetworkMemberDirectory,
    network_id: NetworkId,
    boundary: fuckipv6_common::domain::TopologyBoundary,
    topology_revision: u64,
    generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TopologyDirectoryError {
    message: String,
    network_id: NetworkId,
    boundary: fuckipv6_common::domain::TopologyBoundary,
    topology_revision: u64,
    generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingTopologyLifecycle {
    network_id: NetworkId,
    node_id: String,
    action: MemberLifecycleAction,
}

fn visible_overview_primary_page<'a>(
    overview: &'a OverviewModel,
    manifest: Option<&NodeUiManifest>,
) -> Option<&'a str> {
    let page_id = overview.primary_page_id.as_deref()?;
    manifest?
        .pages
        .iter()
        .any(|page| page.id == page_id)
        .then_some(page_id)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ActivityTimeWindow {
    #[default]
    All,
    LastHour,
    Last24Hours,
    Last7Days,
}

impl ActivityTimeWindow {
    const fn duration_ms(self) -> Option<u64> {
        match self {
            Self::All => None,
            Self::LastHour => Some(60 * 60 * 1_000),
            Self::Last24Hours => Some(24 * 60 * 60 * 1_000),
            Self::Last7Days => Some(7 * 24 * 60 * 60 * 1_000),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionBlockDetail {
    Contract(ActionBlockReason),
    CoordinatorGovernance(CoordinatorGovernanceFreezeReason),
}

impl ActionBlockDetail {
    const fn message_key(self) -> &'static str {
        match self {
            Self::Contract(reason) => reason.message_key(),
            Self::CoordinatorGovernance(reason) => reason.message_key(),
        }
    }
}

fn complete_action_block_reason(
    action: &NodeUiAction,
    snapshot: &NodeSnapshot,
    selected_network: Option<&str>,
) -> Option<ActionBlockDetail> {
    action_block_reason(action, snapshot, selected_network)
        .map(ActionBlockDetail::Contract)
        .or_else(|| {
            action_coordinator_governance_freeze_reason(action, snapshot, selected_network)
                .map(ActionBlockDetail::CoordinatorGovernance)
        })
}

struct GpuiShell {
    commands: NodeCommandBus,
    operations: Option<RemoteOperationClient>,
    configs: ConfigManager,
    language_preference: LanguagePreference,
    locale: Locale,
    snapshot: NodeSnapshot,
    manifest: Option<NodeUiManifest>,
    selected_page: String,
    selected_network: Option<String>,
    pending_confirmation: Option<String>,
    expanded_actions: HashMap<String, bool>,
    form_values: HashMap<String, String>,
    text_inputs: HashMap<String, Entity<NativeTextInput>>,
    search_query: String,
    search_input: Option<Entity<NativeTextInput>>,
    action_panel_query: String,
    action_panel_search_input: Option<Entity<NativeTextInput>>,
    collapsed_action_groups: HashSet<String>,
    discovery_search_input: Option<Entity<NativeTextInput>>,
    discovery_query: String,
    discovery_debounced_query: String,
    discovery_search_generation: u64,
    routing_target_query: String,
    routing_target_search_input: Option<Entity<NativeTextInput>>,
    discovery_filters: DiscoveryFilters,
    activity_filters: ActivityFilters,
    activity_time_window: ActivityTimeWindow,
    expanded_discovery: HashMap<String, bool>,
    configuration: GpuiConfigurationWorkflow,
    configuration_editor: Option<Entity<TextareaState>>,
    configuration_editor_placeholder: Option<String>,
    configuration_initial_load_requested: bool,
    focus_handle: FocusHandle,
    content_focus_handle: FocusHandle,
    pending_commands: u32,
    next_request_id: u64,
    operation: Option<NodeOperationSnapshot>,
    operation_fingerprint: Option<String>,
    operation_idempotency_key: Option<IdempotencyKey>,
    last_operation_command: Option<NodeCommand>,
    operation_cancel_pending: bool,
    operation_cancel_error: Option<String>,
    recent_diagnostics: Option<NodeDiagnosticsSnapshot>,
    diagnostic_copy_notice: Option<String>,
    recent_network_profile_preview: Option<NetworkProfilePreview>,
    recent_network_profile_updated: Option<NetworkProfileApplyResult>,
    recent_network_join_preview: Option<NodeNetworkJoinPreview>,
    recent_join_requests: Option<NodeJoinRequestListResult>,
    recent_network_members: Option<NetworkMemberDirectory>,
    selected_member_node_id: Option<String>,
    member_directory_filters: MemberDirectoryFilters,
    join_request_selection: BTreeSet<String>,
    network_join_terms_confirmed: bool,
    network_join_exit_contribution_confirmed: bool,
    recent_issued_invitation: Option<IssuedInvitation>,
    recent_invitations: Option<Vec<InvitationStatusView>>,
    recent_invitation_status: Option<InvitationStatusView>,
    recent_invitation_preview: Option<InvitationPreview>,
    invitation_terms_confirmed: bool,
    invitation_exit_contribution_confirmed: bool,
    invitation_copy_notice: Option<String>,
    error: Option<String>,
    notice: Option<String>,
    pending_notice_after_snapshot: Option<String>,
    language_menu_open: bool,
    network_menu_open: bool,
    network_actions_open: Option<String>,
    selected_topology_network_id: Option<NetworkId>,
    selected_topology_node_id: Option<String>,
    hovered_topology_node_id: Option<String>,
    topology_menu_node_id: Option<String>,
    topology_selection: BTreeSet<String>,
    topology_directory_request: Option<TopologyDirectoryRequest>,
    topology_directory_ready: Option<TopologyDirectoryReady>,
    topology_directory_error: Option<TopologyDirectoryError>,
    topology_directory_generation: u64,
    pending_topology_lifecycle: Option<PendingTopologyLifecycle>,
}

impl GpuiShell {
    fn new(
        state: NodeStateStore,
        commands: NodeCommandBus,
        operations: Option<RemoteOperationClient>,
        configs: ConfigManager,
        cx: &mut Context<Self>,
    ) -> Self {
        let updates = state.subscribe();
        let snapshot = updates.borrow().clone();
        let language_preference = configs.node_language_preference().unwrap_or_default();
        let locale = resolve_language(language_preference);
        let selected_network = snapshot
            .networks
            .first()
            .map(|network| network.name.clone());
        let mut shell = Self {
            commands,
            operations,
            configs,
            language_preference,
            locale,
            snapshot,
            manifest: None,
            selected_page: "overview".into(),
            selected_network,
            pending_confirmation: None,
            expanded_actions: HashMap::new(),
            form_values: HashMap::new(),
            text_inputs: HashMap::new(),
            search_query: String::new(),
            search_input: None,
            action_panel_query: String::new(),
            action_panel_search_input: None,
            collapsed_action_groups: HashSet::new(),
            discovery_search_input: None,
            discovery_query: String::new(),
            discovery_debounced_query: String::new(),
            discovery_search_generation: 0,
            routing_target_query: String::new(),
            routing_target_search_input: None,
            discovery_filters: DiscoveryFilters::default(),
            activity_filters: ActivityFilters::default(),
            activity_time_window: ActivityTimeWindow::default(),
            expanded_discovery: HashMap::new(),
            configuration: GpuiConfigurationWorkflow::default(),
            configuration_editor: None,
            configuration_editor_placeholder: None,
            configuration_initial_load_requested: false,
            focus_handle: cx.focus_handle(),
            content_focus_handle: cx.focus_handle(),
            pending_commands: 0,
            next_request_id: 1,
            operation: None,
            operation_fingerprint: None,
            operation_idempotency_key: None,
            last_operation_command: None,
            operation_cancel_pending: false,
            operation_cancel_error: None,
            recent_diagnostics: None,
            diagnostic_copy_notice: None,
            recent_network_profile_preview: None,
            recent_network_profile_updated: None,
            recent_network_join_preview: None,
            recent_join_requests: None,
            recent_network_members: None,
            selected_member_node_id: None,
            member_directory_filters: MemberDirectoryFilters::default(),
            join_request_selection: BTreeSet::new(),
            network_join_terms_confirmed: false,
            network_join_exit_contribution_confirmed: false,
            recent_issued_invitation: None,
            recent_invitations: None,
            recent_invitation_status: None,
            recent_invitation_preview: None,
            invitation_terms_confirmed: false,
            invitation_exit_contribution_confirmed: false,
            invitation_copy_notice: None,
            error: None,
            notice: None,
            pending_notice_after_snapshot: None,
            language_menu_open: false,
            network_menu_open: false,
            network_actions_open: None,
            selected_topology_network_id: None,
            selected_topology_node_id: None,
            hovered_topology_node_id: None,
            topology_menu_node_id: None,
            topology_selection: BTreeSet::new(),
            topology_directory_request: None,
            topology_directory_ready: None,
            topology_directory_error: None,
            topology_directory_generation: 0,
            pending_topology_lifecycle: None,
        };

        let search_input = cx.new(|cx| {
            NativeTextInput::new(
                "global-search-input",
                t(locale, "common.search"),
                t(locale, "common.search_placeholder"),
                "",
                cx,
            )
            .compact()
        });
        cx.subscribe(
            &search_input,
            |shell: &mut Self, _, event: &TextInputEvent, cx| {
                shell.search_query = event.value.clone();
                cx.notify();
            },
        )
        .detach();
        shell.search_input = Some(search_input);

        let action_panel_search_input = cx.new(|cx| {
            NativeTextInput::new(
                "action-panel-search-input",
                t(locale, "common.actions_search_placeholder"),
                t(locale, "common.actions_search_placeholder"),
                "",
                cx,
            )
            .compact()
        });
        cx.subscribe(
            &action_panel_search_input,
            |shell: &mut Self, _, event: &TextInputEvent, cx| {
                shell.action_panel_query = event.value.clone();
                cx.notify();
            },
        )
        .detach();
        shell.action_panel_search_input = Some(action_panel_search_input);

        let discovery_search_input = cx.new(|cx| {
            NativeTextInput::new(
                "discovery-search-input",
                t(locale, "discovery.search.label"),
                t(locale, "discovery.search.placeholder"),
                "",
                cx,
            )
            .compact()
        });
        cx.subscribe(
            &discovery_search_input,
            |shell: &mut Self, _, event: &TextInputEvent, cx| {
                shell.set_discovery_query(event.value.clone(), cx);
            },
        )
        .detach();
        shell.discovery_search_input = Some(discovery_search_input);

        let routing_target_search_input = cx.new(|cx| {
            NativeTextInput::new(
                "routing-target-search-input",
                t(locale, "traffic.routing.targets.search"),
                t(locale, "traffic.routing.targets.search_placeholder"),
                "",
                cx,
            )
            .compact()
        });
        cx.subscribe(
            &routing_target_search_input,
            |shell: &mut Self, _, event: &TextInputEvent, cx| {
                shell.routing_target_query = event.value.clone();
                cx.notify();
            },
        )
        .detach();
        shell.routing_target_search_input = Some(routing_target_search_input);

        shell.watch_state(updates, cx);
        shell.submit_raw(NodeCommand::GetUiManifest, cx);
        shell.submit_raw(NodeCommand::GetSnapshot, cx);
        shell
    }

    fn watch_state(
        &mut self,
        mut updates: tokio::sync::watch::Receiver<NodeSnapshot>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |weak, cx| loop {
            if updates.changed().await.is_err() {
                break;
            }
            let snapshot = updates.borrow_and_update().clone();
            if weak
                .update(cx, |shell, cx| {
                    shell.apply_snapshot(snapshot, cx);
                    cx.notify();
                })
                .is_err()
            {
                break;
            }
        })
        .detach();
    }

    fn set_language(&mut self, preference: LanguagePreference, cx: &mut Context<Self>) {
        self.language_preference = preference;
        self.locale = preference.resolve(detect_system_locale());
        set_shell_menus(cx, self.locale);
        if let Some(input) = &self.search_input {
            input.update(cx, |input, cx| {
                input.set_accessibility_copy(
                    t(self.locale, "common.search"),
                    t(self.locale, "common.search_placeholder"),
                    cx,
                );
            });
        }
        if let Some(input) = &self.discovery_search_input {
            input.update(cx, |input, cx| {
                input.set_accessibility_copy(
                    t(self.locale, "discovery.search.label"),
                    t(self.locale, "discovery.search.placeholder"),
                    cx,
                );
            });
        }
        if let Some(input) = &self.action_panel_search_input {
            input.update(cx, |input, cx| {
                input.set_accessibility_copy(
                    t(self.locale, "common.actions_search_placeholder"),
                    t(self.locale, "common.actions_search_placeholder"),
                    cx,
                );
            });
        }
        if let Some(input) = &self.routing_target_search_input {
            input.update(cx, |input, cx| {
                input.set_accessibility_copy(
                    t(self.locale, "traffic.routing.targets.search"),
                    t(self.locale, "traffic.routing.targets.search_placeholder"),
                    cx,
                );
            });
        }
        if let Err(error) = self.configs.set_node_language_preference(preference) {
            self.error = Some(tr(
                self.locale,
                "gpui.operation_failed",
                &[("error", &error.to_string())],
            ));
        }
        cx.notify();
    }

    fn topology_identity_changed(
        previous: Option<&TopologySnapshot>,
        next: Option<&TopologySnapshot>,
    ) -> bool {
        let identity = |topology: Option<&TopologySnapshot>| {
            topology.map(|topology| {
                (
                    topology.revision,
                    topology.viewer.as_str().to_string(),
                    topology
                        .boundaries
                        .iter()
                        .map(|boundary| boundary.network_id.as_str().to_string())
                        .collect::<Vec<_>>(),
                    topology
                        .nodes
                        .iter()
                        .map(|node| node.node_id.as_str().to_string())
                        .collect::<Vec<_>>(),
                )
            })
        };
        identity(previous) != identity(next)
    }

    fn topology_network_id(&self) -> Option<NetworkId> {
        let topology = self.snapshot.topology.as_ref()?;
        self.selected_topology_network_id
            .as_ref()
            .filter(|network_id| topology.boundary(network_id).is_some())
            .cloned()
            .or_else(|| {
                topology
                    .boundaries
                    .iter()
                    .find(|boundary| {
                        self.selected_network
                            .as_deref()
                            .is_some_and(|selected| boundary.name == selected)
                    })
                    .or_else(|| topology.boundaries.first())
                    .map(|boundary| boundary.network_id.clone())
            })
    }

    fn invalidate_topology_directory(&mut self) {
        // Keep the last authoritative directory so the UI can explain that it
        // is stale. Matching helpers reject it for reads and writes until the
        // topology revision is observed again.
        self.topology_directory_request = None;
        self.topology_directory_error = None;
    }

    fn topology_directory_for_network(
        &self,
        network_id: &NetworkId,
    ) -> Option<&NetworkMemberDirectory> {
        let topology = self.snapshot.topology.as_ref()?;
        let boundary = topology.boundary(network_id)?;
        let ready = self.topology_directory_ready.as_ref()?;
        (ready.generation == self.topology_directory_generation
            && &ready.network_id == network_id
            && ready.topology_revision == topology.revision
            && &ready.boundary == boundary)
            .then_some(&ready.directory)
    }

    fn topology_directory_error_for_network(&self, network_id: &NetworkId) -> Option<&str> {
        let topology = self.snapshot.topology.as_ref()?;
        let error = self.topology_directory_error.as_ref()?;
        (error.generation == self.topology_directory_generation
            && &error.network_id == network_id
            && error.topology_revision == topology.revision)
            .then_some(error.message.as_str())
    }

    fn topology_directory_is_stale_for_network(&self, network_id: &NetworkId) -> bool {
        let Some(topology) = self.snapshot.topology.as_ref() else {
            return false;
        };
        let Some(boundary) = topology.boundary(network_id) else {
            return false;
        };
        self.topology_directory_ready.as_ref().is_some_and(|ready| {
            ready.generation == self.topology_directory_generation
                && &ready.network_id == network_id
                && (ready.topology_revision != topology.revision || &ready.boundary != boundary)
        })
    }

    fn request_topology_directory(
        &mut self,
        network_id: NetworkId,
        topology_revision: u64,
        cx: &mut Context<Self>,
    ) {
        if !self
            .snapshot
            .permissions
            .permissions
            .contains(&NodePermission::ViewState)
        {
            return;
        }
        let Some(topology) = self.snapshot.topology.as_ref() else {
            return;
        };
        if topology.revision != topology_revision || topology.boundary(&network_id).is_none() {
            return;
        }
        if self
            .topology_directory_request
            .as_ref()
            .is_some_and(|request| {
                request.generation == self.topology_directory_generation
                    && request.network_id == network_id
                    && request.topology_revision == topology_revision
            })
            || self.topology_directory_for_network(&network_id).is_some()
            || self
                .topology_directory_error_for_network(&network_id)
                .is_some()
        {
            return;
        }

        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1).max(1);
        let boundary = topology
            .boundary(&network_id)
            .expect("validated topology boundary")
            .clone();
        let request = TopologyDirectoryRequest {
            request_id,
            network_id: network_id.clone(),
            boundary,
            topology_revision,
            generation: self.topology_directory_generation,
        };
        let command = NodeCommand::ListNetworkMembers(crate::node::NodeNetworkMembersCommand {
            network_id: network_id.to_string(),
        });
        let command_request = NodeCommandRequest::new(request_id, command);
        let context = NodeCommandContext::local_owner_for_snapshot(&self.snapshot, "gpui");
        let commands = self.commands.clone();
        self.topology_directory_request = Some(request.clone());
        self.pending_commands = self.pending_commands.saturating_add(1);
        cx.notify();

        cx.spawn(async move |weak, cx| {
            let result = commands.execute(context, command_request).await;
            let _ = weak.update(cx, |shell, cx| {
                shell.pending_commands = shell.pending_commands.saturating_sub(1);
                shell.apply_topology_directory_result(&request, result);
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_topology_directory_result(
        &mut self,
        request: &TopologyDirectoryRequest,
        result: Result<NodeCommandResult, NodeCommandBusError>,
    ) {
        let current = self
            .topology_directory_request
            .as_ref()
            .is_some_and(|current| current == request);
        if !current {
            return;
        }
        self.topology_directory_request = None;

        let topology_revision_matches = self.snapshot.topology.as_ref().is_some_and(|topology| {
            topology.revision == request.topology_revision
                && topology.boundary(&request.network_id) == Some(&request.boundary)
        });
        if !topology_revision_matches || request.generation != self.topology_directory_generation {
            return;
        }

        let message = match result {
            Ok(result) if result.request_id == request.request_id => match result.outcome {
                NodeCommandOutcome::NetworkMembers(directory)
                    if directory.network_id == request.network_id =>
                {
                    self.topology_directory_ready = Some(TopologyDirectoryReady {
                        directory: *directory,
                        network_id: request.network_id.clone(),
                        boundary: request.boundary.clone(),
                        topology_revision: request.topology_revision,
                        generation: request.generation,
                    });
                    self.topology_directory_error = None;
                    return;
                }
                NodeCommandOutcome::NetworkMembers(directory) => tr(
                    self.locale,
                    "gpui.command_channel_error",
                    &[(
                        "error",
                        &format!(
                            "member directory network {} does not match requested {}",
                            directory.network_id, request.network_id
                        ),
                    )],
                ),
                NodeCommandOutcome::Rejected(problem) => {
                    format_operation_problem(self.locale, &problem)
                }
                _ => t(self.locale, "topology.batch.directory_failed").to_string(),
            },
            Ok(_) => return,
            Err(error) => tr(
                self.locale,
                "gpui.command_channel_error",
                &[("error", &error.to_string())],
            ),
        };
        self.topology_directory_error = Some(TopologyDirectoryError {
            message,
            network_id: request.network_id.clone(),
            boundary: request.boundary.clone(),
            topology_revision: request.topology_revision,
            generation: request.generation,
        });
    }

    fn select_topology_network(&mut self, network_id: NetworkId, cx: &mut Context<Self>) {
        if self
            .snapshot
            .topology
            .as_ref()
            .and_then(|topology| topology.boundary(&network_id))
            .is_none()
        {
            return;
        }
        if self.selected_topology_network_id.as_ref() == Some(&network_id) {
            return;
        }
        self.selected_topology_network_id = Some(network_id.clone());
        self.selected_network = self
            .snapshot
            .topology
            .as_ref()
            .and_then(|topology| topology.boundary(&network_id))
            .map(|boundary| boundary.name.clone());
        self.selected_topology_node_id = None;
        self.hovered_topology_node_id = None;
        self.topology_menu_node_id = None;
        self.topology_selection.clear();
        self.pending_topology_lifecycle = None;
        self.invalidate_topology_directory();
        cx.notify();
    }

    fn open_topology_node(&mut self, node_id: String, cx: &mut Context<Self>) {
        let network_id = self.topology_network_id();
        let exists = network_id.as_ref().is_some_and(|network_id| {
            self.snapshot
                .topology
                .as_ref()
                .and_then(|topology| {
                    topology
                        .nodes
                        .iter()
                        .find(|node| node.node_id.as_str() == node_id)
                })
                .is_some_and(|node| node.belongs_to(network_id))
        });
        if !exists {
            return;
        }
        self.selected_topology_node_id = Some(node_id);
        self.topology_menu_node_id = None;
        cx.notify();
    }

    fn toggle_topology_selection(&mut self, node_id: String, cx: &mut Context<Self>) {
        let Some(network_id) = self.topology_network_id() else {
            return;
        };
        let exists = self
            .snapshot
            .topology
            .as_ref()
            .and_then(|topology| {
                topology
                    .nodes
                    .iter()
                    .find(|node| node.node_id.as_str() == node_id)
            })
            .is_some_and(|node| node.belongs_to(&network_id));
        if !exists {
            return;
        }
        if !self.topology_selection.remove(&node_id) {
            self.topology_selection.insert(node_id.clone());
        }
        self.selected_topology_node_id = Some(node_id);
        self.pending_topology_lifecycle = None;
        cx.notify();
    }

    fn toggle_topology_menu(&mut self, node_id: String, cx: &mut Context<Self>) {
        let Some(network_id) = self.topology_network_id() else {
            return;
        };
        let exists = self
            .snapshot
            .topology
            .as_ref()
            .and_then(|topology| {
                topology
                    .nodes
                    .iter()
                    .find(|node| node.node_id.as_str() == node_id)
            })
            .is_some_and(|node| node.belongs_to(&network_id));
        if !exists {
            return;
        }
        self.selected_topology_node_id = Some(node_id.clone());
        self.hovered_topology_node_id = None;
        if self.topology_menu_node_id.as_deref() == Some(node_id.as_str()) {
            self.topology_menu_node_id = None;
        } else {
            self.topology_menu_node_id = Some(node_id);
        }
        cx.notify();
    }

    fn prepare_topology_lifecycle(&mut self, node_id: &str, action: MemberLifecycleAction) -> bool {
        let Some(network_id) = self.topology_network_id() else {
            return false;
        };
        let Some(topology) = self.snapshot.topology.as_ref() else {
            return false;
        };
        if topology.boundary(&network_id).is_none() {
            return false;
        }
        let Some(node) = topology
            .nodes
            .iter()
            .find(|node| node.node_id.as_str() == node_id)
        else {
            return false;
        };
        if !node.belongs_to(&network_id) {
            return false;
        }
        let Some((role, expected_revision)) = ({
            let Some(directory) = self.topology_directory_for_network(&network_id) else {
                return false;
            };
            if !topology_member_actions(Some(directory), Some(network_id.as_str()), node_id)
                .contains(&action)
            {
                return false;
            }
            directory
                .members
                .iter()
                .find(|member| member.node_id.as_str() == node_id)
                .map(|member| (member.role, member.revision))
        }) else {
            return false;
        };

        self.set_field_value(
            "change_member_lifecycle",
            "network_id",
            network_id.to_string(),
        );
        self.set_field_value("change_member_lifecycle", "target_node_id", node_id);
        self.set_field_value(
            "change_member_lifecycle",
            "action",
            member_lifecycle_action_value(action),
        );
        self.set_field_value(
            "change_member_lifecycle",
            "role",
            membership_role_value(role),
        );
        self.set_field_value(
            "change_member_lifecycle",
            "expected_revision",
            expected_revision.to_string(),
        );
        self.set_field_value("change_member_lifecycle", "reason", "");
        true
    }

    fn request_topology_lifecycle(
        &mut self,
        node_id: String,
        action: MemberLifecycleAction,
        cx: &mut Context<Self>,
    ) {
        if let Some(reason) = command_coordinator_governance_freeze_reason(
            NodeCommandKind::ChangeMemberLifecycle,
            &self.snapshot,
            self.selected_network.as_deref(),
        ) {
            self.pending_topology_lifecycle = None;
            self.error = Some(tr(
                self.locale,
                "gpui.action_blocked",
                &[
                    ("action", t(self.locale, "action.change_member_lifecycle")),
                    ("reason", t(self.locale, reason.message_key())),
                ],
            ));
            cx.notify();
            return;
        }
        let network_id = self.topology_network_id();
        if network_id.is_none() || !self.prepare_topology_lifecycle(&node_id, action) {
            self.pending_topology_lifecycle = None;
            self.error = Some(t(self.locale, "topology.batch.directory_stale").to_string());
            cx.notify();
            return;
        }
        let confirmed = self.pending_confirmation.as_deref() == Some("change_member_lifecycle");
        self.pending_topology_lifecycle = Some(PendingTopologyLifecycle {
            network_id: network_id.expect("checked topology network"),
            node_id,
            action,
        });
        if confirmed {
            self.pending_topology_lifecycle = None;
        }
        self.submit_manifest_action("change_member_lifecycle", cx);
    }

    fn confirm_topology_lifecycle(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_topology_lifecycle.clone() else {
            return;
        };
        if self.pending_confirmation.as_deref() != Some("change_member_lifecycle") {
            return;
        }
        if self.topology_network_id().as_ref() != Some(&pending.network_id) {
            self.pending_topology_lifecycle = None;
            self.pending_confirmation = None;
            self.error = Some(t(self.locale, "topology.batch.directory_stale").to_string());
            cx.notify();
            return;
        }
        self.request_topology_lifecycle(pending.node_id, pending.action, cx);
    }

    fn apply_snapshot(&mut self, snapshot: NodeSnapshot, cx: &mut Context<Self>) {
        let topology_changed = Self::topology_identity_changed(
            self.snapshot.topology.as_ref(),
            snapshot.topology.as_ref(),
        );
        self.snapshot = snapshot;
        let selection_still_exists = self.selected_network.as_deref().is_some_and(|selected| {
            self.snapshot
                .networks
                .iter()
                .any(|network| network.name == selected)
        });
        if !selection_still_exists {
            self.selected_network = self
                .snapshot
                .networks
                .iter()
                .find(|network| network.status == NodeNetworkStatus::Online)
                .or_else(|| self.snapshot.networks.first())
                .map(|network| network.name.clone());
        }

        let previous_topology_network = self.selected_topology_network_id.clone();
        self.selected_topology_network_id = self.topology_network_id();
        let topology_network_changed =
            previous_topology_network != self.selected_topology_network_id;
        if topology_changed {
            self.invalidate_topology_directory();
        }
        if topology_network_changed {
            self.selected_topology_node_id = None;
            self.hovered_topology_node_id = None;
            self.topology_menu_node_id = None;
            self.topology_selection.clear();
            self.pending_topology_lifecycle = None;
            if self.pending_confirmation.as_deref() == Some("change_member_lifecycle") {
                self.pending_confirmation = None;
            }
        }
        if let Some(network_id) = self.selected_topology_network_id.clone() {
            if let Some(boundary) = self
                .snapshot
                .topology
                .as_ref()
                .and_then(|topology| topology.boundary(&network_id))
            {
                self.selected_network = Some(boundary.name.clone());
            }
            let valid_nodes = self
                .snapshot
                .topology
                .as_ref()
                .map(|topology| {
                    topology
                        .nodes
                        .iter()
                        .filter(|node| node.belongs_to(&network_id))
                        .map(|node| node.node_id.as_str().to_string())
                        .collect::<BTreeSet<_>>()
                })
                .unwrap_or_default();
            if self
                .selected_topology_node_id
                .as_ref()
                .is_some_and(|node_id| !valid_nodes.contains(node_id))
            {
                self.selected_topology_node_id = None;
            }
            if self
                .hovered_topology_node_id
                .as_ref()
                .is_some_and(|node_id| !valid_nodes.contains(node_id))
            {
                self.hovered_topology_node_id = None;
            }
            if self
                .topology_menu_node_id
                .as_ref()
                .is_some_and(|node_id| !valid_nodes.contains(node_id))
            {
                self.topology_menu_node_id = None;
            }
            if self
                .pending_topology_lifecycle
                .as_ref()
                .is_some_and(|pending| !valid_nodes.contains(&pending.node_id))
            {
                self.pending_topology_lifecycle = None;
                if self.pending_confirmation.as_deref() == Some("change_member_lifecycle") {
                    self.pending_confirmation = None;
                }
            }
            self.topology_selection
                .retain(|node_id| valid_nodes.contains(node_id));
        } else {
            self.selected_topology_node_id = None;
            self.hovered_topology_node_id = None;
            self.topology_menu_node_id = None;
            self.topology_selection.clear();
            self.pending_topology_lifecycle = None;
            if self.pending_confirmation.as_deref() == Some("change_member_lifecycle") {
                self.pending_confirmation = None;
            }
        }
        self.fill_empty_network_fields();
        self.sync_text_inputs(cx);
    }

    fn submit_raw(&mut self, command: NodeCommand, cx: &mut Context<Self>) {
        if command.kind().is_operation() {
            self.submit_operation(command, cx);
            return;
        }
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1).max(1);
        let request = NodeCommandRequest::new(request_id, command);
        let context = NodeCommandContext::local_owner_for_snapshot(&self.snapshot, "gpui");
        let commands = self.commands.clone();
        self.pending_commands = self.pending_commands.saturating_add(1);
        self.error = None;
        self.notice = None;
        self.pending_confirmation = None;
        cx.notify();

        cx.spawn(async move |weak, cx| {
            let result = commands.execute(context, request).await;
            let _ = weak.update(cx, |shell, cx| {
                shell.pending_commands = shell.pending_commands.saturating_sub(1);
                shell.apply_command_result(result, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn submit_operation(&mut self, command: NodeCommand, cx: &mut Context<Self>) {
        let Some(operations) = self.operations.clone() else {
            self.error = Some(t(self.locale, "gpui.operations_unavailable").to_string());
            cx.notify();
            return;
        };
        self.last_operation_command = Some(command.clone());
        let fingerprint =
            serde_json::to_string(&command).unwrap_or_else(|_| command.kind().as_str().to_owned());
        if self.operation_fingerprint.as_deref() != Some(fingerprint.as_str()) {
            self.operation = None;
            self.operation_idempotency_key = None;
            self.operation_fingerprint = Some(fingerprint);
        }
        self.operation_cancel_pending = false;
        self.operation_cancel_error = None;
        let idempotency_key = self
            .operation_idempotency_key
            .clone()
            .unwrap_or_else(new_idempotency_key);
        self.operation_idempotency_key = Some(idempotency_key.clone());

        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1).max(1);
        let request = NodeCommandRequest::new(request_id, command);
        let context = NodeCommandContext::local_owner_for_snapshot(&self.snapshot, "gpui");
        self.pending_commands = self.pending_commands.saturating_add(1);
        self.error = None;
        self.notice = None;
        self.pending_confirmation = None;
        cx.notify();

        cx.spawn(async move |weak, cx| {
            let mut submitted = match operations.submit(context, request, idempotency_key).await {
                Ok(operation) => operation,
                Err(error) => {
                    let _ = weak.update(cx, |shell, cx| {
                        shell.pending_commands = shell.pending_commands.saturating_sub(1);
                        shell.fail_operation_client(error);
                        cx.notify();
                    });
                    return;
                }
            };

            let deadline = std::time::Instant::now() + OPERATION_LOOKUP_DEADLINE;
            let mut lookup_failures = 0_u16;
            loop {
                let terminal = operation_state_is_terminal(submitted.operation.state);
                let operation_id = submitted.operation.operation_id.as_str().to_owned();
                let snapshot = submitted.clone();
                let _ = weak.update(cx, |shell, cx| {
                    shell.operation = Some(snapshot.clone());
                    if terminal {
                        shell.finish_operation(snapshot, cx);
                    }
                    cx.notify();
                });
                if terminal {
                    break;
                }

                if std::time::Instant::now() >= deadline {
                    let _ = weak.update(cx, |shell, cx| {
                        shell.pending_commands = shell.pending_commands.saturating_sub(1);
                        shell.error =
                            Some(t(shell.locale, "gpui.operation_lookup_timeout").to_string());
                        if shell.configuration.operation.is_some() {
                            shell.configuration.set_error(
                                t(shell.locale, "gpui.operation_lookup_timeout").to_string(),
                            );
                            shell.configuration.finish();
                        }
                        cx.notify();
                    });
                    break;
                }

                cx.background_executor()
                    .timer(std::time::Duration::from_millis(400))
                    .await;
                match operations.lookup(&operation_id).await {
                    Ok(Some(operation)) => {
                        lookup_failures = 0;
                        submitted = operation;
                    }
                    Ok(None) => {
                        lookup_failures = lookup_failures.saturating_add(1);
                    }
                    Err(error) => {
                        lookup_failures = lookup_failures.saturating_add(1);
                        let _ = weak.update(cx, |shell, cx| {
                            shell.error = Some(tr(
                                shell.locale,
                                "gpui.operation_lookup_failed",
                                &[("error", &error.to_string())],
                            ));
                            cx.notify();
                        });
                    }
                }
                if lookup_failures >= OPERATION_LOOKUP_FAILURE_LIMIT {
                    let _ = weak.update(cx, |shell, cx| {
                        shell.pending_commands = shell.pending_commands.saturating_sub(1);
                        shell.error =
                            Some(t(shell.locale, "gpui.operation_lookup_timeout").to_string());
                        if shell.configuration.operation.is_some() {
                            shell.configuration.set_error(
                                t(shell.locale, "gpui.operation_lookup_timeout").to_string(),
                            );
                            shell.configuration.finish();
                        }
                        cx.notify();
                    });
                    break;
                }
            }
        })
        .detach();
    }

    fn fail_operation_client(&mut self, error: OperationClientError) {
        let message = match error {
            OperationClientError::Invalid(problem) => {
                format_operation_problem(self.locale, &problem)
            }
            error => error.to_string(),
        };
        if self.configuration.operation.is_some() {
            self.configuration.set_error(message.clone());
            self.configuration.finish();
        }
        self.error = Some(tr(
            self.locale,
            "gpui.operation_failed",
            &[("error", &message)],
        ));
    }

    fn cancel_operation(&mut self, cx: &mut Context<Self>) {
        let Some(operation) = self.operation.as_ref() else {
            return;
        };
        if self.operation_cancel_pending
            || !operation.operation.cancellable
            || operation_state_is_terminal(operation.operation.state)
        {
            return;
        }
        let Some(operations) = self.operations.clone() else {
            self.operation_cancel_error =
                Some(t(self.locale, "gpui.operations_unavailable").to_string());
            cx.notify();
            return;
        };
        let operation_id = operation.operation.operation_id.as_str().to_owned();
        let context = NodeCommandContext::local_owner_for_snapshot(&self.snapshot, "gpui");
        self.operation_cancel_pending = true;
        self.operation_cancel_error = None;
        cx.notify();

        cx.spawn(async move |weak, cx| {
            let result = operations.cancel(context, &operation_id).await;
            let _ = weak.update(cx, |shell, cx| {
                shell.operation_cancel_pending = false;
                match result {
                    Ok(operation) => shell.operation = Some(operation),
                    Err(OperationClientError::Invalid(problem)) => {
                        shell.operation_cancel_error =
                            Some(format_operation_problem(shell.locale, &problem));
                    }
                    Err(OperationClientError::CancelRejected(problem)) => {
                        shell.operation_cancel_error =
                            Some(format_operation_problem(shell.locale, &problem));
                    }
                    Err(error) => shell.operation_cancel_error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn finish_operation(&mut self, operation: NodeOperationSnapshot, cx: &mut Context<Self>) {
        self.pending_commands = self.pending_commands.saturating_sub(1);
        if operation_releases_idempotency_key(operation.operation.state) {
            self.operation_idempotency_key = None;
            if let Some(result) = operation.result {
                self.apply_command_result(Ok(result), cx);
            } else {
                self.submit_raw(NodeCommand::GetSnapshot, cx);
            }
            return;
        }

        let message = operation
            .operation
            .error
            .as_ref()
            .map(|error| format_operation_error(self.locale, error))
            .unwrap_or_else(|| {
                tr(
                    self.locale,
                    "gpui.operation_finished_without_error",
                    &[(
                        "state",
                        t(self.locale, operation_state_key(operation.operation.state)),
                    )],
                )
            });
        if self.configuration.operation.is_some() {
            self.configuration.set_error(message.clone());
            self.configuration.finish();
        }
        self.error = Some(tr(
            self.locale,
            "gpui.operation_failed",
            &[("error", &message)],
        ));
    }

    fn refresh_snapshot(&mut self, _: &RefreshSnapshot, _: &mut Window, cx: &mut Context<Self>) {
        if self.pending_commands == 0 {
            self.submit_raw(NodeCommand::GetSnapshot, cx);
        }
    }

    fn submit_manifest_action(&mut self, action_id: &str, cx: &mut Context<Self>) {
        if self.pending_commands > 0 {
            self.error = Some(t(self.locale, "gpui.busy").to_string());
            cx.notify();
            return;
        }

        let action = self.manifest.as_ref().and_then(|manifest| {
            manifest
                .pages
                .iter()
                .flat_map(|page| &page.actions)
                .find(|action| action.id == action_id)
                .cloned()
        });
        let Some(action) = action else {
            self.error = Some(tr(
                self.locale,
                "gpui.unknown_manifest_action",
                &[("action", action_id)],
            ));
            cx.notify();
            return;
        };

        let action_label = manifest_action_label(self.locale, &action);
        if action.backend_status == NodeUiBackendStatus::Unavailable {
            self.error = Some(tr(
                self.locale,
                "gpui.action_unavailable",
                &[("action", &action_label)],
            ));
            cx.notify();
            return;
        }

        if let Some(reason) =
            complete_action_block_reason(&action, &self.snapshot, self.selected_network.as_deref())
        {
            self.error = Some(tr(
                self.locale,
                "gpui.action_blocked",
                &[
                    ("action", &action_label),
                    ("reason", t(self.locale, reason.message_key())),
                ],
            ));
            cx.notify();
            return;
        }

        if let Some(confirmation) = &action.confirmation {
            if self.pending_confirmation.as_deref() != Some(action.id.as_str()) {
                self.pending_confirmation = Some(action.id.clone());
                let mut notice = tr(
                    self.locale,
                    "gpui.confirm_action",
                    &[
                        ("title", t(self.locale, &confirmation.title_key)),
                        ("consequence", t(self.locale, &confirmation.consequence_key)),
                    ],
                );
                if confirmation.recent_authentication_required {
                    notice.push(' ');
                    notice.push_str(t(self.locale, "confirm.reauthentication_required"));
                }
                self.notice = Some(notice);
                cx.notify();
                return;
            }
        }

        if action_id == "list_network_members" {
            self.recent_network_members = None;
            self.selected_member_node_id = None;
        }
        let values = action
            .fields
            .iter()
            .filter_map(|field| {
                self.form_values
                    .get(&self.field_key(&action.id, &field.id))
                    .cloned()
                    .map(|value| (field.id.clone(), value))
            })
            .collect();
        match build_command(&action, &values) {
            Ok(command) => self.submit_raw(command, cx),
            Err(error) => {
                self.error = Some(tr(
                    self.locale,
                    "gpui.invalid_command_input",
                    &[("error", &error.to_string())],
                ));
                self.pending_confirmation = None;
                cx.notify();
            }
        }
    }

    fn configuration_action(&self, action_id: &str) -> Option<&NodeUiAction> {
        self.manifest
            .as_ref()?
            .pages
            .iter()
            .flat_map(|page| &page.actions)
            .find(|action| action.id == action_id)
    }

    fn configuration_action_blocked(&self, action_id: &str) -> Option<&'static str> {
        let action = self.configuration_action(action_id)?;
        action_block_reason(action, &self.snapshot, self.selected_network.as_deref())
            .map(|reason| reason.message_key())
    }

    fn configuration_available(&self, action_id: &str) -> bool {
        self.configuration_action(action_id)
            .is_some_and(|action| action.backend_status == NodeUiBackendStatus::Implemented)
            && self.configuration_action_blocked(action_id).is_none()
    }

    fn join_request_action_available(&self, action_id: &str) -> bool {
        let Some(action) = self.manifest.as_ref().and_then(|manifest| {
            manifest
                .pages
                .iter()
                .flat_map(|page| &page.actions)
                .find(|action| action.id == action_id)
        }) else {
            return false;
        };
        self.pending_commands == 0
            && action.backend_status == NodeUiBackendStatus::Implemented
            && complete_action_block_reason(
                action,
                &self.snapshot,
                self.selected_network.as_deref(),
            )
            .is_none()
    }

    fn selected_page_has_configuration_actions(&self) -> bool {
        self.manifest
            .as_ref()
            .and_then(|manifest| {
                manifest
                    .pages
                    .iter()
                    .find(|page| page.id == self.selected_page)
            })
            .is_some_and(|page| {
                page.actions
                    .iter()
                    .any(|action| is_configuration_action(&action.id))
            })
    }

    fn submit_configuration(
        &mut self,
        operation: ConfigurationOperation,
        command: NodeCommand,
        cx: &mut Context<Self>,
    ) {
        if self.pending_commands > 0 {
            self.configuration
                .set_error(t(self.locale, "gpui.busy").to_string());
            cx.notify();
            return;
        }
        self.configuration.begin(operation);
        self.submit_raw(command, cx);
    }

    fn load_configuration(&mut self, cx: &mut Context<Self>) {
        if !self.configuration_available("get_configuration") {
            self.configuration.set_error(
                self.configuration_action_blocked("get_configuration")
                    .map(|key| t(self.locale, key).to_string())
                    .unwrap_or_else(|| t(self.locale, "configuration.loading").to_string()),
            );
            cx.notify();
            return;
        }
        self.submit_configuration(
            ConfigurationOperation::Load,
            NodeCommand::GetConfiguration,
            cx,
        );
    }

    fn load_configuration_history(&mut self, cx: &mut Context<Self>) {
        if !self.configuration_available("get_configuration_history") {
            return;
        }
        self.submit_configuration(
            ConfigurationOperation::History,
            NodeCommand::GetConfigurationHistory,
            cx,
        );
    }

    fn preview_configuration_draft(&mut self, cx: &mut Context<Self>) {
        if !self.configuration_available("preview_configuration") {
            self.configuration.set_error(
                self.configuration_action_blocked("preview_configuration")
                    .map(|key| t(self.locale, key).to_string())
                    .unwrap_or_else(|| t(self.locale, "configuration.apply.blocked").to_string()),
            );
            cx.notify();
            return;
        }
        let Some(revision) = self
            .configuration
            .document
            .as_ref()
            .map(|document| document.revision)
        else {
            self.configuration
                .set_error(t(self.locale, "configuration.loading").to_string());
            cx.notify();
            return;
        };
        let networks = match self.configuration.draft_networks() {
            Ok(networks) => networks,
            Err(error) => {
                self.configuration.set_error(error);
                cx.notify();
                return;
            }
        };
        self.submit_configuration(
            ConfigurationOperation::Preview,
            NodeCommand::PreviewConfiguration(NodePreviewConfigurationCommand {
                expected_revision: revision,
                networks,
            }),
            cx,
        );
    }

    fn apply_configuration_draft(&mut self, cx: &mut Context<Self>) {
        if !self.configuration.can_apply() {
            return;
        }
        if !self.configuration_available("apply_configuration") {
            self.configuration.set_error(
                self.configuration_action_blocked("apply_configuration")
                    .map(|key| t(self.locale, key).to_string())
                    .unwrap_or_else(|| t(self.locale, "configuration.apply.blocked").to_string()),
            );
            cx.notify();
            return;
        }
        let Some(revision) = self
            .configuration
            .document
            .as_ref()
            .map(|document| document.revision)
        else {
            return;
        };
        let networks = match self.configuration.draft_networks() {
            Ok(networks) => networks,
            Err(error) => {
                self.configuration.set_error(error);
                cx.notify();
                return;
            }
        };
        self.submit_configuration(
            ConfigurationOperation::Apply,
            NodeCommand::ApplyConfiguration(NodeApplyConfigurationCommand {
                expected_revision: revision,
                networks,
            }),
            cx,
        );
    }

    fn rollback_configuration_to(&mut self, target_revision: u64, cx: &mut Context<Self>) {
        if !self.configuration_available("rollback_configuration") {
            self.configuration.set_error(
                self.configuration_action_blocked("rollback_configuration")
                    .map(|key| t(self.locale, key).to_string())
                    .unwrap_or_else(|| t(self.locale, "configuration.apply.blocked").to_string()),
            );
            cx.notify();
            return;
        }
        let Some(revision) = self
            .configuration
            .document
            .as_ref()
            .map(|document| document.revision)
        else {
            return;
        };
        self.submit_configuration(
            ConfigurationOperation::Rollback,
            NodeCommand::RollbackConfiguration(NodeRollbackConfigurationCommand {
                expected_revision: revision,
                target_revision,
            }),
            cx,
        );
    }

    fn import_configuration_file(&mut self, cx: &mut Context<Self>) {
        if self.pending_commands > 0 {
            return;
        }
        let task = pick_paths(cx, PathPickerMode::Files { multiple: false });
        cx.spawn(async move |weak, cx| {
            let result = task.await;
            let _ = weak.update(cx, |shell, cx| {
                match result {
                    Ok(Some(paths)) => {
                        let Some(path) = paths.first() else {
                            return;
                        };
                        match std::fs::read_to_string(path) {
                            Ok(source) => {
                                let name = path
                                    .file_name()
                                    .and_then(|name| name.to_str())
                                    .unwrap_or("configuration.json")
                                    .to_string();
                                match shell.configuration.import(source, name) {
                                    Ok(()) => {
                                        shell.configuration.set_notice(
                                            t(shell.locale, "configuration.import").to_string(),
                                        );
                                    }
                                    Err(error) => shell.configuration.set_error(error),
                                }
                            }
                            Err(error) => shell.configuration.set_error(format!(
                                "{}: {error}",
                                t(shell.locale, "configuration.import")
                            )),
                        }
                    }
                    Ok(None) => {}
                    Err(error) => shell.configuration.set_error(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn sync_configuration_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_page_has_configuration_actions() {
            return;
        }
        if self.configuration_editor.is_none() {
            let draft = self.configuration.draft.clone();
            let placeholder = t(self.locale, "configuration.editor.help").to_string();
            let editor = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder(placeholder.clone())
                    .default_value(draft)
            });
            cx.subscribe(
                &editor,
                |shell: &mut Self, editor, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        let draft = editor.read(cx).value().to_string();
                        shell.configuration.edit(draft);
                        cx.notify();
                    }
                },
            )
            .detach();
            self.configuration_editor = Some(editor);
            self.configuration_editor_placeholder = Some(placeholder);
            return;
        }

        let Some(editor) = self.configuration_editor.clone() else {
            return;
        };
        let draft = self.configuration.draft.clone();
        if editor.read(cx).value().as_ref() != draft {
            editor.update(cx, |editor, cx| editor.set_value(draft, window, cx));
        }
        let placeholder = t(self.locale, "configuration.editor.help").to_string();
        if self.configuration_editor_placeholder.as_deref() != Some(placeholder.as_str()) {
            editor.update(cx, |editor, cx| {
                editor.set_placeholder(placeholder.clone(), window, cx)
            });
            self.configuration_editor_placeholder = Some(placeholder);
        }
    }

    fn focus_configuration_issue(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = json_pointer_selection(&self.configuration.draft, &path);
        self.configuration.select_issue(path);
        if let Some(editor) = self.configuration_editor.clone() {
            editor.update(cx, |editor, cx| {
                editor.focus(window, cx);
                editor.set_selected_range(range, cx);
            });
        }
        cx.notify();
    }

    fn apply_command_result(
        &mut self,
        result: Result<NodeCommandResult, NodeCommandBusError>,
        cx: &mut Context<Self>,
    ) {
        self.pending_confirmation = None;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                let message = tr(
                    self.locale,
                    "gpui.command_channel_error",
                    &[("error", &error.to_string())],
                );
                if self.configuration.operation.is_some() {
                    self.configuration.set_error(message.clone());
                    self.configuration.finish();
                }
                self.error = Some(message);
                return;
            }
        };

        match result.outcome {
            NodeCommandOutcome::Snapshot(snapshot) => {
                self.apply_snapshot(*snapshot, cx);
                self.notice = self
                    .pending_notice_after_snapshot
                    .take()
                    .or_else(|| Some(t(self.locale, "gpui.snapshot_refreshed").to_string()));
            }
            NodeCommandOutcome::UiManifest(manifest) => {
                let manifest = *manifest;
                if let Err(violations) = manifest.validate() {
                    self.manifest = None;
                    self.error = Some(tr(
                        self.locale,
                        "gpui.manifest_invalid",
                        &[("details", &format!("{violations:?}"))],
                    ));
                    return;
                }
                if let Err(error) = validate_manifest_bindings(&manifest) {
                    self.manifest = None;
                    self.error = Some(tr(
                        self.locale,
                        "gpui.action_mapping_invalid",
                        &[("error", &error.to_string())],
                    ));
                    return;
                }
                if !manifest
                    .pages
                    .iter()
                    .any(|page| page.id == self.selected_page)
                {
                    if let Some(page) = manifest.pages.first() {
                        self.selected_page = page.id.clone();
                    }
                }
                self.manifest = Some(manifest);
                self.initialize_forms(cx);
                self.notice = Some(tr(
                    self.locale,
                    "gpui.manifest_loaded",
                    &[("count", &ActionBinding::ALL.len().to_string())],
                ));
            }
            NodeCommandOutcome::Rejected(problem) => {
                if self.configuration.operation.is_some() {
                    self.configuration.record_rejection(&problem);
                    self.configuration.finish();
                } else {
                    let network = problem
                        .network_name
                        .as_deref()
                        .map(|network| format!(" · {network}"))
                        .unwrap_or_default();
                    self.error = Some(format!(
                        "{}{}",
                        format_operation_problem(self.locale, &problem),
                        network
                    ));
                }
            }
            NodeCommandOutcome::Probe(probe) => {
                self.notice = Some(tr(
                    self.locale,
                    "gpui.probe_completed",
                    &[
                        ("peer", &probe.peer_id),
                        ("rtt", &format!("{:?}", probe.rtt_us)),
                        ("timed_out", &probe.timed_out.to_string()),
                        ("p2p", &probe.p2p_active.to_string()),
                    ],
                ));
            }
            NodeCommandOutcome::NetworkProfilePreview(preview) => {
                self.notice = Some(tr(
                    self.locale,
                    "gpui.network_profile_previewed",
                    &[
                        ("network", &preview.network_name),
                        ("revision", &preview.candidate_revision.to_string()),
                        ("diffs", &preview.diff.len().to_string()),
                        ("warnings", &preview.warnings.len().to_string()),
                    ],
                ));
                self.recent_network_profile_preview = Some(*preview);
                self.recent_network_profile_updated = None;
            }
            NodeCommandOutcome::NetworkJoinPreview(preview) => {
                let preview = *preview;
                self.notice = Some(format!(
                    "{}: {} · {} {}",
                    t(self.locale, "action.preview_join_network"),
                    preview.network_name,
                    preview.member_count,
                    t(self.locale, "network.member_count")
                ));
                self.network_join_terms_confirmed = false;
                self.network_join_exit_contribution_confirmed = false;
                self.set_join_preview_fields(&preview);
                self.recent_network_join_preview = Some(preview);
                self.open_action("networks", "join_network", cx);
            }
            NodeCommandOutcome::NetworkJoined(result) => {
                self.apply_snapshot(result.snapshot, cx);
                self.clear_join_preview_state();
                self.notice = Some(format!(
                    "{}: {}",
                    t(self.locale, "action.join_network"),
                    result.membership.network_name
                ));
            }
            NodeCommandOutcome::JoinRequestSubmitted(request) => {
                self.notice = Some(tr(
                    self.locale,
                    "gpui.join_request_submitted",
                    &[
                        ("request_id", request.request_id.as_str()),
                        ("state", &format!("{:?}", request.state)),
                    ],
                ));
            }
            NodeCommandOutcome::JoinRequests(result) => {
                let result = *result;
                self.notice = Some(tr(
                    self.locale,
                    "gpui.join_requests_loaded",
                    &[("count", &result.requests.len().to_string())],
                ));
                self.join_request_selection.retain(|request_id| {
                    result.requests.iter().any(|request| {
                        request.request_id.as_str() == request_id && !request.state.is_terminal()
                    })
                });
                self.recent_join_requests = Some(result);
            }
            NodeCommandOutcome::NetworkMembers(directory) => {
                let directory = *directory;
                let count = directory.members.len();
                self.selected_member_node_id = retain_member_selection(
                    self.selected_member_node_id.as_deref(),
                    &directory.members,
                );
                self.recent_network_members = Some(directory);
                self.notice = Some(tr(
                    self.locale,
                    "gpui.network_members_loaded",
                    &[("count", &count.to_string())],
                ));
            }
            NodeCommandOutcome::MemberRemovalPreview(preview) => {
                self.notice = Some(tr(
                    self.locale,
                    "gpui.member_removal_previewed",
                    &[
                        ("network", preview.network_id.as_str()),
                        ("target", preview.target_node_id.as_str()),
                        ("effects", &preview.impact_keys.len().to_string()),
                    ],
                ));
            }
            NodeCommandOutcome::MemberLifecycleChanged(change) => {
                let notice = tr(
                    self.locale,
                    "gpui.member_lifecycle_changed",
                    &[
                        ("action", change.action.audit_action()),
                        ("target", change.target_node_id.as_str()),
                        ("revision", &change.revision.to_string()),
                        ("idempotent", &change.idempotent.to_string()),
                    ],
                );
                let topology_context = self.selected_page == "topology"
                    && self.topology_network_id().as_ref() == Some(&change.network_id);
                if topology_context {
                    self.notice = Some(notice.clone());
                    self.pending_notice_after_snapshot = Some(notice);
                    let topology_revision = self
                        .snapshot
                        .topology
                        .as_ref()
                        .map(|topology| topology.revision);
                    self.invalidate_topology_directory();
                    if let Some(topology_revision) = topology_revision {
                        self.request_topology_directory(
                            change.network_id.clone(),
                            topology_revision,
                            cx,
                        );
                    }
                    self.submit_raw(NodeCommand::GetSnapshot, cx);
                } else {
                    self.notice = Some(notice);
                    self.submit_raw(
                        NodeCommand::ListNetworkMembers(crate::node::NodeNetworkMembersCommand {
                            network_id: change.network_id.to_string(),
                        }),
                        cx,
                    );
                }
            }
            NodeCommandOutcome::JoinRequestsReviewed(result) => {
                self.apply_join_request_review_ack(&result);
                self.notice = Some(tr(
                    self.locale,
                    "gpui.join_requests_reviewed",
                    &[("count", &result.reviews.len().to_string())],
                ));
            }
            NodeCommandOutcome::JoinRequestUpdated(request) => {
                self.notice = Some(tr(
                    self.locale,
                    "gpui.join_request_updated",
                    &[
                        ("request_id", request.request_id.as_str()),
                        ("state", &format!("{:?}", request.state)),
                    ],
                ));
            }
            NodeCommandOutcome::InvitationCreated(issued) => {
                let issued = *issued;
                self.recent_invitation_status = Some(issued.status.clone());
                self.recent_issued_invitation = Some(issued);
                self.invitation_copy_notice = None;
                self.notice = Some(t(self.locale, "invitations.created_notice").to_string());
            }
            NodeCommandOutcome::Invitations(invitations) => {
                self.notice = Some(tr(
                    self.locale,
                    "invitations.loaded_notice",
                    &[("count", &invitations.len().to_string())],
                ));
                self.recent_invitations = Some(invitations.to_vec());
            }
            NodeCommandOutcome::InvitationStatus(status) => {
                self.recent_invitation_status = Some(*status);
                self.notice = Some(t(self.locale, "invitations.status_notice").to_string());
            }
            NodeCommandOutcome::InvitationRevoked(status) => {
                self.update_invitation_status(*status);
                self.notice = Some(t(self.locale, "invitations.revoked_notice").to_string());
            }
            NodeCommandOutcome::InvitationPreview(preview) => {
                let preview = *preview;
                self.set_invitation_preview_fields(&preview);
                self.recent_invitation_preview = Some(preview);
                self.invitation_terms_confirmed = false;
                self.invitation_exit_contribution_confirmed = false;
                self.notice = Some(t(self.locale, "invitations.preview_notice").to_string());
            }
            NodeCommandOutcome::InvitationRedeemed(redemption) => {
                self.recent_invitation_preview = None;
                self.invitation_terms_confirmed = false;
                self.invitation_exit_contribution_confirmed = false;
                self.notice = Some(tr(
                    self.locale,
                    "invitations.redeemed_notice",
                    &[("role", invitation_role_name(redemption.role))],
                ));
                self.submit_raw(NodeCommand::GetSnapshot, cx);
            }
            NodeCommandOutcome::MembershipPolicyAcknowledged(membership) => {
                self.notice = Some(tr(
                    self.locale,
                    "gpui.membership_policy_acknowledged",
                    &[("network", &membership.network_name)],
                ));
                self.submit_raw(NodeCommand::GetSnapshot, cx);
            }
            NodeCommandOutcome::NetworkLeft(result) => {
                self.apply_snapshot(result.snapshot, cx);
                self.notice = Some(format!(
                    "{}: {}",
                    t(self.locale, "action.leave_network"),
                    result.network_name
                ));
            }
            NodeCommandOutcome::NetworkProfileUpdated(result) => {
                self.notice = Some(tr(
                    self.locale,
                    "gpui.network_profile_updated",
                    &[
                        ("network", &result.network_name),
                        ("previous", &result.previous_revision.to_string()),
                        ("revision", &result.revision.to_string()),
                        ("warnings", &result.warnings.len().to_string()),
                    ],
                ));
                self.recent_network_profile_preview = None;
                self.recent_network_profile_updated = Some(*result);
                self.submit_raw(NodeCommand::GetSnapshot, cx);
            }
            NodeCommandOutcome::NetworkCreated(created) => {
                let key = if created.started {
                    "gpui.network_created_started"
                } else {
                    "gpui.network_created_not_started"
                };
                let next_steps = created
                    .next_steps
                    .iter()
                    .map(|step| {
                        let key = match step {
                            NodeNetworkCreationNextStep::InviteMembers => {
                                "next_step.invite_members"
                            }
                            NodeNetworkCreationNextStep::ConfigurePolicy => {
                                "next_step.configure_policy"
                            }
                            NodeNetworkCreationNextStep::ContributeCapabilities => {
                                "next_step.contribute_capabilities"
                            }
                        };
                        t(self.locale, key)
                    })
                    .collect::<Vec<_>>()
                    .join(t(self.locale, "common.list_separator"));
                self.notice = Some(tr(
                    self.locale,
                    key,
                    &[
                        ("network", &created.network_name),
                        ("network_id", &created.network_id),
                        (
                            "persistence",
                            if created.config_saved {
                                t(self.locale, "outcome.persisted")
                            } else {
                                t(self.locale, "outcome.not_persisted")
                            },
                        ),
                        (
                            "startup",
                            if created.started {
                                t(self.locale, "outcome.started")
                            } else {
                                t(self.locale, "outcome.not_started")
                            },
                        ),
                        ("next_steps", &next_steps),
                        ("ticket", &created.invitation_ticket),
                    ],
                ));
            }
            NodeCommandOutcome::Diagnostics(diagnostics) => {
                let residue = &diagnostics.residue;
                let summary = if residue.is_clean() {
                    t(self.locale, "gpui.no_residue").to_string()
                } else {
                    residue.issues.join(", ")
                };
                self.notice = Some(tr(
                    self.locale,
                    "gpui.diagnostics_completed",
                    &[
                        ("sessions", &residue.sessions_remaining.to_string()),
                        ("listeners", &residue.listeners_remaining.to_string()),
                        ("peers", &residue.peers_remaining.to_string()),
                        ("services", &residue.embedded_services_remaining.to_string()),
                        (
                            "cleanup_candidates",
                            &diagnostics.cleanup_candidate_networks.len().to_string(),
                        ),
                        ("summary", &summary),
                    ],
                ));
                self.recent_diagnostics = Some(*diagnostics);
            }
            NodeCommandOutcome::Cleanup(cleanup) => {
                let summary = if cleanup.residue.issues.is_empty() {
                    t(self.locale, "gpui.no_reported_issues").to_string()
                } else {
                    cleanup.residue.issues.join(", ")
                };
                self.notice = Some(tr(
                    self.locale,
                    "gpui.cleanup_completed",
                    &[
                        ("cleaned", &cleanup.cleaned_networks.len().to_string()),
                        ("failed", &cleanup.failed_networks.len().to_string()),
                        ("summary", &summary),
                    ],
                ));
            }
            NodeCommandOutcome::Blacklist(snapshot) => {
                let changed = if snapshot.changed {
                    t(self.locale, "gpui.state_changed")
                } else {
                    ""
                };
                self.notice = Some(tr(
                    self.locale,
                    "gpui.blacklist_synced",
                    &[
                        ("network", &snapshot.network_name),
                        ("count", &snapshot.clients.len().to_string()),
                        ("changed", changed),
                    ],
                ));
            }
            NodeCommandOutcome::NetworkAllocation(snapshot) => {
                self.notice = Some(tr(
                    self.locale,
                    "gpui.allocation_synced",
                    &[
                        ("network", &snapshot.network_name),
                        ("mode", &format!("{:?}", snapshot.allocation.mode)),
                        ("ipv4", &snapshot.allocation.ipv4_pool),
                        ("ipv6", &snapshot.allocation.ipv6_pool),
                        ("persisted", &snapshot.config_saved.to_string()),
                    ],
                ));
            }
            NodeCommandOutcome::IdentityRotated(rotation) => {
                if let Some(identity) = self.snapshot.identity.as_mut() {
                    identity.node_id = rotation.new_node_id.clone();
                    identity.public_key_fingerprint = rotation.new_fingerprint.clone();
                    identity.created_at_unix_ms = Some(rotation.created_at_unix_ms);
                }
                self.set_field_value(
                    "rotate_node_identity",
                    "expected_fingerprint",
                    rotation.new_fingerprint.clone(),
                );
                if let Some(input) = self
                    .text_inputs
                    .get(&self.field_key("rotate_node_identity", "expected_fingerprint"))
                    .cloned()
                {
                    input.update(cx, |input, cx| {
                        input.set_value(rotation.new_fingerprint.clone(), cx)
                    });
                }
                self.pending_notice_after_snapshot = Some(tr(
                    self.locale,
                    "gpui.identity_rotated",
                    &[
                        ("old_node_id", &rotation.old_node_id),
                        ("new_node_id", &rotation.new_node_id),
                        ("old_fingerprint", &rotation.old_fingerprint),
                        ("new_fingerprint", &rotation.new_fingerprint),
                        (
                            "networks",
                            &rotation.affected_configured_network_count.to_string(),
                        ),
                        (
                            "sessions",
                            if rotation.owner_sessions_invalidated {
                                t(self.locale, "identity.rotation.sessions_invalidated")
                            } else {
                                t(self.locale, "identity.rotation.sessions_preserved")
                            },
                        ),
                    ],
                ));
                self.submit_raw(NodeCommand::GetSnapshot, cx);
            }
            NodeCommandOutcome::Configuration(document) => {
                self.configuration.receive_document(document, true);
                self.configuration
                    .set_notice(t(self.locale, "gpui.configuration_loaded").to_string());
                self.configuration.finish();
                self.load_configuration_history(cx);
            }
            NodeCommandOutcome::ConfigurationPreview(preview) => {
                let preview = *preview;
                let diff_count = preview.diffs.len();
                let validation_count = preview.validation_errors.len();
                self.configuration.receive_preview(preview);
                let notice = if self.configuration.has_revision_conflict() {
                    t(self.locale, "configuration.revision_conflict").to_string()
                } else {
                    tr(
                        self.locale,
                        "gpui.configuration_previewed",
                        &[
                            ("diffs", &diff_count.to_string()),
                            ("errors", &validation_count.to_string()),
                        ],
                    )
                };
                self.configuration.set_notice(notice);
                self.configuration.finish();
            }
            NodeCommandOutcome::ConfigurationApplied(result) => {
                let result = *result;
                self.configuration.set_notice(tr(
                    self.locale,
                    "gpui.configuration_applied",
                    &[
                        ("previous", &result.previous_revision.to_string()),
                        ("revision", &result.revision.to_string()),
                        ("no_op", &result.no_op.to_string()),
                    ],
                ));
                self.configuration.receive_applied(result);
                self.configuration.finish();
                self.load_configuration_history(cx);
            }
            NodeCommandOutcome::ConfigurationHistory(history) => {
                let count = history.len();
                self.configuration.receive_history(history);
                self.configuration.set_notice(tr(
                    self.locale,
                    "gpui.configuration_history_loaded",
                    &[("count", &count.to_string())],
                ));
                self.configuration.finish();
            }
            NodeCommandOutcome::Policy(current) => {
                self.notice = Some(format!(
                    "{}: revision {} · {} network ACL(s)",
                    t(self.locale, "action.get_policy"),
                    current.revision,
                    current.snapshot.network_acls.len()
                ));
            }
            NodeCommandOutcome::PolicyPreview(preview) => {
                self.notice = Some(format!(
                    "{}: revision {} → {} · {} diff(s) · risk {:?} · can_apply={}",
                    t(self.locale, "action.preview_policy"),
                    preview.current_revision,
                    preview.expected_revision,
                    preview.diffs.len(),
                    preview.risk.level,
                    preview.can_apply
                ));
            }
            NodeCommandOutcome::PolicyApplied(result) => {
                self.notice = Some(format!(
                    "{}: revision {} → {} · no_op={}",
                    t(self.locale, "action.apply_policy"),
                    result.previous_revision,
                    result.revision,
                    result.no_op
                ));
            }
            NodeCommandOutcome::PolicyHistory(history) => {
                self.notice = Some(format!(
                    "{}: {} entries · current revision {}",
                    t(self.locale, "action.get_policy_history"),
                    history.entries.len(),
                    history.current_revision
                ));
            }
            NodeCommandOutcome::PolicyRolledBack(result) => {
                self.notice = Some(format!(
                    "{}: target {} · revision {} → {}",
                    t(self.locale, "action.rollback_policy"),
                    result.target_revision,
                    result.previous_revision,
                    result.revision
                ));
            }
            NodeCommandOutcome::PolicyTest(result) => {
                self.notice = Some(format!(
                    "{}: {} · {:?} · {}",
                    t(self.locale, "action.test_policy"),
                    if result.decision.allowed {
                        t(self.locale, "policy_test.allowed")
                    } else {
                        t(self.locale, "policy_test.denied")
                    },
                    result.decision.reason,
                    tr(
                        self.locale,
                        "gpui.policy_test_completed",
                        &[
                            ("rules", &result.explanations.len().to_string()),
                            ("issues", &result.validation_issues.len().to_string()),
                        ],
                    )
                ));
            }
            NodeCommandOutcome::TrafficPolicy(current) => {
                self.notice = Some(format!(
                    "traffic policy · revision {} · {} rules",
                    current.revision,
                    current.policy.rules.len()
                ));
            }
            NodeCommandOutcome::TrafficPolicyPreview(preview) => {
                self.notice = Some(format!(
                    "traffic policy preview · revision {} → {} · {} diffs · apply={}",
                    preview.current_revision,
                    preview.candidate_revision,
                    preview.diffs.len(),
                    preview.can_apply
                ));
            }
            NodeCommandOutcome::TrafficPolicyApplied(result) => {
                self.notice = Some(format!(
                    "traffic policy applied · revision {} → {} · no_op={}",
                    result.previous_revision, result.revision, result.no_op
                ));
            }
            NodeCommandOutcome::TrafficPolicyHistory(history) => {
                self.notice = Some(format!(
                    "traffic policy history · {} entries · current revision {}",
                    history.entries.len(),
                    history.current_revision
                ));
            }
            NodeCommandOutcome::TrafficPolicyRolledBack(result) => {
                self.notice = Some(format!(
                    "traffic policy rolled back · target {} · revision {} → {}",
                    result.target_revision, result.previous_revision, result.revision
                ));
            }
            NodeCommandOutcome::TrafficPolicyTest(comparison) => {
                self.notice = Some(format!(
                    "traffic policy test · changed={} · route_changed={}",
                    comparison.delta.changed, comparison.delta.route_changed
                ));
            }
            NodeCommandOutcome::LocalExitCapability(snapshot) => {
                self.notice = Some(format!(
                    "local Exit · enabled={} · stage={:?} · healthy={}",
                    snapshot.owner_exit_enabled, snapshot.stage, snapshot.healthy
                ));
            }
            NodeCommandOutcome::CoordinatorCandidates(status) => {
                let leader = status
                    .current_leader
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "none".into());
                self.notice = Some(format!(
                    "Coordinator 候选 · config={:?} · candidates={} · leader={} · emergency={:?}",
                    status.config_state,
                    status.candidates.len(),
                    leader,
                    status.emergency
                ));
            }
            NodeCommandOutcome::AdminRecoveryStatus(status) => {
                self.notice = Some(format!(
                    "管理员恢复 · {} · state={:?} · approvals={}/{} · can_execute={}",
                    status.request_id,
                    status.state,
                    status.signature_count,
                    status.required_signatures,
                    status.can_execute
                ));
            }
            NodeCommandOutcome::AdminRecoveries(statuses) => {
                self.notice = Some(format!("管理员恢复请求 · {} 项", statuses.len()));
            }
            NodeCommandOutcome::Accepted { operation_id } => {
                self.notice = Some(tr(
                    self.locale,
                    "gpui.command_accepted",
                    &[("operation_id", &operation_id.to_string())],
                ));
            }
            NodeCommandOutcome::Completed { message } => {
                self.notice = Some(message);
            }
        }
    }

    fn initialize_forms(&mut self, cx: &mut Context<Self>) {
        let Some(manifest) = &self.manifest else {
            return;
        };
        let mut defaults = HashMap::new();
        let mut text_fields = Vec::new();
        for action in manifest.pages.iter().flat_map(|page| &page.actions) {
            if is_configuration_action(&action.id) {
                continue;
            }
            for (field, value) in
                default_form_values(action, &self.snapshot, self.selected_network.as_deref())
            {
                defaults.insert(self.field_key(&action.id, &field), value);
            }
            for field in &action.fields {
                if !matches!(
                    field.input,
                    NodeUiInputKind::Boolean
                        | NodeUiInputKind::Role
                        | NodeUiInputKind::RoutingMode
                        | NodeUiInputKind::NetworkSelection
                        | NodeUiInputKind::Visibility
                        | NodeUiInputKind::JoinMode
                        | NodeUiInputKind::ExitContributionMode
                        | NodeUiInputKind::ExitContribution
                        | NodeUiInputKind::ExitContributionScope
                        | NodeUiInputKind::JoinRequestReviewAction
                ) {
                    let key = self.field_key(&action.id, &field.id);
                    text_fields.push((
                        key,
                        SharedString::from(format!("field-{}-{}", action.id, field.id)),
                        SharedString::from(field_label(self.locale, &field.label_key)),
                        SharedString::from(placeholder_for(
                            self.locale,
                            &field.input,
                            &action.id,
                            &field.id,
                        )),
                    ));
                }
            }
        }
        for (key, value) in defaults {
            self.form_values.entry(key).or_insert(value);
        }
        if let Some(fingerprint) = self
            .snapshot
            .identity
            .as_ref()
            .map(|identity| identity.public_key_fingerprint.clone())
        {
            self.set_field_value("rotate_node_identity", "expected_fingerprint", fingerprint);
        }
        self.fill_empty_network_fields();

        for (key, id, label, placeholder) in text_fields {
            if self.text_inputs.contains_key(&key) {
                continue;
            }
            let value = self.form_values.get(&key).cloned().unwrap_or_default();
            let input = cx.new(|cx| NativeTextInput::new(id, label, placeholder, value, cx));
            let subscription_key = key.clone();
            cx.subscribe(
                &input,
                move |shell: &mut Self, _, event: &TextInputEvent, cx| {
                    shell
                        .form_values
                        .insert(subscription_key.clone(), event.value.clone());
                    cx.notify();
                },
            )
            .detach();
            self.text_inputs.insert(key, input);
        }
        self.sync_text_inputs(cx);
        if !self.configuration_initial_load_requested
            && self.configuration_action("get_configuration").is_some()
        {
            self.configuration_initial_load_requested = true;
            self.load_configuration(cx);
        }
    }

    fn sync_text_inputs(&mut self, cx: &mut Context<Self>) {
        for (key, input) in &self.text_inputs {
            let Some(value) = self.form_values.get(key).cloned() else {
                continue;
            };
            if input.read(cx).value() != value {
                input.update(cx, |input, cx| input.set_value(value, cx));
            }
        }
    }

    /// Discovery search is local-only. Keep the raw input in memory, then
    /// publish it to the projection after a short quiet period so each
    /// keystroke does not rebuild the result list.
    fn set_discovery_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.discovery_query = query;
        self.discovery_search_generation = self.discovery_search_generation.wrapping_add(1);
        let generation = self.discovery_search_generation;
        cx.spawn(async move |weak, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(250))
                .await;
            let _ = weak.update(cx, |shell, cx| {
                if shell.discovery_search_generation == generation {
                    shell.discovery_debounced_query = shell.discovery_query.clone();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn clear_discovery_search(&mut self, cx: &mut Context<Self>) {
        self.discovery_query.clear();
        self.discovery_debounced_query.clear();
        self.discovery_search_generation = self.discovery_search_generation.wrapping_add(1);
        if let Some(input) = &self.discovery_search_input {
            input.update(cx, |input, cx| input.set_value("", cx));
        }
        cx.notify();
    }

    fn clear_routing_target_search(&mut self, cx: &mut Context<Self>) {
        self.routing_target_query.clear();
        if let Some(input) = &self.routing_target_search_input {
            input.update(cx, |input, cx| input.set_value("", cx));
        }
        cx.notify();
    }

    fn discovery_projection(&self) -> DiscoveryProjection {
        project_discovery(
            &self.snapshot,
            &self.discovery_debounced_query,
            &self.discovery_filters,
            unix_time_ms(),
        )
    }

    fn discovery_filter_options(&self) -> (Vec<String>, Vec<String>) {
        let projection = project_discovery(
            &self.snapshot,
            "",
            &DiscoveryFilters::default(),
            unix_time_ms(),
        );
        let languages = projection
            .results
            .iter()
            .map(|result| result.language.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let regions = projection
            .results
            .iter()
            .map(|result| result.region.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        (languages, regions)
    }

    fn request_discovery_join(&mut self, result: &DiscoveryResult, cx: &mut Context<Self>) {
        self.clear_join_preview_state();
        self.set_field_value(
            "preview_join_network",
            "network_id",
            result.network_id.as_str(),
        );
        self.sync_text_inputs(cx);
        self.open_action("networks", "preview_join_network", cx);
    }

    fn prepare_join_request_review(
        &mut self,
        network_id: String,
        request_id: String,
        expected_revision: u64,
        action: JoinRequestReviewAction,
        cx: &mut Context<Self>,
    ) {
        self.set_field_value("review_join_request", "network_id", network_id);
        self.set_field_value("review_join_request", "request_id", request_id);
        self.set_field_value(
            "review_join_request",
            "expected_revision",
            expected_revision.to_string(),
        );
        self.set_field_value(
            "review_join_request",
            "action",
            join_request_review_action_value(action),
        );
        self.set_field_value("review_join_request", "note", "");
        self.sync_text_inputs(cx);
        self.open_action("networks", "review_join_request", cx);
    }

    fn prepare_batch_join_request_review(
        &mut self,
        network_id: String,
        requests: &[(String, u64)],
        action: JoinRequestReviewAction,
        cx: &mut Context<Self>,
    ) {
        let reviews = requests
            .iter()
            .map(
                |(request_id, expected_revision)| NodeJoinRequestReviewItem {
                    request_id: request_id.clone(),
                    expected_revision: *expected_revision,
                    action,
                    note: None,
                },
            )
            .collect::<Vec<_>>();
        let reviews = serde_json::to_string(&reviews).unwrap_or_else(|_| "[]".into());
        self.set_field_value("batch_review_join_requests", "network_id", network_id);
        self.set_field_value("batch_review_join_requests", "reviews", reviews);
        self.set_field_value("batch_review_join_requests", "note", "");
        self.sync_text_inputs(cx);
        self.open_action("networks", "batch_review_join_requests", cx);
    }

    fn toggle_join_request_selection(&mut self, request_id: String, cx: &mut Context<Self>) {
        if !self.join_request_selection.insert(request_id.clone()) {
            self.join_request_selection.remove(&request_id);
        }
        cx.notify();
    }

    fn toggle_all_join_request_selection(
        &mut self,
        request_ids: &[String],
        cx: &mut Context<Self>,
    ) {
        let all_selected = !request_ids.is_empty()
            && request_ids
                .iter()
                .all(|request_id| self.join_request_selection.contains(request_id));
        if all_selected {
            for request_id in request_ids {
                self.join_request_selection.remove(request_id);
            }
        } else {
            self.join_request_selection
                .extend(request_ids.iter().cloned());
        }
        cx.notify();
    }

    fn apply_join_request_review_ack(&mut self, ack: &NodeJoinRequestReviewAck) {
        if let Some(result) = self
            .recent_join_requests
            .as_mut()
            .filter(|result| result.network_id == ack.network_id)
        {
            let now = unix_time_ms();
            for request in &mut result.requests {
                let Some(review) = ack
                    .reviews
                    .iter()
                    .find(|review| review.request_id == request.request_id.as_str())
                else {
                    continue;
                };
                request.state = review.state;
                request.revision = review.revision;
                request.updated_at_unix_ms = now;
            }
        }
        for review in &ack.reviews {
            self.join_request_selection.remove(&review.request_id);
        }
    }

    fn set_join_preview_fields(&mut self, preview: &NodeNetworkJoinPreview) {
        self.set_field_value("join_network", "target", "");
        self.set_field_value("join_network", "network_id", preview.network_id.clone());
        self.set_field_value("join_network", "network_name", preview.network_name.clone());
        self.set_field_value(
            "join_network",
            "preview_digest",
            preview.preview_digest.clone(),
        );
        self.set_field_value(
            "join_network",
            "network_policy_revision",
            preview.network_policy_revision.to_string(),
        );
        self.set_field_value(
            "join_network",
            "privacy_policy_revision",
            preview.privacy_policy_revision.to_string(),
        );
        self.set_field_value("join_network", "terms_hash", preview.terms_hash.as_str());
        self.set_field_value("join_network", "acknowledged_at_unix_ms", "");
        self.set_field_value("join_network", "start", "true");
    }

    fn clear_join_preview_state(&mut self) {
        self.recent_network_join_preview = None;
        self.network_join_terms_confirmed = false;
        self.network_join_exit_contribution_confirmed = false;
        for field in [
            "network_id",
            "preview_digest",
            "network_policy_revision",
            "privacy_policy_revision",
            "terms_hash",
            "acknowledged_at_unix_ms",
            "exit_contribution_acknowledged",
        ] {
            self.set_field_value("join_network", field, "");
        }
    }

    fn confirm_join_preview(&mut self, cx: &mut Context<Self>) {
        let Some(preview) = self.recent_network_join_preview.as_ref() else {
            self.error = Some(t(self.locale, "join.error.unavailable").to_string());
            cx.notify();
            return;
        };
        if !preview.can_join || preview.already_member || !self.network_join_terms_confirmed {
            self.error = Some(t(self.locale, "join.error.terms_required").to_string());
            cx.notify();
            return;
        }
        if preview.join_requires_exit_consent && !self.network_join_exit_contribution_confirmed {
            self.error =
                Some(t(self.locale, "join.error.exit_contribution_consent_required").to_string());
            cx.notify();
            return;
        }
        let join_requires_exit_consent = preview.join_requires_exit_consent;
        self.set_field_value(
            "join_network",
            "acknowledged_at_unix_ms",
            unix_time_ms().to_string(),
        );
        self.set_field_value(
            "join_network",
            "exit_contribution_acknowledged",
            if join_requires_exit_consent {
                "true"
            } else {
                "false"
            },
        );
        self.submit_manifest_action("join_network", cx);
    }

    fn update_invitation_status(&mut self, status: InvitationStatusView) {
        if let Some(invitations) = self.recent_invitations.as_mut() {
            if let Some(existing) = invitations.iter_mut().find(|candidate| {
                candidate.invitation.invitation_id == status.invitation.invitation_id
            }) {
                *existing = status.clone();
            }
        }
        self.recent_invitation_status = Some(status);
    }

    fn invitation_action_available(&self, action_id: &str) -> bool {
        let Some(action) = self.manifest.as_ref().and_then(|manifest| {
            manifest
                .pages
                .iter()
                .flat_map(|page| &page.actions)
                .find(|action| action.id == action_id)
        }) else {
            return false;
        };
        self.pending_commands == 0
            && action.backend_status == NodeUiBackendStatus::Implemented
            && complete_action_block_reason(
                action,
                &self.snapshot,
                self.selected_network.as_deref(),
            )
            .is_none()
    }

    fn set_invitation_preview_fields(&mut self, preview: &InvitationPreview) {
        self.set_field_value(
            "redeem_invitation",
            "network_id",
            preview.network_id.as_str(),
        );
        self.set_field_value(
            "redeem_invitation",
            "invitation_id",
            preview.invitation_id.as_str(),
        );
        self.set_field_value(
            "redeem_invitation",
            "expected_invitation_revision",
            preview.invitation_revision.to_string(),
        );
        self.set_field_value(
            "redeem_invitation",
            "preview_digest",
            preview.preview_digest.as_str(),
        );
        self.set_field_value(
            "redeem_invitation",
            "terms_hash",
            preview.terms_hash.as_str(),
        );
        self.set_field_value("redeem_invitation", "acknowledged_at_unix_ms", "");
        for field in ["code", "coordinator_url"] {
            let source = self.field_value("preview_invitation", field);
            self.set_field_value("redeem_invitation", field, source);
        }
    }

    fn clear_invitation_preview(&mut self) {
        self.recent_invitation_preview = None;
        self.invitation_terms_confirmed = false;
        self.invitation_exit_contribution_confirmed = false;
        for field in [
            "network_id",
            "code",
            "coordinator_url",
            "invitation_id",
            "expected_invitation_revision",
            "preview_digest",
            "terms_hash",
            "acknowledged_at_unix_ms",
            "exit_contribution_acknowledged",
        ] {
            self.set_field_value("redeem_invitation", field, "");
        }
    }

    fn return_to_invitation_edit(&mut self, cx: &mut Context<Self>) {
        self.clear_invitation_preview();
        self.open_action("networks", "preview_invitation", cx);
    }

    fn confirm_invitation_preview(&mut self, cx: &mut Context<Self>) {
        let Some(preview) = self.recent_invitation_preview.as_ref() else {
            self.error = Some(t(self.locale, "invitations.denied").to_string());
            cx.notify();
            return;
        };
        if !preview.can_accept
            || preview.state != InvitationState::Active
            || preview.remaining_uses == 0
            || preview.expires_at_unix_ms <= unix_time_ms()
            || !self.invitation_terms_confirmed
        {
            self.error = Some(t(self.locale, "invitations.denied").to_string());
            cx.notify();
            return;
        }
        if preview
            .terms
            .exit_contribution_mode
            .requires_join_commitment()
            && !self.invitation_exit_contribution_confirmed
        {
            self.error =
                Some(t(self.locale, "join.error.exit_contribution_consent_required").to_string());
            cx.notify();
            return;
        }
        let exit_contribution_required = preview
            .terms
            .exit_contribution_mode
            .requires_join_commitment();
        self.set_field_value(
            "redeem_invitation",
            "acknowledged_at_unix_ms",
            unix_time_ms().to_string(),
        );
        self.set_field_value(
            "redeem_invitation",
            "exit_contribution_acknowledged",
            if exit_contribution_required {
                "true"
            } else {
                "false"
            },
        );
        self.submit_manifest_action("redeem_invitation", cx);
    }

    fn prepare_invitation_status(&mut self, status: &InvitationStatusView, cx: &mut Context<Self>) {
        self.set_field_value(
            "get_invitation_status",
            "network_id",
            status.invitation.network_id.as_str(),
        );
        self.set_field_value(
            "get_invitation_status",
            "invitation_id",
            status.invitation.invitation_id.as_str(),
        );
        self.sync_text_inputs(cx);
        self.open_action("networks", "get_invitation_status", cx);
    }

    fn prepare_invitation_revoke(&mut self, status: &InvitationStatusView, cx: &mut Context<Self>) {
        self.set_field_value(
            "revoke_invitation",
            "network_id",
            status.invitation.network_id.as_str(),
        );
        self.set_field_value(
            "revoke_invitation",
            "invitation_id",
            status.invitation.invitation_id.as_str(),
        );
        self.set_field_value(
            "revoke_invitation",
            "expected_revision",
            status.invitation.revision.to_string(),
        );
        self.sync_text_inputs(cx);
        self.open_action("networks", "revoke_invitation", cx);
    }

    fn fill_empty_network_fields(&mut self) {
        let Some(selected) = self.selected_network.clone() else {
            return;
        };
        let Some(manifest) = &self.manifest else {
            return;
        };
        let defaults = manifest
            .pages
            .iter()
            .flat_map(|page| &page.actions)
            .flat_map(|action| {
                default_form_values(action, &self.snapshot, Some(&selected))
                    .into_iter()
                    .map(|(field_id, value)| (self.field_key(&action.id, &field_id), value))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        for (key, value) in defaults {
            if !value.is_empty() && self.form_values.get(&key).is_none_or(String::is_empty) {
                self.form_values.insert(key, value);
            }
        }
    }

    fn field_key(&self, action_id: &str, field_id: &str) -> String {
        format!("{action_id}.{field_id}")
    }

    fn field_value(&self, action_id: &str, field_id: &str) -> String {
        self.form_values
            .get(&self.field_key(action_id, field_id))
            .cloned()
            .unwrap_or_default()
    }

    fn set_field_value(&mut self, action_id: &str, field_id: &str, value: impl Into<String>) {
        self.form_values
            .insert(self.field_key(action_id, field_id), value.into());
    }

    fn page(&self) -> Option<&NodeUiPage> {
        self.manifest
            .as_ref()?
            .pages
            .iter()
            .find(|page| page.id == self.selected_page)
    }

    fn module_landing_page(&self, module: &NavigationModule) -> String {
        let Some(manifest) = self.manifest.as_ref() else {
            return module.default_page_id.clone();
        };
        manifest
            .pages
            .iter()
            .find(|page| page.id == module.default_page_id)
            .or_else(|| {
                manifest
                    .pages
                    .iter()
                    .find(|page| module.page_ids.contains(&page.id))
            })
            .map(|page| page.id.clone())
            .unwrap_or_else(|| module.default_page_id.clone())
    }

    fn language_selector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected_label = t(self.locale, self.language_preference.label_key()).to_string();
        let mut selector = div().relative().child(
            div()
                .id("language-menu-trigger")
                .role(Role::Button)
                .aria_label(t(self.locale, "common.language").to_string())
                .focusable()
                .tab_stop(true)
                .flex()
                .items_center()
                .gap_2()
                .h(px(34.0))
                .px_3()
                .rounded(px(9.0))
                .bg(rgba(DARK_THEME.colors.surface.rgba_hex()))
                .text_size(px(12.0))
                .font_weight(gpui::FontWeight(600.0))
                .text_color(rgba(COLOR_TEXT_SECONDARY))
                .child(lucide_icon(
                    include_bytes!("../../assets/icons/globe-2.svg"),
                    16.0,
                    COLOR_TEXT_SECONDARY,
                ))
                .child(selected_label)
                .child(lucide_icon(
                    include_bytes!("../../assets/icons/chevron-down.svg"),
                    14.0,
                    COLOR_MUTED,
                ))
                .on_click(cx.listener(|shell, _, _, cx| {
                    shell.language_menu_open = !shell.language_menu_open;
                    shell.network_menu_open = false;
                    cx.notify();
                })),
        );

        if self.language_menu_open {
            selector = selector.child(
                div()
                    .absolute()
                    .top(px(38.0))
                    .right(px(0.0))
                    .w(px(180.0))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_1()
                    .rounded_md()
                    .bg(rgba(COLOR_PANEL_RAISED))
                    .children(LanguagePreference::ALL.into_iter().map(|preference| {
                        let selected = self.language_preference == preference;
                        button(
                            SharedString::from(format!("language-{}", preference.storage_value())),
                            t(self.locale, preference.label_key()).to_string(),
                            if selected {
                                ButtonVariant::Secondary
                            } else {
                                ButtonVariant::Ghost
                            },
                            false,
                        )
                        .w_full()
                        .on_click(cx.listener(move |shell, _, _, cx| {
                            shell.language_menu_open = false;
                            shell.set_language(preference, cx);
                        }))
                    })),
            );
        }

        selector
    }

    fn network_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected_label = self
            .selected_network
            .clone()
            .or_else(|| {
                self.snapshot
                    .networks
                    .first()
                    .map(|network| network.name.clone())
            })
            .unwrap_or_else(|| t(self.locale, "common.no_running_networks").to_string());
        let disabled = self.snapshot.networks.is_empty();
        let mut picker = div()
            .relative()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_size(px(10.0))
                    .font_weight(gpui::FontWeight(600.0))
                    .text_color(rgba(COLOR_FAINT))
                    .child(
                        t(self.locale, "common.current_network")
                            .to_string()
                            .to_uppercase(),
                    ),
            )
            .child(
                div()
                    .id("network-picker-trigger")
                    .role(Role::Button)
                    .aria_label(t(self.locale, "common.current_network").to_string())
                    .focusable()
                    .tab_stop(!disabled)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .h(px(32.0))
                    .min_w(px(180.0))
                    .max_w(px(220.0))
                    .px_3()
                    .rounded(px(9.0))
                    .bg(rgba(DARK_THEME.colors.input_background.rgba_hex()))
                    .text_size(px(12.0))
                    .text_color(rgba(if disabled { COLOR_FAINT } else { COLOR_TEXT }))
                    .opacity(if disabled { 0.6 } else { 1.0 })
                    .child(selected_label)
                    .child(lucide_icon(
                        include_bytes!("../../assets/icons/chevron-down.svg"),
                        14.0,
                        COLOR_MUTED,
                    ))
                    .on_click(cx.listener(|shell, _, _, cx| {
                        shell.network_menu_open = !shell.network_menu_open;
                        shell.language_menu_open = false;
                        cx.notify();
                    })),
            );

        if self.network_menu_open && !self.snapshot.networks.is_empty() {
            picker = picker.child(
                div()
                    .absolute()
                    .top(px(42.0))
                    .right(px(0.0))
                    .w(px(220.0))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_1()
                    .rounded_md()
                    .bg(rgba(COLOR_PANEL_RAISED))
                    .children(self.snapshot.networks.iter().map(|network| {
                        let network_name = network.name.clone();
                        let selected =
                            self.selected_network.as_deref() == Some(network.name.as_str());
                        button(
                            SharedString::from(format!("network-{}", network.name)),
                            network.name.clone(),
                            if selected {
                                ButtonVariant::Secondary
                            } else {
                                ButtonVariant::Ghost
                            },
                            false,
                        )
                        .w_full()
                        .on_click(cx.listener(move |shell, _, _, cx| {
                            shell.network_menu_open = false;
                            shell.select_network_context(network_name.clone(), cx);
                            cx.notify();
                        }))
                    })),
            );
        }

        picker
    }

    fn top_status(&self) -> impl IntoElement {
        let primary_status = self.snapshot.primary_status();
        let palette = semantic_palette(primary_status_semantic(primary_status));
        let traffic_state = self.snapshot.traffic_session.state;
        let traffic_palette = semantic_palette(traffic_protection_semantic(traffic_state));
        let traffic_label = t(self.locale, traffic_protection_state_key(traffic_state));
        let traffic_detail = self
            .snapshot
            .traffic_session
            .reason_code
            .as_deref()
            .unwrap_or_else(|| t(self.locale, "traffic.session.no_reason"));
        div()
            .flex()
            .items_center()
            .gap_3()
            .min_w(px(0.0))
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w(px(0.0))
                    .child(
                        div()
                            .flex_shrink_0()
                            .w(px(9.0))
                            .h(px(9.0))
                            .rounded_full()
                            .bg(rgba(palette.foreground.rgba_hex())),
                    )
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap_2()
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .font_weight(gpui::FontWeight(600.0))
                                    .text_color(rgba(COLOR_TEXT))
                                    .child(t(self.locale, primary_status.label_key()).to_string()),
                            )
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_xs()
                                    .text_color(rgba(COLOR_MUTED))
                                    .child(t(self.locale, primary_status.detail_key()).to_string()),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .w(px(1.0))
                    .h(px(24.0))
                    .bg(rgba(COLOR_DIVIDER)),
            )
            .child(
                div()
                    .debug_selector(|| "visual-traffic-session-status".into())
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w(px(0.0))
                    .child(
                        div()
                            .flex_shrink_0()
                            .w(px(9.0))
                            .h(px(9.0))
                            .rounded_full()
                            .bg(rgba(traffic_palette.foreground.rgba_hex())),
                    )
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap_2()
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .font_weight(gpui::FontWeight(600.0))
                                    .text_color(rgba(COLOR_TEXT))
                                    .child(traffic_label.to_string()),
                            )
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_xs()
                                    .text_color(rgba(COLOR_MUTED))
                                    .child(traffic_detail.to_string()),
                            ),
                    ),
            )
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let disabled = self.pending_commands > 0;
        div()
            .id(app_region_id(AppRegion::Topbar))
            .debug_selector(|| "visual-topbar".into())
            .h(px(DIMENSIONS.topbar_px as f32))
            .flex()
            .items_center()
            .gap_4()
            .px_5()
            .child(div().flex_1().min_w(px(0.0)).child(self.top_status()))
            .child(self.network_picker(cx))
            .child(self.language_selector(cx))
            .child(
                button(
                    SharedString::from("refresh-snapshot"),
                    "",
                    ButtonVariant::Secondary,
                    disabled,
                )
                .aria_label(if disabled {
                    t(self.locale, "common.operation_in_progress").to_string()
                } else {
                    t(self.locale, "common.refresh").to_string()
                })
                .w(px(34.0))
                .h(px(34.0))
                .px_0()
                .child(lucide_icon(
                    include_bytes!("../../assets/icons/refresh-cw.svg"),
                    17.0,
                    COLOR_TEXT_SECONDARY,
                ))
                .on_click(cx.listener(|shell, _, _, cx| {
                    if shell.pending_commands == 0 {
                        shell.submit_raw(NodeCommand::GetSnapshot, cx);
                    }
                })),
            )
            .child(
                button(
                    SharedString::from("quit-application"),
                    "",
                    ButtonVariant::Secondary,
                    false,
                )
                .aria_label(t(self.locale, "common.quit").to_string())
                .w(px(34.0))
                .h(px(34.0))
                .px_0()
                .child(lucide_icon(
                    include_bytes!("../../assets/icons/log-out.svg"),
                    17.0,
                    COLOR_TEXT_SECONDARY,
                ))
                .on_click(cx.listener(|_, _, window, _| window.remove_window())),
            )
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active_module_id =
            navigation_module_for_page(&self.selected_page).map(|module| module.id.as_str());
        let nav = navigation_modules()
            .iter()
            .map(|module| {
                let page_id = self.module_landing_page(module);
                let selected = active_module_id == Some(module.id.as_str());
                let icon = navigation_icon(module.id.as_str(), 19.0, COLOR_MUTED);
                nav_tab(
                    SharedString::from(format!("nav-module-{}", module.id)),
                    t(self.locale, &module.label_key).to_string(),
                    Some(icon),
                    selected,
                )
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.selected_page = page_id.clone();
                    shell.pending_confirmation = None;
                    shell.error = None;
                    shell.notice = None;
                    shell.language_menu_open = false;
                    shell.network_menu_open = false;
                    cx.notify();
                }))
                .into_any_element()
            })
            .collect::<Vec<_>>();

        let syncing = self.pending_commands > 0;
        let sync_label = if syncing {
            t(self.locale, "common.syncing")
        } else {
            "Live"
        };
        let sync_color = if syncing {
            COLOR_WARNING
        } else {
            DARK_THEME.colors.success.rgba_hex()
        };

        div()
            .id(app_region_id(AppRegion::Sidebar))
            .debug_selector(|| "visual-sidebar".into())
            .role(A11Y_NAVIGATION_ROLE)
            .aria_label(t(self.locale, "common.main_navigation").to_string())
            .flex()
            .flex_col()
            .w(px(DIMENSIONS.sidebar_px as f32))
            .flex_shrink_0()
            .h_full()
            .bg(rgba(COLOR_SIDEBAR))
            .child(
                div()
                    .h(px(DIMENSIONS.topbar_px as f32))
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .child(
                        div()
                            .w(px(36.0))
                            .h(px(36.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .bg(rgba(DARK_THEME.colors.accent_soft.rgba_hex()))
                            .child(lucide_icon(
                                include_bytes!("../../assets/icons/wifi.svg"),
                                22.0,
                                COLOR_ACCENT,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight(600.0))
                                    .text_color(rgba(COLOR_TEXT))
                                    .child("fuckipv6"),
                            )
                            .child(
                                div()
                                    .overflow_hidden()
                                    .text_size(px(10.0))
                                    .text_color(rgba(COLOR_MUTED))
                                    .child(t(self.locale, "common.node_contract").to_string()),
                            ),
                    ),
            )
            .child(
                tabs("primary-navigation")
                    .flex_1()
                    .px_2()
                    .py_4()
                    .children(nav),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_4()
                    .py_3()
                    .text_size(px(10.0))
                    .text_color(rgba(DARK_THEME.colors.text_faint.rgba_hex()))
                    .child(format!(
                        "{} #{}",
                        t(self.locale, "common.snapshot"),
                        self.snapshot.revision
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .w(px(7.0))
                                    .h(px(7.0))
                                    .rounded_full()
                                    .bg(rgba(sync_color)),
                            )
                            .child(sync_label.to_string()),
                    ),
            )
    }

    fn error_banner(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let error = self.error.clone().unwrap_or_default();
        component_toast(
            "dismiss-error",
            tr(self.locale, "common.operation_failed", &[("error", &error)]),
            SemanticState::Danger,
        )
        .on_click(cx.listener(|shell, _, _, cx| {
            shell.error = None;
            cx.notify();
        }))
    }

    fn local_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let page_tabs = self
            .manifest
            .as_ref()
            .and_then(|manifest| {
                navigation_module_for_page(&self.selected_page).map(|module| {
                    module
                        .page_ids
                        .iter()
                        .filter_map(|page_id| {
                            manifest.pages.iter().find(|page| &page.id == page_id)
                        })
                        .map(|page| {
                            let page_id = page.id.clone();
                            let selected = page.id == self.selected_page;
                            page_tab_control(
                                SharedString::from(format!("local-page-{}", page.id)),
                                t(self.locale, &page.nav_label_key).to_string(),
                                selected,
                            )
                            .on_click(cx.listener(move |shell, _, _, cx| {
                                shell.selected_page = page_id.clone();
                                shell.pending_confirmation = None;
                                cx.notify();
                            }))
                            .into_any_element()
                        })
                        .collect::<Vec<_>>()
                })
            })
            .unwrap_or_default();
        let search_input = self
            .search_input
            .as_ref()
            .map(|input| input.clone().into_any_element());
        let clear_search = (!self.search_query.trim().is_empty()).then(|| {
            button(
                SharedString::from("clear-global-search"),
                "",
                ButtonVariant::Ghost,
                false,
            )
            .absolute()
            .right(px(4.0))
            .top(px(3.0))
            .w(px(28.0))
            .h(px(28.0))
            .px_0()
            .aria_label(t(self.locale, "common.search_clear").to_string())
            .child(lucide_icon(
                include_bytes!("../../assets/icons/x.svg"),
                14.0,
                COLOR_MUTED,
            ))
            .on_click(cx.listener(|shell, _, _, cx| shell.clear_search(cx)))
        });
        let search_control = div()
            .id("global-search")
            .relative()
            .flex()
            .items_center()
            .w(px(320.0))
            .h(px(34.0))
            .child(
                div()
                    .absolute()
                    .left(px(11.0))
                    .top(px(9.0))
                    .child(lucide_icon(
                        include_bytes!("../../assets/icons/search.svg"),
                        15.0,
                        COLOR_FAINT,
                    )),
            )
            .children(search_input)
            .children(clear_search);

        div()
            .id("local-toolbar")
            .debug_selector(|| "visual-toolbar".into())
            .h(px(48.0))
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .px_5()
            .child(
                div()
                    .id("module-page-tabs")
                    .role(A11Y_TAB_LIST_ROLE)
                    .aria_label(t(self.locale, "common.page_sections").to_string())
                    .flex()
                    .items_center()
                    .gap_1()
                    .children(page_tabs),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(search_control)
                    .children((self.pending_commands > 0).then(|| {
                        sync_badge(
                            "snapshot-syncing",
                            t(self.locale, "common.syncing").to_string(),
                        )
                    })),
            )
    }

    fn search_results_panel(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if self.search_query.trim().is_empty() {
            return None;
        }

        let results = self.search_results();
        let body = if results.is_empty() {
            div()
                .id("global-search-empty")
                .role(Role::Status)
                .px_4()
                .py_3()
                .text_sm()
                .text_color(rgba(COLOR_MUTED))
                .child(t(self.locale, "common.search_no_results").to_string())
                .into_any_element()
        } else {
            div()
                .id("global-search-results")
                .role(Role::List)
                .aria_label(t(self.locale, "common.search_results").to_string())
                .flex()
                .flex_col()
                .children(results.into_iter().map(|result| {
                    let result_id = result.element_id();
                    let button_id = format!("{result_id}-button");
                    let kind_label = t(self.locale, result.kind.label_key()).to_string();
                    let aria_label = format!("{kind_label} · {} · {}", result.title, result.detail);
                    let result_for_click = result.clone();
                    div()
                        .id(result_id)
                        .role(Role::ListItem)
                        .w_full()
                        .child(
                            div()
                                .id(button_id)
                                .role(Role::Button)
                                .aria_label(aria_label)
                                .focusable()
                                .tab_stop(true)
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_4()
                                .w_full()
                                .px_4()
                                .py_2()
                                .on_click(cx.listener(move |shell, _, window, cx| {
                                    shell.navigate_to_search_result(
                                        result_for_click.clone(),
                                        window,
                                        cx,
                                    );
                                }))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .min_w(px(0.0))
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(gpui::FontWeight(600.0))
                                                .text_color(rgba(COLOR_TEXT))
                                                .child(result.title.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgba(COLOR_MUTED))
                                                .child(result.detail.clone()),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(rgba(COLOR_ACCENT))
                                        .child(kind_label.clone()),
                                ),
                        )
                        .into_any_element()
                }))
                .into_any_element()
        };

        Some(
            div()
                .id("global-search-panel")
                .max_h(px(360.0))
                .overflow_y_scroll()
                .bg(rgba(COLOR_PANEL))
                .child(body)
                .into_any_element(),
        )
    }

    fn open_action(&mut self, page_id: &str, action_id: &str, cx: &mut Context<Self>) {
        let exists = self.manifest.as_ref().is_some_and(|manifest| {
            manifest
                .pages
                .iter()
                .find(|page| page.id == page_id)
                .is_some_and(|page| page.actions.iter().any(|action| action.id == action_id))
        });
        if !exists {
            return;
        }
        self.selected_page = page_id.to_string();
        self.pending_confirmation = None;
        self.network_actions_open = None;
        self.expanded_actions.clear();
        self.expanded_actions.insert(action_id.to_string(), true);
        cx.notify();
    }

    fn open_network_members(
        &mut self,
        network_name: String,
        network_id: String,
        cx: &mut Context<Self>,
    ) {
        self.select_network_context(network_name, cx);
        self.set_field_value("list_network_members", "network_id", network_id);
        self.sync_text_inputs(cx);
        self.recent_network_members = None;
        self.selected_member_node_id = None;
        self.open_action("networks", "list_network_members", cx);
    }

    fn select_network_context(&mut self, network_name: String, cx: &mut Context<Self>) {
        self.selected_network = Some(network_name.clone());
        self.network_actions_open = None;
        self.recent_join_requests = None;
        self.join_request_selection.clear();
        self.recent_network_members = None;
        self.selected_member_node_id = None;
        self.recent_issued_invitation = None;
        self.recent_invitations = None;
        self.recent_invitation_status = None;
        self.clear_invitation_preview();
        self.invitation_copy_notice = None;
        if let Some(manifest) = &self.manifest {
            let network_fields = manifest
                .pages
                .iter()
                .flat_map(|page| &page.actions)
                .flat_map(|action| {
                    action
                        .fields
                        .iter()
                        .filter(|field| {
                            field.input == NodeUiInputKind::NetworkName || field.id == "networks"
                        })
                        .map(|field| self.field_key(&action.id, &field.id))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            for key in network_fields {
                self.form_values.insert(key, network_name.clone());
            }
        }
        self.fill_empty_network_fields();
        self.sync_text_inputs(cx);
    }

    fn navigate_to_row(&mut self, navigation: &RowNavigation, cx: &mut Context<Self>) {
        let page_exists = self.manifest.as_ref().is_some_and(|manifest| {
            manifest
                .pages
                .iter()
                .any(|page| page.id == navigation.page_id)
        });
        if !page_exists {
            return;
        }

        self.selected_page = navigation.page_id.clone();
        if let Some(network_name) = navigation.network_name.clone() {
            let network_exists = self
                .snapshot
                .configuration
                .networks
                .iter()
                .any(|network| network.network_name == network_name)
                || self
                    .snapshot
                    .networks
                    .iter()
                    .any(|network| network.name == network_name);
            if network_exists {
                self.select_network_context(network_name.clone(), cx);
            }
            if navigation.page_id == "topology" {
                let network_id = self.snapshot.topology.as_ref().and_then(|topology| {
                    topology
                        .boundaries
                        .iter()
                        .find(|boundary| boundary.name == network_name)
                        .map(|boundary| boundary.network_id.clone())
                });
                if let Some(network_id) = network_id {
                    self.select_topology_network(network_id, cx);
                }
            }
        }

        if navigation.page_id == "topology" {
            if let Some(object_id) = navigation.object_id.as_deref() {
                let network_id = self.snapshot.topology.as_ref().and_then(|topology| {
                    let node = topology
                        .nodes
                        .iter()
                        .find(|node| node.node_id.as_str() == object_id)?;
                    let preferred = navigation.network_name.as_ref().and_then(|network_name| {
                        topology
                            .boundaries
                            .iter()
                            .find(|boundary| &boundary.name == network_name)
                            .map(|boundary| &boundary.network_id)
                    });
                    preferred
                        .filter(|network_id| node.belongs_to(network_id))
                        .or_else(|| {
                            node.networks
                                .iter()
                                .map(|network| &network.network_id)
                                .find(|network_id| topology.boundary(network_id).is_some())
                        })
                        .cloned()
                });
                if let Some(network_id) = network_id {
                    self.select_topology_network(network_id, cx);
                }
                self.open_topology_node(object_id.to_owned(), cx);
            }
        }

        self.pending_confirmation = None;
        self.language_menu_open = false;
        self.network_menu_open = false;
        self.network_actions_open = None;
        self.search_query.clear();
        if let Some(input) = &self.search_input {
            input.update(cx, |input, cx| input.set_value("", cx));
        }
        cx.notify();
    }

    fn search_results(&self) -> Vec<SearchResult> {
        let Some(manifest) = self.manifest.as_ref() else {
            return Vec::new();
        };
        search_node_snapshot(manifest, &self.snapshot, &self.search_query, self.locale)
    }

    fn clear_search(&mut self, cx: &mut Context<Self>) {
        self.search_query.clear();
        if let Some(input) = &self.search_input {
            input.update(cx, |input, cx| input.set_value("", cx));
        }
        cx.notify();
    }

    fn navigate_to_search_result(
        &mut self,
        result: SearchResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let page_exists = self
            .manifest
            .as_ref()
            .is_some_and(|manifest| manifest.pages.iter().any(|page| page.id == result.page_id));
        if !page_exists {
            return;
        }

        self.selected_page = result.page_id;
        if let Some(network_name) = result.network_name {
            if self
                .snapshot
                .networks
                .iter()
                .any(|network| network.name == network_name)
            {
                self.select_network_context(network_name, cx);
            }
        }
        self.pending_confirmation = None;
        self.language_menu_open = false;
        self.network_menu_open = false;
        self.clear_search(cx);
        self.content_focus_handle.focus(window, cx);
        cx.notify();
    }

    fn focus_search(&mut self, _: &FocusSearch, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(input) = &self.search_input {
            input.update(cx, |input, cx| input.focus(window, cx));
        }
    }

    fn overview_content(
        &self,
        overview: &OverviewModel,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let colors = DARK_THEME.colors;
        let hero_tone = match overview.primary_status {
            NodePrimaryStatus::Running => colors.success,
            NodePrimaryStatus::Degraded
            | NodePrimaryStatus::Starting
            | NodePrimaryStatus::Stopping => colors.warning,
            NodePrimaryStatus::Failed => colors.danger,
            NodePrimaryStatus::Uninitialized
            | NodePrimaryStatus::Stopped
            | NodePrimaryStatus::ControlUnavailable => colors.accent,
        };
        let hero_icon = match overview.primary_status {
            NodePrimaryStatus::Running => {
                include_bytes!("../../assets/icons/check-circle-2.svg").as_slice()
            }
            NodePrimaryStatus::Failed => {
                include_bytes!("../../assets/icons/alert-triangle.svg").as_slice()
            }
            _ => include_bytes!("../../assets/icons/activity.svg").as_slice(),
        };

        let mut primary_card = surface_card()
            .id("overview-primary-status")
            .debug_selector(|| "visual-hero".into())
            .bg(rgba(colors.surface.rgba_hex()))
            .role(Role::Status)
            .aria_label(format!(
                "{} · {}",
                overview.node_name,
                t(self.locale, overview.primary_status.label_key())
            ))
            .flex()
            .items_center()
            .gap_4()
            .p_5()
            .child(lucide_icon(hero_icon, 30.0, hero_tone.rgba_hex()))
            .child(
                div()
                    .debug_selector(|| "visual-hero-copy".into())
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(COLOR_MUTED))
                            .child(
                                t(self.locale, "common.primary_status")
                                    .to_string()
                                    .to_uppercase(),
                            ),
                    )
                    .child(
                        div()
                            .mt(px(3.0))
                            .text_size(px(20.0))
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(COLOR_TEXT))
                            .child(overview.node_name.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap_1()
                            .text_size(px(13.0))
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight(600.0))
                                    .text_color(rgba(COLOR_TEXT_SECONDARY))
                                    .child(
                                        t(self.locale, overview.primary_status.label_key())
                                            .to_string(),
                                    ),
                            )
                            .child(div().text_color(rgba(COLOR_TEXT_SECONDARY)).child(format!(
                                "· {}",
                                t(self.locale, overview.primary_status.detail_key())
                            ))),
                    ),
            );

        if let Some(page_id) =
            visible_overview_primary_page(overview, self.manifest.as_ref()).map(str::to_string)
        {
            let target = page_id.clone();
            primary_card = primary_card.child(
                button(
                    SharedString::from("overview-primary-action"),
                    t(self.locale, "overview.manage_runtime").to_string(),
                    ButtonVariant::Primary,
                    false,
                )
                .flex_shrink_0()
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.selected_page = target.clone();
                    shell.pending_confirmation = None;
                    shell.error = None;
                    shell.notice = None;
                    cx.notify();
                })),
            );
        }

        let traffic_palette =
            semantic_palette(traffic_protection_semantic(overview.traffic_session.state));
        let traffic_state_label = overview.traffic_session.state_label.clone();
        let traffic_reason = overview
            .traffic_session
            .reason_code
            .clone()
            .unwrap_or_else(|| t(self.locale, "traffic.session.no_reason").to_string());
        let traffic_failure = overview
            .traffic_session
            .failure_label
            .clone()
            .unwrap_or_else(|| t(self.locale, "common.none").to_string());
        let traffic_changed_at = if overview.traffic_session.changed_at_unix_ms == 0 {
            t(self.locale, "traffic.session.not_observed").to_string()
        } else {
            tr(
                self.locale,
                "traffic.session.changed_at_value",
                &[(
                    "time",
                    &format_diagnostic_timestamp(
                        self.locale,
                        overview.traffic_session.changed_at_unix_ms,
                    ),
                )],
            )
        };
        let meta_item = |label: &'static str, value: String| {
            div()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .w(px(230.0))
                .p_2()
                .border_1()
                .border_color(rgba(COLOR_DIVIDER))
                .rounded(px(8.0))
                .text_xs()
                .text_color(rgba(COLOR_MUTED))
                .child(label.to_string())
                .child(
                    div()
                        .font_family("monospace")
                        .text_color(rgba(COLOR_TEXT))
                        .child(value),
                )
        };
        let mut traffic_card = surface_card()
            .id("overview-traffic-session")
            .debug_selector(|| "visual-traffic-session".into())
            .role(Role::Status)
            .aria_label(format!(
                "{} · {}",
                t(self.locale, "traffic.session.title"),
                traffic_state_label
            ))
            .flex()
            .flex_col()
            .p_4()
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap_3()
                    .mb_3()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .line_height(px(21.7))
                                    .font_weight(gpui::FontWeight(600.0))
                                    .text_color(rgba(COLOR_TEXT))
                                    .child(t(self.locale, "traffic.session.title").to_string()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgba(COLOR_MUTED))
                                    .child(traffic_changed_at),
                            ),
                    )
                    .child(lucide_icon(
                        include_bytes!("../../assets/icons/activity.svg"),
                        17.0,
                        traffic_palette.foreground.rgba_hex(),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .px_2()
                            .py(px(2.0))
                            .rounded_full()
                            .bg(rgba(traffic_palette.background.rgba_hex()))
                            .text_size(px(10.0))
                            .line_height(px(14.0))
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(traffic_palette.foreground.rgba_hex()))
                            .child(traffic_state_label),
                    )
                    .child(
                        div()
                            .font_family("monospace")
                            .text_xs()
                            .text_color(rgba(COLOR_MUTED))
                            .child(traffic_reason),
                    ),
            )
            .child(
                div()
                    .mt_3()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(meta_item(
                        t(self.locale, "traffic.session.reason"),
                        overview
                            .traffic_session
                            .reason_code
                            .clone()
                            .unwrap_or_else(|| {
                                t(self.locale, "traffic.session.no_reason").to_string()
                            }),
                    ))
                    .child(meta_item(
                        t(self.locale, "traffic.session.failure"),
                        traffic_failure,
                    )),
            )
            .child(
                div()
                    .mt_4()
                    .mb_2()
                    .text_xs()
                    .font_weight(gpui::FontWeight(600.0))
                    .text_color(rgba(COLOR_TEXT_SECONDARY))
                    .child(t(self.locale, "traffic.session.validation.title").to_string()),
            );
        if let Some(observed_at) = overview.traffic_session.validation_observed_at_unix_ms {
            if observed_at > 0 {
                traffic_card =
                    traffic_card.child(div().mb_2().text_xs().text_color(rgba(COLOR_MUTED)).child(
                        tr(
                            self.locale,
                            "traffic.session.validation.observed_at",
                            &[(
                                "time",
                                &format_diagnostic_timestamp(self.locale, observed_at),
                            )],
                        ),
                    ));
            }
        }
        if overview.traffic_session.validation.is_empty() {
            traffic_card = traffic_card.child(
                div()
                    .mb_2()
                    .text_xs()
                    .text_color(rgba(COLOR_MUTED))
                    .child(t(self.locale, "traffic.session.validation.none").to_string()),
            );
        } else {
            traffic_card = traffic_card.child(
                div().mb_2().flex().flex_wrap().gap_2().children(
                    overview
                        .traffic_session
                        .validation
                        .iter()
                        .map(|validation| {
                            let validation_palette = semantic_palette(if validation.passed {
                                SemanticState::Success
                            } else {
                                SemanticState::Danger
                            });
                            div()
                                .px_2()
                                .py(px(2.0))
                                .rounded_full()
                                .bg(rgba(validation_palette.background.rgba_hex()))
                                .text_size(px(10.0))
                                .line_height(px(14.0))
                                .font_weight(gpui::FontWeight(600.0))
                                .text_color(rgba(validation_palette.foreground.rgba_hex()))
                                .child(format!(
                                    "{}: {}",
                                    validation.label,
                                    t(
                                        self.locale,
                                        if validation.passed {
                                            "traffic.session.validation.passed"
                                        } else {
                                            "traffic.session.validation.failed"
                                        },
                                    )
                                ))
                        }),
                ),
            );
        }
        traffic_card = traffic_card.child(
            div()
                .mt_2()
                .mb_2()
                .text_xs()
                .font_weight(gpui::FontWeight(600.0))
                .text_color(rgba(COLOR_TEXT_SECONDARY))
                .child(t(self.locale, "traffic.session.events.title").to_string()),
        );
        if overview.traffic_session.events.is_empty() {
            traffic_card = traffic_card.child(
                div()
                    .mb_2()
                    .text_xs()
                    .text_color(rgba(COLOR_MUTED))
                    .child(t(self.locale, "traffic.session.events.empty").to_string()),
            );
        } else {
            traffic_card = traffic_card.child(
                div().flex().flex_col().children(
                    overview
                        .traffic_session
                        .events
                        .iter()
                        .enumerate()
                        .map(|(index, event)| {
                            let event_palette =
                                semantic_palette(traffic_protection_semantic(event.state));
                            let mut event_row = div()
                                .flex()
                                .items_start()
                                .gap_2()
                                .py_2()
                                .text_xs()
                                .text_color(rgba(COLOR_TEXT_SECONDARY));
                            if index > 0 {
                                event_row =
                                    event_row.border_t_1().border_color(rgba(COLOR_DIVIDER));
                            }
                            let mut event_details = div()
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .min_w(px(0.0))
                                .child(
                                    div()
                                        .font_weight(gpui::FontWeight(600.0))
                                        .child(event.kind_label.clone()),
                                )
                                .child(div().text_xs().text_color(rgba(COLOR_MUTED)).child(
                                    format!(
                                        "{} · {}",
                                        event.state_label,
                                        format_diagnostic_timestamp(
                                            self.locale,
                                            event.occurred_at_unix_ms
                                        )
                                    ),
                                ));
                            if let Some(failure) = &event.failure_label {
                                event_details = event_details.child(
                                    div()
                                        .font_family("monospace")
                                        .text_xs()
                                        .text_color(rgba(COLOR_MUTED))
                                        .child(format!(
                                            "{}: {failure}",
                                            t(self.locale, "traffic.session.failure")
                                        )),
                                );
                            }
                            if let Some(reason_code) = &event.reason_code {
                                event_details = event_details.child(
                                    div()
                                        .font_family("monospace")
                                        .text_xs()
                                        .text_color(rgba(COLOR_MUTED))
                                        .child(reason_code.clone()),
                                );
                            }
                            event_row
                                .child(
                                    gpui::svg()
                                        .data(include_bytes!("../../assets/icons/circle-dot.svg"))
                                        .w(px(14.0))
                                        .h(px(14.0))
                                        .text_color(rgba(event_palette.foreground.rgba_hex())),
                                )
                                .child(event_details)
                        }),
                ),
            );
        }

        let mut stack = div()
            .id("overview-stack")
            .flex()
            .flex_col()
            .gap_4()
            .max_w(px(980.0))
            .child(primary_card)
            .child(traffic_card);

        if !overview.issues.is_empty() {
            stack = stack.child(
                surface_card()
                    .id("overview-issues")
                    .flex()
                    .flex_col()
                    .p_4()
                    .child(card_heading(
                        t(self.locale, "overview.need_attention").to_string(),
                        lucide_icon(
                            include_bytes!("../../assets/icons/alert-triangle.svg"),
                            17.0,
                            COLOR_MUTED,
                        ),
                    ))
                    .children(overview.issues.iter().map(|issue| {
                        div()
                            .flex()
                            .items_start()
                            .gap_2()
                            .py_2()
                            .text_size(px(12.0))
                            .child(lucide_icon(
                                include_bytes!("../../assets/icons/alert-triangle.svg"),
                                15.0,
                                COLOR_WARNING,
                            ))
                            .child(
                                div()
                                    .text_color(rgba(COLOR_TEXT_SECONDARY))
                                    .child(issue.clone()),
                            )
                    })),
            );
        }

        let network_rows = if overview.networks.is_empty() {
            vec![empty_state(t(self.locale, "common.no_running_networks")).into_any_element()]
        } else {
            overview
                .networks
                .iter()
                .enumerate()
                .map(|(index, network)| {
                    let network_palette = semantic_palette(match network.status {
                        NodeNetworkStatus::Online => SemanticState::Success,
                        NodeNetworkStatus::Degraded | NodeNetworkStatus::Connecting => {
                            SemanticState::Warning
                        }
                        NodeNetworkStatus::Failed => SemanticState::Danger,
                        NodeNetworkStatus::Offline | NodeNetworkStatus::Stopping => {
                            SemanticState::Neutral
                        }
                    });
                    let icon_color = match network.status {
                        NodeNetworkStatus::Online => colors.success.rgba_hex(),
                        NodeNetworkStatus::Degraded | NodeNetworkStatus::Connecting => {
                            colors.warning.rgba_hex()
                        }
                        NodeNetworkStatus::Failed => colors.danger.rgba_hex(),
                        NodeNetworkStatus::Offline | NodeNetworkStatus::Stopping => COLOR_FAINT,
                    };
                    div()
                        .id(SharedString::from(format!(
                            "overview-network-{}",
                            network.name
                        )))
                        .flex()
                        .items_center()
                        .gap_3()
                        .py_2()
                        .when(index == 0, |row| {
                            row.debug_selector(|| "visual-network-first".into())
                        })
                        .when(index > 0, |row| {
                            row.debug_selector(|| "visual-network-second".into())
                        })
                        .child(lucide_icon(
                            include_bytes!("../../assets/icons/circle-dot.svg"),
                            15.0,
                            icon_color,
                        ))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w(px(0.0))
                                .gap(px(2.0))
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .line_height(px(18.0))
                                        .font_weight(gpui::FontWeight(600.0))
                                        .text_color(rgba(COLOR_TEXT))
                                        .child(network.name.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .line_height(px(16.57))
                                        .text_color(rgba(COLOR_MUTED))
                                        .child(tr(
                                            self.locale,
                                            "overview.network.detail",
                                            &[
                                                ("peers", &network.peer_count.to_string()),
                                                ("tun", &network.tun_name),
                                            ],
                                        )),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .line_height(px(16.57))
                                        .text_color(rgba(COLOR_MUTED))
                                        .child(tr(
                                            self.locale,
                                            "overview.network.path",
                                            &[
                                                ("routing", &network.routing),
                                                ("exit", &network.exit),
                                            ],
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(5.0))
                                .rounded_full()
                                .px(px(7.0))
                                .py(px(2.0))
                                .text_size(px(10.0))
                                .font_weight(gpui::FontWeight(600.0))
                                .text_color(rgba(network_palette.foreground.rgba_hex()))
                                .bg(rgba(network_palette.background.rgba_hex()))
                                .child(
                                    div()
                                        .w(px(6.0))
                                        .h(px(6.0))
                                        .rounded_full()
                                        .bg(rgba(network_palette.foreground.rgba_hex())),
                                )
                                .child(
                                    t(self.locale, network_status_key(network.status)).to_string(),
                                ),
                        )
                        .into_any_element()
                })
                .collect::<Vec<_>>()
        };
        stack = stack.child(
            surface_card()
                .id("overview-networks")
                .debug_selector(|| "visual-networks".into())
                .flex()
                .flex_col()
                .p_4()
                .child(
                    div()
                        .debug_selector(|| "visual-networks-heading".into())
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .mb_3()
                        .child(
                            div()
                                .text_size(px(14.0))
                                .line_height(px(21.7))
                                .font_weight(gpui::FontWeight(600.0))
                                .text_color(rgba(COLOR_TEXT))
                                .child(t(self.locale, "overview.networks_title").to_string()),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .line_height(px(18.0))
                                .text_color(rgba(COLOR_MUTED))
                                .child(overview.networks.len().to_string()),
                        ),
                )
                .children(network_rows),
        );

        let activity_rows = if overview.activity.is_empty() {
            vec![empty_state(t(self.locale, "overview.activity.empty"))
                .debug_selector(|| "visual-activity-empty".into())
                .into_any_element()]
        } else {
            overview
                .activity
                .iter()
                .enumerate()
                .map(|(index, activity)| {
                    div()
                        .flex()
                        .items_start()
                        .gap_3()
                        .py_3()
                        .when(index > 0, |row| row)
                        .child(lucide_icon(
                            include_bytes!("../../assets/icons/circle-dot.svg"),
                            14.0,
                            COLOR_ACCENT,
                        ))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .min_w(px(0.0))
                                .gap(px(2.0))
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .line_height(px(18.0))
                                        .font_weight(gpui::FontWeight(600.0))
                                        .text_color(rgba(COLOR_TEXT))
                                        .child(activity.action.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .line_height(px(16.0))
                                        .text_color(rgba(COLOR_MUTED))
                                        .child(activity.detail.clone()),
                                )
                                .children(activity.command.as_ref().map(|command| {
                                    div()
                                        .mt(px(2.0))
                                        .max_w(px(720.0))
                                        .overflow_hidden()
                                        .rounded(px(4.0))
                                        .px(px(6.0))
                                        .py(px(2.0))
                                        .bg(rgba(COLOR_PANEL_RAISED))
                                        .text_size(px(10.0))
                                        .line_height(px(14.5))
                                        .text_color(rgba(COLOR_TEXT_SECONDARY))
                                        .child(command.clone())
                                })),
                        )
                        .into_any_element()
                })
                .collect::<Vec<_>>()
        };
        stack = stack.child(
            surface_card()
                .id("overview-activity")
                .debug_selector(|| "visual-activity".into())
                .flex()
                .flex_col()
                .p_4()
                .child(
                    card_heading(
                        t(self.locale, "overview.activity_title").to_string(),
                        lucide_icon(
                            include_bytes!("../../assets/icons/activity.svg"),
                            17.0,
                            COLOR_MUTED,
                        ),
                    )
                    .debug_selector(|| "visual-activity-heading".into()),
                )
                .children(activity_rows),
        );

        let shortcut_cards = overview
            .shortcuts
            .iter()
            .map(|shortcut| {
                let enabled = shortcut.action_id.is_some();
                let mut card = div()
                    .id(SharedString::from(format!(
                        "overview-shortcut-{}",
                        shortcut.id
                    )))
                    .flex()
                    .flex_1()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(2.0))
                    .min_w(px(0.0))
                    .min_h(px(48.0))
                    .px_3()
                    .py_2()
                    .rounded(px(9.0))
                    .bg(rgba(colors.surface.rgba_hex()))
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .font_weight(gpui::FontWeight(600.0))
                    .text_color(rgba(COLOR_TEXT_SECONDARY))
                    .when(shortcut.id == "create_network", |card| {
                        card.debug_selector(|| "visual-shortcut-first".into())
                    })
                    .opacity(if enabled { 1.0 } else { 0.5 })
                    .child(shortcut.label.clone());
                if let Some(reason) = &shortcut.unavailable_reason {
                    card = card.child(
                        div()
                            .text_size(px(10.0))
                            .line_height(px(12.5))
                            .font_weight(gpui::FontWeight(400.0))
                            .text_color(rgba(COLOR_FAINT))
                            .child(reason.clone()),
                    );
                }
                if let Some(action_id) = shortcut.action_id.clone() {
                    card = card.on_click(cx.listener(move |shell, _, _, cx| {
                        shell.open_action("networks", &action_id, cx);
                    }));
                }
                card.into_any_element()
            })
            .collect::<Vec<_>>();
        stack = stack.child(
            surface_card()
                .id("overview-shortcuts")
                .debug_selector(|| "visual-shortcuts".into())
                .flex()
                .flex_col()
                .p_4()
                .child(
                    card_heading(
                        t(self.locale, "overview.shortcuts").to_string(),
                        lucide_icon(
                            include_bytes!("../../assets/icons/waypoints.svg"),
                            17.0,
                            COLOR_MUTED,
                        ),
                    )
                    .debug_selector(|| "visual-shortcuts-heading".into()),
                )
                .child(
                    div()
                        .debug_selector(|| "visual-shortcuts-grid".into())
                        .flex()
                        .gap_2()
                        .children(shortcut_cards),
                ),
        );

        stack.into_any_element()
    }

    fn configuration_workspace(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let document = self.configuration.document.clone();
        let preview = self.configuration.preview.clone();
        let history = self.configuration.history.clone();
        let applied = self.configuration.applied.clone();
        let validation_issues = self.configuration.validation_issues.clone();
        let active_issue_path = self.configuration.active_issue_path.clone();
        let selected_audit = self.configuration.selected_audit.clone();
        let imported_from = self.configuration.imported_from.clone();
        let candidate = self.configuration.candidate_json();
        let show_candidate = self.configuration.show_candidate;
        let confirm_apply = self.configuration.confirm_apply;
        let rollback_target = self.configuration.rollback_target;
        let error = self.configuration.error.clone();
        let notice = self.configuration.notice.clone();
        let operation = self.configuration.operation;
        let dirty = self.configuration.is_dirty();
        let can_apply = self.configuration.can_apply();
        let apply_block = t(self.locale, self.configuration.apply_block_key()).to_string();
        let busy = self.pending_commands > 0 || operation.is_some();
        let load_available = self.configuration_available("get_configuration");
        let history_available = self.configuration_available("get_configuration_history");
        let preview_available = self.configuration_available("preview_configuration");
        let apply_available = self.configuration_available("apply_configuration");
        let rollback_available = self.configuration_available("rollback_configuration");
        let apply_permission_block = self
            .configuration_action_blocked("apply_configuration")
            .map(|key| t(self.locale, key).to_string());
        let rollback_permission_block = self
            .configuration_action_blocked("rollback_configuration")
            .map(|key| t(self.locale, key).to_string());
        let editor = self.configuration_editor.clone();
        let current_revision = document.as_ref().map(|document| document.revision);
        let revision_label = document
            .as_ref()
            .map(|document| {
                tr(
                    self.locale,
                    "configuration.revision",
                    &[("revision", &document.revision.to_string())],
                )
            })
            .unwrap_or_else(|| t(self.locale, "configuration.loading").to_string());

        let reload_button = button(
            SharedString::from("configuration-reload"),
            t(self.locale, "configuration.reload_latest"),
            ButtonVariant::Secondary,
            busy || !load_available,
        )
        .on_click(cx.listener(|shell, _, _, cx| shell.load_configuration(cx)));

        let mut heading = div()
            .id("configuration-heading")
            .flex()
            .flex_wrap()
            .items_center()
            .gap_4()
            .p_5()
            .rounded_lg()
            .bg(rgba(COLOR_PANEL))
            .child(
                div()
                    .w(px(46.0))
                    .h(px(46.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .bg(rgba(DARK_THEME.colors.accent_soft.rgba_hex()))
                    .text_color(rgba(COLOR_ACCENT))
                    .child("{ }"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(260.0))
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(10.0))
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(COLOR_MUTED))
                            .child(
                                t(self.locale, "configuration.eyebrow")
                                    .to_string()
                                    .to_uppercase(),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(20.0))
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(COLOR_TEXT))
                            .child(t(self.locale, "configuration.title").to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgba(COLOR_MUTED))
                            .child(t(self.locale, "configuration.description").to_string()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .bg(rgba(COLOR_PANEL_RAISED))
                            .font_family("monospace")
                            .text_xs()
                            .text_color(rgba(COLOR_MUTED))
                            .child(revision_label),
                    ),
            );
        if dirty {
            heading = heading.child(sync_badge(
                "configuration-dirty",
                t(self.locale, "configuration.unsaved").to_string(),
            ));
        }
        heading = heading.child(reload_button);

        let mut editor_card =
            div()
                .id("configuration-editor-card")
                .flex()
                .flex_col()
                .min_w(px(0.0))
                .p_5()
                .rounded_lg()
                .bg(rgba(COLOR_PANEL))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_start()
                        .justify_between()
                        .gap_3()
                        .mb_4()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .font_weight(gpui::FontWeight(600.0))
                                        .text_color(rgba(COLOR_MUTED))
                                        .child(
                                            t(self.locale, "configuration.editor.eyebrow")
                                                .to_string()
                                                .to_uppercase(),
                                        ),
                                )
                                .child(div().text_base().text_color(rgba(COLOR_TEXT)).child(
                                    t(self.locale, "configuration.editor.title").to_string(),
                                )),
                        )
                        .child(
                            button(
                                SharedString::from("configuration-import"),
                                t(self.locale, "configuration.import"),
                                ButtonVariant::Secondary,
                                busy,
                            )
                            .on_click(
                                cx.listener(|shell, _, _, cx| shell.import_configuration_file(cx)),
                            ),
                        ),
                );
        if let Some(source) = imported_from {
            editor_card = editor_card.child(
                div()
                    .mb_2()
                    .text_xs()
                    .text_color(rgba(COLOR_ACCENT))
                    .child(tr(
                        self.locale,
                        "configuration.imported_from",
                        &[("file", &source)],
                    )),
            );
        }
        let editor_body = if let Some(editor) = editor {
            div()
                .w_full()
                .h(px(470.0))
                .overflow_hidden()
                .rounded_md()
                .bg(rgba(DARK_THEME.colors.input_background.rgba_hex()))
                .flex()
                .child(Textarea::new(&editor))
                .into_any_element()
        } else {
            div()
                .w_full()
                .h(px(180.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .text_sm()
                .text_color(rgba(COLOR_MUTED))
                .child(t(self.locale, "configuration.loading").to_string())
                .into_any_element()
        };
        editor_card = editor_card
            .child(editor_body)
            .child(
                div()
                    .mt_2()
                    .text_xs()
                    .text_color(rgba(COLOR_MUTED))
                    .child(t(self.locale, "configuration.editor.help").to_string()),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .mt_4()
                    .pt_4()
                    .child(
                        div().text_xs().text_color(rgba(COLOR_MUTED)).child(
                            t(self.locale, "configuration.editor.networks_only").to_string(),
                        ),
                    )
                    .child(
                        button(
                            SharedString::from("configuration-preview"),
                            t(self.locale, "configuration.preview"),
                            ButtonVariant::Primary,
                            busy || document.is_none() || !preview_available,
                        )
                        .on_click(
                            cx.listener(|shell, _, _, cx| shell.preview_configuration_draft(cx)),
                        ),
                    ),
            );

        let mut review_card =
            div()
                .id("configuration-review-card")
                .flex()
                .flex_col()
                .min_w(px(0.0))
                .p_5()
                .rounded_lg()
                .bg(rgba(COLOR_PANEL))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_start()
                        .justify_between()
                        .gap_3()
                        .mb_4()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .font_weight(gpui::FontWeight(600.0))
                                        .text_color(rgba(COLOR_MUTED))
                                        .child(
                                            t(self.locale, "configuration.review.eyebrow")
                                                .to_string()
                                                .to_uppercase(),
                                        ),
                                )
                                .child(div().text_base().text_color(rgba(COLOR_TEXT)).child(
                                    t(self.locale, "configuration.review.title").to_string(),
                                )),
                        )
                        .when_some(preview.as_ref(), |this, preview| {
                            this.child(
                                div()
                                    .rounded_full()
                                    .px_2()
                                    .py_1()
                                    .bg(rgba(COLOR_PANEL_RAISED))
                                    .text_xs()
                                    .text_color(rgba(COLOR_MUTED))
                                    .child(tr(
                                        self.locale,
                                        "configuration.diff_count",
                                        &[("count", &preview.diffs.len().to_string())],
                                    )),
                            )
                        }),
                );

        if let Some(preview) = preview.as_ref() {
            if preview.revision_conflict {
                review_card = review_card.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p_3()
                        .mb_3()
                        .rounded_md()
                        .bg(rgba(DARK_THEME.colors.warning_soft.rgba_hex()))
                        .child(
                            div()
                                .font_weight(gpui::FontWeight(600.0))
                                .text_sm()
                                .text_color(rgba(COLOR_WARNING))
                                .child(
                                    t(self.locale, "configuration.revision_conflict").to_string(),
                                ),
                        )
                        .child(div().text_xs().text_color(rgba(COLOR_WARNING)).child(tr(
                            self.locale,
                            "configuration.revision_conflict_detail",
                            &[
                                ("expected", &preview.expected_revision.to_string()),
                                ("current", &preview.current_revision.to_string()),
                            ],
                        ))),
                );
            }

            if !validation_issues.is_empty() {
                let issues = validation_issues
                    .iter()
                    .enumerate()
                    .map(|(index, issue)| {
                        let path = issue.path.clone();
                        let active = active_issue_path.as_deref() == Some(issue.path.as_str());
                        div()
                            .id(SharedString::from(format!("configuration-issue-{index}")))
                            .role(Role::Button)
                            .aria_label(format!("{} · {}", issue.path, issue.message))
                            .focusable()
                            .tab_stop(true)
                            .flex()
                            .flex_col()
                            .gap_1()
                            .p_3()
                            .rounded_md()
                            .bg(rgba(if active {
                                DARK_THEME.colors.danger_soft.rgba_hex()
                            } else {
                                COLOR_PANEL_RAISED
                            }))
                            .on_click(cx.listener(move |shell, _, window, cx| {
                                shell.focus_configuration_issue(path.clone(), window, cx)
                            }))
                            .child(
                                div()
                                    .font_family("monospace")
                                    .text_xs()
                                    .text_color(rgba(DARK_THEME.colors.danger_text.rgba_hex()))
                                    .child(issue.path.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgba(COLOR_MUTED))
                                    .child(format!("{} · {}", issue.code, issue.message)),
                            )
                            .into_any_element()
                    })
                    .collect::<Vec<_>>();
                review_card = review_card.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .mb_4()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight(600.0))
                                .text_color(rgba(COLOR_TEXT))
                                .child(tr(
                                    self.locale,
                                    "configuration.validation.title",
                                    &[("count", &validation_issues.len().to_string())],
                                )),
                        )
                        .children(issues),
                );
            }

            let diffs = preview
                .diffs
                .iter()
                .enumerate()
                .map(|(index, diff)| {
                    let impact = match diff.impact {
                        ConfigurationImpact::Immediate => {
                            t(self.locale, "configuration.impact.immediate").to_string()
                        }
                        ConfigurationImpact::RestartRequired => {
                            t(self.locale, "configuration.impact.restart_required").to_string()
                        }
                    };
                    let before = serde_json::to_string_pretty(&diff.before)
                        .unwrap_or_else(|_| "null".to_string());
                    let after = serde_json::to_string_pretty(&diff.after)
                        .unwrap_or_else(|_| "null".to_string());
                    div()
                        .id(SharedString::from(format!("configuration-diff-{index}")))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .p_3()
                        .rounded_md()
                        .bg(rgba(COLOR_PANEL_RAISED))
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    div()
                                        .font_family("monospace")
                                        .text_xs()
                                        .text_color(rgba(COLOR_TEXT))
                                        .child(diff.path.clone()),
                                )
                                .child(
                                    div()
                                        .rounded_full()
                                        .px_2()
                                        .py_1()
                                        .bg(rgba(
                                            if diff.impact == ConfigurationImpact::Immediate {
                                                DARK_THEME.colors.success_soft.rgba_hex()
                                            } else {
                                                DARK_THEME.colors.warning_soft.rgba_hex()
                                            },
                                        ))
                                        .text_size(px(10.0))
                                        .text_color(rgba(
                                            if diff.impact == ConfigurationImpact::Immediate {
                                                DARK_THEME.colors.success.rgba_hex()
                                            } else {
                                                COLOR_WARNING
                                            },
                                        ))
                                        .child(impact),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_3()
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .flex_1()
                                        .min_w(px(120.0))
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_size(px(10.0))
                                                .text_color(rgba(COLOR_MUTED))
                                                .child(
                                                    t(self.locale, "configuration.before")
                                                        .to_string(),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .font_family("monospace")
                                                .text_xs()
                                                .text_color(rgba(COLOR_MUTED))
                                                .child(before),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .flex_1()
                                        .min_w(px(120.0))
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_size(px(10.0))
                                                .text_color(rgba(COLOR_MUTED))
                                                .child(
                                                    t(self.locale, "configuration.after")
                                                        .to_string(),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .font_family("monospace")
                                                .text_xs()
                                                .text_color(rgba(COLOR_TEXT))
                                                .child(after),
                                        ),
                                ),
                        )
                        .into_any_element()
                })
                .collect::<Vec<_>>();
            review_card = review_card.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .mb_4()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(COLOR_TEXT))
                            .child(t(self.locale, "configuration.diffs.title").to_string()),
                    )
                    .children(diffs),
            );

            if preview.diffs.is_empty() {
                review_card = review_card.child(
                    div()
                        .mb_4()
                        .text_xs()
                        .text_color(rgba(COLOR_MUTED))
                        .child(t(self.locale, "configuration.no_changes").to_string()),
                );
            }

            if let Some(candidate) = candidate {
                review_card = review_card.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .mb_4()
                        .child(
                            button(
                                SharedString::from("configuration-toggle-candidate"),
                                t(self.locale, "configuration.candidate.title"),
                                ButtonVariant::Ghost,
                                false,
                            )
                            .on_click(cx.listener(
                                |shell, _, _, cx| {
                                    shell.configuration.show_candidate =
                                        !shell.configuration.show_candidate;
                                    cx.notify();
                                },
                            )),
                        )
                        .when(show_candidate, |this| {
                            this.child(
                                div()
                                    .id("configuration-candidate")
                                    .max_h(px(260.0))
                                    .overflow_y_scroll()
                                    .p_3()
                                    .rounded_md()
                                    .bg(rgba(DARK_THEME.colors.input_background.rgba_hex()))
                                    .font_family("monospace")
                                    .text_xs()
                                    .text_color(rgba(COLOR_MUTED))
                                    .child(candidate.clone()),
                            )
                        }),
                );
            }

            let mut apply_bar = div()
                .flex()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_3()
                .pt_4()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight(600.0))
                                .text_color(rgba(if can_apply {
                                    DARK_THEME.colors.success.rgba_hex()
                                } else {
                                    COLOR_WARNING
                                }))
                                .child(apply_block),
                        )
                        .when_some(apply_permission_block.clone(), |this, reason| {
                            this.child(
                                div()
                                    .text_xs()
                                    .text_color(rgba(COLOR_WARNING))
                                    .child(reason),
                            )
                        }),
                );

            if confirm_apply {
                apply_bar = apply_bar.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(div().text_xs().text_color(rgba(COLOR_WARNING)).child(tr(
                            self.locale,
                            "configuration.apply.confirm_detail",
                            &[
                                ("count", &preview.diffs.len().to_string()),
                                (
                                    "revision",
                                    &preview.candidate_revision.saturating_sub(1).to_string(),
                                ),
                            ],
                        )))
                        .child(
                            button(
                                SharedString::from("configuration-confirm-apply"),
                                t(self.locale, "configuration.apply.confirm_action"),
                                ButtonVariant::Danger,
                                busy || !can_apply || !apply_available,
                            )
                            .on_click(
                                cx.listener(|shell, _, _, cx| shell.apply_configuration_draft(cx)),
                            ),
                        )
                        .child(
                            button(
                                SharedString::from("configuration-cancel-apply"),
                                t(self.locale, "common.cancel"),
                                ButtonVariant::Ghost,
                                busy,
                            )
                            .on_click(cx.listener(
                                |shell, _, _, cx| {
                                    shell.configuration.confirm_apply = false;
                                    cx.notify();
                                },
                            )),
                        ),
                );
            } else {
                apply_bar = apply_bar.child(
                    button(
                        SharedString::from("configuration-apply"),
                        t(self.locale, "configuration.apply"),
                        ButtonVariant::Primary,
                        busy || !can_apply || !apply_available,
                    )
                    .on_click(cx.listener(|shell, _, _, cx| {
                        if shell.configuration.request_apply_confirmation() {
                            cx.notify();
                        }
                    })),
                );
            }
            review_card = review_card.child(apply_bar);
        } else {
            review_card = review_card.child(
                div()
                    .min_h(px(180.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .p_4()
                    .rounded_md()
                    .text_center()
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgba(COLOR_MUTED))
                            .child(t(self.locale, "configuration.review.empty").to_string()),
                    ),
            );
        }

        let status_banner = error
            .as_ref()
            .map(|error| {
                div()
                    .id("configuration-error")
                    .role(Role::Alert)
                    .p_3()
                    .rounded_md()
                    .bg(rgba(DARK_THEME.colors.danger_soft.rgba_hex()))
                    .text_xs()
                    .text_color(rgba(DARK_THEME.colors.danger_text.rgba_hex()))
                    .child(error.clone())
                    .into_any_element()
            })
            .or_else(|| {
                notice.as_ref().map(|notice| {
                    div()
                        .id("configuration-notice")
                        .role(Role::Status)
                        .p_3()
                        .rounded_md()
                        .bg(rgba(DARK_THEME.colors.info_soft.rgba_hex()))
                        .text_xs()
                        .text_color(rgba(DARK_THEME.colors.info.rgba_hex()))
                        .child(notice.clone())
                        .into_any_element()
                })
            });

        let applied_card = applied.map(|applied| {
            let audit = applied.audit.clone();
            div()
                .id("configuration-applied")
                .flex()
                .flex_col()
                .gap_3()
                .p_5()
                .rounded_lg()
                .bg(rgba(COLOR_PANEL))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(rgba(COLOR_MUTED))
                                        .child(
                                            t(self.locale, "configuration.audit.eyebrow")
                                                .to_string()
                                                .to_uppercase(),
                                        ),
                                )
                                .child(div().text_base().text_color(rgba(COLOR_TEXT)).child(
                                    t(self.locale, "configuration.audit.title").to_string(),
                                )),
                        )
                        .child(
                            div()
                                .rounded_full()
                                .px_2()
                                .py_1()
                                .bg(rgba(DARK_THEME.colors.success_soft.rgba_hex()))
                                .text_xs()
                                .text_color(rgba(DARK_THEME.colors.success.rgba_hex()))
                                .child(
                                    t(
                                        self.locale,
                                        if applied.no_op {
                                            "configuration.applied.unchanged"
                                        } else {
                                            "configuration.applied.changed"
                                        },
                                    )
                                    .to_string(),
                                ),
                        ),
                )
                .when_some(audit, |this, audit| {
                    this.child(configuration_audit_details(self.locale, &audit))
                })
                .when(applied.audit.is_none(), |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(rgba(COLOR_MUTED))
                            .child(t(self.locale, "configuration.audit.unavailable").to_string()),
                    )
                })
                .into_any_element()
        });

        let history_rows = history
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let entry = entry.clone();
                let transaction_id = entry.transaction_id.clone();
                let audit_open = selected_audit.as_deref() == Some(entry.transaction_id.as_str());
                let target_confirmation = rollback_target == Some(entry.revision);
                let is_current = current_revision == Some(entry.revision);
                let action_label = match entry.action {
                    ConfigurationChangeAction::Apply => {
                        t(self.locale, "configuration.history.action.apply")
                    }
                    ConfigurationChangeAction::Rollback => {
                        t(self.locale, "configuration.history.action.rollback")
                    }
                };
                let rollback_target_revision = entry.revision;
                let details_entry = entry.clone();
                let mut row = div()
                    .id(SharedString::from(format!("configuration-history-{index}")))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .rounded_md()
                    .bg(rgba(COLOR_PANEL_RAISED))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .font_weight(gpui::FontWeight(600.0))
                                            .text_sm()
                                            .text_color(rgba(COLOR_TEXT))
                                            .child(tr(
                                                self.locale,
                                                "configuration.history.revision",
                                                &[("revision", &entry.revision.to_string())],
                                            )),
                                    )
                                    .child(div().text_xs().text_color(rgba(COLOR_MUTED)).child(
                                        format!(
                                                "{action_label}{}",
                                                entry
                                                    .target_revision
                                                    .map(|target| tr(
                                                        self.locale,
                                                        "configuration.history.target_value",
                                                        &[("revision", &target.to_string())],
                                                    ))
                                                    .unwrap_or_default()
                                            ),
                                    )),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgba(COLOR_MUTED))
                                    .child(format!("{} · {}", entry.actor, entry.source)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .font_family("monospace")
                            .text_color(rgba(COLOR_MUTED))
                            .child(entry.occurred_at.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                button(
                                    SharedString::from(format!(
                                        "configuration-history-details-{index}"
                                    )),
                                    t(self.locale, "configuration.history.details"),
                                    ButtonVariant::Ghost,
                                    false,
                                )
                                .on_click(cx.listener(
                                    move |shell, _, _, cx| {
                                        shell.configuration.select_audit(transaction_id.clone());
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                button(
                                    SharedString::from(format!(
                                        "configuration-history-rollback-{index}"
                                    )),
                                    if is_current {
                                        t(self.locale, "configuration.rollback.current")
                                    } else {
                                        t(self.locale, "configuration.rollback.action")
                                    },
                                    ButtonVariant::Secondary,
                                    busy || is_current
                                        || !rollback_available
                                        || rollback_target.is_some(),
                                )
                                .on_click(cx.listener(
                                    move |shell, _, _, cx| {
                                        if shell
                                            .configuration
                                            .request_rollback_confirmation(rollback_target_revision)
                                        {
                                            cx.notify();
                                        }
                                    },
                                )),
                            ),
                    );
                if audit_open {
                    row = row.child(configuration_audit_details(self.locale, &details_entry));
                }
                if target_confirmation {
                    row = row.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .p_3()
                            .rounded_md()
                            .bg(rgba(DARK_THEME.colors.warning_soft.rgba_hex()))
                            .child(div().text_xs().text_color(rgba(COLOR_WARNING)).child(tr(
                                self.locale,
                                "configuration.rollback.confirm_detail",
                                &[
                                    ("current", &current_revision.unwrap_or_default().to_string()),
                                    ("target", &entry.revision.to_string()),
                                ],
                            )))
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(
                                        button(
                                            SharedString::from(format!(
                                                "configuration-confirm-rollback-{index}"
                                            )),
                                            t(self.locale, "configuration.rollback.confirm_action"),
                                            ButtonVariant::Danger,
                                            busy || !rollback_available,
                                        )
                                        .on_click(
                                            cx.listener(move |shell, _, _, cx| {
                                                shell.rollback_configuration_to(entry.revision, cx)
                                            }),
                                        ),
                                    )
                                    .child(
                                        button(
                                            SharedString::from(format!(
                                                "configuration-cancel-rollback-{index}"
                                            )),
                                            t(self.locale, "common.cancel"),
                                            ButtonVariant::Ghost,
                                            busy,
                                        )
                                        .on_click(
                                            cx.listener(|shell, _, _, cx| {
                                                shell.configuration.rollback_target = None;
                                                cx.notify();
                                            }),
                                        ),
                                    ),
                            ),
                    );
                }
                row.into_any_element()
            })
            .collect::<Vec<_>>();

        let history_card =
            div()
                .id("configuration-history")
                .flex()
                .flex_col()
                .gap_3()
                .p_5()
                .rounded_lg()
                .bg(rgba(COLOR_PANEL))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_start()
                        .justify_between()
                        .gap_3()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(rgba(COLOR_MUTED))
                                        .child(
                                            t(self.locale, "configuration.history.eyebrow")
                                                .to_string()
                                                .to_uppercase(),
                                        ),
                                )
                                .child(div().text_base().text_color(rgba(COLOR_TEXT)).child(
                                    t(self.locale, "configuration.history.title").to_string(),
                                )),
                        )
                        .child(
                            button(
                                SharedString::from("configuration-history-refresh"),
                                t(self.locale, "configuration.history.refresh"),
                                ButtonVariant::Secondary,
                                busy || !history_available,
                            )
                            .on_click(
                                cx.listener(|shell, _, _, cx| shell.load_configuration_history(cx)),
                            ),
                        ),
                )
                .when_some(rollback_permission_block, |this, reason| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(rgba(COLOR_WARNING))
                            .child(reason),
                    )
                })
                .when(history_rows.is_empty(), |this| {
                    this.child(
                        div()
                            .min_h(px(120.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .text_xs()
                            .text_color(rgba(COLOR_MUTED))
                            .child(t(self.locale, "configuration.history.empty").to_string()),
                    )
                })
                .children(history_rows);

        let mut workspace = div()
            .id("configuration-workspace")
            .role(Role::Form)
            .aria_label(t(self.locale, "configuration.title").to_string())
            .flex()
            .flex_col()
            .gap_4()
            .mt_5()
            .child(heading)
            .when_some(status_banner, |this, status| this.child(status))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_start()
                    .gap_4()
                    .child(div().flex_1().min_w(px(360.0)).child(editor_card))
                    .child(div().flex_1().min_w(px(360.0)).child(review_card)),
            )
            .when_some(applied_card, |this, applied| this.child(applied))
            .child(history_card);
        if busy {
            workspace =
                workspace.child(div().text_xs().text_color(rgba(COLOR_MUTED)).child(format!(
                    "{} · {:?}",
                    t(self.locale, "common.operation_in_progress"),
                    operation
                )));
        }
        workspace.into_any_element()
    }

    fn diagnostics_card(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let busy = self.pending_commands > 0;
        let copy_notice = self.diagnostic_copy_notice.clone();
        let run_button = |busy| {
            button(
                SharedString::from("run-node-diagnostics"),
                t(self.locale, "diagnostic.run"),
                ButtonVariant::Secondary,
                busy,
            )
            .on_click(cx.listener(|shell, _, _, cx| {
                shell.diagnostic_copy_notice = None;
                shell.submit_raw(NodeCommand::GetDiagnostics, cx);
            }))
        };

        let Some(diagnostics) = self.recent_diagnostics.clone() else {
            return div()
                .id("runtime-diagnostics")
                .debug_selector(|| "visual-runtime-diagnostics".into())
                .role(Role::Article)
                .aria_label(t(self.locale, "diagnostic.title").to_string())
                .flex()
                .flex_col()
                .gap_3()
                .p_4()
                .rounded_lg()
                .bg(rgba(COLOR_PANEL))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight(600.0))
                                .text_color(rgba(COLOR_TEXT))
                                .child(t(self.locale, "diagnostic.title").to_string()),
                        )
                        .child(run_button(busy)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p_4()
                        .rounded_md()
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgba(COLOR_TEXT_SECONDARY))
                                .child(t(self.locale, "diagnostic.empty_title").to_string()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .line_height(px(18.0))
                                .text_color(rgba(COLOR_MUTED))
                                .child(t(self.locale, "diagnostic.empty_detail").to_string()),
                        ),
                )
                .into_any_element();
        };

        let copy_id_button = button(
            SharedString::from("copy-node-diagnostic-id"),
            t(self.locale, "operation.copy_diagnostic_id"),
            ButtonVariant::Secondary,
            false,
        )
        .on_click(cx.listener(|shell, _, _, cx| {
            if let Some(diagnostics) = &shell.recent_diagnostics {
                cx.write_to_clipboard(ClipboardItem::new_string(diagnostics.diagnostic_id.clone()));
                shell.diagnostic_copy_notice =
                    Some(t(shell.locale, "operation.diagnostic_copied").to_string());
                cx.notify();
            }
        }));
        let copy_report_button = button(
            SharedString::from("copy-node-diagnostic-report"),
            t(self.locale, "diagnostic.copy_report"),
            ButtonVariant::Secondary,
            false,
        )
        .on_click(cx.listener(|shell, _, _, cx| {
            if let Some(diagnostics) = &shell.recent_diagnostics {
                cx.write_to_clipboard(ClipboardItem::new_string(
                    diagnostics.sanitized_report.clone(),
                ));
                shell.diagnostic_copy_notice =
                    Some(t(shell.locale, "diagnostic.report_copied").to_string());
                cx.notify();
            }
        }));

        let duration = diagnostics
            .completed_at_unix_ms
            .saturating_sub(diagnostics.started_at_unix_ms);
        let timeout = t(
            self.locale,
            if diagnostics.timed_out {
                "diagnostic.timed_out_yes"
            } else {
                "diagnostic.timed_out_no"
            },
        )
        .to_string();
        let metadata = div()
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .rounded_md()
            .bg(rgba(COLOR_PANEL_RAISED))
            .child(diagnostic_metadata_row(
                t(self.locale, "diagnostic.id"),
                diagnostics.diagnostic_id.clone(),
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "diagnostic.started_at"),
                format_diagnostic_timestamp(self.locale, diagnostics.started_at_unix_ms),
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "diagnostic.completed_at"),
                format_diagnostic_timestamp(self.locale, diagnostics.completed_at_unix_ms),
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "diagnostic.duration"),
                format_diagnostic_duration(self.locale, duration),
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "diagnostic.timeout"),
                timeout,
            ));

        let checks = diagnostic_checks_in_fixed_order(&diagnostics)
            .into_iter()
            .map(|(layer, check)| {
                let layer_label = t(self.locale, diagnostic_layer_label_key(layer)).to_string();
                if let Some(check) = check {
                    let palette = semantic_palette(diagnostic_state_semantic(check.state));
                    let state_label =
                        t(self.locale, diagnostic_state_label_key(check.state)).to_string();
                    let message = t(self.locale, &check.message_key).to_string();
                    let suggestion = check
                        .suggestion_key
                        .as_deref()
                        .map(|key| t(self.locale, key).to_string())
                        .unwrap_or_else(|| t(self.locale, "diagnostic.no_suggestion").to_string());
                    let error_code = check
                        .error_code
                        .clone()
                        .unwrap_or_else(|| t(self.locale, "common.none").to_string());
                    return div()
                        .id(SharedString::from(format!(
                            "diagnostic-layer-{}",
                            layer.key()
                        )))
                        .debug_selector(move || diagnostic_layer_selector(layer).into())
                        .role(if matches!(check.state, NodeDiagnosticState::Failed) {
                            Role::Alert
                        } else {
                            Role::Status
                        })
                        .aria_label(format!("{layer_label}: {state_label}"))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .p_3()
                        .rounded_md()
                        .bg(rgba(palette.background.rgba_hex()))
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(gpui::FontWeight(600.0))
                                        .text_color(rgba(COLOR_TEXT))
                                        .child(layer_label),
                                )
                                .child(
                                    div()
                                        .px_2()
                                        .py_1()
                                        .rounded_full()
                                        .bg(rgba(COLOR_PANEL_RAISED))
                                        .text_color(rgba(palette.foreground.rgba_hex()))
                                        .text_xs()
                                        .font_weight(gpui::FontWeight(600.0))
                                        .child(state_label),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .line_height(px(18.0))
                                .text_color(rgba(COLOR_TEXT_SECONDARY))
                                .child(message),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(diagnostic_metadata_row(
                                    t(self.locale, "diagnostic.started_at"),
                                    format_diagnostic_timestamp(
                                        self.locale,
                                        check.started_at_unix_ms,
                                    ),
                                ))
                                .child(diagnostic_metadata_row(
                                    t(self.locale, "diagnostic.duration"),
                                    format_diagnostic_duration(self.locale, check.duration_ms),
                                ))
                                .child(diagnostic_metadata_row(
                                    t(self.locale, "diagnostic.error_code"),
                                    error_code,
                                ))
                                .child(diagnostic_metadata_row(
                                    t(self.locale, "diagnostic.suggestion"),
                                    suggestion,
                                )),
                        )
                        .into_any_element();
                }

                let state_label = t(self.locale, "diagnostic.state.not_tested").to_string();
                div()
                    .id(SharedString::from(format!(
                        "diagnostic-layer-{}",
                        layer.key()
                    )))
                    .debug_selector(move || diagnostic_layer_selector(layer).into())
                    .role(Role::Status)
                    .aria_label(format!("{layer_label}: {state_label}"))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .rounded_md()
                    .bg(rgba(COLOR_PANEL_RAISED))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight(600.0))
                                    .text_color(rgba(COLOR_TEXT))
                                    .child(layer_label),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded_full()
                                    .text_xs()
                                    .text_color(rgba(COLOR_TEXT_SECONDARY))
                                    .child(state_label),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgba(COLOR_MUTED))
                            .child(t(self.locale, "diagnostic.check_missing").to_string()),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        let assessment = &diagnostics.exit_health_assessment;
        let health_palette = semantic_palette(exit_health_state_semantic(assessment.state));
        let health_state_label =
            t(self.locale, exit_health_state_label_key(assessment.state)).to_string();
        let health_separator = t(self.locale, "common.list_separator");
        let affected_layers = if assessment.affected_layers.is_empty() {
            t(self.locale, "diagnostic.exit_health.none").to_string()
        } else {
            assessment
                .affected_layers
                .iter()
                .map(|layer| t(self.locale, exit_health_layer_label_key(*layer)).to_string())
                .collect::<Vec<_>>()
                .join(health_separator)
        };
        let affected_targets = if assessment.affected_targets.is_empty() {
            t(self.locale, "diagnostic.exit_health.none").to_string()
        } else {
            assessment
                .affected_targets
                .iter()
                .map(|target| exit_health_target_label(self.locale, target))
                .collect::<Vec<_>>()
                .join(health_separator)
        };
        let affected_protocols = if assessment.affected_protocols.is_empty() {
            t(self.locale, "diagnostic.exit_health.none").to_string()
        } else {
            assessment
                .affected_protocols
                .iter()
                .map(|protocol| t(self.locale, exit_health_protocol_key(*protocol)).to_string())
                .collect::<Vec<_>>()
                .join(health_separator)
        };
        let assessment_source = assessment
            .source
            .map(|source| t(self.locale, exit_health_source_label_key(source)).to_string())
            .unwrap_or_else(|| t(self.locale, "diagnostic.exit_health.none").to_string());
        let assessment_observed = assessment
            .observed_at_unix_ms
            .map(|observed_at| format_diagnostic_timestamp(self.locale, observed_at))
            .unwrap_or_else(|| t(self.locale, "common.not_available").to_string());
        let retest_label = t(
            self.locale,
            exit_health_retest_label_key(diagnostics.exit_health.retest.phase.key()),
        )
        .to_string();
        let health_evidence = diagnostics
            .exit_health
            .evidence
            .iter()
            .map(|evidence| {
                let layer_label =
                    t(self.locale, exit_health_layer_label_key(evidence.layer)).to_string();
                let state_label = t(
                    self.locale,
                    exit_health_evidence_state_label_key(evidence.state),
                )
                .to_string();
                let source_label =
                    t(self.locale, exit_health_source_label_key(evidence.source)).to_string();
                let target_label = evidence
                    .target
                    .as_ref()
                    .map(|target| exit_health_target_label(self.locale, target))
                    .unwrap_or_else(|| t(self.locale, "diagnostic.exit_health.none").to_string());
                let protocol_label = evidence
                    .protocol
                    .map(|protocol| t(self.locale, exit_health_protocol_key(protocol)).to_string())
                    .unwrap_or_else(|| t(self.locale, "diagnostic.exit_health.none").to_string());
                let error_code = evidence
                    .error_code
                    .clone()
                    .unwrap_or_else(|| t(self.locale, "common.none").to_string());
                let selector = format!("visual-diagnostic-exit-health-{}", evidence.layer.key());
                div()
                    .id(SharedString::from(format!(
                        "diagnostic-exit-health-{}",
                        evidence.layer.key()
                    )))
                    .debug_selector(move || selector)
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .rounded_md()
                    .bg(rgba(COLOR_PANEL_RAISED))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(gpui::FontWeight(600.0))
                                    .text_color(rgba(COLOR_TEXT))
                                    .child(layer_label),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgba(COLOR_MUTED))
                                    .child(state_label),
                            ),
                    )
                    .child(diagnostic_metadata_row(
                        t(self.locale, "diagnostic.exit_health.source"),
                        source_label,
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "diagnostic.exit_health.target"),
                        target_label,
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "diagnostic.exit_health.protocol"),
                        protocol_label,
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "diagnostic.exit_health.observed_at"),
                        format_diagnostic_timestamp(self.locale, evidence.observed_at_unix_ms),
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "diagnostic.duration"),
                        format_diagnostic_duration(self.locale, evidence.duration_ms),
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "diagnostic.error_code"),
                        error_code,
                    ))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let exit_health = div()
            .id("diagnostic-exit-health")
            .debug_selector(|| "visual-diagnostic-exit-health".into())
            .role(Role::Status)
            .aria_label(format!(
                "{}: {health_state_label}",
                t(self.locale, "diagnostic.exit_health.title")
            ))
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_md()
            .bg(rgba(health_palette.background.rgba_hex()))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_start()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(gpui::FontWeight(600.0))
                                    .text_color(rgba(COLOR_TEXT))
                                    .child(
                                        t(self.locale, "diagnostic.exit_health.title").to_string(),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .line_height(px(18.0))
                                    .text_color(rgba(COLOR_TEXT_SECONDARY))
                                    .child(
                                        t(self.locale, "diagnostic.exit_health.disclaimer")
                                            .to_string(),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_full()
                            .bg(rgba(COLOR_PANEL_RAISED))
                            .text_xs()
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(health_palette.foreground.rgba_hex()))
                            .child(health_state_label),
                    ),
            )
            .child(diagnostic_metadata_row(
                t(self.locale, "diagnostic.exit_health.default_candidate"),
                t(
                    self.locale,
                    if assessment.default_candidate {
                        "diagnostic.exit_health.yes"
                    } else {
                        "diagnostic.exit_health.no"
                    },
                )
                .to_string(),
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "diagnostic.exit_health.reason"),
                assessment.reason_code.clone(),
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "diagnostic.exit_health.affected_layers"),
                affected_layers,
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "diagnostic.exit_health.affected_targets"),
                affected_targets,
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "diagnostic.exit_health.affected_protocols"),
                affected_protocols,
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "diagnostic.exit_health.fresh_evidence"),
                format_integer(self.locale, assessment.fresh_evidence_count as i64),
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "diagnostic.exit_health.observed_at"),
                assessment_observed,
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "diagnostic.exit_health.retest"),
                retest_label,
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "diagnostic.exit_health.max_age"),
                format_diagnostic_duration(self.locale, diagnostics.exit_health.max_age_ms),
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "diagnostic.exit_health.source"),
                assessment_source,
            ))
            .when(health_evidence.is_empty(), |card| {
                card.child(
                    div()
                        .text_xs()
                        .text_color(rgba(COLOR_MUTED))
                        .child(t(self.locale, "diagnostic.exit_health.no_evidence").to_string()),
                )
            })
            .children(health_evidence);

        let leak_assessment = &diagnostics.leak_detection_assessment;
        let leak_palette = semantic_palette(leak_detection_state_semantic(leak_assessment.state));
        let leak_state_label = t(
            self.locale,
            leak_detection_state_label_key(leak_assessment.state),
        )
        .to_string();
        let leak_separator = t(self.locale, "common.list_separator");
        let leak_affected_protocols = if leak_assessment.affected_protocols.is_empty() {
            t(self.locale, "diagnostic.leak_detection.none").to_string()
        } else {
            leak_assessment
                .affected_protocols
                .iter()
                .map(|protocol| {
                    t(self.locale, leak_detection_protocol_label_key(*protocol)).to_string()
                })
                .collect::<Vec<_>>()
                .join(leak_separator)
        };
        let leak_affected_targets = if leak_assessment.affected_targets.is_empty() {
            t(self.locale, "diagnostic.leak_detection.none").to_string()
        } else {
            leak_assessment
                .affected_targets
                .iter()
                .map(|target| t(self.locale, leak_detection_target_label_key(*target)).to_string())
                .collect::<Vec<_>>()
                .join(leak_separator)
        };
        let leak_dns_mode = t(
            self.locale,
            leak_detection_dns_mode_label_key(diagnostics.leak_detection.environment.dns_mode),
        )
        .to_string();
        let leak_platform = t(
            self.locale,
            leak_detection_platform_label_key(diagnostics.leak_detection.environment.platform),
        )
        .to_string();
        let leak_protected_path = t(
            self.locale,
            if diagnostics.leak_detection.environment.protected_path_active {
                "diagnostic.leak_detection.yes"
            } else {
                "diagnostic.leak_detection.no"
            },
        )
        .to_string();
        let leak_ipv4_carrier = t(
            self.locale,
            if diagnostics
                .leak_detection
                .environment
                .ipv4_carrier_available
            {
                "diagnostic.leak_detection.available"
            } else {
                "diagnostic.leak_detection.unavailable_or_unverified"
            },
        )
        .to_string();
        let leak_ipv6_carrier = t(
            self.locale,
            if diagnostics
                .leak_detection
                .environment
                .ipv6_carrier_available
            {
                "diagnostic.leak_detection.available"
            } else {
                "diagnostic.leak_detection.unavailable_or_unverified"
            },
        )
        .to_string();
        let leak_generated_at = if diagnostics.leak_detection.generated_at_unix_ms == 0 {
            t(self.locale, "common.not_available").to_string()
        } else {
            format_diagnostic_timestamp(
                self.locale,
                diagnostics.leak_detection.generated_at_unix_ms,
            )
        };
        let leak_evidence = diagnostics
            .leak_detection
            .evidence
            .iter()
            .enumerate()
            .map(|(index, evidence)| {
                let target_label = t(
                    self.locale,
                    leak_detection_target_label_key(evidence.target),
                )
                .to_string();
                let state_label = t(
                    self.locale,
                    leak_detection_evidence_state_label_key(evidence.state),
                )
                .to_string();
                let source_label = t(
                    self.locale,
                    leak_detection_source_label_key(evidence.source),
                )
                .to_string();
                let protocol_label = t(
                    self.locale,
                    leak_detection_protocol_label_key(evidence.target.protocol()),
                )
                .to_string();
                let error_code = evidence
                    .error_code
                    .clone()
                    .unwrap_or_else(|| t(self.locale, "common.none").to_string());
                let isolation_action = evidence
                    .isolation_action
                    .map(|action| {
                        t(
                            self.locale,
                            leak_detection_isolation_action_label_key(action),
                        )
                        .to_string()
                    })
                    .unwrap_or_else(|| {
                        t(self.locale, "diagnostic.leak_detection.none").to_string()
                    });
                let role = if evidence.state == LeakDetectionEvidenceState::Failed {
                    Role::Alert
                } else {
                    Role::Status
                };
                let selector = format!(
                    "visual-diagnostic-leak-target-{}-{index}",
                    evidence.target.key()
                );
                let aria_label = format!("{target_label}: {state_label}");
                div()
                    .id(SharedString::from(format!(
                        "diagnostic-leak-target-{}-{index}",
                        evidence.target.key()
                    )))
                    .debug_selector(move || selector)
                    .role(role)
                    .aria_label(aria_label)
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .rounded_md()
                    .bg(rgba(COLOR_PANEL_RAISED))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(gpui::FontWeight(600.0))
                                    .text_color(rgba(COLOR_TEXT))
                                    .child(target_label),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgba(COLOR_MUTED))
                                    .child(state_label),
                            ),
                    )
                    .child(diagnostic_metadata_row(
                        t(self.locale, "diagnostic.leak_detection.protocol"),
                        protocol_label,
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "diagnostic.leak_detection.source"),
                        source_label,
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "diagnostic.leak_detection.observed_at"),
                        format_diagnostic_timestamp(self.locale, evidence.observed_at_unix_ms),
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "diagnostic.duration"),
                        format_diagnostic_duration(self.locale, evidence.duration_ms),
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "diagnostic.error_code"),
                        error_code,
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "diagnostic.leak_detection.isolation_action"),
                        isolation_action,
                    ))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let leak_detection = div()
            .id("diagnostic-leak-detection")
            .debug_selector(|| "visual-diagnostic-leak-detection".into())
            .role(
                if leak_assessment.state == LeakDetectionState::LeakDetected {
                    Role::Alert
                } else {
                    Role::Status
                },
            )
            .aria_label(format!(
                "{}: {leak_state_label}",
                t(self.locale, "diagnostic.leak_detection.title")
            ))
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_md()
            .bg(rgba(leak_palette.background.rgba_hex()))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_start()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(gpui::FontWeight(600.0))
                                    .text_color(rgba(COLOR_TEXT))
                                    .child(
                                        t(self.locale, "diagnostic.leak_detection.title")
                                            .to_string(),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .line_height(px(18.0))
                                    .text_color(rgba(COLOR_TEXT_SECONDARY))
                                    .child(
                                        t(self.locale, "diagnostic.leak_detection.disclaimer")
                                            .to_string(),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_full()
                            .bg(rgba(COLOR_PANEL_RAISED))
                            .text_xs()
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(leak_palette.foreground.rgba_hex()))
                            .child(leak_state_label),
                    ),
            )
            .child(diagnostic_metadata_row(
                t(self.locale, "diagnostic.leak_detection.reason"),
                leak_assessment.reason_code.clone(),
            ))
            .child(diagnostic_metadata_row(
                t(
                    self.locale,
                    "diagnostic.leak_detection.fail_closed_required",
                ),
                t(
                    self.locale,
                    if leak_assessment.fail_closed_required {
                        "diagnostic.leak_detection.yes"
                    } else {
                        "diagnostic.leak_detection.no"
                    },
                )
                .to_string(),
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "diagnostic.leak_detection.affected_protocols"),
                leak_affected_protocols,
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "diagnostic.leak_detection.affected_targets"),
                leak_affected_targets,
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "diagnostic.leak_detection.dns_mode"),
                leak_dns_mode,
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "diagnostic.leak_detection.platform"),
                leak_platform,
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "diagnostic.leak_detection.protected_path"),
                leak_protected_path,
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "diagnostic.leak_detection.ipv4_carrier"),
                leak_ipv4_carrier,
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "diagnostic.leak_detection.ipv6_carrier"),
                leak_ipv6_carrier,
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "diagnostic.leak_detection.generated_at"),
                leak_generated_at,
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "diagnostic.leak_detection.max_age"),
                format_diagnostic_duration(self.locale, diagnostics.leak_detection.max_age_ms),
            ))
            .when(leak_evidence.is_empty(), |card| {
                card.child(
                    div()
                        .text_xs()
                        .text_color(rgba(COLOR_MUTED))
                        .child(t(self.locale, "diagnostic.leak_detection.no_evidence").to_string()),
                )
            })
            .children(leak_evidence);

        let residue_summary = if diagnostics.residue.is_clean() {
            t(self.locale, "diagnostic.residue_clean").to_string()
        } else {
            let residue = &diagnostics.residue;
            tr(
                self.locale,
                "diagnostic.residue_counts",
                &[
                    (
                        "sessions",
                        &format_integer(self.locale, residue.sessions_remaining as i64),
                    ),
                    (
                        "listeners",
                        &format_integer(self.locale, residue.listeners_remaining as i64),
                    ),
                    (
                        "peers",
                        &format_integer(self.locale, residue.peers_remaining as i64),
                    ),
                    (
                        "services",
                        &format_integer(self.locale, residue.embedded_services_remaining as i64),
                    ),
                    (
                        "networks",
                        &format_integer(self.locale, residue.residual_networks.len() as i64),
                    ),
                ],
            )
        };
        let separator = t(self.locale, "common.list_separator");
        let residue = div()
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .rounded_md()
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight(600.0))
                    .text_color(rgba(COLOR_TEXT))
                    .child(t(self.locale, "diagnostic.residue").to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .line_height(px(18.0))
                    .text_color(rgba(COLOR_TEXT_SECONDARY))
                    .child(residue_summary),
            )
            .when(
                !diagnostics.cleanup_candidate_networks.is_empty(),
                |residue| {
                    residue.child(div().text_xs().text_color(rgba(COLOR_MUTED)).child(tr(
                        self.locale,
                        "diagnostic.cleanup_candidates",
                        &[(
                            "networks",
                            &diagnostics.cleanup_candidate_networks.join(separator),
                        )],
                    )))
                },
            );

        let recovery_actions = diagnostics
            .recovery_actions
            .iter()
            .map(|action| {
                let action_name = action_label(self.locale, &action.action_id);
                let confirmation = t(
                    self.locale,
                    if action.confirmation_required {
                        "diagnostic.recovery.confirmation_required"
                    } else {
                        "diagnostic.recovery.no_confirmation"
                    },
                )
                .to_string();
                div()
                    .id(SharedString::from(format!(
                        "diagnostic-recovery-{}",
                        action.action_id
                    )))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_3()
                    .rounded_md()
                    .bg(rgba(COLOR_PANEL_RAISED))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight(600.0))
                                    .text_color(rgba(COLOR_TEXT))
                                    .child(action_name),
                            )
                            .child(
                                div()
                                    .font_family("monospace")
                                    .text_xs()
                                    .text_color(rgba(COLOR_MUTED))
                                    .child(action.action_id.clone()),
                            ),
                    )
                    .child(diagnostic_metadata_row(
                        t(self.locale, "diagnostic.recovery.impact"),
                        t(self.locale, &action.impact_key).to_string(),
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "diagnostic.recovery.action"),
                        confirmation,
                    ))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let recovery = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_md()
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight(600.0))
                    .text_color(rgba(COLOR_TEXT))
                    .child(t(self.locale, "diagnostic.recovery.title").to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .line_height(px(18.0))
                    .text_color(rgba(COLOR_MUTED))
                    .child(t(self.locale, "diagnostic.recovery.disclaimer").to_string()),
            )
            .when(recovery_actions.is_empty(), |recovery| {
                recovery.child(
                    div()
                        .text_xs()
                        .text_color(rgba(COLOR_MUTED))
                        .child(t(self.locale, "diagnostic.recovery.none").to_string()),
                )
            })
            .children(recovery_actions);

        div()
            .id("runtime-diagnostics")
            .debug_selector(|| "visual-runtime-diagnostics".into())
            .role(Role::Article)
            .aria_label(t(self.locale, "diagnostic.title").to_string())
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_lg()
            .bg(rgba(COLOR_PANEL))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(COLOR_TEXT))
                            .child(t(self.locale, "diagnostic.title").to_string()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(run_button(busy))
                            .child(copy_id_button)
                            .child(copy_report_button),
                    ),
            )
            .when_some(copy_notice, |card, notice| {
                card.child(
                    div()
                        .id("diagnostic-copy-notice")
                        .role(Role::Status)
                        .text_xs()
                        .text_color(rgba(DARK_THEME.colors.success.rgba_hex()))
                        .child(notice),
                )
            })
            .child(metadata)
            .child(div().flex().flex_col().gap_2().children(checks))
            .child(exit_health)
            .child(leak_detection)
            .child(residue)
            .child(recovery)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .rounded_md()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(COLOR_TEXT))
                            .child(t(self.locale, "diagnostic.report").to_string()),
                    )
                    .child(
                        div()
                            .id("diagnostic-sanitized-report")
                            .max_h(px(180.0))
                            .overflow_y_scroll()
                            .rounded_md()
                            .bg(rgba(COLOR_BG))
                            .p_2()
                            .font_family("monospace")
                            .text_xs()
                            .line_height(px(18.0))
                            .text_color(rgba(COLOR_TEXT_SECONDARY))
                            .child(diagnostics.sanitized_report.clone()),
                    ),
            )
            .into_any_element()
    }

    fn operation_status_card(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(operation) = self.operation.as_ref() else {
            return div().into_any_element();
        };
        let state = t(self.locale, operation_state_key(operation.operation.state)).to_string();
        let diagnostic_id = operation.diagnostic_id().to_owned();
        let terminal = operation_state_is_terminal(operation.operation.state);
        let can_cancel = operation.operation.cancellable && !terminal;
        let cancellability = if operation.operation.cancellable && !terminal {
            t(self.locale, "operation.cancellable_yes").to_string()
        } else {
            let reason = if terminal {
                t(self.locale, "operation.cancel.already_terminal")
            } else {
                operation
                    .operation
                    .cancel_blocked_reason
                    .as_deref()
                    .map(|key| t(self.locale, key))
                    .unwrap_or_else(|| t(self.locale, "operation.cancellable_unknown"))
            };
            tr(
                self.locale,
                "operation.cancellable_no",
                &[("reason", reason)],
            )
        };
        let error = operation.operation.error.as_ref().map(|error| {
            let retry = if error.retryable {
                t(self.locale, "operation.retryable")
            } else {
                t(self.locale, "operation.not_retryable")
            };
            format!(
                "{} · {}: {} · {}",
                format_operation_error(self.locale, error),
                t(self.locale, "operation.error_code"),
                error.code,
                retry
            )
        });
        let attempt = tr(
            self.locale,
            "operation.attempt",
            &[
                ("attempt", &operation.operation.attempt.to_string()),
                ("max", &operation.operation.max_attempts.to_string()),
            ],
        );
        let cancel_pending = self.operation_cancel_pending;
        let cancel_error = self.operation_cancel_error.clone();
        let retry_command = self.last_operation_command.clone();
        let can_retry = terminal
            && operation
                .operation
                .error
                .as_ref()
                .is_some_and(|error| error.retryable)
            && retry_command.is_some();

        let mut actions = div().flex().flex_wrap().items_center().gap_2();
        if can_retry {
            let retry_command = retry_command.expect("retry command checked above");
            actions = actions.child(
                button(
                    SharedString::from("retry-operation"),
                    t(self.locale, "operation.retry"),
                    ButtonVariant::Secondary,
                    false,
                )
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.submit_operation(retry_command.clone(), cx);
                }))
                .debug_selector(|| "visual-operation-retry".into()),
            );
        }
        if can_cancel {
            actions = actions.child(
                button(
                    SharedString::from("cancel-operation"),
                    t(
                        self.locale,
                        if cancel_pending {
                            "operation.cancelling"
                        } else {
                            "operation.cancel"
                        },
                    ),
                    ButtonVariant::Secondary,
                    cancel_pending,
                )
                .on_click(cx.listener(|shell, _, _, cx| shell.cancel_operation(cx)))
                .debug_selector(|| "visual-operation-cancel".into()),
            );
        }
        actions = actions.child(
            button(
                SharedString::from("copy-operation-diagnostic-id"),
                t(self.locale, "operation.copy_diagnostic_id"),
                ButtonVariant::Secondary,
                false,
            )
            .on_click(cx.listener(|shell, _, _, cx| {
                if let Some(operation) = &shell.operation {
                    let diagnostic_id = operation.diagnostic_id().to_owned();
                    cx.write_to_clipboard(ClipboardItem::new_string(diagnostic_id));
                    shell.notice = Some(t(shell.locale, "operation.diagnostic_copied").to_string());
                    cx.notify();
                }
            })),
        );

        div()
            .id("node-operation-status")
            .debug_selector(|| "visual-node-operation".into())
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_lg()
            .bg(rgba(COLOR_PANEL))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(COLOR_TEXT))
                            .child(t(self.locale, "operation.title").to_string()),
                    )
                    .child(actions),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .text_xs()
                    .text_color(rgba(COLOR_TEXT_SECONDARY))
                    .child(format!("{}: {state}", t(self.locale, "operation.state")))
                    .child(format!(
                        "{}: {}",
                        t(self.locale, "operation.stage"),
                        operation.operation.stage
                    ))
                    .child(attempt)
                    .child(cancellability),
            )
            .when_some(error, |card, error| {
                card.child(
                    div()
                        .text_xs()
                        .line_height(px(18.0))
                        .text_color(rgba(COLOR_WARNING))
                        .child(error),
                )
            })
            .when_some(cancel_error, |card, error| {
                card.child(
                    div()
                        .text_xs()
                        .line_height(px(18.0))
                        .text_color(rgba(COLOR_WARNING))
                        .child(tr(
                            self.locale,
                            "operation.cancel_failed",
                            &[("error", &error)],
                        )),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .text_color(rgba(COLOR_MUTED))
                    .child(t(self.locale, "operation.diagnostic_id").to_string())
                    .child(
                        div()
                            .font_family("monospace")
                            .text_color(rgba(COLOR_TEXT_SECONDARY))
                            .child(diagnostic_id),
                    ),
            )
            .into_any_element()
    }

    fn networks_content(
        &mut self,
        layout: LayoutClass,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let compact = matches!(layout, LayoutClass::Mobile | LayoutClass::Compact);
        let projection = self.discovery_projection();
        let (languages, regions) = self.discovery_filter_options();
        let joined_names = self
            .snapshot
            .networks
            .iter()
            .map(|network| network.name.clone())
            .chain(
                self.snapshot
                    .configuration
                    .networks
                    .iter()
                    .map(|network| network.network_name.clone()),
            )
            .collect::<BTreeSet<_>>();
        let joined_cards = joined_names
            .iter()
            .map(|network_name| {
                let runtime = self
                    .snapshot
                    .networks
                    .iter()
                    .find(|network| &network.name == network_name);
                let configured = self
                    .snapshot
                    .configuration
                    .networks
                    .iter()
                    .find(|network| &network.network_name == network_name);
                let status = runtime
                    .map(|network| t(self.locale, network_status_key(network.status)).to_string())
                    .unwrap_or_else(|| {
                        t(self.locale, network_status_key(NodeNetworkStatus::Offline)).to_string()
                    });
                let detail = runtime
                    .map(|network| {
                        tr(
                            self.locale,
                            "overview.network.detail",
                            &[
                                ("peers", &network.peer_count.to_string()),
                                ("tun", &network.tun_name),
                            ],
                        )
                    })
                    .or_else(|| {
                        configured.map(|network| {
                            format!(
                                "{status} · {} · {}",
                                network.tun_name, network.coordinator_url
                            )
                        })
                    })
                    .unwrap_or(status);
                let actions_open =
                    self.network_actions_open.as_deref() == Some(network_name.as_str());
                let trigger_name = network_name.clone();
                let trigger_selector_name = network_name.clone();
                let mut card = div()
                    .debug_selector(|| "visual-discovery-joined-network".into())
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_3()
                    .rounded_md()
                    .bg(rgba(COLOR_PANEL_RAISED))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .min_w_0()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight(600.0))
                                    .text_color(rgba(COLOR_TEXT))
                                    .child(network_name.clone()),
                            )
                            .child(
                                button(
                                    SharedString::from(format!(
                                        "joined-network-actions-{}",
                                        network_name
                                    )),
                                    t(self.locale, "common.actions").to_string(),
                                    ButtonVariant::Ghost,
                                    false,
                                )
                                .debug_selector(move || {
                                    format!(
                                        "visual-discovery-joined-actions-{}",
                                        trigger_selector_name
                                    )
                                })
                                .on_click(cx.listener(
                                    move |shell, _, _, cx| {
                                        shell.network_actions_open =
                                            if shell.network_actions_open.as_deref()
                                                == Some(trigger_name.as_str())
                                            {
                                                None
                                            } else {
                                                Some(trigger_name.clone())
                                            };
                                        cx.notify();
                                    },
                                )),
                            ),
                    )
                    .child(div().text_xs().text_color(rgba(COLOR_MUTED)).child(detail));

                if actions_open {
                    let menu_name = network_name.clone();
                    let peers_name = network_name.clone();
                    let routing_name = network_name.clone();
                    let traffic_name = network_name.clone();
                    let members_name = network_name.clone();
                    let leave_name = network_name.clone();
                    let members_network_id =
                        network_membership_id(&self.snapshot, network_name).map(str::to_string);
                    let members_action = members_network_id.map(|network_id| {
                        button(
                            SharedString::from(format!("joined-network-members-{}", network_name)),
                            t(self.locale, "action.list_network_members").to_string(),
                            ButtonVariant::Ghost,
                            false,
                        )
                        .debug_selector({
                            let network_name = network_name.clone();
                            move || format!("visual-discovery-joined-members-{network_name}")
                        })
                        .w_full()
                        .on_click(cx.listener(move |shell, _, _, cx| {
                            shell.open_network_members(
                                members_name.clone(),
                                network_id.clone(),
                                cx,
                            );
                        }))
                    });
                    card = card.child(
                        div()
                            .debug_selector(move || {
                                format!("visual-discovery-joined-menu-{}", menu_name)
                            })
                            .flex()
                            .flex_col()
                            .gap_1()
                            .pt_2()
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .text_size(px(10.0))
                                    .font_weight(gpui::FontWeight(600.0))
                                    .text_color(rgba(COLOR_FAINT))
                                    .child(t(self.locale, "common.navigation").to_string()),
                            )
                            .child(
                                button(
                                    SharedString::from(format!(
                                        "joined-network-peers-{}",
                                        network_name
                                    )),
                                    t(self.locale, "nav.peers").to_string(),
                                    ButtonVariant::Ghost,
                                    false,
                                )
                                .w_full()
                                .on_click(cx.listener(
                                    move |shell, _, _, cx| {
                                        shell.navigate_to_row(
                                            &RowNavigation {
                                                page_id: "peers".into(),
                                                network_name: Some(peers_name.clone()),
                                                object_id: None,
                                                label: t(shell.locale, "nav.peers").to_string(),
                                            },
                                            cx,
                                        );
                                    },
                                )),
                            )
                            .child(
                                button(
                                    SharedString::from(format!(
                                        "joined-network-routing-{}",
                                        network_name
                                    )),
                                    t(self.locale, "nav.routing").to_string(),
                                    ButtonVariant::Ghost,
                                    false,
                                )
                                .w_full()
                                .on_click(cx.listener(
                                    move |shell, _, _, cx| {
                                        shell.navigate_to_row(
                                            &RowNavigation {
                                                page_id: "routing".into(),
                                                network_name: Some(routing_name.clone()),
                                                object_id: None,
                                                label: t(shell.locale, "nav.routing").to_string(),
                                            },
                                            cx,
                                        );
                                    },
                                )),
                            )
                            .child(
                                button(
                                    SharedString::from(format!(
                                        "joined-network-traffic-{}",
                                        network_name
                                    )),
                                    t(self.locale, "nav.traffic").to_string(),
                                    ButtonVariant::Ghost,
                                    false,
                                )
                                .w_full()
                                .on_click(cx.listener(
                                    move |shell, _, _, cx| {
                                        shell.navigate_to_row(
                                            &RowNavigation {
                                                page_id: "traffic".into(),
                                                network_name: Some(traffic_name.clone()),
                                                object_id: None,
                                                label: t(shell.locale, "nav.traffic").to_string(),
                                            },
                                            cx,
                                        );
                                    },
                                )),
                            )
                            .children(members_action)
                            .child(div().h(px(1.0)).mx_2().my_1().bg(rgba(COLOR_DIVIDER)))
                            .child(
                                button(
                                    SharedString::from(format!(
                                        "joined-network-leave-{}",
                                        network_name
                                    )),
                                    t(self.locale, "action.leave_network").to_string(),
                                    ButtonVariant::Danger,
                                    false,
                                )
                                .w_full()
                                .on_click(cx.listener(
                                    move |shell, _, _, cx| {
                                        shell.select_network_context(leave_name.clone(), cx);
                                        shell.open_action("networks", "leave_network", cx);
                                    },
                                )),
                            ),
                    );
                }
                card.into_any_element()
            })
            .collect::<Vec<_>>();

        let joined_section = surface_card()
            .id("discovery-joined")
            .debug_selector(|| "visual-discovery-joined".into())
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_base()
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(COLOR_TEXT))
                            .child(t(self.locale, "discovery.section.joined").to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgba(COLOR_MUTED))
                            .child(t(self.locale, "discovery.section.joined_detail").to_string()),
                    ),
            )
            .when(joined_cards.is_empty(), |section| {
                section.child(
                    empty_state(t(self.locale, "discovery.empty.joined"))
                        .debug_selector(|| "visual-discovery-joined-empty".into()),
                )
            })
            .children(joined_cards);

        let search_input = self
            .discovery_search_input
            .as_ref()
            .map(|input| input.clone().into_any_element());
        let clear_search = (!self.discovery_query.trim().is_empty()).then(|| {
            button(
                SharedString::from("clear-discovery-search"),
                "",
                ButtonVariant::Ghost,
                false,
            )
            .absolute()
            .right(px(4.0))
            .top(px(3.0))
            .w(px(28.0))
            .h(px(28.0))
            .px_0()
            .aria_label(t(self.locale, "common.search_clear").to_string())
            .child(lucide_icon(
                include_bytes!("../../assets/icons/x.svg"),
                14.0,
                COLOR_MUTED,
            ))
            .on_click(cx.listener(|shell, _, _, cx| shell.clear_discovery_search(cx)))
        });
        let search_control = div()
            .id("discovery-search")
            .debug_selector(|| "visual-discovery-search".into())
            .relative()
            .flex()
            .items_center()
            .w_full()
            .h(px(34.0))
            .child(
                div()
                    .absolute()
                    .left(px(11.0))
                    .top(px(9.0))
                    .child(lucide_icon(
                        include_bytes!("../../assets/icons/search.svg"),
                        15.0,
                        COLOR_FAINT,
                    )),
            )
            .children(search_input)
            .children(clear_search);

        let mut visibility_controls = Vec::new();
        for (value, label_key, selected) in [
            (
                None,
                "discovery.filter.all",
                self.discovery_filters.visibility.is_none(),
            ),
            (
                Some(DiscoveryVisibilityFilter::Public),
                "discovery.filter.public",
                self.discovery_filters.visibility == Some(DiscoveryVisibilityFilter::Public),
            ),
            (
                Some(DiscoveryVisibilityFilter::Private),
                "discovery.filter.private",
                self.discovery_filters.visibility == Some(DiscoveryVisibilityFilter::Private),
            ),
        ] {
            let label = t(self.locale, label_key).to_string();
            let id = format!(
                "discovery-filter-visibility-{}",
                value.map(discovery_visibility_filter_name).unwrap_or("all")
            );
            visibility_controls.push(
                choice_control(SharedString::from(id), label, selected)
                    .on_click(cx.listener(move |shell, _, _, cx| {
                        shell.discovery_filters.visibility = value;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }

        let mut join_mode_controls = Vec::new();
        for (value, label_key, selected) in [
            (
                None,
                "discovery.filter.all",
                self.discovery_filters.join_mode.is_none(),
            ),
            (
                Some(NetworkJoinMode::Open),
                "choice.join_mode.open",
                self.discovery_filters.join_mode == Some(NetworkJoinMode::Open),
            ),
            (
                Some(NetworkJoinMode::ApprovalRequired),
                "choice.join_mode.approval_required",
                self.discovery_filters.join_mode == Some(NetworkJoinMode::ApprovalRequired),
            ),
            (
                Some(NetworkJoinMode::InviteOnly),
                "choice.join_mode.invite_only",
                self.discovery_filters.join_mode == Some(NetworkJoinMode::InviteOnly),
            ),
        ] {
            let label = t(self.locale, label_key).to_string();
            let id = format!(
                "discovery-filter-join-mode-{}",
                value.map(join_mode_filter_name).unwrap_or("all")
            );
            join_mode_controls.push(
                choice_control(SharedString::from(id), label, selected)
                    .on_click(cx.listener(move |shell, _, _, cx| {
                        shell.discovery_filters.join_mode = value;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }

        let mut language_controls = Vec::new();
        language_controls.push(
            choice_control(
                "discovery-filter-language-all".into(),
                t(self.locale, "discovery.filter.all").to_string(),
                self.discovery_filters.language.is_none(),
            )
            .on_click(cx.listener(|shell, _, _, cx| {
                shell.discovery_filters.language = None;
                cx.notify();
            }))
            .into_any_element(),
        );
        for language in languages {
            let selected = self.discovery_filters.language.as_deref() == Some(&language);
            let filter_value = language.clone();
            language_controls.push(
                choice_control(
                    SharedString::from(format!("discovery-filter-language-{language}")),
                    language,
                    selected,
                )
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.discovery_filters.language = Some(filter_value.clone());
                    cx.notify();
                }))
                .into_any_element(),
            );
        }

        let mut region_controls = Vec::new();
        region_controls.push(
            choice_control(
                "discovery-filter-region-all".into(),
                t(self.locale, "discovery.filter.all").to_string(),
                self.discovery_filters.region.is_none(),
            )
            .on_click(cx.listener(|shell, _, _, cx| {
                shell.discovery_filters.region = None;
                cx.notify();
            }))
            .into_any_element(),
        );
        for region in regions {
            let selected = self.discovery_filters.region.as_deref() == Some(&region);
            let filter_value = region.clone();
            region_controls.push(
                choice_control(
                    SharedString::from(format!("discovery-filter-region-{region}")),
                    region,
                    selected,
                )
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.discovery_filters.region = Some(filter_value.clone());
                    cx.notify();
                }))
                .into_any_element(),
            );
        }

        let filter_group = |title: SharedString, controls: Vec<gpui::AnyElement>| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .min_w(px(150.0))
                .flex_1()
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui::FontWeight(600.0))
                        .text_color(rgba(COLOR_MUTED))
                        .child(title),
                )
                .child(div().flex().flex_wrap().gap_2().children(controls))
        };
        let filters = div()
            .debug_selector(|| "visual-discovery-filters".into())
            .flex()
            .flex_wrap()
            .gap_3()
            .child(filter_group(
                t(self.locale, "discovery.filter.visibility")
                    .to_string()
                    .into(),
                visibility_controls,
            ))
            .child(filter_group(
                t(self.locale, "discovery.filter.join_mode")
                    .to_string()
                    .into(),
                join_mode_controls,
            ))
            .child(filter_group(
                t(self.locale, "discovery.filter.language")
                    .to_string()
                    .into(),
                language_controls,
            ))
            .child(filter_group(
                t(self.locale, "discovery.filter.region").to_string().into(),
                region_controls,
            ));

        let refresh = button(
            "discovery-refresh".into(),
            t(self.locale, "discovery.action.refresh"),
            ButtonVariant::Secondary,
            self.pending_commands > 0,
        )
        .on_click(cx.listener(|shell, _, _, cx| {
            shell.open_action("networks", "refresh_discovery_index", cx);
        }));
        let retry = projection
            .error_category
            .is_some_and(|category| category.retryable())
            .then(|| {
                button(
                    "discovery-retry".into(),
                    t(self.locale, "discovery.action.retry"),
                    ButtonVariant::Primary,
                    self.pending_commands > 0,
                )
                .debug_selector(|| "visual-discovery-retry".into())
                .on_click(cx.listener(|shell, _, _, cx| {
                    shell.submit_raw(NodeCommand::RetryDiscoveryIndex, cx);
                }))
            });

        let notices = self.discovery_notices(&projection);
        let result_cards = projection
            .results
            .iter()
            .map(|result| self.discovery_result_card(result, cx))
            .collect::<Vec<_>>();

        let discovery_section = surface_card()
            .id("discovery-results")
            .debug_selector(|| "visual-discovery-results".into())
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(gpui::FontWeight(600.0))
                                    .text_color(rgba(COLOR_TEXT))
                                    .child(t(self.locale, "discovery.section.results").to_string()),
                            )
                            .child(div().text_xs().text_color(rgba(COLOR_MUTED)).child(
                                t(self.locale, "discovery.section.results_detail").to_string(),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(refresh)
                            .children(retry),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(COLOR_TEXT_SECONDARY))
                            .child(t(self.locale, "discovery.search.label").to_string()),
                    )
                    .child(search_control)
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgba(COLOR_MUTED))
                            .child(t(self.locale, "discovery.search.private_note").to_string()),
                    ),
            )
            .child(filters)
            .children(notices)
            .when(result_cards.is_empty(), |section| {
                section.child(
                    empty_state(t(self.locale, discovery_empty_key(&projection)).to_string())
                        .debug_selector(|| "visual-discovery-empty-results".into()),
                )
            })
            .child(
                div()
                    .debug_selector(|| "visual-discovery-result-list".into())
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children(result_cards),
            )
            .when(projection.no_more_results, |section| {
                section.child(
                    div()
                        .debug_selector(|| "visual-discovery-no-more".into())
                        .text_xs()
                        .text_color(rgba(COLOR_MUTED))
                        .child(t(self.locale, "discovery.result.no_more_results").to_string()),
                )
            });

        div()
            .debug_selector(|| "visual-discovery-networks".into())
            .flex()
            .flex_col()
            .gap_4()
            .when(compact, |page| page.w_full())
            .child(joined_section)
            .child(discovery_section)
            .into_any_element()
    }

    fn discovery_notices(&self, projection: &DiscoveryProjection) -> Vec<gpui::AnyElement> {
        let mut notices = Vec::new();
        let state = match projection.state {
            DiscoveryProjectionState::Idle => Some((
                t(self.locale, "discovery.state.idle").to_string(),
                SemanticState::Neutral,
            )),
            DiscoveryProjectionState::Pending => Some((
                t(self.locale, "discovery.state.pending").to_string(),
                SemanticState::Info,
            )),
            DiscoveryProjectionState::Empty => Some((
                t(self.locale, "discovery.state.empty").to_string(),
                SemanticState::Neutral,
            )),
            DiscoveryProjectionState::NoMatches => Some((
                t(self.locale, "discovery.state.no_matches").to_string(),
                SemanticState::Neutral,
            )),
            DiscoveryProjectionState::Degraded => Some((
                t(self.locale, "discovery.state.degraded").to_string(),
                SemanticState::Warning,
            )),
            DiscoveryProjectionState::Failed => Some((
                t(self.locale, "discovery.state.failed").to_string(),
                SemanticState::Danger,
            )),
            DiscoveryProjectionState::Unsupported => Some((
                t(self.locale, "discovery.state.unsupported").to_string(),
                SemanticState::Unsupported,
            )),
            DiscoveryProjectionState::Ready => None,
        };
        if let Some((message, semantic)) = state {
            notices.push(discovery_notice(
                "visual-discovery-state",
                message,
                semantic,
            ));
        }
        if projection.state == DiscoveryProjectionState::Pending {
            notices.push(discovery_notice(
                "visual-discovery-cancel-hint",
                t(self.locale, "discovery.pending.cancel_hint").to_string(),
                SemanticState::Info,
            ));
        }
        if projection.rejected_record_count > 0 {
            notices.push(discovery_notice(
                "visual-discovery-rejected",
                tr(
                    self.locale,
                    "discovery.state.rejected",
                    &[("count", &projection.rejected_record_count.to_string())],
                ),
                SemanticState::Warning,
            ));
        }
        if projection.has_stale_results() {
            notices.push(discovery_notice(
                "visual-discovery-stale",
                t(self.locale, "discovery.state.stale").to_string(),
                SemanticState::Warning,
            ));
        }
        if let Some(category) = projection.error_category {
            notices.push(discovery_notice(
                "visual-discovery-error",
                tr(
                    self.locale,
                    "discovery.error.detail",
                    &[("message", t(self.locale, category.message_key()))],
                ),
                if category == NodeDiscoveryIndexErrorCategory::Cancelled {
                    SemanticState::Info
                } else {
                    SemanticState::Danger
                },
            ));
        }
        if let Some(last_success) = projection.last_success_unix_ms {
            notices.push(discovery_notice(
                "visual-discovery-last-success",
                tr(
                    self.locale,
                    "discovery.last_success",
                    &[(
                        "time",
                        &format_diagnostic_timestamp(self.locale, last_success),
                    )],
                ),
                SemanticState::Neutral,
            ));
        }
        notices.push(discovery_notice(
            "visual-discovery-source",
            tr(
                self.locale,
                "discovery.source.detail",
                &[("source", t(self.locale, projection.source.message_key()))],
            ),
            SemanticState::Neutral,
        ));
        if let Some(last_attempt) = projection.last_attempt_unix_ms {
            notices.push(discovery_notice(
                "visual-discovery-last-attempt",
                tr(
                    self.locale,
                    "discovery.last_attempt",
                    &[(
                        "time",
                        &format_diagnostic_timestamp(self.locale, last_attempt),
                    )],
                ),
                SemanticState::Neutral,
            ));
        }
        if projection.metrics.request_attempts > 0 {
            let attempts = projection.metrics.request_attempts.to_string();
            let succeeded = projection.metrics.requests_succeeded.to_string();
            let failed = projection.metrics.requests_failed.to_string();
            let retries = projection.metrics.retries_scheduled.to_string();
            let cancellations = projection.metrics.cancellations.to_string();
            notices.push(discovery_notice(
                "visual-discovery-metrics",
                tr(
                    self.locale,
                    "discovery.metrics.summary",
                    &[
                        ("attempts", &attempts),
                        ("succeeded", &succeeded),
                        ("failed", &failed),
                        ("retries", &retries),
                        ("cancellations", &cancellations),
                    ],
                ),
                SemanticState::Neutral,
            ));
        }
        notices
    }

    fn discovery_result_card(
        &self,
        result: &DiscoveryResult,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let network_id = result.network_id.as_str().to_string();
        let expanded = self
            .expanded_discovery
            .get(&network_id)
            .copied()
            .unwrap_or(false);
        let toggle_id = network_id.clone();
        let cta_label = match result.join_mode {
            NetworkJoinMode::Open => "discovery.result.open_join",
            NetworkJoinMode::ApprovalRequired => "discovery.result.request_join",
            NetworkJoinMode::InviteOnly => "discovery.result.use_invite",
        };
        let capabilities = result
            .capabilities
            .iter()
            .map(|capability| t(self.locale, profile_capability_key(*capability)).to_string())
            .collect::<Vec<_>>()
            .join(" · ");
        let tags = if result.tags.is_empty() {
            t(self.locale, "common.not_available").to_string()
        } else {
            result.tags.join(" · ")
        };
        let verified_at = format_diagnostic_timestamp(self.locale, result.verified_at_unix_ms);
        let join_mode = t(self.locale, profile_join_mode_key(result.join_mode)).to_string();
        let visibility = t(self.locale, profile_visibility_key(result.visibility)).to_string();
        let mut card = surface_card()
            .id(SharedString::from(format!("discovery-result-{network_id}")))
            .debug_selector(|| "visual-discovery-result".into())
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .id(SharedString::from(format!(
                        "discovery-result-toggle-{network_id}"
                    )))
                    .role(Role::Button)
                    .aria_label(t(self.locale, "discovery.result.details").to_string())
                    .aria_expanded(expanded)
                    .focusable()
                    .tab_stop(true)
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .p_4()
                    .cursor_pointer()
                    .on_click(cx.listener(move |shell, _, _, cx| {
                        let expanded = shell
                            .expanded_discovery
                            .entry(toggle_id.clone())
                            .or_insert(false);
                        *expanded = !*expanded;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight(600.0))
                                    .text_color(rgba(COLOR_TEXT))
                                    .child(result.title.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .line_height(px(18.0))
                                    .text_color(rgba(COLOR_TEXT_SECONDARY))
                                    .child(result.purpose.clone()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(profile_status_chip(join_mode))
                            .child(profile_status_chip(visibility))
                            .when(result.stale, |chips| {
                                chips.child(
                                    div()
                                        .debug_selector(|| "visual-discovery-result-stale".into())
                                        .rounded_full()
                                        .bg(rgba(DARK_THEME.colors.warning_soft.rgba_hex()))
                                        .px_2()
                                        .py_1()
                                        .text_xs()
                                        .text_color(rgba(COLOR_WARNING))
                                        .child(
                                            t(self.locale, "discovery.result.stale").to_string(),
                                        ),
                                )
                            }),
                    ),
            );

        if expanded {
            card = card.child(
                div()
                    .debug_selector(|| "visual-discovery-result-details".into())
                    .flex()
                    .flex_col()
                    .gap_2()
                    .px_4()
                    .py_3()
                    .text_xs()
                    .line_height(px(18.0))
                    .text_color(rgba(COLOR_TEXT_SECONDARY))
                    .child(format!(
                        "{}: {network_id}",
                        t(self.locale, "discovery.result.network_id")
                    ))
                    .child(format!(
                        "{}: {capabilities}",
                        t(self.locale, "discovery.result.capabilities")
                    ))
                    .child(format!(
                        "{}: {tags}",
                        t(self.locale, "discovery.result.tags")
                    ))
                    .child(tr(
                        self.locale,
                        "discovery.result.last_verified",
                        &[("time", &verified_at)],
                    ))
                    .child(
                        div()
                            .text_color(rgba(COLOR_MUTED))
                            .child(t(self.locale, "discovery.result.privacy_summary").to_string()),
                    )
                    .when(result.stale, |details| {
                        details.child(
                            div()
                                .text_color(rgba(COLOR_WARNING))
                                .child(t(self.locale, "discovery.result.stale_detail").to_string()),
                        )
                    }),
            );
        }

        let result = result.clone();
        card.child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .px_4()
                .py_3()
                .child(div().text_xs().text_color(rgba(COLOR_MUTED)).child(tr(
                    self.locale,
                    "discovery.result.last_verified",
                    &[("time", &verified_at)],
                )))
                .child(
                    button(
                        SharedString::from(format!("discovery-join-{network_id}")),
                        t(self.locale, cta_label),
                        ButtonVariant::Primary,
                        self.pending_commands > 0,
                    )
                    .debug_selector(|| "visual-discovery-cta".into())
                    .on_click(cx.listener(move |shell, _, _, cx| {
                        shell.request_discovery_join(&result, cx);
                    })),
                ),
        )
        .into_any_element()
    }

    fn topology_presentation_for_current_network(&self) -> TopologyPresentationSignals {
        let network_id = self.topology_network_id();
        let directory_stale = network_id
            .as_ref()
            .is_some_and(|network_id| self.topology_directory_is_stale_for_network(network_id));
        TopologyPresentationSignals {
            initial_loading: self.snapshot.topology.is_none() && self.pending_commands > 0,
            refreshing: self.pending_commands > 0
                && self.snapshot.topology.is_some()
                && self.topology_directory_request.is_none(),
            offline: false,
            stale: self.snapshot.control_health == NodeControlHealth::Unavailable
                || directory_stale,
            error: self.error.is_some(),
            permission_denied: !self
                .snapshot
                .permissions
                .permissions
                .contains(&NodePermission::ViewState),
        }
    }

    fn ensure_topology_directory(&mut self, cx: &mut Context<Self>) {
        if self.selected_page != "topology" {
            return;
        }
        let Some(network_id) = self.topology_network_id() else {
            return;
        };
        let Some(topology_revision) = self
            .snapshot
            .topology
            .as_ref()
            .map(|topology| topology.revision)
        else {
            return;
        };
        self.request_topology_directory(network_id, topology_revision, cx);
    }

    fn refresh_topology_directory(&mut self, cx: &mut Context<Self>) {
        let Some(network_id) = self.topology_network_id() else {
            return;
        };
        let Some(topology_revision) = self
            .snapshot
            .topology
            .as_ref()
            .map(|topology| topology.revision)
        else {
            return;
        };
        if self
            .topology_directory_request
            .as_ref()
            .is_some_and(|request| {
                request.generation == self.topology_directory_generation
                    && request.network_id == network_id
                    && request.topology_revision == topology_revision
            })
        {
            return;
        }
        if self
            .topology_directory_error_for_network(&network_id)
            .is_some()
        {
            self.topology_directory_error = None;
        }
        self.request_topology_directory(network_id, topology_revision, cx);
    }

    fn topology_state_card(
        &self,
        presentation: &super::view_model::TopologyPresentation,
    ) -> gpui::AnyElement {
        let palette = semantic_palette(match presentation.kind {
            TopologyPresentationCondition::Ready => SemanticState::Success,
            TopologyPresentationCondition::Error => SemanticState::Danger,
            TopologyPresentationCondition::PermissionDenied => SemanticState::PermissionDenied,
            TopologyPresentationCondition::Offline => SemanticState::Danger,
            TopologyPresentationCondition::Stale
            | TopologyPresentationCondition::Partial
            | TopologyPresentationCondition::Refreshing => SemanticState::Warning,
            TopologyPresentationCondition::InitialLoading => SemanticState::Info,
            TopologyPresentationCondition::Unavailable | TopologyPresentationCondition::Empty => {
                SemanticState::Neutral
            }
        });
        let kind = presentation.kind;
        let title = t(self.locale, kind.title_key()).to_string();
        let detail = t(self.locale, kind.detail_key()).to_string();
        let conditions = presentation
            .conditions
            .iter()
            .map(|condition| condition.key())
            .collect::<Vec<_>>()
            .join(", ");
        div()
            .id("visual-topology-state")
            .debug_selector(|| "visual-topology-state".into())
            .role(Role::Status)
            .aria_label(format!("{title}: {detail}"))
            .flex()
            .flex_col()
            .gap_2()
            .rounded_lg()
            .bg(rgba(palette.background.rgba_hex()))
            .px_4()
            .py_3()
            .child(
                div()
                    .id(format!("topology-state-{}", kind.key()))
                    .debug_selector(move || format!("visual-topology-state-{}", kind.key()))
                    .text_sm()
                    .font_weight(gpui::FontWeight(600.0))
                    .text_color(rgba(palette.foreground.rgba_hex()))
                    .child(title),
            )
            .child(
                div()
                    .text_xs()
                    .line_height(px(18.0))
                    .text_color(rgba(palette.foreground.rgba_hex()))
                    .child(detail),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(rgba(COLOR_MUTED))
                    .child(conditions),
            )
            .into_any_element()
    }

    fn topology_detail_card(
        &self,
        topology: &TopologySnapshot,
        network_id: &NetworkId,
        directory: Option<&NetworkMemberDirectory>,
    ) -> gpui::AnyElement {
        let Some(node_id) = self.selected_topology_node_id.as_deref() else {
            return div()
                .id("visual-topology-detail")
                .debug_selector(|| "visual-topology-detail".into())
                .flex()
                .flex_col()
                .gap_1()
                .p_4()
                .rounded_lg()
                .bg(rgba(COLOR_PANEL))
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight(600.0))
                        .text_color(rgba(COLOR_TEXT))
                        .child(t(self.locale, "topology.detail.empty_title").to_string()),
                )
                .child(
                    div()
                        .debug_selector(|| "visual-topology-detail-empty".into())
                        .text_xs()
                        .text_color(rgba(COLOR_MUTED))
                        .child(t(self.locale, "topology.detail.empty_hint").to_string()),
                )
                .into_any_element();
        };
        let Some(node) = topology
            .nodes
            .iter()
            .find(|node| node.node_id.as_str() == node_id && node.belongs_to(network_id))
        else {
            return div()
                .id("visual-topology-detail")
                .debug_selector(|| "visual-topology-detail".into())
                .p_4()
                .text_sm()
                .text_color(rgba(COLOR_MUTED))
                .child(t(self.locale, "topology.detail.empty_title").to_string())
                .into_any_element();
        };
        let member = directory.and_then(|directory| {
            directory
                .members
                .iter()
                .find(|member| member.node_id.as_str() == node.node_id.as_str())
        });
        let generated_at_unix_ms = directory
            .map(|directory| directory.generated_at_unix_ms)
            .unwrap_or(topology.generated_at_unix_ms);
        let detail = topology_node_detail(
            node,
            Some(network_id.as_str()),
            member,
            self.locale,
            generated_at_unix_ms,
        );
        let exit_observed_at = detail
            .exit_observed_at_unix_ms
            .map(|observed| format_diagnostic_timestamp(self.locale, observed))
            .unwrap_or_else(|| t(self.locale, "common.unknown").to_string());
        let field = |label_key: &'static str, value: String| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .min_w(px(160.0))
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(rgba(COLOR_MUTED))
                        .child(t(self.locale, label_key).to_string()),
                )
                .child(div().text_sm().text_color(rgba(COLOR_TEXT)).child(value))
        };
        div()
            .id("visual-topology-detail")
            .debug_selector(|| "visual-topology-detail".into())
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_lg()
            .bg(rgba(COLOR_PANEL))
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(rgba(COLOR_MUTED))
                                    .child(t(self.locale, "topology.detail.title").to_string()),
                            )
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(gpui::FontWeight(600.0))
                                    .text_color(rgba(COLOR_TEXT))
                                    .child(detail.display_name.clone()),
                            )
                            .child(
                                div()
                                    .font_family("monospace")
                                    .text_xs()
                                    .text_color(rgba(COLOR_TEXT_SECONDARY))
                                    .child(detail.node_id.clone()),
                            ),
                    )
                    .child(
                        div()
                            .rounded_full()
                            .px_2()
                            .py_1()
                            .text_xs()
                            .text_color(rgba(COLOR_TEXT_SECONDARY))
                            .child(detail.health.clone()),
                    ),
            )
            .child(div().flex().flex_wrap().gap_4().children([
                field("topology.detail.role", detail.role),
                field("topology.detail.capabilities", detail.capabilities),
                field("topology.detail.health", detail.health),
                field("topology.detail.status", detail.status),
                field("topology.detail.presence", detail.presence),
                field("topology.detail.networks", detail.networks),
            ]))
            .child(
                div()
                    .debug_selector(|| "visual-topology-exit-availability".into())
                    .flex()
                    .flex_col()
                    .gap_2()
                    .rounded_md()
                    .p_3()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(COLOR_TEXT))
                            .child(t(self.locale, "member_directory.exit.title").to_string()),
                    )
                    .child(div().flex().flex_wrap().gap_4().children([
                        field("member_directory.exit.state", detail.exit_state),
                        field("member_directory.exit.source", detail.exit_source),
                        field("member_directory.exit.declared", detail.exit_declared),
                        field("member_directory.exit.observed_at", exit_observed_at),
                        field("member_directory.exit.freshness", detail.exit_freshness),
                        field("member_directory.exit.reason", detail.exit_reason),
                    ])),
            )
            .into_any_element()
    }

    fn topology_action_panel_body(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(network_id) = self.topology_network_id() else {
            return div()
                .id("topology-action-panel-body")
                .p_4()
                .text_sm()
                .text_color(rgba(COLOR_MUTED))
                .child(t(self.locale, "topology.state.unavailable.title").to_string())
                .into_any_element();
        };
        let topology_revision = self
            .snapshot
            .topology
            .as_ref()
            .map(|topology| topology.revision);
        let network_label = self
            .snapshot
            .topology
            .as_ref()
            .and_then(|topology| topology.boundary(&network_id))
            .map(|boundary| format!("{} · {}", boundary.name, boundary.network_id))
            .unwrap_or_else(|| network_id.to_string());
        let visible_count = self
            .snapshot
            .topology
            .as_ref()
            .map(|topology| {
                topology
                    .nodes
                    .iter()
                    .filter(|node| node.belongs_to(&network_id))
                    .count()
            })
            .unwrap_or_default();
        let directory_loading = topology_revision.is_some_and(|revision| {
            self.topology_directory_request
                .as_ref()
                .is_some_and(|request| {
                    request.generation == self.topology_directory_generation
                        && request.network_id == network_id
                        && request.topology_revision == revision
                })
        });
        let directory_error = self.topology_directory_error_for_network(&network_id);
        let directory_stale = self.topology_directory_is_stale_for_network(&network_id);
        let directory_ready = self.topology_directory_for_network(&network_id);
        let governance_freeze =
            coordinator_governance_freeze_reason(&self.snapshot, self.selected_network.as_deref());
        let pending = self.pending_topology_lifecycle.as_ref();
        let mut body = div()
            .id("topology-action-panel-body")
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(COLOR_TEXT))
                            .child(t(self.locale, "topology.batch.title").to_string()),
                    )
                    .child(div().text_xs().text_color(rgba(COLOR_MUTED)).child(format!(
                        "{}: {network_label}",
                        t(self.locale, "topology.batch.network")
                    )))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgba(COLOR_TEXT_SECONDARY))
                            .child(tr(
                                self.locale,
                                "topology.batch.selection_count",
                                &[
                                    ("selected", &self.topology_selection.len().to_string()),
                                    ("visible", &visible_count.to_string()),
                                ],
                            )),
                    ),
            );
        if let Some(notice) = self.coordinator_governance_notice() {
            body = body.child(notice);
        }

        if directory_loading {
            body = body.child(
                div()
                    .debug_selector(|| "visual-topology-directory-loading".into())
                    .text_xs()
                    .text_color(rgba(COLOR_MUTED))
                    .child(t(self.locale, "topology.batch.directory_loading").to_string()),
            );
        } else if let Some(error) = directory_error {
            body = body.child(
                div()
                    .debug_selector(|| "visual-topology-directory-failed".into())
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_xs()
                    .text_color(rgba(COLOR_WARNING))
                    .child(t(self.locale, "topology.batch.directory_failed").to_string())
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(rgba(COLOR_MUTED))
                            .child(error.to_string()),
                    ),
            );
        } else if directory_stale {
            body = body.child(
                div()
                    .debug_selector(|| "visual-topology-directory-stale".into())
                    .text_xs()
                    .text_color(rgba(COLOR_WARNING))
                    .child(t(self.locale, "topology.batch.directory_stale").to_string()),
            );
        } else if let Some(directory) = directory_ready {
            let actions = if governance_freeze.is_some() {
                BTreeSet::new()
            } else {
                self.topology_selection
                    .iter()
                    .flat_map(|node_id| {
                        topology_member_actions(
                            Some(directory),
                            Some(network_id.as_str()),
                            node_id.as_str(),
                        )
                    })
                    .collect::<BTreeSet<_>>()
            };
            body = body.child(
                div()
                    .debug_selector(|| "visual-topology-directory-ready".into())
                    .text_xs()
                    .text_color(rgba(COLOR_MUTED))
                    .child(if actions.is_empty() {
                        t(self.locale, "topology.batch.no_actions").to_string()
                    } else {
                        actions
                            .iter()
                            .map(|action| t(self.locale, member_lifecycle_action_key(*action)))
                            .collect::<Vec<_>>()
                            .join(t(self.locale, "common.list_separator"))
                    }),
            );
        }

        if let Some(pending) = pending {
            let action = t(self.locale, member_lifecycle_action_key(pending.action)).to_string();
            body = body
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .rounded_md()
                        .bg(rgba(DARK_THEME.colors.warning_soft.rgba_hex()))
                        .p_3()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight(600.0))
                                .text_color(rgba(COLOR_WARNING))
                                .child(t(self.locale, "topology.batch.confirm_title").to_string()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgba(COLOR_TEXT_SECONDARY))
                                .child(format!("{action} · {}", pending.node_id)),
                        )
                        .child(div().text_xs().text_color(rgba(COLOR_MUTED)).child(tr(
                            self.locale,
                            "topology.batch.confirm_counts",
                            &[("selected", "1"), ("eligible", "1"), ("skipped", "0")],
                        ))),
                )
                .child(
                    button(
                        SharedString::from("topology-confirm-lifecycle"),
                        tr(self.locale, "topology.batch.execute", &[("count", "1")]),
                        ButtonVariant::Danger,
                        self.pending_commands > 0 || governance_freeze.is_some(),
                    )
                    .debug_selector(|| "visual-topology-confirm".into())
                    .w_full()
                    .on_click(cx.listener(|shell, _, _, cx| {
                        shell.confirm_topology_lifecycle(cx);
                    })),
                );
        }

        body.child(
            button(
                SharedString::from("topology-refresh-directory"),
                t(self.locale, "topology.batch.refresh_directory"),
                ButtonVariant::Secondary,
                self.pending_commands > 0,
            )
            .debug_selector(|| "visual-topology-refresh-directory".into())
            .w_full()
            .on_click(cx.listener(|shell, _, _, cx| {
                shell.refresh_topology_directory(cx);
            })),
        )
        .into_any_element()
    }

    fn topology_content(
        &mut self,
        layout: LayoutClass,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let signals = self.topology_presentation_for_current_network();
        let presentation = topology_presentation(self.snapshot.topology.as_ref(), signals);
        let state_card = self.topology_state_card(&presentation);
        let Some(topology) = self.snapshot.topology.clone() else {
            return div()
                .id("topology-unavailable")
                .flex()
                .flex_col()
                .gap_4()
                .child(state_card)
                .into_any_element();
        };
        if !presentation.can_render {
            return div()
                .id("topology-presentation-blocked")
                .flex()
                .flex_col()
                .gap_4()
                .child(state_card)
                .into_any_element();
        }
        let Some(network_id) = self.topology_network_id() else {
            return div()
                .id("topology-no-network")
                .flex()
                .flex_col()
                .gap_4()
                .child(state_card)
                .into_any_element();
        };
        let directory = self.topology_directory_for_network(&network_id).cloned();
        let boundary_buttons = topology
            .boundaries
            .iter()
            .enumerate()
            .map(|(index, boundary)| {
                let selected = boundary.network_id == network_id;
                let boundary_id = boundary.network_id.clone();
                let label = format!("{} · {}", boundary.name, boundary.member_count);
                button(
                    SharedString::from(format!("topology-network-{index}")),
                    label,
                    if selected {
                        ButtonVariant::Secondary
                    } else {
                        ButtonVariant::Ghost
                    },
                    false,
                )
                .debug_selector(move || format!("visual-topology-boundary-{index}"))
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.select_topology_network(boundary_id.clone(), cx);
                }))
            })
            .collect::<Vec<_>>();

        let connections = topology
            .connections
            .iter()
            .filter(|connection| connection.networks.contains(&network_id))
            .enumerate()
            .map(|(index, connection)| {
                let state_key = match connection.state {
                    TopologyConnectionState::Active => "topology.connection.active",
                    TopologyConnectionState::Idle => "topology.connection.idle",
                    TopologyConnectionState::Stale => "topology.connection.stale",
                };
                div()
                    .id(format!("topology-connection-{index}"))
                    .debug_selector(move || format!("visual-topology-connection-{index}"))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .rounded_md()
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(rgba(COLOR_TEXT_SECONDARY))
                    .child(format!("{} ↔ {}", connection.from, connection.to))
                    .child(
                        div()
                            .text_color(rgba(COLOR_MUTED))
                            .child(t(self.locale, state_key).to_string()),
                    )
            })
            .collect::<Vec<_>>();

        let visible_nodes = topology
            .nodes
            .iter()
            .filter(|node| node.belongs_to(&network_id))
            .cloned()
            .collect::<Vec<_>>();
        let generated_at_unix_ms = directory
            .as_ref()
            .map(|directory| directory.generated_at_unix_ms)
            .unwrap_or(topology.generated_at_unix_ms);
        let mut node_cards = Vec::new();
        for (index, node) in visible_nodes.iter().enumerate() {
            let member = directory.as_ref().and_then(|directory| {
                directory
                    .members
                    .iter()
                    .find(|member| member.node_id.as_str() == node.node_id.as_str())
            });
            let detail = topology_node_detail(
                node,
                Some(network_id.as_str()),
                member,
                self.locale,
                generated_at_unix_ms,
            );
            let selected = self.selected_topology_node_id.as_deref() == Some(node.node_id.as_str());
            let menu_open = self.topology_menu_node_id.as_deref() == Some(node.node_id.as_str());
            let node_id = node.node_id.as_str().to_string();
            let open_node_id = node_id.clone();
            let menu_node_id = node_id.clone();
            let hover_node_id = node_id.clone();
            let menu_actions = if command_coordinator_governance_freeze_reason(
                NodeCommandKind::ChangeMemberLifecycle,
                &self.snapshot,
                self.selected_network.as_deref(),
            )
            .is_some()
            {
                Vec::new()
            } else {
                topology_member_actions(
                    directory.as_ref(),
                    Some(network_id.as_str()),
                    node.node_id.as_str(),
                )
            };
            let selected_for_menu = self.topology_selection.contains(node.node_id.as_str());

            let mut card = div()
                .id(format!("topology-node-card-{index}"))
                .debug_selector(move || format!("visual-topology-node-{index}"))
                .flex()
                .flex_col()
                .gap_2()
                .rounded_lg()
                .bg(rgba(if selected {
                    DARK_THEME.colors.panel_raised.rgba_hex()
                } else {
                    COLOR_PANEL
                }))
                .p_3()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            button(
                                SharedString::from(format!("topology-open-{index}")),
                                format!("{} · {}", detail.display_name, detail.role),
                                ButtonVariant::Ghost,
                                false,
                            )
                            .debug_selector(move || format!("visual-topology-open-{index}"))
                            .flex_1()
                            .justify_start()
                            .on_click(cx.listener(
                                move |shell, _, _, cx| {
                                    shell.open_topology_node(open_node_id.clone(), cx);
                                },
                            )),
                        )
                        .child(
                            button(
                                SharedString::from(format!("topology-menu-{index}")),
                                "•••",
                                ButtonVariant::Ghost,
                                false,
                            )
                            .debug_selector(move || format!("visual-topology-menu-trigger-{index}"))
                            .aria_label(tr(
                                self.locale,
                                "topology.menu.label",
                                &[("name", &detail.display_name)],
                            ))
                            .on_click(cx.listener(
                                move |shell, _, _, cx| {
                                    shell.toggle_topology_menu(menu_node_id.clone(), cx);
                                },
                            )),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgba(COLOR_MUTED))
                        .child(format!("{} · {}", detail.node_id, detail.health)),
                )
                .on_hover(cx.listener(move |shell, is_hovered: &bool, _, cx| {
                    if shell.topology_menu_node_id.is_some() {
                        return;
                    }
                    let next = is_hovered.then(|| hover_node_id.clone());
                    if shell.hovered_topology_node_id != next {
                        shell.hovered_topology_node_id = next;
                        cx.notify();
                    }
                }));

            let hovered = self.hovered_topology_node_id.as_deref() == Some(node.node_id.as_str())
                && !menu_open;
            if hovered {
                let hover_field =
                    |label_key: &'static str, value: String, selector: &'static str| {
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                div()
                                    .w(px(88.0))
                                    .flex_shrink_0()
                                    .text_size(px(10.0))
                                    .text_color(rgba(COLOR_MUTED))
                                    .child(t(self.locale, label_key).to_string()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .debug_selector(move || selector.into())
                                    .text_xs()
                                    .text_color(rgba(COLOR_TEXT_SECONDARY))
                                    .child(value),
                            )
                    };
                card = card.child(
                    div()
                        .id("visual-topology-hover")
                        .debug_selector(|| "visual-topology-hover".into())
                        .role(Role::Article)
                        .aria_label(format!(
                            "{} · {} · {}: {} · {}: {}",
                            detail.display_name,
                            detail.health,
                            t(self.locale, "topology.hover.role"),
                            detail.role,
                            t(self.locale, "topology.hover.capabilities"),
                            detail.capabilities
                        ))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .rounded_md()
                        .bg(rgba(COLOR_PANEL_RAISED))
                        .w_full()
                        .max_w(px(340.0))
                        .min_w(px(0.0))
                        .p_3()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    div()
                                        .min_w(px(0.0))
                                        .debug_selector(|| "visual-topology-hover-name".into())
                                        .text_sm()
                                        .font_weight(gpui::FontWeight(600.0))
                                        .text_color(rgba(COLOR_TEXT))
                                        .child(detail.display_name.clone()),
                                )
                                .child(
                                    div()
                                        .debug_selector(|| {
                                            "visual-topology-hover-health-badge".into()
                                        })
                                        .text_xs()
                                        .text_color(rgba(COLOR_TEXT_SECONDARY))
                                        .child(detail.health.clone()),
                                ),
                        )
                        .child(hover_field(
                            "topology.hover.role",
                            detail.role.clone(),
                            "visual-topology-hover-role",
                        ))
                        .child(hover_field(
                            "topology.hover.capabilities",
                            detail.capabilities.clone(),
                            "visual-topology-hover-capabilities",
                        ))
                        .child(hover_field(
                            "topology.hover.health",
                            format!("{} · {}", detail.health, detail.presence),
                            "visual-topology-hover-health",
                        ))
                        .child(
                            div()
                                .debug_selector(|| "visual-topology-hover-hint".into())
                                .text_size(px(10.0))
                                .text_color(rgba(COLOR_MUTED))
                                .child(t(self.locale, "topology.hover.hint").to_string()),
                        ),
                );
            }

            if menu_open {
                let mut menu = div()
                    .id(format!("topology-menu-{index}"))
                    .debug_selector(|| "visual-topology-menu".into())
                    .flex()
                    .flex_col()
                    .gap_1()
                    .rounded_md()
                    .bg(rgba(COLOR_PANEL_RAISED))
                    .p_2();
                let open_detail_node = node_id.clone();
                menu = menu.child(
                    button(
                        SharedString::from(format!("topology-menu-open-{index}")),
                        t(self.locale, "topology.menu.open"),
                        ButtonVariant::Ghost,
                        false,
                    )
                    .debug_selector(|| "visual-topology-menu-open".into())
                    .w_full()
                    .on_click(cx.listener(move |shell, _, _, cx| {
                        shell.open_topology_node(open_detail_node.clone(), cx);
                        shell.topology_menu_node_id = None;
                        cx.notify();
                    })),
                );
                let select_node = node_id.clone();
                menu = menu.child(
                    button(
                        SharedString::from(format!("topology-menu-select-{index}")),
                        t(
                            self.locale,
                            if selected_for_menu {
                                "topology.menu.unselect"
                            } else {
                                "topology.menu.select"
                            },
                        ),
                        ButtonVariant::Ghost,
                        false,
                    )
                    .debug_selector(move || {
                        if selected_for_menu {
                            "visual-topology-menu-unselect".into()
                        } else {
                            "visual-topology-menu-select".into()
                        }
                    })
                    .w_full()
                    .on_click(cx.listener(move |shell, _, _, cx| {
                        shell.toggle_topology_selection(select_node.clone(), cx);
                        shell.topology_menu_node_id = None;
                        cx.notify();
                    })),
                );
                if menu_actions.is_empty() {
                    menu = menu.child(
                        div()
                            .debug_selector(|| "visual-topology-menu-no-actions".into())
                            .px_2()
                            .py_1()
                            .text_xs()
                            .text_color(rgba(COLOR_MUTED))
                            .child(t(self.locale, "topology.menu.no_actions").to_string()),
                    );
                } else {
                    for action in menu_actions {
                        let action_node = node_id.clone();
                        menu = menu.child(
                            button(
                                SharedString::from(format!(
                                    "topology-menu-action-{}-{index}",
                                    member_lifecycle_action_value(action)
                                )),
                                t(self.locale, member_lifecycle_action_key(action)),
                                ButtonVariant::Ghost,
                                false,
                            )
                            .debug_selector(move || {
                                format!(
                                    "visual-topology-menu-action-{}",
                                    member_lifecycle_action_value(action)
                                )
                            })
                            .w_full()
                            .on_click(cx.listener(
                                move |shell, _, _, cx| {
                                    shell.request_topology_lifecycle(
                                        action_node.clone(),
                                        action,
                                        cx,
                                    );
                                    shell.topology_menu_node_id = None;
                                    cx.notify();
                                },
                            )),
                        );
                    }
                }
                card = card.child(menu);
            }
            node_cards.push(card);
        }

        let detail_card = self.topology_detail_card(&topology, &network_id, directory.as_ref());
        let compact = matches!(layout, LayoutClass::Mobile | LayoutClass::Compact);
        let mut content = div()
            .id("topology-content")
            .flex()
            .flex_col()
            .gap_4()
            .child(state_card)
            .child(div().flex().flex_wrap().gap_2().children(boundary_buttons))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(COLOR_TEXT))
                            .child(t(self.locale, "topology.legend.connections").to_string()),
                    )
                    .children(connections),
            );
        if compact {
            content = content
                .child(div().flex().flex_col().gap_2().children(node_cards))
                .child(detail_card);
        } else {
            content = content.child(
                div()
                    .flex()
                    .items_start()
                    .gap_4()
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .children(node_cards),
                    )
                    .child(div().w(px(360.0)).flex_shrink_0().child(detail_card)),
            );
        }
        content.into_any_element()
    }

    fn page_content(&mut self, layout: LayoutClass, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(manifest) = self.manifest.as_ref() else {
            return div()
                .id(app_region_id(AppRegion::Content))
                .track_focus(&self.content_focus_handle)
                .flex_1()
                .min_w(px(0.0))
                .child(loading_skeleton())
                .into_any_element();
        };
        let activity_filters = self.effective_activity_filters();
        let model = match build_page_model_with_activity_filters(
            manifest,
            &self.snapshot,
            &self.selected_page,
            self.locale,
            &activity_filters,
        ) {
            Ok(model) => model,
            Err(error) => {
                return div()
                    .id(app_region_id(AppRegion::Content))
                    .track_focus(&self.content_focus_handle)
                    .flex_1()
                    .min_w(px(0.0))
                    .child(component_error_state(
                        "page-mapping-error",
                        t(self.locale, "common.page_mapping_failed"),
                        error.to_string(),
                    ))
                    .into_any_element();
            }
        };
        let selected_page = manifest
            .pages
            .iter()
            .find(|page| page.id == self.selected_page);
        let page_title = selected_page
            .map(|page| t(self.locale, &page.title_key).to_string())
            .unwrap_or_else(|| t(self.locale, "common.unknown_page").to_string());
        let has_configuration_actions = selected_page.is_some_and(|page| {
            page.actions
                .iter()
                .any(|action| is_configuration_action(&action.id))
        });
        let page_icon = lucide_icon(page_icon_data(&self.selected_page), 20.0, COLOR_ACCENT);

        let mut content = div()
            .id(app_region_id(AppRegion::Content))
            .debug_selector(|| "visual-content".into())
            .track_focus(&self.content_focus_handle)
            .role(Role::Article)
            .aria_label(page_title.clone())
            .flex()
            .flex_col()
            .gap_5()
            .min_w(px(0.0))
            .flex_1()
            .self_stretch()
            .overflow_y_scroll()
            .child(
                div()
                    .id("page-heading")
                    .debug_selector(|| "visual-page-heading".into())
                    .role(A11Y_HEADING_ROLE)
                    .aria_level(1)
                    .aria_label(page_title.clone())
                    .flex()
                    .items_center()
                    .gap_3()
                    .h(px(44.0))
                    .child(
                        div()
                            .w(px(40.0))
                            .h(px(40.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .bg(rgba(DARK_THEME.colors.surface.rgba_hex()))
                            .text_color(rgba(COLOR_ACCENT))
                            .child(page_icon),
                    )
                    .child(
                        div().flex().flex_1().flex_col().min_w(px(0.0)).child(
                            div()
                                .text_size(px(20.0))
                                .line_height(px(24.0))
                                .font_weight(gpui::FontWeight(600.0))
                                .text_color(rgba(COLOR_TEXT))
                                .child(page_title),
                        ),
                    ),
            );

        if let Some(overview) = model.overview.as_ref() {
            return content
                .child(self.overview_content(overview, cx))
                .into_any_element();
        }

        if self.selected_page == "topology" {
            return content
                .child(self.topology_content(layout, cx))
                .into_any_element();
        }

        if self.selected_page == "networks" {
            return content
                .child(self.networks_content(layout, cx))
                .into_any_element();
        }

        if self.selected_page == "node_runtime" || self.selected_page == "exit_node" {
            content = content.child(self.operation_status_card(cx));
            content = content.child(self.diagnostics_card(cx));
        }

        if !model.metrics.is_empty() {
            content = content.child(
                div().flex().flex_wrap().gap_3().children(
                    model
                        .metrics
                        .iter()
                        .map(|metric| metric_card(&metric.label, &metric.value, &metric.source)),
                ),
            );
        }
        if self.selected_page == "activity" {
            content = content.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(self.activity_sync_notices()),
            );
            content = content.child(self.activity_filters_card(cx));
        }
        if self.selected_page == "routing" {
            content = content.child(self.routing_target_search_card(cx));
            content = content.child(self.routing_authority_notices());
        }
        let section_cards = model
            .sections
            .iter()
            .map(|section| self.section_card(section, cx))
            .collect::<Vec<_>>();
        content = content.child(div().flex().flex_col().gap_4().children(section_cards));
        if has_configuration_actions {
            content = content.child(self.configuration_workspace(cx));
        }

        content.into_any_element()
    }

    fn effective_activity_filters(&self) -> ActivityFilters {
        let mut filters = self.activity_filters.clone();
        if let Some(duration) = self.activity_time_window.duration_ms() {
            let now = unix_time_ms();
            filters.from_unix_ms = Some(now.saturating_sub(duration));
            filters.to_unix_ms = Some(now);
        } else {
            filters.from_unix_ms = None;
            filters.to_unix_ms = None;
        }
        filters
    }

    fn has_activity_filters(&self) -> bool {
        self.activity_time_window != ActivityTimeWindow::All
            || self.activity_filters.network_id.is_some()
            || self.activity_filters.object_kind.is_some()
            || self.activity_filters.object_id.is_some()
            || self.activity_filters.actor_role.is_some()
            || self.activity_filters.category.is_some()
            || self.activity_filters.severity.is_some()
            || self.activity_filters.outcome.is_some()
    }

    fn routing_target_search_card(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let query = self.routing_target_query.trim().to_owned();
        let rows = if query.is_empty() {
            Vec::new()
        } else {
            routing_target_search_rows(&self.snapshot, &query, self.locale)
        };
        let search_input = self
            .routing_target_search_input
            .as_ref()
            .map(|input| input.clone().into_any_element());
        let clear_search = (!query.is_empty()).then(|| {
            button(
                SharedString::from("clear-routing-target-search"),
                "",
                ButtonVariant::Ghost,
                false,
            )
            .absolute()
            .right(px(4.0))
            .top(px(3.0))
            .w(px(28.0))
            .h(px(28.0))
            .px_0()
            .aria_label(t(self.locale, "common.search_clear").to_string())
            .child(lucide_icon(
                include_bytes!("../../assets/icons/x.svg"),
                14.0,
                COLOR_MUTED,
            ))
            .on_click(cx.listener(|shell, _, _, cx| shell.clear_routing_target_search(cx)))
        });
        let search_control = div()
            .id("routing-target-search")
            .debug_selector(|| "visual-routing-target-search-control".into())
            .relative()
            .flex()
            .items_center()
            .w_full()
            .h(px(34.0))
            .child(
                div()
                    .absolute()
                    .left(px(11.0))
                    .top(px(9.0))
                    .child(lucide_icon(
                        include_bytes!("../../assets/icons/search.svg"),
                        15.0,
                        COLOR_FAINT,
                    )),
            )
            .children(search_input)
            .children(clear_search);

        let mut results = Vec::new();
        if !query.is_empty() {
            if rows.is_empty() {
                results.push(
                    empty_state(t(self.locale, "traffic.routing.targets.no_results"))
                        .debug_selector(|| "visual-routing-target-search-empty".into())
                        .into_any_element(),
                );
            } else {
                results.extend(rows.iter().enumerate().map(|(index, row)| {
                    div()
                        .debug_selector(move || format!("visual-routing-target-result-{index}"))
                        .child(self.row_card_with_navigation_selector(
                            row,
                            Some(routing_target_result_navigation_selector(index)),
                            cx,
                        ))
                        .into_any_element()
                }));
            }
        }

        surface_card()
            .id("routing-target-search")
            .debug_selector(|| "visual-routing-target-search".into())
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_base()
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(COLOR_TEXT))
                            .child(t(self.locale, "traffic.routing.targets.search").to_string()),
                    )
                    .child(
                        div().text_xs().text_color(rgba(COLOR_MUTED)).child(
                            t(self.locale, "traffic.routing.targets.search_hint").to_string(),
                        ),
                    ),
            )
            .child(search_control)
            .children(results)
            .into_any_element()
    }

    fn routing_authority_notices(&self) -> gpui::AnyElement {
        div()
            .debug_selector(|| "visual-routing-authority-notices".into())
            .flex()
            .flex_col()
            .gap_2()
            .child(activity_sync_notice(
                "visual-routing-authority-notice",
                t(self.locale, "gpui.routing.authority_notice").to_string(),
                t(self.locale, "traffic.routing.authority.unavailable_detail").to_string(),
                SemanticState::Info,
            ))
            .child(activity_sync_notice(
                "visual-routing-backend-notice",
                t(self.locale, "gpui.routing.backend_notice").to_string(),
                t(self.locale, "blocked.backend_unavailable").to_string(),
                SemanticState::Warning,
            ))
            .into_any_element()
    }

    fn activity_sync_notices(&self) -> Vec<gpui::AnyElement> {
        let mut notices = Vec::new();
        if !self
            .snapshot
            .permissions
            .permissions
            .contains(&NodePermission::ViewState)
        {
            notices.push(activity_sync_notice(
                "visual-activity-sync-permission-denied",
                t(self.locale, "activity.sync.permission_denied.title").to_string(),
                t(self.locale, "activity.sync.permission_denied.detail").to_string(),
                SemanticState::Danger,
            ));
        }
        if self.snapshot.control_health == NodeControlHealth::Unavailable {
            notices.push(activity_sync_notice(
                "visual-activity-sync-stale",
                t(self.locale, "activity.sync.stale.title").to_string(),
                t(self.locale, "activity.sync.stale.detail").to_string(),
                SemanticState::Warning,
            ));
        }
        if self.pending_commands > 0 {
            notices.push(activity_sync_notice(
                "visual-activity-sync-refreshing",
                t(self.locale, "activity.sync.refreshing.title").to_string(),
                t(self.locale, "activity.sync.refreshing.detail").to_string(),
                SemanticState::Info,
            ));
        }
        notices
    }

    fn activity_filters_card(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let options = activity_filter_options(&self.snapshot.recent_activity);
        let filter_group = |title: SharedString, controls: Vec<gpui::AnyElement>| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .min_w(px(150.0))
                .flex_1()
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui::FontWeight(600.0))
                        .text_color(rgba(COLOR_MUTED))
                        .child(title),
                )
                .child(div().flex().flex_wrap().gap_2().children(controls))
        };
        let all_label = t(self.locale, "activity.filter.all").to_string();

        let mut network_controls = Vec::new();
        for network_id in std::iter::once(None).chain(options.networks.into_iter().map(Some)) {
            let selected = self.activity_filters.network_id == network_id;
            let value = network_id.clone().unwrap_or_else(|| "all".into());
            let label = network_id
                .as_deref()
                .map(|network_id| activity_filter_network_label(&self.snapshot, network_id))
                .unwrap_or_else(|| all_label.clone());
            network_controls.push(
                choice_control(
                    SharedString::from(format!("activity-filter-network-{value}")),
                    label,
                    selected,
                )
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.activity_filters.network_id = network_id.clone();
                    cx.notify();
                }))
                .into_any_element(),
            );
        }

        let mut object_kind_controls = Vec::new();
        for object_kind in std::iter::once(None).chain(options.object_kinds.into_iter().map(Some)) {
            let selected = self.activity_filters.object_kind == object_kind;
            let value = object_kind.clone().unwrap_or_else(|| "all".into());
            let label = object_kind
                .as_deref()
                .map(|kind| activity_filter_object_kind_label(self.locale, kind))
                .unwrap_or_else(|| all_label.clone());
            object_kind_controls.push(
                choice_control(
                    SharedString::from(format!("activity-filter-object-kind-{value}")),
                    label,
                    selected,
                )
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.activity_filters.object_kind = object_kind.clone();
                    cx.notify();
                }))
                .into_any_element(),
            );
        }

        let mut object_id_controls = Vec::new();
        for object_id in std::iter::once(None).chain(options.object_ids.into_iter().map(Some)) {
            let selected = self.activity_filters.object_id == object_id;
            let value = object_id.clone().unwrap_or_else(|| "all".into());
            let label = object_id.clone().unwrap_or_else(|| all_label.clone());
            object_id_controls.push(
                choice_control(
                    SharedString::from(format!("activity-filter-object-id-{value}")),
                    label,
                    selected,
                )
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.activity_filters.object_id = object_id.clone();
                    cx.notify();
                }))
                .into_any_element(),
            );
        }

        let mut actor_role_controls = Vec::new();
        for actor_role in std::iter::once(None).chain(options.actor_roles.into_iter().map(Some)) {
            let selected = self.activity_filters.actor_role == actor_role;
            let value = actor_role.clone().unwrap_or_else(|| "all".into());
            let label = actor_role
                .as_deref()
                .map(|role| activity_filter_role_label(self.locale, role))
                .unwrap_or_else(|| all_label.clone());
            actor_role_controls.push(
                choice_control(
                    SharedString::from(format!("activity-filter-actor-role-{value}")),
                    label,
                    selected,
                )
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.activity_filters.actor_role = actor_role.clone();
                    cx.notify();
                }))
                .into_any_element(),
            );
        }

        let mut category_controls = Vec::new();
        for category in [
            None,
            Some(ActivityCategory::UserActivity),
            Some(ActivityCategory::SystemEvent),
            Some(ActivityCategory::SecurityEvent),
            Some(ActivityCategory::OperationFailure),
        ] {
            let selected = self.activity_filters.category == category;
            let value = category.map(activity_category_value).unwrap_or("all");
            let label = category
                .map(|category| t(self.locale, activity_category_key(category)).to_string())
                .unwrap_or_else(|| all_label.clone());
            category_controls.push(
                choice_control(
                    SharedString::from(format!("activity-filter-category-{value}")),
                    label,
                    selected,
                )
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.activity_filters.category = category;
                    cx.notify();
                }))
                .into_any_element(),
            );
        }

        let mut severity_controls = Vec::new();
        for severity in [
            None,
            Some(ActivitySeverity::Info),
            Some(ActivitySeverity::Notice),
            Some(ActivitySeverity::Warning),
            Some(ActivitySeverity::Critical),
        ] {
            let selected = self.activity_filters.severity == severity;
            let value = severity.map(activity_severity_value).unwrap_or("all");
            let label = severity
                .map(|severity| t(self.locale, activity_severity_key(severity)).to_string())
                .unwrap_or_else(|| all_label.clone());
            severity_controls.push(
                choice_control(
                    SharedString::from(format!("activity-filter-severity-{value}")),
                    label,
                    selected,
                )
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.activity_filters.severity = severity;
                    cx.notify();
                }))
                .into_any_element(),
            );
        }

        let mut outcome_controls = Vec::new();
        for outcome in [
            None,
            Some(AuditOutcome::Succeeded),
            Some(AuditOutcome::Failed),
            Some(AuditOutcome::Denied),
        ] {
            let selected = self.activity_filters.outcome == outcome;
            let value = outcome.map(activity_outcome_value).unwrap_or("all");
            let label = outcome
                .map(|outcome| t(self.locale, activity_outcome_key(outcome)).to_string())
                .unwrap_or_else(|| all_label.clone());
            outcome_controls.push(
                choice_control(
                    SharedString::from(format!("activity-filter-outcome-{value}")),
                    label,
                    selected,
                )
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.activity_filters.outcome = outcome;
                    cx.notify();
                }))
                .into_any_element(),
            );
        }

        let mut time_controls = Vec::new();
        for window in [
            ActivityTimeWindow::All,
            ActivityTimeWindow::LastHour,
            ActivityTimeWindow::Last24Hours,
            ActivityTimeWindow::Last7Days,
        ] {
            let value = activity_time_window_value(window);
            time_controls.push(
                choice_control(
                    SharedString::from(format!("activity-filter-time-{value}")),
                    t(self.locale, activity_time_window_key(window)).to_string(),
                    self.activity_time_window == window,
                )
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.activity_time_window = window;
                    cx.notify();
                }))
                .into_any_element(),
            );
        }

        let has_filters = self.has_activity_filters();
        surface_card()
            .id("activity-filters")
            .debug_selector(|| "visual-activity-filters".into())
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(gpui::FontWeight(600.0))
                                    .text_color(rgba(COLOR_TEXT))
                                    .child(t(self.locale, "activity.filters").to_string()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgba(COLOR_MUTED))
                                    .child(t(self.locale, "activity.description").to_string()),
                            ),
                    )
                    .child(
                        button(
                            "activity-filter-clear".into(),
                            t(self.locale, "activity.filter.clear"),
                            ButtonVariant::Secondary,
                            !has_filters,
                        )
                        .debug_selector(|| "visual-activity-filter-clear".into())
                        .on_click(cx.listener(move |shell, _, _, cx| {
                            shell.activity_filters = ActivityFilters::default();
                            shell.activity_time_window = ActivityTimeWindow::All;
                            cx.notify();
                        })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .child(filter_group(
                        t(self.locale, "activity.filter.network").to_string().into(),
                        network_controls,
                    ))
                    .child(filter_group(
                        t(self.locale, "activity.filter.object_kind")
                            .to_string()
                            .into(),
                        object_kind_controls,
                    ))
                    .child(filter_group(
                        t(self.locale, "activity.filter.object_id")
                            .to_string()
                            .into(),
                        object_id_controls,
                    ))
                    .child(filter_group(
                        t(self.locale, "activity.filter.actor_role")
                            .to_string()
                            .into(),
                        actor_role_controls,
                    ))
                    .child(filter_group(
                        t(self.locale, "activity.filter.category")
                            .to_string()
                            .into(),
                        category_controls,
                    ))
                    .child(filter_group(
                        t(self.locale, "activity.filter.severity")
                            .to_string()
                            .into(),
                        severity_controls,
                    ))
                    .child(filter_group(
                        t(self.locale, "activity.filter.outcome").to_string().into(),
                        outcome_controls,
                    ))
                    .child(filter_group(
                        t(self.locale, "activity.filter.time").to_string().into(),
                        time_controls,
                    )),
            )
            .into_any_element()
    }

    fn action_card(&self, action: NodeUiAction, cx: &mut Context<Self>) -> gpui::AnyElement {
        let guided_join_request_review = self.recent_join_requests.is_some()
            && matches!(
                action.id.as_str(),
                "review_join_request" | "batch_review_join_requests"
            );
        let block_reason =
            complete_action_block_reason(&action, &self.snapshot, self.selected_network.as_deref());
        let blocked = block_reason.is_some();
        let confirmation_pending = self.pending_confirmation.as_deref() == Some(action.id.as_str());
        let unavailable = action.backend_status == NodeUiBackendStatus::Unavailable;
        let destructive = action
            .confirmation
            .as_ref()
            .is_some_and(|confirmation| confirmation.destructive);
        let direct_read = !action.command.is_write()
            && action.fields.is_empty()
            && action.confirmation.is_none()
            && !unavailable
            && !blocked;
        let expanded = !direct_read
            && self
                .expanded_actions
                .get(&action.id)
                .copied()
                .unwrap_or(false);
        let disabled = manifest_action_submit_disabled(&action, self.pending_commands, blocked);
        let button_variant = if destructive || confirmation_pending {
            ButtonVariant::Danger
        } else {
            ButtonVariant::Primary
        };
        let action_label = manifest_action_label(self.locale, &action);
        let button_label = manifest_action_cta_label(self.locale, &action, confirmation_pending);
        let fields = action
            .fields
            .iter()
            .filter(|field| {
                if !guided_join_request_review {
                    return true;
                }
                match action.id.as_str() {
                    "review_join_request" => !matches!(
                        field.id.as_str(),
                        "network_id" | "request_id" | "expected_revision" | "action"
                    ),
                    "batch_review_join_requests" => {
                        !matches!(field.id.as_str(), "network_id" | "reviews")
                    }
                    _ => true,
                }
            })
            .map(|field| self.field_editor(&action, field, cx))
            .collect::<Vec<_>>();
        let join_preview_card = (action.id == "join_network")
            .then_some(self.recent_network_join_preview.as_ref())
            .flatten()
            .map(|preview| self.join_preview_card(preview, cx));
        let join_requests_card = (action.id == "list_join_requests")
            .then_some(self.recent_join_requests.as_ref())
            .flatten()
            .cloned()
            .map(|result| self.join_requests_card(&result, cx));
        let network_members_card = (action.id == "list_network_members")
            .then_some(self.recent_network_members.as_ref())
            .flatten()
            .map(|directory| self.member_directory_card(directory, cx));
        let join_request_review_summary =
            guided_join_request_review.then(|| self.join_request_review_form_summary(&action.id));
        let issued_invitation_card = (action.id == "create_invitation")
            .then_some(self.recent_issued_invitation.as_ref())
            .flatten()
            .map(|issued| self.issued_invitation_card(issued, cx));
        let invitations_card = (action.id == "list_invitations")
            .then_some(self.recent_invitations.as_deref())
            .flatten()
            .map(|invitations| self.invitations_card(invitations, cx));
        let invitation_status_card = matches!(
            action.id.as_str(),
            "get_invitation_status" | "revoke_invitation"
        )
        .then_some(self.recent_invitation_status.as_ref())
        .flatten()
        .map(|status| self.invitation_status_card(status, cx));
        let invitation_preview_card = (action.id == "preview_invitation")
            .then_some(self.recent_invitation_preview.as_ref())
            .flatten()
            .map(|preview| self.invitation_preview_card(preview, cx));
        let has_custom_body = invitation_preview_card.is_some()
            || join_preview_card.is_some()
            || join_requests_card.is_some()
            || network_members_card.is_some()
            || join_request_review_summary.is_some()
            || issued_invitation_card.is_some()
            || invitations_card.is_some()
            || invitation_status_card.is_some();

        let confirmation_notice = action.confirmation.as_ref().map(|confirmation| {
            let title = t(self.locale, &confirmation.title_key).to_string();
            let consequence = t(self.locale, &confirmation.consequence_key).to_string();
            let recent_authentication = confirmation
                .recent_authentication_required
                .then(|| t(self.locale, "confirm.reauthentication_required").to_string());
            let selector = format!("visual-action-confirmation-{}", action.id);
            div()
                .debug_selector(move || selector)
                .flex()
                .flex_col()
                .gap_1()
                .rounded_md()
                .p_3()
                .bg(rgba(DARK_THEME.colors.panel_raised.rgba_hex()))
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui::FontWeight(600.0))
                        .text_color(rgba(if confirmation.destructive {
                            DARK_THEME.colors.danger_text.rgba_hex()
                        } else {
                            COLOR_TEXT
                        }))
                        .child(title),
                )
                .child(
                    div()
                        .text_xs()
                        .line_height(px(18.0))
                        .text_color(rgba(COLOR_TEXT_SECONDARY))
                        .child(consequence),
                )
                .children(recent_authentication.map(|message| {
                    div()
                        .text_xs()
                        .line_height(px(18.0))
                        .text_color(rgba(COLOR_MUTED))
                        .child(message)
                }))
                .into_any_element()
        });
        let unavailable_notice = unavailable.then(|| {
            let selector = format!("visual-action-unavailable-{}", action.id);
            div()
                .debug_selector(move || selector)
                .flex()
                .flex_col()
                .gap_1()
                .rounded_md()
                .p_3()
                .bg(rgba(DARK_THEME.colors.panel_raised.rgba_hex()))
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui::FontWeight(600.0))
                        .text_color(rgba(COLOR_WARNING))
                        .child(t(self.locale, "status.backend.unavailable").to_string()),
                )
                .child(
                    div()
                        .text_xs()
                        .line_height(px(18.0))
                        .text_color(rgba(COLOR_TEXT_SECONDARY))
                        .child(t(self.locale, "common.action_unavailable_detail").to_string()),
                )
                .into_any_element()
        });

        let direct_action_id = action.id.clone();
        let toggle_action_id = action.id.clone();
        let head_selector = format!("visual-action-head-{}", action.id);
        let mut card = div()
            .id(SharedString::from(format!("action-card-{}", action.id)))
            .flex()
            .flex_col()
            .w_full()
            .overflow_hidden()
            .rounded_lg()
            .bg(rgba(COLOR_PANEL_RAISED))
            .when(unavailable, |card| card.opacity(0.72))
            .child(
                div()
                    .id(SharedString::from(format!(
                        "action-card-head-{}",
                        action.id
                    )))
                    .when(action.id == "refresh_snapshot", |head| {
                        head.debug_selector(|| "visual-action-first".into())
                    })
                    .debug_selector(move || head_selector)
                    .role(Role::Button)
                    .aria_label(action_label.clone())
                    .aria_expanded(expanded)
                    .focusable()
                    .tab_stop(true)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .min_h(px(46.0))
                    .px_4()
                    .py_2()
                    .text_sm()
                    .text_color(rgba(COLOR_TEXT))
                    .on_click(cx.listener(move |shell, _, _, cx| {
                        if direct_read {
                            shell.submit_manifest_action(&direct_action_id, cx);
                            return;
                        }
                        let currently_expanded = shell
                            .expanded_actions
                            .get(&toggle_action_id)
                            .copied()
                            .unwrap_or(false);
                        if currently_expanded
                            && shell.pending_confirmation.as_deref()
                                == Some(toggle_action_id.as_str())
                        {
                            shell.pending_confirmation = None;
                        }
                        if currently_expanded {
                            shell.expanded_actions.remove(&toggle_action_id);
                        } else {
                            shell.expanded_actions.clear();
                            shell
                                .expanded_actions
                                .insert(toggle_action_id.clone(), true);
                        }
                        if currently_expanded && toggle_action_id == "list_network_members" {
                            shell.recent_network_members = None;
                            shell.selected_member_node_id = None;
                        }
                        cx.notify();
                    }))
                    .child(action_label)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .when(unavailable, |row| {
                                row.child(
                                    div()
                                        .rounded_md()
                                        .px_2()
                                        .py_1()
                                        .text_size(px(10.0))
                                        .font_weight(gpui::FontWeight(600.0))
                                        .text_color(rgba(COLOR_WARNING))
                                        .bg(rgba(DARK_THEME.colors.warning_soft.rgba_hex()))
                                        .child(
                                            t(self.locale, "status.backend.unavailable")
                                                .to_string(),
                                        ),
                                )
                            })
                            .when(!unavailable && action.confirmation.is_some(), |row| {
                                row.child(
                                    div()
                                        .rounded_md()
                                        .px_2()
                                        .py_1()
                                        .text_size(px(10.0))
                                        .text_color(rgba(if destructive {
                                            DARK_THEME.colors.danger_text.rgba_hex()
                                        } else {
                                            COLOR_TEXT_SECONDARY
                                        }))
                                        .bg(rgba(DARK_THEME.colors.panel_raised.rgba_hex()))
                                        .child(
                                            t(self.locale, "common.confirmation_required")
                                                .to_string(),
                                        ),
                                )
                            })
                            .child(if direct_read {
                                "→"
                            } else if expanded {
                                "⌄"
                            } else {
                                "›"
                            }),
                    ),
            );

        if expanded {
            let mut body = div().flex().flex_col().gap_3().px_4().pb_4();
            if has_custom_body {
                if let Some(requests_card) = join_requests_card {
                    body = body.child(requests_card);
                }
                if let Some(members_card) = network_members_card {
                    body = body.child(members_card);
                }
                if let Some(review_summary) = join_request_review_summary {
                    body = body.child(review_summary);
                }
                if let Some(issued_card) = issued_invitation_card {
                    body = body.child(issued_card);
                }
                if let Some(invitations_card) = invitations_card {
                    body = body.child(invitations_card);
                }
                if let Some(status_card) = invitation_status_card {
                    body = body.child(status_card);
                }
                if let Some(preview_card) = invitation_preview_card {
                    body = body.child(preview_card);
                } else if let Some(preview_card) = join_preview_card {
                    // A discovery-backed join must always pass through the
                    // signed preview and explicit terms acknowledgement. Hide
                    // generic ticket fields from the final confirmation path.
                    body = body.child(preview_card);
                }
                body = body.children(block_reason.map(|reason| {
                    let action_label = manifest_action_label(self.locale, &action);
                    div()
                        .text_xs()
                        .line_height(px(18.0))
                        .text_color(rgba(COLOR_WARNING))
                        .child(tr(
                            self.locale,
                            "gpui.action_blocked",
                            &[
                                ("action", &action_label),
                                ("reason", t(self.locale, reason.message_key())),
                            ],
                        ))
                        .into_any_element()
                }));
            } else if let Some(notice) = unavailable_notice {
                body = body.child(notice);
            } else if let Some(reason) = block_reason {
                let action_label = manifest_action_label(self.locale, &action);
                body = body.children(confirmation_notice).children(fields).child(
                    div()
                        .text_xs()
                        .line_height(px(18.0))
                        .text_color(rgba(COLOR_WARNING))
                        .child(tr(
                            self.locale,
                            "gpui.action_blocked",
                            &[
                                ("action", &action_label),
                                ("reason", t(self.locale, reason.message_key())),
                            ],
                        )),
                );
            } else {
                let form_content = div()
                    .id(SharedString::from(format!("action-form-{}", action.id)))
                    .debug_selector({
                        let selector = format!("visual-action-form-scroll-{}", action.id);
                        move || selector
                    })
                    .flex()
                    .flex_col()
                    .gap_3()
                    .max_h(px(340.0))
                    .overflow_y_scroll()
                    .children(confirmation_notice)
                    .children(fields);
                body = body.child(form_content).child(
                    button(
                        SharedString::from(format!("action-{}", action.id)),
                        button_label,
                        button_variant,
                        disabled,
                    )
                    .debug_selector({
                        let selector = format!("visual-action-cta-{}", action.id);
                        move || selector
                    })
                    .w_full()
                    .on_click(cx.listener(move |shell, _, _, cx| {
                        shell.submit_manifest_action(&action.id, cx);
                    })),
                );
            }
            card = card.child(body);
        }

        card.into_any_element()
    }

    fn field_editor(
        &self,
        action: &NodeUiAction,
        field: &NodeUiField,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let value = self.field_value(&action.id, &field.id);
        let label = field_label(self.locale, &field.label_key);
        let required = if field.required { " *" } else { "" };
        let control = match field.input {
            NodeUiInputKind::Boolean => self.choice_field(
                action,
                field,
                &value,
                &[("true", "common.enabled"), ("false", "common.disabled")],
                cx,
            ),
            NodeUiInputKind::Role => self.choice_field(
                action,
                field,
                &value,
                &[
                    ("member", "choice.role.member"),
                    ("discovery", "choice.role.discovery"),
                    ("relay", "choice.role.relay"),
                    ("exit_node", "choice.role.exit_node"),
                ],
                cx,
            ),
            NodeUiInputKind::RoutingMode => self.choice_field(
                action,
                field,
                &value,
                &[
                    ("mesh_only", "choice.routing.mesh_only"),
                    ("bypass_mainland", "choice.routing.bypass_mainland"),
                    ("global_proxy", "choice.routing.global_proxy"),
                ],
                cx,
            ),
            NodeUiInputKind::NetworkSelection => self.choice_field(
                action,
                field,
                &value,
                &[
                    ("default", "choice.network.default"),
                    ("all", "choice.network.all"),
                    ("none", "choice.network.none"),
                ],
                cx,
            ),
            NodeUiInputKind::Visibility => self.choice_field(
                action,
                field,
                &value,
                &[
                    ("public", "choice.visibility.public"),
                    ("unlisted", "choice.visibility.unlisted"),
                    ("private", "choice.visibility.private"),
                ],
                cx,
            ),
            NodeUiInputKind::JoinMode => self.choice_field(
                action,
                field,
                &value,
                &[
                    ("open", "choice.join_mode.open"),
                    ("approval_required", "choice.join_mode.approval_required"),
                    ("invite_only", "choice.join_mode.invite_only"),
                ],
                cx,
            ),
            NodeUiInputKind::ExitContributionMode => self.choice_field(
                action,
                field,
                &value,
                &[
                    ("disabled", "choice.exit_contribution_mode.disabled"),
                    ("autonomous", "choice.exit_contribution_mode.autonomous"),
                    (
                        "required_on_join",
                        "choice.exit_contribution_mode.required_on_join",
                    ),
                    (
                        "approval_required",
                        "choice.exit_contribution_mode.approval_required",
                    ),
                ],
                cx,
            ),
            NodeUiInputKind::ExitContribution => self.choice_field(
                action,
                field,
                &value,
                &[
                    ("not_required", "choice.exit_contribution.not_required"),
                    ("required", "choice.exit_contribution.required"),
                ],
                cx,
            ),
            NodeUiInputKind::ExitContributionScope => self.choice_field(
                action,
                field,
                &value,
                &[
                    (
                        "not_applicable",
                        "choice.exit_contribution_scope.not_applicable",
                    ),
                    (
                        "joining_members",
                        "choice.exit_contribution_scope.joining_members",
                    ),
                    ("all_members", "choice.exit_contribution_scope.all_members"),
                ],
                cx,
            ),
            NodeUiInputKind::JoinRequestReviewAction => self.choice_field(
                action,
                field,
                &value,
                &[
                    ("approve", "choice.join_request_review.approve"),
                    ("reject", "choice.join_request_review.reject"),
                    (
                        "request_information",
                        "choice.join_request_review.request_information",
                    ),
                ],
                cx,
            ),
            _ => self.text_field(action, field, &value, cx),
        };

        let mut editor = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .text_color(rgba(COLOR_MUTED))
                    .child(format!("{label}{required}")),
            )
            .child(control);
        if field.input == NodeUiInputKind::JoinRequestReason {
            let hint = format!(
                "{} · {}",
                t(self.locale, "join_requests.reason_input_help"),
                tr(
                    self.locale,
                    "join_requests.reason_input_counter",
                    &[
                        ("count", &value.chars().count().to_string()),
                        ("max", &JOIN_REQUEST_MAX_REASON_CHARS.to_string(),),
                    ],
                )
            );
            editor = editor.child(
                div()
                    .debug_selector(|| {
                        format!("visual-join-request-reason-help-{}-{}", action.id, field.id)
                    })
                    .text_xs()
                    .text_color(rgba(COLOR_FAINT))
                    .child(hint),
            );
        }
        editor.into_any_element()
    }

    fn text_field(
        &self,
        action: &NodeUiAction,
        field: &NodeUiField,
        _value: &str,
        _cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let field_key = self.field_key(&action.id, &field.id);
        if let Some(input) = self.text_inputs.get(&field_key) {
            input.clone().into_any_element()
        } else {
            component_error_state(
                SharedString::from(format!("missing-text-input-{field_key}")),
                t(self.locale, "common.input_field"),
                t(self.locale, "common.waiting_manifest"),
            )
            .into_any_element()
        }
    }

    fn choice_field(
        &self,
        action: &NodeUiAction,
        field: &NodeUiField,
        value: &str,
        choices: &[(&str, &str)],
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let action_id = action.id.clone();
        let field_id = field.id.clone();
        let current = value.to_string();
        div()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(choices.iter().map(|(choice_value, label)| {
                let action_id = action_id.clone();
                let field_id = field_id.clone();
                let choice_value = (*choice_value).to_string();
                let label = t(self.locale, label).to_string();
                let selected = current == choice_value;
                choice_control(
                    SharedString::from(format!("choice-{action_id}-{field_id}-{choice_value}")),
                    label,
                    selected,
                )
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.set_field_value(&action_id, &field_id, choice_value.clone());
                    cx.notify();
                }))
                .into_any_element()
            }))
            .into_any_element()
    }

    fn join_request_review_form_summary(&self, action_id: &str) -> gpui::AnyElement {
        let detail = if action_id == "batch_review_join_requests" {
            let count = serde_json::from_str::<Vec<NodeJoinRequestReviewItem>>(
                &self.field_value(action_id, "reviews"),
            )
            .map(|reviews| reviews.len())
            .unwrap_or(0);
            tr(
                self.locale,
                "join_requests.selected_count",
                &[("count", &count.to_string())],
            )
        } else {
            let request_id = self.field_value(action_id, "request_id");
            let revision = self.field_value(action_id, "expected_revision");
            let action = match self.field_value(action_id, "action").as_str() {
                "approve" => t(self.locale, "choice.join_request_review.approve"),
                "reject" => t(self.locale, "choice.join_request_review.reject"),
                "request_information" => t(
                    self.locale,
                    "choice.join_request_review.request_information",
                ),
                _ => t(self.locale, "join_requests.actions_unavailable"),
            };
            format!(
                "{}: {request_id} · {}: {revision} · {}",
                t(self.locale, "join_requests.request_id"),
                t(self.locale, "join_requests.revision"),
                action
            )
        };
        let selected_request_ids = if action_id == "batch_review_join_requests" {
            serde_json::from_str::<Vec<NodeJoinRequestReviewItem>>(
                &self.field_value(action_id, "reviews"),
            )
            .map(|reviews| {
                reviews
                    .into_iter()
                    .map(|review| review.request_id)
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default()
        } else {
            BTreeSet::from([self.field_value(action_id, "request_id")])
        };
        let selected_requests = self
            .recent_join_requests
            .as_ref()
            .map(|result| {
                result
                    .requests
                    .iter()
                    .filter(|request| selected_request_ids.contains(request.request_id.as_str()))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let mut summary = div()
            .debug_selector(|| "visual-join-request-review-summary".into())
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .rounded_md()
            .bg(rgba(COLOR_PANEL))
            .child(
                div()
                    .text_xs()
                    .text_color(rgba(COLOR_MUTED))
                    .child(t(self.locale, "join_requests.review_targets").to_string()),
            )
            .child(div().text_sm().text_color(rgba(COLOR_TEXT)).child(detail))
            .child(
                div()
                    .text_xs()
                    .text_color(rgba(COLOR_MUTED))
                    .child(t(self.locale, "join_requests.revision_pinned").to_string()),
            );
        if !selected_requests.is_empty() {
            summary = summary.child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight(600.0))
                    .text_color(rgba(COLOR_TEXT_SECONDARY))
                    .child(t(self.locale, "join_requests.grant_impact").to_string()),
            );
            for (index, request) in selected_requests.iter().enumerate() {
                summary = summary.child(self.join_request_grant_impact(
                    request,
                    format!("visual-join-request-review-grant-{action_id}-{index}"),
                ));
            }
        }
        summary.into_any_element()
    }

    fn join_request_grant_impact(
        &self,
        request: &JoinRequest,
        debug_selector: String,
    ) -> gpui::AnyElement {
        let capabilities = if request.grant_preview.capabilities.is_empty() {
            t(self.locale, "common.none").to_string()
        } else {
            request
                .grant_preview
                .capabilities
                .iter()
                .map(|capability| t(self.locale, profile_capability_key(*capability)).to_string())
                .collect::<Vec<_>>()
                .join(" · ")
        };
        div()
            .debug_selector(move || debug_selector.clone())
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded_sm()
            .bg(rgba(COLOR_PANEL_RAISED))
            .child(diagnostic_metadata_row(
                t(self.locale, "join_requests.role"),
                t(
                    self.locale,
                    join_request_role_key(request.grant_preview.role),
                )
                .to_string(),
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "join_requests.capabilities"),
                capabilities,
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "join_requests.exit"),
                t(
                    self.locale,
                    match request.grant_preview.exit_contribution_mode {
                        ExitContributionMode::RequiredOnJoin => "join_requests.exit.required",
                        ExitContributionMode::Autonomous => "join_requests.exit.autonomous",
                        ExitContributionMode::ApprovalRequired => {
                            "join_requests.exit.approval_required"
                        }
                        ExitContributionMode::Disabled => "join_requests.exit.not_required",
                    },
                )
                .to_string(),
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "join_requests.file_access"),
                t(
                    self.locale,
                    if request.grant_preview.file_access {
                        "join_requests.file_access.granted"
                    } else {
                        "join_requests.file_access.not_granted"
                    },
                )
                .to_string(),
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "join_requests.data_plane"),
                t(
                    self.locale,
                    if request.grant_preview.grants_data_plane_access {
                        "join_requests.data_plane.granted"
                    } else {
                        "join_requests.data_plane.not_granted"
                    },
                )
                .to_string(),
            ))
            .into_any_element()
    }

    fn invitation_status_row(
        &self,
        status: &InvitationStatusView,
        index: usize,
        show_actions: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let invitation = &status.invitation;
        let state = t(self.locale, invitation_state_key(status.state)).to_string();
        let role = t(self.locale, join_request_role_key(invitation.role)).to_string();
        let capabilities = if invitation.capabilities.is_empty() {
            t(self.locale, "common.none").to_string()
        } else {
            invitation
                .capabilities
                .iter()
                .map(|capability| t(self.locale, profile_capability_key(*capability)))
                .collect::<Vec<_>>()
                .join(" · ")
        };
        let expires = format_diagnostic_timestamp(self.locale, invitation.expires_at_unix_ms);
        let remaining = tr(
            self.locale,
            "invitations.remaining_uses",
            &[("count", &status.remaining_uses.to_string())],
        );
        let used = tr(
            self.locale,
            "invitations.used_count",
            &[
                ("used", &invitation.used_count.to_string()),
                ("max", &invitation.max_uses.to_string()),
            ],
        );
        let active = status.state == InvitationState::Active;
        let refresh_available = self.invitation_action_available("get_invitation_status");
        let revoke_available = active && self.invitation_action_available("revoke_invitation");
        let status_for_refresh = status.clone();
        let status_for_revoke = status.clone();
        let controls = show_actions.then(|| {
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .child(
                    button(
                        SharedString::from(format!("invitation-status-{index}")),
                        t(self.locale, "invitations.status_refresh"),
                        ButtonVariant::Ghost,
                        !refresh_available,
                    )
                    .debug_selector(move || format!("visual-invitation-refresh-{index}"))
                    .on_click(cx.listener(move |shell, _, _, cx| {
                        if shell.invitation_action_available("get_invitation_status") {
                            shell.prepare_invitation_status(&status_for_refresh, cx);
                        }
                    })),
                )
                .child(
                    button(
                        SharedString::from(format!("invitation-revoke-{index}")),
                        t(self.locale, "invitations.revoke"),
                        ButtonVariant::Danger,
                        !revoke_available,
                    )
                    .debug_selector(move || format!("visual-invitation-revoke-{index}"))
                    .on_click(cx.listener(move |shell, _, _, cx| {
                        if shell.invitation_action_available("revoke_invitation")
                            && status_for_revoke.state == InvitationState::Active
                        {
                            shell.prepare_invitation_revoke(&status_for_revoke, cx);
                        }
                    })),
                )
                .into_any_element()
        });

        div()
            .debug_selector(move || format!("visual-invitation-status-{index}"))
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_md()
            .bg(rgba(COLOR_PANEL))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(COLOR_TEXT))
                            .child(format!(
                                "{} · {}",
                                t(self.locale, "invitations.status_title"),
                                state
                            )),
                    )
                    .child(profile_status_chip(remaining)),
            )
            .children([
                diagnostic_metadata_row(
                    t(self.locale, "invitations.network"),
                    invitation.network_id.as_str(),
                )
                .into_any_element(),
                diagnostic_metadata_row(
                    t(self.locale, "invitations.invitation_id"),
                    invitation.invitation_id.as_str(),
                )
                .into_any_element(),
                diagnostic_metadata_row(t(self.locale, "invitations.role"), role)
                    .into_any_element(),
                diagnostic_metadata_row(t(self.locale, "invitations.capabilities"), capabilities)
                    .into_any_element(),
                diagnostic_metadata_row(t(self.locale, "invitations.used_count"), used)
                    .into_any_element(),
                diagnostic_metadata_row(t(self.locale, "invitations.expires_at"), expires)
                    .into_any_element(),
                diagnostic_metadata_row(
                    t(self.locale, "invitations.revision"),
                    invitation.revision.to_string(),
                )
                .into_any_element(),
            ])
            .children(controls)
            .when(
                show_actions && status.state != InvitationState::Active,
                |row| {
                    row.child(
                        div()
                            .text_xs()
                            .text_color(rgba(COLOR_MUTED))
                            .child(t(self.locale, "invitations.denied")),
                    )
                },
            )
            .into_any_element()
    }

    fn issued_invitation_card(
        &self,
        issued: &IssuedInvitation,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let status = &issued.status;
        let role = t(self.locale, join_request_role_key(status.invitation.role)).to_string();
        let capabilities = if status.invitation.capabilities.is_empty() {
            t(self.locale, "common.none").to_string()
        } else {
            status
                .invitation
                .capabilities
                .iter()
                .map(|capability| t(self.locale, profile_capability_key(*capability)))
                .collect::<Vec<_>>()
                .join(" · ")
        };
        let expires =
            format_diagnostic_timestamp(self.locale, status.invitation.expires_at_unix_ms);
        let code = issued.code.as_str().to_string();
        let copy_code = code.clone();
        let pending = self.pending_commands > 0;
        div()
            .id("visual-invitation-issued")
            .debug_selector(|| "visual-invitation-issued".into())
            .role(Role::Article)
            .aria_label(t(self.locale, "invitations.issued_title").to_string())
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_md()
            .bg(rgba(COLOR_PANEL_RAISED))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(COLOR_TEXT))
                            .child(t(self.locale, "invitations.issued_title")),
                    )
                    .child(profile_status_chip(t(
                        self.locale,
                        invitation_state_key(status.state),
                    ))),
            )
            .child(
                div()
                    .p_3()
                    .rounded_md()
                    .bg(rgba(COLOR_BG))
                    .font_family("monospace")
                    .text_sm()
                    .text_color(rgba(COLOR_TEXT))
                    .debug_selector(|| "visual-invitation-code".into())
                    .child(code),
            )
            .child(
                div()
                    .text_xs()
                    .line_height(px(18.0))
                    .text_color(rgba(COLOR_WARNING))
                    .child(t(self.locale, "invitations.code_once")),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        button(
                            SharedString::from("copy-invitation-code"),
                            t(self.locale, "invitations.copy"),
                            ButtonVariant::Secondary,
                            pending,
                        )
                        .debug_selector(|| "visual-invitation-copy".into())
                        .on_click(cx.listener(move |shell, _, _, cx| {
                            if shell.pending_commands > 0 {
                                return;
                            }
                            cx.write_to_clipboard(ClipboardItem::new_string(copy_code.clone()));
                            shell.invitation_copy_notice =
                                Some(t(shell.locale, "invitations.copied").to_string());
                            cx.notify();
                        })),
                    )
                    .children(self.invitation_copy_notice.as_ref().map(|notice| {
                        div()
                            .text_xs()
                            .text_color(rgba(COLOR_TEXT_SECONDARY))
                            .child(notice.clone())
                            .into_any_element()
                    })),
            )
            .children([
                diagnostic_metadata_row(t(self.locale, "invitations.role"), role)
                    .into_any_element(),
                diagnostic_metadata_row(t(self.locale, "invitations.capabilities"), capabilities)
                    .into_any_element(),
                diagnostic_metadata_row(t(self.locale, "invitations.expires_at"), expires)
                    .into_any_element(),
                diagnostic_metadata_row(
                    t(self.locale, "invitations.remaining_uses"),
                    status.remaining_uses.to_string(),
                )
                .into_any_element(),
            ])
            .child(
                div()
                    .debug_selector(|| "visual-invitation-ordinary-membership".into())
                    .text_xs()
                    .line_height(px(18.0))
                    .text_color(rgba(COLOR_TEXT_SECONDARY))
                    .child(t(self.locale, "invitations.ordinary_membership")),
            )
            .child(
                div()
                    .debug_selector(|| "visual-invitation-not-permanent".into())
                    .text_xs()
                    .line_height(px(18.0))
                    .text_color(rgba(COLOR_MUTED))
                    .child(t(self.locale, "invitations.not_permanent")),
            )
            .child(
                div()
                    .debug_selector(|| "visual-invitation-privacy-boundary".into())
                    .text_xs()
                    .text_color(rgba(COLOR_MUTED))
                    .child(t(self.locale, "invitations.privacy_boundary")),
            )
            .into_any_element()
    }

    fn invitations_card(
        &self,
        invitations: &[InvitationStatusView],
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let rows = invitations
            .iter()
            .enumerate()
            .map(|(index, status)| self.invitation_status_row(status, index, true, cx))
            .collect::<Vec<_>>();
        let mut card = div()
            .id("visual-invitation-list")
            .debug_selector(|| "visual-invitation-list".into())
            .role(Role::Article)
            .aria_label(t(self.locale, "invitations.list_title").to_string())
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_md()
            .bg(rgba(COLOR_PANEL_RAISED))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(COLOR_TEXT))
                            .child(t(self.locale, "invitations.list_title")),
                    )
                    .child(profile_status_chip(tr(
                        self.locale,
                        "invitations.count",
                        &[("count", &invitations.len().to_string())],
                    ))),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgba(COLOR_MUTED))
                    .child(t(self.locale, "invitations.privacy_boundary")),
            );
        if rows.is_empty() {
            card = card.child(
                empty_state(t(self.locale, "invitations.empty"))
                    .debug_selector(|| "visual-invitation-empty".into()),
            );
        } else {
            card = card.child(div().flex().flex_col().gap_2().children(rows));
        }
        card.into_any_element()
    }

    fn invitation_status_card(
        &self,
        status: &InvitationStatusView,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.invitation_status_row(status, 0, true, cx)
    }

    fn invitation_preview_card(
        &self,
        preview: &InvitationPreview,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let role = t(self.locale, join_request_role_key(preview.role)).to_string();
        let capabilities = if preview.capabilities.is_empty() {
            t(self.locale, "common.none").to_string()
        } else {
            preview
                .capabilities
                .iter()
                .map(|capability| t(self.locale, profile_capability_key(*capability)))
                .collect::<Vec<_>>()
                .join(" · ")
        };
        let permission_changes = if preview.permission_changes.is_empty() {
            t(self.locale, "common.none").to_string()
        } else {
            preview.permission_changes.join(" · ")
        };
        let expires = format_diagnostic_timestamp(self.locale, preview.expires_at_unix_ms);
        let terms_exit = t(
            self.locale,
            match preview.terms.exit_contribution_mode {
                ExitContributionMode::RequiredOnJoin => "invitations.terms.exit_required",
                ExitContributionMode::Autonomous => "invitations.terms.exit_autonomous",
                ExitContributionMode::ApprovalRequired => {
                    "invitations.terms.exit_approval_required"
                }
                ExitContributionMode::Disabled => "invitations.terms.exit_not_required",
            },
        )
        .to_string();
        let member_limit = preview
            .terms
            .member_limit
            .map(|limit| {
                tr(
                    self.locale,
                    "invitations.terms.member_limit",
                    &[("limit", &limit.to_string())],
                )
            })
            .unwrap_or_else(|| t(self.locale, "common.unlimited").to_string());
        let terms = [
            tr(
                self.locale,
                "invitations.terms.network_policy",
                &[(
                    "revision",
                    &preview.terms.network_policy_revision.to_string(),
                )],
            ),
            tr(
                self.locale,
                "invitations.terms.privacy_policy",
                &[(
                    "revision",
                    &preview.terms.privacy_policy_revision.to_string(),
                )],
            ),
            terms_exit,
            member_limit,
        ]
        .join(" · ");
        let exit_required = preview
            .terms
            .exit_contribution_mode
            .requires_join_commitment();
        let exit_disclosure_visible = preview.terms.exit_contribution_mode.permits_contribution();
        let local_exit_available = self.snapshot.exit_node_capabilities.supported;
        let can_accept = preview.can_accept
            && preview.state == InvitationState::Active
            && preview.remaining_uses > 0
            && preview.expires_at_unix_ms > unix_time_ms()
            && !preview.creates_permanent_membership
            && (!exit_required || local_exit_available);
        let checked = self.invitation_terms_confirmed;
        let exit_checked = self.invitation_exit_contribution_confirmed;
        let pending = self.pending_commands > 0;
        let denial = t(
            self.locale,
            preview
                .denial_key
                .as_deref()
                .unwrap_or("invitations.denied"),
        )
        .to_string();
        let local_capability_denial = exit_required && !local_exit_available;
        let confirmation_pending =
            self.pending_confirmation.as_deref() == Some("redeem_invitation");
        let terms_control = checkbox_control(
            SharedString::from("invitation-terms"),
            t(self.locale, "invitations.terms_checkbox"),
            checked,
            !can_accept || pending,
        )
        .debug_selector(|| "visual-invitation-terms".into())
        .on_click(cx.listener(|shell, _, _, cx| {
            if shell.pending_commands > 0 {
                return;
            }
            let can_accept = shell
                .recent_invitation_preview
                .as_ref()
                .is_some_and(|preview| preview.can_accept);
            if can_accept {
                shell.invitation_terms_confirmed = !shell.invitation_terms_confirmed;
                cx.notify();
            }
        }));
        let exit_control = checkbox_control(
            SharedString::from("invitation-exit-contribution"),
            t(self.locale, "join.exit.consent"),
            exit_checked,
            !can_accept || pending,
        )
        .debug_selector(|| "visual-invitation-exit-contribution".into())
        .on_click(cx.listener(|shell, _, _, cx| {
            if shell.pending_commands == 0
                && shell
                    .recent_invitation_preview
                    .as_ref()
                    .is_some_and(|preview| preview.can_accept)
            {
                shell.invitation_exit_contribution_confirmed =
                    !shell.invitation_exit_contribution_confirmed;
                cx.notify();
            }
        }));
        let confirm_label = if confirmation_pending {
            t(self.locale, "invitations.confirm_accept")
        } else {
            t(self.locale, "invitations.accept")
        };
        let confirm = button(
            SharedString::from("accept-invitation"),
            confirm_label,
            ButtonVariant::Primary,
            !can_accept || !checked || (exit_required && !exit_checked) || pending,
        )
        .debug_selector(|| "visual-invitation-confirm".into())
        .on_click(cx.listener(|shell, _, _, cx| {
            if shell.pending_commands == 0
                && shell.invitation_terms_confirmed
                && shell
                    .recent_invitation_preview
                    .as_ref()
                    .is_some_and(|preview| {
                        !preview
                            .terms
                            .exit_contribution_mode
                            .requires_join_commitment()
                            || shell.invitation_exit_contribution_confirmed
                    })
            {
                shell.confirm_invitation_preview(cx);
            }
        }));
        let preview_again = button(
            SharedString::from("preview-invitation-again"),
            t(self.locale, "invitations.preview_again"),
            ButtonVariant::Secondary,
            pending,
        )
        .debug_selector(|| "visual-invitation-preview-again".into())
        .on_click(cx.listener(|shell, _, _, cx| {
            shell.submit_manifest_action("preview_invitation", cx);
        }));
        let back_to_edit = (!can_accept).then(|| {
            button(
                SharedString::from("back-to-invitation-edit"),
                t(self.locale, "invitations.back_to_edit"),
                ButtonVariant::Secondary,
                pending,
            )
            .debug_selector(|| "visual-invitation-back-to-edit".into())
            .on_click(cx.listener(|shell, _, _, cx| {
                shell.return_to_invitation_edit(cx);
            }))
        });
        let mut preview_actions = div().flex().flex_wrap().gap_2();
        if let Some(back_to_edit) = back_to_edit {
            preview_actions = preview_actions.child(back_to_edit);
        }
        preview_actions = preview_actions.child(preview_again).child(confirm);
        div()
            .id("visual-invitation-preview")
            .debug_selector(|| "visual-invitation-preview".into())
            .role(Role::Article)
            .aria_label(t(self.locale, "invitations.preview_title").to_string())
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_md()
            .bg(rgba(COLOR_PANEL_RAISED))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(COLOR_TEXT))
                            .child(format!(
                                "{} · {}",
                                t(self.locale, "invitations.preview_title"),
                                preview.network_name
                            )),
                    )
                    .child(profile_status_chip(t(
                        self.locale,
                        if can_accept {
                            "invitations.can_accept"
                        } else {
                            "invitations.cannot_accept"
                        },
                    ))),
            )
            .when(!can_accept, |card| {
                card.child(
                    div()
                        .p_3()
                        .rounded_md()
                        .bg(rgba(DARK_THEME.colors.warning_soft.rgba_hex()))
                        .text_xs()
                        .text_color(rgba(COLOR_WARNING))
                        .child(if local_capability_denial {
                            t(self.locale, "join.error.exit_capability_required").to_string()
                        } else {
                            denial.clone()
                        }),
                )
            })
            .when(exit_disclosure_visible, |card| {
                card.child(
                    div()
                        .debug_selector(|| "visual-invitation-exit-disclosure".into())
                        .flex()
                        .flex_col()
                        .gap_1()
                        .text_xs()
                        .text_color(rgba(COLOR_TEXT_SECONDARY))
                        .child(t(self.locale, "join.exit.scope_all_members_hint"))
                        .child(t(self.locale, "join.exit.disclosure.purpose"))
                        .child(t(self.locale, "join.exit.disclosure.bandwidth"))
                        .child(t(self.locale, "join.exit.disclosure.traffic"))
                        .child(t(self.locale, "join.exit.disclosure.logging"))
                        .child(t(self.locale, "join.exit.disclosure.operational_impact"))
                        .child(t(self.locale, "join.exit.disclosure.disable"))
                        .child(t(
                            self.locale,
                            if local_exit_available {
                                "join.exit.local_supported"
                            } else {
                                "join.exit.local_unsupported"
                            },
                        ))
                        .child(t(self.locale, "join.exit.local_not_enabled")),
                )
            })
            .children([
                diagnostic_metadata_row(t(self.locale, "invitations.role"), role)
                    .into_any_element(),
                diagnostic_metadata_row(t(self.locale, "invitations.capabilities"), capabilities)
                    .into_any_element(),
                diagnostic_metadata_row(
                    t(self.locale, "invitations.permission_changes"),
                    permission_changes,
                )
                .into_any_element(),
                diagnostic_metadata_row(t(self.locale, "invitations.terms"), terms)
                    .into_any_element(),
                diagnostic_metadata_row(t(self.locale, "invitations.expires_at"), expires)
                    .into_any_element(),
                diagnostic_metadata_row(
                    t(self.locale, "invitations.remaining_uses"),
                    preview.remaining_uses.to_string(),
                )
                .into_any_element(),
            ])
            .when(preview.requires_recent_reauthentication, |card| {
                card.child(
                    div()
                        .text_xs()
                        .text_color(rgba(COLOR_WARNING))
                        .child(t(self.locale, "invitations.reauthentication_required")),
                )
            })
            .child(
                div()
                    .debug_selector(|| "visual-invitation-ordinary-membership".into())
                    .text_xs()
                    .line_height(px(18.0))
                    .text_color(rgba(COLOR_TEXT_SECONDARY))
                    .child(t(self.locale, "invitations.ordinary_membership")),
            )
            .child(
                div()
                    .debug_selector(|| "visual-invitation-not-permanent".into())
                    .text_xs()
                    .line_height(px(18.0))
                    .text_color(rgba(COLOR_MUTED))
                    .child(t(self.locale, "invitations.not_permanent")),
            )
            .child(terms_control)
            .when(exit_required, |card| card.child(exit_control))
            .child(preview_actions)
            .into_any_element()
    }

    fn join_requests_card(
        &self,
        result: &NodeJoinRequestListResult,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let count = tr(
            self.locale,
            "join_requests.count",
            &[("count", &result.requests.len().to_string())],
        );
        let review_action_available = self.join_request_action_available("review_join_request");
        let batch_action_available =
            self.join_request_action_available("batch_review_join_requests");
        let reviewable_ids = result
            .requests
            .iter()
            .filter(|request| !request.state.is_terminal())
            .map(|request| request.request_id.as_str().to_string())
            .collect::<Vec<_>>();
        let selected_requests = result
            .requests
            .iter()
            .filter(|request| {
                !request.state.is_terminal()
                    && self
                        .join_request_selection
                        .contains(request.request_id.as_str())
            })
            .map(|request| (request.request_id.as_str().to_string(), request.revision))
            .collect::<Vec<_>>();
        let selected_count = selected_requests.len();
        let all_selected = !reviewable_ids.is_empty() && reviewable_ids.len() == selected_count;
        let requests = result
            .requests
            .iter()
            .enumerate()
            .map(|(index, request)| {
                let network_id = result.network_id.clone();
                let request_id = request.request_id.as_str().to_string();
                let request_revision = request.revision;
                let reviewable = !request.state.is_terminal();
                let selected = self.join_request_selection.contains(&request_id);
                let applicant = request
                    .applicant
                    .display_name
                    .clone()
                    .unwrap_or_else(|| request.applicant.node_id.as_str().to_string());
                let state = t(self.locale, join_request_state_key(request.state)).to_string();
                let reason = request
                    .reason
                    .clone()
                    .unwrap_or_else(|| t(self.locale, "join_requests.reason_redacted").to_string());
                let visibility = t(
                    self.locale,
                    "join_requests.reason_visibility.owners_and_administrators",
                )
                .to_string();
                let capabilities = if request.grant_preview.capabilities.is_empty() {
                    t(self.locale, "common.none").to_string()
                } else {
                    request
                        .grant_preview
                        .capabilities
                        .iter()
                        .map(|capability| {
                            t(self.locale, profile_capability_key(*capability)).to_string()
                        })
                        .collect::<Vec<_>>()
                        .join(" · ")
                };
                let role = t(
                    self.locale,
                    join_request_role_key(request.grant_preview.role),
                )
                .to_string();
                let exit = t(
                    self.locale,
                    match request.grant_preview.exit_contribution_mode {
                        ExitContributionMode::RequiredOnJoin => "join_requests.exit.required",
                        ExitContributionMode::Autonomous => "join_requests.exit.autonomous",
                        ExitContributionMode::ApprovalRequired => {
                            "join_requests.exit.approval_required"
                        }
                        ExitContributionMode::Disabled => "join_requests.exit.not_required",
                    },
                )
                .to_string();
                let file_access = t(
                    self.locale,
                    if request.grant_preview.file_access {
                        "join_requests.file_access.granted"
                    } else {
                        "join_requests.file_access.not_granted"
                    },
                )
                .to_string();
                let data_plane = t(
                    self.locale,
                    if request.grant_preview.grants_data_plane_access {
                        "join_requests.data_plane.granted"
                    } else {
                        "join_requests.data_plane.not_granted"
                    },
                )
                .to_string();
                let expires = format_diagnostic_timestamp(self.locale, request.expires_at_unix_ms);
                let expiring = request.expires_at_unix_ms
                    <= unix_time_ms().saturating_add(24 * 60 * 60 * 1000);
                let information_request = request.information_request.clone();
                let selection_label = tr(
                    self.locale,
                    "join_requests.select_request",
                    &[("applicant", &applicant)],
                );
                let review_actions = div()
                    .debug_selector(|| "visual-join-request-review-actions".into())
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .children(
                        [
                            (
                                JoinRequestReviewAction::Approve,
                                "approve",
                                ButtonVariant::Primary,
                                "choice.join_request_review.approve",
                            ),
                            (
                                JoinRequestReviewAction::Reject,
                                "reject",
                                ButtonVariant::Danger,
                                "choice.join_request_review.reject",
                            ),
                            (
                                JoinRequestReviewAction::RequestInformation,
                                "request-information",
                                ButtonVariant::Secondary,
                                "choice.join_request_review.request_information",
                            ),
                        ]
                        .into_iter()
                        .map(
                            |(review_action, action_key, variant, label_key)| {
                                let network_id = network_id.clone();
                                let request_id = request_id.clone();
                                button(
                                    SharedString::from(format!(
                                        "join-request-review-{index}-{action_key}"
                                    )),
                                    t(self.locale, label_key),
                                    variant,
                                    !review_action_available,
                                )
                                .debug_selector(move || {
                                    format!("visual-join-request-review-{index}-{action_key}")
                                })
                                .on_click(cx.listener(move |shell, _, _, cx| {
                                    if shell.join_request_action_available("review_join_request") {
                                        shell.prepare_join_request_review(
                                            network_id.clone(),
                                            request_id.clone(),
                                            request_revision,
                                            review_action,
                                            cx,
                                        );
                                    }
                                }))
                                .into_any_element()
                            },
                        ),
                    );

                div()
                    .id(SharedString::from(format!(
                        "join-request-{}",
                        request.request_id
                    )))
                    .when(index == 0, |item| {
                        item.debug_selector(|| "visual-join-request-first".into())
                    })
                    .role(Role::Article)
                    .aria_label(applicant.clone())
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_3()
                    .rounded_md()
                    .bg(rgba(COLOR_PANEL))
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .justify_between()
                            .gap_2()
                            .child(
                                div().flex().flex_col().gap_1().child(
                                    div()
                                        .debug_selector(|| "visual-join-request-applicant".into())
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_size(px(10.0))
                                                .text_color(rgba(COLOR_MUTED))
                                                .child(
                                                    t(self.locale, "join_requests.applicant")
                                                        .to_string(),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(gpui::FontWeight(600.0))
                                                .text_color(rgba(COLOR_TEXT))
                                                .child(applicant),
                                        ),
                                ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .justify_end()
                                    .gap_1()
                                    .child(profile_status_chip(state))
                                    .child(profile_status_chip(format!(
                                        "{}: {}",
                                        t(self.locale, "join_requests.revision"),
                                        format_integer(self.locale, request.revision as i64)
                                    ))),
                            ),
                    )
                    .child(diagnostic_metadata_row(
                        t(self.locale, "join_requests.node_id"),
                        request.applicant.node_id.as_str().to_string(),
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "join_requests.fingerprint"),
                        request.applicant.public_key_fingerprint.clone(),
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "join_requests.request_id"),
                        request.request_id.as_str().to_string(),
                    ))
                    .child(
                        div()
                            .debug_selector(|| "visual-join-request-reason".into())
                            .flex()
                            .flex_col()
                            .gap_1()
                            .p_2()
                            .rounded_sm()
                            .bg(rgba(COLOR_PANEL_RAISED))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgba(COLOR_TEXT_SECONDARY))
                                    .child(t(self.locale, "join_requests.reason").to_string()),
                            )
                            .child(div().text_sm().text_color(rgba(COLOR_TEXT)).child(reason))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgba(COLOR_MUTED))
                                    .child(visibility),
                            ),
                    )
                    .when(information_request.is_some(), |item| {
                        item.child(
                            div()
                                .p_2()
                                .rounded_sm()
                                .bg(rgba(COLOR_PANEL_RAISED))
                                .text_xs()
                                .text_color(rgba(COLOR_WARNING))
                                .child(format!(
                                    "{}: {}",
                                    t(self.locale, "join_requests.information_request"),
                                    information_request.unwrap_or_default()
                                )),
                        )
                    })
                    .child(
                        div()
                            .debug_selector(|| "visual-join-request-risk".into())
                            .flex()
                            .flex_col()
                            .gap_2()
                            .p_2()
                            .rounded_sm()
                            .bg(rgba(COLOR_PANEL_RAISED))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_xs()
                                    .text_color(rgba(COLOR_WARNING))
                                    .child("!")
                                    .child(t(self.locale, "join_requests.risk").to_string()),
                            )
                            .child(diagnostic_metadata_row(
                                t(self.locale, "join_requests.role"),
                                role,
                            ))
                            .child(diagnostic_metadata_row(
                                t(self.locale, "join_requests.capabilities"),
                                capabilities,
                            ))
                            .child(diagnostic_metadata_row(
                                t(self.locale, "join_requests.exit"),
                                exit,
                            ))
                            .child(diagnostic_metadata_row(
                                t(self.locale, "join_requests.file_access"),
                                file_access,
                            ))
                            .child(diagnostic_metadata_row(
                                t(self.locale, "join_requests.data_plane"),
                                data_plane,
                            ))
                            .child(diagnostic_metadata_row(
                                t(self.locale, "join_requests.expires_at"),
                                expires,
                            ))
                            .when(expiring, |risk| {
                                risk.child(div().text_xs().text_color(rgba(COLOR_WARNING)).child(
                                    t(self.locale, "join_requests.expiring_soon").to_string(),
                                ))
                            }),
                    )
                    .when(reviewable, |item| {
                        item.child(
                            div()
                                .debug_selector(|| "visual-join-request-actions".into())
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .pt_3()
                                .child(
                                    checkbox_control(
                                        SharedString::from(format!("join-request-select-{index}")),
                                        selection_label,
                                        selected,
                                        !review_action_available,
                                    )
                                    .debug_selector(move || {
                                        format!("visual-join-request-select-{index}")
                                    })
                                    .on_click(cx.listener(
                                        move |shell, _, _, cx| {
                                            if shell.join_request_action_available(
                                                "review_join_request",
                                            ) {
                                                shell.toggle_join_request_selection(
                                                    request_id.clone(),
                                                    cx,
                                                );
                                            }
                                        },
                                    )),
                                )
                                .child(review_actions),
                        )
                    })
            })
            .collect::<Vec<_>>();

        let mut card = div()
            .id("join-requests-list")
            .debug_selector(|| "visual-join-requests-list".into())
            .role(Role::Article)
            .aria_label(t(self.locale, "join_requests.title").to_string())
            .flex()
            .flex_col()
            .gap_3()
            .p_3()
            .rounded_md()
            .bg(rgba(COLOR_PANEL_RAISED))
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .font_weight(gpui::FontWeight(600.0))
                                    .text_color(rgba(COLOR_MUTED))
                                    .child(t(self.locale, "join_requests.title").to_string()),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight(600.0))
                                    .text_color(rgba(COLOR_TEXT))
                                    .child(count),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(rgba(COLOR_FAINT))
                            .child(result.network_id.clone()),
                    ),
            )
            .child(
                div()
                    .p_2()
                    .rounded_sm()
                    .bg(rgba(COLOR_PANEL))
                    .text_xs()
                    .text_color(rgba(COLOR_MUTED))
                    .child(t(self.locale, "join_requests.privacy_boundary").to_string()),
            );

        if !reviewable_ids.is_empty() {
            let select_all_ids = reviewable_ids.clone();
            let selected_count_label = tr(
                self.locale,
                "join_requests.selected_count",
                &[("count", &selected_count.to_string())],
            );
            let batch_controls = div()
                .debug_selector(|| "visual-join-request-batch-actions".into())
                .flex()
                .flex_wrap()
                .gap_2()
                .children(
                    [
                        (
                            JoinRequestReviewAction::Approve,
                            "approve",
                            ButtonVariant::Primary,
                            "join_requests.batch.approve",
                        ),
                        (
                            JoinRequestReviewAction::Reject,
                            "reject",
                            ButtonVariant::Danger,
                            "join_requests.batch.reject",
                        ),
                        (
                            JoinRequestReviewAction::RequestInformation,
                            "request-information",
                            ButtonVariant::Secondary,
                            "join_requests.batch.request_information",
                        ),
                    ]
                    .into_iter()
                    .map(|(review_action, action_key, variant, label_key)| {
                        let network_id = result.network_id.clone();
                        let requests = selected_requests.clone();
                        button(
                            SharedString::from(format!("join-request-batch-{action_key}")),
                            t(self.locale, label_key),
                            variant,
                            selected_count == 0 || !batch_action_available,
                        )
                        .debug_selector(move || format!("visual-join-request-batch-{action_key}"))
                        .on_click(cx.listener(move |shell, _, _, cx| {
                            if !requests.is_empty()
                                && shell.join_request_action_available("batch_review_join_requests")
                            {
                                shell.prepare_batch_join_request_review(
                                    network_id.clone(),
                                    &requests,
                                    review_action,
                                    cx,
                                );
                            }
                        }))
                        .into_any_element()
                    }),
                );
            card = card.child(
                div()
                    .debug_selector(|| "visual-join-request-selection-bar".into())
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .p_3()
                    .rounded_md()
                    .bg(rgba(COLOR_PANEL))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_3()
                            .child(
                                checkbox_control(
                                    SharedString::from("join-request-select-all"),
                                    t(self.locale, "join_requests.select_all"),
                                    all_selected,
                                    !review_action_available,
                                )
                                .debug_selector(|| "visual-join-request-select-all".into())
                                .on_click(cx.listener(
                                    move |shell, _, _, cx| {
                                        if shell
                                            .join_request_action_available("review_join_request")
                                        {
                                            shell.toggle_all_join_request_selection(
                                                &select_all_ids,
                                                cx,
                                            );
                                        }
                                    },
                                )),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight(600.0))
                                    .text_color(rgba(COLOR_TEXT))
                                    .child(selected_count_label),
                            ),
                    )
                    .child(batch_controls),
            );
        }

        if requests.is_empty() {
            card = card.child(
                empty_state(t(self.locale, "join_requests.empty"))
                    .debug_selector(|| "visual-join-requests-empty".into()),
            );
        } else {
            card = card.child(div().flex().flex_col().gap_3().children(requests));
        }

        card.into_any_element()
    }

    fn member_directory_card(
        &self,
        directory: &NetworkMemberDirectory,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let filtered = filter_network_members(&directory.members, self.member_directory_filters);
        let filtered_members = filtered
            .iter()
            .map(|member| (*member).clone())
            .collect::<Vec<_>>();
        let selected_member = self
            .selected_member_node_id
            .as_deref()
            .and_then(|selected| {
                filtered
                    .iter()
                    .copied()
                    .find(|member| member.node_id.as_str() == selected)
            });
        let count = tr(
            self.locale,
            "member_directory.count",
            &[("count", &format_integer(self.locale, filtered.len() as i64))],
        );
        let generated_at = format_diagnostic_timestamp(self.locale, directory.generated_at_unix_ms);

        let filter_group = |title: SharedString, controls: Vec<gpui::AnyElement>| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .min_w(px(150.0))
                .flex_1()
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui::FontWeight(600.0))
                        .text_color(rgba(COLOR_MUTED))
                        .child(title),
                )
                .child(div().flex().flex_wrap().gap_2().children(controls))
        };

        let mut role_controls = Vec::new();
        for (role, selected) in [
            (None, self.member_directory_filters.role.is_none()),
            (
                Some(MembershipRole::Owner),
                self.member_directory_filters.role == Some(MembershipRole::Owner),
            ),
            (
                Some(MembershipRole::Administrator),
                self.member_directory_filters.role == Some(MembershipRole::Administrator),
            ),
            (
                Some(MembershipRole::Member),
                self.member_directory_filters.role == Some(MembershipRole::Member),
            ),
        ] {
            let label = role
                .map(|role| t(self.locale, join_request_role_key(role)).to_string())
                .unwrap_or_else(|| t(self.locale, "member_directory.filter.all").to_string());
            let value = role.map(invitation_role_name).unwrap_or("all");
            role_controls.push(
                choice_control(
                    SharedString::from(format!("member-filter-role-{value}")),
                    label,
                    selected,
                )
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.member_directory_filters.role = role;
                    cx.notify();
                }))
                .into_any_element(),
            );
        }

        let mut group_controls = Vec::new();
        for (group, selected) in [
            (None, self.member_directory_filters.group.is_none()),
            (
                Some(NetworkMemberGroup::Owner),
                self.member_directory_filters.group == Some(NetworkMemberGroup::Owner),
            ),
            (
                Some(NetworkMemberGroup::Administrator),
                self.member_directory_filters.group == Some(NetworkMemberGroup::Administrator),
            ),
            (
                Some(NetworkMemberGroup::Member),
                self.member_directory_filters.group == Some(NetworkMemberGroup::Member),
            ),
        ] {
            let label = group
                .map(|group| t(self.locale, member_group_key(group)).to_string())
                .unwrap_or_else(|| t(self.locale, "member_directory.filter.all").to_string());
            let value = group.map(member_group_value).unwrap_or("all");
            group_controls.push(
                choice_control(
                    SharedString::from(format!("member-filter-group-{value}")),
                    label,
                    selected,
                )
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.member_directory_filters.group = group;
                    cx.notify();
                }))
                .into_any_element(),
            );
        }

        let mut capability_controls = Vec::new();
        let capability_options = [
            (MemberCapabilityFilter::All, "all"),
            (MemberCapabilityFilter::Unknown, "unknown"),
            (
                MemberCapabilityFilter::Known(CapabilityKind::Coordinator),
                "coordinator",
            ),
            (
                MemberCapabilityFilter::Known(CapabilityKind::Relay),
                "relay",
            ),
            (MemberCapabilityFilter::Known(CapabilityKind::Exit), "exit"),
            (MemberCapabilityFilter::Known(CapabilityKind::File), "file"),
        ];
        for (capability, value) in capability_options {
            let label = match capability {
                MemberCapabilityFilter::All => {
                    t(self.locale, "member_directory.filter.all").to_string()
                }
                MemberCapabilityFilter::Unknown => t(self.locale, "common.unknown").to_string(),
                MemberCapabilityFilter::Known(capability) => {
                    t(self.locale, profile_capability_key(capability)).to_string()
                }
            };
            capability_controls.push(
                choice_control(
                    SharedString::from(format!("member-filter-capability-{value}")),
                    label,
                    self.member_directory_filters.capability == capability,
                )
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.member_directory_filters.capability = capability;
                    cx.notify();
                }))
                .into_any_element(),
            );
        }

        let mut health_controls = Vec::new();
        health_controls.push(
            choice_control(
                "member-filter-health-all".into(),
                t(self.locale, "member_directory.filter.all").to_string(),
                self.member_directory_filters.health == MemberHealthFilter::All,
            )
            .on_click(cx.listener(|shell, _, _, cx| {
                shell.member_directory_filters.health = MemberHealthFilter::All;
                cx.notify();
            }))
            .into_any_element(),
        );
        for status in [
            NetworkMemberStatus::Pending,
            NetworkMemberStatus::Approved,
            NetworkMemberStatus::Active,
            NetworkMemberStatus::Degraded,
            NetworkMemberStatus::Suspended,
            NetworkMemberStatus::Exited,
            NetworkMemberStatus::Removed,
            NetworkMemberStatus::Expired,
        ] {
            let selected =
                self.member_directory_filters.health == MemberHealthFilter::Status(status);
            let label = tr(
                self.locale,
                "member_directory.filter.status",
                &[("status", t(self.locale, member_status_key(status)))],
            );
            health_controls.push(
                choice_control(
                    SharedString::from(format!(
                        "member-filter-health-status-{}",
                        member_status_value(status)
                    )),
                    label,
                    selected,
                )
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.member_directory_filters.health = MemberHealthFilter::Status(status);
                    cx.notify();
                }))
                .into_any_element(),
            );
        }
        for presence in [
            NetworkMemberOnlineState::Online,
            NetworkMemberOnlineState::Offline,
            NetworkMemberOnlineState::Unknown,
        ] {
            let selected =
                self.member_directory_filters.health == MemberHealthFilter::Presence(presence);
            let label = tr(
                self.locale,
                "member_directory.filter.presence",
                &[("presence", t(self.locale, member_presence_key(presence)))],
            );
            health_controls.push(
                choice_control(
                    SharedString::from(format!(
                        "member-filter-health-presence-{}",
                        member_presence_value(presence)
                    )),
                    label,
                    selected,
                )
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.member_directory_filters.health = MemberHealthFilter::Presence(presence);
                    cx.notify();
                }))
                .into_any_element(),
            );
        }

        let export_button = button(
            "member-directory-export".into(),
            t(self.locale, "member_directory.export").to_string(),
            ButtonVariant::Secondary,
            filtered_members.is_empty(),
        )
        .debug_selector(|| "visual-member-directory-export".into())
        .on_click(cx.listener(|shell, _, _, cx| shell.export_network_members(cx)));

        let list_members = filtered_members.clone();
        let list = if list_members.is_empty() {
            empty_state(t(self.locale, "member_directory.empty"))
                .debug_selector(|| "visual-member-directory-empty".into())
                .into_any_element()
        } else {
            let selected_node_id = self.selected_member_node_id.clone();
            let locale = self.locale;
            let member_exit_now = directory.generated_at_unix_ms;
            uniform_list(
                "network-member-list",
                list_members.len(),
                cx.processor(
                    move |shell: &mut GpuiShell, range: std::ops::Range<usize>, _window, cx| {
                        range
                            .map(|index| {
                                let member = &list_members[index];
                                let node_id = member.node_id.as_str().to_string();
                                let display_name = member_display_name(member);
                                let role =
                                    t(locale, join_request_role_key(member.role)).to_string();
                                let status =
                                    t(locale, member_status_key(member.status)).to_string();
                                let presence =
                                    t(locale, member_presence_key(member.presence.state))
                                        .to_string();
                                let exit_state = member_exit_state_text(
                                    locale,
                                    Some(&member.exit_availability),
                                    member_exit_now,
                                );
                                let groups = member_groups_text(locale, member);
                                let selected = selected_node_id.as_deref()
                                    == Some(node_id.as_str())
                                    || shell.selected_member_node_id.as_deref()
                                        == Some(node_id.as_str());
                                let click_node_id = node_id.clone();
                                div()
                                    .id(SharedString::from(format!("network-member-row-{index}")))
                                    .debug_selector(move || format!("visual-member-row-{index}"))
                                    .when(index == 0, |row| {
                                        row.debug_selector(|| "visual-member-first".into())
                                    })
                                    .role(Role::Button)
                                    .aria_label(tr(
                                        locale,
                                        "member_directory.select_member",
                                        &[("member", &display_name)],
                                    ))
                                    .focusable()
                                    .tab_stop(true)
                                    .h(px(64.0))
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_3()
                                    .px_3()
                                    .when(selected, |row| {
                                        row.bg(rgba(DARK_THEME.colors.accent_soft.rgba_hex()))
                                    })
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |shell, _, _, cx| {
                                        shell.selected_member_node_id = Some(click_node_id.clone());
                                        cx.notify();
                                    }))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_sm()
                                                    .font_weight(gpui::FontWeight(600.0))
                                                    .text_color(rgba(COLOR_TEXT))
                                                    .child(display_name),
                                            )
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_xs()
                                                    .text_color(rgba(COLOR_MUTED))
                                                    .child(format!("{node_id} · {groups}")),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_wrap()
                                            .justify_end()
                                            .gap_1()
                                            .child(profile_status_chip(role))
                                            .child(profile_status_chip(status))
                                            .child(profile_status_chip(presence))
                                            .child(
                                                profile_status_chip(format!(
                                                    "{} · {exit_state}",
                                                    t(locale, "member_directory.exit.title")
                                                ))
                                                .debug_selector(move || {
                                                    format!("visual-member-exit-{index}")
                                                }),
                                            ),
                                    )
                            })
                            .collect::<Vec<_>>()
                    },
                ),
            )
            .debug_selector(|| "visual-member-directory-list".into())
            .h(px(320.0))
            .w_full()
            .into_any_element()
        };

        let detail = selected_member
            .map(|member| self.member_detail_panel(member, directory.generated_at_unix_ms))
            .unwrap_or_else(|| {
                div()
                    .debug_selector(|| "visual-member-detail-hint".into())
                    .rounded_md()
                    .p_3()
                    .text_xs()
                    .text_color(rgba(COLOR_MUTED))
                    .child(t(self.locale, "member_directory.detail_hint").to_string())
                    .into_any_element()
            });

        surface_card()
            .id("network-member-directory")
            .debug_selector(|| "visual-member-directory".into())
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(gpui::FontWeight(600.0))
                                    .text_color(rgba(COLOR_TEXT))
                                    .child(t(self.locale, "member_directory.title").to_string()),
                            )
                            .child(div().text_xs().text_color(rgba(COLOR_MUTED)).child(
                                t(self.locale, "member_directory.privacy_boundary").to_string(),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .debug_selector(|| "visual-member-directory-count".into())
                                    .text_xs()
                                    .text_color(rgba(COLOR_TEXT_SECONDARY))
                                    .child(count),
                            )
                            .child(export_button),
                    ),
            )
            .child(diagnostic_metadata_row(
                t(self.locale, "member_directory.generated_at"),
                generated_at,
            ))
            .child(
                div()
                    .debug_selector(|| "visual-member-directory-filters".into())
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .child(filter_group(
                        t(self.locale, "member_directory.filter.role")
                            .to_string()
                            .into(),
                        role_controls,
                    ))
                    .child(filter_group(
                        t(self.locale, "member_directory.filter.group")
                            .to_string()
                            .into(),
                        group_controls,
                    ))
                    .child(filter_group(
                        t(self.locale, "member_directory.filter.capability")
                            .to_string()
                            .into(),
                        capability_controls,
                    ))
                    .child(filter_group(
                        t(self.locale, "member_directory.filter.health")
                            .to_string()
                            .into(),
                        health_controls,
                    )),
            )
            .child(list)
            .child(detail)
            .into_any_element()
    }

    fn export_network_members(&mut self, cx: &mut Context<Self>) {
        let Some(directory) = self.recent_network_members.as_ref() else {
            self.error = Some(t(self.locale, "member_directory.export_failed").to_string());
            cx.notify();
            return;
        };
        let members = filter_network_members(&directory.members, self.member_directory_filters)
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let text = export_member_directory(self.locale, &members, directory.generated_at_unix_ms);
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.notice = Some(tr(
            self.locale,
            "member_directory.export_succeeded",
            &[("count", &format_integer(self.locale, members.len() as i64))],
        ));
        self.error = None;
        cx.notify();
    }

    fn member_detail_panel(
        &self,
        member: &NetworkMember,
        generated_at_unix_ms: u64,
    ) -> gpui::AnyElement {
        let display_name = member_display_name(member);
        let fingerprint = member
            .identity_fingerprint
            .clone()
            .unwrap_or_else(|| t(self.locale, "member_directory.identity_unknown").to_string());
        let role = t(self.locale, join_request_role_key(member.role)).to_string();
        let groups = member_groups_text(self.locale, member);
        let capabilities = member_capabilities_text(self.locale, member);
        let status = t(self.locale, member_status_key(member.status)).to_string();
        let presence = t(self.locale, member_presence_key(member.presence.state)).to_string();
        let presence_source = t(
            self.locale,
            member_presence_source_key(member.presence.source),
        )
        .to_string();
        let observed_at = member
            .presence
            .observed_at_unix_ms
            .map(|observed| format_diagnostic_timestamp(self.locale, observed))
            .unwrap_or_else(|| t(self.locale, "common.unknown").to_string());
        let freshness = member_freshness_text(
            self.locale,
            member.presence.observed_at_unix_ms,
            generated_at_unix_ms,
        )
        .unwrap_or_else(|| t(self.locale, "common.unknown").to_string());
        let joined_at = member_joined_at_text(self.locale, member);
        let exit_availability = Some(&member.exit_availability);
        let exit_state =
            member_exit_state_text(self.locale, exit_availability, generated_at_unix_ms);
        let exit_source =
            member_exit_source_text(self.locale, exit_availability, generated_at_unix_ms);
        let exit_declared =
            member_exit_declared_text(self.locale, exit_availability, generated_at_unix_ms);
        let exit_observed_at = member
            .exit_availability
            .observed_at_unix_ms
            .map(|observed| format_diagnostic_timestamp(self.locale, observed))
            .unwrap_or_else(|| t(self.locale, "common.unknown").to_string());
        let exit_freshness =
            member_exit_freshness_text(self.locale, exit_availability, generated_at_unix_ms);
        let exit_reason =
            member_exit_reason_text(self.locale, exit_availability, generated_at_unix_ms);

        div()
            .debug_selector(|| "visual-member-detail".into())
            .flex()
            .flex_col()
            .gap_2()
            .rounded_md()
            .bg(rgba(COLOR_PANEL_RAISED))
            .p_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(COLOR_TEXT))
                            .child(format!(
                                "{} · {display_name}",
                                t(self.locale, "member_directory.detail_title")
                            )),
                    )
                    .child(
                        div()
                            .debug_selector(|| "visual-member-presence".into())
                            .child(profile_status_chip(presence)),
                    ),
            )
            .children([
                diagnostic_metadata_row(
                    t(self.locale, "member_directory.display_name"),
                    display_name,
                )
                .into_any_element(),
                diagnostic_metadata_row(
                    t(self.locale, "member_directory.node_id"),
                    member.node_id.as_str().to_string(),
                )
                .into_any_element(),
                diagnostic_metadata_row(
                    t(self.locale, "member_directory.fingerprint"),
                    fingerprint,
                )
                .into_any_element(),
                diagnostic_metadata_row(t(self.locale, "member_directory.role"), role)
                    .into_any_element(),
                diagnostic_metadata_row(t(self.locale, "member_directory.groups"), groups)
                    .into_any_element(),
                diagnostic_metadata_row(
                    t(self.locale, "member_directory.capabilities"),
                    capabilities,
                )
                .into_any_element(),
                diagnostic_metadata_row(t(self.locale, "member_directory.status"), status)
                    .into_any_element(),
                diagnostic_metadata_row(
                    t(self.locale, "member_directory.presence_source"),
                    presence_source,
                )
                .into_any_element(),
                diagnostic_metadata_row(
                    t(self.locale, "member_directory.observed_at"),
                    observed_at,
                )
                .into_any_element(),
                diagnostic_metadata_row(t(self.locale, "member_directory.freshness"), freshness)
                    .into_any_element(),
                diagnostic_metadata_row(t(self.locale, "member_directory.joined_at"), joined_at)
                    .into_any_element(),
            ])
            .child(
                div()
                    .debug_selector(|| "visual-member-exit-availability".into())
                    .flex()
                    .flex_col()
                    .gap_2()
                    .rounded_md()
                    .p_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(gpui::FontWeight(600.0))
                                    .text_color(rgba(COLOR_TEXT))
                                    .child(
                                        t(self.locale, "member_directory.exit.title").to_string(),
                                    ),
                            )
                            .child(
                                div()
                                    .debug_selector(|| "visual-member-exit-state".into())
                                    .child(profile_status_chip(exit_state.clone())),
                            ),
                    )
                    .children([
                        diagnostic_metadata_row(
                            t(self.locale, "member_directory.exit.state"),
                            exit_state,
                        )
                        .into_any_element(),
                        diagnostic_metadata_row(
                            t(self.locale, "member_directory.exit.source"),
                            exit_source,
                        )
                        .into_any_element(),
                        diagnostic_metadata_row(
                            t(self.locale, "member_directory.exit.declared"),
                            exit_declared,
                        )
                        .into_any_element(),
                        diagnostic_metadata_row(
                            t(self.locale, "member_directory.exit.observed_at"),
                            exit_observed_at,
                        )
                        .into_any_element(),
                        diagnostic_metadata_row(
                            t(self.locale, "member_directory.exit.freshness"),
                            exit_freshness,
                        )
                        .into_any_element(),
                        diagnostic_metadata_row(
                            t(self.locale, "member_directory.exit.reason"),
                            exit_reason,
                        )
                        .into_any_element(),
                    ]),
            )
            .into_any_element()
    }

    fn join_preview_card(
        &self,
        preview: &NodeNetworkJoinPreview,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let pending = self.pending_commands > 0;
        let can_join = preview.can_join && !preview.already_member;
        let checked = self.network_join_terms_confirmed;
        let exit_checked = self.network_join_exit_contribution_confirmed;
        let exit_consent_required = preview.join_requires_exit_consent;
        let show_exit_disclosure = preview.exit_contribution_mode.permits_contribution();
        let exit_purpose =
            t(self.locale, preview.exit_contribution_purpose_key.as_str()).to_string();
        let exit_bandwidth = t(
            self.locale,
            preview.exit_contribution_bandwidth_impact_key.as_str(),
        )
        .to_string();
        let exit_traffic = t(
            self.locale,
            preview.exit_contribution_traffic_impact_key.as_str(),
        )
        .to_string();
        let exit_logging = t(
            self.locale,
            preview.exit_contribution_logging_boundary_key.as_str(),
        )
        .to_string();
        let exit_operational_impact = t(
            self.locale,
            preview.exit_contribution_operational_impact_key.as_str(),
        )
        .to_string();
        let exit_disable =
            t(self.locale, preview.exit_contribution_disable_key.as_str()).to_string();
        let blocked_reason = if preview.can_join {
            None
        } else {
            Some(
                t(
                    self.locale,
                    preview
                        .blocked_reason_key
                        .as_deref()
                        .unwrap_or("join.error.unavailable"),
                )
                .to_string(),
            )
        };
        let administrators = if preview.administrators.is_empty() {
            t(self.locale, "common.none").to_string()
        } else {
            preview.administrators.join(" · ")
        };
        let capabilities = if preview.capability_summary.is_empty() {
            t(self.locale, "common.none").to_string()
        } else {
            preview
                .capability_summary
                .iter()
                .map(|capability| t(self.locale, profile_capability_key(*capability)))
                .collect::<Vec<_>>()
                .join(" · ")
        };
        let local_capabilities = if preview.locally_contributed_capabilities.is_empty() {
            t(self.locale, "join.local_capabilities_none").to_string()
        } else {
            preview
                .locally_contributed_capabilities
                .iter()
                .map(|capability| t(self.locale, profile_capability_key(*capability)))
                .collect::<Vec<_>>()
                .join(" · ")
        };
        let privacy_changes = if preview.privacy_changes.is_empty() {
            t(self.locale, "common.none").to_string()
        } else {
            preview.privacy_changes.join(" · ")
        };
        let endpoints = if preview.endpoints.is_empty() {
            t(self.locale, "common.none").to_string()
        } else {
            preview
                .endpoints
                .iter()
                .map(|endpoint| {
                    format!(
                        "{}://{}:{}",
                        discovery_transport_name(endpoint.transport),
                        endpoint.host,
                        endpoint.port
                    )
                })
                .collect::<Vec<_>>()
                .join(" · ")
        };
        let member_limit = preview
            .member_limit
            .map(|limit| format_integer(self.locale, i64::from(limit)))
            .unwrap_or_else(|| t(self.locale, "common.unlimited").to_string());
        let member_count = tr(
            self.locale,
            "join.member_count",
            &[
                (
                    "count",
                    &format_integer(self.locale, i64::from(preview.member_count)),
                ),
                ("limit", &member_limit),
            ],
        );
        let issued_at = format_diagnostic_timestamp(self.locale, preview.issued_at_unix_ms);
        let expires_at = format_diagnostic_timestamp(self.locale, preview.expires_at_unix_ms);
        let terms_control = checkbox_control(
            SharedString::from("join-network-terms"),
            t(self.locale, "join.terms_checkbox"),
            checked,
            !can_join || pending,
        )
        .debug_selector(|| "visual-join-network-terms".into())
        .on_click(cx.listener(|shell, _, _, cx| {
            if shell.pending_commands > 0 {
                return;
            }
            let can_join = shell
                .recent_network_join_preview
                .as_ref()
                .is_some_and(|preview| preview.can_join && !preview.already_member);
            if can_join {
                shell.network_join_terms_confirmed = !shell.network_join_terms_confirmed;
                cx.notify();
            }
        }));
        let exit_control = checkbox_control(
            SharedString::from("join-network-exit-contribution"),
            t(self.locale, "join.exit.consent"),
            exit_checked,
            !can_join || pending,
        )
        .debug_selector(|| "visual-join-network-exit-contribution".into())
        .on_click(cx.listener(|shell, _, _, cx| {
            if shell.pending_commands > 0 {
                return;
            }
            let can_join = shell
                .recent_network_join_preview
                .as_ref()
                .is_some_and(|preview| {
                    preview.can_join
                        && !preview.already_member
                        && preview.join_requires_exit_consent
                });
            if can_join {
                shell.network_join_exit_contribution_confirmed =
                    !shell.network_join_exit_contribution_confirmed;
                cx.notify();
            }
        }));
        let confirm = button(
            SharedString::from("join-network-confirm"),
            t(self.locale, "join.confirm"),
            ButtonVariant::Primary,
            !can_join || !checked || (exit_consent_required && !exit_checked) || pending,
        )
        .debug_selector(|| "visual-join-network-confirm".into())
        .on_click(cx.listener(|shell, _, _, cx| {
            if shell.pending_commands == 0
                && shell.network_join_terms_confirmed
                && shell
                    .recent_network_join_preview
                    .as_ref()
                    .is_some_and(|preview| {
                        preview.can_join
                            && !preview.already_member
                            && (!preview.join_requires_exit_consent
                                || shell.network_join_exit_contribution_confirmed)
                    })
            {
                shell.confirm_join_preview(cx);
            }
        }));
        let preview_again = button(
            SharedString::from("join-network-preview-again"),
            t(self.locale, "join.preview_again"),
            ButtonVariant::Secondary,
            pending,
        )
        .debug_selector(|| "visual-join-network-preview-again".into())
        .on_click(cx.listener(|shell, _, _, cx| {
            shell.submit_manifest_action("preview_join_network", cx);
        }));
        let mut card = div()
            .id("join-network-preview")
            .debug_selector(|| "visual-join-network-preview".into())
            .role(Role::Article)
            .aria_label(t(self.locale, "join.preview_title").to_string())
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_md()
            .bg(rgba(COLOR_PANEL_RAISED))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(10.0))
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(COLOR_MUTED))
                            .child(t(self.locale, "join.preview_title").to_string()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(COLOR_TEXT))
                            .child(preview.network_name.clone()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(profile_status_chip(t(
                        self.locale,
                        if can_join {
                            "join.can_join"
                        } else {
                            "join.cannot_join"
                        },
                    )))
                    .when(preview.already_member, |chips| {
                        chips.child(profile_status_chip(
                            t(self.locale, "join.already_member").to_string(),
                        ))
                    })
                    .child(profile_status_chip(
                        t(self.locale, profile_join_mode_key(preview.join_mode)).to_string(),
                    ))
                    .child(profile_status_chip(
                        t(self.locale, profile_visibility_key(preview.visibility)).to_string(),
                    )),
            );
        if let Some(message) = blocked_reason {
            card = card.child(discovery_notice(
                "visual-join-network-blocked",
                message,
                SemanticState::Danger,
            ));
        }
        card = card
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(diagnostic_metadata_row(
                        t(self.locale, "join.network_id"),
                        preview.network_id.clone(),
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "join.public_key"),
                        preview.public_key.clone(),
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "join.owner"),
                        preview.owner.clone(),
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "join.administrators"),
                        administrators,
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "join.discovery_revision"),
                        format_integer(self.locale, preview.discovery_revision as i64),
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "join.network_policy_revision"),
                        format_integer(self.locale, preview.network_policy_revision as i64),
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "join.privacy_policy_revision"),
                        format_integer(self.locale, preview.privacy_policy_revision as i64),
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "join.members"),
                        member_count,
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "join.exit_contribution"),
                        t(
                            self.locale,
                            match preview.exit_contribution_mode {
                                ExitContributionMode::RequiredOnJoin => "join.exit.required",
                                ExitContributionMode::Autonomous => "join.exit.autonomous",
                                ExitContributionMode::ApprovalRequired => {
                                    "join.exit.approval_required"
                                }
                                ExitContributionMode::Disabled => "join.exit.not_required",
                            },
                        ),
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "join.privacy_changes"),
                        privacy_changes,
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "join.capabilities"),
                        capabilities,
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "join.local_capabilities"),
                        local_capabilities,
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "join.contribution_policy"),
                        t(self.locale, "join.contribution.local_control"),
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "join.endpoints"),
                        endpoints,
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "join.issued_at"),
                        issued_at,
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "join.expires_at"),
                        expires_at,
                    ))
                    .child(diagnostic_metadata_row(
                        t(self.locale, "join.preview_digest"),
                        preview.preview_digest.clone(),
                    )),
            )
            .child(discovery_notice(
                "visual-join-network-local-control",
                t(self.locale, "join.local_revocable").to_string(),
                SemanticState::Info,
            ))
            .when(show_exit_disclosure, |card| {
                card.child(
                    div()
                        .debug_selector(|| "visual-join-network-exit-disclosure".into())
                        .flex()
                        .flex_col()
                        .gap_1()
                        .rounded_md()
                        .p_3()
                        .text_xs()
                        .line_height(px(18.0))
                        .text_color(rgba(COLOR_TEXT_SECONDARY))
                        .child(exit_purpose)
                        .child(exit_bandwidth)
                        .child(exit_traffic)
                        .child(exit_logging)
                        .child(exit_operational_impact)
                        .child(exit_disable)
                        .child(t(
                            self.locale,
                            if preview.local_exit_available {
                                "join.exit.local_supported"
                            } else {
                                "join.exit.local_unsupported"
                            },
                        ))
                        .child(t(
                            self.locale,
                            if preview.local_exit_enabled {
                                "join.exit.local_enabled"
                            } else {
                                "join.exit.local_not_enabled"
                            },
                        ))
                        .when(preview.can_join_without_local_exit, |disclosure| {
                            disclosure
                                .child(t(self.locale, "join.exit.can_join_without_local_exit"))
                        }),
                )
            })
            .child(terms_control)
            .when(exit_consent_required, |card| card.child(exit_control))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(preview_again)
                    .child(confirm),
            );

        card.into_any_element()
    }

    fn network_profile_result_card(&self) -> Option<gpui::AnyElement> {
        if let Some(preview) = &self.recent_network_profile_preview {
            return Some(self.network_profile_preview_card(preview));
        }
        self.recent_network_profile_updated
            .as_ref()
            .map(|result| self.network_profile_updated_card(result))
    }

    fn network_profile_preview_card(&self, preview: &NetworkProfilePreview) -> gpui::AnyElement {
        let signature_valid = preview
            .projections
            .public_card
            .currently_published
            .as_ref()
            .is_some_and(|profile| profile.validate().is_ok());
        let status = div()
            .debug_selector(|| "visual-network-profile-preview".into())
            .flex()
            .flex_wrap()
            .gap_2()
            .child(profile_status_chip(tr(
                self.locale,
                "profile.revision",
                &[(
                    "revision",
                    &format_integer(self.locale, preview.current_revision as i64),
                )],
            )))
            .child(profile_status_chip(tr(
                self.locale,
                "profile.revision",
                &[(
                    "revision",
                    &format_integer(self.locale, preview.candidate_revision as i64),
                )],
            )))
            .child(profile_status_chip(
                t(
                    self.locale,
                    if preview.can_apply {
                        "profile.can_apply"
                    } else {
                        "profile.cannot_apply"
                    },
                )
                .to_string(),
            ))
            .when(preview.conflict, |status| {
                status.child(
                    div()
                        .debug_selector(|| "visual-network-profile-conflict".into())
                        .w_full()
                        .rounded_md()
                        .bg(rgba(DARK_THEME.colors.warning_soft.rgba_hex()))
                        .p_3()
                        .text_xs()
                        .line_height(px(18.0))
                        .text_color(rgba(COLOR_WARNING))
                        .child(tr(
                            self.locale,
                            "profile.conflict",
                            &[
                                (
                                    "current",
                                    &format_integer(self.locale, preview.current_revision as i64),
                                ),
                                (
                                    "expected",
                                    &format_integer(self.locale, preview.expected_revision as i64),
                                ),
                            ],
                        )),
                )
            })
            .into_any_element();

        let diff = if preview.diff.is_empty() {
            div()
                .debug_selector(|| "visual-network-profile-no-changes".into())
                .text_xs()
                .text_color(rgba(COLOR_MUTED))
                .child(t(self.locale, "profile.no_changes").to_string())
                .into_any_element()
        } else {
            div()
                .debug_selector(|| "visual-network-profile-diff".into())
                .flex()
                .flex_col()
                .gap_2()
                .children(preview.diff.iter().map(|entry| {
                    let before = entry
                        .before
                        .clone()
                        .unwrap_or_else(|| t(self.locale, "common.none").to_string());
                    let after = entry
                        .after
                        .clone()
                        .unwrap_or_else(|| t(self.locale, "common.none").to_string());
                    div()
                        .id(SharedString::from(format!(
                            "network-profile-diff-{}",
                            entry.path
                        )))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .rounded_md()
                        .bg(rgba(COLOR_BG))
                        .p_2()
                        .child(
                            div()
                                .font_family("monospace")
                                .text_xs()
                                .text_color(rgba(COLOR_TEXT))
                                .child(entry.path.clone()),
                        )
                        .child(diagnostic_metadata_row(
                            t(self.locale, "configuration.before"),
                            before,
                        ))
                        .child(diagnostic_metadata_row(
                            t(self.locale, "configuration.after"),
                            after,
                        ))
                        .into_any_element()
                }))
                .into_any_element()
        };

        self.network_profile_result_layout(
            "profile.preview_title",
            &preview.network_name,
            &preview.projections,
            signature_valid,
            NetworkProfileResultSections {
                status,
                details: profile_result_section(t(self.locale, "profile.diff"), diff),
                warnings: &preview.warnings,
            },
        )
    }

    fn network_profile_updated_card(&self, result: &NetworkProfileApplyResult) -> gpui::AnyElement {
        let signature_valid = result.signed_profile.validate().is_ok();
        let status = div()
            .debug_selector(|| "visual-network-profile-updated".into())
            .flex()
            .flex_wrap()
            .gap_2()
            .child(profile_status_chip(tr(
                self.locale,
                "profile.revision",
                &[(
                    "revision",
                    &format_integer(self.locale, result.previous_revision as i64),
                )],
            )))
            .child(profile_status_chip(tr(
                self.locale,
                "profile.revision",
                &[(
                    "revision",
                    &format_integer(self.locale, result.revision as i64),
                )],
            )))
            .child(profile_status_chip(tr(
                self.locale,
                "profile.audit",
                &[("action", &result.audit_action)],
            )))
            .into_any_element();

        let changed_paths = if result.changed_paths.is_empty() {
            div()
                .debug_selector(|| "visual-network-profile-no-changes".into())
                .text_xs()
                .text_color(rgba(COLOR_MUTED))
                .child(t(self.locale, "profile.no_changes").to_string())
                .into_any_element()
        } else {
            div()
                .debug_selector(|| "visual-network-profile-changed-paths".into())
                .flex()
                .flex_col()
                .gap_1()
                .children(result.changed_paths.iter().map(|path| {
                    div()
                        .font_family("monospace")
                        .rounded_sm()
                        .bg(rgba(COLOR_BG))
                        .p_2()
                        .text_xs()
                        .text_color(rgba(COLOR_TEXT_SECONDARY))
                        .child(path.clone())
                        .into_any_element()
                }))
                .into_any_element()
        };

        self.network_profile_result_layout(
            "profile.updated_title",
            &result.network_name,
            &result.projections,
            signature_valid,
            NetworkProfileResultSections {
                status,
                details: profile_result_section(
                    t(self.locale, "configuration.history.changed_paths"),
                    changed_paths,
                ),
                warnings: &result.warnings,
            },
        )
    }

    fn network_profile_result_layout(
        &self,
        title_key: &str,
        network_name: &str,
        projections: &NetworkProfileProjections,
        signature_valid: bool,
        sections: NetworkProfileResultSections<'_>,
    ) -> gpui::AnyElement {
        let NetworkProfileResultSections {
            status,
            details,
            warnings,
        } = sections;
        let title = t(self.locale, title_key).to_string();
        let projection_grid = self.network_profile_projection_grid(projections, signature_valid);
        let withheld = div()
            .debug_selector(|| "visual-network-profile-withheld".into())
            .flex()
            .flex_col()
            .gap_1()
            .rounded_md()
            .p_3()
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight(600.0))
                    .text_color(rgba(COLOR_WARNING))
                    .child(t(self.locale, "profile.withheld").to_string()),
            )
            .child(
                div()
                    .font_family("monospace")
                    .text_xs()
                    .line_height(px(18.0))
                    .text_color(rgba(COLOR_MUTED))
                    .child(projections.public_card.withheld_categories.join(", ")),
            );
        let warning_section = self.network_profile_warnings(warnings);

        div()
            .id("network-profile-result")
            .debug_selector(|| "visual-network-profile-result".into())
            .role(Role::Article)
            .aria_label(title.clone())
            .mt_3()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_lg()
            .bg(rgba(COLOR_PANEL_RAISED))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(10.0))
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(COLOR_MUTED))
                            .child(title.to_uppercase()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(COLOR_TEXT))
                            .child(network_name.to_string()),
                    ),
            )
            .child(status)
            .child(projection_grid)
            .child(withheld)
            .child(details)
            .child(warning_section)
            .into_any_element()
    }

    fn network_profile_projection_grid(
        &self,
        projections: &NetworkProfileProjections,
        signature_valid: bool,
    ) -> Div {
        let public_card = self.network_profile_document_card(
            "profile.public_card",
            &projections.public_card.candidate,
            signature_valid,
            None,
            None,
        );
        let member_view = self.network_profile_document_card(
            "profile.member_view",
            &projections.member_view.profile,
            signature_valid,
            Some(projections.member_view.member_count),
            Some(projections.member_view.administrator_count),
        );
        let admin_view = self.network_profile_document_card(
            "profile.admin_view",
            &projections.administrator_view.profile,
            signature_valid,
            Some(projections.administrator_view.member_count),
            Some(projections.administrator_view.administrator_count),
        );
        let secret = &projections.local_secret;
        let secret_card = div()
            .debug_selector(|| "visual-network-profile-local-secret".into())
            .flex_1()
            .min_w(px(220.0))
            .rounded_md()
            .bg(rgba(COLOR_BG))
            .p_3()
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight(600.0))
                    .text_color(rgba(COLOR_TEXT))
                    .child(t(self.locale, "profile.local_secret").to_string()),
            )
            .child(diagnostic_metadata_row(
                t(self.locale, "field.storage"),
                secret.storage.clone(),
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "profile.identity"),
                t(
                    self.locale,
                    if secret.identity_present {
                        "profile.identity_present"
                    } else {
                        "profile.identity_absent"
                    },
                )
                .to_string(),
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "profile.secret"),
                t(
                    self.locale,
                    if secret.secret_material_included {
                        "profile.secret_included"
                    } else {
                        "profile.secret_not_included"
                    },
                )
                .to_string(),
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "identity.public_key_fingerprint"),
                secret.public_key_fingerprint.clone(),
            ));

        div()
            .debug_selector(|| "visual-network-profile-projections".into())
            .flex()
            .flex_wrap()
            .gap_2()
            .child(public_card)
            .child(member_view)
            .child(admin_view)
            .child(secret_card)
    }

    fn network_profile_document_card(
        &self,
        title_key: &str,
        document: &NetworkPublicProfileDocument,
        signature_valid: bool,
        member_count: Option<usize>,
        administrator_count: Option<usize>,
    ) -> Div {
        let capabilities = if document.capability_declarations.is_empty() {
            t(self.locale, "common.none").to_string()
        } else {
            document
                .capability_declarations
                .iter()
                .map(|capability| t(self.locale, profile_capability_key(*capability)))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let tags = if document.tags.is_empty() {
            t(self.locale, "common.none").to_string()
        } else {
            document.tags.join(", ")
        };
        let mut metadata = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .pt_2()
            .child(profile_status_chip(tr(
                self.locale,
                "profile.revision",
                &[(
                    "revision",
                    &format_integer(self.locale, document.revision as i64),
                )],
            )))
            .child(profile_status_chip(tr(
                self.locale,
                "profile.schema_version",
                &[(
                    "version",
                    &format_integer(self.locale, document.schema_version as i64),
                )],
            )))
            .child(profile_status_chip(tr(
                self.locale,
                "profile.expires_at",
                &[(
                    "time",
                    &format_diagnostic_timestamp(self.locale, document.expires_at_unix_ms),
                )],
            )));
        if signature_valid {
            metadata = metadata.child(profile_status_chip(
                t(self.locale, "profile.signature_valid").to_string(),
            ));
        }
        if let Some(count) = member_count {
            metadata = metadata.child(profile_status_chip(tr(
                self.locale,
                "profile.member_count",
                &[("count", &format_integer(self.locale, count as i64))],
            )));
        }
        if let Some(count) = administrator_count {
            metadata = metadata.child(profile_status_chip(tr(
                self.locale,
                "profile.admin_count",
                &[("count", &format_integer(self.locale, count as i64))],
            )));
        }

        div()
            .debug_selector(move || profile_document_selector(title_key).to_string())
            .flex_1()
            .min_w(px(220.0))
            .rounded_md()
            .bg(rgba(COLOR_BG))
            .p_3()
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight(600.0))
                    .text_color(rgba(COLOR_TEXT))
                    .child(t(self.locale, title_key).to_string()),
            )
            .child(diagnostic_metadata_row(
                t(self.locale, "field.purpose"),
                document.purpose.clone(),
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "field.icon"),
                document.icon.clone(),
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "field.language"),
                document.language.clone(),
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "field.region"),
                document.region.clone(),
            ))
            .child(diagnostic_metadata_row(t(self.locale, "field.tags"), tags))
            .child(diagnostic_metadata_row(
                t(self.locale, "field.capability_declarations"),
                capabilities,
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "field.visibility"),
                t(self.locale, profile_visibility_key(document.visibility)).to_string(),
            ))
            .child(diagnostic_metadata_row(
                t(self.locale, "field.join_mode"),
                t(self.locale, profile_join_mode_key(document.join_mode)).to_string(),
            ))
            .child(metadata)
    }

    fn network_profile_warnings(
        &self,
        warnings: &[fuckipv6_common::domain::NetworkProfileWarning],
    ) -> Div {
        let body = if warnings.is_empty() {
            div()
                .debug_selector(|| "visual-network-profile-no-warnings".into())
                .text_xs()
                .text_color(rgba(COLOR_MUTED))
                .child(t(self.locale, "profile.no_warnings").to_string())
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .children(warnings.iter().enumerate().map(|(index, warning)| {
                    let code = profile_warning_key(warning.code).to_string();
                    let capability = warning
                        .capability
                        .map(|capability| t(self.locale, profile_capability_key(capability)))
                        .unwrap_or_default();
                    div()
                        .id(SharedString::from(format!(
                            "network-profile-warning-{index}"
                        )))
                        .debug_selector(move || format!("visual-network-profile-warning-{code}"))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .rounded_md()
                        .p_2()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(gpui::FontWeight(600.0))
                                .text_color(rgba(COLOR_WARNING))
                                .child(
                                    t(self.locale, profile_warning_key(warning.code)).to_string(),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .line_height(px(18.0))
                                .text_color(rgba(COLOR_TEXT_SECONDARY))
                                .child(warning.message.clone()),
                        )
                        .when(!capability.is_empty(), |warning| {
                            warning.child(
                                div()
                                    .font_family("monospace")
                                    .text_xs()
                                    .text_color(rgba(COLOR_MUTED))
                                    .child(capability),
                            )
                        })
                        .into_any_element()
                }))
                .into_any_element()
        };
        profile_result_section(t(self.locale, "profile.warnings"), body)
    }

    fn coordinator_governance_notice(&self) -> Option<gpui::AnyElement> {
        let reason =
            coordinator_governance_freeze_reason(&self.snapshot, self.selected_network.as_deref())?;
        Some(
            div()
                .id("coordinator-governance-frozen")
                .debug_selector(|| "visual-coordinator-governance-frozen".into())
                .flex()
                .flex_col()
                .gap_1()
                .rounded_md()
                .bg(rgba(DARK_THEME.colors.warning_soft.rgba_hex()))
                .p_3()
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight(600.0))
                        .text_color(rgba(COLOR_WARNING))
                        .child(t(self.locale, "coordinator_governance.frozen.title").to_string()),
                )
                .child(
                    div()
                        .text_xs()
                        .line_height(px(18.0))
                        .text_color(rgba(COLOR_TEXT_SECONDARY))
                        .child(t(self.locale, reason.message_key()).to_string()),
                )
                .child(
                    div()
                        .text_xs()
                        .line_height(px(18.0))
                        .text_color(rgba(COLOR_MUTED))
                        .child(
                            t(self.locale, "coordinator_governance.frozen.recovery").to_string(),
                        ),
                )
                .into_any_element(),
        )
    }

    fn action_panel(&self, layout: LayoutClass, cx: &mut Context<Self>) -> impl IntoElement {
        let presentation = ActionPanelPresentation::for_layout(layout);
        let actions = self
            .page()
            .cloned()
            .map(|page| page.actions)
            .unwrap_or_default()
            .into_iter()
            .filter(|action| !is_configuration_action(&action.id))
            .collect::<Vec<_>>();
        let action_count = actions.len();
        let normalized_query = self.action_panel_query.trim().to_lowercase();
        let matching_actions = actions
            .iter()
            .filter(|action| {
                if normalized_query.is_empty() {
                    return true;
                }
                let action_label = t(self.locale, &action.label_key).to_lowercase();
                let group_label = t(self.locale, &action.group_label_key).to_lowercase();
                action_label.contains(&normalized_query) || group_label.contains(&normalized_query)
            })
            .cloned()
            .collect::<Vec<_>>();
        let available_actions = matching_actions
            .iter()
            .filter(|action| action.backend_status != NodeUiBackendStatus::Unavailable)
            .cloned()
            .collect::<Vec<_>>();
        let unavailable_actions = matching_actions
            .iter()
            .filter(|action| action.backend_status == NodeUiBackendStatus::Unavailable)
            .cloned()
            .collect::<Vec<_>>();

        let mut group_sections = Vec::new();
        for group in NODE_UI_ACTION_GROUPS {
            let group_actions = available_actions
                .iter()
                .filter(|action| action.group == group)
                .cloned()
                .collect::<Vec<_>>();
            let Some(first_action) = group_actions.first() else {
                continue;
            };
            let group_label = t(self.locale, &first_action.group_label_key).to_string();
            let group_key = group.label_key().to_string();
            let collapsed =
                normalized_query.is_empty() && self.collapsed_action_groups.contains(&group_key);
            let selector_key = group_key.replace('.', "-");
            let group_cards = group_actions
                .into_iter()
                .map(|action| self.action_card(action, cx))
                .collect::<Vec<_>>();
            let toggle_key = group_key.clone();
            let group_head_label = group_label.clone();
            let mut section = div()
                .id(SharedString::from(format!("action-group-{selector_key}")))
                .debug_selector({
                    let selector = format!("visual-action-group-{selector_key}");
                    move || selector
                })
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "action-group-head-{selector_key}"
                        )))
                        .debug_selector({
                            let selector = format!("visual-action-group-toggle-{selector_key}");
                            move || selector
                        })
                        .role(Role::Button)
                        .aria_label(group_label.clone())
                        .aria_expanded(!collapsed)
                        .focusable()
                        .tab_stop(true)
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .h(px(30.0))
                        .px_1()
                        .text_size(px(11.0))
                        .font_weight(gpui::FontWeight(600.0))
                        .text_color(rgba(COLOR_TEXT_SECONDARY))
                        .on_click(cx.listener(move |shell, _, _, cx| {
                            if !shell.collapsed_action_groups.remove(&toggle_key) {
                                shell.collapsed_action_groups.insert(toggle_key.clone());
                            }
                            cx.notify();
                        }))
                        .child(group_head_label)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .rounded_md()
                                        .px_2()
                                        .py_1()
                                        .text_size(px(10.0))
                                        .text_color(rgba(COLOR_MUTED))
                                        .bg(rgba(COLOR_PANEL_RAISED))
                                        .child(group_cards.len().to_string()),
                                )
                                .child(if collapsed { "›" } else { "⌄" }),
                        ),
                );
            if !collapsed {
                section = section.child(div().flex().flex_col().gap_2().children(group_cards));
            }
            group_sections.push(section.into_any_element());
        }

        if !unavailable_actions.is_empty() {
            let unavailable_key = "common.actions_unavailable_heading";
            let unavailable_label = t(self.locale, unavailable_key).to_string();
            let collapsed = normalized_query.is_empty()
                && self.collapsed_action_groups.contains(unavailable_key);
            let cards = unavailable_actions
                .into_iter()
                .map(|action| self.action_card(action, cx))
                .collect::<Vec<_>>();
            let toggle_key = unavailable_key.to_string();
            let mut section = div()
                .id("action-group-unavailable")
                .debug_selector(|| "visual-action-group-unavailable".into())
                .flex()
                .flex_col()
                .gap_2()
                .mt_2()
                .child(
                    div()
                        .id("action-group-head-unavailable")
                        .debug_selector(|| "visual-action-group-toggle-unavailable".into())
                        .role(Role::Button)
                        .aria_label(unavailable_label.clone())
                        .aria_expanded(!collapsed)
                        .focusable()
                        .tab_stop(true)
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .h(px(30.0))
                        .px_1()
                        .text_size(px(11.0))
                        .font_weight(gpui::FontWeight(600.0))
                        .text_color(rgba(COLOR_WARNING))
                        .on_click(cx.listener(move |shell, _, _, cx| {
                            if !shell.collapsed_action_groups.remove(&toggle_key) {
                                shell.collapsed_action_groups.insert(toggle_key.clone());
                            }
                            cx.notify();
                        }))
                        .child(unavailable_label)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .rounded_md()
                                        .px_2()
                                        .py_1()
                                        .text_size(px(10.0))
                                        .text_color(rgba(COLOR_MUTED))
                                        .bg(rgba(COLOR_PANEL_RAISED))
                                        .child(cards.len().to_string()),
                                )
                                .child(if collapsed { "›" } else { "⌄" }),
                        ),
                );
            if !collapsed {
                section = section.child(div().flex().flex_col().gap_2().children(cards));
            }
            group_sections.push(section.into_any_element());
        }

        let action_search_input = self
            .action_panel_search_input
            .as_ref()
            .map(|input| input.clone().into_any_element());
        let clear_action_search = (!self.action_panel_query.trim().is_empty()).then(|| {
            button(
                SharedString::from("clear-action-panel-search"),
                "",
                ButtonVariant::Ghost,
                false,
            )
            .absolute()
            .right(px(4.0))
            .top(px(3.0))
            .w(px(28.0))
            .h(px(28.0))
            .px_0()
            .aria_label(t(self.locale, "common.actions_search_clear").to_string())
            .child(lucide_icon(
                include_bytes!("../../assets/icons/x.svg"),
                14.0,
                COLOR_MUTED,
            ))
            .on_click(cx.listener(|shell, _, _, cx| {
                shell.action_panel_query.clear();
                if let Some(input) = &shell.action_panel_search_input {
                    input.update(cx, |input, cx| input.set_value("", cx));
                }
                cx.notify();
            }))
        });
        let action_search = div()
            .id("action-panel-search")
            .debug_selector(|| "visual-action-search".into())
            .relative()
            .flex()
            .items_center()
            .h(px(34.0))
            .child(
                div()
                    .absolute()
                    .left(px(11.0))
                    .top(px(9.0))
                    .child(lucide_icon(
                        include_bytes!("../../assets/icons/search.svg"),
                        15.0,
                        COLOR_FAINT,
                    )),
            )
            .children(action_search_input)
            .children(clear_action_search);

        let body = if self.selected_page == "topology" {
            div()
                .id("topology-action-panel-scroll")
                .overflow_y_scroll()
                .child(self.topology_action_panel_body(cx))
        } else {
            let governance_notice = actions
                .iter()
                .any(|action| {
                    action_coordinator_governance_freeze_reason(
                        action,
                        &self.snapshot,
                        self.selected_network.as_deref(),
                    )
                    .is_some()
                })
                .then(|| self.coordinator_governance_notice())
                .flatten();
            div()
                .id("action-panel-body")
                .flex()
                .flex_col()
                .gap_4()
                .px_3()
                .pb_3()
                .overflow_y_scroll()
                .children(governance_notice)
                .when(actions.is_empty(), |body| {
                    body.child(
                        div()
                            .id("no-shared-actions")
                            .debug_selector(|| "visual-action-empty".into())
                            .pt_3()
                            .px_4()
                            .text_size(px(11.0))
                            .line_height(px(16.0))
                            .text_color(rgba(COLOR_MUTED))
                            .child(t(self.locale, "common.page_actions_empty").to_string()),
                    )
                })
                .when(!actions.is_empty() && matching_actions.is_empty(), |body| {
                    body.child(
                        div()
                            .id("no-matching-actions")
                            .debug_selector(|| "visual-action-search-empty".into())
                            .py_5()
                            .text_center()
                            .text_size(px(11.0))
                            .text_color(rgba(COLOR_MUTED))
                            .child(t(self.locale, "common.actions_no_results").to_string()),
                    )
                })
                .children(group_sections)
                .when_some(self.network_profile_result_card(), |body, result| {
                    body.child(div().pb_3().child(result))
                })
        };

        let header = div()
            .debug_selector(|| "visual-action-title".into())
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .h(px(40.5))
            .px_4()
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(gpui::FontWeight(600.0))
                    .text_color(rgba(COLOR_TEXT))
                    .child(t(self.locale, "common.actions_panel_title").to_string()),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(rgba(COLOR_MUTED))
                    .child(tr(
                        self.locale,
                        "common.actions_panel_count",
                        &[("count", &action_count.to_string())],
                    )),
            );

        let mut panel = div()
            .id(app_region_id(AppRegion::ActionPanel))
            .debug_selector(|| "visual-actions".into())
            .role(Role::Complementary)
            .aria_label(t(self.locale, "common.actions_panel_title").to_string())
            .flex()
            .flex_col()
            .min_w(px(0.0))
            .overflow_hidden()
            .border_1()
            .border_color(rgba(COLOR_DIVIDER))
            .rounded_lg()
            .bg(rgba(DARK_THEME.colors.surface.rgba_hex()))
            .shadow(vec![gpui::BoxShadow::new(
                px(0.0),
                px(1.0),
                rgba(0x00000066).into(),
            )
            .blur_radius(px(3.0))])
            .child(header);
        if !actions.is_empty() && self.selected_page != "topology" {
            panel = panel.child(div().px_3().pb_2().child(action_search));
        }
        panel = panel.child(body);

        panel = match presentation {
            ActionPanelPresentation::Stacked => panel.w_full().max_h(px(360.0)),
            ActionPanelPresentation::Compact | ActionPanelPresentation::Full => panel
                .max_h_full()
                .w(px(presentation.width_px().unwrap_or_default() as f32))
                .flex_shrink_0(),
        };
        panel
    }
}

impl Render for GpuiShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        debug_assert_eq!(APP_REGION_ORDER, AppRegion::ALL);
        let layout = layout_for_window_width(f32::from(window.viewport_size().width));
        let presentation = ActionPanelPresentation::for_layout(layout);
        self.sync_configuration_editor(window, cx);
        if self.selected_page == "topology" {
            cx.on_next_frame(window, |shell, _, cx| shell.ensure_topology_directory(cx));
        }
        let content = self.page_content(layout, cx);
        let action_panel = self.action_panel(layout, cx);
        let workspace = if presentation == ActionPanelPresentation::Stacked {
            div()
                .id("page-workspace")
                .debug_selector(|| "visual-workspace".into())
                .role(A11Y_TAB_PANEL_ROLE)
                .aria_label(t(self.locale, "common.module_pages").to_string())
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .gap_5()
                .p_5()
                .overflow_hidden()
                .child(content)
                .child(action_panel)
        } else {
            div()
                .id("page-workspace")
                .debug_selector(|| "visual-workspace".into())
                .role(A11Y_TAB_PANEL_ROLE)
                .aria_label(t(self.locale, "common.module_pages").to_string())
                .flex()
                .flex_row()
                .flex_1()
                .min_h(px(0.0))
                .min_w(px(0.0))
                .items_start()
                .gap_5()
                .p_5()
                .overflow_hidden()
                .child(content)
                .child(action_panel)
        };
        let mut main = div()
            .id("main-content")
            .role(A11Y_MAIN_ROLE)
            .aria_label(t(self.locale, "common.main_content").to_string())
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .bg(rgba(COLOR_BG))
            .child(self.header(cx));

        main = main.child(self.local_tabs(cx));

        if let Some(search_panel) = self.search_results_panel(cx) {
            main = main.child(search_panel);
        }

        if self.error.is_some() {
            main = main.child(self.error_banner(cx));
        }

        main = main.child(workspace);

        div()
            .id("gpui-root")
            .role(A11Y_ROOT_ROLE)
            .aria_label("fuckipv6")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::refresh_snapshot))
            .on_action(cx.listener(Self::focus_search))
            .flex()
            .w_full()
            .h_full()
            .bg(rgba(COLOR_BG))
            .child(self.sidebar(cx))
            .child(main)
    }
}

fn layout_for_window_width(width_px: f32) -> LayoutClass {
    LayoutClass::for_width(width_px.clamp(0.0, u16::MAX as f32) as u16)
}

fn window_min_size() -> gpui::Size<gpui::Pixels> {
    size(
        px(DIMENSIONS.window_min_width_px as f32),
        px(DIMENSIONS.window_min_height_px as f32),
    )
}

fn loading_skeleton() -> Div {
    let block = |height: f32| {
        div()
            .w_full()
            .h(px(height))
            .rounded_lg()
            .bg(rgba(COLOR_PANEL_RAISED))
    };

    div()
        .flex()
        .flex_col()
        .gap_4()
        .px_6()
        .pb_6()
        .pt_3()
        .flex_1()
        .child(
            div()
                .flex()
                .gap_3()
                .child(div().flex_1().child(block(92.0)))
                .child(div().flex_1().child(block(92.0))),
        )
        .child(block(180.0))
        .child(block(140.0))
}

fn page_tab_control(
    id: SharedString,
    label: impl Into<SharedString>,
    selected: bool,
) -> gpui::Stateful<Div> {
    let label = label.into();
    let mut tab = div()
        .id(id)
        .role(Role::Tab)
        .aria_label(label.clone())
        .aria_selected(selected)
        .focusable()
        .tab_stop(true)
        .relative()
        .flex()
        .items_center()
        .h(px(34.0))
        .px_3()
        .rounded_sm()
        .bg(rgba(if selected {
            DARK_THEME.colors.surface_hover.rgba_hex()
        } else {
            0x00000000
        }))
        .text_size(px(12.0))
        .font_weight(gpui::FontWeight(if selected { 600.0 } else { 400.0 }))
        .text_color(rgba(if selected { COLOR_TEXT } else { COLOR_MUTED }))
        .child(label);
    if selected {
        tab = tab.child(
            div()
                .absolute()
                .left(px(10.0))
                .right(px(10.0))
                .bottom(px(-7.0))
                .h(px(2.0))
                .rounded_full()
                .bg(rgba(COLOR_ACCENT)),
        );
    }
    tab
}

fn sync_badge(id: impl Into<gpui::ElementId>, label: String) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .role(Role::Status)
        .aria_label(label.clone())
        .flex_shrink_0()
        .rounded_full()
        .bg(rgba(DARK_THEME.colors.warning_soft.rgba_hex()))
        .px_2()
        .py_1()
        .text_size(px(10.0))
        .font_weight(gpui::FontWeight(600.0))
        .text_color(rgba(COLOR_WARNING))
        .child(label)
}

fn configuration_audit_details(
    locale: Locale,
    audit: &ConfigurationHistoryEntry,
) -> gpui::AnyElement {
    let field = |label: String, value: String, monospace: bool| {
        let mut value_element = div().text_sm().text_color(rgba(COLOR_TEXT)).child(value);
        if monospace {
            value_element = value_element.font_family("monospace");
        }
        div()
            .flex()
            .flex_col()
            .gap_1()
            .flex_1()
            .min_w(px(150.0))
            .p_2()
            .rounded_sm()
            .bg(rgba(COLOR_BG))
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(rgba(COLOR_MUTED))
                    .child(label),
            )
            .child(value_element)
            .into_any_element()
    };

    let mut details = div()
        .id(SharedString::from(format!(
            "configuration-audit-{}",
            audit.transaction_id
        )))
        .flex()
        .flex_wrap()
        .gap_2()
        .child(field(
            t(locale, "configuration.history.revision").to_string(),
            audit.revision.to_string(),
            false,
        ))
        .child(field(
            t(locale, "configuration.history.previous").to_string(),
            audit.previous_revision.to_string(),
            false,
        ))
        .child(field(
            t(locale, "configuration.history.actor").to_string(),
            audit.actor.clone(),
            false,
        ))
        .child(field(
            t(locale, "configuration.history.source").to_string(),
            audit.source.clone(),
            false,
        ))
        .child(field(
            t(locale, "configuration.history.transaction").to_string(),
            audit.transaction_id.clone(),
            true,
        ))
        .child(field(
            t(locale, "configuration.history.time").to_string(),
            audit.occurred_at.clone(),
            true,
        ));
    if let Some(target) = audit.target_revision {
        details = details.child(field(
            t(locale, "configuration.history.target").to_string(),
            target.to_string(),
            false,
        ));
    }
    details
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .w_full()
                .p_2()
                .rounded_sm()
                .bg(rgba(COLOR_BG))
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(rgba(COLOR_MUTED))
                        .child(t(locale, "configuration.history.changed_paths").to_string()),
                )
                .children(if audit.changed_paths.is_empty() {
                    vec![div()
                        .text_xs()
                        .text_color(rgba(COLOR_MUTED))
                        .child(t(locale, "configuration.history.no_changes").to_string())
                        .into_any_element()]
                } else {
                    audit
                        .changed_paths
                        .iter()
                        .map(|path| {
                            div()
                                .font_family("monospace")
                                .text_xs()
                                .text_color(rgba(COLOR_TEXT))
                                .child(path.clone())
                                .into_any_element()
                        })
                        .collect()
                }),
        )
        .into_any_element()
}

fn metric_card(label: &str, value: &str, source: &str) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .min_w(px(180.0))
        .p_4()
        .rounded_lg()
        .bg(rgba(COLOR_PANEL))
        .child(
            div()
                .text_sm()
                .text_color(rgba(COLOR_MUTED))
                .child(label.to_string()),
        )
        .child(
            div()
                .text_2xl()
                .text_color(rgba(COLOR_TEXT))
                .child(value.to_string()),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgba(COLOR_MUTED))
                .child(source.to_string()),
        )
}

fn diagnostic_state_semantic(state: NodeDiagnosticState) -> SemanticState {
    match state {
        NodeDiagnosticState::Passed => SemanticState::Success,
        NodeDiagnosticState::Failed => SemanticState::Danger,
        NodeDiagnosticState::Unsupported => SemanticState::Unsupported,
        NodeDiagnosticState::NoPermission => SemanticState::PermissionDenied,
        NodeDiagnosticState::NotTested => SemanticState::Neutral,
    }
}

fn diagnostic_state_label_key(state: NodeDiagnosticState) -> &'static str {
    match state {
        NodeDiagnosticState::NotTested => "diagnostic.state.not_tested",
        NodeDiagnosticState::Passed => "diagnostic.state.passed",
        NodeDiagnosticState::Failed => "diagnostic.state.failed",
        NodeDiagnosticState::Unsupported => "diagnostic.state.unsupported",
        NodeDiagnosticState::NoPermission => "diagnostic.state.no_permission",
    }
}

fn diagnostic_layer_label_key(layer: NodeDiagnosticLayer) -> &'static str {
    match layer {
        NodeDiagnosticLayer::Network => "diagnostic.layer.network",
        NodeDiagnosticLayer::Dns => "diagnostic.layer.dns",
        NodeDiagnosticLayer::Tun => "diagnostic.layer.tun",
        NodeDiagnosticLayer::Coordinator => "diagnostic.layer.coordinator",
        NodeDiagnosticLayer::Relay => "diagnostic.layer.relay",
        NodeDiagnosticLayer::Exit => "diagnostic.layer.exit",
    }
}

fn exit_health_state_semantic(state: ExitHealthState) -> SemanticState {
    match state {
        ExitHealthState::Available => SemanticState::Success,
        ExitHealthState::Degraded => SemanticState::Warning,
        ExitHealthState::Unavailable => SemanticState::Danger,
        ExitHealthState::Unknown => SemanticState::Neutral,
    }
}

fn exit_health_state_label_key(state: ExitHealthState) -> &'static str {
    match state {
        ExitHealthState::Available => "diagnostic.exit_health.state.available",
        ExitHealthState::Degraded => "diagnostic.exit_health.state.degraded",
        ExitHealthState::Unavailable => "diagnostic.exit_health.state.unavailable",
        ExitHealthState::Unknown => "diagnostic.exit_health.state.unknown",
    }
}

fn exit_health_layer_label_key(layer: ExitHealthLayer) -> &'static str {
    match layer {
        ExitHealthLayer::Process => "diagnostic.exit_health.layer.process",
        ExitHealthLayer::Listening => "diagnostic.exit_health.layer.listening",
        ExitHealthLayer::Network => "diagnostic.exit_health.layer.network",
        ExitHealthLayer::Dns => "diagnostic.exit_health.layer.dns",
        ExitHealthLayer::InternetReachability => {
            "diagnostic.exit_health.layer.internet_reachability"
        }
    }
}

fn exit_health_protocol_key(protocol: ExitHealthProtocol) -> &'static str {
    match protocol {
        ExitHealthProtocol::Dns => "diagnostic.exit_health.protocol.dns",
        ExitHealthProtocol::Tcp => "diagnostic.exit_health.protocol.tcp",
        ExitHealthProtocol::Udp => "diagnostic.exit_health.protocol.udp",
        ExitHealthProtocol::Quic => "diagnostic.exit_health.protocol.quic",
        ExitHealthProtocol::Ipv4 => "diagnostic.exit_health.protocol.ipv4",
        ExitHealthProtocol::Ipv6 => "diagnostic.exit_health.protocol.ipv6",
    }
}

fn exit_health_target_label(locale: Locale, target: &ExitHealthTarget) -> String {
    match target {
        ExitHealthTarget::DnsResolver => {
            t(locale, "diagnostic.exit_health.target.dns_resolver").to_string()
        }
        ExitHealthTarget::Ipv4Internet => {
            t(locale, "diagnostic.exit_health.target.ipv4_internet").to_string()
        }
        ExitHealthTarget::Ipv6Internet => {
            t(locale, "diagnostic.exit_health.target.ipv6_internet").to_string()
        }
        ExitHealthTarget::TcpInternet => {
            t(locale, "diagnostic.exit_health.target.tcp_internet").to_string()
        }
        ExitHealthTarget::UdpInternet => {
            t(locale, "diagnostic.exit_health.target.udp_internet").to_string()
        }
        ExitHealthTarget::QuicInternet => {
            t(locale, "diagnostic.exit_health.target.quic_internet").to_string()
        }
        ExitHealthTarget::Controlled(id) => tr(
            locale,
            "diagnostic.exit_health.controlled_target",
            &[("target", id)],
        ),
    }
}

fn exit_health_source_label_key(source: ExitHealthSource) -> &'static str {
    match source {
        ExitHealthSource::LocalProcess => "diagnostic.exit_health.source.local_process",
        ExitHealthSource::LocalSocket => "diagnostic.exit_health.source.local_socket",
        ExitHealthSource::LocalNetwork => "diagnostic.exit_health.source.local_network",
        ExitHealthSource::LocalDns => "diagnostic.exit_health.source.local_dns",
        ExitHealthSource::ControlledProbe => "diagnostic.exit_health.source.controlled_probe",
        ExitHealthSource::CoordinatorHealth => "diagnostic.exit_health.source.coordinator_health",
        ExitHealthSource::RemotePeerReport => "diagnostic.exit_health.source.remote_peer_report",
    }
}

fn exit_health_evidence_state_label_key(state: ExitHealthEvidenceState) -> &'static str {
    match state {
        ExitHealthEvidenceState::Passed => "diagnostic.exit_health.evidence_state.passed",
        ExitHealthEvidenceState::Degraded => "diagnostic.exit_health.evidence_state.degraded",
        ExitHealthEvidenceState::Failed => "diagnostic.exit_health.evidence_state.failed",
        ExitHealthEvidenceState::Unsupported => "diagnostic.exit_health.evidence_state.unsupported",
        ExitHealthEvidenceState::NotTested => "diagnostic.exit_health.evidence_state.not_tested",
        ExitHealthEvidenceState::Unknown => "diagnostic.exit_health.evidence_state.unknown",
    }
}

fn leak_detection_state_semantic(state: LeakDetectionState) -> SemanticState {
    match state {
        LeakDetectionState::LeakFree => SemanticState::Success,
        LeakDetectionState::Isolated | LeakDetectionState::InsufficientEvidence => {
            SemanticState::Warning
        }
        LeakDetectionState::LeakDetected => SemanticState::Danger,
    }
}

fn leak_detection_state_label_key(state: LeakDetectionState) -> &'static str {
    match state {
        LeakDetectionState::LeakFree => "diagnostic.leak_detection.state.leak_free",
        LeakDetectionState::Isolated => "diagnostic.leak_detection.state.isolated",
        LeakDetectionState::LeakDetected => "diagnostic.leak_detection.state.leak_detected",
        LeakDetectionState::InsufficientEvidence => {
            "diagnostic.leak_detection.state.insufficient_evidence"
        }
    }
}

fn leak_detection_protocol_label_key(protocol: LeakDetectionProtocol) -> &'static str {
    match protocol {
        LeakDetectionProtocol::Dns => "diagnostic.leak_detection.protocol.dns",
        LeakDetectionProtocol::Ipv4 => "diagnostic.leak_detection.protocol.ipv4",
        LeakDetectionProtocol::Ipv6 => "diagnostic.leak_detection.protocol.ipv6",
    }
}

fn leak_detection_target_label_key(target: LeakDetectionTarget) -> &'static str {
    match target {
        LeakDetectionTarget::DnsResolver => "diagnostic.leak_detection.target.dns_resolver",
        LeakDetectionTarget::BrowserApplicationDns => {
            "diagnostic.leak_detection.target.browser_application_dns"
        }
        LeakDetectionTarget::Ipv4Carrier => "diagnostic.leak_detection.target.ipv4_carrier",
        LeakDetectionTarget::Ipv4SourceAddress => {
            "diagnostic.leak_detection.target.ipv4_source_address"
        }
        LeakDetectionTarget::Ipv4DefaultRoute => {
            "diagnostic.leak_detection.target.ipv4_default_route"
        }
        LeakDetectionTarget::Ipv4ControlledLeakService => {
            "diagnostic.leak_detection.target.ipv4_controlled_leak_service"
        }
        LeakDetectionTarget::Ipv4ExternalTarget => {
            "diagnostic.leak_detection.target.ipv4_external_target"
        }
        LeakDetectionTarget::Ipv6Carrier => "diagnostic.leak_detection.target.ipv6_carrier",
        LeakDetectionTarget::Ipv6SourceAddress => {
            "diagnostic.leak_detection.target.ipv6_source_address"
        }
        LeakDetectionTarget::Ipv6DefaultRoute => {
            "diagnostic.leak_detection.target.ipv6_default_route"
        }
        LeakDetectionTarget::Ipv6ControlledLeakService => {
            "diagnostic.leak_detection.target.ipv6_controlled_leak_service"
        }
        LeakDetectionTarget::Ipv6ExternalTarget => {
            "diagnostic.leak_detection.target.ipv6_external_target"
        }
    }
}

fn leak_detection_source_label_key(source: LeakDetectionSource) -> &'static str {
    match source {
        LeakDetectionSource::SystemResolver => "diagnostic.leak_detection.source.system_resolver",
        LeakDetectionSource::ManagedDoh => "diagnostic.leak_detection.source.managed_doh",
        LeakDetectionSource::LocalRoutingTable => {
            "diagnostic.leak_detection.source.local_routing_table"
        }
        LeakDetectionSource::LinkCapability => "diagnostic.leak_detection.source.link_capability",
        LeakDetectionSource::ControlledProbe => "diagnostic.leak_detection.source.controlled_probe",
        LeakDetectionSource::ExternalReachabilityProbe => {
            "diagnostic.leak_detection.source.external_reachability_probe"
        }
        LeakDetectionSource::ApplicationBehaviorProbe => {
            "diagnostic.leak_detection.source.application_behavior_probe"
        }
    }
}

fn leak_detection_evidence_state_label_key(state: LeakDetectionEvidenceState) -> &'static str {
    match state {
        LeakDetectionEvidenceState::Passed => "diagnostic.leak_detection.evidence_state.passed",
        LeakDetectionEvidenceState::Failed => "diagnostic.leak_detection.evidence_state.failed",
        LeakDetectionEvidenceState::Unsupported => {
            "diagnostic.leak_detection.evidence_state.unsupported"
        }
        LeakDetectionEvidenceState::NotTested => {
            "diagnostic.leak_detection.evidence_state.not_tested"
        }
    }
}

fn leak_detection_isolation_action_label_key(action: LeakDetectionIsolationAction) -> &'static str {
    match action {
        LeakDetectionIsolationAction::BlockedUnavailableIpv4 => {
            "diagnostic.leak_detection.isolation_action.blocked_unavailable_ipv4"
        }
        LeakDetectionIsolationAction::BlockedUnavailableIpv6 => {
            "diagnostic.leak_detection.isolation_action.blocked_unavailable_ipv6"
        }
        LeakDetectionIsolationAction::FailClosedAffectedTraffic => {
            "diagnostic.leak_detection.isolation_action.fail_closed_affected_traffic"
        }
        LeakDetectionIsolationAction::RetainedProtectedRoute => {
            "diagnostic.leak_detection.isolation_action.retained_protected_route"
        }
    }
}

fn leak_detection_dns_mode_label_key(mode: LeakDetectionDnsMode) -> &'static str {
    match mode {
        LeakDetectionDnsMode::SystemResolver => {
            "diagnostic.leak_detection.dns_mode.system_resolver"
        }
        LeakDetectionDnsMode::Doh => "diagnostic.leak_detection.dns_mode.doh",
        LeakDetectionDnsMode::Dot => "diagnostic.leak_detection.dns_mode.dot",
        LeakDetectionDnsMode::NetworkSpecified => {
            "diagnostic.leak_detection.dns_mode.network_specified"
        }
        LeakDetectionDnsMode::Unmanaged => "diagnostic.leak_detection.dns_mode.unmanaged",
    }
}

fn leak_detection_platform_label_key(platform: LeakDetectionPlatform) -> &'static str {
    match platform {
        LeakDetectionPlatform::Linux => "diagnostic.leak_detection.platform.linux",
        LeakDetectionPlatform::Windows => "diagnostic.leak_detection.platform.windows",
        LeakDetectionPlatform::Macos => "diagnostic.leak_detection.platform.macos",
        LeakDetectionPlatform::Other => "diagnostic.leak_detection.platform.other",
    }
}

fn exit_health_retest_label_key(phase: &str) -> &'static str {
    match phase {
        "requested" => "diagnostic.exit_health.retest_phase.requested",
        "running" => "diagnostic.exit_health.retest_phase.running",
        "cancelled" => "diagnostic.exit_health.retest_phase.cancelled",
        "completed" => "diagnostic.exit_health.retest_phase.completed",
        "failed" => "diagnostic.exit_health.retest_phase.failed",
        _ => "diagnostic.exit_health.retest_phase.idle",
    }
}

fn diagnostic_layer_selector(layer: NodeDiagnosticLayer) -> &'static str {
    match layer {
        NodeDiagnosticLayer::Network => "visual-diagnostic-layer-network",
        NodeDiagnosticLayer::Dns => "visual-diagnostic-layer-dns",
        NodeDiagnosticLayer::Tun => "visual-diagnostic-layer-tun",
        NodeDiagnosticLayer::Coordinator => "visual-diagnostic-layer-coordinator",
        NodeDiagnosticLayer::Relay => "visual-diagnostic-layer-relay",
        NodeDiagnosticLayer::Exit => "visual-diagnostic-layer-exit",
    }
}

fn diagnostic_checks_in_fixed_order(
    diagnostics: &NodeDiagnosticsSnapshot,
) -> Vec<(NodeDiagnosticLayer, Option<&NodeDiagnosticCheck>)> {
    NodeDiagnosticLayer::ALL
        .into_iter()
        .map(|layer| {
            (
                layer,
                diagnostics.checks.iter().find(|check| check.layer == layer),
            )
        })
        .collect()
}

fn format_diagnostic_duration(locale: Locale, duration_ms: u64) -> String {
    if duration_ms < 1_000 {
        return tr(
            locale,
            "diagnostic.duration_ms_value",
            &[("duration", &format_integer(locale, duration_ms as i64))],
        );
    }
    let precision = if duration_ms.is_multiple_of(1_000) {
        0
    } else {
        1
    };
    let duration = format_decimal(locale, duration_ms as f64 / 1_000.0, precision);
    tr(
        locale,
        "diagnostic.duration_value",
        &[("duration", &duration)],
    )
}

fn format_diagnostic_timestamp(locale: Locale, unix_ms: u64) -> String {
    i64::try_from(unix_ms)
        .ok()
        .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
        .map(|timestamp| timestamp.with_timezone(&chrono::Local).fixed_offset())
        .map(|timestamp| format_timestamp(locale, &timestamp))
        .unwrap_or_else(|| t(locale, "common.not_available").to_string())
}

fn diagnostic_metadata_row(label: impl Into<SharedString>, value: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_wrap()
        .items_start()
        .gap_2()
        .text_xs()
        .line_height(px(18.0))
        .child(
            div()
                .w(px(84.0))
                .flex_shrink_0()
                .text_color(rgba(COLOR_MUTED))
                .child(label.into()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_color(rgba(COLOR_TEXT_SECONDARY))
                .child(value.into()),
        )
}

fn profile_result_section(title: impl Into<SharedString>, body: gpui::AnyElement) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .rounded_md()
        .p_3()
        .child(
            div()
                .text_xs()
                .font_weight(gpui::FontWeight(600.0))
                .text_color(rgba(COLOR_TEXT))
                .child(title.into()),
        )
        .child(body)
}

fn profile_status_chip(label: impl Into<SharedString>) -> Div {
    div()
        .rounded_full()
        .bg(rgba(COLOR_PANEL))
        .px_2()
        .py_1()
        .text_xs()
        .line_height(px(16.0))
        .text_color(rgba(COLOR_TEXT_SECONDARY))
        .child(label.into())
}

fn profile_capability_key(capability: CapabilityKind) -> &'static str {
    match capability {
        CapabilityKind::Coordinator => "capability.coordinator",
        CapabilityKind::Relay => "capability.relay",
        CapabilityKind::Exit => "capability.exit",
        CapabilityKind::File => "capability.file",
    }
}

fn member_status_key(status: NetworkMemberStatus) -> &'static str {
    match status {
        NetworkMemberStatus::Pending => "member_directory.status.pending",
        NetworkMemberStatus::Approved => "member_directory.status.approved",
        NetworkMemberStatus::Active => "member_directory.status.active",
        NetworkMemberStatus::Degraded => "member_directory.status.degraded",
        NetworkMemberStatus::Suspended => "member_directory.status.suspended",
        NetworkMemberStatus::Exited => "member_directory.status.exited",
        NetworkMemberStatus::Removed => "member_directory.status.removed",
        NetworkMemberStatus::Expired => "member_directory.status.expired",
    }
}

fn member_status_value(status: NetworkMemberStatus) -> &'static str {
    match status {
        NetworkMemberStatus::Pending => "pending",
        NetworkMemberStatus::Approved => "approved",
        NetworkMemberStatus::Active => "active",
        NetworkMemberStatus::Degraded => "degraded",
        NetworkMemberStatus::Suspended => "suspended",
        NetworkMemberStatus::Exited => "exited",
        NetworkMemberStatus::Removed => "removed",
        NetworkMemberStatus::Expired => "expired",
    }
}

fn member_presence_key(presence: NetworkMemberOnlineState) -> &'static str {
    match presence {
        NetworkMemberOnlineState::Online => "member_directory.presence.online",
        NetworkMemberOnlineState::Offline => "member_directory.presence.offline",
        NetworkMemberOnlineState::Unknown => "member_directory.presence.unknown",
    }
}

fn member_presence_value(presence: NetworkMemberOnlineState) -> &'static str {
    match presence {
        NetworkMemberOnlineState::Online => "online",
        NetworkMemberOnlineState::Offline => "offline",
        NetworkMemberOnlineState::Unknown => "unknown",
    }
}

fn member_presence_source_key(source: NetworkMemberObservationSource) -> &'static str {
    match source {
        NetworkMemberObservationSource::LiveSession => "member_directory.source.live_session",
        NetworkMemberObservationSource::DisconnectObserved => {
            "member_directory.source.disconnect_observed"
        }
        NetworkMemberObservationSource::NotObserved => "member_directory.source.not_observed",
    }
}

fn member_group_key(group: NetworkMemberGroup) -> &'static str {
    match group {
        NetworkMemberGroup::Owner => "member_directory.group.owner",
        NetworkMemberGroup::Administrator => "member_directory.group.administrator",
        NetworkMemberGroup::Member => "member_directory.group.member",
    }
}

fn member_group_value(group: NetworkMemberGroup) -> &'static str {
    match group {
        NetworkMemberGroup::Owner => "owner",
        NetworkMemberGroup::Administrator => "administrator",
        NetworkMemberGroup::Member => "member",
    }
}

fn member_display_name(member: &NetworkMember) -> String {
    member
        .display_name
        .clone()
        .unwrap_or_else(|| member.node_id.as_str().to_string())
}

fn member_groups_text(locale: Locale, member: &NetworkMember) -> String {
    if member.groups.is_empty() {
        return t(locale, "common.none").to_string();
    }
    member
        .groups
        .iter()
        .map(|group| t(locale, member_group_key(*group)))
        .collect::<Vec<_>>()
        .join(t(locale, "common.list_separator"))
}

fn member_capabilities_text(locale: Locale, member: &NetworkMember) -> String {
    let Some(capabilities) = member.capabilities.as_ref() else {
        return t(locale, "member_directory.capabilities_unknown").to_string();
    };
    if capabilities.is_empty() {
        return t(locale, "common.none").to_string();
    }
    capabilities
        .iter()
        .map(|capability| t(locale, profile_capability_key(*capability)))
        .collect::<Vec<_>>()
        .join(t(locale, "common.list_separator"))
}

fn member_joined_at_text(locale: Locale, member: &NetworkMember) -> String {
    member
        .joined_at_unix_ms
        .map(|joined_at| format_diagnostic_timestamp(locale, joined_at))
        .unwrap_or_else(|| t(locale, "member_directory.joined_at_unknown").to_string())
}

fn member_freshness_text(
    locale: Locale,
    observed_at_unix_ms: Option<u64>,
    generated_at_unix_ms: u64,
) -> Option<String> {
    let observed_at_unix_ms = observed_at_unix_ms?;
    let elapsed_seconds = generated_at_unix_ms.saturating_sub(observed_at_unix_ms) / 1_000;
    if elapsed_seconds < 60 {
        return Some(t(locale, "member_directory.freshness.just_now").to_string());
    }
    let minutes = elapsed_seconds / 60;
    if minutes < 60 {
        return Some(tr(
            locale,
            "member_directory.freshness.minutes",
            &[("count", &format_integer(locale, minutes as i64))],
        ));
    }
    let hours = minutes / 60;
    if hours < 24 {
        return Some(tr(
            locale,
            "member_directory.freshness.hours",
            &[("count", &format_integer(locale, hours as i64))],
        ));
    }
    Some(tr(
        locale,
        "member_directory.freshness.days",
        &[("count", &format_integer(locale, (hours / 24) as i64))],
    ))
}

fn tsv_cell(value: String) -> String {
    value
        .chars()
        .map(|character| match character {
            '\t' | '\r' | '\n' => ' ',
            character => character,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn export_member_directory(
    locale: Locale,
    members: &[NetworkMember],
    generated_at_unix_ms: u64,
) -> String {
    let columns = [
        "member_directory.display_name",
        "member_directory.node_id",
        "member_directory.fingerprint",
        "member_directory.role",
        "member_directory.groups",
        "member_directory.capabilities",
        "member_directory.status",
        "member_directory.presence",
        "member_directory.presence_source",
        "member_directory.observed_at",
        "member_directory.freshness",
        "member_directory.joined_at",
        "member_directory.exit.state",
        "member_directory.exit.source",
        "member_directory.exit.declared",
        "member_directory.exit.observed_at",
        "member_directory.exit.freshness",
        "member_directory.exit.reason",
    ]
    .into_iter()
    .map(|key| tsv_cell(t(locale, key).to_string()))
    .collect::<Vec<_>>();
    let rows = sort_network_members(members)
        .into_iter()
        .map(|member| {
            [
                tsv_cell(member_display_name(member)),
                tsv_cell(member.node_id.as_str().to_string()),
                tsv_cell(
                    member.identity_fingerprint.clone().unwrap_or_else(|| {
                        t(locale, "member_directory.identity_unknown").to_string()
                    }),
                ),
                tsv_cell(t(locale, join_request_role_key(member.role)).to_string()),
                tsv_cell(member_groups_text(locale, member)),
                tsv_cell(member_capabilities_text(locale, member)),
                tsv_cell(t(locale, member_status_key(member.status)).to_string()),
                tsv_cell(t(locale, member_presence_key(member.presence.state)).to_string()),
                tsv_cell(t(locale, member_presence_source_key(member.presence.source)).to_string()),
                member
                    .presence
                    .observed_at_unix_ms
                    .map(|observed| format_diagnostic_timestamp(locale, observed))
                    .unwrap_or_default(),
                tsv_cell(
                    member_freshness_text(
                        locale,
                        member.presence.observed_at_unix_ms,
                        generated_at_unix_ms,
                    )
                    .unwrap_or_default(),
                ),
                tsv_cell(member_joined_at_text(locale, member)),
                tsv_cell(member_exit_state_text(
                    locale,
                    Some(&member.exit_availability),
                    generated_at_unix_ms,
                )),
                tsv_cell(member_exit_source_text(
                    locale,
                    Some(&member.exit_availability),
                    generated_at_unix_ms,
                )),
                tsv_cell(member_exit_declared_text(
                    locale,
                    Some(&member.exit_availability),
                    generated_at_unix_ms,
                )),
                member
                    .exit_availability
                    .observed_at_unix_ms
                    .map(|observed| format_diagnostic_timestamp(locale, observed))
                    .unwrap_or_default(),
                tsv_cell(member_exit_freshness_text(
                    locale,
                    Some(&member.exit_availability),
                    generated_at_unix_ms,
                )),
                tsv_cell(member_exit_reason_text(
                    locale,
                    Some(&member.exit_availability),
                    generated_at_unix_ms,
                )),
            ]
            .join("\t")
        })
        .collect::<Vec<_>>();
    std::iter::once(columns.join("\t"))
        .chain(rows)
        .collect::<Vec<_>>()
        .join("\n")
}

fn join_request_state_key(state: JoinRequestState) -> &'static str {
    match state {
        JoinRequestState::Pending => "join_request.state.pending",
        JoinRequestState::NeedsInformation => "join_request.state.needs_information",
        JoinRequestState::Approved => "join_request.state.approved",
        JoinRequestState::Rejected => "join_request.state.rejected",
        JoinRequestState::Withdrawn => "join_request.state.withdrawn",
        JoinRequestState::Expired => "join_request.state.expired",
    }
}

fn join_request_review_action_value(action: JoinRequestReviewAction) -> &'static str {
    match action {
        JoinRequestReviewAction::Approve => "approve",
        JoinRequestReviewAction::Reject => "reject",
        JoinRequestReviewAction::RequestInformation => "request_information",
    }
}

fn join_request_role_key(role: MembershipRole) -> &'static str {
    match role {
        MembershipRole::Owner => "join_request.role.owner",
        MembershipRole::Administrator => "join_request.role.administrator",
        MembershipRole::Member => "join_request.role.member",
    }
}

fn invitation_role_name(role: MembershipRole) -> &'static str {
    match role {
        MembershipRole::Owner => "owner",
        MembershipRole::Administrator => "administrator",
        MembershipRole::Member => "member",
    }
}

fn invitation_state_key(state: InvitationState) -> &'static str {
    match state {
        InvitationState::Active => "invitations.state.active",
        InvitationState::Exhausted => "invitations.state.exhausted",
        InvitationState::Expired => "invitations.state.expired",
        InvitationState::Revoked => "invitations.state.revoked",
    }
}

fn discovery_transport_name(
    transport: fuckipv6_common::domain::DiscoveryTransport,
) -> &'static str {
    match transport {
        fuckipv6_common::domain::DiscoveryTransport::Quic => "quic",
        fuckipv6_common::domain::DiscoveryTransport::Tcp => "tcp",
        fuckipv6_common::domain::DiscoveryTransport::Websocket => "ws",
        fuckipv6_common::domain::DiscoveryTransport::Local => "local",
    }
}

fn profile_document_selector(title_key: &str) -> &'static str {
    match title_key {
        "profile.public_card" => "visual-network-profile-public-card",
        "profile.member_view" => "visual-network-profile-member-view",
        "profile.admin_view" => "visual-network-profile-admin-view",
        _ => "visual-network-profile-document",
    }
}

fn profile_warning_key(code: NetworkProfileWarningCode) -> &'static str {
    match code {
        NetworkProfileWarningCode::UnsignedLegacy => "warning.network_profile.unsigned_legacy",
        NetworkProfileWarningCode::ProfileExpired => "warning.network_profile.profile_expired",
        NetworkProfileWarningCode::RevisionConflict => "warning.network_profile.revision_conflict",
        NetworkProfileWarningCode::CapabilityNotObserved => {
            "warning.network_profile.capability_not_observed"
        }
        NetworkProfileWarningCode::CapabilityUndeclared => {
            "warning.network_profile.capability_undeclared"
        }
    }
}

fn profile_visibility_key(visibility: NetworkVisibility) -> &'static str {
    match visibility {
        NetworkVisibility::Public => "choice.visibility.public",
        NetworkVisibility::Unlisted => "choice.visibility.unlisted",
        NetworkVisibility::Private => "choice.visibility.private",
    }
}

fn profile_join_mode_key(join_mode: NetworkJoinMode) -> &'static str {
    match join_mode {
        NetworkJoinMode::Open => "choice.join_mode.open",
        NetworkJoinMode::ApprovalRequired => "choice.join_mode.approval_required",
        NetworkJoinMode::InviteOnly => "choice.join_mode.invite_only",
    }
}

fn discovery_visibility_filter_name(value: DiscoveryVisibilityFilter) -> &'static str {
    match value {
        DiscoveryVisibilityFilter::Public => "public",
        DiscoveryVisibilityFilter::Private => "private",
    }
}

fn join_mode_filter_name(value: NetworkJoinMode) -> &'static str {
    match value {
        NetworkJoinMode::Open => "open",
        NetworkJoinMode::ApprovalRequired => "approval-required",
        NetworkJoinMode::InviteOnly => "invite-only",
    }
}

fn discovery_empty_key(projection: &DiscoveryProjection) -> &'static str {
    match projection.state {
        DiscoveryProjectionState::Idle => "discovery.state.idle",
        DiscoveryProjectionState::Pending => "discovery.state.pending",
        DiscoveryProjectionState::Ready | DiscoveryProjectionState::Empty => {
            "discovery.state.empty"
        }
        DiscoveryProjectionState::NoMatches => "discovery.state.no_matches",
        DiscoveryProjectionState::Degraded => "discovery.state.degraded",
        DiscoveryProjectionState::Failed => "discovery.state.failed",
        DiscoveryProjectionState::Unsupported => "discovery.state.unsupported",
    }
}

fn discovery_notice(
    selector: &'static str,
    message: String,
    semantic: SemanticState,
) -> gpui::AnyElement {
    let palette = semantic_palette(semantic);
    div()
        .debug_selector(move || selector.into())
        .rounded_md()
        .bg(rgba(palette.background.rgba_hex()))
        .px_3()
        .py_2()
        .text_xs()
        .line_height(px(18.0))
        .text_color(rgba(palette.foreground.rgba_hex()))
        .child(message)
        .into_any_element()
}

fn activity_sync_notice(
    selector: &'static str,
    title: String,
    detail: String,
    semantic: SemanticState,
) -> gpui::AnyElement {
    let palette = semantic_palette(semantic);
    div()
        .debug_selector(move || selector.into())
        .flex()
        .flex_col()
        .gap_1()
        .rounded_md()
        .bg(rgba(palette.background.rgba_hex()))
        .px_3()
        .py_2()
        .text_xs()
        .line_height(px(18.0))
        .text_color(rgba(palette.foreground.rgba_hex()))
        .child(div().font_weight(gpui::FontWeight(600.0)).child(title))
        .child(div().text_color(rgba(COLOR_MUTED)).child(detail))
        .into_any_element()
}

fn unix_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

fn activity_filter_network_label(snapshot: &NodeSnapshot, network_id: &str) -> String {
    snapshot
        .memberships
        .iter()
        .find(|membership| membership.network_id == network_id)
        .map(|membership| membership.network_name.clone())
        .or_else(|| {
            snapshot
                .network_profiles
                .iter()
                .find(|profile| profile.network_id == network_id)
                .map(|profile| profile.name.clone())
        })
        .unwrap_or_else(|| network_id.to_string())
}

fn activity_filter_object_kind_label(locale: Locale, kind: &str) -> String {
    let key = format!("activity.object_kind.{kind}");
    let localized = t(locale, &key);
    if localized == key {
        kind.to_string()
    } else {
        localized.to_string()
    }
}

fn activity_filter_role_label(locale: Locale, role: &str) -> String {
    let key = format!("activity.role.{role}");
    let localized = t(locale, &key);
    if localized == key {
        role.to_string()
    } else {
        localized.to_string()
    }
}

const fn activity_category_key(category: ActivityCategory) -> &'static str {
    match category {
        ActivityCategory::UserActivity => "activity.category.user_activity",
        ActivityCategory::SystemEvent => "activity.category.system_event",
        ActivityCategory::SecurityEvent => "activity.category.security_event",
        ActivityCategory::OperationFailure => "activity.category.operation_failure",
    }
}

const fn activity_category_value(category: ActivityCategory) -> &'static str {
    match category {
        ActivityCategory::UserActivity => "user_activity",
        ActivityCategory::SystemEvent => "system_event",
        ActivityCategory::SecurityEvent => "security_event",
        ActivityCategory::OperationFailure => "operation_failure",
    }
}

const fn activity_severity_key(severity: ActivitySeverity) -> &'static str {
    match severity {
        ActivitySeverity::Info => "activity.severity.info",
        ActivitySeverity::Notice => "activity.severity.notice",
        ActivitySeverity::Warning => "activity.severity.warning",
        ActivitySeverity::Critical => "activity.severity.critical",
    }
}

const fn activity_severity_value(severity: ActivitySeverity) -> &'static str {
    match severity {
        ActivitySeverity::Info => "info",
        ActivitySeverity::Notice => "notice",
        ActivitySeverity::Warning => "warning",
        ActivitySeverity::Critical => "critical",
    }
}

const fn activity_outcome_key(outcome: AuditOutcome) -> &'static str {
    match outcome {
        AuditOutcome::Succeeded => "activity.outcome.succeeded",
        AuditOutcome::Failed => "activity.outcome.failed",
        AuditOutcome::Denied => "activity.outcome.denied",
    }
}

const fn activity_outcome_value(outcome: AuditOutcome) -> &'static str {
    match outcome {
        AuditOutcome::Succeeded => "succeeded",
        AuditOutcome::Failed => "failed",
        AuditOutcome::Denied => "denied",
    }
}

const fn activity_time_window_key(window: ActivityTimeWindow) -> &'static str {
    match window {
        ActivityTimeWindow::All => "activity.filter.time_all",
        ActivityTimeWindow::LastHour => "activity.filter.last_hour",
        ActivityTimeWindow::Last24Hours => "activity.filter.last_24_hours",
        ActivityTimeWindow::Last7Days => "activity.filter.last_7_days",
    }
}

const fn activity_time_window_value(window: ActivityTimeWindow) -> &'static str {
    match window {
        ActivityTimeWindow::All => "all",
        ActivityTimeWindow::LastHour => "hour",
        ActivityTimeWindow::Last24Hours => "day",
        ActivityTimeWindow::Last7Days => "week",
    }
}

fn traffic_protection_semantic(state: TrafficProtectionState) -> SemanticState {
    match state {
        TrafficProtectionState::Protected => SemanticState::Success,
        TrafficProtectionState::Connecting | TrafficProtectionState::Migrating => {
            SemanticState::Warning
        }
        TrafficProtectionState::FailClosed => SemanticState::Danger,
        TrafficProtectionState::Idle | TrafficProtectionState::ExplicitDirectFallback => {
            SemanticState::Neutral
        }
    }
}

fn primary_status_semantic(status: NodePrimaryStatus) -> SemanticState {
    match status {
        NodePrimaryStatus::Running => SemanticState::Success,
        NodePrimaryStatus::Degraded | NodePrimaryStatus::ControlUnavailable => {
            SemanticState::Warning
        }
        NodePrimaryStatus::Failed => SemanticState::Danger,
        NodePrimaryStatus::Starting | NodePrimaryStatus::Stopping => SemanticState::Info,
        NodePrimaryStatus::Uninitialized | NodePrimaryStatus::Stopped => SemanticState::Neutral,
    }
}

fn routing_target_result_navigation_selector(index: usize) -> &'static str {
    match index {
        0 => "visual-routing-target-navigation-0",
        1 => "visual-routing-target-navigation-1",
        2 => "visual-routing-target-navigation-2",
        3 => "visual-routing-target-navigation-3",
        4 => "visual-routing-target-navigation-4",
        5 => "visual-routing-target-navigation-5",
        6 => "visual-routing-target-navigation-6",
        7 => "visual-routing-target-navigation-7",
        _ => "visual-routing-target-navigation-other",
    }
}

impl GpuiShell {
    fn section_card(&self, section: &SectionModel, cx: &mut Context<Self>) -> gpui::AnyElement {
        let rows = if section.rows.is_empty() {
            vec![div()
                .text_sm()
                .text_color(rgba(COLOR_MUTED))
                .child(section.empty_text.clone())
                .into_any_element()]
        } else {
            section
                .rows
                .iter()
                .map(|row| self.row_card(row, cx))
                .collect()
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .rounded_lg()
            .bg(rgba(COLOR_PANEL))
            .child(
                div()
                    .text_base()
                    .text_color(rgba(COLOR_TEXT))
                    .child(section.title.clone()),
            )
            .children(rows)
            .into_any_element()
    }

    fn row_card(&self, row: &RowModel, cx: &mut Context<Self>) -> gpui::AnyElement {
        self.row_card_with_navigation_selector(row, None, cx)
    }

    fn row_card_with_navigation_selector(
        &self,
        row: &RowModel,
        navigation_selector: Option<&'static str>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let mut card = div()
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .rounded_md()
            .bg(rgba(COLOR_PANEL_RAISED))
            .child(
                div()
                    .text_sm()
                    .text_color(rgba(COLOR_TEXT))
                    .child(row.title.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgba(COLOR_MUTED))
                    .child(row.detail.clone()),
            );

        if let Some(navigation) = row.navigation.clone() {
            let identity = navigation
                .object_id
                .as_deref()
                .or(navigation.network_name.as_deref())
                .unwrap_or(navigation.page_id.as_str())
                .to_string();
            let selector = navigation_selector.map(str::to_owned).unwrap_or_else(|| {
                format!("visual-row-navigation-{}-{identity}", navigation.page_id)
            });
            card = card.child(
                button(
                    SharedString::from(format!("row-navigation-{}-{identity}", navigation.page_id)),
                    navigation.label.clone(),
                    ButtonVariant::Secondary,
                    false,
                )
                .debug_selector(move || selector)
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.navigate_to_row(&navigation, cx);
                })),
            );
        }

        card.into_any_element()
    }
}

fn manifest_action_label(locale: Locale, action: &NodeUiAction) -> String {
    t(locale, &action.label_key).to_string()
}

fn manifest_action_cta_label(
    locale: Locale,
    action: &NodeUiAction,
    confirmation_pending: bool,
) -> String {
    let action_label = manifest_action_label(locale, action);
    if confirmation_pending {
        tr(
            locale,
            "common.confirm_action_again",
            &[("action", &action_label)],
        )
    } else {
        action_label
    }
}

fn manifest_action_submit_disabled(
    action: &NodeUiAction,
    pending_commands: u32,
    blocked: bool,
) -> bool {
    pending_commands > 0 || blocked || action.backend_status == NodeUiBackendStatus::Unavailable
}

fn action_label(locale: Locale, id: &str) -> String {
    let key = match id {
        "refresh_snapshot" => "action.refresh_snapshot",
        "get_node_diagnostics" => "action.get_node_diagnostics",
        "create_network" => "action.create_network",
        "preview_network_profile" => "action.preview_network_profile",
        "update_network_profile" => "action.update_network_profile",
        "publish_network_discovery" => "action.publish_network_discovery",
        "update_network_discovery" => "action.update_network_discovery",
        "unpublish_network_discovery" => "action.unpublish_network_discovery",
        "refresh_discovery_index" => "action.refresh_discovery_index",
        "retry_discovery_index" => "action.retry_discovery_index",
        "preview_join_network" => "action.preview_join_network",
        "join_network" => "action.join_network",
        "submit_join_request" => "action.submit_join_request",
        "list_join_requests" => "action.list_join_requests",
        "list_network_members" => "action.list_network_members",
        "preview_member_removal" => "action.preview_member_removal",
        "change_member_lifecycle" => "action.change_member_lifecycle",
        "review_join_request" => "action.review_join_request",
        "batch_review_join_requests" => "action.batch_review_join_requests",
        "resubmit_join_request" => "action.resubmit_join_request",
        "create_invitation" => "action.create_invitation",
        "list_invitations" => "action.list_invitations",
        "get_invitation_status" => "action.get_invitation_status",
        "revoke_invitation" => "action.revoke_invitation",
        "preview_invitation" => "action.preview_invitation",
        "redeem_invitation" => "action.redeem_invitation",
        "withdraw_join_request" => "action.withdraw_join_request",
        "leave_network" => "action.leave_network",
        "acknowledge_membership_policy" => "action.acknowledge_membership_policy",
        "start_networks" => "action.start_networks",
        "stop_networks" => "action.stop_networks",
        "restart_networks" => "action.restart_networks",
        "rebuild_tun" => "action.rebuild_tun",
        "cleanup_residuals" => "action.cleanup_residuals",
        "probe_peer" => "action.probe_peer",
        "get_blacklist" => "action.get_blacklist",
        "ban_client" => "action.ban_client",
        "unban_client" => "action.unban_client",
        "get_network_allocation" => "action.get_network_allocation",
        "update_network_allocation" => "action.update_network_allocation",
        "update_routing" => "action.update_routing",
        "set_exit_node_role" => "action.set_exit_node_role",
        "set_local_exit_capability" => "action.set_local_exit_capability",
        "set_relay_role" => "action.set_relay_role",
        "set_node_display_name" => "action.set_node_display_name",
        "rotate_node_identity" => "action.rotate_node_identity",
        "start_node" => "action.start_node",
        "stop_node" => "action.stop_node",
        "restart_node" => "action.restart_node",
        "cleanup_node_residuals" => "action.cleanup_node_residuals",
        "get_configuration" => "action.get_configuration",
        "preview_configuration" => "action.preview_configuration",
        "apply_configuration" => "action.apply_configuration",
        "rollback_configuration" => "action.rollback_configuration",
        "get_configuration_history" => "action.get_configuration_history",
        "get_policy" => "action.get_policy",
        "preview_policy" => "action.preview_policy",
        "preview_policy_rollback" => "action.preview_policy_rollback",
        "apply_policy" => "action.apply_policy",
        "rollback_policy" => "action.rollback_policy",
        "get_policy_history" => "action.get_policy_history",
        "test_policy" => "action.test_policy",
        "get_traffic_policy" => "action.get_traffic_policy",
        "preview_traffic_policy" => "action.preview_traffic_policy",
        "preview_traffic_policy_rollback" => "action.preview_traffic_policy_rollback",
        "apply_traffic_policy" => "action.apply_traffic_policy",
        "rollback_traffic_policy" => "action.rollback_traffic_policy",
        "get_traffic_policy_history" => "action.get_traffic_policy_history",
        "test_traffic_policy" => "action.test_traffic_policy",
        "get_coordinator_candidates" => "action.get_coordinator_candidates",
        "update_coordinator_candidates" => "action.update_coordinator_candidates",
        "publish_coordinator_candidate_declaration" => {
            "action.publish_coordinator_candidate_declaration"
        }
        "shutdown_node" => "action.shutdown_node",
        _ => "common.unknown_action",
    };
    t(locale, key).to_string()
}

fn field_label(locale: Locale, key: &str) -> String {
    if key.starts_with("field.") {
        t(locale, key).to_string()
    } else {
        t(locale, "common.input_field").to_string()
    }
}

fn placeholder_for(
    locale: Locale,
    input: &NodeUiInputKind,
    action_id: &str,
    field_id: &str,
) -> String {
    match input {
        NodeUiInputKind::NetworkName => "home".into(),
        NodeUiInputKind::ExternalHost => "vpn.example.com".into(),
        NodeUiInputKind::UrlOrTicket => t(locale, "placeholder.url_or_ticket").into(),
        NodeUiInputKind::DiscoveryIndexUrl => t(locale, "placeholder.discovery_index_url").into(),
        NodeUiInputKind::PeerTarget => t(locale, "placeholder.peer_target").into(),
        NodeUiInputKind::Port => "1-65535".into(),
        NodeUiInputKind::TimeoutMs => "1500".into(),
        NodeUiInputKind::Revision => t(locale, "placeholder.revision").into(),
        NodeUiInputKind::ConfigurationJson => t(locale, "placeholder.configuration_json").into(),
        NodeUiInputKind::PolicyJson => t(locale, "placeholder.policy_json").into(),
        NodeUiInputKind::TrafficPolicyJson => t(locale, "placeholder.policy_json").into(),
        NodeUiInputKind::TrafficPolicyTestJson => t(locale, "placeholder.policy_json").into(),
        NodeUiInputKind::ExitSelectionPolicyJson => {
            t(locale, "placeholder.exit_selection_policy_json").into()
        }
        NodeUiInputKind::Fingerprint => t(locale, "placeholder.fingerprint").into(),
        NodeUiInputKind::Purpose => t(locale, "placeholder.network_purpose").into(),
        NodeUiInputKind::Icon => "network".into(),
        NodeUiInputKind::Language => "zh-CN".into(),
        NodeUiInputKind::JoinRequestId => t(locale, "placeholder.join_request_id").into(),
        NodeUiInputKind::JoinRequestReason => t(locale, "placeholder.join_request_reason").into(),
        NodeUiInputKind::JoinRequestNote => t(locale, "placeholder.join_request_note").into(),
        NodeUiInputKind::JoinRequestReviews => t(locale, "placeholder.join_request_reviews").into(),
        _ if matches!(
            action_id,
            "stop_networks" | "restart_networks" | "cleanup_node_residuals"
        ) && field_id == "networks" =>
        {
            t(locale, "placeholder.network_names").into()
        }
        _ => t(locale, "common.input_value").into(),
    }
}

fn bind_shell_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-r", RefreshSnapshot, None),
        KeyBinding::new("cmd-r", RefreshSnapshot, None),
        KeyBinding::new("ctrl-k", FocusSearch, None),
        KeyBinding::new("cmd-k", FocusSearch, None),
        KeyBinding::new("ctrl-q", Quit, None),
        KeyBinding::new("cmd-q", Quit, None),
    ]);
}

fn new_idempotency_key() -> IdempotencyKey {
    let mut random = [0_u8; 32];
    if getrandom::fill(&mut random).is_err() {
        let fallback = format!(
            "gpui-operation-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        );
        return IdempotencyKey::from_public_key(fallback.as_bytes());
    }
    IdempotencyKey::from_public_key(&random)
}

fn operation_state_is_terminal(state: OperationState) -> bool {
    matches!(
        state,
        OperationState::Succeeded
            | OperationState::Failed
            | OperationState::Cancelled
            | OperationState::TimedOut
    )
}

fn operation_releases_idempotency_key(state: OperationState) -> bool {
    state == OperationState::Succeeded
}

fn operation_state_key(state: OperationState) -> &'static str {
    match state {
        OperationState::Queued => "operation.state.queued",
        OperationState::Running => "operation.state.running",
        OperationState::WaitingForReauth => "operation.state.waiting_for_reauth",
        OperationState::Succeeded => "operation.state.succeeded",
        OperationState::Failed => "operation.state.failed",
        OperationState::Cancelled => "operation.state.cancelled",
        OperationState::TimedOut => "operation.state.timed_out",
    }
}

fn format_operation_error(locale: Locale, error: &OperationError) -> String {
    let params = error
        .params
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    let mut message = tr(locale, &error.message_key, &params);
    if let Some(detail) = error
        .params
        .get("detail")
        .map(String::as_str)
        .map(str::trim)
    {
        if !detail.is_empty() && !message.contains(detail) {
            message.push_str(" · ");
            message.push_str(detail);
        }
    }
    if let Some(recovery) = &error.recovery {
        message.push_str(" · ");
        message.push_str(t(locale, &recovery.hint_key));
    }
    message
}

fn format_operation_problem(locale: Locale, problem: &NodeProblem) -> String {
    let params = problem
        .params
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    let mut message = tr(locale, &problem.message_key, &params);
    if let Some(recovery) = &problem.recovery {
        message.push_str(" · ");
        message.push_str(t(locale, &recovery.hint_key));
    }
    message
}

fn set_shell_menus(cx: &mut App, locale: Locale) {
    cx.set_menus([Menu::new("fuckipv6").items([
        MenuItem::action(t(locale, "common.refresh"), RefreshSnapshot),
        MenuItem::separator(),
        MenuItem::action(t(locale, "common.quit"), Quit),
    ])]);
}

pub fn run_window(
    state: NodeStateStore,
    commands: NodeCommandBus,
    operations: Option<RemoteOperationClient>,
    configs: ConfigManager,
) -> Result<()> {
    let open_error = Arc::new(Mutex::new(None::<String>));
    let error_slot = open_error.clone();
    gpui_platform::application().run(move |cx: &mut App| {
        gpui_base::init(cx);
        super::text_input::bind_keys(cx);
        bind_shell_keys(cx);
        cx.on_action(|_: &Quit, cx: &mut App| cx.quit());
        set_shell_menus(cx, detect_system_locale());
        cx.on_window_closed(|cx, _window_id| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx);
        let result = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(window_min_size()),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some(SharedString::from("fuckipv6")),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|cx| {
                    GpuiShell::new(
                        state.clone(),
                        commands.clone(),
                        operations.clone(),
                        configs.clone(),
                        cx,
                    )
                })
            },
        );

        if let Err(error) = result {
            if let Ok(mut slot) = error_slot.lock() {
                *slot = Some(tr(
                    detect_system_locale(),
                    "gpui.window_create_failed",
                    &[("error", &error.to_string())],
                ));
            }
            cx.quit();
            return;
        }

        cx.activate(true);
    });

    let result = match open_error.lock() {
        Ok(mut error) => error.take().map_or(Ok(()), |message| Err(anyhow!(message))),
        Err(_) => Err(anyhow!(t(
            detect_system_locale(),
            "gpui.window_error_lock_poisoned"
        ))),
    };
    result
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    fn lifecycle_state_for_status(
        status: NetworkMemberStatus,
    ) -> fuckipv6_common::domain::MemberLifecycleStatus {
        match status {
            NetworkMemberStatus::Pending => {
                fuckipv6_common::domain::MemberLifecycleStatus::PendingApproval
            }
            NetworkMemberStatus::Approved => {
                fuckipv6_common::domain::MemberLifecycleStatus::Approved
            }
            NetworkMemberStatus::Active => fuckipv6_common::domain::MemberLifecycleStatus::Active,
            NetworkMemberStatus::Degraded => {
                fuckipv6_common::domain::MemberLifecycleStatus::Degraded
            }
            NetworkMemberStatus::Suspended => {
                fuckipv6_common::domain::MemberLifecycleStatus::Suspended
            }
            NetworkMemberStatus::Exited => fuckipv6_common::domain::MemberLifecycleStatus::Exited,
            NetworkMemberStatus::Removed => fuckipv6_common::domain::MemberLifecycleStatus::Removed,
            NetworkMemberStatus::Expired => fuckipv6_common::domain::MemberLifecycleStatus::Expired,
        }
    }

    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::discovery_index::{
        NodeDiscoveryIndexError, NodeDiscoveryIndexErrorCategory, NodeDiscoveryIndexRecordSnapshot,
        NodeDiscoveryIndexSnapshot, NodeDiscoveryIndexSource, NodeDiscoveryIndexState,
    };
    use crate::gpui::view_model::{ActionBlockReason, SearchResultKind};
    use crate::network_discovery::DISCOVERY_RESULT_STALE_AFTER_MS;
    use crate::node::{
        NodeCommandFuture, NodeCommandHandler, NodeCommandRequest, NodeCommandResult, NodeEventBus,
        NodeNetworkMembershipSnapshot, NodePermissionSnapshot, NodeUiValidation,
    };
    use crate::tun::TunCapabilityReason;
    use fuckipv6_common::crypto::Identity;
    use fuckipv6_common::domain::{
        DiscoveryTransport, ExitAvailabilitySource, ExitContributionAvailability,
        ExitHealthAssessment, ExitHealthEvidence, ExitHealthRetest, ExitHealthRetestPhase,
        JoinApplicantProfile, JoinRequest, JoinRequestGrantPreview, JoinRequestId, MembershipState,
        MembershipTerms, NetworkDiscoveryDocument, NetworkDiscoveryRecord, NetworkEndpoint,
        NetworkId, NetworkMemberPresence, NodeId, Signature, TopologyBoundary, TopologyConnection,
        TopologyNode, TopologyNodeHealth, TopologyNodeNetwork, TrafficAction,
        TrafficDefaultConfirmedBy, TrafficDefaultPolicy, TrafficFailurePolicy, TrafficPolicy,
        TrafficRoutingHardConstraints, TrafficRoutingPageModel, TrafficRoutingTarget,
        TrafficRoutingWorkflowState, TrafficRule,
    };
    use gpui::{Modifiers, OwnedMenuItem, TestAppContext, VisualTestContext};
    use serde_json::{json, Map, Value};

    #[test]
    fn operation_idempotency_key_is_released_only_after_success() {
        for state in [
            OperationState::Queued,
            OperationState::Running,
            OperationState::WaitingForReauth,
            OperationState::Failed,
            OperationState::Cancelled,
            OperationState::TimedOut,
        ] {
            assert!(
                !operation_releases_idempotency_key(state),
                "{state:?} must preserve its operation identity"
            );
        }
        assert!(operation_releases_idempotency_key(
            OperationState::Succeeded
        ));
    }

    const PRODUCT_VISUAL_SNAPSHOT_FIXTURE: &str =
        include_str!("../../../../contracts/typescript/fixtures/product-visual-snapshot.json");
    const VISUAL_GEOMETRY_SELECTORS: [&str; 22] = [
        "visual-sidebar",
        "visual-topbar",
        "visual-toolbar",
        "visual-workspace",
        "visual-content",
        "visual-page-heading",
        "visual-hero",
        "visual-hero-copy",
        "visual-networks",
        "visual-networks-heading",
        "visual-network-first",
        "visual-activity",
        "visual-activity-heading",
        "visual-activity-empty",
        "visual-shortcuts",
        "visual-shortcuts-heading",
        "visual-shortcuts-grid",
        "visual-shortcut-first",
        "visual-actions",
        "visual-action-title",
        "visual-action-first",
        "visual-network-second",
    ];

    #[derive(serde::Deserialize)]
    struct ProductVisualFixture {
        snapshot: NodeSnapshot,
    }

    const TOPOLOGY_SCENARIO_FIXTURE: &str = include_str!(
        "../../../../contracts/typescript/fixtures/topology-presentation-scenarios.json"
    );

    #[derive(serde::Deserialize)]
    struct TopologyScenarioDocument {
        schema_version: u32,
        fixture_revision: u32,
        scenarios: Vec<TopologyScenarioFixture>,
    }

    #[derive(serde::Deserialize)]
    struct TopologyScenarioFixture {
        id: String,
        title: String,
        expect: TopologyScenarioExpectation,
        topology: TopologySnapshotFixture,
    }

    #[derive(serde::Deserialize)]
    struct TopologyScenarioExpectation {
        boundary_ids: Vec<String>,
        node_ids: Vec<String>,
        shared_node_ids: Vec<String>,
        health_by_node: BTreeMap<String, TopologyNodeHealth>,
        enabled_capabilities_by_node: BTreeMap<String, Vec<CapabilityKind>>,
        connection_states: Vec<TopologyConnectionState>,
        detail_node_id: String,
        detail_contains: Vec<String>,
    }

    #[derive(serde::Deserialize)]
    struct TopologySnapshotFixture {
        revision: u64,
        viewer: String,
        generated_at_unix_ms: u64,
        truncated: bool,
        boundaries: Vec<TopologyBoundaryFixture>,
        nodes: Vec<TopologyNodeFixture>,
        connections: Vec<TopologyConnectionFixture>,
    }

    #[derive(serde::Deserialize)]
    struct TopologyBoundaryFixture {
        network_id: String,
        name: String,
        member_count: u32,
        visible_member_count: u32,
        viewer_role: Option<MembershipRole>,
        #[serde(default)]
        capabilities: Vec<CapabilityKind>,
    }

    #[derive(serde::Deserialize)]
    struct TopologyNodeFixture {
        node_id: String,
        display_name: Option<String>,
        is_self: bool,
        health: TopologyNodeHealth,
        #[serde(default)]
        capabilities: Vec<CapabilityKind>,
        networks: Vec<TopologyNodeNetworkFixture>,
    }

    #[derive(serde::Deserialize)]
    struct TopologyNodeNetworkFixture {
        network_id: String,
        health: TopologyNodeHealth,
        presence: NetworkMemberPresence,
        role: MembershipRole,
        status: NetworkMemberStatus,
        #[serde(default)]
        capabilities: Vec<CapabilityKind>,
    }

    #[derive(serde::Deserialize)]
    struct TopologyConnectionFixture {
        connection_id: String,
        from: String,
        to: String,
        #[serde(default)]
        networks: Vec<String>,
        observed_at_unix_ms: Option<u64>,
        state: TopologyConnectionState,
    }

    fn topology_scenario_document() -> TopologyScenarioDocument {
        let document: TopologyScenarioDocument = serde_json::from_str(TOPOLOGY_SCENARIO_FIXTURE)
            .expect("parse shared topology presentation fixture");
        assert_eq!(document.schema_version, 1);
        assert!(document.fixture_revision > 0);
        assert_eq!(document.scenarios.len(), 6);
        document
    }

    fn fixture_network_id(slug: &str) -> NetworkId {
        NetworkId::from_public_key(format!("topology-fixture-network:{slug}").as_bytes())
    }

    fn fixture_node_id(slug: &str) -> NodeId {
        NodeId::from_public_key(format!("topology-fixture-node:{slug}").as_bytes())
    }

    impl TopologySnapshotFixture {
        fn to_snapshot(&self) -> TopologySnapshot {
            TopologySnapshot {
                revision: self.revision,
                generated_at_unix_ms: self.generated_at_unix_ms,
                viewer: fixture_node_id(&self.viewer),
                truncated: self.truncated,
                boundaries: self
                    .boundaries
                    .iter()
                    .map(|boundary| TopologyBoundary {
                        network_id: fixture_network_id(&boundary.network_id),
                        name: boundary.name.clone(),
                        member_count: boundary.member_count,
                        visible_member_count: boundary.visible_member_count,
                        viewer_role: boundary.viewer_role,
                        capabilities: boundary.capabilities.clone(),
                    })
                    .collect(),
                nodes: self
                    .nodes
                    .iter()
                    .map(|node| TopologyNode {
                        node_id: fixture_node_id(&node.node_id),
                        display_name: node.display_name.clone(),
                        is_self: node.is_self,
                        health: node.health,
                        capabilities: node.capabilities.clone(),
                        networks: node
                            .networks
                            .iter()
                            .map(|network| TopologyNodeNetwork {
                                network_id: fixture_network_id(&network.network_id),
                                role: network.role,
                                status: network.status,
                                presence: network.presence.clone(),
                                health: network.health,
                                capabilities: network.capabilities.clone(),
                            })
                            .collect(),
                    })
                    .collect(),
                connections: self
                    .connections
                    .iter()
                    .map(|connection| TopologyConnection {
                        connection_id: fuckipv6_common::domain::ContentHash::from_bytes(
                            connection.connection_id.as_bytes(),
                        ),
                        from: fixture_node_id(&connection.from),
                        to: fixture_node_id(&connection.to),
                        networks: connection
                            .networks
                            .iter()
                            .map(|network_id| fixture_network_id(network_id))
                            .collect(),
                        state: connection.state,
                        observed_at_unix_ms: connection.observed_at_unix_ms,
                    })
                    .collect(),
                clusters: Vec::new(),
            }
        }
    }

    struct DiscoveryRecordFixture<'a> {
        name: &'a str,
        visibility: NetworkVisibility,
        join_mode: NetworkJoinMode,
        language: &'a str,
        region: &'a str,
        tags: &'a [&'a str],
        capabilities: Vec<CapabilityKind>,
        accepted_at_unix_ms: u64,
    }

    impl<'a> DiscoveryRecordFixture<'a> {
        fn visibility(mut self, visibility: NetworkVisibility) -> Self {
            self.visibility = visibility;
            self
        }

        fn join_mode(mut self, join_mode: NetworkJoinMode) -> Self {
            self.join_mode = join_mode;
            self
        }

        fn place(mut self, language: &'a str, region: &'a str) -> Self {
            self.language = language;
            self.region = region;
            self
        }

        fn tags(mut self, tags: &'a [&'a str]) -> Self {
            self.tags = tags;
            self
        }

        fn capabilities(mut self, capabilities: Vec<CapabilityKind>) -> Self {
            self.capabilities = capabilities;
            self
        }

        fn build(self) -> NodeDiscoveryIndexRecordSnapshot {
            let identity = Identity::generate().expect("discovery test identity");
            let public_key =
                fuckipv6_common::domain::PublicKey::from_bytes(identity.public_key_bytes());
            let owner = fuckipv6_common::domain::NodeId::from_public_key(self.name.as_bytes());
            let document = NetworkDiscoveryDocument {
                schema_version: fuckipv6_common::domain::NETWORK_DISCOVERY_SCHEMA_VERSION,
                network_id: NetworkId::from_public_key(public_key.as_str().as_bytes()),
                public_key,
                publisher: owner.clone(),
                name: self.name.into(),
                purpose: format!("{} purpose", self.name),
                language: self.language.into(),
                region: self.region.into(),
                tags: self.tags.iter().map(|tag| tag.to_string()).collect(),
                visibility: self.visibility,
                join_mode: self.join_mode,
                owner,
                administrators: Vec::new(),
                revision: 1,
                policy_revision: 1,
                privacy_policy_revision: 1,
                member_limit: None,
                member_count: 1,
                exit_contribution: fuckipv6_common::domain::ExitContributionObligation::NotRequired,
                exit_contribution_scope: Default::default(),
                exit_contribution_mode: Default::default(),
                privacy_changes: Vec::new(),
                issued_at_unix_ms: self.accepted_at_unix_ms.saturating_sub(1_000),
                expires_at_unix_ms: self.accepted_at_unix_ms.saturating_add(3_600_000),
                capability_summary: self.capabilities,
                endpoints: vec![NetworkEndpoint {
                    transport: DiscoveryTransport::Quic,
                    host: "relay.example".into(),
                    port: 443,
                }],
                revoked_at_unix_ms: None,
            };
            let signature = identity.sign(&document.signing_payload().expect("signing payload"));
            let record = NetworkDiscoveryRecord {
                document,
                signature: Signature::from_bytes(signature.to_bytes()),
            };
            NodeDiscoveryIndexRecordSnapshot {
                digest: record.digest().expect("discovery digest"),
                record,
                accepted_at_unix_ms: self.accepted_at_unix_ms,
                source: NodeDiscoveryIndexSource::Network,
                discoverable: true,
                invalid_reason: None,
            }
        }
    }

    fn discovery_record(name: &str, accepted_at_unix_ms: u64) -> DiscoveryRecordFixture<'_> {
        DiscoveryRecordFixture {
            name,
            visibility: NetworkVisibility::Public,
            join_mode: NetworkJoinMode::Open,
            language: "en",
            region: "US",
            tags: &[],
            capabilities: Vec::new(),
            accepted_at_unix_ms,
        }
    }

    fn join_requests_fixture(empty: bool) -> NodeJoinRequestListResult {
        let now = unix_time_ms();
        let network_id = NetworkId::from_public_key(b"gpui-join-requests-network");
        let applicant = NodeId::from_public_key(b"gpui-join-request-applicant");
        let request = JoinRequest::new(
            JoinRequestId::from_public_key(b"gpui-join-request"),
            network_id.clone(),
            JoinApplicantProfile {
                node_id: applicant,
                display_name: Some("Branch office router".into()),
                public_key_fingerprint: "SHA256:gpui-branch-office".into(),
            },
            "Connect the branch office devices to the family network.",
            MembershipTerms {
                network_policy_revision: 4,
                privacy_policy_revision: 2,
                exit_contribution: ExitContributionObligation::Required,
                exit_contribution_scope:
                    fuckipv6_common::domain::ExitContributionScope::JoiningMembers,
                exit_contribution_mode:
                    fuckipv6_common::domain::ExitContributionMode::RequiredOnJoin,
                member_limit: Some(16),
            },
            JoinRequestGrantPreview {
                role: MembershipRole::Member,
                capabilities: vec![CapabilityKind::File],
                exit_contribution: ExitContributionObligation::Required,
                exit_contribution_scope:
                    fuckipv6_common::domain::ExitContributionScope::JoiningMembers,
                exit_contribution_mode:
                    fuckipv6_common::domain::ExitContributionMode::RequiredOnJoin,
                file_access: true,
                grants_data_plane_access: true,
            },
            now.saturating_sub(60_000),
            now.saturating_add(3_600_000),
        )
        .expect("join request fixture");
        NodeJoinRequestListResult {
            network_id: network_id.to_string(),
            requests: if empty { Vec::new() } else { vec![request] },
        }
    }

    fn network_membership_fixture(
        network_name: &str,
        network_id: &str,
    ) -> NodeNetworkMembershipSnapshot {
        NodeNetworkMembershipSnapshot {
            network_name: network_name.into(),
            network_id: network_id.into(),
            node_id: "local-node".into(),
            state: MembershipState::Active,
            role: MembershipRole::Member,
            local_policy_revision: 1,
            network_policy_revision: Some(1),
            privacy_policy_revision: Some(1),
            member_limit: None,
            exit_contribution: Some(ExitContributionObligation::NotRequired),
            exit_contribution_scope: Some(
                fuckipv6_common::domain::ExitContributionScope::NotApplicable,
            ),
            exit_contribution_mode: Some(Default::default()),
            terms_hash: None,
            current_terms: None,
            current_terms_hash: None,
            policy_confirmation_required: false,
            acknowledged_at_unix_ms: None,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
            suspension_reason: None,
        }
    }

    fn directory_member(
        node_id: &str,
        display_name: &str,
        role: MembershipRole,
        group: NetworkMemberGroup,
        capabilities: Option<Vec<CapabilityKind>>,
        status: NetworkMemberStatus,
        presence: NetworkMemberPresence,
    ) -> NetworkMember {
        NetworkMember {
            node_id: NodeId::from_public_key(node_id.as_bytes()),
            display_name: Some(display_name.into()),
            identity_fingerprint: Some(format!("SHA256:{display_name}")),
            role,
            groups: vec![group],
            capabilities,
            status,
            lifecycle_state: lifecycle_state_for_status(status),
            revision: 1,
            access_epoch: 1,
            available_actions: Vec::new(),
            presence,
            exit_availability: Default::default(),
            joined_at_unix_ms: Some(1_700_000_000_000),
        }
    }

    fn member_directory_fixture() -> NetworkMemberDirectory {
        let mut members = vec![
            directory_member(
                "owner-node",
                "Owner filtered member",
                MembershipRole::Owner,
                NetworkMemberGroup::Owner,
                Some(vec![CapabilityKind::Coordinator]),
                NetworkMemberStatus::Active,
                NetworkMemberPresence::live(1_700_000_001_000),
            ),
            directory_member(
                "unknown-node",
                "Unknown capability member",
                MembershipRole::Member,
                NetworkMemberGroup::Member,
                None,
                NetworkMemberStatus::Suspended,
                NetworkMemberPresence::unknown(),
            ),
            directory_member(
                "empty-capability-node",
                "Empty capability member",
                MembershipRole::Member,
                NetworkMemberGroup::Member,
                Some(Vec::new()),
                NetworkMemberStatus::Exited,
                NetworkMemberPresence::disconnected(1_700_000_002_000),
            ),
        ];
        members[0].exit_availability = ExitContributionAvailability::observed(
            true,
            true,
            ExitAvailabilitySource::LocalRuntime,
            1_700_000_009_000,
        );
        members[2].exit_availability = ExitContributionAvailability::observed(
            true,
            false,
            ExitAvailabilitySource::RemotePeerReport,
            1_700_000_009_000,
        );
        members.extend((0..96).map(|index| {
            directory_member(
                &format!("synthetic-member-{index:03}"),
                &format!("Synthetic member {index:03}"),
                MembershipRole::Member,
                NetworkMemberGroup::Member,
                Some(vec![CapabilityKind::File]),
                NetworkMemberStatus::Active,
                NetworkMemberPresence::live(1_700_000_003_000 + index),
            )
        }));
        let first_node_id = sort_network_members(&members)[0].node_id.clone();
        let first = members
            .iter_mut()
            .find(|member| member.node_id == first_node_id)
            .expect("first sorted member");
        first.presence = NetworkMemberPresence::unknown();
        NetworkMemberDirectory {
            network_id: NetworkId::from_public_key(b"gpui-member-directory-network"),
            generated_at_unix_ms: 1_700_000_010_000,
            viewer_role: MembershipRole::Owner,
            can_manage_members: true,
            exit_selection_policy: Default::default(),
            exit_selection_policy_revision: 0,
            members,
        }
    }

    struct TopologyWidgetFixture {
        snapshot: NodeSnapshot,
        network_id: NetworkId,
        relay_id: NodeId,
        directory: NetworkMemberDirectory,
    }

    fn routing_widget_page(exit_id: NodeId) -> TrafficRoutingPageModel {
        let policy = TrafficPolicy {
            node_id: NodeId::from_public_key(b"gpui-routing-viewer"),
            revision: 7,
            default_policy: TrafficDefaultPolicy {
                action: TrafficAction::Exit(exit_id.clone()),
                confirmed_by: TrafficDefaultConfirmedBy::Owner,
                confirmed_at_unix_ms: 1_700_000_000_000,
            },
            failure_policy: TrafficFailurePolicy::fail_closed(),
            rules: Vec::<TrafficRule>::new(),
        };
        TrafficRoutingPageModel::from_policy(
            &policy,
            TrafficRoutingWorkflowState::Applied { revision: 7 },
            TrafficRoutingHardConstraints::default(),
            vec![
                TrafficRoutingTarget::direct(),
                TrafficRoutingTarget::reject(),
                TrafficRoutingTarget::exit(exit_id, "Tokyo Exit"),
            ],
            true,
        )
        .expect("GPUI routing page fixture")
    }

    fn topology_widget_fixture() -> TopologyWidgetFixture {
        let network_id = NetworkId::from_public_key(b"gpui-topology-network");
        let relay_id = NodeId::from_public_key(b"gpui-topology-relay");
        let viewer_id = NodeId::from_public_key(b"gpui-topology-viewer");
        let relay_presence = NetworkMemberPresence::live(1_700_000_010_000);
        let viewer_presence = NetworkMemberPresence::live(1_700_000_010_100);
        let topology = TopologySnapshot {
            revision: 9,
            generated_at_unix_ms: 1_700_000_010_200,
            viewer: viewer_id.clone(),
            truncated: false,
            boundaries: vec![TopologyBoundary {
                network_id: network_id.clone(),
                name: "gpui-topology".into(),
                member_count: 2,
                visible_member_count: 2,
                viewer_role: Some(MembershipRole::Owner),
                capabilities: vec![CapabilityKind::File],
            }],
            nodes: vec![
                TopologyNode {
                    node_id: relay_id.clone(),
                    display_name: Some("  Relay node  ".into()),
                    is_self: false,
                    health: TopologyNodeHealth::Degraded,
                    capabilities: vec![CapabilityKind::Exit, CapabilityKind::File],
                    networks: vec![TopologyNodeNetwork {
                        network_id: network_id.clone(),
                        role: MembershipRole::Administrator,
                        status: NetworkMemberStatus::Active,
                        presence: relay_presence.clone(),
                        health: TopologyNodeHealth::Degraded,
                        capabilities: vec![CapabilityKind::Exit, CapabilityKind::File],
                    }],
                },
                TopologyNode {
                    node_id: viewer_id.clone(),
                    display_name: Some("Viewer node".into()),
                    is_self: true,
                    health: TopologyNodeHealth::Healthy,
                    capabilities: vec![CapabilityKind::Coordinator],
                    networks: vec![TopologyNodeNetwork {
                        network_id: network_id.clone(),
                        role: MembershipRole::Owner,
                        status: NetworkMemberStatus::Active,
                        presence: viewer_presence.clone(),
                        health: TopologyNodeHealth::Healthy,
                        capabilities: vec![CapabilityKind::Coordinator],
                    }],
                },
            ],
            connections: vec![TopologyConnection {
                connection_id: fuckipv6_common::domain::ContentHash::from_bytes(
                    b"gpui-topology-connection",
                ),
                from: relay_id.clone(),
                to: viewer_id.clone(),
                networks: vec![network_id.clone()],
                state: TopologyConnectionState::Active,
                observed_at_unix_ms: Some(1_700_000_010_000),
            }],
            clusters: Vec::new(),
        };
        let directory = NetworkMemberDirectory {
            network_id: network_id.clone(),
            generated_at_unix_ms: 1_700_000_010_200,
            viewer_role: MembershipRole::Owner,
            can_manage_members: true,
            exit_selection_policy: Default::default(),
            exit_selection_policy_revision: 0,
            members: vec![
                NetworkMember {
                    node_id: relay_id.clone(),
                    display_name: Some("Relay directory member".into()),
                    identity_fingerprint: Some("SHA256:relay".into()),
                    role: MembershipRole::Administrator,
                    groups: vec![NetworkMemberGroup::Administrator],
                    capabilities: Some(vec![CapabilityKind::Exit, CapabilityKind::File]),
                    status: NetworkMemberStatus::Active,
                    lifecycle_state: lifecycle_state_for_status(NetworkMemberStatus::Active),
                    revision: 17,
                    access_epoch: 3,
                    available_actions: vec![
                        MemberLifecycleAction::Invite,
                        MemberLifecycleAction::Approve,
                        MemberLifecycleAction::Exit,
                        MemberLifecycleAction::Suspend,
                        MemberLifecycleAction::Resume,
                        MemberLifecycleAction::Remove,
                        MemberLifecycleAction::UndoRemove,
                    ],
                    presence: relay_presence,
                    exit_availability: ExitContributionAvailability::observed(
                        true,
                        true,
                        ExitAvailabilitySource::CoordinatorHealth,
                        1_700_000_009_000,
                    ),
                    joined_at_unix_ms: Some(1_700_000_000_000),
                },
                NetworkMember {
                    node_id: viewer_id.clone(),
                    display_name: Some("Viewer directory member".into()),
                    identity_fingerprint: Some("SHA256:viewer".into()),
                    role: MembershipRole::Owner,
                    groups: vec![NetworkMemberGroup::Owner],
                    capabilities: Some(vec![CapabilityKind::Coordinator]),
                    status: NetworkMemberStatus::Active,
                    lifecycle_state: lifecycle_state_for_status(NetworkMemberStatus::Active),
                    revision: 4,
                    access_epoch: 1,
                    available_actions: Vec::new(),
                    presence: viewer_presence,
                    exit_availability: Default::default(),
                    joined_at_unix_ms: Some(1_700_000_000_000),
                },
            ],
        };
        TopologyWidgetFixture {
            snapshot: NodeSnapshot {
                revision: 9,
                permissions: NodePermissionSnapshot {
                    permissions: NodePermission::ALL.to_vec(),
                    ..Default::default()
                },
                topology: Some(topology),
                ..Default::default()
            },
            network_id,
            relay_id,
            directory,
        }
    }

    fn install_topology_widget_fixture(
        shell: &mut GpuiShell,
        fixture: &TopologyWidgetFixture,
        cx: &mut Context<GpuiShell>,
    ) {
        shell.manifest = Some(crate::node::node_ui_manifest());
        shell.locale = Locale::En;
        shell.selected_page = "topology".into();
        shell.error = None;
        shell.notice = None;
        shell.apply_snapshot(fixture.snapshot.clone(), cx);
        let topology = shell
            .snapshot
            .topology
            .as_ref()
            .expect("topology fixture snapshot");
        let boundary = topology
            .boundary(&fixture.network_id)
            .expect("topology fixture boundary")
            .clone();
        shell.topology_directory_ready = Some(TopologyDirectoryReady {
            directory: fixture.directory.clone(),
            network_id: fixture.network_id.clone(),
            boundary,
            topology_revision: topology.revision,
            generation: shell.topology_directory_generation,
        });
        shell.topology_directory_error = None;
        shell.topology_directory_request = None;
        cx.notify();
    }

    fn install_shared_topology_snapshot(
        shell: &mut GpuiShell,
        topology: &TopologySnapshot,
        cx: &mut Context<GpuiShell>,
    ) {
        shell.manifest = Some(crate::node::node_ui_manifest());
        shell.locale = Locale::En;
        shell.selected_page = "topology".into();
        shell.error = None;
        shell.notice = None;
        shell.apply_snapshot(
            NodeSnapshot {
                revision: topology.revision,
                permissions: NodePermissionSnapshot {
                    permissions: NodePermission::ALL.to_vec(),
                    ..Default::default()
                },
                topology: Some(topology.clone()),
                ..Default::default()
            },
            cx,
        );
        let boundary = topology
            .boundaries
            .first()
            .expect("shared topology scenario boundary")
            .clone();
        shell.topology_directory_ready = Some(TopologyDirectoryReady {
            directory: NetworkMemberDirectory {
                network_id: boundary.network_id.clone(),
                generated_at_unix_ms: topology.generated_at_unix_ms,
                viewer_role: boundary.viewer_role.unwrap_or(MembershipRole::Member),
                can_manage_members: false,
                exit_selection_policy: Default::default(),
                exit_selection_policy_revision: 0,
                members: Vec::new(),
            },
            network_id: boundary.network_id.clone(),
            boundary,
            topology_revision: topology.revision,
            generation: shell.topology_directory_generation,
        });
        shell.topology_directory_error = None;
        shell.topology_directory_request = None;
        cx.notify();
    }

    #[gpui::test]
    fn routing_navigation_opens_the_exact_authoritative_topology_node(cx: &mut TestAppContext) {
        let fixture = topology_widget_fixture();
        let relay_id = fixture.relay_id.as_str().to_string();
        let window = open_shell_window(cx, 1200.0, 900.0);
        window
            .update(cx, |shell, _, cx| {
                install_topology_widget_fixture(shell, &fixture, cx);
                shell.navigate_to_row(
                    &RowNavigation {
                        page_id: "topology".into(),
                        network_name: Some("gpui-topology".into()),
                        object_id: Some(relay_id.clone()),
                        label: t(shell.locale, "traffic.routing.targets.jump").to_string(),
                    },
                    cx,
                );
                assert_eq!(shell.selected_page, "topology");
                assert_eq!(
                    shell.selected_topology_node_id.as_deref(),
                    Some(relay_id.as_str())
                );
            })
            .expect("navigate to authoritative topology node");
    }

    #[gpui::test]
    fn routing_target_search_renders_authoritative_matches_and_navigates(cx: &mut TestAppContext) {
        let fixture = topology_widget_fixture();
        let relay_id = fixture.relay_id.as_str().to_string();
        let page = routing_widget_page(fixture.relay_id.clone());
        let mut snapshot = fixture.snapshot.clone();
        snapshot.traffic_routing_page = Some(page);
        let window = open_shell_window(cx, 1400.0, 1000.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.locale = Locale::En;
                shell.selected_page = "routing".into();
                shell.error = None;
                shell.notice = None;
                shell.routing_target_query = "tokyo".into();
                shell.apply_snapshot(snapshot, cx);
                shell.selected_page = "routing".into();
                cx.notify();
            })
            .expect("install routing search fixture");

        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        assert!(visible
            .debug_bounds("visual-routing-target-search")
            .is_some());
        assert!(visible
            .debug_bounds("visual-routing-authority-notice")
            .is_some());
        assert!(visible
            .debug_bounds("visual-routing-backend-notice")
            .is_some());
        assert!(visible
            .debug_bounds("visual-routing-target-result-0")
            .is_some());
        assert!(visible
            .debug_bounds("visual-routing-target-result-1")
            .is_none());
        assert!(visible
            .debug_bounds("visual-routing-target-search-empty")
            .is_none());

        let navigation = visible
            .debug_bounds(routing_target_result_navigation_selector(0))
            .expect("authoritative target navigation");
        visible.simulate_click(navigation.center(), Modifiers::default());
        visible.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.selected_page, "topology");
                assert_eq!(
                    shell.selected_topology_node_id.as_deref(),
                    Some(relay_id.as_str())
                );
            })
            .expect("verify routing search navigation");

        let window = open_shell_window(cx, 1400.0, 1000.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.locale = Locale::En;
                shell.selected_page = "routing".into();
                shell.error = None;
                shell.notice = None;
                shell.routing_target_query = "not-an-authoritative-target".into();
                let mut snapshot = fixture.snapshot.clone();
                snapshot.traffic_routing_page = Some(routing_widget_page(fixture.relay_id.clone()));
                shell.apply_snapshot(snapshot, cx);
                shell.selected_page = "routing".into();
                cx.notify();
            })
            .expect("install empty routing search fixture");
        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        assert!(visible
            .debug_bounds("visual-routing-target-search-empty")
            .is_some());
        assert!(visible
            .debug_bounds("visual-routing-target-result-0")
            .is_none());
    }

    fn topology_node_selector(index: usize) -> &'static str {
        match index {
            0 => "visual-topology-node-0",
            1 => "visual-topology-node-1",
            2 => "visual-topology-node-2",
            3 => "visual-topology-node-3",
            4 => "visual-topology-node-4",
            5 => "visual-topology-node-5",
            _ => panic!("unexpected topology node index {index}"),
        }
    }

    fn topology_open_selector(index: usize) -> &'static str {
        match index {
            0 => "visual-topology-open-0",
            1 => "visual-topology-open-1",
            2 => "visual-topology-open-2",
            3 => "visual-topology-open-3",
            4 => "visual-topology-open-4",
            5 => "visual-topology-open-5",
            _ => panic!("unexpected topology node index {index}"),
        }
    }

    fn topology_connection_selector(index: usize) -> &'static str {
        match index {
            0 => "visual-topology-connection-0",
            1 => "visual-topology-connection-1",
            2 => "visual-topology-connection-2",
            3 => "visual-topology-connection-3",
            4 => "visual-topology-connection-4",
            5 => "visual-topology-connection-5",
            _ => panic!("unexpected topology connection index {index}"),
        }
    }

    #[gpui::test]
    fn topology_detail_and_menu_are_independent_actions(cx: &mut TestAppContext) {
        let fixture = topology_widget_fixture();
        let first_node_id = fixture.relay_id.as_str().to_string();
        let window = open_shell_window(cx, 1200.0, 900.0);
        window
            .update(cx, |shell, _, cx| {
                install_topology_widget_fixture(shell, &fixture, cx);
            })
            .expect("install topology fixture");

        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        assert!(visible
            .debug_bounds("visual-topology-detail-empty")
            .is_some());
        assert!(visible.debug_bounds("visual-topology-menu").is_none());

        let open = visible
            .debug_bounds("visual-topology-open-0")
            .expect("first topology node detail action");
        visible.simulate_click(open.center(), Modifiers::default());
        visible.run_until_parked();
        assert!(visible
            .debug_bounds("visual-topology-detail-empty")
            .is_none());
        assert!(visible
            .debug_bounds("visual-topology-exit-availability")
            .is_some());
        assert!(
            visible.debug_bounds("visual-topology-menu").is_none(),
            "opening node details must not also open the object menu"
        );
        window
            .update(cx, |shell, _, _| {
                assert_eq!(
                    shell.selected_topology_node_id.as_deref(),
                    Some(first_node_id.as_str())
                );
                assert_eq!(shell.topology_menu_node_id, None);
            })
            .expect("verify detail-only state");

        let outside = visible
            .debug_bounds("visual-topbar")
            .expect("topbar outside topology nodes");
        visible.simulate_mouse_move(outside.center(), None, Modifiers::default());
        visible.run_until_parked();
        let menu = visible
            .debug_bounds("visual-topology-menu-trigger-0")
            .expect("first topology node menu trigger");
        visible.simulate_mouse_move(menu.center(), None, Modifiers::default());
        visible.run_until_parked();
        let menu = visible
            .debug_bounds("visual-topology-menu-trigger-0")
            .expect("first topology node menu trigger after hover layout");
        visible.simulate_click(menu.center(), Modifiers::default());
        visible.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert_eq!(
                    shell.selected_topology_node_id.as_deref(),
                    Some(first_node_id.as_str())
                );
                assert_eq!(
                    shell.topology_menu_node_id.as_deref(),
                    Some(first_node_id.as_str())
                );
            })
            .expect("verify menu state");
        assert!(visible.debug_bounds("visual-topology-menu").is_some());
    }

    #[gpui::test]
    fn topology_hover_summary_matches_detail_and_yields_to_menu(cx: &mut TestAppContext) {
        let fixture = topology_widget_fixture();
        let relay_id = fixture.relay_id.as_str().to_string();
        let window = open_shell_window(cx, 1600.0, 900.0);
        window
            .update(cx, |shell, _, cx| {
                install_topology_widget_fixture(shell, &fixture, cx);
            })
            .expect("install topology fixture");

        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        assert!(visible.debug_bounds("visual-topology-hover").is_none());

        let node_card = visible
            .debug_bounds("visual-topology-node-0")
            .expect("first topology node card");
        visible.simulate_mouse_move(node_card.center(), None, Modifiers::default());
        visible.run_until_parked();
        for selector in [
            "visual-topology-hover",
            "visual-topology-hover-name",
            "visual-topology-hover-role",
            "visual-topology-hover-capabilities",
            "visual-topology-hover-health",
            "visual-topology-hover-hint",
        ] {
            assert!(
                visible.debug_bounds(selector).is_some(),
                "hover summary must expose {selector}"
            );
        }
        assert!(visible
            .debug_bounds("visual-topology-detail-empty")
            .is_some());
        window
            .update(cx, |shell, _, _| {
                assert_eq!(
                    shell.hovered_topology_node_id.as_deref(),
                    Some(relay_id.as_str())
                );
                assert_eq!(shell.selected_topology_node_id, None);
                assert_eq!(shell.topology_menu_node_id, None);
            })
            .expect("verify hover state");

        let outside = visible
            .debug_bounds("visual-topbar")
            .expect("topbar outside topology nodes");
        visible.simulate_mouse_move(outside.center(), None, Modifiers::default());
        visible.run_until_parked();
        assert!(visible.debug_bounds("visual-topology-hover").is_none());
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.hovered_topology_node_id, None);
            })
            .expect("verify hover dismissal");

        let node_card = visible
            .debug_bounds("visual-topology-node-0")
            .expect("first topology node card after hover dismissal");
        visible.simulate_mouse_move(node_card.center(), None, Modifiers::default());
        visible.run_until_parked();
        let menu_trigger = visible
            .debug_bounds("visual-topology-menu-trigger-0")
            .expect("first topology node menu trigger");
        visible.simulate_mouse_move(menu_trigger.center(), None, Modifiers::default());
        visible.run_until_parked();
        let menu_trigger = visible
            .debug_bounds("visual-topology-menu-trigger-0")
            .expect("first topology node menu trigger after pointer move");
        visible.simulate_click(menu_trigger.center(), Modifiers::default());
        visible.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert_eq!(
                    shell.topology_menu_node_id.as_deref(),
                    Some(relay_id.as_str())
                );
                assert_eq!(shell.hovered_topology_node_id, None);
            })
            .expect("verify menu suppresses hover");
        assert!(visible.debug_bounds("visual-topology-menu").is_some());
        assert!(visible.debug_bounds("visual-topology-hover").is_none());
    }

    #[gpui::test]
    fn topology_shared_scenarios_render_only_authoritative_objects(cx: &mut TestAppContext) {
        let document = topology_scenario_document();
        let window = open_shell_window(cx, 1200.0, 1800.0);
        let mut visible = VisualTestContext::from_window(window.into(), cx);

        for scenario in &document.scenarios {
            assert!(!scenario.title.trim().is_empty());
            let topology = scenario.topology.to_snapshot();
            let expected_boundaries = scenario
                .expect
                .boundary_ids
                .iter()
                .map(|network_id| fixture_network_id(network_id))
                .collect::<Vec<_>>();
            let expected_nodes = scenario
                .expect
                .node_ids
                .iter()
                .map(|node_id| fixture_node_id(node_id))
                .collect::<Vec<_>>();
            let expected_shared_nodes = scenario
                .expect
                .shared_node_ids
                .iter()
                .map(|node_id| fixture_node_id(node_id))
                .collect::<Vec<_>>();

            assert_eq!(
                topology
                    .boundaries
                    .iter()
                    .map(|boundary| boundary.network_id.clone())
                    .collect::<Vec<_>>(),
                expected_boundaries,
                "{} boundaries",
                scenario.id
            );
            assert_eq!(
                topology
                    .nodes
                    .iter()
                    .map(|node| node.node_id.clone())
                    .collect::<Vec<_>>(),
                expected_nodes,
                "{} nodes",
                scenario.id
            );
            assert_eq!(
                topology
                    .nodes
                    .iter()
                    .filter(|node| node.networks.len() > 1)
                    .map(|node| node.node_id.clone())
                    .collect::<Vec<_>>(),
                expected_shared_nodes,
                "{} shared nodes",
                scenario.id
            );
            assert_eq!(
                topology
                    .connections
                    .iter()
                    .map(|connection| connection.state)
                    .collect::<Vec<_>>(),
                scenario.expect.connection_states,
                "{} connections",
                scenario.id
            );
            for (node_id, expected_health) in &scenario.expect.health_by_node {
                let node = topology
                    .nodes
                    .iter()
                    .find(|node| node.node_id == fixture_node_id(node_id))
                    .unwrap_or_else(|| panic!("{} missing node {node_id}", scenario.id));
                assert_eq!(node.health, *expected_health, "{} {node_id}", scenario.id);
            }
            for (node_id, expected_capabilities) in &scenario.expect.enabled_capabilities_by_node {
                let node = topology
                    .nodes
                    .iter()
                    .find(|node| node.node_id == fixture_node_id(node_id))
                    .unwrap_or_else(|| panic!("{} missing node {node_id}", scenario.id));
                assert_eq!(
                    node.capabilities, *expected_capabilities,
                    "{} {node_id}",
                    scenario.id
                );
            }

            let presentation =
                topology_presentation(Some(&topology), TopologyPresentationSignals::default());
            assert_eq!(
                presentation.kind,
                TopologyPresentationCondition::Ready,
                "{}",
                scenario.id
            );
            assert!(presentation.can_render, "{}", scenario.id);

            let network_id = fixture_network_id(
                scenario
                    .expect
                    .boundary_ids
                    .first()
                    .expect("scenario boundary"),
            );
            let detail_node = topology
                .nodes
                .iter()
                .find(|node| node.node_id == fixture_node_id(&scenario.expect.detail_node_id))
                .unwrap_or_else(|| panic!("{} missing detail node", scenario.id));
            let detail = topology_node_detail(
                detail_node,
                Some(network_id.as_str()),
                None,
                Locale::En,
                topology.generated_at_unix_ms,
            );
            let mut detail_text = [
                detail.display_name,
                detail.node_id,
                detail.health,
                detail.role,
                detail.capabilities,
                detail.status,
                detail.presence,
                detail.networks,
            ]
            .join(" | ");
            for (slug, converted) in scenario.expect.node_ids.iter().zip(topology.nodes.iter()) {
                detail_text = detail_text.replace(converted.node_id.as_str(), slug);
            }
            for (slug, converted) in scenario
                .expect
                .boundary_ids
                .iter()
                .zip(topology.boundaries.iter())
            {
                detail_text = detail_text.replace(converted.network_id.as_str(), slug);
            }
            for expected_text in &scenario.expect.detail_contains {
                assert!(
                    detail_text.contains(expected_text),
                    "{} detail {detail_text:?} must contain {expected_text:?}",
                    scenario.id
                );
            }

            let topology_for_shell = topology.clone();
            window
                .update(cx, |shell, _, cx| {
                    install_shared_topology_snapshot(shell, &topology_for_shell, cx);
                })
                .expect("install shared topology scenario");
            visible.run_until_parked();

            let visible_nodes = topology
                .nodes
                .iter()
                .filter(|node| node.belongs_to(&network_id))
                .collect::<Vec<_>>();
            let visible_connections = topology
                .connections
                .iter()
                .filter(|connection| connection.networks.contains(&network_id))
                .collect::<Vec<_>>();
            let detail_index = visible_nodes
                .iter()
                .position(|node| node.node_id == fixture_node_id(&scenario.expect.detail_node_id))
                .expect("detail node is visible");
            for index in 0..visible_nodes.len() {
                assert!(
                    visible
                        .debug_bounds(topology_node_selector(index))
                        .is_some(),
                    "{} must render authoritative node {index}",
                    scenario.id
                );
            }
            assert!(
                visible
                    .debug_bounds(topology_node_selector(visible_nodes.len()))
                    .is_none(),
                "{} must not render extra nodes",
                scenario.id
            );
            for index in 0..visible_connections.len() {
                assert!(
                    visible
                        .debug_bounds(topology_connection_selector(index))
                        .is_some(),
                    "{} must render authoritative connection {index}",
                    scenario.id
                );
            }
            assert!(
                visible
                    .debug_bounds(topology_connection_selector(visible_connections.len()))
                    .is_none(),
                "{} must not render static or extra connections",
                scenario.id
            );

            assert!(visible
                .debug_bounds("visual-topology-detail-empty")
                .is_some());
            let detail_action = visible
                .debug_bounds(topology_open_selector(detail_index))
                .expect("detail action for authoritative node");
            visible.simulate_click(detail_action.center(), Modifiers::default());
            visible.run_until_parked();
            assert!(
                visible
                    .debug_bounds("visual-topology-detail-empty")
                    .is_none(),
                "{} must render the selected authoritative node detail",
                scenario.id
            );
            let expected_detail_node = fixture_node_id(&scenario.expect.detail_node_id)
                .as_str()
                .to_string();
            window
                .update(cx, |shell, _, _| {
                    assert_eq!(
                        shell.selected_topology_node_id.as_deref(),
                        Some(expected_detail_node.as_str())
                    );
                })
                .expect("verify shared scenario selection");
        }
    }

    fn join_preview_fixture() -> NodeNetworkJoinPreview {
        let now = unix_time_ms();
        NodeNetworkJoinPreview {
            network_id: fuckipv6_common::domain::NetworkId::from_public_key(b"gpui-join-preview")
                .to_string(),
            network_name: "atlas".into(),
            public_key: "public-key-atlas".into(),
            owner: "node-owner-atlas".into(),
            administrators: vec!["node-admin-atlas".into()],
            discovery_revision: 9,
            network_policy_revision: 7,
            privacy_policy_revision: 3,
            join_mode: NetworkJoinMode::Open,
            visibility: NetworkVisibility::Public,
            capability_summary: vec![CapabilityKind::Exit],
            endpoints: vec![fuckipv6_common::domain::NetworkEndpoint {
                transport: fuckipv6_common::domain::DiscoveryTransport::Websocket,
                host: "coordinator.example.test".into(),
                port: 443,
            }],
            member_count: 4,
            member_limit: Some(8),
            exit_contribution: ExitContributionObligation::Required,
            exit_contribution_scope: fuckipv6_common::domain::ExitContributionScope::JoiningMembers,
            exit_contribution_mode: fuckipv6_common::domain::ExitContributionMode::RequiredOnJoin,
            exit_contribution_purpose_key: "join.exit.disclosure.purpose".into(),
            exit_contribution_bandwidth_impact_key: "join.exit.disclosure.bandwidth".into(),
            exit_contribution_traffic_impact_key: "join.exit.disclosure.traffic".into(),
            exit_contribution_logging_boundary_key: "join.exit.disclosure.logging".into(),
            exit_contribution_operational_impact_key: "join.exit.disclosure.operational_impact"
                .into(),
            exit_contribution_disable_key: "join.exit.disclosure.disable".into(),
            local_exit_available: true,
            local_exit_enabled: false,
            can_join_without_local_exit: false,
            join_requires_exit_consent: true,
            privacy_changes: vec!["No member activity is published".into()],
            already_member: false,
            can_join: true,
            blocked_reason_key: None,
            terms_hash: fuckipv6_common::domain::ContentHash::from_bytes(b"atlas join terms"),
            preview_digest: "signed-preview-digest".into(),
            issued_at_unix_ms: now.saturating_sub(60_000),
            expires_at_unix_ms: now.saturating_add(3_600_000),
            locally_contributed_capabilities: vec![CapabilityKind::Exit],
            contribution_policy_key: "join.contribution.local_control".into(),
        }
    }

    fn invitation_status_fixture() -> InvitationStatusView {
        let now = unix_time_ms();
        let invitation = fuckipv6_common::domain::InvitationRecord::new(
            fuckipv6_common::domain::InvitationId::from_public_key(b"gpui-invitation"),
            NetworkId::from_public_key(b"gpui-invitation-network"),
            NodeId::from_public_key(b"gpui-invitation-owner"),
            MembershipRole::Member,
            vec![CapabilityKind::File],
            2,
            now.saturating_sub(60_000),
            now.saturating_add(3_600_000),
        )
        .expect("invitation record fixture");
        InvitationStatusView::new(invitation, now)
    }

    fn invitation_issued_fixture() -> IssuedInvitation {
        IssuedInvitation {
            status: invitation_status_fixture(),
            code: fuckipv6_common::domain::InvitationCode::parse(format!(
                "fkinv_{}",
                "A".repeat(43)
            ))
            .expect("invitation code fixture"),
        }
    }

    fn invitation_preview_fixture() -> InvitationPreview {
        let status = invitation_status_fixture();
        let now = unix_time_ms();
        let terms = MembershipTerms {
            network_policy_revision: 7,
            privacy_policy_revision: 3,
            exit_contribution: ExitContributionObligation::NotRequired,
            exit_contribution_scope: Default::default(),
            exit_contribution_mode: Default::default(),
            member_limit: Some(12),
        };
        let mut preview = InvitationPreview {
            invitation_id: status.invitation.invitation_id.clone(),
            invitation_revision: status.invitation.revision,
            network_id: status.invitation.network_id.clone(),
            network_name: "private-lab".into(),
            network_revision: 11,
            role: MembershipRole::Member,
            capabilities: vec![CapabilityKind::File],
            remaining_uses: status.remaining_uses,
            expires_at_unix_ms: status.invitation.expires_at_unix_ms,
            state: InvitationState::Active,
            terms_hash: terms.content_hash().expect("invitation terms hash"),
            terms,
            preview_digest: fuckipv6_common::domain::ContentHash::from_bytes(b"placeholder"),
            permission_changes: vec!["member:file".into()],
            creates_permanent_membership: false,
            requires_recent_reauthentication: true,
            can_accept: true,
            denial_key: None,
        };
        preview.preview_digest = preview.compute_digest().expect("invitation preview digest");
        assert!(preview.expires_at_unix_ms > now);
        preview
    }

    fn diagnostics_fixture(snapshot: &NodeSnapshot) -> NodeDiagnosticsSnapshot {
        NodeDiagnosticsSnapshot {
            diagnostic_id: "diagnostic-gpui-fixed-layers".into(),
            started_at_unix_ms: 1_700_000_000_000,
            completed_at_unix_ms: 1_700_000_001_250,
            timed_out: false,
            lifecycle: snapshot.lifecycle.clone(),
            retained_owner_networks: Vec::new(),
            cleanup_candidate_networks: Vec::new(),
            residue: crate::node::NodeRuntimeResidueSnapshot::default(),
            checks: vec![
                NodeDiagnosticCheck {
                    layer: NodeDiagnosticLayer::Exit,
                    state: NodeDiagnosticState::Failed,
                    started_at_unix_ms: 1_700_000_000_900,
                    duration_ms: 120,
                    error_code: Some("diagnostic.exit.unreachable".into()),
                    message_key: "diagnostic.message.exit_failed".into(),
                    suggestion_key: Some("diagnostic.suggestion.exit_retry".into()),
                },
                NodeDiagnosticCheck {
                    layer: NodeDiagnosticLayer::Network,
                    state: NodeDiagnosticState::Passed,
                    started_at_unix_ms: 1_700_000_000_100,
                    duration_ms: 15,
                    error_code: None,
                    message_key: "diagnostic.message.network_passed".into(),
                    suggestion_key: None,
                },
                NodeDiagnosticCheck {
                    layer: NodeDiagnosticLayer::Coordinator,
                    state: NodeDiagnosticState::Unsupported,
                    started_at_unix_ms: 1_700_000_000_500,
                    duration_ms: 30,
                    error_code: Some("diagnostic.coordinator.unsupported".into()),
                    message_key: "diagnostic.message.coordinator_unsupported".into(),
                    suggestion_key: Some("diagnostic.suggestion.self_hosted".into()),
                },
                NodeDiagnosticCheck {
                    layer: NodeDiagnosticLayer::Tun,
                    state: NodeDiagnosticState::NoPermission,
                    started_at_unix_ms: 1_700_000_000_300,
                    duration_ms: 45,
                    error_code: Some("diagnostic.tun.permission_denied".into()),
                    message_key: "diagnostic.message.tun_no_permission".into(),
                    suggestion_key: Some("diagnostic.suggestion.tun_permission".into()),
                },
                NodeDiagnosticCheck {
                    layer: NodeDiagnosticLayer::Relay,
                    state: NodeDiagnosticState::Passed,
                    started_at_unix_ms: 1_700_000_000_700,
                    duration_ms: 40,
                    error_code: None,
                    message_key: "diagnostic.message.relay_passed".into(),
                    suggestion_key: None,
                },
            ],
            exit_health: fuckipv6_common::domain::ExitHealthSnapshot {
                evidence: vec![
                    ExitHealthEvidence {
                        layer: ExitHealthLayer::Process,
                        state: ExitHealthEvidenceState::Passed,
                        source: ExitHealthSource::LocalProcess,
                        observed_at_unix_ms: 1_700_000_001_000,
                        duration_ms: 1,
                        target: None,
                        protocol: None,
                        error_code: None,
                    },
                    ExitHealthEvidence {
                        layer: ExitHealthLayer::Listening,
                        state: ExitHealthEvidenceState::Passed,
                        source: ExitHealthSource::LocalSocket,
                        observed_at_unix_ms: 1_700_000_001_010,
                        duration_ms: 2,
                        target: None,
                        protocol: None,
                        error_code: None,
                    },
                    ExitHealthEvidence {
                        layer: ExitHealthLayer::Network,
                        state: ExitHealthEvidenceState::Passed,
                        source: ExitHealthSource::LocalNetwork,
                        observed_at_unix_ms: 1_700_000_001_020,
                        duration_ms: 3,
                        target: None,
                        protocol: None,
                        error_code: None,
                    },
                    ExitHealthEvidence {
                        layer: ExitHealthLayer::Dns,
                        state: ExitHealthEvidenceState::Degraded,
                        source: ExitHealthSource::ControlledProbe,
                        observed_at_unix_ms: 1_700_000_001_030,
                        duration_ms: 250,
                        target: Some(ExitHealthTarget::DnsResolver),
                        protocol: Some(ExitHealthProtocol::Dns),
                        error_code: Some("exit.health.dns_timeout".into()),
                    },
                    ExitHealthEvidence {
                        layer: ExitHealthLayer::InternetReachability,
                        state: ExitHealthEvidenceState::Failed,
                        source: ExitHealthSource::ControlledProbe,
                        observed_at_unix_ms: 1_700_000_001_040,
                        duration_ms: 1_500,
                        target: Some(ExitHealthTarget::Ipv6Internet),
                        protocol: Some(ExitHealthProtocol::Ipv6),
                        error_code: Some("exit.health.ipv6_unreachable".into()),
                    },
                ],
                max_age_ms: 90_000,
                retest: ExitHealthRetest {
                    phase: ExitHealthRetestPhase::Completed,
                    requested_at_unix_ms: 1_700_000_000_500,
                    started_at_unix_ms: Some(1_700_000_000_600),
                    finished_at_unix_ms: Some(1_700_000_001_250),
                },
            },
            exit_health_assessment: ExitHealthAssessment {
                state: ExitHealthState::Degraded,
                default_candidate: false,
                reason_code: "exit.health.partially_available".into(),
                affected_layers: vec![ExitHealthLayer::Dns, ExitHealthLayer::InternetReachability],
                affected_targets: vec![
                    ExitHealthTarget::DnsResolver,
                    ExitHealthTarget::Ipv6Internet,
                ],
                affected_protocols: vec![ExitHealthProtocol::Dns, ExitHealthProtocol::Ipv6],
                fresh_evidence_count: 5,
                observed_at_unix_ms: Some(1_700_000_001_250),
                source: Some(ExitHealthSource::ControlledProbe),
            },
            leak_detection: LeakDetectionSnapshot {
                generated_at_unix_ms: 1_700_000_001_200,
                max_age_ms: 90_000,
                environment: LeakDetectionEnvironment {
                    platform: LeakDetectionPlatform::Linux,
                    dns_mode: LeakDetectionDnsMode::Doh,
                    protected_path_active: true,
                    ipv4_carrier_available: true,
                    ipv6_carrier_available: false,
                },
                evidence: vec![
                    LeakDetectionEvidence {
                        target: LeakDetectionTarget::DnsResolver,
                        state: LeakDetectionEvidenceState::Passed,
                        source: LeakDetectionSource::ManagedDoh,
                        observed_at_unix_ms: 1_700_000_001_100,
                        duration_ms: 2,
                        error_code: None,
                        isolation_action: None,
                    },
                    LeakDetectionEvidence {
                        target: LeakDetectionTarget::BrowserApplicationDns,
                        state: LeakDetectionEvidenceState::NotTested,
                        source: LeakDetectionSource::ApplicationBehaviorProbe,
                        observed_at_unix_ms: 1_700_000_001_120,
                        duration_ms: 0,
                        error_code: Some("leak_detection.probe_not_configured".into()),
                        isolation_action: None,
                    },
                    LeakDetectionEvidence {
                        target: LeakDetectionTarget::Ipv4DefaultRoute,
                        state: LeakDetectionEvidenceState::Failed,
                        source: LeakDetectionSource::LocalRoutingTable,
                        observed_at_unix_ms: 1_700_000_001_140,
                        duration_ms: 3,
                        error_code: Some(
                            "leak_detection.ipv4_default_route_outside_protected_path".into(),
                        ),
                        isolation_action: None,
                    },
                    LeakDetectionEvidence {
                        target: LeakDetectionTarget::Ipv6Carrier,
                        state: LeakDetectionEvidenceState::Unsupported,
                        source: LeakDetectionSource::LinkCapability,
                        observed_at_unix_ms: 1_700_000_001_160,
                        duration_ms: 1,
                        error_code: Some("leak_detection.ipv6_carrier_unavailable".into()),
                        isolation_action: Some(
                            LeakDetectionIsolationAction::BlockedUnavailableIpv6,
                        ),
                    },
                ],
            },
            leak_detection_assessment: LeakDetectionAssessment {
                state: LeakDetectionState::LeakDetected,
                reason_code: "leak_detection.leak_detected".into(),
                fail_closed_required: true,
                affected_protocols: vec![LeakDetectionProtocol::Ipv4, LeakDetectionProtocol::Ipv6],
                affected_targets: vec![
                    LeakDetectionTarget::Ipv4DefaultRoute,
                    LeakDetectionTarget::Ipv6Carrier,
                ],
            },
            recovery_actions: vec![crate::node::NodeDiagnosticRecoveryAction {
                action_id: "cleanup_residuals".into(),
                impact_key: "diagnostic.recovery.impact.cleanup_residuals".into(),
                confirmation_required: true,
            }],
            sanitized_report: "diagnostic-gpui-fixed-layers\nnetwork=passed\ndns=not_tested".into(),
        }
    }

    fn network_profile_fixtures() -> (
        NetworkProfilePreview,
        NetworkProfilePreview,
        NetworkProfileApplyResult,
    ) {
        use fuckipv6_common::crypto::Identity;
        use fuckipv6_common::domain::{
            preview_network_profile, NetworkCreateProfile, NetworkProfileUpdate, NetworkRecord,
            NodeId, PublicKey,
        };

        let identity = Identity::generate().expect("generate profile identity");
        let public_key = PublicKey::from_bytes(identity.public_key_bytes());
        let owner = NodeId::from_public_key(b"gpui-profile-owner");
        let mut network = NetworkRecord::create_owned(
            NetworkCreateProfile {
                name: "home-mesh".into(),
                purpose: "Family devices across two homes".into(),
                icon: "home".into(),
                language: "zh-CN".into(),
                region: "CN".into(),
                tags: vec!["family".into(), "home".into()],
                capability_declarations: vec![CapabilityKind::Relay],
                visibility: NetworkVisibility::Unlisted,
                join_mode: NetworkJoinMode::InviteOnly,
                exit_contribution: Default::default(),
                exit_contribution_scope: Default::default(),
                exit_contribution_mode: Default::default(),
                exit_selection_policy: Default::default(),
            },
            public_key,
            owner,
            100,
        )
        .expect("create profile fixture");
        network
            .sign_public_profile(&identity, 100, 10_000)
            .expect("sign profile fixture");

        let unchanged = NetworkProfileUpdate::from_record(&network);
        let empty_preview =
            preview_network_profile("home-mesh", &network, 1, &unchanged, 200).unwrap();
        let update = NetworkProfileUpdate {
            purpose: "Family devices and shared files".into(),
            region: "JP".into(),
            tags: vec!["family".into(), "tokyo".into()],
            capability_declarations: vec![CapabilityKind::Relay, CapabilityKind::File],
            ..NetworkProfileUpdate::from_record(&network)
        };
        let mut preview = preview_network_profile("home-mesh", &network, 1, &update, 200).unwrap();
        preview
            .warnings
            .push(fuckipv6_common::domain::NetworkProfileWarning::capability(
                NetworkProfileWarningCode::CapabilityNotObserved,
                CapabilityKind::File,
                "File capability is declared but not currently observed",
            ));

        let changed_paths = network
            .apply_profile_update(&update, 1, 200)
            .expect("apply profile fixture")
            .into_iter()
            .map(|entry| entry.path)
            .collect::<Vec<_>>();
        network
            .sign_public_profile(&identity, 200, 10_100)
            .expect("re-sign profile fixture");
        let signed_profile = network
            .signed_profile
            .clone()
            .expect("signed updated profile");
        let projections = fuckipv6_common::domain::network_profile_projections(
            &network,
            signed_profile.document.clone(),
            Some(signed_profile.clone()),
        );
        let updated = NetworkProfileApplyResult {
            network_name: "home-mesh".into(),
            revision: network.revision,
            previous_revision: 1,
            applied_at_unix_ms: 200,
            changed_paths,
            signed_profile,
            projections,
            warnings: Vec::new(),
            audit_action: "network.profile.update".into(),
        };
        (empty_preview, preview, updated)
    }

    fn operation_fixture(state: OperationState, cancellable: bool) -> NodeOperationSnapshot {
        use fuckipv6_common::domain::{
            DomainActorRef, DomainObjectRef, IdempotencyKey as DomainIdempotencyKey, NodeId,
            ObjectKind, Operation, OperationId, OperationKind,
        };

        let node_id = NodeId::from_public_key(b"gpui-operation-owner");
        NodeOperationSnapshot {
            operation: Operation {
                operation_id: OperationId::from_public_key(b"gpui-diagnostics-operation"),
                kind: OperationKind::Diagnostics,
                actor: DomainActorRef {
                    node_id: node_id.clone(),
                    display: "gpui-test".into(),
                },
                target: DomainObjectRef {
                    kind: ObjectKind::Node,
                    id: node_id.as_str().to_owned(),
                },
                idempotency_key: DomainIdempotencyKey::from_public_key(
                    b"gpui-diagnostics-idempotency",
                ),
                state,
                stage: "probing".into(),
                created_at_unix_ms: 1_700_000_000_000,
                updated_at_unix_ms: 1_700_000_000_100,
                attempt: 1,
                max_attempts: 1,
                error: None,
                cancellable,
                cancel_blocked_reason: (!cancellable)
                    .then(|| "operation.cancel.not_supported".into()),
            },
            result: None,
        }
    }

    #[test]
    fn diagnostics_keep_all_five_user_visible_states() {
        use NodeDiagnosticState::{Failed, NoPermission, NotTested, Passed, Unsupported};

        assert_eq!(diagnostic_state_semantic(NotTested), SemanticState::Neutral);
        assert_eq!(diagnostic_state_semantic(Passed), SemanticState::Success);
        assert_eq!(diagnostic_state_semantic(Failed), SemanticState::Danger);
        assert_eq!(
            diagnostic_state_semantic(Unsupported),
            SemanticState::Unsupported
        );
        assert_eq!(
            diagnostic_state_semantic(NoPermission),
            SemanticState::PermissionDenied
        );
        assert_eq!(
            diagnostic_state_label_key(NotTested),
            "diagnostic.state.not_tested"
        );
        assert_eq!(
            diagnostic_state_label_key(Passed),
            "diagnostic.state.passed"
        );
        assert_eq!(
            diagnostic_state_label_key(Failed),
            "diagnostic.state.failed"
        );
        assert_eq!(
            diagnostic_state_label_key(Unsupported),
            "diagnostic.state.unsupported"
        );
        assert_eq!(
            diagnostic_state_label_key(NoPermission),
            "diagnostic.state.no_permission"
        );
    }

    #[test]
    fn diagnostics_are_operations_but_snapshot_reads_are_not() {
        assert!(NodeCommand::GetDiagnostics.kind().is_operation());
        assert!(!NodeCommand::GetSnapshot.kind().is_operation());
    }

    #[gpui::test]
    fn diagnostics_card_projects_six_layers_in_fixed_order(cx: &mut TestAppContext) {
        let fixture: ProductVisualFixture = serde_json::from_str(PRODUCT_VISUAL_SNAPSHOT_FIXTURE)
            .expect("parse shared product visual fixture");
        let diagnostics = diagnostics_fixture(&fixture.snapshot);
        let window = open_shell_window(cx, 1200.0, 900.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.snapshot = fixture.snapshot;
                shell.locale = Locale::En;
                shell.selected_page = "node_runtime".into();
                shell.recent_diagnostics = Some(diagnostics);
                cx.notify();
            })
            .expect("install diagnostics fixture");

        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        assert!(visible.debug_bounds("visual-runtime-diagnostics").is_some());
        for selector in [
            "visual-diagnostic-layer-network",
            "visual-diagnostic-layer-dns",
            "visual-diagnostic-layer-tun",
            "visual-diagnostic-layer-coordinator",
            "visual-diagnostic-layer-relay",
            "visual-diagnostic-layer-exit",
            "visual-diagnostic-exit-health",
            "visual-diagnostic-exit-health-process",
            "visual-diagnostic-exit-health-listening",
            "visual-diagnostic-exit-health-network",
            "visual-diagnostic-exit-health-dns",
            "visual-diagnostic-exit-health-internet_reachability",
            "visual-diagnostic-leak-detection",
            "visual-diagnostic-leak-target-dns_resolver-0",
            "visual-diagnostic-leak-target-browser_application_dns-1",
            "visual-diagnostic-leak-target-ipv4_default_route-2",
            "visual-diagnostic-leak-target-ipv6_carrier-3",
        ] {
            assert!(
                visible.debug_bounds(selector).is_some(),
                "missing fixed diagnostic element {selector}"
            );
        }
        let leak_target_offsets = [
            "visual-diagnostic-leak-target-dns_resolver-0",
            "visual-diagnostic-leak-target-browser_application_dns-1",
            "visual-diagnostic-leak-target-ipv4_default_route-2",
            "visual-diagnostic-leak-target-ipv6_carrier-3",
        ]
        .into_iter()
        .map(|selector| {
            f32::from(
                visible
                    .debug_bounds(selector)
                    .expect("leak evidence bounds")
                    .origin
                    .y,
            )
        })
        .collect::<Vec<_>>();
        assert!(
            leak_target_offsets.windows(2).all(|pair| pair[0] < pair[1]),
            "leak evidence must preserve server order: {leak_target_offsets:?}"
        );
    }

    #[gpui::test]
    fn exit_node_page_reuses_the_authoritative_diagnostics_card(cx: &mut TestAppContext) {
        let fixture: ProductVisualFixture = serde_json::from_str(PRODUCT_VISUAL_SNAPSHOT_FIXTURE)
            .expect("parse shared product visual fixture");
        let diagnostics = diagnostics_fixture(&fixture.snapshot);
        let window = open_shell_window(cx, 1200.0, 900.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.snapshot = fixture.snapshot;
                shell.locale = Locale::En;
                shell.selected_page = "exit_node".into();
                shell.recent_diagnostics = Some(diagnostics);
                cx.notify();
            })
            .expect("install Exit page diagnostics fixture");

        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        for selector in [
            "visual-runtime-diagnostics",
            "visual-diagnostic-layer-network",
            "visual-diagnostic-layer-dns",
            "visual-diagnostic-layer-tun",
            "visual-diagnostic-layer-coordinator",
            "visual-diagnostic-layer-relay",
            "visual-diagnostic-layer-exit",
            "visual-diagnostic-leak-detection",
        ] {
            assert!(
                visible.debug_bounds(selector).is_some(),
                "missing Exit page diagnostic element {selector}"
            );
        }
    }

    #[test]
    fn leak_detection_labels_preserve_non_passing_evidence_semantics() {
        assert_eq!(
            leak_detection_state_semantic(LeakDetectionState::LeakDetected),
            SemanticState::Danger
        );
        assert_eq!(
            leak_detection_state_semantic(LeakDetectionState::InsufficientEvidence),
            SemanticState::Warning
        );
        assert_eq!(
            leak_detection_evidence_state_label_key(LeakDetectionEvidenceState::Failed),
            "diagnostic.leak_detection.evidence_state.failed"
        );
        assert_eq!(
            leak_detection_evidence_state_label_key(LeakDetectionEvidenceState::Unsupported),
            "diagnostic.leak_detection.evidence_state.unsupported"
        );
        assert_eq!(
            leak_detection_evidence_state_label_key(LeakDetectionEvidenceState::NotTested),
            "diagnostic.leak_detection.evidence_state.not_tested"
        );
        assert_eq!(
            leak_detection_target_label_key(LeakDetectionTarget::BrowserApplicationDns),
            "diagnostic.leak_detection.target.browser_application_dns"
        );
        assert_eq!(
            leak_detection_isolation_action_label_key(
                LeakDetectionIsolationAction::BlockedUnavailableIpv6
            ),
            "diagnostic.leak_detection.isolation_action.blocked_unavailable_ipv6"
        );
    }

    #[gpui::test]
    fn network_profile_preview_renders_privacy_layers_diff_and_warnings(cx: &mut TestAppContext) {
        let (_, mut preview, _) = network_profile_fixtures();
        preview.conflict = true;
        preview.can_apply = false;
        preview.expected_revision = 0;
        let window = open_shell_window(cx, 1200.0, 900.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.locale = Locale::En;
                shell.selected_page = "networks".into();
                shell.recent_network_profile_preview = Some(preview);
                cx.notify();
            })
            .expect("install profile preview fixture");

        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        for selector in [
            "visual-network-profile-result",
            "visual-network-profile-preview",
            "visual-network-profile-conflict",
            "visual-network-profile-projections",
            "visual-network-profile-public-card",
            "visual-network-profile-member-view",
            "visual-network-profile-admin-view",
            "visual-network-profile-local-secret",
            "visual-network-profile-withheld",
            "visual-network-profile-diff",
            "visual-network-profile-warning-warning.network_profile.capability_not_observed",
        ] {
            assert!(
                visible.debug_bounds(selector).is_some(),
                "missing profile preview element {selector}"
            );
        }
    }

    #[gpui::test]
    fn join_request_reason_editor_explains_visibility_and_limit(cx: &mut TestAppContext) {
        let window = open_shell_window(cx, 1200.0, 900.0);
        window
            .update(cx, |shell, _, cx| {
                let manifest = crate::node::node_ui_manifest();
                let page_id = manifest
                    .pages
                    .iter()
                    .find(|page| {
                        page.actions
                            .iter()
                            .any(|action| action.id == "submit_join_request")
                    })
                    .map(|page| page.id.clone())
                    .expect("join request action page");
                shell.manifest = Some(manifest);
                shell.locale = Locale::En;
                shell.selected_page = page_id;
                shell
                    .expanded_actions
                    .insert("submit_join_request".into(), true);
                cx.notify();
            })
            .expect("install join request form");

        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        assert!(
            visible
                .debug_bounds("visual-join-request-reason-help-submit_join_request-reason")
                .is_some(),
            "join request reason editor must explain its visibility and length limit"
        );
    }

    #[gpui::test]
    fn join_request_review_list_renders_applicant_reason_and_risk_details(cx: &mut TestAppContext) {
        let window = open_shell_window(cx, 1200.0, 900.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.locale = Locale::En;
                shell.selected_page = "networks".into();
                shell
                    .expanded_actions
                    .insert("list_join_requests".into(), true);
                shell.recent_join_requests = Some(join_requests_fixture(false));
                cx.notify();
            })
            .expect("install join request list fixture");

        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        for selector in [
            "visual-join-requests-list",
            "visual-join-request-first",
            "visual-join-request-applicant",
            "visual-join-request-reason",
            "visual-join-request-risk",
            "visual-join-request-actions",
            "visual-join-request-select-0",
            "visual-join-request-review-0-approve",
            "visual-join-request-review-0-reject",
            "visual-join-request-review-0-request-information",
            "visual-join-request-selection-bar",
            "visual-join-request-select-all",
            "visual-join-request-batch-approve",
            "visual-join-request-batch-reject",
            "visual-join-request-batch-request-information",
        ] {
            assert!(
                visible.debug_bounds(selector).is_some(),
                "missing join request review element {selector}"
            );
        }
    }

    #[gpui::test]
    fn join_request_review_list_renders_an_explicit_empty_state(cx: &mut TestAppContext) {
        let window = open_shell_window(cx, 1200.0, 900.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.locale = Locale::En;
                shell.selected_page = "networks".into();
                shell
                    .expanded_actions
                    .insert("list_join_requests".into(), true);
                shell.recent_join_requests = Some(join_requests_fixture(true));
                cx.notify();
            })
            .expect("install empty join request list fixture");

        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        assert!(visible.debug_bounds("visual-join-requests-list").is_some());
        assert!(visible.debug_bounds("visual-join-requests-empty").is_some());
        assert!(visible.debug_bounds("visual-join-request-first").is_none());
    }

    #[gpui::test]
    fn member_directory_virtualizes_rows_and_opens_one_shared_detail_panel(
        cx: &mut TestAppContext,
    ) {
        let directory = member_directory_fixture();
        let first_node_id = sort_network_members(&directory.members)[0]
            .node_id
            .as_str()
            .to_string();
        let window = open_shell_window(cx, 3_200.0, 2_400.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.locale = Locale::En;
                shell.selected_page = "networks".into();
                shell.recent_network_members = Some(directory);
                shell.open_action("networks", "list_network_members", cx);
            })
            .expect("install member directory fixture");

        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        for selector in [
            "visual-member-directory",
            "visual-member-directory-count",
            "visual-member-directory-filters",
            "visual-member-directory-list",
            "visual-member-first",
            "visual-member-exit-0",
            "visual-member-directory-export",
        ] {
            assert!(
                visible.debug_bounds(selector).is_some(),
                "missing member directory element {selector}"
            );
        }
        assert!(
            visible.debug_bounds("visual-member-row-95").is_none(),
            "virtual member list rendered a row outside its viewport"
        );
        assert!(visible.debug_bounds("visual-member-detail").is_none());
        assert!(visible.debug_bounds("visual-member-detail-hint").is_some());

        let first = visible
            .debug_bounds("visual-member-first")
            .expect("first virtual member row");
        visible.simulate_click(first.center(), Modifiers::default());
        visible.run_until_parked();
        assert!(visible.debug_bounds("visual-member-detail").is_some());
        assert!(visible.debug_bounds("visual-member-presence").is_some());
        assert!(visible
            .debug_bounds("visual-member-exit-availability")
            .is_some());
        assert!(visible.debug_bounds("visual-member-exit-state").is_some());
        assert!(visible.debug_bounds("visual-member-detail-hint").is_none());
        assert_eq!(
            member_presence_key(NetworkMemberOnlineState::Unknown),
            "member_directory.presence.unknown"
        );
        assert_ne!(
            member_presence_key(NetworkMemberOnlineState::Unknown),
            member_presence_key(NetworkMemberOnlineState::Offline)
        );

        window
            .update(cx, |shell, _, _| {
                assert_eq!(
                    shell.selected_member_node_id.as_deref(),
                    Some(first_node_id.as_str())
                );
            })
            .expect("read selected member");
    }

    #[gpui::test]
    fn member_directory_exports_only_the_sanitized_filtered_view(cx: &mut TestAppContext) {
        let directory = member_directory_fixture();
        let owner = sort_network_members(&directory.members)
            .into_iter()
            .find(|member| member.role == MembershipRole::Owner)
            .expect("owner fixture")
            .clone();
        let window = open_shell_window(cx, 3_200.0, 2_400.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.locale = Locale::En;
                shell.recent_network_members = Some(directory);
                shell.member_directory_filters.role = Some(MembershipRole::Owner);
                shell.open_action("networks", "list_network_members", cx);
            })
            .expect("install filtered member directory fixture");

        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        assert!(visible
            .debug_bounds("visual-member-directory-export")
            .is_some());
        window
            .update(cx, |shell, _, cx| {
                shell.export_network_members(cx);
            })
            .expect("export filtered member directory");
        visible.run_until_parked();
        let exported = visible
            .read_from_clipboard()
            .and_then(|item| item.text())
            .expect("member directory clipboard export");

        assert_eq!(exported.lines().count(), 2);
        assert_eq!(
            exported
                .lines()
                .next()
                .expect("member export header")
                .split('\t')
                .count(),
            18
        );
        assert!(exported.contains("Owner filtered member"));
        assert!(exported.contains(owner.node_id.as_str()));
        assert!(exported.contains(t(Locale::En, "member_directory.exit.state")));
        assert!(exported.contains(t(Locale::En, "member_directory.exit.state.available")));
        assert!(exported.contains(t(Locale::En, "member_directory.exit.source.local_runtime")));
        assert!(!exported.contains("Synthetic member 001"));
        let lowercase = exported.to_ascii_lowercase();
        for forbidden in [
            "endpoint",
            "public key",
            "private key",
            "invitation",
            "approval reason",
        ] {
            assert!(
                !lowercase.contains(forbidden),
                "member export leaked forbidden field marker {forbidden}"
            );
        }
    }

    #[gpui::test]
    fn joined_network_member_action_uses_the_authoritative_network_id(cx: &mut TestAppContext) {
        let fixture: ProductVisualFixture = serde_json::from_str(PRODUCT_VISUAL_SNAPSHOT_FIXTURE)
            .expect("parse shared product visual fixture");
        let mut snapshot = fixture.snapshot;
        snapshot.memberships.push(network_membership_fixture(
            "home",
            "network-authoritative-id",
        ));
        let window = open_shell_window(cx, 1200.0, 1_400.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.snapshot = snapshot;
                shell.locale = Locale::En;
                shell.selected_page = "networks".into();
                shell.network_actions_open = Some("home".into());
                cx.notify();
            })
            .expect("install joined network member action fixture");

        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        let members = visible
            .debug_bounds("visual-discovery-joined-members-home")
            .expect("joined network member action");
        visible.simulate_click(members.center(), Modifiers::default());
        visible.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.selected_page, "networks");
                assert_eq!(shell.selected_network.as_deref(), Some("home"));
                assert_eq!(
                    shell.field_value("list_network_members", "network_id"),
                    "network-authoritative-id"
                );
                assert!(shell.expanded_actions.contains_key("list_network_members"));
            })
            .expect("verify authoritative member action");

        window
            .update(cx, |shell, _, cx| {
                shell.snapshot.memberships.clear();
                shell.network_actions_open = Some("home".into());
                cx.notify();
            })
            .expect("remove membership identity");
        visible.run_until_parked();
        assert!(
            visible
                .debug_bounds("visual-discovery-joined-members-home")
                .is_none(),
            "member action must not be offered without an authoritative network ID"
        );
    }

    #[gpui::test]
    fn join_request_review_actions_pin_revisions_and_apply_review_results(cx: &mut TestAppContext) {
        let fixture = join_requests_fixture(false);
        let request = fixture.requests[0].clone();
        let request_id = request.request_id.as_str().to_string();
        let network_id = fixture.network_id.clone();
        let revision = request.revision;
        let window = open_shell_window(cx, 1200.0, 900.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.locale = Locale::En;
                shell.selected_page = "networks".into();
                shell.recent_join_requests = Some(fixture);
                shell.join_request_selection.insert(request_id.clone());

                shell.prepare_join_request_review(
                    network_id.clone(),
                    request_id.clone(),
                    revision,
                    JoinRequestReviewAction::RequestInformation,
                    cx,
                );
                assert_eq!(
                    shell.field_value("review_join_request", "network_id"),
                    network_id
                );
                assert_eq!(
                    shell.field_value("review_join_request", "request_id"),
                    request_id
                );
                assert_eq!(
                    shell.field_value("review_join_request", "expected_revision"),
                    revision.to_string()
                );
                assert_eq!(
                    shell.field_value("review_join_request", "action"),
                    "request_information"
                );
                assert_eq!(shell.field_value("review_join_request", "note"), "");
                assert_eq!(shell.expanded_actions.len(), 1);
                assert!(shell.expanded_actions.contains_key("review_join_request"));
                assert!(!shell
                    .expanded_actions
                    .contains_key("batch_review_join_requests"));

                shell.prepare_batch_join_request_review(
                    network_id.clone(),
                    &[(request_id.clone(), revision)],
                    JoinRequestReviewAction::Approve,
                    cx,
                );
                let reviews = serde_json::from_str::<Vec<NodeJoinRequestReviewItem>>(
                    &shell.field_value("batch_review_join_requests", "reviews"),
                )
                .expect("parse internally generated batch review");
                assert_eq!(reviews.len(), 1);
                assert_eq!(reviews[0].request_id, request_id);
                assert_eq!(reviews[0].expected_revision, revision);
                assert_eq!(reviews[0].action, JoinRequestReviewAction::Approve);
                assert_eq!(reviews[0].note, None);
                assert_eq!(shell.expanded_actions.len(), 1);
                assert!(shell
                    .expanded_actions
                    .contains_key("batch_review_join_requests"));
                assert!(!shell.expanded_actions.contains_key("review_join_request"));

                shell.apply_join_request_review_ack(&NodeJoinRequestReviewAck {
                    network_id,
                    reviews: vec![crate::node::NodeJoinRequestReviewOutcome {
                        request_id: request_id.clone(),
                        state: JoinRequestState::Approved,
                        revision: revision + 1,
                    }],
                });
                let updated = &shell.recent_join_requests.as_ref().unwrap().requests[0];
                assert_eq!(updated.state, JoinRequestState::Approved);
                assert_eq!(updated.revision, revision + 1);
                assert!(!shell.join_request_selection.contains(&request_id));
            })
            .expect("exercise guided join request review actions");

        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        for selector in [
            "visual-join-request-review-summary",
            "visual-join-request-review-grant-batch_review_join_requests-0",
        ] {
            assert!(
                visible.debug_bounds(selector).is_some(),
                "missing guided join request approval detail {selector}"
            );
        }
    }

    #[gpui::test]
    fn network_profile_result_renders_empty_diff_and_warning_states(cx: &mut TestAppContext) {
        let (empty_preview, _, _) = network_profile_fixtures();
        let window = open_shell_window(cx, 1200.0, 900.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.locale = Locale::En;
                shell.selected_page = "networks".into();
                shell.recent_network_profile_preview = Some(empty_preview);
                cx.notify();
            })
            .expect("install empty profile preview fixture");

        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        assert!(visible
            .debug_bounds("visual-network-profile-no-changes")
            .is_some());
        assert!(visible
            .debug_bounds("visual-network-profile-no-warnings")
            .is_some());
        assert!(visible
            .debug_bounds(
                "visual-network-profile-warning-warning.network_profile.capability_not_observed"
            )
            .is_none());
    }

    #[gpui::test]
    fn network_profile_updated_renders_signed_audit_result(cx: &mut TestAppContext) {
        let (_, _, updated) = network_profile_fixtures();
        let window = open_shell_window(cx, 1200.0, 900.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.locale = Locale::En;
                shell.selected_page = "networks".into();
                shell.recent_network_profile_updated = Some(updated);
                cx.notify();
            })
            .expect("install applied profile fixture");

        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        for selector in [
            "visual-network-profile-result",
            "visual-network-profile-updated",
            "visual-network-profile-projections",
            "visual-network-profile-admin-view",
            "visual-network-profile-withheld",
            "visual-network-profile-changed-paths",
            "visual-network-profile-no-warnings",
        ] {
            assert!(
                visible.debug_bounds(selector).is_some(),
                "missing applied profile element {selector}"
            );
        }
        assert!(visible
            .debug_bounds("visual-network-profile-preview")
            .is_none());
    }

    #[gpui::test]
    fn networks_page_separates_joined_and_discovery_and_renders_verified_details(
        cx: &mut TestAppContext,
    ) {
        let fixture: ProductVisualFixture = serde_json::from_str(PRODUCT_VISUAL_SNAPSHOT_FIXTURE)
            .expect("parse shared product visual fixture");
        let now = unix_time_ms();
        let result = discovery_record(
            "atlas",
            now.saturating_sub(DISCOVERY_RESULT_STALE_AFTER_MS + 1),
        )
        .join_mode(NetworkJoinMode::ApprovalRequired)
        .tags(&["research"])
        .capabilities(vec![CapabilityKind::Relay])
        .build();
        let expanded_network_id = result.record.document.network_id.as_str().to_string();
        let mut snapshot = fixture.snapshot;
        snapshot.discovery_index = NodeDiscoveryIndexSnapshot {
            state: NodeDiscoveryIndexState::Ready,
            records: vec![result],
            last_success_unix_ms: Some(now.saturating_sub(2_000)),
            ..NodeDiscoveryIndexSnapshot::default()
        };
        let window = open_shell_window(cx, 1200.0, 1_200.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.snapshot = snapshot;
                shell.locale = Locale::En;
                shell.selected_page = "networks".into();
                shell.expanded_discovery.insert(expanded_network_id, true);
                cx.notify();
            })
            .expect("install discovery page fixture");

        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        for selector in [
            "visual-discovery-networks",
            "visual-discovery-joined",
            "visual-discovery-joined-network",
            "visual-discovery-joined-actions-home",
            "visual-discovery-results",
            "visual-discovery-search",
            "visual-discovery-filters",
            "visual-discovery-result",
            "visual-discovery-result-details",
            "visual-discovery-result-stale",
            "visual-discovery-cta",
            "visual-discovery-stale",
            "visual-discovery-last-success",
            "visual-discovery-no-more",
        ] {
            assert!(
                visible.debug_bounds(selector).is_some(),
                "missing discovery element {selector}"
            );
        }
        assert!(visible
            .debug_bounds("visual-discovery-joined-empty")
            .is_none());
        assert!(visible
            .debug_bounds("visual-discovery-empty-results")
            .is_none());
    }

    #[gpui::test]
    fn joined_network_actions_are_keyboard_reachable_and_route_in_network_context(
        cx: &mut TestAppContext,
    ) {
        let fixture: ProductVisualFixture = serde_json::from_str(PRODUCT_VISUAL_SNAPSHOT_FIXTURE)
            .expect("parse shared product visual fixture");
        let window = open_shell_window(cx, 1200.0, 900.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.snapshot = fixture.snapshot;
                shell.locale = Locale::En;
                shell.selected_page = "networks".into();
                shell.selected_network = Some("studio".into());
                shell.network_actions_open = Some("home".into());
                cx.notify();
            })
            .expect("install joined network actions fixture");

        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        for selector in [
            "visual-discovery-joined-actions-home",
            "visual-discovery-joined-menu-home",
        ] {
            assert!(
                visible.debug_bounds(selector).is_some(),
                "missing keyboard-reachable joined network action {selector}"
            );
        }

        window
            .update(cx, |shell, _, cx| {
                shell.navigate_to_row(
                    &RowNavigation {
                        page_id: "peers".into(),
                        network_name: Some("home".into()),
                        object_id: None,
                        label: t(shell.locale, "nav.peers").to_string(),
                    },
                    cx,
                );
                assert_eq!(shell.selected_page, "peers");
                assert_eq!(shell.selected_network.as_deref(), Some("home"));
                assert_eq!(shell.network_actions_open, None);

                shell.select_network_context("home".into(), cx);
                shell.open_action("networks", "leave_network", cx);
                assert_eq!(shell.selected_page, "networks");
                assert_eq!(shell.selected_network.as_deref(), Some("home"));
                assert!(shell.expanded_actions.contains_key("leave_network"));
            })
            .expect("exercise joined network menu routes");
    }

    #[gpui::test]
    fn discovery_search_debounces_and_filters_local_records_without_commands(
        cx: &mut TestAppContext,
    ) {
        let now = unix_time_ms();
        let studio = discovery_record("studio", now)
            .join_mode(NetworkJoinMode::ApprovalRequired)
            .tags(&["creative"])
            .capabilities(vec![CapabilityKind::Relay])
            .build();
        let family = discovery_record("family", now)
            .visibility(NetworkVisibility::Private)
            .join_mode(NetworkJoinMode::InviteOnly)
            .place("zh-CN", "CN")
            .tags(&["home"])
            .capabilities(vec![CapabilityKind::Exit])
            .build();
        let window = open_shell_window(cx, 1200.0, 900.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.snapshot = NodeSnapshot {
                    discovery_index: NodeDiscoveryIndexSnapshot {
                        state: NodeDiscoveryIndexState::Ready,
                        records: vec![studio, family],
                        ..NodeDiscoveryIndexSnapshot::default()
                    },
                    ..NodeSnapshot::default()
                };
                shell.locale = Locale::En;
                shell.selected_page = "networks".into();
                shell.notice = None;
                shell.error = None;
                shell.set_discovery_query("relay".into(), cx);
                assert_eq!(shell.pending_commands, 0);
                assert_eq!(shell.discovery_query, "relay");
                assert!(shell.discovery_debounced_query.is_empty());
                cx.notify();
            })
            .expect("install debounced discovery fixture");

        cx.executor()
            .advance_clock(std::time::Duration::from_millis(249));
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert!(shell.discovery_debounced_query.is_empty());
            })
            .expect("check debounce before deadline");
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(2));
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.discovery_debounced_query, "relay");
                assert_eq!(shell.pending_commands, 0);
                assert!(shell.notice.is_none());
                assert!(shell.error.is_none());
                let projection = shell.discovery_projection();
                assert_eq!(projection.results.len(), 1);
                assert_eq!(projection.results[0].title, "studio");

                shell.discovery_filters = DiscoveryFilters {
                    visibility: Some(DiscoveryVisibilityFilter::Private),
                    join_mode: Some(NetworkJoinMode::InviteOnly),
                    language: Some("ZH-CN".into()),
                    region: Some("cn".into()),
                };
                shell.discovery_debounced_query.clear();
                let projection = shell.discovery_projection();
                assert_eq!(projection.results.len(), 1);
                assert_eq!(projection.results[0].title, "family");
            })
            .expect("check debounced discovery projection");
    }

    #[gpui::test]
    fn discovery_states_distinguish_empty_no_matches_failure_partial_and_stale(
        cx: &mut TestAppContext,
    ) {
        let now = unix_time_ms();
        let current = discovery_record(
            "current",
            now.saturating_sub(DISCOVERY_RESULT_STALE_AFTER_MS + 1),
        )
        .build();
        let ready = NodeSnapshot {
            discovery_index: NodeDiscoveryIndexSnapshot {
                state: NodeDiscoveryIndexState::Ready,
                records: vec![current.clone()],
                ..NodeDiscoveryIndexSnapshot::default()
            },
            ..NodeSnapshot::default()
        };
        assert_eq!(
            project_discovery(&ready, "", &DiscoveryFilters::default(), now).state,
            DiscoveryProjectionState::Ready
        );
        assert_eq!(
            project_discovery(&ready, "missing", &DiscoveryFilters::default(), now).state,
            DiscoveryProjectionState::NoMatches
        );
        let empty = NodeSnapshot {
            discovery_index: NodeDiscoveryIndexSnapshot {
                state: NodeDiscoveryIndexState::Ready,
                ..NodeDiscoveryIndexSnapshot::default()
            },
            ..NodeSnapshot::default()
        };
        assert_eq!(
            project_discovery(&empty, "", &DiscoveryFilters::default(), now).state,
            DiscoveryProjectionState::Empty
        );

        let mut partial = ready.clone();
        partial.discovery_index.rejected_record_count = 2;
        let partial_projection = project_discovery(&partial, "", &DiscoveryFilters::default(), now);
        assert_eq!(partial_projection.state, DiscoveryProjectionState::Degraded);
        assert!(partial_projection.partial);
        assert!(!partial_projection.no_more_results);

        let failed = NodeSnapshot {
            discovery_index: NodeDiscoveryIndexSnapshot {
                state: NodeDiscoveryIndexState::Failed,
                ..NodeDiscoveryIndexSnapshot::default()
            },
            ..NodeSnapshot::default()
        };
        assert_eq!(
            project_discovery(&failed, "", &DiscoveryFilters::default(), now).state,
            DiscoveryProjectionState::Failed
        );

        let mut degraded = ready;
        degraded.discovery_index.state = NodeDiscoveryIndexState::Degraded;
        degraded.discovery_index.rejected_record_count = 1;
        degraded.discovery_index.last_success_unix_ms = Some(now.saturating_sub(3_000));
        degraded.discovery_index.last_error = Some(NodeDiscoveryIndexError::new(
            NodeDiscoveryIndexErrorCategory::Timeout,
        ));
        let degraded_projection =
            project_discovery(&degraded, "", &DiscoveryFilters::default(), now);
        assert_eq!(
            degraded_projection.state,
            DiscoveryProjectionState::Degraded
        );
        assert!(degraded_projection.has_stale_results());
        assert_eq!(
            degraded_projection.error_category,
            Some(NodeDiscoveryIndexErrorCategory::Timeout)
        );
        assert!(!degraded_projection.no_more_results);

        let window = open_shell_window(cx, 1200.0, 1_000.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.snapshot = degraded;
                shell.locale = Locale::En;
                shell.selected_page = "networks".into();
                cx.notify();
            })
            .expect("install degraded discovery fixture");
        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        for selector in [
            "visual-discovery-state",
            "visual-discovery-rejected",
            "visual-discovery-joined-empty",
            "visual-discovery-stale",
            "visual-discovery-error",
            "visual-discovery-last-success",
            "visual-discovery-result",
        ] {
            assert!(
                visible.debug_bounds(selector).is_some(),
                "missing degraded discovery element {selector}"
            );
        }
        assert!(visible.debug_bounds("visual-discovery-no-more").is_none());
        assert!(visible
            .debug_bounds("visual-discovery-joined-network")
            .is_none());
    }

    #[gpui::test]
    fn discovery_cta_opens_signed_preview_without_submitting(cx: &mut TestAppContext) {
        let now = unix_time_ms();
        let record = discovery_record("atlas", now).build();
        let window = open_shell_window(cx, 1200.0, 900.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.snapshot = NodeSnapshot {
                    discovery_index: NodeDiscoveryIndexSnapshot {
                        state: NodeDiscoveryIndexState::Ready,
                        records: vec![record],
                        ..NodeDiscoveryIndexSnapshot::default()
                    },
                    ..NodeSnapshot::default()
                };
                shell.locale = Locale::En;
                shell.selected_page = "overview".into();
                shell.notice = None;
                shell.error = None;
                let result = shell
                    .discovery_projection()
                    .results
                    .into_iter()
                    .next()
                    .expect("discovery result");
                let network_id = result.network_id.as_str().to_string();
                let pending_commands = shell.pending_commands;

                shell.request_discovery_join(&result, cx);

                assert_eq!(
                    shell.field_value("preview_join_network", "network_id"),
                    network_id
                );
                assert_eq!(shell.pending_commands, pending_commands);
                assert_eq!(shell.selected_page, "networks");
                assert!(shell.expanded_actions.contains_key("preview_join_network"));
                assert!(shell.recent_network_join_preview.is_none());
                assert!(shell.notice.is_none());
                assert!(shell.error.is_none());
            })
            .expect("open discovery join action");
    }

    #[gpui::test]
    fn join_preview_requires_explicit_terms_confirmation_before_submitting(
        cx: &mut TestAppContext,
    ) {
        let window = open_shell_window(cx, 1200.0, 4_000.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.snapshot = NodeSnapshot::default();
                shell.snapshot.lifecycle.phase = crate::node::NodePhase::Running;
                shell
                    .snapshot
                    .permissions
                    .permissions
                    .extend(crate::node::NodePermission::ALL);
                shell.locale = Locale::En;
                let preview = join_preview_fixture();
                shell.set_join_preview_fields(&preview);
                shell.recent_network_join_preview = Some(preview);
                shell.open_action("networks", "join_network", cx);

                assert!(!shell.network_join_terms_confirmed);
                assert!(!shell.network_join_exit_contribution_confirmed);
                assert_eq!(shell.pending_commands, 0);
                shell.confirm_join_preview(cx);
                assert_eq!(shell.pending_commands, 0);
                assert_eq!(
                    shell.error.as_deref(),
                    Some("Confirm the join terms first.")
                );
                shell.error = None;
                shell.network_join_terms_confirmed = true;
                shell.confirm_join_preview(cx);
                assert_eq!(shell.pending_commands, 0);
                assert_eq!(
                    shell.error.as_deref(),
                    Some(
                        "Separately acknowledge the Exit purpose, bandwidth impact, logging boundary and disable conditions."
                    )
                );
            })
            .expect("install join preview fixture");

        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        for selector in [
            "visual-join-network-preview",
            "visual-join-network-exit-disclosure",
            "visual-join-network-terms",
            "visual-join-network-exit-contribution",
            "visual-join-network-confirm",
            "visual-join-network-preview-again",
        ] {
            assert!(
                visible.debug_bounds(selector).is_some(),
                "missing join preview element {selector}"
            );
        }

        let exit_control = visible
            .debug_bounds("visual-join-network-exit-contribution")
            .expect("join Exit consent control");
        visible.simulate_click(exit_control.center(), Modifiers::default());
        visible.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert!(shell.network_join_exit_contribution_confirmed);
            })
            .expect("verify join Exit consent");
    }

    #[gpui::test]
    fn invitation_creation_shows_the_code_once_and_never_puts_it_in_the_notice(
        cx: &mut TestAppContext,
    ) {
        let issued = invitation_issued_fixture();
        let code = issued.code.as_str().to_string();
        let window = open_shell_window(cx, 1200.0, 1_000.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.snapshot = NodeSnapshot::default();
                shell.locale = Locale::En;
                shell.apply_command_result(
                    Ok(NodeCommandResult::completed(
                        90,
                        NodeCommandOutcome::InvitationCreated(Box::new(issued)),
                    )),
                    cx,
                );
                assert_eq!(
                    shell.notice.as_deref(),
                    Some(
                        "Invitation created; the one-time credential is shown only in the dedicated invitation panel."
                    )
                );
                assert!(!shell
                    .notice
                    .as_deref()
                    .is_some_and(|notice| notice.contains(&code)));
                assert!(!format!("{:?}", shell.recent_issued_invitation).contains(&code));
                shell.open_action("networks", "create_invitation", cx);
            })
            .expect("install issued invitation fixture");

        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        for selector in [
            "visual-invitation-issued",
            "visual-invitation-code",
            "visual-invitation-copy",
            "visual-invitation-ordinary-membership",
            "visual-invitation-not-permanent",
            "visual-invitation-privacy-boundary",
        ] {
            assert!(
                visible.debug_bounds(selector).is_some(),
                "missing invitation creation element {selector}"
            );
        }
    }

    #[gpui::test]
    fn invitation_status_and_revoke_actions_pin_current_revisions(cx: &mut TestAppContext) {
        let status = invitation_status_fixture();
        let invitation_id = status.invitation.invitation_id.as_str().to_string();
        let network_id = status.invitation.network_id.as_str().to_string();
        let revision = status.invitation.revision;
        let window = open_shell_window(cx, 1200.0, 1_000.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.snapshot = NodeSnapshot::default();
                shell.locale = Locale::En;
                shell.recent_invitations = Some(vec![status.clone()]);
                shell.prepare_invitation_status(&status, cx);
                assert_eq!(
                    shell.field_value("get_invitation_status", "network_id"),
                    network_id
                );
                assert_eq!(
                    shell.field_value("get_invitation_status", "invitation_id"),
                    invitation_id
                );
                shell.prepare_invitation_revoke(&status, cx);
                assert_eq!(
                    shell.field_value("revoke_invitation", "network_id"),
                    network_id
                );
                assert_eq!(
                    shell.field_value("revoke_invitation", "invitation_id"),
                    invitation_id
                );
                assert_eq!(
                    shell.field_value("revoke_invitation", "expected_revision"),
                    revision.to_string()
                );
                shell.open_action("networks", "list_invitations", cx);
            })
            .expect("install invitation list fixture");

        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        for selector in [
            "visual-invitation-list",
            "visual-invitation-status-0",
            "visual-invitation-refresh-0",
            "visual-invitation-revoke-0",
        ] {
            assert!(
                visible.debug_bounds(selector).is_some(),
                "missing invitation list element {selector}"
            );
        }
    }

    #[gpui::test]
    fn invitation_preview_prefills_pinned_redemption_fields_and_requires_terms(
        cx: &mut TestAppContext,
    ) {
        let preview = invitation_preview_fixture();
        let window = open_shell_window(cx, 1200.0, 1_000.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.snapshot = NodeSnapshot::default();
                shell.locale = Locale::En;
                shell.set_field_value(
                    "preview_invitation",
                    "coordinator_url",
                    "https://coordinator.example.test",
                );
                shell.set_field_value("preview_invitation", "code", "fkinv_preview_secret");
                shell.apply_command_result(
                    Ok(NodeCommandResult::completed(
                        91,
                        NodeCommandOutcome::InvitationPreview(Box::new(preview.clone())),
                    )),
                    cx,
                );
                assert!(!shell.invitation_terms_confirmed);
                assert_eq!(
                    shell.field_value("redeem_invitation", "invitation_id"),
                    preview.invitation_id.as_str()
                );
                assert_eq!(
                    shell.field_value("redeem_invitation", "expected_invitation_revision"),
                    preview.invitation_revision.to_string()
                );
                assert_eq!(
                    shell.field_value("redeem_invitation", "preview_digest"),
                    preview.preview_digest.as_str()
                );
                assert_eq!(
                    shell.field_value("redeem_invitation", "terms_hash"),
                    preview.terms_hash.as_str()
                );
                assert_eq!(
                    shell.field_value("redeem_invitation", "code"),
                    "fkinv_preview_secret"
                );
                shell.confirm_invitation_preview(cx);
                assert_eq!(
                    shell.error.as_deref(),
                    Some(
                        "This invitation cannot be accepted. Refresh its status or contact a network administrator."
                    )
                );
                shell.error = None;
                shell.open_action("networks", "preview_invitation", cx);
            })
            .expect("install invitation preview fixture");

        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        for selector in [
            "visual-invitation-preview",
            "visual-invitation-terms",
            "visual-invitation-confirm",
            "visual-invitation-preview-again",
            "visual-invitation-ordinary-membership",
            "visual-invitation-not-permanent",
        ] {
            assert!(
                visible.debug_bounds(selector).is_some(),
                "missing invitation preview element {selector}"
            );
        }
    }

    #[gpui::test]
    fn autonomous_invitation_discloses_exit_impact_without_forcing_consent(
        cx: &mut TestAppContext,
    ) {
        let window = open_shell_window(cx, 1200.0, 1_000.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.snapshot = NodeSnapshot::default();
                shell.locale = Locale::En;
                let mut preview = invitation_preview_fixture();
                preview.terms.exit_contribution = ExitContributionObligation::NotRequired;
                preview.terms.exit_contribution_scope =
                    fuckipv6_common::domain::ExitContributionScope::NotApplicable;
                preview.terms.exit_contribution_mode = ExitContributionMode::Autonomous;
                preview.terms_hash = preview
                    .terms
                    .content_hash()
                    .expect("autonomous invitation terms hash");
                preview.preview_digest = preview
                    .compute_digest()
                    .expect("autonomous invitation preview digest");
                shell.recent_invitation_preview = Some(preview);
                shell.open_action("networks", "preview_invitation", cx);
            })
            .expect("install autonomous invitation preview");

        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        assert!(visible
            .debug_bounds("visual-invitation-exit-disclosure")
            .is_some());
        assert!(visible
            .debug_bounds("visual-invitation-exit-contribution")
            .is_none());
    }

    #[gpui::test]
    fn non_accepting_invitation_states_never_submit_redemption(cx: &mut TestAppContext) {
        for state in [
            InvitationState::Expired,
            InvitationState::Revoked,
            InvitationState::Exhausted,
        ] {
            let window = open_shell_window(cx, 1200.0, 900.0);
            window
                .update(cx, |shell, _, cx| {
                    shell.manifest = Some(crate::node::node_ui_manifest());
                    shell.snapshot = NodeSnapshot::default();
                    shell.locale = Locale::En;
                    let mut preview = invitation_preview_fixture();
                    preview.state = state;
                    preview.can_accept = false;
                    preview.remaining_uses = if state == InvitationState::Exhausted {
                        0
                    } else {
                        preview.remaining_uses
                    };
                    if state == InvitationState::Expired {
                        preview.expires_at_unix_ms = 1;
                    }
                    shell.recent_invitation_preview = Some(preview);
                    shell.invitation_terms_confirmed = true;
                    let pending_commands = shell.pending_commands;
                    shell.confirm_invitation_preview(cx);
                    assert_eq!(shell.pending_commands, pending_commands);
                    assert_eq!(
                        shell.error.as_deref(),
                        Some(
                            "This invitation cannot be accepted. Refresh its status or contact a network administrator."
                        )
                    );
                    shell.open_action("networks", "preview_invitation", cx);
                })
                .expect("install non-accepting invitation fixture");

            let mut visible = VisualTestContext::from_window(window.into(), cx);
            visible.run_until_parked();
            for selector in [
                "visual-invitation-preview-again",
                "visual-invitation-back-to-edit",
            ] {
                assert!(
                    visible.debug_bounds(selector).is_some(),
                    "missing invitation recovery element {selector} for {state:?}"
                );
            }
        }
    }

    #[gpui::test]
    fn returning_to_invitation_edit_preserves_source_inputs(cx: &mut TestAppContext) {
        let window = open_shell_window(cx, 1200.0, 900.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.locale = Locale::En;
                shell.set_field_value(
                    "preview_invitation",
                    "coordinator_url",
                    "https://coordinator.example.test",
                );
                shell.set_field_value("preview_invitation", "code", "fkinv_preview_secret");
                shell.set_field_value("redeem_invitation", "preview_digest", "stale-digest");
                shell.recent_invitation_preview = Some(invitation_preview_fixture());
                shell.invitation_terms_confirmed = true;

                shell.return_to_invitation_edit(cx);

                assert!(shell.recent_invitation_preview.is_none());
                assert!(!shell.invitation_terms_confirmed);
                assert_eq!(
                    shell.field_value("preview_invitation", "coordinator_url"),
                    "https://coordinator.example.test"
                );
                assert_eq!(
                    shell.field_value("preview_invitation", "code"),
                    "fkinv_preview_secret"
                );
                assert_eq!(shell.field_value("redeem_invitation", "preview_digest"), "");
                assert!(shell.expanded_actions.contains_key("preview_invitation"));
            })
            .expect("return to invitation edit");
    }

    #[gpui::test]
    fn rejected_join_keeps_preview_terms_and_confirmation_fields_for_retry(
        cx: &mut TestAppContext,
    ) {
        let window = open_shell_window(cx, 1200.0, 1_000.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.locale = Locale::En;
                let preview = join_preview_fixture();
                shell.set_join_preview_fields(&preview);
                shell.set_field_value(
                    "join_network",
                    "acknowledged_at_unix_ms",
                    "1700000000000",
                );
                shell.recent_network_join_preview = Some(preview.clone());
                shell.network_join_terms_confirmed = true;

                shell.apply_command_result(
                    Ok(NodeCommandResult::rejected(
                        91,
                        crate::node::NodeProblem::new(
                            crate::node::NodeProblemCode::Conflict,
                            "signed preview changed",
                        )
                        .with_message_key("join.error.preview_changed")
                        .retryable(true),
                    )),
                    cx,
                );

                assert_eq!(shell.recent_network_join_preview.as_ref(), Some(&preview));
                assert!(shell.network_join_terms_confirmed);
                assert_eq!(
                    shell.field_value("join_network", "network_id"),
                    preview.network_id
                );
                assert_eq!(
                    shell.field_value("join_network", "preview_digest"),
                    preview.preview_digest
                );
                assert_eq!(
                    shell.field_value("join_network", "acknowledged_at_unix_ms"),
                    "1700000000000"
                );
                assert_eq!(
                    shell.error.as_deref(),
                    Some(
                        "The network preview changed. Preview it again and confirm the terms. · Refresh the state and retry."
                    )
                );
            })
            .expect("apply rejected join result");
    }

    #[test]
    fn operation_error_renders_backend_detail_before_recovery() {
        let error = OperationError {
            code: "operation.unavailable".into(),
            message_key: "error.operation_unavailable".into(),
            params: BTreeMap::from([("detail".into(), "runtime backend unavailable".into())]),
            object: None,
            retryable: true,
            recovery: Some(fuckipv6_common::domain::OperationRecovery {
                kind: "retry".into(),
                hint_key: "recovery.retry".into(),
            }),
            partial_changes: Vec::new(),
        };
        assert_eq!(
            format_operation_error(Locale::En, &error),
            "The service is temporarily unavailable. · runtime backend unavailable · Retry later."
        );

        let duplicate = OperationError {
            params: BTreeMap::from([(
                "detail".into(),
                "The service is temporarily unavailable.".into(),
            )]),
            recovery: None,
            ..error
        };
        assert_eq!(
            format_operation_error(Locale::En, &duplicate),
            "The service is temporarily unavailable."
        );
    }

    #[gpui::test]
    fn operation_card_offers_cancel_only_for_an_active_cancellable_operation(
        cx: &mut TestAppContext,
    ) {
        for (state, cancellable, expected) in [
            (OperationState::Running, true, true),
            (OperationState::Running, false, false),
            (OperationState::Cancelled, true, false),
        ] {
            let fixture: ProductVisualFixture =
                serde_json::from_str(PRODUCT_VISUAL_SNAPSHOT_FIXTURE)
                    .expect("parse shared product visual fixture");
            let operation = operation_fixture(state, cancellable);
            let window = open_shell_window(cx, 1200.0, 900.0);
            window
                .update(cx, |shell, _, cx| {
                    shell.manifest = Some(crate::node::node_ui_manifest());
                    shell.snapshot = fixture.snapshot;
                    shell.locale = Locale::En;
                    shell.selected_page = "node_runtime".into();
                    shell.operation = Some(operation);
                    cx.notify();
                })
                .expect("install operation fixture");

            let mut visible = VisualTestContext::from_window(window.into(), cx);
            visible.run_until_parked();
            assert!(visible.debug_bounds("visual-node-operation").is_some());
            assert_eq!(
                visible.debug_bounds("visual-operation-cancel").is_some(),
                expected,
                "unexpected cancel visibility for state={state:?} cancellable={cancellable}"
            );
        }
    }

    #[gpui::test]
    fn operation_card_offers_retry_only_for_terminal_retryable_failures(cx: &mut TestAppContext) {
        for (state, retryable, with_command, expected) in [
            (OperationState::Failed, true, true, true),
            (OperationState::TimedOut, true, true, true),
            (OperationState::Failed, false, true, false),
            (OperationState::Failed, true, false, false),
            (OperationState::Running, true, true, false),
        ] {
            let fixture: ProductVisualFixture =
                serde_json::from_str(PRODUCT_VISUAL_SNAPSHOT_FIXTURE)
                    .expect("parse shared product visual fixture");
            let mut operation = operation_fixture(state, false);
            operation.operation.error = Some(OperationError {
                code: "operation.test_failed".into(),
                message_key: "error.operation_test_failed".into(),
                params: Default::default(),
                object: None,
                retryable,
                recovery: None,
                partial_changes: Vec::new(),
            });
            let window = open_shell_window(cx, 1200.0, 900.0);
            window
                .update(cx, |shell, _, cx| {
                    shell.manifest = Some(crate::node::node_ui_manifest());
                    shell.snapshot = fixture.snapshot;
                    shell.locale = Locale::En;
                    shell.selected_page = "node_runtime".into();
                    shell.operation = Some(operation);
                    shell.last_operation_command =
                        with_command.then_some(NodeCommand::GetDiagnostics);
                    cx.notify();
                })
                .expect("install operation fixture");

            let mut visible = VisualTestContext::from_window(window.into(), cx);
            visible.run_until_parked();
            assert_eq!(
                visible.debug_bounds("visual-operation-retry").is_some(),
                expected,
                "unexpected retry visibility for state={state:?} retryable={retryable} with_command={with_command}"
            );
        }
    }

    fn visual_geometry(visible: &mut VisualTestContext) -> Value {
        let mut geometry = Map::new();
        for selector in VISUAL_GEOMETRY_SELECTORS {
            let key = selector.trim_start_matches("visual-").replace('-', "_");
            let bounds = visible
                .debug_bounds(selector)
                .map(|bounds| {
                    json!({
                        "x": f32::from(bounds.origin.x),
                        "y": f32::from(bounds.origin.y),
                        "width": f32::from(bounds.size.width),
                        "height": f32::from(bounds.size.height),
                    })
                })
                .unwrap_or(Value::Null);
            geometry.insert(key, bounds);
        }
        Value::Object(geometry)
    }

    struct SnapshotCommandHandler {
        state: NodeStateStore,
        configs: ConfigManager,
        directory: Option<NetworkMemberDirectory>,
        executed_commands: Arc<Mutex<Vec<NodeCommand>>>,
    }

    impl NodeCommandHandler for SnapshotCommandHandler {
        fn handle(&self, request: NodeCommandRequest) -> NodeCommandFuture {
            let state = self.state.clone();
            let configs = self.configs.clone();
            let directory = self.directory.clone();
            let executed_commands = self.executed_commands.clone();
            Box::pin(async move {
                let request_id = request.request_id;
                match request.command {
                    NodeCommand::GetSnapshot => {
                        NodeCommandResult::snapshot(request_id, state.snapshot().await)
                    }
                    NodeCommand::GetUiManifest => {
                        NodeCommandResult::ui_manifest(request_id, crate::node::node_ui_manifest())
                    }
                    NodeCommand::GetConfiguration => match configs.configuration_document() {
                        Ok(document) => NodeCommandResult::completed(
                            request_id,
                            NodeCommandOutcome::Configuration(document),
                        ),
                        Err(error) => NodeCommandResult::rejected(
                            request_id,
                            crate::node::NodeProblem::new(
                                crate::node::NodeProblemCode::Internal,
                                format!("读取配置失败: {error:#}"),
                            ),
                        ),
                    },
                    NodeCommand::GetConfigurationHistory => match configs.configuration_history() {
                        Ok(history) => NodeCommandResult::completed(
                            request_id,
                            NodeCommandOutcome::ConfigurationHistory(history),
                        ),
                        Err(error) => NodeCommandResult::rejected(
                            request_id,
                            crate::node::NodeProblem::new(
                                crate::node::NodeProblemCode::Internal,
                                format!("读取配置历史失败: {error:#}"),
                            ),
                        ),
                    },
                    NodeCommand::ListNetworkMembers(command) => {
                        if let Ok(mut executed) = executed_commands.lock() {
                            executed.push(NodeCommand::ListNetworkMembers(command.clone()));
                        }
                        match directory
                            .filter(|directory| directory.network_id.as_str() == command.network_id)
                        {
                            Some(directory) => NodeCommandResult::completed(
                                request_id,
                                NodeCommandOutcome::NetworkMembers(Box::new(directory)),
                            ),
                            None => NodeCommandResult::rejected(
                                request_id,
                                crate::node::NodeProblem::new(
                                    crate::node::NodeProblemCode::NotFound,
                                    format!("测试成员目录不存在: {}", command.network_id),
                                ),
                            ),
                        }
                    }
                    NodeCommand::ChangeMemberLifecycle(command) => {
                        if let Ok(mut executed) = executed_commands.lock() {
                            executed.push(NodeCommand::ChangeMemberLifecycle(command));
                        }
                        NodeCommandResult::rejected(
                            request_id,
                            crate::node::NodeProblem::new(
                                crate::node::NodeProblemCode::Internal,
                                "测试 operation 必须通过 operation client 提交",
                            ),
                        )
                    }
                    command => panic!("unexpected GPUI test command: {command:?}"),
                }
            })
        }
    }

    fn test_config_manager() -> ConfigManager {
        static NEXT_CONFIG_DIR: AtomicU64 = AtomicU64::new(1);
        let config_dir = std::env::temp_dir().join(format!(
            "fuckipv6-gpui-test-{}-{}",
            std::process::id(),
            NEXT_CONFIG_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        ConfigManager::new_for_dir(config_dir).expect("temporary config directory")
    }

    fn open_shell_window(
        cx: &mut TestAppContext,
        width: f32,
        height: f32,
    ) -> gpui::WindowHandle<GpuiShell> {
        let state = NodeStateStore::new();
        // GPUI tests do not install a Tokio runtime, while the production
        // command bus dispatches through Tokio. Rendering the initial shell
        // only needs the bus to exist; async command behavior is covered by
        // the node command tests and the dedicated topology directory test.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("temporary Tokio runtime");
        let runtime_guard = runtime.enter();
        let configs = test_config_manager();
        let bus = NodeCommandBus::spawn(
            Arc::new(SnapshotCommandHandler {
                state: state.clone(),
                configs: configs.clone(),
                directory: None,
                executed_commands: Arc::new(Mutex::new(Vec::new())),
            }),
            NodeEventBus::default(),
            4,
        );
        drop(runtime_guard);
        drop(runtime);
        let window = cx.open_window(size(px(width), px(height)), move |_, cx| {
            GpuiShell::new(state, bus, None, configs, cx)
        });
        cx.run_until_parked();
        window
    }

    fn bound_keystrokes(
        cx: &mut TestAppContext,
        action: &dyn gpui::Action,
    ) -> Vec<(bool, bool, String)> {
        let keymap = cx.update(|cx| cx.key_bindings());
        let keymap = keymap.borrow();
        keymap
            .bindings_for_action(action)
            .map(|binding| {
                let keystroke = &binding.keystrokes()[0].inner();
                (
                    keystroke.modifiers.control,
                    keystroke.modifiers.platform,
                    keystroke.key.clone(),
                )
            })
            .collect()
    }

    #[test]
    fn all_manifest_pages_have_a_navigation_label() {
        for page in crate::node::node_ui_manifest().pages {
            assert!(fuckipv6_common::i18n::has_key(
                Locale::ZhCn,
                &page.nav_label_key
            ));
            assert!(!t(Locale::ZhCn, &page.nav_label_key).is_empty());
        }
    }

    #[test]
    fn all_manifest_actions_use_their_declared_label_as_the_cta() {
        for action in crate::node::node_ui_manifest()
            .pages
            .iter()
            .flat_map(|page| &page.actions)
        {
            assert!(
                fuckipv6_common::i18n::has_key(Locale::ZhCn, &action.label_key),
                "missing localized manifest label {}",
                action.label_key
            );
            let label = manifest_action_label(Locale::ZhCn, action);
            assert_eq!(label, t(Locale::ZhCn, &action.label_key));
            assert_ne!(label, "未知动作");
            assert_ne!(label, "提交共享命令");
            assert_eq!(
                manifest_action_cta_label(Locale::ZhCn, action, false),
                label
            );
        }
    }

    #[test]
    fn confirmation_cta_names_the_action_and_displays_impact_before_submission() {
        let manifest = crate::node::node_ui_manifest();
        let restart = manifest
            .pages
            .iter()
            .flat_map(|page| &page.actions)
            .find(|action| action.id == "restart_node")
            .expect("restart action");
        assert_eq!(
            manifest_action_cta_label(Locale::ZhCn, restart, true),
            "再次点击“重启节点”确认"
        );

        for action in manifest.pages.iter().flat_map(|page| &page.actions) {
            let Some(confirmation) = action.confirmation.as_ref() else {
                continue;
            };
            assert!(fuckipv6_common::i18n::has_key(
                Locale::ZhCn,
                &confirmation.title_key
            ));
            assert!(fuckipv6_common::i18n::has_key(
                Locale::ZhCn,
                &confirmation.consequence_key
            ));
            assert!(!t(Locale::ZhCn, &confirmation.consequence_key).is_empty());
        }
        assert!(
            manifest
                .pages
                .iter()
                .flat_map(|page| &page.actions)
                .filter_map(|action| action.confirmation.as_ref())
                .any(|confirmation| confirmation.destructive),
            "manifest must exercise the destructive confirmation presentation"
        );
    }

    #[test]
    fn unavailable_manifest_actions_can_never_be_submitted() {
        let unavailable = crate::node::node_ui_manifest()
            .pages
            .into_iter()
            .flat_map(|page| page.actions)
            .filter(|action| action.backend_status == NodeUiBackendStatus::Unavailable)
            .collect::<Vec<_>>();
        assert!(
            !unavailable.is_empty(),
            "manifest must exercise unavailable actions"
        );
        for action in unavailable {
            assert!(manifest_action_submit_disabled(&action, 0, false));
        }
    }

    #[test]
    fn product_field_labels_never_expose_schema_identifiers_or_validation_names() {
        let field = crate::node::node_ui_manifest().pages[1].actions[0].fields[0].clone();
        let label = field_label(Locale::ZhCn, &field.label_key);
        assert!(!label.contains(&field.id));
        assert!(!label.contains(&format!("{:?}", field.validation)));
    }

    #[gpui::test]
    fn manifest_action_cards_render_labeled_cta_confirmation_and_unavailable_states(
        cx: &mut TestAppContext,
    ) {
        let fixture: ProductVisualFixture = serde_json::from_str(PRODUCT_VISUAL_SNAPSHOT_FIXTURE)
            .expect("parse shared product visual fixture");

        {
            let window = open_shell_window(cx, 1200.0, 900.0);
            let snapshot = fixture.snapshot.clone();
            window
                .update(cx, |shell, _, cx| {
                    shell.manifest = Some(crate::node::node_ui_manifest());
                    shell.snapshot = snapshot;
                    shell.locale = Locale::En;
                    shell.selected_page = "networks".into();
                    shell.selected_network = Some("home".into());
                    shell
                        .expanded_actions
                        .insert("preview_network_profile".into(), true);
                    cx.notify();
                })
                .expect("install profile preview action");
            let mut visible = VisualTestContext::from_window(window.into(), cx);
            visible.run_until_parked();
            assert!(visible
                .debug_bounds("visual-action-head-preview_network_profile")
                .is_some());
            assert!(visible
                .debug_bounds("visual-action-cta-preview_network_profile")
                .is_some());
        }

        {
            let window = open_shell_window(cx, 1200.0, 900.0);
            let snapshot = fixture.snapshot.clone();
            window
                .update(cx, |shell, _, cx| {
                    shell.manifest = Some(crate::node::node_ui_manifest());
                    shell.snapshot = snapshot;
                    shell.locale = Locale::En;
                    shell.selected_page = "node_runtime".into();
                    shell.selected_network = Some("home".into());
                    shell.expanded_actions.insert("restart_node".into(), true);
                    cx.notify();
                })
                .expect("install restart confirmation action");
            let mut visible = VisualTestContext::from_window(window.into(), cx);
            visible.run_until_parked();
            assert!(visible
                .debug_bounds("visual-action-confirmation-restart_node")
                .is_some());
            assert!(visible
                .debug_bounds("visual-action-cta-restart_node")
                .is_some());
        }

        {
            let window = open_shell_window(cx, 1200.0, 900.0);
            let snapshot = fixture.snapshot.clone();
            window
                .update(cx, |shell, _, cx| {
                    shell.manifest = Some(crate::node::node_ui_manifest());
                    shell.snapshot = snapshot;
                    shell.locale = Locale::En;
                    shell.selected_page = "routing".into();
                    shell.selected_network = Some("home".into());
                    shell
                        .expanded_actions
                        .insert("get_traffic_policy".into(), true);
                    cx.notify();
                })
                .expect("install unavailable action");
            let mut visible = VisualTestContext::from_window(window.into(), cx);
            visible.run_until_parked();
            assert!(visible
                .debug_bounds("visual-action-unavailable-get_traffic_policy")
                .is_some());
            assert!(visible
                .debug_bounds("visual-action-cta-get_traffic_policy")
                .is_none());
        }
    }

    #[gpui::test]
    fn empty_action_panel_hides_search_and_unrelated_governance_notice(cx: &mut TestAppContext) {
        let fixture: ProductVisualFixture = serde_json::from_str(PRODUCT_VISUAL_SNAPSHOT_FIXTURE)
            .expect("parse shared product visual fixture");
        let window = open_shell_window(cx, 1200.0, 900.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.snapshot = fixture.snapshot;
                shell.locale = Locale::En;
                shell.selected_page = "overview".into();
                shell.selected_network = Some("home".into());
                cx.notify();
            })
            .expect("install overview without page actions");

        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        assert!(visible.debug_bounds("visual-action-empty").is_some());
        assert!(visible.debug_bounds("visual-action-search").is_none());
        assert!(visible
            .debug_bounds("visual-coordinator-governance-frozen")
            .is_none());
    }

    #[gpui::test]
    fn action_panel_groups_actions_filters_by_action_or_group_and_collapses(
        cx: &mut TestAppContext,
    ) {
        let fixture: ProductVisualFixture = serde_json::from_str(PRODUCT_VISUAL_SNAPSHOT_FIXTURE)
            .expect("parse shared product visual fixture");
        let window = open_shell_window(cx, 1200.0, 900.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.snapshot = fixture.snapshot;
                shell.locale = Locale::En;
                shell.selected_page = "node_runtime".into();
                cx.notify();
            })
            .expect("install grouped runtime actions");

        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        let inspect_group = visible
            .debug_bounds("visual-action-group-action_group-inspect")
            .expect("inspect action group");
        let runtime_group = visible
            .debug_bounds("visual-action-group-action_group-runtime")
            .expect("runtime action group");
        assert!(
            runtime_group.origin.y < inspect_group.origin.y,
            "action groups must follow manifest order"
        );
        assert!(visible.debug_bounds("visual-action-search").is_some());

        window
            .update(cx, |shell, _, cx| {
                shell.action_panel_query = "Run and control".into();
                cx.notify();
            })
            .expect("filter actions by group label");
        visible.run_until_parked();
        assert!(visible
            .debug_bounds("visual-action-group-action_group-runtime")
            .is_some());
        assert!(visible
            .debug_bounds("visual-action-group-action_group-inspect")
            .is_none());
        assert!(visible
            .debug_bounds("visual-action-head-start_node")
            .is_some());
        assert!(visible
            .debug_bounds("visual-action-head-restart_node")
            .is_some());
        assert!(visible
            .debug_bounds("visual-action-head-refresh_snapshot")
            .is_none());

        window
            .update(cx, |shell, _, cx| {
                shell.action_panel_query = "restart node".into();
                cx.notify();
            })
            .expect("filter actions by action label");
        visible.run_until_parked();
        assert!(visible
            .debug_bounds("visual-action-head-restart_node")
            .is_some());
        assert!(visible
            .debug_bounds("visual-action-head-start_node")
            .is_none());

        window
            .update(cx, |shell, _, cx| {
                shell.action_panel_query = "no such action".into();
                cx.notify();
            })
            .expect("install empty action search");
        visible.run_until_parked();
        assert!(visible.debug_bounds("visual-action-search-empty").is_some());
        assert!(visible
            .debug_bounds("visual-action-group-action_group-runtime")
            .is_none());

        window
            .update(cx, |shell, _, cx| {
                shell.action_panel_query.clear();
                cx.notify();
            })
            .expect("clear action search");
        visible.run_until_parked();
        let inspect_toggle = visible
            .debug_bounds("visual-action-group-toggle-action_group-inspect")
            .expect("inspect group toggle");
        visible.simulate_click(inspect_toggle.center(), Modifiers::default());
        visible.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert!(shell
                    .collapsed_action_groups
                    .contains("action_group.inspect"));
            })
            .expect("verify collapsed action group");
        assert!(visible
            .debug_bounds("visual-action-head-refresh_snapshot")
            .is_none());
        assert!(visible
            .debug_bounds("visual-action-head-start_node")
            .is_some());
    }

    #[gpui::test]
    fn action_panel_keeps_one_card_expanded_and_long_form_cta_visible(cx: &mut TestAppContext) {
        let fixture: ProductVisualFixture = serde_json::from_str(PRODUCT_VISUAL_SNAPSHOT_FIXTURE)
            .expect("parse shared product visual fixture");

        {
            let window = open_shell_window(cx, 1200.0, 900.0);
            let snapshot = fixture.snapshot.clone();
            window
                .update(cx, |shell, _, cx| {
                    shell.manifest = Some(crate::node::node_ui_manifest());
                    shell.snapshot = snapshot;
                    shell.locale = Locale::En;
                    shell.selected_page = "node_runtime".into();
                    cx.notify();
                })
                .expect("install runtime actions for expansion test");
            let mut visible = VisualTestContext::from_window(window.into(), cx);
            visible.run_until_parked();
            let start = visible
                .debug_bounds("visual-action-head-start_node")
                .expect("start action header");
            visible.simulate_click(start.center(), Modifiers::default());
            visible.run_until_parked();
            window
                .update(cx, |shell, _, _| {
                    assert!(shell.expanded_actions.contains_key("start_node"));
                })
                .expect("verify first action expanded");

            let stop = visible
                .debug_bounds("visual-action-head-stop_node")
                .expect("stop action remains visible");
            visible.simulate_click(stop.center(), Modifiers::default());
            visible.run_until_parked();
            window
                .update(cx, |shell, _, _| {
                    assert_eq!(shell.expanded_actions.len(), 1);
                    assert!(!shell.expanded_actions.contains_key("start_node"));
                    assert!(shell.expanded_actions.contains_key("stop_node"));
                })
                .expect("verify only second action expanded");
        }

        {
            let window = open_shell_window(cx, 1200.0, 900.0);
            let snapshot = fixture.snapshot;
            window
                .update(cx, |shell, _, cx| {
                    shell.manifest = Some(crate::node::node_ui_manifest());
                    shell.snapshot = snapshot;
                    shell.locale = Locale::En;
                    shell.selected_page = "networks".into();
                    shell.expanded_actions.insert("create_network".into(), true);
                    cx.notify();
                })
                .expect("install long action form");
            let mut visible = VisualTestContext::from_window(window.into(), cx);
            visible.run_until_parked();
            let form = visible
                .debug_bounds("visual-action-form-scroll-create_network")
                .expect("scrollable long action form");
            let cta = visible
                .debug_bounds("visual-action-cta-create_network")
                .expect("long form call to action");
            let panel = visible
                .debug_bounds("visual-actions")
                .expect("action panel bounds");
            let form_height = f32::from(form.size.height);
            let form_bottom = f32::from(form.origin.y + form.size.height);
            let cta_top = f32::from(cta.origin.y);
            let cta_bottom = f32::from(cta.origin.y + cta.size.height);
            let panel_bottom = f32::from(panel.origin.y + panel.size.height);
            assert!(
                form_height <= 340.0,
                "long form should scroll inside its bounded region"
            );
            assert!(
                cta_top >= form_bottom,
                "primary CTA must remain outside the scrolling form"
            );
            assert!(
                cta_bottom <= panel_bottom,
                "primary CTA must remain visible in the action panel"
            );
        }
    }

    #[gpui::test]
    fn action_panel_partitions_unavailable_actions_into_a_collapsible_group(
        cx: &mut TestAppContext,
    ) {
        let fixture: ProductVisualFixture = serde_json::from_str(PRODUCT_VISUAL_SNAPSHOT_FIXTURE)
            .expect("parse shared product visual fixture");
        let window = open_shell_window(cx, 1200.0, 900.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.snapshot = fixture.snapshot;
                shell.locale = Locale::En;
                shell.selected_page = "routing".into();
                cx.notify();
            })
            .expect("install routing actions with unavailable operations");
        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        assert!(visible
            .debug_bounds("visual-action-group-unavailable")
            .is_some());
        assert!(visible
            .debug_bounds("visual-action-head-get_traffic_policy")
            .is_some());
        let toggle = visible
            .debug_bounds("visual-action-group-toggle-unavailable")
            .expect("unavailable group toggle");
        visible.simulate_click(toggle.center(), Modifiers::default());
        visible.run_until_parked();
        assert!(visible
            .debug_bounds("visual-action-head-get_traffic_policy")
            .is_none());
        window
            .update(cx, |shell, _, _| {
                assert!(shell
                    .collapsed_action_groups
                    .contains("common.actions_unavailable_heading"));
            })
            .expect("verify unavailable actions collapsed");
    }

    #[test]
    fn shell_regions_follow_the_shared_contract_order() {
        assert_eq!(
            APP_REGION_ORDER.map(AppRegion::name),
            ["sidebar", "topbar", "content", "action-panel"]
        );
        for region in APP_REGION_ORDER {
            assert_eq!(
                app_region_id(region).to_string(),
                format!("gpui-region-{}", region.name())
            );
        }
    }

    #[test]
    fn every_manifest_page_has_a_content_heading_and_icon() {
        for page in crate::node::node_ui_manifest().pages {
            assert!(fuckipv6_common::i18n::has_key(
                Locale::ZhCn,
                &page.title_key
            ));
            assert!(!page_icon_data(&page.id).is_empty());
        }
    }

    #[test]
    fn action_panel_follows_the_shared_desktop_width_contract() {
        assert_eq!(
            ActionPanelPresentation::for_layout(layout_for_window_width(899.0)),
            ActionPanelPresentation::Stacked
        );
        assert_eq!(
            ActionPanelPresentation::for_layout(layout_for_window_width(900.0)),
            ActionPanelPresentation::Compact
        );
        assert_eq!(
            ActionPanelPresentation::for_layout(layout_for_window_width(1279.0)),
            ActionPanelPresentation::Compact
        );
        assert_eq!(
            ActionPanelPresentation::for_layout(layout_for_window_width(1280.0)),
            ActionPanelPresentation::Full
        );
    }

    #[test]
    fn minimum_native_window_matches_the_shared_contract() {
        let minimum = window_min_size();
        assert_eq!(
            f32::from(minimum.width),
            DIMENSIONS.window_min_width_px as f32
        );
        assert_eq!(
            f32::from(minimum.height),
            DIMENSIONS.window_min_height_px as f32
        );
        assert_eq!(
            layout_for_window_width(f32::from(minimum.width)),
            LayoutClass::Desktop
        );
    }

    #[test]
    fn every_manifest_field_kind_has_a_placeholder_or_choice_editor() {
        for page in crate::node::node_ui_manifest().pages {
            for action in page.actions {
                for field in action.fields {
                    match field.input {
                        NodeUiInputKind::Boolean
                        | NodeUiInputKind::Role
                        | NodeUiInputKind::RoutingMode
                        | NodeUiInputKind::NetworkSelection
                        | NodeUiInputKind::JoinRequestReviewAction => {}
                        _ => assert_ne!(
                            placeholder_for(Locale::ZhCn, &field.input, &action.id, &field.id),
                            ""
                        ),
                    }
                }
            }
        }
    }

    #[test]
    fn action_block_reason_messages_are_user_visible() {
        for reason in [
            ActionBlockReason::NodeNotRunning,
            ActionBlockReason::NoOnlineNetwork,
            ActionBlockReason::NoNetworkSelected,
            ActionBlockReason::PermissionDenied,
            ActionBlockReason::ExitNodeUnsupported,
            ActionBlockReason::TunUnavailable(TunCapabilityReason::PlatformUnsupported),
            ActionBlockReason::TunUnavailable(TunCapabilityReason::PermissionDenied),
            ActionBlockReason::TunUnavailable(TunCapabilityReason::DeviceUnavailable),
            ActionBlockReason::TunUnavailable(TunCapabilityReason::ToolUnavailable),
            ActionBlockReason::TunUnavailable(TunCapabilityReason::Unknown),
        ] {
            assert!(fuckipv6_common::i18n::has_key(
                Locale::ZhCn,
                reason.message_key()
            ));
            assert!(fuckipv6_common::i18n::has_key(
                Locale::En,
                reason.message_key()
            ));
        }
    }

    #[test]
    fn validation_enum_is_rendered_from_manifest() {
        let field = crate::node::node_ui_manifest().pages[1].actions[0].fields[0].clone();
        assert_eq!(field.validation, NodeUiValidation::NetworkName);
    }

    #[test]
    fn every_primary_status_maps_to_a_visible_semantic_state() {
        for status in NodePrimaryStatus::ALL {
            let semantic = primary_status_semantic(status);
            assert!(!status.label_zh().is_empty());
            assert!(!semantic.label_key().is_empty());
        }
    }

    #[test]
    fn overview_primary_action_navigates_to_the_runtime_page() {
        let fixture: ProductVisualFixture = serde_json::from_str(PRODUCT_VISUAL_SNAPSHOT_FIXTURE)
            .expect("parse shared product visual fixture");
        let manifest = crate::node::node_ui_manifest();
        let model = build_page_model(&manifest, &fixture.snapshot, "overview", Locale::En)
            .expect("overview page model");
        let overview = model.overview.as_ref().expect("overview projection");

        assert_eq!(
            visible_overview_primary_page(overview, Some(&manifest)),
            Some("node_runtime")
        );

        let mut without_runtime = manifest.clone();
        without_runtime
            .pages
            .retain(|page| page.id != "node_runtime");
        assert_eq!(
            visible_overview_primary_page(overview, Some(&without_runtime)),
            None
        );
    }

    #[gpui::test]
    fn shell_keymap_binds_refresh_search_and_quit(cx: &mut TestAppContext) {
        cx.update(bind_shell_keys);
        assert_eq!(
            bound_keystrokes(cx, &RefreshSnapshot),
            [
                (true, false, "r".to_string()),
                (false, true, "r".to_string())
            ]
        );
        assert_eq!(
            bound_keystrokes(cx, &FocusSearch),
            [
                (true, false, "k".to_string()),
                (false, true, "k".to_string())
            ]
        );
        assert_eq!(
            bound_keystrokes(cx, &Quit),
            [
                (true, false, "q".to_string()),
                (false, true, "q".to_string())
            ]
        );
    }

    #[gpui::test]
    fn search_results_navigate_to_network_and_peer_context(cx: &mut TestAppContext) {
        let fixture: ProductVisualFixture = serde_json::from_str(PRODUCT_VISUAL_SNAPSHOT_FIXTURE)
            .expect("parse shared product visual fixture");
        let window = open_shell_window(cx, 1280.0, WINDOW_HEIGHT);
        window
            .update(cx, |shell, window, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.snapshot = fixture.snapshot;
                shell.selected_page = "overview".into();
                shell.selected_network = Some("studio".into());

                let network = shell
                    .manifest
                    .as_ref()
                    .and_then(|manifest| {
                        search_node_snapshot(manifest, &shell.snapshot, "home", Locale::En)
                            .into_iter()
                            .find(|result| result.kind == SearchResultKind::Network)
                    })
                    .expect("home network search result");
                shell.navigate_to_search_result(network, window, cx);
                assert_eq!(shell.selected_page, "networks");
                assert_eq!(shell.selected_network.as_deref(), Some("home"));
                assert!(shell.search_query.is_empty());

                shell.selected_network = Some("studio".into());
                let peer = shell
                    .manifest
                    .as_ref()
                    .and_then(|manifest| {
                        search_node_snapshot(manifest, &shell.snapshot, "peer-tokyo", Locale::En)
                            .into_iter()
                            .find(|result| result.kind == SearchResultKind::Peer)
                    })
                    .expect("peer search result");
                shell.navigate_to_search_result(peer, window, cx);
                assert_eq!(shell.selected_page, "peers");
                assert_eq!(shell.selected_network.as_deref(), Some("home"));
                assert!(shell.search_query.is_empty());
            })
            .expect("update shell");
    }

    #[gpui::test]
    fn routing_summary_navigation_selects_network_and_opens_routing_page(cx: &mut TestAppContext) {
        let fixture: ProductVisualFixture = serde_json::from_str(PRODUCT_VISUAL_SNAPSHOT_FIXTURE)
            .expect("parse shared product visual fixture");
        let window = open_shell_window(cx, 1280.0, WINDOW_HEIGHT);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.snapshot = fixture.snapshot;
                shell.selected_page = "node_runtime".into();
                shell.selected_network = Some("studio".into());
                shell.search_query = "stale search".into();

                shell.navigate_to_row(
                    &RowNavigation {
                        page_id: "routing".into(),
                        network_name: Some("home".into()),
                        object_id: None,
                        label: t(Locale::En, "routing.manage").to_string(),
                    },
                    cx,
                );

                assert_eq!(shell.selected_page, "routing");
                assert_eq!(shell.selected_network.as_deref(), Some("home"));
                assert!(shell.search_query.is_empty());
            })
            .expect("update shell");
    }

    #[gpui::test]
    fn shell_menus_use_localized_refresh_and_quit_labels(cx: &mut TestAppContext) {
        for (locale, refresh, quit) in [
            (Locale::ZhCn, "刷新", "退出"),
            (Locale::En, "Refresh", "Quit"),
        ] {
            let menus = cx
                .update(|cx| {
                    set_shell_menus(cx, locale);
                    cx.get_menus()
                })
                .expect("test platform menu support");
            assert_eq!(menus.len(), 1);
            assert_eq!(menus[0].name.as_ref(), "fuckipv6");
            assert_eq!(menus[0].items.len(), 3);

            match &menus[0].items[0] {
                OwnedMenuItem::Action { name, action, .. } => {
                    assert_eq!(name, refresh);
                    assert!(action.partial_eq(&RefreshSnapshot));
                }
                _ => panic!("expected refresh menu action"),
            }
            assert!(matches!(menus[0].items[1], OwnedMenuItem::Separator));
            match &menus[0].items[2] {
                OwnedMenuItem::Action { name, action, .. } => {
                    assert_eq!(name, quit);
                    assert!(action.partial_eq(&Quit));
                }
                _ => panic!("expected quit menu action"),
            }
        }
    }

    #[gpui::test]
    fn shell_declares_structural_accessibility_roles(cx: &mut TestAppContext) {
        let window = open_shell_window(cx, 1280.0, WINDOW_HEIGHT);
        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        assert!(visible.update(|window, _| window.is_a11y_enabled()));
        assert_eq!(
            [
                A11Y_ROOT_ROLE,
                A11Y_NAVIGATION_ROLE,
                A11Y_MAIN_ROLE,
                A11Y_HEADING_ROLE,
                A11Y_TAB_LIST_ROLE,
                A11Y_TAB_PANEL_ROLE,
            ]
            .map(|role| format!("{role:?}")),
            [
                "Application",
                "Navigation",
                "Main",
                "Heading",
                "TabList",
                "TabPanel",
            ]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn writes_headless_product_shell_screenshot_when_requested() {
        let Some(output) = std::env::var_os("GPUI_VISUAL_SNAPSHOT") else {
            return;
        };
        let selected_page = std::env::var("GPUI_VISUAL_PAGE").unwrap_or_else(|_| "overview".into());
        let expanded_action = std::env::var("GPUI_VISUAL_EXPANDED_ACTION").ok();

        let fixture: ProductVisualFixture = serde_json::from_str(PRODUCT_VISUAL_SNAPSHOT_FIXTURE)
            .expect("parse shared product visual fixture");
        let expected_revision = fixture.snapshot.revision;
        let state = NodeStateStore::new();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("visual-test Tokio runtime");
        runtime.block_on(state.replace_authoritative(fixture.snapshot));
        let _runtime_guard = runtime.enter();
        let config_dir =
            std::env::temp_dir().join(format!("fuckipv6-gpui-visual-{}", std::process::id()));
        let configs = ConfigManager::new_for_dir(config_dir).expect("temporary config directory");
        configs
            .set_node_language_preference(LanguagePreference::En)
            .expect("fixture language preference");
        let bus = NodeCommandBus::spawn(
            Arc::new(SnapshotCommandHandler {
                state: state.clone(),
                configs: configs.clone(),
                directory: None,
                executed_commands: Arc::new(Mutex::new(Vec::new())),
            }),
            NodeEventBus::default(),
            4,
        );
        let text_system = Arc::new(gpui_wgpu::CosmicTextSystem::new("Inter"));
        let mut cx = gpui::HeadlessAppContext::with_platform(
            text_system,
            Arc::new(()),
            gpui_platform::current_headless_renderer,
        );
        let window = cx
            .open_window(size(px(1440.0), px(900.0)), move |_, cx| {
                cx.new(|cx| GpuiShell::new(state, bus, None, configs, cx))
            })
            .expect("headless GPUI window");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            let ready = cx.update(|app| {
                window
                    .read(app)
                    .map(|shell| {
                        shell.manifest.is_some()
                            && shell.snapshot.revision == expected_revision
                            && shell.locale == Locale::En
                            && shell.pending_commands == 0
                            && shell.error.is_none()
                    })
                    .unwrap_or(false)
            });
            if ready {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "GPUI product shell did not finish loading its manifest and snapshot"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let selected_page_for_shell = selected_page.clone();
        let expanded_action_for_shell = expanded_action.clone();
        cx.update(|app| {
            window
                .update(app, |shell, window, cx| {
                    shell.selected_page = selected_page_for_shell;
                    shell.selected_network = Some("home".into());
                    shell.notice = None;
                    shell.error = None;
                    if let Some(action_id) = expanded_action_for_shell {
                        shell.expanded_actions.insert(action_id, true);
                    }
                    window.set_window_title("fuckipv6");
                    cx.notify();
                })
                .expect("install deterministic GPUI visual state");
        });
        cx.run_until_parked();
        let semantics = cx.update(|app| {
            let shell = window.read(app).expect("read GPUI screenshot shell");
            let manifest = shell
                .manifest
                .as_ref()
                .expect("GPUI screenshot manifest is loaded");
            let page = manifest
                .pages
                .iter()
                .find(|page| page.id == shell.selected_page)
                .expect("GPUI screenshot page is in the manifest");
            let model = build_page_model(
                manifest,
                &shell.snapshot,
                &shell.selected_page,
                shell.locale,
            )
            .expect("project GPUI screenshot page model");
            let overview = model.overview.as_ref();
            let page_tabs = navigation_module_for_page(&shell.selected_page)
                .map(|module| {
                    module
                        .page_ids
                        .iter()
                        .filter_map(|page_id| {
                            manifest.pages.iter().find(|page| &page.id == page_id)
                        })
                        .map(|page| t(shell.locale, &page.nav_label_key).to_string())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            json!({
                "title": "fuckipv6",
                "locale": "en",
                "font_family": "Inter",
                "viewport": {
                    "width": 1440,
                    "height": 900,
                    "devicePixelRatio": 2,
                },
                "modules": navigation_modules()
                    .iter()
                    .map(|module| t(shell.locale, &module.label_key).to_string())
                    .collect::<Vec<_>>(),
                "page_tabs": page_tabs,
                "primary_status": overview.map(|overview| json!({
                    "label": t(shell.locale, overview.primary_status.label_key()).to_string(),
                    "detail": t(shell.locale, overview.primary_status.detail_key()).to_string(),
                })),
                "selected_page": shell.selected_page,
                "expanded_action": expanded_action,
                "page_heading": t(shell.locale, &page.title_key).to_string(),
                "action_panel": t(shell.locale, "common.actions_panel_title").to_string(),
                "actions": page.actions
                    .iter()
                    .filter(|action| !is_configuration_action(&action.id))
                    .map(|action| t(shell.locale, &action.label_key).to_string())
                    .collect::<Vec<_>>(),
                "shortcuts": overview
                    .map(|overview| {
                        overview
                            .shortcuts
                            .iter()
                            .map(|shortcut| shortcut.label.clone())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
                "network_statuses": overview
                    .map(|overview| {
                        overview
                            .networks
                            .iter()
                            .map(|network| {
                                t(shell.locale, network_status_key(network.status)).to_string()
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
            })
        });
        let image = cx
            .capture_screenshot(window.into())
            .expect("GPUI headless screenshot");
        let output = std::path::PathBuf::from(output);
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent).expect("screenshot directory");
        }
        let metadata = json!({
            "schema_version": 1,
            "surface": "gpui",
            "fixture_revision": expected_revision,
            "fixture_locale": "en",
            "platform": std::env::consts::OS,
            "dimensions": {
                "width": image.width(),
                "height": image.height(),
            },
            "semantics": semantics,
        });
        image.save(&output).expect("write GPUI screenshot");
        std::fs::write(
            output.with_extension("json"),
            format!(
                "{}\n",
                serde_json::to_string_pretty(&metadata).expect("serialize GPUI visual metadata")
            ),
        )
        .expect("write GPUI visual metadata");
    }

    #[gpui::test]
    fn reports_product_shell_geometry_for_visual_regression(cx: &mut TestAppContext) {
        if std::env::var_os("GPUI_VISUAL_GEOMETRY").is_none() {
            return;
        }

        let fixture: ProductVisualFixture = serde_json::from_str(PRODUCT_VISUAL_SNAPSHOT_FIXTURE)
            .expect("parse shared product visual fixture");
        let window = open_shell_window(cx, 1440.0, 900.0);
        window
            .update(cx, |shell, _, cx| {
                shell.manifest = Some(crate::node::node_ui_manifest());
                shell.snapshot = fixture.snapshot;
                shell.locale = Locale::En;
                shell.selected_page = "overview".into();
                shell.selected_network = Some("home".into());
                shell.notice = None;
                shell.error = None;
                cx.notify();
            })
            .expect("install visual fixture");
        cx.run_until_parked();

        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        for selector in VISUAL_GEOMETRY_SELECTORS {
            println!("{selector}: {:?}", visible.debug_bounds(selector));
        }
        if let Some(output) = std::env::var_os("GPUI_VISUAL_GEOMETRY_JSON") {
            let output = std::path::PathBuf::from(output);
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent).expect("geometry directory");
            }
            let geometry = visual_geometry(&mut visible);
            std::fs::write(
                output,
                format!(
                    "{}\n",
                    serde_json::to_string_pretty(&geometry).expect("serialize GPUI geometry")
                ),
            )
            .expect("write GPUI visual geometry");
        }
    }

    #[gpui::test]
    fn high_dpi_preserves_logical_window_width_and_breakpoints(cx: &mut TestAppContext) {
        let window = open_shell_window(cx, 900.0, WINDOW_HEIGHT);
        let mut visible = VisualTestContext::from_window(window.into(), cx);
        let before = visible.update(|window, _| window.viewport_size());
        visible.simulate_scale_factor_change(1.5);
        let after = visible.update(|window, _| window.viewport_size());
        let scale_factor = visible.update(|window, _| window.scale_factor());

        assert_eq!(scale_factor, 1.5);
        assert_eq!(before, after);
        assert_eq!(f32::from(after.width), 900.0);
        assert_eq!(
            layout_for_window_width(f32::from(after.width)),
            LayoutClass::Desktop
        );
        assert_eq!(
            ActionPanelPresentation::for_layout(layout_for_window_width(1280.0)),
            ActionPanelPresentation::Full
        );
    }
}
