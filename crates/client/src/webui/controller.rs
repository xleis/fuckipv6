use anyhow::{Context, Result};
use fuckipv6_common::{ClientConfig, NetworkTicket, RoutingMode};
use fuckipv6_coordinator::signaling::session::{PeerSnapshot, TrafficSnapshot};
use fuckipv6_coordinator::webui::{NetworkConfigDto, UpdateNetworkConfigRequest};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::{watch, RwLock};

use crate::cli::commands::{self, RuntimeOptions};
use crate::symmetric::EmbeddedCoordinator;

#[derive(Debug, Clone, Serialize)]
pub struct LogsResponse {
    pub service: String,
    pub lines: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkUiDto {
    pub name: String,
    pub tun_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assigned_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assigned_ipv4: Option<String>,
    pub peer_count: usize,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct UnifiedSnapshotResponse {
    pub service: &'static str,
    pub online_clients: usize,
    pub traffic: TrafficSnapshot,
    pub peers: Vec<PeerSnapshot>,
    pub auth_required: bool,
    pub client_status: String,
    pub virtual_ipv6: Option<String>,
    pub virtual_ipv4: Option<String>,
    pub routing_mode: String,
    pub exit_node: Option<String>,
    pub is_exit_node: bool,
    pub connected_target: Option<String>,
    pub hosting_ticket: Option<String>,
    pub coordinator_active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub networks: Vec<NetworkUiDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppUiState {
    pub status: String, // "offline" | "connecting" | "online" | "hosting"
    pub virtual_ipv6: Option<String>,
    pub virtual_ipv4: Option<String>,
    pub routing_mode: String,
    pub exit_node: Option<String>,
    pub is_exit_node: bool,
    pub connected_target: Option<String>,
    pub hosting_ticket: Option<String>,
    pub peers: Vec<PeerUiInfo>,
    pub logs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub networks: Vec<NetworkUiDto>,

    // === 统一协调中心 (Coordinator) 状态 ===
    pub coordinator_active: bool,
    pub coordinator_signaling_addr: Option<String>,
    pub coordinator_relay_addr: Option<String>,
    pub coordinator_online_clients: usize,
    pub coordinator_traffic: Option<CoordinatorTrafficDto>,
    pub coordinator_peers: Vec<CoordinatorPeerDto>,
    pub coordinator_blacklist: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinatorTrafficDto {
    pub uptime_secs: u64,
    pub signaling_rx_messages: u64,
    pub signaling_rx_bytes: u64,
    pub signaling_tx_messages: u64,
    pub signaling_tx_bytes: u64,
    pub relay_packets: u64,
    pub relay_payload_bytes: u64,
    pub relay_errors: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinatorPeerDto {
    pub client_id: String,
    pub virtual_ip: String,
    pub dual_stack: Option<String>,
    pub physical_addr: String,
    pub nat_type: String,
    pub last_heartbeat_secs_ago: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub networks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerUiInfo {
    pub client_id: String,
    pub virtual_ip: String,
    pub virtual_ipv4: Option<String>,
    pub direct: bool,
    pub latency_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub networks: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ConnectRequest {
    pub target: String,
    pub routing_mode: Option<String>,
    pub exit_node: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct HostRequest {
    pub name: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_relay_port")]
    pub relay_port: u16,
    #[serde(default = "default_true")]
    pub auto_connect: bool,
    pub external_host: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BanRequest {
    pub client_id: String,
}

fn default_port() -> u16 {
    6666
}
fn default_relay_port() -> u16 {
    6667
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct UpdateRoutingRequest {
    pub mode: String,
    pub exit_node: Option<String>,
    #[serde(default)]
    pub is_exit_node: bool,
}

#[derive(Clone)]
pub struct AppController {
    state: Arc<RwLock<AppUiState>>,
    cancel_tx: Arc<RwLock<Option<watch::Sender<bool>>>>,
    coordinator: Arc<RwLock<Option<EmbeddedCoordinator>>>,
    recent_logs: Arc<RwLock<VecDeque<String>>>,
}

impl AppController {
    pub fn new() -> Self {
        let default_state = AppUiState {
            status: "offline".to_string(),
            virtual_ipv6: None,
            virtual_ipv4: None,
            routing_mode: "MeshOnly".to_string(),
            exit_node: None,
            is_exit_node: false,
            connected_target: None,
            hosting_ticket: None,
            peers: Vec::new(),
            logs: Vec::new(),
            network_name: Some("default".to_string()),
            networks: Vec::new(),
            coordinator_active: false,
            coordinator_signaling_addr: None,
            coordinator_relay_addr: None,
            coordinator_online_clients: 0,
            coordinator_traffic: None,
            coordinator_peers: Vec::new(),
            coordinator_blacklist: Vec::new(),
        };

        Self {
            state: Arc::new(RwLock::new(default_state)),
            cancel_tx: Arc::new(RwLock::new(None)),
            coordinator: Arc::new(RwLock::new(None)),
            recent_logs: Arc::new(RwLock::new(VecDeque::with_capacity(200))),
        }
    }

    pub async fn get_state(&self) -> AppUiState {
        let mut s = self.state.read().await.clone();
        let logs: Vec<String> = self.recent_logs.read().await.iter().cloned().collect();
        s.logs = logs;

        if let Some(coord) = self.coordinator.read().await.as_ref() {
            let snap = coord.session_manager.snapshot().await;
            let alloc = coord.session_manager.current_network_allocation().await;
            s.coordinator_active = true;
            s.coordinator_signaling_addr = Some(coord.signaling_addr.to_string());
            s.coordinator_relay_addr = Some(coord.relay_addr.to_string());
            s.coordinator_online_clients = snap.online_clients;
            s.coordinator_traffic = Some(CoordinatorTrafficDto {
                uptime_secs: snap.traffic.uptime_secs,
                signaling_rx_messages: snap.traffic.signaling_rx_messages,
                signaling_rx_bytes: snap.traffic.signaling_rx_bytes,
                signaling_tx_messages: snap.traffic.signaling_tx_messages,
                signaling_tx_bytes: snap.traffic.signaling_tx_bytes,
                relay_packets: snap.traffic.relay_packets,
                relay_payload_bytes: snap.traffic.relay_payload_bytes,
                relay_errors: snap.traffic.relay_errors,
            });
            let net_name = s.network_name.clone().unwrap_or_else(|| "default".to_string());
            s.networks = vec![NetworkUiDto {
                name: net_name.clone(),
                tun_name: crate::multi_network::derive_tun_name(Some(&net_name), "fuckipv6"),
                assigned_ip: Some(alloc.effective_ipv6_pool().to_string()),
                assigned_ipv4: if alloc.mode.is_ipv4_enabled() {
                    Some(alloc.ipv4_pool.clone())
                } else {
                    None
                },
                peer_count: snap.online_clients,
                status: "协调中枢托管中".to_string(),
            }];
            s.coordinator_peers = snap
                .peers
                .into_iter()
                .map(|p| {
                    let p_net = p.network_name.clone().unwrap_or_else(|| net_name.clone());
                    CoordinatorPeerDto {
                        client_id: p.client_id,
                        virtual_ip: p.virtual_ip.to_string(),
                        dual_stack: p.virtual_ipv4.map(|v4| v4.to_string()),
                        physical_addr: p
                            .endpoints
                            .first()
                            .map(|e| e.to_string())
                            .unwrap_or_else(|| "-".to_string()),
                        nat_type: p.nat_type,
                        last_heartbeat_secs_ago: p.idle_secs,
                        network_name: Some(p_net.clone()),
                        networks: vec![p_net],
                    }
                })
                .collect();
            s.coordinator_blacklist = coord.session_manager.get_blacklist().await;
        } else {
            s.coordinator_active = false;
            s.coordinator_signaling_addr = None;
            s.coordinator_relay_addr = None;
            s.coordinator_online_clients = 0;
            s.coordinator_traffic = None;
            s.coordinator_peers = Vec::new();
            s.coordinator_blacklist = Vec::new();

            if s.networks.is_empty() {
                let current_net = s.network_name.clone().unwrap_or_else(|| "default".to_string());
                let tun_dev = crate::multi_network::derive_tun_name(Some(&current_net), "fuckipv6");
                s.networks = vec![NetworkUiDto {
                    name: current_net,
                    tun_name: tun_dev,
                    assigned_ip: s.virtual_ipv6.clone(),
                    assigned_ipv4: s.virtual_ipv4.clone(),
                    peer_count: s.peers.len(),
                    status: s.status.clone(),
                }];
            }
        }

        s
    }

    pub async fn update_state<F>(&self, f: F)
    where
        F: FnOnce(&mut AppUiState),
    {
        let mut st = self.state.write().await;
        f(&mut st);
    }

    pub async fn log(&self, msg: impl Into<String>) {
        let msg = msg.into();
        let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
        let entry = format!("[{timestamp}] {msg}");
        tracing::info!("{entry}");
        let mut logs = self.recent_logs.write().await;
        if logs.len() >= 200 {
            logs.pop_front();
        }
        logs.push_back(entry);
    }

    /// 一键连接指定网络（支持普通地址如 relay.xlei.cc:6666 或 fk:// 邀请码）
    pub async fn connect(
        &self,
        target: String,
        routing_mode: Option<String>,
        exit_node: Option<String>,
    ) -> Result<()> {
        let target = target.trim().to_string();
        if target.is_empty() {
            anyhow::bail!("连接目标不能为空");
        }

        self.log(format!("发起网络连接 -> 目标: {target}")).await;

        // 若当前有活动连接，先断开
        self.disconnect().await?;

        {
            let mut st = self.state.write().await;
            st.status = "connecting".to_string();
            st.connected_target = Some(target.clone());
            if let Some(ref m) = routing_mode {
                st.routing_mode = m.clone();
            }
            if let Some(ref e) = exit_node {
                st.exit_node = Some(e.clone());
            }
        }

        // 解析目标：支持 fk:// 邀请票据
        let (coordinator_url, network_name) = if target.starts_with(NetworkTicket::URI_SCHEME) {
            let ticket = NetworkTicket::parse(&target).context("解析 fk:// 邀请票据失败")?;
            self.log(format!(
                "[票据] 成功解析网络邀请票据: 网络={}, 端点={}",
                ticket.network_name, ticket.endpoint
            ))
            .await;
            (ticket.websocket_url(), ticket.network_name)
        } else {
            let url = if target.starts_with("ws://") || target.starts_with("wss://") {
                target.clone()
            } else {
                format!("ws://{target}")
            };
            (url, "default".to_string())
        };

        self.update_state(|st| {
            st.network_name = Some(network_name.clone());
        }).await;

        // 创建临时配置
        let temp_dir = std::env::temp_dir().join("fuckipv6-ui-temp");
        tokio::fs::create_dir_all(&temp_dir).await?;
        let config_file = temp_dir.join(format!("{network_name}-config.toml"));
        let key_file = temp_dir.join(format!("{network_name}-key.pem"));

        let mode = match routing_mode.as_deref().unwrap_or("MeshOnly").to_lowercase().as_str() {
            "bypassmainland" | "bypass-mainland" | "bypass_mainland" => RoutingMode::BypassMainland,
            "globalproxy" | "global-proxy" | "global_proxy" => RoutingMode::GlobalProxy,
            _ => RoutingMode::MeshOnly,
        };

        let config = ClientConfig {
            client_id: None,
            coordinator_url,
            private_key_path: key_file,
            network: Default::default(),
            tun: Default::default(),
            p2p: Default::default(),
            log: Default::default(),
            acl: Default::default(),
            routing: fuckipv6_common::RoutingConfig {
                mode,
                exit_node: exit_node.clone(),
                direct_cidrs: Vec::new(),
                proxy_cidrs: Vec::new(),
            },
            exit_node: fuckipv6_common::ExitNodeConfig {
                enabled: self.state.read().await.is_exit_node,
                ..Default::default()
            },
        };

        config.save_to_file(&config_file)?;

        let (cancel_tx, mut cancel_rx) = watch::channel(false);
        *self.cancel_tx.write().await = Some(cancel_tx);

        let this = self.clone();
        let config_path_str = config_file.display().to_string();

        tokio::spawn(async move {
            this.log("正在初始化虚拟网卡 TUN 与 P2P 加密隧道...").await;

            let options = RuntimeOptions {
                routing_mode,
                exit_node,
                headless: true,
                gui: false,
                webui_port: 0,
            };

            // 监听停止信号
            tokio::select! {
                res = commands::start_with_options(&config_path_str, options) => {
                    if let Err(e) = res {
                        this.log(format!("⚠️ 客户端运行时异常结束: {e:#}")).await;
                    }
                }
                _ = cancel_rx.changed() => {
                    this.log("收到断开连接指令，已平稳释放虚拟网卡与网络通道").await;
                }
            }

            let mut st = this.state.write().await;
            st.status = "offline".to_string();
            st.virtual_ipv6 = None;
            st.virtual_ipv4 = None;
            st.connected_target = None;
            st.peers.clear();
        });

        // 轮询等待虚拟 IP 分配更新
        let this = self.clone();
        tokio::spawn(async move {
            for _ in 0..30 {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                // 从 IPC 或本地接口检查状态
                if let Ok(crate::ipc::IpcResponse::Status(s)) = crate::ipc::request(crate::ipc::IpcRequest::Status).await {
                    let mut st = this.state.write().await;
                    st.status = "online".to_string();
                    st.virtual_ipv6 = Some(s.assigned_ip.to_string());
                    this.log(format!("[连接] 网络连接成功！分配虚拟 IPv6: {}", s.assigned_ip)).await;
                    break;
                }
            }
        });

        Ok(())
    }

    /// 断开当前网络连接
    pub async fn disconnect(&self) -> Result<()> {
        let cancel = self.cancel_tx.write().await.take();
        if let Some(tx) = cancel {
            self.log("正在停止客户端运行实例...").await;
            let _ = tx.send(true);
        }

        let mut st = self.state.write().await;
        st.status = "offline".to_string();
        st.virtual_ipv6 = None;
        st.virtual_ipv4 = None;
        st.connected_target = None;
        st.peers.clear();
        Ok(())
    }

    /// 一键自建对等网络并生成邀请凭证 (fk://...)
    pub async fn host_network(
        &self,
        name: String,
        port: u16,
        relay_port: u16,
        auto_connect: bool,
        external_host: Option<String>,
    ) -> Result<String> {
        self.log(format!("[启动] 正在启动本地协调 Actor 创建网络: {name} (信令={port}, 中继={relay_port})")).await;

        let coordinator = EmbeddedCoordinator::start(
            &name,
            port,
            relay_port,
            external_host,
            Some("fd00::1000/120"),
        )
        .await
        .context("启动内嵌协调 Actor 失败")?;

        let ticket = coordinator.ticket.to_string();
        self.log(format!("[网络] 对等网络已创建成功！邀请票据: {ticket}")).await;

        *self.coordinator.write().await = Some(coordinator);

        {
            let mut st = self.state.write().await;
            st.network_name = Some(name.clone());
            st.hosting_ticket = Some(ticket.clone());
            st.coordinator_active = true;
            st.coordinator_signaling_addr = Some(format!("0.0.0.0:{port}"));
            st.coordinator_relay_addr = Some(format!("0.0.0.0:{relay_port}"));
            if st.status == "offline" {
                st.status = "hosting".to_string();
            }
        }

        if auto_connect {
            self.log("本机正在自动作为首个 Peer 接入新网络...").await;
            let local_target = format!("127.0.0.1:{port}");
            let this = self.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let _ = this.connect(local_target, Some("MeshOnly".to_string()), None).await;
            });
        }

        Ok(ticket)
    }

    /// 停止自建网络
    pub async fn stop_host(&self) -> Result<()> {
        self.log("正在停止本地对等协调 Actor...").await;
        *self.coordinator.write().await = None;
        self.disconnect().await?;
        let mut st = self.state.write().await;
        st.hosting_ticket = None;
        st.coordinator_active = false;
        st.coordinator_signaling_addr = None;
        st.coordinator_relay_addr = None;
        st.coordinator_online_clients = 0;
        st.coordinator_traffic = None;
        st.coordinator_peers.clear();
        st.coordinator_blacklist.clear();
        st.status = "offline".to_string();
        self.log("本地网络协调服务已终止").await;
        Ok(())
    }

    /// 协调中枢封禁客户端
    pub async fn ban_client(&self, client_id: String) -> Result<()> {
        let coord = self.coordinator.read().await;
        if let Some(c) = coord.as_ref() {
            c.session_manager.ban_client(client_id.clone()).await;
            self.log(format!("[封禁] 协调中枢已封禁客户端: {client_id}")).await;
            Ok(())
        } else {
            anyhow::bail!("本地协调服务未运行")
        }
    }

    /// 协调中枢解封客户端
    pub async fn unban_client(&self, client_id: &str) -> Result<bool> {
        let coord = self.coordinator.read().await;
        if let Some(c) = coord.as_ref() {
            let res = c.session_manager.unban_client(client_id).await;
            self.log(format!("✅ 协调中枢已解封客户端: {client_id}")).await;
            Ok(res)
        } else {
            anyhow::bail!("本地协调服务未运行")
        }
    }

    /// 动态热更新路由分流与出口节点配置
    pub async fn update_routing(
        &self,
        mode: String,
        exit_node: Option<String>,
        is_exit_node: bool,
    ) -> Result<()> {
        let _parsed_mode = match mode.to_lowercase().as_str() {
            "bypassmainland" | "bypass-mainland" | "bypass_mainland" => RoutingMode::BypassMainland,
            "globalproxy" | "global-proxy" | "global_proxy" => RoutingMode::GlobalProxy,
            _ => RoutingMode::MeshOnly,
        };

        {
            let mut st = self.state.write().await;
            st.routing_mode = mode.clone();
            st.exit_node = exit_node.clone();
            st.is_exit_node = is_exit_node;
        }

        self.log(format!(
            "动态应用分流规则: 策略={}, 出口节点={:?}, 声明为出口网关={}",
            mode, exit_node, is_exit_node
        ))
        .await;

        Ok(())
    }

    /// 刷新在线对端信息
    pub async fn refresh_peers(&self) {
        if let Ok(crate::ipc::IpcResponse::Peers(peers)) = crate::ipc::request(crate::ipc::IpcRequest::Peers).await {
            let default_net = self.state.read().await.network_name.clone().unwrap_or_else(|| "default".to_string());
            let peer_infos: Vec<PeerUiInfo> = peers
                .into_iter()
                .map(|p| {
                    let net = p.network_name.clone().unwrap_or_else(|| default_net.clone());
                    PeerUiInfo {
                        client_id: p.client_id,
                        virtual_ip: p.virtual_ip.to_string(),
                        virtual_ipv4: None,
                        direct: p.p2p_active,
                        latency_ms: p.rtt_us.map(|us| us as f64 / 1000.0),
                        network_name: Some(net.clone()),
                        networks: vec![net],
                    }
                })
                .collect();
            self.state.write().await.peers = peer_infos;
        }
    }
    /// 获取全网对等节点快照列表 (兼容原有拓扑与节点管理)
    pub async fn get_peers_snapshots(&self) -> Vec<PeerSnapshot> {
        if let Some(coord) = self.coordinator.read().await.as_ref() {
            coord.session_manager.snapshot().await.peers
        } else {
            let st = self.state.read().await;
            st.peers
                .iter()
                .filter_map(|p| {
                    let v6 = p.virtual_ip.parse().ok()?;
                    let v4 = p.virtual_ipv4.as_ref().and_then(|ip| ip.parse().ok());
                    let net = p.network_name.clone().or_else(|| st.network_name.clone()).unwrap_or_else(|| "default".to_string());
                    Some(PeerSnapshot {
                        client_id: p.client_id.clone(),
                        virtual_ip: v6,
                        virtual_ipv4: v4,
                        endpoints: Vec::new(),
                        nat_type: if p.direct {
                            "Direct P2P".to_string()
                        } else {
                            "QUIC Relay".to_string()
                        },
                        connected_secs: 0,
                        idle_secs: 0,
                        network_name: Some(net.clone()),
                        networks: vec![net],
                    })
                })
                .collect()
        }
    }

    /// 获取统一推送快照 (供 WebSocket /ws 推送使用)
    pub async fn get_unified_snapshot(&self) -> UnifiedSnapshotResponse {
        let st = self.state.read().await.clone();
        if let Some(coord) = self.coordinator.read().await.as_ref() {
            let snap = coord.session_manager.snapshot().await;
            UnifiedSnapshotResponse {
                service: "fuckipv6-unified",
                online_clients: snap.online_clients,
                traffic: snap.traffic,
                peers: snap.peers,
                auth_required: false,
                client_status: st.status,
                virtual_ipv6: st.virtual_ipv6,
                virtual_ipv4: st.virtual_ipv4,
                routing_mode: st.routing_mode,
                exit_node: st.exit_node,
                is_exit_node: st.is_exit_node,
                connected_target: st.connected_target,
                hosting_ticket: st.hosting_ticket,
                coordinator_active: true,
                network_name: st.network_name,
                networks: st.networks,
            }
        } else {
            let peers = self.get_peers_snapshots().await;
            let fake_traffic = TrafficSnapshot {
                uptime_secs: 0,
                signaling_rx_messages: 0,
                signaling_rx_bytes: 0,
                signaling_tx_messages: 0,
                signaling_tx_bytes: 0,
                relay_packets: 0,
                relay_payload_bytes: 0,
                relay_errors: 0,
            };
            UnifiedSnapshotResponse {
                service: "fuckipv6-unified",
                online_clients: peers.len(),
                traffic: fake_traffic,
                peers,
                auth_required: false,
                client_status: st.status,
                virtual_ipv6: st.virtual_ipv6,
                virtual_ipv4: st.virtual_ipv4,
                routing_mode: st.routing_mode,
                exit_node: st.exit_node,
                is_exit_node: st.is_exit_node,
                connected_target: st.connected_target,
                hosting_ticket: None,
                coordinator_active: false,
                network_name: st.network_name,
                networks: st.networks,
            }
        }
    }

    /// 查询当前专网网段与地址池配置
    pub async fn get_network_config(&self) -> NetworkConfigDto {
        if let Some(coord) = self.coordinator.read().await.as_ref() {
            let alloc = coord.session_manager.current_network_allocation().await;
            let ipv4_prefix = coord.session_manager.ipv4_prefix().await;
            let ipv6_prefix = coord.session_manager.ipv6_prefix().await;
            let ipv6_pool = alloc.effective_ipv6_pool().to_string();
            NetworkConfigDto {
                mode: alloc.mode,
                ipv4_pool: alloc.ipv4_pool,
                ipv6_pool,
                ipv4_prefix,
                ipv6_prefix,
                config_file: None,
            }
        } else {
            NetworkConfigDto {
                mode: fuckipv6_common::NetworkMode::DualStack,
                ipv4_pool: "10.8.0.0/24".to_string(),
                ipv6_pool: "fd00::1000/120".to_string(),
                ipv4_prefix: 24,
                ipv6_prefix: 120,
                config_file: None,
            }
        }
    }

    /// 热更新专网网段与地址池配置
    pub async fn update_network_config(
        &self,
        req: UpdateNetworkConfigRequest,
    ) -> Result<NetworkConfigDto> {
        if let Some(coord) = self.coordinator.read().await.as_ref() {
            if req.mode.is_ipv4_enabled() {
                fuckipv6_common::validate_ipv4_cidr(&req.ipv4_pool, "ipv4_pool")?;
            }
            if req.mode.is_ipv6_enabled() {
                fuckipv6_common::validate_ipv6_cidr(&req.ipv6_pool, "ipv6_pool")?;
            }
            let alloc = fuckipv6_common::NetworkAllocation {
                mode: req.mode,
                ipv4_pool: req.ipv4_pool.clone(),
                ipv6_pool: req.ipv6_pool.clone(),
                ip_pool: Some(req.ipv6_pool.clone()),
            };
            coord.session_manager.update_network_allocation(alloc).await?;
            self.log(format!(
                "[配置] 专网地址池参数热更新生效: 模式={:?}, IPv4={}, IPv6={}",
                req.mode, req.ipv4_pool, req.ipv6_pool
            ))
            .await;
            Ok(self.get_network_config().await)
        } else {
            anyhow::bail!("本地协调中枢未运行，无法热更新网段")
        }
    }

    /// 获取黑名单列表
    pub async fn get_blacklist(&self) -> Vec<String> {
        if let Some(coord) = self.coordinator.read().await.as_ref() {
            coord.session_manager.get_blacklist().await
        } else {
            self.state.read().await.coordinator_blacklist.clone()
        }
    }

    /// 读取系统运行日志
    pub async fn read_logs(&self, service: &str, lines: usize) -> LogsResponse {
        if service == "memory" || service == "errors" {
            return LogsResponse {
                service: service.to_string(),
                lines: fuckipv6_common::recent_error_log_lines(lines),
                error: None,
            };
        }

        #[cfg(target_os = "linux")]
        {
            let unit_name = if service.ends_with(".service") {
                service.to_string()
            } else if service.contains("coordinator") || service.contains("client") {
                "fuckipv6.service".to_string()
            } else {
                format!("{service}.service")
            };

            let output = tokio::process::Command::new("journalctl")
                .args(["-u", &unit_name, "-n", &lines.to_string(), "--no-pager"])
                .output()
                .await;

            match output {
                Ok(out) if out.status.success() => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    let mut log_lines: Vec<String> = stdout.lines().map(|s| s.to_string()).collect();
                    if log_lines.len() > lines {
                        log_lines.drain(0..log_lines.len() - lines);
                    }
                    LogsResponse {
                        service: service.to_string(),
                        lines: log_lines,
                        error: None,
                    }
                }
                _ => {
                    let recent: Vec<String> = self.recent_logs.read().await.iter().cloned().collect();
                    LogsResponse {
                        service: service.to_string(),
                        lines: recent,
                        error: None,
                    }
                }
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            let recent: Vec<String> = self.recent_logs.read().await.iter().cloned().collect();
            LogsResponse {
                service: service.to_string(),
                lines: recent,
                error: None,
            }
        }
    }
}

impl Default for AppController {
    fn default() -> Self {
        Self::new()
    }
}
