use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

pub trait ToIpAddr {
    fn to_ip_addr(&self) -> IpAddr;
}

impl ToIpAddr for IpAddr {
    fn to_ip_addr(&self) -> IpAddr {
        *self
    }
}

impl ToIpAddr for &IpAddr {
    fn to_ip_addr(&self) -> IpAddr {
        **self
    }
}

impl ToIpAddr for Ipv6Addr {
    fn to_ip_addr(&self) -> IpAddr {
        IpAddr::V6(*self)
    }
}

impl ToIpAddr for &Ipv6Addr {
    fn to_ip_addr(&self) -> IpAddr {
        IpAddr::V6(**self)
    }
}

impl ToIpAddr for std::net::Ipv4Addr {
    fn to_ip_addr(&self) -> IpAddr {
        IpAddr::V4(*self)
    }
}

impl ToIpAddr for &std::net::Ipv4Addr {
    fn to_ip_addr(&self) -> IpAddr {
        IpAddr::V4(**self)
    }
}

/// 性能统计收集器
#[derive(Clone)]
pub struct StatsCollector {
    global: Arc<RwLock<GlobalStats>>,
    peers: Arc<RwLock<HashMap<IpAddr, PeerStats>>>,
}

/// 全局统计信息
#[derive(Debug, Clone, Default)]
pub struct GlobalStats {
    /// 启动时间
    pub start_time: Option<Instant>,

    /// TUN设备统计
    pub tun_rx_packets: u64,
    pub tun_rx_bytes: u64,
    pub tun_tx_packets: u64,
    pub tun_tx_bytes: u64,

    /// P2P网络统计
    pub p2p_rx_packets: u64,
    pub p2p_rx_bytes: u64,
    pub p2p_tx_packets: u64,
    pub p2p_tx_bytes: u64,

    /// 错误计数
    pub tun_read_errors: u64,
    pub tun_write_errors: u64,
    pub p2p_send_errors: u64,
    pub p2p_recv_errors: u64,

    /// 信令统计
    pub signaling_messages_sent: u64,
    pub signaling_messages_received: u64,
    pub signaling_reconnects: u64,
}

/// 单个对等节点的统计信息
#[derive(Debug, Clone)]
pub struct PeerStats {
    /// 最后活跃时间
    pub last_activity: Instant,

    /// 发送统计
    pub tx_packets: u64,
    pub tx_bytes: u64,

    /// 接收统计
    pub rx_packets: u64,
    pub rx_bytes: u64,

    /// 往返时延 (RTT)
    pub rtt_us: Option<u64>,

    /// 丢包率 (0.0 - 1.0)
    pub loss_rate: f32,
}

impl StatsCollector {
    pub fn new() -> Self {
        Self {
            global: Arc::new(RwLock::new(GlobalStats {
                start_time: Some(Instant::now()),
                ..Default::default()
            })),
            peers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 记录TUN接收数据包
    pub async fn record_tun_rx(&self, bytes: usize) {
        let mut stats = self.global.write().await;
        stats.tun_rx_packets += 1;
        stats.tun_rx_bytes += bytes as u64;
    }

    /// 记录TUN发送数据包
    pub async fn record_tun_tx(&self, bytes: usize) {
        let mut stats = self.global.write().await;
        stats.tun_tx_packets += 1;
        stats.tun_tx_bytes += bytes as u64;
    }

    /// 记录P2P接收数据包
    pub async fn record_p2p_rx(&self, peer_ip: impl Into<IpAddr>, bytes: usize) {
        // 更新全局统计
        {
            let mut stats = self.global.write().await;
            stats.p2p_rx_packets += 1;
            stats.p2p_rx_bytes += bytes as u64;
        }

        // 更新对等节点统计
        let mut peers = self.peers.write().await;
        let peer_stats = peers.entry(peer_ip.into()).or_insert_with(|| PeerStats {
            last_activity: Instant::now(),
            tx_packets: 0,
            tx_bytes: 0,
            rx_packets: 0,
            rx_bytes: 0,
            rtt_us: None,
            loss_rate: 0.0,
        });

        peer_stats.rx_packets += 1;
        peer_stats.rx_bytes += bytes as u64;
        peer_stats.last_activity = Instant::now();
    }

    /// 记录P2P发送数据包
    pub async fn record_p2p_tx(&self, peer_ip: impl Into<IpAddr>, bytes: usize) {
        // 更新全局统计
        {
            let mut stats = self.global.write().await;
            stats.p2p_tx_packets += 1;
            stats.p2p_tx_bytes += bytes as u64;
        }

        // 更新对等节点统计
        let mut peers = self.peers.write().await;
        let peer_stats = peers.entry(peer_ip.into()).or_insert_with(|| PeerStats {
            last_activity: Instant::now(),
            tx_packets: 0,
            tx_bytes: 0,
            rx_packets: 0,
            rx_bytes: 0,
            rtt_us: None,
            loss_rate: 0.0,
        });

        peer_stats.tx_packets += 1;
        peer_stats.tx_bytes += bytes as u64;
        peer_stats.last_activity = Instant::now();
    }

    /// 记录TUN读取错误
    pub async fn record_tun_read_error(&self) {
        let mut stats = self.global.write().await;
        stats.tun_read_errors += 1;
    }

    /// 记录TUN写入错误
    pub async fn record_tun_write_error(&self) {
        let mut stats = self.global.write().await;
        stats.tun_write_errors += 1;
    }

    /// 记录P2P发送错误
    pub async fn record_p2p_send_error(&self) {
        let mut stats = self.global.write().await;
        stats.p2p_send_errors += 1;
    }

    /// 记录信令消息发送
    pub async fn record_signaling_sent(&self) {
        let mut stats = self.global.write().await;
        stats.signaling_messages_sent += 1;
    }

    /// 记录信令消息接收
    pub async fn record_signaling_received(&self) {
        let mut stats = self.global.write().await;
        stats.signaling_messages_received += 1;
    }

    /// 记录信令重连
    pub async fn record_signaling_reconnect(&self) {
        let mut stats = self.global.write().await;
        stats.signaling_reconnects += 1;
    }

    /// 获取全局统计信息
    pub async fn get_global_stats(&self) -> GlobalStats {
        self.global.read().await.clone()
    }

    /// 获取单个对等节点统计信息
    pub async fn get_peer_stat(&self, peer_ip: impl ToIpAddr) -> Option<PeerStats> {
        self.peers.read().await.get(&peer_ip.to_ip_addr()).cloned()
    }

    /// 移除对等节点统计信息
    pub async fn remove_peer(&self, peer_ip: impl ToIpAddr) {
        self.peers.write().await.remove(&peer_ip.to_ip_addr());
    }

    /// 获取运行时长
    pub async fn get_uptime(&self) -> Option<Duration> {
        let stats = self.global.read().await;
        stats.start_time.map(|start| start.elapsed())
    }
}

impl Default for StatsCollector {
    fn default() -> Self {
        Self::new()
    }
}
