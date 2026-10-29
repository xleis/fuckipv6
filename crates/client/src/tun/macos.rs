use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::net::Ipv6Addr;
use std::sync::Arc;
use tokio::sync::Mutex;
use tun::Device;

const UTUN_HEADER_LEN: usize = 4;
const UTUN_AF_INET6_HEADER: [u8; UTUN_HEADER_LEN] = [0, 0, 0, libc::AF_INET6 as u8];
const OVERLAY_ROUTE: &str = "fd00::/64";

pub struct MacosTun {
    reader: Arc<Mutex<tun::platform::posix::Reader>>,
    writer: Arc<Mutex<tun::platform::posix::Writer>>,
    name: String,
    requested_name: String,
    ip: Ipv6Addr,
    cleanup_on_drop: bool,
}

impl Clone for MacosTun {
    fn clone(&self) -> Self {
        Self {
            reader: self.reader.clone(),
            writer: self.writer.clone(),
            name: self.name.clone(),
            requested_name: self.requested_name.clone(),
            ip: self.ip,
            cleanup_on_drop: false,
        }
    }
}

impl MacosTun {
    pub async fn create_dual_stack(
        name: &str,
        virtual_ips: &fuckipv6_common::VirtualIps,
        _ipv4_prefix: u8,
        ipv6_prefix: u8,
        mtu: u16,
    ) -> Result<Self> {
        let ip = virtual_ips.primary_ipv6();
        Self::create(name, ip, ipv6_prefix, mtu).await
    }

    pub async fn create(name: &str, ip: Ipv6Addr, prefix_len: u8, mtu: u16) -> Result<Self> {
        tracing::info!("正在创建 macOS utun 设备: {}", name);

        validate_tun_inputs(name, mtu)?;

        let (device, actual_name) = open_tun_device(name, mtu).await?;
        configure_address(&actual_name, ip, prefix_len).await?;
        configure_route(&actual_name).await;

        let (reader, writer) = device.split();

        tracing::info!(
            "macOS TUN 设备已完全配置并打开: {} (requested: {})",
            actual_name,
            name
        );

        Ok(Self {
            reader: Arc::new(Mutex::new(reader)),
            writer: Arc::new(Mutex::new(writer)),
            name: actual_name,
            requested_name: name.to_string(),
            ip,
            cleanup_on_drop: true,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let reader = self.reader.clone();
        let len = buf.len();

        let data = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
            let mut reader = reader.blocking_lock();
            let mut temp_buf = vec![0u8; len.saturating_add(UTUN_HEADER_LEN)];
            let size = reader
                .read(&mut temp_buf)
                .context("Failed to read from macOS TUN device")?;
            temp_buf.truncate(size);
            strip_utun_header(&temp_buf)
                .map(|packet| packet.to_vec())
                .context("Invalid packet from macOS TUN device")
        })
        .await
        .context("macOS TUN read blocking task failed to join")??;

        if data.len() > buf.len() {
            anyhow::bail!("macOS TUN packet too large for receive buffer");
        }

        buf[..data.len()].copy_from_slice(&data);
        Ok(data.len())
    }

    pub async fn write(&mut self, packet: &[u8]) -> Result<()> {
        let writer = self.writer.clone();
        let packet = add_utun_header(packet)?;
        tokio::task::spawn_blocking(move || {
            let mut writer = writer.blocking_lock();
            writer
                .write_all(&packet)
                .context("Failed to write to macOS TUN device")
        })
        .await
        .context("macOS TUN write blocking task failed to join")?
    }

    pub async fn delete(&mut self) -> Result<()> {
        tracing::info!("正在删除 macOS TUN 配置: {}", self.name);

        delete_route(&self.name).await;
        delete_address(&self.name, self.ip).await;
        self.cleanup_on_drop = false;
        tracing::info!("macOS TUN 配置已删除: {}", self.name);

        Ok(())
    }
}

impl Drop for MacosTun {
    fn drop(&mut self) {
        if !self.cleanup_on_drop {
            return;
        }

        tracing::info!("正在清理 macOS TUN 配置: {}", self.name);
        let name = self.name.clone();
        let ip = self.ip;
        std::thread::spawn(move || {
            let _ = std::process::Command::new("route")
                .args(["-n", "delete", "-inet6", OVERLAY_ROUTE, "-interface", &name])
                .status();
            let _ = std::process::Command::new("ifconfig")
                .args([&name, "inet6", &ip.to_string(), "-alias"])
                .status();
        });
    }
}

fn validate_tun_inputs(name: &str, mtu: u16) -> Result<()> {
    if name.trim().is_empty() {
        anyhow::bail!("TUN 设备名称不能为空；macOS 可使用 utunN，或在配置中保留默认名称让系统分配");
    }

    if name != "fuckipv6" && !valid_utun_name(name) {
        anyhow::bail!("macOS TUN 设备名称必须是 utunN；默认名称 fuckipv6 会自动分配可用 utun");
    }

    if mtu < 1280 {
        anyhow::bail!("IPv6 TUN MTU 不能小于 1280");
    }

    Ok(())
}

fn valid_utun_name(name: &str) -> bool {
    let Some(suffix) = name.strip_prefix("utun") else {
        return false;
    };

    !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
}

async fn open_tun_device(name: &str, mtu: u16) -> Result<(tun::platform::Device, String)> {
    let requested_name = name.to_string();
    tokio::task::spawn_blocking(move || {
        let mut config = tun::Configuration::default();
        config.mtu(mtu as i32).up();

        if valid_utun_name(&requested_name) {
            config.name(&requested_name);
        }

        let device = tun::create(&config).context(classify_create_error(&requested_name))?;
        let actual_name = device.name().context("无法读取 macOS utun 接口名称")?;
        Ok((device, actual_name))
    })
    .await
    .context("macOS TUN open blocking task failed to join")?
}

fn classify_create_error(name: &str) -> String {
    if valid_utun_name(name) {
        format!("创建 macOS utun 设备失败: {name}")
    } else {
        "创建 macOS utun 设备失败：请使用 sudo/root 权限运行，或将 tun.name 配置为 utunN"
            .to_string()
    }
}

async fn configure_address(name: &str, ip: Ipv6Addr, prefix_len: u8) -> Result<()> {
    tracing::debug!("正在为网卡 {} 添加 IPv6 地址: {}/{}", name, ip, prefix_len);

    let output = tokio::process::Command::new("ifconfig")
        .args([
            name,
            "inet6",
            &ip.to_string(),
            "prefixlen",
            &prefix_len.to_string(),
            "alias",
        ])
        .output()
        .await
        .context("执行 ifconfig inet6 alias 命令失败")?;

    if output.status.success() {
        tracing::info!("IPv6 地址配置成功: {}/{}", ip, prefix_len);
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    anyhow::bail!("配置 macOS TUN IPv6 地址失败：{}", stderr.trim());
}

async fn configure_route(name: &str) {
    tracing::debug!(
        "正在为网卡 {} 添加 macOS IPv6 路由: {}",
        name,
        OVERLAY_ROUTE
    );

    let output = tokio::process::Command::new("route")
        .args(["-n", "add", "-inet6", OVERLAY_ROUTE, "-interface", name])
        .output()
        .await;

    match output {
        Ok(output) if output.status.success() => {
            tracing::info!("IPv6 路由配置成功: {}", OVERLAY_ROUTE);
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!(
                "配置 macOS IPv6 路由失败，退出码: {} (可能路由已存在): {}",
                output.status,
                stderr.trim()
            );
        }
        Err(err) => {
            tracing::warn!("执行 route 命令失败: {:?}", err);
        }
    }
}

async fn delete_route(name: &str) {
    let _ = tokio::process::Command::new("route")
        .args(["-n", "delete", "-inet6", OVERLAY_ROUTE, "-interface", name])
        .stderr(std::process::Stdio::null())
        .status()
        .await;
}

async fn delete_address(name: &str, ip: Ipv6Addr) {
    let _ = tokio::process::Command::new("ifconfig")
        .args([name, "inet6", &ip.to_string(), "-alias"])
        .stderr(std::process::Stdio::null())
        .status()
        .await;
}

fn strip_utun_header(frame: &[u8]) -> Result<&[u8]> {
    if frame.len() < UTUN_HEADER_LEN {
        anyhow::bail!("macOS utun frame is shorter than packet information header");
    }

    if frame[0] != 0 || frame[1] != 0 || frame[2] != 0 || frame[3] != libc::AF_INET6 as u8 {
        anyhow::bail!("macOS utun frame is not IPv6");
    }

    Ok(&frame[UTUN_HEADER_LEN..])
}

fn add_utun_header(packet: &[u8]) -> Result<Vec<u8>> {
    if packet.first().map(|byte| byte >> 4) != Some(6) {
        anyhow::bail!("macOS utun writer only supports IPv6 packets");
    }

    let mut frame = Vec::with_capacity(packet.len() + UTUN_HEADER_LEN);
    frame.extend_from_slice(&UTUN_AF_INET6_HEADER);
    frame.extend_from_slice(packet);
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_utun_names() {
        assert!(valid_utun_name("utun0"));
        assert!(valid_utun_name("utun42"));
        assert!(!valid_utun_name("utun"));
        assert!(!valid_utun_name("fuckipv6"));
        assert!(!valid_utun_name("utunx"));
    }

    #[test]
    fn strips_macos_utun_header() -> Result<()> {
        let packet = [0x60, 0, 0, 0, 1, 2, 3, 4];
        let mut frame = Vec::from(UTUN_AF_INET6_HEADER);
        frame.extend_from_slice(&packet);

        assert_eq!(strip_utun_header(&frame)?, packet);
        Ok(())
    }

    #[test]
    fn adds_macos_utun_header() -> Result<()> {
        let packet = [0x60, 0, 0, 0, 1, 2, 3, 4];
        let frame = add_utun_header(&packet)?;

        assert_eq!(&frame[..UTUN_HEADER_LEN], UTUN_AF_INET6_HEADER);
        assert_eq!(&frame[UTUN_HEADER_LEN..], packet);
        Ok(())
    }

    #[test]
    fn rejects_non_ipv6_packet_for_write() {
        let err = add_utun_header(&[0x45, 0, 0, 0]).unwrap_err().to_string();
        assert!(err.contains("IPv6"));
    }
}
