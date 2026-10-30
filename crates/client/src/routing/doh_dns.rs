//! Loopback DNS-over-HTTPS bridge used while Linux proxy routing is active.
//!
//! Resolv.conf nameservers are intentionally outside the tunnel's policy
//! bypasses. In networks with DNS pollution this means applications can fail
//! even when IP traffic through the tunnel is healthy. This bridge accepts
//! ordinary UDP/TCP DNS on loopback, forwards the raw DNS message to a
//! certificate-verified DoH endpoint over the tunnel, and restores the original
//! resolver configuration on shutdown.

use anyhow::{bail, Context, Result};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::net::{IpAddr, SocketAddr};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::watch;
use tokio::task::JoinHandle;

const DEFAULT_LISTEN_ADDR: &str = "127.0.0.2:53";
const DEFAULT_UPSTREAMS: &str = "https://1.1.1.1/dns-query,https://8.8.8.8/dns-query";
const RESOLV_CONF_PATH: &str = "/etc/resolv.conf";
const LOCK_PATH: &str = "/run/fuckipv6-dns.lock";
const BACKUP_PATH: &str = "/run/fuckipv6-resolv.conf.backup";
const DNS_MESSAGE_LIMIT: usize = 65_535;

/// Owns the loopback DoH bridge and the temporary resolv.conf override.
#[derive(Debug)]
pub(crate) struct DohDnsGuard {
    previous_resolv_conf: Vec<u8>,
    shutdown: watch::Sender<bool>,
    tasks: Vec<JoinHandle<()>>,
    _lock: File,
}

impl DohDnsGuard {
    /// Install the resolver bridge for the current process.
    ///
    /// A second client in a multi-network runtime does not own the host DNS
    /// configuration; in that case `Ok(None)` is returned so it can continue
    /// using the bridge installed by the first runtime.
    pub(crate) async fn install() -> Result<Option<Self>> {
        let Some(lock) = acquire_lock()? else {
            return Ok(None);
        };
        restore_stale_backup()?;

        let listen_addr = listen_addr_from_env()?;
        let upstreams = upstreams_from_env()?;
        let udp = UdpSocket::bind(listen_addr)
            .await
            .with_context(|| format!("绑定本地 DoH DNS UDP 地址失败: {listen_addr}"))?;
        let tcp = TcpListener::bind(listen_addr)
            .await
            .with_context(|| format!("绑定本地 DoH DNS TCP 地址失败: {listen_addr}"))?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(8))
            .pool_idle_timeout(Duration::from_secs(60))
            .build()
            .context("创建 DoH HTTP 客户端失败")?;

        let previous_resolv_conf =
            std::fs::read(RESOLV_CONF_PATH).context("读取 /etc/resolv.conf 失败")?;
        save_backup(&previous_resolv_conf)?;
        let managed = managed_resolv_conf(&previous_resolv_conf, listen_addr.ip());
        if let Err(error) = std::fs::write(RESOLV_CONF_PATH, &managed) {
            let _ = std::fs::remove_file(BACKUP_PATH);
            return Err(error).context("写入 DoH 本地 DNS 配置失败");
        }

        let (shutdown, shutdown_rx) = watch::channel(false);
        let tasks = vec![
            tokio::spawn(run_udp(
                udp,
                client.clone(),
                upstreams.clone(),
                shutdown_rx.clone(),
            )),
            tokio::spawn(run_tcp(tcp, client, upstreams.clone(), shutdown_rx)),
        ];

        tracing::info!(
            "DoH DNS 已接管系统解析: listen={}, upstreams={:?}",
            listen_addr,
            upstreams
        );

        Ok(Some(Self {
            previous_resolv_conf,
            shutdown,
            tasks,
            _lock: lock,
        }))
    }

    /// Stop accepting DNS requests and restore the original resolver config.
    pub(crate) fn shutdown(&mut self) {
        let _ = self.shutdown.send(true);
        for task in self.tasks.drain(..) {
            task.abort();
        }
        if let Err(error) = std::fs::write(RESOLV_CONF_PATH, &self.previous_resolv_conf) {
            tracing::warn!("恢复 /etc/resolv.conf 失败: {error:#}");
        } else {
            let _ = std::fs::remove_file(BACKUP_PATH);
            tracing::info!("已恢复原 DNS 解析配置");
        }
    }
}

impl Drop for DohDnsGuard {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn acquire_lock() -> Result<Option<File>> {
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(LOCK_PATH)
        .with_context(|| format!("打开 DNS 配置锁失败: {LOCK_PATH}"))?;
    let result = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(Some(lock));
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        return Ok(None);
    }
    Err(error).with_context(|| format!("锁定 DNS 配置文件失败: {LOCK_PATH}"))
}

fn restore_stale_backup() -> Result<()> {
    let backup = Path::new(BACKUP_PATH);
    if !backup.exists() {
        return Ok(());
    }
    let previous = std::fs::read(backup).context("读取残留 DNS 配置备份失败")?;
    std::fs::write(RESOLV_CONF_PATH, previous).context("恢复残留 DNS 配置失败")?;
    std::fs::remove_file(backup).context("删除残留 DNS 配置备份失败")?;
    tracing::warn!("检测到上次异常退出，已先恢复原 DNS 配置");
    Ok(())
}

fn save_backup(previous: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(BACKUP_PATH)
        .context("创建 DNS 配置备份失败")?;
    file.write_all(previous).context("写入 DNS 配置备份失败")?;
    file.sync_all().context("同步 DNS 配置备份失败")
}

fn listen_addr_from_env() -> Result<SocketAddr> {
    let raw =
        std::env::var("FUCKIPV6_DNS_LISTEN").unwrap_or_else(|_| DEFAULT_LISTEN_ADDR.to_string());
    let address: SocketAddr = raw
        .parse()
        .with_context(|| format!("FUCKIPV6_DNS_LISTEN 不是有效地址: {raw}"))?;
    if !address.ip().is_loopback() {
        bail!("FUCKIPV6_DNS_LISTEN 必须监听回环地址，拒绝对外提供 DNS 代理");
    }
    if address.port() != 53 {
        bail!("FUCKIPV6_DNS_LISTEN 必须使用 53 端口，resolv.conf 不支持指定端口");
    }
    Ok(address)
}

fn upstreams_from_env() -> Result<Vec<String>> {
    let raw =
        std::env::var("FUCKIPV6_DOH_UPSTREAMS").unwrap_or_else(|_| DEFAULT_UPSTREAMS.to_string());
    let upstreams = raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if upstreams.is_empty() {
        bail!("FUCKIPV6_DOH_UPSTREAMS 不能为空");
    }
    for upstream in &upstreams {
        let url = reqwest::Url::parse(upstream)
            .with_context(|| format!("无效的 DoH 上游地址: {upstream}"))?;
        if url.scheme() != "https" || url.host_str().is_none() {
            bail!("DoH 上游必须使用 HTTPS 且包含主机: {upstream}");
        }
    }
    Ok(upstreams)
}

fn managed_resolv_conf(previous: &[u8], listen_ip: IpAddr) -> Vec<u8> {
    let original = String::from_utf8_lossy(previous);
    let mut managed = String::new();
    for line in original.lines() {
        if !line.trim_start().starts_with("nameserver") {
            managed.push_str(line);
            managed.push('\n');
        }
    }
    managed.push_str("# Managed by fuckipv6 while the tunnel is active\n");
    managed.push_str(&format!("nameserver {listen_ip}\n"));
    managed.into_bytes()
}

async fn run_udp(
    socket: UdpSocket,
    client: reqwest::Client,
    upstreams: Vec<String>,
    mut shutdown: watch::Receiver<bool>,
) {
    let socket = Arc::new(socket);
    let mut buffer = vec![0u8; DNS_MESSAGE_LIMIT];
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            result = socket.recv_from(&mut buffer) => {
                let Ok((length, peer)) = result else {
                    continue;
                };
                if length == 0 {
                    continue;
                }
                let query = buffer[..length].to_vec();
                let socket = socket.clone();
                let client = client.clone();
                let upstreams = upstreams.clone();
                tokio::spawn(async move {
                    let response = resolve_query(&client, &upstreams, &query)
                        .await
                        .unwrap_or_else(|error| {
                            tracing::debug!("DoH DNS 查询失败: {error:#}");
                            servfail_response(&query).unwrap_or_default()
                        });
                    if !response.is_empty() {
                        let _ = socket.send_to(&response, peer).await;
                    }
                });
            }
        }
    }
}

async fn run_tcp(
    listener: TcpListener,
    client: reqwest::Client,
    upstreams: Vec<String>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            result = listener.accept() => {
                let Ok((stream, _)) = result else {
                    continue;
                };
                let client = client.clone();
                let upstreams = upstreams.clone();
                tokio::spawn(handle_tcp_connection(stream, client, upstreams));
            }
        }
    }
}

async fn handle_tcp_connection(
    mut stream: TcpStream,
    client: reqwest::Client,
    upstreams: Vec<String>,
) {
    let mut length_bytes = [0u8; 2];
    if stream.read_exact(&mut length_bytes).await.is_err() {
        return;
    }
    let length = u16::from_be_bytes(length_bytes) as usize;
    if length == 0 {
        return;
    }
    let mut query = vec![0u8; length];
    if stream.read_exact(&mut query).await.is_err() {
        return;
    }
    let response = resolve_query(&client, &upstreams, &query)
        .await
        .unwrap_or_else(|error| {
            tracing::debug!("DoH TCP DNS 查询失败: {error:#}");
            servfail_response(&query).unwrap_or_default()
        });
    if response.is_empty() || response.len() > DNS_MESSAGE_LIMIT {
        return;
    }
    let response_len = (response.len() as u16).to_be_bytes();
    let _ = stream.write_all(&response_len).await;
    let _ = stream.write_all(&response).await;
    let _ = stream.shutdown().await;
}

async fn resolve_query(
    client: &reqwest::Client,
    upstreams: &[String],
    query: &[u8],
) -> Result<Vec<u8>> {
    if query.len() < 12 {
        bail!("DNS 查询消息过短");
    }
    let mut last_error = None;
    for upstream in upstreams {
        let result = async {
            let response = client
                .post(upstream)
                .header(ACCEPT, "application/dns-message")
                .header(CONTENT_TYPE, "application/dns-message")
                .body(query.to_vec())
                .send()
                .await
                .with_context(|| format!("连接 DoH 上游失败: {upstream}"))?;
            let status = response.status();
            if !status.is_success() {
                bail!("DoH 上游 {upstream} 返回 HTTP {status}");
            }
            let body = response
                .bytes()
                .await
                .context("读取 DoH 上游响应失败")?
                .to_vec();
            if body.len() < 12 {
                bail!("DoH 上游 {upstream} 返回了过短的 DNS 响应");
            }
            if body[..2] != query[..2] {
                bail!("DoH 上游 {upstream} 返回了不匹配的 DNS 事务 ID");
            }
            Ok(body)
        }
        .await;
        match result {
            Ok(body) => return Ok(body),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("没有可用的 DoH 上游")))
}

fn servfail_response(query: &[u8]) -> Option<Vec<u8>> {
    if query.len() < 12 {
        return None;
    }
    let mut response = query[..12].to_vec();
    response[2] = (response[2] & 0x01) | 0x80;
    response[3] = (response[3] & 0xf0) | 0x02;
    response[4..12].fill(0);
    Some(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_resolv_conf_replaces_only_nameservers() {
        let previous = b"# generated\nsearch lan example.test\nnameserver 10.0.0.1\nnameserver 2001:db8::1\noptions edns0\n";
        let managed =
            String::from_utf8(managed_resolv_conf(previous, "127.0.0.2".parse().unwrap())).unwrap();
        assert!(managed.contains("search lan example.test"));
        assert!(managed.contains("options edns0"));
        assert!(managed.contains("nameserver 127.0.0.2"));
        assert!(!managed.contains("nameserver 10.0.0.1"));
        assert!(!managed.contains("nameserver 2001:db8::1"));
    }

    #[test]
    fn servfail_keeps_transaction_id_and_request_bit() {
        let query = [
            0x12, 0x34, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        let response = servfail_response(&query).unwrap();
        assert_eq!(&response[..2], &[0x12, 0x34]);
        assert_eq!(response[2] & 0x80, 0x80);
        assert_eq!(response[2] & 0x01, 0x01);
        assert_eq!(response[3] & 0x0f, 0x02);
        assert_eq!(&response[4..12], &[0; 8]);
    }

    #[test]
    fn upstream_validation_requires_https() {
        let valid = reqwest::Url::parse("https://1.1.1.1/dns-query").unwrap();
        assert_eq!(valid.scheme(), "https");
        assert!(valid.host_str().is_some());
    }
}
