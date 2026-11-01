use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use fuckipv6_common::design_tokens::{DARK_THEME, DIMENSIONS};
use fuckipv6_common::domain::{CapabilityKind, CapabilityState};
use fuckipv6_common::i18n::{
    detect_system_locale, resolve_language, t, tr, LanguagePreference, Locale,
};
use fuckipv6_common::ui_components::{Capability, SemanticState};
use fuckipv6_common::ui_layout::{DetailPresentation, LayoutClass, PageSection};
use gpui::{
    div, prelude::*, px, rgba, size, App, Application, Bounds, Context, Div, FocusHandle,
    KeyDownEvent, Render, SharedString, Window, WindowBounds, WindowOptions,
};

use super::components::{
    button, capability_ring, choice_control, empty_state as component_empty_state,
    error_state as component_error_state, nav_tab, status_badge, tabs, text_input,
    toast as component_toast, toolbar, ButtonVariant, CapabilityItem, CapabilityVisual,
};
use super::view_model::{
    action_block_reason, build_command, build_page_model, default_form_values,
    validate_manifest_bindings, ActionBinding, PageKind, RowModel, SectionModel,
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

struct GpuiShell {
    commands: NodeCommandBus,
    configs: ConfigManager,
    language_preference: LanguagePreference,
    locale: Locale,
    snapshot: NodeSnapshot,
    manifest: Option<NodeUiManifest>,
    selected_page: String,
    selected_section: PageSection,
    selected_network: Option<String>,
    focused_field: Option<String>,
    pending_confirmation: Option<String>,
    form_values: HashMap<String, String>,
    focus_handle: FocusHandle,
    pending_commands: u32,
    next_request_id: u64,
    error: Option<String>,
    notice: Option<String>,
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
            selected_section: PageSection::DEFAULT,
            selected_network,
            focused_field: None,
            pending_confirmation: None,
            form_values: HashMap::new(),
            focus_handle: cx.focus_handle(),
            pending_commands: 0,
            next_request_id: 1,
            error: None,
            notice: None,
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
                    shell.apply_snapshot(snapshot);
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
        if let Err(error) = self.configs.set_node_language_preference(preference) {
            self.error = Some(tr(
                self.locale,
                "gpui.operation_failed",
                &[("error", &error.to_string())],
            ));
        }
        cx.notify();
    }

    fn apply_snapshot(&mut self, snapshot: NodeSnapshot) {
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
                shell.apply_command_result(result);
                cx.notify();
            });
        })
        .detach();
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

    fn apply_command_result(&mut self, result: Result<NodeCommandResult, NodeCommandBusError>) {
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
                self.apply_snapshot(*snapshot);
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
                        self.selected_section = PageSection::DEFAULT;
                    }
                }
                self.manifest = Some(manifest);
                self.initialize_forms();
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

    fn initialize_forms(&mut self) {
        let Some(manifest) = &self.manifest else {
            return;
        };
        let mut defaults = HashMap::new();
        for action in manifest.pages.iter().flat_map(|page| &page.actions) {
            for (field, value) in default_form_values(action, self.selected_network.as_deref()) {
                defaults.insert(self.field_key(&action.id, &field), value);
            }
        }
        for (key, value) in defaults {
            self.form_values.entry(key).or_insert(value);
        }
        self.fill_empty_network_fields();
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

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(field_key) = self.focused_field.clone() else {
            return;
        };
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control || keystroke.modifiers.alt || keystroke.modifiers.platform {
            return;
        }

        match keystroke.key.as_str() {
            "escape" => {
                self.focused_field = None;
                cx.stop_propagation();
            }
            "backspace" => {
                if let Some(value) = self.form_values.get_mut(&field_key) {
                    value.pop();
                }
                cx.stop_propagation();
                cx.notify();
            }
            "delete" => {
                if let Some(value) = self.form_values.get_mut(&field_key) {
                    value.clear();
                }
                cx.stop_propagation();
                cx.notify();
            }
            "enter" => {
                self.focused_field = None;
                cx.stop_propagation();
                cx.notify();
            }
            _ => {
                if let Some(key_char) = &keystroke.key_char {
                    if !key_char.is_empty() {
                        self.form_values
                            .entry(field_key)
                            .or_default()
                            .push_str(key_char);
                        cx.stop_propagation();
                        cx.notify();
                    }
                }
            }
        }
    }

    fn page(&self) -> Option<&NodeUiPage> {
        self.manifest
            .as_ref()?
            .pages
            .iter()
            .find(|page| page.id == self.selected_page)
    }

    fn language_selector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .text_color(rgba(COLOR_MUTED))
                    .child(t(self.locale, "common.language").to_string()),
            )
            .children(LanguagePreference::ALL.into_iter().map(|preference| {
                choice_control(
                    SharedString::from(format!("language-{}", preference.storage_value())),
                    t(self.locale, preference.label_key()).to_string(),
                    self.language_preference == preference,
                )
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.set_language(preference, cx);
                }))
                .into_any_element()
            }))
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let title = page_title(self.locale, &self.selected_page);
        let subtitle = self
            .page()
            .map(|page| {
                tr(
                    self.locale,
                    "gpui.page_action_count",
                    &[
                        ("page", &page.id),
                        ("count", &page.actions.len().to_string()),
                    ],
                )
            })
            .unwrap_or_else(|| t(self.locale, "common.waiting_manifest").to_string());
        let disabled = self.pending_commands > 0;
        let primary_status = self.snapshot.primary_status();

        div()
            .flex()
            .items_center()
            .justify_between()
            .px_6()
            .py_4()
            .border_b_1()
            .border_color(rgba(COLOR_BORDER))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_2xl().text_color(rgba(COLOR_TEXT)).child(title))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgba(COLOR_MUTED))
                            .child(subtitle),
                    ),
            )
            .child(
                toolbar()
                    .child(status_badge(
                        t(self.locale, primary_status.label_key()).to_string(),
                        primary_status_semantic(primary_status),
                    ))
                    .child(div().text_sm().text_color(rgba(COLOR_MUTED)).child(format!(
                        "{} · {} {}",
                        self.selected_network
                            .as_deref()
                            .unwrap_or(t(self.locale, "common.not_selected")),
                        t(self.locale, "common.revision"),
                        self.snapshot.revision
                    )))
                    .child(self.language_selector(cx))
                    .child(
                        button(
                            SharedString::from("refresh-snapshot"),
                            if disabled {
                                t(self.locale, "common.operation_in_progress")
                            } else {
                                t(self.locale, "common.refresh")
                            },
                            ButtonVariant::Primary,
                            disabled,
                        )
                        .on_click(cx.listener(|shell, _, _, cx| {
                            if shell.pending_commands == 0 {
                                shell.submit_raw(NodeCommand::GetSnapshot, cx);
                            }
                        })),
                    ),
            )
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pages = self
            .manifest
            .as_ref()
            .map(|manifest| manifest.pages.clone())
            .unwrap_or_default();
        let nav = pages
            .into_iter()
            .map(|page| {
                let page_id = page.id.clone();
                let selected = page.id == self.selected_page;
                nav_tab(
                    SharedString::from(format!("nav-{}", page.id)),
                    page_label(self.locale, &page.id),
                    selected,
                )
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.selected_page = page_id.clone();
                    shell.selected_section = PageSection::DEFAULT;
                    shell.focused_field = None;
                    shell.pending_confirmation = None;
                    shell.error = None;
                    shell.notice = None;
                    cx.notify();
                }))
                .into_any_element()
            })
            .collect::<Vec<_>>();

        let network_chips = self
            .snapshot
            .networks
            .iter()
            .map(|network| {
                let network_name = network.name.clone();
                let selected = self.selected_network.as_deref() == Some(network.name.as_str());
                choice_control(
                    SharedString::from(format!("network-{}", network.name)),
                    network.name.clone(),
                    selected,
                )
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.selected_network = Some(network_name.clone());
                    shell.fill_empty_network_fields();
                    cx.notify();
                }))
                .into_any_element()
            })
            .collect::<Vec<_>>();

        let capability_items = self
            .snapshot
            .capabilities
            .capabilities
            .iter()
            .map(|item| CapabilityItem {
                capability: capability_contract(item.kind),
                visual: capability_visual(item.state),
                label: capability_label(self.locale, item.kind, item.state),
            })
            .collect::<Vec<_>>();

        let manifest_state = match &self.manifest {
            Some(manifest) => format!(
                "manifest v{} · {} pages",
                manifest.schema_version,
                manifest.pages.len()
            ),
            None => t(self.locale, "common.manifest_not_loaded").to_string(),
        };

        div()
            .flex()
            .flex_col()
            .w(px(DIMENSIONS.sidebar_px as f32))
            .h_full()
            .bg(rgba(COLOR_SIDEBAR))
            .border_r_1()
            .border_color(rgba(COLOR_BORDER))
            .child(
                div()
                    .px_6()
                    .pt_6()
                    .pb_5()
                    .child(
                        div()
                            .text_xl()
                            .text_color(rgba(COLOR_ACCENT))
                            .child("fuckipv6"),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_xs()
                            .text_color(rgba(COLOR_MUTED))
                            .child(t(self.locale, "gpui.subtitle").to_string()),
                    ),
            )
            .child(tabs().flex_1().children(nav))
            .child(
                div()
                    .mx_3()
                    .p_3()
                    .rounded_md()
                    .bg(rgba(COLOR_PANEL))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgba(COLOR_MUTED))
                            .child(t(self.locale, "common.current_network").to_string()),
                    )
                    .child(div().mt_2().flex().flex_wrap().gap_1().children(
                        if network_chips.is_empty() {
                            vec![div()
                                .text_xs()
                                .text_color(rgba(COLOR_MUTED))
                                .child(t(self.locale, "common.no_running_networks").to_string())
                                .into_any_element()]
                        } else {
                            network_chips
                        },
                    )),
            )
            .child(
                div()
                    .mx_3()
                    .p_3()
                    .rounded_md()
                    .bg(rgba(COLOR_PANEL))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgba(COLOR_MUTED))
                            .child(t(self.locale, "common.node_capabilities").to_string()),
                    )
                    .child(div().mt_2().child(capability_ring(&capability_items))),
            )
            .child(
                div()
                    .m_3()
                    .p_3()
                    .rounded_md()
                    .bg(rgba(COLOR_PANEL))
                    .text_xs()
                    .text_color(rgba(COLOR_MUTED))
                    .child(manifest_state),
            )
    }

    fn error_banner(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let error = self.error.clone().unwrap_or_default();
        component_toast(
            tr(self.locale, "common.operation_failed", &[("error", &error)]),
            SemanticState::Danger,
        )
        .id("dismiss-error")
        .on_click(cx.listener(|shell, _, _, cx| {
            shell.error = None;
            cx.notify();
        }))
    }

    fn notice_banner(&self) -> impl IntoElement {
        component_toast(
            self.notice.clone().unwrap_or_default(),
            SemanticState::Success,
        )
    }

    fn local_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let action_count = self
            .page()
            .map(|page| page.actions.len())
            .unwrap_or_default();
        let tab = |section: PageSection, label: String| {
            let selected = self.selected_section == section;
            choice_control(
                SharedString::from(format!("page-section-{}", section.name())),
                label,
                selected,
            )
            .on_click(cx.listener(move |shell, _, _, cx| {
                shell.selected_section = section;
                shell.focused_field = None;
                shell.pending_confirmation = None;
                cx.notify();
            }))
            .into_any_element()
        };

        div()
            .flex()
            .items_center()
            .gap_2()
            .px_6()
            .pt_4()
            .child(tab(
                PageSection::Data,
                t(self.locale, "common.data").to_string(),
            ))
            .child(tab(
                PageSection::Actions,
                tr(
                    self.locale,
                    "gpui.section_action_count",
                    &[("count", &action_count.to_string())],
                ),
            ))
    }

    fn page_content(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(manifest) = self.manifest.as_ref() else {
            return loading_skeleton().into_any_element();
        };
        let model = match build_page_model(manifest, &self.snapshot, &self.selected_page) {
            Ok(model) => model,
            Err(error) => {
                return component_error_state(
                    t(self.locale, "common.page_mapping_failed"),
                    error.to_string(),
                )
                .into_any_element();
            }
        };
        let actions = if self.selected_section == PageSection::Actions {
            self.page()
                .cloned()
                .map(|page| page.actions)
                .unwrap_or_default()
                .into_iter()
                .map(|action| self.action_card(action, cx))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        let mut content = div()
            .id("page-scroll")
            .flex()
            .flex_col()
            .gap_4()
            .px_6()
            .pb_6()
            .pt_3()
            .overflow_y_scroll()
            .flex_1();

        match self.selected_section {
            PageSection::Data => {
                if !model.metrics.is_empty() {
                    content = content.child(div().flex().flex_wrap().gap_3().children(
                        model.metrics.iter().map(|metric| {
                            metric_card(&metric.label, &metric.value, &metric.source)
                        }),
                    ));
                }
                content = content.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .children(model.sections.iter().map(section_card)),
                );
            }
            PageSection::Actions => {
                content = content.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .text_lg()
                                .text_color(rgba(COLOR_TEXT))
                                .child(t(self.locale, "common.shared_actions").to_string()),
                        )
                        .children(if actions.is_empty() {
                            vec![component_empty_state(
                                t(self.locale, "common.no_shared_actions"),
                                t(self.locale, "common.page_no_actions"),
                            )
                            .into_any_element()]
                        } else {
                            actions
                        }),
                );
            }
        }

        content.into_any_element()
    }

    fn action_card(&self, action: NodeUiAction, cx: &mut Context<Self>) -> gpui::AnyElement {
        let action_id = action.id.clone();
        let block_reason =
            action_block_reason(&action, &self.snapshot, self.selected_network.as_deref());
        let confirmation_pending = self.pending_confirmation.as_deref() == Some(action.id.as_str());
        let disabled = self.pending_commands > 0 || block_reason.is_some();
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
        let availability = format!("{:?}", action.availability);
        let permission = format!("{:?}", action.permission);
        let button_label = if confirmation_pending {
            t(self.locale, "common.confirm_again")
        } else if action.confirmation.is_some() {
            t(self.locale, "common.ready_to_submit")
        } else {
            t(self.locale, "common.submit_command")
        };

        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_lg()
            .bg(rgba(COLOR_PANEL))
            .border_1()
            .border_color(rgba(if confirmation_pending {
                COLOR_WARNING
            } else {
                COLOR_BORDER
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_base()
                                    .text_color(rgba(COLOR_TEXT))
                                    .child(action_label(self.locale, &action.id)),
                            )
                            .child(div().text_xs().text_color(rgba(COLOR_MUTED)).child(format!(
                                "{} · {:?} · permission={} · availability={} · {}",
                                action.id, action.command, permission, availability, status
                            ))),
                    )
                    .child(status_badge(
                        status,
                        if action.backend_status == NodeUiBackendStatus::Implemented {
                            SemanticState::Success
                        } else {
                            SemanticState::Unsupported
                        },
                    )),
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
                .on_click(cx.listener(move |shell, _, _, cx| {
                    if shell.pending_commands == 0 && block_reason.is_none() {
                        shell.submit_manifest_action(&action_id, cx);
                    }
                })),
            )
            .into_any_element()
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
        value: &str,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let field_key = self.field_key(&action.id, &field.id);
        let field_key_for_click = field_key.clone();
        let focused = self.focused_field.as_deref() == Some(field_key.as_str());
        let placeholder = placeholder_for(self.locale, &field.input, &action.id, &field.id);
        let display = if value.is_empty() {
            placeholder.to_string()
        } else if focused {
            format!("{value}│")
        } else {
            value.to_string()
        };
        text_input(
            SharedString::from(format!("field-{field_key}")),
            display,
            value.is_empty(),
            focused,
        )
        .on_click(cx.listener(move |shell, _, window, cx| {
            shell.focused_field = Some(field_key_for_click.clone());
            window.focus(&shell.focus_handle);
            cx.notify();
        }))
        .into_any_element()
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

    fn detail_panel(&self) -> impl IntoElement {
        let status = self.snapshot.primary_status();
        let selected_network = self
            .selected_network
            .as_deref()
            .unwrap_or(t(self.locale, "common.not_selected"));

        div()
            .id("wide-detail")
            .flex()
            .flex_col()
            .gap_4()
            .w(px(DIMENSIONS.detail_default_px as f32))
            .h_full()
            .p_5()
            .bg(rgba(COLOR_SIDEBAR))
            .border_l_1()
            .border_color(rgba(COLOR_BORDER))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_lg()
                            .text_color(rgba(COLOR_TEXT))
                            .child(t(self.locale, "common.current_context").to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgba(COLOR_MUTED))
                            .child(t(self.locale, "gpui.context_hint").to_string()),
                    ),
            )
            .child(detail_row(
                t(self.locale, "common.page"),
                page_title(self.locale, &self.selected_page),
            ))
            .child(detail_row(
                t(self.locale, "common.current_network"),
                selected_network.to_string(),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgba(COLOR_MUTED))
                            .child(t(self.locale, "common.primary_status").to_string()),
                    )
                    .child(status_badge(
                        t(self.locale, status.label_key()).to_string(),
                        primary_status_semantic(status),
                    )),
            )
            .child(detail_row(
                t(self.locale, "common.revision"),
                self.snapshot.revision.to_string(),
            ))
    }
}

impl Render for GpuiShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let layout = layout_for_window_width(f32::from(window.viewport_size().width));
        let show_detail = DetailPresentation::for_layout(layout) == DetailPresentation::Persistent;
        let content = self.page_content(cx);
        let mut main = div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .bg(rgba(COLOR_BG))
            .child(self.header(cx));

        if self.error.is_some() {
            main = main.child(self.error_banner(cx));
        } else if self.notice.is_some() {
            main = main.child(self.notice_banner());
        }

        main = main.child(self.local_tabs(cx)).child(content).child(
            div()
                .px_6()
                .py_3()
                .border_t_1()
                .border_color(rgba(COLOR_BORDER))
                .text_xs()
                .text_color(rgba(COLOR_MUTED))
                .child(t(self.locale, "gpui.footer").to_string()),
        );

        let mut root = div()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .flex()
            .w_full()
            .h_full()
            .bg(rgba(COLOR_BG))
            .child(self.sidebar(cx))
            .child(main);

        if show_detail {
            root = root.child(self.detail_panel());
        }

        root
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

fn detail_row(label: &str, value: String) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .p_3()
        .rounded_md()
        .bg(rgba(COLOR_PANEL))
        .border_1()
        .border_color(rgba(COLOR_BORDER))
        .child(
            div()
                .text_xs()
                .text_color(rgba(COLOR_MUTED))
                .child(label.to_string()),
        )
        .child(div().text_sm().text_color(rgba(COLOR_TEXT)).child(value))
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

fn capability_contract(kind: CapabilityKind) -> Capability {
    match kind {
        CapabilityKind::Coordinator => Capability::Coordinator,
        CapabilityKind::Relay => Capability::Relay,
        CapabilityKind::Exit => Capability::Exit,
        CapabilityKind::File => Capability::File,
    }
}

fn capability_visual(state: CapabilityState) -> CapabilityVisual {
    match state {
        CapabilityState::Enabled => CapabilityVisual::Active,
        CapabilityState::Degraded | CapabilityState::Starting => CapabilityVisual::Degraded,
        CapabilityState::Unsupported => CapabilityVisual::Unsupported,
        CapabilityState::Disabled | CapabilityState::Unavailable | CapabilityState::Unknown => {
            CapabilityVisual::Inactive
        }
    }
}

fn capability_label(locale: Locale, kind: CapabilityKind, state: CapabilityState) -> String {
    let capability_key = match kind {
        CapabilityKind::Coordinator => "capability.coordinator",
        CapabilityKind::Relay => "capability.relay",
        CapabilityKind::Exit => "capability.exit",
        CapabilityKind::File => "capability.file",
    };
    let state_key = match state {
        CapabilityState::Enabled => "status.capability.enabled",
        CapabilityState::Degraded => "status.capability.degraded",
        CapabilityState::Starting => "status.capability.starting",
        CapabilityState::Unsupported => "status.capability.unsupported",
        CapabilityState::Disabled | CapabilityState::Unavailable | CapabilityState::Unknown => {
            "status.capability.unknown"
        }
    };
    format!("{} · {}", t(locale, capability_key), t(locale, state_key))
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

fn page_label(locale: Locale, id: &str) -> String {
    let key = match PageKind::from_id(id) {
        Some(PageKind::Overview) => "nav.overview",
        Some(PageKind::Networks) => "nav.networks",
        Some(PageKind::Peers) => "nav.peers",
        Some(PageKind::Topology) => "nav.topology",
        Some(PageKind::Traffic) => "nav.traffic",
        Some(PageKind::Routing) => "nav.routing",
        Some(PageKind::ExitNode) => "nav.exit_node",
        Some(PageKind::Relay) => "nav.relay",
        Some(PageKind::Logs) => "nav.logs",
        Some(PageKind::Identity) => "nav.identity",
        Some(PageKind::Settings) => "nav.settings",
        None => "common.unknown_page",
    };
    t(locale, key).to_string()
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

fn page_title(locale: Locale, id: &str) -> String {
    format!("{} · {}", page_label(locale, id), id)
}

pub fn run_window(
    state: NodeStateStore,
    commands: NodeCommandBus,
    configs: ConfigManager,
) -> Result<()> {
    let open_error = Arc::new(Mutex::new(None::<String>));
    let error_slot = open_error.clone();

    Application::new().run(move |cx: &mut App| {
        cx.on_window_closed(|cx| {
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
    use super::*;
    use crate::gpui::view_model::ActionBlockReason;
    use crate::node::NodeUiValidation;

    #[test]
    fn all_manifest_pages_have_a_navigation_label() {
        for page in crate::node::node_ui_manifest().pages {
            assert_ne!(page_label(Locale::ZhCn, &page.id), "未知页面");
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
    fn page_titles_keep_the_stable_manifest_id() {
        assert_eq!(
            page_title(Locale::ZhCn, "exit_node"),
            "Exit Node · exit_node"
        );
    }

    #[test]
    fn data_is_the_default_local_page_section() {
        assert_eq!(PageSection::DEFAULT, PageSection::Data);
    }

    #[test]
    fn wide_layout_persists_detail_at_the_shared_breakpoint() {
        assert_eq!(
            DetailPresentation::for_layout(layout_for_window_width(1279.0)),
            DetailPresentation::Hidden
        );
        assert_eq!(
            DetailPresentation::for_layout(layout_for_window_width(1280.0)),
            DetailPresentation::Persistent
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

    #[test]
    fn every_capability_state_has_a_textual_legend() {
        for kind in CapabilityKind::ALL {
            for state in [
                CapabilityState::Enabled,
                CapabilityState::Degraded,
                CapabilityState::Starting,
                CapabilityState::Unsupported,
                CapabilityState::Disabled,
                CapabilityState::Unavailable,
                CapabilityState::Unknown,
            ] {
                let label = capability_label(Locale::ZhCn, kind, state);
                assert!(label.contains(match kind {
                    CapabilityKind::Coordinator => "Coordinator",
                    CapabilityKind::Relay => "Relay",
                    CapabilityKind::Exit => "Exit",
                    CapabilityKind::File => "File",
                }));
            }
        }
    }
}
