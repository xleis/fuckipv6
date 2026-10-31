//! Internal multi-process runtime roles for the single `fuckipv6` binary.
//!
//! The public CLI never exposes these process names. A desktop or future
//! service-manager entrypoint starts `__control`; control owns supervision and
//! starts `__node`; every UI connects to the node over the authenticated local
//! IPC boundary instead of constructing a second node runtime.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use tokio::process::{Child, Command as TokioCommand};
use tokio::sync::watch;
use tokio::time::{sleep, timeout, MissedTickBehavior};

use crate::cli::commands::RuntimeOptions;
use crate::local_ipc::{
    self, EndpointRole, IpcRole, LocalCommand, LocalEndpoint, LocalRequest, LocalResponse,
    LocalServer,
};
use crate::local_service::serve_owned_node;
use crate::logging;
use crate::multi_network::MultiNetworkSupervisor;
use crate::node::{NodeCommand, NodeCommandRequest};

const CONTROL_ARGUMENT: &str = "__control";
const NODE_ARGUMENT: &str = "__node";
const OPTIONS_ENV: &str = "FUCKIPV6_INTERNAL_OPTIONS";
const READY_TIMEOUT: Duration = Duration::from_secs(15);
const READY_POLL_INTERVAL: Duration = Duration::from_millis(100);
const NODE_UNHEALTHY_TIMEOUT: Duration = Duration::from_secs(20);
const NODE_STOP_TIMEOUT: Duration = Duration::from_secs(10);
const COMMAND_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);
const SERVICE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const RESTART_BACKOFF_MIN: Duration = Duration::from_millis(250);
const RESTART_BACKOFF_MAX: Duration = Duration::from_secs(5);

/// Stable process exit codes for internal roles and service managers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum InternalExitCode {
    Success = 0,
    RuntimeFailure = 1,
    Usage = 64,
    InternalSoftware = 70,
    TemporarilyUnavailable = 75,
    Configuration = 78,
    ShutdownTimeout = 124,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct InternalOptions {
    routing_mode: Option<String>,
    exit_node: Option<String>,
    is_exit_node: Option<bool>,
    webui_port: u16,
}

impl From<&RuntimeOptions> for InternalOptions {
    fn from(options: &RuntimeOptions) -> Self {
        Self {
            routing_mode: options.routing_mode.clone(),
            exit_node: options.exit_node.clone(),
            is_exit_node: options.is_exit_node,
            webui_port: options.webui_port,
        }
    }
}

impl InternalOptions {
    fn runtime_options(&self) -> RuntimeOptions {
        RuntimeOptions {
            routing_mode: self.routing_mode.clone(),
            exit_node: self.exit_node.clone(),
            is_exit_node: self.is_exit_node,
            headless: true,
            // Node never opens a desktop/web UI on its own. The control process
            // owns any public surface added in later work.
            gui: false,
            webui_port: 0,
        }
    }

    fn encode(&self) -> Result<String> {
        serde_json::to_string(self).context("无法编码内部进程参数")
    }

    fn decode(value: &str) -> Result<Self> {
        serde_json::from_str(value).context("无法解析内部进程参数")
    }
}

enum InternalRole {
    Control,
    Node,
}

/// Dispatch an internal process role before clap parses the public CLI.
///
/// Returns `None` for every normal invocation, allowing `main` to preserve the
/// existing public CLI and native-GPUI main-thread requirements.
pub fn internal_entrypoint() -> Option<i32> {
    let role = internal_role(std::env::args_os())?;
    let options = match std::env::var(OPTIONS_ENV)
        .map_err(anyhow::Error::from)
        .and_then(|encoded| InternalOptions::decode(&encoded))
    {
        Ok(options) => options,
        Err(error) => {
            eprintln!("fuckipv6 内部进程参数错误: {error:#}");
            return Some(InternalExitCode::Configuration as i32);
        }
    };

    let result = match role {
        InternalRole::Control => run_runtime(|runtime| runtime.block_on(run_control(options))),
        InternalRole::Node => run_runtime(|runtime| runtime.block_on(run_node(options))),
    };
    Some(match result {
        Ok(()) => InternalExitCode::Success as i32,
        Err(error) => {
            eprintln!("fuckipv6 内部进程失败: {error:#}");
            InternalExitCode::RuntimeFailure as i32
        }
    })
}

fn internal_role(arguments: impl IntoIterator<Item = std::ffi::OsString>) -> Option<InternalRole> {
    let role = arguments.into_iter().nth(1)?;
    match role.to_str()? {
        CONTROL_ARGUMENT => Some(InternalRole::Control),
        NODE_ARGUMENT => Some(InternalRole::Node),
        _ => None,
    }
}

fn run_runtime(run: impl FnOnce(&tokio::runtime::Runtime) -> Result<()>) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("无法创建内部进程运行时")?;
    run(&runtime)
}

async fn run_node(options: InternalOptions) -> Result<()> {
    logging::init_default()?;
    let _lock = acquire_instance_lock("node")?.context("另一个节点进程已经持有本地实例锁")?;
    // The previous node may have been SIGKILLed after binding. Holding the node
    // instance lock proves that no peer owns this endpoint, so stale Unix socket
    // files are safe to remove before binding the replacement.
    remove_stale_node_endpoint();

    let supervisor = MultiNetworkSupervisor::new(options.runtime_options())
        .context("无法初始化独立节点运行时")?;
    let endpoint = local_ipc::endpoint(EndpointRole::Node)?;
    let server = LocalServer::bind(endpoint, EndpointRole::Node)
        .await
        .context("无法启动节点 IPC 服务")?;
    let (shutdown_tx, _shutdown_rx) = watch::channel(false);
    let mut service = tokio::spawn(serve_owned_node(
        server,
        supervisor.commands(),
        supervisor.state().clone(),
        supervisor.event_bus(),
        shutdown_tx.clone(),
    ));

    println!(
        "{}",
        serde_json::json!({
            "role": "node",
            "pid": std::process::id(),
            "state": "ready",
        })
    );

    let mut signal = Box::pin(wait_for_shutdown_signal());
    let mut refresh = tokio::time::interval(Duration::from_secs(1));
    refresh.set_missed_tick_behavior(MissedTickBehavior::Skip);
    refresh.tick().await;
    let mut service_finished = false;
    let mut service_failure = None;

    loop {
        tokio::select! {
            _ = &mut signal => break,
            result = &mut service => {
                service_finished = true;
                match result {
                    Ok(Ok(())) => {
                        if !*shutdown_tx.borrow() {
                            service_failure = Some(anyhow::anyhow!(
                                "节点 IPC 服务在未收到停止请求时退出"
                            ));
                        }
                    }
                    Ok(Err(error)) => {
                        service_failure = Some(error.context("节点 IPC 服务异常退出"));
                    }
                    Err(error) => {
                        service_failure = Some(error.into());
                    }
                }
                break;
            }
            _ = refresh.tick() => supervisor.refresh_runtime_state().await,
        }
    }

    let _ = shutdown_tx.send(true);
    let commands = supervisor.commands();
    let shutdown_command =
        commands.execute(NodeCommandRequest::new(u64::MAX, NodeCommand::Shutdown));
    match timeout(COMMAND_SHUTDOWN_TIMEOUT, shutdown_command).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => tracing::warn!(%error, "节点关闭命令未完成"),
        Err(_) => tracing::warn!(
            "节点关闭命令超过 {} 秒，继续回收 IPC 服务",
            COMMAND_SHUTDOWN_TIMEOUT.as_secs()
        ),
    }
    drop(supervisor);

    if !service_finished {
        match timeout(SERVICE_SHUTDOWN_TIMEOUT, &mut service).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => return Err(error).context("节点 IPC 服务关闭失败"),
            Ok(Err(error)) => return Err(error).context("节点 IPC 服务任务关闭失败"),
            Err(_) => {
                service.abort();
                bail!("节点 IPC 服务关闭超时")
            }
        }
    }

    match service_failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

async fn run_control(options: InternalOptions) -> Result<()> {
    logging::init_default()?;
    let Some(_lock) = acquire_instance_lock("control")? else {
        // A concurrent UI launch may have won the race. It is successful only if
        // the already-running control plane actually brings up the node.
        wait_for_node_ready(READY_TIMEOUT).await?;
        return Ok(());
    };

    let mut child: Option<Child> = None;
    let mut adopted_node = false;
    let mut restart_delay = RESTART_BACKOFF_MIN;
    let mut unhealthy_since: Option<Instant> = None;
    let mut signal = Box::pin(wait_for_shutdown_signal());

    loop {
        tokio::select! {
            _ = &mut signal => break,
            _ = sleep(READY_POLL_INTERVAL) => {}
        }

        if child.is_none() && !adopted_node {
            if instance_is_running("node")? || node_ready().await {
                // The previous control may have been killed while node kept the
                // data plane alive. Recover the supervisor without restarting it.
                tracing::info!("接管现有节点进程");
                adopted_node = true;
                unhealthy_since = None;
            } else {
                child = Some(spawn_node_child(&options)?);
            }
        }

        if let Some(managed) = child.as_mut() {
            if let Some(status) = managed.try_wait().context("无法读取节点进程状态")? {
                let was_success = status.success();
                tracing::warn!(%status, "节点进程已退出");
                child = None;
                if was_success {
                    return Ok(());
                }
                sleep(restart_delay).await;
                restart_delay = (restart_delay * 2).min(RESTART_BACKOFF_MAX);
                continue;
            }
        }

        if node_ready().await {
            restart_delay = RESTART_BACKOFF_MIN;
            unhealthy_since = None;
            continue;
        }

        let unhealthy_for = unhealthy_since.get_or_insert_with(Instant::now).elapsed();
        if unhealthy_for < NODE_UNHEALTHY_TIMEOUT {
            continue;
        }

        tracing::error!(
            timeout_secs = NODE_UNHEALTHY_TIMEOUT.as_secs(),
            "节点 IPC 持续不可达，重启无响应的节点进程"
        );
        if let Some(mut managed) = child.take() {
            let _ = managed.kill().await;
            let _ = managed.wait().await;
        }
        adopted_node = false;
        unhealthy_since = None;
        sleep(restart_delay).await;
        restart_delay = (restart_delay * 2).min(RESTART_BACKOFF_MAX);
    }

    stop_managed_node(child).await;
    Ok(())
}

fn acquire_instance_lock(role: &str) -> Result<Option<File>> {
    let directory = local_ipc::runtime_directory()?;
    let path = instance_lock_path(&directory, role);
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("无法打开进程锁 {}", path.display()))?;
    match FileExt::try_lock_exclusive(&file) {
        Ok(()) => {
            file.set_len(0)
                .with_context(|| format!("无法截断进程锁 {}", path.display()))?;
            writeln!(file, "{}", std::process::id())
                .with_context(|| format!("无法写入进程锁 {}", path.display()))?;
            file.flush()
                .with_context(|| format!("无法刷新进程锁 {}", path.display()))?;
            Ok(Some(file))
        }
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(error).with_context(|| format!("无法锁定进程锁 {}", path.display())),
    }
}

fn instance_lock_path(directory: &std::path::Path, role: &str) -> PathBuf {
    directory.join(format!("{role}.lock"))
}

fn instance_is_running(role: &str) -> Result<bool> {
    let directory = local_ipc::runtime_directory()?;
    let path = instance_lock_path(&directory, role);
    let file = match OpenOptions::new().read(true).open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("无法打开进程锁 {}", path.display()));
        }
    };
    match FileExt::try_lock_exclusive(&file) {
        Ok(()) => {
            FileExt::unlock(&file)
                .with_context(|| format!("无法释放进程锁探测 {}", path.display()))?;
            Ok(false)
        }
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(true),
        Err(error) => Err(error).with_context(|| format!("无法探测进程锁 {}", path.display())),
    }
}

fn spawn_node_child(options: &InternalOptions) -> Result<Child> {
    let executable = std::env::current_exe().context("无法定位 fuckipv6 可执行文件")?;
    let mut command = TokioCommand::new(executable);
    command
        .arg(NODE_ARGUMENT)
        .env(OPTIONS_ENV, options.encode()?)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(false);
    command.spawn().context("无法启动节点进程")
}

/// Ensure one detached control process is running and has brought up the node.
/// Closing the calling UI only drops this handle; control and node keep running.
pub async fn ensure_control(options: &RuntimeOptions) -> Result<()> {
    // A live node does not imply a live supervisor: control can be killed while
    // its detached child keeps the data plane running. Always ensure the control
    // lock is held, then wait for the node endpoint it supervises or adopts.
    if !instance_is_running("control")? {
        spawn_detached_control(options)?;
    }
    wait_for_node_ready(READY_TIMEOUT).await
}

fn spawn_detached_control(options: &RuntimeOptions) -> Result<()> {
    let executable = std::env::current_exe().context("无法定位 fuckipv6 可执行文件")?;
    let encoded = InternalOptions::from(options).encode()?;
    let mut command = Command::new(executable);
    command
        .arg(CONTROL_ARGUMENT)
        .env(OPTIONS_ENV, encoded)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command.spawn().context("无法启动控制进程")?;
    Ok(())
}

async fn wait_for_node_ready(limit: Duration) -> Result<()> {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if node_ready().await {
            return Ok(());
        }
        sleep(READY_POLL_INTERVAL).await;
    }
    bail!("控制进程未能在 {} 秒内提供节点 IPC 服务", limit.as_secs())
}

async fn node_ready() -> bool {
    let Ok(endpoint) = local_ipc::endpoint(EndpointRole::Node) else {
        return false;
    };
    let request = local_ipc::request(
        &endpoint,
        IpcRole::Control,
        &LocalRequest {
            request_id: 1,
            command: LocalCommand::Snapshot,
        },
    );
    matches!(
        timeout(Duration::from_secs(1), request).await,
        Ok(Ok(LocalResponse::Snapshot { .. }))
    )
}

async fn stop_managed_node(mut child: Option<Child>) {
    let accepted = request_node_stop().await;
    if let Some(managed) = child.as_mut() {
        if accepted {
            match timeout(NODE_STOP_TIMEOUT, managed.wait()).await {
                Ok(Ok(_)) => return,
                Ok(Err(error)) => tracing::warn!(%error, "等待节点退出失败"),
                Err(_) => tracing::warn!("等待节点优雅退出超时，将强制终止"),
            }
        }
        let _ = managed.kill().await;
        let _ = managed.wait().await;
    } else if accepted {
        let deadline = Instant::now() + NODE_STOP_TIMEOUT;
        while Instant::now() < deadline {
            if !node_ready().await {
                return;
            }
            sleep(READY_POLL_INTERVAL).await;
        }
        tracing::warn!("接管节点未在期限内退出");
    }
    remove_stale_node_endpoint();
}

fn remove_stale_node_endpoint() {
    #[cfg(unix)]
    {
        if let Ok(LocalEndpoint::Path(path)) = local_ipc::endpoint(EndpointRole::Node) {
            match std::fs::remove_file(&path) {
                Ok(()) => tracing::debug!(path = %path.display(), "已清理残留节点 IPC socket"),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => tracing::warn!(
                    path = %path.display(),
                    %error,
                    "无法清理残留节点 IPC socket"
                ),
            }
        }
    }
}

async fn request_node_stop() -> bool {
    let Ok(endpoint) = local_ipc::endpoint(EndpointRole::Node) else {
        return false;
    };
    matches!(
        timeout(
            Duration::from_secs(3),
            local_ipc::request(
                &endpoint,
                IpcRole::Control,
                &LocalRequest {
                    request_id: u64::MAX,
                    command: LocalCommand::StopNode,
                },
            )
        )
        .await,
        Ok(Ok(LocalResponse::Accepted { .. }))
    )
}

async fn wait_for_shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::warn!(%error, "监听 Ctrl+C 失败");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let terminate = async {
            match signal(SignalKind::terminate()) {
                Ok(mut stream) => {
                    stream.recv().await;
                }
                Err(error) => {
                    tracing::warn!(%error, "监听 SIGTERM 失败");
                    std::future::pending::<()>().await;
                }
            }
        };
        tokio::select! {
            _ = ctrl_c => {}
            _ = terminate => {}
        }
    }
    #[cfg(not(unix))]
    {
        ctrl_c.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_role_is_recognized_only_as_the_first_argument() {
        assert!(matches!(
            internal_role([
                std::ffi::OsString::from("fuckipv6"),
                CONTROL_ARGUMENT.into()
            ]),
            Some(InternalRole::Control)
        ));
        assert!(matches!(
            internal_role([std::ffi::OsString::from("fuckipv6"), NODE_ARGUMENT.into()]),
            Some(InternalRole::Node)
        ));
        assert!(internal_role([std::ffi::OsString::from("fuckipv6"), "--webui".into()]).is_none());
        assert!(internal_role([
            std::ffi::OsString::from("fuckipv6"),
            "--flag".into(),
            NODE_ARGUMENT.into()
        ])
        .is_none());
    }

    #[test]
    fn launch_options_round_trip_without_promoting_ui_mode() {
        let runtime = RuntimeOptions {
            routing_mode: Some("global-proxy".into()),
            exit_node: Some("exit-a".into()),
            is_exit_node: Some(true),
            headless: false,
            gui: true,
            webui_port: 8080,
        };
        let encoded = InternalOptions::from(&runtime).encode().unwrap();
        let decoded = InternalOptions::decode(&encoded).unwrap();
        let node = decoded.runtime_options();
        assert_eq!(node.routing_mode.as_deref(), Some("global-proxy"));
        assert_eq!(node.exit_node.as_deref(), Some("exit-a"));
        assert_eq!(node.is_exit_node, Some(true));
        assert!(node.headless);
        assert!(!node.gui);
        assert_eq!(node.webui_port, 0);
    }

    #[test]
    fn lock_paths_are_scoped_by_internal_role() {
        let directory = std::path::Path::new("/tmp/fuckipv6-test");
        assert_eq!(
            instance_lock_path(directory, "control"),
            directory.join("control.lock")
        );
        assert_eq!(
            instance_lock_path(directory, "node"),
            directory.join("node.lock")
        );
    }
}
