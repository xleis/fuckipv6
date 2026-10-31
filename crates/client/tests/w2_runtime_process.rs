#![cfg(target_os = "linux")]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use fuckipv6_client::local_ipc::{self, LocalCommand, LocalEndpoint, LocalRequest, LocalResponse};

struct ProcessGuard {
    child: Child,
}

impl ProcessGuard {
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.child.try_wait()
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn temp_dir(label: &str) -> Result<PathBuf> {
    let directory = std::env::temp_dir().join(format!(
        "fuckipv6-w2-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock before Unix epoch")?
            .as_nanos()
    ));
    std::fs::create_dir_all(directory.join("runtime"))?;
    std::fs::create_dir_all(directory.join("config"))?;
    std::fs::create_dir_all(directory.join("home"))?;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    Ok(directory)
}

fn read_pid(path: &Path) -> Result<u32> {
    std::fs::read_to_string(path)
        .with_context(|| format!("无法读取进程锁 {}", path.display()))?
        .trim()
        .parse()
        .with_context(|| format!("进程锁 {} 不含有效 PID", path.display()))
}

async fn wait_until_ready(endpoint: &LocalEndpoint) -> Result<()> {
    for _ in 0..150 {
        let request = local_ipc::request(
            endpoint,
            local_ipc::IpcRole::Cli,
            &LocalRequest {
                request_id: 1,
                command: LocalCommand::Snapshot,
            },
        );
        if matches!(
            tokio::time::timeout(Duration::from_millis(250), request).await,
            Ok(Ok(LocalResponse::Snapshot { .. }))
        ) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("控制面未在期限内提供节点快照")
}

async fn wait_for_exit(child: &mut ProcessGuard) -> Result<std::process::ExitStatus> {
    for _ in 0..150 {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("控制面未在期限内退出")
}

fn spawn_control(directory: &Path) -> Result<ProcessGuard> {
    let runtime_dir = directory.join("runtime");
    Ok(ProcessGuard {
        child: Command::new(env!("CARGO_BIN_EXE_fuckipv6"))
            .arg("__control")
            .env("FUCKIPV6_RUNTIME_DIR", runtime_dir)
            .env("XDG_CONFIG_HOME", directory.join("config"))
            .env("HOME", directory.join("home"))
            .env(
                "FUCKIPV6_INTERNAL_OPTIONS",
                r#"{"routing_mode":null,"exit_node":null,"is_exit_node":null,"webui_port":0}"#,
            )
            .env("RUST_LOG", "error")
            .current_dir(directory)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("无法启动 __control 集成测试进程")?,
    })
}

fn send_signal(pid: u32, signal: &str) -> Result<()> {
    let status = Command::new("kill")
        .arg(format!("-{signal}"))
        .arg(pid.to_string())
        .status()
        .context("无法执行 kill")?;
    anyhow::ensure!(status.success(), "kill -{signal} {pid} 失败: {status}");
    Ok(())
}

fn process_alive(pid: u32) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

async fn wait_for_pid(path: &Path, expected: u32) -> Result<()> {
    for _ in 0..150 {
        if read_pid(path).is_ok_and(|pid| pid == expected) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("进程锁 {} 未在期限内更新为 PID {expected}", path.display())
}

async fn wait_for_new_pid(path: &Path, previous: u32) -> Result<u32> {
    for _ in 0..150 {
        if let Ok(pid) = read_pid(path) {
            if pid != previous && process_alive(pid) {
                return Ok(pid);
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("进程锁 {} 未在期限内出现新 PID", path.display())
}

#[tokio::test]
async fn control_supervises_one_node_and_ui_disconnect_leaves_it_running() -> Result<()> {
    let directory = temp_dir("process")?;
    let runtime_dir = directory.join("runtime");
    let node_endpoint = LocalEndpoint::Path(runtime_dir.join("node.sock"));
    let control_endpoint = LocalEndpoint::Path(runtime_dir.join("control.sock"));

    let mut child = spawn_control(&directory)?;

    wait_until_ready(&node_endpoint).await?;
    let control_pid = read_pid(&runtime_dir.join("control.lock"))?;
    let node_pid = read_pid(&runtime_dir.join("node.lock"))?;
    assert_eq!(control_pid, child.child.id());
    assert_ne!(
        control_pid, node_pid,
        "control and node must be distinct processes"
    );

    // A short-lived client and an idle subscription both represent UI lifetime.
    // Dropping them must not send Shutdown or stop the node socket.
    drop(local_ipc::connect(&node_endpoint, local_ipc::IpcRole::Gpui).await?);
    assert!(matches!(
        local_ipc::request(
            &node_endpoint,
            local_ipc::IpcRole::Gpui,
            &LocalRequest {
                request_id: 2,
                command: LocalCommand::Snapshot,
            },
        )
        .await?,
        LocalResponse::Snapshot { .. }
    ));

    let response = local_ipc::request(
        &node_endpoint,
        local_ipc::IpcRole::Control,
        &LocalRequest {
            request_id: 3,
            command: LocalCommand::StopNode,
        },
    )
    .await?;
    assert!(matches!(response, LocalResponse::Accepted { .. }));
    let status = wait_for_exit(&mut child).await?;

    let output = if status.success() {
        String::new()
    } else {
        let stdout = child
            .child
            .stdout
            .take()
            .map(|mut stream| std::io::read_to_string(&mut stream))
            .transpose()?
            .unwrap_or_default();
        let stderr = child
            .child
            .stderr
            .take()
            .map(|mut stream| std::io::read_to_string(&mut stream))
            .transpose()?
            .unwrap_or_default();
        format!("stdout:\n{stdout}\nstderr:\n{stderr}")
    };
    assert!(status.success(), "control exited with {status}: {output}");
    assert!(!endpoint_path(&node_endpoint).exists());
    assert!(!endpoint_path(&control_endpoint).exists());
    std::fs::remove_dir_all(directory)?;
    Ok(())
}

#[tokio::test]
async fn replacement_control_adopts_node_after_control_sigkill() -> Result<()> {
    let directory = temp_dir("control-recovery")?;
    let runtime_dir = directory.join("runtime");
    let node_endpoint = LocalEndpoint::Path(runtime_dir.join("node.sock"));
    let mut first_control = spawn_control(&directory)?;
    wait_until_ready(&node_endpoint).await?;

    let first_control_pid = first_control.child.id();
    let node_pid = read_pid(&runtime_dir.join("node.lock"))?;
    send_signal(first_control_pid, "KILL")?;
    let status = wait_for_exit(&mut first_control).await?;
    assert!(
        !status.success(),
        "SIGKILLed control unexpectedly succeeded"
    );
    assert!(process_alive(node_pid), "node must outlive a control crash");
    wait_until_ready(&node_endpoint).await?;

    let mut replacement_control = spawn_control(&directory)?;
    let replacement_pid = replacement_control.child.id();
    wait_for_pid(&runtime_dir.join("control.lock"), replacement_pid).await?;
    wait_until_ready(&node_endpoint).await?;
    assert_eq!(
        read_pid(&runtime_dir.join("node.lock"))?,
        node_pid,
        "control recovery must adopt, not restart, the live node"
    );
    assert!(
        replacement_control.try_wait()?.is_none(),
        "replacement control must stay resident"
    );

    send_signal(replacement_pid, "TERM")?;
    let status = wait_for_exit(&mut replacement_control).await?;
    assert!(status.success(), "replacement control exited with {status}");
    assert!(!process_alive(node_pid), "node was not stopped");
    std::fs::remove_dir_all(directory)?;
    Ok(())
}

#[tokio::test]
async fn control_restarts_a_crashed_node_without_exiting() -> Result<()> {
    let directory = temp_dir("node-crash")?;
    let runtime_dir = directory.join("runtime");
    let node_endpoint = LocalEndpoint::Path(runtime_dir.join("node.sock"));
    let mut control = spawn_control(&directory)?;
    wait_until_ready(&node_endpoint).await?;

    let control_pid = control.child.id();
    let old_node_pid = read_pid(&runtime_dir.join("node.lock"))?;
    send_signal(old_node_pid, "KILL")?;
    let new_node_pid = wait_for_new_pid(&runtime_dir.join("node.lock"), old_node_pid).await?;
    assert_ne!(new_node_pid, old_node_pid);
    wait_until_ready(&node_endpoint).await?;
    assert!(
        control.try_wait()?.is_none(),
        "control must remain available while node restarts"
    );

    send_signal(control_pid, "TERM")?;
    let status = wait_for_exit(&mut control).await?;
    assert!(status.success(), "control exited with {status}");
    assert!(
        !process_alive(new_node_pid),
        "replacement node was not stopped"
    );
    std::fs::remove_dir_all(directory)?;
    Ok(())
}

#[tokio::test]
async fn stopped_node_is_force_killed_by_control_without_manual_cleanup() -> Result<()> {
    let directory = temp_dir("stop-watchdog")?;
    let runtime_dir = directory.join("runtime");
    let node_endpoint = LocalEndpoint::Path(runtime_dir.join("node.sock"));
    let mut control = spawn_control(&directory)?;
    wait_until_ready(&node_endpoint).await?;

    let control_pid = control.child.id();
    let node_pid = read_pid(&runtime_dir.join("node.lock"))?;
    send_signal(node_pid, "STOP")?;
    send_signal(control_pid, "TERM")?;
    let status = wait_for_exit(&mut control).await?;
    assert!(status.success(), "control exited with {status}");
    assert!(
        !process_alive(node_pid),
        "stopped node was not force-killed"
    );
    assert!(
        !endpoint_path(&node_endpoint).exists(),
        "stale node IPC endpoint was not removed"
    );
    std::fs::remove_dir_all(directory)?;
    Ok(())
}

fn endpoint_path(endpoint: &LocalEndpoint) -> &Path {
    match endpoint {
        LocalEndpoint::Path(path) => path,
    }
}
