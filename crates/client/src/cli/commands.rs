use anyhow::{Context, Result};
use bytes::Bytes;
use fuckipv6_common::recent_error_log_lines;
use fuckipv6_common::{
    decrypt_config_file, encrypt_config_file, is_encrypted_config_path, ClientConfig, Identity,
    NatType, PeerInfo, RoutingAction, SignalingMessage, SmartRouteEngine, VirtualIps,
};
use std::collections::HashMap;
use std::future::Future;
#[cfg(target_os = "linux")]
use std::net::SocketAddr;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, RwLock};
use tokio::task::JoinHandle;
use tokio::time::{sleep, Duration};

use crate::ipc::{self, IpcRequest, IpcResponse};
use crate::logging;
use crate::p2p::connection::{InPlaceSendOutcome, InboundPacket, P2pEngine, PeerRouteInfo};
use crate::p2p::punching;
use crate::p2p::quic;
use crate::p2p::relay::QuicRelayClient;
#[cfg(target_os = "linux")]
use crate::routing::linux_exit::LinuxExitForwarder;
#[cfg(target_os = "linux")]
use crate::routing::linux_route::LinuxRouteManager;
#[cfg(target_os = "linux")]
use crate::routing::table::{parse_linux_ip_routes, RouteEntry};
use crate::routing::ExitNodeManager;
use crate::signaling::client::{SignalingClient, SignalingEvent};
use crate::stats::StatsCollector;
use crate::tun::TunDevice;
use fuckipv6_secure_session::{AEAD_TAG_LEN, DATA_HEADER_LEN};

const TUN_PACKET_BUFFER_SIZE: usize = 65536;
const TUN_BUFFER_ALLOCATION_SIZE: usize = TUN_PACKET_BUFFER_SIZE + DATA_HEADER_LEN + AEAD_TAG_LEN;
const TUN_BUFFER_POOL_SIZE: usize = 64;
const TUN_READ_BATCH_SIZE: usize = 32;
const QUIC_RELAY_SEND_TIMEOUT: Duration = Duration::from_secs(2);
const OUTBOUND_QUEUE_CAPACITY: usize = 256;

/// Owns a fixed-size TUN read buffer while carrying the number of valid bytes.
///
/// The allocation includes fixed Secure Session v2 header and authentication-tag
/// space around the TUN payload for the entire pooled lifetime. Returning a
/// packet to the pool resets only `len`, so small packets never clear or
/// zero-fill 64 KiB in the hot path.
#[derive(Debug)]
pub(crate) struct TunPacket {
    buffer: Vec<u8>,
    len: usize,
}

impl TunPacket {
    fn empty(buffer: Vec<u8>) -> Self {
        debug_assert_eq!(buffer.len(), TUN_BUFFER_ALLOCATION_SIZE);
        Self { buffer, len: 0 }
    }

    fn payload(&self) -> &[u8] {
        &self.buffer[DATA_HEADER_LEN..DATA_HEADER_LEN + self.len]
    }

    fn buffer_mut(&mut self) -> &mut [u8] {
        &mut self.buffer
    }

    fn recycle(mut self) -> Self {
        self.len = 0;
        self
    }
}

/// Keeps a pooled TUN allocation alive while exposing only the sealed bytes.
///
/// `Bytes` may outlive the outbound worker when a fallback transport queued the
/// frame. Dropping the final handle returns the original allocation to the TUN
/// pool instead of freeing and reallocating a 64 KiB buffer.
struct TunPacketBytesOwner {
    packet: Option<TunPacket>,
    encoded_len: usize,
    recycle_tx: mpsc::UnboundedSender<TunPacket>,
}

impl AsRef<[u8]> for TunPacketBytesOwner {
    fn as_ref(&self) -> &[u8] {
        let packet = self
            .packet
            .as_ref()
            .expect("TUN packet owner is present until drop");
        debug_assert!(self.encoded_len <= packet.buffer.len());
        &packet.buffer[..self.encoded_len]
    }
}

impl Drop for TunPacketBytesOwner {
    fn drop(&mut self) {
        if let Some(packet) = self.packet.take() {
            let _ = self.recycle_tx.send(packet.recycle());
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeOptions {
    pub routing_mode: Option<String>,
    pub exit_node: Option<String>,
    pub is_exit_node: Option<bool>,
    pub headless: bool,
    pub gui: bool,
    pub webui_port: u16,
}

pub async fn start(config_path: &str) -> Result<()> {
    start_with_options(config_path, RuntimeOptions::default()).await
}

pub async fn start_with_options(config_path: &str, options: RuntimeOptions) -> Result<()> {
    let config = ClientConfig::from_file(config_path).context("无法加载配置文件")?;
    logging::init_with_level(&config.log.level)?;
    start_with_config_and_options(config_path, config, options).await
}

pub async fn start_with_config(config_path: &str, config: ClientConfig) -> Result<()> {
    start_with_config_and_options(config_path, config, RuntimeOptions::default()).await
}

fn persist_runtime_options(
    config_path: &str,
    config: &mut ClientConfig,
    options: &RuntimeOptions,
) -> Result<bool> {
    let changed = apply_runtime_options_to_config(config, options)?;
    if !changed {
        return Ok(false);
    }

    if is_encrypted_config_path(config_path) {
        tracing::warn!("加密配置文件不支持原地持久化 CLI 路由覆盖，本次仅在运行态生效");
    } else {
        config
            .save_to_file(config_path)
            .context("持久化 CLI 路由与 Exit Node 配置失败")?;
    }
    Ok(true)
}

pub(crate) fn apply_runtime_options_to_config(
    config: &mut ClientConfig,
    options: &RuntimeOptions,
) -> Result<bool> {
    let mut changed = false;
    if let Some(mode) = options.routing_mode.as_deref() {
        let parsed = parse_runtime_routing_mode(mode)?;
        changed |= config.routing.mode != parsed;
        config.routing.mode = parsed;
    }
    if let Some(exit_node) = options.exit_node.as_ref() {
        changed |= config.routing.exit_node.as_ref() != Some(exit_node);
        config.routing.exit_node = Some(exit_node.clone());
    }
    if let Some(enabled) = options.is_exit_node {
        changed |= config.exit_node.enabled != enabled;
        config.exit_node.enabled = enabled;
    }
    Ok(changed)
}

pub async fn start_with_config_and_options(
    config_path: &str,
    mut config: ClientConfig,
    options: RuntimeOptions,
) -> Result<()> {
    tracing::info!("启动客户端: {}", config_path);

    persist_runtime_options(config_path, &mut config, &options)?;

    let ui_mode = crate::ui::detect_ui_mode(options.headless, options.gui);
    let webui_port = options.webui_port;
    tracing::info!("运行时界面环境: {:?}", ui_mode);

    let mut runtime = prepare_client_runtime(config_path, config).await?;

    if webui_port > 0 {
        // 启动内嵌 Axum WebUI 服务
        let ctrl = std::sync::Arc::new(crate::webui::AppController::new());
        {
            let assigned_ip = runtime.assigned_ip.to_string();
            let assigned_ipv4 = runtime.assigned_ips.ipv4.map(|ip| ip.to_string());
            let routing_mode = format!("{:?}", runtime.route_engine.routing_mode());
            let exit_node = runtime.configured_exit_node.clone();
            let is_exit_node = runtime.exit_node_manager.is_exit_node();
            ctrl.update_state(|st| {
                st.status = "online".to_string();
                st.virtual_ipv6 = Some(assigned_ip);
                st.virtual_ipv4 = assigned_ipv4;
                st.routing_mode = routing_mode;
                st.exit_node = exit_node;
                st.is_exit_node = is_exit_node;
            })
            .await;
        }
        if let Err(err) = crate::webui::start_app_webui(webui_port, ctrl).await {
            tracing::warn!("启动客户端 WebUI 失败: {err:#}");
        }

        if ui_mode.is_gui() {
            let _ = crate::ui::run_desktop_gui(webui_port).await;
        }
    }

    run_client_event_loop(&mut runtime).await
}

pub(crate) struct ClientRuntime {
    pub(crate) signaling: Arc<SignalingClient>,
    pub(crate) p2p: P2pEngine,
    pub(crate) identity: Identity,
    pub(crate) outbound: OutboundDispatcher,
    #[cfg(target_os = "linux")]
    pub(crate) route_manager: LinuxRouteManager,
    #[cfg(target_os = "linux")]
    pub(crate) exit_forwarder: Option<LinuxExitForwarder>,
    pub(crate) tun: TunDevice,
    pub(crate) assigned_ip: Ipv6Addr,
    pub(crate) assigned_ips: VirtualIps,
    pub(crate) stats: StatsCollector,
    pub(crate) acl: fuckipv6_common::AclEngine,
    pub(crate) route_engine: SmartRouteEngine,
    pub(crate) exit_node_manager: ExitNodeManager,
    pub(crate) configured_exit_node: Option<String>,
    pub(crate) tun_packet_rx: mpsc::UnboundedReceiver<TunPacket>,
    pub(crate) tun_buffer_tx: mpsc::UnboundedSender<TunPacket>,
    pub(crate) heartbeat_interval: tokio::time::Interval,
    pub(crate) config_path: String,
    pub(crate) config: ClientConfig,
    pub(crate) control_rx: mpsc::UnboundedReceiver<ipc::RuntimeCommand>,
    pub(crate) ipc_task: JoinHandle<()>,
    pub(crate) config_reload_task: JoinHandle<()>,
    pub(crate) tun_reader_task: JoinHandle<()>,
}

impl ClientRuntime {
    pub(crate) fn tun_name(&self) -> &str {
        self.tun.name()
    }

    pub(crate) async fn run_with_shutdown(
        &mut self,
        shutdown_rx: &mut tokio::sync::watch::Receiver<bool>,
    ) -> Result<()> {
        run_client_event_loop_with_shutdown(self, shutdown_rx).await
    }

    pub(crate) async fn shutdown(&mut self) {
        shutdown_runtime(self).await;
    }
}

impl Drop for ClientRuntime {
    fn drop(&mut self) {
        self.signaling.shutdown();
        self.ipc_task.abort();
        self.config_reload_task.abort();
        self.tun_reader_task.abort();
    }
}

pub(crate) async fn prepare_client_runtime_ext(
    config_path: &str,
    mut config: ClientConfig,
    network_name: Option<&str>,
    enable_ipc: bool,
) -> Result<ClientRuntime> {
    config.tun.name = crate::multi_network::derive_tun_name(network_name, &config.tun.name);

    let identity = load_identity(&config)?;
    let relay_identity = identity.clone();

    let (p2p, nat_report) = create_p2p_engine(&config).await?;
    p2p.set_identity(identity.clone()).await;
    let signaling = Arc::new(connect_signaling(&config, identity, &p2p, &nat_report).await?);
    let assigned_ips = wait_for_assigned_ips(&signaling).await?;
    let assigned_ip = assigned_ips.primary_ipv6();
    configure_local_ips(&p2p, &assigned_ips).await;
    let relay = connect_quic_relay(signaling.as_ref(), &relay_identity, assigned_ip).await;
    let tun = create_tun_device(&config, &assigned_ips).await?;
    #[cfg(target_os = "linux")]
    let mut tun = tun;
    let acl = fuckipv6_common::AclEngine::new(&config.acl);

    let mut route_engine = SmartRouteEngine::new(config.routing.mode);
    route_engine.set_virtual_networks(
        config
            .network
            .virtual_network
            .as_deref()
            .and_then(|v| v.parse().ok()),
        "10.8.0.0/24".parse().ok(),
    );
    for cidr in &config.routing.direct_cidrs {
        if let Ok(net) = cidr.parse() {
            route_engine.add_direct_cidr(net);
        }
    }
    for cidr in &config.routing.proxy_cidrs {
        if let Ok(net) = cidr.parse() {
            route_engine.add_proxy_cidr(net);
        }
    }
    let exit_node_manager = ExitNodeManager::new(config.exit_node.clone());
    let configured_exit_node = config.routing.exit_node.clone();

    #[cfg(target_os = "linux")]
    let (route_manager, exit_forwarder) = {
        let mut virtual_cidrs = Vec::new();
        if let Some(network) = config
            .network
            .virtual_network
            .as_deref()
            .and_then(|value| value.parse().ok())
        {
            virtual_cidrs.push(network);
        }
        if let Ok(network) = "10.8.0.0/24".parse() {
            virtual_cidrs.push(network);
        }

        let direct_cidrs: Vec<_> = config
            .routing
            .direct_cidrs
            .iter()
            .filter_map(|cidr| cidr.parse().ok())
            .collect();
        let mut control_endpoints = Vec::new();
        if config.routing.mode != fuckipv6_common::RoutingMode::MeshOnly {
            control_endpoints.extend(resolve_websocket_endpoints(&config.coordinator_url).await?);
        }
        control_endpoints.extend(
            signaling
                .relay_endpoints()
                .await
                .into_iter()
                .map(|endpoint| endpoint.addr),
        );
        for peer in signaling.known_peers().await {
            control_endpoints.extend(peer.endpoints);
        }

        let mut manager = match LinuxRouteManager::install(
            config.routing.mode,
            tun.name(),
            &assigned_ips,
            &virtual_cidrs,
            &direct_cidrs,
            &control_endpoints,
        )
        .await
        {
            Ok(manager) => manager,
            Err(error) => {
                signaling.shutdown();
                if let Some(relay) = relay.as_ref() {
                    relay.close().await;
                }
                let _ = tun.delete().await;
                return Err(error).context("安装 Linux 代理路由失败");
            }
        };

        for peer in signaling.known_peers().await {
            if let Err(error) = manager
                .add_peer_bypasses(&peer.client_id, &peer.endpoints)
                .await
            {
                manager.rollback().await;
                signaling.shutdown();
                if let Some(relay) = relay.as_ref() {
                    relay.close().await;
                }
                let _ = tun.delete().await;
                return Err(error).context("安装节点物理网络旁路失败");
            }
        }
        let exit_forwarder = match LinuxExitForwarder::install(&config.exit_node, tun.name()).await
        {
            Ok(forwarder) => forwarder,
            Err(error) => {
                manager.rollback().await;
                signaling.shutdown();
                if let Some(relay) = relay.as_ref() {
                    relay.close().await;
                }
                let _ = tun.delete().await;
                return Err(error).context("安装 Exit Node 内核转发失败");
            }
        };
        (manager, exit_forwarder)
    };

    // 创建性能统计收集器
    let stats = StatsCollector::new();
    let relay_state = RelayState::default();
    let outbound = OutboundDispatcher::new(
        Arc::new(P2pOutboundTransport::new(
            p2p.clone(),
            signaling.clone(),
            stats.clone(),
            relay_state.clone(),
        )),
        relay_state,
    );
    if let Some(relay) = relay {
        let _ = outbound.install_relay(relay).await;
    }
    let (control_tx, control_rx) = mpsc::unbounded_channel();

    tracing::info!("性能监控已启用");

    let tasks = spawn_runtime_tasks_ext(
        config_path,
        &config,
        &p2p,
        &tun,
        &stats,
        assigned_ip,
        enable_ipc,
        Some(control_tx),
    );

    // 主事件循环
    let mut heartbeat_interval =
        tokio::time::interval(Duration::from_secs(config.p2p.heartbeat_interval));
    heartbeat_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    tracing::info!(
        "客户端启动就绪: 网络 [{}] 接口: {}",
        network_name.unwrap_or("default"),
        config.tun.name
    );

    Ok(ClientRuntime {
        signaling,
        p2p,
        identity: relay_identity,
        outbound,
        #[cfg(target_os = "linux")]
        route_manager,
        #[cfg(target_os = "linux")]
        exit_forwarder,
        tun,
        assigned_ip,
        assigned_ips,
        stats,
        acl,
        route_engine,
        exit_node_manager,
        configured_exit_node,
        tun_packet_rx: tasks.tun_packet_rx,
        tun_buffer_tx: tasks.tun_buffer_tx,
        heartbeat_interval,
        config_path: config_path.to_string(),
        config,
        control_rx,
        ipc_task: tasks.ipc_task,
        config_reload_task: tasks.config_reload_task,
        tun_reader_task: tasks.tun_reader_task,
    })
}

#[cfg(target_os = "linux")]
async fn resolve_websocket_endpoints(url: &str) -> Result<Vec<SocketAddr>> {
    let (host, port) = websocket_host_port(url)?;
    let mut endpoints: Vec<_> = tokio::net::lookup_host((host.as_str(), port))
        .await
        .with_context(|| format!("无法解析协调服务器地址 {host}:{port}"))?
        .collect();
    endpoints.sort_unstable();
    endpoints.dedup();
    if endpoints.is_empty() {
        anyhow::bail!("协调服务器地址未解析到任何 IP: {host}:{port}");
    }
    Ok(endpoints)
}

#[cfg(target_os = "linux")]
fn websocket_host_port(url: &str) -> Result<(String, u16)> {
    let url = url.trim();
    let (scheme, rest) = url
        .split_once("://")
        .with_context(|| format!("协调服务器 URL 缺少协议: {url}"))?;
    let default_port = match scheme.to_ascii_lowercase().as_str() {
        "ws" => 80,
        "wss" => 443,
        other => anyhow::bail!("不支持的协调服务器协议: {other}"),
    };
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|authority| !authority.is_empty())
        .with_context(|| format!("协调服务器 URL 缺少主机: {url}"))?;

    if let Some(authority) = authority.strip_prefix('[') {
        let (host, suffix) = authority
            .split_once(']')
            .with_context(|| format!("IPv6 协调服务器 URL 缺少右方括号: {url}"))?;
        let port = if suffix.is_empty() {
            default_port
        } else {
            suffix
                .strip_prefix(':')
                .and_then(|value| value.parse().ok())
                .with_context(|| format!("协调服务器 URL 端口无效: {url}"))?
        };
        return Ok((host.to_string(), port));
    }

    if let Some((host, port)) = authority.rsplit_once(':') {
        if host.contains(':') {
            anyhow::bail!("IPv6 协调服务器地址必须使用方括号: {url}");
        }
        let port = port
            .parse()
            .with_context(|| format!("协调服务器 URL 端口无效: {url}"))?;
        if host.is_empty() {
            anyhow::bail!("协调服务器 URL 缺少主机: {url}");
        }
        return Ok((host.to_string(), port));
    }

    Ok((authority.to_string(), default_port))
}

async fn prepare_client_runtime(config_path: &str, config: ClientConfig) -> Result<ClientRuntime> {
    prepare_client_runtime_ext(config_path, config, None, true).await
}

struct RuntimeTasks {
    tun_packet_rx: mpsc::UnboundedReceiver<TunPacket>,
    tun_buffer_tx: mpsc::UnboundedSender<TunPacket>,
    ipc_task: JoinHandle<()>,
    config_reload_task: JoinHandle<()>,
    tun_reader_task: JoinHandle<()>,
}

fn load_identity(config: &ClientConfig) -> Result<Identity> {
    let identity =
        Identity::load_or_generate(&config.private_key_path).context("无法加载客户端身份")?;
    tracing::info!("客户端 ID: {}", identity.client_id());
    Ok(identity)
}

async fn create_p2p_engine(
    config: &ClientConfig,
) -> Result<(P2pEngine, punching::NatDetectionReport)> {
    fuckipv6_secure_session::configure_crypto_workers(config.p2p.crypto_workers)
        .map_err(anyhow::Error::msg)
        .context("无法配置数据面加密 worker")?;
    let (p2p, report) = match P2pEngine::new_with_nat_detection_and_limits(
        &config.p2p.nat_detection,
        config.p2p.listen_port,
        config.p2p.inbound_queue_capacity,
        config.p2p.udp_batch_size,
    )
    .await
    {
        Ok(result) => result,
        Err(err) if config.p2p.listen_port != 0 => {
            tracing::warn!(
                "配置的 P2P 监听端口 {} 绑定失败，自动回退到随机可用端口: {err:#}",
                config.p2p.listen_port
            );
            P2pEngine::new_with_nat_detection_and_limits(
                &config.p2p.nat_detection,
                0,
                config.p2p.inbound_queue_capacity,
                config.p2p.udp_batch_size,
            )
            .await
            .context("无法创建 P2P 引擎 (随机端口)")?
        }
        Err(err) => return Err(err).context("无法创建 P2P 引擎"),
    };
    p2p.set_fallback_threshold(config.p2p.fallback_threshold.clone())
        .await;
    tracing::info!("P2P 引擎监听地址: {}", p2p.local_addr());
    Ok((p2p, report))
}

async fn connect_signaling(
    config: &ClientConfig,
    identity: Identity,
    p2p: &P2pEngine,
    report: &punching::NatDetectionReport,
) -> Result<SignalingClient> {
    tracing::info!("NAT 类型检测结果: {}", report.nat_type.description());

    let candidates =
        punching::gather_candidate_endpoints(p2p.local_addr().port(), &report.mapped_addresses)
            .await;
    tracing::info!(
        "本地收集到 {} 个 ICE 候选端点: {:?}",
        candidates.len(),
        candidates
    );
    tracing::info!("正在连接协调服务器: {}", config.coordinator_url);

    let signaling = SignalingClient::connect_with_candidates(
        &config.coordinator_url,
        identity,
        p2p.local_addr().port(),
        report.nat_type,
        candidates,
    )
    .await
    .context("无法连接到协调服务器")?;

    tracing::info!("已连接到协调服务器");
    Ok(signaling)
}

async fn wait_for_assigned_ips(signaling: &SignalingClient) -> Result<VirtualIps> {
    tracing::info!("等待获取虚拟 IP 地址 (双栈/单栈)...");

    for _ in 0..50 {
        if let Some(ips) = signaling.assigned_virtual_ips().await {
            return Ok(ips);
        }
        if let Some(ip) = signaling.assigned_ip().await {
            return Ok(VirtualIps::from_ipv6(ip));
        }
        sleep(Duration::from_millis(100)).await;
    }

    anyhow::bail!("无法获取虚拟 IP 地址");
}

#[allow(dead_code)]
async fn wait_for_assigned_ip(signaling: &SignalingClient) -> Result<Ipv6Addr> {
    let ips = wait_for_assigned_ips(signaling).await?;
    Ok(ips.primary_ipv6())
}

async fn configure_local_ips(p2p: &P2pEngine, assigned_ips: &VirtualIps) {
    tracing::info!("已分配虚拟 IP: {:?}", assigned_ips);
    // PeerInfo::virtual_ip is always the IPv6 overlay address. Bind the same
    // address family locally so both sides derive an identical v2 endpoint
    // context instead of mixing 10.8.0.x with fd00::100x.
    if assigned_ips.ipv6.is_some() {
        p2p.set_local_ip(assigned_ips.primary_ipv6()).await;
    } else if let Some(ip) = assigned_ips.primary_ip() {
        p2p.set_local_ip(ip).await;
    }
}

#[allow(dead_code)]
async fn configure_local_ip(p2p: &P2pEngine, assigned_ip: Ipv6Addr) {
    tracing::info!("已分配虚拟 IP: {}", assigned_ip);
    p2p.set_local_ip(assigned_ip).await;
}

async fn create_tun_device(config: &ClientConfig, assigned_ips: &VirtualIps) -> Result<TunDevice> {
    tracing::info!("正在创建 TUN 设备: {}", config.tun.name);

    let tun = TunDevice::create_dual_stack(&config.tun.name, assigned_ips, 24, 64, config.tun.mtu)
        .await
        .context("无法创建 TUN 设备")?;

    tracing::info!("TUN 设备已创建: {}", config.tun.name);
    Ok(tun)
}

#[allow(clippy::too_many_arguments)]
fn spawn_runtime_tasks_ext(
    config_path: &str,
    config: &ClientConfig,
    p2p: &P2pEngine,
    tun: &TunDevice,
    stats: &StatsCollector,
    assigned_ip: Ipv6Addr,
    enable_ipc: bool,
    control_tx: Option<mpsc::UnboundedSender<ipc::RuntimeCommand>>,
) -> RuntimeTasks {
    let ipc_task = if enable_ipc {
        match control_tx {
            Some(control_tx) => {
                ipc::spawn_server_with_control(stats.clone(), p2p.clone(), assigned_ip, control_tx)
            }
            None => ipc::spawn_server(stats.clone(), p2p.clone(), assigned_ip),
        }
    } else {
        tokio::spawn(async {})
    };
    let config_reload_task = crate::config_reload::spawn_config_watcher(
        config_path.to_string(),
        config.clone(),
        p2p.clone(),
    );
    let (tun_packet_tx, tun_packet_rx) = mpsc::unbounded_channel::<TunPacket>();
    let (tun_buffer_tx, tun_buffer_rx) = mpsc::unbounded_channel::<TunPacket>();

    for _ in 0..TUN_BUFFER_POOL_SIZE {
        let _ = tun_buffer_tx.send(TunPacket::empty(vec![0u8; TUN_BUFFER_ALLOCATION_SIZE]));
    }

    let tun_reader_task = spawn_tun_reader(
        tun.clone(),
        tun_packet_tx,
        tun_buffer_rx,
        tun_buffer_tx.clone(),
        stats.clone(),
    );

    RuntimeTasks {
        tun_packet_rx,
        tun_buffer_tx,
        ipc_task,
        config_reload_task,
        tun_reader_task,
    }
}

#[allow(dead_code)]
fn spawn_runtime_tasks(
    config_path: &str,
    config: &ClientConfig,
    p2p: &P2pEngine,
    tun: &TunDevice,
    stats: &StatsCollector,
    assigned_ip: Ipv6Addr,
) -> RuntimeTasks {
    spawn_runtime_tasks_ext(
        config_path,
        config,
        p2p,
        tun,
        stats,
        assigned_ip,
        true,
        None,
    )
}

async fn connect_quic_relay(
    signaling: &SignalingClient,
    identity: &Identity,
    assigned_ip: Ipv6Addr,
) -> Option<QuicRelayClient> {
    let endpoints = signaling.relay_endpoints().await;
    if !endpoints.is_empty() {
        match tokio::time::timeout(
            Duration::from_secs(5),
            QuicRelayClient::connect_optimal(&endpoints, identity, assigned_ip),
        )
        .await
        {
            Ok(Ok(client)) => return Some(client),
            Ok(Err(err)) => {
                tracing::warn!("最优 QUIC 中继连接失败: {err:#}");
            }
            Err(_) => {
                tracing::warn!("最优 QUIC 中继并发探测超时");
            }
        }
    }

    let Some(endpoint) = signaling.relay_endpoint().await else {
        tracing::debug!("协调端未下发 QUIC 中继 endpoint，使用信令中继 fallback");
        return None;
    };

    match tokio::time::timeout(
        Duration::from_secs(3),
        QuicRelayClient::connect(
            endpoint.addr,
            endpoint.public_key_pin,
            &endpoint.server_name,
            identity,
            assigned_ip,
        ),
    )
    .await
    {
        Ok(Ok(client)) => {
            tracing::info!("已连接 QUIC 中继: {}", endpoint.addr);
            Some(client)
        }
        Ok(Err(err)) => {
            tracing::warn!("QUIC 中继连接失败，将继续使用信令中继 fallback: {err:#}");
            None
        }
        Err(_) => {
            tracing::warn!(
                "QUIC 中继连接超时（可能 UDP 端口被云防火墙拦截），降级使用信令中继 fallback"
            );
            None
        }
    }
}

async fn run_client_event_loop(runtime: &mut ClientRuntime) -> Result<()> {
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        shutdown_signal().await;
        let _ = shutdown_tx.send(true);
    });
    run_client_event_loop_with_shutdown(runtime, &mut shutdown_rx).await
}

pub(crate) async fn run_client_event_loop_with_shutdown(
    runtime: &mut ClientRuntime,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    loop {
        tokio::select! {
            // 从TUN设备读取数据包
            Some(packet) = runtime.tun_packet_rx.recv() => {
                handle_tun_reader_packet(runtime, packet).await;
            }

            // 从P2P接收已认证明文；不会再次经过 replay window。
            Some(packet) = runtime.p2p.recv() => {
                handle_inbound_packet(runtime, packet, "P2P packet").await;
            }

            // 处理信令事件
            Some(event) = runtime.signaling.recv_event() => {
                if let Err(error) = handle_runtime_signaling_event(runtime, event).await {
                    let error = error.context(
                        "信令重连后虚拟网络租约无法维持，客户端已失败关闭",
                    );
                    tracing::error!("{error:#}");
                    shutdown_runtime(runtime).await;
                    return Err(error);
                }
            }

            relay_res = async {
                let Some(relay) = runtime.outbound.relay_handle().await else {
                    return std::future::pending().await;
                };
                (relay.generation, relay.client.recv_packet().await)
            } => {
                match relay_res {
                    (_, Some(packet)) => {
                        handle_inbound_packet(
                            runtime,
                            InboundPacket::SealedCiphertext {
                                src_ip: std::net::IpAddr::V6(packet.src_ip),
                                payload: packet.payload,
                            },
                            "QUIC relay packet",
                        )
                        .await;
                    }
                    (generation, None) => {
                        if let Some(relay) = runtime.outbound.clear_relay(generation).await {
                            tracing::warn!("QUIC 中继接收流已断开，重置中继等待自愈重连");
                            relay.client.close().await;
                        }
                    }
                }
            }

            // 本地控制接口下发的运行时命令
            Some(command) = runtime.control_rx.recv() => {
                if handle_runtime_command(runtime, command).await {
                    break;
                }
            }

            // 定期心跳与 QUIC 中继自愈检测
            _ = runtime.heartbeat_interval.tick() => {
                if runtime.signaling.send_heartbeat().is_ok() {
                    runtime.stats.record_signaling_sent().await;
                }
                if runtime.outbound.relay_handle().await.is_none()
                    && runtime.outbound.begin_relay_reconnect().await
                {
                    spawn_relay_reconnect(runtime);
                }
            }

            // 监听停止信号
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    tracing::info!("收到停止信号，正在优雅退出运行时...");
                    shutdown_runtime(runtime).await;
                    break;
                }
            }
        }
    }

    Ok(())
}

async fn handle_runtime_command(runtime: &mut ClientRuntime, command: ipc::RuntimeCommand) -> bool {
    match command {
        ipc::RuntimeCommand::ProbePeer {
            target,
            timeout_ms,
            reply,
        } => {
            let timeout = Duration::from_millis(timeout_ms.clamp(100, 5_000));
            let result = runtime
                .p2p
                .probe_peer_now(&target, timeout)
                .await
                .ok_or_else(|| format!("未找到在线对等节点: {target}"));
            let _ = reply.send(result);
            false
        }
        ipc::RuntimeCommand::UpdateRouting {
            mode,
            exit_node,
            is_exit_node,
            reply,
        } => {
            let result = apply_runtime_routing(runtime, &mode, exit_node, is_exit_node).await;
            let _ = reply.send(result);
            false
        }
        ipc::RuntimeCommand::Disconnect { reply } => {
            tracing::info!("收到本地控制接口的断开请求");
            shutdown_runtime(runtime).await;
            let _ = reply.send(Ok(()));
            true
        }
    }
}

fn parse_runtime_routing_mode(mode: &str) -> Result<fuckipv6_common::RoutingMode> {
    match mode.to_lowercase().as_str() {
        "meshonly" | "mesh-only" | "mesh_only" => Ok(fuckipv6_common::RoutingMode::MeshOnly),
        "bypassmainland" | "bypass-mainland" | "bypass_mainland" => {
            Ok(fuckipv6_common::RoutingMode::BypassMainland)
        }
        "globalproxy" | "global-proxy" | "global_proxy" => {
            Ok(fuckipv6_common::RoutingMode::GlobalProxy)
        }
        other => {
            anyhow::bail!("未知路由模式: {other} (可用值: MeshOnly, BypassMainland, GlobalProxy)")
        }
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExitForwardingAction {
    Keep,
    Install,
    Remove,
}

#[cfg(target_os = "linux")]
fn exit_forwarding_action(enabled: bool, has_forwarder: bool) -> ExitForwardingAction {
    match (enabled, has_forwarder) {
        (true, false) => ExitForwardingAction::Install,
        (false, true) => ExitForwardingAction::Remove,
        _ => ExitForwardingAction::Keep,
    }
}

async fn apply_runtime_routing(
    runtime: &mut ClientRuntime,
    mode: &str,
    exit_node: Option<String>,
    is_exit_node: bool,
) -> std::result::Result<ipc::RoutingUpdateResult, String> {
    let parsed_mode = parse_runtime_routing_mode(mode).map_err(|error| format!("{error:#}"))?;
    #[cfg(target_os = "linux")]
    let previous_mode = runtime.route_engine.routing_mode();

    let mut updated_config = runtime.config.clone();
    updated_config.routing.mode = parsed_mode;
    updated_config.routing.exit_node = exit_node.clone();
    updated_config.exit_node.enabled = is_exit_node;

    // Stage new kernel resources before changing policy routes. A disabled Exit
    // Node keeps its current resources until persistence succeeds, so a failed
    // config write can leave the old runtime completely untouched.
    #[cfg(target_os = "linux")]
    let exit_action = exit_forwarding_action(is_exit_node, runtime.exit_forwarder.is_some());
    #[cfg(target_os = "linux")]
    let mut staged_exit_forwarder = if exit_action == ExitForwardingAction::Install {
        let forwarder = LinuxExitForwarder::install(&updated_config.exit_node, runtime.tun.name())
            .await
            .map_err(|error| format!("安装 Exit Node 内核转发失败: {error:#}"))?;
        Some(forwarder.ok_or_else(|| "Exit Node 配置已启用，但未创建内核转发器".to_string())?)
    } else {
        None
    };

    #[cfg(target_os = "linux")]
    if let Err(error) = apply_linux_routing_mode(runtime, parsed_mode).await {
        if let Some(mut forwarder) = staged_exit_forwarder.take() {
            forwarder.rollback();
        }
        return Err(format!("Linux 策略路由热更新失败: {error:#}"));
    }

    if let Err(error) = updated_config.save_to_file(&runtime.config_path) {
        #[cfg(target_os = "linux")]
        let rollback_error = apply_linux_routing_mode(runtime, previous_mode).await.err();
        #[cfg(not(target_os = "linux"))]
        let rollback_error: Option<anyhow::Error> = None;

        #[cfg(target_os = "linux")]
        if let Some(mut forwarder) = staged_exit_forwarder.take() {
            forwarder.rollback();
        }

        return match rollback_error {
            Some(rollback_error) => Err(format!(
                "持久化路由配置失败，且恢复原策略路由也失败: {error:#}; 回滚错误: {rollback_error:#}"
            )),
            None => Err(format!("持久化路由配置失败，运行态已回滚: {error:#}")),
        };
    }

    runtime.route_engine.set_routing_mode(parsed_mode);
    runtime.exit_node_manager.set_enabled(is_exit_node);
    runtime.configured_exit_node = exit_node.clone();
    runtime.config = updated_config;

    #[cfg(target_os = "linux")]
    match exit_action {
        ExitForwardingAction::Install => {
            runtime.exit_forwarder = staged_exit_forwarder.take();
        }
        ExitForwardingAction::Remove => {
            if let Some(mut forwarder) = runtime.exit_forwarder.take() {
                forwarder.rollback();
            }
        }
        ExitForwardingAction::Keep => {}
    }

    let canonical_mode = format!("{parsed_mode:?}");
    tracing::info!(
        "路由分流已热更新: mode={}, exit_node={:?}, is_exit_node={}",
        canonical_mode,
        exit_node,
        is_exit_node
    );
    Ok(ipc::RoutingUpdateResult {
        mode: canonical_mode,
        exit_node,
        is_exit_node,
    })
}

#[cfg(target_os = "linux")]
async fn apply_linux_routing_mode(
    runtime: &mut ClientRuntime,
    mode: fuckipv6_common::RoutingMode,
) -> Result<()> {
    use fuckipv6_common::RoutingMode;

    let mut virtual_cidrs = Vec::new();
    if let Some(network) = runtime
        .config
        .network
        .virtual_network
        .as_deref()
        .and_then(|value| value.parse().ok())
    {
        virtual_cidrs.push(network);
    }
    if let Ok(network) = "10.8.0.0/24".parse() {
        virtual_cidrs.push(network);
    }
    let direct_cidrs: Vec<_> = runtime
        .config
        .routing
        .direct_cidrs
        .iter()
        .filter_map(|cidr| cidr.parse().ok())
        .collect();

    let mut control_endpoints = Vec::new();
    if mode != RoutingMode::MeshOnly {
        control_endpoints
            .extend(resolve_websocket_endpoints(&runtime.config.coordinator_url).await?);
    }
    control_endpoints.extend(
        runtime
            .signaling
            .relay_endpoints()
            .await
            .into_iter()
            .map(|endpoint| endpoint.addr),
    );
    for peer in runtime.signaling.known_peers().await {
        control_endpoints.extend(peer.endpoints);
    }

    runtime
        .route_manager
        .reconfigure(
            mode,
            runtime.tun.name(),
            &runtime.assigned_ips,
            &virtual_cidrs,
            &direct_cidrs,
            &control_endpoints,
        )
        .await
}

async fn handle_tun_reader_packet(runtime: &mut ClientRuntime, packet: TunPacket) {
    let payload = packet.payload();
    if runtime.acl.is_enabled()
        && runtime.acl.check_outbound(payload) == fuckipv6_common::AclAction::Deny
    {
        tracing::debug!("ACL 规则丢弃出站 TUN 数据包 ({} 字节)", payload.len());
        recycle_tun_buffer(runtime, packet);
        return;
    }
    runtime.stats.record_tun_rx(payload.len()).await;

    let Some((src_ip, dst_ip)) = route_tun_packet(
        &runtime.p2p,
        &runtime.assigned_ips,
        &runtime.route_engine,
        runtime.configured_exit_node.as_deref(),
        payload,
    )
    .await
    else {
        recycle_tun_buffer(runtime, packet);
        return;
    };
    let outbound = OutboundPacket::pooled(src_ip, dst_ip, packet, runtime.tun_buffer_tx.clone());

    match runtime.outbound.submit(outbound) {
        OutboundSubmitOutcome::Queued => {}
        OutboundSubmitOutcome::QueueFull => {
            runtime.stats.record_p2p_send_error().await;
            tracing::warn!("节点发送队列已满，丢弃 TUN 数据包");
        }
        OutboundSubmitOutcome::WorkerUnavailable => {
            runtime.stats.record_p2p_send_error().await;
            tracing::warn!("节点发送 worker 不可用，丢弃 TUN 数据包");
        }
    }
}

fn recycle_tun_buffer(runtime: &ClientRuntime, packet: TunPacket) {
    let _ = runtime.tun_buffer_tx.send(packet.recycle());
}

async fn record_p2p_rx_for_peer(stats: &StatsCollector, peer: &PeerRouteInfo, bytes: usize) {
    // Per-peer counters are keyed by the canonical overlay identity used by
    // peer_snapshot(). Do not use the packet's inner source address here:
    // dual-stack peers may have an IPv4 primary address, and routed/Exit
    // traffic can carry a source address that belongs to a subnet behind the
    // authenticated peer.
    stats
        .record_p2p_rx(IpAddr::V6(peer.virtual_ip), bytes)
        .await;
}

async fn handle_inbound_packet(runtime: &mut ClientRuntime, packet: InboundPacket, source: &str) {
    let (src_ip, plain_payload, peer) = match packet {
        InboundPacket::AuthenticatedPlaintext {
            src_ip,
            peer,
            payload,
        } => (src_ip, payload, Some(peer)),
        InboundPacket::SealedCiphertext { src_ip, payload } => {
            match runtime.p2p.decrypt_bytes_from_peer(src_ip, payload).await {
                Ok(payload) => {
                    // Relay/signaling traffic is authenticated here rather than
                    // by the receiver, so resolve its route once after opening.
                    let peer = runtime.p2p.peer_route_by_ip(src_ip).await;
                    (src_ip, payload, peer)
                }
                Err(error) => {
                    tracing::debug!("丢弃来自 {src_ip} 的未认证数据包: {error:#}");
                    return;
                }
            }
        }
    };
    if runtime.acl.is_enabled()
        && runtime.acl.check_inbound(&plain_payload) == fuckipv6_common::AclAction::Deny
    {
        tracing::debug!(
            "ACL 规则丢弃来自 {} 的入站数据包 ({} 字节)",
            src_ip,
            plain_payload.len()
        );
        return;
    }
    let Some(peer) = peer else {
        tracing::debug!("丢弃未注册来源 {src_ip} 的入站数据包");
        return;
    };
    let trusted_exit_peer = runtime
        .p2p
        .resolve_exit_peer(runtime.configured_exit_node.as_deref())
        .await;
    let Some(packet_src) = authenticated_inbound_source(
        &plain_payload,
        &peer,
        &runtime.assigned_ips,
        trusted_exit_peer,
    ) else {
        match tun_packet_source(&plain_payload) {
            Some(packet_src) => tracing::warn!(
                "丢弃节点 {} 的伪造内层源地址 {} (外层: {})",
                peer.client_id,
                packet_src,
                src_ip
            ),
            None => tracing::debug!("丢弃来源 {src_ip} 的非 IP 入站载荷"),
        }
        return;
    };
    let Some(target_dst) = tun_packet_destination(&plain_payload) else {
        tracing::debug!("丢弃来源 {src_ip} 的无目的地址入站载荷");
        return;
    };

    // Virtual-network destinations remain ordinary Mesh traffic. Any other
    // destination is privileged exit traffic and must pass every policy check.
    if !runtime.route_engine.is_virtual_network(target_dst) {
        if !runtime.exit_node_manager.is_exit_node() {
            tracing::debug!("本机未启用 Exit Node，丢弃 {} -> {target_dst}", packet_src);
            return;
        }
        if !runtime.exit_node_manager.should_forward_as_exit(target_dst) {
            tracing::debug!("Exit Node 策略拒绝目标地址 {target_dst}");
            return;
        }
        if !runtime
            .exit_node_manager
            .is_client_allowed(peer.client_id.as_ref())
        {
            tracing::warn!("Exit Node 拒绝未授权客户端 {}", peer.client_id);
            return;
        }

        #[cfg(target_os = "linux")]
        {
            let Some(forwarder) = runtime.exit_forwarder.as_mut() else {
                tracing::warn!("Exit Node 已启用但转发器尚未安装，丢弃数据包");
                return;
            };
            if !forwarder.supports(target_dst) {
                tracing::debug!("Exit Node 没有目标地址族 {target_dst} 的出站路径");
                return;
            }
            if let Err(error) = forwarder
                .allow_client(peer.client_id.as_ref(), &peer.virtual_ips)
                .await
            {
                tracing::warn!(
                    "无法为客户端 {} 安装 Exit Node 转发规则: {error:#}",
                    peer.client_id
                );
                return;
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            tracing::warn!("当前平台不支持内核 Exit Node 转发，丢弃数据包");
            return;
        }

        #[cfg(target_os = "linux")]
        tracing::debug!(
            "Exit Node 转发已认证数据包: client={}, {} -> {target_dst}, {} 字节",
            peer.client_id,
            packet_src,
            plain_payload.len()
        );
    }

    record_p2p_rx_for_peer(&runtime.stats, &peer, plain_payload.len()).await;
    write_payload_to_tun(&mut runtime.tun, &runtime.stats, &plain_payload, source).await;
}

async fn handle_runtime_signaling_event(
    runtime: &mut ClientRuntime,
    event: SignalingEvent,
) -> Result<()> {
    runtime.stats.record_signaling_received().await;

    if let SignalingEvent::Registered {
        assigned_ip,
        assigned_ips,
        ..
    } = &event
    {
        validate_registered_assignment(&runtime.assigned_ips, assigned_ips.as_ref(), *assigned_ip)?;
    }

    let action = handle_signaling_event(event, &runtime.p2p, &runtime.stats).await;

    #[cfg(target_os = "linux")]
    match &action {
        SignalingAction::PeersUpsert(peers) => {
            for peer in peers {
                if let Err(error) = runtime
                    .route_manager
                    .add_peer_bypasses(&peer.client_id, &peer.endpoints)
                    .await
                {
                    tracing::warn!("无法更新节点 {} 的物理网络旁路: {error:#}", peer.client_id);
                }
            }
            sync_linux_relay_bypasses(runtime).await;
        }
        SignalingAction::PeerRemoved(client_id) => {
            if let Err(error) = runtime.route_manager.remove_peer_bypasses(client_id).await {
                tracing::warn!("无法删除节点 {client_id} 的物理网络旁路: {error:#}");
            }
            if let Some(forwarder) = runtime.exit_forwarder.as_mut() {
                if let Err(error) = forwarder.remove_client(client_id).await {
                    tracing::warn!("无法删除节点 {client_id} 的 Exit Node 转发规则: {error:#}");
                }
            }
        }
        SignalingAction::Reconnected | SignalingAction::RelayedPacket { .. } => {}
        SignalingAction::None => {}
    }

    match action {
        SignalingAction::RelayedPacket { src_ip, payload } => {
            handle_inbound_packet(
                runtime,
                InboundPacket::SealedCiphertext { src_ip, payload },
                "signaling relay packet",
            )
            .await;
        }
        SignalingAction::Reconnected => {
            runtime.stats.record_signaling_reconnect().await;
        }
        SignalingAction::PeersUpsert(peers) => {
            tracing::debug!("已更新 {} 个节点的运行时状态", peers.len());
        }
        SignalingAction::PeerRemoved(client_id) => {
            tracing::debug!("已移除节点 {client_id} 的运行时状态");
        }
        SignalingAction::None => {}
    }

    Ok(())
}

fn validate_registered_assignment(
    runtime_assigned_ips: &VirtualIps,
    registered_assigned_ips: Option<&VirtualIps>,
    registered_assigned_ip: Ipv6Addr,
) -> Result<()> {
    let registered = registered_assigned_ips
        .cloned()
        .unwrap_or_else(|| VirtualIps::from_ipv6(registered_assigned_ip));
    if registered != *runtime_assigned_ips {
        anyhow::bail!(
            "协调端分配的虚拟地址已变化: 运行时={:?}, 重连确认={:?}; 拒绝在旧 TUN/路由上继续转发",
            runtime_assigned_ips,
            registered
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
async fn sync_linux_relay_bypasses(runtime: &mut ClientRuntime) {
    let endpoints: Vec<SocketAddr> = runtime
        .signaling
        .relay_endpoints()
        .await
        .into_iter()
        .map(|endpoint| endpoint.addr)
        .collect();
    if let Err(error) = runtime
        .route_manager
        .add_peer_bypasses("@relay-endpoints", &endpoints)
        .await
    {
        tracing::warn!("无法更新 QUIC 中继物理网络旁路: {error:#}");
    }
}

async fn write_payload_to_tun(
    tun: &mut TunDevice,
    stats: &StatsCollector,
    payload: &[u8],
    source: &str,
) {
    match tun.write(payload).await {
        Ok(_) => {
            stats.record_tun_tx(payload.len()).await;
        }
        Err(e) => {
            stats.record_tun_write_error().await;
            tracing::error!("TUN 写入数据失败 (来源: {}): {:?}", source, e);
        }
    }
}

async fn shutdown_runtime(runtime: &mut ClientRuntime) {
    #[cfg(target_os = "linux")]
    {
        if let Some(forwarder) = runtime.exit_forwarder.as_mut() {
            forwarder.rollback();
        }
        runtime.exit_forwarder = None;
        runtime.route_manager.rollback().await;
    }

    if let Err(e) = runtime.signaling.close().await {
        tracing::warn!("关闭信令连接失败: {:?}", e);
    }

    runtime.outbound.close_relay().await;

    if let Err(e) = runtime.tun.delete().await {
        tracing::warn!("删除TUN设备失败: {:?}", e);
    }

    runtime.ipc_task.abort();
    runtime.config_reload_task.abort();
    runtime.tun_reader_task.abort();
}

fn spawn_tun_reader(
    mut tun: TunDevice,
    packet_tx: mpsc::UnboundedSender<TunPacket>,
    mut buffer_rx: mpsc::UnboundedReceiver<TunPacket>,
    buffer_tx: mpsc::UnboundedSender<TunPacket>,
    stats: StatsCollector,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let first = match buffer_rx.recv().await {
                Some(packet) => packet,
                None => break,
            };
            let mut packets = vec![first];
            while packets.len() < TUN_READ_BATCH_SIZE {
                match buffer_rx.try_recv() {
                    Ok(packet) => packets.push(packet),
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => break,
                }
            }

            let mut buffer_slices: Vec<&mut [u8]> = packets
                .iter_mut()
                .map(|packet| {
                    &mut packet.buffer[DATA_HEADER_LEN..DATA_HEADER_LEN + TUN_PACKET_BUFFER_SIZE]
                })
                .collect();
            let lengths = tun.read_batch(&mut buffer_slices).await;
            drop(buffer_slices);

            match lengths {
                Ok(lengths) if !lengths.is_empty() && lengths[0] == 0 => {
                    tracing::warn!("TUN read EOF (0 bytes)，退出 reader 任务");
                    for packet in packets {
                        let _ = buffer_tx.send(packet.recycle());
                    }
                    break;
                }
                Ok(lengths) => {
                    for (index, packet) in packets.iter_mut().enumerate() {
                        if let Some(size) = lengths.get(index).copied() {
                            debug_assert!(size <= TUN_PACKET_BUFFER_SIZE);
                            packet.len = size;
                        }
                    }
                    let unread = packets.split_off(lengths.len().min(packets.len()));
                    for packet in packets {
                        if packet_tx.send(packet).is_err() {
                            for packet in unread {
                                let _ = buffer_tx.send(packet.recycle());
                            }
                            return;
                        }
                    }
                    for packet in unread {
                        let _ = buffer_tx.send(packet.recycle());
                    }
                }
                Err(error) => {
                    for packet in packets {
                        let _ = buffer_tx.send(packet.recycle());
                    }
                    stats.record_tun_read_error().await;
                    tracing::error!("TUN read error: {:?}", error);
                    break;
                }
            }
        }
    })
}

pub(crate) async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::warn!("监听 Ctrl+C 失败: {:?}", e);
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let terminate = async {
            match signal(SignalKind::terminate()) {
                Ok(mut stream) => {
                    stream.recv().await;
                }
                Err(e) => {
                    tracing::warn!("监听 SIGTERM 失败: {:?}", e);
                    std::future::pending::<()>().await;
                }
            }
        };

        tokio::select! {
            _ = ctrl_c => {}
            _ = terminate => {}
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await;
    }
}

/// 处理从TUN设备读取的数据包
#[allow(clippy::too_many_arguments)]
async fn route_tun_packet(
    p2p: &P2pEngine,
    assigned_ips: &VirtualIps,
    route_engine: &SmartRouteEngine,
    configured_exit_node: Option<&str>,
    packet: &[u8],
) -> Option<(IpAddr, IpAddr)> {
    let dst_ip = tun_packet_destination(packet)?;

    let is_peer_ip = p2p.contains_peer_ip(dst_ip).await;

    // 智能分流与出口网关决策
    let target_dst = if is_peer_ip {
        dst_ip
    } else {
        match route_engine.evaluate(dst_ip) {
            RoutingAction::Mesh => dst_ip,
            RoutingAction::Direct => {
                tracing::trace!("智能分流判定为 Direct 直连，跳过封装: {}", dst_ip);
                return None;
            }
            RoutingAction::Reject => {
                tracing::debug!("智能分流判定为 Reject 阻断: {}", dst_ip);
                return None;
            }
            RoutingAction::Proxy => {
                if let Some(target_v6) = p2p.resolve_exit_peer(configured_exit_node).await {
                    tracing::debug!(
                        "智能分流判定为 Proxy，经由 Exit Node {} 转发流量至 {}",
                        target_v6,
                        dst_ip
                    );
                    IpAddr::V6(target_v6)
                } else {
                    tracing::warn!(
                        "智能分流判定为 Proxy 但未找到可用 Exit Node，丢弃数据包至 {}",
                        dst_ip
                    );
                    return None;
                }
            }
        }
    };

    if !should_forward_tun_destination(target_dst) {
        tracing::trace!("忽略发往非对等节点的 TUN 数据包: {}", target_dst);
        return None;
    }

    let src_ip = get_packet_src_ip(packet, assigned_ips);

    tracing::debug!(
        "TUN 数据包: {} -> {} (目标: {}), {} 字节",
        src_ip,
        dst_ip,
        target_dst,
        packet.len()
    );

    Some((src_ip, target_dst))
}

fn tun_packet_destination(packet: &[u8]) -> Option<IpAddr> {
    if packet.is_empty() {
        return None;
    }
    match packet[0] >> 4 {
        4 => {
            if packet.len() < 20 {
                return None;
            }
            let dst_bytes: [u8; 4] = packet[16..20].try_into().ok()?;
            Some(IpAddr::V4(Ipv4Addr::from(dst_bytes)))
        }
        6 => {
            if packet.len() < 40 {
                return None;
            }
            let dst_bytes: [u8; 16] = packet[24..40].try_into().ok()?;
            Some(IpAddr::V6(Ipv6Addr::from(dst_bytes)))
        }
        _ => None,
    }
}

fn authenticated_inbound_source(
    packet: &[u8],
    peer: &crate::p2p::connection::PeerRouteInfo,
    assigned_ips: &VirtualIps,
    trusted_exit_peer: Option<Ipv6Addr>,
) -> Option<IpAddr> {
    let source = tun_packet_source(packet)?;
    if peer.matches_ip(source) {
        return Some(source);
    }

    // An authenticated Exit peer is the only peer permitted to return packets
    // with a non-virtual source. Restrict those packets to public unicast
    // sources and destinations assigned to this client, so a compromised or
    // misconfigured ordinary peer cannot inject arbitrary traffic into the TUN.
    if trusted_exit_peer != Some(peer.virtual_ip) {
        return None;
    }
    let destination = tun_packet_destination(packet)?;
    if !assigned_ips.contains(destination) || !is_public_exit_return_source(source) {
        return None;
    }
    Some(source)
}

fn is_public_exit_return_source(source: IpAddr) -> bool {
    match source {
        IpAddr::V4(v4) => {
            !v4.is_unspecified()
                && !v4.is_loopback()
                && !v4.is_private()
                && !v4.is_link_local()
                && !v4.is_broadcast()
                && !v4.is_multicast()
                && !v4.is_documentation()
                && v4.octets()[0] != 0
                && v4.octets()[0] < 240
        }
        IpAddr::V6(v6) => {
            !(v6.is_unspecified()
                || v6.is_loopback()
                || v6.is_multicast()
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] == 0x2001 && v6.segments()[1] == 0x0db8))
        }
    }
}

fn tun_packet_source(packet: &[u8]) -> Option<IpAddr> {
    if packet.is_empty() {
        return None;
    }
    match packet[0] >> 4 {
        4 => {
            if packet.len() < 20 {
                return None;
            }
            let src_bytes: [u8; 4] = packet[12..16].try_into().ok()?;
            Some(IpAddr::V4(Ipv4Addr::from(src_bytes)))
        }
        6 => {
            if packet.len() < 40 {
                return None;
            }
            let src_bytes: [u8; 16] = packet[8..24].try_into().ok()?;
            Some(IpAddr::V6(Ipv6Addr::from(src_bytes)))
        }
        _ => None,
    }
}

fn get_packet_src_ip(packet: &[u8], assigned_ips: &VirtualIps) -> IpAddr {
    if let Some(src) = tun_packet_source(packet) {
        if assigned_ips.contains(src) {
            return src;
        }
    }
    if let Some(dst) = tun_packet_destination(packet) {
        match dst {
            IpAddr::V4(_) => {
                if let Some(v4) = assigned_ips.ipv4 {
                    return IpAddr::V4(v4);
                }
            }
            IpAddr::V6(_) => {
                if let Some(v6) = assigned_ips.ipv6 {
                    return IpAddr::V6(v6);
                }
            }
        }
    }
    assigned_ips
        .primary_ip()
        .unwrap_or_else(|| IpAddr::V6(assigned_ips.primary_ipv6()))
}

#[derive(Debug)]
struct OutboundPacket {
    src_ip: IpAddr,
    dst_ip: IpAddr,
    payload: OutboundPacketPayload,
}

impl OutboundPacket {
    #[cfg(test)]
    fn owned(src_ip: IpAddr, dst_ip: IpAddr, payload: Vec<u8>) -> Self {
        let mut buffer = vec![0u8; DATA_HEADER_LEN + payload.len() + AEAD_TAG_LEN];
        buffer[DATA_HEADER_LEN..DATA_HEADER_LEN + payload.len()].copy_from_slice(&payload);
        Self {
            src_ip,
            dst_ip,
            payload: OutboundPacketPayload::Owned(buffer),
        }
    }

    fn pooled(
        src_ip: IpAddr,
        dst_ip: IpAddr,
        packet: TunPacket,
        recycle_tx: mpsc::UnboundedSender<TunPacket>,
    ) -> Self {
        Self {
            src_ip,
            dst_ip,
            payload: OutboundPacketPayload::Pooled(PooledOutboundPacket {
                packet: Some(packet),
                recycle_tx: Some(recycle_tx),
            }),
        }
    }

    #[cfg(test)]
    fn payload(&self) -> &[u8] {
        match &self.payload {
            #[cfg(test)]
            OutboundPacketPayload::Owned(buffer) => {
                &buffer[DATA_HEADER_LEN..DATA_HEADER_LEN + self.plaintext_len()]
            }
            OutboundPacketPayload::Pooled(packet) => packet
                .packet
                .as_ref()
                .expect("pooled outbound packet is present until drop")
                .payload(),
        }
    }

    fn plaintext_len(&self) -> usize {
        match &self.payload {
            #[cfg(test)]
            OutboundPacketPayload::Owned(buffer) => buffer.len() - DATA_HEADER_LEN - AEAD_TAG_LEN,
            OutboundPacketPayload::Pooled(packet) => {
                packet
                    .packet
                    .as_ref()
                    .expect("pooled outbound packet is present until drop")
                    .len
            }
        }
    }

    fn buffer_mut(&mut self) -> &mut [u8] {
        match &mut self.payload {
            #[cfg(test)]
            OutboundPacketPayload::Owned(buffer) => buffer,
            OutboundPacketPayload::Pooled(packet) => packet
                .packet
                .as_mut()
                .expect("pooled outbound packet is present until drop")
                .buffer_mut(),
        }
    }

    fn into_encrypted_bytes(self, encoded_len: usize) -> Bytes {
        match self.payload {
            #[cfg(test)]
            OutboundPacketPayload::Owned(buffer) => Bytes::copy_from_slice(&buffer[..encoded_len]),
            OutboundPacketPayload::Pooled(mut pooled) => {
                let packet = pooled
                    .packet
                    .take()
                    .expect("pooled outbound packet is present until conversion");
                Bytes::from_owner(TunPacketBytesOwner {
                    packet: Some(packet),
                    encoded_len,
                    recycle_tx: pooled
                        .recycle_tx
                        .take()
                        .expect("pooled outbound recycle sender is present until conversion"),
                })
            }
        }
    }
}

#[derive(Debug)]
enum OutboundPacketPayload {
    #[cfg(test)]
    Owned(Vec<u8>),
    Pooled(PooledOutboundPacket),
}

#[derive(Debug)]
struct PooledOutboundPacket {
    packet: Option<TunPacket>,
    recycle_tx: Option<mpsc::UnboundedSender<TunPacket>>,
}

impl Drop for PooledOutboundPacket {
    fn drop(&mut self) {
        if let (Some(packet), Some(recycle_tx)) = (self.packet.take(), self.recycle_tx.take()) {
            let _ = recycle_tx.send(packet.recycle());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutboundSubmitOutcome {
    Queued,
    QueueFull,
    WorkerUnavailable,
}

trait OutboundTransport: Send + Sync {
    fn send<'a>(&'a self, packet: OutboundPacket) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

    /// The default preserves ordered, one-at-a-time delivery for transports
    /// that cannot batch. The production P2P transport overrides this to issue
    /// one `sendmmsg` for a same-peer prefix.
    fn send_batch<'a>(
        &'a self,
        packets: Vec<OutboundPacket>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            for packet in packets {
                self.send(packet).await;
            }
        })
    }

    fn max_batch_size(&self) -> usize {
        1
    }
}

enum RelayForwardOutcome {
    NoRelay,
    Sent,
    Failed(anyhow::Error),
}

/// Fallback transports consume the ciphertext returned by the direct path.
/// They must never receive plaintext or seal it again.
trait FallbackTransport: Send + Sync {
    fn relay<'a>(
        &'a self,
        src_ip: Ipv6Addr,
        dst_ip: Ipv6Addr,
        encrypted: Bytes,
    ) -> Pin<Box<dyn Future<Output = RelayForwardOutcome> + Send + 'a>>;

    fn signaling(&self, src_ip: IpAddr, dst_ip: IpAddr, encrypted: Bytes) -> Result<()>;
}

struct RuntimeFallbackTransport {
    signaling: Arc<SignalingClient>,
    relay_state: RelayState,
}

impl RuntimeFallbackTransport {
    fn new(signaling: Arc<SignalingClient>, relay_state: RelayState) -> Self {
        Self {
            signaling,
            relay_state,
        }
    }
}

impl FallbackTransport for RuntimeFallbackTransport {
    fn relay<'a>(
        &'a self,
        src_ip: Ipv6Addr,
        dst_ip: Ipv6Addr,
        encrypted: Bytes,
    ) -> Pin<Box<dyn Future<Output = RelayForwardOutcome> + Send + 'a>> {
        Box::pin(async move {
            let Some(relay) = self.relay_state.relay_handle().await else {
                return RelayForwardOutcome::NoRelay;
            };

            let result = tokio::time::timeout(
                QUIC_RELAY_SEND_TIMEOUT,
                relay.client.send_packet(0, src_ip, dst_ip, encrypted),
            )
            .await;

            match result {
                Ok(Ok(())) => RelayForwardOutcome::Sent,
                Ok(Err(error)) => {
                    if let Some(failed) = self.relay_state.clear_relay(relay.generation).await {
                        failed.client.close().await;
                    }
                    RelayForwardOutcome::Failed(error)
                }
                Err(_) => {
                    if let Some(failed) = self.relay_state.clear_relay(relay.generation).await {
                        failed.client.close().await;
                    }
                    RelayForwardOutcome::Failed(anyhow::anyhow!(
                        "QUIC relay send exceeded {:?}",
                        QUIC_RELAY_SEND_TIMEOUT
                    ))
                }
            }
        })
    }

    fn signaling(&self, src_ip: IpAddr, dst_ip: IpAddr, encrypted: Bytes) -> Result<()> {
        fuckipv6_common::validate_signaling_relay_payload_len(encrypted.len())
            .context("signaling fallback ciphertext exceeds the data packet limit")?;
        self.signaling
            .send(SignalingMessage::relay_packet(src_ip, dst_ip, encrypted))?;
        Ok(())
    }
}

#[derive(Clone)]
struct RelayHandle {
    generation: u64,
    client: QuicRelayClient,
}

#[derive(Clone, Default)]
struct RelayState {
    slot: Arc<RwLock<Option<RelayHandle>>>,
    next_generation: Arc<AtomicU64>,
    reconnecting: Arc<AtomicBool>,
}

impl RelayState {
    async fn relay_handle(&self) -> Option<RelayHandle> {
        self.slot.read().await.clone()
    }

    async fn install_relay(&self, client: QuicRelayClient) -> Option<RelayHandle> {
        let generation = self.next_generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.slot
            .write()
            .await
            .replace(RelayHandle { generation, client })
    }

    async fn clear_relay(&self, generation: u64) -> Option<RelayHandle> {
        let mut slot = self.slot.write().await;
        if slot
            .as_ref()
            .is_some_and(|relay| relay.generation == generation)
        {
            slot.take()
        } else {
            None
        }
    }

    async fn take_relay(&self) -> Option<RelayHandle> {
        self.slot.write().await.take()
    }

    async fn begin_reconnect(&self) -> bool {
        if self.slot.read().await.is_some() {
            return false;
        }
        self.reconnecting
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn finish_reconnect(&self) {
        self.reconnecting.store(false, Ordering::Release);
    }
}

/// Owns one bounded, ordered send queue per destination peer.
///
/// Submission is synchronous and does not perform crypto or network I/O, so the
/// central client loop cannot be stalled by a slow relay. A worker processes
/// one peer's packets in order; workers for different peers run independently.
#[derive(Clone)]
pub(crate) struct OutboundDispatcher {
    transport: Arc<dyn OutboundTransport>,
    relay_state: RelayState,
    workers: Arc<Mutex<HashMap<IpAddr, mpsc::Sender<OutboundPacket>>>>,
}

impl OutboundDispatcher {
    fn new(transport: Arc<dyn OutboundTransport>, relay_state: RelayState) -> Self {
        Self {
            transport,
            relay_state,
            workers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[cfg(test)]
    fn for_test(transport: Arc<dyn OutboundTransport>) -> Self {
        Self::new(transport, RelayState::default())
    }

    fn submit(&self, packet: OutboundPacket) -> OutboundSubmitOutcome {
        let dst_ip = packet.dst_ip;
        let mut workers = self
            .workers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let sender = workers
            .get(&dst_ip)
            .cloned()
            .unwrap_or_else(|| self.spawn_worker(&mut workers, dst_ip));

        match sender.try_send(packet) {
            Ok(()) => OutboundSubmitOutcome::Queued,
            Err(mpsc::error::TrySendError::Full(_)) => OutboundSubmitOutcome::QueueFull,
            Err(mpsc::error::TrySendError::Closed(packet)) => {
                workers.remove(&dst_ip);
                let replacement = self.spawn_worker(&mut workers, dst_ip);
                match replacement.try_send(packet) {
                    Ok(()) => OutboundSubmitOutcome::Queued,
                    Err(mpsc::error::TrySendError::Full(_)) => OutboundSubmitOutcome::QueueFull,
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        workers.remove(&dst_ip);
                        OutboundSubmitOutcome::WorkerUnavailable
                    }
                }
            }
        }
    }

    fn spawn_worker(
        &self,
        workers: &mut HashMap<IpAddr, mpsc::Sender<OutboundPacket>>,
        dst_ip: IpAddr,
    ) -> mpsc::Sender<OutboundPacket> {
        let (sender, receiver) = mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
        let transport = self.transport.clone();
        tokio::spawn(run_outbound_worker(dst_ip, receiver, transport));
        workers.insert(dst_ip, sender.clone());
        sender
    }

    async fn relay_handle(&self) -> Option<RelayHandle> {
        self.relay_state.relay_handle().await
    }

    async fn install_relay(&self, relay: QuicRelayClient) -> Option<RelayHandle> {
        self.relay_state.install_relay(relay).await
    }

    async fn clear_relay(&self, generation: u64) -> Option<RelayHandle> {
        self.relay_state.clear_relay(generation).await
    }

    async fn close_relay(&self) {
        if let Some(relay) = self.relay_state.take_relay().await {
            relay.client.close().await;
        }
    }

    async fn begin_relay_reconnect(&self) -> bool {
        self.relay_state.begin_reconnect().await
    }

    fn finish_relay_reconnect(&self) {
        self.relay_state.finish_reconnect();
    }
}

async fn run_outbound_worker(
    _dst_ip: IpAddr,
    mut receiver: mpsc::Receiver<OutboundPacket>,
    transport: Arc<dyn OutboundTransport>,
) {
    let max_batch_size = transport.max_batch_size().max(1);
    while let Some(packet) = receiver.recv().await {
        let mut packets = Vec::with_capacity(max_batch_size);
        packets.push(packet);
        while packets.len() < max_batch_size {
            match receiver.try_recv() {
                Ok(packet) => packets.push(packet),
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => break,
            }
        }
        transport.send_batch(packets).await;
    }
}

struct P2pOutboundTransport {
    p2p: P2pEngine,
    fallback: Arc<dyn FallbackTransport>,
    stats: StatsCollector,
}

impl P2pOutboundTransport {
    fn new(
        p2p: P2pEngine,
        signaling: Arc<SignalingClient>,
        stats: StatsCollector,
        relay_state: RelayState,
    ) -> Self {
        Self {
            p2p,
            fallback: Arc::new(RuntimeFallbackTransport::new(signaling, relay_state)),
            stats,
        }
    }

    #[cfg(test)]
    fn with_fallback(p2p: P2pEngine, fallback: Arc<dyn FallbackTransport>) -> Self {
        Self {
            p2p,
            fallback,
            stats: StatsCollector::new(),
        }
    }

    async fn send_packet(&self, packet: OutboundPacket) {
        self.send_packets(vec![packet]).await;
    }

    async fn send_packets(&self, mut packets: Vec<OutboundPacket>) {
        if packets.is_empty() {
            return;
        }
        let destination = packets[0].dst_ip;
        debug_assert!(packets.iter().all(|packet| packet.dst_ip == destination));
        let plaintext_lens: Vec<usize> =
            packets.iter().map(OutboundPacket::plaintext_len).collect();
        let mut batch = Vec::with_capacity(packets.len());
        for packet in packets.iter_mut() {
            let plaintext_len = packet.plaintext_len();
            batch.push((packet.buffer_mut(), plaintext_len));
        }
        let results = self
            .p2p
            .send_batch_to_or_fallback_in_place(destination, batch)
            .await;

        let results = match results {
            Ok(results) => results,
            Err(error) => {
                tracing::debug!(
                    "Failed to start UDP batch for {} ({} packets): {error:#}",
                    destination,
                    packets.len()
                );
                for _ in packets {
                    self.stats.record_p2p_send_error().await;
                }
                return;
            }
        };
        debug_assert_eq!(results.len(), packets.len());

        for ((packet, plaintext_len), outcome) in
            packets.into_iter().zip(plaintext_lens).zip(results)
        {
            match outcome {
                Ok(InPlaceSendOutcome::Sent) => {
                    self.stats.record_p2p_tx(packet.dst_ip, plaintext_len).await;
                }
                Ok(InPlaceSendOutcome::SealedForFallback { encoded_len, error }) => {
                    self.stats.record_p2p_send_error().await;
                    tracing::debug!(
                        "Failed to send packet to {} over P2P, using sealed fallback packet: {:?}",
                        packet.dst_ip,
                        error
                    );
                    self.send_fallback(packet, encoded_len).await;
                }
                Err(error) => {
                    self.stats.record_p2p_send_error().await;
                    tracing::debug!("Failed to seal packet for {}: {:?}", packet.dst_ip, error);
                }
            }
        }
    }

    async fn send_fallback(&self, packet: OutboundPacket, encoded_len: usize) {
        let plaintext_len = packet.plaintext_len();
        let src_ip = packet.src_ip;
        let dst_ip = packet.dst_ip;
        let encrypted = packet.into_encrypted_bytes(encoded_len);
        if let (IpAddr::V6(src_v6), IpAddr::V6(dst_v6)) = (src_ip, dst_ip) {
            match self.fallback.relay(src_v6, dst_v6, encrypted.clone()).await {
                RelayForwardOutcome::Sent => {
                    self.stats.record_p2p_tx(dst_ip, plaintext_len).await;
                    return;
                }
                RelayForwardOutcome::Failed(error) => {
                    tracing::warn!("QUIC 中继转发 {} 失败，降级到信令中继: {error:#}", dst_ip);
                }
                RelayForwardOutcome::NoRelay => {}
            }
        }

        match self.fallback.signaling(src_ip, dst_ip, encrypted) {
            Ok(()) => {
                self.stats.record_signaling_sent().await;
                self.stats.record_p2p_tx(dst_ip, plaintext_len).await;
            }
            Err(error) => {
                self.stats.record_p2p_send_error().await;
                tracing::warn!("通过信令通道向 {} 转发数据包失败: {:?}", dst_ip, error);
            }
        }
    }
}

impl OutboundTransport for P2pOutboundTransport {
    fn send<'a>(&'a self, packet: OutboundPacket) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(self.send_packet(packet))
    }

    fn send_batch<'a>(
        &'a self,
        packets: Vec<OutboundPacket>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(self.send_packets(packets))
    }

    fn max_batch_size(&self) -> usize {
        self.p2p.udp_batch_size()
    }
}

fn spawn_relay_reconnect(runtime: &ClientRuntime) {
    let signaling = runtime.signaling.clone();
    let identity = runtime.identity.clone();
    let assigned_ip = runtime.assigned_ip;
    let outbound = runtime.outbound.clone();

    tokio::spawn(async move {
        let relay = connect_quic_relay(signaling.as_ref(), &identity, assigned_ip).await;
        if let Some(relay) = relay {
            if let Some(previous) = outbound.install_relay(relay).await {
                previous.client.close().await;
            }
        }
        outbound.finish_relay_reconnect();
    });
}

fn should_forward_tun_destination(dst_ip: IpAddr) -> bool {
    match dst_ip {
        IpAddr::V4(v4) => {
            !v4.is_broadcast() && !v4.is_multicast() && !v4.is_unspecified() && !v4.is_loopback()
        }
        IpAddr::V6(v6) => !v6.is_multicast() && !v6.is_unspecified() && !v6.is_loopback(),
    }
}

/// 处理信令事件
enum SignalingAction {
    None,
    Reconnected,
    PeersUpsert(Vec<PeerInfo>),
    PeerRemoved(String),
    RelayedPacket { src_ip: IpAddr, payload: Bytes },
}

async fn handle_signaling_event(
    event: SignalingEvent,
    p2p: &P2pEngine,
    stats: &StatsCollector,
) -> SignalingAction {
    match event {
        SignalingEvent::Registered {
            assigned_ip, peers, ..
        } => {
            tracing::info!("已注册，虚拟 IP: {}", assigned_ip);
            tracing::info!("发现 {} 个在线节点", peers.len());
            add_registered_peers(p2p, peers.clone()).await;
            return SignalingAction::PeersUpsert(peers);
        }

        SignalingEvent::PeerOnline { peer } => {
            tracing::info!("节点上线: {} ({})", peer.client_id, peer.virtual_ip);
            add_peer_from_signaling(p2p, peer.clone()).await;
            return SignalingAction::PeersUpsert(vec![peer]);
        }

        SignalingEvent::PeerOffline { client_id } => {
            remove_offline_peer(p2p, stats, client_id.clone()).await;
            return SignalingAction::PeerRemoved(client_id);
        }

        SignalingEvent::ConnectInfo { peer } | SignalingEvent::ConnectNotify { peer } => {
            tracing::debug!("收到连接信息: {} ({})", peer.client_id, peer.virtual_ip);
            add_peer_from_signaling(p2p, peer.clone()).await;
            return SignalingAction::PeersUpsert(vec![peer]);
        }

        SignalingEvent::HeartbeatAck { timestamp } => {
            tracing::debug!("收到心跳响应: {}", timestamp);
        }

        SignalingEvent::Disconnected { reason } => {
            tracing::warn!("信令连接断开，将自动重连: {}", reason);
        }

        SignalingEvent::Reconnected => {
            tracing::info!("信令连接已恢复");
            return SignalingAction::Reconnected;
        }

        SignalingEvent::RelayPacket {
            src_ip,
            dst_ip,
            payload,
        } => {
            tracing::debug!(
                "收到中继数据包: {} -> {}, {} bytes",
                src_ip,
                dst_ip,
                payload.len()
            );
            return SignalingAction::RelayedPacket { src_ip, payload };
        }

        SignalingEvent::Error { code, message } => {
            tracing::error!("信令错误 {}: {}", code, message);
        }
    }

    SignalingAction::None
}

async fn add_registered_peers(p2p: &P2pEngine, peers: Vec<PeerInfo>) {
    for peer in peers {
        add_peer_from_signaling(p2p, peer).await;
    }
}

async fn add_peer_from_signaling(p2p: &P2pEngine, peer: PeerInfo) {
    if let Err(e) = p2p.add_peer(peer.clone()).await {
        tracing::error!("无法添加节点 {}: {:?}", peer.client_id, e);
    }
}

async fn remove_offline_peer(p2p: &P2pEngine, stats: &StatsCollector, client_id: String) {
    tracing::info!("节点离线: {}", client_id);
    if let Some(peer_ip) = p2p.remove_peer_by_id(&client_id).await {
        stats.remove_peer(peer_ip).await;
    }
}

pub async fn status(json: bool) -> Result<()> {
    let response = ipc::request(IpcRequest::Status).await?;

    let IpcResponse::Status(snapshot) = response else {
        match response {
            IpcResponse::Error { message } => anyhow::bail!(message),
            _ => anyhow::bail!("Unexpected IPC response"),
        }
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&snapshot)?);
        return Ok(());
    }

    if snapshot.networks.len() > 1 {
        println!(
            "多网络并发运行状态 (共 {} 个网络活跃)",
            snapshot.networks.len()
        );
        println!("══════════════════════════════════════════════════════════════════════════════");
        for net in &snapshot.networks {
            println!("🌐 虚拟网络:        {}", net.network_name);
            println!("   TUN 接口:        {}", net.tun_name);
            println!("   虚拟 IPv6:       {}", net.assigned_ip);
            if let Some(v4) = net.assigned_ipv4 {
                println!("   虚拟 IPv4:       {}", v4);
            }
            println!("   P2P 监听:        {}", net.p2p_listen_addr);
            println!("   运行时间:        {}", format_duration(net.uptime_secs));
            println!("   在线对等节点:    {}", net.peers.len());
            let healthy = net.peers.iter().filter(|p| p.p2p_healthy).count();
            println!("   P2P 直连健康:    {}/{}", healthy, net.peers.len());
            println!(
                "   TUN 流量统计:    {} 收 / {} 发 数据包 ({} 字节)",
                net.global.tun_rx_packets,
                net.global.tun_tx_packets,
                net.global.tun_rx_bytes + net.global.tun_tx_bytes
            );
            println!(
                "   P2P 流量统计:    {} 收 / {} 发 数据包 ({} 字节)",
                net.global.p2p_rx_packets,
                net.global.p2p_tx_packets,
                net.global.p2p_rx_bytes + net.global.p2p_tx_bytes
            );
            print_dataplane_status(&net.dataplane, "   ");
            println!(
                "──────────────────────────────────────────────────────────────────────────────"
            );
        }
        return Ok(());
    }

    println!("客户端运行状态 (Client Status)");
    println!("─────────────────────────────────────");
    println!();
    if let Some(ref net_name) = snapshot.network_name {
        println!("虚拟网络:        {}", net_name);
    }
    if let Some(ref tun) = snapshot.tun_name {
        println!("网卡接口:        {}", tun);
    }
    println!("虚拟 IP:         {}", snapshot.assigned_ip);
    println!("P2P 监听地址:    {}", snapshot.p2p_listen_addr);
    println!("在线运行时长:    {}", format_duration(snapshot.uptime_secs));
    println!("对等节点数量:    {}", snapshot.peers.len());
    let healthy_peers = snapshot
        .peers
        .iter()
        .filter(|peer| peer.p2p_healthy)
        .count();
    println!(
        "P2P 直连健康度:  {}/{}",
        healthy_peers,
        snapshot.peers.len()
    );
    println!();
    println!("流量统计:");
    println!(
        "  TUN 收/发包数:   {} / {} 包",
        snapshot.global.tun_rx_packets, snapshot.global.tun_tx_packets
    );
    println!(
        "  P2P 收/发包数:   {} / {} 包",
        snapshot.global.p2p_rx_packets, snapshot.global.p2p_tx_packets
    );
    println!(
        "  P2P 流量:        {} 接收 / {} 发送",
        format_bandwidth(snapshot.global.p2p_rx_bytes),
        format_bandwidth(snapshot.global.p2p_tx_bytes)
    );
    println!();
    println!("异常统计:");
    println!(
        "  TUN 读/写错误:   {} / {}",
        snapshot.global.tun_read_errors, snapshot.global.tun_write_errors
    );
    println!(
        "  P2P 发/收错误:   {} / {}",
        snapshot.global.p2p_send_errors, snapshot.global.p2p_recv_errors
    );
    if let Some(ref dataplane) = snapshot.dataplane {
        println!();
        print_dataplane_status(dataplane, "  ");
    }
    Ok(())
}

fn print_dataplane_status(dataplane: &ipc::DataplaneMetricsSnapshot, prefix: &str) {
    let udp = dataplane.udp;
    let recv_pps = if udp.recv_syscalls == 0 {
        0.0
    } else {
        udp.recv_datagrams as f64 / udp.recv_syscalls as f64
    };
    let send_pps = if udp.send_syscalls == 0 {
        0.0
    } else {
        udp.send_datagrams as f64 / udp.send_syscalls as f64
    };
    let pool = dataplane.buffer_pool;
    let acquisitions = pool.hits + pool.allocations;
    let pool_hit_rate = if acquisitions == 0 {
        0.0
    } else {
        pool.hits as f64 / acquisitions as f64 * 100.0
    };
    let crypto = dataplane.crypto;
    let queue = dataplane.inbound_queue;
    println!("{prefix}数据面统计:");
    println!(
        "{prefix}  UDP 收:        {} 包 / {} syscall ({recv_pps:.2} 包/次), 最大批 {}",
        udp.recv_datagrams, udp.recv_syscalls, udp.max_recv_batch
    );
    println!(
        "{prefix}  UDP 发:        {} 包 / {} syscall ({send_pps:.2} 包/次), 最大批 {}, fallback {}",
        udp.send_datagrams, udp.send_syscalls, udp.max_send_batch, udp.fallback_packets
    );
    println!(
        "{prefix}  RX 缓冲池:     {pool_hit_rate:.1}% 命中, 分配 {}, 回收 {}, 满池丢弃 {}",
        pool.allocations, pool.returns, pool.full_returns
    );
    println!(
        "{prefix}  入站队列:      {}/{} 当前/上限, 峰值 {}, 满丢弃 {}, 等待 {}",
        queue.depth, queue.capacity, queue.max_depth, queue.dropped_full, queue.consumer_waits
    );
    println!(
        "{prefix}  Crypto worker: {} 线程, 并行 {} 批/{} 包, 串行 {}, 过载回退 {}",
        crypto.worker_threads,
        crypto.parallel_jobs,
        crypto.parallel_packets,
        crypto.serial_jobs,
        crypto.overloaded_jobs
    );
    let lock = dataplane.secure_lock;
    println!(
        "{prefix}  会话锁:       {} 个 peer, 获取 {}, 竞争 {}, 等待均值/峰值 {}/{} ns",
        lock.peers,
        lock.acquisitions,
        lock.contended_acquisitions,
        lock.wait_ns_avg,
        lock.wait_ns_max
    );
    println!(
        "{prefix}  持锁时间:     均值/峰值 {}/{} ns, 总计 {} ns",
        lock.hold_ns_avg, lock.hold_ns_max, lock.hold_ns_total
    );
    let operations = lock.operations;
    println!(
        "{prefix}  分类获取/竞争: 发 {}/{}, 收 {}/{}, 控 {}/{}, 生命周期 {}/{}, 诊断 {}/{}",
        operations.outbound.acquisitions,
        operations.outbound.contended_acquisitions,
        operations.inbound.acquisitions,
        operations.inbound.contended_acquisitions,
        operations.control.acquisitions,
        operations.control.contended_acquisitions,
        operations.lifecycle.acquisitions,
        operations.lifecycle.contended_acquisitions,
        operations.diagnostics.acquisitions,
        operations.diagnostics.contended_acquisitions
    );
}

pub async fn peers() -> Result<()> {
    let response = ipc::request(IpcRequest::Peers).await?;

    let IpcResponse::Peers(peers) = response else {
        match response {
            IpcResponse::Error { message } => anyhow::bail!(message),
            _ => anyhow::bail!("Unexpected IPC response"),
        }
    };

    println!("已连接对等节点 (Connected Peers)");
    println!("─────────────────────────────────────");
    println!();

    if peers.is_empty() {
        println!("当前无已连接的对等节点。");
        return Ok(());
    }

    let has_networks = peers.iter().any(|p| p.network_name.is_some());
    if has_networks {
        println!(
            "{:<12}  {:<18}  {:<26}  {:<8}  {:<8}  {:<8}  {:<8}  {:<10}  {:<12}  公网物理端点",
            "所属网络",
            "虚拟 IP",
            "客户端标识 (Client ID)",
            "链路模式",
            "往返时延",
            "丢包率",
            "探测成功",
            "当前带宽",
            "收/发包"
        );
        println!("{}", "─".repeat(150));

        for peer in peers {
            let net = peer.network_name.as_deref().unwrap_or("-");
            let client_id = abbreviate(&peer.client_id, 26);
            let path = if peer.p2p_active && peer.p2p_healthy {
                "P2P直连"
            } else {
                "QUIC中继"
            };
            let rtt = peer
                .rtt_us
                .map(format_rtt)
                .unwrap_or_else(|| "-".to_string());
            let loss = format!("{:.1}%", peer.loss_rate * 100.0);
            let probe = if peer.health_window_probes == 0 {
                "-".to_string()
            } else {
                format!("{:.1}%", peer.probe_success_rate * 100.0)
            };
            let bandwidth = format_bandwidth(peer.bandwidth_bps);
            let packets = format!("{}/{}", peer.rx_packets, peer.tx_packets);
            let endpoint = peer
                .endpoints
                .first()
                .map(|addr| addr.to_string())
                .unwrap_or_else(|| "-".to_string());

            println!(
                "{:<12}  {:<18}  {:<26}  {:<8}  {:<8}  {:<8}  {:<8}  {:<10}  {:<12}  {}",
                net,
                peer.virtual_ip,
                client_id,
                path,
                rtt,
                loss,
                probe,
                bandwidth,
                packets,
                endpoint
            );
        }
        return Ok(());
    }

    println!(
        "{:<18}  {:<30}  {:<8}  {:<8}  {:<8}  {:<8}  {:<10}  {:<12}  公网物理端点",
        "虚拟 IP",
        "客户端标识 (Client ID)",
        "链路模式",
        "往返时延",
        "丢包率",
        "探测成功",
        "当前带宽",
        "收/发包"
    );
    println!("{}", "─".repeat(140));

    for peer in peers {
        let client_id = abbreviate(&peer.client_id, 30);
        let path = if peer.p2p_active && peer.p2p_healthy {
            "P2P直连"
        } else {
            "QUIC中继"
        };
        let rtt = peer
            .rtt_us
            .map(format_rtt)
            .unwrap_or_else(|| "-".to_string());
        let loss = format!("{:.1}%", peer.loss_rate * 100.0);
        let probe = if peer.health_window_probes == 0 {
            "-".to_string()
        } else {
            format!("{:.1}%", peer.probe_success_rate * 100.0)
        };
        let bandwidth = format_bandwidth(peer.bandwidth_bps);
        let packets = format!("{}/{}", peer.rx_packets, peer.tx_packets);
        let endpoint = peer
            .endpoints
            .first()
            .map(|addr| addr.to_string())
            .unwrap_or_else(|| "-".to_string());

        println!(
            "{:<18}  {:<30}  {:<8}  {:<8}  {:<8}  {:<8}  {:<10}  {:<12}  {}",
            peer.virtual_ip, client_id, path, rtt, loss, probe, bandwidth, packets, endpoint
        );
    }

    Ok(())
}

pub async fn ping(target: &str, count: u32) -> Result<()> {
    println!("正在测试节点连通性: {}", target);
    println!("─────────────────────────────────────");
    println!();

    // 支持解析 IPv4、IPv6，或通过本地 IPC 自动匹配在线对等节点
    let target_ip = match target.parse::<std::net::IpAddr>() {
        Ok(ip) => ip,
        Err(_) => {
            // 尝试通过本地控制接口 IPC 查找节点
            if let Ok(crate::ipc::IpcResponse::Peers(peers)) =
                crate::ipc::request(crate::ipc::IpcRequest::Peers).await
            {
                let matched = peers.into_iter().find(|p| {
                    p.client_id == target
                        || p.client_id.starts_with(target)
                        || p.virtual_ip.to_string() == target
                });
                if let Some(peer) = matched {
                    println!("✓ 已匹配对等节点: {}", peer.client_id);
                    println!("  虚拟 IPv6: {}", peer.virtual_ip);
                    println!();
                    std::net::IpAddr::V6(peer.virtual_ip)
                } else {
                    anyhow::bail!(
                        "无法解析目标地址: {} (既非有效 IP，也未匹配到在线节点)",
                        target
                    );
                }
            } else {
                anyhow::bail!(
                    "无效的 IP 地址: {} (且本地未连接到运行中的 fuckipv6 进程)",
                    target
                );
            }
        }
    };

    let (program, args) = platform_ping_command(target_ip, count);

    println!("正在调用系统 {} 命令...", program);
    println!();

    let output = tokio::process::Command::new(program)
        .args(args)
        .output()
        .await
        .with_context(|| format!("执行 {} 命令失败", program))?;

    if output.status.success() {
        print!("{}", String::from_utf8_lossy(&output.stdout));
    } else {
        print!("{}", String::from_utf8_lossy(&output.stderr));
        anyhow::bail!("网络探测失败 (Ping failed)");
    }

    Ok(())
}

fn platform_ping_command(target_ip: std::net::IpAddr, count: u32) -> (&'static str, Vec<String>) {
    let count_str = count.to_string();
    let ip_str = target_ip.to_string();

    #[cfg(target_os = "windows")]
    {
        let flag = match target_ip {
            std::net::IpAddr::V4(_) => "-4",
            std::net::IpAddr::V6(_) => "-6",
        };
        (
            "ping",
            vec![flag.to_string(), "-n".to_string(), count_str, ip_str],
        )
    }

    #[cfg(not(target_os = "windows"))]
    {
        match target_ip {
            std::net::IpAddr::V4(_) => ("ping", vec!["-c".to_string(), count_str, ip_str]),
            std::net::IpAddr::V6(_) => {
                if std::path::Path::new("/bin/ping6").exists()
                    || std::path::Path::new("/usr/bin/ping6").exists()
                {
                    ("ping6", vec!["-c".to_string(), count_str, ip_str])
                } else {
                    (
                        "ping",
                        vec!["-6".to_string(), "-c".to_string(), count_str, ip_str],
                    )
                }
            }
        }
    }
}

pub async fn stats() -> Result<()> {
    println!("网络统计信息 (Network Statistics)");
    println!("─────────────────────────────────────");
    println!();

    let tun_name = "fuckipv6";

    println!("TUN 设备接口: {}", tun_name);

    print_platform_stats(tun_name).await?;

    println!();
    println!("如需查看实时 P2P 连接与传输统计，请运行 'fuckipv6 status'");

    Ok(())
}

async fn print_platform_stats(tun_name: &str) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        print_linux_stats(tun_name).await?;
    }

    #[cfg(target_os = "macos")]
    {
        print_macos_stats(tun_name).await?;
    }

    #[cfg(target_os = "windows")]
    {
        let _ = tun_name;
        print_windows_stats().await?;
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = tun_name;
        println!("当前平台未实现 TUN 设备统计信息采集。");
    }

    Ok(())
}

#[cfg(target_os = "linux")]
async fn print_linux_stats(tun_name: &str) -> Result<()> {
    let stats_path = format!("/sys/class/net/{}/statistics", tun_name);

    if std::path::Path::new(&stats_path).exists() {
        println!();
        println!("下行接收 (RX - Received):");
        if let Ok(bytes) = tokio::fs::read_to_string(format!("{}/rx_bytes", stats_path)).await {
            let bytes: u64 = bytes.trim().parse().unwrap_or(0);
            println!(
                "  字节流量: {} ({:.2} MB)",
                bytes,
                bytes as f64 / 1_048_576.0
            );
        }
        if let Ok(packets) = tokio::fs::read_to_string(format!("{}/rx_packets", stats_path)).await {
            println!("  数据包数: {}", packets.trim());
        }

        println!();
        println!("上行发送 (TX - Transmitted):");
        if let Ok(bytes) = tokio::fs::read_to_string(format!("{}/tx_bytes", stats_path)).await {
            let bytes: u64 = bytes.trim().parse().unwrap_or(0);
            println!(
                "  字节流量: {} ({:.2} MB)",
                bytes,
                bytes as f64 / 1_048_576.0
            );
        }
        if let Ok(packets) = tokio::fs::read_to_string(format!("{}/tx_packets", stats_path)).await {
            println!("  数据包数: {}", packets.trim());
        }
    } else {
        println!("未找到 TUN 设备，客户端是否正在运行？");
    }

    Ok(())
}

#[cfg(target_os = "macos")]
async fn print_macos_stats(tun_name: &str) -> Result<()> {
    let output = tokio::process::Command::new("netstat")
        .args(["-ibn"])
        .output()
        .await
        .context("Failed to execute netstat -ibn")?;

    if !output.status.success() {
        print!("{}", String::from_utf8_lossy(&output.stderr));
        anyhow::bail!("Failed to read macOS interface statistics");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let Some(stats) = parse_macos_netstat_stats(&stdout, tun_name) else {
        println!("未找到 TUN 设备，客户端是否正在运行？");
        println!("在 macOS 上即使 tun.name 为 fuckipv6，实际网卡通常为 utunN。");
        return Ok(());
    };

    print_interface_stats(stats);
    Ok(())
}

#[cfg(target_os = "windows")]
async fn print_windows_stats() -> Result<()> {
    let output = tokio::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-NetAdapterStatistics -Name 'fuckipv6' | Format-List",
        ])
        .output()
        .await
        .context("执行 PowerShell 网卡统计查询失败")?;

    if output.status.success() {
        print!("{}", String::from_utf8_lossy(&output.stdout));
    } else {
        print!("{}", String::from_utf8_lossy(&output.stderr));
        println!("未找到 TUN 设备，客户端是否正在运行？");
    }

    Ok(())
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, PartialEq, Eq)]
struct InterfaceStats {
    rx_packets: u64,
    rx_bytes: u64,
    tx_packets: u64,
    tx_bytes: u64,
}

#[cfg(any(target_os = "macos", test))]
fn parse_macos_netstat_stats(output: &str, tun_name: &str) -> Option<InterfaceStats> {
    output
        .lines()
        .filter_map(parse_macos_netstat_line)
        .find(|(name, _)| *name == tun_name || name.starts_with("utun"))
        .map(|(_, stats)| stats)
}

#[cfg(any(target_os = "macos", test))]
fn parse_macos_netstat_line(line: &str) -> Option<(&str, InterfaceStats)> {
    let columns: Vec<_> = line.split_whitespace().collect();
    if columns.len() < 9 || columns[0] == "Name" {
        return None;
    }

    let numeric_tail: Vec<u64> = columns
        .iter()
        .skip(3)
        .filter_map(|column| column.parse().ok())
        .collect();

    if numeric_tail.len() < 7 {
        return None;
    }

    let offset = numeric_tail.len() - 7;
    let rx_packets = numeric_tail[offset];
    let rx_bytes = numeric_tail[offset + 2];
    let tx_packets = numeric_tail[offset + 3];
    let tx_bytes = numeric_tail[offset + 5];

    Some((
        columns[0],
        InterfaceStats {
            rx_packets,
            rx_bytes,
            tx_packets,
            tx_bytes,
        },
    ))
}

#[cfg(target_os = "macos")]
fn print_interface_stats(stats: InterfaceStats) {
    println!();
    println!("下行接收 (RX - Received):");
    println!(
        "  流量字节: {} ({:.2} MB)",
        stats.rx_bytes,
        stats.rx_bytes as f64 / 1_048_576.0
    );
    println!("  数据包数: {}", stats.rx_packets);

    println!();
    println!("上行发送 (TX - Transmitted):");
    println!(
        "  流量字节: {} ({:.2} MB)",
        stats.tx_bytes,
        stats.tx_bytes as f64 / 1_048_576.0
    );
    println!("  数据包数: {}", stats.tx_packets);
}

pub async fn route(interface: &str, show_all: bool) -> Result<()> {
    println!("IPv6 路由表 (IPv6 Routes)");
    println!("─────────────────────────────────────");
    println!();

    print_platform_routes(interface, show_all).await
}

async fn print_platform_routes(interface: &str, show_all: bool) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        print_linux_routes(interface, show_all).await?;
    }

    #[cfg(target_os = "windows")]
    {
        let _ = (interface, show_all);
        print_windows_routes().await?;
    }

    #[cfg(target_os = "macos")]
    {
        print_macos_routes(interface, show_all).await?;
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = (interface, show_all);
        anyhow::bail!("当前平台尚未实现路由表查看功能");
    }

    Ok(())
}

pub async fn logs(file: Option<&str>, service: &str, lines: usize) -> Result<()> {
    println!("系统运行日志 (Runtime Logs)");
    println!("─────────────────────────────────────");
    println!();

    if service.eq_ignore_ascii_case("memory") || service.eq_ignore_ascii_case("errors") {
        print_memory_logs(lines).await;
        return Ok(());
    }

    if let Some(file) = file {
        print_file_logs(file, lines).await?;
        return Ok(());
    }

    print_platform_logs(service, lines).await
}

#[cfg(target_os = "linux")]
async fn print_linux_routes(interface: &str, show_all: bool) -> Result<()> {
    let output = tokio::process::Command::new("ip")
        .args(["-6", "route", "show"])
        .output()
        .await
        .context("执行 ip -6 route show 失败")?;

    if !output.status.success() {
        print!("{}", String::from_utf8_lossy(&output.stderr));
        anyhow::bail!("读取 IPv6 路由表失败");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let routes = parse_linux_ip_routes(&stdout);
    let routes: Vec<_> = routes
        .into_iter()
        .filter(|route| show_all || route.uses_interface(interface))
        .collect();

    print_linux_route_rows(routes, interface, show_all);
    Ok(())
}

#[cfg(target_os = "linux")]
fn print_linux_route_rows(routes: Vec<RouteEntry>, interface: &str, show_all: bool) {
    if routes.is_empty() {
        if show_all {
            println!("未找到任何 IPv6 路由规则。");
        } else {
            println!("未找到接口 '{}' 的 IPv6 路由规则。", interface);
            println!("使用 --all 参数显示系统完整 IPv6 路由表。");
        }
        return;
    }

    println!(
        "{:<24}  {:<18}  {:<8}  原始规则 (Raw)",
        "目标网段 (Destination)", "下一跳网关 (Gateway)", "优先级 (Metric)"
    );
    println!("{}", "─".repeat(96));
    for route in routes {
        let gateway = route.via.as_deref().unwrap_or("-");
        let metric = route
            .metric
            .map(|metric| metric.to_string())
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{:<24}  {:<18}  {:<8}  {}",
            route.destination, gateway, metric, route.raw
        );
    }
}

#[cfg(target_os = "windows")]
async fn print_windows_routes() -> Result<()> {
    let output = tokio::process::Command::new("netsh")
        .args(["interface", "ipv6", "show", "route"])
        .output()
        .await
        .context("Failed to execute netsh interface ipv6 show route")?;

    if !output.status.success() {
        print!("{}", String::from_utf8_lossy(&output.stderr));
        anyhow::bail!("Failed to read IPv6 routes");
    }

    print!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}

#[cfg(target_os = "macos")]
async fn print_macos_routes(interface: &str, show_all: bool) -> Result<()> {
    let output = tokio::process::Command::new("netstat")
        .args(["-rn", "-f", "inet6"])
        .output()
        .await
        .context("Failed to execute netstat -rn -f inet6")?;

    if !output.status.success() {
        print!("{}", String::from_utf8_lossy(&output.stderr));
        anyhow::bail!("Failed to read IPv6 routes");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let routes: Vec<&str> = stdout
        .lines()
        .filter(|line| show_all || line.contains(interface) || line.contains("utun"))
        .collect();

    if routes.is_empty() {
        if show_all {
            println!("未找到任何 IPv6 路由规则。");
        } else {
            println!("未找到接口 '{}' 的 IPv6 路由规则。", interface);
            println!("使用 --all 参数显示系统完整 IPv6 路由表。");
        }
        return Ok(());
    }

    for route in routes {
        println!("{}", route);
    }

    Ok(())
}

async fn print_memory_logs(lines: usize) {
    let lines = ipc::request(IpcRequest::Logs { lines })
        .await
        .ok()
        .and_then(|response| match response {
            IpcResponse::Logs(lines) => Some(lines),
            _ => None,
        })
        .unwrap_or_else(|| recent_error_log_lines(lines));

    for line in lines {
        println!("{}", line);
    }
}

async fn print_file_logs(file: &str, lines: usize) -> Result<()> {
    let output = tokio::process::Command::new("tail")
        .args(["-n", &lines.to_string(), file])
        .output()
        .await
        .with_context(|| format!("Failed to read log file {}", file))?;

    if !output.status.success() {
        print!("{}", String::from_utf8_lossy(&output.stderr));
        anyhow::bail!("Failed to read log file");
    }

    print!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}

async fn print_platform_logs(service: &str, lines: usize) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        print_journal_logs(service, lines).await?;
    }

    #[cfg(target_os = "windows")]
    {
        print_windows_event_logs(service, lines).await?;
    }

    #[cfg(target_os = "macos")]
    {
        print_macos_logs(service, lines).await?;
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        anyhow::bail!("Log inspection is not implemented for this platform; use --file");
    }

    Ok(())
}

#[cfg(target_os = "linux")]
async fn print_journal_logs(service: &str, lines: usize) -> Result<()> {
    let service_name = normalize_service_name(service);
    let output = tokio::process::Command::new("journalctl")
        .args(["-u", &service_name, "-n", &lines.to_string(), "--no-pager"])
        .output()
        .await
        .context("Failed to execute journalctl")?;

    if !output.status.success() {
        print!("{}", String::from_utf8_lossy(&output.stderr));
        anyhow::bail!("Failed to read systemd logs for {}", service_name);
    }

    print!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}

#[cfg(target_os = "windows")]
async fn print_windows_event_logs(service: &str, lines: usize) -> Result<()> {
    let log_name = if service.eq_ignore_ascii_case("coordinator") {
        "fuckipv6-coordinator"
    } else {
        "fuckipv6"
    };
    let output = tokio::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "Get-EventLog -LogName Application -Source '{}' -Newest {}",
                log_name, lines
            ),
        ])
        .output()
        .await
        .context("Failed to execute PowerShell log query")?;

    if !output.status.success() {
        print!("{}", String::from_utf8_lossy(&output.stderr));
        anyhow::bail!("Failed to read Windows event logs");
    }

    print!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}

#[cfg(target_os = "macos")]
async fn print_macos_logs(service: &str, lines: usize) -> Result<()> {
    let predicate = if service.eq_ignore_ascii_case("coordinator") {
        "process == \"fuckipv6-coordinator\""
    } else {
        "process == \"fuckipv6\""
    };
    let output = tokio::process::Command::new("log")
        .args([
            "show",
            "--style",
            "compact",
            "--last",
            "1h",
            "--predicate",
            predicate,
        ])
        .output()
        .await
        .context("Failed to execute macOS log show")?;

    if !output.status.success() {
        print!("{}", String::from_utf8_lossy(&output.stderr));
        anyhow::bail!("Failed to read macOS unified logs");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let selected = tail_text_lines(&stdout, lines);
    for line in selected {
        println!("{}", line);
    }

    Ok(())
}

pub fn version() {
    println!("fuckipv6 v{}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("IPv6 虚拟专网 - 点对点分布式 Mesh 重叠网");
    println!();
    println!("构建信息 (Build Info):");
    println!("  版本:      {}", env!("CARGO_PKG_VERSION"));
    println!("  维护团队:  {}", env!("CARGO_PKG_AUTHORS"));
    println!("  开发阶段:  第 1 阶段 (MVP)");
    println!();
    println!("支持特性 (Features):");
    println!("  - WebSocket 双向信令");
    println!("  - 全自动 IPv4 / IPv6 虚拟专网地址分配");
    println!("  - 跨平台 TUN 虚拟网卡 (Linux/macOS/Windows)");
    println!("  - UDP P2P 高性能穿透直连");
    println!("  - Ed25519 节点签名与身份鉴权");
    println!("  - UDP 打洞探测与对称 NAT 智能穿透");
    println!("  - QUIC 高性能加密中继与降级回退");
    println!("  - 运行期 IPC 实时遥测通信");
    println!("  - 赛博深空实时图形化 WebUI 控制中心");
    println!("  - 原生智能分流规则 (绕过大陆/全局代理)");
    println!();
}

pub async fn quic_info() -> Result<()> {
    let certificate = quic::generate_node_certificate()?;
    let _server_config = quic::build_server_config(&certificate)?;
    let _client_config = quic::build_client_config(certificate.public_key_pin)?;
    let self_test = quic::run_loopback_self_test(&certificate).await?;
    let zero_rtt = quic::run_zero_rtt_reconnect_self_test(&certificate).await?;

    println!("QUIC 协议与传输层自检:");
    println!(
        "  应用层协议 (ALPN): {}",
        String::from_utf8_lossy(fuckipv6_common::FUCKIPV6_QUIC_ALPN)
    );
    println!("  节点证书: 自签名 (Self-signed ECDSA)");
    println!(
        "  公钥指纹 (Pin): {}",
        hex_encode(&certificate.public_key_pin)
    );
    println!("  环回测试地址: {}", self_test.server_addr);
    println!(
        "  双向帧回显测试: {} 字节通过",
        self_test.echoed.payload.len()
    );
    println!(
        "  0-RTT 快速重连: {}",
        if zero_rtt.accepted {
            "已支持并握手成功 (Accepted)"
        } else {
            "未启用或被拒绝 (Rejected)"
        }
    );
    Ok(())
}

pub async fn register_check(
    coordinator_url: &str,
    key_path: &str,
    p2p_port: u16,
    timeout_secs: u64,
) -> Result<()> {
    let identity = Identity::load_or_generate(key_path).context("无法加载客户端身份")?;
    let client_id = identity.client_id().to_string();
    let signaling = tokio::time::timeout(
        Duration::from_secs(timeout_secs.max(1)),
        SignalingClient::connect(coordinator_url, identity, p2p_port, NatType::Unknown),
    )
    .await
    .context("注册检查超时")?
    .context("注册检查无法连接协调服务器")?;

    let assigned_ip = signaling
        .assigned_ip()
        .await
        .ok_or_else(|| anyhow::anyhow!("注册检查未获得虚拟 IP"))?;
    let relay_endpoint = signaling.relay_endpoint().await;

    println!("注册检查成功 (Register check OK)");
    println!("  客户端 ID / Client ID: {}", client_id);
    println!("  虚拟 IP / Virtual IP: {}", assigned_ip);
    match relay_endpoint {
        Some(endpoint) => {
            println!("  QUIC 中继 / QUIC relay: {}", endpoint.addr);
            println!(
                "  QUIC 中继公钥指纹 / QUIC relay pin: {}",
                hex_encode(&endpoint.public_key_pin)
            );
        }
        None => println!("  QUIC 中继 (QUIC relay): 不可用 (unavailable)"),
    }

    signaling.close().await?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    Ok(())
}

pub fn init_config(output: &str) -> Result<()> {
    // 生成示例配置文件
    let config = ClientConfig {
        client_id: None,
        coordinator_url: "ws://localhost:6666".to_string(),
        private_key_path: "client_key.pem".into(),
        network: fuckipv6_common::NetworkConfig {
            virtual_network: Some("fd00::/64".to_string()),
            preferred_ip: None,
            mode: Default::default(),
            preferred_ipv4: None,
            preferred_ipv6: None,
        },
        tun: Default::default(),
        p2p: Default::default(),
        log: Default::default(),
        acl: Default::default(),
        routing: Default::default(),
        exit_node: Default::default(),
    };

    config.save_to_file(output)?;
    println!("配置文件已成功生成: {}", output);

    Ok(())
}

pub fn encrypt_config(input: &str, output: &str) -> Result<()> {
    let passphrase = config_passphrase_from_env()?;
    encrypt_config_file(input, output, &passphrase)?;
    println!("加密配置文件已写入: {}", output);
    Ok(())
}

pub fn decrypt_config(input: &str, output: Option<&str>) -> Result<()> {
    let passphrase = config_passphrase_from_env()?;
    let plaintext = decrypt_config_file(input, &passphrase)?;

    if let Some(output) = output {
        std::fs::write(output, plaintext)?;
        println!("解密配置文件已写入: {}", output);
    } else {
        print!("{}", String::from_utf8(plaintext)?);
    }

    Ok(())
}

fn config_passphrase_from_env() -> Result<String> {
    std::env::var("FUCKIPV6_CONFIG_PASSPHRASE")
        .map_err(|_| anyhow::anyhow!("配置加密需要设置环境变量 FUCKIPV6_CONFIG_PASSPHRASE"))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn abbreviate(value: &str, max_len: usize) -> String {
    if value.len() <= max_len {
        return value.to_string();
    }

    if max_len <= 1 {
        return value.chars().take(max_len).collect();
    }

    if max_len <= 3 {
        return value.chars().take(max_len).collect();
    }

    format!("{}...", value.chars().take(max_len - 3).collect::<String>())
}

fn format_duration(total_secs: u64) -> String {
    let days = total_secs / 86400;
    let hours = (total_secs % 86400) / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;

    if days > 0 {
        format!("{}天{}小时{}分", days, hours, minutes)
    } else if hours > 0 {
        format!("{}小时{}分{}秒", hours, minutes, seconds)
    } else if minutes > 0 {
        format!("{}分{}秒", minutes, seconds)
    } else {
        format!("{}秒", seconds)
    }
}

fn format_rtt(rtt_us: u64) -> String {
    if rtt_us >= 1000 {
        format!("{:.1}ms", rtt_us as f64 / 1000.0)
    } else {
        format!("{}us", rtt_us)
    }
}

fn format_bandwidth(bps: u64) -> String {
    const KIBIT: f64 = 1024.0;
    const MIBIT: f64 = 1024.0 * 1024.0;

    if bps >= 1024 * 1024 {
        format!("{:.1}Mb/s", bps as f64 / MIBIT)
    } else if bps >= 1024 {
        format!("{:.1}Kb/s", bps as f64 / KIBIT)
    } else if bps > 0 {
        format!("{}b/s", bps)
    } else {
        "-".to_string()
    }
}

#[cfg(target_os = "macos")]
fn tail_text_lines(text: &str, lines: usize) -> Vec<&str> {
    if lines == 0 {
        return Vec::new();
    }

    let mut selected: Vec<_> = text.lines().rev().take(lines).collect();
    selected.reverse();
    selected
}

#[cfg(target_os = "linux")]
fn normalize_service_name(service: &str) -> String {
    match service {
        "client" | "fuckipv6" => "fuckipv6-client.service".to_string(),
        "coordinator" | "fuckipv6-coordinator" => "fuckipv6-coordinator.service".to_string(),
        other if other.ends_with(".service") => other.to_string(),
        other => format!("{}.service", other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::sync::Notify;

    struct RecordingTransport {
        completed_tx: mpsc::UnboundedSender<(IpAddr, Vec<u8>)>,
    }

    impl OutboundTransport for RecordingTransport {
        fn send<'a>(
            &'a self,
            packet: OutboundPacket,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
            Box::pin(async move {
                if packet.payload().first() == Some(&1) {
                    sleep(Duration::from_millis(150)).await;
                }
                let _ = self
                    .completed_tx
                    .send((packet.dst_ip, packet.payload().to_vec()));
            })
        }
    }

    struct GatedTransport {
        started_tx: mpsc::UnboundedSender<()>,
        gate: Arc<Notify>,
    }

    impl OutboundTransport for GatedTransport {
        fn send<'a>(
            &'a self,
            _packet: OutboundPacket,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
            Box::pin(async move {
                let _ = self.started_tx.send(());
                self.gate.notified().await;
            })
        }
    }

    #[derive(Clone, Copy)]
    enum RelayMode {
        Success,
        Failure,
    }

    struct MatrixFallbackTransport {
        relay_mode: RelayMode,
        relay_tx: mpsc::UnboundedSender<Bytes>,
        signaling_tx: mpsc::UnboundedSender<Bytes>,
    }

    impl FallbackTransport for MatrixFallbackTransport {
        fn relay<'a>(
            &'a self,
            _src_ip: Ipv6Addr,
            _dst_ip: Ipv6Addr,
            encrypted: Bytes,
        ) -> Pin<Box<dyn Future<Output = RelayForwardOutcome> + Send + 'a>> {
            Box::pin(async move {
                let _ = self.relay_tx.send(encrypted);
                match self.relay_mode {
                    RelayMode::Success => RelayForwardOutcome::Sent,
                    RelayMode::Failure => {
                        RelayForwardOutcome::Failed(anyhow::anyhow!("injected relay failure"))
                    }
                }
            })
        }

        fn signaling(&self, _src_ip: IpAddr, _dst_ip: IpAddr, encrypted: Bytes) -> Result<()> {
            let _ = self.signaling_tx.send(encrypted);
            Ok(())
        }
    }

    fn outbound_packet(dst_ip: Ipv6Addr, marker: u8) -> OutboundPacket {
        OutboundPacket::owned(
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            IpAddr::V6(dst_ip),
            vec![marker],
        )
    }

    async fn connected_test_engines() -> Result<(P2pEngine, P2pEngine, Ipv6Addr, Ipv6Addr)> {
        let engine_a = P2pEngine::new(0).await?;
        let engine_b = P2pEngine::new(0).await?;
        let identity_a = Identity::generate()?;
        let identity_b = Identity::generate()?;
        let ip_a = "fd00::1".parse::<Ipv6Addr>()?;
        let ip_b = "fd00::2".parse::<Ipv6Addr>()?;

        engine_a.set_identity(identity_a.clone()).await;
        engine_b.set_identity(identity_b.clone()).await;
        engine_a.set_local_ip(ip_a).await;
        engine_b.set_local_ip(ip_b).await;

        let endpoint_a = std::net::SocketAddr::new(
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            engine_a.local_addr().port(),
        );
        let endpoint_b = std::net::SocketAddr::new(
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            engine_b.local_addr().port(),
        );
        engine_a
            .add_peer(PeerInfo::new(
                "b".to_string(),
                ip_b,
                identity_b.public_key(),
                vec![endpoint_b],
                NatType::Open,
            ))
            .await?;
        engine_b
            .add_peer(PeerInfo::new(
                "a".to_string(),
                ip_a,
                identity_a.public_key(),
                vec![endpoint_a],
                NatType::Open,
            ))
            .await?;

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if engine_a
                    .has_established_session_by_ip(IpAddr::V6(ip_b))
                    .await
                    && engine_b
                        .has_established_session_by_ip(IpAddr::V6(ip_a))
                        .await
                {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .context("test peers did not establish a secure session")?;

        Ok((engine_a, engine_b, ip_a, ip_b))
    }

    async fn assert_production_udp_batch(
        payload_len: usize,
        expect_parallel_crypto: bool,
    ) -> Result<()> {
        let (engine_a, engine_b, ip_a, ip_b) = connected_test_engines().await?;
        let (relay_tx, _relay_rx) = mpsc::unbounded_channel();
        let (signaling_tx, _signaling_rx) = mpsc::unbounded_channel();
        let transport = P2pOutboundTransport::with_fallback(
            engine_a.clone(),
            Arc::new(MatrixFallbackTransport {
                relay_mode: RelayMode::Success,
                relay_tx,
                signaling_tx,
            }),
        );

        let payloads: Vec<Vec<u8>> = (0..8u8).map(|index| vec![index; payload_len]).collect();
        let before = engine_a.udp_batch_metrics();
        let crypto_before = fuckipv6_secure_session::crypto_batch_metrics();
        transport
            .send_packets(
                payloads
                    .iter()
                    .cloned()
                    .map(|payload| {
                        OutboundPacket::owned(IpAddr::V6(ip_a), IpAddr::V6(ip_b), payload)
                    })
                    .collect(),
            )
            .await;

        let after = engine_a.udp_batch_metrics();
        assert_eq!(
            after.send_syscalls - before.send_syscalls,
            1,
            "one same-peer sendmmsg must cover all 8 {payload_len}-byte packets"
        );
        assert_eq!(after.send_datagrams - before.send_datagrams, 8);
        assert_eq!(after.max_send_batch, 8);
        assert_eq!(after.fallback_packets, before.fallback_packets);
        if expect_parallel_crypto {
            let crypto_after = fuckipv6_secure_session::crypto_batch_metrics();
            assert!(
                crypto_after.parallel_jobs > crypto_before.parallel_jobs,
                "the production batch path must dispatch AEAD work to the crypto pool"
            );
            assert!(
                crypto_after.parallel_packets >= crypto_before.parallel_packets + 8,
                "all eight production packets must be covered by parallel AEAD work"
            );
        }

        for expected in payloads {
            let inbound = tokio::time::timeout(Duration::from_secs(1), engine_b.recv())
                .await?
                .expect("receiver must deliver every batched packet");
            let InboundPacket::AuthenticatedPlaintext {
                src_ip, payload, ..
            } = inbound
            else {
                panic!("direct UDP batch must deliver authenticated plaintext")
            };
            assert_eq!(src_ip, IpAddr::V6(ip_a));
            assert_eq!(payload.as_slice(), expected);
        }
        Ok(())
    }

    #[tokio::test]
    async fn udp_batch_64_byte_payloads_use_one_send_syscall() -> Result<()> {
        // 8 x 64 B is below the 4 KiB parallel-AEAD threshold. This case proves
        // only that small packets sharing a peer are coalesced by sendmmsg.
        assert_production_udp_batch(64, false).await
    }

    #[tokio::test]
    async fn udp_batch_1420_byte_payloads_use_one_send_syscall() -> Result<()> {
        assert_production_udp_batch(1420, true).await
    }

    #[tokio::test]
    async fn fallback_matrix_reuses_one_ciphertext_for_relay_and_signaling() -> Result<()> {
        let (engine_a, engine_b, ip_a, ip_b) = connected_test_engines().await?;

        for (relay_mode, expected_signaling, payload) in [
            (
                RelayMode::Success,
                false,
                b"udp failed, quic relay succeeded".as_slice(),
            ),
            (
                RelayMode::Failure,
                true,
                b"udp and quic failed, signaling succeeded".as_slice(),
            ),
        ] {
            let (relay_tx, mut relay_rx) = mpsc::unbounded_channel();
            let (signaling_tx, mut signaling_rx) = mpsc::unbounded_channel();
            let transport = P2pOutboundTransport::with_fallback(
                engine_a.clone(),
                Arc::new(MatrixFallbackTransport {
                    relay_mode,
                    relay_tx,
                    signaling_tx,
                }),
            );

            engine_a.set_fail_direct_transport(true);
            let seals_before = engine_a.direct_seal_count();
            transport
                .send_packet(OutboundPacket::owned(
                    IpAddr::V6(ip_a),
                    IpAddr::V6(ip_b),
                    payload.to_vec(),
                ))
                .await;
            engine_a.set_fail_direct_transport(false);

            assert_eq!(
                engine_a.direct_seal_count(),
                seals_before + 1,
                "the complete direct/relay/signaling chain must seal exactly once"
            );
            let relay_ciphertext = relay_rx
                .try_recv()
                .expect("the QUIC relay fallback must receive the sealed packet");
            assert!(relay_rx.try_recv().is_err());

            let signaling_ciphertext = if expected_signaling {
                let ciphertext = signaling_rx
                    .try_recv()
                    .expect("signaling fallback must receive the sealed packet");
                assert_eq!(
                    ciphertext, relay_ciphertext,
                    "signaling must reuse the QUIC relay ciphertext, not reseal plaintext"
                );
                assert_eq!(
                    ciphertext.as_ptr(),
                    relay_ciphertext.as_ptr(),
                    "signaling and QUIC relay must share one ciphertext allocation"
                );
                assert!(signaling_rx.try_recv().is_err());
                Some(ciphertext)
            } else {
                assert!(signaling_rx.try_recv().is_err());
                None
            };

            assert_eq!(
                engine_b.decrypt_from_peer(ip_a, &relay_ciphertext).await?,
                payload,
                "the fallback ciphertext must authenticate on the peer"
            );
            assert!(
                engine_b
                    .decrypt_from_peer(ip_a, &relay_ciphertext)
                    .await
                    .is_err(),
                "submitting the same ciphertext through another transport must replay-fail"
            );
            if let Some(ciphertext) = signaling_ciphertext {
                assert_eq!(ciphertext, relay_ciphertext);
            }
        }

        Ok(())
    }

    #[tokio::test]
    async fn slow_peer_worker_does_not_block_another_peer_or_deliver_out_of_order() {
        let (completed_tx, mut completed_rx) = mpsc::unbounded_channel();
        let dispatcher =
            OutboundDispatcher::for_test(Arc::new(RecordingTransport { completed_tx }));
        let slow_peer = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 2);
        let fast_peer = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 3);

        assert_eq!(
            dispatcher.submit(outbound_packet(slow_peer, 1)),
            OutboundSubmitOutcome::Queued
        );
        assert_eq!(
            dispatcher.submit(outbound_packet(slow_peer, 2)),
            OutboundSubmitOutcome::Queued
        );
        assert_eq!(
            dispatcher.submit(outbound_packet(fast_peer, 3)),
            OutboundSubmitOutcome::Queued
        );

        let first = tokio::time::timeout(Duration::from_millis(100), completed_rx.recv())
            .await
            .expect("fast peer must not wait for the slow peer")
            .expect("completion channel must stay open");
        assert_eq!(first, (IpAddr::V6(fast_peer), vec![3]));

        let first_slow = tokio::time::timeout(Duration::from_secs(1), completed_rx.recv())
            .await
            .expect("slow peer must eventually complete")
            .expect("completion channel must stay open");
        let second_slow = tokio::time::timeout(Duration::from_secs(1), completed_rx.recv())
            .await
            .expect("slow peer's queued packet must eventually complete")
            .expect("completion channel must stay open");
        assert_eq!(first_slow, (IpAddr::V6(slow_peer), vec![1]));
        assert_eq!(second_slow, (IpAddr::V6(slow_peer), vec![2]));
    }

    #[tokio::test]
    async fn slow_worker_does_not_block_heartbeat_or_control_events() {
        let (started_tx, mut started_rx) = mpsc::unbounded_channel();
        let gate = Arc::new(Notify::new());
        let dispatcher = OutboundDispatcher::for_test(Arc::new(GatedTransport {
            started_tx,
            gate: gate.clone(),
        }));
        let peer = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 2);

        assert_eq!(
            dispatcher.submit(outbound_packet(peer, 1)),
            OutboundSubmitOutcome::Queued
        );
        tokio::time::timeout(Duration::from_millis(100), started_rx.recv())
            .await
            .expect("worker must start")
            .expect("worker must remain connected");

        let (control_tx, mut control_rx) = mpsc::unbounded_channel();
        control_tx
            .send("control")
            .expect("control receiver must remain connected");
        let mut heartbeat = tokio::time::interval(Duration::from_millis(5));
        heartbeat.tick().await;

        tokio::time::timeout(Duration::from_millis(50), async {
            tokio::select! {
                _ = heartbeat.tick() => {}
                command = control_rx.recv() => {
                    assert_eq!(command, Some("control"));
                }
                _ = sleep(Duration::from_millis(100)) => {
                    panic!("slow worker blocked central heartbeat/control");
                }
            }
        })
        .await
        .expect("central loop must remain responsive");

        gate.notify_waiters();
    }

    #[tokio::test]
    async fn stalled_outbound_worker_does_not_block_tun_or_inbound_progress() {
        let (started_tx, mut started_rx) = mpsc::unbounded_channel();
        let gate = Arc::new(Notify::new());
        let dispatcher = OutboundDispatcher::for_test(Arc::new(GatedTransport {
            started_tx,
            gate: gate.clone(),
        }));
        let peer = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 2);

        assert_eq!(
            dispatcher.submit(outbound_packet(peer, 1)),
            OutboundSubmitOutcome::Queued
        );
        tokio::time::timeout(Duration::from_millis(100), started_rx.recv())
            .await
            .expect("worker must start")
            .expect("worker must remain connected");

        let (tun_tx, mut tun_rx) = mpsc::unbounded_channel();
        let (inbound_tx, mut inbound_rx) = mpsc::unbounded_channel();
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
        tun_tx
            .send(b"tun packet".as_slice())
            .expect("TUN receiver must remain connected");
        inbound_tx
            .send(b"authenticated inbound".as_slice())
            .expect("inbound receiver must remain connected");
        tokio::spawn(async move {
            sleep(Duration::from_millis(5)).await;
            let _ = shutdown_tx.send(true);
        });

        tokio::time::timeout(Duration::from_millis(100), async {
            let mut tun_seen = false;
            let mut inbound_seen = false;
            let mut shutdown_seen = false;
            while !(tun_seen && inbound_seen && shutdown_seen) {
                tokio::select! {
                    packet = tun_rx.recv(), if !tun_seen => {
                        assert_eq!(packet, Some(b"tun packet".as_slice()));
                        tun_seen = true;
                    }
                    packet = inbound_rx.recv(), if !inbound_seen => {
                        assert_eq!(packet, Some(b"authenticated inbound".as_slice()));
                        inbound_seen = true;
                    }
                    changed = shutdown_rx.changed(), if !shutdown_seen => {
                        changed.expect("shutdown channel must remain connected");
                        shutdown_seen = true;
                    }
                    _ = sleep(Duration::from_millis(150)) => {
                        panic!("stalled outbound worker blocked TUN, inbound, or shutdown progress");
                    }
                }
            }
        })
        .await
        .expect("data-plane events must remain responsive while fallback is stalled");

        gate.notify_waiters();
    }

    #[tokio::test]
    async fn outbound_queue_is_bounded_under_a_stalled_peer() {
        let (started_tx, mut started_rx) = mpsc::unbounded_channel();
        let gate = Arc::new(Notify::new());
        let dispatcher = OutboundDispatcher::for_test(Arc::new(GatedTransport {
            started_tx,
            gate: gate.clone(),
        }));
        let peer = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 2);

        assert_eq!(
            dispatcher.submit(outbound_packet(peer, 0)),
            OutboundSubmitOutcome::Queued
        );
        tokio::time::timeout(Duration::from_millis(100), started_rx.recv())
            .await
            .expect("worker must start")
            .expect("worker must remain connected");

        for marker in 1..=OUTBOUND_QUEUE_CAPACITY {
            assert_eq!(
                dispatcher.submit(outbound_packet(peer, marker as u8)),
                OutboundSubmitOutcome::Queued
            );
        }
        assert_eq!(
            dispatcher.submit(outbound_packet(peer, 0)),
            OutboundSubmitOutcome::QueueFull
        );

        gate.notify_waiters();
    }

    struct TempPath(PathBuf);

    impl TempPath {
        fn new(extension: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before epoch")
                .as_nanos();
            Self(std::env::temp_dir().join(format!(
                "fuckipv6-cli-test-{}-{nonce}.{extension}",
                std::process::id()
            )))
        }

        fn as_path(&self) -> &Path {
            &self.0
        }

        fn as_str(&self) -> &str {
            self.0.to_str().expect("temporary path must be UTF-8")
        }
    }

    impl Drop for TempPath {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn test_config() -> ClientConfig {
        ClientConfig {
            client_id: None,
            coordinator_url: "ws://127.0.0.1:6666".to_string(),
            private_key_path: "client_key.pem".into(),
            network: Default::default(),
            tun: Default::default(),
            p2p: Default::default(),
            log: Default::default(),
            acl: Default::default(),
            routing: Default::default(),
            exit_node: Default::default(),
        }
    }

    #[test]
    fn runtime_options_update_and_reset_exit_node() -> Result<()> {
        let temp = TempPath::new("toml");
        test_config().save_to_file(temp.as_path())?;
        let mut config = ClientConfig::from_file(temp.as_path())?;

        let enable = RuntimeOptions {
            routing_mode: Some("global-proxy".to_string()),
            exit_node: Some("fd00::1001".to_string()),
            is_exit_node: Some(true),
            ..Default::default()
        };
        assert!(persist_runtime_options(
            temp.as_str(),
            &mut config,
            &enable
        )?);
        let saved = ClientConfig::from_file(temp.as_path())?;
        assert_eq!(
            saved.routing.mode,
            fuckipv6_common::RoutingMode::GlobalProxy
        );
        assert_eq!(saved.routing.exit_node.as_deref(), Some("fd00::1001"));
        assert!(saved.exit_node.enabled);

        let disable = RuntimeOptions {
            is_exit_node: Some(false),
            ..Default::default()
        };
        assert!(persist_runtime_options(
            temp.as_str(),
            &mut config,
            &disable
        )?);
        let saved = ClientConfig::from_file(temp.as_path())?;
        assert!(!saved.exit_node.enabled);
        assert_eq!(
            saved.routing.mode,
            fuckipv6_common::RoutingMode::GlobalProxy
        );

        assert!(!persist_runtime_options(
            temp.as_str(),
            &mut config,
            &RuntimeOptions::default()
        )?);
        Ok(())
    }

    #[test]
    fn encrypted_config_is_not_overwritten_by_runtime_options() -> Result<()> {
        let plaintext = TempPath::new("toml");
        let encrypted = TempPath::new("toml.enc");
        test_config().save_to_file(plaintext.as_path())?;
        encrypt_config_file(plaintext.as_path(), encrypted.as_path(), "secret")?;
        let original = std::fs::read(encrypted.as_path())?;
        let mut config = ClientConfig::from_file(plaintext.as_path())?;

        let options = RuntimeOptions {
            is_exit_node: Some(true),
            ..Default::default()
        };
        assert!(persist_runtime_options(
            encrypted.as_str(),
            &mut config,
            &options
        )?);
        assert!(config.exit_node.enabled);
        assert_eq!(std::fs::read(encrypted.as_path())?, original);
        Ok(())
    }

    #[test]
    fn parses_runtime_routing_modes() {
        assert_eq!(
            parse_runtime_routing_mode("GlobalProxy").unwrap(),
            fuckipv6_common::RoutingMode::GlobalProxy
        );
        assert_eq!(
            parse_runtime_routing_mode("bypass-mainland").unwrap(),
            fuckipv6_common::RoutingMode::BypassMainland
        );
        assert_eq!(
            parse_runtime_routing_mode("mesh_only").unwrap(),
            fuckipv6_common::RoutingMode::MeshOnly
        );
    }

    #[test]
    fn rejects_unknown_runtime_routing_mode() {
        let error = parse_runtime_routing_mode("not-a-route-mode")
            .expect_err("unknown modes must not silently fall back");
        assert!(error.to_string().contains("未知路由模式"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn exit_forwarder_transitions_are_idempotent() {
        assert_eq!(
            exit_forwarding_action(true, false),
            ExitForwardingAction::Install
        );
        assert_eq!(
            exit_forwarding_action(true, true),
            ExitForwardingAction::Keep
        );
        assert_eq!(
            exit_forwarding_action(false, true),
            ExitForwardingAction::Remove
        );
        assert_eq!(
            exit_forwarding_action(false, false),
            ExitForwardingAction::Keep
        );
    }

    #[test]
    fn inbound_inner_source_must_match_authenticated_peer() {
        let virtual_ips =
            VirtualIps::dual_stack("10.8.0.2".parse().unwrap(), "fd00::1002".parse().unwrap());
        let peer = PeerInfo::new_with_ips(
            "peer-a".to_string(),
            virtual_ips.clone(),
            vec![0; 32],
            Vec::new(),
            NatType::FullCone,
        );
        let peer = crate::p2p::connection::PeerRouteInfo::from(&peer);
        let mut packet = vec![0u8; 20];
        packet[0] = 0x45;
        packet[12..16].copy_from_slice(&[10, 8, 0, 2]);
        packet[16..20].copy_from_slice(&[1, 1, 1, 1]);
        assert_eq!(
            authenticated_inbound_source(&packet, &peer, &virtual_ips, None),
            Some("10.8.0.2".parse().unwrap())
        );

        packet[12..16].copy_from_slice(&[10, 8, 0, 99]);
        assert_eq!(
            authenticated_inbound_source(&packet, &peer, &virtual_ips, None),
            None
        );
        assert_eq!(
            authenticated_inbound_source(&packet[..19], &peer, &virtual_ips, None),
            None
        );
    }

    #[test]
    fn only_configured_exit_peer_may_return_public_sources_to_local_virtual_ips() {
        let local_ips =
            VirtualIps::dual_stack("10.8.0.2".parse().unwrap(), "fd00::1002".parse().unwrap());
        let exit_peer = PeerInfo::new_with_ips(
            "relay".to_string(),
            VirtualIps::dual_stack("10.8.0.1".parse().unwrap(), "fd00::1001".parse().unwrap()),
            vec![0; 32],
            Vec::new(),
            NatType::FullCone,
        );
        let exit_peer_ip = exit_peer.virtual_ip;
        let exit_peer = crate::p2p::connection::PeerRouteInfo::from(&exit_peer);
        let mut packet = vec![0u8; 20];
        packet[0] = 0x45;
        packet[12..16].copy_from_slice(&[149, 154, 175, 55]);
        packet[16..20].copy_from_slice(&[10, 8, 0, 2]);

        assert_eq!(
            authenticated_inbound_source(&packet, &exit_peer, &local_ips, Some(exit_peer_ip)),
            Some("149.154.175.55".parse().unwrap())
        );
        assert_eq!(
            authenticated_inbound_source(&packet, &exit_peer, &local_ips, None),
            None,
            "ordinary Mesh traffic must retain virtual-source binding"
        );

        packet[16..20].copy_from_slice(&[10, 8, 0, 3]);
        assert_eq!(
            authenticated_inbound_source(&packet, &exit_peer, &local_ips, Some(exit_peer_ip)),
            None,
            "an Exit return must be destined to this client"
        );

        packet[16..20].copy_from_slice(&[10, 8, 0, 2]);
        packet[12..16].copy_from_slice(&[192, 168, 1, 1]);
        assert_eq!(
            authenticated_inbound_source(&packet, &exit_peer, &local_ips, Some(exit_peer_ip)),
            None,
            "private return sources are outside the public Exit contract"
        );
    }

    #[test]
    fn tun_packet_exposes_only_valid_bytes_and_recycles_in_place() {
        let mut packet = TunPacket::empty(vec![0xa5; TUN_BUFFER_ALLOCATION_SIZE]);
        let allocation = packet.buffer.as_ptr();

        for len in [64, 1420, 64] {
            packet.len = len;
            packet.buffer[DATA_HEADER_LEN..DATA_HEADER_LEN + len].fill(0x5a);
            assert_eq!(packet.payload().len(), len);
            assert!(packet.payload().iter().all(|byte| *byte == 0x5a));

            packet = packet.recycle();
            assert_eq!(packet.len, 0);
            assert!(packet.payload().is_empty());
            assert_eq!(packet.buffer.len(), TUN_BUFFER_ALLOCATION_SIZE);
            assert_eq!(packet.buffer.as_ptr(), allocation);
        }

        assert_eq!(
            packet.buffer[DATA_HEADER_LEN], 0x5a,
            "recycling must not clear payload"
        );
        assert_eq!(
            packet.buffer[DATA_HEADER_LEN + 1420],
            0xa5,
            "recycling must not resize or zero-fill the fixed buffer"
        );
    }

    #[test]
    fn pooled_outbound_packet_recycles_the_original_allocation() {
        let mut packet = TunPacket::empty(vec![0xa5; TUN_BUFFER_ALLOCATION_SIZE]);
        packet.len = 1420;
        let allocation = packet.buffer.as_ptr();
        let payload_allocation = unsafe { allocation.add(DATA_HEADER_LEN) };
        let (recycle_tx, mut recycle_rx) = mpsc::unbounded_channel();

        let outbound = OutboundPacket::pooled(
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            "fd00::2".parse().expect("valid destination"),
            packet,
            recycle_tx,
        );
        assert_eq!(outbound.payload().len(), 1420);
        assert_eq!(
            outbound.payload().as_ptr(),
            payload_allocation,
            "the outbound queue must borrow the pooled TUN payload allocation without copying"
        );

        drop(outbound);
        let recycled = recycle_rx
            .try_recv()
            .expect("dropping an outbound packet must return its TUN buffer");
        assert_eq!(recycled.buffer.as_ptr(), allocation);
        assert_eq!(recycled.len, 0);
        assert_eq!(recycled.buffer.len(), TUN_BUFFER_ALLOCATION_SIZE);
    }

    #[test]
    fn tun_forwarding_skips_non_peer_ipv6_destinations() -> anyhow::Result<()> {
        assert!(should_forward_tun_destination("fd00::1001".parse()?));
        assert!(!should_forward_tun_destination("ff02::2".parse()?));
        assert!(!should_forward_tun_destination("::1".parse()?));
        assert!(!should_forward_tun_destination("::".parse()?));
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parses_websocket_coordinator_endpoints() -> anyhow::Result<()> {
        assert_eq!(
            websocket_host_port("ws://relay.xlei.cc/connect")?,
            ("relay.xlei.cc".to_string(), 80)
        );
        assert_eq!(
            websocket_host_port("wss://relay.xlei.cc:6666/connect")?,
            ("relay.xlei.cc".to_string(), 6666)
        );
        assert_eq!(
            websocket_host_port("ws://[2001:db8::1]:7777/ws")?,
            ("2001:db8::1".to_string(), 7777)
        );
        assert!(websocket_host_port("http://relay.xlei.cc").is_err());
        assert!(websocket_host_port("ws://2001:db8::1").is_err());
        Ok(())
    }

    #[test]
    fn registration_assignment_change_is_rejected() -> anyhow::Result<()> {
        let current = VirtualIps::dual_stack("10.8.0.4".parse()?, "fd00::1004".parse()?);
        validate_registered_assignment(&current, Some(&current), current.primary_ipv6())?;

        let changed = VirtualIps::dual_stack("10.8.0.2".parse()?, "fd00::1002".parse()?);
        let error =
            validate_registered_assignment(&current, Some(&changed), changed.primary_ipv6())
                .expect_err("changed assignment must fail closed");
        assert!(error.to_string().contains("虚拟地址已变化"));
        Ok(())
    }

    #[tokio::test]
    async fn authenticated_inbound_stats_use_peer_identity_not_ipv4_primary() -> anyhow::Result<()>
    {
        let virtual_ip: Ipv6Addr = "fd00::1002".parse()?;
        let peer = PeerInfo::new_with_ips(
            "peer-b".to_string(),
            VirtualIps::dual_stack("10.8.0.2".parse()?, virtual_ip),
            vec![1, 2, 3],
            Vec::new(),
            NatType::Open,
        );
        assert_eq!(peer.virtual_ips.primary_ip(), Some("10.8.0.2".parse()?));

        let route = PeerRouteInfo::from(&peer);
        let stats = StatsCollector::new();
        record_p2p_rx_for_peer(&stats, &route, 1420).await;

        let snapshot = stats
            .get_peer_stat(virtual_ip)
            .await
            .expect("canonical overlay peer statistics must exist");
        assert_eq!(snapshot.rx_packets, 1);
        assert_eq!(snapshot.rx_bytes, 1420);
        assert!(
            stats
                .get_peer_stat("10.8.0.2".parse::<IpAddr>()?)
                .await
                .is_none(),
            "authenticated inbound traffic must not be attributed to the IPv4 primary"
        );
        assert_eq!(stats.get_global_stats().await.p2p_rx_packets, 1);
        Ok(())
    }

    #[test]
    fn parses_macos_netstat_interface_stats() {
        let output = "\
Name  Mtu   Network     Address            Ipkts Ierrs    Ibytes Opkts Oerrs    Obytes  Coll
utun4 1380  <Link#21>                       10     0      2048     8     0      1024     0
";

        assert_eq!(
            parse_macos_netstat_stats(output, "fuckipv6"),
            Some(InterfaceStats {
                rx_packets: 10,
                rx_bytes: 2048,
                tx_packets: 8,
                tx_bytes: 1024,
            })
        );
    }
}
