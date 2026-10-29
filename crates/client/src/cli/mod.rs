pub mod commands;
pub mod network_commands;

use anyhow::Result;
use clap::{Parser, Subcommand};
use fuckipv6_common::{decrypt_config_file, is_encrypted_config_path, ClientConfig};

use crate::logging;

#[derive(Parser)]
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

    /// 强制无头模式 (运行内置 WebUI 守护进程)
    #[arg(long)]
    pub headless: bool,

    /// 强制桌面 GUI 模式
    #[arg(long)]
    pub gui: bool,

    /// 统一控制中心 WebUI 监听端口 (默认 8080)
    #[arg(long, default_value = "8080")]
    pub webui_port: u16,
}

#[derive(Subcommand)]
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

        /// 协调服务启动后本机自动作为客户端加入
        #[arg(short, long, default_value_t = true)]
        auto_connect: bool,

        /// 路由模式: mesh-only, bypass-mainland, global-proxy
        #[arg(long)]
        routing_mode: Option<String>,

        /// 出口节点 (虚拟 IPv6, 虚拟 IPv4 或 ClientId)
        #[arg(long)]
        exit_node: Option<String>,

        /// 将本机持久标记为 Exit Node（可写 --is-exit-node=false）
        #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true", require_equals = true)]
        is_exit_node: Option<bool>,

        /// 强制无头模式 (运行内置 Axum WebUI 守护进程)
        #[arg(long)]
        headless: bool,

        /// 强制桌面 GUI 模式
        #[arg(long)]
        gui: bool,

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

        /// 强制无头模式
        #[arg(long)]
        headless: bool,

        /// 强制桌面 GUI 模式
        #[arg(long)]
        gui: bool,

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

        /// 强制无头模式
        #[arg(long)]
        headless: bool,

        /// 强制桌面 GUI 模式
        #[arg(long)]
        gui: bool,

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

        /// 强制无头模式
        #[arg(long)]
        headless: bool,

        /// 强制桌面 GUI 模式
        #[arg(long)]
        gui: bool,

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

        /// 强制无头模式 (运行内置 Axum WebUI 守护进程)
        #[arg(long)]
        headless: bool,

        /// 强制启动原生桌面 GUI (Tauri)
        #[arg(long)]
        gui: bool,

        /// WebUI 监听端口 (默认 8080)
        #[arg(long, default_value = "8080")]
        webui_port: u16,
    },

    /// 查看运行状态
    Status,

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

pub async fn run() -> Result<()> {
    let cli = Cli::parse();
    if let Some(command) = cli.command {
        run_command(command).await
    } else {
        run_default_app(
            cli.webui_port,
            cli.headless,
            cli.gui,
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

#[allow(clippy::too_many_arguments)]
pub async fn run_default_app(
    port: u16,
    headless: bool,
    gui: bool,
    host: bool,
    host_name: String,
    host_port: u16,
    host_relay_port: u16,
    auto_connect: bool,
    external_host: Option<String>,
    is_exit_node: Option<bool>,
) -> Result<()> {
    logging::init_default()?;
    let ui_mode = crate::ui::detect_ui_mode(headless, gui);

    let controller = std::sync::Arc::new(crate::webui::AppController::new());
    if let Some(enabled) = is_exit_node {
        controller
            .update_state(|state| state.is_exit_node = enabled)
            .await;
    }
    let _server_handle = crate::webui::start_app_webui(port, controller.clone()).await?;

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

    if ui_mode.is_gui() {
        crate::ui::run_desktop_gui(port).await?;
    } else {
        println!("========================================================================");
        println!("🚀 fuckipv6 统一控制中心已就绪 (无头/服务端模式)");
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

    println!("按 Ctrl+C 退出程序");
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate())?;
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = term.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await?;
    }
    tracing::info!("接收到退出信号，正在关闭应用...");
    controller.disconnect().await.ok();
    controller.stop_host().await.ok();
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
        | Commands::Status
        | Commands::Peers
        | Commands::Ping { .. }
        | Commands::Stats
        | Commands::Route { .. }
        | Commands::Logs { .. }
        | Commands::QuicInfo
        | Commands::RegisterCheck { .. }) => run_runtime_command(command).await?,
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
            gui,
            webui_port,
        } => {
            let options = commands::RuntimeOptions {
                routing_mode,
                exit_node,
                is_exit_node,
                headless,
                gui,
                webui_port,
            };
            network_commands::host(
                &name,
                port,
                relay_port,
                external_host,
                auto_connect,
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
            gui,
            webui_port,
        } => {
            let options = commands::RuntimeOptions {
                routing_mode,
                exit_node,
                is_exit_node,
                headless,
                gui,
                webui_port,
            };
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
            gui,
            webui_port,
        } => {
            let options = commands::RuntimeOptions {
                routing_mode,
                exit_node,
                is_exit_node,
                headless,
                gui,
                webui_port,
            };
            network_commands::connect(targets, all, temp, options).await?
        }
        Commands::Up {
            networks,
            all,
            routing_mode,
            exit_node,
            is_exit_node,
            headless,
            gui,
            webui_port,
        } => {
            let options = commands::RuntimeOptions {
                routing_mode,
                exit_node,
                is_exit_node,
                headless,
                gui,
                webui_port,
            };
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
            gui,
            webui_port,
        } => {
            let options = commands::RuntimeOptions {
                routing_mode,
                exit_node,
                is_exit_node,
                headless,
                gui,
                webui_port,
            };
            start_from_config_path(&config, options).await?
        }
        Commands::Status => {
            logging::init_default()?;
            commands::status().await?;
        }
        Commands::Peers => {
            logging::init_default()?;
            commands::peers().await?;
        }
        Commands::Ping { target, count } => {
            logging::init_default()?;
            commands::ping(&target, count).await?;
        }
        Commands::Stats => {
            logging::init_default()?;
            commands::stats().await?;
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

async fn start_from_config_path(config: &str, options: commands::RuntimeOptions) -> Result<()> {
    let client_config = load_client_config(config)?;
    logging::init_with_level(&client_config.log.level)?;
    tracing::info!("使用配置文件启动: {}", config);
    commands::start_with_config_and_options(config, client_config, options).await
}

fn load_client_config(path: &str) -> Result<ClientConfig> {
    if !is_encrypted_config_path(path) {
        return ClientConfig::from_file(path);
    }

    let passphrase = config_passphrase_from_env()?;
    let plaintext = decrypt_config_file(path, &passphrase)?;
    let content = String::from_utf8(plaintext)?;
    ClientConfig::from_toml_str(&content)
}

fn config_passphrase_from_env() -> Result<String> {
    std::env::var("FUCKIPV6_CONFIG_PASSPHRASE")
        .map_err(|_| anyhow::anyhow!("加密配置需要设置环境变量 FUCKIPV6_CONFIG_PASSPHRASE"))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
