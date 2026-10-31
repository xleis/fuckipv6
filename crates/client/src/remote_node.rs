//! UI-side adapter for the long-running node process.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::Value;
use tokio::runtime::{Builder, Runtime};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};

use crate::cli::commands::RuntimeOptions;
use crate::local_ipc::{
    self, EndpointRole, FrameKind, IpcErrorCode, IpcRole, LocalCommand, LocalError, LocalEvent,
    LocalRequest, LocalResponse,
};
use crate::node::{
    NodeCommandBus, NodeCommandFuture, NodeCommandHandler, NodeCommandOutcome, NodeCommandRequest,
    NodeCommandResult, NodeControlHealth, NodeEventBus, NodeEventKind, NodeProblem,
    NodeProblemCode, NodeStateStore,
};
use crate::runtime_process;

const INITIAL_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const RECONNECT_MIN: Duration = Duration::from_millis(100);
const RECONNECT_MAX: Duration = Duration::from_secs(2);

/// UI ownership of a connection to the node, never of the node itself.
pub struct RemoteNodeSession {
    runtime: Option<Runtime>,
    state: NodeStateStore,
    commands: NodeCommandBus,
    events: NodeEventBus,
    cancel: watch::Sender<bool>,
    sync: Option<JoinHandle<()>>,
}

impl RemoteNodeSession {
    pub fn start(options: RuntimeOptions) -> Result<Self> {
        Self::start_with_role(options, IpcRole::Gpui)
    }

    /// Connect an arbitrary local UI adapter using its authenticated IPC role.
    ///
    /// The role is carried through the initial snapshot request, every command,
    /// and the event subscription so the node process can authorize each
    /// transport consistently.
    pub fn start_with_role(options: RuntimeOptions, role: IpcRole) -> Result<Self> {
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("fuckipv6-remote-node")
            .enable_all()
            .build()
            .context("无法创建 UI IPC 运行时")?;

        let (state, commands, events, cancel, sync) =
            runtime.block_on(Self::connect_parts(options, role))?;

        Ok(Self {
            runtime: Some(runtime),
            state,
            commands,
            events,
            cancel,
            sync: Some(sync),
        })
    }

    /// Connect from an existing Tokio runtime, as used by the async CLI/WebUI
    /// entrypoint. This avoids constructing and blocking on a nested runtime.
    pub async fn start_with_role_async(options: RuntimeOptions, role: IpcRole) -> Result<Self> {
        let (state, commands, events, cancel, sync) = Self::connect_parts(options, role).await?;
        Ok(Self {
            runtime: None,
            state,
            commands,
            events,
            cancel,
            sync: Some(sync),
        })
    }

    async fn connect_parts(
        options: RuntimeOptions,
        role: IpcRole,
    ) -> Result<(
        NodeStateStore,
        NodeCommandBus,
        NodeEventBus,
        watch::Sender<bool>,
        JoinHandle<()>,
    )> {
        timeout(
            INITIAL_CONNECT_TIMEOUT,
            runtime_process::ensure_control(&options),
        )
        .await
        .context("连接节点控制面超时")??;

        let endpoint = local_ipc::endpoint(EndpointRole::Node)?;
        let snapshot = request_snapshot(&endpoint, role.clone()).await?;
        let state = NodeStateStore::new();
        state.replace_authoritative(snapshot).await;

        let events = NodeEventBus::default();
        let handler = Arc::new(RemoteNodeHandler {
            endpoint: endpoint.clone(),
            role: role.clone(),
        });
        let commands = NodeCommandBus::spawn(handler, events.clone(), 32);
        let (cancel, cancel_rx) = watch::channel(false);
        let sync = tokio::spawn(sync_node_state(endpoint, role, state.clone(), cancel_rx));
        Ok((state, commands, events, cancel, sync))
    }

    pub fn state(&self) -> NodeStateStore {
        self.state.clone()
    }

    pub fn commands(&self) -> NodeCommandBus {
        self.commands.clone()
    }

    /// Event stream associated with `commands()`. UI transports use this to
    /// notify renderers after state or command revisions change.
    pub fn events(&self) -> NodeEventBus {
        self.events.clone()
    }

    /// Disconnect the UI. The control and node processes deliberately keep
    /// running, including every active network session.
    pub fn shutdown(mut self) {
        self.shutdown_inner();
    }

    fn shutdown_inner(&mut self) {
        let _ = self.cancel.send(true);
        let Some(runtime) = self.runtime.take() else {
            if let Some(sync) = self.sync.take() {
                sync.abort();
            }
            return;
        };
        if let Some(sync) = self.sync.take() {
            let _ = runtime.block_on(async { timeout(Duration::from_secs(2), sync).await });
        }
        runtime.shutdown_timeout(Duration::from_secs(2));
    }
}

impl Drop for RemoteNodeSession {
    fn drop(&mut self) {
        self.shutdown_inner();
    }
}

struct RemoteNodeHandler {
    endpoint: local_ipc::LocalEndpoint,
    role: IpcRole,
}

impl NodeCommandHandler for RemoteNodeHandler {
    fn handle(&self, request: NodeCommandRequest) -> NodeCommandFuture {
        let endpoint = self.endpoint.clone();
        let role = self.role.clone();
        Box::pin(async move {
            let request_id = request.request_id;
            match local_ipc::execute(&endpoint, role, request).await {
                Ok(result) => result,
                Err(error) => NodeCommandResult {
                    request_id,
                    outcome: NodeCommandOutcome::Rejected(remote_problem(request_id, error)),
                },
            }
        })
    }
}

fn remote_problem(request_id: u64, error: anyhow::Error) -> NodeProblem {
    if error
        .to_string()
        .contains(&format!("request_id={request_id}"))
    {
        return NodeProblem::new(NodeProblemCode::Internal, error.to_string());
    }
    let code = error
        .downcast_ref::<LocalError>()
        .map(|error| match error.code {
            IpcErrorCode::Unauthorized => NodeProblemCode::PermissionDenied,
            IpcErrorCode::VersionMismatch
            | IpcErrorCode::OversizedFrame
            | IpcErrorCode::MalformedFrame
            | IpcErrorCode::MalformedRequest
            | IpcErrorCode::RequestIdMismatch => NodeProblemCode::InvalidRequest,
            IpcErrorCode::BackendUnavailable | IpcErrorCode::EventGap | IpcErrorCode::Timeout => {
                NodeProblemCode::Unavailable
            }
            IpcErrorCode::Unsupported => NodeProblemCode::Unsupported,
            IpcErrorCode::Internal => NodeProblemCode::Internal,
        })
        .unwrap_or(NodeProblemCode::Unavailable);
    let retryable = error
        .downcast_ref::<LocalError>()
        .is_none_or(|error| error.retryable);
    NodeProblem::new(code, error.to_string()).retryable(retryable)
}

async fn request_snapshot(
    endpoint: &local_ipc::LocalEndpoint,
    role: IpcRole,
) -> Result<crate::node::NodeSnapshot> {
    match local_ipc::request(
        endpoint,
        role,
        &LocalRequest {
            request_id: 1,
            command: LocalCommand::Snapshot,
        },
    )
    .await?
    {
        LocalResponse::Snapshot { snapshot, .. } => Ok(*snapshot),
        other => bail!("节点 IPC 返回了意外快照响应: {other:?}"),
    }
}

async fn sync_node_state(
    endpoint: local_ipc::LocalEndpoint,
    role: IpcRole,
    state: NodeStateStore,
    mut cancel: watch::Receiver<bool>,
) {
    let mut backoff = RECONNECT_MIN;
    loop {
        if *cancel.borrow() {
            return;
        }
        match sync_subscription(&endpoint, role.clone(), &state, &mut cancel).await {
            Ok(()) if *cancel.borrow() => return,
            Ok(()) => {
                state
                    .set_control_health(NodeControlHealth::Unavailable)
                    .await;
            }
            Err(error) => {
                // This is a local adapter observation. The authoritative node
                // revision stays untouched, and a later snapshot proves the
                // control connection has recovered.
                state
                    .set_control_health(NodeControlHealth::Unavailable)
                    .await;
                tracing::debug!(%error, "节点事件订阅断开，将从权威快照重同步");
            }
        }

        tokio::select! {
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    return;
                }
            }
            _ = sleep(backoff) => {}
        }
        backoff = (backoff * 2).min(RECONNECT_MAX);
    }
}

async fn sync_subscription(
    endpoint: &local_ipc::LocalEndpoint,
    role: IpcRole,
    state: &NodeStateStore,
    cancel: &mut watch::Receiver<bool>,
) -> Result<()> {
    let mut connection = local_ipc::connect(endpoint, role.clone()).await?;
    connection
        .send(
            FrameKind::Request,
            &LocalRequest {
                request_id: 2,
                command: LocalCommand::Subscribe {
                    after_sequence: None,
                },
            },
        )
        .await?;

    let (kind, payload): (FrameKind, Value) = connection.receive().await?;
    let snapshot = match kind {
        FrameKind::Response => {
            match serde_json::from_value::<LocalResponse>(payload)
                .context("无法解析节点订阅响应")?
            {
                LocalResponse::Subscribed {
                    snapshot,
                    event_sequence,
                } => {
                    tracing::trace!(event_sequence, "节点事件订阅已建立");
                    *snapshot
                }
                response => bail!("节点订阅收到意外响应: {response:?}"),
            }
        }
        FrameKind::Error => {
            let error: LocalError =
                serde_json::from_value(payload).context("无法解析节点订阅错误响应")?;
            bail!(error);
        }
        kind => bail!("节点订阅收到意外帧: {kind:?}"),
    };
    state.replace_authoritative(snapshot).await;

    loop {
        let next = tokio::select! {
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    return Ok(());
                }
                continue;
            }
            next = connection.receive_next::<Value>() => next?,
        };
        let (kind, value) = next;
        match kind {
            FrameKind::Event => {
                let event: LocalEvent =
                    serde_json::from_value(value).context("无法解析节点事件")?;
                if matches!(event.event.kind, NodeEventKind::StateChanged { .. }) {
                    let snapshot = request_snapshot(endpoint, role.clone()).await?;
                    state.replace_authoritative(snapshot).await;
                }
            }
            FrameKind::Error => {
                let error: LocalError =
                    serde_json::from_value(value).context("无法解析节点事件错误")?;
                bail!(error);
            }
            FrameKind::Pong => {}
            other => bail!("节点事件流收到意外帧: {other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_remote_errors_map_to_fail_closed_problems() {
        let problem = remote_problem(
            9,
            anyhow::Error::new(
                LocalError::new(IpcErrorCode::BackendUnavailable, "node offline").retryable(true),
            ),
        );
        assert_eq!(problem.code, NodeProblemCode::Unavailable);
        assert!(problem.retryable);

        let problem = remote_problem(
            9,
            anyhow::Error::new(LocalError::new(IpcErrorCode::Unsupported, "unsupported")),
        );
        assert_eq!(problem.code, NodeProblemCode::Unsupported);
        assert!(!problem.retryable);
    }

    #[test]
    fn transport_errors_are_retryable_unavailable() {
        let problem = remote_problem(4, anyhow::anyhow!("connection reset"));
        assert_eq!(problem.code, NodeProblemCode::Unavailable);
        assert!(problem.retryable);
    }
}
