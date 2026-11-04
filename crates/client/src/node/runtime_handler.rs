use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::{watch, Mutex};

use fuckipv6_common::domain::{
    CapabilityKind, InvitationCreateRequest, InvitationPreview, InvitationRedeemRequest,
    InvitationRedemption, InvitationState, JoinApplicantProfile, JoinRequest,
    JoinRequestGrantPreview, JoinRequestId, JoinTermsAck, MembershipRole, MembershipTerms,
    NetworkDiscoveryRecord, NetworkId, NetworkMembership, NetworkProfileWarning,
    NetworkProfileWarningCode, NetworkRecord, NodeId, OperationId, JOIN_REQUEST_DEFAULT_EXPIRY_MS,
};
use fuckipv6_common::{Identity, JoinRequestBatchReviewItem};

use super::runtime_control::{network_setup_problem_code, RestartSummary};
use super::state::{NodeNetworkProfileSnapshot, NodeSignedNetworkProfileSnapshot};
use super::{NetworkRuntimeManager, RuntimeControlError, StartSummary};
use super::{
    NodeCommandContext, NodeCommandFuture, NodeCommandHandler, NodeCommandOutcome,
    NodeCommandRequest, NodeCommandResult, NodePeerProbeSnapshot, NodeProblem, NodeProblemCode,
    NodeStateStore, NodeValidationIssue,
};
use crate::config_manager::{ConfigManager, NetworkProfileError};
use crate::config_transaction::{ConfigurationTransactionError, ConfigurationValidationCode};
use crate::discovery_index::{
    DiscoveryIndexError, DiscoveryIndexManager, NodeDiscoveryIndexErrorCategory,
    NodeDiscoveryIndexSnapshot,
};
use crate::ipc::MultiNetworkRegistry;
use crate::network_setup::NetworkSetupStage;
use crate::node::command::{
    NodeJoinRequestListResult, NodeJoinRequestReviewAck, NodeJoinRequestReviewOutcome,
    NodeNetworkProfileCommand, NodeResubmitJoinRequestCommand, NodeSubmitJoinRequestCommand,
};
use crate::symmetric::EmbeddedServiceManager;

#[derive(Clone)]
struct NodeCommandAuditContext {
    actor: String,
    source: String,
    operation_id: Option<OperationId>,
}

/// Production handler for operations backed by the live node runtime.
///
/// Every UI adapter submits the same commands through this handler. Commands
/// whose runtime backend is not wired yet are rejected with a stable error
/// instead of reporting a false success.
pub struct NodeRuntimeHandler {
    state: NodeStateStore,
    registry: MultiNetworkRegistry,
    services: EmbeddedServiceManager,
    runtime: Option<NetworkRuntimeManager>,
    configs: Option<ConfigManager>,
    discovery: Option<DiscoveryIndexManager>,
    join_gate: Arc<Mutex<()>>,
}

impl NodeRuntimeHandler {
    pub fn new(state: NodeStateStore, registry: MultiNetworkRegistry) -> Self {
        Self::with_services(state, registry, EmbeddedServiceManager::spawn(8))
    }

    fn with_services(
        state: NodeStateStore,
        registry: MultiNetworkRegistry,
        services: EmbeddedServiceManager,
    ) -> Self {
        Self {
            state,
            registry,
            services,
            runtime: None,
            configs: None,
            discovery: None,
            join_gate: Arc::new(Mutex::new(())),
        }
    }

    fn with_runtime(
        state: NodeStateStore,
        registry: MultiNetworkRegistry,
        services: EmbeddedServiceManager,
        runtime: NetworkRuntimeManager,
        configs: ConfigManager,
    ) -> Self {
        let discovery = DiscoveryIndexManager::production(&configs);
        Self {
            state,
            registry,
            services,
            runtime: Some(runtime),
            configs: Some(configs),
            discovery: Some(discovery),
            join_gate: Arc::new(Mutex::new(())),
        }
    }

    fn execute(
        &self,
        context: Option<NodeCommandAuditContext>,
        request: NodeCommandRequest,
        cancel: Option<watch::Receiver<bool>>,
    ) -> NodeCommandFuture {
        let state = self.state.clone();
        let registry = self.registry.clone();
        let services = self.services.clone();
        let runtime = self.runtime.clone();
        let configs = self.configs.clone();
        let discovery = self.discovery.clone();
        let join_gate = self.join_gate.clone();

        Box::pin(async move {
            let request_id = request.request_id;
            if let Err(problem) = request.validate() {
                return NodeCommandResult::rejected(request_id, problem);
            }

            match request.command {
                super::NodeCommand::GetSnapshot => {
                    if let Some(discovery) = discovery.as_ref() {
                        state.replace_discovery_index(discovery.snapshot()).await;
                    }
                    if let Some(configs) = configs.as_ref() {
                        refresh_network_profiles(&state, configs).await;
                        if let Err(error) = refresh_network_memberships(&state, configs).await {
                            tracing::warn!(%error, "刷新本地网络成员关系失败，保留上一份快照");
                        }
                    }
                    let (live_tun_names, runtime_problem) = match runtime.as_ref() {
                        Some(runtime) => match runtime.live_tun_names().await {
                            Ok(names) => (Some(names), None),
                            Err(_) => (
                                None,
                                Some(
                                    super::NodeProblem::new(
                                        NodeProblemCode::Unavailable,
                                        "无法从节点运行时读取活动 TUN 信息",
                                    )
                                    .with_param("operation", "live_tun_names")
                                    .retryable(true),
                                ),
                            ),
                        },
                        None => (None, None),
                    };
                    let product_names = live_tun_names.clone().unwrap_or_default();
                    let mut capture =
                        super::network_interfaces::collect_interfaces(product_names).await;
                    if let Some(problem) = runtime_problem {
                        capture.problems.push(problem);
                    }
                    let snapshot = state
                        .refresh_interface_capture(capture, live_tun_names.as_ref())
                        .await;
                    NodeCommandResult::snapshot(request_id, snapshot)
                }
                super::NodeCommand::GetUiManifest => {
                    NodeCommandResult::ui_manifest(request_id, super::node_ui_manifest())
                }
                super::NodeCommand::SetNodeDisplayName(command) => match configs {
                    Some(configs) => {
                        set_node_display_name(&state, &configs, request_id, command).await
                    }
                    None => unavailable(request_id, "修改节点显示名需要节点配置管理器"),
                },
                super::NodeCommand::RotateNodeIdentity(command) => match configs {
                    Some(configs) => {
                        rotate_node_identity(&state, &configs, request_id, command).await
                    }
                    None => unavailable(request_id, "轮换节点身份需要节点配置管理器"),
                },
                super::NodeCommand::GetConfiguration => match configs {
                    Some(configs) => configuration_document(&configs, request_id).await,
                    None => unavailable(request_id, "读取配置需要节点配置管理器"),
                },
                super::NodeCommand::PreviewConfiguration(command) => match configs {
                    Some(configs) => {
                        preview_configuration(
                            &configs,
                            request_id,
                            command.expected_revision,
                            command.networks,
                        )
                        .await
                    }
                    None => unavailable(request_id, "配置预览需要节点配置管理器"),
                },
                super::NodeCommand::ApplyConfiguration(command) => match (configs, context) {
                    (Some(configs), Some(context)) => {
                        apply_configuration(
                            &configs,
                            request_id,
                            command.expected_revision,
                            command.networks,
                            context.actor,
                            context.source,
                        )
                        .await
                    }
                    (None, _) => unavailable(request_id, "配置应用需要节点配置管理器"),
                    (_, None) => unavailable(request_id, "配置应用缺少受信任的认证上下文"),
                },
                super::NodeCommand::RollbackConfiguration(command) => match (configs, context) {
                    (Some(configs), Some(context)) => {
                        rollback_configuration(
                            &configs,
                            request_id,
                            command.expected_revision,
                            command.target_revision,
                            context.actor,
                            context.source,
                        )
                        .await
                    }
                    (None, _) => unavailable(request_id, "配置回滚需要节点配置管理器"),
                    (_, None) => unavailable(request_id, "配置回滚缺少受信任的认证上下文"),
                },
                super::NodeCommand::GetConfigurationHistory => match configs {
                    Some(configs) => configuration_history(&configs, request_id).await,
                    None => unavailable(request_id, "读取配置历史需要节点配置管理器"),
                },
                super::NodeCommand::GetDiagnostics => match runtime {
                    Some(runtime) => diagnostics_runtime(&runtime, request_id, cancel).await,
                    None => {
                        unavailable(request_id, "读取运行时诊断需要运行中的多网络监督器控制通道")
                    }
                },
                super::NodeCommand::StopNetworks { networks, all } => match runtime {
                    Some(runtime) => {
                        stop_runtime_networks(&runtime, request_id, networks, all).await
                    }
                    None => stop_networks(&registry, request_id, &networks, all).await,
                },
                super::NodeCommand::RestartNetworks { networks, all } => match runtime {
                    Some(runtime) => {
                        restart_runtime_networks(&runtime, request_id, networks, all).await
                    }
                    None => unavailable(request_id, "重启网络需要运行中的多网络监督器控制通道"),
                },
                super::NodeCommand::RebuildTun { network_name } => match runtime {
                    Some(runtime) => rebuild_runtime_tun(&runtime, request_id, network_name).await,
                    None => unavailable(request_id, "重建 TUN 需要运行中的多网络监督器控制通道"),
                },
                super::NodeCommand::ProbePeer(command) => {
                    probe_peer(&registry, request_id, command).await
                }
                super::NodeCommand::Shutdown => {
                    let result = stop_all(&registry, request_id).await;
                    if !result.is_success() {
                        return result;
                    }
                    // The supervisor owns the runtime manager and will stop
                    // all sessions and embedded services after receiving the
                    // shutdown signal. Do not close the shared service actor
                    // from this request first.
                    if runtime.is_some() {
                        return result;
                    }
                    match services.shutdown().await {
                        Ok(_) => result,
                        Err(error) => unavailable(
                            request_id,
                            format!("网络停止指令已下发，但释放内嵌网络服务失败: {error}"),
                        ),
                    }
                }
                super::NodeCommand::StartNetworks { selection } => match runtime {
                    Some(runtime) => start_runtime_networks(&runtime, request_id, selection).await,
                    None => unavailable(
                        request_id,
                        "动态启动网络尚未接入监督器；请先完成运行时 actor 化",
                    ),
                },
                super::NodeCommand::StartNode => match runtime {
                    Some(runtime) => start_runtime_node(&runtime, request_id).await,
                    None => unavailable(request_id, "启动节点需要运行中的多网络监督器控制通道"),
                },
                super::NodeCommand::StopNode => match runtime {
                    Some(runtime) => stop_runtime_node(&runtime, request_id).await,
                    None => {
                        unavailable(request_id, "停止节点数据面需要运行中的多网络监督器控制通道")
                    }
                },
                super::NodeCommand::RestartNode => match runtime {
                    Some(runtime) => restart_runtime_node(&runtime, request_id).await,
                    None => unavailable(request_id, "重启节点需要运行中的多网络监督器控制通道"),
                },
                super::NodeCommand::CleanupResiduals { networks, all } => match runtime {
                    Some(runtime) => {
                        cleanup_runtime_residuals(&runtime, request_id, networks, all).await
                    }
                    None => {
                        unavailable(request_id, "清理运行时残留需要运行中的多网络监督器控制通道")
                    }
                },
                super::NodeCommand::CreateNetwork(command) => match runtime {
                    Some(runtime) => create_runtime_network(&runtime, request_id, command).await,
                    None => unavailable(request_id, "创建网络尚未接入内嵌网络服务 actor"),
                },
                super::NodeCommand::PreviewNetworkProfile(command) => match configs {
                    Some(configs) => {
                        preview_network_profile(&state, &configs, request_id, command).await
                    }
                    None => unavailable(request_id, "网络资料预览需要节点配置管理器"),
                },
                super::NodeCommand::UpdateNetworkProfile(command) => match (configs, context) {
                    (Some(configs), Some(context)) => {
                        update_network_profile(
                            &state,
                            &configs,
                            request_id,
                            command,
                            context.actor,
                            context.source,
                            context.operation_id,
                        )
                        .await
                    }
                    (None, _) => unavailable(request_id, "更新网络资料需要节点配置管理器"),
                    (_, None) => unavailable(request_id, "更新网络资料缺少受信任的认证上下文"),
                },
                super::NodeCommand::PublishNetworkDiscovery(command) => {
                    match (configs, discovery) {
                        (Some(configs), Some(discovery)) => {
                            let operation_id = context
                                .as_ref()
                                .and_then(|context| context.operation_id.clone());
                            let result = discovery
                                .publish(
                                    &state,
                                    &configs,
                                    &command.network_name,
                                    &command.index_url,
                                    operation_id,
                                )
                                .await;
                            discovery_command_result(request_id, result)
                        }
                        (None, _) => unavailable(request_id, "发布发现索引需要节点配置管理器"),
                        (_, None) => unavailable(request_id, "发现索引管理器尚未初始化"),
                    }
                }
                super::NodeCommand::UpdateNetworkDiscovery(command) => match (configs, discovery) {
                    (Some(configs), Some(discovery)) => {
                        let operation_id = context
                            .as_ref()
                            .and_then(|context| context.operation_id.clone());
                        let result = discovery
                            .update(
                                &state,
                                &configs,
                                &command.network_name,
                                &command.index_url,
                                operation_id,
                            )
                            .await;
                        discovery_command_result(request_id, result)
                    }
                    (None, _) => unavailable(request_id, "更新发现索引需要节点配置管理器"),
                    (_, None) => unavailable(request_id, "发现索引管理器尚未初始化"),
                },
                super::NodeCommand::UnpublishNetworkDiscovery(command) => {
                    match (configs, discovery) {
                        (Some(configs), Some(discovery)) => {
                            let operation_id = context
                                .as_ref()
                                .and_then(|context| context.operation_id.clone());
                            let result = discovery
                                .unpublish(
                                    &state,
                                    &configs,
                                    &command.network_name,
                                    &command.index_url,
                                    operation_id,
                                )
                                .await;
                            discovery_command_result(request_id, result)
                        }
                        (None, _) => unavailable(request_id, "下线发现索引需要节点配置管理器"),
                        (_, None) => unavailable(request_id, "发现索引管理器尚未初始化"),
                    }
                }
                super::NodeCommand::RefreshDiscoveryIndex { index_url } => match discovery {
                    Some(discovery) => {
                        let result = discovery.refresh(&state, index_url.as_deref()).await;
                        discovery_command_result(request_id, result)
                    }
                    None => unavailable(request_id, "发现索引管理器尚未初始化"),
                },
                super::NodeCommand::RetryDiscoveryIndex => match discovery {
                    Some(discovery) => {
                        let result = discovery.retry(&state).await;
                        discovery_command_result(request_id, result)
                    }
                    None => unavailable(request_id, "发现索引管理器尚未初始化"),
                },
                super::NodeCommand::PreviewJoinNetwork(command) => match discovery {
                    Some(discovery) => {
                        preview_discovery_join(
                            &discovery,
                            &state,
                            configs.as_ref(),
                            request_id,
                            command.network_id,
                        )
                        .await
                    }
                    None => unavailable(request_id, "加入预览需要持久化签名发现记录"),
                },
                super::NodeCommand::LeaveNetwork(command) => match (runtime, configs) {
                    (Some(runtime), Some(configs)) => {
                        leave_runtime_network(
                            &runtime, &state, &configs, &join_gate, request_id, command,
                        )
                        .await
                    }
                    _ => unavailable(request_id, "退出网络需要节点配置和运行时清理能力"),
                },
                super::NodeCommand::JoinNetwork(command) => match (runtime, discovery) {
                    (Some(runtime), discovery) => {
                        join_runtime_network(
                            &runtime,
                            discovery.as_ref(),
                            &state,
                            configs.as_ref(),
                            &join_gate,
                            request_id,
                            command,
                        )
                        .await
                    }
                    (None, _) => unavailable(request_id, "加入网络尚未接入内嵌网络服务 actor"),
                },
                super::NodeCommand::SubmitJoinRequest(command) => {
                    match (runtime, discovery, configs) {
                        (Some(runtime), Some(discovery), Some(configs)) => {
                            submit_join_request(&runtime, &discovery, &configs, request_id, command)
                                .await
                        }
                        _ => unavailable(request_id, "提交加入申请需要节点配置、发现索引和运行时"),
                    }
                }
                super::NodeCommand::ListJoinRequests(command) => match runtime {
                    Some(runtime) => {
                        list_join_requests(
                            &runtime,
                            discovery.as_ref(),
                            request_id,
                            command.network_id,
                        )
                        .await
                    }
                    None => unavailable(request_id, "列出加入申请需要运行中的节点控制面"),
                },
                super::NodeCommand::ReviewJoinRequest(command) => match runtime {
                    Some(runtime) => {
                        review_join_requests(
                            &runtime,
                            request_id,
                            command.network_id,
                            vec![command.review],
                        )
                        .await
                    }
                    None => unavailable(request_id, "审批加入申请需要运行中的节点控制面"),
                },
                super::NodeCommand::BatchReviewJoinRequests(command) => match runtime {
                    Some(runtime) => {
                        review_join_requests(
                            &runtime,
                            request_id,
                            command.network_id,
                            command.reviews,
                        )
                        .await
                    }
                    None => unavailable(request_id, "批量审批加入申请需要运行中的节点控制面"),
                },
                super::NodeCommand::WithdrawJoinRequest(command) => match runtime {
                    Some(runtime) => {
                        withdraw_join_request(
                            &runtime,
                            discovery.as_ref(),
                            request_id,
                            command.network_id,
                            command.request_id,
                        )
                        .await
                    }
                    None => unavailable(request_id, "撤回加入申请需要运行中的节点控制面"),
                },
                super::NodeCommand::ResubmitJoinRequest(command) => match runtime {
                    Some(runtime) => {
                        resubmit_join_request(&runtime, discovery.as_ref(), request_id, command)
                            .await
                    }
                    None => unavailable(request_id, "重申加入申请需要运行中的节点控制面"),
                },
                super::NodeCommand::CreateInvitation(command) => match runtime {
                    Some(runtime) => create_invitation(&runtime, request_id, command).await,
                    None => unavailable(request_id, "创建邀请需要运行中的节点控制面"),
                },
                super::NodeCommand::ListInvitations(command) => match runtime {
                    Some(runtime) => list_invitations(&runtime, request_id, command).await,
                    None => unavailable(request_id, "列出邀请需要运行中的节点控制面"),
                },
                super::NodeCommand::GetInvitationStatus(command) => match runtime {
                    Some(runtime) => get_invitation_status(&runtime, request_id, command).await,
                    None => unavailable(request_id, "读取邀请状态需要运行中的节点控制面"),
                },
                super::NodeCommand::RevokeInvitation(command) => match runtime {
                    Some(runtime) => revoke_invitation(&runtime, request_id, command).await,
                    None => unavailable(request_id, "撤销邀请需要运行中的节点控制面"),
                },
                super::NodeCommand::PreviewInvitation(command) => match runtime {
                    Some(runtime) => preview_invitation(&runtime, request_id, command).await,
                    None => unavailable(request_id, "预览邀请需要运行中的节点控制面"),
                },
                super::NodeCommand::RedeemInvitation(command) => match (runtime, configs) {
                    (Some(runtime), Some(configs)) => {
                        redeem_invitation(&runtime, &configs, &state, request_id, command).await
                    }
                    _ => unavailable(request_id, "接受邀请需要节点配置和运行中的控制面"),
                },
                super::NodeCommand::UpdateRouting(command) => {
                    let capabilities = state.snapshot().await.exit_node_capabilities;
                    if command.is_exit_node && !capabilities.supported {
                        NodeCommandResult::rejected(
                            request_id,
                            NodeProblem::new(
                                NodeProblemCode::Unsupported,
                                capabilities.unsupported_explanation(),
                            ),
                        )
                    } else {
                        match runtime {
                            Some(runtime) => {
                                update_runtime_routing(&runtime, request_id, command).await
                            }
                            None => unavailable(
                                request_id,
                                "路由热更新需要运行中的多网络监督器控制通道",
                            ),
                        }
                    }
                }
                super::NodeCommand::SetNetworkRole(command) => {
                    let capabilities = state.snapshot().await.exit_node_capabilities;
                    if command.role == super::NodeNetworkRole::ExitNode
                        && command.enabled
                        && !capabilities.supported
                    {
                        NodeCommandResult::rejected(
                            request_id,
                            NodeProblem::new(
                                NodeProblemCode::Unsupported,
                                capabilities.unsupported_explanation(),
                            ),
                        )
                    } else {
                        match runtime {
                            Some(runtime) => {
                                set_runtime_network_role(&runtime, request_id, command).await
                            }
                            None => unavailable(
                                request_id,
                                "网络角色热更新需要运行中的多网络监督器控制通道",
                            ),
                        }
                    }
                }
                super::NodeCommand::GetBlacklist { network_name } => {
                    match services.get_blacklist(network_name.clone()).await {
                        Ok(snapshot) => NodeCommandResult::completed(
                            request_id,
                            NodeCommandOutcome::Blacklist(snapshot),
                        ),
                        Err(error) => embedded_service_rejection(request_id, &network_name, error),
                    }
                }
                super::NodeCommand::BanClient(command) => {
                    match services
                        .ban_client(command.network_name.clone(), command.normalized_client_id())
                        .await
                    {
                        Ok(snapshot) => NodeCommandResult::completed(
                            request_id,
                            NodeCommandOutcome::Blacklist(snapshot),
                        ),
                        Err(error) => {
                            embedded_service_rejection(request_id, &command.network_name, error)
                        }
                    }
                }
                super::NodeCommand::UnbanClient(command) => {
                    match services
                        .unban_client(command.network_name.clone(), command.normalized_client_id())
                        .await
                    {
                        Ok(snapshot) => NodeCommandResult::completed(
                            request_id,
                            NodeCommandOutcome::Blacklist(snapshot),
                        ),
                        Err(error) => {
                            embedded_service_rejection(request_id, &command.network_name, error)
                        }
                    }
                }
                super::NodeCommand::GetNetworkAllocation { network_name } => {
                    match services.get_network_allocation(network_name.clone()).await {
                        Ok(snapshot) => NodeCommandResult::completed(
                            request_id,
                            NodeCommandOutcome::NetworkAllocation(snapshot),
                        ),
                        Err(error) => embedded_service_rejection(request_id, &network_name, error),
                    }
                }
                super::NodeCommand::UpdateNetworkAllocation(command) => {
                    let network_name = command.network_name.clone();
                    match services
                        .update_network_allocation(network_name.clone(), command.allocation)
                        .await
                    {
                        Ok(snapshot) => NodeCommandResult::completed(
                            request_id,
                            NodeCommandOutcome::NetworkAllocation(snapshot),
                        ),
                        Err(error) => embedded_service_rejection(request_id, &network_name, error),
                    }
                }
            }
        })
    }
}

impl NodeCommandHandler for NodeRuntimeHandler {
    fn handle(&self, request: NodeCommandRequest) -> NodeCommandFuture {
        self.execute(None, request, None)
    }

    fn handle_authorized(
        &self,
        context: NodeCommandContext,
        request: NodeCommandRequest,
    ) -> NodeCommandFuture {
        self.execute(
            Some(NodeCommandAuditContext {
                actor: context.actor().as_str().to_owned(),
                source: context.source().to_owned(),
                operation_id: context.audit_operation_id().cloned(),
            }),
            request,
            None,
        )
    }

    fn handle_authorized_cancellable(
        &self,
        context: NodeCommandContext,
        request: NodeCommandRequest,
        cancel: watch::Receiver<bool>,
    ) -> NodeCommandFuture {
        self.execute(
            Some(NodeCommandAuditContext {
                actor: context.actor().as_str().to_owned(),
                source: context.source().to_owned(),
                operation_id: context.audit_operation_id().cloned(),
            }),
            request,
            Some(cancel),
        )
    }
}

async fn refresh_network_profiles(state: &NodeStateStore, configs: &ConfigManager) {
    let configs = configs.clone();
    let profiles = tokio::task::spawn_blocking(move || {
        let Ok(list) = configs.list_networks() else {
            return Vec::new();
        };
        let mut profiles = list
            .into_iter()
            .filter_map(|network| configs.load_network_record(&network.name).ok())
            .map(network_profile_snapshot)
            .collect::<Vec<_>>();
        profiles.sort_by(|left, right| left.name.cmp(&right.name));
        profiles
    })
    .await;
    if let Ok(profiles) = profiles {
        state.replace_network_profiles(profiles).await;
    }
}

async fn refresh_network_memberships(
    state: &NodeStateStore,
    configs: &ConfigManager,
) -> Result<(), String> {
    let configs = configs.clone();
    let memberships = tokio::task::spawn_blocking(move || {
        let mut memberships = configs
            .list_joined_memberships()
            .map_err(|error| error.to_string())?;
        memberships.sort_by(|left, right| left.network_name.cmp(&right.network_name));
        Ok::<_, String>(
            memberships
                .iter()
                .map(network_membership_snapshot)
                .collect::<Vec<_>>(),
        )
    })
    .await
    .map_err(|error| format!("成员关系刷新任务失败: {error}"))??;
    state.replace_memberships(memberships).await;
    Ok(())
}

fn network_membership_snapshot(
    joined: &crate::config_manager::JoinedNetworkMembership,
) -> super::NodeNetworkMembershipSnapshot {
    let membership = &joined.membership;
    super::NodeNetworkMembershipSnapshot {
        network_name: joined.network_name.clone(),
        network_id: membership.network_id.to_string(),
        node_id: membership.node_id.to_string(),
        state: membership.state,
        role: membership.role,
        local_policy_revision: membership.local_policy_revision,
        network_policy_revision: membership
            .terms
            .as_ref()
            .map(|terms| terms.network_policy_revision),
        privacy_policy_revision: membership
            .terms
            .as_ref()
            .map(|terms| terms.privacy_policy_revision),
        member_limit: membership
            .terms
            .as_ref()
            .and_then(|terms| terms.member_limit),
        exit_contribution: membership
            .terms
            .as_ref()
            .map(|terms| terms.exit_contribution),
        terms_hash: membership
            .terms_ack
            .as_ref()
            .map(|ack| ack.terms_hash.as_str().to_owned()),
        acknowledged_at_unix_ms: membership
            .terms_ack
            .as_ref()
            .map(|ack| ack.acknowledged_at_unix_ms),
        created_at_unix_ms: membership.created_at_unix_ms,
        updated_at_unix_ms: membership.updated_at_unix_ms,
        suspension_reason: membership.suspension_reason.clone(),
    }
}

fn network_profile_snapshot(record: NetworkRecord) -> NodeNetworkProfileSnapshot {
    NodeNetworkProfileSnapshot {
        network_id: record.network_id.as_str().to_owned(),
        name: record.name,
        purpose: record.purpose,
        icon: record.icon,
        language: record.language,
        region: record.region,
        tags: record.tags,
        capability_declarations: record.capability_declarations,
        visibility: record.visibility,
        join_mode: record.join_mode,
        revision: record.revision,
        policy_revision: record.policy_revision,
        signed_profile: record
            .signed_profile
            .map(|signed| NodeSignedNetworkProfileSnapshot {
                schema_version: signed.document.schema_version,
                issued_at_unix_ms: signed.document.issued_at_unix_ms,
                expires_at_unix_ms: signed.document.expires_at_unix_ms,
            }),
    }
}

async fn preview_network_profile(
    state: &NodeStateStore,
    configs: &ConfigManager,
    request_id: u64,
    command: NodeNetworkProfileCommand,
) -> NodeCommandResult {
    let capabilities = state.snapshot().await.capabilities;
    let configs = configs.clone();
    let network_name = command.network_name.clone();
    let result = tokio::task::spawn_blocking(move || {
        configs.preview_network_profile(&network_name, command.expected_revision, command.update)
    })
    .await;
    match result {
        Ok(Ok(mut preview)) => {
            append_capability_warnings(
                &mut preview.warnings,
                &preview
                    .projections
                    .public_card
                    .candidate
                    .capability_declarations,
                &capabilities,
            );
            NodeCommandResult::completed(
                request_id,
                NodeCommandOutcome::NetworkProfilePreview(Box::new(preview)),
            )
        }
        Ok(Err(error)) => network_profile_rejection(request_id, error),
        Err(_) => unavailable(request_id, "网络资料预览任务异常终止"),
    }
}

#[allow(clippy::too_many_arguments)]
async fn update_network_profile(
    state: &NodeStateStore,
    configs: &ConfigManager,
    request_id: u64,
    command: NodeNetworkProfileCommand,
    actor: String,
    source: String,
    operation_id: Option<OperationId>,
) -> NodeCommandResult {
    let capabilities = state.snapshot().await.capabilities;
    let configs_for_update = configs.clone();
    let result = tokio::task::spawn_blocking(move || {
        configs_for_update.update_network_profile(
            &command.network_name,
            command.expected_revision,
            command.update,
            &actor,
            &source,
            operation_id,
        )
    })
    .await;
    match result {
        Ok(Ok(mut applied)) => {
            append_capability_warnings(
                &mut applied.warnings,
                &applied.signed_profile.document.capability_declarations,
                &capabilities,
            );
            refresh_network_profiles(state, configs).await;
            NodeCommandResult::completed(
                request_id,
                NodeCommandOutcome::NetworkProfileUpdated(Box::new(applied)),
            )
        }
        Ok(Err(error)) => network_profile_rejection(request_id, error),
        Err(_) => unavailable(request_id, "网络资料更新任务异常终止"),
    }
}

fn append_capability_warnings(
    warnings: &mut Vec<NetworkProfileWarning>,
    declared: &[CapabilityKind],
    capabilities: &super::state::NodeCapabilityRegistrySnapshot,
) {
    for capability in declared {
        if !capabilities.state(*capability).is_operational() {
            warnings.push(NetworkProfileWarning::capability(
                NetworkProfileWarningCode::CapabilityNotObserved,
                *capability,
                format!(
                    "网络资料声明了 {} 能力，但当前节点运行时未观察到该能力处于可用状态",
                    capability.wire_name()
                ),
            ));
        }
    }
    for capability in CapabilityKind::ALL {
        if capabilities.state(capability).is_operational() && !declared.contains(&capability) {
            warnings.push(NetworkProfileWarning::capability(
                NetworkProfileWarningCode::CapabilityUndeclared,
                capability,
                format!(
                    "当前节点已观察到 {} 能力，但网络公开资料未声明该能力",
                    capability.wire_name()
                ),
            ));
        }
    }
}

fn network_profile_rejection(request_id: u64, error: NetworkProfileError) -> NodeCommandResult {
    match error {
        NetworkProfileError::Invalid(message) => NodeCommandResult::rejected(
            request_id,
            NodeProblem::new(NodeProblemCode::InvalidRequest, message)
                .with_message_key("error.network_profile_invalid"),
        ),
        NetworkProfileError::RevisionConflict { expected, actual } => NodeCommandResult::rejected(
            request_id,
            NodeProblem::new(
                NodeProblemCode::Conflict,
                format!("网络资料已在别处更新；期望 revision {expected}，当前 revision {actual}"),
            )
            .with_message_key("error.network_profile_revision_conflict")
            .with_revision_conflict(expected, actual),
        ),
        NetworkProfileError::NotFound(network_name) => NodeCommandResult::rejected(
            request_id,
            NodeProblem::new(NodeProblemCode::NotFound, "网络资料不存在")
                .with_message_key("error.network_profile_not_found")
                .for_network(network_name),
        ),
        NetworkProfileError::Storage(message) => NodeCommandResult::rejected(
            request_id,
            NodeProblem::new(NodeProblemCode::Internal, message)
                .with_message_key("error.network_profile_storage")
                .retryable(true),
        ),
    }
}

async fn set_node_display_name(
    state: &NodeStateStore,
    configs: &ConfigManager,
    request_id: u64,
    command: super::NodeSetNodeDisplayNameCommand,
) -> NodeCommandResult {
    if state.snapshot().await.identity.is_none() {
        return NodeCommandResult::rejected(
            request_id,
            NodeProblem::new(NodeProblemCode::NotFound, "节点身份尚未初始化"),
        );
    }

    let configs = configs.clone();
    let persisted = tokio::task::spawn_blocking(move || {
        configs.set_node_display_name(command.display_name.as_deref())?;
        configs.node_display_name()
    })
    .await;
    let display_name = match persisted {
        Ok(Ok(display_name)) => display_name,
        Ok(Err(_)) => return configuration_io_rejection(request_id, "保存节点显示名失败"),
        Err(_) => return configuration_io_rejection(request_id, "保存节点显示名任务异常终止"),
    };

    let snapshot = state
        .update(|snapshot| {
            if let Some(identity) = snapshot.identity.as_mut() {
                identity.display_name = display_name;
            }
        })
        .await;
    NodeCommandResult::snapshot(request_id, snapshot)
}

async fn rotate_node_identity(
    state: &NodeStateStore,
    configs: &ConfigManager,
    request_id: u64,
    command: super::NodeRotateNodeIdentityCommand,
) -> NodeCommandResult {
    let snapshot = state.snapshot().await;
    let Some(identity) = snapshot.identity.as_ref() else {
        return NodeCommandResult::rejected(
            request_id,
            NodeProblem::new(NodeProblemCode::NotFound, "节点身份尚未初始化"),
        );
    };
    if identity.public_key_fingerprint != command.expected_fingerprint {
        return NodeCommandResult::rejected(
            request_id,
            NodeProblem::new(
                NodeProblemCode::Conflict,
                "节点身份已变化，请刷新后重新确认轮换",
            ),
        );
    }
    if snapshot.lifecycle.active_networks > 0 {
        return NodeCommandResult::rejected(
            request_id,
            NodeProblem::new(
                NodeProblemCode::Conflict,
                "请先停止所有活动网络再轮换节点身份",
            )
            .with_param(
                "active_networks",
                snapshot.lifecycle.active_networks.to_string(),
            ),
        );
    }

    let configs = configs.clone();
    let expected_fingerprint = command.expected_fingerprint;
    let rotation = tokio::task::spawn_blocking(move || {
        crate::owner_security::rotation::rotate_node_identity(&configs, &expected_fingerprint)
    })
    .await;
    let rotation = match rotation {
        Ok(Ok(rotation)) => rotation,
        Ok(Err(error)) => {
            let text = format!("{error:#}");
            if text.contains("身份已变化") || text.contains("所有者") {
                return NodeCommandResult::rejected(
                    request_id,
                    NodeProblem::new(
                        NodeProblemCode::Conflict,
                        "节点身份状态已变化，请刷新后再试",
                    ),
                );
            }
            return NodeCommandResult::rejected(
                request_id,
                NodeProblem::new(
                    NodeProblemCode::Internal,
                    "节点身份轮换失败；旧身份保持不变，若新密钥已写入将在下次加载时恢复事务",
                ),
            );
        }
        Err(_) => {
            return NodeCommandResult::rejected(
                request_id,
                NodeProblem::new(NodeProblemCode::Internal, "节点身份轮换任务异常终止"),
            );
        }
    };

    state
        .update(|snapshot| {
            let identity = snapshot
                .identity
                .as_mut()
                .expect("rotation preflight requires an identity snapshot");
            identity.node_id = rotation.new_node_id.clone();
            identity.public_key_fingerprint = rotation.new_fingerprint.clone();
            identity.created_at_unix_ms = Some(rotation.new_created_at_unix_ms);
        })
        .await;

    NodeCommandResult::completed(
        request_id,
        NodeCommandOutcome::IdentityRotated(super::NodeIdentityRotationSnapshot {
            old_node_id: rotation.old_node_id,
            old_fingerprint: rotation.old_fingerprint,
            new_node_id: rotation.new_node_id,
            new_fingerprint: rotation.new_fingerprint,
            created_at_unix_ms: rotation.new_created_at_unix_ms,
            owner_initialized: rotation.owner_initialized,
            owner_sessions_invalidated: rotation.owner_sessions_invalidated,
            affected_configured_network_count: rotation.affected_network_count,
        }),
    )
}

async fn configuration_document(configs: &ConfigManager, request_id: u64) -> NodeCommandResult {
    let configs = configs.clone();
    match tokio::task::spawn_blocking(move || configs.configuration_document()).await {
        Ok(Ok(document)) => {
            NodeCommandResult::completed(request_id, NodeCommandOutcome::Configuration(document))
        }
        Ok(Err(_)) => configuration_io_rejection(request_id, "读取配置失败"),
        Err(_) => configuration_io_rejection(request_id, "读取配置任务异常终止"),
    }
}

async fn preview_configuration(
    configs: &ConfigManager,
    request_id: u64,
    expected_revision: u64,
    networks: std::collections::BTreeMap<String, serde_json::Value>,
) -> NodeCommandResult {
    let configs = configs.clone();
    match tokio::task::spawn_blocking(move || {
        configs.preview_configuration(expected_revision, networks)
    })
    .await
    {
        Ok(Ok(preview)) => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::ConfigurationPreview(Box::new(preview)),
        ),
        Ok(Err(_)) => configuration_io_rejection(request_id, "配置预览失败"),
        Err(_) => configuration_io_rejection(request_id, "配置预览任务异常终止"),
    }
}

async fn apply_configuration(
    configs: &ConfigManager,
    request_id: u64,
    expected_revision: u64,
    networks: std::collections::BTreeMap<String, serde_json::Value>,
    actor: String,
    source: String,
) -> NodeCommandResult {
    let configs = configs.clone();
    match tokio::task::spawn_blocking(move || {
        configs.apply_configuration(expected_revision, networks, &actor, &source)
    })
    .await
    {
        Ok(Ok(result)) => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::ConfigurationApplied(Box::new(result)),
        ),
        Ok(Err(error)) => {
            NodeCommandResult::rejected(request_id, configuration_transaction_problem(error))
        }
        Err(_) => configuration_io_rejection(request_id, "配置应用任务异常终止"),
    }
}

async fn rollback_configuration(
    configs: &ConfigManager,
    request_id: u64,
    expected_revision: u64,
    target_revision: u64,
    actor: String,
    source: String,
) -> NodeCommandResult {
    let configs = configs.clone();
    match tokio::task::spawn_blocking(move || {
        configs.rollback_configuration(expected_revision, target_revision, &actor, &source)
    })
    .await
    {
        Ok(Ok(result)) => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::ConfigurationApplied(Box::new(result)),
        ),
        Ok(Err(error)) => {
            NodeCommandResult::rejected(request_id, configuration_transaction_problem(error))
        }
        Err(_) => configuration_io_rejection(request_id, "配置回滚任务异常终止"),
    }
}

async fn configuration_history(configs: &ConfigManager, request_id: u64) -> NodeCommandResult {
    let configs = configs.clone();
    match tokio::task::spawn_blocking(move || configs.configuration_history()).await {
        Ok(Ok(history)) => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::ConfigurationHistory(history),
        ),
        Ok(Err(_)) => configuration_io_rejection(request_id, "读取配置历史失败"),
        Err(_) => configuration_io_rejection(request_id, "读取配置历史任务异常终止"),
    }
}

fn configuration_io_rejection(request_id: u64, message: &str) -> NodeCommandResult {
    NodeCommandResult::rejected(
        request_id,
        NodeProblem::new(NodeProblemCode::Internal, message),
    )
}

fn configuration_transaction_problem(error: ConfigurationTransactionError) -> NodeProblem {
    match error {
        ConfigurationTransactionError::RevisionConflict { expected, current } => {
            NodeProblem::new(NodeProblemCode::Conflict, "配置已被其他操作修改")
                .with_param("expected_revision", expected.to_string())
                .with_param("current_revision", current.to_string())
                .with_revision_conflict(expected, current)
        }
        ConfigurationTransactionError::InvalidCandidate(preview) => {
            let issues = preview
                .validation_errors
                .into_iter()
                .map(|issue| NodeValidationIssue {
                    path: issue.path,
                    code: validation_code_name(issue.code).into(),
                    message: issue.message,
                })
                .collect::<Vec<_>>();
            NodeProblem::new(NodeProblemCode::InvalidRequest, "配置校验失败")
                .with_param("candidate_revision", preview.candidate_revision.to_string())
                .with_validation_issues(issues)
        }
        ConfigurationTransactionError::RevisionNotFound(revision) => {
            NodeProblem::new(NodeProblemCode::NotFound, "目标配置 revision 不存在")
                .with_param("target_revision", revision.to_string())
        }
        ConfigurationTransactionError::Persistence(_) => NodeProblem::new(
            NodeProblemCode::Internal,
            "配置持久化失败，旧 revision 保持不变",
        ),
    }
}

fn validation_code_name(code: ConfigurationValidationCode) -> &'static str {
    match code {
        ConfigurationValidationCode::InvalidType => "invalid_type",
        ConfigurationValidationCode::MissingRequired => "missing_required",
        ConfigurationValidationCode::UnknownNetwork => "unknown_network",
        ConfigurationValidationCode::MissingNetwork => "missing_network",
        ConfigurationValidationCode::InvalidValue => "invalid_value",
        ConfigurationValidationCode::Unsupported => "unsupported",
    }
}

fn embedded_service_rejection(
    request_id: u64,
    network_name: &str,
    error: crate::symmetric::EmbeddedServiceError,
) -> NodeCommandResult {
    use crate::symmetric::EmbeddedServiceError;

    let (code, retryable) = match &error {
        EmbeddedServiceError::InvalidRequest(_) => (NodeProblemCode::InvalidRequest, false),
        EmbeddedServiceError::NotFound(_) => (NodeProblemCode::NotFound, false),
        EmbeddedServiceError::PersistenceFailed { .. }
        | EmbeddedServiceError::OperationFailed { .. }
        | EmbeddedServiceError::ActorClosed => (NodeProblemCode::Unavailable, true),
        EmbeddedServiceError::AlreadyRunning(_)
        | EmbeddedServiceError::StartFailed { .. }
        | EmbeddedServiceError::StopFailed { .. }
        | EmbeddedServiceError::ShutdownFailed { .. } => (NodeProblemCode::Unavailable, true),
    };
    NodeCommandResult::rejected(
        request_id,
        NodeProblem::new(code, error.to_string())
            .for_network(network_name)
            .retryable(retryable),
    )
}

async fn start_runtime_networks(
    runtime: &NetworkRuntimeManager,
    request_id: u64,
    selection: super::NodeNetworkSelection,
) -> NodeCommandResult {
    match runtime.start_selection(selection).await {
        Ok(summary) if summary.requested == 0 => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::Completed {
                message: "没有需要启动的网络".into(),
            },
        ),
        Ok(summary) if !summary.failed.is_empty() => {
            NodeCommandResult::rejected(request_id, start_failure_problem(&summary, "启动网络失败"))
        }
        Ok(summary) => completed_summary(request_id, summary, "启动网络"),
        Err(error) => runtime_rejection(request_id, error),
    }
}

async fn start_runtime_node(runtime: &NetworkRuntimeManager, request_id: u64) -> NodeCommandResult {
    match runtime.start_node().await {
        Ok(summary) if !summary.failed.is_empty() => {
            NodeCommandResult::rejected(request_id, start_failure_problem(&summary, "启动节点失败"))
        }
        Ok(summary) => completed_summary(request_id, summary, "启动节点"),
        Err(error) => runtime_rejection(request_id, error),
    }
}

async fn stop_runtime_node(runtime: &NetworkRuntimeManager, request_id: u64) -> NodeCommandResult {
    match runtime.stop_node().await {
        Ok(summary) if summary.is_success() => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::Completed {
                message: summary.message(),
            },
        ),
        Ok(summary) => NodeCommandResult::rejected(
            request_id,
            NodeProblem::new(NodeProblemCode::Unavailable, summary.message()).retryable(true),
        ),
        Err(error) => runtime_rejection(request_id, error),
    }
}

async fn restart_runtime_node(
    runtime: &NetworkRuntimeManager,
    request_id: u64,
) -> NodeCommandResult {
    match runtime.restart_node().await {
        Ok(summary) if summary.failed.is_empty() => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::Completed {
                message: summary.message(),
            },
        ),
        Ok(summary) => NodeCommandResult::rejected(request_id, restart_failure_problem(&summary)),
        Err(error) => runtime_rejection(request_id, error),
    }
}

async fn diagnostics_runtime(
    runtime: &NetworkRuntimeManager,
    request_id: u64,
    cancel: Option<watch::Receiver<bool>>,
) -> NodeCommandResult {
    let result = match cancel {
        Some(cancel) => runtime.diagnostics_cancellable(cancel).await,
        None => runtime.diagnostics().await,
    };
    match result {
        Ok(diagnostics) => {
            NodeCommandResult::completed(request_id, NodeCommandOutcome::Diagnostics(diagnostics))
        }
        Err(error) => runtime_rejection(request_id, error),
    }
}

async fn cleanup_runtime_residuals(
    runtime: &NetworkRuntimeManager,
    request_id: u64,
    networks: Vec<String>,
    all: bool,
) -> NodeCommandResult {
    match runtime.cleanup_residuals(networks, all).await {
        Ok(summary) => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::Cleanup(summary.to_public_cleanup_result()),
        ),
        Err(error) => runtime_rejection(request_id, error),
    }
}

async fn create_runtime_network(
    runtime: &NetworkRuntimeManager,
    request_id: u64,
    command: super::NodeCreateNetworkCommand,
) -> NodeCommandResult {
    match runtime.create(command).await {
        Ok(result) => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::NetworkCreated(super::NodeCreatedNetwork {
                network_name: result.network_name,
                network_id: result.network_id,
                public_key: result.public_key,
                owner_role: result.owner_role,
                config_saved: result.config_saved,
                started: result.started,
                next_steps: result.next_steps,
                invitation_ticket: result.ticket,
            }),
        ),
        Err(error) => runtime_rejection(request_id, error),
    }
}

fn discovery_command_result(
    request_id: u64,
    result: Result<NodeDiscoveryIndexSnapshot, DiscoveryIndexError>,
) -> NodeCommandResult {
    match result {
        Ok(snapshot) => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::Completed {
                message: format!(
                    "发现索引状态: {}；已验证记录: {}；数据面受影响: {}",
                    snapshot.state.as_str(),
                    snapshot.records.len(),
                    snapshot.data_plane_affected
                ),
            },
        ),
        Err(error) => NodeCommandResult::rejected(request_id, discovery_problem(error)),
    }
}

fn discovery_problem(error: DiscoveryIndexError) -> NodeProblem {
    let code = match error.category {
        NodeDiscoveryIndexErrorCategory::Unsupported => NodeProblemCode::Unsupported,
        NodeDiscoveryIndexErrorCategory::Rejected => NodeProblemCode::InvalidRequest,
        NodeDiscoveryIndexErrorCategory::Storage => NodeProblemCode::Internal,
        NodeDiscoveryIndexErrorCategory::Offline
        | NodeDiscoveryIndexErrorCategory::Timeout
        | NodeDiscoveryIndexErrorCategory::InvalidResponse
        | NodeDiscoveryIndexErrorCategory::Server => NodeProblemCode::Unavailable,
    };
    NodeProblem::new(
        code,
        format!("发现索引请求失败: {}", error.category.as_str()),
    )
    .with_message_key(error.message_key())
    .with_param("discovery_error", error.category.as_str())
    .retryable(error.retryable())
}

async fn preview_discovery_join(
    discovery: &DiscoveryIndexManager,
    state: &NodeStateStore,
    configs: Option<&ConfigManager>,
    request_id: u64,
    network_id: String,
) -> NodeCommandResult {
    let already_member = join_membership_exists(state, configs, &network_id).await;
    let mut preview = match discovery
        .preview_join_record(&network_id, already_member)
        .await
    {
        Ok(preview) => preview,
        Err(error) => return NodeCommandResult::rejected(request_id, discovery_problem(error)),
    };
    let snapshot = state.snapshot().await;
    preview.locally_contributed_capabilities = snapshot
        .capabilities
        .capabilities
        .iter()
        .filter(|capability| capability.state.is_operational())
        .map(|capability| capability.kind)
        .collect();
    NodeCommandResult::completed(
        request_id,
        NodeCommandOutcome::NetworkJoinPreview(Box::new(preview)),
    )
}

async fn join_membership_exists(
    state: &NodeStateStore,
    configs: Option<&ConfigManager>,
    network_id: &str,
) -> bool {
    if state
        .snapshot()
        .await
        .memberships
        .iter()
        .any(|membership| membership.network_id == network_id)
    {
        return true;
    }
    if let Some(configs) = configs {
        let configs = configs.clone();
        let network_id = network_id.to_owned();
        if tokio::task::spawn_blocking(move || {
            configs
                .joined_membership(&network_id)
                .ok()
                .flatten()
                .is_some()
        })
        .await
        .unwrap_or(false)
        {
            return true;
        }
    }
    configs.is_some_and(|configs| {
        configs.list_networks().is_ok_and(|networks| {
            networks
                .iter()
                .any(|network| network.network_id.as_deref() == Some(network_id))
        })
    })
}

fn join_target_from_record(
    record: &fuckipv6_common::domain::NetworkDiscoveryRecord,
) -> Option<String> {
    record
        .document
        .endpoints
        .iter()
        .find(|endpoint| {
            endpoint.transport == fuckipv6_common::domain::DiscoveryTransport::Websocket
        })
        .map(|endpoint| {
            let host = endpoint.host.trim();
            let host = if host.contains(':') && !host.starts_with('[') {
                format!("[{host}]")
            } else {
                host.to_owned()
            };
            format!("ws://{host}:{}", endpoint.port)
        })
}

fn join_request_rejection(
    request_id: u64,
    message_key: &'static str,
    message: impl Into<String>,
) -> NodeCommandResult {
    NodeCommandResult::rejected(
        request_id,
        NodeProblem::new(NodeProblemCode::InvalidRequest, message).with_message_key(message_key),
    )
}

async fn approval_coordinator_url(
    discovery: Option<&DiscoveryIndexManager>,
    network_id: &NetworkId,
) -> Option<String> {
    let record = discovery?
        .approval_join_record(network_id.as_str())
        .await
        .ok()?;
    join_target_from_record(&record)
}

fn approval_join_request_from_record(
    record: &NetworkDiscoveryRecord,
    identity: &Identity,
    display_name: Option<String>,
    reason: String,
    now_unix_ms: u64,
) -> Result<JoinRequest, String> {
    let document = &record.document;
    let applicant_node_id = NodeId::from_public_key(&identity.public_key_bytes());
    let applicant = JoinApplicantProfile {
        node_id: applicant_node_id.clone(),
        display_name,
        public_key_fingerprint: Identity::public_key_fingerprint(&identity.public_key_bytes()),
    };
    let terms = MembershipTerms {
        network_policy_revision: document.policy_revision,
        privacy_policy_revision: document.privacy_policy_revision,
        exit_contribution: document.exit_contribution,
        member_limit: document.member_limit,
    };
    let grant_preview = JoinRequestGrantPreview {
        role: MembershipRole::Member,
        capabilities: document.capability_summary.clone(),
        exit_contribution: document.exit_contribution,
        file_access: document.capability_summary.contains(&CapabilityKind::File),
        grants_data_plane_access: true,
    };
    let submission_nonce = format!(
        "{}:{}",
        document.revision,
        blake3::hash(reason.as_bytes()).to_hex()
    );
    let request_id =
        JoinRequest::derived_id(&document.network_id, &applicant_node_id, &submission_nonce);
    let expires_at_unix_ms = now_unix_ms
        .saturating_add(JOIN_REQUEST_DEFAULT_EXPIRY_MS)
        .min(document.expires_at_unix_ms);
    JoinRequest::new(
        request_id,
        document.network_id.clone(),
        applicant,
        reason,
        terms,
        grant_preview,
        now_unix_ms,
        expires_at_unix_ms,
    )
    .map_err(|error| error.to_string())
}

fn apply_join_request_status(
    request: &mut JoinRequest,
    status: crate::signaling::client::JoinRequestStatusSnapshot,
    reason: Option<String>,
    now_unix_ms: u64,
) {
    request.state = status.state;
    request.revision = status.revision;
    request.updated_at_unix_ms = now_unix_ms;
    request.information_request = status.information_request;
    if let Some(reason) = reason {
        request.reason = Some(reason);
    }
    if request.state.is_terminal() {
        request.decided_at_unix_ms.get_or_insert(now_unix_ms);
        request.reason_retention_until_unix_ms.get_or_insert(
            now_unix_ms.saturating_add(fuckipv6_common::domain::JOIN_REQUEST_REASON_RETENTION_MS),
        );
    }
}

async fn create_invitation(
    runtime: &NetworkRuntimeManager,
    request_id: u64,
    command: super::NodeCreateInvitationCommand,
) -> NodeCommandResult {
    let network_id = command.network_id.clone();
    let request = InvitationCreateRequest::new(
        command.network_id,
        command.role,
        command.capabilities,
        command.max_uses,
        command.expires_in_ms,
    );
    match runtime.create_invitation(request).await {
        Ok(issued) => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::InvitationCreated(Box::new(issued)),
        ),
        Err(error) => runtime_rejection_for_network(request_id, error, network_id.to_string()),
    }
}

async fn list_invitations(
    runtime: &NetworkRuntimeManager,
    request_id: u64,
    command: super::NodeListInvitationsCommand,
) -> NodeCommandResult {
    let network_id = command.network_id;
    match runtime.list_invitations(network_id.clone()).await {
        Ok(invitations) => {
            NodeCommandResult::completed(request_id, NodeCommandOutcome::Invitations(invitations))
        }
        Err(error) => runtime_rejection_for_network(request_id, error, network_id.to_string()),
    }
}

async fn get_invitation_status(
    runtime: &NetworkRuntimeManager,
    request_id: u64,
    command: super::NodeGetInvitationStatusCommand,
) -> NodeCommandResult {
    let network_id = command.network_id;
    match runtime
        .get_invitation_status(network_id.clone(), command.invitation_id)
        .await
    {
        Ok(invitation) => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::InvitationStatus(Box::new(invitation)),
        ),
        Err(error) => runtime_rejection_for_network(request_id, error, network_id.to_string()),
    }
}

async fn revoke_invitation(
    runtime: &NetworkRuntimeManager,
    request_id: u64,
    command: super::NodeRevokeInvitationCommand,
) -> NodeCommandResult {
    let network_id = command.network_id;
    match runtime
        .revoke_invitation(
            network_id.clone(),
            command.invitation_id,
            command.expected_revision,
        )
        .await
    {
        Ok(invitation) => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::InvitationRevoked(Box::new(invitation)),
        ),
        Err(error) => runtime_rejection_for_network(request_id, error, network_id.to_string()),
    }
}

async fn preview_invitation(
    runtime: &NetworkRuntimeManager,
    request_id: u64,
    command: super::NodePreviewInvitationCommand,
) -> NodeCommandResult {
    let network_id = command.network_id;
    match runtime
        .preview_invitation(network_id.clone(), command.coordinator_url, command.code)
        .await
    {
        Ok(preview) => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::InvitationPreview(Box::new(preview)),
        ),
        Err(error) => runtime_rejection_for_network(request_id, error, network_id.to_string()),
    }
}

fn invitation_conflict(message: impl Into<String>, network_name: Option<&str>) -> NodeProblem {
    let mut problem = NodeProblem::new(NodeProblemCode::Conflict, message)
        .with_message_key("invitation.error.preview_changed")
        .retryable(false);
    if let Some(network_name) = network_name {
        problem = problem.for_network(network_name.to_owned());
    }
    problem
}

/// Bind consent to the exact preview digest and independently verify every
/// security-relevant field before the credential is sent to the Coordinator.
fn validate_invitation_preview(
    request: &InvitationRedeemRequest,
    preview: &InvitationPreview,
    now_unix_ms: u64,
) -> Result<(), NodeProblem> {
    let computed = preview.compute_digest().map_err(|error| {
        invitation_conflict(
            format!("无法验证邀请预览摘要: {error}"),
            Some(&preview.network_name),
        )
    })?;
    if request.network_id != preview.network_id {
        return Err(invitation_conflict(
            "邀请预览的网络 ID 与确认内容不一致",
            Some(&preview.network_name),
        ));
    }
    if request.invitation_id != preview.invitation_id
        || request.expected_invitation_revision != preview.invitation_revision
    {
        return Err(invitation_conflict(
            "邀请已更新，请重新预览后再确认",
            Some(&preview.network_name),
        ));
    }
    if request.preview_digest != computed || request.terms_hash != preview.terms_hash {
        return Err(invitation_conflict(
            "邀请条款或权限预览已变化，请重新预览并确认",
            Some(&preview.network_name),
        ));
    }
    if preview.state != InvitationState::Active
        || !preview.can_accept
        || preview.denial_key.is_some()
        || preview.remaining_uses == 0
    {
        return Err(invitation_conflict(
            preview
                .denial_key
                .clone()
                .unwrap_or_else(|| "邀请已过期、撤销或耗尽，无法接受".into()),
            Some(&preview.network_name),
        ));
    }
    if !matches!(
        preview.role,
        MembershipRole::Member | MembershipRole::Administrator
    ) {
        return Err(invitation_conflict(
            "邀请不能授予网络所有者身份",
            Some(&preview.network_name),
        ));
    }
    if preview
        .capabilities
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        != preview.capabilities.len()
    {
        return Err(invitation_conflict(
            "邀请能力列表包含重复项",
            Some(&preview.network_name),
        ));
    }
    if preview.creates_permanent_membership {
        return Err(invitation_conflict(
            "邀请必须创建普通成员关系，不能创建永久身份",
            Some(&preview.network_name),
        ));
    }
    if now_unix_ms
        > request
            .acknowledged_at_unix_ms
            .saturating_add(5 * 60 * 1000)
        || request.acknowledged_at_unix_ms >= preview.expires_at_unix_ms
    {
        return Err(invitation_conflict(
            "接受确认已过期，请重新预览并确认",
            Some(&preview.network_name),
        ));
    }
    Ok(())
}

fn validate_invitation_redemption(
    preview: &InvitationPreview,
    redemption: &InvitationRedemption,
) -> Result<(), NodeProblem> {
    if redemption.invitation_id != preview.invitation_id
        || redemption.network_id != preview.network_id
        || redemption.role != preview.role
        || redemption.capabilities != preview.capabilities
        || redemption.invitation_revision < preview.invitation_revision
        || redemption.network_revision < preview.network_revision
    {
        return Err(invitation_conflict(
            "协调服务器返回的成员权限与已确认预览不一致",
            Some(&preview.network_name),
        ));
    }
    if redemption.grants_permanent_membership {
        return Err(invitation_conflict(
            "协调服务器返回了永久成员身份，已拒绝写入本地",
            Some(&preview.network_name),
        ));
    }
    Ok(())
}

async fn persist_invitation_membership(
    configs: &ConfigManager,
    preview: InvitationPreview,
    acknowledged_at_unix_ms: u64,
    joined_at_unix_ms: u64,
) -> Result<(crate::config_manager::JoinedNetworkMembership, bool), String> {
    let configs = configs.clone();
    tokio::task::spawn_blocking(move || {
        let identity = configs
            .load_node_identity()
            .map_err(|error| error.to_string())?;
        let node_id = NodeId::from_public_key(&identity.public_key_bytes());
        let mut membership = NetworkMembership::new(
            preview.network_id.clone(),
            node_id,
            preview.role,
            acknowledged_at_unix_ms,
        );
        membership
            .begin_apply(
                preview.terms.clone(),
                JoinTermsAck {
                    terms_hash: preview.terms_hash.clone(),
                    acknowledged_at_unix_ms,
                },
                acknowledged_at_unix_ms,
            )
            .map_err(|error| error.to_string())?;
        membership
            .accept(joined_at_unix_ms.max(acknowledged_at_unix_ms))
            .map_err(|error| error.to_string())?;
        configs
            .upsert_joined_membership(&preview.network_name, membership)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("邀请成员关系写入任务失败: {error}"))?
}

async fn redeem_invitation(
    runtime: &NetworkRuntimeManager,
    configs: &ConfigManager,
    state: &NodeStateStore,
    request_id: u64,
    command: super::NodeRedeemInvitationCommand,
) -> NodeCommandResult {
    let network_id = command.request.network_id.clone();
    let preview = match runtime
        .preview_invitation(
            network_id.clone(),
            command.coordinator_url.clone(),
            command.request.code.clone(),
        )
        .await
    {
        Ok(preview) => preview,
        Err(error) => {
            return runtime_rejection_for_network(request_id, error, network_id.to_string())
        }
    };
    if let Err(problem) = validate_invitation_preview(&command.request, &preview, unix_time_ms()) {
        return NodeCommandResult::rejected(request_id, problem);
    }

    let conflicting_config = configs.list_networks().ok().and_then(|networks| {
        networks.into_iter().find(|network| {
            network.name == preview.network_name
                && network.network_id.as_deref() != Some(preview.network_id.as_str())
        })
    });
    if conflicting_config.is_some() {
        return NodeCommandResult::rejected(
            request_id,
            invitation_conflict(
                "本地已存在同名但不同网络 ID 的配置",
                Some(&preview.network_name),
            ),
        );
    }

    let redemption = match runtime
        .redeem_invitation(command.coordinator_url.clone(), command.request.clone())
        .await
    {
        Ok(redemption) => redemption,
        Err(error) => {
            return runtime_rejection_for_network(request_id, error, network_id.to_string())
        }
    };
    if let Err(problem) = validate_invitation_redemption(&preview, &redemption) {
        return NodeCommandResult::rejected(request_id, problem);
    }

    match persist_invitation_membership(
        configs,
        preview.clone(),
        command.request.acknowledged_at_unix_ms,
        redemption.joined_at_unix_ms,
    )
    .await
    {
        Ok(_) => {}
        Err(error) => {
            return NodeCommandResult::rejected(
                request_id,
                NodeProblem::new(
                    NodeProblemCode::Unavailable,
                    format!("邀请已兑换，但保存本地普通成员关系失败: {error}"),
                )
                .with_message_key("invitation.error.storage")
                .with_param("error_kind", "storage")
                .retryable(true)
                .for_network(preview.network_name.clone()),
            )
        }
    }
    if let Err(error) = refresh_network_memberships(state, configs).await {
        tracing::warn!(%error, network = %preview.network_name, "邀请接受后刷新成员关系失败");
    }

    let join_command = super::NodeJoinNetworkCommand {
        target: command.coordinator_url,
        network_name: Some(preview.network_name.clone()),
        start: true,
        discovery_record: None,
        network_id: Some(preview.network_id.to_string()),
        preview_digest: Some(command.request.preview_digest.as_str().to_owned()),
        network_policy_revision: Some(preview.terms.network_policy_revision),
        privacy_policy_revision: Some(preview.terms.privacy_policy_revision),
        terms_hash: Some(command.request.terms_hash.clone()),
        acknowledged_at_unix_ms: Some(command.request.acknowledged_at_unix_ms),
    };
    if let Err(error) = runtime.join(join_command).await {
        tracing::warn!(
            %error,
            network = %preview.network_name,
            "邀请已兑换并保存成员关系，但本地网络配置或启动失败"
        );
        let problem = NodeProblem::new(
            error.problem_code(),
            format!("邀请已接受并保存普通成员关系，但本地网络配置或启动失败: {error}"),
        )
        .with_message_key(error.message_key())
        .retryable(true)
        .for_network(preview.network_name.clone());
        state
            .update(|snapshot| snapshot.last_error = Some(problem))
            .await;
    }

    NodeCommandResult::completed(
        request_id,
        NodeCommandOutcome::InvitationRedeemed(Box::new(redemption)),
    )
}

async fn submit_join_request(
    runtime: &NetworkRuntimeManager,
    discovery: &DiscoveryIndexManager,
    configs: &ConfigManager,
    request_id: u64,
    command: NodeSubmitJoinRequestCommand,
) -> NodeCommandResult {
    let network_id = match NetworkId::parse(command.network_id) {
        Ok(network_id) => network_id,
        Err(_) => {
            return join_request_rejection(
                request_id,
                "join_request.error.invalid_network_id",
                "网络 ID 格式无效",
            )
        }
    };
    let record = match discovery.approval_join_record(network_id.as_str()).await {
        Ok(record) => record,
        Err(error) => return NodeCommandResult::rejected(request_id, discovery_problem(error)),
    };
    let Some(coordinator_url) = join_target_from_record(&record) else {
        return unavailable(request_id, "签名发现记录没有可用的 Websocket 审批端点");
    };
    let identity = match configs.load_node_identity() {
        Ok(identity) => identity,
        Err(error) => return unavailable(request_id, format!("加载节点身份失败: {error:#}")),
    };
    let display_name = match configs.node_display_name() {
        Ok(display_name) => display_name,
        Err(error) => return unavailable(request_id, format!("加载节点显示名称失败: {error:#}")),
    };
    let now = unix_time_ms();
    let mut request = match approval_join_request_from_record(
        &record,
        &identity,
        display_name,
        command.reason,
        now,
    ) {
        Ok(request) => request,
        Err(error) => {
            return join_request_rejection(
                request_id,
                "join_request.error.invalid_request",
                format!("构造加入申请失败: {error}"),
            )
        }
    };
    match runtime
        .submit_join_request(request.clone(), Some(coordinator_url.clone()))
        .await
    {
        Ok(status) => {
            apply_join_request_status(&mut request, status, None, unix_time_ms());
            NodeCommandResult::completed(
                request_id,
                NodeCommandOutcome::JoinRequestSubmitted(Box::new(request)),
            )
        }
        Err(error) => {
            runtime_rejection_for_network(request_id, error, request.network_id.to_string())
        }
    }
}

async fn list_join_requests(
    runtime: &NetworkRuntimeManager,
    discovery: Option<&DiscoveryIndexManager>,
    request_id: u64,
    network_id: String,
) -> NodeCommandResult {
    let network_id = match NetworkId::parse(network_id) {
        Ok(network_id) => network_id,
        Err(_) => {
            return join_request_rejection(
                request_id,
                "join_request.error.invalid_network_id",
                "网络 ID 格式无效",
            )
        }
    };
    let coordinator_url = approval_coordinator_url(discovery, &network_id).await;
    match runtime
        .list_join_requests(network_id.clone(), coordinator_url)
        .await
    {
        Ok(requests) => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::JoinRequests(Box::new(NodeJoinRequestListResult {
                network_id: network_id.to_string(),
                requests,
            })),
        ),
        Err(error) => runtime_rejection_for_network(request_id, error, network_id.to_string()),
    }
}

fn domain_join_request_reviews(
    reviews: Vec<crate::node::command::NodeJoinRequestReviewItem>,
) -> Result<Vec<JoinRequestBatchReviewItem>, NodeProblem> {
    reviews
        .into_iter()
        .map(|review| {
            let request_id = JoinRequestId::parse(review.request_id).map_err(|_| {
                NodeProblem::new(NodeProblemCode::InvalidRequest, "加入申请 ID 格式无效")
                    .with_message_key("join_request.error.invalid_request_id")
            })?;
            Ok(JoinRequestBatchReviewItem {
                request_id,
                expected_revision: review.expected_revision,
                action: review.action,
                note: review.note,
            })
        })
        .collect()
}

async fn review_join_requests(
    runtime: &NetworkRuntimeManager,
    request_id: u64,
    network_id: String,
    reviews: Vec<crate::node::command::NodeJoinRequestReviewItem>,
) -> NodeCommandResult {
    let network_id = match NetworkId::parse(network_id) {
        Ok(network_id) => network_id,
        Err(_) => {
            return join_request_rejection(
                request_id,
                "join_request.error.invalid_network_id",
                "网络 ID 格式无效",
            )
        }
    };
    let reviews = match domain_join_request_reviews(reviews) {
        Ok(reviews) => reviews,
        Err(problem) => return NodeCommandResult::rejected(request_id, problem),
    };
    match runtime
        .review_join_requests(network_id.clone(), reviews)
        .await
    {
        Ok(reviews) => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::JoinRequestsReviewed(Box::new(NodeJoinRequestReviewAck {
                network_id: network_id.to_string(),
                reviews: reviews
                    .into_iter()
                    .map(|review| NodeJoinRequestReviewOutcome {
                        request_id: review.request_id.to_string(),
                        state: review.state,
                        revision: review.revision,
                    })
                    .collect(),
            })),
        ),
        Err(error) => runtime_rejection_for_network(request_id, error, network_id.to_string()),
    }
}

async fn withdraw_join_request(
    runtime: &NetworkRuntimeManager,
    discovery: Option<&DiscoveryIndexManager>,
    request_id: u64,
    network_id: String,
    join_request_id: String,
) -> NodeCommandResult {
    let network_id = match NetworkId::parse(network_id) {
        Ok(network_id) => network_id,
        Err(_) => {
            return join_request_rejection(
                request_id,
                "join_request.error.invalid_network_id",
                "网络 ID 格式无效",
            )
        }
    };
    let join_request_id = match JoinRequestId::parse(join_request_id) {
        Ok(request_id) => request_id,
        Err(_) => {
            return join_request_rejection(
                request_id,
                "join_request.error.invalid_request_id",
                "加入申请 ID 格式无效",
            )
        }
    };
    let coordinator_url = approval_coordinator_url(discovery, &network_id).await;
    let mut request = match runtime
        .list_join_requests(network_id.clone(), coordinator_url.clone())
        .await
    {
        Ok(requests) => match requests
            .into_iter()
            .find(|request| request.request_id == join_request_id)
        {
            Some(request) => request,
            None => {
                return runtime_rejection_for_network(
                    request_id,
                    RuntimeControlError::NotFound("加入申请不存在或已被清理".into()),
                    network_id.to_string(),
                )
            }
        },
        Err(error) => {
            return runtime_rejection_for_network(request_id, error, network_id.to_string())
        }
    };
    match runtime
        .withdraw_join_request(network_id.clone(), join_request_id, coordinator_url)
        .await
    {
        Ok(status) => {
            apply_join_request_status(&mut request, status, None, unix_time_ms());
            NodeCommandResult::completed(
                request_id,
                NodeCommandOutcome::JoinRequestUpdated(Box::new(request)),
            )
        }
        Err(error) => runtime_rejection_for_network(request_id, error, network_id.to_string()),
    }
}

async fn resubmit_join_request(
    runtime: &NetworkRuntimeManager,
    discovery: Option<&DiscoveryIndexManager>,
    request_id: u64,
    command: NodeResubmitJoinRequestCommand,
) -> NodeCommandResult {
    let network_id = match NetworkId::parse(command.network_id) {
        Ok(network_id) => network_id,
        Err(_) => {
            return join_request_rejection(
                request_id,
                "join_request.error.invalid_network_id",
                "网络 ID 格式无效",
            )
        }
    };
    let join_request_id = match JoinRequestId::parse(command.request_id) {
        Ok(request_id) => request_id,
        Err(_) => {
            return join_request_rejection(
                request_id,
                "join_request.error.invalid_request_id",
                "加入申请 ID 格式无效",
            )
        }
    };
    let coordinator_url = approval_coordinator_url(discovery, &network_id).await;
    let mut request = match runtime
        .list_join_requests(network_id.clone(), coordinator_url.clone())
        .await
    {
        Ok(requests) => match requests
            .into_iter()
            .find(|request| request.request_id == join_request_id)
        {
            Some(request) => request,
            None => {
                return runtime_rejection_for_network(
                    request_id,
                    RuntimeControlError::NotFound("加入申请不存在或已被清理".into()),
                    network_id.to_string(),
                )
            }
        },
        Err(error) => {
            return runtime_rejection_for_network(request_id, error, network_id.to_string())
        }
    };
    let reason = command.reason;
    match runtime
        .resubmit_join_request(
            network_id.clone(),
            join_request_id,
            command.expected_revision,
            reason.clone(),
            coordinator_url,
        )
        .await
    {
        Ok(status) => {
            apply_join_request_status(&mut request, status, Some(reason), unix_time_ms());
            NodeCommandResult::completed(
                request_id,
                NodeCommandOutcome::JoinRequestUpdated(Box::new(request)),
            )
        }
        Err(error) => runtime_rejection_for_network(request_id, error, network_id.to_string()),
    }
}

async fn join_runtime_network(
    runtime: &NetworkRuntimeManager,
    discovery: Option<&DiscoveryIndexManager>,
    state: &NodeStateStore,
    configs: Option<&ConfigManager>,
    join_gate: &Arc<Mutex<()>>,
    request_id: u64,
    mut command: Box<super::NodeJoinNetworkCommand>,
) -> NodeCommandResult {
    // A complete join owns discovery confirmation, membership persistence,
    // configuration mutation and runtime start. Serializing the whole sequence
    // prevents two UI submissions from racing the member-limit check.
    let _join_guard = join_gate.lock().await;
    let mut confirmed_record = None;
    let mut terms_ack = None;

    if let Some(network_id) = command.network_id.as_deref() {
        let Some(discovery) = discovery else {
            return unavailable(request_id, "加入网络前无法验证持久化签名发现记录");
        };
        let already_member = join_membership_exists(state, configs, network_id).await;
        let record = match discovery
            .confirm_join_record(
                network_id,
                command.preview_digest.as_deref().unwrap_or_default(),
                command.network_policy_revision.unwrap_or_default(),
                command.privacy_policy_revision.unwrap_or_default(),
                command
                    .terms_hash
                    .as_ref()
                    .expect("validated join command has terms hash"),
                already_member,
            )
            .await
        {
            Ok(record) => record,
            Err(error) => {
                return NodeCommandResult::rejected(request_id, discovery_problem(error));
            }
        };
        if command
            .network_name
            .as_deref()
            .is_some_and(|name| name != record.document.name)
        {
            return NodeCommandResult::rejected(
                request_id,
                NodeProblem::new(
                    NodeProblemCode::InvalidRequest,
                    "加入网络名称与已确认的签名发现记录不一致",
                )
                .with_message_key("join.error.name_changed")
                .for_network(record.document.name.clone()),
            );
        }
        let Some(target) = join_target_from_record(&record) else {
            return NodeCommandResult::rejected(
                request_id,
                NodeProblem::new(
                    NodeProblemCode::InvalidRequest,
                    "已确认网络没有可用的 Websocket 协调端点",
                )
                .with_message_key("join.error.endpoint_unavailable")
                .for_network(record.document.name.clone()),
            );
        };
        command.network_name = Some(record.document.name.clone());
        command.target = target;
        terms_ack = Some(JoinTermsAck {
            terms_hash: command
                .terms_hash
                .clone()
                .expect("validated join command has terms hash"),
            acknowledged_at_unix_ms: command
                .acknowledged_at_unix_ms
                .expect("validated join command has acknowledgement time"),
        });
        command.discovery_record = Some(record.clone());
        confirmed_record = Some(record);
    } else if let Some(record) = command.discovery_record.take() {
        let Some(discovery) = discovery else {
            return unavailable(request_id, "加入网络前无法验证签名发现记录");
        };
        if let Err(error) = discovery.validate_join_record(&record) {
            return NodeCommandResult::rejected(request_id, discovery_problem(error));
        }
        confirmed_record = Some(record);
    }

    // Persist acceptance before creating configuration or starting a data
    // plane. On any later failure only the record inserted by this operation is
    // rolled back; memberships for other networks are never touched.
    let persisted = match (confirmed_record.as_ref(), terms_ack) {
        (Some(record), Some(ack)) => {
            let Some(configs) = configs else {
                return unavailable(request_id, "加入网络需要节点配置管理器");
            };
            match persist_joined_membership(configs, record, ack).await {
                Ok(membership) => Some(membership),
                Err(error) => {
                    return NodeCommandResult::rejected(
                        request_id,
                        NodeProblem::new(
                            NodeProblemCode::Unavailable,
                            format!("保存本地成员关系失败: {error}"),
                        )
                        .with_param("error_kind", "storage")
                        .retryable(true)
                        .for_network(record.document.name.clone()),
                    );
                }
            }
        }
        _ => None,
    };

    match runtime.join(*command).await {
        Ok(result) => {
            if let Some(configs) = configs {
                if let Err(error) = refresh_network_memberships(state, configs).await {
                    tracing::warn!(%error, "加入成功后刷新本地网络成员关系失败");
                }
            }
            let snapshot = state.snapshot().await;
            let Some((joined, inserted)) = persisted else {
                return NodeCommandResult::completed(
                    request_id,
                    NodeCommandOutcome::Completed {
                        message: format!(
                            "网络 [{}] 配置{}；{}；协调端点: {}",
                            result.network_name,
                            if result.config_saved {
                                "已保存"
                            } else {
                                "已存在"
                            },
                            if result.started {
                                "运行时已启动"
                            } else {
                                "按请求未自动启动"
                            },
                            result.coordinator_url
                        ),
                    },
                );
            };
            let membership = snapshot
                .memberships
                .iter()
                .find(|membership| membership.network_id == joined.membership.network_id.as_str())
                .cloned()
                .unwrap_or_else(|| network_membership_snapshot(&joined));
            NodeCommandResult::completed(
                request_id,
                NodeCommandOutcome::NetworkJoined(Box::new(super::NodeNetworkJoinResult {
                    membership,
                    snapshot,
                    idempotent: !inserted,
                    config_saved: result.config_saved,
                    started: result.started,
                })),
            )
        }
        Err(error) => {
            if let Some((joined, true)) = persisted {
                if let Some(configs) = configs {
                    if let Err(rollback) = remove_joined_membership(
                        configs,
                        joined.membership.network_id.as_str().to_owned(),
                    )
                    .await
                    {
                        return NodeCommandResult::rejected(
                            request_id,
                            NodeProblem::new(
                                NodeProblemCode::Unavailable,
                                format!("加入失败: {error}；回滚本地成员关系失败: {rollback}"),
                            )
                            .with_param("error_kind", "storage")
                            .retryable(true)
                            .for_network(joined.network_name.clone()),
                        );
                    }
                    if let Err(refresh) = refresh_network_memberships(state, configs).await {
                        tracing::warn!(
                            %refresh,
                            network = %joined.network_name,
                            "加入失败回滚后刷新成员关系失败，直接更新本地投影"
                        );
                        let mut memberships = state.snapshot().await.memberships;
                        memberships.retain(|membership| {
                            membership.network_id != joined.membership.network_id.as_str()
                        });
                        state.replace_memberships(memberships).await;
                    }
                }
            }
            runtime_rejection(request_id, error)
        }
    }
}

async fn persist_joined_membership(
    configs: &ConfigManager,
    record: &NetworkDiscoveryRecord,
    ack: JoinTermsAck,
) -> Result<(crate::config_manager::JoinedNetworkMembership, bool), String> {
    let configs = configs.clone();
    let record = record.clone();
    tokio::task::spawn_blocking(move || {
        let now = unix_time_ms().max(ack.acknowledged_at_unix_ms);
        let terms = MembershipTerms {
            network_policy_revision: record.document.policy_revision,
            privacy_policy_revision: record.document.privacy_policy_revision,
            exit_contribution: record.document.exit_contribution,
            member_limit: record.document.member_limit,
        };
        let identity = configs
            .load_node_identity()
            .map_err(|error| error.to_string())?;
        let node_id = NodeId::from_public_key(&identity.public_key_bytes());
        let mut membership = NetworkMembership::new(
            record.document.network_id.clone(),
            node_id,
            MembershipRole::Member,
            ack.acknowledged_at_unix_ms,
        );
        let acknowledged_at_unix_ms = ack.acknowledged_at_unix_ms;
        membership
            .begin_apply(terms, ack, acknowledged_at_unix_ms)
            .map_err(|error| error.to_string())?;
        membership.accept(now).map_err(|error| error.to_string())?;
        configs
            .upsert_joined_membership(&record.document.name, membership)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("成员关系写入任务失败: {error}"))?
}

async fn remove_joined_membership(
    configs: &ConfigManager,
    network_id: String,
) -> Result<(), String> {
    let configs = configs.clone();
    tokio::task::spawn_blocking(move || {
        configs
            .remove_joined_membership(&network_id)
            .map(|_| ())
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("成员关系回滚任务失败: {error}"))?
}

async fn leave_runtime_network(
    runtime: &NetworkRuntimeManager,
    state: &NodeStateStore,
    configs: &ConfigManager,
    join_gate: &Arc<Mutex<()>>,
    request_id: u64,
    command: super::NodeLeaveNetworkCommand,
) -> NodeCommandResult {
    let _join_guard = join_gate.lock().await;
    let network_name = command.network_name;
    let configs_for_lookup = configs.clone();
    let network_name_for_lookup = network_name.clone();
    let joined = match tokio::task::spawn_blocking(move || {
        configs_for_lookup.joined_membership_for_network_name(&network_name_for_lookup)
    })
    .await
    {
        Ok(Ok(joined)) => joined,
        Ok(Err(error)) => {
            return NodeCommandResult::rejected(
                request_id,
                NodeProblem::new(
                    NodeProblemCode::Unavailable,
                    format!("读取网络 [{network_name}] 的本地成员关系失败: {error}"),
                )
                .with_param("error_kind", "storage")
                .retryable(true)
                .for_network(network_name),
            );
        }
        Err(error) => {
            return NodeCommandResult::rejected(
                request_id,
                NodeProblem::new(
                    NodeProblemCode::Internal,
                    format!("读取网络 [{network_name}] 的本地成员关系任务失败: {error}"),
                )
                .retryable(true)
                .for_network(network_name),
            );
        }
    };

    if let Err(error) = runtime.leave(network_name.clone()).await {
        return runtime_rejection_for_network(request_id, error, network_name);
    }

    if let Some(joined) = joined.as_ref() {
        if let Err(error) =
            remove_joined_membership(configs, joined.membership.network_id.as_str().to_owned())
                .await
        {
            return NodeCommandResult::rejected(
                request_id,
                NodeProblem::new(
                    NodeProblemCode::Unavailable,
                    format!(
                        "网络 [{network_name}] 的运行时和配置已退出，但删除本地成员关系失败: {error}"
                    ),
                )
                .with_param("error_kind", "storage")
                .retryable(true)
                .for_network(network_name),
            );
        }
    }

    if let Err(error) = refresh_network_memberships(state, configs).await {
        tracing::warn!(%error, network = %network_name, "退出后刷新成员关系失败，更新本地投影");
        let mut memberships = state.snapshot().await.memberships;
        if let Some(joined) = joined.as_ref() {
            memberships.retain(|membership| {
                membership.network_id != joined.membership.network_id.as_str()
            });
        }
        state.replace_memberships(memberships).await;
    }

    NodeCommandResult::completed(
        request_id,
        NodeCommandOutcome::NetworkLeft(Box::new(super::NodeNetworkLeaveResult {
            network_name,
            left: true,
            snapshot: state.snapshot().await,
        })),
    )
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

async fn stop_runtime_networks(
    runtime: &NetworkRuntimeManager,
    request_id: u64,
    networks: Vec<String>,
    all: bool,
) -> NodeCommandResult {
    match runtime.stop(networks, all).await {
        Ok(summary) if summary.is_success() => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::Completed {
                message: if summary.stopped.is_empty() {
                    "没有正在运行或托管服务的网络".into()
                } else {
                    summary.message()
                },
            },
        ),
        Ok(summary) => NodeCommandResult::rejected(
            request_id,
            NodeProblem::new(NodeProblemCode::Unavailable, summary.message()).retryable(true),
        ),
        Err(error) => runtime_rejection(request_id, error),
    }
}

async fn restart_runtime_networks(
    runtime: &NetworkRuntimeManager,
    request_id: u64,
    networks: Vec<String>,
    all: bool,
) -> NodeCommandResult {
    match runtime.restart(networks, all).await {
        Ok(summary) if summary.failed.is_empty() => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::Completed {
                message: summary.message(),
            },
        ),
        Ok(summary) => NodeCommandResult::rejected(request_id, restart_failure_problem(&summary)),
        Err(error) => runtime_rejection(request_id, error),
    }
}

async fn rebuild_runtime_tun(
    runtime: &NetworkRuntimeManager,
    request_id: u64,
    network_name: String,
) -> NodeCommandResult {
    match runtime.rebuild_tun(network_name.clone()).await {
        Ok(summary)
            if summary.failed.is_empty()
                && summary.restarted.as_slice() == std::slice::from_ref(&network_name) =>
        {
            NodeCommandResult::completed(
                request_id,
                NodeCommandOutcome::Completed {
                    message: format!("网络 [{network_name}] 的 TUN 已重建"),
                },
            )
        }
        Ok(summary) => NodeCommandResult::rejected(
            request_id,
            restart_failure_problem(&summary).for_network(network_name),
        ),
        Err(error) => runtime_rejection_for_network(request_id, error, network_name),
    }
}

async fn update_runtime_routing(
    runtime: &NetworkRuntimeManager,
    request_id: u64,
    command: super::NodeUpdateRoutingCommand,
) -> NodeCommandResult {
    let network_name = command.network_name.clone();
    match runtime.update_routing(command).await {
        Ok(result) => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::Completed {
                message: format!(
                    "网络 [{network_name}] 路由已热更新: mode={}, exit_node={}, is_exit_node={}",
                    result.mode,
                    result.exit_node.as_deref().unwrap_or("none"),
                    result.is_exit_node
                ),
            },
        ),
        Err(error) => runtime_rejection(request_id, error),
    }
}

async fn set_runtime_network_role(
    runtime: &NetworkRuntimeManager,
    request_id: u64,
    command: super::NodeSetNetworkRoleCommand,
) -> NodeCommandResult {
    let network_name = command.network_name.clone();
    let role = command.role;
    let enabled = command.enabled;
    match runtime.set_network_role(command).await {
        Ok(result) => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::Completed {
                message: format!(
                    "网络 [{network_name}] 角色已热更新: {role:?} enabled={enabled}; 当前角色={:?}",
                    result.roles
                ),
            },
        ),
        Err(error) => runtime_rejection_for_network(request_id, error, &network_name),
    }
}

fn completed_summary(request_id: u64, summary: StartSummary, action: &str) -> NodeCommandResult {
    NodeCommandResult::completed(
        request_id,
        NodeCommandOutcome::Completed {
            message: summary.message(action),
        },
    )
}

fn runtime_rejection(request_id: u64, error: RuntimeControlError) -> NodeCommandResult {
    runtime_rejection_scoped(request_id, error, None)
}

fn runtime_rejection_for_network(
    request_id: u64,
    error: RuntimeControlError,
    network_name: impl Into<String>,
) -> NodeCommandResult {
    runtime_rejection_scoped(request_id, error, Some(network_name.into()))
}

fn runtime_rejection_scoped(
    request_id: u64,
    error: RuntimeControlError,
    network_name: Option<String>,
) -> NodeCommandResult {
    let mut problem = NodeProblem::new(error.problem_code(), error.to_string())
        .with_message_key(error.message_key())
        .retryable(error.retryable());
    if let Some(reason) = error.conflict_reason() {
        problem = problem.with_param("reason", reason);
    }
    if error.is_storage_error() {
        problem = problem.with_param("error_kind", "storage");
    }
    if let Some(network_name) = network_name {
        problem = problem.for_network(network_name);
    }
    NodeCommandResult::rejected(request_id, problem)
}

fn network_setup_problem(
    stage: NetworkSetupStage,
    message: impl Into<String>,
    network_name: Option<String>,
) -> NodeProblem {
    let code = network_setup_problem_code(stage);
    let mut problem = NodeProblem::new(code, message)
        .with_message_key(stage.message_key())
        .retryable(!matches!(
            stage,
            NetworkSetupStage::PermissionDenied | NetworkSetupStage::PlatformUnsupported
        ));
    if let Some(network_name) = network_name {
        problem = problem.for_network(network_name);
    }
    problem
}

fn start_failure_problem(summary: &StartSummary, action: &str) -> NodeProblem {
    if let Some(failure) = summary
        .failed
        .iter()
        .find(|failure| failure.stage.is_some())
    {
        return network_setup_problem(
            failure.stage.expect("checked above"),
            failure.message.clone(),
            Some(failure.network_name.clone()),
        );
    }

    NodeProblem::new(NodeProblemCode::Unavailable, summary.message(action)).retryable(true)
}

fn restart_failure_problem(summary: &RestartSummary) -> NodeProblem {
    if let Some(failure) = summary
        .failed
        .iter()
        .find(|failure| failure.stage.is_some())
    {
        return network_setup_problem(
            failure.stage.expect("checked above"),
            failure.message.clone(),
            Some(failure.network_name.clone()),
        );
    }

    NodeProblem::new(NodeProblemCode::Unavailable, summary.message()).retryable(true)
}

async fn stop_networks(
    registry: &MultiNetworkRegistry,
    request_id: u64,
    networks: &[String],
    all: bool,
) -> NodeCommandResult {
    if all {
        return stop_all(registry, request_id).await;
    }

    let sessions = registry.get_all().await;
    let live_names: std::collections::HashSet<_> = sessions
        .iter()
        .map(|session| session.network_name.as_str())
        .collect();
    let missing: Vec<_> = networks
        .iter()
        .filter(|name| !live_names.contains(name.as_str()))
        .cloned()
        .collect();
    if !missing.is_empty() {
        let message = format!("网络未运行: {}", missing.join(", "));
        return NodeCommandResult::rejected(
            request_id,
            NodeProblem::new(NodeProblemCode::NotFound, message),
        );
    }

    for name in networks {
        if !registry.send_stop(Some(name.clone())) {
            return unavailable(request_id, "节点停止通道未就绪，未能下发停止指令");
        }
    }

    NodeCommandResult::completed(
        request_id,
        NodeCommandOutcome::Completed {
            message: format!("已下发停止 {} 个网络指令", networks.len()),
        },
    )
}

async fn stop_all(registry: &MultiNetworkRegistry, request_id: u64) -> NodeCommandResult {
    if !registry.send_stop(None) {
        return unavailable(request_id, "节点停止通道未就绪，未能下发停止指令");
    }

    NodeCommandResult::completed(
        request_id,
        NodeCommandOutcome::Completed {
            message: "已下发停止全部网络指令".into(),
        },
    )
}

async fn probe_peer(
    registry: &MultiNetworkRegistry,
    request_id: u64,
    command: super::NodeProbePeerCommand,
) -> NodeCommandResult {
    let sessions = registry.get_all().await;
    let Some(session) = sessions
        .iter()
        .find(|session| session.network_name == command.network_name)
    else {
        return NodeCommandResult::rejected(
            request_id,
            NodeProblem::new(
                NodeProblemCode::NotFound,
                format!("网络未运行: {}", command.network_name),
            )
            .for_network(command.network_name),
        );
    };

    let timeout_ms = command.timeout_ms.clamp(100, 5_000);
    let Some(outcome) = session
        .p2p
        .probe_peer_now(&command.target, Duration::from_millis(timeout_ms))
        .await
    else {
        return NodeCommandResult::rejected(
            request_id,
            NodeProblem::new(
                NodeProblemCode::NotFound,
                format!("未找到在线 Peer: {}", command.target),
            )
            .for_network(command.network_name),
        );
    };

    NodeCommandResult::completed(
        request_id,
        NodeCommandOutcome::Probe(NodePeerProbeSnapshot {
            peer_id: outcome.client_id,
            virtual_ipv6: outcome.virtual_ip,
            endpoint: outcome.endpoint,
            rtt_us: outcome.rtt_us,
            p2p_active: outcome.p2p_active,
            timed_out: outcome.timed_out,
        }),
    )
}

fn unavailable(request_id: u64, message: impl Into<String>) -> NodeCommandResult {
    NodeCommandResult::rejected(
        request_id,
        NodeProblem::new(NodeProblemCode::Unavailable, message).retryable(true),
    )
}

/// Shared command bus backed by a live node runtime.
pub fn spawn_command_bus(
    state: NodeStateStore,
    registry: MultiNetworkRegistry,
    events: super::NodeEventBus,
) -> super::NodeCommandBus {
    let handler = NodeRuntimeHandler::new(state.clone(), registry);
    super::NodeCommandBus::spawn_with_state(Arc::new(handler), events, 32, state)
}

/// Spawn the command bus while sharing embedded service ownership with the
/// node supervisor. This lets UI commands and the supervisor act on the same
/// signaling/relay lifecycle instead of creating competing service owners.
pub fn spawn_command_bus_with_services(
    state: NodeStateStore,
    registry: MultiNetworkRegistry,
    events: super::NodeEventBus,
    services: EmbeddedServiceManager,
) -> super::NodeCommandBus {
    let handler = NodeRuntimeHandler::with_services(state.clone(), registry, services);
    super::NodeCommandBus::spawn_with_state(Arc::new(handler), events, 32, state)
}

pub(crate) fn spawn_command_bus_with_runtime(
    state: NodeStateStore,
    registry: MultiNetworkRegistry,
    events: super::NodeEventBus,
    services: EmbeddedServiceManager,
    runtime: NetworkRuntimeManager,
    configs: ConfigManager,
) -> super::NodeCommandBus {
    let handler =
        NodeRuntimeHandler::with_runtime(state.clone(), registry, services, runtime, configs);
    super::NodeCommandBus::spawn_with_state(Arc::new(handler), events, 32, state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{
        NodeClientBlacklistCommand, NodeCommand, NodeCommandBus, NodeCreateNetworkCommand,
        NodeEventBus, NodeJoinNetworkCommand, NodeNetworkAllocation, NodeNetworkRole,
        NodeProbePeerCommand, NodeRoutingMode, NodeSetNetworkRoleCommand,
        NodeUpdateNetworkAllocationCommand, NodeUpdateRoutingCommand,
    };
    use crate::symmetric::EmbeddedServiceStartRequest;
    use fuckipv6_common::domain::{ContentHash, InvitationCode, InvitationId};
    use fuckipv6_common::{
        ExitNodeCapabilities, ExitNodePlatform, NetworkMode, EXIT_NODE_CAPABILITY_SCHEMA_VERSION,
    };
    use tokio::sync::mpsc;

    fn invitation_preview() -> InvitationPreview {
        let terms = MembershipTerms {
            network_policy_revision: 7,
            privacy_policy_revision: 3,
            exit_contribution: Default::default(),
            member_limit: Some(25),
        };
        let mut preview = InvitationPreview {
            invitation_id: InvitationId::from_public_key(b"invitation"),
            invitation_revision: 4,
            network_id: NetworkId::from_public_key(b"network"),
            network_name: "private-lab".into(),
            network_revision: 11,
            role: MembershipRole::Member,
            capabilities: vec![CapabilityKind::Exit, CapabilityKind::File],
            remaining_uses: 2,
            expires_at_unix_ms: 100_000,
            state: InvitationState::Active,
            terms_hash: terms.content_hash().expect("terms hash"),
            terms,
            preview_digest: ContentHash::from_bytes(b"placeholder"),
            permission_changes: vec!["member:exit,file".into()],
            creates_permanent_membership: false,
            requires_recent_reauthentication: true,
            can_accept: true,
            denial_key: None,
        };
        preview.preview_digest = preview.compute_digest().expect("preview digest");
        preview
    }

    fn invitation_redeem_request(preview: &InvitationPreview) -> InvitationRedeemRequest {
        InvitationRedeemRequest {
            network_id: preview.network_id.clone(),
            code: InvitationCode::generate().expect("invitation code"),
            invitation_id: preview.invitation_id.clone(),
            expected_invitation_revision: preview.invitation_revision,
            preview_digest: preview.preview_digest.clone(),
            terms_hash: preview.terms_hash.clone(),
            acknowledged_at_unix_ms: 90_000,
        }
    }

    fn rebind_preview_digest(preview: &mut InvitationPreview) {
        preview.preview_digest = preview.compute_digest().expect("preview digest");
    }

    async fn execute_rejected(
        bus: &NodeCommandBus,
        request_id: u64,
        command: NodeCommand,
    ) -> NodeProblem {
        let result = bus
            .execute_test(NodeCommandRequest::new(request_id, command))
            .await
            .expect("command result");
        assert_eq!(result.request_id, request_id);
        assert!(!result.is_success());
        match result.outcome {
            NodeCommandOutcome::Rejected(problem) => problem,
            outcome => panic!("unexpected outcome: {outcome:?}"),
        }
    }

    fn assert_problem(problem: &NodeProblem, code: NodeProblemCode, retryable: bool) {
        assert_eq!(problem.code, code);
        assert_eq!(problem.retryable, retryable);
        assert!(!problem.message.is_empty());
    }

    fn approval_discovery_record(identity: &Identity, now_unix_ms: u64) -> NetworkDiscoveryRecord {
        use fuckipv6_common::domain::{
            DiscoveryTransport, ExitContributionObligation, NetworkDiscoveryDocument,
            NetworkEndpoint, NetworkJoinMode, NetworkVisibility, PublicKey,
            NETWORK_DISCOVERY_SCHEMA_VERSION,
        };

        let public_key = PublicKey::from_bytes(identity.public_key_bytes());
        let node_id = NodeId::from_public_key(&identity.public_key_bytes());
        let document = NetworkDiscoveryDocument {
            schema_version: NETWORK_DISCOVERY_SCHEMA_VERSION,
            network_id: NetworkId::from_public_key(public_key.as_str().as_bytes()),
            public_key,
            publisher: node_id.clone(),
            name: "approval-network".into(),
            purpose: "Test approval joins".into(),
            language: "zh-CN".into(),
            region: "CN".into(),
            tags: vec!["approval".into()],
            visibility: NetworkVisibility::Private,
            join_mode: NetworkJoinMode::ApprovalRequired,
            owner: node_id,
            administrators: Vec::new(),
            revision: 9,
            policy_revision: 7,
            privacy_policy_revision: 3,
            member_limit: Some(16),
            member_count: 4,
            exit_contribution: ExitContributionObligation::Required,
            privacy_changes: vec!["仅公开审批所需的资料".into()],
            issued_at_unix_ms: now_unix_ms.saturating_sub(1_000),
            expires_at_unix_ms: now_unix_ms.saturating_add(60_000),
            capability_summary: vec![CapabilityKind::Exit, CapabilityKind::File],
            endpoints: vec![NetworkEndpoint {
                transport: DiscoveryTransport::Websocket,
                host: "coordinator.example.test".into(),
                port: 443,
            }],
            revoked_at_unix_ms: None,
        };
        NetworkDiscoveryRecord::sign(document, identity).expect("sign approval discovery record")
    }

    #[test]
    fn approval_request_uses_signed_network_terms_and_redacted_public_profile() {
        let identity = Identity::generate().expect("generate identity");
        let now = 1_800_000_000_000;
        let record = approval_discovery_record(&identity, now);
        let request = approval_join_request_from_record(
            &record,
            &identity,
            Some("申请人".into()),
            "希望加入测试网络".into(),
            now,
        )
        .expect("construct approval request");

        assert_eq!(request.network_id, record.document.network_id);
        assert_eq!(
            request.applicant.node_id,
            NodeId::from_public_key(&identity.public_key_bytes())
        );
        assert_eq!(request.applicant.display_name.as_deref(), Some("申请人"));
        assert_eq!(
            request.applicant.public_key_fingerprint,
            Identity::public_key_fingerprint(&identity.public_key_bytes())
        );
        assert_eq!(request.reason.as_deref(), Some("希望加入测试网络"));
        assert_eq!(request.terms.network_policy_revision, 7);
        assert_eq!(request.terms.privacy_policy_revision, 3);
        assert_eq!(request.terms.member_limit, Some(16));
        assert_eq!(
            request.terms.exit_contribution,
            fuckipv6_common::domain::ExitContributionObligation::Required
        );
        assert_eq!(request.grant_preview.role, MembershipRole::Member);
        assert_eq!(
            request.grant_preview.capabilities,
            vec![CapabilityKind::Exit, CapabilityKind::File]
        );
        assert!(request.grant_preview.file_access);
        assert!(request.grant_preview.grants_data_plane_access);
        assert_eq!(
            request.expires_at_unix_ms,
            record.document.expires_at_unix_ms
        );
        assert!(request.request_id.as_str().starts_with("join_"));
    }

    #[test]
    fn terminal_join_request_status_sets_decision_and_retention_metadata() {
        let identity = Identity::generate().expect("generate identity");
        let now = 1_800_000_000_000;
        let record = approval_discovery_record(&identity, now);
        let mut request =
            approval_join_request_from_record(&record, &identity, None, "请审批".into(), now)
                .expect("construct approval request");
        let decided_at = now + 5_000;
        let request_id = request.request_id.clone();
        apply_join_request_status(
            &mut request,
            crate::signaling::client::JoinRequestStatusSnapshot {
                request_id,
                state: fuckipv6_common::domain::JoinRequestState::Rejected,
                revision: 2,
                information_request: None,
            },
            None,
            decided_at,
        );

        assert_eq!(
            request.state,
            fuckipv6_common::domain::JoinRequestState::Rejected
        );
        assert_eq!(request.revision, 2);
        assert_eq!(request.updated_at_unix_ms, decided_at);
        assert_eq!(request.decided_at_unix_ms, Some(decided_at));
        assert_eq!(
            request.reason_retention_until_unix_ms,
            Some(decided_at + fuckipv6_common::domain::JOIN_REQUEST_REASON_RETENTION_MS)
        );
    }

    #[tokio::test]
    async fn get_snapshot_reads_the_shared_store() {
        let state = NodeStateStore::new();
        state
            .update(|snapshot| {
                snapshot.logs.push(crate::node::NodeLogEntry {
                    unix_ms: 42,
                    level: crate::node::NodeLogLevel::Info,
                    target: "test".into(),
                    message: "shared".into(),
                })
            })
            .await;
        let registry = MultiNetworkRegistry::new();
        let stored_revision = state.snapshot().await.revision;
        let bus = spawn_command_bus(state, registry, NodeEventBus::default());
        let result = bus
            .execute_test(NodeCommandRequest::new(1, NodeCommand::GetSnapshot))
            .await
            .expect("snapshot result");

        match result.outcome {
            NodeCommandOutcome::Snapshot(snapshot) => {
                // GetSnapshot refreshes the authoritative host-interface
                // projection and may legitimately advance the revision when
                // the first real capture differs from the default snapshot.
                assert!(snapshot.revision >= stored_revision);
                assert_eq!(snapshot.logs[0].message, "shared");
            }
            outcome => panic!("unexpected outcome: {outcome:?}"),
        }
    }

    #[tokio::test]
    async fn get_ui_manifest_round_trips_and_matches_the_shared_contract() {
        let bus = spawn_command_bus(
            NodeStateStore::new(),
            MultiNetworkRegistry::new(),
            NodeEventBus::default(),
        );
        let request = NodeCommandRequest::new(21, NodeCommand::GetUiManifest);
        let encoded = serde_json::to_string(&request).expect("serialize manifest request");
        let decoded: NodeCommandRequest =
            serde_json::from_str(&encoded).expect("deserialize manifest request");
        assert_eq!(decoded, request);

        let result = bus.execute_test(decoded).await.expect("manifest result");
        match result.outcome {
            NodeCommandOutcome::UiManifest(manifest) => {
                assert_eq!(
                    manifest.schema_version,
                    crate::node::NODE_UI_MANIFEST_SCHEMA_VERSION
                );
                assert_eq!(manifest.pages.len(), 12);
                assert_eq!(*manifest, crate::node::node_ui_manifest());
            }
            outcome => panic!("unexpected outcome: {outcome:?}"),
        }
    }

    #[tokio::test]
    async fn management_commands_reach_the_shared_embedded_service() {
        let config_dir = std::env::temp_dir().join(format!(
            "fuckipv6-runtime-handler-management-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&config_dir);
        let configs = crate::config_manager::ConfigManager::new_for_dir(config_dir.clone())
            .expect("config manager");
        let services = EmbeddedServiceManager::spawn_with_config(4, configs);
        services
            .start(EmbeddedServiceStartRequest::new("lab", 0, 0))
            .await
            .expect("start embedded service");
        let bus = spawn_command_bus_with_services(
            NodeStateStore::new(),
            MultiNetworkRegistry::new(),
            NodeEventBus::default(),
            services.clone(),
        );

        let result = bus
            .execute_test(NodeCommandRequest::new(
                70,
                NodeCommand::GetBlacklist {
                    network_name: "lab".into(),
                },
            ))
            .await
            .expect("get blacklist result");
        match result.outcome {
            NodeCommandOutcome::Blacklist(snapshot) => {
                assert_eq!(snapshot.network_name, "lab");
                assert!(snapshot.clients.is_empty());
                assert!(!snapshot.changed);
            }
            outcome => panic!("unexpected outcome: {outcome:?}"),
        }

        let result = bus
            .execute_test(NodeCommandRequest::new(
                71,
                NodeCommand::BanClient(NodeClientBlacklistCommand {
                    network_name: "lab".into(),
                    client_id: " node-bad ".into(),
                }),
            ))
            .await
            .expect("ban result");
        match result.outcome {
            NodeCommandOutcome::Blacklist(snapshot) => {
                assert_eq!(snapshot.clients, vec!["node-bad".to_string()]);
                assert!(snapshot.changed);
            }
            outcome => panic!("unexpected outcome: {outcome:?}"),
        }

        let result = bus
            .execute_test(NodeCommandRequest::new(
                72,
                NodeCommand::UnbanClient(NodeClientBlacklistCommand {
                    network_name: "lab".into(),
                    client_id: "node-bad".into(),
                }),
            ))
            .await
            .expect("unban result");
        match result.outcome {
            NodeCommandOutcome::Blacklist(snapshot) => {
                assert!(snapshot.clients.is_empty());
                assert!(snapshot.changed);
            }
            outcome => panic!("unexpected outcome: {outcome:?}"),
        }

        let result = bus
            .execute_test(NodeCommandRequest::new(
                73,
                NodeCommand::GetNetworkAllocation {
                    network_name: "lab".into(),
                },
            ))
            .await
            .expect("get allocation result");
        match result.outcome {
            NodeCommandOutcome::NetworkAllocation(snapshot) => {
                assert_eq!(snapshot.network_name, "lab");
                assert!(snapshot.config_saved);
                assert_eq!(snapshot.allocation.mode, NetworkMode::DualStack);
            }
            outcome => panic!("unexpected outcome: {outcome:?}"),
        }

        let updated = NodeNetworkAllocation {
            mode: NetworkMode::Ipv4Only,
            ipv4_pool: "192.168.100.0/24".into(),
            ipv6_pool: "fd00::1000/120".into(),
        };
        let result = bus
            .execute_test(NodeCommandRequest::new(
                74,
                NodeCommand::UpdateNetworkAllocation(NodeUpdateNetworkAllocationCommand {
                    network_name: "lab".into(),
                    allocation: updated.clone(),
                }),
            ))
            .await
            .expect("update allocation result");
        match result.outcome {
            NodeCommandOutcome::NetworkAllocation(snapshot) => {
                assert_eq!(snapshot.network_name, "lab");
                assert_eq!(snapshot.allocation, updated);
                assert!(snapshot.config_saved);
            }
            outcome => panic!("unexpected outcome: {outcome:?}"),
        }

        let problem = execute_rejected(
            &bus,
            75,
            NodeCommand::GetNetworkAllocation {
                network_name: "missing".into(),
            },
        )
        .await;
        assert_problem(&problem, NodeProblemCode::NotFound, false);
        assert_eq!(problem.network_name.as_deref(), Some("missing"));

        services.shutdown().await.expect("shutdown services");
        let _ = std::fs::remove_dir_all(config_dir);
    }

    #[tokio::test]
    async fn shutdown_uses_the_live_stop_channel() {
        let registry = MultiNetworkRegistry::new();
        let (stop_tx, mut stop_rx) = mpsc::unbounded_channel();
        registry.set_stop_sender(stop_tx);
        let bus = spawn_command_bus(NodeStateStore::new(), registry, NodeEventBus::default());

        let result = bus
            .execute_test(NodeCommandRequest::new(2, NodeCommand::Shutdown))
            .await
            .expect("shutdown result");

        assert!(result.is_success());
        assert_eq!(stop_rx.recv().await, Some(None));
    }

    #[tokio::test]
    async fn shutdown_releases_shared_embedded_services() {
        let registry = MultiNetworkRegistry::new();
        let (stop_tx, mut stop_rx) = mpsc::unbounded_channel();
        registry.set_stop_sender(stop_tx);
        let config_dir = std::env::temp_dir().join(format!(
            "fuckipv6-runtime-handler-services-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&config_dir);
        let configs = crate::config_manager::ConfigManager::new_for_dir(config_dir.clone())
            .expect("config manager");
        let services = EmbeddedServiceManager::spawn_with_config(2, configs);
        services
            .start(EmbeddedServiceStartRequest::new("lab", 0, 0))
            .await
            .expect("start embedded service");
        let bus = spawn_command_bus_with_services(
            NodeStateStore::new(),
            registry,
            NodeEventBus::default(),
            services.clone(),
        );

        let result = bus
            .execute_test(NodeCommandRequest::new(20, NodeCommand::Shutdown))
            .await
            .expect("shutdown result");

        assert!(result.is_success());
        assert_eq!(stop_rx.recv().await, Some(None));
        assert!(services.list().await.is_err());
        let _ = std::fs::remove_dir_all(config_dir);
    }

    #[tokio::test]
    async fn stop_named_network_rejects_when_it_is_not_running() {
        let bus = spawn_command_bus(
            NodeStateStore::new(),
            MultiNetworkRegistry::new(),
            NodeEventBus::default(),
        );

        let result = bus
            .execute_test(NodeCommandRequest::new(
                3,
                NodeCommand::StopNetworks {
                    networks: vec!["home".into()],
                    all: false,
                },
            ))
            .await
            .expect("stop result");

        match result.outcome {
            NodeCommandOutcome::Rejected(problem) => {
                assert_problem(&problem, NodeProblemCode::NotFound, false);
            }
            outcome => panic!("unexpected outcome: {outcome:?}"),
        }
    }

    #[tokio::test]
    async fn unsupported_dynamic_start_fails_explicitly() {
        let bus = spawn_command_bus(
            NodeStateStore::new(),
            MultiNetworkRegistry::new(),
            NodeEventBus::default(),
        );

        let result = bus
            .execute_test(NodeCommandRequest::new(
                4,
                NodeCommand::StartNetworks {
                    selection: crate::node::NodeNetworkSelection::All,
                },
            ))
            .await
            .expect("start result");

        match result.outcome {
            NodeCommandOutcome::Rejected(problem) => {
                assert_problem(&problem, NodeProblemCode::Unavailable, true);
                assert!(problem.message.contains("监督器"));
            }
            outcome => panic!("unexpected outcome: {outcome:?}"),
        }
    }

    #[tokio::test]
    async fn non_exit_routing_update_reports_unavailable() {
        let bus = spawn_command_bus(
            NodeStateStore::new(),
            MultiNetworkRegistry::new(),
            NodeEventBus::default(),
        );
        let problem = execute_rejected(
            &bus,
            30,
            NodeCommand::UpdateRouting(NodeUpdateRoutingCommand {
                network_name: "home".into(),
                mode: NodeRoutingMode::GlobalProxy,
                exit_node: Some("office".into()),
                is_exit_node: false,
            }),
        )
        .await;
        assert_problem(&problem, NodeProblemCode::Unavailable, true);
    }

    #[tokio::test]
    async fn exit_node_actions_use_unsupported_capabilities_from_the_snapshot() {
        let state = NodeStateStore::new();
        let capabilities = ExitNodeCapabilities::for_platform(ExitNodePlatform::Windows);
        state
            .update(|snapshot| snapshot.exit_node_capabilities = capabilities.clone())
            .await;
        let bus = spawn_command_bus(state, MultiNetworkRegistry::new(), NodeEventBus::default());
        for (offset, command) in [
            NodeCommand::UpdateRouting(NodeUpdateRoutingCommand {
                network_name: "home".into(),
                mode: NodeRoutingMode::GlobalProxy,
                exit_node: Some("office".into()),
                is_exit_node: true,
            }),
            NodeCommand::SetNetworkRole(NodeSetNetworkRoleCommand {
                network_name: "home".into(),
                role: NodeNetworkRole::ExitNode,
                enabled: true,
            }),
        ]
        .into_iter()
        .enumerate()
        {
            let problem = execute_rejected(&bus, 31 + offset as u64, command).await;
            assert_problem(&problem, NodeProblemCode::Unsupported, false);
            assert_eq!(problem.message, capabilities.unsupported_explanation());
        }
    }

    #[tokio::test]
    async fn supported_exit_node_actions_stay_unavailable_until_backend_is_wired() {
        let state = NodeStateStore::new();
        state
            .update(|snapshot| {
                snapshot.exit_node_capabilities = ExitNodeCapabilities {
                    schema_version: EXIT_NODE_CAPABILITY_SCHEMA_VERSION,
                    platform: ExitNodePlatform::Linux,
                    supported: true,
                    ipv4_forwarding: true,
                    ipv6_forwarding: true,
                    nat: true,
                    cleanup: true,
                    unsupported_reasons: Vec::new(),
                }
            })
            .await;
        let bus = spawn_command_bus(state, MultiNetworkRegistry::new(), NodeEventBus::default());
        for (offset, command) in [
            NodeCommand::UpdateRouting(NodeUpdateRoutingCommand {
                network_name: "home".into(),
                mode: NodeRoutingMode::GlobalProxy,
                exit_node: Some("office".into()),
                is_exit_node: true,
            }),
            NodeCommand::SetNetworkRole(NodeSetNetworkRoleCommand {
                network_name: "home".into(),
                role: NodeNetworkRole::ExitNode,
                enabled: true,
            }),
        ]
        .into_iter()
        .enumerate()
        {
            let problem = execute_rejected(&bus, 41 + offset as u64, command).await;
            assert_problem(&problem, NodeProblemCode::Unavailable, true);
        }
    }

    #[tokio::test]
    async fn tun_rebuild_fails_explicitly_without_a_runtime() {
        let bus = spawn_command_bus(
            NodeStateStore::new(),
            MultiNetworkRegistry::new(),
            NodeEventBus::default(),
        );
        let problem = execute_rejected(
            &bus,
            39,
            NodeCommand::RebuildTun {
                network_name: "home".into(),
            },
        )
        .await;

        assert_problem(&problem, NodeProblemCode::Unavailable, true);
        assert_eq!(problem.message, "重建 TUN 需要运行中的多网络监督器控制通道");
    }

    #[tokio::test]
    async fn create_and_join_fail_explicitly_without_a_runtime() {
        let bus = spawn_command_bus(
            NodeStateStore::new(),
            MultiNetworkRegistry::new(),
            NodeEventBus::default(),
        );
        let commands = [
            NodeCommand::CreateNetwork(NodeCreateNetworkCommand {
                name: "home".into(),
                purpose: "A test network".into(),
                icon: "network".into(),
                language: "und".into(),
                region: "ZZ".into(),
                tags: Vec::new(),
                capability_declarations: Vec::new(),
                visibility: Default::default(),
                join_mode: Default::default(),
                signaling_port: 6666,
                relay_port: 6667,
                external_host: None,
                auto_connect: true,
            }),
            NodeCommand::JoinNetwork(Box::new(NodeJoinNetworkCommand {
                target: "fk://invite".into(),
                network_name: Some("office".into()),
                start: true,
                discovery_record: None,
                network_id: None,
                preview_digest: None,
                network_policy_revision: None,
                privacy_policy_revision: None,
                terms_hash: None,
                acknowledged_at_unix_ms: None,
            })),
        ];

        for (offset, command) in commands.into_iter().enumerate() {
            let problem = execute_rejected(&bus, 40 + offset as u64, command).await;
            assert_problem(&problem, NodeProblemCode::Unavailable, true);
        }
    }

    #[tokio::test]
    async fn probe_unknown_network_returns_network_scoped_not_found() {
        let bus = spawn_command_bus(
            NodeStateStore::new(),
            MultiNetworkRegistry::new(),
            NodeEventBus::default(),
        );

        let problem = execute_rejected(
            &bus,
            50,
            NodeCommand::ProbePeer(NodeProbePeerCommand {
                network_name: "home".into(),
                target: "fd00::2".into(),
                timeout_ms: 1_000,
            }),
        )
        .await;

        assert_problem(&problem, NodeProblemCode::NotFound, false);
        assert_eq!(problem.network_name.as_deref(), Some("home"));
    }

    #[test]
    fn invitation_consent_rejects_digest_terms_and_reauthentication_drift() {
        let preview = invitation_preview();
        let mut request = invitation_redeem_request(&preview);
        assert!(validate_invitation_preview(&request, &preview, 95_000).is_ok());

        request.preview_digest = ContentHash::from_bytes(b"different-preview");
        let problem = validate_invitation_preview(&request, &preview, 95_000)
            .expect_err("changed preview digest must be rejected");
        assert_problem(&problem, NodeProblemCode::Conflict, false);
        assert_eq!(
            problem.message_key.as_str(),
            "invitation.error.preview_changed"
        );

        let mut request = invitation_redeem_request(&preview);
        request.terms_hash = ContentHash::from_bytes(b"different-terms");
        assert!(validate_invitation_preview(&request, &preview, 95_000).is_err());

        let request = invitation_redeem_request(&preview);
        assert!(validate_invitation_preview(&request, &preview, 90_001).is_ok());
        assert!(
            validate_invitation_preview(&request, &preview, 90_001 + 5 * 60 * 1000).is_err(),
            "consent older than five minutes must be rejected"
        );
    }

    #[test]
    fn invitation_consent_rejects_unavailable_privileged_and_permanent_grants() {
        for state in [
            InvitationState::Expired,
            InvitationState::Revoked,
            InvitationState::Exhausted,
        ] {
            let mut preview = invitation_preview();
            preview.state = state;
            preview.can_accept = false;
            preview.denial_key = Some(format!("invitation.state.{state:?}").to_lowercase());
            rebind_preview_digest(&mut preview);
            let request = invitation_redeem_request(&preview);
            assert!(
                validate_invitation_preview(&request, &preview, 95_000).is_err(),
                "{state:?} invitation must not be accepted"
            );
        }

        let mut preview = invitation_preview();
        preview.remaining_uses = 0;
        rebind_preview_digest(&mut preview);
        let request = invitation_redeem_request(&preview);
        assert!(validate_invitation_preview(&request, &preview, 95_000).is_err());

        let mut preview = invitation_preview();
        preview.role = MembershipRole::Owner;
        rebind_preview_digest(&mut preview);
        let request = invitation_redeem_request(&preview);
        assert!(validate_invitation_preview(&request, &preview, 95_000).is_err());

        let mut preview = invitation_preview();
        preview.creates_permanent_membership = true;
        rebind_preview_digest(&mut preview);
        let request = invitation_redeem_request(&preview);
        assert!(validate_invitation_preview(&request, &preview, 95_000).is_err());

        let mut preview = invitation_preview();
        preview.capabilities = vec![CapabilityKind::Exit, CapabilityKind::Exit];
        rebind_preview_digest(&mut preview);
        let request = invitation_redeem_request(&preview);
        assert!(validate_invitation_preview(&request, &preview, 95_000).is_err());
    }

    #[test]
    fn invitation_redemption_must_match_consented_permissions_and_not_be_permanent() {
        let preview = invitation_preview();
        let redemption = InvitationRedemption {
            invitation_id: preview.invitation_id.clone(),
            network_id: preview.network_id.clone(),
            node_id: NodeId::from_public_key(b"member"),
            role: preview.role,
            capabilities: preview.capabilities.clone(),
            network_revision: preview.network_revision,
            invitation_revision: preview.invitation_revision + 1,
            joined_at_unix_ms: 91_000,
            already_redeemed: false,
            grants_permanent_membership: false,
        };
        assert!(validate_invitation_redemption(&preview, &redemption).is_ok());

        for mutate in [
            (|value: &mut InvitationRedemption| value.role = MembershipRole::Administrator)
                as fn(&mut InvitationRedemption),
            |value: &mut InvitationRedemption| value.capabilities.push(CapabilityKind::Exit),
            |value: &mut InvitationRedemption| value.capabilities = vec![CapabilityKind::Relay],
            |value: &mut InvitationRedemption| {
                value.network_id = NetworkId::from_public_key(b"other")
            },
            |value: &mut InvitationRedemption| value.grants_permanent_membership = true,
        ] {
            let mut candidate = redemption.clone();
            mutate(&mut candidate);
            assert!(validate_invitation_redemption(&preview, &candidate).is_err());
        }
    }

    #[tokio::test]
    async fn invitation_persistence_creates_only_ordinary_accepted_membership() {
        let dir = std::env::temp_dir().join(format!(
            "fuckipv6-runtime-handler-invitation-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let configs = ConfigManager::new_for_dir(dir.clone()).expect("config manager");
        let identity = configs.load_node_identity().expect("node identity");
        let preview = invitation_preview();

        let (stored, inserted) =
            persist_invitation_membership(&configs, preview.clone(), 90_000, 91_000)
                .await
                .expect("persist invitation membership");
        assert!(inserted);
        assert_eq!(stored.network_name, preview.network_name);
        assert_eq!(stored.membership.network_id, preview.network_id);
        assert_eq!(
            stored.membership.node_id,
            NodeId::from_public_key(&identity.public_key_bytes())
        );
        assert_eq!(
            stored.membership.state,
            fuckipv6_common::domain::MembershipState::Accepted
        );
        assert_eq!(stored.membership.role, MembershipRole::Member);
        assert_eq!(stored.membership.terms, Some(preview.terms.clone()));

        let (replayed, inserted) =
            persist_invitation_membership(&configs, preview.clone(), 90_000, 92_000)
                .await
                .expect("idempotent invitation persistence");
        assert!(!inserted);
        assert_eq!(replayed, stored);
        assert_eq!(
            configs
                .list_joined_memberships()
                .expect("memberships")
                .len(),
            1
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn runtime_errors_map_to_stable_problem_codes() {
        let cases = [
            (
                RuntimeControlError::Invalid("invalid".into()),
                NodeProblemCode::InvalidRequest,
                false,
            ),
            (
                RuntimeControlError::Conflict("conflict".into()),
                NodeProblemCode::Conflict,
                false,
            ),
            (
                RuntimeControlError::NotFound("missing".into()),
                NodeProblemCode::NotFound,
                false,
            ),
            (
                RuntimeControlError::ActorClosed,
                NodeProblemCode::Unavailable,
                true,
            ),
            (
                RuntimeControlError::Unavailable("unavailable".into()),
                NodeProblemCode::Unavailable,
                true,
            ),
            (
                RuntimeControlError::Persistence("persist".into()),
                NodeProblemCode::Unavailable,
                true,
            ),
            (
                RuntimeControlError::Launch("launch".into()),
                NodeProblemCode::Unavailable,
                true,
            ),
            (
                RuntimeControlError::Timeout("timeout".into()),
                NodeProblemCode::TimedOut,
                true,
            ),
        ];

        for (error, code, retryable) in cases {
            let result = runtime_rejection(60, error);
            assert!(!result.is_success());
            match result.outcome {
                NodeCommandOutcome::Rejected(problem) => {
                    assert_problem(&problem, code, retryable);
                }
                outcome => panic!("unexpected outcome: {outcome:?}"),
            }
        }
    }

    #[tokio::test]
    async fn command_bus_type_is_usable_without_output_adapters() {
        let _: NodeCommandBus = spawn_command_bus(
            NodeStateStore::new(),
            MultiNetworkRegistry::new(),
            NodeEventBus::default(),
        );
    }
}
