use anyhow::{Context, Result};
use fuckipv6_client::symmetric::EmbeddedCoordinator;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;

#[tokio::test]
async fn register_check_command_registers_with_embedded_host() -> Result<()> {
    let node_bin = workspace_bin("fuckipv6")?;
    let signaling_addr = free_loopback_addr().await?;
    let coordinator = EmbeddedCoordinator::start(
        "register-check-test",
        signaling_addr.port(),
        0,
        Some("127.0.0.1".to_string()),
        None,
    )
    .await?;
    let advertised_relay_addr = SocketAddr::new(signaling_addr.ip(), coordinator.relay_addr.port());

    wait_for_tcp(signaling_addr).await?;

    let key_path = unique_temp_key_path();
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        Command::new(&node_bin)
            .arg("register-check")
            .arg("--coordinator")
            .arg(format!("ws://{signaling_addr}"))
            .arg("--key")
            .arg(&key_path)
            .arg("--p2p-port")
            .arg("0")
            .arg("--timeout-secs")
            .arg("5")
            .output(),
    )
    .await
    .context("register-check process timed out")?
    .context("failed to run register-check process")?;

    drop(coordinator);
    let _ = std::fs::remove_file(&key_path);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "register-check failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("Register check OK"));
    assert!(stdout.contains("Virtual IP: fd00::1001"));
    assert!(stdout.contains(&format!("QUIC relay: {advertised_relay_addr}")));
    assert!(stdout.contains("QUIC relay pin: "));

    Ok(())
}

async fn free_loopback_addr() -> Result<SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    drop(listener);
    Ok(addr)
}

async fn wait_for_tcp(addr: SocketAddr) -> Result<()> {
    let mut last_error = None;

    for _ in 0..100 {
        match TcpStream::connect(addr).await {
            Ok(_) => return Ok(()),
            Err(err) => {
                last_error = Some(err);
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }

    Err(last_error
        .map(anyhow::Error::from)
        .unwrap_or_else(|| anyhow::anyhow!("coordinator did not open signaling socket")))
}

fn workspace_bin(name: &str) -> Result<PathBuf> {
    let env_name = format!("CARGO_BIN_EXE_{name}");
    if let Some(path) = std::env::var_os(&env_name) {
        return Ok(PathBuf::from(path));
    }

    let exe = std::env::current_exe()?;
    let debug_dir = exe
        .parent()
        .and_then(|path| path.parent())
        .ok_or_else(|| anyhow::anyhow!("cannot derive target/debug directory from {:?}", exe))?;
    let path = debug_dir.join(name);
    if path.exists() {
        Ok(path)
    } else {
        anyhow::bail!("binary `{}` was not built at {:?}", name, path)
    }
}

fn unique_temp_key_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "fuckipv6-register-check-{}-{nanos}.key",
        std::process::id()
    ))
}
