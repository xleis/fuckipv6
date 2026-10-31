use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
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

/// Counter-only state used by the data plane. Updates are independent atomics,
/// so packet accounting never takes the global statistics write lock.
struct GlobalCounters {
    start_time: Option<Instant>,
    tun_rx_packets: AtomicU64,
    tun_rx_bytes: AtomicU64,
    tun_tx_packets: AtomicU64,
    tun_tx_bytes: AtomicU64,
    p2p_rx_packets: AtomicU64,
    p2p_rx_bytes: AtomicU64,
    p2p_tx_packets: AtomicU64,
    p2p_tx_bytes: AtomicU64,
    tun_read_errors: AtomicU64,
    tun_write_errors: AtomicU64,
    p2p_send_errors: AtomicU64,
    p2p_recv_errors: AtomicU64,
    signaling_messages_sent: AtomicU64,
    signaling_messages_received: AtomicU64,
    signaling_reconnects: AtomicU64,
}

impl GlobalCounters {
    fn new() -> Self {
        Self {
            start_time: Some(Instant::now()),
            tun_rx_packets: AtomicU64::new(0),
            tun_rx_bytes: AtomicU64::new(0),
            tun_tx_packets: AtomicU64::new(0),
            tun_tx_bytes: AtomicU64::new(0),
            p2p_rx_packets: AtomicU64::new(0),
            p2p_rx_bytes: AtomicU64::new(0),
            p2p_tx_packets: AtomicU64::new(0),
            p2p_tx_bytes: AtomicU64::new(0),
            tun_read_errors: AtomicU64::new(0),
            tun_write_errors: AtomicU64::new(0),
            p2p_send_errors: AtomicU64::new(0),
            p2p_recv_errors: AtomicU64::new(0),
            signaling_messages_sent: AtomicU64::new(0),
            signaling_messages_received: AtomicU64::new(0),
            signaling_reconnects: AtomicU64::new(0),
        }
    }

    fn snapshot(&self) -> GlobalStats {
        GlobalStats {
            start_time: self.start_time,
            tun_rx_packets: self.tun_rx_packets.load(Ordering::Relaxed),
            tun_rx_bytes: self.tun_rx_bytes.load(Ordering::Relaxed),
            tun_tx_packets: self.tun_tx_packets.load(Ordering::Relaxed),
            tun_tx_bytes: self.tun_tx_bytes.load(Ordering::Relaxed),
            p2p_rx_packets: self.p2p_rx_packets.load(Ordering::Relaxed),
            p2p_rx_bytes: self.p2p_rx_bytes.load(Ordering::Relaxed),
            p2p_tx_packets: self.p2p_tx_packets.load(Ordering::Relaxed),
            p2p_tx_bytes: self.p2p_tx_bytes.load(Ordering::Relaxed),
            tun_read_errors: self.tun_read_errors.load(Ordering::Relaxed),
            tun_write_errors: self.tun_write_errors.load(Ordering::Relaxed),
            p2p_send_errors: self.p2p_send_errors.load(Ordering::Relaxed),
            p2p_recv_errors: self.p2p_recv_errors.load(Ordering::Relaxed),
            signaling_messages_sent: self.signaling_messages_sent.load(Ordering::Relaxed),
            signaling_messages_received: self.signaling_messages_received.load(Ordering::Relaxed),
            signaling_reconnects: self.signaling_reconnects.load(Ordering::Relaxed),
        }
    }
}

struct PeerCounters {
    last_activity_micros: AtomicU64,
    tx_packets: AtomicU64,
    tx_bytes: AtomicU64,
    rx_packets: AtomicU64,
    rx_bytes: AtomicU64,
    rtt_us: AtomicU64,
    loss_rate: AtomicU32,
}

impl PeerCounters {
    const NO_RTT: u64 = u64::MAX;

    fn new() -> Self {
        Self {
            last_activity_micros: AtomicU64::new(0),
            tx_packets: AtomicU64::new(0),
            tx_bytes: AtomicU64::new(0),
            rx_packets: AtomicU64::new(0),
            rx_bytes: AtomicU64::new(0),
            rtt_us: AtomicU64::new(Self::NO_RTT),
            loss_rate: AtomicU32::new(0.0f32.to_bits()),
        }
    }

    fn snapshot(&self, start_time: Option<Instant>) -> PeerStats {
        let activity_micros = self.last_activity_micros.load(Ordering::Relaxed);
        let last_activity = start_time
            .and_then(|start| start.checked_add(Duration::from_micros(activity_micros)))
            .unwrap_or_else(Instant::now);
        let rtt_us = self.rtt_us.load(Ordering::Relaxed);
        PeerStats {
            last_activity,
            tx_packets: self.tx_packets.load(Ordering::Relaxed),
            tx_bytes: self.tx_bytes.load(Ordering::Relaxed),
            rx_packets: self.rx_packets.load(Ordering::Relaxed),
            rx_bytes: self.rx_bytes.load(Ordering::Relaxed),
            rtt_us: (rtt_us != Self::NO_RTT).then_some(rtt_us),
            loss_rate: f32::from_bits(self.loss_rate.load(Ordering::Relaxed)),
        }
    }
}

/// 性能统计收集器
#[derive(Clone)]
pub struct StatsCollector {
    global: Arc<GlobalCounters>,
    peers: Arc<RwLock<HashMap<IpAddr, Arc<PeerCounters>>>>,
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
            global: Arc::new(GlobalCounters::new()),
            peers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 记录TUN接收数据包
    pub async fn record_tun_rx(&self, bytes: usize) {
        self.global.tun_rx_packets.fetch_add(1, Ordering::Relaxed);
        self.global
            .tun_rx_bytes
            .fetch_add(bytes as u64, Ordering::Relaxed);
    }

    /// 记录TUN发送数据包
    pub async fn record_tun_tx(&self, bytes: usize) {
        self.global.tun_tx_packets.fetch_add(1, Ordering::Relaxed);
        self.global
            .tun_tx_bytes
            .fetch_add(bytes as u64, Ordering::Relaxed);
    }

    /// 记录P2P接收数据包
    pub async fn record_p2p_rx(&self, peer_ip: impl Into<IpAddr>, bytes: usize) {
        self.global.p2p_rx_packets.fetch_add(1, Ordering::Relaxed);
        self.global
            .p2p_rx_bytes
            .fetch_add(bytes as u64, Ordering::Relaxed);

        let peer = self.peer_counters(peer_ip.into()).await;
        peer.rx_packets.fetch_add(1, Ordering::Relaxed);
        peer.rx_bytes.fetch_add(bytes as u64, Ordering::Relaxed);
        self.record_peer_activity(&peer);
    }

    /// 记录P2P发送数据包
    pub async fn record_p2p_tx(&self, peer_ip: impl Into<IpAddr>, bytes: usize) {
        self.global.p2p_tx_packets.fetch_add(1, Ordering::Relaxed);
        self.global
            .p2p_tx_bytes
            .fetch_add(bytes as u64, Ordering::Relaxed);

        let peer = self.peer_counters(peer_ip.into()).await;
        peer.tx_packets.fetch_add(1, Ordering::Relaxed);
        peer.tx_bytes.fetch_add(bytes as u64, Ordering::Relaxed);
        self.record_peer_activity(&peer);
    }

    /// 记录TUN读取错误
    pub async fn record_tun_read_error(&self) {
        self.global.tun_read_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// 记录TUN写入错误
    pub async fn record_tun_write_error(&self) {
        self.global.tun_write_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// 记录P2P发送错误
    pub async fn record_p2p_send_error(&self) {
        self.global.p2p_send_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// 记录信令消息发送
    pub async fn record_signaling_sent(&self) {
        self.global
            .signaling_messages_sent
            .fetch_add(1, Ordering::Relaxed);
    }

    /// 记录信令消息接收
    pub async fn record_signaling_received(&self) {
        self.global
            .signaling_messages_received
            .fetch_add(1, Ordering::Relaxed);
    }

    /// 记录信令重连
    pub async fn record_signaling_reconnect(&self) {
        self.global
            .signaling_reconnects
            .fetch_add(1, Ordering::Relaxed);
    }

    /// 获取全局统计信息
    pub async fn get_global_stats(&self) -> GlobalStats {
        self.global.snapshot()
    }

    /// 获取单个对等节点统计信息
    pub async fn get_peer_stat(&self, peer_ip: impl ToIpAddr) -> Option<PeerStats> {
        let counters = {
            let peers = self.peers.read().await;
            peers.get(&peer_ip.to_ip_addr()).cloned()
        }?;
        Some(counters.snapshot(self.global.start_time))
    }

    /// 移除对等节点统计信息
    pub async fn remove_peer(&self, peer_ip: impl ToIpAddr) {
        self.peers.write().await.remove(&peer_ip.to_ip_addr());
    }

    /// 获取运行时长
    pub async fn get_uptime(&self) -> Option<Duration> {
        self.global.start_time.map(|start| start.elapsed())
    }

    async fn peer_counters(&self, peer_ip: IpAddr) -> Arc<PeerCounters> {
        let existing = {
            let peers = self.peers.read().await;
            peers.get(&peer_ip).cloned()
        };
        if let Some(counters) = existing {
            return counters;
        }

        let mut peers = self.peers.write().await;
        peers
            .entry(peer_ip)
            .or_insert_with(|| Arc::new(PeerCounters::new()))
            .clone()
    }

    fn record_peer_activity(&self, peer: &PeerCounters) {
        let Some(start_time) = self.global.start_time else {
            return;
        };
        let elapsed = start_time.elapsed().as_micros();
        let elapsed = u64::try_from(elapsed).unwrap_or(u64::MAX);
        Self::record_peer_activity_at(peer, elapsed);
    }

    fn record_peer_activity_at(peer: &PeerCounters, elapsed_micros: u64) {
        // Concurrent packet paths can observe timestamps out of order. Keep the
        // newest observation instead of letting an older task overwrite it.
        peer.last_activity_micros
            .fetch_max(elapsed_micros, Ordering::Relaxed);
    }
}

impl Default for StatsCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_activity_does_not_move_backwards_when_updates_arrive_out_of_order() {
        let start_time = Instant::now();
        let peer = PeerCounters::new();

        StatsCollector::record_peer_activity_at(&peer, 250);
        StatsCollector::record_peer_activity_at(&peer, 100);

        assert_eq!(peer.last_activity_micros.load(Ordering::Relaxed), 250);
        assert_eq!(
            peer.snapshot(Some(start_time)).last_activity,
            start_time + Duration::from_micros(250)
        );
    }

    #[tokio::test]
    async fn concurrent_packet_accounting_uses_atomic_snapshots_without_lost_updates() {
        let stats = StatsCollector::new();
        let peer_ip = "fd00::2".parse::<Ipv6Addr>().expect("valid peer IP");
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let stats = stats.clone();
            tasks.push(tokio::spawn(async move {
                for _ in 0..1_000 {
                    stats.record_p2p_rx(peer_ip, 64).await;
                    stats.record_p2p_tx(peer_ip, 1_420).await;
                }
            }));
        }
        for task in tasks {
            task.await.expect("accounting task must not panic");
        }

        let global = stats.get_global_stats().await;
        assert_eq!(global.p2p_rx_packets, 8_000);
        assert_eq!(global.p2p_rx_bytes, 8_000 * 64);
        assert_eq!(global.p2p_tx_packets, 8_000);
        assert_eq!(global.p2p_tx_bytes, 8_000 * 1_420);

        let peer = stats
            .get_peer_stat(peer_ip)
            .await
            .expect("peer counters must exist");
        assert_eq!(peer.rx_packets, 8_000);
        assert_eq!(peer.rx_bytes, 8_000 * 64);
        assert_eq!(peer.tx_packets, 8_000);
        assert_eq!(peer.tx_bytes, 8_000 * 1_420);
        assert!(peer.last_activity >= global.start_time.expect("start time"));
    }

    #[tokio::test]
    async fn removing_peer_drops_only_its_counters() {
        let stats = StatsCollector::new();
        let peer_a = "fd00::2".parse::<Ipv6Addr>().expect("valid peer IP");
        let peer_b = "fd00::3".parse::<Ipv6Addr>().expect("valid peer IP");
        stats.record_p2p_rx(peer_a, 64).await;
        stats.record_p2p_rx(peer_b, 128).await;

        stats.remove_peer(peer_a).await;
        assert!(stats.get_peer_stat(peer_a).await.is_none());
        assert_eq!(
            stats
                .get_peer_stat(peer_b)
                .await
                .expect("peer B counters")
                .rx_bytes,
            128
        );
        assert_eq!(stats.get_global_stats().await.p2p_rx_packets, 2);
    }
}
