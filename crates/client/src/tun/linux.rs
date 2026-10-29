use anyhow::{Context, Result};
use fuckipv6_common::VirtualIps;
use std::fs::File;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::os::fd::{AsRawFd, FromRawFd};
use std::sync::Arc;
use tokio::io::unix::AsyncFd;
use tokio::io::Interest;

pub struct LinuxTun {
    reader: Arc<AsyncFd<tun::platform::posix::Reader>>,
    writer: Arc<AsyncFd<File>>,
    name: String,
    virtual_ips: VirtualIps,
    cleanup_on_drop: bool,
}

impl Clone for LinuxTun {
    fn clone(&self) -> Self {
        Self {
            reader: self.reader.clone(),
            writer: self.writer.clone(),
            name: self.name.clone(),
            virtual_ips: self.virtual_ips.clone(),
            cleanup_on_drop: false,
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
            open_tun_device(name).await
        }
        .await;

        let device = match setup_result {
            Ok(device) => device,
            Err(e) => {
                let _ = delete_link(name).await;
                return Err(e);
            }
        };

        let (reader, split_writer) = device.split();
        drop(split_writer);
        set_nonblocking(reader.as_raw_fd()).context("设置 TUN 为非阻塞模式失败")?;
        let writer_fd = duplicate_fd(reader.as_raw_fd()).context("复制 TUN 文件描述符失败")?;
        // SAFETY: duplicate_fd returned a new owned descriptor, and File assumes
        // responsibility for closing it exactly once.
        let writer_file = unsafe { File::from_raw_fd(writer_fd) };

        tracing::info!("TUN 设备已完全配置并打开: {}", name);

        Ok(Self {
            reader: Arc::new(AsyncFd::new(reader).context("注册 TUN 读事件失败")?),
            writer: Arc::new(AsyncFd::new(writer_file).context("注册 TUN 写事件失败")?),
            name: name.to_string(),
            virtual_ips: virtual_ips.clone(),
            cleanup_on_drop: true,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    #[allow(dead_code)]
    pub fn virtual_ips(&self) -> &VirtualIps {
        &self.virtual_ips
    }

    #[allow(dead_code)]
    pub fn ip(&self) -> Ipv6Addr {
        self.virtual_ips.primary_ipv6()
    }

    pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.reader
            .async_io(Interest::READABLE, |reader| {
                read_fd(reader.as_raw_fd(), buf)
            })
            .await
            .context("Failed to read from TUN device")
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

    /// 删除 TUN 设备
    pub async fn delete(&mut self) -> Result<()> {
        tracing::info!("正在删除 TUN 设备: {}", self.name);

        if let Some(ipv6) = self.virtual_ips.ipv6 {
            let addr_cmd = format!("{}/128", ipv6);
            let _ = tokio::process::Command::new("ip")
                .args(["-6", "addr", "del", &addr_cmd, "dev", &self.name])
                .stderr(std::process::Stdio::null())
                .status()
                .await;
        }

        if let Some(ipv4) = self.virtual_ips.ipv4 {
            let addr_cmd = format!("{}/32", ipv4);
            let _ = tokio::process::Command::new("ip")
                .args(["addr", "del", &addr_cmd, "dev", &self.name])
                .stderr(std::process::Stdio::null())
                .status()
                .await;
        }

        delete_link(&self.name).await?;
        self.cleanup_on_drop = false;
        tracing::info!("TUN 设备已删除");

        Ok(())
    }
}

impl Drop for LinuxTun {
    fn drop(&mut self) {
        if !self.cleanup_on_drop {
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
}
