use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use fuckipv6_common::design_tokens::{DARK_THEME, DIMENSIONS};
use fuckipv6_common::domain::{CapabilityKind, CapabilityState};
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
    fn new(state: NodeStateStore, commands: NodeCommandBus, cx: &mut Context<Self>) -> Self {
        let updates = state.subscribe();
        let snapshot = updates.borrow().clone();
        let selected_network = snapshot
            .networks
            .first()
            .map(|network| network.name.clone());
        let mut shell = Self {
            commands,
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
            self.error = Some("已有节点命令正在处理中，请等待完成".into());
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
            self.error = Some(format!("未知 manifest 动作，已拒绝执行: {action_id}"));
            cx.notify();
            return;
        };

        if let Some(reason) =
            action_block_reason(&action, &self.snapshot, self.selected_network.as_deref())
        {
            self.error = Some(format!("动作 {} 被阻止: {}", action.id, reason.message()));
            cx.notify();
            return;
        }

        if let Some(confirmation) = &action.confirmation {
            if self.pending_confirmation.as_deref() != Some(action.id.as_str()) {
                self.pending_confirmation = Some(action.id.clone());
                self.notice = Some(format!(
                    "确认 {}：{}。再次点击同一动作执行",
                    confirmation.title_key, confirmation.consequence_key
                ));
                cx.notify();
                return;
            }
        }

        let values = self.form_values.clone();
        match build_command(&action, &values) {
            Ok(command) => self.submit_raw(command, cx),
            Err(error) => {
                self.error = Some(format!("动作输入无法转换为共享命令: {error}"));
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
                self.error = Some(format!("节点命令通道错误: {error}"));
                return;
            }
        };

        match result.outcome {
            NodeCommandOutcome::Snapshot(snapshot) => {
                self.apply_snapshot(*snapshot);
                self.notice = Some("共享节点快照已刷新".into());
            }
            NodeCommandOutcome::UiManifest(manifest) => {
                let manifest = *manifest;
                if let Err(violations) = manifest.validate() {
                    self.manifest = None;
                    self.error = Some(format!("共享 UI manifest 校验失败: {violations:?}"));
                    return;
                }
                if let Err(error) = validate_manifest_bindings(&manifest) {
                    self.manifest = None;
                    self.error = Some(format!("GPUI 动作映射校验失败: {error}"));
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
                self.notice = Some(format!(
                    "共享 UI manifest 与 {} 个动作映射已加载",
                    ActionBinding::ALL.len()
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
                    problem.message, network, problem.code, problem.retryable
                ));
            }
            NodeCommandOutcome::Probe(probe) => {
                self.notice = Some(format!(
                    "探测 {} 完成: rtt={:?}, timed_out={}, p2p={}",
                    probe.peer_id, probe.rtt_us, probe.timed_out, probe.p2p_active
                ));
            }
            NodeCommandOutcome::NetworkCreated(created) => {
                self.notice = Some(created.owner_message());
            }
            NodeCommandOutcome::Diagnostics(diagnostics) => {
                let residue = &diagnostics.residue;
                let summary = if residue.is_clean() {
                    "运行时无残留".to_string()
                } else {
                    residue.issues.join("；")
                };
                self.notice = Some(format!(
                    "诊断完成: 会话 {}，监听器 {}，Peer {}，内嵌服务 {}；可重试清理 {} 个网络；{}",
                    residue.sessions_remaining,
                    residue.listeners_remaining,
                    residue.peers_remaining,
                    residue.embedded_services_remaining,
                    diagnostics.cleanup_candidate_networks.len(),
                    summary
                ));
            }
            NodeCommandOutcome::Cleanup(cleanup) => {
                let summary = if cleanup.residue.issues.is_empty() {
                    "未报告问题".to_string()
                } else {
                    cleanup.residue.issues.join("；")
                };
                self.notice = Some(format!(
                    "残留清理完成: 成功 {} 个，失败 {} 个；{}",
                    cleanup.cleaned_networks.len(),
                    cleanup.failed_networks.len(),
                    summary
                ));
            }
            NodeCommandOutcome::Blacklist(snapshot) => {
                self.notice = Some(format!(
                    "网络 [{}] 黑名单已同步：{} 个客户端{}",
                    snapshot.network_name,
                    snapshot.clients.len(),
                    if snapshot.changed {
                        "，状态已变更"
                    } else {
                        ""
                    }
                ));
            }
            NodeCommandOutcome::NetworkAllocation(snapshot) => {
                self.notice = Some(format!(
                    "网络 [{}] 地址分配已同步：{:?}，IPv4={}，IPv6={}，持久化={}",
                    snapshot.network_name,
                    snapshot.allocation.mode,
                    snapshot.allocation.ipv4_pool,
                    snapshot.allocation.ipv6_pool,
                    snapshot.config_saved
                ));
            }
            NodeCommandOutcome::Accepted { operation_id } => {
                self.notice = Some(format!("命令已接受: {operation_id}"));
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

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let title = page_title(&self.selected_page);
        let subtitle = self
            .page()
            .map(|page| format!("{} · {} 个共享动作", page.id, page.actions.len()))
            .unwrap_or_else(|| "等待共享 manifest".into());
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
                        primary_status.label_zh(),
                        primary_status_semantic(primary_status),
                    ))
                    .child(div().text_sm().text_color(rgba(COLOR_MUTED)).child(format!(
                        "{} · revision {}",
                        self.selected_network.as_deref().unwrap_or("未选择网络"),
                        self.snapshot.revision
                    )))
                    .child(
                        button(
                            SharedString::from("refresh-snapshot"),
                            if disabled {
                                "命令处理中…"
                            } else {
                                "刷新快照"
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
                    page_label(&page.id),
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
                label: capability_label(item.kind, item.state),
            })
            .collect::<Vec<_>>();

        let manifest_state = match &self.manifest {
            Some(manifest) => format!(
                "manifest v{} · {} pages",
                manifest.schema_version,
                manifest.pages.len()
            ),
            None => "manifest 尚未加载".into(),
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
                            .child("原生节点控制面 · manifest 驱动"),
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
                            .child("当前网络"),
                    )
                    .child(div().mt_2().flex().flex_wrap().gap_1().children(
                        if network_chips.is_empty() {
                            vec![div()
                                .text_xs()
                                .text_color(rgba(COLOR_MUTED))
                                .child("没有运行中的网络")
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
                            .child("节点能力"),
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
        component_toast(format!("操作失败: {error}"), SemanticState::Danger)
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
            .child(tab(PageSection::Data, "数据".into()))
            .child(tab(PageSection::Actions, format!("动作 · {action_count}")))
    }

    fn page_content(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(manifest) = self.manifest.as_ref() else {
            return loading_skeleton().into_any_element();
        };
        let model = match build_page_model(manifest, &self.snapshot, &self.selected_page) {
            Ok(model) => model,
            Err(error) => {
                return component_error_state("页面映射失败", error.to_string()).into_any_element();
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
                                .child("共享动作"),
                        )
                        .children(if actions.is_empty() {
                            vec![component_empty_state(
                                "没有共享动作",
                                "此页面在 manifest 中没有声明动作；当前仅展示真实快照字段。",
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
            NodeUiBackendStatus::Implemented => "后端已实现",
            NodeUiBackendStatus::Unavailable => "后端当前返回 Unavailable",
        };
        let availability = format!("{:?}", action.availability);
        let permission = format!("{:?}", action.permission);
        let button_label = if confirmation_pending {
            "再次点击确认"
        } else if action.confirmation.is_some() {
            "准备提交"
        } else {
            "提交共享命令"
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
                                    .child(action_label(&action.id)),
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
                    .child(format!("动作被阻止: {}", reason.message()))
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
        let label = field_label(&field.label_key);
        let required = if field.required { " *" } else { "" };
        let control = match field.input {
            NodeUiInputKind::Boolean => self.choice_field(
                action,
                field,
                &value,
                &[("true", "开启"), ("false", "关闭")],
                cx,
            ),
            NodeUiInputKind::Role => self.choice_field(
                action,
                field,
                &value,
                &[
                    ("member", "Member"),
                    ("discovery", "Discovery"),
                    ("relay", "Relay"),
                    ("exit_node", "Exit Node"),
                ],
                cx,
            ),
            NodeUiInputKind::RoutingMode => self.choice_field(
                action,
                field,
                &value,
                &[
                    ("mesh_only", "Mesh"),
                    ("bypass_mainland", "Bypass"),
                    ("global_proxy", "Global"),
                ],
                cx,
            ),
            NodeUiInputKind::NetworkSelection => self.choice_field(
                action,
                field,
                &value,
                &[("default", "Default"), ("all", "All"), ("none", "None")],
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
        let placeholder = placeholder_for(&field.input, &action.id, &field.id);
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
                let label = (*label).to_string();
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
        let selected_network = self.selected_network.as_deref().unwrap_or("未选择网络");

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
                            .child("当前上下文"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgba(COLOR_MUTED))
                            .child("基于实际快照，详情变化不会改变主内容宽度"),
                    ),
            )
            .child(detail_row("页面", page_title(&self.selected_page)))
            .child(detail_row("当前网络", selected_network.to_string()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgba(COLOR_MUTED))
                            .child("首要状态"),
                    )
                    .child(status_badge(
                        status.label_zh(),
                        primary_status_semantic(status),
                    )),
            )
            .child(detail_row(
                "快照 revision",
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
                .child(
                    "fuckipv6 · manifest-driven GPUI · NodeSnapshot/NodeCommandBus shared with WebUI · UI 对等尚未完成",
                ),
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

fn capability_label(kind: CapabilityKind, state: CapabilityState) -> &'static str {
    match (kind, state) {
        (CapabilityKind::Coordinator, CapabilityState::Enabled) => "Coordinator 开启",
        (CapabilityKind::Relay, CapabilityState::Enabled) => "Relay 开启",
        (CapabilityKind::Exit, CapabilityState::Enabled) => "Exit 开启",
        (CapabilityKind::File, CapabilityState::Enabled) => "File 开启",
        (CapabilityKind::Coordinator, CapabilityState::Degraded) => "Coordinator 降级",
        (CapabilityKind::Relay, CapabilityState::Degraded) => "Relay 降级",
        (CapabilityKind::Exit, CapabilityState::Degraded) => "Exit 降级",
        (CapabilityKind::File, CapabilityState::Degraded) => "File 降级",
        (CapabilityKind::Coordinator, CapabilityState::Starting) => "Coordinator 启动中",
        (CapabilityKind::Relay, CapabilityState::Starting) => "Relay 启动中",
        (CapabilityKind::Exit, CapabilityState::Starting) => "Exit 启动中",
        (CapabilityKind::File, CapabilityState::Starting) => "File 启动中",
        (CapabilityKind::Coordinator, CapabilityState::Unsupported) => "Coordinator 不支持",
        (CapabilityKind::Relay, CapabilityState::Unsupported) => "Relay 不支持",
        (CapabilityKind::Exit, CapabilityState::Unsupported) => "Exit 不支持",
        (CapabilityKind::File, CapabilityState::Unsupported) => "File 不支持",
        (CapabilityKind::Coordinator, _) => "Coordinator 未启用",
        (CapabilityKind::Relay, _) => "Relay 未启用",
        (CapabilityKind::Exit, _) => "Exit 未启用",
        (CapabilityKind::File, _) => "File 未启用",
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

fn page_label(id: &str) -> &'static str {
    match PageKind::from_id(id) {
        Some(PageKind::Overview) => "概览",
        Some(PageKind::Networks) => "网络",
        Some(PageKind::Peers) => "Peer",
        Some(PageKind::Topology) => "拓扑",
        Some(PageKind::Traffic) => "流量",
        Some(PageKind::Routing) => "路由",
        Some(PageKind::ExitNode) => "Exit Node",
        Some(PageKind::Relay) => "中继",
        Some(PageKind::Logs) => "日志",
        Some(PageKind::Identity) => "身份",
        Some(PageKind::Settings) => "设置",
        None => "未知页面",
    }
}

fn action_label(id: &str) -> &'static str {
    match id {
        "refresh_snapshot" => "刷新共享快照",
        "get_node_diagnostics" => "查看运行时诊断",
        "create_network" => "创建网络",
        "join_network" => "加入网络",
        "start_networks" => "启动网络",
        "stop_networks" => "停止网络",
        "restart_networks" => "重启网络",
        "probe_peer" => "探测 Peer",
        "get_blacklist" => "读取节点黑名单",
        "ban_client" => "封禁节点",
        "unban_client" => "解除节点封禁",
        "get_network_allocation" => "读取地址池",
        "update_network_allocation" => "更新地址池",
        "update_routing" => "更新路由",
        "set_exit_node_role" => "设置 Exit Node 角色",
        "set_relay_role" => "设置 Relay 角色",
        "start_node" => "启动节点",
        "stop_node" => "停止节点",
        "restart_node" => "重启节点",
        "cleanup_node_residuals" => "清理运行时残留",
        "shutdown_node" => "关闭节点",
        _ => "未知动作",
    }
}

fn field_label(key: &str) -> &'static str {
    match key {
        "field.network_name" => "网络名称",
        "field.signaling_port" => "信令端口",
        "field.relay_port" => "中继端口",
        "field.external_host" => "外部主机",
        "field.auto_connect" => "创建后自动连接",
        "field.ticket_or_url" => "票据或协调端 URL",
        "field.start_after_join" => "加入后启动",
        "field.network_selection" => "网络选择",
        "field.network_names" => "网络名称列表",
        "field.all_networks" => "全部网络",
        "field.peer_target" => "Peer 目标",
        "field.timeout_ms" => "超时（毫秒）",
        "field.client_id" => "节点标识",
        "field.network_mode" => "地址族模式",
        "field.ipv4_pool" => "IPv4 地址池",
        "field.ipv6_pool" => "IPv6 地址池",
        "field.routing_mode" => "路由模式",
        "field.exit_node" => "Exit Node",
        "field.is_exit_node" => "本节点作为 Exit Node",
        "field.role" => "网络角色",
        "field.enabled" => "启用",
        _ => "输入字段",
    }
}

fn placeholder_for(input: &NodeUiInputKind, action_id: &str, field_id: &str) -> &'static str {
    match input {
        NodeUiInputKind::NetworkName => "home",
        NodeUiInputKind::ExternalHost => "vpn.example.com",
        NodeUiInputKind::UrlOrTicket => "ws://host:port 或票据",
        NodeUiInputKind::PeerTarget => "peer-id 或 IPv6",
        NodeUiInputKind::Port => "1-65535",
        NodeUiInputKind::TimeoutMs => "1500",
        _ if matches!(
            action_id,
            "stop_networks" | "restart_networks" | "cleanup_node_residuals"
        ) && field_id == "networks" =>
        {
            "home,office"
        }
        _ => "输入值",
    }
}

fn page_title(id: &str) -> String {
    format!("{} · {}", page_label(id), id)
}

pub fn run_window(state: NodeStateStore, commands: NodeCommandBus) -> Result<()> {
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
            |_, cx| cx.new(|cx| GpuiShell::new(state.clone(), commands.clone(), cx)),
        );

        if let Err(error) = result {
            if let Ok(mut slot) = error_slot.lock() {
                *slot = Some(format!("无法创建 GPUI 原生窗口: {error}"));
            }
            cx.quit();
            return;
        }

        cx.activate(true);
    });

    let result = match open_error.lock() {
        Ok(mut error) => error.take().map_or(Ok(()), |message| Err(anyhow!(message))),
        Err(_) => Err(anyhow!("GPUI 窗口错误状态锁已损坏")),
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
            assert_ne!(page_label(&page.id), "未知页面");
        }
    }

    #[test]
    fn all_manifest_actions_have_a_localized_label() {
        for action in crate::node::node_ui_manifest()
            .pages
            .iter()
            .flat_map(|page| &page.actions)
        {
            assert_ne!(action_label(&action.id), "未知动作");
        }
    }

    #[test]
    fn page_titles_keep_the_stable_manifest_id() {
        assert_eq!(page_title("exit_node"), "Exit Node · exit_node");
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
                        _ => assert_ne!(placeholder_for(&field.input, &action.id, &field.id), ""),
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
        ] {
            assert!(!reason.message().is_empty());
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
                let label = capability_label(kind, state);
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
