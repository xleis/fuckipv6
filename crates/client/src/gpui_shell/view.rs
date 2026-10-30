use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use gpui::{
    div, prelude::*, px, rgb, size, App, Application, Bounds, Context, Render, SharedString,
    Window, WindowBounds, WindowOptions,
};

use crate::node::{
    NodeCommand, NodeCommandBus, NodeCommandBusError, NodeCommandOutcome, NodeCommandRequest,
    NodeCommandResult, NodeSnapshot, NodeStateStore, NodeUiBackendStatus, NodeUiManifest,
    NodeUiPage,
};

const WINDOW_WIDTH: f32 = 1180.0;
const WINDOW_HEIGHT: f32 = 800.0;

const COLOR_BG: u32 = 0x0b0f14;
const COLOR_SIDEBAR: u32 = 0x111821;
const COLOR_PANEL: u32 = 0x16202b;
const COLOR_PANEL_RAISED: u32 = 0x1c2936;
const COLOR_BORDER: u32 = 0x2a3b4c;
const COLOR_TEXT: u32 = 0xe6edf3;
const COLOR_MUTED: u32 = 0x8fa3b5;
const COLOR_ACCENT: u32 = 0x41d1a7;
const COLOR_ACCENT_DIM: u32 = 0x1d6b59;
const COLOR_ERROR_BG: u32 = 0x3a1c22;
const COLOR_ERROR_BORDER: u32 = 0x8f3545;
const COLOR_ERROR_TEXT: u32 = 0xffb4bf;

struct GpuiShell {
    commands: NodeCommandBus,
    snapshot: NodeSnapshot,
    manifest: Option<NodeUiManifest>,
    selected_page: String,
    pending_commands: u32,
    next_request_id: u64,
    error: Option<String>,
    notice: Option<String>,
}

impl GpuiShell {
    fn new(state: NodeStateStore, commands: NodeCommandBus, cx: &mut Context<Self>) -> Self {
        let updates = state.subscribe();
        let snapshot = updates.borrow().clone();
        let mut shell = Self {
            commands,
            snapshot,
            manifest: None,
            selected_page: "overview".into(),
            pending_commands: 0,
            next_request_id: 1,
            error: None,
            notice: None,
        };

        shell.watch_state(updates, cx);
        shell.submit(NodeCommand::GetUiManifest, cx);
        shell.submit(NodeCommand::GetSnapshot, cx);
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
                    shell.snapshot = snapshot;
                    cx.notify();
                })
                .is_err()
            {
                break;
            }
        })
        .detach();
    }

    fn submit(&mut self, command: NodeCommand, cx: &mut Context<Self>) {
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1).max(1);
        let request = NodeCommandRequest::new(request_id, command);
        let commands = self.commands.clone();
        self.pending_commands = self.pending_commands.saturating_add(1);
        self.error = None;
        self.notice = None;
        cx.notify();

        cx.spawn(async move |weak, cx| {
            let result = commands.execute(request).await;
            let _ = weak.update(cx, |shell, cx| {
                shell.pending_commands = shell.pending_commands.saturating_sub(1);
                shell.apply_command_result(result);
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_command_result(&mut self, result: Result<NodeCommandResult, NodeCommandBusError>) {
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                self.error = Some(format!("节点命令通道错误: {error}"));
                return;
            }
        };

        match result.outcome {
            NodeCommandOutcome::Snapshot(snapshot) => {
                self.snapshot = *snapshot;
                self.notice = Some("共享节点快照已刷新".into());
            }
            NodeCommandOutcome::UiManifest(manifest) => {
                let manifest = *manifest;
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
                self.notice = Some("共享 UI manifest 已加载".into());
            }
            NodeCommandOutcome::Rejected(problem) => {
                self.error = Some(format!("{} ({:?})", problem.message, problem.code));
            }
            NodeCommandOutcome::Probe(probe) => {
                self.notice = Some(format!(
                    "探测 {} 完成: rtt={:?}, timed_out={}",
                    probe.peer_id, probe.rtt_us, probe.timed_out
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
        let refresh_color = if disabled {
            COLOR_BORDER
        } else {
            COLOR_ACCENT_DIM
        };

        div()
            .flex()
            .items_center()
            .justify_between()
            .px_6()
            .py_4()
            .border_b_1()
            .border_color(rgb(COLOR_BORDER))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_2xl().text_color(rgb(COLOR_TEXT)).child(title))
                    .child(div().text_sm().text_color(rgb(COLOR_MUTED)).child(subtitle)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(COLOR_MUTED))
                            .child(format!("state revision {}", self.snapshot.revision)),
                    )
                    .child(
                        div()
                            .id("refresh-snapshot")
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(rgb(refresh_color))
                            .text_color(rgb(COLOR_TEXT))
                            .text_sm()
                            .on_click(cx.listener(|shell, _, _, cx| {
                                if shell.pending_commands == 0 {
                                    shell.submit(NodeCommand::GetSnapshot, cx);
                                }
                            }))
                            .child(if disabled {
                                "命令处理中…"
                            } else {
                                "刷新快照"
                            }),
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
                let background = if selected {
                    COLOR_ACCENT_DIM
                } else {
                    COLOR_SIDEBAR
                };
                div()
                    .id(SharedString::from(format!("nav-{}", page.id)))
                    .mx_3()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(background))
                    .text_color(rgb(if selected { COLOR_TEXT } else { COLOR_MUTED }))
                    .text_sm()
                    .on_click(cx.listener(move |shell, _, _, cx| {
                        shell.selected_page = page_id.clone();
                        shell.error = None;
                        shell.notice = None;
                        cx.notify();
                    }))
                    .child(page_label(&page.id))
                    .into_any_element()
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
            .w(px(238.0))
            .h_full()
            .bg(rgb(COLOR_SIDEBAR))
            .border_r_1()
            .border_color(rgb(COLOR_BORDER))
            .child(
                div()
                    .px_6()
                    .pt_6()
                    .pb_5()
                    .child(
                        div()
                            .text_xl()
                            .text_color(rgb(COLOR_ACCENT))
                            .child("fuckipv6"),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_xs()
                            .text_color(rgb(COLOR_MUTED))
                            .child("原生节点控制面"),
                    ),
            )
            .child(div().flex().flex_col().gap_1().flex_1().children(nav))
            .child(
                div()
                    .m_3()
                    .p_3()
                    .rounded_md()
                    .bg(rgb(COLOR_PANEL))
                    .text_xs()
                    .text_color(rgb(COLOR_MUTED))
                    .child(manifest_state),
            )
    }

    fn error_banner(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let error = self.error.clone().unwrap_or_default();
        div()
            .id("dismiss-error")
            .mx_6()
            .mt_4()
            .px_4()
            .py_3()
            .rounded_md()
            .bg(rgb(COLOR_ERROR_BG))
            .border_1()
            .border_color(rgb(COLOR_ERROR_BORDER))
            .text_color(rgb(COLOR_ERROR_TEXT))
            .text_sm()
            .child(format!("操作失败: {error}"))
            .on_click(cx.listener(|shell, _, _, cx| {
                shell.error = None;
                cx.notify();
            }))
    }

    fn notice_banner(&self) -> impl IntoElement {
        div()
            .mx_6()
            .mt_4()
            .px_4()
            .py_2()
            .rounded_md()
            .bg(rgb(COLOR_ACCENT_DIM))
            .text_color(rgb(COLOR_TEXT))
            .text_sm()
            .child(self.notice.clone().unwrap_or_default())
    }

    fn overview(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let identity = self
            .snapshot
            .identity
            .as_ref()
            .map(|identity| {
                identity
                    .display_name
                    .clone()
                    .unwrap_or_else(|| identity.node_id.clone())
            })
            .unwrap_or_else(|| "等待节点身份".into());
        let lifecycle = format!("{:?}", self.snapshot.lifecycle.phase);
        let pending = self.pending_commands;

        div()
            .flex()
            .flex_col()
            .gap_5()
            .p_6()
            .child(
                div()
                    .flex()
                    .gap_4()
                    .child(metric_card("节点阶段", &lifecycle, "共享生命周期"))
                    .child(metric_card(
                        "网络",
                        &self.snapshot.networks.len().to_string(),
                        "NodeSnapshot.networks",
                    ))
                    .child(metric_card(
                        "Peer",
                        &self.snapshot.peers.len().to_string(),
                        "NodeSnapshot.peers",
                    )),
            )
            .child(
                div()
                    .p_5()
                    .rounded_lg()
                    .bg(rgb(COLOR_PANEL))
                    .border_1()
                    .border_color(rgb(COLOR_BORDER))
                    .child(
                        div()
                            .text_lg()
                            .text_color(rgb(COLOR_TEXT))
                            .child("真实节点会话"),
                    )
                    .child(
                        div()
                            .mt_2()
                            .text_sm()
                            .text_color(rgb(COLOR_MUTED))
                            .child(format!("身份: {identity}")),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_sm()
                            .text_color(rgb(COLOR_MUTED))
                            .child(format!(
                                "状态通道 revision {} · {} 条日志 · {} 个内嵌服务",
                                self.snapshot.revision,
                                self.snapshot.logs.len(),
                                self.snapshot.embedded_services.len()
                            )),
                    )
                    .child(div().mt_4().text_sm().text_color(rgb(COLOR_MUTED)).child(
                        if pending == 0 {
                            "本窗口直接订阅 NodeStateStore，并通过 NodeCommandBus 提交命令。"
                        } else {
                            "正在等待节点命令结果…"
                        },
                    )),
            )
            .child(
                div()
                    .p_5()
                    .rounded_lg()
                    .bg(rgb(COLOR_PANEL_RAISED))
                    .border_1()
                    .border_color(rgb(COLOR_BORDER))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(COLOR_MUTED))
                            .child("NX-08 边界"),
                    )
                    .child(
                        div()
                            .mt_2()
                            .text_base()
                            .text_color(rgb(COLOR_TEXT))
                            .child("当前提交只实现原生应用外壳；NX-09 的业务页面与表单仍为占位。"),
                    ),
            )
            .child(
                div()
                    .id("reload-manifest")
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(COLOR_PANEL))
                    .text_color(rgb(COLOR_MUTED))
                    .text_sm()
                    .on_click(cx.listener(|shell, _, _, cx| {
                        if shell.pending_commands == 0 {
                            shell.submit(NodeCommand::GetUiManifest, cx);
                        }
                    }))
                    .child("重新加载 UI manifest"),
            )
    }

    fn placeholder(&self) -> impl IntoElement {
        let page = self.page();
        let title = page_title(&self.selected_page);
        let actions = page
            .map(|page| {
                page.actions
                    .iter()
                    .map(|action| {
                        let status = match action.backend_status {
                            NodeUiBackendStatus::Implemented => "implemented",
                            NodeUiBackendStatus::Unavailable => "unavailable",
                        };
                        format!("{} · {:?} · {}", action.id, action.command, status)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        div()
            .flex()
            .flex_col()
            .gap_4()
            .p_6()
            .child(
                div()
                    .p_8()
                    .rounded_lg()
                    .bg(rgb(COLOR_PANEL))
                    .border_1()
                    .border_color(rgb(COLOR_BORDER))
                    .child(
                        div()
                            .text_xl()
                            .text_color(rgb(COLOR_TEXT))
                            .child(title),
                    )
                    .child(
                        div()
                            .mt_3()
                            .text_base()
                            .text_color(rgb(COLOR_MUTED))
                            .child("此页面由 NX-09 实现。当前外壳已接入共享 manifest、状态订阅和命令通道，但没有伪造业务表单或成功状态。"),
                    )
                    .child(
                        div()
                            .mt_4()
                            .text_sm()
                            .text_color(rgb(COLOR_MUTED))
                            .child("已从 manifest 读取的动作："),
                    )
                    .children(actions.into_iter().map(|action| {
                        div()
                            .mt_2()
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(rgb(COLOR_PANEL_RAISED))
                            .text_sm()
                            .text_color(rgb(COLOR_MUTED))
                            .child(action)
                    })),
            )
    }
}

impl Render for GpuiShell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = if self.selected_page == "overview" {
            self.overview(cx).into_any_element()
        } else {
            self.placeholder().into_any_element()
        };

        let mut main = div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .bg(rgb(COLOR_BG))
            .child(self.header(cx));

        if self.error.is_some() {
            main = main.child(self.error_banner(cx));
        } else if self.notice.is_some() {
            main = main.child(self.notice_banner());
        }

        main = main.child(content).child(
            div()
                .px_6()
                .py_3()
                .border_t_1()
                .border_color(rgb(COLOR_BORDER))
                .text_xs()
                .text_color(rgb(COLOR_MUTED))
                .child("fuckipv6 · native GPUI shell · state + command contract shared with WebUI"),
        );

        div()
            .flex()
            .w_full()
            .h_full()
            .bg(rgb(COLOR_BG))
            .child(self.sidebar(cx))
            .child(main)
    }
}

fn metric_card(label: &str, value: &str, source: &str) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .flex_1()
        .p_4()
        .rounded_lg()
        .bg(rgb(COLOR_PANEL))
        .border_1()
        .border_color(rgb(COLOR_BORDER))
        .child(
            div()
                .text_sm()
                .text_color(rgb(COLOR_MUTED))
                .child(label.to_string()),
        )
        .child(
            div()
                .text_3xl()
                .text_color(rgb(COLOR_TEXT))
                .child(value.to_string()),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(COLOR_MUTED))
                .child(source.to_string()),
        )
}

fn page_label(id: &str) -> &'static str {
    match id {
        "overview" => "概览",
        "networks" => "网络",
        "peers" => "Peer",
        "topology" => "拓扑",
        "traffic" => "流量",
        "routing" => "路由",
        "exit_node" => "Exit Node",
        "relay" => "中继",
        "logs" => "日志",
        "identity" => "身份",
        "settings" => "设置",
        _ => "未知页面",
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

    #[test]
    fn all_manifest_pages_have_a_navigation_label() {
        for page in crate::node::node_ui_manifest().pages {
            assert_ne!(page_label(&page.id), "未知页面");
        }
    }

    #[test]
    fn page_titles_keep_the_stable_manifest_id() {
        assert_eq!(page_title("exit_node"), "Exit Node · exit_node");
    }
}
