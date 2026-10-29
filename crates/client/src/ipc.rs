use anyhow::{Context, Result};
use fuckipv6_common::{recent_error_log_lines, PeerInfo};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, RwLock};
use tokio::task::JoinHandle;
use tokio::time::Duration;

use crate::p2p::connection::{P2pEngine, PeerProbeOutcome};
use crate::stats::StatsCollector;

#[derive(Debug, Serialize, Deserialize)]
pub enum IpcRequest {
    Status,
    Peers,
    Logs {
        lines: usize,
    },
    StopNetwork {
        name: Option<String>,
    },
    ProbePeer {
        target: String,
        timeout_ms: u64,
    },
    UpdateRouting {
        mode: String,
        exit_node: Option<String>,
        is_exit_node: bool,
    },
    Disconnect,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
#[allow(clippy::large_enum_variant)]
pub enum IpcResponse {
    Status(StatusSnapshot),
    Peers(Vec<PeerSnapshot>),
    Logs(Vec<String>),
    MultiStatus(Vec<NetworkSessionSnapshot>),
    PeerProbe(PeerProbeOutcome),
    RoutingUpdated(RoutingUpdateResult),
    Success { message: String },
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingUpdateResult {
    pub mode: String,
    pub exit_node: Option<String>,
    pub is_exit_node: bool,
}

/// Commands delivered from the local control interface to the live runtime.
pub enum RuntimeCommand {
    ProbePeer {
        target: String,
        timeout_ms: u64,
        reply: oneshot::Sender<Result<PeerProbeOutcome, String>>,
    },
    UpdateRouting {
        mode: String,
        exit_node: Option<String>,
        is_exit_node: bool,
        reply: oneshot::Sender<Result<RoutingUpdateResult, String>>,
    },
    Disconnect {
        reply: oneshot::Sender<Result<(), String>>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSessionSnapshot {
    pub network_name: String,
    pub tun_name: String,
    pub assigned_ip: Ipv6Addr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_ipv4: Option<Ipv4Addr>,
    pub p2p_listen_addr: SocketAddr,
    pub uptime_secs: u64,
    pub peers: Vec<PeerSnapshot>,
    pub global: GlobalStatsSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusSnapshot {
    pub assigned_ip: Ipv6Addr,
    pub p2p_listen_addr: SocketAddr,
    pub uptime_secs: u64,
    pub global: GlobalStatsSnapshot,
    pub peers: Vec<PeerSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tun_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub networks: Vec<NetworkSessionSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalStatsSnapshot {
    pub tun_rx_packets: u64,
    pub tun_rx_bytes: u64,
    pub tun_tx_packets: u64,
    pub tun_tx_bytes: u64,
    pub p2p_rx_packets: u64,
    pub p2p_rx_bytes: u64,
    pub p2p_tx_packets: u64,
    pub p2p_tx_bytes: u64,
    pub tun_read_errors: u64,
    pub tun_write_errors: u64,
    pub p2p_send_errors: u64,
    pub p2p_recv_errors: u64,
    pub signaling_messages_sent: u64,
    pub signaling_messages_received: u64,
    pub signaling_reconnects: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerSnapshot {
    pub client_id: String,
    pub virtual_ip: Ipv6Addr,
    pub endpoints: Vec<SocketAddr>,
    pub nat_type: String,
    pub p2p_active: bool,
    pub p2p_healthy: bool,
    pub tx_packets: u64,
    pub tx_bytes: u64,
    pub rx_packets: u64,
    pub rx_bytes: u64,
    pub rtt_us: Option<u64>,
    pub loss_rate: f32,
    pub bandwidth_bps: u64,
    pub health_probes_sent: u64,
    pub health_probes_acked: u64,
    pub health_window_probes: usize,
    pub health_window_lost: usize,
    pub probe_success_rate: f32,
    pub idle_secs: Option<u64>,
    pub last_p2p_success_secs: Option<u64>,
    pub remote_rtt_us: Option<u64>,
    pub remote_loss_rate: Option<f32>,
    pub remote_bandwidth_bps: Option<u64>,
    pub remote_quality_age_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_name: Option<String>,
}

#[derive(Clone)]
pub struct NetworkSessionHandle {
    pub network_name: String,
    pub tun_name: String,
    pub assigned_ip: Ipv6Addr,
    pub assigned_ipv4: Option<Ipv4Addr>,
    pub p2p: P2pEngine,
    pub stats: StatsCollector,
}

#[derive(Clone, Default)]
pub struct MultiNetworkRegistry {
    sessions: Arc<RwLock<HashMap<String, NetworkSessionHandle>>>,
    stop_tx: Arc<RwLock<Option<mpsc::UnboundedSender<Option<String>>>>>,
    control_tx: Arc<RwLock<Option<mpsc::UnboundedSender<RuntimeCommand>>>>,
}

impl MultiNetworkRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_with_session(session: NetworkSessionHandle) -> Self {
        let mut map = HashMap::new();
        map.insert(session.network_name.clone(), session);
        Self {
            sessions: Arc::new(RwLock::new(map)),
            stop_tx: Arc::new(RwLock::new(None)),
            control_tx: Arc::new(RwLock::new(None)),
        }
    }

    pub fn set_control_sender(&self, tx: mpsc::UnboundedSender<RuntimeCommand>) {
        let control_tx = self.control_tx.clone();
        tokio::spawn(async move {
            *control_tx.write().await = Some(tx);
        });
    }

    pub async fn send_control(&self, command: RuntimeCommand) -> bool {
        if let Some(tx) = self.control_tx.read().await.as_ref() {
            tx.send(command).is_ok()
        } else {
            false
        }
    }

    pub fn set_stop_sender(&self, tx: mpsc::UnboundedSender<Option<String>>) {
        let stop_tx = self.stop_tx.clone();
        tokio::spawn(async move {
            *stop_tx.write().await = Some(tx);
        });
    }

    pub async fn register(&self, session: NetworkSessionHandle) {
        let mut w = self.sessions.write().await;
        w.insert(session.network_name.clone(), session);
    }

    pub async fn unregister(&self, network_name: &str) {
        let mut w = self.sessions.write().await;
        w.remove(network_name);
    }

    pub async fn get_all(&self) -> Vec<NetworkSessionHandle> {
        let r = self.sessions.read().await;
        let mut list: Vec<_> = r.values().cloned().collect();
        list.sort_by(|a, b| a.network_name.cmp(&b.network_name));
        list
    }

    pub async fn is_empty(&self) -> bool {
        let r = self.sessions.read().await;
        r.is_empty()
    }

    pub async fn send_stop(&self, name: Option<String>) -> bool {
        if let Some(tx) = self.stop_tx.read().await.as_ref() {
            tx.send(name).is_ok()
        } else {
            false
        }
    }
}

pub fn default_addr() -> String {
    std::env::var("FUCKIPV6_IPC_ADDR").unwrap_or_else(|_| "127.0.0.1:6668".to_string())
}

pub fn spawn_server(
    stats: StatsCollector,
    p2p: P2pEngine,
    assigned_ip: Ipv6Addr,
) -> JoinHandle<()> {
    let handle = NetworkSessionHandle {
        network_name: "default".to_string(),
        tun_name: "fuckipv6".to_string(),
        assigned_ip,
        assigned_ipv4: None,
        p2p,
        stats,
    };
    let registry = MultiNetworkRegistry::new_with_session(handle);
    spawn_multi_server_at(default_addr(), registry)
}

pub fn spawn_server_with_control(
    stats: StatsCollector,
    p2p: P2pEngine,
    assigned_ip: Ipv6Addr,
    control_tx: mpsc::UnboundedSender<RuntimeCommand>,
) -> JoinHandle<()> {
    let handle = NetworkSessionHandle {
        network_name: "default".to_string(),
        tun_name: "fuckipv6".to_string(),
        assigned_ip,
        assigned_ipv4: None,
        p2p,
        stats,
    };
    let registry = MultiNetworkRegistry::new_with_session(handle);
    spawn_multi_server_with_control_at(default_addr(), registry, control_tx)
}

pub fn spawn_server_at(
    addr: String,
    stats: StatsCollector,
    p2p: P2pEngine,
    assigned_ip: Ipv6Addr,
) -> JoinHandle<()> {
    let handle = NetworkSessionHandle {
        network_name: "default".to_string(),
        tun_name: "fuckipv6".to_string(),
        assigned_ip,
        assigned_ipv4: None,
        p2p,
        stats,
    };
    let registry = MultiNetworkRegistry::new_with_session(handle);
    spawn_multi_server_at(addr, registry)
}

pub fn spawn_multi_server(registry: MultiNetworkRegistry) -> JoinHandle<()> {
    spawn_multi_server_at(default_addr(), registry)
}

pub fn spawn_multi_server_at(addr: String, registry: MultiNetworkRegistry) -> JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = serve(&addr, registry).await {
            tracing::warn!("本地 IPC 服务已停止: {:?}", e);
        }
    })
}

fn spawn_multi_server_with_control_at(
    addr: String,
    registry: MultiNetworkRegistry,
    control_tx: mpsc::UnboundedSender<RuntimeCommand>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        registry.set_control_sender(control_tx);
        if let Err(e) = serve(&addr, registry).await {
            tracing::warn!("本地 IPC 服务已停止: {:?}", e);
        }
    })
}

pub async fn request(req: IpcRequest) -> Result<IpcResponse> {
    let addr = default_addr();
    request_at(&addr, req).await
}

async fn request_at(addr: &str, req: IpcRequest) -> Result<IpcResponse> {
    let mut stream = TcpStream::connect(&addr)
        .await
        .with_context(|| format!("无法连接到本地控制接口 {}，客户端可能未运行", addr))?;

    let data = serde_json::to_vec(&req).context("无法序列化本地控制请求")?;
    stream
        .write_all(&data)
        .await
        .context("无法发送本地控制请求")?;
    stream
        .shutdown()
        .await
        .context("无法关闭本地控制请求写入端")?;

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .context("无法读取本地控制响应")?;
    let response = serde_json::from_slice(&response).context("无法解析本地控制响应")?;
    Ok(response)
}

async fn serve(addr: &str, registry: MultiNetworkRegistry) -> Result<()> {
    let listener = TcpListener::bind(&addr)
        .await
        .with_context(|| format!("无法监听本地控制接口 {}", addr))?;

    tracing::info!("本地控制接口监听: {}", addr);

    loop {
        let (stream, _) = listener.accept().await.context("接受本地控制连接失败")?;
        let reg = registry.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, reg).await {
                tracing::debug!("本地 IPC 请求处理失败: {:?}", e);
            }
        });
    }
}

async fn handle_client(mut stream: TcpStream, registry: MultiNetworkRegistry) -> Result<()> {
    let mut request = Vec::new();
    stream
        .read_to_end(&mut request)
        .await
        .context("无法读取本地控制请求")?;

    let response = match serde_json::from_slice::<IpcRequest>(&request) {
        Ok(IpcRequest::Status) => IpcResponse::Status(multi_status_snapshot(&registry).await),
        Ok(IpcRequest::Peers) => IpcResponse::Peers(multi_peer_snapshots(&registry).await),
        Ok(IpcRequest::Logs { lines }) => IpcResponse::Logs(recent_error_log_lines(lines)),
        Ok(IpcRequest::ProbePeer { target, timeout_ms }) => {
            probe_runtime_peer(&registry, target, timeout_ms).await
        }
        Ok(IpcRequest::UpdateRouting {
            mode,
            exit_node,
            is_exit_node,
        }) => update_runtime_routing(&registry, mode, exit_node, is_exit_node).await,
        Ok(IpcRequest::Disconnect) => disconnect_runtime(&registry).await,
        Ok(IpcRequest::StopNetwork { name }) => {
            let signaled = registry.send_stop(name.clone()).await;
            if signaled {
                IpcResponse::Success {
                    message: match &name {
                        Some(n) => format!("已下发停止虚拟网络 [{n}] 指令"),
                        None => "已下发停止全部虚拟网络指令".to_string(),
                    },
                }
            } else {
                IpcResponse::Error {
                    message: "未配置停止通道监听器".to_string(),
                }
            }
        }
        Err(e) => IpcResponse::Error {
            message: format!("Invalid IPC request: {}", e),
        },
    };

    let response = serde_json::to_vec(&response).context("无法序列化本地控制响应")?;
    stream
        .write_all(&response)
        .await
        .context("无法发送本地控制响应")?;
    stream
        .shutdown()
        .await
        .context("无法关闭本地控制响应写入端")?;
    Ok(())
}

async fn probe_runtime_peer(
    registry: &MultiNetworkRegistry,
    target: String,
    timeout_ms: u64,
) -> IpcResponse {
    let timeout_ms = timeout_ms.clamp(100, 5_000);
    let (reply, response) = oneshot::channel();
    if !registry
        .send_control(RuntimeCommand::ProbePeer {
            target,
            timeout_ms,
            reply,
        })
        .await
    {
        return IpcResponse::Error {
            message: "运行时不支持节点探测或控制通道已关闭".to_string(),
        };
    }

    match tokio::time::timeout(Duration::from_millis(timeout_ms + 2_000), response).await {
        Ok(Ok(Ok(outcome))) => IpcResponse::PeerProbe(outcome),
        Ok(Ok(Err(message))) => IpcResponse::Error { message },
        Ok(Err(_)) | Err(_) => IpcResponse::Error {
            message: "节点探测请求未收到运行时响应".to_string(),
        },
    }
}

async fn update_runtime_routing(
    registry: &MultiNetworkRegistry,
    mode: String,
    exit_node: Option<String>,
    is_exit_node: bool,
) -> IpcResponse {
    let (reply, response) = oneshot::channel();
    if !registry
        .send_control(RuntimeCommand::UpdateRouting {
            mode,
            exit_node,
            is_exit_node,
            reply,
        })
        .await
    {
        return IpcResponse::Error {
            message: "运行时不支持路由热更新或控制通道已关闭".to_string(),
        };
    }

    match tokio::time::timeout(Duration::from_secs(20), response).await {
        Ok(Ok(Ok(result))) => IpcResponse::RoutingUpdated(result),
        Ok(Ok(Err(message))) => IpcResponse::Error { message },
        Ok(Err(_)) | Err(_) => IpcResponse::Error {
            message: "路由热更新未收到运行时响应".to_string(),
        },
    }
}

async fn disconnect_runtime(registry: &MultiNetworkRegistry) -> IpcResponse {
    let (reply, response) = oneshot::channel();
    if !registry
        .send_control(RuntimeCommand::Disconnect { reply })
        .await
    {
        return IpcResponse::Error {
            message: "运行时不支持远程断开或控制通道已关闭".to_string(),
        };
    }

    match tokio::time::timeout(Duration::from_secs(15), response).await {
        Ok(Ok(Ok(()))) => IpcResponse::Success {
            message: "客户端运行时已安全停止".to_string(),
        },
        Ok(Ok(Err(message))) => IpcResponse::Error { message },
        Ok(Err(_)) | Err(_) => IpcResponse::Error {
            message: "断开请求未收到运行时响应".to_string(),
        },
    }
}

async fn multi_status_snapshot(registry: &MultiNetworkRegistry) -> StatusSnapshot {
    let sessions = registry.get_all().await;
    if sessions.is_empty() {
        return StatusSnapshot {
            assigned_ip: Ipv6Addr::UNSPECIFIED,
            p2p_listen_addr: SocketAddr::from(([0, 0, 0, 0], 0)),
            uptime_secs: 0,
            global: GlobalStatsSnapshot {
                tun_rx_packets: 0,
                tun_rx_bytes: 0,
                tun_tx_packets: 0,
                tun_tx_bytes: 0,
                p2p_rx_packets: 0,
                p2p_rx_bytes: 0,
                p2p_tx_packets: 0,
                p2p_tx_bytes: 0,
                tun_read_errors: 0,
                tun_write_errors: 0,
                p2p_send_errors: 0,
                p2p_recv_errors: 0,
                signaling_messages_sent: 0,
                signaling_messages_received: 0,
                signaling_reconnects: 0,
            },
            peers: Vec::new(),
            network_name: None,
            tun_name: None,
            networks: Vec::new(),
        };
    }

    let mut net_snapshots = Vec::new();
    let mut all_peers = Vec::new();
    let mut total_global = GlobalStatsSnapshot {
        tun_rx_packets: 0,
        tun_rx_bytes: 0,
        tun_tx_packets: 0,
        tun_tx_bytes: 0,
        p2p_rx_packets: 0,
        p2p_rx_bytes: 0,
        p2p_tx_packets: 0,
        p2p_tx_bytes: 0,
        tun_read_errors: 0,
        tun_write_errors: 0,
        p2p_send_errors: 0,
        p2p_recv_errors: 0,
        signaling_messages_sent: 0,
        signaling_messages_received: 0,
        signaling_reconnects: 0,
    };

    let primary_assigned_ip = sessions[0].assigned_ip;
    let primary_p2p_addr = sessions[0].p2p.local_addr();
    let mut max_uptime = 0;

    for s in &sessions {
        let snap = status_snapshot(&s.stats, &s.p2p, s.assigned_ip).await;
        let mut peers = peer_snapshots(&s.stats, &s.p2p).await;
        for p in &mut peers {
            p.network_name = Some(s.network_name.clone());
        }

        total_global.tun_rx_packets += snap.global.tun_rx_packets;
        total_global.tun_rx_bytes += snap.global.tun_rx_bytes;
        total_global.tun_tx_packets += snap.global.tun_tx_packets;
        total_global.tun_tx_bytes += snap.global.tun_tx_bytes;
        total_global.p2p_rx_packets += snap.global.p2p_rx_packets;
        total_global.p2p_rx_bytes += snap.global.p2p_rx_bytes;
        total_global.p2p_tx_packets += snap.global.p2p_tx_packets;
        total_global.p2p_tx_bytes += snap.global.p2p_tx_bytes;
        total_global.tun_read_errors += snap.global.tun_read_errors;
        total_global.tun_write_errors += snap.global.tun_write_errors;
        total_global.p2p_send_errors += snap.global.p2p_send_errors;
        total_global.p2p_recv_errors += snap.global.p2p_recv_errors;
        total_global.signaling_messages_sent += snap.global.signaling_messages_sent;
        total_global.signaling_messages_received += snap.global.signaling_messages_received;
        total_global.signaling_reconnects += snap.global.signaling_reconnects;

        if snap.uptime_secs > max_uptime {
            max_uptime = snap.uptime_secs;
        }

        net_snapshots.push(NetworkSessionSnapshot {
            network_name: s.network_name.clone(),
            tun_name: s.tun_name.clone(),
            assigned_ip: s.assigned_ip,
            assigned_ipv4: s.assigned_ipv4,
            p2p_listen_addr: s.p2p.local_addr(),
            uptime_secs: snap.uptime_secs,
            peers: peers.clone(),
            global: snap.global,
        });

        all_peers.extend(peers);
    }

    let single_net = if sessions.len() == 1 {
        Some(sessions[0].network_name.clone())
    } else {
        None
    };
    let single_tun = if sessions.len() == 1 {
        Some(sessions[0].tun_name.clone())
    } else {
        None
    };

    StatusSnapshot {
        assigned_ip: primary_assigned_ip,
        p2p_listen_addr: primary_p2p_addr,
        uptime_secs: max_uptime,
        global: total_global,
        peers: all_peers,
        network_name: single_net,
        tun_name: single_tun,
        networks: net_snapshots,
    }
}

async fn multi_peer_snapshots(registry: &MultiNetworkRegistry) -> Vec<PeerSnapshot> {
    let sessions = registry.get_all().await;
    let mut all_peers = Vec::new();
    for s in sessions {
        let mut peers = peer_snapshots(&s.stats, &s.p2p).await;
        for p in &mut peers {
            p.network_name = Some(s.network_name.clone());
        }
        all_peers.extend(peers);
    }
    all_peers
}

async fn status_snapshot(
    stats: &StatsCollector,
    p2p: &P2pEngine,
    assigned_ip: Ipv6Addr,
) -> StatusSnapshot {
    let global = stats.get_global_stats().await;
    let uptime_secs = stats.get_uptime().await.map(|d| d.as_secs()).unwrap_or(0);

    StatusSnapshot {
        assigned_ip,
        p2p_listen_addr: p2p.local_addr(),
        uptime_secs,
        global: GlobalStatsSnapshot {
            tun_rx_packets: global.tun_rx_packets,
            tun_rx_bytes: global.tun_rx_bytes,
            tun_tx_packets: global.tun_tx_packets,
            tun_tx_bytes: global.tun_tx_bytes,
            p2p_rx_packets: global.p2p_rx_packets,
            p2p_rx_bytes: global.p2p_rx_bytes,
            p2p_tx_packets: global.p2p_tx_packets,
            p2p_tx_bytes: global.p2p_tx_bytes,
            tun_read_errors: global.tun_read_errors,
            tun_write_errors: global.tun_write_errors,
            p2p_send_errors: global.p2p_send_errors,
            p2p_recv_errors: global.p2p_recv_errors,
            signaling_messages_sent: global.signaling_messages_sent,
            signaling_messages_received: global.signaling_messages_received,
            signaling_reconnects: global.signaling_reconnects,
        },
        peers: peer_snapshots(stats, p2p).await,
        network_name: None,
        tun_name: None,
        networks: Vec::new(),
    }
}

async fn peer_snapshots(stats: &StatsCollector, p2p: &P2pEngine) -> Vec<PeerSnapshot> {
    let peers = p2p.get_peers().await;
    let mut snapshots = Vec::with_capacity(peers.len());

    for peer in peers {
        snapshots.push(peer_snapshot(stats, p2p, peer).await);
    }

    snapshots.sort_by_key(|peer| peer.virtual_ip);
    snapshots
}

async fn peer_snapshot(stats: &StatsCollector, p2p: &P2pEngine, peer: PeerInfo) -> PeerSnapshot {
    let stat = stats.get_peer_stat(peer.virtual_ip).await;
    let p2p_active = p2p.has_active_peer(&peer.virtual_ip).await;
    let health = p2p.peer_health(&peer.virtual_ip).await;

    PeerSnapshot {
        client_id: peer.client_id,
        virtual_ip: peer.virtual_ip,
        endpoints: peer.endpoints,
        nat_type: peer.nat_type.description().to_string(),
        p2p_active,
        p2p_healthy: health
            .as_ref()
            .map(|health| health.healthy)
            .unwrap_or(false),
        tx_packets: stat.as_ref().map(|s| s.tx_packets).unwrap_or(0),
        tx_bytes: stat.as_ref().map(|s| s.tx_bytes).unwrap_or(0),
        rx_packets: stat.as_ref().map(|s| s.rx_packets).unwrap_or(0),
        rx_bytes: stat.as_ref().map(|s| s.rx_bytes).unwrap_or(0),
        rtt_us: health
            .as_ref()
            .and_then(|health| health.rtt_us)
            .or_else(|| stat.as_ref().and_then(|s| s.rtt_us)),
        loss_rate: health
            .as_ref()
            .map(|health| health.loss_rate)
            .or_else(|| stat.as_ref().map(|s| s.loss_rate))
            .unwrap_or(0.0),
        bandwidth_bps: health
            .as_ref()
            .map(|health| health.bandwidth_bps)
            .unwrap_or(0),
        health_probes_sent: health
            .as_ref()
            .map(|health| health.probes_sent)
            .unwrap_or(0),
        health_probes_acked: health
            .as_ref()
            .map(|health| health.probes_acked)
            .unwrap_or(0),
        health_window_probes: health
            .as_ref()
            .map(|health| health.window_probes)
            .unwrap_or(0),
        health_window_lost: health
            .as_ref()
            .map(|health| health.window_lost)
            .unwrap_or(0),
        probe_success_rate: health
            .as_ref()
            .map(|health| health.probe_success_rate)
            .unwrap_or(0.0),
        idle_secs: health
            .as_ref()
            .and_then(|health| health.last_activity_secs)
            .or_else(|| stat.as_ref().map(|s| s.last_activity.elapsed().as_secs())),
        last_p2p_success_secs: health.as_ref().and_then(|health| health.last_success_secs),
        remote_rtt_us: health
            .as_ref()
            .and_then(|health| health.remote_quality.map(|quality| quality.rtt_us)),
        remote_loss_rate: health
            .as_ref()
            .and_then(|health| health.remote_quality.map(|quality| quality.loss_rate)),
        remote_bandwidth_bps: health
            .as_ref()
            .and_then(|health| health.remote_quality.map(|quality| quality.bandwidth_bps)),
        remote_quality_age_secs: health
            .as_ref()
            .and_then(|health| health.remote_quality.map(|quality| quality.age_secs)),
        network_name: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuckipv6_common::{NatType, PeerInfo};
    use std::net::IpAddr;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_ipc_status_and_peers() -> anyhow::Result<()> {
        let stats = StatsCollector::new();
        let p2p = P2pEngine::new(0).await?;
        let assigned_ip = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0x10, 1);
        p2p.set_local_ip(assigned_ip).await;

        let peer_ip = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0x10, 2);
        let endpoint = SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 50000);
        p2p.add_peer(PeerInfo::new(
            "peer".to_string(),
            peer_ip,
            vec![1; 32],
            vec![endpoint],
            NatType::Open,
        ))
        .await?;
        stats.record_p2p_tx(peer_ip, 128).await;

        let addr = "127.0.0.1:0";
        let listener = TcpListener::bind(addr).await?;
        let local_addr = listener.local_addr()?.to_string();
        drop(listener);

        let task = spawn_server_at(local_addr.clone(), stats, p2p, assigned_ip);
        sleep(Duration::from_millis(50)).await;

        let status = request_at(&local_addr, IpcRequest::Status).await?;
        match status {
            IpcResponse::Status(snapshot) => {
                assert_eq!(snapshot.assigned_ip, assigned_ip);
                assert_eq!(snapshot.peers.len(), 1);
                assert_eq!(snapshot.global.p2p_tx_packets, 1);
            }
            other => panic!("unexpected response: {:?}", other),
        }

        let peers = request_at(&local_addr, IpcRequest::Peers).await?;
        match peers {
            IpcResponse::Peers(peers) => {
                assert_eq!(peers.len(), 1);
                assert_eq!(peers[0].virtual_ip, peer_ip);
                assert_eq!(peers[0].tx_packets, 1);
                assert!(peers[0].health_probes_sent > 0);
            }
            other => panic!("unexpected response: {:?}", other),
        }

        let logs = request_at(&local_addr, IpcRequest::Logs { lines: 10 }).await?;
        match logs {
            IpcResponse::Logs(lines) => {
                assert!(lines.len() <= 10);
            }
            other => panic!("unexpected response: {:?}", other),
        }

        task.abort();
        Ok(())
    }
}
