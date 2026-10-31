//! Long-running authenticated IPC service for the node control boundary.
//!
//! The transport in [`crate::local_ipc`] owns framing, peer authentication and
//! timeouts. This module owns connection lifecycle and delegates every command
//! to the same [`NodeCommandBus`] used by CLI, GPUI and WebUI adapters.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use serde_json::Value;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinSet;
use tokio::time::{sleep, timeout};
use tracing::{debug, warn};

use crate::local_ipc::{
    EndpointRole, FrameKind, IpcErrorCode, LocalConnection, LocalError, LocalEvent, LocalRequest,
    LocalResponse, LocalServer,
};
use crate::node::{NodeCommandBus, NodeCommandBusError, NodeEventBus, NodeStateStore};

/// Commands may perform persistence and runtime transitions, so they get a
/// larger budget than individual frame I/O operations.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CONNECTIONS: usize = 64;
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(50);

/// Serve authenticated node commands and ordered events until `shutdown`.
///
/// Shutdown stops accepting, asks connection tasks to exit, waits briefly for
/// graceful completion, then aborts remaining I/O. The [`LocalServer`] guard is
/// dropped only after all connection tasks release it, so the Unix socket is
/// removed on both normal shutdown and task failure.
pub async fn serve_node(
    server: LocalServer,
    commands: NodeCommandBus,
    state: NodeStateStore,
    events: NodeEventBus,
    shutdown: watch::Receiver<bool>,
) -> Result<()> {
    serve_node_inner(server, commands, state, events, shutdown, None).await
}

/// Serve a node whose lifecycle is owned by this process. Unlike the generic
/// [`serve_node`] entrypoint, this variant can honor `StopNode`: it acknowledges
/// the request only after arming the same shutdown channel that tears down the
/// listener and all active subscriptions.
pub async fn serve_owned_node(
    server: LocalServer,
    commands: NodeCommandBus,
    state: NodeStateStore,
    events: NodeEventBus,
    shutdown: watch::Sender<bool>,
) -> Result<()> {
    let shutdown_rx = shutdown.subscribe();
    serve_node_inner(server, commands, state, events, shutdown_rx, Some(shutdown)).await
}

async fn serve_node_inner(
    server: LocalServer,
    commands: NodeCommandBus,
    state: NodeStateStore,
    events: NodeEventBus,
    mut shutdown: watch::Receiver<bool>,
    stop_controller: Option<watch::Sender<bool>>,
) -> Result<()> {
    if server.role() != EndpointRole::Node {
        bail!("节点命令服务只能绑定 node IPC endpoint");
    }

    let server = Arc::new(server);
    let mut connections = JoinSet::new();

    loop {
        tokio::select! {
            biased;
            _ = wait_for_shutdown(&mut shutdown) => break,
            Some(result) = connections.join_next(), if !connections.is_empty() => {
                log_connection_result(result);
            }
            accepted = server.accept_raw(), if connections.len() < MAX_CONNECTIONS => {
                match accepted {
                    Ok(connection) => {
                        let task_server = Arc::clone(&server);
                        let task_commands = commands.clone();
                        let task_state = state.clone();
                        let task_events = events.clone();
                        let task_shutdown = shutdown.clone();
                        let task_stop_controller = stop_controller.clone();
                        connections.spawn(async move {
                            let mut connection = task_server.finish_handshake(connection).await?;
                            debug!(
                                peer_euid = connection.peer().euid,
                                peer_pid = connection.peer().pid,
                                "本地 IPC 已认证"
                            );
                            serve_connection(
                                &mut connection,
                                task_commands,
                                task_state,
                                task_events,
                                task_shutdown,
                                task_stop_controller,
                            )
                            .await
                        });
                    }
                    Err(error) => {
                        warn!(%error, "接受本地 IPC 连接失败");
                        tokio::select! {
                            _ = wait_for_shutdown(&mut shutdown) => break,
                            _ = sleep(ACCEPT_ERROR_BACKOFF) => {}
                        }
                    }
                }
            }
        }
    }

    // Stop accepting before draining peers. Connection tasks keep their own
    // Arc until they observe shutdown, so the endpoint is removed only after
    // all users of the listener have exited.
    drop(server);
    let drain = async {
        while let Some(result) = connections.join_next().await {
            log_connection_result(result);
        }
    };
    if timeout(SHUTDOWN_GRACE, drain).await.is_err() {
        warn!("本地 IPC 关闭超时，正在中止剩余连接");
        connections.abort_all();
        while let Some(result) = connections.join_next().await {
            if let Err(error) = result {
                if !error.is_cancelled() {
                    warn!(%error, "中止本地 IPC 连接任务失败");
                }
            }
        }
    }

    debug!("本地 IPC 服务已关闭");
    Ok(())
}

async fn wait_for_shutdown(shutdown: &mut watch::Receiver<bool>) {
    if *shutdown.borrow() {
        return;
    }
    loop {
        match shutdown.changed().await {
            Ok(()) if *shutdown.borrow() => return,
            Ok(()) => {}
            // Losing the service owner is itself a shutdown condition. Leaving
            // the listener alive without a supervisor would create an orphan.
            Err(_) => return,
        }
    }
}

async fn serve_connection(
    connection: &mut LocalConnection,
    commands: NodeCommandBus,
    state: NodeStateStore,
    events: NodeEventBus,
    mut shutdown: watch::Receiver<bool>,
    stop_controller: Option<watch::Sender<bool>>,
) -> Result<()> {
    loop {
        let (kind, payload) = tokio::select! {
            _ = wait_for_shutdown(&mut shutdown) => return Ok(()),
            received = connection.receive::<Value>() => received?,
        };

        let request: LocalRequest = match kind {
            FrameKind::Request => match serde_json::from_value(payload) {
                Ok(request) => request,
                Err(error) => {
                    send_local_error(
                        connection,
                        LocalError::new(
                            IpcErrorCode::MalformedRequest,
                            format!("无法解析本地 IPC 请求: {error}"),
                        ),
                    )
                    .await?;
                    continue;
                }
            },
            FrameKind::Ping => {
                connection.send(FrameKind::Pong, &()).await?;
                continue;
            }
            other => {
                send_local_error(
                    connection,
                    LocalError::new(
                        IpcErrorCode::MalformedRequest,
                        format!("本地 IPC 服务收到意外帧: {other:?}"),
                    ),
                )
                .await?;
                continue;
            }
        };

        match request.command {
            crate::local_ipc::LocalCommand::Snapshot => {
                // Capture the boundary before the snapshot. If a state update
                // races this read, its event remains after the returned
                // boundary and the client can safely apply it later.
                let event_sequence = events.latest_sequence();
                let snapshot = state.snapshot().await;
                connection
                    .send_response(&LocalResponse::Snapshot {
                        snapshot: Box::new(snapshot),
                        event_sequence,
                    })
                    .await?;
            }
            crate::local_ipc::LocalCommand::Execute(command_request) => {
                if request.request_id != command_request.request_id {
                    send_local_error(
                        connection,
                        LocalError::new(
                            IpcErrorCode::RequestIdMismatch,
                            format!(
                                "本地 IPC 外层 request_id={} 与节点命令 request_id={} 不一致",
                                request.request_id, command_request.request_id
                            ),
                        ),
                    )
                    .await?;
                    continue;
                }

                let result = match timeout(COMMAND_TIMEOUT, commands.execute(command_request)).await
                {
                    Ok(Ok(result)) => result,
                    Ok(Err(error)) => {
                        send_local_error(connection, command_bus_error(error)).await?;
                        continue;
                    }
                    Err(_) => {
                        send_local_error(
                            connection,
                            LocalError::new(
                                IpcErrorCode::Timeout,
                                format!("节点命令在 {} 秒内未完成", COMMAND_TIMEOUT.as_secs()),
                            )
                            .retryable(true),
                        )
                        .await?;
                        continue;
                    }
                };

                if result.request_id != request.request_id {
                    send_local_error(
                        connection,
                        LocalError::new(
                            IpcErrorCode::RequestIdMismatch,
                            format!(
                                "节点命令响应 request_id={} 与请求 request_id={} 不一致",
                                result.request_id, request.request_id
                            ),
                        ),
                    )
                    .await?;
                    continue;
                }
                connection
                    .send_response(&LocalResponse::NodeCommand(result))
                    .await?;
            }
            crate::local_ipc::LocalCommand::Subscribe { after_sequence } => {
                stream_events(
                    connection,
                    state,
                    events,
                    shutdown.clone(),
                    request.request_id,
                    after_sequence,
                )
                .await?;
                return Ok(());
            }
            crate::local_ipc::LocalCommand::StopNode => match &stop_controller {
                Some(controller) => {
                    // The response is written before the service observes the
                    // shutdown channel. This lets the control process distinguish
                    // an acknowledged stop from a transport failure.
                    connection
                        .send_response(&LocalResponse::Accepted {
                            message: "节点正在停止".into(),
                        })
                        .await?;
                    let _ = controller.send(true);
                }
                None => {
                    // Standalone adapters that do not own a process lifetime must
                    // not acknowledge a no-op.
                    send_local_error(
                        connection,
                        LocalError::new(
                            IpcErrorCode::Unsupported,
                            "当前服务没有节点进程生命周期控制权",
                        ),
                    )
                    .await?;
                }
            },
        }
    }
}

async fn stream_events(
    connection: &mut LocalConnection,
    state: NodeStateStore,
    events: NodeEventBus,
    mut shutdown: watch::Receiver<bool>,
    request_id: u64,
    after_sequence: Option<u64>,
) -> Result<()> {
    // Subscription and boundary capture are atomic with respect to publish.
    // The snapshot is authoritative; events at or before the boundary are
    // intentionally not replayed because they are already represented by it.
    let (mut receiver, event_sequence) = events.subscribe_with_sequence();
    let snapshot = state.snapshot().await;
    if let Some(requested) = after_sequence {
        if requested != event_sequence {
            debug!(
                request_id,
                requested, event_sequence, "本地 IPC 订阅以权威快照重新同步事件边界"
            );
        }
    }
    connection
        .send_response(&LocalResponse::Subscribed {
            snapshot: Box::new(snapshot),
            event_sequence,
        })
        .await?;

    let mut cursor = event_sequence;
    loop {
        let event = tokio::select! {
            _ = wait_for_shutdown(&mut shutdown) => return Ok(()),
            received = receiver.recv() => received,
        };
        match event {
            Ok(event) => {
                if event.sequence <= cursor {
                    continue;
                }
                if cursor.checked_add(1) != Some(event.sequence) {
                    send_local_error(
                        connection,
                        LocalError::new(
                            IpcErrorCode::EventGap,
                            format!(
                                "本地 IPC 事件序号不连续: expected={}, actual={}",
                                cursor.saturating_add(1),
                                event.sequence
                            ),
                        )
                        .retryable(true),
                    )
                    .await?;
                    return Ok(());
                }
                cursor = event.sequence;
                connection.send_event(&LocalEvent { event }).await?;
            }
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                send_local_error(
                    connection,
                    LocalError::new(
                        IpcErrorCode::EventGap,
                        format!("本地 IPC 事件缓冲溢出，丢弃 {skipped} 条事件"),
                    )
                    .retryable(true),
                )
                .await?;
                return Ok(());
            }
            Err(broadcast::error::RecvError::Closed) => {
                send_local_error(
                    connection,
                    LocalError::new(IpcErrorCode::BackendUnavailable, "节点事件总线已关闭")
                        .retryable(true),
                )
                .await?;
                return Ok(());
            }
        }
    }
}

async fn send_local_error(connection: &mut LocalConnection, error: LocalError) -> Result<()> {
    connection.send_error(&error).await
}

fn command_bus_error(error: NodeCommandBusError) -> LocalError {
    match error {
        NodeCommandBusError::Invalid(problem) => {
            LocalError::new(IpcErrorCode::MalformedRequest, problem.message)
        }
        NodeCommandBusError::Closed => {
            LocalError::new(IpcErrorCode::BackendUnavailable, "节点命令总线已关闭").retryable(true)
        }
        NodeCommandBusError::ResponseDropped(request_id) => LocalError::new(
            IpcErrorCode::BackendUnavailable,
            format!("节点命令 {request_id} 的结果通道已关闭"),
        )
        .retryable(true),
    }
}

fn log_connection_result(result: std::result::Result<Result<()>, tokio::task::JoinError>) {
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => debug!(%error, "本地 IPC 连接已结束"),
        Err(error) if error.is_cancelled() => {}
        Err(error) => warn!(%error, "本地 IPC 连接任务异常退出"),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    use anyhow::Context;
    use tokio::sync::watch;

    use crate::local_ipc::{connect, execute, request, IpcRole, LocalEndpoint, LocalRequest};
    use crate::node::{
        NodeCommand, NodeCommandFuture, NodeCommandHandler, NodeCommandOutcome, NodeCommandRequest,
        NodeCommandResult, NodeEventKind, NodeLogLevel,
    };

    struct EchoHandler;

    impl NodeCommandHandler for EchoHandler {
        fn handle(&self, request: NodeCommandRequest) -> NodeCommandFuture {
            Box::pin(async move {
                NodeCommandResult::completed(
                    request.request_id,
                    NodeCommandOutcome::Completed {
                        message: "echo".into(),
                    },
                )
            })
        }
    }

    struct Fixture {
        endpoint: LocalEndpoint,
        directory: PathBuf,
        state: NodeStateStore,
        events: NodeEventBus,
        shutdown: watch::Sender<bool>,
        service: tokio::task::JoinHandle<Result<()>>,
    }

    impl Fixture {
        async fn new(label: &str, event_capacity: usize) -> Result<Self> {
            let directory = std::env::temp_dir().join(format!(
                "fuckipv6-local-service-{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("system clock before Unix epoch")
                    .as_nanos()
            ));
            std::fs::create_dir_all(&directory)?;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;

            let endpoint = LocalEndpoint::Path(directory.join("node.sock"));
            let events = NodeEventBus::new(event_capacity);
            let state = NodeStateStore::with_events(events.clone());
            let commands = NodeCommandBus::spawn(Arc::new(EchoHandler), events.clone(), 8);
            let server = LocalServer::bind(endpoint.clone(), EndpointRole::Node).await?;
            let (shutdown, shutdown_rx) = watch::channel(false);
            let service = tokio::spawn(serve_node(
                server,
                commands,
                state.clone(),
                events.clone(),
                shutdown_rx,
            ));
            Ok(Self {
                endpoint,
                directory,
                state,
                events,
                shutdown,
                service,
            })
        }

        async fn finish(self) -> Result<()> {
            let socket_path = match &self.endpoint {
                LocalEndpoint::Path(path) => path.clone(),
            };
            self.shutdown
                .send(true)
                .context("service already stopped")?;
            timeout(Duration::from_secs(5), self.service)
                .await
                .context("local service shutdown timed out")???;
            assert!(
                !socket_path.exists(),
                "shutdown must remove the Unix socket"
            );
            std::fs::remove_dir_all(&self.directory)?;
            Ok(())
        }
    }

    fn socket_path(endpoint: &LocalEndpoint) -> &Path {
        match endpoint {
            LocalEndpoint::Path(path) => path,
        }
    }

    #[tokio::test]
    async fn serves_snapshot_and_executes_shared_commands() -> Result<()> {
        let fixture = Fixture::new("requests", 8).await?;
        let snapshot = fixture.state.snapshot().await;

        let response = request(
            &fixture.endpoint,
            IpcRole::Cli,
            &LocalRequest {
                request_id: 1,
                command: crate::local_ipc::LocalCommand::Snapshot,
            },
        )
        .await?;
        match response {
            LocalResponse::Snapshot {
                snapshot: received,
                event_sequence,
            } => {
                assert_eq!(received.revision, snapshot.revision);
                assert_eq!(event_sequence, 0);
            }
            other => panic!("unexpected snapshot response: {other:?}"),
        }

        let result = execute(
            &fixture.endpoint,
            IpcRole::Gpui,
            NodeCommandRequest::new(7, NodeCommand::GetSnapshot),
        )
        .await?;
        assert_eq!(result.request_id, 7);
        assert!(matches!(
            result.outcome,
            NodeCommandOutcome::Completed { ref message } if message == "echo"
        ));

        fixture.finish().await
    }

    #[tokio::test]
    async fn rejects_request_id_mismatch_without_losing_service() -> Result<()> {
        let fixture = Fixture::new("request-id", 8).await?;
        let error = request(
            &fixture.endpoint,
            IpcRole::Cli,
            &LocalRequest {
                request_id: 10,
                command: crate::local_ipc::LocalCommand::Execute(NodeCommandRequest::new(
                    11,
                    NodeCommand::GetSnapshot,
                )),
            },
        )
        .await
        .expect_err("mismatched ids must fail");
        assert!(error.to_string().contains("request_id"));

        let response = request(
            &fixture.endpoint,
            IpcRole::Cli,
            &LocalRequest {
                request_id: 12,
                command: crate::local_ipc::LocalCommand::Snapshot,
            },
        )
        .await?;
        assert!(matches!(response, LocalResponse::Snapshot { .. }));

        fixture.finish().await
    }

    #[tokio::test]
    async fn subscription_streams_only_events_after_the_snapshot_boundary() -> Result<()> {
        let fixture = Fixture::new("events", 8).await?;
        let before_subscription = fixture
            .events
            .publish(NodeEventKind::StateChanged { revision: 0 });
        let mut connection = connect(&fixture.endpoint, IpcRole::Gpui).await?;
        connection
            .send(
                FrameKind::Request,
                &LocalRequest {
                    request_id: 20,
                    command: crate::local_ipc::LocalCommand::Subscribe {
                        after_sequence: Some(0),
                    },
                },
            )
            .await?;
        let (kind, response): (FrameKind, LocalResponse) = connection.receive().await?;
        assert_eq!(kind, FrameKind::Response);
        let boundary = match response {
            LocalResponse::Subscribed { event_sequence, .. } => event_sequence,
            other => panic!("unexpected subscribe response: {other:?}"),
        };
        assert_eq!(boundary, before_subscription.sequence);

        let first = fixture.events.publish(NodeEventKind::LogAppended {
            level: NodeLogLevel::Info,
            message: "first".into(),
        });
        let second = fixture
            .events
            .publish(NodeEventKind::StateChanged { revision: 1 });

        let (kind, event): (FrameKind, LocalEvent) = connection.receive().await?;
        assert_eq!(kind, FrameKind::Event);
        assert_eq!(event.event.sequence, first.sequence);
        assert_eq!(event.event.sequence, boundary + 1);
        let (_, event): (FrameKind, LocalEvent) = connection.receive().await?;
        assert_eq!(event.event.sequence, second.sequence);

        drop(connection);
        fixture.finish().await
    }

    #[tokio::test]
    async fn shutdown_closes_active_subscription_and_removes_socket() -> Result<()> {
        let fixture = Fixture::new("shutdown-subscription", 8).await?;
        let mut connection = connect(&fixture.endpoint, IpcRole::Gpui).await?;
        connection
            .send(
                FrameKind::Request,
                &LocalRequest {
                    request_id: 25,
                    command: crate::local_ipc::LocalCommand::Subscribe {
                        after_sequence: None,
                    },
                },
            )
            .await?;
        let (_, response): (FrameKind, LocalResponse) = connection.receive().await?;
        assert!(matches!(response, LocalResponse::Subscribed { .. }));

        fixture.finish().await?;
        assert!(
            connection.receive::<Value>().await.is_err(),
            "shutdown must close an active subscription"
        );
        Ok(())
    }

    #[tokio::test]
    async fn event_buffer_lag_closes_stream_with_typed_gap_error() -> Result<()> {
        let fixture = Fixture::new("event-gap", 1).await?;
        let mut connection = connect(&fixture.endpoint, IpcRole::Gpui).await?;
        connection
            .send(
                FrameKind::Request,
                &LocalRequest {
                    request_id: 30,
                    command: crate::local_ipc::LocalCommand::Subscribe {
                        after_sequence: None,
                    },
                },
            )
            .await?;
        let (_, response): (FrameKind, LocalResponse) = connection.receive().await?;
        assert!(matches!(response, LocalResponse::Subscribed { .. }));

        for index in 0..128 {
            fixture
                .events
                .publish(NodeEventKind::StateChanged { revision: index });
        }

        let mut saw_gap = false;
        for _ in 0..129 {
            let (kind, payload): (FrameKind, Value) = connection.receive().await?;
            match kind {
                FrameKind::Event => continue,
                FrameKind::Error => {
                    let error: LocalError = serde_json::from_value(payload)?;
                    assert_eq!(error.code, IpcErrorCode::EventGap);
                    assert!(error.retryable);
                    saw_gap = true;
                    break;
                }
                other => panic!("unexpected event stream frame: {other:?}"),
            }
        }
        assert!(saw_gap, "overflowed subscription must report an event gap");

        drop(connection);
        fixture.finish().await
    }

    #[tokio::test]
    async fn stop_node_is_typed_unsupported_without_process_ownership() -> Result<()> {
        let fixture = Fixture::new("stop-node", 8).await?;
        let error = request(
            &fixture.endpoint,
            IpcRole::Cli,
            &LocalRequest {
                request_id: 40,
                command: crate::local_ipc::LocalCommand::StopNode,
            },
        )
        .await
        .expect_err("StopNode must not acknowledge a no-op");
        assert!(error.to_string().contains("生命周期控制权"));
        assert!(
            socket_path(&fixture.endpoint).exists(),
            "control service must remain live"
        );

        fixture.finish().await
    }

    #[tokio::test]
    async fn owned_stop_node_acknowledges_and_stops_the_service() -> Result<()> {
        let directory = std::env::temp_dir().join(format!(
            "fuckipv6-owned-local-service-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock before Unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory)?;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        let endpoint = LocalEndpoint::Path(directory.join("node.sock"));
        let events = NodeEventBus::new(8);
        let state = NodeStateStore::with_events(events.clone());
        let commands = NodeCommandBus::spawn(Arc::new(EchoHandler), events.clone(), 8);
        let server = LocalServer::bind(endpoint.clone(), EndpointRole::Node).await?;
        let (shutdown_tx, mut stop_rx) = watch::channel(false);
        let service = tokio::spawn(serve_owned_node(
            server,
            commands,
            state,
            events,
            shutdown_tx.clone(),
        ));

        let response = request(
            &endpoint,
            IpcRole::Cli,
            &LocalRequest {
                request_id: 41,
                command: crate::local_ipc::LocalCommand::StopNode,
            },
        )
        .await?;
        assert!(matches!(response, LocalResponse::Accepted { .. }));
        stop_rx.changed().await?;
        assert!(*stop_rx.borrow());
        timeout(Duration::from_secs(15), service)
            .await
            .context("owned local service shutdown timed out")???;
        assert!(!socket_path(&endpoint).exists());
        drop(shutdown_tx);
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }
}
