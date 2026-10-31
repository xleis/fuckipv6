//! Linux AF_PACKET and TUN implementation for LXT handshake rescue.

use std::collections::{HashMap, VecDeque};
use std::ffi::CString;
use std::fs::File;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::io::unix::AsyncFd;
use tokio::net::UdpSocket;
use tracing::{debug, info, warn};
use zeroize::Zeroizing;

use super::packet::{parse_ethernet_ipv4_tcp, parse_ipv4_tcp, rewrite_synack};
use super::protocol::{MessageKind, Protocol};
use super::{ClientConfig, ServerConfig};

const ETH_P_ALL: u16 = 0x0003;
const PACKET_OUTGOING: u8 = 4;
const IFF_TUN: libc::c_short = 0x0001;
const IFF_NO_PI: libc::c_short = 0x1000;
const TUNSETIFF: libc::c_ulong = 0x4004_54ca;
const IF_NAMESIZE: usize = 16;
const IFREQ_SIZE: usize = 40;
const PEER_TTL: Duration = Duration::from_secs(8);
const PENDING_SYN_TTL: Duration = Duration::from_secs(20);
// Keep this short: a SYN-ACK retransmitted after ~1s must not be suppressed.
const RELAY_DEDUP_TTL: Duration = Duration::from_millis(250);
const MAX_PENDING_SYNS: usize = 1024;
const MAX_RELAY_DEDUP_ENTRIES: usize = 8192;

#[repr(C)]
struct IfReq {
    name: [u8; IF_NAMESIZE],
    union: [u8; IFREQ_SIZE - IF_NAMESIZE],
}

pub(super) async fn run_server(
    config: ServerConfig,
    key: Zeroizing<Vec<u8>>,
    max_udp_datagram: usize,
) -> Result<()> {
    let capture = PacketCapture::open(&config.interface)?;
    let udp = UdpSocket::bind(SocketAddrV4::new(config.bind, config.udp_port))
        .await
        .with_context(|| format!("绑定 LXT 救援 UDP {}:{} 失败", config.bind, config.udp_port))?;
    let local_addr = udp.local_addr().context("读取 LXT UDP 本地地址失败")?;
    info!(
        interface = %config.interface,
        udp = %local_addr,
        tcp_port = config.tcp_port,
        "LXT 救援服务端已启动；等待已认证客户端 HELLO"
    );

    let mut protocol = Protocol::new(key);
    let mut peers: HashMap<Ipv4Addr, UdpPeer> = HashMap::new();
    let mut relay_dedup: HashMap<(Ipv4Addr, u16, u32), Instant> = HashMap::new();
    let mut datagram = vec![0_u8; max_udp_datagram + 1];
    let shutdown = wait_for_shutdown();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            _ = &mut shutdown => {
                info!("LXT 救援服务端收到退出信号");
                break;
            }
            result = capture.recv() => {
                let (frame, packet_type) = result.context("读取 LXT AF_PACKET 抓包失败")?;
                if packet_type != PACKET_OUTGOING {
                    continue;
                }
                let Ok(segment) = parse_ethernet_ipv4_tcp(&frame) else {
                    continue;
                };
                if segment.source_port != config.tcp_port || !segment.is_syn_ack() {
                    continue;
                }

                let now = Instant::now();
                peers.retain(|_, peer| now.duration_since(peer.last_seen) <= PEER_TTL);
                let Some(peer) = peers.get(&segment.destination) else {
                    debug!(
                        client = %segment.destination,
                        port = segment.destination_port,
                        "捕获到 SYN-ACK，但没有有效的已认证 UDP peer"
                    );
                    continue;
                };

                relay_dedup.retain(|_, seen| now.duration_since(*seen) <= RELAY_DEDUP_TTL);
                let relay_key = (segment.destination, segment.destination_port, segment.sequence);
                if relay_dedup.contains_key(&relay_key) {
                    continue;
                }
                if relay_dedup.len() >= MAX_RELAY_DEDUP_ENTRIES {
                    relay_dedup.clear();
                }
                relay_dedup.insert(relay_key, now);

                let packet = segment.packet.to_vec();
                let message = protocol
                    .encode(MessageKind::SynAck, &packet)
                    .context("编码 LXT SYN-ACK 控制消息失败")?;
                if let Err(error) = udp.send_to(&message, peer.endpoint).await {
                    warn!(
                        peer = %peer.endpoint,
                        error = %error,
                        "发送 LXT SYN-ACK 失败"
                    );
                    continue;
                }
                info!(
                    client = %segment.destination,
                    client_port = segment.destination_port,
                    tcp_seq = segment.sequence,
                    udp_peer = %peer.endpoint,
                    "已通过认证 UDP 通道转发真实 SYN-ACK"
                );
            }
            result = udp.recv_from(&mut datagram) => {
                let (len, source) = result.context("读取 LXT UDP 数据报失败")?;
                if len > max_udp_datagram {
                    debug!(peer = %source, len, "丢弃超长 LXT UDP 数据报");
                    continue;
                }
                let message = match protocol.decode(&datagram[..len]) {
                    Ok(message) => message,
                    Err(error) => {
                        debug!(peer = %source, error = %error, "拒绝未通过认证的 LXT UDP 数据报");
                        continue;
                    }
                };
                if message.kind != MessageKind::Hello {
                    debug!(peer = %source, kind = ?message.kind, "忽略非 HELLO 的 LXT 服务端消息");
                    continue;
                }
                if !message.body.is_empty() {
                    debug!(peer = %source, "忽略带负载的 LXT HELLO");
                    continue;
                }
                let IpAddr::V4(client_ip) = source.ip() else {
                    continue;
                };
                let now = Instant::now();
                let new_peer = peers
                    .get(&client_ip)
                    .is_none_or(|peer| peer.endpoint != source);
                peers.insert(
                    client_ip,
                    UdpPeer {
                        endpoint: source,
                        last_seen: now,
                    },
                );
                if new_peer {
                    info!(client = %client_ip, udp_peer = %source, "LXT UDP peer 认证成功");
                }
                let ack = protocol
                    .encode(MessageKind::HelloAck, &[])
                    .context("编码 LXT HELLO_ACK 失败")?;
                if let Err(error) = udp.send_to(&ack, source).await {
                    warn!(peer = %source, error = %error, "发送 LXT HELLO_ACK 失败");
                }
            }
        }
    }

    Ok(())
}

pub(super) async fn run_client(
    config: ClientConfig,
    key: Zeroizing<Vec<u8>>,
    max_udp_datagram: usize,
) -> Result<()> {
    let capture = PacketCapture::open(&config.interface)?;
    let tun = RescueTun::create(&config.tun_name).await?;
    let udp = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
        .await
        .context("绑定 LXT 客户端 UDP socket 失败")?;
    let local_addr = udp.local_addr().context("读取 LXT 客户端 UDP 地址失败")?;
    let endpoint = SocketAddrV4::new(config.server, config.udp_port);
    info!(
        interface = %config.interface,
        tun = %config.tun_name,
        udp = %local_addr,
        server = %endpoint,
        tcp_port = config.tcp_port,
        "LXT 救援客户端已启动；等待发往服务端的 TCP SYN"
    );

    let mut protocol = Protocol::new(key);
    let mut pending: VecDeque<PendingSyn> = VecDeque::new();
    let mut datagram = vec![0_u8; max_udp_datagram + 1];
    let mut hello_interval = tokio::time::interval(config.hello_interval);
    hello_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let shutdown = wait_for_shutdown();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            _ = &mut shutdown => {
                info!("LXT 救援客户端收到退出信号");
                break;
            }
            _ = hello_interval.tick() => {
                let hello = protocol
                    .encode(MessageKind::Hello, &[])
                    .context("编码 LXT HELLO 失败")?;
                if let Err(error) = udp.send_to(&hello, endpoint).await {
                    warn!(server = %endpoint, error = %error, "发送 LXT HELLO 失败");
                } else {
                    debug!(server = %endpoint, "已发送 LXT HELLO");
                }
            }
            result = capture.recv() => {
                let (frame, packet_type) = result.context("读取 LXT AF_PACKET 抓包失败")?;
                if packet_type != PACKET_OUTGOING {
                    continue;
                }
                let Ok(segment) = parse_ethernet_ipv4_tcp(&frame) else {
                    continue;
                };
                if segment.destination != config.server
                    || segment.destination_port != config.tcp_port
                    || !segment.is_syn()
                {
                    continue;
                }

                let now = Instant::now();
                pending.retain(|item| now.duration_since(item.last_seen) < PENDING_SYN_TTL);
                if let Some(existing) = pending.iter_mut().find(|item| {
                    item.local_ip == segment.source
                        && item.local_port == segment.source_port
                        && item.isn == segment.sequence
                }) {
                    existing.last_seen = now;
                } else {
                    if pending.len() >= MAX_PENDING_SYNS {
                        pending.pop_front();
                    }
                    pending.push_back(PendingSyn {
                        local_ip: segment.source,
                        local_port: segment.source_port,
                        isn: segment.sequence,
                        last_seen: now,
                    });
                    info!(
                        local = %segment.source,
                        local_port = segment.source_port,
                        isn = segment.sequence,
                        "记录待救援 TCP SYN"
                    );
                }
            }
            result = udp.recv_from(&mut datagram) => {
                let (len, source) = result.context("读取 LXT 客户端 UDP 数据报失败")?;
                if source != SocketAddr::V4(endpoint) {
                    debug!(peer = %source, "忽略非预期来源的 LXT UDP 数据报");
                    continue;
                }
                if len > max_udp_datagram {
                    debug!(peer = %source, len, "丢弃超长 LXT UDP 数据报");
                    continue;
                }
                let message = match protocol.decode(&datagram[..len]) {
                    Ok(message) => message,
                    Err(error) => {
                        debug!(peer = %source, error = %error, "拒绝未通过认证的 LXT UDP 数据报");
                        continue;
                    }
                };
                match message.kind {
                    MessageKind::HelloAck => {
                        if !message.body.is_empty() {
                            debug!(peer = %source, "忽略带负载的 LXT HELLO_ACK");
                        } else {
                            debug!(peer = %source, "收到 LXT HELLO_ACK");
                        }
                    }
                    MessageKind::SynAck => {
                        let Ok(segment) = parse_ipv4_tcp(&message.body) else {
                            debug!(peer = %source, "忽略格式错误的 LXT SYN-ACK 负载");
                            continue;
                        };
                        if segment.source_port != config.tcp_port || !segment.is_syn_ack() {
                            debug!(peer = %source, "忽略不符合端口/flags 约束的 LXT SYN-ACK");
                            continue;
                        }
                        let now = Instant::now();
                        pending.retain(|item| now.duration_since(item.last_seen) < PENDING_SYN_TTL);
                        let Some(index) = pending.iter().rposition(|item| {
                            item.isn.wrapping_add(1) == segment.acknowledgment
                        }) else {
                            debug!(ack = segment.acknowledgment, "没有匹配的待救援 TCP SYN");
                            continue;
                        };
                        let pending_syn = pending[index].clone();
                        let mut packet = message.body;
                        rewrite_synack(
                            &mut packet,
                            config.server,
                            pending_syn.local_ip,
                            pending_syn.local_port,
                            config.tcp_port,
                        )
                        .context("改写 LXT SYN-ACK 地址和校验和失败")?;
                        tun.write(&packet).await.context("向 LXT 救援 TUN 注入 SYN-ACK 失败")?;
                        pending.remove(index);
                        info!(
                            local = %pending_syn.local_ip,
                            local_port = pending_syn.local_port,
                            isn = pending_syn.isn,
                            bytes = packet.len(),
                            "已向 TUN 注入救援 SYN-ACK"
                        );
                    }
                    MessageKind::Hello => {
                        debug!(peer = %source, "忽略客户端不接受的 LXT HELLO");
                    }
                }
            }
        }
    }

    Ok(())
}

struct UdpPeer {
    endpoint: SocketAddr,
    last_seen: Instant,
}

#[derive(Clone)]
struct PendingSyn {
    local_ip: Ipv4Addr,
    local_port: u16,
    isn: u32,
    last_seen: Instant,
}

struct PacketCapture {
    fd: AsyncFd<OwnedFd>,
}

impl PacketCapture {
    fn open(interface: &str) -> Result<Self> {
        let interface_c = CString::new(interface)
            .with_context(|| format!("LXT 抓包网卡名称包含 NUL 字符: {interface}"))?;
        // SAFETY: `interface_c` is a valid NUL-terminated string for the call.
        let ifindex = unsafe { libc::if_nametoindex(interface_c.as_ptr()) };
        if ifindex == 0 {
            return Err(io::Error::last_os_error())
                .with_context(|| format!("找不到 LXT 抓包网卡: {interface}"));
        }

        // SAFETY: socket is called with Linux AF_PACKET/SOCK_RAW constants and
        // returns a new descriptor or -1.
        let fd = unsafe {
            libc::socket(
                libc::AF_PACKET,
                libc::SOCK_RAW | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                i32::from(ETH_P_ALL.to_be()),
            )
        };
        if fd < 0 {
            return Err(classify_raw_socket_error(io::Error::last_os_error()));
        }
        // SAFETY: fd was just returned by socket and is owned from this point.
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };

        // SAFETY: zero is a valid initialization for sockaddr_ll before setting
        // every field required by bind.
        let mut address: libc::sockaddr_ll = unsafe { std::mem::zeroed() };
        address.sll_family = libc::AF_PACKET as u16;
        address.sll_protocol = ETH_P_ALL.to_be();
        address.sll_ifindex = ifindex as i32;
        // SAFETY: `address` points to a fully initialized sockaddr_ll and the
        // supplied length matches that structure.
        let result = unsafe {
            libc::bind(
                owned.as_raw_fd(),
                (&address as *const libc::sockaddr_ll).cast(),
                std::mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t,
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error())
                .with_context(|| format!("绑定 LXT AF_PACKET 网卡失败: {interface}"));
        }

        // SAFETY: `owned` remains open and unchanged for the complete lifetime
        // of the AsyncFd registration.
        let fd =
            unsafe { AsyncFd::register(owned) }.context("注册 LXT AF_PACKET 非阻塞读取失败")?;
        Ok(Self { fd })
    }

    async fn recv(&self) -> io::Result<(Vec<u8>, u8)> {
        loop {
            let mut readiness = self.fd.readable().await?;
            let result = readiness.try_io(|inner| recv_packet(inner.get_ref().as_raw_fd()));
            match result {
                Ok(Ok(packet)) => return Ok(packet),
                Ok(Err(error)) if error.kind() == io::ErrorKind::WouldBlock => continue,
                Ok(Err(error)) => return Err(error),
                Err(_) => continue,
            }
        }
    }
}

fn recv_packet(fd: libc::c_int) -> io::Result<(Vec<u8>, u8)> {
    let mut buffer = vec![0_u8; 65_535];
    // SAFETY: zero is a valid initialization for sockaddr_ll written by recvfrom.
    let mut address: libc::sockaddr_ll = unsafe { std::mem::zeroed() };
    let mut address_len = std::mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t;
    // SAFETY: all pointers reference writable storage of the supplied lengths.
    let len = unsafe {
        libc::recvfrom(
            fd,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            libc::MSG_TRUNC,
            (&mut address as *mut libc::sockaddr_ll).cast(),
            &mut address_len,
        )
    };
    if len < 0 {
        return Err(io::Error::last_os_error());
    }
    let len = usize::try_from(len).expect("recvfrom length is non-negative");
    buffer.truncate(len.min(buffer.len()));
    Ok((buffer, address.sll_pkttype))
}

fn classify_raw_socket_error(error: io::Error) -> anyhow::Error {
    match error.raw_os_error() {
        Some(libc::EPERM) | Some(libc::EACCES) => anyhow::anyhow!(
            "创建 LXT AF_PACKET RAW socket 失败：权限不足，请使用 root 或授予 CAP_NET_RAW"
        ),
        _ => anyhow::anyhow!("创建 LXT AF_PACKET RAW socket 失败: {error}"),
    }
}

struct RescueTun {
    fd: AsyncFd<File>,
}

impl RescueTun {
    async fn create(name: &str) -> Result<Self> {
        let path = CString::new("/dev/net/tun").expect("static TUN path has no NUL");
        // SAFETY: path is a valid C string; open returns a new descriptor or -1.
        let raw_fd = unsafe {
            libc::open(
                path.as_ptr(),
                libc::O_RDWR | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if raw_fd < 0 {
            return Err(classify_tun_open_error(io::Error::last_os_error()));
        }

        let mut request = IfReq {
            name: [0_u8; IF_NAMESIZE],
            union: [0_u8; IFREQ_SIZE - IF_NAMESIZE],
        };
        request.name[..name.len()].copy_from_slice(name.as_bytes());
        request.union[..2].copy_from_slice(&(IFF_TUN | IFF_NO_PI).to_ne_bytes());
        // SAFETY: the request is at least as large as the kernel's ifreq and
        // remains valid for the duration of the ioctl.
        let result = unsafe { libc::ioctl(raw_fd, TUNSETIFF, &mut request as *mut IfReq) };
        if result < 0 {
            let error = io::Error::last_os_error();
            // SAFETY: raw_fd was returned by open and has not been transferred.
            unsafe { libc::close(raw_fd) };
            return Err(error).with_context(|| {
                format!("创建 LXT 救援 TUN `{name}` 失败；请确认 /dev/net/tun 可用且名称未被占用")
            });
        }

        // SAFETY: raw_fd is a newly opened descriptor whose ownership is moved
        // into File exactly once.
        let file = unsafe { File::from_raw_fd(raw_fd) };
        // SAFETY: `file` owns the descriptor and keeps it open and unchanged for
        // the complete lifetime of the AsyncFd registration.
        let fd = unsafe { AsyncFd::register(file) }.context("注册 LXT 救援 TUN 非阻塞写入失败")?;

        let output = tokio::process::Command::new("ip")
            .args(["link", "set", "dev", name, "up"])
            .output()
            .await
            .context("执行 ip link set 启用 LXT 救援 TUN 失败")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("启用 LXT 救援 TUN `{name}` 失败：{}", stderr.trim());
        }

        info!(tun = name, "LXT 救援 TUN 已创建并启用");
        Ok(Self { fd })
    }

    async fn write(&self, packet: &[u8]) -> Result<()> {
        loop {
            let mut readiness = self.fd.writable().await?;
            let result =
                readiness.try_io(|inner| write_packet(inner.get_ref().as_raw_fd(), packet));
            match result {
                Ok(Ok(written)) if written == packet.len() => return Ok(()),
                Ok(Ok(written)) => {
                    anyhow::bail!("LXT 救援 TUN 短写：仅写入 {written}/{} 字节", packet.len());
                }
                Ok(Err(error)) if error.kind() == io::ErrorKind::WouldBlock => continue,
                Ok(Err(error)) => return Err(error).context("写入 LXT 救援 TUN 失败"),
                Err(_) => continue,
            }
        }
    }
}

fn write_packet(fd: libc::c_int, packet: &[u8]) -> io::Result<usize> {
    // SAFETY: packet points to readable storage for packet.len() bytes.
    let written = unsafe { libc::write(fd, packet.as_ptr().cast(), packet.len()) };
    if written < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(written as usize)
}

fn classify_tun_open_error(error: io::Error) -> anyhow::Error {
    match error.raw_os_error() {
        Some(libc::EPERM) | Some(libc::EACCES) => {
            anyhow::anyhow!("打开 /dev/net/tun 失败：权限不足，请使用 root 或授予 CAP_NET_ADMIN")
        }
        Some(libc::ENOENT) => {
            anyhow::anyhow!("打开 /dev/net/tun 失败：设备不存在，请加载 tun 内核模块")
        }
        _ => anyhow::anyhow!("打开 /dev/net/tun 失败: {error}"),
    }
}

async fn wait_for_shutdown() -> Result<()> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("注册 SIGTERM 处理器失败")?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result.context("注册 Ctrl+C 处理器失败")?,
        _ = terminate.recv() => {},
    }
    Ok(())
}
