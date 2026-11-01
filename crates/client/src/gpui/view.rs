use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use fuckipv6_common::design_tokens::{DARK_THEME, DIMENSIONS};
use fuckipv6_common::i18n::{
    detect_system_locale, resolve_language, t, tr, LanguagePreference, Locale,
};
use fuckipv6_common::ui_components::SemanticState;
use fuckipv6_common::ui_layout::{
    navigation_module_for_page, navigation_modules, ActionPanelPresentation, AppRegion,
    LayoutClass, NavigationModule,
};
use gpui::{
    actions, div, prelude::*, px, rgba, size, App, Bounds, Context, Div, Entity, FocusHandle,
    KeyBinding, Menu, MenuItem, Render, Role, SharedString, Window, WindowBounds, WindowOptions,
};

use super::components::{
    button, choice_control, error_state as component_error_state, nav_tab, semantic_palette, tabs,
    toast as component_toast, ButtonVariant,
};
use super::text_input::{NativeTextInput, TextInputEvent};
use super::view_model::{
    action_block_reason, build_command, build_page_model, default_form_values, network_status_key,
    validate_manifest_bindings, ActionBinding, OverviewModel, PageKind, RowModel, SectionModel,
};
use crate::config_manager::ConfigManager;
use crate::node::{
    NodeCommand, NodeCommandBus, NodeCommandBusError, NodeCommandContext, NodeCommandOutcome,
    NodeCommandRequest, NodeCommandResult, NodeNetworkStatus, NodePrimaryStatus, NodeSnapshot,
    NodeStateStore, NodeUiAction, NodeUiBackendStatus, NodeUiField, NodeUiInputKind,
    NodeUiManifest, NodeUiPage,
};

const WINDOW_WIDTH: f32 = DIMENSIONS.window_default_width_px as f32;
const WINDOW_HEIGHT: f32 = DIMENSIONS.window_default_height_px as f32;

const COLOR_BG: u32 = DARK_THEME.colors.background.rgba_hex();
const COLOR_SIDEBAR: u32 = DARK_THEME.colors.background_soft.rgba_hex();
const COLOR_PANEL: u32 = DARK_THEME.colors.panel_solid.rgba_hex();
const COLOR_PANEL_RAISED: u32 = DARK_THEME.colors.panel_raised.rgba_hex();
const COLOR_BORDER: u32 = DARK_THEME.colors.border.rgba_hex();
const COLOR_TEXT: u32 = DARK_THEME.colors.text.rgba_hex();
const COLOR_MUTED: u32 = DARK_THEME.colors.text_muted.rgba_hex();
const COLOR_ACCENT: u32 = DARK_THEME.colors.accent.rgba_hex();
const COLOR_WARNING: u32 = DARK_THEME.colors.warning.rgba_hex();

actions!(fuckipv6_shell, [RefreshSnapshot, Quit]);

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

struct GpuiShell {
    commands: NodeCommandBus,
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
    focus_handle: FocusHandle,
    pending_commands: u32,
    next_request_id: u64,
    error: Option<String>,
    notice: Option<String>,
    language_menu_open: bool,
    network_menu_open: bool,
}

impl GpuiShell {
    fn new(
        state: NodeStateStore,
        commands: NodeCommandBus,
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
            focus_handle: cx.focus_handle(),
            pending_commands: 0,
            next_request_id: 1,
            error: None,
            notice: None,
            language_menu_open: false,
            network_menu_open: false,
        };

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
                self.notice = Some(tr(
                    self.locale,
                    "gpui.confirm_action",
                    &[
                        ("title", t(self.locale, &confirmation.title_key)),
                        ("consequence", t(self.locale, &confirmation.consequence_key)),
                    ],
                ));
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

    fn apply_command_result(
        &mut self,
        result: Result<NodeCommandResult, NodeCommandBusError>,
        cx: &mut Context<Self>,
    ) {
        self.pending_confirmation = None;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                self.error = Some(tr(
                    self.locale,
                    "gpui.command_channel_error",
                    &[("error", &error.to_string())],
                ));
                return;
            }
        };

        match result.outcome {
            NodeCommandOutcome::Snapshot(snapshot) => {
                self.apply_snapshot(*snapshot, cx);
                self.notice = Some(t(self.locale, "gpui.snapshot_refreshed").to_string());
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
                let network = problem
                    .network_name
                    .as_deref()
                    .map(|network| format!(" · {network}"))
                    .unwrap_or_default();
                self.error = Some(format!(
                    "{}{} ({:?}, retryable={})",
                    t(self.locale, problem.code.message_key()),
                    network,
                    problem.code,
                    problem.retryable
                ));
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
            NodeCommandOutcome::NetworkCreated(created) => {
                let key = if created.started {
                    "gpui.network_created_started"
                } else {
                    "gpui.network_created_not_started"
                };
                self.notice = Some(tr(
                    self.locale,
                    key,
                    &[
                        ("network", &created.network_name),
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
            for (field, value) in default_form_values(action, self.selected_network.as_deref()) {
                defaults.insert(self.field_key(&action.id, &field), value);
            }
            for field in &action.fields {
                if !matches!(
                    field.input,
                    NodeUiInputKind::Boolean
                        | NodeUiInputKind::Role
                        | NodeUiInputKind::RoutingMode
                        | NodeUiInputKind::NetworkSelection
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
                default_form_values(action, Some(&selected))
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
            button(
                SharedString::from("language-menu-trigger"),
                format!("{selected_label} ▾"),
                ButtonVariant::Secondary,
                false,
            )
            .aria_label(t(self.locale, "common.language").to_string())
            .h(px(34.0))
            .px_3()
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
                    .text_color(rgba(DARK_THEME.colors.text_faint.rgba_hex()))
                    .child(t(self.locale, "common.current_network").to_string()),
            )
            .child(
                button(
                    SharedString::from("network-picker-trigger"),
                    format!("{selected_label} ▾"),
                    ButtonVariant::Secondary,
                    disabled,
                )
                .aria_label(t(self.locale, "common.current_network").to_string())
                .h(px(34.0))
                .min_w(px(132.0))
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
                            shell.selected_network = Some(network_name.clone());
                            shell.network_menu_open = false;
                            shell.fill_empty_network_fields();
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
                    "⟳",
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
                .on_click(cx.listener(|shell, _, _, cx| {
                    if shell.pending_commands == 0 {
                        shell.submit_raw(NodeCommand::GetSnapshot, cx);
                    }
                })),
            )
            .child(
                button(
                    SharedString::from("quit-application"),
                    "↪",
                    ButtonVariant::Secondary,
                    false,
                )
                .aria_label(t(self.locale, "common.quit").to_string())
                .w(px(34.0))
                .h(px(34.0))
                .px_0()
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
                let icon = match module.id.as_str() {
                    "overview" => "◎",
                    "nodes" => "▦",
                    "networks" => "◇",
                    "activity" => "∿",
                    "settings" => "⚙",
                    _ => "·",
                };
                nav_tab(
                    SharedString::from(format!("nav-module-{}", module.id)),
                    format!("{icon}  {}", t(self.locale, &module.label_key)),
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
                            .border_color(rgba(DARK_THEME.colors.accent.rgba_hex()))
                            .text_size(px(20.0))
                            .text_color(rgba(COLOR_ACCENT))
                            .child("⌁"),
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
            .child(tabs("primary-navigation").flex_1().p_2().children(nav))
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

        div()
            .id("local-toolbar")
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
            .children((self.pending_commands > 0).then(|| {
                sync_badge(
                    "snapshot-syncing",
                    t(self.locale, "common.syncing").to_string(),
                )
            }))
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

    fn overview_content(
        &self,
        overview: &OverviewModel,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let palette = semantic_palette(primary_status_semantic(overview.primary_status));
        let mut primary_card = div()
            .id("overview-primary-status")
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
            .rounded_lg()
            .bg(rgba(palette.background.rgba_hex()))
            .border_1()
            .border_color(rgba(palette.border.rgba_hex()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgba(palette.foreground.rgba_hex()))
                            .child(t(self.locale, "common.primary_status").to_string()),
                    )
                    .child(
                        div()
                            .text_xl()
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(COLOR_TEXT))
                            .child(overview.node_name.clone()),
                    )
                    .child(div().text_sm().text_color(rgba(COLOR_MUTED)).child(format!(
                        "{} · {}",
                        t(self.locale, overview.primary_status.label_key()),
                        t(self.locale, overview.primary_status.detail_key())
                    ))),
            );

        if let Some(action_id) = overview.primary_action.clone() {
            let target = action_id.clone();
            primary_card = primary_card.child(
                button(
                    SharedString::from("overview-primary-action"),
                    action_label(self.locale, &action_id),
                    ButtonVariant::Primary,
                    false,
                )
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.open_action("overview", &target, cx);
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
                div()
                    .id("overview-issues")
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_4()
                    .rounded_lg()
                    .bg(rgba(DARK_THEME.colors.warning_soft.rgba_hex()))
                    .border_1()
                    .border_color(rgba(DARK_THEME.colors.warning.rgba_hex()))
                    .child(
                        div()
                            .text_base()
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(rgba(COLOR_TEXT))
                            .child(t(self.locale, "overview.need_attention").to_string()),
                    )
                    .children(overview.issues.iter().map(|issue| {
                        div()
                            .flex()
                            .gap_2()
                            .border_t_1()
                            .border_color(rgba(COLOR_BORDER))
                            .pt_2()
                            .text_sm()
                            .text_color(rgba(COLOR_WARNING))
                            .child("!")
                            .child(issue.clone())
                    })),
            );
        }

        let network_rows = if overview.networks.is_empty() {
            vec![div()
                .text_sm()
                .text_color(rgba(COLOR_MUTED))
                .child(t(self.locale, "common.no_running_networks").to_string())
                .into_any_element()]
        } else {
            overview
                .networks
                .iter()
                .map(|network| {
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
                    div()
                        .id(SharedString::from(format!(
                            "overview-network-{}",
                            network.name
                        )))
                        .flex()
                        .items_center()
                        .gap_3()
                        .py_2()
                        .border_t_1()
                        .border_color(rgba(COLOR_BORDER))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w(px(0.0))
                                .gap_1()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(gpui::FontWeight(600.0))
                                        .text_color(rgba(COLOR_TEXT))
                                        .child(network.name.clone()),
                                )
                                .child(div().text_xs().text_color(rgba(COLOR_MUTED)).child(tr(
                                    self.locale,
                                    "overview.network.detail",
                                    &[
                                        ("peers", &network.peer_count.to_string()),
                                        ("tun", &network.tun_name),
                                    ],
                                )))
                                .child(div().text_xs().text_color(rgba(COLOR_MUTED)).child(tr(
                                    self.locale,
                                    "overview.network.path",
                                    &[("routing", &network.routing), ("exit", &network.exit)],
                                )))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgba(COLOR_MUTED))
                                        .child(network.roles.join(" · ")),
                                ),
                        )
                        .child(
                            div()
                                .rounded_full()
                                .px_2()
                                .py_1()
                                .text_xs()
                                .text_color(rgba(network_palette.foreground.rgba_hex()))
                                .bg(rgba(network_palette.background.rgba_hex()))
                                .child(
                                    t(self.locale, network_status_key(network.status)).to_string(),
                                ),
                        )
                        .into_any_element()
                })
                .collect::<Vec<_>>()
        };
        stack = stack.child(
            div()
                .id("overview-networks")
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
                        .flex()
                        .justify_between()
                        .text_base()
                        .text_color(rgba(COLOR_TEXT))
                        .child(t(self.locale, "overview.networks_title").to_string())
                        .child(overview.networks.len().to_string()),
                )
                .children(network_rows),
        );

        let activity_rows = if overview.activity.is_empty() {
            vec![div()
                .text_sm()
                .text_color(rgba(COLOR_MUTED))
                .child(t(self.locale, "overview.activity.empty").to_string())
                .into_any_element()]
        } else {
            overview
                .activity
                .iter()
                .map(|activity| {
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .py_2()
                        .border_t_1()
                        .border_color(rgba(COLOR_BORDER))
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgba(COLOR_TEXT))
                                .child(activity.action.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgba(COLOR_MUTED))
                                .child(activity.detail.clone()),
                        )
                        .into_any_element()
                })
                .collect::<Vec<_>>()
        };
        stack = stack.child(
            div()
                .id("overview-activity")
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
                        .child(t(self.locale, "overview.activity_title").to_string()),
                )
                .children(activity_rows),
        );

        let shortcut_cards = overview
            .shortcuts
            .iter()
            .map(|shortcut| {
                let mut card = div()
                    .id(SharedString::from(format!(
                        "overview-shortcut-{}",
                        shortcut.id
                    )))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .min_h(px(52.0))
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgba(COLOR_BORDER))
                    .bg(rgba(COLOR_PANEL_RAISED))
                    .text_sm()
                    .text_color(rgba(if shortcut.action_id.is_some() {
                        COLOR_TEXT
                    } else {
                        COLOR_MUTED
                    }))
                    .child(shortcut.label.clone());
                if let Some(reason) = &shortcut.unavailable_reason {
                    card = card.child(
                        div()
                            .text_xs()
                            .text_color(rgba(COLOR_MUTED))
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
            div()
                .id("overview-shortcuts")
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
                        .child(t(self.locale, "overview.shortcuts").to_string()),
                )
                .child(div().flex().gap_2().children(shortcut_cards)),
        );

        stack.into_any_element()
    }

    fn page_content(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(manifest) = self.manifest.as_ref() else {
            return div()
                .id(app_region_id(AppRegion::Content))
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
        let page_icon = page_icon(&self.selected_page);

        let mut content = div()
            .id(app_region_id(AppRegion::Content))
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
                    .role(A11Y_HEADING_ROLE)
                    .aria_level(1)
                    .aria_label(page_title.clone())
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .w(px(40.0))
                            .h(px(40.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .bg(rgba(COLOR_PANEL))
                            .border_1()
                            .border_color(rgba(COLOR_BORDER))
                            .text_color(rgba(COLOR_ACCENT))
                            .child(page_icon),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .text_size(px(20.0))
                                    .font_weight(gpui::FontWeight(600.0))
                                    .text_color(rgba(COLOR_TEXT))
                                    .child(page_title),
                            )
                            .child(div().text_xs().text_color(rgba(COLOR_MUTED)).child(
                                t(self.locale, "common.node_contract_description").to_string(),
                            )),
                    ),
            );

        if let Some(overview) = model.overview.as_ref() {
            return content
                .child(self.overview_content(overview, cx))
                .into_any_element();
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
        content = content.child(
            div()
                .flex()
                .flex_col()
                .gap_4()
                .children(model.sections.iter().map(section_card)),
        );

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
            card = card.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .px_4()
                    .pb_4()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgba(COLOR_MUTED))
                            .child(format!("{} · {}", action.id, status)),
                    )
                    .children(fields)
                    .children(block_reason.map(|reason| {
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
                    }))
                    .child(
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
                    ),
            );
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

    fn action_panel(&self, layout: LayoutClass, cx: &mut Context<Self>) -> impl IntoElement {
        let presentation = ActionPanelPresentation::for_layout(layout);
        let actions = self
            .page()
            .cloned()
            .map(|page| page.actions)
            .unwrap_or_default();
        let action_cards = actions
            .into_iter()
            .map(|action| self.action_card(action, cx))
            .collect::<Vec<_>>();
        let body = if action_cards.is_empty() {
            div()
                .id("no-shared-actions")
                .p_4()
                .text_sm()
                .text_color(rgba(COLOR_MUTED))
                .child(t(self.locale, "common.page_actions_empty").to_string())
                .into_any_element()
        } else {
            div()
                .id("action-panel-body")
                .flex()
                .flex_col()
                .overflow_y_scroll()
                .children(action_cards)
                .into_any_element()
        };

        let mut panel = div()
            .id(app_region_id(AppRegion::ActionPanel))
            .role(Role::Complementary)
            .aria_label(t(self.locale, "common.shared_actions").to_string())
            .flex()
            .flex_col()
            .min_w(px(0.0))
            .overflow_hidden()
            .rounded_lg()
            .bg(rgba(COLOR_PANEL))
            .border_1()
            .border_color(rgba(COLOR_BORDER))
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(38.0))
                    .px_4()
                    .border_b_1()
                    .border_color(rgba(COLOR_BORDER))
                    .text_xs()
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
        let content = self.page_content(cx);
        let action_panel = self.action_panel(layout, cx);
        let workspace = if presentation == ActionPanelPresentation::Stacked {
            div()
                .id("page-workspace")
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
    div()
        .id(id)
        .role(Role::Tab)
        .aria_label(label.clone())
        .aria_selected(selected)
        .focusable()
        .tab_stop(true)
        .flex()
        .items_center()
        .h(px(34.0))
        .px_3()
        .rounded_sm()
        .border_b_2()
        .border_color(rgba(if selected { COLOR_ACCENT } else { 0x00000000 }))
        .bg(rgba(if selected {
            COLOR_PANEL_RAISED
        } else {
            0x00000000
        }))
        .text_size(px(12.0))
        .font_weight(gpui::FontWeight(if selected { 600.0 } else { 400.0 }))
        .text_color(rgba(if selected { COLOR_TEXT } else { COLOR_MUTED }))
        .child(label)
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

fn page_icon(id: &str) -> &'static str {
    match PageKind::from_id(id) {
        Some(PageKind::Overview) => "◎",
        Some(PageKind::Networks) => "◇",
        Some(PageKind::Peers) => "⌘",
        Some(PageKind::Topology) => "◉",
        Some(PageKind::Traffic) => "∿",
        Some(PageKind::Routing) => "⇄",
        Some(PageKind::ExitNode) => "↗",
        Some(PageKind::Relay) => "⌁",
        Some(PageKind::Logs) => "☷",
        Some(PageKind::Identity) => "⚿",
        Some(PageKind::Settings) => "⚙",
        None => "·",
    }
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

fn section_card(section: &SectionModel) -> gpui::AnyElement {
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
        .children(if section.rows.is_empty() {
            vec![div()
                .text_sm()
                .text_color(rgba(COLOR_MUTED))
                .child(section.empty_text.clone())
                .into_any_element()]
        } else {
            section.rows.iter().map(row_card).collect()
        })
        .into_any_element()
}

fn row_card(row: &RowModel) -> gpui::AnyElement {
    div()
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
        )
        .into_any_element()
}

fn action_label(locale: Locale, id: &str) -> String {
    let key = match id {
        "refresh_snapshot" => "action.refresh_snapshot",
        "get_node_diagnostics" => "action.get_node_diagnostics",
        "create_network" => "action.create_network",
        "join_network" => "action.join_network",
        "start_networks" => "action.start_networks",
        "stop_networks" => "action.stop_networks",
        "restart_networks" => "action.restart_networks",
        "probe_peer" => "action.probe_peer",
        "get_blacklist" => "action.get_blacklist",
        "ban_client" => "action.ban_client",
        "unban_client" => "action.unban_client",
        "get_network_allocation" => "action.get_network_allocation",
        "update_network_allocation" => "action.update_network_allocation",
        "update_routing" => "action.update_routing",
        "set_exit_node_role" => "action.set_exit_node_role",
        "set_relay_role" => "action.set_relay_role",
        "start_node" => "action.start_node",
        "stop_node" => "action.stop_node",
        "restart_node" => "action.restart_node",
        "cleanup_node_residuals" => "action.cleanup_node_residuals",
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
        NodeUiInputKind::PeerTarget => t(locale, "placeholder.peer_target").into(),
        NodeUiInputKind::Port => "1-65535".into(),
        NodeUiInputKind::TimeoutMs => "1500".into(),
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
        KeyBinding::new("ctrl-q", Quit, None),
        KeyBinding::new("cmd-q", Quit, None),
    ]);
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
    configs: ConfigManager,
) -> Result<()> {
    let open_error = Arc::new(Mutex::new(None::<String>));
    let error_slot = open_error.clone();

    gpui_platform::application().run(move |cx: &mut App| {
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
                cx.new(|cx| GpuiShell::new(state.clone(), commands.clone(), configs.clone(), cx))
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
    use crate::gpui::view_model::ActionBlockReason;
    use crate::node::{
        NodeCommandFuture, NodeCommandHandler, NodeCommandRequest, NodeCommandResult, NodeEventBus,
        NodeUiValidation,
    };
    use gpui::{OwnedMenuItem, TestAppContext, VisualTestContext};

    const PRODUCT_VISUAL_SNAPSHOT_FIXTURE: &str =
        include_str!("../../../../contracts/typescript/fixtures/product-visual-snapshot.json");

    #[derive(serde::Deserialize)]
    struct ProductVisualFixture {
        snapshot: NodeSnapshot,
    }

    struct SnapshotCommandHandler {
        state: NodeStateStore,
    }

    impl NodeCommandHandler for SnapshotCommandHandler {
        fn handle(&self, request: NodeCommandRequest) -> NodeCommandFuture {
            let state = self.state.clone();
            Box::pin(async move {
                let request_id = request.request_id;
                match request.command {
                    NodeCommand::GetSnapshot => {
                        NodeCommandResult::snapshot(request_id, state.snapshot().await)
                    }
                    NodeCommand::GetUiManifest => {
                        NodeCommandResult::ui_manifest(request_id, crate::node::node_ui_manifest())
                    }
                    command => panic!("unexpected GPUI test command: {command:?}"),
                }
            })
        }
    }

    fn open_shell_window(cx: &mut TestAppContext, width: f32) -> gpui::WindowHandle<GpuiShell> {
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
        let bus = NodeCommandBus::spawn(
            Arc::new(SnapshotCommandHandler {
                state: state.clone(),
            }),
            NodeEventBus::default(),
            4,
        );
        drop(runtime_guard);
        drop(runtime);
        let config_dir = std::env::temp_dir().join(format!(
            "fuckipv6-gpui-test-{}-{}",
            std::process::id(),
            NEXT_CONFIG_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        let configs = ConfigManager::new_for_dir(config_dir).expect("temporary config directory");
        let window = cx.open_window(size(px(width), px(WINDOW_HEIGHT)), move |_, cx| {
            GpuiShell::new(state, bus, configs, cx)
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
            assert_ne!(page_icon(&page.id), "·");
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

    #[gpui::test]
    fn shell_keymap_binds_refresh_and_quit(cx: &mut TestAppContext) {
        cx.update(bind_shell_keys);
        assert_eq!(
            bound_keystrokes(cx, &RefreshSnapshot),
            [
                (true, false, "r".to_string()),
                (false, true, "r".to_string())
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
        let window = open_shell_window(cx, 1280.0);
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
        let bus = NodeCommandBus::spawn(
            Arc::new(SnapshotCommandHandler {
                state: state.clone(),
            }),
            NodeEventBus::default(),
            4,
        );
        let config_dir =
            std::env::temp_dir().join(format!("fuckipv6-gpui-visual-{}", std::process::id()));
        let configs = ConfigManager::new_for_dir(config_dir).expect("temporary config directory");
        configs
            .set_node_language_preference(LanguagePreference::En)
            .expect("fixture language preference");
        let text_system = Arc::new(gpui_wgpu::CosmicTextSystem::new("Inter"));
        let mut cx = gpui::HeadlessAppContext::with_platform(
            text_system,
            Arc::new(()),
            gpui_platform::current_headless_renderer,
        );
        let window = cx
            .open_window(size(px(1440.0), px(900.0)), move |_, cx| {
                cx.new(|cx| GpuiShell::new(state, bus, configs, cx))
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
                .update(app, |shell, _, cx| {
                    shell.notice = None;
                    cx.notify();
                })
                .expect("clear GPUI visual-test notice");
        });
        cx.run_until_parked();
        let image = cx
            .capture_screenshot(window.into())
            .expect("GPUI headless screenshot");
        let output = std::path::PathBuf::from(output);
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent).expect("screenshot directory");
        }
        image.save(&output).expect("write GPUI screenshot");
    }

    #[gpui::test]
    fn high_dpi_preserves_logical_window_width_and_breakpoints(cx: &mut TestAppContext) {
        let window = open_shell_window(cx, 900.0);
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
