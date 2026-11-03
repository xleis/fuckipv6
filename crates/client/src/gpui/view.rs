use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use fuckipv6_common::design_tokens::{DARK_THEME, DIMENSIONS};
use fuckipv6_common::domain::{
    CapabilityKind, ExitContributionObligation, IdempotencyKey, NetworkJoinMode,
    NetworkProfileApplyResult, NetworkProfilePreview, NetworkProfileProjections,
    NetworkProfileWarningCode, NetworkPublicProfileDocument, NetworkVisibility, OperationError,
    OperationState,
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
    actions, div, prelude::*, px, rgba, size, App, Bounds, ClipboardItem, Context, Div, Entity,
    FocusHandle, KeyBinding, Menu, MenuItem, Render, Role, SharedString, Window, WindowBounds,
    WindowOptions,
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
use super::view_model::{
    action_block_reason, build_command, build_page_model, default_form_values, network_status_key,
    search_node_snapshot, validate_manifest_bindings, ActionBinding, OverviewModel, PageKind,
    RowModel, RowNavigation, SearchResult, SectionModel,
};
use crate::config_manager::ConfigManager;
use crate::config_transaction::{
    ConfigurationChangeAction, ConfigurationHistoryEntry, ConfigurationImpact,
};
use crate::network_discovery::{
    project_discovery, DiscoveryFilters, DiscoveryProjection, DiscoveryProjectionState,
    DiscoveryResult, DiscoveryVisibilityFilter,
};
use crate::node::{
    NodeApplyConfigurationCommand, NodeCommand, NodeCommandBus, NodeCommandBusError,
    NodeCommandContext, NodeCommandOutcome, NodeCommandRequest, NodeCommandResult,
    NodeDiagnosticCheck, NodeDiagnosticLayer, NodeDiagnosticState, NodeDiagnosticsSnapshot,
    NodeNetworkCreationNextStep, NodeNetworkJoinPreview, NodeNetworkStatus, NodeOperationSnapshot,
    NodePreviewConfigurationCommand, NodePrimaryStatus, NodeProblem,
    NodeRollbackConfigurationCommand, NodeSnapshot, NodeStateStore, NodeUiAction,
    NodeUiBackendStatus, NodeUiField, NodeUiInputKind, NodeUiManifest, NodeUiPage,
    OperationClientError,
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
const COLOR_BORDER: u32 = DARK_THEME.colors.border.rgba_hex();
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
        .bg(gpui::linear_gradient(
            150.0,
            gpui::linear_color_stop(rgba(colors.surface.rgba_hex()), 0.0),
            gpui::linear_color_stop(
                rgba(colors.background_deep.with_alpha(0xeb).rgba_hex()),
                1.0,
            ),
        ))
        .border_1()
        .border_color(rgba(COLOR_BORDER))
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
        .border_1()
        .border_dashed()
        .border_color(rgba(COLOR_BORDER))
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
    discovery_search_input: Option<Entity<NativeTextInput>>,
    discovery_query: String,
    discovery_debounced_query: String,
    discovery_search_generation: u64,
    discovery_filters: DiscoveryFilters,
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
    operation_cancel_pending: bool,
    operation_cancel_error: Option<String>,
    recent_diagnostics: Option<NodeDiagnosticsSnapshot>,
    diagnostic_copy_notice: Option<String>,
    recent_network_profile_preview: Option<NetworkProfilePreview>,
    recent_network_profile_updated: Option<NetworkProfileApplyResult>,
    recent_network_join_preview: Option<NodeNetworkJoinPreview>,
    network_join_terms_confirmed: bool,
    error: Option<String>,
    notice: Option<String>,
    pending_notice_after_snapshot: Option<String>,
    language_menu_open: bool,
    network_menu_open: bool,
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
            discovery_search_input: None,
            discovery_query: String::new(),
            discovery_debounced_query: String::new(),
            discovery_search_generation: 0,
            discovery_filters: DiscoveryFilters::default(),
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
            operation_cancel_pending: false,
            operation_cancel_error: None,
            recent_diagnostics: None,
            diagnostic_copy_notice: None,
            recent_network_profile_preview: None,
            recent_network_profile_updated: None,
            recent_network_join_preview: None,
            network_join_terms_confirmed: false,
            error: None,
            notice: None,
            pending_notice_after_snapshot: None,
            language_menu_open: false,
            network_menu_open: false,
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
        if let Err(error) = self.configs.set_node_language_preference(preference) {
            self.error = Some(tr(
                self.locale,
                "gpui.operation_failed",
                &[("error", &error.to_string())],
            ));
        }
        cx.notify();
    }

    fn apply_snapshot(&mut self, snapshot: NodeSnapshot, cx: &mut Context<Self>) {
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

        if let Some(reason) =
            action_block_reason(&action, &self.snapshot, self.selected_network.as_deref())
        {
            self.error = Some(tr(
                self.locale,
                "gpui.action_blocked",
                &[
                    ("action", &action.id),
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

        let values = self.form_values.clone();
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
                self.notice = Some(tr(
                    self.locale,
                    "gpui.join_requests_loaded",
                    &[("count", &result.requests.len().to_string())],
                ));
            }
            NodeCommandOutcome::JoinRequestsReviewed(result) => {
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
                self.recent_diagnostics = Some(diagnostics);
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
        for field in [
            "network_id",
            "preview_digest",
            "network_policy_revision",
            "privacy_policy_revision",
            "terms_hash",
            "acknowledged_at_unix_ms",
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
        self.set_field_value(
            "join_network",
            "acknowledged_at_unix_ms",
            unix_time_ms().to_string(),
        );
        self.submit_manifest_action("join_network", cx);
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
                .border_1()
                .border_color(rgba(COLOR_BORDER))
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
                    .border_1()
                    .border_color(rgba(COLOR_BORDER))
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
                    .border_1()
                    .border_color(rgba(COLOR_BORDER))
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
                    .border_1()
                    .border_color(rgba(COLOR_BORDER))
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
        div()
            .flex()
            .items_center()
            .gap_2()
            .min_w(px(0.0))
            .child(
                div()
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
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(COLOR_TEXT))
                            .child(t(self.locale, primary_status.label_key()).to_string()),
                    )
                    .child(
                        div()
                            .overflow_hidden()
                            .text_xs()
                            .text_color(rgba(COLOR_MUTED))
                            .child(t(self.locale, primary_status.detail_key()).to_string()),
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
            .border_b_1()
            .border_color(rgba(COLOR_BORDER))
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
            .h_full()
            .bg(rgba(COLOR_SIDEBAR))
            .border_r_1()
            .border_color(rgba(COLOR_BORDER))
            .child(
                div()
                    .h(px(DIMENSIONS.topbar_px as f32))
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .border_b_1()
                    .border_color(rgba(COLOR_BORDER))
                    .child(
                        div()
                            .w(px(36.0))
                            .h(px(36.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .bg(rgba(DARK_THEME.colors.accent_soft.rgba_hex()))
                            .border_1()
                            .border_color(rgba(
                                DARK_THEME.colors.accent.with_alpha(0x73).rgba_hex(),
                            ))
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
                    .border_t_1()
                    .border_color(rgba(COLOR_BORDER))
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
            .border_0()
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
            .border_b_1()
            .border_color(rgba(COLOR_BORDER))
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
                                .border_b_1()
                                .border_color(rgba(COLOR_BORDER))
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
                .border_b_1()
                .border_color(rgba(COLOR_BORDER))
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
        self.expanded_actions.insert(action_id.to_string(), true);
        cx.notify();
    }

    fn select_network_context(&mut self, network_name: String, cx: &mut Context<Self>) {
        self.selected_network = Some(network_name.clone());
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
                self.select_network_context(network_name, cx);
            }
        }

        self.pending_confirmation = None;
        self.language_menu_open = false;
        self.network_menu_open = false;
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
            .border_color(rgba(match overview.primary_status {
                NodePrimaryStatus::Running => colors.success.with_alpha(0x59).rgba_hex(),
                NodePrimaryStatus::Degraded
                | NodePrimaryStatus::Starting
                | NodePrimaryStatus::Stopping => colors.warning.with_alpha(0x66).rgba_hex(),
                NodePrimaryStatus::Failed => colors.danger.with_alpha(0x66).rgba_hex(),
                NodePrimaryStatus::Uninitialized
                | NodePrimaryStatus::Stopped
                | NodePrimaryStatus::ControlUnavailable => COLOR_BORDER,
            }))
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

        let mut stack = div()
            .id("overview-stack")
            .flex()
            .flex_col()
            .gap_4()
            .max_w(px(980.0))
            .child(primary_card);

        if !overview.issues.is_empty() {
            stack = stack.child(
                surface_card()
                    .id("overview-issues")
                    .flex()
                    .flex_col()
                    .p_4()
                    .border_color(rgba(colors.warning.with_alpha(0x59).rgba_hex()))
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
                            .border_t_1()
                            .border_color(rgba(COLOR_BORDER))
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
                                .border_t_1()
                                .border_color(rgba(COLOR_BORDER))
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
                        .when(index > 0, |row| {
                            row.border_t_1().border_color(rgba(COLOR_BORDER))
                        })
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
                    .h(px(51.0))
                    .px_3()
                    .py_2()
                    .rounded(px(9.0))
                    .border_1()
                    .border_color(rgba(COLOR_BORDER))
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
            .border_1()
            .border_color(rgba(COLOR_BORDER))
            .child(
                div()
                    .w(px(46.0))
                    .h(px(46.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .bg(rgba(DARK_THEME.colors.accent_soft.rgba_hex()))
                    .border_1()
                    .border_color(rgba(COLOR_ACCENT))
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
                            .border_1()
                            .border_color(rgba(COLOR_BORDER))
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
                .border_1()
                .border_color(rgba(COLOR_BORDER))
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
                .border_1()
                .border_color(rgba(COLOR_BORDER))
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
                .border_1()
                .border_color(rgba(COLOR_BORDER))
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
                    .border_t_1()
                    .border_color(rgba(COLOR_BORDER))
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
                .border_1()
                .border_color(rgba(COLOR_BORDER))
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
                        .border_1()
                        .border_color(rgba(COLOR_WARNING))
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
                            .border_1()
                            .border_color(rgba(if active {
                                DARK_THEME.colors.danger.rgba_hex()
                            } else {
                                COLOR_BORDER
                            }))
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
                        .border_1()
                        .border_color(rgba(COLOR_BORDER))
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
                                    .border_1()
                                    .border_color(rgba(COLOR_BORDER))
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
                .border_t_1()
                .border_color(rgba(COLOR_BORDER))
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
                    .border_1()
                    .border_color(rgba(COLOR_BORDER))
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
                    .border_1()
                    .border_color(rgba(DARK_THEME.colors.danger.rgba_hex()))
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
                        .border_1()
                        .border_color(rgba(DARK_THEME.colors.info.rgba_hex()))
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
                .border_1()
                .border_color(rgba(COLOR_BORDER))
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
                    .border_1()
                    .border_color(rgba(COLOR_BORDER))
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
                            .border_1()
                            .border_color(rgba(COLOR_WARNING))
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
                .border_1()
                .border_color(rgba(COLOR_BORDER))
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
                            .border_1()
                            .border_color(rgba(COLOR_BORDER))
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
                .border_1()
                .border_color(rgba(COLOR_BORDER))
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
                        .border_1()
                        .border_dashed()
                        .border_color(rgba(COLOR_BORDER))
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
                        .border_1()
                        .border_color(rgba(palette.border.rgba_hex()))
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
                    .border_1()
                    .border_color(rgba(COLOR_BORDER))
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
            .border_1()
            .border_color(rgba(COLOR_BORDER))
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
            .border_1()
            .border_color(rgba(COLOR_BORDER))
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
            .border_1()
            .border_color(rgba(COLOR_BORDER))
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
            .child(residue)
            .child(recovery)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(rgba(COLOR_BORDER))
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

        let mut actions = div().flex().flex_wrap().items_center().gap_2();
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
            .border_1()
            .border_color(rgba(if operation.operation.error.is_some() {
                COLOR_WARNING
            } else {
                COLOR_BORDER
            }))
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
                div()
                    .debug_selector(|| "visual-discovery-joined-network".into())
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_3()
                    .rounded_md()
                    .bg(rgba(COLOR_PANEL_RAISED))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(COLOR_TEXT))
                            .child(network_name.clone()),
                    )
                    .child(div().text_xs().text_color(rgba(COLOR_MUTED)).child(detail))
                    .into_any_element()
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
            .border_0()
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
                    shell.open_action("networks", "retry_discovery_index", cx);
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
                SemanticState::Danger,
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
                                        .border_1()
                                        .border_color(rgba(COLOR_WARNING))
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
                    .border_t_1()
                    .border_color(rgba(COLOR_BORDER))
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
                .border_t_1()
                .border_color(rgba(COLOR_BORDER))
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
        let model =
            match build_page_model(manifest, &self.snapshot, &self.selected_page, self.locale) {
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
                            .border_1()
                            .border_color(rgba(COLOR_BORDER))
                            .text_color(rgba(COLOR_ACCENT))
                            .child(page_icon),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .flex_col()
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .text_size(px(20.0))
                                    .line_height(px(24.0))
                                    .font_weight(gpui::FontWeight(600.0))
                                    .text_color(rgba(COLOR_TEXT))
                                    .child(page_title),
                            )
                            .child(
                                div()
                                    .mt(px(3.0))
                                    .text_xs()
                                    .line_height(px(16.0))
                                    .text_color(rgba(COLOR_MUTED))
                                    .child(
                                        t(self.locale, "common.node_contract_description")
                                            .to_string(),
                                    ),
                            ),
                    ),
            );

        if let Some(overview) = model.overview.as_ref() {
            return content
                .child(self.overview_content(overview, cx))
                .into_any_element();
        }

        if self.selected_page == "networks" {
            return content
                .child(self.networks_content(layout, cx))
                .into_any_element();
        }

        if self.selected_page == "node_runtime" {
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

    fn action_card(&self, action: NodeUiAction, cx: &mut Context<Self>) -> gpui::AnyElement {
        let action_id = action.id.clone();
        let toggle_action_id = action.id.clone();
        let block_reason =
            action_block_reason(&action, &self.snapshot, self.selected_network.as_deref());
        let blocked = block_reason.is_some();
        let confirmation_pending = self.pending_confirmation.as_deref() == Some(action.id.as_str());
        let expanded = self
            .expanded_actions
            .get(&action.id)
            .copied()
            .unwrap_or(false);
        let disabled = self.pending_commands > 0 || blocked;
        let button_variant = if confirmation_pending {
            ButtonVariant::Danger
        } else if action.backend_status == NodeUiBackendStatus::Unavailable {
            ButtonVariant::Secondary
        } else {
            ButtonVariant::Primary
        };
        let fields = action
            .fields
            .iter()
            .map(|field| self.field_editor(&action, field, cx))
            .collect::<Vec<_>>();
        let join_preview_card = (action.id == "join_network")
            .then_some(self.recent_network_join_preview.as_ref())
            .flatten()
            .map(|preview| self.join_preview_card(preview, cx));
        let status = match action.backend_status {
            NodeUiBackendStatus::Implemented => t(self.locale, "status.backend.implemented"),
            NodeUiBackendStatus::Unavailable => t(self.locale, "status.backend.unavailable"),
        };
        let button_label = if confirmation_pending {
            t(self.locale, "common.confirm_again")
        } else if action.confirmation.is_some() {
            t(self.locale, "common.ready_to_submit")
        } else {
            t(self.locale, "common.submit_command")
        };

        let action_label = action_label(self.locale, &action.id);
        let mut card = div()
            .id(SharedString::from(format!("action-card-{}", action.id)))
            .flex()
            .flex_col()
            .w_full()
            .border_b_1()
            .border_color(rgba(if confirmation_pending {
                COLOR_WARNING
            } else {
                COLOR_BORDER
            }))
            .child(
                div()
                    .id(SharedString::from(format!(
                        "action-card-head-{}",
                        action.id
                    )))
                    .when(action_id == "refresh_snapshot", |head| {
                        head.debug_selector(|| "visual-action-first".into())
                    })
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
                        let expanded = shell
                            .expanded_actions
                            .entry(toggle_action_id.clone())
                            .or_insert(false);
                        *expanded = !*expanded;
                        cx.notify();
                    }))
                    .child(action_label)
                    .child(if expanded { "⌄" } else { "›" }),
            );

        if expanded {
            let mut body = div().flex().flex_col().gap_3().px_4().pb_4().child(
                div()
                    .text_xs()
                    .text_color(rgba(COLOR_MUTED))
                    .child(format!("{} · {}", action.id, status)),
            );
            if let Some(preview_card) = join_preview_card {
                // A discovery-backed join must always pass through the
                // signed preview and explicit terms acknowledgement. Hide the
                // generic ticket fields so they cannot be mistaken for the
                // final confirmation path.
                body = body.child(preview_card);
            } else {
                body = body.children(fields).child(
                    button(
                        SharedString::from(format!("action-{}", action.id)),
                        button_label,
                        button_variant,
                        disabled,
                    )
                    .w_full()
                    .on_click(cx.listener(move |shell, _, _, cx| {
                        if shell.pending_commands == 0 && !blocked {
                            shell.submit_manifest_action(&action_id, cx);
                        }
                    })),
                );
            }
            body = body.children(block_reason.map(|reason| {
                div()
                    .text_xs()
                    .text_color(rgba(COLOR_WARNING))
                    .child(tr(
                        self.locale,
                        "gpui.action_blocked",
                        &[
                            ("action", &action.id),
                            ("reason", t(self.locale, reason.message_key())),
                        ],
                    ))
                    .into_any_element()
            }));
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
            _ => self.text_field(action, field, &value, cx),
        };

        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().text_xs().text_color(rgba(COLOR_MUTED)).child(format!(
                "{label}{required} · {} · {:?}",
                field.id, field.validation
            )))
            .child(control)
            .into_any_element()
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

    fn join_preview_card(
        &self,
        preview: &NodeNetworkJoinPreview,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let pending = self.pending_commands > 0;
        let can_join = preview.can_join && !preview.already_member;
        let checked = self.network_join_terms_confirmed;
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
        let confirm = button(
            SharedString::from("join-network-confirm"),
            t(self.locale, "join.confirm"),
            ButtonVariant::Primary,
            !can_join || !checked || pending,
        )
        .debug_selector(|| "visual-join-network-confirm".into())
        .w_full()
        .on_click(cx.listener(|shell, _, _, cx| {
            if shell.pending_commands == 0
                && shell.network_join_terms_confirmed
                && shell
                    .recent_network_join_preview
                    .as_ref()
                    .is_some_and(|preview| preview.can_join && !preview.already_member)
            {
                shell.confirm_join_preview(cx);
            }
        }));
        let status_color = if can_join {
            COLOR_ACCENT
        } else {
            COLOR_WARNING
        };

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
            .border_1()
            .border_color(rgba(status_color))
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
                            match preview.exit_contribution {
                                ExitContributionObligation::Required => "join.exit.required",
                                ExitContributionObligation::NotRequired => "join.exit.not_required",
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
            .child(terms_control)
            .child(confirm);

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
                        .border_1()
                        .border_color(rgba(COLOR_WARNING))
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
            .border_1()
            .border_color(rgba(COLOR_WARNING))
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
            .border_1()
            .border_color(rgba(COLOR_BORDER))
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
            .border_1()
            .border_color(rgba(COLOR_BORDER))
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
            .border_1()
            .border_color(rgba(COLOR_BORDER))
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
                        .border_1()
                        .border_color(rgba(COLOR_WARNING))
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
        let action_cards = actions
            .into_iter()
            .map(|action| self.action_card(action, cx))
            .collect::<Vec<_>>();
        let body = div()
            .id("action-panel-body")
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .when(action_cards.is_empty(), |body| {
                body.child(
                    div()
                        .id("no-shared-actions")
                        .p_4()
                        .text_sm()
                        .text_color(rgba(COLOR_MUTED))
                        .child(t(self.locale, "common.page_actions_empty").to_string()),
                )
            })
            .children(action_cards)
            .when_some(self.network_profile_result_card(), |body, result| {
                body.child(div().px_3().pb_3().child(result))
            });

        let mut panel = div()
            .id(app_region_id(AppRegion::ActionPanel))
            .debug_selector(|| "visual-actions".into())
            .role(Role::Complementary)
            .aria_label(t(self.locale, "common.shared_actions").to_string())
            .flex()
            .flex_col()
            .min_w(px(0.0))
            .overflow_hidden()
            .rounded_lg()
            .bg(rgba(DARK_THEME.colors.surface.rgba_hex()))
            .border_1()
            .border_color(rgba(COLOR_BORDER))
            .shadow(vec![gpui::BoxShadow::new(
                px(0.0),
                px(1.0),
                rgba(0x00000066).into(),
            )
            .blur_radius(px(3.0))])
            .child(
                div()
                    .debug_selector(|| "visual-action-title".into())
                    .flex()
                    .items_center()
                    .h(px(40.5))
                    .px_4()
                    .border_b_1()
                    .border_color(rgba(COLOR_BORDER))
                    .text_size(px(10.0))
                    .font_weight(gpui::FontWeight(600.0))
                    .text_color(rgba(COLOR_MUTED))
                    .child(
                        t(self.locale, "common.shared_actions")
                            .to_string()
                            .to_uppercase(),
                    ),
            )
            .child(body);

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
            .border_1()
            .border_color(rgba(COLOR_BORDER))
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
        .border_1()
        .border_color(rgba(COLOR_WARNING))
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
        .border_1()
        .border_color(rgba(COLOR_BORDER))
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
        .border_1()
        .border_color(rgba(COLOR_BORDER))
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
        .border_1()
        .border_color(rgba(COLOR_BORDER))
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
        .border_1()
        .border_color(rgba(palette.border.rgba_hex()))
        .bg(rgba(palette.background.rgba_hex()))
        .px_3()
        .py_2()
        .text_xs()
        .line_height(px(18.0))
        .text_color(rgba(palette.foreground.rgba_hex()))
        .child(message)
        .into_any_element()
}

fn unix_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
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
            .border_1()
            .border_color(rgba(COLOR_BORDER))
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
                .network_name
                .as_deref()
                .unwrap_or(navigation.page_id.as_str())
                .to_string();
            card = card.child(
                button(
                    SharedString::from(format!("row-navigation-{}-{identity}", navigation.page_id)),
                    navigation.label.clone(),
                    ButtonVariant::Secondary,
                    false,
                )
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.navigate_to_row(&navigation, cx);
                })),
            );
        }

        card.into_any_element()
    }
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
        "leave_network" => "action.leave_network",
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
        NodeUiInputKind::Fingerprint => t(locale, "placeholder.fingerprint").into(),
        NodeUiInputKind::Purpose => t(locale, "placeholder.network_purpose").into(),
        NodeUiInputKind::Icon => "network".into(),
        NodeUiInputKind::Language => "zh-CN".into(),
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
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::discovery_index::{
        NodeDiscoveryIndexError, NodeDiscoveryIndexErrorCategory, NodeDiscoveryIndexRecordSnapshot,
        NodeDiscoveryIndexSnapshot, NodeDiscoveryIndexState,
    };
    use crate::gpui::view_model::{ActionBlockReason, SearchResultKind};
    use crate::network_discovery::DISCOVERY_RESULT_STALE_AFTER_MS;
    use crate::node::{
        NodeCommandFuture, NodeCommandHandler, NodeCommandRequest, NodeCommandResult, NodeEventBus,
        NodeUiValidation,
    };
    use crate::tun::TunCapabilityReason;
    use fuckipv6_common::crypto::Identity;
    use fuckipv6_common::domain::{
        DiscoveryTransport, NetworkDiscoveryDocument, NetworkDiscoveryRecord, NetworkEndpoint,
        NetworkId, Signature,
    };
    use gpui::{OwnedMenuItem, TestAppContext, VisualTestContext};
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
        ] {
            assert!(
                visible.debug_bounds(selector).is_some(),
                "missing fixed diagnostic layer {selector}"
            );
        }
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
        let window = open_shell_window(cx, 1200.0, 1_000.0);
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
                assert_eq!(shell.pending_commands, 0);
                shell.confirm_join_preview(cx);
                assert_eq!(shell.pending_commands, 0);
                assert_eq!(
                    shell.error.as_deref(),
                    Some("Confirm the join terms first.")
                );
            })
            .expect("install join preview fixture");

        let mut visible = VisualTestContext::from_window(window.into(), cx);
        visible.run_until_parked();
        for selector in [
            "visual-join-network-preview",
            "visual-join-network-terms",
            "visual-join-network-confirm",
        ] {
            assert!(
                visible.debug_bounds(selector).is_some(),
                "missing join preview element {selector}"
            );
        }
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
    }

    impl NodeCommandHandler for SnapshotCommandHandler {
        fn handle(&self, request: NodeCommandRequest) -> NodeCommandFuture {
            let state = self.state.clone();
            let configs = self.configs.clone();
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
                    command => panic!("unexpected GPUI test command: {command:?}"),
                }
            })
        }
    }

    fn open_shell_window(
        cx: &mut TestAppContext,
        width: f32,
        height: f32,
    ) -> gpui::WindowHandle<GpuiShell> {
        static NEXT_CONFIG_DIR: AtomicU64 = AtomicU64::new(1);

        let state = NodeStateStore::new();
        // GPUI tests do not install a Tokio runtime, while the production
        // command bus dispatches through Tokio. Rendering the initial shell
        // only needs the bus to exist; async command behavior is covered by
        // the node command tests.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("temporary Tokio runtime");
        let runtime_guard = runtime.enter();
        let config_dir = std::env::temp_dir().join(format!(
            "fuckipv6-gpui-test-{}-{}",
            std::process::id(),
            NEXT_CONFIG_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        let configs = ConfigManager::new_for_dir(config_dir).expect("temporary config directory");
        let bus = NodeCommandBus::spawn(
            Arc::new(SnapshotCommandHandler {
                state: state.clone(),
                configs: configs.clone(),
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
    fn all_manifest_actions_have_a_localized_label() {
        for action in crate::node::node_ui_manifest()
            .pages
            .iter()
            .flat_map(|page| &page.actions)
        {
            assert_ne!(action_label(Locale::ZhCn, &action.id), "未知动作");
        }
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
                        | NodeUiInputKind::NetworkSelection => {}
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
        cx.update(|app| {
            window
                .update(app, |shell, window, cx| {
                    shell.selected_page = "overview".into();
                    shell.selected_network = Some("home".into());
                    shell.notice = None;
                    shell.error = None;
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
            let overview = model.overview.as_ref().expect("overview screenshot model");
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
                "primary_status": {
                    "label": t(shell.locale, overview.primary_status.label_key()).to_string(),
                    "detail": t(shell.locale, overview.primary_status.detail_key()).to_string(),
                },
                "page_heading": t(shell.locale, &page.title_key).to_string(),
                "action_panel": t(shell.locale, "common.shared_actions").to_string(),
                "actions": page.actions
                    .iter()
                    .filter(|action| !is_configuration_action(&action.id))
                    .map(|action| action_label(shell.locale, &action.id))
                    .collect::<Vec<_>>(),
                "shortcuts": overview.shortcuts
                    .iter()
                    .map(|shortcut| shortcut.label.clone())
                    .collect::<Vec<_>>(),
                "network_statuses": overview.networks
                    .iter()
                    .map(|network| t(shell.locale, network_status_key(network.status)).to_string())
                    .collect::<Vec<_>>(),
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
