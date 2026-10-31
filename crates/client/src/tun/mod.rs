#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "windows")]
mod windows;

use anyhow::Result;
use fuckipv6_common::VirtualIps;
use std::net::Ipv6Addr;

/// TUN设备抽象
#[derive(Clone)]
pub struct TunDevice {
    #[cfg(target_os = "linux")]
    inner: linux::LinuxTun,

    #[cfg(target_os = "macos")]
    inner: macos::MacosTun,

    #[cfg(target_os = "windows")]
    inner: windows::WindowsTun,
}

impl TunDevice {
    /// 创建支持双栈/纯IPv4/纯IPv6的TUN设备
    pub async fn create_dual_stack(
        name: &str,
        virtual_ips: &VirtualIps,
        ipv4_prefix: u8,
        ipv6_prefix: u8,
        mtu: u16,
    ) -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            Ok(Self {
                inner: linux::LinuxTun::create_dual_stack(
                    name,
                    virtual_ips,
                    ipv4_prefix,
                    ipv6_prefix,
                    mtu,
                )
                .await?,
            })
        }

        #[cfg(target_os = "macos")]
        {
            Ok(Self {
                inner: macos::MacosTun::create_dual_stack(
                    name,
                    virtual_ips,
                    ipv4_prefix,
                    ipv6_prefix,
                    mtu,
                )
                .await?,
            })
        }

        #[cfg(target_os = "windows")]
        {
            Ok(Self {
                inner: windows::WindowsTun::create_dual_stack(
                    name,
                    virtual_ips,
                    ipv4_prefix,
                    ipv6_prefix,
                    mtu,
                )
                .await?,
            })
        }
    }

    /// 创建TUN设备 (兼容原有 IPv6 接口)
    pub async fn create(name: &str, ip: Ipv6Addr, prefix_len: u8, mtu: u16) -> Result<Self> {
        Self::create_dual_stack(name, &VirtualIps::from_ipv6(ip), 24, prefix_len, mtu).await
    }

    /// 获取TUN设备名称
    pub fn name(&self) -> &str {
        self.inner.name()
    }

    /// 读取数据包
    pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.inner.read(buf).await
    }

    /// Read one packet, then drain immediately available packets on platforms
    /// that expose a nonblocking readiness primitive. Other platforms retain
    /// the single-packet behavior.
    pub async fn read_batch(&mut self, buffers: &mut [&mut [u8]]) -> Result<Vec<usize>> {
        #[cfg(target_os = "linux")]
        {
            self.inner.read_batch(buffers).await
        }
        #[cfg(not(target_os = "linux"))]
        {
            let Some(first) = buffers.first_mut() else {
                return Ok(Vec::new());
            };
            Ok(vec![self.inner.read(first).await?])
        }
    }

    /// 写入数据包
    pub async fn write(&mut self, packet: &[u8]) -> Result<()> {
        self.inner.write(packet).await
    }

    /// 删除TUN设备
    pub async fn delete(&mut self) -> Result<()> {
        self.inner.delete().await
    }
}
