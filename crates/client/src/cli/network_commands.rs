use crate::cli::commands::{self, RuntimeOptions};
use crate::config_manager::ConfigManager;
use crate::node::{
    NodeCommand, NodeCommandOutcome, NodeCreateNetworkCommand, NodeJoinNetworkCommand,
    NodeNetworkSelection, NodeRoutingMode, NodeUpdateRoutingCommand,
};
use crate::signaling::client::SignalingClient;
use anyhow::Result;
use fuckipv6_common::domain::{NetworkJoinPreview, NetworkJoinPreviewRequest};
use fuckipv6_common::{NatType, NetworkTicket};

/// 一键创建网络并作为对等节点运行（生成邀请票据 fk://...）
pub async fn host(
    network_name: &str,
    port: u16,
    relay_port: u16,
    external_host: Option<String>,
    auto_connect: bool,
    options: RuntimeOptions,
) -> Result<()> {
    ensure_options_can_apply(auto_connect, &options)?;
    println!("🚀 正在通过节点控制面创建对等网络 [{network_name}]...");

    let session = commands::cli_node_session().await?;
    let outcome = commands::execute_cli_node_command(
        &session,
        NodeCommand::CreateNetwork(NodeCreateNetworkCommand {
            name: network_name.to_string(),
            purpose: format!("Network created from the CLI: {network_name}"),
            icon: "network".into(),
            language: "und".into(),
            region: "ZZ".into(),
            tags: Vec::new(),
            capability_declarations: Vec::new(),
            visibility: Default::default(),
            join_mode: Default::default(),
            exit_contribution: Default::default(),
            exit_contribution_scope: Default::default(),
            exit_contribution_mode: Default::default(),
            exit_selection_policy: Default::default(),
            signaling_port: port,
            relay_port,
            external_host,
            auto_connect,
        }),
    )
    .await?;
    let message = completed_message(outcome)?;
    println!("{message}");
    // CreateNetwork commits before the event subscriber necessarily observes
    // the new owner membership. Refresh through GetSnapshot so the immediately
    // following network-scoped command is authorized against the authoritative
    // state instead of the pre-create snapshot.
    commands::refresh_cli_node_snapshot(&session).await?;
    if auto_connect {
        apply_route_options(&session, &[network_name.to_string()], &options).await?;
    }
    session.shutdown();
    Ok(())
}

/// 加入网络 (支持传统地址及 fk:// 邀请票据)
pub async fn join(
    server: &str,
    name: Option<&str>,
    auto_start: bool,
    options: RuntimeOptions,
) -> Result<()> {
    ensure_options_can_apply(auto_start, &options)?;
    let network_name = resolve_join_network_name(server, name)?;
    let signed_join_preview = preview_join_ticket(server).await?;
    println!("🌐 通过网络控制面加入 [{network_name}]...");
    if server.starts_with(NetworkTicket::URI_SCHEME) {
        println!("目标: 已验签的 fk:// 票据");
    } else {
        println!("服务器地址: {server}");
    }

    let session = commands::cli_node_session().await?;
    let outcome = commands::execute_cli_node_command(
        &session,
        NodeCommand::JoinNetwork(Box::new(NodeJoinNetworkCommand {
            target: server.to_string(),
            network_name: Some(network_name.clone()),
            start: auto_start,
            discovery_record: None,
            network_id: None,
            preview_digest: None,
            network_policy_revision: None,
            privacy_policy_revision: None,
            terms_hash: None,
            acknowledged_at_unix_ms: None,
            exit_contribution_acknowledged: None,
            signed_join_preview,
        })),
    )
    .await?;
    let message = completed_message(outcome)?;
    println!("{message}");
    if auto_start {
        apply_route_options(&session, &[network_name], &options).await?;
    }
    session.shutdown();
    Ok(())
}

/// 启动一个或多个网络（支持多网并发共存）
pub async fn up(networks: Vec<String>, all: bool, options: RuntimeOptions) -> Result<()> {
    let selection = if all {
        NodeNetworkSelection::All
    } else if networks.is_empty() {
        NodeNetworkSelection::Default
    } else {
        NodeNetworkSelection::Named(networks.clone())
    };

    let session = commands::cli_node_session().await?;
    let outcome = commands::execute_cli_node_command(
        &session,
        NodeCommand::StartNetworks {
            selection: selection.clone(),
        },
    )
    .await?;
    let message = completed_message(outcome)?;
    println!("{message}");

    if has_route_options(&options) {
        let selected_names = resolve_selection_names(&selection)?;
        apply_route_options(&session, &selected_names, &options).await?;
    }
    session.shutdown();
    Ok(())
}

/// 停止一个或全部运行中的网络
pub async fn down(networks: Vec<String>, all: bool) -> Result<()> {
    let stop_all = all || networks.is_empty();
    let session = commands::cli_node_session().await?;
    let outcome = commands::execute_cli_node_command(
        &session,
        NodeCommand::StopNetworks {
            networks: if stop_all { Vec::new() } else { networks },
            all: stop_all,
        },
    )
    .await?;
    println!("{}", completed_message(outcome)?);
    session.shutdown();
    Ok(())
}

/// 连接到网络 (支持单网或多网络并发连接)
pub async fn connect(
    targets: Vec<String>,
    all: bool,
    temp: bool,
    options: RuntimeOptions,
) -> Result<()> {
    if temp {
        anyhow::bail!(
            "临时连接尚未提供可原子清理的节点命令；请先使用 `connect <票据/网络>`，配置会持久保存"
        );
    }

    if all || targets.len() > 1 {
        return up(targets, all, options).await;
    }

    let Some(target) = targets.into_iter().next() else {
        return up(Vec::new(), false, options).await;
    };

    let config_manager = ConfigManager::new()?;
    if config_manager.network_exists(&target) {
        up(vec![target], false, options).await
    } else if target.starts_with(NetworkTicket::URI_SCHEME)
        || target.contains(':')
        || target.contains('.')
    {
        join(&target, None, true, options).await
    } else {
        up(vec![target], false, options).await
    }
}

fn completed_message(outcome: NodeCommandOutcome) -> Result<String> {
    match outcome {
        NodeCommandOutcome::Completed { message } => Ok(message),
        NodeCommandOutcome::NetworkCreated(created) => Ok(created.owner_message()),
        other => anyhow::bail!("节点命令返回了意外结果: {other:?}"),
    }
}

fn ensure_options_can_apply(started: bool, options: &RuntimeOptions) -> Result<()> {
    if !started && has_route_options(options) {
        anyhow::bail!("路由与 Exit Node 覆盖需要同时启动网络；请启用自动连接/start");
    }
    Ok(())
}

fn has_route_options(options: &RuntimeOptions) -> bool {
    options.routing_mode.is_some() || options.exit_node.is_some() || options.is_exit_node.is_some()
}

async fn apply_route_options(
    session: &crate::remote_node::RemoteNodeSession,
    networks: &[String],
    options: &RuntimeOptions,
) -> Result<()> {
    if !has_route_options(options) {
        return Ok(());
    }
    if networks.is_empty() {
        anyhow::bail!("路由覆盖需要至少一个已启动网络");
    }

    let snapshot = session.state().snapshot().await;
    for network_name in networks {
        let configured = snapshot
            .configuration
            .networks
            .iter()
            .find(|network| network.network_name == *network_name);
        let mode = match options.routing_mode.as_deref() {
            Some(mode) => parse_node_routing_mode(mode)?,
            None => configured
                .map(|network| network.routing_mode)
                .unwrap_or(NodeRoutingMode::MeshOnly),
        };
        let exit_node = options
            .exit_node
            .clone()
            .or_else(|| configured.and_then(|network| network.exit_node.clone()));
        let is_exit_node = options
            .is_exit_node
            .unwrap_or_else(|| configured.is_some_and(|network| network.exit_node_enabled));

        let outcome = commands::execute_cli_node_command(
            session,
            NodeCommand::UpdateRouting(NodeUpdateRoutingCommand {
                network_name: network_name.clone(),
                mode,
                exit_node,
                is_exit_node,
            }),
        )
        .await?;
        println!("{}", completed_message(outcome)?);
    }
    Ok(())
}

async fn preview_join_ticket(server: &str) -> Result<Option<NetworkJoinPreview>> {
    if !server.starts_with(NetworkTicket::URI_SCHEME) {
        return Ok(None);
    }
    let ticket = NetworkTicket::parse(server)?;
    let config_manager = ConfigManager::new()?;
    let identity = config_manager.load_node_identity()?;
    let signaling =
        SignalingClient::connect(&ticket.websocket_url(), identity, 0, NatType::Unknown).await?;
    let preview = signaling
        .preview_network_join(NetworkJoinPreviewRequest {
            coordinator_public_key: ticket.pubkey,
            network_name: ticket.network_name.clone(),
        })
        .await;
    let close_result = signaling.close().await;
    let preview = preview?;
    close_result?;

    println!("📋 已验签加入预览");
    println!(
        "    网络: {} ({})",
        preview.network_name, preview.network_id
    );
    println!("    加入模式: {:?}", preview.join_mode);
    println!(
        "    Exit 贡献: {:?} / {:?}，加入承诺: {}",
        preview.terms.exit_contribution_mode,
        preview.terms.exit_contribution_scope,
        preview
            .terms
            .exit_contribution_mode
            .requires_join_commitment()
    );
    println!(
        "    策略 revision: {}，隐私 revision: {}",
        preview.terms.network_policy_revision, preview.terms.privacy_policy_revision
    );
    println!("    条款哈希: {}", preview.terms_hash.as_str());
    println!("    预览摘要: {}", preview.preview_digest.as_str());
    println!("    有效期至: {} ms", preview.expires_at_unix_ms);
    Ok(Some(preview))
}

fn resolve_join_network_name(server: &str, configured_name: Option<&str>) -> Result<String> {
    if let Some(name) = configured_name {
        return Ok(name.to_string());
    }
    if server.starts_with(NetworkTicket::URI_SCHEME) {
        return Ok(NetworkTicket::parse(server)?.network_name);
    }
    Ok(ConfigManager::extract_network_name(&normalize_server_url(
        server,
    )))
}

fn resolve_selection_names(selection: &NodeNetworkSelection) -> Result<Vec<String>> {
    let manager = ConfigManager::new()?;
    match selection {
        NodeNetworkSelection::None => Ok(Vec::new()),
        NodeNetworkSelection::Default => manager
            .get_default_network()?
            .map(|network| vec![network])
            .ok_or_else(|| anyhow::anyhow!("未设置默认网络")),
        NodeNetworkSelection::All => Ok(manager
            .list_networks()?
            .into_iter()
            .map(|network| network.name)
            .collect()),
        NodeNetworkSelection::Named(names) => Ok(names.clone()),
    }
}

fn parse_node_routing_mode(mode: &str) -> Result<NodeRoutingMode> {
    match mode.to_lowercase().as_str() {
        "meshonly" | "mesh-only" | "mesh_only" => Ok(NodeRoutingMode::MeshOnly),
        "bypassmainland" | "bypass-mainland" | "bypass_mainland" => {
            Ok(NodeRoutingMode::BypassMainland)
        }
        "globalproxy" | "global-proxy" | "global_proxy" => Ok(NodeRoutingMode::GlobalProxy),
        other => {
            anyhow::bail!("未知路由模式: {other} (可用值: MeshOnly, BypassMainland, GlobalProxy)")
        }
    }
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
