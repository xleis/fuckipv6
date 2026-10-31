use anyhow::{Context, Result};
use fuckipv6_common::VirtualIps;
use std::fs::File;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::os::fd::{AsRawFd, FromRawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::unix::AsyncFd;
use tokio::io::Interest;

pub struct LinuxTun {
    reader: Arc<AsyncFd<tun::platform::posix::Reader>>,
    writer: Arc<AsyncFd<File>>,
    // This is deliberately the final field. On the last handle, the reader and
    // writer descriptors close before `LinkLifetime::drop` starts link removal.
    lifetime: Arc<LinkLifetime>,
}

struct LinkLifetime {
    name: String,
    virtual_ips: VirtualIps,
    close_requested: AtomicBool,
    deletion_started: AtomicBool,
}

impl LinkLifetime {
    fn new(name: String, virtual_ips: VirtualIps) -> Self {
        Self {
            name,
            virtual_ips,
            close_requested: AtomicBool::new(false),
            deletion_started: AtomicBool::new(false),
        }
    }

    fn begin_deletion(&self) -> bool {
        self.deletion_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}

impl Drop for LinkLifetime {
    fn drop(&mut self) {
        if !self.begin_deletion() {
            return;
        }

        tracing::info!("正在释放 TUN 网卡资源: {}", self.name);
        let name = self.name.clone();
        std::thread::spawn(move || {
            let _ = std::process::Command::new("ip")
                .args(["link", "delete", &name])
                .status();
        });
    }
}

impl Clone for LinuxTun {
    fn clone(&self) -> Self {
        Self {
            reader: self.reader.clone(),
            writer: self.writer.clone(),
            lifetime: Arc::clone(&self.lifetime),
        }
    }
}

impl LinuxTun {
    #[allow(dead_code)]
    pub async fn create(name: &str, ip: Ipv6Addr, prefix_len: u8, mtu: u16) -> Result<Self> {
        Self::create_dual_stack(name, &VirtualIps::from_ipv6(ip), 24, prefix_len, mtu).await
    }

    pub async fn create_dual_stack(
        name: &str,
        virtual_ips: &VirtualIps,
        ipv4_prefix: u8,
        ipv6_prefix: u8,
        mtu: u16,
    ) -> Result<Self> {
        tracing::info!(
            "正在创建 TUN 虚拟网卡: {} (双栈模式: {:?})",
            name,
            virtual_ips
        );

        validate_tun_inputs(name, mtu)?;

        // 用 ip tuntap 命令创建设备（这样会有 IPv6/IPv4 支持）
        let create_output = tokio::process::Command::new("ip")
            .args(["tuntap", "add", "mode", "tun", "name", name])
            .output()
            .await;

        let create_output = match create_output {
            Ok(output) => output,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                anyhow::bail!("无法执行 ip 命令：请安装 iproute2 后重试");
            }
            Err(e) => {
                return Err(e).context("执行 ip tuntap add 命令失败");
            }
        };

        if !create_output.status.success() {
            let stderr = String::from_utf8_lossy(&create_output.stderr);
            tracing::error!("创建 TUN 网卡失败: {}", stderr);
            anyhow::bail!("{}", classify_create_error(&stderr, name));
        }
        tracing::debug!("已通过 ip tuntap 成功创建 TUN 虚拟设备");

        let ips = virtual_ips.clone();
        let setup_result = async {
            configure_link(name, mtu).await?;
            if let Some(ipv4) = ips.ipv4 {
                configure_ipv4_address(name, ipv4, ipv4_prefix).await?;
                configure_ipv4_route(name, ipv4, ipv4_prefix).await;
            }
            if let Some(ipv6) = ips.ipv6 {
                configure_ipv6_address(name, ipv6).await?;
                bring_link_up(name).await?;
                configure_ipv6_route(name, ipv6, ipv6_prefix).await;
            } else {
                bring_link_up(name).await?;
            }

            let device = open_tun_device(name).await?;
            let (reader, split_writer) = device.split();
            drop(split_writer);

            set_nonblocking(reader.as_raw_fd()).context("设置 TUN 为非阻塞模式失败")?;
            let writer_fd = duplicate_fd(reader.as_raw_fd()).context("复制 TUN 文件描述符失败")?;
            // SAFETY: duplicate_fd returned a new owned descriptor, and File
            // assumes responsibility for closing it exactly once.
            let writer_file = unsafe { File::from_raw_fd(writer_fd) };

            // SAFETY: both values are owning `File`s whose descriptors stay
            // open and unchanged for the complete lifetime of their AsyncFd
            // wrappers.
            let reader = unsafe { AsyncFd::register(reader) }.context("注册 TUN 读事件失败")?;
            // SAFETY: `writer_file` owns the duplicated descriptor and retains
            // it until this `AsyncFd` is dropped.
            let writer =
                unsafe { AsyncFd::register(writer_file) }.context("注册 TUN 写事件失败")?;

            Ok::<_, anyhow::Error>((reader, writer))
        }
        .await;

        let (reader, writer) = cleanup_after_failed_setup(setup_result, || async {
            if let Err(error) = delete_link(name).await {
                tracing::error!(%error, "删除初始化失败的 TUN 设备时出错: {}", name);
            }
        })
        .await?;

        tracing::info!("TUN 设备已完全配置并打开: {}", name);

        Ok(Self {
            reader: Arc::new(reader),
            writer: Arc::new(writer),
            lifetime: Arc::new(LinkLifetime::new(name.to_string(), virtual_ips.clone())),
        })
    }

    pub fn name(&self) -> &str {
        &self.lifetime.name
    }

    #[allow(dead_code)]
    pub fn virtual_ips(&self) -> &VirtualIps {
        &self.lifetime.virtual_ips
    }

    #[allow(dead_code)]
    pub fn ip(&self) -> Ipv6Addr {
        self.lifetime.virtual_ips.primary_ipv6()
    }

    pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.reader
            .async_io(Interest::READABLE, |reader| {
                read_fd(reader.as_raw_fd(), buf)
            })
            .await
            .context("Failed to read from TUN device")
    }

    /// Await one readable edge, then drain additional queued L3 packets while
    /// the nonblocking descriptor remains ready. Linux TUN has no
    /// multi-message read syscall, so this reduces wakeups/task handoffs but
    /// deliberately does not claim to reduce one `read(2)` per packet.
    pub async fn read_batch(&mut self, buffers: &mut [&mut [u8]]) -> Result<Vec<usize>> {
        let Some(first) = buffers.first_mut() else {
            return Ok(Vec::new());
        };
        let first_len = self.read(first).await;

        let (lengths, drain_error) =
            collect_read_batch(first_len, &mut buffers[1..], |buffer| self.try_read(buffer))?;
        if let Some(error) = drain_error {
            // The first packet is already valid. Keep the successful prefix so
            // a transient drain failure cannot terminate the reader task.
            tracing::warn!(
                %error,
                packets = lengths.len(),
                "读取 TUN 批次后续数据包失败，返回已成功读取的前缀"
            );
        }
        Ok(lengths)
    }

    fn try_read(&mut self, buf: &mut [u8]) -> Result<Option<usize>> {
        match self.reader.try_io(Interest::READABLE, |reader| {
            read_fd(reader.as_raw_fd(), buf)
        }) {
            Ok(length) => Ok(Some(length)),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error).context("Failed to drain readable TUN batch"),
        }
    }

    pub async fn write(&mut self, packet: &[u8]) -> Result<()> {
        let written = self
            .writer
            .async_io(Interest::WRITABLE, |writer| {
                write_fd(writer.as_raw_fd(), packet)
            })
            .await
            .context("Failed to write to TUN device")?;
        if written != packet.len() {
            anyhow::bail!(
                "TUN short write: wrote {} of {} bytes",
                written,
                packet.len()
            );
        }
        Ok(())
    }

    /// 请求删除 TUN 设备。
    ///
    /// 生命周期由所有 clone 共享。只要还有其他 handle 存活，本方法只记录
    /// 关闭请求并成功返回，不会删除链路；最后一个 handle 显式调用本方法或
    /// 被 drop 时才执行一次实际删除。这样任一 clone 都不会使其他 clone 的
    /// 链路失效。
    pub async fn delete(&mut self) -> Result<()> {
        if self.lifetime.close_requested.swap(true, Ordering::AcqRel) {
            tracing::debug!("重复的 TUN 关闭请求: {}", self.lifetime.name);
        }

        if !request_link_close(&self.lifetime) {
            tracing::info!(
                "TUN 设备仍被其他 clone 使用，延迟删除: {}",
                self.lifetime.name
            );
            return Ok(());
        }

        tracing::info!("正在删除 TUN 设备: {}", self.lifetime.name);

        if let Some(ipv6) = self.lifetime.virtual_ips.ipv6 {
            let addr_cmd = format!("{}/128", ipv6);
            let _ = tokio::process::Command::new("ip")
                .args(["-6", "addr", "del", &addr_cmd, "dev", &self.lifetime.name])
                .stderr(std::process::Stdio::null())
                .status()
                .await;
        }

        if let Some(ipv4) = self.lifetime.virtual_ips.ipv4 {
            let addr_cmd = format!("{}/32", ipv4);
            let _ = tokio::process::Command::new("ip")
                .args(["addr", "del", &addr_cmd, "dev", &self.lifetime.name])
                .stderr(std::process::Stdio::null())
                .status()
                .await;
        }

        if !self.lifetime.begin_deletion() {
            tracing::debug!("TUN 设备删除已启动: {}", self.lifetime.name);
            return Ok(());
        }

        delete_link(&self.lifetime.name).await?;
        tracing::info!("TUN 设备已删除");

        Ok(())
    }
}

fn request_link_close(lifetime: &Arc<LinkLifetime>) -> bool {
    lifetime.close_requested.store(true, Ordering::Release);
    Arc::strong_count(lifetime) == 1
}

fn collect_read_batch<E, F>(
    first: std::result::Result<usize, E>,
    buffers: &mut [&mut [u8]],
    mut drain: F,
) -> std::result::Result<(Vec<usize>, Option<E>), E>
where
    F: FnMut(&mut [u8]) -> std::result::Result<Option<usize>, E>,
{
    let first_len = first?;
    let mut lengths = vec![first_len];
    if first_len == 0 {
        return Ok((lengths, None));
    }

    for buffer in buffers {
        match drain(buffer) {
            // A zero-length read means EOF. The first EOF is retained as a
            // sentinel so the reader can stop, but an EOF encountered while
            // draining after valid packets must not be forwarded as an extra
            // zero-length entry.
            Ok(Some(0)) | Ok(None) => break,
            Ok(Some(length)) => lengths.push(length),
            Err(error) => return Ok((lengths, Some(error))),
        }
    }

    Ok((lengths, None))
}

async fn cleanup_after_failed_setup<T, F, Fut>(setup: Result<T>, cleanup: F) -> Result<T>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    match setup {
        Ok(value) => Ok(value),
        Err(error) => {
            cleanup().await;
            Err(error)
        }
    }
}

fn validate_tun_inputs(name: &str, mtu: u16) -> Result<()> {
    if name.trim().is_empty() {
        anyhow::bail!("TUN 设备名称不能为空");
    }

    if mtu < 1280 {
        anyhow::bail!("IPv6 TUN MTU 不能小于 1280");
    }

    Ok(())
}

fn read_fd(fd: std::os::fd::RawFd, buf: &mut [u8]) -> io::Result<usize> {
    // SAFETY: buf is valid for writes for buf.len() bytes and fd is a live TUN
    // descriptor owned by the AsyncFd registration.
    let read = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
    if read < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(read as usize)
    }
}

fn write_fd(fd: std::os::fd::RawFd, buf: &[u8]) -> io::Result<usize> {
    // SAFETY: buf is valid for reads for buf.len() bytes and fd is a live TUN
    // descriptor owned by the AsyncFd registration.
    let written = unsafe { libc::write(fd, buf.as_ptr().cast(), buf.len()) };
    if written < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(written as usize)
    }
}

fn set_nonblocking(fd: std::os::fd::RawFd) -> std::io::Result<()> {
    // SAFETY: fcntl only reads and updates flags for the live TUN descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: fd remains live for the duration of this call; O_NONBLOCK does not
    // alter ownership or close-on-exec state.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn duplicate_fd(fd: std::os::fd::RawFd) -> std::io::Result<std::os::fd::RawFd> {
    // SAFETY: dup returns a new descriptor on success and -1 on failure.
    let duplicated = unsafe { libc::dup(fd) };
    if duplicated < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(duplicated)
    }
}

fn classify_create_error(stderr: &str, name: &str) -> String {
    let lower = stderr.to_ascii_lowercase();

    if lower.contains("operation not permitted")
        || lower.contains("permission denied")
        || lower.contains("not permitted")
    {
        return "创建 TUN 设备失败：权限不足，请使用 root 运行或授予 CAP_NET_ADMIN".to_string();
    }

    if lower.contains("file exists") || lower.contains("already exists") {
        return format!(
            "创建 TUN 设备失败：设备 {} 已存在，请先运行 `ip link delete {}` 清理残留设备",
            name, name
        );
    }

    if lower.contains("no such file or directory") || lower.contains("/dev/net/tun") {
        return "创建 TUN 设备失败：/dev/net/tun 不可用，请加载 tun 内核模块".to_string();
    }

    format!("创建 TUN 设备失败：{}", stderr.trim())
}

async fn configure_link(name: &str, mtu: u16) -> Result<()> {
    let mtu_output = tokio::process::Command::new("ip")
        .args(["link", "set", name, "mtu", &mtu.to_string()])
        .output()
        .await
        .context("设置 TUN MTU 失败")?;

    if !mtu_output.status.success() {
        let stderr = String::from_utf8_lossy(&mtu_output.stderr);
        tracing::error!("设置 TUN MTU 失败: {}", stderr);
        anyhow::bail!("设置 TUN MTU 失败：{}", stderr.trim());
    }

    tracing::debug!("TUN 设备 MTU 已设置为 {}", mtu);
    Ok(())
}

async fn configure_ipv6_address(name: &str, ip: Ipv6Addr) -> Result<()> {
    let addr_cmd = format!("{}/128", ip);
    tracing::debug!("正在为网卡 {} 添加 IPv6 地址: {}", name, addr_cmd);

    let addr_status = tokio::process::Command::new("ip")
        .args(["-6", "addr", "add", &addr_cmd, "dev", name])
        .output()
        .await
        .context("执行 ip -6 addr add 命令失败")?;

    if addr_status.status.success() {
        tracing::info!("IPv6 地址配置成功: {}", addr_cmd);
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&addr_status.stderr);
    let stdout = String::from_utf8_lossy(&addr_status.stdout);
    tracing::error!("配置 IPv6 地址失败，退出码: {}", addr_status.status);
    if !stderr.is_empty() {
        tracing::error!("stderr: {}", stderr.trim());
    }
    if !stdout.is_empty() {
        tracing::error!("stdout: {}", stdout.trim());
    }

    anyhow::bail!("配置 TUN IPv6 地址失败：{}", stderr.trim());
}

async fn configure_ipv4_address(name: &str, ip: Ipv4Addr, prefix_len: u8) -> Result<()> {
    let addr_cmd = format!("{}/{}", ip, prefix_len);
    tracing::debug!("正在为网卡 {} 添加 IPv4 地址: {}", name, addr_cmd);

    let status = tokio::process::Command::new("ip")
        .args(["addr", "add", &addr_cmd, "dev", name])
        .output()
        .await
        .context("执行 ip addr add 命令失败")?;

    if status.status.success() {
        tracing::info!("IPv4 地址配置成功: {}", addr_cmd);
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&status.stderr);
    let stdout = String::from_utf8_lossy(&status.stdout);
    tracing::error!("配置 IPv4 地址失败，退出码: {}", status.status);
    if !stderr.is_empty() {
        tracing::error!("stderr: {}", stderr.trim());
    }
    if !stdout.is_empty() {
        tracing::error!("stdout: {}", stdout.trim());
    }

    anyhow::bail!("配置 TUN IPv4 地址失败：{}", stderr.trim());
}

async fn bring_link_up(name: &str) -> Result<()> {
    tracing::debug!("正在启用 TUN 网卡: {}", name);
    let up_output = tokio::process::Command::new("ip")
        .args(["link", "set", name, "up"])
        .output()
        .await
        .context("启用 TUN 设备失败")?;

    if !up_output.status.success() {
        let stderr = String::from_utf8_lossy(&up_output.stderr);
        tracing::error!("启用 TUN 网卡失败: {}", stderr);
        anyhow::bail!("启用 TUN 设备失败：{}", stderr.trim());
    }

    tracing::debug!("TUN 网卡 {} 已就绪 (UP)", name);
    Ok(())
}

fn metric_for_device(name: &str) -> u32 {
    let mut hash: u32 = 0;
    for b in name.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(b as u32);
    }
    1000 + (hash % 1000)
}

async fn configure_ipv6_route(name: &str, ip: Ipv6Addr, prefix_len: u8) {
    let mask = if prefix_len == 0 {
        0
    } else {
        (!0u128) << (128 - prefix_len)
    };
    let net = Ipv6Addr::from(u128::from(ip) & mask);
    let route_cmd = format!("{}/{}", net, prefix_len);
    let metric = metric_for_device(name).to_string();
    tracing::debug!(
        "Adding IPv6 route: {} to device: {} (metric: {})",
        route_cmd,
        name,
        metric
    );
    let route_status = tokio::process::Command::new("ip")
        .args([
            "-6", "route", "add", &route_cmd, "dev", name, "metric", &metric,
        ])
        .output()
        .await;

    match route_status {
        Ok(output) if output.status.success() => {
            tracing::info!(
                "IPv6 路由配置成功: {} (dev {}, metric {})",
                route_cmd,
                name,
                metric
            );
        }
        Ok(output) => {
            tracing::warn!("配置路由失败，退出码: {} (可能路由已存在)", output.status);
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.is_empty() {
                tracing::warn!("错误信息: {}", stderr.trim());
            }
        }
        Err(e) => {
            tracing::warn!("执行 ip route 命令失败: {:?}", e);
        }
    }
}

async fn configure_ipv4_route(name: &str, ip: Ipv4Addr, prefix_len: u8) {
    let mask = if prefix_len == 0 {
        0
    } else {
        (!0u32) << (32 - prefix_len)
    };
    let net = Ipv4Addr::from(u32::from(ip) & mask);
    let route_cmd = format!("{}/{}", net, prefix_len);
    let metric = metric_for_device(name).to_string();
    tracing::debug!(
        "Adding IPv4 route: {} to device: {} (metric: {})",
        route_cmd,
        name,
        metric
    );

    let route_status = tokio::process::Command::new("ip")
        .args(["route", "add", &route_cmd, "dev", name, "metric", &metric])
        .output()
        .await;

    match route_status {
        Ok(output) if output.status.success() => {
            tracing::info!("IPv4 路由配置成功: {}", route_cmd);
        }
        Ok(output) => {
            tracing::debug!("配置 IPv4 路由返回: {} (可能路由已自动生成)", output.status);
        }
        Err(e) => {
            tracing::warn!("执行 ip route 命令失败: {:?}", e);
        }
    }
}

async fn open_tun_device(name: &str) -> Result<tun::platform::Device> {
    let name_owned = name.to_string();
    tokio::task::spawn_blocking(move || {
        let mut config = tun::Configuration::default();
        config.name(&name_owned);

        #[cfg(target_os = "linux")]
        config.platform(|config| {
            config.packet_information(false);
        });

        tun::create(&config)
    })
    .await
    .context("TUN open blocking task failed to join")?
    .context("打开 TUN 设备失败")
}

async fn delete_link(name: &str) -> Result<()> {
    let output = tokio::process::Command::new("ip")
        .args(["link", "delete", name])
        .output()
        .await
        .context("执行 ip link delete 命令失败")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let lower = stderr.to_ascii_lowercase();
        if lower.contains("cannot find device") || lower.contains("does not exist") {
            tracing::debug!("TUN 设备已不存在: {}", name);
            return Ok(());
        }

        anyhow::bail!("删除 TUN 设备失败：{}", stderr.trim());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn classify_permission_error() {
        let message = classify_create_error("RTNETLINK answers: Operation not permitted", "tun0");
        assert!(message.contains("权限不足"));
        assert!(message.contains("CAP_NET_ADMIN"));
    }

    #[test]
    fn classify_existing_device_error() {
        let message = classify_create_error("ioctl(TUNSETIFF): File exists", "fuckipv6");
        assert!(message.contains("已存在"));
        assert!(message.contains("ip link delete fuckipv6"));
    }

    #[test]
    fn validate_rejects_ipv6_mtu_below_minimum() {
        let err = validate_tun_inputs("tun0", 1279).unwrap_err().to_string();
        assert!(err.contains("1280"));
    }

    #[test]
    fn read_batch_preserves_prefix_on_drain_error() {
        let mut first = [0u8; 64];
        let mut second = [0u8; 64];
        let mut third = [0u8; 64];
        let mut buffers: [&mut [u8]; 3] = [&mut first[..], &mut second[..], &mut third[..]];
        let mut calls = 0;

        let (lengths, error) = collect_read_batch(Ok::<usize, io::Error>(96), &mut buffers, |_| {
            calls += 1;
            match calls {
                1 => Ok(Some(48)),
                2 => Err(io::Error::from_raw_os_error(libc::EIO)),
                _ => panic!("drain must stop after its first error"),
            }
        })
        .expect("a successful first packet must be preserved");

        assert_eq!(lengths, vec![96, 48]);
        assert_eq!(error.expect("drain error").raw_os_error(), Some(libc::EIO));
        assert_eq!(calls, 2);
    }

    #[test]
    fn read_batch_preserves_first_packet_error() {
        let mut buffer = [0u8; 64];
        let mut buffers: [&mut [u8]; 1] = [&mut buffer[..]];

        let result = collect_read_batch(
            Err::<usize, io::Error>(io::Error::other("first read failed")),
            &mut buffers,
            |_| -> std::result::Result<Option<usize>, io::Error> { panic!("drain must not run") },
        );

        assert_eq!(result.unwrap_err().to_string(), "first read failed");
    }

    #[test]
    fn read_batch_stops_on_would_block_or_eof() {
        fn assert_terminal(terminal: Option<usize>) {
            let mut buffer = [0u8; 64];
            let mut buffers: [&mut [u8]; 1] = [&mut buffer[..]];
            let mut calls = 0;

            let (lengths, error) =
                collect_read_batch(Ok::<usize, io::Error>(96), &mut buffers, |_| {
                    calls += 1;
                    Ok(terminal)
                })
                .expect("terminal drain state is not an error");

            assert_eq!(lengths, vec![96]);
            assert!(error.is_none());
            assert_eq!(calls, 1);
        }

        assert_terminal(None);
        assert_terminal(Some(0));
    }

    #[tokio::test]
    async fn post_open_failure_deletes_link_exactly_once() {
        let deletions = AtomicUsize::new(0);
        let setup_result: Result<()> = Err(anyhow::anyhow!("register TUN reader failed"));

        let result = cleanup_after_failed_setup(setup_result, || async {
            deletions.fetch_add(1, Ordering::SeqCst);
        })
        .await;

        assert!(result
            .unwrap_err()
            .to_string()
            .contains("register TUN reader failed"));
        assert_eq!(deletions.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn post_open_success_does_not_delete_link() {
        let deletions = AtomicUsize::new(0);

        let result = cleanup_after_failed_setup(Ok::<_, anyhow::Error>(7), || async {
            deletions.fetch_add(1, Ordering::SeqCst);
        })
        .await;

        assert_eq!(result.unwrap(), 7);
        assert_eq!(deletions.load(Ordering::SeqCst), 0);
    }

    fn test_lifetime() -> Arc<LinkLifetime> {
        Arc::new(LinkLifetime::new(
            "tun-test".to_string(),
            VirtualIps::from_ipv6(Ipv6Addr::LOCALHOST),
        ))
    }

    #[test]
    fn clone_lifetime_defers_delete_until_last_handle() {
        let original = test_lifetime();
        let clone = Arc::clone(&original);
        let deletions = AtomicUsize::new(0);

        assert!(!request_link_close(&original));
        assert!(original.close_requested.load(Ordering::Acquire));
        assert_eq!(deletions.load(Ordering::SeqCst), 0);

        drop(original);
        assert_eq!(Arc::strong_count(&clone), 1);
        assert_eq!(deletions.load(Ordering::SeqCst), 0);
        assert!(!clone.deletion_started.load(Ordering::Acquire));
        if request_link_close(&clone) && clone.begin_deletion() {
            deletions.fetch_add(1, Ordering::SeqCst);
        }

        assert_eq!(deletions.load(Ordering::SeqCst), 1);
        // The simulated link deletion already claimed the lifetime, so dropping
        // the final handle must not schedule another deletion.
        drop(clone);
        assert_eq!(deletions.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn link_deletion_is_idempotent() {
        let lifetime = test_lifetime();
        let deletions = AtomicUsize::new(0);

        for _ in 0..3 {
            if lifetime.begin_deletion() {
                deletions.fetch_add(1, Ordering::SeqCst);
            }
        }

        assert_eq!(deletions.load(Ordering::SeqCst), 1);
        drop(lifetime);
        assert_eq!(deletions.load(Ordering::SeqCst), 1);
    }
}
