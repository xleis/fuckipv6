pub mod commands;
pub mod network_commands;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::logging;
use crate::ui::{resolve_ui_mode, UiCapabilities, UiMode, UiModeError, UiModeRequest};

#[derive(Debug, Parser)]
#[command(author, version, about = "fuckipv6 - 全能对等网络与智能分流控制中心", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// 启动时自动开启协调网络服务 (自建局域网/互联网中枢)
    #[arg(long)]
    pub host: bool,

    /// 协调服务网络名称 (配合 --host)
    #[arg(long, default_value = "default")]
    pub host_name: String,

    /// 协调服务信令监听端口 (默认 6666)
    #[arg(long, default_value = "6666")]
    pub host_port: u16,

    /// 协调服务 QUIC 中继监听端口 (默认 6667)
    #[arg(long, default_value = "6667")]
    pub host_relay_port: u16,

    /// 外部主机名或公网 IP (写入邀请票据)
    #[arg(long)]
    pub external_host: Option<String>,

    /// 将本机持久标记为 Exit Node（可写 --is-exit-node=false）
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true", require_equals = true)]
    pub is_exit_node: Option<bool>,

    /// 协调服务启动后本机自动作为客户端加入
    #[arg(long, default_value_t = true)]
    pub auto_connect: bool,

    /// 强制无头模式（不打开桌面窗口，WebUI 服务仍然监听）
    #[arg(long, conflicts_with_all = ["webui", "gpui"])]
    pub headless: bool,

    /// 使用浏览器托管的 WebUI（`--gui` 为兼容别名）
    #[arg(long, visible_alias = "gui", conflicts_with_all = ["headless", "gpui"])]
    pub webui: bool,

    /// 使用原生 GPUI 桌面界面
    #[arg(long, conflicts_with_all = ["headless", "webui"])]
    pub gpui: bool,

    /// 统一控制中心 WebUI 监听端口 (默认 8080)
    #[arg(long, default_value = "8080")]
    pub webui_port: u16,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// 作为网络创建者启动对等协调服务并生成邀请票据 (fk://...)
    Host {
        /// 网络名称
        #[arg(short, long, default_value = "default")]
        name: String,

        /// 信令监听端口
        #[arg(short, long, default_value = "6666")]
        port: u16,

        /// QUIC 中继监听端口
        #[arg(long, default_value = "6667")]
        relay_port: u16,

        /// 外部主机名或 IP（写入邀请票据）
        #[arg(long)]
        external_host: Option<String>,

        /// 协调服务启动后本机自动作为客户端加入（可写 --auto-connect=false）
        #[arg(short, long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true", require_equals = true)]
        auto_connect: Option<bool>,

        /// 路由模式: mesh-only, bypass-mainland, global-proxy
        #[arg(long)]
        routing_mode: Option<String>,

        /// 出口节点 (虚拟 IPv6, 虚拟 IPv4 或 ClientId)
        #[arg(long)]
        exit_node: Option<String>,

        /// 将本机持久标记为 Exit Node（可写 --is-exit-node=false）
        #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true", require_equals = true)]
        is_exit_node: Option<bool>,

        /// 强制无头模式（不打开桌面窗口，WebUI 服务仍然监听）
        #[arg(long, conflicts_with_all = ["webui", "gpui"])]
        headless: bool,

        /// 使用浏览器托管的 WebUI（`--gui` 为兼容别名）
        #[arg(long, visible_alias = "gui", conflicts_with_all = ["headless", "gpui"])]
        webui: bool,

        /// 使用原生 GPUI 桌面界面
        #[arg(long, conflicts_with_all = ["headless", "webui"])]
        gpui: bool,

        /// WebUI 监听端口 (默认 8080)
        #[arg(long, default_value = "8080")]
        webui_port: u16,
    },

    /// 加入网络（保存配置并连接，支持传统地址或 fk:// 邀请票据）
    Join {
        /// 协调服务器地址或邀请凭证 (例如: xlei.cc:6666 或 fk://...)
        server: String,

        /// 网络名称（可选，默认从服务器或凭证提取）
        #[arg(short, long)]
        name: Option<String>,

        /// 直接启动连接
        #[arg(short, long)]
        start: bool,

        /// 路由模式: mesh-only, bypass-mainland, global-proxy
        #[arg(long)]
        routing_mode: Option<String>,

        /// 出口节点 (虚拟 IPv6, 虚拟 IPv4 或 ClientId)
        #[arg(long)]
        exit_node: Option<String>,

        /// 将本机持久标记为 Exit Node（可写 --is-exit-node=false）
        #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true", require_equals = true)]
        is_exit_node: Option<bool>,

        /// 强制无头模式（不打开桌面窗口，WebUI 服务仍然监听）
        #[arg(long, conflicts_with_all = ["webui", "gpui"])]
        headless: bool,

        /// 使用浏览器托管的 WebUI（`--gui` 为兼容别名）
        #[arg(long, visible_alias = "gui", conflicts_with_all = ["headless", "gpui"])]
        webui: bool,

        /// 使用原生 GPUI 桌面界面
        #[arg(long, conflicts_with_all = ["headless", "webui"])]
        gpui: bool,

        /// WebUI 监听端口 (默认 8080)
        #[arg(long, default_value = "8080")]
        webui_port: u16,
    },

    /// 连接到网络（使用已保存的配置或临时连接，支持多网并发）
    Connect {
        /// 网络名称、服务器地址或邀请凭证（可指定多个）
        #[arg(name = "TARGET")]
        targets: Vec<String>,

        /// 连接所有已保存的网络
        #[arg(short, long)]
        all: bool,

        /// 临时连接（不保存配置）
        #[arg(short, long)]
        temp: bool,

        /// 路由模式: mesh-only, bypass-mainland, global-proxy
        #[arg(long)]
        routing_mode: Option<String>,

        /// 出口节点 (虚拟 IPv6, 虚拟 IPv4 或 ClientId)
        #[arg(long)]
        exit_node: Option<String>,

        /// 将本机持久标记为 Exit Node（可写 --is-exit-node=false）
        #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true", require_equals = true)]
        is_exit_node: Option<bool>,

        /// 强制无头模式（不打开桌面窗口，WebUI 服务仍然监听）
        #[arg(long, conflicts_with_all = ["webui", "gpui"])]
        headless: bool,

        /// 使用浏览器托管的 WebUI（`--gui` 为兼容别名）
        #[arg(long, visible_alias = "gui", conflicts_with_all = ["headless", "gpui"])]
        webui: bool,

        /// 使用原生 GPUI 桌面界面
        #[arg(long, conflicts_with_all = ["headless", "webui"])]
        gpui: bool,

        /// WebUI 监听端口 (默认 8080)
        #[arg(long, default_value = "8080")]
        webui_port: u16,
    },

    /// 启动并运行一个或多个虚拟网络（支持多网络并发共存）
    Up {
        /// 要启动的网络名称列表（若留空且未指定 --all 则启动默认网络）
        #[arg(name = "NETWORK")]
        networks: Vec<String>,

        /// 启动所有已保存的网络
        #[arg(short, long)]
        all: bool,

        /// 路由模式: mesh-only, bypass-mainland, global-proxy
        #[arg(long)]
        routing_mode: Option<String>,

        /// 出口节点 (虚拟 IPv6, 虚拟 IPv4 或 ClientId)
        #[arg(long)]
        exit_node: Option<String>,

        /// 将本机持久标记为 Exit Node（可写 --is-exit-node=false）
        #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true", require_equals = true)]
        is_exit_node: Option<bool>,

        /// 强制无头模式（不打开桌面窗口，WebUI 服务仍然监听）
        #[arg(long, conflicts_with_all = ["webui", "gpui"])]
        headless: bool,

        /// 使用浏览器托管的 WebUI（`--gui` 为兼容别名）
        #[arg(long, visible_alias = "gui", conflicts_with_all = ["headless", "gpui"])]
        webui: bool,

        /// 使用原生 GPUI 桌面界面
        #[arg(long, conflicts_with_all = ["headless", "webui"])]
        gpui: bool,

        /// WebUI 监听端口 (默认 8080)
        #[arg(long, default_value = "8080")]
        webui_port: u16,
    },

    /// 停止运行中的虚拟网络
    Down {
        /// 要停止的网络名称列表（若留空且未指定 --all 则停止全部）
        #[arg(name = "NETWORK")]
        networks: Vec<String>,

        /// 停止全部运行中的虚拟网络
        #[arg(short, long)]
        all: bool,
    },

    /// 列出所有已保存的网络
    List,

    /// 删除已保存的网络配置
    Leave {
        /// 网络名称
        network: String,
    },

    /// 设置默认网络配置
    Default {
        /// 网络名称
        network: String,
    },

    /// 启动客户端（传统方式）
    Start {
        /// 配置文件路径
        #[arg(short, long, default_value = "client.toml")]
        config: String,

        /// 路由模式: mesh-only, bypass-mainland, global-proxy
        #[arg(long)]
        routing_mode: Option<String>,

        /// 出口节点 (虚拟 IPv6, 虚拟 IPv4 或 ClientId)
        #[arg(long)]
        exit_node: Option<String>,

        /// 将本机持久标记为 Exit Node（可写 --is-exit-node=false）
        #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true", require_equals = true)]
        is_exit_node: Option<bool>,

        /// 强制无头模式（不打开桌面窗口，WebUI 服务仍然监听）
        #[arg(long, conflicts_with_all = ["webui", "gpui"])]
        headless: bool,

        /// 使用浏览器托管的 WebUI（`--gui` 为兼容别名）
        #[arg(long, visible_alias = "gui", conflicts_with_all = ["headless", "gpui"])]
        webui: bool,

        /// 使用原生 GPUI 桌面界面
        #[arg(long, conflicts_with_all = ["headless", "webui"])]
        gpui: bool,

        /// WebUI 监听端口 (默认 8080)
        #[arg(long, default_value = "8080")]
        webui_port: u16,
    },

    /// 查看运行状态
    Status {
        /// 输出机器可读的完整 JSON 快照
        #[arg(long)]
        json: bool,
    },

    /// 查看连接的对等节点
    Peers,

    /// Ping测试虚拟网络连通性
    Ping {
        /// 目标虚拟IP地址
        target: String,

        /// Ping次数
        #[arg(short, long, default_value = "4")]
        count: u32,
    },

    /// 查看网络统计信息
    Stats,

    /// 查看 IPv6 路由表
    Route {
        /// TUN接口名称
        #[arg(short, long, default_value = "fuckipv6")]
        interface: String,

        /// 显示全部IPv6路由，而不是只显示TUN接口路由
        #[arg(long)]
        all: bool,
    },

    /// 查看日志
    Logs {
        /// 日志行数
        #[arg(short, long, default_value = "100")]
        lines: usize,

        /// 直接读取指定日志文件
        #[arg(short, long)]
        file: Option<String>,

        /// systemd服务: client, coordinator, 或完整 .service 名称
        #[arg(short, long, default_value = "client")]
        service: String,
    },

    /// Linux：通过认证 UDP 转发真实 SYN-ACK，救援被阻断的 TCP 握手
    LxtRescue {
        #[command(subcommand)]
        command: LxtRescueCommands,
    },

    /// 生成配置文件
    InitConfig {
        /// 输出配置文件路径
        #[arg(default_value = "client.toml")]
        output: String,
    },

    /// 加密客户端配置文件
    EncryptConfig {
        /// 输入明文配置文件
        input: String,

        /// 输出加密配置文件
        output: String,
    },

    /// 解密客户端配置文件
    DecryptConfig {
        /// 输入加密配置文件
        input: String,

        /// 输出明文配置文件；不指定时输出到 stdout
        #[arg(short, long)]
        output: Option<String>,
    },

    /// 安装、升级和管理 systemd/launchd/Windows 系统服务
    Service(crate::service::ServiceArgs),

    /// 在本机终端验证恢复码并注册新的所有者 Passkey（不提供公网找回）
    RecoverOwner,

    /// 查看版本信息
    Version,

    /// 查看本地 QUIC 能力和证书 pin
    QuicInfo,

    /// 检查协调服务器注册流程（不创建TUN设备）
    RegisterCheck {
        /// 协调服务器 WebSocket 地址
        #[arg(short, long)]
        coordinator: String,

        /// 客户端私钥文件路径
        #[arg(short, long, default_value = "client_key.pem")]
        key: String,

        /// 上报的 P2P 端口
        #[arg(long, default_value = "0")]
        p2p_port: u16,

        /// 注册超时秒数
        #[arg(long, default_value = "5")]
        timeout_secs: u64,
    },
}

#[derive(Debug, Subcommand)]
pub enum LxtRescueCommands {
    /// 在服务端捕获本机出站 SYN-ACK，并通过 UDP 发给已认证客户端
    Server {
        /// Linux 抓包网卡
        #[arg(long)]
        interface: String,

        /// TCP 服务监听端口
        #[arg(long, default_value = "39090")]
        tcp_port: u16,

        /// UDP 救援监听地址
        #[arg(long, default_value = "0.0.0.0")]
        bind: std::net::Ipv4Addr,

        /// UDP 救援监听端口
        #[arg(long, default_value = "39091")]
        udp_port: u16,

        /// 共享十六进制密钥文件（至少 32 字节，权限 0600）
        #[arg(long, default_value = "/etc/fuckipv6/lxt-rescue.key")]
        key_file: std::path::PathBuf,
    },

    /// 在客户端捕获出站 SYN，并把认证 UDP 收到的 SYN-ACK 注入独立 TUN
    Client {
        /// Linux 抓包网卡
        #[arg(long)]
        interface: String,

        /// 救援服务端单播 IPv4 地址
        #[arg(long)]
        server: std::net::Ipv4Addr,

        /// TCP 服务监听端口
        #[arg(long, default_value = "39090")]
        tcp_port: u16,

        /// UDP 救援服务端端口
        #[arg(long, default_value = "39091")]
        udp_port: u16,

        /// HELLO 保活间隔（毫秒）
        #[arg(long, default_value = "1500")]
        hello_interval_ms: u64,

        /// 仅用于握手救援的独立 TUN 名称
        #[arg(long, default_value = "lxtrescue0")]
        tun: String,

        /// 共享十六进制密钥文件（至少 32 字节，权限 0600）
        #[arg(long, default_value = "/etc/fuckipv6/lxt-rescue.key")]
        key_file: std::path::PathBuf,
    },
}

fn requested_ui_mode(
    headless: bool,
    webui: bool,
    gpui: bool,
) -> Result<UiModeRequest, UiModeError> {
    match (headless, webui, gpui) {
        (true, false, false) => Ok(UiModeRequest::Headless),
        (false, true, false) => Ok(UiModeRequest::WebUi),
        (false, false, true) => Ok(UiModeRequest::Gpui),
        (false, false, false) => Ok(UiModeRequest::Auto),
        _ => Err(UiModeError::ConflictingFlags),
    }
}

#[allow(clippy::too_many_arguments)]
fn runtime_options_with_capabilities(
    routing_mode: Option<String>,
    exit_node: Option<String>,
    is_exit_node: Option<bool>,
    headless: bool,
    webui: bool,
    gpui: bool,
    webui_port: u16,
    capabilities: UiCapabilities,
) -> Result<commands::RuntimeOptions> {
    let requested = requested_ui_mode(headless, webui, gpui)?;
    let effective = resolve_ui_mode(requested, capabilities)?.effective;

    let (headless, gui) = match effective {
        UiMode::Headless => (true, false),
        UiMode::WebUi => (false, true),
        // RuntimeOptions still carries the legacy two-bit adapter shape. GPUI
        // is neither the headless node core nor the browser-hosted `gui`
        // adapter; the native launch decision is carried by UiModeResolution.
        UiMode::Gpui => (false, false),
    };

    Ok(commands::RuntimeOptions {
        routing_mode,
        exit_node,
        is_exit_node,
        headless,
        gui,
        webui_port,
    })
}

#[allow(clippy::too_many_arguments)]
fn runtime_options(
    routing_mode: Option<String>,
    exit_node: Option<String>,
    is_exit_node: Option<bool>,
    headless: bool,
    webui: bool,
    gpui: bool,
    webui_port: u16,
) -> Result<commands::RuntimeOptions> {
    runtime_options_with_capabilities(
        routing_mode,
        exit_node,
        is_exit_node,
        headless,
        webui,
        gpui,
        webui_port,
        UiCapabilities::detect(),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CommandUiIntent {
    requested: UiModeRequest,
    webui_port: u16,
}

fn command_ui_intent(command: &Commands) -> Option<CommandUiIntent> {
    let (requested, webui_port) = match command {
        Commands::Host {
            headless,
            webui,
            gpui,
            webui_port,
            ..
        }
        | Commands::Join {
            headless,
            webui,
            gpui,
            webui_port,
            ..
        }
        | Commands::Connect {
            headless,
            webui,
            gpui,
            webui_port,
            ..
        }
        | Commands::Up {
            headless,
            webui,
            gpui,
            webui_port,
            ..
        }
        | Commands::Start {
            headless,
            webui,
            gpui,
            webui_port,
            ..
        } => (
            requested_ui_mode(*headless, *webui, *gpui).ok()?,
            *webui_port,
        ),
        _ => return None,
    };
    Some(CommandUiIntent {
        requested,
        webui_port,
    })
}

async fn run_command_ui(intent: CommandUiIntent, ui_mode: UiMode) -> Result<()> {
    if ui_mode.is_gpui() {
        anyhow::bail!("原生 GPUI 必须在 OS 主线程启动；请通过进程入口运行");
    }

    run_default_app(
        intent.webui_port,
        intent.requested,
        false,
        "default".to_string(),
        6666,
        6667,
        true,
        None,
        None,
    )
    .await
}

fn async_runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("无法创建异步运行时")
}

/// Synchronous process entrypoint. GPUI must be constructed on the OS main
/// thread, so native desktop mode is selected before any Tokio runtime exists.
pub fn run_entrypoint() -> Result<()> {
    let cli = Cli::parse();

    if let Some(intent) = cli.command.as_ref().and_then(command_ui_intent) {
        let effective = resolve_ui_mode(intent.requested, UiCapabilities::detect())?.effective;
        if effective.is_gpui() {
            let command = cli
                .command
                .expect("command UI intent requires a parsed subcommand");
            async_runtime()?.block_on(run_command(command))?;
            return run_native_app(commands::RuntimeOptions {
                is_exit_node: None,
                headless: false,
                gui: false,
                webui_port: 0,
                ..commands::RuntimeOptions::default()
            });
        }
    } else if cli.command.is_none() {
        let requested_ui = requested_ui_mode(cli.headless, cli.webui, cli.gpui)?;
        let effective = resolve_ui_mode(requested_ui, UiCapabilities::detect())?.effective;
        if effective.is_gpui() {
            if cli.host {
                async_runtime()?.block_on(run_top_level_host_prelude(&cli))?;
            }
            return run_native_app(native_runtime_options(cli.is_exit_node));
        }
    }

    async_runtime()?.block_on(run_parsed(cli))
}

/// Async compatibility entrypoint for callers that already own a runtime.
/// The process `main` must use `run_entrypoint` so native GPUI never starts on
/// a Tokio worker thread.
pub async fn run() -> Result<()> {
    run_parsed(Cli::parse()).await
}

async fn run_parsed(cli: Cli) -> Result<()> {
    if let Some(command) = cli.command {
        let command_ui = command_ui_intent(&command)
            .map(|intent| {
                resolve_ui_mode(intent.requested, UiCapabilities::detect())
                    .map(|resolution| (intent, resolution.effective))
            })
            .transpose()?;
        if let Some((_, ui_mode)) = command_ui {
            if ui_mode.is_gpui() {
                anyhow::bail!("原生 GPUI 必须在 OS 主线程启动；请通过进程入口运行");
            }
        }

        run_command(command).await?;
        if let Some((intent, ui_mode)) = command_ui {
            run_command_ui(intent, ui_mode).await?;
        }
        Ok(())
    } else {
        let requested_ui = requested_ui_mode(cli.headless, cli.webui, cli.gpui)?;
        run_default_app(
            cli.webui_port,
            requested_ui,
            cli.host,
            cli.host_name,
            cli.host_port,
            cli.host_relay_port,
            cli.auto_connect,
            cli.external_host,
            cli.is_exit_node,
        )
        .await
    }
}

async fn run_top_level_host_prelude(cli: &Cli) -> Result<()> {
    network_commands::host(
        &cli.host_name,
        cli.host_port,
        cli.host_relay_port,
        cli.external_host.clone(),
        cli.auto_connect,
        commands::RuntimeOptions {
            is_exit_node: cli.is_exit_node,
            headless: false,
            gui: false,
            webui_port: 0,
            ..commands::RuntimeOptions::default()
        },
    )
    .await
}

fn native_runtime_options(is_exit_node: Option<bool>) -> commands::RuntimeOptions {
    commands::RuntimeOptions {
        is_exit_node,
        headless: false,
        // GPUI is not the browser-hosted `gui` adapter. Port 0 prevents a
        // future shared runtime plan from implicitly opening WebUI.
        gui: false,
        webui_port: 0,
        ..commands::RuntimeOptions::default()
    }
}

#[cfg(all(
    feature = "gpui-shell",
    any(
        all(target_os = "linux", not(target_env = "musl")),
        windows,
        target_os = "macos"
    )
))]
fn run_native_app(options: commands::RuntimeOptions) -> Result<()> {
    logging::init_default()?;
    crate::gpui_shell::run(options)
}

#[cfg(not(all(
    feature = "gpui-shell",
    any(
        all(target_os = "linux", not(target_env = "musl")),
        windows,
        target_os = "macos"
    )
)))]
fn run_native_app(_options: commands::RuntimeOptions) -> Result<()> {
    anyhow::bail!("当前构建未包含可用的原生 GPUI 外壳")
}

#[allow(clippy::too_many_arguments)]
pub async fn run_default_app(
    port: u16,
    requested_ui: UiModeRequest,
    host: bool,
    host_name: String,
    host_port: u16,
    host_relay_port: u16,
    auto_connect: bool,
    external_host: Option<String>,
    is_exit_node: Option<bool>,
) -> Result<()> {
    logging::init_default()?;
    let ui_mode = resolve_ui_mode(requested_ui, UiCapabilities::detect())?;
    let shutdown = crate::runtime_process::ShutdownSignal::register()?;

    if ui_mode.effective.is_gpui() {
        anyhow::bail!("原生 GPUI 必须在 OS 主线程启动；请通过 `fuckipv6 --gpui` 运行");
    }

    let session = crate::remote_node::RemoteNodeSession::start_with_role_async(
        commands::RuntimeOptions {
            routing_mode: None,
            exit_node: None,
            is_exit_node,
            headless: ui_mode.effective.is_headless(),
            gui: false,
            webui_port: port,
        },
        crate::local_ipc::IpcRole::WebUi,
    )
    .await?;
    let node_http =
        crate::node::NodeHttpAdapter::new(session.commands(), session.state(), session.events());
    let controller = std::sync::Arc::new(crate::webui::AppController::from_node_session(session));
    let server_handle =
        crate::webui::start_app_webui_with_node(port, controller.clone(), node_http).await?;

    if host {
        if let Err(e) = controller
            .host_network(
                host_name,
                host_port,
                host_relay_port,
                auto_connect,
                external_host,
            )
            .await
        {
            tracing::warn!("启动协调网络服务失败: {e:#}");
        }
    }

    match ui_mode.effective {
        UiMode::Gpui => unreachable!("GPUI is rejected before the WebUI server starts"),
        UiMode::WebUi => crate::ui::run_webui_window(port).await?,
        UiMode::Headless => {
            println!("========================================================================");
            println!("🚀 fuckipv6 统一控制中心已就绪 (无头模式)");
            println!("------------------------------------------------------------------------");
            println!("控制中心访问地址 (客户端 + 协调中枢 + 智能分流):");
            println!("  👉 本地访问: http://127.0.0.1:{port}");
            println!("  👉 局域网/公网访问: http://<你的服务器IP>:{port}");
            println!();
            println!("在浏览器中打开上述地址，即可在一个界面完成：");
            println!("  - 加入对等网络 / 连接节点 (支持 fk:// 邀请票据)");
            println!("  - 原生智能分流规则 (绕过大陆/全局代理) 与出境网关配置");
            println!("  - 开启/管理网络协调中枢、QUIC中继与在线客户端管控");
            println!("  - 查看实时网络拓扑、延时指标与控制台日志");
            println!("========================================================================");
        }
    }

    println!("按 Ctrl+C 退出程序");
    shutdown.wait().await;
    tracing::info!("接收到退出信号，正在关闭 WebUI；节点网络与控制面保持运行...");
    server_handle.abort();
    let _ = server_handle.await;
    Ok(())
}

async fn run_command(command: Commands) -> Result<()> {
    match command {
        command @ (Commands::Host { .. }
        | Commands::Join { .. }
        | Commands::Connect { .. }
        | Commands::Up { .. }
        | Commands::Down { .. }
        | Commands::List
        | Commands::Leave { .. }
        | Commands::Default { .. }) => run_network_command(command).await?,
        command @ (Commands::Start { .. }
        | Commands::Status { .. }
        | Commands::Peers
        | Commands::Ping { .. }
        | Commands::Stats
        | Commands::Route { .. }
        | Commands::Logs { .. }
        | Commands::QuicInfo
        | Commands::RegisterCheck { .. }) => run_runtime_command(command).await?,
        command @ Commands::LxtRescue { .. } => run_lxt_rescue_command(command).await?,
        Commands::RecoverOwner => {
            logging::init_default()?;
            crate::owner_security::recovery::run_local_recovery().await?;
        }
        Commands::Service(args) => crate::service::run(args)?,
        command @ (Commands::InitConfig { .. }
        | Commands::EncryptConfig { .. }
        | Commands::DecryptConfig { .. }
        | Commands::Version) => run_config_command(command)?,
    }

    Ok(())
}

async fn run_network_command(command: Commands) -> Result<()> {
    match command {
        Commands::Host {
            name,
            port,
            relay_port,
            external_host,
            auto_connect,
            routing_mode,
            exit_node,
            is_exit_node,
            headless,
            webui,
            gpui,
            webui_port,
        } => {
            let options = runtime_options(
                routing_mode,
                exit_node,
                is_exit_node,
                headless,
                webui,
                gpui,
                webui_port,
            )?;
            network_commands::host(
                &name,
                port,
                relay_port,
                external_host,
                auto_connect.unwrap_or(true),
                options,
            )
            .await?
        }
        Commands::Join {
            server,
            name,
            start,
            routing_mode,
            exit_node,
            is_exit_node,
            headless,
            webui,
            gpui,
            webui_port,
        } => {
            let options = runtime_options(
                routing_mode,
                exit_node,
                is_exit_node,
                headless,
                webui,
                gpui,
                webui_port,
            )?;
            network_commands::join(&server, name.as_deref(), start, options).await?
        }
        Commands::Connect {
            targets,
            all,
            temp,
            routing_mode,
            exit_node,
            is_exit_node,
            headless,
            webui,
            gpui,
            webui_port,
        } => {
            let options = runtime_options(
                routing_mode,
                exit_node,
                is_exit_node,
                headless,
                webui,
                gpui,
                webui_port,
            )?;
            network_commands::connect(targets, all, temp, options).await?
        }
        Commands::Up {
            networks,
            all,
            routing_mode,
            exit_node,
            is_exit_node,
            headless,
            webui,
            gpui,
            webui_port,
        } => {
            let options = runtime_options(
                routing_mode,
                exit_node,
                is_exit_node,
                headless,
                webui,
                gpui,
                webui_port,
            )?;
            network_commands::up(networks, all, options).await?
        }
        Commands::Down { networks, all } => network_commands::down(networks, all).await?,
        Commands::List => {
            logging::init_default()?;
            network_commands::list_networks()?;
        }
        Commands::Leave { network } => {
            logging::init_default()?;
            network_commands::leave(&network)?;
        }
        Commands::Default { network } => {
            logging::init_default()?;
            network_commands::set_default(&network)?;
        }
        _ => unreachable!("run_network_command received non-network command"),
    }

    Ok(())
}

async fn run_runtime_command(command: Commands) -> Result<()> {
    match command {
        Commands::Start {
            config,
            routing_mode,
            exit_node,
            is_exit_node,
            headless,
            webui,
            gpui,
            webui_port,
        } => {
            let options = runtime_options(
                routing_mode,
                exit_node,
                is_exit_node,
                headless,
                webui,
                gpui,
                webui_port,
            )?;
            start_from_config_path(&config, options).await?
        }
        Commands::Status { json } => {
            logging::init_default()?;
            commands::node_status(json).await?;
        }
        Commands::Peers => {
            logging::init_default()?;
            commands::node_peers().await?;
        }
        Commands::Ping { target, count } => {
            logging::init_default()?;
            commands::node_ping(&target, count).await?;
        }
        Commands::Stats => {
            logging::init_default()?;
            commands::node_stats().await?;
        }
        Commands::Route { interface, all } => {
            logging::init_default()?;
            commands::route(&interface, all).await?;
        }
        Commands::Logs {
            lines,
            file,
            service,
        } => {
            logging::init_default()?;
            commands::logs(file.as_deref(), &service, lines).await?;
        }
        Commands::QuicInfo => {
            logging::init_default()?;
            commands::quic_info().await?;
        }
        Commands::RegisterCheck {
            coordinator,
            key,
            p2p_port,
            timeout_secs,
        } => {
            logging::init_default()?;
            commands::register_check(&coordinator, &key, p2p_port, timeout_secs).await?;
        }
        _ => unreachable!("run_runtime_command received non-runtime command"),
    }

    Ok(())
}

#[cfg(target_os = "linux")]
async fn run_lxt_rescue_command(command: Commands) -> Result<()> {
    logging::init_default()?;
    match command {
        Commands::LxtRescue {
            command:
                LxtRescueCommands::Server {
                    interface,
                    tcp_port,
                    bind,
                    udp_port,
                    key_file,
                },
        } => {
            crate::lxt_rescue::run_server(crate::lxt_rescue::ServerConfig {
                interface,
                bind,
                tcp_port,
                udp_port,
                key_file,
            })
            .await?;
        }
        Commands::LxtRescue {
            command:
                LxtRescueCommands::Client {
                    interface,
                    server,
                    tcp_port,
                    udp_port,
                    hello_interval_ms,
                    tun,
                    key_file,
                },
        } => {
            crate::lxt_rescue::run_client(crate::lxt_rescue::ClientConfig {
                interface,
                server,
                tcp_port,
                udp_port,
                hello_interval: std::time::Duration::from_millis(hello_interval_ms),
                tun_name: tun,
                key_file,
            })
            .await?;
        }
        _ => unreachable!("run_lxt_rescue_command received non-LXT command"),
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
async fn run_lxt_rescue_command(_command: Commands) -> Result<()> {
    logging::init_default()?;
    anyhow::bail!("LXT TCP 握手救援当前只支持 Linux")
}

fn run_config_command(command: Commands) -> Result<()> {
    logging::init_default()?;
    match command {
        Commands::InitConfig { output } => commands::init_config(&output)?,
        Commands::EncryptConfig { input, output } => commands::encrypt_config(&input, &output)?,
        Commands::DecryptConfig { input, output } => {
            commands::decrypt_config(&input, output.as_deref())?;
        }
        Commands::Version => commands::version(),
        _ => unreachable!("run_config_command received non-config command"),
    }

    Ok(())
}

async fn start_from_config_path(_config: &str, _options: commands::RuntimeOptions) -> Result<()> {
    anyhow::bail!(
        "`start --config` 已停用：请使用 `join <票据/地址>` 或 `up [网络]`；节点数据面由后台控制进程统一管理"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;

    #[test]
    fn parses_exit_node_boolean_flag_forms() {
        let bare = Cli::try_parse_from(["fuckipv6", "--is-exit-node"]).unwrap();
        assert_eq!(bare.is_exit_node, Some(true));

        let explicit_true = Cli::try_parse_from(["fuckipv6", "--is-exit-node=true"]).unwrap();
        assert_eq!(explicit_true.is_exit_node, Some(true));

        let explicit_false = Cli::try_parse_from(["fuckipv6", "--is-exit-node=false"]).unwrap();
        assert_eq!(explicit_false.is_exit_node, Some(false));
    }

    #[test]
    fn parses_service_management_commands() {
        let cli = Cli::try_parse_from([
            "fuckipv6",
            "service",
            "install",
            "--scope",
            "user",
            "--webui-port",
            "9090",
        ])
        .unwrap();
        match cli.command {
            Some(Commands::Service(args)) => {
                assert!(matches!(
                    args.action,
                    crate::service::ServiceAction::Install
                ));
                assert_eq!(args.scope, crate::service::ServiceScope::User);
                assert_eq!(args.webui_port, 9090);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_exit_node_flag_on_host_command() {
        let cli =
            Cli::try_parse_from(["fuckipv6", "host", "--name", "relay", "--is-exit-node=true"])
                .unwrap();

        match cli.command {
            Some(Commands::Host { is_exit_node, .. }) => {
                assert_eq!(is_exit_node, Some(true));
            }
            _ => panic!("expected host command"),
        }
    }

    #[test]
    fn parses_auto_connect_boolean_flag_on_host_command() {
        let implicit = Cli::try_parse_from(["fuckipv6", "host", "--name", "benchmark"]).unwrap();
        match implicit.command {
            Some(Commands::Host { auto_connect, .. }) => assert_eq!(auto_connect, None),
            _ => panic!("expected host command"),
        }

        let disabled = Cli::try_parse_from([
            "fuckipv6",
            "host",
            "--name",
            "benchmark",
            "--auto-connect=false",
        ])
        .unwrap();
        match disabled.command {
            Some(Commands::Host { auto_connect, .. }) => assert_eq!(auto_connect, Some(false)),
            _ => panic!("expected host command"),
        }
    }

    #[test]
    fn parses_explicit_webui_alias_and_gpui_modes() {
        let webui = Cli::try_parse_from(["fuckipv6", "--webui"]).unwrap();
        assert!(webui.webui);
        assert!(!webui.headless);
        assert!(!webui.gpui);

        let legacy_alias = Cli::try_parse_from(["fuckipv6", "--gui"]).unwrap();
        assert!(legacy_alias.webui);
        assert!(!legacy_alias.gpui);

        let gpui = Cli::try_parse_from(["fuckipv6", "--gpui"]).unwrap();
        assert!(gpui.gpui);
        assert!(!gpui.webui);
    }

    #[test]
    fn rejects_conflicting_top_level_ui_modes() {
        for args in [
            ["fuckipv6", "--headless", "--webui"],
            ["fuckipv6", "--headless", "--gpui"],
            ["fuckipv6", "--webui", "--gpui"],
            ["fuckipv6", "--gui", "--gpui"],
        ] {
            let error = Cli::try_parse_from(args).expect_err("conflicting modes must fail");
            assert_eq!(error.kind(), ErrorKind::ArgumentConflict);
        }
    }

    #[test]
    fn rejects_conflicting_ui_modes_on_runtime_commands() {
        let error = Cli::try_parse_from(["fuckipv6", "host", "--headless", "--webui"])
            .expect_err("conflicting modes must fail");
        assert_eq!(error.kind(), ErrorKind::ArgumentConflict);

        let parsed = Cli::try_parse_from(["fuckipv6", "start", "--gpui"]).unwrap();
        match parsed.command {
            Some(Commands::Start { gpui, webui, .. }) => {
                assert!(gpui);
                assert!(!webui);
            }
            _ => panic!("expected start command"),
        }
    }

    #[test]
    fn gpui_mode_is_never_silently_downgraded() {
        let available = UiCapabilities {
            display: crate::ui::DisplayEnvironment::LinuxX11,
            gpui_available: true,
        };
        let options = runtime_options_with_capabilities(
            None, None, None, false, false, true, 8080, available,
        )
        .expect("available GPUI must be preserved");
        assert!(!options.headless);
        assert!(!options.gui);

        let unavailable = UiCapabilities {
            gpui_available: false,
            ..available
        };
        let error = runtime_options_with_capabilities(
            None,
            None,
            None,
            false,
            false,
            true,
            8080,
            unavailable,
        )
        .expect_err("unavailable native GPUI must fail explicitly");
        assert!(error.to_string().contains("GPUI"));
    }

    #[test]
    fn automatic_gpui_command_options_do_not_select_webui() {
        let capabilities = UiCapabilities {
            display: crate::ui::DisplayEnvironment::LinuxWayland,
            gpui_available: true,
        };

        let options = runtime_options_with_capabilities(
            None,
            None,
            None,
            false,
            false,
            false,
            8080,
            capabilities,
        )
        .expect("automatic desktop mode");

        assert!(!options.headless);
        assert!(!options.gui);
        assert_eq!(
            resolve_ui_mode(UiModeRequest::Auto, capabilities)
                .expect("automatic mode")
                .effective,
            UiMode::Gpui
        );
    }

    #[test]
    fn command_ui_intents_share_the_top_level_policy() {
        for (display, gpui_available, expected) in [
            (crate::ui::DisplayEnvironment::LinuxX11, true, UiMode::Gpui),
            (
                crate::ui::DisplayEnvironment::LinuxWayland,
                true,
                UiMode::Gpui,
            ),
            (
                crate::ui::DisplayEnvironment::Headless,
                true,
                UiMode::Headless,
            ),
            (
                crate::ui::DisplayEnvironment::LinuxX11,
                false,
                UiMode::WebUi,
            ),
            (
                crate::ui::DisplayEnvironment::Headless,
                false,
                UiMode::Headless,
            ),
        ] {
            let capabilities = UiCapabilities {
                display,
                gpui_available,
            };
            let top_level = resolve_ui_mode(UiModeRequest::Auto, capabilities)
                .expect("top-level automatic mode");
            assert_eq!(top_level.effective, expected);

            for args in [
                vec!["fuckipv6", "host"],
                vec!["fuckipv6", "join", "fk://example"],
                vec!["fuckipv6", "connect"],
                vec!["fuckipv6", "up"],
                vec!["fuckipv6", "start"],
            ] {
                let cli = Cli::try_parse_from(args).expect("parse UI-capable command");
                let intent = cli
                    .command
                    .as_ref()
                    .and_then(command_ui_intent)
                    .expect("command must carry a UI intent");
                let command_mode =
                    resolve_ui_mode(intent.requested, capabilities).expect("command UI mode");
                assert_eq!(command_mode.effective, top_level.effective);
            }
        }
    }

    #[test]
    fn legacy_gui_maps_to_webui_runtime_options() {
        let capabilities = UiCapabilities {
            display: crate::ui::DisplayEnvironment::LinuxX11,
            gpui_available: false,
        };
        let options = runtime_options_with_capabilities(
            None,
            None,
            None,
            false,
            true,
            false,
            8080,
            capabilities,
        )
        .expect("legacy GUI should map to WebUI");
        assert!(!options.headless);
        assert!(options.gui);
        assert_eq!(options.webui_port, 8080);
    }

    #[test]
    fn parses_local_owner_recovery_without_secret_arguments() {
        let cli = Cli::try_parse_from(["fuckipv6", "recover-owner"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::RecoverOwner)));
        assert!(Cli::try_parse_from([
            "fuckipv6",
            "recover-owner",
            "--recovery-code",
            "must-not-be-a-cli-secret",
        ])
        .is_err());
    }

    #[test]
    fn parses_lxt_rescue_server_command() {
        let cli = Cli::try_parse_from([
            "fuckipv6",
            "lxt-rescue",
            "server",
            "--interface",
            "eth0",
            "--tcp-port",
            "443",
            "--udp-port",
            "39091",
        ])
        .unwrap();
        match cli.command {
            Some(Commands::LxtRescue {
                command:
                    LxtRescueCommands::Server {
                        interface,
                        tcp_port,
                        bind,
                        udp_port,
                        key_file,
                    },
            }) => {
                assert_eq!(interface, "eth0");
                assert_eq!(tcp_port, 443);
                assert_eq!(bind, std::net::Ipv4Addr::UNSPECIFIED);
                assert_eq!(udp_port, 39091);
                assert_eq!(
                    key_file,
                    std::path::PathBuf::from("/etc/fuckipv6/lxt-rescue.key")
                );
            }
            _ => panic!("expected LXT rescue server command"),
        }
    }

    #[test]
    fn parses_lxt_rescue_client_command() {
        let cli = Cli::try_parse_from([
            "fuckipv6",
            "lxt-rescue",
            "client",
            "--interface",
            "eth0",
            "--server",
            "198.51.100.10",
            "--hello-interval-ms",
            "750",
            "--tun",
            "lxt0",
        ])
        .unwrap();
        match cli.command {
            Some(Commands::LxtRescue {
                command:
                    LxtRescueCommands::Client {
                        server,
                        hello_interval_ms,
                        tun,
                        ..
                    },
            }) => {
                assert_eq!(server, std::net::Ipv4Addr::new(198, 51, 100, 10));
                assert_eq!(hello_interval_ms, 750);
                assert_eq!(tun, "lxt0");
            }
            _ => panic!("expected LXT rescue client command"),
        }
    }
}
