use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::{watch, Mutex};

use fuckipv6_common::domain::{
    validate_exit_contribution_policy, CapabilityKind, ExitContributionMode,
    ExitContributionObligation, ExitContributionScope, IdempotencyKey, InvitationCreateRequest,
    InvitationPreview, InvitationRedeemRequest, InvitationRedemption, InvitationState,
    JoinApplicantProfile, JoinRequest, JoinRequestGrantPreview, JoinRequestId, JoinTermsAck,
    MemberLifecycleChangeRequest, MemberRemovalPreviewRequest, MembershipRole, MembershipTerms,
    NetworkDiscoveryRecord, NetworkId, NetworkJoinPreview, NetworkMembership,
    NetworkProfileWarning, NetworkProfileWarningCode, NetworkRecord, NodeId, OperationId,
    JOIN_REQUEST_DEFAULT_EXPIRY_MS,
};
use fuckipv6_common::{Identity, JoinRequestBatchReviewItem, NetworkTicket};

use super::runtime_control::{
    local_exit_unsupported_error, network_setup_problem_code, runtime_control_problem,
    RestartSummary,
};
use super::state::{
    effective_membership_state, NodeNetworkProfileSnapshot, NodeSignedNetworkProfileSnapshot,
};
use super::{NetworkRuntimeManager, RuntimeControlError, StartSummary};
use super::{
    NodeCommandContext, NodeCommandFuture, NodeCommandHandler, NodeCommandOutcome,
    NodeCommandRequest, NodeCommandResult, NodeNetworkJoinPreview, NodeNetworkStatus,
    NodePeerProbeSnapshot, NodeProblem, NodeProblemCode, NodeStateStore, NodeValidationIssue,
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
    NodeChangeMemberLifecycleCommand, NodeJoinRequestListResult, NodeJoinRequestReviewAck,
    NodeJoinRequestReviewOutcome, NodeMemberRemovalPreviewCommand, NodeNetworkProfileCommand,
    NodeResubmitJoinRequestCommand, NodeSubmitJoinRequestCommand,
};
use crate::policy_transaction::{PolicySnapshot, PolicyTransactionError, PolicyTransactionStore};
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
    policy_store: Option<PolicyTransactionStore>,
    discovery: Option<DiscoveryIndexManager>,
    join_gate: Arc<Mutex<()>>,
    policy_gate: Arc<Mutex<()>>,
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
            policy_store: None,
            discovery: None,
            join_gate: Arc::new(Mutex::new(())),
            policy_gate: Arc::new(Mutex::new(())),
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
        let policy_store = PolicyTransactionStore::new(configs.config_dir().to_path_buf());
        Self {
            state,
            registry,
            services,
            runtime: Some(runtime),
            configs: Some(configs),
            policy_store: Some(policy_store),
            discovery: Some(discovery),
            join_gate: Arc::new(Mutex::new(())),
            policy_gate: Arc::new(Mutex::new(())),
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
        let policy_store = self.policy_store.clone();
        let discovery = self.discovery.clone();
        let join_gate = self.join_gate.clone();
        let policy_gate = self.policy_gate.clone();

        Box::pin(async move {
            let request_id = request.request_id;
            if let Err(problem) = request.validate() {
                return NodeCommandResult::rejected(request_id, problem);
            }

            match request.command {
                super::NodeCommand::GetSnapshot => {
                    if let Some(policy_store) = policy_store.as_ref() {
                        let _guard = policy_gate.lock().await;
                        if let Err(error) = refresh_policy_state(&state, policy_store).await {
                            return policy_transaction_rejection(request_id, error);
                        }
                    }
                    if let Some(discovery) = discovery.as_ref() {
                        state.replace_discovery_index(discovery.snapshot()).await;
                    }
                    if let Some(configs) = configs.as_ref() {
                        refresh_network_profiles(&state, configs).await;
                        if let Err(error) = refresh_network_memberships(&state, configs).await {
                            tracing::warn!(%error, "刷新本地网络成员关系失败，保留上一份快照");
                        }
                        refresh_local_exit_owner_intent(&state, configs).await;
                    }
                    refresh_embedded_services(&state, &services).await;
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
                super::NodeCommand::GetPolicy => match policy_store {
                    Some(policy_store) => {
                        get_policy(&state, &policy_store, &policy_gate, request_id).await
                    }
                    None => unavailable(request_id, "读取策略需要节点配置管理器"),
                },
                super::NodeCommand::PreviewPolicy(command) => match policy_store {
                    Some(policy_store) => {
                        preview_policy(&state, &policy_store, request_id, command).await
                    }
                    None => unavailable(request_id, "策略预览需要节点配置管理器"),
                },
                super::NodeCommand::PreviewPolicyRollback(command) => match policy_store {
                    Some(policy_store) => {
                        preview_policy_rollback(&state, &policy_store, request_id, command).await
                    }
                    None => unavailable(request_id, "策略回滚预览需要节点配置管理器"),
                },
                super::NodeCommand::ApplyPolicy(command) => match (policy_store, context) {
                    (Some(policy_store), Some(context)) => {
                        apply_policy(
                            &state,
                            &policy_store,
                            &policy_gate,
                            request_id,
                            command,
                            context.actor,
                            context.source,
                        )
                        .await
                    }
                    (None, _) => unavailable(request_id, "策略应用需要节点配置管理器"),
                    (_, None) => unavailable(request_id, "策略应用缺少受信任的认证上下文"),
                },
                super::NodeCommand::RollbackPolicy(command) => match (policy_store, context) {
                    (Some(policy_store), Some(context)) => {
                        rollback_policy(
                            &state,
                            &policy_store,
                            &policy_gate,
                            request_id,
                            command,
                            context.actor,
                            context.source,
                        )
                        .await
                    }
                    (None, _) => unavailable(request_id, "策略回滚需要节点配置管理器"),
                    (_, None) => unavailable(request_id, "策略回滚缺少受信任的认证上下文"),
                },
                super::NodeCommand::GetPolicyHistory => match policy_store {
                    Some(policy_store) => policy_history(&state, &policy_store, request_id).await,
                    None => unavailable(request_id, "读取策略历史需要节点配置管理器"),
                },
                super::NodeCommand::GetTrafficPolicy => unavailable(
                    request_id,
                    "读取流量策略需要已接入的生产 TUN/路由/DNS 事务适配器",
                ),
                super::NodeCommand::PreviewTrafficPolicy(_) => unavailable(
                    request_id,
                    "流量策略预览需要已接入的生产 TUN/路由/DNS 事务适配器",
                ),
                super::NodeCommand::PreviewTrafficPolicyRollback(_) => unavailable(
                    request_id,
                    "流量策略回滚预览需要已接入的生产 TUN/路由/DNS 事务适配器",
                ),
                super::NodeCommand::ApplyTrafficPolicy(_) => unavailable(
                    request_id,
                    "流量策略应用需要已接入的生产 TUN/路由/DNS 事务适配器",
                ),
                super::NodeCommand::RollbackTrafficPolicy(_) => unavailable(
                    request_id,
                    "流量策略回滚需要已接入的生产 TUN/路由/DNS 事务适配器",
                ),
                super::NodeCommand::GetTrafficPolicyHistory => unavailable(
                    request_id,
                    "读取流量策略历史需要已接入的生产 TUN/路由/DNS 事务适配器",
                ),
                super::NodeCommand::TestTrafficPolicy(command) => match command.compare() {
                    Ok(comparison) => NodeCommandResult::completed(
                        request_id,
                        NodeCommandOutcome::TrafficPolicyTest(Box::new(comparison)),
                    ),
                    Err(problem) => NodeCommandResult::rejected(request_id, problem),
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
                        let result = discovery
                            .refresh_cancellable(&state, index_url.as_deref(), cancel)
                            .await;
                        discovery_command_result(request_id, result)
                    }
                    None => unavailable(request_id, "发现索引管理器尚未初始化"),
                },
                super::NodeCommand::RetryDiscoveryIndex => match discovery {
                    Some(discovery) => {
                        let result = discovery.retry_cancellable(&state, cancel).await;
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
                super::NodeCommand::AcknowledgeMembershipPolicy(command) => match configs {
                    Some(configs) => {
                        acknowledge_membership_policy(&state, &configs, request_id, command).await
                    }
                    None => unavailable(request_id, "确认成员策略需要节点配置管理器"),
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
                super::NodeCommand::ListNetworkMembers(command) => match runtime {
                    Some(runtime) => {
                        list_network_members(
                            &runtime,
                            discovery.as_ref(),
                            &state,
                            request_id,
                            command.network_id,
                        )
                        .await
                    }
                    None => unavailable(request_id, "列出成员目录需要运行中的节点控制面"),
                },
                super::NodeCommand::PreviewMemberRemoval(command) => match runtime {
                    Some(runtime) => {
                        preview_member_removal(&runtime, discovery.as_ref(), request_id, command)
                            .await
                    }
                    None => unavailable(request_id, "预览成员移除需要运行中的节点控制面"),
                },
                super::NodeCommand::ChangeMemberLifecycle(command) => match runtime {
                    Some(runtime) => {
                        change_member_lifecycle(&runtime, discovery.as_ref(), request_id, command)
                            .await
                    }
                    None => unavailable(request_id, "变更成员生命周期需要运行中的节点控制面"),
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
                super::NodeCommand::SetLocalExitCapability(command) => {
                    if command.enabled {
                        let capabilities = state.snapshot().await.exit_node_capabilities;
                        if let Some(error) = local_exit_unsupported_error(&capabilities) {
                            return runtime_rejection(request_id, error);
                        }
                    }
                    match runtime {
                        Some(runtime) => {
                            match runtime.set_local_exit_capability(command.enabled).await {
                                Ok(snapshot) => NodeCommandResult::completed(
                                    request_id,
                                    NodeCommandOutcome::LocalExitCapability(Box::new(snapshot)),
                                ),
                                Err(error) => runtime_rejection(request_id, error),
                            }
                        }
                        None => unavailable(
                            request_id,
                            "本机 Exit 能力启停需要运行中的本机 Exit 控制面",
                        ),
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
                super::NodeCommand::GetCoordinatorCandidates(command) => {
                    let network_name = command.network_name;
                    match runtime {
                        Some(runtime) => match runtime
                            .get_coordinator_candidates(network_name.clone())
                            .await
                        {
                            Ok(status) => NodeCommandResult::completed(
                                request_id,
                                NodeCommandOutcome::CoordinatorCandidates(Box::new(status)),
                            ),
                            Err(error) => runtime_rejection(request_id, error),
                        },
                        None => match services
                            .get_coordinator_candidates(network_name.clone())
                            .await
                        {
                            Ok(status) => NodeCommandResult::completed(
                                request_id,
                                NodeCommandOutcome::CoordinatorCandidates(Box::new(status)),
                            ),
                            Err(error) => {
                                embedded_service_rejection(request_id, &network_name, error)
                            }
                        },
                    }
                }
                super::NodeCommand::UpdateCoordinatorCandidates(command) => {
                    let network_name = command.network_name.clone();
                    match runtime {
                        Some(runtime) => match runtime.update_coordinator_candidates(command).await
                        {
                            Ok(status) => NodeCommandResult::completed(
                                request_id,
                                NodeCommandOutcome::CoordinatorCandidates(Box::new(status)),
                            ),
                            Err(error) => runtime_rejection(request_id, error),
                        },
                        None => {
                            let effective_from_unix_ms = command
                                .effective_from_unix_ms
                                .unwrap_or_else(coordinator_now_unix_ms);
                            match services
                                .update_coordinator_candidates(
                                    network_name.clone(),
                                    command.expected_revision,
                                    command.candidates,
                                    effective_from_unix_ms,
                                    command.expires_at_unix_ms,
                                    command.credential,
                                )
                                .await
                            {
                                Ok(status) => NodeCommandResult::completed(
                                    request_id,
                                    NodeCommandOutcome::CoordinatorCandidates(Box::new(status)),
                                ),
                                Err(error) => {
                                    embedded_service_rejection(request_id, &network_name, error)
                                }
                            }
                        }
                    }
                }
                super::NodeCommand::PublishCoordinatorCandidateDeclaration(command) => {
                    let network_name = command.network_name.clone();
                    match runtime {
                        Some(runtime) => match runtime
                            .publish_coordinator_candidate_declaration(command)
                            .await
                        {
                            Ok(status) => NodeCommandResult::completed(
                                request_id,
                                NodeCommandOutcome::CoordinatorCandidates(Box::new(status)),
                            ),
                            Err(error) => runtime_rejection(request_id, error),
                        },
                        None => match services
                            .publish_coordinator_candidate_declaration(
                                network_name.clone(),
                                command.willingness,
                                command.public_reachability,
                                command.resources,
                                command.validity_ms,
                            )
                            .await
                        {
                            Ok(status) => NodeCommandResult::completed(
                                request_id,
                                NodeCommandOutcome::CoordinatorCandidates(Box::new(status)),
                            ),
                            Err(error) => {
                                embedded_service_rejection(request_id, &network_name, error)
                            }
                        },
                    }
                }
                super::NodeCommand::SubmitAdminRecovery(command) => match runtime {
                    Some(runtime) => match runtime.submit_admin_recovery(command).await {
                        Ok(status) => NodeCommandResult::completed(
                            request_id,
                            NodeCommandOutcome::AdminRecoveryStatus(Box::new(status)),
                        ),
                        Err(error) => runtime_rejection(request_id, error),
                    },
                    None => runtime_rejection(
                        request_id,
                        RuntimeControlError::Unavailable(
                            "管理员恢复必须通过活动的数据面运行时会话执行".into(),
                        ),
                    ),
                },
                super::NodeCommand::GetAdminRecoveryStatus(command) => match runtime {
                    Some(runtime) => match runtime.get_admin_recovery_status(command).await {
                        Ok(status) => NodeCommandResult::completed(
                            request_id,
                            NodeCommandOutcome::AdminRecoveryStatus(Box::new(status)),
                        ),
                        Err(error) => runtime_rejection(request_id, error),
                    },
                    None => runtime_rejection(
                        request_id,
                        RuntimeControlError::Unavailable(
                            "管理员恢复状态必须通过活动的数据面运行时查询".into(),
                        ),
                    ),
                },
                super::NodeCommand::ListAdminRecoveries(command) => match runtime {
                    Some(runtime) => match runtime.list_admin_recoveries(command).await {
                        Ok(statuses) => NodeCommandResult::completed(
                            request_id,
                            NodeCommandOutcome::AdminRecoveries(statuses),
                        ),
                        Err(error) => runtime_rejection(request_id, error),
                    },
                    None => runtime_rejection(
                        request_id,
                        RuntimeControlError::Unavailable(
                            "管理员恢复列表必须通过活动的数据面运行时查询".into(),
                        ),
                    ),
                },
                super::NodeCommand::RevokeAdminRecovery(command) => match runtime {
                    Some(runtime) => match runtime.revoke_admin_recovery(command).await {
                        Ok(status) => NodeCommandResult::completed(
                            request_id,
                            NodeCommandOutcome::AdminRecoveryStatus(Box::new(status)),
                        ),
                        Err(error) => runtime_rejection(request_id, error),
                    },
                    None => runtime_rejection(
                        request_id,
                        RuntimeControlError::Unavailable(
                            "管理员恢复撤销必须通过活动的数据面运行时执行".into(),
                        ),
                    ),
                },
                super::NodeCommand::ExecuteAdminRecovery(command) => match runtime {
                    Some(runtime) => match runtime.execute_admin_recovery(command).await {
                        Ok(status) => NodeCommandResult::completed(
                            request_id,
                            NodeCommandOutcome::AdminRecoveryStatus(Box::new(status)),
                        ),
                        Err(error) => runtime_rejection(request_id, error),
                    },
                    None => runtime_rejection(
                        request_id,
                        RuntimeControlError::Unavailable(
                            "管理员恢复执行必须通过活动的数据面运行时执行".into(),
                        ),
                    ),
                },
                super::NodeCommand::TestPolicy(input) => NodeCommandResult::completed(
                    request_id,
                    NodeCommandOutcome::PolicyTest(Box::new(input.evaluate())),
                ),
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

async fn refresh_local_exit_owner_intent(state: &NodeStateStore, configs: &ConfigManager) {
    let configs = configs.clone();
    let intent = tokio::task::spawn_blocking(move || configs.node_local_exit_enabled()).await;
    let current = state.snapshot().await;
    let projected = match intent {
        Ok(Ok(enabled)) => project_local_exit_owner_intent(
            &current.local_exit,
            enabled,
            &current.exit_node_capabilities,
        ),
        Ok(Err(error)) => {
            let error = RuntimeControlError::Persistence(format!(
                "读取本机 Exit owner intent 失败: {error:#}"
            ));
            let mut failed = current.local_exit.clone();
            failed.stage = super::LocalExitCapabilityStage::Failed;
            failed.healthy = false;
            failed.problem = Some(runtime_control_problem(&error));
            failed
        }
        Err(error) => {
            let error = RuntimeControlError::Persistence(format!(
                "读取本机 Exit owner intent 的任务异常终止: {error}"
            ));
            let mut failed = current.local_exit.clone();
            failed.stage = super::LocalExitCapabilityStage::Failed;
            failed.healthy = false;
            failed.problem = Some(runtime_control_problem(&error));
            failed
        }
    };

    if projected != current.local_exit {
        state
            .update(|snapshot| {
                snapshot.local_exit = projected.clone();
                snapshot.local_exit.last_transition_at_unix_ms = unix_time_ms();
            })
            .await;
    }
}

fn project_local_exit_owner_intent(
    current: &super::LocalExitCapabilitySnapshot,
    owner_exit_enabled: bool,
    capabilities: &fuckipv6_common::ExitNodeCapabilities,
) -> super::LocalExitCapabilitySnapshot {
    if !owner_exit_enabled {
        if matches!(
            current.stage,
            super::LocalExitCapabilityStage::Draining
                | super::LocalExitCapabilityStage::Cleaning
                | super::LocalExitCapabilityStage::Failed
        ) {
            let mut projected = current.clone();
            projected.owner_exit_enabled = false;
            projected.healthy = false;
            return projected;
        }
        return super::LocalExitCapabilitySnapshot {
            last_transition_at_unix_ms: current.last_transition_at_unix_ms,
            ..super::LocalExitCapabilitySnapshot::default()
        };
    }

    if let Some(error) = local_exit_unsupported_error(capabilities) {
        return super::LocalExitCapabilitySnapshot {
            owner_exit_enabled: true,
            stage: super::LocalExitCapabilityStage::Failed,
            problem: Some(runtime_control_problem(&error)),
            last_transition_at_unix_ms: current.last_transition_at_unix_ms,
            ..super::LocalExitCapabilitySnapshot::default()
        };
    }

    if !current.owner_exit_enabled {
        return super::LocalExitCapabilitySnapshot {
            owner_exit_enabled: true,
            stage: super::LocalExitCapabilityStage::WaitingForNetworkAuthorization,
            last_transition_at_unix_ms: current.last_transition_at_unix_ms,
            ..super::LocalExitCapabilitySnapshot::default()
        };
    }

    let mut projected = current.clone();
    projected.owner_exit_enabled = true;
    if matches!(
        projected.stage,
        super::LocalExitCapabilityStage::Disabled
            | super::LocalExitCapabilityStage::WaitingForNetworkAuthorization
    ) {
        projected.stage = if projected.network_allowed {
            super::LocalExitCapabilityStage::HealthChecking
        } else {
            super::LocalExitCapabilityStage::WaitingForNetworkAuthorization
        };
        projected.healthy = false;
    }
    projected
}

async fn refresh_network_memberships(
    state: &NodeStateStore,
    configs: &ConfigManager,
) -> Result<(), String> {
    let snapshot = state.snapshot().await;
    let runtime_states = snapshot
        .networks
        .iter()
        .map(|network| (network.name.clone(), network.status))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut current_terms = snapshot
        .discovery_index
        .records
        .iter()
        .filter(|record| record.discoverable)
        .map(|record| {
            let document = &record.record.document;
            (
                document.network_id.as_str().to_owned(),
                fuckipv6_common::domain::MembershipTerms {
                    network_policy_revision: document.policy_revision,
                    privacy_policy_revision: document.privacy_policy_revision,
                    exit_contribution: document.exit_contribution,
                    exit_contribution_scope: document.exit_contribution_scope,
                    exit_contribution_mode: document.exit_contribution_mode,
                    member_limit: document.member_limit,
                },
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let configs = configs.clone();
    let memberships = tokio::task::spawn_blocking(move || {
        // Unlisted networks are intentionally absent from the public discovery
        // index. Their owner still keeps the authoritative signed network
        // record in the local network directory, so use it to keep the local
        // membership projection current. Only inspect records referenced by an
        // existing membership so one unrelated corrupt directory cannot block
        // every membership refresh.
        for joined in configs
            .list_joined_memberships()
            .map_err(|error| error.to_string())?
        {
            let Ok(record) = configs.load_network_record(&joined.network_name) else {
                continue;
            };
            if record.network_id == joined.membership.network_id {
                current_terms.insert(
                    record.network_id.as_str().to_owned(),
                    record.membership_terms(),
                );
            }
        }
        configs
            .observe_joined_membership_policies(&current_terms, unix_time_ms())
            .map_err(|error| error.to_string())?;
        let mut memberships = configs
            .list_joined_memberships()
            .map_err(|error| error.to_string())?;
        memberships.sort_by(|left, right| left.network_name.cmp(&right.network_name));
        Ok::<_, String>(
            memberships
                .iter()
                .map(|joined| {
                    network_membership_snapshot(
                        joined,
                        runtime_states.get(&joined.network_name).copied(),
                    )
                })
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
    runtime: Option<NodeNetworkStatus>,
) -> super::NodeNetworkMembershipSnapshot {
    let membership = &joined.membership;
    super::NodeNetworkMembershipSnapshot {
        network_name: joined.network_name.clone(),
        network_id: membership.network_id.to_string(),
        node_id: membership.node_id.to_string(),
        state: effective_membership_state(membership.state, runtime),
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
        exit_contribution_scope: membership
            .terms
            .as_ref()
            .map(|terms| terms.exit_contribution_scope),
        exit_contribution_mode: membership
            .terms
            .as_ref()
            .map(|terms| terms.exit_contribution_mode),
        terms_hash: membership
            .terms_ack
            .as_ref()
            .map(|ack| ack.terms_hash.as_str().to_owned()),
        current_terms: membership.current_terms.clone(),
        current_terms_hash: membership
            .current_terms
            .as_ref()
            .and_then(|terms| terms.content_hash().ok())
            .map(|hash| hash.as_str().to_owned()),
        policy_confirmation_required: membership.policy_confirmation_required(),
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
        exit_contribution: record.exit_contribution,
        exit_contribution_scope: record.exit_contribution_scope,
        exit_contribution_mode: record.exit_contribution_mode,
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

fn policy_snapshot_from_state(snapshot: &super::NodeSnapshot) -> PolicySnapshot {
    PolicySnapshot {
        local_policy: snapshot.access_control.local_policy.clone(),
        network_acls: snapshot.access_control.network_acls.clone(),
    }
}

async fn policy_fallback(state: &NodeStateStore) -> PolicySnapshot {
    policy_snapshot_from_state(&state.snapshot().await)
}

/// Replace only the policy-owned portion of the authoritative snapshot. Field
/// privacy documents are governed separately and must survive policy writes.
async fn publish_policy_snapshot(state: &NodeStateStore, snapshot: PolicySnapshot) -> bool {
    let current = state.snapshot().await;
    if current.access_control.local_policy == snapshot.local_policy
        && current.access_control.network_acls == snapshot.network_acls
    {
        return false;
    }
    state
        .update(|current| {
            current.access_control.local_policy = snapshot.local_policy;
            current.access_control.network_acls = snapshot.network_acls;
        })
        .await;
    true
}

async fn refresh_policy_state(
    state: &NodeStateStore,
    store: &PolicyTransactionStore,
) -> std::result::Result<(), PolicyTransactionError> {
    let fallback = policy_fallback(state).await;
    let store = store.clone();
    let current = tokio::task::spawn_blocking(move || store.current(&fallback))
        .await
        .map_err(|error| PolicyTransactionError::Persistence(error.to_string()))??;
    publish_policy_snapshot(state, current.snapshot).await;
    Ok(())
}

fn policy_transaction_rejection(
    request_id: u64,
    error: PolicyTransactionError,
) -> NodeCommandResult {
    let problem = match error {
        PolicyTransactionError::RevisionConflict { expected, current } => {
            NodeProblem::new(NodeProblemCode::Conflict, "策略已被其他操作修改")
                .with_message_key("policy.error.revision_conflict")
                .with_param("expected_revision", expected.to_string())
                .with_param("current_revision", current.to_string())
                .with_revision_conflict(expected, current)
        }
        PolicyTransactionError::ScopeMismatch { declared, actual } => NodeProblem::new(
            NodeProblemCode::InvalidRequest,
            "声明的策略影响范围与候选实际变更不一致",
        )
        .with_message_key("policy.error.scope_mismatch")
        .with_param("declared_networks", declared.join(","))
        .with_param("actual_networks", actual.join(","))
        .with_validation_issues(vec![NodeValidationIssue {
            path: "network_ids".into(),
            code: "scope_mismatch".into(),
            message: "必须准确声明预览中受影响的所有网络和本地策略".into(),
        }]),
        PolicyTransactionError::InvalidCandidate(message) => {
            NodeProblem::new(NodeProblemCode::InvalidRequest, "策略候选校验失败")
                .with_message_key("policy.error.invalid_candidate")
                .with_validation_issues(vec![NodeValidationIssue {
                    path: "candidate".into(),
                    code: "invalid_candidate".into(),
                    message,
                }])
        }
        PolicyTransactionError::ExplicitConfirmationRequired => NodeProblem::new(
            NodeProblemCode::InvalidRequest,
            "高风险权限扩大必须显式确认后才能应用",
        )
        .with_message_key("policy.error.explicit_confirmation_required")
        .with_param("confirmation_required", "true"),
        PolicyTransactionError::RevisionNotFound(revision) => {
            NodeProblem::new(NodeProblemCode::NotFound, "目标策略 revision 不存在")
                .with_message_key("policy.error.target_revision_not_found")
                .with_param("target_revision", revision.to_string())
        }
        PolicyTransactionError::Persistence(details) => {
            tracing::error!(%details, "策略事务持久化失败，旧策略保持有效");
            NodeProblem::new(
                NodeProblemCode::Internal,
                "策略持久化失败，旧 revision 保持不变",
            )
            .with_message_key("policy.error.persistence_failed")
        }
    };
    NodeCommandResult::rejected(request_id, problem)
}

async fn get_policy(
    state: &NodeStateStore,
    store: &PolicyTransactionStore,
    policy_gate: &Mutex<()>,
    request_id: u64,
) -> NodeCommandResult {
    let _guard = policy_gate.lock().await;
    let fallback = policy_fallback(state).await;
    let store = store.clone();
    match tokio::task::spawn_blocking(move || store.current(&fallback)).await {
        Ok(Ok(current)) => {
            publish_policy_snapshot(state, current.snapshot.clone()).await;
            NodeCommandResult::completed(request_id, NodeCommandOutcome::Policy(current))
        }
        Ok(Err(error)) => policy_transaction_rejection(request_id, error),
        Err(error) => policy_transaction_rejection(
            request_id,
            PolicyTransactionError::Persistence(format!("读取策略任务异常终止: {error}")),
        ),
    }
}

async fn preview_policy(
    state: &NodeStateStore,
    store: &PolicyTransactionStore,
    request_id: u64,
    command: super::NodePreviewPolicyCommand,
) -> NodeCommandResult {
    let fallback = policy_fallback(state).await;
    let store = store.clone();
    match tokio::task::spawn_blocking(move || {
        store.preview(&fallback, command.expected_revision, command.candidate)
    })
    .await
    {
        Ok(Ok(preview)) => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::PolicyPreview(Box::new(preview)),
        ),
        Ok(Err(error)) => policy_transaction_rejection(request_id, error),
        Err(error) => policy_transaction_rejection(
            request_id,
            PolicyTransactionError::Persistence(format!("策略预览任务异常终止: {error}")),
        ),
    }
}

async fn preview_policy_rollback(
    state: &NodeStateStore,
    store: &PolicyTransactionStore,
    request_id: u64,
    command: super::NodePreviewPolicyRollbackCommand,
) -> NodeCommandResult {
    let fallback = policy_fallback(state).await;
    let store = store.clone();
    match tokio::task::spawn_blocking(move || {
        store.preview_rollback(
            &fallback,
            command.expected_revision,
            command.target_revision,
        )
    })
    .await
    {
        Ok(Ok(preview)) => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::PolicyPreview(Box::new(preview)),
        ),
        Ok(Err(error)) => policy_transaction_rejection(request_id, error),
        Err(error) => policy_transaction_rejection(
            request_id,
            PolicyTransactionError::Persistence(format!("策略回滚预览任务异常终止: {error}")),
        ),
    }
}

async fn authoritative_policy_snapshot(
    store: &PolicyTransactionStore,
    fallback: &PolicySnapshot,
    result_snapshot: &PolicySnapshot,
) -> PolicySnapshot {
    let store = store.clone();
    let fallback = fallback.clone();
    match tokio::task::spawn_blocking(move || store.current_snapshot(&fallback)).await {
        Ok(Ok(snapshot)) => snapshot,
        Ok(Err(error)) => {
            tracing::warn!(%error, "策略已提交但读取最新权威快照失败，先发布本次提交快照");
            result_snapshot.clone()
        }
        Err(error) => {
            tracing::warn!(%error, "策略已提交但权威快照任务异常，先发布本次提交快照");
            result_snapshot.clone()
        }
    }
}

async fn apply_policy(
    state: &NodeStateStore,
    store: &PolicyTransactionStore,
    policy_gate: &Mutex<()>,
    request_id: u64,
    command: super::NodeApplyPolicyCommand,
    actor: String,
    source: String,
) -> NodeCommandResult {
    let _guard = policy_gate.lock().await;
    let fallback = policy_fallback(state).await;
    let authoritative_store = store.clone();
    let store = store.clone();
    match tokio::task::spawn_blocking(move || {
        store.apply(
            &fallback,
            command.expected_revision,
            command.candidate,
            &command.network_ids,
            command.includes_local_policy,
            command.confirm_privilege_expansion,
            &actor,
            &source,
        )
    })
    .await
    {
        Ok(Ok(result)) => {
            if !result.no_op {
                let fallback = policy_fallback(state).await;
                let snapshot = authoritative_policy_snapshot(
                    &authoritative_store,
                    &fallback,
                    &result.snapshot,
                )
                .await;
                publish_policy_snapshot(state, snapshot).await;
            }
            NodeCommandResult::completed(
                request_id,
                NodeCommandOutcome::PolicyApplied(Box::new(result)),
            )
        }
        Ok(Err(error)) => policy_transaction_rejection(request_id, error),
        Err(error) => policy_transaction_rejection(
            request_id,
            PolicyTransactionError::Persistence(format!("策略应用任务异常终止: {error}")),
        ),
    }
}

async fn rollback_policy(
    state: &NodeStateStore,
    store: &PolicyTransactionStore,
    policy_gate: &Mutex<()>,
    request_id: u64,
    command: super::NodeRollbackPolicyCommand,
    actor: String,
    source: String,
) -> NodeCommandResult {
    let _guard = policy_gate.lock().await;
    let fallback = policy_fallback(state).await;
    let authoritative_store = store.clone();
    let store = store.clone();
    match tokio::task::spawn_blocking(move || {
        store.rollback(
            &fallback,
            command.expected_revision,
            command.target_revision,
            &command.network_ids,
            command.includes_local_policy,
            command.confirm_privilege_expansion,
            &actor,
            &source,
        )
    })
    .await
    {
        Ok(Ok(result)) => {
            let fallback = policy_fallback(state).await;
            let snapshot =
                authoritative_policy_snapshot(&authoritative_store, &fallback, &result.snapshot)
                    .await;
            publish_policy_snapshot(state, snapshot).await;
            NodeCommandResult::completed(
                request_id,
                NodeCommandOutcome::PolicyRolledBack(Box::new(result)),
            )
        }
        Ok(Err(error)) => policy_transaction_rejection(request_id, error),
        Err(error) => policy_transaction_rejection(
            request_id,
            PolicyTransactionError::Persistence(format!("策略回滚任务异常终止: {error}")),
        ),
    }
}

async fn policy_history(
    state: &NodeStateStore,
    store: &PolicyTransactionStore,
    request_id: u64,
) -> NodeCommandResult {
    let fallback = policy_fallback(state).await;
    let store = store.clone();
    match tokio::task::spawn_blocking(move || store.history_snapshot(&fallback)).await {
        Ok(Ok(history)) => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::PolicyHistory(Box::new(history)),
        ),
        Ok(Err(error)) => policy_transaction_rejection(request_id, error),
        Err(error) => policy_transaction_rejection(
            request_id,
            PolicyTransactionError::Persistence(format!("读取策略历史任务异常终止: {error}")),
        ),
    }
}

fn coordinator_now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().try_into().unwrap_or(u64::MAX))
        .unwrap_or_default()
}

fn embedded_service_rejection(
    request_id: u64,
    network_name: &str,
    error: crate::symmetric::EmbeddedServiceError,
) -> NodeCommandResult {
    use crate::symmetric::EmbeddedServiceError;

    let (code, retryable, reason_code) = match &error {
        EmbeddedServiceError::InvalidRequest(_) => (NodeProblemCode::InvalidRequest, false, None),
        EmbeddedServiceError::NotFound(_) => (NodeProblemCode::NotFound, false, None),
        EmbeddedServiceError::PersistenceFailed { .. }
        | EmbeddedServiceError::OperationFailed { .. }
        | EmbeddedServiceError::ActorClosed => (NodeProblemCode::Unavailable, true, None),
        EmbeddedServiceError::AlreadyRunning(_)
        | EmbeddedServiceError::StartFailed { .. }
        | EmbeddedServiceError::StopFailed { .. }
        | EmbeddedServiceError::ShutdownFailed { .. } => (NodeProblemCode::Unavailable, true, None),
        EmbeddedServiceError::RelayStartFailed { reason_code, .. }
        | EmbeddedServiceError::RelayStopFailed { reason_code, .. } => match *reason_code {
            "relay.start.platform_unsupported" => {
                (NodeProblemCode::Unsupported, false, Some(*reason_code))
            }
            "relay.start.permission_denied" => {
                (NodeProblemCode::PermissionDenied, false, Some(*reason_code))
            }
            _ => (NodeProblemCode::Unavailable, true, Some(*reason_code)),
        },
    };
    let mut problem = NodeProblem::new(code, error.to_string())
        .for_network(network_name)
        .retryable(retryable);
    if let Some(reason_code) = reason_code {
        problem = problem.with_param("reason", reason_code);
    }
    NodeCommandResult::rejected(request_id, problem)
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
        Ok(diagnostics) => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::Diagnostics(Box::new(diagnostics)),
        ),
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
        NodeDiscoveryIndexErrorCategory::Unsupported
        | NodeDiscoveryIndexErrorCategory::IncompatibleVersion => NodeProblemCode::Unsupported,
        NodeDiscoveryIndexErrorCategory::Rejected | NodeDiscoveryIndexErrorCategory::Signature => {
            NodeProblemCode::InvalidRequest
        }
        NodeDiscoveryIndexErrorCategory::Storage => NodeProblemCode::Internal,
        NodeDiscoveryIndexErrorCategory::Cancelled => NodeProblemCode::Cancelled,
        NodeDiscoveryIndexErrorCategory::Offline
        | NodeDiscoveryIndexErrorCategory::Timeout
        | NodeDiscoveryIndexErrorCategory::Tls
        | NodeDiscoveryIndexErrorCategory::PartialCorruption
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
    apply_local_join_capabilities(&mut preview, &snapshot);
    NodeCommandResult::completed(
        request_id,
        NodeCommandOutcome::NetworkJoinPreview(Box::new(preview)),
    )
}

fn apply_local_join_capabilities(
    preview: &mut NodeNetworkJoinPreview,
    snapshot: &super::NodeSnapshot,
) {
    preview.locally_contributed_capabilities = snapshot
        .capabilities
        .capabilities
        .iter()
        .filter(|capability| capability.state.is_operational())
        .map(|capability| capability.kind)
        .collect();
    preview.local_exit_available = snapshot.exit_node_capabilities.supported;
    preview.local_exit_enabled = snapshot.local_exit.owner_exit_enabled;
    if preview.exit_contribution_mode.requires_join_commitment() && !preview.local_exit_available {
        preview.can_join = false;
        preview.blocked_reason_key = Some("join.error.exit_capability_required".into());
    }
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
        exit_contribution_scope: document.exit_contribution_scope,
        exit_contribution_mode: document.exit_contribution_mode,
        member_limit: document.member_limit,
    };
    let grant_preview = JoinRequestGrantPreview {
        role: MembershipRole::Member,
        capabilities: document.capability_summary.clone(),
        exit_contribution: document.exit_contribution,
        exit_contribution_scope: document.exit_contribution_scope,
        exit_contribution_mode: document.exit_contribution_mode,
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

fn configured_network_conflicts_with_invitation(
    configured_network_id: Option<&str>,
    invitation_network_id: &NetworkId,
) -> bool {
    configured_network_id.is_some_and(|configured| configured != invitation_network_id.as_str())
}

fn exit_contribution_obligation_audit_value(
    obligation: ExitContributionObligation,
) -> &'static str {
    match obligation {
        ExitContributionObligation::Required => "required",
        ExitContributionObligation::NotRequired => "not_required",
    }
}

fn exit_contribution_mode_audit_value(mode: ExitContributionMode) -> &'static str {
    match mode {
        ExitContributionMode::Autonomous => "autonomous",
        ExitContributionMode::RequiredOnJoin => "required_on_join",
        ExitContributionMode::Disabled => "disabled",
        ExitContributionMode::ApprovalRequired => "approval_required",
    }
}

fn exit_contribution_scope_audit_value(scope: ExitContributionScope) -> &'static str {
    match scope {
        ExitContributionScope::NotApplicable => "not_applicable",
        ExitContributionScope::JoiningMembers => "joining_members",
        ExitContributionScope::AllMembers => "all_members",
    }
}

/// Enforce the two independent conditions for an Exit contribution obligation.
/// Network policy can require informed consent, but it can never manufacture
/// host capability or enable the local Exit service.
fn validate_exit_contribution_admission(
    snapshot: &super::NodeSnapshot,
    obligation: ExitContributionObligation,
    scope: ExitContributionScope,
    mode: ExitContributionMode,
    acknowledged: Option<bool>,
    network_name: &str,
) -> Result<(), NodeProblem> {
    if validate_exit_contribution_policy(obligation, scope, mode).is_err() {
        return Err(NodeProblem::new(
            NodeProblemCode::InvalidRequest,
            "Exit 贡献模式与兼容义务字段不一致",
        )
        .with_message_key("join.error.invalid_exit_contribution_policy")
        .with_param(
            "policy_exit_contribution_mode",
            exit_contribution_mode_audit_value(mode),
        )
        .with_param(
            "policy_exit_contribution",
            exit_contribution_obligation_audit_value(obligation),
        )
        .with_param(
            "policy_exit_contribution_scope",
            exit_contribution_scope_audit_value(scope),
        )
        .retryable(false)
        .for_network(network_name.to_owned()));
    }
    if !mode.requires_join_commitment() {
        return Ok(());
    }
    if !snapshot.exit_node_capabilities.supported {
        return Err(NodeProblem::new(
            NodeProblemCode::Unsupported,
            "当前节点不支持 Exit 能力，无法接受要求贡献 Exit 的网络",
        )
        .with_message_key("join.error.exit_capability_required")
        .with_param(
            "policy_exit_contribution",
            exit_contribution_obligation_audit_value(obligation),
        )
        .with_param(
            "policy_exit_contribution_scope",
            exit_contribution_scope_audit_value(scope),
        )
        .retryable(false)
        .for_network(network_name.to_owned()));
    }
    if acknowledged != Some(true) {
        return Err(NodeProblem::new(
            NodeProblemCode::InvalidRequest,
            "加入前必须单独确认 Exit 贡献用途、带宽、日志边界与关闭条件",
        )
        .with_message_key("join.error.exit_contribution_consent_required")
        .with_param(
            "policy_exit_contribution",
            exit_contribution_obligation_audit_value(obligation),
        )
        .with_param(
            "policy_exit_contribution_scope",
            exit_contribution_scope_audit_value(scope),
        )
        .retryable(false)
        .for_network(network_name.to_owned()));
    }
    Ok(())
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
        )
        .with_param(
            "policy_network_revision",
            preview.terms.network_policy_revision.to_string(),
        )
        .with_param(
            "policy_privacy_revision",
            preview.terms.privacy_policy_revision.to_string(),
        )
        .with_param("policy_terms_hash", preview.terms_hash.as_str()));
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
    if preview
        .terms
        .exit_contribution_mode
        .requires_join_commitment()
        && request.exit_contribution_acknowledged != Some(true)
    {
        return Err(NodeProblem::new(
            NodeProblemCode::InvalidRequest,
            "接受邀请前必须单独确认 Exit 贡献义务",
        )
        .with_message_key("join.error.exit_contribution_consent_required")
        .with_param(
            "policy_exit_contribution_mode",
            exit_contribution_mode_audit_value(preview.terms.exit_contribution_mode),
        )
        .with_param(
            "policy_exit_contribution",
            exit_contribution_obligation_audit_value(preview.terms.exit_contribution),
        )
        .with_param(
            "policy_exit_contribution_scope",
            exit_contribution_scope_audit_value(preview.terms.exit_contribution_scope),
        )
        .with_param(
            "policy_network_revision",
            preview.terms.network_policy_revision.to_string(),
        )
        .with_param(
            "policy_privacy_revision",
            preview.terms.privacy_policy_revision.to_string(),
        )
        .with_param("policy_terms_hash", preview.terms_hash.as_str())
        .retryable(false)
        .for_network(preview.network_name.clone()));
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
    if let Err(problem) = validate_exit_contribution_admission(
        &state.snapshot().await,
        preview.terms.exit_contribution,
        preview.terms.exit_contribution_scope,
        preview.terms.exit_contribution_mode,
        command.request.exit_contribution_acknowledged,
        &preview.network_name,
    ) {
        return NodeCommandResult::rejected(request_id, problem);
    }

    let conflicting_config = configs.list_networks().ok().and_then(|networks| {
        networks.into_iter().find(|network| {
            network.name == preview.network_name
                && configured_network_conflicts_with_invitation(
                    network.network_id.as_deref(),
                    &preview.network_id,
                )
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
        exit_contribution_acknowledged: command.request.exit_contribution_acknowledged,
        signed_join_preview: None,
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

async fn acknowledge_membership_policy(
    state: &NodeStateStore,
    configs: &ConfigManager,
    request_id: u64,
    command: crate::node::NodeAcknowledgeMembershipPolicyCommand,
) -> NodeCommandResult {
    let loaded_configs = configs.clone();
    let network_id = command.network_id.clone();
    let loaded = tokio::task::spawn_blocking(move || loaded_configs.joined_membership(&network_id))
        .await
        .map_err(|error| format!("读取成员策略任务失败: {error}"));
    let joined = match loaded {
        Ok(Ok(Some(joined))) => joined,
        Ok(Ok(None)) => {
            return NodeCommandResult::rejected(
                request_id,
                NodeProblem::new(NodeProblemCode::NotFound, "本地不存在该网络的成员关系")
                    .with_message_key("membership.policy.error.not_found")
                    .retryable(false),
            )
        }
        Ok(Err(error)) => {
            return NodeCommandResult::rejected(
                request_id,
                NodeProblem::new(
                    NodeProblemCode::Unavailable,
                    format!("读取成员策略失败: {error}"),
                )
                .with_message_key("membership.policy.error.storage")
                .retryable(true),
            )
        }
        Err(error) => {
            return NodeCommandResult::rejected(
                request_id,
                NodeProblem::new(
                    NodeProblemCode::Unavailable,
                    format!("读取成员策略任务失败: {error}"),
                )
                .with_message_key("membership.policy.error.storage")
                .retryable(true),
            )
        }
    };
    let Some(current_terms) = joined.membership.current_terms.as_ref() else {
        return NodeCommandResult::rejected(
            request_id,
            NodeProblem::new(NodeProblemCode::Conflict, "尚无待确认的成员策略")
                .with_message_key("membership.policy.error.no_pending_policy")
                .retryable(true)
                .for_network(joined.network_name.clone()),
        );
    };
    let Ok(current_hash) = current_terms.content_hash() else {
        return NodeCommandResult::rejected(
            request_id,
            NodeProblem::new(NodeProblemCode::Internal, "成员策略内容哈希失败")
                .with_message_key("membership.policy.error.invalid_terms")
                .retryable(false)
                .for_network(joined.network_name.clone()),
        );
    };
    if current_hash != command.expected_terms_hash {
        let mut problem = NodeProblem::new(
            NodeProblemCode::Conflict,
            "成员策略在确认前已变化，请查看最新条款后重新确认",
        )
        .with_message_key("membership.policy.error.changed")
        .retryable(true)
        .for_network(joined.network_name.clone());
        problem.details = Some(Box::new(super::NodeProblemDetails {
            current_revision: Some(current_terms.network_policy_revision),
            ..Default::default()
        }));
        return NodeCommandResult::rejected(request_id, problem);
    }
    let now = unix_time_ms();
    if command.acknowledged_at_unix_ms > now.saturating_add(5 * 60 * 1000)
        || now
            > command
                .acknowledged_at_unix_ms
                .saturating_add(5 * 60 * 1000)
    {
        return NodeCommandResult::rejected(
            request_id,
            NodeProblem::new(NodeProblemCode::Conflict, "成员策略确认已过期，请重新确认")
                .with_message_key("membership.policy.error.stale_consent")
                .retryable(true)
                .for_network(joined.network_name.clone()),
        );
    }
    if let Err(problem) = validate_exit_contribution_admission(
        &state.snapshot().await,
        current_terms.exit_contribution,
        current_terms.exit_contribution_scope,
        current_terms.exit_contribution_mode,
        command.exit_contribution_acknowledged,
        &joined.network_name,
    ) {
        return NodeCommandResult::rejected(request_id, problem);
    }

    let ack = JoinTermsAck {
        terms_hash: current_hash,
        acknowledged_at_unix_ms: command.acknowledged_at_unix_ms,
    };
    let saved_configs = configs.clone();
    let expected_hash = command.expected_terms_hash;
    let saved = tokio::task::spawn_blocking(move || {
        saved_configs.acknowledge_joined_membership_policy(
            joined.membership.network_id.as_str(),
            &expected_hash,
            ack,
            command.acknowledged_at_unix_ms,
        )
    })
    .await;
    let saved = match saved {
        Ok(Ok(saved)) => saved,
        Ok(Err(error)) => {
            let error = error.to_string();
            let code = if error.contains("策略变化") {
                NodeProblemCode::Conflict
            } else {
                NodeProblemCode::Unavailable
            };
            return NodeCommandResult::rejected(
                request_id,
                NodeProblem::new(code, format!("保存成员策略确认失败: {error}"))
                    .with_message_key("membership.policy.error.storage")
                    .retryable(code == NodeProblemCode::Unavailable),
            );
        }
        Err(error) => {
            return NodeCommandResult::rejected(
                request_id,
                NodeProblem::new(
                    NodeProblemCode::Unavailable,
                    format!("保存成员策略确认任务失败: {error}"),
                )
                .with_message_key("membership.policy.error.storage")
                .retryable(true),
            )
        }
    };
    if let Err(error) = refresh_network_memberships(state, configs).await {
        tracing::warn!(%error, "成员策略确认后刷新成员关系失败");
    }
    let snapshot = state.snapshot().await;
    let membership = snapshot
        .memberships
        .iter()
        .find(|membership| membership.network_id == saved.membership.network_id.as_str())
        .cloned()
        .unwrap_or_else(|| {
            network_membership_snapshot(
                &saved,
                snapshot
                    .networks
                    .iter()
                    .find(|network| network.name == saved.network_name)
                    .map(|network| network.status),
            )
        });
    NodeCommandResult::completed(
        request_id,
        NodeCommandOutcome::MembershipPolicyAcknowledged(Box::new(membership)),
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

async fn list_network_members(
    runtime: &NetworkRuntimeManager,
    discovery: Option<&DiscoveryIndexManager>,
    state: &NodeStateStore,
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
        .list_network_members(network_id, coordinator_url)
        .await
    {
        Ok(directory) => {
            // Cache the authoritative roster so the derived topology reflects
            // the same members without issuing a second Coordinator request.
            state.set_member_directory(directory.clone()).await;
            NodeCommandResult::completed(
                request_id,
                NodeCommandOutcome::NetworkMembers(Box::new(directory)),
            )
        }
        Err(error) => runtime_rejection(request_id, error),
    }
}

async fn preview_member_removal(
    runtime: &NetworkRuntimeManager,
    discovery: Option<&DiscoveryIndexManager>,
    request_id: u64,
    command: NodeMemberRemovalPreviewCommand,
) -> NodeCommandResult {
    let network_id = match NetworkId::parse(command.network_id) {
        Ok(network_id) => network_id,
        Err(_) => {
            return join_request_rejection(
                request_id,
                "member_lifecycle.error.invalid_network",
                "网络 ID 格式无效",
            )
        }
    };
    let target_node_id = match NodeId::parse(command.target_node_id) {
        Ok(node_id) => node_id,
        Err(_) => {
            return join_request_rejection(
                request_id,
                "member_lifecycle.error.invalid_target",
                "目标节点 ID 格式无效",
            )
        }
    };
    let coordinator_url = approval_coordinator_url(discovery, &network_id).await;
    let request = MemberRemovalPreviewRequest {
        network_id: network_id.clone(),
        target_node_id,
    };
    match runtime
        .preview_member_removal(request, coordinator_url)
        .await
    {
        Ok(preview) => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::MemberRemovalPreview(Box::new(preview)),
        ),
        Err(error) => runtime_rejection_for_network(request_id, error, network_id.to_string()),
    }
}

async fn change_member_lifecycle(
    runtime: &NetworkRuntimeManager,
    discovery: Option<&DiscoveryIndexManager>,
    request_id: u64,
    command: NodeChangeMemberLifecycleCommand,
) -> NodeCommandResult {
    let network_id = match NetworkId::parse(command.network_id) {
        Ok(network_id) => network_id,
        Err(_) => {
            return join_request_rejection(
                request_id,
                "member_lifecycle.error.invalid_network",
                "网络 ID 格式无效",
            )
        }
    };
    let target_node_id = match NodeId::parse(command.target_node_id) {
        Ok(node_id) => node_id,
        Err(_) => {
            return join_request_rejection(
                request_id,
                "member_lifecycle.error.invalid_target",
                "目标节点 ID 格式无效",
            )
        }
    };
    let reason = command.reason.map(|reason| reason.trim().to_owned());
    let idempotency_seed = serde_json::to_vec(&(
        network_id.as_str(),
        target_node_id.as_str(),
        command.action,
        command.role,
        command.expected_revision,
        reason.as_deref(),
    ))
    .unwrap_or_else(|_| {
        format!(
            "{}:{}:{:?}:{:?}:{}",
            network_id, target_node_id, command.action, command.role, command.expected_revision
        )
        .into_bytes()
    });
    let request = MemberLifecycleChangeRequest {
        network_id: network_id.clone(),
        target_node_id,
        action: command.action,
        role: command.role,
        expected_revision: command.expected_revision,
        idempotency_key: IdempotencyKey::from_public_key(&idempotency_seed),
        reason,
    };
    let coordinator_url = approval_coordinator_url(discovery, &network_id).await;
    match runtime
        .change_member_lifecycle(request, coordinator_url)
        .await
    {
        Ok(change) => NodeCommandResult::completed(
            request_id,
            NodeCommandOutcome::MemberLifecycleChanged(Box::new(change)),
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

#[derive(Clone)]
struct JoinAcceptance {
    network_name: String,
    network_id: NetworkId,
    terms: MembershipTerms,
    ack: JoinTermsAck,
}

impl JoinAcceptance {
    fn from_discovery_record(record: &NetworkDiscoveryRecord, ack: JoinTermsAck) -> Self {
        Self {
            network_name: record.document.name.clone(),
            network_id: record.document.network_id.clone(),
            terms: MembershipTerms {
                network_policy_revision: record.document.policy_revision,
                privacy_policy_revision: record.document.privacy_policy_revision,
                exit_contribution: record.document.exit_contribution,
                exit_contribution_scope: record.document.exit_contribution_scope,
                exit_contribution_mode: record.document.exit_contribution_mode,
                member_limit: record.document.member_limit,
            },
            ack,
        }
    }

    fn from_signed_preview(preview: &NetworkJoinPreview, ack: JoinTermsAck) -> Self {
        Self {
            network_name: preview.network_name.clone(),
            network_id: preview.network_id.clone(),
            terms: MembershipTerms {
                network_policy_revision: preview.terms.network_policy_revision,
                privacy_policy_revision: preview.terms.privacy_policy_revision,
                exit_contribution: preview.terms.exit_contribution,
                exit_contribution_scope: preview.terms.exit_contribution_scope,
                exit_contribution_mode: preview.terms.exit_contribution_mode,
                member_limit: preview.terms.member_limit,
            },
            ack,
        }
    }
}

fn signed_ticket_join_rejection(
    network_name: impl Into<String>,
    key: &'static str,
    message: impl Into<String>,
) -> NodeProblem {
    NodeProblem::new(NodeProblemCode::InvalidRequest, message)
        .with_message_key(key)
        .retryable(false)
        .for_network(network_name.into())
}

/// Revalidate the Coordinator-signed disclosure against both the join ticket
/// and the command fields. The preview is the local consent record, so none of
/// its security-relevant fields may be trusted merely because the CLI printed it.
fn validate_signed_ticket_join_preview(
    command: &super::NodeJoinNetworkCommand,
    preview: &NetworkJoinPreview,
    now_unix_ms: u64,
) -> Result<JoinTermsAck, NodeProblem> {
    if command.discovery_record.is_some() {
        return Err(signed_ticket_join_rejection(
            preview.network_name.clone(),
            "join.error.preview_conflict",
            "直接票据加入不能同时提交发现记录",
        ));
    }
    let ticket = NetworkTicket::parse(&command.target).map_err(|_| {
        signed_ticket_join_rejection(
            preview.network_name.clone(),
            "join.error.ticket_invalid",
            "直接预览加入必须使用有效的 fk:// 票据",
        )
    })?;
    if ticket.network_name != preview.network_name
        || command
            .network_name
            .as_deref()
            .is_some_and(|name| name != preview.network_name)
    {
        return Err(signed_ticket_join_rejection(
            preview.network_name.clone(),
            "join.error.name_changed",
            "加入票据、网络名称与已验签预览不一致",
        ));
    }
    let expected_network_id = NetworkId::from_public_key(preview.public_key.as_str().as_bytes());
    if preview.network_id != expected_network_id
        || NodeId::from_public_key(&ticket.pubkey) != preview.owner
    {
        return Err(signed_ticket_join_rejection(
            preview.network_name.clone(),
            "join.error.signed_preview_identity_mismatch",
            "已验签预览的网络身份或协调者身份与票据不一致",
        ));
    }
    if preview.terms.validate().is_err()
        || preview.policy_revision != preview.terms.network_policy_revision
        || preview.privacy_policy_revision != preview.terms.privacy_policy_revision
    {
        return Err(signed_ticket_join_rejection(
            preview.network_name.clone(),
            "join.error.signed_preview_invalid",
            "已验签预览的成员条款不一致",
        ));
    }
    let terms_hash = preview.terms.content_hash().map_err(|_| {
        signed_ticket_join_rejection(
            preview.network_name.clone(),
            "join.error.signed_preview_invalid",
            "无法计算已验签预览的成员条款哈希",
        )
    })?;
    let preview_digest = preview.compute_digest().map_err(|_| {
        signed_ticket_join_rejection(
            preview.network_name.clone(),
            "join.error.signed_preview_invalid",
            "无法计算已验签预览摘要",
        )
    })?;
    if terms_hash != preview.terms_hash || preview_digest != preview.preview_digest {
        return Err(signed_ticket_join_rejection(
            preview.network_name.clone(),
            "join.error.signed_preview_changed",
            "已验签预览条款或摘要已变化",
        ));
    }
    if preview.validate_freshness(now_unix_ms).is_err() {
        return Err(signed_ticket_join_rejection(
            preview.network_name.clone(),
            "join.error.signed_preview_expired",
            "加入预览已过期，请重新获取票据预览",
        ));
    }
    if !preview.signature_is_valid(&ticket.pubkey) {
        return Err(signed_ticket_join_rejection(
            preview.network_name.clone(),
            "join.error.signed_preview_signature_invalid",
            "加入预览签名无效",
        ));
    }
    match preview.join_mode {
        fuckipv6_common::domain::NetworkJoinMode::Open => {}
        fuckipv6_common::domain::NetworkJoinMode::ApprovalRequired => {
            return Err(signed_ticket_join_rejection(
                preview.network_name.clone(),
                "join.error.approval_required",
                "该网络需要管理员审批；fk:// 票据只用于定位网络，请提交加入申请",
            ));
        }
        fuckipv6_common::domain::NetworkJoinMode::InviteOnly => {
            return Err(signed_ticket_join_rejection(
                preview.network_name.clone(),
                "join.error.invite_only",
                "该网络仅接受正式邀请；fk:// 票据只用于定位网络，不能作为加入授权",
            ));
        }
    }
    if command
        .network_id
        .as_deref()
        .is_some_and(|network_id| network_id != preview.network_id.as_str())
        || command
            .preview_digest
            .as_deref()
            .is_some_and(|digest| digest != preview.preview_digest.as_str())
        || command
            .network_policy_revision
            .is_some_and(|revision| revision != preview.terms.network_policy_revision)
        || command
            .privacy_policy_revision
            .is_some_and(|revision| revision != preview.terms.privacy_policy_revision)
        || command
            .terms_hash
            .as_ref()
            .is_some_and(|hash| hash != &preview.terms_hash)
    {
        return Err(signed_ticket_join_rejection(
            preview.network_name.clone(),
            "join.error.signed_preview_changed",
            "加入命令字段与已验签预览不一致",
        ));
    }
    let latest_valid_acknowledgement = now_unix_ms.max(preview.issued_at_unix_ms);
    let acknowledged_at_unix_ms = command
        .acknowledged_at_unix_ms
        .unwrap_or(latest_valid_acknowledgement);
    if acknowledged_at_unix_ms < preview.issued_at_unix_ms
        || acknowledged_at_unix_ms > latest_valid_acknowledgement
    {
        return Err(signed_ticket_join_rejection(
            preview.network_name.clone(),
            "join.error.acknowledgement_time_invalid",
            "加入条款确认时间无效",
        ));
    }
    Ok(JoinTermsAck {
        terms_hash: preview.terms_hash.clone(),
        acknowledged_at_unix_ms,
    })
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
    let mut membership_acceptance = None;

    if let Some(preview) = command.signed_join_preview.clone() {
        let ack =
            match validate_signed_ticket_join_preview(command.as_ref(), &preview, unix_time_ms()) {
                Ok(ack) => ack,
                Err(problem) => return NodeCommandResult::rejected(request_id, problem),
            };
        command.network_name = Some(preview.network_name.clone());
        command.network_id = Some(preview.network_id.to_string());
        command.preview_digest = Some(preview.preview_digest.as_str().to_owned());
        command.network_policy_revision = Some(preview.terms.network_policy_revision);
        command.privacy_policy_revision = Some(preview.terms.privacy_policy_revision);
        command.terms_hash = Some(preview.terms_hash.clone());
        command.acknowledged_at_unix_ms = Some(ack.acknowledged_at_unix_ms);
        membership_acceptance = Some(JoinAcceptance::from_signed_preview(&preview, ack));
    } else if let Some(network_id) = command.network_id.as_deref() {
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
        let ack = JoinTermsAck {
            terms_hash: command
                .terms_hash
                .clone()
                .expect("validated join command has terms hash"),
            acknowledged_at_unix_ms: command
                .acknowledged_at_unix_ms
                .expect("validated join command has acknowledgement time"),
        };
        command.discovery_record = Some(record.clone());
        membership_acceptance = Some(JoinAcceptance::from_discovery_record(&record, ack));
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
    if let Some(record) = confirmed_record.as_ref() {
        if let Err(problem) = validate_exit_contribution_admission(
            &state.snapshot().await,
            record.document.exit_contribution,
            record.document.exit_contribution_scope,
            record.document.exit_contribution_mode,
            command.exit_contribution_acknowledged,
            &record.document.name,
        ) {
            return NodeCommandResult::rejected(request_id, problem);
        }
    } else if let Some(acceptance) = membership_acceptance.as_ref() {
        if let Err(problem) = validate_exit_contribution_admission(
            &state.snapshot().await,
            acceptance.terms.exit_contribution,
            acceptance.terms.exit_contribution_scope,
            acceptance.terms.exit_contribution_mode,
            command.exit_contribution_acknowledged,
            &acceptance.network_name,
        ) {
            return NodeCommandResult::rejected(request_id, problem);
        }
    }

    // Persist acceptance before creating configuration or starting a data
    // plane. On any later failure only the record inserted by this operation is
    // rolled back; memberships for other networks are never touched.
    let persisted = if let Some(acceptance) = membership_acceptance {
        let Some(configs) = configs else {
            return unavailable(request_id, "加入网络需要节点配置管理器");
        };
        match persist_joined_membership(configs, acceptance).await {
            Ok(membership) => Some(membership),
            Err(error) => {
                return NodeCommandResult::rejected(
                    request_id,
                    NodeProblem::new(
                        NodeProblemCode::Unavailable,
                        format!("保存本地成员关系失败: {error}"),
                    )
                    .with_param("error_kind", "storage")
                    .retryable(true),
                );
            }
        }
    } else {
        None
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
                .unwrap_or_else(|| {
                    network_membership_snapshot(
                        &joined,
                        snapshot
                            .networks
                            .iter()
                            .find(|network| network.name == joined.network_name)
                            .map(|network| network.status),
                    )
                });
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
    acceptance: JoinAcceptance,
) -> Result<(crate::config_manager::JoinedNetworkMembership, bool), String> {
    let configs = configs.clone();
    tokio::task::spawn_blocking(move || {
        let JoinAcceptance {
            network_name,
            network_id,
            terms,
            ack,
        } = acceptance;
        let now = unix_time_ms().max(ack.acknowledged_at_unix_ms);
        let identity = configs
            .load_node_identity()
            .map_err(|error| error.to_string())?;
        let node_id = NodeId::from_public_key(&identity.public_key_bytes());
        let mut membership = NetworkMembership::new(
            network_id,
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
            .upsert_joined_membership(&network_name, membership)
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

async fn refresh_embedded_services(state: &NodeStateStore, services: &EmbeddedServiceManager) {
    let runtime_services = match services.list().await {
        Ok(services) => services,
        Err(error) => {
            tracing::warn!(%error, "刷新内嵌服务状态失败，保留上一份快照");
            return;
        }
    };
    state
        .update_if_changed(|snapshot| {
            for service in &runtime_services {
                let projected = super::NodeEmbeddedServiceSnapshot::running_from_embedded(service);
                if let Some(existing) = snapshot
                    .embedded_services
                    .iter_mut()
                    .find(|existing| existing.network_name == projected.network_name)
                {
                    *existing = projected;
                } else {
                    snapshot.embedded_services.push(projected);
                }
            }
            snapshot
                .embedded_services
                .sort_by(|left, right| left.network_name.cmp(&right.network_name));
        })
        .await;
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

#[cfg(test)]
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

pub(crate) fn spawn_command_bus_with_runtime_and_audit(
    state: NodeStateStore,
    registry: MultiNetworkRegistry,
    events: super::NodeEventBus,
    services: EmbeddedServiceManager,
    runtime: NetworkRuntimeManager,
    configs: ConfigManager,
    audit: Arc<crate::owner_security::audit::OwnerSecurityAudit>,
) -> super::NodeCommandBus {
    let handler =
        NodeRuntimeHandler::with_runtime(state.clone(), registry, services, runtime, configs);
    super::NodeCommandBus::spawn_with_state_and_durable_audit(
        Arc::new(handler),
        events,
        32,
        state,
        audit,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{
        NodeClientBlacklistCommand, NodeCommand, NodeCommandBus, NodeCreateNetworkCommand,
        NodeEventBus, NodeEventKind, NodeEventReplay, NodeJoinNetworkCommand,
        NodeNetworkAllocation, NodeNetworkRole, NodeProbePeerCommand, NodeRoutingMode,
        NodeSetLocalExitCapabilityCommand, NodeSetNetworkRoleCommand,
        NodeUpdateNetworkAllocationCommand, NodeUpdateRoutingCommand,
    };
    use crate::policy_transaction::PolicyChangeAction;
    use crate::symmetric::EmbeddedServiceStartRequest;
    use fuckipv6_common::domain::{
        ContentHash, InvitationCode, InvitationId, NetworkAcl, NetworkAclGrant, NetworkId,
        NetworkRole, NodeId, NodeLocalAccessPolicy, NodeNetworkScope, PermissionCondition,
        PermissionEffect, PermissionIntent, PermissionRule, PolicyAction, PolicyTestInput,
        PolicyTestRequest, ResourceKind, SecurityRuleId, Signature, SubjectSelector,
    };
    use fuckipv6_common::{
        ExitNodeCapabilities, ExitNodePlatform, NetworkMode, EXIT_NODE_CAPABILITY_SCHEMA_VERSION,
    };
    use tokio::sync::mpsc;

    fn policy_test_input() -> PolicyTestInput {
        let network_id = NetworkId::from_public_key(b"policy-test-network");
        PolicyTestInput {
            network_id: network_id.clone(),
            actor: NodeId::from_public_key(b"policy-test-actor"),
            role: NetworkRole::Owner,
            groups: Default::default(),
            membership_active: true,
            recent_reauthentication: true,
            emergency_coordinator: false,
            request: PolicyTestRequest {
                action: PolicyAction::Read,
                resource: ResourceKind::Network,
                resource_network_id: network_id,
                object_id: None,
                intent: PermissionIntent::ReadOnly,
            },
            local_policy: None,
            network_acl: None,
            now_unix_ms: 1_000,
        }
    }

    #[test]
    fn invitation_binds_legacy_network_config_without_network_id() {
        let invitation_network_id = NetworkId::from_public_key(b"invitation-network");

        assert!(!configured_network_conflicts_with_invitation(
            None,
            &invitation_network_id,
        ));
        assert!(!configured_network_conflicts_with_invitation(
            Some(invitation_network_id.as_str()),
            &invitation_network_id,
        ));
        assert!(configured_network_conflicts_with_invitation(
            Some("net_different"),
            &invitation_network_id,
        ));
    }

    fn policy_transaction_network_id() -> NetworkId {
        NetworkId::from_public_key(b"runtime-policy-network")
    }

    fn policy_transaction_rule(
        name: &str,
        effect: PermissionEffect,
        action: PolicyAction,
        resource: ResourceKind,
        conditions: Vec<PermissionCondition>,
    ) -> PermissionRule {
        PermissionRule {
            rule_id: SecurityRuleId::from_public_key(name.as_bytes()),
            effect,
            subjects: vec![SubjectSelector::AnyMember],
            resources: vec![resource],
            actions: vec![action],
            conditions,
        }
    }

    fn policy_transaction_snapshot() -> PolicySnapshot {
        static NOW: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        let now = *NOW.get_or_init(unix_time_ms);
        let network_id = policy_transaction_network_id();
        PolicySnapshot {
            local_policy: Some(NodeLocalAccessPolicy {
                node_id: NodeId::from_public_key(b"runtime-policy-owner"),
                revision: 1,
                issued_at_unix_ms: now.saturating_sub(60_000),
                expires_at_unix_ms: now + 86_400_000,
                network_scope: NodeNetworkScope::Only(vec![network_id.clone()]),
                contribution_vetoes: Vec::new(),
                exit_contribution: None,
                relay_limits: Default::default(),
                rules: vec![policy_transaction_rule(
                    "runtime-local-read",
                    PermissionEffect::Allow,
                    PolicyAction::Read,
                    ResourceKind::Network,
                    vec![PermissionCondition::RecentReauthentication],
                )],
            }),
            network_acls: vec![NetworkAcl {
                network_id,
                revision: 1,
                issued_at_unix_ms: now.saturating_sub(60_000),
                expires_at_unix_ms: now + 86_400_000,
                rules: vec![policy_transaction_rule(
                    "runtime-acl-read",
                    PermissionEffect::Allow,
                    PolicyAction::Read,
                    ResourceKind::Network,
                    Vec::new(),
                )],
                grants: Vec::new(),
                relay_limits: Default::default(),
            }],
        }
    }

    fn policy_transaction_candidate(document_revision: u64) -> PolicySnapshot {
        let mut candidate = policy_transaction_snapshot();
        let local = candidate.local_policy.as_mut().expect("local policy");
        local.revision = document_revision;
        local.rules = vec![policy_transaction_rule(
            "runtime-local-expanded",
            PermissionEffect::Allow,
            PolicyAction::Update,
            ResourceKind::SecurityPolicy,
            Vec::new(),
        )];
        let acl = candidate.network_acls.get_mut(0).expect("network acl");
        acl.revision = document_revision;
        acl.rules = vec![policy_transaction_rule(
            "runtime-acl-expanded",
            PermissionEffect::Allow,
            PolicyAction::Update,
            ResourceKind::SecurityPolicy,
            Vec::new(),
        )];
        acl.grants.push(NetworkAclGrant {
            grant_id: SecurityRuleId::from_public_key(b"runtime-policy-grant"),
            subjects: vec![SubjectSelector::AnyMember],
            resources: vec![ResourceKind::SecurityPolicy],
            actions: vec![PolicyAction::Update],
            issued_at_unix_ms: acl.issued_at_unix_ms,
            expires_at_unix_ms: acl.expires_at_unix_ms,
            revoked_at_unix_ms: None,
            revoked_by: None,
            requires_recent_reauthentication: true,
        });
        candidate
    }

    async fn policy_runtime_bus(
        name: &str,
    ) -> (
        NodeCommandBus,
        NodeStateStore,
        NodeEventBus,
        PolicyTransactionStore,
        std::path::PathBuf,
    ) {
        let dir = std::env::temp_dir().join(format!(
            "fuckipv6-runtime-policy-{name}-{}-{}",
            std::process::id(),
            unix_time_ms()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let configs = ConfigManager::new_for_dir(dir.clone()).expect("config manager");
        let events = NodeEventBus::default();
        let state = NodeStateStore::with_events(events.clone());
        let policy = policy_transaction_snapshot();
        state
            .update(|snapshot| {
                snapshot.access_control.local_policy = policy.local_policy;
                snapshot.access_control.network_acls = policy.network_acls;
            })
            .await;
        let mut handler = NodeRuntimeHandler::with_services(
            state.clone(),
            MultiNetworkRegistry::new(),
            EmbeddedServiceManager::spawn_with_config(4, configs),
        );
        handler.policy_store = Some(PolicyTransactionStore::new(dir.clone()));
        let bus =
            NodeCommandBus::spawn_with_state(Arc::new(handler), events.clone(), 32, state.clone());
        (
            bus,
            state,
            events,
            PolicyTransactionStore::new(dir.clone()),
            dir,
        )
    }

    fn invitation_preview() -> InvitationPreview {
        let terms = MembershipTerms {
            network_policy_revision: 7,
            privacy_policy_revision: 3,
            exit_contribution: Default::default(),
            exit_contribution_scope: Default::default(),
            exit_contribution_mode: Default::default(),
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
            exit_contribution_acknowledged: Some(true),
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
            exit_contribution_scope: fuckipv6_common::domain::ExitContributionScope::JoiningMembers,
            exit_contribution_mode: fuckipv6_common::domain::ExitContributionMode::RequiredOnJoin,
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
    async fn unchanged_embedded_service_refresh_does_not_create_state_events() {
        let events = NodeEventBus::new(8);
        let mut event_receiver = events.subscribe();
        let state = NodeStateStore::with_events(events);
        let services = EmbeddedServiceManager::spawn(1);
        let before = state.snapshot().await;

        refresh_embedded_services(&state, &services).await;

        let after = state.snapshot().await;
        assert_eq!(after, before);
        assert!(event_receiver.try_recv().is_err());
        services.shutdown().await.expect("shutdown services");
    }

    #[tokio::test]
    async fn get_snapshot_refreshes_authoritative_relay_health_from_the_service_owner() {
        let config_dir = std::env::temp_dir().join(format!(
            "fuckipv6-runtime-handler-relay-health-{}-{}",
            std::process::id(),
            unix_time_ms()
        ));
        let _ = std::fs::remove_dir_all(&config_dir);
        let configs = crate::config_manager::ConfigManager::new_for_dir(config_dir.clone())
            .expect("config manager");
        let services = EmbeddedServiceManager::spawn_with_config(2, configs);
        services
            .start(
                EmbeddedServiceStartRequest::new("live-relay", 0, 0)
                    .with_external_host("127.0.0.1"),
            )
            .await
            .expect("start embedded service");
        let bus = spawn_command_bus_with_services(
            NodeStateStore::new(),
            MultiNetworkRegistry::new(),
            NodeEventBus::default(),
            services.clone(),
        );

        let result = bus
            .execute_test(NodeCommandRequest::new(26, NodeCommand::GetSnapshot))
            .await
            .expect("snapshot result");
        let NodeCommandOutcome::Snapshot(snapshot) = result.outcome else {
            panic!("unexpected snapshot outcome");
        };
        let service = snapshot
            .embedded_services
            .iter()
            .find(|service| service.network_name == "live-relay")
            .expect("embedded service projected into node state");
        let health = service
            .relay_health
            .as_ref()
            .expect("authoritative relay health");
        assert!(health.listener_bound);
        assert_ne!(
            health.status,
            fuckipv6_common::domain::RelayHealthStatus::Healthy,
            "a bound listener must not become healthy without forwarding evidence"
        );

        services.shutdown().await.expect("shutdown services");
        let _ = std::fs::remove_dir_all(config_dir);
    }

    #[tokio::test]
    async fn get_snapshot_projects_persisted_membership_before_first_raw_read() {
        use crate::config_manager::ConfigManager;
        use fuckipv6_common::domain::{
            ExitContributionMode, ExitContributionObligation, ExitContributionScope, JoinTermsAck,
            MembershipRole, MembershipState, MembershipTerms,
        };

        let config_dir = std::env::temp_dir().join(format!(
            "fuckipv6-membership-startup-projection-{}-{}",
            std::process::id(),
            unix_time_ms()
        ));
        let _ = std::fs::remove_dir_all(&config_dir);
        let configs = ConfigManager::new_for_dir(config_dir.clone()).expect("config manager");
        let now = unix_time_ms();
        let network_id = NetworkId::from_public_key(b"persisted-membership-network");
        let node_id = NodeId::from_public_key(b"persisted-membership-node");
        let terms = MembershipTerms {
            network_policy_revision: 4,
            privacy_policy_revision: 2,
            exit_contribution: ExitContributionObligation::Required,
            exit_contribution_scope: ExitContributionScope::AllMembers,
            exit_contribution_mode: ExitContributionMode::RequiredOnJoin,
            member_limit: Some(16),
        };
        let mut membership = fuckipv6_common::domain::NetworkMembership::new(
            network_id,
            node_id,
            MembershipRole::Member,
            now.saturating_sub(10),
        );
        membership
            .begin_apply(
                terms.clone(),
                JoinTermsAck {
                    terms_hash: terms.content_hash().expect("terms hash"),
                    acknowledged_at_unix_ms: now.saturating_sub(10),
                },
                now.saturating_sub(10),
            )
            .expect("begin apply");
        membership.accept(now.saturating_sub(9)).expect("accept");
        configs
            .upsert_joined_membership("persisted-membership", membership)
            .expect("persist membership");

        let events = NodeEventBus::default();
        let state = NodeStateStore::with_events(events.clone());
        let mut handler = NodeRuntimeHandler::with_services(
            state.clone(),
            MultiNetworkRegistry::new(),
            EmbeddedServiceManager::spawn_with_config(2, configs.clone()),
        );
        handler.configs = Some(configs.clone());
        let bus = NodeCommandBus::spawn_with_state(Arc::new(handler), events, 32, state);

        let result = bus
            .execute_test(NodeCommandRequest::new(1, NodeCommand::GetSnapshot))
            .await
            .expect("snapshot result");
        let NodeCommandOutcome::Snapshot(snapshot) = result.outcome else {
            panic!("unexpected snapshot outcome");
        };
        let projected = snapshot
            .memberships
            .iter()
            .find(|membership| membership.network_name == "persisted-membership")
            .expect("first raw snapshot must include persisted membership");
        assert_eq!(projected.state, MembershipState::Accepted);
        assert_eq!(projected.role, MembershipRole::Member);
        assert_eq!(projected.current_terms.as_ref(), Some(&terms));

        let _ = std::fs::remove_dir_all(config_dir);
    }

    #[tokio::test]
    async fn refresh_uses_local_unlisted_network_record_for_membership_terms() {
        use crate::config_manager::ConfigManager;
        use fuckipv6_common::domain::{
            ExitContributionMode, ExitContributionObligation, ExitContributionScope,
            NetworkCreateProfile, NetworkJoinMode, NetworkProfileUpdate, NetworkVisibility,
        };

        let config_dir = std::env::temp_dir().join(format!(
            "fuckipv6-local-membership-terms-{}-{}",
            std::process::id(),
            unix_time_ms()
        ));
        let _ = std::fs::remove_dir_all(&config_dir);
        let configs = ConfigManager::new_for_dir(config_dir.clone()).expect("config manager");
        let created = configs
            .create_owned_network(
                NetworkCreateProfile {
                    name: "w12e9-local".into(),
                    purpose: "Local membership refresh test".into(),
                    icon: "test".into(),
                    language: "zh-CN".into(),
                    region: "HK".into(),
                    tags: Vec::new(),
                    capability_declarations: Vec::new(),
                    visibility: NetworkVisibility::Unlisted,
                    join_mode: NetworkJoinMode::InviteOnly,
                    exit_contribution: ExitContributionObligation::NotRequired,
                    exit_contribution_scope: ExitContributionScope::NotApplicable,
                    exit_contribution_mode: ExitContributionMode::Disabled,
                    exit_selection_policy: Default::default(),
                },
                "ws://127.0.0.1:6666",
                true,
            )
            .expect("create owned network");
        configs
            .persist_owned_network_membership(&created.record)
            .expect("persist owner membership");

        let mut update = NetworkProfileUpdate::from_record(&created.record);
        update.exit_contribution_mode = ExitContributionMode::Autonomous;
        configs
            .update_network_profile(
                "w12e9-local",
                created.record.revision,
                update,
                created.record.owner.as_str(),
                "membership-refresh",
                None,
            )
            .expect("update network policy");
        let current_record = configs
            .load_network_record("w12e9-local")
            .expect("load updated network record");
        assert_eq!(current_record.policy_revision, 2);
        assert_eq!(
            current_record.exit_contribution_mode,
            ExitContributionMode::Autonomous
        );

        let state = NodeStateStore::new();
        assert!(state.snapshot().await.discovery_index.records.is_empty());
        refresh_network_memberships(&state, &configs)
            .await
            .expect("refresh local memberships");

        let snapshot = state.snapshot().await;
        let membership = snapshot
            .memberships
            .iter()
            .find(|membership| membership.network_name == "w12e9-local")
            .expect("local membership projection");
        assert_eq!(
            membership.current_terms.as_ref(),
            Some(&current_record.membership_terms())
        );
        assert_eq!(
            membership
                .current_terms
                .as_ref()
                .unwrap()
                .network_policy_revision,
            2
        );
        assert_eq!(
            membership
                .current_terms
                .as_ref()
                .unwrap()
                .exit_contribution_mode,
            ExitContributionMode::Autonomous
        );
        assert!(membership.policy_confirmation_required);

        let _ = std::fs::remove_dir_all(config_dir);
    }

    #[tokio::test]
    async fn get_snapshot_projects_persisted_local_exit_owner_intent_without_runtime_actor() {
        let config_dir = std::env::temp_dir().join(format!(
            "fuckipv6-local-exit-owner-projection-{}-{}",
            std::process::id(),
            unix_time_ms()
        ));
        let _ = std::fs::remove_dir_all(&config_dir);
        let configs = ConfigManager::new_for_dir(config_dir.clone()).expect("config manager");
        configs
            .set_node_local_exit_enabled(true)
            .expect("persist owner intent");
        let events = NodeEventBus::default();
        let state = NodeStateStore::with_events(events.clone());
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
                };
            })
            .await;
        let mut handler = NodeRuntimeHandler::with_services(
            state.clone(),
            MultiNetworkRegistry::new(),
            EmbeddedServiceManager::spawn_with_config(2, configs.clone()),
        );
        handler.configs = Some(configs.clone());
        let bus = NodeCommandBus::spawn_with_state(Arc::new(handler), events, 32, state);

        let result = bus
            .execute_test(NodeCommandRequest::new(99, NodeCommand::GetSnapshot))
            .await
            .expect("snapshot result");
        let NodeCommandOutcome::Snapshot(snapshot) = result.outcome else {
            panic!("unexpected snapshot outcome");
        };
        assert!(snapshot.local_exit.owner_exit_enabled);
        assert_eq!(
            snapshot.local_exit.stage,
            crate::node::LocalExitCapabilityStage::WaitingForNetworkAuthorization
        );
        assert!(!snapshot.local_exit.network_allowed);
        assert!(snapshot.local_exit.problem.is_none());

        configs
            .set_node_local_exit_enabled(false)
            .expect("persist disabled owner intent");
        let result = bus
            .execute_test(NodeCommandRequest::new(100, NodeCommand::GetSnapshot))
            .await
            .expect("snapshot result");
        let NodeCommandOutcome::Snapshot(snapshot) = result.outcome else {
            panic!("unexpected snapshot outcome");
        };
        assert!(!snapshot.local_exit.owner_exit_enabled);
        assert_eq!(
            snapshot.local_exit.stage,
            crate::node::LocalExitCapabilityStage::Disabled
        );
        assert!(snapshot.local_exit.problem.is_none());

        let _ = std::fs::remove_dir_all(config_dir);
    }

    #[test]
    fn persisted_disable_does_not_hide_local_exit_cleanup_state() {
        let capabilities = ExitNodeCapabilities::for_platform(ExitNodePlatform::Linux);
        let problem =
            NodeProblem::new(NodeProblemCode::Unavailable, "cleanup failed").retryable(true);

        for (stage, expected_problem) in [
            (crate::node::LocalExitCapabilityStage::Draining, false),
            (crate::node::LocalExitCapabilityStage::Cleaning, false),
            (crate::node::LocalExitCapabilityStage::Failed, true),
        ] {
            let current = crate::node::LocalExitCapabilitySnapshot {
                owner_exit_enabled: false,
                stage,
                healthy: true,
                network_allowed: true,
                active_clients: 2,
                total_clients: 7,
                problem: expected_problem.then(|| problem.clone()),
                last_transition_at_unix_ms: 42,
            };
            let projected = project_local_exit_owner_intent(&current, false, &capabilities);
            assert_eq!(projected.stage, stage);
            assert!(!projected.owner_exit_enabled);
            assert!(!projected.healthy);
            assert!(projected.network_allowed);
            assert_eq!(projected.active_clients, 2);
            assert_eq!(projected.total_clients, 7);
            assert_eq!(projected.problem.is_some(), expected_problem);
            assert_eq!(projected.last_transition_at_unix_ms, 42);
        }
    }

    #[tokio::test]
    async fn local_exit_enable_rejects_unsupported_and_privileged_hosts_before_runtime() {
        let cases = [
            (
                ExitNodeCapabilities::for_platform(ExitNodePlatform::Windows),
                NodeProblemCode::Unsupported,
            ),
            (
                ExitNodeCapabilities {
                    supported: false,
                    unsupported_reasons: vec![
                        fuckipv6_common::ExitNodeUnsupportedReason::PrivilegesRequired,
                    ],
                    ..ExitNodeCapabilities::for_platform(ExitNodePlatform::Linux)
                },
                NodeProblemCode::PermissionDenied,
            ),
        ];
        for (index, (capabilities, expected_code)) in cases.into_iter().enumerate() {
            let state = NodeStateStore::new();
            state
                .update(|snapshot| snapshot.exit_node_capabilities = capabilities)
                .await;
            let bus =
                spawn_command_bus(state, MultiNetworkRegistry::new(), NodeEventBus::default());
            let result = bus
                .execute_test(NodeCommandRequest::new(
                    101 + index as u64,
                    NodeCommand::SetLocalExitCapability(NodeSetLocalExitCapabilityCommand {
                        enabled: true,
                    }),
                ))
                .await
                .expect("local exit rejection");
            let NodeCommandOutcome::Rejected(problem) = result.outcome else {
                panic!("unsupported local exit must be rejected");
            };
            assert_problem(&problem, expected_code, false);
        }
    }

    #[tokio::test]
    async fn local_exit_disable_is_not_blocked_by_host_capability_checks() {
        let state = NodeStateStore::new();
        state
            .update(|snapshot| {
                snapshot.exit_node_capabilities =
                    ExitNodeCapabilities::for_platform(ExitNodePlatform::Windows);
            })
            .await;
        let bus = spawn_command_bus(state, MultiNetworkRegistry::new(), NodeEventBus::default());
        let result = bus
            .execute_test(NodeCommandRequest::new(
                103,
                NodeCommand::SetLocalExitCapability(NodeSetLocalExitCapabilityCommand {
                    enabled: false,
                }),
            ))
            .await
            .expect("disable command reaches the runtime boundary");
        let NodeCommandOutcome::Rejected(problem) = result.outcome else {
            panic!("missing runtime must be rejected after capability checks");
        };
        assert_problem(&problem, NodeProblemCode::Unavailable, true);
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
                assert_eq!(manifest.pages.len(), 13);
                assert_eq!(*manifest, crate::node::node_ui_manifest());
            }
            outcome => panic!("unexpected outcome: {outcome:?}"),
        }
    }

    #[tokio::test]
    async fn policy_tests_use_the_runtime_evaluator_without_audit_or_state_side_effects() {
        let state = NodeStateStore::new();
        state
            .update(|snapshot| {
                snapshot.logs.push(crate::node::NodeLogEntry {
                    unix_ms: 1,
                    level: crate::node::NodeLogLevel::Info,
                    target: "policy-test".into(),
                    message: "unchanged".into(),
                });
            })
            .await;
        let bus = spawn_command_bus(
            state.clone(),
            MultiNetworkRegistry::new(),
            NodeEventBus::default(),
        );
        let input = policy_test_input();
        let expected = input.evaluate();
        let before = state.snapshot().await;
        let audit_before = bus.audit_entries().len();

        let result = bus
            .execute_test(NodeCommandRequest::new(76, NodeCommand::TestPolicy(input)))
            .await
            .expect("policy test result");
        match result.outcome {
            NodeCommandOutcome::PolicyTest(result) => {
                assert_eq!(*result, expected);
                assert!(result.decision.allowed);
            }
            outcome => panic!("unexpected outcome: {outcome:?}"),
        }

        assert_eq!(state.snapshot().await, before);
        assert_eq!(bus.audit_entries().len(), audit_before);
    }

    #[tokio::test]
    async fn invalid_candidate_policy_returns_a_successful_explained_test_result() {
        let bus = spawn_command_bus(
            NodeStateStore::new(),
            MultiNetworkRegistry::new(),
            NodeEventBus::default(),
        );
        let mut input = policy_test_input();
        input.network_acl = Some(NetworkAcl {
            network_id: input.network_id.clone(),
            revision: 1,
            issued_at_unix_ms: 100,
            expires_at_unix_ms: 10_000,
            rules: vec![PermissionRule {
                rule_id: fuckipv6_common::domain::SecurityRuleId::from_public_key(b"invalid-rule"),
                effect: PermissionEffect::Deny,
                subjects: vec![SubjectSelector::AnyMember],
                resources: vec![ResourceKind::Network],
                actions: vec![PolicyAction::Read],
                conditions: Vec::new(),
            }],
            grants: Vec::new(),
            relay_limits: Default::default(),
        });
        input.network_acl.as_mut().expect("acl").rules[0]
            .subjects
            .clear();

        let result = bus
            .execute_test(NodeCommandRequest::new(77, NodeCommand::TestPolicy(input)))
            .await
            .expect("invalid candidate still returns a test result");
        match result.outcome {
            NodeCommandOutcome::PolicyTest(result) => {
                assert!(result.validation_issues.iter().any(|issue| {
                    issue.path == "/network_acl/rules/0/subjects"
                        && issue.code == "empty_rule_subjects"
                }));
                assert!(!result.decision.allowed);
            }
            outcome => panic!("unexpected outcome: {outcome:?}"),
        }
    }

    #[tokio::test]
    async fn policy_preview_apply_history_and_rollback_use_one_authoritative_revision_stream() {
        let (bus, state, _events, store, dir) = policy_runtime_bus("workflow").await;
        let base = policy_transaction_snapshot();
        let first_candidate = policy_transaction_candidate(2);
        let network_ids = vec![policy_transaction_network_id().to_string()];

        let preview = bus
            .execute_test(NodeCommandRequest::new(
                80,
                NodeCommand::PreviewPolicy(super::super::NodePreviewPolicyCommand {
                    expected_revision: 0,
                    candidate: first_candidate.clone(),
                }),
            ))
            .await
            .expect("preview result");
        let preview = match preview.outcome {
            NodeCommandOutcome::PolicyPreview(preview) => *preview,
            outcome => panic!("unexpected preview outcome: {outcome:?}"),
        };
        assert!(!preview.revision_conflict);
        assert!(preview.has_changes);
        assert_eq!(preview.network_ids, network_ids);
        assert!(preview.includes_local_policy);
        assert!(preview.risk.requires_explicit_confirmation);

        let applied = bus
            .execute_test(NodeCommandRequest::new(
                81,
                NodeCommand::ApplyPolicy(super::super::NodeApplyPolicyCommand {
                    expected_revision: preview.expected_revision,
                    candidate: first_candidate.clone(),
                    network_ids: preview.network_ids.clone(),
                    includes_local_policy: preview.includes_local_policy,
                    confirm_privilege_expansion: true,
                }),
            ))
            .await
            .expect("apply result");
        let applied = match applied.outcome {
            NodeCommandOutcome::PolicyApplied(applied) => *applied,
            outcome => panic!("unexpected apply outcome: {outcome:?}"),
        };
        assert_eq!(applied.previous_revision, 0);
        assert_eq!(applied.revision, 1);
        assert!(!applied.no_op);
        assert_eq!(applied.snapshot, first_candidate);
        assert_eq!(applied.audit.as_ref().expect("apply audit").revision, 1);

        let current = bus
            .execute_test(NodeCommandRequest::new(82, NodeCommand::GetPolicy))
            .await
            .expect("get policy result");
        match current.outcome {
            NodeCommandOutcome::Policy(current) => {
                assert_eq!(current.revision, 1);
                assert_eq!(current.snapshot, first_candidate);
            }
            outcome => panic!("unexpected get policy outcome: {outcome:?}"),
        }
        assert_eq!(
            state.snapshot().await.access_control.local_policy,
            first_candidate.local_policy
        );

        let second_candidate = policy_transaction_candidate(3);
        let second_preview = bus
            .execute_test(NodeCommandRequest::new(
                83,
                NodeCommand::PreviewPolicy(super::super::NodePreviewPolicyCommand {
                    expected_revision: 1,
                    candidate: second_candidate.clone(),
                }),
            ))
            .await
            .expect("second preview result");
        let second_preview = match second_preview.outcome {
            NodeCommandOutcome::PolicyPreview(preview) => *preview,
            outcome => panic!("unexpected second preview outcome: {outcome:?}"),
        };
        let second = bus
            .execute_test(NodeCommandRequest::new(
                84,
                NodeCommand::ApplyPolicy(super::super::NodeApplyPolicyCommand {
                    expected_revision: 1,
                    candidate: second_candidate.clone(),
                    network_ids: second_preview.network_ids,
                    includes_local_policy: second_preview.includes_local_policy,
                    confirm_privilege_expansion: true,
                }),
            ))
            .await
            .expect("second apply result");
        assert!(matches!(
            second.outcome,
            NodeCommandOutcome::PolicyApplied(result) if result.revision == 2
        ));

        let history = bus
            .execute_test(NodeCommandRequest::new(85, NodeCommand::GetPolicyHistory))
            .await
            .expect("policy history result");
        match history.outcome {
            NodeCommandOutcome::PolicyHistory(history) => {
                assert_eq!(history.current_revision, 2);
                assert_eq!(history.entries.len(), 2);
                assert_eq!(history.entries[0].revision, 2);
                assert_eq!(history.entries[0].action, PolicyChangeAction::Apply);
            }
            outcome => panic!("unexpected history outcome: {outcome:?}"),
        }

        let rollback_preview = bus
            .execute_test(NodeCommandRequest::new(
                86,
                NodeCommand::PreviewPolicyRollback(
                    super::super::NodePreviewPolicyRollbackCommand {
                        expected_revision: 2,
                        target_revision: 1,
                    },
                ),
            ))
            .await
            .expect("rollback preview result");
        let rollback_preview = match rollback_preview.outcome {
            NodeCommandOutcome::PolicyPreview(preview) => *preview,
            outcome => panic!("unexpected rollback preview outcome: {outcome:?}"),
        };
        assert_eq!(rollback_preview.rollback_target_revision, Some(1));
        assert!(rollback_preview.has_changes);

        let rolled_back = bus
            .execute_test(NodeCommandRequest::new(
                87,
                NodeCommand::RollbackPolicy(super::super::NodeRollbackPolicyCommand {
                    expected_revision: 2,
                    target_revision: 1,
                    network_ids: rollback_preview.network_ids,
                    includes_local_policy: rollback_preview.includes_local_policy,
                    confirm_privilege_expansion: true,
                }),
            ))
            .await
            .expect("rollback result");
        match rolled_back.outcome {
            NodeCommandOutcome::PolicyRolledBack(result) => {
                assert_eq!(result.previous_revision, 2);
                assert_eq!(result.revision, 3);
                assert_eq!(result.target_revision, 1);
                assert_eq!(result.audit.action, PolicyChangeAction::Rollback);
                assert_eq!(result.audit.target_revision, Some(1));
                assert_eq!(result.revision, result.audit.revision);
            }
            outcome => panic!("unexpected rollback outcome: {outcome:?}"),
        }

        assert_eq!(
            store
                .current_snapshot(&base)
                .expect("store current")
                .local_policy,
            state.snapshot().await.access_control.local_policy
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn stale_policy_apply_returns_conflict_without_overwriting_the_winner() {
        let (bus, state, _events, store, dir) = policy_runtime_bus("conflict").await;
        let base = policy_transaction_snapshot();
        let candidate = policy_transaction_candidate(2);
        let network_ids = vec![policy_transaction_network_id().to_string()];
        let winner = store
            .apply(
                &base,
                0,
                candidate.clone(),
                &network_ids,
                true,
                true,
                "other-owner",
                "test",
            )
            .expect("external winner");
        assert_eq!(winner.revision, 1);

        let problem = execute_rejected(
            &bus,
            88,
            NodeCommand::ApplyPolicy(super::super::NodeApplyPolicyCommand {
                expected_revision: 0,
                candidate,
                network_ids,
                includes_local_policy: true,
                confirm_privilege_expansion: true,
            }),
        )
        .await;
        assert_problem(&problem, NodeProblemCode::Conflict, false);
        assert_eq!(problem.message_key, "policy.error.revision_conflict");
        assert_eq!(
            problem.params.get("expected_revision").map(String::as_str),
            Some("0")
        );
        assert_eq!(
            problem.params.get("current_revision").map(String::as_str),
            Some("1")
        );
        let current = store
            .current_snapshot(&base)
            .expect("current after conflict");
        assert_eq!(current, winner.snapshot);
        assert_eq!(
            state.snapshot().await.access_control.local_policy,
            base.local_policy
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn no_op_policy_apply_does_not_publish_a_policy_change_or_history_entry() {
        let (bus, state, _events, store, dir) = policy_runtime_bus("no-op").await;
        let base = policy_transaction_snapshot();
        let mut updates = state.subscribe();
        let result = bus
            .execute_test(NodeCommandRequest::new(
                89,
                NodeCommand::ApplyPolicy(super::super::NodeApplyPolicyCommand {
                    expected_revision: 0,
                    candidate: base.clone(),
                    network_ids: Vec::new(),
                    includes_local_policy: false,
                    confirm_privilege_expansion: false,
                }),
            ))
            .await
            .expect("no-op apply result");
        match result.outcome {
            NodeCommandOutcome::PolicyApplied(result) => {
                assert!(result.no_op);
                assert_eq!(result.previous_revision, 0);
                assert_eq!(result.revision, 0);
                assert!(result.audit.is_none());
            }
            outcome => panic!("unexpected no-op outcome: {outcome:?}"),
        }
        while updates.has_changed().expect("watch remains open") {
            let snapshot = updates.borrow_and_update().clone();
            assert_eq!(snapshot.access_control.local_policy, base.local_policy);
            assert_eq!(snapshot.access_control.network_acls, base.network_acls);
        }
        assert_eq!(store.current_snapshot(&base).expect("store current"), base);
        assert!(store.history(&base).expect("history").is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn successful_policy_apply_and_rollback_publish_authoritative_state_changes() {
        let (bus, state, _events, _store, dir) = policy_runtime_bus("state-events").await;
        let first = policy_transaction_candidate(2);
        let network_ids = vec![policy_transaction_network_id().to_string()];
        let mut updates = state.subscribe();
        let applied = bus
            .execute_test(NodeCommandRequest::new(
                90,
                NodeCommand::ApplyPolicy(super::super::NodeApplyPolicyCommand {
                    expected_revision: 0,
                    candidate: first.clone(),
                    network_ids: network_ids.clone(),
                    includes_local_policy: true,
                    confirm_privilege_expansion: true,
                }),
            ))
            .await
            .expect("apply result");
        assert!(matches!(
            applied.outcome,
            NodeCommandOutcome::PolicyApplied(result) if !result.no_op
        ));
        let mut apply_published = false;
        while updates.has_changed().expect("watch remains open") {
            let snapshot = updates.borrow_and_update().clone();
            apply_published |= snapshot.access_control.local_policy == first.local_policy
                && snapshot.access_control.network_acls == first.network_acls;
        }
        assert!(apply_published);

        let rolled_back = bus
            .execute_test(NodeCommandRequest::new(
                91,
                NodeCommand::RollbackPolicy(super::super::NodeRollbackPolicyCommand {
                    expected_revision: 1,
                    target_revision: 1,
                    network_ids: network_ids.clone(),
                    includes_local_policy: true,
                    confirm_privilege_expansion: true,
                }),
            ))
            .await
            .expect("rollback result");
        assert!(matches!(
            rolled_back.outcome,
            NodeCommandOutcome::PolicyRolledBack(result) if result.revision == 2
        ));
        let mut rollback_published = false;
        while updates.has_changed().expect("watch remains open") {
            let snapshot = updates.borrow_and_update().clone();
            let local = snapshot
                .access_control
                .local_policy
                .as_ref()
                .expect("local policy");
            rollback_published |= local.revision == 3;
        }
        assert!(rollback_published);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn policy_apply_and_rollback_resume_after_disconnect_without_losing_revisions() {
        let (bus, state, events, store, dir) = policy_runtime_bus("disconnect").await;
        let base = policy_transaction_snapshot();
        let network_ids = vec![policy_transaction_network_id().to_string()];

        let (disconnected_receiver, acknowledged) = events.subscribe_with_cursor();
        drop(disconnected_receiver);

        let applied = bus
            .execute_test(NodeCommandRequest::new(
                90,
                NodeCommand::ApplyPolicy(super::super::NodeApplyPolicyCommand {
                    expected_revision: 0,
                    candidate: policy_transaction_candidate(2),
                    network_ids: network_ids.clone(),
                    includes_local_policy: true,
                    confirm_privilege_expansion: true,
                }),
            ))
            .await
            .expect("apply result");
        assert!(matches!(
            applied.outcome,
            NodeCommandOutcome::PolicyApplied(result) if result.revision == 1
        ));

        let rolled_back = bus
            .execute_test(NodeCommandRequest::new(
                91,
                NodeCommand::RollbackPolicy(super::super::NodeRollbackPolicyCommand {
                    expected_revision: 1,
                    target_revision: 1,
                    network_ids,
                    includes_local_policy: true,
                    confirm_privilege_expansion: true,
                }),
            ))
            .await
            .expect("rollback result");
        assert!(matches!(
            rolled_back.outcome,
            NodeCommandOutcome::PolicyRolledBack(result) if result.revision == 2
        ));

        let (_reconnected_receiver, boundary) = events.subscribe_with_cursor();
        let NodeEventReplay::Events(replayed) = events.replay_after(acknowledged, boundary) else {
            panic!("retained policy events must replay after reconnect");
        };
        assert!(!replayed.is_empty());
        assert_eq!(
            replayed.first().map(|event| event.sequence),
            Some(acknowledged.sequence + 1)
        );
        assert!(replayed
            .windows(2)
            .all(|pair| pair[0].sequence + 1 == pair[1].sequence));

        let mut published_revisions = Vec::new();
        for event in replayed {
            if let NodeEventKind::StateChanged { revision } = event.kind {
                assert!(published_revisions
                    .last()
                    .is_none_or(|previous| previous < &revision));
                published_revisions.push(revision);
            }
        }
        let current = state.snapshot().await;
        assert_eq!(published_revisions.last().copied(), Some(current.revision));
        assert_eq!(
            store.current_snapshot(&base).expect("store current"),
            PolicySnapshot {
                local_policy: current.access_control.local_policy,
                network_acls: current.access_control.network_acls,
            }
        );
        assert_eq!(
            store
                .history_snapshot(&base)
                .expect("store history")
                .current_revision,
            2
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn concurrent_policy_apply_and_rollback_cannot_move_the_revision_backwards() {
        let (bus, _state, _events, store, dir) = policy_runtime_bus("concurrent").await;
        let base = policy_transaction_snapshot();
        let first = policy_transaction_candidate(2);
        let network_ids = vec![policy_transaction_network_id().to_string()];
        let applied = bus
            .execute_test(NodeCommandRequest::new(
                92,
                NodeCommand::ApplyPolicy(super::super::NodeApplyPolicyCommand {
                    expected_revision: 0,
                    candidate: first,
                    network_ids: network_ids.clone(),
                    includes_local_policy: true,
                    confirm_privilege_expansion: true,
                }),
            ))
            .await
            .expect("initial apply result");
        assert!(matches!(
            applied.outcome,
            NodeCommandOutcome::PolicyApplied(result) if result.revision == 1
        ));

        let second = policy_transaction_candidate(3);
        let apply = bus.execute_test(NodeCommandRequest::new(
            93,
            NodeCommand::ApplyPolicy(super::super::NodeApplyPolicyCommand {
                expected_revision: 1,
                candidate: second,
                network_ids: network_ids.clone(),
                includes_local_policy: true,
                confirm_privilege_expansion: true,
            }),
        ));
        let rollback = bus.execute_test(NodeCommandRequest::new(
            94,
            NodeCommand::RollbackPolicy(super::super::NodeRollbackPolicyCommand {
                expected_revision: 1,
                target_revision: 1,
                network_ids,
                includes_local_policy: true,
                confirm_privilege_expansion: true,
            }),
        ));
        let (apply_result, rollback_result) = tokio::join!(apply, rollback);
        let outcomes = [
            apply_result.expect("apply result").outcome,
            rollback_result.expect("rollback result").outcome,
        ];
        let mut success_revisions = Vec::new();
        let mut conflicts = 0;
        for outcome in outcomes {
            match outcome {
                NodeCommandOutcome::PolicyApplied(result) => {
                    success_revisions.push(result.revision);
                }
                NodeCommandOutcome::PolicyRolledBack(result) => {
                    success_revisions.push(result.revision);
                }
                NodeCommandOutcome::Rejected(problem) => {
                    assert_problem(&problem, NodeProblemCode::Conflict, false);
                    conflicts += 1;
                }
                outcome => panic!("unexpected concurrent outcome: {outcome:?}"),
            }
        }
        assert_eq!(success_revisions, vec![2]);
        assert_eq!(conflicts, 1);
        assert!(store
            .current_snapshot(&base)
            .expect("current")
            .local_policy
            .is_some());
        let current = bus
            .execute_test(NodeCommandRequest::new(95, NodeCommand::GetPolicy))
            .await
            .expect("current policy result");
        assert!(matches!(
            current.outcome,
            NodeCommandOutcome::Policy(current) if current.revision == 2
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn policy_apply_and_rollback_reject_stale_webui_authentication() {
        let (bus, state, _events, _store, dir) = policy_runtime_bus("reauth").await;
        let context = NodeCommandContext::owner_session_for_snapshot(
            &state.snapshot().await,
            "webui:stale-owner",
            false,
        );
        for (request_id, command) in [
            (
                96,
                NodeCommand::ApplyPolicy(super::super::NodeApplyPolicyCommand {
                    expected_revision: 0,
                    candidate: policy_transaction_candidate(2),
                    network_ids: vec![policy_transaction_network_id().to_string()],
                    includes_local_policy: true,
                    confirm_privilege_expansion: true,
                }),
            ),
            (
                97,
                NodeCommand::RollbackPolicy(super::super::NodeRollbackPolicyCommand {
                    expected_revision: 0,
                    target_revision: 1,
                    network_ids: vec![policy_transaction_network_id().to_string()],
                    includes_local_policy: true,
                    confirm_privilege_expansion: true,
                }),
            ),
        ] {
            let result = bus
                .execute(
                    context.clone(),
                    NodeCommandRequest::new(request_id, command),
                )
                .await
                .expect("authenticated command result");
            let NodeCommandOutcome::Rejected(problem) = result.outcome else {
                panic!("stale authentication must be rejected");
            };
            assert_problem(&problem, NodeProblemCode::AuthenticationRequired, false);
            assert_eq!(
                problem.params.get("authorization_code").map(String::as_str),
                Some("permission.recent_authentication_required")
            );
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn get_snapshot_refreshes_policy_written_by_an_external_store_operation() {
        let (bus, state, _events, store, dir) = policy_runtime_bus("external").await;
        let base = policy_transaction_snapshot();
        let candidate = policy_transaction_candidate(2);
        let winner = store
            .apply(
                &base,
                0,
                candidate.clone(),
                &[policy_transaction_network_id().to_string()],
                true,
                true,
                "external-owner",
                "test",
            )
            .expect("external apply");
        assert_eq!(winner.revision, 1);
        assert_eq!(
            state.snapshot().await.access_control.local_policy,
            base.local_policy
        );

        let result = bus
            .execute_test(NodeCommandRequest::new(98, NodeCommand::GetSnapshot))
            .await
            .expect("snapshot result");
        match result.outcome {
            NodeCommandOutcome::Snapshot(snapshot) => {
                assert_eq!(snapshot.access_control.local_policy, candidate.local_policy);
                assert_eq!(snapshot.access_control.network_acls, candidate.network_acls);
            }
            outcome => panic!("unexpected snapshot outcome: {outcome:?}"),
        }
        let _ = std::fs::remove_dir_all(dir);
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
                exit_contribution: Default::default(),
                exit_contribution_scope: Default::default(),
                exit_contribution_mode: Default::default(),
                exit_selection_policy: Default::default(),
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
                exit_contribution_acknowledged: None,
                signed_join_preview: None,
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

    struct SignedTicketJoinFixture {
        identity: Identity,
        command: NodeJoinNetworkCommand,
    }

    fn signed_ticket_join_fixture_with_mode(
        now_unix_ms: u64,
        join_mode: fuckipv6_common::domain::NetworkJoinMode,
    ) -> SignedTicketJoinFixture {
        use fuckipv6_common::domain::{NetworkCreateProfile, NetworkVisibility, PublicKey};

        let identity = Identity::generate().expect("generate coordinator identity");
        let public_key = PublicKey::from_bytes(identity.public_key_bytes());
        let owner = NodeId::from_public_key(&identity.public_key_bytes());
        let record = NetworkRecord::create_owned(
            NetworkCreateProfile {
                name: "direct-ticket".into(),
                purpose: "Direct ticket join test".into(),
                icon: "network".into(),
                language: "en".into(),
                region: "ZZ".into(),
                tags: Vec::new(),
                capability_declarations: Vec::new(),
                visibility: NetworkVisibility::Private,
                join_mode,
                exit_contribution: Default::default(),
                exit_contribution_scope: Default::default(),
                exit_contribution_mode: Default::default(),
                exit_selection_policy: Default::default(),
            },
            public_key,
            owner,
            now_unix_ms.saturating_sub(60_000),
        )
        .expect("create network record");
        let preview = NetworkJoinPreview::for_record(
            &record,
            &identity,
            now_unix_ms.saturating_sub(1_000),
            now_unix_ms.saturating_add(60_000),
        )
        .expect("sign join preview");
        let target = NetworkTicket::new(
            identity.public_key_bytes(),
            "127.0.0.1:16666",
            record.name.clone(),
        )
        .encode();
        let command = NodeJoinNetworkCommand {
            target,
            network_name: None,
            start: false,
            discovery_record: None,
            network_id: None,
            preview_digest: None,
            network_policy_revision: None,
            privacy_policy_revision: None,
            terms_hash: None,
            acknowledged_at_unix_ms: Some(now_unix_ms),
            exit_contribution_acknowledged: None,
            signed_join_preview: Some(preview),
        };
        SignedTicketJoinFixture { identity, command }
    }

    fn signed_ticket_join_fixture(now_unix_ms: u64) -> SignedTicketJoinFixture {
        signed_ticket_join_fixture_with_mode(
            now_unix_ms,
            fuckipv6_common::domain::NetworkJoinMode::Open,
        )
    }

    #[test]
    fn signed_ticket_join_preview_rejects_authorization_join_modes() {
        use fuckipv6_common::domain::NetworkJoinMode;

        let now = 1_800_000_000_000;
        for (join_mode, expected_key) in [
            (NetworkJoinMode::InviteOnly, "join.error.invite_only"),
            (
                NetworkJoinMode::ApprovalRequired,
                "join.error.approval_required",
            ),
        ] {
            let fixture = signed_ticket_join_fixture_with_mode(now, join_mode);
            let preview = fixture
                .command
                .signed_join_preview
                .as_ref()
                .expect("signed preview");
            let problem = validate_signed_ticket_join_preview(&fixture.command, preview, now)
                .expect_err("authorization join modes must not accept a location ticket");
            assert_eq!(problem.message_key.as_str(), expected_key);
        }
    }

    #[test]
    fn signed_ticket_join_preview_is_revalidated_and_pins_the_acknowledgement() {
        let now = 1_800_000_000_000;
        let fixture = signed_ticket_join_fixture(now);
        let preview = fixture
            .command
            .signed_join_preview
            .as_ref()
            .expect("signed preview");
        let ack = validate_signed_ticket_join_preview(&fixture.command, preview, now)
            .expect("valid signed preview");
        assert_eq!(ack.terms_hash, preview.terms_hash);
        assert_eq!(ack.acknowledged_at_unix_ms, now);

        let mut changed_network = preview.clone();
        changed_network.network_id = NetworkId::from_public_key(b"another-network");
        let problem = validate_signed_ticket_join_preview(&fixture.command, &changed_network, now)
            .expect_err("changed network id must be rejected");
        assert_eq!(
            problem.message_key.as_str(),
            "join.error.signed_preview_identity_mismatch"
        );

        let mut changed_terms = preview.clone();
        changed_terms.terms.member_limit = Some(17);
        let problem = validate_signed_ticket_join_preview(&fixture.command, &changed_terms, now)
            .expect_err("changed terms must be rejected");
        assert_eq!(
            problem.message_key.as_str(),
            "join.error.signed_preview_changed"
        );

        let mut changed_signature = preview.clone();
        changed_signature.signature = fuckipv6_common::domain::Signature::from_bytes([7; 64]);
        let problem =
            validate_signed_ticket_join_preview(&fixture.command, &changed_signature, now)
                .expect_err("changed signature must be rejected");
        assert_eq!(
            problem.message_key.as_str(),
            "join.error.signed_preview_signature_invalid"
        );

        assert!(validate_signed_ticket_join_preview(
            &fixture.command,
            preview,
            now.saturating_add(120_000),
        )
        .is_err());

        let wrong_identity = Identity::generate().expect("generate wrong ticket identity");
        let mut wrong_ticket = fixture.command.clone();
        wrong_ticket.target = NetworkTicket::new(
            wrong_identity.public_key_bytes(),
            "127.0.0.1:16666",
            preview.network_name.clone(),
        )
        .encode();
        let problem = validate_signed_ticket_join_preview(&wrong_ticket, preview, now)
            .expect_err("ticket signed by another owner must be rejected");
        assert_eq!(
            problem.message_key.as_str(),
            "join.error.signed_preview_identity_mismatch"
        );

        assert!(
            !preview.signature_is_valid(wrong_identity.public_key().as_slice()),
            "fixture must not validate under a different key"
        );
        assert!(preview.signature_is_valid(&fixture.identity.public_key_bytes()));
    }

    #[test]
    fn signed_ticket_join_default_acknowledgement_tolerates_coordinator_clock_skew() {
        let now = 1_800_000_000_000;
        let mut fixture = signed_ticket_join_fixture(now);
        {
            let preview = fixture
                .command
                .signed_join_preview
                .as_mut()
                .expect("signed preview");
            preview.issued_at_unix_ms = now + 2_000;
            preview.expires_at_unix_ms = preview.issued_at_unix_ms + 60_000;
            preview.preview_digest = preview.compute_digest().expect("preview digest");
            preview.signature = Signature::from_bytes(
                fixture
                    .identity
                    .sign(&preview.signing_payload().expect("signing payload"))
                    .to_bytes(),
            );
        }
        let preview = fixture
            .command
            .signed_join_preview
            .as_ref()
            .expect("signed preview")
            .clone();
        fixture.command.acknowledged_at_unix_ms = None;

        let ack = validate_signed_ticket_join_preview(&fixture.command, &preview, now)
            .expect("default acknowledgement must tolerate an accepted future issue time");
        assert_eq!(ack.acknowledged_at_unix_ms, preview.issued_at_unix_ms);

        fixture.command.acknowledged_at_unix_ms = Some(now);
        let problem = validate_signed_ticket_join_preview(&fixture.command, &preview, now)
            .expect_err("an explicit acknowledgement before issuance must still be rejected");
        assert_eq!(
            problem.message_key.as_str(),
            "join.error.acknowledgement_time_invalid"
        );
    }

    #[tokio::test]
    async fn signed_ticket_join_preview_persists_an_accepted_membership() {
        let now = unix_time_ms();
        let fixture = signed_ticket_join_fixture(now);
        let configs = ConfigManager::new_for_dir(std::env::temp_dir().join(format!(
            "fuckipv6-signed-preview-{}-{now}",
            std::process::id()
        )))
        .expect("config manager");
        let preview = fixture
            .command
            .signed_join_preview
            .as_ref()
            .expect("signed preview")
            .clone();
        let ack = validate_signed_ticket_join_preview(&fixture.command, &preview, now)
            .expect("valid signed preview");
        let (persisted, inserted) =
            persist_joined_membership(&configs, JoinAcceptance::from_signed_preview(&preview, ack))
                .await
                .expect("persist membership");
        assert!(inserted);
        assert_eq!(persisted.network_name, preview.network_name);
        assert_eq!(persisted.membership.network_id, preview.network_id);
        assert_eq!(persisted.membership.role, MembershipRole::Member);
        assert_eq!(
            persisted.membership.state,
            fuckipv6_common::domain::MembershipState::Accepted
        );
        assert_eq!(
            configs.list_joined_memberships().expect("list memberships"),
            vec![persisted]
        );
        let _ = std::fs::remove_dir_all(configs.config_dir());
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
    fn exit_contribution_requires_capability_and_explicit_consent() {
        let mut snapshot = super::super::NodeSnapshot::default();
        let unsupported = validate_exit_contribution_admission(
            &snapshot,
            ExitContributionObligation::Required,
            ExitContributionScope::JoiningMembers,
            ExitContributionMode::RequiredOnJoin,
            Some(true),
            "lab",
        )
        .expect_err("unsupported hosts cannot accept an Exit obligation");
        assert_problem(&unsupported, NodeProblemCode::Unsupported, false);
        assert_eq!(
            unsupported.message_key,
            "join.error.exit_capability_required"
        );

        snapshot.exit_node_capabilities = ExitNodeCapabilities {
            schema_version: EXIT_NODE_CAPABILITY_SCHEMA_VERSION,
            platform: ExitNodePlatform::Linux,
            supported: true,
            ipv4_forwarding: true,
            ipv6_forwarding: true,
            nat: true,
            cleanup: true,
            unsupported_reasons: Vec::new(),
        };
        let missing_consent = validate_exit_contribution_admission(
            &snapshot,
            ExitContributionObligation::Required,
            ExitContributionScope::AllMembers,
            ExitContributionMode::RequiredOnJoin,
            None,
            "lab",
        )
        .expect_err("capability must not imply consent");
        assert_problem(&missing_consent, NodeProblemCode::InvalidRequest, false);
        assert_eq!(
            missing_consent.message_key,
            "join.error.exit_contribution_consent_required"
        );
        assert!(validate_exit_contribution_admission(
            &snapshot,
            ExitContributionObligation::Required,
            ExitContributionScope::JoiningMembers,
            ExitContributionMode::RequiredOnJoin,
            Some(true),
            "lab",
        )
        .is_ok());
        assert!(validate_exit_contribution_admission(
            &snapshot,
            ExitContributionObligation::NotRequired,
            ExitContributionScope::NotApplicable,
            ExitContributionMode::Disabled,
            None,
            "lab",
        )
        .is_ok());
        assert!(validate_exit_contribution_admission(
            &snapshot,
            ExitContributionObligation::NotRequired,
            ExitContributionScope::AllMembers,
            ExitContributionMode::Autonomous,
            Some(true),
            "lab",
        )
        .is_err());
    }

    #[test]
    fn non_commitment_exit_modes_do_not_require_host_exit_or_consent() {
        let snapshot = super::super::NodeSnapshot::default();
        for mode in [
            ExitContributionMode::Autonomous,
            ExitContributionMode::Disabled,
            ExitContributionMode::ApprovalRequired,
        ] {
            assert!(validate_exit_contribution_admission(
                &snapshot,
                ExitContributionObligation::NotRequired,
                ExitContributionScope::NotApplicable,
                mode,
                None,
                "lab",
            )
            .is_ok());

            let mut preview = join_preview_for_local_capability_test(mode);
            apply_local_join_capabilities(&mut preview, &snapshot);
            assert!(preview.can_join, "{mode:?}");
            assert!(preview.can_join_without_local_exit, "{mode:?}");
            assert!(!preview.join_requires_exit_consent, "{mode:?}");
            assert!(!preview.local_exit_available, "{mode:?}");
            assert!(!preview.local_exit_enabled, "{mode:?}");
        }
    }

    #[test]
    fn inconsistent_exit_mode_policy_is_rejected_with_mode_context() {
        let snapshot = super::super::NodeSnapshot::default();
        let problem = validate_exit_contribution_admission(
            &snapshot,
            ExitContributionObligation::Required,
            ExitContributionScope::NotApplicable,
            ExitContributionMode::RequiredOnJoin,
            Some(true),
            "lab",
        )
        .expect_err("inconsistent compatibility fields must fail closed");
        assert_problem(&problem, NodeProblemCode::InvalidRequest, false);
        assert_eq!(
            problem.message_key.as_str(),
            "join.error.invalid_exit_contribution_policy"
        );
        assert_eq!(
            problem
                .params
                .get("policy_exit_contribution_mode")
                .map(String::as_str),
            Some("required_on_join")
        );
    }

    #[tokio::test]
    async fn policy_reconfirmation_requires_exit_consent_and_clears_pending_state() {
        use crate::config_manager::ConfigManager;
        use fuckipv6_common::domain::{JoinTermsAck, MembershipState};

        let dir = std::env::temp_dir().join(format!(
            "fuckipv6-policy-reconfirmation-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let configs = ConfigManager::new_for_dir(dir.clone()).expect("config manager");
        let now = unix_time_ms();
        let network_id = NetworkId::from_public_key(b"policy-reconfirmation-network");
        let node_id = fuckipv6_common::domain::NodeId::from_public_key(b"policy-local-node");
        let initial_terms = MembershipTerms {
            network_policy_revision: 1,
            privacy_policy_revision: 1,
            exit_contribution: ExitContributionObligation::Required,
            exit_contribution_scope: ExitContributionScope::JoiningMembers,
            exit_contribution_mode: fuckipv6_common::domain::ExitContributionMode::RequiredOnJoin,
            member_limit: None,
        };
        let mut membership = fuckipv6_common::domain::NetworkMembership::new(
            network_id.clone(),
            node_id,
            MembershipRole::Member,
            now.saturating_sub(20),
        );
        membership
            .begin_apply(
                initial_terms.clone(),
                JoinTermsAck {
                    terms_hash: initial_terms.content_hash().expect("initial hash"),
                    acknowledged_at_unix_ms: now.saturating_sub(20),
                },
                now.saturating_sub(20),
            )
            .expect("begin applying");
        membership
            .accept(now.saturating_sub(19))
            .expect("accept membership");
        assert_eq!(membership.state, MembershipState::Accepted);
        configs
            .upsert_joined_membership("policy-lab", membership)
            .expect("persist membership");

        let mut current_terms = initial_terms;
        current_terms.network_policy_revision = 2;
        current_terms.exit_contribution_scope = ExitContributionScope::AllMembers;
        let current_hash = current_terms.content_hash().expect("current hash");
        configs
            .observe_joined_membership_policies(
                &[(network_id.as_str().to_owned(), current_terms.clone())]
                    .into_iter()
                    .collect(),
                now.saturating_sub(5),
            )
            .expect("observe policy change");

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
                };
            })
            .await;

        let command = |exit_acknowledged| crate::node::NodeAcknowledgeMembershipPolicyCommand {
            network_id: network_id.as_str().to_owned(),
            expected_terms_hash: current_hash.clone(),
            acknowledged_at_unix_ms: now,
            exit_contribution_acknowledged: exit_acknowledged,
        };
        let rejected = acknowledge_membership_policy(&state, &configs, 1, command(None)).await;
        let NodeCommandOutcome::Rejected(problem) = rejected.outcome else {
            panic!("ordinary policy acceptance must not imply Exit consent");
        };
        assert_eq!(
            problem.message_key,
            "join.error.exit_contribution_consent_required"
        );

        let acknowledged =
            acknowledge_membership_policy(&state, &configs, 2, command(Some(true))).await;
        let NodeCommandOutcome::MembershipPolicyAcknowledged(snapshot) = acknowledged.outcome
        else {
            panic!("policy acknowledgement must complete");
        };
        assert_eq!(snapshot.current_terms.as_ref(), Some(&current_terms));
        assert!(!snapshot.policy_confirmation_required);
        assert_eq!(snapshot.terms_hash.as_deref(), Some(current_hash.as_str()));

        let persisted = configs
            .joined_membership(network_id.as_str())
            .expect("load persisted membership")
            .expect("persisted membership");
        assert_eq!(persisted.membership.local_policy_revision, 2);
        assert!(!persisted.membership.policy_confirmation_required());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn required_invitation_exit_obligation_requires_separate_consent() {
        let mut preview = invitation_preview();
        preview.terms.exit_contribution = ExitContributionObligation::Required;
        preview.terms.exit_contribution_scope = ExitContributionScope::JoiningMembers;
        preview.terms.exit_contribution_mode = ExitContributionMode::RequiredOnJoin;
        preview.terms_hash = preview.terms.content_hash().expect("terms hash");
        rebind_preview_digest(&mut preview);
        let mut request = invitation_redeem_request(&preview);

        request.exit_contribution_acknowledged = None;
        let problem = validate_invitation_preview(&request, &preview, 95_000)
            .expect_err("terms acceptance cannot imply Exit contribution consent");
        assert_problem(&problem, NodeProblemCode::InvalidRequest, false);
        assert_eq!(
            problem.message_key,
            "join.error.exit_contribution_consent_required"
        );

        request.exit_contribution_acknowledged = Some(false);
        assert!(validate_invitation_preview(&request, &preview, 95_000).is_err());
        request.exit_contribution_acknowledged = Some(true);
        assert!(validate_invitation_preview(&request, &preview, 95_000).is_ok());
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
            invited_by: NodeId::from_public_key(b"owner"),
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

    fn join_preview_for_local_capability_test(
        exit_contribution_mode: ExitContributionMode,
    ) -> NodeNetworkJoinPreview {
        let exit_contribution = if exit_contribution_mode.requires_join_commitment() {
            ExitContributionObligation::Required
        } else {
            ExitContributionObligation::NotRequired
        };
        let exit_contribution_scope = if exit_contribution_mode.requires_join_commitment() {
            ExitContributionScope::JoiningMembers
        } else {
            ExitContributionScope::NotApplicable
        };
        NodeNetworkJoinPreview {
            network_id: NetworkId::from_public_key(b"local-exit-preview").to_string(),
            network_name: "exit-lab".into(),
            public_key: "network-public-key".into(),
            owner: "network-owner".into(),
            administrators: Vec::new(),
            discovery_revision: 12,
            network_policy_revision: 4,
            privacy_policy_revision: 2,
            join_mode: fuckipv6_common::domain::NetworkJoinMode::Open,
            visibility: fuckipv6_common::domain::NetworkVisibility::Public,
            capability_summary: vec![CapabilityKind::Exit],
            endpoints: vec![fuckipv6_common::domain::NetworkEndpoint {
                transport: fuckipv6_common::domain::DiscoveryTransport::Websocket,
                host: "coordinator.example.test".into(),
                port: 443,
            }],
            member_count: 3,
            member_limit: Some(16),
            exit_contribution,
            exit_contribution_scope,
            exit_contribution_mode,
            exit_contribution_purpose_key: "join.exit.disclosure.purpose".into(),
            exit_contribution_bandwidth_impact_key: "join.exit.disclosure.bandwidth".into(),
            exit_contribution_traffic_impact_key: "join.exit.disclosure.traffic".into(),
            exit_contribution_logging_boundary_key: "join.exit.disclosure.logging".into(),
            exit_contribution_operational_impact_key: "join.exit.disclosure.operational_impact"
                .into(),
            exit_contribution_disable_key: "join.exit.disclosure.disable".into(),
            local_exit_available: false,
            local_exit_enabled: false,
            can_join_without_local_exit: !exit_contribution_mode.requires_join_commitment(),
            join_requires_exit_consent: exit_contribution_mode.requires_join_commitment(),
            privacy_changes: Vec::new(),
            already_member: false,
            can_join: true,
            blocked_reason_key: None,
            terms_hash: ContentHash::from_bytes(b"local exit preview terms"),
            preview_digest: "preview-digest".into(),
            issued_at_unix_ms: 1_700_000_000_000,
            expires_at_unix_ms: 1_700_000_100_000,
            locally_contributed_capabilities: Vec::new(),
            contribution_policy_key: "join.exit.contribution_policy".into(),
        }
    }

    fn local_capability_snapshot(
        supported: bool,
        network_exit_role_enabled: bool,
        owner_exit_enabled: bool,
        exit_capability_state: fuckipv6_common::domain::CapabilityState,
    ) -> crate::node::NodeSnapshot {
        use crate::node::state::{NodeCapabilityRegistrySnapshot, NodeCapabilitySnapshot};
        use crate::node::{NodeNetworkSnapshot, NodeNetworkStatus, NodeTrafficSnapshot};

        crate::node::NodeSnapshot {
            exit_node_capabilities: ExitNodeCapabilities {
                supported,
                ..ExitNodeCapabilities::default()
            },
            capabilities: NodeCapabilityRegistrySnapshot {
                capabilities: vec![
                    NodeCapabilitySnapshot {
                        kind: CapabilityKind::Exit,
                        state: exit_capability_state,
                    },
                    NodeCapabilitySnapshot {
                        kind: CapabilityKind::File,
                        state: fuckipv6_common::domain::CapabilityState::Enabled,
                    },
                ],
            },
            networks: vec![NodeNetworkSnapshot {
                name: "exit-lab".into(),
                tun_name: "fk-exit-lab".into(),
                status: NodeNetworkStatus::Online,
                assigned_ipv6: None,
                assigned_ipv4: None,
                p2p_listen_addr: None,
                uptime_secs: 30,
                roles: if network_exit_role_enabled {
                    vec![NodeNetworkRole::Member, NodeNetworkRole::ExitNode]
                } else {
                    vec![NodeNetworkRole::Member]
                },
                peer_count: 2,
                traffic: NodeTrafficSnapshot::default(),
            }],
            local_exit: crate::node::LocalExitCapabilitySnapshot {
                owner_exit_enabled,
                ..crate::node::LocalExitCapabilitySnapshot::default()
            },
            ..crate::node::NodeSnapshot::default()
        }
    }

    #[test]
    fn required_exit_without_local_support_is_explicitly_not_joinable() {
        let mut preview =
            join_preview_for_local_capability_test(ExitContributionMode::RequiredOnJoin);
        let snapshot = local_capability_snapshot(
            false,
            false,
            false,
            fuckipv6_common::domain::CapabilityState::Unsupported,
        );

        apply_local_join_capabilities(&mut preview, &snapshot);

        assert!(!preview.can_join);
        assert_eq!(
            preview.blocked_reason_key.as_deref(),
            Some("join.error.exit_capability_required")
        );
        assert!(!preview.can_join_without_local_exit);
        assert!(preview.join_requires_exit_consent);
        assert!(!preview.local_exit_available);
        assert!(!preview.local_exit_enabled);
        assert_eq!(
            preview.locally_contributed_capabilities,
            vec![CapabilityKind::File]
        );
    }

    #[test]
    fn required_exit_with_local_support_keeps_capability_and_enabled_state_separate() {
        let mut preview =
            join_preview_for_local_capability_test(ExitContributionMode::RequiredOnJoin);
        let snapshot = local_capability_snapshot(
            true,
            false,
            false,
            fuckipv6_common::domain::CapabilityState::Enabled,
        );

        apply_local_join_capabilities(&mut preview, &snapshot);

        assert!(preview.can_join);
        assert_eq!(preview.blocked_reason_key, None);
        assert!(!preview.can_join_without_local_exit);
        assert!(preview.join_requires_exit_consent);
        assert!(preview.local_exit_available);
        assert!(!preview.local_exit_enabled);
        assert_eq!(
            preview.locally_contributed_capabilities,
            vec![CapabilityKind::Exit, CapabilityKind::File]
        );

        let enabled_snapshot = local_capability_snapshot(
            true,
            true,
            true,
            fuckipv6_common::domain::CapabilityState::Enabled,
        );
        apply_local_join_capabilities(&mut preview, &enabled_snapshot);
        assert!(preview.local_exit_enabled);
    }

    #[test]
    fn network_exit_role_does_not_override_disabled_local_owner_intent() {
        let mut preview = join_preview_for_local_capability_test(ExitContributionMode::Disabled);
        let snapshot = local_capability_snapshot(
            true,
            true,
            false,
            fuckipv6_common::domain::CapabilityState::Enabled,
        );

        apply_local_join_capabilities(&mut preview, &snapshot);

        assert!(preview.local_exit_available);
        assert!(!preview.local_exit_enabled);
    }

    #[test]
    fn optional_exit_join_remains_available_without_local_exit_support() {
        let mut preview = join_preview_for_local_capability_test(ExitContributionMode::Disabled);
        let snapshot = local_capability_snapshot(
            false,
            false,
            false,
            fuckipv6_common::domain::CapabilityState::Unsupported,
        );

        apply_local_join_capabilities(&mut preview, &snapshot);

        assert!(preview.can_join);
        assert_eq!(preview.blocked_reason_key, None);
        assert!(preview.can_join_without_local_exit);
        assert!(!preview.join_requires_exit_consent);
        assert!(!preview.local_exit_available);
        assert!(!preview.local_exit_enabled);
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
