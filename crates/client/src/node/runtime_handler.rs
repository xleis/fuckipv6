use std::sync::Arc;
use std::time::Duration;

use super::runtime_control::{network_setup_problem_code, RestartSummary};
use super::{NetworkRuntimeManager, RuntimeControlError, StartSummary};
use super::{
    NodeCommandContext, NodeCommandFuture, NodeCommandHandler, NodeCommandOutcome,
    NodeCommandRequest, NodeCommandResult, NodePeerProbeSnapshot, NodeProblem, NodeProblemCode,
    NodeStateStore, NodeValidationIssue,
};
use crate::config_manager::ConfigManager;
use crate::config_transaction::{ConfigurationTransactionError, ConfigurationValidationCode};
use crate::ipc::MultiNetworkRegistry;
use crate::network_setup::NetworkSetupStage;
use crate::symmetric::EmbeddedServiceManager;

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
        }
    }

    fn with_runtime(
        state: NodeStateStore,
        registry: MultiNetworkRegistry,
        services: EmbeddedServiceManager,
        runtime: NetworkRuntimeManager,
        configs: ConfigManager,
    ) -> Self {
        Self {
            state,
            registry,
            services,
            runtime: Some(runtime),
            configs: Some(configs),
        }
    }

    fn execute(
        &self,
        actor: Option<(String, String)>,
        request: NodeCommandRequest,
    ) -> NodeCommandFuture {
        let state = self.state.clone();
        let registry = self.registry.clone();
        let services = self.services.clone();
        let runtime = self.runtime.clone();
        let configs = self.configs.clone();

        Box::pin(async move {
            let request_id = request.request_id;
            if let Err(problem) = request.validate() {
                return NodeCommandResult::rejected(request_id, problem);
            }

            match request.command {
                super::NodeCommand::GetSnapshot => {
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
                super::NodeCommand::ApplyConfiguration(command) => match (configs, actor) {
                    (Some(configs), Some((actor, source))) => {
                        apply_configuration(
                            &configs,
                            request_id,
                            command.expected_revision,
                            command.networks,
                            actor,
                            source,
                        )
                        .await
                    }
                    (None, _) => unavailable(request_id, "配置应用需要节点配置管理器"),
                    (_, None) => unavailable(request_id, "配置应用缺少受信任的认证上下文"),
                },
                super::NodeCommand::RollbackConfiguration(command) => match (configs, actor) {
                    (Some(configs), Some((actor, source))) => {
                        rollback_configuration(
                            &configs,
                            request_id,
                            command.expected_revision,
                            command.target_revision,
                            actor,
                            source,
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
                    Some(runtime) => diagnostics_runtime(&runtime, request_id).await,
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
                super::NodeCommand::JoinNetwork(command) => match runtime {
                    Some(runtime) => join_runtime_network(&runtime, request_id, command).await,
                    None => unavailable(request_id, "加入网络尚未接入内嵌网络服务 actor"),
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
        self.execute(None, request)
    }

    fn handle_authorized(
        &self,
        context: NodeCommandContext,
        request: NodeCommandRequest,
    ) -> NodeCommandFuture {
        self.execute(
            Some((
                context.actor().as_str().to_owned(),
                context.source().to_owned(),
            )),
            request,
        )
    }
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
) -> NodeCommandResult {
    match runtime.diagnostics().await {
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
                config_saved: result.config_saved,
                started: result.started,
                invitation_ticket: result.ticket,
            }),
        ),
        Err(error) => runtime_rejection(request_id, error),
    }
}

async fn join_runtime_network(
    runtime: &NetworkRuntimeManager,
    request_id: u64,
    command: super::NodeJoinNetworkCommand,
) -> NodeCommandResult {
    match runtime.join(command).await {
        Ok(result) => NodeCommandResult::completed(
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
        ),
        Err(error) => runtime_rejection(request_id, error),
    }
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
    use fuckipv6_common::{
        ExitNodeCapabilities, ExitNodePlatform, NetworkMode, EXIT_NODE_CAPABILITY_SCHEMA_VERSION,
    };
    use tokio::sync::mpsc;

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
                signaling_port: 6666,
                relay_port: 6667,
                external_host: None,
                auto_connect: true,
            }),
            NodeCommand::JoinNetwork(NodeJoinNetworkCommand {
                target: "fk://invite".into(),
                network_name: Some("office".into()),
                start: true,
            }),
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
                NodeProblemCode::Unavailable,
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
