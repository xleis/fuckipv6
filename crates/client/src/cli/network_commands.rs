use crate::cli::commands::{self, RuntimeOptions};
use crate::cli::RecoveryReasonArg;
use crate::config_manager::ConfigManager;
use crate::node::{
    NodeAdminRecoveryStatusCommand, NodeCommand, NodeCommandOutcome, NodeCreateNetworkCommand,
    NodeExecuteAdminRecoveryCommand, NodeJoinNetworkCommand, NodeNetworkSelection,
    NodeRevokeAdminRecoveryCommand, NodeRoutingMode, NodeSubmitAdminRecoveryCommand,
    NodeUpdateRoutingCommand,
};
use crate::signaling::client::SignalingClient;
use anyhow::{Context, Result};
use fuckipv6_common::domain::{
    AdminRecoveryApproval, AdminRecoveryImpact, AdminRecoveryProposal, AdminRecoveryReason,
    AdminRecoveryRequest, AdminRecoveryRequestId, AdminRecoveryRevocationPlan, AdminRecoveryState,
    AdminRecoveryStatusView, AdminRecoveryTarget, ContentHash, NetworkJoinPreview,
    NetworkJoinPreviewRequest, NetworkRecord, NodeId, ADMIN_RECOVERY_MAX_VALIDITY_MS,
};
use fuckipv6_common::{NatType, NetworkTicket};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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

const ADMIN_RECOVERY_APPROVAL_FILE_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminRecoveryApprovalFile {
    schema_version: u16,
    request_id: AdminRecoveryRequestId,
    request_digest: ContentHash,
    approval: AdminRecoveryApproval,
}

#[allow(clippy::too_many_arguments)]
pub fn recovery_request(
    network_name: &str,
    owner: &str,
    administrators: Vec<String>,
    target_members: Vec<String>,
    reason: RecoveryReasonArg,
    validity_ms: u64,
    nonce: Option<String>,
    output: &Path,
) -> Result<()> {
    if validity_ms == 0 || validity_ms > ADMIN_RECOVERY_MAX_VALIDITY_MS {
        anyhow::bail!("恢复请求有效期必须在 1..={ADMIN_RECOVERY_MAX_VALIDITY_MS} ms 之间");
    }

    let manager = ConfigManager::new()?;
    let network = load_recovery_network(&manager, network_name)?;
    let identity = manager.load_node_identity()?;
    let owner = NodeId::parse(owner.to_string())?;
    let administrators = parse_node_ids(administrators)?;
    let mut members = if target_members.is_empty() {
        network.members.iter().cloned().collect::<BTreeSet<_>>()
    } else {
        parse_node_ids(target_members)?
            .into_iter()
            .collect::<BTreeSet<_>>()
    };
    members.insert(owner.clone());
    members.extend(administrators.iter().cloned());
    let target = AdminRecoveryTarget {
        owner,
        administrators,
        members: members.into_iter().collect(),
    };
    let issued_at_unix_ms = unix_time_ms().max(1);
    let expires_at_unix_ms = issued_at_unix_ms
        .checked_add(validity_ms)
        .context("恢复请求过期时间溢出")?;
    let nonce = nonce.unwrap_or_else(|| generated_recovery_nonce(&identity, issued_at_unix_ms));
    let proposal = AdminRecoveryProposal::new(
        &network,
        target,
        NodeId::from_public_key(&identity.public_key_bytes()),
        recovery_reason(reason),
        nonce,
        issued_at_unix_ms,
        expires_at_unix_ms,
        AdminRecoveryRevocationPlan::default(),
    )?;
    let request = AdminRecoveryRequest::new(proposal);
    write_json_exclusive(output, &request)?;

    println!("管理员恢复请求已生成");
    print_recovery_request(&request, Some(&network));
    println!("请求文件: {}", output.display());
    Ok(())
}

pub fn recovery_approve(request_path: &Path, output: &Path, confirmed: bool) -> Result<()> {
    let request: AdminRecoveryRequest = read_json(request_path)?;
    request.validate_structure()?;
    print_recovery_request(&request, None);
    confirm_action(
        "请核对上述请求摘要。确认用本机节点身份签发审批？输入 yes: ",
        confirmed,
    )?;

    let manager = ConfigManager::new()?;
    let identity = manager.load_node_identity()?;
    let approval = AdminRecoveryApproval::signed(
        &request.proposal,
        &identity,
        unix_time_ms().max(request.proposal.issued_at_unix_ms),
    )?;
    let approval_file = AdminRecoveryApprovalFile {
        schema_version: ADMIN_RECOVERY_APPROVAL_FILE_SCHEMA_VERSION,
        request_id: request.proposal.request_id.clone(),
        request_digest: request.proposal.request_digest(),
        approval,
    };
    write_json_exclusive(output, &approval_file)?;

    println!("恢复请求审批已签发");
    println!("    请求 ID: {}", approval_file.request_id);
    println!("    审批人 Node ID: {}", approval_file.approval.signer);
    println!(
        "    公钥指纹: {}",
        public_key_fingerprint(&approval_file.approval)
    );
    println!("审批文件: {}", output.display());
    Ok(())
}

pub fn recovery_verify(
    request_path: &Path,
    approval_paths: &[PathBuf],
    network_name: Option<&str>,
    json: bool,
) -> Result<()> {
    let request = load_recovery_request_with_approvals(request_path, approval_paths)?;
    let now_unix_ms = unix_time_ms().max(1);
    let verification = if let Some(network_name) = network_name {
        let manager = ConfigManager::new()?;
        let network = load_recovery_network(&manager, network_name)?;
        verify_recovery_request_against_network(&request, &network, now_unix_ms)?
    } else {
        RecoveryVerification::structural(&request)
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&verification)?);
    } else {
        print_recovery_request(&request, None);
        print_recovery_verification(&verification);
    }
    if verification.network_checked && !verification.threshold_met {
        anyhow::bail!("恢复审批数量未达到服务端要求的阈值");
    }
    Ok(())
}

pub async fn recovery_submit(
    request_path: &Path,
    approval_paths: &[PathBuf],
    network_name: Option<&str>,
    confirmed: bool,
    json: bool,
) -> Result<()> {
    let request = load_recovery_request_with_approvals(request_path, approval_paths)?;
    let manager = ConfigManager::new()?;
    let (network_name, network) =
        resolve_recovery_network(&manager, network_name, &request.proposal.network_id)?;
    let now_unix_ms = unix_time_ms().max(1);
    let verification = verify_recovery_request_against_network(&request, &network, now_unix_ms)?;
    if !verification.threshold_met {
        anyhow::bail!("恢复审批数量未达到服务端要求的阈值");
    }

    print_recovery_request(&request, Some(&network));
    print_recovery_verification(&verification);
    confirm_action(
        "提交后请求将进入 Coordinator 权威审计账本，但不会自动执行。确认提交？输入 yes: ",
        confirmed,
    )?;

    let session = commands::cli_node_session().await?;
    let outcome = commands::execute_cli_node_command(
        &session,
        NodeCommand::SubmitAdminRecovery(NodeSubmitAdminRecoveryCommand {
            network_name,
            request,
        }),
    )
    .await;
    session.shutdown();
    let status = admin_recovery_status_outcome(outcome?)?;
    print_recovery_status(&status, json)?;
    Ok(())
}

pub async fn recovery_status(network_name: &str, request_id: &str, json: bool) -> Result<()> {
    let request_id = AdminRecoveryRequestId::parse(request_id.to_string())?;
    let session = commands::cli_node_session().await?;
    let outcome = commands::execute_cli_node_command(
        &session,
        NodeCommand::GetAdminRecoveryStatus(NodeAdminRecoveryStatusCommand {
            network_name: network_name.to_string(),
            request_id,
        }),
    )
    .await;
    session.shutdown();
    let status = admin_recovery_status_outcome(outcome?)?;
    print_recovery_status(&status, json)
}

pub async fn recovery_revoke(
    network_name: &str,
    request_id: &str,
    reason: &str,
    confirmed: bool,
    json: bool,
) -> Result<()> {
    let request_id = AdminRecoveryRequestId::parse(request_id.to_string())?;
    confirm_action(
        "撤销只适用于 Pending 请求；Executed 请求不可撤销。确认撤销？输入 yes: ",
        confirmed,
    )?;
    let session = commands::cli_node_session().await?;
    let outcome = commands::execute_cli_node_command(
        &session,
        NodeCommand::RevokeAdminRecovery(NodeRevokeAdminRecoveryCommand {
            network_name: network_name.to_string(),
            request_id,
            reason: reason.to_string(),
        }),
    )
    .await;
    session.shutdown();
    let status = admin_recovery_status_outcome(outcome?)?;
    print_recovery_status(&status, json)
}

pub async fn recovery_execute(
    network_name: &str,
    request_id: &str,
    confirmed: bool,
    json: bool,
) -> Result<()> {
    let request_id = AdminRecoveryRequestId::parse(request_id.to_string())?;
    let session = commands::cli_node_session().await?;
    let current = commands::execute_cli_node_command(
        &session,
        NodeCommand::GetAdminRecoveryStatus(NodeAdminRecoveryStatusCommand {
            network_name: network_name.to_string(),
            request_id: request_id.clone(),
        }),
    )
    .await;
    let current = match current {
        Ok(outcome) => admin_recovery_status_outcome(outcome)?,
        Err(error) => {
            session.shutdown();
            return Err(error);
        }
    };
    print_recovery_status(&current, json)?;
    if !current.can_execute {
        session.shutdown();
        anyhow::bail!(
            "服务端当前不允许执行该请求: {}",
            current
                .denial_key
                .as_deref()
                .unwrap_or("admin_recovery.error.not_executable")
        );
    }
    if let Err(error) = confirm_action(
        "执行后将立即替换 Owner/管理员并撤销旧会话；当前会话将失效。确认执行？输入 yes: ",
        confirmed,
    ) {
        session.shutdown();
        return Err(error);
    }

    let outcome = commands::execute_cli_node_command(
        &session,
        NodeCommand::ExecuteAdminRecovery(NodeExecuteAdminRecoveryCommand {
            network_name: network_name.to_string(),
            request_id: request_id.clone(),
        }),
    )
    .await;
    match outcome {
        Ok(outcome) => {
            let status = admin_recovery_status_outcome(outcome)?;
            session.shutdown();
            print_recovery_status(&status, json)
        }
        Err(execute_error) => {
            // An execute Ack can be lost after the Coordinator committed the
            // roster change. Never infer the result locally: re-read the
            // authoritative status instead.
            let status = commands::execute_cli_node_command(
                &session,
                NodeCommand::GetAdminRecoveryStatus(NodeAdminRecoveryStatusCommand {
                    network_name: network_name.to_string(),
                    request_id,
                }),
            )
            .await;
            session.shutdown();
            match status {
                Ok(outcome) => {
                    let status = admin_recovery_status_outcome(outcome)?;
                    eprintln!(
                        "执行响应未确认，已查询 Coordinator 权威状态；不得据此重复执行同一请求: {execute_error:#}"
                    );
                    print_recovery_status(&status, json)
                }
                Err(status_error) => Err(execute_error.context(format!(
                    "执行响应丢失，且无法查询权威状态: {status_error:#}"
                ))),
            }
        }
    }
}

#[derive(Debug, Serialize)]
struct RecoveryVerification {
    request_id: AdminRecoveryRequestId,
    network_id: fuckipv6_common::domain::NetworkId,
    structural_valid: bool,
    network_checked: bool,
    snapshot_matches: Option<bool>,
    expired: bool,
    signature_count: u32,
    required_signatures: Option<u32>,
    threshold_met: bool,
    participants: Vec<NodeId>,
}

impl RecoveryVerification {
    fn structural(request: &AdminRecoveryRequest) -> Self {
        let mut participants = request
            .approvals
            .iter()
            .map(|approval| approval.signer.clone())
            .collect::<Vec<_>>();
        participants.sort();
        participants.dedup();
        Self {
            request_id: request.proposal.request_id.clone(),
            network_id: request.proposal.network_id.clone(),
            structural_valid: true,
            network_checked: false,
            snapshot_matches: None,
            expired: request.proposal.is_expired(unix_time_ms().max(1)),
            signature_count: request.approvals.len() as u32,
            required_signatures: None,
            threshold_met: false,
            participants,
        }
    }
}

fn verify_recovery_request_against_network(
    request: &AdminRecoveryRequest,
    network: &NetworkRecord,
    now_unix_ms: u64,
) -> Result<RecoveryVerification> {
    request.validate_structure()?;
    if request.proposal.network_id != network.network_id {
        anyhow::bail!("恢复请求属于另一个网络");
    }
    let snapshot_matches = request.proposal.snapshot.matches_network(network);
    if !snapshot_matches {
        anyhow::bail!("恢复请求的 roster 快照已过期");
    }
    let expired = request.proposal.is_expired(now_unix_ms);
    if expired {
        anyhow::bail!("恢复请求已过期");
    }
    let signature_count = request.trusted_approval_count(network);
    let required_signatures = request.proposal.required_approvals(network);
    let mut participants = request
        .approvals
        .iter()
        .filter(|approval| {
            if network.recovery_authorities.is_empty() {
                approval.is_current_authority(&request.proposal.snapshot)
            } else {
                approval.is_offline_trustee(network)
            }
        })
        .map(|approval| approval.signer.clone())
        .collect::<Vec<_>>();
    participants.sort();
    participants.dedup();
    Ok(RecoveryVerification {
        request_id: request.proposal.request_id.clone(),
        network_id: request.proposal.network_id.clone(),
        structural_valid: true,
        network_checked: true,
        snapshot_matches: Some(snapshot_matches),
        expired,
        signature_count,
        required_signatures: Some(required_signatures),
        threshold_met: signature_count >= required_signatures,
        participants,
    })
}

fn load_recovery_request_with_approvals(
    request_path: &Path,
    approval_paths: &[PathBuf],
) -> Result<AdminRecoveryRequest> {
    let mut request: AdminRecoveryRequest = read_json(request_path)?;
    request.validate_structure()?;
    for path in approval_paths {
        let approval_file: AdminRecoveryApprovalFile = read_json(path)?;
        if approval_file.schema_version != ADMIN_RECOVERY_APPROVAL_FILE_SCHEMA_VERSION {
            anyhow::bail!(
                "不支持的恢复审批文件版本: {} ({})",
                approval_file.schema_version,
                path.display()
            );
        }
        if approval_file.request_id != request.proposal.request_id
            || approval_file.request_digest != request.proposal.request_digest()
        {
            anyhow::bail!("审批文件与恢复请求不匹配: {}", path.display());
        }
        if let Some(existing) = request
            .approvals
            .iter()
            .find(|approval| approval.signer == approval_file.approval.signer)
        {
            if existing == &approval_file.approval {
                continue;
            }
            anyhow::bail!("同一审批人提交了不同的恢复审批: {}", path.display());
        }
        request.add_approval(approval_file.approval)?;
    }
    request.validate_structure()?;
    Ok(request)
}

fn resolve_recovery_network(
    manager: &ConfigManager,
    requested_name: Option<&str>,
    network_id: &fuckipv6_common::domain::NetworkId,
) -> Result<(String, NetworkRecord)> {
    if let Some(network_name) = requested_name {
        let network = load_recovery_network(manager, network_name)?;
        if &network.network_id != network_id {
            anyhow::bail!("本地网络 {network_name} 与恢复请求的 network_id 不匹配");
        }
        return Ok((network_name.to_string(), network));
    }

    let mut matches = Vec::new();
    for saved in manager.list_networks()? {
        if let Ok(network) = manager.load_network_record(&saved.name) {
            if &network.network_id == network_id {
                matches.push((saved.name, network));
            }
        }
    }
    match matches.len() {
        0 => anyhow::bail!("未找到与恢复请求 network_id 匹配的本地网络"),
        1 => Ok(matches.remove(0)),
        _ => anyhow::bail!("多个本地网络匹配该 network_id，请显式提供 --network"),
    }
}

fn load_recovery_network(manager: &ConfigManager, network_name: &str) -> Result<NetworkRecord> {
    manager
        .load_network_record(network_name)
        .with_context(|| format!("无法加载网络 {network_name} 的权威记录"))
}

fn parse_node_ids(values: Vec<String>) -> Result<Vec<NodeId>> {
    let mut ids = values
        .into_iter()
        .map(|value| NodeId::parse(value))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    ids.sort();
    ids.dedup();
    Ok(ids)
}

fn recovery_reason(reason: RecoveryReasonArg) -> AdminRecoveryReason {
    match reason {
        RecoveryReasonArg::LostCredentials => AdminRecoveryReason::LostCredentials,
        RecoveryReasonArg::CompromisedAuthority => AdminRecoveryReason::CompromisedAuthority,
        RecoveryReasonArg::DisasterRecovery => AdminRecoveryReason::DisasterRecovery,
        RecoveryReasonArg::Other => AdminRecoveryReason::Other,
    }
}

fn generated_recovery_nonce(
    identity: &fuckipv6_common::crypto::Identity,
    issued_at_unix_ms: u64,
) -> String {
    let mut seed = Vec::new();
    seed.extend_from_slice(&issued_at_unix_ms.to_le_bytes());
    seed.extend_from_slice(&std::process::id().to_le_bytes());
    seed.extend_from_slice(&identity.public_key_bytes());
    if let Ok(nanos) = SystemTime::now().duration_since(UNIX_EPOCH) {
        seed.extend_from_slice(&nanos.as_nanos().to_le_bytes());
    }
    format!("cli-{}", blake3::hash(&seed).to_hex())
}

fn admin_recovery_status_outcome(outcome: NodeCommandOutcome) -> Result<AdminRecoveryStatusView> {
    match outcome {
        NodeCommandOutcome::AdminRecoveryStatus(status) => Ok(*status),
        other => anyhow::bail!("节点命令返回了意外的恢复状态: {other:?}"),
    }
}

fn print_recovery_request(request: &AdminRecoveryRequest, network: Option<&NetworkRecord>) {
    let proposal = &request.proposal;
    println!("管理员恢复请求");
    println!("    请求 ID: {}", proposal.request_id);
    println!("    网络 ID: {}", proposal.network_id);
    println!("    发起人: {}", proposal.initiated_by);
    println!("    原因: {:?}", proposal.reason);
    println!("    网络 revision: {}", proposal.snapshot.network_revision);
    println!("    快照摘要: {}", proposal.snapshot.roster_digest.as_str());
    println!(
        "    Owner: {} -> {}",
        proposal.snapshot.owner, proposal.target.owner
    );
    println!(
        "    管理员: [{}] -> [{}]",
        join_ids(&proposal.snapshot.administrators),
        join_ids(&proposal.target.administrators)
    );
    println!(
        "    成员数: {} -> {}",
        proposal.snapshot.members.len(),
        proposal.target.members.len()
    );
    let impact = network
        .map(|network| AdminRecoveryImpact::from_network(network, &proposal.target))
        .unwrap_or_else(|| AdminRecoveryImpact {
            owner_before: proposal.snapshot.owner.clone(),
            owner_after: proposal.target.owner.clone(),
            administrators_added: proposal
                .target
                .administrators
                .iter()
                .filter(|id| !proposal.snapshot.administrators.contains(id))
                .cloned()
                .collect(),
            administrators_removed: proposal
                .snapshot
                .administrators
                .iter()
                .filter(|id| !proposal.target.administrators.contains(id))
                .cloned()
                .collect(),
            members_added: proposal
                .target
                .members
                .iter()
                .filter(|id| !proposal.snapshot.members.contains(id))
                .cloned()
                .collect(),
            members_removed: proposal
                .snapshot
                .members
                .iter()
                .filter(|id| !proposal.target.members.contains(id))
                .cloned()
                .collect(),
        });
    print_recovery_impact(&impact);
    println!(
        "    有效期: {} 至 {}",
        format_unix_ms(proposal.issued_at_unix_ms),
        format_unix_ms(proposal.expires_at_unix_ms)
    );
    println!(
        "    撤销计划: 旧会话={}，邀请={}，未确认写入={}",
        proposal.revocation.old_sessions,
        proposal.revocation.pending_invitations,
        proposal.revocation.unconfirmed_writes
    );
    println!("    已有审批: {}", request.approvals.len());
}

fn print_recovery_impact(impact: &AdminRecoveryImpact) {
    println!(
        "    影响: Owner {} -> {}；管理员新增 [{}]，移除 [{}]；成员新增 [{}]，移除 [{}]",
        impact.owner_before,
        impact.owner_after,
        join_ids(&impact.administrators_added),
        join_ids(&impact.administrators_removed),
        join_ids(&impact.members_added),
        join_ids(&impact.members_removed)
    );
}

fn print_recovery_verification(verification: &RecoveryVerification) {
    println!("恢复审批复核");
    println!("    结构签名有效: {}", verification.structural_valid);
    println!(
        "    权威网络校验: {}",
        if verification.network_checked {
            "已执行"
        } else {
            "未执行（未提供 --network）"
        }
    );
    if let Some(required) = verification.required_signatures {
        println!(
            "    审批数: {}/{}，阈值: {}",
            verification.signature_count,
            required,
            if verification.threshold_met {
                "满足"
            } else {
                "未满足"
            }
        );
    } else {
        println!("    结构有效审批数: {}", verification.signature_count);
    }
    println!("    参与者: [{}]", join_ids(&verification.participants));
    println!("    已过期: {}", verification.expired);
}

fn print_recovery_status(status: &AdminRecoveryStatusView, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(status)?);
        return Ok(());
    }
    println!("管理员恢复状态");
    println!("    请求 ID: {}", status.request_id);
    println!("    网络 ID: {}", status.network_id);
    println!("    状态: {:?}", status.state);
    println!(
        "    审批: {}/{}，参与者: [{}]",
        status.signature_count,
        status.required_signatures,
        join_ids(&status.participants)
    );
    println!(
        "    有效期: {} 至 {}",
        format_unix_ms(status.issued_at_unix_ms),
        format_unix_ms(status.expires_at_unix_ms)
    );
    print_recovery_impact(&status.impact);
    println!(
        "    撤销计划: 旧会话={}，邀请={}，未确认写入={}",
        status.revocation.old_sessions,
        status.revocation.pending_invitations,
        status.revocation.unconfirmed_writes
    );
    println!("    服务端 can_execute: {}", status.can_execute);
    if let Some(denial_key) = &status.denial_key {
        println!("    拒绝原因: {denial_key}");
    }
    if status.state == AdminRecoveryState::Executed {
        println!("    结果: 恢复已由 Coordinator Ack 确认；不得重复执行");
    }
    Ok(())
}

fn join_ids(ids: &[NodeId]) -> String {
    ids.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn public_key_fingerprint(approval: &AdminRecoveryApproval) -> String {
    let digest = approval
        .signer_public_key
        .to_bytes()
        .map(|bytes| blake3::hash(&bytes))
        .unwrap_or_else(|_| blake3::hash(approval.signer_public_key.as_str().as_bytes()));
    digest.to_hex()[..16].to_string()
}

fn format_unix_ms(unix_ms: u64) -> String {
    let unix_ms = i64::try_from(unix_ms).unwrap_or(i64::MAX);
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(unix_ms)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| unix_ms.to_string())
}

fn confirm_action(prompt: &str, confirmed: bool) -> Result<()> {
    if confirmed {
        return Ok(());
    }
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        anyhow::bail!("非交互环境必须提供 --confirm");
    }
    print!("{prompt}");
    std::io::stdout().flush()?;
    let mut input = String::new();
    stdin.lock().read_line(&mut input)?;
    if input.trim() != "yes" {
        anyhow::bail!("操作已取消");
    }
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("无法读取 JSON 文件: {}", path.display()))?;
    serde_json::from_str(&content)
        .with_context(|| format!("无法解析 JSON 文件: {}", path.display()))
}

fn write_json_exclusive<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if path.exists() {
        anyhow::bail!("输出文件已存在，拒绝覆盖: {}", path.display());
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("无法创建输出目录: {}", parent.display()))?;
    }
    let content = serde_json::to_vec_pretty(value)?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&temporary, content)
        .with_context(|| format!("无法写入临时文件: {}", temporary.display()))?;
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error).with_context(|| format!("无法保存 JSON 文件: {}", path.display()));
    }
    Ok(())
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
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
