use crate::cli::commands::RuntimeOptions;
use crate::config_manager::ConfigManager;
use anyhow::Result;
use fuckipv6_common::NetworkTicket;
use std::path::Path;

/// 一键创建网络并作为对等节点运行（生成邀请票据 fk://...）
pub async fn host(
    network_name: &str,
    port: u16,
    relay_port: u16,
    external_host: Option<String>,
    auto_connect: bool,
    options: RuntimeOptions,
) -> Result<()> {
    println!("🚀 正在启动对等网络协调服务...");
    let coordinator = crate::symmetric::EmbeddedCoordinator::start(
        network_name,
        port,
        relay_port,
        external_host,
        None,
    )
    .await?;

    println!();
    println!("══════════════════════════════════════════════════════════════════════════════");
    println!("  🎉 对等网络已就绪！邀请凭证 (Ticket):");
    println!("  {}", coordinator.ticket);
    println!("══════════════════════════════════════════════════════════════════════════════");
    println!("  其他节点加入命令:");
    println!("    fuckipv6 join \"{}\"", coordinator.ticket);
    println!();

    if auto_connect {
        println!("🔗 本地客户端正在自动加入网络...");
        let config_manager = ConfigManager::new()?;
        let local_url = format!("ws://127.0.0.1:{port}");
        let config_file = config_manager.create_network(network_name, &local_url, true)?;
        let config_path = path_to_string(&config_file);
        super::commands::start_with_options(&config_path, options).await?;
    } else {
        println!("按 Ctrl+C 停止协调服务");
        tokio::signal::ctrl_c().await?;
        println!("协调服务已停止");
    }

    Ok(())
}

/// 加入网络 (支持传统地址及 fk:// 邀请票据)
pub async fn join(
    server: &str,
    name: Option<&str>,
    auto_start: bool,
    options: RuntimeOptions,
) -> Result<()> {
    println!("🌐 加入网络");
    println!("─────────────────────────────────────");
    println!();

    let (coordinator_url, default_network_name) = if server.starts_with(NetworkTicket::URI_SCHEME) {
        let ticket = NetworkTicket::parse(server)?;
        println!("🎫 检测到网络邀请凭证 (Ticket):");
        println!("  网络: {}", ticket.network_name);
        println!("  端点: {}", ticket.endpoint);
        (ticket.websocket_url(), ticket.network_name)
    } else {
        let url = normalize_server_url(server);
        let net = ConfigManager::extract_network_name(&url);
        (url, net)
    };

    println!("服务器地址: {}", coordinator_url);

    // 确定网络名称
    let config_manager = ConfigManager::new()?;
    let network_name = name.map(|s| s.to_string()).unwrap_or(default_network_name);

    println!("📝 网络名称: {}", network_name);

    // 检查网络是否已存在
    if config_manager.network_exists(&network_name) {
        if auto_start {
            println!(
                "🚀 检测到网络 [{}] 已存在，正在直接启动连接...",
                network_name
            );
            return up(vec![network_name], false, options).await;
        }
        println!();
        println!("⚠️  网络配置已存在: {}", network_name);
        println!();
        println!("选项:");
        println!(
            "  1. 使用其他网络名称: fuckipv6 join \"{}\" --name <名称>",
            server
        );
        println!("  2. 删除现有配置: fuckipv6 leave {}", network_name);
        println!("  3. 直接连接: fuckipv6 connect {}", network_name);
        anyhow::bail!("网络配置已存在");
    }

    // 创建网络配置
    println!();
    println!("📁 创建网络配置...");

    let config_file = config_manager.create_network(&network_name, &coordinator_url, true)?;

    println!("配置已保存: {}", config_file.display());
    println!("已设置为默认网络");
    println!();
    println!("网络信息:");
    println!("  名称: {}", network_name);
    println!("  服务器: {}", coordinator_url);
    println!("  配置: {}", config_file.display());
    println!();

    if auto_start {
        println!("启动连接...");
        println!();
        let config_path = path_to_string(&config_file);
        super::commands::start_with_options(&config_path, options).await?;
    } else {
        println!("下一步:");
        println!("  连接到网络: fuckipv6 connect");
        println!("  或者:       fuckipv6 connect {}", network_name);
    }

    Ok(())
}

/// 启动一个或多个网络（支持多网并发共存）
pub async fn up(networks: Vec<String>, all: bool, options: RuntimeOptions) -> Result<()> {
    if all {
        crate::multi_network::start_all_networks(options).await
    } else if !networks.is_empty() {
        crate::multi_network::start_multiple_networks(&networks, options).await
    } else {
        let config_manager = ConfigManager::new()?;
        let default_network = config_manager.get_default_network()?.ok_or_else(|| {
            anyhow::anyhow!(
                "未设置默认网络。请指定网络名称 (fuckipv6 up <名称>) 或使用 --all 启动全部网络"
            )
        })?;
        crate::multi_network::start_multiple_networks(&[default_network], options).await
    }
}

/// 停止一个或全部运行中的网络
pub async fn down(networks: Vec<String>, all: bool) -> Result<()> {
    crate::multi_network::stop_networks(&networks, all).await
}

/// 连接到网络 (支持单网或多网络并发连接)
pub async fn connect(
    targets: Vec<String>,
    all: bool,
    temp: bool,
    options: RuntimeOptions,
) -> Result<()> {
    if temp {
        // 临时连接模式
        let server = targets
            .first()
            .ok_or_else(|| anyhow::anyhow!("临时连接需要指定服务器地址或邀请凭证"))?;

        println!("🔗 临时连接到: {}", server);
        println!("⚠️  配置不会被保存");
        println!();

        // 创建临时配置文件
        let temp_dir = std::env::temp_dir().join("fuckipv6-temp");
        std::fs::create_dir_all(&temp_dir)?;

        let coordinator_url = if server.starts_with(NetworkTicket::URI_SCHEME) {
            let ticket = NetworkTicket::parse(server)?;
            ticket.websocket_url()
        } else {
            normalize_server_url(server)
        };
        let temp_config_file = temp_dir.join("temp-config.toml");

        use fuckipv6_common::ClientConfig;
        let config = ClientConfig {
            client_id: None,
            coordinator_url,
            private_key_path: temp_dir.join("temp-identity.pem"),
            network: Default::default(),
            tun: Default::default(),
            p2p: Default::default(),
            log: Default::default(),
            acl: Default::default(),
            routing: Default::default(),
            exit_node: Default::default(),
        };

        config.save_to_file(&temp_config_file)?;

        let config_path = path_to_string(&temp_config_file);
        super::commands::start_with_options(&config_path, options).await?;

        // 清理临时文件
        let _ = std::fs::remove_dir_all(&temp_dir);
    } else if all || targets.len() > 1 {
        up(targets, all, options).await?;
    } else if let Some(target) = targets.into_iter().next() {
        let config_manager = ConfigManager::new()?;
        if config_manager.network_exists(&target) {
            up(vec![target], false, options).await?;
        } else if target.starts_with(NetworkTicket::URI_SCHEME)
            || target.contains(':')
            || target.contains('.')
        {
            // 自动检测到邀请票据或服务器地址，执行加入并自动启动连接
            join(&target, None, true, options).await?;
        } else {
            up(vec![target], false, options).await?;
        }
    } else {
        up(Vec::new(), false, options).await?;
    }

    Ok(())
}

/// 列出所有网络
pub fn list_networks() -> Result<()> {
    let config_manager = ConfigManager::new()?;
    let networks = config_manager.list_networks()?;
    let default_network = config_manager.get_default_network()?;

    println!("📋 已保存的网络配置");
    println!("─────────────────────────────────────");
    println!();

    if networks.is_empty() {
        println!("  (空)");
        println!();
        println!("使用 'fuckipv6 join <服务器>' 加入网络");
    } else {
        for network in &networks {
            let is_default = default_network.as_ref() == Some(&network.name);
            let prefix = if is_default { "✓" } else { " " };

            println!("{} {}", prefix, network.name);
            println!("    服务器: {}", network.coordinator_url);
            println!("    创建时间: {}", format_time(&network.created_at));
            println!("    最后使用: {}", format_time(&network.last_used));

            if is_default {
                println!("    (默认网络)");
            }

            println!();
        }

        println!("提示:");
        println!("  连接到网络: fuckipv6 connect <网络名称>");
        println!("  连接默认网络: fuckipv6 connect");
        println!("  删除网络: fuckipv6 leave <网络名称>");
    }

    Ok(())
}

/// 删除网络配置
pub fn leave(network_name: &str) -> Result<()> {
    let config_manager = ConfigManager::new()?;

    if !config_manager.network_exists(network_name) {
        println!("网络配置不存在: {}", network_name);
        println!();
        println!("查看所有网络: fuckipv6 list");
        anyhow::bail!("网络配置不存在");
    }

    println!("🗑️  删除网络配置: {}", network_name);
    println!();
    println!("⚠️  警告: 此操作将删除以下内容:");
    println!("  - 网络配置文件");
    println!("  - 客户端密钥");
    println!("  - 所有相关数据");
    println!();

    // 简单确认
    println!("确认删除? (y/N): ");
    use std::io::{self, BufRead};
    let stdin = io::stdin();
    let mut input = String::new();
    stdin.lock().read_line(&mut input)?;

    let input = input.trim().to_lowercase();
    if input != "y" && input != "yes" {
        println!("已取消");
        return Ok(());
    }

    config_manager.delete_network(network_name)?;

    println!();
    println!("网络配置已删除: {}", network_name);

    Ok(())
}

/// 设置默认网络
pub fn set_default(network_name: &str) -> Result<()> {
    let config_manager = ConfigManager::new()?;

    if !config_manager.network_exists(network_name) {
        println!("网络配置不存在: {}", network_name);
        println!();
        println!("查看所有网络: fuckipv6 list");
        anyhow::bail!("网络配置不存在");
    }

    config_manager.set_default_network(network_name)?;
    println!("默认网络已设置为: {}", network_name);
    Ok(())
}

/// 规范化服务器地址
fn normalize_server_url(server: &str) -> String {
    let server = server.trim();

    // 如果已经包含协议，直接返回
    if server.starts_with("ws://") || server.starts_with("wss://") {
        return server.to_string();
    }

    // 添加默认协议和端口
    if server.contains(':') {
        // 已经包含端口
        format!("ws://{}", server)
    } else {
        // 添加默认端口
        format!("ws://{}:6666", server)
    }
}

/// 格式化时间
fn format_time(rfc3339: &str) -> String {
    use chrono::DateTime;

    if let Ok(dt) = DateTime::parse_from_rfc3339(rfc3339) {
        let local = dt.with_timezone(&chrono::Local);
        local.format("%Y-%m-%d %H:%M:%S").to_string()
    } else {
        rfc3339.to_string()
    }
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
