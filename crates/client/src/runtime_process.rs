//! Internal multi-process runtime roles for the single `fuckipv6` binary.
//!
//! The public CLI never exposes these process names. A desktop or future
//! service-manager entrypoint starts `__control`; control owns supervision and
//! starts `__node`; every UI connects to the node over the authenticated local
//! IPC boundary instead of constructing a second node runtime.

use std::fs::{File, OpenOptions};
use std::future::Future;
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
    self, EndpointRole, IpcRole, LocalCommand, LocalRequest, LocalResponse, LocalServer,
};
use crate::local_service::serve_owned_node;
use crate::logging;
use crate::multi_network::MultiNetworkSupervisor;
use crate::node::{NodeCommand, NodeCommandContext, NodeCommandRequest};

const CONTROL_ARGUMENT: &str = "__control";
const NODE_ARGUMENT: &str = "__node";
const SERVICE_ARGUMENT: &str = "__service";
#[cfg(windows)]
const WINDOWS_SERVICE_ARGUMENT: &str = "--windows-service";
const OPTIONS_ENV: &str = "FUCKIPV6_INTERNAL_OPTIONS";
const OPTIONS_FILE_ARGUMENT: &str = "--options-file";
const CONFIG_DIR_ARGUMENT: &str = "--config-dir";
const READY_TIMEOUT: Duration = Duration::from_secs(45);
const READY_POLL_INTERVAL: Duration = Duration::from_millis(100);
const NODE_UNHEALTHY_TIMEOUT: Duration = Duration::from_secs(20);
const NODE_IPC_BIND_TIMEOUT: Duration = Duration::from_secs(5);
const NODE_STATE_REFRESH_TIMEOUT: Duration = Duration::from_secs(5);
const REMOTE_SESSION_START_TIMEOUT: Duration = Duration::from_secs(15);
const WEBUI_START_TIMEOUT: Duration = Duration::from_secs(15);
// The node first drains data-plane work, then drains its IPC service. Keep the
// supervisor's grace period longer than both inner deadlines so it does not
// SIGKILL a node that is still making legitimate shutdown progress.
const NODE_STOP_TIMEOUT: Duration = Duration::from_secs(45);
const NODE_STARTUP_TIMEOUT: Duration = Duration::from_secs(35);
const COMMAND_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);
const SERVICE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const SUPERVISOR_STOP_TIMEOUT: Duration = Duration::from_secs(60);
const FORCE_STOP_TIMEOUT: Duration = Duration::from_secs(5);
const TASK_ABORT_TIMEOUT: Duration = Duration::from_secs(5);
const RESTART_BACKOFF_MIN: Duration = Duration::from_millis(250);
const RESTART_BACKOFF_MAX: Duration = Duration::from_secs(5);

/// Shutdown listeners are registered before any long-running service becomes
/// visible to callers. In particular, binding the WebUI socket before polling
/// `ctrl_c()` creates a window where the OS default action still terminates the
/// process for SIGTERM instead of allowing graceful cleanup.
pub(crate) struct ShutdownSignal {
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
    #[cfg(windows)]
    ctrl_c: tokio::signal::windows::CtrlC,
}

impl ShutdownSignal {
    pub(crate) fn register() -> Result<Self> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            Ok(Self {
                interrupt: signal(SignalKind::interrupt()).context("注册 SIGINT 处理器失败")?,
                terminate: signal(SignalKind::terminate()).context("注册 SIGTERM 处理器失败")?,
            })
        }
        #[cfg(windows)]
        {
            Ok(Self {
                ctrl_c: tokio::signal::windows::ctrl_c().context("注册 Ctrl+C 处理器失败")?,
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            Ok(Self {})
        }
    }

    pub(crate) async fn wait(mut self) {
        #[cfg(unix)]
        {
            tokio::select! {
                _ = self.interrupt.recv() => {}
                _ = self.terminate.recv() => {}
            }
        }
        #[cfg(windows)]
        {
            let _ = self.ctrl_c.recv().await;
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}

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
    ui_mode: Option<crate::ui::UiModeResolution>,
}

impl From<&RuntimeOptions> for InternalOptions {
    fn from(options: &RuntimeOptions) -> Self {
        Self {
            routing_mode: options.routing_mode.clone(),
            exit_node: options.exit_node.clone(),
            is_exit_node: options.is_exit_node,
            webui_port: options.webui_port,
            ui_mode: options.ui_mode,
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
            ui_mode: self.ui_mode,
        }
    }

    fn webui_runtime_options(&self) -> RuntimeOptions {
        RuntimeOptions {
            routing_mode: self.routing_mode.clone(),
            exit_node: self.exit_node.clone(),
            is_exit_node: self.is_exit_node,
            headless: true,
            gui: false,
            webui_port: self.webui_port,
            ui_mode: self.ui_mode,
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
    Service,
}

/// Dispatch an internal process role before clap parses the public CLI.
///
/// Returns `None` for every normal invocation, allowing `main` to preserve the
/// existing public CLI and native-GPUI main-thread requirements.
pub fn internal_entrypoint() -> Option<i32> {
    let arguments = std::env::args_os().collect::<Vec<_>>();
    let role = internal_role(arguments.iter().cloned())?;
    let options = match internal_options(&arguments) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("fuckipv6 内部进程参数错误: {error:#}");
            return Some(InternalExitCode::Configuration as i32);
        }
    };

    if matches!(role, InternalRole::Service) {
        match argument_value(&arguments, CONFIG_DIR_ARGUMENT) {
            Ok(Some(config_dir)) => std::env::set_var("FUCKIPV6_CONFIG_DIR", config_dir),
            Ok(None) => {}
            Err(error) => {
                eprintln!("fuckipv6 内部进程参数错误: {error:#}");
                return Some(InternalExitCode::Configuration as i32);
            }
        }
    }

    let result = match role {
        InternalRole::Control => run_runtime(|runtime| runtime.block_on(run_control(options))),
        InternalRole::Node => run_runtime(|runtime| runtime.block_on(run_node(options))),
        #[cfg(windows)]
        InternalRole::Service if is_windows_service_invocation(&arguments) => {
            crate::windows_service::run(move |shutdown| {
                run_runtime(|runtime| runtime.block_on(run_service(options, Some(shutdown))))
            })
        }
        InternalRole::Service => {
            run_runtime(|runtime| runtime.block_on(run_service(options, None)))
        }
    };
    Some(match result {
        Ok(()) => InternalExitCode::Success as i32,
        Err(error) => {
            eprintln!("fuckipv6 内部进程失败: {error:#}");
            InternalExitCode::RuntimeFailure as i32
        }
    })
}

#[cfg(windows)]
fn is_windows_service_invocation(arguments: &[std::ffi::OsString]) -> bool {
    arguments
        .iter()
        .any(|argument| argument == WINDOWS_SERVICE_ARGUMENT)
}

fn internal_role(arguments: impl IntoIterator<Item = std::ffi::OsString>) -> Option<InternalRole> {
    let role = arguments.into_iter().nth(1)?;
    match role.to_str()? {
        CONTROL_ARGUMENT => Some(InternalRole::Control),
        NODE_ARGUMENT => Some(InternalRole::Node),
        SERVICE_ARGUMENT => Some(InternalRole::Service),
        _ => None,
    }
}

fn internal_options(arguments: &[std::ffi::OsString]) -> Result<InternalOptions> {
    if let Some(path) = argument_value(arguments, OPTIONS_FILE_ARGUMENT)? {
        let encoded = std::fs::read_to_string(path)
            .with_context(|| format!("无法读取内部服务参数 {}", PathBuf::from(path).display()))?;
        return InternalOptions::decode(&encoded);
    }

    let encoded = std::env::var(OPTIONS_ENV).context("缺少内部进程参数")?;
    InternalOptions::decode(&encoded)
}

fn argument_value<'a>(
    arguments: &'a [std::ffi::OsString],
    name: &str,
) -> Result<Option<&'a std::ffi::OsStr>> {
    let Some(index) = arguments.iter().position(|argument| argument == name) else {
        return Ok(None);
    };
    arguments
        .get(index + 1)
        .map(std::ffi::OsString::as_os_str)
        .map(Some)
        .with_context(|| format!("内部进程参数 {name} 缺少路径"))
}

pub(crate) fn service_options_json(
    webui_port: u16,
    capabilities: crate::ui::UiCapabilities,
) -> Result<String> {
    let ui_mode = service_ui_resolution(capabilities)?;
    InternalOptions {
        routing_mode: None,
        exit_node: None,
        is_exit_node: None,
        webui_port,
        ui_mode: Some(ui_mode),
    }
    .encode()
}

fn service_ui_resolution(
    capabilities: crate::ui::UiCapabilities,
) -> Result<crate::ui::UiModeResolution> {
    crate::ui::resolve_ui_mode(crate::ui::UiModeRequest::WebUi, capabilities)
        .map_err(|error| anyhow::anyhow!("服务模式只能在无桌面环境的主机上运行: {error}"))
}

pub(crate) fn ensure_service_ui_available(capabilities: crate::ui::UiCapabilities) -> Result<()> {
    service_ui_resolution(capabilities).map(|_| ())
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
    let shutdown = ShutdownSignal::register()?;
    let _lock = acquire_instance_lock("node")?.context("另一个节点进程已经持有本地实例锁")?;
    // The previous node may have been SIGKILLed after binding. Holding the node
    // instance lock proves that no peer owns this endpoint, so stale Unix socket
    // files are safe to remove before binding the replacement.
    remove_stale_node_endpoint();

    let supervisor = MultiNetworkSupervisor::new(options.runtime_options())
        .context("无法初始化独立节点运行时")?;
    let endpoint = local_ipc::endpoint(EndpointRole::Node)?;
    let server = complete_with_timeout(
        "节点 IPC 服务启动",
        NODE_IPC_BIND_TIMEOUT,
        LocalServer::bind(endpoint, EndpointRole::Node),
    )
    .await
    .context("无法启动节点 IPC 服务")?;
    let commands = supervisor.commands();
    let (shutdown_tx, _shutdown_rx) = watch::channel(false);
    let mut service = tokio::spawn(serve_owned_node(
        server,
        commands.clone(),
        supervisor.state().clone(),
        supervisor.event_bus(),
        shutdown_tx.clone(),
    ));

    // The IPC listener is live first so startup failures remain observable, but
    // `ready` is deliberately not reported until the persisted data plane has
    // had a chance to start. The control process uses stdout readiness as the
    // barrier before exposing the node to UI clients.
    let startup_snapshot = supervisor.state().snapshot().await;
    let startup_context =
        NodeCommandContext::local_owner_for_snapshot(&startup_snapshot, "node_runtime_startup");
    match timeout(
        NODE_STARTUP_TIMEOUT,
        commands.execute(
            startup_context,
            NodeCommandRequest::new(u64::MAX - 1, NodeCommand::StartNode),
        ),
    )
    .await
    {
        Ok(Ok(result)) if result.is_success() => {
            tracing::info!(?result.outcome, "节点持久化数据面启动完成");
        }
        Ok(Ok(result)) => {
            tracing::warn!(?result.outcome, "节点持久化数据面启动失败，IPC 保持可用");
        }
        Ok(Err(error)) => {
            tracing::warn!(%error, "节点持久化数据面启动命令未送达，IPC 保持可用");
        }
        Err(_) => {
            tracing::warn!(
                "节点持久化数据面启动超过 {} 秒，IPC 保持可用",
                NODE_STARTUP_TIMEOUT.as_secs()
            );
        }
    }
    refresh_node_state_bounded(&supervisor).await;

    println!(
        "{}",
        serde_json::json!({
            "role": "node",
            "pid": std::process::id(),
            "state": "ready",
        })
    );

    let (command_stop_tx, mut command_stop_rx) = tokio::sync::mpsc::unbounded_channel();
    supervisor.registry().set_stop_sender(command_stop_tx);

    let mut signal = Box::pin(shutdown.wait());
    let mut refresh = tokio::time::interval(Duration::from_secs(1));
    refresh.set_missed_tick_behavior(MissedTickBehavior::Skip);
    refresh.tick().await;
    let mut service_finished = false;
    let mut service_failure = None;

    loop {
        tokio::select! {
            _ = &mut signal => break,
            Some(stop_target) = command_stop_rx.recv() => {
                if stop_target.is_none() {
                    break;
                }
            }
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
            _ = refresh.tick() => refresh_node_state_bounded(&supervisor).await,
        }
    }

    let _ = shutdown_tx.send(true);
    // The supervisor owns the actor and its service manager. Shutting them down
    // directly is deterministic and does not depend on the registry control
    // channel used by the long-running CLI supervisor path.
    match timeout(COMMAND_SHUTDOWN_TIMEOUT, supervisor.shutdown()).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!(%error, "节点数据面关闭失败"),
        Err(_) => tracing::warn!(
            "节点数据面关闭超过 {} 秒，继续回收 IPC 服务",
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
                abort_task_with_timeout(service, "节点 IPC 服务取消", TASK_ABORT_TIMEOUT)
                    .await
                    .context("节点 IPC 服务取消失败")?;
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
    let shutdown = ShutdownSignal::register()?;
    let Some(_lock) = acquire_instance_lock("control")? else {
        // A concurrent UI launch may have won the race. It is successful only if
        // the already-running control plane actually brings up the node.
        wait_for_node_ready(READY_TIMEOUT).await?;
        return Ok(());
    };

    run_control_loop(options, shutdown.wait()).await
}

async fn run_service(
    mut options: InternalOptions,
    windows_shutdown: Option<watch::Receiver<bool>>,
) -> Result<()> {
    logging::init_default()?;
    // A system service is the WebUI process. Re-check the live display
    // environment instead of trusting options written at installation time or
    // an older service.json that predates explicit UI modes.
    options.ui_mode = Some(service_ui_resolution(crate::ui::UiCapabilities::detect())?);
    if options.webui_port == 0 {
        bail!("服务模式要求非零 WebUI 监听端口");
    }
    let _surface_lease = crate::ui::acquire_surface(crate::ui::UiSurfaceKind::WebUi)?;
    let shutdown = ShutdownSignal::register()?;

    // The service process must own the control lease. If another control already
    // exists, failing closed avoids a unit that appears healthy while its stop
    // action cannot stop the authority responsible for the node/data plane.
    let _control_lock = acquire_instance_lock("control")?
        .context("已有 control 进程运行；请先停止交互式实例再启动系统服务")?;
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let mut control = Some(tokio::spawn(run_control_loop(
        options.clone(),
        wait_for_control_shutdown(shutdown_rx),
    )));

    let result = async {
        wait_for_node_ready(READY_TIMEOUT).await?;

        let session = complete_with_timeout(
            "连接节点 IPC",
            REMOTE_SESSION_START_TIMEOUT,
            crate::remote_node::RemoteNodeSession::start_with_role_async(
                options.webui_runtime_options(),
                IpcRole::WebUi,
            ),
        )
        .await?;
        let node_http = crate::node::NodeHttpAdapter::new(
            session.commands(),
            session.state(),
            session.events(),
        );
        let controller =
            std::sync::Arc::new(crate::webui::AppController::from_node_session(session));
        let mut webui = Some(
            complete_with_timeout(
                "WebUI 启动",
                WEBUI_START_TIMEOUT,
                crate::webui::start_app_webui_with_node(options.webui_port, controller, node_http),
            )
            .await?,
        );
        #[cfg(windows)]
        crate::windows_service::report_running();

        let exit_result = tokio::select! {
            _ = wait_for_service_shutdown(windows_shutdown.clone(), shutdown) => Ok(()),
            result = control.as_mut().expect("control task is live") => {
                control = None;
                match result {
                    Ok(Ok(())) => Err(anyhow::anyhow!("control 在服务停止请求前退出")),
                    Ok(Err(error)) => Err(error.context("control 异常退出")),
                    Err(error) => Err(anyhow::Error::from(error).context("等待 control 失败")),
                }
            }
            result = webui.as_mut().expect("WebUI task is live") => {
                webui = None;
                match result {
                    Ok(()) => Err(anyhow::anyhow!("WebUI 在服务停止请求前退出")),
                    Err(error) => Err(anyhow::Error::from(error).context("等待 WebUI 失败")),
                }
            }
        };

        let webui_cleanup = match webui.take() {
            Some(webui) => abort_task_with_timeout(webui, "WebUI 关闭", TASK_ABORT_TIMEOUT).await,
            None => Ok(()),
        };
        let _ = shutdown_tx.send(true);
        let cleanup_result = stop_control_task(control.take()).await;
        exit_result?;
        webui_cleanup?;
        cleanup_result
    }
    .await;

    if control.is_some() {
        let _ = shutdown_tx.send(true);
        if let Err(error) = stop_control_task(control.take()).await {
            tracing::warn!(%error, "服务启动失败后的 control 清理失败");
        }
    }
    result
}

async fn run_control_loop<F>(options: InternalOptions, shutdown: F) -> Result<()>
where
    F: Future<Output = ()> + Send,
{
    let mut child: Option<Child> = None;
    let mut adopted_node = false;
    let mut restart_delay = RESTART_BACKOFF_MIN;
    let mut unhealthy_since: Option<Instant> = None;
    let mut shutdown = Box::pin(shutdown);

    loop {
        tokio::select! {
            _ = &mut shutdown => break,
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
            if let Err(error) = terminate_child(&mut managed).await {
                tracing::error!(%error, "无法终止无响应的节点进程，保留所有权并稍后重试");
                child = Some(managed);
                sleep(restart_delay).await;
                continue;
            }
        }
        adopted_node = false;
        unhealthy_since = None;
        sleep(restart_delay).await;
        restart_delay = (restart_delay * 2).min(RESTART_BACKOFF_MAX);
    }

    stop_managed_node(child).await
}

async fn wait_for_control_shutdown(mut shutdown: watch::Receiver<bool>) {
    if *shutdown.borrow() {
        return;
    }
    while shutdown.changed().await.is_ok() {
        if *shutdown.borrow() {
            return;
        }
    }
}

async fn stop_control_task(control: Option<tokio::task::JoinHandle<Result<()>>>) -> Result<()> {
    let Some(mut control) = control else {
        return Ok(());
    };
    match timeout(SUPERVISOR_STOP_TIMEOUT, &mut control).await {
        Ok(Ok(Ok(()))) => Ok(()),
        Ok(Ok(Err(error))) => Err(error.context("control 关闭失败")),
        Ok(Err(error)) => Err(anyhow::Error::from(error).context("等待 control 关闭失败")),
        Err(_) => {
            control.abort();
            abort_task_with_timeout(control, "control 任务取消", TASK_ABORT_TIMEOUT).await?;
            bail!(
                "control 未能在 {} 秒内关闭；服务管理器必须终止整个服务进程组",
                SUPERVISOR_STOP_TIMEOUT.as_secs()
            )
        }
    }
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
    let mut child = command.spawn().context("无法启动控制进程")?;
    // The UI deliberately does not own control's lifetime, but it is still the
    // parent on Unix. Reap control if it exits while the UI remains open so a
    // crashed supervisor cannot linger as a zombie until the UI closes.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
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

async fn stop_managed_node(mut child: Option<Child>) -> Result<()> {
    let accepted = request_node_stop().await;
    if let Some(managed) = child.as_mut() {
        if accepted {
            match timeout(NODE_STOP_TIMEOUT, managed.wait()).await {
                Ok(Ok(_)) => return Ok(()),
                Ok(Err(error)) => tracing::warn!(%error, "等待节点退出失败"),
                Err(_) => tracing::warn!("等待节点优雅退出超时，将强制终止"),
            }
        }
        terminate_child(managed).await?;
    } else if accepted {
        let deadline = Instant::now() + NODE_STOP_TIMEOUT;
        while Instant::now() < deadline {
            if !node_ready().await {
                return Ok(());
            }
            sleep(READY_POLL_INTERVAL).await;
        }
        bail!("接管节点未在 {} 秒内退出", NODE_STOP_TIMEOUT.as_secs());
    }
    remove_stale_node_endpoint();
    Ok(())
}

async fn complete_with_timeout<T, F>(stage: &'static str, limit: Duration, future: F) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    match timeout(limit, future).await {
        Ok(result) => result,
        Err(_) => bail!("{stage}超过 {} 秒", limit.as_secs()),
    }
}

async fn abort_task_with_timeout<T>(
    mut task: tokio::task::JoinHandle<T>,
    stage: &'static str,
    limit: Duration,
) -> Result<()>
where
    T: Send + 'static,
{
    task.abort();
    match timeout(limit, &mut task).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(error)) if error.is_cancelled() => Ok(()),
        Ok(Err(error)) => Err(anyhow::Error::from(error)).context(stage),
        Err(_) => bail!("{stage}超过 {} 秒", limit.as_secs()),
    }
}

async fn terminate_child(child: &mut Child) -> Result<()> {
    if let Err(error) = child.start_kill() {
        if child
            .try_wait()
            .context("无法读取被终止子进程的状态")?
            .is_some()
        {
            return Ok(());
        }
        return Err(error).context("无法强制终止子进程");
    }
    match timeout(FORCE_STOP_TIMEOUT, child.wait()).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(error)) => Err(error).context("等待被终止子进程退出失败"),
        Err(_) => bail!(
            "子进程强制终止后超过 {} 秒仍未退出",
            FORCE_STOP_TIMEOUT.as_secs()
        ),
    }
}

async fn refresh_node_state_bounded(supervisor: &MultiNetworkSupervisor) {
    if let Err(error) = complete_with_timeout("节点状态刷新", NODE_STATE_REFRESH_TIMEOUT, async {
        supervisor.refresh_runtime_state().await;
        Ok::<(), anyhow::Error>(())
    })
    .await
    {
        tracing::warn!(%error, "节点状态刷新未完成，保留节点 IPC 可用性");
    }
}

fn remove_stale_node_endpoint() {
    #[cfg(unix)]
    {
        if let Ok(local_ipc::LocalEndpoint::Path(path)) = local_ipc::endpoint(EndpointRole::Node) {
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

async fn wait_for_service_shutdown(
    mut windows_shutdown: Option<watch::Receiver<bool>>,
    shutdown: ShutdownSignal,
) {
    let Some(mut windows_shutdown) = windows_shutdown.take() else {
        shutdown.wait().await;
        return;
    };
    tokio::select! {
        _ = shutdown.wait() => {}
        _ = async {
            while !*windows_shutdown.borrow() {
                if windows_shutdown.changed().await.is_err() {
                    std::future::pending::<()>().await;
                }
            }
        } => {}
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
        let ui_mode = crate::ui::UiModeResolution {
            requested: crate::ui::UiModeRequest::Gpui,
            effective: crate::ui::UiMode::Gpui,
            display: crate::ui::DisplayEnvironment::LinuxX11,
            gpui_available: true,
        };
        let runtime = RuntimeOptions {
            routing_mode: Some("global-proxy".into()),
            exit_node: Some("exit-a".into()),
            is_exit_node: Some(true),
            headless: false,
            gui: true,
            webui_port: 8080,
            ui_mode: Some(ui_mode),
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
        assert_eq!(node.ui_mode, Some(ui_mode));
    }

    #[test]
    fn service_mode_is_explicit_webui_and_rejects_desktops() {
        let headless = crate::ui::UiCapabilities {
            display: crate::ui::DisplayEnvironment::Headless,
            gpui_available: false,
        };
        let encoded = service_options_json(8080, headless).expect("headless service options");
        let decoded = InternalOptions::decode(&encoded).expect("decode service options");
        let ui_mode = decoded.ui_mode.expect("service must persist its UI mode");
        assert_eq!(ui_mode.requested, crate::ui::UiModeRequest::WebUi);
        assert_eq!(ui_mode.effective, crate::ui::UiMode::WebUi);
        assert_eq!(ui_mode.display, crate::ui::DisplayEnvironment::Headless);

        let desktop = crate::ui::UiCapabilities {
            display: crate::ui::DisplayEnvironment::LinuxWayland,
            gpui_available: true,
        };
        let error =
            service_options_json(8080, desktop).expect_err("desktop service must fail closed");
        assert!(error.to_string().contains("无桌面环境"));
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

    #[test]
    fn node_stop_timeout_covers_all_inner_shutdown_deadlines() {
        assert!(
            NODE_STOP_TIMEOUT > COMMAND_SHUTDOWN_TIMEOUT + SERVICE_SHUTDOWN_TIMEOUT,
            "node supervisor must allow data-plane and IPC shutdown to finish"
        );
    }

    #[tokio::test]
    async fn stalled_startup_phase_hits_its_hard_deadline() {
        let result = complete_with_timeout("测试启动阶段", Duration::from_millis(10), async {
            sleep(Duration::from_secs(60)).await;
            Ok::<(), anyhow::Error>(())
        })
        .await;

        let error = result.expect_err("stalled startup must time out");
        assert!(error.to_string().contains("测试启动阶段"));
    }

    #[tokio::test]
    async fn aborted_task_is_joined_within_its_hard_deadline() {
        let task = tokio::spawn(std::future::pending::<()>());

        abort_task_with_timeout(task, "测试任务取消", Duration::from_millis(500))
            .await
            .expect("aborted task must be joined");
    }
}
