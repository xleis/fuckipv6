use std::time::Duration;

use anyhow::{Context, Result};
use tokio::runtime::{Builder, Runtime};

use crate::cli::commands::RuntimeOptions;
use crate::multi_network::MultiNetworkSupervisor;
use crate::node::{
    NodeCommand, NodeCommandBus, NodeCommandRequest, NodeCommandResult, NodeStateStore,
};

/// Owns the live node runtime used by the native window.
///
/// The GPUI event loop must run on the process main thread. Node actors and
/// Tokio-backed services therefore live on this dedicated multi-thread runtime;
/// the view only receives the shared state store and command bus handles.
pub struct NativeNodeSession {
    runtime: Option<Runtime>,
    supervisor: Option<MultiNetworkSupervisor>,
    state: NodeStateStore,
    commands: NodeCommandBus,
}

impl NativeNodeSession {
    pub fn start(options: RuntimeOptions) -> Result<Self> {
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("fuckipv6-gpui-node")
            .enable_all()
            .build()
            .context("无法创建 GPUI 节点运行时")?;

        let supervisor = {
            let _entered = runtime.enter();
            MultiNetworkSupervisor::new(options).context("无法初始化 GPUI 节点命令总线")?
        };
        let state = supervisor.state().clone();
        let commands = supervisor.commands();

        Ok(Self {
            runtime: Some(runtime),
            supervisor: Some(supervisor),
            state,
            commands,
        })
    }

    pub fn state(&self) -> NodeStateStore {
        self.state.clone()
    }

    pub fn commands(&self) -> NodeCommandBus {
        self.commands.clone()
    }

    /// Submit the shared shutdown command, then tear down the dedicated runtime
    /// even if a backend operation fails to finish promptly.
    pub fn shutdown(mut self) {
        self.shutdown_inner();
    }

    fn shutdown_inner(&mut self) {
        let Some(runtime) = self.runtime.take() else {
            return;
        };

        let commands = self.commands.clone();
        let shutdown_result = runtime.block_on(async {
            tokio::time::timeout(
                Duration::from_secs(2),
                commands.execute(NodeCommandRequest::new(u64::MAX, NodeCommand::Shutdown)),
            )
            .await
        });

        match shutdown_result {
            Ok(Ok(NodeCommandResult { outcome, .. })) => {
                tracing::info!("GPUI 退出清理已提交: {outcome:?}");
            }
            Ok(Err(error)) => tracing::warn!("GPUI 退出命令未完成: {error}"),
            Err(_) => tracing::warn!("GPUI 退出命令超时，将强制回收节点运行时"),
        }

        drop(self.supervisor.take());
        runtime.shutdown_timeout(Duration::from_secs(2));
    }
}

impl Drop for NativeNodeSession {
    fn drop(&mut self) {
        // Window-close and unwind paths both converge here. The explicit
        // timeout prevents a failed node actor from keeping the UI process alive.
        self.shutdown_inner();
    }
}
