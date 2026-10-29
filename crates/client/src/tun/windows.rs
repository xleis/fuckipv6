use anyhow::{Context, Result};
use std::net::Ipv6Addr;
use std::sync::Arc;
use wintun::Session;

pub struct WindowsTun {
    session: Arc<Session>,
    name: String,
    ip: Ipv6Addr,
}

impl Clone for WindowsTun {
    fn clone(&self) -> Self {
        Self {
            session: self.session.clone(),
            name: self.name.clone(),
            ip: self.ip,
        }
    }
}

impl WindowsTun {
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

    pub async fn create(name: &str, ip: Ipv6Addr, _prefix_len: u8, mtu: u16) -> Result<Self> {
        tracing::info!("正在创建 Windows TUN 设备: {}", name);

        // 检查管理员权限
        if !is_elevated() {
            anyhow::bail!("需要管理员权限才能创建 TUN 设备。请以管理员身份运行。");
        }

        // 加载 WinTun 驱动（优先环境已有驱动，缺失时自动解压并加载内嵌驱动，实现独立自主无需外部 DLL）
        let wintun = load_wintun().context("无法加载 WinTun 驱动")?;

        tracing::info!("WinTun 驱动已就绪");

        // 创建或打开适配器
        let adapter = match wintun::Adapter::open(&wintun, name) {
            Ok(adapter) => {
                tracing::info!("使用现有的 WinTun 适配器: {}", name);
                adapter
            }
            Err(_) => {
                tracing::info!("创建新的 WinTun 适配器: {}", name);
                wintun::Adapter::create(&wintun, name, "fuckipv6", None)
                    .context("无法创建 WinTun 适配器")?
            }
        };

        // 设置 MTU
        adapter.set_mtu(mtu as usize).context("无法设置 MTU")?;
        tracing::info!("WinTun 适配器 MTU 已设置为: {}", mtu);

        // 创建会话
        let session = Arc::new(
            adapter
                .start_session(wintun::MAX_RING_CAPACITY)
                .context("无法启动 WinTun 会话")?,
        );
        tracing::info!("WinTun 会话已启动");

        // 使用纯 Rust Windows IP Helper API 配置 IPv6 地址和路由，彻底消除外部 netsh.exe 依赖
        ip_helper::configure_ipv6_address(name, ip, 64)?;
        let prefix = "fd00::"
            .parse::<Ipv6Addr>()
            .unwrap_or(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 0));
        ip_helper::configure_ipv6_route(name, prefix, 64)?;

        tracing::info!("TUN 设备已完全配置并打开: {}", name);

        Ok(Self {
            session,
            name: name.to_string(),
            ip,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let session = self.session.clone();
        let len = buf.len();

        let (size, data) = tokio::task::spawn_blocking(move || -> Result<(usize, Vec<u8>)> {
            let packet = session
                .receive_blocking()
                .context("从 WinTun 读取数据失败")?;

            let packet_bytes = packet.bytes();
            let size = packet_bytes.len().min(len);
            Ok((size, packet_bytes[..size].to_vec()))
        })
        .await
        .context("WinTun read blocking task failed to join")??;

        buf[..size].copy_from_slice(&data[..size]);
        Ok(size)
    }

    pub async fn write(&mut self, packet_data: &[u8]) -> Result<()> {
        let session = self.session.clone();
        let data = packet_data.to_vec();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut packet = session
                .allocate_send_packet(data.len() as u16)
                .context("分配 WinTun 发送缓冲区失败")?;

            packet.bytes_mut().copy_from_slice(&data);
            session.send_packet(packet);
            Ok(())
        })
        .await
        .context("WinTun write blocking task failed to join")?
    }

    /// 删除 TUN 设备
    pub async fn delete(&mut self) -> Result<()> {
        tracing::info!("正在清理 TUN 设备: {}", self.name);

        // 使用纯 Rust IP Helper API 删除 IPv6 地址与路由
        let _ = ip_helper::delete_ipv6_address(&self.name, self.ip);
        let prefix = "fd00::"
            .parse::<Ipv6Addr>()
            .unwrap_or(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 0));
        let _ = ip_helper::delete_ipv6_route(&self.name, prefix, 64);

        tracing::info!("TUN 设备清理完成");
        Ok(())
    }
}

impl Drop for WindowsTun {
    fn drop(&mut self) {
        tracing::info!("清理 TUN 设备资源: {}", self.name);
    }
}

/// 独立自主的 WinTun 驱动加载器
fn load_wintun() -> Result<wintun::Wintun> {
    // 1. 尝试直接从系统已安装环境或同级目录加载
    if let Ok(wintun) = unsafe { wintun::load() } {
        tracing::debug!("已从系统环境加载已有的 wintun");
        return Ok(wintun);
    }

    // 2. 独立自主：从二进制内嵌的官方 WHQL 签名驱动直接释放到临时目录，用户无需外部下载或提供任何 DLL
    let temp_dll_path = std::env::temp_dir().join("fuckipv6_wintun.dll");

    #[cfg(target_arch = "x86_64")]
    const WINTUN_DLL_BYTES: &[u8] = include_bytes!("embedded/wintun_amd64.dll");
    #[cfg(target_arch = "aarch64")]
    const WINTUN_DLL_BYTES: &[u8] = include_bytes!("embedded/wintun_arm64.dll");
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    const WINTUN_DLL_BYTES: &[u8] = &[];

    if !WINTUN_DLL_BYTES.is_empty() {
        if !temp_dll_path.exists()
            || std::fs::metadata(&temp_dll_path)
                .map(|m| m.len())
                .unwrap_or(0)
                != WINTUN_DLL_BYTES.len() as u64
        {
            let _ = std::fs::write(&temp_dll_path, WINTUN_DLL_BYTES);
        }
        if let Ok(wintun) = unsafe { wintun::load_from_path(&temp_dll_path) } {
            tracing::info!(
                "使用内嵌独立自主 WinTun 驱动成功: {}",
                temp_dll_path.display()
            );
            return Ok(wintun);
        }
    }

    anyhow::bail!("无法初始化 Windows TUN 驱动环境。请确保拥有管理员权限。")
}

/// 纯 Rust Windows IP Helper API 实现（完全替代 netsh.exe）
mod ip_helper {
    use anyhow::Result;
    use std::net::Ipv6Addr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::NetworkManagement::IpHelper::*;
    use windows_sys::Win32::NetworkManagement::Ndis::NET_LUID_LH;
    use windows_sys::Win32::Networking::WinSock::*;

    pub fn name_to_luid(name: &str) -> Result<NET_LUID_LH> {
        let wide: Vec<u16> = std::ffi::OsStr::new(name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut luid: NET_LUID_LH = unsafe { std::mem::zeroed() };
        let ret = unsafe { ConvertInterfaceNameToLuidW(wide.as_ptr(), &mut luid) };
        if ret != NO_ERROR {
            anyhow::bail!("无法获取网卡 {} 的 LUID (错误码: {})", name, ret);
        }
        Ok(luid)
    }

    pub fn configure_ipv6_address(name: &str, ip: Ipv6Addr, prefix_len: u8) -> Result<()> {
        let luid = name_to_luid(name)?;
        let mut row: MIB_UNICASTIPADDRESS_ROW = unsafe { std::mem::zeroed() };
        unsafe { InitializeUnicastIpAddressEntry(&mut row) };

        row.InterfaceLuid = luid;
        row.Address.si_family = AF_INET6;
        row.Address.Ipv6.sin6_family = AF_INET6;
        row.Address.Ipv6.sin6_addr.u.Byte = ip.octets();
        row.OnLinkPrefixLength = prefix_len;

        let ret = unsafe { CreateUnicastIpAddressEntry(&row) };
        if ret == NO_ERROR || ret == ERROR_OBJECT_ALREADY_EXISTS {
            tracing::info!(
                "IPv6 地址配置成功 (纯 Rust IP Helper): {}/{}",
                ip,
                prefix_len
            );
            Ok(())
        } else {
            anyhow::bail!("添加 IPv6 地址失败 (错误码: {})", ret);
        }
    }

    pub fn delete_ipv6_address(name: &str, ip: Ipv6Addr) -> Result<()> {
        let luid = match name_to_luid(name) {
            Ok(luid) => luid,
            Err(_) => return Ok(()),
        };
        let mut row: MIB_UNICASTIPADDRESS_ROW = unsafe { std::mem::zeroed() };
        unsafe { InitializeUnicastIpAddressEntry(&mut row) };

        row.InterfaceLuid = luid;
        row.Address.si_family = AF_INET6;
        row.Address.Ipv6.sin6_family = AF_INET6;
        row.Address.Ipv6.sin6_addr.u.Byte = ip.octets();

        let ret = unsafe { DeleteUnicastIpAddressEntry(&row) };
        if ret != NO_ERROR && ret != ERROR_NOT_FOUND && ret != ERROR_FILE_NOT_FOUND {
            tracing::warn!("删除 IPv6 地址返回代码: {}", ret);
        }
        Ok(())
    }

    pub fn configure_ipv6_route(name: &str, prefix: Ipv6Addr, prefix_len: u8) -> Result<()> {
        let luid = name_to_luid(name)?;
        let mut row: MIB_IPFORWARD_ROW2 = unsafe { std::mem::zeroed() };
        unsafe { InitializeIpForwardEntry(&mut row) };

        row.InterfaceLuid = luid;
        row.DestinationPrefix.Prefix.si_family = AF_INET6;
        row.DestinationPrefix.Prefix.Ipv6.sin6_family = AF_INET6;
        row.DestinationPrefix.Prefix.Ipv6.sin6_addr.u.Byte = prefix.octets();
        row.DestinationPrefix.PrefixLength = prefix_len;
        row.Metric = 256;

        let ret = unsafe { CreateIpForwardEntry2(&row) };
        if ret == NO_ERROR || ret == ERROR_OBJECT_ALREADY_EXISTS {
            tracing::info!(
                "IPv6 路由配置成功 (纯 Rust IP Helper): {}/{}",
                prefix,
                prefix_len
            );
            Ok(())
        } else {
            anyhow::bail!("添加 IPv6 路由失败 (错误码: {})", ret);
        }
    }

    pub fn delete_ipv6_route(name: &str, prefix: Ipv6Addr, prefix_len: u8) -> Result<()> {
        let luid = match name_to_luid(name) {
            Ok(luid) => luid,
            Err(_) => return Ok(()),
        };
        let mut row: MIB_IPFORWARD_ROW2 = unsafe { std::mem::zeroed() };
        unsafe { InitializeIpForwardEntry(&mut row) };

        row.InterfaceLuid = luid;
        row.DestinationPrefix.Prefix.si_family = AF_INET6;
        row.DestinationPrefix.Prefix.Ipv6.sin6_family = AF_INET6;
        row.DestinationPrefix.Prefix.Ipv6.sin6_addr.u.Byte = prefix.octets();
        row.DestinationPrefix.PrefixLength = prefix_len;

        let ret = unsafe { DeleteIpForwardEntry2(&row) };
        if ret != NO_ERROR && ret != ERROR_NOT_FOUND && ret != ERROR_FILE_NOT_FOUND {
            tracing::warn!("删除 IPv6 路由返回代码: {}", ret);
        }
        Ok(())
    }
}

/// 检查是否以管理员权限运行
#[cfg(windows)]
fn is_elevated() -> bool {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }

        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut size = 0;

        let result = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut _ as *mut _,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        );

        windows_sys::Win32::Foundation::CloseHandle(token);

        result != 0 && elevation.TokenIsElevated != 0
    }
}

#[cfg(not(windows))]
fn is_elevated() -> bool {
    false
}
