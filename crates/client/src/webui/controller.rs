use anyhow::{Context, Result};
use fuckipv6_common::{ClientConfig, NetworkTicket, RoutingMode};
use fuckipv6_coordinator::signaling::session::{PeerSnapshot, TrafficSnapshot};
use fuckipv6_coordinator::webui::{NetworkConfigDto, UpdateNetworkConfigRequest};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{watch, RwLock};

use crate::cli::commands::{self, RuntimeOptions};
use crate::ipc::{GlobalStatsSnapshot, NetworkSessionSnapshot, StatusSnapshot};
use crate::p2p::connection::PeerProbeOutcome;
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traffic: Option<TrafficSnapshot>,
    pub runtime_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_p2p_addr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_traffic: Option<RuntimeTrafficDto>,
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

    // === 本机客户端运行时状态 (来自 IPC) ===
    #[serde(default)]
    pub runtime_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_p2p_addr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_traffic: Option<RuntimeTrafficDto>,
}

impl AppUiState {
    fn offline() -> Self {
        Self {
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
            runtime_available: false,
            runtime_p2p_addr: None,
            runtime_traffic: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeTrafficDto {
    pub uptime_secs: u64,
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
        Self {
            state: Arc::new(RwLock::new(AppUiState::offline())),
            cancel_tx: Arc::new(RwLock::new(None)),
            coordinator: Arc::new(RwLock::new(None)),
            recent_logs: Arc::new(RwLock::new(VecDeque::with_capacity(200))),
        }
    }

    pub async fn get_state(&self) -> AppUiState {
        let mut s = self.state.read().await.clone();
        let logs: Vec<String> = self.recent_logs.read().await.iter().cloned().collect();
        s.logs = logs;

        match self.runtime_status().await {
            Some(status) => apply_runtime_status(&mut s, &status),
            None => clear_runtime_status(&mut s),
        }

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
            let net_name = s
                .network_name
                .clone()
                .unwrap_or_else(|| "default".to_string());
            if !s.runtime_available {
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
            }
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
        }

        if !s.runtime_available && s.status == "online" {
            s.status = if s.coordinator_active {
                "hosting".to_string()
            } else {
                "offline".to_string()
            };
            s.virtual_ipv6 = None;
            s.virtual_ipv4 = None;
            s.connected_target = None;
            s.networks.clear();
        }

        if s.networks.is_empty() {
            let current_net = s
                .network_name
                .clone()
                .unwrap_or_else(|| "default".to_string());
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

        s
    }

    async fn runtime_status(&self) -> Option<StatusSnapshot> {
        match tokio::time::timeout(
            Duration::from_millis(500),
            crate::ipc::request(crate::ipc::IpcRequest::Status),
        )
        .await
        {
            Ok(Ok(crate::ipc::IpcResponse::Status(status))) => Some(status),
            _ => None,
        }
    }

    async fn runtime_peers(&self) -> Option<Vec<crate::ipc::PeerSnapshot>> {
        match tokio::time::timeout(
            Duration::from_millis(500),
            crate::ipc::request(crate::ipc::IpcRequest::Peers),
        )
        .await
        {
            Ok(Ok(crate::ipc::IpcResponse::Peers(peers))) => Some(peers),
            _ => None,
        }
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
        })
        .await;

        // 创建临时配置
        let temp_dir = std::env::temp_dir().join("fuckipv6-ui-temp");
        tokio::fs::create_dir_all(&temp_dir).await?;
        let config_file = temp_dir.join(format!("{network_name}-config.toml"));
        let key_file = temp_dir.join(format!("{network_name}-key.pem"));

        let mode = match routing_mode
            .as_deref()
            .unwrap_or("MeshOnly")
            .to_lowercase()
            .as_str()
        {
            "bypassmainland" | "bypass-mainland" | "bypass_mainland" => RoutingMode::BypassMainland,
            "globalproxy" | "global-proxy" | "global_proxy" => RoutingMode::GlobalProxy,
            _ => RoutingMode::MeshOnly,
        };

        let is_exit_node = self.state.read().await.is_exit_node;
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
                enabled: is_exit_node,
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
                is_exit_node: Some(is_exit_node),
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
                if let Ok(crate::ipc::IpcResponse::Status(s)) =
                    crate::ipc::request(crate::ipc::IpcRequest::Status).await
                {
                    let mut st = this.state.write().await;
                    st.status = "online".to_string();
                    st.virtual_ipv6 = Some(s.assigned_ip.to_string());
                    this.log(format!(
                        "[连接] 网络连接成功！分配虚拟 IPv6: {}",
                        s.assigned_ip
                    ))
                    .await;
                    break;
                }
            }
        });

        Ok(())
    }

    /// 断开当前网络连接，并等待运行中的客户端确认已经释放资源。
    pub async fn disconnect(&self) -> Result<()> {
        let has_runtime = {
            let st = self.state.read().await;
            matches!(st.status.as_str(), "online" | "connecting")
        };

        if has_runtime {
            self.log("正在请求客户端运行时安全断开...").await;
            match crate::ipc::request(crate::ipc::IpcRequest::Disconnect).await? {
                crate::ipc::IpcResponse::Success { .. } => {}
                crate::ipc::IpcResponse::Error { message } => anyhow::bail!(message),
                other => anyhow::bail!("断开请求返回了意外响应: {other:?}"),
            }
        }

        *self.cancel_tx.write().await = None;
        let mut st = self.state.write().await;
        st.status = "offline".to_string();
        st.virtual_ipv6 = None;
        st.virtual_ipv4 = None;
        st.connected_target = None;
        st.peers.clear();
        Ok(())
    }

    /// 向运行中的 P2P 引擎发起真实探针。
    pub async fn probe_peer(&self, target: String, timeout_ms: u64) -> Result<PeerProbeOutcome> {
        if target.trim().is_empty() {
            anyhow::bail!("探测目标不能为空");
        }
        match crate::ipc::request(crate::ipc::IpcRequest::ProbePeer { target, timeout_ms }).await? {
            crate::ipc::IpcResponse::PeerProbe(outcome) => Ok(outcome),
            crate::ipc::IpcResponse::Error { message } => anyhow::bail!(message),
            other => anyhow::bail!("节点探测返回了意外响应: {other:?}"),
        }
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
        self.log(format!(
            "[启动] 正在启动本地协调 Actor 创建网络: {name} (信令={port}, 中继={relay_port})"
        ))
        .await;

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
        self.log(format!("[网络] 对等网络已创建成功！邀请票据: {ticket}"))
            .await;

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
                let _ = this
                    .connect(local_target, Some("MeshOnly".to_string()), None)
                    .await;
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
            self.log(format!("[封禁] 协调中枢已封禁客户端: {client_id}"))
                .await;
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
            self.log(format!("✅ 协调中枢已解封客户端: {client_id}"))
                .await;
            Ok(res)
        } else {
            anyhow::bail!("本地协调服务未运行")
        }
    }

    /// 动态热更新路由分流与出口节点配置，并等待运行时确认。
    pub async fn update_routing(
        &self,
        mode: String,
        exit_node: Option<String>,
        is_exit_node: bool,
    ) -> Result<()> {
        let result = match crate::ipc::request(crate::ipc::IpcRequest::UpdateRouting {
            mode,
            exit_node,
            is_exit_node,
        })
        .await?
        {
            crate::ipc::IpcResponse::RoutingUpdated(result) => result,
            crate::ipc::IpcResponse::Error { message } => anyhow::bail!(message),
            other => anyhow::bail!("路由更新返回了意外响应: {other:?}"),
        };

        {
            let mut st = self.state.write().await;
            st.routing_mode = result.mode.clone();
            st.exit_node = result.exit_node.clone();
            st.is_exit_node = result.is_exit_node;
        }

        self.log(format!(
            "已应用运行时分流规则: 策略={}, 出口节点={:?}, 声明为出口网关={}",
            result.mode, result.exit_node, result.is_exit_node
        ))
        .await;

        Ok(())
    }

    /// 刷新在线对端信息
    pub async fn refresh_peers(&self) {
        let mut state = self.state.write().await;
        if let Some(status) = self.runtime_status().await {
            apply_runtime_status(&mut state, &status);
        } else {
            clear_runtime_status(&mut state);
        }
    }

    /// 获取全网对等节点快照列表 (兼容原有拓扑与节点管理)
    pub async fn get_peers_snapshots(&self) -> Vec<PeerSnapshot> {
        let default_net = self
            .state
            .read()
            .await
            .network_name
            .clone()
            .unwrap_or_else(|| "default".to_string());

        if let Some(peers) = self.runtime_peers().await {
            return peers
                .iter()
                .map(|peer| ipc_peer_to_snapshot(peer, &default_net))
                .collect();
        }

        if let Some(coord) = self.coordinator.read().await.as_ref() {
            coord.session_manager.snapshot().await.peers
        } else {
            Vec::new()
        }
    }

    /// 获取统一推送快照 (供 WebSocket /ws 推送使用)
    pub async fn get_unified_snapshot(&self) -> UnifiedSnapshotResponse {
        let st = self.get_state().await;
        let peers = self.get_peers_snapshots().await;
        let traffic = st
            .coordinator_traffic
            .as_ref()
            .map(|traffic| TrafficSnapshot {
                uptime_secs: traffic.uptime_secs,
                signaling_rx_messages: traffic.signaling_rx_messages,
                signaling_rx_bytes: traffic.signaling_rx_bytes,
                signaling_tx_messages: traffic.signaling_tx_messages,
                signaling_tx_bytes: traffic.signaling_tx_bytes,
                relay_packets: traffic.relay_packets,
                relay_payload_bytes: traffic.relay_payload_bytes,
                relay_errors: traffic.relay_errors,
            });
        let online_clients = if st.runtime_available {
            st.peers.len()
        } else if st.coordinator_active {
            st.coordinator_online_clients
        } else {
            0
        };

        UnifiedSnapshotResponse {
            service: "fuckipv6-unified",
            online_clients,
            traffic,
            runtime_available: st.runtime_available,
            runtime_p2p_addr: st.runtime_p2p_addr,
            runtime_traffic: st.runtime_traffic,
            peers,
            auth_required: false,
            client_status: st.status,
            virtual_ipv6: st.virtual_ipv6,
            virtual_ipv4: st.virtual_ipv4,
            routing_mode: st.routing_mode,
            exit_node: st.exit_node,
            is_exit_node: st.is_exit_node,
            connected_target: st.connected_target,
            hosting_ticket: st.hosting_ticket,
            coordinator_active: st.coordinator_active,
            network_name: st.network_name,
            networks: st.networks,
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
            coord
                .session_manager
                .update_network_allocation(alloc)
                .await?;
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
                    let mut log_lines: Vec<String> =
                        stdout.lines().map(|s| s.to_string()).collect();
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
                    let recent: Vec<String> =
                        self.recent_logs.read().await.iter().cloned().collect();
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

fn runtime_traffic_from_status(status: &StatusSnapshot) -> RuntimeTrafficDto {
    runtime_traffic_from_global(&status.global, status.uptime_secs)
}

fn runtime_traffic_from_global(
    global: &GlobalStatsSnapshot,
    uptime_secs: u64,
) -> RuntimeTrafficDto {
    RuntimeTrafficDto {
        uptime_secs,
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
    }
}

fn network_ui_from_status(network: &NetworkSessionSnapshot) -> NetworkUiDto {
    NetworkUiDto {
        name: network.network_name.clone(),
        tun_name: network.tun_name.clone(),
        assigned_ip: (!network.assigned_ip.is_unspecified())
            .then(|| network.assigned_ip.to_string()),
        assigned_ipv4: network.assigned_ipv4.map(|ip| ip.to_string()),
        peer_count: network.peers.len(),
        status: "运行时在线".to_string(),
    }
}

fn ipc_peer_to_ui(peer: &crate::ipc::PeerSnapshot, default_network: &str) -> PeerUiInfo {
    let network = peer
        .network_name
        .clone()
        .unwrap_or_else(|| default_network.to_string());
    PeerUiInfo {
        client_id: peer.client_id.clone(),
        virtual_ip: peer.virtual_ip.to_string(),
        virtual_ipv4: peer.virtual_ipv4.map(|ip| ip.to_string()),
        direct: peer.p2p_active,
        latency_ms: peer.rtt_us.map(|us| us as f64 / 1000.0),
        network_name: Some(network.clone()),
        networks: vec![network],
    }
}

fn ipc_peer_to_snapshot(peer: &crate::ipc::PeerSnapshot, default_network: &str) -> PeerSnapshot {
    let network = peer
        .network_name
        .clone()
        .unwrap_or_else(|| default_network.to_string());
    PeerSnapshot {
        client_id: peer.client_id.clone(),
        virtual_ip: peer.virtual_ip,
        virtual_ipv4: peer.virtual_ipv4,
        endpoints: peer.endpoints.clone(),
        nat_type: peer.nat_type.clone(),
        connected_secs: peer.last_p2p_success_secs.unwrap_or(0),
        idle_secs: peer.idle_secs.unwrap_or(0),
        network_name: Some(network.clone()),
        networks: vec![network],
    }
}

fn apply_runtime_status(state: &mut AppUiState, status: &StatusSnapshot) {
    if status.networks.is_empty() && status.assigned_ip.is_unspecified() {
        clear_runtime_status(state);
        return;
    }

    state.runtime_available = true;
    state.runtime_p2p_addr = Some(status.p2p_listen_addr.to_string());
    state.runtime_traffic = Some(runtime_traffic_from_status(status));

    let default_network = status
        .network_name
        .clone()
        .or_else(|| status.networks.first().map(|n| n.network_name.clone()))
        .or_else(|| state.network_name.clone())
        .unwrap_or_else(|| "default".to_string());
    state.network_name = Some(default_network.clone());

    let primary_v4 = status
        .networks
        .iter()
        .find_map(|network| network.assigned_ipv4);
    state.virtual_ipv6 =
        (!status.assigned_ip.is_unspecified()).then(|| status.assigned_ip.to_string());
    state.virtual_ipv4 = primary_v4.map(|ip| ip.to_string());

    state.peers = status
        .peers
        .iter()
        .map(|peer| ipc_peer_to_ui(peer, &default_network))
        .collect();

    if !status.networks.is_empty() {
        state.networks = status.networks.iter().map(network_ui_from_status).collect();
        state.status = "online".to_string();
    } else if !status.assigned_ip.is_unspecified() {
        state.networks = vec![NetworkUiDto {
            name: default_network,
            tun_name: status
                .tun_name
                .clone()
                .unwrap_or_else(|| "fuckipv6".to_string()),
            assigned_ip: Some(status.assigned_ip.to_string()),
            assigned_ipv4: primary_v4.map(|ip| ip.to_string()),
            peer_count: status.peers.len(),
            status: "运行时在线".to_string(),
        }];
        state.status = "online".to_string();
    }
}

fn clear_runtime_status(state: &mut AppUiState) {
    state.runtime_available = false;
    state.runtime_p2p_addr = None;
    state.runtime_traffic = None;
    state.peers.clear();
}

impl Default for AppController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::{PeerSnapshot as IpcPeerSnapshot, StatusSnapshot};
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

    fn zero_global() -> GlobalStatsSnapshot {
        GlobalStatsSnapshot {
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
        }
    }

    fn runtime_peer() -> IpcPeerSnapshot {
        IpcPeerSnapshot {
            client_id: "peer-alpha".to_string(),
            virtual_ip: Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0x10, 2),
            virtual_ipv4: Some(Ipv4Addr::new(10, 8, 0, 2)),
            endpoints: vec!["127.0.0.1:41000".parse().unwrap()],
            nat_type: "Full Cone".to_string(),
            p2p_active: true,
            p2p_healthy: true,
            active_endpoint: Some("127.0.0.1:41000".parse().unwrap()),
            tx_packets: 12,
            tx_bytes: 1_200,
            rx_packets: 10,
            rx_bytes: 1_000,
            rtt_us: Some(23_000),
            loss_rate: 0.0,
            bandwidth_bps: 8_000_000,
            health_probes_sent: 3,
            health_probes_acked: 3,
            health_window_probes: 3,
            health_window_lost: 0,
            probe_success_rate: 1.0,
            idle_secs: Some(1),
            last_p2p_success_secs: Some(42),
            remote_rtt_us: Some(24_000),
            remote_loss_rate: Some(0.0),
            remote_bandwidth_bps: Some(7_500_000),
            remote_quality_age_secs: Some(1),
            network_name: Some("office".to_string()),
        }
    }

    fn active_status() -> StatusSnapshot {
        let mut global = zero_global();
        global.tun_rx_packets = 20;
        global.tun_rx_bytes = 2_000;
        global.p2p_tx_packets = 12;
        global.p2p_tx_bytes = 1_200;
        global.signaling_messages_sent = 7;
        global.signaling_reconnects = 1;

        let peer = runtime_peer();
        StatusSnapshot {
            assigned_ip: Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0x10, 1),
            p2p_listen_addr: "0.0.0.0:40000".parse().unwrap(),
            uptime_secs: 90,
            global: global.clone(),
            peers: vec![peer.clone()],
            network_name: Some("office".to_string()),
            tun_name: Some("fuckipv6-office".to_string()),
            networks: vec![NetworkSessionSnapshot {
                network_name: "office".to_string(),
                tun_name: "fuckipv6-office".to_string(),
                assigned_ip: Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0x10, 1),
                assigned_ipv4: Some(Ipv4Addr::new(10, 8, 0, 1)),
                p2p_listen_addr: "0.0.0.0:40000".parse().unwrap(),
                uptime_secs: 90,
                peers: vec![peer],
                global,
                dataplane: Default::default(),
            }],
            dataplane: Some(Default::default()),
        }
    }

    #[test]
    fn runtime_status_updates_ui_identity_networks_peers_and_traffic() {
        let mut state = AppUiState::offline();
        apply_runtime_status(&mut state, &active_status());

        assert!(state.runtime_available);
        assert_eq!(state.runtime_p2p_addr.as_deref(), Some("0.0.0.0:40000"));
        assert_eq!(state.status, "online");
        assert_eq!(state.virtual_ipv6.as_deref(), Some("fd00::10:1"));
        assert_eq!(state.virtual_ipv4.as_deref(), Some("10.8.0.1"));
        assert_eq!(state.network_name.as_deref(), Some("office"));
        assert_eq!(state.networks.len(), 1);
        assert_eq!(state.networks[0].peer_count, 1);
        assert_eq!(state.peers.len(), 1);
        assert_eq!(state.peers[0].virtual_ipv4.as_deref(), Some("10.8.0.2"));
        assert_eq!(state.peers[0].network_name.as_deref(), Some("office"));

        let traffic = state.runtime_traffic.expect("runtime traffic snapshot");
        assert_eq!(traffic.uptime_secs, 90);
        assert_eq!(traffic.tun_rx_bytes, 2_000);
        assert_eq!(traffic.p2p_tx_bytes, 1_200);
        assert_eq!(traffic.signaling_messages_sent, 7);
    }

    #[test]
    fn ipc_peer_conversion_preserves_runtime_link_metadata() {
        let peer = runtime_peer();
        let snapshot = ipc_peer_to_snapshot(&peer, "default");

        assert_eq!(snapshot.virtual_ipv4, Some(Ipv4Addr::new(10, 8, 0, 2)));
        assert_eq!(snapshot.endpoints, peer.endpoints);
        assert_eq!(snapshot.nat_type, "Full Cone");
        assert_eq!(snapshot.connected_secs, 42);
        assert_eq!(snapshot.network_name.as_deref(), Some("office"));
    }

    #[test]
    fn empty_runtime_status_does_not_mark_client_online() {
        let mut state = AppUiState::offline();
        let status = StatusSnapshot {
            assigned_ip: Ipv6Addr::UNSPECIFIED,
            p2p_listen_addr: SocketAddr::from(([0, 0, 0, 0], 0)),
            uptime_secs: 0,
            global: zero_global(),
            peers: Vec::new(),
            network_name: None,
            tun_name: None,
            networks: Vec::new(),
            dataplane: None,
        };

        apply_runtime_status(&mut state, &status);

        assert!(!state.runtime_available);
        assert_eq!(state.status, "offline");
        assert!(state.runtime_p2p_addr.is_none());
        assert!(state.runtime_traffic.is_none());
        assert!(state.peers.is_empty());
    }
}
