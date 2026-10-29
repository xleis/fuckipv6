use anyhow::Result;
use clap::Parser;
use fuckipv6_common::{init_logging_with_level, CoordinatorConfig, RelayEndpoint};
use fuckipv6_coordinator::{relay::server::RelayServer, signaling::server::SignalingServer, webui};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// 配置文件路径
    #[arg(short, long, default_value = "coordinator.toml")]
    config: String,

    /// 直接指定监听地址（覆盖配置文件）
    #[arg(short, long)]
    bind: Option<String>,

    /// IP地址池
    #[arg(long, default_value = "fd00::1000/120")]
    ip_pool: String,

    /// Web 管理界面监听地址
    #[arg(long)]
    webui_bind: Option<String>,

    /// QUIC 中继服务监听地址
    #[arg(long)]
    relay_bind: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let file_config = if std::path::Path::new(&args.config).exists() {
        let config = CoordinatorConfig::from_file(&args.config)?;
        Some(config)
    } else {
        None
    };

    init_logging_with_level(
        file_config
            .as_ref()
            .map(|config| config.log.level.as_str())
            .unwrap_or("info"),
    )?;

    tracing::info!("启动 fuckipv6 协调服务器");
    if file_config.is_some() {
        tracing::info!("已从配置文件加载: {}", args.config);
    }

    // 加载配置（如果文件存在）或使用默认值
    let bind_addr = if let Some(bind) = args.bind.clone() {
        bind
    } else if let Some(config) = &file_config {
        config.signaling_bind.clone()
    } else {
        // 默认监听所有接口（IPv4/IPv6双栈）
        let default_bind = "0.0.0.0:6666".to_string();
        tracing::warn!(
            "未找到配置文件，使用默认监听地址: {} (IPv4/IPv6双栈)",
            default_bind
        );
        default_bind
    };

    let webui_bind = args
        .webui_bind
        .clone()
        .or_else(|| file_config.as_ref().map(|config| config.webui_bind.clone()))
        .unwrap_or_else(|| "0.0.0.0:8080".to_string());

    let relay_bind = args
        .relay_bind
        .clone()
        .or_else(|| file_config.as_ref().map(|config| config.relay_bind.clone()))
        .unwrap_or_else(|| "0.0.0.0:6667".to_string());

    let heartbeat_timeout_secs = file_config
        .as_ref()
        .map(|config| config.heartbeat_timeout_secs)
        .unwrap_or(120);

    tracing::info!("信令服务器监听地址: {}", bind_addr);
    tracing::info!("QUIC 中继服务器监听地址: {}", relay_bind);
    tracing::info!("Web 管理界面监听地址: {}", webui_bind);
    tracing::info!("客户端心跳超时: {} 秒", heartbeat_timeout_secs);

    if let Some(config) = &file_config {
        tracing::info!(
            "虚拟网络模式: {:?}, IPv4 网段: {}, IPv6 网段: {}",
            config.network.mode,
            if config.network.mode.is_ipv4_enabled() {
                config.network.ipv4_pool.as_str()
            } else {
                "未启用"
            },
            if config.network.mode.is_ipv6_enabled() {
                config.network.effective_ipv6_pool()
            } else {
                "未启用"
            }
        );
    } else {
        tracing::info!("IPv6 地址池: {}", args.ip_pool);
    }

    // 启动信令服务器和 QUIC 中继服务器，共享会话状态。
    let session_manager = if let Some(config) = &file_config {
        std::sync::Arc::new(
            fuckipv6_coordinator::signaling::session::SessionManager::new_with_allocation_and_blacklist(
                &config.network,
                config.blacklist.clone(),
            )?,
        )
    } else {
        std::sync::Arc::new(
            fuckipv6_coordinator::signaling::session::SessionManager::new(&args.ip_pool)?,
        )
    };

    let server = SignalingServer::new_with_manager(bind_addr, session_manager.clone());
    let relay_server =
        RelayServer::bind_with_generated_certificate(relay_bind.parse()?, session_manager.clone())?;
    let relay_pin = relay_server.certificate().public_key_pin;
    let relay_endpoint = RelayEndpoint {
        addr: relay_server.local_addr(),
        server_name: "fuckipv6.local".to_string(),
        public_key_pin: relay_pin,
    };
    let server = server.with_relay_endpoint(relay_endpoint);
    let extra_relays = file_config
        .as_ref()
        .map(|config| config.relay_endpoints.clone())
        .unwrap_or_default();
    let server = server.with_relay_endpoints(extra_relays);
    server.spawn_timeout_cleanup(heartbeat_timeout_secs);

    tracing::info!("协调服务器启动成功");
    tracing::info!("QUIC 中继证书 pin: {}", hex_pin(relay_pin));
    tracing::info!("等待客户端连接...");

    let auth_config = file_config
        .as_ref()
        .map(|config| config.auth.clone())
        .unwrap_or_default();

    if auth_config.is_auth_required() {
        tracing::info!(
            "Web 管理界面已开启 JWT 访问鉴权 (管理员: {})",
            auth_config.admin_username
        );
    } else {
        tracing::info!("Web 管理界面当前运行于免密模式 (未配置 admin_password)");
    }

    let config_file_path = Some(args.config);
    tokio::try_join!(
        server.run(),
        relay_server.run(),
        webui::run_with_options(webui_bind, session_manager, auth_config, config_file_path),
    )?;

    Ok(())
}

fn hex_pin(pin: [u8; 32]) -> String {
    pin.iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("")
}
