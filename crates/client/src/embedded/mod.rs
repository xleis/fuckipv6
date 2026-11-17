//! In-process host for the existing authoritative node, without CLI, IPC, UI,
//! process supervision or native TUN creation.

mod platform;
#[cfg(test)]
mod tests;

pub(crate) use platform::NetworkLease;
pub(crate) use platform::PlatformError;
pub use platform::{
    EmbeddedNetwork, EmbeddedNetworkLease, EmbeddedNetworkRequest, EmbeddedPlatform,
};

use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use fs2::FileExt;
use fuckipv6_common::domain::IdempotencyKey;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::cli::commands::RuntimeOptions;
use crate::config_manager::ConfigManager;
use crate::multi_network::MultiNetworkSupervisor;
use crate::node::{
    NodeCommand, NodeCommandBusError, NodeCommandContext, NodeCommandOutcome, NodeCommandRequest,
    NodeCommandResult, NodeOperationSnapshot, NodeProblem, NodeProblemCode, NodeSnapshot,
    OperationService, OperationSubmitError,
};

/// All persistent node identity, config, audit and operation state stays under
/// this explicit root, using the desktop formats. No HOME/env fallback is used.
#[derive(Debug, Clone)]
pub struct EmbeddedNodeConfig {
    pub storage_root: PathBuf,
    /// Empty retains host DNS policy; otherwise the adapter must apply these.
    pub dns_servers: Vec<std::net::IpAddr>,
}

#[derive(Clone)]
pub(crate) struct EmbeddedContext {
    pub(crate) platform: Arc<dyn EmbeddedPlatform>,
    pub(crate) storage_root: PathBuf,
    pub(crate) dns_servers: Vec<std::net::IpAddr>,
    pub(crate) cancel: watch::Receiver<bool>,
    pub(crate) initialization_cleanup: Arc<std::sync::Mutex<Vec<JoinHandle<anyhow::Result<()>>>>>,
}

impl EmbeddedContext {
    pub(crate) async fn cancelled(&self) {
        let mut cancel = self.cancel.clone();
        let _ = cancel.wait_for(|cancelled| *cancelled).await;
    }
}

/// One trusted local host. Use execute_authorized for an already authenticated
/// non-owner principal; never deserialize a principal from an untrusted request.
/// Stop is observable and repeatable. Drop requests cleanup on the current
/// Tokio runtime; callers must await stop before shutting down that runtime.
pub struct EmbeddedNodeRuntime {
    supervisor: Arc<MultiNetworkSupervisor>,
    operations: OperationService,
    cancel: watch::Sender<bool>,
    cleanup: Option<JoinHandle<Result<(), NodeProblem>>>,
    stopped: Option<Result<(), NodeProblem>>,
    context: EmbeddedContext,
    lock: Option<Arc<File>>,
}

impl EmbeddedNodeRuntime {
    pub async fn start(
        config: EmbeddedNodeConfig,
        platform: Arc<dyn EmbeddedPlatform>,
    ) -> Result<Self, NodeProblem> {
        if !config.storage_root.is_absolute() {
            return Err(NodeProblem::new(
                NodeProblemCode::InvalidRequest,
                "Embedded storage root must be absolute",
            ));
        }
        let configs =
            ConfigManager::new_for_dir(config.storage_root.clone()).map_err(storage_problem)?;
        let lock = Arc::new(acquire_lock(
            config.storage_root.join("embedded-node.lock"),
        )?);
        let (cancel, cancel_rx) = watch::channel(false);
        let context = EmbeddedContext {
            platform,
            storage_root: config.storage_root.clone(),
            dns_servers: config.dns_servers,
            cancel: cancel_rx,
            initialization_cleanup: Default::default(),
        };
        let supervisor = Arc::new(
            MultiNetworkSupervisor::new_embedded(
                RuntimeOptions {
                    headless: true,
                    ..RuntimeOptions::default()
                },
                configs,
                context.clone(),
            )
            .map_err(storage_problem)?,
        );
        let operations = match OperationService::with_store_path(
            supervisor.commands(),
            supervisor.state().clone(),
            supervisor.event_bus(),
            config.storage_root.join("embedded-operations.json"),
        ) {
            Ok(operations) => operations,
            Err(error) => {
                cancel.send_replace(true);
                tokio::spawn(finish_cleanup(
                    supervisor.clone(),
                    None,
                    context.clone(),
                    lock.clone(),
                ))
                .await
                .map_err(storage_problem)??;
                return Err(storage_problem(error));
            }
        };
        let cleanup_supervisor = supervisor.clone();
        let cleanup_operations = operations.clone();
        let cleanup_context = context.clone();
        let cleanup_lock = lock.clone();
        let mut shutdown = cancel.subscribe();
        let cleanup = tokio::spawn(async move {
            let _ = shutdown.wait_for(|cancelled| *cancelled).await;
            finish_cleanup(
                cleanup_supervisor,
                Some(cleanup_operations),
                cleanup_context,
                cleanup_lock,
            )
            .await
        });
        let mut host = Self {
            supervisor,
            operations,
            cancel,
            cleanup: Some(cleanup),
            stopped: None,
            context,
            lock: Some(lock),
        };
        host.supervisor
            .state()
            .update(|snapshot| {
                snapshot.tun_capabilities = crate::tun::TunCapabilities {
                    schema_version: crate::tun::TUN_CAPABILITY_SCHEMA_VERSION,
                    platform: crate::tun::TunPlatform::current(),
                    supported: true,
                    can_rebuild: true,
                    reasons: Vec::new(),
                };
            })
            .await;
        let request = NodeCommandRequest::new(next_request_id(), NodeCommand::StartNode);
        let started = async {
            host.snapshot().await?;
            host.execute(request).await
        }
        .await;
        match started {
            Ok(NodeCommandOutcome::Rejected(problem)) | Err(problem) => {
                if let Err(cleanup) = host.stop().await {
                    return Err(cleanup);
                }
                Err(problem)
            }
            Ok(_) => Ok(host),
        }
    }

    /// Read through the same typed handler used by desktop adapters, so config,
    /// governance, live sessions and privacy-dependent state stay authoritative.
    pub async fn snapshot(&self) -> Result<NodeSnapshot, NodeProblem> {
        if *self.cancel.borrow() {
            return Ok(self.supervisor.state().snapshot().await);
        }
        match self
            .execute(NodeCommandRequest::new(
                next_request_id(),
                NodeCommand::GetSnapshot,
            ))
            .await?
        {
            NodeCommandOutcome::Snapshot(snapshot) => Ok(*snapshot),
            NodeCommandOutcome::Rejected(problem) => Err(problem),
            _ => Err(NodeProblem::new(
                NodeProblemCode::Internal,
                "Unexpected snapshot response",
            )),
        }
    }

    pub async fn execute(
        &self,
        request: NodeCommandRequest,
    ) -> Result<NodeCommandOutcome, NodeProblem> {
        let snapshot = self.supervisor.state().snapshot().await;
        let context = NodeCommandContext::local_owner_for_snapshot(&snapshot, "embedded");
        Ok(self.execute_authorized(context, request).await?.outcome)
    }

    pub async fn execute_authorized(
        &self,
        context: NodeCommandContext,
        request: NodeCommandRequest,
    ) -> Result<NodeCommandResult, NodeProblem> {
        self.ensure_running()?;
        if matches!(request.command, NodeCommand::GetSnapshot) {
            self.supervisor.refresh_runtime_state().await;
        }
        self.supervisor
            .commands()
            .execute(context, request)
            .await
            .map_err(bus_problem)
    }

    /// Operations retain the same durable idempotency, authorization and audit
    /// behavior as the desktop service, without constructing a local endpoint.
    pub async fn submit(
        &self,
        context: NodeCommandContext,
        request: NodeCommandRequest,
        key: IdempotencyKey,
    ) -> Result<(NodeOperationSnapshot, bool), OperationSubmitError> {
        self.ensure_running()
            .map_err(OperationSubmitError::Invalid)?;
        self.operations.submit(context, request, key).await
    }

    pub fn operation(&self, id: &str) -> Option<NodeOperationSnapshot> {
        self.operations.lookup(id)
    }

    /// Reconnect existing transport sessions. Packet devices and host network
    /// leases stay owned by the running sessions. RebuildTun/RestartNetworks
    /// explicitly stop and reopen them through the platform factory instead.
    pub async fn network_path_changed(&self) -> Result<(), NodeProblem> {
        self.ensure_running()?;
        self.supervisor.notify_network_path_changed().await
    }

    pub async fn stop(&mut self) -> Result<(), NodeProblem> {
        if let Some(result) = &self.stopped {
            return result.clone();
        }
        self.cancel.send_replace(true);
        if self.cleanup.is_none() {
            self.supervisor
                .retry_embedded_cleanup()
                .await
                .map_err(storage_problem)?;
            self.cleanup = Some(tokio::spawn(finish_cleanup(
                self.supervisor.clone(),
                Some(self.operations.clone()),
                self.context.clone(),
                self.lock
                    .as_ref()
                    .expect("active host owns storage lock")
                    .clone(),
            )));
        }
        let result = match self.cleanup.as_mut() {
            Some(task) => task.await.unwrap_or_else(|error| {
                Err(NodeProblem::new(
                    NodeProblemCode::Internal,
                    format!("Embedded cleanup task failed: {error}"),
                ))
            }),
            None => Ok(()),
        };
        self.cleanup.take();
        if result.is_ok() {
            self.stopped = Some(result.clone());
            self.lock.take();
        }
        result
    }

    fn ensure_running(&self) -> Result<(), NodeProblem> {
        if *self.cancel.borrow() {
            Err(NodeProblem::new(
                NodeProblemCode::Unavailable,
                "Embedded host is stopping or stopped",
            ))
        } else {
            Ok(())
        }
    }
}

impl Drop for EmbeddedNodeRuntime {
    fn drop(&mut self) {
        self.cancel.send_replace(true);
        if self.cleanup.is_none() && self.stopped.is_none() {
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                let supervisor = self.supervisor.clone();
                let operations = self.operations.clone();
                let context = self.context.clone();
                let lock = self
                    .lock
                    .as_ref()
                    .expect("active host owns storage lock")
                    .clone();
                runtime.spawn(async move {
                    let _ = supervisor.retry_embedded_cleanup().await;
                    finish_cleanup(supervisor, Some(operations), context, lock).await
                });
            }
        }
    }
}

async fn finish_cleanup(
    supervisor: Arc<MultiNetworkSupervisor>,
    operations: Option<OperationService>,
    context: EmbeddedContext,
    _lock: Arc<File>,
) -> Result<(), NodeProblem> {
    let mut failures = Vec::new();
    if let Err(error) = supervisor.commands().shutdown().await {
        failures.push(error.to_string());
    }
    if let Some(operations) = operations {
        if let Err(error) = operations.shutdown().await {
            failures.push(error.to_string());
        }
    }
    match supervisor.shutdown().await {
        Ok(()) => {
            if let Err(error) = supervisor.join_embedded_tasks().await {
                failures.push(error.to_string());
            }
        }
        Err(error) => failures.push(error.to_string()),
    }
    let tasks = std::mem::take(
        &mut *context
            .initialization_cleanup
            .lock()
            .expect("initialization cleanup poisoned"),
    );
    for task in tasks {
        match task.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => failures.push(error.to_string()),
            Err(error) => failures.push(error.to_string()),
        }
    }
    if let Err(problem) = context.platform.shutdown().await {
        failures.push(problem.message);
    }
    if failures.is_empty() {
        let lifecycle = supervisor.lifecycle().stopped().await;
        supervisor.state().set_lifecycle(lifecycle).await;
        Ok(())
    } else {
        let lifecycle = supervisor.lifecycle().failed().await;
        supervisor.state().set_lifecycle(lifecycle).await;
        let message = failures.join("; ");
        tracing::error!(%message, "Embedded cleanup incomplete; retain the host and retry stop");
        Err(storage_problem(message))
    }
}

fn next_request_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1 << 62);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn acquire_lock(path: PathBuf) -> Result<File, NodeProblem> {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(storage_problem)?;
    FileExt::try_lock_exclusive(&file).map_err(|error| {
        NodeProblem::new(
            if error.kind() == std::io::ErrorKind::WouldBlock {
                NodeProblemCode::Conflict
            } else {
                NodeProblemCode::Unavailable
            },
            "Embedded storage is already owned or cannot be locked",
        )
    })?;
    Ok(file)
}

pub(crate) fn storage_problem(error: impl std::fmt::Display) -> NodeProblem {
    NodeProblem::new(NodeProblemCode::Unavailable, error.to_string()).retryable(true)
}

fn bus_problem(error: NodeCommandBusError) -> NodeProblem {
    match error {
        NodeCommandBusError::Invalid(problem) => problem,
        other => storage_problem(other),
    }
}
