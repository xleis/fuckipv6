//! Authoritative node operation lifecycle owned by the node process.
//!
//! Adapters submit commands here, but never own idempotency or operation state.
//! The registry is persisted before any side effect is dispatched. Results are
//! intentionally not persisted because command results can contain sensitive
//! one-time material such as invitation tickets.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use fuckipv6_common::domain::{
    ContentHash, DomainActorRef, DomainObjectRef, IdempotencyKey, IdempotencyRegistry,
    IdempotentSubmission, NodeId, ObjectKind, Operation, OperationError, OperationId,
    OperationKind, OperationRecovery, OperationState,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::local_ipc::{self, IpcRole, LocalEndpoint, LocalError};
use crate::node::{
    NodeCommand, NodeCommandBus, NodeCommandBusError, NodeCommandContext, NodeCommandOutcome,
    NodeCommandRequest, NodeCommandResult, NodeErrorCategory, NodeEventBus, NodeEventKind,
    NodeProblem, NodeProblemCode, NodeSnapshot, NodeStateStore,
};

const STORE_FILE: &str = "operations.json";
const INTERRUPTED_ERROR_CODE: &str = "operation.interrupted";
const INTERRUPTED_MESSAGE_KEY: &str = "error.operation_interrupted";
const CANCEL_NOT_SUPPORTED: &str = "operation.cancel.not_supported";

/// Transport projection of the shared operation state machine. A result is
/// available only while the node process that executed the operation is alive.
#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct NodeOperationSnapshot {
    #[serde(flatten)]
    pub operation: Operation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<NodeCommandResult>,
}

impl NodeOperationSnapshot {
    /// Stable identifier that a user can copy into a support report.
    pub fn diagnostic_id(&self) -> &str {
        self.operation.operation_id.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum OperationSubmitError {
    Invalid(NodeProblem),
    Conflict(OperationId),
    Internal(String),
}

impl fmt::Display for OperationSubmitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(problem) => write!(formatter, "{}", problem.message),
            Self::Conflict(id) => write!(formatter, "幂等键冲突；原操作: {}", id.as_str()),
            Self::Internal(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for OperationSubmitError {}

#[derive(Debug, Clone)]
pub enum OperationClientError {
    Invalid(NodeProblem),
    Conflict(OperationId),
    NotFound(String),
    Unavailable(String),
    Internal(String),
}

impl fmt::Display for OperationClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(problem) => write!(formatter, "{}", problem.message),
            Self::Conflict(id) => write!(formatter, "幂等键冲突；原操作: {}", id.as_str()),
            Self::NotFound(id) => write!(formatter, "operation not found: {id}"),
            Self::Unavailable(message) | Self::Internal(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for OperationClientError {}

impl From<OperationSubmitError> for OperationClientError {
    fn from(error: OperationSubmitError) -> Self {
        match error {
            OperationSubmitError::Invalid(problem) => Self::Invalid(problem),
            OperationSubmitError::Conflict(id) => Self::Conflict(id),
            OperationSubmitError::Internal(message) => Self::Internal(message),
        }
    }
}

#[derive(Clone)]
pub struct OperationService {
    commands: NodeCommandBus,
    state: NodeStateStore,
    events: NodeEventBus,
    registry: Arc<Mutex<OperationRegistry>>,
    store_path: Option<PathBuf>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct OperationRegistry {
    #[serde(default)]
    idempotency: IdempotencyRegistry,
    #[serde(default)]
    records: BTreeMap<String, Operation>,
    /// Results live only in the authoritative process. One-time material such
    /// as invitation tickets must never be copied into durable state.
    #[serde(skip)]
    results: BTreeMap<String, NodeCommandResult>,
}

impl OperationService {
    /// Create an in-memory service for isolated tests and adapters whose node
    /// process is explicitly ephemeral.
    pub fn in_memory(
        commands: NodeCommandBus,
        state: NodeStateStore,
        events: NodeEventBus,
    ) -> Self {
        Self {
            commands,
            state,
            events,
            registry: Arc::new(Mutex::new(OperationRegistry::default())),
            store_path: None,
        }
    }

    /// Create a durable service under the owner-only local runtime directory.
    /// Corrupt or inconsistent state fails closed before the node accepts work.
    pub fn persistent(
        commands: NodeCommandBus,
        state: NodeStateStore,
        events: NodeEventBus,
    ) -> Result<Self> {
        let path = local_ipc::runtime_directory()
            .context("无法定位 operation 持久化目录")?
            .join(STORE_FILE);
        Self::with_store_path(commands, state, events, path)
    }

    pub(crate) fn persistent_for_endpoint(
        endpoint: &LocalEndpoint,
        commands: NodeCommandBus,
        state: NodeStateStore,
        events: NodeEventBus,
    ) -> Result<Self> {
        match endpoint {
            LocalEndpoint::Path(path) => {
                let directory = path.parent().context("节点 IPC endpoint 缺少父目录")?;
                Self::with_store_path(commands, state, events, directory.join(STORE_FILE))
            }
            #[cfg(windows)]
            LocalEndpoint::Name(_) => Self::persistent(commands, state, events),
        }
    }

    fn with_store_path(
        commands: NodeCommandBus,
        state: NodeStateStore,
        events: NodeEventBus,
        store_path: PathBuf,
    ) -> Result<Self> {
        let mut registry = load_registry(&store_path)?;
        let recovered = recover_interrupted(&mut registry);
        if recovered {
            persist_registry(&store_path, &registry)
                .context("无法持久化恢复后的 operation 状态")?;
        }
        Ok(Self {
            commands,
            state,
            events,
            registry: Arc::new(Mutex::new(registry)),
            store_path: Some(store_path),
        })
    }

    pub fn client(&self) -> OperationClient {
        OperationClient::Local(self.clone())
    }

    /// Register and launch a mutating command. Registration, idempotency
    /// resolution and durable persistence happen before the task is spawned.
    pub async fn submit(
        &self,
        context: NodeCommandContext,
        request: NodeCommandRequest,
        idempotency_key: IdempotencyKey,
    ) -> Result<(NodeOperationSnapshot, bool), OperationSubmitError> {
        request.validate().map_err(OperationSubmitError::Invalid)?;
        if request.request_id == 0 {
            return Err(OperationSubmitError::Invalid(NodeProblem::new(
                NodeProblemCode::InvalidRequest,
                "request_id 必须大于 0",
            )));
        }
        if !request.command.kind().is_write() {
            return Err(OperationSubmitError::Internal(
                "read-only command cannot be registered as an operation".into(),
            ));
        }

        let request_digest = ContentHash::from_bytes(
            &serde_json::to_vec(&request.command)
                .map_err(|error| OperationSubmitError::Internal(error.to_string()))?,
        );
        let actor = DomainActorRef {
            node_id: context.actor().clone(),
            display: context.source().to_owned(),
        };
        let operation_id = new_operation_id()?;
        let now = unix_time_ms();
        let operation = Operation {
            operation_id: operation_id.clone(),
            kind: operation_kind(&request.command),
            actor: actor.clone(),
            target: operation_target(&request.command, &actor.node_id),
            idempotency_key: idempotency_key.clone(),
            state: OperationState::Queued,
            stage: "queued".into(),
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
            attempt: 0,
            max_attempts: 3,
            error: None,
            cancellable: false,
            cancel_blocked_reason: Some(CANCEL_NOT_SUPPORTED.into()),
        };
        operation
            .validate()
            .map_err(|error| OperationSubmitError::Internal(error.to_string()))?;

        let (snapshot, created, operation_id) = {
            let mut current = self
                .registry
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut next = current.clone();
            let resolved = next.idempotency.resolve_or_insert(
                &actor.node_id,
                &idempotency_key,
                operation_id.clone(),
                request_digest,
            );
            match resolved {
                IdempotentSubmission::Created { operation_id } => {
                    next.records
                        .insert(operation_id.as_str().to_owned(), operation);
                    self.persist_candidate(&next)
                        .map_err(OperationSubmitError::Internal)?;
                    *current = next;
                    let snapshot = NodeOperationSnapshot {
                        operation: current
                            .records
                            .get(operation_id.as_str())
                            .expect("newly inserted operation")
                            .clone(),
                        result: None,
                    };
                    (snapshot, true, operation_id)
                }
                IdempotentSubmission::Existing { operation_id } => {
                    let snapshot = current
                        .records
                        .get(operation_id.as_str())
                        .map(|operation| NodeOperationSnapshot {
                            operation: operation.clone(),
                            result: current.results.get(operation_id.as_str()).cloned(),
                        })
                        .ok_or_else(|| {
                            OperationSubmitError::Internal(
                                "idempotency registry referenced a missing operation".into(),
                            )
                        })?;
                    (snapshot, false, operation_id)
                }
                IdempotentSubmission::Conflict {
                    existing_operation_id,
                } => return Err(OperationSubmitError::Conflict(existing_operation_id)),
            }
        };

        if created {
            self.publish(&snapshot.operation);
            let service = self.clone();
            let context = context.with_audit_operation_id(operation_id.clone());
            tokio::spawn(async move {
                service.run(operation_id, context, request).await;
            });
        }
        Ok((snapshot, created))
    }

    pub fn lookup(&self, operation_id: &str) -> Option<NodeOperationSnapshot> {
        let registry = self
            .registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        registry
            .records
            .get(operation_id)
            .map(|operation| NodeOperationSnapshot {
                operation: operation.clone(),
                result: registry.results.get(operation_id).cloned(),
            })
    }

    fn persist_candidate(&self, registry: &OperationRegistry) -> Result<(), String> {
        match &self.store_path {
            Some(path) => persist_registry(path, registry).map_err(|error| format!("{error:#}")),
            None => Ok(()),
        }
    }

    async fn run(
        &self,
        operation_id: OperationId,
        context: NodeCommandContext,
        request: NodeCommandRequest,
    ) {
        let request_id = request.request_id;
        if !self.transition(&operation_id, OperationState::Running, "dispatching") {
            return;
        }
        if !self.progress(&operation_id, "executing") {
            return;
        }
        match self.commands.execute(context, request).await {
            Ok(result) => {
                let result = result_for_control_client(result);
                let finished = match result.outcome.clone() {
                    NodeCommandOutcome::Rejected(problem)
                        if problem.code == NodeProblemCode::Cancelled =>
                    {
                        self.finish_cancelled(&operation_id, problem)
                    }
                    NodeCommandOutcome::Rejected(problem)
                        if problem.code == NodeProblemCode::TimedOut =>
                    {
                        self.finish_failed(&operation_id, problem, true)
                    }
                    NodeCommandOutcome::Rejected(problem) => {
                        self.finish_failed(&operation_id, problem, false)
                    }
                    _ => self.finish_succeeded(&operation_id),
                };
                if finished {
                    self.remember_result(&operation_id, result);
                }
            }
            Err(error) => {
                let (code, retryable) = match error {
                    NodeCommandBusError::Invalid(problem) => {
                        self.finish_failed(&operation_id, problem, false);
                        return;
                    }
                    NodeCommandBusError::Closed => (NodeProblemCode::Unavailable, true),
                    NodeCommandBusError::ResponseDropped(_) => (NodeProblemCode::Unavailable, true),
                };
                let message = format!("request_id={request_id} 的节点命令执行中断");
                let problem = NodeProblem::new(code, message).retryable(retryable);
                self.finish_failed(&operation_id, problem, false);
            }
        }
    }

    fn transition(&self, id: &OperationId, state: OperationState, stage: &str) -> bool {
        self.mutate_and_publish(id, |operation| {
            operation.transition(state, stage, unix_time_ms())
        })
    }

    fn progress(&self, id: &OperationId, stage: &str) -> bool {
        self.mutate_and_publish(id, |operation| operation.progress(stage, unix_time_ms()))
    }

    fn finish_succeeded(&self, id: &OperationId) -> bool {
        self.mutate_and_publish(id, |operation| {
            operation.transition(OperationState::Succeeded, "succeeded", unix_time_ms())
        })
    }

    fn finish_failed(&self, id: &OperationId, problem: NodeProblem, timed_out: bool) -> bool {
        self.mutate_and_publish(id, |operation| {
            let error = operation_error_for_problem(&problem, &operation.target);
            operation.fail(error, timed_out, unix_time_ms())
        })
    }

    fn finish_cancelled(&self, id: &OperationId, problem: NodeProblem) -> bool {
        self.mutate_and_publish(id, |operation| {
            let error = operation_error_for_problem(&problem, &operation.target);
            operation.finish_cancelled(Some(error), unix_time_ms())
        })
    }

    fn mutate_and_publish(
        &self,
        id: &OperationId,
        mutate: impl FnOnce(&mut Operation) -> Result<(), fuckipv6_common::domain::DomainError>,
    ) -> bool {
        let changed = {
            let mut current = self
                .registry
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut next = current.clone();
            let operation = {
                let Some(operation) = next.records.get_mut(id.as_str()) else {
                    return false;
                };
                if mutate(operation).is_err() {
                    return false;
                }
                operation.clone()
            };
            if let Err(error) = self.persist_candidate(&next) {
                tracing::error!(%error, operation_id = id.as_str(), "operation 状态持久化失败");
                return false;
            }
            *current = next;
            operation
        };
        self.publish(&changed);
        true
    }

    fn remember_result(&self, id: &OperationId, result: NodeCommandResult) {
        self.registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .results
            .insert(id.as_str().to_owned(), result);
    }

    fn publish(&self, operation: &Operation) {
        self.events.publish(NodeEventKind::OperationChanged {
            operation_id: operation.operation_id.as_str().to_owned(),
            state: operation.state,
            stage: operation.stage.clone(),
        });
    }

    /// State is read from the same authoritative store used by the command bus.
    pub async fn snapshot(&self) -> Option<NodeSnapshot> {
        Some(self.state.snapshot().await)
    }
}

/// Adapter-facing operation API. Production adapters use the IPC variant and
/// therefore never become an operation authority themselves.
#[derive(Clone)]
pub enum OperationClient {
    Local(OperationService),
    Ipc {
        endpoint: LocalEndpoint,
        role: IpcRole,
    },
}

impl OperationClient {
    pub fn remote(endpoint: LocalEndpoint, role: IpcRole) -> Self {
        Self::Ipc { endpoint, role }
    }

    pub async fn submit(
        &self,
        context: NodeCommandContext,
        request: NodeCommandRequest,
        idempotency_key: IdempotencyKey,
    ) -> Result<NodeOperationSnapshot, OperationClientError> {
        match self {
            Self::Local(service) => service
                .submit(context, request, idempotency_key)
                .await
                .map(|(snapshot, _)| snapshot)
                .map_err(OperationClientError::from),
            Self::Ipc { endpoint, role } => local_ipc::submit_operation(
                endpoint,
                role.clone(),
                request,
                idempotency_key,
                context.source().to_owned(),
            )
            .await
            .map_err(operation_client_error_from_ipc),
        }
    }

    pub async fn lookup(
        &self,
        operation_id: &str,
    ) -> Result<Option<NodeOperationSnapshot>, OperationClientError> {
        match self {
            Self::Local(service) => Ok(service.lookup(operation_id)),
            Self::Ipc { endpoint, role } => {
                local_ipc::lookup_operation(endpoint, role.clone(), operation_id)
                    .await
                    .map(Some)
                    .or_else(|error| {
                        let local = error.downcast_ref::<LocalError>();
                        if local
                            .is_some_and(|error| error.code == local_ipc::IpcErrorCode::NotFound)
                        {
                            Ok(None)
                        } else {
                            Err(operation_client_error_from_ipc(error))
                        }
                    })
            }
        }
    }
}

fn operation_client_error_from_ipc(error: anyhow::Error) -> OperationClientError {
    if let Some(local) = error.downcast_ref::<LocalError>() {
        if let Some(submit_error) = &local.operation_error {
            return submit_error.clone().into();
        }
        return match local.code {
            local_ipc::IpcErrorCode::NotFound => OperationClientError::NotFound(error.to_string()),
            local_ipc::IpcErrorCode::BackendUnavailable
            | local_ipc::IpcErrorCode::Timeout
            | local_ipc::IpcErrorCode::EventGap => {
                OperationClientError::Unavailable(error.to_string())
            }
            _ => OperationClientError::Internal(error.to_string()),
        };
    }
    OperationClientError::Unavailable(error.to_string())
}

pub(crate) fn operation_kind(command: &NodeCommand) -> OperationKind {
    match command {
        NodeCommand::StartNetworks { .. } | NodeCommand::StartNode => OperationKind::NodeStart,
        NodeCommand::StopNetworks { .. } | NodeCommand::StopNode | NodeCommand::Shutdown => {
            OperationKind::NodeStop
        }
        NodeCommand::RestartNetworks { .. } | NodeCommand::RestartNode => {
            OperationKind::NodeRestart
        }
        NodeCommand::RebuildTun { .. } => OperationKind::TunRebuild,
        NodeCommand::CleanupResiduals { .. } => OperationKind::NodeCleanup,
        NodeCommand::CreateNetwork(_) => OperationKind::NetworkCreate,
        NodeCommand::JoinNetwork(_) => OperationKind::NetworkApply,
        NodeCommand::UpdateRouting(_) => OperationKind::TrafficRulesApply,
        NodeCommand::SetNetworkRole(_) => OperationKind::NetworkRoleUpdate,
        NodeCommand::BanClient(_) | NodeCommand::UnbanClient(_) => {
            OperationKind::ClientBlacklistUpdate
        }
        NodeCommand::UpdateNetworkAllocation(_) => OperationKind::NetworkAllocationUpdate,
        NodeCommand::ApplyConfiguration(_) => OperationKind::PolicyApply,
        NodeCommand::RollbackConfiguration(_) => OperationKind::PolicyRollback,
        NodeCommand::GetSnapshot
        | NodeCommand::GetUiManifest
        | NodeCommand::GetDiagnostics
        | NodeCommand::GetBlacklist { .. }
        | NodeCommand::GetNetworkAllocation { .. }
        | NodeCommand::ProbePeer(_)
        | NodeCommand::GetConfiguration
        | NodeCommand::PreviewConfiguration(_)
        | NodeCommand::GetConfigurationHistory => OperationKind::PolicyTest,
    }
}

pub(crate) fn operation_target(command: &NodeCommand, local_node_id: &NodeId) -> DomainObjectRef {
    let node_target = || DomainObjectRef {
        kind: ObjectKind::Node,
        id: local_node_id.as_str().to_owned(),
    };
    let network_target = |id: &str| DomainObjectRef {
        kind: ObjectKind::Network,
        id: id.to_owned(),
    };
    match command {
        NodeCommand::CreateNetwork(command) => network_target(&command.name),
        NodeCommand::JoinNetwork(command) => command
            .network_name
            .as_deref()
            .map(network_target)
            .unwrap_or_else(node_target),
        NodeCommand::RebuildTun { network_name } => network_target(network_name),
        NodeCommand::UpdateRouting(command) => network_target(&command.network_name),
        NodeCommand::SetNetworkRole(command) => network_target(&command.network_name),
        NodeCommand::BanClient(command) | NodeCommand::UnbanClient(command) => {
            network_target(&command.network_name)
        }
        NodeCommand::UpdateNetworkAllocation(command) => network_target(&command.network_name),
        NodeCommand::GetBlacklist { network_name }
        | NodeCommand::GetNetworkAllocation { network_name } => network_target(network_name),
        NodeCommand::StartNetworks { .. }
        | NodeCommand::StopNetworks { .. }
        | NodeCommand::RestartNetworks { .. }
        | NodeCommand::GetSnapshot
        | NodeCommand::GetUiManifest
        | NodeCommand::GetDiagnostics
        | NodeCommand::ProbePeer(_)
        | NodeCommand::StartNode
        | NodeCommand::StopNode
        | NodeCommand::RestartNode
        | NodeCommand::CleanupResiduals { .. }
        | NodeCommand::GetConfiguration
        | NodeCommand::PreviewConfiguration(_)
        | NodeCommand::ApplyConfiguration(_)
        | NodeCommand::RollbackConfiguration(_)
        | NodeCommand::GetConfigurationHistory
        | NodeCommand::Shutdown => node_target(),
    }
}

pub(crate) fn operation_error_for_problem(
    problem: &NodeProblem,
    target: &DomainObjectRef,
) -> OperationError {
    let hide_object = matches!(
        problem.category,
        NodeErrorCategory::Authentication | NodeErrorCategory::Authorization
    );
    let code = match problem.code {
        NodeProblemCode::AuthenticationRequired => "operation.authentication_required",
        NodeProblemCode::InvalidRequest => "operation.invalid_request",
        NodeProblemCode::NotFound => "operation.not_found",
        NodeProblemCode::Conflict => "operation.conflict",
        NodeProblemCode::Unavailable => "operation.unavailable",
        NodeProblemCode::PermissionDenied => "operation.permission_denied",
        NodeProblemCode::Cancelled => "operation.cancelled",
        NodeProblemCode::TimedOut => "operation.timed_out",
        NodeProblemCode::Unsupported => "operation.unsupported",
        NodeProblemCode::Internal => "operation.internal",
    };
    let recovery = problem.recovery.as_deref().cloned().or_else(|| {
        problem
            .code
            .recovery()
            .map(|(kind, hint_key)| OperationRecovery {
                kind: kind.into(),
                hint_key: hint_key.into(),
            })
    });
    OperationError {
        code: code.into(),
        message_key: problem.message_key.clone(),
        params: if hide_object {
            BTreeMap::new()
        } else {
            (*problem.params).clone()
        },
        object: if hide_object {
            None
        } else {
            Some(target.clone())
        },
        retryable: problem.retryable,
        recovery,
        partial_changes: Vec::new(),
    }
}

pub(crate) fn result_for_control_client(mut result: NodeCommandResult) -> NodeCommandResult {
    match &mut result.outcome {
        NodeCommandOutcome::Snapshot(snapshot) => {
            snapshot.control_health = crate::node::NodeControlHealth::Available;
        }
        NodeCommandOutcome::Rejected(problem) => {
            *problem = sanitize_problem_for_client(problem.clone());
        }
        _ => {}
    }
    result
}

fn sanitize_problem_for_client(mut problem: NodeProblem) -> NodeProblem {
    if matches!(
        problem.category,
        NodeErrorCategory::Authentication | NodeErrorCategory::Authorization
    ) {
        problem.message = match problem.category {
            NodeErrorCategory::Authentication => "需要重新进行身份认证",
            NodeErrorCategory::Authorization => "无权执行此操作",
            _ => unreachable!("guarded by authentication/authorization check"),
        }
        .into();
        problem.params = Box::default();
        problem.network_name = None;
        problem.details = None;
    }
    problem
}

fn recover_interrupted(registry: &mut OperationRegistry) -> bool {
    let now = unix_time_ms();
    let mut recovered = false;
    for operation in registry.records.values_mut() {
        if operation.state.is_terminal() {
            continue;
        }
        let error = OperationError {
            code: INTERRUPTED_ERROR_CODE.into(),
            message_key: INTERRUPTED_MESSAGE_KEY.into(),
            params: BTreeMap::new(),
            object: Some(operation.target.clone()),
            retryable: true,
            recovery: Some(OperationRecovery {
                kind: "retry".into(),
                hint_key: "recovery.retry".into(),
            }),
            partial_changes: Vec::new(),
        };
        if operation
            .fail(error, true, now.max(operation.updated_at_unix_ms))
            .is_ok()
        {
            recovered = true;
        }
    }
    recovered
}

fn validate_registry(registry: &OperationRegistry) -> Result<()> {
    registry.idempotency.validate()?;
    for (id, operation) in &registry.records {
        if id != operation.operation_id.as_str() {
            bail!("operation registry key does not match operation id");
        }
        operation.validate()?;
    }
    Ok(())
}

fn load_registry(path: &Path) -> Result<OperationRegistry> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(OperationRegistry::default());
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("无法读取 operation 状态: {}", path.display()))
        }
    };
    let registry: OperationRegistry = serde_json::from_slice(&bytes)
        .with_context(|| format!("operation 状态损坏，已失败关闭: {}", path.display()))?;
    validate_registry(&registry).context("operation 状态不一致，已失败关闭")?;
    Ok(registry)
}

fn persist_registry(path: &Path, registry: &OperationRegistry) -> Result<()> {
    validate_registry(registry)?;
    let bytes = serde_json::to_vec_pretty(registry).context("无法序列化 operation 状态")?;
    atomic_write_private(path, &bytes)
}

fn atomic_write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("operation 状态文件缺少父目录")?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("operation 状态文件名无效")?;
    let temporary = parent.join(format!(
        ".{file_name}.tmp-{}-{}",
        std::process::id(),
        random_suffix()
    ));
    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .with_context(|| format!("无法创建 operation 临时文件: {}", temporary.display()))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        sync_directory(parent)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn new_operation_id() -> Result<OperationId, OperationSubmitError> {
    let mut random = [0_u8; 32];
    getrandom::fill(&mut random).map_err(|error| {
        OperationSubmitError::Internal(format!("operation id randomness unavailable: {error}"))
    })?;
    Ok(OperationId::from_public_key(&random))
}

fn random_suffix() -> String {
    let mut random = [0_u8; 12];
    if getrandom::fill(&mut random).is_ok() {
        return random.iter().map(|byte| format!("{byte:02x}")).collect();
    }
    format!(
        "{:x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    )
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().try_into().unwrap_or(u64::MAX))
        .unwrap_or(1)
        .max(1)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;
    use crate::node::{
        NodeCommandFuture, NodeCommandHandler, NodeCommandRequest, NodeCommandResult,
        NodeCreateNetworkCommand, NodeCreatedNetwork,
    };

    #[derive(Clone)]
    struct TestHandler {
        write_count: Arc<AtomicUsize>,
        invitation_ticket: String,
    }

    impl NodeCommandHandler for TestHandler {
        fn handle(&self, request: NodeCommandRequest) -> NodeCommandFuture {
            let write_count = self.write_count.clone();
            let invitation_ticket = self.invitation_ticket.clone();
            Box::pin(async move {
                let request_id = request.request_id;
                write_count.fetch_add(1, Ordering::SeqCst);
                match request.command {
                    NodeCommand::CreateNetwork(command) => NodeCommandResult::completed(
                        request_id,
                        NodeCommandOutcome::NetworkCreated(NodeCreatedNetwork {
                            network_name: command.name,
                            config_saved: true,
                            started: true,
                            invitation_ticket,
                        }),
                    ),
                    _ => NodeCommandResult::completed(
                        request_id,
                        NodeCommandOutcome::Completed {
                            message: "completed".into(),
                        },
                    ),
                }
            })
        }
    }

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "fuckipv6-operation-service-{label}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system clock before Unix epoch")
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).expect("temporary operation directory");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                    .expect("private temporary operation directory");
            }
            Self { path }
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn service_fixture(
        store_path: PathBuf,
        invitation_ticket: &str,
    ) -> (OperationService, NodeStateStore, Arc<AtomicUsize>) {
        let events = NodeEventBus::new(64);
        let state = NodeStateStore::with_events(events.clone());
        let write_count = Arc::new(AtomicUsize::new(0));
        let commands = NodeCommandBus::spawn(
            Arc::new(TestHandler {
                write_count: write_count.clone(),
                invitation_ticket: invitation_ticket.into(),
            }),
            events.clone(),
            8,
        );
        let service =
            OperationService::with_store_path(commands, state.clone(), events, store_path)
                .expect("operation service fixture");
        (service, state, write_count)
    }

    async fn context(state: &NodeStateStore) -> NodeCommandContext {
        NodeCommandContext::local_owner_for_snapshot(&state.snapshot().await, "operation-test")
    }

    async fn wait_for_terminal(
        service: &OperationService,
        operation_id: &str,
    ) -> NodeOperationSnapshot {
        for _ in 0..500 {
            let operation = service.lookup(operation_id).expect("operation exists");
            if operation.operation.state.is_terminal() {
                return operation;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("operation {operation_id} did not reach a terminal state");
    }

    fn create_network_request(request_id: u64, name: &str) -> NodeCommandRequest {
        NodeCommandRequest::new(
            request_id,
            NodeCommand::CreateNetwork(NodeCreateNetworkCommand {
                name: name.into(),
                signaling_port: 6666,
                relay_port: 6667,
                external_host: None,
                auto_connect: false,
            }),
        )
    }

    #[test]
    fn tun_rebuild_uses_a_network_scoped_operation() {
        let local_node = NodeId::from_public_key(b"operation-service-local-node");
        let command = NodeCommand::RebuildTun {
            network_name: "home".into(),
        };

        assert_eq!(operation_kind(&command), OperationKind::TunRebuild);
        assert_eq!(
            operation_target(&command, &local_node),
            DomainObjectRef {
                kind: ObjectKind::Network,
                id: "home".into(),
            }
        );
    }

    #[tokio::test]
    async fn first_submission_is_persisted_and_idempotent_reuse_dispatches_once() {
        let directory = TestDirectory::new("idempotent");
        let store_path = directory.path.join(STORE_FILE);
        let (service, state, write_count) = service_fixture(store_path.clone(), "ticket");
        let context = context(&state).await;
        let key = IdempotencyKey::from_public_key(b"idempotent-submit");
        let request = create_network_request(10, "idempotent-network");

        let (first, created) = service
            .submit(context.clone(), request.clone(), key.clone())
            .await
            .expect("first operation submission");
        assert!(created);
        assert_eq!(first.operation.state, OperationState::Queued);
        assert!(store_path.exists(), "queued operation must be durable");

        let (reused, created) = service
            .submit(context, request, key)
            .await
            .expect("idempotent operation reuse");
        assert!(!created);
        assert_eq!(reused.operation.operation_id, first.operation.operation_id);

        let terminal = wait_for_terminal(&service, first.operation.operation_id.as_str()).await;
        assert_eq!(terminal.operation.state, OperationState::Succeeded);
        assert_eq!(write_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn result_is_available_in_memory_but_never_written_to_durable_state() {
        let directory = TestDirectory::new("ephemeral-result");
        let store_path = directory.path.join(STORE_FILE);
        let secret = "fk://owner-secret-invitation@example.test:6666";
        let (service, state, _) = service_fixture(store_path.clone(), secret);
        let request = create_network_request(11, "result-network");
        let (submitted, _) = service
            .submit(
                context(&state).await,
                request,
                IdempotencyKey::from_public_key(b"result-submission"),
            )
            .await
            .expect("operation submission");

        let terminal = wait_for_terminal(&service, submitted.operation.operation_id.as_str()).await;
        assert_eq!(terminal.operation.state, OperationState::Succeeded);
        assert!(matches!(
            terminal.result.as_ref().map(|result| &result.outcome),
            Some(NodeCommandOutcome::NetworkCreated(created)) if created.invitation_ticket == secret
        ));

        let persisted = std::fs::read_to_string(&store_path).expect("read operation store");
        assert!(persisted.contains(submitted.operation.operation_id.as_str()));
        assert!(!persisted.contains(secret));
        assert!(!persisted.contains("\"result\""));

        let (reloaded, _, _) = service_fixture(store_path, secret);
        let recovered = reloaded
            .lookup(submitted.operation.operation_id.as_str())
            .expect("terminal operation survives process restart");
        assert_eq!(recovered.operation.state, OperationState::Succeeded);
        assert!(recovered.result.is_none());
    }
}
